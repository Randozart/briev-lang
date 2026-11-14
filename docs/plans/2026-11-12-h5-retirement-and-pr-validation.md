# H5 retirement + PR validation (2026-11-12)

Status: ACTIVE. Executes the chosen lane: validate the two open collaborator
PRs, then convert the completed H5 port into actual deletion.

Predecessors: `2026-11-11-compiler-completion-and-verification.md`,
`2026-11-05-h5-retirement-inventory.md`,
`2026-11-04-typed-hir-h3-h5-execution.md`.

## Part 1 — PR validation (first)

### #92 `fix(llvm): nested enum allocation and payload layouts` (+271/−10)
- Touches `src/analysis/enum_layout.rs`, `src/backend/llvm/emit_expr.rs`,
  docs, a fixture and `tests/nested_enum_allocation.rs`.
- Claim: resolve the concrete constructor before sizing/choosing the image;
  inline a nested enum only when the complete payload tree fits the depth
  budget, else retain a handle.
- CI: Default gates FAILURE (branch-side). Validation: merge into a clean
  worktree from current main; run `cargo test --lib enum`, the new
  `nested_enum_allocation` suite, `daily_use_smoke`, quick gate, and the
  corpus sweep; inspect the emitted `.ll`/Valgrind evidence in the PR body.

### #93 `fix(reclamation): stop element reads inheriting container storage` (+241/−3)
- Touches `src/analysis/reclamation.rs` + `walk.rs` **and the new HIR
  reclamation graph/walk**, docs, fixture, `tests/reclamation_runtime.rs`.
- Claim: element reads should not inherit container storage class through
  provenance (a load/collection-element correctness fix).
- Risk: it modifies the freshly flipped HIR reclamation analysis. Validate
  against the plan-parity oracle: run `analysis::reclamation`, `hir::reclamation`,
  `publish_reclamation`, `reclamation_runtime`, memory campaign, quick gate,
  sweep. Any HIR/AST drift is a blocker.
- The PR body reports 10 full-gate failures on an older HEAD, nine attributed
  to pre-existing branches (C boundary/C++ roundtrip, ffi_proof_engine_tests,
  integration_features) and one environmental. Re-run the full gate on current
  main and compare the failing set.

### Merge protocol
1. `git fetch` both PR heads; `git merge-tree` against current `main` and each
   other (they touch disjoint files today).
2. One temp worktree per PR; merge; run the targeted suites above.
3. Merge #93 first if HIR parity is clean (it touches live analysis), then #92
   (backend layout), or as conflicts dictate; re-run quick gate after each.
4. Never weaken a failing test to land a PR; record any pre-existing failure in
   the plan/BUGS with the exact test names.

## Part 2 — H5 retirement

Current debt meters: **`to_ast::` 53 refs**; **`hir_emit_enabled()` 39
branches**; emitter entries still take AST items; `hir_stmt_bodies` /
`hir_sourced_body` still bridge.

### Phase A — Emitter entries take HIR
- `emit_definition`/`emit_transaction` (`emit_toplevel.rs:4923`, `:5954`) take
  `&HirCallable`; delete the body hook: `hir_stmt_bodies` (llvm/mod.rs:1451,
  build at 8701, refill at 2850), `hir_sourced_body` (emit_hir_stmt.rs:30),
  and the `hir_sourced_body` calls in `emit_toplevel`.
- Declaration surfaces on HIR: params/outputs/contracts/exports, global
  initializers, `emit_ssa_main` txn walk, `reordered_items`, export ABI,
  module metadata. Gate: A/B opt-out-vs-default byte-identical, sweep, daily_use.
- Acceptance: no `hir_stmt_bodies`; entries take HIR; `to_ast` count drops.

### Phase B — Remaining consumer analyses
Port with the established parity-then-flip recipe:
1. `destination_ownership` (1.2k; plan consumer in `AnalysisResults`).
2. `destination_model` (2.9k; loop-engine plans).
3. `inline_cost` (293; inline decisions).
Then re-sweep `crate::analysis::` uses in `backend/mod.rs`; `range`/`dataflow`
remain consumerless by decision (no port), `call_graph` is an empty field.
Each: AST oracle, debug-assert flip, named consumer, module tests.

### Phase C — Native-ize remaining reconstruction (drive `to_ast` down)
Locations today: `emit_expr.rs` 22, `emit_hir_stmt.rs` 12, `emit_stmt.rs` 3,
`backend/mod.rs` 3 (+ hir-side boundaries). Convert the dispatch-only families
(Match scrutinee/arms, Slice/Index, StructLiteral/Tuple, MethodCall/Reflect/
Field, short-circuit, plugin intercept) to HIR-taking helpers. Documented
constructor exceptions (collection-witness List arm, `#StdOut` `Print#`) are
kept only if the owner explicitly accepts them; the exit criterion is
`to_ast` = 0 outside those exceptions, or the exception list is ratified.

