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

Landed this increment: **Cast** — the arm splits cleanly at operand
emission; the tail (graph path + LLVM coercion fallback + strategy
propagation) extracted verbatim into `emit_cast_operand(out, v, src_reg,
target, indent)`. The HIR side emits its operand natively — no AST, and
NOT blocked on `Cast.via` typing (the graph path is recomputed from
`(src.ty, target)` at emission, identical either way). IR parity test
extended with a Cast case.

Landed this increment: **AddrOf** — the identifier path (state-field GEP /
struct-literal alloca / let-slot spill / function pointer) extracted
verbatim into `emit_addr_of_identifier`; the HIR side dispatches on
`HirExprKind::Identifier` natively and emits non-identifier operands via
`emit_hir_operand` (the AST add-i64 tail mirrored). Deep coverage extended.

Landed this increment: the **Call family** — `emit_user_call` refactored
onto the dual-fueled `emit_user_call_args` (HIR args emit through
`emit_hir_operand`: covered leaves native, uncovered leaves convert
per-leaf; AST args keep the exact legacy path via the infallible
`emit_user_call` wrapper), `emit_call_hir` mirrors the AST dispatch
(intrinsic / enum variant / struct constructor / user), and the seam's
`Expression` fast path admits Call statements (reconstructibility guarded
before any emission). The sweep's first run caught a real register drift:
`emit_hir_operand` routed uncovered leaves through `emit_hir_expr`, which
burns its result register up front — one extra register per argument
(rating_population_gate). Fixed by deep-coverage check first; corpus pins
it. IR parity test covers literal/unary/binary/cast families.

Landed this increment: **Field / Reflect / MethodCall / IsType** HIR arms —
dispatch-only into the existing helpers (receiver/args reconstructed before
any emission, so None falls back cleanly; MethodCall/Reflect dispatch on the
reconstructed receiver's shape, not a register, so no register-taking
refactor is needed for them). IsType is the constant-true leaf and joins the
deep coverage set. **Expression-statement seam coverage is now complete for
every reconstructible HIR expression**; what remains outside it is
`Other`-node statements and the big structural emitters (Match/If/Block/
Foreach as EXPRESSIONS inside non-statement positions), which still convert
whole-node.

Remaining: Index/Tuple/List/Range/Slice/Lambda/Spawn/StructLiteral
expression families (structural; same reconstruct-then-dispatch pattern
where the helpers take AST), MethodCall/Reflect receiver TYPING (HIR
`Cast.via`) remains the only typer-blocked item.

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

## H3.0 closed: the unsupported surface is ZERO (2026-11-05)

The H4.1 census showed every `Other` node in the corpus (339 across 104
files) was `plugin-intercept`. PluginIntercept is now a fully-represented
HIR node (name/args/type_args/receiver/chain_refs) with exact AST-mirrored
handling in every analysis (visit walks args+receiver; defn-liveness roots
args; storage class collects nothing; swan-song remaps args; graph
treats it opaque — each matching its AST counterpart). The audit ratchet
tightened 62 → **0**: the whole corpus lowers with no `Other` nodes, so
every HIR parity suite now covers all 132–135 files (skipped_unsupported=0
everywhere). Checked lowering still rejects intercepts explicitly
(plugin stages must expand them; the backend panics on one reaching
codegen). The expanded coverage immediately caught and fixed a real HIR
gap: the storage-class proof surface included sync-wrapped transactions
(the AST iterates bare `TopLevel::Transaction` only) —
`mixed_effect_shape` pinned it.

## H3.6 backend wiring ✅ DONE (2026-11-05)

