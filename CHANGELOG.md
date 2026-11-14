# Changelog

Notable changes follow the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) structure.
This reconstruction groups local history and package reports; `0.1.0` is a pre-release development baseline, not a published-release claim.

## [Unreleased]

### Added

- MIT license, English project overview and contribution/security documentation.
- GitHub issue and pull-request templates, Linux CI and draft-release workflows.
- Offline updater-manifest generator and tests; the release workflow now signs the final AppImage.
- UPD1 native updater, tray and user-local desktop-entry commands; persisted settings and a mocked, tested TypeScript policy. UPD1b connects the approved settings groups, update banner, release notes, cancellable download progress and start-menu prompt to the runtime, with DE/EN text and DOM tests. Native tests are pending.
- UPD1: native commands for the updater, tray and user-local start-menu entry; persisted settings and tested TypeScript flow control. UPD1b wires the released settings groups, update banner, release notes, cancellable download progress and start-menu prompt (DE/EN text, DOM tests) into the runtime. Native tests pending.
- Distribution: Linux AppImage and tar.gz for x86_64 and arm64 (Raspberry Pi). The draft-release workflow builds each architecture on a native runner, signs both AppImages and publishes one `latest.json` covering `linux-x86_64` and `linux-aarch64`.
- Offline dependency-license inventory generator and screenshot capture checklist.

## [0.1.0] - Development baseline

### Added

- Two-pane Linux file manager with tabs, bookmarks, themes and English/German UI.
- File operations, trash, undo/history, background job queues, pause/resume and SHA-256 transfer verification.
- Viewer and quick view, search, batch rename, archives, directory synchronization and text comparison.
- FTP/FTPS, SFTP and native SMB profiles; Dolphin/GNOME system-place import.
- Process-plugin host, duplicate search, disk usage and similar-image search.
- Command palette and AppImage build/repack script.

### Changed

- Revised toolbar, connection indicators, pane headers, file lists, function-key bar and status rows.
- Remote paths display protocol/profile context, disconnected state and navigation back to local or remote locations.
- Clearer connection dialogs, job counters, comparison feedback and rename previews.

### Fixed

- Return-to-connection navigation resolves current session IDs and reconnects disconnected profiles.
- Modal keyboard handling and Escape after focus leaves a dialog.
- Selection retention across filters, context-menu behavior and light-theme contrast.
- ZIP local timestamps, session saving on close and trash actions during cancellation.
- AppImage environment leakage to external applications and graphics-library bundling compatibility.

Evidence: local history through `52ebc3d`. No release dates have been invented.
