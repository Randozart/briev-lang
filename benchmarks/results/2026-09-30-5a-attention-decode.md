# 5a — decode attention measurement (2026-09-30)

5a's plan (followup-stages) assumed the composite attention path landed
and the only work was to retire the dead `fused_attention_*` family. This
measurement **refutes that premise**: at the target geometry the decode
attention is ~20× off the ≤125 µs goal.

Geometry: bitnet-2b decode, **D=128, H=20, HKV=5, G=4, NKV=4096**, CUDA
lane, GPU 1. Rigs: `benchmarks/m4_decode_microbench.sh` (3-kernel
composition) and `benchmarks/composite_decode_microbench.sh` (one-launch
composite), p50 of 200 reps.

| path | p50 | vs target ≤125 µs | vs ggml |
|---|---|---|---|
| 3-kernel composition (qk→softmax→pv) | **3443 µs** chain | 27× | **~59× slower** |
| — qk | 222 µs | | |
| — softmax | 41 µs | | |
| — **pv** | **3179 µs** | | |
| one-launch composite (`softmax_fused!`) | **2478 µs** launch | 20× | **~43× slower** |
| ggml stock fattn (test-backend-ops) | ~58 µs | — | — |

**The pv step dominates** (M=1 single-query, N=D=128, K=NKV=4096): with
M=1 the tensor mw kernel cannot tile, so the reduction over NKV is
serialised — the classic decode under-parallelisation. qk (222 µs) and
softmax (41 µs) are secondary.

## Consequence for 5a

- The `fused_attention_*` family is **not** dead weight at this geometry:
  the plan's own target table lists "Flash decode (hand-PTX)" at
  **125 µs f32 / 91 µs f16** — ~20× faster than the composite here. The
  composite wins only for *prefill* (512²), where the 1-kernel was 10×
  slower than the 2-pass composition. So the choice is **shape-dependent**.
- **Retiring the family unconditionally would regress decode** — the
  opposite of the planned intent. Do NOT delete until the composite reaches
  decode parity (or the cost model gates decode to the fused path).
- The real 5a work is a **decode-parallel attention kernel** (split-K /
  parallel NKV reduction / lane-mapped reduction per `0051d920`/`05c`), to
  bring the composite from ~2478 µs toward 125 µs.

## MEASUREMENT CAVEAT → RESOLVED (high-REPS sustained protocol)

The composite launch latency is **bimodal at low rep counts**: over 200
reps, p10 = 200.8 µs but p50 = 2470.5 µs. That was the **clock-ramp
transient** — the first few hundred launches run at a parked clock while
the driver ramps. Re-run with **REPS = 2000** (sustained) the distribution
tightens to **p10 194.9 / p50 200.6 / p90 207.1 µs**.

**Reliable protocol: use ≥1000 reps.** True baselines:

| path | ramped p50 |
|---|---|
| composite (one launch) | **200.6 µs** |
| 3-kernel (earlier, low-rep) | confounded — re-measure with high reps |
| ggml fattn | ~58 µs |
| target | ≤125 µs |

So the decode attention gap is **~1.6× to the target** (3.4× to ggml) —
NOT 20×. The earlier "20× off" and "2478 µs" figures are retracted as
clock-ramp artifacts. The structural finding stands: the kernel is
grid-underfilled (`grid = H`, no split-K) with heavy per-`j` overhead.


## Recommendation

Re-scope 5a: (1) gate the decode path by shape (prefill → composite,
decode → the fast path); (2) implement the parallel decode reduction; (3)
delete `fused_attention_*` only once the composite meets decode parity,
per Rule 24's retirement gate. The measurement rig is the m4 +
composite_decode microbenches above.

## Split-K sweep (2026-10-01) — design option 1 measured

`benchmarks/composite_decode_split_sweep.sh` (1200 reps, ramped, the
two-launch CUDA contract: partial n*S + combine n). Same session, same
rig — compare rows to each other:

