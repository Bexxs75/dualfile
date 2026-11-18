//! Remembers size and maximized state of the main window through the window-state plugin, and
//! its position on X11 in a file of its own.
//!
//! The plugin's stored position cannot be trusted. A session that does not track the position
//! (Wayland) still writes `"x": 0, "y": 0` into `.window-state.json`, and the plugin also saves
//! what the toolkit reports for a window that was never shown, which is 0/0 as well. A later
//! X11 start then moved the window to the top-left corner. So the plugin never handles the
//! position here; only a position that was seen on a shown X11 window is stored and applied.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::plugin::TauriPlugin;
use tauri::{Manager, Wry};
use tauri_plugin_window_state::StateFlags;

const POSITION_FILE: &str = "window-position.json";

/// POSITION stays out (see the module comment). VISIBLE stays out too: the tray-start logic
/// decides whether the window shows.
fn flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
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

static WAYLAND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Setup has an initialized GTK display; async commands must not call GTK themselves.
pub(crate) fn initialize_backend() {
    WAYLAND.store(detect_wayland(), std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn is_wayland() -> bool {
    WAYLAND.load(std::sync::atomic::Ordering::Relaxed)
}

/// Outer top-left corner and outer width in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Placement {
    x: i32,
    y: i32,
    width: u32,
}

#[derive(Default)]
struct Tracker {
    current: Option<Placement>,
    previous: Option<Placement>,
}

impl Tracker {
    fn record(&mut self, placement: Placement) {
        // Every resize reports the position again; only a change may push the older one out.
        if self.current != Some(placement) {
            self.previous = self.current;
            self.current = Some(placement);
        }
    }

    /// The placement worth saving. The move that maximizes a window can arrive before the
    /// maximized state does and is then recorded like a normal move; it is recognizable by
    /// being where the maximized window is now, and the one before it is the real one.
    fn settled(&self, zoomed: bool, now: (i32, i32)) -> Option<Placement> {
        match self.current {
            Some(current) if zoomed && (current.x, current.y) == now => self.previous,
            current => current,
        }
    }
}

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker {
    current: None,
    previous: None,
});

/// Wayland reports no position. A hidden window reports the toolkit's initial 0/0, a
/// minimized, maximized or fullscreen one a place the user did not choose.
fn trackable(wayland: bool, visible: bool, minimized: bool, zoomed: bool) -> bool {
    !wayland && visible && !minimized && !zoomed
}

/// A stored position is only applied while a top corner lies on a monitor, so the title bar
/// can be reached; otherwise (monitor gone, other layout) the window manager places the window.
/// Monitors are `(x, y, width, height)`.
fn reachable(placement: Placement, monitors: &[(i32, i32, u32, u32)]) -> bool {
    let right = placement
        .x
        .saturating_add_unsigned(placement.width.saturating_sub(1));
    monitors.iter().any(|&(x, y, width, height)| {
        let inside = |px: i32| px >= x && px < x.saturating_add_unsigned(width);
        placement.y >= y
            && placement.y < y.saturating_add_unsigned(height)
            && (inside(placement.x) || inside(right))
    })
}

fn position_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    Some(app.path().app_config_dir().ok()?.join(POSITION_FILE))
}

fn zoomed(window: &tauri::WebviewWindow) -> bool {
    window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false)
}

/// Called for every move of the main window.
pub(crate) fn track(app: &tauri::AppHandle, position: tauri::PhysicalPosition<i32>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if !trackable(
        is_wayland(),
        window.is_visible().unwrap_or(false),
        window.is_minimized().unwrap_or(true),
        zoomed(&window),
    ) {
        return;
    }
    let Ok(size) = window.outer_size() else {
        return;
    };
    TRACKER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record(Placement {
            x: position.x,
            y: position.y,
            width: size.width,
        });
}

/// Writes the last tracked position. Without one (Wayland, window never shown) the file stays
/// as it is, so a hidden start or a Wayland session in between loses nothing.
pub(crate) fn save(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let now = window
        .outer_position()
        .map_or((i32::MIN, i32::MIN), |p| (p.x, p.y));
    let placement = TRACKER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .settled(zoomed(&window), now);
    let (Some(placement), Some(path)) = (placement, position_file(app)) else {
        return;
    };
    let written = serde_json::to_vec(&placement)
        .map_err(std::io::Error::other)
        .and_then(|json| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, json)
        });
    if let Err(e) = written {
        log::warn!("Fensterposition nicht gespeichert: {e}");
    }
}

