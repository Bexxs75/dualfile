import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { activeTabOf } from "./app/state";
import { TRASH_PATH } from "./state/paths";
import { DOCS, DOWNLOADS, setupApp } from "./test-utils";

async function trashTwo() {
  const app = await setupApp({ left: DOCS, right: DOWNLOADS });
  await app.base.trash([`${DOCS}/notizen.txt`, `${DOWNLOADS}/Datei2.txt`]);
  await app.runJobs();
  fireEvent.click(within(screen.getByRole("navigation", { name: "Orte" })).getByRole("treeitem", { name: /^Papierkorb/ }));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(TRASH_PATH));
  await screen.findByRole("grid", { name: "Papierkorb" });
  return app;
}

describe("Papierkorb-Ansicht", () => {
  test("Klick in der Seitenleiste öffnet die Ansicht als Tab; Einträge mit ursprünglichem Ort", async () => {
    const app = await trashTwo();
    expect(app.names(0).sort()).toEqual(["Datei2.txt", "notizen.txt"]);
    expect(within(app.pane(0)).getByRole("tab", { name: /Papierkorb/ })).toBeInTheDocument();
    expect(within(app.pane(0)).getByRole("gridcell", { name: DOCS })).toBeInTheDocument();
  });

  test("Wiederherstellen legt den Eintrag zurück; Schreiben in die Ansicht wird abgelehnt", async () => {
    const app = await trashTwo();
    await app.user.click(app.row(0, "notizen.txt"));
    await app.user.click(within(app.pane(0)).getByRole("button", { name: "Wiederherstellen" }));
    await waitFor(() => expect(app.names(0)).toEqual(["Datei2.txt"]));
    expect((await app.api.list(DOCS)).entries.map(e => e.name)).toContain("notizen.txt");
    await app.user.keyboard("{F7}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Entf fragt nach, Abbrechen ist vorbelegt und löscht nichts; Bestätigen löscht endgültig", async () => {
    const app = await trashTwo();
    await app.user.click(app.row(0, "Datei2.txt"));
    await app.user.keyboard("{Delete}");
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("nicht rückgängig");
    expect(within(dialog).getByRole("button", { name: "Abbrechen" })).toHaveFocus();
    await app.user.keyboard("{Enter}");
    expect(app.names(0)).toHaveLength(2);
    await app.user.click(app.row(0, "Datei2.txt"));
    await app.user.keyboard("{Delete}");
    await app.user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Endgültig löschen" }));
    await waitFor(() => expect(app.names(0)).toEqual(["notizen.txt"]));
  });

  test("Kontextmenü: Ursprünglichen Ort öffnen zeigt den Ordner in der anderen Seite", async () => {
    const app = await trashTwo();
    fireEvent.contextMenu(app.row(0, "notizen.txt"));
    await app.user.click(screen.getByRole("menuitem", { name: "Ursprünglichen Ort öffnen" }));
    await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 1).path).toBe(DOCS));
  });

  test("Leerer Papierkorb zeigt den Leerzustand", async () => {
    await setupApp();
    fireEvent.click(within(screen.getByRole("navigation", { name: "Orte" })).getByRole("treeitem", { name: /^Papierkorb/ }));
    expect(await screen.findByText("Der Papierkorb ist leer")).toBeInTheDocument();
  });
});
