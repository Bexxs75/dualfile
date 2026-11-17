import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import { CoreError, type DualfileApi } from "./ipc/api";
import { archiveRootOf, formatArchivePath } from "./state/paths";
import { DOCS, DOWNLOADS, setupApp } from "./test-utils";

const ARCHIVE = `${DOWNLOADS}/rezept-paket.tar.gz`;
const ROOT = archiveRootOf(ARCHIVE);

type App = Awaited<ReturnType<typeof setupApp>>;
const tabPath = (app: App, w: 0 | 1): string | undefined => {
  const window = app.runtime.store.getState().windows[w];
  return window.tabs[window.activeTab]?.path;
};
const alerts = () => screen.queryAllByRole("alert").map((node) => node.textContent ?? "");

/** Links liegt das Demo-Archiv im Ordner Downloads, rechts die Dokumente. */
async function inDownloads() {
  const app = await setupApp({ left: DOWNLOADS, right: DOCS });
  await app.user.click(app.row(0, "rezept-paket.tar.gz"));
  return app;
}

async function insideArchive() {
  const app = await inDownloads();
  await app.user.keyboard("{Enter}");
  await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
  await waitFor(() => expect(app.names(0)).toContain("rezepte"));
  return app;
}

describe("Archiv öffnen und verlassen", () => {
  test("Enter öffnet die Archivdatei als Ordner, Rücktaste verlässt sie und fokussiert die Datei", async () => {
    const app = await insideArchive();
    expect(app.names(0).filter((n) => n !== "..").sort()).toEqual(["README.md", "aktuell", "extras.zip", "rezepte"]);
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(DOWNLOADS));
    expect(app.focused(0)).toBe("rezept-paket.tar.gz");
  });

  test("Doppelklick öffnet es ebenso; Enter auf „..“ im Archivwurzel-Ordner verlässt es", async () => {
    const app = await setupApp({ left: DOWNLOADS, right: DOCS });
    await app.user.dblClick(app.row(0, "rezept-paket.tar.gz"));
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    await waitFor(() => expect(app.names(0)[0]).toBe(".."));
    await app.user.keyboard("{Home}{Enter}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(DOWNLOADS));
  });

  test("Ordner im Archiv betreten und wieder hinaus: Innenpfad, dann Wurzel, dann Ordner der Datei", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "rezepte"));
    await app.user.keyboard("{Enter}");
    const inner = formatArchivePath(ARCHIVE, "/rezepte");
    await waitFor(() => expect(tabPath(app, 0)).toBe(inner));
    expect(app.names(0)).toContain("pizza.txt");
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    expect(app.focused(0)).toBe("rezepte");
  });

  test("verschachtelte Archive: eine Ebene zurück je Rücktaste", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "extras.zip"));
    await app.user.keyboard("{Enter}");
    const nestedRoot = archiveRootOf(formatArchivePath(ARCHIVE, "/extras.zip"));
    await waitFor(() => expect(tabPath(app, 0)).toBe(nestedRoot));
    expect(app.names(0)).toContain("tipps.txt");
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    expect(app.focused(0)).toBe("extras.zip");
    await app.user.keyboard("{Backspace}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(DOWNLOADS));
  });

  test("Dateien ohne Archivendung werden weiter extern geöffnet, nicht als Archiv", async () => {
    const open = vi.fn(async () => undefined);
    const app = await setupApp({ left: DOWNLOADS, right: DOCS, wrap: (api) => ({ ...api, openExternal: open }) });
    await app.user.click(app.row(0, "anleitung.pdf"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(open).toHaveBeenCalledWith(`${DOWNLOADS}/anleitung.pdf`));
    expect(tabPath(app, 0)).toBe(DOWNLOADS);
  });
});

