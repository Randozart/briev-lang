# Remaining Verification Work — Plan

**2026-11-15.** Follow-on to `2026-11-15-verification-next-steps.md`. That
document recorded what shipped and the evidence behind the Tier 3 deferral.
This one turns the leftovers into ordered, verifiable work items, and corrects
four things that document got wrong.

---

## 0. What already shipped (do not redo)

| Item | State |
|---|---|
| `z3-profile` instrumentation (`Z3EscalationProfile`, `categorize_post`, `record_smt_result`) | `0b07cbed1`, merged `63959d0a8` |
| P4 collections differential slice (`fuzz/src/semantic.rs`, `fuzz_targets/collections.rs`) | same commit |
| Baseline profile run | 650 files: 183 PROVEN / 467 UNPROVEN / 0 VIOLATED |
| Tier 3 sign-lattice | deferred |
| Observability-as-Liveness amended to two sink classes (FFI + native) | `897873f6f` |

---

## 1. Corrected baseline (the previous tally was wrong in four places)

The baseline report was appended three times, so raw `grep -c` counts triple.
Counts below are obligation counts across the whole file, used for **ranking
only** (the summary counts *files*: 467 UNPROVEN).

| Goal | Function(s) | Count | Real cause |
|---|---|---|---|
| `(term >= 1) && (term < (1 << 24))` | `mantissa_val` | 1330 | **stdlib restructure** (see W3) |
| `CharCount#(term) <= CharCount#(s)` | `trim`, `trim_left/right/start/end` | 225 | uninterpreted-function axioms |
| `CharCount#(term) == CharCount#(a) + CharCount#(b)` | `concat`, `append`, `prepend` | 138 | uninterpreted-function axioms |
| `(term >= 0)` | `fold_lower_byte`, `fold_upper_byte` | 90 | **dead + unsatisfiable contract** (§1.3) |
| `(term == true) \|\| (term == false)` | `is_ws_byte`, `char_eq` | 75 | logic tautology (W2) |
| `(i == count)` | `repeat_loop`, `hash_loop` | 51 | loop induction |
| `(term >= 0) && (term <= 4294967295)` | `ip_v4` | 33 | bit-level shift/Or reasoning |
| `(term >= 0)` | `ml_arena_off_bytes`, `ascii_lower`, `bump` | 39 | unconstrained input/global |
| `(term >= 0) && (term <= MASK32)` | `sha1_f`, `rotl32` | 24 | **mask bound** (W3-D1) |
| `(term == 0) \|\| (term != 0)` | `lshr64` | 9 | logic tautology (W2) |
| `(((term >= (msg_len + 9)) && (term >= 9)) && ((term % 64) == 0))` | `sha1_padded_len` | 9 | modular arithmetic |
| "body has no terminating value path" | `init_exit`, `script_main`, `tick`, … | ~90 | **not a gap** (§1.4) |

### 1.1 Correction — the string class is the second largest, and it is not trivial

The previous plan reported `~30x CharCount#`. It is **~363 obligations** —
larger than the bool tautology class (84). It is also not a simplifier tweak:
it needs axioms over an uninterpreted function (length is a homomorphism for
`concat`/`append`/`prepend`, monotone for `trim*`). That is the lemma-registry
workstream, not sign-lattice work.

### 1.2 Correction — mask bounds buy 24, not 160

The previous plan's "uint-range" class looked like the payoff for bound
inference. Reading the bodies: only `sha1_f` and `rotl32` are genuinely
`x & MASK32` (24 obligations). `ip_v4` is `(a<<24)|(b<<16)|(c<<8)|d` — needs
shift bounds plus bit-disjointness, a different (harder) rule. `fold_*` is not
a mask case at all (§1.3).

### 1.3 Correction — three "unproven" classes are contract bugs, not verifier gaps

| Function | Contract | Why unprovable |
|---|---|---|
| `fold_lower_byte(b)` / `fold_upper_byte(b)` (`lib/std/string.bv:379,384`) | `[true][term >= 0]` | `b` is unconstrained; both branches return `b` or `b±32`, so `b = -1` refutes the postcondition. **And they have no call sites** — dead code. |
| `ml_arena_off_bytes()` (`lib/saltpetre/tensor.bv:43`) | `[true][term >= 0]` | returns the global `ml_arena_off`, which carries no declared range fact. |
| `ascii_lower(b)`, `bump(...)` | `[term >= 0]` | same shape — input or global unconstrained. |

