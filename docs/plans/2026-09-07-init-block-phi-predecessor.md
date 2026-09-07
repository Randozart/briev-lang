# Init-block phi predecessor fix — all fold engines

**Date:** 2026-09-07
**Status:** COMPLETE (commit `c4ee2e19`)
**Baseline:** `3709f832` (HEAD at slice start — countdown swan-song dominance fix)

## Problem

`hash_ops_idio` fails to compile with:

```
error: invalid LLVM IR input: PHI node entries do not match predecessors!
  %cdc117 = phi i64 [ %t114, %entry ], [ %cdn118, %.cdl_110 ]
label %entry
label %.match_end_71
Instruction does not dominate all uses!
  %t114 = load i64, ptr %t113, align 8
  %cdc117 = phi i64 [ %t114, %entry ], [ %cdn118, %.cdl_110 ]
```

## Root cause

`emit_match` (`src/backend/llvm/emit_expr.rs:4811`) emits its condition chain
inline in the caller's current block, opens `.match_arm_*`/`.match_next_*`/
`.match_end_*` blocks, and **leaves `cur_block = Some(".match_end_N")`**
(`emit_expr.rs:4970`).

In every fold engine, `emit_inline_init_stores` runs while the entry block is
open. When a state field's `op Init` body contains a `match` — exactly
`HashMap.init` (`lib/std/collections.bv:139`):

```
let c: Int = match capacity {
    0 => 256,
    _ => capacity,
};
```

triggered by `hash_ops_idio.bv:19` (`let m: HashMap<Int,Int> = 2*N`) — the
match blocks land inside entry, and `cur_block` is left at `.match_end_71`.

The counter/field **init loads** and the **`br label %header`** then emit into
`.match_end_71` (the leaked `cur_block`), *not* entry. So:

1. The block that actually branches to the header is `.match_end_71`, not
   `%entry`.
2. The header phis hardcode `[ init, %entry ]`.

Two verifier violations:
- **phi cites a non-predecessor** (`%entry` is not a predecessor of the
  header; `.match_end_71` is).
- **dominance**: the init load register (e.g. `%t114`) is defined in
  `.match_end_71`, not `%entry`.

The `cur_block` leak is *accidentally correct placement* — the loads and `br`
are in the right block. Only the **phi citation** is wrong.

## Affected engines

All four fold engines share the defect. The trigger set is: *a state field
whose `op Init` body emits blocks (currently only `HashMap.init` has a
match)*, dispatched to any fold engine.

| Engine | Function | Init loads site | Phi sites (`[ init, %entry ]`) |
|---|---|---|---|
| countdown | `emit_countable_countdown_main` | counter.rs:997 | 1039, 1041, 1059 |
| PerFieldPhi | `emit_countable_loop_wrapped` | counter.rs:333 (in loop_buf) | 371, 374, 402 |
| version-DAG | `emit_version_dag_main_inner` | counter.rs:1498, 1508 | 1553, 1584 |
| folded | `emit_folded_main` | counter.rs:212+ | 136, 144 |
| SSA | `emit_ssa_main` | ssa.rs:220 (canonical setup) | 225 (cites `%.ss_loop`, verify) |

Only `hash_ops_idio` (countdown) and `test_hashmap_surface.bv` (SSA) currently
fire. The rest are latent.

## Fix

Capture `self.fun.cur_block` (the block the init loads + `br` actually emit
into, i.e. the header's true predecessor) right after the init loads in each
engine, and use it as the phi's init predecessor instead of the hardcoded
`%entry`:

```rust
let init_pred = self.fun.cur_block.clone()
    .unwrap_or_else(|| "entry".to_string());
```

Then replace each `[ {init}, %entry ]` with `[ {init}, %{init_pred} ]` in that
engine's phis.

- No structural IR change: match blocks stay where they are; loads/`br` are
  already in `init_pred`; only the phi label string changes.
- When no match runs, `cur_block` is `None` → `init_pred = "entry"` → identical
  IR to today (zero regression risk for the 16 building benchmarks).
- Additive only: no existing optimization path modified.

### Per-engine notes

- **Countdown** (the active bug): capture after line 1000 (after the init-load
  loop). Fix phis at 1039/1041/1059. The init loads run in the real `out`
  (before `cd_buf` is opened), so `cur_block` at capture time is the match-end
  block.
- **PerFieldPhi**: init loads run inside `loop_buf` (buffered). Capture
  `init_pred` inside the buffer scope, after the init-load loop, before the
  header phis. Fix phis at 371/374/402.
- **version-DAG**: init loads at 1498/1508 run in the real `out` (before
  `vd_loop` buffer). Capture after 1510. Fix phis at 1553/1584.
- **folded**: init loads at 212+. Capture after the init-load loop. Fix phis
  at 136/144.
- **SSA**: `emit_ssa_canonical_loop_setup` (ssa.rs:211) — the phi at line 225
  cites `%.ss_loop`, not `%entry`. The reported symptom was a *dominance*
  error, not a phi-citation error. Investigate the exact IR before fixing;
  the fix may be to move the init loads into the preheader block or to
  adjust the phi citation to the actual predecessor.

## Verification

1. `cargo build --release --bin brievc`.
2. `hash_ops_idio` builds (clang) + output matches C at `BOUND=5000000`.
3. `tests/tier1/test_hashmap_surface.bv` compiles to valid IR (SSA engine).
4. `cargo test --lib` — 2076 green (no regression).
5. Interleaved A/B on the 16 building benchmarks — confirm timing-neutral
   (the fix only changes a label string when a match is present; when absent,
   `init_pred == "entry"` and the IR is byte-identical).
6. Record in `benchmarks/results/`, update BUGS.md, commit.

## Documentation

- BUGS.md: new entry for the init-block phi predecessor bug (root cause,
  trigger, fix).
- This plan: mark complete with results table.
- No SPEC change (no syntax change).
- No architecture doc change (no structural change — the `cur_block`
  machinery already existed; the engines just didn't use it for the loop
  header phi).

## Results (2026-09-07, commit c4ee2e19)

- `hash_ops_idio` builds (clang -O3 -flto), output matches C at
  `BOUND=5000000` (`24999995000000`).
- `tests/tier1/test_hashmap_surface.bv`: phi + label now valid (was
  double-written phi + missing label). Remaining clang rejection is the
  PRE-EXISTING `%t765` dominance bug (guarded `HashMap.insert` alloc not
  dominating a nested-foreach read) — documented in BUGS.md, deferred to a
  structural slice.
- `async-events-compiled.bv` prints 17, `async-ready-gate.bv` prints 111
  (the cross-function `cur_block` leak that made it cite `%guard.end106` is
  fixed by the `cur_block = None` resets after `emit_inline_init_stores` in
  `emit_main`, `emit_countable_loop_wrapped`, `emit_countable_batched_main`,
  `emit_version_dag_main`, `emit_ssa_main`).
- `cargo test --lib`: 2076 green.
- Praetor: zero new diagnostics in changed files (12 = 12 complexity issues
  before/after, all pre-existing in untouched functions).
- A/B timing-neutral: when no block-emitting init runs, `init_pred == "entry"`
  and the emitted IR is byte-identical to the prior build.
