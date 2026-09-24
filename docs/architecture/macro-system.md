# Macro System — Compile-Time `$` Intrinsics

**Date:** 2026-07-23
**Status:** Architecture documentation

---

## Contract

Every `$` intrinsic is **fully generic** — useful to ANY plugin, not just
the GLUE bridge generator. A proposed intrinsic must have at least 3 distinct
non-GLUE use cases before it's accepted.

If a capability is only useful for bridge generation, it belongs in the
bridge generator `.bv` file itself (as a composition of generic intrinsics),
not as a new intrinsic in the Rust engine.

---

## Architecture

Macros run as `$(Stage) { body }` blocks at specified pipeline stages.
The compiler extracts them at parse time, evaluates them at their declared
stage using a tree-walking interpreter, and the body can:

- Select AST nodes via `Tag$`, `Named$`, `All$`, etc.
- Traverse via `Children$`, `First$`, `Parent$`, etc.
- Modify via `Insert$`, `Delete$`, `ReplaceWith$`
- Construct new nodes via `Import$`, `Defn$`, `Call$`, etc.
- Read configuration via `ConfigGet$`
- Query type info via `TypeInfo$`
- Read/write files via `FileRead$`/`FileWrite$`
- Execute shell commands via `ShellCmd$`
- Emit diagnostics via `EmitInfo$`/`EmitWarning$`/`EmitError$`
- Control flow: `let`, `when`, `foreach`, assignment (`=`), string concat (`+`)

---

## Intrinsic Reference

### AST Selection

Select nodes from the program tree by structural properties:

| Intrinsic | Signature | Returns | Example |
|-----------|-----------|---------|---------|
| `Tag$` | `(tag: str)` | Selection | `Tag$("transaction")` — all transactions |
| `Named$` | `(name: str)` | Selection | `Named$("main")` — items named "main" |
| `WithKey$` | `(key: str)` | Selection | `WithKey$("entry")` — items with entry key |
| `WithAttr$` | `(key: str, val: str)` | Selection | `WithAttr$("entry", "true")` — entry items |
| `All$` | `()` | Selection | `All$()` — every top-level item |

### AST Traversal

Navigate the tree relative to a selection:

| Intrinsic | Signature | Returns | Example |
|-----------|-----------|---------|---------|
| `First$` | `([n])` | Selection | `Tag$("txn").First$()` — first transaction |
| `Last$` | `([n])` | Selection | `Tag$("txn").Last$()` — last transaction |
| `Nth$` | `(n: int)` | Selection | `Tag$("txn").Nth$(1)` — second transaction |
| `Children$` | `([filter])` | Selection | `Named$("main").Children$()` |
| `Descendants$` | `([filter])` | Selection | All descendants |
| `Parent$` | `()` | Selection | Parent node |
| `IsEmpty$` | `()` | Bool | `Tag$("error").IsEmpty$()` |

### AST Positions

Compute insertion positions relative to selections:

| Intrinsic | Returns | Example |
|-----------|---------|---------|
| `Before$` | Position | `Tag$("import").First$().Before$()` |
| `After$` | Position | `Tag$("import").Last$().After$()` |
| `Replace$` | Position | `Named$("old_fn").Replace$()` |
| `Inside$` | Position | `Named$("main").Inside$()` |
| `AppendTo$` | Position | `Named$("struct").AppendTo$()` |

### AST Actions

Modify the program tree:

| Intrinsic | Effect | Example |
|-----------|--------|---------|
| `Insert$(pos, nodes...)` | Inserts nodes at position | `Before$().Insert$(Import$("std"))` |
| `Delete$(sel)` | Removes selected nodes | `Delete$(Named$("dead_fn"))` |
| `ReplaceWith$(sel, node)` | Replaces selection | `ReplaceWith$(Named$("old"), Defn$("new"))` |
| `Set$(sel, key, value)` | Sets metadata | `Set$(Named$("f"), "attr", "true")` |
| `Rename$(sel, name)` | Renames item | `Rename$(Named$("x"), "y")` |

