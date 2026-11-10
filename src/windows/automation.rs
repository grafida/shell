//! The automation channel's commands on WebView2 (debug builds with `automation` only; the channel
//! itself is `src/automation.rs`). grafida/grafida#91.
//!
//! - `eval`: `Runtime.evaluate` through `CallDevToolsProtocolMethod`, **in process**: no debugging
//!   port is opened, in this or any build. `awaitPromise` makes `js` an async function body, as on
//!   Linux and macOS. It runs with a user gesture by default (`"gesture": false` runs without one),
//!   as an injected script does in WebKit; note that Chromium's transient activation lasts about
//!   five seconds, not WebKit's one.
//! - `screenshot`: `CapturePreview` to a PNG.
//! - `pick`, `opened`: as on the other platforms.
//! - `drop`: WebView2 hands a real drop to the page as `File` objects (the shell does not take it),
//!   so this delivers exactly that: the files, read here, as `File`s in a `DataTransfer`, with
//!   `dragenter`/`dragover`/`drop` dispatched at the point (inside a same-origin `<iframe>` too).
//!   It tests the page's handling and the bodies it sends; a real drag from Explorer is a by-hand
//!   check.
//! - `key`: a key press through `SendInput`, after bringing the window to the foreground, so it
//!   travels the path a real one does (WebView2's accelerator handling, then the page).

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use serde_json::{Value, json};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2,
    },
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{STGM_CREATE, STGM_READWRITE},
            Threading::{AttachThreadInput, GetCurrentThreadId},
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
                VIRTUAL_KEY, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F5, VK_F12,
                VK_HOME, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
                VkKeyScanW,
            },
            Shell::SHCreateStreamOnFileEx,
            WindowsAndMessaging::{
                BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
                SetForegroundWindow,
            },
        },
    },
    core::HSTRING,
};

use super::RECORDED_OPENS;
use crate::{automation::Command, dialog::Picks};

pub fn run(webview: Option<&ICoreWebView2>, hwnd: HWND, picks: &Picks, command: Command) {
    let Command { request, reply } = command;
    let fail = |e: String| json!({"ok": false, "error": e});
    let paths = |key: &str| -> Option<Vec<PathBuf>> {
        request.get(key)?.as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
    };
    match request.get("cmd").and_then(Value::as_str) {
        Some("pick") => {
            picks.lock().unwrap().push_back(paths("paths"));
            let _ = reply.send(json!({"ok": true}));
        }
        Some("record") => {
            // On (the default while the channel is open): external URLs are recorded, not
            // opened. Off: the shell really hands them to the OS browser, for checking that path.
            let on = request.get("on").and_then(Value::as_bool).unwrap_or(true);
            *RECORDED_OPENS.lock().unwrap() = on.then(Vec::new);
            let _ = reply.send(json!({"ok": true, "result": on}));
        }
        Some("type") => {
            let text = request.get("text").and_then(Value::as_str).unwrap_or("");
            let _ = reply.send(match type_text(hwnd, text) {
                Ok(()) => json!({"ok": true}),
                Err(e) => fail(e),
            });
        }
        Some("window") => {
            let _ = reply.send(json!({"ok": true, "result": window_info(hwnd)}));
        }
        Some(cmd @ ("mouse" | "drag")) => {
            let num = |k: &str| request.get(k).and_then(Value::as_f64);
            let button = request
                .get("button")
                .and_then(Value::as_str)
                .unwrap_or("left");
            let mods: Vec<String> = request
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_lowercase)
                        .collect()
                })
                .unwrap_or_default();
            let answer = if cmd == "mouse" {
                mouse_click(
                    hwnd,
                    num("x").unwrap_or(0.0),
                    num("y").unwrap_or(0.0),
                    button,
                    &mods,
                )
            } else {
                let from = request.get("from").and_then(Value::as_array).map(|a| {
                    (
                        a.first().and_then(Value::as_i64).unwrap_or(0) as i32,
                        a.get(1).and_then(Value::as_i64).unwrap_or(0) as i32,
                    )
                });
                match from {
                    Some(from) => {
                        drag(hwnd, from, num("x").unwrap_or(0.0), num("y").unwrap_or(0.0))
                    }
                    None => Err("`from` must be [screen x, screen y]".into()),
                }
            };
            let _ = reply.send(match answer {
                Ok(at) => json!({"ok": true, "result": {"screen": at}}),
                Err(e) => fail(e),
            });
        }
        Some("opened") => {
            let urls = RECORDED_OPENS
                .lock()
                .unwrap()
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default();
            let _ = reply.send(json!({"ok": true, "result": urls}));
        }
        Some("key") => {
            let key = request.get("key").and_then(Value::as_str).unwrap_or("");
            let mods: Vec<String> = request
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_lowercase)
                        .collect()
                })
                .unwrap_or_default();
            let _ = reply.send(match press(hwnd, key, &mods) {
                Ok(foreground) => json!({"ok": true, "result": {"foreground": foreground}}),
                Err(e) => fail(e),
            });
        }
        Some(cmd @ ("eval" | "screenshot" | "drop" | "click")) => {
            let Some(webview) = webview else {
                let _ = reply.send(fail("the webview does not exist yet".into()));
                return;
            };
            match cmd {
                "eval" => {
                    let js = request
                        .get("js")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let gesture = request
                        .get("gesture")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    evaluate(webview, &js, gesture, reply);
                }
                "click" => {
                    let at = |k: &str| request.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    click(webview, at("x"), at("y"), reply);
                }
                "screenshot" => {
                    let path = request
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    screenshot(webview, &path, reply);
                }
                _ => {
                    let at = |k: &str| request.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    match drop_script(&paths("paths").unwrap_or_default(), at("x"), at("y")) {
                        Ok(js) => evaluate(webview, &js, true, reply),
                        Err(e) => {
                            let _ = reply.send(fail(e));
                        }
                    }
                }
            }
        }
        other => {
            let _ = reply.send(fail(format!("unknown command {other:?}")));
        }
    }
}

