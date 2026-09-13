#!/usr/bin/env bash
# Phase 4 gate (rv64 capability kernel, plan 2026-09-14-bootstrap-kernel.md):
# two user-mode tasks print through the ecall syscall boundary; the CLINT
# timer preempts and the scheduler round-robins. Golden: alternating BABA (round-robin starts at index 1).
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

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(head -c 12 "$OUT" | tr -d '\n')
EXPECT="BABABABABABA"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: preemptive two-task kernel ($ACTUAL...)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
