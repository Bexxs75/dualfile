import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { setupApp } from "./test-utils";
import type { DiscoveredService } from "./ipc/types";
import { createMockApi } from "./ipc/mock";
const service: DiscoveredService = { key: "nas-smb-445", name: "Office NAS", protocol: "smb", host: "nas.local", port: 445, path: "documents" };
const network = () => screen.queryByRole("treeitem", { name: "Im Netzwerk gefunden" });

test("LAN discovery defaults off, including fresh settings, and missing Avahi stays hidden", async () => {
  expect((await createMockApi().loadSettings()).settings.sidebarDiscovery).toBe(false);
  const app = await setupApp();
  act(() => app.base.devDiscover(service));
  expect(network()).toBeNull();
  await act(async () => { app.runtime.controller.updateSettings(s => ({ ...s, sidebarDiscovery: true })); await app.runtime.controller.saveNow(); });
  expect(network()).toBeNull();
  expect(app.runtime.store.getState().dialog).toBeNull();
  expect(screen.getAllByRole("grid")).toHaveLength(2);
});

test("two LAN services add/remove live; clicks only open the editor, including modified clicks", async () => {
  const app = await setupApp({ settings: { sidebarDiscovery: true } });
  const editor = vi.spyOn(app.api, "connectionEditorOpen");
  const connect = vi.spyOn(app.api, "connectionConnect"), save = vi.spyOn(app.api, "connectionSave"), secret = vi.spyOn(app.api, "secretSet");
  const before = app.runtime.store.getState().windows;
  act(() => { app.base.devDiscover(service); app.base.devDiscover({ ...service, key: "second", name: "Second NAS", host: "second.local" }); });
  const entry = await screen.findByRole("treeitem", { name: /^Office NAS/ });
  expect(network()).toBeVisible();
  expect(screen.getByRole("treeitem", { name: /^Second NAS/ })).toBeVisible();
  await app.user.click(entry);
  fireEvent.click(entry, { ctrlKey: true });
  fireEvent(entry, new MouseEvent("auxclick", { bubbles: true, button: 1 }));
  await waitFor(() => expect(editor).toHaveBeenCalledTimes(3));
  expect(editor).toHaveBeenLastCalledWith(null, expect.any(String), expect.any(Object), service.key);
  expect(app.runtime.store.getState().windows).toBe(before);
  expect(connect).not.toHaveBeenCalled(); expect(save).not.toHaveBeenCalled(); expect(secret).not.toHaveBeenCalled();
  act(() => app.base.devForget(service.key));
  await waitFor(() => expect(screen.queryByRole("treeitem", { name: /^Office NAS/ })).toBeNull());
  expect(network()).toBeVisible();
  act(() => app.base.devForget());
  await waitFor(() => expect(network()).toBeNull());
});

test("disabling hides and clears immediately; re-enabling waits for fresh announcements", async () => {
  const app = await setupApp({ settings: { sidebarDiscovery: true }, options: { saveDelayMs: 60_000 } });
  const save = vi.spyOn(app.api, "saveSettings");
  act(() => app.base.devDiscover(service));
  await screen.findByRole("treeitem", { name: /^Office NAS/ });
  act(() => app.runtime.controller.updateSettings(s => ({ ...s, sidebarDiscovery: false })));
  expect(network()).toBeNull();
  expect(app.runtime.store.getState().places?.discoveredServices).toEqual([]);
  await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ sidebarDiscovery: false })));
  await act(async () => { await app.runtime.controller.saveNow(); app.runtime.controller.updateSettings(s => ({ ...s, sidebarDiscovery: true })); await app.runtime.controller.saveNow(); });
  expect(network()).toBeNull();
  act(() => app.base.devDiscover(service));
  expect(await screen.findByRole("treeitem", { name: /^Office NAS/ })).toBeVisible();
});

test("network rows work in the icon rail, retain hide/collapse settings and reject drops", async () => {
  const app = await setupApp({ settings: { sidebarDiscovery: true, sidebarMode: "icons" } });
  act(() => app.base.devDiscover(service));
  const entry = await screen.findByRole("treeitem", { name: /^Office NAS/ });
  const editor = vi.spyOn(app.api, "connectionEditorOpen"), copy = vi.spyOn(app.api, "copy");
  act(() => app.runtime.controller.dragStart(0, 0));
  fireEvent.drop(entry);
  expect(copy).not.toHaveBeenCalled(); expect(editor).not.toHaveBeenCalled();
  act(() => entry.focus());
  await app.user.keyboard("{Enter}");
  await waitFor(() => expect(editor).toHaveBeenCalledOnce());
  await app.user.click(network()!);
  expect(app.runtime.store.getState().settings.sidebarCollapsed).toContain("network");
  await app.user.click(network()!);
  expect(within(screen.getByRole("navigation", { name: "Orte" })).getByRole("treeitem", { name: /^Office NAS/ })).toBeVisible();
});
