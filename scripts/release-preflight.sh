#!/usr/bin/env bash
# shellcheck disable=SC2015  # ok/bad/warn liefern immer 0, A && B || C ist hier gewollt
# Mechanische Release-Prüfung für Dualfile. Aufruf im Release-Worktree:
#   scripts/release-preflight.sh <pre|draft|post> X.Y.Z
#   pre   vor dem Tag/Build: Stand, CI auf genau diesem Commit, Versionen, CHANGELOG, Release-Notes
#   draft vor dem Veröffentlichen: pre + Entwurf (Lauf auf diesem Commit, Pakete, Signaturen, Prüfsummen)
#   post  nach dem Veröffentlichen: Tag, latest.json
# Jede Zeile OK/FAIL; bei einem FAIL endet das Skript mit Exit 1. Ohne Exit 0 weder taggen noch veröffentlichen.
set -u
MODE=${1:?pre|draft|post}; V=${2:?Version X.Y.Z}
REPO=${DUALFILE_REPO:-Bexxs75/dualfile}
FAILS=0
ok(){ echo "OK    $*"; }
bad(){ echo "FAIL  $*"; FAILS=$((FAILS+1)); }
warn(){ echo "WARN  $*"; }
chk(){ local d=$1; shift; if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi; }

cd "$(git rev-parse --show-toplevel)" || exit 2
BR=$(git rev-parse --abbrev-ref HEAD); SHA=$(git rev-parse HEAD)
echo "== Preflight $MODE v$V  Branch $BR  Commit ${SHA:0:8}"

versions_ok() {
  python3 - "$V" <<'PY'
import json, re, sys
v = sys.argv[1]; bad = []
def rd(p): return open(p, encoding="utf-8").read()
def chk(label, found):
    if found != v: bad.append(f"{label}={found!r}")
chk("tauri.conf.json", json.load(open("src-tauri/tauri.conf.json"))["version"])
m = re.search(r'\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', rd("Cargo.toml"), re.M | re.S)
chk("Cargo.toml", m and m.group(1))
m = re.search(r'\[package\][^\[]*?^version\s*=\s*"([^"]+)"', rd("src-tauri/Cargo.toml"), re.M | re.S)
chk("src-tauri/Cargo.toml", m and m.group(1))
for lock in ("Cargo.lock", "src-tauri/Cargo.lock"):
    pk = re.findall(r'^name = "(dualfile[^"]*)"\nversion = "([^"]+)"', rd(lock), re.M)
    if not pk: bad.append(f"{lock}: keine dualfile-Pakete")
    for n, f in pk: chk(f"{lock} {n}", f)
# npm-Paket ist bewusst unversioniert; beide Dateien müssen nur zusammenpassen
pj = json.load(open("package.json"))["version"]; pl = json.load(open("package-lock.json"))
if pl["version"] != pj or pl["packages"][""]["version"] != pj: bad.append("package.json/package-lock.json weichen ab")
print("; ".join(bad)); sys.exit(1 if bad else 0)
PY
}
if [ "$MODE" != post ]; then
  [ -z "$(git status --porcelain)" ] && ok "Arbeitsbaum sauber" || bad "ungesicherte Änderungen: $(git status --porcelain | wc -l) Dateien"
  git fetch -q origin "$BR" 2>/dev/null
  [ "$(git rev-parse "origin/$BR" 2>/dev/null)" = "$SHA" ] && ok "HEAD ist auf origin gepusht" || bad "HEAD nicht (vollständig) auf origin/$BR"
  # CI muss auf genau diesem Commit grün sein (nicht auf einem früheren); gleiche Regel wie im Gate von release.yml
  ci=$(gh run list --repo "$REPO" --workflow ci.yml --commit "$SHA" --json status,conclusion -q '.[]|"\(.status):\(.conclusion)"' 2>/dev/null)
  grep -qx 'completed:success' <<<"$ci" && ok "CI grün auf ${SHA:0:8}" || bad "CI auf ${SHA:0:8}: '${ci:-kein Lauf}'"
  out=$(versions_ok 2>&1) && ok "Versionen in allen Dateien = $V (tauri.conf, Cargo.toml, Cargo.lock x2, package-lock)" || bad "Versionen: $out"
  out=$(python3 scripts/check-release-notes.py "$V" 2>&1) && ok "CHANGELOG-Abschnitt und src/release-notes.ts-Eintrag $V vollständig (Datum, de, en, keine Platzhalter)" || bad "Release-Texte: $out"
  bash -c 'git tag -l "v$0" | grep -q .' "$V" && warn "Tag v$V existiert lokal schon"
fi

if [ "$MODE" = draft ]; then
  dr=$(gh release view "v$V" --repo "$REPO" --json isDraft -q .isDraft 2>/dev/null)
  [ "$dr" = true ] && ok "Entwurf v$V vorhanden" || bad "Entwurf v$V fehlt oder ist schon veröffentlicht ($dr)"
  run=$(gh run list --repo "$REPO" --workflow release.yml --commit "$SHA" --json conclusion,status -q '.[0]|"\(.status):\(.conclusion)"' 2>/dev/null)
  [ "$run" = "completed:success" ] && ok "Release-Lauf auf genau diesem Commit grün" || bad "Release-Lauf auf ${SHA:0:8}: '${run:-kein Lauf}'"
  names=$(gh release view "v$V" --repo "$REPO" --json assets -q '.assets[].name' 2>/dev/null)
  for a in x86_64 aarch64; do
    for ext in AppImage AppImage.sig tar.gz; do
      grep -qx "Dualfile-$V-$a.$ext" <<<"$names" && ok "Paket Dualfile-$V-$a.$ext" || bad "Paket Dualfile-$V-$a.$ext fehlt"
    done
  done
  for f in latest.json SHA256SUMS; do grep -qx "$f" <<<"$names" && ok "Datei $f" || bad "Datei $f fehlt"; done
  warn "Kurz-VM-Test (docs/release-process.md, scripts/vmtest-dualfile.sh) am Entwurf auf KDE und GNOME bestätigt? Das prüft dieses Skript nicht."
fi

if [ "$MODE" = post ]; then
  chk "Tag v$V auf origin" bash -c "git ls-remote --tags origin v$V | grep -q v$V"
  pub=$(gh release view "v$V" --repo "$REPO" --json isDraft -q .isDraft 2>/dev/null)
  [ "$pub" = false ] && ok "Release v$V ist veröffentlicht" || bad "Release v$V nicht veröffentlicht ('$pub')"
  lj=$(curl -sL "https://github.com/$REPO/releases/latest/download/latest.json" | python3 -c "import sys,json;print(json.load(sys.stdin)['version'])" 2>/dev/null)
  [ "$lj" = "$V" ] && ok "latest.json zeigt $V" || warn "latest.json zeigt '${lj}' (bei Vorabversion normal)"
  warn "Homepage/Wiki-Version, Forgejo-Issues schließen, Vault: von Hand abhaken"
fi
echo "== $FAILS FAIL"; [ "$FAILS" -eq 0 ]
