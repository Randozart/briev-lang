# D1 root cause: the countable-body `Let` arm discards the declared type (2026-11-18)

Research only. Three prior attempts failed from the outside; this one found the writer.

## The writer

`src/backend/llvm/loop_engine/counter.rs`, in the countable-body statement emitter, its
`Statement::Let` arm:

```rust
self.fun.let_bindings.insert(name.clone(), reg.name.clone());
if let Statement::Let { ty: Some(dt), .. } = stmt {
    if crate::backend::llvm::emit_stmt::is_event_type(dt) {
        self.fun.let_binding_types.insert(name.clone(), dt.clone());   // declared type
        self.fun.let_original_types.insert(name.clone(), dt.clone());
    } else {
        self.fun
            .let_binding_types
            .insert(name.clone(), reg.ty.clone());                     // ← the VALUE's type
        self.fun
            .let_original_types
            .insert(name.clone(), reg.ty.clone());
    }
} else {
    self.fun.let_binding_types.insert(name.clone(), reg.ty.clone());
    …
}
```

So for every declared local **except** `Event<T>`, the map is written from the
initialiser's register type. For `let acc_: Float = 0;` the initialiser `0` registers as
Int, the map says `Int`, and `emit_foreach_statement` types the carrier slot from that
map with an `i64` fallback:

```rust
let llvm_ty = backend.fun.let_binding_types.get(&name)
    .map(|t| backend.llvm_type(t))
    .unwrap_or_else(|| "i64".to_string());
```

Measured: `[carrier] 'acc_' -> typed=Some(Custom("Int")); let_binding_types=[acc_:Custom("Int")]`
— which is why seeding the map from outside (three attempts: countable pre-pass with
`or_insert`, the same with overwrite, `emit_statement`'s `Let` arm) all changed nothing and
why the AST plainly reads `Float`.

**The `Event<T>` branch is the tell.** Someone already needed "the declared type wins" for
one case and special-cased it. It is a general rule that was applied locally.

## The machinery already exists

`helpers.rs::coerce_register_value` — and its doc comment names this exact bug:

> 2026-10-05 (BUGS.md int-literal-float-init): VALUE-semantic register coercion to a
> declared scalar type. `let acc: Float = 0` must store sitofp(i64 0) = +0.0f, not the low
> bits.

It is called from `emit_stmt.rs:4340` (the ordinary `let` path, which is why a plain node
works) and from five other sites — but **not** from the countable-body arm. So this is the
same class of defect the project already fixed once, in the path that the composite's
expansion does not take. `BRIEV_REF` in `BUGS.md` records that earlier fix.

## The fix

In the countable-body `Let` arm only:

1. When `ty: Some(dt)`, use **`dt`** for `let_binding_types` and `let_original_types`
   (dropping the `is_event_type` special case — it is subsumed, and
   `coerce_register_value` returns handle-like types unchanged by design).
2. Coerce the bound value: `let reg = self.coerce_register_value(out, indent, &reg, dt);`
   before `let_bindings.insert(...)`, mirroring `emit_stmt.rs:4340`.
3. Rebind `let_bindings` to the *coerced* register name.

Then the carrier slot is `float`, the value stored is `sitofp i64 0 → float`, the
`fptosi`/`sitofp` round-trip inside the loop body disappears, and the backedge reads a
float.

## Verification

1. `examples/syntax/meta/defn-composite.bv` → clang-valid IR (the verifier now says so
   unprompted), the accumulator prints 8.0, and **no** `fptosi` remains in the loop body.
2. A tier1 fixture: composite `Float` accumulator printing its value.
3. Remove the entry from `tests/ir_verify_allow.txt` in the same commit (the ratchet fails
   on a stale exemption).
4. `bash scripts/gate.sh quick` — this arm handles every statement form, so the whole
   suite is the real gate.
5. A/B `bash benchmarks/build_and_bench.sh --runtime` before/after for the float corpora;
   a regression refutes the change (Rule 20). Watch `float_math`, `kalman`, softmax.
6. `bash benchmarks/ll_determinism_check.sh --quick` — the slot change alters register
   numbering, which must stay deterministic.

## Risks and what would refute it

* **Handle-like declared types.** `coerce_register_value` returns them unchanged by design
  (`is_handle_ty`: struct tables, `Ptr`, `Vector`), so collections/objs/`Event<T>` keep
  their current behaviour — but this arm serves every declared local, so a broad corpus run
  is mandatory, not optional.
* **Where the coercion's `out` writes.** It emits instructions; in this arm they land in
  the body, before the value is bound. That is the same position `emit_stmt.rs:4340` uses.
* **Refutation test:** if `float_math` or `kalman` moves materially, the slot-typing change
  is touching something wider than the carrier path and the change must be narrowed to
  carriers only (skip the retype when the name is not in
  `foreach_carrier_obligations()`), not accepted with a shrug.

## Why this took four attempts

Each earlier attempt asked "who should write `Float`?" — the answer was "someone, from
outside". The question that worked was "who writes `Int`, and why?" and the answer is a
six-line arm in the countable emitter that reads the declared type and then ignores it.
The `Event<T>` special case sitting three lines below the `reg.ty` assignment is the
strongest possible hint that this branch's rule was already known to be wrong.
