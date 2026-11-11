# Closed-region ownership proof — completed CI evidence

Commit: `f3012b95d66acbbec0da8f66eb2eee6fc939c0f0`.
Run: https://github.com/odecode/brief-lang/actions/runs/43026494439

- Library: 3,607 passed, zero failed.
- Native ownership: four passed, zero failed.
- Kani: one harness verified, zero failures out of 880 checks (18 unreachable).
  Unwinding assertions enabled, unwind 4; 5 GiB resident-memory cgroup, no swap,
  35-minute timeout. Both jobs completed successfully.
- Praetor on `src/analysis/reclamation`: passed before the push.

The proof covers production storage rules instantiated with a two-root,
one-word exact-set model sharing production union and iteration primitives.
It checks external-storage self-edge preservation through field/raw projection,
a further raw load, and publication. It does not verify the B-tree backing,
full solver scheduling, or the entire compiler. See
`docs/architecture/llvm-memory-management.md` for the scope and test coverage.

This supersedes the pending proof status and initial virtual-memory-limit
wording in earlier dated records. Earlier resource failures were not proofs.
Application measurements in the repair record predate this commit and are not
claimed as fresh application runs. Full authenticated endurance remains open.

Upstream `60673b8c` merged cleanly after this successful run. A new fork CI run
will verify the combined revision, including the full integration gate, before
replacement PR publication.
