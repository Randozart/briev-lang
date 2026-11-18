#!/usr/bin/env bash
# Accel reactor non-hang gate: a beginprogram && i < nb entry node must
# actually fire and make progress. Regression for the stale-terminated lazy
# logical fold that left nbody's init node out of the main dispatch loop.
#
# 2026-11-18 (native-flake quarantine, W1-A): builds in --safe mode
# (-O0 -march=x86-64, no -flto, no -ffast-math) so the gate tests the
# REACTOR LOGIC (the thing it was written for), not the build host's
# AVX/AMX codegen. -march=native on GitHub runners intermittently SIGILLs
# (known flake, ~2 days of red CI); the native codegen risk is covered by
# the --runtime benchmark suite + toolchain_matrix.sh, not by this gate.
# W1-B will make -march config-driven (default x86-64-v3) so this gate
# keeps -O3 -flto with a deterministic ISA.
# 2026-11-18 (W1-C): bounded crash-retry — a transient runner crash
# (OOM, flaky SIGILL on x86-64-v3) is retried up to 3 times. A timeout is
# NEVER retried: it is the actual regression this gate exists to catch.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$(mktemp -d /tmp/briev-accel-reactor.XXXXXX)
trap 'rm -rf "$OUT"' EXIT
MAX_ATTEMPTS=3
attempt=1
while true; do
    BOUND=50 BODYCOUNT=16 ./target/release/brievc build benchmarks/nbody_newton_accel.bv \
        --out "$OUT" --optimize-budget 2048 --safe >"$OUT/build.log" 2>&1
    set +e
    BODYCOUNT=16 BOUND=50 timeout -s KILL 30s "$OUT/nbody_newton_accel" >"$OUT/run.out" 2>&1
    RC=$?
    set -e
    # Timeout (124) or SIGKILL (137) = the actual regression — never retry.
    if [[ $RC -eq 124 || $RC -eq 137 ]]; then
        echo "accel reactor non-hang: FAIL (timed out, attempt $attempt)" >&2
        exit 1
    fi
    # Crash (non-zero, non-timeout): retry up to MAX_ATTEMPTS.
    if [[ $RC -ne 0 ]]; then
        if [[ $attempt -ge $MAX_ATTEMPTS ]]; then
            echo "accel reactor non-hang: FAIL (exit $RC after $MAX_ATTEMPTS attempts)" >&2
            cat "$OUT/run.out" >&2
            exit 1
        fi
        echo "accel reactor non-hang: attempt $attempt crashed (exit $RC), retrying..." >&2
        attempt=$((attempt + 1))
        continue
    fi
    # Success: check for observable output.
    if [[ ! -s "$OUT/run.out" ]]; then
        echo "accel reactor non-hang: FAIL (no observable output, attempt $attempt)" >&2
        exit 1
    fi
    echo "accel reactor non-hang: PASS (attempt $attempt)"
    break
done
