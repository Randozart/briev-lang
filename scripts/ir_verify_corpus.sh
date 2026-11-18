#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Emitted-IR verification sweep (2026-11-18).
#
# For every `.bv` in the given directories, compile it TWICE:
#   1. `--no-verify-ir` — frontend + codegen only. A failure here is a program or
#      frontend diagnostic (`files_frontend_error`), not an IR defect.
#   2. default (verifier on) — the module must be well-formed LLVM.
# A file that passes (1) and fails (2) is `files_invalid_ir`: the compiler emitted
# a module LLVM rejects. That is the column `BUGS.md` groups A–E describe, and
# before this it was a number nobody could act on.
#
# Parallel by default: one worker per CPU (`IR_VERIFY_JOBS` overrides). Running it
# serially over `lib/` needs 2 compilations × ~200 files and blew a 40-minute
# window, which is why the first version was only ever pointed at small trees.
#
# Allowance ratchet: if `tests/ir_verify_allow.txt` exists, the sweep EXITS 1 when
# the set of invalid files differs from it — a new invalid module fails, and so
# does a stale exemption. That is what makes this a gate rather than a report.
#
# Usage: bash scripts/ir_verify_corpus.sh [dir ...]     (default: lib tests examples)
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BRIEVC="${BRIEVC:-/var/tmp/briev-xpu-target/debug/brievc}"
OUT="${IR_VERIFY_OUT:-/var/tmp/ir-verify-corpus}"
JOBS="${IR_VERIFY_JOBS:-$(nproc 2>/dev/null || echo 4)}"
ALLOW="tests/ir_verify_allow.txt"
rm -rf "$OUT"; mkdir -p "$OUT/results"

dirs=("$@")
full_tree=1
if [ "${#dirs[@]}" -eq 0 ]; then dirs=(lib tests examples); else full_tree=0; fi

# One file → one result line on stdout (workers interleave safely: single write).
verify_one() {
    local f="$1"
    local key
    key="$(printf '%s' "$f" | tr '/' '_' | sed 's/\.bv$//')"
    mkdir -p "$OUT/$key"
    if ! BRIEV_HOME="$ROOT" timeout 300 "$BRIEVC" build "$f" --llvm --no-verify-ir \
            --stdlib-path lib --config-dir config --out "$OUT/$key" >"$OUT/$key/nv.log" 2>&1; then
        printf 'frontend_error\t%s\t\n' "$f"
        return
    fi
    if BRIEV_HOME="$ROOT" timeout 300 "$BRIEVC" build "$f" --llvm \
            --stdlib-path lib --config-dir config --out "$OUT/$key" >"$OUT/$key/v.log" 2>&1; then
        printf 'valid\t%s\t\n' "$f"
        return
    fi
    local detail
    detail="$(grep -m1 'LLVM rejected the module' "$OUT/$key/v.log" | cut -c1-110)"
    printf 'invalid_ir\t%s\t%s\n' "$f" "$detail"
}
export -f verify_one
export ROOT BRIEVC OUT

printf '%s\n' "${dirs[@]}" | while read -r d; do find "$d" -name '*.bv' -type f 2>/dev/null; done \
    | sort -u \
    | xargs -r -P "$JOBS" -I{} bash -c 'verify_one "$@"' _ {} >"$OUT/results.tsv"

ok=$(grep -c '^valid' "$OUT/results.tsv" || true)
front_err=$(grep -c '^frontend_error' "$OUT/results.tsv" || true)
invalid=$(grep -c '^invalid_ir' "$OUT/results.tsv" || true)

printf 'STATUS\tFILE\tDETAIL\n'
grep '^invalid_ir' "$OUT/results.tsv" || true

printf '\n== emitted-IR verification ==\n'
printf 'files_valid          %d\n' "$ok"
printf 'files_invalid_ir     %d   (compiler emitted a module LLVM rejects)\n' "$invalid"
printf 'files_frontend_error %d   (not an IR defect; the program did not compile)\n' "$front_err"
printf 'jobs                 %d\n' "$JOBS"
printf 'logs: %s\n' "$OUT"

# The ratchet only makes sense over the tree the allowance was baselined on; a
# partial sweep (`... tests`) would report every out-of-scope entry as stale.
if [ -f "$ALLOW" ] && [ "$full_tree" -eq 1 ]; then
    grep '^invalid_ir' "$OUT/results.tsv" | cut -f2 | sort >"$OUT/found.txt"
    grep -v '^#' "$ALLOW" | grep -v '^[[:space:]]*$' | sed 's/[[:space:]]*#.*//' | sort >"$OUT/allowed.txt"
    if ! diff -q "$OUT/found.txt" "$OUT/allowed.txt" >/dev/null; then
        printf '\n== allowance ratchet FAILED ==\n'
        printf 'newly invalid (fix or add with a reason):\n'
        comm -23 "$OUT/found.txt" "$OUT/allowed.txt" | sed 's/^/  + /'
        printf 'no longer invalid (remove from %s):\n' "$ALLOW"
        comm -13 "$OUT/found.txt" "$OUT/allowed.txt" | sed 's/^/  - /'
        exit 1
    fi
    printf 'allowance ratchet: OK (%s entries, all still invalid)\n' "$(wc -l <"$OUT/allowed.txt")"
fi
exit 0
