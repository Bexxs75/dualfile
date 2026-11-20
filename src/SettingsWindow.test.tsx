import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { SettingsWindow } from "./SettingsWindow";
import { createMockApi } from "./ipc/mock";
import type { SettingsWindowDelivery, SettingsWindowSnapshot } from "./ipc/types";
import { testSnapshot } from "./app/settingsWindow.fixture";
afterEach(() => { cleanup(); document.documentElement.className = ""; });
function setup(tab = "appearance") {
  const api = createMockApi();
  let receive!: (delivery: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { receive = fn; };
  const send = vi.spyOn(api, "settingsWindowSend");
  const close = vi.spyOn(api, "settingsWindowClose");
  const load = vi.spyOn(api, "loadSettings"), save = vi.spyOn(api, "saveSettings"), events = vi.spyOn(api, "onEvent");
  render(<SettingsWindow api={api} search={`?tab=${tab}&lang=en`} />);
  act(() => receive({ kind: "session", session: "s1" }));
  const snapshot = (value: Partial<SettingsWindowSnapshot> = {}) => act(() => receive({ kind: "snapshot", snapshot: testSnapshot(value) }));
  return { api, send, close, snapshot, receive, load, save, events, user: userEvent.setup() };
}
test("busy until first snapshot; live appearance and language without settings IO or events", async () => {
  const b = setup();
  expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "true");
  expect(screen.queryByRole("tab")).toBeNull();
  b.snapshot();
  expect(screen.getByRole("button", { name: "Done" })).toHaveFocus();
  expect(document.documentElement.lang).toBe("en");
  const theme = screen.getAllByRole("radio").find(el => el.textContent === "Paper")!;
  fireEvent.click(theme);
  expect(document.documentElement.dataset.fm).toBe("papier");
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ session: "s1", seq: 1, kind: "patch", patch: { theme: "papier" } }));
  b.snapshot(); expect(document.documentElement.dataset.fm).toBe("papier");
  b.snapshot({ ack: 1, settings: { ...testSnapshot().settings, theme: "papier", language: "de" } });
  expect(document.documentElement.lang).toBe("de");
  expect(screen.getByRole("button", { name: "Fertig" })).toBeVisible();
  expect([b.load, b.save, b.events].map(s => s.mock.calls.length)).toEqual([0, 0, 0]);
});
test("tray sends patch before action; start-menu error appears inline", async () => {
  const b = setup("system"); b.snapshot();
  await b.user.click(screen.getByRole("switch", { name: "Show icon in the notification area (tray)" }));
  await waitFor(() => expect(b.send.mock.calls.map(([m]) => m.kind)).toEqual(["refreshStatus", "patch", "setTray"]));
  const add = screen.getAllByRole("button").find(el => /Add/.test(el.textContent ?? ""))!;
  await b.user.click(add);
  await waitFor(() => expect(b.send).toHaveBeenCalledTimes(4));
  b.snapshot({ ack: 4, errors: [{ seq: 4, message: "permission denied" }] });
  expect(await screen.findAllByText("permission denied")).not.toHaveLength(0);
});
test("autostart select shows the entry on disk, sends the choice and reports a failure inline", async () => {
  const b = setup("system"); b.snapshot();
  const select = screen.getByRole("combobox", { name: "Start with the system" });
  expect(select).toHaveValue("off");
  expect([...select.querySelectorAll("option")].map(o => o.textContent)).toEqual(["Off", "Start minimized", "Start and show the window"]);
  await b.user.selectOptions(select, "minimized");
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ session: "s1", seq: 2, kind: "setAutostart", mode: "minimized" }));
  b.snapshot({ ack: 2, system: { ...testSnapshot().system, status: { appImage: true, startMenuExists: false, autostart: "minimized" } } });
  await waitFor(() => expect(select).toHaveValue("minimized"));
  await b.user.selectOptions(select, "shown");
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ session: "s1", seq: 3, kind: "setAutostart", mode: "shown" }));
  b.snapshot({ ack: 3, errors: [{ seq: 3, message: "Permission denied" }], system: { ...testSnapshot().system, status: { appImage: true, startMenuExists: false, autostart: "minimized" } } });
  expect(await screen.findByText("The autostart entry could not be changed: Permission denied")).toBeVisible();
  expect(select).toHaveValue("minimized");
});
test("opening the system tab and focusing the window re-read the entries from disk", async () => {
  const b = setup("system"); b.snapshot();
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ session: "s1", seq: 1, kind: "refreshStatus" }));
  const select = screen.getByRole("combobox", { name: "Start with the system" });
  b.snapshot({ ack: 1, system: { ...testSnapshot().system, status: { appImage: true, startMenuExists: false, autostart: "shown" } } });
  await waitFor(() => expect(select).toHaveValue("shown"));
  act(() => { window.dispatchEvent(new Event("focus")); });
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ session: "s1", seq: 2, kind: "refreshStatus" }));
  // Deleted outside Dualfile in the meantime: the control follows the disk.
  b.snapshot({ ack: 2, system: { ...testSnapshot().system, status: { appImage: true, startMenuExists: false, autostart: "off", startMenuError: "Symlink in desktop integration path" } } });
  await waitFor(() => expect(select).toHaveValue("off"));
  expect(select).toBeEnabled();
  expect(screen.getByText("Symlink in desktop integration path")).toBeVisible();
});
test("autostart entry that could not be read or refreshed is reported in the settings", () => {
  const b = setup("system");
  b.snapshot({ system: { ...testSnapshot().system, status: { appImage: true, startMenuExists: false, autostart: "off", autostartError: "Autostart entry is a symlink" } } });
  expect(screen.getByText("The autostart entry could not be changed: Autostart entry is a symlink")).toBeVisible();
});
test("Done waits for acknowledgement, and X uses the same protocol", async () => {
  const b = setup(); b.snapshot();
  await b.user.click(screen.getByRole("button", { name: "Done" }));
  await waitFor(() => expect(b.send).toHaveBeenCalledWith({ kind: "flush", seq: 1, session: "s1" }));
  expect(b.close).not.toHaveBeenCalled();
  expect(screen.getByText("Changes are still being applied …")).toBeVisible();
  b.snapshot({ ack: 1 });
  await waitFor(() => expect(b.close).toHaveBeenCalledWith("s1", 1));
});
test("Escape flushes; persistence notices are visible", async () => {
  const b = setup(); b.snapshot({ persistence: { outcome: { kind: "newerVersion", version: 99 }, error: "disk full" } });
  expect(screen.getByRole("alert")).toHaveTextContent("disk full");
  expect(screen.getByText(/99/)).toBeVisible();
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() => expect(b.send.mock.lastCall?.[0].kind).toBe("flush"));
  b.snapshot({ ack: 1 }); await waitFor(() => expect(b.close).toHaveBeenCalled());
});
test("Check now delegates; status follows snapshots", async () => {
  const b = setup("info"); b.snapshot();
  await b.user.click(screen.getByRole("button", { name: /Check now/i }));
  await waitFor(() => expect(b.send.mock.lastCall?.[0].kind).toBe("checkUpdates"));
  b.snapshot({ ack: 1, system: { ...testSnapshot().system, check: "checking" } });
  expect(screen.getByRole("button", { name: /Check now/i })).toBeDisabled();
});

