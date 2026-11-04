# H4 completion: consumer flips → emitter seam (2026-11-05)

Finishes the typed-HIR migration slices A–D after H3 + the H4.1/4.2 groundwork
(`docs/plans/2026-11-04-typed-hir-h3-h5-execution.md`). Standing gates apply to
every slice: `benchmarks/hir_emit_ab.sh` byte-identical A/B, the full-corpus
debug sweep with cross-check asserts, HIR parity suites, `cargo test --lib`,
determinism. AST remains authoritative until a slice flips; emission is
unchanged until a slice explicitly changes it (and then only behind
`BRIEV_HIR_EMIT=1` with byte-identical proof).

## Slice A — loop-shape + program-convergence consumer flip ✅ DONE (2026-11-05)

All steps landed. Extra findings fixed on the way:
- **Merged vector-group order was random**: `slp_isomorphism::merge_groups`
  iterated its sig/width HashMaps unsorted, so `vector_groups` order drifted
  run-to-run (nbody_sqrt). Both SLP modules now iterate sorted keys;
  `nbody_sqrt` + `mixed_effect_shape` added to the determinism corpus (full
  mode green).
- **Sync-wrapped self observation**: AST's local-field walk only skips a BARE
  top-level transaction; sync-wrapped copies of the same txn are re-walked as
  foreign surfaces. HIR mirrored (`mixed_effect_shape` temp pinned this).

## Slice A (plan, kept for the record)

The biggest remaining semantic dispatch that needs no emitter work.

1. **Delete dead `Convergence::Explicit(Expr)`** (`src/analysis/loop_shape.rs:62`).
   Grep-verified: never constructed. This removes the last AST type from
   `LoopShape`/`Convergence`.
2. **Unify HIR shape types**: `hir::loop_shape` returns the shared
   `LoopShape`/`ProgramConvergence`; delete `HirLoopShape`, `HirConvergence`,
   `HirProgramConvergence` and the duplicated selector functions
   (`prefer_effect_carry`, `all_direct_loops_prefer_carry`,
   `program_carry_mode`, `escaped_effect_cause` — AST module's are used and
   now take the shared type). Add `PartialEq` to `LoopShape` (all members
   already `PartialEq`).
3. **Neutral bound for convergence**: add
   `BoundExpr { Literal(i64), Var(String) }` with `to_expr()`; change
   `ProgramConvergence.counter_ge_bounds` to `Vec<(String, BoundExpr)>`. AST
   and HIR builders emit it; the backend synthesis site
   (`src/backend/llvm/mod.rs` ~5019) calls `to_expr()`.
4. **Flip `loop_shapes`** in `analyze_program`: under the flag build the HIR
   transition graph (`None` exit / no pragmas, mirroring the AST graph) and
   compute shapes from HIR; debug-assert exact AST equality; use HIR.
5. **Flip `program_convergence`** in the backend: under the flag build the HIR
   graph from `analysis.hir` and use HIR convergence; debug-assert equality.
6. **Update** `tests/hir_loop_shape_parity.rs` to direct `LoopShape`
   equality (no more field-by-field adapters).
7. Gates: sweep + A/B + determinism + lib.

Risk: low-moderate. Diff mostly type unification. Impact: loop-lowering
strategy (carry mode, tiling, vector groups, natural exit) becomes HIR-driven.

## Slice B — swan-song / batch-shape consumer flip ✅ DONE (2026-11-05)

Under the flag the hoist runs on typed HIR and crosses the lossless
HIR→AST boundary; unsupported nodes fall back to the AST result for the
whole program; debug asserts exact AST equality. Txn set mirrors the AST
loop (bare top-level transactions only). Sweep 131/0; A/B byte-identical;
lib 3497.

## Slice B (plan, kept for the record)

`build_swan_songs` (backend/mod.rs ~640) consumes AST statements. Under the
flag: compute `has_swan_song`/`hoist_swan_song` from HIR and reconstruct
bodies through the lossless `hir::to_ast` boundary; unsupported nodes fall
back to the AST result for that program. Debug-assert equality. `batch_shape`
consumes the same structure unchanged. Small, gated identically.

## Slice C — statement emitter seam ✅ infrastructure DONE (2026-11-05)

`src/backend/llvm/emit_hir_stmt.rs` lands the seam: the choke point walks the
HIR body per statement — supported statements are emitted from HIR (native
count), statements containing unsupported nodes are spliced from the AST body
at the same index (fallback count), length drift falls back to the whole AST
body. Coverage counters live on the backend (`hir_stmts_seen/native/fallback`)
with unit tests. The body-equality debug assert still guards every emission.
Native per-kind arms (Guarded/Foreach/Match recursion, then leaf kinds with
HIR expressions) are the remaining work, tracked below — they replace the
conversion inside `convert_hir_statement` kind-by-kind.

## Slice C (plan, kept for the record)