| variant | p10 | p50 | p90 |
|---|---|---|---|
| no-split | 218.9 | 220.8 | 339.1 |
| split=2 | 222.3 | 223.5 | 232.0 |
| **split=4** | **200.4** | **202.5** | **210.2** |
| split=8 | 250.6 | 252.0 | 260.9 |
| split=16 | 318.5 | 324.1 | 336.6 |

Findings:
- **split=4 wins**: −8% p50 (220.8 → 202.5) and the tail TIGHTENS
  1.6× (p90 339 → 210 — the combine stabilizes what the no-split
  single-launch leaves to clock-cap variance).
- **S > 4 regresses**: per-CTA work shrinks below the two-pass
  overhead and the combine's merge cost grows with S. 4 is the model
  factor (`reduction_split_factor(20, 4096)` = 4) — the cost model
  and the measurement agree.
- The grid-underfill lever is worth ~8%, NOT the 1.6×: with S=4 the
  grid is 80 CTAs (fills 28 SMs) yet latency barely moves — the
  residual cost is the PER-J WORK (2 passes × 128 j-iterations/warp ×
  strips + 11 butterflies + Exp# + div slowpath). Lever 2 (fuse the
  two passes, inline the butterfly, hoist q) is the remaining path to
  125 µs.
- Harness fixes landed: `composite_decode_microbench.sh` (the split
  contract drive + BRIEVC_FLAGS default) and the sweep script (the
  two-launch drive parsed from the runner's own sequence; the first
  plain launch after the partial is the combine — later plain launches
  are other lanes' full images).
- Correctness: the split machinery's numerics are device-proven at the
  softmax_gate fixtures (5.91e-06 both lanes); the decode-geometry
  split correctness gate rides the next increment (lever 2 lands with
  its own m3-harness gate).

## E2/E3 — the j-iteration body composition (2026-10-01, spy-ptxas dump)

The split=2 partial's PTX (pre-ptxas, 832 lines / 21178 bytes, dumped
via a saving-ptxas wrapper) shows the per-j body is NOT load- or
butterfly-bound — it is **address-arithmetic bound**:

- 4 strips × (q, k, v) = 12 `ld.global` per j-iteration.
- EACH load's address is recomputed from scratch per iteration: ~10
  integer ops (`mul r,h,128` + `mul r30,128,4096` + `mul.wide` + two
  `add.u64` + the const offset) — including CONSTANT-FOLDABLE products
  (`128*4096 = 524288` emitted as two muls!) and the loop-INVARIANT q
  base (`h*128` + the const buffer offset).
- The butterflies are already inline (0 `call` in the pre-ptxas text —
  the out-of-line `CALL.REL.NOINC` figures were post-ptxas SASS
  artifacts; 10 `shfl` total per kernel = one butterfly set per pass,
  NOT per strip).
- 3 `bar.sync`, 1 `ex2` — secondary.

**Lever 2 re-sized:** strength-reduce the deferred-region j loop —
hoist the q base, carry k/v addresses by stride increment (`+= 4096*4`
per j), and let the emitter's index emission skip recomputing
loop-invariant sub-expressions. This is an index-emission change in
`emit_deferred_region`'s j loop → the kernel-rule on-device gate
applies (m3 harness, both lanes) before any timing claim.

## Lever 2 (first half): the j-invariant load hoist — 2026-10-01

The E3 composition (12 loads/j, each q reload j-invariant) sized the
hoist; the implementation lifts j-invariant array reads out of BOTH
deferred passes as immutable per-strip lets (registers — last_val_temps;
the alloca path only takes mutated bindings).

Two defects found by the gates on the way, both fixed:
1. strip-collapse: one shared hoist name let the last strip's register
   win in last_val_temps — every strip read strip-N's q element
   (CUDA max_rel 4.48e+01). Per-strip names (`__dqh<strip>_<idx>`) +
   per-strip rewrite pairs.
