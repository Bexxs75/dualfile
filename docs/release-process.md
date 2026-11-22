# Release-Prozess Dualfile

Gilt für jede Version (Fix und Funktion). Ergänzt `docs/release-checklist.md` (Texte, Tests, Freigabe). Ein Release wird nur veröffentlicht, wenn jeder Schritt unten belegt ist (Befehl plus Ausgabe), nicht nur behauptet.

## Ablauf

1. **Vorbereiten** auf dem Release-Zweig: Version in `src-tauri/tauri.conf.json`, `Cargo.toml` (`[workspace.package]`), `src-tauri/Cargo.toml` und beiden Lock-Dateien (`Cargo.lock`, `src-tauri/Cargo.lock`) setzen; `CHANGELOG.md`-Abschnitt `## [X.Y.Z] - Datum` und `src/release-notes.ts` (Datum, `de`, `en`) schreiben. `package.json` bleibt bewusst auf `0.0.0`, beide npm-Dateien müssen nur zueinander passen.
2. **Pushen und CI abwarten:** Der Commit muss auf GitHub liegen und der Workflow `CI` (`ci.yml`) muss auf genau diesem Commit grün sein.
3. **Preflight `pre`:** `scripts/release-preflight.sh pre X.Y.Z` im Release-Worktree. Jede Zeile OK, sonst Exit 1 und kein Tag.
4. **Tag `vX.Y.Z` pushen.** Der Workflow `release.yml` startet.
5. **Gate (Job `verify`)** bricht vor dem Bauen ab, wenn eines fehlt: erfolgreicher CI-Lauf auf `github.sha`; Version in `tauri.conf.json`, `Cargo.toml`, `src-tauri/Cargo.toml`, `Cargo.lock`, `src-tauri/Cargo.lock` gleich dem Tag; `package.json` und `package-lock.json` zueinander passend; nicht leerer CHANGELOG-Abschnitt ohne Platzhalter (TODO, TBD, FIXME ...); `release-notes.ts` mit Datum sowie nicht leeren, platzhalterfreien `de`/`en`-Texten. Danach baut `build` (x86_64, aarch64, signiert), `assemble` legt nur einen **Entwurf** an (nie ein veröffentlichtes Release).
6. **Preflight `draft`:** `scripts/release-preflight.sh draft X.Y.Z` prüft Entwurf, Release-Lauf auf diesem Commit, beide AppImages, Signaturen, Tarballs, `latest.json`, `SHA256SUMS`.
7. **Kurz-VM-Test** (unten) auf KDE und GNOME am AppImage aus dem Entwurf.
8. **Zweiter Blick** (Codex oder Sonnet) auf Diff, Texte und Testergebnis; dann erst veröffentlichen: `gh release edit vX.Y.Z --draft=false`.
9. **Preflight `post`:** `scripts/release-preflight.sh post X.Y.Z`; danach Homepage/Wiki-Version, Forgejo-Issues schließen.

## Kurz-VM-Test

Das AppImage kommt **aus dem Entwurf** (`gh release download vX.Y.Z --repo Bexxs75/dualfile --pattern '*x86_64.AppImage'`), nie lokal gebaut (glibc der VM). Die Linux-Test-VMs laufen auf ObiWan (`ssh obiwan-claude`, `/mnt/Daten2/vms`); Datei in `/mnt/Daten2/vms/austausch/` ablegen und dort je VM ausführen (ein VM zur Zeit, die Fremdshell ist fish, daher `bash datei`):

    bash /mnt/Daten2/vms/austausch/vmtest-dualfile.sh kubuntu-kde  /mnt/Daten2/vms/austausch/Dualfile-X.Y.Z-x86_64.AppImage
    bash /mnt/Daten2/vms/austausch/vmtest-dualfile.sh ubuntu-gnome /mnt/Daten2/vms/austausch/Dualfile-X.Y.Z-x86_64.AppImage

(`scripts/vmtest-dualfile.sh` aus dem Repo vorher dorthin kopieren.) Das Skript startet die VM, kopiert das AppImage, startet es mit leerem XDG-Ordner und prüft: genau ein Prozess `dualfile`; Screenshot des Fensters; zweiter Start endet mit Exit 0 und es bleibt bei einem Prozess (Single-Instance); Alt+F4 schließt das Fenster (Screenshot) und beendet den Prozess, sonst Warnung als Befund (unter GNOME mit 0.4.3 bleibt der Prozess ohne Fenster bestehen, unter KDE endet er); zuletzt fährt es die VM herunter und belegt das (`vm.sh <vm> status` = gestoppt). Pro VM Exit 0 und Screenshots ansehen: Fenster zeigt zwei Datei-Panes, nach Alt+F4 ist es weg.

Bei Änderungen an Updater, Datenformat, Remote-Protokollen oder Terminal reicht das nicht: zusätzlich von Hand testen (Update von der Vorversion, betroffene Funktion; Hinweise in `docs/release-checklist.md`). KDE nutzt Wayland; X11- und Autostart-Fälle sind nicht Teil des Kurztests. Stolpersteine: der Prozess heißt klein `dualfile`; eine noch laufende Instanz lässt neue Starts mit Exit 0 enden.

## Werkzeug

- `scripts/release-preflight.sh <pre|draft|post> X.Y.Z` (siehe oben), Voraussetzung `gh` angemeldet.
- `scripts/vmtest-dualfile.sh <vm> <AppImage> [Ordner]`.
