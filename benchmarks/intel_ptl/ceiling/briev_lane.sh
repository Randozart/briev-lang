#!/usr/bin/env bash
# Briev lane numbers at the ceiling shapes: median device time over REPS
# process runs (the LAST "# gpu_time" line per run is the measured dispatch).
# Usage: briev_lane.sh <lane:vulkan|levelzero|opencl> [reps] [prog.abv...]
set -euo pipefail
lane="${1:?lane}"; reps="${2:-5}"; shift 2 || true
cd "$(dirname "$0")/../../.."
[ "$#" -gt 0 ] || set -- benchmarks/intel_ptl/ceiling/ceil_*.abv
export BRIEV_HOME="$PWD" BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE="$lane"
# Runtime bounds: a const-bound counter is precomputed and never dispatched.
export CEIL_N="${CEIL_N:-1048576}" CEIL_ROWS="${CEIL_ROWS:-4096}" CEIL_GO="${CEIL_GO:-1}"
BRIEVC="${BRIEVC:-$( [ -x bin/brievc ] && echo bin/brievc || echo target/release/brievc )}"
for prog in "$@"; do
    times=()
    for _ in $(seq 1 "$reps"); do
        t=$(timeout 600 "$BRIEVC" run "$prog" --stdlib-path lib \
              --config-dir config --out /tmp/briev-ceil 2>&1 \
            | sed -n 's/.*# gpu_time: \([0-9.]*\) ms/\1/p' | tail -1 || true)
        [ -n "$t" ] && times+=("$t")
    done
    if [ "${#times[@]}" -eq 0 ]; then
        echo "$(basename "$prog") lane=$lane HANG/no-timing"
        continue
    fi
    median=$(printf '%s\n' "${times[@]}" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
    echo "$(basename "$prog") lane=$lane ms=$median n=${#times[@]}"
done
