import config from "../src-tauri/tauri.conf.json";

export const currentVersion = config.version;
export interface ReleaseText { new: readonly string[]; improved: readonly string[]; fixed: readonly string[] }
export interface ReleaseNote { date: string; de: ReleaseText; en: ReleaseText }
export const releaseNotes: Record<string, ReleaseNote> = {
  "0.3.5": {
    date: "2026-11-17",
    de: {
      new: ["Nach einem Update zeigen dir die Neuigkeiten, was sich geändert hat."],
      improved: ["Die Einstellungen öffnen sich in einem eigenen Fenster, das du frei verschieben und in der Größe ändern kannst.", "Strg+D am Shell-Prompt schließt das Terminal.", "Fenstergröße und maximierter Zustand werden wiederhergestellt. Unter X11 auch die Position; unter Wayland bestimmt sie der Desktop."],
      fixed: [],
    },
    en: {
      new: ["After an update, What's new shows you what changed."],
      improved: ["Settings open in their own window that you can move and resize freely.", "Ctrl+D at the shell prompt closes the terminal.", "Window size and maximized state are restored. On X11 the position is restored too; on Wayland the desktop places the window."],
      fixed: [],
    },
  },
  "0.3.4": {
    date: "2026-11-16",
    de: {
      new: ["Verbindungen lassen sich in einem eigenen Fenster bearbeiten."],
      improved: ["Das Verbindungsmenü ist kompakter und bietet Aktionen in einem Untermenü.", "Neue Installationen starten im Terminal deine eigene Shell.", "Der Fenstertitel zeigt die Version. Lesezeichen öffnen sich nur im aktiven Fenster."],
      fixed: [],
    },
    en: {
      new: ["Edit connections in a separate window."],
      improved: ["The connection menu is more compact, with actions in a submenu.", "New installations start your own shell in the terminal.", "The window title shows the version. Bookmarks open in the active pane only."],
      fixed: [],
    },
  },
  "0.3.3": {
    date: "2026-11-16",
    de: { new: [], improved: [], fixed: ["Nach Updates starten Dualfile und externe Programme wieder zuverlässig. Beim Update auf diese Version gegebenenfalls einmal manuell neu starten.", "Server-Tabs verhindern nicht mehr das Speichern der Einstellungen."] },
    en: { new: [], improved: [], fixed: ["Dualfile and external programs start reliably after updates. When updating to this version, you may need to restart manually once.", "Server tabs no longer prevent settings from being saved."] },
  },
};
