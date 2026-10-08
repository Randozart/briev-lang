<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# BILLD — Briev Intermediate Low-Level Dialect (`.bld`)

**Status: PLANNED 2026-10-08.** Plan-driven work; Rule 13 docs named in
§Milestones. Separate worktree: `../briev-billd`, branch `feat/billd-dialect`
(does not touch main; foreign-lane rules of `docs/plans/INDEX.md` apply).

## The gap

The tier ladder has a hole:

| Tier | Dialect | Nature | Guarantee |
|---|---|---|---|
| High | `.bv` Briev | expressive, proven | bounded loops, reactor, contracts |
| **Mid** | **`.bld` BILLD** | **execution recipe / engine manual** | **unbounded loops, symbolic registers, naked code** |
| Low | `.bad` Briev Assembly | exact instructions | physical registers r0-r15, per-target rows |

- **Assembly is too granular**: the author micromanages scratch registers for
  values that have no architectural meaning (`mov eax, cr0; or eax, 1;
  mov cr0, eax` — `eax` is a forced mechanical bucket).
- **C is too detached**: it assumes stack, frames, ABI, virtual memory — you
  cannot write the first 50 instructions of a boot sequence in pure C.

BILLD sits between: **the architecture of the machine without the chore of
the scratchpad.** Code reads as an execution manual / flight checklist:
symbolic dataflow + PascalCase engine verbs (`DisableInterrupts()`,
`ReadControlReg(0)`, `FarJump(seg, off)`).

## Confirmed decisions

| Decision | Choice | Rationale |
|---|---|---|
| Extension | `.bld` | free (grepped zero hits), matches 3-letter dialect convention |
| Intrinsics | bare PascalCase, dialect-gated | extension boundary IS the Rule-3 disclosure; registry = config data (Rules 3/15/23) |
| Keywords | lowercase (`loop`, `if`, `break`, `let`, `defn`, `bootstrap`) | one grammar habit across dialects; contextual in the BILLD parser — NO global lexer keyword change (a `.bv` identifier `loop` must not break) |
| Lowering | `.bld` AST → `BadProgram` AST → existing `.bad` backend | reuses `bad-isa.dbvl`, `bad-registers.dbvl`, contracts, raw blocks, `--raw-bin`, `--run`, all targets; ladder: BILLD → .bad → machine |
| Targets | all `.bad` targets (x86_64, aarch64, riscv64, thumbv7m) | ISA rows are data; per-target absence = loud capability error |
| Registers | SSA-shaped values → linear scan over r0-r15 → frame spill (`loadoff`/`storeoff`); frameless spill = loud error naming the value | Rule 2 efficient default; LuaJIT/clang -O1 class allocator; boot recipes rarely spill |
| `.bad` reach | BOTH inline `bad { ... }` passthrough blocks AND `import "x.bad"` | raw-block doctrine; sections/data/contracts need no reimplementation |
| Entry | `bootstrap Name() { ... }` | established terminology (`machine-entry.md`, `bootstrap bad`); `entry` would collide with typechecker entry-loop + linker ENTRY |
| Bit functions | `set_bit`/`clear_bit`/`test_bit`/`toggle_bit` in `lib/std/bits.bv` (shared with `.bv`) + `lib/std/bld/bits.bld` wrappers | Rule 14: stdlib, not Rust; mirrors the `std/bad/` vs `lib/std/` split |
| Loops | `loop { }` + `while cond { }` sugar + `break`/`continue`; NO termination analysis | physical world needs unbounded spin (polling, spinlocks, idle loop); the lane never enters `.bv` gates |
| Contracts | none in v1 `.bld` grammar; `bad { }` blocks carry `.bad` positional contracts | zero reimplementation; contract-first preserved |

## Architecture

```
.bld source
  → lexer   (reuse Briev Token; `loop`/`while`/... contextual, BILLD-parser only)
  → parser  (src/parser/bld.rs — braced Briev-style expressions)
  → AST     (src/ast/bld.rs)
  → analysis-lite (type inference Int/Float/Ptr/Bool, definite assignment)
  → lower   (src/backend/bld/lower.rs)
        • structured control flow → labels + jmp/jz/jnz (unbounded OK)
        • expressions → virtual regs → linear scan → r0-r15 (+ frame spill)
        • PascalCase intrinsic → config/bld-intrinsics.dbvl row →
              .bad op sequence | call <stdlib symbol>
        • `bad { }` block → passthrough lines
        • `bootstrap Name` → exported label (.global + ENTRY)
  → BadProgram → src/backend/bad/* (UNCHANGED) → .s → .o → binary
```

### Registration checklist (mirror `.bad`)

- `config/targets.dbvl`: `.bld → bld` row
- `BackendKind::Bld` (`src/target.rs`) + resolve + golden row tests
- `SourceKind::Bld` + classify arm (`src/conformance.rs`)
- `brievc bld` subcommand + default-path ext arm (`src/main.rs`)
- `src/compile.rs` route arm — hard error directing to `brievc bld`
  (same shape as `Bad`, `src/compile.rs:2422-2432`)
- import resolver arm: `.bld` imports `.bad`/`.bld`; records provenance
  (`src/import_resolver.rs`, beside the `.bad` arm `:944-1003`)
- vocab/highlighter extension list (`src/vocab.rs:285`)
- `split_known_code_ext` only if `.bld` participates in code imports

### Intrinsic registry — `config/bld-intrinsics.dbvl`

Data rows (loader beside `src/backend/bad/registry.rs`), NOT Rust string
matches (Rule 15/23). Per row: name, arity, observable flag, per-target
lowering template (`.bad` op sequence or `call <sym>`).

v1 set:

| Intrinsic | Lowering |
|---|---|
| `ReadControlReg(n)` / `WriteControlReg(n, v)` | x86 new `bad-isa.dbvl` CR rows; riscv64 `csrr/csrw`; aarch64 MRS/MSR rows |
| `DisableInterrupts()` / `EnableInterrupts()` | `cli/sti`; `csrrc/csrrs mstatus`; aarch64 DAIF |
| `SetBit(v,i)` / `ClearBit` / `ToggleBit` / `TestBit` | `lib/std/bld/bits.bld` defns (raw `\|=`/`&=` escape hatch also legal) |
| `LoadDescriptorTable(base, limit)` | x86 `lgdt`; other targets = loud capability error |
| `InvalidateTlb()` / `MemoryBarrier()` | per-target (`sfence.vma`/`tlbi`/`invlpg` families) |
| `Halt()` / `WaitForInterrupt()` | `.bad` `halt` row (`hlt`/`wfi`) |
| `FarJump(seg, off)` | x86 `ljmp`-form; riscv/aarch64 = unconditional `jmp` (no segmentation) |

Rules: target-absent row = loud capability error naming available targets
(use-site capability doctrine). Intrinsic that cannot express a sequence =
author falls to `bad { }` raw block — no silent pass, no compiler knowledge
of specific types.

### Naked semantics

Lowering emits no prologue/epilogue, no auto-`ret` unless written. What you
write is the sequence. (Spill code in an initialized frame is the one
documented exception; frameless spill refuses loudly.)

## Milestones (each: tests + commit + Rule 13 docs)

1. **Plan doc (this file) + worktree setup.**
2. **Parser + AST** — `src/parser/bld.rs`, `src/ast/bld.rs`: `bootstrap`,
   `defn`, `loop`/`while`/`if`/`break`/`continue`, `let`, PascalCase calls,
   `bad { }` blocks, `import`. Round-trip + house-style error tests
   (what/why/fix, `src/errors.rs`).
3. **Lowering core** — expressions, assignment, structured CF → labels,
   `bootstrap` export; golden tests on emitted `BadProgram`/`.s` fragments.
4. **Register allocator** — linear scan + frame spill; Kani harness
   (no two simultaneously-live values share a reg; frameless-spill error).
5. **Intrinsic registry + engine intrinsics** — `bld-intrinsics.dbvl`,
   new `bad-isa.dbvl` rows (CR/MRS/MSR), per-target capability-error tests.
   (Lane bypasses the `.bv` interpreter like `.bad` does — Rule 5 applies
   to the `.bv` surface; interpreter-first addition rule does not apply to
   a lane that never enters it.)
6. **Bit functions** — `lib/std/bits.bv` additions (checked by `.bv` tests)
   + `lib/std/bld/bits.bld`.
7. **End-to-end examples** — `examples/bld/boot_protected_x86.bld` (the
   CR0/protected-mode recipe), `boot_rv64.bld`, `boot_aarch64.bld`; gate =
   QEMU output equality via the `.bad` `--run` harness pattern (toolchain
   absent = printed skip, never silent).
8. **Docs** — `docs/architecture/bld-dialect.md` (grammar table, tier table,
   registry, **To undo** section), SPEC §20.2, `vocab.rs`, INDEX.md row,
   `primitive-coverage.md` gap closure. Syntax highlighter updated.

**Non-goals v1**: `.bv` ↔ `.bld` bridge (`bld fn`, phase 2 mirrors
`bad fn`), contracts in `.bld` grammar, PTX family, LSP beyond basics,
performance benchmarks (not a throughput tier — correctness/QEMU gates
carry Rule 12 instead; record that explicitly in results when run).

## Gates

- `cargo test --lib` green; no new warnings
- Praetor `--warn` on new dirs (`src/parser`, `src/ast`, `src/backend/bld`)
- conformance sweep green; `brievc freshness` clean
- QEMU boot equality per target (skip printed if toolchain absent)
- `git grep 'Type::Custom.*==' src/backend/` stays zero (Rule 19)
