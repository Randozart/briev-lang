# Verifier integer arithmetic

The body prover's symbolic evaluator and `eval_symbolic_expr` share the literal
fold in `briev-verify/src/symbolic.rs`. Addition, subtraction and multiplication
use wrapping i64 arithmetic, matching `src/interpreter/eval.rs` and the 64-bit
`bvadd`/`bvsub`/`bvmul` encoding in `briev-verify/src/smt.rs`. This contract covers
the verifier's current 64-bit integer model, not arbitrary target word widths.

Literal division folds only when `checked_div` succeeds. Zero divisors and
`MIN / -1` remain symbolic rather than panicking or inventing a native result.
This is a local folding rule: SMT bitvector division is total, so retaining a
symbolic division does **not** establish trap safety for the whole body prover.
Native division and remainder require explicit domain obligations before an
exported guarantee can include them.

`briev-verify/tests/arithmetic_boundary.rs` exercises both public evaluators
at wrapping boundaries and checks signed truncating division. The explicit-Z3
`wrapping_addition_cannot_prove_unbounded_monotonicity` regression rejects the
false universal contract `x + 1 > x`, whose counterexample is signed MAX.

Kani harnesses call the production fold with arbitrary i64 inputs. Add/sub and
multiply compare with exact i128 arithmetic truncated to 64 bits; the exceptional
division harness checks the two excluded cases. A harness's presence does not
mean it passed: execution results and timeouts are retained in external evidence.

## Native wrapping repair and remaining correspondence obligations

