import { defaultSettings } from "../app/state";
import type { Settings, SettingsTab } from "../ipc/types";
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
  { id: "theme", section: "darstellung", group: "design", label: "settings.theme", kind: "custom", options: ["schiefer", "papier", "werkstatt", "weiss"] },
  { id: "accentAutomatic", section: "darstellung", group: "design", label: "settings.accent.default", kind: "custom" },
  { id: "accent", section: "darstellung", group: "design", label: "settings.accent", kind: "custom" },
  { id: "fontSize", section: "darstellung", group: "text", label: "settings.fontSize", kind: "range", min: 10, max: 24, step: 1 },
  { id: "density", section: "darstellung", group: "text", label: "settings.density", kind: "segment", options: ["compact", "normal", "comfortable"] },
  { id: "showParentEntry", section: "dateiliste", group: "content", label: "settings.showParentEntry", kind: "switch" },
  { id: "extensionColumn", section: "dateiliste", group: "columns", label: "settings.extensionColumn", kind: "switch" },
  ...(["showPermissionsColumn", "showUidColumn", "showGidColumn"] as const).map(id => ({ id, section: "dateiliste" as const, group: "columns", label: `settings.${id}` as TranslationKey, kind: "check" as const, keywords: ["settings.extraColumns" as const] })),
  ...(["Left", "Right"] as const).map(side => ({ id: `paneColor${side}` as keyof Settings, section: "dateiliste" as const, group: "paneColors", label: `settings.paneColors.${side.toLowerCase()}` as TranslationKey, kind: "custom" as const })),
  { id: "paneTintStrength", section: "dateiliste", group: "paneColors", label: "settings.paneTintStrength", kind: "range", min: 80, max: 100, step: 5, rare: true },
  { id: "sidebarMode", section: "seitenleiste", group: "display", label: "sidebar.mode", kind: "segment", options: ["hidden", "icons", "full"], optionPrefix: "sidebar" },
  { id: "sidebarWidth", section: "seitenleiste", group: "display", label: "sidebar.width", kind: "range", min: 160, max: 400, step: 10, rare: true },
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
  { id: "about", section: "info", text: ["settings.about.heading", "settings.about.license", "settings.about.github", "settings.wiki"] },
  { id: "updates", section: "info", text: ["updates.title", "updates.checkNow"] },
  { id: "troubleshoot", section: "info", text: ["log.heading", "log.intro", "log.view", "log.openFolder", "log.report", "log.reportHelp"] },
] satisfies readonly { id: string; section: Section; group?: string; text: TranslationKey[] }[];

export function settingDefault(id: keyof Settings): Settings[keyof Settings] {
  return defaultSettings[id] ?? (id === "paneColorLeft" || id === "paneColorRight" ? "" : undefined);
}
export function settingChanged(settings: Settings, id: keyof Settings): boolean {
  const value = settings[id] ?? settingDefault(id);
  const standard = settingDefault(id);
  if (Array.isArray(value) && Array.isArray(standard)) return JSON.stringify([...value].sort()) !== JSON.stringify([...standard].sort());
  return value !== standard;
}
export function resetSetting(settings: Settings, id: keyof Settings): Settings { return { ...settings, [id]: settingDefault(id) }; }
export function settingSearch(entry: SettingEntry, tr: (key: TranslationKey) => string): string {
  return [...(entry.keywords ?? []).map(tr), tr(entry.label), entry.help ? tr(entry.help) : "", tr(`settings.section.${entry.section}` as TranslationKey), tr(`settings.group.${entry.group}` as TranslationKey), ...(entry.options ?? []).map(option => tr(`${entry.optionPrefix ?? entry.label}.${option}` as TranslationKey))].join(" ");
}
