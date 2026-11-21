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

The check script runs each available tool, reports missing tools and exits nonzero
on missing tools or validation failure. Equivalent individual commands:

```sh
desktop-file-validate packaging/flatpak/com.dualfileapp.Dualfile.desktop
appstreamcli validate --strict --no-net packaging/flatpak/com.dualfileapp.Dualfile.metainfo.xml
```

## Later build on ObiWan / Späterer Bau auf ObiWan

**DE:** Die folgenden Befehle sind nur dokumentiert, nicht ausgeführt.
`flatpak-builder` ist auf ObiWan NICHT installiert. Seine Installation ist eine
Systemänderung und benötigt vorherige Freigabe. Auch SDK-/Runtime-Installation,
Generatorbeschaffung, Downloads, lokales Installieren und Einreichen sind separate
spätere Schritte. Der aktuelle Auftrag erlaubt weder Netzwerk noch Installation.

**EN:** Commands below are instructions for later, not executed work.
ObiWan does NOT have flatpak-builder installed. Obtain explicit approval before
that system change. Acquiring tools, SDKs/runtimes and dependency downloads,
installing the app and submitting it are outside this offline task.

1. TODO: aktuell stabile GNOME-Version vor dem Bau prüfen. Set the same verified
   branch for `org.gnome.Platform` and `org.gnome.Sdk` via `runtime-version`.
   Verify compatible versions/branches of rust-stable and node20. The locked Vite
   requires Node **20.19 or later within Node 20** (or a supported newer major).
   If node20 is unavailable for that SDK, report the conflict and approve a
   replacement; do not silently substitute an extension. Pin generator versions
   for reproducibility and verify their CLI help before running the commands.
2. TODO: Verify GTK3/WebKitGTK 4.1 and native dependencies in the selected SDK and
   runtime, including the tray's libayatana-appindicator/libappindicator and the
   repo's documented libxdo requirement. Missing libraries need separate pinned
   source modules and checksums; none have been guessed here. Verify runtime
   `flatpak-spawn` and portal-aware `xdg-open` too. The manifest does not yet
   establish that these runtime helpers/libraries are present.
3. Prepare a clean source tree containing this packaging directory, the root
   package-lock.json, src-tauri/Cargo.lock, all local crates and vendor/trash-5.3.0.
   Exclude host node_modules, targets and local Cargo configuration. Replace
   `TODO_SOURCE_CHECKOUT` with its path **relative to this manifest**. Do not point
   at or edit the original checkout from the work order. For Flathub replace the
   dir source with `type: git`, `url: https://github.com/Bexxs75/dualfile` and an
   explicitly selected immutable `commit` (TODO, not invented here).
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
   flatpak-builder --user --install --force-clean build-flatpak packaging/flatpak/com.dualfileapp.Dualfile.yml
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

DE: Offene TODOs vollständig: GNOME-/SDK-/Erweiterungsversionen und Node-Eignung;
Generator-/Linterversionen und deren CLI/Cachelayout; native Bibliotheken und
Runtime-Helfer (gegebenenfalls zusätzliche gepinnte Module); saubere Quelle bzw.
Git-Commit; beide generierten Offline-Quellen; echte Paket-/Desktop-Tests und
Lint-Ergebnisse; OARS-Bestätigung; Screenshot-Upload/URLs; aktueller
Einreichungsprozess und zugeteilter Domain-Nachweis. Metadaten und Rechte sind
Entwurfsentscheidungen zur unabhängigen Prüfung, keine Freigabe.

EN: All outstanding assumptions/TODOs are listed above. No installation, build,
network access, upload, application, commit, push or publication is performed by
this preparation. This is advisory input for independent review.
