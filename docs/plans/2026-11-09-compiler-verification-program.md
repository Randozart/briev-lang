# Compiler verification program (2026-11-09, umbrella workstream)

Status: ACTIVE. Rides on top of the existing roadmap
(`docs/plans/2026-11-08-post-integration-next-leg.md`) and the typed-HIR
workstream; it does not replace either. Every phase is an independent
increment with its own gates and may interleave with H5 ports, pipeline-HIR,
and A2 typing.

Related plans / doctrine:

- `docs/plans/2026-11-05-hir-praetor-remediation-leg2.md` — gate style and
  exit criteria (gates after each group, docs in the same commit).
- `docs/plans/2026-11-05-h5-retirement-inventory.md` — pass inventory; every
  pass in the retirement plan re-verifies its HIR input.
- `docs/plans/2026-11-05-h4-completion-loop-convergence-swan-song-emitter.md`
  — the HIR/AST byte-identity gate this program extends.
- `docs/plans/2026-11-09-cast-via-typing-scoping.md` and the A1 landing
  (`5357191b`) — `HirExpr.ty` now carries advisory types; P1 consumes them.
- `docs/handoff-methodology.md` — investigate → plan → experiment →
  implement → verify → document.
- AGENTS.md Rules 9 (tests or it doesn't exist), 12 (benchmarks in plans),
  13 (docs in the same commit), 20 (measure first).

## Assessment (what already exists — do not rebuild)

| Area | Existing | Gap this program closes |
|---|---|---|
| Run-pass | `brievc test` + sibling `.golden` (`src/test_runner.rs`); `tests/daily_use_smoke.rs` compiles+links+runs all 54 tier1 fixtures; parity corpus+goldens (`benchmarks/parity/run.sh`) | No RUN directives, inline expectations, per-test options; only 3 tier1 goldens |
| Negative | Ad-hoc (`tests/hold_law.rs`, `tail_expr.rs`, `termination_diagnostics_test.rs`); conformance is parse+check | No compile-fail corpus, no pinned diagnostics |
| LLVM validity | `opt -passes=verify -disable-output` before clang/llc (`src/compile.rs:2827`), surfaced as Briev diagnostics | None — LLVM owns SSA/dominance; do not duplicate |
| HIR verify | Unique DefId/ItemId/NodeId + export metadata (`src/hir/verify.rs`); runs only under `BRIEV_HIR_EMIT` | Typed/structure invariants; always-on in debug; re-run after specialize |
| Differential | AST-vs-HIR byte equality (`/tmp/opencode/hir_sweep.sh`, `benchmarks/hir_emit_ab.sh`); `ll_determinism_check.sh`; spot `-O0` tests | Corpus-wide opt matrix; interpreter-vs-compiled harness; cross-backend gate |
| Fuzz | `tests/fuzz_frontend.rs` (proptest no-panic lexer/parser/typechecker); fault injection; `fuzz_backend.rs` is a dead-backend stub | Well-typed program generator + differential execution; shrink-to-fixture |
| Snapshots | Determinism gate only | Per-pass HIR snapshots; small-fixture LLVM snapshots |
| Reduction | None | cvise/creduce wrapper + bug→fixture workflow |
| Curated | Perf benchmarks vs C; tier1 fixtures | Deterministic algorithm corpus with goldens |

## Locked decisions

- All phases are in scope; implementation interleaves with the roadmap.
- CI: **quick subset per commit; full matrix in a scheduled workflow**.
- Compile-fail pinning: **required substrings** (1–3 per case).
- Runner features are comment directives; **no new language surface**.
- Do not reimplement SSA/dominance verification: `opt -passes=verify` is the
  authority for emitted LLVM.

## Phase ledger

Status: `TODO` / `ACTIVE` / `DONE`. Cost: `per-commit` / `nightly`.

### P1 — Verification hardening — DONE (per-commit, 2026-11-09)

- Extend `src/hir/verify.rs`:
  - `verify_typed(program, universe, graph)`: every present `expr.ty`
    resolves; identifier types agree with the declared binding; every cast
    whose endpoints are known and whose graph path exists carries `via`.
  - Structural: exactly one `hold` marker per fact body; `through` only with a
    `hold` marker; markers only inside `Guarded` bodies; marker value shapes.
- Run `verify`/`verify_typed` on every `build_hir` under `debug_assertions`
  (not only `BRIEV_HIR_EMIT=1`).
- Re-verify immediately after `hir::specialize` at its backend call site.
- `tests/hir_verify_negatives.rs`: one crafted mutation per invariant.
- Acceptance: corpus sweep 0 mismatch; `cargo test --all` green; no IR change
  (`hir_emit_ab.sh --quick` byte-identical).

### P2 — Compile-fail runner + corpus — DONE (per-commit, 2026-11-09)

- `src/test_runner.rs` directives:
  `// RUN: --backend llvm --optimize-budget 0` · `// EXPECT-ERROR: <substr>`
  (all required) · `// EXPECT-EXIT: n` · inline `// EXPECT-STDOUT:`.
  `.golden` behavior unchanged.
- `tests/compile_fail/*.bv` (~30–40: contract, hold misuse, ownership, type
  mismatch, capability gates, retired syntax) + `tests/diagnostics.rs`.
- Acceptance: every diagnostic family covered; runner names the missing
  substring; suite < ~60 s.

### P3 — Differential gates — ACTIVE (opt-level quick DONE; interpreter/wasm/full queued)

- `tests/opt_level_parity.rs`: eligible tier1 fixtures → one `.ll` →
  `-O0`/`-O2`/`-O3` link+run, compare stdout/exit. Explicit skip list with
  reasons; coverage ratchet (may shrink only); `OPT_MATRIX_FULL=1` runs all.
- Interpreter-vs-compiled harness over the same set; interpreter callable-txn
  gaps become a documented skip ratchet (fix or accept explicitly).
  *2026-11-09: PR #80 delivered a generated-transition version of this*
  (`tests/deterministic_transitions.rs`, interpreter vs native `-O0`/`-O3`,
  proptest shrinking + JSON replay). Remaining: run it over the tier1 corpus,
  not only generated families.
- Cross-backend corpus gate (wasm32 vs native) over the eligible subset.
- Acceptance: zero divergence on eligible set; quick mode < ~5 min.

### P4 — Structured program fuzzing — PARTIAL (PR #80 landing, 2026-11-09)

- `tests/fuzz_program_differential.rs` (`#[ignore]`, `FUZZ_ITERS`): generate
  terminating deterministic programs (Int/Bool lets, `when`, bounded
  `foreach`, arithmetic, print), run interpreter vs `-O0`/`-O3`, compare;
  proptest shrinks; tier1 seeds.
- Growth slices: collections → enums/tuples → strings.
- Acceptance: iterations clean per slice; every find reduced into P2 or P7.
  *2026-11-09 (PR #80):* in-process cargo-fuzz targets
  (`fuzz/fuzz_targets/{frontend,arithmetic,interpreter}.rs`), proptest
  transition families 0–12 and memory families 0–4, shrink + replay
  artifacts, nightly `Fuzz`/`Memory` workflows. Remaining: corpus seeds from
  tier1, growth slices, and folding generated families into P3's corpus gate.

### P5 — Pass-level snapshots — TODO (per-commit, small set)

- `hir::render` canonical text; snapshots per representative fixture after
  lower, specialize, and each consumer flip. In-repo `.snap` files,
  `UPDATE_SNAPSHOTS=1` to refresh; deterministic iteration enforced.
- Optional `.ll` snapshots for small fixtures (complements determinism gate).
- Acceptance: two-run byte determinism; update mode reviewed in diffs.

### P6 — Reduction workflow — PARTIAL (PR #80 shrink/replay)

- `scripts/reduce.sh` + `scripts/interesting.sh` (ICE, `-O0`/`-O3`
  divergence, interpreter mismatch); `cvise` preferred, `creduce` fallback.
- `BUGS.md` workflow: every bug gets a reduced fixture committed with the fix.
- Acceptance: one historical bug reduced end-to-end as the worked example.
  *2026-11-09 (PR #80):* proptest shrinking + JSON/`.bv`/stderr replay
  artifacts in both campaigns (the cvise/creduce wrapper from this phase
  remains queued).

### P7 — Curated semantic corpus — TODO (incremental, per-commit)

- 10–15 deterministic bounded tasks with goldens (sort/search, hash map,
  matrix ops, tree traversal, coroutine pipeline).
- Acceptance: every entry runs in the tier1 E2E gate; no overlap with perf
  benchmarks; goldens captured from the reference path.

## CI wiring

- Per commit (fits the 45-minute budget): P1 verification, P2 diagnostics
  suite, P3 quick subset, P5 snapshots.
- New `.github/workflows/verification-nightly.yml` (schedule): full opt
  matrix, interpreter differential, wasm subset, `FUZZ_ITERS` fuzz.
- When P1/P2 land, add their commands to the AGENTS.md per-commit checklist.

## Risks

- Interpreter callable-txn gap limits P3 coverage; decide fix-vs-skip from the
  measured skip list.
- Fuzz generator scope creep; grow only when a slice is clean.
- Snapshot churn; restrict snapshots to pass-sensitive fixtures.
- Flaky float goldens: P3 compares runtime output of the *same* program, not
  stored goldens, so known drift (e.g. `sweep_mid`) does not leak into it.

## Execution record

(append dated notes per phase as they land, including commit hashes and
measured gate runtimes.)

**P1 (2026-11-09, DONE).** `hir::verify` now also enforces law-marker
structure; new `verify_typed` checks identifier/declaration agreement and
requires a resolved `Cast.via` whenever the casting graph has a path (an
empty lane sequence is the identity path and counts as resolved).
`verify_single_callable` re-verifies the substituted tree at the
specialization site in `emit_toplevel.rs`. `build_hir` runs the full check
on every debug build (and in release under `BRIEV_HIR_EMIT=1`); a violation
panics in debug instead of silently emitting worse IR. Cast-path resolution
also now walks contracts, globals, const initializers, and `init` bodies —
they carried casts that were previously never resolved.

Negative coverage: `tests/hir_verify_negatives.rs` (7 tests: duplicate id,
through-without-hold, double hold, stray hold, identifier drift, missing
cast path, clean-pass control). Gates: `cargo test --all` 3611 pass (before
the P1 refactor; 3568 lib + negatives after), corpus sweep 139/0 mismatch
with always-on verification, `hir_emit_ab --quick` byte-identical, Praetor
clean on `src/hir`.

**P2 (2026-11-09, DONE).** `brievc test` now understands comment directives
(`RUN:`, `EXPECT-ERROR:` all-substrings, `EXPECT-STDOUT:` inline,
`EXPECT-EXIT:`); `.golden` behavior is unchanged and mutually exclusive with
an inline stdout pin. New `tests/compile_fail/` (13 cases: law-surface
migration, tautology, missing contract, undefined name, break placement, `if`
keyword, duplicate init, type mismatch, component reference) and
`tests/directives/` (inline stdout proof). `tests/diagnostics.rs` runs both
suites through the real binary. The runner is split into
`check_compile_fail`/`run_program` and Praetor-clean. Runtime ~4 s.

**P3 (opt-level slice, 2026-11-09).** `tests/opt_level_parity.rs` compiles one
`.ll` per fixture and links/runs it at `-O0`, `-O2`, `-O3`, comparing stdout
and exit code against the `-O0` arm. Quick mode (4 fixtures, no LTO) runs in
~20 s; `OPT_MATRIX_FULL=1` enumerates every program-surface tier1 fixture and
uses the harness `-O3 -flto` pipeline for the top arm. Internal deadlines
(build 120 s, link 300 s, run 10 s) keep a non-terminating fixture from
stalling CI; known non-terminating/idle fixtures belong in `SKIP` with a
BUGS.md reason (the list may only shrink). Still queued: full corpus run and
skip inventory, interpreter-vs-compiled, wasm cross-backend, nightly workflow.

**2026-11-09 PR integration note.** #79 (sparse `NodeSet` solver) and #81
(publish-into-incoming reclamation fix) merged together; `site_is_safe` was
adapted to `NodeSet` iteration. #80 (deterministic differential + sanitizer
campaigns + cargo-fuzz) overlapped P3/P4/P6; its harnesses are now the P3
interpreter-vs-native arm and part of P4/P6. The memory workflow's family-1
comment was corrected: the publish leak is fixed by #81 and the lane stays a
non-blocking regression sentinel.

Gates after integration: `cargo test --all` 3620 pass; reclamation 36;
`publish_reclamation` 1; `deterministic_transitions` 5 (2 ignored reproducers);
`memory_campaign` 3 (26 s); release A/B byte-identical; determinism quick OK.
