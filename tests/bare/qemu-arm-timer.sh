#!/usr/bin/env bash
# Phase 5 gate (rv64-finish plan): SysTick-driven tick count on ARM Cortex-M3
# (QEMU MPS2-AN385) prints decimal digits. Proves the second-architecture
# proof is not a hello-world fluke: the same reactor / @ vector / mechanism-
# inference pattern drives a timer on a completely different ISA.
#
# Build flow: brievc emits LLVM IR → clang lowers to thumbv7m object → ld.lld
# links with the board's startup.S (vector table + .data copy) and the ARM
# compiler-rt shims (AEABI division + memclr).
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/timer_arm.out
ELF=/tmp/opencode/timer_arm.elf

command -v qemu-system-arm >/dev/null || { echo "SKIP: qemu-system-arm not installed"; exit 0; }
command -v clang >/dev/null        || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null       || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/timer_arm.b.bv \
    --triple thumbv7m-none-eabi \
    --linker-script lib/targets/qemu-mps2-an385.ld \
    --out /tmp/opencode >/dev/null

clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c /tmp/opencode/timer_arm.b.ll -o /tmp/opencode/timer_arm.o 2>/dev/null
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/boards/mps2-an385/startup.S -o /tmp/opencode/arm_startup.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.c -o /tmp/opencode/crt_arm_c.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.S -o /tmp/opencode/crt_arm_s.o
ld.lld -T lib/targets/qemu-mps2-an385.ld --entry Reset_Handler \
    /tmp/opencode/arm_startup.o /tmp/opencode/timer_arm.o \
    /tmp/opencode/crt_arm_c.o /tmp/opencode/crt_arm_s.o -o "$ELF"

timeout 4 qemu-system-arm -machine mps2-an385 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(head -c 24 "$OUT" | tr -d '\n')
EXPECT="123456789012345678901234"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: ARM Cortex-M SysTick timer ($ACTUAL...)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi