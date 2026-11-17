import type { SettingsWindowSnapshot } from "../ipc/types";
import { project } from "./settingsWindow";
import { defaultSettings } from "./state";
export function testSnapshot(overrides: Partial<SettingsWindowSnapshot> = {}): SettingsWindowSnapshot {
  return { session: "s1", ack: 0, errors: [], settings: project({ ...defaultSettings, language: "en" }),
    system: { status: { appImage: true, startMenuExists: false }, check: "idle", error: "", installing: false, trayUnavailable: false },
    logDir: "/home/demo", persistence: { outcome: { kind: "loaded" }, error: "" }, ...overrides };
}
