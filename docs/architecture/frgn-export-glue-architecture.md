# Metropolitan FFI — frgn / export / GLUE Architecture

**Date:** 2026-07-22
**Status:** Stable — Phase 8 complete
**Applies to:** All backends, `src/glue/`, `src/compile.rs`, `src/ast/top.rs`,
  `src/parser/definitions.rs`, `lib/glue.toml`, `lib/pp/`, `src/library.rs`

---

## Overview

Briev's **Metropolitan FFI** architecture has three pillars:

| Pillar | Direction | Purpose |
|--------|-----------|---------|
| **`frgn`** | Host → Briev | Declare an external function so Briev can call it |
| **`export`** | Briev → Host | Expose a Briev function so foreign code can call it |
| **GLUE** | Broker (compile-time) | Negotiate protocol paths, meld data, generate bridge code |
| **Metropipe** | Runtime | Shared memory IPC between running processes |

GLUE and Metropipe are the two mechanisms under the Metropolitan FFI umbrella.
GLUE handles **compile-time bridge generation** — it's what runs when you
invoke `briev export`. Metropipe handles **runtime shared memory** — it's
what runs when two processes communicate through a `MetropolitanChannel`.

**Protocol types** drive the boundary between languages. See
`docs/architecture/protocol-types.md` for the full explanation of hashwords,
CastTo/CastFrom, and protocol path BFS.

The backend decides *how* to implement each frgn or export. `.c` sources get
compiled and LTO-inlined by LLVM. `.py` sources cannot be inlined — GLUE
mediates through protocol negotiation and zero-copy melds. The frontend
resolves the dispatch path (inline vs bridge vs error) during the main
compilation pass, before the backend runs.

**GLUE is an ABI generator, not an FFI.** It computes the cheapest transform
chain between any two language representations at compile time, emits the
transforms, and eliminates the boundary when the path is identity (at LTO).
See `docs/architecture/glue-as-abi-generator.md` for the full conceptual
framework.

---

## 1. `frgn` — Foreign Function Import

### 1.1 Syntax

```briev
frgn <foreign_symbol>(<params>) [-> <ret>] [as <briev_name>] from <source> [fallback <expr>];
```

`frgn` is an **import**. The first name after `frgn` is the **foreign/C
symbol name**. The `as` clause gives the Briev-side name (what callsites use).
`from` is **required** — every frgn must specify its provenance.

Examples:
```briev
// C symbol is "__getenv_briev", Briev calls it "frgn__getenv_briev"
frgn __getenv_briev(key: String) -> String as frgn__getenv_briev
  from "lib/runtime/briev_rt.c" fallback "";

// No as clause: foreign and Briev names are the same
frgn briev_cstr_to_briev(ptr: Int) -> String as cstr_to_briev
  from "lib/runtime/briev_rt.c" fallback "";

// With compiler-resolved registry path:
frgn XXH64(data: Int, len: Int, seed: Int) -> Int as frgn__xxh64
  from <xxhash.c> fallback 0;
```

### 1.2 Grammar

```
frgn_decl ::= "frgn" identifier "(" [param_list] ")" ["->" type]
              ["as" identifier]
              "from" (string_literal | "<" identifier ">")
              ["fallback" (expr | identifier "(" [arg_list] ")" | ";")]
              ";"
```

### 1.3 Naming Convention

Raw FFI declarations visible to Briev code use the `frgn__` prefix. When the
C symbol differs, `as` provides the mapping:
```briev
frgn XXH64(data: Int, len: Int, seed: Int) -> Int as frgn__xxh64 from "lib/xxhash.c" fallback 0;
```

The `frgn__` naming is a convention, not enforced by the compiler. It makes
FFI boundaries visually distinct from pure Briev function calls.

### 1.4 Fallback

Every frgn must have a `fallback` clause (parser requires it). Three forms:

| Form | Example | Behavior |
|------|---------|----------|
| Static | `fallback 0` | Return zero-initialized value on failure |
| Function | `fallback default_val()` | Call a Briev function as fallback |
| Implicit | `fallback;` | Skip the call, return zero-value |

