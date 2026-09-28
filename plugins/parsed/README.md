# Parsed-Stage Plugins

Plugins in this directory run at the `$(Parsed)` stage, immediately after
parsing and before import resolution.  They operate on the raw AST and can
inject imports, desugar syntax, or transform the program before type checking.

**2026-07-21:** Renamed from `front/` to `parsed/` as part of the granular
pipeline expansion.  See `docs/plans/2026-07-21-granular-pipeline-and-ast-navigation.md`.

## Current plugins

- `prelude.bv` — Injects standard library imports for LLVM/Webstack/GPU targets
- `prelude-hw.bv` — Silicon prelude slot for CIRCT/Cell targets (no stdlib insert today — see the file header, gap E9)

## Writing a Parsed plugin

See [`docs/architecture/features/plugins.md`](../../docs/architecture/features/plugins.md).
