import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useAppState, useController, useRuntime, useT } from "../app/context";
import type { DualfileApi, TerminalOutput, TerminalExit } from "../ipc/api";
import { mapKey } from "../state/keymap";
import { terminalColorsFor, xtermTheme } from "../themes/terminal";
import { isArchivePath, parseRemotePath } from "../state/paths";

const START_FAILURE_MS = 2000;

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
    // Steady block cursor, visible even when the terminal is not focused. It only renders at all
    // because the production CSP allows xterm's runtime <style> (style-src 'unsafe-inline' in
    // tauri.conf.json); without that WebKitGTK refuses the injected sheet and the cursor vanishes.
    const term = new Terminal({
      cursorBlink: false,
      cursorStyle: "block",
      cursorInactiveStyle: "block",
      fontSize: 13,
      fontFamily: '"IBM Plex Mono", monospace',
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(element);
    let disposed = false;
    let exited = false;
    const openedAt = Date.now();
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
        // The shell ending (Ctrl+D, `exit`) closes the panel. A failed start (wrong shell, broken rc file)
        // exits non-zero right away and stays readable; the code alone cannot tell, as bash exits with the last status on Ctrl+D.
        if (event.code === 0 || Date.now() - openedAt > START_FAILURE_MS) { controller.command("terminal"); return; }
        term.options.disableStdin = true;
        term.writeln(`\r\n\x1b[2m${exitedText.current}${event.code === null ? "" : ` (${event.code})`}\x1b[0m`);
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
    const theme = () => { term.options.theme = xtermTheme(terminalColorsFor(store.getState().settings)); };
    theme();
    // Only the colour fields matter; a plain store listener would reapply the theme on every state change.
    let lastColors = JSON.stringify(terminalColorsFor(store.getState().settings));
    const stopTheme = store.subscribe(() => {
      const next = JSON.stringify(terminalColorsFor(store.getState().settings));
      if (next !== lastColors) { lastColors = next; theme(); }
    });
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
      observer.disconnect(); stopTheme();
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
