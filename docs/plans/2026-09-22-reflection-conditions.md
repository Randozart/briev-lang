# Reflection Conditions — `.^^Size`/`.^^Element` in Comptime Fold Conditions

**2026-09-22.** Extends `docs/plans/2026-09-21-comptime-fold-expansion.md`
(Phase 2 → reflection). A composite body may gate a `match`/`when` arm on
the SHAPE of a receiver — `match x.^^Size { 4 => … }` — where `x` is
bound to a declared state variable (`let buf: Float[4]`). Today the fold
env carries only VALUES (`ComptimeVal`), so `Expr::Reflect` returns `None`
and the condition stays a runtime branch. The plan resolves the reflection
from the state declaration's TYPE at expansion time.

## Why

The composite is the metaprogramming layer: `softmax_fused!(x, …)` already
splices `match dlen <= 32` when `dlen` is a comptime value. The same
specialization must work when the discriminant is the receiver's static
SHAPE — `.^^Size` (element count of a declared array) and `.^^Element`
(element category code). A composite that fuses a fixed-shape kernel can
then generate per-shape bodies at expansion, not at runtime.

Zero new syntax, zero compiler algorithm knowledge (rule 23): `.^^` is
already the reflection disclosure marker; this wires the compile-time
fold to the declared type the way the LLVM backend already folds it
(`emit_expr.rs:3297` `vector_element_count`, `:3437` element category code).

## Design

1. **State-type map.** `expand_composites(items, pm)` scans `items` for
   `TopLevel::Statement(Statement::Let { name, ty, .. })` and builds
   `state_types: HashMap<String, Type>` — the declared type of each
   top-level state variable. (Also sweep `TopLevel::StateDecl` defensively;
   analysis may have produced it.)

2. **Thread through the fold.** `eval_const` gains a `&HashMap<String, Type>`
   parameter (the state types). On `Expr::Reflect(recv, target, kind)`:
   - `recv` is `Expr::Identifier(name)` and `state_types[name]` resolves:
     - `("Size", CompileTime)` on `Type::Vector(_, dims)` →
       `Some(ComptimeVal::Int(product of Anonymous dims))` (mirrors
       `vector_element_count`; non-`Anonymous` dims contribute 1).
     - `("Element", CompileTime)` → the element category code of the
       inner type (String→Char→3, Float→1, Bool→2, Char→3, Ptr→7,
       struct→5, else 0 — mirrors `type_category_code`).
   - Anything else → `None` (fail-open; unchanged).
   - The map lookup is `state_types.get(name)`; a receiver that is a
     plain value (not a state decl) declines — no guessing.

3. **Call-site threading.** Every fold entry that reaches `eval_const`
   passes the state map: `fold_stmt_list`, `fold_expr_in_place`,
   `fold_stmt_form_match`, `fold_expr_form_match`, `fold_let_stmt`,
   `unroll_static` (generation pre-substitution). `expand_composites`
   builds the map once and passes it down; `expand_composite_invocation`
   forwards it. To limit churn, the map is `&HashMap<String, Type>` —
   immutable, shared.

## Gates

- A composite body `match x.^^Size { 4 => …; _ => …; }` invoked with a
  `let buf: Float[4]` argument splices ONLY the `4` arm (expansion-side
  test asserts the other arm is absent from the expanded body).
- `.^^Element` on `Float[16]` folds to `1`; on a `String` receiver to `3`.
- A runtime receiver (a `let` with a non-vector type, or an expr arg that
  is not a state name) stays a runtime branch — fail-open, no error.
- Full `cargo test --lib` + conformance sweep green; Praetor on changed
  files (helpers ≤ 15 complexity).

## Files

- `src/plugin/composite.rs` — state-type map, threading, `eval_const`
  reflection arms, tests.
- Corpus: `examples/syntax/meta/reflect-fold.bv` — a shape-gated composite
  (follows `reflect.bv`). Conformance-enforced.
- Docs: `docs/plans/2026-09-21-comptime-fold-expansion.md` — note the
  reflection conditions extension.

## Undo

Remove the `Expr::Reflect` arms from `eval_const` and the `state_types`
parameter; the fold degrades to runtime branches (the pre-change
behavior).