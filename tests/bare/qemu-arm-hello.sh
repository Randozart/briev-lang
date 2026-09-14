#!/usr/bin/env bash
# Phase 5 gate (rv64-finish plan): ARM Cortex-M3 on QEMU MPS2-AN385 prints
# "Hello, ARM!" through the CMSDK APB UART. Proves the architecture is
# data, not coincidence — the same bootstrap / mechanism-inference pattern
# runs on a second ISA with no Briev-level changes.
#
# Build flow: brievc emits LLVM IR → clang lowers to thumbv7m object →
# ld.lld links with the board's startup.S (vector table + Reset_Handler).
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/hello_arm.out
ELF=/tmp/opencode/hello_arm.elf

command -v qemu-system-arm >/dev/null || { echo "SKIP: qemu-system-arm not installed"; exit 0; }
command -v clang >/dev/null        || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null       || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/hello_arm.b.bv \
    --triple thumbv7m-none-eabi \
    --linker-script lib/targets/qemu-mps2-an385.ld \
    --out /tmp/opencode >/dev/null

clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c /tmp/opencode/hello_arm.b.ll -o /tmp/opencode/hello_arm.o 2>/dev/null
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/boards/mps2-an385/startup.S -o /tmp/opencode/arm_startup.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.c -o /tmp/opencode/crt_arm_c.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.S -o /tmp/opencode/crt_arm_s.o
ld.lld -T lib/targets/qemu-mps2-an385.ld --entry Reset_Handler \
    /tmp/opencode/arm_startup.o /tmp/opencode/hello_arm.o \
    /tmp/opencode/crt_arm_c.o /tmp/opencode/crt_arm_s.o -o "$ELF"

timeout 3 qemu-system-arm -machine mps2-an385 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(head -c 12 "$OUT" | tr -d '\n')
EXPECT="Hello, ARM!"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: ARM Cortex-M hello world ($ACTUAL)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi
