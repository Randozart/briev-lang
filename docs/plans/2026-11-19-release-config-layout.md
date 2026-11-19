# Release resource layout

## Goal and evidence

The v0.2.0 archive installs configuration under `config/config/`, while the
compiler resolves files under `config/`. At baseline `efd99d7b`,
`scripts/package_release.sh` creates `$staging/config` before copying the
directory `config` onto that existing directory. Standard `cp -R` semantics
therefore introduce an extra level.

## Implementation and documentation

1. Add an archive-level regression using a temporary packaging fixture. Mock
   only the Cargo build, then execute the real script, extract the archive,
   verify its checksum and resource contents, and repeat to check replacement
   and deterministic archive generation.
2. Copy the contents of the config directory, retaining its hidden files and
   subdirectories. Preserve the script's explanatory comments and document the
   root cause in `BUGS.md`. No compiler architecture changes are required.
3. Run the regression before/after, library and quick gates, build, shell syntax
   checks and Praetor on changed source directories. Record actual outcomes.

This is an artifact-correctness change; it changes no generated program or
optimization and makes no performance claim.

## Execution record

- Archive regression: baseline failed on the missing direct configuration
  path; fixed version passed, including checksum and repeatability checks.
- `cargo test --lib`: 3,935 passed, 2 ignored, zero failed.
- Shell syntax and Praetor on `tests/`: passed.
- `cargo build`: passed with the same 12 existing `briev-verify` warnings.
- `bash scripts/gate.sh quick`: passed, 4,056 selected/passed, zero failed,
  unreported or not-run cases. Manifest:
  `target/gate-results/20261119T065103Z-46b3e51fd2db/manifest.json`.
