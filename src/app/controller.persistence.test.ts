import { afterEach, expect, test, vi } from "vitest";
import { createRuntime } from "./runtime";
import { createMockApi } from "../ipc/mock";
import type { CoreEvent } from "../ipc/types";

afterEach(() => vi.useRealTimers());
async function setup() {
  vi.useFakeTimers();
  const base = createMockApi();
  let emit!: (event: CoreEvent) => void;
  const saveSettings = vi.fn(base.saveSettings);
  const appCloseNow = vi.fn(async () => {});
  const runtime = createRuntime({ ...base, saveSettings, appCloseNow,
    onEvent: listener => { emit = listener; return () => {}; },
  });
  await runtime.controller.start();
  return { ...runtime, saveSettings, appCloseNow, emit };
}
test("navigation and tabs debounce to the latest session without duplicate writes", async () => {
  const r = await setup();
  await r.controller.navigateTo(0, "/home/demo/Dokumente");
  r.controller.newTab(0);
  await r.controller.navigateTo(0, "/home/demo/Downloads");
  await vi.advanceTimersByTimeAsync(399);
  expect(r.saveSettings).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(r.saveSettings).toHaveBeenCalledTimes(1);
  expect(r.saveSettings.mock.calls[0]![0].session.windows[0].tabs.map(t => t.path)).toEqual(["/home/demo/Dokumente", "/home/demo/Downloads"]);
  await r.controller.saveNow();
  expect(r.saveSettings).toHaveBeenCalledTimes(1);
  r.controller.dispose();
});
test("close signal waits for an in-flight save before acknowledging", async () => {
  const r = await setup();
  let release!: () => void;
  r.saveSettings.mockImplementationOnce(() => new Promise<void>(resolve => { release = resolve; }));
  await r.controller.navigateTo(0, "/home/demo/Dokumente");
  await vi.advanceTimersByTimeAsync(400);
  r.emit({ type: "closeSaveRequested" } as CoreEvent);
  await vi.advanceTimersByTimeAsync(0);
  expect(r.appCloseNow).not.toHaveBeenCalled();
  release();
  await vi.advanceTimersByTimeAsync(0);
  expect(r.appCloseNow).toHaveBeenCalledTimes(1);
  expect(r.saveSettings).toHaveBeenCalledTimes(1);
  r.controller.dispose();
});
test("close signal flushes pending state and save failure still acknowledges", async () => {
  const r = await setup();
  r.saveSettings.mockRejectedValueOnce(new Error("disk full"));
  await r.controller.navigateTo(0, "/home/demo/Dokumente");
  r.emit({ type: "closeSaveRequested" } as CoreEvent);
  await vi.advanceTimersByTimeAsync(0);
  expect(r.saveSettings).toHaveBeenCalledTimes(1);
  expect(r.appCloseNow).toHaveBeenCalledTimes(1);
  r.controller.dispose();
});
test("a folder that keeps reporting changes does not postpone a pending save forever", async () => {
  const r = await setup();
  const home = r.store.getState().home;
  r.controller.updateSettings(s => ({ ...s, language: s.language === "en" ? "de" : "en" }));
  // The watcher debounces at 250 ms: a busy folder reloads faster than the save delay (400 ms).
  for (let i = 0; i < 12; i++) {
    await vi.advanceTimersByTimeAsync(250);
    r.emit({ type: "dirChanged", path: home });
  }
  expect(r.saveSettings).toHaveBeenCalledTimes(1);
  r.controller.dispose();
});
