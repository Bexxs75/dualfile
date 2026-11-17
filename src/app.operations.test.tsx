import { act, createEvent, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import { CoreError, type DualfileApi } from "./ipc/api";
import { DOCS, DOWNLOADS, HOME, setupApp } from "./test-utils";

const sizeOf = async (api: DualfileApi, dir: string, name: string) =>
  (await api.list(dir)).entries.find((entry) => entry.name === name)?.size;
const namesIn = async (api: DualfileApi, dir: string) => (await api.list(dir)).entries.map((entry) => entry.name);

/** Rechts liegt `notizen.txt` (21 Bytes) schon, links ist es 1200 Bytes groß: ein Konflikt. */
async function conflictSetup() {
  const app = await setupApp({ left: DOCS, right: DOWNLOADS });
  await app.base.rename(`${DOWNLOADS}/Datei2.txt`, "notizen.txt");
  await waitFor(() => expect(app.names(1)).toContain("notizen.txt"));
  await app.user.click(app.row(0, "notizen.txt"));
  await app.user.keyboard("{F5}");
  await app.user.keyboard("{Enter}"); // OK im Übertragungsdialog
  await app.runJobs();
  return app;
}

describe("F5 / F6 Kopieren und Verschieben", () => {
  test("F5: Dialog zeigt Quelle, Ziel = anderes Fenster, Anzahl; Standardknopf bestätigt, danach liegt die Datei im Ziel", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}");
    const dlg = app.dialog();
    expect(dlg).toHaveAccessibleName("Kopieren");
    expect(within(dlg).getByRole("textbox")).toHaveValue(DOWNLOADS);
    expect(within(dlg).getByText(/1 Element von „\/home\/demo\/Dokumente“ nach:/)).toBeInTheDocument();
    expect(within(dlg).getByText("Gesamtgröße: 1,2 KB")).toBeInTheDocument();
    expect(within(dlg).getByRole("textbox")).toHaveFocus();
    await app.user.keyboard("{Enter}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).toContain("notizen.txt"));
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(1200);
    expect(app.names(0)).toContain("notizen.txt");
  });

  test("F5 mit Fokus auf „..“ und ohne Markierung tut nichts", async () => {
    const app = await setupApp({ left: DOCS });
    expect(app.focused(0)).toBe("..");
    await app.user.keyboard("{F5}{F6}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.runtime.store.getState().jobs.jobs).toHaveLength(0);
  });

  test("das Ziel im Dialog lässt sich ändern; derselbe Ordner wird abgelehnt", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}");
    const input = within(app.dialog()).getByRole("textbox");
    await app.user.clear(input);
    await app.user.type(input, DOCS);
    expect(within(app.dialog()).getByRole("alert")).toHaveTextContent("Quelle und Ziel sind derselbe Ordner.");
    expect(within(app.dialog()).getByRole("button", { name: "Kopieren" })).toBeDisabled();
    await app.user.clear(input);
    await app.user.type(input, `${HOME}/Bilder{Enter}`);
    await app.runJobs();
    expect(await namesIn(app.api, `${HOME}/Bilder`)).toContain("notizen.txt");
  });

  test("F6 verschiebt: weg aus der Quelle, da im Ziel", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F6}");
    expect(app.dialog()).toHaveAccessibleName("Verschieben");
    await app.user.keyboard("{Enter}");
    await app.runJobs();
    await waitFor(() => expect(app.names(0)).not.toContain("notizen.txt"));
    await waitFor(() => expect(app.names(1)).toContain("notizen.txt"));
  });

  test("Konflikt: Standardknopf ist Überspringen, ohne Antwort wird nichts überschrieben", async () => {
    const app = await conflictSetup();
    const dlg = app.alertDialog();
    expect(dlg).toHaveAccessibleName("Das Ziel existiert bereits");
    expect(within(dlg).getByRole("button", { name: "Überspringen" })).toHaveFocus();
    expect(screen.getByText("Wartet auf Eingabe")).toBeVisible();
    // Ohne Antwort bleibt das Ziel unangetastet.
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(21);
    await app.user.keyboard("{Enter}");
    await app.runJobs();
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByText("0 kopiert, 1 übersprungen")).toBeInTheDocument();
  });

  test("Konflikt: Überschreiben nur durch ausdrücklichen Klick", async () => {
    const app = await conflictSetup();
    await app.user.click(within(app.alertDialog()).getByRole("button", { name: "Überschreiben" }));
    await app.runJobs();
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(1200);
  });

  test("Konflikt: Esc bricht den Auftrag ab und überschreibt nichts", async () => {
    const app = await conflictSetup();
    await app.user.keyboard("{Escape}");
    await app.runJobs();
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(within(screen.getByRole("region", { name: "Aufträge" })).getByText("Abgebrochen")).toBeInTheDocument();
  });

  test("Konflikt: Beide behalten legt eine Kopie mit Namenszusatz an", async () => {
    const app = await conflictSetup();
    await app.user.click(within(app.alertDialog()).getByRole("button", { name: "Beide behalten" }));
    await app.runJobs();
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(await sizeOf(app.api, DOWNLOADS, "notizen (2).txt")).toBe(1200);
  });

  test("Konflikt: „Für alle“ gilt für weitere Konflikte desselben Auftrags", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.base.rename(`${DOWNLOADS}/Datei2.txt`, "notizen.txt");
    await app.base.rename(`${DOWNLOADS}/Datei1.txt`, "Änderungen.txt");
    await waitFor(() => expect(app.names(1)).toContain("Änderungen.txt"));
    await app.user.click(app.row(0, "Änderungen.txt"));
    await app.user.keyboard("{Control>}");
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{/Control}{F5}{Enter}");
    await app.runJobs();
    const dlg = app.alertDialog();
    await app.user.click(within(dlg).getByLabelText("Für alle"));
    await app.user.click(within(dlg).getByRole("button", { name: "Überspringen" }));
    await app.runJobs();
    expect(screen.queryByRole("alertdialog")).toBeNull(); // der zweite Konflikt wurde nicht mehr gefragt
    expect(await sizeOf(app.api, DOWNLOADS, "notizen.txt")).toBe(21);
    expect(await sizeOf(app.api, DOWNLOADS, "Änderungen.txt")).toBe(12); // Datei1.txt (12 Bytes) blieb unangetastet
  });

  test("Fehlerliste: „1 von 4 nicht kopiert“ mit Grund; Entfernen schließt den Auftrag", async () => {
    const app = await setupApp({ left: HOME, right: DOWNLOADS });
    await app.user.click(app.row(0, "Projekte"));
    await app.user.keyboard("{F5}{Enter}");
    await app.runJobs();
    const jobs = within(screen.getByRole("region", { name: "Aufträge" }));
    expect(await jobs.findByText("Mit Fehlern beendet")).toBeInTheDocument();
    expect(jobs.getByText(/^1 von \d+ nicht kopiert$/)).toBeInTheDocument();
    expect(jobs.getByText(/Grund: Zugriff verweigert/)).toBeInTheDocument();
    await app.user.click(jobs.getByRole("button", { name: "Entfernen" }));
    expect(screen.queryByRole("region", { name: "Aufträge" })).toBeNull();
  });

  test("Auftrag lässt sich abbrechen", async () => {
    const app = await setupApp({ left: HOME, right: DOWNLOADS });
    await app.user.click(app.row(0, "große-datei.bin"));
    await app.user.keyboard("{F5}{Enter}");
    const jobs = within(await screen.findByRole("region", { name: "Aufträge" }));
    expect(jobs.getByText("Wartet")).toBeInTheDocument();
    await app.user.click(jobs.getByRole("button", { name: "Auftrag abbrechen" }));
    await app.runJobs();
    expect(jobs.getByText("Abgebrochen")).toBeInTheDocument();
    expect(await namesIn(app.api, DOWNLOADS)).not.toContain("große-datei.bin");
  });

  test("Ziehen: Standard kopieren mit Hinweis, mit Umschalt verschieben; das Loslassen fragt noch einmal nach", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    fireEvent.dragStart(app.row(0, "notizen.txt"), { dataTransfer });
    fireEvent.dragOver(app.pane(1), { dataTransfer, clientX: 300, clientY: 200 });
    expect(await screen.findByText(`Kopieren nach ${DOWNLOADS}`)).toBeInTheDocument();
    expect(app.pane(1)).toHaveClass("drop");
    // jsdom kennt kein DragEvent und übernimmt shiftKey nicht aus den Optionen; deshalb von Hand setzen.
    const shiftOver = createEvent.dragOver(app.pane(1), { dataTransfer, clientX: 300, clientY: 200 });
    Object.defineProperty(shiftOver, "shiftKey", { value: true });
    fireEvent(app.pane(1), shiftOver);
    expect(await screen.findByText(`Verschieben nach ${DOWNLOADS}`)).toBeInTheDocument();
    const shiftDrop = createEvent.drop(app.pane(1), { dataTransfer });
    Object.defineProperty(shiftDrop, "shiftKey", { value: true });
    fireEvent(app.pane(1), shiftDrop);
    expect(app.dialog()).toHaveAccessibleName("Verschieben");
    expect(screen.queryByText(/Verschieben nach \//)).toBeNull();
    expect(await sizeOf(app.api, DOCS, "notizen.txt")).toBe(1200); // noch nichts passiert
  });

  test("Ziehen auf eine Ordnerzeile im selben Fenster: Hinweis und Hervorhebung, Ablegen nimmt diesen Ordner als Ziel; nicht auf sich selbst", async () => {
    const app = await setupApp({ left: HOME, right: DOWNLOADS });
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    fireEvent.dragStart(app.row(0, "Bilder"), { dataTransfer });
    // Auf sich selbst: kein Ordnerziel, kein Hinweis, Loslassen öffnet nichts.
    fireEvent.dragOver(app.row(0, "Bilder"), { dataTransfer, clientX: 50, clientY: 50 });
    expect(app.row(0, "Bilder")).not.toHaveClass("drop-into");
    expect(screen.queryByText(/Kopieren nach/)).toBeNull();
    fireEvent.drop(app.row(0, "Bilder"), { dataTransfer });
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.dragStart(app.row(0, "Bilder"), { dataTransfer });
    // WebKit braucht ein abgebrochenes dragenter, sonst kommt kein drop an (fireEvent liefert false = abgebrochen).
    expect(fireEvent.dragEnter(app.row(0, "Dokumente"), { dataTransfer, clientX: 50, clientY: 80 })).toBe(false);
    fireEvent.dragOver(app.row(0, "Dokumente"), { dataTransfer, clientX: 50, clientY: 80 });
    expect(await screen.findByText(`Kopieren nach ${DOCS}`)).toBeInTheDocument();
    expect(app.row(0, "Dokumente")).toHaveClass("drop-into");
    expect(app.pane(0)).not.toHaveClass("drop");
    // Weiter auf die freie Fläche des anderen Fensters: Ordnerziel fällt weg, dort gilt wieder der aktuelle Ordner.
    fireEvent.dragOver(app.pane(1), { dataTransfer, clientX: 300, clientY: 200 });
    expect(await screen.findByText(`Kopieren nach ${DOWNLOADS}`)).toBeInTheDocument();
    expect(app.row(0, "Dokumente")).not.toHaveClass("drop-into");
    fireEvent.dragOver(app.row(0, "Dokumente"), { dataTransfer, clientX: 50, clientY: 80 });
    fireEvent.drop(app.row(0, "Dokumente"), { dataTransfer });
    expect(app.dialog()).toHaveAccessibleName("Kopieren");
    await app.user.keyboard("{Enter}");
    await app.runJobs();
    expect(await namesIn(app.api, DOCS)).toContain("Bilder");
  });
});

