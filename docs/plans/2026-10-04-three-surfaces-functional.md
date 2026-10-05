# Three functional surfaces — `.bv` / `.abv` / `.rbv` (2026-10-04)

**Goal (the finish line, stated once):** *a stranger can install Briev and get
real work done in all three surfaces.* Nothing in this plan is a marketing
goal; every item removes a concrete blocker a competent outsider would hit.

- **`.bv`** — install → run → import modules; CPU performance within 1% of
  baseline always; competitive vs C where claimed.
- **`.abv`** — GPU programs on both lanes (CUDA/PTX, Vulkan/SPIR-V),
  correctness-gated, cuBLAS-parity on the shapes users hit.
- **`.rbv`** — build a web program and load it in a browser.

**Explicitly not this plan:** 1.0/marketing, self-hosting, large ecosystem,
`.ebv`/`.sbv` productization (they get decision records, not freezes), new
backend surface beyond what a phase needs. Phase 4's public-face items are
the bridge from *usable* to *taken seriously* — the commitment ends at
"adoptable without broken corners or surprise slowdowns."

Origin: strategy discussion 2026-10-04 (this plan's predecessor sections
were inspected line-by-line before writing). Related context this session:
`benchmarks/results/2026-10-03-shallow-k-tip-verification.md` (fill-mask
defect fix `9da0c750`, f16acc verification `49f663e5`, provisional Vulkan
regression note `c3115156` — resolved by Phase 2.1 below).

## Governing rules (apply to every phase)

1. **Derivation, not recognition** (`docs/architecture/derivation-not-recognition.md`):
   the compiler derives; stdlib declares; keywords exist only for *proven
   uncertainty* or *required semantics*. Modifier admission requires a filed
   derivation gap (§ Phase 0.4). A keyword that only gains speed is a
   compiler bug (Golden Rule 2).
2. **Perf bar** — the definition of "no performance left on the table":
   - never >1% regression vs `../briv-compiler-baseline` on the B1
     `--runtime` suite + the GPU shape suite;
   - parity-or-better vs C/cuBLAS on the shapes users hit
     (512³–4096³ GEMM, attention decode); canaries 64³–256³ protected;
   - the correctness gate (all-ones + patterned + probe, both lanes —
     `benchmarks/declared_matmul_gate.sh` modes 0/1/2) before every timing.
3. **House discipline**: `cargo test --lib` green + Praetor no-new-
   diagnostics + docs updated in the same commit + continuous commits; Kani
   harnesses for safety-critical new code; BUGS.md for every diagnosed bug.

## The language invariant (one language, surface vocabulary allowed)

**Shared and invariant** (never forks): grammar *forms* (`node/txn [pre][post]`,
`{}`=grouping, `<>`=compile-time specialization, `[]`=bound, `let/const`,
contracts); the semantic core (reactor, contracts/proofs,
derivation-not-recognition); the rules *governing names* (the three-category
taxonomy — strategy/ambiguity/intrinsic — and the admission process).

**Surface-owned** (allowed to mean nothing elsewhere): `.abv` — `scope<…>`,
`shared`, shape modifiers, `Tensor<…>`, `Subgroup*#` intrinsics; `.rbv` —
`render`, the HTML layer, `b-text`/`b-trigger`.

**Checkability (the "clear mental model" test):**
1. every name is *locatable* to its surface (file surface or disclosure
   marker — `#` for intrinsics);
2. surface names never alter core semantics: stripping the surface parts of
   any program leaves valid `.bv` (the `.rbv`→`.bv` declared edge already
   guarantees this for `.rbv`);
3. one rulebook for new names — a web keyword goes through the same
   admission process as a GPU one;
4. one SPEC organized core → projections; the conformance sweep typechecks
   every active `.bv`/`.ebv`/`.abv`/`.sbv`/`.rbv` under one grammar.

**Stretch ladder (the honest grading):**

