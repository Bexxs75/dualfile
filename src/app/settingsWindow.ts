import type { DualfileApi } from "../ipc/api";
import type { Settings, SettingsWindowMessage, SettingsWindowPatch, SettingsWindowSnapshot } from "../ipc/types";
import type { AppRuntime } from "./runtime";
import type { AppStore, Controller } from "./controller";
import type { SystemController } from "./system";
import type { AutostartMode } from "../ipc/system";
import { activeTabOf, defaultSettings, type AppState } from "./state";
import { isArchivePath, parseRemotePath } from "../state/paths";
import { createTranslator } from "../i18n";

export const PROJECTED_KEYS = ["sidebarMode", "sidebarWidth", "sidebarCollapsed", "sidebarHidden", "sidebarDiscovery", "theme", "accent", "accentAutomatic", "density", "fontSize", "language", "showParentEntry", "extensionColumn", "paneColorLeft", "paneColorRight", "terminalColors", "terminalBg", "terminalFg", "verifyPolicy", "terminalShell", "trayEnabled", "checkUpdatesOnStart", "lastUpdateCheck", "keybindings"] as const;
export const PATCH_KEYS = PROJECTED_KEYS.filter(key => key !== "lastUpdateCheck");
export function project(settings: Settings): SettingsWindowSnapshot["settings"] {
  return Object.fromEntries(PROJECTED_KEYS.filter(key => settings[key] !== undefined).map(key => [key, settings[key]])) as SettingsWindowSnapshot["settings"];
}
export function logSaveDir(state: AppState): string {
  const path = activeTabOf(state, state.active).path;
  return parseRemotePath(path) || isArchivePath(path) || !path.startsWith("/") ? state.home : path;
}
const CLOSE_TIMEOUT = 5000;
function deadline<T>(promise: Promise<T>, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  const timeout = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(message)), CLOSE_TIMEOUT); });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}
const delay = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function createSettingsWindowBridge(api: DualfileApi, store: AppStore, controller: Controller, system: SystemController) {
  let session = "", ack = 0, errors: SettingsWindowSnapshot["errors"] = [];
  let messages = Promise.resolve(), pushes = Promise.resolve(), previous = "", active = false;
  let latest: SettingsWindowSnapshot | undefined;
  const closeWaiters = new Set<{ resolve: () => void; reject: (error: Error) => void }>();
  const drainError = () => new Error(createTranslator(controller.language())("settings.drainFailed"));
  function failDrain() { closeWaiters.forEach(waiter => waiter.reject(drainError())); }
  async function pushWithRetry(snapshot: SettingsWindowSnapshot, title: string) {
    for (const backoff of [0, 200, 500, 1000]) {
      if (backoff) await delay(backoff);
      if (!active) return false;
      try { await api.settingsWindowPush(snapshot, title); return true; }
      catch (error) {
        if (backoff !== 1000) continue;
        previous = ""; failDrain();
        // This control delivery bypasses the failed snapshot application. If IPC itself is
        // unavailable, the client's independent acknowledgement watchdog offers the exit.
        await api.settingsWindowPush(snapshot, title, false, { error: errorText(error), cancelClose: true }).catch(() => {});
      }
    }
    return false;
  }
  function push() {
    const state = store.getState();
    if (!active || state.phase !== "ready") return;
    const { status, check, error, installing, trayUnavailable } = system.getSnapshot();
    const snapshot: SettingsWindowSnapshot = { session, ack, errors: [...errors], settings: project(state.settings),
      system: { status, check, error, installing, trayUnavailable }, logDir: logSaveDir(state),
      persistence: { outcome: state.outcome, error: state.saveError } };
    latest = snapshot;
    const text = JSON.stringify(snapshot);
    if (text === previous) return;
    previous = text;
    const title = createTranslator(controller.language())("settings.title");
    pushes = pushes.then(async () => {
      if (active) {
        const delivered = await pushWithRetry(snapshot, title);
        if (delivered && session === snapshot.session) errors = errors.filter(error => !snapshot.errors.includes(error));
      }
    }).catch(() => { previous = ""; });
  }
  function receive(message: SettingsWindowMessage) {
    if (message.kind === "closed") { if (message.session !== session) return; closeWaiters.forEach(waiter => waiter.resolve()); closeWaiters.clear(); return; }
    if (message.kind === "closeFailed") { if (message.session === session) failDrain(); return; }
    // Invalidate an old in-flight action immediately when a new window attaches.
    if (message.kind === "session") { if (message.session === session) return; session = message.session; ack = 0; errors = []; push(); return; }
    messages = messages.then(async () => {
      if (!active || message.session !== session || message.seq <= ack) return;
      try {
        switch (message.kind) {
          case "patch": {
            const patch = Object.fromEntries(Object.entries(message.patch).filter(([key]) => (PATCH_KEYS as readonly string[]).includes(key)));
            controller.updateSettings(s => ({ ...s, ...patch })); break;
          }
          case "checkUpdates": void system.check(true); break;
          case "setTray": await system.setTray(message.enabled, controller.language()); break;
          case "setStartMenu": await system.setStartMenu(message.enabled); break;
          case "refreshStatus": await system.refreshStatus(); break;
          case "setAutostart": await system.setAutostart(message.mode); break;
          // Closing is a persistence barrier as well as an applied-message barrier.
          case "flush":
            if (store.getState().outcome.kind !== "newerVersion") await controller.saveNow(true);
            break;
        }
      } catch (error) {
        if (message.session === session) { errors.push({ seq: message.seq, message: errorText(error) }); if (message.kind === "flush") failDrain(); }
      }
      if (message.session !== session) return;
      ack = message.seq;
      push();
    });
  }
  return { async flush() {
    if (!active || !latest) return;
    push();
    let waiter!: { resolve: () => void; reject: (error: Error) => void };
    const closed = new Promise<void>((resolve, reject) => { waiter = { resolve, reject }; closeWaiters.add(waiter); });
    // Attach a rejection handler before waiting on the serialized push queue.
    const completion = deadline(closed, drainError().message);
    let cancelled = false;
    try {
      const request = pushes.then(async () => {
        if (cancelled) return;
        const exists = await api.settingsWindowPush(latest!, createTranslator(controller.language())("settings.title"), true);
        if (!exists) waiter.resolve();
      });
      pushes = request.catch(() => { previous = ""; });
      void request.catch(() => waiter.reject(drainError()));
      await completion;
    } catch {
      cancelled = true;
      // Out of band so a hung ordinary push cannot retain an old parent-close intent.
      void api.settingsWindowPush(latest!, createTranslator(controller.language())("settings.title"), false, { cancelClose: true }).catch(() => {});
      throw drainError();
    } finally { closeWaiters.delete(waiter); }
  }, start() {
    active = true;
    const stops = [api.onSettingsWindowMessage(receive), store.subscribe(push), system.subscribe(push)];
    push();
    return () => { active = false; stops.forEach(stop => stop()); };
  } };
}

