//! Native failure alerts (`NSAlert`). Used before the window exists (an invalid manifest, a missing
//! worker) and when the app cannot go on (a worker that never sends `ready`, or keeps crashing).
//! stderr is not a user-facing channel for a macOS app (a bundle's goes to unified logging), so these
//! always show an alert; the same text also goes to stderr and the log.
//!
//! `SHELL_ALERTS=stderr` keeps them to stderr (tests, headless smoke runs), and so does a process
//! with no window-server session to show one in (started over SSH outside the desktop session).

use block2::RcBlock;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationPolicy, NSFont, NSModalResponse,
    NSScrollView, NSTextView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tao::{platform::macos::WindowExtMacOS, window::Window};

unsafe extern "C" {
    /// CoreGraphics: the current window-server session, or NULL when the process has none.
    fn CGSessionCopyCurrentDictionary() -> *const std::ffi::c_void;
    fn CFRelease(cf: *const std::ffi::c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

/// Whether an alert can be shown at all.
pub fn can_show() -> bool {
    if std::env::var("SHELL_ALERTS").is_ok_and(|v| v == "stderr") {
        return false;
    }
    // SAFETY: a plain query; the dictionary it returns is released at once.
    unsafe {
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return false;
        }
        CFRelease(session);
    }
    true
}

/// Shows a modal alert without a parent window. For use **before** the event loop runs: inside
/// tao's event handler, [`show`] must be used instead (a modal run loop would re-enter it).
pub fn alert(title: &str, message: &str, detail: &str) {
    eprintln!("{title}: {message}\n{detail}");
    if !can_show() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = build(mtm, title, message, detail);
    let _: NSModalResponse = alert.runModal();
}

/// The same alert as a sheet on the app window, **without blocking**. `on_close` runs when the user
/// dismisses it.
pub fn show(
    window: &Window,
    title: &str,
    message: &str,
    detail: &str,
    on_close: impl Fn() + 'static,
) {
    eprintln!("{title}: {message}\n{detail}");
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !can_show() {
        on_close();
        return;
    }
    let alert = build(mtm, title, message, detail);
    // SAFETY: tao's NSWindow, alive as long as `window`.
    let ns_window: &NSWindow = unsafe { &*(window.ns_window() as *const NSWindow) };
    let handler = RcBlock::new(move |_: NSModalResponse| on_close());
    alert.beginSheetModalForWindow_completionHandler(ns_window, Some(&handler));
}

/// An error alert with the detail (e.g. a stderr tail) in a scrollable monospace pane.
fn build(mtm: MainThreadMarker, title: &str, message: &str, detail: &str) -> Retained<NSAlert> {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(message));
    alert.window().setTitle(&NSString::from_str(title));
    if !detail.trim().is_empty() {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(560.0, 220.0));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
        text.setEditable(false);
        text.setString(&NSString::from_str(detail));
        if let Some(font) = NSFont::userFixedPitchFontOfSize(11.0) {
            text.setFont(Some(&font));
        }
        scroll.setDocumentView(Some(&text));
        alert.setAccessoryView(Some(&scroll));
    }
    alert
}
