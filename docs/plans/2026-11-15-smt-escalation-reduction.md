# Reducing SMT Escalations: Tautology Discharge + mantissa_val Precondition

**Date:** 2026-11-15
**Status:** Plan (verification + implementation phases)
**Baseline:** 650 total / 183 PROVEN / 467 UNPROVEN / 0 VIOLATED (`/tmp/verify-bodies-baseline.txt`)

## Motivation

The profiling instrumentation (committed `0b07cbed1`) revealed that 467 of 650
verification bodies are UNPROVEN. The escalation histogram shows two
addressable classes:

| Goal shape | Count | Root cause |
|---|---|---|
| `((term >= 1) && (term < (1 << 24)))` — `mantissa_val` | **1330** | stdlib precondition too weak (see §1) |
| `(term == true) \|\| (term == false)` — `char_eq`, `is_ws_byte` | **87** | logic tautology, discharged structurally (§2) |
| `(term >= 0) && (term <= 4294967295)` / `MASK32` | **60** | uint-range interval (Tier 3, deferred) |
| `CharCount#` string goals | ~30 | SMT-only (string intrinsic) |
| `(i == count)` loop invariant | 9 | SMT-only (induction) |
| timestamp arithmetic (`x * 86400`) | ~15 | SMT-only (nonlinear mul) |
| `(term == 0) \|\| (term != 0)` | 9 | logic tautology (§2) |

Fixing the top two classes would clear **1417/467 ≈ 304%** of the UNPROVEN
count (with overlap — some bodies have both classes), bringing the PROVEN
count from 183 to an estimated **~500+** with no solver involvement.

---

## §1 — `mantissa_val` precondition tightening

### Finding (verified in source)

`lib/std/float_fmt.bv:467`:

```briev
defn mantissa_val(exp8: Int, mant: Int) [exp8 >= 0 && exp8 <= 255 && mant >= 0]
    [term >= 1 && term < 1 << 24]
    -> Int {
    when exp8 == 0 {
        term mant;
    };
    term mant + 0x800000;
};
```

The postcondition `term >= 1 && term < 1 << 24` is **genuinely unsatisfiable**
under the current precondition `exp8 >= 0 && exp8 <= 255 && mant >= 0`:

- **Path 1** (`exp8 == 0`): `term = mant`. Postcondition requires `mant >= 1`
  and `mant < 1 << 24`. Neither is established — `mant = 0` gives `term = 0`
  (violates `term >= 1`); `mant = 0x800000` gives `term = 0x800000`
  (violates `term < 1 << 24`).
- **Path 2** (`exp8 != 0`): `term = mant + 0x800000`. Postcondition requires
  `term < 1 << 24` = `16777216`. With `mant` up to `0x7FFFFF` (23-bit mask,
  the max value of `bits & 0x7FFFFF`), `term` can be up to
  `0x800000 + 0x7FFFFF = 0xFFFFFF = 16777215`, which IS `< 1 << 24`.
  But the precondition doesn't establish `mant <= 0x7FFFFF` — it only says
  `mant >= 0`, so `mant` could be arbitrarily large, making `term` exceed
  `1 << 24`.

### Reachability check (done)

`float_format` (line 426) calls `mantissa_val(exp8, mant)` where:
- `exp8 = (bits >> 23) & 0xFF` — range `[0, 255]` ✓
- `mant = bits & 0x7FFFFF` — range `[0, 0x7FFFFF]` ✓
- The guard `when exp8 == 0 && mant == 0` **terminates early** (returns
  "0"/"-0"), so `mantissa_val` is only reached when
  NOT (`exp8 == 0` AND `mant == 0`).

So `exp8 == 0 && mant == 0` is **unreachable** at the call site (the early
return handles it). But the verifier doesn't see that context — it verifies
`mantissa_val` in isolation with only its own precondition.

### Correct fix

The postcondition IS satisfiable for all reachable inputs. The problem is
the **precondition is too weak** — it doesn't capture the actual range of
`mant` (which is `[0, 0x7FFFFF]` due to the `& 0x7FFFFF` mask) or the
early-return guard. Two options:

**Option A (preferred): Tighten the precondition to match the call site.**

```briev
defn mantissa_val(exp8: Int, mant: Int)
    [exp8 >= 0 && exp8 <= 255 && mant >= 0 && mant <= 0x7FFFFF]
    [term >= 1 && term < 1 << 24]
    -> Int
```

