import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { HOME, DOCS, DOWNLOADS, setupApp } from "./test-utils";
import { activeTabOf, visibleEntries } from "./app/state";
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
  expect(item(/^NAS \(fstab\)/)).not.toHaveAttribute("aria-disabled", "true");
  const mount = vi.spyOn(app.api, "fstabMount");
  await app.user.click(item(/^NAS \(fstab\)/));
  await waitFor(() => expect(mount).toHaveBeenCalledWith("/mnt/nas"));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe("/mnt/nas"));
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
test("disconnected profile opens connect dialog; trash opens in-app, system trash via context menu", async () => {
  const app = await setupApp();
  const trash = vi.spyOn(app.api, "trashOpenSystem");
  await app.user.click(item(/^Papierkorb/));
  expect(trash).not.toHaveBeenCalled();
  expect(await screen.findByRole("grid", { name: "Papierkorb" }).catch(() => screen.findByText("Der Papierkorb ist leer"))).toBeInTheDocument();
  fireEvent.contextMenu(item(/^Papierkorb/));
  await app.user.click(screen.getByRole("menuitem", { name: "Im Systemdateimanager öffnen" })); expect(trash).toHaveBeenCalledOnce();
  trash.mockRejectedValueOnce(new Error("helper failed"));
  fireEvent.contextMenu(item(/^Papierkorb/));
  await app.user.click(screen.getByRole("menuitem", { name: "Im Systemdateimanager öffnen" }));
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

function dragFocused(app: Awaited<ReturnType<typeof setupApp>>, name = "Dokumente") {
  const entries = app.runtime.store.getState().listings[0].entries;
  // The controller uses the visible/sorted listing, as the file list does.
  const index = visibleEntries(app.runtime.store.getState(), 0).findIndex(e => e.name === name);
  expect(entries.some(e => e.name === name)).toBe(true);
  act(() => app.runtime.controller.dragStart(0, index));
}

test("sidebar and rail drops prefill copy/move and use the destination instead of the other pane", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark], sidebarMode: "icons" } });
  dragFocused(app);
  fireEvent.dragOver(item(/^Work/));
  expect(item(/^Work/)).toHaveClass("drop-over");
  fireEvent(item(/^Work/), new MouseEvent("drop", { bubbles: true, shiftKey: true }));
  const dialog = app.runtime.store.getState().dialog;
  expect(dialog?.kind).toBe("transfer");
  if (dialog?.kind === "transfer") expect(dialog.plan).toMatchObject({ destination: DOCS, kind: "move", sources: [DOCS] });
  act(() => app.runtime.controller.closeDialog());
  // An archive in the opposite pane does not make a writable sidebar destination read-only.
  act(() => app.runtime.store.dispatch({ type: "window", w: 1, update: w => ({ ...w, tabs: w.tabs.map(t => ({ ...t, path: `archive:${HOME}/test.zip!/` })) }) }));
  dragFocused(app);
  fireEvent.drop(item(/^Work/));
  const copy = app.runtime.store.getState().dialog;
  expect(copy?.kind === "transfer" && copy.plan.kind).toBe("copy");
});

test("drop refusal covers disconnected, unmounted, archive and read-only bookmark destinations", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark, { ...bookmark, name: "Archive", path: `archive:${HOME}/test.zip!/`, order: 1 }] } });
  await act(async () => { await app.api.connectionSave({ ...newSshProfile(), name: "Offline", host: "host", user: "demo" }); await app.runtime.controller.refreshConnections(); });
  for (const [name, message] of [[/^Offline/, "Bitte zuerst verbinden."], [/^NAS \(fstab\)/, "Bitte zuerst einhängen."], [/^Archive/, "schreibgeschützt"]] as const) {
    dragFocused(app); fireEvent.drop(item(name));
    expect(app.runtime.store.getState().dialog).toBeNull();
    expect(app.runtime.store.getState().toasts.at(-1)?.text.toLowerCase()).toContain(message.toLowerCase());
  }
  act(() => app.base.devPlug({ path: DOCS, name: "Read only", kind: "partition", mounted: true, readOnly: true }));
  await screen.findByRole("treeitem", { name: /^Read only/ });
  dragFocused(app); fireEvent.drop(item(/^Work/));
  expect(app.runtime.store.getState().dialog).toBeNull();
  expect(app.runtime.store.getState().toasts.at(-1)?.text.toLowerCase()).toContain("schreibgeschützt");
});

