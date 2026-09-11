# The GLUE FFI Architecture — Briev, the central language

**Date:** 2026-08-04
**Status:** Active architecture documentation
**Related:** `docs/guides/ffi-and-export.md` (practical how-to),
`docs/architecture/frgn-export-glue-architecture.md` (deep pipeline),
`learn-briev/07-ffi.md` (tutorial),
`docs/plans/2026-08-04-zero-friction-ffi-gate.md`.

---

## 1. The model

Briev compiles to a native library that any host language calls at **native
speed**. The FFI is:

- **Config-driven.** Every language is a folder `lib/glue/<lang>/` — a `glue.dbvl`
  config file (protocols, ABI, templates, toolchain recipe) + `types.bv`
  (boundary declarations). The compiler carries **zero language knowledge**:
  `briev bindings|export|extension <bridge> <lang>` resolves `lib/glue/<lang>/`
  by name and renders through a single generic pipeline.
- **Protocol-driven.** A boundary type is a protocol variant (`CStr` is
  `String<C_String>`). The casting graph derives the ABI width, LLVM type, and
  marshalling delta from `(protocol, metadata)` — no type-name tables.
- **Composite ABI.** A String/Data crosses the boundary as an **i64 handle** into
  a state-owned `(ptr, len)` region with a **NUL invariant**
  (`bytes[len] == '\0'`). Every host reads it zero-copy in place
  (`C.GoString`, `NewStringUTF`, `lua_pushstring`, `Marshal.PtrToStringUTF8`,
  `PyUnicode_FromString`, `napi_create_string_utf8`).
- **FFI, not a backend.** There is no per-language code-emission compiler. The
  cgo/JNI boundaries are the accepted FFI bounds for Go/Java — the gate proves
  we sit *on* the floor, never above it.

**Mental model:** a host calling `briev.feature_hash(...)` should feel like
calling a super-efficient version of itself. The zero-friction gate (§7) proves
this per host.

**Ownership inference:** every export/frgn parameter and return is classified
as `Borrowed` / `Owned` / `ZeroCopy` / `Value` / `ZeroCost` by the frontend
pass `src/analysis/boundary_ownership.rs` (plan
`docs/plans/2026-08-31-boundary-ownership-inference.md`). The compiler derives
it deterministically from three signals it already holds — protocol variant,
direction (export vs frgn), and calling convention — then propagates it
transitively through the call graph. This makes the boundary's copy-vs-zero-copy
story explicit and checkable, and is the basis for eliminating provably
unnecessary copies (a follow-up wiring phase). Explicit `borrow`/`consume`/
`owned`/`borrowed<source>` annotations (Phase 9 ownership algebra) override the
inference where it cannot see (custom pointer types, opaque C returns, host-GC
bridges).

**Audit:** `brievc ownership <bridge.bv>` prints every boundary's ownership
class — a developer sees immediately whether a String return is `zero-copy`
(host receives the NUL-terminated data pointer, no copy) or `owned` (Briev
holds the arena handle). The `bridge-exports.dbvl` metadata carries it per
export (fields 6/7), so wrapper generators can render ownership-aware
marshalling. The codegen already honors the asymmetry (`String → CStr` emits
`str_to_c`, copy-free; `CStr → String` emits `cstr_to_briev`, the inherent
length-header allocation).

---

## 2. How to link

Build the bridge as a static + shared library:

```bash
brievc build examples/glue-host/bench.bv --library --out build/
#   → build/libbench.a   (real ELF, gcc/rustc-linkable, -O3)
#   → build/bench.so     (PIC shared, for dlopen/ctypes/P/Invoke)
```

The `.a` pipeline runs `opt -passes='default<O3>'` **before** llc — a plain
`llc -O3` in LLVM 18.1.3 does not SROA the transaction loop's allocas.

**The lifecycle (the state is the arena handle):**

| Function | Purpose |
|----------|---------|
| `BrievState* __briev_init_state(void)` | Allocate + init the process-global state; returns its address |
| `void __glue_release(BrievState* state)` | Release (no-op today; arena is process-lifetime) |
| `void __briev_set_cancel(BrievState*, int32_t)` | Raise the process-global cancel flag |
| `void __briev_clear_cancel(BrievState*)` | Clear it |

`__briev_init_state` returns a **module-global** state (not a stack address —
a prior `alloca` version dangled on return; see `BUGS.md`). The library model
is one state per process.

Link from any C-ABI host: `cc ... -Lbuild -lbench`, `g++`, `rustc`/cargo,
cgo (`#cgo LDFLAGS: -L${SRCDIR} -lbench`), P/Invoke
(`[DllImport("bench")]`), JNI (`System.loadLibrary("bench")`).

---

## 3. How to export

### 3.1 The export signature IS the boundary contract

```briev
import "glue/c.bv";                 // the C boundary types + the CStr↔String meld

export defn echo(name: CStr) -> CStr { term name; };
export defn greet(name: CStr) -> CStr {
    let s: String = name;           // meld: CStr → String, no `as`
    term s;                          // meld: String → CStr
};
export defn join(a: CStr, b: CStr) -> CStr { term a + b; };   // cstring_concat
export defn identity(x: CDouble) -> CDouble { term x; };
```

