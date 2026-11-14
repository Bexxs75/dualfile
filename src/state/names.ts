import type { TranslationKey } from "../i18n";

/** Höchstlänge eines Namens in Bytes (UTF-8), wie im Kern. */
export const MAX_NAME_BYTES = 255;

/**
 * Prüft einen neuen Datei- oder Ordnernamen nach den Regeln des Plans (leer, `.`, `..`, Schrägstrich,
 * zu lang, schon vorhanden). Liefert den i18n-Schlüssel der Fehlermeldung oder null.
 * `current` ist der bisherige Name beim Umbenennen: derselbe Name zählt nicht als „vorhanden“.
 */
export function nameError(name: string, existing: Iterable<string>, current?: string): TranslationKey | null {
  if (name === "" || name.trim() === "") return "error.name.empty";
  if (name === "." || name === "..") return "error.name.dot";
  if (name.includes("/") || name.includes("\0")) return "error.name.slash";
  if (new TextEncoder().encode(name).length > MAX_NAME_BYTES) return "error.name.tooLong";
  if (name !== current) for (const other of existing) if (other === name) return "error.name.exists";
  return null;
}