test("Escape during key capture cancels only capture", async () => {
  const b = setup("keyboard"); b.snapshot();
  await b.user.click(screen.getAllByRole("button", { name: "Change" })[0]!);
  fireEvent.keyDown(document, { key: "Escape" });
  expect(b.send).not.toHaveBeenCalled(); expect(b.close).not.toHaveBeenCalled();
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() => expect(b.send.mock.lastCall?.[0].kind).toBe("flush"));
});
test("log save uses main's logDir; Escape closes only the log modal", async () => {
  const b = setup("info");
  const save = vi.spyOn(b.api, "logSave").mockResolvedValue("/main/folder/log.txt");
  b.snapshot({ logDir: "/main/folder" });
  await b.user.click(screen.getByRole("button", { name: /View log file/i }));
  expect(screen.getByRole("dialog")).toBeVisible();
  await b.user.click(screen.getByRole("button", { name: /Save/ }));
  expect(save).toHaveBeenCalledWith("/main/folder", true);
  fireEvent.keyDown(document, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(b.send).not.toHaveBeenCalled(); expect(b.close).not.toHaveBeenCalled();
});
test("native close requested before the first snapshot waits for readiness", async () => {
  const b = setup();
  act(() => b.receive({ kind: "close", session: "s1" }));
  expect(b.send).not.toHaveBeenCalled();
  b.snapshot(); await waitFor(() => expect(b.send.mock.lastCall?.[0].kind).toBe("flush"));
  b.snapshot({ ack: 1 }); await waitFor(() => expect(b.close).toHaveBeenCalledWith("s1", 1));
});

test("failed parent close exposes an explicit loss warning and force close", async () => {
  const b = setup(); b.snapshot();
  act(() => b.receive({ kind: "close", session: "s1" }));
  await waitFor(() => expect(b.send.mock.lastCall?.[0].kind).toBe("flush"));
  b.snapshot({ ack: 1, errors: [{ seq: 1, message: "disk full" }], persistence: { outcome: { kind: "loaded" }, error: "disk full" } });
  expect(await screen.findByRole("button", { name: "Close anyway" })).toBeVisible();
  expect(screen.getByText("Changes that were not saved will be lost")).toBeVisible();
  await waitFor(() => expect(b.close).toHaveBeenCalledWith("s1", 1, false, true));
  await b.user.click(screen.getByRole("button", { name: "Close anyway" }));
  expect(b.close).toHaveBeenLastCalledWith("s1", 1, true);
});
test.each([
  [{ kind: "repaired", messages: ["test repair"] }, /test repair/],
  [{ kind: "recoveredFromCorrupt" }, /corrupt|damaged/i],
  [{ kind: "migrated", from: 2, to: 5 }, /2.*5/],
] as const)("settings mirrors the %s persistence notice", (outcome, text) => {
  const b = setup();
  b.snapshot({ persistence: { outcome: { ...outcome, ...(outcome.kind === "repaired" ? { messages: [...outcome.messages] } : {}) } as SettingsWindowSnapshot["persistence"]["outcome"], error: "" } });
  expect(screen.getByText(text)).toBeVisible();
});

test("attach failure leaves the emergency close usable while the panel is busy", async () => {
  const api = createMockApi();
  vi.spyOn(api, "settingsWindowAttach").mockRejectedValue(new Error("attach failed"));
  const close = vi.spyOn(api, "settingsWindowClose");
  render(<SettingsWindow api={api} search="?lang=en" />);
  expect(await screen.findByRole("alert")).toHaveTextContent("attach failed");
  expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "true");
  fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
  await waitFor(() => expect(close).toHaveBeenCalledWith("", 0, true));
});

