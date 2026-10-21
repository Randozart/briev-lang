# Pointer-valued collection store investigation

Task 10 schema gate emits invalid LLVM in pg_decode_fields. `%t28` is a
string-literal pointer but is consumed by `store volatile i64 %t28` at
scratch/recommender_schema_gate.ll:6879. The dynamically decoded text has the
same mismatch later in the function. Both enter singleton List<String>
construction before iter_chain$String. No PostgreSQL connection is needed to
reproduce this compilation failure.

First experiment: identical singleton-list defns over Int versus String, with
observable element reads in a node. Scratch probes isolate collection literal
construction from the net stack and database. Human builds/runs these probes;
do not treat the proposed reduced trigger as confirmed until output arrives.

Trace: emit_stmt.rs indexed-store path already calls ensure_typed_value for
scalar pointees. That helper skips boxed handles, then resolves source LLVM
type and converts ptr to i64. Determine why this string register bypasses the
conversion before selecting a fix; do not change pg.bv to evade the failure.

After confirmation, fix on a separate branch from main with regression tests,
including the integer control. Audit conversion-helper consumers if changing
shared behavior. Update BUGS.md, net-stack bug log, backend architecture rationale,
and prepare an untracked root PR body. Preserve existing rationale comments.
Human runs cargo check/test/build and native probes; compare Praetor against
the unchanged file. Backend changes also require the documented IR determinism
and toolchain gates. No performance change is proposed.

Status: investigation and probes authored, verification pending.

First controlled result (2026-10-21, human-run): both isolated Int and String
singleton-list defns compile and print true. This REFUTES the broad hypothesis
that List<String> literal construction alone fails. The schema's failing
expression is fields + [text] / fields + [""], with an inline RHS literal.
Second probes isolate list concatenation, covering a String parameter and empty
String literal plus an Int control. Human execution pending; no compiler change
has been made based on the refuted first hypothesis.

Confirmed discriminator (2026-10-21): human-run Int concatenation prints both
checks true; String concatenation fails clang with the same ptr/i64 mismatch.
The first literal-only hypothesis was refuted and is not the implemented fix.

Root cause: generic function-call lowering constructs a literal argument using
its declared unspecialized List<T> parameter before monomorphizing the call.
The String pointer is consequently assigned an unresolved T register type in
collection initialization. Typed standalone literals already work correctly.

Implementation: frontend typechecker/call_literals.rs reuses generic-call
inference, substitutes the declared collection parameter, and elaborates a
literal argument to an ordinary typed value block at that argument's original
evaluation site. Both authored calls and operator-generated calls use the same
helper. The internal binding cannot be spelled as a source identifier. Existing
collection construction/codegen paths are unchanged. Tests cover literal and
parameter String values, Int controls, direct generic calls, and nesting.
Architecture note updated; no backend source change, performance claim, or new
syntax. Verification pending HUMAN cargo check/test/build and native probes.

Pre-test review (2026-10-21): generic-call literal typing runs from one shared
elaboration wrapper after child/operator rewriting, so direct and synthesized
calls cannot drift. The original expression walker is preserved under the name
elaborate_expr_children. Praetor comparison reports only that existing walker
name changing on its pre-existing 103-line diagnostic; counts, complexity and
line metrics otherwise match. The new module has no diagnostics.
The internal temporary uses a dot, which the lexer excludes from binding
identifiers (unlike $, which IS legal); nested literal blocks are covered.

Regression investigation (2026-10-21): human full-suite run reports 43 failures.
The newly failing conformance, package import, JSON interpreter, list interpreter,
list elaboration, and operator-call literal tests share exactly the same
iter_enumerate_loop / iter_zip_loop empty tuple-accumulator type errors. These
are real regressions from the new pass, not GPU/toolchain exclusions.

Cause: elaborate_ops had not initialized current_output_type, unlike ordinary
check_item_body; revisiting generic calls inferred [] as List<Int> rather than
the enclosing declared List<(Int,T)> / List<(T,U)>. Set the declared output
context for both defns and txns before elaborating. Add regression assertions
for both stdlib tuple accumulators and a callable-txn body. No test exclusions
or stdlib edits. Human filtered-test verification pending, then full suite.

Focused regression verification (2026-10-21): human reports both
`cargo test --lib -- typechecker::` and `cargo test --lib -- list_plus_list`
green after initializing declared-return context. This includes the new
literal/tuple-accumulator regressions and interpreter concatenation. Remaining
gates: post-fix full suite against the known environment baseline; debug build;
native Int/String concatenation probes. No commit yet.

Final verification (2026-10-21): human reports full suite green. The rebuilt
String concat probe prints four true checks; Int control prints two true checks.
The originally failing reduced native program now compiles and preserves values.
Praetor has no new diagnostics or increased metrics. Compiler fix is ready for
review and commit approval; schema-gate rerun awaits integration. No feature
files are included in the compiler-fix branch's intended commit.
