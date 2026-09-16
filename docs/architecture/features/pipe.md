# Pipe Chaining (`|>`, `.N|>`)

**Date added:** 2026-06-22  
**Phase:** Expression-level desugaring  
**Status:** **Removed — superseded by universal chaining (2026-09-16)**

> The `|>` pipe token was removed (language audit, 2026-08-05). Its
> positional-history concept lives on as **universal chaining**
> (`docs/architecture/universal-chaining.md`): `.N>>`/`.name>>` back-references
> and `expr >> name` captures under the dot. This page is kept as historical
> reference for the removed `|>` design.

## Syntax

Briev supports pipe chaining as a syntactic sugar that desugars to flat
let-bindings before typechecking. All forms use the intrinsic system for
output — no `frgn` declarations needed:

```briev
x |> f()              // Call f(x) — pipeline value is first arg
x |> f() |> g()       // g(f(x)) — multi-step chain
x |> f() .|> g()      // .|> reads from 1 position back (skip=1)
x |> f() ..|> g()     // reads from 2 positions back (skip=2)
x |> f() .2|> g()     // reads from 2 positions back (explicit integer)
x |> f() .5|> g()     // reads from 5 positions back
x |> f                // auto-wrapped: f(x)
f() |> g()            // pipeline can start with a function call
```

### Dot-skip variants

| Syntax | Skip | Reads from |
|--------|------|------------|
| `\|>` | 0 | Immediately preceding result (position N-1) |
| `.\|>` / `.1\|>` | 1 | Position N-2 (one back from adjacent) |
| `..\|>` / `.2\|>` | 2 | Position N-3 (two back from adjacent) |
| `.N\|>` | N | Position N-1-N (N back) |

### Semantics

The dot-skip operator `.N|>` causes the step to receive the value from
N positions further back in the pipeline history. For example:

```briev
a |> f() |> g() .|> h()
// Step 1: f(a)              — position 1
// Step 2: g(f(a))            — position 2
// Step 3 (.|>): h(f(a))      — h receives f(a), as if it were in g's position
```

### Skip overflow

A skip that exceeds the pipeline position is a compile-time error caught
by the desugarer assertion. For example, `3 |> square() .2|> double()`
is invalid because only 1 command precedes the `.2|>`. Use `.1|>` instead,
or add more preceding steps.

The pipeline value is always prepended as the **first argument** to the
target function. Existing arguments follow.

## Desugaring Model

Pipe chains are fully desugared at parse time — they never reach the
typechecker, interpreter, or any backend.

`x |> f() |> g() .|> h()` desugars to:

```briev
{
    let __pipe_0 = x;
    let __pipe_1 = f(__pipe_0);       // step 0, reads __pipe_0
    let __pipe_2 = g(__pipe_1);       // step 1, reads __pipe_1
    let __pipe_3 = h(__pipe_1);       // step 2 (skip=1), reads __pipe_{3-1-1}
    __pipe_3
}
```

The formula for the read index at step `pos` (1-indexed) with skip `s`:
`read_idx = pos - 1 - s`

### Bare identifier auto-wrap

If the target is a bare `Identifier` (not a call), it is auto-wrapped:
`x |> f` → `Call("f", [pipeline_value])`.

## Typechecking

No typechecking is performed on the pipe chain itself — it is desugared
into let-bindings with calls before the typechecker runs. The typechecker
sees only the desugared form.

## Evaluation

Same as above — the interpreter sees only the flat let-binding block.
Each let-binding creates a new binding for the `__pipe_N` variable, and
the trailing expression returns the final result.

## Codegen

Zero codegen changes needed. The `Expr::PipeChain` variant is declared as
`unreachable!()` in all backend match arms since it should never survive
past desugaring.

## Implementation

Files modified:

- `src/ast.rs` — Added `PipeChain` struct, `PipeStep` struct,
  `Expr::PipeChain` variant
- `src/lexer.rs` — Added `PipeGreater` token (`|>`)
- `src/parser.rs` — Added `parse_pipe_chain` at lowest precedence
  (wrapping `parse_or`). Dot-skip detection: `.|>` checks peek for
  `PipeGreater`, `..|>` handles `DotDot + PipeGreater`.
  `parse_postfix` modified to not consume `.` when followed by `|>`.
- `src/desugarer.rs` — `desugar_expr`/`desugar_stmt`/`desugar_toplevel`
  recursively transform all expressions. `desugar_pipe_chain` generates
  the let-binding block.
- Match arm stubs: `src/interpreter.rs`, `src/backend/llvm/mod.rs`,
  `src/analysis/dependency_graph.rs`, `src/proof_engine.rs`,
  `src/symbolic.rs`

## Tests

15 tests added across parser, desugarer, and interpreter:

- **Parser:** basic pipe, chaining, dot-skip, dotdot-skip, precedence
  (add), bare identifier, function start, no-pipe passthrough
- **Desugarer:** basic pipe block, chaining, dot-skip indexing,
  auto-wrap, three-step with mixed skips
- **Interpreter (E2E):** full desugar+eval for basic, dot-skip,
  with-args, three-step, auto-wrap, function start

## `<:` Query Compatibility

Pipe chains work with `<:` subtype projections via the two-statement pattern:

```briev
let result : database { FILTER(.active); };
result |> print_rows();
```

The `let` binding produces a value, which is then piped to a function.
No special parser support is needed.

## Future Work

- Inline `let x : db { ... } |> f()` syntax would require changes to
  the let-statement parser.
- `_`/`_1`/`_2` placeholders for explicit argument routing in pipe
  targets.
