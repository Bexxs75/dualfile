import type { Entry } from "../ipc/types";
import { matchPattern } from "./filter";

/**
 * Unveränderliches Auswahlmodell eines Fensters. Markiert wird nach Name, nicht nach Index: so
 * überlebt die Markierung Neuladen und Umsortieren. Fokus und Anker merken sich zusätzlich den
 * Namen, damit `reconcile` sie nach Änderungen wiederfindet.
 */
export interface SelectionState {
  focus: number;
  focusedName: string | null;
  anchor: number;
  anchorName: string | null;
  selected: ReadonlySet<string>;
}

export type FocusMove = number | "home" | "end" | "pageUp" | "pageDown";

export interface SelectionSummary {
  selectedCount: number;
  totalCount: number;
  selectedBytes: number;
}

export const createSelection = (): SelectionState => ({
  focus: 0,
  focusedName: null,
  anchor: 0,
  anchorName: null,
  selected: new Set(),
});

// ".." steht für „eine Ebene hoch“ und ist nie ein echter Eintrag: nie markieren, nie kopieren.
const markable = (entry: Entry | undefined): entry is Entry => entry !== undefined && entry.name !== "..";

function clampIndex(entries: readonly Entry[], index: number): number {
  return Math.max(0, Math.min(entries.length - 1, index));
}

function withFocus(
  state: SelectionState,
  entries: readonly Entry[],
  focus: number,
  moveAnchor: boolean,
  selected: ReadonlySet<string> = state.selected,
): SelectionState {
  const index = entries.length === 0 ? 0 : clampIndex(entries, focus);
  const focusedName = entries[index]?.name ?? null;
  return {
    focus: index,
    focusedName,
    anchor: moveAnchor ? index : state.anchor,
    anchorName: moveAnchor ? focusedName : state.anchorName,
    selected,
  };
}

function rangeNames(entries: readonly Entry[], a: number, b: number): string[] {
  const names: string[] = [];
  for (let i = Math.max(0, Math.min(a, b)); i <= Math.min(entries.length - 1, Math.max(a, b)); i++) {
    const entry = entries[i];
    if (markable(entry)) names.push(entry.name);
  }
  return names;
}

/** Einfacher Klick: nur dieser Eintrag ist markiert (bei ".." keiner). */
export function click(state: SelectionState, entries: readonly Entry[], index: number): SelectionState {
  const entry = entries[index];
  return withFocus(state, entries, index, true, markable(entry) ? new Set([entry.name]) : new Set());
}

export function ctrlClick(state: SelectionState, entries: readonly Entry[], index: number): SelectionState {
  const entry = entries[index];
  const selected = new Set(state.selected);
  if (markable(entry)) {
    if (selected.has(entry.name)) selected.delete(entry.name);
    else selected.add(entry.name);
  }
  return withFocus(state, entries, index, true, selected);
}

/**
 * Bereich vom Anker bis zum Ziel. Ändert sich der Bereich gegenüber dem bisherigen Fokus, werden
 * nur die Einträge angefasst, die dadurch neu hinein- bzw. herausfallen; frühere Strg-Markierungen
 * außerhalb des Bereichs bleiben.
 */
function extendTo(state: SelectionState, entries: readonly Entry[], target: number): SelectionState {
  const to = clampIndex(entries, target);
  const selected = new Set(state.selected);
  for (const name of rangeNames(entries, state.anchor, state.focus)) selected.delete(name);
  for (const name of rangeNames(entries, state.anchor, to)) selected.add(name);
  return withFocus(state, entries, to, false, selected);
}

export function shiftClick(state: SelectionState, entries: readonly Entry[], index: number): SelectionState {
  if (entries.length === 0) return state;
  return extendTo(state, entries, index);
}

