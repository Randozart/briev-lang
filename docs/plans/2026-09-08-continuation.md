# Continuation Plan — Phase 7 (PTX) + Phase 8 (Housekeeping)

**Date:** 2026-09-08
**Depends on:** `docs/plans/2026-09-08-master-workstream.md` (Phases 1-6 COMPLETE)

---

## Phase 8: Housekeeping (quick wins, parallel)

### 8a — `brievc run x.abv` native subcommand

**Currently:** 6-line shell wrapper (build → cc runner → exec).
**Target:** Native `brievc run <file.abv>` subcommand.

**Steps:**
1. Add `Run` variant to the CLI arg enum in `src/main.rs`
2. In the run handler: invoke `build` logic in-process (reuse existing build path)
3. Locate the generated `_runner.c` file
4. Compile with `cc -O2 -I <runtime_dir> <runner.c> -o <tmp_binary> -lm -lvulkan`
5. Exec the binary, forward exit code

**Files:** `src/main.rs` (CLI dispatch), possibly `src/backend/spirv/kernel.rs` (runner gen)
**Gate:** `brievc run examples/gpu/gemm_4096x4096x512.abv` produces same output as manual shell flow

### 8b — Protocol proof codec bodies

**Blocked on:** 6 codec bodies in stdlib `.bv` + trusted-axiom cast-edge marker.
**Scope:** Implement `ascii_to_utf8`/`utf8_to_ascii`, `utf16_to_utf8`/`utf8_to_utf16`, `Posit32_to_IEEE754`/`IEEE754_to_Posit32` as stdlib functions.
**Then:** Flip `protocol_graph.rs` missing-body skip arm to hard error.

**Files:** `lib/std/` (new `.bv` files), `src/protocol_graph.rs`, `spec/SPEC.md` §8.7
**Gate:** `cargo test --lib` green, all protocol round-trips pass

### 8c — Resident-launch policy

**Gate:** "all readers of a resident array are kernels" — analysis check before emitting resident-launch wrapper.
**Scope:** `.bv` offload path only (not `.abv` — `.abv` always launches resident).

**Files:** `src/backend/spirv/kernel.rs` (wrapper emission), `src/analysis/` (resident-readers check)
**Gate:** Correctness proof: host state never goes stale

### 8d — Old plan file review/closure

~150 plan files, ~35 complete, ~55 active, ~150 with no status. Sweep and mark CLOSED/SUPERSEDED where the work is done.

---

## Phase 7: PTX Tier (3-5 sessions, high effort)

From `docs/plans/2026-09-04-beyond-coopmat.md` Stage 2. Only if 42.0 TFLOP/s target matters.

| Sub | Task | Files | Gate |
|-----|------|-------|------|
| S1 | CUDA driver module | `lib/runtime/briev_dev_cuda.c` | Probe-driven selection alongside Vulkan |
| S2 | PTX emitter | `src/backend/ptx/`, `capabilities.rs` | `--backend ptx` |
| S3 | Tensor GEMM: mma.sync.m16n8k16, ldmatrix, cp.async | S2 | Per-op microtests |
| S4 | Correctness gate | Shape portfolio, 5e-3/1e-2 tolerances |
| S5 | Performance gate: 42.0 TFLOP/s at 4096³ | Ledger row |
| S6 | Auto-tune loop | `derive --stochastic` |
| Docs | backend-contracts, HANDOFF, SPEC §9.8, AGENTS.md | Same commit as S2 |

---

## Execution Order

1. **8a — `brievc run` subcommand** (1-2 hours, quick win, improves DX)
2. **8b — Protocol proof codecs** (1 session, correctness gate)
3. **8c — Resident-launch policy** (1 session, correctness gate)
4. **8d — Plan file sweep** (parallel, low priority)
5. **Phase 7 — PTX tier** (3-5 sessions, only if needed)
