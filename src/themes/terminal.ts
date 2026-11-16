import type { ITheme } from "@xterm/xterm";
import { contrastRatio, themes, type ThemeName } from "./tokens";

export type TerminalColorMode = "theme" | "classic" | "green" | "amber" | "light" | "custom";

export const terminalColorModes: readonly TerminalColorMode[] = ["theme", "classic", "green", "amber", "light", "custom"];
export const DEFAULT_TERMINAL_COLORS: TerminalColorMode = "green";
export const DEFAULT_TERMINAL_BG = "#102030";
export const DEFAULT_TERMINAL_FG = "#f0e6d2";
/** WCAG-Schwelle für normalen Text; darunter zeigt der Dialog einen Hinweis. */
export const MIN_TERMINAL_CONTRAST = 4.5;

export interface TerminalColors { bg: string; fg: string }

const presets: Record<"classic" | "green" | "amber" | "light", TerminalColors> = {
  classic: { bg: "#000000", fg: "#e6e6e6" },
  green: { bg: "#0a120a", fg: "#4af626" },
  amber: { bg: "#160f05", fg: "#ffb000" },
  light: { bg: "#fbfaf6", fg: "#1f2328" },
};

export interface TerminalColorSettings {
  theme: ThemeName;
  terminalColors?: TerminalColorMode;
  terminalBg?: string;
  terminalFg?: string;
}

export function terminalColorsFor(settings: TerminalColorSettings): TerminalColors {
  const mode = settings.terminalColors ?? DEFAULT_TERMINAL_COLORS;
  if (mode === "theme") return { bg: themes[settings.theme].panel, fg: themes[settings.theme].fg };
  if (mode === "custom") return { bg: settings.terminalBg ?? DEFAULT_TERMINAL_BG, fg: settings.terminalFg ?? DEFAULT_TERMINAL_FG };
  return presets[mode];
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map(i => {
    const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export const isLightBackground = (bg: string): boolean => luminance(bg) > 0.4;

type Ansi = Pick<ITheme, "black" | "red" | "green" | "yellow" | "blue" | "magenta" | "cyan" | "white"
  | "brightBlack" | "brightRed" | "brightGreen" | "brightYellow" | "brightBlue" | "brightMagenta" | "brightCyan" | "brightWhite">;

/** Für dunkle Hintergründe. black/white sind absichtlich nicht schwarz/weiß, damit sie auf dem Hintergrund sichtbar bleiben. */
const darkPalette: Ansi = {
  black: "#7d8590", red: "#f07a6a", green: "#7bc08a", yellow: "#e6c36a", blue: "#7fb4ea", magenta: "#d59bf0", cyan: "#6fd0d0", white: "#d9dee4",
  brightBlack: "#9aa3ae", brightRed: "#ff9a8c", brightGreen: "#9fdcab", brightYellow: "#f3d98c", brightBlue: "#a3cdf7", brightMagenta: "#e6b9f7", brightCyan: "#93e3e3", brightWhite: "#ffffff",
};
/** Für helle Hintergründe, alle mit mindestens 4,5:1 auf dem hellen Preset. */
const lightPalette: Ansi = {
  black: "#1f2328", red: "#b42318", green: "#1a7f37", yellow: "#7a5600", blue: "#0a5bb5", magenta: "#8a3fb0", cyan: "#0b6e75", white: "#57606a",
  brightBlack: "#424a53", brightRed: "#9a1b10", brightGreen: "#116329", brightYellow: "#6a4a00", brightBlue: "#084b96", brightMagenta: "#74309a", brightCyan: "#085a60", brightWhite: "#3a424a",
};

export function xtermTheme({ bg, fg }: TerminalColors): ITheme {
  return {
    background: bg,
    foreground: fg,
    cursor: fg,
    cursorAccent: bg,
    selectionBackground: `${fg}4d`,
    ...(isLightBackground(bg) ? lightPalette : darkPalette),
  };
}

export const terminalContrast = (colors: TerminalColors): number => contrastRatio(colors.bg, colors.fg);
