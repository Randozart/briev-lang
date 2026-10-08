<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# BILLD — Briev Intermediate Low-Level Dialect (`.bld`)

**Status: M4 DONE 2026-10-08** (register allocator + 10 tests, suite 3021
green, Praetor clean; M3: lowering core + 55 tests; M2: parser + AST + 26
tests — all same day). Milestones below; check them off as they land.

Plan-driven work; Rule 13 docs named in §Milestones. Separate worktree:
`../briev-billd`, branch `feat/billd-dialect` (does not touch main;
foreign-lane rules of `docs/plans/INDEX.md` apply).

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
| Keywords | lowercase (`when`, `loop`, `break`, `let`, `defn`, `bootstrap`) | one grammar habit across dialects; contextual in the BILLD parser — NO global lexer keyword change (a `.bv` identifier `loop` must not break). **2026-10-08 amendment**: the conditional is `when` in EVERY dialect — Briev has no `if` (`.bv` never had it); a statement-head `if` errors with a `when` fix |
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
   `defn`, `loop`/`while`/`when`/`break`/`continue`, `let`, PascalCase calls,
   `bad { }` blocks, `import`. Round-trip + house-style error tests
   (what/why/fix, `src/errors.rs`).
3. **Lowering core** — DONE 2026-10-08: `src/backend/bld/{mod,lower}.rs`,
   55 tests (`backend::bld`), one end-to-end `.s` triple. Also in this
   commit: `bad-registers.dbvl`'s `abi_args_fp` listed as a scalar row
   (it parsed as a register row and silently emptied the accessor).

   M3 decisions (each test-pinned in `src/backend/bld/mod.rs`):
   - **Virtual registers in the emitted BadProgram are intentional.**
     The .bad backend resolves only real register names on value
     operands, so M3 golden tests assert on the `BadProgram` shape;
     only const/physical recipes reach `.s` (three do, end to end).
     Virtuals are `vN` (Int/Bool/Ptr) and `fvN` (Float) — the class
     rides the name so the M4 allocator reads it off the BadProgram
     with no extra state; neither prefix collides with a register name.
   - **`let` adopts freshly emitted single-def value registers** (the
     `Lowered::temp` flag): `let b = a + 1` emits `add v0, …` with no
     second copy. Values read from existing bindings never adopt
     (adopting would alias `let x = y` with `y`).
   - **ABI convention** (mirrors `compile.rs` `bad_param_env`):
     integer-class parameters/consumers take the `abi_args` order,
     float parameters the `abi_args_fp` order (class-separate
     counters), results return in r0/f0. Register-budget overflow is
     loud (frameless v1); `.bv`-lane `bad fn` starts at register index
     1 for the state pointer — `.bld` defns start at index 0 (no state
     pointer; `.bld` calls are `.bld`-to-`.bld` or C-ABI external).
   - **Calls stage through fresh virtuals first** (parallel-move
     safety): `f(b, a)` with both args already in argument registers
     must not clobber a source mid-copy.
   - **Float compare/branch operands materialize to float registers**
     before the branch — the `fj*` rows have no immediate form, and a
     pooled literal substituted into a register slot mis-assembles.
     Integer immediates pass directly (every `j*` row accepts them).
   - **No-immediate ops materialize via the registry** (`ImmHandling::
     Illegal`): `mul`/`div`/`mod` on aarch64/riscv/thumb, all `f*` ops
     — one generic check, no per-op knowledge.
   - **Numeric promotion is the defined semantics**: Int↔Float
     converts automatically (itof/ftoi; constants fold exactly, a
     fractional Float → Int is loud). Int/Bool/Ptr interchange as the
     same machine word (class relabel, no instruction).
   - **`>>` folds as a LOGICAL shift** (`(x as u64) >> n`), matching
     the .bad `shr` (shrq/lsr/srl) on every target; `<<` wraps bits.
     Const division by zero is loud; float division by zero folds to
     inf (hardware behavior).
   - **`bad { }` ownership rule**: instruction lines splice into the
     recipe body anywhere (parsed inside a `_bldwrap:` label so they
     can never orphan); ownership items (sections, data labels, defns,
     raw blocks) attach only at the recipe's FIRST or LAST statement —
     .bad ownership is positional, a mid-recipe owner would steal the
     instructions after it.
   - **`.bad` imports** pass through as directives (absolutized paths)
     and their labels/named raw blocks are harvested as external call
     signatures (C-ABI, result assumed r0/Int, arity unchecked);
     `.bad` sequence defns keep their arity. `.bld` imports merge
     recursively (canonical-path dedup; `root_path` seeds it so cycles
     back to the root file terminate — without it, a root cycle is
     still loud via duplicate-declaration errors, never silent).
   - **Conditions are branch-shaped** (`lower_cond(e, target,
     when_true)`): `when`/`while` jump on false polarity, `&&`/`||`
     short-circuit with explicit dual-polarity tables (`branch_op` +
     `dual`), and boolean VALUES use the dance (`mov 1` / branch /
     `mov 0` / labels). Labels: `whe/whend/wlp/wlpend/lpe/lpend/bf/be`.
   - **`when`, never `if`** — enforced at the parser (2026-10-08
     amendment, commit `47ef9b55`); the lowerer matches `BldStmt::When`.
4. **Register allocator** — DONE 2026-10-08: `src/backend/bld/alloc.rs`,
   wired into `generate_with` (`lower_to_bad` stays allocator-free for
   the M3 golden tests). Kani harness drafted and DROPPED by decision
   (2026-10-08): Kani is retired on this lane — the repo's kani gate
   currently verifies nothing (harnesses gated on `feature = "kani"`,
   which `cargo kani` does not set, so no harness ever runs), and full
   runs cost minutes of whole-crate codegen for proofs nobody executes.
   Repo-wide Kani policy cleanup belongs to the main lane (BUGS.md
   candidate). The allocator's safety invariant — no two allocated
   intervals share a register slot — is held constructively by the scan
   (expiry at `end < start`, one free-list, no reuse while active) and
   pinned by the emitted-shape tests.

   M4 decisions (test-pinned in `src/backend/bld/mod.rs`):
   - **Intervals are linear-order with loop extension.** A virtual's
     interval is [first def, max(last def/use)], extended to every
     loop's backedge when the value is defined before the loop and used
     inside it — the conservative fix that makes linear-order intervals
     SAFE under backedges (a loop-carried value must survive
     iterations). Extension only widens; overlap checks stay
     conservative (safe), never optimistic (wrong).
   - **Call walls**: a value whose interval contains a `call` takes a
     callee-saved register (registry `RegProp`) or a frame slot — never
     caller-saved. Call-argument staging copies run BEFORE the call, so
     argument intervals end at the wall (no forced spill). Float
     values crossing a call always spill on x86_64 (no callee f-regs);
     aarch64/riscv64 route to their callee f-regs.
   - **Pools come from the registry**: r0-r15/f0-f15 that resolve on
     the family, minus the ABI argument registers and the return
     registers (r0/f0). Two scratch registers per class are reserved
     from the callee-saved TAIL — scratches are never live across
     anything (a reload feeds the next instruction), so their property
     is irrelevant, and keeping caller-saved registers free lets the
     common recipe allocate with no frame at all.
   - **Frames**: opened iff something spilled OR a callee-saved
     register was used; callee-saved regs are push/pop-saved in the
     prologue and restored in the epilogue (C-ABI callers own them).
     Frame size = spill slots × 8 + push_width × saves, 16-aligned; the
     epilogue runs BEFORE every `ret` and at fall-off-the-end (the
     frame the compiler opened is undone — no auto-ret, naked
     semantics stand). First cut emitted the epilogue AFTER the ret —
     unreachable dead code; the .bad W3 sp-tracker caught it.
   - **`sp` guard**: a recipe whose assembly touches `sp` cannot take a
     frame — ANY frame (saves push/pop too, not just spills) — so spill
     needs there are loud, naming the values.
   - **Param stash**: a recipe whose body contains any call cannot keep
     parameters in caller-saved ABI argument registers (the callee
     clobbers them — the M3 design note's caveat, now closed):
     `bind_params` stashes each parameter into a virtual at entry and
     the allocator routes it like any call-crossing value. Leaf recipes
     keep the zero-cost ABI binding.
   - **`vN`/`fvN` namespace reserved**: internal defn/bootstrap names
     matching the virtual pattern are rejected at collect — the
     allocator rewrites those operand names mechanically. External
     `.bad` labels named `vN` stay legal (the allocator works from
     operand roles, not name patterns).
   - **`bad { }` blocks naming a `vN` are loud**: raw assembly works in
     physical registers; compiler-managed names are not addressable.
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