2. the rewrite pairs passed to a strip were ALL strips' pairs — the
   first match won, so every strip read strip 0. (The probe: got[d] ≈
   ref[d−1].)

**Timing (composite decode, 1200 reps, same session):**

| variant | pre-hoist p50 | post-hoist p50 |
|---|---|---|
| no-split | 220.8 | **191.5** (−13%) |
| split=2 | 223.5 | 212.8 |
| split=4 | 202.5 | 198.8 |
| split=8 | 252.0 | 249.1 |
| split=16 | 324.1 | 317.8 |

The no-split path gains most (128 j-iters/warp × 2 passes × 4 removed
loads) and is now the best decode number: **191.5 µs** (from the
200.6 µs baseline; target 125 µs — 1.53×). The 8 in-loop q loads per
kernel are gone (12 → 16 total loads incl. the 4 pre-loop hoists).

Correctness: softmax_gate s8 + knob fixtures PASS both lanes
(5.91e-06 / 2.06e-05 — the pre-hoist numbers exactly); suite 2845.

## Pass-split probe (2026-10-01) — the fusion case measured

`ptx_deferred_skip_pass` (diagnostic, the nofill precedent; config-dir
with the full ir-lowering + the key). Decode geometry, n=800:

| variant | p10 | p50 | p90 |
|---|---|---|---|
| full (both passes) | 252.8 | 255.1 | 302.6 |
| pass B only (skip A) | 124.1 | 124.8 | 128.1 |
| pass A only (skip B) | 76.8 | 77.3 | 78.7 |

(The config-dir file must carry the FULL ir-lowering — a one-key file
resets every unset knob to its parse default and the kernel degrades to
~10.7 ms; the 255 µs full row is the honest baseline.)

**The dot is computed TWICE** (pass A for max, pass B for the
accumulate) — that is 2× the loads and 2× the FMA work for one
softmax. The online form (one sweep: dot, running max, rescale acc/l
on update) computes the dot ONCE: projected ~125-140 µs. The rescale
counter-cost is negligible — max updates occur ~ln(4096) ≈ 8 times
over the sweep, and the 128-float acc is already smem-resident
(smacc[16384]).

The composite ALREADY has the online structure (the `dlen ≤ 32` branch
of softmax_fused!); the span threshold exists because of the
acc-in-REGISTERS assumption. The lever: the deferred emitter learns
the fused form for smem-resident acc — a j-loop restructure gated by
the m3 harness (both lanes) before timing.

## Lever 2 (second half): the fused online j loop — 2026-10-01

**The target is beaten.** The pass-split probe showed the dot computed
twice; the fused online form computes it once. `ptx_deferred_online`
(default 1 after the A/B) — one j sweep: dot, running max + rescale,
p, l, acc; per-warp running max merged with rescale
(l_tot = Σ redl[w]·exp(redm[w]−m_glob), same for acc strips).

**Timing (decode geometry, 1200 reps, same session):**

| variant | p10 | p50 | p90 |
|---|---|---|---|
| two-pass (pre-change) | 183.7 | 320.2 | 326.7 |
| **fused online** | **72.0** | **72.5** | **75.5** |

**2.77× faster; the 125 µs target is BEATEN (72.5 µs)** — ggml
reference ~58 µs now 1.25× away. The p90/p50 ratio collapsed (1.04 —
the short kernel keeps the clock ramped; the clock-cap tail is gone).

Correctness: softmax_gate s8 + knob PASS both lanes — the CUDA lane
*improved* to max_rel = 0.00e+00 (the online rescale algebra matches
the combine exactly); Vulkan 2.06e-05 (pre-change value). m3 harness
at the attention geometry: PASS both lanes (a_err 1.63e-05/1.34e-05).
Suite 2845; gemm_h byte-identical (the deferred change doesn't touch
the tensor path).