describe("Pfadzeile im Archiv", () => {
  test("Kennzeichen ARCHIV, lesbarer Pfad, nie das Rohformat", async () => {
    const app = await insideArchive();
    const pane = app.pane(0);
    expect(within(pane).getByText("ARCHIV")).toBeInTheDocument();
    const nav = within(pane).getByRole("navigation", { name: "Pfad" });
    expect(nav.textContent).not.toContain("archive:");
    expect(nav.textContent).not.toContain("!");
    expect(within(nav).getByRole("button", { name: "rezept-paket.tar.gz" })).toHaveAttribute("aria-current", "location");
    await app.user.click(app.row(0, "rezepte"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(within(pane).getByRole("button", { name: "rezepte" })).toBeInTheDocument());
    // Klick auf die Archiv-Krume geht zur Archivwurzel
    await app.user.click(within(nav).getByRole("button", { name: "rezept-paket.tar.gz" }));
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    // Tab-Titel zeigt den Archivnamen, der Tooltip den lesbaren Pfad
    expect(within(pane).getByRole("tab", { name: "rezept-paket.tar.gz" })).toHaveAttribute("title", ARCHIVE);
  });

  test("kein Kennzeichen in normalen Ordnern; das Pfadfeld zeigt im Archiv den lesbaren Pfad", async () => {
    const app = await setupApp({ left: DOWNLOADS, right: DOCS });
    expect(within(app.pane(0)).queryByText("ARCHIV")).toBeNull();
    await app.user.click(app.row(0, "rezept-paket.tar.gz"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    await app.user.keyboard("{Control>}l{/Control}");
    const input = await within(app.pane(0)).findByRole("textbox", { name: "Pfad" });
    expect(input).toHaveValue(ARCHIVE);
    await app.user.keyboard("{Enter}"); // unverändert: bleibt im Archiv
    await waitFor(() => expect(within(app.pane(0)).queryByRole("textbox", { name: "Pfad" })).toBeNull());
    expect(tabPath(app, 0)).toBe(ROOT);
  });
});

describe("Archive sind schreibgeschützt", () => {
  test("F2, F6, F7, F8 und Umschalt+F8 öffnen nichts und melden den Hinweis", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "README.md"));
    for (const key of ["{F2}", "{F6}", "{F7}", "{F8}", "{Shift>}{F8}{/Shift}", "{Control>}m{/Control}"]) {
      await app.user.keyboard(key);
      expect(screen.queryByRole("dialog"), key).toBeNull();
      expect(alerts().some((text) => text.includes("Archive sind schreibgeschützt")), key).toBe(true);
    }
    expect(app.runtime.store.getState().jobs.jobs).toHaveLength(0);
  });

  test("Schaltflächen der F-Tasten-Leiste sind ausgegraut, Kopieren und Ansehen nicht", async () => {
    await insideArchive();
    const bar = screen.getByRole("toolbar", { name: "Funktionstasten" });
    for (const name of [/^F2/, /^F6/, /^F7/, /^F8/, /^⇧F8/]) expect(within(bar).getByRole("button", { name })).toBeDisabled();
    expect(within(bar).getByRole("button", { name: /F5/ })).toBeEnabled();
    expect(within(bar).getByRole("button", { name: /F3/ })).toBeEnabled();
  });

  test("Kontextmenü: Umbenennen und Papierkorb ausgegraut, Packen und Öffnen frei", async () => {
    const app = await insideArchive();
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "README.md") });
    const menu = await screen.findByRole("menu", { name: "Kontextmenü" });
    expect(within(menu).getByRole("menuitem", { name: "Umbenennen" })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: "In den Papierkorb" })).toBeDisabled();
    expect(within(menu).queryByRole("menuitem", { name: "Mehrfach-Umbenennen" })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Öffnen" })).toBeEnabled();
    expect(within(menu).getByRole("menuitem", { name: "Packen …" })).toBeEnabled();
  });

  test("Kopieren oder Verschieben INS Archiv wird abgelehnt (kein Dialog)", async () => {
    const app = await setupApp({ left: DOCS, right: ROOT });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(alerts().some((t) => t.includes("Archive sind schreibgeschützt"))).toBe(true);
    await app.user.keyboard("{F6}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.runtime.store.getState().jobs.jobs).toHaveLength(0);
  });

  test("ein Ziel im Archiv im Kopierdialog lässt sich nicht bestätigen", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}");
    const input = within(app.dialog()).getByRole("textbox");
    await app.user.clear(input);
    await app.user.click(input);
    await app.user.paste(ROOT);
    expect(within(app.dialog()).getByRole("alert")).toHaveTextContent("Archive sind schreibgeschützt");
    expect(within(app.dialog()).getByRole("button", { name: "Kopieren" })).toBeDisabled();
  });

  test("Fehlerart readOnly vom Kern zeigt dieselbe Meldung, ohne Papierkorb-Fallback", async () => {
    const plan = vi.fn();
    const app = await setupApp({
      left: DOCS,
      right: DOWNLOADS,
      wrap: (api) => ({
        ...api,
        trash: async () => {
          throw new CoreError("Archive sind schreibgeschützt", "readOnly");
        },
        planDelete: async (...args: Parameters<DualfileApi["planDelete"]>) => {
          plan();
          return api.planDelete(...args);
        },
      }),
    });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F8}");
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(alerts().some((t) => t.includes("Archive sind schreibgeschützt"))).toBe(true));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(plan).not.toHaveBeenCalled();
  });
});

