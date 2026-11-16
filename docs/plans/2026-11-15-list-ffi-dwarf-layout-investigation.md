# List<T> ↔ C Array FFI: DWARF Layout Investigation

**Date:** 2026-11-15
**Plan:** `docs/plans/2026-11-15-llvm-predictability-and-ffi-zero-copy.md` §7
**Mode:** PLAN (research-first)

## 1. What I found

### 1.1 Existing layout mechanisms in Briev

| Mechanism | What it does | FFI completeness |
|---|---|---|
| `pack struct` | Bit-contiguous sub-byte or whole-byte packing with optional `spec Endian` | Complete for structs, no stride/array support |
| `coll obj List<T>` | Compiler appends hidden `cap`/`len` slots; data lives in `ListBuffer<T>.data: Ptr<T>` | Unknown for FFI |
| `coll struct Fixed<T,N>` | Fixed-size inline array, no separate heap block | No explicit FFI lane |
| `seq coll` | Forces one contiguous block for element storage | Forces contiguity, not a layout declaration |
| `dwarf.bv` | Runtime C `sizeof`/`offsetof` probing + `InjectTypeLayout$` | Runtime probe, not compile-time metadata |

### 1.2 DWARF's type-description model (what the user is pointing at)

DWARF (Debugging With Attributed Record Formats) describes memory layout using formal type descriptors, not just debug metadata. Key properties:

```
DW_TAG_array_type
  DW_AT_type → element type
  DW_AT_byte_stride → byte distance between consecutive elements
  DW_AT_alignment   → ABI alignment requirement
  DW_AT_order       → row-major vs column-major

DW_TAG_structure_type
  DW_AT_byte_size   → total byte size
  DW_AT_data_member_location → offset of each field
  DW_AT_alignment   → struct alignment
  (no DW_AT_padding — it's implicit from alignment, not declared)
```

**Key insight from DWARF:** The `byte_stride` attribute separates the **element size** from the **element spacing**. For packed arrays where stride = element size, the attribute is redundant; for arrays with padding between elements, stride > element size. DWARF can describe both precisely.

### 1.3 Current List<T> memory layout

```briev
struct ListBuffer<T> {
    data: Ptr<T>;         // pointer to element 0
}
coll obj List<T> {
    inner: ListBuffer<T>; // compiler appends cap, len at known offsets
}
```

The boxed handle (the thing passed around) is:
```
Word 0: data pointer (handle + 0 + 0)
Word 1: capacity     (handle + 0 + 8)  ← compiler-hidden
Word 2: length       (handle + 0 + 16) ← compiler-hidden
```

The data buffer is a separate heap allocation. The C FFI pointer to pass is `(ListBuffer.data, List.len)` — not the boxed handle itself.

### 1.4 What `seq coll` actually means

From `docs/architecture/iterable-protocol.md` §2:
> `seq coll` forces the elements into one contiguous memory block — for a growable `Ptr<T>` coll the data buffer IS one block; for a fixed `T[N]` coll it forbids the columnar layout.

`seq` forces element contiguity (no gaps between elements in the buffer), but:
- It does NOT declare a canonical ABI layout
- It does NOT declare an element stride
- For `List<T>`, the data buffer is already contiguous — `seq` doesn't change anything
- For fixed-size `T[N]`, `seq pack coll struct` gives one packed contiguous block

### 1.5 Existing dwarf.bv approach

`lib/std/dwarf.bv` does C struct layout probing at runtime:
- Generates a C program with `sizeof`/`offsetof`
- Compiles and runs it
- Parses output for layout info
- Used with `InjectTypeLayout$`

This is a **runtime probe** approach, not a DWARF metadata approach. It discovers layout after compilation, which is useful for reverse-engineering C headers but not for declaring Briev layouts upfront.

### 1.6 What the compiler already knows

