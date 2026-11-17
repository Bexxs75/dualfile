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
fn is_wayland() -> bool {
    use gtk::prelude::*;
    gtk::gdk::Display::default().is_some_and(|display| display.type_().name().contains("Wayland"))
}

#[cfg(not(target_os = "linux"))]
fn is_wayland() -> bool {
    false
}

/// VISIBLE stays out of the flags: the tray-start logic decides whether the window shows.
/// The editor window is skipped so it does not overwrite the main window's size.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_window_state::Builder::default()
        .with_state_flags(flags(is_wayland()))
        .with_denylist(&["connection-editor"])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

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