| Surface | Relation to core | Status |
|---|---|---|
| `.abv` | projection — same reactor/contracts, backend lowers to PTX/SPIR-V | Tier A — invariant applies as-is |
| `.rbv` | core + presentation layer via the declared edge | Tier A |
| `.ebv` | **execution-model stretch**: machine entry = authored reset, `node @ vector` = machine-vectored, MMIO `@addr` state, register shims, wake sets; reactor time becomes hardware time | Tier B — genuinely the weirdest syntax; own decision record required before any freeze claim touches it |
| `.sbv` | **backend-is-synthesis stretch**: lowering = hardware; timing/placement semantics have no `.bv` analogue | Tier B — deepest stretch |

Tier B surface semantics are admissible only as **required semantics from
the hardware's own model** (MMIO touching the register *is* the program;
machine-vectored entry *is* the machine's reset reality) — same rulebook,
different license class: never speed, never convenience. The Phase 0.5
surface register records, per Tier B surface, which core concepts it
redefines (entry/time/state/backend).

## Current state (verified 2026-10-04)

| Surface | Works | Gaps |
|---|---|---|
| `.bv` | full pipeline (parse→contracts→LLVM→run), FFI/GLUE, B1 39/39 MATCH vs baseline, suite 2855 | `json.bv` blocked on generic type inference + 3 language gaps (INDEX open-bugs, unscoped); package/module v0; stranger install path |
| `.abv` | both lanes (CUDA 27.5 TF @4096³; 64³–256³ beat cuBLAS); declared composites landed (`matmul!`/`dot!`/`softmax_rows!`); three-mode correctness gate permanent | 512³/4096³ behind cuBLAS (fill campaign); Vulkan lane −16-22% vs the 09-30 record (runtime-port suspect — Phase 2.1); Tier-2 matchers remaining |
| `.rbv` | parses/typechecks; obj/txn/render model; `.rbv`→`.bv` declared edge DONE (interop wave 1); webstack backend = wasm32 LLVM + view normalizer | no end-to-end artifact story (what runs a built `.rbv`); webstack-v2 completion state unprobed |

Promotion-sweep investigation findings (2026-10-04):
- `[*]` = `Expr::Wildcard` — parses generically (`src/parser/expressions.rs`),
  first-class AST, but **LLVM codegen panics** (`src/backend/llvm/emit_expr.rs:1851`);
  only the electronics expansion gives it semantics (SPEC 2026-09-28).
- Quantities — `Quantity` is a core AST type (`src/ast/types.rs:212`) with
  generic parser machinery (`src/parser/quantity.rs`), but the dimension enum
  is electrical-only (Volt/Amp/Ohm/Farad/Henry — no Seconds).
- `mode` (SPEC:353) — described generically ("exclusive operating states"),
  electronics-only consumers today (audit needed).
- `node @ vector`, `@addr`, `stdnet<>`, `unpop`, `pin in: Power`, `reference` —
  electronics-native / licensed stretch; stay `.ebv`-owned.

## Phase 0 — Syntax freeze set (est. 1-2 sessions)

- **0.1 D31 amendment** (`docs/architecture/gpu-syntax-decision-record.md`):
  DROP from the author surface: `fragment` (subsumed by `Tensor<elem,M,N,K>`,
  D21), `swizzle` (bank-conflict synthesis is derivable general machinery),
  `unroll` (pure speed keyword — Rule 2; LLVM/ptxas own it), `vector`
  (derivable from dtype × alignment × access). D14's vocabulary 7→2:
  `tile`, `stage`. DEFER: `irr` (D29 — until warning false-positives exist;
  `### warn:` policy is the interim), `persistent node` modifier (derived
  persistent stays), `split<N>` (until the reduction-split machinery proves
  a need). KEEP the load-bearing set: D2/D18 (observability spine), scope
  blocks + `shared` (D3/D5/D8/D10/D15/D16/D23), persistent-as-lifetime-axis
  (D6), `Tensor<>` (D11/D13/D21), `lemma_properties` (D17/D19/D20),
  megakernel fusion (D24/D25), `###` (D30, implemented), asm layers (D26/D27),
  A1-A8/A10.
- **0.2 Four open-item decisions**: §11.6 — rename the node-classification
  value to `sync<peer>` (barriers stay `sync<workgroup|subgroup|cluster|grid>`);
  §11.7 — declared `scope<…, size>` is a **hint** (the model may override
  with a remark, D2 pattern); §11.1 — shape modifiers ride the node for
  single-node kernels, the `scope<…>` for fused multi-node kernels; §11.2 —
  exact `lemma_properties` vocabulary decided at first consumer
  (data-driven registration + validation).
