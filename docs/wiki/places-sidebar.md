# Places sidebar / Orte-Seitenleiste

## English

The sidebar shows your home folder, existing XDG folders, bookmarks, trash, saved connections and mounted storage. Unmounted fstab locations are displayed with an explanation; this first step does not add mount, unmount, eject or drag-and-drop actions. Opening an automount location lets the system automounter activate it. Trash opens in the system file manager.

F9 shows or hides the sidebar. Shift+F9 switches between icons and full width. Existing custom bindings keep those keys. The command palette also offers both actions and **Focus sidebar**. The bookmark ribbon appears only when the sidebar is hidden; Alt+1–9 works in every mode.

Click opens a place in the active pane, Ctrl+click/Ctrl+Enter in the other pane, and middle-click/Shift+Enter in a new tab. Arrow keys, Home/End and typing navigate the tree; Escape returns to the file list. Sections can collapse. Right-click hides an entry or reveals hidden entries; bookmarks can be removed with Delete and restored with Undo. Sidebar preferences also appear in the separate settings window.

Drag the divider to choose a width between 160 and 400 pixels; release applies the change. Dragging below the snap threshold or double-clicking the divider switches to icons. Floating windows grow or shrink with the sidebar. On X11, their position is adjusted where the monitor work area permits. **Wayland clients cannot position their windows**, so growth goes to the right. Maximized, fullscreen or compositor-constrained windows use the available space; a narrow window temporarily uses icons without changing the saved preference.

Mount changes are detected live. GVfs/KIO locations are polled every five seconds while the sidebar and window are visible; Dolphin network locations require KIO-FUSE. Local free space refreshes periodically and after completed jobs. Filesystem calls have response deadlines and fixed worker limits; an unavailable filesystem can leave the last complete snapshot visible.

Implementation limitation awaiting independent review: GVfs/KIO mount paths that embed authentication usernames appear as redacted, display-only rows to keep those values out of sidebar DTOs. Supporting them requires navigation by opaque backend keys. No mount actions or discovery of unmounted removable devices are included in this step.

## Deutsch

Die Seitenleiste zeigt den persönlichen Ordner, vorhandene XDG-Ordner, Lesezeichen, den Papierkorb, gespeicherte Verbindungen und eingehängte Datenträger. Nicht eingehängte fstab-Orte erscheinen mit einer Erklärung. Dieser erste Schritt enthält keine Aktionen zum Einhängen, Aushängen oder Auswerfen und kein Ziehen und Ablegen. Das Öffnen eines Automount-Orts kann die System-Automount-Funktion auslösen. Der Papierkorb öffnet sich im System-Dateimanager.

F9 blendet die Seitenleiste ein oder aus. Umschalt+F9 wechselt zwischen Symbolen und vollständiger Breite. Bereits selbst belegte Tasten bleiben erhalten. Beide Aktionen und **Seitenleiste fokussieren** sind auch in der Befehlspalette verfügbar. Die Lesezeichenleiste erscheint nur bei ausgeblendeter Seitenleiste; Alt+1–9 funktioniert in jedem Modus.

Ein Klick öffnet den Ort im aktiven Fenster, Strg+Klick/Strg+Enter im anderen Fenster und ein Mittelklick/Umschalt+Enter in einem neuen Reiter. Pfeiltasten, Pos1/Ende und Tippen navigieren im Baum; Escape führt zur Dateiliste zurück. Abschnitte lassen sich einklappen. Ein Rechtsklick erlaubt das Ausblenden oder Anzeigen ausgeblendeter Einträge. Entf entfernt ein Lesezeichen; Rückgängig stellt es wieder her. Die Optionen stehen auch im separaten Einstellungsfenster.

Der Trennstrich lässt sich auf 160 bis 400 Pixel Breite ziehen; erst beim Loslassen wird die Änderung übernommen. Unterhalb der Einrastgrenze oder per Doppelklick wird zur Symbolansicht gewechselt. Frei schwebende Fenster wachsen oder schrumpfen mit der Leiste. Unter X11 wird ihre Position angepasst, soweit der Monitor-Arbeitsbereich es erlaubt. **Unter Wayland dürfen Anwendungen ihre Fenster nicht positionieren**; sie wachsen nach rechts. Maximierte, Vollbild- und vom Fenstermanager begrenzte Fenster nutzen den vorhandenen Platz. Bei wenig Platz erscheint vorübergehend die Symbolansicht, ohne die gespeicherte Einstellung zu ändern.

Mount-Änderungen werden laufend erkannt. GVfs/KIO-Orte werden bei sichtbarer Seitenleiste und sichtbarem Fenster alle fünf Sekunden geprüft; Dolphin-Netzwerkorte benötigen KIO-FUSE. Lokaler freier Speicher wird regelmäßig und nach abgeschlossenen Aufträgen aktualisiert. Dateisystemaufrufe haben Antwortfristen und feste Worker-Grenzen. Bei einem nicht erreichbaren Dateisystem kann der letzte vollständige Stand sichtbar bleiben.

Implementierungseinschränkung zur unabhängigen Prüfung: GVfs/KIO-Mountpfade mit eingebetteten Anmeldenamen werden bereinigt und ohne Navigationsfunktion angezeigt, damit diese Werte nicht in die Sidebar-DTOs gelangen. Ihre Unterstützung benötigt Navigation über undurchsichtige Backend-Schlüssel. Mount-Aktionen und die Erkennung nicht eingehängter Wechselmedien gehören nicht zu diesem Schritt.
