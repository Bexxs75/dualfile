# Flatpak draft / Flatpak-Entwurf

**DE:** Stufe 3, nur Vorbereitung zu Forgejo #30. Nicht baubar, bevor die unten
aufgeführten TODOs erledigt sind. Keine Veröffentlichung oder Flathub-Freigabe.
Alle Pfade beziehen sich auf diesen Checkout. Die beiden Skripte liegen bewusst
unter `packaging/flatpak/scripts/`, entsprechend „alles unter packaging/flatpak“.
Der vollständige `FLATPAK-PLAN.md` liegt hier nicht vor; Grundlage ist der
übergebene eingefrorene Auftrag und `docs/plans/flatpak-stage2-result.md`.

**EN:** Stage 3 preparation only. Resolve the TODOs below before building. This is
not a published package or Flathub approval. All paths refer to this checkout.
Scripts are nested here to keep every new file under `packaging/flatpak/`.
The full `FLATPAK-PLAN.md` is absent; the frozen work order and stage 2 handoff
are the available specification.

## Metadata / Metadaten (offline)

Run from the checkout root / Im Wurzelverzeichnis dieses Checkouts ausführen:

```sh
node packaging/flatpak/scripts/flatpak-releases.mjs > /tmp/dualfile-flatpak-releases.xml
packaging/flatpak/scripts/flatpak-check.sh
```

DE: Das Node-Skript benötigt keine Pakete. Es gibt die letzten drei datierten
Versionen in Changelog-Reihenfolge als XML aus (aktuell 0.4.1, 0.4.0, 0.3.10).
Zum Aktualisieren ausschließlich den Block zwischen `BEGIN GENERATED RELEASES`
und `END GENERATED RELEASES` in der metainfo durch diese Ausgabe ersetzen.
Es verändert weder Changelog noch App-Version. Die Daten belegen keine externe
Veröffentlichung. Englische Release-Texte stammen aus CHANGELOG.md; DE/EN-Apptexte
sind aus README.md abgeleitet, der Entwicklername aus LICENSE. Die leere OARS-1.1-
Bewertung beschreibt einen Dateimanager ohne eigene bewertungspflichtige Inhalte;
diese Einstufung vor Einreichung bestätigen.

EN: The dependency-free Node script emits the latest three dated changelog entries
in source order. Replace only the marked generated block in metainfo with its
output. It does not change app versions or claim that these releases were
published. Descriptions derive from README.md; the developer name derives from
LICENSE. Confirm the empty OARS content rating before submission.

DE: Das Prüfskript führt alle verfügbaren Metadatenprüfer aus und prüft mit
Python 3/PyYAML das Manifest auf Quellplatzhalter, veraltete Node-Erweiterung und
fehlende Pflichtausschlüsse. Fehlende Werkzeuge oder Prüffehler ergeben Exit != 0.

EN: The check script runs each available metadata validator and uses Python 3 /
PyYAML to check the manifest for source placeholders, the obsolete Node extension
and missing required source exclusions. Missing tools or failed checks yield a
nonzero exit. Equivalent individual metadata commands:

```sh
desktop-file-validate packaging/flatpak/com.dualfileapp.Dualfile.desktop
appstreamcli validate --strict --no-net packaging/flatpak/com.dualfileapp.Dualfile.metainfo.xml
```

## Later build on ObiWan / Späterer Bau auf ObiWan

**DE:** Die folgenden Baubefehle sind nur dokumentiert, nicht ausgeführt.
Laut Koordinator läuft auf ObiWan flatpak-builder **1.4.12** im Benutzerbereich
als `flatpak run --user org.flatpak.Builder`. SDK/Runtime und Erweiterungen sind
dort installiert. Generatorbeschaffung, Downloads, lokales Installieren und
Einreichen bleiben spätere Schritte; dieser Auftrag erlaubt kein Netzwerk,
keine Installation und keinen Builder-Aufruf.

**EN:** Build commands below are instructions for later, not executed work.
The coordinator verified flatpak-builder **1.4.12** on ObiWan in the user
installation, invoked as `flatpak run --user org.flatpak.Builder`, with SDK/runtime
and extensions installed. Acquiring generators, downloading dependencies,
installing the app and submitting it remain outside this offline task.

1. **DE – geprüft durch den Koordinator:** `org.gnome.Platform//50` und
   `org.gnome.Sdk//50` (50.6, freedesktop-Basis 26.08), dazu `rust-stable//26.08`
   und freigegebenes `node22//26.08`. Node 22 erfüllt Vites Anforderung >=20.19.
   TODO: Generatorversionen festlegen und deren CLI-Hilfe prüfen.

   **EN – coordinator verified:** GNOME Platform/SDK 50 (50.6, freedesktop 26.08),
   rust-stable and approved node22 on branch 26.08. Node 22 meets Vite's >=20.19
   requirement. TODO: pin generator versions and verify their CLI help.
