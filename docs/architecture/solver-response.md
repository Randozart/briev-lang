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

Normalization and recognition are separate production phases. The adapter keeps
Rust's existing line splitting and Unicode whitespace trimming; the byte kernel
`classify_status_line` recognizes exact status tokens. Its Kani harness
`normalized_status_is_exact` checks all byte strings of length 0 through 6,
including non-UTF-8 bytes: UNSAT iff the token is `unsat`, SAT iff it is `sat`,
and unknown otherwise. SAT, UNSAT, unknown and both length endpoints have
explicit cover properties, all of which must be satisfied.

```bash
cargo kani --manifest-path briev-verify/Cargo.toml --harness normalized_status_is_exact
```

The first run passed with zero failed checks and all five cover properties
satisfied. A private copy restoring substring recognition failed the same
assertions; Kani produced `wunsat` and `ssat` counterexamples, retained as ordinary
parser regressions. Full command logs and source identities are external evidence.

This is a bounded proof of the actual classifier. It does not establish Unicode
normalization, arbitrary-length parser behavior, query encoding, Z3 correctness,
or native compiler correctness. The earlier whole-parser harness for arbitrary
eight-byte ASCII suffixes after a leading SAT line remains present and
undischarged: its first run timed out. The smaller proof does not replace it.


## Query encoding boundary

Production terms carry Boolean or Word64 sorts. Equality requires matching
operand sorts; connectives require Boolean terms, while arithmetic and signed
comparisons require Word64 operands. Unsupported expressions/operators and
ill-sorted formulas have no encoding. Public String query builders emit a
diagnostic-only script with no `check-sat`; solver consumers consequently return
Unknown. Bitwise operators remain unsupported. The legacy expression String API
uses an undeclared error sentinel on failure; internal protocol consumers use
`try_encode_word_for_proof` and propagate absence explicitly.

`symbols::encode` maps ordinary UTF-8 names to `v_` plus two lowercase hexadecimal
digits per byte. `#Self` and `#self` intentionally share `b_self`, disjoint from
ordinary names. Declarations deduplicate canonical aliases. Contract queries
reject free identifiers outside their declared parameters/self; VC queries
declare their collected free names. Protocol query templates obtain binder
names from this same encoder rather than assuming a literal `x` or `y`.

`symbol_byte_codec_is_injective` proves the actual two-digit byte primitive is
injective and stays within the hexadecimal alphabet for arbitrary u8 inputs.
It does not prove unbounded String iteration, declaration collection, caller
binding or AST-to-SMT semantics. Real-Z3 tests in `smt_encoding` and the ignored
`namespaced_protocol_queries_preserve_real_solver_results` compiler unit test
must be executed explicitly. The prover command `-- --include-ignored` includes
the solver-dependent package controls. Copied sanitizer examples formerly in
`kani/prove_smt_cache.rs` are retired; the remaining two scalar cache-cap models
do not prove the production cache/mutex. Division is total in SMT and still
requires the separate typed/domain admission rules for source proofs.


The structural body prover separately infers ranks for directly tracked field
postconditions in its mathematical word API. This does not make fields supported
SMT terms. Unsupported or ill-sorted contracts remain advisory/unproven; an
unsupported body or declaration retains its separate skipped outcome. The same
binary rank table checks structural postconditions and serialized SMT terms.
