import config from "../src-tauri/tauri.conf.json";

export const currentVersion = config.version;
export interface ReleaseText { new: readonly string[]; improved: readonly string[]; fixed: readonly string[] }
export interface ReleaseNote { date: string; de: ReleaseText; en: ReleaseText }
export const releaseNotes: Record<string, ReleaseNote> = {
  "0.4.8": {
    date: "2026-11-26",
    de: {
      new: ["Neues Design „Spaceship“: Konsolen-Look mit schwarzem Grund, orangen, fliederfarbenen und hellblauen Pillen, schmaler Großschrift und einem Rahmen aus Bögen und Streifen oben und unten. Wählbar unter Einstellungen > Darstellung. In Nebenfenstern und bei Fensterhöhe unter 700 Pixeln gibt es keinen Rahmen.", "Neue Einstellung „Schriftstärke“ (Normal oder Fett) unter Einstellungen > Darstellung > Text und Zeilen. Sie gilt für alle Designs; das Terminal bleibt unverändert."],
      improved: ["Verbindungs-Editor und Einstellungsfenster übernehmen jetzt Design und Schriftstärke."],
      fixed: [],
    },
    en: {
      new: ["New design \"Spaceship\": a console look with a black ground, orange, lilac and light-blue pills, narrow capital lettering and a frame of arcs and stripes at the top and bottom. Choose it under Settings > Appearance. There is no frame in secondary windows or when the window is lower than 700 pixels.", "New setting \"Font weight\" (Normal or Bold) under Settings > Appearance > Text and rows. It applies to all designs; the terminal stays unchanged."],
      improved: ["The connection editor and the settings window now take over the design and font weight."],
      fixed: [],
    },
  },
  "0.4.7": {
    date: "2026-11-25",
    de: {
      new: ["RAR-Archive öffnen sich jetzt auch ohne installiertes unrar oder 7z: Das AppImage bringt ein eigenes 7-Zip mit."],
      improved: ["Große Ordner: Die Dateiliste zeichnet nur noch die sichtbaren Zeilen. Ein Ordner mit rund 44.000 Einträgen öffnet in 0,6 Sekunden statt in 30, Sortieren und Filtern dauern etwa 0,3 Sekunden, der Speicherbedarf sinkt von 4 auf 0,6 GiB.", "Einstellungen: Die Fenstergröße kann gemerkt werden (Fragen, Immer merken, Nie merken).", "WebDAV- (https) und NFS-Orte des Systems erscheinen als eigene Verbindungen."],
      fixed: [],
    },
    en: {
      new: ["RAR archives now open even without unrar or 7z installed: the AppImage ships its own 7-Zip."],
      improved: ["Large folders: the file list only draws the visible rows. A folder with about 44,000 entries opens in 0.6 seconds instead of 30, sorting and filtering take about 0.3 seconds, and memory use drops from 4 to 0.6 GiB.", "Settings: the window size can be remembered (ask, always, never).", "WebDAV (https) and NFS places known to the system appear as their own connections."],
      fixed: [],
    },
  },
  "0.4.6": {
    date: "2026-11-25",
    de: {
      new: [],
      improved: [],
      fixed: ["Sicherheit: Ein Papierkorb auf einem anderen Laufwerk wird nur noch genutzt, wenn er dir gehört und sonst niemand darin schreiben kann. Verschieben in den Papierkorb weicht sonst auf deinen Papierkorb im Heimordner aus.", "WebDAV: Dateien mit nicht darstellbaren Namen werden als Fehler aufgelistet, und ein Verschieben löscht sie nicht mehr mit.", "7z-Archive: Sehr große Wörterbuch- oder Eintragsangaben werden abgelehnt, statt den Arbeitsspeicher zu füllen."],
    },
    en: {
      new: [],
      improved: [],
      fixed: ["Security: a trash folder on another drive is only used when it belongs to you and nobody else can write to it. Otherwise moving to the trash falls back to your trash in the home folder.", "WebDAV: files with names that cannot be shown are listed as errors, and moving them no longer deletes them along the way.", "7z archives: very large dictionary or entry counts are rejected instead of filling up memory."],
    },
  },
  "0.4.5": {
    date: "2026-11-25",
    de: {
      new: [],
      improved: [],
      fixed: ["Verbindungen: Schlägt die Anmeldung nach dem Bestätigen des Server-Schlüssels fehl, weil das gespeicherte Passwort veraltet ist, öffnet sich jetzt der Passwort-Dialog.", "Papierkorb leeren meldet auch eigene Papierkorb-Ordner, die sich nicht öffnen lassen (etwa wegen fehlender Rechte); ihr Inhalt bleibt unberührt.", "WebDAV: Ein falsches Passwort wird als Anmeldefehler gemeldet und ein nicht erreichbarer Server als Verbindungsfehler, statt „Ungültige Eingabe“. Ein leerer Basispfad bedeutet jetzt das Wurzelverzeichnis des Servers."],
    },
    en: {
      new: [],
      improved: [],
      fixed: ["Connections: if the login fails after you confirm the server key because the saved password is outdated, the password dialog now opens.", "Emptying the trash also reports your own trash folders that cannot be opened (for example due to missing permissions); their contents are left alone.", "WebDAV: a wrong password is reported as a login failure and an unreachable server as a connection error, instead of \u201cInvalid input\u201d. An empty base path now means the server root."],
    },
  },
  "0.4.4": {
    date: "2026-11-24",
    de: {
      new: ["Neuer Kontextmenü-Eintrag „Hier duplizieren“: legt eine Kopie im selben Ordner mit freiem Namen an (name_copy, name_copy2). F5 mit beiden Fenstern im selben Ordner kopiert jetzt direkt ohne Dialog."],
      improved: ["Gespeicherte Passwörter werden ohne Nachfrage genutzt; der Passwort-Dialog erscheint erst, wenn die Anmeldung fehlschlägt.", "Papierkorb leeren meldet Ordner, die wegen fehlender Rechte übersprungen wurden."],
      fixed: ["WebDAV und NFS: Die Dateiliste blieb leer, wenn der Startordner leer war oder der Pfad in der Schreibweise webdav:/nfs: angegeben wurde."],
    },
    en: {
      new: ["New context-menu entry \u201cDuplicate here\u201d: creates a copy in the same folder with a free name (name_copy, name_copy2). F5 with both panes in the same folder now copies directly without a dialog."],
      improved: ["Saved passwords are used without asking; the password dialog only appears when the login fails.", "Emptying the trash reports folders that were skipped because of missing permissions."],
      fixed: ["WebDAV and NFS: the file list stayed empty when the start folder was empty or the path used the webdav:/nfs: notation."],
    },
  },
  "0.4.3": {
    date: "2026-11-22",
    de: {
      new: [],
      improved: ["In den Einstellungen gelten „Geändert“, der Zähler und „Zurücksetzen“ jetzt für die laufende Sitzung (Stand beim Programmstart); nach einem Neustart sind die Markierungen weg.", "Helle Designs (Weiß, Papier): kräftigere Schrift und im Weiß ein dunkleres Grau für Nebentexte, damit der Text besser lesbar ist."],
      fixed: [],
    },
    en: {
      new: [],
      improved: ["In Settings, \u201cModified\u201d, the counter and \u201cReset\u201d now refer to the current session (state at program start); the markers are gone after a restart.", "Light themes (White, Paper): heavier text and a darker grey for secondary text in White for better readability."],
      fixed: [],
    },
  },
  "0.4.2": {
    date: "2026-11-21",
    de: {
      new: ["Dateiliste: Trenner, Breite und Reihenfolge der Spalten Rechte, UID und GID lassen sich anpassen (Reihenfolge per Maus oder Alt+Links/Rechts, Zurücksetzen mit Alt+Pos1).", "Kontextmenüs sind vervollständigt und gruppiert, Kürzel stehen rechts, bei ausgegrauten Einträgen wird der Grund genannt."],
      improved: ["Menü-Begriffe sind vereinheitlicht, doppelte Einträge entfernt, das Werkzeuge-Menü ist gruppiert."],
      fixed: ["In hellen Designs färbt die Akzentfarbe jetzt Auswahl, Cursorzeile und Ordner."],
    },
    en: {
      new: ["File list: separators, width and order of the Permissions, UID and GID columns are adjustable (reorder by mouse or Alt+Left/Right, reset with Alt+Home).", "Context menus are completed and grouped, shortcuts sit on the right, disabled entries state the reason."],
      improved: ["Menu terms are unified, duplicate entries removed, the Tools menu is grouped."],
      fixed: ["In light themes the accent colour now tints selection, cursor row and folders."],
    },
  },
  "0.4.1": {
    date: "2026-11-21",
    de: {
      new: [],
      improved: ["Dualfile hat eine neue App-Kennung (com.dualfileapp.Dualfile) als Vorbereitung für Flatpak. Fensterposition und -größe, Autostart und der Startmenü-Eintrag werden beim ersten Start übernommen.", "Läuft noch eine ältere Dualfile-Version, holt der Start sie nach vorn, statt ein zweites Dualfile zu öffnen."],
      fixed: ["Der Hinweis zum Autostart nennt jetzt den richtigen Bereich der Einstellungen („Allgemein“)."],
    },
    en: {
      new: [],
      improved: ["Dualfile has a new application ID (com.dualfileapp.Dualfile) in preparation for Flatpak. Window position and size, autostart and the start-menu entry are carried over on first start.", "If an older Dualfile is still running, starting Dualfile brings it to the front instead of opening a second one."],
      fixed: ["The autostart hint now names the right settings section (\u201cGeneral\u201d)."],
    },
  },
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
