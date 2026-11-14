export interface SystemStatus { appImage: boolean; startMenuExists: boolean }
export interface AvailableUpdate { version: string; notes: string }
export interface UpdateProgress { downloaded: number; total: number | null; phase: "download" | "install" }
/** Mockable boundary: no network or desktop side effects in the UI/policy tests. */
export interface SystemApi {
  systemStatus(): Promise<SystemStatus>;
  startMenuSet(enabled: boolean): Promise<void>;
  traySet(enabled: boolean, language: "de" | "en"): Promise<boolean>;
  updateCheck(): Promise<AvailableUpdate | null>;
  updateInstall(version: string, onProgress: (progress: UpdateProgress) => void): Promise<void>;
  updateCancel(): Promise<boolean>;
  restart(): Promise<void>;
  releaseOpen(): Promise<void>;
}
