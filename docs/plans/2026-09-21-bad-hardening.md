# .bad — Hardening Train

**2026-09-21.** Follow-up to `2026-09-21-bad-application-grade.md` (shipped
same day). Closes the gap between "application-grade" and "fully
functional": every target hardware-verified, addressing correct at
range, FP literals, stack-arg ABI, frame proofs beyond push/pop.

## Decisions (locked in session)

- riscv64 binutils installed during the train → all three targets
  hardware-verified (probe: qemu-aarch64/riscv64 present; aarch64 gnu
  toolchain present; riscv64 binutils installed in Phase 0).
- Merge `feat/bad-dialect` → `main` at the END of the train.
- Comptime contract folding DEFERRED (needs symbolic value tracking —
  its own train). Diagnostics spans deferred (low).

## Phase 0 — toolchain

`apt install binutils-riscv64-linux-gnu` (user-approved sudo).

## Phase 1 — cross-target verification infra

1. `config/bad-registers.dbvl` rows: `cross_as` / `cross_ld` — family →
   toolchain name (`aarch64:aarch64-linux-gnu-as`, ...). `assemble()`
   probes availability; absent toolchain = documented skip, never a
   silent pass.
2. `brievc bad --run`: after link, execute — host binary for the host
   family, `qemu-<family>` otherwise (with `-L` sysroot when the gnu
   cross environment provides one).
3. Tests (gated, honest skip notes): stdlib + hello + struct program
   cross-assembled and RUN under qemu-aarch64 and qemu-riscv64,
   asserting output bytes.

## Phase 2 — aarch64 addressing correctness

`addr` row: `adr $1, $2` (±1MB) → `adrp $1, $2; add $1, $1, :lo12:$2`
(full range, PIC-safe). Data-only row change. Proven by Phase 1 runs.

## Phase 3 — FP literal pool

1. Parser: float literal operand → `BadOperand::Float(f64)`
   (`1.5`, `3.14e-2`).
2. Lowerer literal pool: dedup by bits → `.Lfloat_N: .double v` in a
   trailing `.rodata` block.
3. `fmov` imm rows: x86 `movsd pool(%rip)`, aarch64 `ldr $1, =1.5`
   (GAS literal pool), riscv64 `la t0, pool; fld $1, 0(t0)` (t0
   clobber disclosed). MVP: `fmov` immediates only; fp arithmetic
   composes through registers.

## Phase 4 — proofs + ABI completion

1. `[frame: N]` learns `sub sp, sp, imm` / `add sp, sp, imm`
   displacement (portable `sp`).
2. Stack-passed args (7+): `abi_stack_arg_base` row (x86_64 = 8 after
   the call push, arm/riscv = 0), dialect-doc section, real C-interop
   test: C caller → 8-arg `.export` reading stack args via `loadoff`,
   linked with `cc`, run host + qemu.

## Phase 5 — docs + merge

bad-dialect.md (cross-verify, FP literals, stack args, frame
sp-arith), SPEC §20.1 touch, highlighter float literals — same commit
as structural changes. Then merge to `main`.

## Gates per commit

`cargo test --lib` green (sole expected failure: pre-existing ptx
fixture), no new warnings in changed files, Praetor clean on
`src/backend/bad` + `src/parser`.

## Undo

Additive rows + isolated mechanisms; delete rows/modules, revert the
parser float variant and the `--run` flag.
