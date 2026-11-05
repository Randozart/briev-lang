# Tuple values and controlled transport — 2026-11-05

Next checkpoint: controlled native client transport, with bounded single-step
I/O, readiness, connect success/refusal, split data, EOF, backpressure and broken
peers. No server workload or capacity claim belongs to this checkpoint.

Clean compiler base 84f37578. The saved returned-object tuple reproduction
panics because unannotated destructuring defaults elements to Int. A previous
partial fix failed local tuples because materialization also returns Int.
Preserve Type::Tuple metadata through construction and hoisting while retaining
target-width handles and fixed i64 storage slots. Both numeric projection and
destructuring must consume one decoding helper using the existing casting-graph
LLVM resolution/adapt_from_i64. Include tuples among narrow boxed handles.
Do not specialize names, socket types, or an application shape.

Verify returned/local/nested/hoisted tuples with mixed object, float, string,
boolean, integer and pointer values. Check native execution at O0/O3, wasm IR
validation/execution where supported, and lifetime with Valgrind. Investigate
any transport runtime failure only after the compiler reproductions pass.
Use separate main-based fix and stdlib branches; integrate with the completed
collector branches for application checks. Publish focused PRs without another
approval step, as the user authorized in this session.

Preserve existing tuple ABI and float/String rationale comments in the shared
helper; update backend-contracts and BUGS with the final behavior. Final checks
follow WORKFLOW: full library suite after implementation and application gates
(except the source sweep previously delegated to CI), build, Praetor comparison,
full IR determinism and installed-toolchain checks. This is correctness work,
not a performance optimization, so no timing or benchmark claim is proposed.

Validation: 3,501 library tests passed, zero failures; only the explicitly
CI-delegated source conformance sweep was excluded. Both native O0/O3 and
wasm32 LLVM integration tests passed; the independent Node execution returned
wasm32_nested_tuple_values_ok. Mixed native tuples had zero Valgrind errors and
zero definite/indirect loss. The combined client/collector integration passed
all controlled transport gates and collector memory readiness. Praetor adds no
new diagnostics; the two touched large emitter functions have lower complexity.
