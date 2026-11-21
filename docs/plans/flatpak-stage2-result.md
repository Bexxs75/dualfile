# Flatpak Stufe 2 – Ergebnis / review handoff

21.11.2026. Beratender Implementierungsbericht, keine unabhängige Freigabe.
Implemented in this checkout only; independent review and actual Flatpak runtime acceptance remain outstanding.

Auftrag und SHA256 vor Umsetzung geprüft:
`fc023b7d15f8cfd7896c1039d6c6fd2e977f2327eb8cb1f9d43a615b09ed2c17`.
Keine Commits, Pushes oder Veröffentlichungen; ursprünglichen Checkout nicht verändert.

## Umsetzung

Paket-Erkennung mit injizierbaren Eingaben und Flatpak-Vorrang; Paketart über
system_status. Updater, Startmenü-Statusabgleich und Startmigration sind im Flatpak
abgetrennt. Die Oberfläche blendet Startmenü und Update-Aktionen aus und zeigt den
DE/EN-Flathub-Hinweis. Autostart verwendet RequestBackground über vorhandenes zbus,
prüft die Response und übergibt --minimized bei minimiertem Start.

Terminal verwendet die gemeinsame Host-Aufrufstelle und meldet den Sandbox-Fallback
zweisprachig. Gerätehelfer und Mounttabellen laufen über dieselbe Host-Aufrufstelle.
Im Flatpak ersetzt vorhandenes Polling den lokalen proc-Mountmonitor. Host-Desktop-
Pfade berücksichtigen HOST_XDG_*; Einstellungen und Logs bleiben privat.
Papierkorb-Löschen, Auflisten, Wiederherstellen, Purge und Leeren nutzen Host-Pfade.
Runtime-xdg-open bleibt in der Sandbox; sein Exitstatus wird im Flatpak bis zu
120 Sekunden abgewartet und Fehler werden an die Oberfläche gegeben.

## Proof

Cargo lief offline mit einem Build-Job (CARGO_NET_OFFLINE=true, CARGO_BUILD_JOBS=1).
Der vereinbarte Befehl wurde tatsächlich ausgeführt:

```text
$ cargo check -p dualfile_core
error: package ID specification `dualfile_core` did not match any packages
help: a package with a similar name exists: `dualfile-core`
Exit 101
```

Zusätzlich mit dem tatsächlichen Paketnamen, letzter Lauf:

```text
$ cargo check -p dualfile-core
    Checking dualfile-platform v0.1.0
    Checking trash v5.3.0 (.../vendor/trash-5.3.0)
    Checking dualfile-core v0.4.1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.71s
Exit 0
```

| Prüfung | Ergebnis |
| --- | --- |
| npm run typecheck | Exit 0 |
| npm test -- src/app.system.test.tsx src/app/system.test.ts src/ipc/system-tauri.test.ts --maxWorkers=1 | 3 Dateien, 58 Tests bestanden |
| cargo test -p dualfile-platform | 3 Tests bestanden |
| cargo test -p dualfile-core --test flatpak | 1 Test bestanden, einschließlich Löschen/Auflisten/Wiederherstellen/Purge/Leeren |
| Isoliertes Prüfprojekt: cargo check -p flatpak-proof | Exit 0 |
| Prüfprojekt: Filter flatpak | 2 Tests bestanden: Starter unverändert, Portalantwort/Minimiert-Argument |
| Prüfprojekt: Filter desktop_entry::tests | 10 Tests bestanden |
| Prüfprojekt: Filter autostart::tests | 13 Tests bestanden |
| git diff --check | keine Befunde |

Das kleine Prüfprojekt unter /tmp/dualfile-flatpak-proof bindet die echten Dateien
des Checkouts per #[path] ein (desktop_entry.rs, autostart.rs, background_portal.rs).
Es benötigt kein Tauri/GTK. Warnungen betreffen dort nicht aufgerufene Funktionen.
Die Testfilter überschneiden sich; Zahlen nicht als disjunkte Tests addieren.

## Abweichungen und offene Abnahme

1. **Proof-Paketname:** dualfile_core existiert nicht. Vorgeschlagener und zusätzlich
   ausgeführter Ersatz: dualfile-core. Kein Paket umbenannt.
2. **Fehlendes Vendor-Verzeichnis:** Die vorhandene lokale Cargo-Quelle trash-5.3.0
   wurde ohne Netzwerkzugriff übernommen. Core und Tauri verwenden dieselbe lokale
   Pfadabhängigkeit. Geändert sind nur Freedesktop-Hostauflösung und Cargo-Manifest;
   das separate Portalmodul bleibt unverändert. Das standardmäßige Freedesktop-Feature
   erlaubt die vollständige Papierkorbverwaltung; das separate Portalfeature kann
   nicht auflisten/wiederherstellen/leeren. Herkunft: vendor/trash-5.3.0/DUALFILE-PATCH.md.
   MIT-Lizenz enthalten; trash steht bereits in THIRD-PARTY-LICENSES.md.
