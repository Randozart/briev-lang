#!/usr/bin/env bash
# 2026-11-10: driver for docs/plans/2026-11-10-exhaustive-defect-campaign.md.
# Runs each defect-discovery surface to a fixed budget and logs raw evidence.
# Findings land in fuzz/artifacts/<target>/ and target/*-failure/. This script
# never edits sources; triage and fixes are separate, reviewed steps.
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

LOG_DIR="${CAMPAIGN_LOG_DIR:-target/defect-campaign}"
mkdir -p "$LOG_DIR"
HOST="$(rustc -vV | sed -n 's/^host: //p')"

FUZZ_SECONDS="${FUZZ_SECONDS:-300}"
SIM_CASES="${SIM_CASES:-64}"
MEM_FAMILIES="${MEM_FAMILIES:-0 2 3 4}"
MEM_CASES="${MEM_CASES:-16}"
MEM_SEED="${MEM_SEED:-7}"

stamp() { date -u +%Y-%m-%dT%H:%M:%SZ; }
section() { printf '\n===== %s :: %s =====\n' "$(stamp)" "$1" | tee -a "$LOG_DIR/campaign.log"; }

section "build fuzz targets (host=$HOST)"
cargo +nightly fuzz build 2>&1 | tee -a "$LOG_DIR/build.log"

run_fuzz() {
  local target="$1"
  section "fuzz $target (${FUZZ_SECONDS}s, fork=$(nproc))"
  mkdir -p "fuzz/corpus/$target"
  cargo +nightly fuzz run --target "$HOST" "$target" "fuzz/corpus/$target" -- \
    "-max_total_time=$FUZZ_SECONDS" "-fork=$(nproc)" -print_final_stats=1 \
    2>&1 | tee -a "$LOG_DIR/fuzz-$target.log"
  return "${PIPESTATUS[0]}"
}

run_fuzz frontend
run_fuzz arithmetic

section "native differential family 13 (${SIM_CASES} cases)"
BRIEV_SIM_FAMILY=13 BRIEV_SIM_CASES="$SIM_CASES" BRIEV_SIM_SEED=1 \
  cargo test --test deterministic_transitions scripted_integer_transitions -- --nocapture \
  2>&1 | tee -a "$LOG_DIR/sim-family13.log"

for family in $MEM_FAMILIES; do
  section "memory family $family (${MEM_CASES} cases, valgrind)"
  MEM_FAMILY="$family" MEM_CASES="$MEM_CASES" MEM_SEED="$MEM_SEED" MEM_VALGRIND=1 \
    cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture \
    2>&1 | tee -a "$LOG_DIR/mem-family$family.log"
done

section "artifacts"
find fuzz/artifacts -type f -not -name '.gitignore' -printf '%p\n' 2>/dev/null | tee -a "$LOG_DIR/campaign.log"

section "done"