The two-pass form remains behind `ptx_deferred_online: 0` (the A/B
fallback); `ptx_deferred_skip_pass` stays diagnostic-only.

## Fused + split sweep (2026-10-01) — the shipped-best config

The pass-split probe's sweep measured the TWO-PASS form under split;
this sweep re-measures with the fused online kernel (`ptx_deferred_online: 1`,
1200 reps):

| variant | p10 | p50 | p90 |
|---|---|---|---|
| fused no-split | 71.8 | 75.9 | 383.4 |
| fused split=2 | 86.1 | 86.6 | 90.4 |
| **fused split=4** | **71.7** | **72.2** | **74.5** |
| fused split=8 | 92.2 | 93.1 | 100.4 |

- p50: split=4 (72.2) and no-split (75.9) are within noise — the fused
  kernel is not grid-bound.
- The TAIL is the difference: no-split p90 = 383 (the clock-cap
  artifact on the 20-CTA grid) vs split=4 p90 = 74.5 (160 CTAs fill
  the SMs; the clock stays ramped). split=4 is the STABLE config.
- Correctness: the s8 gate fixture declares `split<8>` — its PASS with
  the online config IS fused+split verified (CUDA 0.00e+00: the online
  rescale + the combine rescale compose exactly). The combine handles
  per-slice (m, l, acc) correctly under the fused form.

Shipped-best decode config: fused + split=4 — 72.2 µs p50, 74.5 p90
(target 125; ggml ~58).

## Next-increment design: float4 k/v loads (the ggml 1.25× gap)

The fused loop per j per thread: 4 scalar k loads + 4 scalar v loads
(one per strip, d = lane + 32·strip). The d-mapping is TRANSPOSED vs
float4: `ld.global.v4.f32` needs 4 CONSECUTIVE elements per lane
(d = lane·4 + c). The flip touches:

1. **d_regs setup**: `lane + 32·i` → `lane·4 + i` (the strips become
   per-lane contiguous quads).
2. **k loads**: 4 scalar → 1 `ld.global.v4.f32` per j (the row base
   `k_row + lane·16` — 16-byte alignment REQUIRED: the decode fixture's
   offsets are 16-aligned ✓ but the emitter needs a runtime alignment
   check with a scalar fallback — the general rule, not a fixture
   assumption).
3. **v loads in acc_stmts**: same v4 form.
4. **The hoisted q reads**: 4 lets → 1 v4 load (q base + lane·16 ✓).
5. **The smacc merge offsets**: the warp-row layout must match the new
   d-mapping (currently `lane + 32·i` f32 within the row → `lane·4 + i`).

Payoff: 8 scalar LDG → 2 v4 LDG per j per thread (4× fewer load
instructions; the loads are the residual cost after the hoist + fusion).
Kernel-index-math change → the m3 + softmax gates (both lanes) are
mandatory before any timing claim. Fixture offsets verified 16-aligned.

## Float4 implementation findings (2026-10-01) — design refined, deferred to fresh session

The banked flip has TWO coupling constraints surfaced by source review:

1. **The flip without v4 fusion is neutral-to-negative.** Current
   mapping (d = lane + 32·strip): every strip load is 100%-coalesced
   (one 128B sector). Flipped without v4: stride-4 accesses, 25%
   per-transaction efficiency — 3 of 4 strips become L1 re-read hits
   (DRAM traffic identical, but 12 extra L1-latency loads per j set,
   pipelined). No win without the fusion; the two are inseparable.
2. **The fusion vehicle exists but has a key-collision subtlety.** The
   `pipelined` map (M3, general.rs:879 — the lane-reduction's
   preload/consume pattern) is the right mechanism: the schedule owns
   the loads, the body consumes registers via the expr's Debug key.
   But the k expr's Debug form is STRIP-INVARIANT (it contains the `d`
   binder, swapped per strip at emit time) — one pipelined entry would
   serve all four strips the SAME register. The fix: the per-strip
   rewrite substitutes unique binder names (d_regs have unique reg
   names) making the keys strip-unique — plus the v4 raw loads
   addressed off the row term (the `Add(row, d_binder)` shape, scalar
   fallback otherwise) and the compile-time 16-byte field-offset check.

