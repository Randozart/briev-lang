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