/// Runs an async function body in the page and answers with its value.
fn evaluate(
    webview: &ICoreWebView2,
    js: &str,
    gesture: bool,
    reply: std::sync::mpsc::Sender<Value>,
) {
    let params = json!({
        "expression": format!("(async () => {{\n{js}\n}})()"),
        "awaitPromise": true,
        "returnByValue": true,
        "userGesture": gesture,
    });
    let reply2 = reply.clone();
    // SAFETY: COM call on the UI thread; the handler runs on it.
    let started = unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from("Runtime.evaluate"),
            &HSTRING::from(params.to_string()),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |code, result| {
                let answer = match code {
                    Err(e) => json!({"ok": false, "error": e.message()}),
                    Ok(()) => match serde_json::from_str::<Value>(&result) {
                        Ok(v) if v.get("exceptionDetails").is_some() => json!({
                            "ok": false,
                            "error": v["exceptionDetails"]["exception"]["description"]
                                .as_str()
                                .or(v["exceptionDetails"]["text"].as_str())
                                .unwrap_or("exception")
                        }),
                        Ok(v) => json!({"ok": true, "result": v["result"].get("value").cloned().unwrap_or(Value::Null)}),
                        Err(e) => json!({"ok": false, "error": format!("bad DevTools answer: {e}")}),
                    },
                };
                let _ = reply2.send(answer);
                Ok(())
            })),
        )
    };
    if let Err(e) = started {
        let _ = reply.send(json!({"ok": false, "error": e.message()}));
    }
}

/// One DevTools method; `then` gets its answer (or the error), exactly once.
fn devtools(
    webview: &ICoreWebView2,
    method: &str,
    params: &Value,
    then: impl FnOnce(Result<String, String>) + 'static,
) {
    type Then = Box<dyn FnOnce(Result<String, String>)>;
    let then: Rc<RefCell<Option<Then>>> = Rc::new(RefCell::new(Some(Box::new(then))));
    let answer = then.clone();
    // SAFETY: COM call on the UI thread; the handler runs on it.
    let started = unsafe {
        webview.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.to_string()),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |code, result| {
                if let Some(f) = answer.borrow_mut().take() {
                    f(code.map(|()| result).map_err(|e| e.message()));
                }
                Ok(())
            })),
        )
    };
    if let Err(e) = started
        && let Some(f) = then.borrow_mut().take()
    {
        f(Err(e.message()));
    }
}

