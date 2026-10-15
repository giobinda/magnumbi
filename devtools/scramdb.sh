#!/usr/bin/env bash
# Local development ScramDB for Magnum BI, in a container (Docker or Podman).
#   devtools/scramdb.sh up | down | reset | status | url | psql
# ScramDB listens on 127.0.0.1:${MAGNUM_DEV_DB_PORT:-55432} (PostgreSQL wire protocol), user and
# database `scramdb`, no password (the image's default: local development only).
set -euo pipefail
PORT="${MAGNUM_DEV_DB_PORT:-55432}"
NAME=magnum-scramdb
VOLUME=magnum-scramdb-data
IMAGE="${MAGNUM_SCRAMDB_IMAGE:-docker.io/scramdb/scramdb:latest}"
URL="postgresql://scramdb@127.0.0.1:${PORT}/scramdb"

engine() {
  if command -v podman >/dev/null; then echo podman
  elif command -v docker >/dev/null && docker info >/dev/null 2>&1; then echo docker
  else echo "Neither Podman nor a running Docker found; install one of them." >&2; exit 1; fi
}

wait_ready() {
  local e; e=$(engine)
  for _ in $(seq 1 60); do
    if "$e" exec "$NAME" psql -h 127.0.0.1 -U scramdb scramdb -qAtc 'select 1' >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  echo "ScramDB did not become ready; see: $e logs $NAME" >&2; return 1
}

case "${1:-status}" in
  up)
    e=$(engine)
    if ! "$e" container exists "$NAME" 2>/dev/null && ! "$e" ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
      "$e" run -d --name "$NAME" -p "127.0.0.1:${PORT}:5432" -v "$VOLUME:/var/lib/scramdb" "$IMAGE" >/dev/null
    else
      "$e" start "$NAME" >/dev/null
    fi
    wait_ready
    echo "ScramDB up ($e): $URL" ;;
  down)
    e=$(engine); "$e" stop "$NAME" >/dev/null 2>&1 || true
    echo "ScramDB stopped (data kept; 'devtools/scramdb.sh reset' deletes it)" ;;
  reset)
    e=$(engine); "$e" rm -f "$NAME" >/dev/null 2>&1 || true; "$e" volume rm "$VOLUME" >/dev/null 2>&1 || true
    echo "ScramDB container and data deleted" ;;
  url) echo "$URL" ;;
  psql) e=$(engine); exec "$e" exec -it "$NAME" psql -h 127.0.0.1 -U scramdb scramdb ;;
  status) e=$(engine); echo "engine: $e  url: $URL"; "$e" ps -a --filter "name=$NAME" ;;
  *) echo "usage: $0 up|down|reset|status|url|psql" >&2; exit 2 ;;
esac
