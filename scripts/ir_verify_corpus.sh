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
# Usage: bash scripts/ir_verify_corpus.sh [dir ...]     (default: lib tests examples)
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BRIEVC="${BRIEVC:-/var/tmp/briev-xpu-target/debug/brievc}"
OUT="${IR_VERIFY_OUT:-/var/tmp/ir-verify-corpus}"
rm -rf "$OUT"; mkdir -p "$OUT"

dirs=("$@")
[ "${#dirs[@]}" -eq 0 ] && dirs=(lib tests examples)

ok=0 front_err=0 invalid=0
printf 'STATUS\tFILE\tDETAIL\n'
while IFS= read -r f; do
    key="$(printf '%s' "$f" | tr '/' '_' | sed 's/\.bv$//')"
    mkdir -p "$OUT/$key"
    if ! BRIEV_HOME="$ROOT" timeout 300 "$BRIEVC" build "$f" --llvm --no-verify-ir \
            --stdlib-path lib --config-dir config --out "$OUT/$key" >"$OUT/$key/nv.log" 2>&1; then
        front_err=$((front_err + 1))
        continue
    fi
    if BRIEV_HOME="$ROOT" timeout 300 "$BRIEVC" build "$f" --llvm \
            --stdlib-path lib --config-dir config --out "$OUT/$key" >"$OUT/$key/v.log" 2>&1; then
        ok=$((ok + 1))
        continue
    fi
    invalid=$((invalid + 1))
    detail="$(grep -m1 'LLVM rejected the module' "$OUT/$key/v.log" | cut -c1-110)"
    printf 'invalid_ir\t%s\t%s\n' "$f" "$detail"
done < <(printf '%s\n' "${dirs[@]}" | while read -r d; do find "$d" -name '*.bv' -type f 2>/dev/null; done | sort -u)

printf '\n== emitted-IR verification ==\n'
printf 'files_valid        %d\n' "$ok"
printf 'files_invalid_ir   %d   (compiler emitted a module LLVM rejects)\n' "$invalid"
printf 'files_frontend_error %d (not an IR defect; the program did not compile)\n' "$front_err"
printf 'logs: %s\n' "$OUT"
