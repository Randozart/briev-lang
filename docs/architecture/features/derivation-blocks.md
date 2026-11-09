# Derivation Blocks — Synthesis by Example

Date: 2026-07-29
Status: Implemented (Phases A–I)

## Overview

Derivation blocks allow defining functions by example rather than by
implementation. The compiler synthesizes a function body that satisfies
a set of input→output examples using enumerative search, SMT solving,
and stochastic superoptimization.

## Syntax

### Basic Derivation

```briev
defn add(x: Int, y: Int) -> Int := {
    2, 3 -> 5;
    0, 0 -> 0;
    10, -3 -> 7;
};
```

The `:= { ... }` block contains examples (inputs → expected output).
The compiler synthesizes a body that matches all examples.

### Reference Function

```briev
defn add_ref(x: Int, y: Int) -> Int { term x + y; };
defn add(x: Int, y: Int) -> Int := add_ref;
```

Reusing `:=` — `:= add_ref` copies the reference function's body directly.
Combined form verifies against reference:

```briev
defn popcount(x: Int) -> Int := { 0 -> 0; 1 -> 1; } := popcount_ref;
```

### Contracts — `[[post]`, `[pre][post]`, `[pre]]`

```briev
defn popcount(x: Int) -> Int := {
    0 -> 0;
} [[ #Term >= 0 && #Term < 64 ];
```

- `[[post]` = `[true][post]` — postcondition only
- `[pre][post]` — both precondition and postcondition
- `[pre]]` = `[pre][true]` — precondition only

`#Term` is a hashword referencing the function's return value.

## Pipeline (Phases A–I)

### Phase A: Tolerance Syntax
Each example can specify a tolerance for floating-point comparison:
```briev
    2.0, 3.0 -> [0.01] 5.0;
```

### Phase B: Assertion Build Gate
Every `briev build` re-verifies the synthesized body against the examples
using the interpreter. A mismatch aborts the build.

### Phase C: Enumerative Synthesis
Depth-bounded expression enumeration. Generates all valid expressions of
the target type up to `max_depth`, evaluates against examples, returns
the cheapest match. Uses cost-ordered beam search with adaptive width.

### Phase D: SMT Synthesis
Falls back to Z3 when enumerative search finds no solution. Uses
`declare-fun` + `get-model` to find any function matching the examples.
Returns ite chains (table lookups) that match the examples exactly.

### Phase E: Doppelganger Writer
Original source files are NEVER mutated. The synthesized body is written
to a shadow `.derive.bv` file, inserted before the derivation block.
The developer reviews the output, then runs `briev accept` to fold the
body back into the source.

### Phase F: MCMC Superoptimizer
Stochastic search over the expression space using Metropolis-Hastings.
Improves on the enumerative/SMT result by finding lower-cost equivalents.

### Phase G: Metadata Vocabulary
`attr` metadata annotations on functions and loops map to backend-specific
attributes through the `MetadataRegistry` (config/meta-vocab.dbv).

### Verification Loop (CEGIS)

The CEGIS loop (`synthesize()`) iterates:
1. Synthesize candidate from examples
2. Verify against postcondition / reference
3. If counterexample found → add as new example → re-synthesize
4. If proven → return candidate

Verification uses Z3 4.12+ forall quantifiers when a `[[postcondition]]`
or reference function is provided. Falls back to random-input verification
(Tier 2/3) when Z3 is unavailable.

### Overfitting Prevention

Three tiers:
1. **Identity operation pruning** — skip `0 + X`, `X * 1`, `X >> 0`, etc.
2. **Random-input verification** — test 100+ random inputs after synthesis
3. **Boundary testing** — edge cases (0, 1, -1, MAX, MIN) for each param

## Research: Abstraction Discovery

A research plan analyzing frequency-based abstraction discovery (adapted
from Koza ADFs, Polozov/Gulwani version-spaces) is in
`derivation-abstraction-discovery.md`. This approach was implemented but
found ineffective: additive registration of helpers (alongside raw
expressions) fails because helpers compete at equal cost and lose to
cheaper ite-chain solutions.

A revised plan using anti-unification with replacement (Feser et al. λ²,
PLDI 2015) is in `anti-unification-abstraction.md`. This is the proven
technique: find common sub-structure between pairs of expressions, extract
as a helper, and REPLACE the originals with calls to it — shrinking the
search space instead of growing it.

## Key Commands

- `briev derive <file>` — synthesize bodies from derivation blocks
- `briev derive --stochastic <file>` — also run MCMC superoptimizer
- `briev accept <file>` — fold `.derive.bv` bodies back into source
- `briev build <file>` — compile with assertion verification

## CLI Flags

| Flag | Purpose | Default |
|------|---------|---------|
| `--stochastic` | Run MCMC after synthesis | off |
| `--iterations N` | MCMC iterations | 10000 |
| `--temperature T` | MCMC initial temperature | 1.0 |
| `--enumerative-depth N` | Maximum search depth | 3 |
| `--verify-samples N` | Random verification samples | 50 |
| `--all` | Process all transitive imports | off |

## Implementation

| Component | File | Purpose |
|-----------|------|---------|
| Engine | `src/derive/engine.rs` | Enumerative search, cost model, expr generation |
| SMT | `src/derive/smt.rs` | Z3 query builder and response parser |
| MCMC | `src/derive/mcmc.rs` | Metropolis-Hastings superoptimizer |
| CLI | `src/derive/cli.rs` | Command handlers, flag parsing |
| Accept | `src/derive/accept.rs` | Fold doppelganger bodies into source |
| Assert | `src/derive/assert.rs` | Build-time assertion verification |
| Verify | `src/derive/verify.rs` | Random-input verification |
| Verify SMT | `src/derive/verify_smt.rs` | Z3 forall verification query |
| Doppelganger | `src/derive/doppelganger.rs` | Shadow file writer |
| Mutate | `src/derive/mutate.rs` | MCMC mutation operators |
| Equivalence | `src/derive/equivalence.rs` | Equivalence checker |
| Pareto | `src/derive/pareto.rs` | Pareto frontier for MCMC |