The fallback fires when the contract is violated (postcondition check on the
return value) or when the foreign call cannot be completed.

---

## 2. Backend Extension Dispatch

### 2.1 The Decision Tree

```
frgn x(args) -> Ret as sym from "path.ext"
                      │
                      ▼
  ┌─ Extension is inlineable? (.c, .rs, .so) ───────┐
  │ YES → Emit direct call, compile & link source    │
  └──────────────────────────────────────────────────┘
  │
  ┌─ Extension maps to a GLUE target? (.py, .js) ────┐
  │ YES → Compute protocol path for each param        │
  │     → Emit CastTo/CastFrom transform chain        │
  │     → Emit bridge call + fallback                 │
  └──────────────────────────────────────────────────┘
  │
  └─ NO → Compile error: unsupported extension
```

### 2.2 Extension Resolution

The `extension_to_language` function was **removed** — it was dead code. The
only lookup is `find_language_by_extension()` which iterates the loaded
`glue_targets` map (from `lib/glue.toml`). Adding a new language = adding a
section to the TOML — zero Rust changes.

### 2.3 The `ResolvedFrgn` Enum

Resolved during the main compilation pass (in `src/compile.rs`), before the
backend runs:

```rust
pub enum ResolvedFrgn {
    /// Backend compiles source and calls the symbol directly
    Inline { symbol: String, compile_source: bool },
    /// Route through GLUE bridge with protocol paths
    Bridge {
        language: String,
        param_paths: Vec<ProtocolStep>,     // one per parameter
        return_path: Option<ProtocolStep>,  // one for return
        fallback: Fallback,
    },
    /// Compile error
    Unsupported(String),
}
```

### 2.4 Protocol Path Resolution (Phase 8)

For each parameter and return type in a bridged frgn, `resolve_single_frgn`
now calls `compute_protocol_path()`, which uses the BFS in
`find_cast_path()` (via TypeUniverse):

```
Source = Briev String
Target = Foreign *mut u8

BFS finds: [String, Bits, *mut u8]
  ├─ String → Bits: Cast(Bits) — identity (same byte width)
  └─ Bits → *mut u8: Bitcast — LLVM bitcast instruction

Cost: Bitcast = low (one instruction, inlined)
```

The BFS always has `Bits` as a fallback (every type can Cast to `Bits`).
If no protocol path is available, the compiler emits a `bitcast` instruction
as the last resort.

The `ProtocolStep` types:

```rust
pub struct ProtocolStep {
    pub source: Type,
    pub target: Type,
    pub kind: TransformKind,
}

pub enum TransformKind {
    Identity,                 // No transform needed
    Bitcast,                  // Raw bitcast — Cast(Bits)
    MeldShuffle,              // Field reordering
    ProtocolTransform(String), // CastTo/CastFrom via category
}
```

---

## 3. GLUE — Bridge Generation

### 3.1 What GLUE Does

GLUE mediates between Briev and a foreign language when the backend cannot
inline the foreign code directly. It:

1. **Negotiates protocol paths** — BFS via `find_cast_path()`
2. **Emits transforms** — `emit_protocol_chain()` in `src/glue/bridge.rs`
3. **Generates bridge calls** — `emit_bridge_frgn_call()` with fallback
4. **Applies fallbacks** — phi-node structure with contract verification

### 3.2 `emit_protocol_chain` Implementation (Phase 8)

Located in `src/glue/bridge.rs`. Transforms a value through a chain of
`ProtocolStep`s, emitting LLVM IR for each kind:

| Kind | IR emitted |
|------|-----------|
| `Identity` | No instructions |
| `Bitcast` | `%r = bitcast T1 %val to T2` |
| `MeldShuffle` | `%r = bitcast T1 %val to T2` (fallback; full extractvalue/insertvalue deferred) |
| `ProtocolTransform(cat)` | `%r = call T2 @_CastTo_#cat(T1 %val)` |

The signature changed from the Phase 3 stub:

