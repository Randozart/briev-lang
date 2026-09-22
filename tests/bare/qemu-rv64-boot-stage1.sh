#!/usr/bin/env bash
# Gate (bootstrap-bad plan, 2026-09-22): a two-stage bootloader in .bad.
# The `bootstrap bad` entry (stage 1) does the machine setup (PMP grant),
# prints "stage1 ", then CALLS a kernel routine (stage 2) that prints
# "kernel\n". Built as a flat .bin (--raw-bin) and loaded directly in
# QEMU -kernel. Proves the load-and-handoff bootstrapper pattern with no
# reactor and no compiler-owned _start.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/boot_stage1.out
BIN=/tmp/opencode/boot_stage1.bin

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }
command -v clang >/dev/null           || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null          || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/bad/boot_stage1.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode --raw-bin >/dev/null

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$BIN" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="stage1 kernel"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: two-stage bootloader handoff ($ACTUAL)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi