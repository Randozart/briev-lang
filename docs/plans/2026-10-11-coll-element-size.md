# 2026-10-11 — coll element size: derive from the type, not 8

## Problem

`List<dyn Trait>` (and any `List<T>` with `sizeof(T) != 8`) heap-corrupts. The
`coll` scaffold hardcodes element size = 8 bytes in three places:

1. `src/backend/llvm/coll_scaffold.rs:135` — `synth_init_empty`: `Malloc#(16 * 8)`
2. `src/backend/llvm/coll_scaffold.rs:295` — `synth_init`: `Malloc#(16 * 8)`
3. `lib/std/cast_lanes.bv:1056-1059` — `__briev_coll_resize`: `Alloc#(8 * new_cap)`, `Copy#(nd, old_data, 8 * copy_n)`

A `dyn` element is a 24-byte fat pointer (`[3 x i64]`: data, tag, vtable).
The data buffer is under-allocated: `malloc(16 * 8 = 128)` for 16 slots of 8
bytes each, but each `dyn` needs 24. Element writes stride at 8 bytes into a
24-byte value → heap overflow. Observed: `-O0` segfault, `-O3`
`free(): invalid pointer`, `-O3 -flto` exit 232.

This is latent and type-dependent: `List<Struct>`, `List<Bool>`,
`List<Bits<4>>`, `List<dyn>` all misallocate. The switch/phi/br/trap IR
(Phase C dyn dispatch) was correct — the crash is upstream in the collection
layout.

## Root cause in two halves

**Half 1 (scaffold + sizing):** the scaffold's `Malloc#` size and the
`type_size(Type::Dyn)` fallback both assume 8 bytes. `Type::Dyn` has no
universe key (`ast/types.rs:76-82` returns `None`), so `type_size` falls to
the `_ => 8` default (`types.rs:110`). A `dyn` fat pointer is 24 bytes.

**Half 2 (resize):** the runtime `__briev_coll_resize` hardcodes 8-byte
element size in `Alloc#`/`Copy#`. Grow-on-full resizes at 8 bytes even if the
scaffold is fixed. The `[data, cap, len]` block carries no type tag, so the
resize cannot derive the element size from the handle — it must be passed.

## Decisions

- **`Type::Dyn` is invariant** (a keyword, per the user). The fat pointer is
  always `[3 x i64]` (3 i64 slots) — a compiler ABI invariant, like
  `Ptr(_) => 8` (`types.rs:59`). Hardcode 24 with a provenance comment. This
  does not conflict with the adaptability goals: the size is not type-specific
  (it's the same for every `dyn Trait`), and it is derived from the ABI, not
  from a type name match (Rule 15/19 compliant).
- **Fix the scaffold to derive `elem_size` from `type_size(elem_ty, universe)`**
  instead of hardcoding 8. This is the general fix — works for `dyn`, structs,
  `Int[N]`, any element type.
- **Fix the resize to take an element size.** The runtime defn + the 3
  intrinsics (`Resize#`/`EnsureCap#`/`TrimCap#`) + the declare all gain an
  element-size parameter.
- **Do not touch `synthesize_members_for_check`** — it only synthesizes the op
  surface (no `Malloc#`), so it needs no fix.
- **Option A ordering:** Phase 1+2+4 first (fixes the literal-construction
  crash, the reported bug), then Phase 3 (resize) as a follow-up. The resize
  bug only manifests on grow-past-capacity and is separable.

## Phases

### Phase 1 — `Type::Dyn` sizing (prerequisite)

1. `src/backend/llvm/types.rs` `type_size` — add a `Type::Dyn(_) => 24` arm
   before the `_ => 8` fallback. Provenance comment: the fat pointer is
   `[3 x i64]` (data, tag, vtable), a compiler ABI invariant.
2. `src/backend/llvm/types.rs` `lower_type` — add a `Type::Dyn(_) => "ptr"`
   arm (the dyn handle is carried as an i64, inttoptr'd at use — `ptr` is the
   safe lowering, matching `Ptr(_)`).
3. `src/backend/llvm/mod.rs` `protocol_llvm_type` — `Type::Dyn` returns `ptr`
   (it's not a `Custom`/`Applied`, so it currently falls to the `i64` default
   at line 907 — wrong for a fat pointer handle).
4. `src/memory_spec.rs:226` — `Type::Dyn(_) => 24` (the estimate is
   `estimate_type_size(inner).max(8)`, which underestimates the 24-byte fat
   pointer). Low priority (memory-spec bounds, not codegen), but correct for
   consistency.

### Phase 2 — scaffold element size (the main fix)

5. `src/backend/llvm/coll_scaffold.rs` — `synthesize_members` takes
   `universe: Option<&crate::type_universe::TypeUniverse>`. `synth_init_empty`
   and `synth_init` compute `elem_size = type_size(&elem_ty, universe).max(8)`
   instead of the constant `8`.
6. Thread `self.ctx.type_universe.as_ref()` at the 3 `synthesize_members`
   call sites (`mod.rs:3120`, `mod.rs:3232`, `emit_toplevel.rs:2218`) +
   `coll_body_with_members` (`coll_scaffold.rs:533`, which calls
   `synthesize_members`).

### Phase 3 — resize element size (the second bug, follow-up)

7. `lib/std/cast_lanes.bv:1041` — `__briev_coll_resize(handle, new_cap,
   elem_size)`; `Alloc#(elem_size * new_cap)`,
   `Copy#(nd, old_data, elem_size * copy_n)`.
8. `src/backend/llvm/intrinsics.rs` `emit_resize`/`emit_ensure_cap`/
   `emit_trim_cap` — derive the element size from the coll's element type via
   the universe, pass it as the 3rd arg.
9. `src/backend/llvm/mod.rs:3649` — update the `@__briev_coll_resize` declare
   to 3 args.

### Phase 4 — tests (clang-compiling, Rule 9/20)

The existing dyn tests (`tests.rs:10357`) are string-only — they never
clang-compiled the switch IR. Add tests that actually build + run the binary:

10. `List<dyn>` differing-field, node-indexed dispatch → expect 701/91.
11. `List<dyn>` + `endprogram` in node → expect printed value, exit 0.
12. `List<Struct>` (`sizeof > 8`, non-dyn) literal + index → confirms the
    general fix.
13. `List<Bool>` (sub-word) literal + index → confirms `.max(8)` doesn't
    over-allocate.
14. (Phase 3) Grow-on-full: `List<Struct>` push past 16 → confirms the resize
    fix.
15. Mirror the clang-compiling pattern at `tests.rs:7388` (write `.ll`,
    `clang -O3 -flto`, link, run, assert stdout + exit 0).

### Phase 5 — gates + docs

16. `cargo test --lib` green (was 3050/0), `bash benchmarks/rbv_gate.sh`
    PASS, Praetor no-new diagnostics on `src/backend/llvm` + `lib/std`.
17. Update this plan's status + `BUGS.md` (latent under-allocation for
    `List<dyn>`/large elements + resize 8-byte assumption).

