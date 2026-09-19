# Flash-Decode Shape: Composition Ceiling Proof + Gate Plan

**Date:** 2026-09-19
**Status:** experiment evidence collected; hand-PTX gate designed, not yet run
**Supersedes:** nothing; extends `benchmarks/results/2026-09-18-m4-decode-microbench-gate.md`

## 1. What was measured (dmaj-K 3-kernel composition, bitnet geometry)

Per-kernel wall time (launch + exec + `cuCtxSynchronize`), H=20 HKV=5 G=4
NKV=4096 D=128, K stored d-major, V/j-major f32, CUDA lane, RTX 3060:

| phase    | baseline | +4x unroll pv | +8x unroll pv |
|----------|---------:|--------------:|--------------:|
| qk       |   66 µs  |      66 µs    |      66 µs    |
| softmax  |   34 µs  |      38 µs    |      38 µs    |
| pv       |  212 µs  |     153 µs    |     146 µs    |
| chain    |  317 µs  |     253 µs    |     252 µs    |

Correctness held at every step: `a_err=9.65e-06 -> PASS`, identical to the
unpatched chain (4x and 8x fp reassociation do not move the 1e-3 bound).

Experiment artifacts: `/tmp/opencode/m3.eKBX/` — `pv.ptx` (emitted, serial),
`pv_pipe.ptx` (hand 4x unroll + running pointers, ptxas-clean, PASS),
`pv_pipe8.ptx` (8x, distinct load registers, PASS), `measure_perclock.c`
(per-kernel decomposition driver), `harness_pipe*.c` (spliced kernels).

## 2. Findings

### F1: pv was issue/latency-bound, not bandwidth-bound
Emitted pv inner loop (serial foreach, 4096 iterations): zero unroll, index
math recomputed per iteration, MLP=2. Hand 4x unroll with running pointers:
212 -> 153 µs (-28%). 8x: 146 µs. The M3 pipelining pass (a4884ef7) only
covers lane-reduction loops; **serial reduction loops get no pipelining**.
This is an emitter gap with a measured payoff, independent of fusion.

### F2: composition cannot reach 58 µs
- softmax (20 blocks, trivial) costs 34-38 µs -> that IS the launch+sync
  floor. Three serialized launches floor the chain near ~50 µs before any
  work.
- pv at 146 µs is now L2-traffic-bound: V is read once per query head
  (G=4 re-reads, f32) -> ~336 MB L2 side-band at ~2.3 TB/s effective.
  f16 V halves this, but the earlier f16 refutation (qk, latency-bound)
  does not transfer to pv; untested here.
- Even perfect kernels sum to ~135 µs > 58 µs target.

Conclusion (confirms M4 gate): **kernel-count is the structural wall. Only
a fused flash-decode shape can reach parity.**

### F3: fused shape constraint (design input for the gate)
Flash decode needs, per block (one head): persistent per-lane acc[d] (threads
must own d), while K-dot wants coalesced K. With K d-major and threads
over d, fixed-j K loads are stride-16KB uncoalesced. Resolution (ggml's
design, and ours): stage K/V tiles in shared memory with coalesced
*linear* loads, consume from smem, `bar.sync` per tile (~256 syncs, cheap),
warp-shuffle dot reduce + cross-warp smem reduce per j. V accumulate:
`acc_lane = acc_lane*em + p*V_smem[j][lane]`.
**f16 KV is required, not optional**: f32 KV gives a 21 MB DRAM floor = 58 µs
— parity at best. f16 halves the floor to 29 µs and leaves headroom.

## 3. Gate experiment (before any emitter work)

Hand-write the flash-decode PTX kernel (grid=20, block=128 threads, smem
tiles, online softmax, f16 K/V decode via `ld.global.u16` + `cvt.f32.f16`),
drive it through a 1-kernel harness desc, verify `a_err < 1e-3` vs the CPU
reference, then time launch+sync.

**Gate criteria:**
- >= 2x faster than the 253 µs pipelined chain -> build the emitter pass
  (frontend LoopShape variant `FlashDecode`: outer j foreach, inner d reduce
  with per-lane persistent accumulators, online-softmax scalars uniform).
- < 58 µs -> wire fattn.cu replacement path.
- 58-126 µs -> f16 + tile-shape tuning before any emitter work.
- fails correctness or >= 253 µs -> composition + serial-unroll is the
  shipping path; fusion closed as refuted.

## 4. Ordered work queue

1. **Serial-loop unroll pass** (emitter, independent win, do first):
   extend the M3 pipelining machinery to serial foreach-reduction loops —
   running pointers + N-deep load unroll, N chosen from trip count divisibility
   (front-end computes; 4 for 4096/128). Measured ceiling today: -28% pv,
   chain 317 -> 253 µs. Additive: new match arm in the serial-emit path;
   `_ => return None` untouched.
2. **Flash-decode hand-PTX gate** (§3).
3. If gate passes: plan `docs/plans/2026-09-XX-flash-decode-shape.md` for the
   LoopShape + emitter feature (frontend-driven per dispatch doctrine).
4. f16 V for pv path (re-test the refuted hypothesis on the L2-bound kernel).

## 5. Results (updated 2026-09-19, work queue item 1 LANDED)

The serial-loop unroll pass shipped as a first-class emitter feature
(`SerialUnrollPlan` in `src/backend/ptx/general.rs`, knob
`ptx_serial_unroll` in `config/ir-lowering.dbvl`, default 4):

| phase | baseline | hand 4x PTX | **compiler pass** |
|-------|---------:|------------:|------------------:|
| pv    |  212 µs  |    153 µs   |      **98 µs**    |
| chain |  317 µs  |    253 µs   |     **202 µs**    |
| qk    |   66 µs  |     66 µs   |       65 µs       |

Correctness unchanged: a_err = 9.65e-06 PASS on both lanes (bit-exact vs
the serial form — accumulation order preserved). The compiler pass beats
the hand experiment likely because the kernel now ships as offline-ptxas
cubin (`ptx_emit_cubin`), not driver-JIT text.

qk did not move: its inner d-loop is `q[..d..] * k[..d..]` — both sites
linear in d, so the plan matches, but the kernel is already
latency-saturated at 65 µs; investigated, no regression.

Verification: 2282 lib tests green (4 new: linear_coeff, subst_item,
matcher, emitted-PTX shape), Praetor clean (zero new diagnostics vs
baseline 4319e8df), release build warning-neutral.

Remaining against the 58 µs ggml target: chain 202 µs — kernel-count
still the structural wall (§2/F2). Work queue items 2-4 (flash-decode
hand-PTX gate, LoopShape feature, f16 V for the L2-bound pv) are open.
