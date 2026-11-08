# General workstream: countable-body statement drops + queued fold work

**2026-11-08.** Status: ACTIVE.
**Provenance:** the §12 idiom workstream (`2026-11-06-idiom-recognition-doctrine.md`,
W3/W3.1) surfaced a host-side backend bug while wiring the `g0_minmax` XPU
gate. This plan owns the **general compiler/language** follow-ups. The XPU
roadmap (G1.5 Level Zero / OpenCL IL projection) is explicitly out of scope
here and stays in `docs/architecture/intel-dispatch.md`.

## P0 — `emit_countable_body` silently drops nested `Foreach` (correctness)

**Status:** LANDED 2026-11-08 — Foreach arm + exhaustive audit
(`loop_engine/counter.rs`), regression test
(`tests/countable_body_foreach.rs`), `g0_minmax` required + PASS on PTL
(`bad=0 MIN=ok MAX=ok`), BUGS.md updated.

**Evidence (2026-11-06, `/tmp/opencode/mm_allplain.ll`):** a plain counted
node `maxrow { let acc = a[j*K]; foreach k in 1..K { acc = Max#(acc, a[j*K+k]) }; ymax[j] = acc; j = j+1 }`,
folded into `@txn_maxrow`, emitted `ymax[j] = a[j*K]` — the inner `foreach`
**vanished**. The sibling `minrow` (same shape, alwaysinline path) kept its
loop. The walker
`src/backend/llvm/loop_engine/counter.rs:1974 emit_countable_body` has no
`Statement::Foreach` arm; its `_ => {}` catch-all silently drops it. The
file's own comments record the same bug class twice before (free/keep hints,
statement-match, arrow-assign) — each fixed by delegating to
`emit_stmt::emit_statement`.

**Impact:** silent wrong results for ANY folded/counted node containing a
nested loop, on every backend (CPU included). Exposed by the W3.1 gate, not
caused by it.

**Work items:**
1. Add a `Statement::Foreach` arm delegating to
   `super::emit_stmt::emit_statement` (provenance comment + "To undo").
2. Exhaustiveness audit: enumerate every `Statement` variant against the
   walker's arms; executable statements must have an explicit arm or a
   documented deliberate decision — no silent fallthrough.
3. Nested-foreach **field writes**: the walker's field-phi bookkeeping
   (`pending_phi_backedge` / `last_val_temps`) does not see writes made
   inside a nested loop. First pass: local-accumulator folds must be
   correct (the gate shape); a field write inside a nested foreach must
   either work or produce an actionable diagnostic. Test both.
4. Regression tests: IR test pinning that the folded node contains the
   nested loop and its ops (`llvm.smin/smax` in the max folD); re-enable
   `g0_minmax` as a **required** Vulkan rung in `benchmarks/intel_ptl/g0.sh`.
5. Full suite + PTL gate; BUGS.md entry updated with the IR excerpt.

**Acceptance:** `g0_minmax` PASS on Vulkan (bad=0, exact Int extrema);
`cargo test --lib` green; no silent `_ => {}` for executable statements in
the walker.

## P1 — Float folds behind a numeric contract (queued)

Gate 3 currently hard-blocks float fold reassociation
(`FoldIdiomBlock::FloatUnlicensed`). Design first: which declared surface
licenses float reassociation (a fold `assoc` declaration, a numeric
tolerance contract, or an ambiguity keyword per `keyword-taxonomy.md`).
Then extend the outcome/synthesis (FAdd/FMul/FMin/FMax group ops) and
consume the license per Rule 25. Needs its own plan addendum before code.

## P2 — W5 idiom candidates (queued)

Scan (prefix/inclusive), transpose-in-GEMM (register-shuffle load path),
map-chain fusion. Each requires its own plan instance + §8 ledger row
(`tests/idiom_ledger_gate.rs` enforces the row).

## P3 — AGENTS.md §12 pointer (queued)

Rule 23 gains a one-line pointer to `derivation-not-recognition.md` §12.
The branch is merged into main, so the merge-time blocker is gone.

## Verification protocol

Every P0 rung: correctness before timing; same-commit gate evidence in
`benchmarks/results/` when the PTL node is involved; `cargo test --lib`
green; Praetor on changed files (house checklist). BUGS.md entry updated
with the root cause, the fix, and the undo path.
