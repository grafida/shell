//! Process-wide settings, made from `main()` before any thread exists (grafida/grafida#91).

use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
};

use crate::manifest::Manifest;

/// The variables through which anyone who can set our environment could change the browser
/// WebView2 runs: its command line (`--remote-debugging-port`), its executable, its profile and
/// which channel it picks. A release shell removes them before it creates the environment, so a
/// release build never opens a debugging port it was not built to open (grafida/grafida#91). The
/// admin-only `Software\Policies\Microsoft\Edge\WebView2` policies are out of reach and out of scope.
pub const BROWSER_VARIABLES: [&str; 5] = [
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    "WEBVIEW2_USER_DATA_FOLDER",
    "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
    "WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER",
];

/// Applies the process-wide settings and returns what it did, for the log.
///
/// - A release build removes [`BROWSER_VARIABLES`]; a debug build keeps them, for development.
/// - The shell is a GUI-subsystem program, so it has no console. Started from a terminal with no
///   redirected stderr, it attaches to that terminal's console so `--dev` logging shows there.
///
/// # Safety
///
/// Must be called before any other thread is started: it changes environment variables.
pub unsafe fn prepare(_manifest: &Manifest) -> Vec<String> {
    let mut notes = Vec::new();

    let present: Vec<&str> = BROWSER_VARIABLES
        .iter()
        .copied()
        .filter(|v| std::env::var_os(v).is_some())
        .collect();
    if cfg!(debug_assertions) {
        if !present.is_empty() {
            notes.push(format!(
                "debug build: WebView2 variables kept: {}",
                present.join(", ")
            ));
        }
    } else {
        for v in &present {
            // SAFETY: the caller guarantees no other thread exists yet.
            unsafe { std::env::remove_var(v) };
        }
        if !present.is_empty() {
            notes.push(format!(
                "WebView2 variables removed: {}",
                present.join(", ")
            ));
        }
    }

    // SAFETY: plain Win32 calls; a failure only means there is no console to attach to.
    unsafe {
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        if stderr.map_or(true, |h| h.is_invalid() || h.0.is_null()) {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    notes
}
