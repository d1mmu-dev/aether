#!/usr/bin/env bash
# Isolated stack for the visual comparison of aether-web against the upstream kima-hub Next.js frontend: ONE backend
# (aether-server + its own throw-away Docker postgres), ONE seeded library, and both frontends in front of it.
#
#   tools/run-aether-web-visual.sh up       start everything that is not running (idempotent), print the env
#   tools/run-aether-web-visual.sh reload   rebuild aether-web (vertigo build) and restart ONLY aether-server
#   tools/run-aether-web-visual.sh compare [args]   css --check, `up`, `reload`, run tools/compare-aether-web-visual.mjs, leave the stack up
#   tools/run-aether-web-visual.sh run [args]       `compare`, then `down`
#   tools/run-aether-web-visual.sh down     stop aether/next and remove the postgres container + state
#
# The reference is an EXTERNAL kima-hub checkout (this repository no longer bundles the Next.js app, D-180).
# `up` runs tools/prepare-kima-hub-reference.sh --install first: clean-tree check, canonical upstream remote and
# branch, `git pull --ff-only`, HEAD == upstream, provenance recorded, `frontend/` verified against (or re-extracted
# from) the git objects into an isolated copy whose dependencies are installed there (never in the checkout).
# Next.js runs ONLY here, as the visual reference, from the copy's own node_modules/.bin/next (never npx); it is not
# part of any product, dev or CI path. The reference dev server is restarted when the SHA, the copy path or the
# copy's content manifest changes, or when the copy had been modified and was restored. compare refuses to run
# without that provenance and writes it (SHA, remote URL, branch, pulled, relation, tree, manifest) into report.json.
# Needs: cargo, node/npm (reference copy only), docker (pgvector/pgvector:pg17 on a random 127.0.0.1 port),
# ffmpeg, vertigo 0.14.5 (+ wasm-opt), Chrome.
# Environment, or the private tools config (tools/tools-env.inc.sh): AETHER_UPSTREAM_REFERENCE (required),
# AETHER_REFERENCE_COPY, VISUAL_DIR (default ${XDG_CACHE_HOME:-~/.cache}/aether/visual), VERTIGO (default
# `vertigo` on PATH), FFMPEG (default `ffmpeg` on PATH), AETHER_TOOLS_PATH (prepended to PATH), CHROME.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=tools/tools-env.inc.sh
. "$root/tools/tools-env.inc.sh"
dir="${VISUAL_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/aether/visual}"
vertigo="${VERTIGO:-vertigo}"
ffmpeg="${FFMPEG:-ffmpeg}"
name="aether-visual-pg-$$"
state="$dir/state.env"
mkdir -p "$dir"

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
alive() { [ -n "${1:-}" ] && kill -0 "$1" 2>/dev/null; }
load() { [ -f "$state" ] && . "$state" || true; }
save() {
  cat >"$state" <<EOF
PG_NAME=$PG_NAME
PG_PORT=$PG_PORT
AETHER_PORT=$AETHER_PORT
SSR_PORT=$SSR_PORT
LEGACY_PORT=$LEGACY_PORT
AETHER_PID=${AETHER_PID:-}
LEGACY_PID=${LEGACY_PID:-}
REF_SHA=${REF_SHA:-}
REF_FRONTEND=${REF_FRONTEND:-}
REF_MANIFEST=${REF_MANIFEST:-}
EOF
}

# Why the running reference dev server must be restarted (empty: keep it). Arguments: the SHA, copy frontend path and
# content manifest it was started on, then the freshly prepared ones, then AETHER_REFERENCE_COPY_RESTORED.
reference_restart_reason() {
  if [ "$1" != "$4" ]; then echo "reference moved to $4"
  elif [ "$2" != "$5" ]; then echo "reference copy path changed to $5"
  elif [ "$3" != "$6" ]; then echo "reference copy content changed"
  elif [ "$7" != no ]; then echo "reference copy had been modified and was restored from git"
  fi
}

# The reference copy's own Next.js binary; fails (never downloads via npx) when the copy or its install is missing.
reference_next_bin() {
  [ -d "$1" ] || { echo "reference frontend $1 does not exist" >&2; return 1; }
  [ -x "$1/node_modules/.bin/next" ] || { echo "no node_modules/.bin/next in $1: run tools/prepare-kima-hub-reference.sh --install" >&2; return 1; }
  echo "$1/node_modules/.bin/next"
}

