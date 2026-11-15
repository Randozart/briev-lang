# Compiler assurance and Lean export

Owner: the compiler-assurance workstream in
[the implementation plan](../plans/2026-11-14-201223-compiler-assurance-and-lean-export.md).

The first objective is assurance about the existing Rust compiler and discovery
of missed bugs. A restricted optional Briev-to-Lean backend now exports selected program
semantics and proof obligations. No language syntax change is part of this work.

## First component: solver response classification

The [solver response contract](solver-response.md) documents the production fix,
its trust boundary and real-Z3 regressions. This is the first implementation-linked
assurance component; it is not a proof of query generation or compiler soundness.

## Bounded implementation proof

The [production classifier proof](solver-response.md) now checks exact token
recognition over all byte strings of length 0–6. Kani verifies the actual Rust
kernel, with explicit coverage for all result classes and both length endpoints.
The restored-substring mutant fails on concrete `wunsat` and `ssat` inputs.
This bounded implementation result is separate from the checked Lean model.

The older `sat_model_cannot_authorize_unsat` harness still covers the larger
whole-parser obligation (a leading SAT response plus eight arbitrary ASCII bytes).
Its initial run timed out. Neither the new kernel proof nor shared examples
prove that normalization refines the Lean model. Rust-to-Lean refinement and
arbitrary-length coverage remain explicit work items.

## Lean model and current evidence limits

`verification/lean/SolverResponse.lean` is an initial model, with Lean pinned by
the adjacent `lean-toolchain`. It states exact-status recognition theorems and
includes SAT-model/error/unknown controls. From the repository root:

```bash
bash verification/lean/check.sh /path/outside/repository/lean-check
```

The gate uses Lean on PATH or the standard user Elan installation; `LEAN_BIN`
can name an explicit executable. It checks version 4.24.0, compiles the model
into the external directory and requires `negative/FalseSat.lean` to exit 1
because `decide` refutes its false SAT-to-UNSAT theorem. A missing import, tool
error or accepted false theorem cannot satisfy that control. All eleven named
theorems across both models must report their axiom dependencies; the gate accepts no
reported dependencies beyond Lean's foundational `propext`, and writes
`axioms.json` alongside the full log. Missing or duplicate reports fail closed.

**Current Lean status: checked under Lean 4.24.0.** The six named theorems prove
exact SAT/UNSAT recognition, first-nonempty selection, leading SAT/UNSAT
preservation for arbitrary tails, and rejection of nonempty non-UNSAT heads as
proofs. All six currently depend only on `propext`; the false-theorem control
was rejected for the intended logical reason. Unlike the bounded Rust proof,
these model properties quantify over arbitrary strings and normalized line lists.

The model starts after whitespace normalization. An extracted implementation or
Rust refinement theorem is still needed to connect it to the real adapter.
The successful bounded Kani kernel proof above is separate evidence about the
actual production function; it does not close that refinement gap.

## Export acceptance boundary

An export must name its supported source subset, machine arithmetic,
errors, effects and termination assumptions. Unsupported constructs must produce
coverage diagnostics. Contracts must become obligations, not assumed facts.
Emitting Lean that typechecks is not equivalent to proving a source contract.

Guarantees about native output additionally need a connection to lowering and
optimization, through preservation proofs or checked translation evidence.
Exporting after optimization alone can reproduce an optimizer bug in both the
program and its purported specification. Source revision and artifact hashes,
theorems, assumptions, trusted components, skipped obligations and coverage must
accompany any eventual guarantee manifest. The ordered milestones in the plan
separate these obligations rather than promising arbitrary-program proofs.

## Integer expressions and assignment verification conditions

`verification/lean/IntegerVC.lean` defines a restricted expression AST with
64-bit words, variables, literals and wrapping add/sub/mul. Assertions support
Boolean literals, equality, signed less-than, negation, conjunction, disjunction
and implication. The original three integer theorems establish expression
substitution, assertion substitution and single-assignment VC soundness.

Finite assignment lists now have explicit `execute` semantics and a backward
`wp` transformation. `sequence_wp_correct` proves that a weakest precondition
holds exactly when the postcondition holds after execution, for arbitrary
states, code lists and assertions. `sequence_vc_sound` transfers a checked VC
to the final state. These are proofs about the actual Lean definitions, with
no fixed bound on program length.

The gate checks eleven named theorems across both models and rejects three
false-theorem controls. `negative/UnboundedAddition.lean` specifically rejects
positive monotonicity at signed MAX; `negative/StaleAssignment.lean` rejects
retaining an earlier value after a later assignment. No division, calls, memory, loops, effects
or source/native whole-program termination claim is included. No exporter or Rust-to-Lean refinement theorem
has been implemented. The [production arithmetic contract](verifier-arithmetic.md)
records Kani's separate implementation obligations and the newly discovered
native no-wrap mismatch. These checks are foundations for export, not evidence
that arbitrary Briev programs or native outputs are proved.

## Restricted export now available

The optional library API `backend::lean::export_source` uses the real parser and
typechecker and translates one named definition into a Lean word program and a
contract obligation. Run its executable example from the repository root:

```bash
cargo run --example lean_export -- path/to/source.bv function_name /tmp/new-export
python3 verification/lean/check_export.py /tmp/new-export
```

The output directory must be new. Generation writes `Program.lean`, the reviewed
integer model, the pinned toolchain, original and expanded typed source, and
`manifest.json`. It reports **generated-unchecked**. Only a successful checker
run writes `checked.json`, naming all three obligations, Lean version, axiom dependencies
and hashes of the inputs, logs and compiled artifacts. A failed recheck removes
the previous success marker. Modified inputs, missing theorem reports and
unapproved axioms fail closed. The gate accepts only `propext`, matching the
existing model gate; it uses simplification and `bv_omega`, not native reduction.
Lean's [proof-validation reference](https://lean-lang.org/doc/reference/latest/ValidatingProofs/)
explains why native evaluation adds a trust dependency; this choice was also
checked against the installed Lean 4.24.0 tactic implementation.

