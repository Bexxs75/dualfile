import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { CoreError } from "./ipc/api";
import { archiveRootOf } from "./state/paths";
import { HOME, setupApp } from "./test-utils";

const LEFT = `${HOME}/Vergleich/links`;
const RIGHT = `${HOME}/Vergleich/rechts`;
const TEXT = `${HOME}/Vergleich/text`;
const ZIP = archiveRootOf(`${HOME}/Downloads/rezept-paket.tar.gz`);

async function open(setup: Parameters<typeof setupApp>[0] = {}) {
  const app = await setupApp({ left: LEFT, right: RIGHT, compareDemo: true, ...setup });
  await app.user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
  return app;
}
const cmp = () => screen.getByRole("dialog", { name: "Verzeichnisse vergleichen" });
const syncDlg = () => screen.getByRole("alertdialog", { name: "Synchronisieren" });
const tableRows = (dialog: HTMLElement, name: string) => within(within(dialog).getByRole("table", { name })).getAllByRole("row").slice(1);
const rowText = (row: HTMLElement) => row.textContent ?? "";

async function startCompare(app: Awaited<ReturnType<typeof open>>) {
  await app.user.click(within(cmp()).getByRole("button", { name: "Vergleichen" }));
  await app.runJobs();
  await waitFor(() => expect(within(cmp()).getByRole("table", { name: "Vergleichsergebnis" })).toBeInTheDocument());
}

