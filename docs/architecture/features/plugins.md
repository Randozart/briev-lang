# Plugin Architecture — AST Navigation DSL

**2026-07-21:** Replaces the old Front/Mid/Post/Back four-stage system and the
`Collect$`/`MatchIR$` serialize-deserialize intrinsics. The new system has 11
granular pipeline stages and a direct AST navigation DSL that operates on the
live tree in memory — no `.beast` serialization in the plugin data path.

---

## How Plugins Work

A plugin is a `.bv` file containing one or more `$(StageName)` blocks.
Each block runs at the corresponding compiler pipeline stage:

```briev
// plugins/parsed/prelude.bv
$(Parsed) @ highest {
    Tag$("import").First$().Before$()
        .Insert$(Import$("std/types/bootstrap.bv"));
};
```

The block body can contain:
- Navigation chains (`Tag$("import").First$().Before$().Insert$(...)`)
- Flow control (`let`, `when`, `foreach`, `match`)
- Full Briev code (`defn`, `let`, `when`, `match`, `for`) evaluated at compile time
- Plugin injection (`Stage$.Insert$`, `Stage$.Remove$`, `Stage$.List$`)
- Diagnostics (`EmitInfo$`, `EmitWarning$`, `EmitError$`)

These are not separate DSL constructs — they are the standard Briev interpreter
extended with compile-time types (`CTSelection`, `CTPosition`, `CTTarget`)
and navigation intrinsics as built-in methods on selections.

All of these are evaluated at compile time by the interpreter (extended with
compile-time value types like `CTSelection`, `CTPosition`, `CTTarget`).

### Three-Link Chain

Every transformation follows:

```
SELECT ──► TRAVERSE ──► POSITION ──► ACT
```

```briev
Tag$("import") .First$() .Before$() .Insert$(Import$("std/x.bv"))
 └─SELECT──┘  └TRAVERSE┘ └POSITION┘ └───────────ACT────────────┘
```

---

## Pipeline Stages

| Stage | Plugin directory | Data | What's available |
|-------|-----------------|------|------------------|
| `$(PreLex)` | `plugins/prelex/` | Source text (`Source$`) | Raw `.bv` text before lexing |
| `$(Parsed)` | `plugins/parsed/` | AST (implicit) | Freshly parsed AST, no imports resolved |
| `$(Resolved)` | `plugins/resolved/` | AST | All imports resolved and merged |
| `$(Typed)` | `plugins/typed/` | AST | Full type universe available |
| `$(Normalized)` | `plugins/normalized/` | AST | Backend type annotations attached |
| `$(Verified)` | `plugins/verified/` | AST | Protocol round-trip verified |
| `$(Allocated)` | `plugins/allocated/` | AST | Allocation strategies assigned |
| `$(Provenanced)` | `plugins/provenanced/` | AST | Pointer provenance validated |
| `$(Generated)` | `plugins/generated/` | IR text (`Ir$`) | Emitted backend IR (`.ll`, `.mlir`, `.ts`) |
| `$(Optimized)` | `plugins/optimized/` | IR text (`Ir$`) | Optimized IR |
| `$(Linked)` | `plugins/linked/` | Binary path (`Bin$`) | Compiled binary |

### Stage Priority

The `@` syntax sets execution priority (lower number = earlier):

```briev
$(Parsed) @ highest { ... }     // maps to priority 0
$(Typed) @ 100 { ... }          // explicit priority
$(Verified) @ lowest { ... }    // maps to priority 999
```

Default priority is 500.  Within the same stage, plugins run in priority order.

---

## The Four Targets

Navigation operations target one of four data surfaces:

| Target | Type | Operation style | Valid stages |
|--------|------|----------------|--------------|
| `Source$` | Source text | Text ops: `Find$`, `ReplaceWith$`, `Prepend$`, `Append$`, `Text$()`, `Path$()` | All (read-only after PreLex) |
| AST (implicit) | `Vec<TopLevel>` | Tree ops: `Tag$`, `Named$`, `foreach`, `Insert$`, `Delete$` | Parsed – Provenanced |
| `Ir$` | IR text | Text ops: `Find$`, `ReplaceWith$`, `InsertBefore$`, `Text$()` | Generated, Optimized |
| `Bin$` | Binary path | External: `Run$("command {{path}}")`, `Path$()`, `Size$()`, `ReadBytes$()` | Linked |
| `Stage$` | Plugin registry | `Insert$(block)`, `Insert$(path)`, `Remove$(name)`, `List$()` | All (forward-only) |