- **0.3 `Asm#` two-lane audit**: diff the intrinsic registry +
  `config/asm-lowering.dbvl` against PTX sm_80/86/90/100 + SPIR-V coverage;
  lane asymmetries = filed gaps, fixed in Phase 2.
- **0.4 Admission process**: any new shape/keyword modifier requires a
  filed derivation gap (coverage-ledger entry: *what derivation fails
  without it*), a taxonomy category, and a disclosure marker. The escape
  ladder for the clever engineer: `tile`/`stage` at shape level, body-level
  arithmetic (hand-rolled swizzles are 3 lines of plain indexing),
  `Asm#` + data rows at instruction level, `###` config at module level.
- **0.5 SPEC fold**: keyword taxonomy + stretch-graded surface register +
  freeze policy into `spec/SPEC.md`; tutorial + syntax-highlighter same
  commit if any surface syntax changes.
- **0.6 Promotion sweep**: (a) `[*]` backport — general array semantics;
  decided default: AST-level desugar to an explicit lift (expression lifted
  over the array, declaration-order index in scope) so interpreter and all
  backends get it from one implementation; electronics expansion subsumed;
  recorded as a D-item. (b) quantities: add the Time dimension (Seconds +
  SI prefixes) and make quantity literals legal on all surfaces; electrical
  dimensions retained. **DONE 2026-10-05**: Time dimension (`c1f1fc9f`);
  expression-position quantity literals parse + scale to SI on all
  consumers (BUGS.md 2026-10-05; `250mA`→0.25 on `.bv`, `check` OK on all
  five surfaces; grammar inventory corrected). (c) `mode` consumer audit → promote or keep
  electronics-owned. (d) mechanical inventory of remaining one-surface
  grammar forms → classification table (promote / surface-own /
  licensed-stretch) as the first output.
- Gate: SPEC + D31 land together; conformance sweep green; suite green.

## Phase 1 — `.bv` stranger-functional

1. Scoping read of `json.bv`'s blockers (generic type inference + the three
   language gaps — INDEX names but does not enumerate them), then implement.
   The generality test: a real stdlib module, not a toy.
   **IN PROGRESS 2026-10-05**: `lib/std/json.bv` migrated from the archive
   (typechecks; inert, not prelude-loaded). The migration surfaced SIX
   codegen/liveness defects — FIVE fixed (`734a8dac`: enum struct-payload
   match binding, bare tail match, List+List liveness, String cast-lane
   liveness, String indexing); TWO open: `list_concat` unimplemented and the
   Char cast lane mis-typing the value as the Data variant (`char_at`
   unusable). Both filed in BUGS.md. The generality test is doing its job.
2. Package/module v0: git-based registry; reuse macro-lock; `briev.toml`
   target profiles exist.
3. Install story: single static binary; `brievc run hello.bv` < 5 min on
   x86/ARM/mac; tested on a clean box, not on this one.
4. Housekeeping: BUGS 6707 (prime full-upload repro before closure;
   mitigation `02418f49` in place), ledger leftovers.
Gate: B1 MATCH + suite green per landing; hello-world from scratch works.

## Phase 2 — `.abv` perf credibility

> **Status 2026-10-04 (same-day):** Phase 2.1 RESOLVED — the Vulkan
> "regression" was the B-fill mask fix un-masking the true cost (the bug's
> 32-column collapse was an accidental 2× L2 reuse; the 09-30 record's
> 11.59 ms was measured on the buggy kernel). The quad fill
> (`spirv_coopmat_fill_quad`) landed as DEFAULT with a `plan.n >= 256`
> correctness guard: **4096³ Vulkan = 19.9 TF (was 9.5), 2.09×; exact ×3
> both lanes; canaries safe**. Bisect + measurements:
> `benchmarks/results/2026-10-03-shallow-k-tip-verification.md` ADDENDUM 3.
> NEW rung 0 discovered by the quad matrix: a pre-existing SPIR-V-only
> small-N defect (N ≤ 128 miscomputes; BUGS.md this session) — fix it,
> drop the guard, then the levers below.

