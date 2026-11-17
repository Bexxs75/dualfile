import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { HOME, DOCS, DOWNLOADS, setupApp } from "./test-utils";
import { activeTabOf } from "./app/state";
import { newSshProfile } from "./components/dialogs/ConnectionProfileForm";
import { CoreError } from "./ipc/api";
const bookmark = { name: "Work", path: DOCS, color: "#123456", order: 0 };
const sidebar = () => screen.getByRole("navigation", { name: "Orte" });
const item = (name: RegExp) => within(sidebar()).getByRole("treeitem", { name });

test("sidebar clicks use active/other pane and middle click opens a tab", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  expect(screen.queryByRole("toolbar", { name: "Lesezeichen" })).toBeNull();
  await app.user.click(item(/^Work/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  fireEvent.click(item(/^Work/), { ctrlKey: true });
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 1).path).toBe(DOCS));
  fireEvent(item(/^Downloads/), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
  await waitFor(() => expect(app.runtime.store.getState().windows[0].tabs).toHaveLength(2));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOWNLOADS));
});
test("mode shortcuts persist and restore the last visible mode, ribbon only while hidden", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  const resize = vi.spyOn(app.api, "sidebarResizeWindow");
  await app.user.keyboard("{Shift>}{F9}{/Shift}");
  expect(app.runtime.store.getState().settings.sidebarMode).toBe("icons");
  await app.user.keyboard("{F9}");
  expect(screen.queryByRole("navigation", { name: "Orte" })).toBeNull();
  expect(screen.getByRole("toolbar", { name: "Lesezeichen" })).toBeInTheDocument();
  await app.user.keyboard("{F9}");
  expect(app.runtime.store.getState().settings.sidebarMode).toBe("icons");
  await act(() => app.runtime.controller.saveNow());
  expect((await app.api.loadSettings()).settings.sidebarMode).toBe("icons");
  await waitFor(() => expect(resize).toHaveBeenCalledWith(-166));
});
test("sidebar keyboard tree navigation is isolated and explicit global commands still work", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  const before = activeTabOf(app.runtime.store.getState(), 0).selection;
  act(() => item(/^Home-Ordner/).focus());
  await app.user.keyboard("{ArrowDown}{End}{Home}w");
  expect(activeTabOf(app.runtime.store.getState(), 0).selection).toEqual(before);
  expect(item(/^Work/)).toHaveFocus();
  await app.user.keyboard("{Control>}{Enter}{/Control}");
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 1).path).toBe(DOCS));
  await app.user.keyboard("{Tab}");
  expect(app.runtime.store.getState().active).toBe(1);
  expect(within(app.pane(1)).getByRole("grid")).toHaveFocus();
  act(() => item(/^Work/).focus());
  await app.user.keyboard("{Control>}f{/Control}");
  expect(within(app.pane(1)).getByRole("searchbox")).toHaveFocus();
  act(() => item(/^Work/).focus());
  await app.user.keyboard("{Control>}l{/Control}");
  expect(app.runtime.store.getState().pathEditing).toBe(1);
});
test("collapse, hidden entries, unmounted explanation and rail focus tooltip", async () => {
  const app = await setupApp();
  expect(item(/^NAS \(fstab\)/)).toHaveAttribute("aria-disabled", "true");
  await app.user.click(item(/^NAS \(fstab\)/));
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME);
  fireEvent.contextMenu(item(/^USB-Stick/), { clientX: 40, clientY: 80 });
  await app.user.click(screen.getByRole("menuitem", { name: "Eintrag ausblenden" }));
  expect(app.runtime.store.getState().settings.sidebarHidden).toContain("drive:/run/media/demo/USB-Stick");
  expect(within(sidebar()).queryByRole("treeitem", { name: /^USB-Stick/ })).toBeNull();
  fireEvent.contextMenu(item(/^Geräte$/), { clientX: 40, clientY: 80 });
  await app.user.click(screen.getByRole("menuitemcheckbox", { name: "Ausgeblendete Einträge anzeigen" }));
  expect(item(/^USB-Stick/)).toHaveClass("place-hidden");
  await app.user.click(item(/^Geräte$/));
  expect(app.runtime.store.getState().settings.sidebarCollapsed).toContain("devices");
  act(() => app.runtime.controller.toggleSidebarIcons());
  await app.user.click(item(/^Geräte$/));
  const usb = item(/^USB-Stick/);
  act(() => usb.focus());
  expect(usb.getAttribute("aria-label")).toContain("eingehängt");
  expect(await screen.findByRole("tooltip")).toHaveTextContent("USB-Stick");
});
test("live plug is announced and unplug sends all affected tabs home", async () => {
  const app = await setupApp();
  act(() => app.base.devPlug({ name: "TESTSTICK", path: DOCS, kind: "removable", mounted: true }));
  await screen.findByRole("treeitem", { name: /^TESTSTICK/ });
  expect(sidebar().querySelector('[aria-live="polite"]')).toHaveTextContent("TESTSTICK eingehängt");
  await app.user.click(item(/^TESTSTICK/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  act(() => app.base.devUnplug(DOCS));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME));
  expect(sidebar().querySelector('[aria-live="polite"]')).toHaveTextContent("TESTSTICK entfernt");
});
test("disconnected profile opens connect dialog; trash uses system trash", async () => {
  const app = await setupApp();
  const trash = vi.spyOn(app.api, "trashOpenSystem");
  await app.user.click(item(/^Papierkorb/)); expect(trash).toHaveBeenCalledOnce();
  trash.mockRejectedValueOnce(new Error("helper failed"));
  await app.user.click(item(/^Papierkorb/));
  expect(screen.getByText("Der System-Papierkorb konnte nicht geöffnet werden.")).toBeInTheDocument();
  await act(async () => { await app.api.connectionSave(newSshProfile("offline")); await app.runtime.controller.refreshConnections(); });
  vi.spyOn(app.api, "connectionConnect").mockRejectedValue(new CoreError("password", "needsPassword"));
  await app.user.click(item(/^offline/));
  expect(app.runtime.store.getState().dialog).toMatchObject({ kind: "connections", entry: { action: "connect" } });
});
test("automatic rail is temporary when compositor refuses growth", async () => {
  const old = window.innerWidth; Object.defineProperty(window, "innerWidth", { configurable: true, value: 900 });
  try {
    const app = await setupApp({ wrap: api => { api.sidebarResizeWindow = vi.fn(async () => ({ actualDelta: 0, canGrow: false })); return api; } });
    await waitFor(() => expect(sidebar()).toHaveClass("sidebar-icons"));
    expect(app.runtime.store.getState().settings.sidebarMode).toBeUndefined();
    await act(() => app.runtime.controller.saveNow());
    expect((await app.api.loadSettings()).settings.sidebarMode).toBeUndefined();
  } finally { Object.defineProperty(window, "innerWidth", { configurable: true, value: old }); }
});

