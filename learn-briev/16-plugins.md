# Compiler Plugins

Briev supports compile-time plugins that run at defined hooks in the
compilation pipeline.  Plugins are written in Briev and use a tree-navigation
DSL to inspect and transform the AST, source text, or generated IR.

## Pipeline Stages

There are 11 stages.  Each runs a specific subset of plugins:

```text
Source ──► PreLex ──► Parsed ──► Resolved ──► Typed ──► Normalized
             │           │            │           │            │
          text ops   tree ops      tree ops    tree ops     tree ops

──► Verified ──► Allocated ──► Provenanced ──► Generated ──► Optimized
        │             │              │               │              │
     tree ops      tree ops       tree ops        text ops       text ops

──► Linked
       │
    binary ops
```

## Writing a Plugin

A plugin is a `.bv` file with one or more `$(StageName)` blocks:

```briev
// my-plugin.bv
$(Parsed) {
    // Runs after parsing, before import resolution
    Tag$("import").First$().Before$()
        .Insert$(Import$("std/custom.bv"));
};
```

### The Navigation Chain

Every operation follows the same pattern:

```
SELECT ──► TRAVERSE ──► POSITION ──► ACT
```

```briev
Tag$("import") .First$() .Before$() .Insert$(Import$("std/x.bv"))
 └─SELECT──┘  └TRAVERSE┘ └POSITION┘ └────────ACT────────────┘
```

- **Selectors** find nodes: `Tag$("defn")`, `Named$("main")`, `WithAttr$("entry", true)`
- **Traversal** narrows: `.First$()`, `.Children$("param")`, `.Descendants$("call")`
- **Positions** pick where: `.Before$()`, `.After$()`, `.Replace$()`, `.Inside$()`
- **Actions** do the work: `.Insert$(Import$("..."))`, `.Delete$()`, `.Set$("key", val)`

### Flow Control

Inside `$(Stage)` blocks, standard Briev syntax (`let`, `when`, `foreach`, `match`)
is evaluated at compile time. Navigation selections are first-class values.

```briev
// Bind a selection to a variable
let imports = Tag$("import");

// Iterate over matches
foreach imp in imports) {
    imp.After$().Insert$(Import$("std/debug.bv"));
};

// Conditional — no parens needed
when imports.Count$() == 0 {
    EmitWarning$("no imports found");
};
```

## Enabling/Disabling Plugins

```bash
# Disable the prelude plugin (no auto-imports)
briev build file.bv --disable-plugin prelude

# Enable a specific plugin
briev build file.bv --enable-plugin my-custom

# Disable all plugins (equivalent to --no-stdlib)
briev build file.bv --disable-plugin prelude
```

## Building With Plugins

```bash
# Default: system plugins run automatically
briev build file.bv

# With BEAST snapshots for debugging
briev build file.bv --emit-beast parsed

# Custom plugin file
briev build file.bv --enable-plugin my-plugin
```

## Target Selection

Each stage has a default data target:

| Stage | Default target | What you can do |
|-------|---------------|-----------------|
| `$(PreLex)` | `Source$` (text) | `Find$`, `Prepend$`, `ReplaceWith$`, `Text$()` |
| `$(Parsed)` through `$(Provenanced)` | AST (tree) | `Tag$`, `Named$`, `Insert$`, `Delete$`, `Set$` |
| `$(Generated)` through `$(Optimized)` | `Ir$` (text) | `Find$`, `InsertBefore$`, `ReplaceWith$`, `Text$()` |
| `$(Linked)` | `Bin$` (binary) | `Run$("command {{path}}")`, `Path$()`, `ReadBytes$()`, `Size$()` |
| All stages | `Stage$` (registry) | `Insert$(block)`, `Insert$(file)`, `Remove$(name)`, `List$()` |

You can always override by prefixing `Source$.`, `Ir$.`, `Bin$.`, or `Stage$.`:

```briev
$(Typed) {
    // Default: AST operations
    Tag$("defn").Named$("main").Set$("entry", true);
    // Explicit: source text access (read-only)
    let lines = Source$.Find$("#define").Count$();
};
```

## Stage Priority

Stage blocks can declare a priority to control execution order. The
syntax is `$(Stage @ priority)`:

```briev
$(Parsed @ 750) {
    // Runs at priority 750 (high)
};

$(Typed @ 250) {
    // Runs at priority 250 (low)
};
```

Priority can be specified as:

| Form | Example | Meaning |
|------|---------|---------|
| Integer (0–1000) | `$(Parsed @ 750)` | Exact priority number |
| Named: `highest` | `$(Parsed @ highest)` | 1000 |
| Named: `high` | `$(Parsed @ high)` | 750 |
| Named: `normal` | `$(Parsed @ normal)` | 500 (default) |
| Named: `low` | `$(Parsed @ low)` | 250 |
| Named: `lowest` | `$(Parsed @ lowest)` | 0 |

Without an explicit priority, stage blocks default to `normal` (500).
Plugins registered from other plugins can specify priorities to
control their position in the execution queue.

## Compile-Time Variables (`$let` / `$const`)

Stage blocks can define mutable and immutable compile-time variables:

```briev
$(Parsed) {
    $let target_count = 100;       // mutable
    $const max_items = 500;        // immutable

    // Use bare names inside stage blocks
    when Tag$("defn").Count$() > max_items {
        EmitError$("too many definitions");
    };
};
```

Rules:
- `$let name = expr;` — mutable, can be reassigned
- `$const name = expr;` — immutable, cannot be reassigned
- Evaluated during stage block execution (before codegen)
- Accessible by bare name (no `$` prefix) inside stage blocks
- Available to regular `const X = name;` declarations and `trg name @ instance;` bindings