start_aether() {
  AETHER_PID=""
  (cd "$root" && SQLX_OFFLINE=true cargo build -p aether-server)
  (
    cd "$root"
    DATABASE_URL="postgres://aether:aether@127.0.0.1:${PG_PORT}/aether" MUSIC_PATH="$dir/library" \
      AETHER_DATA_DIR="$dir/data" PORT="$AETHER_PORT" AETHER_BIND_HOST=127.0.0.1 AETHER_IDENTIFY=off \
      AETHER_WEB_BUILD_DIR=crates/aether-web/build AETHER_WEB_SSR_PORT="$SSR_PORT" \
      exec target/debug/aether >"$dir/aether.log" 2>&1
  ) &
  AETHER_PID=$!
  for _ in $(seq 1 240); do
    curl -fsS "http://127.0.0.1:${AETHER_PORT}/api/health" >/dev/null 2>&1 && return 0
    alive "$AETHER_PID" || { echo "aether exited during start, see $dir/aether.log" >&2; return 1; }
    sleep 1
  done
  echo "aether did not come up" >&2; return 1
}

stop_legacy() {
  if alive "${LEGACY_PID:-}"; then pkill -P "$LEGACY_PID" 2>/dev/null || true; kill "$LEGACY_PID" 2>/dev/null || true; fi
  # next dev leaves a child behind on its port.
  [ -n "${LEGACY_PORT:-}" ] && (fuser -k "${LEGACY_PORT}/tcp" >/dev/null 2>&1 || true)
  LEGACY_PID=""
}

stop_aether() {
  if alive "${AETHER_PID:-}"; then kill "$AETHER_PID" 2>/dev/null || true; for _ in $(seq 1 30); do alive "$AETHER_PID" || break; sleep 0.5; done; fi
  AETHER_PID=""
}

up() {
  load
  if [ -z "${PG_NAME:-}" ] || ! docker ps -q -f "name=^${PG_NAME}$" | grep -q .; then
    PG_NAME="$name"; PG_PORT="$(free_port)"
    docker run -d --name "$PG_NAME" --label aether.run=aether-web-visual \
      -e POSTGRES_USER=aether -e POSTGRES_PASSWORD=aether -e POSTGRES_DB=aether \
      -p "127.0.0.1:${PG_PORT}:5432" pgvector/pgvector:pg17 >/dev/null
    AETHER_PORT="$(free_port)"; SSR_PORT="$(free_port)"; LEGACY_PORT="$(free_port)"
    AETHER_PID=""; LEGACY_PID=""
    save
  fi
  for _ in $(seq 1 90); do
    docker exec "$PG_NAME" pg_isready -U aether -d aether -h 127.0.0.1 >/dev/null 2>&1 && break
    sleep 1
  done
  [ -d "$dir/library" ] || FFMPEG="$ffmpeg" "$root/tools/make-visual-library.sh" "$dir/library"
  [ -f "$root/crates/aether-web/build/index.json" ] || (cd "$root/crates/aether-web" && "$vertigo" build)
  if ! alive "${AETHER_PID:-}"; then
    start_aether; save
    seed
  fi
  # Fresh reference before every comparison: check the external checkout, pull, record provenance, verified copy.
  local prov sha frontend manifest restored reason next
  prov="$("$root/tools/prepare-kima-hub-reference.sh" --install)"
  sha="$(sed -n 's/^AETHER_UPSTREAM_REFERENCE_SHA=//p' <<<"$prov")"
  frontend="$(sed -n 's/^AETHER_REFERENCE_FRONTEND=//p' <<<"$prov")"
  manifest="$(sed -n 's/^AETHER_REFERENCE_MANIFEST=//p' <<<"$prov")"
  restored="$(sed -n 's/^AETHER_REFERENCE_COPY_RESTORED=//p' <<<"$prov")"
  { [ -n "$sha" ] && [ -n "$frontend" ] && [ -n "$manifest" ] && [ -n "$restored" ]; } \
    || { echo "prepare-kima-hub-reference.sh printed incomplete provenance" >&2; exit 1; }
  grep -E '^AETHER_(HUB_REFERENCE|REFERENCE)_[A-Z_]+=' <<<"$prov" >"$dir/reference.env"
  next="$(reference_next_bin "$frontend")" || exit 1
  if alive "${LEGACY_PID:-}"; then
    reason="$(reference_restart_reason "${REF_SHA:-}" "${REF_FRONTEND:-}" "${REF_MANIFEST:-}" "$sha" "$frontend" "$manifest" "$restored")"
    if [ -n "$reason" ]; then
      echo "$reason: restarting the reference dev server" >&2
      stop_legacy
    fi
  fi
  REF_SHA="$sha"; REF_FRONTEND="$frontend"; REF_MANIFEST="$manifest"; save
  if ! alive "${LEGACY_PID:-}"; then
    (
      cd "$REF_FRONTEND"
      NEXT_PUBLIC_BACKEND_URL="http://127.0.0.1:${AETHER_PORT}" AETHER_DEV_ORIGINS="127.0.0.1:${LEGACY_PORT}" \
        NEXT_TELEMETRY_DISABLED=1 exec "$next" dev -p "$LEGACY_PORT" -H 127.0.0.1 >"$dir/legacy.log" 2>&1
    ) &
    LEGACY_PID=$!; save
    for _ in $(seq 1 120); do
      curl -fsS "http://127.0.0.1:${LEGACY_PORT}/login" >/dev/null 2>&1 && break
      alive "$LEGACY_PID" || { echo "legacy dev server exited, see $dir/legacy.log" >&2; exit 1; }
      sleep 1
    done
  fi
  env_out
}

