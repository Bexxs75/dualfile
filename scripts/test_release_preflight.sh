#!/usr/bin/env bash
# shellcheck disable=SC2155,SC2015  # Testskript: export mit Befehlsersetzung und A && B || C sind hier gewollt
# Simulationstest für scripts/release-preflight.sh (Modi post/pre-Eingabeprüfung): lokales "origin", gh/curl als Attrappen.
set -u
HERE=$(cd "$(dirname "$0")" && pwd); T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
FAIL=0; t(){ if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1 (erwartet $2, war $3)"; FAIL=1; fi; }
git init -q --bare "$T/origin.git"; git init -q "$T/w"; cd "$T/w" || exit 2
git config user.email t@t; git config user.name t; git remote add origin "$T/origin.git"
mkdir -p scripts; cp "$HERE/release-preflight.sh" scripts/
git add -A; git commit -qm one; A=$(git rev-parse HEAD); git commit -q --allow-empty -m two; B=$(git rev-parse HEAD)
git tag -a v1.0.0 "$A" -m annotated; git tag v1.1.0 "$B"; git push -q origin v1.0.0 v1.1.0
mkdir "$T/bin"
cat > "$T/bin/gh" <<'GH'
#!/usr/bin/env bash
case "$*" in
  *"run list"*) echo "$STUB_RUNS" ;;
  *"release view"*) echo "${STUB_DRAFT:-false}" ;;
esac
GH
cat > "$T/bin/curl" <<'CURL'
#!/usr/bin/env bash
case "$*" in *"/latest/download/"*) b=${STUB_LATEST-}; ;; *) b=${STUB_VERSIONED-}; esac
[ "${STUB_CURL_FAIL:-0}" = 1 ] && exit 22
printf '%s' "$b"
CURL
chmod +x "$T/bin/"*
export PATH="$T/bin:$PATH" DUALFILE_REPO=x/y
run(){ scripts/release-preflight.sh "$@" >"$T/out" 2>&1; echo $?; }
J(){ echo "{\"version\":\"$1\"}"; }

# R3: bösartige Argumente enden mit Fehler, ohne Nebenwirkung
t "R3 Version mit Shell-Code abgelehnt" 2 "$(run post "1.1.0; touch $T/x; #")"
t "R3 Modus mit Shell-Code abgelehnt" 2 "$(run "post; touch $T/x" 1.1.0)"
t "R3 keine Nebenwirkung" no "$([ -e "$T/x" ] && echo yes || echo no)"
git tag -a v1.2.0 "$B" -m annotated2; git tag v1.3.0-1 "$B"; git push -q origin v1.2.0 v1.3.0-1
ok_env(){ export STUB_RUNS="$B:completed:success" STUB_LATEST="$(J 1.1.0)" STUB_VERSIONED="$(J 1.1.0)"; unset STUB_CURL_FAIL; }
ok_env
t "post grün: leichter Tag = HEAD" 0 "$(run post 1.1.0)"
J2(){ export STUB_LATEST="$(J "$1")" STUB_VERSIONED="$(J "$1")"; }
J2 1.2.0; t "R5 post grün: annotierter Tag wird zum Commit aufgelöst" 0 "$(run post 1.2.0)"
ok_env; t "R5 Tag zeigt auf anderen Commit (annotiert, A)" 1 "$(run post 1.0.0)"; grep -q 'zeigt auf' "$T/out" && echo "ok    Meldung nennt Abweichung" || { echo "FAIL  Meldung"; FAIL=1; }
t "R5 Tag fehlt auf origin" 1 "$(run post 1.4.0)"
ok_env; STUB_RUNS="$A:completed:success"; export STUB_RUNS
t "R5 Release-Lauf auf anderem Commit" 1 "$(run post 1.1.0)"
ok_env; export STUB_RUNS="$B:in_progress:"
t "R5 Release-Lauf nicht abgeschlossen" 1 "$(run post 1.1.0)"
ok_env; export STUB_LATEST="$(J 1.0.0)"
t "R4 stabil: veraltetes latest.json = FAIL" 1 "$(run post 1.1.0)"
ok_env; export STUB_LATEST="kein json"
t "R4 stabil: kaputtes JSON = FAIL" 1 "$(run post 1.1.0)"
ok_env; export STUB_CURL_FAIL=1
t "R4 stabil: Download-Fehler = FAIL" 1 "$(run post 1.1.0)"
ok_env; export STUB_DRAFT=true
t "post: Entwurf ist nicht veröffentlicht = FAIL" 1 "$(run post 1.1.0)"; unset STUB_DRAFT
ok_env; export STUB_LATEST="$(J 1.0.0)" STUB_VERSIONED="$(J 1.3.0-1)"
t "R4 Vorabversion: latest ignoriert, versioniertes Manifest zählt" 0 "$(run post 1.3.0-1)"
export STUB_VERSIONED="$(J 1.0.0)"
t "R4 Vorabversion: versioniertes Manifest falsch = FAIL" 1 "$(run post 1.3.0-1)"
exit $FAIL
