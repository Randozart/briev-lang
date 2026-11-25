#!/usr/bin/env bash
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
# Cross-target IR-validity gate.
#
# INVARIANT: a program the compiler accepts for the native triple must also be
# accepted — with verifiable emitted IR — for every target we SHIP. The corpus is
# SELF-CALIBRATING, with no hand-maintained list: a fixture that legitimately
# fails natively (the parse-error / termination / reactive-liveness probes) drops
# out on its own, so this check never needs a skip list that rots as the corpus
# grows.
#
# Why it exists: the Tier-2 "targets" step compiled ONE trivial fixture per
# triple, so an aarch64-only emission defect shipped unseen — the arena `brk`
# syscall asm had three operands and two input constraints, making EVERY
# arena/collection program unverifiable for aarch64 (BUGS.md 2026-11-24).
# `--llvm` is sufficient: the compiler runs LLVM verification on the emitted
# module, and no cross toolchain or linker is needed.
#
# 2026-11-25 (wasm32 supported-target plan, P0): wasm32 is a SHIPPED target — it
# is the web build, what a `.rbv` program compiles to — so it is gated here like
# any other. Three things changed and the old assumptions are withdrawn:
#   * the corpus includes `*.rbv` (`examples/`, `tests/`). Before this, NO `.rbv`
#     program was in any cross-target sweep, so the web path had never been
#     IR-verified against its own target by a broad gate.
#   * a triple's failure count is compared against a DECLARED baseline in
#     `config/cross-target-baseline.toml`. A count may only decrease; the file is
#     the record of how much is left, and `cargo test --test cross_target_baseline`
#     fails when reality is better than the record (a stale record is a lie).
#   * the sweep runs in PARALLEL (one process per program). Sequential it took
#     ~5 min for 100 programs, which is why it was never in PR CI; parallel it is
#     well inside the gate budget.
#
# Usage: bash scripts/cross_target_ir.sh [triple...]
#   default: aarch64-unknown-linux-gnu — the same Linux/ABI class as the native
#   triple. Pass wasm32-unknown-wasi explicitly to check the web target.
#   JOBS=<n> overrides the parallelism (default: nproc).
set -euo pipefail
cd "$(dirname "$0")/.."

BRIEVC="${BRIEVC:-./target/release/brievc}"
NATIVE="${NATIVE_TRIPLE:-x86_64-unknown-linux-gnu}"
BASELINE="${CROSS_TARGET_BASELINE:-config/cross-target-baseline.toml}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"
TRIPLES=("$@")
if [ "${#TRIPLES[@]}" -eq 0 ]; then TRIPLES=(aarch64-unknown-linux-gnu); fi

if [[ ! -x "$BRIEVC" ]]; then
    echo "cross-target-ir: $BRIEVC is not executable; build the compiler first" >&2
    exit 1
fi

# Declared base path, for the relative paths in the failure list.
BRIEVC="$(cd "$(dirname "$BRIEVC")" && pwd)/$(basename "$BRIEVC")"
ROOT="$(pwd)"

OUT="${CROSS_TARGET_ARTIFACTS:-target/cross-target-ir}"
# One sweep per artifact directory. Two concurrent sweeps `rm -rf` each other's
# per-program result files mid-flight, which does not error — it produces a
# SMALLER failure count than reality (observed 2026-11-25: 97 checked / 8 failed
# instead of 129 / 27). A gate that can under-report is worse than a slow one, so
# claim the directory and refuse to share it.
if [ -f "$OUT/.claim" ]; then
    owner="$(cat "$OUT/.claim" 2>/dev/null || true)"
    if [ -n "$owner" ] && kill -0 "$owner" 2>/dev/null; then
        echo "cross-target-ir: $OUT is in use by a running sweep (pid $owner)." >&2
        echo "  Two concurrent sweeps under-report failures because they delete each" >&2
        echo "  other's result files. Wait for it, or set CROSS_TARGET_ARTIFACTS." >&2
        exit 1
    fi
fi
rm -rf "$OUT"
mkdir -p "$OUT"
echo "$$" > "$OUT/.claim"
trap 'rm -f "$OUT/.claim"' EXIT

