# Leistungstest große Ordner (Issue #28), Messung vom 25.11.2026

Gemessen an Release 0.4.6 (AppImage `Dualfile-0.4.6-x86_64`, Tag `v0.4.6`). Alle Zahlen mit Methode; was nicht gemessen wurde, steht am Ende.

## Ergebnis in einem Satz

Der Rust-Teil (Ordner lesen) ist unkritisch: 50.000 Einträge in rund 60 ms. Die Oberfläche ist es nicht: Die Dateiliste rendert alle Zeilen (keine Virtualisierung), dadurch dauert das Öffnen eines Ordners mit 50.000 Einträgen in der VM 25 bis 46 Sekunden, der Web-Prozess braucht 3,5 bis 7 GB RAM und wurde bei 4 GB RAM der VM nach 1,5 bis 2,5 Minuten beendet (im ersten Lauf als OOM-Kill belegt). Das Ziel „unter 1 s bis bedienbar“ wird um zwei Größenordnungen verfehlt.

## Ziel gegen gemessen

| Ziel (Issue) | Gemessen | Wert | Ziel erreicht |
| --- | --- | --- | --- |
| Ordner mit 50.000 Einträgen öffnen, < 1 s bis bedienbar | Enter bis erste Zeile sichtbar (VM, 3 Läufe) | 33 s, 46 s, 46 s | nein |
| | Bis Tasten wieder reagieren | ab der ersten Zeile mit Aussetzern (siehe unten), frühestens ca. 46 s | nein |
| | Rust-Teil allein (`LocalVfs::list`, 50.000 Einträge) | 57 ms gemischt, 43 ms nur Dateien | ja |
| Scrollen flüssig | Scrollen mit 100 Radklicks, geänderte Bilder je Sekunde | 1,3 pro Sekunde bei 50k, gegen 15 bis 18 pro Sekunde bei 500 und 10k Einträgen | nein (VM ohne GPU, siehe Grenzen) |
| Sortieren/Filtern < 300 ms | Klick auf Spaltenkopf bis neu gezeichnet (VM) | 32 bis 50 s | nein |
| | Reine Rechenzeit Sortieren (Node/V8, ruhiger Rechner) | Name 112 ms, Größe 50 ms, Typ 70 ms, Datum 16 ms | ja (nur Rechenanteil) |
| | Reine Rechenzeit Filtern (Node/V8) | 0,6 bis 8 ms | ja (nur Rechenanteil) |
| Kopierauftrag mit 10.000 kleinen Dateien ohne Hänger | 10.000 Dateien à 1 KiB lokal (VM, 3 Läufe) | 26 bis 28 s Dauer, Taste-zu-Reaktion im Median 0,4 bis 0,5 s, p90 1,5 bis 1,8 s, schlimmster Wert 2,5 bis 2,8 s | teilweise (kein Stillstand, aber Verzögerungen bis ca. 3 s) |

Zwischenwerte zur Skalierung des Öffnens (VM, Enter bis erste Zeile): 500 Einträge 0,4 s, 10.000 Einträge 2,4 s, 46.250 sichtbare Einträge 33 bis 46 s. Das ist überlinear.

Speicher des Web-Prozesses (`WebKitWebProcess`, RSS): Leerlauf 410 MiB, 500 Einträge 520 MiB, 10.000 Einträge 1,1 GiB, 50.000 Einträge Spitze 3,5 bis 4,2 GiB und danach 2,7 GiB. Nach Sortieren und Filtern in derselben Sitzung stieg die Spitze auf 7,0 GiB. Der Hauptprozess `dualfile` bleibt bei 200 bis 260 MiB.

## Rechner und Aufbau

- Host ObiWan: Intel Core i5-14600K (20 Threads), 62 GB RAM, Datei- und Testordner auf ext4 (`/mnt/Daten2`, SSD), Linux 7.3.5, Rust-Bau im Release-Profil.
- Rust-Zeiten: Zuerst unter fremder Last (Load Average 100 bis 280 durch andere Aufträge; dieselbe Messung schwankte zwischen 56 und 354 ms), dann in Ruhe (Load 0,8) wiederholt. Genannt werden die Werte in Ruhe.
- GUI-Zeiten: VM `kubuntu-kde` (Kubuntu 24.04, Plasma auf X11, 4 vCPU, 1920x1080, Software-Rendering ohne GPU). Für die 50k-Läufe 12 GB RAM, für den OOM-Nachweis 4 GB RAM. Die VM lief zeitweise neben einer zweiten Test-VM und teilweise unter Last des Hosts, daher sind die absoluten Sekunden ungenau, die Größenordnung nicht.
- Testordner: 50.000 Einträge in einem Ordner, 30.000 Dateien (Größen 0 bis 100 KB als Sparse-Dateien, Änderungszeit über Jahre gestreut) und 20.000 leere Unterordner, Namen mit Umlauten, CJK, Leerzeichen und Punktdateien (ein Achtel versteckt, daher zeigt die App 46.250 Einträge). Zweiter Ordner: 50.000 nur Dateien. Kopierquelle: 10.000 Dateien à 1 KiB.

