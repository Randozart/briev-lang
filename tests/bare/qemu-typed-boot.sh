#!/usr/bin/env bash
# Gate (Interpretation B, 2026-09-22): examples/bad/typed_boot.bv — the
# full typed chain bootstrap bad → .bv defn → `bad` fn (contract-checked)
# → per-arch .bad named raw block. QEMU riscv64 must print 'BAD'.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/typed-boot.out
ELF=/tmp/opencode/typed-boot/typed_boot

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }

./target/release/brievc build examples/bad/typed_boot.bv \
    --triple riscv64-unknown-none \
    --linker-script lib/targets/qemu-virt-rv64.ld \
    --out /tmp/opencode/typed-boot >/dev/null 2>&1

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="BAD"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: Interpretation B typed chain prints '$ACTUAL'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
