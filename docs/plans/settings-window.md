# Plan: Settings dialog as its own OS window (Dualfile 0.3.4 → next)

I only read files and changed nothing. The line numbers below come from HEAD `83e3165`.

## 1. What the connection editor already gives us

- **Rust command** `src-tauri/src/connection_editor.rs`:
  - The `connection_editor_open` command is restricted to the `main` label (`:137`).
  - Opening is serialized by `OPEN_LOCK` (`:18`, `:139`). The `MAIN_SHUTTING_DOWN` flag is checked before and after `build()` (`:140`, `:161`). That is A3.
  - An existing window is focused instead of opened twice via `focus_existing` → `system::bring_to_front` (`:117`, `system.rs:87-98`). A "label already exists" error counts as already open (`:167`).
  - Window setup: `.transient_for(&window)` on Linux (`:154`), 520x640, minimum 400x320 (`:148`).
  - The query values are validated in Rust: `editor_query`, `Appearance` (`:27-115`).
  - The window can only close itself through `connection_editor_close` (`:178-184`).
  - `destroy_editor` (`:187`) is called when main is destroyed (`lib.rs:260-265`). `mark_main_closing` is also called on `Decision::Allow` (`lib.rs:136`).
- **Capabilities**:
  - `capabilities/connection-editor.json` has no `core:*` permissions.
  - `default.json` (main) has `core:event:allow-listen`.
  - Every command must be listed in `build.rs` COMMANDS and in `generate_handler` (`lib.rs:153-256`).
- **Frontend**:
  - `src/main.tsx:16-18` branches on `?view=` and lazy-imports the window.
  - `src/ConnectionEditorWindow.tsx:82-91` builds a `createRuntime(api)` that is never started. Its settings are written straight into the store, so `useT`, `useAppState` and so on work unchanged.
  - The window applies `applyTheme` and `lang` once (`:41-46`), focuses `[data-autofocus]` after loading (A4, `:60-62`), and closes on Escape (`:64-68`).
  - The root gets the class `editor-window-root`, which lifts the 900x560 body minimum (A2, `app.css:31`, `:546-548`).
- **Events**: `CoreEventDto::ConnectionsChanged` is broadcast as `core-event` via `TauriSink` (`lib.rs:50-56`) and handled at `controller.ts:1039`.

### Facts that drive the design

1. Settings are only written by main's controller.
   - `updateSettings` (`controller.ts:750`) only dispatches.
   - `onStoreChange` → `scheduleSave` (400 ms, `controller.ts:1149-1172`) → `saveNow` → `api.saveSettings` (`:1104-1131`).
   - Saving only happens after `start()` and `phase === "ready"` (`:1110`). So a runtime that is never started (as in the editor) never writes, even when its `updateSettings` is called.
2. `load_settings` has side effects (the editor plan noted it can write a backup). The new window must never call `load_settings` or `save_settings`.
3. **Tauri 2.12.5 `emit_to(label)` is not isolation.** `emit_js_filter` (`tauri-2.12.5/src/event/listener.rs:309-334`) uses `match_any_or_filter`. A JS `listen()` uses target `Any` by default, so it receives events sent with `emit_to` to another label. Any webview with `core:event:allow-listen` could therefore also read `core-event` (job paths) and `ssh-prompt` (`ssh_prompt.rs:32`).
   - So the settings window must not get `core:event:*`.
   - Main → settings therefore goes over a Tauri `ipc::Channel`. The repo already uses one: `system.rs:208`, `tauri.ts:93`.
   - The channel's large-payload fetch skips the ACL (`webview/mod.rs:2082`), so it needs no permission.
4. Some commands have no label check: `open_url` (`commands.rs:162`), `log_*` and `report_issue_url` (`diagnostics.rs:434-478`), `tray_set` and the `update_*` commands (`system.rs:58-345`). The capability is their only gate.
5. `verifyPolicy` and `terminalShell` take effect in Rust from the saved file: `state.rs:870`, `terminal.rs:379`. They only apply after main's debounced save, which is the same as today.
6. A value the save step rejects (`validate.rs:213-262`) makes every later save fail. So a patch from the window must pass the same checks before main applies it.

