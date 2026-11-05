# Field-receiver API integration with HIR dispatch

PR #70 CI run 42011950221 fails to compile its merge with upstream main:
the HIR Field arm added in 78be8eb6 still passes a register-name string to
the field loader's new optional evaluated-receiver argument. The standalone
branch was tested against the earlier base and did not contain this caller.

Merge current main without rewriting history. The HIR arm reconstructs an AST
receiver but has not emitted it, so pass None. The AST dispatcher's previously
evaluated receiver continues to pass Some; audit every loader call site. Extend
the existing native counter fixture to run with HIR emission both unset and
enabled, at O0 and O3. Preserve the shared loader's rationale and document its
two entry contracts in backend contracts and BUGS.

Final verification: cargo check/build, the native regression in both modes,
cargo test --lib (the source sweep remains delegated to CI), Praetor comparison,
full IR determinism and HIR/AST A/B gates. Push the follow-up to the existing PR
and inspect the replacement CI run. This is an API-integration correction, not
a performance change.

Local results: check/build pass without warnings; the counter fixture executes in both emission modes at O0/O3; 3499 library tests pass with the delegated source sweep excluded; Praetor adds no diagnostics; all ten determinism and eight AST/HIR A/B cases pass. Clang 17/18 compile and execute all three matrix programs with equal semantic output after excluding only labelled elapsed times. The raw matrix still reports those timing-byte differences.
