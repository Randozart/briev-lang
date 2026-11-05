# Collector checkpoint on current main — 2026-11-05

Complete one load-harness checkpoint: reproducible collector correctness,
allocation and descriptor stability, using a compiler built from main plus
explicit stdlib prerequisites. Preserve the dirty integration tree.

The prior use-after-free is a source-combination mismatch: its uncommitted
casting graph aliases Data-to-String while main's formatter frees its Data
buffer after the encoding copy. Main still uses the encoding constructor.
Do not change upstream String semantics to compensate for the mixed tree.

Main independently leaks read_file descriptors: read_whole returns without
closing the successfully opened descriptor. A minimal repeated-read probe will
compare the next available descriptor across successful, empty, growing,
open-failure and read-failure cases. First run it against unchanged main;
then close the descriptor in read_file after read_whole returns, preserving
the original read result. This is a stdlib resource-lifetime fix, not a
compiler optimization. No benchmark timing claims are involved.

The clean fix branch is fix/read-file-descriptor-lifetime, based on 3737396b.
Regression fixture and native integration test accompany the fix. Final gates:
focused native A/B, Valgrind, full library tests (existing CI-delegated source
sweep excluded), build without new warnings, Praetor comparison. Update BUGS.md,
resource-lifetime documentation and the net-stack/task ledgers; preserve rationale.
Carve collector prerequisites separately from main. The app checkpoint records
source/binary hashes and passes functional/memory checks plus two 5000-row runs.

Final descriptor-fix verification: 3,499 library tests passed, zero failures,
with only the previously delegated source sweep excluded. The native integration
test passed; cargo build completed without warnings. Praetor passed the changed
stdlib directory. The independent Valgrind probe passed 640 reads with zero
memory errors/losses. The combined application functional suite, nine memory
cases and two 5,000-sample preflights passed before final repository tests.
No Rust compiler change was needed for this checkpoint.