## 2. Design

### One writer, a mirror in the window, patches up and snapshots down

```
Settings window (no core:event)          Rust relay                         Main window (only writer)
 optimistic local apply              settings_window_send(msg) ──typed+validated──► emit_to("main","settings-window")
 seq-numbered send queue ───────────►  (settings-label only)                       bridge: whitelist → controller.updateSettings / system.*
                                                                                     ack = msg.seq; push()
 rebase(snapshot, unacked) ◄── Channel ── settings_window_push(snapshot,title) ◄──── store/system subscription (projection changed)
 settings_window_attach(channel)        caches latest snapshot; sets window title    (main-label only, serialized pushes)
```

**Why this design:**
- Main stays the only owner of state and the only caller of `save_settings`. That means no lost updates: patches merge key by key into main's live state, so a theme switched from the palette or a bookmark pinned in main at the same time survives.
- The settings window never needs event permissions.
- Rust caches the last snapshot, so a newly opened window gets its state when it attaches, and the open/attach timing does not matter.
- The window title follows the language through the push (Rust calls `set_title`). The window gets no `core:window` permission.

**Message protocol (TS ↔ Rust ↔ TS):**
- Window → main, `SettingsWindowMessage`: `{seq, kind:"patch", patch}`, `{seq, kind:"checkUpdates"}`, `{seq, kind:"setTray", enabled}`, `{seq, kind:"setStartMenu", enabled}`.
- Main → window, `SettingsWindowSnapshot`:
  - `ack`: the last seq main handled
  - `errors`: `{seq, message}[]`
  - `settings`: a projection containing only the keys listed below. No `bookmarks`, `session` or `columnWidths`, because the window has no use for them.
  - `system`: `{status, check, error, installing, trayUnavailable}`
  - `logDir`: string
- Projected keys: theme, accent, accentAutomatic, density, fontSize, language, showParentEntry, extensionColumn, paneColorLeft, paneColorRight, terminalColors, terminalBg, terminalFg, verifyPolicy, terminalShell, trayEnabled, checkUpdatesOnStart, lastUpdateCheck, keybindings. Patchable keys are the same minus `lastUpdateCheck`.

**Window side (optimistic and ordered):**
- The proxied `controller.updateSettings(update)` reads `before`, applies locally through the real non-started controller, reads `after`, and diffs the patchable keys into a patch.
- Sends are queued: each `invoke` is awaited before the next one. That gives ordering and keeps slider drags responsive.
- Every message gets a seq and stays in `unacked` with a promise.
- When a snapshot arrives: settle entries with `seq <= ack` (reject the ones listed in `errors`). Then set the store's settings to `{...defaultSettings, ...snapshot.settings}` with the remaining unacked patches applied on top.
  - The rebuild must start from `defaultSettings` (`state.ts:120`), not `{...s, ...proj}`. Otherwise a key main has removed keeps its stale value.
- If a send is rejected (Rust validation), drop that entry, rebase onto the last snapshot, and show a `role="alert"` line in the footer.
- **Do not render the panel until the first snapshot arrives** (`aria-busy`). Otherwise Keybindings would compute a full map from empty defaults and wipe main's custom bindings.

**Main side (`src/app/settingsWindow.ts`, a new bridge in `AppRuntime`):**
- Started from the existing `Shell` start effect (`App.tsx:91-97`), after `controller.start()` / `system.start()`.
- Handles messages one at a time through a promise chain, so `ack` only increases:
  - Patch: filter against the whitelist again, then `controller.updateSettings(s => ({...s, ...patch}))`.
  - `checkUpdates`: `void system.check(true)`, not awaited. Progress reaches the window through the snapshot.
  - `setTray`: `system.setTray(enabled, controller.language())`.
  - `setStartMenu`: `await system.setStartMenu(enabled)`. On error, add `{seq, message}` to `errors`.
- Then sets `ack` and calls `push()`.
- `push()` runs after store or system changes when the projection JSON, `ack` or `errors` changed. Pushes are serialized (each push awaited before the next) and clear `errors` after sending.
- It also pushes once after the phase becomes ready, so Rust's cache gets filled.
- Pushes go out even while no window is open. That is cheap because projected keys change rarely.

