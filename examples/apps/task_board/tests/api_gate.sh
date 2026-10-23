#!/usr/bin/env bash
# Task Board A1 gate: one-shot /health backend.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
APP="$ROOT/examples/apps/task_board"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
PORT=18367
TMP="$(mktemp -d)"
SERVER="$TMP/main"
BODY="$TMP/body"
HEADERS="$TMP/headers"
STDOUT="$TMP/stdout"
STDERR="$TMP/stderr"
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill -9 "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"
if ! timeout -s KILL 30s "$BRIEVC" check "$APP/backend/main.bv" >"$TMP/check.log" 2>&1; then
    cat "$TMP/check.log" >&2
    exit 1
fi
if ! timeout -s KILL 60s "$BRIEVC" build "$APP/backend/main.bv" --out "$TMP" >"$TMP/build.log" 2>&1; then
    cat "$TMP/build.log" >&2
    exit 1
fi
test -x "$SERVER"

"$SERVER" >"$STDOUT" 2>"$STDERR" &
SERVER_PID=$!

response=""
for _ in $(seq 1 50); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        break
    fi
    if response="$(curl -sS --max-time 1 -D "$HEADERS" -o "$BODY" "http://127.0.0.1:$PORT/health" 2>/dev/null)"; then
        break
    fi
    sleep 0.05
done

if [ -z "$(cat "$BODY" 2>/dev/null || true)" ]; then
    echo "task board: backend did not serve /health" >&2
    cat "$STDERR" >&2 || true
    exit 1
fi

first_line="$(head -n 1 "$HEADERS" | tr -d '\r')"
content_type="$(awk 'BEGIN{IGNORECASE=1} /^Content-Type:/{sub(/^[^:]*:[[:space:]]*/, ""); sub(/\r$/, ""); print; exit}' "$HEADERS")"
body="$(cat "$BODY")"

test "$first_line" = "HTTP/1.1 200 OK"
test "$content_type" = "application/json"
test "$body" = '{"ok":true}'

wait "$SERVER_PID"
SERVER_PID=""
test "$(cat "$STDOUT")" = "1"
test ! -s "$STDERR"

echo "task board: /health PASS"
