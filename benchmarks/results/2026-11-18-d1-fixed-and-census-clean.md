# D1 fixed: the invalid-IR census reaches 0 of 336 (2026-11-18)

Closes the loop-carried-`Float` bug that opened this campaign's codegen work, and with
it the last entry in `tests/ir_verify_allow.txt`.

## The defect (root cause, one arm)

`loop_engine/counter.rs`, countable-body `Statement::Let`:

```rust
if let Statement::Let { ty: Some(dt), .. } = stmt {
    if is_event_type(dt) { let_binding_types.insert(name, dt); }     // declared type
    else                 { let_binding_types.insert(name, reg.ty); }  // ← the INITIALISER's
}
```

Every declared local except `Event<T>` was typed from its initialiser. `let acc_: Float = 0;`
registered `Int`, so `emit_foreach_statement` gave the carrier an `i64` slot and the body
round-tripped the accumulator through an integer each iteration — `sitofp` → `fadd` →
`fptosi`, silently dropping the fraction — before handing an i64 to `fadd fast float` at
the backedge (the invalid IR that stopped the program from linking).

Three outside-in attempts (compositor literal typing, countable pre-declaration, `Let`-arm
pre-registration) all failed because the map is **written** there, not merely cleared.

## The fix

The rule the `Event<T>` branch already applied locally, generalised: **the declared type
wins**, and the value is coerced through `helpers::coerce_register_value` — the helper whose
doc comment cites this exact case (`int-literal-float-init`, the ordinary `let` path, i.e.
why a plain node always worked). Handle-like types pass through it unchanged by design, so
collections/objs/`Event<T>` keep their behaviour.

## Evidence

| check | result |
|---|---|
| `defn-composite.bv` | valid IR, `alloca float`, **0** `fptosi`, links, runs |
| new gate test | passes with the fix, **fails without it** (verified both ways) |
| emitted IR, 6 float benchmarks (float_math, float_math_nonzero, kalman, iir, accumulator_flush, matrix_pipeline) | **byte-identical** old vs new — the fix is surgical |
| `gate.sh quick` | pass |
| `ll_determinism_check.sh --quick` | all deterministic |
| full census | **336 valid, 0 invalid**, 93 frontend errors |

The zero-diff result is the important one: no float benchmark's codegen moved, so there is
no timing A/B to run and no regression to explain.

## Two things the gate caught along the way

* The regression gate must build the **real reproducer**. A bespoke tier1 fixture written
  during the investigation passed both with and without the fix — decoration. The shipped
  test builds `examples/syntax/meta/defn-composite.bv` and asserts no `fptosi` plus
  `alloca float`, and it was verified to fail on the un-fixed compiler.
* The new undefined-call check rejected two healthy programs (`@llvm.smax.i64`,
  `@llvm.exp.f32`): **LLVM intrinsics** are never declared in the module. The allowance
  ratchet surfaced it immediately; the check now exempts the `llvm.` namespace, with a test.
