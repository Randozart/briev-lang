# `dyn Trait` — Derive, Don't Fix (Dispatch-Shape Cost Model)

**Date:** 2026-10-09
**Status:** Active
**Doctrine:** SPEC §8.6.1 (dispatch is a derived shape, not a fixed mechanism)
**Supersedes:** the fixed fat-pointer `{ptr, ptr}` representation (Phase 5c T1)

---

## The Problem With the Fixed Shape

The first `dyn Trait` implementation (Phase 5c, T1) made `llvm_type(Dyn) ==
"{ptr, ptr}"` — a fixed 16-byte fat pointer `{data_ptr, table_ptr}` — and
resolved the concrete type from a name-keyed side table (`dyn_concrete_of`).
This breaks the language's core doctrine in three ways:

1. **It hardcodes a representation** in `llvm_type` instead of *deriving* it from
   `(protocol, metadata)` via the casting graph, like every other type. It is a
   special case, not a derived value (Rule 15/19 territory).
2. **It makes the vtable the default.** A vtable is the *most expensive* dispatch
   form (a table load + indirect call per method call). The language's
   MAXIMUM-EFFICIENT-DEFAULT rule (Golden Rule 2) requires the compiler to pick
   the cheapest provable form, not the most general one.
3. **The identity is side-tabled by name**, not carried in the value. A `dyn`
   value passed to a function, returned, or stored in a collection loses its
   concrete tag — the map is keyed by variable name, not carried with the value.
   This makes `dyn` useless the moment a value flows, which is the whole point of
   the feature.

## The Doctrine (SPEC §8.6.1)

A `dyn <Trait>` is a **capability** — "this value carries a concrete that
implements `<Trait>`" — not a **representation**. The compiler owns the
representation; the user owns the observable input→output contract; strategy
keywords are the only lever between them.

The compiler therefore:

- **Chooses the dispatch form per use-site** by a cost model, ascending in
  expense: **Inline → Switch → Vtable → Omit**.
- **Derives the value's shape** as *tag + payload*, where the **tag** is present
  only when the form is Switch or Vtable (concrete not provable inline), and the
  **payload** is present only when the value is live (dispatched or otherwise
  observably consumed).
- **Never hardcodes a `dyn` representation.** Its LLVM type resolves through the
  casting graph from `(protocol, metadata)`, exactly like every other type.
- **Adapts at FFI/GLUE boundaries** via the existing boundary-marshal machinery —
  a native vtable for C, an inlined identity for an LTO bridge, a diagnostic for a
  GPU kernel with no heap.
- **Honors strategy keywords** as opt-outs: `seq`/`vol`/`atomic` on a `dyn` demand
  the predictable (indirect/vtable) form. Keywords express correctness/intent,
  never speed.

The dispatch form is a **frontend decision** — computed once (the value's
reachable concrete set at each use-site) and read by the backend — the same
frontend-driven-dispatch pillar that chooses loop shapes and GPU tiles.

---

## Dispatch Forms

### 1. Inline (default)

When the concrete type is provable at the use-site — the `dyn` value's concrete
is known from the coercion that created it, and the value has not since been
aliased to a different concrete — the compiler **inlines the impl directly** and
erases the `dyn` at that site. No tag, no payload move, no table.

```briev
let g: dyn Greeter = Dog { base: 7 };
g.greet(3);
// lowers to the body of Dog::greet, `g`'s payload as the Self arg. No vtable.
```

This is the observability principle applied to dispatch: the dispatch *is* the
inlined call; the data (the receiver) is already where the body needs it, so it
does not move.

### 2. Switch (small closed set)

When a small **closed** set of concretes can reach a use-site (a function may
return `Dog` or `Cat`; the caller dispatches), the compiler emits a **tag
switch** (jump table or if-chain) over the known set. The tag rides in the value
(a derived shape). No table indirection. Cheaper than a vtable load.

```briev
defn make(k: Int) -> dyn Greeter {
    when k == 0 { term Dog { base: 1 }; };
    term Cat { lives: 9 };
};
let g = make(0);
g.greet(3);   // switch on tag: { Dog => Dog::greet, Cat => Cat::greet }
```

### 3. Vtable (fallback)

