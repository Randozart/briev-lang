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

## Triage status (2026-10-17)

**Root cause found before triage: the 10-17 morning "C-wins" were mostly
one regression.** `series_converge` MISMATCH bisected (deterministic
IR-gate) to `eae6ab66` (2026-10-06): the native-prelude import of
std/iterator.bv put a dead callable txn in the transition graph, and the
backend dispatch gates counted raw entries — every program with a single
foldable reactive node fell off the per-field-phi fold onto the direct-SSA
loop. Fixed reactive-only in `821d8155`
(docs/plans/2026-10-17-series-converge-watchdog-dispatch.md).

**Post-fix full runtime suite**
(`benchmarks/results/2026-10-17-post-stability-runtime-fixed.md`): all
rows MATCH/PASS, zero BUILD-FAIL/MISMATCH/SKIP. Rows that recovered vs
the 10-17 morning run (same commit, pre-fix):

| bench | morning | fixed |
|---|---|---|
| float_math | 2.74x | 0.90x Briev |
| fannkuch_redux | 4.47x | 1.11x |
| kalman_filter_runtime | 1.85x | 1.08x |
| ring_buffer | 1.20x | 0.96x Briev |
| cancel_math | 1.39x | 0.88x Briev |
| nbody_newton | 1.09x | 0.97x Briev |
| series_converge | MISMATCH | MATCH 1.00x |

**Triage COMPLETE (2026-10-17, report:
`docs/2026-10-17-dispatch-regression-report.md` §7).** Baseline
repointed to `13ee67fa` (last pre-regression), release built, all
candidates A/B'd 5×5 interleaved. Every "worsened" row and two of three
"confirmed regressions" were **stale-baseline artifacts**:
`compare_baseline.sh` only rebuilt the baseline binary when missing, so
queue_drain (Aug 16 binary), float_math_nonzero and interval_step
(Sep 7 binaries) were compared against compilers OLDER than the baseline
commit — fresh builds read 0.98–1.04x across the board. The script now
always rebuilds both binaries. Remaining C-wins (mandelbrot 1.20x,
knucleotide 1.22x, fannkuch 1.11x, kalman 1.08x, sweep family,
float_math_nonzero 1.24x) are at their pre-regression levels —
historical gaps, not drift. The one live signal:
**nbody_newton_accel 1.10x** over pre-regression (on top of the known
~12x vs C) — handoff to the accel-offload plan.

**Harness fixes landed with the sweep:** `build_c` cross-ref is not a
build failure (queue_drain_idio row restored); dead `UTF8_ops` entry
retired (source archived in `80c89d4c`, adjudicated 09-30 §5); optimizer
suite 4/4 precomputed MATCH
(`benchmarks/results/2026-10-17-post-stability-optimizer.md`).
