# Work order: places sidebar, step 1 only (Forgejo #4)

Build **only "Step 1: read-only sidebar with live places"** from `docs/plans/places-sidebar.md` §9, following the whole plan (§1–§8) and ALL sections under "Amendments after Codex review", "Owner decisions", "Mockup approved" and "Owner decision: window grows instead of squeezing the panes". The amendments override the earlier plan text where they conflict.

Step-1 scope reminders:
- Display only: no mount/unmount/eject actions, no drag & drop, no context-menu actions beyond what step 1 lists (those are step 2/3).
- P1 amendments apply already here: every places I/O in a bounded, isolated worker pool with a response deadline; no unbounded `wait()` after `kill()`; no credentials in DTOs/Debug/errors.
- P2 GVfs: bounded polling while the sidebar is visible, not inotify on the runtime dir.
- P2 keyboard: F9 / Shift+F9 defaults stay unbound if the user already bound those keys; explicit allow-list in `useGlobalKeys`.
- Owner decisions: F9 show/hide, Shift+F9 icons↔full, ribbon bookmark bar only while sidebar hidden, default `full`, window grows (prefer one Rust command `sidebar_resize_window(delta)` restricted to the main window).
- The settings window (`src/SettingsWindow.tsx`) now exists: new sidebar settings must appear and sync there too.
- Strings in DE and EN, code/comments in English, CHANGELOG `[Unreleased]` entry (English, existing format).
- Tests per §7 for step 1, including the amendment regression tests.

Do not commit, push or release. Rust/Docker may be unavailable in your sandbox: write the Rust code and tests anyway; the host runs `cargo clippy -D warnings`, `cargo test` and `cargo fmt` afterwards. Report anything you could not verify.
