# Solver response contract

Both public solver entry points in `briev-verify/src/smt.rs` use
`solver_response::parse_solver_output`. For the single `check-sat` queries used
by the compiler, only an exact first nonempty status line selects SAT or UNSAT.
Earlier errors and unknown/malformed responses remain unknown. Later model text
cannot change the verdict. UNSAT followed by a model-unavailable error remains
UNSAT: the check completed before the unsuccessful model request.

This trusts the solver's answer. It does not check a proof certificate, prove
that the query models the source program, or supply counterexample values;
the existing empty SAT model payload is preserved.

The former substring classifier accepted a SAT model containing a variable
named `unsat` as UNSAT. It also accepted error text containing that word. The
real body-prover regression confirms a false `Proven` verdict for `square(unsat)`
with body `unsat * unsat` and postcondition `term == 1` (false at input zero).
The fix leaves parameter-dependent SAT counterexamples advisory under the
existing API, while preventing that false proof.

Run from the repository root with an authorized `CARGO_TARGET_DIR`. Generate
the local untracked root lockfile with `cargo generate-lockfile --offline` if
absent:

```bash
cargo test --locked --offline -p briev-verify
cargo test --locked --offline -p briev-verify --test solver_boundary -- --ignored
cargo test --locked --offline --release -p briev-verify
```

The two ignored integration tests require Z3 and must be run explicitly. A
normal test run that skips them is not a passing solver-boundary gate. The
resolver checks PATH and `~/.local/share/briev-compiler/bin/z3`.

For an external Cargo target, configure `BRIEV_HOME` as this checkout and
`BRIEV_STDLIB_PATH` as its `lib` directory when running compiler integration
tests. This preserves stdlib discovery for temporary source fixtures.

The production parser has a bounded Kani harness for arbitrary eight-byte ASCII
suffixes after a leading SAT line. Its initial run timed out without a verdict;
the harness is available for further verification, not an accepted proof.
