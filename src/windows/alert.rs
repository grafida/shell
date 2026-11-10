//! Native failure alerts (`MessageBoxW`). Used before the window exists (an invalid manifest, a
//! missing WebView2 runtime) and when the app cannot go on (the worker keeps crashing). A
//! GUI-subsystem process has no console, so stderr is not a user-facing channel; the same text also
//! goes to the log. `SHELL_ALERTS=stderr` keeps them to stderr (tests, headless smoke runs).

use windows::{
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW},
    },
    core::HSTRING,
};

/// The longest stderr tail shown in a box: a message box does not scroll.
const DETAIL_LINES: usize = 40;

fn to_stderr_only() -> bool {
    std::env::var("SHELL_ALERTS").is_ok_and(|v| v == "stderr")
}

/// Shows an alert without a parent window. Blocks until it is dismissed.
pub fn alert(title: &str, message: &str, detail: &str) {
    eprintln!("{title}: {message}\n{detail}");
    if to_stderr_only() {
        return;
    }
    show_box(None, title, message, detail);
}

/// The same alert over the app window. `MessageBoxW` runs its own modal loop, which keeps the
/// window painting; `on_close` runs once it is dismissed.
pub fn show(parent: HWND, title: &str, message: &str, detail: &str, on_close: impl FnOnce()) {
    eprintln!("{title}: {message}\n{detail}");
    if !to_stderr_only() {
        show_box(Some(parent), title, message, detail);
    }
    on_close();
}

fn show_box(parent: Option<HWND>, title: &str, message: &str, detail: &str) {
    let detail: Vec<&str> = detail.lines().collect();
    let skip = detail.len().saturating_sub(DETAIL_LINES);
    let text = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}\n\n{}", detail[skip..].join("\n"))
    };
    // SAFETY: plain Win32 call with valid, NUL-terminated strings that outlive it.
    unsafe {
        MessageBoxW(
            parent,
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}
