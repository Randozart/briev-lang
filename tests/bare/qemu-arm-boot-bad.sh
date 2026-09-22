#!/usr/bin/env bash
# Gate (bootstrap-bad plan, 2026-09-22): a `bootstrap bad` on Cortex-M3
# (QEMU MPS2-AN385) prints "Briev boot" with NO startup.S and NO
# compiler-owned _start — the .bad body IS the vector table + reset
# handler, assembled through the bad backend and linked by ld.lld.
#
# Build flow: brievc parses `bootstrap bad` → bad backend emits thumb-2 →
# clang's integrated assembler (no arm-none-eabi binutils needed) → ld.lld
# with the board linker script.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/boot_bad.out
ELF=/tmp/opencode/boot_mps2.b

command -v qemu-system-arm >/dev/null || { echo "SKIP: qemu-system-arm not installed"; exit 0; }
command -v clang >/dev/null          || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null         || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/bad/boot_mps2.b.bv \
    --triple thumbv7m-none-eabi \
    --linker-script lib/targets/qemu-mps2-an385.ld \
    --out /tmp/opencode >/dev/null

timeout 3 qemu-system-arm -machine mps2-an385 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: bootstrap bad on Cortex-M3 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi