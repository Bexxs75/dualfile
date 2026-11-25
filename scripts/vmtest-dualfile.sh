#!/usr/bin/env bash
# shellcheck disable=SC2015  # ok/bad liefern immer 0, A && B || C ist hier gewollt
# Kurz-VM-Test für ein Dualfile-AppImage (Entwurf aus der CI, nie lokal gebaut: glibc der VM).
# Läuft AUF ObiWan (Linux-Test-VMs, siehe docs/release-process.md):
#   scripts/vmtest-dualfile.sh <kubuntu-kde|ubuntu-gnome> /pfad/Dualfile-X.Y.Z-x86_64.AppImage [Ergebnisordner]
# Prüft: AppImage startet und zeigt ein Fenster (Screenshot), zweiter Start übergibt an die laufende
# Instanz (Single-Instance, genau ein Prozess), Fenster schließen (Alt+F4 beendet den Prozess; Tray-Symbol ist in der Sandbox aus, siehe #40).
# GNOME/Wayland-Umgebung (DISPLAY=:0, mutter-Xauthority, XDG_SESSION_TYPE) wird im Gast gesetzt; ein altes
# Flatpak-Dualfile wird beendet und sein Autostart beiseite gelegt; ein Schluesselbund-Dialog wird weggeklickt.
# Die App läuft mit leerem XDG-Ordner (nie gegen echte Daten). Danach wird die VM heruntergefahren
# und das belegt. Nicht geprüft: Update von der Vorversion, Remote-Protokolle, Terminal, Autostart.
# Hinweis: Das Gast-Skript wird bei jedem Lauf neu kopiert; der Skriptlauf selbst war in keiner VM getestet (siehe Issue #58).
set -u
VM=${1:?kubuntu-kde|ubuntu-gnome}; IMG=${2:?AppImage}; OUT=${3:-./vmtest-$VM-$(date +%Y%m%d-%H%M%S)}
VMSH=${VMSH:-/mnt/Daten2/vms/linux/vm.sh}
FAILS=0
ok(){ echo "OK    [$VM] $*"; }
bad(){ echo "FAIL  [$VM] $*"; FAILS=$((FAILS+1)); }
vm(){ "$VMSH" "$VM" "$@" </dev/null; }   # stdin schließen: ssh/scp würden sonst das eigene Skript fressen
mkdir -p "$OUT"; [ -f "$IMG" ] || { echo "AppImage fehlt: $IMG"; exit 2; }

# Gast-Skript als Datei (nicht per Heredoc an ssh): ein Schritt je Aufruf, Ausgabe = Ergebnis.
GUEST=$(mktemp); trap 'rm -f "$GUEST"' EXIT
cat > "$GUEST" <<'GUESTEOF'
#!/bin/bash
export XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
[ -S /run/user/1000/wayland-0 ] && export WAYLAND_DISPLAY=wayland-0 XDG_SESSION_TYPE=wayland
MUTTER_AUTH=$(ls /run/user/1000/.mutter-Xwaylandauth.* 2>/dev/null | head -1)
if [ -n "$MUTTER_AUTH" ]; then
  # GNOME/Wayland (Ubuntu): Xwayland laeuft auf :0 mit mutter-Auth; ohne diese Umgebung (und mit
  # DISPLAY=:1/xauth_*) erscheint das Fenster nicht (0.4.5-VM-Test, Issue #58).
  export DISPLAY=:0 XAUTHORITY=$MUTTER_AUTH XDG_CURRENT_DESKTOP=ubuntu:GNOME
else
  # KDE (Wayland, Auth-Datei xauth_*)
  export DISPLAY=:1 XAUTHORITY=$(ls /run/user/1000/xauth_* 2>/dev/null | head -1)