describe("Entpacken", () => {
  test("F5 mit Quelle im Archiv heißt „Entpacken“ (Dialog und Knopf); die Datei landet im Ziel", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "README.md"));
    await app.user.keyboard("{F5}");
    const dlg = app.dialog();
    expect(dlg).toHaveAccessibleName("Entpacken");
    expect(within(dlg).getByRole("textbox")).toHaveFocus();
    expect(within(dlg).getByRole("textbox")).toHaveValue(DOCS);
    await app.user.keyboard("{Enter}");
    const job = await waitFor(() => {
      const found = app.runtime.store.getState().jobs.jobs[0];
      expect(found).toBeDefined();
      return found!;
    });
    expect(job.name).toMatch(/Entpacken/);
    expect(app.runtime.controller.jobKindOf(job.id)).toBe("extract");
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).toContain("README.md"));
  });

  test("Kopieren aus einem normalen Ordner heißt weiter „Kopieren“", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}");
    expect(app.dialog()).toHaveAccessibleName("Kopieren");
  });

  test("Konflikt beim Entpacken: derselbe Konfliktdialog, nichts wird still überschrieben", async () => {
    const app = await insideArchive();
    await app.base.rename(`${DOCS}/Bericht 2026-01.txt`, "README.md"); // Ziel hat schon eine README.md
    await waitFor(() => expect(app.names(1)).toContain("README.md"));
    await app.user.click(app.row(0, "README.md"));
    await app.user.keyboard("{F5}{Enter}");
    await app.runJobs();
    await waitFor(() => expect(screen.getByRole("alertdialog")).toBeInTheDocument());
    expect((await app.api.list(DOCS)).entries.find((e) => e.name === "README.md")?.size).toBe(2_900);
  });

  test("Fehlgeschlagene Entpack-Aufträge haben eine passende Zusammenfassung", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "README.md"));
    await app.user.keyboard("{F5}");
    await app.user.keyboard("{Enter}");
    const id = app.runtime.store.getState().jobs.jobs[0]!.id;
    act(() =>
      app.runtime.store.dispatch({
        type: "job",
        event: { type: "job", status: { ...app.runtime.store.getState().jobs.jobs[0]!, id, state: "failed", failed: [{ path: "x", message: "kaputt" }] } },
      }),
    );
    expect(await screen.findByText("1 von 1 nicht entpackt")).toBeInTheDocument();
  });
});

