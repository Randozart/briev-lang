# Shallow-K tip re-verification + Vulkan f16 GEMM fill-mask defect (2026-10-03)

Plan: `docs/plans/2026-10-03-shallow-k-tip-verification-and-ledger-truth.md`
(adopted as written). Scope: prove the 2026-09-16 shallow-K race fix still
holds at tip, then the BUGS/INDEX ledger truth pass. Phase 1 found a REAL,
unrelated correctness defect on the Vulkan f16 coopmat path; it is fixed in
the same session (BUGS.md, last entry). No timing claims in this file —
verification only.

## Phase 0 — dispatch ground truth

- The mw tensor tier is f16-ONLY: `let tensor = a_elem == 2 && …`
  (src/backend/ptx/mod.rs:1120). f32 GEMM lowers to `naive_gemm_ptx`
  (flat, race-free). The historical "f32 ref" in the shallow-K entry was
  the reference operand, not the compute operand.
- The shallow-K fix is present at tip: `wait_depth = if k <= 128 { 0 }
  else { stages.saturating_sub(2) }` (src/backend/ptx/tensor.rs:1762).
  Emitted PTX for gemm_h 1024²×64: 562 lines, 16 mma.sync, 8 ldmatrix,
  `cp.async.wait_group 0` ×2. Gate `shallow_k_race`/`mw_kernel_ok`:
  zero references remain in src/ and benchmarks/.

## Phase 1 — verification protocol

Fixtures: `examples/gpu/.tmp_shallow/gemm_h_<MxNxK>.abv` (7 shapes:
512²×64, 512²×128, 1024²×64, 1024²×128, 2048²×128, 4096²×64, 4096²×128;
f16 declared matmul; temp — never committed).

Gate: `benchmarks/declared_matmul_gate.sh` extended with
- mode 1 (patterned f16): seeds mirror `benchmarks/gpu/gemm_h_bench.c`
  — `a[i] = (i%7)*0.25`, `b[i] = (i%5)*0.5` as f16; 16 spread sampled
  rows (`m=(r*7919)%M`), per-element rel vs a C-side f64 reference;
  tol 5e-3 (1e-2 under `BRIEV_GEMM_F16ACC`; `ptx_tensor_f16acc` default
  false); WORST-cell + bad-count diagnostics.
- mode 2 (index/count probe): a constant per row
  (`((m%7)+1)*0.25`), b constant per column (`((n%5)+1)*0.5`);
  correct kernel ⇒ `y = K·α(m)·β(n)` exactly; any pitch/transpose/
  pairing error decodes to the index the kernel actually used.
  `GATE_F16=1 GATE_MODE=2` on the gate binary; run-length decode of bad
  column ranges per sampled row.

### Finding (before fix) — Vulkan-only, f16-only, deterministic

| probe | CUDA | Vulkan |
|---|---|---|
| mode 0 (all-ones), 7 shapes ×3 | EXACT | EXACT |
| mode 1 (patterned), 1024²×64 | 0.000e+00 | **8.889e-02 FAIL** (7376/16384 cells; worst y[956,35]=46.125 ref=50.625) |
| mode 1, 512²×64 | 0.000e+00 | **6.914e-02 FAIL** |
| mode 2 (probe), 1024²×64 | 0 mismatches | **8192/16384** |

Probe decode: every row, columns [0,32) of each 64-col tile exact;
[32,64) equal to column n−32 (−32 ≡ +3 mod 5 on the mod-5 classes);
term count intact (64); a rows intact. RLE (16 bad ranges of 32 per
row, first=32, last=1023). f32 control (gemm.abv 4096³) patterned
exact on BOTH lanes (2.613e-06) → f16-specific; CUDA exact →
SPIR-V-specific. All-ones blind (row-const a × ones-b cannot see
which B column is read) — the pre-existing corpus could never catch
this. f16 accumulation mathematically ruled out (dyadic eighth-multiple
seeds are exact in f16 regardless of order). Not a race (deterministic
3/3).

### Root cause + fix

