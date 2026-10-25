#!/usr/bin/env bash
# Optional live PostgreSQL gate. Requires a local trust-auth DB and PGUSER/PGDATABASE.
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${PGUSER:?set PGUSER}"
: "${PGDATABASE:?set PGDATABASE}"
OUT=$(mktemp -d /tmp/briev-pg.XXXXXX)
trap 'rm -rf "$OUT"' EXIT
./target/release/brievc build tests/fixtures/stdlib/pg_receive_gate.bv --out "$OUT"
"$OUT/pg_receive_gate"
