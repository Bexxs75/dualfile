import { defaultSettings } from "../app/state";
import type { Settings, SettingsBase, SettingsTab } from "../ipc/types";
import type { TranslationKey } from "../i18n";
import type { IconName } from "../components/Icon";

export const sections = ["allgemein", "darstellung", "dateiliste", "seitenleiste", "terminal", "kopieren", "tastatur", "info"] as const;
export type Section = typeof sections[number];
export const sectionIcons: Record<Section, IconName> = { allgemein: "settings", darstellung: "preview", dateiliste: "list", seitenleiste: "sidebar", terminal: "terminal", kopieren: "copy", tastatur: "tools", info: "text" };
export function resolveSection(tab?: SettingsTab | string | null): Section {
  const aliases: Record<string, Section> = { appearance: "darstellung", system: "allgemein", keyboard: "tastatur" };
  return sections.includes(tab as Section) ? tab as Section : aliases[tab ?? ""] ?? "darstellung";
}
export const normalizeSearch = (text: string) => text.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/ß/g, "ss");
export interface SettingEntry {
  id: keyof Settings; section: Section; group: string; label: TranslationKey;
  kind: "switch" | "check" | "segment" | "range" | "select" | "custom";
  help?: TranslationKey; keywords?: readonly TranslationKey[]; options?: readonly string[]; optionPrefix?: string; min?: number; max?: number; step?: number; rare?: boolean;
}
export const catalog: readonly SettingEntry[] = [
  { id: "language", section: "allgemein", group: "language", label: "settings.language", kind: "segment", options: ["auto", "de", "en"] },
  { id: "trayEnabled", section: "allgemein", group: "start", label: "system.tray", help: "system.trayHelp", kind: "switch" },
  { id: "checkUpdatesOnStart", section: "allgemein", group: "updates", label: "updates.onStart", help: "updates.onStartHelp", kind: "switch" },
  { id: "settingsWindowSizeMode", section: "allgemein", group: "window", label: "settings.windowSize", kind: "segment", options: ["ask", "always", "never"],
    keywords: ["settings.windowSize.hint.ask", "settings.windowSize.hint.always", "settings.windowSize.hint.never", "settings.windowSize.rememberedNone"] },
  { id: "theme", section: "darstellung", group: "design", label: "settings.theme", kind: "custom", options: ["schiefer", "papier", "werkstatt", "weiss", "spaceship"] },
  { id: "accentAutomatic", section: "darstellung", group: "design", label: "settings.accent.default", kind: "custom" },
  { id: "accent", section: "darstellung", group: "design", label: "settings.accent", kind: "custom", keywords: ["settings.accent.default"] },
  { id: "fontSize", section: "darstellung", group: "text", label: "settings.fontSize", kind: "range", min: 10, max: 24, step: 1 },
  { id: "density", section: "darstellung", group: "text", label: "settings.density", kind: "segment", options: ["compact", "normal", "comfortable"] },
  { id: "showParentEntry", section: "dateiliste", group: "content", label: "settings.showParentEntry", kind: "switch" },
  { id: "extensionColumn", section: "dateiliste", group: "columns", label: "settings.extensionColumn", kind: "switch" },
  ...(["showPermissionsColumn", "showUidColumn", "showGidColumn"] as const).map(id => ({ id, section: "dateiliste" as const, group: "columns", label: `settings.${id}` as TranslationKey, kind: "check" as const, keywords: ["settings.extraColumns" as const] })),
  ...(["Left", "Right"] as const).map(side => ({ id: `paneColor${side}` as keyof Settings, section: "dateiliste" as const, group: "paneColors", label: `settings.paneColors.${side.toLowerCase()}` as TranslationKey, kind: "custom" as const })),
  { id: "paneTintStrength", section: "dateiliste", group: "paneColors", label: "settings.paneTintStrength", kind: "range", min: 80, max: 100, step: 5, rare: true },
  { id: "sidebarMode", section: "seitenleiste", group: "display", label: "sidebar.mode", kind: "segment", options: ["hidden", "icons", "full"], optionPrefix: "sidebar" },
  { id: "sidebarWidth", section: "seitenleiste", group: "display", label: "sidebar.width", kind: "range", min: 160, max: 400, step: 1, rare: true },
  { id: "sidebarCollapsed", section: "seitenleiste", group: "areas", label: "settings.areas", kind: "custom", keywords: ["sidebar.places", "sidebar.remote", "places.drives"] },
  { id: "sidebarDiscovery", section: "seitenleiste", group: "areas", label: "sidebar.discovery", help: "sidebar.discoveryHelp", kind: "switch", rare: true },
  { id: "sidebarHidden", section: "seitenleiste", group: "areas", label: "sidebar.resetHidden", help: "sidebar.desktopHelp", kind: "custom", rare: true },
  { id: "terminalShell", section: "terminal", group: "shell", label: "settings.terminalShell", help: "settings.terminalShell.help", kind: "select", options: ["auto", "bash", "login"] },
  { id: "terminalColors", section: "terminal", group: "colors", label: "settings.terminal.colors", kind: "custom", options: ["theme", "classic", "green", "amber", "light", "custom"], optionPrefix: "settings.terminal.mode", keywords: ["settings.terminal.help.theme", "settings.terminal.help.custom", "settings.terminal.help.preset"] },
  { id: "terminalBg", section: "terminal", group: "colors", label: "settings.terminal.bg", kind: "custom" },
  { id: "terminalFg", section: "terminal", group: "colors", label: "settings.terminal.fg", kind: "custom" },
  { id: "verifyPolicy", section: "kopieren", group: "verify", label: "settings.verifyPolicy", help: "settings.verifyPolicy.help", kind: "segment", options: ["auto", "always", "never"] },
];
/** System actions share section/group/search metadata but are not persisted Settings fields. */
export const actionCatalog = [
  { id: "autostart", section: "allgemein", group: "start", text: ["system.autostart", "system.autostartHelp", "system.autostart.off", "system.autostart.minimized", "system.autostart.shown"] },
  { id: "startMenu", section: "allgemein", group: "start", text: ["system.startMenu", "system.add", "system.remove"] },
  { id: "about", section: "info", text: ["settings.about.heading", "settings.about.license", "settings.about.github", "settings.wiki", "whatsNew.title"] },
  { id: "updates", section: "info", text: ["updates.title", "updates.checkNow"] },
  { id: "troubleshoot", section: "info", text: ["log.heading", "log.intro", "log.view", "log.openFolder", "log.report", "log.reportHelp"] },
] satisfies readonly { id: string; section: Section; group?: string; text: TranslationKey[] }[];

