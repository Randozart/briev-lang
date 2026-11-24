#!/usr/bin/env bash
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
# Cross-target IR-validity gate.
#
# INVARIANT: a program the compiler accepts for the native triple must also be
# accepted — with verifiable emitted IR — for the sibling triples of the same
# OS/ABI class. The corpus is SELF-CALIBRATING, with no hand-maintained list:
# a fixture that legitimately fails natively (the parse-error / termination /
# reactive-liveness probes) drops out on its own, so this check never needs a
# skip list that rots as the corpus grows.
#
# Why it exists: the Tier-2 "targets" step compiled ONE trivial fixture per
# triple, so an aarch64-only emission defect shipped unseen — the arena `brk`
# syscall asm had three operands and two input constraints, making EVERY
# arena/collection program unverifiable for aarch64 (BUGS.md 2026-11-24).
# `--llvm` is sufficient: the compiler runs LLVM verification on the emitted
# module, and no cross toolchain or linker is needed.
#
# Usage: bash scripts/cross_target_ir.sh [triple...]
#   default: aarch64-unknown-linux-gnu — the same Linux/ABI class as the native
#   triple. wasm32 / bare-metal triples are a DIFFERENT capability surface (no
#   syscalls, no libc); pass them explicitly to inspect a migration, but they
#   are intentionally not the default gate.
set -euo pipefail
cd "$(dirname "$0")/.."

BRIEVC="${BRIEVC:-./target/release/brievc}"
NATIVE="${NATIVE_TRIPLE:-x86_64-unknown-linux-gnu}"
TRIPLES=("$@")
if [ "${#TRIPLES[@]}" -eq 0 ]; then TRIPLES=(aarch64-unknown-linux-gnu); fi

if [[ ! -x "$BRIEVC" ]]; then
    echo "cross-target-ir: $BRIEVC is not executable; build the compiler first" >&2
    exit 1
fi

OUT="${CROSS_TARGET_ARTIFACTS:-target/cross-target-ir}"
rm -rf "$OUT"
mkdir -p "$OUT"

fail=0
for triple in "${TRIPLES[@]}"; do
    checked=0
    triple_fail=0
    for src in tests/tier1/*.bv tests/tier2/*.bv; do
        [ -f "$src" ] || continue
        name="$(basename "$src" .bv)"
        # Self-calibration: the native triple decides membership in the corpus.
        if ! "$BRIEVC" build "$src" --llvm --out "$OUT/native/$name" \
                >"$OUT/$name.native.log" 2>&1; then
            continue
        fi
        checked=$((checked + 1))
        if ! "$BRIEVC" build "$src" --llvm --triple "$triple" --out "$OUT/$triple/$name" \
                >"$OUT/$name.$triple.log" 2>&1; then
            triple_fail=$((triple_fail + 1))
            echo "FAIL $triple $name ($src)" >&2
            grep -m2 -E 'error:|why:|invalid' "$OUT/$name.$triple.log" | sed 's/^/    /' >&2 || true
        fi
    done
    echo "cross-target-ir: $triple — $checked programs checked (native $NATIVE accepted), $triple_fail failed"
    fail=$((fail + triple_fail))
done

if [ "$fail" -ne 0 ]; then
    echo "cross-target-ir: FAIL ($fail) — emitted IR is not valid for every sibling triple; logs in $OUT" >&2
    exit 1
fi
echo "cross-target-ir: PASS"
