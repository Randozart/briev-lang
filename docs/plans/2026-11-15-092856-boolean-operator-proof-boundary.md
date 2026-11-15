---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T09:28:56+01:00"
timezone: "Europe/Amsterdam"
status: "implemented"
baseline_commit: "9ef2c786e64515b7f834741e025d23e437823732"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-15-080204-typed-verification-and-lean-export.md"]
---

# boolean-operator-proof-boundary

## Purpose

Reject operators borrowed from an unrelated right operand and prevent narrow Boolean values receiving 64-bit arithmetic proofs

## Approach and alternatives

Preserve declared and inherited LHS bindings, remove canonical RHS-only fallback; conservatively exclude declared Boolean storage until its expression semantics have a separate proof model

- Special-case Bool plus integer: rejected because an unrelated right operand cannot supply the left operand's missing operator for any category

## Scope and documentation

Typechecker operator resolution, body model admission, regressions and assurance docs; preserve declared cross-type operator support

## Verification

- Before/after failing frontend and type-model regressions; real CLI rejection of Bool arithmetic; root build/quick/library; package debug/release/Z3; Lean/export controls; four Kani harnesses; baseline-aware Praetor; IR determinism and explicit matrix/snapshot limitations

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T09:28:56+01:00 — Plan created before implementation.

### 2026-11-15T09:36:00+01:00 — Reproduction and implementation

The native probe retained and called `f(b: Bool) -> Bool { term b + 1; }`.
Body verification reported proven, but clang -O0 and -O3 both rejected the i64
sum returned as i8. Logs and failing baseline frontend/model regressions are in
`/home/randozart/verification-evidence/briev-lean-assurance/typed-export/`.
The initial assumption that Boolean storage fit the word model is retracted;
Boolean formulas remain supported, but declared Boolean storage is skipped.

Canonical operators now come from the left receiver. Width-constrained receivers
inherit their own base operators, preserving Int<8> arithmetic; they never borrow
an unrelated right-hand operator. Explicit declared cross-type functions remain
authorized. Frontend rejection and declared-operator tests pass after the fix.
Affected documentation: BUGS.md and verifier-arithmetic.md; the previous plan's
Boolean admission was historical and is narrowed by this dated follow-up.

Before committing, run build/quick/full library and package gates, four Kani
harnesses, Lean models/export controls, changed-scope format/security/link checks,
Praetor compared with 9ef2c786e, and IR determinism/matrix with explicit unavailable
outcomes. Reconcile main, freeze the committed state and repeat final validation.
New evidence is retained at
`/home/randozart/verification-evidence/briev-lean-assurance/boolean-boundary/`.
A nonnumeric receiver gaining an undeclared operator, a lost legitimate inherited
operator, or a declared Boolean receiving a word proof would falsify this fix.

### 2026-11-15T09:42:00+01:00 — Preserve unsigned receivers explicitly

The broader quick gate found two sized UInt<8> silicon regressions after the
right-operand fallback was removed. Canonical UInt bindings were absent, so the
old implementation had relied on an Int numeral to authorize UInt addition.
Add receiver-owned canonical bindings for the shared modular arithmetic and
bit-pattern operations (+, -, *, &, |, ^, left shift, equality). Signed-sensitive
operations are not aliased. Width-constrained types inherit their base's own
binding. Preserve the existing silicon gate tests and add a direct UInt<8>
arithmetic regression; do not weaken the tests or restore unrelated RHS borrowing.
This amendment extends the affected source to type_universe/operators.rs.

### 2026-11-15T10:03:00+01:00 — Pre-commit results

- Root build and quick gate passed: 3800 library tests and 53 integration tests.
  Full `cargo test --lib` passed all 3801 tests, including the active-source
  conformance sweep. The focused typechecker suite passed 205 tests and silicon
  checks passed six tests. The Boolean storage regression failed before the fix
  and passes now; explicit declared cross-type operators remain accepted.
- All seven real CLI probes have the expected result: Int64 identity is proved;
  Float, Int32, wasm32 and Boolean storage are skipped; Boolean and Boolean-alias
  arithmetic fail in the frontend before IR/report emission.
- Package debug/release with real Z3, four Kani harnesses, nine Lean model theorems
  and the checked export/false-contract/evidence controls passed. Rustfmt, Ruff,
  local links, whitespace and changed-source secret checks passed.
- Praetor comparison with 9ef2c786e found no new blocking diagnostics in package,
  analysis, backend, root, examples, checker, typechecker or type-universe scopes.
  The ten fixed rejection cases are expressed as a flat case table; coverage is
  unchanged and the new test introduces no nested-loop diagnostic.
- Release build and all ten IR determinism cases passed. Clang's recommender row
  passes; missing u.data leaves multinode/giantnode rows unavailable and the
  matrix exits 2 on its empty output glob. Standard source snapshots still fail
  on test-project-otto; separate regular-file/literal-link manifests are retained.
- Commit the implementation and this note, reconcile local main, then freeze and
  rerun final commands. Pre-commit logs are historical; final logs and source/tool/
  artifact freshness evidence stay external. No native/source/exporter refinement
  theorem is claimed, and Boolean storage needs its own future model.