## Verification

Each phase: `cargo test --lib` + the new clang-compiling tests. Phase 4's tests
are the real gate — they compile the IR and check the runtime output, catching
what the string-only tests miss.

## Risk

- `protocol_llvm_type` change for `Type::Dyn`: any site that lowerings a `dyn`
  type to an LLVM type must now get `ptr`, not `i64`. Grep the call sites of
  `protocol_llvm_type` with a `Dyn` operand to confirm none expect `i64`.
- `synthesize_members` signature change: 3 call sites + `coll_body_with_members`.
  All have the universe in scope.
- Resize signature change (Phase 3): the stdlib `.bv` defn + 3 intrinsics +
  the declare. The declare must match the defn's arity or clang errors.

## STATUS: SUPERSEDED (2026-10-12)

The element-size theory (Phases 1-3) is DISPROVEN by experiment:

- ASan on the failing `List<dyn>` probe pins a READ from near-null
  (`0x7` = the element's field VALUE used as a pointer) inside `main` —
  not a heap-overflow signature. The 8-byte-handle data buffer was never
  under-allocated: every buffer access GEPs `i64` (8-byte stride), so
  `Malloc#(cap * 8)` was the CORRECT allocation size all along.
- Controls isolate the bug to `List<dyn>` exactly: bare `dyn` (no list)
  and `List<Big>` (struct element, same scaffold shape) both pass; only
  the combination fails.
- Root cause: the collection construction stores each element as the
  BOXED CONCRETE struct, while the element read path GEPs it as a
  `[3 x i64]` fat pointer (data/tag/vtable). Representation mismatch,
  not sizing.

Phases 1-2 changes (type_size(Dyn)=24, memory_spec Dyn=24,
elem_size-from-universe threading) are REVERTED. Kept from this plan's
session: the emit_dyn_switch IR-form fixes (multi-line switch, %-labels,
phi verbatim, @llvm.trap) and the ensure_mono replace-merge (mono
re-synthesis must win over the base generic's unresolved-`T` members).

The fix proceeds under SPEC §8.6.1 Option B: the element payload keeps
its native shape; a tag rides alongside only when the read site's
dispatch form (frontend-derived) needs Switch/Vtable. See the
dyn-derived-shape plan (2026-10-12) for the live design.
