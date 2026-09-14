# Campaign plan: the remaining levers — parity, split-K, small-K, contract tier

**Date:** 2026-09-14
**Baseline:** ship 35.5 TF @4096³ (84.5% of the 42-TF cuBLAS anchor),
36.3 @8192³ (86%), 31.5 @2048³ (75%). E1f bound: 42.5 = parity. E8a
**stage-1 FIXED (2026-09-14): producer predicate selected warp 8 only;
`setp.ge %p2, %r9, 8` selects warps 8-9 → 128-K-sweep exact, 4096³
max_rel_err 5.208e-03 OK. But E8a is 34% SLOWER than the ship (23.8 vs
35.9 TF @4096³) — L1 closed with a negative result; ship remains the
best kernel.** Calibration: Briev +32% over Triton 3.2 on this part;
cuBLAS 42 = 82% of nameplate. Findings-of-record: the plan docs
2026-09-11/13 series + /tmp working notes.

## L1 — E8a stage-1 defect: `ws_debug` instrument (the parity gate)

**State:** consumer path proven exact (128-K16 single-CTA exact);
producer-fill data wrong (K≥32 fails ~0.6); four fill mechanisms and
both memory proxies eliminated; barrier pairing/peel/rebase/y-guard
audited repeatedly.

**Instrument (built in the generator, not hand-patched PTX):**
`ws_debug` mode — each kstep's accumulator chunk stores to a SEPARATE y
region (y sized M·N·(ksteps+1); chunk k goes to slab k; slab ksteps =
the accumulated result). One driver read answers: which ksteps are
wrong, which warp-region within the kstep, and whether the wrongness is
constant per kstep (addressing) or drifting (timing/barrier).

**Hypotheses it resolves, in order:**
1. Odd ksteps wrong + even exact → the producer fill lands in the
   WRONG STAGE (parity off-by-one in the fill-dst vs read-src) — check
   `((r2+16)>>4)&1` vs the consumer's `(r2>>4)&1` under the ACTUAL
   loop-entry state (the peel advances %r2 before the first WCLOOP).
2. All ksteps after the first wrong → the barrier handoff publishes
   stale data (producer publishes before its fill completes).
3. A fixed warp-column wrong → slab↔ng mapping slip in the fill.
4. Errors grow with kstep index → stage reuse collision (producer
   lapping the consumers).

**Cheap precursor (L2, 15 min): producer double-fill** — fill BOTH
stages with stripe (r2+16) every iteration. If y becomes exact → the
defect is stage-parity/timing; if still wrong → data-path. Runs before
the instrument.

**Gates:** correctness at 128-K16..128 + 2048/4096/8192 (existing
signatures) → interleaved A/B ×4 vs 35.5 → win bar ≥40 TF (parity
bound 42.5) → commit per verdict.

## L3 — Split-K for 2048³ (the other tail fix)

2048³ loses ~24% to wave quantization (256 CTAs / 112 slots = 2.29
waves). Split-K: partition K into S chunks (S=2: 512 CTAs → 4.57
waves; S chosen so ksteps·S fills the machine), partial sums to an f32
workspace, one combine kernel reduces. Contracts prove associativity
safety and workspace liveness — a compiler-native split, not a
hand-rolled one.

- Eligibility: shapes with < 3 waves AND K ≥ 512 (chunks stay ≥ 256
  deep); S = ceil(112·2 / ctas) bounded to divide K/16.
- Numerics: f32 workspace accumulate; final combine rounds once to
  f16 — CONTRACT CHECK: the f16acc chain currently accumulates in f16
  pairs; the split path is MORE accurate (f32 partials). Gate: 1e-2
  unchanged.
- Effort: generator (chunked-K variant of the existing kernel + combine
  emitter) + dispatch policy + driver support for the workspace. One
  session.
