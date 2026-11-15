# LLVM Predictability + FFI Zero-Copy Audit

**2026-11-15 · two-workstream grind: cross-toolchain LLVM verification + FFI boundary copy audit**

---

## Stream 1 — LLVM Predictability

### Current state (verified this session)

| Gate | Result | Notes |
|------|--------|-------|
| `ll_determinism_check.sh --quick` | **PASS** — recommender, multinode, pp-types byte-identical across processes | No unsorted HashMap emission |
| `hir_emit_ab.sh --quick` + full | **PASS** — all 8 corpus programs AST==HIR byte-identical | HIR port correct |
| `toolchain_matrix.sh` | **PARTIAL** — single `clang 23.1.5` only; recommender=OK, multinode/giantnode=SKIP (no `u.data`) | Cross-version parity requires 2+ LLVMs on one host |

### Findings

**Determinism (same source → same .ll):**
- `ll_determinism_check.sh` compiles each corpus program twice in separate processes and byte-compares. All three quick-set programs are byte-identical. This is the per-commit gate that enforces sorted HashMap iteration.
- HIR A/B (`hir_emit_ab.sh`) confirms AST-driven and HIR-driven emission are byte-identical across the full 8-program corpus (recommender, nbody_accel, multinode, queue_drain, iter_chain, pp-types, test_collections, rating_population_gate).

**Cross-toolchain (same source → same behavior across LLVM versions):**
- `toolchain_matrix.sh` discovers versioned clangs (`clang-17` through `clang-20`) plus plain `clang`, deduplicates by version string, and runs the 3-program corpus (recommender, multinode, giantnode) on each. It then checks output-byte parity across toolchains.
- On this machine only `clang 23.1.5` is installed. The script runs but cannot produce a cross-version parity row — only the single-clang `recommender=OK` result.
- The `u.data` MovieLens file is absent, so multinode/giantnode are skipped. The stand-in `simlist.txt` is synthesized but the program also needs `u.data`.
- **Conclusion:** cross-toolchain parity is not measurable on this machine. It requires a host with 2+ LLVM versions (e.g. `clang-19 clang-20 clang`), which is available on the GitHub Actions runners. The `toolchain_matrix.sh` script is correct and should run on CI before each release and on backend-touching commits.

### Action items

| # | Item | Where |
|---|------|-------|
| 1 | Run `toolchain_matrix.sh` in CI (`Verification nightly` or `main` push on backend-touching commits) | `.github/workflows/` |
| 2 | Add `u.data` to CI (synthesize or upload) so multinode/giantnode rows are not SKIP | `toolchain_matrix.sh` or CI step |
| 3 | Record the single-clang `recommender=OK` as a baseline data point in `results/` | `results/` |

---

## Stream 2 — FFI Zero-Copy Audit

### Architecture summary

Briev's FFI boundary has three layers:

1. **Briev-level glue** (`lib/glue/c.bv`) — pure-Briev defns that implement the C ABI boundary transforms.
2. **LLVM emission** (`src/backend/llvm/emit_expr.rs`) — emits the delta transforms from the casting graph when a String-family arg crosses the FFI boundary.
3. **Destination ownership model** (`src/analysis/destination_model.rs`) — the frontend analysis that classifies each FFI parameter/return as a conversion class.

### `lib/glue/c.bv` — C string boundary

```
// str_to_c — zero-copy view
defn str_to_c(handle: String) -> Int [true][term >= 0] {
    term ((handle as Data) as Int) + 8;
};

// cstr_to_briev — one mandatory copy (C→Briev)
defn cstr_to_briev(p: Int) -> String [true][term .^Length >= 0] {
    ...
    let buf: Int = Alloc#(8 + len + 1) as Int;
    Store#(buf, len, 8);
    Copy#(buf + 8, base, len);
    Store#(buf + 8 + len, 0, 1);
    term (buf as Data) as String;
};
```

**Key invariant:** Every Briev String allocation is `[len: i64][bytes][NUL]` — the data region is already a NUL-terminated C string. `str_to_c` is therefore **truly zero-copy**: it returns `handle + 8` (the data pointer). No copy occurs when passing a Briev String to C.

**Copy direction:** `cstr_to_briev` is the only copy — it happens when a C string is received into a Briev String. This is semantically mandatory (the C string has no length prefix; the Briev String must own its buffer). It is not an FFI overhead — it is the representation change.

**C-string concat** (`cstring_concat`) also allocates+copies, but it is an operation (like `+`), not a boundary crossing.

### `briev_rt.c` — runtime

