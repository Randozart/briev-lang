# Small-object destination reuse — ws_frame prototype

**Date:** 2026-10-29
**Status:** mechanism validated; production implementation pending
**Workload:** `benchmarks/ws_frame.bv`, `BOUND=1000`, native x86_64,
`clang -O3 -flto -march=native -ffast-math`, interleaved wall timings.
**Expected output:** `0016384` (`acc=0`, `invalid=0`, total=16384).

## Question

After the ScalarValue enum aggregate prototype removed `%enum_out`,
`%esrt`, and `ret_copy`, `ws_frame` remained ~0.044s. Optimized IR showed
that the enum payload was a boxed 34-byte `WsFrame` object and that LLVM
inlining produced more object allocations than the unsafe no-copy control.
The next question was whether object construction into a caller-owned
destination was the missing performance mechanism.

## Hand transform

Starting from the environment-gated ScalarValue prototype IR:

1. `ws_partial` gained a leading hidden `ptr %obj_out`.
2. Its `malloc(i64 34)` was removed; every field store targeted `%obj_out`.
3. Its returned handle was `ptrtoint ptr %obj_out`.
4. Call sites allocated a local 34-byte destination and passed it first.
5. `ws_decode` gained `ptr %frame_obj_out`.
6. Its `ws_decode_mask` calls passed the frame destination as the object
   result destination.
7. `ws_decode_mask` gained `ptr %result_out`; the masked `Ok` path copied the
   source frame fields into `%result_out` and returned its handle.
8. The final unmasked `Ok(f)` path used `llvm.memcpy` from `f` into
   `%result_out` and returned `%result_out`.

This was a hand-written proof experiment, not production codegen. The exact
final hand transform remained output-incorrect (`acc=10920`), which identified
missing universal destination/copy semantics rather than refuting the speed
mechanism.

## Result

| Variant | Median-ish wall time | Output |
|---|---:|---|
| Current copying ABI | ~0.045s | correct at this stage |
| ScalarValue enum aggregate only | ~0.044s | correct |
| **Object destination reuse** | **~0.012–0.017s** | hand-transform incomplete |
| Unsafe no-copy control | ~0.012–0.016s | correct by coincidence/undefined lifetime |

Representative final runs: `0.014, 0.015, 0.015, 0.012, 0.014`.

This reaches the old baseline range without storing a pointer to a dead
callee stack frame. The mechanism is therefore validated.

## Interpretation

The dominant cost was not merely the two-word enum copy. It was heap
constructing the `WsFrame` object payload and preventing LLVM from treating
the result as a destination-initialized value. Caller-owned object
destinations remove the allocation and let the aggregate result carry a
stable handle.

## Production requirements

A compiler implementation must do more than the hand transform:

1. Classify small, non-recursive object constructions as destination
   candidates.
2. Add hidden object-result destinations to eligible helper signatures.
3. At call sites, allocate or reuse a destination only with a lifetime proof.
4. Emit destination writes for **every** constructor path, including pass-through.
5. Insert explicit copies when source/destination may alias or when a value
   escapes.
6. Keep exported/FFI object ABI unchanged through wrappers.
7. Preserve the public output and all ownership contracts.

Gate before enabling by default:

- `ws_frame` output `0016384`;
- `ws_frame` ≤1.10x locked baseline or an approved corrected baseline;
- no hot-path malloc for eligible object construction;
- `ws_frame_codec_parity`;
- `cargo test --all`;
- server and browser recommender suites.

Prototype artifacts were ephemeral under `/tmp/opencode`; the durable record
is this file and
`docs/plans/2026-10-29-enum-value-model.md`.
