#!/usr/bin/env bash
# Local development ScramDB instances for Magnum BI, in containers (Podman or Docker).
#
#   devtools/scramdb.sh up|down|reset|status|url|psql [instance] [port]
#
# `instance` is "primary" (the default: Magnum BI's primary database, port 55432) or an
# organisation id: each organisation gets its own instance, e.g. `devtools/scramdb.sh up acme 55433`.
# ScramDB listens on 127.0.0.1:<port> (PostgreSQL wire protocol), user and database `scramdb`,
# no password (the image's default: local development only).
set -euo pipefail
CMD="${1:-status}"
INSTANCE="${2:-primary}"
IMAGE="${MAGNUM_SCRAMDB_IMAGE:-docker.io/scramdb/scramdb:latest}"
if [ "$INSTANCE" = primary ]; then
  NAME=magnum-scramdb; VOLUME=magnum-scramdb-data; DEFAULT_PORT="${MAGNUM_DEV_DB_PORT:-55432}"
else
  NAME="magnum-scramdb-$INSTANCE"; VOLUME="magnum-scramdb-$INSTANCE-data"; DEFAULT_PORT=""
fi

engine() {
  if command -v podman >/dev/null; then echo podman
  elif command -v docker >/dev/null && docker info >/dev/null 2>&1; then echo docker
  else echo "Neither Podman nor a running Docker found; install one of them." >&2; exit 1; fi
}

exists() { "$1" ps -a --format '{{.Names}}' | grep -qx "$NAME"; }

# the host port of an existing container, else the one asked for
port() {
  local e; e=$(engine)
  if exists "$e"; then "$e" port "$NAME" 5432/tcp 2>/dev/null | head -1 | sed 's/.*://'; return; fi
  local p="${3:-$DEFAULT_PORT}"
  if [ -z "$p" ]; then echo "give a port for a new instance: $0 up $INSTANCE <port>" >&2; exit 2; fi
  echo "$p"
}

wait_ready() {
  local e; e=$(engine)
  for _ in $(seq 1 60); do
    if "$e" exec "$NAME" psql -h 127.0.0.1 -U scramdb scramdb -qAtc 'select 1' >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  echo "ScramDB ($INSTANCE) did not become ready; see: $e logs $NAME" >&2; return 1
}

case "$CMD" in
  up)
    e=$(engine)
    if ! exists "$e"; then
      p=$(port "$@")
      "$e" run -d --name "$NAME" -p "127.0.0.1:${p}:5432" -v "$VOLUME:/var/lib/scramdb" "$IMAGE" >/dev/null
    else
      "$e" start "$NAME" >/dev/null
    fi
    wait_ready
    echo "ScramDB $INSTANCE up ($e): postgresql://scramdb@127.0.0.1:$(port "$@")/scramdb" ;;
  down)
    e=$(engine); "$e" stop "$NAME" >/dev/null 2>&1 || true
    echo "ScramDB $INSTANCE stopped (data kept; '$0 reset $INSTANCE' deletes it)" ;;
  reset)
    e=$(engine); "$e" rm -f "$NAME" >/dev/null 2>&1 || true; "$e" volume rm "$VOLUME" >/dev/null 2>&1 || true
    echo "ScramDB $INSTANCE container and data deleted" ;;
  url) echo "postgresql://scramdb@127.0.0.1:$(port "$@")/scramdb" ;;
  psql) e=$(engine); exec "$e" exec -it "$NAME" psql -h 127.0.0.1 -U scramdb scramdb ;;
  status) e=$(engine); "$e" ps -a --filter "name=magnum-scramdb" ;;
  *) echo "usage: $0 up|down|reset|status|url|psql [primary|<org id>] [port]" >&2; exit 2 ;;
esac
