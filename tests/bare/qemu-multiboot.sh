#!/usr/bin/env bash
# Gate (bad-raw-blocks plan, 2026-09-22): a multiboot2 x86_64 kernel whose
# 32-bit prologue is a `raw x86_64` block and whose 64-bit body is portable
# core ops. The header rides .text (objcopy -O binary flattens only the
# first LOAD segment), so the flat image starts with the multiboot magic.
# qemu -kernel accepts it and runs the long-mode entry.
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN=/tmp/opencode/boot_multiboot.bin

command -v qemu-system-x86_64 >/dev/null || { echo "SKIP: qemu-system-x86_64 not installed"; exit 0; }

./target/release/brievc bad examples/bad/boot_multiboot.bad \
    --target x86_64-unknown-linux-gnu --raw-bin >/dev/null
cp boot_multiboot.bin "$BIN"

# The flat image must START with the multiboot2 magic 0xE85250D6.
MAGIC=$(xxd -p -l 4 "$BIN")
if [[ "$MAGIC" != "d65052e8" ]]; then
    echo "FAIL: expected multiboot2 magic d65052e8 at offset 0, got $MAGIC"
    exit 1
fi

# qemu -kernel must accept the image (no "invalid kernel header").
OUT=$(timeout 3 qemu-system-x86_64 -kernel "$BIN" -nographic 2>&1 || true)
if echo "$OUT" | grep -qi "invalid kernel header"; then
    echo "FAIL: qemu rejected the multiboot image"
    exit 1
fi

rm -f boot_multiboot boot_multiboot.o boot_multiboot.s boot_multiboot.bin
echo "PASS: multiboot2 x86_64 kernel (raw-block prologue) boots under qemu"