## Methode

### A. Rust-Teil (reproduzierbar, ohne Oberfläche)

`crates/dualfile-core/tests/perf_listing.rs` (`#[ignore]`):

```text
cargo test -p dualfile-core --test perf_listing --release -- --ignored --nocapture --test-threads=1
```

Gemessen werden `LocalVfs::list`, die Umwandlung in das Transportobjekt samt Besitzernamen (Spiegel von `EntryDto`, denn das echte liegt im Tauri-Crate), die JSON-Serialisierung als Stellvertreter für die IPC-Übertragung, und der RSS-Zuwachs aus `/proc/self/status`. Je 5 Läufe pro Prozess, 3 Prozesse. Sortieren und Filtern gibt es im Rust-Teil nicht; sie laufen im Web-Teil (`src/state/sort.ts`, `src/state/filter.ts`).

| Messgröße (50.000 Einträge) | Median | Streuung (3 Prozesse x 5 Läufe) |
| --- | --- | --- |
| `LocalVfs::list`, gemischt (30k Dateien, 20k Ordner) | 57 ms | 57,0 bis 58,3 ms |
| `LocalVfs::list`, nur Dateien | 43 ms | 42,2 bis 44,2 ms |
| Umwandlung samt Besitzernamen | 6,4 ms | 6,2 bis 9,2 ms |
| JSON-Serialisierung | 9,4 ms | 9,0 bis 10,6 ms |
| JSON-Nutzlast | 11,8 MiB (12,4 MB) | fest |
| RSS-Zuwachs Liste, dann Umwandlung samt JSON | +8 bis 9 MiB, danach +29 bis 31 MiB | je Erstlauf im frischen Prozess |
| Skalierung 5.000 auf 50.000 Einträge | Faktor 11,5 (linear wäre 10) | |

Alle Werte gelten für warmen Dateisystem-Cache (kalter Cache braucht Root zum Leeren und wurde nicht gemessen).

### A2. Sortieren und Filtern als reine Rechenzeit

`src/state/perf28.test.ts` (nur mit `DUALFILE_PERF=1`), 50.000 Einträge, zufällig gemischt, 5 Läufe pro Zeile, Node 26 (V8), nicht WebKitGTK:

```text
DUALFILE_PERF=1 npx vitest run src/state/perf28.test.ts
```

| Aufruf | Median |
| --- | --- |
| `sortEntries` nach Name aufsteigend / absteigend | 112 ms / 109 ms |
| nach Größe / Typ / Datum | 50 ms / 70 ms / 16 ms |
| `filterEntries` ohne Text / „bericht“ / „*.pdf“ | 0,6 ms / 2,5 ms / 8 ms |
| Filter „bericht“ plus Sortierung Name | 8,8 ms |
| Filter leer plus Sortierung Name | 100 ms |

Der Rechenanteil liegt also unter dem Ziel von 300 ms. Wie lang es wirklich dauert, entscheidet das anschließende Neuzeichnen der Liste (siehe Engpässe).

### B. Oberfläche in der VM

Die Test-VM lief mit dem Release-AppImage 0.4.6. Aufnahme des ganzen Bildschirms im Gast mit `ffmpeg -f x11grab` (30 Bilder pro Sekunde, Zeitstempel der Wanduhr, `-copyts`), Tastendrücke und Klicks per `xdotool` mit Zeitstempel aus derselben Gastuhr. Auswertung: Bild-für-Bild-Vergleich des Bereichs der aktiven Dateiliste (auf 84x86 Grauwerte verkleinert), „erste Zeile sichtbar“ = erstes Bild nach dem Auslösen mit großem Unterschied. „Bedienbar“ wurde als Antwortzeit auf Probetasten (Pfeil runter/hoch alle 2 s oder 0,5 s) bestimmt: Ein Tastendruck zählt als beantwortet, wenn sich die Markierung sichtbar bewegt. Speicher: `ps` im Gast alle 0,25 s für `dualfile`, `WebKitWebProcess`, `WebKitNetworkProcess`.

