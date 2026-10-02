# 5a — the fused online j loop (design banked, next session's move)

**2026-10-01, end of session.** The pass-split probe (`9f97c72b`)
measured the case: the dot is computed TWICE (full = 255.1 p50,
pass-B-only = 124.8, pass-A-only = 77.3). The online form — ONE sweep
(dot, running max + rescale, p, l, acc) — computes the dot once,
projected ~125-140 µs. `config_tuning.rs` already carries the
`ptx_deferred_online` knob (uncommitted — land it first).

## The measured evidence

| variant | p10 | p50 | p90 |
|---|---|---|---|
| full (both passes) | 252.8 | 255.1 | 302.6 |
| pass B only | 124.1 | 124.8 | 128.1 |
| pass A only | 76.8 | 77.3 | 78.7 |

Rescale counter-cost: negligible (~ln(4096) ≈ 8 max updates; the
128-float acc is smem-resident in smacc[16384]).

## The design (validated against the pass structure)

Per-warp online softmax — each warp's j slice produces a partial
(m_w, l_w, acc_w) with its OWN running max; the merge rescales:

1. j loop (reuses pass B's machinery: dot_b_body strips + bfly +
   acc_stmts): dot → bfly → z_w; `if z_w > m: f = exp(m−z);
   a_strips ×= f; l ×= f; m = z_w`; `p = exp(z_w − m)` (1 when the
   rescale fired); `l += p`; acc strips += p·v.
2. redm[warp] = m (redm smem exists).
3. Merge with rescale: m_glob = max redm; l_tot = Σ redl[w]·exp(
   redm[w]−m_glob); acc[d] = Σ smacc[w][d]·exp(redm[w]−m_glob). The
   exp per warp is 2 mul + ex2 — 32×(cheap).
4. The split-store / normalize tail unchanged (l broadcast; the split
   partial-store path takes m/l/acc as before — the per-warp m is
   already merged by then).

The p_expr/m_name/l_rhs machinery is reused verbatim (pass B's
register bindings). The rescale branch is warp-uniform (z_w and m are
warp-uniform post-butterfly) — no divergence.

## Session postmortem (why this is a plan and not a diff)

Two hand-splice attempts into the 900-line `emit_deferred_region`
corrupted the file (label/register scoping: pass A's `lab`/`saved_j`
declarations ended up inside the gated region while their uses sat
outside; the else-wrapper brace accounting fought the split/normalize
branches). The file was restored from HEAD twice; the working tree is
clean. **Lesson (rule-13 adjacent): a >100-line splice into a function
with pre-existing internal branches is generator work that needs its
own session and a full-function reconstruction, not a diff.**

## Next session

1. Land `config_tuning.rs` (the online knob — dirty, verified).
2. Reconstruct `emit_deferred_region` as: an `online` early branch
   calling a NEW self-contained `emit_deferred_region_online(...)`
   (own labels, own merges, own split-store/normalize tail — duplicating
   the ~80-line tail is cheaper than splicing), then the existing
   two-pass path untouched. The new function is written as a WHOLE via
   the Write tool, never spliced.
3. Gates: m3 harness (both lanes) + softmax fixtures + suite; timing
   via the split sweep at the decode geometry; flip
   `ptx_deferred_online` default if it wins.
