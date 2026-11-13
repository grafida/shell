//! Menu key equivalents and the page (grafida/grafida#88).
//!
//! WKWebView claims every Cmd/Ctrl key in `performKeyEquivalent:` and hands it to the page. As a
//! subview of tao's content view, a key the page leaves unhandled never comes back to the menu bar,
//! so without help no menu shortcut would fire at all. The shell therefore routes them itself:
//!
//! - **Edit shortcuts go to the page first** (undo, redo, cut, copy, paste, select all, the
//!   Spelling ones): a rich editor keeps its own undo history and handles Cmd+Z itself, and the
//!   webview's native Undo knows nothing of what the editor did by script. The key is parked and
//!   passed to WebKit; a document-start script in every frame reports, once the page's handlers have
//!   run, whether the page cancelled it (`defaultPrevented`). If it did not, the parked key goes to
//!   the menu bar, so a plain text field still gets the native command. If no report comes within
//!   [`REPORT_TIMEOUT`] (a frame without the script, a page too busy to answer), the menu bar gets it
//!   then.
//! - **App and Window shortcuts go to the menu bar first** (Quit, Hide, Minimize, …): a page must
//!   not be able to keep the app from quitting.
//! - **Anything the menu bar does not bind goes to the page as before** (Cmd+Shift+V, an editor's
//!   Cmd+B/I/K, …). The menu never binds a reserved chord.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Mutex,
    time::Duration,
};

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKUserContentController,
};
use tao::event_loop::EventLoopProxy;

use super::UserEvent;
use crate::menu::Chord;

/// How long a parked key waits for the page's report before the menu bar gets it anyway.
pub const REPORT_TIMEOUT: Duration = Duration::from_secs(1);

/// The script message handler's name.
pub const HANDLER: &str = "shellKey";

/// The content world the reporter and its handler live in. Page script, the app's or an embed's, runs
/// in the page world and cannot see the handler there: before, any frame could post `true` to eat
/// every parked Edit key, or `false` to make the menu act on keys the page cancelled (security.md
/// L4). The reporter still sees the page's `defaultPrevented`: the event is the same DOM object in
/// every world. macOS 11+, below the shell's floor of 12.
pub fn world(mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    // SAFETY: a plain class method; the name is ours.
    unsafe { WKContentWorld::worldWithName(&NSString::from_str("grafida-shell-keys"), mtm) }
}

/// The menu actions whose key equivalents the page gets first.
const PAGE_FIRST: &[&str] = &[
    "undo:",
    "redo:",
    "cut:",
    "copy:",
    "paste:",
    "selectAll:",
    "showGuessPanel:",
    "checkSpelling:",
];

/// The document-start script, in every frame: after each trusted Cmd/Ctrl key-down has been
/// dispatched (a timer, so every handler has run), tell the shell whether the page cancelled it.
pub const REPORTER: &str = r#"(() => {
  addEventListener("keydown", (e) => {
    if (!e.isTrusted || !(e.metaKey || e.ctrlKey)) return;
    setTimeout(() => {
      try { webkit.messageHandlers.shellKey.postMessage(e.defaultPrevented); } catch (_) {}
    }, 0);
  }, true);
})();"#;

/// The key waiting for the page's verdict. Main thread only.
pub struct KeyState {
    pending: RefCell<Option<(u64, Retained<NSEvent>)>>,
    seq: Cell<u64>,
    proxy: Mutex<EventLoopProxy<UserEvent>>,
}

impl KeyState {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Rc<KeyState> {
        Rc::new(KeyState {
            pending: RefCell::default(),
            seq: Cell::new(0),
            proxy: Mutex::new(proxy),
        })
    }

