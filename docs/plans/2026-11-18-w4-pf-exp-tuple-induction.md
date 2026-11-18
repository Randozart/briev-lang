# W4 — `pf_exp` tuple induction: the multi-field value-summary gap (2026-11-18)

Part of the trunk-stability plan (`docs/plans/2026-11-18-trunk-stability-plan.md`,
W4). This document re-scopes W4: the original framing (add `i >= 0` to
`pf_exp`, give `pf_exp_digits` a tuple postcondition, fix the clamp, build the
caller ladder) is **already measured and reverted** in
`docs/plans/2026-11-17-pf-exp-float-slot-and-ptl-queue.md` §A. It is a
relabelling, not progress. The real blocker is the multi-field (tuple)
value-summary gap — value-summaries "phase 2"
(`docs/plans/2026-11-15-value-summaries.md` §3.2). This document states that
finding, derives the principled fix, and sequences it.

## Finding 0 — the contract-only ladder was already refuted

`docs/plans/2026-11-17-pf-exp-float-slot-and-ptl-queue.md` §A measured, on
`lib/std/cast_lanes.bv` alone, four moves and reverted them all:

1. **Adding `i >= 0` to `pf_exp`'s precondition changed nothing.** `pf_exp`
   stayed at "could not prove the postcondition on path 1 of 6".
2. **Rewriting `pf_exp_digits`' postcondition as the tuple form**
   `term.0 >= -1000 && term.0 <= 1000 && term.1 >= 0` moved `pf_exp` from
   *unproven* to *skipped* and proved nothing: `0 unproven / 142 skipped`.
   That is a relabelling — exactly the failure mode the corpus discipline
   exists to prevent.
3. The skip message is `"a call to pf_exp_digits, which has no value summary"`.

The 2026-11-17 measurement concluded: *"the blocker is the representation of a
tuple result, which is phase 2 of the value-summaries plan ('two obligations,
one per field, or a record sort in SMT'). The 172-of-207 number is the argument
for scheduling that phase, not for editing the stdlib."*

## Finding 1 — confirmed with a fresh probe (2026-11-18)

A probe file (`/tmp/probe_pfexp2.bv`, renamed to avoid the prelude's shared
root namespace) reproduces the skip independently of preconditions:

```
warning: --verify-bodies could not prove: pf_exp: ... path 1 of 6
info: --verify-bodies skipped: pf_exp2: a call to `pf_exp_digits2`, which has no value summary
info: --verify-bodies skipped: pf_exp_digits2: a call to `pf_exp_digits2`, which has no value summary
```

Even with the added `i >= 0 && i <= len` preconditions, both `pf_exp2` and
`pf_exp_digits2` are **skipped**, not proven. The skip is structural — it comes
from the value-summary gate (`verify.rs:3103`, `a call to X, which has no value
summary`), not from a missing contract fact. Precondition edits cannot close it.

## Finding 2 — where the gap actually is (mechanism trace)

The body prover has two independent surfaces, and only one works:

- **POST side (works).** `bind_output` (`verify.rs:1955`) binds each field of a
  tuple return separately under `term.N`, and deliberately leaves the whole
  value unbound. The existing test
  `tuple_fields_prove_from_the_precondition` (`src/analysis/verify_bodies.rs:1366`)
  proves `[acc >= 1 && k >= 0] [term.0 >= 1 && term.1 >= 0]` for the
  straight-line body `term (acc, k)`. So a *declared* tuple postcondition is
  checkable against a *straight-line* body.
- **CALL-SITE / summary side (the gap).** `Summary { params, value: Expr }`
  (`verify.rs:3130`) is **single-valued**: one `Expr` per callee. `build_summaries`
  (`verify.rs:3141`) iterates to a fixpoint and **rejects recursion** (a
  self-referential summary is a guess, not a proof — the sound answer).
  `evaluate_body` (`verify.rs:3206`) returns a single value. `inline_expr`
  (`verify.rs:3285`) substitutes `sum.value` as a scalar, and `substitute_params`
  (`verify.rs:3325`) has **no `Field` arm**, so it cannot bind `term.0`/`term.1`.

Consequently:

- A callee that **returns a tuple** can never earn a `Summary` (its result has
  no single `Expr`).
- A callee that is **recursive** can never earn a `Summary` (fixpoint rejects it).

`pf_exp_digits` is *both*, so it can never earn a summary, so its declared
postcondition never reaches `pf_exp`'s call site, so `pf_exp`'s `term.0`/`term.1`
bounds over the recursive arms are undecidable. `pf_exp` is *not* recursive
(it calls `pf_exp_digits`), but it cannot be proven because it calls a
callee the summary machinery cannot model.

This is the precise content of "PF_exp induction": the prover must be able to
**consume a recursive callee's declared tuple postcondition at the call site**,
certified by the callee's own inductive step, instead of demanding a
single-valued value summary.

## The principled fix — value-summaries phase 2, for the tuple case

The fix is *not* more stdlib contracts. It is extending the prover's value
model so a tuple-returning callee contributes what it declares. Three layers,
in dependency order:

### Layer 1 — multi-field summaries (the tuple case)

Generalise `Summary` from one `value: Expr` to a tuple-aware form. A
**non-recursive** callee whose body is a straight-line `term (e0, e1, ...)`
earns a per-field summary: a mapping from each `term.N` to the expression over
the callee's parameters that `evaluate_body` already computes per component.
`inline_expr` / `substitute_params` gain the `Field`-aware substitution so a
call site like `(e, i2) = pf_num(...)` can read `pf_num`'s field `term.3` (the
next index) as a concrete expression instead of an opaque atom.