The initial subset is monomorphic, pure signed 64-bit parameters/results,
straight-line word locals and assignments to those locals, and a final return.
Parameters are immutable in the export subset; binding shadowing is rejected so
contract parameter references retain their input meaning. Word expressions
support literals, variables, negation, add/sub/mul; contracts support equality,
signed comparisons and propositional connectives. Types resolve through metadata
and protocol categories, with standard operators required. Calls, loops, effects,
globals, division, narrower/unsigned/floating arithmetic, generic declarations,
authority contracts and modifiers are rejected. Only the selected definition is
covered; this export makes no claim about other functions or whole-program
termination. Some valid obligations exceed the current proof tactics and fail
without a certificate.

Version 2 exports retain the direct `program`, `pre` and `post` definitions and
independently serialize expressions and mutable locals into `code`, `model_pre`
and `model_post`. Direct emission gives each assignment a fresh Lean name;
model serialization reuses local slots and executes the assignment list. The
checker requires exactly these three named proof reports:

- `vc_checked`: for every model state, the precondition implies `wp code model_post`.
- `translation_checked`: the direct function equals model execution, its
  precondition agrees with the model, and its postcondition agrees for **every
  possible result**, rather than only the result this program happens to return.
- `contract_checked`: the requested contract follows from `sequence_vc_sound`
  and those checked correspondences. It transports the modeled final state back
  to the immutable input parameters and returned word.

Both the export and check records use schema `v2`; regenerate old `v1` bundles.
The axiom record now maps each proof name to its dependencies. Every proof must
use only the approved `propext` axiom (or no axioms). Program correspondence uses
definitional equality; contract normalization is restricted to named logical,
order and slot-index rules, avoiding a broader simplifier trust dependency.

**This checks correspondence between two emitted Lean views.** The Briev parser,
typechecker and AST-to-model serialization remain trusted implementation steps;
two emitters can share the same systematic mapping mistake. Source/native and
whole-exporter refinement remain unproved and are recorded in the manifests.
The backend currently consumes a typechecked AST; a typed-HIR/native bridge is
still a separate obligation. There is no runtime optimization consumer for these
certificates yet; their consumers are the Lean correspondence and contract gates.

Hashes bind inputs, proof logs, models, checker and compiled artifacts. Source
hashes identify the direct emitter, independent model serializer and type adapter;
expanded source records resolved declarations, including imports. Hashes identify
particular inputs and implementations, rather than proving translation correctness.

Run the end-to-end gate after building the example:

```bash
cargo build --example lean_export
python3 verification/lean/export_gate.py /path/to/debug/examples/lean_export /tmp/new-export-checks
```

It checks cancellation, signed relations, copied/reassigned locals, negation,
constants, logical contracts and multiplication. It rejects false overflow and
identity contracts, hash drift, each missing proof report, legacy-schema bundles
and user-axiom dependencies.
Further controls independently corrupt the direct program, a model assignment
slot, the direct postcondition and the precondition. The postcondition control
would evade a correspondence check limited to the program's actual result; the
precondition control catches a vacuous contract introduced by the emitter.
Unsupported source forms remain covered by unit tests. These controls strengthen
per-artifact evidence while leaving the AST-serialization refinement gap explicit.

## Finite execution comparison

`verification/lean/execution_gate.py` compares the same selected functions through
the exporter, reference interpreter and native compiler. Build the sampling
helper alongside the compiler and exporter:

```bash
cargo build --bin brievc --example lean_export --example lean_interpret
python3 verification/lean/execution_gate.py /path/to/debug/brievc /path/to/debug/examples/lean_export /path/to/debug/examples/lean_interpret /tmp/new-execution-checks
```

The gate uses eight functions covering add/sub/mul, negation, copied locals,
reassignment, a polynomial and an ordinary scalar alias. Nine signed-word
boundary values form all 81 `(x, y)` pairs, with a deterministic third argument;
this is not the full three-argument Cartesian product. An independent Python
integer oracle reduces each result modulo `2^64` and interprets it as signed.
For this add/sub/mul/neg subset, reducing intermediate values or only the final
value yields the same word. The gate checks each exported contract, then asks
Lean's kernel to check each concrete result with `decide` and requires an
axiom-free report for every sample. `lean_interpret` runs the actual typechecked
definition through `Interpreter::call_function`; it implements no arithmetic
oracle of its own.

The native wrapper reads runtime environment inputs, casts them to the selected
parameter type, prints the result through an explicit word-to-`Int` cast, and is
compiled through both AST and typed HIR.
Each emitted IR program is linked with the installed Clang at `-O0` and `-O3`.
Every result must match the oracle. Deliberately false Lean results must be
logically refuted, and a separately compiled subtracting program must fail
against the unchanged addition oracle at execution comparison. Tool/import or
compilation failures cannot satisfy those controls. Missing tools are unavailable
outcomes, not successful comparisons.

The fresh output directory retains sources, inputs, expected values, compiled
exports, Lean reports, native IR/binaries and execution logs. `results.json`
records the finite coverage, tool/source identities and explicit unproved
refinement fields; `artifact-hashes.json` binds the retained files. The gate
rejects a tool/source identity change during its run. It covers the selected
host compiler/Clang and inputs only; the existing v2 certificate remains a Lean
model certificate. These differential checks can discover shared serializer or
lowering mistakes but do not prove AST serialization, all-input native
preservation, arbitrary programs or cross-target correctness.
