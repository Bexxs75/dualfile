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
  expect(rt.store.getState().settings.columnWidths).toBeUndefined();
  fireEvent.keyUp(size, { key: "ArrowLeft" });
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([100, 104, 130]);
  fireEvent.keyDown(size, { key: "ArrowRight" });
  fireEvent.blur(size);
  expect(rt.store.getState().settings.columnWidths?.normal).toEqual([92, 104, 130]);
});
