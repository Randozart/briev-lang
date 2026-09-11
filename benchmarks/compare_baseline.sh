#!/usr/bin/env bash
# Compare a benchmark between baseline worktree and current worktree.
# Usage: bash benchmarks/compare_baseline.sh <benchmark_name>
#
# Example:
#   bash benchmarks/compare_baseline.sh nbody_newton
#
# Runs each binary 5 times and prints average times.
# Returns non-zero if current is >10% slower than baseline.

set -euo pipefail
cd "$(dirname "$0")/.."

NAME="${1:-nbody_newton}"
BASELINE_DIR="../briev-compiler-baseline"
BOUND="${BOUND:-500000000}"
RUNTIME_LIMIT="${RUNTIME_LIMIT:-120}"
CURRENT_DIR="."
RUNS=5

if [ ! -d "$BASELINE_DIR" ]; then
    echo "ERROR: Baseline worktree not found at $BASELINE_DIR"
    echo "Create it with: git worktree add $BASELINE_DIR 334a168"
    exit 1
fi

echo "=== Comparing $NAME ==="
echo "Baseline: $(cd $BASELINE_DIR && git rev-parse --short HEAD)"
echo "Current:  $(git rev-parse --short HEAD)"
echo ""

# Ensure both binaries exist
if [ ! -f "$BASELINE_DIR/benchmarks/$NAME" ]; then
    echo "Building baseline binary..."
    cd "$BASELINE_DIR"
    # 2026-08-01: binary renamed briev-compiler -> brievc long ago; the stale
    # name made baseline builds fail silently and the comparison compare only
    # the current binary (or error). Must match the harness's brievc binary.
    BOUND="$BOUND" ./target/release/brievc build "benchmarks/${NAME}.bv" --out benchmarks 2>&1 | tail -1
    cd "$CURRENT_DIR"
fi
if [ ! -f "benchmarks/$NAME" ]; then
    echo "Building current binary..."
    BOUND="$BOUND" ./target/release/brievc build "benchmarks/${NAME}.bv" --out benchmarks 2>&1 | tail -1
fi

time_binary() {
    local dir="$1"
    local name="$2"
    local total=0
    for i in $(seq 1 $RUNS); do
        # Program stdout → /dev/null: merged-stream ordering otherwise lets
        # program output shadow the `time` line (fasta prints its sequence).
        local t=$(timeout "$RUNTIME_LIMIT" bash -c "cd '$dir' && export BOUND='$BOUND'; export TIMEFORMAT='%3R'; time ./benchmarks/$name > /dev/null" 2>&1 | tail -1)
        # Replace comma with dot for locale-independent parsing
        t="${t/,/.}"
        total=$(awk -v a="$total" -v b="$t" 'BEGIN { printf "%.4f", a + b }')
        echo "  Run $i: ${t}s"
    done
    awk -v a="$total" -v r="$RUNS" 'BEGIN { printf "%.4f", a / r }' 
}

echo ""
echo "--- Baseline ---"
baseline_avg=$(time_binary "$BASELINE_DIR" "$NAME" | tail -1)

echo ""
echo "--- Current ---"
current_avg=$(time_binary "$CURRENT_DIR" "$NAME" | tail -1)

echo ""
echo "--- Result ---"
echo "Baseline avg: ${baseline_avg}s"
echo "Current avg:  ${current_avg}s"
ratio=$(awk -v c="$current_avg" -v b="$baseline_avg" 'BEGIN { if (b > 0) printf "%.4f", c / b; else printf "1.0" }')
echo "Ratio: $ratio (current/baseline)"

if [ "$(awk -v r="$ratio" 'BEGIN { print (r > 1.10) ? 1 : 0 }')" = "1" ]; then
    echo "WARNING: Current is >10% slower than baseline!"
    exit 1
else
    echo "OK: Within tolerance."
    exit 0
fi