1. **Small-N defect (rung 0, NEW)**: the SPIR-V tiled f16 kernel
   miscomputes at N ≤ 128 (zero rows; CUDA exact; pre-existing — BUGS.md).
   Fix + drop the quad guard + full matrix.
2. **cuBLAS parity**: the fill campaign
   (`2026-09-30-stage5b-structural-fill-campaign.md` — the correctness
   license for pipelined fills is already written), recalibrated from the
   19.9 TF post-quad baseline; re-rank queue (attention, warp-slice
   thresholds, M4 ladder).
3. **Vocabulary retirement**: every remaining Tier-2 recognized-vocabulary
   matcher → declared composites (each retirement = a generalisation
   proof, A10 ledger).
4. **Escape-ladder acceptance test**: a CUTLASS-class hand-tuned kernel
   (swizzled smem layout) expressed WITHOUT the dropped keywords —
   body arithmetic + `tile`/`stage` + `Tensor<>` — must compile, pass the
   scoped-sharing/alignment proofs, and hit the vendor kernel's numbers;
   the delta vs the derived swizzle is recorded in the retirement ledger.
   Failures = derivation gaps → admissions. Runs in parallel with 2; its
   outcomes refine 0.1's freeze set.
Gate: per-shape table vs baseline + cuBLAS with no canary regression;
three-mode correctness gate per landing.

## Phase 3 — `.rbv` end-to-end

1. Webstack state probe (unprobed territory — scoping first), then the
   artifact decision. **Default: static WASM bundle** (the webstack backend
   is wasm32-LLVM already); dev-server later if the model demands it.
2. `brievc build counter.rbv` → servable artifact; `b-text`/`b-trigger`
   bindings live in a browser.
3. Dev loop (rebuild-on-change) + one worked example beyond the counter.
Gate: a stranger loads a `.rbv` page; the `.rbv`→`.bv` edge keeps it one
language.

## Phase 4 — One product, public face

1. Cross-surface demo: `.bv` logic + `.abv` kernel + `.rbv` UI from one
   program (the interop edges exist: `.bv` verified-offload chains,
   `.rbv`→`.bv`).
2. Public reproducible benchmark page (the A/B discipline here — baseline
   worktrees, interleaved measurement, correctness-before-timing — is
   publication-grade; weaponize it).
3. Docs front door: "Why Briev" (contracts + derivation, 1 page) +
   playground (wasm backend exists).
4. Parked, opportunistic: self-hosting feed (Phase 1 work grows
   `lib/compiler/*.bv`); a PL paper on derivation-not-recognition +
   proof-licensed rewrites. `.ebv`/`.sbv` get their own decision records +
   capability pass (seed material: `machine-entry.md`,
   `2026-08-27-cbv-foreign-hardware-and-mmio.md`).

## Defaults taken (flagged for revision)

- `.rbv` = static WASM bundle (not dev-server).
- Perf bar as in Governing rules (no pure-TFLOP chase).
- Order Phase 1→2→3; self-hosting parked behind Phase 2.
- Escape-ladder test gates Phase 2 syntax work, not the Phase 0 freeze
  (freezes the minimal closure set; admissions stay evidence-gated).
- `[*]` = AST-level desugar-to-lift (cheapest correct; subsumes
  electronics; optimize later).
- Quantities: Time dimension first, electrical retained.
- Phase 0.6 sweep seed = `[*]`, quantities, `mode` + mechanical discovery.

## Risk register

- `json.bv` blockers unscoped — Phase 1.1 scopes before building; may
  reveal genuine language work.
- Webstack end-state unknown — Phase 3.1 probes before committing to the
  bundle decision.
- Vulkan regression cause unproven — two recorded hypotheses (runtime port
  vs kernel/config churn); the shim decides.
- The freeze touches SPEC + tutorial + highlighter — same-commit
  obligation (Golden-rule docs maintenance).
- Session estimates are guesses; the gates, not the estimates, are the
  contract.

## First execution steps

(1) this plan doc; (2) Phase 0.1+0.2 D31 amendment; (3) Phase 2.1
era-runtime shim while the measurement context is hot.