test("trash drops capture the source pane, ask before trashing, and retain undo", async () => {
  const app = await setupApp();
  const trash = vi.spyOn(app.api, "trash");
  dragFocused(app);
  fireEvent.drop(item(/^Papierkorb/));
  expect(trash).not.toHaveBeenCalled();
  expect(screen.getByRole("alertdialog")).toHaveTextContent("In den Papierkorb legen?");
  await app.user.click(screen.getByRole("button", { name: "Abbrechen" }));
  expect(trash).not.toHaveBeenCalled();
  dragFocused(app); fireEvent.drop(item(/^Papierkorb/));
  await app.user.click(screen.getByRole("button", { name: "Ja, Papierkorb" }));
  expect(trash).toHaveBeenCalledWith([DOCS]);
  await act(async () => { await app.scheduler.runAll(); });
  expect((await app.api.undoList()).length).toBeGreaterThan(0);
});

test("single-folder drag-in and bookmark reorder work at gaps and in the rail", async () => {
  const app = await setupApp({ settings: { bookmarks: [{ ...bookmark, name: "Downloads", path: DOWNLOADS }], sidebarMode: "icons" } });
  dragFocused(app);
  fireEvent.drop(sidebar().querySelector('.sidebar-bookmark-gap')!);
  expect(app.runtime.store.getState().settings.bookmarks.map(b => b.path)).toEqual([DOCS, DOWNLOADS]);
  fireEvent.dragStart(item(/^Dokumente/));
  fireEvent.drop(sidebar().querySelector(`[data-place-key="bookmark:${DOWNLOADS}"]`)!);
  expect(app.runtime.store.getState().settings.bookmarks.map(b => b.path)).toEqual([DOWNLOADS, DOCS]);
  expect(app.runtime.store.getState().drag).toBeNull();
});

test("empty trash context action requires confirmation and properties do not stat on open", async () => {
  const app = await setupApp();
  const empty = vi.spyOn(app.api, "trashEmpty"), space = vi.spyOn(app.api, "placesSpace");
  fireEvent.contextMenu(item(/^Papierkorb/));
  await app.user.click(screen.getByRole("menuitem", { name: "Papierkorb leeren …" }));
  expect(screen.getByRole("alertdialog")).toHaveTextContent("endgültig");
  expect(empty).not.toHaveBeenCalled();
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  expect(empty).toHaveBeenCalledOnce();
  fireEvent.contextMenu(item(/^NAS \(fstab\)/));
  await app.user.click(screen.getByRole("menuitem", { name: "Ort-Einstellungen" }));
  expect(screen.getByRole("dialog", { name: "Ort-Einstellungen" })).toHaveTextContent("nfs");
  expect(space).not.toHaveBeenCalled();
});

test("empty trash tells about trash folders skipped for their permissions (#47)", async () => {
  const app = await setupApp();
  vi.spyOn(app.api, "trashEmpty").mockResolvedValueOnce({ id: 4711, skipped: [{ path: "/home/demo/.local/share/Trash", mode: 0o775 }, { path: "/mnt/x/.Trash-1000", mode: 0o775 }] });
  fireEvent.contextMenu(item(/^Papierkorb/));
  await app.user.click(screen.getByRole("menuitem", { name: "Papierkorb leeren …" }));
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  expect(await screen.findByText("2 Papierkorb-Ordner übersprungen, weil sie anderen Schreibzugriff erlauben (Rechte 775).")).toBeInTheDocument();
});

