import { afterEach, expect, test, vi } from "vitest";
import { createMockApi } from "../ipc/mock";
import { createRuntime } from "./runtime";
import { createSettingsWindowClient } from "./settingsWindow";
import type { SettingsWindowDelivery, SettingsWindowMessage } from "../ipc/types";
import { testSnapshot } from "./settingsWindow.fixture";

const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
afterEach(() => vi.useRealTimers());
async function bridge() {
  const api = createMockApi();
  const runtime = createRuntime(api);
  let receive!: (message: SettingsWindowMessage) => void;
  api.onSettingsWindowMessage = fn => { receive = fn; return () => {}; };
  const push = vi.spyOn(api, "settingsWindowPush");
  const save = vi.spyOn(api, "saveSettings");
  await runtime.controller.start();
  await runtime.controller.saveNow(); save.mockClear();
  const stop = runtime.settingsWindow.start();
  receive({ session: "s1", seq: 0, kind: "session" });
  await tick();
  return { api, runtime, push, save, receive, stop: () => { stop(); runtime.controller.dispose(); } };
}
test("bridge merges whitelisted patches, preserves main changes, and saves once after 400ms", async () => {
  vi.useFakeTimers();
  const b = await bridge();
  b.runtime.controller.updateSettings(s => ({ ...s, theme: "papier" }));
  b.receive({ session: "s1", seq: 1, kind: "patch", patch: { fontSize: 18, bookmarks: [{ name: "injected", path: "/bad", color: "#123456", order: 0 }] } } as unknown as SettingsWindowMessage);
  await tick();
  expect(b.runtime.store.getState().settings).toMatchObject({ theme: "papier", fontSize: 18 });
  expect(b.runtime.store.getState().settings.bookmarks.some(b => b.name === "injected")).toBe(false);
  expect(b.save).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(399); expect(b.save).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1); expect(b.save).toHaveBeenCalledOnce();
  const snapshot = b.push.mock.lastCall![0];
  expect(snapshot.ack).toBe(1); expect(snapshot.settings).not.toHaveProperty("bookmarks");
  expect(snapshot.settings).not.toHaveProperty("session"); expect(snapshot.logDir).toBe("/home/demo");
  b.stop();
});
test("bridge serializes actions and ignores the old session when reopened during an action", async () => {
  const b = await bridge();
  let release!: () => void;
  vi.spyOn(b.runtime.system, "setStartMenu").mockImplementation(() => new Promise(resolve => { release = resolve; }));
  b.receive({ session: "s1", seq: 1, kind: "setStartMenu", enabled: true });
  b.receive({ session: "s1", seq: 2, kind: "patch", patch: { fontSize: 17 } });
  await tick(); expect(b.runtime.store.getState().settings.fontSize).toBe(13);
  b.receive({ session: "s2", seq: 0, kind: "session" });
  b.receive({ session: "s2", seq: 1, kind: "patch", patch: { fontSize: 19 } });
  release(); await tick();
  expect(b.runtime.store.getState().settings.fontSize).toBe(19);
  expect(b.push.mock.lastCall![0]).toMatchObject({ session: "s2", ack: 1 });
  expect(b.push.mock.calls.filter(([s]) => s.session === "s2").map(([s]) => s.ack)).toEqual([0, 0, 1]);
  b.stop();
});
test("system and persistence changes push snapshots; flush reports save errors", async () => {
  const b = await bridge();
  await b.runtime.system.check(true); await tick();
  expect(b.push.mock.lastCall![0].system.check).toBe("current");
  b.runtime.controller.updateSettings(s => ({ ...s, fontSize: 20 }));
  vi.spyOn(b.api, "saveSettings").mockRejectedValue(new Error("disk full"));
  b.receive({ session: "s1", seq: 1, kind: "flush" }); await tick();
  expect(b.push.mock.lastCall![0]).toMatchObject({ ack: 1, persistence: { error: "disk full" }, errors: [{ seq: 1, message: "disk full" }] });
  b.stop();
});
test("client orders invokes, rebases optimistic patches and resets missing optional keys", async () => {
  const api = createMockApi(), rt = createRuntime(api);
  let deliver!: (value: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { deliver = fn; };
  let release!: () => void;
  const send = vi.spyOn(api, "settingsWindowSend").mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  const client = createSettingsWindowClient(api, rt); await client.attach();
  deliver({ kind: "session", session: "s1" });
  deliver({ kind: "snapshot", snapshot: testSnapshot({ settings: { ...testSnapshot().settings, paneColorLeft: "#123456" } }) });
  client.runtime.controller.updateSettings(s => ({ ...s, fontSize: 18 }));
  client.runtime.controller.updateSettings(s => ({ ...s, trayEnabled: true }));
  void client.runtime.system.setTray(true, "en"); await tick();
  expect(send).toHaveBeenCalledTimes(1);
  deliver({ kind: "snapshot", snapshot: testSnapshot() });
  expect(rt.store.getState().settings.fontSize).toBe(18);
  expect(rt.store.getState().settings.paneColorLeft).toBeUndefined();
  release(); await tick();
  expect(send.mock.calls.map(([m]) => m.kind)).toEqual(["patch", "patch", "setTray"]);
  deliver({ kind: "snapshot", snapshot: testSnapshot({ session: "old", ack: 99 }) });
  expect(rt.store.getState().settings.fontSize).toBe(18);
});
test("close waits for main acknowledgement, never closes on a timeout, and retains errors", async () => {
  vi.useFakeTimers();
  const api = createMockApi(), rt = createRuntime(api);
  let deliver!: (value: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { deliver = fn; };
  const close = vi.spyOn(api, "settingsWindowClose");
  const client = createSettingsWindowClient(api, rt); await client.attach();
  deliver({ kind: "session", session: "s1" }); deliver({ kind: "snapshot", snapshot: testSnapshot() });
  const done = client.close(); await tick(); await vi.advanceTimersByTimeAsync(10000);
  expect(close).toHaveBeenCalledWith("s1", 1, false, true); expect(client.getSnapshot().closing).toBe(false);
  expect(client.getSnapshot().canForce).toBe(true);
  deliver({ kind: "snapshot", snapshot: testSnapshot({ ack: 1, errors: [{ seq: 1, message: "disk full" }] }) });
  await done; expect(close.mock.calls.every(call => call[3] === true)).toBe(true); expect(client.getSnapshot().error).toBe("disk full");
  const retry = client.close(); await tick();
  deliver({ kind: "snapshot", snapshot: testSnapshot({ ack: 2 }) });
  await retry; expect(close).toHaveBeenCalledWith("s1", 2);
});
test("a rejected send rolls back its optimistic patch and displays its error", async () => {
  const api = createMockApi(), rt = createRuntime(api);
  let deliver!: (value: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { deliver = fn; };
  vi.spyOn(api, "settingsWindowSend").mockRejectedValue(new Error("invalid patch"));
  const client = createSettingsWindowClient(api, rt); await client.attach();
  deliver({ kind: "session", session: "s1" }); deliver({ kind: "snapshot", snapshot: testSnapshot() });
  client.runtime.controller.updateSettings(s => ({ ...s, fontSize: 33 })); await tick();
  expect(rt.store.getState().settings.fontSize).toBe(13);
  expect(client.getSnapshot().error).toBe("invalid patch");
});
test("install waits for the native settings close acknowledgement", async () => {
  const b = await bridge();
  vi.spyOn(b.api, "settingsWindowPush").mockResolvedValue(true);
  let done = false;
  const flush = b.runtime.settingsWindow.flush().then(() => { done = true; });
  await tick(); expect(done).toBe(false);
  expect(b.push.mock.lastCall?.[2]).toBe(true);
  b.receive({ kind: "closed", session: "old", seq: 10 }); await tick(); expect(done).toBe(false);
  b.receive({ kind: "closed", session: "s1", seq: 1 }); await flush; expect(done).toBe(true);
  b.stop();
});
test("client attaches once even when its mount effect runs again", async () => {
  const api = createMockApi();
  const attach = vi.spyOn(api, "settingsWindowAttach");
  const client = createSettingsWindowClient(api, createRuntime(api));
  await Promise.all([client.attach(), client.attach()]);
  expect(attach).toHaveBeenCalledOnce();
});

test("drain is a no-op without an active bridge or latest snapshot", async () => {
  const api = createMockApi(); const rt = createRuntime(api);
  const push = vi.spyOn(api, "settingsWindowPush");
  await rt.settingsWindow.flush();
  const stop = rt.settingsWindow.start();
  await rt.settingsWindow.flush(); stop();
  expect(push).not.toHaveBeenCalled();
});
test("failed pushes retry the same ack before later snapshots", async () => {
  vi.useFakeTimers(); const b = await bridge();
  b.push.mockClear(); b.push.mockRejectedValueOnce(new Error("temporary"));
  b.receive({ session: "s1", seq: 1, kind: "checkUpdates" }); await tick();
  const first = b.push.mock.calls[0]![0];
  await vi.advanceTimersByTimeAsync(199); expect(b.push).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(b.push.mock.calls[1]![0]).toBe(first);
  expect(b.push.mock.lastCall![0].ack).toBe(1);
  b.stop();
});
test("exhausted pushes send a visible channel error and cancel the drain", async () => {
  vi.useFakeTimers(); const b = await bridge();
  b.push.mockClear(); b.push.mockRejectedValue(new Error("relay unavailable"));
  b.receive({ session: "s1", seq: 1, kind: "patch", patch: { fontSize: 15 } }); await tick();
  await vi.advanceTimersByTimeAsync(1700);
  expect(b.push.mock.calls.some(call => call[3]?.error === "relay unavailable" && call[3]?.cancelClose)).toBe(true);
  b.stop();
});
test("installation drain times out, cancels the intent and ignores a late queued request", async () => {
  vi.useFakeTimers(); const b = await bridge();
  let release!: (value: boolean) => void;
  b.push.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
  b.runtime.controller.updateSettings(s => ({ ...s, fontSize: 16 })); await tick();
  const result = b.runtime.settingsWindow.flush().catch(error => error as Error);
  await vi.advanceTimersByTimeAsync(5000);
  expect((await result as Error).message).toMatch(/Installation|installation/);
  expect(b.push.mock.lastCall?.[3]).toEqual({ cancelClose: true });
  release(false); await tick();
  expect(b.push.mock.calls.some(call => call[2] === true)).toBe(false);
  b.stop();
});
test("failed flush immediately cancels the install drain", async () => {
  const b = await bridge(); b.push.mockResolvedValue(true);
  const result = b.runtime.settingsWindow.flush().catch(error => error as Error);
  await tick();
  b.receive({ kind: "closeFailed", session: "s1", seq: 0 });
  expect((await result as Error).message).toMatch(/Installation|installation/);
  expect(b.push.mock.lastCall?.[3]).toEqual({ cancelClose: true }); b.stop();
});
test("attach rejection allows a force close without a session", async () => {
  const api = createMockApi(); vi.spyOn(api, "settingsWindowAttach").mockRejectedValue(new Error("attach denied"));
  const close = vi.spyOn(api, "settingsWindowClose");
  const client = createSettingsWindowClient(api, createRuntime(api)); await client.attach();
  expect(client.getSnapshot()).toMatchObject({ canForce: true, error: "attach denied" });
  await client.forceClose(); expect(close).toHaveBeenCalledWith("", 0, true);
});
test("a hung attach times out and exposes the emergency close", async () => {
  vi.useFakeTimers(); const api = createMockApi();
  vi.spyOn(api, "settingsWindowAttach").mockImplementation(() => new Promise(() => {}));
  const client = createSettingsWindowClient(api, createRuntime(api)); const attached = client.attach();
  await vi.advanceTimersByTimeAsync(5000); await attached;
  expect(client.getSnapshot().canForce).toBe(true);
});
test("push failure delivery offers force close; a clean snapshot clears the error", async () => {
  const api = createMockApi(); let deliver!: (value: SettingsWindowDelivery) => void;
  api.settingsWindowAttach = async fn => { deliver = fn; };
  const client = createSettingsWindowClient(api, createRuntime(api)); await client.attach();
  deliver({ kind: "session", session: "s1" }); deliver({ kind: "snapshot", snapshot: testSnapshot() });
  deliver({ kind: "error", session: "s1", message: "push failed" });
  expect(client.getSnapshot()).toMatchObject({ canForce: true, error: "push failed" });
  deliver({ kind: "snapshot", snapshot: testSnapshot() });
  expect(client.getSnapshot()).toMatchObject({ canForce: false, error: "" });
});
