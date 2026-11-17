# Settings and updates / Einstellungen und Updates

## English

After an update, **What's new** opens once on startup, in the selected UI language (German or English). It includes changes since the last acknowledged version, newest first. An update from 0.3.4 or earlier without a version marker shows only the current version. The bundled notes work offline.

Close with **Got it**, Escape or the × button to acknowledge the notes. Opening alone does not acknowledge them: if Dualfile exits before you close the dialog, they appear again. A fresh installation skips the dialog. **Tools → What's new** or the command palette (Ctrl+K) reopens the current version's notes. The GitHub link opens that version's release page in your browser and needs internet access.

The optional `last_seen_version` in `settings.json` records the last acknowledged application version. Downgrading and manually reopening the notes never lowers it. When settings come from a newer, unsupported settings format, the dialog is unavailable because Dualfile cannot save the acknowledgment.

## Deutsch

Nach einem Update öffnet sich **Neuigkeiten** einmal beim Start, in der eingestellten UI-Sprache (Deutsch oder Englisch). Das Fenster zeigt die Änderungen seit der zuletzt bestätigten Version, neueste zuerst. Beim Update von 0.3.4 oder älter ohne Versionsmarkierung wird nur die aktuelle Version angezeigt. Die mitgelieferten Texte funktionieren offline.

Mit **Alles klar**, Escape oder × bestätigst du die Neuigkeiten. Das Öffnen allein bestätigt nichts: Beendest du Dualfile vor dem Schließen des Dialogs, erscheint er erneut. Bei einer Neuinstallation bleibt er aus. Über **Werkzeuge → Neuigkeiten** oder die Befehlspalette (Strg+K) kannst du die Hinweise zur aktuellen Version erneut öffnen. Der GitHub-Link öffnet die Release-Seite dieser Version im Browser und benötigt Internetzugang.

Das optionale Feld `last_seen_version` in `settings.json` speichert die zuletzt bestätigte App-Version. Ein Downgrade mit manuellem Öffnen setzt sie nicht zurück. Bei einem neueren, noch nicht unterstützten Einstellungsformat ist der Dialog nicht verfügbar, da Dualfile die Bestätigung nicht speichern kann.
