#!/usr/bin/env bash
# ll_determinism_check.sh — S1 of the stability-remediation plan
# (docs/plans/2026-10-16-stability-remediation.md).
#
# House rule §4 mandates sorted iteration wherever HashMap iteration
# produces LLVM IR — SipHash seeds vary per process, so an unsorted site
# drifts the emitted registers and every downstream byte of the module.
# The rule existed unenforced; the 2026-10-16 reassigned-let fix shipped
# unsorted for a day before a double diff caught it. This gate enforces:
# every corpus program is compiled TWICE in separate processes and the
# two modules must be byte-identical.
#
# Usage:
#   bash benchmarks/ll_determinism_check.sh           # full corpus
#   bash benchmarks/ll_determinism_check.sh --quick   # 2 programs (per-commit)
#
# Exit 0 = all deterministic. Exit 1 = drift, named.
set -euo pipefail
cd "$(dirname "$0")/.."

BV=./target/release/brievc
RT=lib/runtime/briev_rt.c
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# name|source|extra args (env set separately below)
CORPUS=(
    "recommender|benchmarks/recommender.bv|"
    "nbody_accel|benchmarks/nbody_newton_accel.bv|env:BODYCOUNT=64"
    "multinode|benchmarks/recommender_corpus/recommender_multinode.bv|"
    "giantnode|benchmarks/recommender_corpus/recommender_giantnode.bv|"
    "handoff|benchmarks/recommender_corpus/handoff_repro3.bv|"
    "queue_drain|benchmarks/queue_drain.bv|"
    # 2026-10-17: pp-types exercises the defn-liveness fixpoint path
    # (glue/c.bv cast lanes → @str_to_int round-2 re-emission) — a standing
    # tripwire for IR stability of re-entrant emission.
    "pp-types|pp-types.bv|"
)
if [ "${1:-}" = "--quick" ]; then
    CORPUS=(
        "recommender|benchmarks/recommender.bv|"
        "multinode|benchmarks/recommender_corpus/recommender_multinode.bv|"
        "pp-types|pp-types.bv|"
    )
fi

fail=0
for row in "${CORPUS[@]}"; do
    IFS='|' read -r name src extra <<< "$row"
    envs=()
    if [[ "$extra" == env:* ]]; then
        IFS='=' read -r k v <<< "${extra#env:}"
        envs=("$k=$v")
    fi
    ok=1
    for pass in 1 2; do
        if ! env BOUND=1000 ${envs[@]+"${envs[@]}"} "$BV" build "$src" --llvm \
             --out "$TMP/pass$pass" --stdlib-path lib > "$TMP/$name.$pass.log" 2>&1; then
            echo "FAIL $name: compile pass $pass failed"
            sed -n '1,5p' "$TMP/$name.$pass.log"
            ok=0
            break
        fi
        mv "$TMP/pass$pass/$(basename "${src%.bv}").ll" "$TMP/$name.$pass.ll"
    done
    [ "$ok" = 1 ] || { fail=1; continue; }
    if cmp -s "$TMP/$name.1.ll" "$TMP/$name.2.ll"; then
        echo "OK   $name (byte-identical across processes)"
    else
        echo "FAIL $name: .ll drifts between processes — unsorted HashMap iteration \
producing IR (house rule §4). Find the emitter whose output differs and sort \
its iteration (sort_by on the key)."
        diff "$TMP/$name.1.ll" "$TMP/$name.2.ll" | head -10
        fail=1
    fi
done

if [ "$fail" = 0 ]; then
    echo "determinism: ALL DETERMINISTIC"
else
    echo "determinism: DRIFT DETECTED"
    exit 1
fi
