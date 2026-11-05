# Nonblocking transport checkpoint — 2026-11-05

Carve out the existing single-step socket and finite poll modules from the
full-stack vessel into a main-based feature branch. Native controlled peers
cover partial writes, EAGAIN, read progress and EOF, readiness clearing,
connection success/refusal, invalid descriptors and eleven bounds/lifetime
rejections. Keep implementation pure Briev and raw Linux syscalls.

Document caller-owned cursors, SO_ERROR read-and-clear, descriptor ownership,
finite wait limits and Linux x86-64 ABI in architecture/nonblocking-transport.md.
Final compiler library and native integration checks follow WORKFLOW. The app
checkpoint additionally checks all four probes under Valgrind. Compiler tuple
and socket-effect fixes remain separate branches; no compiler source belongs
in this feature PR. No HTTP workload, throughput or capacity claim is made.

Final checks: 3,499 compiler library tests passed (the source conformance sweep
remains delegated to CI). The clean branch's native integration passes all three
successful fixture outputs and all eleven invalid-input cases. On the combined
compiler integration, all four consuming transport probes additionally pass
Valgrind and descriptor cleanup. Praetor passes for both new stdlib modules.

Final cargo build completed without warnings.
