/** "Start with the system": off, hidden at login (tray, else task bar) or with the window shown. */
export type AutostartMode = "off" | "minimized" | "shown";
/** `autostart` is read from the autostart entry on disk; `startMenuError`/`autostartError` say why reading or refreshing that entry failed; one never hides the other. */
export interface SystemStatus { appImage: boolean; startMenuExists: boolean; startMenuError?: string | null; autostart: AutostartMode; autostartError?: string | null }
export interface AvailableUpdate { version: string; notes: string }
export interface UpdateProgress { downloaded: number; total: number | null; phase: "download" | "install" }
/** Mockable boundary: no network or desktop side effects in the UI/policy tests. */
export interface SystemApi {
  systemStatus(): Promise<SystemStatus>;
  startMenuSet(enabled: boolean): Promise<void>;
  /** Writes or removes the autostart entry; resolves with the mode found on disk afterwards. */
  autostartSet(mode: AutostartMode): Promise<AutostartMode>;
  traySet(enabled: boolean, language: "de" | "en"): Promise<boolean>;
  updateCheck(): Promise<AvailableUpdate | null>;
  updateInstall(version: string, onProgress: (progress: UpdateProgress) => void): Promise<void>;
  updateCancel(): Promise<boolean>;
  restart(): Promise<void>;
  releaseOpen(): Promise<void>;
}
