# Wave-8 plan — the linear exit: totality, the verified gate, and the record (2026-11-21)

**Trigger.** The owner stated `term`'s design: *"If the postcondition holds when
`term` is encountered, the function terminates. If it doesn't it continues. If
any valid input would not lead to a valid term/output, compilation fails. Defns
are linear, whereas txn and node are cyclical, so you can run into the same
`term` multiple times."*

Owner direction: "Let's do this as you have proposed" — i.e. fix the semantics
first (totality, the gate, the fact licence), and treat the *surface* question
(`term` vs an explicit `return` / tail expressions in linear bodies) as its own
later change with a deprecation window and an IR-equivalence gate.

## 0. Research corrections (before anything is built)

Three findings changed the plan, and one of them retracts something I wrote
into `BUGS.md` two hours ago.

**The interpreter already implements the design.** The 2026-10-16
*term-semantics plan §1* lowered every bare `term;` to `when P { term <void>; }`
at `FunctionDef` construction (`lower_bare_terms`, `src/interpreter/eval.rs:2485`),
with `Bool(true)` (no post) lowering to an unconditional return. Measured:

```briev
let hits: Int = 0;
defn f(x: Int) -> Void { when x > 0 { term; }; hits = hits + 1; };
test ... { f(1); expect hits == 0; };     // PASS
```

So the entry I added to `BUGS.md` yesterday — *"bare `term;` in a defn:
interpreter continues, native returns"* — is **wrong**, and it must be retracted
rather than left as a phantom defect. I read the raw `Term(None)` arm
(`eval.rs:2980`) and missed the lowering pass in front of it. Both engines
agree, and both agree with the owner's design.

**The `term` gate is already conditional in one direction only.** `Term(Some)`
returns unconditionally in a defn — documented as deliberate ("defn
value-terms return unconditionally"). That is consistent with the design
*provided* the post is discharged statically, which brings us to the next
finding.

**Nothing enforces the design's two obligations.**

- *Totality:* a value-returning defn that falls off the end compiles today.
  `defn f(x: Int) -> Int { let y: Int = x + 1; };` → **OK**. Measured corpus
  exposure with a crude scan: 33 candidate defns, most false positives
  (multi-line tail expressions, union members); the genuine fall-throughs are a
  handful, and the `-> Void` ones (`http_response_reset`) are legitimate
  implicit returns.
- *Post discharge:* body verification is opt-in (`--verify-bodies`) or implied
  by a "strict profile" (`src/compile.rs:847`). A defn whose post is false
  compiles by default, so "if any valid input would not lead to a valid
  term/output, compilation fails" is not true of the default compiler.

Corpus floor for the second: 242 files — 210 PROVEN, 18 UNPROVEN, 34 ERR. So
flipping post discharge to mandatory is a campaign, not a stage; the plan
measures it and applies the contract-gate rollout discipline (evidence, then
flip), rather than quietly widening the error surface.

## 1. Totality — every non-void exit produces a value

New predicate in `src/analysis/termination.rs`, separate from
`statement_always_terminates` (which answers a different question — "is the
next sibling unreachable" — and correctly treats a node's bare `term;` as a
conditional exit):

```
statement_ends_defn(s, post_vacuous) =
    Term(Some(_)) | Tail(_) | EndProgram(_) | Trap | Halt
  | Term(None)      when post_vacuous
  | Block(body)     when list_ends_defn(body)
  | Match(arms)     when every arm's body ends the defn      (exhaustiveness
                                                              is already a
                                                              typechecker error
                                                              for sums, and an
                                                              open scrutinee
                                                              requires `_`)
```

A non-void defn whose body does not end the defn is a compile error naming the
defn, the return type, and the missing return. `-> Void` bodies are exempt:
falling off the end there produces the only value the type has.

## 2. The gate — licence facts only where the exit is unconditional

The three sites that record a guard's negation (`mod.rs:3277` elaborate,
`:5155` infer, `:5482` checkfree walker) call `statement_always_terminates`,
which excludes `Term(None)` — so `when n < 1 { term; }; loop2(n - 1)` proves
nothing. They must use `statement_ends_defn` with the current defn's post
vacuity.

Consequence to accept deliberately: the negation is licensed **by the verified
gate**, not by the syntax. A defn whose post is not vacuous and whose guard
exits with a bare `term;` gets no licence until the body is verified.

Wave-7 built a workaround for exactly this: `tests/tier1/divergent_loop_attrs.bv`
was restructured to recurse on the true side of its guard. The natural shape is
the one that should prove, so the workaround is reverted and the natural shape
becomes a test.

## 3. Post discharge — measured, then flipped

Measure the corpus with `--verify-bodies` on the default tree. Decision rule,
stated now so the outcome is not negotiated later:

- If the unproven set is small enough to fix in this wave (target: the 18
  UNPROVEN, no new ERR), make discharge required for a defn that **declares** a
  post, and fix the corpus.
- If it is not, keep the flag as the migration stage, make the gap an explicit
  `--strict` error, and record the corpus numbers in the plan — with `docs/` and
  a BUGS entry saying the design's obligation is not yet default-enforced.

Either way, the outcome is written down with its numbers.

## 4. The record

- SPEC §11.3: state the **conditional exit** as the single rule — `term`
  evaluates the completeness goal at that point; met → exit, unmet → continue;
  defns are linear (one arrival), nodes/txns are cyclic (repeated arrivals). The
  `[term …]` postcondition is what evaluates; a vacuous post makes every `term`
  unconditional.
- `src/analysis/termination.rs:152` and `src/hir/termination.rs:72` currently
  tell **defn authors** "execution continues to the statement after the guard" —
  node semantics, taught to the wrong audience. Rewrite both to say which body
  kind they are talking about.
