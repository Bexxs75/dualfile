# Changelog

Notable changes follow the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) structure.
This reconstruction groups local history and package reports; `0.1.0` is a pre-release development baseline, not a published-release claim.

## [Unreleased]

### Added

- Start with the system: Off (default), Start minimized, or Start and show the window. Dualfile asks once at startup (after the other startup questions; "Not now" means off and is not asked again), and the choice can be changed at any time under Settings → System. It writes a per-user XDG autostart entry (`~/.config/autostart/de.thebexxs.dualfile.desktop`, respecting `XDG_CONFIG_HOME`) that points to the AppImage file itself and follows it when the AppImage is moved; "Off" removes the entry, and an entry of another application is never replaced or removed. A minimized start (`--minimized`) does not flash the window: it stays in the tray when the tray icon is enabled and a panel actually shows it (StatusNotifier host), otherwise it starts minimized in the task bar. Write errors are shown next to the setting.

### Fixed

- X11: the main window opened in the top-left corner of the screen instead of where it was left. The stored position could come from a session that does not know it: a Wayland session writes 0/0 into the window-state file, and so did a start that never showed the window (minimized to the tray). Dualfile now keeps the position in a file of its own (`window-position.json` in the configuration folder) and only stores what a shown, non-maximized X11 window reported; a position whose title bar would lie outside every monitor is not applied, and the window manager places the window instead. The first start after the update is placed by the window manager once, because the old stored position is not trusted. Wayland is unchanged: the compositor places the window. (#18)

### Changed

- Tray help text and README: on Wayland (KDE Plasma, GNOME) only closing hides the window in the tray; minimizing keeps the task bar entry, because the compositor does not tell the app that the window was minimized. On X11 minimizing still hides to the tray. (#19)

## [0.3.6] - 2026-11-17

### Added

- The Trash opens inside Dualfile (as a tab in the active pane) instead of the system file manager: list with original location, deletion date and size; restore, delete permanently (always confirmed) and empty. A context-menu entry on the sidebar Trash still opens the system file manager. Writing into the Trash view (copy, move, rename, new folder) is refused.
- Right-click a folder in the file list and choose "Add to Places" (or "Remove from Places") to pin it to the sidebar. Requested by a tester.
- Drop files from outside (Dolphin, the desktop) onto a Dualfile pane or onto a folder row: the target is highlighted and the copy dialog opens with the dropped paths. Dropping always copies (the native drop carries no Shift modifier); read-only, archive and unmounted destinations are refused as usual. Requested by a tester.

### Fixed

- The cursor row is now clearly marked: an accent outline and tint in the active pane (muted outline in the inactive pane), while the mouse-hover highlight is much fainter, so a hovered row is no longer mistaken for the row Delete acts on. Reported by a tester.
- Delete and F8 (and the context menu entry) always ask before moving files to the Trash; the confirmation is focused on "Yes, move to Trash" so Enter confirms and Esc cancels. This cannot be switched off. Requested by a tester.
- Sidebar: an Ubuntu installer stick (hybrid ISO) no longer shows up a second time as "not mounted"; opening that entry failed with "mounting is not allowed". A whole disk with partitions no longer counts as a drive itself.

## [0.3.5] - 2026-11-17

### Added

- Opt-in LAN service discovery through Avahi over D-Bus. The sidebar shows up to 64 discovered SMB, NFS, SFTP and WebDAV endpoints; clicking opens a validated, prefilled connection editor without connecting or sending credentials. The setting defaults off and synchronizes with the settings window. Browsing is isolated, bounded and cancelled when disabled; unavailable Avahi stays silent, and removed/restarted services clear stale results.

- Removable-device sidebar actions: bounded `lsblk` discovery and live device watches show unmounted USB/hotplug media, with udisks mounting, unmounting and whole-drive safe removal. All partitions and bind aliases participate in job protection and tab confirmation. Local non-user fstab entries use polkit; encrypted media point to the system file manager. Safe helper errors include cancelled authentication, and timeout results are checked against fresh state. Optical media eject through the `eject` helper; USB Eject deliberately unmounts and powers off. Safe removal locks unlocked LUKS containers first. Concurrent discovery and device operations wait within bounded deadlines; internal SATA/NVMe hotplug flags do not enable removal. Device watches rearm only after loss.

- Places sidebar interactions: drop files onto destinations to open the transfer dialog, confirm trash drops, pin folders between bookmarks and reorder bookmarks. Context menus open places in the other pane or a new tab, manage connections, save network mounts, show properties and empty the trash. Eligible fstab mounts and GVfs mounts can be unmounted with queued/running job protection and confirmation before affected tabs return home. Helper timeouts recheck mount state and retain protection until the helper exits. Mount admission isolates stalled paths, trash purging skips network/automount filesystems and reserves only actual trash locations, and mount dialogs prevent navigation during mounting. fstab ownership checks, symlink-aware tab checks and normalized system-profile matching protect mount actions. Trash purging accepts standard owned 0755 directories, uses a progress watchdog instead of a total deadline, supports additional local filesystems and invalidates size caches. Large selections share parent resolution and show at most 20 confirmation paths; URL account removal preserves at-signs in paths. Admission leaf checks tolerate steady progress on slow mounts; concurrent requests on the same healthy mount wait within a bounded budget instead of immediately reporting busy. Trash discovery resolves symlinked home/data directories; deletion handles deep trees iteratively and refuses device boundaries encountered during traversal.

- Places sidebar with live mounts, XDG folders, bookmarks, connections, local free space, icon rail, keyboard navigation and synchronized sidebar settings. F9 shows/hides it; Shift+F9 switches icons/full without replacing existing bindings. Floating windows grow with the sidebar where the compositor permits. GVfs/KIO mounts whose local paths contain sign-in information appear with redacted labels but cannot be navigated until opaque-key navigation is available. Their supported context actions use opaque backend keys.

- **What’s new after an update:** a German or English dialog shows bundled release notes once, with skipped versions grouped newest first. Close it to acknowledge, or reopen the current notes through Tools or the command palette. Fresh installations stay quiet; downgrades preserve the last acknowledged version.

- **Dualfile remembers the size and position of its main window.** The next start opens the window as you left it, including the maximized state. On Wayland desktops only the size and the maximized state are restored: applications cannot read or set their window position there, so the compositor places the window (KDE can remember it with a window rule). On X11 the position is restored too. Other windows (such as the connection editor) are not remembered.

- **Drag files onto a folder in the same pane.** Folder rows are drop targets in both panes: the row is highlighted, the hint names the folder, and releasing opens the copy dialog (Shift: move) with that folder as destination. A dragged folder cannot be dropped onto itself; dropping on free space still targets the other pane's current folder. Requested by a tester.

### Changed

- **The embedded terminal closes when its shell ends.** Ctrl+D (or `exit`) at the shell prompt now closes the terminal panel, as in Konsole; programs inside the terminal such as `python3` keep their own Ctrl+D. If the shell fails right at the start (non-zero exit within 2 seconds, e.g. a wrong shell path), the panel stays open so the message remains readable.

- Settings open in a separate, resizable OS window. Main remains the only settings writer; appearance, language, shortcuts and system status stay synchronized. Closing waits for pending changes to be applied and saved, and read-only settings and save failures are shown in the window. The browser preview keeps the inline dialog.

## [0.3.4] - 2026-11-16

### Changed

- **The connection menu is more compact.** Each connection is now a single row (status, name, protocol and state); its actions *Connect*/*Disconnect* and *Edit …* open in a side submenu next to the menu. The submenu opens with a click, Enter, Space or the right arrow key, closes with the left arrow key or Escape, and flips to the left side when there is no room on the right.
- **Creating and editing a connection opens its own window.** The connection form is no longer a dialog inside the main window but a separate window that you can move and resize freely, also outside the main window. It stays on top of the main window without blocking it; the connection list refreshes as soon as you save. Only one editor window is open at a time (asking for another brings the open one forward). If a profile's host, user or port changes while a password or host-key question for it is open, that question is closed and connecting refuses the outdated target.
- **The window title shows the version** (e.g. "Dualfile 0.3.4").
- **The embedded terminal now starts your own shell by default** (`$SHELL` with your configuration, functions and prompt), so your own commands and abbreviations work there too. Existing installations keep the shell they had; switch under Settings → System → Terminal shell.

### Removed

- **The "Link panes" toggle was removed.** Opening a bookmark now always changes the active pane only; the other pane no longer follows. An old `mirror` entry in an existing settings file is ignored and dropped on the next save.

## [0.3.3] - 2026-11-16

### Fixed

- **After an update from within the app, links and other programs did not open, and on newer systems Dualfile did not start again at all.** The restart after an update started the new AppImage with the settings of the old one: paths into the old AppImage, which stayed mounted, so its bundled libraries were loaded by everything Dualfile started (browser, terminal, default applications). On newer systems (e.g. Ubuntu 26.04) the new version did not even start. Quitting and starting Dualfile again fixed it. The restart now starts the new version cleanly, and paths of any older AppImage are no longer passed on. **Updating to this version still uses the old restart:** if links do not open or Dualfile does not come back after this update, quit it and start it again once.
- **"Invalid settings: tabs" when a tab showed a server.** A tab opened on a server address (`sftp://…` and the like) made every save of the session fail with this error, for example when switching tabs. Such tabs are now left out of the saved session; tabs of saved connections and local folders are kept.

## [0.3.2] - 2026-11-16

### Fixed

- **Folder views now catch up after the system dropped change notifications.** When too many changes happen at once, Linux discards the queued notifications and only reports that it did; that notice used to get lost, so a view could stay stale until refreshed by hand. All watched folders now reload.
- **Links and files opened with the default application could close again at once.** When `xdg-open` handed over quickly, Dualfile closed the error channel it had passed on, and the started browser or program was killed as soon as it wrote a message there; the click seemed to do nothing. It now stays open for the started program.
- **Opening a link that fails is reported.** *Project on GitHub*, *Report a problem on GitHub* and other links used to fail silently when the system could not open a browser; the reason is now shown and written to the log file.

## [0.3.1] - 2026-11-16

### Fixed

- **Settings were often not saved.** Changes such as the language took effect at once but frequently never reached `settings.json`, without any error message. The folder watcher treated merely opening or listing a folder as a change, so the shown folder reloaded about four times a second, and every reload pushed the pending save back again. Reading a folder no longer counts as a change, a reload no longer postpones a pending save, and a change made while a save is still running is written afterwards. This also ends the constant background reloading of the shown folder.

### Changed

- README: the plugin interface is listed as planned (groundwork only, no UI and no plugins yet).

## [0.3.0] - 2026-11-16

### Added

- **Log file for troubleshooting** (Settings → About → Troubleshooting). Dualfile now writes errors and warnings (failed file jobs, connection and update problems, panics) to `dualfile.log` in `$XDG_STATE_HOME/dualfile/` (default `~/.local/state/dualfile/`), 1 MiB per file with two older files kept. Passwords, keys and file contents are never logged. *View log file…* shows the last 500 lines, optionally anonymized (home folder, user name, computer name, server addresses and e-mail addresses are replaced), and can copy them or save them into the current folder. *Open folder* and *Report a problem on GitHub* (opens a new issue with version and system filled in) sit next to it, and error messages get a *Details in the log file* button. Nothing is ever sent automatically.

### Fixed

- **Tray "Open" brings the window back.** When Dualfile was minimized or behind other windows, *Open* in the tray menu (and starting Dualfile a second time) did nothing on KDE: the window manager refused the focus request (focus-stealing prevention). The window is now re-shown, so it comes to the front.

## [0.2.11] - 2026-11-16

### Added

- **Terminal colours** (Settings → Appearance → Terminal): choose *Like the theme*, *Classic*, *Green on black*, *Amber*, *Light* or *Custom…* (colour picker or `#rrggbb` hex for background and text, with a swap button). A live contrast indicator warns below 4.5 : 1, but the colours are still applied. Cursor and selection follow the text colour, and the ANSI colours of command output adapt to light and dark backgrounds. Changes apply to all open terminals immediately.

### Changed

- The terminal now defaults to *Green on black* (`terminalColors`: `green`), including for existing settings without a saved value. Choose *Like the theme* to get the previous look.

## [0.2.10] - 2026-11-15

### Fixed

- The embedded terminal's cursor is now actually visible. In the packaged build the Content-Security-Policy (`style-src` without `'unsafe-inline'`) blocked the runtime stylesheet xterm injects to draw the cursor, so WebKitGTK dropped it and no cursor appeared — which is why the 0.2.9 cursor change had no effect. `style-src` now allows `'unsafe-inline'` (as the dev build already did); `script-src` stays `'self'`. The cursor is a steady block, visible whether or not the terminal is focused.
- "This computer" no longer lists other AppImages' FUSE self-mounts (`/tmp/.mount_*`) or the KIO-FUSE bridge (`/run/user/*/kio-fuse-*`) as drives. Real FUSE mounts such as sshfs or ntfs are still shown.

## [0.2.9] - 2026-11-15

### Added

- A **Terminal shell** setting (Settings → System) lets you choose which shell the embedded terminal launches: *Automatic* (fish with inline command suggestions when fish is installed, otherwise a plain `bash`/`sh`), *Plain bash*, or *My shell (`$SHELL`)* with your own config, prompt and greeting.

### Changed

- When fish is installed, the embedded terminal now opens it with `--no-config` (so still no custom prompt or greeting), giving inline command suggestions as you type; without fish it falls back to the plain `bash --norc` / `/bin/sh` as before. This is the new *Automatic* default and can be changed in the new Terminal shell setting.

### Fixed

- The embedded terminal's cursor is now a steady block instead of blinking, so it stays visible. The blink animation did not run in the app's webview, which left the cursor invisible.

## [0.2.8] - 2026-11-15

### Changed

- The embedded terminal now opens a plain shell — `bash --norc` (or `/bin/sh`) — instead of your login shell, so it starts clean without a custom prompt, greeting or login banner from your shell configuration.

### Removed

- The optional quick-access bar (added in 0.2.5) has been removed. It was off by default; it did not earn its place and grew awkward at narrow widths. Drives are still reachable through each pane's "This computer" menu.

### Fixed

- The embedded terminal no longer cuts off its last line: the panel now sizes the terminal to the space below the resize grip, so nothing is clipped at the bottom when the window or panel is resized.

## [0.2.7] - 2026-11-15

### Changed

- Each pane now has its own quick-access bar; a click opens the place in that pane. Previously a single shared bar always moved the active pane.
- The quick-access bar shows curated places — your Home folder, your bookmarks and your mounted drives — instead of listing every mounted filesystem. System mounts (`/boot`, `/efi`) are left out.
- Keyboard shortcuts in the command palette and in the keyboard-shortcuts settings are now drawn as raised keycaps, matching the terminal button's hover card, instead of plain text.

## [0.2.6] - 2026-11-15

### Added

- The quick-access bar now lists every real mounted filesystem (the root, extra partitions and btrfs subvolumes, network shares and removable media), like a twin-pane file manager's drive bar, instead of only auto-mounted removable media. Kernel pseudo-filesystems (proc, sysfs, cgroup, tmpfs, snap/loop images, container overlays, …) are left out.
- The terminal toolbar button shows a hover card with its keyboard shortcut drawn as keycaps, using the key legend of the active language (German shows `^` on the top-left key, not `` ` ``) plus a hint that it is the top-left key under Esc. The plain-text tooltip stays as a fallback.

### Changed

- The terminal resize grip and the column-width dividers are easier to see and grab — a taller terminal handle with a visible bar, and a clearer column divider that highlights on hover/focus — in all three themes.

### Fixed

- Opening a file externally no longer fails silently: if no default application is set for the file type, or the launcher exits with an error, the actual reason is now shown instead of nothing happening.

## [0.2.5] - 2026-11-15

### Added

- NFSv3 connections (export, uid/gid) can be created in the connection dialog.
- The height of the embedded terminal can be changed by dragging its top edge (double-click resets); it is remembered.
- Optional quick-access bar above the panes with local drives and bookmarks; one click opens the place in the active pane. Off by default (Settings).

### Security

- Names and text a plugin supplies (entry names and its manifest name/description) can no longer contain invisible or bidirectional Unicode format characters, which could reorder or hide text in the UI to spoof a name (F8).
- Removing a local source tree during a move now uses the hardened directory removal that never follows a directory symlink swapped in during the walk, so a local race cannot redirect the deletion outside the source tree (F3).
- A remote file opened externally is never handed to the desktop as a launchable `.desktop` entry: the private local copy of such a file is renamed so it opens as text instead of being executed (F11).
- A saved password is discarded when a profile is changed to another server (host, port or protocol), so it is never sent to the new server.
- Resetting trust now also closes the open session that still used the forgotten certificate.

### Fixed

- SMB: uploading a file larger than a single transfer chunk no longer fails — the sequential-write check compared the write offset against the server-acknowledged byte count (which lags behind the pipelined writes) instead of the bytes accepted, rejecting the second chunk.
- WebDAV: subfolders on servers that redirect a collection path without a trailing slash (e.g. Apache mod_dav) are now browsable — a PROPFIND that is 301/308-redirected is retried once with a trailing slash (the server's redirect target is still never followed).
- The embedded terminal now starts the shell with a clean environment in the AppImage build: it no longer inherits the bundle's LD_LIBRARY_PATH/PATH, so the shell loads system libraries (e.g. fish/libpcre2) and finds system commands instead of the bundled ones.
- Large entries in a plain (uncompressed) tar are no longer wrongly rejected as a possible zip bomb past 100 MiB (the compression-ratio guard was not fed the bytes read).
- The terminal keyboard shortcut (Ctrl+`) now works on keyboard layouts where the backtick is a dead/accent key (e.g. German): the physical top-left key is matched regardless of layout.

## [0.2.4] - 2026-11-15

### Security

- Cancelling a conflict prompt during a folder move no longer deletes the source folder: the job now stops, and a cancelled copy never removes source files in any verify mode.
- The plugin sandbox no longer allows `fcntl` unconditionally; owner/signal commands and `O_ASYNC` (which could signal other processes of the same user) are denied.
- The release workflow pins appimagetool to a fixed release and verifies its SHA-256.

### Fixed

- Starting the update replacement re-checks for running file jobs after the download, so a job started meanwhile is not killed by the restart.

## [0.2.3] - 2026-11-15

### Added

- File-list columns can be resized by dragging the header dividers; widths are remembered.

### Fixed

- The file panes no longer flood with "Update installation in progress" during an update download; file operations are only paused for the brief replacement step, not the whole download.

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
