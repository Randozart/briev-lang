# Stdlib fixture campaign, fixture 1 — ws_frame (WebSocket codec): first results

**Date:** 2026-10-18
**Commit:** 855a9c1e (read-side pointee-stride fix) + harness registration
**Status:** FINAL for fixture 1's first pass — all 79 harness gates MATCH,
zero MISMATCH.

## What was built

- `lib/std/net/bytes.bv`: RFC 6455 codec — get_u64_be/put_u64_be, WsFrame,
  ws_decode (bounds-proven, Err("incomplete")/Err("protocol")), ws_payload_byte
  (whole-frame-fit precondition — over-reads unrepresentable), ws_encode
  (minimal length forms, per-byte masking). Commit 9707f4bf, fixed c2d1aa62.
- `benchmarks/ws_frame.bv` + `ws_frame_c.c`: deterministic corpus (4096 frames
  spanning 7/16/64-bit lengths, masked/unmasked), decode-validate-checksum
  across PASSES=4; C reference shares the LCG, corpus, validation, prints.
- `tests/ws_frame_codec_parity.rs`: interpreter contract pin (decode
  scenarios green; len64 + encode deferred — interpreter BugOr bug, below).

## The campaign's real product: five compiler bugs found and fixed

1. **Import splice closure** (bde072ac): match-arm calls + member-body calls
   invisible to the transitive closure — every cross-module Result parser and
   any GrowBuf importer failed with raw-Int fallbacks.
2. **Pattern grammar** (9a43fcb2): qualified variant patterns impossible in
   importers (known_types gate); `::` gave a misleading parse error. `::` is
   illegal; dot form works everywhere; SPEC corrected.
3. **Causality/concurrency harvesters** (acbff4bb): nested + indexed writes
   invisible — false "no convergence" refusals AND an unsound Rule 22
   write-disjointness proof for every indexed store.
4. **Goal-reachability** (acbff4bb): direct stores of the goal's value
   unprovable — the beginprogram and causality gates jointly refused valid
   single-node boot programs.
5. **Pointee-stride mismatch** (acbff4bb + 855a9c1e): element stores through
   raw pointers hardcoded i64 (INVALID IR for byte pointees); fixing writes
   alone then broke Ptr<Float> reads (4-byte write vs 8-byte read —
   recommender regressed, caught by the harness MISMATCH gate, fixed same
   session).

Plus one SOURCE-level lesson recorded in BUGS.md: the codec's own
sequential-guard fall-through (c2d1aa62) — discarded tail values are not
returns; the compiler was right, three hours of IR archaeology proved it.

And one OPEN interpreter defect (BUGS.md 2026-10-18): compound bitwise-or
over non-literal operands yields stale values in spliced programs (also hits
GrowBuf growth) — len64 decode + encode round-trip are deferred in the
interpreter parity test until fixed; compiled side fully verified.

## ws_frame numbers (provisional shape note: per-byte codec-call design)

| Metric | Value |
|---|---|
| Briev | 0.0367s |
| C | 0.0233s |
| Ratio | 1.58x (C wins) |
| Correctness | MATCH (acc=0, invalid=0, total=16384 — byte-identical output) |

The 1.58x gap is the per-byte codec-call design (ws_payload_byte per byte,
bounds re-proven per call) vs C's inlined per-byte loop. An `_idio` variant
(word-at-a-time unmask, or inlined payload walk) is the natural follow-up —
recorded, not excuses.

## Full runtime suite (post-campaign-fixes, all MATCH)

| Benchmark | Briev | C | Ratio | Winner | Correct |
| ring_buffer | 0.0428s | 0.0421s | 1.02x | C | MATCH |
| float_math | 0.1229s | 0.0432s | 2.84x | C | MATCH |
| float_math_nonzero | 0.2432s | 0.1644s | 1.48x | C | MATCH |
| sparse_dispatch | 0.0415s | 0.0571s | 0.73x | Briev | MATCH |
| print_loop | 0.0274s | 0.0541s | 0.51x | Briev | MATCH |
| nbody_newton | 7.2978s | 7.6645s | 0.95x | Briev | MATCH |
| nbody_newton_accel | 1.3188s | 0.1220s | 10.81x | C | MATCH |
| nbody_sqrt | 2.5572s | 3.2426s | 0.79x | Briev | MATCH |
| nbody_sqrt_idio | 2.7931s | 3.0599s | 0.91x | Briev | MATCH |
| fasta | 0.1915s | 0.2569s | 0.75x | Briev | MATCH |
| fannkuch_redux | 0.0609s | 0.0548s | 1.11x | C | MATCH |
| mandelbrot | 0.6374s | 0.6754s | 0.94x | Briev | MATCH |
| kalman_filter_runtime | 0.1852s | 0.1790s | 1.03x | C | MATCH |
| knucleotide | 0.1804s | 0.1880s | 0.96x | Briev | MATCH |
| cancel_math | 0.0408s | 0.0568s | 0.72x | Briev | MATCH |
| bit_clear | 0.0005s | 0.0002s | 2.50x | C | MATCH |
| queue_drain | 0.0277s | 0.0569s | 0.49x | Briev | MATCH |
| queue_drain_sym | 0.0272s | 0.0573s | 0.47x | Briev | MATCH |
| queue_drain_idio | 0.0275s | 0.0566s | 0.49x | Briev | MATCH |
| stack_push_pop | 0.0291s | 0.0572s | 0.51x | Briev | MATCH |
| interval_step | 0.0403s | 0.0585s | 0.69x | Briev | MATCH |
| telemetry_stream | 0.2321s | 0.1553s | 1.49x | C | MATCH |
| pid_control | 0.4246s | 0.3471s | 1.22x | C | MATCH |
| matrix_pipeline | 0.4762s | 0.4924s | 0.97x | Briev | MATCH |
| accumulator_flush | 0.1257s | 0.1357s | 0.93x | Briev | MATCH |
| sweep_sparse | 0.2283s | 0.1527s | 1.50x | C | MATCH |
| sweep_mid | 0.2246s | 0.1649s | 1.36x | C | MATCH |
| sweep_dense | 0.3852s | 0.3686s | 1.05x | C | MATCH |
| sweep_arr | 0.4290s | 0.4276s | 1.00x | ~tie | MATCH |
| series_converge | 0.0004s | 0.0002s | 2.00x | C | MATCH |
| global_lifetime | 0.0278s | 0.0645s | 0.43x | Briev | MATCH |
| deep_recursion | 0.0002s | 0.0001s | 2.00x | C | MATCH |
| arena_churn | 0.0288s | 0.0927s | 0.31x | Briev | MATCH |
| linked_list | 0.9793s | 1.8601s | 0.53x | Briev | MATCH |
| hash_ops | 0.8792s | 0.9820s | 0.90x | Briev | MATCH |
| hash_ops_idio | 0.7061s | 0.6728s | 1.05x | C | MATCH |
| enemy_swarm | 0.0994s | 0.1288s | 0.77x | Briev | MATCH |
| recommender | 0.0119s | 0.0111s | 1.07x | C | MATCH |
| ws_frame | 0.0367s | 0.0233s | 1.58x | C | MATCH |
| bridge_glue | done |  |  |  | MATCH |
| bridge_multi | done |  |  |  | PASS |

