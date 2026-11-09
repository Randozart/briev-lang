#!/usr/bin/env bash
# §12 W3 parity A/B (retirement-gate evidence): the sum idiom lane versus
# the general lowering, identical work.
#
#   lane    = g0_sum.bv          (const K % 32 -> sum_lane fires)
#   general = g0_sum_general.bv  (runtime K    -> why-not LengthNotConst)
#
# Both compute the same rows*K Int row sums with K=1024 and self-verify
# (BAD=0 is required for a sample to count). Timing is the driver's
# per-dispatch timestamp (# gpu_time, ms) — dispatch-only, not wall clock.
#
# Usage: BRIEV_PTL_SSH="ssh <node>" bash benchmarks/intel_ptl/sum_ab.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SSH_CMD="${BRIEV_PTL_SSH:-ssh overlordx86-64}"
read -r -a SSH <<< "$SSH_CMD"
WORK="${BRIEV_PTL_WORK:-/tmp/briev-ptl}"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
REPS="${BRIEV_PTL_REPS:-10}"

tar -C "$ROOT" -cf - lib config plugins benchmarks/intel_ptl \
    | "${SSH[@]}" "mkdir -p '$WORK' && tar -C '$WORK' -xf -"
tar -C "$(dirname "$BRIEVC")" -cf - "$(basename "$BRIEVC")" \
    | "${SSH[@]}" "mkdir -p '$WORK/bin' && tar -C '$WORK/bin' -xf -"
tar -C "$ROOT" -cf - target/compiler-in-briv \
    | "${SSH[@]}" "tar -C '$WORK' -xf -"

sample() {
    local file="$1"
    "${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && for i in \$(seq 1 $REPS); do K=1024 BRIEV_HOME=$WORK BRIEV_ACCEL_DEVICE=vulkan $WORK/bin/brievc run --accel=force benchmarks/intel_ptl/$file --stdlib-path lib --config-dir config --out g0out 2>&1 | grep -E \"G0_SUM_BAD|gpu_time\"; done'"
}

report() {
    local label="$1" out="$2"
    local bad n times
    bad=$(printf '%s\n' "$out" | grep -c "G0_SUM_BAD=0" || true)
    n=$(printf '%s\n' "$out" | grep -c "gpu_time" || true)
    times=$(printf '%s\n' "$out" | sed -n 's/^# gpu_time: \(.*\) ms/\1/p' | sort -n)
    local median
    median=$(printf '%s\n' "$times" | awk 'NF{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}')
    echo "-- $label: samples=$n correct=$bad median_gpu_ms=${median:-?}"
    printf '%s\n' "$times" | awk '{printf "%s%s", (NR>1?" ":""), $1} END{print ""}' | sed 's/^/   /'
    if [ "$bad" -ne "$n" ] || [ "$n" -eq 0 ]; then
        echo "   GATE FAIL: every sample must print G0_SUM_BAD=0"
        return 1
    fi
}

fail=0
echo "== §12 W3 parity A/B (K=1024, reps=$REPS) =="
report "lane    (g0_sum)" "$(sample g0_sum.bv)" || fail=1
report "general (g0_sum_general)" "$(sample g0_sum_general.bv)" || fail=1
if [ "$fail" -ne 0 ]; then echo "PARITY A/B: FAIL"; exit 1; fi
echo "PARITY A/B: OK (compare the medians above; the lane retires when general reaches parity)"