### AST Constructors

Build new AST nodes:

| Intrinsic | Returns | Example |
|-----------|---------|---------|
| `Import$(path)` | TopLevel | `Import$("std/env.bv")` |
| `Defn$(name)` | TopLevel | `Defn$("my_fn")` |
| `Call$(name, args...)` | Statement | `Call$("print", "hello")` |
| `Block$(stmts...)` | Statement | `Block$(Call$("work"))` |

### Compile-Time Data

| Intrinsic | Signature | Returns | Category |
|-----------|-----------|---------|----------|
| `StrLen$` | `(s)` | Int | String |
| `StrReplace$` | `(s, from, to)` | String | String |
| `StrJoin$` | `(list, sep)` | String | String |
| `StrSplit$` | `(s, pat)` | List | String |
| `StrSubstr$` | `(s, start, end)` | String | String |
| `FileRead$` | `(path)` | String | I/O |
| `FileWrite$` | `(path, content, [persist])` | Void | I/O |
| `ConfigGet$` | `(section, key)` | String | Config |
| `DocRead$` | `(type_name, property)` | varies | Universe |
| `TypeInfo$` | `(selection, field)` | varies | Type |
| `CastPath$` | `(src_type, tgt_type)` | List | Protocol |
| `ShellCmd$` | `(cmd, args...)` | String | Process |
| `Quote$` | `(template)` | TopLevel | Meta |
| `SysQuery$` | `(query)` | Int/Str | System |
| `TimeNow$` | `()` | Int | Timestamp |
| `EnvGet$` | `(name)` | String | Environment |
| `HttpFetch$` | `(url)` | String | Network |
| `EmitInfo$` | `(msg)` | Void | Diagnostic |
| `EmitWarning$` | `(msg)` | Void | Diagnostic |
| `EmitError$` | `(msg)` | Void | Diagnostic |
| `Count$` | `([sel])` | Int | Introspection |
| `Names$` | `(sel)` | List[String] | Introspection |

### ConfigGet$ Key Syntax

`ConfigGet$(lang, "templates.fn_template")` supports dotted keys:

| Pattern | Example | Returns |
|---------|---------|---------|
| `templates.<name>` | `templates.fn_template` | Template string |
| `protocols.<Type>` | `protocols.Int` | `"native/c_abi"` |
| `protocols.<Type>.native` | `protocols.Int.native` | Native type name |
| `protocols.<Type>.c_abi` | `protocols.Int.c_abi` | C ABI type name |

### TypeInfo$ Field Reference

| Field | Works on | Example result |
|-------|----------|----------------|
| `name` | `defn`, `txn`, `frgn`, `import` | `"my_function"` |
| `params.count` | `defn`, `txn` | `"2"` |
| `params.0.name` | `defn`, `txn` | `"a"` |
| `params.0.type` | `defn`, `txn` | `"Int"` |
| `output_type` | `defn` | `"Int"` |
| `outputs.count` | `defn` | `"0"` |
| `path` | `import` | `"std/io.bv"` |

`TypeInfo$` delegates through `TopLevel::Export` — calling it on an export node
automatically queries the inner definition.

---

## Security & Sandboxing

### Capability Sandbox

Every I/O-bearing `$` intrinsic is gated by a `Capability` check:

| Capability | Intrinsics | CLI flag |
|-----------|------------|----------|
| `DiskRead` | `FileRead$` | `--allow-read` |
| `DiskWrite` | `FileWrite$` | `--allow-write` |
| `Shell` | `ShellCmd$` | `--allow-run` |
| `SysQuery` | `SysQuery$` | `--allow-sys-query` |
| `Network` | `HttpFetch$` | `--allow-net` |
| `Pure` | All others | Always granted |

### Gas Budget

