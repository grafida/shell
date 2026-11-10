//! The macOS menu bar, built with AppKit from [`crate::menu::standard`] (muda has no Spelling
//! selectors; grafida/grafida#88).

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::Sel, sel,
};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use tao::event_loop::EventLoopProxy;

use super::UserEvent;
use crate::menu::{self, Action, Chord, Entry, Role, Submenu};

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivar is only touched on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaShellMenuTarget"]
    #[ivars = EventLoopProxy<UserEvent>]
    pub struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(shellAbout:))]
        fn about(&self, _sender: Option<&NSObject>) {
            let _ = self.ivars().send_event(UserEvent::About);
        }

        #[unsafe(method(shellQuit:))]
        fn quit(&self, _sender: Option<&NSObject>) {
            let _ = self.ivars().send_event(UserEvent::Quit);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, proxy: EventLoopProxy<UserEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(proxy);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Installs the standard menu bar. Returns the target the About and Quit items point at, which
/// must be kept alive (a menu item does not retain its target).
pub fn install(
    mtm: MainThreadMarker,
    app_title: &str,
    reserved: &[String],
    proxy: EventLoopProxy<UserEvent>,
) -> Retained<MenuTarget> {
    let target = MenuTarget::new(mtm, proxy);
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    for submenu in menu::standard(app_title, reserved) {
        let (top, built) = build_submenu(mtm, &app, &target, &submenu);
        top.setSubmenu(Some(&built));
        bar.addItem(&top);
    }
    app.setMainMenu(Some(&bar));
    target
}

fn build_submenu(
    mtm: MainThreadMarker,
    app: &NSApplication,
    target: &MenuTarget,
    submenu: &Submenu,
) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
    let title = NSString::from_str(&submenu.title);
    let top = NSMenuItem::new(mtm);
    top.setTitle(&title);
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
    for entry in &submenu.entries {
        match entry {
            Entry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            Entry::Submenu(inner) => {
                let (item, built) = build_submenu(mtm, app, target, inner);
                item.setSubmenu(Some(&built));
                menu.addItem(&item);
            }
            Entry::Item(spec) => {
                let (action, own): (Sel, bool) = match spec.action {
                    Action::Selector(name) => (Sel::register(&cstr(name)), false),
                    Action::About => (sel!(shellAbout:), true),
                    Action::Quit => (sel!(shellQuit:), true),
                };
                let (key, mask) = match &spec.key {
                    Some(chord) => appkit_key(chord),
                    None => (String::new(), NSEventModifierFlags::empty()),
                };
                // SAFETY: a plain initialiser; the selector exists on the target or the responder
                // chain (AppKit disables the item otherwise).
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(&spec.title),
                        Some(action),
                        &NSString::from_str(&key),
                    )
                };
                if !key.is_empty() {
                    item.setKeyEquivalentModifierMask(mask);
                }
                if own {
                    // SAFETY: the target outlives the menu (the caller keeps it).
                    unsafe { item.setTarget(Some(target)) };
                }
                menu.addItem(&item);
            }
        }
    }
    match submenu.role {
        Role::Services => app.setServicesMenu(Some(&menu)),
        Role::Window => app.setWindowsMenu(Some(&menu)),
        Role::App | Role::Edit | Role::Spelling => {}
    }
    (top, menu)
}

fn cstr(name: &str) -> std::ffi::CString {
    std::ffi::CString::new(name).expect("selector names have no NUL")
}

/// A chord as an AppKit key equivalent: always lowercase, with Shift in the mask.
fn appkit_key(chord: &Chord) -> (String, NSEventModifierFlags) {
    let mut mask = NSEventModifierFlags::empty();
    if chord.cmd {
        mask |= NSEventModifierFlags::Command;
    }
    if chord.ctrl {
        mask |= NSEventModifierFlags::Control;
    }
    if chord.alt {
        mask |= NSEventModifierFlags::Option;
    }
    if chord.shift {
        mask |= NSEventModifierFlags::Shift;
    }
    (chord.key.clone(), mask)
}

/// Walks the live menu bar, items AppKit added included, and removes the key equivalent of any item
/// that would take a reserved chord from the page. Returns the titles it changed.
pub fn strip_reserved(mtm: MainThreadMarker, reserved: &[String]) -> Vec<String> {
    let reserved: Vec<Chord> = reserved
        .iter()
        .filter_map(|c| Chord::parse_mac(c))
        .collect();
    let mut changed = Vec::new();
    if reserved.is_empty() {
        return changed;
    }
    if let Some(bar) = NSApplication::sharedApplication(mtm).mainMenu() {
        walk(&bar, &reserved, &mut changed);
    }
    changed
}

fn walk(menu: &NSMenu, reserved: &[Chord], changed: &mut Vec<String>) {
    for item in menu.itemArray().iter() {
        if let Some(sub) = item.submenu() {
            walk(&sub, reserved, changed);
        }
        let key = item.keyEquivalent().to_string();
        let mask = item.keyEquivalentModifierMask();
        let chord = Chord::from_appkit(
            &key,
            mask.contains(NSEventModifierFlags::Command),
            mask.contains(NSEventModifierFlags::Control),
            mask.contains(NSEventModifierFlags::Option),
            mask.contains(NSEventModifierFlags::Shift),
        );
        if chord.is_reserved(reserved) {
            item.setKeyEquivalent(&NSString::from_str(""));
            changed.push(item.title().to_string());
        }
    }
}
