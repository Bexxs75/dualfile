import type { DualfileApi } from "../ipc/api";
import type { Settings, SettingsBase, SettingsWindowBaseDelta, SettingsWindowMessage, SettingsWindowPatch, SettingsWindowSnapshot, WindowSize } from "../ipc/types";
import type { AppRuntime } from "./runtime";
import type { AppStore, Controller } from "./controller";
import type { SystemController } from "./system";
import type { AutostartMode } from "../ipc/system";
import { activeTabOf, defaultSettings, type AppState } from "./state";
import { isArchivePath, parseRemotePath } from "../state/paths";
import { createTranslator } from "../i18n";
import { answerPatch, decideSize, sizeDiffers, type SizeAnswer } from "./settingsWindowSize";

export const PROJECTED_KEYS = ["sidebarMode", "sidebarWidth", "sidebarCollapsed", "sidebarHidden", "sidebarDiscovery", "settingsWindowSizeMode", "settingsWindowSize", "theme", "accent", "accentAutomatic", "density", "fontSize", "fontWeight", "language", "showParentEntry", "paneTintStrength", "showPermissionsColumn", "showUidColumn", "showGidColumn", "extensionColumn", "paneColorLeft", "paneColorRight", "terminalColors", "terminalBg", "terminalFg", "verifyPolicy", "terminalShell", "trayEnabled", "checkUpdatesOnStart", "lastUpdateCheck", "keybindings"] as const;
export const PATCH_KEYS = PROJECTED_KEYS.filter(key => key !== "lastUpdateCheck");
export function project(settings: Settings): SettingsWindowSnapshot["settings"] {
  return Object.fromEntries(PROJECTED_KEYS.filter(key => settings[key] !== undefined).map(key => [key, settings[key]])) as SettingsWindowSnapshot["settings"];
}
/**
 * The start-state baseline travels as a difference to the current settings: only the small fields
 * that differ. Hidden sidebar entries are not part of "modified" and never travel twice, so a snapshot
 * stays within the native relay's 256 KiB limit.
 */
export function baseDelta(current: Settings, base: SettingsBase): SettingsWindowBaseDelta {
  const changed = Object.fromEntries(PATCH_KEYS.filter(key => key !== "sidebarHidden")
    .map(key => [key, base.settings[key] ?? defaultSettings[key]] as const)
    .filter(([key, value]) => JSON.stringify(value) !== JSON.stringify(current[key] ?? defaultSettings[key])));
  return { changed, autostart: base.autostart };
}
/** Rebuilds the baseline from the current settings and a `baseDelta`. */
export function applyBaseDelta(current: Settings, delta: SettingsWindowBaseDelta): SettingsBase {
  return { settings: { ...current, ...delta.changed }, autostart: delta.autostart };
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
      base: state.settingsBase ? baseDelta(state.settings, state.settingsBase) : null,
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
/** The question "keep this window size?" while closing; `answer` settles the close. */
export interface SizePrompt { width: number; height: number; direction: "larger" | "smaller"; answer(answer: SizeAnswer): void }
export function createSettingsWindowClient(api: DualfileApi, runtime: AppRuntime) {
  let session = "", seq = 0, queue = Promise.resolve();
  let snapshot: SettingsWindowSnapshot | null = null;
  let state: { snapshot: SettingsWindowSnapshot | null; error: string; closing: boolean; canForce: boolean; sizePrompt: SizePrompt | null } = { snapshot, error: "", closing: false, canForce: false, sizePrompt: null };
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
  // The size the user last answered for; a retried close does not ask again for the same size.
  let handledSize: WindowSize | undefined;
  // The size of the window as it opened: the reference for "was it resized". It is measured in the
  // web view (`innerWidth`/`innerHeight`: the client area in logical pixels on every platform) and
  // not taken from the native side, whose size is not the size the window was given: GTK3 on
  // GNOME/Wayland reports the window with its client-side decoration and shadow (a window created
  // as 720 x 640 reports 772 x 729), and `Resized` and `inner_size()` of the toolkit both carry
  // that value. The web view's own size is the same quantity that is passed to the next opening.
  // Layout changes while the window settles (first configure) move the reference, later ones are the user's.
  const viewport = (): WindowSize => ({ width: Math.round(window.innerWidth), height: Math.round(window.innerHeight) });
  const settleUntil = Date.now() + 1500;
  let baseline = viewport();
  window.addEventListener("resize", () => { if (Date.now() < settleUntil) baseline = viewport(); });
  /**
   * Closing decides what happens to the window size first (see `decideSize`). The question waits
   * for the user without the close deadline; the native close timeout is held meanwhile. A
   * maximized window or an unmeasurable size is left alone.
   */
  async function sizeStep(interactive: boolean): Promise<SettingsWindowPatch | null> {
    const mode = snapshot?.settings.settingsWindowSizeMode ?? "ask";
    if (!snapshot || mode === "never") return null;
    try { if (await api.settingsWindowZoomed()) return null; } catch { return null; }
    const current = viewport();
    if (current.width <= 0 || current.height <= 0) return null;
    if (handledSize && !sizeDiffers(current, handledSize)) return null;
    const decision = decideSize(mode, current, baseline);
    if (decision.kind === "none") return null;
    if (decision.kind === "save") { handledSize = current; return { settingsWindowSize: decision.size }; }
    // The main window closing or restarting waits only a few seconds: no question then, nothing changes.
    if (!interactive) return null;
    const heldSession = session;
    void api.settingsWindowHold(heldSession, true).catch(() => {});
    const answer = await new Promise<SizeAnswer>(resolve => {
      state.sizePrompt = { width: decision.size.width, height: decision.size.height, direction: decision.direction, answer: resolve };
      notify();
    });
    state.sizePrompt = null; notify();
    handledSize = current;
    void api.settingsWindowHold(heldSession, false).catch(() => {});
    return answerPatch(answer, decision.size);
  }
  function close(interactive = true): Promise<void> {
    if (closePromise) return closePromise;
    const attempt = ++closeAttempt;
    // With a size to look at, "closing" starts after the question, so the window stays usable under it.
    const wantsSize = !!snapshot && (snapshot.settings.settingsWindowSizeMode ?? "ask") !== "never";
    if (!wantsSize) { state.closing = true; notify(); }
    closePromise = (async () => {
      try {
        const sizePatch = wantsSize ? await sizeStep(interactive) : null;
        if (attempt !== closeAttempt) return;
        if (wantsSize) { state.closing = true; notify(); }
        await deadline((async () => {
          if (!snapshot) await new Promise<void>(resolve => { readyForClose = resolve; });
          if (attempt !== closeAttempt) return;
          // Queued ahead of the flush: the main window applies messages in order, so the flush also
          // persists the size; a refusal comes back as an error of this message.
          if (sizePatch) void send({ kind: "patch", patch: sizePatch }).catch(() => {});
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
        if (delivery.kind === "close") { if (delivery.session === session) void close(delivery.interactive !== false); return; }
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
        if (next.base) runtime.store.dispatch({ type: "settingsBase", base: applyBaseDelta({ ...defaultSettings, ...next.settings }, next.base) });
        rebase(); notify();
      }), timeoutText());
      if (!snapshot) initialSnapshotTimer = setTimeout(() => failed(timeoutText()), CLOSE_TIMEOUT);
    } catch (error) { failed(error); }
  }
  return { runtime: { ...runtime, controller, system }, subscribe, getSnapshot: () => state, close: () => close(), forceClose,
    attach: () => attachment ??= attach(),
  };
}
