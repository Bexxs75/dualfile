import type { Settings } from "../ipc/types";

/** Breite der drei festen Spalten (px); Name bleibt flexibel. Normal: Größe, Typ, Geändert; Endungsmodus: Endung, Größe, Geändert. */
export type ColumnTriple = [number, number, number];
export type ColumnMode = "normal" | "ext";

export const COLUMN_MIN = 48;
export const COLUMN_MAX = 400;
export const COLUMN_STEP = 8;
export const COLUMN_DEFAULTS: Record<ColumnMode, ColumnTriple> = { normal: [84, 104, 130], ext: [56, 84, 130] };

export function clampColumn(value: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(COLUMN_MAX, Math.max(COLUMN_MIN, Math.round(value)));
}

/** Gespeicherte Breiten eines Modus, auf den erlaubten Bereich geklemmt; fehlende/ungültige Werte fallen auf den Standard zurück. */
export function columnWidthsFor(settings: Pick<Settings, "columnWidths">, mode: ColumnMode): ColumnTriple {
  const defaults = COLUMN_DEFAULTS[mode];
  const stored: unknown = settings.columnWidths?.[mode];
  if (!Array.isArray(stored)) return [...defaults];
  return [0, 1, 2].map((i) => clampColumn(Number(stored[i]), defaults[i])) as ColumnTriple;
}

/** Existing layouts without ownership retain their fixed widths. */
export function columnTemplate(widths: ColumnTriple): string {
  return `minmax(120px, 1fr) ${widths[0]}px ${widths[1]}px ${widths[2]}px`;
}

/** Cap metadata to the space left after name, owners and gaps; stored widths stay intact. */
export function ownerColumnTemplates(widths: ColumnTriple, ext: boolean): Record<string, string> {
  const template = (slots: number[], name = 160, owners = true) => {
    const fixed = owners ? "96px 64px 64px" : "96px";
    const reserved = name + (owners ? 224 : 96) + (slots.length + (owners ? 3 : 1)) * 6;
    const total = slots.reduce((sum, i) => sum + widths[i]!, 0);
    const metadata = slots.map(i => `min(${widths[i]}px, max(0px, calc((100% - ${reserved}px) * ${widths[i]! / total})))`);
    return [`minmax(${name}px, 1fr)`, ...metadata, fixed].join(" ");
  };
  return {
    "--df-owner-all": template([0, 1, 2]),
    "--df-owner-no-type": template(ext ? [0, 1, 2] : [0, 2]),
    "--df-owner-no-date": template(ext ? [0, 1] : [0]),
    "--df-owner-small": template(ext ? [0] : [], 120),
    "--df-owner-minimal": template([], 120, false),
  };
}