Boundary types (`lib/glue/c.bv`): `CStr` (`String<C_String>`), `CFloat`,
`CDouble` (`Float<C_Double>`), `CI64`, `CI32`, `CBool`, `CChar`, `CPtr`.

### 3.2 Stateful exports

If an export reads/writes a state field, the wrapper carries `ptr %state`. The
decision is a first-class frontend analysis (`compute_export_needs_state`,
`src/analysis/export_abi.rs`) — transitive through calls, and it detects bare
state-field reads even after the marshalling rewrites them into frgn-call args.

```briev
let saved: String = "";
export defn read() -> CStr { term saved; };   // needs_state → takes %state
```

### 3.3 The three commands

| Command | Renders | Example target |
|---------|---------|----------------|
| `briev bindings <bridge> <lang>` | Declarative bindings | C header, C# P/Invoke class |
| `briev export <bridge> <lang>` | A language package/wrapper | Go package, Java class, Rust crate |
| `briev extension <bridge> <lang>` | A **native extension** (compiled shim) | Python `.so`, Node `.node`, Java JNI `lib*.so`, Lua C module |

`briev extension` builds the bridge library, renders the shim from the
language's `native.*` templates, and compiles+links it via the language's
**toolchain recipe** (`native_include_cmd` / `native_suffix` /
`native_link_cmd` / `native_cc` / `native_prefix`) — the compiler only knows
"compile C, link a shared library."

**Shim correctness invariants** (all config/template-driven):
- Every export is declared in the shim as `__briev_export_<name>` with an
  `asm("<name>")` label — an export named like a libc/header function (`read`,
  `open`, `malloc`) would otherwise collide at compile time and be PLT-interposed
  at link time.
- The link adds `-Wl,-Bsymbolic-functions` so the addon binds its own symbols.

---

## 4. How to import

### 4.1 `frgn` — call a foreign function from Briev

```briev
// frgn <C_symbol>(<params>) [-> <ret>] [as <briev_name>] from <source> [fallback <expr>];
frgn __read_file__(path: String) -> Int as briev_read_file_raw
  from "lib/runtime/briev_rt.c" fallback 0;
frgn __write_file__(path: String, data: String) -> Int as briev_write_file
  from "lib/runtime/briev_rt.c" fallback 0;
```

The first identifier is the **C/runtime symbol**; `as` gives the Briev name used
at call sites. `from` resolves to: an inlined C source (`file.c`), a GLUE bridge
target (`.py`/`.mjs`/…), or a linked library (`#Link<name>`). A `fallback`
expression covers "cannot call this" (checked at compile time).

### 4.2 `import` — bring in a boundary module

`import "glue/c.bv"` brings the C boundary types **and** the
`meld CStr -> String` declaration (both survive import resolution), so the
melds and boundary vocabulary of a library apply to the importing bridge.

---

## 5. How to add a new language

> **New to Briev? Start with `docs/guides/add-an-ffi-target.md`** — a complete,
> field-by-field walkthrough with the shipped Lua target as the worked example
> (plus a copy-the-right-folder table and a checklist). The anatomy below is the
> reference summary; the guide is the tutorial.

Adding a language is **config-only** — zero Rust changes (any exception is a
finding about the generic system). Create `lib/glue/<lang>/`:

### 5.1 `types.bv` — boundary declarations

Usually thin: `import "glue/c.bv";`. Declare any extra boundary types the
language needs.

### 5.2 `glue.dbvl` — the target entry

```
<lang>: { types_module: "glue/<lang>/types.bv";
          extension: "<ext>"; bridge_kind: "<...>"; calling_convention: "c_abi";
          module_init: <bool>;
          protocols: { "Int":   { native: "i64";    c_abi: "long long" };
                       "Float": { native: "f64";     c_abi: "double" };
                       "String":{ native: "string";  c_abi: "long long" }; };
          conversions: { to_abi:   { "String": "..."; };
                         from_abi: { "String": "..."; }; };
          state: { decl: "..."; arg: "state"; ffi_type: "" };
          param_decl: "{name}: {type}";
          native_include_cmd: "..."; native_suffix: ".so";
          native_link_cmd: "..."; native_cc: "cc"; native_prefix: ""; };
```

- `protocols` maps each protocol category to the language's **native** type and
  its **C ABI** form. Conversion expressions (with a `{name}` / `result_abi`
  placeholder) bridge them.
- The **toolchain recipe** (`native_*`) is how the extension is compiled — the
  include flags, the output suffix, the link flags, the compiler, and a filename
  prefix (the JVM needs `lib<bridge>.so`).

### 5.3 templates

