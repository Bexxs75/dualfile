import { createSettingsWindowBridge } from "./settingsWindow";
import type { DualfileApi } from "../ipc/api";
import { createController, type Controller, type ControllerOptions, type AppStore } from "./controller";
import { createStore } from "./store";
import { initialState, reducer } from "./state";

import { createSystemController, type SystemController } from "./system";
import { isFinished } from "../state/operations";

export interface AppRuntime {
  api: DualfileApi;
  store: AppStore;
  controller: Controller;
  system: SystemController;
  settingsWindow: ReturnType<typeof createSettingsWindowBridge>;
}

/** Baut Speicher und Controller um eine API. Die Oberfläche startet den Controller (`controller.start()`). */
export function createRuntime(api: DualfileApi, options: ControllerOptions = {}): AppRuntime {
  const store = createStore(reducer, initialState());
  const controller = createController(api, store, options);
  const system = createSystemController(api, () => store.getState().settings,
    patch => controller.updateSettings(settings => ({ ...settings, ...patch })),
    () => store.getState().jobs.jobs.some(job => !isFinished(job.state)),
    () => controller.saveNow(true), undefined, () => settingsWindow.flush());
  // Baseline for "modified": the autostart state arrives asynchronously and is filled in once.
  const fillBaseAutostart = () => {
    const base = store.getState().settingsBase, status = system.getSnapshot().status;
    if (base && base.autostart === null && status) store.dispatch({ type: "settingsBase", base: { ...base, autostart: status.autostart } });
  };
  system.subscribe(fillBaseAutostart);
  store.subscribe(fillBaseAutostart);
  const settingsWindow = createSettingsWindowBridge(api, store, controller, system);
  return { api, store, controller, system, settingsWindow };
}
