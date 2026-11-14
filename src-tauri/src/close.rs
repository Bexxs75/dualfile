//! Main-window close handshake; active-job confirmation takes precedence.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

pub(crate) static SAVING: AtomicBool = AtomicBool::new(false);
pub(crate) static READY: AtomicBool = AtomicBool::new(false);

pub(crate) use crate::close_policy::{decide, Decision};
pub(crate) fn begin(window: &tauri::Window) {
    if SAVING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = window.emit(
        crate::CORE_EVENT,
        crate::convert::CoreEventDto::CloseSaveRequested,
    );
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        finish(&window, false, 2000);
    });
}
pub(crate) fn finish(window: &tauri::Window, acknowledged: bool, elapsed_ms: u64) {
    if crate::close_policy::should_finish(
        SAVING.load(Ordering::SeqCst),
        READY.load(Ordering::SeqCst),
        acknowledged,
        elapsed_ms,
    ) && !READY.swap(true, Ordering::SeqCst)
    {
        let _ = window.close();
    }
}
