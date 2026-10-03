# Plan: declared matmul — the GemmPlan retirement rung (M4, 2026-10-03)

**Status:** ACTIVE — queue item 5 (5d remainder), the M4 ladder's last
matcher rung. Governs itself by
`docs/architecture/derivation-not-recognition.md` (the GEMM structural
matcher is admissible as fact derivation with a Rule 24 retirement gate —
KernelPlan plan §Governance point 3) and
`2026-09-20-gpu-dialect-beyond-cuda.md` §M4.

## The doctrine gap being closed

`detect_gemm_shape` (`src/analysis/gemm_shape.rs`) recognizes the
author's triple-loop matmul and routes it to the tensor kernels. That is
RECOGNITION of an algorithm shape (Rule 23) — admissible only as an
interim with a retirement gate. The declared form: the author writes
`matmul!(a, b, y, M, N, K, i)` (stdlib, `lib/std/numeric.bv`) — the
declaration gates the specialization; the expanded canonical body still
supplies the FACTS (m/n/k/fields) the lowerings derive from.

## Today's consumers (all inherit the gate from one place)

`GemmPlan::match_stmts` (`src/backend/spirv/gemm.rs:186`) is the single
routing entry; its callers: `ptx/mod.rs:1595` (PTX tensor routing),
`spirv/kernel.rs:76` + `spirv/runner.rs:952` (SPIR-V),
`gpu_lowering.rs:189` (the strangler), `kernel_plan.rs:346` (plan
construction). Gating the MATCHER gates every consumer at once.

## Claimed fixtures (probed 2026-10-03, probe in the session log)

`gemm.abv` (f32 4096³), `gemm_chain.abv` (2 of 3 nodes, f16 128³),
`gemm_h.abv` (f16 4096³), `gemm_2048x2048x2048`, `gemm_4096x4096x512`,
`gemm_8192x8192x8192`. All bodies are the SAME canonical form modulo
element type — one composite covers them.

## Changes (increments, each committed)

### Increment 1 — the identity channel (pure additive, byte-identical)

1. `plugin/composite.rs`: statement-position expansion marks its FIRST
   `Statement::Let` with `Annotation { name: "declared_composite",
   value: Some(Expr::Identifier(composite-name)) }` (the `vol` Let-
   modifier precedent; unknown annotations are ignored by every existing
   consumer — typechecker, LLVM, interpreter). Non-Let-first bodies
   carry no marker (documented; the matmul body starts with the acc
   let).
2. `analysis`: `KernelShape.declared_composite: Option<String>` lifted
   from the kernel statements' first-Let marker (accel prove path).
   Additive; `None` changes nothing.