## Compile-Time Functions (`$defn` / `$txn`)

Stage blocks can define reusable compile-time functions:

```briev
$(Parsed) {
    $defn count_defns() -> Int {
        term Tag$("defn").Count$();
    };

    let count = count_defns();
    EmitInfo$("definitions: " + count);
};
```

- `$defn` — pure compile-time function
- `$txn` — convergent compile-time function (needs `[pre][post]`)

See the existing `defn` example in "Full Briev at Compile Time" — the
same syntax works with `$defn` inside `$(Stage)` blocks. The `$` prefix
distinguishes compile-time from runtime definitions.

## Full Briev at Compile Time

Inside `$(Stage)` blocks, you can write arbitrary Briev code and it runs at
compile time:

```briev
$(Parsed) {
    defn count_tagged(sel: Selection, tag: String) -> Int {
        let total = 0;
        foreach item in sel) {
            when item.Tag$(tag).Count$() > 0 {
                total = total + 1;
            };
        };
        term total;
    };

    let defns = Tag$("defn");
    EmitInfo$("defns with calls: " + count_tagged(defns, "call"));
};
```

Note: `txn`/`node`/`trg`/`frgn`/`Malloc#` are not available at compile time.
Only `let`/`defn`/`when`/`match`/`for` and the navigation DSL.

## Diagnostics

```briev
EmitInfo$("informational message");     // prints to stdout
EmitWarning$("suspicious pattern");     // prints to stderr
EmitError$("fatal problem");            // aborts compilation
```

## Plugins Creating Plugins

A plugin can register new plugins for later stages:

```briev
$(Parsed) {
    when Tag$("call").Named$("Unsafe#").Count$() > 0 {
        Stage$.Insert$(Typed) {
            foreach call in Tag$("call").Named$("Unsafe#")) {
                EmitWarning$("unsafe: " + call.Names$().First$());
            };
        };
    };
};
```

Forward-only: a `$(Parsed)` plugin cannot register for `$(Parsed)` or earlier.
Only stages > the current one.

## What's Next

- See `docs/architecture/features/plugins.md` for the full intrinsic reference.
- See `examples/stage/` for runnable plugin examples.
- See `docs/plans/2026-07-21-granular-pipeline-and-ast-navigation.md` for the design.

## Beyond AST: Generic Compile-Time Intrinsics

The `$` system is not limited to AST manipulation. Generic intrinsics
provide file I/O, string processing, configuration reading, universe
queries, and external command execution — usable by ANY plugin, not
just bridge generators.

### String Processing

```briev
$(Parsed) {
    let msg = StrReplace$("Found {{n}} errors", "{{n}}", "42");
    EmitInfo$(msg);  // prints "Found 42 errors"

    let parts = StrSplit$("a,b,c", ",");
    let joined = StrJoin$(parts, " | ");
    EmitInfo$(joined);  // prints "a | b | c"
};
```

### File I/O

```briev
$(Parsed) {
    // Read a config file
    let cfg = FileRead$("config.toml");

    // Write generated output
    FileWrite$("output.txt", "generated content");
};
```

### Configuration Reading

```briev
$(Parsed) {
    let tmpl = ConfigGet$("rust", "templates.fn_template");
    // Reads lib/glue.toml → [rust.templates] → "fn_template"
};
```

### Type Information

```briev
$(Parsed) {
    let name = TypeInfo$(Named$("my_fn").First$(), "name");
    let pcount = TypeInfo$(Named$("my_fn").First$(), "params.count");
    let p0type = TypeInfo$(Named$("my_fn").First$(), "params.0.type");
};
```

### Protocol Path Queries

```briev
$(Parsed) {
    // Compute protocol path between two types
    let path = CastPath$("String", "String");
    // Returns ["String", "String"] — identity path
};
```

### External Commands

```briev
$(Parsed) {
    // Run an external tool at compile time
    let output = ShellCmd$("briev", "check", "file.bv");
};
```

### Environment and System Intrinsics

```briev
$(Parsed) {
    // Get environment variable (requires --allow-read)
    let home = EnvGet$("HOME");

    // System information query (requires --allow-sys-query)
    let os = SysQuery$("os");

    // Current UTC timestamp
    let now = TimeNow$();

    // HTTP GET request (requires --allow-net)
    let response = HttpFetch$("https://api.example.com/data");
};
```

| Intrinsic | Signature | Permission |
|-----------|-----------|------------|
| `EnvGet$` | `EnvGet$(name: String) -> String` | `--allow-read` |
| `SysQuery$` | `SysQuery$(query: String) -> String` | `--allow-sys-query` |
| `TimeNow$` | `TimeNow$() -> String` | None |
| `HttpFetch$` | `HttpFetch$(url: String) -> String` | `--allow-net` |

### Design Principle

Every `$` intrinsic is **fully generic** — it must have at least 3 distinct
non-GLUE use cases before it's accepted into the engine. If a capability is
only useful for bridge generation, it belongs in the bridge generator `.bv`
file (as a composition of generic intrinsics), not as a new Rust intrinsic.

The bridge generator itself is a `.bv` plugin that combines exactly 7 generic
intrinsics: `ConfigGet$`, `StrReplace$`, `TypeInfo$`, `CastPath$`, `FileWrite$`,
`ShellCmd$`, `EnvGet$`, `SysQuery$`, `TimeNow$`, `HttpFetch$`, and the AST
selection/traversal chain.

