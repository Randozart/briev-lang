#!/usr/bin/env bash
# Briev lane numbers at the ceiling shapes: median device time over REPS
# process runs (the LAST "# gpu_time" line per run is the measured dispatch).
# Usage: briev_lane.sh <lane:vulkan|levelzero|opencl> [reps] [prog.abv...]
#        CEIL_CONFIG_DIR=<dir>  overrides --config-dir (default: config)
set -euo pipefail
lane="${1:?lane}"; reps="${2:-5}"; shift 2 || true
cd "$(dirname "$0")/../../.."
[ "$#" -gt 0 ] || set -- benchmarks/intel_ptl/ceiling/ceil_*.abv
export BRIEV_HOME="$PWD" BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE="$lane"
# Runtime bounds: a const-bound counter is precomputed and never dispatched.
export CEIL_N="${CEIL_N:-1048576}" CEIL_ROWS="${CEIL_ROWS:-4096}" CEIL_GO="${CEIL_GO:-1}"
BRIEVC="${BRIEVC:-$( [ -x bin/brievc ] && echo bin/brievc || echo target/release/brievc )}"
# 2026-11-17 (PTL night queue P1): the config dir is overridable so a lane knob can
# be A/B'd (copy config/ to a scratch dir, flip one line, point CEIL_CONFIG_DIR at
# it). Default keeps every earlier record reproducible byte-for-byte.
CONFIG_DIR="${CEIL_CONFIG_DIR:-config}"
for prog in "$@"; do
    times=()
    for _ in $(seq 1 "$reps"); do
        t=$(timeout 600 "$BRIEVC" run "$prog" --stdlib-path lib \
              --config-dir "$CONFIG_DIR" --out /tmp/briev-ceil 2>&1 \
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
