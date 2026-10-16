# cpp-vs-briev contributor findings — assessment, compiler fixes, benchmark adoption

**Date:** 2026-10-16
**Status:** COMPLETE 2026-10-16 — all phases landed (P0 repro + flaw-4 fix `93be0795`; P1 memset `a42de3c0`; P2 node-inline budget `6e3a78ef`; P3 doc verdict; P4 harness benchmark + corpus + attribution). Full record:
flaw 4 (NEW — reassigned-let alloca demotion in the reactive txn emitters,
blocked every compile) FOUND + FIXED + pinned by test; flaw 3 verdict:
era-dependent, current diagnostics sufficient → documentation remainder;
flaws 1-2 do not reproduce on clang 23 but the IR pathologies remain →
P1/P2 continue as hardening. Results file is provisional until the P4 gate.
**Trigger:** external contributor experiment reported via LinkedIn; repo
analyzed at our request ("could you write up a PR with this? It might give me
new ideas on how to optimise the backend" — R. Smits).

## Attribution

- **Otto Westerlund** (`github.com/odecode`, author email
  `westerlundotto@gmail.com`) designed and ran the entire experiment:
  C++ reference re-optimization, three Briev ports (dense → sparse →
  multinode), fair-timing re-instrumentation of both sides, and the
  LLVM-crash workarounds documented in his README. His repo:
  <https://github.com/odecode/cpp-vs-briev> (single commit `714130e`,
  pushed 2026-10-16 14:10 EEST). Otto's org is also the `fork` remote of
  this repository (`odecode/brief-lang`). Local clone used for analysis:
  `/tmp/opencode/cpp-vs-briev` (shallow, depth 50).
- **Randy Smits** directed the follow-up: `-O3` correctness priority
  ("Briev should work with -O3 btw"), inspection of emitted IR *and* asm
  for backend learnings, and the constraint that any technique learned
  must not get in LLVM's way.
- All compiler-side work in this plan: this repository, this session.

Otto's environment: Windows/WSL, LLVM/clang **17** (`C:\Users\wotto\code\
briev-lang` ↔ `/mnt/c/...`). Our environment: Linux, clang **23.1.1**
(recorded P0 variable — crash repro may differ across this delta).

## Background — what the contributor built

