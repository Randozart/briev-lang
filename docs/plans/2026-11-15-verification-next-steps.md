# Verification Next Steps (2026-11-15)

## Status

Structural derivation (Tiers 1 + 2a + 2b) landed in commit `c732c0b02`
(merged `56dd4409c`). The body prover now handles:
- concrete arithmetic result vs literal (Tier 1a)
- algebraic structural equality via `simplifies_to` (Tier 1b)
- field projection in postconditions (Tier 2a)

Remaining Tier 3 items were deferred pending data.

## What the "how-far" plan recommended as the very first step

> **Then profile**: instrument `smt_discharge` to log which `Undecided` patterns
> it handles. Run the full corpus through `--verify-bodies`. The histogram of
> `Undecided` patterns tells you exactly where Tier 2 and Tier 3 are worth doing.

This was not done. It is still the right first step.

---

## 1. Profile `smt_discharge` escalation (ALWAYS FIRST — no prerequisites)

### What exists

`verify_one` (`briev-verify/src/verify.rs`) counts `undecided` paths:

```rust
let mut undecided = 0usize;
for (index, state) in terminated.iter().enumerate() {
    match verdict_for_path(post, state) {
        ProofVerdict::Undecided => {
            // escalates to Z3 via smt_discharge
            match smt_discharge(post, state) {
                Some(ProofVerdict::Violated) => { /* violation */ }
                Some(ProofVerdict::Holds) => { /* proved by Z3 */ }
                _ => {
                    undecided += 1;  // no solver, model-dependent, or bound issue
                }
            }
        }
        ...
    }
}
```

`undecided` tells us HOW MANY paths escalated; it says nothing about WHY each path returned `Undecided`.

### What to add

A `Debug`-gated profiling struct in `briev-verify/src/verify.rs` (active only under
`cargo test --features profile-z3` or `BRIEV_PROFILE_Z3=1`):

```rust
#[cfg(feature = "profile-z3")]
mod z3_profile {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static CATEGORIES: Mutex<HashMap<&'static str, usize>> = Mutex::new(HashMap::new());

    #[derive(Default)]
    pub struct Z3Profile {
        pub escalated: usize,       // smt_discharge was called
        pub smt_violated: usize,    // Z3 found a violation
        pub smt_holds: usize,       // Z3 proved it
        pub smt_none: usize,       // Z3 unavailable / model-dependent / bound
        pub concrete_arith: usize, // Binary vs Literal (Tier 1a would handle)
        pub field_projection: usize, // Expr::Field in post
        pub mul_sign: usize,       // multiplication postcondition
        pub uninterpreted_fn: usize, // PostExpr::Call
        pub other: usize,
    }

    pub fn record(path: &Expr, state: &SymbolicState, verdict: Option<ProofVerdict>) {
        let category = categorize_post(path, state);  // classify by post shape
        let mut map = CATEGORIES.lock().unwrap();
        *map.entry(category).or_insert(0) += 1;
    }

    pub fn report() -> String { /* dump the histogram */ }
}
```

The `categorize_post` function inspects the postcondition expression to classify
why `verdict_for_path` returned `Undecided`:

| Category | Pattern | Likely cause |
|----------|---------|--------------|
| `Binary vs non-literal` | `lv` is `Binary` without bounds | Tier 1a gap (a + b > 0 where neither is literal) |
| `Field projection` | post contains `x.n` | Tier 2a handled (now resolved) |
| `Mul sign` | post has `a * b > 0` or similar | sign lattice candidate |
| `Uninterpreted fn` | post contains `f(x)` | needs lemma or Z3 |
| `Pure nonlinear` | `Sqrt`, `Pow` in post | Z3 only |
| `Other` | everything else | needs per-case analysis |

`smt_discharge` already returns `Option<ProofVerdict>`. The categories above
distinguish whether the escalation was for a legitimately Z3 problem or for
something the structural prover should have handled.

### Gate

Run the corpus through `verify_bodies` with the profile feature:
```bash
cargo test -p briev-verify --features profile-z3 2>&1 | grep -A50 "Z3 escalation profile"
```
Output: a histogram like `field_projection: 142, mul_sign: 38, uninterpreted_fn: 7, other: 23`.

### Decision trigger

If `mul_sign` or `Binary vs non-literal` together represent >30% of escalations:
→ Sign lattice (Tier 3) is worth ~200 lines.

If the histogram is dominated by `uninterpreted_fn` and `Pure nonlinear`:
→ Tier 3 is low-value; Z3 is doing the right thing. Invest elsewhere.

---

## 2. P3 wasm32 cross-backend gate (ACTIVE, no prerequisites)

Per the verification program (`docs/plans/2026-11-09-compiler-verification-program.md`):

> Cross-backend corpus gate (wasm32 vs native) over the eligible subset.

Status: "Remaining: wasm32 cross-backend gate." The opt-level parity and
interpreter-over-corpus arms are done.

### What's needed

`tests/opt_level_parity.rs` (or a new `tests/wasm32_parity.rs`) that:
1. Iterates over the eligible tier1 fixture set (same eligibility as the
   existing opt-level parity suite — no FFI, no external I/O, deterministic).
