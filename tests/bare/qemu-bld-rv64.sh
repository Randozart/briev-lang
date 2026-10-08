#!/usr/bin/env bash
# Gate (BILLD plan M7, 2026-10-08): a .bld bootstrap on riscv64 (QEMU
# virt) prints "Briev rv64 boot". PMP setup rides the WriteControlReg
# engine verb, the banner is a `bad { }` MMIO loop, Halt parks the hart.
# Proves the engine-verb registry on real hardware behavior (CSR numbers
# splice as constants) and the whole .bld → .s → ELF → qemu chain.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/boot_bld_rv64.out
ELF=/tmp/opencode/boot_bld_rv64

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }
command -v clang >/dev/null           || { echo "SKIP: clang not installed"; exit 0; }

./target/release/brievc bld examples/bld/boot_rv64.bld \
    --target riscv64-unknown-none --emit-asm >/dev/null

clang --target=riscv64-unknown-none -march=rv64imac_zicsr -c \
    examples/bld/boot_rv64.s -o /tmp/opencode/boot_bld_rv64.o
ld.lld -T lib/targets/qemu-virt-rv64.ld -e Reset_Handler \
    /tmp/opencode/boot_bld_rv64.o -o "$ELF"

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev rv64 boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: .bld bootstrap on riscv64 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
rm -f examples/bld/boot_rv64.s
