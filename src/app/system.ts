import type { SystemApi, SystemStatus, AvailableUpdate, UpdateProgress, AutostartMode } from "../ipc/system";
import type { Settings } from "../ipc/types";

export interface SystemState {
  status: SystemStatus | null;
  update: AvailableUpdate | null;
  check: "idle" | "checking" | "current" | "available" | "error";
  error: string;
  progress: UpdateProgress | null;
  installing: boolean;
  installed: boolean;
  prompt: boolean;
  /** The one-time "start with the system?" question is due (shown after the start-menu question). */
  autostartPrompt: boolean;
  trayUnavailable: boolean;
}
export function shouldPrompt(settings: Settings): boolean {
  return !settings.startMenuPrompted && (settings.startMenuPromptCount ?? 0) < 3;
}
/** SemVer precedence, including prereleases; build metadata does not affect precedence. */
export function isNewerVersion(version: string, previous: string): boolean {
  const parse = (value: string) => /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(value);
  const a = parse(version), b = parse(previous);
  if (!a) return false;
  if (!b) return true;
  for (let i = 1; i <= 3; i++) {
    const x = BigInt(a[i]!), y = BigInt(b[i]!);
    if (x !== y) return x > y;
  }
  if (!a[4] || !b[4]) return !!b[4] && !a[4];
  const ap = a[4].split("."), bp = b[4].split(".");
  for (let i = 0; i < Math.max(ap.length, bp.length); i++) {
    const x = ap[i], y = bp[i];
    if (x === undefined || y === undefined) return y === undefined;
    if (x === y) continue;
    const xn = /^\d+$/.test(x), yn = /^\d+$/.test(y);
    if (xn && yn) return BigInt(x) > BigInt(y);
    if (xn !== yn) return !xn;
    return x > y;
  }
  return false;
}
/** One controller per runtime, so remounting a dialog cannot repeat startup work. */
export function createSystemController(api: SystemApi, getSettings: () => Settings,
  patch: (value: Partial<Settings>) => void, jobsActive: () => boolean,
  save: () => Promise<void>, now = () => new Date().toISOString(), beforeInstall: () => Promise<void> = async () => {}) {
  let state: SystemState = { status: null, update: null, check: "idle", error: "", progress: null,
    installing: false, installed: false, prompt: false, autostartPrompt: false, trayUnavailable: false };
  const listeners = new Set<() => void>();
  let started = false;
  let cancelRequested = false;
  let nativeInstall = false;
  let checking: Promise<void> | null = null;
  let traySequence: Promise<void> = Promise.resolve();
  // Status reads and autostart writes run one after another, so a slow older answer can
  // never overwrite what a newer choice put on disk.
  let statusSequence: Promise<unknown> = Promise.resolve();
  function queued<T>(task: () => Promise<T>): Promise<T> {
    const result = statusSequence.then(task);
    statusSequence = result.catch(() => {});
    return result;
  }
  async function reload(): Promise<SystemStatus> {
    const status = await api.systemStatus();
    set({ status });
    return status;
  }
  /** Shows what is on disk afterwards, also when the write failed half-way. */
  async function writeAutostart(mode: AutostartMode): Promise<void> {
    try {
      const autostart = await api.autostartSet(mode);
      if (state.status) set({ status: { ...state.status, autostart, autostartError: null } });
      else await reload().catch(() => {});
    } catch (error) {
      await reload().catch(() => {});
      throw error;
    }
  }
  const set = (next: Partial<SystemState>) => { state = { ...state, ...next }; listeners.forEach(fn => fn()); };
  const errorText = (e: unknown) => e instanceof Error ? e.message : String(e);
  async function check(manual = false): Promise<void> {
    try {
      if ((state.status ?? await queued(reload)).packaging === "flatpak") return;
    } catch (e) { set({ check: "error", error: errorText(e) }); return; }
    if (checking || state.installing) return checking ?? Promise.resolve();
    set({ check: "checking", error: "" });
    checking = (async () => {
      try {
        const update = await api.updateCheck();
        const dismissed = getSettings().dismissedUpdateVersion;
        set({ check: update ? "available" : "current", update: update && (manual || isNewerVersion(update.version, dismissed ?? "")) ? update : null });
      } catch (error) {
        // Background errors are retained for Settings, never raised as a dialog/toast.
        console.warn("Dualfile: update check failed");
        set({ check: "error", error: errorText(error) });
      } finally { patch({ lastUpdateCheck: now() }); }
    })();
    await checking;
    checking = null;
  }
  function setTray(enabled: boolean, language: "de" | "en"): Promise<void> {
    // Preserve click order when native requests resolve at different speeds.
    traySequence = traySequence.then(async () => {
      try { if (!await api.traySet(enabled, language)) set({ trayUnavailable: true }); }
      catch { set({ trayUnavailable: true }); }
    });
    return traySequence;
  }
  return {
    getSnapshot: () => state,
    subscribe: (fn: () => void) => { listeners.add(fn); return () => { listeners.delete(fn); }; },
    async start(language: "de" | "en") {
      if (started) return;
      started = true;
      const settings = getSettings();
      void setTray(settings.trayEnabled ?? false, language);
      try {
        const status = await queued(reload);
        if (settings.checkUpdatesOnStart !== false) void check();
        if (status.packaging !== "flatpak" && !status.startMenuExists && !status.startMenuError && shouldPrompt(getSettings())) {
          const count = Math.min(3, (getSettings().startMenuPromptCount ?? 0) + 1);
          patch({ startMenuPromptCount: count });
          await save();
          set({ prompt: true });
        }
        // Never answered: ask once. An entry that already exists is an answer.
        if (!getSettings().autostartPrompted) {
          if (status.autostart === "off") set({ autostartPrompt: true });
          else { patch({ autostartPrompted: true }); await save(); }
        }
      } catch (e) { set({ error: errorText(e) }); }
    },
    check,
    setTray,
    dismiss() {
      if (state.update) patch({ dismissedUpdateVersion: state.update.version });
      set({ update: null });
    },
    async answerPrompt(install: boolean, never: boolean) {
      if ((state.status ?? await queued(reload)).packaging === "flatpak") return;
      if (install) {
        await api.startMenuSet(true);
        set({ status: state.status ? { ...state.status, startMenuExists: true } : null });
      }
      patch({ startMenuPrompted: install || never || (getSettings().startMenuPromptCount ?? 0) >= 3 });
      await save();
      set({ prompt: false });
    },
    async setStartMenu(enabled: boolean) {
      if ((state.status ?? await queued(reload)).packaging === "flatpak") return;
      await api.startMenuSet(enabled);
      set({ status: state.status ? { ...state.status, startMenuExists: enabled } : null });
      if (enabled) { patch({ startMenuPrompted: true }); await save(); }
    },
    /** Reads the entries from disk again (settings opened or focused): they may have changed outside Dualfile. */
    refreshStatus(): Promise<void> {
      return queued(async () => { try { await reload(); } catch (e) { set({ error: errorText(e) }); } });
    },
    /** Applies the choice; the shown mode is what the autostart entry on disk says afterwards. */
    async setAutostart(mode: AutostartMode) {
      await queued(() => writeAutostart(mode));
      patch({ autostartPrompted: true });
      await save();
    },
    /** "off" is also the answer of "Not now" and Escape; the question does not return. */
    async answerAutostart(mode: AutostartMode) {
      // "Not now" always ends with Off, whatever the cached status says: removing is idempotent
      // and only ever touches our own entry.
      await queued(() => writeAutostart(mode));
      patch({ autostartPrompted: true });
      // The entry on disk already matches the answer. If the answer cannot be saved, the
      // question may return at the next start; report that instead of holding the dialog open.
      try { await save(); set({ autostartPrompt: false }); }
      catch (e) { set({ autostartPrompt: false, error: errorText(e) }); }
    },
    async install() {
      if (jobsActive() || state.installing || !state.update || !state.status || state.status.packaging === "flatpak") return;
      if (!state.status.appImage) { await api.releaseOpen(); return; }
      cancelRequested = false;
      set({ installing: true, error: "", progress: { downloaded: 0, total: null, phase: "download" } });
      try {
        await save();
        if (jobsActive() || cancelRequested) { set({ installing: false, progress: null }); return; }
        await beforeInstall();
        if (jobsActive() || cancelRequested) { set({ installing: false, progress: null }); return; }
        nativeInstall = true;
        await api.updateInstall(state.update.version, progress => set({ progress }));
        set({ installed: true });
        await api.restart();
      } catch (error) {
        set({ error: errorText(error) === "Update cancelled" ? "" : errorText(error), installing: state.installed, progress: state.installed ? state.progress : null });
      } finally { nativeInstall = false; }
    },
    async cancel() {
      if (!state.installing || state.installed || state.progress?.phase === "install") return;
      if (!nativeInstall) cancelRequested = true;
      else await api.updateCancel();
    },
    restart: () => api.restart(),
  };
}
export type SystemController = ReturnType<typeof createSystemController>;