test("unmount confirms inactive locked tabs, retains them on failure, and resets them only on success", async () => {
  let reject = true;
  const unmount = vi.fn(async () => { if (reject) throw new CoreError("busy", "busy"); });
  const app = await setupApp({ wrap: api => ({ ...api, fstabUnmount: unmount }) });
  act(() => app.base.devPlug({ name: "Mounted", path: DOCS, mounted: true, fstab: { userMountable: true, automount: false, noauto: true } }));
  await screen.findByRole("treeitem", { name: /^Mounted/ });
  act(() => app.runtime.store.dispatch({ type: "window", w: 1, update: w => ({ ...w, tabs: [...w.tabs, { ...w.tabs[0]!, path: DOCS, lockRoot: DOCS }] }) }));
  fireEvent.contextMenu(item(/^Mounted/));
  await app.user.click(screen.getByRole("menuitem", { name: "Aushängen" }));
  expect(unmount).not.toHaveBeenCalled();
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  await waitFor(() => expect(unmount).toHaveBeenCalledOnce());
  expect(app.runtime.store.getState().windows[1].tabs[1]?.path).toBe(DOCS);
  reject = false;
  fireEvent.contextMenu(item(/^Mounted/));
  await app.user.click(screen.getByRole("menuitem", { name: "Aushängen" }));
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  await waitFor(() => expect(app.runtime.store.getState().windows[1].tabs[1]?.path).toBe(HOME));
  expect(app.runtime.store.getState().windows[1].tabs[1]?.lockRoot).toBeUndefined();
});

test("profile-less GVfs menu saves a connection and validates unmount through the opaque key", async () => {
  const app = await setupApp();
  const save = vi.spyOn(app.api, "placesSaveConnection"), unmount = vi.spyOn(app.api, "placesMountUnmount");
  fireEvent.contextMenu(item(/^Shared/));
  await app.user.click(screen.getByRole("menuitem", { name: "Als Verbindung speichern" }));
  expect(save).toHaveBeenCalledWith("mount:mock-shared");
  await waitFor(() => expect(app.runtime.store.getState().connections.length).toBe(1));
  fireEvent.contextMenu(sidebar().querySelector('[data-place-key^="mount:"]')!);
  await app.user.click(screen.getByRole("menuitem", { name: "Aushängen" }));
  await waitFor(() => expect(unmount).toHaveBeenCalledWith("mock-shared"));
});

test("opaque GVfs unmount checks inactive and archive tabs without publishing its local path", async () => {
  const secretRoot = "/run/user/1000/gvfs/smb-share:server=nas,user=private";
  const archive = `archive:${secretRoot}/data.zip!/nested`;
  const affected = vi.fn(async (_key: string, paths: string[]) => paths.flatMap((p, i) => p === archive ? [i] : []));
  const unmount = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api,
    placesSnapshot: async () => ({ ...await api.placesSnapshot(), networkMounts: [{ name: "nas/shared", host: "nas", path: "/shared", url: "smb://nas/shared", sources: ["mounted"], scheme: "smb", port: null, user: null, mountedAt: null, mountKey: "opaque-redacted", canUnmount: true }] }),
    placesMountAffected: affected, placesMountUnmount: unmount,
  }) });
  act(() => app.runtime.store.dispatch({ type: "window", w: 0, update: w => ({ ...w, tabs: [...w.tabs, { ...w.tabs[0]!, path: archive }] }) }));
  const mount = item(/^nas\/shared/);
  expect(mount.getAttribute("aria-label")).not.toContain("private");
  fireEvent.contextMenu(mount);
  await app.user.click(screen.getByRole("menuitem", { name: "Aushängen" }));
  expect(affected).toHaveBeenCalledWith("opaque-redacted", expect.arrayContaining([archive]));
  expect(unmount).not.toHaveBeenCalled();
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  await waitFor(() => expect(app.runtime.store.getState().windows[0].tabs[1]?.path).toBe(HOME));
  expect(unmount).toHaveBeenCalledWith("opaque-redacted");
});

test("context navigation opens other pane and creates an unlocked new tab", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  fireEvent.contextMenu(item(/^Work/));
  await app.user.click(screen.getByRole("menuitem", { name: "Im anderen Fenster öffnen" }));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 1).path).toBe(DOCS));
  act(() => app.runtime.store.dispatch({ type: "window", w: app.runtime.store.getState().active, update: w => ({ ...w, tabs: w.tabs.map(t => ({ ...t, lockRoot: t.path })) }) }));
  const active = app.runtime.store.getState().active;
  const before = app.runtime.store.getState().windows[active].tabs.length;
  fireEvent.contextMenu(item(/^Home-Ordner/));
  await app.user.click(screen.getByRole("menuitem", { name: "In neuem Tab öffnen" }));
  await waitFor(() => expect(app.runtime.store.getState().windows[active].tabs).toHaveLength(before + 1));
  expect(activeTabOf(app.runtime.store.getState(), active).path).toBe(HOME);
  expect(activeTabOf(app.runtime.store.getState(), active).lockRoot).toBeUndefined();
});

test("saved system connections offer Unmount and never an unrelated Disconnect", async () => {
  const app = await setupApp({ left: DOCS });
  await act(async () => {
    await app.api.connectionSave({ ...newSshProfile(), name: "Desktop share", protocol: "system", transport: "builtin", systemUrl: "smb://nas/team", host: "nas", port: 0, share: "", startFolder: "/team" });
    await app.runtime.controller.refreshConnections();
  });
  const unmount = vi.spyOn(app.api, "systemMountUnmount");
  fireEvent.contextMenu(item(/^Desktop share/));
  expect(screen.getByRole("menuitem", { name: "Aushängen" })).toBeVisible();
  expect(screen.queryByRole("menuitem", { name: "Trennen" })).toBeNull();
  await act(async () => { await app.runtime.controller.disconnectConnection(app.runtime.store.getState().connections[0]!.profile.id); });
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS);
  expect(unmount).not.toHaveBeenCalled();
});

test("Ribbon and sidebar never overlap while the compositor delays hiding", async () => {
  const app = await setupApp({ settings: { bookmarks: [bookmark] } });
  await waitFor(() => expect(sidebar()).toBeVisible());
  let finish!: (value: { actualDelta: number; canGrow: boolean }) => void;
  vi.spyOn(app.api, "sidebarResizeWindow").mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  act(() => app.runtime.controller.toggleSidebar());
  expect(screen.queryByRole("navigation", { name: "Orte" })).toBeNull();
  expect(screen.getByRole("toolbar", { name: "Lesezeichen" })).toBeVisible();
  await waitFor(() => expect(finish).toBeDefined());
  await act(async () => { finish({ actualDelta: -210, canGrow: true }); });
});

test.each([false, true])("mounting blocks navigation and writing shortcuts until completion (failure=%s)", async failure => {
  let finish!: () => void;
  const mount = vi.fn(() => new Promise<void>((resolve, reject) => { finish = () => failure ? reject(new CoreError("busy", "busy")) : resolve(); }));
  const app = await setupApp({ wrap: api => ({ ...api, fstabMount: mount }) });
  const before = activeTabOf(app.runtime.store.getState(), 0).path;
  fireEvent.click(item(/^NAS \(fstab\)/));
  expect(screen.getByRole("alertdialog")).toHaveTextContent("wird eingehängt");
  expect(screen.getByRole("button", { name: "Abbrechen" })).toBeDisabled();
  await app.user.keyboard("{Escape}{F7}");
  act(() => app.runtime.controller.closeDialog());
  await act(async () => { await app.runtime.controller.openPlace("xdg:documents"); });
  expect(app.runtime.store.getState().dialog).toMatchObject({ kind: "placeConfirm", action: "mount", busy: true });
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(before);
  await act(async () => { finish(); });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  if (failure) expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(before);
});

