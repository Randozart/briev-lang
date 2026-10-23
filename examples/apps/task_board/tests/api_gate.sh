#!/usr/bin/env bash
# Task Board A2 gate: sequential backend loop serving multiple requests.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
APP="$ROOT/examples/apps/task_board"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
PORT=18367
TMP="$(mktemp -d)"
SERVER="$TMP/main"
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

fetch() {
    local method="$1"
    local path="$2"
    local tag="$3"
    local body="${4:-}"
    local args=(-sS --max-time 2 -X "$method" -D "$TMP/$tag.headers" -o "$TMP/$tag.body")
    if [ -n "$body" ]; then
        args+=(-H "Content-Type: application/json" --data "$body")
    fi
    timeout -s KILL 3s curl "${args[@]}" "http://127.0.0.1:$PORT$path"
}

status_of() {
    head -n 1 "$TMP/$1.headers" | tr -d '\r'
}

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

health_ok=0
for _ in $(seq 1 60); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "task board: server exited before /health" >&2
        cat "$STDERR" >&2 || true
        exit 1
    fi
    if fetch GET /health health 2>/dev/null; then
        health_ok=1
        break
    fi
    sleep 0.05
done
test "$health_ok" = 1

test "$(status_of health)" = "HTTP/1.1 200 OK"
test "$(awk 'BEGIN{IGNORECASE=1} /^Content-Type:/{sub(/^[^:]*:[[:space:]]*/, ""); sub(/\r$/, ""); print; exit}' "$TMP/health.headers")" = "application/json"
test "$(cat "$TMP/health.body")" = '{"ok":true}'

fetch GET /api/tasks empty
test "$(status_of empty)" = "HTTP/1.1 200 OK"
test "$(cat "$TMP/empty.body")" = '[]'

fetch POST /api/tasks create '{"title":"write tests"}'
test "$(status_of create)" = "HTTP/1.1 200 OK"
test "$(cat "$TMP/create.body")" = '{"id":1,"title":"write tests","done":false}'

fetch GET /api/tasks listed
test "$(status_of listed)" = "HTTP/1.1 200 OK"
test "$(cat "$TMP/listed.body")" = '[{"id":1,"title":"write tests","done":false}]'

fetch POST /api/tasks/1/toggle toggled
test "$(status_of toggled)" = "HTTP/1.1 200 OK"
test "$(cat "$TMP/toggled.body")" = '[{"id":1,"title":"write tests","done":true}]'

fetch POST /api/tasks invalid '{}'
test "$(status_of invalid)" = "HTTP/1.1 400 Bad Request"
test "$(cat "$TMP/invalid.body")" = '{"error":"invalid_request"}'

fetch GET /missing missing
test "$(status_of missing)" = "HTTP/1.1 404 Not Found"
test "$(cat "$TMP/missing.body")" = '{"error":"not_found"}'

fetch DELETE /api/tasks/1 deleted
test "$(status_of deleted)" = "HTTP/1.1 200 OK"
test "$(cat "$TMP/deleted.body")" = '[]'

kill -9 "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

test ! -s "$STDOUT"
test ! -s "$STDERR"

echo "task board: sequential loop PASS"