`--macro-budget <N>` sets an instruction limit (0 = unlimited). Each `$`
intrinsic call consumes one unit.

### Virtual Filesystem (VFS)

`FileWrite$` writes to an in-memory VFS by default. Pass `true` as the third
argument to persist to physical disk. `FileRead$` checks VFS first, then disk.
`--dump-vfs` prints VFS contents after compilation.

### Macro Lockfile

`macro-lock.toml` at the project root records approved capabilities per plugin
`.bv`. Validated by SHA-256 hash. `--update-lockfile` regenerates.

---

## Tainted Node Filtering

Macros can produce output that must be isolated from subsequent plugin
evaluation. The `tainted_indices: BTreeSet<usize>` set on `PluginManager`
tracks which top-level indices were produced by macros:

| Operation | Taint effect |
|-----------|-------------|
| `Insert$` at top-level position | Inserted indices are marked tainted. Traces recorded. |
| `Delete$` | Deleted indices removed from taint set; remaining indices shifted by count of deletions before them. |
| `ReplaceWith$` | Replaced indices marked tainted. |
| StageBlock append | All nodes from `prev_len..program.len()` are marked tainted at end of evaluation. |

AST selection intrinsics (`All$`, `Tag$`, `Named$`, `WithKey$`, `WithAttr$`)
filter tainted nodes out via `filter_tainted_nodes()` after selection.
This prevents macros from seeing each other's output unless they explicitly
opt in via the selection mechanism.

---

## Transactional Macro Execution

`evaluate_stage_block` wraps execution in a snapshot/restore mechanism:

1. Before execution: snapshot `program.clone()`, `pm.vfs.clone()`, `pm.tainted_indices.clone()`
2. Execute the stage block body
3. On any `Err`: restore all three snapshots, propagating the error

This ensures that a failing macro does not leave the AST, VFS, or taint
state in a corrupted half-applied state.

---

## Expansion Traces

`expansion_traces: HashMap<usize, String>` on `PluginManager` records
the provenance of every macro-produced AST node:

- `Insert$` records `"Insert$ -> defn my_fn"`
- `ReplaceWith$` records `"ReplaceWith$ -> defn my_fn"`  
- StageBlock appends record `"StageBlock appended at index N"`

Use `--dump-traces` after compilation to print the trace map sorted by index:

```
=== Macro Expansion Traces ===
  [12] Insert$ -> import "std/foo.bv"
  [13] Insert$ -> defn helper
  [14] ReplaceWith$ -> defn optimized_fn
=== End Expansion Traces ===
```

The `record_expansion(pm, index, description)` helper in `eval.rs` allows
custom intrinsics to write traces.

---

## `--diff` / Dry-Run Mode

`--diff` shows what macros changed without writing output:

1. Snapshots the program after parsing
2. Runs all compilation stages normally
3. Computes a diff between original and final program
4. Prints the diff and exits early (no codegen or output file)

Diff detection (`src/macros/diff.rs`):

| Entry | Meaning | Format |
|-------|---------|--------|
| `Added(idx, summary)` | Item present only in final | `+ [12] defn helper` |
| `Removed(idx, summary)` | Item present only in original | `- [3] defn old_fn` |
| `Modified(before, after, summary)` | Item changed between passes | `~ [3→5] defn foo → defn foo` |

Items are matched by name-based keys (`defn:foo`, `import:std/io.bv`).
Modification is detected via Debug output comparison.

---

## Compile-Time Functions (`$defn` / `$txn`)

`$defn` and `$txn` are compile-time-only function definitions. They live
at the top level alongside `$(Stage)` blocks, are extracted before codegen,
and can call `$` intrinsics. They push logic from Rust into Briev.
(2026-09-22, C6): `$txn` declares at TOP LEVEL only — there is no inline
`$txn` declaration. Inline *invocation* (`$txn name(args)` from a
`$(Stage)` block) is the call path.

### Syntax

