# Issue #7 & #8 — Deep Research (2026-11-17)

Companion to `2026-11-17-trunk-next-steps.md` §4 (native-stability queue).
This document records the **code traces, reproductions, and root causes**
for issues #7 and #8, with concrete fix plans.

## Issue #8 — `term` in transaction guard flow emits contradictory outputs

### Reproduction (confirmed 2026-11-17)

```
# probe8.bv
let done: Bool = false;
let x: Int = 0;
node main [!done][done] {
  x = 1;
  when x == 1 { term Print#(1); };
  term Print#(2);
};
```

`brievc build probe8.bv` → **fails at `opt -passes=verify`**:
```
opt: probe8.ll:4000:7: error: value doesn't match function result type 'i32'
  ret i64 %t23
```

### What the interpreter says (the reference)

`src/interpreter/eval.rs:756` — a `TermReturn(v)` unwinds the **whole txn
body** (skips everything after the guard, including `Print#(2)`). The
interpreter's guard handler treats `TermReturn` as "stop this firing":
result = v, post-check, loop. Only one observable effect (`Print#(1)`).

### Root cause

The emitter dispatches `term` via **four different arms** in
`emit_term_value_tail` (`src/backend/llvm/emit_stmt.rs:1750`), keyed on the
**function context**, not the **semantic scope** (reactor main vs defn):

| Condition | Emission | Correct for |
|---|---|---|
| `callable_txn_result.is_some()` | `store` to %result + `br` to post | callable txn |
| `member_result.is_some()` | no terminator (inlined member) | member body |
| `fn_ret_ty != "void"` | **`ret {fn_ret_ty} {val}`** | defn (value return) |
| else (void) | `br` to `void_txn_abort_label` or `ret void` | reactor main |

The reactor main (`define i32 @main`, `emit_main_header`
`helpers.rs:401`) sets `fn_ret_ty = "i64"` (Int width) in its SSA-loop
context. The **value-form `term`** arm matches `fn_ret_ty != "void"`
before checking whether we are in a defn, so it emits
`ret i64 %val` into `define i32 @main` — **two type mismatches**:
(i) i64 vs i32 (the reported error), (ii) the semantic error that the
reactor main is not a callable function with a return value — a `term`
in a node body means "end this firing," not "return a value from main."

The **void-form `term;`** (bare term) is handled by
`emit_bare_term_gate` → `emit_bare_term_stop`, which correctly branches to
the next-txn / abort label. That's why `term;` in a guard **works**
(probes 8c/8d build successfully), while `term <val>` **fails** (probe 8).

### Why it's a single-arm fix, not a guard-flow fix

The guard handler (`Statement::Guarded`, `emit_stmt.rs:671`) is **correct**:
it checks `!backend.fun.terminated` before emitting the convergence branch,
and the `terminated` flag is set by the term arm. The bug is that the
**term arm emits the wrong terminator** for the reactor-main context.
The guard handler never sees two `ret`s to one block — it sees one `ret`
(the wrong one) and correctly skips the convergence branch.

The "two `ret` to one block" hypothesis from the original issue is
**incorrect**. The actual bug is a **type mismatch** in the term
terminator, not a double-termination.

### Control probes (confirmed working)

| Probe | Shape | Result |
|---|---|---|
| 8b | `when { Print# }` (no term) | ✅ builds |
| 8c | `when { term; }` (bare) | ✅ builds |
| 8d | `when { term; }` + `endprogram` after | ✅ builds |
| 8e | `defn` with `when { term x*2; }; term 0;` | ✅ builds (defn context, `fn_ret_ty` correct) |
| 8f | `when { endprogram Print# }` | ✅ builds (endprogram has own arm) |

### Fix plan

**Location:** `emit_term_value_tail` (`src/backend/llvm/emit_stmt.rs:1750`).

**Add a reactor-main guard before the `fn_ret_ty != "void"` arm.** The
reactor main is not a defn — it has no return value. A value-form `term`
inside a reactor-main body should:
1. Evaluate the value expression (for its side effects, e.g. `Print#`).
2. **Branch to the next-txn / abort label** (same as the void arm) —
   NOT emit a `ret`.