The default target at each stage is shown in the table above.  You can always
override by prefixing with `Source$.`, `Ir$.`, or `Bin$.`:

```briev
$(Parsed) {
    // Default: AST
    Tag$("import").First$().Before$().Insert$(Import$("std/x.bv"));
    // Explicit: source text (read-only at this stage)
    Source$.Find$("#define").Count$();
};

$(Generated) {
    // Default: Ir$
    Find$("target triple").ReplaceWith$("target triple = \"riscv64\"");
    // Still can access source for cross-referencing
    Source$.Find$("// METADATA").Count$();
};
```

---

## Core Intrinsic Reference

### Tree Selectors (AST target)

| Intrinsic | Returns | Description |
|-----------|---------|-------------|
| `All$()` | Selection | Every top-level AST node |
| `Tag$(name)` | Selection | Nodes by S-expression tag (`"defn"`, `"txn"`, `"call"`, `"import"`) |
| `Pattern$(sexpr)` | Selection | Nodes matching `.beast` pattern with `?var`/`?*`/`??*` |
| `Named$(name)` | Selection | Nodes whose name field equals `name` |
| `WithKey$(key)` | Selection | Nodes having metadata key `key` |
| `WithAttr$(key, val)` | Selection | Nodes with metadata `key` = `val` |

### Text Selectors (Source$, Ir$ target)

| Intrinsic | Returns | Description |
|-----------|---------|-------------|
| `Find$(pattern)` | TextSelection | Lines/regions matching regex pattern |

### Selector Combinators

| Intrinsic | Target | Description |
|-----------|--------|-------------|
| `.And$(sel)` | Both | Intersection |
| `.Or$(sel)` | Both | Union |
| `.Not$(sel)` | Both | Complement |

### Tree Traversal (AST target)

| Intrinsic | Returns | Description |
|-----------|---------|-------------|
| `.First$(n?)` | Selection | First N elements (default 1) |
| `.Last$(n?)` | Selection | Last N elements (default 1) |
| `.Nth$(n)` | Selection | Nth element (0-indexed) |
| `.Children$(sel?)` | Selection | Direct children (optionally filtered) |
| `.Descendants$(sel?)` | Selection | All descendants (optionally filtered) |
| `.Parent$()` | Selection | Parent(s) |
| `.Ancestors$(sel)` | Selection | Ancestors matching selector |
| `.Closest$(sel)` | Selection | Nearest ancestor matching |
| `.Next$(sel?)` | Selection | Following siblings |
| `.Prev$(sel?)` | Selection | Preceding siblings |

### Positions

| Intrinsic | Target | Description |
|-----------|--------|-------------|
| `.Before$()` | Tree + Text | Insert before each selected |
| `.After$()` | Tree + Text | Insert after each selected |
| `.Replace$()` | Tree + Text | Replace each selected |
| `.Inside$()` | Tree only | Prepend to each selected's children |
| `.AppendTo$()` | Tree only | Append to each selected's children |

### Actions

| Intrinsic | Target | Description |
|-----------|--------|-------------|
| `.Insert$(node...)` | Tree | Insert constructed AST nodes |
| `.Insert$(text)` | Text | Insert text string |
| `.Delete$()` | Both | Remove selected |
| `.ReplaceWith$(node)` | Tree | Substitute AST node |
| `.ReplaceWith$(text)` | Text | Substitute text |
| `.Set$(key, val)` | Tree only | Set metadata |
| `.Wrap$(tag)` | Tree only | Enclose in container |
| `.Rename$(name)` | Tree only | Rename identifier |
| `.Prepend$(text)` | Text only | Prepend to entire buffer |
| `.Append$(text)` | Text only | Append to entire buffer |
| `.Run$(cmd)` | Binary only | Run external command with `{{path}}` |

