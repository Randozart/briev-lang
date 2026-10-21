# Match result types exclude terminating arms (2026-10-21)

Recommender Task 10.4 loaded all 100,000 source ratings but its persisted-engine
adapter exposed two separate issues: Result's wrong-side unwraps fabricated
values despite their documented trap behavior (stdlib fix preserved with feature
work), and a match containing a trap arm erased a continuing arm's result.

Controlled experiment on the existing integration binary: ordinary Bool match
with Int arms compiles; replacing one value arm with `{ trap; }` fails clang
with an undefined return register. The probe imports neither Result nor app code.

Mechanism: match type probing treated the terminating arm's Void as a continuing
valueless arm, suppressing the merge phi. It also did not reset/restore the
termination flag between probe emissions. Extract the existing probe with its
binding/alloca/block snapshot logic intact; retain types only for arms whose
emission reaches the merge, and restore termination state. Preserve genuine
continuing-Void behavior and the existing all-arms-terminate path.

Tests: trap arm in both orders, pointer result, nested all-terminating match,
and continuing-Void control; validate IR with installed clang. Native minimal
probe builds/runs after the human rebuilds the compiler. Full cargo test --lib
and cargo build remain human-run. Praetor before/after comparison on changed
directories; backend determinism/toolchain gates after rebuilding. No performance
claim or optimization strategy change. Update BUGS.md, net-stack bug log, and
backend rationale; preserve existing commentary when extracting the probe.

Feature edits are retained in scratch/task10-feature-progress-20261021.patch.
Untracked feature sources and gate fixtures remain in the workspace. Apply that
patch ONLY after returning to integration/ptr-provenance; it is not part of this
compiler-fix PR. Branch: fix/match-terminating-arm-values, based on main.

Status: fix/tests authored; human cargo verification pending.

Praetor before/after (2026-10-21): no added diagnostics on changed files or the
new test module. Sorting normalized report lines removes report-order changes;
remaining differences are improvements in emit_match: cyclomatic 27 -> 26,
cognitive 47 -> 46, lines 267 -> 238. Existing diagnostics are preserved; the
new probe helper and regression module have none. Human cargo gates pending.

Verification (2026-10-21, agent execution authorized for this session): cargo
check/build pass without new warnings; all four focused regressions pass with
clang IR verification. Native value arm returns 42; trap arm exits SIGILL (132).
Full cargo test --lib: 3157 passed, 38 failed. Every failure name occurs in the
human's earlier report, including adapters.dbvl schema conformance; no added
failure names. This is not a fully green suite.

Full determinism corpus: six emitted modules are byte-identical between separate
processes; giantnode is refused by unchanged frontend liveness checking before IR
emission. Toolchain matrix: installed clang 17 passes recommender; multinode has
no root input fixture and is skipped; giantnode has the same frontend refusal.
Neither broad harness is claimed fully green. Focused match IR is byte-identical.
Praetor reports no new diagnostics. All four temporarily held feature modules
were restored to examples/recommender; tracked feature changes remain in the
scratch patch. WORKFLOW.md was not edited for the session authorization.