```rust
// Before (stub — returned value unchanged):
emit_protocol_chain(value_reg, path, value_ty) -> Result<String, String>

// After (emits real IR):
emit_protocol_chain(out, value_reg, path, value_ty, gen_reg) -> Result<String, String>
```

### 3.3 The Bridge Call (Phase 8)

`emit_bridge_frgn_call()` in `src/backend/llvm/emit_expr.rs` is fully wired:

```
For each argument:
  1. emit_expr(arg) — evaluate the expression
  2. emit_protocol_chain(out, reg, path_for_arg, llvm_type, gen_reg)
  3. Collect transformed args

Emit bridge call: call @bridge_{sym}(transformed_args...)

For return value:
  1. emit_protocol_chain(out, v, return_path, ret_llvm, gen_reg)

Wrap with fallback:
  1. emit_fallback_llvm(out, final_reg, ret_type, ret_llvm, fallback, indent, gen_reg)
```

The fallback dispatch emits a phi-node structure:

```llvm
%ok = icmp ne i64 %result, zeroinitializer
br i1 %ok, label %use_result, label %use_fallback

use_fallback:
  %fb = i64 zeroinitializer
  br label %merge

merge:
  %final = phi i64 [%result, %use_result], [%fb, %use_fallback]
```

---

## 4. `export` — Briev Dressed Up as the Foreign Language

### 4.1 Syntax

```briev
export defn <name>(<params>) -> <ret> { <body> };
```

`export` is a straight keyword before `defn`. It marks the function as
externally visible, generating a `dso_local` symbol and producing a
language-specific wrapper in the export output.

### 4.2 The Export Pipeline (Phase 8)

`briev export <bridge.bv> <language> --out <dir>`

```
1. Parse + typecheck bridge.bv (with import resolution)
2. Extract bridge info (exports, frgns, melds)
3. Find language target in lib/glue.toml (TOML-driven, no hardcoded lookup)
4. Generate LLVM IR via FULL backend (LlvmBackend::generate)
   → Real function bodies (no 'ret i64 0' stubs)
5. Write bridge.ll
6. Compile to .o via llc
7. Generate language wrappers from TOML templates
   → {{mustache}} substitution with {{exports}}, {{ffi_decls}}, etc.
8. Write metadata (.dbvl)
```

**Key: Step 4 uses the full LLVM backend** (the same as `briev build --llvm`),
not the stub generator from `library.rs`. The `library.rs` `generate_with_exports`
function is no longer called by the export CLI.

### 4.3 Template System

Each language in `lib/glue.toml` provides templates with `{{mustache}}`
substitution. No Rust code knows about specific languages.

Template variables:

| Variable | Source | Description |
|----------|--------|-------------|
| `{{bridge_name}}` | CLI arg | Name of the bridge |
| `{{name}}` | Export | Function name |
| `{{params}}` | `type_map` | Language-native parameter list (`n: String`) |
| `{{ffi_params}}` | `c_type_map` | C ABI parameter list (`n: *mut u8`) |
| `{{c_types}}` | `c_type_map` | C ABI types only (`*mut u8, i64`) |
| `{{args}}` | Export | Argument names only (`n, s`) |
| `{{args_abi}}` | `conversions.to_abi` | ABI-converted arguments (`n as i64, s`) |
| `{{return}}` | `type_map` | Language-native return type |
| `{{c_return}}` | `c_type_map` | C ABI return type |
| `{{return_expr}}` | `conversions.from_abi` | Return value conversion |
| `{{s_param}}` | Convention | State variable prefix (`_STATE, ` or `""`) |
| `{{s_init}}` | Convention | State initialization code |
| `{{exports}}` | Generated | Per-function wrappers (from `fn_template`) |
| `{{ffi_decls}}` | Generated | FFI declarations (from `ffi_template`) |

Per-language conversion expressions (`conversions`):

```toml
[rust.conversions.String]
to_abi = "{name}.as_ptr() as i64"
from_abi = "String::from_raw_parts({name} as *mut u8, len)"
```

### 4.4 Generated Output

