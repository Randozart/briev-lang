#!/usr/bin/env bash
#
# Parity golden capture for REAL benchmarks (plan §6.1).
# Captures stdout from the benchmark at a small BOUND with the CURRENT
# (C-backed) compiler — the reference the Briev-native runtime must match.
#
# Usage: bash benchmarks/parity/capture_bench.sh <name> [bound]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GOLDEN_DIR="$ROOT/benchmarks/parity/goldens"
OUT_DIR="$ROOT/benchmarks/parity/_out"

NAME="$1"
BOUND="${2:-}"
if [ -z "$BOUND" ]; then
    BOUND="$(awk -v n="$NAME" '$1 == n {print $2}' "$ROOT/benchmarks/parity/bench.list")"
fi
if [ -z "$BOUND" ]; then
    echo "error: no bound for $NAME (pass one or add to bench.list)" >&2
    exit 1
fi

mkdir -p "$GOLDEN_DIR" "$OUT_DIR"
rm -f "$OUT_DIR/$NAME" "$OUT_DIR/$NAME.ll" "$OUT_DIR/$NAME.o"
rm -f ~/.cache/briev-compiler/ffi/*.o /tmp/briev_rt*.o 2>/dev/null || true

env BOUND="$BOUND" "$ROOT/target/release/brievc" build "benchmarks/$NAME.bv" \
    --out "$OUT_DIR" --optimize-budget "${BUDGET:-256}" 2>&1

if [ ! -f "$OUT_DIR/$NAME" ]; then
    echo "error: build produced no binary" >&2
    exit 1
fi

BOUND="$BOUND" "$OUT_DIR/$NAME" > "$GOLDEN_DIR/$NAME.stdout" 2> "$GOLDEN_DIR/$NAME.stderr"
if [ ! -s "$GOLDEN_DIR/$NAME.stderr" ]; then
    rm -f "$GOLDEN_DIR/$NAME.stderr"
fi
echo "captured $NAME (BOUND=$BOUND): $(wc -c < "$GOLDEN_DIR/$NAME.stdout") bytes stdout"