# Relative code-import fix

The recommender parser gate exposed dot-to-slash namespace expansion corrupting
filesystem-relative code imports. Preserve relative paths while retaining the
existing bare namespace rule. No application import changes are needed.

Regression tests cover current/parent directory paths, explicit and implicit
extensions, a dotted filename, and the existing bare dotted namespace behavior.
Documentation updated: BUGS.md, net-stack bug log, and import architecture note.
Existing rationale comments are preserved. No performance change is intended.

Verification pending: the human runs cargo check, cargo test --lib, the compiler
build, Praetor on src, and the recommender parser gate. Do not commit until these
gates pass and the human has reviewed the increment, per WORKFLOW.md.

Verification update (2026-10-21): human reports cargo test --lib green. The
human's debug cargo build finished successfully without warnings; brievc check
on udata.bv returned OK; the compiled udata_gate printed all six checks true.
This verifies the previously failing ../ import end to end. Praetor and commit
approval remain pending; integration into the task branch follows the commit.

Pre-commit Praetor (2026-10-21): full src scan reports existing diagnostics.
A controlled same-config before/after check of import_resolver.rs initially
showed increased resolve_import complexity; path conversion was extracted into
code_module_path. Repeating the comparison produced identical diagnostics
(normalized only temporary directory names and source line numbers). No new
Praetor diagnostics. This extraction changes no behavior but needs the human's
post-refactor cargo check/test/build gate before the authorized commit.

Final verification (2026-10-21): after helper extraction, the human reports
cargo check, cargo test --lib, and cargo build all green. The earlier native
parser gate passed all six checks, including the originally failing import.
Praetor before/after diagnostics match. Commit and push authorized; feature work
is paused until the human submits the compiler-fix PR. No integration yet.