| Language | Files |
|----------|-------|
| Rust | `Cargo.toml`, `build.rs`, `src/lib.rs`, `src/ffi.rs`, `bridge.ll`, `bridge.o` |
| Python | `__init__.py`, `bridge.ll`, `bridge.o` |
| Node | `index.mjs`, `bridge.ll`, `bridge.o` |

The state parameter (`ptr %state`) is automatically added to all exported
functions by the LLVM backend. The generated wrappers allocate a state buffer
and initialize it via `init_state()`:

```rust
// Generated src/lib.rs
static STATE: *mut c_void = std::ptr::null_mut();

fn init_state() {
    let buf = alloc_zeroed(...);
    ffi::init_state(buf as *mut c_void);
    STATE = buf;
}

pub fn briev_pp_type_bits(n: *mut u8) -> *mut u8 {
    unsafe { ffi::briev_pp_type_bits(STATE, n) }
}
```

---

## 5. The TOML Registry

### 5.1 `lib/glue.toml`

The GLUE registry is fully language-agnostic. Language entries are collected
via `#[serde(flatten)] HashMap<String, LanguageEntry>` — no named fields in
Rust code. Adding a language = adding a `[lang]` section to the TOML.

Each language section declares:

| Field | Purpose |
|-------|---------|
| `types_module` | `.bv` file declaring foreign type representations |
| `extension` | File extension used for extension→language lookup |
| `bridge_kind` | How to bridge: `"native_module"`, `"esm_module"`, `"extern_c_crate"` |
| `calling_convention` | ABI at the boundary: `"lto"` (LLVM link, zero-cost) or `"c_abi"` (FFI) |
| `type_map` | Briev type → language-native wrapper type |
| `c_type_map` | Briev type → C ABI type for FFI declarations |
| `conversions` | Per-type `to_abi` / `from_abi` conversion expressions |
| `templates` | Output file paths and content with `{{mustache}}` substitution |

### 5.2 Calling Convention: LTO vs C ABI

| Convention | Mechanism | Overhead | Best for |
|-----------|-----------|----------|----------|
| `"lto"` | LLVM links `.ll` + host `.ll` together, inlines boundary | Zero (after inlining) | Rust, C, Zig — any LLVM-compatible host |
| `"c_abi"` | Host loads `.so` via FFI (dlopen), calls C wrapper | Per-call overhead (~6μs) | Python, Node, Java — any non-LLVM host |

The LTO path generates a `.a` static library with `dso_local` symbols.
The foreign build system links it directly; LLVM inlines across the boundary
at LTO time. The C ABI path generates a `.so` shared library loaded via
`libloading`/`ctypes`/`ffi-napi`.

---

## 6. Layout Optimization — "Become the Foreign"

**Status:** Phase 6 (foundation exists, not fully wired).

The protocol system provides the infrastructure for layout optimization.
`find_cast_path()` BFS finds the shortest protocol path. If a meld exists
between a Briev type and a foreign type with identity transform, the boundary
cost is zero. The layout optimizer (proposed) would specialize data layouts
at the boundary to eliminate protocol transforms entirely.

---

## 7. String ABI (Phase 8)

### 7.1 String Format: `[length][data]` (C-compatible)

The LLVM backend stores strings in a C-compatible format:

```
Offset 0: i64 length
Offset 8: data bytes
Offset 8+len: null terminator (for C compatibility)
```

This is the SAME format that `briev_rt.c` uses. The old format had a
`{data_ptr, length, chars}` struct that was incompatible with the C runtime.

### 7.2 Global String Constants

Global string constants use the same format:
```
@str.N = private unnamed_addr constant <{ i64, [N x i8] }> <{ i64 len, [N x i8] c"...\00" }>
```

The handle points to the struct start. `handle[0]` = length, `handle+8` = chars.
All functions that read strings (`emit_load_length`, `emit_copy_data`) access
offset 0 for the length and offset 8 for the data.

### 7.3 Tag Bits

Tags are stored in the bottom 2 bits of the string handle:
- Bit 0: SSO inline (packed data)
- Bit 1: Temporary concat result (safe to free when consumed)

