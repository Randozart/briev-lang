# Performance regression bisect — fast era vs current

**Date:** 2026-10-20
**Machine:** same host, 5 iterations, BOUND=50000000, harness
`benchmarks/build_and_bench.sh`.

## Trigger

The Oct 18 fixture-1 table showed several rows slower than the Oct 17
"post-stability-runtime-fixed" fast era. The question was whether the
provenance patch caused it.

## Bisect result

| Commit | float_math Briev | C | Ratio |
|---|---:|---:|---:|
| `3c15268a` fast era | 0.0423s | 0.0449s | **0.94x (Briev wins)** |
| `5aa90185` state/plain | 0.1260s | 0.0464s | **2.72x (C wins)** |
| current (`394ee57c`+ws fix) | ~0.126s | ~0.046s | ~2.74x |

The regression first appears at **`5aa90185` (state/plain storage classes)**.
It is not caused by the raw-pointer provenance patch; `float_math` has no
`Ptr` and the provenance patch does not touch its state fields.

## What the IR shows

At both commits `@txn_tick` uses the same countdown-loop shape and the same
13 loop-carried Float phis (FMUL/FADD fast). The main differences are:

- fast `%State` is 88 bytes with `stdout_buf`/print-chain fields present;
  state/plain `%State` is 72 bytes (the print chain is stateless);
- fast calls `@print_float(ptr %state, float)` / `@put_char(ptr %state, i64)`;
  state/plain calls `@print_float(float)` / `@put_char(i64)`;
- field indexes shift accordingly.

The hot loop is structurally the same. The slowdown is therefore in how the
stateless print chain / state layout interacts with the loop optimization, not
in the matrix arithmetic. It needs asm-level attribution before a fix; do not
"fix" it by restoring `ptr %state` to the print chain blindly.

## Other fast→current rows (same historical tables)

| Benchmark | Fast `3c15268a` | State `5aa90185` | Current |
|---|---:|---:|---:|
| float_math | 0.90x | 2.01x | 2.74x |
| telemetry_stream | 0.86x | 1.03x | 1.48x |
| pid_control | 0.99x | 1.23x | 1.23x |
| sweep_sparse | 1.30x | 1.44x | 1.49x |
| queue_drain | 1.35x | 0.45x | 0.47x |
| interval_step | 1.23x | 0.61x | 0.72x |
| ws_frame | — | — | **3.50x → 1.48x fixed** |

Two facts matter for the fix: state/plain **improved** queue_drain and
interval_step (the print-chain escape fix), and it **hurt** the float/stream
lane. A fix must preserve the former.

## Next

1. Compare `3c15268a` vs `5aa90185` at the linked-binary asm level for
   `@main`/`@txn_tick`: spills, register pressure, loop body size, FMA.
2. Try a minimal change on `5aa90185`: keep the print chain stateless but
   verify whether `%State` SROA/promotion or call inlining causes the gap.
3. Apply the general fix (no shape matcher, no strategy keyword) and re-run
   the fast-era table interleaved.

## ws_frame (already fixed)

The 3.50x ws_frame regression was a separate, self-inflicted contract-migration
artifact: per-byte runtime guards on `get_u8`/`ws_payload_byte` plus a
benchmark-level guard. Foreach range facts + a proven direct `Load#` removed
them; measured **1.48x**, better than the old 1.58x.
