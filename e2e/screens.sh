#!/usr/bin/env bash
# The website's screenshots (make screenshots): a throwaway Magnum BI with a demo company (ACME,
# its own ScramDB, a warehouse with the sample sales), photographed by e2e/screens.mjs into
# website/assets/screens/. Local only: needs Podman or Docker, Node, and the built UI and server.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="$ROOT/target/debug/magnum-bi"
PORT="${SCREENS_PORT:-8096}"
OWNER="owner@acme.example"
ADMIN="ana@acme.example"
PASSWORD="screens-password-1"
WORK="$(mktemp -d)"
SERVER=""

cleanup() {
  [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null || true
  for db in screens screensorg screensdata; do devtools/scramdb.sh reset "$db" >/dev/null 2>&1 || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

devtools/scramdb.sh up screens 55491 >/dev/null
devtools/scramdb.sh up screensorg 55492 >/dev/null
devtools/scramdb.sh up screensdata 55493 >/dev/null
# the company's data warehouse: the sample sales
MAGNUM_DATABASE_URL="postgresql://scramdb@127.0.0.1:55493/scramdb" "$BIN" seed-sample >/dev/null

export MAGNUM_DATABASE_URL="postgresql://scramdb@127.0.0.1:55491/scramdb"
export MAGNUM_SECRET_KEY="$("$BIN" gen-key)"
export MAGNUM_SYSTEM_OWNERS="$OWNER"
export MAGNUM_PORT="$PORT"
export MAGNUM_PUBLIC_URL="http://127.0.0.1:$PORT"
export MAGNUM_DUCKDB_DIR="$WORK/duckdb"
export MAGNUM_DUCKDB_INSTALL_EXTENSIONS=0
"$BIN" seed-sample >/dev/null   # the platform workspace's own sample (its template dashboard)
for who in "$OWNER" "$ADMIN"; do echo "$PASSWORD" | "$BIN" user set-password "$who" --stdin >/dev/null; done

"$BIN" serve >"$WORK/server.log" 2>&1 &
SERVER=$!
for _ in $(seq 60); do curl -sf "http://127.0.0.1:$PORT/api/health" >/dev/null && break; sleep 1; done

(cd e2e && { [ -d node_modules ] || npm ci --silent; } && npx playwright install chromium >/dev/null)
export SCREENS_BASE="http://127.0.0.1:$PORT" SCREENS_OWNER="$OWNER" SCREENS_ADMIN="$ADMIN" SCREENS_PASSWORD="$PASSWORD"
node e2e/screens.mjs organisation
echo "postgresql://scramdb@127.0.0.1:55492/scramdb" | "$BIN" org set-database acme >/dev/null
node e2e/screens.mjs
