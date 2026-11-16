import type { Bookmark } from "../ipc/types";
import { baseName, normalizePath } from "./paths";

export type BookmarkResult = { ok: true; bookmarks: Bookmark[] } | { ok: false; error: BookmarkError };
export type BookmarkError = "invalidColor" | "emptyName" | "notFound" | "badIndex";

export const DEFAULT_BOOKMARK_COLOR = "#4f8cff";

export const isValidColor = (color: string): boolean => /^#[0-9a-fA-F]{6}$/.test(color);

/** Sortiert nach `order` und nummeriert lückenlos 0..n-1 neu; alle Funktionen liefern so normalisierte Listen. */
function normalize(bookmarks: readonly Bookmark[]): Bookmark[] {
  return bookmarks
    .map((bookmark, index) => ({ bookmark, index }))
    .sort((a, b) => a.bookmark.order - b.bookmark.order || a.index - b.index)
    .map(({ bookmark }, order) => ({ ...bookmark, order }));
}

const ok = (bookmarks: readonly Bookmark[]): BookmarkResult => ({ ok: true, bookmarks: normalize(bookmarks) });

/**
 * Ordner anheften. Ist der Pfad schon angeheftet, bleibt die Liste unverändert (kein Duplikat,
 * kein Fehler: das Ziel „ist angeheftet“ ist erreicht). Name vorbelegt mit dem Ordnernamen.
 */
export function pin(
  bookmarks: readonly Bookmark[],
  path: string,
  name?: string,
  color: string = DEFAULT_BOOKMARK_COLOR,
): BookmarkResult {
  const norm = normalizePath(path);
  if (!isValidColor(color)) return { ok: false, error: "invalidColor" };
  const label = (name ?? baseName(norm)).trim();
  if (label === "") return { ok: false, error: "emptyName" };
  if (bookmarks.some((b) => normalizePath(b.path) === norm)) return ok(bookmarks);
  return ok([...bookmarks, { name: label, path: norm, color, order: bookmarks.length }]);
}

export function unpin(bookmarks: readonly Bookmark[], path: string): Bookmark[] {
  const norm = normalizePath(path);
  return normalize(bookmarks.filter((b) => normalizePath(b.path) !== norm));
}

export const isPinned = (bookmarks: readonly Bookmark[], path: string): boolean =>
  bookmarks.some((b) => normalizePath(b.path) === normalizePath(path));

function update(
  bookmarks: readonly Bookmark[],
  path: string,
  change: (b: Bookmark) => Bookmark,
): BookmarkResult {
  const norm = normalizePath(path);
  if (!bookmarks.some((b) => normalizePath(b.path) === norm)) return { ok: false, error: "notFound" };
  return ok(bookmarks.map((b) => (normalizePath(b.path) === norm ? change(b) : b)));
}

export function rename(bookmarks: readonly Bookmark[], path: string, name: string): BookmarkResult {
  const label = name.trim();
  if (label === "") return { ok: false, error: "emptyName" };
  return update(bookmarks, path, (b) => ({ ...b, name: label }));
}

/** Farbe nur als `#rrggbb`: die Farbe landet später in einem CSS-Wert, daher streng prüfen. */
export function setColor(bookmarks: readonly Bookmark[], path: string, color: string): BookmarkResult {
  if (!isValidColor(color)) return { ok: false, error: "invalidColor" };
  return update(bookmarks, path, (b) => ({ ...b, color: color.toLowerCase() }));
}

/** Verschiebt das Lesezeichen an Position `from` nach `to` (Positionen = Anzeigereihenfolge). */
export function move(bookmarks: readonly Bookmark[], from: number, to: number): BookmarkResult {
  const list = normalize(bookmarks);
  const valid = (i: number) => Number.isInteger(i) && i >= 0 && i < list.length;
  if (!valid(from) || !valid(to)) return { ok: false, error: "badIndex" };
  const [item] = list.splice(from, 1);
  list.splice(to, 0, item!);
  return ok(list.map((b, order) => ({ ...b, order })));
}

/** Alt+1…9: n-tes Lesezeichen in Anzeigereihenfolge, sonst null. */
export function byShortcut(bookmarks: readonly Bookmark[], n: number): Bookmark | null {
  if (!Number.isInteger(n) || n < 1 || n > 9) return null;
  return normalize(bookmarks)[n - 1] ?? null;
}

