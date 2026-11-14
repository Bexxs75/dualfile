// Reine Hilfen für Vergleichen, Synchronisieren und Textvergleich: Sitzungstypen, Zustandsmarken,
// Zeilenaufbau der Textansicht, Zähler und Beschriftung des Bestätigungsknopfs. Kein Zugriff auf die API.
import type {
  CompareEntry,
  CompareOptions,
  ComparePage,
  CompareState,
  DiffHunk,
  DiffOptions,
  DiffResult,
  JobStatus,
  SyncAction,
  SyncMode,
  SyncPlan,
} from "../ipc/types";
import { naturalCompare } from "./sort";
import { isArchivePath } from "./paths";

/** Einträge je Seite der Ergebnisliste (der Kern liefert höchstens `MAX_COMPARE_PAGE`). */
export const COMPARE_PAGE_SIZE = 100;
/** Aktionen je Seite der Sync-Vorschau (nur Anzeige; geladen wird in Kernseiten). */
export const SYNC_VIEW_PAGE_SIZE = 100;
/** Seitengröße beim Nachladen der Aktionen (`syncPlanPage`). */
export const SYNC_FETCH_PAGE = 1000;
/** Obergrenze der Aktionen, die die Oberfläche zum Abwählen lädt; darüber kein Einzelabwählen. */
export const SYNC_LOAD_LIMIT = 20_000;

// ---------------------------------------------------------------- Zustandsmarken

export interface StateMark {
  /** Zeichen, das auch ohne Farbe unterscheidbar ist. */
  symbol: string;
  /** CSS-Klasse für die Farbe. */
  tone: "same" | "left" | "right" | "diff" | "warn" | "unknown";
}

export const STATE_MARKS: Record<CompareState, StateMark> = {
  same: { symbol: "=", tone: "same" },
  onlyLeft: { symbol: "◀", tone: "left" },
  onlyRight: { symbol: "▶", tone: "right" },
  leftNewer: { symbol: "◀", tone: "left" },
  rightNewer: { symbol: "▶", tone: "right" },
  different: { symbol: "≠", tone: "diff" },
  typeMismatch: { symbol: "⚠", tone: "warn" },
  unknown: { symbol: "?", tone: "unknown" },
};

/** Zustände, bei denen ein Textvergleich zweier Dateien sinnvoll ist. */
export function canDiffEntry(entry: CompareEntry): boolean {
  return entry.left?.kind === "file" && entry.right?.kind === "file" && entry.state !== "unknown";
}

// ---------------------------------------------------------------- Sitzung: Vergleich

export interface CompareFilterState {
  states: readonly CompareState[];
  text: string;
}

export type ComparePhase = "setup" | "running" | "ready" | "failed" | "cancelled";

export interface SyncSession {
  mode: SyncMode;
  replaceNewer: boolean;
  phase: "planning" | "preview" | "running" | "finished" | "error";
  /** Vollständiger Plan ohne Abwahl; seine Aktionen stehen in `actions`. */
  base: SyncPlan | null;
  actions: readonly SyncAction[];
  /** Zu wenige geladen (mehr als `SYNC_LOAD_LIMIT`): Einzelabwahl nicht möglich. */
  actionsPartial: boolean;
  /** Abgewählte Aktionen (Pfad relativ zum Zielordner). */
  unchecked: ReadonlySet<string>;
  /** Der Plan, der ausgeführt würde (Basisplan oder Neuplan mit Auswahl). */
  effective: SyncPlan | null;
  /** Anzahl der Neupläne seit dem Basisplan (die Kernseite behält nur wenige Pläne). */
  replans: number;
  viewOffset: number;
  jobId: number | null;
  status: JobStatus | null;
  error: string | null;
}

export interface CompareSession {
  includeHidden: boolean;
  left: string;
  right: string;
  options: CompareOptions;
  phase: ComparePhase;
  jobId: number | null;
  status: JobStatus | null;
  error: string | null;
  filter: CompareFilterState;
  /** Beginn der angezeigten Seite im gefilterten Ergebnis. */
  offset: number;
  page: ComparePage | null;
  /** Gewählte Zeilen (relativer Pfad), über Seiten hinweg. */
  selected: ReadonlySet<string>;
  sync: SyncSession | null;
}

export function newCompareSession(left: string, right: string, includeHidden = false): CompareSession {
  return {
    left,
    right,
    includeHidden,
    options: { mode: "sizeTime", recursive: true },
    phase: "setup",
    jobId: null,
    status: null,
    error: null,
    filter: { states: [], text: "" },
    offset: 0,
    page: null,
    selected: new Set(),
    sync: null,
  };
}

/** Welche Richtungen sind erlaubt? Ein Archiv ist nie Ziel (der Kern lehnt es ebenfalls ab). */
export function allowedSyncModes(left: string, right: string): SyncMode[] {
  const leftArchive = isArchivePath(left);
  const rightArchive = isArchivePath(right);
  const modes: SyncMode[] = [];
  if (!rightArchive) modes.push("leftToRight");
  if (!leftArchive) modes.push("rightToLeft");
  if (!leftArchive && !rightArchive) modes.push("mirror");
  return modes;
}

// ---------------------------------------------------------------- Sync-Vorschau

