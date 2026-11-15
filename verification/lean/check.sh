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
"$lean_bin" -o "$artifact_dir/IntegerVC.olean" IntegerVC.lean \
    >> "$artifact_dir/model.log" 2>&1
if grep -Eq 'sorryAx|declaration uses .sorry.|Lean.ofReduceBool' "$artifact_dir/model.log"; then
    echo "The model uses an unaccepted proof placeholder or native reduction axiom." >&2
    exit 1
fi
python3 - "$artifact_dir/model.log" "$artifact_dir/axioms.json" <<'PY'
import json
from pathlib import Path
import re
import sys

expected = {
    "unsat_iff_exact", "sat_iff_exact", "first_nonempty_decides",
    "leading_sat_dominates", "leading_unsat_dominates",
    "malformed_head_cannot_prove", "substitution_preserves_evaluation",
    "assertion_substitution", "assignment_vc_sound", "sequence_wp_correct",
    "sequence_vc_sound", "copy_survives_source_overwrite",
}
pattern = r"'(?:BrievAssurance|BrievIntegerVC)\.(\w+)' (?:depends on axioms: \[([^]]*)\]|does not depend on any axioms)"
reports = {}
for line in Path(sys.argv[1]).read_text().splitlines():
    match = re.fullmatch(pattern, line)
    if match is None:
        continue
    name, dependencies = match.groups()
    axioms = set(dependencies.split(", ")) if dependencies else set()
    if name in reports or axioms - {"propext"}:
        sys.exit("Duplicate axiom report or unapproved theorem dependency.")
    reports[name] = sorted(axioms)
if set(reports) != expected:
    sys.exit("Expected an axiom report for every named model theorem.")
Path(sys.argv[2]).write_text(json.dumps(reports, indent=2) + "\n")
PY

for control in FalseSat UnboundedAddition StaleAssignment SavedAlias; do
    if LEAN_PATH="$artifact_dir${LEAN_PATH:+:$LEAN_PATH}" \
        "$lean_bin" "negative/$control.lean" > "$artifact_dir/$control.log" 2>&1; then
        echo "The intentionally false theorem $control was accepted." >&2
        exit 1
    else
        negative_status=$?
    fi
    if [[ "$negative_status" -ne 1 ]] \
        || ! grep -Fq "Tactic \`decide\` proved that the proposition" "$artifact_dir/$control.log" \
        || ! grep -Fq 'is false' "$artifact_dir/$control.log"; then
        echo "Unexpected negative-control failure; inspect $control.log." >&2
        exit 1
    fi
done
echo "Lean models checked (12 named theorems); axioms approved; 4 false theorems rejected."
