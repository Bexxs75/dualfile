# Plan: Places sidebar for Dualfile (revised: fstab, network shares, icon rail)

Read-only investigation of `/mnt/claude/projekte/dualfile` at 0.3.4 (`bc59641`). All paths below are relative to that repo root. I changed nothing.

---

## 0. What already exists

| Area | Finding | Where |
|---|---|---|
| **Pane place menu** | `PathBar` builds a `HeaderMenu` with sections: Computer, Home, saved connections (shows the connected state, click calls `chooseConnection`), bookmarks, drives ("Geräte"), "Manage connections", "New connection". | `src/components/Pane.tsx:140-159`, rendered at `:176-179` |
| **Drive refresh** | **No live updates.** Each `PathBar` (two instances) calls `api.localDrives()` once at mount and again on every `window` `focus` event. Nothing is pushed from the backend. | `src/components/Pane.tsx:119-127` |
| **`local_drives`** | Reads **`/proc/mounts` only** (not mountinfo, **not fstab**). Drops pseudo filesystem types, including `autofs` and `fuse.gvfsd-fuse`. Also drops `/boot` and `/efi`, `.mount_*` AppImage mounts, `kio-fuse*` and `$APPDIR`. Capped at 64 entries. `Drive` holds only `{name, path}`: no device, fstype, kind, read-only flag or space. Kernel CIFS/NFS/`fuse.sshfs` mounts do come through. | `crates/dualfile-core/src/drives.rs:13-41, 43-49, 72-85, 89-127, 130-135`; command at `src-tauri/src/commands.rs:866-870` |
| **`system_places_list`** | Covers **network locations only**: Dolphin `remoteview/*.desktop`, `user-places.xbel` (honours `IsHidden`), GTK bookmarks and `~/.gtk-bookmarks`. It also reads live mounts from `$XDG_RUNTIME_DIR/gvfs/*` (smb-share, dav, sftp, … with passwords filtered) and `kio-fuse-*/<scheme>/<host>`. Each entry is a `SystemPlace{sources,name,scheme,host,port,user,path,url,mountedAt}`. | `crates/dualfile-core/src/system_places.rs:12-22, 349-423, 424-508`; command at `commands.rs:874-884` |
| **`system_places_import`** | Turns discovered places into saved `System` connection profiles. | `commands.rs:887-891`; dialog in `src/components/dialogs/SystemPlacesDialog.tsx:20` |
| **GVfs mount/unmount** | `system_mount_connect` and `system_mount_unmount` run `gio mount [-u] -- <url>` with a cleaned AppImage environment, a 30 s timeout and sanitised stderr. Network URLs only (`parse_url` validates them). | `crates/dualfile-remote/src/system_mount.rs:18-85, 130-149`; `src-tauri/src/state/connection_impl.rs:822-849`; UI unmount button at `ConnectionsDialog.tsx:159` |
| **fstab** | **Nothing reads `/etc/fstab`** (no match for fstab anywhere). Automount points are hidden because `autofs` is in the pseudo list (`drives.rs:33`). | — |
| **mDNS / Avahi** | **Nothing.** No mdns, avahi or zeroconf code or crates. | — |
| **Block devices / udisks2** | **Nothing.** No lsblk or udisksctl. `zbus` 4 and 5 are only indirect dependencies (via `keyring`). | `src-tauri/Cargo.toml:43-45`, `src-tauri/Cargo.lock:7512,7550` |
| **XDG user dirs** | **None.** Nothing parses `user-dirs.dirs`. Only `home_dir` exists. | `src-tauri/src/lib.rs:60-65`, `commands.rs:53` |
| **Trash** | `trash` (to the trash) and `restore_from_trash`, which matches on *original* paths via `trash::os_limited`. **There is no trash browsing view.** | `crates/dualfile-core/src/ops.rs:1352, 1385-1419`; `commands.rs:93-99` |
| **Free space** | `space_free(path)` uses statvfs (`vfs.rs:919-935`). The command allows **at most 2 concurrent workers** and a **750 ms** timeout, and returns `None` beyond either limit. The status bar uses it at `Pane.tsx:259`. | `src-tauri/src/commands.rs:906-944` |
| **Folder watcher** | `notify-debouncer-mini` (inotify) watches the open folders and emits `DirChanged`. A folder that vanishes is reported. | `src-tauri/src/watch.rs:116-170`; `Cargo.toml:29` |
| **Events** | `CoreEventDto` is emitted as `"core-event"`. Variants: Job, Conflict, DirChanged, ConnectionsChanged, … The frontend `handleEvent` calls `refreshConnections` on `connectionsChanged`. | `src-tauri/src/convert.rs:855-875`; `lib.rs:45-58`; `src/ipc/types.ts:482-489`; `src/app/controller.ts:1010-1042` |
| **Bookmarks** | `Bookmark{name,path,color,order}`. `togglePin`, `openBookmark` (active pane), `editBookmark`, `moveBookmark`. The **Ribbon** bookmark bar has drag-reorder and Alt+1..9. | `crates/dualfile-core/src/settings.rs:113-130`; `controller.ts:754-792`; `src/components/Ribbon.tsx:7-48` |
| **Navigation** | `navigatePlace(w, path)` opens the connect dialog for disconnected remotes. `chooseConnection(w, profile)` connects, or returns to the last place on that connection. | `controller.ts:331-368` |
| **Drag & drop** | Drag between panes goes through `dragStart`, `dragOver` and `drop`, then `openTransfer(kind, from)`. That opens the transfer dialog, whose destination is prefilled with the other pane and is editable (`withDestination`). `confirmTransfer` calls `api.copy` or `api.move`, which runs the job pipeline and the `ConflictDialog`. | `controller.ts:983-1006, 385-407`; `src/state/operations.ts:16-22, 68-72`; `Pane.tsx:296-304`; `FileList.tsx:91-98` |
| **Global keys** | The global keydown handler only skips dialogs, menus and the terminal. **Arrow keys in a sidebar would move the file list** unless the sidebar is excluded. | `src/app/keyboard.ts:22-30, 76-85` |
| **Keymap** | F2–F8 and Shift+F8 are taken. **F9 and Shift+F9 are free.** Ctrl+F-keys cannot be bound (`eventToSpec`). | `src/state/keymap.ts:86-109, 131-134` |
| **Layout / minimum width** | `.panes` is a 2-column grid. Body `min-width: 900px` and Tauri `minWidth 900`; the default window is 1280×760. File-list columns need about `minmax(180px,1fr)+84+104+130`. | `src/App.tsx:105-123`; `src/styles/app.css:31, 67, 95`; `src-tauri/tauri.conf.json:18-20` |
| **Toolbar toggles** | `iconButton(...)` with `aria-pressed` is used for hidden files, quick view and terminal. | `src/components/Toolbar.tsx:63-70`; commands at `src/app/commands.ts:34-35` |
| **Settings loader** | `field()` falls back to the default **silently when a key is missing**, and records a "fix" (so the file is reported as `Repaired`) **only when the type is wrong**. Unknown keys are ignored. Version is 6. | `settings.rs:18, 225-240, 310-504`; unknown-key test at `:808` |
| **UI validation** | `validate_settings` checks UI-supplied settings (≤200 bookmarks, `plain_text`, `settings_path`). `SettingsDto` uses `#[serde(default)]` for its optional fields. | `src-tauri/src/validate.rs:213-284`; `convert.rs:606-654` (conversions around `:718` and `:781`) |
| **Command registration** | Each command needs: the `COMMANDS` list in `build.rs`, `generate_handler!` in `lib.rs:153+`, and `allow-*` in `capabilities/default.json`. A vitest checks this per command (`src/ipc/tauri.test.ts:543-557`). | `src-tauri/build.rs:3-104`; `capabilities/default.json:14-18` |
| **AppImage environment** | `apply_clean_env` strips `GIO_MODULE_DIR`, `GIO_EXTRA_MODULES`, `LD_LIBRARY_PATH` and similar for child processes. **The app process itself runs with the bundled GIO modules.** | `crates/dualfile-core/src/appenv.rs:67-77, 132` |

