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
}));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  rows = 24; cols = 80; options = {};
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

test("buffers early binary output and exit, filters other IDs and stops input", async () => {
  const api = createMockApi();
  let output!: (event: TerminalOutput) => void;
  let exit!: (event: TerminalExit) => void;
  const unlisten = vi.fn();
  vi.spyOn(api, "onTerminalOutput").mockImplementation(handler => { output = handler; return unlisten; });
  vi.spyOn(api, "onTerminalExit").mockImplementation(handler => { exit = handler; return unlisten; });
  vi.spyOn(api, "terminalOpen").mockImplementation(async () => {
    output({ id: 42, data: btoa("\xff\x00") });
    output({ id: 999, data: btoa("wrong") });
    exit({ id: 42, code: 0 });
    return 42;
  });
  const { write, view } = setup("/tmp", api);
  fireEvent.click(screen.getByText("toggle"));
  await waitFor(() => expect(xterm.writeln).toHaveBeenCalled());
  expect(xterm.write).toHaveBeenCalledExactlyOnceWith(new Uint8Array([255, 0]));
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
