# Mixed physical integer operands

Correctness defect at upstream 9f49b2f8: an enum Bool payload is unboxed to i8,
while a Bool parameter is boxed to i64. Comparing them emits `icmp eq i8
%payload, %boxed`, rejected by LLVM. A standalone Toggle enum reproduces it;
no JSON or app code is required. Inspect both operand orders and all comparison
operators, plus signed/unsigned fixed-width operands.

Add a general unequal-integer-width normalization before binary dispatch.
Extend the narrower operand according to its declared protocol and retain the
wider operand's physical type. Equal-width, float and vector paths stay intact.
This is a correctness repair, not a performance change. Verify native O0/O3,
AST/HIR, library tests, IR determinism and changed-file Praetor baselines.
Update backend-contracts.md and BUGS.md with the representation invariant;
preserve all existing rationale comments.
