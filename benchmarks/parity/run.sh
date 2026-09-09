#!/usr/bin/env bash
#
# Parity verification (2026-09-09, plan §6).
#
# Rebuilds each parity corpus with the CURRENT compiler and diffs stdout
# against the committed golden. A migration is complete only when every
# corpus it touches reports PASS — byte-identical output to the pre-migration
# C-backed runtime.
#
# Usage: bash benchmarks/parity/run.sh [corpus-name...]
#   (no args: run every corpus + benchmark golden in benchmarks/parity/)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CORPUS_DIR="$ROOT/benchmarks/parity/corpus"
GOLDEN_DIR="$ROOT/benchmarks/parity/goldens"
OUT_DIR="$ROOT/benchmarks/parity/_out"
BENCH_LIST="$ROOT/benchmarks/parity/bench.list"

PASS=0
FAIL=0
FAILED_NAMES=()

if [ "$#" -gt 0 ]; then
    NAMES=("$@")
else
    mapfile -t NAMES < <(find "$CORPUS_DIR" -name '*.bv' -printf '%f\n' | sed 's/\.bv$//' | sort)
    # Append benchmarks that have a golden (skip comment lines in bench.list).
    while read -r bname bbound; do
        case "$bname" in ''|\#*) continue ;; esac
        [ -f "$GOLDEN_DIR/$bname.stdout" ] && NAMES+=("$bname")
    done < "$BENCH_LIST"
fi

for NAME in "${NAMES[@]}"; do
    SRC="$CORPUS_DIR/$NAME.bv"
    GOLD="$GOLDEN_DIR/$NAME.stdout"
    BOUND=""
    if [ ! -f "$SRC" ]; then
        SRC="$ROOT/benchmarks/$NAME.bv"
        BOUND="$(awk -v n="$NAME" '$1 == n {print $2}' "$BENCH_LIST")"
    fi
    if [ ! -f "$SRC" ] || [ ! -f "$GOLD" ]; then
        echo "  SKIP $NAME (missing corpus/benchmark source or golden)"
        continue
    fi

    mkdir -p "$OUT_DIR"
    rm -f "$OUT_DIR/$NAME" "$OUT_DIR/$NAME.ll" "$OUT_DIR/$NAME.o"
    rm -f ~/.cache/briev-compiler/ffi/*.o /tmp/briev_rt*.o 2>/dev/null || true

    if ! env BOUND="${BOUND:-1000}" "$ROOT/target/release/brievc" build "$SRC" \
        --out "$OUT_DIR" --optimize-budget "${BUDGET:-256}" >/dev/null 2>&1; then
        echo "  FAIL $NAME (build failed)"
        FAIL=$((FAIL + 1)); FAILED_NAMES+=("$NAME")
        continue
    fi
    if [ ! -f "$OUT_DIR/$NAME" ]; then
        echo "  FAIL $NAME (no binary produced)"
        FAIL=$((FAIL + 1)); FAILED_NAMES+=("$NAME")
        continue
    fi

    BOUND="${BOUND:-1000}" "$OUT_DIR/$NAME" > "$OUT_DIR/$NAME.stdout" 2> "$OUT_DIR/$NAME.stderr"
    if cmp -s "$GOLD" "$OUT_DIR/$NAME.stdout"; then
        echo "  PASS $NAME"
        PASS=$((PASS + 1))
    else
        echo "  FAIL $NAME (stdout differs from golden)"
        FAIL=$((FAIL + 1)); FAILED_NAMES+=("$NAME")
    fi
done

echo "---"
echo "parity: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '  failed: %s\n' "${FAILED_NAMES[*]}"
    exit 1
fi