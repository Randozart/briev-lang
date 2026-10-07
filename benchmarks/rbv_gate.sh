#!/usr/bin/env bash
# rbv_gate.sh — the web-surface regression gate.
#
# 2026-10-07 (plan 2026-10-07-web-surface-completion.md, W1+W2): builds the
# web fixtures, checks the generated IR for call/declare agreement, then runs
# the wasm32 runtime gate. Guards the pointer-width / void-frgn /
# view-surface-liveness fixes and the unpacked-obj String-field init
# (BUGS.md:8800) — a funcref signature mismatch or an aggregate-vs-element
# store is accepted by some stages but rejected by llc / traps on wasm32.
#
# Usage: bash benchmarks/rbv_gate.sh
#   BRIEVC=/path/to/brievc   override the compiler binary
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BRIEVC="${BRIEVC:-$ROOT/target/debug/brievc}"
[ -x "$BRIEVC" ] || BRIEVC="$ROOT/target/release/brievc"
[ -x "$BRIEVC" ] || { echo "brievc not found (build first, or set BRIEVC)"; exit 1; }

OUT_ROOT="$(mktemp -d /tmp/opencode/rbvgate.XXXXXX)"

# Build a fixture; echo its output dir (empty on failure).
build_fixture() {
    local fixture="$1"
    local name; name="$(basename "${fixture%.rbv}")"
    local out="$OUT_ROOT/$name"
    mkdir -p "$out"
    "$BRIEVC" build "$fixture" --split --out "$out" >/dev/null || {
        echo "build failed: $fixture" >&2; return 1;
    }
    echo "$out"
}

# IR call/declare agreement for a built fixture.
check_ir() {
    local out="$1" name="$2"
    local ll="$out/$name.ll"
    [ -f "$ll" ] || { echo "no IR emitted at $ll" >&2; return 1; }
    if command -v python3 >/dev/null; then
        python3 "$ROOT/benchmarks/rbv_ir_check.py" "$ll" || return 1
    else
        echo "SKIP: python3 not available (IR call/declare check)"
    fi
}

# 1. Router smoke — IR + runtime behavior.
ROUTER_OUT="$(build_fixture "$ROOT/tests/fixtures/router.rbv")" || exit 1
check_ir "$ROUTER_OUT" router || exit 1
if command -v node >/dev/null; then
    node "$ROOT/benchmarks/rbv_gate.mjs" "$ROUTER_OUT/router.wasm" || exit 1
else
    echo "SKIP: node not available (runtime gate)"
fi

# 2. Unpacked-obj String-field init (BUGS.md:8800) — build + IR.
#    The old emitter's invalid `store [1 x ptr]` made the build itself fail.
OBJ_OUT="$(build_fixture "$ROOT/tests/fixtures/obj_init.rbv")" || exit 1
check_ir "$OBJ_OUT" obj_init || exit 1

echo "rbv_gate: OK"
