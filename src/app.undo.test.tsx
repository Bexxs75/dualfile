import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { DOCS, DOWNLOADS, setupApp } from "./test-utils";

afterEach(() => vi.useRealTimers());
const openHistory = async (app: Awaited<ReturnType<typeof setupApp>>) => {
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Verlauf" }));
  return within(screen.getByRole("dialog", { name: "Verlauf" }));
};

test("empty history, focus trap, Escape and return focus", async () => {
  const app = await setupApp();
  const trigger = screen.getByRole("button", { name: "Werkzeuge" });
  const panel = await openHistory(app);
  expect(panel.getByText("Noch keine Aktionen.")).toBeInTheDocument();
  const close = panel.getByRole("button", { name: "Schließen" });
  expect(close).toHaveFocus();
  await app.user.tab();
  expect(close).toHaveFocus();
  await app.user.tab({ shift: true });
  expect(close).toHaveFocus();
  await app.user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
});

test("move, rename and trash appear newest first; individual undo restores mock files", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.move([`${DOCS}/notizen.txt`], DOWNLOADS); });
  await app.runJobs();
  await act(async () => { await app.api.rename(`${DOCS}/Änderungen.txt`, "neu.txt"); await app.api.trash([`${DOCS}/neu.txt`]); });
  const panel = await openHistory(app);
  const rows = panel.getAllByRole("listitem");
  expect(rows.map((row) => row.querySelector(".undo-label")?.textContent)).toEqual(["Papierkorb: neu.txt", "Umbenennen: Änderungen.txt", "Verschieben: notizen.txt"]);
  expect(rows[0]!.querySelector("time")?.textContent).toMatch(/^\d{2}:\d{2}$/);
  await app.user.click(within(rows[2]!).getByRole("button"));
  expect(within(rows[0]!).getByRole("button")).toBeDisabled();
  expect(panel.getByRole("button", { name: "Auftrag abbrechen" })).toBeInTheDocument();
  await app.runJobs();
  expect((await app.api.list(DOCS)).entries.some((e) => e.name === "notizen.txt")).toBe(true);
  expect(within(rows[2]!).getByRole("button")).toBeDisabled();
  expect(rows[2]).toHaveClass("undone");
  expect(app.names(0)).toContain("notizen.txt");
});

