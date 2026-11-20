import { describe, expect, test } from "vitest";
import {
  accentPresets,
  accentWarning,
  applyTheme,
  contrastRatio,
  inkForAccent,
  isValidHex,
  themeNames,
  themeVariables,
  themes,
  type ThemeName,
  type ThemeSettings,
} from "./tokens";

const base: ThemeSettings = { theme: "schiefer", accent: "#e0a45c", density: "normal", fontSize: 13 };

describe("Tokens", () => {
  test("es gibt genau die vier Themes Schiefer, Papier, Werkstatt und Weiß", () => {
    expect([...themeNames].sort()).toEqual(["papier", "schiefer", "weiss", "werkstatt"]);
    expect(Object.keys(themes).sort()).toEqual(["papier", "schiefer", "weiss", "werkstatt"]);
  });

  test("jedes Theme hat alle Token als gültige Farbwerte", () => {
    const keys = Object.keys(themes.schiefer).sort();
    for (const name of themeNames) {
      expect(Object.keys(themes[name]).sort(), name).toEqual(keys);
      for (const [key, value] of Object.entries(themes[name])) {
        if (key === "colorScheme") expect(["dark", "light"]).toContain(value);
        else expect(isValidHex(value as string), `${name}.${key}`).toBe(true);
      }
    }
  });

  test("die Werte stimmen mit dem Mockup überein (Stichproben je Theme)", () => {
    expect(themes.schiefer).toMatchObject({ bg: "#161a1f", fg: "#d9dee4", accent: "#e0a45c", current: "#38506a", colorScheme: "dark" });
    expect(themes.papier).toMatchObject({ bg: "#ece8df", fg: "#2a2824", accent: "#2f6f6a", accentInk: "#ffffff", colorScheme: "light" });
    expect(themes.werkstatt).toMatchObject({ bg: "#1b1713", fg: "#eadfce", accent: "#d4663f", directory: "#e8c27a", colorScheme: "dark" });
  });

  test.each(themeNames)("Text auf Hintergrund hat mindestens 4,5:1 im Theme %s", (name) => {
    const theme = themes[name];
    expect(contrastRatio(theme.fg, theme.bg)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(theme.fg, theme.panel)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(theme.fg, theme.panel2)).toBeGreaterThanOrEqual(4.5);
  });

  test.each(themeNames)("Text auf Markierung und aktueller Zeile ist lesbar im Theme %s", (name) => {
    const theme = themes[name];
    expect(contrastRatio(theme.fg, theme.selection)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(theme.fg, theme.current)).toBeGreaterThanOrEqual(4.5);
  });

  test.each(themeNames)("Ordner-, Warn- und Erfolgsfarbe sind auf dem Panel lesbar im Theme %s", (name) => {
    const theme = themes[name];
    for (const color of [theme.directory, theme.warning, theme.success]) {
      expect(contrastRatio(color, theme.panel)).toBeGreaterThanOrEqual(4.5);
    }
  });

  test.each(themeNames)("Haupttext und gedämpfter Text erreichen auf allen Kopfflächen 4,5:1 (%s)", name => {
    const theme = themes[name];
    for (const surface of [theme.bg, theme.panel, theme.panel2, theme.raised, theme.accentSoft]) {
      for (const text of [theme.fg, theme.muted]) expect(contrastRatio(text, surface), `${name}: ${text} on ${surface}`).toBeGreaterThanOrEqual(4.5);
    }
    expect(themeVariables({ ...base, theme: name, accentAutomatic: true })["--accent-soft"]).toContain("color-mix");
  });

  test.each(themeNames)("Standard-Akzent und Text darauf sind im Theme %s ohne Warnung", (name) => {
    const theme = themes[name];
    expect(contrastRatio(theme.accentInk, theme.accent)).toBeGreaterThanOrEqual(4.5);
    expect(accentWarning(theme.accent, name)).toEqual([]);
  });
});