/** Aktionen, die etwas überschreiben oder entfernen: sie werden rot und mit Text markiert. */
export const isDestructive = (action: SyncAction): boolean => action.kind === "replace" || action.kind === "trash";

/**
 * Ordner, die nur als Voraussetzung für gewählte Aktionen angelegt werden: der Kern führt sie bei
 * Auswahl mit; die Liste zeigt sie als „benötigt“.
 */
export function neededDirs(actions: readonly SyncAction[], unchecked: ReadonlySet<string>): Set<string> {
  const needed = new Set<string>();
  const dirs = new Set(actions.filter((a) => a.kind === "makeDir").map((a) => a.rel));
  for (const action of actions) {
    if (action.kind === "makeDir" || action.kind === "trash" || unchecked.has(action.rel)) continue;
    for (let up = action.rel; up.includes("/"); ) {
      up = up.slice(0, up.lastIndexOf("/"));
      if (dirs.has(up)) needed.add(up);
    }
  }
  return needed;
}

/** Pfade der Aktionen, die ausgeführt werden sollen (nicht abgewählt). */
export const checkedRels = (actions: readonly SyncAction[], unchecked: ReadonlySet<string>): string[] =>
  actions.filter((action) => !unchecked.has(action.rel)).map((action) => action.rel);

export interface ConfirmParts {
  copies: number;
  replaces: number;
  trashes: number;
  makeDirs: number;
}

/** Zähler für den Bestätigungsknopf; leer, wenn nichts zu tun ist. */
export function confirmParts(plan: SyncPlan): ConfirmParts {
  return { copies: plan.copies, replaces: plan.replaces, trashes: plan.trashes, makeDirs: plan.makeDirs };
}

// ---------------------------------------------------------------- Textvergleich

export interface DiffSession {
  left: string;
  right: string;
  options: DiffOptions;
  phase: "loading" | "ready" | "error";
  result: DiffResult | null;
  error: string | null;
  /** Index in der Liste der Unterschiede (0-basiert); -1 solange es keinen gibt. */
  current: number;
}

export interface DiffRow {
  kind: DiffHunk["kind"];
  /** Index des Abschnitts in `DiffResult.hunks`. */
  hunk: number;
  leftNo: number | null;
  left: string | null;
  rightNo: number | null;
  right: string | null;
}

/** Baut die Zeilen der Zwei-Spalten-Ansicht; ungleich lange Abschnitte werden mit Lücken aufgefüllt. */
export function buildDiffRows(hunks: readonly DiffHunk[]): DiffRow[] {
  const rows: DiffRow[] = [];
  hunks.forEach((hunk, index) => {
    const count = Math.max(hunk.left.length, hunk.right.length);
    for (let i = 0; i < count; i += 1) {
      const hasLeft = i < hunk.left.length;
      const hasRight = i < hunk.right.length;
      rows.push({
        kind: hunk.kind,
        hunk: index,
        leftNo: hasLeft ? hunk.leftStart + i + 1 : null,
        left: hasLeft ? hunk.left[i]! : null,
        rightNo: hasRight ? hunk.rightStart + i + 1 : null,
        right: hasRight ? hunk.right[i]! : null,
      });
    }
  });
  return rows;
}

/** Indizes der Abschnitte, die nicht gleich sind, in Reihenfolge. */
export const differenceHunks = (hunks: readonly DiffHunk[]): number[] =>
  hunks.flatMap((hunk, index) => (hunk.kind === "equal" ? [] : [index]));

/** Nächster oder vorheriger Unterschied; stößt am Anfang/Ende an (kein Umbruch). */
export function stepDifference(current: number, delta: 1 | -1, total: number): number {
  if (total === 0) return -1;
  return Math.min(total - 1, Math.max(0, current + delta));
}

/** Hidden directories hide all descendants, exactly as in panel navigation. */
export const hiddenComparePath = (rel: string): boolean => rel.split("/").some(part => part.startsWith(".") && part !== "." && part !== "..");

/** Sort the entire result before slicing pages, using the panels' comparator. */
export function presentComparePage(snapshot: ComparePage, includeHidden: boolean, filter: CompareFilterState, offset: number): ComparePage {
  const visible = snapshot.entries.filter(entry => includeHidden || !hiddenComparePath(entry.rel));
  const entries = visible.filter(entry => (filter.states.length === 0 || filter.states.includes(entry.state)) && entry.rel.toLowerCase().includes(filter.text.toLowerCase()))
    .sort((a, b) => naturalCompare(a.rel, b.rel));
  const counts = { ...snapshot.summary.counts };
  for (const state of Object.keys(counts) as CompareState[]) counts[state] = 0;
  for (const entry of visible) counts[entry.state]++;
  return { ...snapshot, offset, total: entries.length, entries: entries.slice(offset, offset + COMPARE_PAGE_SIZE), summary: { ...snapshot.summary, entries: visible.length, counts } };
}

/** Also protect directory actions containing hidden descendants (including Mirror removals). */
export function hiddenSyncAction(action: SyncAction, hiddenPaths: readonly string[]): boolean {
  const { rel, kind } = action;
  return hiddenComparePath(rel) || ((kind === "trash" || kind === "replace") && hiddenPaths.some(path => rel === "" || path.startsWith(`${rel}/`)));
}