- Prize: 2048³ 31.5 → ~38-39 TF (portfolio's weakest shape fixed).

## L4 — small-K family (K=64-128): the "beat cuBLAS" shapes

Decode-shaped attention-head GEMMs (K=64-128, M/N 128-4096). Now
unblocked by the K=16 empty-commit fix. cuBLAS is split-K-bound here
(hand-tuned but generic); our (2,4) kernel at 4 CTAs/SM with full-K
f16 accumulation may already lead. Plan: gate the sweep
(M ∈ {128,256,512,1024,2048} × K ∈ {64,128}), compare vs cuBLAS
through the llama.cpp path at identical shapes, ship a dispatch entry.
The claim to win: "faster than cuBLAS at decode shapes" — a class
neither Triton nor cuBLAS tunings own.

## L5 — `gpu_schedule` pass (the contract-leverage tier)

Own plan doc before work (2026-09-14-gpu-schedule-pass.md). Scope:
DAG-driven inter-node analysis feeding the runner — sync elimination
(dependents-empty → back-to-back launches), y-lifetime buffer reuse
(last-consumer chains), fusion eligibility (elementwise consumer whose
only edge is the GEMM y → epilogue fusion; FlashAttention-class
multi-GEMM fusion as the stretch). Anchor evidence: FlashAttention
IO-awareness wins; our zero-copy residency is already contract-proven.

## L6 — ship micro-polish: CLOSED

Exhausted by evidence: five-axis config sweep, E5a/b/d, E7, E7b. No
further config-level lever exists; the remaining kernel gap is exactly
what E8a (L1) addresses.

## Order of execution

1. ~~L2 double-fill diagnostic~~ → ~~L1 instrument + fix chase~~ **L1 CLOSED
   (2026-09-14): E8a fixed but 34% slower than the ship — ship wins,
   `ptx_tensor_warp_spec` stays off.**
2. L3 split-K (independent; one session) — the next performance lever.
3. L4 small-K gates (cheap, high claim value).
4. L5 gpu_schedule plan doc + first pass.

## Undo

L1/L2 behind `ptx_tensor_warp_spec` (default off). L3 behind a
`split_k` dispatch policy knob (default off until gated). L4 adds
gates only. L5 additive analysis pass — no existing behavior changes.

## L2/L1 progress (2026-09-14): the kstep-chunk map — partial coverage CONFIRMED

Built a two-cubin diff driver (drv3): y(K=32) − y(K=16) = the kstep-16
chunk, element-wise. **The map's periodicity is CORRECT** (row period 7
= the A seed cycle via a_row; column period 5 = the B seed cycle) —
**the magnitude is ~1/8 of true** (observed ±1.6-2.5 vs expected
10-12.6). Interpretation: the producer's 256 st.shared copies COLLAPSE
to ~32 distinct smem positions — stage 1 is ~7/8 unwritten garbage with
the correct structural fingerprint showing through.

The static audit keeps asserting full coverage (64 lanes × 4 copies ×
16B, strides 1024B — exact tiling by construction) while runtime says
1/8 — factor 8 ≈ the copies-per-position ratio. The discrepancy is
exactly what a POSITION-ENCODED fill resolves: ws_debug mode writes
value = (smem_offset/16) into each copy; one y read maps every landing
position. Next session: build ws_debug in the generator (per-kstep
slabs + position encoding + driver BRIEV_Y_SLABS), read the map, fix,
parity A/B.

L2's hand-patched double-fill self-IMA'd — hand-PTX is formally
retired; all instruments go through the generator from here.

## L1 progress (2026-09-14, second pass): the seeding bug + a clean stage-1 verdict

**CRITICAL: the kstep-chunk "1/8 magnitude" finding was measured on a
HALF-SEEDED B matrix.** Every debug-driver invocation in this campaign
passed `b_off = M*K` (elements), but drv2/drv3/drv5 treat it as BYTES.
The kernel's B region is `[8192, 16384)` (K=32) / `[4096, 8192)` (K=16);
the drivers seeded B at `[4096, 12288)` / `[2048, 6144)` — so
kernel-B rows 16..31 (the **stage-1 source**) were literally ZEROS.
The stage-1 producer read zeros, wrote zeros into stage-1 smem, and the
consumer's mma added zero. The "periodic-but-1/8" chunk map was the
structural fingerprint of that empty-B artifact, not of a fill collapse.

Correct invocations: `b_off = M*K*2` bytes
(K=16: `0 4096 8200`, K=32: `0 8192 16392`).

**Clean verdict with correct seeding (refcheck, MSE vs CPU f32 reference):**
- E8a warp-spec **K=16: PERFECT** — MSE = 0.000000, all 16384 elements exact.
  Stage-0 (K prologue fill + prologue compute) is correct.
- E8a warp-spec **K=32: BROKEN** — MSE = 91.4, maxerr 15.5, every element
  off. Stage-1 (producer fill + WCLOOP) is genuinely broken; the
  "stage-1-broken" conclusion survives the seeding fix.

