# noalias-via-dispatch + swan-song dominance — benchmark results

**Date:** 2026-09-07
**Baseline:** fresh worktree at `5d1d7e45` (HEAD before this slice)
**Harness:** interleaved wall-clock, `BOUND=50000000`, best-of/avg of 5+ runs,
identical linked binaries (`clang -O3 -flto -march=native -ffast-math`).

## Changes measured

1. **Dispatch: call `@txn_*` instead of inline for multi-txn ticks** (dispatch.rs).
   Restores the `noalias nocapture` function boundary that the reactor's
   inlining erased. Single-txn ticks stay inlined (no boundary to gain).
2. **Swan-song dominance fix** (counter.rs + emit_stmt.rs + context.rs): the
   exit block now clears `pending_phi_backedge`/`phi_field_regs` and
   swan-song-referenced let-locals bind through preheader allocas. Post-loop
   field/local reads are `%State`/alloca loads — dominance-valid on the
   zero-trip path and watchdog-exit path.

## Results (avg ms, lower is better; ratio = current/baseline)

| Benchmark | Txns | Baseline | Current | Ratio |
|---|---|---|---|---|
| sparse_dispatch | 8 | 61 | 61 | 1.000 |
| float_math | 1 | 49 | 47 | 0.947 (interleaved ×8) |
| ring_buffer | 1 | 71 | 70 | 0.986 |
| fasta | 1 | 326 | 312 | 0.957 |
| print_loop | 1 | 43 | 43 | 1.000 |

**No regressions.** The multi-txn target (sparse_dispatch) is timing-neutral —
the win there is structural: LLVM now sees the `noalias` boundary per
transaction body and can promote field loads to SSA registers across calls;
per-field phi phis + write masks already carried the sequential shape, so the
call overhead is absorbed by inlining where profitable (`@txn_*` carries
`alwaysinline` for cycle-free programs; cycle programs keep LLVM's cost model).

## Build-restored programs (previously invalid IR at HEAD)

| Program | Output | Status |
|---|---|---|
| examples/async-events-compiled.bv | 17 | CORRECT (acceptance value) |
| examples/async-ready-gate.bv | 111 | CORRECT (acceptance value) |
| benchmarks/nbody_newton.bv | -0.169207186 | builds; 7th-decimal drift vs C — pre-existing fast-math reassociation, see BUGS.md 2026-09-07 |

## Measurement discipline

- Baseline binaries and current binaries are byte-identical at the same commit
  (md5 verified) — the timing methodology was validated before use.
- The first naive harness measured `cd` subshell overhead as benchmark time;
  full-path invocation from a fixed workdir replaced it.
- float_math's first reading (1.044) was ordering noise; the interleaved ×8
  A/B (alternating baseline/current runs) gives 0.947. Lesson recorded:
  interleave runs, never time baseline-block-then-current-block.
