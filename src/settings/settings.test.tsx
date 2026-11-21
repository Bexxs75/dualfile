import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { setupApp } from "../test-utils";
import { defaultSettings } from "../app/state";
import { accentPresetsForTheme } from "../themes/tokens";
import { parseSettingsParams } from "../SettingsWindow";
import { currentVersion } from "../release-notes";
import type { Settings } from "../ipc/types";

afterEach(() => vi.useRealTimers());
async function setup(settings: Partial<Settings> = {}) {
  vi.spyOn(navigator, "language", "get").mockReturnValue("de-DE");
  const app = await setupApp({ settings: { ...defaultSettings, lastSeenVersion: currentVersion, autostartPrompted: true, ...settings } });
  await app.user.keyboard("{Control>},{/Control}");
  return { ...app, dlg: within(app.dialog()), state: () => app.runtime.store.getState().settings };
}

test("search matches options/help across sections, highlights, counts and navigates out of results", async () => {
  const app = await setup();
  const search = app.dlg.getByRole("searchbox");
  await app.user.type(search, "Papier");
  expect(app.dlg.getByRole("radio", { name: "Papier" }).querySelector("mark")).toHaveTextContent("Papier");
  expect(app.dlg.getByRole("tab", { name: "Darstellung 1" })).toHaveTextContent("1");
  expect(app.dlg.getByRole("tab", { name: "Terminal 0" })).toHaveClass("dim");
  await app.user.click(app.dlg.getByRole("radio", { name: "Papier" }));
  expect(app.state().theme).toBe("papier");
  await app.user.click(app.dlg.getByRole("button", { name: "Darstellung" }));
  expect(search).toHaveValue("");
  expect(app.dlg.getByRole("tab", { name: "Darstellung geändert" })).toHaveAttribute("aria-selected", "true");
  await app.user.type(search, "Avahi");
  expect(app.dlg.getByRole("switch", { name: "Dienste im lokalen Netzwerk suchen" })).toBeInTheDocument();
  expect(app.dialog().querySelector("mark")).toHaveTextContent("Avahi");
  await app.user.clear(search);
  await app.user.type(search, "Schriftgrosse");
  expect(app.dlg.getByRole("slider", { name: "Schriftgröße" })).toBeInTheDocument();
  await app.user.clear(search);
  await app.user.type(search, "no-such-setting");
  expect(app.dlg.getByRole("status")).toHaveTextContent("Nichts gefunden");
});

test("modified values and keys share the counter, combine with search and reset individually", async () => {
  const app = await setup({ fontSize: 19, sidebarCollapsed: ["devices"], paneColorLeft: "#123456", keybindings: { copy: "Alt+C" } });
  await app.user.click(app.dlg.getByRole("button", { name: "Geändert (4)" }));
  expect(app.dialog().querySelectorAll("[data-setting]")).toHaveLength(3);
  expect(app.dlg.getByText("Kopieren", { selector: ".keys-name" })).toBeInTheDocument();
  await app.user.type(app.dlg.getByRole("searchbox"), "Alt+C");
  expect(app.dialog().querySelectorAll(".keys-row")).toHaveLength(1);
  expect(app.dialog().querySelector("mark")).toHaveTextContent("Alt+C");
  await app.user.click(app.dlg.getByRole("button", { name: /^Zurücksetzen$/ }));
  expect(app.state().keybindings?.copy).toBeUndefined();
  expect(app.dlg.getByRole("button", { name: "Geändert (3)" })).toBeInTheDocument();
  await app.user.clear(app.dlg.getByRole("searchbox"));
  await app.user.click(app.dlg.getByRole("button", { name: "Zurücksetzen: Schriftgröße" }));
  expect(app.state().fontSize).toBe(defaultSettings.fontSize);
  await app.user.click(app.dlg.getByRole("button", { name: "Zurücksetzen: Angezeigte Bereiche" }));
  expect(app.state().sidebarCollapsed).toEqual([]);
  await app.user.click(app.dlg.getByRole("button", { name: "Zurücksetzen: Linkes Fenster" }));
  expect(app.state().paneColorLeft).toBe("");
  expect(app.dlg.getByRole("status")).toHaveTextContent("Alles steht auf Standard.");
});

