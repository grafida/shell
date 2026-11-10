//! The debug-only automation channel (grafida/grafida#82, Phase 2): the one uniform way to drive
//! the real window from a script, since an embedded WebKitGTK or WKWebView cannot be attached to
//! by Playwright or WebDriver.
//!
//! **It does not exist in a release build.** It is behind the `automation` Cargo feature and
//! `debug_assertions`, and even then it opens only with `--automation`.
//!
//! The listener binds `127.0.0.1` on an ephemeral port and writes `{port, token, pid}` to
//! `$XDG_RUNTIME_DIR/<id>.automation.json` (mode 0600, removed on exit). A client sends one JSON
//! object per line and gets one JSON line back; every command must carry the token.
//!
//! ```text
//! {"token":"…","cmd":"eval","js":"return location.origin"}  → {"ok":true,"result":"boson://app"}
//! {"token":"…","cmd":"screenshot","path":"/tmp/x.png"}       → {"ok":true}
//! {"token":"…","cmd":"pick","paths":["/tmp/a.png"]}          → {"ok":true}   (next dialog answers this)
//! {"token":"…","cmd":"pick","paths":null}                    → {"ok":true}   (next dialog is cancelled)
//! {"token":"…","cmd":"opened"}                               → {"ok":true,"result":["https://…"]}
//! {"token":"…","cmd":"drop","paths":["/tmp/a.png"],"x":400,"y":300} → {"ok":true}  (as if dropped)
//! {"token":"…","cmd":"menu"}                                  → {"ok":true,"result":[{title,key,…}]}  (macOS)
//! {"token":"…","cmd":"key","key":"v","modifiers":["cmd"]}     → {"ok":true}  (macOS: a queued key press)
//! ```
//!
//! Windows adds real input through `SendInput`, into the window brought to the foreground:
//!
//! ```text
//! {"token":"…","cmd":"type","text":"Ελληνικά"}                → {"ok":true}  (one Unicode key press per character)
//! {"token":"…","cmd":"mouse","x":120,"y":80,"button":"right","modifiers":["ctrl"]} → {"ok":true,"result":{"screen":[x,y]}}
//! {"token":"…","cmd":"drag","from":[300,200],"x":800,"y":400}  → {"ok":true,…}  (from a screen point to the page)
//! {"token":"…","cmd":"click","x":120,"y":80}                  → {"ok":true}  (DevTools input, not SendInput)
//! {"token":"…","cmd":"window"}                                → {"ok":true,"result":{title,frame,client,dpi,darkTitleBar,foreground}}
//! {"token":"…","cmd":"record","on":false}                     → {"ok":true}  (external URLs really open)
//! ```
//!
//! Page coordinates are CSS pixels at 100 % zoom, scaled by the window's DPI.
//!
//! `key` activates the app and makes its window key first (it takes the keyboard focus), then
//! queues the event, so it travels the path a real key press does: the menu bar's key equivalents,
//! then the page.
//!
//! While the channel is open the shell **records** the URLs it would hand to the OS browser instead
//! of opening them (`opened` returns and clears the list), so an automated run never launches a
//! browser on the desktop it runs on.
//!
//! `eval`'s `js` is an async function body: `return await fetch(…).then(r => r.status)` works.
//! `scripts/dev/automation.py` is a client.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    time::Duration,
};

use serde_json::{Value, json};

/// A command for the UI thread, with the channel its JSON answer goes back on.
pub struct Command {
    pub request: Value,
    pub reply: Sender<Value>,
}

/// The open channel. Dropping it removes the runtime file — but the event loop ends the process
/// without dropping anything, so the platform layer also calls [`Channel::close`] as it stops.
pub struct Channel {
    file: PathBuf,
}

impl Channel {
    /// Removes the runtime file, so no client (or test script) mistakes it for a live shell.
    pub fn close(&self) {
        let _ = fs::remove_file(&self.file);
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.close();
    }
}

/// Where a client finds the channel for an app id.
pub fn runtime_file(id: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{id}.automation.json"))
}

/// Opens the listener. `dispatch` hands each authenticated command to the UI thread.
pub fn open(id: &str, dispatch: impl Fn(Command) + Send + 'static) -> std::io::Result<Channel> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = crate::tokens::random_hex(16);
    let file = runtime_file(id);
    write_private(
        &file,
        &json!({"port": port, "token": token, "pid": std::process::id()}).to_string(),
    )?;

    std::thread::Builder::new()
        .name("automation".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &token, &dispatch);
            }
        })?;

    Ok(Channel { file })
}

fn serve(stream: TcpStream, token: &str, dispatch: &impl Fn(Command)) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(request) if request.get("token").and_then(Value::as_str) == Some(token) => {
                let (reply, answer) = mpsc::channel();
                dispatch(Command { request, reply });
                answer
                    .recv_timeout(Duration::from_secs(120))
                    .unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}))
            }
            Ok(_) => json!({"ok": false, "error": "bad_token"}),
            Err(e) => json!({"ok": false, "error": format!("bad_json: {e}")}),
        };
        if writeln!(writer, "{answer}").is_err() {
            return;
        }
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())
}
