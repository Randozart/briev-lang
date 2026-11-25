# wasm32 debt 27 → 0: fix the counter, then four emitter families

**Date:** 2026-11-25
**Predecessor:** `docs/plans/2026-11-25-wasm32-supported-target-gate-and-width-refactor.md` (P0–P3 landed at `55f1077c6`)
**Status:** plan only. Nothing implemented.

---

## 0. First, a correction I owe

The wave I just landed reported the wasm32 debt as **12**, with 15 programs "refused
for the target". That was wrong, and wrong in the optimistic direction.

The classifier I added in P2a keys on log text:

```bash
if grep -qE 'walked the live program|try-mode miss|no endprogram' "$log"; then
    printf 'REFUSED\n'
```

But `walked the live program … no endprogram, halt, or trap is reachable` is a
**warning**, not a refusal — the compiler prints it and then emits anyway. Checked
across the 15 files classified as refusals:

> **all 15 contain `rejected the module`** — every one of them is invalid IR.

So the debt is **27**, exactly the number I "corrected" down from. Two consequences:

1. `config/cross-target-baseline.toml` understates reality by 15 and must go back to
   27 — today, before any other work, so nobody plans against a fiction.
2. The gate has the exact defect I wrote a paragraph about two commits earlier (*"a
   gate that can under-report is worse than a slow one"*). That paragraph was right;
   I should have applied it to my own classifier.

Confirmed mechanically, not by argument:
`tests_tier1_test_piggybank_bv` was classified REFUSED and its log contains
`Briev's emitted-module check rejected the module: line 4504 …`. And running LLVM
directly on the emitted module reproduces it:

```
$ llc -mtriple=wasm32-unknown-wasi test_piggybank.ll
4504:13: error: '%t245' defined with type 'i32' but expected 'i64'
  store i64 %t245, ptr %t15
```

So `mistyped_operands` is correct on its verdicts; only the sweep's *classification*
of failures was wrong. (The bisect lesson from last wave applies again: verify the
instrument before trusting its number.)

---

## 1. What the 27 actually are

Not 27 bugs. Four emitter families, by offending instruction at the reported line
(extracted from the emitted modules, cross-checked with `llc`):

| family | offending instruction | programs |
|---|---|---|
| **A. `inttoptr` source width** | `%r = inttoptr i64 %v to ptr` (8), `inttoptr i32` (1) | **9** |
| **B. call-argument width** | `call i32 @briev_getenv_int_impl(i32 …)` (6), `call ptr @int_to_str(i32 …)` (2), `call ptr @briev_str_substr(ptr …)` (1), `call ptr @briev_slice_strided64(ptr …)` (1) | **10** |
| **C. arithmetic / comparison width** | `%r = shl i64` (2), `%r = mul i32` (2), `%r = icmp slt i32` (2) | **6** |
| **D. store width** | `store i64 %v, ptr %p` (2) | **2** |

Family A is the documented 103-`inttoptr i64`-site class. Family B is the mirror of
the wave-3 float fix: `adapt_arg_to_param` exists and handles `float`/`double`
parameters, and does **not** handle integer parameters, so an i64 (or i32) register
is passed into a parameter declared at the other width. Families C and D are the
`operand_width` rule applied to emitters I have not touched yet.

Every one of these is invisible on x86-64: there the hardcoded word *is* the right
width, which is why 4646 tests pass while 27 modules are invalid for the shipped
32-bit target.

---

## 2. Invariants

| id | invariant | pinned by |
|---|---|---|
| **J-1** | A program is counted as IR debt if and only if the compiler **emitted a module it declared invalid**. Every other failure is counted as a failure too, unless the log positively shows no module was written. | `tests/cross_target_baseline.rs::a_failure_with_both_markers_is_debt_not_refusal` + `scripts/cross_target_ir.sh --classify` self-check |
| **J-2** | Unknown failures default to being **counted**, never excused. | `the_classifier_never_excuses_an_unrecognised_failure` |
| **J-3** | `inttoptr`'s source width is the width of the value it converts, asked from the value. | `word_to_ptr_asks_the_value_for_its_width` (unit) + family A programs |
| **J-4** | A call argument's width is the parameter's declared width; the value is converted, never re-typed. | `an_integer_argument_is_converted_to_the_declared_parameter_width` (unit) + family B programs |
| **J-5** | An arithmetic, comparison or store width is asked from the operand (`operand_width`), never written as a literal. | `operand_width_is_used_by_every_typed_emitter` (source scan) + family C/D programs |
| **J-6** | Native (x86-64) emitted IR is byte-identical throughout, except where a stage states otherwise. | `ll_determinism_check.sh --quick` + the 12-fixture byte diff per stage |
| **J-7** | The wasm32 invalid-IR count strictly decreases at each stage and reaches **0**; the baseline is decremented in the same commit. | `scripts/cross_target_ir.sh wasm32-unknown-wasi` at each stage |
| **J-8** | The width-literal budget only decreases. | `tests/width_literal_ratchet.rs` |

---

## 3. Stages

### S1 — the counter (must be first, ~1 hour)

1. In `scripts/cross_target_ir.sh`, classify **debt first**:
   - if the log contains `rejected the module` / `LLVM rejected` → `FAIL` (debt);
   - else if it contains a refusal marker (`walked the live program`, `try-mode
     miss`, `no endprogram`) → `REFUSED`;
   - **else → `FAIL`** (J-2: an unrecognised failure is a failure).
2. Add `--classify <log>` mode to the script so the rule is testable without a
   sweep, and three tests: both markers → debt; refusal only → refused; garbage →
   debt.
3. `config/cross-target-baseline.toml`: `wasm32 = 27` again, with the corrected
   rationale (and the note that a *warning* is not a refusal, since that is the trap).
4. Correct `BUGS.md`: the 12/15 split was wrong; 27 invalid IR; name the classifier
   bug as the cause. Do not delete the earlier entry — the record is the record.

Exit: sweep reports **27 failed, 0 refused**; tests green; no code change.

### S2 — family A: `inttoptr` asks the value (9 programs, ~1 day)

The 103 hardcoded `inttoptr i64` sites become calls to one helper that takes a
`&TypedRegister` (or a name + type) and asks `operand_width`; when the value is
already `ptr`, it emits nothing.

Order of work: introduce the helper and route the sites the 9 failing programs
reach, then the remainder by budget (each conversion lowers
`config/width-literals.toml`). Do **not** batch all 103 in one commit: the gate must
show the count dropping per commit.

Exit: baseline 27 → 18; native IR identical; ratchet counts lowered for the files
touched.

### S3 — family B: integer argument adaptation (10 programs, ~1 day)

`adapt_arg_to_param` today:

```rust
let param_ll = self.llvm_type(param_ty);
let param_is_float = param_ll == "float" || param_ll == "double";
if !param_is_float { return None; }   // ← integers fall through, unconverted
```

Extend it to integer parameters: take the value's width from
`operand_width(&reg)`, and when it differs from the parameter's declared width,
emit the conversion (`sext` for a signed source, else `zext`/`trunc`) and return the
converted register. The float path already does exactly this shape, so the change is
a sibling arm, not a redesign.

Special care: the runtime ABI symbols (`briev_getenv_int_impl`, `int_to_str`,
`briev_str_substr`, `briev_slice_strided64`) have **fixed-width** C signatures.
Either the call site adapts to the declaration (preferred) or the declaration
becomes target-parametric — but not both, and the choice must be recorded in the
comment, because the two together would convert twice.

Exit: baseline 18 → 8.

### S4 — families C and D: the remaining typed emitters (8 programs, ~1 day)

Route `shl`/`mul`/`icmp`/`store` widths through `operand_width`. The `shl i64` and
`mul i32` cases are the interesting ones: they declare a width the *operands* do not
have, which means the emitter decided from a type rather than from the value.

Exit: baseline 8 → **0**. Gate green at zero; `cross_target_ir.sh` becomes a real
pass/fail gate with no baseline slack for wasm32.

### S5 — finish P1's channel clear (~half day, only after S2–S4)

P1 named the three reset scopes but deliberately did not drop counter-scoped entries
from the representation channel, because the records were not truthful. After S2–S4
the recorders that stamp a *canonical* width (`emit_expr.rs:13137`, `:13526`) are the
remaining liars; fix them to record the emitted width, then add
`counter_scoped_regs` provenance (populated by the three minters) and drain it in
`begin_function`. This is the structural completion of the invariant the plan was
built on, and it becomes safe only once every recorder is honest.

Exit: native IR byte-identical; `rbv_list_each_wasm` green; a test that a
counter-scoped name minted in one body has no record in the next.

### S6 — last ownership violation (~2 hours, optional)

~6 sites do `backend.fun.txn_counter += 1` directly outside `FunctionContext`
(`emit_hir_stmt.rs:492,823,987,1015`, `emit_toplevel.rs:574`, …). They are minting,
not resetting, so they are not a bug — but "one owner" is now true for resets and not
for mints. Route them through `next_reg`/`next_reg_with_prefix`, or extend the
existing source-scan test to allow `+= 1` with an explanation. Decide once, in code.

### S7 — the nightly, separately

`Verification nightly` has been red since `96f940883` (2026-11-23) on
`tests/saltpetre_gemm_baseline.rs:111`, and does not reproduce locally (exit 0,
`SALT_GEMM_BAD=0`). It is **not** part of this plan's scope, but it should not be
forgotten: the next step is to read the run's captured fixture text
(`gh run view <id> --log-failed`) to separate a non-zero exit from a plausibility
bound. Tracked in BUGS.md.

---

## 4. Named tests

| test | where | asserts |
|---|---|---|
| `a_failure_with_both_markers_is_debt_not_refusal` | `tests/cross_target_baseline.rs` | a log containing a liveness **warning** and an invalid-module **error** classifies as debt (this is the bug that produced 12-vs-27) |
| `the_classifier_never_excuses_an_unrecognised_failure` | same | an unrecognised failure is counted, not skipped (J-2) |
| `word_to_ptr_asks_the_value_for_its_width` | `src/backend/llvm/tests.rs` | `inttoptr i32` for a 32-bit value, `inttoptr i64` for a 64-bit one, nothing for `ptr` |
| `an_integer_argument_is_converted_to_the_declared_parameter_width` | `src/backend/llvm/tests.rs` | an i64 register at an i32 parameter emits one `trunc`; an i32 at an i64 parameter emits `sext`/`zext`; equal widths emit nothing |
| `operand_width_is_used_by_every_typed_emitter` | `src/backend/llvm/tests.rs` | source scan: the shl/mul/icmp/store emitters contain no `i64` literal in their type position |
| `every_definition_records_its_width` | `src/backend/llvm/tests.rs` | after S5: no recorder stamps a width the emission did not use (source scan + the two `emit_expr` sites) |
| `no_counter_scoped_record_survives_a_body` | `src/backend/llvm/context.rs` | after S5: mint in body A, record, `begin_function`, record is gone; a position-derived `%ac{i}` record survives |

---

## 5. Risks and failure modes

| risk | mitigation |
|---|---|
| The classifier fix flips a genuinely-refused program into debt, changing the number again | S1 ships alone, before any emitter work, so the noise is isolated; LLVM is the arbiter for any disputed case (`llc` on the emitted module) |
| `inttoptr` narrowing to the word breaks programs that legitimately box a handle as i64 | the helper emits only when the widths differ, and the slot store already zero-extends; the 9 family-A programs are the guard, plus the full gate and byte-identical native IR |
| Integer argument conversion double-converts (both caller and declaration adapted) | pick ONE side per ABI class and write the choice in the comment; the unit test asserts "equal widths emit nothing" |
| A stage lowers the baseline but a later stage regresses one program | the baseline may only decrease and the sweep names every newly failing program, so a regression cannot hide behind a net improvement |
| Native IR changes where a width was accidentally load-bearing | byte-identity requirement per stage; if a change is intended, the stage says so with the fixture that moved |
| `.rbv` (the shipped web path) regresses | the sweep already includes all 29 `.rbv` programs; a web-path regression fails the gate immediately |

---

## 6. Acceptance

1. `cross_target_ir.sh wasm32-unknown-wasi` → **0 failed, 0 refused**; aarch64 stays 0.
2. `cross_target_ir.sh --classify` self-tests green (J-1, J-2).
3. Native IR byte-identical on the 12-fixture baseline at every stage; only
   stated, fixture-verified changes allowed.
4. `width-literals.toml` lowered for every file touched, never raised.
5. Full gate green at each landing; CI 4/4; determinism clean.
6. Recorded in BUGS.md and this plan, including the corrected 12→27 history, so the
   next reader knows the counter was fixed rather than always right.

---

## 7. Estimated shape

| stage | size | risk |
|---|---|---|
| S1 counter | ~1 h | none (no code) |
| S2 family A | ~1 day | medium — it is the 103-site class |
| S3 family B | ~1 day | medium — ABI ownership decision |
| S4 families C/D | ~1 day | low |
| S5 channel clear | ~½ day | medium — byte-identity is the guard |
| S6 mint ownership | ~2 h | low |
| S7 nightly | separate | unknown until the log is read |

---

## 8. Outcome (measured 2026-11-25, incomplete — read the table)

| stage | state | measured |
|---|---|---|
| **S1** counter | **LANDED** `d2ec569e9` | debt-first classifier; `--classify` mode; 3 tests; baseline back to **27**; 0 refusals reported |
| **S2** family A | **PARTIAL** `c8395d45c` | `inttoptr_word` wired at 49 sites; provenance (`counter_scoped_regs`) + counter-scoped clearing, the deferred half of P1; both `malloc` paths record their handle width. **27 → 26**, zero regressions |
| **S3** family B | **REVERTED, measured** | see §9 — two variants, both regressed |
| **S4** families C/D | **NOT STARTED** | 8 programs |
| **S5** channel clear | **LANDED EARLY** (inside S2) | it was a prerequisite of S2, not a follow-up |
| **S6** mint ownership | **PARTIAL** `note_counter_scoped` | closes the provenance hole for hand-minted `%t{n}`; label minting left alone (no width records) |
| **S7** nightly | untouched | pre-existing, needs the CI log |

## 9. Two S3 variants, measured and rejected

Integer argument adaptation is the obvious fix for ten of the twenty-six, and both
obvious shapes made things worse. This is recorded because the next attempt will reach
for exactly these:

**Variant 1 — convert every integer argument (zext and trunc).** 26 → **30**. Four new
failures (`byte_sink_trait_smoke`, `net_stack_helpers`, `trait_generics_smoke`,
`unix_socket_loopback_smoke`) report *"the module calls `@put_bytes`, `@put_u8` with no
definition or declaration"* — the extra conversion register changed which defns the
liveness pass emits. The width fix reached into dead-code elimination.

**Variant 2 — narrow only, source width from the argument's declared type.** 26 → **27**.
`net_stack_helpers` still fails: `%ac3` is declared `Int` but emitted **i64**, and it
reaches a call site that never passes through `adapt_arg_to_param` at all. Asking the
record instead (`operand_width`) did not help — same failure, different reason: the call
is on another path.

**Conclusion, and the correction to this plan's own ordering:** the missing piece is not
the adapter, it is (a) a call-argument path that *every* call site shares, and (b)
records that describe the emitted width rather than the declared type. S2 already
showed the same dependency from the other direction. So the honest order is:
**make every producer record what it emitted → one shared call-argument path → then the
width conversion**, not the reverse.

## 10. What remains, precisely

1. **The missing recorders.** `inttoptr` reads the record, and `%t115 = load i64` with a
   record saying `i32` (reproduced on `test_hashmap_surface`) is why family A only got
   one program. Every instruction that defines a register must record the width it
   emitted; the site-by-site list is what S2's failure logs name.
2. **A shared call-argument path** (family B, 10 programs).
3. **`shl`/`mul`/`icmp`/`store` widths** (families C/D, 8 programs) — `operand_width`
   already exists for this; it is mechanical once (1) is true.
4. **S7**, untouched.

The gate is honest now, which is the durable part: 26 is measured, counted, and cannot
move up without a failing build.