## Befunde in der VM im Einzelnen

1. **Öffnen**: Nach Enter bleibt das Fenster ohne Rückmeldung stehen (kein „Lade …“, der alte Ordner bleibt stehen), dann erscheint die fertige Liste in einem einzigen Bild. 33 s, 46 s, 46 s (3 Läufe, frisch gestartete App). Der Web-Prozess wächst dabei von 410 MiB auf 3,5 bis 4,2 GiB (Spitze), danach 2,7 GiB.
2. **Danach nicht sofort bedienbar**: Im Probelauf (Probetaste alle 2 s) reagierte die Oberfläche ab 46 s, aber mit Aussetzern von etwa 10 s Länge bei 64 bis 74 s und 94 bis 102 s (keine sichtbare Reaktion auf Tasten). Im ersten Lauf kam der Tastendruck bei 70 s erst bei 80 s an. Ein stabiles „bedienbar“ wurde im Messfenster nicht erreicht.
3. **Sortieren per Spaltenkopf**: 38 s (Größe aufwärts), 50 s (Größe abwärts), 35 s (Name), 33 s (Änderungsdatum), jeweils bis zum Neuzeichnen. Der Web-Prozess erreicht dabei bis zu 7,0 GiB.
4. **Filter**: Tippen von „bericht“ zeigte das erste Zeichen nach 19 s, Zwischenstände je Buchstabe mit mehreren Sekunden Abstand; Löschen des Textes dauerte 15 s bis zur Anzeige.
5. **Scrollen** (100 Radklicks): bei 500 Einträgen 15 und bei 10.000 Einträgen 18 geänderte Bilder pro Sekunde (kein Stau); bei 50.000 nur 1,3 pro Sekunde und das letzte Bild kam 5,9 s nach dem Ende der Eingabe.
6. **4 GB RAM der VM**: Der Web-Prozess wuchs bis 2,8 GB und wurde vom Kernel beendet (Kernelmeldung im ersten Lauf belegt: `Out of memory: Killed process … WebKitWebProces … anon-rss:2794296kB`, nach etwa 2,5 Minuten; im zweiten Lauf verschwand der Prozess nach etwa 1,5 Minuten bei 2,8 GB, die Kernelmeldung wurde dort nicht mehr abgerufen), die Oberfläche blieb eingefroren und zeigte die Liste nie. Mit 12 GB RAM lief es durch.
7. **Kopierauftrag, 10.000 Dateien à 1 KiB** (lokal, alles markiert, F5, Enter): fertig nach 25,8 s, 27,5 s, 27,9 s (ca. 360 Dateien pro Sekunde). Während des Kopierens reagierten Tasten im Median nach 0,4 bis 0,5 s, p90 1,5 bis 1,8 s, schlimmster Wert 2,5 bis 2,8 s; zum Vergleich im Leerlauf bei 500 Einträgen 0,12 bis 0,16 s. Kein Stillstand, aber spürbares Ruckeln (vermutlich durch das Neuzeichnen des Zielordners mit zuletzt 10.000 Zeilen).

## Engpässe mit Fundstelle

1. **Keine Virtualisierung der Dateiliste** (Hauptursache): `src/components/FileList.tsx` rendert `entries.map(... <Row ...>)` für jeden Eintrag (ab Zeile 430). Jede Zeile besteht aus rund 12 bis 16 DOM-Knoten und hat bis zu 8 Ereignis-Handler. Bei 46.250 Zeilen sind das gut 700.000 Knoten, rund 60 bis 70 KB Web-Prozess-Speicher je Zeile (10.000 Einträge 680 MiB über Leerlauf, 46.250 Einträge rund 3 GiB über Leerlauf an der Spitze). `content-visibility: auto` in `src/styles/app.css` (Zeile 108) spart nur Layout und Malen außerhalb des Bildes, nicht das Erzeugen der Knoten und nicht den Speicher.
2. **Jede Änderung rendert alles neu**: Sortieren und Filtern erzeugen in `visibleEntries` (`src/app/state.ts`, Zeile 242 bis 256) eine neue Liste, `FileList` bildet sie wieder komplett ab. Deshalb kostet auch der Klick auf einen Spaltenkopf 33 bis 50 s, obwohl die Sortierung selbst unter 120 ms bleibt.
3. **Ein einzelner Ordnerwechsel blockiert den Web-Prozess** (kein Zwischenzustand „Lade …“, solange der Hauptthread beschäftigt ist; der Nutzer sieht einen eingefrorenen alten Ordner).
4. **Rust-Teil** (zur Einordnung, kein Engpass): `LocalVfs::list` (`crates/dualfile-core/src/vfs.rs`, Zeile 970) macht je Eintrag `symlink_metadata`, bei Ordnern zusätzlich `fs::read_dir(path).err()` als Zugriffsprobe (Zeile 671 bis 675). Das kostet bei 20.000 Unterordnern etwa 15 ms mehr (57 gegenüber 43 ms). Die IPC-Nutzlast ist 11,8 MiB JSON für 50.000 Einträge (ein einziger Aufruf, `src-tauri/src/commands.rs` Zeile 48 und `src-tauri/src/state.rs` Zeile 848).
5. **Fernzugriff** (nur aus dem Code, nicht gemessen): `crates/dualfile-remote/src/transport.rs` Zeile 31 bis 45 begrenzt Listen auf 100.000 Einträge und 16 MiB, `sftpvfs.rs` Zeile 74 auf 120 s. 50.000 Einträge sind zulässig; dieselbe Dateiliste (Engpass 1) gilt für alle Quellen.

