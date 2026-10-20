#!/usr/bin/env bash
# .bad assembly+ parity harness (2026-10-20, bad-assembly+ plan).
#
# Compiles each raw/symbolic kernel pair to target assembly, counts
# instructions and assembled .text bytes, and prints a markdown table.
# Instruction parity is the contract for scalar symbolic code; a delta is
# recorded, not hidden. Cycles are not measured here (QEMU timing is not
# representative) — the ledger records instruction/byte deltas.
#
#   TARGET=x86_64|aarch64|riscv64 bash benchmarks/bad/parity.sh
#
# A kernel whose source does not compile on the target (e.g. x86-only
# `outb`) is skipped, not silently counted.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${TARGET:-x86_64}"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
WORK="${WORK:-/tmp/opencode/bad-parity-$TARGET}"

case "$TARGET" in
    x86_64*)  AS=as;                       ASFLAGS=--64 ;;
    aarch64*) AS=aarch64-linux-gnu-as;     ASFLAGS= ;;
    riscv64*) AS=riscv64-linux-gnu-as;     ASFLAGS= ;;
    *)        AS=as;                       ASFLAGS= ;;
esac

[[ -x "$BRIEVC" ]] || { echo "missing $BRIEVC (build --release first)" >&2; exit 1; }
command -v "$AS" >/dev/null || { echo "missing $AS" >&2; exit 1; }
command -v size >/dev/null  || { echo "missing size" >&2; exit 1; }

mkdir -p "$WORK"
count_instr() {
    awk '
        { line=$0; sub(/^[ \t]+/,"",line); sub(/[ \t]+$/,"",line) }
        line==""            { next }
        line ~ /^[.#]/      { next }
        line ~ /^\/\//      { next }
        line ~ /:$/         { next }
        { c++ }
        END { print c+0 }
    ' "$1"
}

text_bytes() {
    "$AS" $ASFLAGS -o "$2" "$1" 2>/dev/null
    size "$2" | tail -1 | awk '{ print $1 }'
}

KERNELS=(vga_loop uart_banner memcpy bit_ops crc8 call_chain)

echo "| Kernel | raw instr | sym instr | Δ instr | raw bytes | sym bytes | Δ bytes |"
echo "|---|---:|---:|---:|---:|---:|---:|"
for k in "${KERNELS[@]}"; do
    raw="$ROOT/benchmarks/bad/${k}_raw.bad"
    sym="$ROOT/benchmarks/bad/${k}_sym.bad"
    [[ -f "$raw" && -f "$sym" ]] || continue
    "$BRIEVC" bad "$raw" --target "$TARGET" --emit-asm >/dev/null 2>&1 || continue
    "$BRIEVC" bad "$sym" --target "$TARGET" --emit-asm >/dev/null 2>&1 || continue
    rs="$ROOT/benchmarks/bad/${k}_raw.s"
    ss="$ROOT/benchmarks/bad/${k}_sym.s"
    ri=$(count_instr "$rs"); si=$(count_instr "$ss")
    rb=$(text_bytes "$rs" "$WORK/${k}_raw.o"); sb=$(text_bytes "$ss" "$WORK/${k}_sym.o")
    echo "| \`$k\` | $ri | $si | $((si - ri)) | $rb | $sb | $((sb - rb)) |"
    rm -f "$rs" "$ss"
done
