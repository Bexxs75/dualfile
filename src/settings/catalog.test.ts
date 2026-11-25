import { describe, expect, test } from "vitest";
import defaultsJson from "./backend-defaults.json?raw";
import { defaultSettings } from "../app/state";
import { createTranslator } from "../i18n";
import { catalog, normalizeSearch, resetSetting, resolveSection, revertSetting, settingChanged as changedAgainst, settingSearch } from "./catalog";
import type { Settings, SettingsBase } from "../ipc/types";

const baseOf = (settings: Settings = defaultSettings, autostart: SettingsBase["autostart"] = "off"): SettingsBase => ({ settings, autostart });
/** Compares against a base taken from the factory state unless another one is given. */
const settingChanged = (settings: Settings, id: keyof Settings, base: SettingsBase | null = baseOf()) => changedAgainst(settings, base, id);

describe("settings catalog", () => {
  test("a collapsed network section alone is not a modified value", () => {
    expect(settingChanged({ ...defaultSettings, sidebarCollapsed: ["network"] }, "sidebarCollapsed")).toBe(false);
    expect(settingChanged({ ...defaultSettings, sidebarCollapsed: ["network", "devices"] }, "sidebarCollapsed")).toBe(true);
    expect(resetSetting({ ...defaultSettings, sidebarCollapsed: ["network", "devices"] }, "sidebarCollapsed").sidebarCollapsed).toEqual(["network"]);
  });
  test("contains every editable Settings field exactly once", () => {
    const expected = ["language", "trayEnabled", "checkUpdatesOnStart", "settingsWindowSizeMode", "theme", "accentAutomatic", "accent", "fontSize", "density", "showParentEntry", "extensionColumn", "showPermissionsColumn", "showUidColumn", "showGidColumn", "paneColorLeft", "paneColorRight", "paneTintStrength", "sidebarMode", "sidebarWidth", "sidebarCollapsed", "sidebarDiscovery", "sidebarHidden", "terminalShell", "terminalColors", "terminalBg", "terminalFg", "verifyPolicy"];
    expect(catalog.map(e => e.id).sort()).toEqual(expected.sort());
    expect(catalog.every(e => !settingChanged(defaultSettings, e.id))).toBe(true);
  });
  test.each([
    ["trayEnabled", true], ["density", "compact"], ["terminalShell", "bash"], ["sidebarCollapsed", ["devices"]], ["paneColorLeft", "#112233"], ["fontSize", 20],
  ] as const)("compares and resets %s", (id, value) => {
    const changed = { ...defaultSettings, [id]: value };
    expect(settingChanged(changed, id)).toBe(true);
    expect(settingChanged(resetSetting(changed, id), id)).toBe(false);
  });
  test("searches title, help, options, section and group with accent tolerance", () => {
    const tr = createTranslator("de");
    const shell = catalog.find(e => e.id === "terminalShell")!;
    expect(settingSearch(shell, tr)).toContain(tr("settings.terminalShell.help"));
    expect(settingSearch(shell, tr)).toContain(tr("settings.terminalShell.login"));
    expect(normalizeSearch("GRÖẞE Ändern")).toBe("grosse andern");
  });
  test.each([["appearance", "darstellung"], ["system", "allgemein"], ["keyboard", "tastatur"], ["info", "info"], ["dateiliste", "dateiliste"], ["unknown", "darstellung"]])("resolves %s", (input, result) => expect(resolveSection(input)).toBe(result));
});

test("frontend defaults equal the complete backend SettingsDto fixture", () => {
  expect(defaultSettings).toEqual(JSON.parse(defaultsJson));
});

test("accent is one logical setting and revert restores both fields", () => {
  for (const accentAutomatic of [true, false]) {
    const settings = { ...defaultSettings, accentAutomatic, accent: "#123456" };
    expect(catalog.filter(e => settingChanged(settings, e.id)).map(e => e.id)).toEqual(accentAutomatic ? [] : ["accent"]);
    expect(revertSetting(settings, baseOf(), "accent")).toEqual(defaultSettings);
    expect(resetSetting(settings, "accent")).toEqual(defaultSettings);
  }
});

test("accent against a base with a custom colour", () => {
  const base = baseOf({ ...defaultSettings, accentAutomatic: false, accent: "#123456" });
  expect(settingChanged(base.settings, "accent", base)).toBe(false);
  expect(settingChanged({ ...base.settings, accent: "#654321" }, "accent", base)).toBe(true);
  expect(settingChanged({ ...base.settings, accentAutomatic: true }, "accent", base)).toBe(true);
  expect(settingChanged({ ...base.settings, accentAutomatic: true }, "accentAutomatic", base)).toBe(false);
  const back = revertSetting({ ...base.settings, accentAutomatic: true, accent: "#abcdef" }, base, "accent");
  expect(back.accent).toBe("#123456");
  expect(back.accentAutomatic).toBe(false);
});

