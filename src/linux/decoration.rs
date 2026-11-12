//! The colours of a title bar KWin draws (grafida/grafida#92).
//!
//! tao's `set_theme` sets GTK's `gtk-application-prefer-dark-theme`, which reaches only a title bar
//! GTK draws itself: GNOME's. On KDE Plasma, GTK asks KWin to decorate the window, and KWin paints
//! that decoration from a colour scheme it is told per window. KDE's own applications tell it
//! through `org_kde_kwin_server_decoration_palette` on Wayland and the `_KDE_NET_WM_COLOR_SCHEME`
//! property on X11, so the shell does the same. Under any other compositor or window manager the
//! global does not exist, or the property is ignored, and nothing happens.
//!
//! The scheme is Breeze's own light or dark one, looked up in the XDG data directories; `system`
//! sends the empty name, which is KWin's "the user's colour scheme" (`kdeglobals`).

use std::path::PathBuf;

use gtk::{glib::object::ObjectType, prelude::*};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_surface::WlSurface},
};
use wayland_protocols_plasma::server_decoration_palette::client::{
    org_kde_kwin_server_decoration_palette::OrgKdeKwinServerDecorationPalette as Palette,
    org_kde_kwin_server_decoration_palette_manager::OrgKdeKwinServerDecorationPaletteManager as PaletteManager,
};

use crate::manifest::Theme;

/// The colour scheme file names, most preferred first. Plasma 6 ships `BreezeLight` and
/// `BreezeDark`; Plasma 5 before 5.24 called the light one plain `Breeze`.
fn scheme_names(theme: Theme) -> &'static [&'static str] {
    match theme {
        Theme::Light => &["BreezeLight.colors", "Breeze.colors"],
        Theme::Dark => &["BreezeDark.colors"],
        Theme::System => &[],
    }
}

/// The scheme to ask KWin for: an absolute path, the empty string for the user's own scheme, or
/// `None` when the scheme the theme needs is not installed (the decoration is then left alone).
pub fn scheme_for(theme: Theme, data_dirs: &[PathBuf]) -> Option<String> {
    if theme == Theme::System {
        return Some(String::new());
    }
    scheme_names(theme).iter().find_map(|name| {
        data_dirs
            .iter()
            .map(|dir| dir.join("color-schemes").join(name))
            .find(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    })
}

/// `$XDG_DATA_HOME`, then `$XDG_DATA_DIRS`, as GLib resolves them.
fn data_dirs() -> Vec<PathBuf> {
    std::iter::once(gtk::glib::user_data_dir())
        .chain(gtk::glib::system_data_dirs())
        .collect()
}

#[derive(Default)]
pub struct Decoration {
    wayland: Option<Result<Wayland, String>>,
}

/// The KWin palette object for the window's surface, and the queue its objects belong to.
struct Wayland {
    connection: Connection,
    _queue: EventQueue<State>,
    _manager: PaletteManager,
    palette: Palette,
}

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(State: PaletteManager);
delegate_noop!(State: Palette);

impl Decoration {
    /// Tells KWin which colour scheme to paint the window's decoration with. Returns a line for the
    /// log when something was sent or could not be, `None` when there is no KWin to tell.
    pub fn apply(&mut self, window: &gtk::ApplicationWindow, theme: Theme) -> Option<String> {
        let gdk_window = window.window()?;
        let display = gdk_window.display();
        let backend = display.type_().name();
        let scheme = scheme_for(theme, &data_dirs());
        match backend {
            "GdkWaylandDisplay" => {
                let wayland = self
                    .wayland
                    .get_or_insert_with(|| Wayland::new(&display, &gdk_window));
                let wayland = match wayland {
                    Ok(wayland) => wayland,
                    // Asked once: no KWin (GNOME, wlroots) is the common case, not an error.
                    Err(_) => return None,
                };
                let Some(scheme) = scheme else {
                    return Some(missing(theme));
                };
                wayland.palette.set_palette(scheme.clone());
                match wayland.connection.flush() {
                    Ok(()) => Some(sent(&scheme)),
                    Err(e) => Some(format!("window decoration: could not tell KWin: {e}")),
                }
            }
            // Only KWin reads the property; elsewhere there is nothing to tell.
            "GdkX11Display" if is_plasma() => {
                let Some(scheme) = scheme else {
                    return Some(missing(theme));
                };
                let property = gtk::gdk::Atom::intern("_KDE_NET_WM_COLOR_SCHEME");
                if scheme.is_empty() {
                    gtk::gdk::property_delete(&gdk_window, &property);
                } else {
                    gtk::gdk::property_change(
                        &gdk_window,
                        &property,
                        &gtk::gdk::Atom::intern("STRING"),
                        8,
                        gtk::gdk::PropMode::Replace,
                        gtk::gdk::ChangeData::UChars(scheme.as_bytes()),
                    );
                }
                Some(sent(&scheme))
            }
            _ => None,
        }
    }
}

impl Wayland {
    fn new(display: &gtk::gdk::Display, window: &gtk::gdk::Window) -> Result<Self, String> {
        // SAFETY: GDK owns both objects for the window's lifetime, which is the process's; the
        // backend built over a foreign display neither closes it nor dispatches GDK's queue.
        let (wl_display, wl_surface) = unsafe {
            let display: *mut gdk_wayland_sys::GdkWaylandDisplay = display.as_ptr() as *mut _;
            let window: *mut gdk_wayland_sys::GdkWaylandWindow = window.as_ptr() as *mut _;
            (
                gdk_wayland_sys::gdk_wayland_display_get_wl_display(display),
                gdk_wayland_sys::gdk_wayland_window_get_wl_surface(window),
            )
        };
        if wl_display.is_null() || wl_surface.is_null() {
            return Err("GDK has no Wayland surface for the window".into());
        }
        let connection =
            Connection::from_backend(unsafe { Backend::from_foreign_display(wl_display.cast()) });
        let (globals, queue) =
            registry_queue_init::<State>(&connection).map_err(|e| e.to_string())?;
        let handle = queue.handle();
        let manager: PaletteManager = globals
            .bind(&handle, 1..=1, ())
            .map_err(|e| e.to_string())?;
        let surface_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), wl_surface.cast()) }
            .map_err(|e| e.to_string())?;
        let surface = WlSurface::from_id(&connection, surface_id).map_err(|e| e.to_string())?;
        let palette = manager.create(&surface, &handle, ());
        Ok(Wayland {
            connection,
            _queue: queue,
            _manager: manager,
            palette,
        })
    }
}