The discriminator: the reactor main is the function emitted by
`emit_main` (`loop_engine/mod.rs:251`) with the `#0` attribute and the
SSA-loop structure. It is NOT a `txn_` function, NOT a defn, NOT a
member body. The existing context flags:
- `callable_txn_result.is_none()` (not a callable txn) ✓
- `member_result.is_none()` (not a member body) ✓
- `self_binding.is_none()` (not a member) ✓
- `fn_ret_ty == "i64"` (Int width, NOT "void") ← **the wrong signal**

The correct discriminator: **is this the reactor main?** The reactor main
is the ONLY function that:
- is emitted by `emit_main` (not `emit_txn` / `emit_defn` / `emit_member`)
- has the SSA-loop structure (`.ss_main_loop`, `.exit_check`, `.loop`)
- is `define i32 @main`

A clean signal: the reactor main's `fn_ret_ty` is "i64" (Int) but its
**actual LLVM return type is i32**. The mismatch between `fn_ret_ty`
("i64") and the actual `define i32` signature is itself the bug. Two
possible fixes:

**Fix A (minimal):** In `emit_term_value_tail`, before the
`fn_ret_ty != "void"` arm, check whether the enclosing function is the
reactor main (e.g. a `self.is_reactor_main` flag set in `emit_main`).
If so, route to the void arm (branch to abort label) after emitting the
value's side effects.

**Fix B (root cause):** Set `fn_ret_ty = "void"` in `emit_main` (the
reactor main has no return value — the i32 is the process exit code,
handled by `endprogram`/`exit_prog`, not by `term`). Then the existing
void arm fires correctly. This requires verifying that no other codegen
path depends on `fn_ret_ty == "i64"` in the reactor main context.

**Fix A is safer** (additive, one new guard clause). **Fix B is more
correct** but has wider blast radius. Recommend Fix A first, then evaluate
Fix B as a follow-up.

### Test fixture

```
# tests/fixtures/term_guard_value_reactor.bv
let done: Bool = false;
let x: Int = 0;
node main [!done][done] {
  x = 1;
  when x == 1 { term Print#(1); };
  term Print#(2);
};
```

Expected: builds, prints "1" only (interpreter parity — `Print#(2)` is
skipped by TermReturn). Add to the tier1 suite + an IR validation test
(asserts the `.ll` passes `opt -verify`).

### Implementation (2026-11-17) — and the regression that refined it

**Fix A, implemented** in `emit_term_value_tail`: a new arm checked
BEFORE `fn_ret_ty != "void"`:

```rust
} else if backend.fun.void_txn_abort_label.is_some()
    && backend.fun.self_binding.is_none()
    && backend.fun.self_prefix.is_none()
{
    // reactor main: value-term is a body-unwind, not a function return.
    let abort = backend.fun.void_txn_abort_label.as_ref().unwrap().clone();
    writeln!(out, "{}br label %{}", indent, abort).ok();
    backend.fun.terminated = true;
} else if backend.fun.member_result.is_some() {
```

The discriminator is `void_txn_abort_label.is_some()` (set by the SSA loop,
`loop_engine/ssa.rs:567`) — it is `Some` only inside the reactor main's
txn-body loops — combined with `self_binding.is_none() &&
self_prefix.is_none()` (not an inlined member).

