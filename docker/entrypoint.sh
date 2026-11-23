#!/bin/bash
# Starts the bundled Postgres (unless DATABASE_URL points elsewhere), then aether; stops both cleanly.
set -euo pipefail

PGDATA=/data/postgres
PGPASS_FILE=/data/postgres-password
# /data is writable by the unprivileged aether user; never follow a symlink planted there while running as root.
for p in /data "$PGDATA" "$PGPASS_FILE"; do
    if [ -L "$p" ]; then
        echo "aether-entrypoint: refusing to start: $p is a symlink" >&2
        exit 1
    fi
done
mkdir -p /data
chown -h aether:aether /data

# psql as the postgres superuser over the local socket (peer auth): pg_sql DB [psql args]; SQL on stdin.
pg_sql() {
    local db=$1
    shift
    gosu postgres psql -X -q -v ON_ERROR_STOP=1 -d "$db" "$@"
}

if [ -z "${DATABASE_URL:-}" ]; then
    if [ -e "$PGDATA" ]; then
        # aether owns /data and could have planted its own cluster (with a crafted postgresql.conf)
        # here; only accept a directory that postgres itself owns.
        if [ -L "$PGDATA" ] || [ ! -d "$PGDATA" ] || [ "$(stat -c '%U:%G %a' "$PGDATA")" != "postgres:postgres 700" ]; then
            echo "aether-entrypoint: refusing to start: $PGDATA must be a directory owned by postgres:postgres with mode 700" >&2
            echo "aether-entrypoint: found: $(stat -c '%F %U:%G %a' "$PGDATA")" >&2
            exit 1
        fi
    else
        mkdir "$PGDATA"
        chown -h postgres:postgres "$PGDATA"
        chmod 700 "$PGDATA"
    fi
    if [ ! -s "$PGDATA/PG_VERSION" ]; then
        gosu postgres initdb -D "$PGDATA" --username=postgres --auth-local=peer --auth-host=scram-sha-256 \
            --encoding=UTF8 >/dev/null
        echo "listen_addresses = '127.0.0.1'" >> "$PGDATA/postgresql.conf"
    elif grep -qE '^[^#]*[[:space:]]trust([[:space:]]|$)' "$PGDATA/pg_hba.conf"; then
        echo "aether-entrypoint: $PGDATA was created by an older image (passwordless 'aether' superuser)." >&2
        echo "aether-entrypoint: dump it with that image, remove $PGDATA, then start this image and restore." >&2
        exit 1
    fi
    # Only the postgres OS user may use the superuser; aether logs in with a password over 127.0.0.1.
    # Written as postgres, not root: a planted symlink inside PGDATA then cannot reach root-owned files.
    gosu postgres sh -c 'umask 077 && cat > "$1/pg_hba.conf"' sh "$PGDATA" <<'HBA'
local all  postgres                peer
host  aether aether     127.0.0.1/32   scram-sha-256
HBA

    # Generated once, readable by root only; aether receives it through DATABASE_URL.
    if [ ! -s "$PGPASS_FILE" ]; then
        rm -f "$PGPASS_FILE"
        (umask 077 && set -o noclobber && head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' > "$PGPASS_FILE")
    fi
    # A hardlink planted by aether would make the chown/chmod below hit another file.
    if [ -L "$PGPASS_FILE" ] || [ "$(stat -c '%h %F' "$PGPASS_FILE")" != "1 regular file" ]; then
        echo "aether-entrypoint: refusing to start: $PGPASS_FILE is not a regular single-link file" >&2
        exit 1
    fi
    chown -h root:root "$PGPASS_FILE"
    chmod 600 "$PGPASS_FILE"
    PGPASS=$(cat "$PGPASS_FILE")
    # aether owns /data and could swap the file; only accept what this script generates.
    if ! printf '%s' "$PGPASS" | grep -qxE '[A-Za-z0-9]{32,}'; then
        echo "aether-entrypoint: refusing to start: $PGPASS_FILE is not a generated password" >&2
        exit 1
    fi

    gosu postgres pg_ctl -D "$PGDATA" -w -l "$PGDATA/server.log" start
    # Idempotent on every start: an unprivileged login role owning the aether database.
    pg_sql postgres -v pass="$PGPASS" <<'SQL'
SELECT 'CREATE ROLE aether LOGIN' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aether')\gexec
ALTER ROLE aether LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD :'pass';
SELECT 'CREATE DATABASE aether OWNER aether ENCODING ''UTF8''' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'aether')\gexec
ALTER DATABASE aether OWNER TO aether;
SQL
    # Extensions a non-superuser cannot create; the migrations then find them present.
    pg_sql aether <<'SQL'
CREATE EXTENSION IF NOT EXISTS unaccent;
SQL
    export DATABASE_URL="postgres://aether:${PGPASS}@127.0.0.1:5432/aether"
    unset PGPASS
    BUNDLED_PG=1
fi

stop_pg() {
    if [ "${BUNDLED_PG:-}" = 1 ]; then gosu postgres pg_ctl -D "$PGDATA" -m fast stop || true; fi
}

# Private snapshot directory for M4 embeddings: on the library mount (reflink cannot cross
# mounts), owned by aether, mode 0700. aether itself verifies this at startup and disables the
# embedding analysis (server keeps running) if it is wrong.
if [ -n "${AETHER_SNAPSHOT_DIR:-}" ]; then
    if [ -L "$AETHER_SNAPSHOT_DIR" ]; then
        echo "aether-entrypoint: refusing to use $AETHER_SNAPSHOT_DIR: it is a symlink" >&2
    elif ! { install -d -m 0700 -o aether -g aether "$AETHER_SNAPSHOT_DIR" \
        && chown -h aether:aether "$AETHER_SNAPSHOT_DIR" && chmod 0700 "$AETHER_SNAPSHOT_DIR"; }; then
        echo "aether-entrypoint: could not prepare $AETHER_SNAPSHOT_DIR (read-only mount?); embeddings will be disabled" >&2
    fi
fi

gosu aether aether &
AETHER_PID=$!
trap 'kill -TERM "$AETHER_PID" 2>/dev/null || true' TERM INT
set +e
wait "$AETHER_PID"; status=$?
# `wait` returns early when a trapped signal arrives; wait again for aether to finish.
if kill -0 "$AETHER_PID" 2>/dev/null; then wait "$AETHER_PID"; status=$?; fi
set -e
stop_pg
exit "$status"
