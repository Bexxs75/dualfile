import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import type { Timers } from "./app/controller";
import type { LoadOutcome, Settings } from "./ipc/types";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";

const PROJEKTE = `${HOME}/Projekte`;
const bookmarks = [
  { name: "Downloads", path: DOWNLOADS, color: "#6fb3d9", order: 0 },
  { name: "Projekte", path: PROJEKTE, color: "#7bc08a", order: 1 },
];
const pathOf = (app: Awaited<ReturnType<typeof setupApp>>, w: 0 | 1) => {
  const window = app.runtime.store.getState().windows[w];
  return window.tabs[window.activeTab]?.path;
};
const root = () => document.documentElement;

describe("Lesezeichen", () => {
  test("Klick wechselt den Ordner des aktiven Fensters, das andere bleibt (Spiegeln aus)", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS, settings: { bookmarks, sidebarMode: "hidden" } });
    const ribbon = within(screen.getByRole("toolbar", { name: "Lesezeichen" }));
    expect(ribbon.getByRole("button", { name: /Downloads/ })).toHaveTextContent("Alt+1");
    await app.user.click(ribbon.getByRole("button", { name: /Projekte/ }));
    await waitFor(() => expect(pathOf(app, 0)).toBe(PROJEKTE));
    expect(pathOf(app, 1)).toBe(DOWNLOADS);
  });

  test("Alt+1…9 öffnet Lesezeichen im aktiven Fenster", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS, settings: { bookmarks, sidebarMode: "hidden" } });
    await app.user.keyboard("{Tab}{Alt>}2{/Alt}");
    await waitFor(() => expect(pathOf(app, 1)).toBe(PROJEKTE));
    expect(pathOf(app, 0)).toBe(DOCS);
    await app.user.keyboard("{Alt>}9{/Alt}"); // gibt es nicht: nichts passiert
    expect(pathOf(app, 1)).toBe(PROJEKTE);
  });

  test("anheften und lösen; Strg+D schaltet ebenfalls um", async () => {
    const app = await setupApp({ left: DOCS, settings: { sidebarMode: "hidden" } });
    const ribbon = () => within(screen.getByRole("toolbar", { name: "Lesezeichen" }));
    await app.user.click(within(app.pane(0)).getByRole("button", { name: "Ordner merken …" }));
    expect(ribbon().getByRole("button", { name: /^Dokumente/ })).toBeInTheDocument();
    expect(within(app.pane(0)).getByRole("button", { name: "Lesezeichen lösen" })).toHaveAttribute("aria-pressed", "true");
    await app.user.keyboard("{Control>}d{/Control}");
    expect(screen.queryByRole("toolbar", { name: "Lesezeichen" })).toBeNull();
    await app.user.keyboard("{Control>}d{/Control}");
    expect(ribbon().getByRole("button", { name: /^Dokumente/ })).toBeInTheDocument();
  });

  test("Rechtsklick: Name und Farbe ändern, Lesezeichen lösen", async () => {
    const app = await setupApp({ settings: { bookmarks, sidebarMode: "hidden" } });
    const ribbon = () => within(screen.getByRole("toolbar", { name: "Lesezeichen" }));
    await app.user.pointer({ keys: "[MouseRight]", target: ribbon().getByRole("button", { name: /^Projekte/ }) });
    const dlg = app.dialog();
    const name = within(dlg).getByLabelText("Name");
    await app.user.clear(name);
    await app.user.type(name, "Arbeit");
    const hex = within(dlg).getByLabelText("Farbwert");
    await app.user.clear(hex);
    await app.user.type(hex, "nichtfarbe");
    await app.user.click(within(dlg).getByRole("button", { name: "OK" }));
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Ungültige Farbe");
    await app.user.clear(hex);
    await app.user.type(hex, "#112233");
    await app.user.click(within(dlg).getByRole("button", { name: "OK" }));
    const button = await ribbon().findByRole("button", { name: /^Arbeit/ });
    expect(button.style.getPropertyValue("--c")).toBe("#112233");
    await app.user.pointer({ keys: "[MouseRight]", target: button });
    await app.user.click(within(app.dialog()).getByRole("button", { name: "Lesezeichen lösen" }));
    expect(ribbon().queryByRole("button", { name: /^Arbeit/ })).toBeNull();
  });

  test("Reihenfolge per Ziehen", async () => {
    const app = await setupApp({ settings: { bookmarks, sidebarMode: "hidden" } });
    const ribbon = () => within(screen.getByRole("toolbar", { name: "Lesezeichen" }));
    const first = ribbon().getByRole("button", { name: /^Downloads/ });
    const second = ribbon().getByRole("button", { name: /^Projekte/ });
    fireEvent.dragStart(first);
    fireEvent.dragOver(second);
    fireEvent.drop(second);
    expect(app.runtime.store.getState().settings.bookmarks.map((b) => b.name)).toEqual(["Projekte", "Downloads"]);
    expect(ribbon().getByRole("button", { name: /^Projekte/ })).toHaveTextContent("Alt+1");
  });
});

