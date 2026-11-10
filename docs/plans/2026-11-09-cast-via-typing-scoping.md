# `Cast.via` / HIR expression typing — scoping (2026-11-09)

Roadmap item #1 after the HIR Praetor leg. Investigates what "the typer-blocked
item" actually blocks, and chooses an approach.

## What exists today

**HIR expression types are almost entirely absent.**

- `HirExpr.ty: Option<Type>` is filled only for literal kinds and for the
  *target* of a `Cast` (`src/hir/lower.rs:723-755`). Identifiers, `Call`,
  `Binary`, `Unary`, `Field`, `Index`, `MethodCall`, `Reflect`, `Tuple`,
  `StructLiteral` are lowered with `ty: None`.
- `HirExprKind::Cast { inner, to, via }` carries `via:
  Option<Vec<CastStep>>`. `resolve_cast_paths` (`src/hir/cast_paths.rs:12`)
  fills it only when `inner.ty` is `Some`, so today it resolves literal-source
  casts only.
- **`resolve_cast_paths` is not called in production.** Its only caller is the
  unit test at `src/hir/mod.rs:342`. `build_hir` (`src/backend/mod.rs:600`)
  lowers without it. So `via` is effectively an unpopulated fidelity field.

**The backend does not need `via`.** `emit_cast_operand`
(`src/backend/llvm/emit_expr.rs:743`) recomputes the lane from the *typed
source register* (`TypedRegister.ty`) and the target via `emit_cast_path`; the
HIR cast arm (`emit_expr.rs:588`) passes the register. This matches the H4
note: cast emission was never blocked on `Cast.via`.

**What is actually AST-shaped.** The HIR arms that still reconstruct AST for
dispatch, not for typing:

| HIR arm | site | reconstruction reason |
|---|---|---|
| `Field` | `emit_expr.rs:609` | `emit_field_access` inspects receiver syntax (event wire, pooled instance prefix, boxed offsets) |
| `Reflect` | `emit_expr.rs:621` | `emit_reflection` takes the AST receiver |
| `MethodCall` | `emit_expr.rs:627` | `emit_method_call` inspects receiver + args shapes (tiers, string ops, traits) |
| `Match` | `emit_expr.rs:436` | `emit_match` evaluates scrutinee first; arm bodies reconstruct (queued body split) |
| `Binary` And/Or | `emit_expr.rs:653` | `emit_short_circuit` consumes AST operands |
| `Slice`/`Index`/`StructLiteral`/`Tuple` | `emit_expr.rs:507-586` | dispatch reads AST shapes; receiver register is already split |

Those are shape-driven dispatch helpers, and they already accept an
`evaluated: Option<&TypedRegister>` where useful. `expr.ty` would not change
their decisions; it is not the blocker H4 feared.

**What consumes `HirExpr.ty` today**

- `cast_paths::resolve_cast_paths` (test-only).
- `hir/ownership.rs:99` — fallback for a value's type when the binding has
  none (globals/lets).
- `specialize::subst_expr` rewrites it (`specialize.rs:297`).
- `transition_graph` clones `via` when rebuilding (`transition_graph.rs:336`).

No codegen path reads `expr.ty`; register types come from emission.

## Options

### A. HIR-local type propagation pass (`hir/typing.rs`)

Bottom-up annotation using facts the HIR already carries:

- identifiers: let/param/state/global/init/const declared types (all present
  in `HirProgram`);
- `Call`: `HirCallable.output_list` + intrinsic signatures + protocol ops;
- `Binary`/`Unary`: operand protocols (Int/Float/Bool/String) via the
  CastingGraph, never type names (Rule 19);
- `Field`/`Index`: struct/collection type facts (`hir/type_facts.rs` already
  builds field tables);
- `MethodCall`/`Reflect`: trait method tables (`HirProgram::trait_functions`,
  `protocol_binding_fns`) + receiver type;
- `Cast`: target (already typed); recurse.

Cost: ~400–700 lines, one new pass, plus parity tests. Advisory annotations —
no IR change is possible if emission keeps using registers. Risk: a second,
partial type system drifting from the frontend typer; mitigate by asserting
agreement with declared types and by gating every consumer incrementally.

### B. Typechecker side-table (`Span`/id → Type)

Instrument the existing typer to record the type of every expression it
infers, key it by span (or node identity), and stamp HIR nodes during
lowering.