- **`bindings.*`** — declarative files rendered by `briev bindings`
  (C header, C# class). Whole-file templates use `{{ffi_decls}}`
  (from `bindings.ffi_template`).
- **`templates.*`** — package files rendered by `briev export`, with
  `{{exports}}` (from `fn_template`) for per-export wrappers.
- **`native.*`** — the extension shim rendered by `briev extension`:
  - `native.module` — module boilerplate + init (`PyInit_<bridge>`,
    `NAPI_MODULE_INIT()`, `JNI_OnLoad`, `luaopen_<bridge>`).
  - `native.method` — the per-export method.
  - `native.method_def` — the method-table entry (where the host uses one).
  - per-category `native.parse.<cat>` / `native.build.<cat>` /
    `native.c_type.<cat>` / `native.ret.<cat>` — marshalling snippets
    (tuple-parse for Python, direct-sig for JNI/Lua).
  - JNI-style shims also use `native.sig.<cat>` (direct signature params) +
    `native.ret_jni.<cat>`.

**Renderer variables** per export: `{{name}}`, `{{name_upper}}` (camelCase),
`{{bridge_name}}`, `{{params}}` (param_decl + native types), `{{ffi_params}}`,
`{{args}}`, `{{c_types}}`, `{{args_abi}}` (to_abi), `{{return_expr}}`
(from_abi), `{{return}}`, `{{c_return}}`, `{{s_param}}` (state arg),
`{{s_ffi_param}}`, `{{s_ffi_type}}`, `{{exports}}`, `{{ffi_decls}}`; native
shims add `{{parse_code}}`, `{{ret_c}}`, `{{call}}`, `{{build_code}}`,
`{{sig_params}}`, `{{ret_jni}}`, `{{nargs}}`, `{{nargs_arr}}`. Each is
explained with a real example in `docs/guides/add-an-ffi-target.md` §4.

### 5.4 Test it

`tests/c_driver_<lang>.rs` — a **render assertion** (runs everywhere) plus a
**toolchain-guarded round-trip** (runs where the toolchain exists). The
toolchain discovery pattern: PATH first, then `~/briev-tools/<tool>-*`.

---

## 6. The prepackaged languages + speed table

| Language | Flavor | Command | Round-trip test |
|----------|--------|---------|-----------------|
| C | bindings | `briev bindings <b> c` | `c_driver_boundary` |
| C++ | bindings | same C header (`extern "C"`) | `c_driver_cpp` |
| Rust | bindings | `briev export <b> rust` | `rust-host` crate |
| Python | native ext | `briev extension <b> python` | `c_driver_python` |
| Node | native ext | `briev extension <b> node` | `c_driver_node` |
| Go | cgo package | `briev export <b> go` | `c_driver_go` |
| Java | native ext (JNI) | `briev extension <b> java` + `briev export <b> java` | `c_driver_java` |
| Lua | native ext (C module) | `briev extension <b> lua` | `c_driver_lua` |
| C# | bindings (P/Invoke) | `briev bindings <b> csharp` | `c_driver_csharp` |

### The zero-friction speed table

`feature_hash(count=1000)` — **Briev vs the host writing it natively**
(median ns/call, `benchmarks/bridge/gate/run_gate.sh`; 2026-08-04).

| host | Briev | native | ratio | |
|------|-------|--------|-------|---|
| C | 1098 | 1100 | **1.00** | parity |
| C++ | 1107 | 1094 | **1.01** | parity |
| Java | 1116 | 1122 | **1.00** | parity (JIT) |
| Go | 1189 | 1107 | **1.07** | parity (cgo) |
| Lua | 1162 | 12309 | **0.09** | Briev 11× |
| Python | 1179 | 229794 | **0.01** | Briev 195× |
| Node | 1282 | 190498 | **0.01** | Briev 149× |

Compiled hosts are at parity; **interpreted hosts get Briev's native-machine-code
compute and win by 1–2 orders of magnitude** — "as if Python calls a super
efficient version of itself."

**Dispatch** (Briev `add` vs the host's pure-internal `add`): C 1.07, C++ 1.09,
Lua 1.10, **Python 0.77** (the `METH_FASTCALL` shim dispatches *faster* than
Python's own function call), Node 2.17, Java ~6×, Go ~70×. Node/Java/Go sit at
their structural FFI bounds (NAPI/JNI/cgo — the host's own foreign-call cost).

---

## 7. The zero-friction gate

`benchmarks/bridge/gate/run_gate.sh` builds `bench.bv` and drives every host
whose toolchain is present — **Gate A** (real work: Briev vs native
`feature_hash`) and **Gate B** (dispatch: Briev vs native internal `add`), 3
interleaved rounds, medians. Committed as an opt-in regression canary:
`BRIEV_RUN_GATE=1 cargo test --test gate` (~3 min).

**Gate hygiene** (a fair gate is hard to get right):
- Native functions take a **runtime-varying argument** so no compiler hoists the
  pure call out of the timed loop.
- The native **sink is kept live** — Go's dead-code eliminator stripped a dead
  timed loop into bogus sub-1ns/iter numbers.
- Go's native is measured in a **pure-Go binary** — a cgo-linked binary
  distorted the Go-native numbers.

The gate asserts (generous canaries): every present host's Briev `feature_hash`
< 1.6× native; Python/Lua/Node win by > 1.7×; Python's `METH_FASTCALL` dispatch
stays < 2×. See `docs/plans/2026-08-04-zero-friction-ffi-gate.md`.
