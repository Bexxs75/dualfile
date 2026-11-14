import type { Entry } from "../ipc/types";

export const TYPEAHEAD_WINDOW_MS = 800;

export interface TypeaheadState {
  buffer: string;
  /** Zeitpunkt des letzten Zeichens (ms, aus der injizierten Uhr). */
  lastAt: number;
}

export interface TypeaheadResult {
  state: TypeaheadState;
  /** Neuer Fokusindex; bei keinem Treffer der unveränderte Fokus. */
  focus: number;
  matched: boolean;
}

export const createTypeahead = (): TypeaheadState => ({ buffer: "", lastAt: 0 });
export const resetTypeahead = (): TypeaheadState => createTypeahead();

/** Nur einzelne druckbare Zeichen ohne Strg/Alt zählen als Suchtext. */
export function isTypeaheadKey(event: { key: string; ctrlKey?: boolean; altKey?: boolean }): boolean {
  return [...event.key].length === 1 && !event.ctrlKey && !event.altKey && event.key.trim() !== "";
}

/**
 * Nimmt ein Zeichen auf und sucht ab dem Fokus (einschließlich) den ersten Eintrag, dessen Name
 * mit dem Puffer beginnt, ringsum. Die Uhr kommt von außen (`now`), damit Tests ohne Warten laufen.
 * Wiederholt man denselben Buchstaben ("aaa"), wird stattdessen reihum zum jeweils nächsten Eintrag
 * mit diesem Anfangsbuchstaben gesprungen – so wie im Dateimanager gewohnt.
 */
export function typeahead(
  state: TypeaheadState,
  char: string,
  now: number,
  entries: readonly Entry[],
  focus: number,
): TypeaheadResult {
  const expired = state.buffer === "" || now - state.lastAt > TYPEAHEAD_WINDOW_MS;
  const buffer = (expired ? "" : state.buffer) + char;
  const next: TypeaheadState = { buffer, lastAt: now };
  const lower = buffer.toLowerCase();
  const first = [...lower][0] ?? "";
  const repeated = [...lower].length > 1 && [...lower].every((c) => c === first);
  const needle = repeated ? first : lower;
  const start = repeated ? focus + 1 : focus;
  const count = entries.length;
  for (let step = 0; step < count; step++) {
    const index = (((start + step) % count) + count) % count;
    const entry = entries[index];
    if (entry && entry.name !== ".." && entry.name.toLowerCase().startsWith(needle)) {
      return { state: next, focus: index, matched: true };
    }
  }
  return { state: next, focus, matched: false };
}
