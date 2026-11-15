// Zustand der Oberfläche: beide Fenster (Tabs, Auswahl, Eintragsliste), Einstellungen, Aufträge,
// Dialoge und Meldungen. Reine Funktionen; alles Asynchrone liegt im Controller.
import type { UndoEntry as HistoryEntry, UndoResult, ConflictInfo, CoreEvent, DeletePlan, Entry, LoadOutcome, Settings } from "../ipc/types";
import type { RenamePreset, RenameSource } from "../state/multirename";
import { filterEntries } from "../state/filter";
import type { CompareSession, DiffSession } from "../state/compare";
import { isFinished, jobsReducer, closeJob, emptyJobs, type DeleteDialogModel, type JobsState, type OperationPlan } from "../state/operations";
import { reconcile, type SelectionState } from "../state/selection";
import { parseSort, sortEntries } from "../state/sort";
import { joinPath } from "../state/paths";
import { activeTab, createWindow, fromSession, setSelection, type WindowState } from "../state/tabs";

export type WinIdx = 0 | 1;
export const otherWindow = (w: WinIdx): WinIdx => (w === 0 ? 1 : 0);

export interface PaneListing {
  /** Pfad, zu dem `entries` gehören; null = noch nichts geladen. */
  path: string | null;
  entries: readonly Entry[];
  error: string | null;
  loading: boolean;
}

export interface Toast {
  id: number;
  text: string;
  kind: "info" | "error";
}

export type JobKind = "copy" | "move" | "trash" | "delete" | "rename" | "extract" | "pack" | "compare" | "sync" | "undo";

export type Dialog =
  | { kind: "history" }
  | { kind: "connections"; entry?: { action: "new" | "import" | "system" | "edit" | "connect" | "disconnect"; profile?: import("../ipc/types").ConnectionProfile; onConnected?: (root: string) => Promise<boolean> } }
  | { kind: "transfer"; plan: OperationPlan; from: WinIdx }
  | { kind: "delete"; model: DeleteDialogModel; plan: DeletePlan; reason: "permanent" | "trashFailed" }
  | { kind: "rename"; w: WinIdx; dir: string; name: string; existing: readonly string[] }
  | { kind: "makeDir"; w: WinIdx; parent: string; existing: readonly string[] }
  | { kind: "multiRename"; w: WinIdx; dir: string; sources: readonly RenameSource[]; existing: readonly string[] }
  /** Packen (Strg+Umschalt+P): Quellen des Fensters `w`, Ziel ist der Ordner des Gegenfensters. */
  | { kind: "pack"; w: WinIdx; sources: readonly string[]; sourceNames: readonly string[]; destination: string; existing: readonly string[]; suggestion: string }
  /** Ein Archiv ließ sich nicht öffnen; `path` ist die Archivdatei, `external` sagt, ob „Extern öffnen“ möglich ist. */
  | { kind: "archiveError"; path: string; message: string; external: boolean }
  | { kind: "properties"; entry: Entry; path: string }
  | { kind: "pattern"; mode: "select" | "deselect"; w: WinIdx }
  | { kind: "settings" }
  | { kind: "search"; w: WinIdx }
  | { kind: "bookmark"; path: string };

export interface DragState {
  from: WinIdx;
  over: WinIdx | null;
  mode: "copy" | "move";
  x: number;
  y: number;
}

export interface ContextMenuState {
  background?: boolean;
  /** Set when the menu was opened on a tab; its index. */
  tab?: number;
  w: WinIdx;
  x: number;
  y: number;
}

/** Der Lister zeigt die Datei, auf der der Fokus des Fensters `w` steht (Blättern verschiebt den Fokus). */
export interface ListerState {
  w: WinIdx;
}

