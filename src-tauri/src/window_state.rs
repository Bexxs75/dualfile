//! Remembers size and maximized state of the main window (plus position where the
//! windowing system lets an app set it).

use tauri::plugin::TauriPlugin;
use tauri::Wry;
use tauri_plugin_window_state::StateFlags;

/// Wayland apps can neither read nor set their window position, so restoring a stored
/// one would only write a meaningless value; the compositor places the window.
fn flags(wayland: bool) -> StateFlags {
    let flags = StateFlags::SIZE | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN;
    if wayland {
        flags
    } else {
        flags | StateFlags::POSITION
    }
}

#[cfg(target_os = "linux")]
fn detect_wayland() -> bool {
    use gtk::prelude::*;
    gtk::gdk::Display::default().is_some_and(|display| display.type_().name().contains("Wayland"))
}

#[cfg(not(target_os = "linux"))]
fn detect_wayland() -> bool {
    false
}

/// The plugin is built before GTK is initialized, so GDK must not be touched here
/// (Display::default() panics). Mirror GDK's own backend choice from the environment.
fn wayland_from_env(gdk_backend: Option<&str>, wayland_display: Option<&str>) -> bool {
    match gdk_backend.map(str::trim) {
        Some(b) if b.starts_with("wayland") => true,
        Some(b) if !b.is_empty() && !b.starts_with('*') => false,
        _ => wayland_display.is_some_and(|d| !d.is_empty()),
    }
}

static WAYLAND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Setup has an initialized GTK display; async commands must not call GTK themselves.
pub(crate) fn initialize_backend() {
    WAYLAND.store(detect_wayland(), std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn is_wayland() -> bool {
    WAYLAND.load(std::sync::atomic::Ordering::Relaxed)
}

/// VISIBLE stays out of the flags: the tray-start logic decides whether the window shows.
/// Only the main window is tracked, so editor or settings windows never overwrite its size.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_window_state::Builder::default()
        .with_state_flags(flags(wayland_from_env(
            std::env::var("GDK_BACKEND").ok().as_deref(),
            std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        )))
        .with_filter(|label| label == "main")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_backend_guess_needs_no_gtk() {
        assert!(wayland_from_env(None, Some("wayland-0")));
        assert!(!wayland_from_env(None, None));
        assert!(!wayland_from_env(Some("x11"), Some("wayland-0")));
        assert!(wayland_from_env(Some("wayland,x11"), None));
        assert!(wayland_from_env(Some("*"), Some("wayland-0")));
    }

    #[test]
    fn position_is_only_restored_outside_wayland() {
        assert!(flags(false).contains(StateFlags::POSITION));
        assert!(!flags(true).contains(StateFlags::POSITION));
    }

    #[test]
    fn size_and_maximized_are_restored_everywhere_but_visibility_never() {
        for wayland in [false, true] {
            let f = flags(wayland);
            assert!(f.contains(StateFlags::SIZE | StateFlags::MAXIMIZED));
            assert!(!f.contains(StateFlags::VISIBLE));
        }
    }
}
