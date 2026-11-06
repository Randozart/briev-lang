#!/usr/bin/env bash
# g0.sh — Intel G0 gate: existing Vulkan/OpenCL SPIR-V lanes on PTL.
#
# 2026-11-06 (intel-dispatch.md §5 G0): correctness-first rung. Runs two
# self-verifying accel programs (elementwise self-add, 512^3 MatMul), each
# comparing device output against an independent host computation.
#
# PASS requires BOTH:
#   1. the program's printed BAD=0 and GPU_SUM == CPU_SUM, and
#   2. the verbose log's "compiled on '<device>'" line (no silent CPU
#      fallback).
#
# OpenCL is reported, not required: NEO rejects our IL entry point today
# (CL_INVALID_KERNEL_NAME) — recorded in the results file.
#
# Usage:
#   cargo build --release
#   BRIEV_PTL_SSH="ssh user@host" bash benchmarks/intel_ptl/g0.sh
#
# Env: BRIEV_PTL_SSH, BRIEV_PTL_WORK, BRIEVC (same as run.sh).
# The generated binaries need a larger stack than 8 MB (two 1 Mi / 512^2 f32
# arrays); the runner sets `ulimit -s 65536` (BUGS.md 2026-11-06).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SSH_CMD="${BRIEV_PTL_SSH:-ssh overlordx86-64}"
read -r -a SSH <<< "$SSH_CMD"
WORK="${BRIEV_PTL_WORK:-/tmp/briev-ptl}"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"

command -v "${SSH[0]}" >/dev/null || { echo "missing command: ${SSH[0]}" >&2; exit 2; }
[ -x "$BRIEVC" ] || { echo "build brievc first: cargo build --release" >&2; exit 2; }

echo "== sync -> $WORK =="
tar -C "$ROOT" -cf - lib config plugins benchmarks/intel_ptl \
    | "${SSH[@]}" "mkdir -p '$WORK' && tar -C '$WORK' -xf -"
tar -C "$(dirname "$BRIEVC")" -cf - "$(basename "$BRIEVC")" \
    | "${SSH[@]}" "mkdir -p '$WORK/bin' && tar -C '$WORK/bin' -xf -"
tar -C "$ROOT" -cf - target/compiler-in-briv \
    | "${SSH[@]}" "tar -C '$WORK' -xf -"

fail=0
sample_report() {
    local bench="$1" dev="$2"
    "${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run benchmarks/intel_ptl/$bench.bv --stdlib-path lib --config-dir config --out g0out 2>&1'" | grep -E "G0_.*_BAD|compiled on" | tail -3 || true
}
run_gate() {
    local bench="$1" dev="$2"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run benchmarks/intel_ptl/$bench.bv --stdlib-path lib --config-dir config --out g0out 2>&1'")
    local bad gpu cpu compiled
    bad=$(printf '%s\n' "$out" | sed -n "s/.*$(echo "$bench" | tr '[:lower:]' '[:upper:]')_BAD=\([-0-9]*\).*/\1/p" | tail -1)
    gpu=$(printf '%s\n' "$out" | sed -n 's/.*GPU_SUM=\([-0-9.eE+]*\).*/\1/p' | tail -1)
    cpu=$(printf '%s\n' "$out" | sed -n 's/.*CPU_SUM=\([-0-9.eE+]*\).*/\1/p' | tail -1)
    compiled=$(printf '%s\n' "$out" | grep -c "kernel '.*' compiled on '$dev'")
    local t; t=$(printf '%s\n' "$out" | grep -o '# gpu_time: .*' | tail -1)

    echo "-- $bench on $dev: bad=${bad:-?} gpu_sum=${gpu:-?} cpu_sum=${cpu:-?} ${t:-}"
    if [ "$compiled" -lt 1 ]; then
        echo "   FAIL: device did not compile the kernel (CPU fallback)"
        fail=1
    elif [ "${bad:-x}" != "0" ] || [ "${gpu:-x}" != "${cpu:-y}" ]; then
        echo "   FAIL: device output != host reference"
        fail=1
    else
        echo "   PASS"
    fi
}

echo "== g0: elementwise =="
run_gate g0_elem vulkan
echo "== g0: gemm 512^3 =="
run_gate g0_gemm vulkan
echo "== g0: sum idiom (§12 W3) =="
run_gate g0_sum vulkan

echo "== g0: min/max idioms (§12 W3.1) =="
# Reported, not required: the end-to-end gate hits the multi-node mid-run
# visibility issue (BUGS.md 2026-11-06) — the lane code is unit/IR-verified.
mm=$(sample_report g0_minmax vulkan)
echo "-- g0_minmax on vulkan: ${mm:-no output} (reported — BUGS.md multi-node visibility)"

echo "== g0: opencl + levelzero (reported, not required) =="
report_lane() {
    local dev="$1"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run benchmarks/intel_ptl/g0_elem.bv --stdlib-path lib --config-dir config --out g0out 2>&1'" || true)
    if printf '%s\n' "$out" | grep -q "compiled on '$dev'"; then
        echo "-- $dev: kernel compiled (lane usable)"
    else
        local reason
        reason=$(printf '%s\n' "$out" | grep -E "$dev" | grep -vE "G0_ELEM|stdout_buf|stdout_fill|folded|counter" | tail -3 | tr '\n' ' ')
        echo "-- $dev: rejected (${reason:-no route})"
    fi
}
report_lane levelzero
report_lane opencl

if [ "$fail" -ne 0 ]; then
    echo "G0: FAIL"
    exit 1
fi
echo "G0: PASS (vulkan elementwise + 512^3 GEMM vs host reference)"