These are **strengthen-the-precondition or delete** fixes in stdlib, and they
should not sit in the verifier backlog. Strengthening a precondition is sound
(Rule 1 forbids weakening, not strengthening); for the two dead `fold_*`
functions, deletion is the honest option.

### 1.4 Correction — "body has no terminating value path" is correct behaviour

~90 warnings are `node`/`txn` bodies with no `term`/`endprogram`. There is no
postcondition to discharge; the message is accurate. These must be **excluded
from the UNPROVEN denominator** in future profiling runs so the ratio is
meaningful. (Suggestion: have the profiler count them in a separate
`NO_POSTCONDITION` bucket.)

---

## 2. Work items

### W1 — Documentation accuracy pass (30 min, no code risk)

| Where | Wrong | Fix |
|---|---|---|
| `2026-11-15-verification-next-steps.md` §"Next steps (updated)" item 4 | "implement a Node-based DOM host shim" | contradicted by the correction later in the same file → "anchor on a native sink; no shim" |
| same file, "Files to change" table | `tests/fuzz_program_differential.rs` | does not exist; real surface is `fuzz/src/semantic.rs` + `fuzz/fuzz_targets/*.rs` and `tests/fuzz_frontend.rs` / `tests/fuzz_backend.rs` |
| same file, Tier 3 table | `~30x CharCount#` | ~363 (see §1.1) |
| same file, Tier 3 rationale | mask/uint-range as the Tier 3 candidate | split into mask bound (24) vs shift/Or (33) vs contract bugs (§1.3) |
| this plan | — | carried into the next profiling run: separate `NO_POSTCONDITION` bucket (§1.4) |

Also add the wrong-diagnosis record for the wasm harness (§W4) so it is not
re-derived a third time.

### W2 — Tautology / contradiction discharge in `eval_post` (~2 h, 84 obligations)

**Why cheap:** `eval_post` already has `And`/`Or` arms
(`briev-verify/src/verify.rs:1020-1030`), `negate()` already flips comparisons
(`verify.rs:1404`), and `Expr` derives `PartialEq` (`ast/expr.rs:37`).

**Design — two rules, admitted before recursing:**

- **R1 complementary relations.** For `l || r`: if `negate(l) == *r` (or
  `negate(r) == *l`) → `Holds`. For `l && r` the same check → `Violated`.
  Covers `(x == 0) || (x != 0)` (`lshr64`, 9) and `(x < k) || (x >= k)`.
- **R2 exhausted Bool domain.** For `l || r` where `l = (a == true)` and
  `r = (a == false)`, same `a`, either order → `Holds`. Sound because in a
  well-typed program only a `Bool` compares with Bool literals, and `Bool` has
  exactly two values. Covers `is_ws_byte` + `char_eq` (75).

**Placement:** `fn tautology_verdict(op: BinaryOpKind, l: &Expr, r: &Expr) ->
Option<ProofVerdict>` called at the top of the `And`/`Or` arms. Two
early-return helpers (`is_complement`, `is_bool_exhaustive`) so `eval_post`'s
complexity — already flagged in the Praetor baseline — does not grow.

**Tests:** `(x==0)||(x!=0)` holds; `(x==0)&&(x!=0)` violated;
`(b==true)||(b==false)` holds; `(b==true)||(b==true)` **undecided**;
`(x==0)||(x==1)` **undecided** (the over-discharge guard — this is the test
that matters).

**Measurement:** re-run the profile. Expect exactly 84 escalations removed
(is_ws_byte 75 + lshr64 9) and **zero** change elsewhere. Any other delta is a
bug, not a bonus.

### W3 — `mantissa_val`: 1330 obligations (stdlib restructure first, one small interval operation if needed)

**Key architectural finding:** `--verify-bodies` checks a body's postcondition
**at the definition site against the declared precondition**. There is **no
call-site precondition obligation** in this pass (the only diagnostics are
postcondition failures, guard/invariant obligations, and "no terminating value
path" — verified by reading `verify_one`/`eval_post`/`loop_obligations` and the
absence of any call-site check). Therefore no amount of caller-side derivation
reaches `mantissa_val`: path 1 of 3 is checked against
`[exp8 >= 0 && exp8 <= 255 && mant >= 0]` alone.

