import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";

const path = (app: Awaited<ReturnType<typeof setupApp>>, w: 0 | 1) => {
  const window = app.runtime.store.getState().windows[w];
  return window.tabs[window.activeTab]?.path;
};

describe("Anzeige und Navigation", () => {
  test("zeigt beide Fenster: „..“ zuerst, dann Ordner, dann Dateien, versteckte nicht", async () => {
    const app = await setupApp();
    expect(app.names(0).slice(0, 6)).toEqual(["..", "Bilder", "Dokumente", "Downloads", "Musik", "Projekte"]);
    expect(app.names(0)).not.toContain(".bashrc");
    expect(app.names(1)[0]).toBe("..");
    expect(app.names(1)).toContain("Datei10.txt");
    expect(path(app, 0)).toBe(HOME);
    expect(path(app, 1)).toBe(DOWNLOADS);
    expect(screen.getByRole("main", { name: "Dualfile" })).toBeInTheDocument();
  });

  test("Tastatur: Pfeile, Pos1/Ende, Enter betritt, Rücktaste geht hoch und fokussiert den Herkunftsordner", async () => {
    const app = await setupApp();
    expect(app.focused(0)).toBe("..");
    await app.user.keyboard("{ArrowDown}{ArrowDown}");
    expect(app.focused(0)).toBe("Dokumente");
    await app.user.keyboard("{End}");
    expect(app.focused(0)).toBe("start.sh");
    await app.user.keyboard("{Home}{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
    expect(app.names(0)).toContain("notizen.txt");
    expect(app.focused(0)).toBe("..");
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(path(app, 0)).toBe(HOME));
    expect(app.focused(0)).toBe("Dokumente");
  });

  test("Aktives Fenster hat den Fokus; Tab wechselt das Fenster, Pfeile wirken im neuen", async () => {
    const app = await setupApp();
    expect(app.pane(0)).toHaveClass("active");
    expect(within(app.pane(0)).getByRole("grid")).toHaveFocus();
    await app.user.keyboard("{Tab}");
    expect(app.pane(1)).toHaveClass("active");
    expect(within(app.pane(1)).getByRole("grid")).toHaveFocus();
    await app.user.keyboard("{ArrowDown}");
    expect(app.focused(1)).toBe("anleitung.pdf");
    expect(app.focused(0)).toBe("..");
  });

  test("Maus: Klick markiert, Strg-Klick ergänzt, Umschalt-Klick markiert den Bereich, Doppelklick öffnet", async () => {
    const app = await setupApp();
    await app.user.click(app.row(0, "Bilder"));
    expect(app.selected(0)).toEqual(["Bilder"]);
    await app.user.keyboard("{Control>}");
    await app.user.click(app.row(0, "Dokumente"));
    await app.user.keyboard("{/Control}");
    expect(app.selected(0)).toEqual(["Bilder", "Dokumente"]);
    await app.user.keyboard("{Shift>}");
    await app.user.click(app.row(0, "Musik"));
    await app.user.keyboard("{/Shift}");
    expect(app.selected(0)).toEqual(["Bilder", "Dokumente", "Downloads", "Musik"]);
    await app.user.click(app.row(0, "Projekte"));
    expect(app.selected(0)).toEqual(["Projekte"]);
    await app.user.dblClick(app.row(0, "Dokumente"));
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
  });

  test("Klick in ein Fenster macht es aktiv", async () => {
    const app = await setupApp();
    await app.user.click(app.row(1, "Datei2.txt"));
    expect(app.pane(1)).toHaveClass("active");
    expect(app.pane(0)).not.toHaveClass("active");
    expect(app.selected(1)).toEqual(["Datei2.txt"]);
  });

  test("Einfg/Leertaste markiert und rückt vor; + - * und Strg+A", async () => {
    const app = await setupApp({ right: DOWNLOADS });
    await app.user.keyboard("{Tab}{ArrowDown}");
    await app.user.keyboard("{Insert}{Insert}");
    expect(app.selected(1)).toEqual(["anleitung.pdf", "Datei1.txt"]);
    expect(app.focused(1)).toBe("Datei2.txt");
    await app.user.keyboard("*");
    expect(app.selected(1)).toHaveLength(6);
    expect(app.selected(1)).not.toContain("Datei1.txt");
    expect(app.selected(1)).toContain("Datei2.txt");
    await app.user.keyboard("{Control>}a{/Control}");
    expect(app.selected(1)).toHaveLength(8);
    await app.user.keyboard("-");
    const dlg = app.dialog();
    await app.user.clear(within(dlg).getByRole("textbox"));
    await app.user.type(within(dlg).getByRole("textbox"), "*.jpg{Enter}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.selected(1)).toHaveLength(6);
    expect(app.selected(1)).not.toContain("foto-0412.jpg");
  });

  test("Sortierung per Klick auf die Kopfzeile: Größe auf- und absteigend, Name natürlich", async () => {
    const app = await setupApp();
    // natürliche Reihenfolge ohne Rücksicht auf Groß-/Kleinschreibung: Datei2 vor Datei10
    expect(app.names(1).slice(1, 5)).toEqual(["anleitung.pdf", "Datei1.txt", "Datei2.txt", "Datei10.txt"]);
    const header = within(app.pane(1));
    await app.user.click(header.getByRole("button", { name: "Größe" }));
    expect(header.getByRole("columnheader", { name: /Größe/ })).toHaveAttribute("aria-sort", "ascending");
    expect(app.names(1).slice(1, 4)).toEqual(["Datei1.txt", "Datei2.txt", "Datei10.txt"]);
    expect(app.names(1).at(-1)).toBe("setup.AppImage");
    await app.user.click(header.getByRole("button", { name: "Größe" }));
    expect(header.getByRole("columnheader", { name: /Größe/ })).toHaveAttribute("aria-sort", "descending");
    expect(app.names(1)[1]).toBe("setup.AppImage");
    // Ordner bleiben im anderen Fenster oben, auch bei absteigender Größe
    await app.user.click(within(app.pane(0)).getByRole("button", { name: "Größe" }));
    await app.user.click(within(app.pane(0)).getByRole("button", { name: "Größe" }));
    expect(app.names(0)[1]).toBe("Bilder");
  });

  test("Filterfeld schränkt ein, Esc leert es und gibt den Fokus an die Liste zurück", async () => {
    const app = await setupApp();
    const filter = within(app.pane(1)).getByRole("searchbox");
    await app.user.click(filter);
    await app.user.type(filter, "foto");
    expect(app.names(1)).toEqual(["..", "foto-0412.jpg", "foto-0413.jpg"]);
    await app.user.keyboard("{Escape}");
    expect(filter).toHaveValue("");
    expect(app.names(1)).toHaveLength(9);
    expect(within(app.pane(1)).getByRole("grid")).toHaveFocus();
  });

  test("Tippen zum Suchen springt zum passenden Eintrag", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Tab}");
    await app.user.keyboard("r");
    expect(app.focused(1)).toBe("rezept-paket.tar.gz");
    await app.user.keyboard("{Escape}");
    await app.user.keyboard("s");
    expect(app.focused(1)).toBe("setup.AppImage");
  });

  test("Strg+H blendet versteckte Dateien ein und wieder aus", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Control>}h{/Control}");
    expect(app.names(0)).toContain(".bashrc");
    expect(within(app.pane(0)).getByText("Versteckte Dateien sichtbar")).toBeInTheDocument();
    await app.user.keyboard("{Control>}h{/Control}");
    expect(app.names(0)).not.toContain(".bashrc");
  });

  test("Strg+U tauscht die Fenster", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Control>}u{/Control}");
    expect(path(app, 0)).toBe(DOWNLOADS);
    expect(app.names(0)).toContain("Datei10.txt");
    expect(app.names(1)).toContain("Dokumente");
  });

  test("Tabs: Strg+T kopiert, Strg+Tab wechselt, Strg+W schließt, der letzte bleibt", async () => {
    const app = await setupApp();
    await app.user.keyboard("{Control>}t{/Control}");
    expect(within(app.pane(0)).getAllByRole("tab")).toHaveLength(2);
    await app.user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
    await app.user.keyboard("{Control>}{Tab}{/Control}");
    await waitFor(() => expect(path(app, 0)).toBe(HOME));
    expect(within(app.pane(0)).getByRole("tab", { selected: true })).toHaveTextContent("demo");
    await app.user.keyboard("{Control>}{Tab}{/Control}");
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
    await app.user.keyboard("{Control>}w{/Control}");
    expect(within(app.pane(0)).getAllByRole("tab")).toHaveLength(1);
    await app.user.keyboard("{Control>}w{/Control}");
    expect(within(app.pane(0)).getAllByRole("tab")).toHaveLength(1);
  });

  test("Tabs per Maus: + öffnet, × schließt, Klick wechselt", async () => {
    const app = await setupApp();
    const pane = within(app.pane(0));
    await app.user.click(pane.getByRole("button", { name: "Neuer Tab" }));
    expect(pane.getAllByRole("tab")).toHaveLength(2);
    await app.user.click(pane.getAllByRole("tab")[0]!);
    expect(pane.getAllByRole("tab")[0]).toHaveAttribute("aria-selected", "true");
    await app.user.click(pane.getAllByRole("button", { name: /Tab schließen/ })[1]!);
    expect(pane.getAllByRole("tab")).toHaveLength(1);
  });

  test("Pfadzeile: Teile sind anklickbar, Strg+L öffnet die Eingabe, ungültiger Pfad bleibt Fehlermeldung", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(within(app.pane(0)).getByRole("button", { name: "demo" }));
    await waitFor(() => expect(path(app, 0)).toBe(HOME));
    await app.user.keyboard("{Control>}l{/Control}");
    const input = within(app.pane(0)).getByRole("textbox", { name: "Pfad" });
    expect(input).toHaveFocus();
    expect(input).toHaveValue(HOME);
    await app.user.clear(input);
    await app.user.type(input, "/nirgendwo{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Nicht gefunden: /nirgendwo");
    expect(path(app, 0)).toBe(HOME);
    await app.user.clear(input);
    await app.user.type(input, `${HOME}/Projekte{Enter}`);
    await waitFor(() => expect(path(app, 0)).toBe(`${HOME}/Projekte`));
    expect(within(app.pane(0)).queryByRole("textbox", { name: "Pfad" })).toBeNull();
    await app.user.keyboard("{Control>}l{/Control}{Escape}");
    expect(within(app.pane(0)).queryByRole("textbox", { name: "Pfad" })).toBeNull();
  });

  test("Statuszeile zeigt „x von y markiert“ und die Gesamtgröße", async () => {
    const app = await setupApp();
    const pane = within(app.pane(1));
    expect(pane.getByText("0 von 8 markiert")).toBeInTheDocument();
    await app.user.click(app.row(1, "Datei1.txt"));
    await app.user.keyboard("{Control>}");
    await app.user.click(app.row(1, "Datei2.txt"));
    await app.user.keyboard("{/Control}");
    expect(pane.getByText("2 von 8 markiert")).toBeInTheDocument();
    expect(pane.getByText("Markiert: 33 B")).toBeInTheDocument();
  });

  test("externe Änderung lädt neu, die Markierung bleibt erhalten", async () => {
    const app = await setupApp();
    await app.user.click(app.row(0, "Bilder"));
    await app.base.makeDir(HOME, "Extern");
    await waitFor(() => expect(app.names(0)).toContain("Extern"));
    expect(app.selected(0)).toEqual(["Bilder"]);
    expect(app.focused(0)).toBe("Bilder");
  });

  test("Ordner, der verschwindet, schickt das Fenster in die nächste vorhandene Ebene", async () => {
    const app = await setupApp({ left: `${HOME}/Musik` });
    await app.base.trash([`${HOME}/Musik`]);
    await waitFor(() => expect(path(app, 0)).toBe(HOME));
  });

  test("F3 öffnet den Lister, Esc schließt ihn, F4 öffnet extern", async () => {
    const calls: string[] = [];
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({ ...api, openExternal: async (p: string) => void calls.push(p) }),
    });
    await app.user.keyboard("{ArrowDown}{F3}");
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await app.user.keyboard("{F4}");
    await waitFor(() => expect(calls.length).toBe(1));
    expect(calls[0]).toMatch(/^\/home\/demo\/Dokumente\//);
    // Enter auf einer Datei öffnet ebenfalls extern, auf „..“ geht es hoch
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(calls.length).toBe(2));
  });

  test("F-Tasten-Leiste ist klickbar", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.click(screen.getByRole("button", { name: /F5/ }));
    expect(app.dialog()).toHaveAccessibleName("Kopieren");
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("5000 Einträge bleiben bedienbar", async () => {
    const entries = Array.from({ length: 5000 }, (_, i) => ({
      name: `datei-${String(i).padStart(4, "0")}.txt`,
      kind: "file" as const,
      size: i,
      modified: 0,
      mode: 0o644,
      hidden: false,
      linkTarget: null,
      brokenLink: false,
      error: null,
    }));
    const app = await setupApp({
      wrap: (api) => ({ ...api, list: async (p: string) => (p === HOME ? { path: HOME, entries } : api.list(p)) }),
    });
    // Since #63 only a window of rows is in the DOM; the grid still announces all of them.
    expect(within(app.pane(0)).getByRole("grid")).toHaveAttribute("aria-rowcount", "5002");
    expect(app.rows(0).length).toBeLessThan(300);
    await app.user.keyboard("{End}");
    expect(app.focused(0)).toBe("datei-4999.txt");
    await app.user.keyboard("{Control>}a{/Control}");
    expect(within(app.pane(0)).getByText("5000 von 5000 markiert")).toBeInTheDocument();
    fireEvent.change(within(app.pane(0)).getByRole("searchbox"), { target: { value: "4999" } });
    expect(app.names(0)).toEqual(["..", "datei-4999.txt"]);
  }, 30_000);
});

