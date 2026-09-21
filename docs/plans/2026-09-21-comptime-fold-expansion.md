# Comptime fold in composite expansion (shape-adaptive declared composites)

**2026-09-21.** Follows `2026-09-20-metaprogrammed-composites.md` (Fronts A-D).
Front C resolved by dissolution (`0aa69a9e`, `dd70f144`); this plan implements
the layer that makes the *declared* composite shape-adaptive without giving
the compiler any shape knowledge.

## Problem

Composites expand one fixed body regardless of arguments. `softmax_fused!`
always emits the deferred 3-pass structure; a small-span caller
(`dlen <= 32`, everything register-resident) wants the ONLINE one-pass
structure instead. Today that choice needs a second declared composite —
the policy lives with the compiler's users, duplicated per shape.

The analysis chain (purity proof, M2, deferred-region matcher, schedule) is
strictly intra-procedural: it sees statements, never calls. That is why
expansion-before-typecheck is load-bearing. The same fact makes
**expansion-time branching** the right injection point: whatever the
expansion splices, the existing proofs handle unchanged.

## Design decision: the language is the metaprogramming layer

Rejected: `is_const(...)`, `array_dim(...)` builtins, `if`/`else` special
forms. They violate Golden Rule 3 (function-shaped hidden compiler
knowledge, no disclosure marker), Rule 14 (new Rust match arms instead of
language/stdlib), and duplicate two mechanisms the language already has:

- **comptime evaluation**: `$let`/`$const` (`TopLevel::CompileTimeLet`),
  `evaluate_pending_comptime`, `comptime_vars`, `NavValue`,
  `eval_expr` (`src/macros/eval.rs`) — runs pre-expansion in all three
  pipeline paths.
- **reflection**: `.^^Element` (implemented), `.^^Size` (compile-time vector
  shape), `.^Length` — disclosed forms, resolved in the typechecker
  (`resolve_reflect`).

Briev has no `if`/`else` (SPEC §11): two-sided branching is `match`
(exhaustive, `Pattern::Literal` includes Bool), one-sided guards are
`when`. Exhaustiveness makes `match` the natural fold target: both futures
must be written, the fold picks one.

## The feature, precisely

After parameter substitution (existing `expand_composite_invocation`), a
new **fold pass** walks the expanded statements:

1. A `let` whose init evaluates (existing `eval_expr`, on the fold env +
   comptime-known substitution args) binds its `NavValue` into the fold env.
2. A `match` whose scrutinee evaluates to a constant: splice **only the
   taken arm's statements**, recursively folded.
3. A `when` whose condition evaluates to `true`: splice its block,
   recursively folded; `false`: splice nothing.
4. Everything else is copied verbatim.

**Fail-open degradation**: a non-foldable scrutinee/condition (runtime
value, or references `expr_item` binders) stays an ordinary runtime
`match`/`when` — semantics identical to hand-written code, just not
specialized. No errors, no special forms; the same rule as any Briev
conditional. Shape adaptivity *emerges* when the caller passes comptime
spans (the m3 template already passes `4096, 128` as literals).

**Recursive**: fold applies to statements spliced from taken arms (nested
adaptivity — a tile-size `match` inside the large-span arm). One traversal
either way.

**Hygiene unchanged**: fold reads values, binds no names into user scopes;
the fold env is internal to one expansion and dies with it.

## What it buys (the doctrine test)

One declaration covers all shapes; the branch policy lives in
`lib/std/numeric.bv`; the compiler stays algorithm-blind; general passes
(M2, deferred-region, coalescing) handle whichever ordinary body emerges.
Delete stdlib, write the year-two algorithm with the same `match`, get the
same treatment — Golden Rule 24 with the temporal side holding the policy.

Power positioning (vs C++/CUDA) recorded in session discussion: expression
params are strictly wider than C++ NTTPs (`q[h*128+d]` splices directly);
comptime `match` ≈ `if constexpr`; per-instantiation contract Gates and the
proof layer (M2 deferral, disjointness) have **no CUDA analog**; comptime
computation is initially a subset of `constexpr` (no comptime loops —
additive trajectory via `eval_expr`).

## Phase 1 (this plan)

- Fold pass in `plugin/composite.rs`, consuming `eval_expr` from
  `macros/eval.rs` (reuse, not fork). Composite bodies may use `let`,
  `match`, `when`; Int/Bool/Float/Str literals fold.
- Adaptive `softmax_fused!`: `match dlen <= 32` → online arm (running max +
  rescale, single sweep) / deferred arm (today's body verbatim).
- Fixture gate: adaptive composite at two spans — small (`dlen=16` → online
  body, numeric PASS) and gate geometry (`dlen=128` → deferred body,
  numeric PASS, CUDA lane ≥ 2.93e-06-class accuracy). Degradation fixture:
  runtime `dlen` → both arms present as runtime `match`, numeric PASS.
- Unit tests: fold, degradation, recursive arms, comptime lets, hygiene,
  `when` both polarities.

## Phase 2 (boundary, NOT this plan)

- Early reflection resolution: `buf.^^Size` / `.^^Element` conditions in
  composite bodies resolved from `ProgramInfo::array_types` (state decls,
  post-parse) before typecheck. Narrow allowlist served through the
  existing reflection forms — additive, disclosed.
- Convergence: `plugin/composite.rs` expansion lowers onto the same
  machinery as `$let` evaluation where duplication appears (Golden Rule 3,
  accidental complexity). Follow-up unless the fold pass makes it
  immediate.

## Non-goals

- No comptime loops/aggregates (subset of `constexpr`; grows later).
- No type-level computation in composites (generics `<>` own that quadrant).
- No fold requirements (never fail-closed on non-foldable conditions).
- No compiler knowledge of any algorithm; no Tier-2 matchers added.
