# Plan Index — START HERE (current status)

**2026-10-06.** 430+ files in `docs/plans/`; historical plans are
reference-only (never retroactively edited — AGENTS.md Rule 13).

**Suite state:** `cargo test --lib` 3732 green (2026-11-12, incl. 7 owned-storage tests); daily corpus 41 green; quick gate 36 green; Task Board structured Chrome/API green; full runtime suite green; browser host/heap/snapshot gates green; determinism quick green.

**Production correctness strategy (adopted 2026-11-15):**
[Production compiler correctness: evidence, oracles, and an execution plan](2026-11-14-production-correctness-strategy.md)
preserves the audit of upstream `e9169750`. The
[current-main reassessment](2026-11-15-production-correctness-reassessment.md)
checks `817f1d69`: CI solver setup, exact solver-status recognition and native
wrapping repairs landed; gate coverage/accounting, strong HIR/native oracles,
and the complete ownership/proof campaigns remain.
[A0 gate integrity](2026-11-15-a0-gate-integrity.md) implements shared quick
selection, explicit verifier/Z3 execution, fail-closed runner controls, and
retained manifests; its execution record contains the current gate results.
Acceptance: both quick runners **3,873 passed / 17 suites**, exact identity and
outcome parity; full root library **3,788 passed**; **22** infrastructure
controls; all **32** verifier cases including the three named Z3 regressions.
The [independent CI jobs increment](2026-11-15-a0-independent-ci-jobs.md) splits
required checks behind their actual build dependencies, transfers the compiler
with its runtime archives, and keeps a fail-closed `Default gates` aggregate.
Local acceptance: **31 controls**, the same **3,873** quick results, **3,788**
library tests, **18** binary tests, **42** tier-1 fixtures, census, and restored-
artifact native/browser checks pass; hosted scheduling remains to be qualified.
The [hosted qualification increment](2026-11-15-a0-hosted-qualification.md)
integrates `8b69cbe2`, retains upstream's per-file Counsel census, restores full
39-line float-formatting coverage, and makes all six typed-boundary cases
required (including the ignored real-Z3 regression). Refreshed validation and
publication results are recorded there.
Integrated local acceptance: **3,892** passing quick tests under both runners
with identical identities/outcomes, **38** required verifier cases, **3,801**
library tests, **4,254** full-workspace tests, **32** controls, and all native/
browser checks pass. The full workspace's four ignored Z3 cases run explicitly
in each quick invocation.
The [gate contract](../architecture/compiler-correctness.md) documents runner,
job, and evidence boundaries. Next: A1 controls and A2 full observations;
exact-revision hosted qualification and broader per-case manifests remain A0
acceptance work. Suite counts above and prior CI/proof results are
dated; each execution record states its own evidence.

**Ledger state:** `docs/plans/2026-10-06-bugs-ledger-sweep.md` — 152
unmarked BUGS entries verified, 4 open. Fixed during the sweep: the
`List<T> + List<T>` silent miscompile (now elaborates to stdlib `iter_chain`),
the interpreter's missing `<-` push (every list accumulator was wrong in the
reference), the json-interpreter array hang + two more Rule-5 divergences
(`&&`/`||` short-circuit, trailing-expression result), the 12 stale
normalizer warnings, and the unfired-`txn` empty-program silence (build now
warns).

**UNIFIED PROGRAM (2026-11-10, TOP-LEVEL UMBRELLA):**
`docs/plans/2026-11-10-unified-program-capability-and-ceilings.md` — one
program, two meters: the kiosk acceptance suite is the **external capability
meter**; the Intel/GPU lane ceilings + float-format seam are the **internal
performance meter**; H5/verification/stability/lifetime/arenas/proofs/eternal
ABI are the **trust substrate** for both. Defines the extension rule (every
ceiling names a kiosk consumer or records `none`; every kiosk gap becomes a
compiler/lane item with a gate), one critical path (kiosk S2 tensor core until
the first native model), work classes A–D with stop/go, the unified ledger
schema, one measurement schema, edge-readiness gates, U0–U5, and the immediate
actions. References — does not replace — the kiosk program, lane ceilings,
float formats, H5, verification, stability, lifetime/arena/destination,
proof-consumption, eternal ABI, Intel G1.5, and doctrine plans.

**Next steps + lane protocol (2026-11-16, ACTIVE — M2/R/D now CLOSED):**
`docs/plans/2026-11-16-next-steps-memory-determinism-lanes.md` — the four open
engineering items in priority order. **Status 2026-11-17:** M2 (tail-loop owned
destruction) landed with a flat-RSS proof across a 16× carried-length range
(`benchmarks/results/2026-11-16-tailloop-owned-destruction.md`); R (RSS
step-growth) is answered — arena retention, not a leak, by a three-arm A/B
(`briev-kiosk-avatar/docs/results/2026-11-17-kiosk-rss-truth.md`); D (IR
determinism) is closed — the daemon compiles byte-identically across processes
and `tests/tier1/state_layout_determinism.bv` joined the corpus. Only X (six XPU
items blocked on PTL node access) and K3 (edge budgets blocked on owner numbers)
remain. Also records the
binding worktree/branch protocol (own worktree `briev-intel-xpu` on
`intel-xpu-harvest`; `main` is a shared trunk never checked out here;
fast-forward-only pushes after a fetch) and ships `scripts/sync-main.sh`, which
refuses off-branch or on a dirty tree, reports what would merge, and prints the
two known conflict recipes instead of guessing.

