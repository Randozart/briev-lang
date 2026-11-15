# 2026-11-15 — What's Next (state audit + plan)

Written in PLAN mode after the Float div/0 + counsel seed session. The branch
has moved since that session (two merges, one new fix commit, two new BUGS
entries). This document audits the current state and sets the next work queue.

## 1. Current state (as of 2026-11-15)

### Git
- Tip: `817f1d698` — Merge main contract refactor into compiler assurance
  (parents: `8278566cb` + `8f9ebf394`, my counsel commit).
- Since my session: `74311bce0` (fix(llvm): preserve integer wrapping under
  optimization) and two merges. Working tree clean except untracked
  `fuzz/Cargo.lock`.
- My float div/0 fix (`3a2fea32f`) survived the merge intact and was
  **extended**: `74311bce0` + BUGS 2026-11-15 "floating division guard compared
  incompatible SSA types" moved the guard to use the mixed-operand adapter
  before checking the promoted divisor's Float category. Two div-zero guards
  now exist: the top-level dispatch (~line 2182) and the intrinsic/template
  path (~line 11573).
- My counsel seed (`8f9ebf394`) merged cleanly.

### CI
- Tip `817f1d698`: CI **in progress** (started 2026-11-15T07:02:53Z). No
  completed run yet for the tip.
- The two "failure" rows in the CI list are on `3a2fea32f` and `8f9ebf394` —
  both **superseded** by `74311bce0` (the wrapping fix) and the merges. They are
  historical, not current. The Fuzz workflow passed on both.
- Net: CI is green-or-pending; the last completed CI success is `89dbaa687`.

### Open PR
- **#100** `feat/counsel-seed-v3`: "4 advisory contracts (size, node_count,
  count_stat, canonical_rid)". All honest, zero Violated. Documents the
  counsel 0.5.0 pure-integer subset limits (✅ i64/u64 arithmetic,
  comparisons, if/else, `for lo..hi` loops, field access; ❌ `as u64` casts,
  `while let`, `HashMap::get`, `Vec::len` not provably non-neg). Next per its
  description: "expand to provable contracts (pure arithmetic, no casts, no
  method calls)".
- Related branches exist: `feat/counsel-contracts`, `feat/counsel-expand`,
  `feat/counsel-lab`, `feat/counsel-seed`, `feat/counsel-seed-v2`,
  `feat/counsel-seed-v3`.

### BUGS.md new entries (2026-11-15)
- **Floating division guard compared incompatible SSA types** — fixed in
  `74311bce0` (the guard now uses the numeric mixed-operand adapter first).
- **Float64 printing lacked its C declaration and lost buffered output** —
  fixed (printer declaration phase includes the double ABI; C formatter
  flushes; exact `0.5\n` O3 regression).
- **Native arithmetic no-wrap flags conflict with interpreter wrapping** —
  repaired in `74311bce0` (flag-free add/sub/mul, flag-free negation,
  O0/O3 AST/HIR regression in the quick gate).
- **SAT models and solver errors misclassified as UNSAT** — fixed (shared
  exact-leading-status parser; `solver_response` unit tests + real-Z3
  `solver_boundary` integration tests).
- **symbolic arithmetic panics on valid wrapping integer expressions** —
  fixed (wrap add/sub/mul explicitly, `checked_div`).
- **`--verify-bodies` relational-over-sum** — **RETRACTED, not a defect**:
  bitvector wraparound is the correct refusal. Lesson: contracts must bound
  values they reason about; unbounded arithmetic postconditions are correctly
  unprovable. Do not re-investigate.

## 2. What is genuinely next

### A. Wait for tip CI (blocking gate)
The tip `817f1d698` CI is in progress. It is the first full run covering the
wrapping fix + both merges + my counsel seed. If it fails, that is the
immediate priority. If it passes, the branch is stable and the queue below
proceeds.

**Action:** check the run; if green, move to B. If red, read the failing
step and repair (likely candidates: the wrapping regression, the Float64
printer, or the div-zero guard interaction).

### B. Land or close PR #100 (counsel seed v3)
#100 is open, honest, and documents the subset limits. Decision:
- **Merge it** if the 4 advisory contracts are acceptable as a seed (they are
  marked "not checked" / "unproven (advisory)" — zero Violated, so the
  non-blocking counsel gate stays happy). This completes the "seed honest
  contracts" phase.