# The corpus: compiler-feature fixtures plus every shipped web-path program.
# Sorted and deduplicated so the artifact names are deterministic.
mapfile -t CORPUS < <(
    {
        ls tests/tier1/*.bv tests/tier2/*.bv 2>/dev/null || true
        find examples tests -name '*.rbv' -not -path '*/archive/*' 2>/dev/null || true
    } | sort -u
)
if [ "${#CORPUS[@]}" -eq 0 ]; then
    echo "cross-target-ir: no corpus programs found" >&2
    exit 1
fi

# A path becomes one flat artifact name; `/` and `.` become `_` so two files that
# share a basename cannot overwrite each other's logs.
slug() { printf '%s' "$1" | sed 's|/|_|g; s|\.|_|g'; }

# One program: native acceptance decides membership, then the sibling triple must
# produce verifiable IR. Writes PASS / SKIP / FAIL to a per-program result file so
# the parallel fan-out needs no shared counter.
run_one() {
    local src="$1" triple="$2"
    local name
    name="$(slug "$src")"
    if ! "$BRIEVC" build "$src" --llvm --out "$OUT/native/$name" \
            >"$OUT/$name.native.log" 2>&1; then
        printf 'SKIP\n' >"$OUT/$name.result"
        return 0
    fi
    if "$BRIEVC" build "$src" --llvm --triple "$triple" --out "$OUT/$triple/$name" \
            >"$OUT/$name.$triple.log" 2>&1; then
        printf 'PASS\n' >"$OUT/$name.result"
    else
        printf 'FAIL\n' >"$OUT/$name.result"
        printf '%s\n' "$src" >"$OUT/$name.failed-src"
    fi
    return 0
}
export -f run_one
export -f slug
export BRIEVC OUT

# Declared baseline for one triple (0 when the file does not mention it).
# Takes the value AFTER `=` only: stripping non-digits from the whole line would
# read `wasm32`'s own digits as part of the count (wasm32 ... = 27 -> 3227).
baseline_for() {
    local triple="$1" n
    n="$(grep -E "^\s*\"?$triple\"?\s*=" "$BASELINE" 2>/dev/null | head -1 \
         | sed 's/^[^=]*=//' | grep -oE '[0-9]+' | head -1 || true)"
    printf '%s' "${n:-0}"
}

fail_total=0
stale_any=0
for triple in "${TRIPLES[@]}"; do
    rm -f "$OUT"/*.result "$OUT"/*.failed-src
    printf '%s\n' "${CORPUS[@]}" \
        | xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {} "$triple" >/dev/null 2>&1 || true

    checked=0
    failed=0
    for f in "$OUT"/*.result; do
        [ -f "$f" ] || continue
        case "$(cat "$f")" in
            PASS) checked=$((checked + 1)) ;;
            FAIL) checked=$((checked + 1)); failed=$((failed + 1)) ;;
        esac
    done

    declared="$(baseline_for "$triple")"
    if [ "$failed" -gt "$declared" ]; then
        echo "cross-target-ir: $triple — $checked programs checked, $failed failed (declared baseline $declared)" >&2
        echo "  NEW OR REGRESSED failures:" >&2
        for f in "$OUT"/*.failed-src; do
            [ -f "$f" ] || continue
            src="$(cat "$f")"
            echo "    $src" >&2
            grep -m2 -E 'error:|why:|invalid' \
                "$OUT/$(slug "$src").$triple.log" 2>/dev/null | sed 's/^/        /' >&2 || true
        done
        fail_total=$((fail_total + failed - declared))
    elif [ "$failed" -lt "$declared" ]; then
        echo "cross-target-ir: $triple — $checked checked, $failed failed: BETTER than the declared baseline $declared."
        echo "  Lower the baseline (this is the ratchet, and the record must stay true):" >&2
        echo "    $BASELINE: \"$triple\" = $failed" >&2
        stale_any=1
    else
        echo "cross-target-ir: $triple — $checked programs checked, $failed failed (matches declared baseline $declared)"
    fi
done

if [ "$fail_total" -ne 0 ]; then
    echo "cross-target-ir: FAIL — emitted IR is not valid for every shipped triple; logs in $OUT" >&2
    exit 1
fi
if [ "$stale_any" -ne 0 ]; then
    echo "cross-target-ir: PASS, but a baseline is stale (see above); fix the record" >&2
    exit 1
fi
echo "cross-target-ir: PASS — emitted IR is valid for every shipped triple, at or below the declared baselines"
