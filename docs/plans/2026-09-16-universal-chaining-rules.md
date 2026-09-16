# Universal Chaining Rules

**Date:** 2026-09-16
**Branch:** feat/universal-chaining
**Scope:** Syntax-level redesign of function chaining + power features

---

## 0. Implementation Status (2026-09-16)

All layers implemented and tested (`cargo test --lib`: 2237 passing).

### Delivered

| Feature | Status |
|---------|--------|
| `!` plugin chaining (`obj.plugin!(x)`) | parser + AST (`PluginIntercept.receiver`) + typechecker + plugin walks |
| `$` nav-chain unified under `MethodCall` | parser + macro evaluator (`eval_nav_chain` handles `MethodCall` with `$` suffix) |
| `.N>>func()` positional back-reference | parser + typechecker + interpreter + LLVM codegen |
| `.name>>func()` named capture reference | parser + typechecker + interpreter + LLVM codegen |
| `expr >> name;` capture statement | parser (postfix capture) + typechecker + interpreter + codegen |
| `.(Type)>>func()` cast annotation | parser |

### Resolved Semantics (from implementation)

- **Back-reference counts operations back**: `.1` = the immediately previous
  result (which is also the receiver), `.2` = two back, `.N` = N back. The
  chain stack is `[base, r1, r2, ..., prev]`; `.N` resolves to
  `stack[len - N]`.
- **References are leading args**: `receiver.N>>func(a, b)` dispatches as
  `func(receiver, <ref>, a, b)` — the receiver binds `self` (member path) or
  arg 0 (UFCS path), then each resolved reference precedes the written args.
- **Named captures bind in the interpreter's `bindings` and the typechecker's
  `bindings`+`captures`**, so both `.name>>` and a plain identifier reference
  resolve.
- **`>>` disambiguation**: `expr >> name` is a capture only when followed by
  `.`, `;`, `}`, or EOF (chain position). Inside an argument list
  (`f(x >> y)`) it stays a shift. `.N>>`/`.name>>` are back-references only
  when followed by a call head (`identifier (`).
- **Lexing caveat**: `5.1>>f()` lexes as a float `5.1` followed by `>>` — a
  literal receiver before `.N>>` must be parenthesized or used via a
  non-literal expression.

### Not Yet Delivered

- `ChainRef` on `PluginIntercept` (`obj.plugin!()` with back-refs) — the
  field exists but plugin expansion does not yet resolve chain refs.
- `ChainRef::Named` runtime lookup in codegen for the UFCS fallback uses the
  capture binding (correct); positional refs in the UFCS fallback re-emit the
  receiver chain (matches the pre-existing UFCS double-emission behavior).

---

## 1. Mental Model

**Dot is member access first, UFCS fallback. Suffixes are naming conventions
that tell the compiler the role of the call.**

```
expr.suffix_name(args)
```

| Suffix | Role | Dispatch |
|--------|------|----------|
| (none) | regular member or function | member lookup → UFCS fallback |
| `#` | intrinsic / operation | trim `#`, member → intrinsic table |
| `$` | compile-time metaprogramming | trim `$`, `eval_nav_call()` intrinsic registry |
| `!` | plugin / macro | trim `!`, plugin system |
| `^` / `^^` | reflection | separate tokens (`DotCaret`/`DotCaretCaret`) |

**Programmer's rule:** "Dot means apply the right-side operation to the
left-side result. The suffix tells the compiler how to find the implementation."

**Compiler's rule:** For any `MethodCall(recv, name, args)`:
1. No suffix → member lookup, then UFCS fallback
2. `#` suffix → trim `#`, try member, then intrinsic table (via `infer_call`)
3. `$` suffix → trim `$`, dispatch through `eval_nav_call()` (intrinsic macro
   registry: `Tag$`, `Named$`, `First$`, `All$`, `Insert$`, etc.)
4. `!` suffix → trim `!`, plugin system
5. `^`/`^^` → reflection resolution (separate tokens, not suffixes)

**$(Stage) block convention:** `Tag$("defn")` and `x.Tag$("defn")` are two
syntaxes for the same thing. Bare calls resolve through `fn_registry` (`$defn`/
`$txn`) then regular `defn`/`txn`. Dot calls prepend the receiver as the first
argument and dispatch through `eval_nav_call()`.

---

## 2. Syntax Specification

### 2.1 Basic chaining (unchanged)