### Phase D — AST arm deletion / flag retirement
After the opt-out release window:
- Delete AST arms family-by-family (Rule 6 inversion), starting with arms whose
  seam paths are already HIR-native; each deletion runs the full battery.
- Remove `hir_emit_enabled()` branches (39) as their AST alternatives vanish.
- Finally delete `hir::to_ast` and the AST-only declaration shims.
- Keep the sweep/A-B/determinism nets; interpreter stays AST (Rule 5).

### Gates per step
`bash scripts/gate.sh quick`; `cargo test --all` before pushes touching shared
analysis; corpus sweep; A/B opt-out-vs-default; determinism quick; daily_use;
Praetor on changed dirs; docs in the same commit.

## Part 3 — Alternatives (documented, not taken now)

- **Wasm32 `.bv` Ptr physical-width ABI** — unblocks V2. Deferred: architectural
  (TypedRegister carries no LLVM type; Ptr handles are i64 by convention), needs
  a convention decision + inttoptr/ptrtoint audit. Trigger: before the next
  Tier-1 release gate or when a wasm consumer appears.
- **Verification growth** — P7 to 15 algorithms, P4 generator slides, P2
  compile-fail expansion. Deferred as incremental background work; no
  architectural blocker.
- **Stability v0.1** — freeze, nightly perf baseline, preview release. Deferred
  until H5 deletion lands so the freeze happens on the final architecture.
- **Doctrine lanes** — enum-boundary Proof 3 gate removal, memory auto-free M1,
  lifetime substrate phases. Deferred; they are larger independent programs and
  touch runtime semantics, not the HIR retirement path.
- **Intel/GPU ceilings** — separate active lane; not blocked by this plan.

## Estimates
PR validation: 0.5 day (plus any PR-specific fixes). Phase A: 1–2 days.
Phase B: 3–5 days. Phase C: 2–4 days (family by family). Phase D: 1–2 days
after the opt-out release. Docs/ledger reconciliation included in each commit.

## Execution record (2026-11-12)

- Reviewed and merged **#93** (collection element provenance): targeted HIR/AST
  reclamation suites, publish/reclamation runtime, quick gate green; full suite
  matched the pre-existing `c_driver_boundary` failure.
- Reviewed and merged **#92** (nested enum allocation/payload layouts):
  `nested_enum_allocation` 1, enum 80, quick 42/42; same single pre-existing
  full-suite failure. BUGS.md append conflicts resolved during both merges.
- Runtime integration fix included en route: `briev_rt.c` now includes the coll
  request arena, arena entry points are weak (FFI object + runtime include must
  coexist), and owned-`_start` harness links pass `-nostartfiles`
  (`c03c12e6`, `e28919b3`).
- **Known blockers before Phase D:** the HIR/AST reclamation-plan drift on
  `c_driver_boundary` (BUGS.md 2026-11-12) must be fixed while the AST oracle
  still exists. Phase A (entry switch) can proceed; do not delete the oracle
  until this drift is resolved.

## Phase C, statement-emission slice (2026-11-12, done)

Statement emission no longer reconstructs through `hir::to_ast`. The
convert-then-substitute seam (H4.2) is deleted:

- `hir_sourced_body` is now a pair validator (slot alignment only); the body
  loops in `emit_definition`/`emit_transaction`/`emit_callable_txn` walk the
  (HIR, source AST) pair directly — pure inserts (`ReleaseOwned`) flush
  natively before each source slot; the paired statement emits natively and
  splices the SOURCE statement when its native arm declines (`Other`,
  uncovered expression leaves). No AST substitution, no drift assert, no
  `storage_release_view`.
- `convert_hir_statement`, `convert_children_seam`/`convert_scoped_seam`,
  `reconstruct_match_arms`, `release_ast_stmt` deleted. Nested scopes, match
  arm bodies, foreach bodies, and defer flushes go through one shared
  `emit_children_pair` (block/serial arms thread the last register; guard/
  match/foreach arms must NOT allocate one — register parity with their AST
  arms was the A/B failure mode found and fixed en route).
- New native arms: `Open`, `TrgBinding`, `MetadataAssignment`, `InlineAsm`
  (the AST arm emits only a void register; mirrored directly).
- Callable txns now route through the seam too (they previously emitted the
  substituted body purely via the AST path).