test("fstab unmount checks symlink tabs in the backend and moves only affected tabs", async () => {
  const alias = `${HOME}/alias`;
  const affected = vi.fn(async (_key: string, paths: string[]) => paths.flatMap((p, i) => p === alias ? [i] : []));
  const unmount = vi.fn(async () => {});
  const app = await setupApp({ wrap: api => ({ ...api, placesMountAffected: affected, fstabUnmount: unmount }) });
  act(() => app.base.devPlug({ name: "Alias mount", path: "/media/disk", mounted: true, fstab: { userMountable: true, automount: false, noauto: true } }));
  await screen.findByRole("treeitem", { name: /^Alias mount/ });
  act(() => app.runtime.store.dispatch({ type: "window", w: 1, update: w => ({ ...w, tabs: [...w.tabs, { ...w.tabs[0]!, path: alias, lockRoot: alias }] }) }));
  fireEvent.contextMenu(item(/^Alias mount/));
  await app.user.click(screen.getByRole("menuitem", { name: "Aushängen" }));
  expect(affected).toHaveBeenCalledWith("path:/media/disk", expect.arrayContaining([alias]));
  expect(unmount).not.toHaveBeenCalled();
  await app.user.click(screen.getByRole("button", { name: "Bestätigen" }));
  await waitFor(() => expect(app.runtime.store.getState().windows[1].tabs[1]?.path).toBe(HOME));
});

test("remote transfer destinations are never classified as unmounted local drives", async () => {
  const remote = "sftp:test!/folder";
  const app = await setupApp({ wrap: api => ({ ...api, copy: vi.fn(async () => 999) }) });
  act(() => app.base.devPlug({ name: "Irrelevant", path: remote, mounted: false, fstab: { userMountable: true, automount: false, noauto: true } }));
  await screen.findByRole("treeitem", { name: /^Irrelevant/ });
  dragFocused(app);
  act(() => { app.runtime.controller.dragEnd(); app.runtime.controller.openTransfer("copy", 0, DOWNLOADS); });
  const dialog = app.runtime.store.getState().dialog;
  expect(dialog?.kind).toBe("transfer");
  if (dialog?.kind !== "transfer") throw new Error("transfer missing");
  await act(async () => { await app.runtime.controller.confirmTransfer(dialog.plan, remote); });
  expect(app.api.copy).toHaveBeenCalledWith(expect.any(Array), remote);
});

test.each(["de", "en"] as const)("trash confirmation caps displayed paths without changing the operation (%s)", async language => {
  const trash = vi.fn(async () => 999);
  const app = await setupApp({ settings: { language }, wrap: api => ({ ...api, trash }) });
  const paths = Array.from({ length: 25 }, (_, i) => `${HOME}/selected-${i}`);
  act(() => app.runtime.store.dispatch({ type: "dialog", dialog: { kind: "trashConfirm", paths } }));
  const dialog = screen.getByRole("alertdialog");
  expect(within(dialog).getAllByRole("listitem")).toHaveLength(21);
  expect(within(dialog).getByText(paths[19]!)).toBeVisible();
  expect(within(dialog).queryByText(paths[20]!)).toBeNull();
  expect(within(dialog).getByText(language === "de" ? "+5 weitere" : "+5 more")).toBeVisible();
  await app.user.click(within(dialog).getByRole("button", { name: language === "de" ? "Ja, Papierkorb" : "Yes, move to Trash" }));
  expect(trash).toHaveBeenCalledWith(paths);
});