/// A left click at a point of the page (CSS pixels of the main frame), as real input: DevTools'
/// `Input.dispatchMouseEvent` enters where the browser's own input does, so WebView2 treats what
/// it starts (a `target=_blank` link) as user-initiated, which a script's `el.click()` is not.
fn click(webview: &ICoreWebView2, x: f64, y: f64, reply: std::sync::mpsc::Sender<Value>) {
    let event =
        |kind: &str| json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1});
    let (w, r) = (webview.clone(), reply.clone());
    let released = event("mouseReleased");
    devtools(
        webview,
        "Input.dispatchMouseEvent",
        &event("mousePressed"),
        move |pressed| {
            if let Err(e) = pressed {
                let _ = r.send(json!({"ok": false, "error": e}));
                return;
            }
            devtools(&w, "Input.dispatchMouseEvent", &released, move |done| {
                let _ = r.send(match done {
                    Ok(_) => json!({"ok": true}),
                    Err(e) => json!({"ok": false, "error": e}),
                });
            });
        },
    );
}

fn screenshot(webview: &ICoreWebView2, path: &str, reply: std::sync::mpsc::Sender<Value>) {
    // SAFETY: COM calls on the UI thread; the stream is kept alive by the handler until it runs.
    unsafe {
        let stream = match SHCreateStreamOnFileEx(
            &HSTRING::from(path),
            (STGM_CREATE | STGM_READWRITE).0,
            0,
            true,
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                let _ = reply.send(json!({"ok": false, "error": e.message()}));
                return;
            }
        };
        let keep = stream.clone();
        let reply2 = reply.clone();
        let started = webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &CapturePreviewCompletedHandler::create(Box::new(move |code| {
                drop(keep);
                let _ = reply2.send(match code {
                    Ok(()) => json!({"ok": true}),
                    Err(e) => json!({"ok": false, "error": e.message()}),
                });
                Ok(())
            })),
        );
        if let Err(e) = started {
            let _ = reply.send(json!({"ok": false, "error": e.message()}));
        }
    }
}

/// The page-side half of a real drop, as a script.
fn drop_script(paths: &[PathBuf], x: f64, y: f64) -> Result<String, String> {
    use base64_lite::encode;
    let mut files = Vec::new();
    for path in paths {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        files.push(json!({"name": name, "type": mime(&name), "b64": encode(&bytes)}));
    }
    Ok(format!(
        r#"const files = {files};
const dt = new DataTransfer();
for (const f of files) {{
  const bin = atob(f.b64), bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  dt.items.add(new File([bytes], f.name, {{ type: f.type }}));
}}
let doc = document, x = {x}, y = {y}, target = doc.elementFromPoint(x, y);
while (target && target.tagName === 'IFRAME' && target.contentDocument) {{
  const r = target.getBoundingClientRect();
  x -= r.left; y -= r.top; doc = target.contentDocument; target = doc.elementFromPoint(x, y);
}}
if (!target) throw new Error('nothing at the drop point');
for (const type of ['dragenter', 'dragover', 'drop']) {{
  target.dispatchEvent(new DragEvent(type, {{ bubbles: true, cancelable: true, dataTransfer: dt, clientX: x, clientY: y }}));
}}
return target.tagName;"#,
        files = Value::Array(files)
    ))
}

fn mime(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "css" => "text/css",
        _ => "application/octet-stream",
    }
}

/// The window as the desktop sees it: its title, its frame and client area in screen pixels, its
/// DPI, whether DWM draws its title bar dark (`DWMWA_USE_IMMERSIVE_DARK_MODE`), and whether it is
/// in the foreground.
fn window_info(hwnd: HWND) -> Value {
    use windows::Win32::{
        Foundation::{POINT, RECT},
        Graphics::{Dwm::DwmGetWindowAttribute, Gdi::ClientToScreen},
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{GetClientRect, GetWindowRect, GetWindowTextW},
        },
    };
    // SAFETY: plain Win32 calls on our own window, into buffers that outlive them.
    unsafe {
        let mut frame = RECT::default();
        let _ = GetWindowRect(hwnd, &mut frame);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut title);
        // 20 since Windows 10 20H1; 19 before it.
        let mut dark = windows::core::BOOL::default();
        let read = [20u32, 19].into_iter().any(|attribute| {
            DwmGetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(attribute as i32),
                (&raw mut dark).cast(),
                size_of::<windows::core::BOOL>() as u32,
            )
            .is_ok()
        });
        json!({
            "title": String::from_utf16_lossy(&title[..n.max(0) as usize]),
            "frame": [frame.left, frame.top, frame.right, frame.bottom],
            "client": [origin.x, origin.y, client.right, client.bottom],
            "dpi": GetDpiForWindow(hwnd),
            "darkTitleBar": read.then(|| dark.as_bool()),
            "foreground": GetForegroundWindow() == hwnd,
        })
    }
}

