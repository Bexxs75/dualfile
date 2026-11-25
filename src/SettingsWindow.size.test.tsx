import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { SettingsWindow } from "./SettingsWindow";
import { createMockApi } from "./ipc/mock";
import type { SettingsWindowDelivery, SettingsWindowSettings, SettingsWindowSnapshot } from "./ipc/types";
import { testSnapshot } from "./app/settingsWindow.fixture";
afterEach(() => { cleanup(); document.documentElement.className = ""; });

type Size = { width: number; height: number };
const STANDARD: Size = { width: 720, height: 640 };
const resize = ({ width, height }: Size) => { Object.defineProperty(window, "innerWidth", { configurable: true, value: width }); Object.defineProperty(window, "innerHeight", { configurable: true, value: height }); };
/**
 * `now` is the size of the window when it is closed; it opened at `opened`. The size is the one of the
 * web view (`innerWidth`/`innerHeight`), exactly what the client measures.
 */
function setup(now: Size | null, settings: Partial<SettingsWindowSettings> = {}, { opened = STANDARD, zoomed = false }: { opened?: Size; zoomed?: boolean } = {}) {
  resize(opened);
  const api = createMockApi();
  let receive!: (delivery: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { receive = fn; };
  api.settingsWindowZoomed = vi.fn(async () => zoomed);
  const send = vi.spyOn(api, "settingsWindowSend");
  const close = vi.spyOn(api, "settingsWindowClose");
  const hold = vi.spyOn(api, "settingsWindowHold");
  render(<SettingsWindow api={api} search="?tab=darstellung&lang=de" />);
  act(() => receive({ kind: "session", session: "s1" }));
  const snapshot = (value: Partial<SettingsWindowSnapshot> = {}) => act(() => receive({ kind: "snapshot", snapshot: testSnapshot({ ...value, settings: { ...testSnapshot().settings, language: "de", ...settings, ...value.settings } }) }));
  snapshot();
  if (now) resize(now);
  return { api, send, close, hold, snapshot, receive, user: userEvent.setup() };
}
const patches = (b: ReturnType<typeof setup>) => b.send.mock.calls.map(([m]) => m).filter(m => m.kind === "patch").map(m => m.kind === "patch" ? m.patch : null);
const dialog = () => screen.getByRole("alertdialog", { name: "Fenstergröße beibehalten?" });
/** Presses "Fertig" and lets the main window acknowledge everything the window sent so far. */
async function closeAndAck(b: ReturnType<typeof setup>) {
  await waitFor(() => expect(b.send.mock.calls.some(([m]) => m.kind === "flush")).toBe(true));
  b.snapshot({ ack: b.send.mock.calls.length });
  await waitFor(() => expect(b.close).toHaveBeenCalled());
}

test("ask: the question shows the new size, the window keeps working until the answer, Yes stores it and the window closes", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  const dlg = await screen.findByRole("alertdialog");
  expect(within(dlg).getByText("Fenstergröße beibehalten?")).toBeVisible();
  expect(dlg).toHaveTextContent("Du hast das Einstellungsfenster auf 900 × 700 vergrößert. Soll Dualfile diese Größe beim nächsten Öffnen wieder verwenden?");
  expect(within(dlg).getByRole("checkbox", { name: "Nicht mehr fragen" })).not.toBeChecked();
  expect(dlg).toHaveTextContent("Das lässt sich unter Einstellungen › Allgemein wieder ändern.");
  expect(within(dlg).getByRole("button", { name: "Ja, beibehalten" })).toHaveFocus();
  // Nothing is sent and the native close timeout is held while the question is open.
  expect(b.send).not.toHaveBeenCalled();
  expect(b.hold).toHaveBeenCalledWith("s1", true);
  await b.user.click(within(dlg).getByRole("button", { name: "Ja, beibehalten" }));
  await waitFor(() => expect(patches(b)).toEqual([{ settingsWindowSize: { width: 900, height: 700 } }]));
  expect(b.hold).toHaveBeenLastCalledWith("s1", false);
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await closeAndAck(b);
  expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["patch", "flush"]);
});
test("a smaller window is called smaller", async () => {
  const b = setup({ width: 600, height: 500 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  expect(await screen.findByRole("alertdialog")).toHaveTextContent("auf 600 × 500 verkleinert");
});
test("No forgets the size; with the tick the mode becomes never", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await b.user.click(within(await screen.findByRole("alertdialog")).getByRole("checkbox", { name: "Nicht mehr fragen" }));
  await b.user.click(screen.getByRole("button", { name: "Nein, Standardgröße" }));
  await waitFor(() => expect(patches(b)).toEqual([{ settingsWindowSize: { width: 0, height: 0 }, settingsWindowSizeMode: "never" }]));
});
test("Yes with the tick makes the mode always", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await b.user.click(within(await screen.findByRole("alertdialog")).getByRole("checkbox", { name: "Nicht mehr fragen" }));
  await b.user.click(screen.getByRole("button", { name: "Ja, beibehalten" }));
  await waitFor(() => expect(patches(b)).toEqual([{ settingsWindowSize: { width: 900, height: 700 }, settingsWindowSizeMode: "always" }]));
});
test("No without the tick forgets the size and keeps asking", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await b.user.click(await screen.findByRole("button", { name: "Nein, Standardgröße" }));
  await waitFor(() => expect(patches(b)).toEqual([{ settingsWindowSize: { width: 0, height: 0 } }]));
});
test("Enter is Yes, even from the tick box", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  const box = within(await screen.findByRole("alertdialog")).getByRole("checkbox", { name: "Nicht mehr fragen" });
  box.focus();
  await b.user.keyboard("{Enter}");
  await waitFor(() => expect(patches(b)).toEqual([{ settingsWindowSize: { width: 900, height: 700 } }]));
});
test("Esc closes without any change and never sets the tick, even when it is ticked", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await b.user.click(within(await screen.findByRole("alertdialog")).getByRole("checkbox", { name: "Nicht mehr fragen" }));
  fireEvent.keyDown(dialog(), { key: "Escape" });
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(patches(b)).toEqual([]);
  await closeAndAck(b);
});
test("Escape in the window starts the question, a second Escape answers it as dismissal", async () => {
  const b = setup({ width: 900, height: 700 });
  fireEvent.keyDown(document, { key: "Escape" });
  await screen.findByRole("alertdialog");
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
test.each([[728, 648], [712, 632], [720, 640]])("no question for a change of at most 8 px (%i x %i)", async (width, height) => {
  const b = setup({ width, height });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
test("a change of 9 px asks", async () => {
  const b = setup({ width: 729, height: 640 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  expect(await screen.findByRole("alertdialog")).toBeVisible();
});
test("always saves silently when closing", async () => {
  const b = setup({ width: 900, height: 700 }, { settingsWindowSizeMode: "always" });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["patch", "flush"]));
  expect(patches(b)).toEqual([{ settingsWindowSize: { width: 900, height: 700 } }]);
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(b.hold).not.toHaveBeenCalled();
});
test("never neither asks nor stores nor even looks at the size", async () => {
  const b = setup({ width: 900, height: 700 }, { settingsWindowSizeMode: "never" });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(b.api.settingsWindowZoomed).not.toHaveBeenCalled();
});
test("the reference is the opened size: a remembered size that was not touched is not asked about", async () => {
  const b = setup(null, { settingsWindowSize: { width: 900, height: 700 } }, { opened: { width: 900, height: 700 } });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
});
test("a native close on behalf of the main window never asks; a native close from the title bar does", async () => {
  const quiet = setup({ width: 900, height: 700 });
  act(() => quiet.receive({ kind: "close", session: "s1", interactive: false }));
  await waitFor(() => expect(quiet.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  cleanup();
  const bar = setup({ width: 900, height: 700 });
  act(() => bar.receive({ kind: "close", session: "s1", interactive: true }));
  expect(await screen.findByRole("alertdialog")).toBeVisible();
  expect(bar.hold).toHaveBeenCalledWith("s1", true);
});
test("a close that failed and is retried does not ask the same question twice", async () => {
  const b = setup({ width: 900, height: 700 });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await b.user.click(await screen.findByRole("button", { name: "Ja, beibehalten" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["patch", "flush"]));
  b.snapshot({ ack: 2, errors: [{ seq: 2, message: "disk full" }] });
  await waitFor(() => expect(screen.getByRole("button", { name: "Fertig" })).toBeEnabled());
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.filter(([m]) => m.kind === "flush")).toHaveLength(2));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
test("a maximized, fullscreen or minimized window is neither asked about nor remembered, not even with always", async () => {
  for (const mode of ["ask", "always"] as const) {
    const b = setup({ width: 1900, height: 1000 }, { settingsWindowSizeMode: mode }, { zoomed: true });
    await b.user.click(screen.getByRole("button", { name: "Fertig" }));
    await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    cleanup();
  }
});
test("a failing zoom lookup just closes", async () => {
  const b = setup({ width: 900, height: 700 });
  b.api.settingsWindowZoomed = vi.fn(async () => { throw new Error("no window"); });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
test("the reference is the size the window actually opened with, even if the system gave it another one (tiling, GNOME decoration)", async () => {
  // Asked for 720 x 640, got 772 x 729: closing at that size is no change.
  const b = setup(null, {}, { opened: { width: 772, height: 729 } });
  await b.user.click(screen.getByRole("button", { name: "Fertig" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["flush"]));
  expect(screen.queryByRole("alertdialog")).toBeNull();
});
test("layout changes while the window settles move the reference; later ones are the user's", async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  try {
    const b = setup(null);
    resize({ width: 800, height: 700 });
    fireEvent(window, new Event("resize"));
    vi.setSystemTime(Date.now() + 5000);
    resize({ width: 1000, height: 800 });
    fireEvent(window, new Event("resize"));
    await b.user.click(screen.getByRole("button", { name: "Fertig" }));
    // The user's change is measured against the settled 800 x 700.
    expect(await screen.findByRole("alertdialog")).toHaveTextContent("auf 1000 × 800 vergrößert");
  } finally { vi.useRealTimers(); }
});
