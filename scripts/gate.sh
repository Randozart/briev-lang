#!/usr/bin/env bash
# 2026-11-09: test gate wrapper.
#
#   scripts/gate.sh quick      per-commit: library (minus conformance sweep),
#                              focused integrations, verifier + explicit Z3 tests
#   scripts/gate.sh verifier   required prover checks, including ignored Z3 tests
#   scripts/gate.sh full       every workspace test binary, current Cargo profile
#
# Uses cargo-nextest when installed (parallel test binaries); falls back to
# cargo test otherwise. Quick/verifier share selection and retain a manifest.
# 2026-11-15: gate.py owns accounting so missing tools or empty test runs cannot
# pass; retire its text adapter when Cargo supplies stable structured test results.
# The opt-out (AST) suite is a nightly/CI check, not a
# per-commit one: BRIEV_HIR_EMIT=0 is only validated on releases/flip changes.
set -euo pipefail
cd "$(dirname "$0")/.."

mode=${1:-quick}
case "$mode" in
  quick|verifier)
    exec python3 -B scripts/gate.py "$@"
    ;;
  full)
    if [ "$#" -gt 1 ]; then
      echo "full takes no extra arguments; use quick --help for recorded gate options" >&2
      exit 2
    fi
    # A conflict block once broke a fixture. Also reject Git failures: only
    # grep exit 1 establishes absence; command substitution isolates concurrent runs.
    if matches=$(git grep -nE '^(<<<<<<< |>>>>>>> |=======$)' -- ':!target'); then
      echo "conflict markers found in tracked files:" >&2
      echo "$matches" >&2
      exit 1
    else
      status=$?
      if [ "$status" != 1 ]; then exit "$status"; fi
    fi
    if command -v cargo-nextest >/dev/null 2>&1; then
      cargo nextest run --workspace -j 4
    else
      cargo test --workspace
    fi
    ;;
  *)
    echo "usage: scripts/gate.sh [quick|verifier] [--runner auto|nextest|cargo] [--artifacts DIR] [--timeout-seconds N] | full" >&2
    exit 2
    ;;
esac
