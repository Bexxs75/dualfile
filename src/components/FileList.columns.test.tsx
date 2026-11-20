import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { RuntimeProvider } from "../app/context";
import { createRuntime, type AppRuntime } from "../app/runtime";
import { defaultSettings } from "../app/state";
import { createMockApi } from "../ipc/mock";
import type { Settings } from "../ipc/types";
import { ManualScheduler } from "../test-utils";
import { FileList } from "./FileList";

const runtimes: AppRuntime[] = [];
afterEach(() => runtimes.splice(0).forEach((r) => r.controller.dispose()));

async function setup(patch: Partial<Settings> = {}) {
  const api = createMockApi({ scheduler: new ManualScheduler(), initialSettings: { ...defaultSettings, language: "en", ...patch } });
  const rt = createRuntime(api, { saveDelayMs: 1 });
  runtimes.push(rt);
  await rt.controller.start();
  render(<RuntimeProvider runtime={rt}><FileList w={0} /></RuntimeProvider>);
  return { rt };
}
const cols = () => screen.getByRole("grid").style.getPropertyValue("--df-cols");
const handle = (name: RegExp) => screen.getByRole("separator", { name });

test("default template matches the old CSS widths in both modes; name column has no handle", async () => {
  await setup();
  expect(cols()).toBe("minmax(120px, 1fr) 84px 104px 130px");
  expect(screen.getAllByRole("separator")).toHaveLength(3);
  expect(screen.queryByRole("separator", { name: /Name/ })).toBeNull();
});

test("ext mode uses its own widths", async () => {
  await setup({ extensionColumn: true, columnWidths: { ext: [70, 90, 140] } });
  expect(cols()).toBe("minmax(120px, 1fr) 70px 90px 140px");
});

test("stored widths are clamped on read", async () => {
  await setup({ columnWidths: { normal: [10, 999, 130] } });
  expect(cols()).toBe("minmax(120px, 1fr) 48px 400px 130px");
});

test("pointer drag updates live, clamps, and persists once on pointer-up", async () => {
  const { rt } = await setup();
  const before = rt.store.getState().settings;
  fireEvent.pointerDown(handle(/Size/), { button: 0, clientX: 100 });
  // Handle is on the column's left edge: dragging left widens it.
  act(() => { window.dispatchEvent(new PointerEvent("pointermove", { clientX: 70 })); });
  expect(cols()).toBe("minmax(120px, 1fr) 114px 104px 130px");
  expect(rt.store.getState().settings).toBe(before); // nothing persisted mid-drag
  act(() => { window.dispatchEvent(new PointerEvent("pointermove", { clientX: -900 })); });
  expect(cols()).toBe("minmax(120px, 1fr) 400px 104px 130px");
  act(() => { window.dispatchEvent(new PointerEvent("pointerup", { clientX: -900 })); });
  expect(rt.store.getState().settings.columnWidths).toEqual({ normal: [400, 104, 130] });
  expect(cols()).toBe("minmax(120px, 1fr) 400px 104px 130px");
});

test("drag below the minimum clamps to 48", async () => {
  const { rt } = await setup();
  fireEvent.pointerDown(handle(/Modified/), { button: 0, clientX: 500 });
  // Dragging the left-edge handle right narrows the column, down to the 48px minimum.
  act(() => { window.dispatchEvent(new PointerEvent("pointermove", { clientX: 900 })); });
  act(() => { window.dispatchEvent(new PointerEvent("pointerup", { clientX: 900 })); });
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([84, 104, 48]);
});

test("double-click resets that column to its default", async () => {
  const { rt } = await setup({ columnWidths: { normal: [200, 300, 250] } });
  fireEvent.doubleClick(handle(/Type/));
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([200, 104, 250]);
  expect(cols()).toBe("minmax(120px, 1fr) 200px 104px 250px");
});

test("arrow keys nudge by 8px and persist on keyup; blur also persists", async () => {
  const { rt } = await setup();
  const size = handle(/Size/);
  expect(size).toHaveAttribute("aria-valuenow", "84");
  fireEvent.keyDown(size, { key: "ArrowLeft" });
  fireEvent.keyDown(size, { key: "ArrowLeft" });
  expect(cols()).toBe("minmax(120px, 1fr) 100px 104px 130px");
  expect(rt.store.getState().settings.columnWidths).toEqual({});
  fireEvent.keyUp(size, { key: "ArrowLeft" });
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([100, 104, 130]);
  fireEvent.keyDown(size, { key: "ArrowRight" });
  fireEvent.blur(size);
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([92, 104, 130]);
});

test.each([false, true])("ownership columns are optional, sortable and keep three resize handles (ext=%s)", async extensionColumn => {
  const { rt } = await setup({ extensionColumn });
  expect(screen.queryByRole("columnheader", { name: /Permissions/ })).toBeNull();
  act(() => rt.controller.updateSettings(s => ({ ...s, showPermissionsColumn: true, showUidColumn: true, showGidColumn: true })));
  expect(screen.getAllByRole("columnheader")).toHaveLength(7);
  expect(screen.getAllByRole("separator")).toHaveLength(3);
  for (const [label, key] of [["Permissions", "permissions"], ["uid", "uid"], ["gid", "gid"]]) {
    const header = screen.getByRole("columnheader", { name: new RegExp(label!) });
    fireEvent.click(header.querySelector("button")!);
    expect(header).toHaveAttribute("aria-sort", "ascending");
    expect(rt.store.getState().windows[0].tabs[0]!.sort).toBe(`${key}:asc`);
    fireEvent.click(header.querySelector("button")!);
    expect(header).toHaveAttribute("aria-sort", "descending");
  }
  act(() => rt.controller.updateSettings(s => ({ ...s, showPermissionsColumn: false, showUidColumn: false, showGidColumn: false })));
  expect(screen.getAllByRole("columnheader")).toHaveLength(4);
});

