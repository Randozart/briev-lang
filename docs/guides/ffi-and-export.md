# Briev FFI & Export — A Practical Guide

**Date:** 2026-08-04
**Applies to:** the current `glue-host-callable` work (per-language glue
folders, native extensions, the zero-friction gate).
**Architecture reference:** `docs/architecture/glue-ffi.md` — this guide is the
hands-on how-to; the arch doc is the complete method.

Briev compiles to a native library that any language can call at **native
speed** — C, C++, Rust, Python, Node, Go, Java, Lua, C# are prepackaged
(§14). The FFI is **protocol-driven**: Briev has no type layouts, only
adaptive protocols. A boundary representation is a sub-protocol
(`String<C_String>`), a `proto` declaration supplies the transforms, and the
casting graph finds the minimal path between a Briev type and its boundary
representation — emitting the **delta**, not a chain. The composite String
crosses as a pointer into a state-owned NUL-invariant region — zero-copy from
every host.

---

## 1. The mental model

- **The export signature IS the boundary contract.** Declare a function with
  `export defn` and boundary types, and the generated ABI (widths, pointers)
  derives from the protocol graph — no per-language conversion tables.
- **Boundary types are declared in Briev** (`lib/glue/c.bv`): `CStr`,
  `CFloat`, `CDouble`, `CI64`, `CI32`, `CBool`, `CChar`, `CPtr`.
- **Marshalling is ordinary Briev casting.** `name as String` on a `CStr`
  emits the graph's binding call (`cstr_to_briev`); `s as CStr` emits
  `str_to_c`. `+` concatenates strings; `CStr + CStr` uses the variant's own
  `cstring_concat`. The `meld CStr -> String` declaration makes the pair
  interchangeable with no `as` at all.
- **The compiler knows no type names and no language.** Only protocol
  categories and metadata are hardcoded; every language's vocabulary lives in
  per-language Data Briev config (`lib/glue/<lang>/glue.dbvl`).

## 2. Quick start

`examples/glue-host/boundary.bv` is the running example:

```briev
import "glue/c.bv";

export defn echo(name: CStr) -> CStr {
    term name;
};

export defn greet(name: CStr) -> CStr {
    let s: String = name as String;   // CStr → String via cstr_to_briev
    term s as CStr;                    // String → CStr via str_to_c
};

export defn join(a: CStr, b: CStr) -> CStr {
    term a + b;                        // the C_String variant's cstring_concat
};

export defn identity(x: CDouble) -> CDouble {
    term x;                            // CDouble → double ABI
};
```

Build a C-callable static library **and** a PIC shared library:

```bash
brievc build examples/glue-host/boundary.bv --library --out build/
#   → libboundary.a  (gcc/rustc-linkable, real ELF, -O3)
#   → boundary.so    (clang -O3 -flto, for ctypes/ffi-napi)
```

Generate C bindings (a header that declares the exports + the `BrievState`
lifecycle):

```bash
brievc bindings examples/glue-host/boundary.bv c --out build/
#   → build/boundary-bindings/briev_types.h
```

The header resolves the boundary types to C ABI names (`CStr → int64_t`,
`CDouble → double`) and declares:

```c
typedef struct BrievState BrievState;
extern BrievState* __briev_init_state(void);
extern void __glue_release(BrievState* state);
extern void __briev_set_cancel(BrievState* state, int32_t flag);
extern void __briev_clear_cancel(BrievState* state);

int64_t echo(int64_t name);
int64_t greet(int64_t name);
int64_t join(int64_t a, int64_t b);
double identity(double x);
```

## 3. Calling from C

```c
#include "boundary-bindings/briev_types.h"
#include <stdio.h>

int main(void) {
    BrievState* st = __briev_init_state();
    printf("%s\n", (char*)(uintptr_t)echo((int64_t)"hello"));   // hello
    printf("%s\n", (char*)(uintptr_t)greet((int64_t)"hello"));  // hello
    printf("%s\n", (char*)(uintptr_t)join((int64_t)"foo", (int64_t)"bar")); // foobar
    printf("%f\n", identity(3.14));                              // 3.140000
    __glue_release(st);
    return 0;
}
```

```bash
cc -o driver driver.c -I build/boundary-bindings -L build/ -lboundary
```

## 4. Calling from Rust

`examples/glue-host/rust-host/` is a self-contained crate whose `build.rs`
compiles the Briev library with `brievc` and links it:

```bash
cd examples/glue-host/rust-host
BRIEVC=$PWD/../../../target/release/brievc cargo run --release
```

The generated `briev_bindings.rs` exposes plain `extern "C"` functions. The
boundary is a single C-ABI call; measured `feature_hash` runs **within 2.4% of
native Rust** — this is the path for writing compiler-internal components in
Briev without loss of efficiency.

## 5. Calling from Python (ctypes)

```python
import ctypes
lib = ctypes.CDLL("build/boundary.so")
lib.__briev_init_state.restype = ctypes.c_void_p
state = lib.__briev_init_state()
lib.greet.argtypes = [ctypes.c_void_p, ctypes.c_int64]
lib.greet.restype = ctypes.c_int64
print(ctypes.cast(lib.greet(state, ctypes.c_void_p(b"hello").value), ctypes.c_char_p).value.decode())
```

The ~2µs/call is ctypes marshalling (identical for C through ctypes). The
shipped **native Python C-extension** target (§10, `briev extension python`)
removes it entirely — a Python→Briev call is now **faster than Python calling
Python** (§15, Gate B).

## 6. Callbacks (host → Briev → host)

A host can pass a function pointer into Briev; Briev calls it back for
first-level-primitive updates (progress bars, per-item status):

```briev
export defn apply(cb: fn(Int) -> Int, x: Int) -> Int {
    term CallPtr#(cb, x);        // call through the pointer
};
```

```c
int64_t doubler(int64_t x) { return x * 2; }
apply(doubler, 21);              // → 42
```

The generated header declares the parameter as a C function pointer
(`int64_t (*cb)(int64_t)`). `fn(P) -> R` annotations are the boundary contract
for callbacks.

## 7. Cancellation

A host thread can cancel a long-running Briev call:

```briev
txn sum_loop(acc: Int, i: Int, count: Int)
    [i < count && !CancelRequested#()][i == count] -> Int
{
    let na: Int = acc + (i * 3);
    acc = na;
    i = i + 1;
    term acc;
};

export defn cancellable_sum(count: Int) -> Int {
    term sum_loop(0, 0, count);
};
```

```c
pthread_t t;
pthread_create(&t, NULL, canceller, NULL);   // canceller calls __briev_set_cancel(st, 1)
int64_t partial = cancellable_sum(st, 2000000000);   // stops early
```

Polling is **explicit** (`CancelRequested#()` in the loop precondition) — the
compiler never injects checks.

## 8. Extending: adding a language

> **New to Briev? Read `docs/guides/add-an-ffi-target.md`** — the beginner
> walkthrough. It explains the three target shapes (`bindings` / `export` /
> `extension`), every field of `glue.dbvl`, the template system with real Lua
> content, the renderer variables, and ends with a checklist and a
> copy-the-right-folder table.

The FFI is infinitely extensible through per-language glue folders
(`lib/glue/<lang>/`), each a Data Briev config + templates. Zero compiler
changes. The steps:

1. `mkdir lib/glue/<lang>/` with `types.bv` (boundary declarations — usually
   `import "glue/c.bv"`).
2. `glue.dbvl` — the target entry: `protocols` (category → native / C-ABI
   names), `conversions` (`to_abi`/`from_abi` per category), `state`,
   `param_decl`, and the **toolchain recipe** (`native_include_cmd`,
   `native_suffix`/`native_suffix_cmd`, `native_link_cmd`, `native_cc`,
   `native_prefix`).
3. Templates — `bindings.*` (declarative, via `briev bindings`), `templates.*`
   (packages, via `briev export`, with `{{exports}}`), and/or `native.*` (the
   extension shim, via `briev extension`: module, method, per-category
   `parse`/`build`/`c_type`/`ret` snippets; JNI-style shims add
   `native.sig.<cat>` + `native.ret_jni.<cat>`).
4. `tests/c_driver_<lang>.rs` — a render assertion (runs everywhere) + a
   toolchain-guarded round-trip.

Each step is explained field-by-field in `docs/guides/add-an-ffi-target.md`.
The complete anatomy is in `docs/architecture/glue-ffi.md` §5.

## 9. Performance notes

The authoritative numbers are the **zero-friction gate** (§15) — Briev vs each
host writing the same function natively. Quick reference (feature_hash
count=1000, ns/call):

| host | Briev | native | ratio |
|------|-------|--------|-------|
| C | 1098 | 1100 | 1.00 |
| C++ | 1107 | 1094 | 1.01 |
| Java | 1116 | 1122 | 1.00 |
| Go | 1189 | 1107 | 1.07 |
| Lua | 1162 | 12309 | **0.09** |
| Python | 1179 | 229794 | **0.01** |
| Node | 1282 | 190498 | **0.01** |

- The `.a` path runs `opt -passes='default<O3>'` before llc so the emitted loop
  is fully SSA (a plain `llc -O3` in LLVM 18.1.3 did not SROA the transaction
  loop's allocas).
- The boundary is a single C-ABI call; the compute dominates. Interpreted hosts
  get Briev's native-machine-code compute and win by 1–2 orders of magnitude.

## 10. Native Python extension (no ctypes)

`briev extension <bridge.bv> python` generates a CPython C-extension module
that calls the Briev exports directly — no ctypes marshalling layer:

```
$ briev extension rank.bv python --out build/
  Extension: build/rank.cpython-312-x86_64-linux-gnu.so
$ python3 -c "import rank; print(rank.feature_hash(1000, 42))"
```

- The shim is a single `.c`: a `PyInit_<bridge>` module with one method per
  export (`METH_FASTCALL` — the argument tuple is skipped). Per-category
  parse/build snippets (in `lib/glue/python/glue.dbvl`, the python target's
  `native.*` templates) marshal natively — Python `int`/`float`/`str` in,
  native Python values out. String params use `PyUnicode_AsUTF8AndSize`
  (limited API ≥ 3.10); `String` handles are the CStr/Briev pointer.
- The `CStr <-> String` meld (`lib/glue/c.bv`) makes boundary functions
  cast-free: `let s: String = name;` needs no `as`, and the marshalling inserts
  `cstr_to_briev`/`str_to_c` (zero-copy in the String → CStr direction — a
  Briev String's data region IS a nul-terminated C string).
- Adding another language is a config section (templates + protocol mappings)
  — the compiler renders, it never hardcodes a language.
- The composite ABI contract: a String/Data composite crosses as an i64 handle;
  every shim dereferences it to a state-owned `(ptr, len)` region (NUL
  invariant) valid for the state's life; hosts borrow read-only; mutability is
  declared by the meld, never per language. See
  `docs/architecture/casting-protocol.md` and the plan
  `docs/plans/2026-08-03-native-python-meld-composite.md`.

## 11. Per-language glue folders (referenced as config)

Each language's entire interop definition lives in `lib/glue/<lang>/` — a
`glue.dbvl` config file (protocols, ABI, templates, **toolchain recipe**),
`types.bv` (boundary declarations), and an optional `gen.bv` compile-time
plugin escape hatch. `briev export|bindings|extension <bridge.bv> <lang>`
resolves `lib/glue/<lang>/` BY NAME and loads its config; `load_glue_config`
scans the folders for extension routing. The compiler carries zero language
knowledge — the glue folder is data.

The toolchain recipe lives in `glue.dbvl`: `native_include_cmd`,
`native_suffix`/`native_suffix_cmd`, `native_link_cmd`, `native_cc`. The
compiler only does "compile C, link a shared library"; python's
`python3-config` and node's include discovery are config commands.

## 12. Native Node addon + the Python ↔ Node bridge

`briev extension <bridge.bv> node` generates a NAPI `.node` addon (no npm) —
same generic renderer as the Python shim, node's `native.*` templates in
`lib/glue/node/glue.dbvl`:

```
$ briev extension node_bridge.bv node --out build/
  Extension: build/node_bridge.node
$ node -e "const b = require('./node_bridge.node'); console.log(b.save('hi'))"
```

Python and Node have no native binding between them; Briev's composite is their
only common interface. The cross-language test (`tests/c_driver_node.rs`)
proves both directions: Node persists `"hello from node"` via the bridge's
`persist` (runtime file I/O), Python loads it with `load`; then Python
persists `"hello from python"` and Node loads it. Stateful exports (a String
state field read/written across calls) work — several latent backend bugs were
fixed along the way (see `BUGS.md`): the library `__briev_init_state` now
returns a module-global state instead of a dangling stack pointer, state-field
references from exports are no longer eliminated as dead, and stateful exports
keep their `%state` param.

Two shim-level correctness notes: every export is declared in the shim as
`__briev_export_<name>` with an `asm("<name>")` label (a bridge export named
like a libc function — `read`, `open` — would otherwise collide with the host's
prototype at compile time and be PLT-interposed at runtime); and the link adds
`-Wl,-Bsymbolic-functions` so the addon binds its own symbols.

## 13. The gen.bv plugin escape hatch (designed)

If `lib/glue/<lang>/gen.bv` exists, the command can invoke it instead of the
renderer: the pipeline writes a `bridge.dbvl` contract next to the output, the
plugin reads it via `FileRead$`/`ConfigGet$`, generates files via `FileWrite$`,
and runs the toolchain via `ShellCmd$`. Turing-complete generation for anything
the templates can't express. Staged after the config-driven path is proven.

## 14. Shipped language roster (2026-08-04)

Each language is a `lib/glue/<lang>/` folder — config, templates, toolchain
recipe. Zero compiler knowledge; `briev bindings` / `briev export` /
`briev extension` render through the generic pipeline.

| Language | Flavor | Command | Notes |
|----------|--------|---------|-------|
| C | bindings | `briev bindings <b> c` | header, C/C++-compatible (`extern "C"`) |
| C++ | bindings | same C header | g++ round-trip test |
| Rust | bindings | `briev export <b> rust` | cgo-free crate |
| Python | native ext | `briev extension <b> python` | CPython C-extension (no ctypes) |
| Node | native ext | `briev extension <b> node` | NAPI `.node` addon (no npm) |
| Go | cgo package | `briev export <b> go` | `import "C"` + wrappers; String → `C.GoString` |
| Java | native ext | `briev extension <b> java` + `briev export <b> java` | JNI shim (`lib<b>.so`) + class with `native` methods |
| Lua | native ext | `briev extension <b> lua` | C module `luaopen_<b>`; `lua_pushstring` |
| C# | bindings | `briev bindings <b> csharp` | P/Invoke DllImport class |

The composite String crosses every boundary as a pointer into the
NUL-invariant `[len][bytes][\0]` region — zero-copy read from every host
(`C.GoString`, `NewStringUTF`, `lua_pushstring`, `Marshal.PtrToStringUTF8`,
`PyUnicode_FromString`, `napi_create_string_utf8`).

**The speed table** (feature_hash count=1000, median ns/call — see §9 and
`docs/architecture/glue-ffi.md` §6): compiled hosts at parity (C 1.00, C++ 1.01,
Java 1.00, Go 1.07), interpreted hosts won by Briev 1–2 orders of magnitude
(Lua 0.09, Python 0.01, Node 0.01).

## 15. The zero-friction FFI gate

`benchmarks/bridge/gate/run_gate.sh` is the committed regression gate: Briev's
`feature_hash` vs each host's *own* native `feature_hash` (Gate A — real work)
and Briev's `add` vs the host's pure-internal `add` (Gate B — dispatch). Run it
with `BRIEV_RUN_GATE=1 cargo test --test gate`.

**Gate A — Briev vs native feature_hash (median):**
| host | ratio | |
|------|-------|---|
| C | 1.00 | parity |
| C++ | 1.01 | parity |
| Java | 1.00 | parity (JIT) |
| Go | 1.07 | parity (cgo) |
| Lua | 0.09 | Briev 11× |
| Python | 0.01 | Briev 195× |
| Node | 0.01 | Briev 149× |

Compiled hosts are at parity; interpreted hosts get Briev's native-machine-code
compute and win by 1–2 orders of magnitude.

**Gate B — dispatch (Briev add vs native internal add):** Python 0.77 (the
`METH_FASTCALL` shim dispatches *faster* than Python's own function call),
C 1.07, C++ 1.09, Lua 1.10, Node 2.17, Java ~6×, Go ~70×. Node/Java/Go sit at
their structural FFI bounds (NAPI/JNI/cgo — the host's own foreign-call cost).

The gate keeps the native sink live (Go's dead-code eliminator stripped a
dead timed loop → bogus sub-1ns/iter numbers) and measures Go native in a
pure-Go binary (a cgo-linked binary distorted it). See
`docs/plans/2026-08-04-zero-friction-ffi-gate.md` and
`docs/architecture/glue-ffi.md` §7.