### Introspection

| Intrinsic | Returns | Description |
|-----------|---------|-------------|
| `.Count$()` | Int | Number of selected nodes |
| `.IsEmpty$()` | Bool | Selection empty? |
| `.Names$()` | List[String] | Name fields of selected |
| `.Lines$()` | List[Int] | Line numbers of text matches (Text target only) |

### Flow Control

Inside `$(Stage)` blocks, standard Briev syntax (`let`, `when`, `foreach`, `match`)
is evaluated at compile time. Navigation selections are first-class values.

| Construct | Description |
|-----------|-------------|
| `let name = expr;` | Bind selection/position/value |
| `foreach(item in sel) { body }` | Iterate over selection, binds `item` |
| `when cond { body };` | Conditional execution — no parens needed |
| `match expr { arms }` | Pattern matching on values |
All navigation intrinsics are available on `$`.

---

## AST Constructors

These `PascalCase$` intrinsics construct AST nodes for use with `.Insert$()`
and `.ReplaceWith$()`.  They are only valid inside `$(Stage)` blocks.

| Constructor | Produces | Example |
|-------------|----------|---------|
| `Import$(path, symbols?)` | `TopLevel::Import` | `Import$("std/io.bv")` |
| `Defn$(name, params, ret, body)` | `TopLevel::Definition` | `Defn$("main", [], Type$("Int"), ...)` |
| `Txn$(name, params, contract, body)` | `TopLevel::Transaction` | `Txn$("work", [], c, b)` |
| `Contract$(pre, post, entry?)` | `Contract` | `Contract$(Expr$(true), Expr$(true))` |
| `Block$(stmts...)` | `Vec<Statement>` | `Block$(Let$(...), ...)` |
| `Let$(name, ty?, expr?)` | `Statement::Let` | `Let$("x", Type$("Int"), Expr$(42))` |
| `Assign$(target, expr)` | `Statement::Assign` | `Assign$(Ident$("x"), Expr$(5))` |
| `Term$(expr?)` | `Statement::Term` | `Term$(Ident$("result"))` |
| `Call$(fn, args...)` | `Expr::Call` | `Call$("Print#", Ident$("x"))` |
| `Ident$(name)` | `Expr::Identifier` | `Ident$("x")` |
| `Expr$(lit)` | Literal expression | `Expr$(42)`, `Expr$("hello")` |
| `BinOp$(kind, lhs, rhs)` | `Expr::BinaryOp` | `BinOp$("Add", a, b)` |
| `Type$(name)` | `Type::Custom` | `Type$("Int")` |
| `Bits$(n)` | `Type::Bits` | `Bits$(64)` |
| `Ptr$(inner)` | `Type::Ptr` | `Ptr$(Type$("Int"))` |
| `Metadata$(key, val)` | `(String, PropertyValue)` | `Metadata$("inline", true)` |
| `Pattern$(sexpr)` | Compiled `Pattern` (for querying) | `Pattern$("(call ?fn ?arg)")` |

---

## Plugin Discovery

### System Plugins

System plugins live in `plugins/{stage}/<name>.bv`.  They are discovered
automatically at startup:

```bash
plugins/
  parsed/
    prelude.bv        # Injects stdlib imports
    prelude-hw.bv     # Silicon prelude slot for .sbv (insert dropped — gap E9)
  typed/
    auto-main.bv      # Adds [#] entry marker to main
    entry-check.bv     # Verifies entry mechanism exists
  verified/
    validate-trg.bv   # Checks dynamic trigger targets
```

Extension-specific plugin selection is configured in `config/targets.toml`:

```toml
[".bv"]
plugins = ["prelude"]

[".sbv"]
plugins = ["prelude-hw"]
```

Plugins listed in the extension's `plugins` array are active for that extension.
Plugins not listed are skipped.

### Inline Plugins

Plugins can also be embedded directly in source files using `$(Stage)` blocks:

```briev
// file.bv
$(Parsed) {
    // Custom compile-time logic for this file only
    Tag$("import").First$().After$()
        .Insert$(Import$("std/local/custom.bv"));
};

defn main() -> Int { term 0; };
```

Inline plugins are extracted from the AST before the Parsed stage runs.
They are registered as plugins for their declared stage.

### CLI Management

```bash
# Disable a system plugin
briev build file.bv --disable-plugin prelude

# Enable only specific plugins
briev build file.bv --enable-plugin auto-main

# Disable plugin = --no-stdlib (same effect)
briev build file.bv --no-stdlib

# Emit BEAST snapshots for plugin debugging
briev build file.bv --emit-beast typed
briev build file.bv --emit-beast all
```

---

## Examples

### Prelude — Insert standard library imports

```briev
$(Parsed) @ highest {
    let anchor = Tag$("import").First$();
    anchor.Before$().Insert$(
        Import$("std/types/bootstrap.bv"),
        Import$("std/os/fs.bv"),
        Import$("std/os/net.bv"),
        Import$("std/os/signal.bv"),
        Import$("std/os/ipc.bv"),
        Import$("std/os/thread.bv"),
        Import$("std/os/user.bv"),
        Import$("std/os/mem.bv"),
        Import$("std/os/atomic.bv"),
        Import$("std/core/ptr.bv"),
        Import$("std/core/string_builder.bv"),
        Import$("std/env.bv"),
        Import$("std/io.bv")
    );
};
```

### Auto-main — Set entry marker

```briev
$(Typed) @ highest {
    Tag$("defn").Named$("main").First$()
        .Descendants$("contract").First$().Set$("entry", true);
    Tag$("txn").Named$("main").First$()
        .Descendants$("contract").First$().Set$("entry", true);
};
```

### Entry check — Verify program can start

```briev
$(Typed) {
    let has_entry = Tag$("contract").WithAttr$("entry", true).Count$();
    let has_trg = Tag$("trigger").Count$();
    when has_entry == 0 && has_trg == 0 {
        EmitError$("no entry point: add [#] to defn main or trg declaration");
    };
};
```

### PrintLn! expansion

```briev
$(Parsed) {
    foreach(intercept in Tag$("plugin_intercept").Named$("PrintLn")) {
        let args = intercept.Children$();
        intercept.ReplaceWith$(Block$(
            Call$("PrintString#", Expr$("\n")),
            Call$("Print#", args.Nth$(0))
        ));
    };
};
```

### IR text modification

```briev
$(Generated) {
    Find$("target triple = \"x86_64\"")
        .ReplaceWith$("target triple = \"arm64\"");
    Prepend$("; Optimized by Briev plugin\n");
};
```

### Post-link binary stripping

```briev
$(Linked) {
    Bin$.Run$("strip --strip-unnecessary {{path}}");
};
```

### Conditional plugin injection

```briev
$(Parsed) {
    // Only register a typed validator if unsafe code is present
    let unsafe = Tag$("call").Named$("Unsafe#").Count$();
    when unsafe > 0 {
        Stage$.Insert$(Typed) {
            foreach(call in Tag$("call").Named$("Unsafe#")) {
                EmitWarning$("unsafe call: " + call.Names$().First$());
            };
        };
    };
};
```

### Diagnostics

```briev
$(Parsed) {
    let count = Tag$("import").Count$();
    EmitInfo$("file has " + count + " imports");
    when count == 0 {
        EmitWarning$("no imports — program may be incomplete");
    };
    when count > 50 {
        EmitError$("too many imports (" + count + "): consider consolidating");
    };
};
```

### Full Briev evaluation at compile time

```briev
$(Typed) {
    defn count_pattern(sel: Selection, tag: String) -> Int {
        let total = 0;
        foreach(item in sel) {
            when item.Tag$(tag).Count$() > 0 {
                total = total + 1;
            };
        };
        term total;
    };

    let all_defns = Tag$("defn");
    let with_calls = count_pattern(all_defns, "call");
    EmitInfo$("defns containing calls: " + with_calls);
};
```

---

## Tainted Node Filtering

Macro-produced AST nodes are tracked via `tainted_indices: BTreeSet<usize>`
on `PluginManager`. Selection intrinsics filter these out by default:

| Operation | Taint Behavior |
|-----------|---------------|
| `Insert$` (top-level) | Inserted indices marked tainted |
| `Delete$` | Removed indices dropped from taint set; remaining shifted |
| `ReplaceWith$` | Replaced indices marked tainted |
| StageBlock evaluation | All appended nodes marked tainted |

This prevents cascading interference between plugins — one macro's output
is invisible to subsequent selectors unless explicitly addressed.

---

## Transactional Rollback

`evaluate_stage_block` snapshots `program`, `vfs`, and `tainted_indices`
before execution. On any error, all three are restored to their pre-execution
state. No stale modifications survive a failing macro.

---

## Expansion Traces

`expansion_traces: HashMap<usize, String>` records the provenance of each
macro-produced node. `--dump-traces` prints the trace map:

```
=== Macro Expansion Traces ===
  [12] Insert$ -> import "std/foo.bv"
  [13] Insert$ -> defn helper
  [14] ReplaceWith$ -> defn optimized_fn
=== End Expansion Traces ===
```

The `record_expansion(pm, index, description)` helper is available for
custom intrinsics to write their own traces.

---

## `--diff` / Dry-Run Mode

`--diff` shows what macros changed without compiling:

1. Snapshots the program after parsing
2. Runs all stages normally
3. Diffs original vs final program using name-based key matching
4. Prints added/removed/modified items and exits early

Output format:
```
=== Macro Changes (2 change(s)) ===
  + [12] defn helper
  - [3] defn old_fn
  ~ [5→7] txn loop → txn compute
=== End Macro Changes ===
```

---

## Multi-Target SysQuery$ Overrides

The `--sysquery <key=value>` and `--sysquery-file <path>` flags override
`SysQuery$` results without changing source code:

```bash
briev build hello.bv \
  --sysquery cpu.cores=32 \
  --sysquery cpu.arch=x86_64 \
  --sysquery-file ./prod-sysquery.txt
```

Precedence (low → high): `--target` profile → `--sysquery-file` → `--sysquery`.

The file format is one `key=value` pair per line, with `#` comments and
blank lines skipped. No TOML/serde dependency required.

---

## Why WASM?

WASM plugin support is unchanged.  See prior documentation for the rationale:
sandboxing, language independence, stable ABI via WIT, and microsecond
instantiation.  WASM plugins implement the same `Plugin` trait and receive
the same `(program, universe)` state.

---

## BEAST Visualization

The `.beast` format is preserved as a **read-only visualization tool** for
plugin authors.  It shows the AST as S-expressions for human inspection:

```bash
briev build file.bv --emit-beast parsed    # → file.beast.parse
briev build file.bv --emit-beast typed     # → file.beast.types
briev build file.bv --emit-beast all       # all stages
```

`.beast` snapshots show the AST exactly as the navigation DSL sees it at
each stage.  This helps when writing `Tag$`, `Pattern$`, and `Named$`
selectors.  Plugins never read `.beast` text — they operate on the live AST.

---

## Old API Migration

The following table maps every removed intrinsic to its replacement:

| Removed | Replacement |
|---------|-------------|
| `InsertLiteralImport$("path")` | `Tag$("import").First$().Before$().Insert$(Import$("path"))` |
| `InsertRegistryImport$("name")` | `Tag$("import").First$().Before$().Insert$(Import$("name"))` |
| `Collect$("pattern")` | `Tag$(...).Count$()` or `Pattern$("...").Count$()` |
| `MatchIR$("pat", "rep")` | Pattern matching is handled by `foreach(match in Pattern$("pat")) { match.ReplaceWith$(...) }` |
| `CheckReactive$()` | `If$(Tag$("txn").WithAttr$("reactive", true).Count$() > 0) { ... }` |
| `$(Front)` for source | `$(PreLex)` |
| `$(Front)` for AST | `$(Parsed)` |
| `$(Mid)` | `$(Typed)` |
| `$(Post)` | `$(Generated)` |
| `$(Back)` | `$(Optimized)` |
