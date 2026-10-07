#!/usr/bin/env bash
# rbv_gate.sh — the web-surface regression gate.
#
# 2026-10-07 (plan 2026-10-07-web-surface-completion.md, W1): builds the
# router smoke fixture, checks the generated IR for call/declare agreement,
# then runs the wasm32 runtime gate. Guards the pointer-width / void-frgn /
# view-surface-liveness fixes that unblocked the router (a funcref signature
# mismatch lowers to a trap stub on wasm32 — only the IR check or a runtime
# probe catches it).
#
# Usage: bash benchmarks/rbv_gate.sh [fixture.rbv]
#   BRIEVC=/path/to/brievc   override the compiler binary
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="${1:-$ROOT/tests/fixtures/router.rbv}"
NAME="$(basename "${FIXTURE%.rbv}")"

BRIEVC="${BRIEVC:-$ROOT/target/debug/brievc}"
[ -x "$BRIEVC" ] || BRIEVC="$ROOT/target/release/brievc"
[ -x "$BRIEVC" ] || { echo "brievc not found (build first, or set BRIEVC)"; exit 1; }

OUT="$(mktemp -d /tmp/opencode/rbvgate.XXXXXX)"
"$BRIEVC" build "$FIXTURE" --split --out "$OUT" >/dev/null || {
    echo "build failed: $FIXTURE"; exit 1;
}

LL="$OUT/$NAME.ll"
WASM="$OUT/$NAME.wasm"
[ -f "$LL" ] || { echo "no IR emitted at $LL"; exit 1; }

if command -v python3 >/dev/null; then
    python3 "$ROOT/benchmarks/rbv_ir_check.py" "$LL" || exit 1
else
    echo "SKIP: python3 not available (IR call/declare check)"
fi

if command -v node >/dev/null; then
    node "$ROOT/benchmarks/rbv_gate.mjs" "$WASM" || exit 1
else
    echo "SKIP: node not available (runtime gate)"
fi

echo "rbv_gate: OK"
