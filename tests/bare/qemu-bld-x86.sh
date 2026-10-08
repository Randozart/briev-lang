#!/usr/bin/env bash
# Gate (BILLD plan M7, 2026-10-08): a multiboot2 x86_64 kernel whose
# 32-bit prologue rides the recipe's FIRST-statement `bad { }` block and
# whose 64-bit body is BILLD statements (value loop, MMIO stores, engine
# verbs). Proves the .bld artifact pipeline end to end (lower → allocate
# → .bad backend → objcopy) and that qemu -kernel accepts the image.
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN=/tmp/opencode/boot_bld_x86.bin

command -v qemu-system-x86_64 >/dev/null || { echo "SKIP: qemu-system-x86_64 not installed"; exit 0; }

./target/release/brievc bld examples/bld/boot_protected_x86.bld \
    --target x86_64-unknown-linux-gnu --raw-bin >/dev/null
cp boot_protected_x86.bin "$BIN"

# The flat image must START with the multiboot2 magic 0xE85250D6 (the
# header rode the first-statement ownership block).
MAGIC=$(xxd -p -l 4 "$BIN")
if [[ "$MAGIC" != "d65052e8" ]]; then
    echo "FAIL: expected multiboot2 magic d65052e8 at offset 0, got $MAGIC"
    exit 1
fi

# qemu -kernel must accept the image (no "invalid kernel header").
OUT=$(timeout 3 qemu-system-x86_64 -kernel "$BIN" -nographic 2>&1 || true)
if echo "$OUT" | grep -qi "invalid kernel header"; then
    echo "FAIL: qemu rejected the .bld multiboot image"
    exit 1
fi

rm -f boot_protected_x86 boot_protected_x86.bin boot_protected_x86.o boot_protected_x86.s
echo "PASS: .bld multiboot kernel builds and boots under qemu x86_64"
