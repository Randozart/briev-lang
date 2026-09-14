#!/usr/bin/env bash
# Phase 4 gate (rv64 capability kernel, plan 2026-09-14-bootstrap-kernel.md):
# two user-mode tasks print through the ecall syscall boundary; the CLINT
# timer preempts them and the scheduler round-robins by RESUME (each task's
# interrupted pc + full register set is preserved in its context area and
# restored on switch). Each task prints two chars with a wall-clock delay
# between them; the timer preempts mid-delay, so the chars land on separate
# slices. Golden: the FINITE interleave BA21 (restart would re-run from the
# task top every slice → continuous output; resume parks the tasks forever).
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/kernel_rv64.out
ELF=/tmp/opencode/kernel_rv64.elf

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }

./target/release/brievc build examples/kernel_rv64.b.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode >/dev/null
mv /tmp/opencode/kernel_rv64.b "$ELF"

timeout 5 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(cat "$OUT" | tr -cd '[:print:]')
EXPECT="BA21"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: resume-scheduled two-task kernel ($ACTUAL — finite)"
else
    echo "FAIL: expected '$EXPECT' (finite), got '$ACTUAL'"
    exit 1
fi