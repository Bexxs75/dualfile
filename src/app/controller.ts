import { buildPlaces, inside } from "../state/places";
import { systemPlaceExists } from "../ipc/systemPlaces";
import { currentVersion, releaseNotes } from "../release-notes";
import { selectReleaseVersions, seenVersionOnClose } from "../state/releaseNotes";
import { formatJobPresentation } from "../state/jobPresentation";
// Verbindet Zustand, Zustandsmodule und die API: alle Abläufe, die etwas anstoßen (Laden, Kopieren,
// Löschen …). Komponenten rufen nur diese Methoden und lesen den Zustand.
import { CoreError, type DualfileApi, type NativeDropEvent } from "../ipc/api";
import type { ConflictInfo, CoreEvent, Entry, JobStatus, PackFormat, RenameItem, Resolution, Settings, TrashEntry } from "../ipc/types";
import { createTranslator, errorKey, resolveLanguage, type Language, type TranslationKey, type TranslationParams } from "../i18n";
import type { Command } from "../state/keymap";
import { bookmarkNumber } from "../state/keymap";
import * as bookmarkOps from "../state/bookmarks";
import { nameError } from "../state/names";
import { deletePreset, emptyTags, sanitizePresets, savePreset, type MultiRenameRules } from "../state/multirename";
import {
  confirmDelete as confirmDeleteToken,
  deleteDialogModel,
  isFinished,
  planOperation,
  planPaths,
  withDestination,
  type OperationKind,
  type OperationPlan,
  type PaneSnapshot,
} from "../state/operations";
import { currentRemotePath, parseRemotePath, archiveHostFolder, archiveRootOf, baseName, isArchiveName, isArchivePath, isTrashPath, TRASH_PATH, joinPath, normalizePath, parentPath } from "../state/paths";
import { suggestPackBase } from "../state/pack";
import * as sel from "../state/selection";
import { formatSort, toggleSort, visibleSort, type SortKey } from "../state/sort";
import * as tabs from "../state/tabs";
import { createTypeahead, isTypeaheadKey, typeahead as typeaheadStep } from "../state/typeahead";
import { accentPresets } from "../themes/tokens";
import { createCompareFeature } from "./compare";
import type { Store } from "./store";
import {
  type Dialog,
  activeTabOf,
  focusedEntry,
  otherWindow,
  settingsToPersist,
  visibleEntries,
  type Action,
  type AppState,
  type JobKind,
  type WinIdx,
} from "./state";

