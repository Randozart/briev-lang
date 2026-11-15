# Verification Next Steps (2026-11-15)

## Dated amendment — 2026-11-15, after `a9917e166`

The [assurance handoff and next-steps plan](2026-11-15-163529-assurance-handoff-and-next-steps.md)
updates the compiler-assurance ordering and records the merged baseline. The
original proposals below remain historical/planned; illustrative profiling code
and feature flags are not implemented commands. Known false-proof boundaries
should be repaired without waiting for performance profiling.

**Correction to section 4:** positive signed 64-bit operands do not imply a
positive or nonnegative wrapping product: `9223372036854775807 * 2 == -2`.
The proposed sign-lattice conclusions below require modular semantics or proved
no-overflow premises. They must not be implemented as unconditional word rules.
The linked handoff preserves the independent wasm/fuzz proposals and separates
model proofs from production and source/native refinement obligations.

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

The fuzz program differential (`fuzz/src/semantic.rs` + `fuzz/fuzz_targets/*.rs`,
the libFuzzer surface; `tests/fuzz_frontend.rs` / `tests/fuzz_backend.rs` are
the proptest surfaces) generates typed deterministic programs. The current
slice exercises: Int/Bool lets,
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
| `fuzz/src/semantic.rs` + `fuzz/fuzz_targets/` | collection/enum/string differential slices (libFuzzer); proptest counterparts in `tests/fuzz_frontend.rs` / `tests/fuzz_backend.rs` |
| `docs/plans/2026-11-09-compiler-verification-program.md` | update P3/P4 phase statuses |

---

## Session results (2026-11-15, end of session)

### Completed
- **Profiling instrumentation** (Sections 1a+1b): `Z3EscalationProfile` + `categorize_post` + `record_smt_result` in `briev-verify/src/verify.rs`, gated behind `z3-profile` feature. Committed as `0b07cbed1`, merged to `main` as `63959d0a8`.
- **P4 collections fuzz slice**: `collections_program` + `check_collections` in `fuzz/src/semantic.rs`, `collections.rs` fuzz target. 3 unit tests, all passing. Same commit.
- **Baseline profiling run**: 650 total, 183 PROVEN, 467 UNPROVEN, 0 VIOLATED (report: `/tmp/verify-bodies-baseline.txt`).

### Tier 3 decision: NOT justified
The escalation histogram (from the baseline run) shows:

| Goal shape | Count | Category | Structural? |
|---|---|---|---|
| `(term >= 1) && (term < (1 << 24))` mantissa_val | 1330 | interval/bound | **No** — stdlib precond too weak (see below) |
| `(term == true) \|\| (term == false)` tautology | 87 | logic tautology | **Yes** — trivial `x ∨ ¬x` |
| `(term >= 0) && (term <= 4294967295/MASK32)` uint range | 60 | interval | Yes, but small |
| `CharCount#` string goals | ~363 | uninterpreted-fn axioms | No (lemma registry) |
| `(i == count)` loop invariant | 9 | induction | No (SMT only) |
| timestamp `x * 86400/3600/60` | ~15 | nonlinear mul | No (SMT only) |

**The dominant class (1330×, ~85% of UNPROVEN) is NOT a missing derivation rule.** `mantissa_val` in `lib/std/float_fmt.bv` has postcondition `term >= 1 && term < 1 << 24` with precondition `exp8 >= 0 && exp8 <= 255 && mant >= 0`. On the `exp8 == 0` path, `term = mant`, so `term >= 1` requires `mant >= 1` (false for `mant = 0`) and `term < 1 << 24` requires `mant < 1 << 24` (unestablished). The contract is **genuinely unsatisfiable** under the current precondition. The verifier correctly returns `Undecided` (tri-state working as designed). The fix is to **tighten the stdlib precondition** (e.g. `exp8 == 0 && mant >= 1 && mant < 1 << 24` for the `exp8 == 0` branch, or a combined `mant > 0 || exp8 > 0` style), not to add a new derivation rule.

**The tautology class (87×) IS structural** — `x ∨ ¬x` forms dischargeable by a simplifier extension without SMT. This is a smaller, simpler win than Tier 3 sign-lattice work.

