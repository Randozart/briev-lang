# attr — Consumer Metadata Vocabulary Reference

Complete reference for every non-physical consumer-metadata key (`attr`),
its values, semantics, and which owner/backends honor it. Physical layout is
`spec`; `!>` is removed (2026-11-08). Offload policy (`accel`,
`accel_report`) is a compiler mode, not a key.

The machine-readable source of truth is `config/meta-vocab.dbv` (Data Briev format).
This document is a human-readable rendering. When adding a new key, update both.

## Arithmetic Semantics

| Key | Values | Description | LLVM | Webstack | CIRCT | MCMC |
|-----|--------|-------------|------|----------|-------|------|
| `overflow` | wrapping, checked, saturating | Integer overflow behavior | nuw nsw | implicit | comb.add flags | algebraic identities |
| `associative` | true, false | **Retired author surface (2026-11-08):** numeric intent is the `approx` keyword; this row survives only as internal lowering. May reassociate operations | reassoc | reorder | tree balance | reassociation mutations |
| `commutative` | true, false | May swap operands | ignored | ignored | ignored | commutative swap mutations |
| `fp_contract` | fast, strict | May form FMA | contract | fma fuse | fma | fma fusion mutations |
| `fp_math` | ieee754, fast | **Retired author surface (2026-11-08):** `approx`/`exact` are the source surface; this row survives only as internal lowering for instruction fast-math flags. Floating-point compliance | fast-math flags | no subnormals | relaxed | relaxed equivalence |

## Memory Semantics

| Key | Values | Description | LLVM | Webstack | CIRCT | MCMC |
|-----|--------|-------------|------|----------|-------|------|
| `readonly` | true, false | Function has no observable side effects | readonly | — | — | skips side-effect checks |
| `alloc_scope` | heap, stack | Where to allocate memory | allockind | stack_allocation | — | — |

## Code Generation Hints

| Key | Values | Description | LLVM | Webstack | CIRCT | MCMC |
|-----|--------|-------------|------|----------|-------|------|
| `inline_hint` | always, never, hint | Inlining suggestion | alwaysinline/noinline | — | — | — |
| `convergence` | tight, loose | Hardware convergence mode | — | — | single_cycle | — |
| `unroll_hint` | Int | Loop unroll factor hint | unroll | — | unroll_factor | — |

## MCMC Search Configuration

| Key | Values | Description | Effect on MCMC |
|-----|--------|-------------|----------------|
| `search_space` | linear, bitwise, all | Restrict mutation grammar | `linear` → only change_operator/swap/fold; `bitwise` → only subtree/operator/fold |
| `cost_model` | latency, throughput, size | Which cost function to use | Selects CostFn variant for performance evaluation |
| `tolerance` | Float | FP equivalence tolerance | Sets epsilon for `values_within_tolerance` during equivalence checking |
| `allowed_mutations` | List of Strings | Only apply named mutations | Restricts `apply_random_mutation` to listed operators |

## Backend Support Matrix

| Key | LLVM | Webstack | CIRCT | MCMC |
|-----|------|----------|-------|------|
| overflow | ✓ | — | comb.add | — |
| associative | ✓ | — | — | ✓ |
| commutative | — | — | — | ✓ |
| fp_contract | ✓ | ✓ | — | ✓ |
| fp_math | ✓ | — | — | ✓ |
| readonly | ✓ | — | — | ✓ |
| alloc_scope | ✓ | ✓ | — | — |
| inline_hint | ✓ | — | — | — |
| convergence | — | — | ✓ | — |
| unroll_hint | ✓ | — | ✓ | — |
| search_space | — | — | — | ✓ |
| cost_model | — | — | — | ✓ |
| tolerance | — | — | — | ✓ |
| allowed_mutations | — | — | — | ✓ |

## Adding a New Key

1. Add a new `as MetaField { key: Type; "description" }` entry in `config/meta-vocab.dbv`
2. Add backend mapping entries in `as BackendMapping { ... }` for each backend
3. Update this document with the new row
4. Implement backend codegen in the relevant backend module

## Mapping Rules

The `config/meta-vocab.dbv` file defines `BackendMapping` entries that map
`(metadata_key, value_pattern)` pairs to backend-specific IR attributes.
Patterns may be literal values (`"wrapping"`, `"fast"`) or wildcards (`"*"`)
that match any value (used for `unroll_hint` where the value is dynamic).