test.each(["/local", "sftp:server!/remote"])("owner cells use local names only, numeric tooltips and empty missing values: %s", async path => {
  const { rt } = await setup({ showPermissionsColumn: true, showUidColumn: true, showGidColumn: true });
  const entries = [{ name: "owned", kind: "file" as const, size: 0, modified: null, mode: 0o7755, uid: 1000, gid: 100, ownerName: "alice", groupName: "staff", hidden: false, linkTarget: null, brokenLink: false, error: null },
    { name: "unknown", kind: "file" as const, size: 0, modified: null, mode: null, uid: 12345, gid: null, hidden: false, linkTarget: null, brokenLink: false, error: null }];
  act(() => rt.store.dispatch({ type: "window", w: 0,
    update: win => ({ ...win, tabs: win.tabs.map((tab, i) => i === win.activeTab ? { ...tab, path } : tab) }),
    listing: { path, entries },
  }));
  expect(screen.getByTitle("uid 1000")).toHaveTextContent(path.startsWith("/") ? "alice" : "1000");
  expect(screen.getByTitle("gid 100")).toHaveTextContent(path.startsWith("/") ? "staff" : "100");
  expect(screen.getByTitle("uid 12345")).toHaveTextContent("12345");
  expect(screen.getByText("rwsr-sr-t")).toBeInTheDocument();
});

test.each([false, true])("owner layout shares responsive templates and preserves stored widths (ext=%s)", async extensionColumn => {
  const { rt } = await setup({ extensionColumn, showPermissionsColumn: true, showUidColumn: true, showGidColumn: true,
    columnWidths: { normal: [400, 400, 400], ext: [400, 400, 400] } });
  const grid = screen.getByRole("grid");
  expect(grid).toHaveClass("owner-columns");
  expect(grid.style.getPropertyValue("--df-min")).toBe("");
  expect(grid.style.getPropertyValue("--df-owner-all")).toContain("min(400px");
  expect(grid.style.getPropertyValue("--df-owner-all")).toContain("96px 64px 64px");
  if (extensionColumn) {
    // Ext bleibt im kleinen Layout als eigene Spalte erhalten
    expect(grid.style.getPropertyValue("--df-owner-small")).toMatch(/^minmax\(120px, 1fr\) min\(400px, .*\) 96px 64px 64px$/);
  } else {
    expect(grid.style.getPropertyValue("--df-owner-small")).toBe("minmax(120px, 1fr) 96px 64px 64px");
  }
  expect(grid.style.getPropertyValue("--df-owner-minimal")).toBe("minmax(120px, 1fr) minmax(0, 96px) minmax(0, 64px) minmax(0, 64px)");
  for (const row of grid.querySelectorAll(".head, .row")) {
    expect(row.classList.contains("ext-split")).toBe(extensionColumn);
    expect(row.querySelectorAll(":scope > span")).toHaveLength(7);
  }
  expect(rt.store.getState().settings.columnWidths).toEqual({ normal: [400, 400, 400], ext: [400, 400, 400] });
});

test("a hidden owner sort reverses the visible name sort on the first click", async () => {
  const { rt } = await setup({ showPermissionsColumn: false, showUidColumn: false, showGidColumn: false });
  act(() => rt.store.dispatch({ type: "window", w: 0,
    update: win => ({ ...win, tabs: win.tabs.map((tab, i) => i === win.activeTab ? { ...tab, sort: "uid:desc" } : tab) }) }));
  act(() => rt.controller.sortBy(0, "name"));
  const win = rt.store.getState().windows[0];
  expect(win.tabs[win.activeTab]!.sort).toBe("name:desc");
});

test("tint belongs only to the row group, including empty space", async () => {
  await setup({ paneColorLeft: "#aabbcc", paneTintStrength: 100 });
  const grid = screen.getByRole("grid");
  const body = screen.getByRole("rowgroup");
  expect(body).toHaveAttribute("data-tint");
  expect(body.style.getPropertyValue("--panel")).toBe("#aabbcc");
  expect(grid).not.toHaveAttribute("data-tint");
  expect(grid.querySelector(".head")?.closest("[data-tint]")).toBeNull();
});

for (const extensionColumn of [false, true]) {
  test.each([[true, false, false], [false, false, true], [false, true, true], [true, true, true]])(
    `independent owner subset %s/%s/%s (ext=${extensionColumn})`, async (permissions, uid, gid) => {
      await setup({ extensionColumn, showPermissionsColumn: permissions, showUidColumn: uid, showGidColumn: gid });
      const grid = screen.getByRole("grid");
      const count = 4 + [permissions, uid, gid].filter(Boolean).length;
      for (const row of grid.querySelectorAll(".head, .row")) expect(row.children).toHaveLength(count);
      const fixed = [permissions ? "minmax(0, 96px)" : "", uid ? "minmax(0, 64px)" : "", gid ? "minmax(0, 64px)" : ""].filter(Boolean).join(" ");
      expect(grid.style.getPropertyValue("--df-owner-minimal")).toBe(`minmax(120px, 1fr) ${fixed}`);
      expect(grid.querySelector('.head [data-column="size"]')).not.toBeNull();
      expect(grid.querySelector('.head [data-column="modified"]')).not.toBeNull();
    });
}