**Verdict: real work — whole-function treatment, fresh session** (the
postmortem discipline: no late-session splices into the 900-line
function). The payoff (8 LDG → 2 LDG.128 per j) is estimated 10-20%,
worth a focused session, not a tail-of-session attempt.

## VERIFICATION CORRECTION (2026-10-01, late session) — read before citing

1. **The fused online kernel's correctness is VERIFIED at the s8
   geometry only** (KV=256, H=8, split<8> and no-split, both lanes,
   hardened gate: 8.48e-06 / 6.57e-06 / 2.06e-05). **The decode
   geometry (H=20, NKV=4096) probe shows o1 = 0** — the node fires
   (r=20), the launch succeeds, the PTX contains the normalize stores —
   but o1 reads zero post-run. Under investigation; the 72.5 µs figure
   is the measured TIMING of the fused kernel's work shape, not a
   verified-correct kernel at that geometry. Do not cite 72.5 as a
   proven kernel until the decode probe is resolved.
2. **`softmax_gate.sh` ignores `BRIEFC_FLAGS`** (only
   `m3_attention_harness.sh` expands them) — earlier "online PASS"
   runs via the config-dir built the TWO-PASS path. Post-flip runs use
   the shipped default (online=1) so the current gate numbers are real.
3. **The gate's error metric had a NaN hole** — `NaN > max_rel` is
   false, so NaN outputs silently passed (the CUDA 0.00e+00 anomaly).
   Hardened: NaN → err = 1e300 → FAIL.
4. **The online fn was missing the redl/smacc stores and the tail
   call** — both fixed (the stores make the merges read real warp
   partials; the tail call writes o1). The s8 verification above is
   post-fix.

Open item: the decode-geometry probe (got=0, node fires, PTX has the
normalize stores). Next session: resolve with the m3 harness at the
decode geometry (it honors BRIEVC_FLAGS) and a fresh reading of the
normalize addressing (the o1 write offset vs the probe's read offset —
GQA kvh indexing is the first suspect).

## RESOLVED (2026-10-01, the variant-diff gate) — the decode probe was plumbing

`deferred_ab_gate.sh` (the P1 harness) ran both variants at the decode
geometry with fresh runners: **two-pass vs online agree at max_rel_diff
= 5.894e-06** — the fused online kernel IS correct at the decode
geometry. The earlier `got=0` probe was an artifact: the probe's
printf was re-spliced across multiple attempts (real newlines inside
the C string), mangling the injected runner — the DEC-PROBE read was
untrustworthy while the reference loop read seeded data.

**The 72.5 µs decode claim is now variant-verified**: timing shape AND
variant agreement at the target geometry. The trusted instruments for
every future deferred-path change: `deferred_ab_gate.sh` (variant
diff) + the m3 harness (live reference, honors BRIEFC_FLAGS) — never a
hand-spliced probe again.

Shipped-best decode config: fused + split=4 — 72.2 µs p50.

## Float4 k/v loads LANDED (2026-10-02) — the ggml-gap increment

Commits `d21f61d8` (the quad path) + `6512ac86` (three gate-caught defects).
Design as banked above; the two coupling constraints held: flip+v4 landed
together; per-strip unique binder names (`__vdqk{i}`/`__vdqv{vi}_{i}`/
`__vdqq{gq}_{i}`) keep the quad rewrite strip-correct. Fallbacks (D%128≠0,
misaligned fields, f16, non-d-affine sites) keep the scalar strips — the
scalar form also stays the two-pass A/B arm.

**The gates caught three real defects before any timing claim:**

