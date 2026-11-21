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
export function ownerColumnTemplates(widths: ColumnTriple, ext: boolean, enabled: readonly boolean[]): Record<string, string> {
  const template = (slots: number[], name = 160) => {
    const active = [96, 64, 64].filter((_, i) => enabled[i]);
    const fixed = active.map(width => `${width}px`).join(" ");
    const reserved = name + active.reduce((sum, width) => sum + width, 0) + (slots.length + active.length) * 6;
    const total = slots.reduce((sum, i) => sum + widths[i]!, 0);
    const metadata = slots.map(i => `min(${widths[i]}px, max(0px, calc((100% - ${reserved}px) * ${widths[i]! / total})))`);
    return [`minmax(${name}px, 1fr)`, ...metadata, fixed].join(" ");
  };
  return {
    "--df-owner-all": template([0, 1, 2]),
    "--df-owner-no-type": template(ext ? [0, 1, 2] : [0, 2]),
    "--df-owner-no-date": template(ext ? [0, 1] : [0]),
    "--df-owner-small": template(ext ? [0] : [], 120),
    "--df-owner-minimal": `minmax(120px, 1fr) ${[96, 64, 64].filter((_, i) => enabled[i]).map(width => `minmax(0, ${width}px)`).join(" ")}`,
  };
}

export type ColumnKey = "size" | "type" | "modified" | "permissions" | "uid" | "gid";
export type AllWidths = [number, number, number, number, number, number];
export const OWNER_DEFAULTS: ColumnTriple = [96, 64, 64];
export function defaultOrder(mode: ColumnMode): ColumnKey[] {
  return [...(mode === "ext" ? ["type", "size"] as const : ["size", "type"] as const), "modified", "permissions", "uid", "gid"];
}
export function columnOrderFor(settings: Pick<Settings, "columnOrder">, mode: ColumnMode): ColumnKey[] {
  const defaults = defaultOrder(mode);
  const stored = settings.columnOrder?.[mode];
  return [...new Set([...(Array.isArray(stored) ? stored.filter(k => defaults.includes(k)) : []), ...defaults])];
}
export function allWidthsFor(settings: Settings, mode: ColumnMode): AllWidths {
  const stored = settings.ownerColumnWidths?.[mode];
  return [...columnWidthsFor(settings, mode), ...OWNER_DEFAULTS.map((v, i) => clampColumn(stored?.[i] ?? v, v))] as AllWidths;
}

/** Tracks follow column identity, including at each responsive breakpoint. */
export function orderedTemplates(widths: AllWidths, mode: ColumnMode, order: ColumnKey[], owners: readonly boolean[]): Record<string, string> {
  const defaults = defaultOrder(mode);
  const visible = order.filter(k => defaults.indexOf(k) < 3 || owners[defaults.indexOf(k) - 3]);
  const template = (hidden: ColumnKey[], name = 160) => {
    const keys = visible.filter(k => !hidden.includes(k));
    const ownerKeys = keys.filter(k => defaults.indexOf(k) >= 3);
    const ownerTotal = ownerKeys.reduce((sum, k) => sum + widths[defaults.indexOf(k)]!, 0);
    const metadataTotal = keys.filter(k => defaults.indexOf(k) < 3).reduce((sum, k) => sum + widths[defaults.indexOf(k)]!, 0);
    const reserved = name + keys.length * 6;
    return [`minmax(${name}px, 1fr)`, ...keys.map(k => {
      const width = widths[defaults.indexOf(k)]!;
      // Preserve ownership widths while space permits, as in ownerColumnTemplates.
      // If even those no longer fit, shrink them proportionally instead of overflowing.
      const owner = defaults.indexOf(k) >= 3;
      const budget = reserved + (owner ? 0 : ownerTotal);
      const total = owner ? ownerTotal : metadataTotal;
      return `min(${width}px, max(0px, calc((100% - ${budget}px) * ${width / total})))`;
    })].join(" ");
  };
  return {
    "--df-cols": `minmax(120px, 1fr) ${visible.map(k => `${widths[defaults.indexOf(k)]}px`).join(" ")}`,
    "--df-owner-all": template([]),
    "--df-owner-no-type": template(mode === "ext" ? [] : ["type"]),
    "--df-owner-no-date": template(mode === "ext" ? ["modified"] : ["type", "modified"]),
    "--df-owner-small": template(mode === "ext" ? ["size", "modified"] : ["size", "type", "modified"], 120),
    "--df-owner-minimal": template(["size", "type", "modified"], 120),
  };
}