```briev
obj.method(x)             // MethodCall(obj, "method", [x])
obj.OpName#(x)            // MethodCall(obj, "OpName#", [x])
obj.Nav$(x)               // MethodCall(obj, "Nav$", [x])
obj.serialize!()          // PluginIntercept { name: "serialize", receiver: Some(obj) }
obj.^^Size                // Reflect(obj, "Size", CompileTime)
obj.^Length               // Reflect(obj, "Length", Runtime)
```

### 2.2 Capture operator (statement-level)

```briev
expr >> capture_name;
```

Captures the result of `expr` into `capture_name`. Available for reference
in subsequent chain operations within the same statement.

```briev
list.append(5) >> step_1;
step_1.filter(x => x > 3).count()   // uses captured step_1
```

**Parsing:** After an expression, if `Shr` + `Identifier` + `Semicolon`
(or block boundary), treat as capture statement. Otherwise `>>` is
bitwise shift-right.

### 2.3 Positional back-references (dot-postfix)

```briev
.N>>func()        // pass result from N operations back as first arg
.name>>func()     // pass named capture as first arg
```

**Rules:**
- `.N` counts operations back (`.1` = previous, `.2` = two back)
- `.1` is implicit — bare `func()` ≡ `.1>>func()`
- `.name>>` references a capture from a previous `>> name` statement
- `.name` followed by anything other than `>>` is always field access
- References are leading args: `.ref1,ref2>>func(a,b)` → `func(ref1, ref2, a, b)`
- No anonymous captures — every capture needs a name

**Position counting is absolute** (counts operations, not captures):

```briev
variable                // result_0
.op()>>a               // result_1 — captured as 'a'
.foo()                 // result_2
.3>>bar()              // bar(variable) — .3 = 3 ops back = result_0
.2>>baz()              // baz(a) — .2 = 2 ops back = result_1 = 'a'
```

### 2.4 Type annotation in chains

```briev
.(Type)>>func()   // cast previous result to Type, then func(cast_result)
```

### 2.5 Combined examples

```briev
// Basic chain with capture
data.parse() >> input;
    .validate() >> valid;
    .transform() >> result;
    .emit(result, input)

// Back-references without explicit captures
variable
    .op()                        // step 1: op(variable)
    .2>>secondOp()               // step 2: secondOp(variable) — 2 ops back
    .step_1,1>>CombinedOp()      // step 3: CombinedOp(step_1, variable)

// Type annotation
value
    .rawParse()
    .(Int)>>clamp(0, 255)
    .toColor()

// Member access on captured values
list >> original;
    .filter(x => x > 3) >> filtered;
    .count()(filtered.sum())(original.len())
```

---

## 3. Implementation — Layer 1: Core Fixes

### 3.1 AST: Add `receiver` to `PluginIntercept`

**File:** `src/ast/expr.rs:150-154`

```rust
PluginIntercept {
    name: String,
    args: Vec<Expr>,
    type_args: Vec<Type>,
    receiver: Option<Box<Expr>>,  // NEW
},
```

All existing `PluginIntercept` nodes get `receiver: None`. Only chained
usage like `obj.serialize!()` sets `receiver: Some(...)`.

### 3.2 Parser: Fix `!` chaining

**File:** `src/parser/expressions.rs:388-441` (the `.` handler)

Add a branch between the `$` nav-chain check and the method-call check:

```rust
// After parsing `.` + identifier `name`:
if name.ends_with('$') && self.check(&Token::LParen) {
    // ... existing nav chain — CHANGED to produce MethodCall (see 3.3)
} else if self.check(&Token::Not) {
    // NEW: chained plugin — obj.name!(args)
    self.advance(); // consume !
    self.expect(Token::LParen)?;
    let mut p_args = Vec::new();
    if !self.check(&Token::RParen) {
        loop {
            p_args.push(self.parse_expression()?);
            if !self.eat(&Token::Comma) { break; }
        }
    }
    self.expect(Token::RParen)?;
    expr = Expr::PluginIntercept {
        name,
        args: p_args,
        type_args: vec![],
        receiver: Some(Box::new(expr)),
    };
} else if self.check(&Token::LParen) {
    // ... existing method call (unchanged)
} else {
    expr = Expr::Field(Box::new(expr), name);
}
```

**Also fix bare `!` handler** (line 527-547): add `receiver: None` to
existing `PluginIntercept` construction.

### 3.3 Parser: Unify `$` under `MethodCall`

**File:** `src/parser/expressions.rs:415-426`

