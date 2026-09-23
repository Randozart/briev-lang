# Unified Metaprogramming Layer — Variadic Composites, then One Compute-and-Emit Function Kind

**2026-09-22.** The metaprogramming surface is three bolted-on mechanisms —
`$(Stage)` blocks + `$` intrinsics (AST mutation, `NavValue`), `$defn`/`$txn`
stage functions (value-returning, `NavValue`), and `$defn` composites
(statement-splicing, `ComptimeVal`) — with two value domains and split
routing. This plan unifies them into ONE coherent model: a compile-time
function that computes values AND emits code, with TypeScript-style rest
params as the sanctioned compile-time iteration channel, and composites
firing everywhere (statement, expression, top level → topology emission).

## The missing thing (why this matters)

A composite must be able to **iterate over a runtime-uncertain list of
arguments and emit code per element** — one body, ordinary loops, proofs
carried through every expansion. `execute_many!(f(a), f(b), f(c))` is the
concrete need: an uncertain number of args, one sequential call emitted per
arg, looping N times at compile time.

The current composite model deliberately refuses to unroll over
caller-provided lists ("caller spans are POLICY, not structure") — correct
for VALUE params, wrong for the rest-param case. The principled resolution:
**rest params are the sanctioned compile-time iteration channel.** A plain
`Expr` param is a runtime quantity (never unrolls); a `...rest` param
declares "this trailing argument list is meant to be iterated at compile
time" — a `foreach` over it unrolls, splicing one emission per element.

## Syntax — TypeScript-style rest param

```briev
$defn execute_many(...calls: Expr) {
    foreach c in calls { c; }   // execute_many!(f(a), f(b), f(c)) → f(a); f(b); f(c);
};
```

- `Token::Ellipsis` already exists and is only consumed inside `[]`
  (full-range slice `a[...]`) — zero ambiguity in a parameter list.
- `...` must be the FINAL parameter (rest binds all trailing args).
- Compile-time-only: a runtime `defn` declaring `...` is an error.
- Each arg is a sequential call: `execute_many!(f(a), f(b))` emits
  `f(a); f(b);` — the full-call shape, body = the emission.

## Phase 1 — variadic composites + `execute_many!` (landable now)

### Parser
- `parse_parameter_list` (`definitions.rs:1629`): consume a leading
  `Token::Ellipsis`; the rest name is parsed like any param.
- The `$defn`/`$txn` paths record the rest name; a RUNTIME `defn` with `...`
  is rejected ("rest params are compile-time-only — use `$defn`").
- A non-final `...` is an error.

### AST
- `Definition` gains `variadic_param: Option<String>` (the rest name).
  `None` = not variadic. Mirrors `ForeignBinding.is_variadic`.

### Composite expansion (`src/plugin/composite.rs`)
- `composite_signature`: the rest param is EXCLUDED from the fixed
  substitution list; it captures `args[fixed_count..]`.
- `expand_composite_invocation`: arity check becomes `args.len() >=
  fixed_params.len()`; the rest list = trailing args. Seed the fold env:
  `calls → ComptimeVal::List([arg1, arg2, ...])`. A body
  `foreach c in calls { c; }` unrolls via the EXISTING `comptime_iters`
  Identifier→List path (`composite.rs:561-564`) → one `c;` per arg.
- `substitute_param`/`subst_expr`: skip the rest name (it is a list, not a
  single expression).

### Hygiene
- The rest param joins the `exposed` set in `check_hygiene` (like
  `ExprItem`) — the body's `c` binder is loop-local, not a capture.

### `subst_expr` — PluginIntercept arm
- `subst_expr` has no `Expr::PluginIntercept` arm today — a composite
  parameter inside `execute_many!(f, (x), (x+1))` never substitutes. Add an
  arm descending into `args` (fixes the execute_many-in-composite dead end
  + enables nested composites).

