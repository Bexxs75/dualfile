import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import type { DualfileApi } from "./ipc/api";
import type { RegexResult } from "./ipc/types";
import { DOCS, setupApp } from "./test-utils";

type App = Awaited<ReturnType<typeof setupApp>>;

const namesIn = async (api: DualfileApi, dir: string) => (await api.list(dir)).entries.map((entry) => entry.name).sort();

/** Linkes Fenster auf Dokumente, alle Dateien markiert, Strg+M gedrückt. */
async function openAll(app: App) {
  await app.user.click(app.row(0, "notizen.txt"));
  await app.user.keyboard("{Control>}a{/Control}");
  await app.user.keyboard("{Control>}m{/Control}");
  return await screen.findByRole("dialog", { name: /Mehrfach-Umbenennen/ });
}
const field = (label: string) => screen.getByLabelText(label);
const setField = (label: string, value: string) => fireEvent.change(field(label), { target: { value } });
const previewRows = () => within(screen.getByRole("table", { name: "Vorschau" })).getAllByRole("row").slice(1);
const newNames = () => previewRows().map((row) => within(row).getAllByRole("cell")[1]?.textContent);
const summary = () => within(screen.getByRole("dialog")).getByRole("status").textContent;
const alertIn = () => within(screen.getByRole("dialog")).getByRole("alert");
const listNames = (app: App) => app.names(0).filter((name) => name !== "..");
const renameButton = () => screen.getByRole("button", { name: "Umbenennen" });

describe("Strg+M öffnet den Dialog nur mit gültiger Auswahl", () => {
  test("mit Markierung: Dialog mit Vorschau aller markierten Einträge in Listenreihenfolge", async () => {
    const app = await setupApp({ left: DOCS });
    const dialog = await openAll(app);
    expect(dialog).toHaveAccessibleName(/7 Einträge in „Dokumente“/);
    expect(previewRows()).toHaveLength(7);
    expect(within(previewRows()[0]!).getAllByRole("cell")[0]).toHaveTextContent(listNames(app)[0]!);
    expect(summary()).toBe("0 werden umbenannt, 7 unverändert, 0 Fehler");
  });

  test("ohne Markierung gilt der Fokuseintrag", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Control>}m{/Control}");
    expect(await screen.findByRole("dialog", { name: /1 Eintrag in „Dokumente“/ })).toBeInTheDocument();
    expect(previewRows()).toHaveLength(1);
  });

  test("Fokus auf „..“ ohne Markierung: nichts öffnet sich", async () => {
    const app = await setupApp({ left: DOCS });
    expect(app.focused(0)).toBe("..");
    await app.user.keyboard("{Control>}m{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Strg+A markiert nie „..“: auch bei allem markiert fehlt „..“ in der Vorschau", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    expect(previewRows().some((row) => row.textContent?.startsWith(".."))).toBe(false);
  });

  test("in einem Eingabefeld (Filter) löst Strg+M nichts aus", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "notizen.txt"));
    const filter = app.pane(0).querySelector<HTMLInputElement>("input.filter");
    expect(filter).not.toBeNull();
    await app.user.click(filter!);
    expect(filter).toHaveFocus();
    await app.user.keyboard("{Control>}m{/Control}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Toolbar-Knopf öffnet den Dialog, ohne gültige Auswahl nicht", async () => {
    const app = await setupApp({ left: DOCS });
    const toolbar = within(screen.getByRole("toolbar", { name: /Werkzeugleiste|Toolbar/ }));
    await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(toolbar.getByRole("menuitem", { name: "Mehrfach-Umbenennen" }));
    expect(screen.queryByRole("dialog")).toBeNull(); // Fokus auf „..“
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(toolbar.getByRole("menuitem", { name: "Mehrfach-Umbenennen" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  test("Kontextmenü: Eintrag „Mehrfach-Umbenennen“ öffnet den Dialog; auf „..“ fehlt er", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "..") });
    const upMenu = await screen.findByRole("menu", { name: "Kontextmenü" });
    expect(within(upMenu).queryByRole("menuitem", { name: "Mehrfach-Umbenennen" })).toBeNull();
    await app.user.keyboard("{Escape}");
    await app.user.pointer({ keys: "[MouseRight]", target: app.row(0, "notizen.txt") });
    const menu = await screen.findByRole("menu", { name: "Kontextmenü" });
    await app.user.click(within(menu).getByRole("menuitem", { name: "Mehrfach-Umbenennen" }));
    expect(await screen.findByRole("dialog", { name: /Mehrfach-Umbenennen/ })).toBeInTheDocument();
  });
});