test("reset everything needs two clicks and expires, preserving unrelated preferences", async () => {
  const app = await setup({ fontSize: 19, keybindings: { copy: "Alt+C" }, terminalHeight: 350 });
  vi.useFakeTimers();
  fireEvent.click(app.dlg.getByRole("button", { name: "Alles auf Standard …" }));
  expect(app.state().fontSize).toBe(19);
  act(() => vi.advanceTimersByTime(3500));
  expect(app.dlg.queryByRole("button", { name: "Wirklich alles zurücksetzen?" })).toBeNull();
  fireEvent.click(app.dlg.getByRole("button", { name: "Alles auf Standard …" }));
  fireEvent.click(app.dlg.getByRole("button", { name: "Wirklich alles zurücksetzen?" }));
  expect(app.state().fontSize).toBe(defaultSettings.fontSize);
  expect(app.state().keybindings).toEqual({});
  expect(app.state().terminalHeight).toBe(350);
  expect(app.dlg.getByRole("button", { name: "Geändert (0)" })).toBeInTheDocument();
});

test("Ctrl+F, Escape and roving section navigation", async () => {
  const app = await setup();
  const tab = app.dlg.getByRole("tab", { name: "Darstellung" });
  tab.focus();
  await app.user.keyboard("{Home}");
  expect(app.dlg.getByRole("tab", { name: "Allgemein" })).toHaveFocus();
  await app.user.keyboard("{ArrowDown}");
  expect(tab).toHaveFocus();
  await app.user.keyboard("{End}");
  expect(app.dlg.getByRole("tab", { name: "Info & Hilfe" })).toHaveFocus();
  await app.user.keyboard("{ArrowUp}");
  expect(app.dlg.getByRole("tab", { name: "Tastatur" })).toHaveFocus();
  await app.user.tab();
  expect(app.dlg.getByRole("tabpanel")).toHaveFocus();
  await app.user.keyboard("{Control>}f{/Control}");
  expect(app.dlg.getByRole("searchbox")).toHaveFocus();
  await app.user.keyboard("xyz{Escape}");
  expect(app.dlg.getByRole("searchbox")).toHaveValue("");
  await app.user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("preview tracks real design, accent, density, font, tint and extra columns", async () => {
  const app = await setup();
  const preview = () => app.dialog().querySelector<HTMLElement>(".settings-preview")!;
  expect(preview()).toHaveAttribute("aria-hidden", "true");
  await app.user.click(app.dlg.getByRole("radio", { name: "Weiß" }));
  expect(preview()).toHaveAttribute("data-fm", "weiss");
  expect(preview().style.getPropertyValue("--panel")).toBe("#ffffff");
  const preset = accentPresetsForTheme(app.state().theme)[1]!;
  await app.user.click(app.dlg.getByRole("button", { name: preset }));
  expect(preview().style.getPropertyValue("--accent")).toBe(preset);
  await app.user.click(app.dlg.getByRole("radio", { name: "Kompakt" }));
  expect(preview().style.getPropertyValue("--rowh")).toBe("22px");
  fireEvent.change(app.dlg.getByRole("slider", { name: "Schriftgröße" }), { target: { value: "20" } });
  expect(preview().style.getPropertyValue("--fs")).toBe("20px");
  await app.user.click(app.dlg.getByRole("tab", { name: "Dateiliste" }));
  const body = () => preview().querySelector<HTMLElement>(".list-body")!;
  const head = () => preview().querySelector<HTMLElement>(".head")!;
  expect(app.dlg.getByText("Erst eine Fensterfarbe wählen.")).toBeInTheDocument();
  expect(app.dialog().querySelectorAll(".color-box.none")).toHaveLength(2);
  fireEvent.change(app.dlg.getByLabelText("Linkes Fenster"), { target: { value: "#004080" } });
  expect(body().style.getPropertyValue("--panel")).toBe("#004080");
  expect(head().style.getPropertyValue("--panel")).toBe("");
  fireEvent.change(app.dlg.getByRole("slider", { name: "Deckung der Fensterfarbe" }), { target: { value: "80" } });
  expect(body().style.getPropertyValue("--panel")).not.toBe("#004080");
  await app.user.click(app.dlg.getByRole("checkbox", { name: "Besitzer (uid) anzeigen" }));
  expect(preview().querySelector(".list")).toHaveClass("owner-columns");
  // The actual list's container queries hide metadata by data-column.
  expect(head().querySelector('[data-column="modified"]')).toBeInTheDocument();
  expect(body().querySelectorAll('[data-column="modified"]')).toHaveLength(3);
  expect(body().querySelectorAll('[data-column="type"]')).toHaveLength(3);
  expect(body().querySelectorAll(".owner")).toHaveLength(3);
  await app.user.click(app.dlg.getByRole("switch", { name: "Dateiendung in eigener Spalte" }));
  expect(head()).toHaveClass("ext-split");
});

test("missing shell is login, accent accepts bare hex, sidebar slider is bounded and hidden state explicit", async () => {
  const app = await setup({ terminalShell: undefined });
  fireEvent.change(app.dlg.getByLabelText("Eigener Farbwert (#rrggbb)"), { target: { value: "112233" } });
  expect(app.state()).toMatchObject({ accent: "#112233", accentAutomatic: false });
  await app.user.click(app.dlg.getByRole("tab", { name: "Terminal" }));
  expect(app.dlg.getByRole("combobox", { name: "Terminal-Shell" })).toHaveValue("login");
  await app.user.click(app.dlg.getByRole("tab", { name: "Seitenleiste" }));
  const width = app.dlg.getByRole("slider", { name: "Breite der Seitenleiste" });
  expect(width).toHaveAttribute("min", "160"); expect(width).toHaveAttribute("max", "400"); expect(width).toHaveAttribute("step", "1");
  expect(app.dlg.getByText("Nichts ausgeblendet")).toBeInTheDocument();
});

test.each([["de", "https://wiki.dualfileapp.com/de/"], ["en", "https://wiki.dualfileapp.com/"], ["auto", "https://wiki.dualfileapp.com/de/"]] as const)("wiki resolves %s and info deep link", async (language, url) => {
  const app = await setup({ language });
  const openUrl = vi.spyOn(app.api, "openUrl");
  await app.user.type(app.dlg.getByRole("searchbox"), "no-such-setting");
  act(() => app.runtime.controller.openDialog({ kind: "settings", tab: "info" }));
  const name = language === "en" ? "Open manual (wiki)" : "Handbuch (Wiki) öffnen";
  await app.user.click(app.dlg.getByRole("button", { name }));
  expect(openUrl).toHaveBeenCalledWith(url);
});

test.each([["appearance", "darstellung"], ["system", "allgemein"], ["keyboard", "tastatur"], ["info", "info"], ["terminal", "terminal"]])("window query %s", (tab, expected) => {
  expect(parseSettingsParams(`?tab=${tab}`).tab).toBe(expected);
});

test("all changed control types can be reset while search remains usable", async () => {
  const app = await setup({ showParentEntry: false, density: "compact", terminalShell: "bash", showPermissionsColumn: true });
  await app.user.click(app.dlg.getByRole("button", { name: "Geändert (4)" }));
  for (const label of ["Eintrag „..“ anzeigen", "Zeilenhöhe", "Terminal-Shell", "Rechte anzeigen"]) {
    await app.user.click(app.dlg.getByRole("button", { name: `Zurücksetzen: ${label}` }));
  }
  expect(app.dlg.getByRole("status")).toHaveTextContent("Alles steht auf Standard.");
});

test("search includes the additional-column group and highlights sidebar options and actions", async () => {
  const app = await setup();
  const search = app.dlg.getByRole("searchbox");
  await app.user.type(search, "Zusatzliche Spalten");
  expect(app.dlg.getAllByRole("checkbox")).toHaveLength(3);
  await app.user.clear(search);
  await app.user.type(search, "Verbindungen");
  expect(app.dlg.getByRole("checkbox", { name: "Verbindungen" }).closest("label")?.querySelector("mark")).toHaveTextContent("Verbindungen");
  await app.user.clear(search);
  await app.user.type(search, "Handbuch");
  expect(app.dlg.getByRole("button", { name: "Handbuch (Wiki) öffnen" }).querySelector("mark")).toHaveTextContent("Handbuch");
  await app.user.clear(search);
  await app.user.type(search, "Start");
  expect(app.dlg.getByRole("combobox", { name: "Mit dem System starten" })).toBeInTheDocument();
});

test("English search uses translated options and help", async () => {
  const app = await setup({ language: "en" });
  await app.user.type(app.dlg.getByRole("searchbox"), "Paper");
  expect(app.dlg.getByRole("radio", { name: "Paper" }).querySelector("mark")).toHaveTextContent("Paper");
  await app.user.clear(app.dlg.getByRole("searchbox"));
  await app.user.type(app.dlg.getByRole("searchbox"), "Avahi");
  expect(app.dlg.getByRole("switch")).toBeInTheDocument();
});


test("accent counts once in filter, footer and accessible tab, and resets together", async () => {
  const app = await setup({ accentAutomatic: false, accent: "#123456" });
  expect(app.dlg.getByRole("tab", { name: "Darstellung geändert" })).toBeInTheDocument();
  expect(app.dialog().querySelector(".settings-footer")).toHaveTextContent("1 Werte");
  await app.user.click(app.dlg.getByRole("button", { name: "Geändert (1)" }));
  expect(app.dialog().querySelectorAll("[data-setting]")).toHaveLength(1);
  await app.user.click(app.dlg.getByRole("button", { name: "Zurücksetzen: Akzentfarbe" }));
  expect(app.state()).toMatchObject({ accentAutomatic: true, accent: defaultSettings.accent });
  expect(app.dlg.getByRole("button", { name: "Geändert (0)" })).toBeInTheDocument();
});

test("segment row is named by its visible label", async () => {
  const app = await setup();
  const group = app.dlg.getByRole("radiogroup", { name: "Zeilenhöhe" });
  expect(group).toHaveAttribute("aria-labelledby", "label-density");
  expect(within(group).getByRole("radio", { name: "Normal" })).toBeInTheDocument();
});

test("dragged sidebar width is rounded and the slider preserves every pixel", async () => {
  const app = await setup();
  act(() => app.runtime.controller.setSidebarWidth(237.4));
  await app.user.click(app.dlg.getByRole("tab", { name: /Seitenleiste/ }));
  const slider = app.dlg.getByRole("slider", { name: "Breite der Seitenleiste" });
  expect(slider).toHaveValue("237");
  expect(slider).toHaveAttribute("step", "1");
  expect(app.dlg.getByText("237 px")).toBeInTheDocument();
  fireEvent.change(slider, { target: { value: "238" } });
  expect(app.state().sidebarWidth).toBe(238);
});


test("automatic accent and hidden terminal colours leave counters and tabs at defaults", async () => {
  const app = await setup({ accent: "#123456", terminalBg: "#000000", terminalFg: "#ffffff" });
  expect(app.dlg.getByRole("tab", { name: "Darstellung" })).toBeInTheDocument();
  expect(app.dlg.getByRole("tab", { name: "Terminal" })).toBeInTheDocument();
  await app.user.click(app.dlg.getByRole("button", { name: "Geändert (0)" }));
  expect(app.dlg.getByRole("status")).toHaveTextContent("Alles steht auf Standard.");
  await app.user.click(app.dlg.getByRole("tab", { name: "Terminal 0" }));
  await app.user.click(app.dlg.getByRole("button", { name: /Eigene/ }));
  expect(app.dlg.getByRole("button", { name: "Geändert (3)" })).toBeInTheDocument();
  await app.user.click(app.dlg.getByRole("button", { name: "Geändert (3)" }));
  expect(app.dialog().querySelectorAll("[data-setting]")).toHaveLength(3);
});