test("German close failure shows the translated loss warning and clears on recovery", async () => {
  const b = setup(); b.snapshot({ settings: { ...testSnapshot().settings, language: "de" } });
  act(() => b.receive({ kind: "error", session: "s1", message: "relay failed" }));
  expect(screen.getByRole("button", { name: "Trotzdem schließen" })).toBeVisible();
  expect(screen.getByText("Nicht gespeicherte Änderungen gehen verloren")).toBeVisible();
  b.snapshot({ settings: { ...testSnapshot().settings, language: "de" } });
  expect(screen.queryByText("relay failed")).toBeNull();
  expect(screen.queryByRole("button", { name: "Trotzdem schließen" })).toBeNull();
});

test("sidebar preferences appear and synchronize in the separate settings window", async () => {
  const b = setup("seitenleiste");
  b.snapshot({ settings: { ...testSnapshot().settings, sidebarMode: "full", sidebarWidth: 210, sidebarCollapsed: [], sidebarHidden: ["drive:/data"] } });
  await b.user.click(screen.getByRole("radio", { name: "Icons" }));
  await waitFor(() => expect(b.send).toHaveBeenCalledWith(expect.objectContaining({ kind: "patch", patch: { sidebarMode: "icons" } })));
  b.snapshot({ ack: 1, settings: { ...testSnapshot().settings, sidebarMode: "hidden", sidebarWidth: 260, sidebarCollapsed: ["devices"], sidebarHidden: [] } });
  expect(screen.getByRole("radio", { name: "Hidden" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("slider", { name: "Sidebar width" })).toHaveValue("260");
  expect(b.load).not.toHaveBeenCalled(); expect(b.save).not.toHaveBeenCalled();
});

test("LAN discovery is opt-in and synchronized through the settings window relay", async () => {
  const b = setup("seitenleiste");
  b.snapshot();
  const label = "Discover services on the local network";
  const checkbox = await screen.findByRole("switch", { name: label });
  expect(checkbox).not.toBeChecked();
  await b.user.click(checkbox);
  await waitFor(() => expect(b.send).toHaveBeenCalledWith(expect.objectContaining({ kind: "patch", patch: { sidebarDiscovery: true } })));
  b.snapshot({ ack: 1, settings: { ...testSnapshot().settings, sidebarDiscovery: true } });
  expect(checkbox).toBeChecked();
  b.snapshot({ ack: 1, settings: { ...testSnapshot().settings, sidebarDiscovery: false, language: "de" } });
  expect(screen.getByRole("switch", { name: "Dienste im lokalen Netzwerk suchen" })).not.toBeChecked();
  expect(b.load).not.toHaveBeenCalled(); expect(b.save).not.toHaveBeenCalled();
});
