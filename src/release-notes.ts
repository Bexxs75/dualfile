import config from "../src-tauri/tauri.conf.json";

export const currentVersion = config.version;
export interface ReleaseText { new: readonly string[]; improved: readonly string[]; fixed: readonly string[] }
export interface ReleaseNote { date: string; de: ReleaseText; en: ReleaseText }
export const releaseNotes: Record<string, ReleaseNote> = {
  "0.3.5": {
    date: "2026-11-17",
    de: {
      new: ["Seitenleiste mit Orten, Laufwerken und Netzwerkfreigaben, live aktualisiert (F9 ein/aus, Umschalt+F9 nur Symbole). Ziehen und Ablegen öffnet den Transferdialog; Lesezeichen lassen sich anheften und sortieren.", "Wechselmedien erscheinen auch vor dem Einhängen. Einhängen, Aushängen, optisches Auswerfen und sicheres Entfernen berücksichtigen Aufträge und Tabs auf allen Partitionen.", "Optionale Netzwerksuche über Avahi: Gefundene Server öffnen einen vorbelegten Verbindungseditor. Standardmäßig aus; keine automatische Verbindung.", "Nach einem Update zeigen dir die Neuigkeiten, was sich geändert hat."],
      improved: ["Die Einstellungen öffnen sich in einem eigenen Fenster, das du frei verschieben und in der Größe ändern kannst.", "Dateien lassen sich auf einen Ordner im selben Fenster ziehen: Loslassen kopiert hinein, mit Umschalt verschieben.", "Strg+D am Shell-Prompt schließt das Terminal.", "Fenstergröße und maximierter Zustand werden wiederhergestellt. Unter X11 auch die Position; unter Wayland bestimmt sie der Desktop."],
      fixed: [],
    },
    en: {
      new: ["Sidebar with places, drives and network shares, updated live (F9 show/hide, Shift+F9 icons only). Drop files to open the transfer dialog; pin and reorder bookmarks.", "Removable media appear before mounting. Mount, unmount, optical eject and safe removal protect jobs and tabs across all partitions.", "Optional network discovery through Avahi: discovered servers open a prefilled connection editor. Off by default; never connects automatically.", "After an update, What's new shows you what changed."],
      improved: ["Settings open in their own window that you can move and resize freely.", "Drag files onto a folder in the same pane: release to copy into it, hold Shift to move.", "Ctrl+D at the shell prompt closes the terminal.", "Window size and maximized state are restored. On X11 the position is restored too; on Wayland the desktop places the window."],
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