Change:
```rust
// BEFORE:
expr = Expr::Call(name, args, None);

// AFTER:
expr = Expr::MethodCall(Box::new(recv), name, args, None);
```

### 3.4 Macro evaluator: Handle `MethodCall` for `$` functions

**File:** `src/macros/eval.rs` — `eval_nav_chain()`

Add a new match arm before the existing `Expr::Call` arm:

```rust
Expr::MethodCall(recv, name, args, _) if name.ends_with('$') => {
    let recv_val = eval_nav_chain(recv, program, universe, stage, scope, sandbox, pm)?;
    let mut full_args = vec![recv_val];
    for a in args {
        full_args.push(eval_nav_chain(a, program, universe, stage, scope, sandbox, pm)?);
    }
    eval_nav_call(name, &full_args, program, universe, stage, scope, sandbox, pm)
}
```

This makes `a.Nav$(x)` and `Nav$(a, x)` produce the same result inside
stage blocks.

### 3.5 Typechecker: Resolve `PluginIntercept` receiver

**File:** `src/typechecker/mod.rs:1411-1431`

```rust
Expr::PluginIntercept { name, args, receiver, .. } => {
    if let Some(recv) = receiver {
        infer_type_only(recv, ctx)?;
    }
    for a in args {
        infer_type_only(a, ctx)?;
    }
    match name.as_str() {
        // ... existing dispatch unchanged ...
    }
}
```

### 3.6 Plugin walks: Handle receiver

Every plugin walk function that matches `Expr::PluginIntercept` needs
to also walk the `receiver` field:

```rust
Expr::PluginIntercept { name, args, receiver, .. } => {
    if let Some(recv) = receiver {
        walk_expr(recv, ...)?;
    }
    for a in args {
        walk_expr(a, ...)?;
    }
    // ... existing logic ...
}
```

Files:
- `src/plugin/print_plugin.rs` — `walk_expr()` (line 219)
- `src/plugin/entry_plugin.rs` — `rewrite_expr()` (line 448)
- `src/plugin/env_plugin.rs` — walk function
- `src/plugin/inline_frgn_plugin.rs` — walk function
- `src/plugin/script_plugin.rs` — walk function

### 3.7 Mechanical updates (~25 files)

All other `Expr::PluginIntercept` match sites get `receiver: _` or
`receiver: ref recv` added. Key sites:

| File | Change |
|------|--------|
| `src/annotator.rs:220,665` | Walk receiver |
| `src/typechecker/mod.rs:2466` | Walk receiver |
| `src/backend/mod.rs:639` | Walk receiver |
| `src/backend/capabilities.rs:457` | Walk receiver |
| `src/backend/llvm/emit_expr.rs:1698` | Walk receiver |
| `src/backend/llvm/emit_toplevel.rs:3423` | Walk receiver |
| `src/backend/llvm/mod.rs:799` | Walk receiver |
| `src/backend/llvm/context.rs:684` | Walk receiver |
| `src/backend/llvm/helpers.rs:154` | Walk receiver |
| `src/analysis/gpu_schedule.rs:202` | Walk receiver |
| `src/analysis/defn_liveness.rs:524,756` | Walk receiver |
| `src/analysis/narrow_slice.rs:97` | Walk receiver |
| `src/analysis/licm.rs:128` | Walk receiver |
| `src/analysis/dependency_graph.rs:286` | Walk receiver |
| `src/analysis/dataflow.rs:153` | Walk receiver |
| `src/analysis/allocation.rs:314` | Walk receiver |
| `src/analysis/swan_song.rs:177` | Walk receiver |
| `src/symbolic.rs:282` | Walk receiver |
| `src/interpreter/eval.rs:260` | Evaluate receiver |
| `src/macros/eval.rs:2030` | Walk receiver |
| `src/ast/display.rs:144` | Display receiver |

---

## 4. Implementation — Layer 2: Power Features

### 4.1 AST: New expression variants

**File:** `src/ast/expr.rs`

Add to `Expr` enum:

```rust
/// Statement-level capture: `expr >> name;`
/// Stores the captured expression and the binding name.
Capture {
    expr: Box<Expr>,
    name: String,
},
```

**New struct:**

```rust
/// A reference to a previous chain result — positional or named.
#[derive(Debug, Clone)]
pub enum ChainRef {
    /// `.N>>` — N operations back (1-indexed, .1 = previous)
    Positional(usize),
    /// `.name>>` — reference to a named capture
    Named(String),
}
```

**Extend `MethodCall`:**