describe("Vergleichsdialog", () => {
  test("Strg+Umschalt+D öffnet den Dialog mit den Ordnern beider Fenster und den Optionen", async () => {
    const app = await open();
    const dlg = cmp();
    expect(dlg).toHaveAttribute("aria-modal", "true");
    expect(within(dlg).getByText(`Links: ${LEFT}`)).toBeInTheDocument();
    expect(within(dlg).getByText(`Rechts: ${RIGHT}`)).toBeInTheDocument();
    expect(within(dlg).getByRole("checkbox", { name: /Inhalt vergleichen/ })).not.toBeChecked();
    expect(within(dlg).getByRole("checkbox", { name: "Unterordner einbeziehen" })).toBeChecked();
    expect(within(dlg).getByRole("button", { name: "Synchronisieren …" })).toBeDisabled();
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Toolbar-Knopf und Kontextmenü öffnen ihn ebenfalls", async () => {
    const app = await setupApp({ left: LEFT, right: RIGHT, compareDemo: true });
    await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Verzeichnisse vergleichen …" }));
    expect(cmp()).toBeInTheDocument();
    await app.user.keyboard("{Escape}");
    fireEvent.contextMenu(within(app.pane(0)).getByRole("grid"));
    await app.user.click(screen.getByRole("menuitem", { name: "Verzeichnisse vergleichen …" }));
    expect(cmp()).toBeInTheDocument();
  });

  test("Vergleich zeigt Zustände mit Text und Symbol, den Unvollständig-Hinweis und den Grund bei „Unbekannt“", async () => {
    const app = await open();
    await startCompare(app);
    const dlg = cmp();
    const rows = tableRows(dlg, "Vergleichsergebnis");
    const byPath = (path: string) => rows.find((row) => within(row).queryByText(path, { selector: "code" }) !== null)!;
    expect(rowText(byPath("gleich.txt"))).toContain("Gleich");
    expect(rowText(byPath("nur-links.txt"))).toContain("Nur links");
    expect(rowText(byPath("nur-links.txt"))).toContain("◀");
    expect(rowText(byPath("nur-rechts.txt"))).toContain("Nur rechts");
    expect(rowText(byPath("neuer-links.txt"))).toContain("Links neuer");
    expect(rowText(byPath("neuer-rechts.txt"))).toContain("Rechts neuer");
    expect(rowText(byPath("anders.txt"))).toContain("Unterschiedlich");
    expect(rowText(byPath("typ"))).toContain("Art verschieden");
    expect(rowText(byPath("gesperrt"))).toContain("Unbekannt");
    expect(rowText(byPath("gesperrt"))).toContain("Nicht untersucht:");
    expect(within(dlg).getByText(/Unvollständiges Ergebnis/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Synchronisieren …" })).toBeEnabled();
  });

  test("Filter nach Zustand und Text", async () => {
    const app = await open();
    await startCompare(app);
    const dlg = cmp();
    await app.user.click(within(dlg).getByRole("button", { name: /Nur links \(\d+\)/ }));
    await waitFor(() => expect(tableRows(dlg, "Vergleichsergebnis").every((r) => rowText(r).includes("Nur links"))).toBe(true));
    expect(tableRows(dlg, "Vergleichsergebnis").length).toBeGreaterThan(1);
    expect(within(dlg).getByRole("button", { name: /Nur links/ })).toHaveAttribute("aria-pressed", "true");
    await app.user.click(within(dlg).getByRole("button", { name: /Nur links/ }));
    await app.user.type(within(dlg).getByRole("searchbox", { name: "Pfad enthält" }), "ordner-nur");
    await waitFor(() => expect(tableRows(dlg, "Vergleichsergebnis").length).toBeGreaterThan(0));
    expect(tableRows(dlg, "Vergleichsergebnis").every((r) => rowText(r).includes("ordner-nur"))).toBe(true);
    await app.user.clear(within(dlg).getByRole("searchbox", { name: "Pfad enthält" }));
    await app.user.type(within(dlg).getByRole("searchbox", { name: "Pfad enthält" }), "gibt-es-nicht");
    await waitFor(() => expect(within(dlg).getByText("Keine Einträge für diesen Filter.")).toBeInTheDocument());
  });

  test("ein laufender Vergleich lässt sich abbrechen und hinterlässt kein Ergebnis", async () => {
    const app = await open();
    await app.user.click(within(cmp()).getByRole("button", { name: "Vergleichen" }));
    const dlg = cmp();
    expect(await within(dlg).findByRole("status")).toHaveTextContent("Vergleiche");
    const id = app.runtime.store.getState().compare!.jobId!;
    await app.user.click(within(dlg).getByRole("button", { name: "Vergleich abbrechen" }));
    await app.runJobs();
    await waitFor(() => expect(within(dlg).getByText("Der Vergleich wurde abgebrochen. Es gibt kein Ergebnis.")).toBeInTheDocument());
    await expect(app.base.comparePage(id, 0, 10)).rejects.toBeInstanceOf(CoreError);
    expect(within(dlg).queryByRole("table", { name: "Vergleichsergebnis" })).toBeNull();
  });

  test("Ergebnisse kommen seitenweise (100 je Seite)", async () => {
    const app = await open();
    await act(async () => {
      for (let i = 1; i <= 120; i += 1) await app.base.makeDir(LEFT, `zz-${String(i).padStart(3, "0")}`);
    });
    await startCompare(app);
    const dlg = cmp();
    expect(tableRows(dlg, "Vergleichsergebnis")).toHaveLength(100);
    expect(within(dlg).getByText(/^1–100 von \d+$/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Vorige Seite" })).toBeDisabled();
    await app.user.click(within(dlg).getByRole("button", { name: "Nächste Seite" }));
    await waitFor(() => expect(within(dlg).getByText(/^101–\d+ von \d+$/)).toBeInTheDocument());
    expect(tableRows(dlg, "Vergleichsergebnis").length).toBeGreaterThan(20);
    expect(within(dlg).getByRole("button", { name: "Nächste Seite" })).toBeDisabled();
    await app.user.click(within(dlg).getByRole("button", { name: "Vorige Seite" }));
    await waitFor(() => expect(within(dlg).getByText(/^1–100 von/)).toBeInTheDocument());
  });

  test("Schließen gibt das Ergebnis frei", async () => {
    const app = await open();
    await startCompare(app);
    const id = app.runtime.store.getState().compare!.jobId!;
    await expect(app.base.comparePage(id, 0, 1)).resolves.toBeDefined();
    await app.user.click(within(cmp()).getAllByRole("button", { name: "Schließen" }).at(-1)!);
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(async () => {
      await expect(app.base.comparePage(id, 0, 1)).rejects.toMatchObject({ kind: "notFound" });
    });
  });

  test("Schließen während der Vergleich läuft: abgebrochen und nichts bleibt zurück", async () => {
    const app = await open();
    await app.user.click(within(cmp()).getByRole("button", { name: "Vergleichen" }));
    const id = app.runtime.store.getState().compare!.jobId!;
    await app.user.keyboard("{Escape}");
    await app.runJobs();
    await expect(app.base.comparePage(id, 0, 1)).rejects.toMatchObject({ kind: "notFound" });
  });

  test("Zeilenauswahl über Seiten, Auswahl aufheben", async () => {
    const app = await open();
    await startCompare(app);
    const dlg = cmp();
    await app.user.click(within(dlg).getByRole("checkbox", { name: "Zeile auswählen: nur-links.txt" }));
    expect(within(dlg).getByText("1 Zeile ausgewählt")).toBeInTheDocument();
    await app.user.click(within(dlg).getByRole("button", { name: "Auswahl aufheben" }));
    expect(within(dlg).getByText("0 Zeilen ausgewählt")).toBeInTheDocument();
  });
});

// ------------------------------------------------------------------------------------ Abgleich

const GEM_L = `${LEFT}/gemeinsam`;
const GEM_R = `${RIGHT}/gemeinsam`;
const namesIn = async (api: { list(p: string): Promise<{ entries: { name: string }[] }> }, dir: string) =>
  (await api.list(dir)).entries.map((entry) => entry.name).sort();

/** Vollständiger Vergleich (ohne gesperrten Teilbaum): `gemeinsam` links gegen rechts. */
async function openSync(setup: Parameters<typeof setupApp>[0] = { left: GEM_L, right: GEM_R }) {
  const app = await open(setup);
  await startCompare(app);
  await app.user.click(within(cmp()).getByRole("button", { name: "Synchronisieren …" }));
  await syncReady();
  return app;
}
const syncReady = () => waitFor(() => expect(within(syncDlg()).getByRole("table", { name: "Geplante Aktionen" })).toBeInTheDocument());
const planRows = () => tableRows(syncDlg(), "Geplante Aktionen");
const confirmButton = () => within(syncDlg()).getByRole("button", { name: /^Ausführen/ });

describe("Synchronisieren: Vorschau", () => {
  test("Standard: nur Fehlendes ergänzen, jede Aktion einzeln, nichts überschrieben oder gelöscht", async () => {
    const app = await openSync();
    const dlg = syncDlg();
    expect(within(dlg).getByRole("radio", { name: "Links → rechts" })).toBeChecked();
    expect(within(dlg).getByRole("checkbox", { name: /Neueres ersetzen/ })).not.toBeChecked();
    expect(within(dlg).getByText(/Standard: Es wird nur Fehlendes ergänzt/)).toBeInTheDocument();
    expect(within(dlg).getByText("1 neu kopieren, 0 überschreiben, 0 in den Papierkorb, 0 Ordner anlegen, 70 B")).toBeInTheDocument();
    expect(planRows()).toHaveLength(1);
    expect(rowText(planRows()[0]!)).toContain("Neu kopieren");
    expect(rowText(planRows()[0]!)).toContain("c.txt");
    expect(confirmButton()).toHaveTextContent("Ausführen: 1 kopieren");
    expect(app.runtime.store.getState().compare!.sync!.effective!.trashes).toBe(0);
  });

  test("Überschreiben und Papierkorb sind rot und mit Text markiert, mit Zählern im Knopf", async () => {
    await openSync();
    const dlg = syncDlg();
    await openMirrorWithReplace(dlg);
    const rows = planRows();
    const replace = rows.find((r) => rowText(r).includes("b.txt"))!;
    const trash = rows.find((r) => rowText(r).includes("d.txt"))!;
    expect(replace).toHaveClass("danger");
    expect(rowText(replace)).toContain("ÜBERSCHREIBEN");
    expect(rowText(replace)).toContain("⚠");
    expect(trash).toHaveClass("danger");
    expect(rowText(trash)).toContain("IN DEN PAPIERKORB");
    expect(within(dlg).getByText(/Achtung: Diese Aktionen überschreiben/)).toBeInTheDocument();
    expect(confirmButton()).toHaveTextContent("Ausführen: 1 kopieren, 1 überschreiben, 1 in den Papierkorb");
    expect(confirmButton()).toHaveClass("warn");
  });

  test("Enter bestätigt nicht: der Standardknopf ist „Abbrechen“ und schließt nur", async () => {
    const app = await openSync();
    expect(within(syncDlg()).getByRole("button", { name: "Abbrechen" })).toHaveFocus();
    await app.user.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog", { name: "Synchronisieren" })).toBeNull();
    await app.runJobs();
    expect(app.runtime.store.getState().jobs.jobs.filter((j) => app.runtime.controller.jobKindOf(j.id) === "sync")).toHaveLength(0);
    expect(await namesIn(app.base, GEM_R)).toEqual(["a.txt", "b.txt", "d.txt"]);
  });

  test("Enter auf einer Aktionszeile oder Option führt ebenfalls nichts aus", async () => {
    const app = await openSync();
    await openMirrorWithReplace(syncDlg());
    for (const control of [
      within(syncDlg()).getByRole("checkbox", { name: "Aktion ausführen: b.txt" }),
      within(syncDlg()).getByRole("radio", { name: /Spiegeln/ }),
    ]) {
      control.focus();
      await app.user.keyboard("{Enter}");
    }
    await app.runJobs();
    expect(syncDlg()).toBeInTheDocument();
    expect(app.runtime.store.getState().compare!.sync!.phase).toBe("preview");
    expect(await namesIn(app.base, GEM_R)).toEqual(["a.txt", "b.txt", "d.txt"]);
  });

  test("Spiegeln bei unvollständigem Ergebnis: nichts wird gelöscht, der Grund steht da", async () => {
    const app = await open();
    await startCompare(app);
    await app.user.click(within(cmp()).getByRole("button", { name: "Synchronisieren …" }));
    await syncReady();
    await app.user.click(within(syncDlg()).getByRole("radio", { name: /Spiegeln/ }));
    await waitFor(() => expect(app.runtime.store.getState().compare!.sync!.mode).toBe("mirror"));
    await syncReady();
    const dlg = syncDlg();
    await waitFor(() => expect(within(dlg).getByText("Der Vergleich ist unvollständig: Es wird nichts gelöscht.")).toBeInTheDocument());
    expect(planRows().some((row) => rowText(row).includes("IN DEN PAPIERKORB"))).toBe(false);
    expect(app.runtime.store.getState().compare!.sync!.effective!.trashes).toBe(0);
    await app.user.click(within(dlg).getByText(/Hinweise \(/));
    expect(within(dlg).getByText(/Der Vergleich ist unvollständig; es wird nichts gelöscht/)).toBeInTheDocument();
  });

  test("einzelne Aktionen lassen sich abwählen; der Zähler und der ausgeführte Plan folgen", async () => {
    const app = await openSync();
    await openMirrorWithReplace(syncDlg());
    const dlg = syncDlg();
    await app.user.click(within(dlg).getByRole("checkbox", { name: "Aktion ausführen: d.txt" }));
    await waitFor(() => expect(confirmButton()).toHaveTextContent("Ausführen: 1 kopieren, 1 überschreiben"));
    expect(confirmButton()).not.toHaveTextContent("Papierkorb");
    expect(within(dlg).getByRole("checkbox", { name: "Aktion ausführen: d.txt" })).not.toBeChecked();
    await app.user.click(within(dlg).getByRole("button", { name: "Alle abwählen" }));
    await waitFor(() => expect(within(dlg).getByText("Nichts ausgewählt: Es gibt nichts auszuführen.")).toBeInTheDocument());
    expect(confirmButton()).toBeDisabled();
    await app.user.click(within(dlg).getByRole("button", { name: "Alle wählen" }));
    await waitFor(() => expect(confirmButton()).toHaveTextContent("1 in den Papierkorb"));
  });

  test("die Zeilenauswahl des Vergleichs wählt vor: nur diese Zeilen sind angehakt", async () => {
    const app = await open({ left: GEM_L, right: GEM_R });
    await startCompare(app);
    await app.user.click(within(cmp()).getByRole("checkbox", { name: "Zeile auswählen: c.txt" }));
    await app.user.click(within(cmp()).getByRole("button", { name: "Synchronisieren …" }));
    await syncReady();
    await app.user.click(within(syncDlg()).getByRole("radio", { name: /Spiegeln/ }));
    await syncReady();
    await waitFor(() => expect(confirmButton()).toHaveTextContent("Ausführen: 1 kopieren"));
    expect(confirmButton()).not.toHaveTextContent("Papierkorb");
    expect(within(syncDlg()).getByRole("checkbox", { name: "Aktion ausführen: d.txt" })).not.toBeChecked();
  });

  test("Archiv als Ziel: keine Richtung, die es beschreibt; beide Archive: kein Abgleich", async () => {
    const app = await open({ left: ZIP, right: RIGHT });
    await startCompare(app);
    await app.user.click(within(cmp()).getByRole("button", { name: "Synchronisieren …" }));
    await syncReady().catch(() => undefined);
    const dlg = syncDlg();
    expect(within(dlg).getByRole("radio", { name: "Links → rechts" })).not.toBeDisabled(); // Ziel rechts ist lokal
    expect(within(dlg).getByRole("radio", { name: "Rechts → links" })).toBeDisabled(); // Ziel links ist ein Archiv
    expect(within(dlg).getByRole("radio", { name: /Spiegeln/ })).toBeDisabled();
  });

  test("zwei Archive: „Synchronisieren“ ist gesperrt und nennt den Grund", async () => {
    const app = await open({ left: ZIP, right: ZIP });
    await startCompare(app);
    const dlg = cmp();
    expect(within(dlg).getByRole("button", { name: "Synchronisieren …" })).toBeDisabled();
    expect(within(dlg).getByText("Das Ziel ist ein Archiv: Synchronisieren ist nicht möglich.")).toBeInTheDocument();
  });
});

async function openMirrorWithReplace(dlg: HTMLElement) {
  await userEventFor().click(within(dlg).getByRole("radio", { name: /Spiegeln/ }));
  await syncReady();
  await userEventFor().click(within(dlg).getByRole("checkbox", { name: /Neueres ersetzen/ }));
  await waitFor(() => expect(confirmButton()).toHaveTextContent("1 überschreiben"));
}
import userEvent from "@testing-library/user-event";
const userEventFor = () => userEvent.setup();

const conflictDlg = () => screen.findByRole("alertdialog", { name: "Das Ziel existiert bereits" });
const report = () => screen.findByRole("region", { name: "Bericht" });

describe("Synchronisieren: Ausführen und Bericht", () => {
  test("Ausführen (Spiegeln + Ersetzen): Fortschritt, Konfliktfrage über die Konfliktbrücke, Bericht, Papierkorb statt Löschen", async () => {
    const app = await openSync();
    await openMirrorWithReplace(syncDlg());
    await app.user.click(confirmButton());
    await app.runJobs();
    const conflict = await conflictDlg();
    expect(within(conflict).getByRole("button", { name: "Überspringen" })).toHaveFocus();
    await app.user.click(within(conflict).getByRole("button", { name: "Überschreiben" }));
    await app.runJobs();
    const rep = await report();
    expect(within(rep).getByText("3 ausgeführt, 0 übersprungen, 0 fehlgeschlagen")).toBeInTheDocument();
    expect(await namesIn(app.base, GEM_R)).toEqual(["a.txt", "b.txt", "c.txt"]);
  });

  test("abgewählte Aktionen werden nicht ausgeführt", async () => {
    const app = await openSync();
    await app.user.click(within(syncDlg()).getByRole("radio", { name: /Spiegeln/ }));
    await syncReady();
    await waitFor(() => expect(confirmButton()).toHaveTextContent("1 in den Papierkorb"));
    await app.user.click(within(syncDlg()).getByRole("checkbox", { name: "Aktion ausführen: d.txt" }));
    await waitFor(() => expect(confirmButton()).toHaveTextContent("Ausführen: 1 kopieren"));
    await waitFor(() => expect(confirmButton()).not.toHaveTextContent("Papierkorb"));
    await app.user.click(confirmButton());
    await app.runJobs();
    await report();
    expect(await namesIn(app.base, GEM_R)).toEqual(["a.txt", "b.txt", "c.txt", "d.txt"]);
  });

  test("veralteter Plan: das Ziel hat sich geändert, die Aktion wird übersprungen und im Bericht begründet", async () => {
    const app = await openSync();
    await app.user.click(within(syncDlg()).getByRole("radio", { name: /Spiegeln/ }));
    await syncReady();
    await waitFor(() => expect(confirmButton()).toHaveTextContent("1 in den Papierkorb"));
    await app.base.rename(`${GEM_R}/d.txt`, "d-neu.txt");
    await app.user.click(confirmButton());
    await app.runJobs();
    const rep = await report();
    expect(within(rep).getByText(/2 ausgeführt|1 ausgeführt/)).toBeInTheDocument();
    expect(within(rep).getByText(/1 übersprungen/)).toBeInTheDocument();
    expect(within(rep).getByText("Übersprungen")).toBeInTheDocument();
    expect(within(rep).getByText(/Grund: Übersprungen:/)).toBeInTheDocument();
    expect(await namesIn(app.base, GEM_R)).toContain("d-neu.txt");
  });

  test("Konflikt „Überspringen“ steht im Bericht als übersprungen", async () => {
    const app = await openSync();
    await openMirrorWithReplace(syncDlg());
    await app.user.click(confirmButton());
    await app.runJobs();
    await app.user.click(within(await conflictDlg()).getByRole("button", { name: "Überspringen" }));
    await app.runJobs();
    const rep = await report();
    expect(within(rep).getByText(/1 übersprungen/)).toBeInTheDocument();
    expect(within(rep).getByText(/ohne Angabe eines Grundes/)).toBeInTheDocument();
  });

  test("Abbrechen aus der Konfliktfrage beendet den Abgleich; der Bericht sagt es", async () => {
    const app = await openSync();
    await openMirrorWithReplace(syncDlg());
    await app.user.click(confirmButton());
    await app.runJobs();
    await app.user.click(within(await conflictDlg()).getByRole("button", { name: "Abbrechen" }));
    await app.runJobs();
    const rep = await report();
    expect(within(rep).getByText("Der Abgleich wurde abgebrochen.")).toBeInTheDocument();
  });

  test("Abgleich läuft: Abbrechen-Knopf bricht ab, Schließen ist währenddessen gesperrt", async () => {
    const app = await openSync();
    await app.user.click(confirmButton());
    const dlg = syncDlg();
    expect(within(dlg).getByRole("status")).toHaveTextContent("Abgleich läuft");
    await app.user.keyboard("{Escape}");
    expect(syncDlg()).toBeInTheDocument();
    await app.user.click(within(dlg).getByRole("button", { name: "Abgleich abbrechen" }));
    await app.runJobs();
    await report();
  });

  test("nach dem Abgleich: „Erneut vergleichen“ zeigt den neuen Stand", async () => {
    const app = await openSync();
    await app.user.click(confirmButton());
    await app.runJobs();
    await report();
    await app.user.click(within(syncDlg()).getByRole("button", { name: "Erneut vergleichen" }));
    await app.runJobs();
    await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Synchronisieren" })).toBeNull());
    await waitFor(() => {
      const rows = tableRows(cmp(), "Vergleichsergebnis");
      const c = rows.find((row) => rowText(row).includes("c.txt"))!;
      expect(rowText(c)).toContain("Gleich");
    });
  });

  test("ein abgelehntes Token: Fehlermeldung im Dialog, nichts ausgeführt, „Neu planen“ erzeugt einen frischen Plan", async () => {
    let reject = true;
    const app = await openSync({
      left: GEM_L,
      right: GEM_R,
      wrap: (api) => ({
        ...api,
        runSync: (token: string) =>
          reject ? Promise.reject(new CoreError("Falsches Token", "invalidToken")) : api.runSync(token),
      }),
    });
    await app.user.click(confirmButton());
    await waitFor(() => expect(within(syncDlg()).getByRole("alert")).toHaveTextContent("Die Bestätigung gehört nicht zu diesem Abgleichsplan"));
    expect(await namesIn(app.base, GEM_R)).toEqual(["a.txt", "b.txt", "d.txt"]);
    reject = false;
    await app.user.click(within(syncDlg()).getByRole("button", { name: "Neu planen" }));
    await syncReady();
    await app.user.click(confirmButton());
    await app.runJobs();
    await report();
    expect(await namesIn(app.base, GEM_R)).toContain("c.txt");
  });
});

// ------------------------------------------------------------------------------------ Textvergleich

const diffDlg = () => screen.getByRole("dialog", { name: "Textvergleich" });
const diffRows = () => tableRows(diffDlg(), "Zeilenvergleich").filter((row) => row.getAttribute("data-kind") !== null);
const kinds = () => diffRows().map((row) => row.getAttribute("data-kind"));

async function markTwo(app: Awaited<ReturnType<typeof setupApp>>, a: string, b: string) {
  await app.user.click(app.row(0, a));
  await app.user.keyboard("{Control>}");
  await app.user.click(app.row(0, b));
  await app.user.keyboard("{/Control}");
}

describe("Textvergleich", () => {
  test("zwei markierte Dateien: Strg+Umschalt+F zeigt zwei Spalten mit Marken (Text + Symbol) für Einfügen und Ersetzen", async () => {
    const app = await setupApp({ left: TEXT, right: LEFT, compareDemo: true });
    await markTwo(app, "vergleich-alt.txt", "vergleich-neu.txt");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{Control}");
    await app.user.keyboard("{/Control}");
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    await waitFor(() => expect(within(dlg).getByRole("table", { name: "Zeilenvergleich" })).toBeInTheDocument());
    expect(within(dlg).getByText(`Links: ${TEXT}/vergleich-alt.txt`)).toBeInTheDocument();
    expect(within(dlg).getByText(`Rechts: ${TEXT}/vergleich-neu.txt`)).toBeInTheDocument();
    const k = kinds();
    expect(k).toContain("equal");
    expect(k).toContain("replace");
    expect(k).toContain("insert");
    const insert = diffRows().find((row) => row.getAttribute("data-kind") === "insert")!;
    expect(insert.textContent).toContain("+");
    expect(insert.textContent).toContain("Eingefügt");
    expect(insert.textContent).toContain("- 1 EL Zucker");
    const replace = diffRows().find((row) => row.getAttribute("data-kind") === "replace" && row.textContent!.includes("250 g"))!;
    expect(replace.textContent).toContain("- 250 g Mehl");
    expect(replace.textContent).toContain("- 300 g Mehl");
    expect(replace.textContent).toContain("Geändert");
  });

  test("Springen zum nächsten und vorigen Unterschied (Tasten und Knöpfe), kein Umbruch an den Enden", async () => {
    const app = await setupApp({ left: TEXT, right: LEFT, compareDemo: true });
    await markTwo(app, "vergleich-alt.txt", "vergleich-neu.txt");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    const position = () => within(dlg).getByText(/^Unterschied \d+ von \d+$/).textContent!;
    await waitFor(() => expect(position()).toMatch(/^Unterschied 1 von \d+$/));
    const total = Number(/von (\d+)/.exec(position())![1]);
    expect(total).toBeGreaterThanOrEqual(3);
    expect(within(dlg).getByRole("button", { name: "Voriger Unterschied" })).toBeDisabled();
    await app.user.keyboard("n");
    expect(position()).toBe(`Unterschied 2 von ${total}`);
    await app.user.keyboard("{F3}");
    expect(position()).toBe(`Unterschied 3 von ${total}`);
    await app.user.keyboard("p");
    expect(position()).toBe(`Unterschied 2 von ${total}`);
    await app.user.keyboard("{Shift>}{F3}{/Shift}");
    expect(position()).toBe(`Unterschied 1 von ${total}`);
    await app.user.keyboard("p");
    expect(position()).toBe(`Unterschied 1 von ${total}`);
    for (let i = 0; i < total + 2; i += 1) await app.user.click(within(dlg).getByRole("button", { name: "Nächster Unterschied" }).hasAttribute("disabled") ? within(dlg).getByRole("button", { name: "Voriger Unterschied" }) : within(dlg).getByRole("button", { name: "Nächster Unterschied" }));
    expect(position()).toMatch(/^Unterschied \d+ von \d+$/);
    // Die aktuelle Zeile ist ausgezeichnet.
    expect(diffRows().filter((row) => row.getAttribute("aria-current") === "true")).toHaveLength(1);
  });

  test("Optionen Leerraum und Groß-/Kleinschreibung laden neu (eine Reihe aus gleichen Zeilen bleibt gleich)", async () => {
    const app = await setupApp({ left: TEXT, right: LEFT, compareDemo: true });
    await markTwo(app, "vergleich-alt.txt", "vergleich-neu.txt");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    await waitFor(() => expect(within(dlg).getByRole("table", { name: "Zeilenvergleich" })).toBeInTheDocument());
    const calls: unknown[] = [];
    const original = app.runtime.controller.api.diffFiles;
    app.runtime.controller.api.diffFiles = (l, r, o) => (calls.push(o), original(l, r, o));
    await app.user.click(within(dlg).getByRole("checkbox", { name: "Leerraum ignorieren" }));
    await app.user.click(within(dlg).getByRole("checkbox", { name: "Groß-/Kleinschreibung ignorieren" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toEqual({ ignoreWhitespace: true, ignoreCase: true });
    app.runtime.controller.api.diffFiles = original;
  });

  test("Binärdatei und zu große Datei: verständliche Meldung statt Ansicht", async () => {
    const app = await setupApp({ left: TEXT, right: LEFT, compareDemo: true });
    await markTwo(app, "vergleich-alt.txt", "vergleich-bin.dat");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    let dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    expect(await within(dlg).findByRole("alert")).toHaveTextContent("Binärdateien lassen sich nicht als Text vergleichen.");
    expect(within(dlg).queryByRole("table", { name: "Zeilenvergleich" })).toBeNull();
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    // zweite Datei wählen: 3 MiB
    await app.user.click(app.row(0, "vergleich-alt.txt"));
    await app.user.keyboard("{Control>}");
    await app.user.click(app.row(0, "vergleich-gross.txt"));
    await app.user.keyboard("{/Control}");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    expect(await within(dlg).findByRole("alert")).toHaveTextContent("Die Datei ist zu groß für den Textvergleich.");
  });

  test("eine Datei in jedem Fenster genügt; sonst ein Hinweis", async () => {
    const app = await setupApp({ left: TEXT, right: TEXT, compareDemo: true });
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull(); // Fokus auf „..“: keine Datei
    expect(screen.getByText(/Bitte zwei Dateien wählen/)).toBeInTheDocument();
    await app.user.click(app.row(0, "vergleich-alt.txt"));
    await app.user.click(app.row(1, "vergleich-neu.txt"));
    await app.user.click(app.row(0, "vergleich-alt.txt"));
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    expect(within(dlg).getByText(`Links: ${TEXT}/vergleich-alt.txt`)).toBeInTheDocument();
    expect(within(dlg).getByText(`Rechts: ${TEXT}/vergleich-neu.txt`)).toBeInTheDocument();
  });

  test("aus einer Ergebniszeile des Ordnervergleichs; Schließen führt zurück in den Vergleich", async () => {
    const app = await open();
    await startCompare(app);
    await app.user.click(within(cmp()).getByRole("button", { name: "Textvergleich: anders.txt" }));
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    expect(within(dlg).getByText(`Links: ${LEFT}/anders.txt`)).toBeInTheDocument();
    await waitFor(() => expect(within(dlg).getByRole("table", { name: "Zeilenvergleich" })).toBeInTheDocument());
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Textvergleich" })).toBeNull();
    expect(cmp()).toBeInTheDocument();
  });

  test("Inhalt wird nur als Text gesetzt, nie als HTML", async () => {
    const app = await setupApp({ left: TEXT, right: LEFT, compareDemo: true });
    await markTwo(app, "vergleich-alt.txt", "vergleich-neu.txt");
    await app.user.keyboard("{Control>}{Shift>}F{/Shift}{/Control}");
    const dlg = await screen.findByRole("dialog", { name: "Textvergleich" });
    await waitFor(() => expect(within(dlg).getByRole("table", { name: "Zeilenvergleich" })).toBeInTheDocument());
    expect(dlg.querySelector("script, img, iframe, a")).toBeNull();
    expect(dlg.innerHTML).not.toMatch(/dangerouslySetInnerHTML/);
  });
});

describe("Barrierefreiheit", () => {
  test("Fokusfalle: Tab bleibt im Vergleichs- und im Abgleichsdialog, Esc schließt zuerst den oberen", async () => {
    const app = await openSync();
    for (let i = 0; i < 12; i += 1) {
      await app.user.tab();
      expect(syncDlg().contains(document.activeElement)).toBe(true);
    }
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog", { name: "Synchronisieren" })).toBeNull();
    expect(cmp()).toBeInTheDocument();
    for (let i = 0; i < 12; i += 1) {
      await app.user.tab();
      expect(cmp().contains(document.activeElement)).toBe(true);
    }
  });

  test("Tabellen und Gruppen sind beschriftet; Fortschritt hat einen Namen", async () => {
    const app = await open();
    await app.user.click(within(cmp()).getByRole("button", { name: "Vergleichen" }));
    expect(within(cmp()).getByRole("progressbar", { name: "Verzeichnisse vergleichen" })).toBeInTheDocument();
    await app.runJobs();
    await waitFor(() => expect(within(cmp()).getByRole("group", { name: "Nach Zustand filtern" })).toBeInTheDocument());
    expect(within(cmp()).getByRole("table", { name: "Vergleichsergebnis" })).toBeInTheDocument();
    for (const header of ["Auswahl", "Pfad", "Links", "Zustand", "Rechts", "Aktion"]) {
      expect(within(cmp()).getByRole("columnheader", { name: header })).toBeInTheDocument();
    }
  });

  test("englische Oberfläche", async () => {
    const app = await setupApp({ left: LEFT, right: RIGHT, compareDemo: true, language: "en" });
    await app.user.keyboard("{Control>}{Shift>}D{/Shift}{/Control}");
    const dlg = screen.getByRole("dialog", { name: "Compare folders" });
    await app.user.click(within(dlg).getByRole("button", { name: "Compare" }));
    await app.runJobs();
    await waitFor(() => expect(within(dlg).getByRole("table", { name: "Comparison result" })).toBeInTheDocument());
    expect(within(dlg).getByText(/Incomplete result/)).toBeInTheDocument();
    await app.user.click(within(dlg).getByRole("button", { name: "Synchronize …" }));
    expect(await screen.findByRole("alertdialog", { name: "Synchronize" })).toBeInTheDocument();
  });
});
