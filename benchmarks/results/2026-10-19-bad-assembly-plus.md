<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# `.bad` assembly+ — scalar parity results

**Date:** 2026-10-20
**Plan:** `docs/plans/2026-10-19-bad-assembly-plus.md`
**Harness:** `benchmarks/bad/parity.sh` (release `brievc`, `--target x86_64`, instruction + assembled `.text` byte counts)
**Corpus:** `benchmarks/bad/<kernel>_{raw,sym}.bad`

Instruction parity is the contract for scalar symbolic code. A delta is
recorded, never hidden. Cycles are **not** reported: QEMU timing is not
representative of hardware, so the ledger measures instructions and
assembled bytes, which are deterministic.

## Results

| Kernel | raw instr | sym instr | Δ instr | raw bytes | sym bytes | Δ bytes |
|---|---:|---:|---:|---:|---:|---:|
| `vga_loop` | 8 | 8 | 0 | 80 | 80 | 0 |
| `uart_banner` | 14 | 25 | +11 | 104 | 133 | +29 |
| `memcpy` | 12 | 12 | 0 | 96 | 96 | 0 |
| `bit_ops` | 6 | 6 | 0 | 71 | 71 | 0 |
| `crc8` | 24 | 22 | −2 | 141 | 134 | −7 |
| `call_chain` | 9 | 9 | 0 | 80 | 80 | 0 |

## Per-kernel analysis

- **`bit_ops`, `memcpy`, `vga_loop` — exact instruction parity (0).** The
  in-place assignment and accumulate-form fixes land, and a trailing
  `when cond: break` now fuses with the loop back-edge into one inverted
  conditional branch (`jne head`) instead of `je exit; jmp head`. The
  `let b` declaration folds into the first `load b, src` (no dead
  initializer).
- **`crc8` — −2 (symbolic tighter).** The nested-loop register reuse
  coalesces slightly better than the hand-written raw form; the byte
  delta (−7) follows from tighter register choice. Its breaks sit at the
  top of each loop (not trailing), so the back-edge fusion does not apply.
- **`uart_banner` — +11.** `outb` pins the hardware's `%al`/`%dx`, so the
  ISA row stages both operands (`movb $1.w8, %al; movw $2.w16, %dx;
  outb %al, %dx`) and the immutable port re-materializes per store. The
  raw form keeps port in `%rdx` and writes `outb %al, %dx` directly.
  x86 port I/O is the escape-hatch shape; parity is not expected here.
- **`call_chain` — exact instruction parity (0).** A symbolic-only
  redundant-move pass coalesces a single-use temp producer into its
  consumer, so `mov tmp, ret; mov arg, tmp` becomes `mov arg, ret` and
  the return copy folds into the producing `add`. The raw kernel uses the
  same C ABI (arg r5, result r0) so the comparison is apples-to-apples.

## Raw identity

The raw corpus is unchanged: `tests/bad_raw_golden.rs` pins the exact
assembly for physical-register code across x86_64/aarch64/riscv64/thumb.
Assembly+ lowering only runs on bodies that contain symbolic items; a
raw-only label never enters it, so its output is byte-identical.

## Boot gates

Not parity, but the hardware-verification companion:

- `tests/bare/qemu-rv64-boot-plus-bad.sh` — PASS (`Briev`)
- `tests/bare/qemu-aarch64-boot-plus-bad.sh` — PASS (`Briev`)
- `tests/bare/qemu-multiboot.sh` — PASS (`Briev`, then `R64` from the
  symbolic 64-bit body)

## Reproduce

```sh
cargo build --release
bash benchmarks/bad/parity.sh
bash tests/bare/qemu-rv64-boot-plus-bad.sh
bash tests/bare/qemu-aarch64-boot-plus-bad.sh
bash tests/bare/qemu-multiboot.sh
```
