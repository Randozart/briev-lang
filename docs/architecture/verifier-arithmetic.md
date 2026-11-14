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

## Native correspondence remains open

The general integer expression emitter in `src/backend/llvm/emit_expr.rs`
currently emits `add nuw nsw`, `sub nsw` and `mul nsw`. These flags require
non-overflow; the interpreter permits wrapping. LLVM documents that overflow
with these flags produces poison ([LLVM Language Reference](https://llvm.org/docs/LangRef.html#add-instruction),
consulted 2026-11-14). Optimizing the matching IR pattern `(x + 1) > x`
with these flags produces constant true, whereas wrapping evaluation is false
at signed MAX. This is a confirmed semantic gap in the emission rule; the
isolated IR experiment is not a full Briev end-to-end reproduction.

Do not infer a native-program guarantee from a verifier proof until the emission
rules, alternative lowering paths, target widths and exceptional arithmetic have
been reconciled and tested. Removing a flag only in a proof model cannot repair
the emitted program.