Cost: ~200 lines of plumbing but touches every inference site in a 6k-line
typechecker; spans are not unique for synthesized/normalized nodes
(composites, variadics, macros run before/around typing); the typer would need
a record mode. Risk to correctness is concentrated in the most load-bearing
pass. Higher risk, similar payoff to A.

### C. Status quo (drop `via`, keep AST dispatch)

Stop pretending: remove `via` (or keep it test-only) and keep dispatch
AST-shaped. Zero cost. But it forfeits two H5 prerequisites: (i) a
`Cast.via` audit/consumer story, and (ii) HIR-native `emit_match` bodies and
`MethodCall`/`Reflect` receivers, which are the remaining seams where the
backend still asks the AST questions.

## Recommendation

**A, staged, with a hard consumer gate.** Do not port the typer.

1. **A1 — cast paths first.** Wire `resolve_cast_paths` into `build_hir`
   (it is currently dead), extend `expr.ty` for identifiers, `Cast`,
   `Binary`, `Unary`, and `Call` (signature lookup). Measure the resolved
   fraction on the corpus. Acceptance: every explicit `as` cast in the corpus
   whose endpoints are declared types resolves; add an audit that reports
   unresolved casts by reason (unknown source, unknown target, no path) —
   audit baseline may be non-zero, ratcheted like `hir_unsupported_audit`.
2. **A2 — field/index/method.** Field and index typing from
   `hir/type_facts.rs`; method-call result type from trait/protocol tables.
   This is what lets `MethodCall`/`Reflect`/`Field` arms ask HIR for the
   receiver type instead of reconstructing AST shapes. Convert one arm at a
   time; each conversion must keep `hir_emit_ab.sh --quick` byte-identical.
3. **A3 — Match body split.** With typed receivers and bodies, split
   `emit_match_arm_value` so arm bodies emit through the HIR seam. This is
   the last dual-fuel arm.

Every stage: `cargo test --all`, corpus sweep (0 mismatch), A/B byte identity,
new `hir_typing_parity` tests (types agree with declared types; unresolved
casts audit), Praetor clean on changed files.

**Explicit non-goals**

- A second full typechecker. Unannotated nodes stay unannotated instead of
  guessing; consumers must tolerate `None` (as today).
- Type names as dispatch keys (Rule 19). All protocol/category answers come
  from the CastingGraph/TypeUniverse.
- Changing emitted IR. Any stage that changes IR is a codegen bug, not a
  typing win.

## Open questions

- Should unresolved `Cast.via` be a compile error, a warning, or only an
  audit? (Proposal: audit first; error once A1 shows zero unexplained
  corpus cases.)
- Do `specialize`'s substituted types need to invalidate cast paths (it
  rewrites `to` but keeps `via`)? Follow-up once A1 lands: decide whether
  `via` is re-resolved after specialization or recomputed at emission only.
- Trait-method dispatch resolution for dyn values (`Dyn<T>`) — which table is
  authoritative (protocol bindings vs trait functions) needs one worked
  example before A2.

## Execution record (2026-11-09)

**A1 landed.** `src/hir/typing.rs` (`annotate_types`) fills `HirExpr.ty` for
identifiers (globals, constants, params, lets), binary results (comparisons →
Bool; arithmetic → Float if either side is Float, Int only when both are Int),
unary, call results (`output_list` / single `result`, then intrinsic
signatures), and cast targets. Container kinds recurse and stay untyped unless
unambiguous (`If` takes the then-branch type; `IsType` → Bool). `None` remains
the answer for unknown shapes.

Wiring: `backend::build_hir(items, type_universe)` runs the pass (and
`resolve_cast_paths`) whenever the universe is supplied; `via` is now
populated in production for the first time. `target/debug` sweep: 139/0;
`cargo test --all`: 3605 pass; `hir_emit_ab --quick`: byte-identical.

Still open: A2 (Field/Index/MethodCall/Reflect typing from type facts +
trait/protocol tables) and the unresolved-cast audit proposed above.

**A2 slice (2026-11-09).** Field and Index annotations landed: a field table
built from `HirTypeDefKind` (generic params retained) types `receiver.field`
with use-site substitution, and `List<T>`/`Array<T>`/`Vector` indexing types
the element. Explicit casts through field/index sources now resolve `via`.
Two focused tests plus a cast-path test; gates: lib 3576, sweep 139/0, A/B
byte-identical, all-tests 3623. MethodCall/Reflect remain.
