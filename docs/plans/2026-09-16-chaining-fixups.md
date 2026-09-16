# Chaining Fixups

**Date:** 2026-09-16
**Branch:** fix/chaining-fixups
**Base:** main (01429a99, merge of feat/universal-chaining)
**Scope:** Fix 5 bugs found in the universal-chaining implementation.

---

## Bugs

### A — Chained plugin calls drop the receiver (high)

`obj.plugin!(x)` is accepted by the parser but:
- typechecker `src/typechecker/mod.rs:1416` uses `{ name, args, .. }` — receiver never inferred (undefined receiver not caught)
- interpreter `src/interpreter/eval.rs:266` passes only `name, args` to `eval_intercept`
- all 4 plugins (`print_plugin.rs:219`, `entry_plugin.rs:448`, `env_plugin.rs:115`, `inline_frgn_plugin.rs:147`) destructure `receiver: _`

`undefined_thing.print!(1)` compiles and prints `1`, silently discarding the receiver.

**Fix (decided): pass the receiver as the first argument (UFCS-style).**
`obj.plugin!(x)` evaluates `obj` (side effects preserved) and passes it as `args[0]`.

### B — Codegen chain-stack pollution (high)

`emit_method_call` uses a **per-function global** `chain_stack`. Member bodies run inline via `emit_member_body` and their internal chains push onto the same stack, shifting `.N` back-ref indices. The UFCS fallback re-emission also pollutes and returns without pushing its result. The interpreter builds a fresh stack per chain (`eval_chain_stack`), so `.2`+ back-refs compute different values in compiled vs interpreted code.

**Fix:** snapshot `chain_stack.len()` before `emit_member_body` / fallback re-emission, truncate after, then push the result.

### C — Inline captures don't bind (medium)

`a.b() >> x .c()` parses and codegen binds `x`, but `chain_value_stack` (`typechecker:5966`) and `eval_chain_stack` (`interpreter:1040`) recurse through `Capture` **without registering the name**. A later `.x>>` fails in the typechecker/interpreter while codegen would work.

**Fix:** register the capture name (captures+bindings / bindings) when recursing.

### D — BEAST serialization loses chain data (medium)

`src/beast/serialize.rs:256` ignores MethodCall's 5th field; `Capture` falls to the Debug catch-all. BEASTPACK round-trips silently drop `ChainRef`/`Capture`.

**Fix:** serialize/deserialize `chain_refs`, `Capture`, and `PluginIntercept`.

### E — Back-ref targets limited to bare calls (low)

`peek_is_call_head` requires `identifier (`. `.2>>obj.method()` and `.2>>f<T>(x)` silently misparse as tuple-field + shift.

**Fix:** `.N`/`.name` followed by `>>` is an unconditional back-ref; a missing direct call head is a clear error (preserve tuple-shift via parens).

### F — Liveness misses UFCS calls to top-level defns (pre-existing)

`defn_liveness` only roots `Expr::Call` edges; a MethodCall to a top-level defn
via UFCS (`a.f(x)` → `f(a, x)`) is not rooted, so the defn is eliminated even
though codegen calls it. Found while testing Bug B (`.2>>pick()`).

**Fix:** `self.mark(name)` / `out.push(name)` in the MethodCall arms of the
liveness walker — a no-op for genuine member names (they are not defns/txns).

---

## Files

| Bug | File | Change |
|-----|------|--------|
| A | `src/typechecker/mod.rs` | infer receiver in PluginIntercept handler |
| A | `src/interpreter/eval.rs` | eval receiver, prepend to args |
| A | `src/plugin/print_plugin.rs` | walk receiver, prepend, print all legacy args |
| A | `src/plugin/entry_plugin.rs` | walk receiver, prepend |
| A | `src/plugin/env_plugin.rs` | walk receiver, prepend |
| A | `src/plugin/inline_frgn_plugin.rs` | walk receiver, prepend |
| B | `src/backend/llvm/emit_expr.rs` | snapshot/truncate chain_stack around member body + fallbacks |
| C | `src/typechecker/mod.rs` | chain_value_stack Capture registers name |
| C | `src/interpreter/eval.rs` | eval_chain_stack Capture binds name |
| D | `src/beast/serialize.rs` | serialize ChainRef/Capture/Plugin |
| D | `src/beast/deserialize.rs` | deserialize them |
| E | `src/parser/expressions.rs` | back-ref unconditional + error on missing call head |
| F | `src/analysis/defn_liveness.rs` | root UFCS MethodCall names |

## Tests

- A: interpreter + typechecker chained-plugin-receiver test
- B: codegen test — member body with internal chain + `.2>>` ref equals interpreter
- C: interpreter + typechecker inline-capture test
- D: BEAST round-trip for ChainRef + Capture
- E: parser `.2>>d` errors; `.2>>f()` works

## Verification

- `cargo test --lib` green
- praetor on changed directories
- No hot-loop codegen path changed (chain-stack truncate is O(1) bookkeeping)