# Plan Index — START HERE (current status)

**2026-10-06.** 430+ files in `docs/plans/`; historical plans are
reference-only (never retroactively edited — AGENTS.md Rule 13).

**Suite state:** `cargo test --lib` 2901 green (2026-10-06, incl. the two
normalizer-diagnostic tests, three `List + List` tests, four json interpreter
tests, and five folio package tests); conformance sweep green; `gemm_h`
byte-identical; 19 pre-existing warnings; Praetor no new diagnostics.
**Ledger state:** `docs/plans/2026-10-06-bugs-ledger-sweep.md` — 152
unmarked BUGS entries verified, 4 open. Fixed during the sweep: the
`List<T> + List<T>` silent miscompile (now elaborates to stdlib `iter_chain`),
the interpreter's missing `<-` push (every list accumulator was wrong in the
reference), the json-interpreter array hang + two more Rule-5 divergences
(`&&`/`||` short-circuit, trailing-expression result), the 12 stale
normalizer warnings, and the unfired-`txn` empty-program silence (build now
warns).

**GPU standing state:** fused online softmax (`ptx_deferred_online: 1`) is
the shipped default. Composite decode: **float4 k/v loads landed
2026-10-02** (`d21f61d8`+`6512ac86`) — interleaved A/B vs `fec8b89a`:
**no-split -13% p50 (118.1 µs same-session)**; gates: m3 decode + softmax
s8 both lanes, AB variant-diff s8 9.466e-06 + decode 3.274e-06, gemm_h
byte-identical. Shipped-best decode config: **fused v4, NO-SPLIT**
(split=4 superseded — see `benchmarks/results/2026-09-30-5a-attention-decode.md`
last section). `ptx_deferred_skip_pass` diagnostic-only.

**Trusted GPU instruments** (never a hand-spliced probe — a probe
contradicting these means the gates are broken):
`benchmarks/deferred_ab_gate.sh` (variant diff, the A/B instrument),
`benchmarks/m3_attention_harness.sh` (live reference, honors
`BRIEFC_FLAGS`), `benchmarks/softmax_gate.sh` (NaN-hardened, honors
`BRIEFC_FLAGS`), atomic/workid/bad_ptx gates. Late-session kernel work
needs gates re-run at the TARGET geometry — s8 passing says nothing
about decode.

This index is the fresh-session orientation: the live foreign lanes, the
active umbrella, every workstream's remaining work with pointers, the open
bugs, and recommended starting points. Everything below is a map, not the
territory — read the linked plan before starting a workstream.

---

## Live foreign lanes — do NOT touch from main-side work

| Lane | Worktree | Domain |
|------|----------|--------|
| `feat/e14a-intent-synthesis` | `../briev-e14a` | Electronics `.ebv` (component laws, tolerance model, ERC) — **C2's parked remainder belongs here** |
| `feat/bad-dialect` | `../briv-compiler-bad-dialect` | Embedded / bad-dialect bootstrapper |
| (baseline) | `../briev-compiler-baseline` | Rule 12b A/B worktree — measure `main` only |

Standing exclusion (`2026-09-24-followup-stages.md` §Exclusion): never merge,
never touch; all baselines/gates measure **main only**.

---

## The active umbrella

`docs/plans/2026-09-24-followup-stages.md` — the post-metacomputing queue.
Declared order: **1 Front D → 2 Wave 1 (done) → 3 Wave 2 → 4 Wave 3 → 5 GPU
re-rank.** Interop Wave 2 ran ahead of stage 1 (independent lanes).
Stage verdicts: **Stage 1 Front D A/B-REJECTED 2026-09-25** (deferred
emitter stays; plain path 26× slower on composite @4096;
`benchmarks/results/2026-09-25-front-d-ab.md` — settled, no re-run).
**Stage 5 GPU re-rank executed 2026-09-30/10-01**: 5c ✅ (`8c0ece90`),
5a ✅ (72.5 µs, target 125 beaten; family retired `a7871a27`),
5d headline ✅ (GemmPlan remainder behind its own A/B), 5b remainder =
the GEMM fill-pipeline campaign (see Workstream 3). Stage table:
`2026-09-30-stage5-re-rank-and-5c.md`.

---

## Workstream 1 — Cross-dialect interop

