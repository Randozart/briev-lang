# Fused F16 Decode-Attention Node — Plan & Rationale

**Date:** 2026-09-18
**Status:** Active — P0 starting
**Parent:** `docs/plans/2026-09-18-m3-matrix-and-m4-fattn-shim.md` (M4 gate:
honest negative), CyberLlama plan `37ab73093`, before-ledger
`benchmarks/results/2026-09-17-cyberllama-before-ledger.md`

---

## The measured problem

M4 microbench gate (2026-09-18, bitnet decode D=128 H=20 HKV=5 NKV=4096,
CUDA lane): the 3-kernel composition runs **412.8 µs** at kernel parity vs
stock ggml fattn **58.3–59.6 µs** — ~7×. No win ⇒ no fattn.cu wiring.

## Root cause — why the composition is slow, and why it exists anyway

The 3-node form is the execution model's honest expression: the reactor
runs contracting stages (`qk → softmax → pv`), nodes exchange data through
the state buffer, and each stage is independently contract-verifiable —
that separability is exactly why the M3 harness can report `s_err` vs
`o1_err` vs `a_err` and why the member-index bug was root-caused in hours.
The shape is dataflow-correct and bandwidth-naive. Its measured costs:

| Defect | Why the shape causes it | Measured cost |
|---|---|---|
| K read f32 + re-walked per head group | qk is its own full pass over state | 226.9 µs vs ~30 µs BW floor |
| V re-read per query head (GQA 4×) | pv is a second full pass | 145.9 µs |
| S and O1 materialized to state | separate nodes can only exchange through state arrays | ~1.3 MB extra + serialization |
| 3 launches, each fully synced | node boundaries | ~60–90 µs |
| f32 everywhere | fields declared `Float` | 2× every KV byte |

Total logical traffic ≈ 85 MB vs ggml's ~10.5 MB effective (one fused
kernel: s_j in registers, never stored; online max/sum in registers; V
applied immediately; f16 loads). The 7× is the shape's cost — not
tunable away, because S/O1-in-state, 3 launches, and re-walks are
structural to the decomposition.

## The fix — a general capability, not an attention feature

The fused decode form is ONE node: work item = head, lanes = D (32 lanes
× 4 d each, coalesced f16 loads), one `foreach j` loop with a per-
iteration cross-lane reduction:

```
foreach j in 0..NKV {
    p  = Σ_{lane's d} q[..] * K[j][..]      // coalesced Float16 loads
    s  = SubgroupFAdd#(p)                    // per-iteration lane reduce
    m2 = Max#(m, s*SCALE); sum = sum*Exp#(m-m2) + Exp#(s*SCALE-m2); m = m2
    acc = acc*Exp#(m-m2) + V[j][d_lane]*Exp#(s*SCALE-m2)   // loop-carried
}
a_out[..] = lane-reduce(acc) / sum           // epilogue — existing machinery
```

3 loop-carried scalars per lane, no S/O1, K/V read once, f16 KV.
Traffic floor: K 5.2 MB + V 5.2–21 MB (L2-dependent) → **~35–65 µs vs
ggml's 58 µs — parity is plausible**.

## Standing constraints (user directives, 2026-09-18)

1. **No language finetuning for this purpose.** P1's deliverable is the
   general capability *lane-reduction expressions in loop bodies* — same
   customer base as the existing epilogue reductions (matvec, conv sums,
   any online algorithm). No attention-specific match arms, no keywords,
   no shape detection.
2. **The fused kernel is an ordinary `.abv` node.** The compiler's only
   new job is lowering what the author writes.
3. **LLVM path untouched by construction.** Lane intrinsics exist only on
   device paths; interpreter + LLVM lower them with sequential semantics
   (`SubgroupFAdd#(x)` = plain sum over the lane dimension — the M2a
   precedent). The fused node lowers to LLVM as a plain nested loop; no
   AST→LLVM emission changes.
