# Followup stages — the post-metaprogramming queue

**Date:** 2026-09-24
**Status:** active — umbrella plan for every stage after the unified
metaprogramming layer (C1–C6 done, residue cleaned, tree green at `159a8dc9`)
**Refines:** `docs/plans/2026-09-20-metaprogrammed-composites.md` (Front D),
`docs/plans/2026-09-21-cross-dialect-interop.md` (waves),
`docs/plans/2026-09-20-gpu-dialect-beyond-cuda.md` (M/S ladder)
**Doctrine:** `docs/architecture/proof-vs-shape.md`

## Exclusion (standing)

`feat/e14a-intent-synthesis` (`briev-e14a` worktree) is LIVE foreign work —
another agent's `.ebv` domain, 46 commits ahead, dirty tree. Never merge,
never touch from these stages. All baselines and gates measure **main only**.

## Execution order

1 → 2 → 3 → 4 sequentially (2–4 are one plan's waves); then 5 re-ranks the
GPU performance backlog.

---

## Stage 1 — Front D: the deferred emitter retires

Formal milestone (`metaprogrammed-composites.md:92`). The backend's
deferred-region structural matcher is a loan; repay it.

**Mechanism.** The composite 1-launch path already matches the chain on
correctness and timing (both lanes PASS; 198–200 µs vs 202 µs —
`benchmarks/results/2026-09-21-composite-gates-both-lanes.md:11,24`).
The remaining question: does the plain general emission path (no structural
matcher) hold that number, or is the 198 µs carried by the matcher?

**Procedure.**

1. Fresh full baseline at current commit (Rule 12):
   `cargo build --release && bash benchmarks/build_and_bench.sh --runtime`
   + `bash benchmarks/compare_baseline.sh front-d-before`.
2. A/B: set `ptx_deferred_region: 0` in `config/ir-lowering.dbvl:115`
   (vs `1`), interleave reference/experiment ×N at the m3 gate geometry
   (`TEMPLATE=examples/gpu/attention_decode_composite.abv
   bash benchmarks/m3_attention_harness.sh 4096`), both lanes, correctness
   gate `max_rel < 1e-3` BEFORE any timing claim. Interleave timings
   (`LC_ALL=C /usr/bin/time -f "%e"`) ×N; record averages.
3. **Gate:** plain general path ≥ deferred timing − 10%, both correct,
   at m3 gate geometry.

**If PASS (retire):**

- Delete `general::has_deferred_region` (`src/backend/ptx/general.rs:3220`)
  + the deferred emission branch (`src/backend/ptx/mod.rs:1799-1812`).
- Remove the `ptx_deferred_region` knob (`config/ir-lowering.dbvl`,
  `src/config_tuning.rs:239,351,612-615`).
- SPIR-V lane: repeat the same A/B if a deferred counterpart exists there;
  retire symmetrically (Rule 2, additive audit of both backends).
- Device correctness at a REAL shape (gemm_h_bench 4096³ or m3 attention
  4096) on-device both lanes — timing-only verification is forbidden
  (AGENTS kernel-index gate lesson).

**If FAIL (rebuild, never re-add):** keep the matcher, record WHY the
general path loses (verified IR property, actual linked binary —
`llc -O2` inspection is NOT the `-O3 -flto` pipeline), derive the
principled version from current analysis (LoopShape, node_decompose,
CastingGraph), experiment on the actual `.ll`/`.ptx` before building.
Refuted hypothesis blocks the fix (Rule 20).

**Documentation:** this plan (status line), `metaprogrammed-composites.md`
Front D section, `proof-vs-shape.md` ledger if a matcher retires,
`benchmarks/results/YYYY-MM-DD-front-d-ab.md` (full protocol + numbers),
config docs if knob removed. Committed in the same change as the code.

---

## Stage 2 — Cross-dialect interop Wave 1: provenance + first edge

Plan of record: `docs/plans/2026-09-21-cross-dialect-interop.md:104-111`.

1. **Import provenance.** Resolver classifies resolved imports by dialect
   (`classify()` exists in `src/conformance.rs`); tags spliced items with
   per-module `SourceKind` — the missing data structure. Nothing per-module
   survives the splice today.
2. **Per-module extension-keyed semantics.** Accel default, profiles,
   prelude filtering apply per imported module, not root-only.
3. **Stale diagnostic.** Error text claims `{bv,ebv}` search
   (`src/import_resolver.rs:861-863`) but the resolver tries `.bv` only
   (`:800-830`). Either implement the `.ebv` candidate or correct the text —
   never both wrong ways; diagnostic must state what is searched, why, fix.
4. **Name-collision rule.** `.bv` and `.abv` both exporting `kernel` is an
   ERROR (or forces aliasing) — silent last-wins across dialects is a
   correctness trap. Tests for both outcomes.
5. **Formalize `.rbv`→`.bv`** as the FIRST declared edge (view bindings +
   write-contract routing exist informally).

**Gates:** provenance survives a two-level import chain; extension
semantics resolve per imported module; collision produces the diagnostic;
corpus example per new behavior (conformance sweep picks up `examples/`
automatically).

**Documentation:** wave-1 status in the interop plan, `glue-ffi.md`,
SPEC §7 import section, `errors.rs` house style for new diagnostics.

---

## Stage 3 — Cross-dialect interop Wave 2: static pair + runtime pairs

Plan of record: interop plan `:112-113`.

1. **`.sbv`→`.ebv` projection** (headline case): silicon module →
   electronics COMPONENT; ports → pins with direction map (output port
   NEVER projects to an input pin); port contracts → electrical
   constraints (`reference`/`tolerance` clauses exist in `.ebv` Volt);
   emits hierarchical KiCad symbol/sheet. Both dialects static, neither
   executes — purest pair, no runtime semantics invented (doctrine §3
   "Forbidden, named loudly").
2. **Declare `.bv`↔`.abv`** (accel mixed lane exists — kernel↔state,
   `.abv` GPU-only charter): make the implicit edge explicit.
3. **Declare `.bv`↔`.sbv`** (MMIO `@addr` on shared AST, board packs half
   exist): formalize port↔state-region.

**Gates:** direction-preservation tests per pair; capability intersection
validated per imported module (`src/backend/capabilities.rs` against
intersection(host surface, source surface)); obligation transport
(contracts cross the bridge, conjunction-checked) tested for each pair.

**Documentation:** interop plan per-pair status, `SPEC.md` (§7/§8 as the
pairs land), backend-contracts.md if a backend charter shifts.

---

## Stage 4 — Cross-dialect interop Wave 3: derivation

Plan of record: interop plan `:114-115`.

1. **Transitive bridges.** Adjacent pairs declared; distant pairs DERIVED
   by composing adjacent edges; direct edges that skip a runtime are
   REFUSED (refusal = explicit runtime topology — a feature).
2. **Synthesized bridge node** (user requirement, the artificial driver):
   a distant pair (`.rbv`→`.sbv`) is served by a compiler-SYNTHESIZED
   bridge node. Constraints: it is a REAL node (reactor, schedulable,
   contract-capable); DISCLOSED (diagnostics + artifacts name it —
   `// synthesized: bridge rbv→sbv for 'fpga'`); DERIVED from adjacent
   edge tables, never authored — hand-written bridge remains available
   and the compiler stands down when the author writes the host segment
   (the compiler's optimum must be expressible in the language).
3. **Diagnostics** name the first failing hop with the concrete fix.

**Gates:** derivation terminates and is deterministic; refusal fires on a
skip-a-runtime import with the what/why/fix diagnostic; synthesized node
appears in artifacts with its disclosure tag; a hand-authored bridge
suppresses synthesis (test).

**Documentation:** interop plan wave 3, SPEC §7, `proof-vs-shape.md`
(the synthesized node is general lowering machinery, the edge tables are
declared/temporal).

---

## Stage 5 — re-rank session (GPU performance backlog)

Run a controlled re-rank AFTER stages 1–4, with a fresh full baseline
(Rule 12 + Rule 12b worktree A/B). Candidates, in current order:

| # | Stage | Prize | Dependency |
|---|-------|-------|------------|
| 5a | **B — attention 198→125 µs** | Closes the deferred-target row; retires the fused-attention family (~1400-line matcher) behind a composite-parity gate | Stage 1 done (deferred numbers settled) |
| 5b | **S3b — cp.async GEMM pipeline** | 4096³ 23→38 TF; unlocks S4 correctness + S5 perf + S6 auto-tune | S3a proven (mma.sync exact) |
| 5c | **Warp-slice threshold retirement** | `has_warp_slice` (`general.rs:319`) span≥512 / span%4==0 are TUNING heuristics → `config/targets.toml` or composite params; hardware facts (warp=32, `shfl`) stay in the backend | none — mechanical |
| 5d | **M4 ladder remainder** | `detect_row_softmax` → `detect_reduction` → `GemmPlan` retire in that order, each behind a perf A/B gate (`gpu-dialect-beyond-cuda.md:204-211`) | 5a first (fused-attention already at table row 1) |

Re-rank outputs a single committed plan for the chosen stage (this umbrella
plan's stage 5 entry points at it).

---

## Standing gates (every stage)

- `cargo test --lib` green per commit (known-flaky only:
  `accel_rt::self_test::pack_math_and_launch_roundtrip`).
- `cargo build` — no new warnings.
- Praetor on changed dirs: `praetor validate --warn --target <dir>` — no NEW
  diagnostics in changed files.
- Kani harnesses for new safety-critical code.
- GPU stages: on-device correctness at a real shape, BOTH lanes, before
  any timing claim.
- Docs in the same commit as structural changes (Rule 13); timestamped
  records never retroactively edited (Working Rules §5).
- Never `git checkout --` / `git restore` / `git stash` (Rule 8).