export interface AppState {
  connections: readonly import("../ipc/types").ConnectionEntry[];
  remoteConnections: import("../state/paths").RemoteConnections;
  phase: "loading" | "ready";
  home: string;
  settings: Settings;
  outcome: LoadOutcome;
  noticeVisible: boolean;
  windows: readonly [WindowState, WindowState];
  listings: readonly [PaneListing, PaneListing];
  /** Name, auf den der Fokus nach dem nächsten Laden springen soll (nach Umbenennen, Hochgehen …). */
  pendingFocus: readonly [string | null, string | null];
  active: WinIdx;
  showHidden: boolean;
  pathEditing: WinIdx | null;
  jobs: JobsState;
  jobKinds: Readonly<Record<number, JobKind>>;
  jobDestinations: Readonly<Record<number, string>>;
  jobsExpanded: boolean;
  closeRequested: number | null;
  itemErrors: readonly import("../ipc/types").ItemErrorInfo[];
  conflicts: readonly ConflictInfo[];
  dialog: Dialog | null;
  menu: ContextMenuState | null;
  toasts: readonly Toast[];
  nextToastId: number;
  history: readonly HistoryEntry[];
  undoBusy: boolean;
  undoJob: number | null;
  undoResults: Readonly<Record<number, UndoResult>>;
  undoToast: HistoryEntry | null;
  drag: DragState | null;
  lister: ListerState | null;
  /** Schnellansicht (Strg+Q): zeigt die fokussierte Datei des aktiven Fensters anstelle des anderen. */
  quickView: boolean;
  terminal: boolean;
  /** Vergleichsdialog (Strg+Umschalt+D) samt Abgleich; `null` = geschlossen. */
  compare: CompareSession | null;
  /** Textvergleich zweier Dateien; liegt über dem Vergleichsdialog. */
  diff: DiffSession | null;
  /**
   * Voreinstellungen des Mehrfach-Umbenennens. Nur im Sitzungsspeicher: `Settings` und der Rust-Kern
   * kennen dafür kein Feld, und der Kern wird in diesem Schritt nicht geändert.
   */
  renamePresets: readonly RenamePreset[];
}

export const defaultSettings: Settings = {
  version: 5,
  verifyPolicy: "auto",
  theme: "schiefer",
  accent: "#e0a45c",
  accentAutomatic: true,
  showParentEntry: true,
  showQuickAccess: false,
  extensionColumn: false,
  checkUpdatesOnStart: true,
  trayEnabled: false,
  startMenuPrompted: false,
  startMenuPromptCount: 0,
  dismissedUpdateVersion: "",
  lastUpdateCheck: null,
  density: "normal",
  fontSize: 13,
  language: "auto",
  bookmarks: [],
  mirror: false,
  session: { windows: [], activeWindow: 0 },
};

const emptyListing: PaneListing = { path: null, entries: [], error: null, loading: false };

export function initialState(): AppState {
  return {
    connections: [], remoteConnections: {},
    phase: "loading",
    home: "/",
    settings: defaultSettings,
    outcome: { kind: "fresh" },
    noticeVisible: false,
    windows: [createWindow("/"), createWindow("/")],
    listings: [emptyListing, emptyListing],
    pendingFocus: [null, null],
    active: 0,
    showHidden: false,
    pathEditing: null,
    jobs: emptyJobs,
    jobKinds: {},
    jobDestinations: {},
    jobsExpanded: true, closeRequested: null, itemErrors: [],
    conflicts: [],
    dialog: null,
    menu: null,
    toasts: [],
    nextToastId: 1,
    history: [], undoBusy: false, undoJob: null, undoResults: {}, undoToast: null,
    drag: null,
    lister: null,
    quickView: false,
    terminal: false,
    compare: null,
    diff: null,
    renamePresets: [],
  };
}

// ---------------------------------------------------------------- Sichtbare Einträge

const UP_ENTRY: Entry = {
  name: "..",
  kind: "dir",
  size: 0,
  modified: null,
  mode: null,
  hidden: false,
  linkTarget: null,
  brokenLink: false,
  error: null,
};
const NONE: readonly Entry[] = [];

interface VisibleCache {
  raw: readonly Entry[];
  key: string;
  result: readonly Entry[];
}
const caches: [VisibleCache | null, VisibleCache | null] = [null, null];

/**
 * Was ein Fenster zeigt: „..“ (außer in der Wurzel), gefiltert, sortiert. Das Ergebnis wird je
 * Fenster zwischengespeichert, damit Tastendrücke bei 5000 Einträgen nicht jedes Mal neu sortieren.
 */
