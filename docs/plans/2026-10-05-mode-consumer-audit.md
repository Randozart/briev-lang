# Phase 0.6(c) — `mode` consumer audit

**2026-10-05.** Output of the promotion sweep item (c): audit every consumer
of the `mode` construct and classify — promote to core, or keep
electronics-owned. Timestamped record; corrections append.

## The construct

`mode name { <law facts> }` inside a component type body (SPEC §8 region,
2026-09-24): mutually exclusive operating states. Mode bodies are ordinary
component-law equations; each declared mode is an explicit state candidate;
a node precondition selects modes with Boolean member sugar (`sw1.closed`);
postconditions are proved only in states satisfying the precondition.

## Consumers (complete, verified 2026-10-05)

| Site | Role |
|---|---|
| `src/parser/definitions.rs` (`parse_mode_decl`, contextual keyword) | parses on EVERY surface — one grammar, no extension gate |
| `src/ast/top.rs` (`ModeDecl`, `TypeDefBody.modes`) | AST storage |
| `src/analysis/electronics_laws.rs` (:237–:351) | the ONLY semantic consumer: mode facts join the when-law fact set, the bistable/state-space enumeration, node-precondition mode selection, unknown-mode errors |
| `src/analysis/electronics.rs` (:1029, :1576, :1610) | `TypeInfo.modes` (names), `assigned_modes` (solved states) |

No consumer exists outside the electronics analysis; no `.bv`/`.abv`/`.rbv`
program uses `mode` (repo-wide grep). The interpreter and every backend
never see a `ModeDecl` — the analysis consumes them before emission.

## Classification: **keep electronics-owned**

- The syntax is already core-legal (contextual, one grammar) — nothing to
  promote grammatically.
- The semantics (fact propagation, state-space solving, mode-selection
  proof obligations) live entirely in the electronics laws engine. There is
  no second consumer to serve, and promoting without a consumer would be
  speculative generality — the derivation-not-recognition rule applied to
  language surface: admit on filed need, not on plausibility.
- The promotion path is named for when the need arrives: mode facts are
  already shaped like general type-state constraints (TypeDef-body facts +
  node-precondition selection). A non-electronics consumer (a reactor
  type with mutually exclusive operating states) promotes by moving the
  fact-propagation and selection obligations into a surface-neutral
  analysis — the electronics engine becomes one consumer of it, exactly
  like the casting graph serves every backend.

## Cross-reference

- Quantity literals (`250mA`) share the electronics-pipeline admission
  today (pipeline.rs:1015) — their promotion (Time dimension, all-surface
  literals) is Phase 0.6(b), tracked separately.
- The stretch-graded register (SPEC §3.6) already grades this boundary:
  `.ebv` surface-owned analysis, core-legal syntax.