/** Einfg/Leertaste: Eintrag umschalten und eine Zeile weiter (am Ende bleibt der Fokus). */
export function toggleAndAdvance(state: SelectionState, entries: readonly Entry[]): SelectionState {
  const current = entries[state.focus];
  const selected = new Set(state.selected);
  if (markable(current)) {
    if (selected.has(current.name)) selected.delete(current.name);
    else selected.add(current.name);
  }
  return withFocus(state, entries, state.focus + 1, true, selected);
}

/** Fokus bewegen; mit `extend` (Umschalt+Taste) wird der Bereich ab dem Anker markiert. */
export function moveFocus(
  state: SelectionState,
  entries: readonly Entry[],
  move: FocusMove,
  pageSize = 10,
  extend = false,
): SelectionState {
  if (entries.length === 0) return state;
  const page = Math.max(1, pageSize);
  const target =
    typeof move === "number"
      ? state.focus + move
      : move === "home"
        ? 0
        : move === "end"
          ? entries.length - 1
          : state.focus + (move === "pageUp" ? -page : page);
  return extend ? extendTo(state, entries, target) : withFocus(state, entries, target, true);
}

export function selectByPattern(state: SelectionState, entries: readonly Entry[], pattern: string): SelectionState {
  const selected = new Set(state.selected);
  for (const entry of entries) if (markable(entry) && matchPattern(entry.name, pattern)) selected.add(entry.name);
  return { ...state, selected };
}

export function deselectByPattern(state: SelectionState, entries: readonly Entry[], pattern: string): SelectionState {
  const selected = new Set(state.selected);
  for (const entry of entries) if (matchPattern(entry.name, pattern)) selected.delete(entry.name);
  return { ...state, selected };
}

export function invert(state: SelectionState, entries: readonly Entry[]): SelectionState {
  const selected = new Set<string>();
  for (const entry of entries) if (markable(entry) && !state.selected.has(entry.name)) selected.add(entry.name);
  return { ...state, selected };
}

export function selectAll(state: SelectionState, entries: readonly Entry[]): SelectionState {
  return { ...state, selected: new Set(entries.filter(markable).map((entry) => entry.name)) };
}

export const clear = (state: SelectionState): SelectionState => ({ ...state, selected: new Set() });

/**
 * Nach Neuladen/Umsortieren: verschwundene Namen aus der Markierung entfernen, Fokus (und Anker)
 * auf denselben Namen setzen. Ist er weg, rückt der Fokus an die alte Position nach – das ist der
 * nächste Nachbar, weil die Folgeeinträge nachgerutscht sind.
 */
export function reconcile(state: SelectionState, entries: readonly Entry[]): SelectionState {
  const present = new Set(entries.map((entry) => entry.name));
  const selected = new Set([...state.selected].filter((name) => present.has(name) && name !== ".."));
  const find = (name: string | null, fallback: number): number => {
    const found = name === null ? -1 : entries.findIndex((entry) => entry.name === name);
    return found >= 0 ? found : fallback;
  };
  const lastIndex = Math.max(0, entries.length - 1);
  const focus = find(state.focusedName, Math.min(state.focus, lastIndex));
  const anchor = find(state.anchorName, Math.min(state.anchor, lastIndex));
  return {
    focus,
    focusedName: entries[focus]?.name ?? null,
    anchor,
    anchorName: entries[anchor]?.name ?? null,
    selected,
  };
}

/**
 * Für die Statuszeile „x von y markiert“. ".." zählt nicht mit; Ordner zählen als markiert,
 * ihre Größe aber nicht (unbekannt ohne Durchlauf) – nur Dateien tragen zu den Bytes bei.
 */
export function summary(state: SelectionState, entries: readonly Entry[]): SelectionSummary {
  const real = entries.filter(markable);
  const chosen = real.filter((entry) => state.selected.has(entry.name));
  return {
    selectedCount: chosen.length,
    totalCount: real.length,
    selectedBytes: chosen.reduce((sum, entry) => sum + (entry.kind === "file" ? entry.size : 0), 0),
  };
}
