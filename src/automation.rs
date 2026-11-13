//! The debug-only automation channel (grafida/grafida#82): the one uniform way to drive
//! the real window from a script, since an embedded WebKitGTK or WKWebView cannot be attached to
//! by Playwright or WebDriver.
//!
//! **It does not exist in a release build.** It is behind the `automation` Cargo feature and
//! `debug_assertions`, and even then it opens only with `--automation`.
//!
//! The listener binds `127.0.0.1` on an ephemeral port and writes `{port, token, pid}` to
//! `$XDG_RUNTIME_DIR/<id>.automation.json` (else the temp dir; mode 0600, removed on exit). A client
//! sends one JSON object per line and gets one JSON line back; every command must carry the token.
//!
//! The file is never written through a symlink: a stale entry at the path (a shell killed by a
//! signal leaves its file) is removed, then the file is created with `O_EXCL`. On Unix a
//! directory that other users can write to (a bare `/tmp`) is refused with an error that says to
//! set `XDG_RUNTIME_DIR` to a private directory. macOS `$TMPDIR` and Windows `%TEMP%` are per-user.
//!
//! ```text
//! {"token":"…","cmd":"eval","js":"return location.origin"}  → {"ok":true,"result":"shell://app"}
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

/// Creates the runtime file, never through a symlink and never in a directory other users can
/// write to. A shell killed by a signal leaves its file behind, so any existing entry is removed
/// first (a symlink is removed, not followed), and the file is then created with `O_EXCL`: if
/// something reappears in between, this fails rather than writing through it.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        if fs::metadata(dir)?.permissions().mode() & 0o002 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to put the automation runtime file in {}, which other users can write to: set XDG_RUNTIME_DIR to a private directory",
                    dir.display()
                ),
            ));
        }
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, private scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "grafida-automation-test-{tag}-{}-{}",
                std::process::id(),
                crate::tokens::random_hex(4)
            ));
            fs::create_dir(&dir).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fresh_runtime_file_is_written() {
        let dir = Scratch::new("fresh");
        let path = dir.0.join("app.automation.json");
        write_private(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_is_replaced_and_its_target_left_alone() {
        let dir = Scratch::new("symlink");
        let victim = dir.0.join("victim");
        fs::write(&victim, "precious").unwrap();
        let path = dir.0.join("app.automation.json");
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        write_private(&path, "new").unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_file_is_replaced_with_a_private_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("stale");
        let path = dir.0.join("app.automation.json");
        fs::write(
            &path,
            "left behind by a killed shell, and longer than the new text",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, "new").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_world_writable_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o777, 0o1777] {
            let dir = Scratch::new("shared");
            fs::set_permissions(&dir.0, fs::Permissions::from_mode(mode)).unwrap();
            let path = dir.0.join("app.automation.json");

            let error = write_private(&path, "new").unwrap_err();

            let message = error.to_string();
            assert!(message.contains("XDG_RUNTIME_DIR"), "{message}");
            assert!(message.contains(&dir.0.display().to_string()), "{message}");
            assert!(!path.exists());
        }
    }
}