describe("Dialog: Vorschau und Sperre", () => {
  test("Maske ändert die Vorschau, Zusammenfassung zählt", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Maske für den Namen", "Doc_[C]");
    setField("Stellen", "2");
    expect(newNames()).toEqual(listNames(app).map((name, i) => `Doc_0${i + 1}.${name.split(".").pop()}`));
    expect(summary()).toBe("7 werden umbenannt, 0 unverändert, 0 Fehler");
    expect(renameButton()).toBeEnabled();
  });

  test("Fehler (unbekannter Platzhalter) wird hervorgehoben und sperrt „Umbenennen“", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Maske für den Namen", "[Q]");
    expect(summary()).toBe("0 werden umbenannt, 0 unverändert, 7 Fehler");
    expect(previewRows()[0]).toHaveAttribute("data-status", "error");
    expect(previewRows()[0]).toHaveTextContent("Unbekannter Platzhalter: [Q]");
    expect(renameButton()).toBeDisabled();
  });

  test("nichts zu tun: „Umbenennen“ ist gesperrt", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    expect(renameButton()).toBeDisabled();
  });

  test("Kollision mit nicht ausgewähltem Eintrag ist ein Fehler, Tausch innerhalb der Auswahl nicht", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "Bericht 2026-01.txt"));
    await app.user.keyboard("{Control>}m{/Control}");
    await screen.findByRole("dialog");
    setField("Maske für den Namen", "notizen");
    expect(previewRows()[0]).toHaveAttribute("data-status", "error");
    expect(previewRows()[0]).toHaveTextContent("Der Name ist bereits vorhanden.");
    expect(renameButton()).toBeDisabled();
  });

  test("Tausch zweier Dateien ist freigegeben und vertauscht die Inhalte tatsächlich", async () => {
    const app = await setupApp({ left: DOCS });
    await app.user.click(app.row(0, "Bericht 2026-01.txt"));
    await app.user.keyboard("{Control>}");
    await app.user.click(app.row(0, "Bericht 2026-02.txt"));
    await app.user.keyboard("{/Control}");
    expect(app.selected(0)).toEqual(["Bericht 2026-01.txt", "Bericht 2026-02.txt"]);
    const sizes = async () => Object.fromEntries((await app.api.list(DOCS)).entries.map((e) => [e.name, e.size]));
    const before = await sizes();
    await app.user.keyboard("{Control>}m{/Control}");
    await screen.findByRole("dialog");
    setField("Maske für den Namen", "Bericht 2026-0[C]");
    setField("Start", "2");
    setField("Schrittweite", "-1");
    expect(newNames()).toEqual(["Bericht 2026-02.txt", "Bericht 2026-01.txt"]);
    expect(previewRows().map((row) => row.getAttribute("data-status"))).toEqual(["ok", "ok"]);
    expect(renameButton()).toBeEnabled();
    await app.user.click(renameButton());
    await app.runJobs();
    await waitFor(async () => expect((await sizes())["Bericht 2026-01.txt"]).toBe(before["Bericht 2026-02.txt"]));
    expect((await sizes())["Bericht 2026-02.txt"]).toBe(before["Bericht 2026-01.txt"]);
  });

  test("ungültiger Zähler zeigt eine Meldung und sperrt", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Maske für den Namen", "x[C]");
    setField("Stellen", "");
    expect(alertIn()).toHaveTextContent("Zähler: ganze Zahlen eingeben");
    expect(renameButton()).toBeDisabled();
  });

  test("Standardknopf ist „Abbrechen“; Enter in einem Feld benennt nie um", async () => {
    const app = await setupApp({ left: DOCS });
    const spy = vi.spyOn(app.runtime.controller.api, "renameMany");
    await openAll(app);
    setField("Maske für den Namen", "Neu_[N]");
    expect(screen.getByRole("button", { name: "Abbrechen" })).toHaveClass("primary");
    expect(renameButton()).not.toHaveClass("primary");
    field("Maske für den Namen").focus();
    await app.user.keyboard("{Enter}");
    expect(spy).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await app.user.click(screen.getByRole("button", { name: "Abbrechen" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  test("Esc schließt den Dialog ohne Umbenennen", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("Platzhalter-Menü", () => {
  test("Einfügen an der Cursorstelle der zuletzt benutzten Maske; Esc schließt nur das Menü", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    const nameInput = field("Maske für den Namen") as HTMLInputElement;
    nameInput.focus();
    nameInput.setSelectionRange(1, 1); // „[|N]“ ... Cursor nach der Klammer
    await app.user.click(screen.getByRole("button", { name: /Platzhalter einfügen/ }));
    const menu = screen.getByRole("menu", { name: "Platzhalter" });
    expect(within(menu).getAllByRole("menuitem")[0]).toHaveFocus();
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("dialog")).toBeInTheDocument(); // Dialog bleibt
    await app.user.click(screen.getByRole("button", { name: /Platzhalter einfügen/ }));
    await app.user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: /\[Y\]/ }));
    expect((field("Maske für den Namen") as HTMLInputElement).value).toBe("[[Y]N]");
  });
});

describe("Regex über den Kern", () => {
  test("Gruppen: Vorschau kommt aus api.regexReplace, nach Entprellung", async () => {
    const app = await setupApp({ left: DOCS });
    const spy = vi.spyOn(app.runtime.controller.api, "regexReplace");
    await openAll(app);
    fireEvent.change(field("Suchart"), { target: { value: "regex" } });
    setField("Suchen", "Bericht (\\d+)-(\\d+)");
    setField("Ersetzen durch", "R$2.$1");
    expect(within(screen.getByRole("dialog")).getByRole("status")).toHaveTextContent("Die Vorschau wird berechnet");
    expect(renameButton()).toBeDisabled();
    await waitFor(() => expect(newNames()).toContain("R01.2026.txt"));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(newNames()).toEqual(expect.arrayContaining(["R02.2026.txt", "R10.2026.txt"]));
    expect(screen.getByText(/RE2/)).toBeInTheDocument();
    expect(renameButton()).toBeEnabled();
  });

  test("ungültiger Ausdruck: Fehler in der Vorschau, Umbenennen gesperrt", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    fireEvent.change(field("Suchart"), { target: { value: "regex" } });
    setField("Suchen", "(");
    await waitFor(() => expect(alertIn()).toHaveTextContent("Regulärer Ausdruck:"));
    expect(summary()).toBe("0 werden umbenannt, 0 unverändert, 7 Fehler");
    expect(renameButton()).toBeDisabled();
  });

  test("veraltete Antworten werden verworfen (neuere Eingabe gewinnt, auch wenn die alte zuletzt eintrifft)", async () => {
    const pending: { pattern: string; resolve: (r: RegexResult[]) => void; names: string[] }[] = [];
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({
        ...api,
        regexReplace: (names, pattern) => new Promise<RegexResult[]>((resolve) => pending.push({ pattern, resolve, names })),
      }),
    });
    await openAll(app);
    fireEvent.change(field("Suchart"), { target: { value: "regex" } });
    setField("Suchen", "alt");
    await waitFor(() => expect(pending).toHaveLength(1));
    setField("Suchen", "neu");
    await waitFor(() => expect(pending).toHaveLength(2));
    const [first, second] = pending as [(typeof pending)[number], (typeof pending)[number]];
    expect(first.pattern).toBe("alt");
    expect(second.pattern).toBe("neu");
    await act(async () => second.resolve(second.names.map((name) => ({ kind: "ok", name: `NEU_${name}` }))));
    await waitFor(() => expect(newNames()[0]).toMatch(/^NEU_/));
    await act(async () => first.resolve(first.names.map((name) => ({ kind: "ok", name: `ALT_${name}` }))));
    expect(newNames()[0]).toMatch(/^NEU_/);
    expect(renameButton()).toBeEnabled();
  });

  test("während einer neuen Anfrage ist „Umbenennen“ gesperrt (nie mit veralteter Vorschau)", async () => {
    const pending: ((r: RegexResult[]) => void)[] = [];
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({ ...api, regexReplace: () => new Promise<RegexResult[]>((resolve) => pending.push(resolve)) }),
    });
    await openAll(app);
    fireEvent.change(field("Suchart"), { target: { value: "regex" } });
    setField("Suchen", "x");
    await waitFor(() => expect(pending).toHaveLength(1));
    expect(renameButton()).toBeDisabled();
  });
});

