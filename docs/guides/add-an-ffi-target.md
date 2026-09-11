# How to add a new FFI target — a beginner's walkthrough

**Date:** 2026-08-04
**Audience:** a new Briev user who wants their language to call Briev
**Other docs:** `docs/architecture/glue-ffi.md` (the complete method),
`docs/guides/ffi-and-export.md` (hands-on how-to), `learn-briev/07-ffi.md`
(tutorial). This guide is the **step-by-step for adding a language**.

---

## 0. What you're actually doing

Briev compiles to a native library. "Adding an FFI target" means teaching the
compiler how to talk to **one more host language** (Python, Node, Go, …). You
do **not** touch any Rust code. Everything lives in a folder:

```
lib/glue/<lang>/
    glue.dbvl     # the config: types, ABI, templates, how to build
    types.bv      # boundary type declarations (usually just re-export C's)
```

`brievc bindings <bridge> <lang>`, `brievc export <bridge> <lang>`, and
`brievc extension <bridge> <lang>` find `lib/glue/<lang>/` **by name**, load
its config, and render through one generic pipeline. The compiler has **zero
knowledge of your language** — your folder is data.

The easiest way to understand the folder is to copy an existing one that is
close to what you want and reshape it. The rest of this guide walks through the
shipped **Lua** target line by line, then tells you what to change for the
other shapes.

---

## 1. The three commands, and which shape you want

| Command | What it renders | When to use it |
|---------|-----------------|----------------|
| `briev bindings <b> <lang>` | Declarative files — a C header, a C# P/Invoke class | Your language can call a C ABI directly (C, C++, C#) |
| `briev export <b> <lang>` | A language **package** — a Go package, a Java class, a Rust crate | Your language wants idiomatic wrappers over the C ABI |
| `briev extension <b> <lang>` | A compiled **native extension** shim — Python `.so`, Node `.node`, Lua C module, Java JNI `lib*.so` | Your language loads a module and calls into it (no manual FFI) |

Each shape uses a different set of templates (§4). A target can implement one,
two, or all three (Java ships both an extension and a package).

> **The one thing you must build first** (any shape): a Briev **bridge** — a
> `.bv` file that `import "glue/c.bv"` and `export defn`s your functions. See
> `docs/guides/ffi-and-export.md` §2. Everything below is about the *other
> side* — the target that consumes a bridge.

---

## 2. Step 1 — `types.bv`

The boundary types your target needs. Almost always just:

```briev
import "glue/c.bv";
```

That brings in the standard C boundary types (`CStr`, `CDouble`, `CI64`, `CBool`,
…). Only add a `types.bv` of your own if your target needs a type C doesn't have.

---

## 3. Step 2 — `glue.dbvl`, field by field

This is the whole game. Here is the **complete, shipped Lua config**, annotated.
It is one line of a Data-Briev map (the file is literally one long line, but
each value is `key: value;`).

```text
lua: {
  types_module: "glue/lua/types.bv";   // §2 — the types.bv for this target
  extension: "lua";                    // this target's source/package label
  bridge_kind: "native_module";        // "native_module" | "cgo_package" |
                                       // "jni_module" | "extern_c_crate" | ...
  calling_convention: "c_abi";         // always "c_abi" today
  module_init: true;                   // does the shim have an init function
                                       // (luaopen_*, PyInit_*, JNI_OnLoad)?

  // ── protocols: how each Briev protocol category maps to YOUR language ──
  // native = the type as your language writes it
  // c_abi  = the type as your C shim sees it at the boundary
  protocols: {
    "Bool":   { native: "boolean";  c_abi: "long long" };
    "Char":   { native: "string";   c_abi: "long long" };
    "Data":   { native: "string";   c_abi: "long long" };
    "Float":  { native: "number";   c_abi: "double" };
    "Int":    { native: "integer";  c_abi: "long long" };
    "String": { native: "string";   c_abi: "long long" };
  };

  // ── conversions: how a value crosses the boundary ─────────────────────
  // to_abi:   render each argument from your value to the C boundary form
  //           ({name} is the argument's name). Identity when absent.
  // from_abi: render the raw C return (result_abi) back to your value.
  // These are your language's type-bridging snippets.
  conversions: {
    to_abi:   { "Int": "{name}"; "Float": "{name}"; "String": "{name}" };
    from_abi: { "Int": "result_abi"; "Float": "result_abi"; "String": "result_abi" };
  };

  // ── state: how the Briev state handle appears in your generated code ──
  // decl     = the state's declaration inside the shim
  // arg      = the state value at the call site
  // ffi_type = the type in an ffi/bindings signature (empty = omit)
  state: { decl: ""; arg: "state"; ffi_type: "" };

  // ── param_decl: how ONE parameter is written in your language ────────
  // C-family uses "{type} {name}"; Go uses "{name} {type}";
  // Python/Rust use "{name}: {type}".
  param_decl: "{name} {type}";

  // ── the toolchain recipe: how to COMPILE + LINK the extension ─────────
  // The compiler only knows "compile C, link a shared library". These
  // commands produce the flags it needs; each prints its answer on stdout.
  native_include_cmd:  "for L in ~/briev-tools/lua-*/src /usr/include/lua5.4 /usr/include; do [ -f \"$L/lua.h\" ] && echo \"-I$L\" && break; done";
  native_suffix:       ".so";          // output suffix (node uses ".node")
  // native_suffix_cmd: (optional) a command whose stdout IS the suffix
  // native_link_cmd:   (optional) prints link flags (python's --ldflags)
  // native_cc:         (optional) the C compiler (default "cc")
  // native_prefix:     (optional) output filename prefix (Java needs "lib")
};
```