- **Or keep it open** if the team wants to wait until the CI counsel step is
  made per-file-scanning (task C below) so the contracts are actually
  exercised in CI, not just locally.

The PR's own "Next" line says "expand to provable contracts" — that is the
follow-up to D.

### C. Make the counsel-check CI step functional (unblocks task 4)
The step in `ci.yml:110` runs `counsel-check --format human src/` and currently
exits 2 on the whole tree (refused at `src/accel_rt.rs` for an unexpanded
attribute macro). It is non-blocking (`continue-on-error: true`), so the
exit-2 is swallowed and the step contributes nothing. To make it a real gate:

1. **Per-file scanning.** Change the CI step (and `scripts/counsel-check.sh`)
   to iterate `.rs` files under `src/`, run counsel-check per file, and
   collect verdicts. Refused files are logged as skipped, not as failures.
   The step fails only on a `violated` verdict.
2. **Or fix the refusal.** Find why `accel_rt.rs` is refused (likely a
   `#[cfg_attr]` or unexpanded proc-macro attribute) and either rewrite that
   file or add an allowlist so the whole-tree scan works. The per-file route
   is lower-risk and matches counsel's file-independent design.
3. **Re-run and read the census.** With per-file scanning, the step produces a
   real `proven/unproven/violated/skipped` count. That census is the
   "proving phase" the plan requires before flipping the gate.
4. **Then remove `continue-on-error: true`** (task 4) — only after the census
   shows 0 violated and the step is stable.

### D. Expand to provable contracts (the real "proving phase")
Per #100's next-step and the counsel subset limits, the next seed batch should
target functions in the pure-integer subset: i64/u64 arithmetic, comparisons,
if/else, `for lo..hi` loops, field access — no casts, no `HashMap`, no
`Vec::len`. Candidates to survey (read before picking):
- `src/hir/stats.rs` — `UnsupportedCounts` already has a proven `total`;
  sibling counters may be provable.
- `src/backend/llvm/context.rs` — has existing `use counsel::{post, pre}`;
  check which small pure helpers are in the subset.
- `src/ffi/error.rs` — `ErrorConventions::is_null` is proven; sibling null
  checks may be.
- `src/backend/bad/registry.rs` — check for pure-integer helpers.
- `src/analysis/region.rs` — has `use counsel::pre`; check small helpers.

For each candidate: confirm the body is in the subset (no casts, no method
calls on unknown types, no while-let), write a `#[pre]`/`#[post]` that is
actually provable (not just honest), and verify with counsel-check locally
(Z3 may be needed for some; local Z3 is absent, so CI is the arbiter).

### E. Verify the div-zero guard covers all emission paths
The 2026-11-15 BUGS entry shows the guard was moved and extended. Two guards
exist (top-level dispatch ~2182, intrinsic/template ~11573). Verify:
- Literal denominators on either side (`7 / denom`, `1.0 / arg_float("--d")`).
- Zero/non-zero behavior on both AST and HIR paths (the regression already
  covers this per BUGS, but confirm it's in the committed test suite).
- Float64 zero/negative-zero traps (BUGS says the native regression checks
  both emission paths — confirm the test exists).

### F. Housekeeping
- `fuzz/Cargo.lock` is untracked. Decide: commit it (if the fuzz workspace is
  intended to be lockfile-pinned) or add to `.gitignore`. Low priority.
- The `trismegistus snapshot turn N` commits in the graph are session
  checkpoints, not work — ignore them.

## 3. Recommended order

1. **A** — tip CI green? (gate everything else)
2. **B** — merge #100 (completes the seed phase)
3. **C** — per-file counsel CI step (unblocks task 4)
4. **D** — next provable-contract seed batch
5. **E** — div-zero guard coverage check
6. **F** — housekeeping

Task 4 (remove `continue-on-error`) is **still blocked** until C + D produce
a real census with 0 violated. It is the final step of the counsel workstream,
not a standalone action.

## 4. What is NOT next
- Do not re-investigate the `--verify-bodies` relational-over-sum "bug" —
  it was retracted as correct bitvector wraparound behavior.
- Do not try to prove unbounded arithmetic postconditions — bound the values
  or accept the refusal.
- Do not touch the live foreign lanes or the GPU branch work.