test("hidden terminal colours only count in custom mode", () => {
  for (const terminalColors of ["green", "custom"] as const) {
    const settings = { ...defaultSettings, terminalColors, terminalBg: "#000000", terminalFg: "#ffffff" };
    expect(settingChanged(settings, "terminalBg", baseOf({ ...defaultSettings, terminalColors, terminalBg: "#111111", terminalFg: "#eeeeee" }))).toBe(terminalColors === "custom");
    expect(settingChanged(settings, "terminalFg", baseOf({ ...defaultSettings, terminalColors, terminalBg: "#111111", terminalFg: "#eeeeee" }))).toBe(terminalColors === "custom");
  }
});

describe("changes are measured against the start state, not the factory state", () => {
  test("without a base nothing is changed", () => {
    const odd = { ...defaultSettings, density: "compact", fontSize: 20, sidebarCollapsed: ["devices"] } as Settings;
    expect(catalog.some(e => settingChanged(odd, e.id, null))).toBe(false);
    expect(changedAgainst(odd, undefined, "density")).toBe(false);
    expect(revertSetting(odd, null, "density")).toBe(odd);
  });
  test("hidden sidebar entries never count as modified", () => {
    expect(settingChanged({ ...defaultSettings, sidebarHidden: ["drive:/x"] }, "sidebarHidden", baseOf({ ...defaultSettings, sidebarHidden: [] }))).toBe(false);
  });
  test("a non-default value already in the base is not marked (the bug)", () => {
    const start = { ...defaultSettings, density: "compact", fontSize: 20, trayEnabled: true } as Settings;
    expect(catalog.filter(e => settingChanged(start, e.id, baseOf(start))).map(e => e.id)).toEqual([]);
  });
  test("exactly the edited value is changed and revert restores the base value", () => {
    const start = { ...defaultSettings, density: "compact", fontSize: 20 } as Settings;
    const now = { ...start, fontSize: 14 };
    expect(catalog.filter(e => settingChanged(now, e.id, baseOf(start))).map(e => e.id)).toEqual(["fontSize"]);
    const back = revertSetting(now, baseOf(start), "fontSize");
    expect(back.fontSize).toBe(20);
    expect(back.density).toBe("compact");
    expect(settingChanged(back, "fontSize", baseOf(start))).toBe(false);
  });
  test("arrays compare independent of order; network section is ignored and kept on revert", () => {
    const start = { ...defaultSettings, sidebarCollapsed: ["places", "devices"] } as Settings;
    expect(settingChanged({ ...start, sidebarCollapsed: ["devices", "places"] }, "sidebarCollapsed", baseOf(start))).toBe(false);
    expect(settingChanged({ ...start, sidebarCollapsed: ["devices", "places", "network"] }, "sidebarCollapsed", baseOf(start))).toBe(false);
    expect(settingChanged({ ...start, sidebarCollapsed: ["devices"] }, "sidebarCollapsed", baseOf(start))).toBe(true);
    expect(revertSetting({ ...start, sidebarCollapsed: ["network", "remote"] }, baseOf(start), "sidebarCollapsed").sidebarCollapsed).toEqual(["places", "devices", "network"]);
  });
});

test("the remembered window size belongs to its mode row: either changing counts, reset and revert restore both", () => {
  const remembered = { ...defaultSettings, settingsWindowSize: { width: 900, height: 700 } };
  expect(settingChanged(remembered, "settingsWindowSizeMode")).toBe(true);
  expect(settingChanged({ ...defaultSettings, settingsWindowSizeMode: "never" }, "settingsWindowSizeMode")).toBe(true);
  expect(settingChanged(defaultSettings, "settingsWindowSizeMode")).toBe(false);
  // Against a start state that already had the size, an unchanged size is not a change.
  expect(settingChanged(remembered, "settingsWindowSizeMode", baseOf(remembered))).toBe(false);
  const both = { ...remembered, settingsWindowSizeMode: "always" as const };
  expect(resetSetting(both, "settingsWindowSizeMode")).toEqual(defaultSettings);
  expect(revertSetting(both, baseOf(), "settingsWindowSizeMode")).toEqual(defaultSettings);
  expect(revertSetting(defaultSettings, baseOf(both), "settingsWindowSizeMode")).toEqual(both);
});

test("the window-size row is found by its label and by its hints, in both languages", () => {
  const entry = catalog.find(e => e.id === "settingsWindowSizeMode")!;
  for (const lang of ["de", "en"] as const) {
    const tr = createTranslator(lang);
    const text = settingSearch(entry, tr);
    for (const key of ["settings.windowSize", "settings.windowSize.hint.ask", "settings.windowSize.hint.always", "settings.windowSize.hint.never", "settings.windowSize.always"] as const) expect(text).toContain(tr(key));
  }
});
