#!/usr/bin/env bash
# Starts a local aether-web stack in one of two shapes:
#   dev (default / --watch): aether-server (API) + the vertigo dev server, which serves aether-web and
#       proxies /api/* to aether-server, so the browser sees ONE origin (http://127.0.0.1:$WEB_PORT).
#   --ssr: the production shape. `vertigo build`, then aether-server alone with the SSR server embedded
#       in-process (AETHER_WEB_BUILD_DIR): ONE origin, http://127.0.0.1:$AETHER_PORT/, pages server-rendered
#       by aether-server's wasmtime and proxied to a loopback-only port ($SSR_PORT). No vertigo dev server.
#
# Usage:  MUSIC_PATH=/path/to/music tools/run-aether-web-dev.sh [--watch | --ssr] [--no-build]
#   (default)   `vertigo build` once, then `vertigo serve` (release wasm, needs wasm-opt on PATH)
#   --watch     `vertigo watch` instead: debug wasm, rebuilds on source changes
#   --ssr       build, then run aether-server with the embedded SSR (see above); no WEB_PORT is used
#   --no-build  reuse crates/aether-web/build/ as it is (serve and --ssr modes)
#
# Environment:
#   MUSIC_PATH         REQUIRED. Directory aether-server scans. First-run flow: open the UI, create the admin
#                      account, press "Scan library" (it calls POST /api/library/scan on this directory).
#   DATABASE_URL       Postgres (pgvector) for aether-server. If unset, a dedicated Docker postgres is started
#                      or reused: container aether-web-dev-postgres, label aether.run=aether-web-dev, named volume
#                      aether-web-dev-pgdata, bound to 127.0.0.1:$AETHER_WEB_DEV_PG_PORT (default 55433).
#                      It never defaults to the dev DB on 5456.
#   AETHER_DATA_DIR      aether-server state (secrets, caches). Default: $HOME/.local/share/aether-web-dev
#   AETHER_PORT          aether-server port (default 3030)        WEB_PORT   dev server port (default 4444)
#   SSR_PORT           loopback port of the embedded SSR server in --ssr mode (default 3031)
#   VERTIGO            vertigo CLI (0.13.4, default: `vertigo` on PATH)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
web="$root/crates/aether-web"
vertigo="${VERTIGO:-vertigo}"
aether_port="${AETHER_PORT:-3030}"
web_port="${WEB_PORT:-4444}"
mode=serve
build=1
ssr_port="${SSR_PORT:-3031}"
for arg in "$@"; do
  case "$arg" in
    --watch) mode=watch ;;
    --ssr) mode=ssr ;;
    --no-build) build=0 ;;
    -h | --help) sed -n '2,29p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

: "${MUSIC_PATH:?MUSIC_PATH must point at the music directory to scan (e.g. MUSIC_PATH=\$HOME/Music)}"
[ -d "$MUSIC_PATH" ] || { echo "MUSIC_PATH is not a directory: $MUSIC_PATH" >&2; exit 1; }
export MUSIC_PATH
export AETHER_DATA_DIR="${AETHER_DATA_DIR:-$HOME/.local/share/aether-web-dev}"
mkdir -p "$AETHER_DATA_DIR"

# Waits until Postgres answers a real connection from the host (not just an open port).
wait_pg() {
  for _ in $(seq 1 60); do
    if psql "$1" -qtAc 'select 1' >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  echo "Postgres did not answer on the host within 60s: $1" >&2
  return 1
}

if [ -z "${DATABASE_URL:-}" ]; then
  pg_port="${AETHER_WEB_DEV_PG_PORT:-55433}"
  name=aether-web-dev-postgres
  if [ -z "$(docker ps -aq -f "name=^${name}$")" ]; then
    docker run -d --name "$name" --label aether.run=aether-web-dev \
      -e POSTGRES_USER=aether -e POSTGRES_PASSWORD=aether -e POSTGRES_DB=aether \
      -v aether-web-dev-pgdata:/var/lib/postgresql/data \
      -p "127.0.0.1:${pg_port}:5432" pgvector/pgvector:pg17 >/dev/null
    echo "started dedicated Docker postgres '$name' (label aether.run=aether-web-dev, volume aether-web-dev-pgdata, 127.0.0.1:${pg_port})"
  else
    docker start "$name" >/dev/null
    echo "reusing dedicated Docker postgres '$name' (volume aether-web-dev-pgdata)"
  fi
  export DATABASE_URL="postgres://aether:aether@127.0.0.1:${pg_port}/aether"
  echo "remove it with: docker rm -f $name && docker volume rm aether-web-dev-pgdata"
fi
wait_pg "$DATABASE_URL"

pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM

echo "building aether-server..."
(cd "$root" && SQLX_OFFLINE=true cargo build -p aether-server)

if [ "$mode" = ssr ]; then
  if [ "$build" = 1 ]; then (cd "$web" && "$vertigo" build); fi
  [ -f "$web/build/index.json" ] || { echo "no vertigo build output in $web/build (run without --no-build)" >&2; exit 1; }
  # AETHER_WEB_BUILD_DIR turns the embedded SSR on; aether exits non-zero if that server cannot start or dies.
  (
    cd "$root"
    PORT="$aether_port" AETHER_BIND_HOST=127.0.0.1 AETHER_IDENTIFY="${AETHER_IDENTIFY:-off}" \
      AETHER_WEB_BUILD_DIR=crates/aether-web/build AETHER_WEB_SSR_PORT="$ssr_port" \
      exec target/debug/aether
  ) &
  aether_pid=$!
  pids+=("$aether_pid")
  # The first start compiles the wasm with wasmtime before the port opens: allow it time.
  for _ in $(seq 1 180); do
    curl -fsS "http://127.0.0.1:${aether_port}/api/health" >/dev/null 2>&1 && break
    kill -0 "$aether_pid" 2>/dev/null || { wait "$aether_pid" || true; echo "aether-server exited during start" >&2; exit 1; }
    sleep 1
  done
  curl -fsS "http://127.0.0.1:${aether_port}/api/health" >/dev/null || { echo "aether-server did not come up on :$aether_port" >&2; exit 1; }
  echo
  echo "aether-web (embedded SSR): http://127.0.0.1:${aether_port}/   (SSR server on loopback :${ssr_port})"
  echo "music dir: $MUSIC_PATH"
  # Stay in the foreground for as long as aether-server runs; its exit status is the script's.
  wait "$aether_pid"
  exit $?
fi

# AETHER_IDENTIFY=off: no outbound MusicBrainz/AcoustID lookups in dev.
(
  cd "$root"
  PORT="$aether_port" AETHER_BIND_HOST=127.0.0.1 AETHER_IDENTIFY="${AETHER_IDENTIFY:-off}" \
    exec target/debug/aether
) &
pids+=($!)
for _ in $(seq 1 60); do
  curl -fsS "http://127.0.0.1:${aether_port}/api/health" >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS "http://127.0.0.1:${aether_port}/api/health" >/dev/null || { echo "aether-server did not come up on :$aether_port" >&2; exit 1; }

proxy="--proxy /api=http://127.0.0.1:${aether_port}/api"
cd "$web"
echo
echo "aether-web: http://127.0.0.1:${web_port}   (API proxied to http://127.0.0.1:${aether_port}/api)"
echo "music dir: $MUSIC_PATH"
if [ "$mode" = watch ]; then
  # shellcheck disable=SC2086
  "$vertigo" watch --port "$web_port" $proxy
else
  [ "$build" = 1 ] && "$vertigo" build
  # shellcheck disable=SC2086
  "$vertigo" serve --port "$web_port" $proxy
fi