Then the templates (§4) follow as numbered lines:

```text
lua.templates.0: "native.module" "… the module skeleton …";
lua.templates.1: "native.method" "… one export's wrapper function …";
...
```

The first quoted string is the **logical template name** (`native.module`,
`bindings.ffi_template`, `templates.cargo`, …) — the renderer looks templates
up by that name. The second string is the template **content** (with `\n`
escapes). The number (`templates.0`, `templates.1`) just gives them an order in
the file.

---

## 4. Step 3 — the templates

### 4.1 What a generated shim looks like (the `native.*` templates)

For `bridge_kind: "native_module"` (Python, Node, Lua), `briev extension`
renders **one C file** from three skeleton templates, then compiles it:

- **`native.module`** — the whole file: includes, the extern prototypes for the
  Briev exports, a module-global state, the per-export methods, and the
  module-init function (`luaopen_<bridge>`, `PyInit_<bridge>`, …). Placeholders:
  `{{bridge_name}}`, `{{export_protos}}`, `{{methods}}`, `{{method_defs}}`.
- **`native.method`** — one export's wrapper C function. Placeholders:
  `{{name}}`, `{{parse_code}}` (read args), `{{ret_c}}` (return C type),
  `{{call}}` (the call), `{{build_code}}` (push the return value).
- **`native.method_def`** — one row of the method table.

Then, **per protocol category**, four snippets that marshal each type:

| Template | When it's used | Placeholder |
|----------|----------------|-------------|
| `native.parse.<cat>` | reads one argument from the host | `{{name}}` |
| `native.build.<cat>` | pushes the return value back to the host | — |
| `native.c_type.<cat>` | the C type of a parameter in the extern prototype | — |
| `native.ret.<cat>` | the C type of the return | — |

Here is the **real Lua `native.method`** and its pieces:

```c
// native.method
static int lua_{{name}}(lua_State* L) {
    int _i = 1;
{{parse_code}}            // one line per param, e.g.:
                          //   long long count = luaL_checkinteger(L, _i++);
    {{ret_c}} r = {{call}};   // e.g.:  long long r = __briev_export_feature_hash(g_state, count);
{{build_code}}            // e.g.:  lua_pushinteger(L, r);  return 1;
}
```

```c
// native.parse.Int     (param <name>)
    long long {{name}} = luaL_checkinteger(L, _i++);
// native.build.Int
    lua_pushinteger(L, r);
    return 1;
// native.c_type.Int
long long
// native.ret.Int
long long
```

The `{{call}}` uses the export under a **mangled C name** with an asm label:

```c
extern long long __briev_export_feature_hash(BrievState* state, long long count)
    asm("feature_hash");
```

This is an invariant, not a choice: an export named like a libc/header function
(`read`, `open`, `malloc`) would otherwise collide with the host's own
prototype at compile time and be PLT-interposed at link time. The compiler
always does this; your templates just reference `{{call}}`.

### 4.2 The `bindings.*` templates (declarative files)

`briev bindings` renders every template whose name starts with `bindings.`.
Two names are special:

- **`bindings.ffi_template`** — rendered once per export and joined into the
  `{{ffi_decls}}` variable.
- **`bindings.<output-file>`** — the whole file, with `{{ffi_decls}}` where the
  per-export lines go.

Real C# example (rendered to the file named after the template key,
`bridge.cs`):

```text
csharp.bindings.bridge.cs: "public static class {{bridge_name}} {
    [DllImport(\"{{bridge_name}}\")]
    private static extern IntPtr __briev_init_state();
    public static IntPtr Init() { return __briev_init_state(); }
{{ffi_decls}}
}
";
csharp.bindings.ffi_template: "    [DllImport(\"{{bridge_name}}\")]
    private static extern {{c_return}} {{name}}({{s_ffi_param}}{{ffi_params}});
";
```

### 4.3 The `templates.*` templates (packages)

`briev export` renders package files (Go/Java/Rust wrappers). Per-export
wrapper functions come from **`fn_template`** (joined into `{{exports}}`);
whole-file templates use `{{exports}}`.

### 4.4 Every renderer variable

Available inside a **per-export** template (`bindings.ffi_template`,
`native.method`, `fn_template`, …):

