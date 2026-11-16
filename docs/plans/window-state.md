# Plan: remember window size and position (target 0.3.5)

Tester request (FantoxFox, Discord, 2026-11-16): Dualfile should remember its window size and position. Today nothing is stored; every start uses 1280×760 from `tauri.conf.json`.

## Approach
- Use the official `tauri-plugin-window-state` (Tauri 2 plugin; saves size, position, maximized, fullscreen per window label) instead of own code. Only for the `main` window (deny-list/`with_denylist` for `connection-editor` and the future `settings` window).
- State flags: SIZE | POSITION | MAXIMIZED | FULLSCREEN (not VISIBLE: the tray-start logic decides visibility). Saved on close and on exit; restore on start before the window is shown, to avoid a visible jump.
- **Wayland limitation (must be said in CHANGELOG/wiki):** apps can neither read nor set their window position on Wayland, so only **size and maximized state** are restored there; the position is chosen by the compositor (KWin can remember it via a window rule). X11 restores position too.
- Off-screen safety: the plugin restores only if the saved position is on an existing monitor; verify with a disconnected second monitor in the VM; if not reliable, clamp to the primary monitor's work area ourselves.
- Minimum size 900×560 stays enforced.
- Interaction with the planned sidebar window growth (`places-sidebar.md`): the stored size includes the sidebar; on start the sidebar mode is applied without growing the window again.
- Storage: plugin file in the app config dir (`~/.config/…/.window-state.json`), separate from `settings.json`; nothing to migrate. A corrupt file is ignored by the plugin (verify).

## Changes
- `src-tauri/Cargo.toml` + lockfiles: `tauri-plugin-window-state` (version matching Tauri 2.12.5); `cargo deny` check.
- `src-tauri/src/lib.rs`: register the plugin with flags and denylist; restore before `show()`.
- Capabilities: only if the frontend needs it (it does not); no new frontend permissions.
- CHANGELOG, wiki (Window / Settings page), release notes DE/EN for the what's-new window.

## Tests / VM
- Plasma 6 Wayland + GNOME Wayland: resize → restart → same size; maximized → restart → maximized.
- X11 session: position also restored; second monitor removed → window lands on the primary monitor.
- Tray start (hidden) still works; update restart keeps size.