test("handle drag saves on release and resizes the window only once", async () => {
  const app = await setupApp({ settings: { sidebarMode: "full", sidebarWidth: 210 } });
  await waitFor(() => expect(sidebar()).toHaveClass("sidebar-full"));
  const resize = vi.spyOn(app.api, "sidebarResizeWindow");
  const handle = screen.getByRole("separator", { name: "Breite der Seitenleiste" });
  handle.setPointerCapture = vi.fn();
  fireEvent(handle, new MouseEvent("pointerdown", { bubbles: true, clientX: 210 }));
  fireEvent(handle, new MouseEvent("pointermove", { bubbles: true, clientX: 270 }));
  expect(app.runtime.store.getState().settings.sidebarWidth).toBe(210);
  expect(resize).not.toHaveBeenCalled();
  fireEvent(handle, new MouseEvent("pointerup", { bubbles: true, clientX: 270 }));
  await waitFor(() => expect(resize).toHaveBeenCalledWith(60));
  expect(app.runtime.store.getState().settings.sidebarWidth).toBe(270);
  fireEvent.doubleClick(handle);
  await waitFor(() => expect(sidebar()).toHaveClass("sidebar-icons"));
});
test("partial window growth undoes only excess pixels when falling back to rail", async () => {
  const old = window.innerWidth; Object.defineProperty(window, "innerWidth", { configurable: true, value: 1000 });
  const resize = vi.fn().mockResolvedValueOnce({ actualDelta: 100, canGrow: false }).mockResolvedValue({ actualDelta: -56, canGrow: false });
  try {
    await setupApp({ wrap: api => { api.sidebarResizeWindow = resize; return api; } });
    await waitFor(() => expect(sidebar()).toHaveClass("sidebar-icons"));
    expect(resize.mock.calls.slice(0, 2)).toEqual([[210], [-56]]);
  } finally { Object.defineProperty(window, "innerWidth", { configurable: true, value: old }); }
});
test("bookmark Delete is isolated and offers undo; Alt+number works from sidebar", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  act(() => item(/^Work/).focus());
  await app.user.keyboard("{Delete}");
  expect(app.runtime.store.getState().settings.bookmarks).toEqual([]);
  expect(app.runtime.store.getState().dialog).toBeNull();
  await app.user.click(within(sidebar()).getByRole("button", { name: "Rückgängig" }));
  expect(app.runtime.store.getState().settings.bookmarks).toEqual([bookmark]);
  act(() => item(/^Home-Ordner/).focus());
  await app.user.keyboard("{Alt>}1{/Alt}");
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
});
test("snapshot calls coalesce and both pane menus use the live shared store", async () => {
  const app = await setupApp();
  const snapshot = await app.api.placesSnapshot();
  let release!: (value: typeof snapshot) => void;
  const snapshots = vi.spyOn(app.api, "placesSnapshot").mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  const local = vi.spyOn(app.api, "localDrives");
  let pending!: Promise<void>;
  act(() => { pending = app.runtime.controller.refreshPlaces(); void app.runtime.controller.refreshPlaces(); void app.runtime.controller.refreshPlaces(); });
  expect(snapshots).toHaveBeenCalledOnce();
  await act(async () => { release(snapshot); await pending; });
  expect(snapshots).toHaveBeenCalledTimes(2);
  act(() => app.base.devPlug());
  await screen.findByRole("treeitem", { name: /^TESTSTICK/ });
  await app.user.click(within(app.pane(0)).getByRole("button", { name: /Ort wählen/ }));
  expect(screen.getByRole("menuitem", { name: /TESTSTICK/ })).toBeInTheDocument();
  expect(local).not.toHaveBeenCalled();
});

