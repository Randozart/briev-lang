#!/usr/bin/env bash
# Phase 3 gate (rv64 capability kernel, plan 2026-09-11 §Phase 3):
# the kernel prints an incrementing tick count driven purely by CLINT
# timer traps. Timeout-guarded; golden-output diff. Integration-only —
# requires qemu-system-riscv64 (not part of `cargo test`).
#
# Usage: bash tests/bare/qemu-rv64-timer.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

GOLDEN_DIR=tests/bare/golden
OUT=/tmp/opencode/timer_rv64.out
ELF=/tmp/opencode/timer_rv64.elf

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }

./target/release/brievc build examples/timer_rv64.b.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode >/dev/null
mv /tmp/opencode/timer_rv64.b "$ELF"

# 1.3 s → ~13 ticks at 10 Hz; the pattern is cyclic 1..9,0.
timeout 2 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

# Expected: cyclic digit sequence starting at 1, at least 10 ticks.
ACTUAL=$(head -c 12 "$OUT" | tr -d '\n')
EXPECT="123456789012"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: timer-driven tick count (${ACTUAL}...)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