export function visibleEntries(state: AppState, w: WinIdx): readonly Entry[] {
  const tab = activeTab(state.windows[w]);
  const listing = state.listings[w];
  if (listing.path !== tab.path) return NONE;
  const key = `${tab.path}\u0000${tab.sort}\u0000${tab.filter}\u0000${state.showHidden}\u0000${state.settings.showParentEntry !== false}`;
  const cached = caches[w];
  if (cached && cached.raw === listing.entries && cached.key === key) return cached.result;
  const spec = parseSort(tab.sort);
  const base = tab.path === "/" || state.settings.showParentEntry === false ? listing.entries : [UP_ENTRY, ...listing.entries];
  const result = sortEntries(filterEntries(base, tab.filter, state.showHidden), spec.key, spec.direction);
  caches[w] = { raw: listing.entries, key, result };
  return result;
}

// ---------------------------------------------------------------- Aktionen

export type Action =
  | { type: "connections"; entries: readonly import("../ipc/types").ConnectionEntry[]; remote: import("../state/paths").RemoteConnections }
  | { type: "jobsExpanded"; expanded: boolean }
  | { type: "closeRequested"; count: number | null }
  | { type: "itemErrorAdd"; error: import("../ipc/types").ItemErrorInfo }
  | { type: "itemErrorDone"; jobId: number }
  | { type: "undoState"; patch: Partial<Pick<AppState, "history" | "undoBusy" | "undoJob" | "undoResults" | "undoToast">> }
  | { type: "init"; settings: Settings; outcome: LoadOutcome; home: string }
  | {
      type: "window";
      w: WinIdx;
      update: (window: WindowState) => WindowState;
      /** Mit dem Ergebnis des Ladens zusammen anwenden (ein Schritt, kein Flackern). */
      listing?: { path: string; entries: readonly Entry[] };
      focusName?: string | null;
    }
  | { type: "listing"; w: WinIdx; path: string; entries: readonly Entry[] }
  | { type: "listingError"; w: WinIdx; path: string; message: string }
  | { type: "listingLoading"; w: WinIdx }
  | { type: "selection"; w: WinIdx; update: (selection: SelectionState, entries: readonly Entry[]) => SelectionState }
  | { type: "pendingFocus"; w: WinIdx; name: string | null }
  | { type: "active"; w: WinIdx }
  | { type: "swap" }
  | { type: "settings"; update: (settings: Settings) => Settings }
  | { type: "toggleHidden" }
  | { type: "pathEdit"; w: WinIdx | null }
  | { type: "job"; event: CoreEvent }
  | { type: "closeJob"; id: number }
  | { type: "jobKind"; id: number; kind: JobKind; destination?: string }
  | { type: "conflictAdd"; conflict: ConflictInfo }
  | { type: "conflictDone"; jobId: number }
  | { type: "dialog"; dialog: Dialog | null }
  | { type: "menu"; menu: ContextMenuState | null }
  | { type: "toastAdd"; text: string; kind: Toast["kind"] }
  | { type: "toastRemove"; id: number }
  | { type: "drag"; drag: DragState | null }
  | { type: "lister"; lister: ListerState | null }
  | { type: "quickView"; on: boolean }
  | { type: "terminal"; on: boolean }
  | { type: "compare"; update: (session: CompareSession | null) => CompareSession | null }
  | { type: "diff"; update: (session: DiffSession | null) => DiffSession | null }
  | { type: "renamePresets"; presets: readonly RenamePreset[] }
  | { type: "dismissNotice" };

const hasNotice = (outcome: LoadOutcome): boolean => outcome.kind !== "fresh" && outcome.kind !== "loaded";

function withWindow(state: AppState, w: WinIdx, window: WindowState): AppState {
  const windows: [WindowState, WindowState] = [state.windows[0], state.windows[1]];
  windows[w] = window;
  return { ...state, windows };
}

