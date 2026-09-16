# Universal Chaining (2026-09-16)

**Status:** Implemented (parser, typechecker, interpreter, LLVM codegen)
**Plan:** `docs/plans/2026-09-16-universal-chaining-rules.md`
**Replaces:** the removed `|>` pipe chaining (`docs/architecture/features/pipe.md`)

## The rule

Every `.`-suffixed call — method, `#` intrinsic, `$` compile-time navigation,
`!` plugin, `^`/`^^` reflection — obeys one rule:

> **The right-side operation applies to the left-side result.**

The suffix is a naming convention that tells the compiler how to dispatch, not
a different syntax. The receiver is preserved as the operation's input.

| Suffix | Dispatch | Example |
|--------|----------|---------|
| (none) | member lookup → UFCS fallback | `obj.method(x)` |
| `#` | intrinsic / operation identity | `obj.Add#(x)` |
| `$` | compile-time navigation (`eval_nav_call`) | `obj.Nav$(x)` |
| `!` | plugin intercept (carries receiver) | `obj.serialize!(x)` |
| `^` / `^^` | reflection (separate tokens) | `obj.^Length`, `obj.^^Size` |

**Priority:** member → operation/intrinsic (`#`) → plugin intercept (`!`) →
UFCS fallback.

`a.Nav$(x)` and `Nav$(a, x)` are the same call. `a.serialize!(x)` and a
top-level `serialize(a, x)` share the receiver-preserving shape.

## Power features

Unix-pipe semantics unified under the dot:

```briev
data.parse() >> input          // capture result as `input`, chain continues
    .validate() >> valid
    .emit(valid, input);

variable
    .op()                      // result 1
    .2>>secondOp()             // secondOp(op_result, variable) — .2 = 2 ops back
    .step_1,1>>combined()      // combined(prev, step_1) — named + positional refs

value.rawParse().(Int)>>clamp(0, 255);   // cast then call
```

### Semantics

- **`.N` counts operations back.** `.1` = the immediately previous result
  (the same value the chain passes implicitly); `.2` = two back; `.N` = N
  back. The chain stack is `[base, r1, r2, ..., prev]`; `.N` resolves to
  `stack[len - N]`.
- **References are leading arguments.** `receiver.N>>func(a)` dispatches as
  `func(receiver, <ref>, a)` — the receiver binds `self` (member path) or arg
  0 (UFCS path), then the resolved references precede the written args.
- **`expr >> name` captures** the result into `name` for later `.name>>`
  references. The capture does not break the chain. Captures bind in the
  interpreter's `bindings` and the typechecker's `bindings` + `captures`.
- **`.(Type)>>func()`** casts the previous result to `Type` before the call.

### Disambiguation

- `>>` is a capture only at a chain position: followed by `.`, `;`, `}`, or
  end of expression. Inside an argument list (`f(x >> y)`) it stays the shift
  operator.
- `.N>>` / `.name>>` are back-references only when followed by a call head
  (`identifier (`). `.name` with any other following token is field access;
  `.N` with any other following token is tuple element access.
- **Lexing caveat:** `5.1>>f()` lexes as a float `5.1` followed by `>>` — a
  literal receiver directly before `.N>>` must be parenthesized
  (`(5).1>>f()`) or use a named expression.

## AST

- `MethodCall(Box<Expr>, String, Vec<Expr>, Option<usize>, Vec<ChainRef>)` —
  fifth field carries back-references.
- `PluginIntercept { name, args, type_args, receiver, chain_refs }` —
  `receiver` supports chained plugin calls.
- `ChainRef::Positional(usize)` / `ChainRef::Named(String)`.
- `Expr::Capture { expr, name }`.

## Implementation notes

- **Typechecker:** `chain_value_stack` walks nested MethodCall/Capture
  receivers collecting `(type, expr)` pairs; `resolve_chain_refs` resolves
  `.N`/`.name` to leading args; `resolve_method_call` takes leading exprs +
  types and validates them as the first parameters.
- **Interpreter:** `eval_chain_stack` mirrors the typechecker at runtime;
  `dispatch_method_value` receives the precomputed receiver and leading args.
- **Codegen:** `FunctionContext.chain_stack` tracks in-flight chain results
  (register, type, source expr); positional refs resolve to registers, named
  refs to capture bindings; member calls prepend leading regs; the UFCS
  fallback reconstructs leading args from the stored source exprs (matching
  the pre-existing UFCS receiver re-emission behavior).
- **Macro evaluator:** `eval_nav_chain` handles `MethodCall` with a `$`
  suffix by prepending the receiver to args and delegating to `eval_nav_call`
  — the intrinsic-macro registry.

## Remaining work

- `ChainRef` on `PluginIntercept` (back-references inside `obj.plugin!()`) —
  the field exists; plugin expansion does not yet resolve chain refs.
- **2026-09-16 fixups** (`docs/plans/2026-09-16-chaining-fixups.md`): chained
  plugins pass the receiver as the first argument; codegen chain-stack is
  isolated per call (member bodies and UFCS fallbacks no longer pollute `.N`
  back-ref indices); inline captures bind for same-chain refs; BEAST
  round-trips chain data; numeric `.N>>` without a call target errors.