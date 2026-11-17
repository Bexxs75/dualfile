# Work order: places sidebar, step 4 only (Forgejo #7)

Steps 1–3 are built and merged (sidebar, bounded places pools `crates/dualfile-core/src/places_pool.rs`, mount guard `src-tauri/src/mount_guard.rs`, helper runner `crates/dualfile-core/src/appenv.rs`, places/devices in `src-tauri/src/places.rs` and `src-tauri/src/devices.rs`). Build **only "Step 4 (optional): LAN discovery"** from `docs/plans/places-sidebar.md` (§3.3, §9 step 4, test 10, preview 13), following §1–§8 and ALL amendment/owner-decision sections at the end (they override earlier text).

Scope:
- Avahi over D-Bus via zbus (§3.3 option 2): browse `_smb._tcp`, `_nfs._tcp`, `_sftp-ssh._tcp`, `_webdav._tcp`/`_webdavs._tcp` on its own browse thread; emit `PlacesChanged` on add/remove. No avahi-browse, no mdns-sd crate.
- New section "Im Netzwerk gefunden" / "Found on the network". Clicking an entry opens the existing connection editor prefilled (host, port, protocol, share/path if TXT gives one); it never connects or sends credentials by itself.
- Opt-in setting, **off by default**, through the normal `Settings` → `SettingsDto` path and visible in the settings window (DE+EN). Turning it off stops browsing and clears the section at once.
- No avahi-daemon or D-Bus failure may block or slow listings, places refresh or job admission; missing avahi = section hidden, one safe log line, no error dialog.

Safety / robustness (must hold): everything from the network is untrusted — validate and bound host names, service names, ports, TXT records (length, count, control characters, bidi/zero-width characters shown escaped or stripped); cap entries (e.g. 64) and dedupe by host+protocol+port; IPv4/IPv6/link-local handled; no automatic resolving loop storms (debounce); stale entries removed on ItemRemove and when avahi restarts; never log TXT contents. Rules from steps 1–3: bounded I/O, parallel callers wait, tests pass with default parallel `cargo test`.

Strings DE + EN, code/comments English, CHANGELOG `[Unreleased]` (English, existing format), wiki `docs/wiki/places-sidebar.md` (DE+EN), 0.3.5 notes in `src/release-notes.ts` if user-visible wording needs it. Tests: parsing/validation of untrusted service data, opt-in default off, enable/disable lifecycle, add/remove events, avahi missing; frontend tests for section + prefilled editor.

Do not commit, push or release. No report file; reply only. Rust may not compile in your sandbox (missing offline crate): write the Rust code and tests anyway; the host runs clippy -D warnings, cargo test and cargo fmt.