When the concrete set is **open**, or the value crosses a **FFI/GLUE boundary**
whose other side requires a native vtable, the compiler emits the **per-trait
vtable**: the value carries a tag (index into the trait's conformance set) and a
payload pointer; dispatch loads `table[tag][slot]` and indirect-calls. This is
the last resort — the existing `emit_dyn_thunk_tables` machinery, demoted from
default to fallback.

### 4. Omit

When a `dyn` value is never method-dispatched (stored but never called, or its
method is an observable identity), the compiler **omits the payload** and may
eliminate the value entirely (observability as liveness). A `dyn` that needs no
data movement carries none.

---

## The Value Shape: Derived Tag + Payload

A `dyn` value is *tag + payload*:

- **tag** — the concrete's index in the trait's conformance set. Present **only**
  for Switch/Vtable (concrete not provable inline). Absent for Inline.
- **payload** — the concrete value in whatever shape that concrete already uses
  (heap object, pool row, boxed primitive). Present **only** when live.

The LLVM type of a `dyn` is therefore:

| Form | LLVM shape |
|------|-----------|
| Inline | the payload's own shape (no tag, no wrapper) — the `dyn` is erased |
| Switch | `{ i64 tag, <payload-shape> }` |
| Vtable | `{ i64 tag, i64 payload_ptr }` (the fat pointer, demoted) |
| Omit | nothing (value eliminated) |

No fixed 16-byte image. The tag/payload decomposition is a property of the
dispatch form the cost model chose, not a constant.

---

## Architecture: Frontend-Driven Dispatch (Consistent With the Pillar)

1. **Frontend analysis** (`AnalysisResults` or a new `DispatchShape` analysis):
   - For each `dyn` value, compute its **reachable concrete set** per use-site
     (from the coercion that created it + aliasing). A single concrete ⇒
     Inline. A small closed set ⇒ Switch. Open/unknown ⇒ Vtable. Never used ⇒
     Omit.
   - Record the **concrete index** (tag) for each concrete in the trait's
     conformance set (derived from the trait def, declaration order — matches the
     interpreter).
   - The decision is stored per use-site and read by the backend.

2. **Backend** (`emit_method_call` dyn arm) **consumes** the decision:
   - Inline: emit the impl body inline with the receiver as Self (the A5
     self-bound member emission, already present for concrete receivers).
   - Switch: emit a tag switch over the closed set, each arm the impl body.
   - Vtable: the existing thunk-table load + indirect call (the fallback I built).
   - Omit: emit nothing.

3. **Casting graph** (`type_to_protocol` / `resolve_llvm_type`): a `dyn <Trait>`
   resolves to its **payload's** shape (plus a tag slot for Switch/Vtable). No
   hardcoded `Type::Dyn → "{ptr,ptr}"` early-return. The fixed-shape early-return
   in `llvm_type` (emit_toplevel.rs:774) is **removed** and replaced by the
   derived shape.

4. **Boundary/GLUE**: a `dyn` crossing an FFI edge adapts per the edge's ABI —
   the existing boundary-marshal emits the transform. C side: native vtable. LTO:
   inlined identity. GPU kernel: diagnostic (no heap).

5. **Strategy keywords**: `seq`/`vol`/`atomic` on a `dyn` force the Vtable form
   (predictable indirect dispatch) even when Inline/Switch is provable.

---

## Phases

### Phase A — Unblock + vtable fallback (small, this session)

Make the existing vtable path *correct and derived*, so the feature works end-to-end
for the fallback case and the regression test passes. This is the "vtable as
fallback" the doctrine keeps.

- **Fix `register_dyn_thunks`** to derive `(trait, concrete)` pairs from the
  **coercion sites** (`let x: dyn T = Concrete{..}`), not from `TypeDef.traits`.
  The source of truth is the coercion site (where the concrete is known); the
  trait def supplies the slot order (`dyn_trait_slots`).
- **Keep** the `emit_dyn_thunk_tables` / `emit_dyn_thunk_fn` / thunk-load dispatch
  as the **Vtable form** (form 3). It is correct as-is; it just stops being the
  default.
- **Fix the regression test** `test_dyn_trait_emits_thunk_table_and_indirect_call`
  (currently failing: `thunk fn missing`). With the pair registered from the
  coercion site, the thunk table + fn are emitted and the test passes.
- Gates: `cargo test --lib`, `bash benchmarks/rbv_gate.sh`, Praetor on changed
  dirs.

**Exit criterion:** the Greeter repro compiles, links, prints **703** through the
LLVM backend, and the regression test passes. The vtable path is green and is
documented as the fallback form.

### Phase B — Inline default + frontend cost model (the headline)

Make Inline the default, so the common single-concrete case erases the `dyn` and
inlines the impl. No tag, no payload move, no table.

- **New frontend analysis** (`src/analysis/dyn_dispatch.rs` or folded into
  `AnalysisResults`): for each `dyn` use-site, compute the reachable concrete set
  from the coercion + aliasing. Emit a per-site decision: `Inline(concrete)`,
  `Switch(Vec<concrete>)`, `Vtable`, `Omit`.
- **Backend `emit_method_call` dyn arm** consumes the decision:
  - `Inline` → emit the impl body inline with the receiver as Self (reuse the A5
    self-bound member emission against the impl body in `dyn_impl_bodies`).
  - `Switch` → emit a tag switch, each arm the impl body.
  - `Vtable` → the existing thunk-table path.
  - `Omit` → nothing.
- **Strategy keywords** on a `dyn` override the decision to `Vtable`.
- **Value shape**: for Inline, the `dyn` value is the payload's own shape (erased).
  For Switch/Vtable, `{ tag, payload }`. Update `llvm_type` to derive this (remove
  the fixed `"{ptr,ptr}"` early-return; route `dyn` through the casting graph).
- **Tests:**
  - Inline: `let g: dyn Greeter = Dog{..}; g.greet(3)` → IR contains the inlined
    `Dog::greet` body, **no** `@__dyn_Greeter_Dog` thunk table, **no** indirect call.
  - Switch: a two-concrete maker → IR contains a tag switch, no vtable.
  - Vtable: an open-set or FFI-boundary case → IR contains the thunk table +
    indirect call (the existing path).
  - Omit: a stored-but-never-called `dyn` → no payload emitted.
- Gates: `cargo test --lib`, `bash benchmarks/rbv_gate.sh`, Praetor.

**Exit criterion:** the single-concrete `dyn` call is inlined (no vtable in the
IR), the two-concrete case is a switch, and the vtable is only emitted when the
set is open or a boundary requires it. The MAXIMUM-EFFICIENT-DEFAULT rule holds:
no keyword is required to reach the fastest provable form.

### Phase C — Heterogeneous collections `[dyn Trait]` (the payoff)

A collection whose element type is `dyn <Trait>`, each row a `(tag, payload)`
pair, dispatching per row.

- **Syntax**: confirm/enable `let handlers: [dyn Greeter] = [...]` (element type
  `dyn <Trait>` in a collection). Check the parser + typechecker accept it; add if
  not.
- **Element shape**: each row is a `(tag, payload)` pair in the collection's
  element layout. The collection pass stores/loads rows as the derived element
  shape.
- **Per-row dispatch**: iterating `[dyn Greeter]` and calling `.greet(3)` on each
  row uses the same per-site cost model (Switch/Vtable, since the row's concrete
  is not provable inline from the row's position).
- **Tests:** a `[dyn Greeter]` of mixed concretes, iterate + dispatch per row,
  verify per-row results.
- Gates: `cargo test --lib`, `bash benchmarks/rbv_gate.sh`, Praetor.

**Exit criterion:** a heterogeneous `dyn` collection iterates and dispatches each
row to its concrete's impl, with the per-row dispatch form chosen by the cost
model. This is the "makes it useful for programming" capability.

---

## What Changes Per Surface (Doctrine Audit)

| Surface | Phase A (vtable fallback) | Phase B (derived) | Phase C (collections) |
|---|---|---|---|
| `llvm_type(Dyn)` | keep `"{ptr,ptr}"` (vtable shape) | **remove** fixed early-return; derive from payload + tag slot | derive element shape |
| Casting graph `resolve_llvm_type` | (not consulted — fixed) | **consulted**: `dyn` → payload shape + tag slot | same |
| `register_dyn_thunks` | **fix**: derive from coercion sites | derive from coercion sites | same |
| Coercion (3 Let paths + init) | `coerce_to_dyn` (vtable wrap) | Inline: no wrap; Switch/Vtable: tag+payload wrap | row = tag+payload |
| `emit_method_call` dyn arm | thunk-table load (fallback) | **consume** frontend decision (inline/switch/vtable/omit) | per-row dispatch |
| Arg passing / return / field / phi | i64/`{ptr,ptr}` handle (generic) | Inline: payload shape (free); Switch/Vtable: tag+payload (free) | row movement (free) |
| Strategy keywords | (not read) | **read**: force Vtable | read |
| FFI/GLUE boundary | vtable (C) | vtable (C) / inlined (LTO) / diag (GPU) | vtable (C) |
| Typechecker (conformance at coercion) | (already checks) | checks + records concrete set | element `dyn` check |
| Interpreter | `Value::Dyn { concrete, inner }` (reference) | unchanged (reference) | unchanged |
| `memory_spec.estimate_type_size(Dyn)` | 8 (i64) | derive (tag+payload) | derive |

---

## Retirement Gate (Per the "naive fallback is a floor, never a ceiling" doctrine)

The fixed fat pointer (`llvm_type(Dyn) == "{ptr,ptr}"` + `dyn_concrete_of` side
table) is the **floor**. It is kept only as the Vtable fallback (form 3). It
**retires** when Phase B's inline/switch machinery drives the single- and
two-concrete cases to zero vtable emission — at which point the fixed-shape
early-return and the name-keyed `dyn_concrete_of` map are **removed**, leaving the
vtable to fire only for open sets and boundaries. Until Phase B lands, the fixed
shape is documented in-code as a `// TEMP: 2026-10-09:` floor with this plan as the
path to permanence.

---

## Non-Goals

- No new syntax beyond `dyn <Trait>` (already present) and the `[dyn Trait]`
  collection element type (Phase C). No new strategy keyword — `seq`/`vol`/
  `atomic` already express the "predictable indirect dispatch" opt-out.
- No change to the interpreter (it is the reference; `Value::Dyn { concrete,
  inner }` already models the capability).
- No new language vocabulary or algorithm-shape recognition (Rule 23/24).

---

## Verification

- `cargo test --lib` green (≥ current 3045, plus the new Phase A/B/C tests).
- `bash benchmarks/rbv_gate.sh` PASS.
- Praetor on changed dirs: no NEW diagnostics.
- The Greeter repro prints **703** through the LLVM backend (Phase A).
- Phase B: single-concrete `dyn` call IR has **no** thunk table / **no** indirect
  call (inlined); two-concrete IR has a tag switch.
- Phase C: mixed-concrete `[dyn Greeter]` dispatches per row correctly.
