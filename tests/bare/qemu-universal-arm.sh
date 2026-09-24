#!/usr/bin/env bash
# Gate (per-arch stdlib boot entries, 2026-09-22): the universal
# bootstrapper examples/bad/bootloader.bv — ONE .bv source, per-arch
# prologues in std/bad/arch.bad as named raw blocks, imported at the .bv
# top level. Boots thumbv7m (MPS2-AN385) and prints "universal boot".
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/uni-arm.out
ELF=/tmp/opencode/uni-arm/bootloader

command -v qemu-system-arm >/dev/null || { echo "SKIP: qemu-system-arm not installed"; exit 0; }

./target/release/brievc build examples/bad/bootloader.bv \
    --triple thumbv7m-none-eabi \
    --linker-script lib/targets/qemu-mps2-an385.ld \
    --out /tmp/opencode/uni-arm >/dev/null 2>&1

timeout 3 qemu-system-arm -machine mps2-an385 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="universal boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: universal bootstrapper on Cortex-M3 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi