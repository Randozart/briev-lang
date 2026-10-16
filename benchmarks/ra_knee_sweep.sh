#!/usr/bin/env bash
# ra_knee_sweep.sh — absolute-limit addendum part 2 (2026-10-16,
# docs/plans/2026-10-16-cpp-vs-briev-contributor-findings.md).
#
# Measures where inlined-function cost goes superlinear on THIS toolchain,
# so `node_inline_weight_threshold` is a re-derivable measurement instead of
# a corpus anecdote. For each rung K (synthetic node with K independent
# lets; body weight ≈ 4K + 8) the program carries `#inline`, forcing the
# concentrated shape (body inlined into the dispatch loop) so the P2 valve
# never masks the mechanism being measured. Metric: median-of-3 clang -O3
# (harness flags) wall time per rung + emitted-object size.
#
# The knee = the rung where time jumps ~2x over the linear extrapolation of
# the small rungs. Derived valve := knee weight ÷ 4 (safety factor).
# Record: date, clang version, table, knee, derived value → append to
# benchmarks/results/2026-10-16-recommender-cpp-vs-briev.md.
#
# Usage: bash benchmarks/ra_knee_sweep.sh
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=${OUT_DIR:-/tmp/briev_ra_knee}
mkdir -p "$OUT"
: > "$OUT/knee_results.txt"

BV=./target/release/brievc
RT=lib/runtime/briev_rt.c
rungs=(50 100 200 400 800 1600 3200 6400 12800)

echo "clang: $(clang --version | head -1)"
echo "  weight  clang_ms   obj_bytes"
for K in "${rungs[@]}"; do
    src="$OUT/knee_$K.bv"
    {
        # The ### override pins node_inline_weight_threshold above every
        # rung: the sweep measures the CONCENTRATED shape (body inlined into
        # the dispatch loop) at every size — the P2 valve must never mask
        # the mechanism under test.
        echo '###'
        echo 'node_inline_weight_threshold: 999999999;'
        echo '###'
        echo '!> accel: try_all;'
        echo 'let count: Int = 0;'
        echo 'let a: Int = 1;'
        echo 'let b: Int = 2;'
        echo 'let phase: Int = 0;'
        # Chained lets: every value feeds the next (serial dependence), so
        # DCE cannot fold the body away and the register-pressure mechanism
        # under test (long live chains through the inlined body) is real.
        # Three-node phase chain (the recommender.bv shape): beginprogram
        # nodes must be single-pass [true], the mid node declares the
        # completion the liveness pass requires, `halt` is reserved.
        echo 'async node run [beginprogram && phase == 0][true] {'
        echo '    let v0: Int = a + b;'
        for ((i = 1; i < K; i++)); do
            echo "    let v$i: Int = v$((i - 1)) + $i;"
        done
        echo "    count = v$((K - 1));"
        echo '    phase = 1;'
        echo '    term;'
        echo '};'
        echo 'async node mid [phase == 1][phase == 2] {'
        echo '    phase = 2;'
        echo '    term;'
        echo '};'
        echo 'async node stop [phase == 2][true] {'
        echo '    endprogram println!("{}", count);'
        echo '};'
    } > "$src"

    "$BV" build "$src" --llvm --out "$OUT" --stdlib-path lib > /dev/null 2>&1

    best=""
    for _ in 1 2 3; do
        t0=$(date +%s%N)
        clang -O3 -flto -march=native -ffast-math -fdata-sections \
            -ffunction-sections -Wl,--gc-sections \
            "$OUT/knee_$K.ll" "$RT" -ldl -o "$OUT/knee_$K" 2> /dev/null
        t1=$(date +%s%N)
        ms=$(( (t1 - t0) / 1000000 ))
        # NOT `[ ] || [ ] && x=` — a non-improving timing would return 1 and
        # `set -e` would kill the sweep at the first non-best iteration.
        if [ -z "$best" ] || [ "$ms" -lt "$best" ]; then best=$ms; fi
    done
    obj=$(stat -c %s "$OUT/knee_$K")
    printf "%8d %8d %12d\n" "$K" "$best" "$obj" | tee -a "$OUT/knee_results.txt"
done
echo ""
echo "table: weight ≈ 4K+8; knee = first ~2x jump over linear extrapolation;"
echo "derived valve = knee weight ÷ 4. Append to the results file with the"
echo "clang version line above."
