//! The standard menu bar (`menu.preset: "standard"`), as data: what the macOS layer builds its
//! `NSMenu`s from (grafida/grafida#88), kept here so the reserved-chord rule is tested on every
//! platform.
//!
//! **A menu accelerator fires before the webview sees the key**, so the shell must never bind a
//! chord the manifest reserves for the page (Grafida: `CmdOrCtrl+Shift+V`, its paste-as-plain-text).
//! [`standard`] drops the key equivalent of any item whose chord is reserved (the item stays, without
//! a shortcut), and the macOS layer applies [`Chord::from_appkit`] to the live menu bar as well,
//! since AppKit adds items of its own.

/// A key chord, normalised: lowercase key, explicit modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// A single character, lowercase for letters.
    pub key: String,
}

impl Chord {
    /// No key and no modifiers.
    pub const fn none() -> Chord {
        Chord {
            cmd: false,
            ctrl: false,
            alt: false,
            shift: false,
            key: String::new(),
        }
    }

    fn with(key: &str, cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Chord {
        Chord {
            cmd,
            ctrl,
            alt,
            shift,
            key: key.to_lowercase(),
        }
    }

    /// A manifest chord (`schema: menu.reservedChords`), as it applies on macOS: `CmdOrCtrl` and
    /// `Super` are Cmd. `None` when it is not a single-key chord.
    pub fn parse_mac(spec: &str) -> Option<Chord> {
        let mut chord = Chord::none();
        let mut parts: Vec<&str> = spec.split('+').collect();
        let key = parts.pop()?;
        if key.chars().count() != 1 {
            return None;
        }
        for part in parts {
            match part {
                "CmdOrCtrl" | "Cmd" | "Super" => chord.cmd = true,
                "Ctrl" => chord.ctrl = true,
                "Alt" => chord.alt = true,
                "Shift" => chord.shift = true,
                _ => return None,
            }
        }
        chord.key = key.to_lowercase();
        Some(chord)
    }

    /// An AppKit key equivalent and modifier mask. **An uppercase key equivalent implies Shift**:
    /// "V" with Cmd is Cmd+Shift+V, whatever the mask says.
    pub fn from_appkit(
        key_equivalent: &str,
        cmd: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> Chord {
        let implied = key_equivalent.chars().any(char::is_uppercase);
        Chord::with(key_equivalent, cmd, ctrl, alt, shift || implied)
    }

    pub fn is_reserved(&self, reserved: &[Chord]) -> bool {
        !self.key.is_empty() && reserved.contains(self)
    }
}

/// What an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// An AppKit action sent down the responder chain (the webview handles the editing ones).
    Selector(&'static str),
    /// Posts a `shell:about` DOM event to the page.
    About,
    /// The shell's own stop path (the worker's graceful shutdown), not `terminate:`.
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    pub action: Action,
    /// `None`: no key equivalent.
    pub key: Option<Chord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item(Item),
    Separator,
    Submenu(Submenu),
}

/// The role a submenu plays for AppKit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    App,
    Services,
    Edit,
    Spelling,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submenu {
    pub title: String,
    pub role: Role,
    pub entries: Vec<Entry>,
}

fn item(title: &str, action: Action, key: Option<Chord>) -> Entry {
    Entry::Item(Item {
        title: title.to_string(),
        action,
        key,
    })
}

fn sel(title: &str, selector: &'static str, key: Option<Chord>) -> Entry {
    item(title, Action::Selector(selector), key)
}

fn cmd(key: &str) -> Option<Chord> {
    Some(Chord::with(key, true, false, false, false))
}

fn cmd_shift(key: &str) -> Option<Chord> {
    Some(Chord::with(key, true, false, false, true))
}

fn cmd_alt(key: &str) -> Option<Chord> {
    Some(Chord::with(key, true, false, true, false))
}

/// The app, Edit and Window menus for an app called `app`, without any chord in `reserved`
/// (manifest strings; ones that do not parse are ignored, the schema has already vetted them).
///
/// `pasteAsPlainText:` (Paste and Match Style) is deliberately absent: the page owns plain-text
/// paste, through the shell's clipboard read.
pub fn standard(app: &str, reserved: &[String]) -> Vec<Submenu> {
    let reserved: Vec<Chord> = reserved
        .iter()
        .filter_map(|c| Chord::parse_mac(c))
        .collect();
    let mut menus = vec![
        Submenu {
            title: app.to_string(),
            role: Role::App,
            entries: vec![
                item(&format!("About {app}"), Action::About, None),
                Entry::Separator,
                Entry::Submenu(Submenu {
                    title: "Services".into(),
                    role: Role::Services,
                    entries: Vec::new(),
                }),
                Entry::Separator,
                sel(&format!("Hide {app}"), "hide:", cmd("h")),
                sel("Hide Others", "hideOtherApplications:", cmd_alt("h")),
                sel("Show All", "unhideAllApplications:", None),
                Entry::Separator,
                item(&format!("Quit {app}"), Action::Quit, cmd("q")),
            ],
        },
        Submenu {
            title: "Edit".into(),
            role: Role::Edit,
            entries: vec![
                sel("Undo", "undo:", cmd("z")),
                sel("Redo", "redo:", cmd_shift("z")),
                Entry::Separator,
                sel("Cut", "cut:", cmd("x")),
                sel("Copy", "copy:", cmd("c")),
                sel("Paste", "paste:", cmd("v")),
                sel("Select All", "selectAll:", cmd("a")),
                Entry::Separator,
                Entry::Submenu(Submenu {
                    title: "Spelling and Grammar".into(),
                    role: Role::Spelling,
                    entries: vec![
                        sel("Show Spelling and Grammar", "showGuessPanel:", cmd(":")),
                        sel("Check Document Now", "checkSpelling:", cmd(";")),
                        Entry::Separator,
                        sel(
                            "Check Spelling While Typing",
                            "toggleContinuousSpellChecking:",
                            None,
                        ),
                        sel(
                            "Check Grammar With Spelling",
                            "toggleGrammarChecking:",
                            None,
                        ),
                        sel(
                            "Correct Spelling Automatically",
                            "toggleAutomaticSpellingCorrection:",
                            None,
                        ),
                    ],
                }),
            ],
        },
        Submenu {
            title: "Window".into(),
            role: Role::Window,
            entries: vec![
                sel("Minimize", "performMiniaturize:", cmd("m")),
                sel("Zoom", "performZoom:", None),
                Entry::Separator,
                sel("Bring All to Front", "arrangeInFront:", None),
            ],
        },
    ];
    for menu in &mut menus {
        strip(&mut menu.entries, &reserved);
    }
    menus
}

fn strip(entries: &mut [Entry], reserved: &[Chord]) {
    for entry in entries {
        match entry {
            Entry::Item(item) => {
                if item.key.as_ref().is_some_and(|k| k.is_reserved(reserved)) {
                    item.key = None;
                }
            }
            Entry::Submenu(sub) => strip(&mut sub.entries, reserved),
            Entry::Separator => {}
        }
    }
}

/// Every item, depth first (tests and the live-menu check).
pub fn items(menus: &[Submenu]) -> Vec<&Item> {
    fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a Item>) {
        for entry in entries {
            match entry {
                Entry::Item(item) => out.push(item),
                Entry::Submenu(sub) => walk(&sub.entries, out),
                Entry::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for menu in menus {
        walk(&menu.entries, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grafida's reserved chord, as its manifest spells it.
    const PLAIN_PASTE: &str = "CmdOrCtrl+Shift+V";

    #[test]
    fn no_key_equivalent_is_the_plain_text_paste_chord() {
        let reserved = Chord::parse_mac(PLAIN_PASTE).unwrap();
        // Even with nothing reserved, the standard menu never binds it.
        for menus in [standard("T", &[]), standard("T", &[PLAIN_PASTE.into()])] {
            for item in items(&menus) {
                assert_ne!(item.key.as_ref(), Some(&reserved), "{}", item.title);
            }
        }
    }

    #[test]
    fn paste_and_match_style_is_not_in_the_menu() {
        let menus = standard("T", &[]);
        assert!(
            !items(&menus)
                .iter()
                .any(|i| i.action == Action::Selector("pasteAsPlainText:"))
        );
    }

    #[test]
    fn a_reserved_chord_loses_its_shortcut_but_keeps_its_item() {
        let menus = standard("T", &["Cmd+Z".into(), "CmdOrCtrl+Shift+Z".into()]);
        let all = items(&menus);
        let undo = all.iter().find(|i| i.title == "Undo").unwrap();
        let redo = all.iter().find(|i| i.title == "Redo").unwrap();
        assert_eq!(undo.key, None);
        assert_eq!(redo.key, None);
        let copy = all.iter().find(|i| i.title == "Copy").unwrap();
        assert_eq!(copy.key, cmd("c"));
    }

    #[test]
    fn manifest_chords_parse_as_on_a_mac() {
        assert_eq!(
            Chord::parse_mac("CmdOrCtrl+Shift+V"),
            Some(Chord::with("v", true, false, false, true))
        );
        assert_eq!(
            Chord::parse_mac("Ctrl+Alt+k"),
            Some(Chord::with("k", false, true, true, false))
        );
        assert_eq!(Chord::parse_mac("Cmd+F12"), None);
        assert_eq!(Chord::parse_mac("Hyper+V"), None);
    }

    #[test]
    fn an_uppercase_appkit_key_equivalent_implies_shift() {
        let reserved = [Chord::parse_mac(PLAIN_PASTE).unwrap()];
        assert!(Chord::from_appkit("V", true, false, false, false).is_reserved(&reserved));
        assert!(Chord::from_appkit("v", true, false, false, true).is_reserved(&reserved));
        assert!(!Chord::from_appkit("v", true, false, false, false).is_reserved(&reserved));
        assert!(!Chord::from_appkit("V", true, false, true, false).is_reserved(&reserved));
        assert!(!Chord::from_appkit("", true, false, false, true).is_reserved(&reserved));
    }

    #[test]
    fn about_and_quit_are_the_shells_own() {
        let menus = standard("Grafida", &[]);
        let all = items(&menus);
        assert_eq!(all[0].title, "About Grafida");
        assert_eq!(all[0].action, Action::About);
        let quit = all.iter().find(|i| i.title == "Quit Grafida").unwrap();
        assert_eq!(quit.action, Action::Quit);
    }
}