test("maximize/hide/unmaximize/show cycles only shrink confirmed growth", async () => {
  let maximized = false, actualWidth = 1400;
  const resize = vi.fn(async (delta: number) => {
    const actualDelta = maximized ? 0 : delta;
    actualWidth += actualDelta;
    return { actualDelta, canGrow: !maximized };
  });
  const old = window.innerWidth; Object.defineProperty(window, "innerWidth", { configurable: true, value: 1600 });
  try {
    const app = await setupApp({ wrap: api => { api.sidebarResizeWindow = resize; return api; } });
    await waitFor(() => expect(actualWidth).toBe(1610));
    for (let i = 0; i < 3; i++) {
      maximized = true;
      await app.user.keyboard("{F9}");
      await waitFor(() => expect(screen.queryByRole("navigation", { name: "Orte" })).toBeNull());
      maximized = false;
      await app.user.keyboard("{F9}");
      await waitFor(() => expect(sidebar()).toHaveClass("sidebar-full"));
      expect(actualWidth).toBe(1610);
    }
    await app.user.keyboard("{F9}");
    await waitFor(() => expect(actualWidth).toBe(1400));
    expect(resize.mock.calls.map(([delta]) => delta)).toEqual([210, -210, -210, -210, -210]);
  } finally { Object.defineProperty(window, "innerWidth", { configurable: true, value: old }); }
});

test("tiled refusal earns no shrink credit and restored geometry does", async () => {
  let tiled = true;
  const resize = vi.fn(async (delta: number) => ({ actualDelta: tiled ? 0 : delta, canGrow: !tiled }));
  const app = await setupApp({ wrap: api => { api.sidebarResizeWindow = resize; return api; } });
  await waitFor(() => expect(resize).toHaveBeenCalledWith(210));
  await app.user.keyboard("{F9}");
  tiled = false;
  await app.user.keyboard("{F9}");
  await waitFor(() => expect(resize).toHaveBeenCalledTimes(2));
  expect(resize.mock.calls).toEqual([[210], [210]]);
  await app.user.keyboard("{F9}");
  await waitFor(() => expect(resize).toHaveBeenLastCalledWith(-210));
});