describe("contrastRatio", () => {
  test("Schwarz auf Weiß ist 21:1, gleiche Farben 1:1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 10);
    expect(contrastRatio("#ffffff", "#ffffff")).toBeCloseTo(1, 10);
    expect(contrastRatio("#336699", "#336699")).toBeCloseTo(1, 10);
  });

  test("ist symmetrisch", () => {
    expect(contrastRatio("#e0a45c", "#161a1f")).toBeCloseTo(contrastRatio("#161a1f", "#e0a45c"), 10);
  });

  test("bekannter Zwischenwert: #777777 auf Weiß ist rund 4,48:1", () => {
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
  });

  test("Groß- und Kleinschreibung der Hexwerte ist egal", () => {
    expect(contrastRatio("#FFFFFF", "#000000")).toBeCloseTo(contrastRatio("#ffffff", "#000000"), 10);
  });

  test("ungültige Farbe wirft", () => {
    expect(() => contrastRatio("rot", "#ffffff")).toThrow(RangeError);
    expect(() => contrastRatio("#fff", "#ffffff")).toThrow(RangeError);
  });
});

describe("isValidHex", () => {
  test("erkennt nur #rrggbb", () => {
    expect(isValidHex("#12aBef")).toBe(true);
    expect(isValidHex("#000000")).toBe(true);
    expect(isValidHex("#fff")).toBe(false);
    expect(isValidHex("12aBef")).toBe(false);
    expect(isValidHex("#12aBeg")).toBe(false);
    expect(isValidHex("#12aBef0")).toBe(false);
    expect(isValidHex("")).toBe(false);
    expect(isValidHex(" #12aBef")).toBe(false);
    expect(isValidHex("#12aBef\n")).toBe(false);
  });
});

describe("accentWarning", () => {
  test("ungültiger Wert wird als solcher gemeldet", () => {
    expect(accentWarning("bad", "schiefer")).toEqual(["invalid"]);
    expect(accentWarning("#abc", "papier")).toEqual(["invalid"]);
  });

  test("zu wenig Kontrast zum Hintergrund (unter 3:1) wird gemeldet", () => {
    expect(contrastRatio("#f2eee5", themes.papier.bg)).toBeLessThan(3);
    expect(accentWarning("#f2eee5", "papier")).toContain("lowContrastBackground");
    expect(accentWarning("#202428", "schiefer")).toContain("lowContrastBackground");
  });

  test("zu wenig Kontrast für Text auf dem Akzent (unter 4,5:1) wird gemeldet", () => {
    // Mittleres Grau: weder dunkle noch helle Schrift erreicht 4,5:1.
    const grey = "#777777";
    expect(contrastRatio(inkForAccent(grey, "schiefer"), grey)).toBeLessThan(4.5);
    expect(accentWarning(grey, "schiefer")).toContain("lowContrastInk");
  });

  test("die Mockup-Vorgaben sind auf dem dunklen Theme Schiefer unproblematisch", () => {
    for (const preset of accentPresets) expect(accentWarning(preset, "schiefer"), preset).toEqual([]);
  });

  test("helle Vorgaben auf dem hellen Papier-Theme werden zu Recht beanstandet", () => {
    expect(accentWarning("#e0a45c", "papier")).toContain("lowContrastBackground");
  });

  test("Akzent auf gleichem Wert wie der Hintergrund meldet beides sinnvoll", () => {
    expect(accentWarning(themes.schiefer.bg, "schiefer")).toContain("lowContrastBackground");
  });

  test("inkForAccent nimmt beim Standard-Akzent die Theme-Farbe, sonst Dunkel oder Weiß", () => {
    expect(inkForAccent("#E0A45C", "schiefer")).toBe(themes.schiefer.accentInk);
    expect(inkForAccent("#ffff00", "schiefer")).toBe("#15110a");
    expect(inkForAccent("#101040", "schiefer")).toBe("#ffffff");
  });
});

