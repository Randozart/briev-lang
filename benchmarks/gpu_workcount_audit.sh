#!/usr/bin/env bash
# gpu_workcount_audit.sh — X1's first action, without a device.
#
# X1 (plan 2026-11-16-memory-and-xpu-finish-grind §X1) says: "verify the work
# count of both arms before treating 39.7 ms as the target; a wrong denominator
# would make the whole comparison meaningless." The work count is a property of
# the EMITTED DISPATCH GEOMETRY, so it is checkable on any host.
#
# The geometry lives in exactly one place — `GemmPlan::dispatch_geometry`
# (src/backend/spirv/gemm.rs), which the tier gate also consumes — so this
# script cannot drift from the compiler the way a hand-copied tile constant
# would. It runs the audit test and prints that test's table.
#
# Usage: bash benchmarks/gpu_workcount_audit.sh [extra cargo test args]
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

echo "=== GPU dispatch-geometry audit (X1 denominator) ==="
echo "source of truth: GemmPlan::dispatch_geometry (src/backend/spirv/gemm.rs)"
echo

if ! cargo test --lib x1_dispatch_geometry -- --nocapture "$@" 2>&1 \
    | grep -E '^(512|4096)\^3'; then
    echo "audit: the geometry test did not produce a table — build/test failure" >&2
    exit 1
fi

cat <<'EOF'

Reading the table:
  workgroups · tile_m · tile_n must equal m·n  → the grid visits every output
                                                 element exactly once (a folded
                                                 or short dispatch would fail).
  flops = 2·m·n·k                              → the only count a GEMM has.
  TFLOP/s = flops / measured wall clock        → compare with the OTHER arm of
                                                 the same A/B, not with a
                                                 reference-kernel figure.

Reference figures are NOT hardware bounds: the recorded tiled arm beats both
"ceilings" (1.46 f32-SIMD, 1.63 f16-storage TFLOP/s) at 512^3, where the window
is far too short for a launch artefact to explain a 9x gap. See
benchmarks/results/2026-11-17-x1-denominator-audit.md.
EOF