- Defer bodies carry the SOURCE AST body alongside the HIR lowering
  (`DeferCleanupBody`) so the flush splices instead of reconstructing.
- `to_ast` in `emit_hir_stmt.rs`: 22 → 9 lines (7 code refs, all
  expression-level `to_ast::expr` gates kept per the Phase C boundary —
  expression families are the remaining slice).
- Byte-identity: `hir_emit_ab.sh` full corpus ALL BYTE-IDENTICAL; determinism
  quick ALL DETERMINISTIC; quick gate 3788 lib + 42 integration green;
  tier1 corpus 52/54 (same two pre-existing `strexp`/`strexp2` link failures
  as baseline HEAD); Praetor at-or-below baseline on every changed file.

## Execution record (2026-11-12, Phase A+C)

**Phase A — emitter entries switch (DONE, `e7ea2e04`).**
`hir_stmt_bodies` HashMap eliminated entirely. `emit_definition`/`emit_transaction`
receive HIR bodies via per-call spec structs (`DefinitionEmitSpec`/`TransactionEmitSpec`).
Specialization passes HIR body directly. ISR synthetic body passes None.

**Phase C (statement-level) — DONE (`37c1a2b2`).**
`hir_sourced_body` is now a pair validator (slot alignment only). Deleted:
`convert_hir_statement`, `convert_children_seam`, `convert_scoped_seam`,
`reconstruct_match_arms`, `release_ast_stmt`, `storage_release_view`/`strip_view`.
One shared pair walk (`emit_children_pair`) for nested scopes, match arms, foreach
bodies, defer flush. New native arms for Open/TrgBinding/MetadataAssignment/InlineAsm.
`to_ast` in `emit_hir_stmt.rs`: **22 → 8** (expression-level gates only).

**Phase B — destination_ownership + destination_model ports (DONE).**
`hir::destination_ownership` (13 parity) + `hir::destination_model` (33 parity)
with HIR fidelity (`HirTypeDef.parent`, `declaration_order`, `HirForeign.link_symbol`,
`ItemCtx` threading).

**Phase C (expression-level) — REMAINING (48 to_ast refs, was 56).**
Breakdown:
- `emit_expr.rs` (22): structural dispatch reconstructs AST for Field/Reflect/
  MethodCall/Match/Slice/StructLiteral to reuse AST-shaped helpers. Each needs
  a HIR-native version. Multi-session.
- `emit_hir_stmt.rs` (8): expression-level `to_ast::expr` gates for Assign/
  Guarded/Term/Match scrutinee. Same category.
- `accel.rs` (6) + `batch_shape.rs` (4) + `region.rs` (1) + `electronics.rs` (1):
  result-boundary crossings, ratified as accepted exceptions.
- `emit_stmt.rs` (3) + `backend/mod.rs` (3): declaration/ABI surfaces, deferred
  to Phase D.

These 30 emission-path refs (emit_expr + emit_hir_stmt) require HIR-native
dispatch for the remaining expression families. Each family (Field, MethodCall,
Reflect, Match, Slice, StructLiteral) needs its own HIR-native emitter mirroring
the AST-shaped helper. This is the last major Phase C slice.

## Phase C, expression-emission slice (2026-11-13, partial)

`emit_expr.rs` no longer reconstructs AST for the structural expression
families. Converted to HIR-native dispatch:

- **Match**: scrutinee, patterns (condition + payload binding), guards and
  arm bodies emit from HIR. `MatchArms::Hir` no longer carries an AST arm
  slice; the probe binds/decomposes HIR patterns directly. Expression-position
  matches used as their own statement keep the AST statement route (`.smt_*`)
  — `hir_expr_family_covered` deliberately excludes Match so the statement
  seam still reaches that path; the operand path opts Match in explicitly.
- **Slice**: the shared helper takes an `ExprView` (AST/HIR) for the array and
  bounds; String/vector shape probes read the HIR identifier and bounds emit
  through `emit_hir_operand`.
- **StructLiteral**: image allocation and field-store tail are shared; HIR
  field values emit through the operand path. `open_struct_image` /
  `close_struct_image` / `StructSlot` keep both paths on one implementation.
- **Tuple**: `TupleElems::Hir` carries only HIR items; constant-element
  placement now mirrors the AST predicate (`is_hir_compile_time_const`).
- **Field**: the full field dispatch (pooled-instance column, deref receiver,
  tuple element, layout read, array field) moved into one
  `emit_field_expression` shared by AST and HIR; receiver shape probes use
  `ExprView`.
