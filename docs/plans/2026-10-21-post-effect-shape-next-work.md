# Post-effect-shape work plan — provenance, small sums, inlining, vectorization

**Date:** 2026-10-21
**Status:** ACTIVE — approved execution plan.
**Baseline lock:** `compiler-baseline-2026-10-21` at `98f7fc42`
(`../briev-compiler-baseline`).
**Current main:** effect-shape selector + CI/provenance fixes; full runtime
suite MATCH; library 3,284 green.
**Abandoned reference:** `abandoned/canonical-effect-abi-2026-10-21` — do not
merge; see `docs/2026-10-21-canonical-effect-abi-negative-result.md`.
**Backlog:** `docs/plans/2026-10-20-open-work-backlog.md` (2026-10-21 addendum).

## Scientific method

Every slice follows the house loop:

1. State the hypothesis and the observable it predicts.
2. Measure a baseline at the locked compiler baseline and current `main`.
3. Make one structural change.
4. Verify the predicted IR/asm and benchmark effect.
5. If refuted, record the refutation and revert the experiment.
6. Keep the suite/gates green; commit per logical step.

No benchmark names, field prefixes, or algorithm vocabulary may enter compiler
logic. No `#inline`/`#unroll` requirement for speed. Every performance result
gets a same-commit correctness gate.

## Phase 0 — Baseline and inventory

### Work

- Confirm `main` clean and pushed.
- Run and record:
  - `cargo test --lib`
  - `tests/daily_use_smoke`
  - `bash benchmarks/effect_shape_guard.sh --quick`
  - `bash benchmarks/effect_shape_guard.sh --timing`
  - full `bash benchmarks/build_and_bench.sh --runtime`
  - `bash benchmarks/ll_determinism_check.sh --quick`
- Inventory pointer conversion sites and enum construction sites:
  - `canonical_ptr`, `state_ptr_cache`, `volatile_raw_ptr_params`
  - `emit_enum_construct` / match tag-payload loads
- Record the inventory in this plan's experiment log.

### Result — baseline and inventory (2026-10-21)

- `cargo test --lib`: 3,282 green on `main`/fence branch.
- Effect-shape quick/timing guard: PASS.
- Latest full runtime run after PR merges and pointer fixes: all MATCH
  (`/tmp/opencode/full_after_otto.log`); selected ratios: float_math 1.03x,
  float_math_nonzero 0.97x, kalman 0.86x, sweep_sparse 0.97x, queue 0.51x,
  interval 0.71x, ws_frame 1.52x.
- Pointer inventory: ~482 `inttoptr`/`ptrtoint`/canonical-pointer sites across
  the LLVM backend; the hot cross-defn path is covered by the Ptr-arg fence.
- Enum inventory: `emit_enum_construct` always mallocs a 16-byte
  `{tag,payload}`; FSM/SPSC compile through `Result` constructors and match.
  First P2-1 slice will target `InlineLocal`, then internal `ReturnedSmall`
  (`sret`) values.

### Acceptance

Baseline table and inventory committed. No code change.

## Phase 1 — P1-4/P1-6: provenance coverage and fence retirement

### Hypothesis

The narrow volatile fence for raw accesses inside Ptr-parameter defns is no
longer required now that Ptr arguments marshal through `canonical_ptr`, txn/main
Ptr state is preloaded at entry, and the direct-SSA replay shares the canonical
pointer.

### Experiment 1A — fence removal probe (do first)

- Branch from `main`: `feat/ptr-provenance-fence-retirement`.
- Remove the `volatile_raw_ptr_params` behavior but keep the flag for easy
  re-enable.
- Run:
  - `cargo test --test daily_use_smoke ptr_crossdefn_probe_golden_output`
  - `cargo test --lib`
  - full runtime suite (no perf regression)
  - CI LLVM-18 job (the original failure environment)
- If green: keep removal, add a regression note, retire the flag.
- If red: restore the fence and continue with 1B.

### Experiment 1B — interprocedural provenance (only if 1A fails)

Inventory every `i64 ↔ ptr` conversion:

- Ptr state-field loads/stores
- Ptr defn parameters/returns
- `Load#` / `Store#` / `Copy#` / `Set#`
- String/List carriers and runtime helpers
- local/block values and mixed access paths

Design rules:

- one canonical pointer per provenance root (state field, argument, local);
- cache only where the defining SSA name dominates the use;
- preload at function/txn/main entry when a field is live across calls;
- marshal calls through the same canonical pointer;
- never cite a non-dominating block-local register.

Add P1-6 mixed-path tests:

- raw index/deref vs `Load#`/`Store#`
- Ptr state field passed to sibling defns
- Ptr local + state mixed
- String/List carrier boundary

### Result — Experiment 1A refuted (2026-10-21)

Branch `feat/ptr-provenance-fence-retirement`, commit `c626c2dc`: fence
disabled, local LLVM 23 and full lib green, but LLVM-18 CI failed
`ptr_crossdefn_probe` with `0` instead of `7`. Fence restored in
`f2a580cc`; draft PR #33 closed without merge.

