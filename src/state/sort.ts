import type { Entry } from "../ipc/types";

export type SortKey = "name" | "size" | "type" | "modified" | "permissions" | "uid" | "gid";
export type SortDirection = "asc" | "desc";
export interface SortSpec {
  key: SortKey;
  direction: SortDirection;
}

export const DEFAULT_SORT: SortSpec = { key: "name", direction: "asc" };

// numeric: Datei2 vor Datei10; sensitivity "base": Groß-/Kleinschreibung und Akzente zählen nicht,
// Umlaute werden nach deutscher Regel eingeordnet (ä bei a).
const collator = new Intl.Collator("de", { numeric: true, sensitivity: "base" });

/** Natürlicher Vergleich. Der Tiebreak über die Rohzeichen macht die Ordnung total (stabil und deterministisch). */
export function naturalCompare(a: string, b: string): number {
  const result = collator.compare(a, b);
  if (result !== 0) return result;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Dateiendung kleingeschrieben; ".bashrc" und "Makefile" haben keine. */
export function fileType(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot <= 0 || dot === name.length - 1 ? "" : name.slice(dot + 1).toLowerCase();
}

function compareNumbers(a: number | null, b: number | null): number {
  // Unbekannt (null) zählt als ältester Wert; ohne diese Vorsorge ergäbe -Infinity - -Infinity NaN.
  const x = a ?? Number.NEGATIVE_INFINITY;
  const y = b ?? Number.NEGATIVE_INFINITY;
  return x < y ? -1 : x > y ? 1 : 0;
}

function compareByKey(a: Entry, b: Entry, key: SortKey): number {
  switch (key) {
    case "name":
      return naturalCompare(a.name, b.name);
    case "size":
      return compareNumbers(a.size, b.size);
    case "type":
      return naturalCompare(fileType(a.name), fileType(b.name));
    case "permissions":
      return compareNumbers(a.mode === null ? null : a.mode & 0o7777, b.mode === null ? null : b.mode & 0o7777);
    case "uid":
    case "gid":
      return compareNumbers(a[key] ?? null, b[key] ?? null);
    case "modified":
      return compareNumbers(a.modified, b.modified);
  }
}

/**
 * Sortiert ohne die Eingabe zu verändern. ".." bleibt ganz oben, danach alle Ordner, danach Dateien –
 * unabhängig von der Richtung (die Richtung gilt innerhalb der Gruppen). Bei Größe/Typ haben
 * Ordner keinen sinnvollen Wert und werden deshalb nach Name aufsteigend geordnet.
 */
export function sortEntries<T extends Entry>(
  entries: readonly T[],
  key: SortKey,
  direction: SortDirection = "asc",
): T[] {
  const sign = direction === "asc" ? 1 : -1;
  const group = (e: Entry): number => (e.name === ".." ? 0 : e.kind === "dir" ? 1 : 2);
  return entries
    .map((entry, index) => ({ entry, index }))
    .sort((a, b) => {
      const groups = group(a.entry) - group(b.entry);
      if (groups !== 0) return groups;
      if (group(a.entry) === 0) return a.index - b.index;
      const dirWithoutMeaning = group(a.entry) === 1 && (key === "size" || key === "type");
      const result = dirWithoutMeaning
        ? naturalCompare(a.entry.name, b.entry.name)
        : sign * compareByKey(a.entry, b.entry, key);
      // Gleichstand: Eingabereihenfolge (stabil, unabhängig von der Engine).
      return result !== 0 ? result : a.index - b.index;
    })
    .map(({ entry }) => entry);
}

/** Sitzungsstring → Spezifikation; Ungültiges ergibt die Standardsortierung. */
export function parseSort(value: string): SortSpec {
  const match = /^(name|size|type|modified|permissions|uid|gid)(?::(asc|desc))?$/.exec(value);
  if (!match) return { ...DEFAULT_SORT };
  return { key: match[1] as SortKey, direction: (match[2] as SortDirection | undefined) ?? "asc" };
}

/** "name" für die Standardsortierung, sonst "key:direction". */
export function formatSort(spec: SortSpec): string {
  return spec.key === "name" && spec.direction === "asc" ? "name" : `${spec.key}:${spec.direction}`;
}

/** Klick auf eine Kopfzeile: gleiche Spalte kehrt die Richtung um, neue Spalte startet aufsteigend. */
export function toggleSort(current: SortSpec, key: SortKey): SortSpec {
  if (current.key === key) return { key, direction: current.direction === "asc" ? "desc" : "asc" };
  return { key, direction: "asc" };
}

/** Hidden ownership metadata must not determine the visible order. */
export function visibleSort(value: string, owners: boolean): SortSpec {
  const spec = parseSort(value);
  return !owners && ["permissions", "uid", "gid"].includes(spec.key) ? { ...DEFAULT_SORT } : spec;
}
