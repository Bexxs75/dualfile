# Settings and updates / Einstellungen und Updates

## English

After an update, **What's new** opens once on startup, in the selected UI language (German or English). It includes changes since the last acknowledged version, newest first. An update from 0.3.4 or earlier without a version marker shows only the current version. The bundled notes work offline.

Close with **Got it**, Escape or the × button to acknowledge the notes. Opening alone does not acknowledge them: if Dualfile exits before you close the dialog, they appear again. A fresh installation skips the dialog. **Tools → What's new** or the command palette (Ctrl+K) reopens the current version's notes. The GitHub link opens that version's release page in your browser and needs internet access.

The optional `last_seen_version` in `settings.json` records the last acknowledged application version. Downgrading and manually reopening the notes never lowers it. When settings come from a newer, unsupported settings format, the dialog is unavailable because Dualfile cannot save the acknowledgment.

### Start with the system

**Settings → System → Start with the system** offers three choices: **Off** (default), **Start minimized** and **Start and show the window**. Dualfile also asks once at startup; **Not now** or Escape means off, and the question does not return. An autostart entry that already exists counts as an answer.

Dualfile writes a per-user XDG autostart entry, `~/.config/autostart/de.thebexxs.dualfile.desktop` (or below `$XDG_CONFIG_HOME` when that is set). No administrator privileges are needed. The entry starts the AppImage file itself; if you move the AppImage, Dualfile corrects the path the next time it runs from the new place. **Off** removes the entry. A file of the same name that Dualfile did not create is left untouched and reported as an error next to the setting, as is a directory that cannot be written. The setting always shows what the entry on disk says: if you delete the entry or switch it off with your desktop's autostart settings, it reads **Off**.

**Start minimized** starts Dualfile with `--minimized`, without showing the window first: with the tray icon enabled it waits in the tray, otherwise it starts minimized in the task bar. Dualfile checks that a panel really shows tray icons (a StatusNotifier host); if none appears within about ten seconds after login, the window goes to the task bar instead and minimizing or closing does not hide it for that session. The setting is read from disk again whenever the settings are opened or focused. Starting Dualfile again while it runs brings the existing window to the front. The optional `autostart_prompted` in `settings.json` only records that the question was answered.

## Deutsch

Nach einem Update öffnet sich **Neuigkeiten** einmal beim Start, in der eingestellten UI-Sprache (Deutsch oder Englisch). Das Fenster zeigt die Änderungen seit der zuletzt bestätigten Version, neueste zuerst. Beim Update von 0.3.4 oder älter ohne Versionsmarkierung wird nur die aktuelle Version angezeigt. Die mitgelieferten Texte funktionieren offline.

Mit **Alles klar**, Escape oder × bestätigst du die Neuigkeiten. Das Öffnen allein bestätigt nichts: Beendest du Dualfile vor dem Schließen des Dialogs, erscheint er erneut. Bei einer Neuinstallation bleibt er aus. Über **Werkzeuge → Neuigkeiten** oder die Befehlspalette (Strg+K) kannst du die Hinweise zur aktuellen Version erneut öffnen. Der GitHub-Link öffnet die Release-Seite dieser Version im Browser und benötigt Internetzugang.

Das optionale Feld `last_seen_version` in `settings.json` speichert die zuletzt bestätigte App-Version. Ein Downgrade mit manuellem Öffnen setzt sie nicht zurück. Bei einem neueren, noch nicht unterstützten Einstellungsformat ist der Dialog nicht verfügbar, da Dualfile die Bestätigung nicht speichern kann.

### Mit dem System starten

**Einstellungen → System → Mit dem System starten** bietet drei Möglichkeiten: **Aus** (Standard), **Minimiert starten** und **Starten und Fenster zeigen**. Dualfile fragt außerdem einmal beim Start; **Nicht jetzt** oder Escape bedeutet Aus, und die Frage kommt nicht wieder. Ein bereits vorhandener Autostart-Eintrag gilt als Antwort.

Dualfile legt einen XDG-Autostart-Eintrag für deinen Benutzer an: `~/.config/autostart/de.thebexxs.dualfile.desktop` (oder unterhalb von `$XDG_CONFIG_HOME`, wenn das gesetzt ist). Administratorrechte sind nicht nötig. Der Eintrag startet die AppImage-Datei selbst; verschiebst du die AppImage, berichtigt Dualfile den Pfad, sobald es das nächste Mal vom neuen Ort aus läuft. **Aus** entfernt den Eintrag. Eine gleichnamige Datei, die nicht von Dualfile stammt, bleibt unangetastet und wird neben der Einstellung als Fehler gemeldet, ebenso ein nicht beschreibbarer Ordner. Die Einstellung zeigt immer, was der Eintrag auf der Platte sagt: Löschst du ihn oder schaltest ihn in den Autostart-Einstellungen deines Desktops ab, steht dort **Aus**.

**Minimiert starten** startet Dualfile mit `--minimized`, ohne das Fenster vorher zu zeigen: Mit eingeschaltetem Tray-Symbol wartet es im Tray, sonst startet es minimiert in der Taskleiste. Dualfile prüft, ob eine Leiste Tray-Symbole wirklich anzeigt (StatusNotifier-Host); erscheint innerhalb von etwa zehn Sekunden nach der Anmeldung keiner, kommt das Fenster stattdessen in die Taskleiste, und Minimieren oder Schließen versteckt es in dieser Sitzung nicht. Die Einstellung wird bei jedem Öffnen oder Fokussieren der Einstellungen neu von der Platte gelesen. Ein weiterer Start, während Dualfile läuft, holt das vorhandene Fenster nach vorn. Das optionale Feld `autostart_prompted` in `settings.json` hält nur fest, dass die Frage beantwortet wurde.