/// A point of the page (CSS pixels at 100 % zoom) on the screen.
fn to_screen(hwnd: HWND, x: f64, y: f64) -> (i32, i32) {
    use windows::Win32::{
        Foundation::POINT, Graphics::Gdi::ClientToScreen, UI::HiDpi::GetDpiForWindow,
    };
    // SAFETY: plain Win32 calls on our own window.
    unsafe {
        let scale = f64::from(GetDpiForWindow(hwnd).max(96)) / 96.0;
        let mut p = POINT {
            x: (x * scale).round() as i32,
            y: (y * scale).round() as i32,
        };
        let _ = ClientToScreen(hwnd, &mut p);
        (p.x, p.y)
    }
}

/// Moves the real pointer (SendInput, absolute) and optionally presses or releases a button.
fn pointer(
    x: i32,
    y: i32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<(), String> {
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::{
            INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
            MOUSEINPUT,
        },
        WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        },
    };
    // SAFETY: plain Win32 calls with an input built here.
    unsafe {
        let (vx, vy) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        let (vw, vh) = (
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        );
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: ((x - vx) * 65535) / (vw - 1),
                    dy: ((y - vy) * 65535) / (vh - 1),
                    dwFlags: MOUSEEVENTF_MOVE
                        | MOUSEEVENTF_ABSOLUTE
                        | MOUSEEVENTF_VIRTUALDESK
                        | flags,
                    ..Default::default()
                },
            },
        };
        if SendInput(&[input], size_of::<INPUT>() as i32) == 1 {
            Ok(())
        } else {
            Err("SendInput refused the mouse input".into())
        }
    }
}

/// A real click (left or right) at a point of the page: what a user's mouse does, so WebView2
/// shows its own context menu for a right click.
fn mouse_click(
    hwnd: HWND,
    x: f64,
    y: f64,
    button: &str,
    mods: &[String],
) -> Result<(i32, i32), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP,
    };
    let (down, up) = match button {
        "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        other => return Err(format!("unknown button {other:?}")),
    };
    let held = modifier_keys(mods)?;
    foreground(hwnd)?;
    let (sx, sy) = to_screen(hwnd, x, y);
    pointer(sx, sy, MOUSE_EVENT_FLAGS(0))?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    keys(&held, false);
    pointer(sx, sy, down)?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    pointer(sx, sy, up)?;
    keys(&held, true);
    Ok((sx, sy))
}

/// Types text as a keyboard would, one Unicode key press per character (`KEYEVENTF_UNICODE`),
/// into whatever has the focus in the window.
fn type_text(hwnd: HWND, text: &str) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;
    foreground(hwnd)?;
    for unit in text.encode_utf16() {
        let input = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wScan: unit,
                    dwFlags: if up {
                        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                    } else {
                        KEYEVENTF_UNICODE
                    },
                    ..Default::default()
                },
            },
        };
        // SAFETY: plain Win32 call with inputs built above.
        let sent = unsafe { SendInput(&[input(false), input(true)], size_of::<INPUT>() as i32) };
        if sent != 2 {
            return Err("SendInput refused a key".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    Ok(())
}

/// The modifier keys by name (`ctrl`, `shift`, `alt`).
fn modifier_keys(mods: &[String]) -> Result<Vec<VIRTUAL_KEY>, String> {
    mods.iter()
        .map(|m| match m.as_str() {
            "ctrl" | "control" | "cmdorctrl" => Ok(VK_CONTROL),
            "shift" => Ok(VK_SHIFT),
            "alt" => Ok(VK_MENU),
            other => Err(format!("unknown modifier {other:?}")),
        })
        .collect()
}

/// Presses a set of keys, or (`up`) releases them in reverse order.
fn keys(held: &[VIRTUAL_KEY], up: bool) {
    let input = |vk: VIRTUAL_KEY| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    };
    let inputs: Vec<INPUT> = if up {
        held.iter().rev().map(|&k| input(k)).collect()
    } else {
        held.iter().map(|&k| input(k)).collect()
    };
    if !inputs.is_empty() {
        // SAFETY: plain Win32 call with inputs built above.
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    }
}