describe("Einstellungen", () => {
  test("Themewechsel setzt Attribut und CSS-Variablen sofort", async () => {
    const app = await setupApp();
    expect(root().dataset.fm).toBe("schiefer");
    expect(root().style.getPropertyValue("--bg")).toBe("#161a1f");
    await app.user.keyboard("{Control>},{/Control}");
    const dlg = app.dialog();
    expect(dlg).toHaveAccessibleName("Einstellungen");
    await app.user.click(within(dlg).getByRole("radio", { name: "Papier" }));
    expect(root().dataset.fm).toBe("papier");
    expect(root().style.getPropertyValue("--bg")).toBe("#ece8df");
    expect(root().style.colorScheme).toBe("light");
    await app.user.click(within(dlg).getByRole("radio", { name: "Werkstatt" }));
    expect(root().style.getPropertyValue("--panel")).toBe("#241e18");
  });

  test("Dichte und Schriftgröße (waagerechter Schieber) wirken sofort", async () => {
    const app = await setupApp();
    await app.user.click(screen.getByRole("button", { name: /Einstellungen/ }));
    const dlg = app.dialog();
    await app.user.click(within(dlg).getByRole("radio", { name: "Luftig" }));
    expect(root().style.getPropertyValue("--rowh")).toBe("36px");
    await app.user.click(within(dlg).getByRole("radio", { name: "Kompakt" }));
    expect(root().style.getPropertyValue("--rowh")).toBe("22px");
    const slider = within(dlg).getByRole("slider", { name: "Schriftgröße" });
    expect(slider).toHaveAttribute("type", "range");
    fireEvent.change(slider, { target: { value: "18" } });
    expect(root().style.getPropertyValue("--fs")).toBe("18px");
    expect(within(dlg).getByText("18 px")).toBeInTheDocument();
  });

  test("Akzent: Vorgabe, Hex und Kontrastwarnung", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Control>},{/Control}");
    const dlg = app.dialog();
    await app.user.click(within(dlg).getByRole("button", { name: "#6fb3d9" }));
    expect(root().style.getPropertyValue("--accent")).toBe("#6fb3d9");
    const hex = within(dlg).getByLabelText("Eigener Farbwert (#rrggbb)");
    await app.user.clear(hex);
    await app.user.type(hex, "#12");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Kein gültiger Farbwert");
    expect(root().style.getPropertyValue("--accent")).toBe("#6fb3d9"); // Ungültiges wird nicht übernommen
    await app.user.clear(hex);
    await app.user.type(hex, "#181c22");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("hebt sich zu wenig vom Hintergrund ab");
    expect(root().style.getPropertyValue("--accent")).toBe("#181c22");
    await app.user.clear(hex);
    await app.user.type(hex, "#ff8800");
    expect(within(dlg).queryByRole("alert")).toBeNull();
  });

  test("Sprachumschaltung Deutsch/Englisch wirkt sofort in der ganzen Oberfläche", async () => {
    const app = await setupApp();
    expect(screen.getByRole("button", { name: /F5/ })).toHaveTextContent("Kopieren");
    await app.user.keyboard("{Control>},{/Control}");
    await app.user.click(within(app.dialog()).getByRole("tab", { name: "Allgemein" }));
    await app.user.click(within(app.dialog()).getByRole("radio", { name: "English" }));
    expect(root().lang).toBe("en");
    expect(screen.getByRole("dialog")).toHaveAccessibleName("Settings");
    await app.user.keyboard("{Escape}");
    expect(screen.getByRole("region", { name: "Left pane" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /F5/ })).toHaveTextContent("Copy");
    expect(within(app.pane(0)).getByRole("columnheader", { name: /Size/ })).toBeInTheDocument();
    expect(within(app.pane(0)).getByText("0 of 9 selected")).toBeInTheDocument();
    await app.user.click(app.row(0, "Bilder"));
    expect(within(app.pane(0)).getByText("1 of 9 selected")).toBeInTheDocument();
    await app.user.keyboard("{F7}");
    expect(app.dialog()).toHaveAccessibleName("New folder");
  });

  test("Sprache „Systemsprache“ folgt navigator.language", async () => {
    const spy = vi.spyOn(window.navigator, "language", "get").mockReturnValue("de-DE");
    const german = await setupApp({ language: "auto" });
    expect(screen.getByRole("region", { name: "Linkes Fenster" })).toBeInTheDocument();
    german.view.unmount();
    spy.mockReturnValue("fr-FR"); // nicht erkannt: Englisch
    await setupApp({ language: "auto" });
    expect(screen.getByRole("region", { name: "Left pane" })).toBeInTheDocument();
    spy.mockRestore();
  });
});

