#!/usr/bin/env bash
# Gate (bad assembly+ plan, 2026-10-20): a pure `.bad` x86_64 multiboot
# image. QEMU's multiboot loader takes only multiboot1 and rejects ELF64,
# so the image uses the multiboot1 a.out kludge (flat `--raw-bin`, header
# carries load/entry addresses). The 32-bit prologue prints "Briev" over
# COM1, switches to long mode through identity page tables, and a symbolic
# 64-bit body prints "R64" and parks. No .bld, no ELF32.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT=/tmp/opencode/boot_plus_x86.out
WORK=/tmp/opencode/boot_plus_x86

command -v qemu-system-x86_64 >/dev/null || { echo "SKIP: qemu-system-x86_64 not installed"; exit 0; }
command -v objcopy >/dev/null          || { echo "SKIP: objcopy not installed"; exit 0; }
[[ -x "$ROOT/target/release/brievc" ]] || { echo "SKIP: release brievc not built"; exit 0; }

mkdir -p "$WORK"
cp "$ROOT/examples/bad/boot_plus_x86.bad" "$WORK/boot_plus_x86.bad"
cd "$WORK"
"$ROOT/target/release/brievc" bad "$WORK/boot_plus_x86.bad" \
    --target x86_64-unknown-none \
    --linker-script "$ROOT/lib/targets/multiboot-x86_64.ld" \
    --raw-bin >/dev/null
BIN="$WORK/boot_plus_x86.bin"

timeout 3 qemu-system-x86_64 -machine pc -m 256 -kernel "$BIN" \
    -display none -serial "file:$OUT" -no-reboot >/dev/null 2>&1 || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
# "Briev" = the 32-bit multiboot entry; "R64" = the symbolic 64-bit body.
if [[ "$ACTUAL" == *"Briev"* && "$ACTUAL" == *"R64"* ]]; then
    echo "PASS: pure .bad multiboot x86_64 prints 'Briev' then 'R64'"
else
    echo "FAIL: expected 'Briev' and 'R64', got '$ACTUAL'"
    exit 1
fi
rm -f "$BIN" "$WORK/boot_plus_x86" "$WORK/boot_plus_x86.s" "$WORK/boot_plus_x86.o"
