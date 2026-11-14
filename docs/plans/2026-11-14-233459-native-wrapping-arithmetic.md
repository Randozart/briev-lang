---
schema: "codex-work-plan/v1"
created_at: "2026-11-14T23:34:59+01:00"
timezone: "Europe/Amsterdam"
status: "active"
baseline_commit: "89dbaa687c9fea71d4b688332cf2e1c03bfba23f"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-14-225642-verifier-arithmetic-and-vc-semantics.md"]
---

# native-wrapping-arithmetic

## Purpose

Repair native overflow miscompilation discovered by the compiler-assurance work

## Approach and alternatives

Reproduce on unmodified compiler with maintained tests, remove unsupported no-wrap promises, exercise both HIR and AST at O0/O3 and multiple widths, update instruction-shape tests to check behavior without demanding poison-producing flags

- Emit no-wrap flags only with a per-operation overflow proof; deferred until range information establishes the required premise. Existing unconditional promises are unsound.

## Scope and documentation

Unconditional integer add/sub/mul/neg flags in shared arithmetic templates and general expressions; maintained optimized native regression; preserve proven bounded/address arithmetic

## Verification

- Native regression fails before fix and passes after; compiler library/quick/build; full IR determinism and installed clang matrix for backend changes; Lean gate and production arithmetic Kani; Praetor baseline comparison; source-bound evidence outside repo

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-14T23:34:59+01:00 — Plan created before implementation.

### 2026-11-14T23:39:39+01:00 — Implementation and acceptance boundary

Baseline: 89dbaa687 on feature/local main. The maintained full-executable test
fails before the fix at HIR=0, O3, MAX+1: the monotonicity result is 1 rather
than 0. The targeted fixed regression passes both emission paths and optimization
levels for all seven runtime input pairs. Existing isolated emitted-IR removal
of only `nsw` also restored the correct result (previous phase evidence).

Changes: shared add/sub/mul/neg templates, general expression add/sub/mul and
legacy bindings emit no unconditional no-wrap flags. Generic instruction-shape
assertions retain their opcode/operand obligations but drop the invalid flags;
loop-counter assertions remain unchanged. Add width tests for shared templates
and general-expression fallback. Add the native regression to both quick-gate
implementations. Affected docs: backend-architecture.md, verifier-arithmetic.md,
BUGS.md. No new arithmetic algorithm or dependency; existing production arithmetic
Kani proofs remain separate from the native regression, not proofs of LLVM.

Falsification: if removing the flags does not fix the actual source pipeline at
O3, or ordinary non-overflow arithmetic/dataflow regressions fail, the repair is
insufficient. Specialized loop counters and div/rem are separate open obligations;
no native-program proof or exporter is claimed. Performance is not being traded
for a known wrong answer, and no speed improvement is claimed.

Verification evidence location (private, outside source):
/home/randozart/verification-evidence/briev-lean-assurance/native-wrapping/.
Before commit: root build/quick/library, maintained native test, release build,
full IR determinism, installed-clang matrix, baseline-aware Praetor, formatting,
shell/links/secrets checks, Lean and relevant Kani gates. Freeze inputs before
final checks; preserve the existing standard-runner external-symlink limitation.
Reconcile with local main without staging or discarding its unrelated backend
edits; those files overlap, so use a content-preserving merge rather than a
forced checkout or stash. No remote push.

### Test setup correction before final freeze

The attempted no-universe fallback test did not bypass template lookup: known
numeric operations still use shared templates. Exporting the function also
introduced an ABI wrapper, which was not the arithmetic body being tested.
The retained function-expression test forces ordinary definition liveness without
export wrappers and checks the native 64-bit expression path. Shared-template
checks cover 8/16/32/64-bit spellings. No separate execution proof of the general
fallback or arbitrary target widths is claimed. Existing wasm loop-counter
assertions remain intact. These corrections fix test setup/coverage attribution;
the old compiler's full native overflow failure remains the acceptance regression.

### Existing regression predicate corrected before retry

The first quick run exposed a list-push test that used `add nsw` as an increment
marker. Matching ordinary `add` also counted a zero-initialization store (5 vs
4). The test now recognizes the actual `+ 1` operand, retaining the exact four
pushes and constant-one-seed rejection. No product behavior failure was observed
in this case. Preserve the failed run as evidence and repeat required gates on
the corrected frozen inputs.

The assertion audit also tightened the sync-body and inline-member add markers
to require a register operand (`add i64 %...`). This excludes the backend's
`add i64 0, literal` materialization, preserving the original silent-drop
regressions after removing flags. Restart the source-bound gates after this
last test-only correction.

The increment predicate must also follow materialized constants: actual IR adds
`%old, %one`, where `%one = add i64 0, 1`, rather than a literal RHS 1. The
maintained assertion now recognizes constant-one registers and requires a
register-plus-one definition, still exactly four stores. A direct inspection of
the retained failure IR confirms four such stores before recompilation.