This layer alone discharges the *caller-ladder* fact that is currently
undecidable: `i2 = pf_num(...).term.3`, and `i2 >= 0` follows from
`pf_num`'s declared postcondition `term.3`-bound once the field is readable.

**Scope note (Rule 23 / phase 2 honesty).** This is the *representation*
extension the value-summaries plan deferred. It is additive: single-valued
summaries keep working unchanged; only tuple-returning callees gain a field
view. No existing contract is weakened; no proof is retracted.

### Layer 2 — the inductive step for recursive tuple callees

For a **tail-recursive** callee (the `pf_exp_digits` shape), a per-field
summary is still a guess. The sound substitute is an **inductive invariant**
over the accumulator, certified by the callee's own body:

- **Established at entry** (the invariant holds at the call's initial state).
- **Preserved by one pass** (the invariant is maintained across the recursive
  step).

When both obligations are discharged (reusing the existing Houdini-style
`infer_invariant` machinery, `verify.rs:2589`, which already proves
establishment + preservation for the *loop* shape), the callee's declared
postcondition on the *returned tuple fields* is exposed to the caller as a
fact — exactly what `build_call_contracts` / `post_facts_for_atom` do for
single-valued callees, extended to `term.N`.

**Rule 1 (advisory).** When the invariant **cannot** be established for a
given callee (e.g. the accumulator's bound depends on a fact the prover cannot
model), the caller **stays advisory-unproven**, never a CI failure and never a
silent proof. The unproven goal is reported with the reason *"a call to
`pf_exp_digits`, whose inductive invariant could not be established"*. This is
the conservative-pure-integer-subset boundary: the prover proves what the
induction covers and is honest about the rest.

### Layer 3 — the code fix the contract demands (contract-first, Rule 2)

Independent of the prover, `pf_exp_digits`'s clamp **contradicts its own
commented contract** and must be fixed in the *code*:

```
when acc < 1000 {
    term pf_exp_digits(s, i + 1, len, acc * 10 + (b - 48), sg);
};
```

With `acc = 999` and digit `9`, the next accumulator is `9999`, against a
declared/commented cap of `<= 1000`. To guarantee `<= 1000` after
`acc * 10 + d` with `d <= 9`, the guard must be `acc < 100`. This is the
contract-first fix: keep the contract `[term.0 <= 1000]`, fix the guard.
The 2026-11-17 plan recorded this as "recorded, not changed" because the tuple
postcondition did not yet *mean* anything to the prover; Layer 1/2 give it
meaning, so the clamp fix is now both necessary and checkable.

## Sequence (one rung at a time, re-measured)

Each rung re-runs `bash scripts/verify_bodies_corpus.sh lib/std` so a change in
one class cannot hide behind aggregate noise (the 2026-11-15 corpus-discipline
rule).

1. **Fixtures before edits.** A minimal pair under `tests/tier1/`:
   - a **non-recursive tuple callee** that must become *provable* via the
     multi-field summary (Layer 1's positive case);
   - a **recursive tuple callee** whose inductive invariant *cannot* be
     established, which must **stay advisory-unproven** with the named reason
     (the Rule 1 negative case).
   These pin the prover's behaviour independently of the stdlib.
2. **Layer 1** — multi-field `Summary` + field-aware substitution. Re-measure:
   the non-recursive fixture flips skipped → proven; the recursive fixture
   stays unproven (unchanged, so no regression).
3. **Layer 2** — inductive step for recursive tuple callees. Re-measure: the
   recursive fixture either flips to proven (invariant established) or stays
   advisory-unproven with the named reason — never violated.
4. **Layer 3** — fix the `pf_exp_digits` clamp to `acc < 100` (code) + build the
   caller ladder (`i2 >= 0` from `pf_num.term.3`, `str_to_float`). Re-measure:
   `pf_exp`'s goal is either discharged or stays advisory-unproven with the
   named reason. `goals_violated` must stay **0**.
5. **Kani + briev-verify tests.** A Kani harness (standalone crate, standard
   `assert!`) for the inductive step's accumulator bound; a `briev-verify` unit
   test that the recursive-tuple fixture's invariant is established + preserved
   (or, when not, that the report is advisory-unproven with the named reason,
   never violated).
6. **Census + docs + commit.** Final `lib/std` census; update this doc, the
   value-summaries doc (mark phase 2's tuple case addressed or record the
   residual), the corpus floor note; commit + sync + push.

## Gates

- `goals_violated` for the corpus stays **0** (a contract falsely proved is the
  gate; Rule 1 / Rule 2).
- The non-recursive tuple fixture: skipped → proven (Layer 1 works).
- The recursive tuple fixture: never violated; either proven or
  advisory-unproven with the named reason (Rule 1 works).
- `pf_exp`'s goal: discharged or advisory-unproven with the named reason —
  never a silent "unproven" with no cause.
- `cargo test -p briev-verify` green; `cd kani && kani prove <harness>` passes;
  `cargo nextest --lib` green (the parser is on the hot path of every float
  parse — the golden outputs are the net).

## Risk

**Medium.** This is the only workstream in the trunk-stability plan that adds
new *proof* surface. It must not weaken any existing contract (Rule 1) and must
stay advisory for uncovered cases. It is the one that cannot be verified by a
green CI job alone — it needs the proof to actually go through, or an honest
advisory-unproven report. The additive design (Layer 1/2 extend, never replace,
the single-valued path) bounds the blast radius: a regression would show as an
existing single-valued fixture losing its proof, which the gate catches.

**Honest-scope caveat.** The 2026-11-17 measurement showed the *contract-only*
ladder cannot close `pf_exp`. This plan adds the missing *prover* machinery
(tuple summary + induction). If, after Layers 1–2, `pf_exp`'s particular
recursive shape still does not fit the inductive template (e.g. the exponent
accumulator's bound depends on the string's digit run in a way the template
does not capture), the correct outcome is an **advisory-unproven report with the
named reason** — not a forced proof, and not a weakened contract. That is the
Rule 1 boundary, and it is the honest end-state for the cases the induction
does not yet cover.
