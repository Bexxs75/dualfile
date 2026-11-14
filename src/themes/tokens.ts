// Zentrale Farbwerte der drei Themes; Kopfbereich nach dem Redesign-Mockup.
// Gedämpfte Textfarben erfüllen auch auf abgesetzten Kopfflächen WCAG AA.
import type { Settings } from "../ipc/types";
import type { TranslationKey } from "../i18n";

export type ThemeName = Settings["theme"];
export type Density = Settings["density"];

export interface ThemeTokens {
  /** Fensterhintergrund */
  bg: string;
  panel: string;
  panel2: string;
  raised: string;
  lineSoft: string;
  accentSoft: string;
  connectionWarning: string;
  line: string;
  /** Haupttextfarbe */
  fg: string;
  muted: string;
  /** Standard-Akzent des Themes */
  accent: string;
  /** Textfarbe auf dem Akzent */
  accentInk: string;
  selection: string;
  current: string;
  directory: string;
  warning: string;
  success: string;
  colorScheme: "dark" | "light";
}

export const themeNames: readonly ThemeName[] = ["schiefer", "papier", "werkstatt"];

export const themes: Record<ThemeName, ThemeTokens> = {
  schiefer: {
    bg: "#161a1f",
    panel: "#1c2229",
    panel2: "#212932",
    raised: "#212932",
    lineSoft: "#252e38",
    accentSoft: "#2b3b4d",
    connectionWarning: "#e6c36a",
    line: "#2c3540",
    fg: "#d9dee4",
    muted: "#a0aab6",
    accent: "#e0a45c",
    accentInk: "#1a1307",
    selection: "#2b3b4d",
    current: "#38506a",
    directory: "#9cc3e6",
    warning: "#e07a5f",
    success: "#7bc08a",
    colorScheme: "dark",
  },
  papier: {
    bg: "#ece8df",
    panel: "#f6f3ec",
    panel2: "#efebe2",
    raised: "#efebe2",
    lineSoft: "#dfd9ce",
    accentSoft: "#dfe8e3",
    connectionWarning: "#806000",
    line: "#d6d0c2",
    fg: "#2a2824",
    muted: "#625c53",
    accent: "#2f6f6a",
    accentInk: "#ffffff",
    selection: "#dfe8e3",
    current: "#c9dcd5",
    directory: "#2d5d8a",
    warning: "#b3452c",
    success: "#3d7a4a",
    colorScheme: "light",
  },
  werkstatt: {
    bg: "#1b1713",
    panel: "#241e18",
    panel2: "#2b241c",
    raised: "#2b241c",
    lineSoft: "#332a21",
    accentSoft: "#40301f",
    connectionWarning: "#e6c36a",
    line: "#3a3025",
    fg: "#eadfce",
    muted: "#b3a28d",
    accent: "#d4663f",
    accentInk: "#1d0c05",
    selection: "#40301f",
    current: "#5a3d22",
    directory: "#e8c27a",
    warning: "#e8745a",
    success: "#9bc47b",
    colorScheme: "dark",
  },
};

/** Zeilenhöhen und Schriftgrößen wie im Mockup (Kompakt 22, Normal 28, Luftig 36 px). */
export const rowHeights: Record<Density, number> = { compact: 22, normal: 28, comfortable: 36 };
export const FONT_SIZE_MIN = 10;
export const FONT_SIZE_MAX = 24;

/** Vorgegebene Akzentfarben aus dem Mockup. */
export const accentPresets: readonly string[] = ["#e0a45c", "#6fb3d9", "#7bc08a", "#e07a9f", "#c98fe0"];

/** Dunkle Textfarbe auf eigenen Akzenten (wie im Mockup), sonst Weiß. */
const INK_DARK = "#15110a";
const INK_LIGHT = "#ffffff";

// ---------------------------------------------------------------- Farben

export function isValidHex(value: string): boolean {
  return /^#[0-9a-fA-F]{6}$/.test(value);
}

