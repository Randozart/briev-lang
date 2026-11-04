#!/usr/bin/env bash
# hir_emit_ab.sh — H4 gate: AST-driven vs HIR-driven emission must be
# byte-identical. Each family flips only after this passes on the corpus.
#
# Usage:
#   bash benchmarks/hir_emit_ab.sh           # full corpus
#   bash benchmarks/hir_emit_ab.sh --quick   # 3 programs (per-commit)
#
# Currently the HIR flag is inert (H4.1 plumbing): emission is AST-driven in
# both passes, so this is the always-on tripwire for the flip.
set -euo pipefail
cd "$(dirname "$0")/.."

BV=${BV:-./target/release/brievc}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

CORPUS=(
    "recommender|benchmarks/recommender.bv|"
    "nbody_accel|benchmarks/nbody_newton_accel.bv|env:BODYCOUNT=64"
    "multinode|benchmarks/recommender_corpus/recommender_multinode.bv|"
    "queue_drain|benchmarks/queue_drain.bv|"
    "iter_chain|tests/tier1/iter_chain_multi.bv|"
    "pp-types|pp-types.bv|"
)
if [ "${1:-}" = "--quick" ]; then
    CORPUS=(
        "recommender|benchmarks/recommender.bv|"
        "queue_drain|benchmarks/queue_drain.bv|"
        "iter_chain|tests/tier1/iter_chain_multi.bv|"
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
    for mode in ast hir; do
        flag=()
        [ "$mode" = hir ] && flag=(BRIEV_HIR_EMIT=1)
        if ! env BOUND=1000 ${envs[@]+"${envs[@]}"} ${flag[@]+"${flag[@]}"} \
             "$BV" build "$src" --llvm --out "$TMP/$mode" --stdlib-path lib \
             > "$TMP/$name.$mode.log" 2>&1; then
            echo "FAIL $name: $mode compile failed"
            sed -n '1,5p' "$TMP/$name.$mode.log"
            ok=0
            break
        fi
        mv "$TMP/$mode/$(basename "${src%.bv}").ll" "$TMP/$name.$mode.ll"
    done
    [ "$ok" = 1 ] || { fail=1; continue; }
    if cmp -s "$TMP/$name.ast.ll" "$TMP/$name.hir.ll"; then
        echo "OK   $name (AST == HIR .ll)"
    else
        echo "FAIL $name: HIR-driven .ll differs from AST-driven .ll"
        diff "$TMP/$name.ast.ll" "$TMP/$name.hir.ll" | head -10
        fail=1
    fi
done

if [ "$fail" = 0 ]; then
    echo "hir-emit A/B: ALL BYTE-IDENTICAL"
else
    echo "hir-emit A/B: DIFFERENCES DETECTED"
    exit 1
fi
