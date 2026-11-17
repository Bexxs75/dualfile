import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { RuntimeProvider, useAppState } from "../app/context";
import { createRuntime } from "../app/runtime";
import { createMockApi } from "../ipc/mock";
import type { TerminalOutput, TerminalExit } from "../ipc/api";
import { mapKey, REBINDABLE } from "../state/keymap";
import { TerminalPanel } from "./TerminalPanel";

const xterm = vi.hoisted(() => ({
  write: vi.fn(), writeln: vi.fn(), dispose: vi.fn(), fit: vi.fn(),
  input: undefined as ((data: string) => void) | undefined,
  key: undefined as ((event: KeyboardEvent) => boolean) | undefined,
  resize: undefined as (() => void) | undefined,
  last: undefined as { options: { theme?: Record<string, string> } } | undefined,
}));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  rows = 24; cols = 80; options: { theme?: Record<string, string> } = {};
  constructor() { xterm.last = this; }
  loadAddon() {} open() {} focus() {}
  write = xterm.write; writeln = xterm.writeln; dispose = xterm.dispose;
  onData(handler: (data: string) => void) { xterm.input = handler; return { dispose() { xterm.input = undefined; } }; }
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) { xterm.key = handler; }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = xterm.fit; } }));
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { xterm.resize = callback; }
    observe() {} disconnect() {}
  });
});
afterEach(() => vi.unstubAllGlobals());

function setup(cwd = "/tmp", api = createMockApi()) {
  const rt = createRuntime(api);
  const open = vi.spyOn(api, "terminalOpen");
  const write = vi.spyOn(api, "terminalWrite");
  const close = vi.spyOn(api, "terminalClose");
  const resize = vi.spyOn(api, "terminalResize");
  function Harness() {
    const enabled = useAppState(state => state.terminal);
    return <><button onClick={() => rt.controller.command("terminal")}>toggle</button>{enabled && <TerminalPanel api={api} cwd={cwd} />}</>;
  }
  const view = render(<RuntimeProvider runtime={rt}><Harness /></RuntimeProvider>);
  return { api, rt, open, write, close, resize, view };
}

test("toggle opens, forwards UTF-8 input, decodes echoed bytes, resizes and closes", async () => {
  const { open, write, close, resize, view } = setup();
  expect(screen.queryByRole("region", { name: "Terminal" })).toBeNull();
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(open).toHaveBeenCalledWith("/tmp", 24, 80));
  expect(screen.getByRole("region", { name: "Terminal" })).toBeVisible();
  await act(async () => xterm.input?.("echo ä\r"));
  expect(write).toHaveBeenCalledWith(1, "echo ä\r");
  expect(Array.from(xterm.write.mock.calls.at(-1)![0] as Uint8Array)).toEqual(Array.from(new TextEncoder().encode("echo ä\r")));
  act(() => xterm.resize?.());
  expect(resize).toHaveBeenCalledWith(1, 24, 80);
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(close).toHaveBeenCalledWith(1));
  expect(xterm.dispose).toHaveBeenCalledTimes(1);
  view.unmount();
});

test.each(["sftp:one!/tmp", "archive:/tmp/a.zip!/", "webdav:one!/", "plugin:one!/", "relative"])("virtual cwd %s uses home", async cwd => {
  const { open, view } = setup(cwd);
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(open).toHaveBeenCalledWith(undefined, 24, 80));
  view.unmount();
});

test("toggle-off while opening closes the late session", async () => {
  const api = createMockApi();
  let resolve!: (id: number) => void;
  vi.spyOn(api, "terminalOpen").mockReturnValue(new Promise<number>(done => { resolve = done; }));
  const { close, view } = setup("/tmp", api);
  fireEvent.click(screen.getByText("toggle"));
  fireEvent.click(screen.getByText("toggle"));
  await act(async () => resolve(42));
  expect(close).toHaveBeenCalledWith(42);
  view.unmount();
});

test("buffers early binary output and a failed start, filters other IDs and stops input", async () => {
  const api = createMockApi();
  let output!: (event: TerminalOutput) => void;
  let exit!: (event: TerminalExit) => void;
  const unlisten = vi.fn();
  vi.spyOn(api, "onTerminalOutput").mockImplementation(handler => { output = handler; return unlisten; });
  vi.spyOn(api, "onTerminalExit").mockImplementation(handler => { exit = handler; return unlisten; });
  vi.spyOn(api, "terminalOpen").mockImplementation(async () => {
    output({ id: 42, data: btoa("\xff\x00") });
    output({ id: 999, data: btoa("wrong") });
    exit({ id: 42, code: 1 });
    return 42;
  });
  const { write, view } = setup("/tmp", api);
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(xterm.writeln).toHaveBeenCalled());
  expect(xterm.write).toHaveBeenCalledExactlyOnceWith(new Uint8Array([255, 0]));
  expect(xterm.writeln).toHaveBeenCalledWith(expect.stringContaining("(1)"));
  expect(screen.getByRole("region", { name: "Terminal" })).toBeVisible();
  act(() => xterm.input?.("ignored"));
  expect(write).not.toHaveBeenCalled();
  view.unmount();
  expect(unlisten).toHaveBeenCalledTimes(2);
});