The specialization site (`emit_specialized_definition`) now derives
specializations by HIR substitution under `BRIEV_HIR_EMIT=1`:
`hir::specialize::specialize_with_substitution` (explicit subst-map form,
added for the queue's shape) produces the specialized HIR callable, whose
body feeds the statement seam; the AST-substituted body stays as the
fallback/splice source, and the seam's per-statement debug asserts pin
HIR substitution == AST substitution statement-for-statement (the sweep's
131/0 run IS that proof on every trait-generic specialization in the
corpus). Golden: `iter_chain_multi_struct_golden_output` green (runtime);
full daily_use_smoke 37/37. Base HIR callables ride on
`CompilerContext.generic_hir` (populated once per round).

## Native control-flow statements: Guarded + Block (2026-11-05)

The seam now emits `when` guards and blocks natively: the condition goes
through operand recursion, children recurse through the seam (native or
per-child conversion), and the bookkeeping mirrors the AST arms verbatim
(guard temp scoping, label counters, termination/backedge/cur_block
updates, Block's trailing void register). Children of a seam-reachable
Guarded/Block are reconstructible by construction (the parent converted in
`hir_sourced_body`), so the per-child convert fallback is unreachable —
it exists so no path can double-emit. `guard_cond_i1` shared. Every
`when` in the corpus now emits from HIR control flow.

## Native terminal/control statements: Break/Trap/Halt/Yield/Check (2026-11-05)

The seam also emits the small terminal statements natively, mirrored
verbatim from the AST arms (Break branches at the innermost foreach label,
Trap/Halt with the target-family wfi spin, Yield's void register, Check's
static-only no-op with the condition flowing through operand recursion for
emission ordering). Break-without-foreach falls to the AST arm's same
panic. The big four (Let/Assign/Term/Foreach/Match) remain conversion-
based pending their register-taking refactors.

## Native Gate + Rollback statements (2026-11-05)

Gate (convergence branch with defn-trap fallback) and Rollback (deferred
cleanup + ret-ty-matched return) emit natively, mirrored verbatim from the
AST arms. Statement seam native set: Expression, Guarded, Block, Break,
Trap, Halt, Yield, Check, Gate, Rollback — the remaining conversion-based
kinds are the data-flow big four (Let/Assign/Term) plus Foreach/Match and
the cleanup/wire kinds (FreeHint/KeepHint/Defer/SyncBlock/Mutex/
ArrowAssign/EndProgram/Open/TrgBinding/MetadataAssignment/InlineAsm).

## Native FreeHint/KeepHint/SyncBlock/Mutex statements (2026-11-05)

FreeHint (task-cancel branch + strategy-aware destroy), KeepHint (scheduler
directive, no emission), and SyncBlock/Mutex (serial sections, inline
recursion through the seam) emit natively. The seam's native statement set
now covers every control/terminal/cleanup kind except the data-flow big
four (Let/Assign/Term), Foreach/Match, EndProgram, and ArrowAssign.

## Native EndProgram statement (2026-11-05)

Process-exit emission (web flush batch, value side effects via operand
recursion, exit-provider dispatch with the `defn_takes_state` gate,
program-end marker, unreachable) mirrored verbatim from the AST arm. The
remaining conversion-based statement kinds are exactly the data-flow big
four (Let/Assign/Term), Foreach/Match, and ArrowAssign.

## Native Term statement (2026-11-05)

The P-gated transaction exit emits natively: reconstructibility checked
before the web-flush/defer prefix (no partial emission on fallback); value
emission through operand recursion (list-literal returns construct from
the declared return type like the AST arm); the entire tail — backedge
guard, context restores, float-cache unbox, member capture, callable-txn
store+post-branch, enum retain + ret adaptation, void unwinding, and the
bare-term P-gate — extracted verbatim into `emit_term_value_tail` +
`emit_bare_term_gate` + `begin_term_return_context`, shared by both arms.
The reactor's most semantically loaded statement now emits from HIR.

## Native Tail statement (2026-11-05)

`tail expr;` IS `term expr;` (the AST arm delegates verbatim) — the seam
routes it through the native Term path; the Term arm's reconstructibility
guard keeps the legacy convert path for unsupported values.

ArrowAssign stays conversion-based for now: its ~200-line dispatch (wire
fire with native parameter slots, stream writes, member insert/extract
with consume, provenance copy) inspects target/value AST shapes across
every path — nativizing it needs the same operand-split treatment but is
the largest remaining arm; queued for a focused session with Foreach/
Match/Assign/Let.

## Native statement Match (2026-11-05)

Scrutinee through operand recursion; patterns reconstructed (lossless)
for the shared condition-chain/binding helpers; arm children through the
seam (native or per-child conversion). Patterns/scrutinee reconstruct
BEFORE the scrutinee emits — no partial IR on fallback. The BUGS.md
"callable-txn bodies silently drop match" fix (per-arm terminated reset,
all-terminated merge with unreachable) mirrored verbatim.

## Native Foreach statement (2026-11-05)

The AST Foreach arm extracted verbatim into `emit_foreach_statement` with a
dual-fueled body (`ForeachBody::Ast` keeps the legacy path; `Hir` routes
children through the seam). The iteration-source dispatch (ranges, vector
fields, tier iteration, collection kinds) reads the reconstructed list's
shape — identical — and the loop engine (carrier slots, phis, break
labels, string-lane early exit) is shared verbatim. The list reconstructs
before any emission. Every `foreach` in the corpus now emits from HIR
control flow.

## Native Assign statement (2026-11-05)

The AST Assign arm extracted verbatim into `emit_assign_statement` with a
dual-fueled RHS (`AssignValue::Hir` emits covered leaves natively through
`emit_hir_operand`; `Ast` keeps the legacy path). The dispatch still reads
the reconstructed LHS/RHS shapes — provenance, AddrOf-pop early return,
typed-list collection construction, provenance copy — which is what drives
assignment semantics; both cross the boundary, reconstructed before any
emission.

## Order and cadence

A → B → C → D. Commit per slice; every commit passes the full gate set. If a
slice uncovers a parity gap (as the sweeps did), fix HIR/lowering first — the
AST path stays authoritative for anything not yet proven equal.
