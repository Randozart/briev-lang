# Well-typed program generation + semantic oracle (2026-11-10)

Status: ACTIVE. Branch `test/welltyped-program-generation`.

## Why

The `frontend` and `typecheck` fuzz targets feed *random bytes*. Random bytes
almost never form a deep valid program, so they reach the typechecker shallowly
and cannot judge correctness. This work adds a **grammar-aware generator**: it
builds a typed expression tree and emits Briev source from it, so the tree is
both the generator and the oracle.

## Design (`src/typed_program.rs`)

A program is emitted from a byte cursor over a small typed grammar:

- `Int` expressions: literals, parameters, a `let x` local, `+ - * / %`, unary
  `-`, `pick(cond,a,b)`, `match` on `Int`, `List<Int>` indexing, and `match` on
  an `enum E { A, B, C }` (qualified `E.A` patterns — a bare `A` is a
  catch-all binding, verified against the typechecker).
- `Bool` expressions for guards: literals, comparisons, `&& || !`.
- `Float` arithmetic in a second defn `g`.

Each generated program must:

1. **typecheck** — it is well-typed by construction, so a rejection is a
   false-negative in the typechecker;
2. **interpret** in the reference interpreter; and
3. **agree with the model**, on both `Int` and `Float` results.

Bounds (literals `[-8,8]`, args `[-20,20]`, depth ≤ 3, nonzero divisor
literals, exactly-representable floats) keep a disagreement actionable rather
than an overflow, trap, or rounding artifact.

The same module is the adapter for the `welltyped` fuzz target
(`fuzz/fuzz_targets/welltyped.rs`), and its unit test runs 600 deterministic
random programs.

`src/typed_program.rs` is gated on `cfg(any(test, feature = "fuzzing"))`: the
default build excludes it, `cargo test --lib` covers it, and the `fuzz/`
workspace enables the `fuzzing` feature to reach it (the sibling `fuzzing`
modules need `proptest`, a dev-dependency, so they cannot be un-gated).

## Findings

- **Interpreter negated a Float literal to `Int(0)`** — `-1.5` evaluated to
  `Int(0)` (`(0.0 - 1.5)` was correct). Root cause: `eval_unary_op`'s `Neg`
  used `as_i64().unwrap_or(0)`, and `as_i64()` is `None` for a `Float`. Fixed;
  see `BUGS.md` and `tests`/`negative_float_literal_is_a_float`.
- One generator bug was caught by the oracle itself: bare enum patterns are
  catch-all bindings, so the generator must emit qualified `E.A`. Recorded
  because it is a real trap for anyone writing Briev `match` over an enum.

## Gates

- `cargo test --lib` — 3577 pass
- `typed_program` — 4 pass (600 random programs, ints + floats)
- `deterministic_transitions` family 14 (native enum) — reference trace + 6
  native cases agree at `-O0`/`-O3`
- `welltyped` fuzz campaign — see the campaign log

## Native differential extension

`tests/deterministic_transitions.rs` gains **family 14**: it constructs
`Mode.Active(e)` / `Mode.Idle` and `match`es it, returning `Int`. This is the
first native-differential family beyond `Int`/`List`/`Int-match`, so native
enum (Sum) layout and matching are compared against the interpreter at `-O0`
and `-O3`.

## Gaps / next

- The generator does not yet emit: multi-node/reactor programs, `foreach`,
  strings, generics, structs, or the native backend path. The oracle here is
  interpreter-vs-model, not interpreter-vs-native.
- Native differential still covers only `Int`/`List`/`Int-match`/`enum`;
  **float** and **string** native families remain (they need a non-Int trace
  channel).