test("resize handle drags up to persist a clamped height, resets on double-click, steps with arrows", async () => {
  const { rt, view } = setup();
  fireEvent.click(screen.getByText("toggle"));
  await act(async () => {});
  const handle = screen.getByRole("separator", { name: "Resize terminal" });
  const panel = screen.getByRole("region", { name: "Terminal" });
  expect(panel.style.height).toBe("260px");
  fireEvent.pointerDown(handle, { clientY: 500 });
  fireEvent.pointerMove(window, { clientY: 400 });
  expect(panel.style.height).toBe("360px");
  expect(rt.store.getState().settings.terminalHeight).toBe(260);
  fireEvent.pointerUp(window, { clientY: 400 });
  expect(rt.store.getState().settings.terminalHeight).toBe(360);
  fireEvent.pointerDown(handle, { clientY: 500 });
  fireEvent.pointerUp(window, { clientY: -5000 });
  expect(rt.store.getState().settings.terminalHeight).toBe(Math.min(800, Math.round(window.innerHeight * 0.8)));
  fireEvent.pointerDown(handle, { clientY: 0 });
  fireEvent.pointerUp(window, { clientY: 5000 });
  expect(rt.store.getState().settings.terminalHeight).toBe(100);
  fireEvent.doubleClick(handle);
  expect(rt.store.getState().settings.terminalHeight).toBe(260);
  fireEvent.keyDown(handle, { key: "ArrowUp" });
  expect(rt.store.getState().settings.terminalHeight).toBe(276);
  fireEvent.keyDown(handle, { key: "ArrowDown" });
  fireEvent.keyDown(handle, { key: "ArrowDown" });
  expect(rt.store.getState().settings.terminalHeight).toBe(244);
  view.unmount();
});

test("shortcut is unique and can close the focused terminal", async () => {
  expect(mapKey({ key: "`", ctrlKey: true })).toBe("terminal");
  expect(Object.values(REBINDABLE).filter(spec => spec === "Ctrl+`")).toHaveLength(1);
  const { close, view } = setup();
  fireEvent.click(screen.getByText("toggle"));
  await act(async () => {});
  act(() => { expect(xterm.key?.(new KeyboardEvent("keydown", { key: "`", ctrlKey: true }))).toBe(false); });
  expect(close).toHaveBeenCalledWith(1);
  view.unmount();
});

test("terminal colors come from the setting and change live for open terminals", async () => {
  const { rt } = setup();
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(xterm.last).toBeDefined());
  expect(xterm.last!.options.theme).toMatchObject({ background: "#0a120a", foreground: "#4af626", cursor: "#4af626" });
  act(() => rt.controller.updateSettings(s => ({ ...s, terminalColors: "light" })));
  expect(xterm.last!.options.theme).toMatchObject({ background: "#fbfaf6", foreground: "#1f2328", green: "#1a7f37" });
  act(() => rt.controller.updateSettings(s => ({ ...s, terminalColors: "theme", theme: "papier" })));
  expect(xterm.last!.options.theme).toMatchObject({ background: "#f6f3ec", foreground: "#2a2824" });
});

async function openAndExit(code: number | null, id = 1, afterMs = 0) {
  vi.useFakeTimers({ toFake: ["Date"] });
  const api = createMockApi();
  let exit!: (event: TerminalExit) => void;
  vi.spyOn(api, "onTerminalExit").mockImplementation(handler => { exit = handler; return () => {}; });
  const { view } = setup("/tmp", api);
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(xterm.last).toBeDefined());
  await act(async () => {});
  vi.setSystemTime(Date.now() + afterMs);
  act(() => exit({ id, code }));
  vi.useRealTimers();
  return view;
}

test("shell exit after the start window closes the panel, whatever the code", async () => {
  const view = await openAndExit(130, 1, 2500);
  expect(screen.queryByRole("region", { name: "Terminal" })).toBeNull();
  view.unmount();
});

test("clean shell exit right away closes the panel", async () => {
  const view = await openAndExit(0);
  expect(screen.queryByRole("region", { name: "Terminal" })).toBeNull();
  view.unmount();
});

test("non-zero exit within 2 s keeps the message readable", async () => {
  const view = await openAndExit(127, 1, 1500);
  expect(screen.getByRole("region", { name: "Terminal" })).toBeVisible();
  expect(xterm.writeln).toHaveBeenCalledWith(expect.stringContaining("(127)"));
  view.unmount();
});

test("exit of another terminal is ignored", async () => {
  const view = await openAndExit(0, 99, 5000);
  expect(screen.getByRole("region", { name: "Terminal" })).toBeVisible();
  view.unmount();
});
