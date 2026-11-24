import { describe, expect, test } from "vitest";
import { t } from "../i18n";
import appCss from "../styles/app.css?raw";
import {
  accentPresets,
  accentPresetsForTheme,
  accentAdjustment,
  darkenForContrast,
  displayAccent,
  effectiveAccent,
  fontWeights,
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
    const soft = themeVariables({ ...base, theme: name, accentAutomatic: true })["--accent-soft"]!;
    if (theme.colorScheme === "dark") expect(soft).toContain("color-mix");
    else for (const text of [theme.fg, theme.muted]) expect(contrastRatio(text, soft)).toBeGreaterThanOrEqual(4.5);
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
    expect(accentWarning("#f2eee5", "papier")).not.toContain("lowContrastBackground");
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

  test("helle Vorgaben auf Papier werden vor der Kontrastprüfung abgedunkelt", () => {
    expect(accentWarning("#e0a45c", "papier")).not.toContain("lowContrastBackground");
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


describe("designabhängige Akzente", () => {
  test.each(themeNames)("alle Vorgaben erfüllen Akzent- und Ink-Kontrast (%s)", theme => {
    for (const accent of accentPresetsForTheme(theme)) {
      const vars = themeVariables({ ...base, theme, accent });
      expect(vars["--accent"]).toBe(accent);
      expect(contrastRatio(accent, themes[theme].bg)).toBeGreaterThanOrEqual(3);
      expect(contrastRatio(vars["--accent-ink"]!, accent)).toBeGreaterThanOrEqual(4.5);
      expect(accentWarning(accent, theme)).toEqual([]);
    }
  });

  test.each(["weiss", "papier"] as const)("abgeleitete Flächen und Ordner sind lesbar (%s)", theme => {
    for (const accent of [...accentPresetsForTheme(theme), ...accentPresets, themes[theme].accent, "#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff"]) {
      const settings = { ...base, theme, accent };
      const vars = themeVariables(settings);
      expect(settings.accent).toBe(accent);
      expect(vars["--accent"]).toBe(effectiveAccent(settings));
      for (const surface of ["--panel", "--sel", "--cur"]) {
        expect(contrastRatio(vars["--dir"]!, vars[surface]!), `${theme} ${accent} ${surface}`).toBeGreaterThanOrEqual(4.5);
        expect(contrastRatio(vars["--fg"]!, vars[surface]!)).toBeGreaterThanOrEqual(4.5);
      }
      for (const ink of ["--fg", "--muted"]) expect(contrastRatio(vars[ink]!, vars["--accent-soft"]!)).toBeGreaterThanOrEqual(4.5);
      expect(vars["--sel"]).not.toBe(vars["--cur"]);
    }
    expect(themeVariables({ ...base, theme, accent: "#be185d" })["--sel"])
      .not.toBe(themeVariables({ ...base, theme, accent: "#1f7a3a" })["--sel"]);
  });

  test("Abdunkeln terminiert auch bei unerreichbarem Kontrast und ist idempotent", () => {
    expect(darkenForContrast("#ffffff", ["#000000", "#ffffff"], 21)).toBe("#000000");
    expect(darkenForContrast("#010101", ["#000000"], 21)).toBe("#000000");
    for (const theme of ["weiss", "papier"] as const) {
      for (const accent of [...accentPresets, "#ffffff", "#000000", "#010101"]) {
        const displayed = displayAccent(accent, theme);
        expect(displayAccent(displayed, theme)).toBe(displayed);
        expect(contrastRatio(displayed, themes[theme].bg)).toBeGreaterThanOrEqual(3);
      }
    }
  });

  test("Hinweis enthält die angewandte Farbe in DE/EN und erscheint nur bei Abdunklung", () => {
    for (const theme of ["weiss", "papier"] as const) {
      const color = accentAdjustment("#E0A45C", theme);
      expect(color).toBe(effectiveAccent({ theme, accent: "#e0a45c" }));
      expect(t("settings.accent.adjusted", { color: color! }, "de")).toBe(`Auf hellem Design zu hell: wird zu ${color} abgedunkelt`);
      expect(t("settings.accent.adjusted", { color: color! }, "en")).toBe(`Too bright on a light design: darkened to ${color}`);
      expect(accentAdjustment("bad", theme)).toBeUndefined();
      expect(accentAdjustment("#2563EB", theme)).toBeUndefined();
      expect(accentAdjustment(color!, theme)).toBeUndefined();
      const automatic = { ...base, theme, accentAutomatic: true };
      expect(effectiveAccent(automatic)).toBe(themes[theme].accent);
    }
    expect(accentAdjustment("#ffffff", "schiefer")).toBeUndefined();
    expect(accentWarning("#777777", "weiss")).toContain("lowContrastInk");
  });

  test("dunkle Designs behalten ihre CSS-Tokens und Vorgaben", () => {
    for (const theme of ["schiefer", "werkstatt"] as const) {
      expect(accentPresetsForTheme(theme)).toEqual(["#e0a45c", "#6fb3d9", "#7bc08a", "#e07a9f", "#c98fe0"]);
    }
    expect(["schiefer", "werkstatt"].map(theme => themeVariables({ ...base, theme: theme as ThemeName }))).toMatchInlineSnapshot(`
      [
        {
          "--accent": "#e0a45c",
          "--accent-ink": "#1a1307",
          "--accent-soft": "color-mix(in srgb, #e0a45c 12%, #1c2229)",
          "--bg": "#161a1f",
          "--connection-warning": "#e6c36a",
          "--cur": "#38506a",
          "--dir": "#9cc3e6",
          "--fg": "#d9dee4",
          "--fs": "13px",
          "--fw": "400",
          "--fw-strong": "500",
          "--line": "#2c3540",
          "--line-soft": "#252e38",
          "--muted": "#a0aab6",
          "--ok": "#7bc08a",
          "--panel": "#1c2229",
          "--panel2": "#212932",
          "--raised": "#212932",
          "--rowh": "28px",
          "--sel": "#2b3b4d",
          "--warn": "#e07a5f",
        },
        {
          "--accent": "#e0a45c",
          "--accent-ink": "#15110a",
          "--accent-soft": "color-mix(in srgb, #e0a45c 12%, #241e18)",
          "--bg": "#1b1713",
          "--connection-warning": "#e6c36a",
          "--cur": "#5a3d22",
          "--dir": "#e8c27a",
          "--fg": "#eadfce",
          "--fs": "13px",
          "--fw": "400",
          "--fw-strong": "500",
          "--line": "#3a3025",
          "--line-soft": "#332a21",
          "--muted": "#b3a28d",
          "--ok": "#9bc47b",
          "--panel": "#241e18",
          "--panel2": "#2b241c",
          "--raised": "#2b241c",
          "--rowh": "28px",
          "--sel": "#40301f",
          "--warn": "#e8745a",
        },
      ]
    `);
  });
});

describe("light designs: heavier text and darker muted (#37)", () => {
  const light = themeNames.filter(name => themes[name].colorScheme === "light");
  const dark = themeNames.filter(name => themes[name].colorScheme === "dark");

  test("White and Paper are the light designs; Slate and Workshop the dark ones", () => {
    expect([...light].sort()).toEqual(["papier", "weiss"]);
    expect([...dark].sort()).toEqual(["schiefer", "werkstatt"]);
  });

  test.each(light)("%s uses weight 500 for text and 600 for emphasis", name => {
    const vars = themeVariables({ ...base, theme: name });
    expect([vars["--fw"], vars["--fw-strong"]]).toEqual(["500", "600"]);
  });

  test.each(dark)("%s keeps weight 400 for text and 500 for emphasis", name => {
    const vars = themeVariables({ ...base, theme: name });
    expect([vars["--fw"], vars["--fw-strong"]]).toEqual(["400", "500"]);
  });

  test("weights are bundled Inter cuts", () => {
    for (const { text, strong } of Object.values(fontWeights)) for (const weight of [text, strong]) expect([400, 500, 600]).toContain(weight);
  });

  test("White muted text is darker and reaches 7:1 on every surface", () => {
    const theme = themes.weiss;
    expect(theme.muted).toBe("#434b57");
    for (const surface of [theme.bg, theme.panel, theme.panel2, theme.raised, theme.accentSoft]) {
      expect(contrastRatio(theme.muted, surface), surface).toBeGreaterThanOrEqual(7);
    }
  });

  test("the stylesheet takes body and emphasis weights from the tokens", () => {
    expect(appCss).toMatch(/body \{[^}]*font: var\(--fw\) var\(--fs\)/);
    expect(appCss).not.toMatch(/font-weight:\s*(normal|400|500)\b/);
    // The font shorthand resets the weight to 400 unless it names one: plain text must take --fw.
    const shorthands = [...appCss.matchAll(/(?:^|[;{\s])font:\s*([^;}]+)/g)].map(m => m[1]!.trim()).filter(value => value !== "inherit");
    expect(shorthands.length).toBeGreaterThan(5);
    for (const value of shorthands) expect(value, value).toMatch(/^(var\(--fw(-strong)?\)|[5-9]00)\s/);
    expect(appCss).not.toMatch(/font:\s*(normal|400)\s/);
    expect(appCss).toContain("--fw: 400; --fw-strong: 500;");
  });

  test("applyTheme sets the weights on the root element", () => {
    const root = document.createElement("div");
    applyTheme(root, { ...base, theme: "weiss" });
    expect(root.style.getPropertyValue("--fw")).toBe("500");
    applyTheme(root, { ...base, theme: "schiefer" });
    expect(root.style.getPropertyValue("--fw")).toBe("400");
  });
});