describe("Papierkorb, Löschen, Rückgängig", () => {
  test("F8 legt ohne Dialog in den Papierkorb, Strg+Z holt es zurück", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F8}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await waitFor(() => expect(app.names(0)).not.toContain("notizen.txt"));
    expect(await screen.findByText("„notizen.txt“ liegt im Papierkorb. Rückgängig mit Strg+Z.")).toBeInTheDocument();
    await app.user.keyboard("{Control>}z{/Control}");
    await app.runJobs();
    await waitFor(() => expect(app.names(0)).toContain("notizen.txt"));
    expect(await screen.findByText("1 rückgängig gemacht")).toBeInTheDocument();
  });

  test("Entf wirkt auf die ganze Markierung", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.keyboard("{Control>}a{/Control}{Delete}");
    await waitFor(() => expect(app.names(0)).toEqual([".."]));
    expect(await screen.findByText("7 Elemente liegen im Papierkorb. Rückgängig mit Strg+Z.")).toBeInTheDocument();
    await app.user.keyboard("{Control>}z{/Control}");
    await app.runJobs();
    await waitFor(() => expect(app.names(0)).toHaveLength(8));
  });

  test("Papierkorb fällt aus: Rückfrage, Standard Abbrechen, nichts gelöscht", async () => {
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({ ...api, trash: async () => Promise.reject(new CoreError("kein Papierkorb", "io")) }),
    });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F8}");
    const dlg = await screen.findByRole("alertdialog");
    expect(within(dlg).getByText(/Papierkorb ist hier nicht verfügbar/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Abbrechen" })).toHaveFocus();
    await app.user.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(await namesIn(app.api, DOCS)).toContain("notizen.txt");
  });

  test("Endgültig löschen: Umfang und Warnung, Standard Abbrechen, falsches Token wirkungslos, richtiges löscht", async () => {
    const deletePermanent = vi.fn();
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({
        ...api,
        deletePermanent: (plan, token) => {
          deletePermanent(plan, token);
          return api.deletePermanent(plan, token);
        },
      }),
    });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Shift>}{F8}{/Shift}");
    const dlg = app.alertDialog();
    expect(dlg).toHaveAccessibleName("Endgültig löschen?");
    expect(within(dlg).getByText("1 Datei")).toBeInTheDocument();
    expect(within(dlg).getByText("Diese Löschung kann nicht rückgängig gemacht werden.")).toBeInTheDocument();
    expect(within(dlg).getByText("notizen.txt")).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Abbrechen" })).toHaveFocus();

    // Enter wählt den Standardknopf: abbrechen, nichts passiert
    await app.user.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(deletePermanent).not.toHaveBeenCalled();
    expect(await namesIn(app.api, DOCS)).toContain("notizen.txt");

    // Ein falsches (oder leeres) Token bewirkt nichts; der Dialog bleibt stehen.
    await app.user.keyboard("{Shift>}{F8}{/Shift}");
    let accepted = true;
    await act(async () => {
      accepted = (await app.runtime.controller.confirmDelete("falsch")) || (await app.runtime.controller.confirmDelete(""));
    });
    expect(accepted).toBe(false);
    expect(deletePermanent).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(await namesIn(app.api, DOCS)).toContain("notizen.txt");

    // Nur der ausdrückliche Klick löscht, und zwar mit genau dem Token des Plans.
    await app.user.click(within(app.alertDialog()).getByRole("button", { name: "Endgültig löschen" }));
    await waitFor(() => expect(deletePermanent).toHaveBeenCalledTimes(1));
    expect(deletePermanent.mock.calls[0]?.[1]).toBe(deletePermanent.mock.calls[0]?.[0].token);
    await waitFor(() => expect(app.names(0)).not.toContain("notizen.txt"));
    expect(await namesIn(app.api, DOCS)).not.toContain("notizen.txt");
  });

  test("Esc im Löschdialog löscht nichts", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Shift>}{Delete}{/Shift}");
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(await namesIn(app.api, DOCS)).toContain("notizen.txt");
  });

  test("Strg+Z nimmt Kopieren zurück; ohne Verlauf erscheint ein Hinweis", async () => {
    const app = await setupApp({ left: DOCS, right: DOWNLOADS });
    await app.user.keyboard("{Control>}z{/Control}");
    expect(await screen.findByText("Es gibt nichts zum Rückgängigmachen.")).toBeInTheDocument();
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F5}{Enter}");
    await app.runJobs();
    await app.user.keyboard("{Control>}z{/Control}");
    await app.runJobs();
    await waitFor(() => expect(app.names(1)).not.toContain("notizen.txt"));
  });
});

