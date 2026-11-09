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
    "${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run --accel=force benchmarks/intel_ptl/$bench.bv --stdlib-path lib --config-dir config --out g0out 2>&1'" | grep -E "G0_.*_BAD|compiled on" | tail -3 || true
}
run_gate() {
    local bench="$1" dev="$2"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run --accel=force benchmarks/intel_ptl/$bench.bv --stdlib-path lib --config-dir config --out g0out 2>&1'")
    local bad compiled
    # Fixtures name their own label (e.g. G0_SUM_BAD, G0_MINMAX_BAD); accept
    # the last must-be-zero counter line.
    bad=$(printf '%s\n' "$out" | sed -n 's/.*_BAD=\([-0-9]*\).*/\1/p' | tail -1)
    compiled=$(printf '%s\n' "$out" | grep -c "kernel '.*' compiled on '$dev'" || true)
    local t; t=$(printf '%s\n' "$out" | grep -o '# gpu_time: .*' | tail -1 || true)

    # Every GPU_<KIND> / CPU_<KIND> pair the fixture prints (SUM, MIN, MAX)
    # must agree; the fixture defines which kinds it reports.
    local mismatch=0 kinds="" k gpu cpu
    for k in SUM MIN MAX; do
        gpu=$(printf '%s\n' "$out" | sed -n "s/.*GPU_${k}=\([-0-9.eE+]*\).*/\1/p" | tail -1)
        cpu=$(printf '%s\n' "$out" | sed -n "s/.*CPU_${k}=\([-0-9.eE+]*\).*/\1/p" | tail -1)
        [ -z "$gpu" ] && [ -z "$cpu" ] && continue
        if [ "$gpu" = "$cpu" ]; then kinds="$kinds $k=ok"; else kinds="$kinds $k=DIFF"; fi
    done
    for pair in $kinds; do
        case "$pair" in *=DIFF) mismatch=1 ;; esac
    done

    echo "-- $bench on $dev: bad=${bad:-?}${kinds} ${t:-}"
    if [ "$compiled" -lt 1 ]; then
        echo "   FAIL: device did not compile the kernel (CPU fallback)"
        fail=1
    elif [ "${bad:-x}" != "0" ] || [ "$mismatch" -ne 0 ]; then
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
run_gate g0_minmax vulkan
echo "== g0: float fold approx/exact (§12 P1) =="
run_gate g0_fsum_approx vulkan
run_gate g0_fsum_exact vulkan

# Reported-lane variant: a timeout/HANG is printed, not fatal; a wrong result
# (BAD != 0 or GPU != CPU) still fails the gate.
run_gate_reported() {
    local bench="$1" dev="$2"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 90 $WORK/bin/brievc run --accel=force benchmarks/intel_ptl/$bench.bv --stdlib-path lib --config-dir config --out g0out 2>&1'" || true)
    local bad compiled
    bad=$(printf '%s\n' "$out" | sed -n 's/.*_BAD=\([-0-9]*\).*/\1/p' | tail -1)
    compiled=$(printf '%s\n' "$out" | grep -c "kernel '.*' compiled on '$dev'" || true)
    local mismatch=0 k gpu cpu kinds=""
    for k in SUM MIN MAX; do
        gpu=$(printf '%s\n' "$out" | sed -n "s/.*GPU_${k}=\([-0-9.eE+]*\).*/\1/p" | tail -1)
        cpu=$(printf '%s\n' "$out" | sed -n "s/.*CPU_${k}=\([-0-9.eE+]*\).*/\1/p" | tail -1)
        [ -z "$gpu" ] && [ -z "$cpu" ] && continue
        if [ "$gpu" = "$cpu" ]; then kinds="$kinds $k=ok"; else kinds="$kinds $k=DIFF"; fi
    done
    case "$kinds" in *DIFF) mismatch=1 ;; esac
    if [ "$compiled" -lt 1 ]; then
        echo "-- $bench on $dev: HANG/no output (bad=${bad:-?}) — reported"
    elif [ "${bad:-x}" != "0" ] || [ "$mismatch" -ne 0 ]; then
        echo "-- $bench on $dev: WRONG (bad=${bad:-?}${kinds})"
        fail=1
    else
        echo "-- $bench on $dev: bad=0${kinds}"
    fi
}

report_gemm_fallback() {
    local dev="$1"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 300 $WORK/bin/brievc run --accel=force benchmarks/intel_ptl/g0_gemm.bv --stdlib-path lib --config-dir config --out g0out 2>&1'" || true)
    local bad
    bad=$(printf '%s\n' "$out" | sed -n 's/.*_BAD=\([-0-9]*\).*/\1/p' | tail -1)
    if printf '%s\n' "$out" | grep -q "has no OpenCL projection"; then
        if [ "${bad:-x}" = "0" ]; then
            echo "-- gemm on $dev: CPU fallback (no OpenCL projection, Vulkan-only until G3), self-check ok"
        else
            echo "-- gemm on $dev: CPU fallback but self-check failed (bad=${bad:-?})"
            fail=1
        fi
    elif printf '%s\n' "$out" | grep -q "compiled on '$dev'"; then
        echo "-- gemm on $dev: kernel compiled (unexpected before G3 — record it)"
    else
        echo "-- gemm on $dev: no output (${bad:-?})"
        fail=1
    fi
}

# 2026-11-09 (plan intel-lane-ceilings-and-probe-cost): the five non-GEMM
# gates are REQUIRED on the Intel lanes too. GEMM stays Vulkan-only until G3
# (cooperative-matrix-on-IL): L0/NEO cannot consume the Vulkan image (IGC
# requires OpenCL.std), so the lane falls back to the CPU body — clean, but
# not a lane pass.
echo "== g0: levelzero lane (required) =="
for bench in g0_elem g0_sum g0_minmax g0_fsum_approx g0_fsum_exact; do
    run_gate "$bench" levelzero
done
# 2026-11-09: the OpenCL (NEO) lane is REPORTED, not required yet: the
# exact-fold kernel intermittently hangs in clFinish (~1/3 under node load;
# BUGS.md). Correct runs match the host reference; a HANG must be visible,
# never silent. Promote once the hang is fixed.
echo "== g0: opencl lane (reported; NEO hang tracked in BUGS.md) =="
for bench in g0_elem g0_sum g0_minmax g0_fsum_approx g0_fsum_exact; do
    run_gate_reported "$bench" opencl
done
echo "== g0: gemm on Intel lanes (reported; Vulkan-only until G3) =="
report_gemm_fallback levelzero
report_gemm_fallback opencl


echo "== g1.5 stage F: per-lane smoke (reported) =="
report_lane() {
    local dev="$1"
    local out
    out=$("${SSH[@]}" "bash -lc 'ulimit -s 65536; cd $WORK && BRIEV_HOME=$WORK BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=$dev timeout 240 $WORK/bin/brievc run benchmarks/intel_ptl/g1_lane_smoke.abv --stdlib-path lib --config-dir config --out g0out 2>&1'" || true)
    if printf '%s\n' "$out" | grep -q "compiled on '$dev'"; then
        local endpoints
        endpoints=$(printf '%s\n' "$out" | grep -o "dst\[0\] = i64 [0-9-]* | dst\[255\] = i64 [0-9-]*" | tail -1)
        if printf '%s\n' "$endpoints" | grep -q "dst\[0\] = i64 1 | dst\[255\] = i64 511"; then
            echo "-- $dev: kernel compiled + correct ($endpoints)"
        else
            echo "-- $dev: kernel compiled but WRONG OUTPUT (${endpoints:-no endpoints})"
            fail=1
        fi
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
