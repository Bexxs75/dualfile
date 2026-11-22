#!/usr/bin/env bash
# shellcheck disable=SC2015  # ok/bad liefern immer 0, A && B || C ist hier gewollt
# Kurz-VM-Test für ein Dualfile-AppImage (Entwurf aus der CI, nie lokal gebaut: glibc der VM).
# Läuft AUF ObiWan (Linux-Test-VMs, siehe docs/release-process.md):
#   scripts/vmtest-dualfile.sh <kubuntu-kde|ubuntu-gnome> /pfad/Dualfile-X.Y.Z-x86_64.AppImage [Ergebnisordner]
# Prüft: AppImage startet und zeigt ein Fenster (Screenshot), zweiter Start übergibt an die laufende
# Instanz (Single-Instance, genau ein Prozess), Fenster schließen (Alt+F4) beendet den Prozess.
# Die App läuft mit leerem XDG-Ordner (nie gegen echte Daten). Danach wird die VM heruntergefahren
# und das belegt. Nicht geprüft: Update von der Vorversion, Remote-Protokolle, Terminal, Autostart.
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
export XDG_RUNTIME_DIR=/run/user/1000 DISPLAY=:1 XAUTHORITY=$(ls /run/user/1000/xauth_* 2>/dev/null | head -1)
[ -S /run/user/1000/wayland-0 ] && export WAYLAND_DISPLAY=wayland-0
# GNOME/X11-Sitzung: Display und Auth ermitteln, falls :1 nicht stimmt
[ -z "${WAYLAND_DISPLAY:-}" ] && export DISPLAY=${DISPLAY:-:0}
SB=/home/tester/vmtest-sandbox; IMG=/home/tester/Dualfile.AppImage
export XDG_DATA_HOME=$SB/data XDG_CONFIG_HOME=$SB/config XDG_CACHE_HOME=$SB/cache
pids(){ pgrep -x dualfile | tr '\n' ' '; }
case "$1" in
  clean) pkill -9 -x dualfile; sleep 1; rm -rf "$SB"; mkdir -p "$SB"; echo "pids=[$(pids)]" ;;
  start) nohup "$IMG" --appimage-extract-and-run >/tmp/df1.log 2>&1 </dev/null & disown; sleep "${2:-12}"; echo "pids=[$(pids)]" ;;
  second) timeout 30 "$IMG" --appimage-extract-and-run >/tmp/df2.log 2>&1 </dev/null; echo "exit=$?"; sleep 2; echo "pids=[$(pids)]" ;;
  state) echo "pids=[$(pids)]" ;;
esac
GUESTEOF

val(){ vm ssh "bash /home/tester/vmtest-guest.sh $*" 2>&1; }
cnt(){ grep -o 'pids=\[[^]]*\]' <<<"$1" | tr -cd '0-9 ' | wc -w; }

vm start | tail -1
for _ in $(seq 1 60); do vm ssh true >/dev/null 2>&1 && break; sleep 5; done
vm ssh true >/dev/null 2>&1 && ok "VM erreichbar (SSH)" || { bad "VM nicht erreichbar"; vm stop; exit 1; }
sleep 20   # Autologin-Sitzung braucht etwas länger als SSH
"$VMSH" "$VM" scp "$IMG" tester@127.0.0.1:/home/tester/Dualfile.AppImage </dev/null && "$VMSH" "$VM" scp "$GUEST" tester@127.0.0.1:/home/tester/vmtest-guest.sh </dev/null && ok "AppImage und Gast-Skript kopiert" || bad "scp fehlgeschlagen"
vm ssh 'chmod +x /home/tester/Dualfile.AppImage' >/dev/null

r=$(val clean); [ "$(cnt "$r")" = 0 ] && ok "Ausgangslage: kein dualfile-Prozess, leere Sandbox" || bad "alter Prozess läuft: $r"
r=$(val start 12); echo "      $r"
[ "$(cnt "$r")" = 1 ] && ok "1. Start: genau ein dualfile-Prozess" || bad "1. Start: $r"
vm shot "$OUT/$VM-1-start.png" >/dev/null 2>&1 && ok "Screenshot $OUT/$VM-1-start.png (Fenster mit Datei-Panes von Hand ansehen!)" || bad "Screenshot fehlgeschlagen"
r=$(val second); echo "      $(tr '\n' ' ' <<<"$r")"
grep -q 'exit=0' <<<"$r" && [ "$(cnt "$r")" = 1 ] && ok "Single-Instance: 2. Start endet mit Exit 0, weiter genau ein Prozess" || bad "Single-Instance: $r"
vm vnc "key:alt+F4" "wait:4" "shot:$OUT/$VM-2-nach-schliessen.png" >/dev/null 2>&1
r=$(val state)
[ "$(cnt "$r")" = 0 ] && ok "Fenster schließen (Alt+F4): Prozess beendet" || bad "Prozess läuft nach Alt+F4 weiter: $r (Tray-Verhalten? Screenshot prüfen)"

# Aufräumen und belegen: App beenden, VM herunterfahren, Status zeigen.
val clean >/dev/null
vm stop | tail -1
st=$("$VMSH" "$VM" status </dev/null)
grep -q 'gestoppt' <<<"$st" && ok "VM heruntergefahren: $st" || bad "VM läuft noch: $st"
echo "== $FAILS FAIL  (Ergebnisse in $OUT)"; [ "$FAILS" -eq 0 ]
