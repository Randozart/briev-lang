# Closed-region repair verification — 2026-11-11

Base: upstream `c7a06444`; original candidate replay `f63e7c56` plus the
uncommitted boundary-storage repair. These are correctness/memory gates, not
throughput or capacity measurements. PR #90 is closed.

## Reproduction and correction

The original compiler reproduced the server double free (`io_flush` stack).
The standalone nested-storage fixture reported two Valgrind errors: reading a
payload freed by `publish_nested`, then freeing it again. The repaired fixture
passes at O0 and the compiler's optimized link: zero errors and no
definite/indirect/possible loss; 65,536 stdout bytes remain reachable.

## Application and catalogue gates

- Single authenticated journey: all five operations, `journey_ok` and
  `journey_durable_ok` (independent SQL plus exact receipt checks).
- Rebuilt repaired driver, three users, one slot, closed admission: completed
  3, failed 0, missed 0; ratings started/done/OK 15/15/15; all error outcomes 0.
  Independent SQL: `15|15|15|15|15|3`.
- Artifacts: application `target/pr90-repair/{gdb-smoke,fixed-smoke,repeated}`;
  `repeated/inputs.sha256` records compiler/server/driver/dataset hashes.

| Catalogue repetitions | Definite/indirect/possible loss | Massif peak heap bytes |
|---:|---:|---:|
| 1 | 0 / 0 / 0 | 37,164,092 |
| 4 | 0 / 0 / 0 | 37,164,092 |
| 16 | 0 / 0 / 0 | 37,164,092 |

Every run printed `catalogue_checked,N`. Artifacts and input hashes are under
application `target/pr90-repair/catalogue`. No historical baseline peak is
presented as a newly measured value. Full-journey bounded memory is not claimed.

## Compiler gates

- Focused reclamation: 41 passed.
- Native reclamation: 4 passed, including new incoming-storage fixture.
- Full determinism: all ten corpus modules byte-identical across processes.
- Toolchain matrix with MovieLens input: LLVM 18 recommender/multinode/giantnode
  all OK; only one distinct compiler version discovered.
- Praetor: changed Rust files pass after extracting boundary seeding helper.
- Final release rebuild: passed without warnings; library suite: 3,607 passed,
  zero failures; native suite: 4 passed, zero failures. Final compiler SHA-256:
  `48c466b0be4f2232f69b99c4d268c470c4c1ed96ed5a1c11f33437c6bf70b27d`.
  Recompiled regression IR is byte-identical before/after helper extraction.
- Kani: pending. Both local attempts were interrupted by restarts; neither is
  a completed proof. The user reported machine memory exhaustion and requested
  CI-only execution. `.github/workflows/closed-region-proof.yml` runs the same
  harness on the fork with a 5 GiB virtual-memory limit, 35-minute proof-command
  timeout and uploaded logs/status/commit provenance. Exhaustion fails the job.
  Do not restart the whole-graph proof locally.

The first toolchain attempt had missing input and did not complete the gate;
only the subsequent dataset-backed run is counted. Final suites include the
equivalent helper extraction and shared optional-Valgrind test cleanup.