```briev
$defn render(tmpl: String, name: String) -> String {
    let body = StrReplace$(tmpl, "{{name}}", name);
    let body = StrReplace$(body, "{{other}}", "...");
    term body;
};

$txn converge(x: Int) [x < 100][x >= 100] {
    when x < 50 { x = x + 10; };
    when x >= 50 { x = 100; term x; };
};
```

### Semantics

| Construct | Semantics |
|-----------|-----------|
| `$defn name(params) -> Type { body }` | Pure function. Body executes when called. `term` returns a value. |
| `$txn name(params) [pre][post] { body }` | Convergent loop. Runs body repeatedly until postcondition is met. |

### `$defn` Execution

1. Bind parameters from call arguments
2. Execute body statements sequentially
3. `term expr` → evaluate `expr`, return value immediately
4. `escape expr` → abort with error
5. No `term` reached → return `Void`

### `$txn` Execution (Convergent Loop)

1. Evaluate **precondition**. If false → converged, return `Void`.
2. Execute body:
   - `term expr` → store `expr` as candidate, exit body
   - `escape expr` → abort with error
3. Evaluate **postcondition**. If true → converged, return candidate (or `Void`).
4. If false → repeat from step 1.
5. Max 1000 iterations guard.

The pre/post conditions are `Expr` values evaluated in the function scope.
They can reference parameters and local variables:

```briev
$txn accum(list: List, n: Int) [list.Count$() < 100][list.Count$() >= 100] {
    list <- n;
};
```

### Control Flow Inside Bodies

Only `when` is available for conditionals (no `if`). Only `term` and `escape`
for exit:

| Statement | Effect |
|-----------|--------|
| `let x = expr;` | Bind variable |
| `x = expr;` | Reassign variable |
| `when guard { body };` | Execute body if guard is truthy |
| `term expr;` | Return value from function |
| `escape expr;` | Abort with error message |
| `expr;` | Evaluate expression (typically a `$` intrinsic call) |

### Calling from Stage Blocks

```briev
$defn greet(name: String) -> String {
    let msg = "hello " + name;
    term msg;
};

$(Parsed @ highest) {
    let result = greet("world");
    EmitInfo$("msg: " + result);  // prints "msg: hello world"
};
```

### Extraction Before Codegen

`$defn`/`$txn` items are extracted by `extract_inline_stage_blocks` in
`src/plugin/loader.rs`. They are:
1. Found in the top-level program after parsing
2. Registered in `PluginManager.fn_registry`
3. Removed from the program before codegen

They never reach the LLVM backend, so `$` intrinsics inside them are
unavailable at runtime — enforced by the double barrier of extraction +
runtime evaluation failure.

### `$` Prefix Convention

| Prefix | Category | Example |
|--------|----------|---------|
| `$` before keyword | Compile-time function definition | `$defn`, `$txn` |
| `$` after identifier | `$` intrinsic call | `Tag$`, `Insert$` |
| `#` after identifier | Backend intrinsic | `Sqrt#`, `Malloc#` |

The prefix `$` before `defn`/`txn` is intentionally scannable — `grep '$defn' *.bv`
finds all compile-time definitions instantly.

---

## Compile-Time Variables (`$let` / `$const`)

Compile-time variables store values in `PluginManager.comptime_vars` and are
accessible in `$(Stage)` blocks by bare name.

### Syntax

```briev
$let name = expr;     // mutable compile-time variable
$const name = expr;   // immutable compile-time constant
```

### Semantics

| Construct | Semantics |
|-----------|-----------|
| `$let name = expr;` | Evaluates `expr` at compile time, stores result. Can be reassigned later. |
| `$const name = expr;` | Evaluates `expr` at compile time, stores result. Immutable — reassignment is an error. |

### Storage

