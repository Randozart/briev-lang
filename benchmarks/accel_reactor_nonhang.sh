#!/usr/bin/env bash
# Accel reactor non-hang gate: a beginprogram && i < nb entry node must
# actually fire and make progress. Regression for the stale-terminated lazy
# logical fold that left nbody's init node out of the main dispatch loop.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(mktemp -d /tmp/briev-accel-reactor.XXXXXX)
trap 'rm -rf "$OUT"' EXIT
BOUND=50 BODYCOUNT=16 ./target/release/brievc build benchmarks/nbody_newton_accel.bv \
    --out "$OUT" --optimize-budget 2048 >"$OUT/build.log" 2>&1
set +e
BODYCOUNT=16 BOUND=50 timeout -s KILL 30s "$OUT/nbody_newton_accel" >"$OUT/run.out" 2>&1
RC=$?
set -e
if [[ $RC -eq 124 || $RC -eq 137 ]]; then
    echo "accel reactor non-hang: FAIL (timed out)" >&2
    exit 1
fi
if [[ $RC -ne 0 ]]; then
    echo "accel reactor non-hang: FAIL (exit $RC)" >&2
    cat "$OUT/run.out" >&2
    exit 1
fi
if [[ ! -s "$OUT/run.out" ]]; then
    echo "accel reactor non-hang: FAIL (no observable output)" >&2
    exit 1
fi
echo "accel reactor non-hang: PASS"
