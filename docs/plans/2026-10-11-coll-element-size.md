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

## ADDENDUM — Phase 1 complete: interpreter contract pin (2026-10-12)

### The interpreter gap (and its fix)

The interpreter (reference, Rule 5) had the SAME class of bug the backend
exhibits: a `List<dyn Trait>` literal's elements were NOT wrapped in
`Value::Dyn`. The `Statement::Let` coercion site (Phase 5b, 2026-08-22)
handled only the scalar case — `let g: dyn Greeter = Dog{..}` — by reading
the *annotation* (`Type::Dyn`) and wrapping the whole value. A list literal
under a `List<dyn Greeter>` annotation evaluates via `Expr::List` →
`Value::product` with no coercion, so `handlers[0]` returned a plain
`Product { fields: [7], names: ["base"] }` and the dispatch
`dispatch_method_value` fell through the `Value::Dyn` arm to the
"not a known function" fallback, returning `Void` (→ 0). The probe printed
`0` instead of `701`.

Fix (src/interpreter/eval.rs): the `Let` arm now delegates to
`eval_let_binding`, which handles two coercion shapes:

- **Collection-of-dyn** — the annotation is `Applied(_ , [Dyn(T)])` or
  `Generic(_ , [Dyn(T)])` (matched STRUCTURALLY by the type argument, never
  by collection name — Rule 19 compliant). Each element of a list literal is
  evaluated and wrapped in `Value::Dyn { trait_name: T, concrete, inner }`,
  where `concrete` comes from the element's syntax (constructor name or
  another dyn binding) — the same provenance rule as the scalar case. A
  non-literal initializer under a collection-of-dyn annotation is refused
  loudly (the elements' provenance is unknowable after the fact).
- **Scalar `dyn`** — unchanged behavior (the whole value is wrapped), now
  via the shared `dyn_concrete_name` helper.

Contract pin (tests/dyn_list_interpreter_parity.rs): two probes, both
heterogeneous `List<dyn Greeter>` (`Dog{base:7}`, `Cat{lives:9}` — differing
field layouts, the exact shape the backend corrupts):

- **Probe A** — `handlers[0].greet(1)` exits the program → 701.
- **Probe B** — `handlers[1].greet(1)` (element 1, differing layout) → 91.

Both pass. The contract is: whatever the interpreter computes, the backend
must reproduce.

### The reactor harness subtleties

Two traps in driving the interpreter for a contract pin (both cost an hour):

1. **Top-level `let`s must seed via `exec_stmt`, not `eval_expr`.** The
   coercion lives in the *statement* path (`eval_statement` →
   `eval_let_binding`); a raw `interp.eval_expr(initializer)` skips it and
   produces an uncoerced value.
2. **No `clear_dirty()` before the first `run()`.** `build_from_program`
   marks every reactive txn dirty (the bootstrap wake). `run_reactor`
   (reactor.rs:428) clears that set before its first `run()`, so it can
   never fire anything (`dependency_map` is never populated — `mark_dirty`
   is a no-op). The working pattern: call `reactor.run(&mut interp)`
   directly in a loop, no pre-clear; `endprogram` surfaces as
   `RuntimeError::ProgramExit(value)`. `run_reactor_continuous`
   (reactor.rs:452) is the only production driver with this lifecycle and
   is the working reference. See BUGS.md for the `run_reactor` bug.

### The `endprogram Print#(x)` exit-code convention

`endprogram Print#(greet(1))` prints `701` and exits `0`, NOT `701`. This is
by design, both sides:

- `Print#` is a side-effect intrinsic. Defns cannot be void (Family B ABI),
  so it claims the neutral success value `0`. (The 2026-09-23
  exit-code-parity fix aligned the backend's `__print_*` to the
  interpreter's `Print#` — both return 0.)
- `endprogram code;` evaluates `code` (the print is the last action) and
  exits with the expression's VALUE. `Print#`'s value is `0`, so the exit
  code is `0`. `endprogram greet(1)` exits with `701` — a genuine value
  carries to the exit code.
- The same consumption rule as `term`: the value flows to whatever
  consumes it (the term postcondition, the process exit code), and a
  side-effect expression's neutral value satisfies both without
  distortion.

**Probe design consequence:** to observe a dispatch result as the exit
value, the probe must print AND pass the value to `endprogram` separately:

```briev
Print#(handlers[0].greet(1));      // observable stdout
endprogram handlers[0].greet(1);   // exit value = 701
```

This convention is documented in SPEC §11.5 ("The exit code is the
expression's value"). The factually-wrong "printf + return 0" comment in
`cast_lanes.bv` was rewritten (C's `printf` returns the byte count — that
was the original bug, not the convention). `__eprint_str` was aligned to
`term 0` (it was the last print-family outlier returning the count).

## ADDENDUM 2 — Phase 4 complete: tagged-box backend (2026-10-12)

### The shipped design

Construction and read are symmetric around a **global per-trait tag**:

- **Construction** (`emit_init_op_construction` → `dyn_collection_element_box`,
  emit_toplevel.rs): when the frontend's per-collection decision says Switch
  (a closed set of 2..=4 concretes), each element is stored as a 16-byte
  TAGGED BOX `[tag, payload]` — tag = the concrete's GLOBAL per-trait index
  (`dyn_concrete_tag_of`, sorted position — the same discriminant the dispatch
  switch selects on), payload = the boxed-concrete handle. The buffer slot
  holds the box's ADDRESS (8-byte stride unchanged — grow/resize/Pop#/At stay
  width-agnostic). Inline (1 concrete) and Vtable (open/>4) collections store
  the payload handle directly — no box, no tag.
- **Read** (Index arm, emit_expr.rs): the tier2 `op At` dispatch records the
  collection binding in `FunctionContext.current_dyn_collection`; the inner
  `data[i]` load — a re-entry into the Index arm with `obj = Identifier("data")`
  (the buffer field, NOT the binding) — consults it. For a tagged `dyn`
  element it reconstructs the 24-byte fat pointer `[payload, tag, table]` the
  dispatch machinery already consumes (`malloc(24)` per READ — no allocation
  is stored, so no ownership/lifetime change; the box in the buffer stays the
  single source of truth).
- **Dispatch**: unchanged — `emit_dyn_switch` maps tag i → concretes[i] over
  the global per-trait sorted set, which is exactly what the box tag indexes.

Controls verified: bare `dyn` (scalar coercion, 701), `List<Big>` (non-dyn
elements, 3) — both unchanged paths still pass. `cargo test --lib` 3058/0;
rbv_gate PASS; Praetor clean (no new diagnostics).

### The double-construction bug (found by -O0 assembly tracing)

The first cut boxed element 0, bailed MID-LOOP at element 1 (the insert-member
lookup used the MONOMORPHIZED key — `operator_defs` is keyed by the plain
base name "List", while `type_key` is `resolve_obj_key`'s monomorphized
form), and the caller then re-ran the LEGACY path on top: a second `op Init`
allocated a fresh buffer (the boxed one leaked), Dog was constructed twice,
and Cat was stored RAW. Runtime symptom: exit 132 (SIGILL — the switch's
default trap) at both -O0 and -O3 (at -O3 the whole `main` folded to `ud2`).

Two lessons encoded in the code:

1. **Feasibility pre-pass before any emission** (`dyn_element_tags`): the box
   path must be total — once it emits one element it cannot bail, or the
   caller's fallback double-constructs. The pre-pass proves every element (a
   struct literal with a registered tag) and the insert member BEFORE the
   first store.
2. **`operator_defs` is keyed by the base name, `obj_members` by the
   monomorphized key** — resolve lookups from the caller's already-resolved
   `defs`/`members` (the `DynBoxCtor` now carries the resolved insert member),
   never re-derive from `type_key`.

### Measurement protocol note

Per the performance-recovery protocol: probes were verified at a print
boundary before timing discipline applied (701/91 exact stdout, exit 0,
harness-exact clang command). The tagged box adds one 16-byte malloc per
ELEMENT at construction and one 24-byte malloc per element READ — the read
allocation is the cost of not changing the dispatch ABI (Phase 5's bare-dyn
unification can retire it by making the box itself the fat pointer: 24 bytes
`[tag, payload, reserved]`, store once, read with zero reconstruction).

## ADDENDUM 3 — Phase 4b: soundness closure (2026-10-12, same day)

The tagged-box commit left three holes, found by probing past the happy path.
All closed in the same increment:

1. **The dispatch form was still backend-derived.** `emit_dyn_method_call`
   chose Inline/Switch/Vtable from the GLOBAL per-trait set cardinality — a
   5-concrete program (global set > the Switch threshold) crashed
   `resolve_dyn_concrete` (the panic at emit_expr.rs:3382) even for a
   2-element collection, because construction only boxed Switch-form
   collections. Fix: a COLLECTION-ELEMENT receiver (recv = `Index` over a
   binding with a `dyn_collection_info` entry or a `fun.dyn_boxed_bindings`
   entry) dispatches Inline (set of 1) or Switch over the GLOBAL per-trait set
   for ANY cardinality — the box tag is always a global index, so the
   global-set switch is always sound, and LLVM lowers a wide switch to a jump
   table (competitive with the vtable's load + indirect call). The Vtable form
   never fires for elements.
2. **Node-body LOCAL `List<dyn>` crashed** (exit 132): the frontend analysis
   only walks top-level lets, so locals got no box, no reconstruction. Fix:
   `construct_local_collection` gained the dyn arm (`construct_local_dyn_boxed`,
   reusing the SAME pre-pass + box-emission helpers — one contract, two
   construction sites) and records the binding in
   `FunctionContext.dyn_boxed_bindings` — PER-FUNCTION state, so two nodes
   using the same local name cannot collide. The read gates and the
   dispatch-site check consult BOTH maps (ctx for top-level, fun for locals).
3. **`-O3 -flto` masked an invalid IR.** The local read's fat-pointer table
   slot pointed at `@__dyn_<Trait>_<Trait>` (the tag→concrete lookup fell back
   to the trait name for locals) — an UNDEFINED symbol. `clang -O0` rejects it
   outright; `-O3 -flto` materialized/ELIMINATED the dead store and linked
   fine, then trapped at runtime. The new local-list behavioral test links at
   `-O0` as the IR-validity gate (the LTO lesson, inverted: -O0 catches what
   -O3 hides).

Probes (all harness-exact clang, print-boundary-verified, exit 0):
`701` (top-level elem 0), `91` (elem 1, differing layout), `1 5` (5-concrete
global set), `44 11` (2-element collection inside the 5-concrete global set —
the case that panicked pre-fix), `7 8` (node-body local). `cargo test --lib`
3059/0 (2 compiled behavioral tests); rbv_gate PASS; Praetor: no new flagged
functions (`construct_local_collection` was baseline-flagged; same 2 kinds).

Known remaining holes (next increment, same mechanism — push-site boxing):
- `h.push(Dog{..})` on an empty-init dyn collection stores the RAW handle
  (the push argument is not boxed) → dispatch reads garbage. The push site
  needs the same box-with-provable-arg treatment at the member-call boundary.
- A dyn list passed as a defn ARGUMENT (a call-arg list literal) constructs
  via the legacy path (no binding name at the call site) → same hole.
Both predate this work (they crashed identically before the tagged box) and
end in a LOUD failure (trap/panic), never silent wrong data.