The original general expression emitter used `add nuw nsw`, `sub nsw` and
`mul nsw`; shared integer templates also promised `nsw` for add/sub/mul/neg.
LLVM specifies poison on overflow with these flags
([LLVM Language Reference](https://llvm.org/docs/LangRef.html#add-instruction),
consulted 2026-11-14). At signed MAX, O3 could replace `(x + 1) > x` by true,
contradicting wrapping evaluation. A full Briev executable now reproduces this
failure on the old compiler, rather than only an isolated IR experiment.

General expressions and shared templates now emit ordinary integer add/sub/mul
and negation via ordinary subtraction. The legacy bindings table agrees.
Width selection is unchanged. Existing loop-engine counter flags are a separate
path: this repair neither adds such flags nor proves all counter domains safe.
No-wrap flags require an operation-specific non-overflow premise; they are not
an unconditional property of integer operators.

`tests/integer_wrapping.rs` builds the maintained runtime-input fixture, links the
whole emitted module and runtime at O0/O3, and checks seven input pairs under
both AST and HIR emission. It checks four arithmetic results and three signed
comparisons against Rust wrapping reference operations. The quick gate includes
this regression. Backend unit tests cover shared templates at 8/16/32/64 bits
and ordinary function-expression emission at 64 bits. Those width checks inspect IR;
the native executable regression is x86-64, not a cross-target execution proof.

Division/remainder domains, specialized loop-counter arithmetic, target-width
semantics and Rust/Lean/native refinement remain separate obligations. The
passing native regression repairs the demonstrated failure; it is not a proof
of every lowering or optimization pass.

## Floating integration regressions

The incoming Float zero-divisor guard exposed a physical-type boundary: an
integer-spelled literal can be accepted in a Float expression while its emitted
register still needs numeric promotion. The guard now runs after the shared
promotion adapter and compares the divisor using its actual Float LLVM type.
The language's explicit-cast rule for Int variables is retained. Float64 binary
division/printing regressions cover the double guard/declaration ABI and stdout
flush ordering with raw exit. The CLI integration binary runs in the quick gate.
These are native regressions; the Lean integer VC model contains no floating
semantics or precision guarantee. Direct intrinsic division and exceptional
integer division remain separate obligations.

## Typed admission and exceptional domains

Body proofs now retain declaration types before helper summarization and loop
obligation extraction. The compiler supplies its resolved type universe and
actual LLVM target width, including profile/triple/layout overrides. Admission
requires standard signed 64-bit scalar semantics; declared Boolean storage, floating,
unsigned, narrower, structured, unresolved and operator-overridden types are
explicitly skipped. Fixed-width storage metadata must agree with the model even
when the casting graph would fall back to a target-wide integer. Other backends
currently receive no arithmetic model. Globals remain outside the state-free
model; a reference is skipped conservatively even when a local shadows its name.

The standalone `briev_verify::verify::verify_bodies` API admits untyped
mathematical word ASTs. Source consumers must preserve their types and use
`verify_bodies_with_types`, whose resolver declares the actual scalar semantics.
The compiler wrapper resolves metadata instead of matching user type names.
Unrecognized declarations cannot acquire a word proof through a helper summary.

Integer division/remainder are admitted only with a literal divisor other than
zero or minus one. The restriction is conservative: dynamic safe divisions and
some valid minus-one operations are skipped until definedness obligations exist.
SMT bitvector division is total, which cannot by itself establish source/native
exceptional behavior. Generated loop contracts are rechecked at this boundary.

The regressions reproduce two previously accepted Float proofs (a nonzero
increment and cancellation through a Float local). Their f32 rounding
counterexamples demonstrate the missing type boundary; they do not establish
that every optimized native floating path has identical rounding behavior.

A subsequent native probe exposed `Bool + 1`: the typechecker borrowed the
right-hand integer's operator while retaining a Boolean result, and LLVM
returned an i64 sum as i8. Clang rejected that IR even though body verification
reported an identity proof. Resolution now requires the receiver's own declared,
inherited or protocol operator. Explicit cross-type declarations remain valid.
Declared Boolean parameters, locals and results are conservatively skipped by
the word prover until a separate expression/storage model exists. Boolean
literals, comparisons and connectives in contracts remain available.

## Symbolic value and path-fact snapshots

`SymbolicValue::Identifier` denotes an immutable entry atom inside an evaluated
value. `eval_symbolic` already eagerly copies the current symbolic value on
assignment. The production converter in `briev-verify/src/symbolic/snapshot.rs`
therefore consults current bindings only for leaves in the AST expression being
evaluated. Converting a saved symbolic value never follows that mutable map.
The same conversion serves SMT goals, helper summaries and guard/check facts.
Facts are frozen when encountered; an unknown value supplies no fact. Omitting
such a constraint weakens the VC conservatively. Known guards keep only their
feasible branch. The saved-tree depth cap remains eight and rejects oversized
values rather than guessing a free identifier.

The previous transitive substitution certified a copied input as zero after its
source parameter was overwritten. It also corrupted helper summaries. Raw guard
facts could contradict the entry precondition after a write and make an arbitrary
postcondition vacuously SMT-valid. Package and compiler CLI regressions cover these
false proofs, valid copies and known-state branches. A maintained external native
witness returns 7 at input 7 on AST/HIR at O0/O3 while the baseline reports the
false `term == 0` contract proven. Native generation is unchanged by this repair.
Input-dependent SAT counterexamples remain advisory, as in the existing API.

This repair establishes a concrete snapshot discipline, not a universal proof of
the full symbolic executor, recursive AST conversion, bound inference or solver
encoding. The Lean assignment model separately proves that a copied slot survives
an overwrite of its distinct source. Implementation proof attempts and their
bounds/unavailable outcomes are recorded in the dated
[state-snapshot plan](../plans/2026-11-15-132215-symbolic-value-snapshots.md).

The bounded Kani obligation `snapshot_leaf_conversion_is_exact` checks the actual
production leaf converter for arbitrary 64-bit literals, a fixed saved input
atom and Unknown. It reports zero failed checks with all four cover properties
satisfied. A private zero-for-identifier mutant fails the intended assertion.
The harness excludes cleanup of its proof-owned recursive enums; recursive AST
conversion, arbitrary identifier strings, HashMap state transitions and caller
soundness remain outside this proof. The five existing production kernels now
include this narrow obligation; this is not a full symbolic-executor theorem.


## Interval boundaries and feasible violations

The production `verify/interval.rs` kernel represents closed signed-i64 ranges;
`lower > upper` denotes emptiness. Strict restrictions use checked arithmetic:
`x > MAX` and `x < MIN` are empty. Equality/order restrictions are exact;
interior disequalities keep their interval hull and therefore overapproximate.
Literal-left comparisons use `Relation::mirror`, and negated comparisons use
`Relation::negate`. Both adapters share the production proof obligations.

Known inconsistent range facts make the implication vacuous. A fast negative
verdict becomes a hard violation only when its path is feasible. The adapter
first tries a concrete assignment satisfying every supported recorded fact,
then asks Z3 if needed. Unknown feasibility remains advisory. This prevents an
unreachable constant-return branch from refuting a valid contract. These facts
remain snapshots under the assignment discipline above.

Five Kani harnesses check actual production intersection, restriction,
classification, mirroring and negation for arbitrary i64 endpoints, literals and
witnesses. They establish membership/verdict properties without proving the
whole fact extractor, symbolic executor, witness adapter or solver. Together
with the previous five kernels and the SMT byte codec, the discharged production
kernel count is eleven. Deliberate wrapping-endpoint and incorrect-mirror copies
must fail their corresponding obligations. Debug/release API and compiler CLI
regressions cover the adapters. The constructed-false-contract unit fixture now
uses `Expr::Bool(true)` rather than a free identifier spelled `true`; its hard
violation assertion is retained.


Directly tracked field postconditions remain available in the standalone word
AST API. Their sort inference treats a field of a plain variable as a word, but
does not serialize fields into SMT or widen typed source admission. Field reads
use the lexical base name; following its saved entry atom would read another
variable's stale field. Every common assignment invalidates that base's field
entries after reading its RHS, including declaration callers. The field snapshot
regressions preserve latest writes and reject rebound stale values. Structural
equality recursively rejects Unknown, including pointer-identical trees. Field
maps, object alias semantics and these adapters are not covered by the eleven
primitive Kani obligations.