MovieLens-100k neighborhood-CF recommender (AutAI method, Ren et al.
CIKM'12), C++ original vs Briev ports. The Briev program: raw-syscall file
IO, byte-scanning integer parser, dense 943×1682 float matrix, column-major
per-item sparse layout (count → prefix-sum → fill, untimed), per-item
prediction over its sparse block only, top-5 selection, in-program timing
via `SysCall# 228` (`clock_gettime(CLOCK_MONOTONIC)`, two reads total).

His headline results (his timings, prediction phase, 1501 predictions,
user 10):

| Implementation                          | Time      |
|-----------------------------------------|-----------|
| C++ original (`-O3`, dense, mean recompute) | ~5.3 s  |
| Briev dense (`-O1`)                     | ~3.9 ms   |
| C++ optimized ST (`-O3`, SIMD+sparse)   | ~1.5 ms   |
| **Briev sparse (`-O1`)**                | **~0.15 ms** |
| C++ single-pass ST (`-O3 -march=native`) | ~0.15–0.27 ms |

Like-for-like on identical work (single-pass, no scratch arrays, no SIMD):
**Briev ≈ hand-tuned C++ `-O3`**. His honest finding: the earlier ~10× C++
"win" was SIMD-as-pessimization (per-item `std::vector` setup cost more
than the reduction saved at these block sizes); the earlier ~2× C++ loss
was unfair timing (3002 `chrono::now()` calls inside the hot loop — he
re-instrumented his own C++ to fix it). His summary, which we endorse:
the biggest wins were data layout and computation strategy; the compiler
was not the bottleneck at `-O1`.

His key language learnings (README, verbatim findings):
1. 1.6M-element **global** `Float[]` crashes LLVM 17's X86 instruction
   selector; workaround = `Malloc#` + `Int` addresses.
2. File paths need `to_c_string` (`std/string_c.bv`) — raw `String as Int`
   points at the Briev header, `open(2)` returns `-EFAULT`.
3. Multi-node reactive handoff hit either a `__wait_for_trigger__`
   liveness panic or the concurrency gate; workaround = all `async` nodes
   gated on distinct `phase` values (the `benchmarks/nbody_newton_accel.bv`
   pattern), single-pass nodes with `[true]` postconditions.
4. `-O2`/`-O3` crashed LLVM 17's **greedy register allocator** on the
   single giant node (whole program inlines into one `@txn_run`);
   workaround = split into per-phase `async node`s + extract the hot loop
   into a standalone `defn predict_item` (a real non-inlined function via
   `src/analysis/inline_cost.rs` weighted-body decision).

His repo contains: `README.md` (238 lines, full protocol),
`recommender_multinode.bv` (348 lines — the final version),
`recommender_singlepass.cpp` (414 lines — like-for-like C++ reference),
`recommender_optimized_st` (40 KB ELF — committed binary). **Missing from
the repo** (referenced by README, needed for full repro):
`recommender_poc.bv`, `recommender_full.bv`, `recommender_sparse.bv`,
`probe_heap.bv`, `probe_bigarray.bv`, `probe_small.bv`, `probe_open.bv`,
`probe_read.bv`, and the input data (`u.data`, `simlist.txt`).
**Decision:** reconstruct the missing probes from README descriptions
(approved); do not block on the contributor.

## Findings against the current tree (root causes, file:line)

### Flaw 1 — giant zero-init aggregate store

`emit_field_init_value` emits `store [N x T] zeroinitializer` for
array-typed state fields with no initializer
(`src/backend/llvm/emit_toplevel.rs:2811-2819`, and the zero-`Decimal`
path at `:2735-2743`). At `N = 1586126` floats this is a single
multi-megabyte aggregate store that (per Otto, on LLVM 17) crashes the
X86 instruction selector. The `@__briev_state` global itself is defined
`zeroinitializer` (`emit_toplevel.rs:1276/:1308`) — the store exists for
reset/init semantics, but its *lowering* degenerates at large N.

### Flaw 2 — node-body txns bypass the inline-cost budget

Callable txns get a frontend weighted-cost inline decision
(`src/analysis/inline_cost.rs:38`, `callable_inline_weight_threshold`
default 40 from `config/ir-lowering.dbvl` via `config_tuning`; FFI/
trigger bodies excluded). Node-body txns get **no** such decision: they
are emitted `alwaysinline` whenever `#inline` is absent and
`!self.ctx.has_cycles` (`src/backend/llvm/emit_toplevel.rs:3745-3778`,
applied at `:3963`). A large single-node program therefore inlines
everything into one LLVM function → greedy regalloc
(`RegAllocBase::allocatePhysRegs`) segfaults at `-O2`/`-O3` on LLVM 17.
The `has_cycles` fallback exists, but an acyclic 1400-line node sails
through. Context: the two-pass emission and register shim assume the
txn body is available at the go-site (`context.rs:1392`,
`emit_stmt.rs:2312`) — **structural-inlining question P2 must answer
first**.

### Flaw 3 — sequential multi-node handoff panic (undecided)

Plain reactive nodes handing off via a shared `phase` field hit a
`__wait_for_trigger__` liveness panic (`loop_engine/mod.rs:285` emits the
call; `defn_liveness.rs:294` roots it) or the Rule-22 concurrency gate.
Otto's all-`async` + distinct-phase-gate pattern works and is Rule-22-
compliant (explicit classification). Undecided: (a) liveness-analysis
bug, (b) correct-but-cryptic diagnostic, (c) intended behavior needing
documentation. Minimal repro decides.

### Opportunity — emitted-asm study (P0.6)

User directive: inspect emitted asm for optimization techniques we do not
yet account for, **only if we don't get in LLVM's way**. Classification
(see P0.6): (a) LLVM already wins → record; (b) our lowering blocks LLVM
→ fix lowering (P1/P2 class); (c) genuine technique gap → candidate
general pass under the full Rule 12/20 experiment protocol. No
asm-level hacks, no fighting the optimizer, no algorithm-shape
recognition (Rules 23/24 hold — every technique must generalize to all
programs).

## Phases

### P0 — Reproduce + IR/asm analysis (no source changes)

0.1 **Environment pin.** clang/LLVM 23.1.1 (Linux) vs Otto's LLVM 17
(WSL). Record in the results file; all repro verdicts are
version-qualified.

0.2 **Build.** `cargo build --release`; verify `cargo test --lib` green
before starting (baseline sanity).

0.3 **Inputs.**
- `u.data`: MovieLens-100k from grouplens (`ml-100k.zip`) if network
  allows; else synthesized deterministically.
- `simlist.txt`: original unavailable (Otto's project data). Synthesize
  deterministic non-negative integers (Briev byte parser handles digits
  only; C++ `>>` float parser must agree) 943 rows × 1682 values.
  Parity therefore = cross-implementation on identical synthesized
  inputs, NOT vs Otto's original outputs. The 1501-prediction /
  top-5 checks from his README apply to *his* inputs and are recorded
  as his claims; our parity gate is Briev-vs-C++ on our inputs.

0.4 **Repro matrix** — each claim gets CONFIRMED / REFUTED /
ENVIRONMENT-DEPENDENT with the exact IR/error captured:
  a. `recommender_multinode.bv` at `-O1/-O2/-O3 -march=native` + link
     with harness flags (`-O3 -flto -march=native -ffast-math
     -fdata-sections -ffunction-sections -Wl,--gc-sections` +
     `lib/runtime/briev_rt.c -ldl`); run; cross-check vs
     `recommender_singlepass.cpp` binary on same inputs.
  b. Reconstructed `probe_bigarray` (global `Float[1586126]`): capture
     emitted store shape; attempt `-O1/-O2/-O3`; record crash or pass.
  c. Reconstructed single-giant-node sparse variant (merge multinode
     phases into one node): confirm `-O2/-O3` greedy-regalloc failure;
     capture the inlined `@txn_run` size; `-O1` must work.
  d. Sequential (non-`async`) two-node `phase` handoff minimal repro:
     capture the liveness panic / concurrency-gate error verbatim for
     P3.

0.5 **`.ll` diff.** Briev `predict_item` + predict-node IR vs
`clang -O3 -march=native` C++ IR: addressing, load widths, vectorization,
per-item setup, loop shape. Records what the frontend offers LLVM.

0.6 **Emitted-asm study** (user directive; Rule 6 — the LTO lesson).
NEVER judge from `llc -O2` or raw `.ll`; disassemble the actual
harness-linked binaries (`objdump -d` on the `-O3 -flto` Briev build and
`recommender_optimized_st`). Hot regions by profile. Diff on the hot
loop for: vectorization (width, masked, gather vs unit-stride),
addressing/strength reduction, unroll factors, branch structure, spill
counts, loop-invariant hoists, memory-level parallelism, prefetch.
Classify every observation (a)/(b)/(c) per the Opportunity section.
Every (c) candidate: transform the ACTUAL generated IR as a pre-build
experiment, link with the harness command, verify output equality at a
BOUND that crosses a print boundary, interleave reference/experiment
timings ×N (`LC_ALL=C /usr/bin/time -f "%e"`), before any implementation
is considered. Results → backlog ledger with measured prize; nothing
implemented speculatively in this plan.

0.7 **Baseline timings** (Rule 12). Full table at current commit via the
harness: multinode at -O1/-O2/-O3, C++ singlepass, C++ `optimized_st`
binary. Provisional until the P4 parity gate; marked provisional in the
results file itself.

**Gate:** written repro status per claim; asm/IR observations ledger
with (a)/(b)/(c) classification.

### P1 — Zero-init aggregate: `llvm.memset` above threshold

- **Root cause:** Flaw 1.
- **Fix:** aggregate zero-init with byte size ≥ threshold →
  `llvm.memset.p0.i64(ptr, 0, size)` intrinsic call; below threshold the
  existing `store` stays. Threshold knob in `config/ir-lowering.dbvl`
  (beside `callable_inline_weight_threshold`), read via `config_tuning`.
  Applies at both sites (`:2811-2819` uninit-array path, `:2735-2743`
  zero-`Decimal` path). Reset semantics preserved: the memset fires at
  exactly the sites the store fired; the global's `zeroinitializer`
  definition is untouched.
- **Non-zero-into-struct fallback** (`:2738-2748`): keep, but the zero
  store inside it also becomes memset above threshold.
- **Tests:** codegen test — large array field → `llvm.memset` present,
  no aggregate store; small array field → store retained (byte-identical
  to today); zero-`Decimal`-into-array path covered; regression: full
  suite; benchmark `.ll` outputs unchanged where no large array exists
  (spot-check `gemm_h` byte-identical per house practice).
- **Gate:** `cargo test --lib` green; no new warnings; Praetor on
  changed dirs (directory targets only); `probe_bigarray` compiles clean
  at `-O3`.

### P2 — Node-body inline budget (the `-O3` fix; largest item)

- **Root cause:** Flaw 2.
- **Investigation first:** is alwaysinline `@txn_go` structural (two-pass
  emission, register shim, `@txn_go` pass boundary at `context.rs:1392`,
  `emit_stmt.rs:2312`) or an optimization? Read the emission passes;
  determine what breaks if a node txn loses `alwaysinline`.
- **Fix (a) — if de-inlining is safe:** extend the `inline_cost` weighted
  machinery to node-body txns. New additive `AnalysisResults` field
  (e.g. `node_inline_decisions`), computed once in the frontend; backend
  consumes (frontend-driven dispatch doctrine; backend never re-decides).
  Config knob `node_inline_weight_threshold` in `config/ir-lowering.dbvl`.
  Small nodes keep today's alwaysinline path.
- **Fix (b) — if structural:** general weight-budget **body-segment
  extraction** pass in the frontend: oversized node segments are lowered
  into real non-inlined functions called from the body (generalizing
  Otto's manual `predict_item`). Shape-agnostic (loops/guards by weight,
  no vocabulary matching — Rules 23/24); extraction boundary chosen so
  LLVM's own inliner can still re-inline below its threshold — we make
  the IR *well-formed for LLVM*, never compete with it (user's "don't
  get in LLVM's way").
- **A/B (Rule 12b):** full suite vs `../briv-compiler-baseline` via
  `bash benchmarks/compare_baseline.sh <name>`; any regression gets the
  controlled experiment, never "noise". Verify: giant-node probe
  compiles + runs at `-O2/-O3`; multinode output parity; suite
  byte-identical outputs where behavior is unchanged.
- **Tests:** frontend decision unit tests (weight around threshold);
  backend emission tests (attribute present/absent per decision); P0.4c
  giant-node probe as the end-to-end regression.

### P3 — Sequential handoff panic

- Minimal repro from P0.4d; read against
  `docs/architecture/briev-execution-model.md` (reactor = scheduler,
  wake sets, equilibrium) and Rule 22 (no implicit concurrency).
- Verdict exactly one of:
  - **(a) liveness-analysis bug** → fix in `defn_liveness.rs` /
    `loop_engine` + regression test (interpreter stays the reference —
    fix codegen, never the interpreter);
  - **(b) correct but cryptic** → improved diagnostic per house style
    (`src/errors.rs`): what is wrong, the proof/why, the concrete fix
    ("gate each node on a distinct phase value and mark the pair `async`
    or `sync<g>`" — supply the trivial proof where applicable), no
    compiler-internal jargon, no doc-path references;
  - **(c) intended** → document in the execution-model doc; no code
    change.
- The Rule-22 concurrency gate itself is NOT weakened in any verdict.

### P4 — Adopt benchmark + docs

- Port to `benchmarks/`: `recommender_multinode.bv` (+ the reconstructed
  giant-node variant as a `-O3` codegen stress case),
  `recommender_singlepass.cpp` as the C++ reference; `build_and_bench.sh`
  integration; parity gate = identical prediction count + top-5 item set
  (Briev vs C++ on identical synthesized inputs; document Otto's
  tie-order-permutation caveat — dozens of items tie at the clamp, sort
  permutation among ties differs, predictions equivalent).
- Results file `benchmarks/results/2026-10-16-recommender.md`: P0.7
  tables (unmarked-provisional until gate), P0.6 asm ledger, repro
  matrix, environment (clang 23.1.1 vs Otto's LLVM 17).
- **Docs same-commit (Rule 13):** BUGS.md entries for flaws 1-3 with
  repro status + fix commits; `docs/architecture/` — zeroinit lowering
  rule (where aggregate-init lowering is documented), node-body budget
  (where inline decisions are documented); execution-model doc per P3
  verdict; `docs/plans/INDEX.md` workstream entry; this plan's status
  line. Never retro-edit historical plans (Rule 13).
- Otto's repo credit recorded in the benchmark README and results file.
- License: any adopted contributor code keeps its provenance noted; the
  compiler stays Apache-2.0 WITH LLVM-exception; emitted kernels/binaries
  get no license headers (Working Rules).

## Verification gates (per-commit checklist)

`cargo test --lib` green; `cargo build` no new warnings; Praetor
`validate --warn --target <changed-dir>` (DIRECTORY targets only — file
targets silently pass); Kani harnesses for any new safety-critical
analysis code (P2 frontend pass); architecture docs updated with API
contract changes; BUGS.md updated; auto-commit per completed step
(working rules); push to `origin main` per user instruction for the plan
commit, subsequent work commits follow the same flow.

## Commit order

1. This plan doc (attribution complete) — commit + push **first** (user
   directive).
2. P0 findings: repro matrix + asm/IR ledger + provisional timings →
   results file + plan status update.
3. P1 fix (independent).
4. P2 fix (independent of P1; sequence by P0 evidence — if the regalloc
   crash reproduces and the zeroinit crash does not, P2 goes first).
5. P3 verdict + fix/diagnostic/doc.
6. P4 adoption + docs + un-mark provisional timings.

## Risks & fallbacks

- **Crashes don't reproduce on clang 23** (LLVM 17→23 delta): P1/P2
  become hardening-with-regression-tests (the IR shape is still
  pathological — a multi-megabyte aggregate store and an
  everything-inlined `@txn_run` are wrong at any LLVM version); record
  ENVIRONMENT-DEPENDENT verdicts with version.
- **`@txn_go` inlining proves structural:** P2b (segment extraction) is
  the larger build — scope the structural sub-investigation in a
  dedicated step before committing to (b); the frontend-decision
  scaffolding from (a) is reusable either way.
- **No network for ml-100k:** synthesized `u.data` (deterministic RNG,
  100k ratings, 1-indexed ids in range); parity stays
  cross-implementation; note in results file.
- **simlist parser skew** (Briev digits-only vs C++ `>>`): synthesized
  values are non-negative integers within float-exact range; verified
  by cross-implementation prediction-count equality before timing.
- **P2 A/B regression on the existing suite:** Rule 12b experiment vs
  baseline worktree before any corrective action; the small-node
  alwaysinline fast path must remain byte-identical in emission.

## Rule compliance summary

Contract-first (no contract touched); Rule 2 (all fixes are defaults, no
new strategy keywords); Rule 3 (no hidden special treatment — config
knobs disclosed); Rules 6/11 (additive match arms only, prerequisites
implemented); Rules 12/12b/20 (baseline table, baseline worktree A/B,
experiment-before-build for every perf claim); Rules 15/19/23/24 (no
type-name matching, no vocabulary matching — extraction is weight-based
and shape-agnostic); Rule 22 (gate not weakened; `async` classification
respected); Rule 13 (docs ledger above); Working Rules (plan before
work, same-commit docs, helpful diagnostics, flat control flow, sorted
HashMap iteration in any new IR-producing iteration).

## Addendum (2026-10-16, later session): absolute-limit instrumentation

Review challenge: the P2 threshold's "empirical calibration" (600) is
box-dependent — the boundary between two corpus anecdotes. Decomposition
agreed with the author: the "absolute limit on a single compilation target"
is three distinct quantities, previously conflated:

1. **Frame size — the true a-priori absolute.** Per-function Σ(alloca
   bytes) is exactly computable in the frontend; the target's bound
   (displacement cliffs / declared stack ceiling) is a machine fact →
   `frame_limit_bytes` per target in `config/targets.dbvl`. Breach =
   house-style diagnostic (what/why/fix). Absent key = hosted, skipped.
2. **RA cost — no frontend-computable absolute; measured knee per
   toolchain.** `benchmarks/ra_knee_sweep.sh` protocol: weight ladder,
   compile time + spills, knee := superlinear jump, threshold := knee ÷ 4.
   Result recorded as a dated measurement (toolchain version pinned),
   replacing 600's corpus-derived value. Re-derivable when LLVM changes.
3. **Backend defects — version-bounded ledger.** LLVM 17 greedy-RA
   segfault enters BUGS.md as DEFECT (not limit): affected versions, the
   valve covering it, retirement gate (floor toolchain > 17).

Order: 3 → 1 → 2, per-commit gates as usual. The 600 valve stays in force
until Part 2 replaces it with the derived value.

**COMPLETE 2026-10-16.** Part 3: BUGS.md defect ledger (a2909c9a). Part 1:
frame-limit check, `frame_limit_bytes` per target, `llvm_type_bytes` sizer,
house-style rejection (63107709). Part 2: `benchmarks/ra_knee_sweep.sh` +
live bisection — clang 23 linear through 51k weight (no resource knee); the
binding constraint is the LLVM 17 defect boundary, bisected at (510, 1460);
valve 600 = 1460/2.4, byte-identity held (58b292ca). Retires with the
defect per the ledger.