type Action = { kind: "patch"; patch: SettingsWindowPatch } | { kind: "setTray" | "setStartMenu"; enabled: boolean } | { kind: "setAutostart"; mode: AutostartMode } | { kind: "checkUpdates" | "flush" | "refreshStatus" };
export function createSettingsWindowClient(api: DualfileApi, runtime: AppRuntime) {
  let session = "", seq = 0, queue = Promise.resolve();
  let snapshot: SettingsWindowSnapshot | null = null;
  let state: { snapshot: SettingsWindowSnapshot | null; error: string; closing: boolean; canForce: boolean } = { snapshot, error: "", closing: false, canForce: false };
  let systemState = runtime.system.getSnapshot();
  const listeners = new Set<() => void>();
  const pending = new Map<number, { message: SettingsWindowMessage; resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  const notify = () => { state = { ...state, snapshot }; listeners.forEach(fn => fn()); };
  const subscribe = (fn: () => void) => { listeners.add(fn); return () => { listeners.delete(fn); }; };
  const timeoutText = () => createTranslator(runtime.controller.language())("settings.ackTimeout");
  function failed(error: unknown) {
    state.error = errorText(error); state.canForce = true; notify();
  }
  function rebase() {
    if (!snapshot) return;
    let settings = { ...defaultSettings, ...snapshot.settings };
    for (const { message } of pending.values()) if (message.kind === "patch") settings = { ...settings, ...message.patch };
    runtime.store.dispatch({ type: "settings", update: () => settings });
  }
  function send(action: Action): Promise<void> {
    if (!session || !snapshot) return Promise.reject(new Error("Settings are still loading"));
    const message = { ...action, session, seq: ++seq } as SettingsWindowMessage;
    const result = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => { const error = new Error(timeoutText()); failed(error); reject(error); }, CLOSE_TIMEOUT);
      pending.set(message.seq, { message, resolve, reject, timer });
    });
    queue = queue.then(async () => {
      try { await api.settingsWindowSend(message); }
      catch (error) {
        const entry = pending.get(message.seq);
        clearTimeout(entry?.timer);
        entry?.reject(new Error(errorText(error)));
        pending.delete(message.seq);
        failed(error); rebase(); notify();
      }
    });
    return result;
  }
  let closePromise: Promise<void> | null = null;
  let readyForClose: (() => void) | undefined;
  let closeAttempt = 0;
  function abandon(error: unknown) {
    closeAttempt++; state.closing = false; failed(error);
    void api.settingsWindowClose(session, seq, false, true).catch(() => {});
  }
  function close(): Promise<void> {
    if (closePromise) return closePromise;
    const attempt = ++closeAttempt;
    state.closing = true; notify();
    closePromise = (async () => {
      try {
        await deadline((async () => {
          if (!snapshot) await new Promise<void>(resolve => { readyForClose = resolve; });
          if (attempt !== closeAttempt) return;
          await send({ kind: "flush" });
          // The native close invocation can also stall after the flush was acknowledged.
          // Keep the emergency exit available for the entire close handshake.
          if (attempt === closeAttempt) await api.settingsWindowClose(session, seq);
        })(), timeoutText());
      } catch (error) { if (attempt === closeAttempt) abandon(error); }
      finally { closePromise = null; }
    })();
    return closePromise;
  }
  async function forceClose() {
    closeAttempt++;
    try { await api.settingsWindowClose(session, seq, true); }
    catch (error) { state.closing = false; failed(error); }
  }
  const controller: Controller = { ...runtime.controller, updateSettings(update) {
    if (state.closing || !snapshot) return;
    const before = runtime.store.getState().settings;
    runtime.controller.updateSettings(update);
    const after = runtime.store.getState().settings;
    const patch = Object.fromEntries(PATCH_KEYS.filter(key => JSON.stringify(before[key]) !== JSON.stringify(after[key])).map(key => [key, after[key]]));
    if (Object.keys(patch).length) void send({ kind: "patch", patch }).catch(() => {});
  } };
  const system: SystemController = { ...runtime.system, getSnapshot: () => systemState, subscribe,
    start: async () => {}, check: () => send({ kind: "checkUpdates" }).catch(() => {}),
    setTray: enabled => send({ kind: "setTray", enabled }).catch(() => {}),
    setStartMenu: enabled => send({ kind: "setStartMenu", enabled }),
    setAutostart: mode => send({ kind: "setAutostart", mode }),
    refreshStatus: () => send({ kind: "refreshStatus" }).catch(() => {}),
    dismiss() {}, answerPrompt: async () => {}, answerAutostart: async () => {}, install: async () => {}, cancel: async () => {}, restart: async () => {},
  };
  let attachment: Promise<void> | undefined;
  let initialSnapshotTimer: ReturnType<typeof setTimeout> | undefined;
  async function attach() {
    try {
      await deadline(api.settingsWindowAttach(delivery => {
        if (delivery.kind === "session") {
          if (session && session !== delivery.session) return;
          session = delivery.session; return;
        }
        if (delivery.kind === "close") { if (delivery.session === session) void close(); return; }
        if (delivery.kind === "timeout" || delivery.kind === "error") {
          if (delivery.session === session) abandon(delivery.kind === "timeout" ? timeoutText() : delivery.message);
          return;
        }
        if (delivery.kind !== "snapshot") return;
        const next = delivery.snapshot;
        if (next.session !== session || snapshot && next.ack < snapshot.ack) return;
        clearTimeout(initialSnapshotTimer);
        snapshot = next;
        state.error = next.errors.at(-1)?.message ?? "";
        if (next.errors.length) state.canForce = true;
        readyForClose?.(); readyForClose = undefined;
        for (const [id, entry] of pending) if (id <= next.ack) {
          clearTimeout(entry.timer);
          const error = next.errors.find(error => error.seq === id);
          if (error) { state.canForce = true; state.error = error.message; entry.reject(new Error(error.message)); } else entry.resolve();
          pending.delete(id);
        }
        if (!next.errors.length && !next.persistence.error && !pending.size) state.canForce = false;
        systemState = { ...systemState, ...next.system };
        rebase(); notify();
      }), timeoutText());
      if (!snapshot) initialSnapshotTimer = setTimeout(() => failed(timeoutText()), CLOSE_TIMEOUT);
    } catch (error) { failed(error); }
  }
  return { runtime: { ...runtime, controller, system }, subscribe, getSnapshot: () => state, close, forceClose,
    attach: () => attachment ??= attach(),
  };
}