`$let` and `$const` variables are stored in `PluginManager.comptime_vars`
(a `HashMap<String, NavValue>`). Runtime rebinding is explicit via `= expr;`
in a stage block (without `$let`/`$const` prefix). The `resolve_comptime_refs()`
pass resolves bare-name references in `const X = name;` and `trg name @ instance;`
declarations by looking up `comptime_vars`.

### Example

```briev
$(Parsed @ highest) {
    $let target_name = "linux";
    $const debug_mode = true;
    EmitInfo$("Building for: " + target_name);
    target_name = "linux_x86_64";  // rebind mutable
};
```

---

## Macro DSL Expression Support

Inside `$(Stage)` blocks, the following expression types are evaluated:

| Expression | Handling | Example |
|-----------|----------|---------|
| `$` intrinsic call | `eval_nav_call` | `Tag$("defn")` |
| `.Method$()` chain | `eval_nav_field_method` | `exports.First$()` |
| `Identifier` | Scope lookup | `name` (resolves from `let` bindings) |
| `Decimal`, `Float`, `Bool` | Literal | `42`, `3.14`, `true` |
| `Quoted` (string literal) | `NavValue::Str` | `"hello"` |
| `+` (BinaryOp::Add) | String/int concatenation | `"a" + name + "b"` |
| `List` | `NavValue::List` | `[a, b, c]` |

### String Concatenation

The `+` operator works for `Str + Str`, `Str + Int`, `Int + Str`, `Int + Int`.
Used pervasively in the GLUE generator to build template variables:

```
let msg = "cpu.cores = " + SysQuery$("cpu.cores");
```

### List Construction

`[elem1, elem2]` produces `NavValue::List`, used with `StrJoin$`:

```
StrJoin$(["a", "b", "c"], ", ")  → "a, b, c"
```

### Assignment

`let x = expr;` creates a scope binding. `x = expr;` (reassignment) updates
an existing binding. Both use `Statement::Assign` for the second form.

### Variable Resolution in Intrinsic Arguments

`expect_str_arg` resolves `Expr::Identifier` from the compile-time scope.
This means `StrReplace$(tmpl, "{{name}}", name)` correctly uses the value
of `name` from a previous `let` binding, rather than the literal string
`"name"`. Complex expressions (concatenations) are evaluated via
`eval_nav_chain` fallback.

---

## Multi-Target Compilation

### SysQuery$ Override System

Three override sources, cascading precedence (low → high):

| Source | Format | Priority |
|--------|--------|----------|
| `--target <name>` | briev.toml `[target.*]` profile | Lowest |
| `--sysquery-file <path>` | Plain text key=value file | Medium |
| `--sysquery <key=value>` | CLI pairs (repeatable) | Highest |

The file format for `--sysquery-file`:
```
# comments and blank lines are ignored
cpu.cores=32
cpu.arch=x86_64
cpu.cache_line_size=64
```

### SysQuery$ Override Check

When `SysQuery$("cpu.cores")` is called, the handler checks
`sandbox.sysquery_overrides` first. If an override exists for the key,
the mocked value is returned. Otherwise the real host is queried.

### Per-Target Output

With `--target <name>`, output goes to `bin/<name>/`. Without any target
flags, behavior is identical to pre-multi-target compilation (backward
compatible).

---

## The Bridge Generator as a `.bv` Plugin

The GLUE bridge generator (`lib/glue/generator.bv`) is implemented entirely
in Briev using `$defn` helpers and `$` intrinsics — a stress test:

```briev
$defn render_fn(tmpl: String, name: String, params: String, ...) -> String {
    // single-pass template substitution using $defn + StrReplace$
    term body;
};

$(Normalized @ highest) {
    let fn_tmpl = ConfigGet$("rust", "templates.fn_template");
    let exports = Tag$("export");
    foreach(exp in exports) {
        let name = TypeInfo$(exp, "name");
        // ...
        let fn_body = render_fn(fn_tmpl, name, params_str, ...);
        exports_code = exports_code + fn_body + "\n";
    };
    FileWrite$("glue-out/src/lib.rs", lib_rs, true);
};
```