describe("Ordnerbeobachtung", () => {
  test("meldet dem Kern die Ordner beider Fenster beim Start, bei Navigation und Tabwechsel", async () => {
    const calls: string[][] = [];
    const app = await setupApp({
      wrap: (api) => ({
        ...api,
        watch: async (paths) => {
          calls.push(paths);
        },
      }),
    });
    expect(calls).toEqual([[HOME, DOWNLOADS]]);

    await app.user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
    expect(calls.at(-1)).toEqual([DOCS, DOWNLOADS]);

    // Zweiter Reiter im aktiven Fenster: der neue Reiter zeigt denselben Ordner, nichts ändert sich.
    const before = calls.length;
    await app.user.keyboard("{Control>}t{/Control}");
    expect(calls.length).toBe(before);

    // Beide Fenster im selben Ordner: nur ein Eintrag, keine Doppelten.
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(path(app, 0)).toBe(HOME));
    await app.user.keyboard("{Tab}{Backspace}");
    await waitFor(() => expect(path(app, 1)).toBe(HOME));
    expect(calls.at(-1)).toEqual([HOME]);
  });

  test("ein Fehler der Beobachtung bleibt unbemerkt (keine Meldung, Oberfläche läuft weiter)", async () => {
    const app = await setupApp({
      wrap: (api) => ({ ...api, watch: async () => Promise.reject(new Error("kein inotify")) }),
    });
    await app.user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    await waitFor(() => expect(path(app, 0)).toBe(DOCS));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
