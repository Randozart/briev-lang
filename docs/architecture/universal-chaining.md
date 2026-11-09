# Universal Chaining (2026-09-16; simplified 2026-11-08)

**Status:** Implemented (parser, typechecker, interpreter, LLVM codegen)
**Plan:** `docs/plans/2026-11-08-language-surfaces-attr-and-chaining.md`
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

## Binding intermediate results

The chain keeps flowing without positional bookkeeping:

```briev
data.parse() into input;      // real local, type inferred from the call
input.validate() into valid;
input.emit(valid);

value.rawParse().(Int).clamp(0, 255);   // cast step, chain continues
```

- **`expr into name;`** desugars to `let name = expr;`. It is an ordinary
  local binding (scope, shadowing, inference apply) and works on any
  expression, not only chains.
- **`.(Type)`** casts the in-flight value; the next `.f()` applies to the
  cast value.
- **`it` names the receiver's argument position.** `a.f(x, it)` = `f(x, a)`;
  `a.f(it, x)` = `a.f(x)` (receiver remains argument 0). At most one `it`;
  absent means argument 0. It is rejected on `$` navigation and `!` plugin
  calls, whose receiver threading is explicit.
- **No positional references.** `>>`/`<<` are bitwise shifts only. The former
  `.N>>` / `.name>>` back-references, the `expr >> name` capture, the chain
  stack, and `Expr::Capture` were removed in 2026-11-08: passing an earlier
  value is written by binding it (`into`) and naming it.

## AST

- `MethodCall(Box<Expr>, String, Vec<Expr>, Option<usize>)` — receiver, name,
  arguments, analysis id.
- `PluginIntercept { name, args, type_args, receiver }` — `receiver` supports
  chained plugin calls.
- No `ChainRef`, no `Expr::Capture`.

## Implementation notes

- **Parser:** `.(Type)` parses as an `Expr::Cast` step that the postfix loop
  continues from. `into` is statement-level sugar producing `Statement::Let`
  with `ty: None`. `it` is resolved at the call site: position 0 consumes the
  placeholder (receiver stays implicit arg 0); a later position substitutes
  the receiver and emits a plain UFCS/intrinsic call.
- **Typechecker/interpreter/codegen:** a method call resolves the receiver and
  arguments normally; the receiver is the first argument (member `self` or
  UFCS arg 0), and args follow in source order. There is no chain stack to
  maintain or isolate.
- **Macro evaluator:** `eval_nav_chain` handles `MethodCall` with a `$`
  suffix by prepending the receiver to args and delegating to `eval_nav_call`
  — the intrinsic-macro registry.
- **BEAST:** serializes `MethodCall` (receiver/name/args) and
  `PluginIntercept`; chain-ref and capture encodings are removed.