**Compiler messages: invalid IR as a first-class diagnostic (2026-11-18, ACTIVE):**
`docs/plans/2026-11-18-compiler-diagnostics-ir-verification.md`. Four findings: on a
clang failure the compiler prints `clang failed to compile '<f>.bv'` with no cause
(its real diagnostic goes unattributed to the terminal); `--llvm` never validates, so
invalid IR ships with **exit 0** (both of this session's codegen bugs hid there); the
emitted IR has no `!dbg` and statements carry no spans, so `%t36` cannot be attributed
to `acc_ = acc_ + x`; and the verifier already exists — in
`match_termination_tests.rs`, unused in production. Plan: M1 verify every module by
default in every mode (message says *the compiler* is at fault and leaves the `.ll` as
a reproducer), M2 `; briev:` construct breadcrumbs so a failing line names the
statement (no spans, no metadata, zero codegen risk), M3 a corpus `invalid_ir` column
that turns BUGS groups A–E from a number into 17 named diagnostics, M4 a what/why/fix
sweep starting with the three messages authors actually hit.

**Device-free queue (2026-11-17, ACTIVE — PTL unavailable):**
`docs/plans/2026-11-17-device-free-queue.md`. D1 = the loop-carried `Float` backedge
(`examples/syntax/meta/defn-composite.bv` emits `fadd fast float 0.0, %t36` with an
i64 operand; six float copy sites in `loop_engine/counter.rs`, and the deciding
question is whether the i64 slot holds a boxed float or an integer — unbox at the
copy vs fix the recording). D2 = **correct** the residency results file: `nm` refutes
the duplicate-driver-table hypothesis (`D briev_dev_opencl` only in
`libbriev_gpu_rt.a`; `U` in `libbriev_accel_rt.a`), so instrument the three branches
in `briev_accel_launch_resident_2d` and check C/Rust field-order agreement instead.
D3 = localise `g0_fsum_approx` WRONG on OpenCL statically (it differs from the exact
fixture by one keyword and the data is integer-valued, so the subgroup-sum lowering
is wrong, not the licence). D4 = GEMM input-integrity canary, the three corpus
rewrites, `pf_exp` phase 2.

**Accel surface, NEO residency, reduce lane (2026-11-17, ACTIVE — next three workstreams):**
`docs/plans/2026-11-17-accel-surface-neo-and-reduce.md`. Six instrument defects found
this campaign (dead-pid soak, shared `--out`, `.s` report pollution, `brievc run`'s
dispatch, the `LocalSize` parse, and the silent CPU fallback); this finishes that
class. **S1 accel surface:** (S1.0) survey the 21 files with `accel node` for silent
CPU fallbacks — `kernel.rs:194` warns and runs on the CPU, so those "GPU" numbers are
CPU numbers; (S1.1) make emission failure an ERROR with an explicit
`--accel-allow-cpu-fallback` opt-out, never silence; (S1.2) fix the f16 fallback store
(`store half <float>`); (S1.3) the differential correctness gate — emit `_runner.c`
for `.bv` accel programs too, and add a runner `[check]` compare against a CPU-lane
reference (cheaper and more honest than widening the kernel statement surface).
**S2 NEO residency:** fill the optional `mapped`/`launch_dev`/`launch_dev2d` driver
entries in `briev_dev_opencl.c` (the table already exists in `src/accel_rt.rs:188`),
reuse one timestamp event, gate on wall ≈ device. **S3 reduce:** a cooperative
smem tree-reduce only under the `approx` licence (0.106 → beat 0.0969), unlicensed
lane untouched.

**PTL tiled-GEMM dispatch — a correctness bug, found night 2 (2026-11-17, ACTIVE):**
`docs/plans/2026-11-17-ptl-tiled-gemm-dispatch.md`. `emit_tiled` decodes its tile
from `wgid_x` with **no bounds check**, while the emitted runner launches
`n_gemm = M·N` work items — 65536 workgroups where the tile grid has 4096. So
`tile_m` runs to 1023 against 64 real tile rows and the kernel stores 16x outside
`y`; `cooperativeMatrixRobustBufferAccess = false` makes that UB, not a clamped
discard. Unnoticed because every ceiling fixture is zero-filled ("zeros over
zeros"). Three consumers compute the launch three different ways (emitted runner
tiled lane, emitted runner coopmat lane, `prepare_run`), which is why it hid. It
also dissolves night 1's "f32 GEMM is 8.5x behind" target: 459/800 ms at 16x
redundancy is ~29/~50 ms, i.e. *faster* than the CL-C reference. Plan: a non-zero
checked fixture first (today's compiler must FAIL it), then a kernel guard, then one
geometry shared by all three consumers, then re-measure; then NEO residency.

**PTL night queue (2026-11-17, EXECUTED — see the results file):**
`docs/plans/2026-11-17-ptl-night-queue.md` — **executed 2026-11-17 evening.** Outcomes:
key auth works with `prestopoverlord@` (the earlier probe misspelled it); preflight
confirms Arc B390 with `cooperativeMatrix = true`; and **P1 overturned X1** —
`brievc run` dispatches a different grid than the emitted runner (16x for the
coopmat tier), so "coopmat is a 9x regression" was the harness. Measured properly
(build → link → run): 4096³ f16 = 26.23 ms / 5.24 TFLOP/s, 3.2x FASTER than the
hand-written CL-C reference; the shipped `coopmat: 1, f16acc: 1` default is correct
and f32 accumulation is 1.46x slower. `briev_lane.sh` now builds+links+times the
shipped binary. New top target from the re-taken table: **f32-storage GEMM, 8.5x
behind the reference at 4096³**, because that path has no tiled kernel (coopmat only
handles f16 storage). K3's cgroup check is negative on this node too. It also records a **correction that changes the experiment**:
`benchmarks/results/2026-11-09-intel-lane-ceilings.md:249` shows the f16 A/B is
coopmat **1.05/371 ms vs f16-tiled 1.70/459 ms** — coopmat WINS like-for-like — and
the `0.123/39.7 ms` figures are the **f32** tiled path. X1's "coopmat is 9× slower"
therefore compares dtypes; the real deficit is the f16 TIER being 9× behind f32,
whose recorded remedy is W3(1): an f16-storage/**f32-accumulate** tiled kernel (the
CL-C reference reaches 84.4 ms / 1628 GFLOP/s). Queue: preflight (coopmat exposure,
subgroup size, clinfo, glslang, cgroup) → P1 four-arm A/B over
`spirv_coopmat × spirv_coopmat_f16acc` plus an explicit-finish run (settles X4) →
P2 that kernel → P3 NEO residency → P5 K3 enforceability check.

**pf_exp, the uninitialised float slot, and tonight's PTL queue (2026-11-17, ACTIVE):**
`docs/plans/2026-11-17-pf-exp-float-slot-and-ptl-queue.md` — three follow-ups to the
device-free queue. (A) `pf_exp` is 172 of the corpus's 207 unproven goals, and the
multiplicity is the prelude re-contributing one contract to every native unit, not
172 problems. **Outcome: it is not a contract bug** — it is the `Int, Int`
tuple-summary gap already documented as phase 2 of
`docs/plans/2026-11-15-value-summaries.md` §3.2. Measured: adding `i >= 0` changes
nothing, and rewriting `pf_exp_digits`' post in tuple form moves the goal from
UNPROVEN to SKIPPED (`0 unproven / 142 skipped`) without proving anything — a
relabelling, reverted. The clamp still allows 9999 against a commented ±1000 cap;
recorded, not changed, because it is unverifiable until a tuple post means
something. (B)
`defn-composite`'s `store float 0` is the `None` arm of `emit_field_init_value`:
`let r: Float;` with no initializer emits an integer zero into a float slot, so
*any* uninitialised Float state field is currently a compile failure. (C) the
evening PTL queue, prepared: X1 first (its denominator question is already settled
by the audit — the target is the tiled arm's own 2.24/3.46 TFLOP/s, and the only
open question is whether the wall clock includes completion, so run each arm with
and without an explicit finish), then X2 (NEO residency, mirror the L0
`launch_dev` shape), then X3 (GLSL/XMX ceiling, same-device CL-C references), with
every number recorded and kernel knobs A/B'd via the variant gate.

**Device-free work queue (2026-11-17, ACTIVE):**
`docs/plans/2026-11-17-device-free-work-queue.md` — what to do while the PTL node
stays unreachable. Five items with gates: (1) corpus ERR triage, because the
recorded "34 ERR / groups A–E" has **no written taxonomy anywhere** and
`verify_bodies_corpus.sh` loses the file association for its `error:` bucket, so
triage is one run and one table before any fix; (2) make `--config-dir`
authoritative for data lookups (yesterday's `config/abi/` fix honours
`BRIEV_HOME` and the binary tree but not the flag, while `config_resolver.rs`
already owns the chain and `deferred_ab_gate.sh` depends on the partial-dir
shape); (3) X1's *first action* — verify the work count of both GEMM arms —
which is a property of the emitted dispatch geometry (`spirv/gemm.rs:60`) and
hence device-free, plus the provisional fact-gate row; (4) a cgroup
constrained-runner harness for K3 with the budgets as data (`UNMEASURED` is
never a pass); (5) the Data→Float invalid-width bitcast (F2). Lane note: the
FFI session owns F1–F11, so item 1 is scoped to triage plus unclaimed slices.

**Finishing grind — memory to zero + XPU ceilings (2026-11-16, ACTIVE execution plan):**
`docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` — executes the
remaining work of the post-re-land memory plan (§B) and the Intel lane
ceilings plan (§W2c/W3/W4) as one dependency-ordered grind. Records the
retirement of the HIR escape walker (non-monotone cross-pass summaries; the
frame leak is fixed by the AST map + `fresh_changed` + nested-call
attribution — 500-call soak flat 1.9→1.6 MB) and CORRECTS an interim claim:
the 12 undecided census families are §13 copy discipline (parser chain,
`packed_ints`, `arg`, `conv_attr4`, `unwrap` source, `dispatch`,
`select_path`), not syscall-bound — zero is reachable. Tracks: M1 soak truth,
M2 tail-loop destruction via `RecursiveCarrierValue` owned-arg facts, M3
census 12→0, M4 deny-at-zero, M5 conservative-drops ratchet; X1 f16 coopmat
tier (9× regression — fix or fact-gate), X2 NEO residency, X3 GLSL/XMX
ceilings, X4 `clFinish` hang, X5 Stage F promotion (unblocked by the W1 probe
fix), X6 dispatch waves, X7 OpenVINO gate on the node; K1 pin bump, K2 stale
capability ledger, K3 edge budgets. Waves 0–7 with an exit gate each.

**Kiosk native acceptance program (2026-11-10, ACTIVE — capability detail):**
`docs/plans/2026-11-10-kiosk-native-acceptance-program.md` — make Briev
capable of the entire `2d-Kiosk-Avatar` stack natively. The fork
(`briev-kiosk`) runs the original as a permanent differential oracle; the
only sanctioned external runtime is **VITRIOL** (LLM), ONNX is the model
format (Briev executes the graph), and OpenVINO is interop-now/retire-later.
Defines the proof statement, nativeness boundary + FFI provenance rule, the
W0–W12 workstreams (service runtime, Tensor/ONNX compiler+runtime, kernels/
devices, audio/media, vision/avatar, transport/WebRTC, app/client, VITRIOL,
real-time, verification), the capability/retirement ledgers, the S0–S8
stages, the model acceptance ladder, and the SLO/differential harness
discipline. Compiler lanes feed this program; its scenarios gate Briev PRs.

**H5 completion next leg (2026-11-09, ACTIVE):**
`docs/plans/2026-11-09-h5-completion-next-leg.md` — Leg B pipeline-HIR
flips DONE (termination + watchdog under the flag with AST debug oracles);
P5 pass-level snapshots DONE (`hir::render` + tests/snapshots/hir) → next:
all-three seam refactors (Defer, ArrowAssign, Lambda/Spawn) → A2 typing +
`emit_match` bodies → H5 ports
(reclamation parity first, then electronics/accel/gpu_schedule) → default
flip with owner sign-off. Nightly verification workflow lands with P3.

**IR layers + BIRD naming decision (2026-11-10):**
`docs/architecture/ir-layers-and-naming.md` — HIR is the in-memory IR;
BEAST stays the AST snapshot/pattern/distribution format; BIRD is reserved for
a round-trippable serialized HIR (adoption gates listed). Also records the
plugin-system intent (live AST; real IR text at Generated/Optimized) and the
cleanliness exit criteria.

**Unit test blocks (2026-11-10, LANDED):**
`docs/plans/2026-11-10-ergonomic-unit-tests.md` + `docs/guides/unit-test-blocks.md`
— `test domain { expect <bool>; };` (SPEC §8.11), reference-interpreter
runner, `--tests-only`/`--list-tests`/`--domain`, isolation and no-emission
guarantees; follow-ups in the completion plan below.

**Compiler completion + verification (2026-11-11, ACTIVE):**
`docs/plans/2026-11-11-compiler-completion-and-verification.md` — ordered
ledger: test-block polish → reclamation HIR parity → interpreter/wasm corpus
gates → batch_shape → large H5 ports; no-runtime-consumer decisions for
range/dataflow/destination_ownership; AST-arm deletion after the opt-out
release. 2026-11-12: the destination-ownership defer is superseded — R1 facts
(`hir::destination_ownership`) and the generalized Phase D1 model
(`hir::destination_model`) are ported with exact AST-parity suites; the
backend still consumes the AST-built plan (consumer flip pending).

**Verification program (2026-11-09, umbrella):**
`docs/plans/2026-11-09-compiler-verification-program.md` — P1 HIR
verification hardening (typed + structure invariants, always-on in debug,
re-verify after specialize), P2 compile-fail runner+corpus (comment
directives, required substrings), P3 corpus-wide differential (`-O0`/`-O2`/
`-O3`, interpreter, wasm; quick per-commit + nightly full), P4 structured
program fuzzing (typed generator, interpreter vs compiled), P5 pass-level
HIR/LLVM snapshots, P6 cvise/creduce workflow, P7 curated semantic corpus.
Rides on the H5/H4 gates and A1 typing; existing coverage inventory and gaps
are in the plan.

**Live lanes (2026-11-08):**
- **Language surfaces — `attr` + chaining (LANDED 2026-11-08: `68e4ba0a`, `a55ac25a`, `d70ebcff`, `64706531`, `ab0f6164`):** `docs/plans/2026-11-08-language-surfaces-attr-and-chaining.md` — `!>` hard-broken for owner-optional `attr` (owner registry validates known schemas), accel policy is `--accel`, chaining binds with `into` + `it` and has no positional `>>` back-refs. Bare-attr claim warnings land with the same plan (registered vocab keys only).
- **Idiom recognition doctrine:** `docs/plans/2026-11-06-idiom-recognition-doctrine.md` — §12 (three categories, five gates, silicon carve-out), W1–W5; ledger gate `tests/idiom_ledger_gate.rs`.
- **General workstream (fold idioms):** `docs/plans/2026-11-08-general-workstream-countable-body-and-folds.md` — P0 countable-body fix, P1 `approx`/`exact`, W4 why-nots, `seq` contract; gates in `benchmarks/intel_ptl/g0.sh`.
- **Compiler efficiency + Intel lane ceilings (ACTIVE 2026-11-09):** `docs/plans/2026-11-09-intel-lane-ceilings-and-probe-cost.md` — probe-gate IR is now O(#buffers) (641 MB → 204 KB, >45 min → 78 s); Level Zero REQUIRED for the five non-GEMM gates; OpenCL correct but reported (exact-fold `clFinish` hang, BUGS.md). Ceilings measured (OpenCL C refs): ~130-160 GB/s, f32 SIMD GEMM ~1.46 TFLOP/s, f16-storage/f32-acc ~1.63 TFLOP/s at 4096³. Briev-side matrix measured (Vulkan ticks fixed: 52.0833 ns/tick): Vulkan f32 GEMM 0.29x/0.42x of the CL-C reference at 512³/4096³; elem at parity; L0 ~2x slower; NEO 2.5-5x faster than the simple reference; f16 GEMM is the outlier (Vulkan 4-5x, L0 11.7x). f16 GEMM now takes the coopmat tier (declared-kind + cast-tolerant detection); device A/B recorded (coopmat 1.05/371 ms vs f32-tiled 0.12/39.7 at 512³/4096³) — f16 microkernel quality is the open gap. L0 residency landed (wall ≈ device). Next: f16 tiled microkernel + measured coopmat strategy facts, NEO residency, GLSL/XMX ceilings.
- **Intel XPU / G1.5:** `docs/plans/2026-11-08-intel-g1.5-opencl-flavored-spirv.md` — Stage E PASS (OpenCL-flavored IL). Stage F design probed 2026-11-08 (`benchmarks/results/2026-11-08-intel-g1.5-stage-f-design.md`): OpenCL needs Physical64+OpenCL, state as a CrossWorkgroup kernel arg, and byte-offset `PtrAccessChain` state access (Block/Offset require Shader); first slice LANDED 2026-11-08 (`benchmarks/results/2026-11-08-intel-g1.5-stage-f-slice1.md`): flavor + kernel-arg state + byte-offset access + dual-image descriptor (`opencl_spirv`/`opencl_size`, driver preference) for non-GEMM/non-image kernels; spirv-val clean on `.abv` and `.bv` artifacts. On-device 2026-11-08/09: BOTH Level Zero and NEO/OpenCL compile + launch the projection and return correct results (`g1_lane_smoke.abv`). Gate promotion is blocked by a pre-existing 641 MB / >45 min link on the const-N `.bv` gates, not by Stage F. Vulkan remains the working lane.

**Active stability lane (2026-10-25):** `docs/plans/2026-10-25-stability-v0.1.md` — freeze features; make `cargo test --all` a hard gate, close interpreter/backend parity, declare Tier 1 = x86_64 Linux + wasm32, add a nightly performance baseline, and ship `v0.1.0-preview` then `v0.1.0`. GPU/embedded remain experimental; Otto’s server stays external.

**ws_frame safe fast path (2026-11-03, Step 1 DONE — objective revised):**
`docs/plans/2026-11-03-ws-frame-fast-path-execution.md` — execution plan,
Steps 0–3: I11 gate-relaxation root cause → attribution A/B → Proof 1
image-ownership threading → Proof 3 boundary three-condition proof
→ Proof 2 `ws_partial` caller-destination. **Verdict:** the "2x vs baseline"
regression is a phantom — the locked baseline `42279aaf` is numerically
**wrong** (dead-frame UB; `PASSES=1` checksum `7441` vs C's `103005804`;
segfaults on payload-heavy frames), while the current tree is correct and at
**C parity** (C 78.65 M / 20.4 ms vs current 85.22 M / 23.0 ms). Proof 1 is
re-scoped to the ≤ ~2.6 ms current-vs-C residual; Proof 2/3 are correctness
work, off the `ws_frame` path. Measured data:
`benchmarks/results/2026-11-03-ws-frame-attribution.md`; evidence/proofs:
`docs/plans/2026-11-03-ws-frame-safe-fast-path.md`; benchmark flaw in
`BUGS.md` (even `PASSES` hides checksum errors). **Step 0 DONE:** I11
root-caused — enum images escape to state without `own_enum_value`
(`emit_stmt.rs` state-store paths); a defn-local sret image stored to node
state. A defn-local sret image stored to node state dangles after the defn
returns. Confirmed by IR A/B (`/tmp/opencode/repro/defn_store.bv`: garbage
under relaxation, `42` after owning). Not layout divergence. Fix = Proof 3
condition 2 (own at every escape sink); the boundary gate stays mandatory
until then. See `BUGS.md`.

**Enum boundary-gate removal (2026-11-03, ACTIVE):**
`docs/plans/2026-11-03-enum-boundary-gate-removal.md` — finish Proof 3 so the
blanket gate becomes `conditions1∧2∧3` (layout agreement ∧ escape ownership ∧
no external raw-byte consumer; `Unknown ⇒ box`). Phases: A analysis
(transitive enum-handle containment + txn-local/persistent lifetime facts) →
B reclamation (txn arena for txn-local, destination threading / managed slot
for persistent state) → C emission at tuple/closure/aggregate/collection/state
sinks → D gate replacement (export/FFI-only boundary) → E verification
(minimal escape repros, ASan/UAR, LSan, net_stack O0–O3, determinism, Kani,
runtime A/B). State-store sink already fixed (`c4a5f4a1`); rest of Proof 3 is
this workstream.

**Arena runtime parity + allocation folding (2026-11-03, ACTIVE):**
`docs/plans/2026-11-03-arena-runtime-parity-and-folding.md` — two goals:
(R) runtime parity/better than the C arena on an observable workload —
register-resident bump/end (local cursor, SROA-promoted), cold grow, scope
reset; (O) keep proof-driven dead-allocation folding via the escape facts
(restores the ~18ms fold where semantics allow). Evidence: current arena is
~1.4x C (338 vs 240ms @50M); the baseline's 18ms was a legal fold.

**Arena enablement (2026-11-03, ACTIVE):**
`docs/plans/2026-11-03-arena-enablement.md` — make `Alloc#` actually select a
safe arena. Root cause: two analysis passes stamp conflicting `Alloc#` ids.
Fix reuses the existing loop convention (dedicated latch/end blocks +
captured final block) at the two hardcoded sites, then fixes the arena
subsystem (Direct-SSA init, brk-aware fini, grow preserves the bump) and
re-enables selection behind an IR gate. Prerequisite for per-worker arenas.

**Saltpetre ML library (2026-11-12, LANDED v0):**
`docs/plans/2026-11-12-saltpetre-ml-library.md` +
`docs/architecture/saltpetre.md` — the kiosk `briev/ml` stack is now
`lib/saltpetre/` in-repo (tensor/proto/onnx/exec/frame/yunet verbatim; NEW
`session.bv` owns both request-arena lifecycles: `sp_begin`/`sp_request_reset`/
`sp_run`/`sp_end`). Tiny ONNX corpus conformance gate in-repo
(`tests/saltpetre_conformance.rs`, atol/rtol 1e-3, no PTL); kiosk acceptance
stays the external capability meter. Saltpetre is the OpenVINO-retire vehicle.

**Memory decisions + auto-free (2026-11-10, ACTIVE — M2 in progress):**
`docs/plans/2026-11-10-memory-decisions-and-auto-free.md` — plain `Alloc#()`
is the author surface; the compiler picks arena/stack/malloc when feasible.
Provenance + interprocedural escape summaries; prove dead sole owners and
auto-free (reported in `memcheck`, `keep` overrides); warn on undecided
owners, deny in kiosk CI; no GC/refcount; no new `store` keyword. LANDED:
request coll arena (C runtime, weak entry points), String-ownership slices
(§14–15: read_whole/stdlib copy lanes/strip_frac_zeros + HIR fresh-summary
pass `hir::owned_storage`), owned List destruction + temp-transfer (§16:
`ReleaseOwned`/`TempTransfer` first-class HIR kinds, body-seam two-cursor
walk, fresh-set gated on zero escaping params, identity-return transfer).
Kiosk face soak: 144→187 MB pre-slices → flat 171 MB at 200 calls. DONE SINCE: guard-nested seam (§17), owned List + temp-transfer (§16),
tensor dims owner slot + cached-model session, VAD chunks RPC + sessions,
coll Float fix, Saltpetre op ledger, census campaigns 1–2 (undecided
147→12, releases 58, ratchet live), coll-object owns-check + self-tail fix.
NEXT: post-re-land hygiene, soak truth, tail-loop-aware destruction, census
12→0 then deny-at-zero (`2026-11-14-post-reland-hygiene-soak-tailloop.md`).
STT/TTS/visemes + NPU await owner decisions.

**Lifetime and reclamation — north star (2026-11-03, ACTIVE):**
`docs/plans/2026-11-03-lifetime-and-reclamation.md` (+ decision-record Part
XIII, D27–D33). The long-term ownership/health model: lifetime is a proof in
the destination plan (`Frame`/`TxnBody`/`StateField`/`LoopCarrier`/`Program`/
`External`); allocation follows lifetime (construction-time strategy via the
`Alloc#` escape DAG); reclamation follows lifetime (txn arena, owned state
slots, boxed/leak for Unknown); immutable images with copy-on-escape or
destination threading. No GC, no pervasive refcount, no new syntax. Phases
0–7: correctness net (closure-let gap) → **execution substrate (parallel
`async` via `SysCall#(SYS_clone)`+futex, per-worker arenas in worker-indexed
slots, real `sync<group>` join, capability-gated cooperative fallback)** →
`Lifetime`/`Sendable` facts + per-worker allocation strategy → construction-time
strategy → state slots → observability (`memcheck`) → optional sound refcount →
enum boundary-gate removal. Decisions: parallel by default where threads exist;
shared `%State` safe for fields via the gate's XOR-disjoint proof; async output
interleaving contract-unspecified. This is the durable plan the enum gate-removal
workstream executes; the two share Phase 0.

**Typed HIR H3–H5 execution (2026-11-04, ACTIVE) + H4 completion (2026-11-05):**
`docs/plans/2026-11-04-typed-hir-h3-h5-execution.md` and
`docs/plans/2026-11-05-h4-completion-loop-convergence-swan-song-emitter.md` —
H3 COMPLETE (every analysis over HIR at parity; unsupported surface ZERO —
PluginIntercept is a represented node, audit ratchet at 0, all parity suites
cover the full corpus). H4 in progress: 7 consumers flipped (coll proofs,
defn liveness, boundary ownership, tail plans, spawn pools, task segments,
loop shapes + program convergence) and the statement seam emits 14 native
expression families (defn + txn loops), all behind `BRIEV_HIR_EMIT=1` with
byte-identical A/B + full-corpus debug sweep. Remaining: native control-flow
statement arms (Let/Assign/Term/Guarded/Foreach/Match), structural expression
composites (register-taking refactors), H3.6 backend wiring (specialization),
MethodCall/Reflect receiver typing (HIR `Cast.via`), H5 retirement.

**H5 retirement inventory (2026-11-05, PLANNING):**
`docs/plans/2026-11-05-h5-retirement-inventory.md` — the H5 work list:
what still reads the AST (emitter entries, declaration/ABI surfaces, the
to_ast bridge, Defer/ArrowAssign/Match-expr conversion), the retirement
mechanics (flip default → family-by-family arm deletion → entries switch
→ bridge delete), preconditions for the flip, and the forever
invariants. Update as families land; it IS the H5 work list.

**Typed HIR H4/H5 progress (2026-11-05, ACTIVE — updated):**
Statements: 21/22 kinds native through the seam (only Defer conversion,
by design). Expressions: every reconstructible kind seam-covered; 21
families native (incl. match dual-fuel — arm bodies + scrutinee via
operand recursion; index; slice; block; tuple; struct-literal; arrow
dual-fuel via `ArrowAssignValue`). H5 ports flipped + assert-gated:
web_commits, modulo_partition, density, dyn_concrete_sets,
global_lifetime; termination port-only (pipeline-HIR batch); batch_shape
transitive. Remaining H5: accel-blocked pair (coalescing, image_storage),
gpu_schedule, watchdog, electronics, reclamation lineage. Emission arcs:
Cast.via typer (MethodCall/Reflect receiver typing), H5 flip batch
(pipeline-HIR for termination flip, then default flip per inventory
preconditions).

**Collaborator integration (2026-11-05, DONE):**
`docs/plans/2026-11-05-collaborator-integration-and-h4-resumption.md` — all
nine of Otto's PRs merged (#63–#71): stdlib measurement/network modules,
reclamation lifetime facts + socket/method effects, field-receiver
single-eval, statement-match fallthrough (HIR mirror landed), tuple value
types (ported into emit_let_statement). Full battery green at `364f42eb`
(lib 3512, 101 test binaries ok, sweep 131/0, A/B byte-identical,
determinism, daily_use_smoke 37/37).

**Typed HIR (2026-11-02, active):**
`docs/plans/2026-11-02-typed-hir.md` — parser AST remains; typed normalized HIR
becomes the semantic source for analyses and lowering.

**Language stabilization (2026-11-02, active):**
`docs/plans/2026-11-02-language-stabilization.md` — re-triage OPEN core bugs,
fix enum/collection ownership and reference-interpreter parity, then unquarantine
the full test lane.

**Canonical enum payload + selective inline (2026-11-01, closed 2026-11-02):**
`docs/plans/2026-11-01-canonical-enum-payload-and-selective-inline.md` —
D4b enabled with PR49/50/51 owned extraction; callable-boundary gate mandatory;
copy elision removed after a controlled regression. Next performance work is
region-local enum/object scalar replacement.

**Generalized destination implementation (2026-10-31, merged 2026-11-02):**
`main` at `4dfa4e85` plus integration closeout. Phases D1–D7 landed; D8 cleanup
complete. Historical execution queue:
`docs/plans/2026-10-31-destination-ownership-remaining-work.md`.
Umbrella: `docs/plans/2026-10-30-generalized-value-destination-ownership.md`.
ADR: `docs/architecture/value-destination-ownership-decision-record.md`.
Migration uses generalized value lanes with frozen public/runtime ABI.

**Active destination/ownership reconciliation (2026-10-29):**
`docs/plans/2026-10-29-destination-ownership-reconciliation.md` — supersedes
ad-hoc ObjectDestination metadata; reconciles small-sum unboxing, nested
layout, carrier slots, tail-loop lowering, ownership algebra, boundary
ownership, and P2-1/P2-2 into one frontend `ValueDestinationPlan`.

**Active enum model (2026-10-29):** `docs/plans/2026-10-29-enum-value-model.md` —
hybrid ScalarValue/PackedMemory/OwnedBox migration replacing internal enum
handles; Stage 0 classification/metadata complete, list-alias defect recorded.

**Active release ledger (2026-10-28):** `docs/plans/2026-10-28-v0.1-preview-readiness.md` — execution checklist and exit criteria for PR #1 and `v0.1.0-preview`: baseline snapshot, gate integrity, interpreter parity, Tier 1 platform contract, performance A/B, app acceptance, release engineering.

**Active recommender lane (2026-10-25):** `docs/plans/2026-10-25-recommender-fixes-and-app-adoption.md` — A/C/D compiler/web/WASM bundles landed; casting change rejected; generic txn specialization deferred. App remains external and pinned at `79234665`. Source compiler commit: `894fcfa5`.

**Session report (2026-10-27):** `docs/2026-10-27-session-report.md` — chronological record of app compatibility, compiler hardening, proof elimination, ledger, and remaining browser/hardening work.

**Active JSON/runtime lane (2026-10-25):** `docs/plans/2026-10-25-json-wasm-runtime-closure.md` — strict JSON, browser heap, dynamic snapshots, helper-write flushes, lexical elaboration, logical semantics, metadata and target-width constants are landed. Phase D stdlib breadth (PostgreSQL, portable web, CSR) is adopted; the remaining compiler item is generic callable transaction specialization.

**Active proof-completeness lane (2026-10-26):** `docs/plans/2026-10-26-proof-completeness-and-guard-elimination.md` — make the prover discharge constructible obligations and let the compiler consume them. Source guards are retained; proof-aware guard elimination is implemented (Rule 25).

**Active eternal-ABI lane (2026-10-27):** `docs/plans/2026-10-27-eternal-abi-and-temporal-transport.md` — centralize target ABI/layout, verify emitted IR, make transport/timers explicit. Phase C (verifier/invariants) is first.

**Active proof-consumption workstream (2026-10-27):** `docs/plans/2026-10-26-proof-consumption-ledger.md` — every proof family must name its codegen consumer (or an explicit no-runtime-consumer record). Instrumentation landed: 176/176 assertion guards consumed, 0 unconsumed. Accessor bounds are `NONE (static contract; no runtime check)`; loop/constructor/alias consumers remain open.

**Active app acceptance lane (2026-10-26):** `docs/plans/2026-10-26-recommender-app-acceptance-and-upstream.md` — recommender app builds against current main (PR #1, draft); compiler prerequisites `e442f06b`/`1c1dd70f` pushed to Randozart main. Remaining: `cargo test --all`, non-PostgreSQL app gates, then PG/MovieLens/Playwright acceptance and the upstream landing decision.

**Active adoption lane (2026-10-25):** `docs/plans/2026-10-25-otto-contribution-adoption.md` — adopt Otto Westerlund's missing compiler/stdlib/browser work on current main, starting with attribution, Result/print/view/collection fixes, then JSON, PostgreSQL, portable web stdlib, logical/metadata semantics and browser runtime. Credit ledger: `docs/contributors/otto-westerlund.md`.

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

**Open work backlog (2026-10-20):** `docs/plans/2026-10-20-open-work-backlog.md`
— the consolidated P0–P4 list after the pointer-provenance fix and the four
stdlib fixture first passes. Start there for the current queue; this index
remains the map of the older umbrellas and live lanes.

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
| ~~`feat/billd-dialect`~~ | — | **MERGED 2026-10-08; RETIRED 2026-10-19** — `.bld` dialect deleted; symbolic assembly+ lives inside `.bad`. Plan `2026-10-08-billd-intermediate-dialect.md` (historical); retirement + forward plan `2026-10-19-bad-assembly-plus.md` |
| (baseline) | `../briev-compiler-baseline` | Rule 12b A/B worktree — measure `main` only |

Standing exclusion (`2026-09-24-followup-stages.md` §Exclusion): never merge,
never touch; all baselines/gates measure **main only**.

---

## FFI contract + metaprogramming (2026-11-16, plan of record)

`2026-11-16-ffi-contract-and-metaprogramming.md` — the universal FFI surface:
ABI cores (`config/abi/`) + eight boundary value classes + per-language adapters
(`lib/glue/<lang>/glue.dbv`) + generated protocol bindings, with the
metaprogramming layer (`Quote$`/`Insert$`/`InjectTypeLayout$` at stage
`Resolved`) doing the generation. Records the measured pipeline ordering that
forces the stage choice, and the HIR gap: `HirTypeDefKind` has no protocol
variant and `HirForeign` drops the `from`/flags/contracts, so the HIR path is
blind to the boundary surface — measured, with `HIR_AUDIT ok … unsupported=0` on
a boundary-bearing program. Decisions D1–D6 (ownership per function; ABI cores in
config; versioned descriptor with unknown-key errors; the removed-surface gate
before the generator; facts never strategies; generated items traced to a
descriptor key path). S0 spike: generated protocol drives zero-copy
marshalling (PASS); aggregate layout injection still unmeasured.

**Status (2026-11-16):** S1–S3 landed (versioned descriptor, ABI cores,
`brievc render <lang>` + `--check`, C adapter split). **G4′ parity gate
PASSES** — generated vs hand-written protocols emit byte-identical IR over
both lanes (pins program structure, not just text). G5 resolved the
dangling `types_module`. **H1–H4 landed**: `HirProtocol`/`HirBoundary` /
`HirFromSpec` on `HirForeign`; the audit counts the remaining gap (`cross_ops`) —
the BUGS.md fixture now reports `unsupported=1 (protocols=1 boundary=0)` instead
of the old `unsupported=0`; the destination model reads `HirBoundary` directly.
**S5 stage guard** landed (structural synthesis refused after `Resolved`; `defn`
bodies face `verify_bodies`). **PY closure test passes**: Python adapter's
`values:` block renders with zero Rust changes. Remaining: R1 (doc matrix —
in progress), D1/D2 (removed-surface gate + sweep), S4 (populate
`HirProtocol::ops` to drive the audit's protocol count to 0). Full status +
touch points: `2026-11-16-ffi-handoff-remainder.md`.

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

## Workstream 0 — Stability remediation (2026-10-16)

Plan of record: `2026-10-16-stability-remediation.md`. From the
cpp-vs-briev session's seven weaknesses: S1 `.ll` determinism gate →
S5-A toolchain matrix → S2 reserved-word diagnostics → S3 reactive
recipes doc → S6 config ledger → S4 shared body-emission prologue
(prologue-only; walkers untouched). Standing guards after S1/S4: the
determinism script on backend-touching commits.

| Piece | Status | Next |
|-------|--------|------|
| cpp-vs-briev fixes (flaw 4, memset, node-inline valve, frame limit, defect ledger) | **DONE** (`93be0795`, `a42de3c0`, `6e3a78ef`, `63107709`, `58b292ca`) | — |
| S1 determinism gate | **DONE** (`e5dd108c`) | standing per-commit guard |
| S5-A toolchain matrix | **DONE** (`d7740513`) | pre-release/backend ritual |
| S2 reserved-word diagnostics + recovery cascade | **DONE** (`6d9d1cea`) | — |
| S3 reactive recipes | **DONE** (`14d976d5`) | — |
| S6 config ledger | **DONE** (`27ff8d0c`) | sweeps → followup-stages |
| S4 shared prologue (5 drivers) | **DONE** (`d7e2b10f`, fixtures `44a75ddc`) | byte-identity held; SSA-main aggregate cross-referenced |

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

**`.rbv` dev loop (2026-10-08, stabilise-for-use T3):** `brievc watch <dir>
[--split] [--once]` — rebuild the page set on change (dev loop). Thin
orchestration over the per-file build (Part 1 seam) + the existing `src/watch.rs`
debouncer (notify 6.1) — no new codegen, no new deps. `--once` is the testable
path. Worked example beyond the counter: `examples/todo-list.rbv` (multi-
component, `b-each` over an obj vector + a member txn). Gate: `rbv_gate.sh`
step 5c. Plan: `docs/plans/2026-10-08-stabilise-for-use.md`.

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

- **Term semantics + tails — LANDED 2026-10-16** (plan
  `2026-10-16-term-semantics-and-tails.md`, steps 2–8; commits `9915c098`,
  `72a80571`, `84d37a38`, `5fa723ca`, `442fbd31`): P-gated bare `term;`
  (interpreter + backend, BUGS.md divergence resolved), defn-body tail
  expressions (`Statement::Tail`), arrowless output inference
  (`typechecker::output_inference`), the state-purity probe
  (`analysis::purity`, first `diagnostics::Block` consumer), SPEC
  §9.1/§9.3/§9.4 term rule + docs. Step 6 note: the `fn_return_types`
  Int fallback for arrowless callables remains for the no-value-site
  shape (pre-existing wart, documented).
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
| `2026-10-18-readme-philosophy-refresh.md` | COMPLETE 2026-10-18 — README pillars made honest from the owner interview (record: `2026-10-18-philosophy-interview.md`; 13 Q&A + consistency verdict + 7 seams, 2 rulings), dialect stability table (Stable `.bv`/`.dbv`, Functional `.abv`/`.rbv`, In dev `.ebv`, Experimental `.sbv`), auxiliary `.bad`/`.bld`/`.beast` documented, @odecode listed as contributor; `docs/architecture/philosophy.md` refresh deferred |
| `2026-10-16-term-semantics-and-tails.md` | Steps 1–8 DONE 2026-10-16 (`9915c098`…`442fbd31` + docs commit) — P-gated bare term, tails, output inference, purity probe, SPEC/execution-model docs |
| `2026-10-16-cpp-vs-briev-contributor-findings.md` | COMPLETE 2026-10-16 (`35092fe1`…`d79afb94`) — flaw-4 fix, memset threshold, node-inline valve (measured), frame limit, defect ledger, recommender benchmark + corpus |
| `2026-10-16-stability-remediation.md` | ACTIVE — S1 gate → S5-A matrix → S2 diagnostics → S3 recipes → S6 ledger → S4 prologue |
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