1. **Split partial-store layout** (`6512ac86`): the per-slice partial
   record baked the ORIGINAL d mapping (lane·4 B base, 128 B strip stride
   = d lane+32·i). Under the flip the slots must follow the NEW mapping
   (lane·16 B base, 4 B stride = d lane·4+i). First fix attempt changed
   only the stride — the 16-byte stores then OVERLAPPED (lane L covered
   floats 2+L..5+L; slots d≥35 zeroed by write races). The gate metrics
   that caught it: softmax s8 CUDA 61.2 → 33.4 → 8.47e-06 PASS.
2. **Cast-invisible k site**: the decode composite reads
   `(k[..] as Float)`; the v4 walks + `rewrite_j_expr` did not descend
   `Expr::Cast` — the decode kernel silently stayed scalar. All walks are
   Cast-transparent now (dot_buffers precedent); the composite emits 3
   quads (q hoist + k + v).
3. **AB-gate GQA seed overrun** (`6512ac86`, instrument): the P1 gate
   seeded k/v with H·NKV·D floats — 4× the GQA decode buffers
   (HKV·NKV·D) — glibc aborted both variants pre-dump and the gate
   compared STALE bins from a previous run. Today's first "PASS 5.894e-06"
   was exactly the Oct-1 record value = stale-file comparison. Fixed: the
   seed reads the declared counts from the runner field table.
   **Rule for every gate consumer: delete the dump files before the run
   and require fresh files — a PASS on pre-existing bins is void.**

Also: `examples/gpu/attention_decode_composite_d20.abv` — the decode
geometry as a PERMANENTLY named fixture. The three `_tmp.abv` files
deleted earlier today (abdec/dec/decspy) were the Oct-1 session's decode
fixtures — the committed composite is s8 geometry; "byte-identical to
each other" made them look like strays. Lesson: an instantiation differs
from its template by CONSTS ONLY — those consts are the geometry; check
what a gate actually ran before deleting its fixtures.

**Correctness gates (v4-active build, all PASS):** m3 decode H=20/NKV=4096
both lanes (a_err 9.65e-06 CUDA / 1.02e-05 VK); softmax s8 both lanes
(8.47e-06 CUDA / 2.06e-05 VK; record 8.48e-06/2.06e-05); AB variant-diff
s8 9.466e-06 + decode 3.274e-06 (fresh bins, fixed instrument);
gemm_h byte-identical (the Cast arm touches shared rewrite machinery);
suite 2847.

**Timing (Rule 12 interleave, base = worktree at `fec8b89a` built fresh,
same machine window, 1200 reps):**

| variant | base p10 | v4 p10 | Δ | base p50 | v4 p50 | Δ |
|---|---|---|---|---|---|---|
| no-split | 133.1 | **116.1** | **-12.8%** | 136.2 | 118.1 | **-13%** |
| split=2 | 153.1 | 135.7 | -11.4% | 154.2 | 138.5 | -10.2% |
| split=4 | 143.7 | 140.2 | -2.4% | 146.3 | 143.9 | -1.6% |
| split=8 | 184.9 | 178.1 | -3.7% | 186.3 | 181.1 | -2.8% |
| split=16 | 262.9 | 245.1 | -6.8% | 264.6 | 258.3 | -2.4% |

(The morning pre-change sweep and the first post-change sweep sit in the
same bands — the record's 2026-10-01 absolute ~72 µs remains unreachable
in today's machine state; ALL absolute numbers here are same-session
relative only. One v4 no-split run caught a transient spike — p90 741 —
the table uses the clean interleaved runs.)

**Shipped-best decode config UPDATE: fused v4, NO-SPLIT** — 118.1 µs p50
/ 135.7 p90 clean. The split=4 recommendation above is superseded: with
the loads fused, the 20-CTA no-split grid wins p50 AND tail in this
window (v4 no-split p90 135.7 < split=4 p90 152.4). The estimate
(72.5 → ~60, -17%) scoped the win correctly (-13% measured, same-session).
