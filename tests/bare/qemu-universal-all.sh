#!/usr/bin/env bash
# Gate (universal completion, 2026-09-22): `--all-targets` — ONE
# `brievc build examples/bad/bootloader.bv --all-targets` invocation
# builds a binary per [target.*] profile in examples/bad/briev.toml
# (rv64/arm/aarch64, each with its triple/linker-script/entry), and
# every resulting binary boots its QEMU machine printing 'universal
# boot'.
set -euo pipefail
cd "$(dirname "$0")/../.."

OUTDIR=/tmp/opencode/uni-all
EXPECT="universal boot"

command -v qemu-system-riscv64 >/dev/null || { echo "SKIP: qemu-system-riscv64 not installed"; exit 0; }

./target/release/brievc build examples/bad/bootloader.bv \
    --all-targets --out "$OUTDIR" >/dev/null 2>&1

boot() {  # name qemu-binary machine extra-args...
    local name=$1 qemu=$2 machine=$3; shift 3
    command -v "$qemu" >/dev/null || { echo "SKIP: $qemu not installed"; return 0; }
    local out=/tmp/opencode/uni-all-$name.out
    timeout 3 "$qemu" -machine "$machine" -nographic "$@" \
        -kernel "$OUTDIR/$name/bootloader" > "$out" 2>/dev/null || true
    local actual
    actual=$(tr -cd '[:print:]' < "$out")
    if [[ "$actual" == "$EXPECT" ]]; then
        echo "PASS: --all-targets $name prints '$actual'"
    else
        echo "FAIL: $name expected '$EXPECT', got '$actual'"
        exit 1
    fi
}

boot rv64   qemu-system-riscv64 virt  -bios none
boot arm    qemu-system-arm    mps2-an385
boot aarch64 qemu-system-aarch64 virt -cpu cortex-a57
