# Gate hardening + variant-diff verification protocol

**2026-10-01.** Status: **active, this commit.** Origin: the fused
online kernel shipped with three compounding verification failures —
missing `redl`/`smacc` stores, the early return skipping the
split-store/normalize tail, and a NaN-blind error metric — caught only
by a manual probe contradicting the gates (got=0 vs "0.00e+00 PASS").
Failure chain + fixes in
`benchmarks/results/2026-09-30-5a-attention-decode.md` (VERIFICATION
CORRECTION section). Companions: `handoff-methodology.md` (P4 rule),
the 5b campaign doc (look-item precedent).

## The failure chain → the prevention map

| # | Failure link | Prevention |
|---|---|---|
| 1 | Whole-function replacement not behaviorally diffed vs its predecessor | **P1** variant-diff gate |
| 2 | A/B knob changed the kernel without the gates following (`softmax_gate.sh` ignores `BRIEFC_FLAGS`) | **P3** flags expansion |
| 3 | NaN-blind metric (`NaN > max_rel` is false → NaN outputs passed as 0.00e+00) | **P2** NaN hardening sweep |
| 4 | Timing headline (72.5 µs) landed before same-geometry correctness | **P4** protocol rule |
| 5 | Proven-kernel claims drifted from saved evidence | **P5** golden outputs |

## Milestones

- **P2 — NaN/inf hardening sweep** (trivial): every gate harness's
  error metric fails on NaN/inf. Done: `softmax_gate.sh`. Remaining:
  `m3_attention_harness.sh`, `atomic_gate.sh`, `workid_gate.sh`,
  `bad_ptx_gate.sh` — same fix (`isnan(err) → err = 1e300`), plus
  `isnan(got)` where the metric reads raw outputs.
- **P3 — flags expansion**: `softmax_gate.sh` expands `BRIEFC_FLAGS`
  like `m3_attention_harness.sh` already does. Kills the
  silent-default-path A/B trap.
- **P4 — protocol rule** (docs, same commit): in
  `handoff-methodology.md` + AGENTS working rules: **no timing number
  lands in a results file without a same-commit, same-geometry,
  same-config correctness gate.** Timing before proof is marked
  provisional in the results file itself.
- **P1 — variant-diff gate** (`deferred_ab_gate.sh D H HKV NKV`): the
  same fixture built twice (`ptx_deferred_online: 0` / `: 1` via
  config-dir), both run, output buffers compared **variant-vs-variant
  element-wise** (two paths wrong in different ways diverge; same-way
  wrong passes a reference check). Runs at the target geometry AND the
  s8 canary. This is also the decode-geometry investigation tool
  (open item: o1 = 0 — the diff pinpoints the first divergent element).
- **P5 — golden outputs for proven kernels**: each proven fixture's
  gate saves a golden output hash alongside the run; regeneration
  requires a passing live-reference check in the same commit (protocol
  in the gate header). Closes the "proven kernels saved" loop: source
  (examples/) + evidence (results/) + golden (hash).

## Gates

Suite green; the hardened gates all still PASS at their verified
geometries (s8 softmax both lanes, atomic, workid); the decode AB gate
run feeds the open decode-bug investigation. Docs same commit.