test("unmounted devices mount before navigating to the returned path", async () => {
  const app = await setupApp();
  const mount = vi.spyOn(app.api, "deviceMount").mockResolvedValue(DOCS);
  act(() => app.base.devPlug({ name: "Unmounted USB", path: "", kind: "removable", source: "device", mounted: false,
    deviceActions: { key: "usb:1", removable: true, encrypted: false, optical: false } }));
  const row = await screen.findByRole("treeitem", { name: /^Unmounted USB/ });
  expect(row).not.toHaveAttribute("aria-disabled", "true");
  expect(sidebar().querySelector('[aria-live="polite"]')).toHaveTextContent("Unmounted USB angeschlossen");
  fireEvent.click(row, { ctrlKey: true });
  await waitFor(() => expect(mount).toHaveBeenCalledWith("usb:1"));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 1).path).toBe(DOCS));
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME);
});

test("whole-drive removal confirms tabs on another partition and moves them only after success", async () => {
  const app = await setupApp();
  const info = { removable: true, encrypted: false, optical: false };
  act(() => {
    app.base.devPlug({ name: "Partition one", path: DOCS, kind: "removable", mounted: true, deviceActions: { ...info, key: "usb:1" } });
    app.base.devPlug({ name: "Partition two", path: DOWNLOADS, kind: "removable", mounted: true, deviceActions: { ...info, key: "usb:2" } });
  });
  await screen.findByRole("treeitem", { name: /^Partition two/ });
  await app.user.click(item(/^Partition two/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOWNLOADS));
  let finish!: () => void;
  const eject = vi.spyOn(app.api, "deviceEject").mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  const affected = vi.spyOn(app.api, "placesMountAffected");
  await app.user.click(screen.getByRole("button", { name: "Auswerfen · Partition one" }));
  expect(affected).toHaveBeenCalledWith("whole-device:usb:1", expect.arrayContaining([DOWNLOADS]));
  expect(app.runtime.store.getState().dialog).toMatchObject({ kind: "placeConfirm", action: "eject" });
  expect(eject).not.toHaveBeenCalled();
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Bestätigen" }));
  expect(eject).toHaveBeenCalledWith("usb:1");
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOWNLOADS);
  await act(async () => finish());
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME));
});