test("changed and conflict errors are translated; partial success remains retryable", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.move([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`, `${DOCS}/Bericht 2026-01.txt`], DOWNLOADS); });
  await app.runJobs();
  await act(async () => {
    await app.api.rename(`${DOWNLOADS}/notizen.txt`, "changed.txt");
    await app.api.makeDir(DOCS, "Änderungen.txt");
  });
  const panel = await openHistory(app);
  await app.user.click(panel.getByRole("button", { name: /^Rückgängig: Verschieben:/ }));
  await app.runJobs();
  expect(panel.getByText("1 von 3 zurückgenommen, 2 nicht")).toBeInTheDocument();
  expect(panel.getByText("Der Eintrag wurde verändert")).toBeInTheDocument();
  expect(panel.getByText("Der Ursprungspfad ist belegt")).toBeInTheDocument();
  expect(panel.getByRole("button", { name: /^Rückgängig: Verschieben:/ })).toBeEnabled();
  expect(screen.queryByText(DOWNLOADS, { exact: false, selector: ".job-errors" })).toBeNull();
});

test("Ctrl/Cmd Z avoid text and dialog focus; Shift opens history", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.rename(`${DOCS}/notizen.txt`, "neu.txt"); });
  const spy = vi.spyOn(app.api, "undoRun");
  await app.user.keyboard("{Control>}f{/Control}");
  fireEvent.keyDown(document.activeElement!, { key: "z", ctrlKey: true });
  expect(spy).not.toHaveBeenCalled();
  await app.user.keyboard("{Escape}");
  fireEvent.keyDown(document.activeElement!, { key: "Z", ctrlKey: true, shiftKey: true });
  expect(screen.getByRole("dialog", { name: "Verlauf" })).toBeInTheDocument();
  fireEvent.keyDown(document.activeElement!, { key: "z", ctrlKey: true });
  expect(spy).not.toHaveBeenCalled();
  await app.user.keyboard("{Escape}");
  fireEvent.keyDown(document.activeElement!, { key: "z", metaKey: true });
  await app.runJobs();
  expect(spy).toHaveBeenCalledTimes(1);
  expect((await app.api.undoList())[0]?.undone).toBe(true);
});

test("toast replaces previous action and pauses its six seconds on hover and focus", async () => {
  const app = await setupApp({ left: DOCS });
  vi.useFakeTimers();
  await act(async () => { await app.api.rename(`${DOCS}/notizen.txt`, "neu.txt"); });
  expect(document.querySelectorAll(".undo-toast")).toHaveLength(1);
  act(() => vi.advanceTimersByTime(2000));
  await act(async () => { await app.api.makeDir(DOCS, "Ordner"); });
  const toast = document.querySelector<HTMLElement>(".undo-toast")!;
  expect(toast).toHaveTextContent("Ordner anlegen");
  expect(document.querySelectorAll(".undo-toast")).toHaveLength(1);
  fireEvent.mouseEnter(toast);
  act(() => vi.advanceTimersByTime(10000));
  expect(toast).toBeInTheDocument();
  act(() => within(toast).getByRole("button").focus());
  fireEvent.mouseLeave(toast);
  act(() => vi.advanceTimersByTime(10000));
  expect(toast).toBeInTheDocument();
  act(() => within(toast).getByRole("button").blur());
  act(() => vi.advanceTimersByTime(5999));
  expect(toast).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(1));
  expect(toast).not.toBeInTheDocument();
});

test("undo rejections never expose backend text", async () => {
  const app = await setupApp({ left: DOCS, wrap: (api) => ({ ...api, undoRun: async () => { throw new Error("/secret/system-error"); } }) });
  await act(async () => { await app.api.makeDir(DOCS, "Ordner"); });
  const panel = await openHistory(app);
  await app.user.click(panel.getByRole("button", { name: /^Rückgängig: Ordner anlegen:/ }));
  await waitFor(() => expect(panel.getByText("Dateisystemfehler")).toBeInTheDocument());
  expect(document.body).not.toHaveTextContent("/secret/system-error");
});

test("running undo can be cancelled from history and retried", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.trash([`${DOCS}/notizen.txt`, `${DOCS}/Änderungen.txt`]); });
  const panel = await openHistory(app);
  await app.user.click(panel.getByRole("button", { name: /^Rückgängig: Papierkorb:/ }));
  await app.user.click(panel.getByRole("button", { name: "Auftrag abbrechen" }));
  await app.runJobs();
  expect(panel.getByText("Rücknahme abgebrochen")).toBeInTheDocument();
  expect(panel.getByRole("button", { name: /^Rückgängig: Papierkorb:/ })).toBeEnabled();
  await app.user.click(panel.getByRole("button", { name: /^Rückgängig: Papierkorb:/ }));
  await app.runJobs();
  expect((await app.api.undoList())[0]?.undone).toBe(true);
});

test("English labels and all fixed error codes; backend labels are never displayed", async () => {
  const app = await setupApp({ language: "en", wrap: (api) => ({ ...api,
    undoList: async () => [{ id: 1, time: 0, kind: "move", label: "/private/backend-message", undone: false }],
    undoRun: async () => ({ jobId: 1, ok: false, done: 3, remaining: 2, cancelled: false, errors: ["changed", "conflict", "alreadyUndone", "notFound", "unsupported", "notLocal", "io"] }),
  }) });
  expect(document.querySelector(".undo-toast")).toBeNull();
  await app.user.click(screen.getByRole("button", { name: "Tools" }));
  await app.user.click(screen.getByRole("menuitem", { name: "History" }));
  const panel = within(screen.getByRole("dialog"));
  await app.user.click(panel.getByRole("button", { name: "Undo: Move" }));
  expect(panel.getByText("3 of 5 undone, 2 remaining")).toBeInTheDocument();
  expect(panel.getByRole("status").querySelectorAll("p")).toHaveLength(7);
  expect(document.body).not.toHaveTextContent("/private/backend-message");
});

test("Ctrl+N opens the connections dialog and Escape closes it", async () => {
  const app = await setupApp();
  fireEvent.keyDown(document.activeElement!, { key: "n", ctrlKey: true });
  expect(await screen.findByRole("dialog", { name: /Verbindungen/ })).toBeInTheDocument();
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog", { name: /Verbindungen/ })).toBeNull());
});

test("undo toast stays operable while a dialog is open, and Escape still closes the dialog afterwards", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => { await app.api.rename(`${DOCS}/notizen.txt`, "neu.txt"); });
  const toast = within(document.querySelector<HTMLElement>(".undo-toast")!);
  await app.user.click(screen.getByRole("button", { name: "Werkzeuge" }));
  await app.user.click(screen.getByRole("menuitem", { name: "Verlauf" }));
  expect(screen.getByRole("dialog", { name: "Verlauf" })).toBeInTheDocument();
  const button = toast.getByRole("button", { name: /Rückgängig/ });
  expect(button).toBeEnabled();
  expect(document.querySelector(".undo-toast")).not.toHaveAttribute("inert");
  const spy = vi.spyOn(app.api, "undoRun");
  await app.user.click(button);
  await app.runJobs();
  expect(spy).toHaveBeenCalledTimes(1);
  expect((await app.api.undoList())[0]?.undone).toBe(true);
  // Der Fokus liegt jetzt im Hinweis, außerhalb des Dialogs: Esc muss den Dialog trotzdem schließen.
  act(() => button.focus());
  await app.user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Verlauf" })).toBeNull());
});