**Why the current contract cannot be proven.** With only `mant >= 0`:
- `exp8 == 0` path: `term = mant`, and `mant = 0` refutes `term >= 1`.
- else path: `term = mant + 0x800000`, and `mant >= 0` allows arbitrarily large
  `mant`, so `term < 1 << 24` is not implied.

The contract is *satisfiable in practice* (the caller masks `mant` with
`0x7FFFFF` and has already `term`ed out the `exp8 == 0xFF` and
`exp8 == 0 && mant == 0` cases) — but that is a **caller-side** fact the
verifier never sees.

**Fix, part A — restructure, never weaken and never rely on caller facts.**
Split the two physically different cases into two definitions:

```
// normal: hidden bit set
defn mantissa_normal(mant: Int)
    [mant >= 0 && mant < 8388608]              // < 2^23
    [term >= 1 && term < 16777216]             // 1 <= term < 2^24
    -> Int {
    term mant + 0x800000;
};

// subnormal: hidden bit clear, so mant != 0 is required
defn mantissa_subnormal(mant: Int)
    [mant >= 1 && mant < 8388608]
    [term >= 1 && term < 16777216]
    -> Int {
    term mant;
};
```

Caller change in `float_format` (`lib/std/float_fmt.bv:452-453`): branch on
`exp8 == 0` and call the matching defn. This also makes the precondition
obligation *visible in the source* rather than implied by masks two lines
earlier — the honest form.

**Fix, part B — one small missing interval operation.** The subnormal body
(`term mant`, an identifier) is decided by the existing
`ident_verdict`/`Interval::classify` path. The normal body is **not**: its term
is `mant + 0x800000`, i.e. `SymbolicValue::Binary("+", …)`, and
`rel_verdict` only consults bounds when one side is a bare `Identifier`. The
interval domain (`briev-verify/src/verify/interval.rs`, `Interval { lower,
upper }` with `restrict`/`intersect`/`classify`) therefore also needs:

- `Interval::shift(delta: i64) -> Interval` — `x ∈ [a,b] ⇒ x ± c ∈ [a±c, b±c]`,
  saturating (`checked_add`/`checked_sub`, falling back to `FULL` on overflow,
  never to a wrong bound). ~10 lines, next to `restrict`/`classify`.
- `fn interval_of(sv: &SymbolicValue, bounds: &HashMap<String, Bound>) ->
  Option<Bound>` in `verify.rs` — returns a bound for a bare `Identifier` and
  for `Binary("+"/"-", <bounded>, <literal>)`. Called from `rel_verdict` when
  neither side is a literal. ~30 lines, early-return shaped.

This is deliberately the *smallest* interval extension that closes the case:
affine offset only (`x + c`, `x - c`). No multiplication, no sign lattice, no
bit reasoning. It also serves `ascii_lower`, `bump`, `ml_arena_off_bytes` and
any `x ± const` postcondition.

**Kani.** `interval.rs` already carries a `#[cfg(kani)] mod proofs`; add
`shift` soundness to it (`∀ x ∈ [a,b]`, `∀ c`: `x + c ∈ shift(a,b,c)`) so the
new operation is proved, not assumed. Kani harnesses live in the standalone
`kani/` crate (`cd kani && kani …`), not the workspace.

**Verification:** `brievc build lib/std/float_fmt.bv --verify-bodies` (or the
corpus run) must go from 1330 UNPROVEN to 0 for these two defns, and the
caller's own postcondition `[term .^Length >= 1]` must stay PROVEN. Plus a
regression fixture with a known float formatting case compared against the C
`printf("%g")` reference, to prove behaviour did not change.

**Spike before committing to part B.** Write the two split defns first and run
`--verify-bodies`. If the normal case is already decided (possible if some
other path I have not traced supplies the interval), skip part B entirely.
Only build `Interval::shift` + `interval_of` if the spike proves it is the
missing piece — this keeps the verifier change evidence-driven.

**Guard:** if either split defn still fails to prove, stop and report — do not
relax the postcondition. The postcondition is the specification.

### W3-D1 — Mask bound inference (~1 day, 24 obligations + generality)

After W3 the remaining interval work is small, but the rule is general:

- **Rule:** `x & M` with `M >= 0` constant ⇒ result in `[0, M]`. Record as a
  `Bound` during symbolic evaluation rather than adding a verdict path —
  `ident_verdict` then answers `>= 0` / `<= k` for free.
- **Soundness:** only for non-negative constant masks; a negative mask is not a
  range. If either operand is unknown and the mask is negative → `Undecided`,
  never `Holds`.
