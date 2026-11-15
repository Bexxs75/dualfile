# Changelog

Notable changes follow the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) structure.
This reconstruction groups local history and package reports; `0.1.0` is a pre-release development baseline, not a published-release claim.

## [Unreleased]

## [0.2.2] - 2026-11-15

### Added

- Only one Dualfile instance runs at a time; launching it again focuses the existing window.
- Toggle a single embedded local shell terminal below the file panes with Ctrl+` or the toolbar button.

### Changed

- Updating is one click: after installing, Dualfile restarts into the new version by itself; the "Restart now" button only appears if the automatic restart fails.
- Settings are split into tabs (Appearance, System, Info, Keyboard) instead of one long list. The Info tab holds the update controls, the version, the license and a GitHub link.

### Fixed

- Adding the start-menu entry no longer fails with "Icon belongs to another application" when an icon from an earlier version is present; the entry's own outdated icon is refreshed.
- The pane color pickers start from the current theme's panel color instead of a fixed dark color.
- Update errors are shown as a toast and no longer grow the update banner or shift the window layout.

## [0.2.1] - 2026-11-15

### Security

- Harden the certificate pin store and the TLS trust lifecycle. A pre-existing concurrency issue
  (affecting FTPS, SFTP and WebDAV) could lose a "trust always" decision when two profiles connected
  at once, and several TLS trust paths bypassed a trust reset. Each connection attempt now owns its
  pending pins; a reconnect or follow-up transport verifies read-only against the persisted and
  just-confirmed pins (a different valid certificate is a pin mismatch); a trust reset reliably
  invalidates earlier confirmations; and WebDAV TLS connections no longer resume sessions, so every
  handshake re-verifies the certificate. FTPS keeps session resumption unchanged.

## [0.2.0] - 2026-11-15

### Added

- Native **WebDAV** connections (read and write) as a first-class protocol. HTTPS verifies the
  certificate against the system root store, and self-signed / internal-CA servers go through the
  same fingerprint-confirmation flow as FTPS/SFTP; the connection form has a base-path field, an
  HTTPS toggle and an explicit confirmation for unencrypted (http) connections.
- **Global content search** across many files, reachable from the command palette; results navigate
  into the containing folder.
- **Rebindable keyboard shortcuts** — an editor in Settings remaps the function keys and Ctrl
  combinations; structural keys stay fixed and reserved shortcuts are protected.
- **Metadata-based batch rename** with EXIF (capture date, camera) and ID3 (artist/title/album/track)
  placeholders.
- **Per-pane window colours** — a subtle, freely chosen tint per pane.
- **Extension column** in the Total-Commander style (name · extension · size · date), off by default.
- Mounted external media (USB) appear as **devices** in the places bar, refreshed when the window
  regains focus.
- **Refresh button per pane**.
- Tab **context menu** (new tab / close / close others), **pin**, and **lock to folder**; tabs are
  restored on start.
- `known_hosts` entries are additionally suggested while connecting.
- Offline updater-manifest generator and tests; the release workflow signs the final AppImage.
- Native updater, tray and user-local start-menu entry; persisted settings and a tested TypeScript
  update flow (update banner, release notes, cancellable download progress, start-menu prompt;
  DE/EN text, DOM tests).
- Distribution: Linux AppImage and tar.gz for x86_64 and arm64 (Raspberry Pi); the draft-release
  workflow builds each architecture on a native runner, signs both AppImages and publishes one
  `latest.json` covering `linux-x86_64` and `linux-aarch64`.
- Offline dependency-license inventory generator and screenshot-capture checklist.
- MIT license, English project overview and contribution/security documentation; GitHub issue and
  pull-request templates; Linux CI and draft-release workflows.

### Changed

- The default UI font is now **Inter** (self-hosted, CSP-compliant); the monospace font stays
  IBM Plex Mono.
- SSH-config import follows **relative and nested `Include`** directives like OpenSSH, so hosts from
  included files are found.
- Remote servers are distinguishable in tabs (host/icon); job names are localized.

### Fixed

- Mouse-wheel scrolling in large folders no longer snaps the selection back into view.
- Opening a remote file now downloads it to a private temporary copy and opens it, instead of
  reporting a misleading "unknown error".
- Connecting now refreshes the pane to the remote location.
- Core hardening: a copy from a remote source can no longer exceed the size the server advertised
  (guard against a lying server over-filling the staging file).

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
