#!/usr/bin/env bash
#
# Clean-box smoke test for the install story (2026-10-06, folio).
#
# Builds the release compiler, installs it to a throwaway prefix (binary +
# resources), scaffolds a project with `brievc init`, and runs it FROM OUTSIDE
# the source tree — proving the installed compiler finds its stdlib through
# resource_root() (<prefix>/share/briev/lib). Times the run.
#
# Usage: bash scripts/folio-smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
PREFIX="${TMP}/prefix/bin"
WORK="${TMP}/app"

echo "== build release =="
( cd "${ROOT}" && cargo build --release )

echo "== install -> ${PREFIX} =="
"${ROOT}/scripts/briev-install" --prefix "${PREFIX}"

BIN="${PREFIX}/brievc"
echo "== init =="
mkdir -p "${WORK}"
( cd "${TMP}" && "${BIN}" init app )

echo "== run (from ${WORK}, outside the source tree) =="
start=$(date +%s.%N)
out="$(cd "${WORK}" && "${BIN}" run src/main.bv 2>&1)"
end=$(date +%s.%N)

if echo "${out}" | grep -q "Hello, Briev!"; then
    printf 'PASS: greeting produced in %ss\n' "$(echo "${end} - ${start}" | bc)"
else
    echo "FAIL: no greeting. Output:"
    echo "${out}"
    exit 1
fi