2. For each fixture: compile to native (`-O0`/`-O3` as the native baseline)
   AND to wasm32 (`--target wasm32-unknown-unknown` via LLVM triple, `l lc`
   → `.wasm`, `wasm-ld`).
3. Run both and compare stdout + exit code.
4. Explicit skip list with documented reasons.

The compiler already has the wasm emission path (see `compile_wasm` in
`src/compile.rs`). The gate adds the cross-backend harness and the comparison
step.

### Test file location

`tests/opt_level_parity.rs` is the natural place (add a `wasm32` variant in the
same harness), or a dedicated `tests/wasm32_parity.rs` if the options diverge
significantly. The P2 directive runner infrastructure (`// RUN:` directives)
should be usable here with a `// RUN: --target wasm32` variant.

### Gate

Per-commit quick subset + nightly full matrix. Integration with the existing
`.github/workflows/verification-nightly.yml` if not already wired.

---

## 3. P4 growth slices: collections → enums/tuples → strings

Per the verification program:

> Growth slices: collections → enums/tuples → strings.

The fuzz program differential (`tests/fuzz_program_differential.rs`) generates
typed deterministic programs. The current slice exercises: Int/Bool lets,
`when`, bounded `foreach`, arithmetic, print. Growth slices add:

**Slice 1 — collections**: `List<T>`, `Vector<N>`, `Map<K,V>` in fuzzed programs.
Exercises: index bounds, iteration, structural aliasing.

**Slice 2 — enums/tuples**: tagged unions, tuple destructuring in fuzzed programs.
Exercises: pattern-match exhaustiveness, `match` arm coverage.

**Slice 3 — strings**: `String` / `&Str` in fuzzed programs.
Exercises: UTF-8 encoding, indexing (byte vs char), substring.

Each slice is independently testable. A new failing find from a slice becomes
a P2 diagnostic fixture or a P7 golden.

### What's already scaffolded

`fuzz/fuzz_targets/frontend.rs` etc. from PR #80. The growth slices need:
- New `proptest` strategies for collection/enum/string generation
- Extension of the comparator (run interpreter + native, compare)
- Seeds from the existing tier1 corpus

### Acceptance criteria

Each slice: clean per-iteration (no panics), finds reduced into P2 or P7, folded
into P3's corpus gate when stable.

---

## 4. Tier 3 — Sign lattice (only after profiling data)

Deferred per the "how-far" plan:

> Adding a sign lattice is architecturally clean but adds complexity to every
> `SymbolicValue` operation. It's worth doing if the budget analysis shows
> multiplication postconditions are a significant Z3 cost.

**Effort**: ~200 lines (sign lattice `{-∞, Negative, Zero, Positive, +∞}` on
`SymbolicValue`, all arithmetic ops, `rel_verdict` extension).

**Prerequisite**: Section 1 profiling confirms `mul_sign` > 30% of escalations.

**What it enables**:
```
pre:    a > 0 && b > 0
post:   a * b > 0     →  Holds (Positive * Positive > 0)
post:   a * b >= 0    →  Holds (always non-negative)
post:   a * b < 0     →  Violated (always non-negative)
post:   a * b == 0    →  Undecided (could be zero or positive)
```

The sign lattice is the minimal extension to `SymbolicValue` — it does NOT need
a full real-arithmetic theory, only the five-element lattice above.

---

## 5. Tier 3 — Lemma registry (after proven-stdlib contracts plan)

Deferred per the "how-far" plan. The proof ledger (`proof_engine/ledger.rs`)
already tracks available lemmas. A structural lemma registry would integrate
with the body prover so that a known-injective function lemma can be used to
prove `[f(x) == f(y)][x == y]` structurally rather than routing to Z3.

**Prerequisite**: Proven-stdlib contracts plan (declares which stdlib functions
have which proof-relevant properties).

**Effort**: ~300 lines.

---

## Decision summary

| Item | Priority | Effort | Prerequisite | Status |
|------|----------|--------|--------------|--------|
| smt_discharge profiling | **1 — do now** | ~50 lines | None | Not started |
| P3 wasm32 cross-backend | **2 — parallel** | ~200 lines | None | ACTIVE (incomplete) |
| P4 growth slices (collections) | **3 — parallel** | ~100 lines | None | PARTIAL |
| Tier 3 sign lattice | **4 — after profiling** | ~200 lines | Profiling data | Deferred |
| Tier 3 lemma registry | **5 — after stdlib plan** | ~300 lines | Proven-stdlib plan | Deferred |

The profiling step (Section 1) is the only item that MUST come first. Sections 2
and 3 can run in parallel. Sections 4 and 5 wait on data or a prerequisite plan.

---

## Files to change (implementation phase)

Once the planning phase clears this document:

| File | Change |
|------|--------|
| `briev-verify/src/verify.rs` | `z3_profile` module + `categorize_post` + gated counters in `verify_one` |
| `tests/opt_level_parity.rs` | wasm32 variant in the harness |
| `tests/fuzz_program_differential.rs` | collection/enum/string proptest strategies |
| `docs/plans/2026-11-09-compiler-verification-program.md` | update P3/P4 phase statuses |
