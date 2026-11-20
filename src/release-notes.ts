import config from "../src-tauri/tauri.conf.json";

export const currentVersion = config.version;
export interface ReleaseText { new: readonly string[]; improved: readonly string[]; fixed: readonly string[] }
export interface ReleaseNote { date: string; de: ReleaseText; en: ReleaseText }
export const releaseNotes: Record<string, ReleaseNote> = {
  "0.4.0": {
    date: "2026-11-20",
    de: {
      new: ["Neue Einstellungen: acht Bereiche in einer Seitenleiste, eine Suche über alle Einstellungen und der Filter „Geändert“, der nur zeigt, was vom Standard abweicht.", "Jede Einstellung lässt sich einzeln zurücksetzen, „Alles auf Standard …“ fragt vorher nach.", "Bei Darstellung und Dateiliste zeigt eine Vorschau sofort, wie die Dateiliste aussieht.", "Unter Info & Hilfe öffnet ein Knopf das Handbuch (Wiki)."],
      improved: ["Eine nicht gesetzte Fensterfarbe zeigt „keine“, die Deckkraft ist erst mit einer Farbe einstellbar.", "Die Akzentfarbe nimmt Werte auch ohne führendes # an."],
      fixed: [],
    },
    en: {
      new: ["New settings: eight sections in a sidebar, a search across all settings and a Modified filter that shows only what differs from the defaults.", "Every setting can be reset on its own; \u201cReset everything \u2026\u201d asks first.", "Appearance and File list show a live preview of the file list.", "Info & Help has a button that opens the manual (wiki)."],
      improved: ["An unset pane colour shows \u201cnone\u201d; opacity can only be set once a colour is chosen.", "The accent colour accepts values without a leading #."],
      fixed: [],
    },
  },
  "0.3.10": {
    date: "2026-11-20",
    de: {
      new: [],
      improved: ["Die Fensterfarbe färbt nur noch die Dateiliste. Tabs, Pfadzeile, Spaltenkopf und Statuszeile bleiben in den normalen Farben.", "Rechte, Besitzer (uid) und Gruppe (gid) lassen sich in den Einstellungen einzeln einblenden."],
      fixed: [],
    },
    en: {
      new: [],
      improved: ["The pane colour now tints only the file list. Tabs, path bar, column header and status bar keep the normal colours.", "Permissions, owner (uid) and group (gid) can be shown individually in Settings."],
      fixed: [],
    },
  },
  "0.3.9": {
    date: "2026-11-20",
    de: {
      new: ["Neues Design „Weiß“: reines Weiß mit eigenem blauen Akzent.", "Auf Wunsch zeigt die Dateiliste Rechte, Besitzer (uid) und Gruppe (gid) an, sortierbar. Einschalten in den Einstellungen."],
      improved: ["Fensterfarben erscheinen jetzt satt und deckend. Ein Regler stellt die Deckung zwischen 80 und 100 % ein; Schrift, Auswahl und Cursor passen sich automatisch an, damit alles lesbar bleibt."],
      fixed: [],
    },
    en: {
      new: ["New “White” design: pure white with its own blue accent.", "The file list can show permissions, owner (uid) and group (gid), sortable. Turn it on in Settings."],
      improved: ["Pane colours now appear rich and opaque. A slider sets the opacity between 80 and 100%; text, selection and cursor adapt automatically so everything stays readable."],
      fixed: [],
    },
  },
  "0.3.8": {
    date: "2026-11-18",
    de: {
      new: ["Netzwerkorte, die dein Desktop schon kennt (Dolphin, GNOME Dateien), stehen direkt unter Verbindungen. Ein Klick speichert und verbindet sie."],
      improved: ["Eingesteckte Geräte, neue Einhängepunkte und geänderte Lesezeichen erscheinen in der Seitenleiste nach etwa einer Sekunde statt nach bis zu einer Minute.", "Einhängepunkte, die du nicht öffnen darfst, werden unter Geräte nicht mehr angezeigt."],
      fixed: ["Die Passwortabfrage beim Verbinden heißt jetzt „Passwort“ statt „Antwort“."],
    },
    en: {
      new: ["Network locations your desktop already knows (Dolphin, GNOME Files) appear directly under Connections. One click saves and connects them."],
      improved: ["Plugged-in devices, new mounts and changed bookmarks show up in the sidebar after about a second instead of up to a minute.", "Mounts you may not open are no longer listed under Devices."],
      fixed: ["The password prompt when connecting is now labeled “Password” instead of “Answer”."],
    },
  },
  "0.3.7": {
    date: "2026-11-18",
    de: {
      new: ["Mit dem System starten: aus, minimiert oder mit Fenster. Dualfile fragt einmal beim Start, später unter Einstellungen → System änderbar."],
      improved: ["Unter X11 öffnet sich das Fenster wieder dort, wo du es zuletzt hattest."],
      fixed: ["Nach einem minimierten Start im Tray hat das Fenster wieder seine gespeicherte Größe."],
    },
    en: {
      new: ["Start with the system: off, minimized or with the window shown. Dualfile asks once at startup; change it later under Settings → System."],
      improved: ["On X11 the window opens again where you left it."],
      fixed: ["After a minimized start in the tray the window has its saved size again."],
    },
  },
  "0.3.6": {
    date: "2026-11-17",
    de: {
      new: ["Der Papierkorb öffnet sich in Dualfile als eigener Tab: Wiederherstellen, endgültig löschen und leeren, jeweils mit Rückfrage beim Löschen.", "Dateien aus Dolphin oder vom Desktop lassen sich in ein Fenster oder auf einen Ordner ziehen; es öffnet sich der Kopierdialog.", "Rechtsklick auf einen Ordner: „Zu Orten hinzufügen“ legt ihn in die Seitenleiste."],
      improved: ["Entf und F8 fragen immer nach, bevor etwas in den Papierkorb wandert.", "Die Zeile, auf die Entf wirkt, ist deutlich markiert; die Hervorhebung unter der Maus ist dezenter."],
      fixed: ["Ein Ubuntu-Installationsstick erscheint nicht mehr doppelt in der Seitenleiste und lässt sich öffnen."],
    },
    en: {
      new: ["The Trash opens inside Dualfile as its own tab: restore, delete permanently and empty, always confirmed before deleting.", "Drag files from Dolphin or the desktop onto a pane or a folder; the copy dialog opens.", "Right-click a folder and choose \"Add to Places\" to put it in the sidebar."],
      improved: ["Delete and F8 always ask before moving anything to the Trash.", "The row Delete acts on is clearly marked; the mouse-hover highlight is subtler."],
      fixed: ["An Ubuntu installer stick no longer appears twice in the sidebar and can be opened."],
    },
  },
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