## Empfehlung zum Lasttest in der CI

- **Keinen Millisekunden-Schwellentest in die CI.** Dieselbe Rust-Messung schwankte unter Last zwischen 56 und 354 ms, in Ruhe nur um 1 ms. Das Projekt hat schon wackelnde Zeit-Tests; ein Schwellenwert auf Wanduhrzeit würde auf geteilten CI-Rechnern abwechselnd rot und grün.
- **Der Rust-Test bleibt `#[ignore]` und wird nur von Hand oder im manuellen Arbeitsablauf gestartet** (Referenz: 57 ms für 50k). Er enthält nur eine grobe Komplexitätsprüfung (Faktor 5k auf 50k unter 30, quadratisches Verhalten ergäbe etwa 100), die auch bei Lärm nicht flackert.
- **Was die CI stattdessen prüfen sollte, sind Zählgrößen am eigentlichen Engpass**, zusammen mit der Behebung von Engpass 1: Ein Vitest-Test (jsdom), der `FileList` mit 50.000 Einträgen darstellt und prüft, dass nur eine begrenzte Zahl Zeilen im DOM steht (zum Beispiel höchstens Bildschirmzeilen plus Puffer, also unter 300), und dass beim Sortieren die Zahl der erzeugten Zeilen konstant bleibt. Das ist deterministisch, kostet Sekunden und fängt genau die Regression ab, die hier gemessen wurde. Ein zweiter Zähltest: `sortEntries` ruft den Vergleich höchstens c mal n log n mal auf (Aufrufe zählen statt Zeit).
- Der Test auf die Zeilenzahl schlägt heute fehl; er gehört in denselben Umbau wie die Virtualisierung, nicht vorher.

## Nicht gemessen und Grenzen

- **SFTP und SMB**: nicht gemessen. Es braucht dafür Verbindungsprofile, Hostschlüssel-Bestätigung und Anmeldung in der Oberfläche; der Aufwand wäre unverhältnismäßig, und das Ergebnis wäre durch denselben Render-Engpass (50.000 Zeilen) dominiert. Der Listenabruf im Netz (Zeit des Auflistens, Zahl der Server-Antworten) bleibt offen.
- **Kalter Dateisystem-Cache**, **Scrollen mit echter GPU**: nicht gemessen (kein Root zum Cache-Leeren; VM ohne GPU, Software-Rendering, dadurch ist die Bildrate absolut nicht auf echte Rechner übertragbar, nur der Unterschied 500 / 10k / 50k).
- **Aufnahmeaufwand**: Die Bildschirmaufnahme verbraucht in der VM einen Kern; der Web-Prozess ist ohnehin einthreadig gebunden, ein Einfluss auf die Größenordnung ist unwahrscheinlich, auf die Sekunden schon.
- **Zeit bis bedienbar** ist nur als Antwort auf Probetasten und im 2-Sekunden-Raster bestimmt; Aussetzer von etwa 10 s Länge bei 64 bis 74 s und 94 bis 102 s wurden in einem Lauf beobachtet, ihre Ursache (Garbage Collection, Layout) ist nicht geklärt.
- **Node-Messung** (Abschnitt A2) läuft in V8, die App in WebKitGTK (JavaScriptCore); der Vergleich mit `Intl.Collator` kann dort anders ausfallen.
- Es gab nur einen Rechner und eine VM-Konfiguration; die ersten 33 s (gegenüber 46 s) liefen unmittelbar nach dem Start der VM.