From `src/type_universe/packed.rs`: the compiler has a **complete, formal, single-authority layout derivation** for `pack struct` (`packed_field_offsets`, `packed_total_bits`, `packed_bytes`, `is_whole_byte_packed`). This is already DWARF-like in its formal precision — it's a pure function of declared fields that produces exact byte/bit offsets.

The gap is that this is **struct-centric**, not **array/collection-centric**. For FFI arrays, you need stride (not just element size) and alignment.

## 2. Three approaches evaluated

### Approach A: Infer from `seq coll` + element type (current implicit path)

The compiler infers: "this coll has `seq` so elements are contiguous → pass `(data_ptr, len)` to C."

**Efficiency:** Zero-copy (no memcpy). The data pointer already points to the element buffer.

**Completeness:** Works for `List<T>` where stride = element size. Breaks when:
- A user wants padding between elements (stride > element size)
- A custom coll uses columnar layout (multiple buffers)
- The element type itself has padding

**Gap:** No formal declaration. The compiler guesses. If the user's intent doesn't match the guess, the FFI silently misbehaves.

### Approach B: Explicit `spec ABI: Array<elem, stride, align>` declaration

Add a `spec ABI` clause to the coll declaration:

```briev
// Briev declaration
coll obj CArray<T> {
    spec ABI: Array(elem: T, stride: sizeof(T), align: alignof(T));
    inner: ListBuffer<T>;
}

// Compiler generates: when passed to C, emit:
//   param 0: data_ptr  (ListBuffer.data)
//   param 1: len       (List.len)
//   stride is checked but equals elem_size for this coll type
```

**Efficiency:** Zero-copy with explicit guarantee. The `spec ABI` is a formal contract.

**Completeness:** Full DWARF-style descriptor. Handles stride > element_size, explicit alignment, and arbitrary element types.

**Gap:** New syntax. More verbose. Requires the user to know the C ABI details.

### Approach C: Compiler auto-emits ABI spec for FFI-crossing colls

When a `coll obj` crosses an FFI boundary, the compiler auto-generates the ABI descriptor from the known layout:
- Elements are in `ListBuffer.data`
- Count is the hidden `len` slot
- Stride = `sizeof(T)` (elements are contiguous)
- Alignment = natural alignment of `T`

**Efficiency:** Zero-copy, zero declaration. The compiler knows the layout.

**Completeness:** Handles the common case. Breaks when the user wants non-standard stride (padding between elements).

**Gap:** Implicit. No explicit contract. If the compiler's inference ever differs from the user's expectation, no error is raised at the boundary.

## 3. Recommendation

**Approach C (compiler auto-emit) for `List<T>` → C arrays** is the most efficient default:

1. The compiler already knows `List<T>`'s layout (boxed handle + data pointer + len slot)
2. `seq coll` already guarantees element contiguity
3. The FFI zero-copy path `(data_ptr, len)` requires only extracting existing fields — no copy, no allocation
4. For the common case (contiguous elements, native stride), no declaration is needed

**Approach B (explicit `spec ABI`) as the opt-in mechanism** for non-standard cases:

```briev
// For a coll with padding between elements:
coll obj PaddedVec<T> {
    spec ABI: Array(elem: T, stride: 32, align: 16);
    inner: ListBuffer<T>;
}
```