# Admin account + scan, once. The password is a throw-away of this isolated stack.
seed() {
  local base="http://127.0.0.1:${AETHER_PORT}"
  if [ "$(curl -fsS "$base/api/onboarding/status" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("hasAccount"))')" != "True" ]; then
    local tok
    tok="$(curl -fsS -X POST "$base/api/onboarding/register" -H 'content-type: application/json' \
      -d '{"username":"admin","password":"visual-pass-1"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')"
    curl -fsS -X POST "$base/api/onboarding/complete" -H "authorization: Bearer $tok" >/dev/null
    local job
    job="$(curl -fsS -X POST "$base/api/library/scan" -H "authorization: Bearer $tok" | python3 -c 'import json,sys;print(json.load(sys.stdin)["jobId"])')"
    for _ in $(seq 1 120); do
      st="$(curl -fsS "$base/api/library/scan/status/$job" -H "authorization: Bearer $tok" | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')"
      [ "$st" = completed ] && break
      sleep 1
    done
    [ "$st" = completed ] || { echo "scan did not complete ($st)" >&2; exit 1; }
    # A playlist so the playlists pages have content.
    local pl
    pl="$(curl -fsS -X POST "$base/api/playlists" -H "authorization: Bearer $tok" -H 'content-type: application/json' \
      -d '{"name":"Late Night Drive"}' | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d.get("id") or d.get("playlist",{}).get("id"))')"
    curl -fsS "$base/api/library/tracks?limit=5" -H "authorization: Bearer $tok" | python3 -c '
import json,sys
d=json.load(sys.stdin)
for t in d.get("tracks",[]): print(t["id"])' | while read -r tid; do
      curl -fsS -X POST "$base/api/playlists/$pl/items" -H "authorization: Bearer $tok" -H 'content-type: application/json' \
        -d "{\"trackId\":\"$tid\"}" >/dev/null
    done
    curl -fsS -X POST "$base/api/playlists" -H "authorization: Bearer $tok" -H 'content-type: application/json' \
      -d '{"name":"Empty Shelf"}' >/dev/null
  fi
}

env_out() {
  load
  cat <<EOF
export WEB_URL=http://127.0.0.1:${AETHER_PORT}
export LEGACY_URL=http://127.0.0.1:${LEGACY_PORT}
export ADMIN_USER=admin ADMIN_PASS=visual-pass-1
export AETHER_UPSTREAM_REFERENCE_SHA=${REF_SHA:-}
export AETHER_REFERENCE_PROVENANCE=$dir/reference.env
EOF
}

reload() {
  load
  (cd "$root/crates/aether-web" && "$vertigo" build)
  stop_aether
  start_aether; save
}

down() {
  load
  stop_aether
  stop_legacy
  [ -n "${PG_NAME:-}" ] && docker rm -f -v "$PG_NAME" >/dev/null 2>&1 || true
  rm -f "$state"
  echo "stopped"
}

cmd="${1:-up}"; shift || true
case "$cmd" in
  up) up ;;
  reload) reload ;;
  compare | run)
    # A stale checked-in Tailwind sheet made aether-web render without any utility class once; refuse to compare then.
    node "$root/tools/gen-aether-web-css.mjs" --check || { echo "run: node tools/gen-aether-web-css.mjs" >&2; exit 1; }
    up >/dev/null
    reload >/dev/null   # always a real `vertigo build` + server rebuild/restart, never a leftover build directory
    load
    set +e
    WEB_URL="http://127.0.0.1:${AETHER_PORT}" LEGACY_URL="http://127.0.0.1:${LEGACY_PORT}" \
      ADMIN_USER=admin ADMIN_PASS=visual-pass-1 AETHER_REFERENCE_PROVENANCE="$dir/reference.env" \
      node "$root/tools/compare-aether-web-visual.mjs" "$@"
    rc=$?
    set -e
    [ "$cmd" = run ] && down
    exit $rc
    ;;
  down) down ;;
  *) sed -n 2,13p "$0"; exit 2 ;;
esac