**Decision**: Tier 3 sign-lattice is **deferred**. The profiling data does not justify it: the dominant escalation is a stdlib contract defect, the only true interval shape (uint-range, 60×) is small, and the tautology class is a different, simpler problem. If Tier 3 is revisited, the correct target is the 60× uint-range class, not the 1330× mantissa_val class.

### P3 wasm32 parity gate: BLOCKED
The webstack `.wasm` exports `txn_run(state_ptr)` but requires the full DOM host shim (`render_frame`, flush-binding table, `__web_boot` state initialization) to reach `endprogram`. A minimal Node harness cannot honestly drive the wasm to completion — `txn_run` hits the guard-fail `unreachable` path on the first call. The broken test was removed; the blocker is documented. A Node-based DOM shim implementation is required before this gate can function.

### Next steps (updated)
1. **Tighten `mantissa_val` precondition** in `lib/std/float_fmt.bv` — would clear the 1330× dominant escalation class. This is a stdlib change, separate from the verifier.
2. **Add tautology discharge** (`x ∨ ¬x → true`) to the structural simplifier in `briev-verify` — would clear 87× escalations with zero SMT. Small, contained change.
3. **Tier 3 sign-lattice**: deferred. Revisit only if the uint-range class (60×) becomes the dominant escalation after steps 1 and 2.
4. **P3 wasm32**: anchor the gate on a **native sink** (state-slot value decoded
   via `state_layout`, or the `briev_syscall(231)` exit code for terminating
   fixtures). No DOM shim is required — see the correction section below, which
   also records two wrong diagnoses made while scoping this gate.
5. **P4 enums/strings fuzz slices**: extend the collections slice pattern to enum and string differential oracles.

### Correction: P3 blocker was over-claimed (2026-11-15, post-session)

**Lesson / oversight record.** In the original session I documented the P3 wasm32
parity gate as "BLOCKED: webstack `.wasm` needs the full DOM host shim to reach
`endprogram`." That was an **over-claim** and I record it as such:

1. **`endprogram` is not a requirement for a Briev program to complete.**
   Per `docs/architecture/briev-execution-model.md`, a reactive program has no
   `main()`; the reactor IS the pseudo-loop. A program "finishes" by reaching
   **equilibrium** (no node precondition can fire → idle). `endprogram` is an
   *optional lifecycle law* (`hold G { endprogram; }`) that makes the reactor
   terminate the process when `G` holds. I conflated "the fixture must call
   `endprogram`" with "the program must terminate," and then conflated
   "I need an observable outcome" with "the observable must be an exit code."

2. **The parity gate does NOT need the DOM host shim.** The observable for a
   parity check is the **final state** (after the reactor reaches equilibrium)
   or the exit code for terminating fixtures. Both are reachable from the
   existing `txn_run(state_ptr)` loop in a minimal Node harness: drive
   `txn_run` until it stops firing (equilibrium), then read the exported
   `%State` slots from linear memory and compare against the native run. The
   DOM shim (`render_frame`, `stdout_buf`, flush-binding) is only required for
   the **stdout / browser-observability** path — not for state or exit-code
   parity.

3. **The actual remaining gap is a harness-contract detail**, not a missing
   runtime: the minimal Node harness's `txn_run` loop was hitting the
   guard-fail `unreachable` branch on iteration 0. The guard (`tick < 1`)
   is satisfiable at `tick = 0` (set by `init_state` via `__web_boot`), so the
   failure is in the harness's state/memory plumbing (stale `DataView` of the
   wasm `memory`, or passing the state pointer in the wrong form), not in the
   wasm or the compiler. This needs a focused harness fix, not a DOM shim.

**Corrected P3 scope.** Re-scope the wasm32 parity gate to **equilibrium-state
comparison**: drive `txn_run` to equilibrium in Node, read the final `%State`
from linear memory, and compare against the native execution's final state.
For fixtures with `endprogram`, the exit code is a cheap additional observable.
The gate should classify fixtures by shape (terminating vs equilibrium) and
compare the appropriate observable per shape. The DOM shim is out of scope for
state/exit-code parity and remains a separate, larger effort for stdout parity.