Intrinsics exercised: `$defn`, `ConfigGet$`, `Tag$`, `TypeInfo$`,
`StrReplace$`, `FileWrite$`, `EmitInfo$`, `IsEmpty$`, `+` (string concat),
`foreach`, `when` guards, `let` bindings, `=` assignment.

---

## Generic `$` Design Principles

### Principle 1: No GLUE-specific knowledge in Rust

The Rust engine (`src/macros/eval.rs`) should never reference `glue`, `bridge`,
`export`, or any language-specific concept. Everything in the `$` system is a
generic primitive. The bridge generator is a `.bv` plugin file that composes
these primitives.

### Principle 2: Three-use-case test

Every proposed intrinsic must have at least 3 distinct non-GLUE use cases:

| Intrinsic | Use case 1 | Use case 2 | Use case 3 |
|-----------|-----------|------------|------------|
| `StrReplace$` | Template substitution | Error message formatting | Code generation |
| `FileWrite$` | Scaffold generation | Doc output | Log file writing |
| `ConfigGet$` | Plugin configuration | Build profiles | Feature flags |
| `DocRead$` | Doc generator | Linter rules | LSP plugin |
| `TypeInfo$` | Bridge generator | Doc generator | Linter |
| `CastPath$` | Bridge generator | Type checker extension | Protocol debugger |
| `ShellCmd$` | Bridge build step | External formatter | Test runner |
| `SysQuery$` | Multi-target profiles | Hardware detection | Conditional compilation |

### Principle 3: `NavValue` variants stay orthogonal

| Variant | Meaning | Used as input by |
|---------|---------|------------------|
| `Selection` | Set of tree nodes | All traversal, position, and action intrinsics |
| `Position` | Insertion point | `Insert$` |
| `Count` | Integer count | `when guard > 0` |
| `Names` | List of string names | `StrJoin$` |
| `Bool` | Boolean | `when guard` |
| `Int` | Integer | Arithmetic, indexing |
| `Str` | String | All string intrinsics |
| `List` | Generic list of NavValues | `StrJoin$` (input), `StrSplit$` (output) |
| `TopLevel` | Single AST node | `Insert$`, `ReplaceWith$` |
| `VecTopLevel` | Multiple AST nodes | `Insert$` |
| `Void` | No value | `FileWrite$`, actions |
| `Map` | Key-value pairs | `ConfigGet$` (output) |

---

## `!`-suffix front-end plugins (2026-08-01, Phase 1-4)

Separate from the `$` compile-time macros above, Briev has `!`-suffix
**front-end plugins** that rewrite `name!(args)` intercepts at the Parsed
stage. These are ordinary Rust plugins (see `src/plugin/`), registered for
the `.bv` target in `config/targets.toml`. They are NOT `$` macros and do
NOT use the macro engine.

| Macro | Plugin | Expansion |
|-------|--------|-----------|
| `print!(...)` / `println!(...)` | `print` | Rust-style format strings → `__print_*` runtime calls |
| `get_env!("NAME")` / `get_env_int!("NAME")` | `env` | stdlib `get_env` / `get_env_int` wrappers |
| `entry!("<cmd>")` | `entry` | one-shot CLI subcommand guard: `entry_cmd() == "<cmd>" && !__entry_<cmd>_done` + flip |
| `args!("--flag")` / `args!("--flag", T)` | `entry` | snapshot state field from `__argv_has` / `__argv_value` |

Naming convention: `!`-suffix = compile-time expansion of a plugin
intercept; `$`-suffix = compile-time macro-system intrinsic; `#`-suffix =
backend intrinsic (`Sqrt#`); `#`-prefix = hashword (`Int`). The removed
`[#]` entry marker is replaced by `entry!`/`args!` (Phase 2-3).

See `docs/plans/2026-08-01-plugin-macro-rework.md` for the full design.