describe("Umbenennen und Rückgängig", () => {
  test("renameMany bekommt nur geänderte Einträge; danach heißen die Dateien neu und Strg+Z nimmt es zurück", async () => {
    const app = await setupApp({ left: DOCS });
    const spy = vi.spyOn(app.runtime.controller.api, "renameMany");
    const before = await namesIn(app.api, DOCS);
    await openAll(app);
    setField("Suchen", "Bericht");
    setField("Ersetzen durch", "Report");
    expect(summary()).toBe("3 werden umbenannt, 4 unverändert, 0 Fehler");
    await app.user.click(renameButton());
    await app.runJobs();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(DOCS, [
      { oldName: "Bericht 2026-01.txt", newName: "Report 2026-01.txt" },
      { oldName: "Bericht 2026-02.txt", newName: "Report 2026-02.txt" },
      { oldName: "Bericht 2026-10.txt", newName: "Report 2026-10.txt" },
    ]);
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(app.names(0)).toContain("Report 2026-01.txt"));
    expect(await namesIn(app.api, DOCS)).not.toContain("Bericht 2026-01.txt");
    expect(await screen.findByText(/3 Einträge umbenannt/)).toBeInTheDocument();

    await app.user.keyboard("{Control>}z{/Control}");
    await app.runJobs();
    await waitFor(async () => expect(await namesIn(app.api, DOCS)).toEqual(before));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/3 rückgängig gemacht/)).toBeInTheDocument();
  });

  test("Rückgängig bei Teilfehler: erfolgreiche werden zurückbenannt, der Rest wird gemeldet", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Suchen", "Bericht");
    setField("Ersetzen durch", "Report");
    await app.user.click(renameButton());
    await app.runJobs();
    await waitFor(() => expect(app.names(0)).toContain("Report 2026-02.txt"));
    const renameId = (await app.api.undoList())[0]!.id;
    // Außen verschwindet ein umbenannter Eintrag: das Zurückbenennen scheitert für ihn allein.
    await app.base.trash([`${DOCS}/Report 2026-02.txt`]);
    await app.runJobs();
    act(() => { void app.runtime.controller.undo(renameId); });
    await app.runJobs();
    expect(await screen.findByText(/2 von 3 zurückgenommen, 1 nicht/)).toBeInTheDocument();
    const after = await namesIn(app.api, DOCS);
    expect(after).toContain("Bericht 2026-01.txt");
    expect(after).toContain("Bericht 2026-10.txt");
    expect(after).not.toContain("Report 2026-01.txt");
  });

  test("Teilfehler beim Umbenennen: nur die erfolgreichen kommen auf den Rückgängig-Stapel, der Rest wird gemeldet", async () => {
    const app = await setupApp({
      left: DOCS,
      wrap: (api) => ({
        ...api,
        // Zwischen Vorschau und Lauf verschwindet eine Quelle von außen: sie scheitert allein.
        renameMany: async (folder, items) => {
          await api.rename(`${folder}/Bericht 2026-02.txt`, "weg.txt");
          return api.renameMany(folder, items);
        },
      }),
    });
    await openAll(app);
    setField("Suchen", "Bericht");
    setField("Ersetzen durch", "Report");
    await app.user.click(renameButton());
    await app.runJobs();
    expect(await screen.findByText(/2 umbenannt, 1 nicht umbenannt/)).toBeInTheDocument();
    const entry = (await app.api.undoList())[0];
    expect(entry?.kind).toBe("rename");
    act(() => { void app.runtime.controller.undo(entry!.id); });
    await app.runJobs();
    expect(await namesIn(app.api, DOCS)).toContain("Bericht 2026-01.txt");
    expect(await namesIn(app.api, DOCS)).toContain("Bericht 2026-10.txt");
  });

  test("Kern lehnt ab (Kollision entsteht von außen): Auftrag failed, nichts im Rückgängig-Stapel, Meldung", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Suchen", "Bericht 2026-01");
    setField("Ersetzen durch", "Zusammen");
    // Nach dem Öffnen entsteht der Zielname von außen.
    await app.base.rename(`${DOCS}/notizen.txt`, "Zusammen.txt");
    await app.user.click(renameButton());
    await app.runJobs();
    expect((await app.api.undoList())).toHaveLength(1);
    expect(await screen.findByText(/Nichts wurde umbenannt/)).toBeInTheDocument();
    expect(await namesIn(app.api, DOCS)).toContain("Bericht 2026-01.txt");
  });
});