Make the H4.2 choke point emit **HIR statement kinds directly** instead of
reconstructed AST bodies.

1. New `src/backend/llvm/emit_hir_stmt.rs`:
   `emit_hir_statement(backend, out, &HirStmt, indent) -> Option<TypedRegister>`.
   `None` = this statement needs the AST path (unsupported node), so the
   caller converts just that statement via `hir::to_ast::stmt` and delegates.
2. Leaf kinds first (`Expression`, `Term`, `Tail`, `Assign`, `Let`): HIR
   drives the statement; leaf expressions still cross `hir::to_ast::expr`
   (Slice D removes that).
3. Then control flow (`Guarded`, `Block`, `Foreach`, `Match`): children
   recurse through `emit_hir_statement`, never converting a whole block.
   Reuse the AST emitters' helpers where they take data (labels, scopes) and
   only duplicate control flow; anything that needs AST-shaped statements
   falls back per statement.
4. The choke point (`emit_definition`/`emit_transaction` + body hooks)
   iterates HIR statements calling `emit_hir_statement`; a debug coverage
   counter (native vs fallback) tracks progress.
5. Gates unchanged; the body-equality debug assert is replaced by per-statement
   fallback accounting (equality of IR is proven by the A/B).

Risk: highest of the four — control-flow state (member.exit labels, P-gating,
defer stacks) lives in `emit_stmt.rs` (3.3k lines). Land kind-by-kind; never
convert-and-delegate a whole body.

## Slice D — expression families 🚜 IN PROGRESS (first family landed 2026-11-05)

Landed: the 605-line AST `Identifier` arm extracted verbatim into
`emit_identifier_load` (single shared implementation); `emit_hir_expr` with
the literal families (Int/TaggedInt/Char/Float/Bool/BeginProgram/Bytes/
TaggedBytes) plus Identifier via the shared helper; the defn body loop now
routes through `emit_hir_statement` — a family-covered `Expression`
statement emits natively (no AST conversion, mirrors the plain fallthrough
including `last_expr_reg`), everything else converts/falls back per
statement. Unit test pins literal-family IR text equality (registers and
indent normalized).

Landed this increment: **Unary + Binary** native families (operand recursion
via `emit_hir_operand` — native when covered, per-leaf conversion otherwise;
Mod phi-counter special case and float-mix conversion mirrored verbatim;
And/Or short-circuit falls back to whole-expression conversion) and the
**txn body loop** routed through the seam (mirrors the defn loop). Deep
coverage guard (`hir_expr_family_covered`) extended to Unary/Binary.

Landed this increment: **Consume/Await/Deref** native families — the AST
arms extracted verbatim into shared helpers (`emit_consume_operand`,
`emit_await_handle`, `emit_deref_load`), HIR arms mirror via
`emit_hir_operand`, deep coverage extended. Txn-loop routing also landed
earlier in this slice.

Remaining families: Call (intrinsic/enum/user — needs register-level operand
helpers, the deeper refactor), Field/Index (large arms, same extraction
pattern), then `Cast`/`MethodCall`/`Reflect` need HIR `Cast.via` typing
(frontend typer).

## Slice D (original plan, kept for the record)

Refined first step after reading the emission arms: the AST `Expression`
statement arm (emit_stmt.rs ~1621) has three special cases before the plain
path — `AddrOf` extract-strategy dispatch, void-match rerouting to the
statement-match path, and the plain `emit_expr` fallthrough. The first
native arm must reproduce that dispatch for `HirStmtKind::Expression` when
the expression is family-covered (Literal/Identifier), and fall back to
conversion otherwise. Literal emitters to reuse directly: `emit_int`,
`emit_string_literal`, `emit_byte_literal` (emit_expr.rs 2712/2728); the
`Identifier` arm (emit_expr.rs ~259-860) is large — extract it into a
name-keyed helper (`emit_identifier_load`) so the AST arm and the native HIR
arm share one implementation rather than duplicating 600 lines.

## Slice D (original plan, kept for the record)

1. `emit_hir_expr` for leaf families: `Literal`, `Identifier`, `Unary`,
   `Binary`, `Call` (with `to_ast::expr` only for arguments outside the
   ported set, shrinking as families land).
2. Wire into `emit_hir_statement`'s leaf expressions; then into recursion
   sites (Binary operands, Call args).
3. Delete `hir::to_ast` piece by piece as families cover the corpus (the
   debug sweep's fallback counter shows what remains).
4. `Cast`/`MethodCall`/`Reflect` families land last (they need HIR `Cast.via`
   typing — the frontend typer work called out in the 2026-11-04 plan).

## Order and cadence

A → B → C → D. Commit per slice; every commit passes the full gate set. If a
slice uncovers a parity gap (as the sweeps did), fix HIR/lowering first — the
AST path stays authoritative for anything not yet proven equal.