describe("Umbenennen und Neuer Ordner", () => {
  test("F2: Namensfehler stehen direkt am Feld, erst ein gültiger Name wird übernommen, Strg+Z macht es rückgängig", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{F2}");
    const dlg = app.dialog();
    const input = within(dlg).getByRole("textbox");
    expect(input).toHaveValue("notizen.txt");
    expect(within(dlg).queryByRole("alert")).toBeNull();
    await app.user.clear(input);
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Der Name darf nicht leer sein.");
    await app.user.type(input, "a/b");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Schrägstrich");
    await app.user.clear(input);
    await app.user.type(input, "Änderungen.txt{Enter}");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Der Name ist bereits vorhanden.");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(app.dialog()).toBeInTheDocument();
    await app.user.clear(input);
    await app.user.type(input, "neu.txt{Enter}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(app.names(0)).toContain("neu.txt"));
    expect(app.names(0)).not.toContain("notizen.txt");
    expect(app.focused(0)).toBe("neu.txt");
    expect(await screen.findByText("„notizen.txt“ heißt jetzt „neu.txt“.")).toBeInTheDocument();
    await app.user.keyboard("{Control>}z{/Control}");
    await app.runJobs();
    await waitFor(() => expect(app.names(0)).toContain("notizen.txt"));
    expect(app.names(0)).not.toContain("neu.txt");
  });

  test("Umschalt+F6 öffnet ebenfalls das Umbenennen; auf „..“ passiert nichts", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.keyboard("{F2}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await app.user.keyboard("{ArrowDown}{Shift>}{F6}{/Shift}");
    expect(app.dialog()).toHaveAccessibleName("Umbenennen");
  });

  test("F7 legt einen Ordner an und fokussiert ihn; vorhandener Name wird gemeldet", async () => {
    const app = await setupApp();
    await app.user.keyboard("{F7}");
    const dlg = app.dialog();
    expect(dlg).toHaveAccessibleName("Neuer Ordner");
    const input = within(dlg).getByRole("textbox");
    expect(input).toHaveFocus();
    await app.user.type(input, "Bilder{Enter}");
    expect(within(dlg).getByRole("alert")).toHaveTextContent("Der Name ist bereits vorhanden.");
    await app.user.clear(input);
    await app.user.type(input, "Neu{Enter}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(app.names(0)).toContain("Neu"));
    expect(app.focused(0)).toBe("Neu");
    expect(await namesIn(app.api, HOME)).toContain("Neu");
  });
});

describe("Kontextmenü", () => {
  test("Rechtsklick öffnet das Eintragsmenü ohne Ordnervergleich; Eigenschaften zeigt den Pfad", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "notizen.txt") });
    const menu = await screen.findByRole("menu", { name: "Kontextmenü" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "Öffnen",
      "Pfad kopieren",
      "Umbenennen",
      "Packen …",
      "Mehrfach-Umbenennen",
      "In den Papierkorb",
      "Eigenschaften",
    ]);
    expect(within(menu).getAllByRole("menuitem")[0]).toHaveFocus();
    expect(app.selected(0)).toEqual(["notizen.txt"]);
    await app.user.click(within(menu).getByRole("menuitem", { name: "Eigenschaften" }));
    expect(app.dialog()).toHaveAccessibleName("Eigenschaften");
    expect(within(app.dialog()).getByText(`${DOCS}/notizen.txt`)).toBeInTheDocument();
  });

  test("Menü per Tastatur: Pfeile, Esc schließt, Umbenennen öffnet den Dialog", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.keyboard("{ArrowDown}{Shift>}{F10}{/Shift}");
    const menu = await screen.findByRole("menu");
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(within(app.pane(0)).getByRole("grid")).toHaveFocus();
    await app.user.keyboard("{Shift>}{F10}{/Shift}");
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    await app.user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(app.dialog()).toHaveAccessibleName("Umbenennen");
    expect(menu).not.toBeInTheDocument();
  });

  test("In den Papierkorb aus dem Menü", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "notizen.txt") });
    await app.user.click(await screen.findByRole("menuitem", { name: "In den Papierkorb" }));
    await waitFor(() => expect(app.names(0)).not.toContain("notizen.txt"));
  });
});