describe("applyTheme", () => {
  test("setzt data-fm, Farben, Akzent, Zeilenhöhe und Schriftgröße am Wurzelelement", () => {
    const root = document.createElement("html");
    applyTheme(root, { theme: "papier", accent: "#2f6f6a", density: "comfortable", fontSize: 16 });
    expect(root.dataset.fm).toBe("papier");
    expect(root.style.getPropertyValue("--bg")).toBe("#ece8df");
    expect(root.style.getPropertyValue("--accent")).toBe("#2f6f6a");
    expect(root.style.getPropertyValue("--accent-ink")).toBe("#ffffff");
    expect(root.style.getPropertyValue("--rowh")).toBe("36px");
    expect(root.style.getPropertyValue("--fs")).toBe("16px");
    expect(root.style.colorScheme).toBe("light");
  });

  test("Themewechsel überschreibt alle Variablen des vorigen Themes", () => {
    const root = document.createElement("html");
    applyTheme(root, base);
    applyTheme(root, { ...base, theme: "werkstatt", accent: themes.werkstatt.accent });
    expect(root.dataset.fm).toBe("werkstatt");
    for (const [name, value] of Object.entries(themeVariables({ ...base, theme: "werkstatt", accent: themes.werkstatt.accent }))) {
      expect(root.style.getPropertyValue(name), name).toBe(value);
    }
    expect(root.style.getPropertyValue("--bg")).toBe("#1b1713");
    expect(root.style.colorScheme).toBe("dark");
  });

  test.each([
    ["compact", "22px"],
    ["normal", "28px"],
    ["comfortable", "36px"],
  ] as const)("Dichte %s ergibt Zeilenhöhe %s", (density, height) => {
    const root = document.createElement("html");
    applyTheme(root, { ...base, density });
    expect(root.style.getPropertyValue("--rowh")).toBe(height);
  });

  test("Schriftgröße wird auf 10 bis 24 px begrenzt und gerundet", () => {
    expect(themeVariables({ ...base, fontSize: 4 })["--fs"]).toBe("10px");
    expect(themeVariables({ ...base, fontSize: 99 })["--fs"]).toBe("24px");
    expect(themeVariables({ ...base, fontSize: 13.6 })["--fs"]).toBe("14px");
    expect(themeVariables({ ...base, fontSize: Number.NaN })["--fs"]).toBe("13px");
  });

  test("ein ungültiger Akzent fällt auf den Standard-Akzent des Themes zurück", () => {
    const vars = themeVariables({ ...base, theme: "papier", accent: "kaputt" });
    expect(vars["--accent"]).toBe(themes.papier.accent);
    expect(vars["--accent-ink"]).toBe(themes.papier.accentInk);
  });

  test("ein eigener Akzent bekommt lesbare Schriftfarbe", () => {
    const vars = themeVariables({ ...base, accent: "#6fb3d9" });
    expect(vars["--accent"]).toBe("#6fb3d9");
    expect(contrastRatio(vars["--accent-ink"] ?? "", "#6fb3d9")).toBeGreaterThanOrEqual(4.5);
  });

  test("ein unbekanntes Theme aus einer fremden Datei fällt auf Schiefer zurück", () => {
    const root = document.createElement("html");
    applyTheme(root, { ...base, theme: "gibtsnicht" as ThemeName });
    expect(root.dataset.fm).toBe("schiefer");
    expect(root.style.getPropertyValue("--bg")).toBe(themes.schiefer.bg);
  });
});

test.each(themeNames)("list text and metadata contrast on actual mixed selection surfaces (%s)", name => {
  const theme = themes[name];
  // Channel extrema also cover every custom RGB accent at the fixed 12% mix.
  for (const accent of [...accentPresets, "#4f8cff", theme.accent, "#000000", "#ffffff"]) {
    const mixed = "#" + [1, 3, 5].map(i => Math.round(
      parseInt(accent.slice(i, i + 2), 16) * .12 + parseInt(theme.panel.slice(i, i + 2), 16) * .88,
    ).toString(16).padStart(2, "0")).join("");
    for (const surface of [theme.panel, theme.raised, mixed]) {
      for (const text of [theme.fg, theme.muted]) expect(contrastRatio(text, surface), `${name} ${text} / ${surface}`).toBeGreaterThanOrEqual(4.5);
    }
  }
});
