# series_converge MISMATCH — reactive-only dispatch gates

**2026-10-17.** Root cause found by deterministic IR-property bisect (gate:
`brievc build series_converge.bv --llvm` output contains
`txn 'converge' dispatched via per-field phi`).

## Symptom

`benchmarks/series_converge` prints `""` (C reference: `0.500050008`) —
MISMATCH in every results file since 2026-10-13. Last MATCH:
`2026-09-30-b1-rebaseline.md`.

## Root cause (proven)

`eae6ab66` (2026-10-06, "List + List elaborates to iter_chain") added
`Import$("std/iterator.bv")` to the native prelude. `txn iter_take_loop<T>`
is **callable** (`txn` keyword → `parse_transaction(is_reactive=false)`;
`txn` is always non-reactive — remnant of when nodes were called "rct txn")
and dead (never referenced, not emitted), but:

1. `ReactorTransitionGraph::build` includes every effective txn
   (`is_reactive` flag carried on the node) — by design; frontend consumers
   filter (`build_loop_shapes` skips `!is_reactive`, `program_convergence`
   filters reactive).
2. The backend dispatch gates re-derive counts WITHOUT the reactive filter:
   - `mod.rs` `foldable`: `graph.nodes.len() == 1 && txns.len() == 1` — now 2.
   - `mod.rs` enum legacy single-txn (`enum_ci/enum_ti/enum_tcn`): same.
   - A003 diagnostics loop: prints `info: txn 'iter_take_loop' folded to O(1)`
     for a txn that is never emitted (false claim).
3. Dispatch falls to `emit_ssa_main` (direct SSA loop). The contract
   watchdog `?[(x - last)^2 > eps] -> print_best(x)` is emitted ONLY by the
   counter engine (`loop_engine/counter.rs`, `.cmwd_` labels). The SSA path
   never emits watchdogs → the action call disappears → no output.

Boundary verified (deterministic gate, ×2 each): parent `d6f89727` good,
`eae6ab66` bad. An earlier runtime-output bisect produced contradictory
verdicts (load flakes under `timeout`); the IR-string gate is stable.

## Fix (general — Rule 15/23 compliant)

Dispatch gates count only REACTIVE txns/nodes — callables never affect
reactor dispatch. The frontend already treats the graph this way; the
backend gates now consume the same view.

- `src/backend/llvm/mod.rs` `foldable` gate: count `is_reactive` nodes/txns;
  the body/watchdog source becomes the reactive txn (was `txns[0]`).
- Enum legacy single-txn params: same reactive counts.
- A003 + dead-fields diagnostics loops: skip `!is_reactive` nodes (a dead
  callable's stores are never emitted; "stores eliminated"/"folded to O(1)"
  claims are false for it).
- `multi_foldable` loop: verify each async candidate is reactive
  (an `async txn` callable with a bounded pre could otherwise join the
  multi-txn fold).

Regression test: node + dead generic callable txn (iter_take_loop shape)
must still dispatch per-field phi and emit the watchdog action call.

## Verification (2026-10-17, landed)

- `cargo test --lib` 3154/0 (new: `dead_callable_txn_does_not_break_single_node_fold`).
- Praetor `src/backend/llvm`: no NEW diagnostics vs HEAD (metric drift on
  `generate_with_analysis` cog 1590→1597 / cycl 424→426 accepted; an
  interim +1 on `internal_fold_info` was removed by filtering reactive
  nodes in the iterator instead of the condition).
- Full `ll_determinism_check.sh`: ALL DETERMINISTIC.
- `series_converge`: `dispatched via per-field phi`, output `0.500050008`
  MATCH; QUICK harness row `0.60x Briev MATCH`.
- Same commit: harness `build_c` cross-ref false failure fixed (a
  cross-reference is not a build failure; only a missing cross-ref target
  or a failed clang is). `queue_drain_idio` row restored
  (`0.44x Briev MATCH` — it was recorded `BUILD-FAIL` in the 10-17 suite).
- NOTE: the harness/QUICK runs use `target/release/brievc` — stale
  release binaries mislead (the first post-fix run still said MISMATCH
  until `cargo build --release`). Release rebuilt this commit.

## Known gap (filed, not fixed here)

Watchdogs are emitted only on counter-engine paths. Any program whose
dispatch is not a single foldable reactive node (multi-node, non-foldable)
silently drops the watchdog and its action — a contract-emission gap
independent of this regression. BUGS.md entry; separate fix.

## Verification

- `cargo test --lib` green; no new warnings.
- Praetor `src/backend/llvm` — no NEW diagnostics vs HEAD.
- Full `ll_determinism_check.sh` (backend-touching commit).
- `series_converge` output == `0.500050008` (direct compile + run).
- QUICK harness `series_converge` row MATCH.