4. **The LLVM-optimal shape must not be accidentally ruined.** Every
   compiler-touching step runs the full gate: `cargo test --lib` (2273),
   M3 matrix both lanes, and the Rule 12b baseline A/B
   (`../briv-compiler-baseline`, `compare_baseline.sh`) on the GPU suite.
   Refresh the baseline worktree if stale before P1.

## Steps

| Step | Work | Gate |
|---|---|---|
| **P0** | `.abv`-only f16 swap (k/v → `Float16`, existing type: stdlib
`type Float16 : Float { spec MaxBits: 16 }`, loads widen via
`load_widened`, stores `OpFConvert`, PTX `cvt.f32.f16`): extend
`attn_instantiate.py --f16-kv`, harness f16 seed/reference (gate 1e-2 per
the original M3 f16 spec), microbench rerun. Plus: verify runtime-NKV
(`count_expr` from a state scalar) works today. | M3 both lanes; f16
number recorded |
| **P1** | Audit where lane intrinsics lower today (epilogue-only?);
additively support `SubgroupFAdd#`/`SubgroupBroadcast#` in loop-body
expression position on PTX (`emit_lane_intrinsic` reachable in loop
bodies) + SPIR-V (group ops with correct loop convergence); interpreter
semantics per M2a. Additive match arms only; `_ => None` fallthroughs
untouched. | 2273 tests + M3 matrix + **baseline A/B clean** |

**P1 sharpened (2026-09-18, post probe `12a45506`):** the nested
foreach-reduction shape already LOWERS CORRECTLY on both lanes
(probe_fused_shape.abv: worst_rel=3.63e-06) — the emitters are not the
blocker. P1 is the *lane-mapping capability*: the analysis detects
"serial outer foreach + inner reduction foreach" and the cooperative
emitters map the INNER loop across lanes with an in-loop cross-lane
combine (per outer iteration); serial fallback stays exactly as
proven. Serial alternatives are analytically dead: (h,d) work items
re-read all of K per item through a 3 MB L2; (h) work items need
D-wide acc state. P1 is therefore required for the fused kernel's
performance, and its scope is: (a) analysis detection of the nested
shape, (b) PTX: reuse emit_warp_reduce inside the outer loop body,
(c) SPIR-V: nested structured loop (begin/end_structured_loop
reusable), (d) in-loop SubgroupFAdd# on both — position-independent
PTX text, OpGroupNonUniform* needs no extra convergence on Subgroup
scope.
| **P2** | `examples/gpu/attention_decode_fused.abv` (one node), M3-style
correctness vs the same CPU reference, then the microbench gate vs
`test-backend-ops` rows | beat f16-3-kernel AND competitive with 58 µs |
| **P3** | Runtime NKV: fill any gaps found in P0's verify (llama's KV
grows per token) | harness at growing NKV |
| **P4** | M4: `BEST_FATTN_KERNEL_BRIEV` arm in fattn.cu (sm_86 + F16 KV +
D=128 decode only, stock fallback elsewhere, additive); KV append via
`briev_accel_push_ranges` (built, `e8b57230`) | llama.cpp graph-compute
kernel-level A/B |
| **P5** | M5: llama-bench matrix — bitnet tg32 p0/p1024/p4096 headline,
mellum tg32 p0 + pp512 must-not-move, vs before-ledger; append "after"
section | verdict rules as written |

P2's gate is the kill line: P4/P5 happen only on a pass. Each step lands
as its own commit.

## Risks

| Risk | Mitigation |
|---|---|
| In-loop group-op convergence semantics (SPIR-V needs scope/barrier care in loops) | Audit first; the cooperative softmax precedent (M2a) emits group machinery — extend, don't reinvent |
| f16 seed/host path surprises (elem_bytes=2 field tables, harness regexes assume 4) | P0 exists exactly to surface this before P1 |
| f16 accuracy vs the 1e-3 gate | Original M3 spec says 1e-2 for f16 KV — use it, report the actual number |
| Baseline worktree stale | Refresh before P1; Rule 12b experiment for any regression claim |
| ncu unavailable | P0 proceeds timing-only; DRAM traffic inferred from KV-sized sweeps |
