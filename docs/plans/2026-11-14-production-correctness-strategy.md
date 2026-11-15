# Production compiler correctness: evidence, oracles, and an execution plan

**Date:** 2026-11-14. **Status:** proposed implementation strategy, grounded in
a source/CI audit. **Audited revision:**
`e9169750f23e2bd9d9fbd2168c682366a52edced` on upstream `main`.
Source locations and counts below refer to that revision; CI runs identify
their own revisions. Historical passes are not evidence that this revision
passes. Implementation milestones below are proposals, not completed work.

## Recommendation

Briev already has useful correctness infrastructure. The highest-return next
step is to make its results trustworthy and exercise the production path with
strong observations. Increasing random-test volume comes after that.

Start with five priorities, in order:

1. **Make green mean executed and checked.** Repair required CI failures;
   distinguish missing tools, skipped cases, timeouts, and unsupported features
   from success. Prove that the harnesses detect planted failures.
2. **Compare complete semantic results.** Extend the existing differential
   tests to the default HIR path, full-width values, collections, and effects.
   Use independent small models as well as the reference interpreter.
3. **Attack proof and ownership boundaries.** False proofs and invalid
   lifetime/alias facts can license silent wrong code across many programs.
   Verify both the analysis and its code-generation consumer.
4. **Grow structured, reproducible campaigns.** Generate well-typed programs,
   meaningful feature combinations, and replayable event histories; retain
   shrinking and failure identity throughout.
5. **Qualify the shipped product.** Run the actual release compiler and
   packaged resources across supported targets, toolchains, ABIs, and failure
   conditions, including real devices where required.

The objective is an accumulating, reviewable correctness argument:
**language rule → obligation → oracle → executed test/proof → retained evidence**.
Test counts, coverage percentages, and agreement between implementations are
useful signals, but none independently establishes compiler correctness.

### Reading map

