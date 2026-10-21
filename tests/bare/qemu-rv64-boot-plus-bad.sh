#!/usr/bin/env bash
# Pure .bad assembly+ boot on rv64 QEMU virt: profile constants, inline
# defn steps, engine verbs, address-of, no calls/frames, no .bld.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT=/tmp/opencode/boot_plus_rv64.out
WORK=/tmp/opencode/boot_plus_rv64

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }
command -v clang >/dev/null             || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null            || { echo "SKIP: ld.lld not installed"; exit 0; }
[[ -x "$ROOT/target/release/brievc" ]] || { echo "SKIP: release brievc not built"; exit 0; }

mkdir -p "$WORK"
cp "$ROOT/examples/bad/boot_plus_rv64.bad" "$WORK/boot_plus_rv64.bad"
# Run from the repo root so `std/bad/...` imports resolve; the linked
# binary lands in the stem's cwd.
cd "$ROOT"
"$ROOT/target/release/brievc" bad "$WORK/boot_plus_rv64.bad" \
    --target riscv64-unknown-none \
    --linker-script "$ROOT/lib/targets/qemu-virt-rv64.ld" >/dev/null
ELF="$ROOT/boot_plus_rv64"

timeout 3 qemu-system-riscv64 -machine virt -bios none -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev"
if [[ "$ACTUAL" == *"$EXPECT"* ]]; then
    echo "PASS: pure .bad assembly+ rv64 prints '$EXPECT'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
rm -f "$ELF" "$ROOT/boot_plus_rv64"