- **Spawn**: the redundant AST conversion in the HIR argument path is gone
  (`emit_hir_operand` already converts per leaf).

`to_ast` in `emit_expr.rs`: **22 → 14** textual references (**21 → 9** code
references). Remaining code crossings and why they stay:

- **Index**: the helper threads the index into `emit_method_call` for tier-2
  `op At` inlining, and mask operands store the AST receiver.
- **MethodCall / Reflect**: dispatch reads receiver AST identity
  (`Expr::Index` collection-element detection, dynamic-dispatch concrete
  resolution) and `.^Ptr` constructs `AddrOf(receiver)`; member bodies take
  AST argument slices.
- **Call**: intrinsic/enum/struct/frgn callee surfaces take AST argument
  slices (ordinary user-call args already emit through the HIR operand path).
- **Short-circuit Binary**: `logical.rs` consumes AST operands and AST purity
  predicates.
- **Block children**: `to_ast::stmt` supplies the SOURCE statement to the
  statement seam's splice fallback (not a dispatch reconstruction).
- **Match `List` arm bodies**: accepted collection-witness constructor
  exception.

Gates: `cargo build` (no new warnings); `scripts/gate.sh quick` lib 3782/3782
(the pre-existing `verify_report` JSON-schema failure remains in the
integration batch — reproduced on unmodified main); `hir_emit_ab.sh` full
corpus ALL BYTE-IDENTICAL; `ll_determinism_check.sh --quick` ALL
DETERMINISTIC; focused HIR/match/field/tier2/corpus suites 28/28; Praetor
on `src/backend/llvm` produces no new diagnostics (net one fewer issue in
`emit_match`/`emit_struct_literal`).

## Phase C, statement-operand slice (2026-11-13, done)

`emit_hir_stmt.rs` no longer crosses `hir::to_ast` at all: **8 → 0** code
references (the module doc is the only remaining textual mention). The
value-binding and term/match statement arms now take their shape and fallback
expressions from the SOURCE statement at the same pair slot, and HIR still
drives native value emission:

- **ArrowAssign (gate + operand)**: new `ArrowOperand::HirSourced(hir, source_ast)`
  carries the pair slot's AST leaf. `name()`/`declared_type()` read HIR; `emit()`
  prefers `emit_hir_operand` and falls back to the source AST; `to_ast()` is the
  source AST, so the `#StdOut` `Print#` path needs no reconstruction. The
  reconstructibility gate is gone (the source slot always exists).
- **Let**: `emit_let_pair` takes the paired `HirStmt` plus the SOURCE
  initializer (`LetValue::Hir(hir, source_ast)`); bare lets pass `Ast(None)`.
- **Assign**: LHS/RHS shape and RHS fallback come from
  `Statement::Assign`; the RHS still emits through `AssignValue::Hir`.
- **Term/Tail**: `term_source_value` extracts the paired source expression;
  the value emits through `emit_hir_operand` with the source AST as the
  uncovered-leaf fallback. A value term with no source declines before the web
  flush (no partial IR).
- **Match scrutinee**: emitted through `emit_hir_operand`, falling back to the
  paired `Statement::Match` scrutinee; arm patterns/bodies were already sourced.
- **TempTransfer**: the synthetic binding uses the replaced return statement's
  source expression at the pair slot; its synthetic return carries a
  synthetic `Identifier` source so the term arm never crosses `to_ast`.

No `to_ast` site in this file is kept as an AST-only exception. The remaining
`to_ast` families in `emit_expr.rs` (Index/MethodCall/Reflect/Call/short-circuit/
Block children) are unchanged and remain the next expression slice.

Gates (re-verified independently 2026-11-13 after the interruption):
`cargo build`/`--release` green, no new warnings; `cargo nextest run --lib`
3767/3767; `scripts/gate.sh quick` 42/42; `hir_emit_ab.sh` full corpus ALL
BYTE-IDENTICAL; `ll_determinism_check.sh --quick` ALL DETERMINISTIC.
`cargo test --all --no-fail-fast` shows 72 failures — all exec/toolchain
golden suites plus contract proof binaries (host clang currently degraded:
`unknown platform, assuming -mfloat-abi=soft`) — and the failure set is
IDENTICAL at the base commit `33b4ca0c0` in a temp worktree
(`daily_use_smoke` 41/41, `inferred_output` 4/4, `tail_expr` 1/2 pass,
`pgated_term_backend` 2/2, `c_driver_callback`,
`frgn_contract_preconditions`); each sampled failure also fails with
`BRIEV_HIR_EMIT=0`. No failure implicates this slice. Praetor on the
changed files: `emit_hir_stmt.rs` clean; `emit_stmt.rs` has the identical
27-diagnostic pre-existing set at HEAD (line-number shifts only).