describe("Voreinstellungen", () => {
  test("speichern, laden und löschen (nur Sitzung)", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    expect(screen.getByText(/nicht dauerhaft gespeichert/)).toBeInTheDocument();
    setField("Maske für den Namen", "Foto_[C]");
    setField("Stellen", "3");
    fireEvent.change(screen.getByLabelText("Groß-/Kleinschreibung"), { target: { value: "upper" } });
    expect(screen.getByRole("button", { name: "Speichern" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Laden" })).toBeDisabled();
    setField("Name der Voreinstellung", "Fotos");
    await app.user.click(screen.getByRole("button", { name: "Speichern" }));
    expect(app.runtime.store.getState().renamePresets.map((p) => p.name)).toEqual(["Fotos"]);

    setField("Maske für den Namen", "[N]");
    setField("Stellen", "1");
    fireEvent.change(screen.getByLabelText("Groß-/Kleinschreibung"), { target: { value: "keep" } });
    const select = screen.getByLabelText("Gespeicherte Voreinstellung");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual(["(keine)", "Fotos"]);
    expect((select as HTMLSelectElement).value).toBe("Fotos"); // frisch gespeichert ist ausgewählt
    await app.user.click(screen.getByRole("button", { name: "Laden" }));
    expect((field("Maske für den Namen") as HTMLInputElement).value).toBe("Foto_[C]");
    expect((field("Stellen") as HTMLInputElement).value).toBe("3");
    expect((screen.getByLabelText("Groß-/Kleinschreibung") as HTMLSelectElement).value).toBe("upper");

    await app.user.click(screen.getByRole("button", { name: "Löschen" }));
    expect(app.runtime.store.getState().renamePresets).toEqual([]);
  });

  test("Voreinstellungen überleben das Schließen und Öffnen des Dialogs", async () => {
    const app = await setupApp({ left: DOCS });
    await openAll(app);
    setField("Maske für den Namen", "A_[N]");
    setField("Name der Voreinstellung", "Vorne");
    await app.user.click(screen.getByRole("button", { name: "Speichern" }));
    await app.user.click(screen.getByRole("button", { name: "Abbrechen" }));
    await app.user.keyboard("{Control>}m{/Control}");
    await screen.findByRole("dialog");
    expect(within(screen.getByLabelText("Gespeicherte Voreinstellung")).getByRole("option", { name: "Vorne" })).toBeInTheDocument();
  });

  test("gespeicherte Rohdaten werden vor dem Übernehmen geprüft", async () => {
    const app = await setupApp({ left: DOCS });
    app.runtime.controller.setRenamePresets([{ name: "kaputt", rules: { nameMask: 1 } }, "x"]);
    expect(app.runtime.store.getState().renamePresets).toEqual([]);
  });
});

describe("Fokusfalle und Beschriftung", () => {
  test("Tab und Umschalt+Tab bleiben im Dialog; Fokus beginnt in der Namensmaske und kehrt beim Schließen zurück", async () => {
    const app = await setupApp({ left: DOCS });
    const dialog = await openAll(app);
    expect(field("Maske für den Namen")).toHaveFocus();
    const focusables = [...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled])')];
    const last = focusables[focusables.length - 1]!;
    last.focus();
    await app.user.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(focusables[0]);
    await app.user.tab({ shift: true });
    expect(document.activeElement).toBe(last);
    await app.user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.pane(0).contains(document.activeElement)).toBe(true);
  });

  test("Dialog ist ein modaler dialog mit Namen; Fehler stehen als Text, nicht nur als Farbe", async () => {
    const app = await setupApp({ left: DOCS });
    const dialog = await openAll(app);
    expect(dialog).toHaveAttribute("aria-modal", "true");
    setField("Maske für den Namen", "[Q]");
    expect(previewRows()[0]).toHaveTextContent("Unbekannter Platzhalter");
    expect(screen.getByRole("table", { name: "Vorschau" })).toBeInTheDocument();
  });

  test("Englisch: Dialog, Knöpfe und Fehlertexte kommen aus dem englischen Wörterbuch", async () => {
    const app = await setupApp({ left: DOCS, language: "en" });
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.keyboard("{Control>}m{/Control}");
    const dialog = await screen.findByRole("dialog", { name: /Multi-rename – 1 item in “Dokumente”/ });
    fireEvent.change(within(dialog).getByLabelText("Name mask"), { target: { value: "[Q]" } });
    expect(within(dialog).getByRole("table", { name: "Preview" })).toHaveTextContent("Unknown placeholder: [Q]");
    expect(within(dialog).getByRole("status")).toHaveTextContent("0 will be renamed, 0 unchanged, 1 errors");
    expect(within(dialog).getByRole("button", { name: "Rename" })).toBeDisabled();
  });
});