3. **Internes Crate:** dualfile-platform hat keine externen Abhängigkeiten. Es teilt
   Host-/Paketlogik zwischen Core und Trash ohne Abhängigkeitszyklus. Keine neuen
   Drittanbieter-Crates.
4. **Autostart-Status:** Das Background-Portal kann das aktuelle Autostart-Kommando
   nicht abfragen. Die Anzeige verwendet die zuletzt bestätigte, privat gespeicherte
   Auswahl; externe Änderungen werden nicht sofort sichtbar. Die Hilfe benennt das.
5. **Echte Flatpak-Abnahme offen:** Portal-Freigaben auf KDE/GNOME, Host-Dateirechte,
   flatpak-spawn-Berechtigung, reale Geräteaktionen, PTY-Interaktion und Runtime-
   xdg-open müssen im fertigen Paket geprüft werden. Manifest/Berechtigungsvergabe
   sind ausdrücklich nicht Teil dieses Auftrags.
6. **Tauri-Prüfung begrenzt:** Zwei Rust-Terminaltests für Host-Aufruf/Fallback sind
   ergänzt, aber nicht ausgeführt. Kein vollständiger Tauri-/GTK-Check, Workspace-Test
   oder Release-/Frontend-Build gemäß Ressourcenbegrenzung. Eine pauschale Aussage,
   sämtliche bestehenden Tests seien grün, ist damit nicht belegt.

Testzuordnung: (1) Controller/UI; (2) unveränderter AppImage-Starter und UI;
(3) Portalantwort/Argument plus native Autostart-Regressionen; (4) Terminaltests
vorhanden, Ausführung offen; (5) Pfadtests und isolierter Papierkorb-Rundlauf samt
Leeren; (6) Host-argv-Tests und Host-Mount-Leseweg im Papierkorbtest; (7) sichtbare
Portalfehlermeldung im UI-Test. Simulationen beweisen keine reale Portalintegration.

## Blockierte / unterlassene Aktionen

- Ein erster Offline-Proof scheiterte bei der lokalen Trash-Einbindung an der nicht
  gecachten Dev-Abhängigkeit defer. Der Ausschluss des Vendor-Crates als Workspace-
  Mitglied beseitigt diese unnötige Testabhängigkeit.
- npm ci --offline --ignore-scripts scheiterte mit ENOTCACHED. Vorhandene node_modules
  wurden anschließend lokal in den Checkout kopiert. Kein Netzwerkzugriff und keine
  Systeminstallation; der ursprüngliche Checkout wurde nicht verändert.
- Automatische Werkzeugprüfung verweigerte rm -rf target:
  "rm -f style commands are not permitted. Use a safer approach".
  Der nur für diese Prüfung erzeugte, ignorierte target-Ordner bleibt liegen (~2 GB).
  Die zusammen damit eingereichte erste Berichtsschreiboperation wurde ebenfalls
  nicht ausgeführt; dieser Bericht wurde anschließend separat geschrieben.
- Keine Berechtigungsanfragen, externen Nachrichten, Commits, Pushes, Tags oder Releases.

## Geänderte / neue Dateien

- `.gitignore`
- `Cargo.lock`
- `Cargo.toml`
- `crates/dualfile-core/Cargo.toml`
- `crates/dualfile-core/src/drives.rs`
- `crates/dualfile-core/src/lib.rs`
- `crates/dualfile-core/src/system_places.rs`
- `crates/dualfile-core/src/trash_empty.rs`
- `crates/dualfile-core/tests/flatpak.rs`
- `crates/dualfile-platform/Cargo.toml`
- `crates/dualfile-platform/src/lib.rs`
- `docs/plans/flatpak-stage2-result.md`
- `src-tauri/Cargo.lock`
- `src-tauri/src/app_id_migration.rs`
- `src-tauri/src/autostart.rs`
- `src-tauri/src/background_portal.rs`
- `src-tauri/src/desktop_entry.rs`
- `src-tauri/src/devices.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/places.rs`
- `src-tauri/src/state.rs`
- `src-tauri/src/system.rs`
- `src-tauri/src/terminal.rs`
- `src/app.system.test.tsx`
- `src/app/system.test.ts`
- `src/app/system.ts`
- `src/components/dialogs/SettingsDialog.tsx`
- `src/components/dialogs/SystemDialogs.tsx`
- `src/i18n/de.ts`
- `src/i18n/en.ts`
- `src/ipc/mock.ts`
- `src/ipc/system.ts`
- `vendor/trash-5.3.0/Cargo.toml`
- `vendor/trash-5.3.0/DUALFILE-PATCH.md`
- `vendor/trash-5.3.0/LICENSE.txt`
- `vendor/trash-5.3.0/README.md`
- `vendor/trash-5.3.0/src/freedesktop.rs`
- `vendor/trash-5.3.0/src/lib.rs`
- `vendor/trash-5.3.0/src/macos/mod.rs`
- `vendor/trash-5.3.0/src/macos/tests.rs`
- `vendor/trash-5.3.0/src/portal.rs`
- `vendor/trash-5.3.0/src/tests.rs`
- `vendor/trash-5.3.0/src/windows.rs`