The `spec ABI` declaration formally communicates the FFI layout contract. The compiler verifies:
- `stride >= sizeof(T)` (stride can't be smaller than element size)
- `align` is a power of 2

## 4. What this means for task #27

The `List<T>` ↔ C array FFI lane is already zero-copy in the common case:

```
List<T>              → C function(ptr, len)
─────────────────────────────────────────
handle = boxed list
data = Load#(handle, 0)   // ListBuffer.data offset = 0
len  = Load#(handle, 16)  // len slot offset = 16 (after cap at 8)
call C_function(data, len)  // zero-copy — C borrows the buffer
```

No memcpy. No new allocation. Just extract the two known slots.

**The actual task #27 work** is:
1. Implement a `list_to_c_ptr(list) → Int` helper in `lib/glue/c.bv` (mirrors `str_to_c`):
   ```briev
   defn list_to_c_ptr(list: List<T>) -> Int {
       term ((list as Data) as Int) + 24; // skip [cap][len][data_ptr] header
   }
   ```
   Wait — let me verify: `ListBuffer` has `data: Ptr<T>` at offset 0 within the boxed handle. The boxed handle itself is `[cap(i64 at +0), len(i64 at +8), data_ptr(i64 at +16)]`. So `handle + 24` is NOT the right offset.

   Actually: the boxed handle IS the `[cap, len, data]` block. `data` is at offset 16. So `list_to_c_ptr` should be:
   ```briev
   defn list_to_c_ptr(list: List<T>) -> Int {
       term ((list as Data) as Int) + 16; // skip [cap at +0, len at +8], point to data at +16
   }
   ```

2. Add `list_len(list) → Int`:
   ```briev
   defn list_len(list: List<T>) -> Int {
       term ((list as Data) as Int + 8) as Int; // load len at offset +8
   }
   ```

3. Add `BoundaryConversion` classification in `destination_model.rs`:
   - `List<T>` → C array param: `CopyAtBoundary` (need to check if data buffer needs copying — actually, for a borrow/const read, `CopyAtBoundary` is appropriate; for a mutable buffer, `ZeroCopyWithLifetimeProof`)
   - Actually: for C array params, the data pointer is passed by value — C borrows. No copy needed. But the C function might write to the buffer, which means we need `ZeroCopyWithLifetimeProof` for mutable, `ZeroCopyWithLifetimeProof` for immutable (C just reads). The question is whether C's writes should be reflected back.

   For `String → CStr`, the result is `ZeroCopyWithLifetimeProof` because C doesn't modify the string (it's `const char*`). For `List<T> → C array`, the same applies: `ZeroCopyWithLifetimeProof` for const read. For mutable write, we'd need to handle it differently.

4. Write the integration test (mirrors `str_to_c_no_memcpy.rs`):
   - Build a fixture that imports `glue/c.bv`, calls a C function with `(list_to_c_ptr(l), list_len(l))`
   - Assert the emitted `.ll` contains the pointer arithmetic at `+16` offset

## 5. Open question: explicit layout syntax

**Should Briev have explicit layout declaration syntax beyond `seq`?**

The SPEC's "no canonical layout" principle applies to **internal representation** — layout derived from operations. For **FFI boundaries**, explicit layout declaration is valuable because:
- It formally documents the ABI contract
- The compiler can verify the declared layout is ABI-compatible
- For common cases (contiguous array, stride = element size), the compiler auto-infers without declaration

Proposed syntax using existing `spec` mechanism (no new keyword needed):

```briev
// Option 1: extend spec for ABI declaration
coll obj CArray<T> {
    spec ABI: Array(elem: T, stride: sizeof(T), align: alignof(T));
    inner: ListBuffer<T>;
}

// Option 2: use existing pack for contiguous, explicit stride
pack coll obj StridedVec<T> {
    spec Stride: 32;
    inner: ListBuffer<T>;
}
```

The existing `spec Endian: Big` pattern (`pack struct`) is the precedent. Extending `spec` for ABI declarations is consistent.

For now (task #27): implement the implicit zero-copy path for `List<T>` → C arrays using the known layout. Add explicit `spec ABI` as a follow-up if the need for non-standard stride arises.

## 6. References

- `lib/std/dwarf.bv` — existing C struct layout probing
- `src/type_universe/packed.rs` — formal packed layout derivation (single authority)
- `lib/std/cast_lanes.bv` — `coll_resize` showing `[data, cap, len]` handle at offsets 0, 8, 16
- `lib/std/collections.bv` — `coll obj List<T>` definition
- `docs/architecture/iterable-protocol.md` — `seq coll` contiguity semantics
