# Compiler-in-Briev — the recipe

**Date:** 2026-08-04
**Status:** Active (P1–P5 complete; the two pilots ship in the compiler)
**Related:** `docs/plans/2026-08-04-compiler-in-briev-dogfood-ffi.md`,
`docs/architecture/glue-ffi.md`, `src/glue/briev_pass.rs`

This is how a Rust compiler pass becomes a Briev program, compiled by `brievc`
itself, and loaded through the GLUE C ABI at runtime — so the compiler
dogfoods the FFI it ships. Two passes are wired this way:

- **`needs_state`** — `lib/compiler/needs_state.bv` → `needs_state_compute`:
  which exported defns carry `ptr %state`. Replaces
  `compute_export_needs_state` at `src/glue/export.rs` and
  `src/backend/llvm/mod.rs` when the pass library is present.
- **`soa_reorder`** — `lib/compiler/soa_reorder.bv` → `soa_reorder_compute`:
  the AoS → SoA field permutation. Replaces `reorder_fields` at
  `src/backend/llvm/mod.rs:1808` when the pass library is present.

## The model

```
Rust (projection) ── tagged Data Briev string ──▶ Briev pass ──▶ i64 result
                                                        │
        src/*_projection.rs                      lib/compiler/*.bv
                                                        │ (dlopen)
        fallback reference ◀── if absent ── src/glue/briev_pass.rs
```

1. **Rust serializes** the pass's input into a tagged Data Briev projection
   (`src/analysis/needs_state_projection.rs`, `soa_projection.rs`). This is the
   long-lived interchange contract: Rust walks the AST, Briev decides.
2. **Briev decides** (`lib/compiler/*.bv`), reading the projection with the
   shared scanner (`lib/compiler/reader.bv`). `when` is an if-guard (not a
   while loop) in both interpreter and backend — all iteration is RECURSION.
3. **build.rs** compiles each pass with a prebuilt `brievc` into
   `target/compiler-in-briev/*.so` and embeds the paths via `cargo:rustc-env`
   (read at compile time with `option_env!` — rustc-env is not a runtime var).
   First build has no `brievc` yet (self-hosted bootstrap) and every pass
   falls back to its Rust reference.
4. **briev_pass.rs** dlopens the `.so` (raw `extern dlopen/dlsym` + libdl),
   resolves `compute(state, proj) -> i64` + `__briev_init_state`, calls through
   the C ABI. The i64's MEANING is pass-specific (needs_state: bitmask;
   soa_reorder: the address of a Malloc'd `[total][idx0]...` permutation
   buffer).
5. **Rust applies** the result; a transition test asserts it equals the
   reference on a corpus.

## Adding a new pass (the recipe)

1. **Projection** (`src/analysis/<pass>_projection.rs`): a
   `serialize_<pass>_projection(items) -> String` emitting section lines
   (`<key> <count> <tokens...>`, one section per line, single spaces). Keep the
   flat/linear shape — the Briev reader scans line-by-line with `:` slices.
2. **Pass** (`lib/compiler/<pass>.bv`): `export defn <pass>_compute(proj: CStr)
   -> Int`, reading the projection via `import "compiler/reader.bv"`.
   - Recursion, not `when`-loops.
   - Substrings via `str_substr` (the runtime `briev_str_substr` frgn — the LLVM
     backend's dynamic String slice returns the whole array, see BUGS.md).
   - Character compares via `char_at` (no allocation; a per-char substr corrupts
     the heap under recursion).
   - No `List` element reads (generic `T`), no String `+` (register collision).
   - `Malloc#` + `Ptr<Int>` indexed stores for output buffers (the i64 ABI).
3. **build.rs**: add a `build_pass` line + a `cargo:rustc-env` var.
4. **briev_pass.rs**: a `LoadedPass` singleton + a `compute_<pass>_...`
   helper that serializes, calls, applies, and falls back to the reference.
5. **Wire the call site** to prefer the Briev path.
6. **Transition test**: assert the Briev result equals the Rust reference
   (`tests/c_driver_needs_state.rs` pattern; or a unit test in the reference
   module using `parse_and_check` + `reorder_fields_briev`).

## Verified facts that shape every pass

- `when cond { body }` is an **if-guard**, not a while loop (interpreter and
  LLVM backend agree). Iteration = tail recursion.
- A **String param / frgn result is an i64 HANDLE** at the boundary; a String
  in a register is a ptr. `::Len` and `==` inttoptr it back
  (`is_semantic_string` + `string_ptr`, emit_expr.rs).
- **Dynamic String slices return the whole array** (emit_expr.rs:992) — use
  `briev_str_substr`. **String `+` codegens a register collision** — avoid.
- **`List<String>` element reads return generic `T`** — avoid collection reads;
  re-scan the projection string instead.
- The pass's C signature takes the **state handle first**
  (`<pass>(state, proj)`) — a stateful export is 2 args, not 1.

## Rust-side codegen rules learned the hard way

- `::Len` on a boxed String panicked (Phase-1b) — `is_semantic_string`
  recovers the binding's declared type. (emit_expr.rs Len arm)
- `let_binding_allocas` leaked across functions (reg numbers rewind) — every
  function start must call `clear_locals()`, not a partial manual clear.
- A let reassigned inside a guard demotes to an alloca AT the assignment site
  (LLVM dominance violation) — `emit_definition` pre-scans and pre-declares
  entry allocas.
- `expr_needs_state` must recurse into wrapping Expr kinds (Cast/MethodCall/
  Reflect/Index/Slice/AddrOf) — a cast-wrapped call made an export STATELESS
  while its call site passed `%state` (opt: "use of undefined value").
- Generic struct layouts were silently zeroed (Ptr width, flexible-primordial
  bytes, Cast.# inheritance) — see BUGS.md for the three-part fix.