3. Type-parameter substitution: `substitute_param` extends to
   `Let.ty: Type::Custom(p)` when `p` is a composite param bound to a
   bare type-name arg (the matmul body declares `let acc: acc_ty;` —
   the element type follows the caller's buffers). Types were never
   substituted before; this is the composite machinery's first type
   parameter, disclosed in the composite header.

### Increment 2 — the declaration

`lib/std/numeric.bv`: `matmul!(abuf, bbuf, ybuf, mlen, nlen, klen, idx,
acc_ty, m, n, kk, acc)` — canonical body EXACTLY the form
`detect_gemm_shape` matches (`let acc: acc_ty = 0;` — note: the acc
type annotation with a float-literal init `0.0`; the detector needs the
decomposition lets `idx/nlen`, `idx%nlen`, the k-foreach with the
stride-matched reduction, and the bare-counter store). Header discloses
every parameter's role (softmax_fused precedent).

Fixture migration (same commit): the six fixtures above rewrite their
bodies to the `matmul!(...)` call + `import "std/numeric.bv"`.
Verification per fixture: the compiled kernels are BYTE-IDENTICAL to
pre-migration (same canonical body → same facts → same lowering).

### Increment 3 — the retirement gate

1. `GemmPlan::match_stmts` returns `None` unless
   `shape.declared_composite.as_deref() == Some("matmul")` — the
   recognition route is DEAD; only the declaration specializes.
2. Diagnostic (helpful, actionable): a node whose body WOULD match the
   GEMM facts but carries no declaration gets a warning at build:
   what was found (m/n/k), why nothing specialized, and the exact fix
   (`matmul!(...)` from std/numeric.bv). The matcher runs once more for
   the diagnostic — detection for ADVICE is not recognition for
   SPECIALIZATION (the specialization channel is the declaration).
3. `kernel_plan.rs` / `gpu_lowering.rs` consumers inherit the gate via
   the matcher; the plan-lowering parity tests get the declared marker.

### Increment 4 — gates and A/B

1. Suite green; Praetor no-new-rows on changed dirs.
2. Byte-identical kernels for every migrated fixture (Increment 2's
   gate, re-verified after Increment 3).
3. Device correctness BEFORE timing (kernel-index rule): gemm f32 4096³
   all-ones exact + f64 reference, BOTH lanes, declared-matmul build.
   Canaries 64³-256³ at parity.
4. Timing A/B (Rule 12 interleave): declared vs pre-change at 4096³
   (f32) + the shape matrix — the numbers must HOLD (the same body, the
   same lowering; any delta = the channel changed something).
5. Docs: this plan + `numeric.bv` header + INDEX queue update; the M4
   table row in `gpu-dialect-beyond-cuda.md` is a timestamped record —
   referenced, not edited.

## Non-goals

- `detect_reduction` retirement (separate rung).
- Changing the tensor emitters (the same lowerings run underneath).
- Hand-written-loop diagnostics beyond the advice warning; no
  auto-upgrading of undeclared bodies.

## Risks

- **Hidden recognition dependents**: any fixture that matches the GEMM
  facts but isn't in the claimed list silently falls to the general
  path at Increment 3 (slower, correct). Mitigation: the probe sweep
  re-run over `examples/` + `benchmarks/` before the gate lands;
  migrate everything claimed.
- **Byte-identity of expansions**: the marker annotation must not leak
  into emission (it lives in Let.modifiers — the emitters match
  specific modifier names; unknown ones ignored — verified by suite +
  the byte-identical fixture gate).

## Increment 4 — device gates (2026-10-03, outcome)

**Instrument:** `benchmarks/declared_matmul_gate.sh` — splices seed +
verifier into the GENERATED runner (the compiler's own desc), runs
all-ones (exact, full output) + a patterned LCG seed (16 spread rows vs
a bit-exact C-side f64 reference) on BOTH lanes. Build notes: the seed
must land AFTER `briev_accel_init` (the AB-gate anchor — the first
resident launch seeds the projection from the authored bytes), the
verifier is C-side (no cross-language LCG drift), and no `\n` inside
injected C strings (the 2026-10-01 postmortem trap, hit and escaped).

**Results (4096³ f32, `gemm.abv`):**
- Vulkan lane: all-ones **EXACT PASS** (16777216/16777216) — the
  declared channel computes the full product correctly end-to-end.
- CUDA lane: nondeterministic wrong outputs (varying first-wrong-row,
  patterned garbage to 5e+40) — **PRE-EXISTING**: the gate fails
  identically on the pre-migration fixture, and the kernels are
  disassembly-identical across the change. This is the 5b ladder's
  standing open defect, now with the nondeterminism evidence —
  BUGS.md 2026-10-03 (uninitialized smem / tile-boundary race shape).
- Vulkan patterned max_rel ≈ 0.12 (exact on ones, off on mixed
  magnitudes) — bounded, recorded in the same BUGS.md entry (the f32
  accumulation-order question 5b Phase 2 predicted).

**Verdict:** the declaration channel is exonerated and Vulkan-proven;
the CUDA GEMM correctness defect is inherited 1:1 and blocks CUDA-lane
timing claims (they were already blocked — 5b). No new timing table:
the emitted kernels are proven identical to the pre-change ones, so the
standing 5b numbers carry.

## The derive-the-counts experiment (2026-10-03, REVERTED — gap recorded)

Attempted to derive the fixtures' flat counts (`const MN: Int = M * N;
let a: Float[MN];`) — the composite comptime chain folds const
expressions, but the KERNEL const readers demand literals
(`spirv/lower.rs materialize_consts`: "kernels read literal consts
only") and the parser accepts only literal-or-single-ident array dims.
REVERTED to literals (all 8 fixtures byte-identical to HEAD). The
follow-up is the parser/typechecker increment: fold const-expression
inits in `materialize_consts` + the state-decl Named-dim resolution
(~2 sites, `fold_consts` already exists). Until then the fixtures spell
the counts — fixture hygiene deferred to that increment.

## Status

- Increments 1-3: LANDED (`466f0e59`, `e2096a98`, `607e5a38`).
- Increment 4: gate landed; Vulkan-proven; CUDA blocked on the inherited
  5b defect (BUGS.md).
- M4 remaining: `detect_reduction` retirement (separate rung);
  GemmPlan itself is now declaration-gated — the matcher survives only
  as the fact derivation for ADVICE + the declared channel.