2. **DE – geprüft durch den Koordinator:** SDK 50 enthält WebKitGTK 4.1 2.54.1,
   GTK3 3.24.52, libsoup-3.0, librsvg, OpenSSL 3.5.9, libsecret und dbus. Die Runtime
   enthält `flatpak-spawn` und `xdg-open`. libayatana-appindicator3,
   libappindicator3 und libxdo fehlen. `libappindicator-sys` lädt per Dlopen zur
   Laufzeit; das Tauri-Lockfile enthält kein libxdo-Crate. Diese Bibliotheken
   blockieren den Bau nicht. Das optionale, standardmäßig ausgeschaltete Tray
   bleibt in dieser Stufe ohne Bibliothek; keine zusätzlichen Module.
   TODO Folgearbeit: Tray-Bibliothek gezielt paketieren und Laufzeit testen.

   **EN – coordinator verified:** SDK 50 includes WebKitGTK 4.1 2.54.1, GTK3
   3.24.52, libsoup-3.0, librsvg, OpenSSL 3.5.9, libsecret and dbus; the runtime
   includes `flatpak-spawn` and `xdg-open`. The missing appindicator libraries
   are loaded at runtime by `libappindicator-sys`; no libxdo crate occurs in the
   Tauri lockfile. They do not block compilation. The optional tray is off by
   default and has no library in this stage; no extra modules are added.
   TODO follow-up: package a tray library deliberately and test it at runtime.
3. **DE – erledigt:** Die `dir`-Quelle zeigt relativ zum Manifest auf `../..`.
   Ausgeschlossen sind `.git`, `node_modules`, `src-tauri/target`, `target`,
   `target-tauri`, `dist`, `build-flatpak`, `.flatpak-builder`, `.cargo` und
   `src-tauri/.cargo`. Enthalten bleiben die npm-Dateien, Frontend-Quellen,
   Root-Workspace-Manifest, Tauri-Manifest/Lockfile, alle vier lokalen Crates
   (`dualfile-core`, `dualfile-platform`, `dualfile-remote`, `dualfile-plugin`),
   `vendor/trash-5.3.0`, Desktop-/AppStream-Dateien und `src-tauri/icons`.
   Lokale Cargo-Konfiguration liegt an den beiden ausgeschlossenen Stellen
   derzeit nicht vor. Die untenstehenden Metadata-Befehle liefen offline mit
   Exit 0; auch die vollständige Auflösung gelang mit dem vorhandenen Host-Cache.
   Das beweist noch keinen Flatpak-Bau mit den ausstehenden Quellenlisten.

   **EN – complete:** The manifest-relative `dir` source is `../..`, excluding
   the host files/directories listed above while retaining npm/frontend inputs,
   both workspace manifests, the Tauri lockfile, all four local crates, vendored
   trash, packaging metadata and icons. Neither excluded Cargo configuration
   directory currently exists. Both metadata commands below passed offline
   (exit 0), including full resolution using the existing host cache. This does
   not prove the Flatpak build with the still-missing generated source lists.

   Offline evidence commands, run from this checkout / ausgeführte Belege:

   ```sh
   rg -n 'path\s*=|patch|replace' --glob Cargo.toml
   cargo metadata --offline --locked --no-deps --format-version 1 --manifest-path src-tauri/Cargo.toml
   cargo metadata --offline --locked --format-version 1 --manifest-path src-tauri/Cargo.toml
   ```

   **DE:** TODO für Flathub: auf `type: git` mit öffentlicher URL
   `https://github.com/Bexxs75/dualfile` und ausdrücklich ausgewähltem,
   unveränderlichem Commit umstellen. Kein Commit wird hier erfunden.
   **EN:** TODO for Flathub: replace the dir source with `type: git`, that public
   URL and an explicitly selected immutable commit; none is invented here.
4. Generate sources after downloads/tool acquisition are separately authorized.
   From this checkout root, with `flatpak-cargo-generator.py` and
   `flatpak-node-generator` installed on PATH, the commands are:

   ```sh
   flatpak-cargo-generator.py src-tauri/Cargo.lock -o packaging/flatpak/generated-sources.json
   flatpak-node-generator npm package-lock.json -o packaging/flatpak/node-sources.json
   ```

   The upstream cargo generator is also commonly invoked as
   `python3 /path/to/flatpak-cargo-generator.py` with the same arguments; replace
   that example path with the reviewed tool location. Use the **Tauri lockfile**,
   not the separate root workspace lockfile. Do not disable npm development
   dependencies: TypeScript and Vite are needed to build the frontend.
   TODO: inspect generated JSON against the pinned generator version: Cargo
   configuration/vendor sources must be under `cargo/`, npm cache under
   `flatpak-node/npm-cache`, matching manifest CARGO_HOME/npm_config_cache.
   Check local path dependencies remain in the source tree and all platform
   dependencies/lifecycle-script downloads are covered. The generators may use
   the network; their output files are deliberately absent in this draft.
5. After all TODOs are resolved, dependency sources are downloaded and the SDK is
   installed, build and install locally (explicitly authorized later):

   ```sh
   flatpak run --user org.flatpak.Builder --user --install --force-clean build-flatpak packaging/flatpak/com.dualfileapp.Dualfile.yml
   flatpak run com.dualfileapp.Dualfile
   ```

   `build-flatpak` must be a disposable build directory: `--force-clean` clears
   it. Builder fetches source archives before the sandboxed build; npm and Cargo
   build commands are offline. For a repeat build with sources already cached,
   add `--disable-download`. Cargo runs in `src-tauri/`, whose workspace is
   independent, with `tauri/custom-protocol` to embed the built frontend.
   No Tauri CLI bundle build or AppImage packaging is involved.