`emit_fill_pair_dram` (src/backend/spirv/gemm.rs) masked the B DRAM
source index — HALF units (`flat2 − a_stage_elems`) — with
`b_stage_pairs_mask` (= b_stage_elems/2 − 1 = 511, a PAIR-unit mask):
b_flat ≥ 512 wrapped to 0, b_tile_idx collapsed {0..3}→{0..1}, so B
columns 32..63 of every 64-col tile loaded from columns 0..31. The
function's own comment ("j in 0..3") contradicted its mask. Every
sibling path (quad fill, scalar fill, pair smem dest) already used
`b_stage_elems_mask` (1023). Fix: use `b_stage_elems_mask`; dead
`b_stage_pairs_mask` removed. CUDA/PTX unaffected (different codegen).

### After fix — full claim matrix (all EXACT)

7 shapes × 3 runs × both lanes (cuda, vulkan) × modes {0,1}:

| shape | all-ones | patterned (both lanes, 3 runs each) |
|---|---|---|
| 512²×64 | EXACT 262144/262144 | max_rel 0.000e+00 PASS |
| 512²×128 | EXACT 262144/262144 | 0.000e+00 PASS |
| 1024²×64 | EXACT 1048576/1048576 | 0.000e+00 PASS |
| 1024²×128 | EXACT 1048576/1048576 | 0.000e+00 PASS |
| 2048²×128 | EXACT 4194304/4194304 | 0.000e+00 PASS |
| 4096²×64 | EXACT 16777216/16777216 | 0.000e+00 PASS |
| 4096²×128 | EXACT 16777216/16777216 | 0.000e+00 PASS |

Probe (mode 2) after fix: 0 mismatches, both lanes. `cargo test --lib`
2855 green. Praetor: identical diagnostic set vs HEAD (no new).

## Ledger truth pass (same session)

BUGS.md: 6568 + 6651 headers corrected to their in-body resolutions
(2026-09-16); 7662 marked DUPLICATE of 7602; 7687 marked SUPERSEDED by
7627 (harness corruption); new root-caused+fixed entry for the fill-mask
defect. INDEX: shallow-K lines updated (was "OPEN, correctness-gated");
div-slowpath queue line marked SIZED+CLOSED 2026-10-02
(`2026-09-30-5a-attention-decode.md`).

## Open follow-ups

- 6707 (prime full-upload clobber, OPEN): needs a repro at the prime
  shape before closure; mitigation `02418f49`
  (src/accel_rt.rs snapshot/restore) still in place.
- CUDA 13.4 cuMemcpy2D driver issue (6740) stays OPEN (external).
- GPU re-rank (Workstream 3) — the fill-pipeline campaign
  (`2026-09-30-stage5b-structural-fill-campaign.md`) is the next big
  absolute prize; the patterned gate is now a permanent lane check for it.

## ADDENDUM 2026-10-03 (later session): the "4.5% = f16acc" attribution VERIFIED at 4096³

A/B with the pre-fix compiler (worktree at `37772189`, today's gate):
4096³ patterned16 max_rel = **4.436e-03 on ALL THREE** (pre-fix Vulkan,
post-fix Vulkan, CUDA) — the fill-mask defect's residual at 4096³ on the
periodic seeds is INVISIBLE: the product period of the seeds is 35 and
4096 = 117·35 + 1, so full periods sum phase-independently (CRT) and the
mask's −32 column substitution leaves ONE boundary term (≤ 5.7e-4 rel)
under the f16acc rounding floor (4.4e-3 at y≈3072, ulp 2). At K=64 the
partial period is 29/35 terms → residual 8.9e-2, plainly visible.
Conclusion: the 2026-09-30 record's f16acc attribution at 4096³ is
CORRECT and stands; the mask bug's manifestation is small-K (K not near
a multiple of 35), which is exactly where the all-ones corpus was blind.
No retraction needed. The 4.436e-03 number itself is the f16acc
parallel-order rounding floor on periodic data (post-fix Vulkan == CUDA
bitwise — a strong cross-lane equivalence check to keep in the gate).