test("restored sidebar width is credited without another grow", async () => {
  const resize = vi.fn(async (delta: number) => ({ actualDelta: delta, canGrow: true, restoredWidth: 0 }))
    .mockResolvedValueOnce({ actualDelta: 0, canGrow: true, restoredWidth: 210 });
  const app = await setupApp({ wrap: api => { api.sidebarResizeWindow = resize; return api; } });
  await app.user.keyboard("{F9}");
  await waitFor(() => expect(resize.mock.calls).toEqual([[210], [-210]]));
});

test("user window resizes only recheck rail fallback without IPC resize", async () => {
  const resize = vi.fn(async (delta: number) => ({ actualDelta: delta, canGrow: true }));
  const old = window.innerWidth;
  try {
    await setupApp({ wrap: api => { api.sidebarResizeWindow = resize; return api; } });
    await waitFor(() => expect(resize).toHaveBeenCalledOnce());
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 950 });
    fireEvent(window, new Event("resize"));
    await waitFor(() => expect(sidebar()).toHaveClass("sidebar-icons"));
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1600 });
    fireEvent(window, new Event("resize"));
    await waitFor(() => expect(sidebar()).toHaveClass("sidebar-full"));
    expect(resize).toHaveBeenCalledOnce();
  } finally { Object.defineProperty(window, "innerWidth", { configurable: true, value: old }); }
});

test("keyboard repeats and settings width edits coalesce compositor requests", async () => {
  const app = await setupApp();
  const resize = vi.spyOn(app.api, "sidebarResizeWindow");
  const handle = screen.getByRole("separator", { name: "Breite der Seitenleiste" });
  for (let i = 0; i < 5; i++) fireEvent.keyDown(handle, { key: "ArrowRight" });
  expect(app.runtime.store.getState().settings.sidebarWidth).toBe(260);
  expect(resize).not.toHaveBeenCalled();
  await waitFor(() => expect(resize.mock.calls).toEqual([[50]]));
  resize.mockClear();
  for (const width of [280, 300, 320]) act(() => app.runtime.controller.setSidebarWidth(width));
  expect(resize).not.toHaveBeenCalled();
  await waitFor(() => expect(resize.mock.calls).toEqual([[60]]));
});

test("sidebar mode changes publish visibility and disposal stops polling", async () => {
  const visible = vi.fn(async (_value: boolean) => {});
  const app = await setupApp({ wrap: api => { api.placesSetVisible = visible; return api; } });
  expect(visible.mock.calls).toEqual([[true]]);
  await app.user.keyboard("{F9}");
  expect(visible.mock.calls).toEqual([[true], [false]]);
  await app.user.keyboard("{Shift>}{F9}{/Shift}");
  expect(visible.mock.calls).toEqual([[true], [false], [true]]);
  await app.user.keyboard("{Shift>}{F9}{/Shift}");
  expect(visible).toHaveBeenCalledTimes(3);
  act(() => app.runtime.controller.dispose());
  expect(visible).toHaveBeenLastCalledWith(false);
});

test("idle-expired automount preserves tabs/history and removal toast requires moved tabs", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  const drive = { name: "Auto", path: DOCS, kind: "network" as const, mounted: true, fstab: { automount: true, userMountable: false, noauto: false } };
  act(() => app.base.devPlug(drive));
  await screen.findByRole("treeitem", { name: /^Auto/ });
  await app.user.click(item(/^Work/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  const before = activeTabOf(app.runtime.store.getState(), 0);
  const snapshot = await app.api.placesSnapshot();
  snapshot.drives = snapshot.drives.map(d => d.path === DOCS ? { ...d, mounted: false } : d);
  vi.spyOn(app.api, "placesSnapshot").mockResolvedValue(snapshot);
  await act(() => app.runtime.controller.refreshPlaces());
  expect(activeTabOf(app.runtime.store.getState(), 0)).toEqual(before);
  expect(app.runtime.store.getState().toasts.some(t => t.text.includes("Betroffene Reiter"))).toBe(false);
  // Unplug a different drive that has no open tabs.
  snapshot.drives = snapshot.drives.filter(d => d.path !== "/run/media/demo/USB-Stick");
  await act(() => app.runtime.controller.refreshPlaces());
  expect(app.runtime.store.getState().toasts.some(t => t.text.includes("Betroffene Reiter"))).toBe(false);
});
