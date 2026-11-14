// Hilfen für den Packen-Dialog: Endungen je Format und der Namensvorschlag.
import type { PackFormat } from "../ipc/types";

export const PACK_EXTENSIONS: Readonly<Record<PackFormat, string>> = {
  zip: ".zip",
  tarGz: ".tar.gz",
  tarBz2: ".tar.bz2",
  tarXz: ".tar.xz",
  sevenZ: ".7z",
};

/** Alle Endungen, die beim Wechsel des Formats ersetzt werden (auch die nicht packbare .rar nicht: nur die eigenen). */
const KNOWN = Object.values(PACK_EXTENSIONS);

/** Entfernt eine bekannte Archivendung am Ende (längste zuerst); sonst bleibt der Name unverändert. */
export function stripPackExtension(name: string): string {
  const lower = name.toLowerCase();
  const hit = [...KNOWN].sort((a, b) => b.length - a.length).find((ext) => lower.endsWith(ext) && lower.length > ext.length);
  return hit ? name.slice(0, name.length - hit.length) : name;
}

/** Name mit der Endung des Formats; eine vorhandene bekannte Endung wird ersetzt. */
export const withPackExtension = (name: string, format: PackFormat): string => `${stripPackExtension(name)}${PACK_EXTENSIONS[format]}`;

/**
 * Vorschlag aus dem ersten Namen: bei Dateien ohne die letzte Endung (`foto.jpg` → `foto`), bei Ordnern
 * und Namen ohne Endung unverändert; ohne Namen „Archiv“.
 */
export function suggestPackBase(firstName: string | undefined, isDir: boolean): string {
  if (!firstName) return "Archiv";
  const dot = firstName.lastIndexOf(".");
  return !isDir && dot > 0 ? firstName.slice(0, dot) : firstName;
}
