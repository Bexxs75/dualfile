import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { DOCS, DOWNLOADS, HOME, setupApp } from "../test-utils";
import { activeTabOf } from "../app/state";
import { archiveRootOf } from "../state/paths";
import { mapKey } from "../state/keymap";

afterEach(() => vi.restoreAllMocks());
const menu = () => within(screen.getByRole("menu", { name: "Kontextmenü" }));

test("file actions are grouped, use effective shortcuts and retain the delete confirmation", async () => {
  const app = await setupApp({ left: DOCS, settings: { keybindings: { copy: "Ctrl+J" } } });
  fireEvent.contextMenu(app.row(0, "notizen.txt"));
  expect(menu().getAllByRole("menuitem").map(e => e.getAttribute("aria-label"))).toEqual([
    "Öffnen", "Ansehen", "Bearbeiten", "Kopieren", "Verschieben", "Packen …", "Umbenennen", "Mehrfach-Umbenennen …", "Pfad kopieren", "Eigenschaften", "In den Papierkorb", "Endgültig löschen",
  ]);
  expect(menu().getAllByRole("separator")).toHaveLength(6);
  expect(menu().getByRole("menuitem", { name: "Kopieren" })).toHaveTextContent("Strg+J");
  expect(mapKey({ key: "j", ctrlKey: true }, "list", { copy: "Ctrl+J" })).toBe("copy");
  const permanent = menu().getByRole("menuitem", { name: "Endgültig löschen" });
  expect(permanent).toHaveClass("danger");
  expect(permanent).toHaveTextContent("Umsch+Entf");
  await app.user.click(permanent);
  expect(screen.getByRole("alertdialog")).toBeVisible();
});

test("folder, multiple selection and parent row show only applicable actions", async () => {
  const app = await setupApp();
  fireEvent.contextMenu(app.row(0, "Dokumente"));
  expect(menu().queryByRole("menuitem", { name: "Ansehen" })).toBeNull();
  expect(menu().getByRole("menuitem", { name: "Zu Orten hinzufügen" })).toBeEnabled();
  await app.user.keyboard("{Escape}");
  await app.user.click(app.row(0, "Dokumente"));
  await app.user.keyboard("{Control>}");
  await app.user.click(app.row(0, "Downloads"));
  await app.user.keyboard("{/Control}");
  fireEvent.contextMenu(app.row(0, "Dokumente"));
  const rename = menu().getByRole("menuitem", { name: "Umbenennen" });
  expect(rename).toBeDisabled();
  expect(rename).toHaveAccessibleDescription("Bei mehreren: Mehrfach-Umbenennen");
  expect(menu().getByRole("menuitem", { name: "Eigenschaften" })).toHaveAccessibleDescription("Nur ein Ziel");
  expect(menu().getByText("2 markiert")).toBeVisible();
  expect(menu().queryByRole("menuitem", { name: "Zu Orten hinzufügen" })).toBeNull();
  await app.user.keyboard("{Home}");
  expect(menu().getByRole("menuitem", { name: "Kopieren" })).toHaveFocus();
  await app.user.keyboard("{End}");
  expect(menu().getByRole("menuitem", { name: "Endgültig löschen" })).toHaveFocus();
  await app.user.keyboard("{ArrowDown}");
  expect(menu().getByRole("menuitem", { name: "Kopieren" })).toHaveFocus();
  await app.user.keyboard("{Tab}");
  fireEvent.contextMenu(app.row(0, ".."));
  expect(menu().getAllByRole("menuitem")).toHaveLength(1);
  expect(menu().getByRole("menuitem", { name: "Öffnen" })).toBeEnabled();
});

test("archive disables writes with reasons and reuses the existing extraction dialog", async () => {
  const app = await setupApp({ left: archiveRootOf(`${DOWNLOADS}/rezept-paket.tar.gz`), right: DOCS });
  fireEvent.contextMenu(app.row(0, "README.md"));
  for (const name of ["Bearbeiten", "Verschieben", "Umbenennen", "In den Papierkorb", "Endgültig löschen"]) {
    expect(menu().getByRole("menuitem", { name })).toBeDisabled();
    expect(menu().getByRole("menuitem", { name })).toHaveAccessibleDescription("Archiv ist schreibgeschützt");
  }
  expect(menu().getByRole("menuitem", { name: "Ansehen" })).toBeEnabled();
  await app.user.click(menu().getByRole("menuitem", { name: "Entpacken …" }));
  expect(screen.getByRole("dialog", { name: "Entpacken" })).toBeVisible();
});

test("server trash is disabled before a backend call", async () => {
  const app = await setupApp({ left: DOCS });
  act(() => app.runtime.store.dispatch({ type: "window", w: 0, listing: { path: "sftp:test!/docs", entries: app.runtime.store.getState().listings[0].entries }, update: w => ({ ...w, tabs: w.tabs.map(t => ({ ...t, path: "sftp:test!/docs" })) }) }));
  fireEvent.contextMenu(app.row(0, "notizen.txt"));
  const trash = menu().getByRole("menuitem", { name: "In den Papierkorb" });
  expect(trash).toHaveAttribute("aria-disabled", "true");
  expect(trash).toHaveAccessibleDescription("Ein Server hat keinen Papierkorb");
  await app.user.click(trash);
  expect(app.runtime.store.getState().dialog).toBeNull();
  expect(menu().getByRole("menuitem", { name: "Endgültig löschen" })).toBeEnabled();
});

