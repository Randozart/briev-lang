# Socket borrowing effects — 2026-11-05

Controlled nonblocking socket probes retain descriptor/object wrappers because
the platform effect table lacks networking calls. Add synchronous socket ABI
effects in configuration, with conditional borrowing for sendto: MSG_ZEROCOPY
must be proved absent. Unknown flags and zero-copy sends retain capture behavior.
Linux documents buffer retention at https://docs.kernel.org/networking/msg_zerocopy.html.

Use bounded constant-set propagation for ordinary bitwise expressions, retaining
all pointer provenance and rejecting incomplete/dynamic flag sets. Tests must
distinguish copied sends, zero-copy, mixed and unknown flags; memory/descriptor
gates exercise real controlled peers. Preserve the existing conservative fallback.
Update reclamation architecture documentation and BUGS. No performance claim.

The connect fixture also exposed unclassified concrete method calls in address
construction. Register explicit-receiver methods in the call graph and dispatch
those calls using the receiver type. Preserve member shadowing even for methods
whose effects cannot be modeled; implicit receiver fields, generic members,
chain back-references and dynamic dispatch remain conservative. Tests retain
returned pointer aliases and reject a same-named free-function fallback.

Final library gate: 3,503 passed, zero failures; only the CI-delegated source
conformance sweep was excluded. All four new effect/alias regressions pass.
Controlled transport on the combined integration passes four Valgrind and
file-descriptor gates plus eleven invalid-input rejections. Collector functional,
640-read descriptor, nine memory cases and repeated 5,000-row preflights pass.
Praetor introduces no new diagnostics. Full IR determinism passes 10 programs;
Clang 17/18 agree for the available input (two dataset-dependent rows were
skipped because u.data is absent). The Kani harness is present but could not run:
the executor's glibc lacks the versions required by cargo-kani.

Final cargo build completed without warnings.