function relativeLuminance(hex: string): number {
  if (!isValidHex(hex)) throw new RangeError(`Ungültige Farbe: ${hex}`);
  const [r, g, b] = [1, 3, 5].map((index) => {
    const channel = Number.parseInt(hex.slice(index, index + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Kontrastverhältnis nach WCAG 2 (1 bis 21), unabhängig von der Reihenfolge der Farben. */
export function contrastRatio(a: string, b: string): number {
  const first = relativeLuminance(a);
  const second = relativeLuminance(b);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

/** Textfarbe auf dem Akzent: Theme-Vorgabe beim Standard-Akzent, sonst die lesbarere von Dunkel/Weiß. */
export function inkForAccent(accent: string, theme: ThemeName): string {
  const tokens = themes[theme];
  if (accent.toLowerCase() === tokens.accent) return tokens.accentInk;
  return contrastRatio(INK_DARK, accent) >= contrastRatio(INK_LIGHT, accent) ? INK_DARK : INK_LIGHT;
}

export type AccentWarning = "invalid" | "lowContrastBackground" | "lowContrastInk";

/**
 * Prüft einen Akzent: gültiger Farbwert, mindestens 3:1 gegen den Hintergrund (Bedienelemente),
 * mindestens 4,5:1 für Text auf dem Akzent. Leere Liste = in Ordnung.
 */
export function accentWarning(accent: string, theme: ThemeName): AccentWarning[] {
  if (!isValidHex(accent)) return ["invalid"];
  const warnings: AccentWarning[] = [];
  if (contrastRatio(accent, themes[theme].bg) < 3) warnings.push("lowContrastBackground");
  if (contrastRatio(inkForAccent(accent, theme), accent) < 4.5) warnings.push("lowContrastInk");
  return warnings;
}

export const accentWarningKeys: Record<AccentWarning, TranslationKey> = {
  invalid: "settings.accent.invalid",
  lowContrastBackground: "settings.accent.lowContrast",
  lowContrastInk: "settings.accent.lowInkContrast",
};

// ---------------------------------------------------------------- Anwenden

/** Keep the application's blue default outside Paper. */
export const defaultAccent = (theme: ThemeName): string => theme === "papier" ? themes.papier.accent : "#4f8cff";

export type ThemeSettings = Pick<Settings, "theme" | "accent" | "accentAutomatic" | "density" | "fontSize">;

/** Alle CSS-Variablen für die Einstellungen (Namen wie im Mockup). Ungültiges fällt auf Vorgaben zurück. */
export function themeVariables(settings: ThemeSettings): Record<string, string> {
  const tokens = themes[settings.theme] ?? themes.schiefer;
  const theme: ThemeName = settings.theme in themes ? settings.theme : "schiefer";
  const accent = settings.accentAutomatic ? defaultAccent(theme) : isValidHex(settings.accent) ? settings.accent.toLowerCase() : tokens.accent;
  const fontSize = Number.isFinite(settings.fontSize)
    ? Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(settings.fontSize)))
    : 13;
  return {
    "--bg": tokens.bg,
    "--panel": tokens.panel,
    "--panel2": tokens.panel2,
    "--raised": tokens.raised,
    "--line-soft": tokens.lineSoft,
    "--accent-soft": `color-mix(in srgb, ${accent} 12%, ${tokens.panel})`,
    "--connection-warning": tokens.connectionWarning,
    "--line": tokens.line,
    "--fg": tokens.fg,
    "--muted": tokens.muted,
    "--accent": accent,
    "--accent-ink": inkForAccent(accent, theme),
    "--sel": tokens.selection,
    "--cur": tokens.current,
    "--dir": tokens.directory,
    "--warn": tokens.warning,
    "--ok": tokens.success,
    "--rowh": `${rowHeights[settings.density] ?? rowHeights.normal}px`,
    "--fs": `${fontSize}px`,
  };
}

/** Setzt Theme, Akzent, Dichte und Schriftgröße am Wurzelelement, sofort und ohne Neustart. */
export function applyTheme(root: HTMLElement, settings: ThemeSettings): void {
  root.dataset.fm = settings.theme in themes ? settings.theme : "schiefer";
  for (const [name, value] of Object.entries(themeVariables(settings))) root.style.setProperty(name, value);
  root.style.colorScheme = (themes[settings.theme] ?? themes.schiefer).colorScheme;
}