**Process note.** The over-claim happened because I stopped investigating at the
first plausible-sounding blocker ("needs the DOM shim") without verifying the
execution-model semantics first. The correct move was to read
`briev-execution-model.md` (equilibrium vs `endprogram`) *before* scoping the
gate's observable. Future gate work should ground the observable choice in the
execution model, not in the convenience of the existing harness.

### P3 harness root cause (verified in `.ll`, 2026-11-15)

Further investigation of the `txn_run` guard-fail (on the terminating fixture
`enum_local_return`, which native-runs to `endprogram` → exit 0):

- `txn_run(state)`'s first guard is `slot0 < 1` (the node's `tick < 1`).
- `init_state` (called by `__web_boot`) sets `slot0 = 0`, so the guard is
  **satisfiable** — the reactor *should* fire the node on the first call.
- The minimal Node harness hits the `unreachable` guard-fail branch on
  iteration 0. This is a **harness state-pointer/plumbing mismatch**: the
  `txn_run(sp)` call is not reaching the same state the DOM driver does
  (the DOM driver reads `_statePtr` from `__briev_state_ptr()` and calls
  `txn_run` through the `WasmDomRuntime` with the full `state_layout`/
  `bindingTable` contract; a bare harness that re-instantiates or passes the
  pointer in a different form does not).
- `render_frame` is a **no-op stub** (`ret void`) in the webstack emission —
  the actual tick driver in the DOM path is the host calling `txn_run` in a
  loop (the `requestAnimationFrame` loop wraps `render_frame`, but the
  real per-frame work is `txn_run`).

**Conclusion:** the P3 gate's remaining work is a **focused harness fix** —
replicate the DOM driver's `txn_run`-in-a-loop + `state_layout`/`bindingTable`
contract in the Node test — then drive to equilibrium and diff the final
`%State` (or compare `endprogram` exit code for terminating fixtures). No DOM
shim, no compiler change. This is a small, contained test-infrastructure fix,
not the "large separate effort" originally documented.

### P3 observable anchored on the amended Observability-as-Liveness doctrine (2026-11-15)

The P3 gate's observable is now anchored on the **amended Observability-as-Liveness
doctrine** (see `AGENTS.md` §"Observability as Liveness" and
`docs/architecture/defn-liveness.md` §"Native observability roots"). The doctrine
recognizes two classes of observable sink:

- **FFI sinks** — `endprogram code` (syscall 231), `__print_int` / `Print#`
  (stdout), `GetEnvInt#` (runtime-determined bound).
- **Native sinks** — `vol` stores (volatile memory the optimizer can't
  eliminate), webstack state-slot writes (flush records read via the binding
  table), reactor node-firing (the reactor IS the observable scheduler), and
  the `reside` lifecycle law (absence of termination is the observable).

**The P3 parity gate anchors on a native sink, not on FFI.** For a wasm32
parity check, the observable is a **state-slot value** read from the wasm's
linear memory via the `state_layout`/`bindingTable` contract (a native
sink), or a `vol`-anchored value. `endprogram`'s exit code is an FFI sink and
is a cheap *additional* observable for terminating fixtures, but it is not the
required anchor. The gate does **not** need the DOM shim: the DOM shim
(`render_frame`, `stdout_buf`, flush-binding for browser rendering) is only
required for the stdout/browser-observability path. State-slot parity is
reachable by replicating the DOM driver's `txn_run`-in-a-loop + `state_layout`
contract in a Node test.

This corrects the earlier over-claim that the gate "needs the DOM host shim."
The shim was only needed for stdout. The parity observable is a native sink —
exactly the class the amended doctrine now names.

### Correction: "body has no terminating value path" is expected, not unproven

Roughly 90 baseline warnings read:

```
could not prove: <name>: body has no terminating value path — nothing to verify
```

(`init_exit`, `script_main`, `tick`, `increment`, `withdraw`, `vol_user`, …).
These are reactive `node`/`txn` bodies with no `term`/`endprogram`: there is no
postcondition to discharge, and the message says so. They are **correct
behaviour**, not proof gaps.

Consequence for measurement: future profiling runs must exclude them from the
UNPROVEN denominator (a separate `NO_POSTCONDITION` bucket), otherwise the
"467 UNPROVEN" figure understates the pass rate and dilutes the histogram. The
follow-on plan (`2026-11-15-remaining-verification-work.md` §1.4) carries this.
