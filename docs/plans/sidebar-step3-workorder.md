# Work order: places sidebar, step 3 only (Forgejo #6)

Steps 1 and 2 are built and merged (sidebar, bounded places pools in `crates/dualfile-core/src/places_pool.rs`, mount guard in `src-tauri/src/mount_guard.rs`, bounded helper runner in `crates/dualfile-core/src/appenv.rs`, fstab/GVfs mount in `src-tauri/src/places.rs`). Build **only "Step 3: devices"** from `docs/plans/places-sidebar.md` §9, following §1–§8 and ALL amendment/owner-decision sections at the end of that plan (they override earlier text).

Scope:
- `lsblk --json` listing of unmounted removable media (bounded helper, LC_ALL=C), live updates via a `/dev/disk/by-id` watch (plus the existing mount watcher); show them in the devices section.
- `udisksctl` mount / unmount / eject / power-off for removable devices, with job and pane safety via the existing mount guard (refuse while a job touches the device; ask before moving tabs inside it home; move only after success). Power-off checks ALL partitions of the whole drive (amendment P1).
- fstab non-`user` local entries via udisks (polkit decides), per the authorisation matrix (amendment P2): "removable only" applies to udisks devices, not to fstab NFS/noauto.
- Safe errors only (busy, not permitted, not found, helper missing, timeout, polkit dismissed), no device serials/labels in logs beyond what the UI shows, no credentials.
- No mDNS/Avahi (that is step 4).

Rules learnt in steps 1–2 (must hold): every I/O on devices/mounts runs bounded and must never block listings, places refresh or job admission for other mounts; parallel admissions on one mount wait instead of failing; helpers run with LC_ALL=C and no shell; re-read state after a helper timeout instead of assuming the result; tests must pass with the default parallel `cargo test`.

Strings DE + EN, code/comments English, CHANGELOG `[Unreleased]` (English, existing format), wiki `docs/wiki/places-sidebar.md` (DE+EN), 0.3.5 notes in `src/release-notes.ts` if the user-visible wording needs it. Tests per §7 for step 3 incl. the safety regressions.

Do not commit, push or release. No report file; reply only. Rust may not compile in your sandbox (missing offline crate): write the Rust code and tests anyway; the host runs clippy -D warnings, cargo test and cargo fmt.