    /// `performKeyEquivalent:` on the webview, before WebKit sees the key: `Some(true)` when the
    /// menu bar took it, `Some(false)` when it is parked for the page's verdict (pass it to WebKit),
    /// `None` when it is none of the menu's business.
    pub fn route(&self, mtm: MainThreadMarker, event: &NSEvent) -> Option<bool> {
        let flags = event.modifierFlags();
        if !flags.intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control) {
            return None;
        }
        let bar = NSApplication::sharedApplication(mtm).mainMenu()?;
        let action = find(&bar, &event_chord(event))?;
        if !PAGE_FIRST.contains(&action.as_str()) {
            return Some(bar.performKeyEquivalent(event));
        }
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        *self.pending.borrow_mut() = Some((seq, event.retain()));
        let proxy = self.proxy.lock().unwrap().clone();
        std::thread::spawn(move || {
            std::thread::sleep(REPORT_TIMEOUT);
            let _ = proxy.send_event(UserEvent::KeyTimeout(seq));
        });
        Some(false)
    }

    /// The page's verdict on the parked key.
    fn report(&self, mtm: MainThreadMarker, prevented: bool) {
        let Some((_, event)) = self.pending.borrow_mut().take() else {
            return; // a key the menu has no business with, or one already timed out
        };
        if !prevented {
            perform(mtm, &event);
        }
    }

    /// No verdict came in time: the menu bar gets the key.
    pub fn timeout(&self, mtm: MainThreadMarker, seq: u64) {
        let event = {
            let mut pending = self.pending.borrow_mut();
            match pending.as_ref() {
                Some((s, _)) if *s == seq => pending.take().map(|(_, e)| e),
                _ => None,
            }
        };
        if let Some(event) = event {
            perform(mtm, &event);
        }
    }
}

fn perform(mtm: MainThreadMarker, event: &NSEvent) {
    if let Some(bar) = NSApplication::sharedApplication(mtm).mainMenu() {
        bar.performKeyEquivalent(event);
    }
}

use objc2::Message;

/// An event's chord as a menu item's would read. Shift is part of a letter's chord (Cmd+Z is not
/// Cmd+Shift+Z) but not of a punctuation key's, whose character already carries it (Cmd+: is typed
/// as Cmd+Shift+;).
fn event_chord(event: &NSEvent) -> Chord {
    let key = event
        .charactersIgnoringModifiers()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let flags = event.modifierFlags();
    let letter = key.chars().any(char::is_alphabetic);
    Chord::from_appkit(
        &key,
        flags.contains(NSEventModifierFlags::Command),
        flags.contains(NSEventModifierFlags::Control),
        flags.contains(NSEventModifierFlags::Option),
        letter && flags.contains(NSEventModifierFlags::Shift),
    )
}

fn item_chord(item: &NSMenuItem) -> Chord {
    let key = item.keyEquivalent().to_string();
    let mask = item.keyEquivalentModifierMask();
    let letter = key.chars().any(char::is_alphabetic);
    Chord::from_appkit(
        &key,
        mask.contains(NSEventModifierFlags::Command),
        mask.contains(NSEventModifierFlags::Control),
        mask.contains(NSEventModifierFlags::Option),
        letter && mask.contains(NSEventModifierFlags::Shift),
    )
}

/// The action of the live menu item bound to `chord`, AppKit's own items included.
fn find(menu: &NSMenu, chord: &Chord) -> Option<String> {
    for item in menu.itemArray().iter() {
        if let Some(sub) = item.submenu()
            && let Some(found) = find(&sub, chord)
        {
            return Some(found);
        }
        if !item.keyEquivalent().to_string().is_empty() && item_chord(&item) == *chord {
            return Some(
                item.action()
                    .map(|s| s.name().to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
    }
    None
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivar is only touched on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaShellKeyReporter"]
    #[ivars = Rc<KeyState>]
    pub struct KeyReporter;

    unsafe impl NSObjectProtocol for KeyReporter {}

    unsafe impl WKScriptMessageHandler for KeyReporter {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            let prevented = body.downcast_ref::<NSNumber>().is_none_or(|n| n.as_bool());
            self.ivars().report(self.mtm(), prevented);
        }
    }
);

impl KeyReporter {
    pub fn new(mtm: MainThreadMarker, keys: Rc<KeyState>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(keys);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Registers the handler in the reporter's own [`world`], never the page's.
    pub fn install(&self, controller: &WKUserContentController) {
        unsafe {
            controller.addScriptMessageHandler_contentWorld_name(
                ProtocolObject::from_ref(self),
                &world(self.mtm()),
                &NSString::from_str(HANDLER),
            );
        }
    }
}
