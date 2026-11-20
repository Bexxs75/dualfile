import { expect, test } from "vitest";
import { contrastRatio, themes } from "./tokens";
import { isLightBackground, terminalColorsFor, terminalContrast, xtermTheme } from "./terminal";

test("missing mode means green on black", () => {
  expect(terminalColorsFor({ theme: "schiefer" })).toEqual({ bg: "#0a120a", fg: "#4af626" });
});

test("theme mode follows the active design, custom uses its own colors", () => {
  expect(terminalColorsFor({ theme: "papier", terminalColors: "theme" })).toEqual({ bg: themes.papier.panel, fg: themes.papier.fg });
  expect(terminalColorsFor({ theme: "papier", terminalColors: "custom", terminalBg: "#112233", terminalFg: "#ddeeff" })).toEqual({ bg: "#112233", fg: "#ddeeff" });
  expect(terminalColorsFor({ theme: "papier", terminalColors: "amber" })).toEqual({ bg: "#160f05", fg: "#ffb000" });
});

test("contrast ratio follows WCAG", () => {
  expect(terminalContrast({ bg: "#000000", fg: "#ffffff" })).toBeCloseTo(21, 1);
  expect(terminalContrast({ bg: "#777777", fg: "#777777" })).toBeCloseTo(1, 5);
});

test("cursor is the text color and the selection is a translucent text color", () => {
  const theme = xtermTheme({ bg: "#000000", fg: "#4af626" });
  expect(theme).toMatchObject({ background: "#000000", foreground: "#4af626", cursor: "#4af626", selectionBackground: "#4af6264d" });
});

test("light and dark backgrounds get different ANSI palettes", () => {
  expect(isLightBackground("#fbfaf6")).toBe(true);
  expect(isLightBackground("#0a120a")).toBe(false);
  expect(xtermTheme({ bg: "#fbfaf6", fg: "#1f2328" }).green).not.toBe(xtermTheme({ bg: "#000000", fg: "#e6e6e6" }).green);
});

test("every ANSI color is readable on the presets that use its palette", () => {
  const ansi = ["red", "green", "yellow", "blue", "magenta", "cyan", "white", "black", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite", "brightBlack"] as const;
  const check = (bg: string) => {
    const theme = xtermTheme({ bg, fg: "#808080" });
    for (const name of ansi) expect(contrastRatio(bg, theme[name]!), `${name} on ${bg}`).toBeGreaterThanOrEqual(4.5);
  };
  check("#fbfaf6");
  for (const bg of ["#000000", "#0a120a", "#160f05"]) check(bg);
});

test("white design uses the light terminal palette", () => {
  const colors = terminalColorsFor({ theme: "weiss", terminalColors: "theme" });
  expect(colors).toEqual({ bg: "#ffffff", fg: "#1b1f24" });
  expect(xtermTheme(colors).blue).toBe("#0a5bb5");
});