fn sent(scheme: &str) -> String {
    if scheme.is_empty() {
        "window decoration: KWin uses the user's colour scheme".into()
    } else {
        format!("window decoration: KWin uses {scheme}")
    }
}

fn missing(theme: Theme) -> String {
    format!(
        "window decoration: no {} colour scheme installed ({}), so KWin's stays as it is",
        match theme {
            Theme::Dark => "dark",
            _ => "light",
        },
        scheme_names(theme).join(" or ")
    )
}

fn is_plasma() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d == "KDE"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn dirs_with(files: &[&str]) -> (PathBuf, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!(
            "decoration-test-{}-{}",
            std::process::id(),
            files.join("+")
        ));
        let user = root.join("user");
        let system = root.join("system");
        for file in files {
            let (dir, name) = file.split_once('/').unwrap();
            let dir = root.join(dir).join("color-schemes");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), "[General]\n").unwrap();
        }
        (root, vec![user, system])
    }

    fn path(root: &Path, file: &str) -> Option<String> {
        let (dir, name) = file.split_once('/').unwrap();
        Some(
            root.join(dir)
                .join("color-schemes")
                .join(name)
                .to_string_lossy()
                .into_owned(),
        )
    }

    #[test]
    fn system_is_the_users_own_scheme() {
        assert_eq!(scheme_for(Theme::System, &[]), Some(String::new()));
    }

    #[test]
    fn breeze_is_found_in_the_system_data_dirs() {
        let (root, dirs) = dirs_with(&["system/BreezeDark.colors", "system/BreezeLight.colors"]);
        assert_eq!(
            scheme_for(Theme::Dark, &dirs),
            path(&root, "system/BreezeDark.colors")
        );
        assert_eq!(
            scheme_for(Theme::Light, &dirs),
            path(&root, "system/BreezeLight.colors")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_users_copy_wins_and_plasma_5s_light_name_is_a_fallback() {
        let (root, dirs) = dirs_with(&[
            "user/BreezeDark.colors",
            "system/BreezeDark.colors",
            "system/Breeze.colors",
        ]);
        assert_eq!(
            scheme_for(Theme::Dark, &dirs),
            path(&root, "user/BreezeDark.colors")
        );
        assert_eq!(
            scheme_for(Theme::Light, &dirs),
            path(&root, "system/Breeze.colors")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_scheme_leaves_the_decoration_alone() {
        let (root, dirs) = dirs_with(&["system/BreezeLight.colors"]);
        assert_eq!(scheme_for(Theme::Dark, &dirs), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