## Phase C-4 design (recorded 2026-11-13, not started)

Remaining `to_ast` code refs: `emit_expr.rs` 9, `mod.rs` 1 (8029), plus
result-boundary 12 (ratified) and decl/ABI surfaces 6 (Phase D).

All remaining `emit_expr` crossings are receiver/callee dispatch surfaces.
The shared blocker is `emit_method_call` (line ~4089): it probes receiver AST
identity, resolves dynamic dispatch, and hands member bodies AST argument
slices via `emit_member_body`. `emit_reflection` (~5095) additionally probes
`Expr::Identifier` (wire `Ready`), `Expr::AddrOf` (`.^Ptr`), and calls
`is_semantic_string(recv, ..)`. `Index` threads its operands into the tier-2
`op At` method-call path and `MaskIndexOperands` stores AST receivers.

Required work, in dependency order:
1. Extend `ExprView` with the probes these families need: `as_addr_of()`,
   `is_index()`, `is_literal()`, `index_parts()`, and a `shape()` that
   covers the tuple/collection/dynamic-receiver discriminations currently
   done by matching on `Expr`. Every probe must be answered from the 1:1 HIR
   node (`HirExprKind`), not by reconstruction.
2. Convert `emit_method_call` + `emit_member_body` to take `ExprView`
   receiver and `ArgsView` (Ast slice / Hir slice) — mirroring `SpawnArgs`.
   Member bodies that genuinely require AST statement surfaces (none known;
   statement path is native now) are the only allowed leaf conversions.
3. Convert `emit_reflection` to `ExprView` (Ready/AddrOf/string probes).
4. Convert `Index` via `emit_index_with_obj` with `ExprView` operands.
5. Convert `Call`'s eager `ast_args` gate: the AST slice exists only for
   intrinsic/enum-ctor/struct-ctor/frgn callee surfaces; those callee arms
   take HIR args once (2) lands.
6. Short-circuit Binary: add a `logical.rs` HIR operand variant.
7. `mod.rs:8029` closure match-arm body: convert to HIR statement emission
   (the statement path is native).
8. Block children `to_ast::stmt` and the Match `List` arm stay: they supply
   the source statement / collection-witness constructor, not dispatch.

Recognition gate: every step must keep full `hir_emit_ab.sh` BYTE-IDENTICAL
and `--quick` determinism clean; no IR-visible change is acceptable in this
phase.

### C-4 step 1 done (2026-11-13): Reflect converted to ExprView

`emit_reflection` now takes `ExprView<'_>`; both callers pass views (Hir arm
gates with `hir_expr_lossless` — the exact `to_ast` reconstructibility
predicate — before declining to the seam splice, as before). Receiver
emission goes through `emit_view_expr`; the Ready/Available probes use
`identifier()`; `is_semantic_string` replaced by its ident twin; `.^Ptr`
emits a native AddrOf view (shared H4/AST AddrOf arms are structurally
identical). The duplicated Available symbol block was collapsed (identical
rebinding). `emit_expr.rs` to_ast code refs: 10 (was 11; Reflect receiver
gate gone). Gates: quick 42/42, full A/B BYTE-IDENTICAL, determinism clean.
Next: Index + MethodCall receiver/args (needs `emit_member_body` ArgsView),
then Call eager-arg gate, short-circuit, mod.rs closure arm.

### C-4 step 2 done (2026-11-13): short-circuit And/Or fast path native

`logical.rs` now owns the shared predicate (`short_circuit_register_fast`)
and the register-level emission (`short_circuit_register_level`) over
`ExprView`, with HIR twins of `expr_is_side_effect_free` /
`expr_contains_beginprogram` that mirror the AST matches one-for-one (to_ast's
literal map is 1:1 — Bytes→Quoted, Int→Decimal, BeginProgram→BeginProgram —
so fast-vs-CFG routing cannot diverge). `hir_expr_family_covered` admits
fast-safe, lossless And/Or; the HIR Binary arm emits them through the view
path. The CFG form declines to whole-expression conversion (today's AST
routing), including `emit_exit_short_circuit` (loop_engine, unchanged).
Gates: quick 42/42, logical 4/4, full A/B BYTE-IDENTICAL, determinism clean.
Remaining C-4: Index + MethodCall (emit_member_body ArgsView), Call eager-arg
gate, Block-children splice, mod.rs closure fallback (accepted leaf
fallback), Match-List constructor exception (accepted).