fi
SB=/home/tester/vmtest-sandbox; IMG=/home/tester/Dualfile.AppImage
export XDG_DATA_HOME=$SB/data XDG_CONFIG_HOME=$SB/config XDG_CACHE_HOME=$SB/cache
pids(){ pgrep -x dualfile | tr '\n' ' '; }
case "$1" in
  clean)
    # Ein altes Flatpak-Dualfile (Autostart der VM, App-ID com.dualfileapp.Dualfile) haelt sonst die
    # Single-Instance und verdeckt den Test: beenden und seine Autostart-Dateien beiseite legen.
    flatpak kill com.dualfileapp.Dualfile >/dev/null 2>&1
    mkdir -p "$HOME/.config/autostart.vmtest-aus"
    for f in "$HOME"/.config/autostart/*[Dd]ualfile*.desktop; do [ -e "$f" ] && mv -f "$f" "$HOME/.config/autostart.vmtest-aus/"; done
    pkill -9 -x dualfile; sleep 1; rm -rf "$SB"; mkdir -p "$SB"; echo "pids=[$(pids)]" ;;
  keyring) # Schluesselbund-Dialog ("Authentication required", gcr-prompter) bei Autologin: 1 = Dialog offen
    pgrep -f gcr-prompter >/dev/null && echo "dialog=1" || echo "dialog=0" ;;
  start) nohup "$IMG" --appimage-extract-and-run >/tmp/df1.log 2>&1 </dev/null & disown; sleep "${2:-12}"; echo "pids=[$(pids)]" ;;
  second) timeout 30 "$IMG" --appimage-extract-and-run >/tmp/df2.log 2>&1 </dev/null; echo "exit=$?"; sleep 2; echo "pids=[$(pids)]" ;;
  state) echo "pids=[$(pids)]" ;;
esac
GUESTEOF

val(){ vm ssh "bash /home/tester/vmtest-guest.sh $*" 2>&1; }
# Anzahl der PIDs aus genau EINEM pids=[...]-Datensatz; "?" wenn kein oder mehrere Datensätze (z. B. SSH-Abfrage fehlgeschlagen).
# Eine leere Liste "pids=[]" ist ein gültiger Datensatz und ergibt 0.
cnt(){ local rec; rec=$(grep -o 'pids=\[[^]]*\]' <<<"$1"); [ -n "$rec" ] && [ "$(wc -l <<<"$rec")" -eq 1 ] && tr -cd '0-9 ' <<<"${rec#pids=}" | wc -w || echo "?"; }

vm start | tail -1
for _ in $(seq 1 60); do vm ssh true >/dev/null 2>&1 && break; sleep 5; done
vm ssh true >/dev/null 2>&1 && ok "VM erreichbar (SSH)" || { bad "VM nicht erreichbar"; vm stop; exit 1; }
sleep 20   # Autologin-Sitzung braucht etwas länger als SSH
"$VMSH" "$VM" scp "$IMG" tester@127.0.0.1:/home/tester/Dualfile.AppImage </dev/null && "$VMSH" "$VM" scp "$GUEST" tester@127.0.0.1:/home/tester/vmtest-guest.sh </dev/null && ok "AppImage und Gast-Skript kopiert" || bad "scp fehlgeschlagen"
# Schluesselbund-Dialog (Autologin entsperrt den Login-Schluesselbund nicht) liegt sonst ueber dem Fenster:
# per VNC abbrechen (Escape, dann Cancel-Knopf des 1920x1080-Dialogs), solange ein gcr-prompter laeuft.
dismiss_keyring(){ local i r; for i in 1 2 3; do r=$(val keyring); grep -q 'dialog=1' <<<"$r" || { [ "$i" -gt 1 ] && ok "Schluesselbund-Dialog weggeklickt"; return 0; }; vm vnc "key:Escape" "wait:1" "click:855,648" "wait:2" >/dev/null 2>&1; done; echo "WARN  [$VM] Schluesselbund-Dialog noch offen (Screenshot pruefen)"; }
vm ssh 'chmod +x /home/tester/Dualfile.AppImage' >/dev/null || bad "chmod auf dem AppImage fehlgeschlagen"

r=$(val clean); [ "$(cnt "$r")" = 0 ] && ok "Ausgangslage: kein dualfile-Prozess, leere Sandbox" || bad "alter Prozess läuft: $r"
dismiss_keyring
r=$(val start 12); echo "      $r"
dismiss_keyring
[ "$(cnt "$r")" = 1 ] && ok "1. Start: genau ein dualfile-Prozess" || bad "1. Start: $r"
vm shot "$OUT/$VM-1-start.png" >/dev/null 2>&1 && [ -s "$OUT/$VM-1-start.png" ] && ok "Screenshot $OUT/$VM-1-start.png (Fenster mit Datei-Panes von Hand ansehen!)" || bad "Screenshot fehlgeschlagen"
r=$(val second); echo "      $(tr '\n' ' ' <<<"$r")"
grep -q 'exit=0' <<<"$r" && [ "$(cnt "$r")" = 1 ] && ok "Single-Instance: 2. Start endet mit Exit 0, weiter genau ein Prozess" || bad "Single-Instance: $r"
vm vnc "key:alt+F4" "wait:10" "shot:$OUT/$VM-2-nach-schliessen.png" >/dev/null 2>&1 && [ -s "$OUT/$VM-2-nach-schliessen.png" ] \
  && ok "Alt+F4 per VNC gesendet, Screenshot $OUT/$VM-2-nach-schliessen.png (Fenster muss weg sein, von Hand ansehen!)" \
  || bad "VNC-Aufruf (Alt+F4, zweiter Screenshot) fehlgeschlagen: Schließen nicht belegt"
r=$(val state)
# Das Fenster muss im Screenshot weg sein (von Hand ansehen). Die Sandbox hat Standardeinstellungen, also
# kein Tray-Symbol: Alt+F4 muss den Prozess beenden (gemessen 25.11.2026 mit 0.4.3 und 0.4.6 auf GNOME und
# KDE: unter 1 s). Nur mit eingeschaltetem Tray-Symbol und vorhandenem StatusNotifier-Host (z. B. Ubuntu-GNOME
# mit AppIndicator-Erweiterung) läuft Dualfile gewollt im Tray weiter (#40) – das prüft dieser Test nicht.
c=$(cnt "$r")
if [ "$c" = "?" ]; then bad "Prozessstand nach Alt+F4 unbekannt (kein gültiger pids-Datensatz, SSH-Abfrage fehlgeschlagen?): $r"
elif [ "$c" = 0 ]; then ok "Fenster schließen (Alt+F4): Prozess beendet"
else bad "Prozess läuft nach Alt+F4 weiter, obwohl das Tray-Symbol in der Sandbox aus ist: $r"; fi

# Aufräumen und belegen: App beenden, VM herunterfahren, Status zeigen.
val clean >/dev/null
vm stop | tail -1
st=$("$VMSH" "$VM" status </dev/null)
grep -q 'gestoppt' <<<"$st" && ok "VM heruntergefahren: $st" || bad "VM läuft noch: $st"
echo "== $FAILS FAIL  (Ergebnisse in $OUT)"; [ "$FAILS" -eq 0 ]
