#!/usr/bin/env bash
# Task Board A4 gate: real Chromium reads native-backend state through CORS.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
APP="$ROOT/examples/apps/task_board"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
API_PORT=18367
WEB_PORT=18468
TMP="$(mktemp -d)"
BACKEND="$TMP/main"
FRONTEND="$TMP/frontend"
API_LOG="$TMP/api.log"
WEB_LOG="$TMP/web.log"
BACKEND_PID=""
WEB_PID=""

cleanup() {
    if [ -n "$BACKEND_PID" ] && kill -0 "$BACKEND_PID" 2>/dev/null; then
        kill -9 "$BACKEND_PID" 2>/dev/null || true
    fi
    if [ -n "$WEB_PID" ] && kill -0 "$WEB_PID" 2>/dev/null; then
        kill -9 "$WEB_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"

if ! timeout -s KILL 30s "$BRIEVC" check "$APP/backend/main.bv" >"$TMP/check.log" 2>&1; then
    cat "$TMP/check.log" >&2
    exit 1
fi
if ! timeout -s KILL 60s "$BRIEVC" build "$APP/backend/main.bv" --out "$TMP" >"$TMP/backend_build.log" 2>&1; then
    cat "$TMP/backend_build.log" >&2
    exit 1
fi
mkdir -p "$FRONTEND"
if ! timeout -s KILL 90s "$BRIEVC" build "$APP/frontend/task_board.rbv" --split --out "$FRONTEND" >"$TMP/frontend_build.log" 2>&1; then
    cat "$TMP/frontend_build.log" >&2
    exit 1
fi
test -x "$BACKEND"
test -f "$FRONTEND/task_board.html"

"$BACKEND" >"$API_LOG" 2>&1 &
BACKEND_PID=$!
ready=0
for _ in $(seq 1 60); do
    if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
        echo "task board frontend gate: API exited before readiness" >&2
        cat "$API_LOG" >&2 || true
        exit 1
    fi
    if timeout -s KILL 2s curl -sS --max-time 1 -o /dev/null "http://127.0.0.1:$API_PORT/health" 2>/dev/null; then
        ready=1
        break
    fi
    sleep 0.05
done
test "$ready" = 1

# Seed one task so the browser gate proves actual API content, not merely a
# successful empty response.
create_status="$(timeout -s KILL 3s curl -sS --max-time 2 -o "$TMP/create.body" -w '%{http_code}' \
    -X POST -H 'Content-Type: application/json' \
    --data '{"title":"browser read test"}' \
    "http://127.0.0.1:$API_PORT/api/tasks")"
test "$create_status" = "200"
test "$(cat "$TMP/create.body")" = '{"id":1,"title":"browser read test","done":false}'

(
    cd "$FRONTEND"
    exec python3 -m http.server "$WEB_PORT" --bind 127.0.0.1
) >"$WEB_LOG" 2>&1 &
WEB_PID=$!
sleep 0.2

expected='[{"id":1,"title":"browser read test","done":false}]'
timeout -s KILL 60s node "$ROOT/benchmarks/task_board_frontend.mjs" \
    "http://127.0.0.1:$WEB_PORT/task_board.html" "$expected"

kill -9 "$BACKEND_PID" "$WEB_PID" 2>/dev/null || true
BACKEND_PID=""
WEB_PID=""

test ! -s "$API_LOG"
echo "task board frontend gate: PASS"