- [1. Audited baseline](#1-audited-baseline)
- [2. Lessons from mature projects](#2-lessons-from-mature-projects)
- [3. Correctness contract and oracle design](#3-correctness-contract-and-oracle-design)
- [4. Risk-driven test campaigns](#4-risk-driven-test-campaigns)
- [5. Tools and their assurance boundaries](#5-tools-and-their-assurance-boundaries)
- [6. CI and release execution](#6-ci-and-release-execution)
- [7. Failure reduction and correction](#7-failure-reduction-and-correction)
- [8. Ordered implementation backlog](#8-ordered-implementation-backlog)
- [9. Metrics and release evidence](#9-metrics-and-release-evidence)
- [10. Documentation and maintenance](#10-documentation-and-maintenance)
- [11. Sources](#11-sources)
- [12. Local validation](#12-validation-record-for-this-document)

## 1. Audited baseline

### 1.1 Relationship to existing work

Continue the [November 9 verification program](2026-11-09-compiler-verification-program.md)
and [November 11 completion plan](2026-11-11-compiler-completion-and-verification.md).
The [November 10 defect campaign](2026-11-10-exhaustive-defect-campaign.md)
already provides a discovery driver. Preserve their historical records and
use this document to order and strengthen the remaining work.

| Existing phase | Audited state | Next increment here |
|---|---|---|
| P1: internal verification | Structural/typed HIR checks, negative tests, LLVM IR verification exist | Check pass boundaries, release-path activation, and semantic preservation (§4.4) |
| P2: directives/negative tests | Compile-fail, stdout, exit, and runner options exist | Requirement-linked positive/negative pairs and explicit outcomes (§4.1) |
| P3: differential testing | Optimization matrix, generated transitions, six interpreter/native examples exist | Strong trace oracle, HIR participation, independent inputs, then wasm (§3, §4.2) |
| P4: structured fuzzing | Five cargo-fuzz targets; typed generator and model exist | Connect typed generation to native execution; measure semantic reach (§4.2) |
| P5: snapshots | HIR rendering and stored snapshots exist | Keep useful structural assertions; add behavioral companions (§4.4) |
| P6: reduction | Proptest shrinking/replay and reducer scripts exist | Validate a real reducer invocation and preserve the original failure (§7) |
| P7: curated programs | Algorithm/directive and daily-use corpora exist | Stronger invariants, feature interactions, and real application histories (§4) |

Do not create replacement runners for these facilities. Share process handling,
artifact manifests, and outcome reporting while keeping semantic oracles
independent of the compiler components they check.

### 1.2 Inventory: presence is not executed coverage

At the audited revision:

| Files/surfaces | Count |
|---|---:|
| Top-level Rust integration test files, `tests/*.rs` | 125 |
| Compile-fail `.bv` fixtures | 13 |
| Directive `.bv` fixtures | 11 |
| Tier-1 `.bv` fixtures | 55 |
| Tier-1 sibling `.golden` files | 4 |
| HIR `.snap` files | 13 |
| cargo-fuzz targets | 5 |
| GitHub workflow files | 5 |
| `#[kani::proof]` markers under root `src/` / `briev-verify/src/` | 15 / 5 |

These are file/marker counts, not passing-test counts, independent properties,
or percentages of the language covered. A fixture can check its own result;
absence of a `.golden` does not by itself mean absence of an oracle. Conditional
compilation, early returns, external-tool detection, filters, and workflow
dependencies determine what actually runs.

### 1.3 Confirmed harness gaps

The following are source-inspection findings, not newly demonstrated compiler
miscompilations.

| Surface and evidence | What it checks today | Gap and consequence |
|---|---|---|
| `src/typed_program.rs:500–547`, `fuzz/fuzz_targets/welltyped.rs` | Generated well-typed programs typecheck and interpreter results agree with a separate expression model; 600 deterministic cases in the library test | This typed target does not compile and execute the program natively. Its bounded, exactly representable numeric subset does not cover overflow or difficult floating-point cases. |
| `tests/deterministic_transitions.rs:317–388` | Model/interpreter/native transitions, `-O0`/`-O3`, JSON replay and shrinking | Build explicitly sets `BRIEV_HIR_EMIT=0`. Each event restarts a process and supplies state through the environment: this is not a persistent multi-node reactor or schedule test. |
| Transition/memory generated smoke configuration | Both default to family 0, seed 1, four cases | Family inventories are 0–14 and 0–7 respectively; a default smoke pass does not establish that the other families ran. Handwritten tests provide additional coverage but do not replace family accounting. |
| `tests/interpreter_corpus_parity.rs` | Six curated programs compare interpreter values with native `-O0` process exit codes | Several results are reduced modulo 256. The sorting example sums elements, which is unchanged by failing to sort. Full values and ordered outputs are needed. |
| `tests/opt_level_parity.rs:121–139` | One emitted `.ll` linked at `-O0`, `-O2`, `-O3`; stdout and exit compared | A shared Briev lowering error survives all arms. Optimized arms also change LTO/fast-math settings, so this is a composite pipeline comparison, not isolated optimization-level coverage. |
| `tests/memory_campaign.rs:146–177,230–265` | ASan runtime/interceptor checks; optional Valgrind access checks | The file explains that linking ASan does not instrument emitted LLVM accesses. Profiles enable address sanitizer, despite broader UBSan wording. Missing Valgrind returns success. Some existing-fixture checks can accept timeout status when `require_zero_exit=false`. |
| `.github/workflows/memory.yml:25–54` | Nightly generated families 0–4, three default seeds, Valgrind requested | Families 5–7 are absent from this matrix; family 1 tolerates failure. Workflow green is not an all-family blocking memory gate. |
| `tests/fuzz_fault_injection.rs:69–209` | Random-source/interpreter state mutation with no-panic assertions | Many inputs return early before any fault. It does not verify the advertised rollback/recovery semantics or real I/O failures. |
| `src/fuzzing/concolic.rs:528–531` | A property invokes analysis on an empty program | Generated source is unused in that property; it provides no generated-path coverage. Other concrete concolic unit tests do exist. |
| `src/fuzzing/ast_generator.rs:497`, `tests/fuzz_frontend.rs:129` | Roundtrip-named properties only debug-format ASTs or parse without asserting a roundtrip | Genuine canonical-format tests exist in `tests/ast_parser_roundtrip.rs`; extend those rather than counting the weaker properties as equivalent. |
| `tests/unit_test_blocks.rs:1–4`, `src/test_runner.rs` | Language `test … { expect …; }` blocks run in the interpreter | These blocks do not establish native code-generation correctness. The same runner also has separate compile/run fixture support. |
| `scripts/reduce.sh:17–25`, `tests/reduction_scripts.rs` | Shell syntax and direct interestingness predicate tests | The generated driver expects `$1`; C-Vise's documented driver reads the candidate by filename without an argument. End-to-end invocation remains to be validated. |
| `scripts/verify.sh:27–32`, feature-gated proof modules, `briev-verify/Cargo.toml` | Kani commands and proof source are present | Many harnesses require both `kani` and `kani_full`; the default command enables neither. The extracted crate declares neither feature. A successful command is insufficient without discovered/executed harness identities. |
| `.github/workflows/closed-region-proof.yml:3–6` | A focused bounded ownership proof workflow | Triggered by `fix/closed-region-ownership` pushes/manual dispatch, not ordinary main PRs. |
| `scripts/gate.sh:27–36` | Quick nextest gate or cargo-test fallback | Fallback selects fewer integration suites. The same advertised gate can provide different coverage depending on tool availability. |

### 1.4 Actual execution evidence

Workflow configuration and old INDEX counts are not a current green baseline.
During this audit, GitHub reported:

- [CI run 43583827933][ci-failure], revision `bedfc4dc`: **failed** library
  tests, with 3,775 of 3,782 passing and seven `analysis::verify_bodies`
  failures. All-target tests, browser checks, IR determinism, and parity steps
  after that step were **skipped**. The workflow's package installation does
  not install Z3. Solver availability is a concrete investigation lead, not a
  proven explanation for all failures.
- [CI run 43584116325][ci-audited], audited revision `e9169750`: subsequently
  completed with the same seven named body-verification failures,
  **3,775/3,782 passed**. Downstream all-target, browser, determinism and
  parity steps were again **skipped**.
- [Verification nightly 43423322610][nightly-run], revision `16ecab67`:
  **failed** AST opt-out job; **passed** full optimization matrix job.
- [Memory nightly 43424765208][memory-run], revision `0c3a5e81`: all five
  family campaign steps **passed**. This evidence covers those configured
  families/seeds and their actual detector capabilities.

These are observations of specific runs, not claims about every upstream run.
Local validation of this documentation work is recorded in §12.

### 1.5 Investigation targets, not established false proofs

`briev-verify/src/smt.rs:66` ignores its `_timeout_ms` argument; solver verdicts
are recognized with substring searches, and the expression encoder has
permissive fallback cases. These deserve immediate robustness and soundness
tests. However, `briev-verify/src/verify.rs:306–362,491+` explicitly restricts
the body-prover encoding subset and preserves undecided outcomes. A claim of
false proof requires tracing every reachable caller, supplying a concrete
obligation, and replaying a counterexample. Do not infer it from a fallback
in isolation.

Existing `BUGS.md` records provide stronger starting evidence for campaigns:
nested enum underallocation despite correct printed output, scalar-load
provenance errors, growth-barrier leaks, Bool boxing, wasm pointer-width
failure, and the interpreter's derived-defn gap. Treat OPEN/FIXED labels as
dated records; reproduce on the chosen implementation base before classifying
current status.

## 2. Lessons from mature projects

| Project/research | Practice worth adopting | Briev adaptation and boundary |
|---|---|---|
| LLVM [R1] | Separate unit, small regression, and whole-program suites; targeted IR checks | Keep fast pass tests plus end-to-end observations. LLVM verifier success establishes well-formed IR, not preservation of Briev semantics. |
| Rust compiler [R2] | Compiletest distinguishes diagnostics, run tests, codegen/MIR checks, incremental and build-system tests | Extend Briev's directives with explicit capabilities/outcomes and revisions as needed; keep diagnostics, behavior, and build/ABI checks distinct. |
| GCC [R3] | Torture configurations, LTO, compatibility and target-aware suites | Run a controlled configuration matrix, including link/ABI boundaries. Compare only configurations with compatible language semantics. |
| Csmith [R4] | Generate complicated programs with a well-defined interpretation, then compare implementations | Generate typed, bounded Briev programs with explicit numeric/ownership assumptions. Rejection of a valid generated case is a failure, not a discard. Csmith itself generates C, not Briev. |
| YARPGen [R5] | Generator policies target optimization opportunities and varied expression/control structures | Measure whether generated programs reach the intended Briev analyses and LLVM transforms; vary policies instead of repeatedly sampling one template. Its C/C++ generator is methodological evidence, not a drop-in tool. |
| Equivalence Modulo Inputs, EMI [R6] | Mutate programs while preserving behavior for a particular observed input set | Use input-specific, precondition-checked mutations; run original and mutant on exactly that input set. Unexecuted code can still affect types, contracts, imports, and liveness. |
| Alive2 [R7] | Translation validation checks refinement of concrete LLVM transformations | Pilot small intraprocedural LLVM pairs. Record unsupported/time-limited cases; it neither proves Briev lowering nor validates an unrestricted LTO pipeline. |
| CompCert [R8] | State formal semantics and prove pass-preservation properties with explicit trust boundaries | Identify Briev's trusted proof core and prove tractable components. CompCert's claim is scoped; external preprocessing, assembly/linking and parts of its frontend are outside its verified core. |
| SQLite [R9] | Independent harnesses, allocation/I/O failure enumeration, boundary testing, mutation tests, disabled-optimization tests, delivered-build testing | Fail each operation in turn, verify recovery/resource invariants, and compare real release artifacts. Adopt the discipline; do not borrow SQLite's coverage figures as Briev evidence. |
| FoundationDB [R10] | Deterministic simulation with controllable time, external events, and faults; replayable seeds | Exercise persistent reactor histories through controllable host boundaries. Complement simulation with native/device tests: the model cannot expose everything omitted from it. |

The common pattern is **multiple complementary oracles plus reproducible
failure handling**, rather than one enormous suite or one verification tool.

## 3. Correctness contract and oracle design

### 3.1 Make the language rule the authority

Build a requirement ledger linked to `spec/SPEC.md`, backend capability
contracts, and the execution model. For each rule record:

`rule → supported targets → valid example → invalid/boundary example → oracle
→ test IDs → required CI lane → latest executed revision`.

First clarify rules that affect any oracle: integer overflow and shifts,
division/remainder, conversion boundaries, evaluation order, floating-point
modes, aliasing, ownership, rollback, FFI effects, equilibrium, and concurrency.
Where semantics are genuinely unresolved, make a language decision before
blessing one implementation's output. Do not turn current behavior into the
specification merely by snapshotting it.

Correctness includes:

- **Acceptance:** supported valid programs compile and run correctly;
  unnecessary rejection is a defect too.
- **Rejection:** invalid or unsupported programs receive the intended
  diagnostic and never silently lower to a default value.
- **Behavior:** values, effects, ordering, traps, and lifecycle match the
  language contract, including legal nondeterminism.
- **Safety/resources:** generated accesses and lifetimes are valid; resource
  cleanup and failure behavior meet the declared runtime contract.
- **Robustness:** malformed/unbounded inputs cannot silently corrupt artifacts
  or leave a test apparently successful after crashing or timing out.
- **Deployment:** packaged compiler, resources, target configuration and
  external toolchain implement the same supported behavior as development.

The interpreter remains the reference implementation. Keep testing it against
small, independently specified models and spec examples. When disagreement
reveals an interpreter defect, fix it against the language rule; never change
it merely to agree with emitted code. AST/HIR agreement is migration evidence,
not two fully independent implementations.

### 3.2 A shared observation protocol

Extend existing harnesses around a versioned observation format. A case should
carry source, input bytes, target/semantic mode, generator version, seed,
event schedule, permitted outcomes, and the expected observation contract.

An observation records:

1. Compilation outcome: accepted, expected diagnostic, unexpected diagnostic,
   compiler crash, timeout, resource limit, or infrastructure failure.
2. Execution outcome: normal exit, specified trap, signal/abort, timeout,
   resource limit, or expected bounded observation of a resident process.
3. Full typed result: integer width/signedness, float bit pattern, ordered
   collection contents, enum tag/payload, and meaningful state projections.
4. External effect trace: FFI calls and arguments/results, output bytes,
   callback/event order, and commit/rollback boundaries where observable.
5. Safety observations: detector identity/result, allocation/resource balance,
   and whether instrumentation was actually active.

Use an existing public export/FFI boundary where possible, with a small tested
host shim for full-width values. Capture interpreter output through an
appropriate sink or subprocess boundary. Avoid adding artificial branches
solely to keep results live: a real consumer must observe them. Legitimate
constant folding is correct; runtime inputs are needed when testing a runtime
path. Record evidence that the intended path was reached.

Normalize addresses only into stable logical identities while preserving
alias relationships. Do not compare padding/uninitialized bytes. Normalize
paths and temporary names in diagnostics without hiding wrong names, spans,
values, or error categories. A float JSON decimal is insufficient for signed
zero and NaNs; use explicit bit strings and semantic classification.

For small cases compare complete results. A checksum may be a secondary
large-case screen, never the sole oracle when full comparison is practical.
For sorting, assert ordered output **and** multiset preservation. For a parser,
compare the parsed structure; for a map, query all modeled keys and missing
keys. A permutation-insensitive sum cannot establish order.

### 3.3 Oracle portfolio

| Oracle | Finds | Does not establish |
|---|---|---|
| Hand-derived spec cases / independent bounded model | Shared interpreter/backend semantic bugs | General correctness beyond modeled constructs/domains |
| Interpreter vs emitted native/wasm | Lowering, runtime, representation disagreements | Correctness when both share a defect or when the interpreter lacks the feature |
| AST vs HIR | Migration regressions and fact/representation drift | Independence from shared frontend, analysis, runtime, or helpers |
| Briev optimization enabled/disabled, same input | Briev pass/consumer mistakes | Correctness of shared lowering or reference arm |
| Same LLVM module at multiple LLVM options | LLVM-sensitive UB/poison or optimization/link problems | Briev frontend correctness |
| Metamorphic relation | Inconsistency without an exact output oracle | Correctness if the relation is invalid or both outputs share the error |
| Memory/race detector | Defects visible to the instrumented detector | Value correctness or absence of unexercised/undetectable bugs |
| Bounded proof / translation validation | A stated property over a precisely modeled domain | Whole-language, whole-toolchain, or unbounded correctness |

Triangulate; do not majority-vote away a mismatch. Three backends can share
one faulty analysis. Use a hand-derived counterexample to resolve disagreements.
Validate the small models themselves with exhaustive tiny domains and known
examples; do not implement their arithmetic, casting or ownership rules by
calling the compiler helper being tested. Shared serialization is useful;
shared semantic implementation weakens independence.

### 3.4 Test the tests first

Every required lane needs a clean positive control and a planted detector
failure exercised through the same runner and flags:

- Wrong scalar output, reordered collection, missing/duplicated effect,
  expected diagnostic replaced by a crash, and a nonterminating child.
- A deliberate invalid read/write in **generated LLVM code**, double-free,
  and leak to establish exactly which memory detectors see which defect.
- Missing `clang`/solver/Valgrind, corrupt solver output, and failed linker.
- A failing language `expect`, unmatched test selection, and zero-case campaign.
- A known unsound proof/optimization mutant and a known valid proof control.
- A reducer that actually produces a smaller case preserving the same failure.

Isolate deliberate unsafe fixtures in subprocesses and test-only artifacts.
Require the expected detector/error category, not merely a nonzero status.
These controls directly test the audit's highest-risk false-green modes.

## 4. Risk-driven test campaigns

Priority **A** is first implementation work; **B** follows the reliable
oracle; **C** expands platform/assurance depth. A is not permission to ignore
a discovered B/C defect in a supported production surface.

### 4.1 Frontend, typing, and negative conformance — A/B

Extend `tests/diagnostics.rs`, compile-fail/directive fixtures, canonical
roundtrips, and the `frontend`/`typecheck` fuzz targets.

- Pair each important rejection with the nearest valid program: contract
  boundary, unsupported target construct, invalid cast, shadowing/scope,
  generic substitution, recursive definitions, and concurrency classification.
- Include Unicode and multibyte error spans, unterminated syntax, nested
  delimiters, long literals, cyclic/diamond imports, import aliases, and
  malformed configuration/plugin metadata. Assert bounded failure and useful
  diagnostics, not just absence of panic.
- Generate valid ASTs for canonical `parse → print → parse` tests; compare a
  documented normalization and require formatting idempotence. Span removal
  must not erase meaningful syntax or evaluation order.
- Exercise user-defined protocol/metadata variants and arbitrary user names.
  Renaming a type/function must not change compiler behavior through hidden
  vocabulary matching. Cover no-stdlib bootstrap and stdlib/import paths.
- Audit Rust/compiler-side recursion, input sizes, solver work, and
  specialization growth with explicit resource bounds. A timeout is a result
  to investigate, not an acceptable parse rejection.

**First concrete deliverable:** a rule-indexed positive/negative corpus for
casts, contract verdicts, and capability rejection, run through the real CLI.
Reachability counters must show the intended lexer/parser/type/proof stage.

### 4.2 Structured generation and differential execution — A/B

Grow `src/typed_program.rs` and `tests/deterministic_transitions.rs`; retain
their independent models, deterministic seeds, shrinkers, and replay files.

**Generator design:** maintain a typed environment, scope, initialization,
effect permissions, ownership/provenance, and a termination budget. Generate
legal programs constructively rather than discarding almost every random
string. Separate valid semantic, intentionally invalid, and robustness
campaigns; an invalid-program campaign still has an expected failure class.

Growth order:

1. Existing Int/Bool/Float subset into native default HIR execution, with
   full-width results and live runtime inputs.
2. Branch joins, short-circuit operands with effects, nested calls, returns,
   recursion with bounds, loops with zero/one/many iterations, and shadowing.
3. Collections, nested enums/tuples/structs, casts, generic specialization,
   user operators and receiver chains; use reference containers in the model.
4. Persistent multi-node programs and modeled external events (§4.7).

For every generated case count: generated, model-valid, parsed, typed, lowered,
verified, linked, executed, compared, and detector-checked. Also count construct
combinations, boundary values, proof outcomes, and targeted pass activation.
Report generator rejections separately from compiler false rejections. A
failure-preserving shrinker must retain typing, termination, the triggering
input, and the original failure class.

**Configuration axes:** explicitly distinguish (a) compiler debug/release,
(b) Briev HIR/AST and Briev optimization controls, (c) LLVM optimization/LTO,
and (d) target/runtime/numeric mode. Changing clang `-O0` alone does not disable
Briev constant folding, proof consumption, or other frontend analyses.

Start with a small pinned corpus across default HIR and supported AST opt-out,
native `-O0` and production optimization. Then add one axis at a time and
pairwise configuration coverage; run selected high-risk full cross-products.
Do not require every random program to run through every expensive backend.
Give transition families 0–14 and memory families 0–7 explicit execution
budgets and manifests instead of relying on default family 0.

Keep numeric semantics fixed within each comparison. In particular,
`spec/SPEC.md` defines precise-by-default, `exact`, and `approx` behavior;
unconditional fast-math must not silently become the conformance oracle.
Retain a separate harness-exact production/benchmark pipeline with cases whose
semantics admit those flags. An option mismatch needs classification before
it can be called a compiler defect.

### 4.3 Metamorphic and interaction testing — B

Use relations with explicit, mechanically checked preconditions:

| Transformation | Required condition and expected relation |
|---|---|
| Alpha-renaming / fresh helper names | Preserve binding, exported ABI, reflection-visible names and contracts; same behavior |
| Insert a pure unused binding | No effect, allocation/lifecycle obligation, trap, or name capture; same trace |
| Extract/inline a pure helper | Preserve argument evaluation count/order, contracts, captures and return semantics |
| Split/rejoin a loop | Preserve iteration order, bounds, state, and absence of cross-boundary effects/alias dependencies |
| Change collection representation | Same declared logical semantics; compare logical contents and alias-visible effects, not raw representation |
| Rename/reorder independent declarations | No ordering, initialization, reflection, or scheduling significance |
| EMI dead-region mutation | Region unexecuted on the recorded inputs; program remains well-typed and contracts/observable effects are preserved on those inputs |
| Add a proved redundant guard | Guard expression pure; independent proof and identical behavior; retain source guard and check the compiler consumer separately |

Do not use floating-point reassociation, `x/x == 1`, arbitrary integer
reassociation, duplicated calls, or reordered effects as universal identities.
Generate both a valid relation and a counterexample violating its precondition
to verify that the mutator's applicability check matters.

Build a feature-interaction ledger. High-yield starting combinations are:
`enum payload × collection × generic call × return ownership`,
`short-circuit × FFI × guard elimination`,
`slice alias × mutation × resize`, and
`cast × target width × callback ABI`. Pairwise coverage is a useful floor;
these known three/four-way risks need explicit cases.

### 4.4 HIR, analysis facts, optimization, and LLVM — A/B

Use the existing HIR verifier/negative tests, snapshots, AST/HIR parity,
`verify_emitted_ir`, and determinism gates as the foundation.

- Audit every lowering/pass boundary: names/IDs, scopes, expression types,
  control-flow edges, casts, effect summaries, ownership facts, and stale
  analysis invalidation. Inject a single invalid invariant and require the
  verifier to reject it. Confirm verifier activation in the builds being run.
- Snapshot small stable contracts, not register numbering across whole
  modules. Pair shape checks with runtime observations; valid-looking IR can
  still be wrong.
- For each proof-consuming optimization, test an independently valid proof,
  its nearest invalid case, and an unknown/unsupported case. Compare behavior
  with consumption enabled and disabled while retaining source guards. Check
  the emitted branch/access as a separate optimization assertion.
- Add test-only pass controls where a reliable A/B is otherwise impossible.
  They are debugging interfaces, not user strategy keywords. The conservative
  arm must preserve runtime checks rather than pretending unproved facts hold.
- Exercise LLVM `nsw`/`nuw`, `inbounds`, alignment, `noalias`, lifetime markers,
  range attributes, and fast-math promises at their boundary conditions.
  Incorrect promises introduce poison/UB and may only fail after optimization
  [R11]. `opt -passes=verify` does not validate their semantic justification.
- Pilot Alive2 on small supported LLVM transformation pairs. Preserve the
  source precondition in the validation problem; validate refinement in the
  right direction. Record bounds, unsupported operations, timeouts, and
  interprocedural exclusions. Whole LTO validation is not an initial gate.
- Check actual linked release binaries when conclusions depend on LTO or
  target lowering; raw `.ll` and `llc -O2` do not describe that pipeline.

Include repeated independent-process compilation with perturbed map insertion
order, unrelated working directories, parallel jobs, and cache state. Pin
`.ll` determinism where required; canonicalize only documented nonsemantic
metadata for other artifacts.

### 4.5 Proof-engine soundness and consumers — A

This is a high-leverage campaign because a false fact can remove the runtime
mechanism that would otherwise catch the error.

**Small-domain oracle.** For supported formulas over small bit widths and
bounded states, enumerate all assignments with an independent evaluator.
For each `Proven(P ⇒ Q)`, require every modeled state satisfying P to satisfy
Q. For `Refuted`, replay an actual counterexample satisfying P and violating
Q. `Unknown` is an honest outcome, not a proof. Separate a legitimately
vacuous implication from evidence that its precondition is reachable.

Cover signed/unsigned comparisons, casts/extensions, overflow, shifts,
division, path joins, old/new state, early `term`, substitution/shadowing,
loop invariant entry/preservation/exit, and recursive/call contracts. Test
unsupported syntax and unknown side effects at every solver entry point.
Small-width enumeration is evidence for that domain, not a proof for 64 bits;
add target-width boundary cases and targeted Kani/SMT properties.

**Solver boundary.** Use a controllable solver subprocess to test missing
binary, unsuccessful exit, signal, empty output, malformed output, extra
diagnostics containing `sat`/`unsat`, genuine `unknown`, timeout, and partial
writes. Parse complete protocol responses, enforce an actual deadline, and
preserve uncertainty through all consumers. Save the exact SMT query, solver
version, assumptions, and returned model. A second solver is useful for the
shared supported SMT subset, but agreement cannot validate a shared encoder.

**Proof provenance/cache.** Change imported bodies, contracts, type metadata,
target width, solver options/version, and optimization assumptions one at a
time. Cached success must not survive a change invalidating the obligation.
Compare cold/warm results and independently replay refutations.

**Kani.** Inventory named harnesses per crate, correct feature/cfg activation,
and require the expected nonzero set to execute. Preserve unwinding
assertions; report loop/heap bounds and assumptions [R12]. Apply first to
small ownership sets/graphs, bit-width/layout arithmetic, and verdict
combination. Test assumptions for satisfiability so overconstraint cannot
make a harness vacuously green. Keep Rust helper proofs paired with generated
program tests: proving an analysis helper does not prove its inputs or consumer.

**Acceptance:** no false `Proven` or unreplayable `Refuted` in the declared
enumerated domain; every failure/unknown response retains the required
runtime check or produces the specified diagnostic; positive proofs still
work, preventing an always-unknown implementation from passing the campaign.

### 4.6 Memory, representation, ownership, and resource lifetime — A/B

Strengthen `tests/memory_campaign.rs` and the existing reclamation, nested-enum,
descriptor-lifetime, and collection tests. Test values and memory safety
together: `BUGS.md` already records correct output with invalid enum accesses.

Start with this boundary cross-product:

| Dimension | Cases |
|---|---|
| Size | 0, 1, capacity−1, capacity, capacity+1, repeated growth; allocation-size overflow checked without huge allocations |
| Representation | Scalar, Bool/Char, nested tuple/struct/enum, list of aggregates, alternate protocol metadata/layouts |
| Origin/lifetime | Local, argument-owned, returned, global, field-published, captured, FFI-retained |
| Alias | Disjoint, identical handle, overlapping slice, nested field, alias across growth/call/return |
| Control | Early return, match arm, branch join, recursion, rollback/cancellation, repeated reactor events |
| Target | Native and supported 32-bit representation, alignment/packing/width boundaries |

Model logical contents and ownership with simple test-side objects. Observe
alloc/free/adopt events by logical identity when possible; assert resources
return to their baseline after an explicit lifecycle boundary. Long-lived
legitimate state is not a leak. RSS alone is not an allocation oracle because
allocator caching changes it.

Make Valgrind mandatory in a named Linux memory lane. For ASan, first establish
a working instrumentation path for actual emitted loads/stores and check the
planted bad-access controls at each supported toolchain. Linking a sanitizer
runtime is insufficient. UBSan checks normally introduced during C frontend
lowering cannot be assumed to exist in arbitrary emitted LLVM IR; test the C
runtime separately and explicitly validate Briev's arithmetic/alignment rules.

Check stack, global, and heap cases; no one detector covers every intra-object
or semantic bounds violation. For custom allocators, document detector
integration/redzones. Run the same cases uninstrumented in release because
instrumentation can change layout and timing.

### 4.7 Reactor, concurrency, and fault/recovery histories — B

Extend the generated transition work into **one persistent process** with a
modeled host boundary: controllable clock, external input, FFI results,
callbacks, and cancellation. Begin with deterministic single-threaded
histories before exploring scheduler interleavings.

Test:

- Preconditions becoming enabled/disabled, repeated events, diamond
  dependencies, old-state reads, publication, equilibrium, wake-up, and
  `beginprogram`/`endprogram`/resident lifecycle behavior.
- Co-enabled nodes: unclassified eligible pairs must be rejected under the
  explicit-concurrency rule; `async` and `sync<group>` pairs must preserve
  their specified constraints. Test both satisfiable and exclusive guards.
- Atomic/transaction boundaries, rollback after partial work, deferred cleanup,
  no stale callback after cancellation, and required exactly-once effects.
  Where delivery semantics allow retries, assert the specified alternative.
- Lost wakeups, barrier completion, queue saturation, reentrant callbacks, and
  cancellation at each meaningful transition.

Use exact traces for a chosen deterministic schedule. For legally
nondeterministic programs compare allowed observations, invariants, and
happens-before constraints; do not require all schedules to produce the same
total order. Check bounded progress under explicit fairness/input assumptions.
Intentional equilibrium is not a hang; a missed wake-up can be.

Borrow SQLite's failure enumeration: fail the first, second, … Nth allocation
or I/O operation, once and persistently, then test faults during recovery.
Use short reads/writes, EOF, EINTR-like retry cases, disk-full, permission
failure, delayed callbacks and disconnects only where the host contract
allows them. Assert state, effects, ownership and resource balance afterward.
Arbitrary interpreter-variable corruption is not a substitute for these tests.

Apply Loom/Shuttle [R16] only to Rust concurrency components that can use their
instrumented scheduling primitives. They do not explore generated Briev code
automatically. Native schedule replay and real runtime stress remain necessary.

### 4.8 Numeric semantics — A boundary seeds, B expansion

Use exact small-integer models and target-width boundaries: zero, ±1, min/max,
adjacent values, negative division/remainder, invalid divisors, shift counts
0/width−1/width and conversion limits. Encode the specified wrap/trap/reject
behavior explicitly; do not inherit Rust/C debug/release behavior by accident.

For floats, test raw bit patterns: both zeros, subnormals, normal boundaries,
infinities, NaNs, cancellation, rounding ties, f32/f64 conversions, FMA versus
separate operations, and reduction order. Distinguish:

- Precise/default and `exact`: enforce their declared order/contraction and
  representation rules; do not indiscriminately tolerate rounding drift.
- `approx`: use an operation/algorithm-specific justified error bound,
  including accumulation length and conditioning. The keyword alone does not
  define one universal epsilon or license arbitrary error.
- NaNs/infinities: explicitly classify before computing error. A condition
  such as `abs(got-ref) > epsilon` can silently accept NaN. Require finiteness
  when the reference is finite; treat NaN payload/sign guarantees according
  to the specification, not raw equality by default.

Use TestFloat/SoftFloat [R13] for supported primitive IEEE operations, and a
high-precision MPFR oracle [R20] with explicit target rounding for appropriate
numeric functions. Decimal formatting/parsing needs its own specification and
oracle; TestFloat does not cover decimal conversions. Hardware-dependent
rounding/flush modes belong in the recorded configuration.

### 4.9 ABI, targets, GPU, and other dialects — B/C

Extend existing `c_driver_*`, glue, wasm/browser, and backend capability tests.
Use separately compiled host callees/callers so both sides cannot share the
same Briev ABI mistake. Cover scalar widths/signedness, Bool, aggregate
layout/returns, alignment, enums, strings/encoding, zero-length values,
callbacks, retained ownership, reentrancy, error propagation, and multiple
translation units. Use C layout probes (`sizeof`, alignment, offsets) for
C-compatible layouts; add other hosts only for their advertised support.

For cross-target parity, restrict to the supported common semantic subset.
Represent target-dependent widths explicitly; identical source need not have
identical output when it legitimately queries target size. Validate unsupported
features fail before emission. A compile-only target pass is not a runtime pass.

The `.bv` wasm corpus gate is blocked in the dated bug record by pointer-width
handling; reproduce and repair that prerequisite before expanding the gate.
Keep `.rbv` browser tests distinct. Add wasm runtime execution (e.g. Wasmtime
for the supported WASI ABI) and browser-host transport tests with memory
growth, stale views, snapshot lifetimes, async events and JS boundary values.

For GPU changes, retain current device correctness/variant-diff gates and
same-geometry validation. Add awkward dimensions (0 where legal, 1, tile±1,
nonmultiples), masks/tails, strides/layouts, alias restrictions, address-width
limits, synchronization, and numerical edge inputs. Compare full small
outputs to an independent CPU model; use justified numeric error bounds for
large reductions. Run SPIR-V validation and suitable device memory/race tools
such as Compute Sanitizer [R21] on supported hardware. Validators and CPU
simulation do not establish device indexing/barrier correctness.
For CUDA-driver/PTX execution, select memcheck/racecheck/initcheck/synccheck
explicitly, configure a nonzero detector error exit code, and retain positive
controls. Racecheck's shared-memory scope is not a general global-memory race
proof; these tools do not automatically cover SPIR-V/Vulkan execution.

For advertised `.ebv`/`.sbv`/other dialect surfaces, add the corresponding
semantic oracle: independently calculated circuit/constraint examples,
small truth-table or transition enumeration, or simulation against an
independent HDL model. Reuse capability rejection for unsupported constructs.
Order hardware qualification by the current target support table, not by an
aspirational backend inventory.

### 4.10 Build, cache, packaging, and production robustness — B/C

Extend `tests/relocatability.rs` beyond its useful symlink-based fixture to
the actual release archive installed outside the repository. Exercise CLI,
stdlib, configuration, plugins, link inputs, and resource discovery from an
unrelated working directory; missing resources must fail clearly.

Test cold/warm compilation and invalidate one dependency at a time: source,
transitive import, stdlib, protocol metadata, configuration, target, compiler,
proof assumptions, external headers/library identity where relevant. Check
content changes despite preserved timestamps, interrupted/partial cache
writes, concurrent builds, and corrupt serialized artifacts. Never accept a
stale proof/result as a cache hit. Malformed serialized input needs parser-
quality validation and resource bounds.

Inject missing/failing tools, truncated output, disk-full/short writes,
unwritable directories, process cancellation, unusual paths, locale changes,
and repeated invocations in one process. The CLI must not leave a successful-
looking partial binary or reuse stale output as the new result. Document
whether OOM is a controlled failure or supported recovery; test that contract
without inventing a universal recovery promise.

Separate stress budgets for compiler time/memory/recursion and generated
program resources. Use deterministic counters/bounds where possible and
generous wall-clock deadlines for containment. Investigate reproducible growth
or hangs independently of throughput benchmarking.

### 4.11 Compile-time execution, expansion, and synthesis — B

Exercise the same supported pure computation as a constant/compile-time
evaluation and with runtime inputs, across zero/small/default optimization
budgets. Require identical specified results; budget exhaustion must preserve
semantics through a valid fallback or the documented diagnostic. Include
casts, recursion, overflow boundaries and imported generic definitions.

For macros/reflection and generated bodies, test binding hygiene, expansion
order, source locations, recursively generated code, target-dependent
metadata and downstream re-typechecking/verification. Every generated body
needs the same invariants as an authored body. Test that changing an imported
definition invalidates cached expansion and proof results.

For `:=` chains, synthesis and assembly alternatives, compare against independent
examples and the declared reference, including counterexamples just outside
the training/sample set. Trace candidate selection and fallback behavior on
the actual implementation. Finite random agreement is test evidence, not a
universal proof or an optimization license for an otherwise unproved fact.
Test unresolved obligations, failed candidates, target incompatibility, and
the interpreter's handling of the selected/reference body. Assembly/FFI
assumptions and clobbers belong in the trust boundary and ABI tests.

## 5. Tools and their assurance boundaries

| Tool | Recommended use | Adoption / limitation |
|---|---|---|
| Existing nextest, directive runner, proptest | Regression execution, typed/stateful generation, shrinking | First; fix reporting and capability selection before adding another runner |
| cargo-fuzz/libFuzzer | Coverage-guided malformed and structured frontend/interpreter testing | Already present; native compilation belongs in a budgeted subprocess campaign; replay corpus separately |
| `cargo-llvm-cov` [R14] | Rust compiler/prover region/line coverage, supported branch coverage | Early baseline by crate and stage; branch support is toolchain-dependent. It measures compiler execution, not generated-program semantics. |
| `cargo-mutants` [R15] | Targeted mutation testing of verdict, width, bounds and ownership decisions | Start with small modules; distinguish killed, survived, unviable, timeout, equivalent. Do not chase a misleading global score. |
| LLVM verifier, FileCheck-style assertions | IR legality and narrow intended-emission assertions | Already partly present; avoid whole-module brittle goldens and pair with execution |
| Valgrind, ASan/LSan, UBSan | Access, free/leak, and C/Rust runtime UB checks where supported | First memory phase; verify actual instrumentation with positive controls (§4.6) |
| Miri [R17] | Unsafe Rust helpers, host-side layouts/aliasing, small pure tests | Cannot execute emitted LLVM machine code or an arbitrary external-tool integration suite |
| Kani [R12] | Bounded Rust analysis/core properties | Existing harnesses need activation/accounting; proof is conditional on assumptions and completed bounds |
| Alive2 [R7] | LLVM intraprocedural refinement pilot | Pin compatible LLVM/Alive2 versions; unsupported/timeout is not success; no unrestricted interprocedural/LTO claim |
| Z3 / optional second SMT solver | Encoder differential checks, independent small formula tests | Solver disagreement is actionable; solver agreement does not prove encoding correctness |
| TestFloat/SoftFloat, MPFR [R13, R20] | Primitive floats, difficult numeric reference calculations | Apply the correct rounding/semantic mode; libraries are not whole-program oracles |
| Loom / Shuttle [R16] | Controlled interleavings in eligible Rust components | Requires adapted primitives and a scoped model; pair with runtime tests |
| ThreadSanitizer [R23] | Dynamic CPU/host race discovery where the toolchain supports it | Instrument participating memory accesses and synchronization; prove detection with a planted race. A runtime-only link is insufficient; unsupported atomics/FFI and unobserved schedules remain gaps. |
| C-Vise/C-Reduce, proptest shrinkers, `llvm-reduce` [R18, R22] | Source/IR reduction | Briev-aware shrinking first; C-oriented structural passes may not understand `.bv`; validate predicate protocol end-to-end |
| Wasmtime, device validators/sanitizers | Supported wasm execution and real-device validation | Pin engine/driver/ISA; device/tool availability is a reported capability, not a silent pass |

Consider OSS-Fuzz [R19] after the fuzz targets are self-contained, bounded,
replayable, and actively triaged. More machines do not repair a weak oracle.
Do not begin by attempting a whole-compiler formal proof: first isolate and
specify high-impact proof/analysis components and their trust boundaries.

## 6. CI and release execution

### 6.1 Execution contract

Each lane publishes a machine-readable manifest: commit, dirty-state flag,
compiler/tool versions, target, flags, semantic mode, selected cases,
attempted/executed/compared counts, verdicts, exclusions, seeds, elapsed time,
resource limits, and artifact locations.

Required capabilities are preflighted. A missing required tool or zero
executed cases fails that lane. Local optional capabilities may yield an
explicit **UNSUPPORTED/SKIPPED** result. Distinguish **PASS**, **FAIL**,
**TIMEOUT**, **INFRASTRUCTURE ERROR**, **UNKNOWN PROOF**, **EXPECTED FAILURE**,
and **NOT RUN**. Never fold these into one green count.

Split independent suites into independent jobs after their actual build
dependencies, so one library failure does not erase all browser/memory/ABI
evidence. A required aggregate still fails when any required lane fails or
does not run. Upload logs and minimized/raw artifacts even after failure.
Do not use `continue-on-error` as the required gate for an advertised guarantee.

Expected failures need an exact failure identity, owner, issue, affected
configuration, and review date. An unexpected pass requests removal of the
exception. Preserve the original failure on retry; do not turn flakiness into
success by rerunning until green. Nextest and fallback selection must have the
same declared coverage, or the reduced mode must be named explicitly.

### 6.2 Proposed lanes

Budgets below are initial targets to measure, not current runtime claims.
Reuse build artifacts and shard by capability; cap concurrency to fit memory.

| Lane | Contents | Cadence / initial budget |
|---|---|---|
| Core | Workspace library tests, HIR negatives/snapshots, diagnostics, curated runtime regressions, proof-verdict cases | Required PR, aim ≤10 minutes warm |
| Semantic smoke | Fixed high-yield generated seeds, default HIR, model/interpreter/native observations, numeric boundaries | Required PR, aim ≤5 minutes warm |
| Memory smoke | Required detector controls plus ownership boundary regressions | Required PR for relevant changes; daily unconditional baseline, aim ≤10 minutes |
| Proof core | Named Kani harnesses and small-domain soundness enumeration | Required for prover/analysis changes; daily unconditional baseline |
| Full verification | Full integration/corpus, generated family matrix, HIR/AST while supported, broader LLVM/LTO options | Nightly; shard within 30–90 minute job budgets |
| Exploration | Persistent fuzz corpus, rotating seeds, metamorphic cases, fault histories, memory families | Nightly/continuous, explicit CPU/case budgets |
| Platform/ABI | Required supported OS/targets, host compilers, wasm/browser, hardware gates | Change-triggered plus scheduled; all supported configurations before release |
| Coverage/mutation | Stage/branch reach and focused high-risk mutants | Weekly and after significant harness changes |
| Release qualification | Exact packaged release binaries/resources, cold environment, full required target matrix and replay corpus | Every release candidate |

Always retain an unconditional scheduled full run; path filters can miss
cross-cutting dependencies. Test both debug assertions and production release
compiler builds. Select the exact binary under test explicitly: a release
build step followed by integration tests using `CARGO_BIN_EXE_brievc` does not
by itself establish that those tests exercised the release binary.

Seed policy: fixed regression seeds on every PR, a rotating deterministic
seed set nightly, and recorded exploratory seeds. Keep historical
counterexamples even after coverage-guided corpus minimization. Add new cases
when a measured coverage/semantic gap justifies them, not to maximize test count.

### 6.3 Existing entry points for the first implementation phase

These commands exist at the audited revision; the campaigns retain the gaps
in §1 until their harnesses are strengthened. Run from the repository root,
record tool versions and capabilities first, and retain command output.

```bash
cargo test --lib
cargo test -p briev-verify --lib
bash scripts/gate.sh quick
OPT_MATRIX_FULL=1 cargo test --test opt_level_parity -- --nocapture
```

The separate `briev-verify` command matters: root-package `cargo test --lib`
does not run dependency-crate unit tests. For exploratory generated families,
select the actual test and explicit budget, for example:

```bash
BRIEV_SIM_FAMILY=13 BRIEV_SIM_SEED=7 BRIEV_SIM_CASES=32 \
  cargo test --test deterministic_transitions scripted_integer_transitions -- --nocapture
MEM_FAMILY=0 MEM_SEED=7 MEM_CASES=8 MEM_VALGRIND=1 \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
```

These are sample budgets, not claims that the larger campaigns have passed.
The transition command still forces AST today; setting HIR in the parent shell
does not override the harness's child-process setting. Likewise `MEM_VALGRIND=1`
does not repair its missing-tool success path. Use the existing
`BRIEV_SIM_REPLAY`/`MEM_REPLAY` variables with saved case files for reproduction.
Choose a documented supported Kani harness only after fixing activation and
checking discovery; a bare `cargo kani --lib` is not a verified proof gate.

## 7. Failure reduction and correction

One finding should be replayable from a retained bundle containing source,
imports/resources or their identities, original and minimized generator case,
inputs, event schedule, flags, target, toolchain, compiler revision, stdout/
stderr/status for every stage, emitted IR, and detector/solver reports.
Preserve the initial failure before starting reduction.

1. **Classify:** compiler crash, invalid IR, false rejection/acceptance,
   wrong value/effect, false proof, memory/race error, liveness failure,
   infrastructure failure, or specification ambiguity.
2. **Reproduce:** use the exact seed/input/configuration and process deadline;
   verify on the original revision and a clean chosen base. For intermittent
   failures retain the schedule and repeated-outcome counts.
3. **Minimize:** first shrink the typed case/event history, then source or IR.
   Keep necessary imports, contract assumptions and numeric mode. An
   interpreter mismatch predicate must not accept a later parser crash.
4. **Localize:** compare the independent model, interpreter, HIR/AST facts,
   emitted IR, optimization stages, and runtime. Bisect a known-good interval
   when useful; do not assume the most recent component is responsible.
5. **Correct generally:** add the smallest regression at the defective layer
   plus an end-to-end behavioral/safety test. Add nearby boundary and feature-
   interaction cases that exercise the generalized rule.
6. **Close:** record cause, affected scope, fix and test IDs in `BUGS.md`;
   replay the original and minimized case; update the coverage ledger and
   retain discovery metadata.

Reducer predicates need timeouts for every process, clean per-attempt
directories, explicit expected diagnostics/mismatch fingerprints, and valid
program checks for wrong-code bugs. Preserve the intended failure, not any
nonzero status. C-Vise invocation must be tested as C-Vise actually calls the
driver; syntax checking and calling a predicate manually do not suffice.
Use `llvm-reduce` only after confirming the IR reproduces the same defect; it
does not replace a source-level regression for Briev lowering.

## 8. Ordered implementation backlog

Owner labels below are responsibility areas to assign when opening work items.
Each row is a reviewable increment with an exit criterion, not a deadline.

| Order | Deliverable / owner | Existing surfaces to extend | Exit criterion |
|---|---|---|---|
| **A0** | Trustworthy baseline / test infrastructure | CI workflows, `scripts/gate.sh`, `scripts/verify.sh`, result manifests | Exact-revision required jobs green; solver/tool preflight; expected nonzero workspace test/proof counts; skipped downstream work visible; quick fallback coverage explicit |
| **A1** | Harness controls / test infrastructure | Process wrappers, memory campaign, directive runner, reducer tests | Wrong output, crash, timeout, missing tool, invalid generated access, failed proof and real reducer invocation all produce the intended failure classification |
| **A2** | Strong differential oracle / frontend + runtime | `typed_program`, transitions, interpreter corpus | Default HIR exercised; full values/ordered collections/effects compared; planted no-sort and truncated-value errors detected; input/replay bundle complete |
| **A3** | Proof boundary / verifier owner | `briev-verify`, `verify_bodies`, proof consumers, Kani modules | Small-domain soundness/replay suite; real solver deadline/protocol tests; supported proofs still discharge; unknown/errors cannot license unsafe optimization |
| **A4** | Ownership boundary matrix / analysis + runtime | Memory/reclamation/nested-enum suites | Required detector self-tests; growth/nesting/escape/alias cases check values and lifetimes on HIR; all 8 memory families have explicit results |
| **B1** | Structured breadth / frontend + testing | Typed generator, transition families, fuzz adapters | All 15 transition families accounted for; typed native execution and validity-preserving shrinking; useful stage/interaction coverage reported |
| **B2** | Optimization and numeric semantics / analysis + backend | HIR invariants, opt parity, proof-consumption ledger, compile-time/synthesis cases (§4.11) | Briev-vs-LLVM axes separated; precise/exact/approx and compile-time/runtime parity; pass A/B behavior with guards retained; boundary mutants detected |
| **B3** | Persistent reactor and fault schedules / runtime | Transition/fault campaigns, host boundaries | Replayable multi-event process; legal concurrency classification; bounded progress and recovery/resource invariants under enumerated faults |
| **B4** | Cross-target/ABI product gate / target owners | Wasm prerequisites, C/glue/browser suites, relocatability | Supported-subset runtime parity; independent host ABI probes; real relocated package passes; unsupported capabilities reject clearly |
| **C1** | Assurance depth / verification + target owners | Alive2 pilot, coverage/mutation, device suites | Explicit validation domains and unknowns; real-device addressing/synchronization gates; focused mutation gaps closed |
| **C2** | Continuous qualification / release owner | Full target matrix, soak/replay, release manifests | Exact candidate and package evidence satisfies §9; recurring discovery/triage ownership assigned |

**Recommended first implementation change:** A0 with a small shared outcome
manifest and capability preflight, followed by A1 controls for the existing
memory and differential runners. A2 then connects the existing typed model to
default-HIR native execution. This sequence makes subsequent discoveries
credible and immediately targets the production path.

After A0, A3 and A4 can proceed as independent owner workstreams sharing the
observation contract. Do not block direct regression fixes on completion of
the broader generator. Keep each increment shippable with its own controls,
documentation and measured execution evidence.

This is a correctness plan, not a performance change. If implementing a pass
or instrumentation default changes production performance, follow the existing
baseline-worktree and benchmark A/B protocol, including the complete before/
after suite and same-configuration correctness gate. Fast test execution is
not evidence of fast or correct generated code.

### First high-value test specifications

Before adding each case, locate its existing regression and extend that test
when it already expresses the same obligation. These are proposed behaviors,
not a second test-runner design or claims of newly reproduced bugs.

| Case | Required observation / defect it should expose |
|---|---|
| Full-width result 0 versus 256, then large positive/negative values | Distinguish values even when OS exit bytes collide; detect truncating comparison shims |
| Sort `[3, -1, 3, 0]` | Exact `[-1, 0, 3, 3]`, preserved multiplicities; a no-sort mutant must fail |
| False `&&` and true `\|\|` with an effectful right operand; inverse cases | Right-side call count 0 or 1 as specified; catch eager or duplicated evaluation |
| Receiver/index expression with a counted external effect | One evaluation per source occurrence through direct, chained and generic call paths |
| Nested enum elements returned through a generic collection helper | Tags/payloads preserved and no invalid accesses at multiple nesting depths and growth boundaries |
| Growing collection with scalar read versus escaping slice/aggregate | Correct scalar value; only actual escapes extend ownership; resources balance at the declared lifetime boundary |
| True implication, false implication with a known witness, unsupported goal | `Proven`, replayable refutation and honest uncertainty; detect always-proven and always-unknown mutants |
| Solver prints an error containing `unsat`, then exits unsuccessfully | No proof or check elimination; exact protocol/error classification |
| Native/wasm pointer-sized exported value and aggregate containing it | Target-correct width/layout across a separately compiled host boundary; no silent truncation |
| Finite numeric reference with a deliberately NaN result | Comparator rejects it before tolerance arithmetic; precise/approx policies remain distinct |
| Reactor event → equilibrium → wake-up → cancellation | State/effect trace matches the model; no lost wake-up, stale callback or leaked per-event resource |
| Warm proof/compile cache followed by a changed imported contract | Same result as a cold rebuild; stale acceptance/optimization must not survive |

For language-level `test` blocks, first reuse equivalent compile/run fixtures
to cover native behavior. A later compiled test-block engine should run the
same `expect` cases and report failures consistently with the interpreter;
counting interpreter-only blocks as backend coverage is never the substitute.

## 9. Metrics and release evidence

Measure both **reach** and **detection**:

- Required/selected/executed/compared cases per configuration; missing tools,
  unsupported cases, quarantines, unknown proofs and timeouts separately.
- Valid-generation rate, unique minimized failures, construct/interaction and
  boundary coverage, intended pass activation, and generator shrink success.
- Rust source coverage by crate/stage plus semantic rule coverage. A compiler
  branch being hit does not show its generated code was executed correctly.
- Targeted mutation outcomes: a survived wrong-verdict or width mutant is an
  oracle gap; an equivalent/performance-only mutant needs separate treatment.
- Kani harness identities, assumptions/bounds, completed checks, and vacuity
  controls; Alive2 attempted/supported/proved/refuted/unknown counts.
- Time to reproduce, reduce, diagnose, fix, and promote a regression; recurring
  causes and escaped bugs, rather than rewarding only a large discovery count.
- Cost per useful case, suite runtime/memory, flake frequency, and aging of
  known exceptions, so correctness gates remain usable.

Establish measured coverage baselines before setting ratchets. Require every
new semantic rule/optimization consumer to have positive, negative/boundary,
and end-to-end evidence. For high-risk finite decision logic, test individual
conditions and use mutation/MC/DC-style reasoning; do not promise arbitrary
whole-repository 100% coverage or delete defensive code to reach a number.

Before calling a release production-ready for a particular support tier:

1. Required lanes executed successfully for the exact candidate, target and
   shipped package; no missing capability masquerades as success.
2. Known wrong-code, false-proof and memory-safety findings affecting that
   advertised tier are resolved. Any narrower supported surface is explicitly
   specified and enforced by rejection, not by an undocumented skip.
3. Regression corpus, independent semantic oracle, high-risk ownership/numeric
   matrix, ABI checks and relevant device checks pass with retained artifacts.
4. Proof/translation-validation claims state their modeled domain and bounds;
   unsupported cases have execution-based evidence or remain outside the claim.
5. Discovery, reduction, and ownership of future failures continue after release.

This is an engineering assurance argument, not certification or a mathematical
claim that testing has covered all Briev programs.

## 10. Documentation and maintenance

For each implementation increment update documentation in the same commit:

| Change | Documentation/comment obligation |
|---|---|
| Outcome protocol, shared harness or CI contract | Add a current `docs/architecture/compiler-correctness.md`; update `docs/guides/deterministic-testing.md`, `docs/guides/memory-defect-campaign.md` and relevant workflow headers |
| New oracle/generator family | Describe modeled semantics, exclusions, termination/shrinking assumptions and family selection beside the generator; update the rule/interaction ledger |
| Proof verdict/encoder/consumer contract | Document the trust boundary in `docs/architecture/compiler-correctness.md` and update INDEX; review `docs/architecture/features/verification-chain.md` for test-vs-proof claims; reference `docs/plans/2026-10-26-proof-consumption-ledger.md` and document assumptions/invalidation rules |
| Ownership/layout/runtime boundary | Update `docs/architecture/llvm-memory-management.md`, `docs/architecture/backend-contracts.md` and the applicable type/layout contract; preserve historical failure rationale and add the new regression ID |
| Semantic clarification | Update `spec/SPEC.md`, relevant tutorial/guides, and positive/negative examples; update syntax tooling only if syntax changes |
| Tool or required target lane | Record version/capability policy, actual detector coverage, commands, deadlines and artifact/replay procedure |
| Confirmed defect/fix | Add the concrete reproducer, root cause, generalized repair and tests to `BUGS.md` |

Preserve existing rationale comments when refactoring; rewrite them with date,
reason, targeted pattern and retirement/undo condition rather than deleting
the explanation. Correct misleading test/workflow headers when their harness
is changed, especially sanitizer, roundtrip and fault-recovery claims.

Historical dated plans/results remain historical. Add new execution records
and maintain the current INDEX/architecture guides instead of retroactively
turning old claims into a new baseline. This strategy initially changes the
plan and its INDEX pointer; architecture/API documentation changes accompany
the implementation increments that introduce those contracts.

## 11. Sources

Primary project documentation and papers consulted for this strategy; accessed
2026-11-14. Tool limitations above are part of the recommendation.

- **R1 — LLVM:** [Testing Infrastructure Guide](https://www.llvm.org/docs/TestingGuide.html).
- **R2 — Rust:** [Compiletest and compiler test suites](https://rustc-dev-guide.rust-lang.org/tests/compiletest).
- **R3 — GCC:** [Testsuites](https://gcc.gnu.org/onlinedocs/gccint/Testsuites.html), including torture, LTO and binary compatibility testing.
- **R4 — Csmith:** Yang et al., [Finding and Understanding Bugs in C Compilers](https://users.cs.utah.edu/~regehr/papers/pldi11-preprint.pdf), PLDI 2011.
- **R5 — YARPGen:** Livinskii et al., [Random Testing for C and C++ Compilers with YARPGen](https://users.cs.utah.edu/~regehr/yarpgen-oopsla20.pdf), OOPSLA 2020.
- **R6 — EMI:** Le et al., [Compiler Validation via Equivalence Modulo Inputs](https://web.cs.ucdavis.edu/~su/publications/emi.pdf), PLDI 2014.
- **R7 — Alive2:** [Project and documented limitations](https://github.com/AliveToolkit/alive2); [bounded translation validation paper](https://web.ist.utl.pt/nuno.lopes/pubs/alive2-pldi21.pdf), PLDI 2021.
- **R8 — CompCert:** [Trustworthy compilation and the semantic-preservation guarantee](https://compcert.org/man/manual001.html).
- **R9 — SQLite:** [How SQLite Is Tested](https://sqlite.org/testing.html), especially anomaly, mutation, coverage and delivered-build testing.
- **R10 — FoundationDB:** [Simulation and Testing](https://apple.github.io/foundationdb/testing.html).
- **R11 — LLVM semantics:** [Undefined Behavior](https://llvm.org/docs/UndefinedBehavior.html).
- **R12 — Kani:** [Loops, unwinding, and bounds](https://model-checking.github.io/kani/tutorial-loop-unwinding.html); [limitations](https://model-checking.github.io/kani/limitations.html).
- **R13 — Floating point:** [Berkeley TestFloat](https://www.jhauser.us/arithmetic/TestFloat.html) and [SoftFloat](https://www.jhauser.us/arithmetic/SoftFloat.html).
- **R14 — Coverage:** [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov).
- **R15 — Mutation testing:** [cargo-mutants](https://mutants.rs/).
- **R16 — Rust concurrency:** [Loom](https://github.com/tokio-rs/loom) and [Shuttle](https://github.com/awslabs/shuttle).
- **R17 — Rust UB checking:** [Miri](https://github.com/rust-lang/miri).
- **R18 — Source reduction:** [C-Vise](https://github.com/marxin/cvise), including its interestingness-test invocation convention.
- **R19 — Continuous fuzzing:** [OSS-Fuzz](https://google.github.io/oss-fuzz/).
- **R20 — High-precision numeric oracle:** [GNU MPFR](https://www.mpfr.org/).
- **R21 — GPU dynamic analysis:** [NVIDIA Compute Sanitizer](https://docs.nvidia.com/compute-sanitizer/ComputeSanitizer/index.html).
- **R22 — LLVM reduction:** [llvm-reduce](https://llvm.org/docs/CommandGuide/llvm-reduce.html).
- **R23 — CPU races:** [ThreadSanitizer](https://clang.llvm.org/docs/ThreadSanitizer.html).

[ci-failure]: https://github.com/Randozart/briev-lang/actions/runs/43583827933
[ci-audited]: https://github.com/Randozart/briev-lang/actions/runs/43584116325
[nightly-run]: https://github.com/Randozart/briev-lang/actions/runs/43423322610
[memory-run]: https://github.com/Randozart/briev-lang/actions/runs/43424765208

## 12. Validation record for this document

Local Linux x86-64 environment: Rust/Cargo 1.99.0, clang/LLVM 21.1.8,
Z3 4.13.3 installed, nextest 0.9.151. Commands use
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1` to bound
build memory/disk use; this remains an unoptimized compiler test build.

- `cargo test --lib`: **3,782 passed, 0 failed**, 264.80 seconds test time.
  This is the root library suite, not every workspace/integration/device test.
  Log: `/tmp/opencode/briev-correctness-plan-lib-test.log`.
- `bash scripts/gate.sh quick`: **passed**. Library selection: 3,781 passed,
  one conformance-sweep test deliberately filtered (covered by the full
  library command above), 96.867 seconds. Focused integration selection:
  **42 passed across 10 binaries**, 103.289 seconds; 118 other binaries were
  outside this quick selection. These are test-execution times, not cold-build
  timings. Log: `/tmp/opencode/briev-correctness-plan-quick-gate.log`.
- `cargo build`: **passed**, with the same five `briev-verify` warnings as the
  library build. Log: `/tmp/opencode/briev-correctness-plan-build.log`.
- Source build emitted five existing `briev-verify` warnings: two unused
  imports, two undeclared Kani feature conditions, and an unreachable pattern.
  No Rust source was changed by this documentation work.
  The quick gate's broader test compilation also reported existing warnings
  in untouched integration/binary test files.
- `praetor validate --warn --target docs/plans`: passed. The changed files are
  Markdown; this is not source-code complexity or semantic assurance.
- Local Markdown links, anchors, reference labels, table structure, shell
  example syntax, and `git diff --check`: **passed**.

The local library pass and audited-revision CI failure are both retained.
They establish an environment/invocation discrepancy to investigate, not a
demonstrated solver root cause. The proposed campaigns in this plan have not
been executed as part of writing it.