### Diagnostics (the two opaque dead-ends)
- Composite called without `!` → "call `f!(...)`" hint (was: silent LLVM
  undefined-symbol error).
- `!` on a non-composite/unknown name → "`name` is not a compile-time
  macro/composite" (was: `unresolved plugin-intercept reached the
  typechecker`).
- Leftover unexpanded `PluginIntercept` gets a distinct message naming the
  likely author error (composite in expression position, etc.).

### Retire `execute_many_plugin.rs` (2026-09-18 Rust macro)
- Superseded by the stdlib composite `execute_many`. Move its tests to the
  composite; delete the plugin, its registration (pipeline.rs), and its doc
  section. Rule 14: stdlib learns, Rust retires.
- Stdlib: `lib/std/meta.bv` (or `execute_many.bv`) declares
  `$defn execute_many(...calls: Expr) { foreach c in calls { c; } };`.

### Tests + corpus
- Arity edges (0 args → reject as a mistake, per the retired plugin's own
  contract; 1 arg; N args), nested variadic calls, multi-instantiation env
  isolation.
- `examples/syntax/meta/execute-many.bv` (conformance-enforced).

## Phase 2 — unification (intermediate commits)

- **C2 — value-returning composites — DONE 2026-09-22**: `term v`
  alpha-renamed, wrapped in `Expr::Block` → `let r = f!(x)` works; never
  leaks into the caller's node. `expand_composite_body` is the shared core;
  `expand_composite_value` wraps the folded body in a block whose trailing
  `term v` becomes `Statement::Expression(v)` — the block types as the value
  (typechecker now types a block ending in `Expression(e)` as `e`'s type,
  matching interpreter+backend). `expand_expr_values` walks expression
  positions (let inits, assign RHS, call args, match scrutinee/arms) and
  expands value composites depth-first. Side fix (soundness-net catch,
  pre-existing): cold-outline guard functions now thread `ptr %state` — the
  outline referenced `%state` via observable intrinsics (Print#) with no
  state param; and the liveness table roots `briev_await_impl`,
  `briev_task_spawn_impl`, `briev_task_cancel_impl` at their constructs —
  async-tasks.bv now compiles (was a hard liveness panic).
- **C3 — one value domain**: reconcile `NavValue` (stage fns) and
  `ComptimeVal` (composites); a body computes (`let x = list`) AND emits
  (`foreach over x`) in one pass.
- **C4 — uniform AST walker**: one visitor over every statement-bearing
  `TopLevel` + nested/expression positions (replaces the hand-rolled
  `expand_stmt_list`/`expand_nested` pair).
- **C5 — topology emission**: composites may emit `async node`/`sync<g>
  node` — the schedule-generation purpose, Rule-22 classification carried in
  the language, not the compiler.
- **C6 — `$txn` resolution**: adopt `$txn` as the convergent-loop flavor
  (repeat-until-postcondition — the ONE semantic `$defn` lacks), fix its
  parse-broken inline form, add real tests; OR delete it. Decided once
  `foreach`-over-rest is proven.

## Verification
- `cargo test --lib` green per commit; conformance sweep; Praetor on changed
  files (≤ 15 complexity, ≤ 100 lines, ≤ 6 params).
- Corpus `execute-many.bv` enforced by the sweep.

## Documentation
- SPEC §18 updated for rest params + composite semantics (same commit as
  Phase 1 code).
- `docs/architecture/macro-system.md` — `$defn`/`$txn`/composite section
  rewritten to the unified model.
- `docs/plans/2026-09-20-metaprogrammed-composites.md` — mark Front A
  variadic + composite-everywhere landed.

## Undo
- Rest-param parse: remove the `Ellipsis` arm from `parse_parameter_list`;
  `variadic_param` stays `None` everywhere.
- Composite rest seeding: revert to the strict `args.len() == params.len()`
  arity check.
- Plugin retirement: restore `execute_many_plugin.rs` from git history.