#!/usr/bin/env bash
# Gate (bootstrap-bad plan, 2026-09-22): a `bootstrap bad` on riscv64
# (QEMU virt) prints "Briev rv64 boot" with no compiler-owned _start and
# no `.b` suffix. The body is the authored entry: PMP grant via CSR ops
# (csrw pmpaddr0/pmpcfg0), UART banner via MMIO. Proves the second
# architecture for the bootstrap-bad mechanism + the CSR op family.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/boot_bad_rv64.out
ELF=/tmp/opencode/boot_rv64

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }
command -v clang >/dev/null           || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null          || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/bad/boot_rv64.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode >/dev/null

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev rv64 boot"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: bootstrap bad on riscv64 prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi