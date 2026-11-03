# Arena enablement — make `Alloc#` actually select a safe arena (2026-11-03)

Status: ACTIVE. Parent: `2026-11-03-lifetime-and-reclamation.md` Phase 1
prerequisite. Root-cause record: `BUGS.md` "Alloc# never selects the arena".

## Goal

`Alloc#(size)` in a txn/defn must select the bump arena (the efficient
default), produce **valid IR on every dispatch/loop shape**, and run correctly.
This is the prerequisite for per-worker arenas and the parallel substrate.

## Root causes (confirmed by instrumentation)

1. **Wrong strategy id.** `analyze_alloc_strategies` and `analyze_arena_need`
   both run the allocation DAG and both stamp `Expr::Call(Alloc#)` analysis ids
   onto the AST, with different counters (global vs per-function). The second
   pass overwrites codegen's ids, so codegen looks up the wrong strategy and
   selects `Malloc`. Fix: only the strategy pass stamps ids
   (`DagBuilder::stamp_ids`).
2. **Arena subsystem defects** (exposed once selection works):
   - Direct-SSA `main` (`emit_ssa_main`) never inits the arena → `Alloc#` grows
     from a zero base (`brk(0+size)`) → bogus pointer.
   - `emit_arena_fini` `@free`s a `brk`-backed pointer → invalid free.
   - Grow resets the bump to base → overwrites live arena data.
   - Inline check/grow/ok control flow changes folded-loop phi predecessors →
     invalid IR.

## The wheel we already have (do not reinvent)

The loop emitters already solve "inline CF changed the phi predecessor":

- Dedicated latch/end blocks are the header phi's predecessor (`.cdl_`
  `counter.rs:1077`, `.cm_latch` `:437`); bodies branch into them.
- Phis emitted after a body capture the **actual final block** from
  `cur_block` (`body_final` `counter.rs:986`, `guard_pred` `:1074`), with the
  comment that hardcoding the label broke when-ended guards.

Rejected alternative: an out-of-line `@__briev_arena_alloc` helper (option B).
It would be a second mechanism for a problem the existing convention already
solves, and adds a call per allocation.

## Decision

**Reuse the existing convention (option A).** Keep allocation inline. Do not
add a call helper or a per-site inline/call strategy now; add an
`AllocStrategy` call variant later only if measurement shows a hot-loop code
bloat.

## Steps

1. **Version-DAG end blocks** (`emit_version_dag`, `counter.rs:1376-1611`):
   add pre-named `.vd<N>_latch_end` / `.vd<N>_present_end` blocks. Header phis
   name them; each body emits `br <end>`; `<end>: br header`. Mirrors `.cdl_`.
2. **Merge phi** (`counter.rs:2061`): after `emit_countable_body`, capture
   `self.fun.cur_block` and use it as the phi predecessor instead of
   `body_label`.
3. **Audit** for remaining `phi … %<hardcoded body label>` sites (only the two
   above were found).
4. **Arena subsystem fixes:**
   - `emit_ssa_main`: `emit_arena_init` after init stores, `emit_arena_reset`
     at `.ss_main_loop:`, `emit_arena_fini` at `.end:`.
   - `emit_arena_fini`: brk-aware — `brk(base)` shrink instead of `@free`
     (`arena_uses_brk()`).
   - `emit_arena_alloc` grow: preserve the bump (brk: keep `cur`; realloc:
     rebase `cur` by the base delta) instead of resetting to base.
5. **Enable selection:** `analyze_arena_need` sets `stamp_ids = false`.
6. **Gates:**
   - IR gate: a folded-loop `Alloc#` program (e.g. `arena_churn`, a small
     deterministic sum program) emits arena bumps (`aacheck_`), contains no
     `@free` on the brk base, and the module is valid (clang parses/links).
   - Behavioral: the deterministic sum program yields the correct sum; the
     arena is reused (no 64KB/tick growth; leak check).
   - `cargo test --lib`, `daily_use_smoke`, `ll_determinism_check --quick`,
     Praetor, and a runtime A/B (`arena_churn` now measures the arena).

## Validation

- Invalid-IR gate: compile the corpus; any "PHI node entries do not match
  predecessors" is a hard failure.
- `arena_churn` with `BOUND=1000` runs clean; a sum program prints the exact
  expected value.
- No regression in the existing suite; determinism preserved.
- Per-worker arenas (TLS) are the next step and depend on this.

## Risks / undo

- Adding trampoline blocks perturbs version-DAG loop IR; LLVM folds them.
  Undo: remove the end labels and the two phi references.
- `stamp_ids=false` in the arena-need pass is a pure fix (it only stops
  clobbering); undo by restoring the unconditional stamp.
- brk shrink assumes the arena is the sole brk user during a program; note it
  and revisit with the per-worker/mmap design.

## Follow-ups (out of scope here)

- Per-worker arenas via TLS (`clone(CLONE_SETTLS)`), real `sync<group>` join,
  parallel async substrate (lifetime plan Phase 1).
- `AllocStrategy` inline-vs-call variant only if measured.
