# Post-stability full-suite benchmark sweep

**Date:** 2026-10-16
**Status:** ACTIVE
**Motivation:** The stability remediation (S1–S6, `e5dd108c`…`d7e2b10f`)
plus the cpp-vs-briev session's backend fixes (`93be0795` flaw-4,
`6e3a78ef` node-inline valve=600, `63107709` frame-limit check) changed
codegen-facing paths without benchmark gating. Every LLVM backend change
tends to introduce noise; the standing claim "Briev beats C on every
benchmark" must be re-verified at HEAD (`97bba13f`).

## Baseline

- **A/B baseline worktree:** `../briev-compiler-baseline` @ `b073c68`
  (2026-09-30 rebaseline). Codegen deltas since: flaw-4 pre-declare,
  weighted node-inline decision (valve 600), per-function frame-limit,
  shared body prologue (five drivers, callable-txn predeclare is a fix),
  plus analysis gates (purity probe, lifecycle laws, needs_state scoping).
- **Historical ratios:** `benchmarks/results/` per-benchmark history.

## Protocol

1. **Preflight:** `cargo build --release` clean;
   `bash benchmarks/ll_determinism_check.sh --quick` green; clang version
   recorded (matrix: 23.1.1).
2. **Runtime suite:** `bash benchmarks/build_and_bench.sh --runtime`
   tee'd to `benchmarks/results/2026-10-16-post-stability-runtime.md`.
3. **Optimizer suite:** same, `2026-10-16-post-stability-optimizer.md`.
4. **Triage:** per candidate with worsened ratio —
   `bash benchmarks/compare_baseline.sh <name>` (interleaved 5×5, same
   geometry). Threshold: >10% slower = fail signal; below that needs two
   consecutive A/B confirmations before any action. No "noise" verdict
   without the controlled A/B. Spot-check the reactive-txn family
   (accumulator/async/phase-chain) — the direct S4 subjects — even if
   the table looks clean.
   - Verdict classes: documented-noise (A/B evidence recorded), real
     regression (investigate / Performance Recovery Protocol), intended
     shift (node-inline valve / frame limit — expected, documented).
5. **Record:** findings ledger in each results file; BUGS.md /
   followup-stages entries for real regressions; docs commit.

**Scope:** CPU C-comparison corpus only; GPU lanes out of scope.
