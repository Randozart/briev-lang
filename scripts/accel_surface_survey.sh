#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Accel-surface survey (2026-11-17).
#
# Every file that declares an `accel node` is compiled and its codegen warnings
# are read: an accel kernel that cannot be lowered to SPIR-V currently emits
# "kernel emission failed — … (staying CPU)" and runs on the CPU lane, so any
# timing for that program is a CPU number wearing a GPU label.
#
# Prints one line per file: STATUS<TAB>FILE<TAB>REASON, then a summary.
# STATUS is one of:
#   gpu     - kernel emitted (no fallback warning)
#   cpu     - fell back silently, reason captured
#   error   - the program did not build at all
#   skip    - not an accel-declaring file
#
# Usage: bash scripts/accel_surface_survey.sh [--root DIR] [--key NAME]
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BRIEVC="${BRIEVC:-/var/tmp/briev-xpu-target/release/brievc}"
OUT="${SURVEY_OUT:-/var/tmp/accel-survey}"
rm -rf "$OUT"; mkdir -p "$OUT"

n_gpu=0 n_cpu=0 n_err=0
printf '%s\n' "STATUS	FILE	REASON"
while IFS= read -r f; do
    key="$(printf '%s' "$f" | tr '/' '_' | sed 's/\.\(bv\|abv\)$//')"
    log="$OUT/$key.log"
    mkdir -p "$OUT/$key"
    BRIEV_HOME="$ROOT" timeout 900 "$BRIEVC" build "$f" \
        --stdlib-path lib --config-dir config --out "$OUT/$key" \
        >"$log" 2>&1
    rc=$?
    if grep -qiE "^error|parse error" "$log"; then
        n_err=$((n_err + 1))
        reason="$(grep -iE "^error" "$log" | head -1 | cut -c1-110)"
        printf 'error\t%s\t%s\n' "$f" "$reason"
        continue
    fi
    # 2026-11-17: classify by ARTIFACTS, never by the absence of a warning.
    # The warning path only covers "candidate registered, emission failed"; an
    # `accel` node whose body never became a kernel candidate emits nothing and
    # says nothing (benchmarks/recommender.bv: three `async accel node`s, zero
    # kernels, no diagnostic). A non-empty embedded blob (`.bv`) or a written
    # `.spv` (`.abv`) is the only proof that a kernel exists.
    blobs="$(grep -ohE "@briev_kernel_[A-Za-z0-9_]+ = private constant \[([2-9]|[1-9][0-9]+) x i8\]" \
        "$OUT/$key"/*.ll 2>/dev/null | sed -E 's/ =.*//' | tr '\n' ' ')"
    spvs="$(ls "$OUT/$key"/*.spv 2>/dev/null | xargs -r -n1 basename | tr '\n' ' ')"
    if grep -q "staying CPU" "$log"; then
        n_cpu=$((n_cpu + 1))
        reason="$(grep -o "kernel emission failed — [^(]*" "$log" | head -1 | cut -c1-100)"
        [ -z "$reason" ] && reason="$(grep -o "no TypeUniverse[^\"]*" "$log" | head -1)"
        printf 'cpu\t%s\t%s\n' "$f" "$reason"
        continue
    fi
    if [ -z "$blobs$spvs" ]; then
        n_cpu=$((n_cpu + 1))
        printf 'cpu\t%s\tno kernel emitted and no diagnostic (accel body never became a candidate)\n' "$f"
        continue
    fi
    n_gpu=$((n_gpu + 1))
    printf 'gpu\t%s\t%s%s\n' "$f" "$(printf '%s' "$spvs" | cut -c1-40)" "$(printf '%s' "$blobs" | cut -c1-60)"
done < <(grep -rl "accel node" tests/ benchmarks/ examples/ lib/ 2>/dev/null | sort)

printf '\n== accel surface ==\n'
printf 'gpu  %d   (kernel emitted)\n' "$n_gpu"
printf 'cpu  %d   (silent CPU fallback — these numbers are not GPU numbers)\n' "$n_cpu"
printf 'error %d  (did not build)\n' "$n_err"
printf 'logs: %s\n' "$OUT"