Conclusion: canonical Ptr call marshaling + entry preloading are insufficient
on LLVM 18. Interprocedural provenance (1B) is still required; it is a larger
design effort and is **deferred behind P2-1** so the correctness fence stays in
place while performance work proceeds.

### Acceptance

- `ptr_crossdefn_probe` passes under LLVM 18.
- Fence removed, or a documented invariant explains why it must stay.
- Full suite MATCH; no measured benchmark regression.
- New pins for every fixed mixed path.

### Rollback

Branch-local. If 1B fails, restore the fence; the CI-correct main behavior is
the fallback.

## Phase 2 — P2-1: small-sum unboxing

### Evidence

FSM 2.86x and SPSC 1.99x are dominated by heap-boxed `Result`/enum values:
`emit_enum_construct` mallocs a 16-byte `{tag, payload}` image; match reloads
tag/payload from that image.

### Representation plan

Derive `EnumRepresentationPlan` from use/escape analysis:

| Class | Representation | When |
|---|---|---|
| `InlineLocal` | SSA/stack `{tag, payload}`, no heap | value consumed locally; no escape |
| `ReturnedSmall` | hidden caller `sret` slot | internal defn returns enum; caller owns slot |
| `Boxed` | existing 16-byte heap image | state/global/collection/export/external/unknown escape |

### Slices

1. **Inventory:** instrument enum construction/use categories; record
   FSM/SPSC/Arena shapes. No codegen change.
2. **InlineLocal:** construct tag/payload into a local slot or SSA; match reads
   the same representation. Measure FSM/SPSC.
3. **ReturnedSmall:** hidden `ptr` out-slot for internal enum-returning defns;
   caller allocates and matches. Keep external/exported ABI boxed.
4. **Match/destructuring:** support `Result`, `is_ok`/`is_err`, `unwrap`,
   tuple payloads, generic payload substitution.
5. **Escape fallback:** any value stored in `%State`, collection, passed to an
   external/dynamic surface, or whose lifetime is unknown stays boxed.

### Acceptance

- FSM transition loop has no per-step `malloc`/tag-payload heap loads.
- SPSC pop has no per-pop boxed `Result`.
- IR/asm gate: allocation count in hot loop drops as predicted.
- Full suite/goldens/parity MATCH; no regression in current winners.
- No per-type special cases, no source workaround, no new keyword.

### Risks

- Interprocedural `sret` shape and generic `Result<T,E>` substitution.
- Pointer lifetime across loops/async.
- Escape analysis must be conservative; boxed fallback is mandatory.

### Rollback

Slice-local. If InlineLocal does not improve FSM/SPSC, record the refutation and
do not proceed to sret until the cause is understood.

## Phase 3 — P2-2: small-function inlining and field scalar replacement

### Work

- Reuse `src/analysis/inline_cost.rs`; compare inline decisions against actual
  linked IR at hot call sites.
- Add cost-model inlining for small defns without requiring `#inline`.
- Scalar-replace non-escaping struct/obj fields in loops.
- Feed contract facts (range, alignment, immutability) into the cost model.
- Diagnose Arena bump and ws_frame codec call overhead.

### Acceptance

- Arena and ws_frame hot loops collapse to direct bump/load paths where the
  language permits.
- IR deltas recorded; full suite MATCH; no benchmark regression.
- No shape-specific matcher or keyword.

## Phase 4 — P2-4: remaining vectorization gaps

Only after P2-1/P2-2; they may remove the remaining losses.

### Work

- Run `-Rpass`/`-Rpass-missed` on the actual `-O3 -flto` linked build (not raw
  `.ll`).
- Classify remaining gaps: alias, alignment, trip count, reduction, loop shape,
  call/effect boundary.
- Feed facts from contracts/analyses; fix general loop shapes.
- Update the effect-shape baseline table; remove stale P2-4 entries.

### Acceptance

- Each retained gap has a replayable diagnosis and measured effect.
- No vocabulary matching; no strategy keyword as the fix.

## Gates for every slice

- `cargo test --lib`
- `cargo test --lib conformance_sweep`
- `tests/daily_use_smoke`
- `bash benchmarks/effect_shape_guard.sh --quick` (+ `--timing` for shape work)
- full runtime suite for codegen/default changes
- `bash benchmarks/ll_determinism_check.sh --quick` for backend work
- CLI/parity gates as applicable
- Praetor on changed directories
- BUGS.md entry for every new compiler gap
- Plan/results update with measured numbers
- Commit per logical step; checkpoint branch for failed experiments

## Commit log

| Date | Commit | Change |
|---|---|---|
| 2026-10-21 | `dca482b0` | abandoned canonical effect ABI + next queue |
| `eb297ee8` | Phase 0 baseline/inventory recorded |
| `c626c2dc` | Phase 1 fence-removal probe (refuted; fence restored `f2a580cc`) |
| deferred | Phase 1B interprocedural provenance |
| _pending_ | Phase 2 P2-1 enum representation inventory |
| _pending_ | Phase 2 P2-1 InlineLocal |
| _pending_ | Phase 2 P2-1 ReturnedSmall |
| _pending_ | Phase 3 P2-2 |
| _pending_ | Phase 4 P2-4 |