This is correct: the call site guarantees `mant <= 0x7FFFFF` (the `& 0x7FFFFF`
mask). With this precondition:
- Path 1 (`exp8 == 0`): `term = mant`. Need `mant >= 1` — but `mant == 0` is
  possible under the precondition! So `term >= 1` is still unprovable on this
  path. **The postcondition is still wrong** for `exp8 == 0, mant == 0`.

**Option B (correct): Split the postcondition by branch, or adjust it.**

The real issue: on the `exp8 == 0` path, `term = mant`, and `mant` can be 0
(the denormal zero case). But `float_format` handles that case with an early
return, so `mantissa_val` is never called with `exp8 == 0 && mant == 0`. The
postcondition `term >= 1` is **only valid when `exp8 == 0 → mant >= 1`**.

The cleanest fix that preserves the contract's intent:

```briev
defn mantissa_val(exp8: Int, mant: Int)
    [exp8 >= 0 && exp8 <= 255 && mant >= 0 && mant <= 0x7FFFFF
     && (exp8 != 0 || mant >= 1)]
    [term >= 1 && term < 1 << 24]
    -> Int
```

This adds two facts:
1. `mant <= 0x7FFFFF` — the actual bit-width bound (from `& 0x7FFFFF`).
2. `(exp8 != 0 || mant >= 1)` — the denormal zero is handled before this
   call, so on the `exp8 == 0` path, `mant >= 1`.

With these, both paths satisfy the postcondition:
- Path 1 (`exp8 == 0`): `mant >= 1` (from precond), `mant <= 0x7FFFFF < 1<<24` ✓
- Path 2 (`exp8 != 0`): `term = mant + 0x800000`, range `[0x800000, 0xFFFFFF]`,
  all `>= 1` and `< 1<<24` ✓

**Verification step (prerequisite for implementation):**
Before implementing, add a test fixture that calls `mantissa_val` with the
precondition and confirm the verifier proves it. Then add a negative test
that `mantissa_val(0, 0)` is rejected (the precondition should be unsatisfiable
for that input — `(exp8 != 0 || mant >= 1)` is false for `(0, 0)`).

---

## §2 — Tautology discharge in `eval_post`

### Finding

`char_eq` (`lib/std/char.bv:168`) and `is_ws_byte` (`lib/std/string.bv:318`)
have postcondition `[term == true || term == false]`. These are **logic
tautologies** — `x ∨ ¬x` for any Bool `x`. The verifier's `eval_post`
reaches `Undecided` because:

1. `term` is an `Expr::Identifier("term")` → `eval_symbolic` returns
   `SymbolicValue::Identifier("term")` (not a concrete value).