**Producer fills stage-1 smem, but the consumer's ldmatrix reads mostly
zeros.** With correct seeding, direct smem dumps at WCLOOP time show:
- B stage-1 `[0..16)` = `1.5, 2.0, 0, 0.5, 1.0, 1.5, 2.0, 0` = exactly
  `B[16][0..7]` (the producer's B-fill lands correct data).
- A stage-1 `[0..8)` = `0.5, 0.75, 1.0, 1.25` = exactly `A[0][16..19]`.

But the consumer's `ldmatrix.x2.trans` on the SAME stage-1 base returns a
B tile that is almost all zeros (one `1.5` at position 1). So the
producer's B-fill smem destination layout (XOR-swizzled, verified
formula-identical to the working K-prologue B-fill) does NOT line up with
the consumer's ldmatrix read addressing on the stage-1 path — the fills
and reads disagree on where stage-1 B lives. A stage-0 works because the
K-prologue fill uses 256 threads with `tid*16` dests; the producer uses
64 rebased lanes × 4 unrolled copies (`r5*16 + i*1024`) into the same
swizzle — the mismatch is in that translation, or in a missing fragment
swap on the consumer side. Next: diff the producer B-fill dest derivation
against the consumer B ldmatrix address derivation for a concrete (lane,
iter) pair, then fix the producer's dest (or the consumer's read).

**ws_debug instrument fixes made this pass** (env-gated, default off):
- WCLOOP slab base is recomputed fresh from `%r2` each iteration
  (`rd6 = state + y_off + (r2>>4)*slab_stride`) instead of carrying
  `%rd6` across `emit_f16_compute` — ptxas was spilling/reordering it.
- Prologue ws_debug store moved before `bar.arrive 2`.
- Residual instrument artifact: with `BRIEV_WS_DEBUG`, the WCLOOP store
  block also writes a stale quadrant to slab+2 (rows 0..63, cols 32..63 =
  warp-1's tile shape, 2048 elements). Provably a ptxas register/
  scheduling side-effect of the instrument (independent of `rd6`, SASS
  shows all STG offsets ≤ +0x800, loop executes once) — does NOT affect
  the real kernel (the y-store path is separate). Not worth chasing.

## L1 RESOLVED (2026-09-14): E8a stage-1 FIXED — and slower than the ship

**Root cause (one line):** the producer-role predicate was
`setp.eq.u32 %p2, %r9, 8` — it selected only **warp 8**, so warp 9
(r5 32..63) fell through to the consumer path. The fill formulas assume
64 producer lanes, so only half of stage-1 landed (B rows 16..23 = warp
8's `r5>>2` range; warp 9's rows 24..31 never written) — the stage-1
half-coverage behind the "MSE 91". Fix: `setp.ge.u32 %p2, %r9, 8`.

**Diagnostic trail:** a position-encoded producer canary (store the writer
lane `r5` at `global[4MB + dest_offset]` per B `st.shared`) proved only
lanes 0..31 wrote to stage-1; a warp-dependent constant (warp 8 → 1.0,
warp 9 → 2.0) proved the consumer never saw warp 9's data; the consumer's
stage-1 chunk was exactly `sum_{k=16..23} A[r][k]` (5.75) = warp 8's half.

**Verified (refcheck, CPU f32):**
- 128×128 K=16/32/64/96: MSE = 0.000000, every element exact.
- 512³ / 1024³: correct within f16-accumulation precision (rel ~1.6e-3).
- 4096³ max_rel_err = 5.208e-03 OK (identical to the ship).

**Benchmark verdict (ptx_gemm_bench, same driver):**
- 4096³: E8a 23.8 TF vs ship 35.9 TF — **E8a 34% slower**.
- 2048³: E8a 20.6 TF vs ship 31.1 TF — **E8a 34% slower**.

**Conclusion: E8a is correct but loses.** The producer/consumer split
trades the ship's cheap consumer-side cp.async self-fill for two idle
producer warps per CTA (40 vs 32 warps/SM) plus a per-stage barrier
handshake (`bar.sync 1/2` + `membar.cta`) on the critical path. At 2
stages the fill is tiny, so the prefetch benefit never materializes.
L1's parity gate is closed with a negative result — the ship (E4c)
remains the best kernel. The `ptx_tensor_warp_spec` config stays default
off. (A deeper-stage E8a variant could amortize the producer cost, but
the barrier latency on the critical path is structural; not worth it on
this GPU.)
