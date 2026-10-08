#!/usr/bin/env bash
# Gate (BILLD plan M7, 2026-10-08): a .bld bootstrap on aarch64 (QEMU
# virt) prints "Briev" — every statement is tier-native (Ptr values,
# MMIO stores with materialized immediates, the Halt verb). Proves the
# allocator's spill-free path and the store row fix (aarch64 str needs a
# register operand) on real hardware behavior.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/boot_bld_aarch64.out
ELF=/tmp/opencode/boot_bld_aarch64

command -v qemu-system-aarch64 >/dev/null || { echo "SKIP: qemu-system-aarch64 not installed"; exit 0; }
command -v clang >/dev/null           || { echo "SKIP: clang not installed"; exit 0; }

./target/release/brievc bld examples/bld/boot_aarch64.bld \
    --target aarch64-unknown-none --emit-asm >/dev/null

clang --target=aarch64-unknown-none -march=armv8-a -c \
    examples/bld/boot_aarch64.s -o /tmp/opencode/boot_bld_aarch64.o
ld.lld -T lib/targets/qemu-virt-aarch64.ld -e Reset_Handler \
    /tmp/opencode/boot_bld_aarch64.o -o "$ELF"

timeout 3 qemu-system-aarch64 -machine virt -cpu cortex-a57 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: .bld bootstrap on aarch64 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
rm -f examples/bld/boot_aarch64.s