| Variable | Meaning |
|----------|---------|
| `{{name}}` | the export name (`feature_hash`) |
| `{{name_upper}}` | camelCased (`FeatureHash`) — for Go/Java exported names |
| `{{bridge_name}}` | the bridge file's stem (`bench`) |
| `{{params}}` | parameters with **native** types, via `param_decl` |
| `{{ffi_params}}` | parameters with **C ABI** types, via `param_decl` |
| `{{args}}` | bare argument names (`a, b`) |
| `{{c_types}}` | the params' C ABI types |
| `{{args_abi}}` | each arg passed through `conversions.to_abi` |
| `{{return_expr}}` | the raw return passed through `conversions.from_abi` |
| `{{return}}` | the native return type |
| `{{c_return}}` | the C ABI return type |
| `{{s_param}}` / `{{s_ffi_param}}` / `{{s_ffi_type}}` | the state handle as a call arg / a signature param / a type (empty when the export is pure) |
| `{{parse_code}}` | concatenated `native.parse.<cat>` lines (native shims) |
| `{{ret_c}}` | `native.ret.<cat>` for the return (native shims) |
| `{{call}}` | `__briev_export_<name>(…)` with the right args (native shims) |
| `{{build_code}}` | `native.build.<cat>` for the return (native shims) |
| `{{sig_params}}` / `{{ret_jni}}` | JNI-style shims that take host values as direct signature params |
| `{{nargs}}` / `{{nargs_arr}}` | parameter count / ≥1 for array sizing (Node) |

Available inside a **whole-file** template (`bindings.<file>`,
`native.module`, package files):

| Variable | Meaning |
|----------|---------|
| `{{bridge_name}}` | the bridge file's stem |
| `{{ffi_decls}}` | per-export `bindings.ffi_template` lines, joined |
| `{{exports}}` | per-export `fn_template` wrappers, joined |
| `{{export_protos}}` / `{{methods}}` / `{{method_defs}}` | native-shim pieces, joined |

---

## 5. Step 4 — build and call it

Create a bridge and run the command:

```bash
brievc extension examples/glue-host/bench.bv lua --out build/
#   → build/bench.so   the Lua module (exports luaopen_bench AND the bridge's
#                      own symbols); the static bridge lib is build/libbench.a
```

Under the hood `briev extension`:
1. builds the bridge as a shared library,
2. renders the C shim from your `native.*` templates,
3. asks your toolchain recipe for the include/link flags,
4. compiles and links the shim into the extension (named per
   `native_suffix`/`native_prefix` — here `<bridge>.so`).

From Lua (the module name comes from your `native.module`'s init function,
`luaopen_<bridge_name>`):

```lua
local bench = package.loadlib("./build/bench.so", "luaopen_bench")()
print(bench.feature_hash(1000, 42))   -- 8125762261814307938
```

---

## 6. Step 5 — test it

Add `tests/c_driver_<lang>.rs` following the shipped pattern — two tests:

1. **A render assertion** (runs everywhere): run the command and assert the
   generated files exist and contain the expected lines. This catches template
   breakage on any machine.
2. **A toolchain-guarded round-trip** (runs only where the toolchain exists):
   actually build the extension, drive it from the host, and assert the result.
   Discover the toolchain the same way the config does — **PATH first, then
   `~/briev-tools/<tool>-*`** (the per-language downloads live there).

Example of the guard:

```rust
fn has(cmd: &str) -> bool { Command::new(cmd).arg("--version").output().is_ok() }
// skip if `lua` isn't installed
```

---

## 7. Checklist — did you add it right?

- [ ] `lib/glue/<lang>/types.bv` exists (usually `import "glue/c.bv";`).
- [ ] `lib/glue/<lang>/glue.dbvl` has `types_module`, `extension`,
      `bridge_kind`, `calling_convention`, `module_init`, `protocols`, and
      `param_decl`.
- [ ] Every protocol category your bridge uses (`Int`, `Float`, `String`,
      `Bool`, `Data`, `Char`) has a `protocols` entry.
- [ ] Every category has the `native.parse/build/c_type/ret` snippets your
      shim needs (native modules), or you rely on a default.
- [ ] `conversions.to_abi/from_abi` bridge the types that need it.
- [ ] If `module_init: true`, your `native.module` defines the init function
      and calls `__briev_init_state()` once into a global.
- [ ] `tests/c_driver_<lang>.rs` has the render assertion + a guarded
      round-trip.
- [ ] `brievc extension|export|bindings <bridge> <lang>` runs clean.

---

## 8. Copying the right starting point

| You want | Copy | Change |
|----------|------|--------|
| A native C module (Python/Node/Lua-style) | `lib/glue/lua/` | the `native.*` snippets to your host's C API |
| JNI shim + Java class | `lib/glue/java/` | the `native.sig.*` + `native.ret_jni.*` signature forms |
| A cgo Go package | `lib/glue/go/` | `bridge_kind: "cgo_package"`, Go `param_decl`, `templates.*` |
| P/Invoke C# class | `lib/glue/csharp/` | `bridge_kind: "extern_c_crate"`, `bindings.*` |
| A Rust crate | `lib/glue/rust/` | `bridge_kind`, `templates.*` |

The compiler's only rule: **any exception to "config-only" is a bug in the
generic system** — if you find yourself editing Rust to add a language, file it.
