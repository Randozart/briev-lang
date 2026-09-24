#!/usr/bin/env bash
# Gate (universal completion, 2026-09-22): the universal bootstrapper
# examples/bad/bootloader.bv boots aarch64 (QEMU virt) — ONE .bv source,
# per-arch prologues in std/bad/arch.bad as named raw blocks, imported at
# the .bv top level. PL011 UART at 0x09000000, boots at the ELF entry.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/uni-aarch64.out
ELF=/tmp/opencode/uni-aarch64/bootloader

command -v qemu-system-aarch64 >/dev/null || { echo "SKIP: qemu-system-aarch64 not installed"; exit 0; }

./target/release/brievc build examples/bad/bootloader.bv \
    --triple aarch64-unknown-none \
    --linker-script lib/targets/qemu-virt-aarch64.ld \
    --out /tmp/opencode/uni-aarch64 >/dev/null 2>&1

timeout 3 qemu-system-aarch64 -machine virt -cpu cortex-a57 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="universal boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: universal bootstrapper on aarch64 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi