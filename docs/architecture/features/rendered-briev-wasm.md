# Rendered Briev — WASM-First Webstack Architecture v2

**Date:** 2026-07-26
**Status:** Spec — awaiting implementation
**Supersedes:** `docs/plans/2026-06-19-webstack-typescript-wasm-ffi.md` (TS-only emitter)

## Overview

Rendered Briev (`.rbv`) and plain Briev (`.bv`) compile to the web via **WebAssembly + a thin generated JS shim**. The old webstack backend (TypeScript emitter) is replaced by a dual-codegen pipeline: the existing `LlvmBackend` with `wasm32-unknown-wasi` target generates the WASM module, while a new `GlueWebGenerator` emits the minimal JS DOM binding layer.

This is not a "transpile to JS" approach. WASM runs the application logic at near-native speed. The JS shim is a passive reader of WASM's linear memory — it applies DOM mutations that the compiler has proven correct through Briev's contract system.

## Language Variants

| Extension | Syntax | Backend | Output |
|-----------|--------|---------|--------|
| `.bv` | Pure Briev logic | `LlvmBackend(wasm32)` | `*.wasm` + optional `metropipe-shim.js` |
| `.rbv` | Briev + `<view>` + `<style>` + `render` blocks | `LlvmBackend(wasm32)` + `GlueWebGenerator` | `*.wasm` + `dom-shim.mjs` + `*.css` |

A `.bv` compiled with `--backend webstack` produces a logic-only WASM module with no DOM bindings — for web workers, compute kernels, or shared libraries consumed by JS/TS. An `.rbv` produces a full rendered application.

## The `render` Keyword

The `render` keyword attaches view information to types:

```briev
struct Counter {
    count: Int;
    label: String;
};

render struct Counter {
    <div class="counter">
        <span b-text="count">0</span>
        <button b-trigger:click="increment">+</button>
    </div>
};

render obj Observable {
    <div b-each:item="items">
        <span b-text="item">item</span>
    </div>
};
```

- `render struct <name> { <html> }` — attaches a view to an existing `StaticStruct`. The struct fields become state signals; each `txn` on the struct becomes a method.
- `render obj <name> { <html> }` — attaches a view to an existing `obj` type. The obj's methods with contracts become reactive transactions.
- Both desugar into `TopLevel::RenderBlock` (defined at `src/ast/top.rs:1007`) but with richer metadata: struct fields, transaction references, and typed signal bindings.

### Desugaring

```briev
render struct Counter { ... <span b-text="count"> ... <button b-trigger:click="increment"> ... };
```

Desugars into:

1. A `RenderBlock` referencing `Counter` with the view HTML and a binding table:
   ```
   Bindings = [
     Text { element_id: "span:0", signal: "Counter.count" },
     Trigger { element_id: "button:0", event: "click", txn: "Counter.increment" },
   ]
   ```
2. The view compiler (`src/view_compiler.rs`) processes the HTML — unchanged, it already handles all `b-*` directives.
3. The codegen phase uses these bindings to generate the JS shim's binding table.

## Dual-Codegen Contract Architecture

This is the core architectural insight. **Contracts ARE the UI binding.** The compiler proves that after every transaction, the DOM state matches the declared contracts — and generates the exact minimal JS to enforce this, with no polling, no virtual DOM diffing, and no runtime binding framework.

### Pipeline

```
.rbv file
  │
  ├── RbvFile::parse() → briev_source + view_html + style_css
  │
  ├── typecheck + normalize (webstack_normalizer)
  │
  ├── ViewCompiler → Vec<Binding>                    (unchanged)
  │   Each Binding is a contract: "element #X's textContent == count"
  │
  ├── Contract Analysis                               (NEW: contract_bindings pass)
  │   For each txn, compute { modified_state_fields }
  │   For each Binding, compute { depends_on_fields }
  │   → per-txn update set: <txn, [(Binding, field_offset, field_type)]>
  │   The analysis is a compile-time proof, not a runtime system.
  │
  ├── LlvmBackend(wasm32) → app.wasm                  (existing LLVM, target wasm32)
  │   State fields at known offsets in linear memory
  │   Each txn commits state at `term;` → calls `__web_flush_state`
  │   The WASM export table includes a state descriptor:
  │     (export "state_layout" (func ...)) — returns ptr to field offset table
  │   (export "generation" (global i32)) — tick counter, increments after each txn commit
  │
  └── GlueWebGenerator → dom-shim.mjs                 (NEW: src/glue/web_generator.rs)
      Reads from the WASM module's exports:
        state_layout: [{ field_handle: i32, offset: i32, size: i32, type_tag: i32 }]
        generation:   global i32 — JS checks this at init time to know state changed
      Produces:
        dom-shim.mjs — WasmDomRuntime class
        <app>.d.ts   — TypeScript declarations
```

### How the JS Shim Applies DOM Updates

When the WASM module calls `__web_flush_state(updates_ptr: i32, count: i32)`:

1. JS reads the update batch from WASM linear memory at `updates_ptr`:
   ```
   struct Update { field_handle: u32, value_ptr: u32, value_len: u32 }
   ```
2. For each update, JS looks up `field_handle` in its binding table:
   ```
   binding_table[handle] = {
     type_tag: "Text" | "Show" | "Class" | ...,
     element_id: "...",
     apply_fn: (value) => { ... native DOM operation ... }
   }
   ```
3. Applies the DOM mutation synchronously — `element.textContent = decoded_value`, `element.classList.toggle(class, bool)`, etc.
4. Returns control to WASM. Total JS execution time: microseconds per transaction.

The key property: **no JS runs unless a transaction actually commits.** Briev's convergence semantics guarantee that if the pre-condition is false, the transaction body does not execute and `__web_flush_state` is never called. Zero overhead in the idle state.

### Zero Overhead Guarantee

| Scenario | WASM does | JS does | FFI crossings |
|----------|-----------|---------|---------------|
| State unchanged (precondition false) | Transaction skipped | Nothing | 0 |
| State updated, 1 field changed | Runs txn body, calls `__web_flush_state` | Applies 1 DOM mutation | 1 |
| State updated, N fields changed | Runs txn body, calls `__web_flush_state` | Applies N DOM mutations | 1 |
| Canvas/WebGPU frame | Writes pixels to shared memory | Nothing (context handed off) | 0 |

Compare to the old TS emitter: every watcher was a JS `Proxy` or `Object.defineProperty` that ran on every field write, even intermediate values. The WASM approach batches all updates to the transaction commit point — exactly one FFI boundary crossing per transaction, regardless of how many fields changed.

### Interactive State Exports

The whole program state lives in one `@__web_state` global; every `_txn`
export takes `%state` **first**, and the shim passes `__briev_state_ptr()`
so the marshalled input lands in the state before the txn body reads it.
The exported contract:

- `__briev_state_ptr()` — returns the state pointer; the shim marshals each
  transaction's input into the state, then reads the output fields back out
  of the same state (the `_txn` wrapper prepends it to every export's
  signature).
- `__web_boot()` — boot initialization before the first flush.
- `render_frame()` — the flush driver; the JS side calls it in a
  `requestAnimationFrame` loop.
- If the program's observable side effects fold away, `render_frame()` and
  `reactor_tick` are still emitted (a folded program has no state to render,
  but the shim contract stays stable).

Verified end-to-end with a Node harness: an incrementing counter
(`main.count = ...`) stepped 5→6→7→8 across flushes, each with its flush
record.

## Memory Layout as the Bridge

```
WASM Linear Memory:
  ┌──────────────────────────────────┐
  │ generation: u32                  │ ← exported global, incremented after each txn
  │ state_field_0: <type>            │ ← fixed offset, computed at compile time
  │ state_field_1: <type>            │
  │ ...                              │
  │ state_field_N: <type>            │
  │                                  │
  │ flush_buffer: Update[N]          │ ← per-txn write buffer at known offset
  │   [{ handle: u32,               │     field handle (matches binding table)
  │      value_ptr: u32,            │     pointer to value data in linear memory
  │      value_len: u32 }]          │     byte length of value
  └──────────────────────────────────┘
```

The compiler emits a `state_layout` exported function that returns a pointer to a table in linear memory:

```wat
(func (export "state_layout") (result i32)
  ;; returns pointer to StateLayout struct in linear memory
  ;; struct StateLayout {
  ;;   field_count: u32,
  ;;   generation_offset: u32,
  ;;   flush_buffer_offset: u32,
  ;;   max_flush_entries: u32,
  ;;   fields: [FieldLayout {
  ;;     field_handle: u32,
  ;;     offset: u32,
  ///    size: u32,
  ;;     type_tag: u32,   ;; 0=Int, 1=Float, 2=Bool, 3=String
  ;;   }],
  ;; }
)
```

The JS shim reads this table once at initialization and builds its binding table. After that, all communication is through linear memory reads, not function calls — except `__web_flush_state` which is the single FFI import.

## GLUE `[web]` Target

New entry in `lib/glue.toml`:

```toml
[web]
types_module = "glue/web/types.bv"
extension = "mjs"
bridge_kind = "wasm_runtime"
calling_convention = "wasm_import"

[web.protocols]
"Int" = { native = "number", wasm_abi = "i32" }
"Float" = { native = "number", wasm_abi = "f64" }
"Bool" = { native = "boolean", wasm_abi = "i32" }
"String" = { native = "string", wasm_abi = "i32" }
"#Element" = { native = "Element", wasm_abi = "i32" }
```

The `GlueWebGenerator` (new module at `src/glue/web_generator.rs`) reads:
1. The WASM module's `state_layout` export
2. The `Vec<Binding>` from the view compiler
3. The type universe for protocol-to-JS-type mapping

And produces:
- `dom-shim.mjs` — ES module exporting `WasmDomRuntime` that wraps the WASM instance
- `<app>.d.ts` — TypeScript declarations for type-safe consumption

### Webtypes: Opaque Handle-Based Object Marshalling

DOM elements and browser API objects are represented inside WASM as opaque `i32` handles:

```briev
type Element: #System;    // i32 handle in WASM, HTMLElement in JS
type CanvasContext;       // i32 handle, WebGL/WebGPU context in JS
```

The JS shim maintains a flat array mapping handles to real objects:

```javascript
const _web_objects = [null]; // index 0 is reserved (null handle)
```

`frgn` declarations from `#Web` protocol use this handle table:

```briev
frgn create_element(tag: String) -> Element from #Web;
frgn set_text(elem: Element, text: String) from #Web;
```

These compile to WASM imports that the JS shim implements as handle-table lookups:

```javascript
imports.env = {
  create_element: (tag_ptr) => {
    const tag = read_string(wasm, tag_ptr);
    const el = document.createElement(tag);
    return _web_objects.push(el) - 1; // return new handle
  },
  set_text: (elem_handle, text_ptr) => {
    const el = _web_objects[elem_handle];
    el.textContent = read_string(wasm, text_ptr);
  },
};
```

## Canvas/WebGPU Path

When a `.rbv` declares a canvas rendering context, the JS shim performs a one-time handoff and steps back completely:

```briev
frgn get_canvas(id: String) -> CanvasContext from #Web fallback null;
frgn present_frame(ctx: CanvasContext) from #Web;
```

### Initialization Sequence

1. JS shim locates `<canvas id="...">` in the DOM
2. Requests `WebGL2RenderingContext` or `GPUDevice` from the browser
3. Registers the context in the handle table
4. Calls WASM's `init(canvas_handle, width, height)` export
5. Calls WASM's `render_frame()` export inside a `requestAnimationFrame` loop

### After Handoff — Zero JS Overhead

- WASM writes pixel data directly to a shared memory `ArrayBuffer` (the `<canvas>`'s transfer buffer, or a GPU-compatible memory region)
- WASM calls `present_frame` (a no-op in WebGPU's case — the GPU compositor reads the swap chain directly)
- JS side: `requestAnimationFrame` loop simply calls `wasm.exports.render_frame()` — no DOM manipulation, no data marshalling

The `present_frame` import exists only for WebGL's `swapBuffers` equivalent. In WebGPU mode, the WASM module submits command buffers directly to the GPU queue via the handle.

## Webstack Targets in `config/targets.toml`

Updated entries:

```toml
[".bv"]
backend = "llvm"
defaults = ["--budget", "256"]
plugins = ["prelude", "env", "print"]

[".rbv"]
backend = "webstack"
defaults = ["--target", "wasm32"]
plugins = ["prelude"]
```

Additionally, a `.bv` can be compiled for web explicitly:

```bash
brievc build logic.bv --backend webstack --target wasm32-unknown-wasi
```

This routes through `LlvmBackend` with the WASM target triple, producing `logic.wasm` and a minimal `metropipe-shim.mjs` that handles only `frgn from #Web` imports (no DOM binding layer).

## Published Outputs

For a `.rbv` file `app.rbv`, the compiler produces:

| File | Contents | Producer |
|------|----------|----------|
| `app.wasm` | Compiled Briev logic, WASM32 | `LlvmBackend(wasm32)` |
| `dom-shim.mjs` | ES module: `WasmDomRuntime` class + handle table + state binding | `GlueWebGenerator` |
| `app.d.ts` | TypeScript declarations for `WasmDomRuntime` exports | `GlueWebGenerator` |
| `app.css` | Extracted `<style>` block (passthrough) | `RbvFile::parse()` |

For a `.bv` file `logic.bv` compiled with `--backend webstack`:

| File | Contents | Producer |
|------|----------|----------|
| `logic.wasm` | Compiled Briev logic, WASM32 | `LlvmBackend(wasm32)` |
| `metropipe-shim.mjs` | Minimal import stubs for `frgn from #Web` | `GlueWebGenerator` |

## Relationship to Existing Systems

| System | What it provides | How this spec uses it |
|--------|-----------------|----------------------|
| `LlvmBackend` (wasm32) | WASM binary codegen with pointer width handling | The main codegen path — `.bv`/`.rbv` logic → WASM |
| `ViewCompiler` | HTML b-* directive parsing → `Vec<Binding>` | Unchanged — feeds binding table to shim generator |
| `RbvFile::parse()` | .rbv parsing → briev_source + view_html + style_css | Unchanged — entry point for .rbv compilation |
| `GlueEngine` (src/glue/) | Protocol-based FFI bridge generation | Extended with `[web]` target in `lib/glue.toml` |
| `Metropipe` (src/ffi/) | Shared memory IPC code generation | Not used — WASM linear memory replaces it for web |
| `WebstackGenerator` (old) | TS App class emitter | **Replaced** — no longer generates TypeScript |
| `WebstackOutput` | TS + Rust + JS glue strings | **Replaced** — output is `.wasm` + `.mjs` + `.d.ts` + `.css` |

## Backward Compatibility

The `rstruct` keyword (old syntax, `Token::Rstruct`) is preserved in the lexer but deprecated. The parser emits a deprecation warning: `"rstruct is deprecated, use 'render struct'"`.  
The `WebstackGenerator` in `src/backend/webstack.rs` is kept for the migration period but its codegen path is redirected to the new pipeline (no TS emission).

Existing `.rbv` files using the old format (`<script type="briev">` wrapper, view-only without `render` keyword) continue to parse correctly via `RbvFile::parse()` backward compatibility logic.

## See Also

- `docs/plans/2026-07-26-rendered-briev-webstack-v2.md` — Implementation plan
- `src/view_compiler.rs` — View directive parsing (unchanged)
- `src/rbv.rs` — `.rbv` file parser (unchanged)
- `src/glue/web_generator.rs` — (NEW) JS shim generator
- `lib/glue.toml` — Target registry, updated with `[web]` entry