```rust
MethodCall(
    Box<Expr>,       // receiver
    String,          // name
    Vec<Expr>,       // args
    Option<usize>,   // analysis_id
    Vec<ChainRef>,   // NEW: leading references
),
```

**Extend `PluginIntercept`** (already has `receiver`, add refs):

```rust
PluginIntercept {
    name: String,
    args: Vec<Expr>,
    type_args: Vec<Type>,
    receiver: Option<Box<Expr>>,
    chain_refs: Vec<ChainRef>,  // NEW
},
```

### 4.2 Parser: Capture statement

**File:** `src/parser/statements.rs` or expression parsing

After parsing an expression statement, check for `Shr` + `Identifier`:

```rust
// In statement parsing, after parsing the expression:
if self.eat(&Token::Shr) {
    let name = self.expect_identifier()?;
    self.expect(Token::Semicolon)?;
    return Ok(Statement::Expr(Expr::Capture {
        expr: Box::new(expr),
        name,
    }));
}
```

**Disambiguation:** `>>` after an expression is capture only when followed
by `Identifier` + `Semicolon` (or block boundary). Otherwise it's a
shift-right binary operator.

### 4.3 Parser: Back-reference syntax

**File:** `src/parser/expressions.rs` — `parse_postfix()` loop

After parsing `.` + identifier/integer, before checking for `(`:

```rust
// NEW: back-reference check
if self.eat(&Token::Shr) {
    // .N>> or .name>> — back-reference
    // ... parse refs, then (args), produce MethodCall with chain_refs
}
```

**Detailed parsing flow for `.N,M>>func(args)`:**

1. Parse `.` — enter postfix loop
2. Parse `N` (integer) — store as potential back-ref position
3. See `Shr` (`>>`) — confirm back-reference
4. See `,` — more references follow
5. Parse `M` (integer or identifier) — another reference
6. After all refs, expect `(`
7. Parse `(args)` — the function call
8. Produce `MethodCall(recv, func_name, args, None, chain_refs)`

**For named refs `.name>>func()`:**

1. Parse `.` — enter postfix loop
2. Parse `name` (identifier)
3. See `Shr` (`>>`) — confirm named reference (NOT field access)
4. Parse `(args)` — the function call
5. Produce `MethodCall(recv, func_name, args, None, [ChainRef::Named(name)])`

**For `.name` without `>>`:** falls through to existing field access /
method call / plugin-intercept logic.

### 4.4 Parser: Type annotation syntax

**File:** `src/parser/expressions.rs`

After `.`, check for `(` + `Identifier` (type) + `)` + `>>`:

```rust
if self.check(&Token::LParen) {
    // Could be: (Type)>> cast or (args) method call
    // Lookahead: if LParen + Identifier + RParen + Shr → cast
    // Otherwise → method call args
    let save_pos = self.pos;
    self.advance(); // consume (
    if let Some(Token::Identifier(type_name)) = self.peek() {
        self.advance();
        if self.check(&Token::RParen) {
            self.advance();
            if self.check(&Token::Shr) {
                // Cast: .(Type)>>
                // ... parse cast + function call
            }
        }
    }
    // Reset and try as method call args
    self.pos = save_pos;
}
```

**Recommended parse:** `.(Type)>>func()` → cast the previous result to
Type, then call func with the cast result as the receiver.

### 4.5 Typechecker: Resolve chain references

**File:** `src/typechecker/mod.rs` — `resolve_method_call()`

When `chain_refs` is non-empty, resolve each reference:

```rust
for chain_ref in chain_refs {
    match chain_ref {
        ChainRef::Positional(n) => {
            // Look up the nth previous result in the chain context
            // Requires tracking a "chain stack" of previous result types
        }
        ChainRef::Named(name) => {
            // Look up the named capture in scope
        }
    }
}
```

**Chain tracking:** The typechecker maintains a stack of previous result
types as it processes chained expressions. Each `MethodCall` pushes its
return type. `ChainRef::Positional(n)` pops the nth entry.

### 4.6 Codegen: Emit chain references

**File:** `src/backend/llvm/emit_expr.rs`

For `MethodCall` with `chain_refs`:
1. Resolve each reference to its LLVM value
2. Prepend resolved values to the args list
3. Emit the call with combined args

### 4.7 Interpreter: Evaluate chain references

**File:** `src/interpreter/eval.rs`

For `MethodCall` with `chain_refs`:
1. Look up positional references in the evaluation stack
2. Look up named captures in the environment
3. Prepend to args
4. Evaluate normally

