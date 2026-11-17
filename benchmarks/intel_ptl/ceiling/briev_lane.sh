#!/usr/bin/env bash
# Briev lane numbers at the ceiling shapes: median device time over REPS
# process runs (the LAST "# gpu_time" line per run is the measured dispatch).
#
# 2026-11-17: builds + LINKS the emitted `_runner.c` and times that binary.
# `brievc run` must NOT be used here: its generic driver dispatches a different
# work-item count (16x the tile grid for coopmat), which recorded a 26 ms kernel
# as 368 ms. See benchmarks/results/2026-11-17-ptl-f16-tier.md §0.
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
    # 2026-11-17 (PTL night): measure the artifact a program actually ships, not
    # `brievc run`. `run` links a GENERIC driver that dispatches its own work-item
    # count — it launches the coopmat tier with 16x the tile grid, which is how a
    # 26 ms kernel was recorded as 368 ms and a 9x "regression" was believed for
    # two weeks. `build` emits `<name>_runner.c`: link THAT and run it.
    name=$(basename "$prog" .abv)
    out="/tmp/briev-ceil-$name"
    rm -rf "$out"; mkdir -p "$out"
    if ! "$BRIEVC" build "$prog" --stdlib-path lib --config-dir "$CONFIG_DIR" \
            --out "$out" >/dev/null 2>&1; then
        echo "$name lane=$lane BUILD-FAILED"
        continue
    fi
    if ! ( cd "$out" && clang -O2 -I. -o bin "${name}_runner.c" \
            -L. -lbriev_accel_rt -lbriev_gpu_rt -ldl -lm -lpthread ) >/dev/null 2>&1; then
        echo "$name lane=$lane LINK-FAILED"
        continue
    fi
    times=()
    for _ in $(seq 1 "$reps"); do
        # The LAST `# gpu_time` per process is the steady-state dispatch; the first
        # includes residency seeding.
        t=$(BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE="$lane" timeout 600 "$out/bin" 2>&1 \
            | sed -n 's/.*# gpu_time: \([0-9.]*\) ms/\1/p' | tail -1 || true)
        [ -n "$t" ] && times+=("$t")
    done
    if [ "${#times[@]}" -eq 0 ]; then
        echo "$name lane=$lane HANG/no-timing"
        continue
    fi
    median=$(printf '%s\n' "${times[@]}" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
    echo "$name lane=$lane ms=$median n=${#times[@]}"
done