**Consequence of the last row:** in-process GIO `GVolumeMonitor` (`gtk` 0.18 is linked, `Cargo.toml:44`) would very likely not load the system gvfs-udisks2 monitor inside the AppImage. Prefer the existing pattern instead: external helpers (`gio`, `mount`, `udisksctl`, `lsblk`) run with a clean environment, plus our own kernel-level change detection.

---

## 1. UX design

### 1.1 Placement and modes
- **The sidebar is part of the main window (left of the panes), NOT a separate OS window** (owner, 2026-11-16, explicit). Only the settings dialog becomes its own window (separate plan).
- **One sidebar for the whole window**, to the left of both panes. Reasons:
  - It matches Dolphin, Nautilus and Total Commander's tree.
  - Per-pane sidebars would cost twice the width at a 900 px minimum.
  - The per-pane place menu already covers "this pane only".
- **Three persisted modes**: `hidden` | `icons` (rail of about 44 px) | `full` (resizable, 160–400 px, default 210).
- **Default for new and old settings files: `full`.** That is what the tester asked for. Old files without the key load as `full` without being reported as `Repaired`.
  - Alternative for the owner to decide: `icons` for existing users, so their layout changes less.
- **Automatic narrowing (temporary, never persisted):** if the window is narrower than the full sidebar plus 2 × 460 px panes (about 1150 px at width 210), `full` is shown as `icons`. Implement with a ResizeObserver on `.app`. The rail is 44 px, so at 900 px each pane gets about 410 px. Panes stay usable, and **900 px stays the minimum**; it is not raised.

