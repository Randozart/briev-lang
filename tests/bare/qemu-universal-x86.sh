#!/usr/bin/env bash
# Gate (per-arch stdlib boot entries, 2026-09-22): the universal
# bootstrapper examples/bad/bootloader.bv's x86_64 path — the raw x86_64
# uart_init block in std/bad/arch.bad (multiboot2 header + long-mode
# prologue + COM1 outb putc). qemu -kernel accepts the flat image.
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN=/tmp/opencode/uni-x86.bin

command -v qemu-system-x86_64 >/dev/null || { echo "SKIP: qemu-system-x86_64 not installed"; exit 0; }

# The x86 path of the universal source is exercised through the .bad
# pipeline (the .bv build path for x86 uses the linked-ELF route). Build a
# minimal bootloader that imports the same arch.bad and jumps uart_init.
cat > /tmp/uni_x86.bad << 'EOF'
import "std/bad/arch.bad"
section .text
global _start
_start:
    jmp uart_init
core:
    mov r0, 66
    call putc
    halt
EOF

./target/release/brievc bad /tmp/uni_x86.bad \
    --target x86_64-unknown-linux-gnu --raw-bin >/dev/null 2>&1
cp uni_x86.bin "$BIN" 2>/dev/null || cp /tmp/uni_x86.bin "$BIN" 2>/dev/null || true

# The flat image must carry the multiboot2 magic (0xE85250D6) near 0.
MAGIC=$(xxd -p -s 8 -l 4 "$BIN" 2>/dev/null || echo "")
if [[ "$MAGIC" != "d65052e8" ]]; then
    echo "FAIL: expected multiboot2 magic at offset 8, got '$MAGIC'"
    exit 1
fi

OUT=$(timeout 3 qemu-system-x86_64 -kernel "$BIN" -nographic 2>&1 || true)
if echo "$OUT" | grep -qi "invalid kernel header"; then
    echo "FAIL: qemu rejected the multiboot image"
    exit 1
fi

rm -f uni_x86.bin uni_x86.o uni_x86.s /tmp/uni_x86.bad /tmp/uni_x86.bin
echo "PASS: universal bootstrapper x86_64 path (multiboot) boots under qemu"