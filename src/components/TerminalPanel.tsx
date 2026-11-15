import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useAppState, useController, useRuntime, useT } from "../app/context";
import type { DualfileApi, TerminalOutput, TerminalExit } from "../ipc/api";
import { mapKey } from "../state/keymap";
import { isArchivePath, parseRemotePath } from "../state/paths";

export function TerminalPanel({ api, cwd }: { api: DualfileApi; cwd: string }) {
  const host = useRef<HTMLDivElement>(null);
  // Navigation does not restart or cd an already running shell.
  const initialCwd = useRef(cwd);
  const controller = useController();
  const { store } = useRuntime();
  const tr = useT();
  const settingsHeight = useAppState(s => s.settings.terminalHeight);
  const [dragHeight, setDragHeight] = useState<number | null>(null);
  const clamp = (h: number) => Math.max(100, Math.min(h, Math.min(800, Math.round(window.innerHeight * 0.8))));
  const height = clamp(dragHeight ?? settingsHeight ?? 260);
  const persist = (terminalHeight: number) => controller.updateSettings(s => ({ ...s, terminalHeight }));
  const onDown = (e: React.PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY, start = height;
    const move = (ev: PointerEvent) => setDragHeight(clamp(start - (ev.clientY - startY)));
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const final = clamp(start - (ev.clientY - startY));
      setDragHeight(null);
      persist(final);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    persist(clamp(height + (e.key === "ArrowUp" ? 16 : -16)));
  };
  const exitedText = useRef(tr("terminal.exited"));
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    // Steady cursor: the blink animation never runs in the WebKitGTK webview, leaving the cursor stuck invisible.
    const term = new Terminal({ cursorBlink: false, fontSize: 13, fontFamily: '"IBM Plex Mono", monospace' });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(element);
    let disposed = false;
    let exited = false;
    let id: number | undefined;
    // Output (including immediate exit) can arrive before the open command resolves.
    const pending: Array<TerminalOutput | TerminalExit> = [];
    const fail = (error: unknown) => {
      if (!disposed) term.writeln(`\r\n${String(error).replace(/[\x00-\x1f\x7f-\x9f]/g, " ")}`);
    };
    const receive = (event: TerminalOutput | TerminalExit) => {
      if (disposed) return;
      if (id === undefined) { pending.push(event); return; }
      if (event.id !== id || exited) return;
      if ("data" in event) term.write(Uint8Array.from(atob(event.data), char => char.charCodeAt(0)));
      else {
        exited = true;
        term.options.disableStdin = true;
        term.writeln(`\r\n\x1b[2m${exitedText.current}\x1b[0m`);
      }
    };
    const stopOutput = api.onTerminalOutput(receive);
    const stopExit = api.onTerminalExit(receive);
    let inputQueue = Promise.resolve();
    const input = term.onData((data: string) => {
      inputQueue = inputQueue.then(async () => {
        if (!disposed && id !== undefined && !exited) await api.terminalWrite(id, data);
      }).catch(fail);
    });
    term.attachCustomKeyEventHandler((event: KeyboardEvent) => {
      if (mapKey(event, "list", store.getState().settings.keybindings) !== "terminal") return true;
      if (event.type === "keydown" && !event.repeat) controller.command("terminal");
      event.preventDefault();
      return false;
    });
    const resize = () => {
      if (disposed) return;
      fit.fit();
      if (id !== undefined && !exited) void api.terminalResize(id, term.rows, term.cols).catch(fail);
    };
    const theme = () => {
      const colors = getComputedStyle(element);
      term.options.theme = {
        background: colors.getPropertyValue("--panel").trim() || undefined,
        foreground: colors.getPropertyValue("--fg").trim() || undefined,
        cursor: colors.getPropertyValue("--fg").trim() || undefined,
        selectionBackground: colors.getPropertyValue("--selection").trim() || undefined,
      };
    };
    theme();
    const themeObserver = new MutationObserver(theme);
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["style", "class", "data-theme"] });
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    window.addEventListener("resize", resize);
    fit.fit();
    const path = initialCwd.current;
    const local = path.startsWith("/") && !isArchivePath(path) && !parseRemotePath(path) ? path : undefined;
    void api.terminalOpen(local, term.rows, term.cols).then(opened => {
      id = opened;
      if (disposed) { void api.terminalClose(opened).catch(console.error); return; }
      for (const event of pending) receive(event);
      pending.length = 0;
      resize();
      term.focus();
    }).catch(fail);
    return () => {
      disposed = true;
      stopOutput(); stopExit(); input.dispose();
      observer.disconnect(); themeObserver.disconnect();
      window.removeEventListener("resize", resize);
      if (id !== undefined) void api.terminalClose(id).catch(console.error);
      term.dispose();
    };
  }, [api, controller, store]);
  return (
    <section className="terminal-panel" style={{ height, flex: `0 0 ${height}px` }} aria-label={tr("menu.view.terminal")}>
      <div className="terminal-resize" role="separator" aria-orientation="horizontal" tabIndex={0}
        aria-label={tr("terminal.resize")} title={tr("terminal.resize")}
        onPointerDown={onDown} onDoubleClick={() => persist(260)} onKeyDown={onKey} />
      <div className="terminal-host" ref={host} />
    </section>
  );
}