/// A real drag with the left button, from a screen point (another program's window: a file in
/// Explorer) to a point of the page, in small steps, as a hand would move it.
fn drag(hwnd: HWND, from: (i32, i32), x: f64, y: f64) -> Result<(i32, i32), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    };
    let pause = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
    let (tx, ty) = to_screen(hwnd, x, y);
    pointer(from.0, from.1, MOUSE_EVENT_FLAGS(0))?;
    pause(150);
    pointer(from.0, from.1, MOUSEEVENTF_LEFTDOWN)?;
    pause(150);
    // Past the drag threshold first, then across in steps.
    pointer(from.0 + 12, from.1 + 12, MOUSE_EVENT_FLAGS(0))?;
    pause(150);
    let steps = 30;
    for i in 1..=steps {
        let px = from.0 + (tx - from.0) * i / steps;
        let py = from.1 + (ty - from.1) * i / steps;
        pointer(px, py, MOUSE_EVENT_FLAGS(0))?;
        pause(25);
    }
    pause(300);
    pointer(tx, ty, MOUSEEVENTF_LEFTUP)?;
    Ok((tx, ty))
}

/// Brings the window to the foreground (see [`press`]); an error if Windows refuses.
fn foreground(hwnd: HWND) -> Result<(), String> {
    // SAFETY: plain Win32 calls; the input queues are detached again.
    unsafe {
        if GetForegroundWindow() != hwnd {
            let fg = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(fg, None);
            let ours = GetCurrentThreadId();
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if GetForegroundWindow() == hwnd {
            Ok(())
        } else {
            Err("the window is not in the foreground".into())
        }
    }
}

/// Presses a key with modifiers (`ctrl`, `shift`, `alt`). `SendInput` delivers to the foreground
/// window, so the shell's window is brought there first; if Windows' foreground lock still refuses,
/// nothing is sent (it would type into another program) and the command fails.
fn press(hwnd: HWND, key: &str, mods: &[String]) -> Result<bool, String> {
    let vk = virtual_key(key).ok_or_else(|| format!("unknown key {key:?}"))?;
    let mut held = Vec::new();
    for m in mods {
        held.push(match m.as_str() {
            "ctrl" | "control" | "cmdorctrl" => VK_CONTROL,
            "shift" => VK_SHIFT,
            "alt" => VK_MENU,
            other => return Err(format!("unknown modifier {other:?}")),
        });
    }
    let input = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    };
    // SAFETY: plain Win32 calls with inputs built above; the input queues are detached again.
    unsafe {
        if GetForegroundWindow() != hwnd {
            // The foreground lock lets a process take the foreground only from the one that has
            // it: share that window's input queue for the switch.
            let fg = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(fg, None);
            let ours = GetCurrentThreadId();
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let foreground = GetForegroundWindow() == hwnd;
        if !foreground {
            return Err("the window is not in the foreground; no key was sent".into());
        }
        let mut inputs: Vec<INPUT> = held.iter().map(|&m| input(m, false)).collect();
        inputs.push(input(vk, false));
        inputs.push(input(vk, true));
        inputs.extend(held.iter().rev().map(|&m| input(m, true)));
        let sent = SendInput(&inputs, size_of::<INPUT>() as i32);
        if sent as usize != inputs.len() {
            return Err(format!(
                "SendInput delivered {sent} of {} inputs",
                inputs.len()
            ));
        }
        Ok(foreground)
    }
}

fn virtual_key(key: &str) -> Option<VIRTUAL_KEY> {
    let named = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(VK_RETURN),
        "tab" => Some(VK_TAB),
        "space" => Some(windows::Win32::UI::Input::KeyboardAndMouse::VK_SPACE),
        "backspace" => Some(windows::Win32::UI::Input::KeyboardAndMouse::VK_BACK),
        "esc" | "escape" => Some(VK_ESCAPE),
        "delete" => Some(VK_DELETE),
        "left" => Some(VK_LEFT),
        "right" => Some(VK_RIGHT),
        "up" => Some(VK_UP),
        "down" => Some(VK_DOWN),
        "home" => Some(VK_HOME),
        "end" => Some(VK_END),
        "f5" => Some(VK_F5),
        "f12" => Some(VK_F12),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    // Letters and digits have the same virtual key on every layout ('A'..'Z', '0'..'9'); asking
    // the layout would fail for a Latin letter on, say, a Greek keyboard.
    if c.is_ascii_alphanumeric() {
        return Some(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
    }
    // SAFETY: plain Win32 call. The low byte is the virtual key in the current layout.
    let scan = unsafe { VkKeyScanW(c as u16) };
    (scan != -1).then_some(VIRTUAL_KEY((scan as u16) & 0xff))
}

/// A tiny standard base64 encoder: the drop script carries file bytes, and nothing else in the
/// shell needs base64 (the dev-dependency is test-only).
mod base64_lite {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[cfg(test)]
    #[test]
    fn known_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }
}
