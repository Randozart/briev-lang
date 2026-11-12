#!/usr/bin/env bash
# 2026-11-09: test gate wrapper.
#
#   scripts/gate.sh quick   per-commit: library (minus the full conformance
#                           sweep) + the focused integration suites
#   scripts/gate.sh full    pre-push: every test binary, both profiles
#
# Uses cargo-nextest when installed (parallel test binaries); falls back to
# cargo test otherwise. The opt-out (AST) suite is a nightly/CI check, not a
# per-commit one: BRIEV_HIR_EMIT=0 is only validated on releases/flip changes.
set -euo pipefail
cd "$(dirname "$0")/.."

# Conflict-marker guard: a committed conflict block is a latent parse/merge
# bug (cost us a broken fixture and a red suite). Cheap, always first.
if git grep -nE '^(<<<<<<< |>>>>>>> |=======$)' -- ':!target' >/tmp/opencode/conflict_markers 2>/dev/null; then
  echo "conflict markers found in tracked files:" >&2
  cat /tmp/opencode/conflict_markers >&2
  exit 1
fi

mode=${1:-quick}
have_nextest=0
command -v cargo-nextest >/dev/null 2>&1 && have_nextest=1

case "$mode" in
  quick)
    if [ "$have_nextest" = 1 ]; then
      cargo nextest run --lib -j 4 -E 'not test(conformance_sweep)'
      cargo nextest run -j 4 -E \
        'binary(diagnostics) | binary(hir_snapshots) | binary(hold_law) | binary(publish_reclamation) | binary(deterministic_transitions) | binary(unit_test_blocks) | binary(interpreter_corpus_parity) | binary(hir_reclamation_parity) | binary(reduction_scripts) | binary(verify_report)'
    else
      cargo test --lib -- --skip conformance_sweep
      cargo test --test diagnostics --test hir_snapshots --test hold_law \
        --test publish_reclamation --test deterministic_transitions
    fi
    ;;
  full)
    if [ "$have_nextest" = 1 ]; then
      cargo nextest run --all -j 4
    else
      cargo test --all
    fi
    ;;
  *)
    echo "usage: scripts/gate.sh [quick|full]" >&2
    exit 2
    ;;
esac