/** Gleicht Fokus und Markierung des aktiven Tabs eines Fensters mit der sichtbaren Liste ab. */
function reconcileWindow(state: AppState, w: WinIdx): AppState {
  const tab = activeTab(state.windows[w]);
  if (state.listings[w].path !== tab.path) return state; // lädt noch: Markierung nicht anfassen
  const entries = visibleEntries(state, w);
  const pending = state.pendingFocus[w];
  let selection = tab.selection;
  if (pending !== null) selection = { ...selection, focusedName: pending, anchorName: pending };
  const reconciled = reconcile(selection, entries);
  // Filters hide marks; only an actual listing removal discards them.
  reconciled.selected = reconcile(selection, filterEntries(state.listings[w].entries, "", state.showHidden)).selected;
  const next = withWindow(state, w, setSelection(state.windows[w], reconciled));
  if (pending === null) return next;
  const pendingFocus: [string | null, string | null] = [next.pendingFocus[0], next.pendingFocus[1]];
  pendingFocus[w] = null;
  return { ...next, pendingFocus };
}

function withListing(state: AppState, w: WinIdx, path: string, entries: readonly Entry[]): AppState {
  if (activeTab(state.windows[w]).path !== path) return state;
  const listings: [PaneListing, PaneListing] = [state.listings[0], state.listings[1]];
  listings[w] = { path, entries, error: null, loading: false };
  return reconcileWindow({ ...state, listings }, w);
}

function withPending(state: AppState, w: WinIdx, name: string | null): AppState {
  const pendingFocus: [string | null, string | null] = [state.pendingFocus[0], state.pendingFocus[1]];
  pendingFocus[w] = name;
  return { ...state, pendingFocus };
}