test("background offers checked hidden state, select all and shared folder tools", async () => {
  const app = await setupApp({ left: DOCS });
  const open = () => fireEvent.contextMenu(within(app.pane(0)).getByRole("grid"));
  open();
  const before = app.runtime.store.getState().showHidden;
  expect(menu().getByRole("menuitemcheckbox")).toHaveAttribute("aria-checked", String(before));
  await app.user.click(menu().getByRole("menuitemcheckbox"));
  open();
  expect(menu().getByRole("menuitemcheckbox")).toHaveAttribute("aria-checked", String(!before));
  expect(menu().getByRole("menuitem", { name: "Alles auswählen" })).toHaveTextContent("Strg+A");
  await app.user.click(menu().getByRole("menuitem", { name: "Speicherbelegung …" }));
  expect(screen.getByRole("dialog")).toHaveTextContent(DOCS);
});

test("measured menu height can exceed 310px and is clamped to viewport on resize", async () => {
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(300);
  const app = await setupApp({ left: DOCS });
  fireEvent.contextMenu(app.row(0, "notizen.txt"), { clientX: 1000, clientY: 700 });
  const el = screen.getByRole("menu");
  expect(el.style.top).toBe(`${Math.max(4, window.innerHeight - 604)}px`);
  expect(el.style.left).toBe(`${window.innerWidth - 304}px`);
  fireEvent(window, new Event("resize"));
  expect(el.style.top).toBe(`${Math.max(4, window.innerHeight - 604)}px`);
});

test("tools announce disabled reasons and do not activate; news is in Info & Help", async () => {
  const app = await setupApp();
  await app.user.click(app.row(0, ".."));
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  const rename = screen.getByRole("menuitem", { name: "Mehrfach-Umbenennen …" });
  expect(rename).toHaveAttribute("aria-disabled", "true");
  expect(rename).toHaveTextContent("Erst Dateien markieren");
  await app.user.click(rename);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByRole("menuitem", { name: "Neuigkeiten" })).toBeNull();
  await app.user.keyboard("{Escape}");
  act(() => { void app.runtime.controller.openSettings("info"); });
  await app.user.click(await screen.findByRole("tab", { name: "Info & Hilfe" }));
  await app.user.click(screen.getByRole("button", { name: "Neuigkeiten" }));
  expect(screen.getByRole("dialog", { name: /Neu in Dualfile/ })).toBeVisible();
});

test("file and sidebar path copying share the controller and toast", async () => {
  const app = await setupApp({ left: DOCS, settings: { bookmarks: [{ name: "Testort", path: DOCS, order: 0, color: "#e0a45c" }] } });
  const copy = vi.spyOn(app.runtime.controller, "copyPath");
  fireEvent.contextMenu(app.row(0, "notizen.txt"));
  await app.user.click(menu().getByRole("menuitem", { name: "Pfad kopieren" }));
  expect(copy).toHaveBeenLastCalledWith(0);
  await waitFor(() => expect(screen.getByText("Pfad kopiert.")).toBeVisible());
  fireEvent.contextMenu(screen.getByRole("treeitem", { name: /Testort/ }));
  await app.user.click(screen.getByRole("menuitem", { name: "Pfad kopieren" }));
  expect(copy).toHaveBeenLastCalledWith(undefined, DOCS);
  expect(await navigator.clipboard.readText()).toBe(DOCS);
});

test.each(["ContextMenu", "F10"])("trash row menu supports %s and background emptying", async key => {
  const app = await setupApp({ left: DOCS });
  await app.base.trash([`${DOCS}/notizen.txt`]);
  await app.runJobs();
  fireEvent.click(screen.getByRole("treeitem", { name: /^Papierkorb/ }));
  const grid = await screen.findByRole("grid", { name: "Papierkorb" });
  await waitFor(() => expect(app.names(0)).toContain("notizen.txt"));
  fireEvent.keyDown(grid, { key, shiftKey: key === "F10" });
  expect(menu().getByRole("menuitem", { name: "Wiederherstellen" })).toHaveFocus();
  expect(menu().getByRole("menuitem", { name: "Endgültig löschen …" })).toHaveClass("danger");
  await app.user.keyboard("{Escape}");
  expect(grid).toHaveFocus();
  fireEvent.contextMenu(grid);
  await app.user.click(menu().getByRole("menuitem", { name: "Papierkorb leeren …" }));
  expect(screen.getByRole("alertdialog")).toBeVisible();
  expect(activeTabOf(app.runtime.store.getState(), 0).path).not.toBe(HOME);
});

test("empty trash keeps its disabled background menu keyboard-accessible", async () => {
  const app = await setupApp();
  fireEvent.click(screen.getByRole("treeitem", { name: /^Papierkorb/ }));
  const grid = await screen.findByRole("grid", { name: "Papierkorb" });
  await screen.findByText("Der Papierkorb ist leer");
  fireEvent.contextMenu(grid);
  expect(screen.getByRole("menu")).toHaveFocus();
  expect(menu().getByRole("menuitem", { name: /Papierkorb leeren/ })).toBeDisabled();
  await app.user.keyboard("{Escape}");
  expect(screen.queryByRole("menu")).toBeNull();
  expect(grid).toHaveFocus();
});
