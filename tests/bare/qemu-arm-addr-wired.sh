#!/usr/bin/env bash
# Phase 4b gate (rv64-finish plan): address-wired (reactor-pass) polling on
# ARM Cortex-M3 (QEMU MPS2-AN385). SysTick runs WITHOUT its interrupt — the
# only eligibility is the `node poll_val @ *(...)` address-wired node. The
# reactor must spin and re-read VAL every pass; a wfi park would sleep
# forever. Golden: digits print as VAL changes.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=/tmp/opencode/addr_wired.out
ELF=/tmp/opencode/addr_wired.elf

command -v qemu-system-arm >/dev/null || { echo "SKIP: qemu-system-arm not installed"; exit 0; }
command -v clang >/dev/null        || { echo "SKIP: clang not installed"; exit 0; }
command -v ld.lld >/dev/null       || { echo "SKIP: ld.lld not installed"; exit 0; }

./target/release/brievc build examples/addr_wired.b.bv \
    --triple thumbv7m-none-eabi \
    --linker-script lib/targets/qemu-mps2-an385.ld \
    --out /tmp/opencode >/dev/null

clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c /tmp/opencode/addr_wired.b.ll -o /tmp/opencode/addr_wired.o 2>/dev/null
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/boards/mps2-an385/startup.S -o /tmp/opencode/arm_startup.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.c -o /tmp/opencode/crt_arm_c.o
clang --target=thumbv7m-none-eabi -mcpu=cortex-m3 \
    -c lib/runtime/compiler_rt_arm.S -o /tmp/opencode/crt_arm_s.o
ld.lld -T lib/targets/qemu-mps2-an385.ld --entry Reset_Handler \
    /tmp/opencode/arm_startup.o /tmp/opencode/addr_wired.o \
    /tmp/opencode/crt_arm_c.o /tmp/opencode/crt_arm_s.o -o "$ELF"

timeout 3 qemu-system-arm -machine mps2-an385 -nographic \
    -kernel "$ELF" > "$OUT" 2>/dev/null || true

ACTUAL=$(head -c 24 "$OUT" | tr -d '\n')
EXPECT="012345678901234567890123"
if [[ "$ACTUAL" == "$EXPECT" ]]; then
    echo "PASS: address-wired polling spins, never sleeps ($ACTUAL...)"
else
    echo "FAIL: expected '$EXPECT', got '$ACTUAL'"
    exit 1
fi