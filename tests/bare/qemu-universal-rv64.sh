#!/usr/bin/env bash
# Gate (per-arch stdlib boot entries, 2026-09-22): the universal
# bootstrapper examples/bad/bootloader.bv — ONE .bv source, per-arch
# prologues in std/bad/arch.bad as named raw blocks, imported at the .bv
# top level. Boots riscv64 (QEMU virt) and prints "universal boot".
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/uni-rv64.out
ELF=/tmp/opencode/uni-rv64/bootloader

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }

./target/release/brievc build examples/bad/bootloader.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode/uni-rv64 >/dev/null 2>&1

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="universal boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: universal bootstrapper on riscv64 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi