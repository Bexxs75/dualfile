import { describe, expect, test } from "vitest";
import { defaultSettings } from "../app/state";
import { createTranslator } from "../i18n";
import { catalog, normalizeSearch, resetSetting, resolveSection, settingChanged, settingSearch } from "./catalog";

describe("settings catalog", () => {
  test("contains every editable Settings field exactly once", () => {
    const expected = ["language", "trayEnabled", "checkUpdatesOnStart", "theme", "accentAutomatic", "accent", "fontSize", "density", "showParentEntry", "extensionColumn", "showPermissionsColumn", "showUidColumn", "showGidColumn", "paneColorLeft", "paneColorRight", "paneTintStrength", "sidebarMode", "sidebarWidth", "sidebarCollapsed", "sidebarDiscovery", "sidebarHidden", "terminalShell", "terminalColors", "terminalBg", "terminalFg", "verifyPolicy"];
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
