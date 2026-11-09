//! Process-wide settings that GTK and WebKitGTK read once, at start, and so must be made before the
//! first GTK call (grafida/grafida#86). `prepare()` runs from `main()` before any thread exists,
//! which is what makes setting environment variables sound.

use std::{ffi::OsStr, path::Path};

use gtk::glib;

use crate::manifest::{DmabufRenderer, Manifest};

/// WebKitGTK's own switch for its DMA-BUF renderer.
pub const DMABUF_VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Applies the manifest's process-wide settings and returns what it did, for the log.
///
/// - The program name becomes the manifest's `id`. GTK uses it as the Wayland `app_id` and the
///   X11 `WM_CLASS`, which is how a dock or task bar matches the window to `<id>.desktop` and
///   shows its icon (the binary's own name, `grafida-shell`, matches nothing).
/// - The DMA-BUF renderer, per `webview.dmabufRenderer`.
///
/// # Safety
///
/// Must be called before any other thread is started: it sets environment variables.
pub unsafe fn prepare(manifest: &Manifest) -> Vec<String> {
    glib::set_prgname(Some(manifest.id.as_str()));
    glib::set_application_name(&manifest.title);
    let mut notes = vec![format!("program name (Wayland app_id): {}", manifest.id)];

    let user = std::env::var_os(DMABUF_VAR);
    let decision = dmabuf(
        manifest.webview.dmabuf_renderer,
        user.as_deref(),
        nvidia_loaded(),
    );
    if decision == Dmabuf::Disable {
        // SAFETY: the caller guarantees no other thread exists yet.
        unsafe { std::env::set_var(DMABUF_VAR, "1") };
    }
    notes.push(format!("DMA-BUF renderer: {}", decision.describe()));
    notes
}

/// What to do about the DMA-BUF renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dmabuf {
    /// The user set `WEBKIT_DISABLE_DMABUF_RENDERER` themselves: leave it alone.
    UserSet,
    /// Turn it off (set the variable).
    Disable,
    /// Leave WebKit's default (on).
    Keep,
}

impl Dmabuf {
    fn describe(self) -> &'static str {
        match self {
            Dmabuf::UserSet => "left to the user's WEBKIT_DISABLE_DMABUF_RENDERER",
            Dmabuf::Disable => "off (WEBKIT_DISABLE_DMABUF_RENDERER=1)",
            Dmabuf::Keep => "on (WebKit's default)",
        }
    }
}

/// Pure, so every combination is testable without NVIDIA hardware.
pub fn dmabuf(setting: DmabufRenderer, user: Option<&OsStr>, nvidia: bool) -> Dmabuf {
    if user.is_some() {
        return Dmabuf::UserSet;
    }
    match setting {
        DmabufRenderer::Off => Dmabuf::Disable,
        DmabufRenderer::On => Dmabuf::Keep,
        DmabufRenderer::Auto if nvidia => Dmabuf::Disable,
        DmabufRenderer::Auto => Dmabuf::Keep,
    }
}

/// NVIDIA's kernel driver (proprietary or open modules; not nouveau) is loaded. The blank-window
/// failure is in its userspace GBM/EGL stack, which both kernel flavours share.
fn nvidia_loaded() -> bool {
    Path::new("/sys/module/nvidia").exists() || Path::new("/proc/driver/nvidia/version").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_variable_always_wins() {
        for setting in [
            DmabufRenderer::Auto,
            DmabufRenderer::On,
            DmabufRenderer::Off,
        ] {
            for nvidia in [false, true] {
                assert_eq!(
                    dmabuf(setting, Some(OsStr::new("0")), nvidia),
                    Dmabuf::UserSet
                );
                assert_eq!(
                    dmabuf(setting, Some(OsStr::new("")), nvidia),
                    Dmabuf::UserSet
                );
            }
        }
    }

    #[test]
    fn auto_turns_it_off_only_on_nvidia() {
        assert_eq!(dmabuf(DmabufRenderer::Auto, None, true), Dmabuf::Disable);
        assert_eq!(dmabuf(DmabufRenderer::Auto, None, false), Dmabuf::Keep);
    }

    #[test]
    fn on_and_off_are_forced() {
        assert_eq!(dmabuf(DmabufRenderer::On, None, true), Dmabuf::Keep);
        assert_eq!(dmabuf(DmabufRenderer::Off, None, false), Dmabuf::Disable);
    }
}