describe("Dialoge", () => {
  test("Fokusfalle: Tab und Umschalt+Tab bleiben im Dialog, Esc schließt und gibt den Fokus zurück", async () => {
    const app = await setupApp();
    await app.user.keyboard("{F7}");
    const dlg = app.dialog();
    for (let i = 0; i < 6; i += 1) {
      await app.user.tab();
      expect(dlg.contains(document.activeElement)).toBe(true);
    }
    for (let i = 0; i < 6; i += 1) {
      await app.user.tab({ shift: true });
      expect(dlg.contains(document.activeElement)).toBe(true);
    }
    // Von außen in den Dialog zurückgeholt: Tab am letzten Element springt an den Anfang
    const buttons = within(dlg).getAllByRole("button");
    buttons[buttons.length - 1]!.focus();
    await app.user.tab();
    expect(dlg.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(within(dlg).getAllByRole("button")[0]);
    // Tastenbefehle der Liste greifen im Dialog nicht
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(within(app.pane(0)).getByRole("grid")).toHaveFocus();
  });

  test("Listenbefehle (F5, Entf, Tab) wirken nicht, solange ein Dialog offen ist", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F2}");
    await app.user.keyboard("{F5}{Delete}");
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(app.dialog()).toHaveAccessibleName("Umbenennen");
    expect(app.runtime.store.getState().history).toHaveLength(0);
  });

  test("Eigenschaften-Dialog zeigt Größe, Rechte und Linkziel", async () => {
    const app = await setupApp();
    await app.user.click(app.row(0, "Projekte-Link"));
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "Projekte-Link") });
    await app.user.click(await screen.findByRole("menuitem", { name: "Eigenschaften" }));
    const dlg = app.dialog();
    expect(within(dlg).getByText(PROJEKTE)).toBeInTheDocument(); // Linkziel
    expect(within(dlg).getByText("777")).toBeInTheDocument();
    expect(within(dlg).getByText("Link")).toBeInTheDocument();
  });
});

describe("Speichern und Hinweise", () => {
  test("Änderungen werden entprellt gespeichert, auch Tabs und aktives Fenster", async () => {
    // Die Entprellung läuft über einen von Hand gesteuerten Zeitgeber, nicht über die Uhr: Auf einem
    // ausgelasteten Rechner feuerte der echte 20-ms-Zeitgeber schon während des Starts oder zwischen den
    // Tastenfolgen (#39, #42). Andere Zeitgeber (Toasts, Orte-Abfrage) bleiben echt.
    const saveDelayMs = 7777;
    const held = new Map<number, () => void>();
    let scheduled = 0;
    const timers: Timers = {
      set: (callback, ms) => {
        if (ms !== saveDelayMs) return setTimeout(callback, ms);
        scheduled++;
        held.set(scheduled, callback);
        return scheduled;
      },
      clear: (handle) => {
        if (typeof handle === "number") held.delete(handle);
        else clearTimeout(handle as ReturnType<typeof setTimeout>);
      },
    };
    const flush = () => act(async () => { [...held.values()].forEach((run) => run()); held.clear(); });
    const saved: Settings[] = [];
    const app = await setupApp({
      options: { saveDelayMs, timers },
      wrap: (api) => ({
        ...api,
        saveSettings: async (settings) => {
          saved.push(structuredClone(settings));
          return api.saveSettings(settings);
        },
      }),
    });
    // Der Start trägt den Zeitpunkt der Update-Prüfung ein (asynchron): erst danach ist die Startphase
    // vorbei. Das ist eine echte Änderung, aber sie wird nur geplant und nicht geschrieben.
    await waitFor(() => expect(app.runtime.store.getState().settings.lastUpdateCheck).not.toBeNull());
    expect(saved).toHaveLength(0);
    await flush();
    await waitFor(() => expect(saved).toHaveLength(1));
    held.clear();
    const before = scheduled;
    await app.user.keyboard("{Control>}t{/Control}{Tab}");
    await app.user.keyboard("{Control>}h{/Control}"); // Ctrl+H gehört nicht zu den gespeicherten Werten
    expect(saved).toHaveLength(1); // nichts wird vor Ablauf der Wartezeit geschrieben
    expect(scheduled - before).toBeGreaterThanOrEqual(2); // mehrere Änderungen ...
    expect(held.size).toBe(1); // ... aber nur ein ausstehendes Speichern
    await flush();
    await waitFor(() => expect(saved).toHaveLength(2)); // genau ein Schreibvorgang für alles zusammen
    const last = saved.at(-1)!;
    expect(last.session.windows[0]?.tabs).toHaveLength(2);
    expect(last.session.windows[1]?.tabs[0]?.path).toBe(DOWNLOADS);
    expect(last.session.activeWindow).toBe(1);
    expect((await app.api.loadSettings()).settings.session.windows[0]?.tabs).toHaveLength(2);
  });

  test("Einstellungen (Theme) landen in der gespeicherten Datei", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Control>},{/Control}");
    await app.user.click(within(app.dialog()).getByRole("radio", { name: "Papier" }));
    await waitFor(async () => expect((await app.api.loadSettings()).settings.theme).toBe("papier"));
  });

  test.each<[LoadOutcome, RegExp]>([
    [{ kind: "recoveredFromCorrupt" }, /Einstellungsdatei war beschädigt/],
    [{ kind: "newerVersion", version: 9 }, /neueren Version \(9\)/],
    [{ kind: "repaired", messages: ["Akzent ersetzt"] }, /repariert: Akzent ersetzt/],
    [{ kind: "migrated", from: 1, to: 2 }, /Version 1 auf 2/],
  ])("Hinweisbalken bei %j", async (outcome, text) => {
    const app = await setupApp({
      wrap: (api) => ({ ...api, loadSettings: async () => ({ ...(await api.loadSettings()), outcome }) }),
    });
    const notice = screen.getByRole("status", { name: "Hinweis" });
    expect(notice).toHaveTextContent(text);
    await app.user.click(within(notice).getByRole("button", { name: "Schließen" }));
    expect(screen.queryByRole("status", { name: "Hinweis" })).toBeNull();
  });

  test("kein Hinweisbalken bei fresh/loaded", async () => {
    await setupApp();
    expect(screen.queryByRole("status", { name: "Hinweis" })).toBeNull();
  });

  test("eine neuere Einstellungsdatei wird nie überschrieben", async () => {
    const saveSettings = vi.fn(async () => {});
    const app = await setupApp({
      wrap: (api) => ({
        ...api,
        saveSettings,
        loadSettings: async () => ({ ...(await api.loadSettings()), outcome: { kind: "newerVersion" as const, version: 9 } }),
      }),
    });
    await app.user.keyboard("{Control>}t{/Control}");
    await app.user.keyboard("{Control>},{/Control}");
    await app.user.click(within(app.dialog()).getByRole("radio", { name: "Papier" }));
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(saveSettings).not.toHaveBeenCalled();
  });
});

