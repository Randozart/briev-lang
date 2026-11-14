#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# Check the pinned model and reject a false theorem; retain logs outside source.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: bash verification/lean/check.sh <external-artifact-directory>" >&2
    exit 2
fi

model_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd -- "$model_dir/../.." && pwd)
mkdir -p -- "$1"
artifact_dir=$(cd -- "$1" && pwd)
case "$artifact_dir/" in
    "$repo_dir/"*)
        echo "Choose an artifact directory outside the repository." >&2
        exit 2
        ;;
esac

lean_bin=${LEAN_BIN:-}
if [[ -z "$lean_bin" ]]; then
    lean_bin=$(command -v lean || true)
fi
if [[ -z "$lean_bin" && -x "$HOME/.elan/bin/lean" ]]; then
    lean_bin="$HOME/.elan/bin/lean"
fi
if [[ -z "$lean_bin" ]]; then
    echo "Lean unavailable: install the pinned toolchain or set LEAN_BIN." >&2
    exit 2
fi

cd -- "$model_dir"
"$lean_bin" --version > "$artifact_dir/version.log" 2>&1
if ! grep -Fq 'version 4.24.0,' "$artifact_dir/version.log"; then
    echo "Expected Lean 4.24.0; inspect version.log before checking this model." >&2
    exit 2
fi
"$lean_bin" -o "$artifact_dir/SolverResponse.olean" SolverResponse.lean \
    > "$artifact_dir/model.log" 2>&1
if grep -Eq 'sorryAx|declaration uses .sorry.|Lean.ofReduceBool' "$artifact_dir/model.log"; then
    echo "The model uses an unaccepted proof placeholder or native reduction axiom." >&2
    exit 1
fi

if LEAN_PATH="$artifact_dir${LEAN_PATH:+:$LEAN_PATH}" \
    "$lean_bin" negative/FalseSat.lean > "$artifact_dir/negative.log" 2>&1; then
    echo "The intentionally false SAT-to-UNSAT theorem was accepted." >&2
    exit 1
fi
if ! grep -Fq "tactic 'decide' proved that the proposition" "$artifact_dir/negative.log" \
    || ! grep -Fq 'is false' "$artifact_dir/negative.log"; then
    echo "The negative control failed for an unexpected reason; inspect negative.log." >&2
    exit 1
fi
echo "Lean model checked; false theorem rejected. Review axiom reports in model.log."
