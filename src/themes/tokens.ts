// Zentrale Farbwerte der vier Themes; Kopfbereich nach dem Redesign-Mockup.
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

export const themeNames: readonly ThemeName[] = ["schiefer", "papier", "werkstatt", "weiss"];

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
  weiss: {
    bg: "#f4f5f7", panel: "#ffffff", panel2: "#f7f8fa", raised: "#f7f8fa",
    lineSoft: "#eceef1", line: "#d9dde3", fg: "#1b1f24", muted: "#505966",
    accent: "#2563eb", accentInk: "#ffffff", selection: "#dbe7fd", current: "#c3d6fb",
    directory: "#1d4ed8", warning: "#b4441f", success: "#1f7a3a",
    connectionWarning: "#8a6100", accentSoft: "#e3ecfd", colorScheme: "light",
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

const lightAccentPresets: readonly string[] = ["#2563eb", "#1f7a3a", "#b45309", "#be185d", "#7e22ce"];

/** Akzentvorgaben passend zum Design; die Pastellpalette bleibt auch für Lesezeichen verfügbar. */
export function accentPresetsForTheme(theme: ThemeName): readonly string[] {
  return themes[theme].colorScheme === "light" ? lightAccentPresets : accentPresets;
}

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

/** sRGB-Mischung wie bei den Pane-Farben. */
function mixColor(a: string, b: string, fraction: number): string {
  return "#" + [1, 3, 5].map(i => Math.round(
    parseInt(a.slice(i, i + 2), 16) * (1 - fraction) + parseInt(b.slice(i, i + 2), 16) * fraction,
  ).toString(16).padStart(2, "0")).join("");
}

/** Maximal 20 Schritte bis Schwarz; vermeidet Rundungsstillstand bei sehr dunklen Farben. */
export function darkenForContrast(color: string, backgrounds: readonly string[], minimum: number): string {
  for (let step = 0; step <= 20; step++) {
    const candidate = mixColor(color, "#000000", step / 20);
    if (backgrounds.every(bg => contrastRatio(candidate, bg) >= minimum)) return candidate;
  }
  return "#000000";
}

/** Nur die Anzeige wird angepasst, niemals der gespeicherte Wunschwert. */
export function displayAccent(accent: string, theme: ThemeName): string {
  return themes[theme].colorScheme === "light"
    ? darkenForContrast(accent, [themes[theme].bg], 3)
    : accent.toLowerCase();
}

/** Hinweis nur bei einem gültigen, tatsächlich abgedunkelten Akzent. */
export function accentAdjustment(accent: string, theme: ThemeName): string | undefined {
  if (!isValidHex(accent)) return undefined;
  const displayed = displayAccent(accent, theme);
  return displayed !== accent.toLowerCase() ? displayed : undefined;
}

export type AccentWarning = "invalid" | "lowContrastBackground" | "lowContrastInk";

/**
 * Prüft einen Akzent: gültiger Farbwert, mindestens 3:1 gegen den Hintergrund (Bedienelemente),
 * mindestens 4,5:1 für Text auf dem Akzent. Leere Liste = in Ordnung.
 */
export function accentWarning(accent: string, theme: ThemeName): AccentWarning[] {
  if (!isValidHex(accent)) return ["invalid"];
  accent = displayAccent(accent, theme);
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

/** Paper and White use their own accent; dark designs keep the application blue. */
export const defaultAccent = (theme: ThemeName): string => (theme === "papier" || theme === "weiss") ? themes[theme].accent : "#4f8cff";

export type ThemeSettings = Pick<Settings, "theme" | "accent" | "accentAutomatic" | "density" | "fontSize">;

/** Die tatsächlich wirksame Akzentfarbe (automatisch, eigene oder Design-Vorgabe). */
export function effectiveAccent(settings: Pick<Settings, "theme" | "accent" | "accentAutomatic">): string {
  const theme: ThemeName = settings.theme in themes ? settings.theme : "schiefer";
  const chosen = settings.accentAutomatic ? defaultAccent(theme) : isValidHex(settings.accent) ? settings.accent.toLowerCase() : themes[theme].accent;
  return displayAccent(chosen, theme);
}

/** Alle CSS-Variablen für die Einstellungen (Namen wie im Mockup). Ungültiges fällt auf Vorgaben zurück. */
export function themeVariables(settings: ThemeSettings): Record<string, string> {
  const tokens = themes[settings.theme] ?? themes.schiefer;
  const theme: ThemeName = settings.theme in themes ? settings.theme : "schiefer";
  const accent = effectiveAccent(settings);
  const light = tokens.colorScheme === "light";
  const selection = light ? mixColor(tokens.panel, accent, 0.14) : tokens.selection;
  const current = light ? mixColor(tokens.panel, accent, 0.26) : tokens.current;
  const directory = light ? darkenForContrast(accent, [tokens.panel, selection, current], 4.5) : tokens.directory;
  const fontSize = Number.isFinite(settings.fontSize)
    ? Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(settings.fontSize)))
    : 13;
  return {
    "--bg": tokens.bg,
    "--panel": tokens.panel,
    "--panel2": tokens.panel2,
    "--raised": tokens.raised,
    "--line-soft": tokens.lineSoft,
    "--accent-soft": light ? mixColor(tokens.panel, accent, 0.10) : `color-mix(in srgb, ${accent} 12%, ${tokens.panel})`,
    "--connection-warning": tokens.connectionWarning,
    "--line": tokens.line,
    "--fg": tokens.fg,
    "--muted": tokens.muted,
    "--accent": accent,
    "--accent-ink": inkForAccent(accent, theme),
    "--sel": selection,
    "--cur": current,
    "--dir": directory,
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
