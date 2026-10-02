# B1 --runtime re-run after the 5b Phase 3 default change — 2026-10-01

**Gate discharge.** The 5b Phase 3 model gating (`3ff5ed49` — the
DRAM-bound wide-tile default for large deep-K GEMMs) postdates the B1
baseline (`6dca75b0`), and the campaign's standing gate requires a full
`--runtime` table after any default change. This run also covers the
~66 commits between (atomics family, the `.bad` GPU escape + site
blocks, D14 modifiers, the A1/A2/D5 daily-use fixes).

- Harness: `bash benchmarks/build_and_bench.sh --runtime` then
  `--optimizer`. Machine/driver unchanged (RTX 3060, 615.71.09).
- Commit: `5ac1ee1c`.

## Result

**39/39 rows MATCH/PASS — zero correctness regressions.** Optimizer
precomputes intact (iir_filter, precompute_sum, const_heavy,
async_counters_idio all precomputed; UTF8_ops SKIP unchanged).

Throughput deltas vs B1 are within the documented session-band hazard
(`2026-09-30-5b-structural-fill.md`: fine A/B unreliable in long
sessions). The one apparent mover — nbody_sqrt 2.37s (B1) → 3.01s —
was checked against the second reference
(`2026-09-25-front-d-before.md`: **3.407s** same machine/driver): the
current 3.01s sits mid-band, i.e. the B1 number was the clock-state
outlier, not a regression. All other rows moved within noise (|Δ| ≤
the C side's own drift).

## Standing gate status

- B1 `--runtime` table after the 5b default change: **discharged**
  (this file).
- Next 5b step (unchanged): Phase 3 winners-only remainder — the
  load-path axis (smem vs register-staged per shape × device), behind
  its own Rule 20 experiment.
