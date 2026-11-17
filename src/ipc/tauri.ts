import type { UpdateProgress } from "./system";
// Echte `DualfileApi`: dünne Schicht über die Tauri-Befehle (`src-tauri/src/commands.rs`) und das
// Ereignis `core-event`. Keine Logik, nur Befehlsnamen, Argumentform und Fehlerabbildung.
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CoreError, type DualfileApi } from "./api";
import type { SettingsWindowDelivery, SettingsWindowMessage, CoreEvent, SshPrompt } from "./types";

/** Name des Ereignisses, über das der Kern Aufträge, Konflikte und Ordneränderungen meldet. */
export const CORE_EVENT = "core-event";

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

/** Der Kern liefert `{ kind, message }`; alles andere (Zeichenkette, Tauri-Fehler) wird `io`. */
export function toCoreError(error: unknown): CoreError {
  if (error instanceof CoreError) return error;
  if (isRecord(error) && typeof error.message === "string") {
    return new CoreError(error.message, typeof error.kind === "string" ? error.kind : "io");
  }
  if (typeof error === "string" && error !== "") return new CoreError(error, "io");
  return new CoreError("Unbekannter Fehler", "io");
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toCoreError(error);
  }
}

/**
 * `read_block` kommt als roher binärer IPC-Weg (ArrayBuffer), nicht als JSON-Zahlenfeld und nicht
 * als Base64. Ein Zahlenfeld wird ebenfalls angenommen (ältere/andere Laufzeit).
 */
function toBytes(value: unknown): Uint8Array {
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (Array.isArray(value) && value.every((byte) => Number.isInteger(byte))) return Uint8Array.from(value);
  throw new CoreError("Unerwartete Antwort beim Lesen der Datei", "io");
}

/** Nur bekannte Ereignisformen erreichen die Oberfläche. */
function isCoreEvent(value: unknown): value is CoreEvent {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case "closeSaveRequested": return true;
    case "closeRequested": return typeof value.count === "number";
    case "itemError": return isRecord(value.error);
    case "job":
      return isRecord(value.status);
    case "conflict":
      return isRecord(value.conflict);
    case "dirChanged":
      return typeof value.path === "string";
    case "connectionsChanged": return true;
    default:
      return false;
  }
}

