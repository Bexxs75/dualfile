# Work order: places sidebar, step 2 only (Forgejo #5)

Step 1 is built and merged (see the sidebar code in `src-tauri/src/places.rs`, `crates/dualfile-core/src/places_pool.rs`, `drives.rs`, `system_places.rs`, `appenv.rs`, `src/components/Sidebar.tsx`, `src/state/places.ts`). Build **only "Step 2: interaction"** from `docs/plans/places-sidebar.md` §9, following §1–§8 and ALL amendment/owner-decision sections at the end of that plan (they override earlier text).

Step-2 scope:
- Drop targets (transfer dialog, trash), drag-in to pin a bookmark, bookmark reordering.
- Context menu: connect, disconnect, save mount as connection, open in other pane / new tab, properties, empty trash.
- fstab `user`/`users`/`owner` mount/unmount via `mount -- <point>` / `umount -- <point>` (no shell, no options from the UI, absolute freshly resolved targets; the helper decides the real permission). GVfs unmount via a new validated mount-key endpoint (profile-less GVfs mounts, amendment P2 "Missing API for step 2").
- Ribbon visibility rule (bookmark bar only while sidebar hidden; already partly in step 1 — complete it).

Mandatory safety (amendments P1/P2), already for step 2:
- **Unmount safety in the backend:** track the mount points touched by every running/queued job (sources and destinations), refuse unmount while a job touches it, and block new jobs on a mount that is being unmounted. Panes/tabs inside the mount: ask first, then move them to home after a successful unmount.
- Authorisation matrix: mount requires "not mounted", unmount requires "mounted"; separate checks for fstab vs GVfs.
- Mount helpers return only safe error codes (busy, not permitted, not found, helper missing, timeout) or redacted diagnostics; never credentials/user names in DTOs, Debug, errors or logs (tests).
- Use the bounded helper runner from step 1 (`appenv.rs`), re-read mount state after a helper timeout instead of assuming the result.
- Keep the step-1 guarantees: no listing/places call may block on a dead mount.

Strings DE + EN, code/comments English, CHANGELOG `[Unreleased]` entry (English, existing format), wiki page `docs/wiki/places-sidebar.md` updated (DE+EN as that page does), 0.3.5 notes in `src/release-notes.ts` only if user-visible wording needs it.
Tests per §7 for step 2, including the safety regressions above.

Do not commit, push or release. Do not create a report file; report in your reply. Rust may not compile in your sandbox (missing offline crate): write the Rust code and tests anyway; the host runs clippy -D warnings, cargo test and cargo fmt.
