#!/usr/bin/env bash
# toolchain_matrix.sh — S5-A of the stability-remediation plan
# (docs/plans/2026-10-16-stability-remediation.md).
#
# The cpp-vs-briev session flipped verdicts twice on LLVM version alone
# (17 vs 23: the X86-IS crash, the greedy-RA segfault). This script makes
# per-toolchain truth cheap: for every clang installed on this machine it
# compiles the corpus .ll → binary → runs it, and prints a verdict table.
# Run BEFORE RELEASES and on backend-touching commits; record surprising
# rows in the results file (environment-dependent verdicts are
# version-qualified from here on).
#
# Usage: bash benchmarks/toolchain_matrix.sh
set -euo pipefail
cd "$(dirname "$0")/.."

BV=./target/release/brievc
RT=lib/runtime/briev_rt.c
TMP=$(mktemp -d)

# Corpus: (name, source, env) — small, fast, spans the emission paths.
declare -a NAMES=(recommender multinode giantnode)
declare -A SRC=(
    [recommender]=benchmarks/recommender.bv
    [multinode]=benchmarks/recommender_corpus/recommender_multinode.bv
    [giantnode]=benchmarks/recommender_corpus/recommender_giantnode.bv
)
declare -A NEEDS_SIM=( [multinode]=1 [giantnode]=1 )

# Inputs the multinode/giantnode ports read from cwd (synthesized stand-ins
# — Otto's originals are not public; the Briev/C++ programs only need
# identical bytes, and u.data is the real MovieLens file when present).
if [ ! -f u.data ]; then
    echo "note: u.data missing — multinode/giantnode runs need it; skipping those rows" >&2
fi
if [ ! -f simlist.txt ]; then
    awk 'BEGIN{for(i=0;i<943;i++){s="";for(j=0;j<1682;j++){s=s (j?" ":"") ((i*31+j*17)%997);}print s}}' > simlist.txt
    SYNTHESIZED_SIM=1
    echo "note: synthesized a stand-in simlist.txt" >&2
fi
cleanup() {
    [ -n "${SYNTHESIZED_SIM:-}" ] && rm -f simlist.txt
    rm -rf "$TMP"
}
trap cleanup EXIT

# Discover clangs: version-suffixed first, then plain clang.
declare -a CLANGS=()
for c in clang-17 clang-18 clang-19 clang-20 clang; do
    command -v "$c" > /dev/null 2>&1 || continue
    v=$("$c" --version | head -1)
    # Skip duplicates (clang aliasing clang-23, same version string).
    dup=0
    for seen in "${CLANGS[@]:-}"; do
        [ -n "$seen" ] && [ "$("$seen" --version | head -1)" = "$v" ] && dup=1
    done
    [ "$dup" = 0 ] && CLANGS+=("$c")
done

if [ ${#CLANGS[@]} -eq 0 ]; then
    echo "no clang found" >&2
    exit 1
fi

printf "%-18s %-22s %s\n" "CLANG" "VERSION" "VERDICTS"
for c in "${CLANGS[@]}"; do
    ver=$("$c" --version | head -1 | sed 's/clang version \([0-9.]*\).*/\1/')
    cells=""
    for name in "${NAMES[@]}"; do
        src="${SRC[$name]}"
        out="$TMP/${name}.$ver.ll"
        if ! "$BV" build "$src" --llvm --out "$TMP/$name.$ver" --stdlib-path lib > /dev/null 2>&1; then
            cells="$cells $name=COMPILE-FAIL"
            continue
        fi
        ll="$TMP/$name.$ver/$(basename "${src%.bv}").ll"
        if ! "$c" -O3 -march=native -ffast-math "$ll" "$RT" -ldl -o "$TMP/$name.$ver.bin" > /dev/null 2>&1; then
            cells="$cells $name=CLANG-FAIL"
            continue
        fi
        run_env=()
        if [ -n "${NEEDS_SIM[$name]:-}" ] && [ ! -f u.data ]; then
            cells="$cells $name=SKIP(no-input)"
            continue
        fi
        if timeout 30 "$TMP/$name.$ver.bin" > "$TMP/$name.$ver.out" 2>/dev/null; then
            # Verdict = ran + produced output; byte-parity across toolchains
            # is checked by the caller diffing the .out files (same name).
            cells="$cells $name=OK"
        else
            cells="$cells $name=RUN-FAIL"
        fi
    done
    printf "%-18s %-22s %s\n" "$c" "$ver" "$cells"
done

# Cross-toolchain output parity where a program ran under 2+ clangs.
echo ""
for name in "${NAMES[@]}"; do
    first=""
    mismatch=0
    for f in "$TMP/$name."*.out; do
        [ -f "$f" ] || continue
        if [ -z "$first" ]; then
            first="$f"
            continue
        fi
        cmp -s "$first" "$f" || { mismatch=1; break; }
    done
    count=$(ls "$TMP/$name."*.out 2>/dev/null | wc -l)
    if [ "$count" -ge 2 ]; then
        if [ "$mismatch" = 0 ]; then
            echo "parity: $name identical across $count toolchains"
        else
            echo "PARITY: $name OUTPUT DIFFERS ACROSS TOOLCHAINS — investigate"
            exit 1
        fi
    fi
done