**The regression that refined it (2026-11-18):** the first cut of the arm
was just `void_txn_abort_label.is_some()` — which **also matched inlined
member bodies**. When a member's `term <val>` runs *inlined* inside the
reactor main (e.g. `hs[0].greet(1)` on a `List<dyn Greeter>`), both
`void_txn_abort_label` (the enclosing node's) AND `member_result` are
set. The arm branched to `.ssn_<name>`, dropping the member's computed
value — so `emit_member_body`'s value phi (`member.exit.t*`) never
materialised, and the dyn dispatch switch missed its cases, hitting the
default arm (`dynswd_N`: `call void @llvm.trap()`). Two lib tests caught it:
`test_dyn_list_local_binding_dispatch_compiled` and
`collection_field_index_writes_reach_storage`. The `self_binding`/
`self_prefix` exclusions route those terms to the member arm (which emits
no terminator — `member_result` is taken by `emit_member_body`), restoring
byte-identical IR (verified by diffing `@main` clean vs fixed).

**Verified:**
- `probe8.bv` (the reproducer): `opt -passes=verify` passes; `@main` shows
  `guard.then → br label %.ssn_main` (no `ret i64`).
- `tests/fixtures/term_guard_value_reactor.bv` builds; the new IR-validity
  tests (`tests/term_guard_value_reactor.rs`) pass.
- The dyn list + collection IR is byte-identical to pre-fix.

**Risk:** now **low** — the discriminator is exact (reactor-main value-term
has no `self` binding; inlined members do), and the regression is pinned by
the two existing lib tests.

---

## Issue #7 — unresolved state/local symbols in outlined/nested control flow

### What "outlined" means

The "outlined" path is the **cold-function outlining** optimization in
`emit_toplevel.rs:6644`: a guard body containing an FFI/observable call
over pure scalar reads is extracted into a separate `define void
@txn_<name>_cold_N(...)` function to create a memory barrier (the FFI call
is not inlined into the hot guard, so the optimizer cannot reorder it).

The cold function:
1. Receives `ptr %state` + scalar params (`__cp_<name>`) for every
   identifier the guard body reads (state fields, let bindings, constants).
2. The body is **rewritten** via `rewrite_stmt_idents`
   (`emit_toplevel.rs:5946`): every identifier in `field_names` is
   replaced by its `__cp_<name>` param.
3. The cold function is `define void` — it **cannot** write state (a
   rewritten `done = true` would store to the param copy and vanish).

### The symbol-resolution chain in `emit_identifier_load`

`emit_expr.rs:1265` — the identifier resolution is a **long fallthrough
chain** with NO final validation:

1. `trg_addresses` (MMIO pins)
2. `#Self` (member receiver)
3. `self_prefix` (pooled columns)
4. `let_bindings` (local/param → alloca load)
5. `last_val_temps` (intra-iteration write cache)
6. `active_vector_groups` (vector phi extract)
7. `self_binding` (boxed obj slots)
8. `get_local` (native param slots)
9. `pending_phi_backedge` / `phi_field_regs`
10. `plain_lets` (module globals)
11. `field_index_map` (%State fields)
12. `constants` (global constants)
13. `inits` (runtime-seeded invariants)
14. **else: `ident_is_module_global` check** (2026-11-17 class D fix)
    - if not a module global → **warn + emit `add i64 0, 0`** (placeholder)
    - if a module global → `load i64, ptr @name`

**The class D fix (2026-11-17) already exists** in the final else branch:
if a name matches none of the 13 resolution paths, the emitter warns and
materialises zero instead of emitting an undefined `@name` global. This was
added for the `export let` shape (skipped by both storage walks).

### The residual gap

The class D fix catches **truly undefined** symbols (not in any map). But
there is a **narrower gap** specific to the outlined path:

**If a guard body reads an identifier that the outline scanner's
`collect_idents` missed, but that identifier IS in one of the 13
resolution maps** (e.g. a state field in `field_index_map`), the cold
function will emit `load i64, ptr %state` + GEP — but the cold function
**has `%state` as its first param**, so the load is valid. The value is
correct.

**The actual gap is the reverse:** if `collect_idents` captures an
identifier but the `rewrite_stmt_idents` does NOT replace it (e.g. the
identifier is in a nested `Expr::Field` or `Expr::Index` that
`rewrite_expr_idents` doesn't recurse into), the cold function will
reference the **original name** (e.g. `done`) which is NOT a param in the
cold function — it will fall through to the `field_index_map` path and
emit `load i64, ptr %state` — **which is valid** (the cold function has
`%state`).

So the outlined path is **largely safe** because:
1. `rewrite_stmt_idents` covers all expression shapes (`Call`, `BinaryOp`,
   `UnaryOp`, `Cast`, `Field`, `Index`, `Block`, `List`, `Tuple`).
2. The `field_index_map` fallthrough in the cold function is valid
   (`%state` is available).
3. The class D fix catches truly undefined names.

**The residual risk** is a **narrow edge case**: an identifier that is in
`collect_idents` but NOT in `rewrite_expr_idents`'s coverage, AND NOT in
`field_index_map` (e.g. a let binding that the outline scanner captured
but whose `__cp_` param was not added because `txn_let_names` didn't
include it). In that case, the cold function would reference the original
let name, which is NOT a param and NOT a state field → falls to the class
D else → **warns + emits zero** (silent data loss, not invalid IR).

### Current status

The **class D fix (2026-11-17) already mitigates** the most dangerous form
of issue #7 (undefined `@name` globals → invalid IR). The residual gap
(missed rewrite → silent zero) is a **correctness bug** (wrong value), not
an IR-validity bug. It is much harder to reproduce and detect.

### Fix plan

**Phase 1 (validation, low risk):** Add a post-outline validation pass that
checks every identifier in the cold function body is either:
- a `__cp_*` param, OR
- in `field_index_map` (state field, loadable via `%state`), OR
- a global constant / init / plain let.

If any identifier matches none, **emit a compile error** (not a warning) —
the outline should have captured it. This turns the silent-zero bug into
a loud diagnostic.

**Phase 2 (rewrite completeness, medium risk):** Audit
`rewrite_expr_idents` for expression shapes not covered. The current
coverage looks complete (all `Expr` variants are handled), but add a
**test fixture** with a guard body that exercises every expression shape
in an outlined context to prove the rewrite is total.

**Phase 3 (outline completeness, high risk):** Audit `collect_idents` for
statement shapes not covered. Add a test fixture with a guard body that
reads identifiers from every statement shape (`Let`, `Expression`,
`Term`, `Tail`, `EndProgram`, `Assign`, `Guarded`, `Block`).

### Test fixtures

```
# tests/fixtures/outline_guard_all_shapes.bv
// Exercises every expression shape in an outlined guard body.
let done: Bool = false;
let a: Int = 1;
let b: Int = 2;
node main [!done][done] {
  when a + b == 3 {
    let c: Int = a * b;      // Let + BinaryOp
    Print#(c);               // Expression + Call
    done = true;             // Assign (defeats outlining — stays inline)
  };
};
```

(Note: the `done = true` assign defeats outlining — the guard stays
inline. A truly outlinable body has no assigns. The fixture for outlining
must use only reads + FFI calls.)

```
# tests/fixtures/outline_guard_ffi_read.bv
// A truly outlinable guard: FFI read over a scalar state field.
let done: Bool = false;
let x: Int = 0;
node main [!done][done] {
  x = 5;
  when x == 5 { Print#(x); };  // outlinable: FFI + scalar read
  done = true;
};
```

### Risk

**Phase 1 (validation): Low.** Additive check, no codegen change.
**Phase 2 (rewrite audit): Medium.** Proves total coverage, no change.
**Phase 3 (outline audit): High.** Could reveal a real gap; fixing it
requires changing `collect_idents` or `rewrite_stmt_idents`.

**Overall assessment:** The **most dangerous form** of issue #7
(undefined `@name` globals) is **already fixed** by the class D change
(2026-11-17). The **residual gap** (missed rewrite → silent zero) is a
correctness bug that is hard to reproduce. The issue is **narrower than
the original description suggested** — it is not a general scope-tracking
failure, it is a narrow rewrite-completeness gap in the cold-function
outline path.

---

## Priority & sequencing

| Issue | Reproduced? | Root cause found? | Fix risk | Next step |
|---|---|---|---|---|
| #8 | ✅ (probe8.bv) | ✅ (term arm type mismatch) | Low–Med | Fix A: reactor-main guard in `emit_term_value_tail` |
| #7 | ⚠️ (class D fix exists; residual gap not repro'd) | ✅ (narrow rewrite gap) | Low (Ph1) / Med (Ph2) / High (Ph3) | Phase 1: post-outline validation pass |

**Recommended order:**
1. **#8 fix** (self-contained, one guard clause, clear repro) — implement
   now.
2. **#7 Phase 1** (validation pass, additive) — implement after #8.
3. **#7 Phases 2–3** (rewrite/outline audit) — if Phase 1 surfaces a
   concrete case.

**Both are lower-risk than the original "high risk" assessment suggested.**
The code traces revealed that the bugs are **narrow and localized**, not
broad scope-tracking failures.