---

## 5. Files to Change

### Layer 1 (Core Fixes)

| File | Change |
|------|--------|
| `src/ast/expr.rs` | Add `receiver` to `PluginIntercept` |
| `src/parser/expressions.rs` | Fix `.` handler for `!`; change `$` to `MethodCall` |
| `src/typechecker/mod.rs` | Resolve `PluginIntercept` receiver |
| `src/macros/eval.rs` | Handle `MethodCall` for `$` functions |
| `src/plugin/print_plugin.rs` | Walk receiver |
| `src/plugin/entry_plugin.rs` | Walk receiver |
| `src/plugin/env_plugin.rs` | Walk receiver |
| `src/plugin/inline_frgn_plugin.rs` | Walk receiver |
| `src/plugin/script_plugin.rs` | Walk receiver |
| ~25 other files | Mechanical `receiver: _` in match arms |

### Layer 2 (Power Features)

| File | Change |
|------|--------|
| `src/ast/expr.rs` | Add `ChainRef`, `Capture`, extend `MethodCall` |
| `src/parser/expressions.rs` | Parse `.N>>`, `.name>>`, `.(Type)>>` |
| `src/parser/statements.rs` | Parse `>> name;` capture statement |
| `src/typechecker/mod.rs` | Resolve `ChainRef`, track chain stack |
| `src/backend/llvm/emit_expr.rs` | Emit resolved chain refs as leading args |
| `src/interpreter/eval.rs` | Evaluate chain refs |
| `src/ast/display.rs` | Display new variants |
| `learn-briev/00a-base-design.md` | Document chaining model |
| `spec/SPEC.md` | Update §11.4 |

---

## 6. Testing Strategy

### Layer 1 tests

1. `cargo test --lib` — all existing tests pass (backward compatible)
2. Parser test: `obj.plugin!(x)` chains correctly
3. Parser test: `obj.method().plugin!(x).field` — full chain parses
4. Parser test: `a.Nav$(x).Method$(y)` — unified `MethodCall` chain
5. Macro evaluator test: `a.Nav$(x)` inside `$(Stage)` produces same as `Nav$(a, x)`
6. Typechecker test: `PluginIntercept` with receiver resolves correctly
7. Plugin walk test: all plugins correctly walk receiver field

### Layer 2 tests

8. Parser test: `expr >> name;` → `Capture { expr, name }`
9. Parser test: `.2>>func()` → `MethodCall(..., chain_refs: [Positional(2)])`
10. Parser test: `.name>>func()` → `MethodCall(..., chain_refs: [Named("name")])`
11. Parser test: `.2,1>>func(a, b)` → multiple chain_refs
12. Parser test: `.(Int)>>func()` → cast + method call
13. Typechecker test: positional references resolve to correct types
14. Typechecker test: named references resolve to capture types
15. Codegen test: chain refs emit correct LLVM IR with leading args
16. Interpreter test: chain refs evaluate correctly
17. Integration test: full program with captures and back-references

### Edge case tests

18. `.name` without `>>` is always field access (not reference)
19. `.2` without `>>` is always tuple access (not back-reference)
20. `>>` inside expression is shift-right, not capture
21. Anonymous capture is rejected
22. Reference to non-existent capture is a type error
23. Position out of range is a type error

---

## 7. Migration Notes

**No source-level migration needed.**

- Layer 1: `receiver` field is `None` for all existing `PluginIntercept` nodes.
  `$` → `MethodCall` is internal AST only.
- Layer 2: New syntax is purely additive. Existing code doesn't use `>>` for
  capture or `.N>>` for back-references.

**Breaking changes:** None. All existing programs parse and compile identically.

---

## 8. Implementation Order

1. **AST changes** — `PluginIntercept.receiver`, `ChainRef`, `Capture`, extended `MethodCall`
2. **Parser: core fixes** — `!` chaining, `$` unification
3. **Parser: power features** — `.N>>`, `.name>>`, `.(Type)>>`, `>> name;`
4. **Typechecker** — receiver resolution, chain reference resolution
5. **Macro evaluator** — `MethodCall` for `$` functions
6. **Plugin walks** — receiver field in all plugin walk functions
7. **Mechanical updates** — `receiver: _` in ~25 match sites
8. **Codegen** — emit chain refs as leading args
9. **Interpreter** — evaluate chain refs
10. **Tests** — all 23+ test cases
11. **Documentation** — learn-briev, SPEC, architecture docs
