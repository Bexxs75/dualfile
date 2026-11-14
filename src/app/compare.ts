// Abläufe für Vergleichen, Synchronisieren und Textvergleich. Hängt am Controller (siehe
// `createController`): Zustand liegt in `AppState.compare`/`AppState.diff`, alles Asynchrone hier.
// Sicherheit: ausgeführt wird nur das Token des Plans, den der Nutzer in der Vorschau sieht
// (`SyncSession.effective`); jede Änderung der Auswahl erzeugt einen neuen Plan mit neuem Token.
import { CoreError, type DualfileApi } from "../ipc/api";
import type { ComparePage, CompareOptions, CompareState, DiffOptions, JobStatus, SyncMode, SyncPlan } from "../ipc/types";
import { MSG_DIFF_BINARY, MSG_DIFF_TOO_LARGE } from "../ipc/types";
import type { TranslationKey, TranslationParams } from "../i18n";
import {
  allowedSyncModes,
  presentComparePage,
  hiddenComparePath,
  hiddenSyncAction,
  checkedRels,
  newCompareSession,
  stepDifference,
  SYNC_FETCH_PAGE,
  SYNC_LOAD_LIMIT,
  type CompareFilterState,
  type CompareSession,
  type DiffSession,
  type SyncSession,
} from "../state/compare";
import { joinPath } from "../state/paths";
import { activeTabOf, visibleEntries, type Action, type AppState, type JobKind, type WinIdx } from "./state";
import { isFinished } from "../state/operations";

export interface CompareDeps {
  api: DualfileApi;
  get: () => AppState;
  dispatch: (action: Action) => void;
  tr: (key: TranslationKey, params?: TranslationParams) => string;
  toast: (text: string, kind?: "info" | "error") => void;
  describeError: (error: unknown, context?: { path?: string; name?: string }) => string;
}

