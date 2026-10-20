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
- **`call_chain` — exact instruction parity (0) on x86_64.** A symbolic-only
  redundant-move pass coalesces a single-use temp producer into its
  consumer, so `mov tmp, ret; mov arg, tmp` becomes `mov arg, ret` and
  the return copy folds into the producing `add`. The raw kernel uses the
  same C ABI (arg r5, result r0) so the comparison is apples-to-apples.

## Cross-target (aarch64, riscv64)

Same corpus, `TARGET=aarch64` / `TARGET=riscv64`. `uart_banner` is
omitted (it uses the x86-only `outb`).

**aarch64**

| Kernel | raw instr | sym instr | Δ instr | raw bytes | sym bytes | Δ bytes |
|---|---:|---:|---:|---:|---:|---:|
| `vga_loop` | 8 | 8 | 0 | 48 | 48 | 0 |
| `memcpy` | 12 | 12 | 0 | 72 | 72 | 0 |
| `bit_ops` | 5 | 5 | 0 | 32 | 32 | 0 |
| `crc8` | 22 | 21 | −1 | 120 | 104 | −16 |
| `call_chain` | 9 | 7 | −2 | 48 | 40 | −8 |

**riscv64**

| Kernel | raw instr | sym instr | Δ instr | raw bytes | sym bytes | Δ bytes |
|---|---:|---:|---:|---:|---:|---:|
| `vga_loop` | 8 | 8 | 0 | 32 | 32 | 0 |
| `memcpy` | 12 | 12 | 0 | 48 | 48 | 0 |
| `bit_ops` | 5 | 5 | 0 | 24 | 24 | 0 |
| `crc8` | 22 | 21 | −1 | 88 | 84 | −4 |
| `call_chain` | 9 | 7 | −2 | 44 | 36 | −8 |

The loop/memory/bit kernels are at exact parity on every target. `crc8`
is one instruction tighter symbolically. `call_chain` is −2 on
aarch64/riscv64 because the raw kernel is written in the **x86** C-ABI
shape (first arg r5); those targets pass the first argument and the
return in the same register (x0/a0), so the symbolic form needs no
result→argument move across calls. The comparison is therefore a floor,
not a fair ABI comparison, for that one kernel on those targets.

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
bash benchmarks/bad/parity.sh                 # x86_64
TARGET=aarch64 bash benchmarks/bad/parity.sh
TARGET=riscv64 bash benchmarks/bad/parity.sh
bash tests/bare/qemu-rv64-boot-plus-bad.sh
bash tests/bare/qemu-aarch64-boot-plus-bad.sh
bash tests/bare/qemu-multiboot.sh
```
