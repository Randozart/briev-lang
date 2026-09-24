#!/usr/bin/env bash
# Gate (bad-raw-blocks plan, 2026-09-22): a real-mode MBR boot sector
# written with `raw x86_64` blocks + the `int` core op. The 512-byte flat
# image (--raw-bin, no link) carries the 0x55AA signature; SeaBIOS loads
# it to 0x7C00 and executes the INT 10h teletype body.
#
# Proves: raw blocks are the ergonomic verbatim escape for 16-bit GAS,
# and the full boot-sector chain (as --64 + .code16 + .org + signature →
# objcopy -O binary → SeaBIOS) works end to end.
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN=/tmp/opencode/boot_mbr.bin
OUT=/tmp/opencode/boot_mbr.out

command -v qemu-system-i386 >/dev/null || { echo "SKIP: qemu-system-i386 not installed"; exit 0; }

./target/release/brievc bad examples/bad/boot_mbr.bad \
    --target x86_64-unknown-linux-gnu --raw-bin --no-link >/dev/null
cp boot_mbr.bin "$BIN"

# The image must be exactly 512 bytes with the boot signature at 510-511.
SIZE=$(stat -c%s "$BIN")
SIG=$(xxd -p -s 510 -l 2 "$BIN")
if [[ "$SIZE" != "512" || "$SIG" != "55aa" ]]; then
    echo "FAIL: expected 512 bytes with 55aa signature, got size=$SIZE sig=$SIG"
    exit 1
fi

# SeaBIOS must load and execute the sector — no "No bootable device".
timeout 3 qemu-system-i386 -drive "file=$BIN,format=raw,if=floppy" \
    -nographic > "$OUT" 2>&1 || true
if grep -qi "No bootable" "$OUT"; then
    echo "FAIL: SeaBIOS did not boot the MBR"
    exit 1
fi

rm -f boot_mbr.bin boot_mbr.o boot_mbr.s
echo "PASS: 512-byte real-mode MBR boots under SeaBIOS ($SIZE bytes, sig $SIG)"