export function createCompareFeature(deps: CompareDeps) {
  const { api, get, dispatch, tr, toast, describeError } = deps;
  const noop = (): void => undefined;

  const patch = (update: (session: CompareSession) => CompareSession): void =>
    dispatch({ type: "compare", update: (session) => (session ? update(session) : session) });
  const patchSync = (update: (sync: SyncSession) => SyncSession): void =>
    patch((session) => (session.sync ? { ...session, sync: update(session.sync) } : session));
  const patchDiff = (update: (diff: DiffSession) => DiffSession): void =>
    dispatch({ type: "diff", update: (diff) => (diff ? update(diff) : diff) });
  const markKind = (id: number, kind: JobKind): void => dispatch({ type: "jobKind", id, kind });
  /** Der Auftragsstand, wie die Auftragsliste ihn führt (Zähler laufen nie rückwärts). */
  const latest = (status: JobStatus): JobStatus => get().jobs.jobs.find((job) => job.id === status.id) ?? status;

  let pageSeq = 0;
  let snapshot: ComparePage | null = null;
  let syncSeq = 0;
  let replanSeq = 0;
  let diffSeq = 0;
  /** Vergleiche, deren Dialog schon zu ist, während der Auftrag noch lief: Ergebnis nach dem Ende freigeben. */
  const closedWhileRunning = new Set<number>();

  // ---------------------------------------------------------------- Vergleich

  function openCompare(): void {
    if (get().compare !== null) return;
    for (const w of [0, 1] as const) {
      if (get().listings[w].error !== null) return void toast(tr("compare.notReadable"), "error");
    }
    const left = activeTabOf(get(), 0).path;
    const right = activeTabOf(get(), 1).path;
    dispatch({ type: "compare", update: () => newCompareSession(left, right, get().showHidden) });
  }

  function setCompareOptions(options: Partial<CompareOptions>): void {
    patch((session) => (session.phase === "running" ? session : { ...session, options: { ...session.options, ...options } }));
  }

  /** Gibt das Ergebnis frei, das die Sitzung hält (falls vorhanden). */
  async function release(session: CompareSession): Promise<void> {
    if (session.jobId !== null && (session.page !== null || session.status?.state === "done")) await api.compareClose(session.jobId).catch(noop);
  }

  async function startCompare(): Promise<void> {
    const session = get().compare;
    if (!session || session.phase === "running") return;
    ++pageSeq;
    snapshot = null;
    await release(session);
    if (get().compare === null) return;
    patch((s) => ({
      ...s,
      phase: "running",
      jobId: null,
      status: null,
      error: null,
      page: null,
      offset: 0,
      selected: new Set(),
      sync: null,
    }));
    try {
      const id = await api.compareDirs(session.left, session.right, session.options);
      markKind(id, "compare");
      if (get().compare === null) {
        // Der Dialog wurde während des Startens geschlossen.
        closedWhileRunning.add(id);
        await api.cancelJob(id).catch(noop);
        return;
      }
      patch((s) => ({ ...s, jobId: id }));
      const known = get().jobs.jobs.find((job) => job.id === id);
      if (known) onJobEvent(known);
    } catch (error) {
      patch((s) => ({ ...s, phase: "failed", error: describeError(error) }));
    }
  }

  const cancelCompare = async (): Promise<void> => {
    const id = get().compare?.jobId;
    if (id != null) await api.cancelJob(id).catch((error) => toast(describeError(error), "error"));
  };

  async function loadPage(offset: number): Promise<void> {
    const session = get().compare;
    if (!session || session.jobId === null) return;
    const id = session.jobId;
    const seq = ++pageSeq;
    try {
      if (snapshot?.id !== id) {
        const first = await api.comparePage(id, 0, 1000);
        const entries = [...first.entries];
        while (entries.length < first.total) {
          if (seq !== pageSeq || get().compare?.jobId !== id) return;
          const next = await api.comparePage(id, entries.length, 1000);
          if (next.entries.length === 0) throw new Error(tr("compare.notReadable"));
          entries.push(...next.entries);
        }
        if (seq !== pageSeq || get().compare?.jobId !== id) return;
        snapshot = { ...first, entries };
      }
      if (seq !== pageSeq || get().compare?.jobId !== id) return;
      const page = presentComparePage(snapshot, session.includeHidden, session.filter, offset);
      patch((s) => ({ ...s, page, offset, phase: s.phase === "running" ? "ready" : s.phase }));
    } catch (error) {
      if (seq !== pageSeq || get().compare?.jobId !== id) return;
      patch((s) => ({ ...s, phase: s.page ? s.phase : "failed", error: describeError(error) }));
      toast(describeError(error), "error");
    }
  }

  function setCompareFilter(change: Partial<CompareFilterState>): void {
    patch((s) => ({ ...s, filter: { ...s.filter, ...change } }));
    void loadPage(0);
  }

  function setCompareHidden(includeHidden: boolean): void {
    if (get().compare?.phase === "running" || get().compare?.sync) return;
    patch(s => ({ ...s, includeHidden, selected: new Set() }));
    if (get().compare?.page) void loadPage(0);
  }

  function toggleCompareState(state: CompareState): void {
    const current = get().compare?.filter.states ?? [];
    setCompareFilter({ states: current.includes(state) ? current.filter((entry) => entry !== state) : [...current, state] });
  }

  const compareGoto = (offset: number): Promise<void> => loadPage(Math.max(0, offset));

  function setCompareRows(rels: readonly string[], on: boolean): void {
    patch((s) => {
      const selected = new Set(s.selected);
      for (const rel of rels) {
        if (on) selected.add(rel);
        else selected.delete(rel);
      }
      return { ...s, selected };
    });
  }
  const clearCompareRows = (): void => patch((s) => ({ ...s, selected: new Set() }));

  function closeCompare(): void {
    const session = get().compare;
    if (!session) return;
    // Läuft ein Abgleich, bleibt der Dialog offen: erst abbrechen.
    if (session.sync?.phase === "running") return;
    ++pageSeq;
    snapshot = null;
    dispatch({ type: "diff", update: () => null });
    dispatch({ type: "compare", update: () => null });
    if (session.jobId === null) return;
    if (session.phase === "running" && session.status?.state !== "done") {
      closedWhileRunning.add(session.jobId);
      void api.cancelJob(session.jobId).catch(noop);
    } else void release(session);
  }

  /** Auftragsereignisse: Vergleich fertig → erste Seite laden; Abgleich fertig → Bericht. */
  function onJobEvent(incoming: JobStatus): void {
    const status = latest(incoming);
    if (closedWhileRunning.has(status.id)) {
      if (isFinished(status.state)) {
        closedWhileRunning.delete(status.id);
        if (status.state === "done") void api.compareClose(status.id).catch(noop);
      }
      return;
    }
    const session = get().compare;
    if (!session) return;
    if (session.jobId === status.id) {
      patch((s) => ({ ...s, status }));
      if (session.phase !== "running") return;
      if (status.state === "done") void loadPage(0);
      else if (status.state === "failed") {
        patch((s) => ({ ...s, phase: "failed", error: status.failed[0]?.message ?? tr("error.unknown") }));
      } else if (status.state === "cancelled") patch((s) => ({ ...s, phase: "cancelled" }));
    }
    if (session.sync?.jobId === status.id) {
      patchSync((sync) => ({ ...sync, status, phase: isFinished(status.state) ? "finished" : sync.phase }));
    }
  }

  // ---------------------------------------------------------------- Abgleich

  const isPlanCurrent = (seq: number, compareId: number): boolean =>
    seq === syncSeq && get().compare?.jobId === compareId && get().compare?.sync != null;

  /** Plant ohne Abwahl, lädt alle Aktionen und wendet danach die Zeilenauswahl des Vergleichs an. */
  async function planBase(): Promise<void> {
    const session = get().compare;
    const sync = session?.sync;
    if (!session || !sync || session.jobId === null) return;
    const compareId = session.jobId;
    const seq = ++syncSeq;
    ++replanSeq;
    patchSync((s) => ({ ...s, phase: "planning", error: null, base: null, effective: null, actions: [], unchecked: new Set(), replans: 0 }));
    try {
      const plan = await api.planSync(compareId, sync.mode, { replaceNewer: sync.replaceNewer });
      const actions = [...plan.actions];
      while (actions.length < plan.totalActions && actions.length < SYNC_LOAD_LIMIT) {
        const page = await api.syncPlanPage(plan.token, actions.length, SYNC_FETCH_PAGE);
        if (page.actions.length === 0) break;
        actions.push(...page.actions);
      }
      if (!isPlanCurrent(seq, compareId)) return;
      const partial = actions.length < plan.totalActions;
      const selected = session.selected;
      if (partial && (selected.size > 0 || !session.includeHidden)) {
        patchSync((s) => ({ ...s, phase: "error", error: tr("sync.error.selectionTooLarge") }));
        return;
      }
      const hiddenPaths = snapshot?.entries.filter(entry => hiddenComparePath(entry.rel)).map(entry => entry.rel) ?? [];
      const excluded = new Set(!session.includeHidden ? actions.filter(a => hiddenSyncAction(a, hiddenPaths)).map(a => a.rel) : []);
      const unchecked = new Set(actions.filter(a => excluded.has(a.rel) || (selected.size > 0 && !selected.has(a.rel))).map(a => a.rel));
      patchSync((s) => ({
        ...s,
        phase: "preview",
        base: plan,
        actions: actions.filter(a => !excluded.has(a.rel)),
        actionsPartial: partial,
        unchecked,
        effective: unchecked.size > 0 ? null : plan,
        replans: 0,
        viewOffset: 0,
      }));
      if (unchecked.size > 0 || excluded.size > 0) await replan();
    } catch (error) {
      if (seq === syncSeq) patchSync((s) => ({ ...s, phase: "error", error: describeError(error) }));
    }
  }

  /** Neuer Plan für die angehakten Aktionen; ohne Abwahl gilt der Basisplan, solange sein Token sicher lebt. */
  async function replan(): Promise<void> {
    const session = get().compare;
    const sync = session?.sync;
    if (!session || !sync || session.jobId === null || !sync.base || sync.actionsPartial) return;
    const compareId = session.jobId;
    const seq = ++replanSeq;
    const checked = [...new Set(checkedRels(sync.actions, sync.unchecked))];
    if (checked.length === 0) {
      patchSync((s) => ({ ...s, effective: null, error: null }));
      return;
    }
    if (checked.length === sync.base.totalActions && sync.unchecked.size === 0 && sync.replans === 0) {
      patchSync((s) => ({ ...s, effective: s.base, error: null }));
      return;
    }
    patchSync(s => ({ ...s, effective: null }));
    try {
      const plan: SyncPlan = await api.planSync(compareId, sync.mode, { replaceNewer: sync.replaceNewer }, checked);
      if (seq !== replanSeq || get().compare?.sync == null) return;
      patchSync((s) => ({ ...s, effective: plan, replans: s.replans + 1, error: null }));
    } catch (error) {
      if (seq !== replanSeq) return;
      patchSync((s) => ({ ...s, effective: null, error: describeError(error) }));
    }
  }

  function openSync(): void {
    const session = get().compare;
    if (!session || session.phase !== "ready" || session.sync) return;
    const modes = allowedSyncModes(session.left, session.right);
    const mode = modes[0];
    if (!mode) return void toast(tr("sync.noTarget"), "error");
    const sync: SyncSession = {
      mode,
      replaceNewer: false,
      phase: "planning",
      base: null,
      actions: [],
      actionsPartial: false,
      unchecked: new Set(),
      effective: null,
      replans: 0,
      viewOffset: 0,
      jobId: null,
      status: null,
      error: null,
    };
    patch((s) => ({ ...s, sync }));
    void planBase();
  }

  function setSyncMode(mode: SyncMode): void {
    const session = get().compare;
    if (!session?.sync || session.sync.phase === "running" || !allowedSyncModes(session.left, session.right).includes(mode)) return;
    patchSync((s) => ({ ...s, mode }));
    void planBase();
  }

  function setSyncReplaceNewer(replaceNewer: boolean): void {
    if (!get().compare?.sync || get().compare?.sync?.phase === "running") return;
    patchSync((s) => ({ ...s, replaceNewer }));
    void planBase();
  }

  function toggleSyncAction(rel: string): void {
    patchSync((s) => {
      if (s.phase !== "preview" || s.actionsPartial) return s;
      const unchecked = new Set(s.unchecked);
      if (unchecked.has(rel)) unchecked.delete(rel);
      else unchecked.add(rel);
      return { ...s, unchecked };
    });
    void replan();
  }

  function setSyncAll(on: boolean): void {
    patchSync((s) =>
      s.phase !== "preview" || s.actionsPartial ? s : { ...s, unchecked: on ? new Set() : new Set(s.actions.map((a) => a.rel)) },
    );
    void replan();
  }

  const setSyncViewOffset = (offset: number): void => patchSync((s) => ({ ...s, viewOffset: Math.max(0, offset) }));

  /** Führt genau den Plan aus, den die Vorschau zeigt. Ohne Aktion oder in anderer Phase passiert nichts. */
  async function confirmSync(): Promise<void> {
    const sync = get().compare?.sync;
    if (!sync || sync.phase !== "preview" || !sync.effective || sync.effective.totalActions === 0) return;
    const token = sync.effective.token;
    patchSync((s) => ({ ...s, phase: "running", error: null, status: null }));
    try {
      const id = await api.runSync(token);
      markKind(id, "sync");
      if (get().compare?.sync == null) return;
      patchSync((s) => ({ ...s, jobId: id }));
      const known = get().jobs.jobs.find((job) => job.id === id);
      if (known) onJobEvent(known);
    } catch (error) {
      // Das Token gehört dem Plan der Vorschau; ein abgelehntes Token heißt: nichts wurde ausgeführt.
      const text = error instanceof CoreError && error.kind === "invalidToken" ? tr("sync.error.invalidToken") : describeError(error);
      patchSync((s) => ({ ...s, phase: "error", error: text }));
    }
  }

  const cancelSync = async (): Promise<void> => {
    const id = get().compare?.sync?.jobId;
    if (id != null) await api.cancelJob(id).catch((error) => toast(describeError(error), "error"));
  };

  function closeSync(): void {
    if (get().compare?.sync?.phase === "running") return;
    ++syncSeq;
    ++replanSeq;
    patch((s) => ({ ...s, sync: null }));
  }

  /** Nach dem Abgleich: Vergleich mit denselben Optionen wiederholen. */
  async function recompare(): Promise<void> {
    const session = get().compare;
    if (!session || session.sync?.phase === "running") return;
    ++syncSeq;
    ++replanSeq;
    patch((s) => ({ ...s, sync: null }));
    await startCompare();
  }

  // ---------------------------------------------------------------- Textvergleich

  function diffErrorText(error: unknown): string {
    if (error instanceof CoreError && error.kind === "unsupported") {
      if (error.message === MSG_DIFF_BINARY) return tr("diff.error.binary");
      if (error.message === MSG_DIFF_TOO_LARGE) return tr("diff.error.tooLarge");
    }
    return describeError(error);
  }

  async function loadDiff(): Promise<void> {
    const diff = get().diff;
    if (!diff) return;
    const seq = ++diffSeq;
    patchDiff((d) => ({ ...d, phase: "loading", error: null }));
    try {
      const result = await api.diffFiles(diff.left, diff.right, diff.options);
      if (seq !== diffSeq || !get().diff) return;
      patchDiff((d) => ({ ...d, phase: "ready", result, current: result.differences > 0 ? 0 : -1 }));
    } catch (error) {
      if (seq !== diffSeq || !get().diff) return;
      patchDiff((d) => ({ ...d, phase: "error", result: null, current: -1, error: diffErrorText(error) }));
    }
  }

  function openDiff(left: string, right: string): void {
    dispatch({
      type: "diff",
      update: () => ({
        left,
        right,
        options: { ignoreWhitespace: false, ignoreCase: false },
        phase: "loading",
        result: null,
        error: null,
        current: -1,
      }),
    });
    void loadDiff();
  }

  function setDiffOption(change: Partial<DiffOptions>): void {
    patchDiff((d) => ({ ...d, options: { ...d.options, ...change } }));
    void loadDiff();
  }

  function diffStep(delta: 1 | -1): void {
    patchDiff((d) => (d.result ? { ...d, current: stepDifference(d.current, delta, d.result.differences) } : d));
  }

  function closeDiff(): void {
    ++diffSeq;
    dispatch({ type: "diff", update: () => null });
  }

  /** Zwei Dateien aus den Fenstern: zwei markierte im aktiven Fenster, sonst je eine (markiert oder fokussiert) pro Fenster. */
  function diffTargets(): { left: string; right: string } | null {
    const state = get();
    const filesOf = (w: WinIdx): { paths: string[]; marked: number } => {
      const tab = activeTabOf(state, w);
      const entries = visibleEntries(state, w);
      const marked = entries.filter((entry) => entry.name !== ".." && tab.selection.selected.has(entry.name));
      const chosen = tab.selection.selected.size > 0 ? marked : [entries[tab.selection.focus]].filter((entry) => entry !== undefined);
      return {
        paths: chosen.filter((entry) => entry.kind === "file").map((entry) => joinPath(tab.path, entry.name)),
        marked: marked.length,
      };
    };
    const active = filesOf(state.active);
    if (active.marked === 2 && active.paths.length === 2) return { left: active.paths[0]!, right: active.paths[1]! };
    const [left, right] = [filesOf(0), filesOf(1)];
    if (left.marked <= 1 && right.marked <= 1 && left.paths.length === 1 && right.paths.length === 1) {
      return { left: left.paths[0]!, right: right.paths[0]! };
    }
    return null;
  }

  function openDiffFromPanes(): void {
    const targets = diffTargets();
    if (!targets) return void toast(tr("diff.needTwoFiles"));
    openDiff(targets.left, targets.right);
  }

  return {
    openCompare,
    setCompareOptions,
    setCompareHidden,
    startCompare,
    cancelCompare,
    setCompareFilter,
    toggleCompareState,
    compareGoto,
    setCompareRows,
    clearCompareRows,
    closeCompare,
    onJobEvent,
    openSync,
    setSyncMode,
    setSyncReplaceNewer,
    toggleSyncAction,
    setSyncAll,
    setSyncViewOffset,
    confirmSync,
    cancelSync,
    closeSync,
    recompare,
    openDiff,
    setDiffOption,
    diffStep,
    closeDiff,
    diffTargets,
    openDiffFromPanes,
  };
}

export type CompareFeature = ReturnType<typeof createCompareFeature>;