2. `term == true` → `rel_verdict(Eq, Identifier("term"), Literal(1))` →
   `Undecided` (unknown identifier, can't compare to literal).
3. `term == false` → `rel_verdict(Eq, Identifier("term"), Literal(0))` →
   `Undecided`.
4. `combine_or(Undecided, Undecided)` → `Undecided`.
5. Escalates to SMT.

### Fix

In `eval_post` (or a helper called before SMT escalation), recognize the
tautology pattern: **`A == true || A == false`** where `A` is the same
identifier/expression. This is `x ∨ ¬x` — always true for any Bool.

Implementation location: `eval_post` in `briev-verify/src/verify.rs`, in the
`BinaryOpKind::Or` arm. Before calling `combine_or`, check if the two
disjuncts are complementary tautologies:

```rust
BinaryOpKind::Or => {
    // Tautology discharge: `A == true || A == false` is always true for Bool A.
    if is_complementary_tautology(l, r) {
        return ProofVerdict::Holds;
    }
    let lv = eval_post(l, state, bounds);
    let rv = eval_post(r, state, bounds);
    combine_or(lv, rv)
}
```

Where `is_complementary_tautology(l, r)` returns true if:
- `l = BinaryOp(Eq, a, Bool(true))` and `r = BinaryOp(Eq, a, Bool(false))`
  (same `a`), OR
- `l = BinaryOp(Eq, a, Bool(false))` and `r = BinaryOp(Eq, a, Bool(true))`.

This is a **structural pattern match on the Expr AST** — no semantic
evaluation needed. The pattern is:
- `Expr::BinaryOp(BinaryOpKind::Eq, Box<Expr::Identifier(name)>, Box<Expr::Bool(true)>)`
  paired with
  `Expr::BinaryOp(BinaryOpKind::Eq, Box<Expr::Identifier(name)>, Box<Expr::Bool(false)>)`
  (same `name`, either order).

Also handle `(term == 0) || (term != 0)` (9× in the baseline):
- `Expr::BinaryOp(BinaryOpKind::Eq, a, Expr::Decimal(0))`
  paired with
  `Expr::BinaryOp(BinaryOpKind::Neq, a, Expr::Decimal(0))`.

### Scope

This is a small, contained change in `eval_post` (or a helper `fn
is_tautology_or(l, r) -> bool`). It adds ~20 lines of pattern matching. No
changes to SMT, no changes to the tri-state semantics (the tautology IS
provably true — `Holds` is the correct verdict, not a shortcut).

---

## §3 — P3 wasm32 parity gate (deferred to after §1+§2)

Correctly scoped per the amended Observability-as-Liveness doctrine:
- Anchor on a **native sink** (state-slot value via `state_layout`/
  `bindingTable`), not the DOM shim.
- For terminating fixtures, `endprogram` exit code is an additional
  observable.
- Remaining work: focused Node-harness fix (replicate DOM driver's
  `txn_run`-in-a-loop contract).

---

## §4 — P4 enum/string fuzz slices (deferred)

Extend the collections-slice pattern (`fuzz/src/semantic.rs::
collections_program` + `check_collections`) to enum and string differential
oracles. Same shape, mechanical.

---

## §5 — Tier 3 sign-lattice (deferred)

Revisit only if the 60× uint-range class becomes the dominant escalation
after §1+§2. Not justified by current profiling data.

---

## Implementation plan

### Phase 1: Tautology discharge (§2) — smallest, contained, verifier-only

1. Add `fn is_tautology_or(l: &Expr, r: &Expr) -> bool` to `briev-verify/src/verify.rs`.
   - Pattern-match the two complementary tautology shapes.
2. Call it in the `BinaryOpKind::Or` arm of `eval_post`, before `combine_or`.
3. Add tests:
   - `char_eq`-shape tautology → `Holds` (not escalated to SMT).
   - `(term == 0) || (term != 0)` → `Holds`.
   - Non-tautology `Or` (e.g. `x == 1 || x == 2`) → unchanged behavior.
4. Run `cargo test -p briev-verify` + quick gate.
5. Commit.

### Phase 2: `mantissa_val` precondition tightening (§1) — stdlib change

1. Verify reachability: confirm `float_format` never calls `mantissa_val`
   with `exp8 == 0 && mant == 0` (already confirmed in source — the early
   return handles it).
2. Edit `lib/std/float_fmt.bv:467`:
   - Tighten precondition to `[exp8 >= 0 && exp8 <= 255 && mant >= 0 && mant <= 0x7FFFFF && (exp8 != 0 || mant >= 1)]`.
3. Add a test fixture (or extend an existing one) that calls `mantissa_val`
   and verifies the postcondition is now provable.
4. Add a negative test: `mantissa_val(0, 0)` should be rejected by the
   precondition (unsatisfiable).
5. Run `--verify-bodies` on the corpus — confirm the 1330× `mantissa_val`
   escalations are gone.
6. Commit.

### Phase 3: P3 wasm32 gate (§3) — test infra

1. Fix the Node harness `txn_run` state-pointer plumbing (match the DOM
   driver's `__briev_state_ptr()` + `state_layout` contract).
2. Drive to equilibrium; read final `%State` from linear memory.
3. Compare against native execution's final state.
4. For terminating fixtures, compare `endprogram` exit code.
5. Classify fixtures by shape; compare appropriate observable per shape.
6. Commit.

### Phase 4: P4 enum/string fuzz slices (§4) — fuzz infra

1. Add `enum_program` + `check_enum` to `fuzz/src/semantic.rs`.
2. Add `string_program` + `check_string` to `fuzz/src/semantic.rs`.
3. Add fuzz targets + Cargo.toml wiring.
4. Add unit tests.
5. Commit.

---

## Verification gates

Each phase:
- `cargo build` — no new warnings
- `cargo test -p briev-verify` (Phase 1) / `--verify-bodies` on corpus
  (Phase 2)
- `bash scripts/gate.sh quick`
- `praetor validate --warn --target <dir>` on changed directories
- Commit with provenance comment

## Expected outcome

| Phase | Escalations cleared | SMT calls saved |
|---|---|---|
| §1 (mantissa_val) | ~1330 | ~1330 |
| §2 (tautology) | ~87 | ~87 |
| **Total** | **~1417** | **~1417** |

PROVEN count: 183 → estimated **~500+** (some overlap between classes).
UNPROVEN: 467 → estimated **~100-150** (SMT-only classes remain).