- The `return` teaching error (`src/parser/statements.rs:163`,
  `src/parser/definitions.rs:412`) cites `term!`, which SPEC §3072 says was
  removed in favour of `endprogram`. Fix the message.
- `BUGS.md`: retract the bare-`term;` entry with the reason (the research
  correction above), so the next reader does not "fix" a non-defect.

## Invariants

| # | Property | Pinned by |
|---|---|---|
| 1 | A non-void defn that can fall off the end is a compile error naming the defn and the missing return | `non_void_defn_fallthrough_is_error` |
| 2 | A non-void defn that ends in a tail expression, `term X`, `endprogram`, or `trap` is accepted | `defn_exit_forms_are_accepted` |
| 3 | A defn whose every path returns through a `match` (all arms exiting) is accepted | `defn_match_all_arms_exit_is_total` |
| 4 | A `match` with one non-exiting arm does not make the defn total | `defn_match_one_fallthrough_arm_is_error` |
| 5 | `-> Void` bodies may fall off the end | `void_defn_implicit_return_allowed` |
| 6 | In a defn, a guard body ending in a bare `term;` with a vacuous post licenses its negation | `guard_negation_licensed_by_unconditional_term` |
| 7 | In a node/txn, a bare `term;` does NOT license its negation (the gate may continue) | `node_term_does_not_license_negation` |
| 8 | The natural recursive shape `when n < 1 { term; }; loop2(n - 1)` proves and compiles | `natural_recursive_base_case_proves` + `tests/tier1/divergent_loop_attrs.bv` (workaround reverted) |
| 9 | Interpreter and native agree on the conditional exit, measured through the reference interpreter | `tests/tier1/term_gate_semantics.bv` (SPEC §8.11 blocks) |
| 10 | Post discharge status is stated in the plan with corpus numbers, whichever way §3 lands | review + the numbers recorded here |
| 11 | SPEC §11.3 states the conditional-exit rule once, for both body kinds | vocab/SPEC gate + `docs/plans` cross-reference |
| 12 | No diagnostic teaches node semantics to a defn author, or cites a removed keyword | `diagnostics.rs` test on the guard-warning text; `return`-error test |

## Tests to write

- `src/analysis/termination.rs` unit tests: `statement_ends_defn` over each AST
  form (tail, `term X`, bare `term;` with/without vacuous post, `trap`,
  `endprogram`, block, match all-arms / one-arm, foreach body).
- `src/typechecker/mod.rs` tests: invariants 1–4, 6–8 (mutation-checked: dropping
  `Term(None)` from the licence must fail #6 and #8; adding it for nodes must
  fail #7).
- `tests/tier1/term_gate_semantics.bv`: interpreter blocks for #9.
- `cargo test --lib` corpus: whichever fixtures the new totality rule finds.

## Non-goals (deferred by the owner's direction)

- Removing `term` from non-void defns, or introducing `return`. That is the
  surface decision, sequenced after this wave, with a deprecation window and a
  byte-identical-`.ll` A/B over the corpus (`ll_determinism_check.sh`).

## Outcome (2026-11-21)

**R1 — record corrected.** The BUGS.md entry claiming the interpreter continues
past a bare `term;` was withdrawn: `lower_bare_terms` (2026-10-16) already gates
it, and a measurement through the reference interpreter confirms the exit. The
termination header that repeated the same misreading is corrected too.

**R2 — totality landed.** `defn_totality_errors` + `stmts_end_defn` in
`src/analysis/termination.rs` (via `analyze_all`, so `check` and `build` share
it). Corpus after the rule: **0 failures** across lib/, tests/, examples/ and
benchmarks. Three fixtures were corrected along the way, and one of them exposed
a genuine specification/emitter disagreement:

- a trailing expression STATEMENT is the value (`defn f(a, b) -> Int { a + b; }`
  returns 3). SPEC §11.3 says the tail is written without `;`. The rule follows
  the emitter, because a totality check that rejects working code teaches
  authors to fight the compiler; the `;` question is recorded for the surface
  decision below.

**R3 — the licence landed.** The three fact sites now ask
`stmts_end_defn(body, post_vacuous)`, where vacuity is answered from the
contract (`current_defn_post_is_vacuous` / `post_is_vacuous`, sharing their
reasoning). `when n < 1 { term; }; loop2(n - 1)` proves as written, and the
wave-7 workaround in `tests/tier1/divergent_loop_attrs.bv` is retired. A node's
bare `term;` still licenses nothing — pinned at the CLI, because a unit test
through `check()` cannot observe the context decision. The bare-term hint is now
kind-aware: it no longer tells a DEFN author that execution continues, and no
longer cites the removed `term!`.

**R4 — post discharge: measured, NOT flipped.** The corpus baseline is 242 files
— 210 PROVEN, 18 UNPROVEN, 34 ERR (`BUGS.md`, 2026-11-16 hygiene pass). Making
discharge mandatory would fail ~52 files: that is a campaign, not a stage, so
per the decision rule written above the flag stays the migration stage and
`--strict` remains the enforcement path. **The design's obligation is therefore
NOT default-enforced**, which is recorded here and in the SPEC text rather than
implied. Flipping it is the next wave's first item, measured the same way.

**R5 — record.** SPEC §11.3 now states the conditional exit once, for both body
kinds, plus the two compile-time obligations. The `return` teaching error no
longer cites `term!`. `tests/tier1/term_gate_semantics.bv` pins the linear half
through the reference interpreter.

**Not done, deliberately:** the surface decision (removing `term` from non-void
defns / introducing `return`) stays out of this wave, as sequenced.