- **Tests:** `x & 0xFF` ⇒ `[0,255]`; `x & 0xFFFFFFFF` ⇒ `[0,4294967295]`;
  `x & 0` ⇒ `== 0`; `x & -1` ⇒ undecided.
- **Payoff:** `sha1_f`, `rotl32` (24). Do **not** claim `ip_v4` — that needs
  shift bounds plus bit-disjointness (Or of non-overlapping fields), which is a
  separate rule and not scheduled here.

### W4 — P3 wasm32 parity gate (with the corrected diagnosis)

**Two wrong diagnoses, both recorded so they are not repeated:**

1. "Needs the DOM host shim" — wrong. The shim is only for stdout/browser
   rendering.
2. "The `txn_run` tick guard fails at iteration 0" — wrong. The harness matched
   **any** `unreachable` in the trap message. The `unreachable` that fires is
   the **intentional noreturn marker after `exit_prog`**:

```
; briev.program_end
call i64 @exit_prog(i64 %code)
unreachable                     ; expected
```

and `exit_prog` is a *defined* function (`ll` line ~1652) lowering to:

```
call i32 @stdout_flush()
%t1 = call i64 @briev_syscall(i64 231, i64 code, i64 0, i64 0, i64 0, i64 0, i64 0)
```

**Harness bug list (this is the actual work):**

1. `briev_syscall` must be declared with **seven i64 parameters** — match the
   import arity exactly.
2. Provide `__web_flush_state` (called by `__web_boot` and by every state
   write); a counting stub is enough to prove the native sink fired.
3. **A trap immediately after a recorded `briev_syscall(231, …)` is SUCCESS.**
   Check "was exit 231 recorded" before interpreting any `unreachable`.
4. Re-read `memory.buffer` after any call that may grow memory; never hold a
   `DataView` across `malloc`.

**Gate design — observables per the amended doctrine (native sink first):**

- **Primary:** decode `state_layout` the way `_loadStateLayout` does
  (`fieldCount`@0, `genOff`@4, `flushOff`@8, then 16-byte records at +16:
  handle, offset, size, typeTag) and compare the final slot values with the
  native run.
- **Secondary (terminating fixtures):** `briev_syscall(231, code)` vs the
  native process exit status.

**Fixture classification, explicit and reported:**

- *program-surface, terminating* (`node`/`txn` + `endprogram`) — both
  observables. (`enum_local_return`, `enum_state_lifetime`,
  `enum_tuple_escape`, `pg_buffer_gate`)
- *program-surface, equilibrium* — flush-state only, bounded tick budget.
- *library-only* (no `node`/`txn`) — skip with a printed reason.
- *`beginprogram`-shaped* — exports `main(argc, argv)`; separate arm or skip.

A skip never fails the gate; only a comparison mismatch does. Stdout parity is
explicitly out of scope.

### W5 — P4 differential breadth

Current coverage: arithmetic + tuples + literal-list collections
(`fuzz/src/semantic.rs`). `tests/interpreter_corpus_parity.rs` covers Int/List
only (gcd, bubble sort, fib, binary search, sieve, matrix checksum).

- **W5a enums (~half day).** Generate an `enum` whose constructor is chosen by
  fuzz bytes, `match` it, reduce to `Int`, compare to the oracle. Must reduce to
  `Int` so the existing `as_i64` result path is reused.
- **W5b strings (later).** Needs an oracle accessor — `interpret()` requires
  `value.as_i64()`, so a `String` result needs an `as_string` comparison path.
  Scope to `CharCount#`, `+`, and slicing to avoid the full unicode surface.
- **Every disagreement becomes a fixture** under `tests/tier1/`, not just a
  fuzz seed.

### W6 — Verification discipline (applies to every item)

`cargo test -p briev-verify`, `cargo build`, `bash scripts/gate.sh quick`,
`praetor validate --warn --target <changed dir>`, and for W2 / W3 / W3-D1 the
z3-profile before-after table. W4 runs `cargo test --test wasm32_parity`.
No class regression lands without an A/B explanation.

---

## 3. Non-goals

- **Lemma registry / `CharCount#` axioms** — second-largest class (~363) but
  needs a sound axiom layer plus a proven-stdlib contracts plan. Out of scope;
  the measurement is recorded so it can be scheduled on evidence.
- **Tier 3 sign lattice** — still not justified. Mask bounds (W3-D1) cover the
  sound part of the interval story; shift/Or bit reasoning is a distinct rule.