### 1.2 Toggle and shortcuts (decision)
- **F9**: hidden ↔ last visible mode (Dolphin's F9 means "show/hide Places").
- **Shift+F9**: icons ↔ full. When the sidebar is hidden, Shift+F9 shows it.
- **Why two keys instead of one cycling key:** a three-state cycle makes F9 unpredictable (you cannot know how many presses are needed). Each key is now a binary toggle, which also makes the screen-reader announcements clear: "Seitenleiste ausgeblendet" / "Seitenleiste: Symbole".
- Both are `REBINDABLE` commands (`toggleSidebar`, `toggleSidebarIcons`) and appear in the command palette.
- One extra unbound command, `focusSidebar`. Suggested default Ctrl+Shift+B; check it against the palette list first.
- Also: a toolbar button (PanelLeft icon, `aria-pressed`), a collapse/expand chevron button at the bottom of the sidebar/rail, and **double-click on the resize handle** to switch icons ↔ full.
- Dragging the handle below about 120 px snaps to `icons`; dragging the rail wider than about 100 px snaps to `full`.

### 1.3 Sections (in order) and entries

**1. Orte (Places)**
- Home.
- XDG folders: Desktop, Documents, Downloads, Music, Pictures, Videos. Only show a folder if it exists and is not the home folder itself.
- Bookmarks, in `order`, with a colour dot and an Alt+n hint.
- Papierkorb (Trash).

**2. Verbindungen / Remote**
- (a) Saved Dualfile connections (sftp/ftp/smb/webdav/nfs/system) with a state dot (connected / disconnected / connecting). Click = `chooseConnection` (connects if needed).
- (b) Mounted network shares that have no saved profile:
  - kernel CIFS/SMB3, NFS/NFS4, `fuse.sshfs`, `fuse.rclone`, davfs, 9p from mountinfo;
  - GVfs mounts `/run/user/$UID/gvfs/*` and KIO-FUSE mounts, using `system_places::mounts` (`system_places.rs:424-508`). Context menu: "Als Verbindung speichern" (calls `system_places_import`), "Aushängen".
- (c) **fstab network entries that are not mounted** (see 1.4), shown greyed.
- (d) Optional later: "Im Netzwerk gefunden" (mDNS, step 4).
- Footer: "Neue Verbindung …".
- What is realistic for KIO: plain Dolphin `smb://` sessions **have no local path without kio-fuse**, so they only appear via the Dolphin bookmarks/remoteview import (`system_places.rs:351-400`). GVfs shares do show up as directories. State this in the UI help.

**3. Geräte (Devices)**
- Root "/" ("Systemlaufwerk"), other partitions (`/home` if it is a separate mount, `/data`, btrfs subvolume mount points), and removable media under `/run/media/$USER/*` or `/media/*`.
- Each entry shows a free-space bar.
- Removable media get an inline eject button (step 3).
- fstab local entries that are not mounted (USB/noauto) are shown greyed with a "Einhängen" action.
- Step 3: removable partitions that are plugged in but not mounted (from lsblk) are also shown greyed.

**Not in scope:**
- Dolphin's "Recent": there is no recent-files source. Tab history exists but is per tab.
- Dolphin's "Search": `SearchDialog` already exists.
- Note both as later options.

### 1.4 fstab entries (new requirement)
**Parsing.** Read `/etc/fstab`:
- Skip comments and swap, `none`, `proc`, `tmpfs`, `bind` entries, and the root and `/boot` entries.
- Un-escape `\040`.
- Bounded: at most 256 KiB and 256 entries.

**Merging with mountinfo**, keyed by mount point:
- **mounted** → a normal entry with a free-space bar (network types skip space, see §2.3), marked "aus fstab".
- **not mounted** → a greyed entry with a mount state and a reason:

| fstab options | Action | Mechanism |
|---|---|---|
| `user` / `users` / `owner` (owner only if the device belongs to us) | "Einhängen" | `mount <mountpoint>` (`/bin/mount` is setuid; util-linux allows it for user entries). Clean environment, 60 s timeout, `--` separator, mount point only. |
| `x-systemd.automount` (fstype `autofs` in mountinfo) | Click = navigate; the access triggers the mount | No helper. The listing is time-boxed by the existing remote/IO error path. Show "Automount". |
| local block device, `noauto`, not `user` | "Einhängen" via `udisksctl mount -b <dev>` (step 3) | udisks honours fstab options. Polkit may ask for an admin password, so use a 120 s timeout and let the desktop's polkit agent show its dialog. |
| network (nfs/cifs) without `user` | No action. Tooltip/notice: "Nur mit Administratorrechten einhängbar – Eintrag in /etc/fstab um `user` ergänzen oder per Verbindung öffnen" | Offer "Als Dualfile-Verbindung anlegen" (SMB/NFS profile prefilled with host and share from `//host/share` or `host:/export`). |

- `nofail`/`noauto` are only informational.
- Credentials in options (`credentials=`, `password=`, `username=`) **must never be serialised or logged**. Drop every option except an allow-list: `user`, `users`, `owner`, `noauto`, `ro`, `x-systemd.automount`, `_netdev`, `x-gvfs-show`, `x-gvfs-hide`, `x-gvfs-name=`.
- Respect `x-gvfs-hide` (hide) and `x-gvfs-name` (label), as GNOME does.

### 1.5 Interaction
**Click (left, or Enter):** navigates the **active** pane, like `openBookmark` at `controller.ts:763`.
- Remote targets go through `navigatePlace` / `chooseConnection`.
- Unmounted entries run "mount, then navigate".

**Ctrl+click or Ctrl+Enter:** other pane. This is the twin-pane convention.

**Middle-click or Shift+Enter:** new tab in the active pane (Dolphin: middle-click = new tab). Uses `newTab` + `navigatePlace`.

**Highlight:**
- The entry matching the active pane's path gets `aria-current="location"` and an accent bar.
- Small "L" or "R" markers show which pane is inside that place. This is cheap twin-pane context.

**Context menu** (right-click, ContextMenu key, Shift+F10):
- Open / Open in other pane / Open in new tab.
- Bookmarks: Rename/colour (`BookmarkDialog`), Remove.
- Connections: Connect / Disconnect / Edit (`openConnectionEditor`).
- Mounts: Mount / Unmount / Eject / Safely remove.
- Trash: Empty trash … / Open in system file manager.
- Hide entry; "Show hidden entries" (on the section menu); Copy path; Properties (free space, device, fstype, options).

**Collapsible sections:** in full mode the section header button has `aria-expanded`; the state is persisted.

**Density:** follows `settings.density` (compact/normal/comfortable), with row heights of about 24/28/32 px.

**Drop targets** (step 2):
- While `state.drag` is set (an internal drag from a pane), entries highlight.
- Drop = `openTransfer(shift ? "move" : "copy", drag.from, destination)`, which opens the **existing transfer dialog** with the entry's path prefilled, then the job pipeline and `ConflictDialog`.
- Drop on Trash = trash the dragged selection. It goes through the existing confirmation and undo.
- Refused with a toast:
  - archive paths (read-only);
  - disconnected connections ("erst verbinden"). Option for later: connect first, then open the dialog;
  - unmounted devices;
  - read-only mounts.
- Dropping a single folder onto the **gap between bookmarks or the Places header** = pin it as a bookmark at that position. This is Dolphin's "drag in".
- Bookmarks can be reordered by drag; reuse `moveBookmark`.
- XDG, device and connection entries are not reorderable in v1.
- External OS drags: not supported today. Out of scope.

**Hidden entries:**
- Hide via the context menu. The entry key goes into `sidebar_hidden`.
- "Show hidden entries" in a section menu shows them faded, with "Show again".
- Bookmarks are removed with Remove (unpin), not hidden.

### 1.6 Icon rail (`icons` mode)
- **Content:** the same entries as full mode. Sections are separated by a thin divider instead of a heading. Collapsed sections stay collapsed in the rail too, shown as one "…" button that opens a popover listing the section.
- **Telling icons apart:**
  - Type icons: home, desktop, document, download, music, image, video, trash, bookmark (tinted with the bookmark colour), server (by protocol: sftp/smb/nfs/webdav), hard disk, root disk, USB, SD card, optical, network folder, fstab-unmounted (dashed outline).
  - Badge: the first letter of the label (2 letters if needed to disambiguate; bookmarks use their colour) in the bottom-right corner, so "Sicherung" and "Sammlung" disks differ.
  - Status dot top-right: green = connected/mounted, grey = disconnected/unmounted, amber = busy.
- **Free space:** a 3 px underline bar under the icon, filled to the used fraction (warning colour above 90 %). Full numbers only in the tooltip.
- **Tooltip** (hover *and* keyboard focus, after about 400 ms; uses `aria-describedby`): "USB-Stick – 12,3 GB frei von 32 GB · /run/media/…". For connections: "NAS · SMB · verbunden".
- **Accessibility:**
  - Each button gets an `aria-label` containing name and state ("NAS, SMB, verbunden").
  - The tree semantics stay the same as in full mode (§1.7), so screen readers announce the same structure.
  - The rail toggle button at the bottom: `aria-label="Seitenleiste erweitern"`, `aria-expanded=false`.
- **Clicks, the context menu and drop targets work identically.** While dragging, hovering an icon for 800 ms shows its tooltip, so you can see the target name.

### 1.7 Keyboard and screen reader
- Container: `<nav aria-label="Orte">` containing `role="tree"`.
  - Sections are `treeitem aria-level=1 aria-expanded`; entries are `treeitem aria-level=2`.
  - Roving tabindex: a single Tab stop.
- Keys:
  - Up/Down: move.
  - Left/Right: collapse/expand, or jump to the parent section.
  - Home/End.
  - Typeahead by first letters.
  - Enter / Ctrl+Enter / Shift+Enter: as in §1.5.
  - ContextMenu / Shift+F10: menu.
  - Esc: back to the active file list.
  - Delete on a bookmark: remove (with an undo toast).
- **Required change in `useGlobalKeys`:** return early when the target is inside `.sidebar`, except for F9/Shift+F9 and the global Ctrl shortcuts. Otherwise arrow keys and typeahead would drive the file list (`keyboard.ts:24-30, 76-85`).
- Live region: announce "USB-Stick eingehängt" / "… entfernt" politely, with a debounce.

---

## 2. Live updates

### 2.1 Decision: event-driven mount detection, plus cheap refreshes
A new **`MountWatcher`** thread in `src-tauri` with one job: tell the UI "places may have changed".

1. **`/proc/self/mountinfo` with `poll(POLLPRI|POLLERR)`.** The kernel signals every mount-table change this way (proc(5); systemd and libmount use the same mechanism). Zero new dependencies (`libc` is already present). It catches udisks automounts, `mount` calls from fstab `user` entries, NFS/CIFS mounts, sshfs and systemd automount activations.
   - **Why not inotify:** inotify does not work on `/proc`.
   - **Why not only `/run/media`:** that misses fstab and network mounts.
   - **Why not udisks D-Bus:** it misses manual and network mounts and needs zbus code.
2. **inotify (existing `notify` crate) on `$XDG_RUNTIME_DIR/gvfs`** (non-recursive). GVfs shares are subdirectories of a single FUSE mount, so they do **not** change mountinfo. Also watch `$XDG_RUNTIME_DIR` itself for new `kio-fuse-*` directories (best effort).
3. **inotify on `/etc/fstab`.** Rare, but free. Watch the parent directory, because editors replace the file.
4. Step 3: **inotify on `/dev/disk/by-id`** (devtmpfs supports inotify; udev creates the symlinks) to see plugged-in but unmounted sticks.

**Debounce and emission:**
- Debounce 300 ms.
- Re-parse the data and compare it with the previous snapshot. Emit **`CoreEventDto::PlacesChanged`** only if the visible set changed, so container and snap churn is filtered.
- The event carries no payload. The frontend then refetches `places_snapshot` (§3), in the same style as `connectionsChanged` (`controller.ts:1039`).

**Fallbacks:**
- Keep the window-focus refresh (`Pane.tsx:121-126`, moved to the controller).
- Keep a 60 s timer, only while the sidebar is visible and the window is not hidden. It covers free space and anything the watcher missed.

**Everything else:**
- Free space is also refreshed when a copy/move/trash/extract job reaches `done`.
- Connection state is already in the store (`state.connections`, `refreshConnections` at `controller.ts:94`); the sidebar only subscribes.
- Bookmarks, hidden entries and modes live in settings in the store and update immediately.
- mDNS (step 4): its own browse thread, emitting `PlacesChanged` on add/remove.

### 2.2 Shutdown and robustness
- The watcher runs for the app lifetime. Use a poll timeout of 5 s so the thread notices a stop flag.
- If `/proc/self/mountinfo` cannot be polled (unusual sandboxes), fall back to re-reading every 5 s while the sidebar is visible.
- Log once through `log::warn`.

### 2.3 Free space safely
- statvfs on a dead NFS/CIFS hard mount can block for minutes, and statvfs on an unmounted autofs point *triggers the mount*.
- Rule: compute space in the backend **only for local block filesystems and already-mounted tmpfs-like media**. Skip types `nfs*`, `cifs`, `smb3`, `fuse.*`, `9p`, `ceph`, `glusterfs`, `davfs`, and skip autofs points.
- Network entries get space on demand only (tooltip or context menu "Speicherplatz ermitteln"), via the existing bounded `space_free`.
- Run it in one worker thread with a per-entry 300 ms timeout and an overall 1.5 s budget.
- Do **not** loop the existing `space_free` from the frontend. Its limit of 2 workers would starve the status bars (`commands.rs:914-920`).

---

## 3. Backend (Rust)

### 3.1 Core crate `crates/dualfile-core`

**`drives.rs`** (extend; stays pure and testable)
- `parse_mountinfo(text, self_mount)`. Mountinfo provides the fstype, source and mount options, and resolves bind duplicates by mount ID.
- Add to `Drive`: `device`, `fstype`, `kind: Root|Partition|Removable|Network|Fuse|Automount`, `read_only`, `source: Mount|Fstab`, `mounted: bool`, `fstab: Option<FstabInfo{user_mountable, automount, noauto}>`, `space: Option<SpaceFree>` (filled in the command layer).
- Kind heuristics: `/run/media/<user>/` and `/media/` → Removable; network fstypes → Network; `fuse.sshfs`/`rclone` → Network; other fuse → Fuse.
- Keep `/proc/mounts` parsing as a fallback.
- Existing tests that compare whole `Drive` structs (`drives.rs:141-226`) need updating.
- Remove `autofs` from the drop list *only* for fstab-listed automount points.

**New `fstab.rs`**
- `parse_fstab(text) -> Vec<FstabEntry>` with the option allow-list (§1.4) and credential stripping.
- `merge(drives, fstab)`.
- Fixture tests:
  - user/noauto NFS
  - CIFS with `credentials=` (must not leak in Debug or Serialize)
  - escaped spaces
  - UUID=/LABEL= sources
  - `x-gvfs-hide` and `x-gvfs-name`
  - systemd automount
  - comments and garbage
  - 256-entry cap

**New `user_dirs.rs`**
- Parse `$XDG_CONFIG_HOME/user-dirs.dirs` (`XDG_DESKTOP_DIR="$HOME/Desktop"`, quoted, `$HOME` expansion only).
- Drop entries that equal home or do not exist.
- Return a fixed set of kinds.
- Trash path: `$XDG_DATA_HOME/Trash`; `trash_state() -> {empty: bool}` via `read_dir(files).next()` (cheap).

**New `devices.rs`** (step 3)
- `parse_lsblk(json)` from `lsblk -J -b -o PATH,LABEL,FSTYPE,MOUNTPOINTS,RM,HOTPLUG,SIZE,TYPE,RO,UUID`.
- Return removable/hotplug partitions with a filesystem, excluding LUKS (shown as "verschlüsselt – im System öffnen").
- Generalise the bounded helper runner from `system_mount.rs:18-85` into `appenv::run_bounded(cmd, timeout) -> Result<(), HelperError>`, and make `system_mount` use it.

**`settings.rs`:** new fields, see §5.

### 3.2 `src-tauri`

**New `places.rs`**
- `MountWatcher` (poll + notify), a snapshot diff, and emission of `PlacesChanged`.
- Started in `lib.rs` setup next to `watch_minimize` (`lib.rs:111-112`), with access to the sink.

**New commands**

| Command | Step | Purpose |
|---|---|---|
| `places_snapshot` | 1 | One call returning `{ home, userDirs[], trash{path,empty}, drives[] (mounted + fstab, with space for local ones), networkMounts: SystemPlace[] (gvfs/kio, via `system_places::mounts`) }`. Runs in a worker with the space budget from §2.3. `local_drives` stays for compatibility; the place menu switches to the store. |
| `places_space` | 1 | Space for paths on demand (bounded). |
| `fstab_mount` | 2/3 | `fstab_mount(mount_point)`: allowed only if the point is in the parsed fstab **and** is `user`/`users`/`owner` **and** is not mounted. Runs `mount -- <point>`. |
| `fstab_unmount` | 2/3 | Same checks, `umount -- <point>`. |
| `trash_empty` | 2 | `trash::os_limited::purge_all(list())`, as a job; the UI confirms first. |
| `trash_open_system` | 2 | `xdg-open trash:/` with a clean environment, like `release_open` (`system.rs:345-347`). |
| `device_list` | 3 | lsblk. |
| `device_mount` | 3 | `udisksctl mount -b <dev>`. |
| `device_unmount` | 3 | `udisksctl unmount -b <dev>`, or `umount` for fstab `user` entries. |
| `device_eject` | 3 | `udisksctl unmount` then `power-off -b <dev>`. |

**Validation for all mount commands:**
- Re-resolve the target from fresh lsblk/mountinfo data. Never pass a UI-supplied device path through unchecked.
- Removable/hotplug devices only; root and system mounts are never allowed.
- Map errors: busy → `UiError("busy", "Laufwerk wird noch verwendet")`; helper missing → `Unavailable` with a hint (like `convert.rs:988`).

**Event:** `CoreEventDto::PlacesChanged` (`convert.rs:855-875`), sent through `CORE_EVENT`.

**Registration:** `build.rs` COMMANDS, `generate_handler!` in `lib.rs`, `allow-*` in `capabilities/default.json`, and the autogenerated permissions. Do **not** add them to `connection-editor.json`; its narrowness is asserted at `connection_editor.rs:303-323`. Add a vitest registration test for each command, following `tauri.test.ts:543-557`.

**Eject safety:**
- Refuse unmount/eject while a running job's sources or destination are under the mount point (job status already carries the destination; `controller.ts:403`).
- Before unmounting, navigate panes and tabs located under that mount to home (frontend).
- Folders that vanish are already reported by `watch.rs`.

### 3.3 mDNS (step 4, optional): cost/benefit
- **Benefit:** a NAS appears without typing a hostname. Real, but it shows *hosts*, not shares. Listing shares needs SMB share enumeration (`smb.rs` would need a share-list call; unverified) or NFS `showmount` (RPC).
- **Options:**
  1. `avahi-browse -rpt _smb._tcp _nfs._tcp _sftp-ssh._tcp _webdav._tcp`. No new crate, but avahi-utils is often not installed, so availability varies.
  2. Avahi over D-Bus via zbus (already compiled transitively; becomes a direct dependency). Needs avahi-daemon, which is standard on Plasma and GNOME.
  3. The pure-Rust `mdns-sd` crate. Works without avahi, but needs multicast sockets, can hit firewalls, and adds a thread and a dependency.
- **Recommendation:** option 2, as a later step. Results go into "Im Netzwerk gefunden"; click = open the connection editor prefilled with protocol/host (never auto-connect). Off by default via `sidebar_discovery: bool`, because it is network-chatty.

---

## 4. Relationship to the existing UI
- **Pane place menu: keep it.** It is the per-pane switcher, the fallback when the sidebar is hidden, and the anchor of the connection dot.
  - Change it to read drives, connections and XDG dirs from the **shared store** (`state.places`, loaded by the controller), so the two `PathBar` instances stop polling separately (`Pane.tsx:119-127`).
  - Optionally add the XDG folders. No functional duplication is removed.
- **Ribbon (bookmark bar):** it duplicates Places → bookmarks. Owner decision, to be shown in the mockup.
  - Recommendation: show the Ribbon only when the sidebar is `hidden`. Alt+1..9 keeps working in every mode, and the numbers appear as hints in the sidebar.
  - Alternative: keep both and add a separate setting.
- **ConnectionsDialog / SystemPlacesDialog:** unchanged. The sidebar links to them through "Verbindungen verwalten …".
- **"Settings as own window" (separate plan):** the new settings keys must go through the normal `Settings` → `SettingsDto` path, so the future settings window can show a "Seitenleiste" group (mode, show sections, mDNS, show hidden entries). The sidebar never opens settings UI itself, apart from linking to the settings page. No coupling beyond that.

---

## 5. Settings

**New fields** in Rust `Settings` (snake_case); in `SettingsDto` and TS they are camelCase and optional:

| Key | Type | Default | Load-time check (typed; repair only when invalid) | `validate.rs` check |
|---|---|---|---|---|
| `sidebar_mode` | `"hidden"\|"icons"\|"full"` | `"full"` | unknown value → default + fix | enum |
| `sidebar_width` | u16 | 210 | outside 160..=400 → clamp + fix | 160..=400 |
| `sidebar_collapsed` | Vec<String> | [] | drop unknown section ids (`places`, `remote`, `devices`, `network`) + fix | ≤8, each one of the ids |
| `sidebar_hidden` | Vec<String> | [] | at most 500 entries; each key must be `kind:value` with a kind in {`xdg`,`bookmark`,`drive`,`fstab`,`conn`,`mount`,`trash`,`net`} | ≤500, `plain_text(…, MAX_PATH_BYTES+16)`, no NUL |
| `sidebar_discovery` (step 4) | bool | false | typed | — |

**Backward compatibility:**
- Missing keys → `field()` returns the default without a fix (`settings.rs:233-234`), so old files load as `Loaded`, not `Repaired`.
- **Do not bump `SETTINGS_VERSION`.** A bump would produce `Migrated` for every user; there is precedent for adding fields without a bump (`column_widths`, `extension_column`).
- Old builds reading new files ignore the unknown keys (test at `settings.rs:808`).

**Changes:**
- Add `#[serde(default = …)]` on the `SettingsDto` fields, both conversions in `convert.rs` (around `:718` and `:781`), the TS `Settings` interface (`types.ts:432+`), and the mock defaults.
- Width is saved on pointer-up only, not on every move.
- The automatic narrowing from §1.1 is never written to settings.

**Rust tests:**
- `{"version":6}` → `Loaded`, sidebar defaults.
- Wrong types → `Repaired(["sidebar_mode"])`, etc.
- Width out of range → clamped + repaired.
- Hidden list cap.
- `validate_settings` accepts and rejects as in the table.

---

## 6. File-by-file change list

### Rust
- `crates/dualfile-core/src/drives.rs`: mountinfo parser, extended `Drive`, kinds, fstab merge hook; update tests.
- `crates/dualfile-core/src/fstab.rs` (new): parser with the option allow-list; fixtures.
- `crates/dualfile-core/src/user_dirs.rs` (new): XDG user dirs, trash path and state.
- `crates/dualfile-core/src/devices.rs` (new, step 3): lsblk parse; udisksctl wrappers.
- `crates/dualfile-core/src/appenv.rs`: `run_bounded` helper, moved out of `system_mount.rs`.
- `crates/dualfile-remote/src/system_mount.rs`: use `run_bounded`; behaviour unchanged (its existing tests at `:158-240` must still pass).
- `crates/dualfile-core/src/lib.rs`: module declarations.
- `crates/dualfile-core/src/settings.rs`: new fields, defaults, load and repair logic, tests.
- `src-tauri/src/places.rs` (new): `MountWatcher`, snapshot assembly, space budget, mount and trash commands' state logic.
- `src-tauri/src/commands.rs`: thin command wrappers (next to `:866-904`).
- `src-tauri/src/convert.rs`: `PlacesChanged`, the DTOs (`DriveDto`, `PlacesSnapshotDto`, …), settings DTO fields, mount error mapping.
- `src-tauri/src/validate.rs`: sidebar settings checks and tests.
- `src-tauri/src/lib.rs`: start the watcher in setup; register handlers.
- `src-tauri/build.rs`, `src-tauri/capabilities/default.json`, `src-tauri/permissions/autogenerated/*`: register the new commands.

### Frontend
- `src/ipc/types.ts`: `Drive` extension, `PlacesSnapshot`, `UserDir`, `CoreEvent` `placesChanged`, Settings fields.
- `src/ipc/api.ts`, `src/ipc/tauri.ts`: new API methods.
- `src/ipc/mock.ts` (+ `systemPlaces.ts`): mock snapshot with a USB stick, an fstab noauto NFS entry, a GVfs SMB mount, and a `devPlug()`/`devUnplug()` helper that emits `placesChanged` (the mock handler set is at `:3101-3106`).
- `src/app/state.ts`: `places` slice, `sidebarFocus`, a place context-menu variant (extend `ContextMenuState` with `place?: string`, or a separate `placeMenu`).
- `src/app/controller.ts`:
  - `refreshPlaces()` (coalesced), `placesChanged` handling, focus and 60 s timers, space refresh on job done;
  - `openPlace(key, target: "active"|"other"|"tab")`;
  - `openTransfer(kind, from, destination?)`, so the archive and read-only checks look at the destination;
  - `dropOnPlace`, `mountPlace`, `unmountPlace`, `ejectPlace`, `hidePlace`;
  - sidebar mode, width and collapse setters.
- `src/state/places.ts` (new, pure):
  - build the section model from snapshot + connections + bookmarks + settings;
  - entry keys; hidden filtering;
  - match the active place (longest-prefix);
  - badge letters; icon kinds;
  - unit-tested.
- `src/components/Sidebar.tsx` (new): tree, full and rail rendering, resize handle, tooltips, drop targets, live region.
- `src/components/SidebarMenu.tsx` (new) or an extended `ContextMenu.tsx`: the place context menu.
- `src/components/Pane.tsx`: the place menu reads from the store; remove the per-pane drives effect.
- `src/components/Ribbon.tsx`: visibility rule (§4).
- `src/components/Toolbar.tsx`: sidebar toggle button.
- `src/components/Icon.tsx`: new lucide icons (PanelLeft, HardDrive, Usb, Disc, FolderDown, Music, Image, Video, Trash2, Network, Eject, …).
- `src/app/commands.ts`, `src/state/keymap.ts`: `toggleSidebar` (F9), `toggleSidebarIcons` (Shift+F9), `focusSidebar`.
- `src/app/keyboard.ts`: sidebar exemption (§1.7).
- `src/App.tsx`: layout `<div className="body-row"><Sidebar/><div className="panes">…`.
- `src/styles/app.css`: sidebar, rail and badge styles, density variants, the narrowing breakpoint. `min-width` stays 900px.
- `src/i18n/de.ts`, `en.ts`: all strings (sections, states, fstab explanations, errors); parity is checked by the existing i18n test.
- `src/components/dialogs/SettingsDialog.tsx`: "Seitenleiste" group (until the settings-window plan lands).

---

## 7. Tests

**Rust (cargo test)**
- `fstab` fixtures (§3.1).
- mountinfo fixtures: bind mounts, escaped paths, `autofs` + real mount on the same point, CIFS with credentials in the super options (must not leak), sshfs, gvfsd-fuse filtered, `.mount_*` filtered.
- `user_dirs` fixtures.
- `parse_lsblk` fixtures: USB with two partitions, LUKS, optical, `RM=0 HOTPLUG=1`.
- Space budget: skips network types; a fake slow statvfs implemented via a trait is time-boxed.
- Watcher diff logic: an unchanged snapshot emits nothing; a changed one emits a single event per burst (injected tables plus a fake clock).
- Mount commands refuse: non-fstab points; non-`user` entries; root; devices that are not removable; UI-forged device paths.
- `run_bounded` timeout and sanitising: move the existing `system_mount` tests.
- Settings load, repair and validation (§5).
- Capability tests: the new commands are in `default.json` and not in the editor capability.

**Vitest**
- `state/places.test.ts`: section building, ordering, hidden filtering, active-place matching (nested mounts: `/run/media/u/X/sub` matches X, not `/`), unique badges, the rule for showing XDG dirs.
- `app.sidebar.test.tsx`:
  - clicks: active pane, Ctrl to the other pane, middle to a new tab;
  - a disconnected connection opens the connect dialog;
  - F9 / Shift+F9 modes are persisted;
  - automatic narrowing does not persist;
  - collapse is persisted;
  - keyboard tree navigation does not move the file list;
  - drop on a bookmark opens the transfer dialog with that destination;
  - drop on Trash trashes;
  - drop on a disconnected or unmounted entry is refused;
  - drag reorders bookmarks;
  - Hide / Show hidden;
  - mock `devPlug` → the entry appears and is announced in the live region; `devUnplug` while a pane is inside → the pane goes to home;
  - the fstab noauto entry is greyed, and Mount → navigate.
- Rail: `aria-label` contains name and state; the tooltip appears on focus; drop works on the icon.
- `tauri.test.ts`: registration for each new command.
- Update `app.orte.test.tsx` and the place-menu tests for the store-backed drive data.

**VM checks** (Plasma 6 and GNOME, AppImage build)
1. USB image: `truncate -s 256M stick.img; mkfs.vfat -n TESTSTICK stick.img; udisksctl loop-setup -f stick.img`. Expected:
   - GNOME automounts → appears live within 1 s;
   - Plasma, which does not automount by default → appears greyed in step 3, Mount works.
   - Fill it with data → the space bar updates after the job finishes.
   - Eject while a copy runs → refused. Eject while a pane is inside → the pane moves to home.
2. NFS from the host (`/etc/exports` on the host): an fstab line `host:/srv/share /mnt/nfs nfs noauto,user 0 0` → greyed; Mount works without root; unmount works. A second line without `user` → greyed with the explanation, and no action.
3. CIFS fstab with `credentials=/root/.smbcred,noauto,user` → the credentials path never appears in the UI, logs or `places_snapshot`.
4. `x-systemd.automount` NFS entry → shown as Automount; the click mounts; a powered-off NAS → no UI freeze (the space call is skipped; the listing times out with the existing error).
5. GNOME: Nautilus connects `smb://host/share` → appears live under Remote via `$XDG_RUNTIME_DIR/gvfs`; "Als Verbindung speichern" works. Plasma with kio-fuse: a Dolphin smb mount appears; without kio-fuse it is correctly absent (documented).
6. sshfs mount from a terminal → appears live.
7. Window at 900 px: the rail is forced and the panes are usable. At 1280 px: full. Width persists across restarts.
8. Orca (GNOME) and the KDE screen reader: navigate the tree, read rail labels and the live announcement.
9. Old 0.3.4 `settings.json` → the app starts without a "Repaired" notice; the sidebar is visible (full).
10. Step 4: two Samba hosts advertised with avahi on the LAN → listed; offline → removed.

---

## 8. Risks

| Risk | Mitigation |
|---|---|
| statvfs or listing hangs on dead network mounts | Skip network/fuse types for space; time-boxed worker (§2.3); never stat autofs points. |
| Credential leakage from fstab or mount options | Option allow-list; Debug/Serialize tests like `system_places.rs:584`. |
| Polkit prompt for udisks on non-removable devices; the 30 s timeout kills it | Removable-only in step 3; 120 s timeout for fstab-udisks; the user may cancel. |
| `mount`/`umount` setuid behaviour differs across distros | Only `user`/`users`/`owner` entries; clear error mapping; VM matrix. |
| GIO modules inside the AppImage | Avoid in-process GIO; external helpers with `apply_clean_env`. |
| Global key handler hijacking | Explicit sidebar exemption plus a test. |
| Width pressure at 900 px | Rail and automatic narrowing; min width unchanged. |
| Event storms (docker, snap, flatpak mounts) | Diff the visible set before emitting; 300 ms debounce. |
| Trash: browsing `Trash/files` in-app bypasses `.trashinfo` | No in-app trash browsing in v1. Click = open the system trash (`xdg-open trash:/`); drop = trash; Empty. Removable media trash (`.Trash-$UID`) is not covered. |
| Ribbon/sidebar duplication confuses users | Decide with the mockup (§4). |
| mDNS: firewall, multicast, privacy | Opt-in, later step. |

---

## 9. Suggested split

**Step 1: read-only sidebar with live places** (shippable alone)
- Snapshot command: XDG dirs, mountinfo + fstab (display only; unmounted fstab entries greyed with an explanation, no mount action yet), gvfs/kio mounts, saved connections with state, local free-space bars.
- `MountWatcher` and `PlacesChanged`.
- Full mode, rail, hidden; F9 / Shift+F9; resize; collapse; hide entries; keyboard and ARIA tree; Trash entry (open system trash).
- Place menu reads from the store.
- Settings keys plus loader and validation.

**Step 2: interaction**
- Drop targets (transfer dialog, trash), drag-in to pin, bookmark reordering.
- Context menu (connect, disconnect, save mount as connection, open in other pane / tab, properties, empty trash).
- fstab `user` mount/unmount via `mount`/`umount`; GVfs unmount via the existing `system_mount_unmount`.
- Ribbon visibility rule.

**Step 3: devices**
- lsblk listing of unmounted removable media; `/dev/disk/by-id` watch.
- udisksctl mount/unmount/eject/power-off with job and pane safety checks.
- fstab non-user local entries via udisks (polkit).

**Step 4 (optional): LAN discovery**
- Avahi over zbus; "Im Netzwerk gefunden"; opt-in setting.

---

## 10. Mockup (clickable HTML) must show
1. Default window at 1280 px, full sidebar, all four sections populated: XDG dirs, three coloured bookmarks, Trash (empty/full icon), two saved connections (one connected, one not), a GVfs SMB mount without a profile, a kernel NFS mount, root, `/home` partition, USB stick with an eject button, a 92 %-full disk (warning bar).
2. Active-place highlight, plus L/R pane markers.
3. Collapsed section; "Show hidden entries" state with a faded entry.
4. **Icon rail**: dividers, type icons with letter badges for two similar disks, status dots, underline space bars, a tooltip on hover *and* on keyboard focus, the expand button, a rail drop target highlighted during a drag.
5. Hidden mode: toolbar button off; Ribbon visible (per the §4 decision); F9 / Shift+F9 hints.
6. 900 px window: automatic rail; panes usable.
7. Resize-handle drag, and double-click switching icons ↔ full.
8. fstab entries:
   - `user` NFS unmounted (greyed, Mount);
   - non-user NFS (greyed, explanation tooltip, "Als Verbindung anlegen");
   - automount entry;
   - a mounted fstab entry with a space bar.
9. Live change: a USB stick plugged in (entry fades in with an announcement); unplugged while a pane is inside (pane goes to home, with a toast).
10. Context menus for a bookmark, a connection, a removable device, Trash and the section header.
11. Drag a file over a bookmark (copy/move indicator), then the transfer dialog prefilled; a refused drop on a disconnected connection (toast).
12. Mount progress and errors: busy on eject, helper missing, polkit cancelled.
13. Step 4 preview: "Im Netzwerk gefunden" with two NAS hosts, opening a prefilled connection editor.
14. Compact density, and the Papier (light) theme variant of 1 and 4.

### Critical files for implementation
- `crates/dualfile-core/src/drives.rs`
- `crates/dualfile-core/src/settings.rs`
- `src/app/controller.ts`
- `src/components/Pane.tsx`
- `src-tauri/src/commands.rs`
- `src-tauri/build.rs`
- `src-tauri/capabilities/default.json`
- `src/app/keyboard.ts`
- `crates/dualfile-core/src/system_places.rs`
- `crates/dualfile-remote/src/system_mount.rs`
## Amendments after Codex review (2026-11-16, mandatory before building)

Codex (gpt-6-astra) checked the plan against `ed51901`. Confirmed: missing settings keys load without `Repaired` (`settings.rs:233`), no version bump; `poll(POLLPRI|POLLERR)` on `/proc/self/mountinfo` is correct for our mount namespace (treat POLLERR as "changed", re-read). Verdict: not ready until these are settled:

- **P1 Automount/dead-share hangs already in step 1.** Local listings have no timeout (`commands.rs:48` → `state.rs:711` → `vfs.rs:957`), and `system_places.rs:427` (`is_dir`) can block too. Every places I/O (XDG existence checks, trash state, GVfs/KIO detection, statvfs) runs in a bounded, isolated worker pool with a response deadline; skipping network types for statvfs alone is not enough.
- **P1 Timeouts don't kill blocked syscalls.** Fixed global worker limit (no replacement thread per refresh), separate response deadline, and process cleanup; the runner from `system_mount.rs:78` calls `wait()` unbounded after `kill()` — fix that before reusing it. After a helper timeout, re-read the mount state instead of assuming the result.
- **P1 Credentials can leak via helper stderr.** `system_mount.rs:97` filters neither `credentials=…` nor `username=…`. Mount helpers return only safe error codes (busy, not permitted, not found, helper missing, timeout) or redacted diagnostics; tests that credential paths, user names and sensitive sources never appear in DTOs, Debug output or error texts.
- **P1 Unmount safety needs a backend basis, before any step-2 unmount.** The frontend only knows the transfer destination (`controller.ts:403`); `JobStatusDto` (`convert.rs:351`) lacks full sources/destinations. Track the mount points touched by every running/queued job in the backend, block new jobs on a mount that is being unmounted, and for `power-off` check all partitions of the whole drive.
- **P2 Separate authorisation matrix** for fstab entries and devices: mount requires "not mounted", unmount requires "mounted"; absolute, freshly resolved targets; arguments without a shell; `mount -- <point>` / `umount -- <point>`; no options from the UI. Distinguish `user`/`users`/`owner`; the helper decides the real permission. "Removable only" applies to udisks devices, not to fstab NFS/noauto entries. `user` alone does not guarantee that NFS/CIFS mounting works; map the error clearly.
- **P2 GVfs inotify is not a reliable live source** (GVfs adds its synthetic directories internally, no normal create/delete events; a watch on the runtime dir misses changes inside existing KIO mounts). Use bounded polling (e.g. 5 s while the sidebar is visible) or GIO/D-Bus mount signals via an external helper; re-arm after daemon restarts; test live behaviour explicitly.
- **P2 Keyboard compatibility.** Users may already have bound F9 (test `settings.rs:824` uses `copy: F9`); new defaults must not override existing bindings (`keymap.ts:189`): if a default key is taken, the new command stays unbound. The sidebar exemption in `useGlobalKeys` uses an explicit allow-list of commands (F9 toggles, Alt+1..9, Tab, Ctrl actions tested), not a check for particular keys.
- **P2 Missing API for step 2.** `system_mount_unmount` needs a saved `profile_id` (`commands.rs:901`, `connection_impl.rs:837`), so profile-less GVfs mounts cannot be unmounted. Add a validated mount-key endpoint; move the helper extraction and unmount protection from step 3 forward.
- **P3** Update references to the reviewed commit; `:300` (job status) is a wrong source; name the `system_mount` tests by function, not line range.

### Owner decisions (2026-11-16)
1. **F9 = show/hide** the sidebar (as in Dolphin, back to the last visible mode); **Shift+F9 = icons ↔ full**. (Still subject to the keyboard-compatibility rule above: if a user already bound F9, the new command stays unbound.)
2. **Ribbon bookmark bar is shown only while the sidebar is hidden.** Alt+1..9 keeps working in every mode; the numbers appear as hints next to the bookmarks in the sidebar.
3. **Default after update and for new installs: `full`.** The automatic narrowing to the icon rail on narrow windows (§1.1) still applies and is never persisted.

### Mockup approved (2026-11-16)
The owner approved the clickable mockup (https://claude.ai/artifact/TWagKuzfRQrPvZcSTLybn7, version 2) as the visual and interaction reference. Not yet built; start date open.