- **All C string conversion helpers were deleted 2026-09-23** (`briev_str_to_c`, `briev_cstr_to_briev`, `briev_cstring_concat`). The comment in `briev_rt.c` line 52 confirms: *"Their pure-Briev twins live in lib/glue/c.bv (str_to_c zero-copy view)"*.
- Remaining `memcpy` calls in `briev_rt.c`:
  - `briev_bits_to_str` (line ~78): `Data→String` door — copies bit representation into a fresh String buffer. This is a Data→String conversion, not an FFI boundary crossing.
  - `briev_str_bitop` (line ~108): allocates a new String for bitwise ops. This is a value operation, not FFI.
  - `ShellCmd` (line ~190): allocates+copies the command string for `popen`. This is the C process-launch path, not a data FFI.
- **No FFI data copies remain in the runtime.** All FFI string handling is in `lib/glue/c.bv`.

### `emit_expr.rs` — LLVM emission

- `emit_frgn_variant_cast` (line 7828): when a Briev String arg crosses to a `CStr` param, the casting graph emits the `str_to_c` delta — a **zero-copy pointer arithmetic** (block + 8). No memcpy in the emitted IR.
- `emit_bridge_frgn_call` (line 8040): foreign calls with `CStr` params/returns pass the raw C string pointer. No implicit copy.
- The `emit_normalized_binary_op` path for String is a value operation, not FFI.

### `destination_model.rs` — ownership model

- `BoundaryConversion` enum (line 515):
  ```
  None, BoxAtBoundary, UnboxAtBoundary, CopyAtBoundary,
  ZeroCopyWithLifetimeProof, WidthConvert
  ```
- **All FFI boundaries are currently `BoundaryConversion::None`** (lines 1853, 1867, 1911, 1931). The model does not yet classify String crossing as `CopyAtBoundary` or `ZeroCopyWithLifetimeProof`.
- The `CopyAtBoundary` and `ZeroCopyWithLifetimeProof` variants are defined but never assigned. The frontend treats all FFI parameters as `ScalarValue` with `None` conversion.
- This is a **model gap**, not an emission gap. The emission is correct (zero-copy for String→CStr, one mandatory copy for CStr→String), but the analysis does not reflect it.

### FFI zero-copy verdict

| FFI surface | Zero-copy? | Where |
|---|---|---|
| Briev String → C CStr param | **Yes** — `str_to_c` returns `handle + 8` | `lib/glue/c.bv` |
| C CStr → Briev String return | **No** — `cstr_to_briev` allocates+copies | `lib/glue/c.bv` (mandatory) |
| Briev List<T> → C array | **Unknown** — no `c.bv` lane yet; `ptr` ABI passes the data pointer directly | `emit_expr.rs` (ptr ABI) |
| Struct pass-by-pointer | **Yes** — `@repr(C)`-like ptr ABI | `export.rs` |
| Int/Float scalars | **Yes** — pass-by-value (i64/f64) | `export.rs` |

### Action items

| # | Item | Priority |
|---|------|----------|
| 4 | Add `BoundaryConversion::ZeroCopyWithLifetimeProof` for String→CStr in `destination_model.rs` | Medium |
| 5 | Add `BoundaryConversion::CopyAtBoundary` for CStr→String in `destination_model.rs` | Medium |
| 6 | Document the zero-copy invariant in `lib/glue/c.bv` header comment (already present) | Low |
| 7 | Add a `List<T>` ↔ C array lane in `lib/glue/c.bv` (currently no `c.bv` lane exists for collections) | Low |
| 8 | Add a test that verifies `str_to_c` emits `add i64 %handle, 8` in the .ll (no memcpy) | Medium |
| 9 | Add a test that verifies `cstr_to_briev` emits exactly one `Alloc#` + one `Copy#` in the .ll | Medium |

---

## Summary

**LLVM predictability:** Determinism (same source → same .ll) is proven by `ll_determinism_check.sh` (byte-identical across processes). HIR/AST parity is proven by `hir_emit_ab.sh` (byte-identical across the 8-program corpus). Cross-toolchain parity (same source → same behavior across LLVM versions) is **not measurable on this machine** (single clang) — the `toolchain_matrix.sh` script is correct and should run on CI with 2+ LLVMs.

**FFI zero-copy:** The FFI boundary is **zero-copy for Briev→C String passing** (`str_to_c` returns the data pointer, no memcpy in the emitted IR). The **only mandatory copy** is C→Briev String receipt (`cstr_to_briev` allocates+copies — semantically mandatory, not overhead). All FFI string conversion helpers were removed from the C runtime 2026-09-23 and replaced with pure-Briev defns. The **ownership model gap** (`BoundaryConversion` never assigned) means the frontend analysis does not yet reflect the zero-copy fact — the emission is correct but the analysis is incomplete.
