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

## Phase 0 — the decomposition (DONE 2026-10-03)

The SPIR-V lane's cooperative path is ALREADY general machinery (the
body synthesis + the generic lowering) — **the loans are the PTX-side
hand-written emitters** (`emit_cooperative_softmax_ptx`,
`emit_cooperative_dot_ptx` — complete hand-written kernels bypassing
the general emit path).

The measured structure (softmax_rows 4×256, dot_row 20×128):

| | general (OFF) | cooperative (ON) |
|---|---|---|
| LocalSize | 256 (1 thread/row, rest idle) | 32 (lane = column chunk) |
| work/row | serial O(K) in one thread | K/32 per lane + reduction |
| reductions | serial accumulator | warp shuffle/butterfly |
| softmax ops | 470 | 255 |
| dot ops | 1 loop, 0 group ops | 3 FMA + 1 group op |

The gap decomposition: (a) 32× the parallelism per row (the tiny-row
shapes cap it at 1.8-2.5× measured — launch/wave bound), (b) the
serial accumulator vs the shuffle tree.

## Phase 1 — the design (scoped)

The PTX general emitter learns the THIRD work binding — **row form**:
- row = ctaid.x (one BLOCK per row), the row index_var = ctaid.x;
- the column loops stride by the block width (tid.x + 32·i);
- the accumulator reductions = the warp butterfly (`shfl.sync.bfly`)
  — licensed by the associativity proof the shape already carries.

Where: the general node emitter's work-binding branch (general.rs
~:1085 — the `block_work_item` arm's sibling), keyed on
`is_cooperative_shape` (declared + proven — the same gate the SPIR-V
lane uses). The bodies lower through the generic emit_stmt — NO
hand-written kernels.

The retirement verdict then: the general-derived PTX row kernels vs
the hand-written emitters (the emitter_ab_gate A/B). Reach → the
emitters retire.

## The implementation notes

- The PTX shuffle: `shfl.sync.bfly.shfl.b32 %r, %r, 16, 0x1f;` halves —
  5 steps for 32 lanes; the max = `OpGroupNonUniformFMax`-equivalent via
  fmax + the butterfly on the fused pattern (the emitters' exact
  sequences = the reference).
- The inner-loop remap: the desugar'd `foreach kk in 0..K` → the
  strided form (`kk = tid.x; kk < K; kk += 32`) — the same remap
  synthesize_softmax_stmts does on the SPIR-V side.
- The tiny-shape guard: rows < 32 → the flat form (the row-form's
  tail waste dominates) — the strategy threshold.

## The emitters' destination — bad<ptx> payloads, post-verdict (decided 2026-10-03)

The hand-written PTX emitters' retirement home = **bad<ptx> payloads**
(the kernel-level GPU escape: the fn name bridges by node, the derived
SPIR-V keeps serving Vulkan). NOT before M3's verdict:

1. The emitters are gated on this campaign anyway — M3 landing retires
   them and the question dissolves.
2. The parameterization ABI does not exist: the emitters bake the SSBO
   projected offsets, the row count, and the row length per program;
   bad<ptx> today = one `.param .b64` (the state pointer), offsets as
   source literals — a shipped `lib/bad/softmax_rows.bad` would be
   wrong for every other layout. The template/ABI extension gets built
   against the KNOWN survivor set after the verdict.
3. Moving now = per-program .bad sources + per-fixture bridges + new
   machinery, for artifacts the campaign may delete.

Post-verdict: the surviving emitters (if any) move to
`lib/bad/*.bad` payloads with the ABI they need; M3's derived lowering
races the payloads (the tier-3 floor per derivation-not-recognition).

## Phase 1 — the implementation order (concrete)

1. The row-form gate: `is_cooperative_shape` (declared + proven +
   knob-independent NOW — the row form = the GENERAL lowering's own
   decision, so the knob retires with the emitters) + the tiny-shape
   guard (rows < 32 → the flat form).
2. The work binding: row = ctaid.x, lane = tid.x, the row index_var =
   ctaid.x, the block-wide guard.
3. The column-loop remap: the inner foreach → the strided form
   (kk = tid.x; kk < K; kk += 32) — mirroring
   `synthesize_softmax_stmts` on the SPIR-V side.
4. The reductions: Max# → the shfl butterfly fmax; the Exp#-sum /
   dot-acc → the shfl butterfly add — the emitters' exact sequences
   are the reference (redux.f32 = sm_100+, NOT available on sm_86).
5. The gates: the unit tests (the PTX assertions), the conformance
   sweep, blob A/B vs knob=off, `emitter_ab_gate.sh` correctness both
   lanes, then the timing vs the emitters.