const sizeOf = (settings: Settings) => JSON.stringify(settings.settingsWindowSize ?? defaultSettings.settingsWindowSize);
export function settingDefault(id: keyof Settings): Settings[keyof Settings] {
  return defaultSettings[id] ?? (id === "paneColorLeft" || id === "paneColorRight" ? "" : undefined);
}
const sortedJson = (value: unknown) => JSON.stringify(Array.isArray(value) ? [...value].sort() : value);
const baseValue = (base: SettingsBase, id: keyof Settings) => base.settings[id] ?? settingDefault(id);
/** "Modified" means: differs from the state at program start. Without a baseline nothing is modified. */
export function settingChanged(settings: Settings, base: SettingsBase | null | undefined, id: keyof Settings): boolean {
  if (!base) return false;
  // The automatic flag belongs to the accent row, not a second changed value.
  if (id === "accentAutomatic") return false;
  if (id === "accent") {
    const auto = !!(settings.accentAutomatic ?? defaultSettings.accentAutomatic), baseAuto = !!(base.settings.accentAutomatic ?? defaultSettings.accentAutomatic);
    return auto !== baseAuto || !auto && settings.accent !== base.settings.accent;
  }
  // The remembered window size belongs to the "remember size" row: changing or forgetting it counts there.
  if (id === "settingsWindowSizeMode") return (settings.settingsWindowSizeMode ?? "ask") !== (base.settings.settingsWindowSizeMode ?? "ask") || sizeOf(settings) !== sizeOf(base.settings);
  if ((id === "terminalBg" || id === "terminalFg") && settings.terminalColors !== "custom") return false;
  // Hidden sidebar entries are managed in the sidebar; the row here is an action with its own count.
  if (id === "sidebarHidden") return false;
  // The "network" section can be collapsed in the sidebar but has no control here, so it does not count.
  if (id === "sidebarCollapsed") {
    const strip = (value: readonly string[] | undefined) => (value ?? []).filter(section => section !== "network");
    return sortedJson(strip(settings.sidebarCollapsed)) !== sortedJson(strip(base.settings.sidebarCollapsed));
  }
  return sortedJson(settings[id] ?? settingDefault(id)) !== sortedJson(baseValue(base, id));
}
/** Resets one entry to the factory default (only used by "Reset everything"). */
export function resetSetting(settings: Settings, id: keyof Settings): Settings {
  if (id === "accent" || id === "accentAutomatic") return { ...settings, accent: defaultSettings.accent, accentAutomatic: defaultSettings.accentAutomatic };
  if (id === "settingsWindowSizeMode") return { ...settings, settingsWindowSizeMode: defaultSettings.settingsWindowSizeMode, settingsWindowSize: defaultSettings.settingsWindowSize };
  // Keep the network section's state: it is collapsed in the sidebar only, not in this dialog.
  if (id === "sidebarCollapsed") return { ...settings, sidebarCollapsed: (settings.sidebarCollapsed ?? []).filter(section => section === "network") };
  return { ...settings, [id]: settingDefault(id) };
}
/** Reverts one entry to its value at program start. */
export function revertSetting(settings: Settings, base: SettingsBase | null | undefined, id: keyof Settings): Settings {
  if (!base) return settings;
  if (id === "accent" || id === "accentAutomatic") return { ...settings, accent: base.settings.accent ?? defaultSettings.accent, accentAutomatic: base.settings.accentAutomatic ?? defaultSettings.accentAutomatic };
  if (id === "settingsWindowSizeMode") return { ...settings, settingsWindowSizeMode: base.settings.settingsWindowSizeMode ?? defaultSettings.settingsWindowSizeMode, settingsWindowSize: base.settings.settingsWindowSize ?? defaultSettings.settingsWindowSize };
  if (id === "sidebarCollapsed") return { ...settings, sidebarCollapsed: [...(base.settings.sidebarCollapsed ?? []).filter(section => section !== "network"), ...(settings.sidebarCollapsed ?? []).filter(section => section === "network")] };
  return { ...settings, [id]: baseValue(base, id) };
}
export function settingVisible(settings: Settings, id: keyof Settings): boolean {
  return id !== "accentAutomatic" && (!["terminalBg", "terminalFg"].includes(id) || settings.terminalColors === "custom");
}
export function settingSearch(entry: SettingEntry, tr: (key: TranslationKey) => string): string {
  return [...(entry.keywords ?? []).map(tr), tr(entry.label), entry.help ? tr(entry.help) : "", tr(`settings.section.${entry.section}` as TranslationKey), tr(`settings.group.${entry.group}` as TranslationKey), ...(entry.options ?? []).map(option => tr(`${entry.optionPrefix ?? entry.label}.${option}` as TranslationKey))].join(" ");
}