export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "connections": return { ...state, connections: action.entries, remoteConnections: action.remote };
    case "jobsExpanded": return { ...state, jobsExpanded: action.expanded };
    case "closeRequested": return { ...state, closeRequested: action.count };
    case "itemErrorAdd": return { ...state, itemErrors: [...state.itemErrors, action.error] };
    case "itemErrorDone": return { ...state, itemErrors: state.itemErrors.filter(e => e.jobId !== action.jobId) };
    case "undoState": return { ...state, ...action.patch };
    case "init": {
      const session = action.settings.session;
      const windows: [WindowState, WindowState] = [
        session.windows[0] ? fromSession(session.windows[0], action.home) : createWindow(action.home),
        session.windows[1] ? fromSession(session.windows[1], action.home) : createWindow(action.home),
      ];
      return {
        ...state,
        phase: "ready",
        home: action.home,
        settings: action.settings,
        outcome: action.outcome,
        noticeVisible: hasNotice(action.outcome),
        windows,
        active: session.activeWindow === 1 ? 1 : 0,
      };
    }
    case "window": {
      let next = withWindow(state, action.w, action.update(state.windows[action.w]));
      if (action.focusName !== undefined) next = withPending(next, action.w, action.focusName);
      if (action.listing) return withListing(next, action.w, action.listing.path, action.listing.entries);
      return reconcileWindow(next, action.w);
    }
    case "listing":
      return withListing(state, action.w, action.path, action.entries);
    case "listingError": {
      if (activeTab(state.windows[action.w]).path !== action.path) return state;
      const listings: [PaneListing, PaneListing] = [state.listings[0], state.listings[1]];
      listings[action.w] = { path: action.path, entries: [], error: action.message, loading: false };
      return { ...state, listings };
    }
    case "listingLoading": {
      const listings: [PaneListing, PaneListing] = [state.listings[0], state.listings[1]];
      listings[action.w] = { ...listings[action.w], loading: true };
      return { ...state, listings };
    }
    case "selection": {
      const tab = activeTab(state.windows[action.w]);
      const entries = visibleEntries(state, action.w);
      return withWindow(state, action.w, setSelection(state.windows[action.w], action.update(tab.selection, entries)));
    }
    case "pendingFocus":
      return withPending(state, action.w, action.name);
    case "active":
      return state.active === action.w ? state : { ...state, active: action.w };
    case "swap": {
      const caches0 = caches[0];
      caches[0] = caches[1];
      caches[1] = caches0;
      return {
        ...state,
        windows: [state.windows[1], state.windows[0]],
        listings: [state.listings[1], state.listings[0]],
        pendingFocus: [state.pendingFocus[1], state.pendingFocus[0]],
      };
    }
    case "settings": {
      const next = { ...state, settings: action.update(state.settings) };
      return (next.settings.showParentEntry !== false) === (state.settings.showParentEntry !== false)
        ? next : reconcileWindow(reconcileWindow(next, 0), 1);
    }
    case "toggleHidden": {
      const next = { ...state, showHidden: !state.showHidden };
      return reconcileWindow(reconcileWindow(next, 0), 1);
    }
    case "pathEdit":
      return { ...state, pathEditing: action.w };
    case "job": {
      const jobs = jobsReducer(state.jobs, action.event);
      if (action.event.type !== "job") return { ...state, jobs };
      const status = action.event.status;
      const conflicts = isFinished(status.state)
        ? state.conflicts.filter((conflict) => conflict.jobId !== status.id)
        : state.conflicts;
      return { ...state, jobs, conflicts, itemErrors: isFinished(status.state) ? state.itemErrors.filter(e => e.jobId !== status.id) : state.itemErrors };
    }
    case "closeJob":
      return { ...state, jobs: closeJob(state.jobs, action.id) };
    case "jobKind":
      return { ...state, jobKinds: { ...state.jobKinds, [action.id]: action.kind },
        jobDestinations: action.destination ? { ...state.jobDestinations, [action.id]: action.destination } : state.jobDestinations };
    case "conflictAdd":
      return { ...state, conflicts: [...state.conflicts, action.conflict] };
    case "conflictDone": {
      const index = state.conflicts.findIndex((conflict) => conflict.jobId === action.jobId);
      if (index < 0) return state;
      return { ...state, conflicts: state.conflicts.filter((_, i) => i !== index) };
    }
    case "dialog":
      return { ...state, dialog: action.dialog, menu: null };
    case "menu":
      return { ...state, menu: action.menu };
    case "toastAdd":
      return {
        ...state,
        toasts: [...state.toasts.filter((toast, index) => toast.kind === "error" || index >= state.toasts.length - 3), { id: state.nextToastId, text: action.text, kind: action.kind }],
        nextToastId: state.nextToastId + 1,
      };
    case "toastRemove":
      return { ...state, toasts: state.toasts.filter((toast) => toast.id !== action.id) };
    case "drag":
      return { ...state, drag: action.drag };
    case "lister":
      return { ...state, lister: action.lister, menu: null };
    case "terminal":
      return state.terminal === action.on ? state : { ...state, terminal: action.on };
    case "quickView":
      return state.quickView === action.on ? state : { ...state, quickView: action.on };
    case "compare": {
      const compare = action.update(state.compare);
      return compare === state.compare ? state : { ...state, compare, menu: null };
    }
    case "diff": {
      const diff = action.update(state.diff);
      return diff === state.diff ? state : { ...state, diff, menu: null };
    }
    case "renamePresets":
      return { ...state, renamePresets: action.presets };
    case "dismissNotice":
      return { ...state, noticeVisible: false };
  }
}

// ---------------------------------------------------------------- Abfragen

/** Eintrag mit dem Fokus eines Fensters (auch Ordner und „..“). */
export function focusedEntry(state: AppState, w: WinIdx): Entry | undefined {
  return visibleEntries(state, w)[activeTab(state.windows[w]).selection.focus];
}

/** Pfad der fokussierten Datei (Datei oder Link, nie Ordner und „..“) oder null. */
export function focusedFilePath(state: AppState, w: WinIdx): string | null {
  const entry = focusedEntry(state, w);
  if (!entry || entry.kind === "dir" || entry.name === "..") return null;
  return joinPath(activeTab(state.windows[w]).path, entry.name);
}

/** Der Teil der Einstellungen, der auf die Platte soll: inklusive aktueller Sitzung. */
export function settingsToPersist(state: AppState, toSession: (w: WindowState) => Settings["session"]["windows"][number]): Settings {
  return {
    ...state.settings,
    session: { windows: [toSession(state.windows[0]), toSession(state.windows[1])], activeWindow: state.active },
  };
}

export const activeTabOf = (state: AppState, w: WinIdx) => activeTab(state.windows[w]);
