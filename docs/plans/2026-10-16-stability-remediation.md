# Stability remediation — determinism gate, diagnostics, recipes, prologue unification

**Date:** 2026-10-16
**Status:** COMPLETE 2026-10-16 — S1 `e5dd108c`, S5-A `d7740513`, S2 `6d9d1cea`
(recovery-cascade fix included), S3 `14d976d5`, S6 `27ff8d0c`, S4 `d7e2b10f`;
fixture cleanup `44a75ddc` (six `[true][true]` test programs → legal
`[beginprogram][true]`; found by author review — backend tests bypass the
proof engine, so fixture legality needs the same review discipline as
source). Standing guards: `ll_determinism_check.sh --quick` per commit,
full mode + toolchain matrix on backend-touching commits.
**Scope (locked in review):** S4 prologue-only; no CI (local scripts only);
S6 ledger-only.
**Trigger:** stability assessment after the cpp-vs-briev session
(`2026-10-16-cpp-vs-briev-contributor-findings.md`): correctness strong on
common shapes; seven weaknesses found and enumerated there.

## Weaknesses → work items

| # | Weakness (evidence) | Item |
|---|---|---|
| 1 | Emission-path multiplicity: the reassigned-let entry-alloca fix lived in 2 of 4+ body-emission paths for months; uncovered paths emitted dominance-invalid IR (flaw 4, fixed 2026-10-16) | S4 |
| 2 | Nondeterministic `.ll` was live (HashMap-ordered alloca registers, per-process random output; latent since 2026-10-14, caught only by a double diff) | S1 |
| 3 | Low-frequency shapes rot silently (giant zeroinit store; 1400-statement node) | S1/S4 standing gates |
| 4 | Reserved words give misleading errors (`halt` as a name → "expected identifier, found '}'" at the wrong site; found by bisection) | S2 |
| 5 | Reactive proof front door learnable only by folklore (4 iterations on a 15-line program with full source access; Otto cargo-culted nbody) | S3 |
| 6 | Single-LLVM testing: 17 vs 23 flipped verdicts twice in one session | S5-A |
| 7 | Knob folklore: `node_inline_weight_threshold` 600 was corpus-tuned until measured | S6 |

## S1 — `.ll` determinism gate

`benchmarks/ll_determinism_check.sh`: fixed corpus —
`benchmarks/recommender.bv`, `benchmarks/nbody_newton_accel.bv`,
`benchmarks/recommender_corpus/recommender_multinode.bv`,
`benchmarks/recommender_corpus/recommender_giantnode.bv`, a copied
`handoff_repro3.bv`, one struct/obj program (`queue_drain`) — each compiled
`--llvm` in TWO separate processes, byte-compared; failure names the program
and the house rule (§4, sorted HashMap iteration). `--quick` mode = 2
programs (per-commit checklist); full mode = backend-touching commits.
AGENTS.md **Commands** row added same-commit. Gate: green on the current
tree; standing guard for S4.

## S5-A — Toolchain matrix

`benchmarks/toolchain_matrix.sh`: discover installed clangs (17→current,
else the single system clang); per version: corpus → .ll → binary → run →
per-version verdict table. Documented as the pre-release / backend-commit
ritual. Phase B (real CI) deliberately out of scope — infra decision open.

## S2 — Reserved-word diagnostics

`Parser::expect_identifier` (parser/helpers.rs) and sibling
identifier-expecting paths: when the offending token is a keyword/reserved
token, emit "reserved word `X` cannot be used as an identifier — rename (X
is <one-line meaning>)" at the token's span, replacing the misleading
"expected identifier, found …". Word list: ONE accessor over the lexer's
reserved-ledger (lexer.rs:787) — DRY, never a second list. Tests:
reserved-as-node-name, reserved-as-let-name (new message), a genuine syntax
error keeps the old message. Golden-output tests are updated — the message
change is the deliverable.

## S3 — Authoring recipes doc

`docs/guides/reactive-recipes.md`: the known-good shapes — multi-node phase
chain (beginprogram = single-pass `[true]`; mid nodes declare real goals;
endprogram exits), counter-goal convergence, `async` + distinct-phase gates
(the Rule 22 classification), `halt`-reserved and friends (rename on
collision) — each shape paired with the exact diagnostic that fires when
you deviate (goal-provability, reactive-cycle convergence, unbounded-counter
self-cycle: all reproduced in `handoff_repro{,2,3}` this session). Working
material: `benchmarks/recommender.bv`, `nbody_newton_accel.bv`, corpus
README, execution-model doc. Cross-links: SPEC reactive section,
execution-model doc, AGENTS.md docs-ledger row.

## S6 — Config ledger

`docs/architecture/config-ledger.md`: every `ir-lowering.dbvl` +
`targets.dbvl` key → **derived / measured / provisional** + one-line
justification + derivation-protocol pointer where one exists (the inline
valve points at `benchmarks/ra_knee_sweep.sh` + the results-file section;
frame/aggregate knobs at their commits). Keys nobody can justify are marked
provisional honestly, never silently blessed. Derivation sweeps queue into
the followup-stages umbrella — ledger only, per locked scope.

## S4 — Shared body-emission prologue (largest, last)

Seam: all six drivers hand-roll `clear_locals →
predeclare_reassigned_lets → in_callable_txn reset → [arena init]`:
`emit_definition` (emit_toplevel.rs), SSA main's per-txn inlining
(loop_engine/ssa.rs), the reactive `@txn_` standard + assume_shape
emitters, `emit_async_body`, and `emit_callable_txn` for parity. Flaw 4
happened because drivers forgot the predeclare step.

Extract `emit_body_prologue(&mut self, out, body, opts: PrologueOpts)` —
exactly that sequence, parameterized where drivers intentionally differ
(assume_shape ordering; the cold-outliner's `reassigned_lets.clear()`
stays explicit at its call site). Hand-rolled blocks deleted 1:1. **Body
walkers untouched** — guard outlining, fusion, batching are real per-path
semantics; deeper unification is a future plan gated on this one's
results. Tests: unit (prologue binds slots, sets the states), corpus
`.ll` byte-identity vs HEAD (multinode, giantnode, recommender, nbody),
full suite. S1 is the standing guard once merged.

## Commit order & per-commit gates

S1 → S5-A → S2 → S3 → S6 → S4. Each commit: `cargo test --lib` green, no
new warnings, Praetor changed dirs (DIRECTORY targets), docs same-commit,
this plan's status line updated.

## Risks

S4: assume_shape prologue may differ intentionally — parameterize via
opts, never force-fit; byte-identity gates catch any slip. S2: golden-test
churn expected (behavior change is the deliverable). S6: provisional
labels must be honest, not ceremonial.

## Rule compliance

Rule 6 (1:1 prologue replacement, no path semantics changed), Rules 17/18
(prologue centralized; ledger DRY-sources the lexer ledger), house §4
(the gate exists because the rule existed unenforced), Rule 13 (docs
same-commit; INDEX row added with the plan commit), observability razor
(S1/S5 are liveness for toolchain facts). Gate fixtures reuse Otto
Westerlund's corpus programs, credited in the corpus README.