6. Validate metadata with the offline commands above and, once installed:

   ```sh
   flatpak-builder-lint manifest packaging/flatpak/com.dualfileapp.Dualfile.yml
   flatpak-builder-lint builddir build-flatpak
   ```

   TODO: verify these commands against the chosen linter version and review all
   findings. A valid YAML document alone does not prove builder/Flathub validity.

DE: Anschließend unter KDE und GNOME testen: Start/Fensterkennung/Icons,
Dateizugriff und Papierkorb (wiederherstellen/leeren), Netzwerkverbindungen und
SSH-Agent, Schlüsselbund, optionales Tray, Terminal und Geräteaktionen,
Autostart inklusive minimiertem Start und Portal-Ablehnung sowie Öffnen externer
Dateien/URLs. Prüfen, dass keine AppImage-Updater-/Startmenü-Aktionen erscheinen.
Keine produktiven Dateien für Löschtests verwenden.

EN: Test on KDE and GNOME: launch/window identity/icons, files and trash, remote
connections/SSH agent, credential persistence, optional tray, terminal and device
helpers, background autostart including minimized launch/denied portal prompts,
and external file/URL opening. AppImage update/menu actions should be absent.
Use disposable data for destructive tests.

## Reviewer questions / Prüferfragen

| Permission | Reason / Begründung and review question |
| --- | --- |
| `--filesystem=host` | Dateimanager für lokale Ordner, Datenträger und Papierkorb. File-manager functionality needs user-accessible host paths; Flatpak's reserved/private paths remain restricted. Do actual mount/trash tests pass? |
| `--talk-name=org.freedesktop.Flatpak` | Stufe 2 startet Terminal und Gerätehelfer über `flatpak-spawn --host`. Broad host execution is intentional and needs explicit Flathub review; can later code narrow this? |
| `--talk-name=org.freedesktop.secrets` | Gespeicherte Zugangsdaten über Secret Service. Does credential storage work on both GNOME and KDE without granting further bus names? |
| `--talk-name=org.kde.StatusNotifierWatcher` | Optionales Tray. Does registration work on supported desktops (GNOME may require an extension)? |

DE: UDisks2 bleibt auskommentiert, weil Geräteaktionen über Host-Helfer laufen.
`--talk-name` würde zudem den Session-Bus freigeben, UDisks2 liegt am System-Bus.
Background/OpenURI verwenden den von Flatpak standardmäßig erreichbaren
Desktop-Portal-Dienst; keine zusätzliche Busfreigabe. Kein pauschaler Session-
oder System-Bus und kein zusätzlicher IPC-Zugriff. Die übrigen Berechtigungen
sind einzeln im Manifest begründet. Ihre Laufzeitabnahme steht aus.

EN: UDisks2 is commented out: stage 2 uses host helpers, and a session-bus grant
would target the wrong bus. Background/OpenURI use standard Flatpak portal
access. Other permissions are explained individually in the manifest; runtime
acceptance remains outstanding.

## Submission description only / Einreichung nur beschrieben

1. Resolve all TODOs above, validate/lint and independently review the final
   source revision, permissions, licenses and real sandbox tests.
2. TODO: Upload approved screenshots to dualfileapp.com in a separately approved
   action; replace the metainfo comment with real screenshot entries/URLs.
3. Follow the then-current Flathub new-app contribution process: prepare the
   app-ID repository/manifest contribution, submit it after user approval, and
   answer reviewers' permission and reproducibility questions. Nothing is sent
   by this draft.
4. Request domain verification through Flathub. Flathub assigns the exact content
   **after the application**; do not invent a token or upload a placeholder.
   The requested proof location is
   `https://dualfileapp.com/.well-known/org.flathub.VerifiedApps.txt`.
5. After approval, arrange separately authorized publication and verify the
   listing, screenshots, metadata and updates.

DE: Offene TODOs: Generator-/Linterversionen und deren CLI/Cachelayout;
beide generierten Offline-Quellen einschließlich aller Plattformabhängigkeiten;
Git-Commit für Flathub; echte Paket-/Desktop-Tests und Lint-Ergebnisse;
Tray-Unterstützung als Folgearbeit; OARS-Bestätigung; Screenshot-Upload/URLs;
aktueller Einreichungsprozess und zugeteilter Domain-Nachweis. Metadaten und
Rechte sind Entwurfsentscheidungen zur unabhängigen Prüfung, keine Freigabe.

EN: Remaining TODOs: generator/linter versions and CLI/cache layout; both generated
offline source lists including platform dependencies; immutable Flathub commit;
real package/desktop tests and lint results; tray support as follow-up; OARS
confirmation; screenshot upload/URLs; current submission process and assigned
domain proof. No installation, build, network access, upload, application, commit,
push or publication is performed by this preparation. This is advisory input for
independent review.
