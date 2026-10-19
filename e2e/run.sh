#!/usr/bin/env bash
# The browser attack (make e2e-attack): a throwaway Magnum BI with hostile names, labels, markdown
# and data, opened in Chromium; fails if any script runs. Local only: needs Podman or Docker,
# Node, and the built UI and server (the make target builds them).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="$ROOT/target/debug/magnum-bi"
PORT="${E2E_PORT:-8097}"
OWNER="attacker@e2e.example"
PASSWORD="e2e-attack-password"
WORK="$(mktemp -d)"
SERVER=""

cleanup() {
  [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null || true
  for db in e2eattack e2eattackorg; do devtools/scramdb.sh reset "$db" >/dev/null 2>&1 || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

devtools/scramdb.sh up e2eattack 55495 >/dev/null
devtools/scramdb.sh up e2eattackorg 55496 >/dev/null
export MAGNUM_DATABASE_URL="postgresql://scramdb@127.0.0.1:55495/scramdb"
export MAGNUM_SECRET_KEY="$("$BIN" gen-key)"
export MAGNUM_SYSTEM_OWNERS="$OWNER"
export MAGNUM_PORT="$PORT"
export MAGNUM_DUCKDB_DIR="$WORK/duckdb"
export MAGNUM_DUCKDB_INSTALL_EXTENSIONS=0
"$BIN" migrate >/dev/null
echo "$PASSWORD" | "$BIN" user set-password "$OWNER" --stdin >/dev/null

"$BIN" serve >"$WORK/server.log" 2>&1 &
SERVER=$!
for _ in $(seq 60); do curl -sf "http://127.0.0.1:$PORT/api/health" >/dev/null && break; sleep 1; done

# an organisation with a hostile name, its own database, and data full of hostile values
BASE="http://127.0.0.1:$PORT"
(cd e2e && { [ -d node_modules ] || npm ci --silent; } && npx playwright install chromium >/dev/null)
export E2E_BASE="$BASE" E2E_EMAIL="$OWNER" E2E_PASSWORD="$PASSWORD"
node e2e/attack.mjs setup
echo "postgresql://scramdb@127.0.0.1:55496/scramdb" | "$BIN" org set-database xss >/dev/null
node e2e/data.mjs > "$WORK/sales.csv"
mkdir -p "$MAGNUM_DUCKDB_DIR/xss" && cp "$WORK/sales.csv" "$MAGNUM_DUCKDB_DIR/xss/sales.csv"

node e2e/attack.mjs