**Update install, jobs check, start-menu prompt and the banner stay in main**, unchanged (`SystemUpdates`, `SystemDialogs.tsx:126-174`, `system.ts:120-135`). The window only gets "Check now" (delegated) and the read-only status. The install banner appears in main. `system.install` already blocks while jobs are running (`system.ts:121`) and restarts the app, which takes the settings window with it.

**Fallback when no second window exists (browser mock and vitest):**
- `api.settingsWindowOpen` returns `"opened" | "focused" | "inline"`. Tauri maps its bool to opened/focused; the mock returns `"inline"`.
- On `"inline"` the controller opens the existing in-app modal (`dialog.kind === "settings"`, `App.tsx:64`), so the browser preview and the existing tests keep working.
- In Tauri, an error becomes a toast via `fail()`, the same as `openConnectionEditor` (`controller.ts:1045-1052`).

## 3. Every control: where it applies today, its IPC today, and how it works in the window

The "effect site" (where a change takes effect in main) is the same before and after for every row.

| Control | Code today | Effect site in main | IPC today | In the new window |
|---|---|---|---|---|
| Show ".." entry | `SettingsDialog.tsx:183` | reducer `state.ts:371-374`, `state.ts:216` | `save_settings` (debounced) | patch `showParentEntry` |
| Extension column | `:184` | `FileList.tsx:121` | `save_settings` | patch |
| Pane colour left/right, Clear | `:188-189`, `:193-194` (default shown is `themes[theme].panel`) | `Pane.tsx:287` | `save_settings` | patch (`""` clears; Rust allows `""` or hex, as `validate.rs:251`) |
| Theme | `:198-206` (also resets the local `hex`) | `App.tsx:85,99` `applyTheme` | `save_settings` | patch. The window applies the theme itself from its mirror. |
| Accent: default / presets / hex | `:211`, `:221-224`, `:228-235` via `onHex` `:149-152`; warnings computed locally `:144,239` | `applyTheme` | `save_settings` | patch. **Fix:** local `hex` (`:141`) is only set once. Add an effect that sets it back to `currentAccent` when theme, accent or accentAutomatic change from outside; the palette can now change the theme while the window is open (`commands.ts:57`). |
| Density | `:248-256` | `applyTheme` | `save_settings` | patch |
| Font size (range) | `:261-269` | `applyTheme` | `save_settings` | patch. The optimistic apply keeps the slider from jumping. |
| Language | `:275-283` | `context.tsx:27-35`, `controller.ts:81`, `App.tsx:100-102` | `save_settings` | patch. The window uses `useLanguage` from its mirror; the push re-sets the title. |
| Terminal colour mode, custom bg/fg, swap, contrast badge | `:103-131` (`TerminalColorSettings`) | `TerminalPanel.tsx:97` `xtermTheme` | `save_settings` | patch `terminalColors/Bg/Fg`. The badge is computed locally. |
| Verify policy | `:290` | Rust reads the file at copy time, `state.rs:870` | `save_settings` | patch (works after main's save, as today) |
| Terminal shell | `:297` | Rust `terminal.rs:379` when a terminal opens | `save_settings` | patch |
| Tray checkbox | `SystemDialogs.tsx:37-41` | `system.ts:72-79` → `tray_set` (`system.rs:105`) | `tray_set` + `save_settings` | patch `trayEnabled`, then `setTray` action. Main uses its own language. `trayUnavailable` (`:42`) comes from `snapshot.system`. |
| Start-menu status + Add/Remove | `:43-45` | `system.ts:90` `system_status`; `system.ts:115-119` | `start_menu_set` + `save_settings` | `setStartMenu` action, awaited until acked. Errors from `errors` are shown inline (`useAction`). |
| About: version | `:58` (static import of `tauri.conf.json`) | – | none | same |
| Project on GitHub | `:59` | `open_url` (`commands.rs:162`) | `open_url` | direct in the window (`allow-open-url`) |
| Updates: check on start | `:102` | `system.ts:88` at next start | `save_settings` | patch |
| Check now + status line | `:103-105` | `system.ts:55-71` → `update_check` (`system.rs:184`); banner `SystemDialogs.tsx:155` | `update_check` (+ `lastUpdateCheck` patch) | `checkUpdates` action. Status comes from `snapshot.system.check/error`, `lastUpdateCheck`, `installing`. The banner and install stay in main. |
| Log location / size | `:73-81` | – | `log_info` | direct (`allow-log-info`) |
| View log | `:83` → `LogDialog` (`LogDialog.tsx:15-73`; read `:37`, copy `:52`, save `:59` to the active pane's folder `:28-31`) | – | `log_read`, `log_save` | Shown inside the settings window as a `Modal` over the panel. Uses the window store's `openDialog({kind:"log",fromSettings})` and goes back to the Info tab (`LogDialog.tsx:21`). The save folder comes from the new `saveDir` prop = `snapshot.logDir`, computed in main with the same logic, extracted as `logSaveDir(state)`. |
| Open folder | `:84` | – | `log_open_folder` | direct |
| Report a problem | `:85` | – | `report_issue_url` + `open_url` | direct |
| Keyboard tab: Change (capture) / Reset / Replace / Reset all | `KeybindingsSettings.tsx:34-62`, `:90`, `:64-71,99`, `:110` | `keyboard.ts:76` `mapKey` | `save_settings` | patch `keybindings` (full map; Rust: ≤200 entries, each ≤64 bytes, no NUL, as `validate.rs:256-262`). Keys are captured in the window's own document, so main's `useGlobalKeys` (`keyboard.ts:16-90`) cannot see or interfere with them. The capture listener (window, capture phase, `stopPropagation`) means Escape only cancels the capture. |
| Tabs / start tab | `:145-180`; start tab from `dialog.tab` `:146` | – | – | local state. Start tab from the `?tab=` URL value (allowed: `appearance|system|info|keyboard`). |
| Done / Escape | `:158-162` | – | – | Done and Escape wait for the send queue to empty (timeout about 1 s), then call `settings_window_close`. Escape is ignored when `defaultPrevented` or `isModalOpen()` (log modal open; Modal handles Escape itself, `Modal.tsx`). |

"Show hidden files" is `state.showHidden` (`state.ts:376`). It is not part of the settings dialog, so nothing changes there.

## 4. Entry points

`grep openDialog({ kind: "settings"` finds:
- `Toolbar.tsx:77` (gear)
- `keyboard.ts:53` (Ctrl+,)
- `commands.ts:49` (palette)
- `LogDialog.tsx:21` (back to the Info tab; this now happens inside the window, or in the inline fallback)
- `devDemo.ts:19` (mock only; keep it)

Nothing in the connection list opens settings. The only tab ever passed is `"info"` (`state.ts:46`).

Change the first three to `controller.openSettings()`. The new function sits next to `openConnectionEditor`:
```ts
async function openSettings(tab?: SettingsTab) {
  const { theme, accent, accentAutomatic, density, fontSize } = get().settings;
  try {
    const r = await api.settingsWindowOpen(tab ?? null, tr("settings.title"), { theme, accent, accentAutomatic: accentAutomatic ?? false, density, fontSize, lang: language() });
    if (r === "inline") dispatch({ type: "dialog", dialog: { kind: "settings", ...(tab === "info" ? { tab } : {}) } });
  } catch (e) { fail(e); }
}
```
- When the window is already open ("focused"), there is no toast, unlike the editor: bringing the window forward is the expected result of Ctrl+, or the gear.
- While the window has focus, Ctrl+, there does nothing, because its document has no global key handler.
- Main's shortcuts keep working while the settings window is open, because `state.dialog` stays `null`. Rebound keys take effect in main immediately.

## 5. Window behaviour

- **Size:** inner 720x640 (the modal was `.dlg.wide` = 680 px, `app.css:168`), minimum 520x420, resizable, focused, no position set (Wayland).
- **Transient parent:** `transient_for(main)` on Linux, the same as the editor. The window can be moved anywhere; it only stacks above main.
- **Single instance:** label `settings`, shared `OPEN_LOCK` / `MAIN_SHUTTING_DOWN`, `focus_existing` → `bring_to_front`. A tab passed while the window is already open is ignored.
- **Destroyed with main:** generalise `destroy_editor` into "destroy all child windows" (editor + settings) at `lib.rs:262-265`. It is already covered by `mark_main_closing` (`lib.rs:136`).
- **Main hidden to the tray:** close the settings window too, both in `Decision::Hide` (`lib.rs:137-140`) and in `watch_minimize` (`system.rs:376-380`). By then every change is already applied in main, and a transient window would otherwise be left without a visible parent.
- **First paint:** appearance and language travel in the URL, validated by the existing `Appearance` checks (`connection_editor.rs:83-91`), so the window starts in the right theme. From then on, the theme and language follow the mirror live: `applyTheme` effect + `document.documentElement.lang` (copy `App.tsx:99-102`). The title is set by Rust on every push.
- **Focus:** `[data-autofocus]` (the Done button) is focused after the first snapshot has rendered (as in A4).
- **CSS:** reuse `html.editor-window-root` + `.editor-window` / `.editor-scroll`. Add a sticky footer (`.settings-window-foot`) for Done and the error line.

## 6. Least privilege

**New `src-tauri/capabilities/settings-window.json`** (`windows: ["settings"]`):
```
allow-settings-window-attach, allow-settings-window-send, allow-settings-window-close,
allow-open-url, allow-log-info, allow-log-read, allow-log-save, allow-log-open-folder, allow-report-issue-url
```
- Not included: any `core:*`, `load/save_settings`, `tray_set`, `start_menu_set`, `system_status`, `update_*`, `app_restart`, `release_open`, `log_frontend`. The controller's `logToFile` already swallows the rejection (`controller.ts:144`).
- `default.json` gains `allow-settings-window-open` and `allow-settings-window-push`.
- Defense in depth in Rust via `only_label`: `open` and `push` accept only `main`; `attach`, `send` and `close` accept only `settings`.

## 7. File-by-file changes

### Rust

1. **`src-tauri/src/connection_editor.rs`** — make `only_label`, `sanitize_title`, `Appearance`, `focus_existing(app, label)` and `mark_main_closing` `pub(crate)`. Move the appearance query building into `append_appearance(query, &Appearance)`, keeping the current validation. Extract the existing body of `connection_editor_open` into `pub(crate) fn open_child(app, parent, label, page, title, size, min) -> Result<bool, UiError>`. It keeps the lock, the before/after shutdown checks, the transient parent and the label-exists handling. Add `destroy_children(app)`, which destroys `EDITOR_LABEL` and `SETTINGS_LABEL`. Existing tests stay.
2. **`src-tauri/src/settings_window.rs` (new):**
   - `SETTINGS_LABEL = "settings"`.
   - `settings_query(tab, &Appearance)` with the tab whitelist.
   - `SettingsRelay { latest: Mutex<Option<Value>>, sink: Mutex<Option<Box<dyn Fn(&Value) -> bool + Send>>> }` with `attach`, `push` (cache, forward, drop the sink if forwarding fails) and `detach`. The closure keeps it testable without a webview, like `ssh_prompt.rs:32`.
   - `SettingsPatch` (`#[serde(rename_all="camelCase", deny_unknown_fields)]`, every field `Option`) plus `validate()`, which reuses the `validate.rs` rules: theme, density and language lists; terminal shell and colours; hex (`valid_color`, make it `pub(crate)`); pane colours `""` or hex; fontSize 8..=32; `VerifyPolicy` via serde; keybindings limits (`plain_text`).
   - `SettingsWindowMessage`, tagged by `kind`, with a `seq: u64`.
   - Commands:
     - `settings_window_open` (async, main only, `open_child` 720x640 / 520x420)
     - `settings_window_close`
     - `settings_window_attach(channel: Channel<Value>)`
     - `settings_window_push(snapshot: Value, title: String)`: main only. Also calls `set_title(sanitize_title)` on the window if it exists. Optionally cap the size at 256 KiB.
     - `settings_window_send(message)`: validates, then `emit_to("main", "settings-window", &message)`.
3. **`src-tauri/src/lib.rs`**
   - `mod settings_window;`
   - `app.manage(Arc::new(SettingsRelay::default()))` in setup (`:83-114`).
   - Register the 5 commands (`:153-256`).
   - `Decision::Hide` closes the settings window (`:137-140`).
   - `RunEvent::WindowEvent{label=="settings", Destroyed}` → `relay.detach()`.
   - The main-Destroyed branch calls `destroy_children` (`:262-265`).
4. **`src-tauri/src/system.rs:376-380`** — when main is hidden on minimize, close the settings window too.
5. **`src-tauri/src/validate.rs`** — make `valid_color` and `plain_text` `pub(crate)`.
6. **`src-tauri/build.rs`** — add the 5 command names. The autogenerated `permissions/autogenerated/*.toml` appear at build time.
7. **`src-tauri/capabilities/settings-window.json`** (new) and **`default.json`** (+2 permissions).

### TypeScript

8. **`src/ipc/types.ts`** — `SettingsTab`, `SettingsWindowSettings` (Pick of the projected keys), `SettingsWindowSnapshot`, `SettingsWindowMessage`.
9. **`src/ipc/api.ts`** — `settingsWindowOpen(tab, title, appearance): Promise<"opened"|"focused"|"inline">`, `settingsWindowClose()`, `settingsWindowAttach(onSnapshot): Promise<void>` (Channel), `settingsWindowSend(msg)`, `settingsWindowPush(snapshot, title)`, `onSettingsWindowMessage(handler): () => void`.
10. **`src/ipc/tauri.ts`** — implement them: Channel as at `:93`; `listen("settings-window")` with a shape check, as `onEvent` does at `:216-233`.
11. **`src/ipc/mock.ts`** (near `:2240`) — open returns `"inline"`; the others do nothing.
12. **`src/app/settingsWindow.ts`** (new):
    - Main bridge `createSettingsWindowBridge(api, store, controller, system)` with `start()` returning `stop()`, as described in section 2.
    - Shared constants `PROJECTED_KEYS` and `PATCH_KEYS`, plus `project(settings)`.
    - Window client `createSettingsWindowClient(api, runtime)`: queue, seq, unacked, rebase, and a proxied `controller` / `system` (`SystemController` shape; unused members do nothing).
    - Shared `logSaveDir(state)`.
13. **`src/app/runtime.ts`** — create the bridge after `system` and add `settingsWindow` to `AppRuntime`.
14. **`src/App.tsx:91-97`** — start/stop the bridge next to `system.start`. The inline `SettingsDialog` stays at `:64`.
15. **`src/app/controller.ts`** — add `openSettings(tab?)` next to `:1045` and export it next to `:1214`.
16. **`src/components/Toolbar.tsx:77`, `src/app/keyboard.ts:53`, `src/app/commands.ts:49`** — call `controller.openSettings()`.
17. **`src/components/dialogs/SettingsDialog.tsx`** — split it into `SettingsPanel({ startTab })` (the tabs and panel, `:165-306`) and `SettingsDialog` (Modal + Done, reading `dialog.tab` as now). Add the `hex` sync effect.
18. **`src/components/dialogs/LogDialog.tsx`** — optional `saveDir` prop; otherwise `logSaveDir(state)` as before (`:28-31`).
19. **`src/SettingsWindow.tsx`** (new):
    - `parseSettingsParams(search)` reuses the appearance parsing from `ConnectionEditorWindow.tsx:16-31` (export a `parseAppearance`).
    - Non-started runtime with the appearance seeded, as at `:84-89`, plus `dialog = {kind:"settings", tab}`.
    - Client attach; `<RuntimeProvider runtime={proxied}>`.
    - Live theme and lang effects, gate on the first snapshot, autofocus, Escape and Done → flush → close.
    - `LogDialog saveDir={snapshot.logDir}` when `dialog.kind === "log"`.
20. **`src/main.tsx:16`** — add the branch `view === "settings"` → lazy-import `SettingsWindow`.
21. **`src/styles/app.css:546+`** — footer and scroll rules for the settings window.
22. **`CHANGELOG.md`**.

## 8. Tests

**Rust:**
- `only_label` checks for all 5 commands.
- `settings_query`: encoding, tab whitelist, rejecting a bad appearance.
- `SettingsPatch`: unknown key, bad theme or language, `#abc`, fontSize 7/33, 201 keybindings and a NUL byte are rejected; `""` pane colour and valid values are accepted.
- Invariant test: a valid patch applied to a valid `SettingsDto` still passes `validate_settings`.
- Message `kind`, `seq` and an unknown `kind` are rejected.
  - Check that `deny_unknown_fields` behaves as expected with the tagged enum; if not, put it on the inner structs only.
- Relay: push before attach is cached and sent on attach; detach stops forwarding; a failing sink is dropped.
- Capability JSON tests (as `connection_editor.rs` tests at about `:303-330`):
  - `settings-window.json` equals the exact list in section 6.
  - `default.json` contains open and push but not attach, send or close.
  - **No capability other than `default` contains any `core:event` permission.** This guards the `emit_to("main")` assumption from section 1, fact 3.
- `build.rs` contains the 5 names.

**Vitest:**
- `src/SettingsWindow.test.tsx` with a fake api (attach captures the handler; send is recorded):
  - busy until the first snapshot
  - the snapshot applies the theme and `lang` to `documentElement`
  - a theme click sends `{kind:"patch",patch:{theme}}` with seq 1 and updates the UI right away
  - a stale snapshot (ack 0) does not revert (rebase)
  - a snapshot with a different language re-labels the UI live
  - removed keys are reset from the defaults
  - the tray toggle sends the patch and then `setTray`, in that order
  - the start-menu button shows the error from `errors[seq]`
  - "Check now" sends the action and the status follows the snapshot
  - Escape closes; Escape during key capture only cancels the capture; Escape with the log modal open only closes the log
  - Done flushes and then closes
  - the log save uses `logDir`
- `src/app/settingsWindow.test.ts` (main bridge with the real runtime and mock api):
  - a patch reaches `updateSettings`; `saveSettings` is called once after 400 ms
  - non-whitelisted keys are dropped
  - a palette theme change and a window fontSize patch at the same time both survive
  - the push contains `ack`, no `bookmarks` or `session`, and the `logDir`
  - a system state change triggers a push
  - messages are handled in order; ack only increases
- Controller and call sites: gear, Ctrl+, and palette call `settingsWindowOpen`; `"inline"` opens the modal.
  - The existing tests use `openDialog({kind:"settings"})` directly or render `SettingsDialog`, so they keep running against the inline path: `log.test.tsx`, `app.system.test.tsx`, `app.settings.test.tsx`, `SettingsTerminal.test.tsx`, `jobs10c.test.tsx`.
- `src/ipc/tauri.test.ts`: command names and arguments; the Channel is passed.

## 9. Manual VM checks

Run on the built AppImage: Plasma 5 and 6 Wayland, GNOME Wayland, one X11 session.

1. Open via gear, Ctrl+, and the palette.
   - Move the window outside the main window.
   - With it in the background, press Ctrl+, again: the existing window comes forward on KWin (focus-stealing prevention), and only one window exists.
   - Note whether the re-map in `bring_to_front` resets a position you dragged it to.
2. Change theme, accent, density and font size (drag the slider), language, pane colours and terminal colours (with the terminal open). Main and the settings window update together and the slider does not jitter.
3. Switch the theme from the palette in main while the window is open: the window follows and the hex field updates. The window title changes language.
4. Keyboard tab: capture Ctrl+Shift+G, F-keys and Escape (cancels only). Afterwards the new key works in main right away. Main's shortcuts keep working while the window is open.
5. **Single writer:** run `inotifywait` on `settings.json` during a fast slider drag → one debounced write from main. Also confirm the settings window process never opens the file.
6. Close main normally and with jobs running: the settings window disappears and no orphan `WebKitWebProcess` or process remains.
   - With the tray enabled: closing or minimising main hides it and the settings window closes; reopen from the tray.
7. Tray toggle from the window: the tray icon appears and disappears. Start-menu add/remove shows the right state; check the error path.
8. "Check now" in the window: the banner appears in main. Install from the banner while the settings window is open: the app restarts and both windows close.
9. Log viewer, copy, and save (into the folder of main's active pane), open folder, GitHub, Report a problem.
10. Minimum size 520x420: scrolls, no horizontal overflow, footer stays visible.
11. A `settings.json` from a newer version (read-only notice): changes apply live but are not saved, the same as today.
12. Restart: all changes made in the window persist.

## 10. Risks

- **Patch lost when closed with the X button.** Done and Escape flush the queue, but the window-manager close cannot be intercepted without `core:window` permissions. The window is a few milliseconds per change. Accepted and documented.
- **`emit_to` reaches other listening webviews.** It is not delivered only to the target label (Tauri 2.12.5). This is guarded by the capability test that no other window has `core:event`.
- **Two code paths (window and inline fallback).** The window path needs its own tests (`SettingsWindow.test.tsx`, bridge test). The fallback is only reachable through the mock.
- **The proxied `controller` is a spread of a non-started controller.** Methods other than `updateSettings`, `openDialog` and `closeDialog` act on a dead local runtime; without permissions their IPC fails safely. Keep the panel code to these methods.
- **Payloads over 8 KiB** (large keybinding maps) use the channel fetch path. It skips the ACL (`webview/mod.rs:2082`); verify it in the VM.
- **Keybindings sends the whole map.** This is safe because nothing in main changes keybindings concurrently, and the gate on the first snapshot prevents sending a map built from defaults.
- **Extra memory:** another WebKit process while the window is open, the same as the editor.

## Critical Files for Implementation
- /mnt/claude/projekte/dualfile/src-tauri/src/connection_editor.rs
- /mnt/claude/projekte/dualfile/src-tauri/src/lib.rs
- /mnt/claude/projekte/dualfile/src/components/dialogs/SettingsDialog.tsx
- /mnt/claude/projekte/dualfile/src/components/dialogs/SystemDialogs.tsx
- /mnt/claude/projekte/dualfile/src/app/controller.ts
- New files: /mnt/claude/projekte/dualfile/src-tauri/src/settings_window.rs, /mnt/claude/projekte/dualfile/src/app/settingsWindow.ts, /mnt/claude/projekte/dualfile/src/SettingsWindow.tsx, /mnt/claude/projekte/dualfile/src-tauri/capabilities/settings-window.json
## Amendments after Codex review (2026-11-16, mandatory before building)

Codex (gpt-6-astra) confirmed the `emit_to` finding (tauri 2.12.5 `src/manager/mod.rs:567`, `src/event/listener.rs:350`: JS `listen()` defaults to `EventTarget::Any`), so the Channel stays. Verdict: not ready until these are fixed:

- **S1 (P1) Sessions for seq/ack.** A reopened window starts at seq 1 while main/relay still hold e.g. `ack=20`, so a snapshot could acknowledge (and rebase away) unsent changes. Every window instance gets a session id (random, created on attach, held by the relay); messages and snapshots carry it; main keeps `ack` per session and rejects messages from old sessions; the client ignores snapshots of other sessions. Test: reopen while an action (e.g. start-menu) is still running.
- **S2 (P1) One close protocol for Done, Escape, the window's X, tray-hide and app exit.** An empty invoke queue only means Rust emitted the events, not that main applied them (main may still be awaiting `setStartMenu`). Intercept the window's X in Rust (`WindowEvent::CloseRequested` + `prevent_close()`, as `lib.rs:137` already does for main; no frontend `core:window` needed), ask the window to flush, wait until main has acknowledged the last seq, then close. On app exit main applies the last acknowledged patches and saves before quitting. No silent close after a timeout: show "Änderungen werden noch übernommen …" and keep the window open instead.
- **S3 (P2) Atomic relay attach/push.** Guard cache, sink and delivery with ONE mutex (or a single ordered task) so attach sends the newest snapshot and no push can slip between reading the cache and registering the sink. Tests: concurrent attach/push, re-attach.
- **S4 (P2) Persistence state in the window.** `ack` means "applied", not "saved". Mirror the read-only/persistence state (`newerVersion`, save errors; today only `src/components/NoticeBar.tsx:18` in main shows it) into the snapshot and show it in the settings window.