export interface Timers {
  set(callback: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface ControllerOptions {
  /** Wartezeit bis zum Speichern nach einer Änderung (entprellt). */
  saveDelayMs?: number;
  toastMs?: number;
  /** Erfolgreich beendete Aufträge verschwinden nach dieser Zeit aus der Liste. */
  jobDismissMs?: number;
  clock?: () => number;
  navigatorLanguage?: string;
  timers?: Timers;
}

const realTimers: Timers = {
  set: (callback, ms) => setTimeout(callback, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Fehlerarten, bei denen der Papierkorb nicht „ausgefallen“ ist: hier hilft endgültiges Löschen nicht. */
const NO_FALLBACK_KINDS = new Set(["notFound", "invalidPath", "invalidName", "cancelled", "invalidTarget", "readOnly"]);

export type AppStore = Store<AppState, Action>;

export function createController(api: DualfileApi, store: AppStore, options: ControllerOptions = {}) {
  const timers = options.timers ?? realTimers;
  const saveDelay = options.saveDelayMs ?? 400;
  const toastMs = options.toastMs ?? 4500;
  const jobDismissMs = options.jobDismissMs ?? 8000;
  const clock = options.clock ?? (() => Date.now());
  const navLanguage = options.navigatorLanguage ?? (typeof navigator === "undefined" ? undefined : navigator.language);

  const get = (): AppState => store.getState();
  const dispatch = (action: Action): void => store.dispatch(action);
  const language = (): Language => resolveLanguage(get().settings.language, navLanguage);
  const tr = (key: TranslationKey, params?: TranslationParams): string => createTranslator(language())(key, params);

  const compareFeature = createCompareFeature({ api, get, dispatch, tr, toast, describeError });

  const reqSeq: [number, number] = [0, 0];
  const pageSize: [number, number] = [10, 10];
  let typeaheadState = createTypeahead();
  const timerHandles = new Set<unknown>();
  let unsubscribeEvents: (() => void) | null = null;
  let unsubscribeStore: (() => void) | null = null;
  let disposed = false;

  const unmounting = new Set<string>();
  let placesRequest: Promise<void> | null = null;
  let placesAgain = false;
  let discoveredServices: import("../ipc/types").DiscoveredService[] = [];
  let discoveryRequest: Promise<void> | null = null;
  let discoveryAgain = false;
  let discoveryEpoch = 0;
  let lastVisibleMode: "icons" | "full" = "full";
  let placesVisibility: boolean | null = null;
  function sidebarVisible() { return (get().settings.sidebarMode ?? "full") !== "hidden" && (typeof document === "undefined" || document.visibilityState !== "hidden"); }
  function syncPlacesVisibility() {
    const visible = sidebarVisible();
    if (placesVisibility === visible) return;
    placesVisibility = visible;
    void api.placesSetVisible(visible).catch(() => {});
    if (visible) void refreshPlaces();
  }
  function refreshDiscovery(): Promise<void> {
    if (discoveryRequest) { discoveryAgain = true; return discoveryRequest; }
    discoveryRequest = (async () => {
      do {
        discoveryAgain = false;
        const epoch = discoveryEpoch;
        try {
          const services = await api.discoverySnapshot();
          if (disposed) return;
          if (epoch !== discoveryEpoch || discoveryAgain) continue;
          discoveredServices = get().settings.sidebarDiscovery ? services : [];
          const snapshot = get().places;
          if (snapshot) dispatch({ type: "places", snapshot: { ...snapshot, discoveredServices }, announcement: get().placesAnnouncement });
        } catch { /* Retain the last discovery cache until the next event. */ }
      } while (discoveryAgain && !disposed);
    })().finally(() => { discoveryRequest = null; });
    return discoveryRequest;
  }
  function refreshPlaces(): Promise<void> {
    if (placesRequest) { placesAgain = true; return placesRequest; }
    placesRequest = (async () => {
      do {
        placesAgain = false;
        try {
          const snapshot = await api.placesSnapshot();
          if (disposed) return;
          // A slow places response must not overwrite a newer discovery-only update.
          snapshot.discoveredServices = discoveredServices;
          const old = get().places;
          const mounted = (p: import("../ipc/types").PlacesSnapshot) => [...p.drives.filter(d => d.mounted !== false || d.fstab?.automount), ...p.networkMounts.filter(m => m.mountedAt).map(m => ({ path: m.mountedAt!, name: m.name }))];
          const before = old ? mounted(old) : [], after = mounted(snapshot);
          const removed = before.filter(d => !old?.drives.some(previous => previous.path === d.path && previous.fstab?.automount) && !after.some(n => n.path === d.path));
          const added = old ? after.filter(d => !before.some(n => n.path === d.path)) : [];
          const plugged = old ? snapshot.drives.filter(d => d.mounted === false && d.deviceActions && !old.drives.some(p => p.deviceActions?.key === d.deviceActions?.key)) : [];
          const unplugged = old?.drives.filter(d => d.mounted === false && d.deviceActions && !snapshot.drives.some(p => p.deviceActions?.key === d.deviceActions?.key)) ?? [];
          const announcement = [...plugged.map(d => tr("sidebar.deviceAdded", { name: d.name })), ...unplugged.map(d => tr("sidebar.removed", { name: d.name })), ...added.map(d => tr("sidebar.added", { name: d.name })), ...removed.map(d => tr("sidebar.removed", { name: d.name }))].join(". ");
          dispatch({ type: "places", snapshot, announcement: announcement || get().placesAnnouncement });
          let movedTabs = false;
          for (const w of [0, 1] as const) {
            const affected = (path: string) => removed.some(d => d.path !== "/" && ![...unmounting].some(root => pathInPlace(path, root)) && inside(path, d.path));
            const reloadActive = affected(activeTabOf(get(), w).path);
            if (get().windows[w].tabs.some(t => affected(t.path))) {
              movedTabs = true;
              reqSeq[w]++;
              dispatch({ type: "window", w, update: win => ({ ...win, tabs: win.tabs.map(t => affected(t.path) ? tabs.createTab(get().home, t.sort) : t) }) });
              if (reloadActive) void reload(w);
            }
          }
          if (movedTabs) toast(tr("sidebar.removedHome"));
        } catch { /* Keep the last complete snapshot when a filesystem misses its deadline. */ }
      } while (placesAgain && !disposed);
    })().finally(() => { placesRequest = null; });
    return placesRequest;
  }
  function toggleSidebar() {
    const mode = get().settings.sidebarMode ?? "full";
    if (mode !== "hidden") lastVisibleMode = mode;
    updateSettings(s => ({ ...s, sidebarMode: mode === "hidden" ? lastVisibleMode : "hidden" }));
  }
  function toggleSidebarIcons() {
    const mode = get().settings.sidebarMode ?? "full";
    lastVisibleMode = mode === "icons" ? "full" : "icons";
    updateSettings(s => ({ ...s, sidebarMode: lastVisibleMode }));
  }
  function setSidebarWidth(width: number) {
    updateSettings(s => ({ ...s, sidebarWidth: Math.max(160, Math.min(400, Math.round(width))), sidebarMode: "full" }));
  }
  function collapseSidebarSection(id: string, collapse: boolean) {
    updateSettings(s => ({ ...s, sidebarCollapsed: collapse ? [...new Set([...(s.sidebarCollapsed ?? []), id])] : (s.sidebarCollapsed ?? []).filter(v => v !== id) }));
  }
  function hidePlace(key: string, hidden: boolean) {
    if (key.startsWith("bookmark:")) return;
    updateSettings(s => ({ ...s, sidebarHidden: hidden ? [...new Set([...(s.sidebarHidden ?? []), key])].slice(0, 500) : (s.sidebarHidden ?? []).filter(v => v !== key) }));
  }
  async function openPlace(key: string, target: "active" | "other" | "tab" = "active") {
    const state = get();
    const place = buildPlaces(state.places, state.connections, state.settings, state.home, true).flatMap(s => s.entries).find(e => e.key === key);
    if (!place || place.disabled || isUnmountBusy()) return;
    if (place.discoveryKey) { await openConnectionEditor(undefined, place.discoveryKey); return; }
    let destination = place.path;
    if (place.state === "unmounted") {
      if (!place.canMount) return;
      dispatch({ type: "dialog", dialog: { kind: "placeConfirm", action: "mount", key, busy: true } });
      try {
        if (key.startsWith("fstab:") && place.path) await api.fstabMount(place.path);
        else if (place.deviceKey) destination = await api.deviceMount(place.deviceKey);
        await refreshPlaces();
      }
      catch (error) { placeFailure(error); return; }
      finally { dispatch({ type: "dialog", dialog: null }); }
    }
    if (key.startsWith("trash:")) { openTrash(target === "other" ? otherWindow(state.active) : state.active); return; }
    const w = target === "other" ? otherWindow(state.active) : state.active;
    if (target === "tab") {
      newTab(w);
      dispatch({ type: "window", w, update: win => ({ ...win, tabs: win.tabs.map((t, i) => i === win.activeTab ? { ...t, lockRoot: undefined } : t) }) });
    }
    if (place.profileId) {
      const entry = state.connections.find(e => e.profile.id === place.profileId);
      if (entry) await chooseConnection(w, entry.profile);
    } else if (place.systemKey) {
      const profile = await saveSystemPlace(key, place.systemKey);
      if (profile) await chooseConnection(w, profile);
    } else if (destination) await navigatePlace(w, destination);
  }
  /**
   * Connecting needs a stored profile (password store, host-key trust, start folder), so
   * opening a desktop bookmark first saves it exactly like "Save as connection" and then
   * connects like any saved connection. The list is fetched directly: the backend's change
   * event may supersede a concurrent refresh, and the new profile must not be missed.
   */
  async function saveSystemPlace(key: string, placeKey: string): Promise<import("../ipc/types").ConnectionProfile | null> {
    const bookmark = get().places?.systemPlaces?.find(p => p.placeKey === placeKey);
    if (!bookmark) return null;
    try {
      await api.placesSaveConnection(key);
      const entries = await api.connectionsList();
      void refreshConnections();
      return entries.find(e => systemPlaceExists(bookmark, [e.profile]))?.profile ?? null;
    } catch (error) { fail(error); return null; }
  }
  function findPlace(key: string) {
    const s = get();
    return buildPlaces(s.places, s.connections, s.settings, s.home, true).flatMap(section => section.entries).find(e => e.key === key);
  }
  function readOnlyPlace(path: string) {
    if (isTrashPath(path)) return true;
    return get().places?.drives.filter(d => inside(path, d.path)).sort((a, b) => b.path.length - a.path.length)[0]?.readOnly === true;
  }
  function unmountedPlace(path: string) {
    if (parseRemotePath(path) || isArchivePath(path)) return false;
    const drive = get().places?.drives.filter(d => inside(path, d.path)).sort((a, b) => b.path.length - a.path.length)[0];
    return drive?.mounted === false && !drive.fstab?.automount;
  }
  function placeFailure(error: unknown) {
    const code = error instanceof CoreError ? error.kind : "notPermitted";
    const keys = { busy: "sidebar.errorBusy", notPermitted: "sidebar.errorPermission", permissionDenied: "sidebar.errorPermission", notFound: "sidebar.errorMissing", helperMissing: "sidebar.errorHelper", polkitDismissed: "sidebar.errorDismissed", timeout: "sidebar.errorTimeout" } as const;
    toast(tr(keys[code as keyof typeof keys] ?? "sidebar.errorPermission"), "error");
  }
  function dropOnPlace(key: string, shift: boolean) {
    const drag = get().drag; dragEnd();
    if (!drag) return;
    const place = findPlace(key);
    if (!place) return;
    if (key.startsWith("trash:")) {
      if (inArchive(drag.from) || parseRemotePath(activeTabOf(get(), drag.from).path)) { toast(tr("error.readOnly"), "error"); return; }
      const paths = targetPaths(drag.from);
      if (paths.length) dispatch({ type: "dialog", dialog: { kind: "trashConfirm", paths } });
      return;
    }
    if (place.state === "disconnected") { toast(tr("sidebar.dropConnect"), "error"); return; }
    if (place.state === "unmounted" || place.disabled || !place.path) { toast(tr("sidebar.dropMount"), "error"); return; }
    if (isArchivePath(place.path) || place.readOnly || readOnlyPlace(place.path)) { toast(tr("error.readOnly"), "error"); return; }
    const remote = parseRemotePath(place.path);
    if (remote && !get().connections.some(e => e.connected && (e.profile.id === remote.id || e.connectionId === remote.id))) { toast(tr("sidebar.dropConnect"), "error"); return; }
    openTransfer(shift ? "move" : "copy", drag.from, place.path);
  }
  /** Merkt einen Ordner als Lesezeichen (Ort); false, wenn das Limit erreicht ist oder er nicht gemerkt werden kann. */
  function pinBookmarkPath(path: string): boolean {
    const current = get().settings.bookmarks;
    if (current.length >= 200 && !current.some(b => b.path === path)) return false;
    const pinned = bookmarkOps.pin(current, path);
    if (!pinned.ok) return false;
    updateSettings(s => ({ ...s, bookmarks: pinned.bookmarks }));
    return true;
  }
  /** Pfad des einen markierten (sonst fokussierten) lokalen Ordners; sonst null. */
  function bookmarkTarget(w: WinIdx): string | null {
    const dir = activeTabOf(get(), w).path;
    if (parseRemotePath(dir) || isArchivePath(dir)) return null;
    const names = targetNames(w);
    if (names.length !== 1) return null;
    return entriesOf(w).find(e => e.name === names[0])?.kind === "dir" ? joinPath(dir, names[0]!) : null;
  }
  function pinDraggedBookmark(index: number) {
    const drag = get().drag; dragEnd();
    if (!drag) return;
    const plan = planOperation("copy", snapshot(drag.from), snapshot(otherWindow(drag.from)));
    if (!plan || plan.count !== 1 || plan.folderCount !== 1) { toast(tr("sidebar.pinFolder"), "error"); return; }
    const path = plan.sources[0]!;
    if (!pinBookmarkPath(path)) return;
    const bookmarks = get().settings.bookmarks;
    const from = bookmarks.findIndex(b => b.path === path);
    const moved = bookmarkOps.move(bookmarks, from, Math.min(index, bookmarks.length - 1));
    if (moved.ok) updateSettings(s => ({ ...s, bookmarks: moved.bookmarks }));
  }
  const pathInPlace = (path: string, root: string) => inside(isArchivePath(path) ? archiveHostFolder(path) ?? path : path, root);
  function isUnmountBusy() { const d = get().dialog; return d?.kind === "placeConfirm" && d.busy === true; }
  async function unmountPlace(key: string, confirmed = false, action: "unmount" | "eject" | "powerOff" = "unmount") {
    const place = findPlace(key);
    if (!place || !(action === "unmount" ? place.canUnmount : place.canRemove) || isUnmountBusy()) return;
    const root = place.path;
    const suppressedRoots: string[] = [];
    dispatch({ type: "dialog", dialog: { kind: "placeConfirm", action, key, busy: true } });
    try {
      // Redacted GVfs paths stay in the backend; only indices into our own tabs return.
      const tabPaths = get().windows.flatMap(w => w.tabs.map(t => t.path));
      const membershipKey = place.deviceKey && (action !== "unmount" || !key.startsWith("fstab:"))
        ? `${action === "unmount" ? "device" : "whole-device"}:${place.deviceKey}`
        : place.mountKey ?? (root ? `path:${root}` : undefined);
      const affectedPaths = membershipKey
        ? (await api.placesMountAffected(membershipKey, tabPaths)).map(i => tabPaths[i]).filter((p): p is string => !!p)
        : [];
      if (!confirmed && affectedPaths.length) {
        dispatch({ type: "dialog", dialog: { kind: "placeConfirm", action, key } }); return;
      }
      // Keep navigation behind a modal while the helper owns the mount. All current tabs
      // were checked above; no new tab can enter the mount during this operation.
      dispatch({ type: "dialog", dialog: { kind: "placeConfirm", action, key, busy: true } });
      // Suppress unplug recovery for every partition while the operation is pending.
      // Tabs move only once the requested helper sequence has succeeded.
      suppressedRoots.push(...(root ? [root] : []), ...affectedPaths);
      for (const point of suppressedRoots) unmounting.add(point);
      if (place.deviceKey && action === "eject") await api.deviceEject(place.deviceKey);
      else if (place.deviceKey && action === "powerOff") await api.devicePowerOff(place.deviceKey);
      else if (place.deviceKey && !key.startsWith("fstab:")) await api.deviceUnmount(place.deviceKey);
      else if (place.mountKey) await api.placesMountUnmount(place.mountKey);
      else if (place.profileId) await api.systemMountUnmount(place.profileId);
      else if (root) await api.fstabUnmount(root);
      const affected = (path: string) => affectedPaths.includes(path) || !!root && pathInPlace(path, root);
      for (const w of [0, 1] as const) {
        if (!get().windows[w].tabs.some(t => affected(t.path))) continue;
        reqSeq[w]++;
        dispatch({ type: "window", w, update: win => ({ ...win, tabs: win.tabs.map(t => affected(t.path) ? tabs.createTab(get().home, t.sort) : t) }) });
        void reload(w);
      }
      await refreshConnections(); await refreshPlaces();
    } catch (error) { placeFailure(error); await refreshPlaces(); }
    finally {
      for (const point of suppressedRoots) unmounting.delete(point);
      const dialog = get().dialog;
      if (dialog?.kind === "placeConfirm" && dialog.busy) dispatch({ type: "dialog", dialog: null });
    }
  }
  async function confirmPlaceAction() {
    const dialog = get().dialog;
    if (dialog?.kind !== "placeConfirm" || dialog.busy) return;
    dispatch({ type: "dialog", dialog: null });
    if (["unmount", "eject", "powerOff"].includes(dialog.action) && dialog.key) { await unmountPlace(dialog.key, true, dialog.action as "unmount" | "eject" | "powerOff"); return; }
    try {
      if (dialog.action !== "emptyTrash") return;
      const id = await api.trashEmpty();
      dispatch({ type: "jobKind", id, kind: "delete" });
      await refreshPlaces();
    } catch (error) { fail(error); }
  }
  async function savePlaceConnection(key: string) {
    const place = findPlace(key);
    if (!place) return;
    try { await api.placesSaveConnection(place.mountKey ? `mount:${place.mountKey}` : key); await refreshConnections(); }
    // Saving a desktop bookmark mounts nothing, so a mount-permission message would mislead.
    catch (error) { if (place.systemKey) fail(error); else placeFailure(error); }
  }
  function focusFileList() { document.querySelector<HTMLElement>(`[data-pane="${get().active}"] [role="grid"]`)?.focus(); }
  function focusSidebar() {
    if (get().settings.sidebarMode === "hidden") toggleSidebar();
    later(() => document.querySelector<HTMLElement>('.sidebar [role="treeitem"][tabindex="0"]')?.focus(), 0);
  }
  const focusPlaces = () => { void refreshPlaces(); };
  const visibilityPlaces = () => syncPlacesVisibility();
  function pollPlaces() { if (disposed) return; if (sidebarVisible()) void refreshPlaces(); later(pollPlaces, 60000); }

  let connectionRequest = 0;
  async function refreshConnections(): Promise<void> {
    const request = ++connectionRequest;
    try {
      const entries = await api.connectionsList();
      const remote = Object.fromEntries(Object.entries(get().remoteConnections).map(([id, value]) => [id, { ...value, connected: false }]));
      await Promise.all(entries.map(async (entry) => {
        if (!entry.connectionId) return;
        const p = entry.profile;
        let host = p.host, user = p.user, port = p.port || (p.protocol === "smb" ? 445 : p.protocol === "sftp" ? 22 : 21);
        if (p.transport === "openssh") {
          try {
            const resolved = await api.sshHostResolve(p.host);
            host = resolved.hostname; user ||= resolved.user; port = p.port || resolved.port;
          } catch { /* Profile values remain available if resolution fails. */ }
        }
        remote[entry.connectionId] = { profile: p, connected: entry.connected,
          endpoint: `${user ? `${user}@` : ""}${host.includes(":") ? `[${host}]` : host}:${port}` };
      }));
      if (!disposed && request === connectionRequest) dispatch({ type: "connections", entries, remote });
    } catch (error) { if (!disposed && request === connectionRequest) fail(error); }
  }

  const later = (callback: () => void, ms: number): void => {
    const handle = timers.set(() => {
      timerHandles.delete(handle);
      callback();
    }, ms);
    timerHandles.add(handle);
  };

  // ---------------------------------------------------------------- Meldungen

  function describeError(error: unknown, context: { path?: string; name?: string } = {}): string {
    if (error instanceof CoreError) {
      const key = errorKey(error.kind);
      const text = tr(key, {
        path: context.path ?? context.name ?? error.message,
        name: context.name ?? context.path ?? error.message,
        reason: error.message,
      });
      return key === "error.unknown" ? `${text} ${error.message}` : text;
    }
    return error instanceof Error ? error.message : tr("error.unknown");
  }

  /** Fehlermeldungen der Oberfläche landen auch in der Logdatei; ein Fehler dabei wird verschluckt, damit kein Toast-Kreis entsteht. */
  function logToFile(text: string): void {
    // eslint-disable-next-line no-control-regex
    const message = text.replace(/[\u0000-\u001f\u007f]/g, " ").slice(0, 2000);
    try {
      void Promise.resolve(api.logFrontend("error", message)).catch(() => {});
    } catch {
      /* nichts tun */
    }
  }

  function toast(text: string, kind: "info" | "error" = "info"): void {
    const id = get().nextToastId;
    dispatch({ type: "toastAdd", text, kind });
    if (kind === "error") logToFile(text);
    later(() => dispatch({ type: "toastRemove", id }), kind === "error" ? Math.max(6000, toastMs * 2) : toastMs);
  }
  const fail = (error: unknown, context?: { path?: string; name?: string }): void =>
    toast(describeError(error, context), "error");

  // ---------------------------------------------------------------- Laden

  async function reload(w: WinIdx, then?: { focusName?: string }): Promise<void> {
    const startPath = activeTabOf(get(), w).path;
    if (isTrashPath(startPath)) { dispatch({ type: "listing", w, path: startPath, entries: [] }); return; }
    const removing = () => [...unmounting].some(root => pathInPlace(startPath, root));
    if (removing()) return;
    const seq = ++reqSeq[w];
    if (then?.focusName !== undefined) dispatch({ type: "pendingFocus", w, name: then.focusName });
    let path = startPath;
    for (;;) {
      try {
        const listing = await api.list(path);
        if (seq !== reqSeq[w] || disposed || removing()) return;
        if (listing.path !== path) path = listing.path;
        if (path !== activeTabOf(get(), w).path) {
          dispatch({ type: "window", w, update: (win) => tabs.navigate(win, path), listing: { path, entries: listing.entries } });
        } else dispatch({ type: "listing", w, path, entries: listing.entries });
        if (parseRemotePath(path)) await refreshConnections();
        return;
      } catch (error) {
        if (seq !== reqSeq[w] || disposed || removing()) return;
        await refreshConnections();
        if (seq !== reqSeq[w] || disposed || removing()) return;
        // Der Ordner ist verschwunden (außen gelöscht/umbenannt): zur nächsten vorhandenen Ebene.
        if (error instanceof CoreError && error.kind === "notFound" && path !== "/" && parentPath(path) !== path) {
          path = parentPath(path);
          continue;
        }
        const current = activeTabOf(get(), w).path;
        dispatch({ type: "listingError", w, path: current, message: describeError(error, { path: current }) });
        return;
      }
    }
  }

  /** Wechselt in einen Ordner; schlägt das Lesen fehl, bleibt das Fenster unverändert und eine Meldung erscheint. */
  async function navigateTo(
    w: WinIdx,
    path: string,
    focusName?: string | null,
    onError?: (error: unknown) => void,
    history?: "back" | "forward",
  ): Promise<boolean> {
    const seq = ++reqSeq[w];
    try {
      const listing = await api.list(path);
      if (seq !== reqSeq[w] || disposed) return false;
      rememberPlace(w, activeTabOf(get(), w).path);
      rememberPlace(w, listing.path);
      dispatch({
        type: "window",
        w,
        update: (win) => history ? tabs.resolveHistory(win, history, listing.path) : tabs.navigate(win, listing.path),
        listing: { path: listing.path, entries: listing.entries },
        focusName: focusName ?? null,
      });
      if (parseRemotePath(path)) await refreshConnections();
      return true;
    } catch (error) {
      if (parseRemotePath(path)) await refreshConnections();
      if (seq === reqSeq[w]) (onError ?? ((e) => fail(e, { path })))(error);
      return false;
    }
  }

  const reloadMatching = (path: string): void => {
    const norm = normalizePath(path);
    // Auch Fenster unterhalb des geänderten Ordners laden neu: ein Vorfahre kann sie gelöscht oder umbenannt haben.
    const prefix = norm === "/" ? "/" : `${norm}/`;
    for (const w of [0, 1] as const) {
      const current = activeTabOf(get(), w).path;
      if (current === norm || current.startsWith(prefix)) void reload(w);
      // Archivfenster: ändert sich der Ordner der Archivdatei, kann das Archiv selbst sich geändert haben.
      else if (isArchivePath(current) && archiveHostFolder(current) === norm) void reload(w);
    }
  };

  // ---------------------------------------------------------------- Auswahl

  const entriesOf = (w: WinIdx): readonly Entry[] => visibleEntries(get(), w);
  const selectionOf = (w: WinIdx) => activeTabOf(get(), w).selection;

  function snapshot(w: WinIdx): PaneSnapshot {
    const tab = activeTabOf(get(), w);
    return { path: tab.path, entries: entriesOf(w), selected: tab.selection.selected, focus: tab.selection.focus };
  }

  /** Namen der Einträge, auf die eine Operation wirkt: Markierung, sonst Fokus; nie „..“. */
  function targetNames(w: WinIdx): string[] {
    const s = snapshot(w);
    const marked = s.entries.filter((e) => e.name !== ".." && s.selected.has(e.name));
    if (s.selected.size > 0) return marked.map((e) => e.name);
    const focused = s.entries[s.focus];
    return focused && focused.name !== ".." ? [focused.name] : [];
  }
  const targetPaths = (w: WinIdx): string[] => {
    const dir = activeTabOf(get(), w).path;
    return targetNames(w).map((name) => joinPath(dir, name));
  };

  /** Liegt der aktuelle Ordner des Fensters in einem Archiv? Dann ist es schreibgeschützt. */
  const inArchive = (w: WinIdx): boolean => isArchivePath(activeTabOf(get(), w).path) || isTrashPath(activeTabOf(get(), w).path);

  /** Meldet „Archiv ist schreibgeschützt“ und liefert true, wenn das Fenster in einem Archiv steht. */
  function refuseInArchive(w: WinIdx): boolean {
    if (!inArchive(w)) return false;
    toast(tr("error.readOnly"), "error");
    return true;
  }

  const select = (w: WinIdx, update: (s: sel.SelectionState, entries: readonly Entry[]) => sel.SelectionState): void =>
    dispatch({ type: "selection", w, update });

  function move(w: WinIdx, how: sel.FocusMove, extend = false): void {
    select(w, (s, entries) => sel.moveFocus(s, entries, how, pageSize[w], extend));
  }

  // ---------------------------------------------------------------- Öffnen

  async function openEntry(w: WinIdx, entry: Entry | undefined): Promise<void> {
    if (!entry) return;
    const dir = activeTabOf(get(), w).path;
    if (entry.name === "..") return goUp(w);
    const path = joinPath(dir, entry.name);
    if (entry.kind === "dir") {
      await navigateTo(w, path);
      return;
    }
    if (entry.kind === "file" && isArchiveName(entry.name)) {
      await openArchive(w, path, entry.name);
      return;
    }
    if (entry.kind === "symlink") {
      // Ob der Link auf einen Ordner zeigt, weiß erst der Kern: erst als Ordner versuchen, sonst öffnen.
      if (!entry.brokenLink && (await navigateTo(w, path))) return;
      if (entry.brokenLink) return toast(tr("error.openFailed", { name: entry.name }), "error");
    }
    // Remote files are copied to a local temp file by the backend before opening; that fetch can
    // take a moment, so say what is happening.
    if (parseRemotePath(path)) toast(tr("info.openingRemote", { name: entry.name }), "info");
    try {
      await api.openExternal(path);
    } catch (error) {
      toast(`${tr("error.openFailed", { name: entry.name })} ${describeError(error, { path })}`, "error");
    }
  }

  /** Öffnet eine Archivdatei als Ordner im Fenster; bei einem Archivfehler erscheint ein Hinweis mit der Option „Extern öffnen“. */
  async function openArchive(w: WinIdx, filePath: string, name: string): Promise<void> {
    await navigateTo(w, archiveRootOf(filePath), null, (error) => {
      if (error instanceof CoreError && error.kind === "archive") {
        dispatch({
          type: "dialog",
          dialog: { kind: "archiveError", path: filePath, message: describeError(error, { path: filePath, name }), external: !isArchivePath(filePath) },
        });
      } else fail(error, { path: filePath, name });
    });
  }

  // Remember places independently for each panel, including visits made outside its menu.
  const placePaths: [Map<string, string>, Map<string, string>] = [new Map(), new Map()];
  function rememberPlace(w: WinIdx, path: string) {
    const remote = parseRemotePath(path);
    const id = remote && get().remoteConnections[remote.id]?.profile.id;
    if (id) placePaths[w].set(id, path);
  }

  // A connect dialog for this profile is already open (e.g. double-click): replacing it would orphan the
  // running connect, whose onConnected guard then fails and leaves the pane unrefreshed.
  function connectPending(profileId: string): boolean {
    const dialog = get().dialog;
    return dialog?.kind === "connections" && dialog.entry?.action === "connect" && dialog.entry.profile?.id === profileId;
  }

  async function navigatePlace(w: WinIdx, saved: string, history?: "back" | "forward"): Promise<boolean> {
    const target = currentRemotePath(saved, get().remoteConnections);
    const remote = parseRemotePath(target);
    const connection = remote && get().remoteConnections[remote.id];
    if (connection && !connection.connected) {
      if (connectPending(connection.profile.id)) return false;
      const sourceIndex = get().windows[w].activeTab;
      const dialog: Dialog = { kind: "connections", entry: {
        action: "connect", profile: connection.profile,
        onConnected: async root => {
          if (get().dialog !== dialog || get().windows[w].activeTab !== sourceIndex) return false;
          const live = parseRemotePath(root);
          return navigateTo(w, live ? live.root + remote.inner : root, undefined, undefined, history);
        },
      } };
      dispatch({ type: "dialog", dialog });
      return false;
    }
    return navigateTo(w, target, undefined, undefined, history);
  }

  async function chooseConnection(w: WinIdx, profile: import("../ipc/types").ConnectionProfile): Promise<boolean> {
    rememberPlace(w, activeTabOf(get(), w).path);
    const saved = placePaths[w].get(profile.id);
    const entry = get().connections.find(e => e.profile.id === profile.id);
    if (entry?.connected && entry.root) return navigatePlace(w, saved ?? entry.root);
    if (connectPending(profile.id)) return false;
    const dialog: Dialog = { kind: "connections", entry: {
      action: "connect", profile,
      onConnected: async root => {
        if (get().dialog !== dialog) return false;
        const live = parseRemotePath(root);
        return navigateTo(w, saved && live ? live.root + (parseRemotePath(saved)?.inner ?? "/") : root);
      },
    } };
    dispatch({ type: "dialog", dialog });
    return false;
  }

  async function goHistory(w: WinIdx, direction: "back" | "forward"): Promise<boolean> {
    const path = activeTabOf(get(), w)[direction].at(-1);
    return path === undefined ? false : navigatePlace(w, path, direction);
  }
  const goBack = (w: WinIdx) => goHistory(w, "back");
  const goForward = (w: WinIdx) => goHistory(w, "forward");

  async function goUp(w: WinIdx): Promise<void> {
    const path = activeTabOf(get(), w).path;
    if (path === "/" || isTrashPath(path)) return;
    await navigateTo(w, parentPath(path), baseName(path));
  }

  // ---------------------------------------------------------------- Operationen

  function openTransfer(kind: OperationKind, from: WinIdx = get().active, destination = activeTabOf(get(), otherWindow(from)).path): void {
    // Archive sind schreibgeschützt: nicht hinein kopieren, nicht daraus verschieben (das würde die Quelle löschen).
    if (isArchivePath(destination) || readOnlyPlace(destination) || (kind === "move" && inArchive(from))) {
      toast(tr("error.readOnly"), "error");
      return;
    }
    if (unmountedPlace(destination)) { toast(tr("sidebar.dropMount"), "error"); return; }
    const plan = planOperation(kind, snapshot(from), { ...snapshot(otherWindow(from)), path: destination });
    if (plan) dispatch({ type: "dialog", dialog: { kind: "transfer", plan, from } });
  }

  /** Opens the copy dialog for absolute paths dropped from outside onto `destination`: copy only, the native drop event carries no modifier keys. shortcut: Shift+drop = move is not available here. */
  function dropPaths(sources: string[], destination: string): void {
    if (isArchivePath(destination) || readOnlyPlace(destination)) { toast(tr("error.readOnly"), "error"); return; }
    if (unmountedPlace(destination)) { toast(tr("sidebar.dropMount"), "error"); return; }
    const plan = planPaths("copy", sources, destination);
    if (plan) dispatch({ type: "dialog", dialog: { kind: "transfer", plan, from: get().active } });
  }

  /** Pane and (directory) row under a window position, or null outside both panes. */
  function nativeTarget(x: number, y: number): { w: WinIdx; folder: { name: string; path: string } | null } | null {
    const el = document.elementFromPoint(x, y);
    const pane = el?.closest<HTMLElement>("[data-pane]");
    if (!el || !pane) return null;
    const w: WinIdx = pane.dataset.pane === "1" ? 1 : 0;
    const match = /^pane[01]-row-(\d+)$/.exec(el.closest<HTMLElement>('[role="row"]')?.id ?? "");
    const entry = match ? entriesOf(w)[Number(match[1])] : undefined;
    const folder = entry && entry.kind === "dir" && entry.name !== ".." ? { name: entry.name, path: joinPath(activeTabOf(get(), w).path, entry.name) } : null;
    return { w, folder };
  }
  function nativeDropEvent(event: NativeDropEvent): void {
    const drag = get().drag;
    if (drag && !drag.external) return; // internal drags use the HTML5 path
    if (event.type === "leave") { if (drag) dispatch({ type: "drag", drag: null }); return; }
    const target = nativeTarget(event.x, event.y);
    if (event.type === "over") {
      if (!target) { if (drag) dispatch({ type: "drag", drag: null }); return; }
      const rx = Math.round(event.x / 6) * 6, ry = Math.round(event.y / 6) * 6;
      dispatch({ type: "drag", drag: { from: target.w, over: target.folder ? null : target.w, folder: target.folder ? { w: target.w, ...target.folder } : null, mode: "copy", x: rx, y: ry, external: true } });
      return;
    }
    dispatch({ type: "drag", drag: null });
    if (target) dropPaths(event.paths, target.folder?.path ?? activeTabOf(get(), target.w).path);
  }
  async function confirmTransfer(plan: OperationPlan, destination: string): Promise<void> {
    const final = withDestination(plan, destination, parentPath(plan.sources[0] ?? "/"));
    if (!final || final.sameFolder) return;
    if (unmountedPlace(final.destination)) { toast(tr("sidebar.dropMount"), "error"); return; }
    if (isArchivePath(final.destination) || readOnlyPlace(final.destination)) { toast(tr("error.readOnly"), "error"); return; } // nie in den eigenen Ordner
    dispatch({ type: "dialog", dialog: null });
    try {
      const id = await (final.kind === "copy"
        ? api.copy(final.sources, final.destination)
        : api.move(final.sources, final.destination));
      dispatch({ type: "jobKind", id, kind: final.extract ? "extract" : final.kind, destination: final.destination });
    } catch (error) {
      fail(error, { path: final.destination });
    }
  }

  async function answerConflict(conflict: ConflictInfo, resolution: Resolution, applyToAll: boolean): Promise<void> {
    dispatch({ type: "conflictDone", jobId: conflict.jobId });
    try {
      await api.answerConflict(conflict.jobId, { resolution, applyToAll });
    } catch (error) {
      fail(error, { path: conflict.destinationPath });
    }
  }

  async function trashSelection(): Promise<void> {
    const w = get().active;
    if (refuseInArchive(w)) return;
    const paths = targetPaths(w);
    if (paths.length === 0) return;
    // Immer nachfragen (nicht abschaltbar); trashPaths läuft erst nach confirmTrash.
    dispatch({ type: "dialog", dialog: { kind: "trashConfirm", paths } });
  }

  async function trashPaths(paths: string[]): Promise<void> {
    try {
      const id = await api.trash(paths);
      dispatch({ type: "jobKind", id, kind: "trash" });
      toast(tr("toast.trashed", { count: paths.length, name: baseName(paths[0] ?? "") }));
    } catch (error) {
      if (error instanceof CoreError && NO_FALLBACK_KINDS.has(error.kind)) return fail(error, { path: paths[0] });
      // Papierkorb nicht möglich: nur mit ausdrücklicher Rückfrage endgültig löschen.
      await planPermanentDelete(paths, "trashFailed");
    }
  }

  async function planPermanentDelete(paths: string[], reason: "permanent" | "trashFailed"): Promise<void> {
    if (paths.length === 0) return;
    try {
      const plan = await api.planDelete(paths);
      dispatch({ type: "dialog", dialog: { kind: "delete", model: deleteDialogModel(plan), plan, reason } });
    } catch (error) {
      fail(error, { path: paths[0] });
    }
  }

  /** Bestätigt das endgültige Löschen; ein falsches oder leeres Token bewirkt nichts (nichts wird gesendet). */
  async function confirmDelete(token: string): Promise<boolean> {
    const dialog = get().dialog;
    if (dialog?.kind !== "delete") return false;
    const valid = confirmDeleteToken(dialog.model, token);
    if (valid === null) return false;
    dispatch({ type: "dialog", dialog: null });
    try {
      const id = await api.deletePermanent(dialog.plan, valid);
      dispatch({ type: "jobKind", id, kind: "delete" });
    } catch (error) {
      fail(error);
    }
    return true;
  }

  let historyQueue = Promise.resolve();
  let historyInitialized = false;
  let unsubscribeUndo: (() => void) | null = null;
  let unsubscribeNativeDrop: (() => void) | null = null;
  function refreshHistory(): Promise<void> {
    historyQueue = historyQueue.then(async () => {
      const entries = await api.undoList();
      if (disposed) return;
      const history = [...entries].sort((a, b) => b.time - a.time || b.id - a.id);
      const known = new Set(get().history.map((entry) => entry.id));
      const fresh = historyInitialized ? history.find((entry) => !known.has(entry.id) && !entry.undone) : undefined;
      historyInitialized = true;
      dispatch({ type: "undoState", patch: { history, ...(fresh ? { undoToast: fresh } : {}) } });
    }).catch(() => { if (!disposed) toast(tr("undoError.io"), "error"); });
    return historyQueue;
  }

  async function undo(id?: number): Promise<void> {
    if (get().undoBusy) return;
    const entry = id === undefined ? get().history.find((item) => !item.undone) : get().history.find((item) => item.id === id);
    if (!entry) return toast(tr("toast.nothingToUndo"));
    if (entry.undone) return;
    dispatch({ type: "undoState", patch: { undoBusy: true, undoJob: null } });
    try {
      const result = await api.undoRun(entry.id);
      if (disposed) return;
      dispatch({ type: "undoState", patch: { undoResults: { ...get().undoResults, [entry.id]: result } } });
      const summary = formatJobPresentation({ undo: result }, tr).counter;
      toast([summary, result.cancelled ? tr("undo.cancelled") : "", ...[...new Set(result.errors)].map((error) => tr(`undoError.${error}`))].filter(Boolean).join(". "), result.errors.length ? "error" : "info");
      await refreshHistory();
      if (result.done) await Promise.all([reload(0), reload(1)]);
    } catch {
      if (!disposed) {
        dispatch({ type: "undoState", patch: { undoResults: { ...get().undoResults, [entry.id]: { jobId: 0, ok: false, done: 0, remaining: 0, cancelled: false, errors: ["io"] } } } });
        toast(tr("undoError.io"), "error");
      }
    } finally {
      if (!disposed) dispatch({ type: "undoState", patch: { undoBusy: false, undoJob: null } });
    }
  }

  function openRename(): void {
    const w = get().active;
    if (refuseInArchive(w)) return;
    const entry = entriesOf(w)[selectionOf(w).focus];
    if (!entry || entry.name === "..") return;
    const raw = get().listings[w].entries;
    dispatch({
      type: "dialog",
      dialog: { kind: "rename", w, dir: activeTabOf(get(), w).path, name: entry.name, existing: raw.map((e) => e.name) },
    });
  }

  /** Liefert den Fehlertext zum Anzeigen im Dialog oder null bei Erfolg. */
  async function submitRename(name: string): Promise<string | null> {
    const dialog = get().dialog;
    if (dialog?.kind !== "rename") return null;
    const invalid = nameError(name, dialog.existing, dialog.name);
    if (invalid) return tr(invalid);
    if (name === dialog.name) {
      dispatch({ type: "dialog", dialog: null });
      return null;
    }
    const oldPath = joinPath(dialog.dir, dialog.name);
    try {
      await api.rename(oldPath, name);
      toast(tr("toast.renamed", { oldName: dialog.name, newName: name }));
      dispatch({ type: "dialog", dialog: null });
      void reload(dialog.w, { focusName: name });
      return null;
    } catch (error) {
      return describeError(error, { name });
    }
  }

  // ---------------------------------------------------------------- Mehrfach-Umbenennen

  const jobWaiters = new Set<() => void>();
  /** Wartet, bis der Auftrag beendet ist (Ereignis des Kerns); null, wenn die Oberfläche vorher beendet wird. */
  function waitForJob(id: number): Promise<JobStatus | null> {
    const finished = (): JobStatus | undefined => get().jobs.jobs.find((job) => job.id === id && isFinished(job.state));
    const now = finished();
    if (now) return Promise.resolve(now);
    return new Promise((resolve) => {
      const done = (value: JobStatus | null): void => {
        unsubscribe();
        jobWaiters.delete(cancel);
        resolve(value);
      };
      const unsubscribe = store.subscribe(() => {
        const job = finished();
        if (job) done(job);
      });
      const cancel = (): void => done(null);
      jobWaiters.add(cancel);
    });
  }

  /** Einträge, die ein beendeter Umbenennen-Auftrag umbenannt hat (geplant minus `failed`; die Pfade dort sind alte Pfade). */
  function succeededItems(dir: string, items: readonly RenameItem[], status: JobStatus): RenameItem[] {
    const failed = new Set(status.failed.map((f) => f.path));
    return items.filter((item) => !failed.has(joinPath(dir, item.oldName)));
  }

  /** Strg+M: Ziel sind Markierung oder Fokuseintrag des aktiven Fensters (nie „..“); sonst passiert nichts. */
  function canMultiRename(w: WinIdx = get().active): boolean {
    return targetNames(w).length > 0 && get().listings[w].error === null && !inArchive(w);
  }

  function openMultiRename(w: WinIdx = get().active): void {
    if (refuseInArchive(w)) return;
    if (!canMultiRename(w)) return;
    const names = new Set(targetNames(w));
    const sources = entriesOf(w)
      .filter((entry) => names.has(entry.name))
      .map((entry) => ({ name: entry.name, kind: entry.kind, modified: entry.modified, tags: emptyTags }));
    dispatch({
      type: "dialog",
      dialog: { kind: "multiRename", w, dir: activeTabOf(get(), w).path, sources, existing: get().listings[w].entries.map((e) => e.name) },
    });
  }

  /** Führt den Auftrag aus; liefert null und eine Meldung im Dialog, wenn der Kern den Lauf ablehnt. */
  async function submitMultiRename(items: RenameItem[]): Promise<string | null> {
    const dialog = get().dialog;
    if (dialog?.kind !== "multiRename" || items.length === 0) return null;
    const { dir } = dialog;
    let id: number;
    try {
      id = await api.renameMany(dir, items);
    } catch (error) {
      return describeError(error, { path: dir });
    }
    dispatch({ type: "jobKind", id, kind: "rename" });
    dispatch({ type: "dialog", dialog: null });
    const status = await waitForJob(id);
    if (!status) return null;
    const done = succeededItems(dir, items, status);
    if (status.failed.length === 0) toast(tr("toast.multiRenamed", { count: done.length }));
    else if (done.length === 0) toast(tr("toast.multiRenameFailed", { reason: status.failed[0]?.message ?? "" }), "error");
    else toast(tr("toast.multiRenamePartial", { done: done.length, failed: status.failed.length }), "error");
    reloadMatching(dir);
    return null;
  }

  function savePresetNamed(name: string, rules: MultiRenameRules): void {
    dispatch({ type: "renamePresets", presets: savePreset(get().renamePresets, name, rules) });
  }
  function deletePresetNamed(name: string): void {
    dispatch({ type: "renamePresets", presets: deletePreset(get().renamePresets, name) });
  }
  /** Übernimmt Voreinstellungen von außen (z. B. gespeicherte Daten) erst nach der Prüfung. */
  function setRenamePresets(raw: unknown): void {
    dispatch({ type: "renamePresets", presets: sanitizePresets(raw) });
  }

  function openMakeDir(): void {
    const w = get().active;
    if (refuseInArchive(w)) return;
    dispatch({
      type: "dialog",
      dialog: { kind: "makeDir", w, parent: activeTabOf(get(), w).path, existing: get().listings[w].entries.map((e) => e.name) },
    });
  }

  async function submitMakeDir(name: string): Promise<string | null> {
    const dialog = get().dialog;
    if (dialog?.kind !== "makeDir") return null;
    const invalid = nameError(name, dialog.existing);
    if (invalid) return tr(invalid);
    try {
      await api.makeDir(dialog.parent, name);
      toast(tr("toast.folderCreated", { name }));
      dispatch({ type: "dialog", dialog: null });
      void reload(dialog.w, { focusName: name });
      return null;
    } catch (error) {
      return describeError(error, { name });
    }
  }

  // ---------------------------------------------------------------- Packen

  /** Packen ist möglich, wenn etwas gewählt ist, das Fenster lesbar ist und das Ziel (Gegenfenster) kein Archiv ist. */
  function canPack(w: WinIdx = get().active): boolean {
    return targetNames(w).length > 0 && get().listings[w].error === null && !inArchive(otherWindow(w));
  }

  /** Strg+Umschalt+P / „Packen …“: Ziel ist immer der Ordner des Gegenfensters; ist das ein Archiv, gibt es den Dialog nicht. */
  function openPack(w: WinIdx = get().active): void {
    if (targetNames(w).length === 0) return void toast(tr("error.noSelection"));
    const other = otherWindow(w);
    if (inArchive(other)) return void toast(tr("pack.targetIsArchive"), "error");
    const names = targetNames(w);
    const dir = activeTabOf(get(), w).path;
    const first = entriesOf(w).find((entry) => entry.name === names[0]);
    const destination = activeTabOf(get(), other).path;
    const targetListing = get().listings[other];
    dispatch({
      type: "dialog",
      dialog: {
        kind: "pack",
        w,
        sources: names.map((name) => joinPath(dir, name)),
        sourceNames: names,
        destination,
        existing: targetListing.path === destination ? targetListing.entries.map((entry) => entry.name) : [],
        suggestion: suggestPackBase(first?.name ?? tr("pack.defaultName"), first?.kind === "dir"),
      },
    });
  }

  /** Startet den Packauftrag; liefert einen Fehlertext für den Dialog (z. B. der Name ist vergeben) oder null. */
  async function submitPack(format: PackFormat, name: string): Promise<string | null> {
    const dialog = get().dialog;
    if (dialog?.kind !== "pack") return null;
    const invalid = nameError(name, dialog.existing);
    if (invalid) return tr(invalid);
    try {
      const id = await api.packArchive([...dialog.sources], dialog.destination, name, format);
      dispatch({ type: "jobKind", id, kind: "pack" });
      dispatch({ type: "dialog", dialog: null });
      return null;
    } catch (error) {
      return describeError(error, { name });
    }
  }

  function openProperties(w: WinIdx = get().active): void {
    const entry = entriesOf(w)[selectionOf(w).focus];
    if (!entry || entry.name === "..") return;
    dispatch({ type: "dialog", dialog: { kind: "properties", entry, path: joinPath(activeTabOf(get(), w).path, entry.name) } });
  }

  async function editFocused(): Promise<void> {
    const w = get().active;
    if (isArchivePath(activeTabOf(get(), w).path)) return;
    const entry = entriesOf(w)[selectionOf(w).focus];
    if (!entry || entry.name === "..") return;
    const path = joinPath(activeTabOf(get(), w).path, entry.name);
    try {
      await api.openExternal(path);
    } catch (error) {
      fail(error, { path, name: entry.name });
    }
  }

  async function copyPath(w: WinIdx = get().active): Promise<void> {
    const paths = targetPaths(w);
    const text = paths.length > 0 ? paths.join("\n") : activeTabOf(get(), w).path;
    try {
      await navigator.clipboard.writeText(text);
      toast(tr("toast.pathCopied"));
    } catch (error) {
      fail(error);
    }
  }

  // ---------------------------------------------------------------- Tabs, Sortierung, Filter

  function windowCommand(w: WinIdx, update: (win: tabs.WindowState) => tabs.WindowState, reloadAfter: boolean): void {
    dispatch({ type: "window", w, update });
    if (reloadAfter) void reload(w);
  }
  const newTab = (w: WinIdx = get().active): void => windowCommand(w, tabs.newTab, false);
  const closeTab = (w: WinIdx = get().active, index?: number): void =>
    windowCommand(w, (win) => tabs.closeTab(win, index), true);
  const closeOtherTabs = (w: WinIdx, keep: number): void =>
    windowCommand(w, (win) => tabs.closeOthers(win, keep), true);
  const toggleTabLock = (w: WinIdx, index: number): void =>
    windowCommand(w, (win) => tabs.toggleLock(win, index), false);
  const openTabMenu = (w: WinIdx, index: number, x: number, y: number): void => {
    dispatch({ type: "active", w });
    dispatch({ type: "menu", menu: { w, x, y, tab: index } });
  };
  const selectTab = (w: WinIdx, index: number): void => {
    dispatch({ type: "active", w });
    windowCommand(w, (win) => tabs.selectTab(win, index), true);
  };

  function sortBy(w: WinIdx, key: SortKey): void {
    dispatch({
      type: "window",
      w,
      update: (win) => tabs.setSort(win, formatSort(toggleSort(visibleSort(tabs.activeTab(win).sort, get().settings), key))),
    });
  }
  const setFilter = (w: WinIdx, text: string): void => dispatch({ type: "window", w, update: (win) => tabs.setFilter(win, text) });

  // ---------------------------------------------------------------- Lesezeichen und Einstellungen

  function updateSettings(update: (settings: Settings) => Settings): void {
    const before = get().settings.sidebarDiscovery ?? false;
    dispatch({ type: "settings", update });
    if (before !== (get().settings.sidebarDiscovery ?? false)) {
      discoveryEpoch++;
      discoveredServices = [];
      const snapshot = get().places;
      if (snapshot) dispatch({ type: "places", snapshot: { ...snapshot, discoveredServices }, announcement: "" });
      // Discovery changes bypass the ordinary autosave debounce, including in the settings window.
      void saveNow().then(() => refreshDiscovery());
    }
  }

  function togglePin(w: WinIdx = get().active): void {
    const path = activeTabOf(get(), w).path;
    const bookmarks = get().settings.bookmarks;
    if (bookmarkOps.isPinned(bookmarks, path)) return updateSettings((s) => ({ ...s, bookmarks: bookmarkOps.unpin(s.bookmarks, path) }));
    const color = accentPresets[bookmarks.length % accentPresets.length];
    const result = bookmarkOps.pin(bookmarks, path, path === "/" ? "/" : undefined, color);
    if (result.ok) updateSettings((s) => ({ ...s, bookmarks: result.bookmarks }));
  }

  async function openBookmark(path: string): Promise<void> {
    await navigateTo(get().active, path);
  }

  function openBookmarkNumber(n: number): void {
    const bookmark = bookmarkOps.byShortcut(get().settings.bookmarks, n);
    if (bookmark) void openBookmark(bookmark.path);
  }

  /** Liefert einen Fehlerschlüssel oder null. */
  function editBookmark(path: string, change: { name?: string; color?: string }): TranslationKey | null {
    let bookmarks = get().settings.bookmarks;
    for (const step of [
      change.name === undefined ? null : () => bookmarkOps.rename(bookmarks, path, change.name ?? ""),
      change.color === undefined ? null : () => bookmarkOps.setColor(bookmarks, path, change.color ?? ""),
    ]) {
      if (!step) continue;
      const result = step();
      if (!result.ok) return result.error === "invalidColor" ? "bookmarks.invalidColor" : "error.name.empty";
      bookmarks = result.bookmarks;
    }
    updateSettings((s) => ({ ...s, bookmarks }));
    return null;
  }
  // ---------------------------------------------------------------- Papierkorb-Ansicht

  /** Tells open trash views to reload their list. */
  const trashChanged = (): void => { document.dispatchEvent(new CustomEvent("dualfile:trash-changed")); };
  /** Opens (or selects) the trash view tab in window `w`. */
  function openTrash(w: WinIdx): void {
    dispatch({ type: "active", w });
    const index = get().windows[w].tabs.findIndex(t => isTrashPath(t.path));
    if (index >= 0) return selectTab(w, index);
    dispatch({
      type: "window", w, listing: { path: TRASH_PATH, entries: [] },
      update: win => { const opened = tabs.newTab(win); return { ...opened, tabs: opened.tabs.map((t, i) => i === opened.activeTab ? tabs.createTab(TRASH_PATH) : t) }; },
    });
  }
  async function openSystemTrash(): Promise<void> {
    try { await api.trashOpenSystem(); } catch { toast(tr("sidebar.trashUnavailable"), "error"); }
  }
  async function trashRestore(items: readonly TrashEntry[]): Promise<void> {
    if (!items.length) return;
    try {
      await api.trashRestore(items.map(item => item.id));
      toast(items.length === 1 ? tr("trash.restoredOne", { name: items[0]!.name, where: items[0]!.originalParent }) : tr("trash.restoredMany", { count: items.length }));
    } catch (error) {
      if (error instanceof CoreError && error.kind === "exists") toast(tr("trash.restoreExists", { count: items.length, name: items[0]!.name }), "error");
      else fail(error);
    }
    trashChanged();
    void refreshPlaces();
  }
  function askTrashPurge(items: readonly TrashEntry[]): void {
    if (items.length) dispatch({ type: "dialog", dialog: { kind: "trashPurge", items: items.map(({ id, name }) => ({ id, name })) } });
  }
  async function confirmTrashPurge(): Promise<void> {
    const dialog = get().dialog;
    if (dialog?.kind !== "trashPurge") return;
    dispatch({ type: "dialog", dialog: null });
    try {
      toast(tr("trash.purged", { count: await api.trashPurge(dialog.items.map(item => item.id)) }));
    } catch (error) { fail(error); }
    trashChanged();
    void refreshPlaces();
  }
  /** Shows the original folder of a trashed item in the other pane. */
  const openTrashOriginal = (w: WinIdx, item: TrashEntry): void => void navigateTo(otherWindow(w), item.originalParent, item.name);
  const unpinBookmark = (path: string): void =>
    updateSettings((s) => ({ ...s, bookmarks: bookmarkOps.unpin(s.bookmarks, path) }));
  function moveBookmark(from: number, to: number): void {
    const result = bookmarkOps.move(get().settings.bookmarks, from, to);
    if (result.ok) updateSettings((s) => ({ ...s, bookmarks: result.bookmarks }));
  }

  // ---------------------------------------------------------------- Lister und Schnellansicht

  /** F3: öffnet die fokussierte Datei des aktiven Fensters im Lister; Ordner bekommen einen Hinweis, „..“ nichts. */
  function openLister(): void {
    const w = get().active;
    const entry = focusedEntry(get(), w);
    if (!entry || entry.name === "..") return;
    if (entry.kind === "dir") return toast(tr("toast.viewFolder"));
    if (entry.kind === "symlink" && entry.brokenLink) return toast(tr("error.openFailed", { name: entry.name }), "error");
    dispatch({ type: "lister", lister: { w } });
  }

  const closeLister = (): void => dispatch({ type: "lister", lister: null });

  /** Pfeil links/rechts im Lister: nächste Datei der Liste (Ordner und „..“ werden übersprungen), der Fokus der Liste folgt. */
  function listerStep(delta: 1 | -1): void {
    const lister = get().lister;
    if (!lister) return;
    const entries = entriesOf(lister.w);
    const focus = selectionOf(lister.w).focus;
    for (let index = focus + delta; index >= 0 && index < entries.length; index += delta) {
      const entry = entries[index]!;
      if (entry.kind !== "dir" && entry.name !== "..") {
        select(lister.w, (s, list) => sel.moveFocus(s, list, index - s.focus));
        return;
      }
    }
  }

  const toggleTerminal = (): void => dispatch({ type: "terminal", on: !get().terminal });
  const toggleQuickView = (): void => dispatch({ type: "quickView", on: !get().quickView });

  async function openExternalPath(path: string): Promise<void> {
    try {
      await api.openExternal(path);
    } catch (error) {
      toast(`${tr("error.openFailed", { name: baseName(path) })} ${describeError(error, { path })}`, "error");
    }
  }

  // ---------------------------------------------------------------- Befehle

  function command(cmd: Command): void {
    const state = get();
    const w = state.active;
    typeaheadState = createTypeahead();
    switch (cmd) {
      case "toggleSidebar": return toggleSidebar();
      case "toggleSidebarIcons": return toggleSidebarIcons();
      case "focusSidebar": return focusSidebar();
      case "switchPane":
        return dispatch({ type: "active", w: otherWindow(w) });
      case "moveUp":
        return move(w, -1);
      case "moveDown":
        return move(w, 1);
      case "home":
        return move(w, "home");
      case "end":
        return move(w, "end");
      case "pageUp":
        return move(w, "pageUp");
      case "pageDown":
        return move(w, "pageDown");
      case "extendUp":
        return move(w, -1, true);
      case "extendDown":
        return move(w, 1, true);
      case "extendHome":
        return move(w, "home", true);
      case "extendEnd":
        return move(w, "end", true);
      case "extendPageUp":
        return move(w, "pageUp", true);
      case "extendPageDown":
        return move(w, "pageDown", true);
      case "open":
        return void openEntry(w, entriesOf(w)[selectionOf(w).focus]);
      case "goBack": return void goBack(w);
      case "goForward": return void goForward(w);
      case "goUp":
        return void goUp(w);
      case "rename":
        return openRename();
      case "multiRename":
        return openMultiRename();
      case "pack":
        return openPack();
      case "compareDirs":
        return compareFeature.openCompare();
      case "compareFiles":
        return compareFeature.openDiffFromPanes();
      case "view":
        return openLister();
      case "terminal":
        return toggleTerminal();
      case "quickView":
        return toggleQuickView();
      case "edit":
        return void editFocused();
      case "copy":
        return openTransfer("copy");
      case "move":
        return openTransfer("move");
      case "makeDir":
        return openMakeDir();
      case "trash":
        return void trashSelection();
      case "deletePermanent": {
        if (refuseInArchive(w)) return;
        const paths = targetPaths(w);
        if (paths.length === 0) return toast(tr("ui.dialog.delete.nothing"));
        return void planPermanentDelete(paths, "permanent");
      }
      case "toggleSelection":
        return select(w, sel.toggleAndAdvance);
      case "selectByPattern":
        return dispatch({ type: "dialog", dialog: { kind: "pattern", mode: "select", w } });
      case "deselectByPattern":
        return dispatch({ type: "dialog", dialog: { kind: "pattern", mode: "deselect", w } });
      case "invertSelection":
        return select(w, sel.invert);
      case "selectAll":
        return select(w, sel.selectAll);
      case "pathInput":
        return dispatch({ type: "pathEdit", w });
      case "refresh":
        return void reload(w);
      case "toggleHidden":
        return dispatch({ type: "toggleHidden" });
      case "swapPanes":
        return dispatch({ type: "swap" });
      case "newTab":
        return newTab(w);
      case "closeTab":
        return closeTab(w);
      case "nextTab":
        return windowCommand(w, tabs.nextTab, true);
      case "prevTab":
        return windowCommand(w, tabs.prevTab, true);
      case "undo":
        return void undo();
      case "escape": {
        if (state.pathEditing !== null) return dispatch({ type: "pathEdit", w: null });
        if (activeTabOf(state, w).filter !== "") return setFilter(w, "");
        return select(w, sel.clear);
      }
      default:
        return openBookmarkNumber(bookmarkNumber(cmd));
    }
  }

  /** Tippen zum Suchen: springt zum Eintrag, dessen Name so beginnt. */
  function typeahead(char: string): void {
    const w = get().active;
    const entries = entriesOf(w);
    const result = typeaheadStep(typeaheadState, char, clock(), entries, selectionOf(w).focus);
    typeaheadState = result.state;
    if (result.matched) select(w, (s, e) => sel.moveFocus(s, e, result.focus - s.focus));
  }

  function applyPattern(mode: "select" | "deselect", w: WinIdx, pattern: string): void {
    select(w, (s, entries) => (mode === "select" ? sel.selectByPattern(s, entries, pattern) : sel.deselectByPattern(s, entries, pattern)));
    dispatch({ type: "dialog", dialog: null });
  }

  // ---------------------------------------------------------------- Zeilen (Maus)

  function clickRow(w: WinIdx, index: number, mods: { ctrl: boolean; shift: boolean }): void {
    dispatch({ type: "active", w });
    typeaheadState = createTypeahead();
    select(w, (s, entries) => (mods.shift ? sel.shiftClick(s, entries, index) : mods.ctrl ? sel.ctrlClick(s, entries, index) : sel.click(s, entries, index)));
  }
  function activateRow(w: WinIdx, index: number): void {
    dispatch({ type: "active", w });
    select(w, (s, entries) => sel.click(s, entries, index));
    void openEntry(w, entriesOf(w)[index]);
  }
  function openContextMenu(w: WinIdx, index: number | null, x: number, y: number): void {
    dispatch({ type: "active", w });
    if (index !== null) {
      const entry = entriesOf(w)[index];
      const already = entry !== undefined && selectionOf(w).selected.has(entry.name);
      // Rechtsklick auf eine markierte Zeile behält die Markierung, sonst wird nur diese Zeile markiert.
      if (already) select(w, (s, entries) => ({ ...s, focus: index, focusedName: entries[index]?.name ?? null }));
      else select(w, (s, entries) => sel.click(s, entries, index));
    }
    dispatch({ type: "menu", menu: { w, x, y, background: index === null } });
  }

  // ---------------------------------------------------------------- Ziehen

  function dragStart(w: WinIdx, index: number): void {
    const entry = entriesOf(w)[index];
    if (!entry || entry.name === "..") return;
    dispatch({ type: "active", w });
    if (!selectionOf(w).selected.has(entry.name)) select(w, (s, entries) => sel.click(s, entries, index));
    dispatch({ type: "drag", drag: { from: w, over: null, mode: "copy", x: 0, y: 0 } });
  }
  function dragOver(w: WinIdx, shift: boolean, x: number, y: number): void {
    const drag = get().drag;
    if (!drag) return;
    const over = w === drag.from ? null : w;
    const mode = shift ? "move" : "copy";
    const rx = Math.round(x / 6) * 6;
    const ry = Math.round(y / 6) * 6;
    if (drag.over === over && !drag.folder && drag.mode === mode && drag.x === rx && drag.y === ry) return;
    dispatch({ type: "drag", drag: { ...drag, over, folder: null, mode, x: rx, y: ry } });
  }
  const dragEnd = (): void => dispatch({ type: "drag", drag: null });
  function drop(w: WinIdx, shift: boolean): void {
    const drag = get().drag;
    dispatch({ type: "drag", drag: null });
    if (!drag || w === drag.from) return;
    openTransfer(shift ? "move" : "copy", drag.from);
  }
  /** Zielordner für das Ablegen auf einer Ordnerzeile; null, wenn die Zeile kein gültiges Ziel ist. */
  function folderTarget(w: WinIdx, index: number): { w: WinIdx; name: string; path: string } | null {
    const drag = get().drag;
    const entry = entriesOf(w)[index];
    if (!drag || !entry || entry.kind !== "dir" || entry.name === "..") return null;
    // Ein gezogener Ordner kann nicht in sich selbst abgelegt werden.
    if (w === drag.from && selectionOf(w).selected.has(entry.name)) return null;
    return { w, name: entry.name, path: joinPath(activeTabOf(get(), w).path, entry.name) };
  }
  /** Ziehen über einer Ordnerzeile; true, wenn dort abgelegt werden darf. */
  function dragOverFolder(w: WinIdx, index: number, shift: boolean, x: number, y: number): boolean {
    const drag = get().drag;
    const folder = folderTarget(w, index);
    if (!drag || !folder) return false;
    const mode = shift ? "move" : "copy";
    const rx = Math.round(x / 6) * 6;
    const ry = Math.round(y / 6) * 6;
    if (drag.folder?.w === folder.w && drag.folder.name === folder.name && drag.mode === mode && drag.x === rx && drag.y === ry) return true;
    dispatch({ type: "drag", drag: { ...drag, over: null, folder, mode, x: rx, y: ry } });
    return true;
  }
  /** Ablegen auf einer Ordnerzeile (auch im selben Fenster); true, wenn der Auftrag geöffnet wurde. */
  function dropOnFolder(w: WinIdx, index: number, shift: boolean): boolean {
    const folder = folderTarget(w, index);
    const drag = get().drag;
    if (!drag || !folder) return false;
    dispatch({ type: "drag", drag: null });
    openTransfer(shift ? "move" : "copy", drag.from, folder.path);
    return true;
  }

  // ---------------------------------------------------------------- Aufträge und Ereignisse

  function handleEvent(event: CoreEvent): void {
    if (disposed) return;
    switch (event.type) {
      case "discoveryChanged": void refreshDiscovery(); return;
      case "placesChanged": void refreshPlaces(); trashChanged(); return;
      case "job": {
        // 9b exposes the job ID through events before undoRun resolves.
        if (event.status.name === "Rückgängig") {
          dispatch({ type: "jobKind", id: event.status.id, kind: "undo" });
          if (get().undoBusy) dispatch({ type: "undoState", patch: { undoJob: event.status.id } });
        }
        dispatch({ type: "job", event });
        compareFeature.onJobEvent(event.status);
        const status = event.status;
        if (["done", "failed", "cancelled"].includes(status.state)) rememberedErrors.delete(status.id);
        if (status.state === "done") { void refreshPlaces(); trashChanged(); }
        if (status.state === "done" && status.failed.length === 0) {
          later(() => dispatch({ type: "closeJob", id: status.id }), jobDismissMs);
        }
        return;
      }
      case "closeSaveRequested": void flushForClose(); return;
      case "closeRequested": return dispatch({ type: "closeRequested", count: event.count });
      case "itemError":
        dispatch({ type: "itemErrorAdd", error: event.error });
        if (rememberedErrors.has(event.error.jobId)) void answerItemError(event.error.jobId, "skip");
        return;
      case "conflict":
        return dispatch({ type: "conflictAdd", conflict: event.conflict });
      case "dirChanged":
        return reloadMatching(event.path);
      // A profile was saved, deleted or imported, possibly in the editor window.
      case "connectionsChanged":
        return void refreshConnections();
    }
  }

  /** Opens the profile editor in its own window; `profile` undefined = new connection. */
  async function openSettings(tab?: import("../ipc/types").SettingsTab): Promise<void> {
    const { theme, accent, accentAutomatic, density, fontSize } = get().settings;
    try {
      const result = await api.settingsWindowOpen(tab ?? null, tr("settings.title"), { theme, accent, accentAutomatic: accentAutomatic ?? false, density, fontSize, lang: language() });
      if (result === "inline") dispatch({ type: "dialog", dialog: { kind: "settings", ...(tab === "info" ? { tab } : {}) } });
    } catch (error) { fail(error); }
  }

  async function openConnectionEditor(profile?: import("../ipc/types").ConnectionProfile, discoveryKey?: string): Promise<void> {
    const { theme, accent, accentAutomatic, density, fontSize } = get().settings;
    try {
      const opened = await api.connectionEditorOpen(profile?.id ?? null, tr("ssh.profile"), { theme, accent, accentAutomatic: accentAutomatic ?? false, density, fontSize, lang: language() }, ...(discoveryKey ? [discoveryKey] as const : []));
      // A second editor is never opened: the first one is brought forward, and the user is told why.
      if (!opened) toast(tr("ssh.editorOpen"));
    } catch (error) { fail(error); }
  }

  const rememberedErrors = new Set<number>();
  async function answerItemError(jobId: number, decision: import("../ipc/types").ItemErrorDecision, remember = false): Promise<void> {
    const question = get().itemErrors.find(error => error.jobId === jobId);
    if (decision === "skip" && remember) rememberedErrors.add(jobId);
    // Remove before sending: the next question can arrive before the command resolves.
    dispatch({ type: "itemErrorDone", jobId });
    try { await api.answerItemError(jobId, decision); } catch (error) {
      rememberedErrors.delete(jobId);
      const job = get().jobs.jobs.find(job => job.id === jobId);
      if (question && (!job || !["done", "failed", "cancelled"].includes(job.state)) && !get().itemErrors.some(error => error.jobId === jobId)) {
        dispatch({ type: "itemErrorAdd", error: question });
      }
      fail(error);
    }
  }
  async function flushForClose(): Promise<void> {
    await saveNow();
    try { await api.appCloseNow(); } catch (error) { fail(error); }
  }
  async function appConfirmClose(): Promise<void> {
    try { await api.appConfirmClose(); dispatch({ type: "closeRequested", count: null }); } catch (error) { fail(error); }
  }
  async function pauseJob(id: number, paused: boolean): Promise<void> {
    try { await (paused ? api.jobPause(id) : api.jobResume(id)); } catch (error) { fail(error); }
  }
  async function pauseAllJobs(paused: boolean): Promise<void> {
    await Promise.all(get().jobs.jobs.filter(j => paused ? j.state === "running" || j.state === "queued" : j.state === "paused").map(j => pauseJob(j.id, paused)));
  }
  async function cancelJob(id: number): Promise<void> {
    try {
      await api.cancelJob(id);
    } catch (error) {
      if (get().jobKinds[id] === "undo") toast(tr("undoError.io"), "error");
      else fail(error);
    }
  }

  // ---------------------------------------------------------------- Speichern (entprellt)

  let saveTimer: unknown = null;
  let lastSaved = "";
  let scheduledSnapshot = "";
  let lastSettingsRef: Settings | null = null;
  let lastWindows: AppState["windows"] | null = null;
  let lastActive: WinIdx | null = null;

  const toSession = (win: tabs.WindowState) => tabs.toSession(win);
  const currentSnapshot = (): string => JSON.stringify(settingsToPersist(get(), toSession));

  let saving: Promise<void> = Promise.resolve();
  function saveNow(strict = false): Promise<void> {
    if (saveTimer !== null) timers.clear(saveTimer);
    saveTimer = null;
    // Serialize writes, and let close await writes already in progress as well.
    saving = saving.catch(() => undefined).then(async () => {
      const state = get();
      if (state.phase !== "ready" || state.outcome.kind === "newerVersion") {
        if (strict) throw new Error(tr("system.saveUnavailable"));
        return;
      }
      const settings = settingsToPersist(state, toSession);
      const text = JSON.stringify(settings);
      if (text === lastSaved) return;
      try {
        await api.saveSettings(settings);
        lastSaved = text;
        dispatch({ type: "saveError", error: "" });
        // A change made while this write ran was compared against the old lastSaved and may have
        // been skipped (e.g. switching back to it): catch up instead of keeping the stale file.
        const now = currentSnapshot();
        if (!disposed && saveTimer === null && now !== text) scheduleSave(now);
      } catch (error) {
        lastSaved = "";
        dispatch({ type: "saveError", error: error instanceof Error ? error.message : String(error) });
        if (!disposed) fail(error);
        if (strict) throw error;
      }
    });
    return saving;
  }

  // ---------------------------------------------------------------- Ordnerbeobachtung

  let watchedKey = "";

  /** Meldet dem Kern die Ordner der aktiven Reiter beider Fenster, sobald sie sich ändern. */
  function syncWatch(): void {
    const state = get();
    if (state.phase !== "ready") return;
    const paths = [...new Set(([0, 1] as const).map((w) => activeTabOf(state, w).path))].filter((p) => p !== "" && !isTrashPath(p)).sort();
    const key = JSON.stringify(paths);
    if (key === watchedKey) return;
    watchedKey = key;
    // Die Beobachtung ist nur Komfort (Strg+R lädt von Hand neu): ein Fehler ist keine Meldung wert.
    api.watch(paths).catch(() => undefined);
  }

  function onStoreChange(): void {
    syncPlacesVisibility();
    const state = get();
    if (state.phase !== "ready") return;
    syncWatch();
    if (state.settings === lastSettingsRef && state.windows === lastWindows && state.active === lastActive) return;
    lastSettingsRef = state.settings;
    lastWindows = state.windows;
    lastActive = state.active;
    const snapshot = currentSnapshot();
    if (snapshot === lastSaved) return;
    // Only new content restarts the delay: a folder reloading over and over (new listing, same
    // session) would otherwise keep pushing the save away and it would never be written.
    if (saveTimer !== null && snapshot === scheduledSnapshot) return;
    scheduleSave(snapshot);
  }

  function scheduleSave(snapshot: string): void {
    scheduledSnapshot = snapshot;
    if (saveTimer !== null) timers.clear(saveTimer);
    saveTimer = timers.set(() => {
      saveTimer = null;
      void saveNow();
    }, saveDelay);
  }

  // ---------------------------------------------------------------- Start und Ende

  async function start(): Promise<void> {
    unsubscribeEvents = api.onEvent(handleEvent);
    unsubscribeUndo = api.onUndoChanged(() => void refreshHistory());
    unsubscribeNativeDrop = api.onNativeDrop(nativeDropEvent);
    await refreshHistory();
    await refreshConnections();
    try {
      const [{ settings, outcome }, home] = await Promise.all([api.loadSettings(), api.homeDir()]);
      if (disposed) return;
      dispatch({ type: "init", settings, outcome, home });
      lastVisibleMode = settings.sidebarMode === "icons" ? "icons" : "full";
      syncPlacesVisibility();
      window.addEventListener("focus", focusPlaces);
      document.addEventListener("visibilitychange", visibilityPlaces);
      later(pollPlaces, 60000);
      await Promise.all([refreshPlaces(), refreshDiscovery()]);
      lastSaved = currentSnapshot();
      lastSettingsRef = get().settings;
      lastWindows = get().windows;
      lastActive = get().active;
      syncWatch();
      unsubscribeStore = store.subscribe(onStoreChange);
      if (outcome.kind === "fresh") {
        dispatch({ type: "settings", update: s => ({ ...s, lastSeenVersion: currentVersion }) });
      } else {
        const versions = selectReleaseVersions(Object.keys(releaseNotes), currentVersion, settings.lastSeenVersion, outcome);
        if (versions.length) dispatch({ type: "dialog", dialog: { kind: "whatsNew", versions } });
      }
      await Promise.all([reload(0), reload(1)]);
      for (const status of await api.jobs()) handleEvent({ type: "job", status });
    } catch (error) {
      if (!disposed) fail(error);
    }
  }

  function dispose(): void {
    if (saveTimer !== null) void saveNow();
    disposed = true;
    window.removeEventListener("focus", focusPlaces);
    document.removeEventListener("visibilitychange", visibilityPlaces);
    void api.placesSetVisible(false).catch(() => {});
    [...jobWaiters].forEach((cancel) => cancel());
    unsubscribeEvents?.();
    unsubscribeUndo?.();
    unsubscribeNativeDrop?.();
    unsubscribeStore?.();
    if (watchedKey !== "") api.watch([]).catch(() => undefined);
    timerHandles.forEach((handle) => timers.clear(handle));
    timerHandles.clear();
  }

  return {
    ...compareFeature,
    api,
    refreshConnections,
    refreshPlaces, openPlace, dropOnPlace, pinDraggedBookmark, pinBookmarkPath, bookmarkTarget, unmountPlace, confirmPlaceAction, savePlaceConnection, toggleSidebar, toggleSidebarIcons, setSidebarWidth, collapseSidebarSection, hidePlace, focusSidebar, focusFileList,
    setSidebarFocus: (key: string | null) => dispatch({ type: "sidebarFocus", key }),
    openConnectionEditor,
    openSettings,
    disconnectConnection: async (profileId: string): Promise<void> => {
      try {
        const entry = get().connections.find(e => e.profile.id === profileId);
        if (entry?.profile.protocol === "system") return;
        await api.connectionDisconnect(profileId);
        await refreshConnections();
      } catch (error) { if (!disposed) fail(error); }
    },
    start,
    dispose,
    command,
    closeLister,
    listerStep,
    openExternalPath,
    describeError,
    typeahead,
    isTypeaheadKey,
    tr,
    language,
    toast,
    reload,
    navigateTo,
    goUp,
    goBack,
    goForward,
    navigatePlace,
    chooseConnection,
    setPageSize: (w: WinIdx, size: number): void => {
      pageSize[w] = Math.max(1, size);
    },
    setActive: (w: WinIdx): void => dispatch({ type: "active", w }),
    clickRow,
    activateRow,
    openContextMenu,
    closeMenu: (): void => dispatch({ type: "menu", menu: null }),
    openWhatsNew: (): void => {
      if (get().outcome.kind !== "newerVersion") dispatch({ type: "dialog", dialog: { kind: "whatsNew", versions: [currentVersion] } });
    },
    closeDialog: (): void => {
      if (isUnmountBusy()) return;
      if (get().dialog?.kind === "whatsNew" && get().outcome.kind !== "newerVersion") {
        dispatch({ type: "settings", update: s => ({ ...s, lastSeenVersion: seenVersionOnClose(currentVersion, s.lastSeenVersion) }) });
        void saveNow();
      }
      dispatch({ type: "dialog", dialog: null });
    },
    openDialog: (dialog: NonNullable<AppState["dialog"]>): void => dispatch({ type: "dialog", dialog }),
    sortBy,
    setFilter,
    newTab,
    closeTab,
    closeOtherTabs,
    toggleTabLock,
    openTabMenu,
    selectTab,
    setPathEditing: (w: WinIdx | null): void => dispatch({ type: "pathEdit", w }),
    openTransfer,
    confirmTransfer,
    answerConflict,
    confirmDelete,
    submitRename,
    submitMakeDir,
    openProperties,
    openRename,
    openMultiRename,
    canMultiRename,
    canPack,
    openPack,
    submitPack,
    inArchive,
    submitMultiRename,
    savePresetNamed,
    deletePresetNamed,
    setRenamePresets,
    trashSelection,
    dropPaths,
    nativeDropEvent,
    confirmTrash: async () => {
      const dialog = get().dialog;
      if (dialog?.kind !== "trashConfirm") return;
      dispatch({ type: "dialog", dialog: null });
      await trashPaths(dialog.paths);
    },
    copyPath,
    applyPattern,
    undo,
    dismissUndoToast: (): void => dispatch({ type: "undoState", patch: { undoToast: null } }),
    updateSettings,
    togglePin,
    openBookmark,
    editBookmark,
    unpinBookmark,
    moveBookmark,
    openTrash,
    openSystemTrash,
    trashRestore,
    askTrashPurge,
    confirmTrashPurge,
    openTrashOriginal,
    dragStart,
    dragOver,
    dragEnd,
    drop,
    dragOverFolder,
    dropOnFolder,
    cancelJob, pauseJob, pauseAllJobs, answerItemError, appConfirmClose,
    dismissCloseRequest: () => dispatch({ type: "closeRequested", count: null }),
    setJobsExpanded: (expanded: boolean) => dispatch({ type: "jobsExpanded", expanded }),
    closeJob: (id: number): void => dispatch({ type: "closeJob", id }),
    dismissNotice: (): void => dispatch({ type: "dismissNotice" }),
    removeToast: (id: number): void => dispatch({ type: "toastRemove", id }),
    saveNow,
    jobKindOf: (id: number): JobKind | undefined => get().jobKinds[id],
  };
}

export type Controller = ReturnType<typeof createController>;