test.each(["busy", "notPermitted", "notFound", "helperMissing", "timeout", "polkitDismissed"])("failed removal (%s) preserves tabs and never exposes helper diagnostics", async kind => {
  const app = await setupApp();
  act(() => app.base.devPlug({ name: "Test device", path: DOCS, kind: "removable", mounted: true,
    deviceActions: { key: "usb:1", removable: true, encrypted: false, optical: false } }));
  await screen.findByRole("treeitem", { name: /^Test device/ });
  await app.user.click(item(/^Test device/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  vi.spyOn(app.api, "devicePowerOff").mockRejectedValue(new CoreError("serial=SECRET credentials=/secret", kind));
  await act(() => app.runtime.controller.unmountPlace("drive:device-usb:1", false, "powerOff"));
  await act(() => app.runtime.controller.confirmPlaceAction());
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS);
  expect(document.body).not.toHaveTextContent("SECRET");
  expect(app.runtime.store.getState().dialog).toBeNull();
});

test("cancelled eject leaves the device and tabs untouched; encrypted media do not mount", async () => {
  const app = await setupApp();
  act(() => app.base.devPlug({ name: "Test device", path: DOCS, kind: "removable", mounted: true,
    deviceActions: { key: "usb:1", removable: true, encrypted: false, optical: false } }));
  await screen.findByRole("treeitem", { name: /^Test device/ });
  await app.user.click(item(/^Test device/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  const eject = vi.spyOn(app.api, "deviceEject"), mount = vi.spyOn(app.api, "deviceMount");
  await app.user.click(screen.getByRole("button", { name: "Auswerfen · Test device" }));
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Abbrechen" }));
  expect(eject).not.toHaveBeenCalled();
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS);
  act(() => app.base.devPlug({ name: "Locked USB", path: "", kind: "removable", mounted: false,
    deviceActions: { key: "locked:1", removable: true, encrypted: true, optical: false } }));
  const locked = await screen.findByRole("treeitem", { name: /^Locked USB/ });
  expect(locked).toHaveAttribute("aria-disabled", "true");
  expect(locked).toHaveAccessibleName(/Verschlüsselt/);
  await app.user.click(locked);
  expect(mount).not.toHaveBeenCalled();
});

test("partial removal failure refreshes mount state without moving tabs", async () => {
  const app = await setupApp();
  act(() => app.base.devPlug({ name: "Partial", path: DOCS, kind: "removable", mounted: true,
    deviceActions: { key: "partial:1", removable: true, encrypted: false, optical: false } }));
  await screen.findByRole("treeitem", { name: /^Partial/ });
  await app.user.click(item(/^Partial/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  vi.spyOn(app.api, "devicePowerOff").mockImplementation(async () => {
    await app.api.deviceUnmount("partial:1");
    throw new CoreError("not permitted", "notPermitted");
  });
  await act(() => app.runtime.controller.unmountPlace("drive:device-partial:1", false, "powerOff"));
  await act(() => app.runtime.controller.confirmPlaceAction());
  expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS);
  expect(app.runtime.store.getState().places?.drives.find(d => d.name === "Partial")?.mounted).toBe(false);
});


test("optical eject uses the eject endpoint without powering off the drive", async () => {
  const app = await setupApp();
  act(() => app.base.devPlug({ name: "Optical disc", path: DOCS, kind: "removable", source: "device", mounted: true,
    deviceActions: { key: "optical:1", removable: true, encrypted: false, optical: true } }));
  await screen.findByRole("treeitem", { name: /^Optical disc/ });
  await app.user.click(item(/^Optical disc/));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(DOCS));
  const eject = vi.spyOn(app.api, "deviceEject");
  const powerOff = vi.spyOn(app.api, "devicePowerOff");
  await app.user.click(screen.getByRole("button", { name: "Auswerfen · Optical disc" }));
  expect(eject).not.toHaveBeenCalled();
  await app.user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Bestätigen" }));
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toBe(HOME));
  expect(eject).toHaveBeenCalledWith("optical:1");
  expect(powerOff).not.toHaveBeenCalled();
  expect(screen.queryByRole("treeitem", { name: /^Optical disc/ })).not.toBeInTheDocument();
});

test("a desktop bookmark is announced as system place, can be hidden, and a click saves and connects it", async () => {
  const app = await setupApp({ samples: true });
  const save = vi.spyOn(app.api, "placesSaveConnection");
  const row = () => within(sidebar()).queryByRole("treeitem", { name: /^Home server · SFTP · Nicht verbunden · Systemort/ });
  await waitFor(() => expect(row()).toBeInTheDocument());
  // The visible marker is decorative; the text lives in the accessible name.
  expect(row()!.querySelector(".place-system")).toHaveAttribute("aria-hidden", "true");
  fireEvent.contextMenu(row()!);
  expect(screen.getByRole("menuitem", { name: "Als Verbindung speichern" })).toBeInTheDocument();
  await app.user.click(screen.getByRole("menuitem", { name: "Eintrag ausblenden" }));
  expect(app.runtime.store.getState().settings.sidebarHidden).toContain("system:mock0");
  expect(row()).toBeNull();
  act(() => app.runtime.controller.hidePlace("system:mock0", false));
  await app.user.click(row()!);
  // Same path as a saved connection: the connect dialog runs and the pane opens the remote root.
  await waitFor(() => expect(activeTabOf(app.runtime.store.getState(), 0).path).toMatch(/^sftp:/));
  expect(save).toHaveBeenCalledWith("system:mock0");
  expect(app.runtime.store.getState().connections).toMatchObject([{ connected: true, profile: { name: "Home server", protocol: "sftp", host: "nas" } }]);
  // Now a saved connection: listed once, without the system-place wording.
  await waitFor(() => expect(row()).toBeNull());
  expect(within(sidebar()).getAllByRole("treeitem", { name: /^Home server/ })).toHaveLength(1);
});
