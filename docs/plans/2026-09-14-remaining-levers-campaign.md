# Campaign plan: the remaining levers — parity, split-K, small-K, contract tier

**Date:** 2026-09-14
**Baseline:** ship 35.5 TF @4096³ (84.5% of the 42-TF cuBLAS anchor),
36.3 @8192³ (86%), 31.5 @2048³ (75%). E1f bound: 42.5 = parity. E8a
built, stage-1-incorrect (consumer path proven exact; producer-fill
data wrong). Calibration: Briev +32% over Triton 3.2 on this part;
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

1. L2 double-fill diagnostic (15 min) → L1 instrument + fix chase
   (the parity verdict).
2. L3 split-K (independent; one session).
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