Plan of record: `2026-09-21-cross-dialect-interop.md`;
design: `2026-09-25-sbv-ebv-bridge.md` (§"The graft pattern", §"Syntax
decision"); execution: `2026-09-27-wave2-sbv-ebv-exec.md`.

| Piece | Status | Next |
|-------|--------|------|
| Wave 1 — provenance, per-module semantics, collision gate, `.rbv`→`.bv` edge | **DONE** (C0–C6, `2026-09-25-interop-wave1.md`) | — |
| Wave 2 static `.sbv`→`.ebv` graft | **C0/C1 DONE** (`5d82be7a`, `47247c89`); **C2 parked** (`5f666996`) | C2's electronics remainder → e14a lane; interop-side: nothing blocking |
| `.sbv` corpus | `examples/silicon/sensor_die.sbv` (die), `examples/silicon/die_board.ebv` (checks clean; `build` needs the tolerance site) | — |
| Wave 2 declarations — `.bv`↔`.abv`, `.bv`↔`.sbv` | not started | declare the edges |
| **Wave 2b runtime pairs** — alias = interface-instance binding (`.bv` drives die pins → MMIO under the alias, synthesized-obj disclosure, unaliased-instance gate; `.abv` buffer surfaces) | not started | **biggest interop deliverable; approved design already written** |
| Wave 3 derivation — transitive bridges, synthesized bridge node, skip-a-runtime refusal | not started | after 2b |

C2 findings + tolerance-site options A–D: tail of `2026-09-27-wave2-sbv-ebv-exec.md`.

---

## Workstream 2 — Front D: retire the deferred emitter (umbrella stage 1)

**SETTLED — A/B REJECTED 2026-09-25** (`benchmarks/results/
2026-09-25-front-d-ab.md`): the deferred emitter stays; the plain
general path is 26× slower on composite @4096. Retirement effort moved
to the matcher ledger instead: fused-attention family retired
(`a7871a27`, ~1300 lines); remaining loans: `has_warp_slice` (retired
to config, 5c `8c0ece90`), `detect_row_softmax`, `GemmPlan` (each
behind a perf A/B — 5d remainder).

---

## Workstream 3 — GPU performance (umbrella stage 5, re-rank EXECUTED)

Stage table of record: `2026-09-30-stage5-re-rank-and-5c.md`
(5c ✅ 5b-active 5a ✅ 5d-headline ✅). Session record:
`benchmarks/results/2026-09-30-5a-attention-decode.md`.

**Current queue (in order):**

1. ~~Attention float4 k/v loads~~ — **DONE 2026-10-02** (-13% no-split
   p50; defects + lessons in the 5a results file's last section).
2. ~~div slowpath~~ — **SIZED + CLOSED 2026-10-02** (`2026-09-30-5a-attention-decode.md`); the cost was a misattribution (<0.5%), no slowpath warranted.
3. **GEMM fill-pipeline campaign** (32 → 42 TF @4096³) —
   `2026-09-30-stage5b-structural-fill-campaign.md`; the
   contract-licensed pipelining note (fill reorders loads the
   shape proves safe); no-fill evidence bounds the prize (compute
   intact at 45.4 TF); Rule 12 protocol. **STATUS 2026-10-04**: the
   Vulkan lane recalibrated — the 09-30 "12.2 TF" record measured a
   WRONG kernel (the B-fill mask bug, fixed `9da0c750`; its 32-column
   collapse was an accidental 2× L2 reuse). The **quad fill landed as
   default (n ≥ 256 guard): 4096³ Vulkan = 19.9 TF, 2.09×** (`52a6f12d`;
   results ADDENDUM 3). The campaign now runs from a REAL 19.9 TF base.
   **Rung 0: SPIR-V small-N — ROOT-CAUSED + FIXED 2026-10-04** (`7f285b6f`:
   the runner under-dispatched the naive tier 16× — the dispatch now keys
   on the emitter's chosen body; 64³/128³ EXACT; the n≥256 quad guard
   dropped as misattribution). **Rung 0b RESOLVED (2026-10-04, same day)**: the
   "naive-lane under-accumulation" was a GATE-HARNESS artifact — fixtures
   sized `a/b: Float16[MN]` (cube-only) let the M·K/K·N seed overflow
   into `i`, skipping the launch entirely; gemm_h.abv now sizes a[M*K],
   b[K*N], y[M*N] and the gate fails loudly on undersized fixtures; all
    non-cube shapes pass both lanes bit-exact. **g1024 CUDA race
    RESOLVED (2026-10-05)**: the run-varying patterned cells were the
    k-loop's TAIL — the fill guard stops committing, so
    `wait_group (stages−2)` no-ops with one pending group and the last
    stripe's cp.async races the final reads (BUGS.md resolution; fix =
    WAIT_DRAIN branch in `src/backend/ptx/tensor.rs`, deep-K sibling of
    the 2026-09-16 shallow-K drain). Post-fix: g1024 deterministic
    3.748e-3 ×12, full fixture sweep green both lanes, min-convention
    perf parity (`benchmarks/results/2026-10-05-cuda-tail-drain-fix.md`).
    Then: pipelined fills,
    B-traffic levers toward 32 TF. **2026-10-06 (Phase 2 start): rung 0
    re-verified EXACT at tip (64³/128³ f16, both lanes); the 5b PTX/CUDA
    timing rig is STALE/ephemeral — `gemm_h_bench` and the `--backend ptx`
    runner both dispatch-fail on the current kernel. Next: reconstruct the
    batched timing protocol as a committed script and validate against the
    19.9 TF Vulkan record before measuring the L2/B-traffic lever
    (`benchmarks/results/2026-10-06-rung0-verification.md`).**
4. **Re-rank table fold-in** — fold fused-attention + float4 numbers into
   the stage-5 table (row 3 already marked DONE).
5. ~~GemmPlan retirement~~ — **DECLARATION-GATED 2026-10-03** (increments
   1-4: the declared-composite channel, `matmul!` in numeric.bv, the
   matcher gate + advice diagnostic, the device gate). **BOTH lanes
   device-proven**: the CUDA GEMM correctness defect (the 5b ladder's
   standing "ptx_gemm_bench FAILs correctness") was ROOT-CAUSED and
   FIXED 2026-10-03 — the runner fed the SPIR-V tiled workgroup count to
   the flat CUDA kernel; the lane-split dispatch arm now covers the
   flat-PTX/tiled-SPIR-V pair (`e55103b2`, BUGS.md). Whole GEMM family
   all-ones EXACT both lanes (4096³ f32, 64³, f16 4096³/k1024/2048³/
   8192³). M4 remaining: `detect_reduction` rung — **DONE 2026-10-03**
   (`8d7050ad`: the cooperative channel declaration-gated —
   declared_matmul || declared dot; `dot!` in numeric.bv; the six chain
   fixtures migrated; the f16 chain fixtures' never-built defect fixed
   en route — the SPIR-V coerce now FConverts float-width mismatches,
   BUGS.md). Softmax-branch residual: **DONE 2026-10-03** (`b7833c63`:
   `softmax_rows!` declared, the cooperative-SOFTMAX channel
   kind-gated, softmax_rows.abv migrated — kernel identical).
   Const-expression folding — **DONE 2026-10-03** (`82f7c135` GPU +
   `373622e7` LLVM parity: the CPU lane derives counts too).

Also open (post-5 ladder): **M3** producer-consumer chain fusion;
**M4** `numeric.bv` declarations + vocabulary retirement
(`2026-09-20-gpu-dialect-beyond-cuda.md`). **M4 status: the
declared-coverage rows ALL landed (matmul!/dot!/softmax_rows!; the
matcher gates); the emitters' retirement A/B RAN — the cooperative
softmax emitter KEEP-AS-LOAN at a MEASURED 1.8-2.5x gap; the dot
emitter KEEP-AS-LOAN at 1.05-1.4x — BOTH verdicts in
(`benchmarks/results/2026-10-03-emitter-retirement-ab.md`). The
'Float' typecheck defect CLOSED as harness corruption (a corrupted
reference file — BUGS.md, closed; the compiler innocent). M3 LANDED (`b8f5d3f0`):
the general PTX lowering learned the row form; the cooperative
emitters RETIRED (~200 lines); the knob retired; the retirement gate
MET (the M3 row-form kernels reach the emitters' numbers within band,
both lanes — the results file's M3 table).

**shallow-K: RESOLVED** — the 2026-09-16 fix (`wait_depth = 0` at
K ≤ 128, tensor.rs:1762) re-verified at tip 2026-10-03 (full matrix
7 shapes × 3 runs × both lanes, all-ones + patterned + index probe, all
EXACT). The re-verification's patterned gate exposed a REAL Vulkan-only
f16 defect (coopmat B-fill pair mask, cols 32..63 of every 64-tile read
from 0..31) — root-caused + fixed the same day (BUGS.md, last entry;
`benchmarks/results/2026-10-03-shallow-k-tip-verification.md`). The
all-ones-only corpus is blind to column-mapping errors — patterned mode
is permanent now.

Pointers: `2026-09-16-gpu-strategy-findings-and-levers.md` (lever ledger +
the 64³–4096³ vs-cuBLAS map), `docs/architecture/gpu-backend-strategy.md`
(full landscape), vitriol ledger (single-source benchmark ledger).

**Three-surfaces umbrella (2026-10-04, ACTIVE)**:
`2026-10-04-three-surfaces-functional.md` — `.bv`/`.abv`/`.rbv` functional
for a stranger; the language invariant (surface vocabulary allowed, core
never forks, stretch-graded register); Phase 0 = the syntax freeze set
(D31 amendment landed `8fadfc2a`: D14 7→2 `tile`/`stage`, drops+defers,
admission process) → 0.3 `Asm#` two-lane audit **DONE 2026-10-05**
(`docs/plans/2026-10-05-intrinsic-coverage-audit.md`: Asm# = CPU/LLVM only,
PTX zero-coverage + SPIR-V gate-reject filed for Phase 2; coverage matrix
`scripts/intrinsic_probe.py`; fixed same-day: gate harvest bypass (new
`src/ast/visit.rs`), volatile-arity panic, SPIR-V gate drift 28→37; the
two filed LLVM classes **CLOSED later 2026-10-05** (`2dcdf464`
declared_min_arity; `6571c840` de-list — probe re-run: 0 panics, 0
undefined-symbol; audit-plan correction appended) → 0.5 SPEC fold
**DONE 2026-10-05** (SPEC §2.4: three keyword
categories, observability razor, warn/error ambiguity resolution, admission
process + D31 freeze note; SPEC §3.6: stretch-graded surface register
(Tier A/B, per-Tier-B redefined concepts), language invariant +
checkability rules; §4.1 cross-ref; no surface syntax changed →
tutorial/highlighter untouched) → 0.6 promotion sweep **(d) mechanical
inventory DONE 2026-10-05**
(`docs/plans/2026-10-05-grammar-form-inventory.md` +
`scripts/grammar_probe.py`: cross-surface matrix, classification table —
core / surface-owned (.ebv .abv .rbv) / licensed-stretch (.sbv has NO
exclusive grammar); probe found 2 new defect classes — electronics-under-.bv
codegen panic (`emit_expr.rs:2575`) and GPU-under-.bv mislowering, both
filed in BUGS.md; remaining: (a) `[*]` backport, (b) quantities Time, (c)
mode audit, registry-params fill + gate⇔arm rule (both **CLOSED later
2026-10-05**: `2dcdf464`/`6571c840` — probe 0 panics/0 undefined)) → (c)
`mode` audit **DONE 2026-10-05**
(`docs/plans/2026-10-05-mode-consumer-audit.md`: consumers = electronics
laws engine only; KEEP electronics-owned, promotion path named for the
first non-electronics type-state consumer) → (b) quantities **DONE 2026-10-05**
(`c1f1fc9f`: QuantityDim::Time, ns/…/Second suffixes, additive; then
expression-position quantity literals: keyword-suffix parse (`ms` =
`Token::Ms`) + SI magnitude at every consumer — codegen, reference
interpreter, GPU admission gate, guard-equality proofs; `250mA`→0.25 on
`.bv`, `check` OK all five surfaces; BUGS.md, grammar inventory corrected)
→ (a) `[*]` backport **DONE 2026-10-05** (plan-decided default
landed: `analysis::desugar::rewrite_wildcard_lift` — AST-level explicit
lift, declaration-order unroll, ONE implementation pre-typecheck so
interp/backends see only plain forms; v1 surface = assign-form lifts +
broadcast, fail-closed elsewhere; SPEC §15 note + `examples/wildcard_lift.bv`;
BONUS: closed the check/build desugar divergence — `brievc check`/sweep
had missed BOTH desugars (multi-index markers typechecked as unknown
calls since 2026-09-17); expression-position quantity literals **DONE
2026-10-05** (see (b) above); remaining 0.6: GPU-modifier boundary half
(P2 modifier walk, BUGS.md).
**Surface-capability gate LANDED 2026-10-05** (`8d4307ec` +
`70685b18` + `03ec5883`): component-pin access off `.ebv` = typecheck
boundary error (was a codegen panic; residual: stdlib-prelude
components under the wrong surface — open-world typing, BUGS.md);
GetGlobalId# de-listed from LLVM; the P2 "GPU mislowering" root-caused
to the int-literal-float-init IR bug — FIXED, `reduce.abv` builds on
`.bv`, grammar probe now all-ok/all-diagnostics (zero panics but
electronics-min, whose residual is filed). Remaining 0.6:
GPU-modifier boundary half (P2 modifier walk, BUGS.md).
Phase 1 = json.bv generics + package v0 + install.
**Phase 1 json.bv DONE 2026-10-05**: `lib/std/json.bv` works — objects,
arrays, numbers, escaped strings, literals parse/print in the LLVM backend.
~18 defects fixed (`734a8dac`, `cf38ced2`, `deacdf79`), including
callable-`txn` convergence (backend + interpreter) and List<enum> append.
**2026-10-06 interpreter parity:** json arrays now parse in the reference too —
three Rule-5 divergences fixed (`eval_match` arm writes, `&&`/`||`
short-circuit, trailing-expression result); `json_parse("[1,2,3]")` → length 3
in BOTH engines (BUGS.md FIXED). `list_concat` (`List + List`) now works — it
elaborates to the stdlib `iter_chain` (BUGS.md FIXED).
**Phase 1 package/install DONE 2026-10-06**: `folio.toml`/`folio.lock` (git +
path deps, Cargo-style), `src/packages.rs`, import wiring in build + check,
`brievc add/remove/update`; `brievc --version`, `brievc init` (runnable
`node entry [beginprogram][true]` — no `Main`), `brievc run x.bv` executes the
binary, relocatable `resource_root()`, installer ships `lib`/`config`/`plugins`,
`scripts/folio-smoke.sh` passes. Plan: `docs/plans/2026-10-06-package-module-v0.md`;
architecture: `docs/architecture/folio.md`.
Phase 2 = fill campaign (rung 0 =
small-N defect) + vocabulary retirement + the escape-ladder test.

---

## Workstream 3b — `.rbv` web surface

**Status 2026-10-07:** W1–W3 + rider DONE (`docs/plans/2026-10-07-web-surface-completion.md`).
- **W1** — in-repo router regression gate (`tests/fixtures/router.rbv`,
  `tests/rbv_router.rs`, `benchmarks/rbv_gate.{sh,mjs}`, `benchmarks/rbv_ir_check.py`).
- **W2** — `BUGS.md:8800` FIXED (unpacked-obj String-field init).
- **W3** — multi-page B2 (`examples/multi_page_{a,b}.rbv`); B1 covered by the
  router fixture in bundle mode; file-based routing deferred.
- **Rider** — `#Web` swept from feature docs; `warn_undispatched_txns` no longer
  false-warns a view-bound no-param txn.

**Decision (2026-10-07):** `#Web` is **not a protocol** —
`docs/architecture/web-host-boundary-decision-record.md` supersedes
web-routing decision #4. The browser is a **library over a host-import
namespace** (the JS/Web + wasm-embedder shape); `#System` is the one base
host namespace; GLUE stays **languages only**. D7 host-import provenance
**landed** (`1dc1bd80`); `node`→`js` rename deferred.

**FIXED (2026-10-07):** member txn on a plain top-level obj var is emitted as
a top-level variant (`@go_<var>`) and the view's bare-name `b-trigger` is
rewritten to it (BUGS.md:8921). Gate: `tests/fixtures/obj_router.rbv`
(`rbv_gate.sh` step 3). Both the plain-var and `render <Obj>` component forms
now emit + bind member txns.

**Popstate (2026-10-07):** real back/forward routing via the window-scoped
trigger `b-window:popstate` (parallel to `b-trigger:`/`b-on:`; plan
`docs/plans/2026-10-07-rbv-popstate.md`). `TriggerScope` distinguishes
`Element` (`el.addEventListener`) from `Window` (`window.addEventListener`);
the shim picks the listener target by scope. The event name is not validated
(the browser owns the vocabulary); the txn validity inference (unknown
directive, SRBV004/005 existence, precondition lint, liveness root) is shared
across scopes. Gate: `tests/fixtures/popstate.rbv` (`rbv_gate.sh` step 4).

**`b-window:` on a `b-each` container (2026-10-08):** allowed as a single
global Window-scope binding (one listener, not per-item) + an informational
`note[RBV012]` teaching the semantics. Uniform rule: a `b-window:` directive
is always a Window-scope trigger, wherever it appears. The shared
`trigger_scope_and_prefix` helper is the single source of truth for scope
(Rule 17). See the amendment in the plan doc.

**Stranger-loads-page probe (2026-10-08, Phase 3.1 gate):** the web surface is
now proven end-to-end in a real browser. `benchmarks/rbv_browser_smoke.mjs`
(Playwright/Chromium, `rbv_gate.sh` step 6) loads `counter.rbv`'s bundle from
`file://` and asserts zero console/page errors, the seeded `b-text` reflects
the Briev-side seed on load, and the `+`/`Reset` click round-trip. The probe
found + fixed two stranger-relevant bugs (BUGS.md, 2026-10-08): the
`createApp` null-exports race and the `__web_boot` initial-flush gap (the
seeded `b-text` now shows the real seed, not the HTML literal). `b-` =
"binding" documented in SPEC §21.4 + the feature doc.

**File-based routing seam (2026-10-08):** the deferred multi-page routing is
implemented as a **seam, not a compiler route-table** (plan
`docs/plans/2026-10-08-file-based-routing.md`, decision record
`docs/architecture/web-routing-boundary.md`). The compiler owns only the
*eternal*: a `folio.toml [web.pages]` section (page key → `.rbv`), a
`data-briev-page="<key>"` stamp on `<body>`, a `<stem>.page.json` manifest in
`--split`, and `brievc web <dir>` (thin orchestration over the per-file build →
`nav.json` + `nav.html`). The compiler never interprets a key as a route. Route
*policy* is the **framework**'s: `lib/std/web/pages.bv` (`page_href`,
`route_name`, `current_path` over the browser host) is a swappable convenience,
NOT load-bearing. Gate: `rbv_gate.sh` step 5b.

---

## Workstream 4 — Runtime elimination (`briev_rt.c`)

`lib/runtime/briev_rt.c` is still ~20 KB on main. Families A+B+C (cast lanes,
print family, string ops — 14/14 parity corpora) merged; **remaining families**
(collections/vector, allocator, …) + embedded fold + Electronics/Silicon parts
+ logo swap. Parity harness: `bash benchmarks/parity/run.sh`.
Pointer: `2026-09-09-briev-native-runtime-and-family-realignment.md` (Parts
A–E, parity gates, allocator-ownership + `Asm#` amendments).

---

## Workstream 5 — Language features / expressiveness

- **Collections/watchdogs/memory Phase E** — `seq`/`vol`/`async`/`sync<g>`
  modifiers + the Rule 22 concurrency-classification gate. User-deferred;
  full track (`2026-07-31-collections-watchdogs-memory.md`,
  `planned-features-tracker.md` item 8).
- **`Asm#` two-mode intrinsic** — unlocks prefetch, hand SIMD, `rdtsc`,
  cpuid, TLS, fibers ("one primitive away" tier).
- **Allocator ownership** — `lib/std/alloc.bv` + `alloc-strategies.dbvl`;
  compiler keeps only the `--no-std` bootstrap heap.
- **`inline_frgn!` plugin**; **stable addresses** (address-of + contract
  class — genuinely future).
- **Self-hosting native emission tier** (x86-64/aarch64 or `.s` text) — the
  last rung. Embryo: `lib/compiler/*.bv`, the tamer VM, `compiler-in-Briv`
  dogfood passes. Bare-metal (rv64) tier already proven.
- Pointer: `docs/architecture/briev-capability-frontier.md` (expressiveness
  closure, tier table, the residual gap is ecosystem not mechanism).

---

## Workstream 6 — Silicon / CIRCT

Retire `.cbv` (Part D); `.sbv` semantics beyond the graft; **hook up the dead
`hardware_validator`** (zero call sites — the `.sbv` synthesizability gate
never runs; BUGS.md 2026-09-28, compiler-owned, unclaimed); CIRCT
`ExportVerilog` rejects `hw.module.generated` (OPEN toolchain, BUGS.md ≈5393).

---

## Workstream 7 — Other active tracks

From this index's 2026-09-08 pass — **verify freshness before starting**:

| Plan | Status | What's left |
|------|--------|-------------|
| `2026-09-02-graphics-ray-and-images.md` | Milestone A DONE; **B OPEN** | Storage image through the compute stack + live X11 window |
| `2026-09-06-cpp-expressiveness.md` | **Active** | C++-level expressiveness remainder (ISR/sections split out, DONE) |
| `2026-09-12-dynamics-causal-dag.md` | **Active** | Causal DAG wiring (proven/weak edges), cycle classification, liveness refusal, `--explain-causality`; fusion gated on an LLVM experiment |
| `2026-09-04-beyond-coopmat.md` | Stage 0 DONE; Stage 1 EXHAUSTED; **1.5 ACTIVE** | Portable tier at structural limit; PTX tier (Stage 2) demoted to optional |

---

## Open bugs / known gaps (`BUGS.md`)

**2026-10-06 ledger sweep** (`docs/plans/2026-10-06-bugs-ledger-sweep.md`):
all 152 previously-unmarked `BUGS.md` entries classified against the current
tree — 6 OPEN, 5 OPEN-UNVERIFIED (instrument named), 118 stale/resolved/
by-design/not-a-bug. Conformance sweep green at tip; 30/30 `lib/std` modules
`brievc check` PASS. Headers carry `[LEDGER 2026-10-06: …]` tags.

Fixed during the sweep (suite 2888 → 2896):

- **`List<T> + List<T>` silent miscompile** — now elaborates to the stdlib
  `iter_chain`; mismatched element types hit the ordinary `InvalidOperation`
  diagnostic (BUGS.md, 2026-10-06 FIXED).
- **Interpreter `<-` never pushed** — every list accumulator was wrong in the
  reference (`iter_chain([1,2],[3])` → `Int(3)`); now pushes (BUGS.md, FIXED).
- **json.bv array parsing hung in the interpreter** — three Rule-5 divergences
  (`eval_match` arm writes, eager `&&`/`||`, dropped trailing expression); now
  `json_parse("[1,2,3]")` → length 3 in BOTH engines (BUGS.md, FIXED).
- **A program of only plain `txn`s built to nothing** — the build path now warns
  per user-declared, non-reactive, uncalled txn (BUGS.md:5698, FIXED — diagnosis
  only; `node` remains the fired form).
- 12 stale normalizer warnings on hello-world (BUGS.md, FIXED).

Verified-open, in stranger-blocking order:

- **`lib/compiler/*.bv` dogfood: 10 of 12 fail `brievc check`** — the sweep's
  deliberate exclusion; a green sweep does NOT mean the self-hosting embryo
  parses (only `reader.bv`, `token.bv` pass).
- `hardware_validator` dead code — the `.sbv` synthesizability gate never runs
  (BUGS.md:7439; `src/lib.rs:57` is its only reference).
- `dyn Trait` — interpreter dispatch complete, **LLVM backend still panics**
  (`emit_toplevel.rs:754`; BUGS.md:5148 HALF-CLOSED).
- `i64` boxing tax Phase 1 never executed (`adapt_to_i64`, helpers.rs:2223).
- nbody_newton 7th-decimal drift — re-measured 2026-10-06 (BUGS.md:5954).
- CIRCT `ExportVerilog` `hw.module.generated` — OPEN (toolchain; BUGS.md:5678).
- `hardware_validator`/`.sbv` items and the CUDA `push_strided` quirk
  (BUGS.md:6740) — vendor-side or hardware-gated.

Open-unverified (do not re-investigate; run the named instrument): BUGS.md
5827 (2–7-workgroup RTX 3060 dispatch), 6166 (`spirv_coopmat_subgroups=1`
2048³), 6707 (m4 decode microbench), 7460 (driver 615 vs 580 triple).

- ~~`json.bv` migration blocked on generic type inference + three language
  gaps~~ — **CLOSED 2026-10-05** (json.bv works in the LLVM backend; the
  interpreter's array path is OPEN, see above).
- Baseline-harness defects — **PARTIAL**; protocol round-trip proofs — **PARTIAL**.
- `2026-09-11-phase2b2-instance-state.md` — extracted housekeeping item, pending.

**Closed 2026-09-28** (this session, umbrella
`2026-09-28-native-daily-use-gpu-parity-umbrella.md`):
- Tuple-returning defn with a String field — **FIXED-VERIFIED** at tip
  `1d0dc01f` (repro compiles + runs; BUGS.md entry updated).
- BEAST TypeDef members — **FIXED** (`1300ce92`): serialize/deserialize
  round-trip `body.members` + `parse_toplevel` dispatch + round-trip test.
- **Stale-binary guard — SHIPPED** (`478940bb`): `brievc freshness`
  command, mtime compare vs `src/`+`config/`, exit 1 + offending file
  when stale.
- `get_env_int_or` migration — **STALE INDEX ENTRY**: zero references
  remain (env.bv replaced the intrinsic 2026-07-19); no action needed.
- Front D (umbrella stage 1) — **A/B REJECTED 2026-09-25** (deferred
  emitter stays; plain path 26× slower on composite @4096;
  `benchmarks/results/2026-09-25-front-d-ab.md`). Stage 1 verdict
  settled — no re-run needed at the 2026-09-28 tip.

---

## Recommended starting points (no foreign-lane overlap)

0. **`.bv` stranger blockers** (2026-10-06 sweep): the `List<T> + List<T>`
   silent miscompile, the json-interpreter array hang, the unfired-`txn`
   empty-program silence (BUGS.md:5698), and Phase 1.2/1.3 (package v0 + install
   story) are all DONE. Phase 1 is complete; next is Phase 2 (fill campaign).
1. **div slowpath sizing probe** then the **GEMM fill-pipeline campaign**
   (Workstream 3 queue items 2-3) — the fill campaign is the biggest
   absolute prize (32 → 42 TF), plan + correctness license written
   (`2026-09-30-stage5b-structural-fill-campaign.md`).
2. **Wave 2b runtime pairs / alias binding** — approved design, biggest interop value.
3. **Quick wins** — `hardware_validator` hookup (stale-binary guard now
   shipped: `brievc freshness`).
4. **Runtime families H/I/J** — finish `briev_rt.c` (511 lines remain:
   async/event machine, spawn/setenv, Tamer HCALL, string-bitop helpers).

---

## GPU ledger

`2026-08-31-vitriol-gemm-comparison.md` is the single-source GPU benchmark
ledger (M1 GEMV → O2/O3 → GEMM f32/f16 → mma ceiling → CUDA race). Add new
rows there, never rewrite old ones.

## Full landscape reference

`docs/architecture/gpu-backend-strategy.md` — the complete, evaluated
optimization space (Roofline, emitter routes, async pipelines, beat-CUDA
levers, multi-vendor matrix). Use for any GPU-direction question; the
stage/execution plans are the concrete campaigns.

## How to classify a plan you're about to touch

1. Grep `Status:`/`**Status:**` in the first 25 lines — most recent plans carry one.
2. If none: check the ledger or `git log` for the committing session's outcome.
3. If still unknown: read it; date + title usually says pre/post-rewrite.

## Recently closed (reference when touching related code)

| Plan | Closure |
|------|---------|
| `2026-09-28-daily-use-sweep-and-gpu-session.md` | Phase B GPU (B1–B4) DONE; Phase A core DONE 2026-10-01 (A1 name-capture fix, A2 Stack peek, D5 test_collections repair); A3 executable gate + A5 audit + A6 probe REMAIN |
| `2026-09-30-stage5-re-rank-and-5c.md` | 5c DONE (`8c0ece90`, byte-identical IR gate + determinism fixes); 5a DONE (72.5 µs); session record `2026-09-30-5a-attention-decode.md` |
| `2026-09-08-master-workstream.md` | Phases 1-6 DONE; Phase 7 optional, Phase 8 verified done |
| `2026-09-08-gemm-occupancy-campaign.md` | CLOSED — 0.708ms = 24.3 TFLOP/s (95% HW peak) |
| `2026-09-08-hashmap-rehash-and-foreach-fix.md` | COMPLETE |
| `2026-09-07-init-block-phi-predecessor.md` | COMPLETE (`c4ee2e19`) |
| `2026-09-07-noalias-benchmarking-and-test-alignment.md` | DONE |
| `2026-09-06-isr-handlers-and-sections.md` | COMPLETE |
| `2026-09-06-p0-p1-implementation.md` | DONE — P0 prefetch, P1 strength-reduce |
| `2026-09-05-gpu-profiling.md`, `-kernel-profiling-analysis.md` | COMPLETE |
| `2026-09-04-gemm-perf-blocks.md` | Superseded by the occupancy campaign |
| `2026-09-01-smallm-splitk.md`, `-warp-mlp-ilp.md`, `-vec4-projection-layout.md`, `-cooperative-row-kernels.md` | Rungs landed/refuted; results in the vitriol ledger |
| `2026-08-31-gpu-next.md`, `-o3-float4-loads.md`, `-abv-gpu-by-default.md` | DONE |
| Interop Wave 1 (`2026-09-25-interop-wave1.md`) | DONE (C0–C6) |
| `.sbv` die graft C0/C1 (`2026-09-27-wave2-sbv-ebv-exec.md`) | DONE; C2 parked |
