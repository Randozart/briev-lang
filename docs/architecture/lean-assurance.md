# Compiler assurance and Lean export

Owner: the compiler-assurance workstream in
[the implementation plan](../plans/2026-11-14-201223-compiler-assurance-and-lean-export.md).

The first objective is assurance about the existing Rust compiler and discovery
of missed bugs. A later optional Briev-to-Lean backend would export program
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
error or accepted false theorem cannot satisfy that control. All nine named
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

A future export must name the supported typed-HIR subset, machine arithmetic,
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
equality, signed less-than, conjunction and implication. The three named
theorems establish expression substitution, assertion substitution and soundness
of an assignment VC for arbitrary states and expressions in this model.
A VC is a universally quantified precondition implying the substituted
postcondition; the checked theorem transfers that obligation to the updated state.

The gate now checks nine named theorems across both models and rejects two
false-theorem controls. `negative/UnboundedAddition.lean` specifically rejects
positive monotonicity at signed MAX. No division, calls, memory, loops, effects
or termination claim is included. No exporter or Rust-to-Lean refinement theorem
has been implemented. The [production arithmetic contract](verifier-arithmetic.md)
records Kani's separate implementation obligations and the newly discovered
native no-wrap mismatch. These checks are foundations for export, not evidence
that arbitrary Briev programs or native outputs are proved.