export function createTauriApi(): DualfileApi {
  const terminalListeners = new Set<Promise<unknown>>();
  function terminalListener<T>(name: string, handler: (event: T) => void): () => void {
    let active = true;
    let stop: (() => void) | undefined;
    // Opening waits for event registration, so a short-lived shell cannot beat listen().
    const ready = listen<T>(name, event => { if (active) handler(event.payload); }).then(unlisten => {
      if (active) stop = unlisten; else unlisten();
      return null;
    }).catch(error => error);
    terminalListeners.add(ready);
    return () => { active = false; stop?.(); terminalListeners.delete(ready); };
  }
  return {
    terminalOpen: async (cwd, rows, cols) => {
      const failures = await Promise.all(terminalListeners);
      const error = failures.find(result => result !== null);
      if (error !== undefined) throw toCoreError(error);
      return call("terminal_open", { cwd: cwd ?? null, rows, cols });
    },
    terminalWrite: (id, data) => call("terminal_write", { id, data }),
    terminalResize: (id, rows, cols) => call("terminal_resize", { id, rows, cols }),
    terminalClose: id => call("terminal_close", { id }),
    onTerminalOutput: handler => terminalListener("terminal-output", handler),
    onTerminalExit: handler => terminalListener("terminal-exit", handler),
    settingsWindowOpen: async (tab, title, appearance) => await call<boolean>("settings_window_open", { tab, title, appearance }) ? "opened" : "focused",
    settingsWindowClose: (session, lastSeq, force = false, cancel = false) => call("settings_window_close", { session, lastSeq, force, cancel }),
    settingsWindowAttach: handler => {
      const channel = new Channel<SettingsWindowDelivery>();
      channel.onmessage = handler;
      return call("settings_window_attach", { channel });
    },
    settingsWindowSend: message => call("settings_window_send", { message }),
    settingsWindowPush: (snapshot, title, close = false, control = {}) => call("settings_window_push", { snapshot, title, close, control: { cancelClose: control.cancelClose ?? false, error: control.error ?? null } }),
    onSettingsWindowMessage: handler => terminalListener<SettingsWindowMessage>("settings-window", message => {
      if (isRecord(message) && typeof message.session === "string" && Number.isSafeInteger(message.seq) && message.seq >= 0 &&
        (["session", "closed", "closeFailed", "flush", "checkUpdates"].includes(message.kind) ||
         message.kind === "patch" && isRecord(message.patch) ||
         ["setTray", "setStartMenu"].includes(message.kind) && typeof (message as { enabled?: unknown }).enabled === "boolean")) handler(message);
    }),
    systemStatus: () => call("system_status"),
    startMenuSet: enabled => call("start_menu_set", { enabled }),
    traySet: (enabled, language) => call("tray_set", { enabled, language }),
    updateCheck: () => call("update_check"),
    updateInstall: (version, onProgress) => {
      const progress = new Channel<UpdateProgress>();
      progress.onmessage = onProgress;
      return call("update_install", { version, progress });
    },
    updateCancel: () => call("update_cancel"),
    restart: () => call("app_restart"),
    releaseOpen: () => call("release_open"),
    spaceFree: path => call("space_free", { path }),
    usageStart: (path, options) => call("usage_start", { path, options }),
    usageNode: (id, nodePath) => call("usage_node", { id, nodePath }),
    usageClose: (id) => call("usage_close", { id }),
    similarStart: (roots, options) => call("similar_start", {roots, options}),
    similarPage: (id, offset, limit) => call("similar_page", {id, offset, limit}),
    similarClose: id => call("similar_close", {id}),
    similarSuggest: (id, rule) => call("similar_suggest", {id, rule}),
    similarThumbnail: (id, groupIndex, entryIndex) => call("similar_thumbnail", {id, groupIndex, entryIndex}),
    similarTrash: (id, choices) => call("similar_trash", {id, choices}),
    dupesStart: (roots, options) => call("dupes_start", { roots, options }),
    dupesPage: (id, offset, limit) => call("dupes_page", { id, offset, limit }),
    dupesClose: (id) => call("dupes_close", { id }),
    dupesSuggest: (id, rule, folder) => call("dupes_suggest", { id, rule, folder }),
    dupesTrash: (id, choices) => call("dupes_trash", { id, choices }),
    appCloseNow: () => call("app_close_now"),
    appConfirmClose: () => call("app_confirm_close"),
    answerItemError: (jobId, decision) => call("answer_item_error", { jobId, decision }),
    undoList: () => call("undo_list"),
    undoRun: (id) => call("undo_run", { id }),
    onUndoChanged(handler) {
      let active = true;
      let stop: (() => void) | undefined;
      void listen("undo-changed", () => { if (active) handler(); }).then((unlisten) => {
        if (active) stop = unlisten; else unlisten();
      }).catch(() => console.error("undo events unavailable"));
      return () => { active = false; stop?.(); };
    },
    list: (path) => call("list", { path }),
    homeDir: () => call("home_dir"),
    copy: (sources, destination) => call("copy", { sources, destination }),
    // „move“ ist in Rust ein Schlüsselwort.
    move: (sources, destination) => call("move_items", { sources, destination }),
    packArchive: (sources, destination, name, format) => call("pack_archive", { sources, destination, name, format }),
    trash: (items) => call("trash", { items }),
    restoreFromTrash: (items) => call("restore_from_trash", { items }),
    planDelete: (items) => call("plan_delete", { items }),
    deletePermanent: (plan, confirmedToken) => call("delete_permanent", { plan, confirmedToken }),
    rename: (path, newName) => call("rename", { path, newName }),
    renameMany: (folder, items) => call("rename_many", { folder, items }),
    regexReplace: (names, pattern, replacement, ignoreCase, all) =>
      call("regex_replace", { names, pattern, replacement, ignoreCase, all }),
    makeDir: (parent, name) => call("make_dir", { parent, name }),
    openExternal: (path) => call("open_external", { path }),
    openUrl: (url) => call("open_url", { url }),
    logInfo: () => call("log_info"),
    logRead: (anonymize) => call("log_read", { anonymize }),
    logSave: (dir, anonymize) => call("log_save", { dir, anonymize }),
    logOpenFolder: () => call("log_open_folder"),
    reportIssueUrl: () => call("report_issue_url"),
    logFrontend: (level, message) => call("log_frontend", { level, message }),
    jobPause: (id) => call("job_pause", { id }),
    jobResume: (id) => call("job_resume", { id }),
    cancelJob: (id) => call("cancel_job", { id }),
    answerConflict: (jobId, answer) => call("answer_conflict", { jobId, answer }),
    jobs: () => call("jobs"),
    loadSettings: () => call("load_settings"),
    saveSettings: (settings) => call("save_settings", { settings }),
    watch: (paths) => call("watch", { paths }),
    fileInfo: (path) => call("file_info", { path }),
    mediaTags: (paths) => call("media_tags", { paths }),
    readBlock: async (path, offset, len) => toBytes(await call("read_block", { path, offset, len })),
    // Die Suchbytes gehen als Zahlenfeld (klein, höchstens 4 KiB); ein Uint8Array würde zu einem Objekt.
    findInFile: (path, needle, start, direction, ignoreCase) =>
      call("find_in_file", { path, needle: Array.from(needle), start, direction, ignoreCase }),
    cancelSearch: () => call("cancel_search"),
    compareDirs: (left, right, options) => call("compare_dirs", { left, right, options }),
    comparePage: (id, offset, limit, filter) => call("compare_page", { id, offset, limit, filter: filter ?? {} }),
    compareClose: (id) => call("compare_close", { id }),
    planSync: (compareId, mode, options, selection) =>
      call("plan_sync", { compareId, mode, options, selection: selection ?? null }),
    syncPlanPage: (token, offset, limit) => call("sync_plan_page", { token, offset, limit }),
    runSync: (token) => call("run_sync", { token }),
    diffFiles: (left, right, options) => call("diff_files", { left, right, options }),
    searchStart: (root, options) => call("search_start", { root, options }),
    searchPage: (id, offset, limit) => call("search_page", { id, offset, limit }),
    searchClose: (id) => call("search_close", { id }),
    connectionsList: () => call("connections_list"),
    connectionSave: (profile) => call("connection_save", { profile }),
    connectionDelete: (profileId) => call("connection_delete", { profileId }),
    connectionConnect: (profileId, secret, expected) => call("connection_connect", { profileId, secret: secret ?? null, expected: expected ?? null }),
    connectionEditorOpen: (profileId, title, appearance) => call("connection_editor_open", { profileId, title, appearance }),
    connectionEditorClose: () => call("connection_editor_close"),
    connectionTrust: (profileId, decision, fingerprint) => call("connection_trust", { profileId, decision, fingerprint }),
    connectionResetTrust: (profileId) => call("connection_reset_trust", { profileId }),
    connectionDisconnect: (profileId) => call("connection_disconnect", { profileId }),
    secretSet: (profileId, secret) => call("secret_set", { profileId, secret }),
    secretClear: (profileId) => call("secret_clear", { profileId }),
    pluginsList: () => call("plugins_list"),
    pluginActivate: (id, manifestHash, grantWrite, unsandboxed, confirmUnsandboxed) =>
      call("plugin_activate", { id, manifestHash, grantWrite, unsandboxed, confirmUnsandboxed }),
    pluginDeactivate: (id) => call("plugin_deactivate", { id }),
    pluginTrustReset: (id) => call("plugin_trust_reset", { id }),
    pluginDiagnostics: (id) => call("plugin_diagnostics", { id }),
    pluginColumns: (paths, columnIds) => call("plugin_columns", { paths, columnIds }),
    pluginPreview: (path, pluginId) => call("plugin_preview", { path, pluginId: pluginId ?? null }),

    placesSnapshot: () => call("places_snapshot"),
    placesSpace: path => call("places_space", { path }),
    placesSetVisible: visible => call("places_set_visible", { visible }),
    trashOpenSystem: () => call("trash_open_system"),
    placesMountAffected: (mountKey, paths) => call("places_mount_affected", { mountKey, paths }),
    placesSaveConnection: (key) => call("places_save_connection", { key }),
    trashEmpty: () => call("trash_empty"),
    fstabMount: (mountPoint) => call("fstab_mount", { mountPoint }),
    fstabUnmount: (mountPoint) => call("fstab_unmount", { mountPoint }),
    placesMountUnmount: (mountKey) => call("places_mount_unmount", { mountKey }),
    sidebarResizeWindow: delta => call("sidebar_resize_window", { delta }),
    localDrives: () => call("local_drives"),
    systemPlacesList: () => call("system_places_list"),
    systemPlacesImport: (urls) => call("system_places_import", { urls }),
    systemMountConnect: (profileId) => call("system_mount_connect", { profileId }),
    systemMountUnmount: (profileId) => call("system_mount_unmount", { profileId }),
    sshHostsList: () => call("ssh_hosts_list"),
    sshHostResolve: (alias) => call("ssh_host_resolve", { alias }),
    sshPromptAnswer: (id, text) => call("ssh_prompt_answer", { id, text }),
    onSshPrompt(handler) {
      let active = true;
      let unlisten: (() => void) | null = null;
      listen<SshPrompt>("ssh-prompt", (event) => {
        if (active) handler(event.payload);
      }).then((stop) => {
        if (active) unlisten = stop;
        else stop();
      }).catch(() => { /* No window remains to receive prompts. */ });
      return () => { active = false; unlisten?.(); };
    },
    onEvent(handler) {
      let active = true;
      let unlisten: (() => void) | null = null;
      listen<unknown>(CORE_EVENT, (event) => {
        if (active && isCoreEvent(event.payload)) handler(event.payload);
      })
        .then((stop) => {
          // Wurde schon abgemeldet, bevor die Anmeldung fertig war, sofort wieder lösen.
          if (active) unlisten = stop;
          else stop();
        })
        .catch((error: unknown) => console.error("Ereignisse des Kerns nicht verfügbar", error));
      return () => {
        active = false;
        unlisten?.();
        unlisten = null;
      };
    },
  };
}