- **Stdout parity for wasm**, **call-site precondition checking** — both real,
  both larger, both noted as follow-ons.
- **Weakening any contract** to make a proof pass.

---

## 4. Order and payoff

| # | Item | Effort | Obligations cleared | Risk |
|---|---|---|---|---|
| W1 | doc accuracy | 30 min | — | none |
| W3a | `mantissa_val` split (stdlib) + spike | ~2 h | up to 1330 | low |
| W2 | tautology / contradiction | ~2 h | 84 | low |
| W3b | `Interval::shift` + `interval_of` (**only if the spike needs it**) | ~3 h | finishes W3's 1330 | medium (Kani proof) |
| stdlib | delete/strengthen `fold_*`, `ml_arena_off_bytes`, `ascii_lower`, `bump` | ~1 h | 129 | low |
| W5a | enum differential | ~half day | new coverage | none |
| W3-D1 | mask bounds | ~1 day | 24 + generality | medium |
| W4 | wasm32 gate | ~1 day | new cross-backend coverage | medium |
| W5b | string differential | ~1 day | new coverage | low |

Rationale for the order: **W3a first** — it is the single largest class (1330,
~70% of all obligations), and the restructure alone may clear it. W2 is the
cheapest verifier change with a precisely measurable delta. W3b is gated on the
spike so no interval machinery is built on speculation. W3-D1 comes after,
because W3 shrinks the interval class and clarifies what remains.

W4 and W5 are independent and can run in parallel with W2/W3.

---

## 5. Landing log

### W3a — DONE (2026-11-15)

**The plan was wrong about the blocker.** `mantissa_val` was not merely
"unprovable": its postcondition `term < 1 << 24` was **not sortable** by the
word-model encoder at all. `smt/encoding.rs::signature` has no shift operator
(`And/Or`, `Lt/Le/Gt/Ge`, `Add/Sub/Mul/Div/Mod` only), so
`postcondition_sort` returned `None` and `verify_supported` rejected the whole
obligation as "unsupported or ill-sorted word-model contract" before any
reasoning ran. Three contracts in `lib/std/float_fmt.bv` carried `<<`
(`pow2b`, `float_rounded`, `mantissa_val`); all three now use the equivalent
literal.

Two further real defects were fixed in the same file:

| Defect | Fix |
|---|---|
| `mantissa_val` precondition `mant >= 0` admits `mant == 0` on the `exp8 == 0` path (refuting `term >= 1`) and any magnitude (refuting `term < 2^24`) | precondition now states the true call-site facts: `exp8 <= 254`, `mant < 8388608`, and `(exp8 > 0 \|\| mant >= 1)` |
| `exponent_val` precondition `exp8 <= 255` yields `term == 105` at `exp8 == 255`, refuting `term <= 104` | precondition tightened to `exp8 <= 254` — exactly what the file's own comment already prescribed |

**W3b was not needed.** The spike showed the split/restate cleared the
obligations without `Interval::shift`, because `rel_verdict` already reaches a
decided verdict when the precondition pins the range directly. No verifier
change was made — the honest outcome is that the 1330 obligations were a
**stdlib contract defect**, and the "missing interval operation" hypothesis is
retired until a case actually needs it.

**Measurement** (`scripts/verify_bodies_corpus.sh lib/std`, new reproducible
runner): `mantissa_val`, `exponent_val` and `pow2b` are absent from the
unproven tally — `mantissa_val` went from 1330 obligations to 0.

**Behaviour gate:** `bash benchmarks/parity/run.sh float_fmt_cases` → PASS
(byte-identical to the golden) and `cast_cases` → PASS. The contract edits are
behaviour-preserving, as required.

**Remaining `lib/std` unproven** (13): `lshr64` ×3 (tautology → W2), `ip_v4` ×3
(shift/Or bit reasoning), `sha1_padded_len` ×2 (modular), `fold_upper_byte` ×2 +
`fold_lower_byte` ×2 + `ascii_lower` ×1 (unconstrained input → stdlib item).

**Note on the runner:** it verifies every file with `brievc build`, so library
fragments that cannot link standalone report `ERR` (e.g. `std/os/temp.bv`'s
undefined `@to_c_string`, `std/posix/io.bv`'s `@FD_STDOUT`). Those are
pre-existing standalone-linking artifacts, not proof failures; the runner
prints them in a separate section for that reason.
