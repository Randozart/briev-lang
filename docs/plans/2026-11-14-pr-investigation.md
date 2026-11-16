# PR Investigation — 2026-11-14

## PR #94 — fix(llvm): marshal aggregate value handles to ptr at call boundaries

**Status: GREEN · Ready to merge**

### What it fixes
A struct/obj **value** whose fields include a collection/pointer (`List<T>`, `String`)
is an `i{int_bits}` handle in SSA, but its Briev ABI type is `ptr`. Passing such a
value to a defn used to forward the i64 register as a `ptr` operand — invalid LLVM
IR rejected by `opt -passes=verify` and clang:

```
'%ac0' defined with type 'i64' but expected 'ptr'
call i64 @idx(ptr %ac0, i64 …)
```

Repro:
```briev
struct Named { n: Int; label: String; };
defn double_n(x: Named) -> Int { term x.n * 2; };
defn wrap(x: Named) -> Int { term double_n(x) + x.label .^Length; };
let res: Int = 0; let t: Int = 0;
node main [t == 0][t == 1] {
    let x: Named = Named { n = 3, label = "hi" };
    res = wrap(x);
    t = 1;
    term;
};
```

### Root cause
Two producers of aggregate handles never recorded their real SSA type:
1. The `ptr` parameter prologue emits `%ac{i} = ptrtoint ptr %arg{i} to i64` without
   recording `%ac{i}` as i64.
2. `emit_struct_literal` emits `%tN = ptrtoint ptr %alloca to i{int_bits}` while the
   `TypedRegister.ty` is the struct.

Both then marshalled as `ptr` at call boundaries (call-argument marshalling reads
`fun.reg_llvm_ty`, which fell back to `llvm_type(reg.ty)` = `ptr`).

### Fix
Record the emitted SSA width so the call boundary inttoptrs:
- `fun.reg_llvm_ty.insert(conv, "i64")` in both defn and callable-txn parameter loops
  (`emit_toplevel.rs`, ~line 5186 and ~line 8011)
- Same in `emit_struct_literal` (`emit_expr.rs`, ~line 3573)

Includes regression test:
`backend::llvm::crash_regressions::aggregate_value_arguments_marshal_handle_to_ptr`

### CI: ✅ ALL GREEN
- Default gates: SUCCESS
- Fuzz smoke (blocking): SUCCESS
- Fuzz explore: SKIPPED (expected — matrix target not triggered)

---

## PR #95 — fix(interpreter): compare Float relationals in the f64 domain

**Status: GREEN · Ready to merge**

### What it fixes
In the reference interpreter (`brievc test --tests-only`), every Float comparison was
broken:
- `<`, `>`, `<=`, `>=` **never** evaluated to true (fell through to `false` for all
  Float operands)
- `==`/`!=` were **degenerate**: `31.0/9.0 == 3.0` AND `31.0/9.0 == 3.44444442` both
  returned `true`

Native (compiled) execution was correct. This caused the interpreter-based test
runner (`expect` blocks) to misreport float tests.

### Root cause
`eval_binary_op`'s `Lt/Gt/Le/Ge` arms read only `as_i64()`. A `Float` atom is
`Atom::Float(f)`, for which `as_i64()` returns `None` — so `_ => Bool(false)` fired
for every float comparison. `Eq/Neq` read `lv.as_i64() == rv.as_i64()`; with both
sides `None`, `None == None` is `true`, making all float (in)equalities compare equal.

### Fix
Added `Value::is_float` / `Value::as_number_f64` (promotes ints, bools, chars to f64)
and an `ordered_compare` helper in `src/interpreter/eval.rs`. All ordering predicates
(`Lt/Gt/Le/Ge`) now select the f64 domain when either operand is a Float (with mixed
Int/Float promotion), else the i64 domain. `Eq/Neq` gain the same float branch,
preserving existing String-content paths.

### Tests added
- `test_float_relationals_compare_in_f64` — pure float comparisons
- `test_float_relationals_promote_mixed_int` — mixed Float/Int operands
- `test_float_equality_is_value_based` — `NaN != NaN`, edge cases

### CI: ✅ ALL GREEN
- Default gates: SUCCESS
- Fuzz smoke (blocking): SUCCESS
- Fuzz explore: SKIPPED

---

## Interaction between #94 and #95

Both PRs touch different parts of the codebase:
- **#94**: LLVM backend (`emit_expr.rs`, `emit_toplevel.rs`, `crash_regressions.rs`)
- **#95**: Interpreter (`src/interpreter/eval.rs`)

They do **not** overlap. Neither can regress the other. Both are independent fixes.

---

## Recommendation

**Both PRs are ready to merge now.** All gates are green, both fixes are well-tested
and documented in BUGS.md with full root cause, fix description, and regression tests.

Merging order is irrelevant (no conflicts, no interdependency).