test("white design and pane opacity update live; opacity needs a pane colour", async () => {
  const app = await setupApp();
  await app.user.keyboard("{Control>},{/Control}");
  const dlg = within(app.dialog());
  await app.user.click(dlg.getByRole("radio", { name: "Weiß" }));
  expect(root().dataset.fm).toBe("weiss");
  await app.user.click(dlg.getByRole("tab", { name: "Dateiliste" }));
  const slider = dlg.getByRole("slider", { name: "Deckung der Fensterfarbe" });
  expect(slider).toBeDisabled();
  expect(slider).toHaveValue("100");
  expect(slider).toHaveAttribute("min", "80");
  expect(slider).toHaveAttribute("max", "100");
  expect(slider).toHaveAttribute("step", "5");
  fireEvent.change(dlg.getByLabelText("Linkes Fenster"), { target: { value: "#004080" } });
  expect(slider).toBeEnabled();
  expect(within(app.pane(0)).getByRole("rowgroup").style.getPropertyValue("--panel")).toBe("#004080");
  fireEvent.change(slider, { target: { value: "85" } });
  expect(app.runtime.store.getState().settings.paneTintStrength).toBe(85);
  expect(within(app.pane(0)).getByRole("rowgroup").style.getPropertyValue("--panel")).not.toBe("#004080");
  expect(slider).toHaveAttribute("aria-valuetext", "85%");
  await app.user.click(dlg.getByRole("checkbox", { name: "Rechte anzeigen" }));
  for (const w of [0, 1] as const) expect(within(app.pane(w)).getByRole("columnheader", { name: /Rechte/ })).toBeInTheDocument();
});


test("permissions, uid and gid checkboxes update independently", async () => {
  const app = await setupApp();
  await app.user.keyboard("{Control>},{/Control}");
  const dlg = within(app.dialog());
  await app.user.click(dlg.getByRole("tab", { name: "Dateiliste" }));
  for (const [label, key] of [["Rechte anzeigen", "showPermissionsColumn"], ["Besitzer (uid) anzeigen", "showUidColumn"], ["Gruppe (gid) anzeigen", "showGidColumn"]] as const) {
    const checkbox = dlg.getByRole("checkbox", { name: label });
    expect(checkbox).not.toBeChecked();
    await app.user.click(checkbox);
    expect(app.runtime.store.getState().settings[key]).toBe(true);
    await app.user.click(checkbox);
    expect(app.runtime.store.getState().settings[key]).toBe(false);
  }
  expect(app.pane(0)).not.toHaveAttribute("data-tint");
});
