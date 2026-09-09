#!/usr/bin/env bash
#
# Parity golden capture (2026-09-09, plan §6).
#
# Builds a parity corpus .bv with the CURRENT compiler and captures its
# stdout as the golden reference. Must be run BEFORE any runtime migration —
# the goldens are the regression target the Briev-native runtime must match
# byte-for-byte.
#
# Usage: bash benchmarks/parity/capture.sh <corpus-name>
#   corpus source: benchmarks/parity/corpus/<name>.bv
#   golden output: benchmarks/parity/goldens/<name>.stdout
#   golden stderr: benchmarks/parity/goldens/<name>.stderr (if non-empty)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CORPUS_DIR="$ROOT/benchmarks/parity/corpus"
GOLDEN_DIR="$ROOT/benchmarks/parity/goldens"
OUT_DIR="$ROOT/benchmarks/parity/_out"

NAME="$1"
SRC="$CORPUS_DIR/$NAME.bv"
if [ ! -f "$SRC" ]; then
    echo "error: corpus $SRC not found" >&2
    exit 1
fi

mkdir -p "$GOLDEN_DIR" "$OUT_DIR"
rm -f "$OUT_DIR/$NAME" "$OUT_DIR/$NAME.ll" "$OUT_DIR/$NAME.o"

# 2026-07-26: Clear FFI cache + temp objects to avoid duplicate symbols.
rm -f ~/.cache/briev-compiler/ffi/*.o /tmp/briev_rt*.o 2>/dev/null || true

BOUND="${BOUND:-1000}" "$ROOT/target/release/brievc" build "$SRC" \
    --out "$OUT_DIR" --optimize-budget "${BUDGET:-256}" 2>&1

if [ ! -f "$OUT_DIR/$NAME" ]; then
    echo "error: build produced no binary" >&2
    exit 1
fi

"$OUT_DIR/$NAME" > "$GOLDEN_DIR/$NAME.stdout" 2> "$GOLDEN_DIR/$NAME.stderr"
if [ ! -s "$GOLDEN_DIR/$NAME.stderr" ]; then
    rm -f "$GOLDEN_DIR/$NAME.stderr"
fi

echo "captured $NAME: $(wc -c < "$GOLDEN_DIR/$NAME.stdout") bytes stdout"