The `briev_str_to_c` function in `briev_rt.c` strips tag bits via `& ~3ULL`
before reading the string data:

```c
char* briev_str_to_c(int64_t handle) {
    int64_t ptr = handle & ~3ULL;  // strip SSO + temp flags
    ...
    int64_t len = *(int64_t*)(uintptr_t)ptr;
    memcpy(c_str, (void*)(uintptr_t)(ptr + 8), (size_t)len);
    ...
}
```

### 7.4 Runtime Helpers

Added as part of the round-trip test infrastructure:

| Function | Purpose |
|----------|---------|
| `briev_cstr_to_briev(const char*)` | C string → Briev string handle (heap-allocated) |
| `briev_str_to_c(int64_t handle)` | Briev string handle → C string (heap-allocated) |
| `briev_free_briev_str(int64_t handle)` | Free a Briev string allocated by `briev_cstr_to_briev` |

---

## 8. Integration Tests

Located in `tests/pp_roundtrip_tests.rs` (gitignored by `tests/` entry).

Tests the full pipeline end-to-end:
1. `briev build pp-types.bv --llvm` → `.ll` with real function bodies
2. `llc` → `.o`, `cc -shared` → `.so`
3. `libloading::Library::new(&so_path)` → load the bridge
4. `func(state, input)` → call via FFI
5. `CStr::from_ptr(ptr).to_str()` → read the result

**8 tests, all passing:**

| Test | What it verifies |
|------|------------------|
| `test_bridge_compiles_to_valid_llvm_ir` | IR has expected exports, typed args |
| `test_bridge_compiles_to_shared_library` | `.so` builds and has valid size |
| `test_bridge_loads_and_resolves` | Symbols resolve at runtime |
| `test_pp_void_via_ffi` | `briev_test_type_void()` returns `"void"` |
| `test_cstr_roundtrip_via_ffi` | `"42"` → cstr_to_briev → str_to_c → `"42"` |
| `test_custom_echo_via_ffi` | Pass-through string works |
| `test_pp_bits_via_ffi` | `"42"` → pp_type_bits → `"Bits(42)"` |
| `test_bits_static_via_ffi` | Static string return works |

---

## 9. CLI Subcommands

### 9.1 `briev export <bridge.bv> <language> --out <dir>`

```
1. Read + parse bridge.bv (with import resolution)
2. Extract bridge info
3. Find language target in lib/glue.toml
4. Generate LLVM IR via full backend (LlvmBackend::generate)
5. Write bridge.ll
6. Compile to .o via llc
7. Generate language wrappers from TOML templates
8. Write bridge-exports.dbvl metadata
```

### 9.2 `briev library <file.bv>`

```
1. Parse + typecheck
2. Generate stub LLVM IR (placeholder bodies)
3. Compile via llc
4. Create .a archive via ar
```

Note: `briev library` uses stub codegen (still in `library.rs`). For real
function bodies, use `briev build --llvm`. The `briev export` command also
uses the full backend.

---

## 10. Key Architecture Decisions (Phase 8)

| Decision | Rationale |
|----------|-----------|
| **TOML-driven templates** | Adding a language = adding a TOML section. No Rust code changes. Zero hardcoded generators. |
| **Full backend for export** | Stub codegen (`ret i64 0`) was useless for actual FFI. Export now produces real bodies. |
| **C-compatible string format** | `[length][data]` is the simplest format that works with both C and LLVM IR. No data_ptr prefix. |
| `#[serde(flatten)]` for config | Dynamic language discovery — no `if let Some(python)` blocks. New languages from TOML only. |
| **State parameter in wrappers** | The LLVM backend allocates state internally. The wrapper just passes a pointer. |
| **Protocol path BFS** | `find_cast_path()` in `layout_optimizer.rs` — always falls back to `Bits` (Cast). |
| **`emit_protocol_chain` with `&mut String`** | The function needs to WRITE IR (not just return `&str`). Extended signature with `gen_reg`. |
