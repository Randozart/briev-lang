#!/usr/bin/env bash
# Pure .bad assembly+ boot on aarch64 QEMU virt: profile constants, inline
# defn steps, PL011 sub-width stores, engine verbs, no calls/frames.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT=/tmp/opencode/boot_plus_aarch64.out
WORK=/tmp/opencode/boot_plus_aarch64
ELF="$ROOT/boot_plus_aarch64"

command -v qemu-system-aarch64 >/dev/null || { echo "SKIP: qemu-system-aarch64 not installed"; exit 0; }
command -v clang >/dev/null               || { echo "SKIP: clang not installed"; exit 0; }
command -v aarch64-linux-gnu-ld >/dev/null || { echo "SKIP: aarch64-linux-gnu-ld not installed"; exit 0; }
[[ -x "$ROOT/target/release/brievc" ]] || { echo "SKIP: release brievc not built"; exit 0; }

mkdir -p "$WORK"
cp "$ROOT/examples/bad/boot_plus_aarch64.bad" "$WORK/boot_plus_aarch64.bad"
cd "$ROOT"
"$ROOT/target/release/brievc" bad "$WORK/boot_plus_aarch64.bad" \
    --target aarch64-unknown-none \
    --linker-script "$ROOT/lib/targets/qemu-virt-aarch64.ld" >/dev/null

timeout 3 qemu-system-aarch64 -machine virt -cpu cortex-a57 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(tr -cd '[:print:]' < "$OUT")
EXPECT="Briev"
if [[ "$ACTUAL" == *"$EXPECT"* ]]; then
    echo "PASS: pure .bad assembly+ aarch64 prints '$EXPECT'"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
rm -f "$ELF"
