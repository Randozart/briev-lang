# Plan: M3 — the general row-form lowering (2026-10-03)

**Status:** ACTIVE. Closes the measured emitter gaps (the retirement
campaign: `benchmarks/results/2026-10-03-emitter-retirement-ab.md`):
softmax 1.8-2.5×, dot 1.05-1.4× — general vs the cooperative row
emitters. The retirement gate: general machinery reaches their numbers
→ the emitters (the hand-written loans) retire.

## What the two paths ARE (knob = `spirv_row_cooperative`)

- **OFF (general)**: one THREAD per row. The declared body lowers
  flat — every column access = a serial loop inside one thread
  (`o[w] = Σ x[w·D+d]·y[d]` = D dependent loads+FMAs in one thread).
  4-row softmax_rows = 4 threads total; 20-row dot = 20 threads.
- **ON (the emitters)**: 32 LANES per row (`warp_id = row`, lane =
  column chunk) — the row's work spreads across a warp, reductions via
  shuffles/SMEM. 4-row softmax = 4 warps = 128 threads.

The gap decomposition hypothesis: the OFF path's loss = (a) 32× less
parallelism per row (the tiny-row shapes = launch/wave bound, so not
32× measured — but per-row LATENCY = 32×), (b) no coalescing (one
thread walks a row serially = strided-1 but single-issued), (c) no
warp-level reduction (the serial accumulator).

## Phase 0 — the decomposition (this session's measurement)

1. Dump BOTH PTX kernels for softmax_rows + dot_row (knob on/off).
2. The instruction-mix + occupancy comparison: threads, registers,
   the loop structure.
3. The per-shape scaling probe: the gap vs the row length (K ∈ {128,
   1024}) and the row count — where the 1.8-2.5× lives (per-row
   latency vs tail vs launch).

## Phase 1 — the design

The declared bodies (softmax_rows!, dot!) + the associativity facts
→ the general lowering derives the lane mapping. The doctrine shape:
the lowering consumes the declaration + the proofs, NOT a recognized
pattern. Design sketch (from the decomposition, refined in-phase):
- the work-item = the row → the CTA = 32 lanes/row (the cooperative
  geometry, derived from `WorkItem::PerItem` + the reduction span);
- the column loop → per-lane strided chunks;
- the reductions → the warp shuffle tree (licensed by associativity).

The KEY question Phase 0 answers: is this a NEW general pass, or does
the existing general tier need only the work-mapping change (the flat
1-thread-per-item → 32-lane-per-item)?

## Phase 2 — implement behind the gates

Suite + the conformance sweep + the blob A/B + the
`emitter_ab_gate.sh` correctness (EXACT both arms) + the timing vs
BOTH the emitter and the current general.

## Phase 3 — the retirement verdicts

The gate re-run: general-new vs the emitters. Reach → the emitters
retire (the deletions). Not → the remaining gap = the ledger.

## Non-goals

- The tensor-tier GEMM path (production, not a loan).
- The chain-fusion M3 item (the OTHER M3 — the producer-consumer
  chains; separate).