/// Moves the still hidden main window to its stored X11 position; GTK applies it when the
/// window is mapped. Must run before the window is shown.
pub(crate) fn restore(window: &tauri::WebviewWindow) {
    if is_wayland() {
        return;
    }
    let stored = position_file(window.app_handle())
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|json| serde_json::from_slice::<Placement>(&json).ok());
    let Some(placement) = stored else {
        return;
    };
    let monitors: Vec<_> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            (
                m.position().x,
                m.position().y,
                m.size().width,
                m.size().height,
            )
        })
        .collect();
    if reachable(placement, &monitors) {
        let _ = window.set_position(tauri::PhysicalPosition::new(placement.x, placement.y));
    }
}

/// Only the main window is tracked, so editor or settings windows never overwrite its size.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_window_state::Builder::default()
        .with_state_flags(flags())
        .with_filter(|label| label == "main")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_restores_size_and_maximized_but_neither_position_nor_visibility() {
        let f = flags();
        assert!(f.contains(StateFlags::SIZE | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN));
        assert!(!f.contains(StateFlags::POSITION));
        assert!(!f.contains(StateFlags::VISIBLE));
    }

    fn at(x: i32, y: i32) -> Placement {
        Placement { x, y, width: 1428 }
    }

    #[test]
    fn only_a_shown_normal_x11_window_has_a_position_worth_keeping() {
        assert!(trackable(false, true, false, false));
        // Wayland reports nothing usable.
        assert!(!trackable(true, true, false, false));
        // Hidden start: the toolkit still says 0/0.
        assert!(!trackable(false, false, false, false));
        assert!(!trackable(false, true, true, false));
        assert!(!trackable(false, true, false, true));
    }

    #[test]
    fn nothing_tracked_means_nothing_to_save() {
        assert_eq!(Tracker::default().settled(false, (0, 0)), None);
        assert_eq!(Tracker::default().settled(true, (0, 0)), None);
    }

    #[test]
    fn the_last_move_is_saved_and_repeated_reports_do_not_push_it_out() {
        let mut tracker = Tracker::default();
        tracker.record(at(100, 80));
        tracker.record(at(300, 200));
        tracker.record(at(300, 200));
        assert_eq!(tracker.settled(false, (300, 200)), Some(at(300, 200)));
        assert_eq!(tracker.previous, Some(at(100, 80)));
    }

    #[test]
    fn a_maximizing_move_recorded_too_early_is_not_saved() {
        let mut tracker = Tracker::default();
        tracker.record(at(300, 200));
        // State arrived first, the move was never recorded: the current one is real.
        assert_eq!(tracker.settled(true, (0, 28)), Some(at(300, 200)));
        // Move arrived first and was recorded: fall back to the one before.
        tracker.record(Placement {
            x: 0,
            y: 28,
            width: 1920,
        });
        assert_eq!(tracker.settled(true, (0, 28)), Some(at(300, 200)));
        // The same position on a normal window is a real one.
        assert_eq!(
            tracker.settled(false, (0, 28)),
            Some(Placement {
                x: 0,
                y: 28,
                width: 1920
            })
        );
    }

    #[test]
    fn a_position_is_applied_only_while_the_title_bar_is_on_a_monitor() {
        let one = [(0, 0, 1920, 1080)];
        assert!(reachable(at(0, 0), &one));
        assert!(reachable(at(400, 300), &one));
        // Pushed out to the left or right: the other top corner is still there.
        assert!(reachable(at(-1000, 300), &one));
        assert!(reachable(at(1800, 300), &one));
        // Saved on a second monitor that is gone, or below/above every monitor.
        assert!(!reachable(at(2200, 300), &one));
        assert!(!reachable(at(400, 1080), &one));
        assert!(!reachable(at(400, -40), &one));
        assert!(reachable(
            at(2200, 300),
            &[(0, 0, 1920, 1080), (1920, 0, 1920, 1080)]
        ));
        assert!(!reachable(at(0, 0), &[]));
    }
}
