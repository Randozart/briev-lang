#!/usr/bin/env bash
# Keyword-vs-default adversarial gate (F1, plan coalesced-kv-memory-path;
# doctrine: analysis-transparency in briev-capability-frontier.md).
#
# Rule 2 made falsifiable: run a program built WITH a strategy keyword and
# the IDENTICAL program without; verify the outputs agree (the keyword must
# not change semantics on this program); compare best-of-N wall times.
#
# Verdict:
#   outputs differ               -> FAIL (invalid pair — a keyword that
#                                   changes semantics is a pair bug)
#   keyword faster beyond noise  -> FINDING (exit 1): a genuine find. A
#                                   finding record is written with a
#                                   classification that MUST be filled in:
#                                   instance-specific (causal reason) or
#                                   general (compiler gap). The gate stays
#                                   red until the classification lands.
#   default faster or parity     -> PASS (the default carries the win)
#
# Usage:
#   bash benchmarks/keyword_ab_gate.sh --selftest
#   bash benchmarks/keyword_ab_gate.sh <default-src> <keyword-src> [name] [runs]
set -euo pipefail
cd "$(dirname "$0")/.."

BRIEVC=./target/release/brievc
NOISE=1.05   # keyword must be >5% faster to count as a win (noise floor)
RUNS_N=7

# ── verdict core (pure; exercised by --selftest) ────────────────────────────
verdict() {  # args: default_ms keyword_ms outputs_equal(0/1)
    awk -v d="$1" -v k="$2" -v e="$3" -v n="$NOISE" 'BEGIN {
        if (e != 0)          { print "FAIL:outputs-differ"; exit }
        if (d <= 0 || k <= 0){ print "FAIL:bad-timing";      exit }
        if (k * n < d)       { print "FINDING:keyword-wins"; exit }
        print "PASS:default-carries"
    }'
}

if [ "${1:-}" = "--selftest" ]; then
    [ "$(verdict 100 80 0)" = "FINDING:keyword-wins" ] || { echo "selftest: win undetected"; exit 1; }
    [ "$(verdict 100 100 0)" = "PASS:default-carries" ] || { echo "selftest: parity failed"; exit 1; }
    [ "$(verdict 80 100 0)" = "PASS:default-carries" ] || { echo "selftest: default-win failed"; exit 1; }
    [ "$(verdict 100 80 1)" = "FAIL:outputs-differ" ] || { echo "selftest: mismatch undetected"; exit 1; }
    [ "$(verdict 0 80 0)" = "FAIL:bad-timing" ] || { echo "selftest: bad-timing undetected"; exit 1; }
    echo "selftest: verdict core OK"
    exit 0
fi

DEF_SRC="${1:?usage: keyword_ab_gate.sh <default-src> <keyword-src> [name] [runs]}"
KW_SRC="${2:?usage: keyword_ab_gate.sh <default-src> <keyword-src> [name] [runs]}"
NAME="${3:-pair}"
RUNS_N="${4:-$RUNS_N}"
OUT=$(mktemp -d /tmp/opencode/kwab.XXXX)
echo "gate: $NAME (runs=$RUNS_N, noise=${NOISE}) artifacts: $OUT"

# build_and_run <src> <tag> <stdout-file> <time-file>
build_and_run() {
    local src=$1 tag=$2 stdout_f=$3 time_f=$4
    local base; base=$(basename "${src%.*}")
    mkdir -p "$OUT/$tag"
    # stdlib resolves relative to the source, so the copy builds in-tree
    # (the m3/m4 harness pattern).
    local tmp_src="examples/gpu/kwab_${tag}_tmp.abv"
    cp "$src" "$tmp_src"
    "$BRIEVC" build "$tmp_src" --out "$OUT/$tag" >/dev/null
    rm -f "$tmp_src"
    local runner
    runner=$(ls "$OUT/$tag"/*_runner.c | head -1)
    # 2026-09-21 (Family K): orchestration + driver archives (Rust-built).
    cc -O2 -I"$OUT/$tag" -L"$OUT/$tag" -o "$OUT/$tag/bin" "$runner" \
        -lbriev_accel_rt -lbriev_gpu_rt \
        -lvulkan -lOpenCL -lcuda -lpthread -lm
    # first run: capture stdout for the equality check
    BRIEV_ACCEL_DEVICE=cuda "$OUT/$tag/bin" > "$stdout_f"
    # best-of-N wall time (ns), process total — identical overhead both sides
    local best=0 t0 t1
    for _ in $(seq 1 "$RUNS_N"); do
        t0=$(date +%s%N)
        BRIEV_ACCEL_DEVICE=cuda "$OUT/$tag/bin" > /dev/null
        t1=$(date +%s%N)
        local dt=$((t1 - t0))
        if [ "$best" = 0 ] || [ "$dt" -lt "$best" ]; then best=$dt; fi
    done
    echo $((best / 1000)) > "$time_f"   # microseconds
}

build_and_run "$DEF_SRC" def "$OUT/def.out" "$OUT/def_time.txt"
build_and_run "$KW_SRC" kw "$OUT/kw.out" "$OUT/kw_time.txt"

DEF_MS=$(cat "$OUT/def_time.txt")
KW_MS=$(cat "$OUT/kw_time.txt")
if diff -q "$OUT/def.out" "$OUT/kw.out" > /dev/null; then EQUAL=0; else EQUAL=1; fi

echo "  default: ${DEF_MS} us | keyword: ${KW_MS} us | outputs_equal=$EQUAL"
V=$(verdict "$DEF_MS" "$KW_MS" "$EQUAL")
echo "  verdict: $V"

case "$V" in
    PASS:*)
        echo "  the default carries the win — nothing to do"
        ;;
    FAIL:*)
        echo "  the PAIR is invalid (keyword changed semantics, or a build broke)"
        exit 1
        ;;
    FINDING:*)
        local_dir=benchmarks/results/keyword-findings
        mkdir -p "$local_dir"
        rec="$local_dir/$(date +%Y-%m-%d)-${NAME}.md"
        if [ ! -f "$rec" ]; then
            cat > "$rec" <<EOF
# Keyword finding: ${NAME}

**Date:** $(date +%Y-%m-%d) | **Gate:** keyword_ab_gate.sh
**Measured:** default ${DEF_MS} us vs keyword ${KW_MS} us (best of ${RUNS_N})

## Classification (REQUIRED — the gate stays red until this is filled)

- [ ] **Instance-specific** — the keyword wins on this shape only.
      Causal reason (which analysis/cost model lacks this shape, and the
      roadmap to close it):
- [ ] **General** — the default is missing an optimization class.
      Compiler gap:

## Artifacts
$OUT
EOF
            echo "  finding record written: $rec"
            echo "  CLASSIFY the finding (instance-specific vs general + why); the gate stays red until then"
        else
            echo "  finding record exists: $rec (classification pending?)"
        fi
        exit 1
        ;;
esac