describe("Archiv-Fehler beim Öffnen", () => {
  function failing(message: string, opened?: (path: string) => void): (api: DualfileApi) => DualfileApi {
    return (api) => ({
      ...api,
      list: async (path: string) => {
        if (path.startsWith("archive:")) throw new CoreError(message, "archive");
        return api.list(path);
      },
      openExternal: async (path: string) => opened?.(path),
    });
  }

  test("beschädigt: Hinweis mit Meldung, das Fenster bleibt im Ordner; „Extern öffnen“ öffnet die Datei", async () => {
    const opened = vi.fn();
    const app = await setupApp({ left: DOWNLOADS, right: DOCS, wrap: failing("Die Datei ist beschädigt", opened) });
    await app.user.click(app.row(0, "rezept-paket.tar.gz"));
    await app.user.keyboard("{Enter}");
    const dlg = await screen.findByRole("alertdialog");
    expect(dlg).toHaveAccessibleName("„rezept-paket.tar.gz“ lässt sich nicht öffnen");
    expect(within(dlg).getByText(/Das Archiv lässt sich nicht verwenden: Die Datei ist beschädigt/)).toBeInTheDocument();
    expect(tabPath(app, 0)).toBe(DOWNLOADS);
    await app.user.click(within(dlg).getByRole("button", { name: "Extern öffnen" }));
    expect(opened).toHaveBeenCalledWith(ARCHIVE);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  test("RAR ohne externes Programm: die verständliche Meldung des Kerns erscheint; Schließen genügt", async () => {
    const app = await setupApp({ left: DOWNLOADS, right: DOCS, wrap: failing("RAR benötigt unrar oder 7z") });
    await app.base.rename(`${DOWNLOADS}/anleitung.pdf`, "alt.rar");
    await waitFor(() => expect(app.names(0)).toContain("alt.rar"));
    await app.user.click(app.row(0, "alt.rar"));
    await app.user.keyboard("{Enter}");
    const dlg = await screen.findByRole("alertdialog");
    expect(within(dlg).getByText(/RAR benötigt unrar oder 7z/)).toBeInTheDocument();
    await app.user.click(within(dlg).getAllByRole("button", { name: "Schließen" }).at(-1)!);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(tabPath(app, 0)).toBe(DOWNLOADS);
  });

  test("anderer Fehler (z. B. Zugriff) bleibt eine Meldung statt Dialog", async () => {
    const app = await setupApp({
      left: DOWNLOADS,
      right: DOCS,
      wrap: (api) => ({
        ...api,
        list: async (path: string) => {
          if (path.startsWith("archive:")) throw new CoreError("nein", "permission");
          return api.list(path);
        },
      }),
    });
    await app.user.click(app.row(0, "rezept-paket.tar.gz"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(alerts().some((t) => t.startsWith("Zugriff verweigert"))).toBe(true));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  test("ein Archiv, das im geöffneten Zustand unlesbar wird: Meldung im Fenster mit „Archiv verlassen“", async () => {
    let broken = false;
    const app = await setupApp({
      left: DOWNLOADS,
      right: DOCS,
      wrap: (api) => ({
        ...api,
        list: async (path: string) => {
          if (broken && path.startsWith("archive:")) throw new CoreError("abgeschnitten", "archive");
          return api.list(path);
        },
      }),
    });
    await app.user.click(app.row(0, "rezept-paket.tar.gz"));
    await app.user.keyboard("{Enter}");
    await waitFor(() => expect(tabPath(app, 0)).toBe(ROOT));
    broken = true;
    await app.user.keyboard("{Control>}r{/Control}");
    await app.user.click(await within(app.pane(0)).findByRole("button", { name: "Archiv verlassen" }));
    await waitFor(() => expect(tabPath(app, 0)).toBe(DOWNLOADS));
  });
});

describe("Packen", () => {
  async function packSetup(options: Parameters<typeof setupApp>[0] = {}) {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS, ...options });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    return app;
  }

  test("Strg+Umschalt+P öffnet den Dialog: Name aus dem ersten Namen plus .zip, Ziel = Gegenfenster", async () => {
    const app = await packSetup();
    const dlg = app.dialog();
    expect(dlg).toHaveAccessibleName("Packen");
    expect(within(dlg).getByRole("textbox", { name: "Name des Archivs" })).toHaveValue("notizen.zip");
    expect(within(dlg).getByRole("combobox", { name: "Format" })).toHaveValue("zip");
    expect(within(dlg).getByText(`Ziel: ${DOWNLOADS}`)).toBeInTheDocument();
    expect(within(dlg).getByText("1 Element packen")).toBeInTheDocument();
    expect(within(dlg).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "ZIP (.zip)",
      "TAR.GZ (.tar.gz)",
      "TAR.BZ2 (.tar.bz2)",
      "TAR.XZ (.tar.xz)",
      "7Z (.7z)",
    ]);
  });

  test("Formatwechsel ändert die Endung; ein selbst getippter Name bleibt erhalten", async () => {
    const app = await packSetup();
    const dlg = app.dialog();
    const name = within(dlg).getByRole("textbox", { name: "Name des Archivs" });
    await app.user.selectOptions(within(dlg).getByRole("combobox", { name: "Format" }), "tarXz");
    expect(name).toHaveValue("notizen.tar.xz");
    await app.user.clear(name);
    await app.user.type(name, "mein-paket.zip");
    await app.user.selectOptions(within(dlg).getByRole("combobox", { name: "Format" }), "sevenZ");
    expect(name).toHaveValue("mein-paket.7z");
  });

  test("Kontextmenü „Packen …“ öffnet denselben Dialog; mehrere Markierte, Name vom ersten", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Shift>}{ArrowDown}{/Shift}");
    const marked = app.selected(0);
    expect(marked).toHaveLength(2);
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, marked[0]!) });
    await app.user.click(await screen.findByRole("menuitem", { name: "Packen …" }));
    expect(app.dialog()).toHaveAccessibleName("Packen");
    expect(within(app.dialog()).getByText("2 Elemente packen")).toBeInTheDocument();
    const first = marked[0]!;
    expect(within(app.dialog()).getByRole("textbox", { name: "Name des Archivs" })).toHaveValue(`${first.slice(0, first.lastIndexOf("."))}.zip`);
  });

  test("Packen startet einen Auftrag mit Fortschritt; danach liegt das Archiv im Zielordner", async () => {
    const app = await packSetup();
    const dlg = app.dialog();
    await app.user.click(within(dlg).getByRole("button", { name: "Packen" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    const panel = await screen.findByRole("region", { name: "Aufträge" });
    expect(within(panel).getByRole("progressbar")).toBeInTheDocument();
    expect(within(panel).getByRole("group", { name: /Packen/ })).toBeInTheDocument();
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).toContain("notizen.zip"));
    expect(within(panel).getByText("Fertig")).toBeInTheDocument();
    // das neue Archiv lässt sich öffnen und enthält die Datei
    const entries = (await app.api.list(archiveRootOf(`${DOWNLOADS}/notizen.zip`))).entries.map((e) => e.name);
    expect(entries).toEqual(["notizen.txt"]);
  });

  test("Abbrechen im Auftragspanel: kein Archiv entsteht", async () => {
    const app = await packSetup();
    await app.user.click(within(app.dialog()).getByRole("button", { name: "Packen" }));
    const panel = await screen.findByRole("region", { name: "Aufträge" });
    await app.user.click(within(panel).getByRole("button", { name: "Auftrag abbrechen" }));
    await app.runJobs();
    await waitFor(() => expect(within(panel).getByText("Abgebrochen")).toBeInTheDocument());
    expect((await app.api.list(DOWNLOADS)).entries.map((e) => e.name)).not.toContain("notizen.zip");
  });

  test("Abbrechen im Dialog tut nichts; Esc schließt", async () => {
    const app = await packSetup();
    await app.user.click(within(app.dialog()).getByRole("button", { name: "Abbrechen" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.runtime.store.getState().jobs.jobs).toHaveLength(0);
  });

  test("vorhandener Name: Fehler am Feld, nie überschreiben; mit neuem Namen klappt es", async () => {
    const app = await packSetup();
    const dlg = app.dialog();
    const name = within(dlg).getByRole("textbox", { name: "Name des Archivs" });
    await app.user.clear(name);
    await app.user.type(name, "rezept-paket.tar.gz");
    await app.user.selectOptions(within(dlg).getByRole("combobox", { name: "Format" }), "tarGz");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Der Name ist bereits vorhanden.");
    expect(within(dlg).getByRole("button", { name: "Packen" })).toBeDisabled();
    await app.user.clear(name);
    await app.user.type(name, "neu.tar.gz");
    expect(within(dlg).queryByRole("alert")).toBeNull();
    await app.user.click(within(dlg).getByRole("button", { name: "Packen" }));
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).toContain("neu.tar.gz"));
    expect((await app.api.list(DOWNLOADS)).entries.find((e) => e.name === "rezept-paket.tar.gz")?.size).toBe(730_000);
  });

  test("Fehler `exists` vom Kern (Name erst nach dem Öffnen des Dialogs vergeben) bleibt im Dialog, Name änderbar", async () => {
    const app = await packSetup();
    await app.base.packArchive([`${DOCS}/Änderungen.txt`], DOWNLOADS, "notizen.zip", "zip");
    await app.runJobs();
    const dlg = app.dialog();
    await app.user.click(within(dlg).getByRole("button", { name: "Packen" }));
    expect(await within(dlg).findByRole("alert")).toHaveTextContent("Der Name ist bereits vorhanden: notizen.zip");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    const name = within(dlg).getByRole("textbox", { name: "Name des Archivs" });
    await app.user.clear(name);
    await app.user.type(name, "notizen2.zip");
    expect(within(dlg).queryByRole("alert")).toBeNull();
    await app.user.click(within(dlg).getByRole("button", { name: "Packen" }));
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).toContain("notizen2.zip"));
    // das vorhandene Archiv blieb unberührt
    expect(await app.api.list(archiveRootOf(`${DOWNLOADS}/notizen.zip`)).then((l) => l.entries.map((e) => e.name))).toEqual(["Änderungen.txt"]);
  });

  test("andere Fehler des Kerns erscheinen im Dialog statt zu verschwinden", async () => {
    const app = await packSetup({
      wrap: (api) => ({
        ...api,
        packArchive: async () => {
          throw new CoreError("Zugriff verweigert", "permission");
        },
      }),
    });
    await app.user.click(within(app.dialog()).getByRole("button", { name: "Packen" }));
    expect(await within(app.dialog()).findByRole("alert")).toHaveTextContent("Zugriff verweigert");
  });

  test("aus einem Archiv heraus lässt sich packen (Quelle im Archiv, Ziel lokal)", async () => {
    const app = await insideArchive();
    await app.user.click(app.row(0, "README.md"));
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    expect(within(app.dialog()).getByRole("textbox", { name: "Name des Archivs" })).toHaveValue("README.zip");
    expect(within(app.dialog()).getByText(`Ziel: ${DOCS}`)).toBeInTheDocument();
  });

  test("nicht anbieten, wenn das Ziel ein Archiv ist (Tastatur und Kontextmenü)", async () => {
    const app = await setupApp({ left: DOCS, right: ROOT });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(alerts().some((t) => t.includes("Das Ziel ist ein Archiv"))).toBe(true);
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "notizen.txt") });
    const menu = await screen.findByRole("menu", { name: "Kontextmenü" });
    expect(within(menu).queryByRole("menuitem", { name: "Packen …" })).toBeNull();
  });

  test("ohne Auswahl (Fokus auf „..“) passiert nichts außer dem Hinweis; Strg+Umschalt+P im Eingabefeld ist wirkungslos", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await app.user.click(within(app.pane(0)).getByRole("searchbox"));
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("englische Oberfläche", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS, language: "en" });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Control>}{Shift>}P{/Shift}{/Control}");
    expect(app.dialog()).toHaveAccessibleName("Pack");
    expect(within(app.dialog()).getByRole("textbox", { name: "Archive name" })).toHaveValue("notizen.zip");
    expect(within(app.dialog()).getByText(`Target: ${DOWNLOADS}`)).toBeInTheDocument();
  });
});
