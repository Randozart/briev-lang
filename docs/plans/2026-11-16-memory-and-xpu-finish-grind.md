# The finishing grind: memory ownership to zero + XPU lane ceilings (2026-11-16)

**Status:** ACTIVE. Waves 0–2 landed 2026-11-16 (see §2.0). Two tracks, one
critical path, one measurement discipline. Supersedes nothing; it *executes*
the remaining work of `2026-11-14-post-reland-hygiene-soak-tailloop.md` (§B) and
`2026-11-09-intel-lane-ceilings-and-probe-cost.md` (§W2c/W3/W4).

## 2.0 Wave 0–2 outcome (2026-11-16)

| item | state | evidence |
|---|---|---|
| M1 soak truth | **done** | `benchmarks/results/2026-11-16-kiosk-soak-truth.md` — and it CORRECTED an earlier false result: the "flat 1.9 MB" curves were sampling a dead pid (`sample_vmrss_mb` returned 0.0 and the loop accepted it). Harness now asserts pid liveness + `comm` + aborts on vanish. |
| K1 pin bump | **done** | pin `4aed72809` → native/ML/VAD RPC/VAD sessions/dashboard 624 calls/ratchet all green; 500-call soak flat |
| M3 census 12 → 0 | **done** | `2026-11-16-owners-census-zero.md`; releases 65 → 73, drops 2200 → 2196 |
| M4 deny-at-zero | **done** | `brievc memcheck --deny-memory-ambiguity` exits 0; wired into kiosk CI before the ratchet |
| M5 drops ratchet | **done** | three counters/three directions; both new failure modes probe-tested |
| M2 tail-loop | **done (2026-11-16)** | `benchmarks/results/2026-11-16-tailloop-owned-destruction.md` — landed as a frontend fact (`OwnedTailParam` / `AnalysisResults::owned_tail_params`) + per-target loop lowering; 4k/8k/16k/32k carried length → 12.6/12.8/12.8/13.0 MB flat (pre-fix 12.9/33.7/127.9 MB, quadratic). Gate: `daily_use_smoke::owned_tail_transfer_frees_at_every_backedge`. |

**What M2 taught (record it so nobody "simplifies" it back):** freeing the
slot value at the backedge also frees the CALLER's argument on iteration 1 —
the slot is initialised from the caller's handle. Two mechanisms are needed and
both exist now: (a) a **transfer proof over EVERY call site in the program**
(inline `fresh` allocation, a local the caller mentions exactly once, or a
literal), which alone licenses freeing the first backedge; and (b) a per-slot
`first` bit for the slots where the first value is a caller literal nothing
owns. A fact is recorded ONLY when both the loop can be lowered AND every site
passes the transfer proof — otherwise the leak stays in the census. Do not
widen the transfer proof to make the census look better: the unsafe direction is
a use-after-free in the caller, not a leak.

**Mandatory first reads:** `AGENTS.md` (Rules 12/12b/20/25),
`docs/plans/INDEX.md`, `docs/plans/2026-11-10-unified-program-capability-and-ceilings.md`
(§3 work classes, §7 edge gates), the two plans above, and the kiosk
`docs/progress.md` (the last six entries are the ground truth of what is
actually native).

---

## 0. Corrected record of 2026-11-15/16 (read this before re-planning)

The session that preceded this plan changed the picture in two ways. Both
must be recorded or the next session will re-derive them.

### 0.1 The HIR escape walker is RETIRED (and the leak is still fixed)

`43c11b6a4` landed `summarize_escapes_hir` and merged its summaries over the
AST escape map. It **crashed the daemon** (`free(): invalid pointer`) on the
second soak, and removing it alone did not fix the crash — the walker's
output was the fault.

Root cause (verified with pass instrumentation): the fixpoint is
**non-monotone across same-pass cross-call edges**. `escape_hir_stmts` reads
`summaries` (previous pass) while writing `next_summaries` (this pass), so
when A calls B, A sees B's *stale* summary for one pass. The result is a
permanent oscillation, not a slow convergence:

```
[hir-escape] STILL CHANGING at 100: init arity=1 old=[false] new=[true]
[hir-escape] STILL CHANGING at 100: init arity=1 old=[true] new=[false]
[hir-escape] WARN: not converged after 2001 passes (662 callables)
```

A second, independent defect was found and fixed en route but was **not
sufficient**: the summary map was keyed by name only, so same-named
overloads (`init()`, `init(x)`; `contains(x)`, `contains(x,y)`) overwrote
each other and drove their own oscillation. Arity-keying removed those two
and left four genuinely non-monotone callables.

`00fcd5e90` removed the walker and its 321 lines of dead helpers, and
restored `is_borrowed_intrinsic` to private. `d4cbc9c49` seeded
`fresh_string` in `fresh_callables`.

**What actually fixes the raw-frame leak is the AST escape map plus two
smaller corrections** already in the branch:

1. `fresh_changed` in `fresh_callables` — `ok0` was a stale snapshot taken
   when `fresh` was empty, so a newly-fresh allocator (`tensor_new`) never
   unlocked its ok-fresh deduction in later passes. Recomputed when `fresh`
   grows.
2. Nested-call attribution in `use_arg` — `tensor_set(t, i, f32_from_bytes(text, 4*i))`
   attributed `text` to `tensor_set`; nested expressions must be analyzed by
   their own callee.

**Evidence (this is the number to preserve):** 500-call soak on the
walker-free daemon, `/tmp/soak-build12/daemon`, binary frames 640×640:
VmRSS `1.9 → 1.6 MB` (`Δ = -0.3 MB`), no crash, all 500 calls served.
Pre-fix the same geometry grew `+4.07 MB/call`.

### 0.2 CORRECTION: the 12 undecided families are NOT syscall-bound

An interim claim this session said the census residual was all socket/syscall
work and therefore intractable. **That was a misread of the memcheck output
layout.** The `escapes via syscall` lines belong to the *conservative drops*
block (leaks by design, 2200 of them). The actual undecided set is:

| # | family | blocker |
|---|---|---|
| 1 | `next in parse_shape_loop` | `parse_dimension_loop` not proven fresh |
| 2 | `next in parse_tensor_type_loop` | `parse_shape_loop` not proven fresh |
| 3 | `next in parse_type_loop` | `parse_tensor_type_loop` not proven fresh |
| 4 | `next in parse_value_name_loop` | `parse_type_loop` not proven fresh |
| 5 | `next in parse_attr_loop` | `packed_ints` not proven fresh |
| 6 | `next in parse_tensor_loop` | `packed_ints` not proven fresh |
| 7 | `a in entry_cmd_walk` | `arg` not proven fresh |
| 8 | `pads in conv2d` | `conv_attr4` not proven fresh |
| 9 | `ps in maxpool` | `conv_attr4` not proven fresh |
| 10 | `name in method_name` | `unwrap` not proven fresh |
| 11 | `response in handle_conn` | `dispatch` not proven fresh |
| 12 | `chosen in entry` | `select_path` not proven fresh |

Every one of these is §13 copy discipline inside stdlib/saltpetre — the same
class the campaign has already closed 50 → 12. **Zero is reachable.** This
plan's M3 is that work.

Current census on the pinned daemon:
`releases=65`, `undecided=12`, `conservative drops=2200`,
`static-backed locals=9`.

`--deny-memory-ambiguity` (verified `src/main.rs:1972`) fails **only** on
undecided, never on conservative drops. So undecided=0 is exactly the gate.

---

## 1. The two meters, and the ordering rule

Per the unified program: the kiosk acceptance suite is the **external
capability meter**; the Intel/GPU lane ceilings are the **internal
performance meter**. Both are starved by the same two things — a compiler
that leaves ownership ambiguous (soak numbers are not trustworthy) and a
lane whose ceiling is unmeasured (perf claims are not attributable).

**Ordering rule (unchanged, binding):** do not start a performance item
while a memory-correctness item on the same measured path is open. The
kiosk soak is the ground truth for *every* XPU number taken through the
daemon; a leaking harness invalidates the measurement. Memory first, then
lanes, with the XPU items that are *unblocked and cheap* pulled forward into
wave 0 as records.

**One critical path at a time** (unified §3). Within this plan the critical
path is **Track M**. Tracks X and K are in-band: each item must name the
kiosk stage or lane gate that consumes it, or it is deferred.

---

## 2. Track M — memory ownership to zero (critical path)

### M1. Record the soak truth (do first, ~30 min)

The 159→188 MB stair-step from the 2026-11-14 plan is now explained by
measurement, not hypothesis: with the leak fixed the curve is flat from call
1. Write it down so it stops being re-litigated.

- File: `benchmarks/results/2026-11-16-kiosk-soak-truth.md` (compiler repo)
  and a matching entry in kiosk `docs/progress.md`.
- Contents: command, geometry, frame bytes, commit, the
  `drops/undecided/releases` census at that commit, the 500-call table, and
  the pre-fix contrast (`+4.07 MB/call` @640², `+1.29 MB/call` @320²,
  `+0.13 MB/call` JPEG).
- **Exit:** the two curves (pre/post) exist in one results file with
  same-geometry correctness evidence (face count `0`, `error: None`).

### M2. Tail-loop-aware destruction (replaces the self-tail exclusion)

**LANDED 2026-11-16.** Outcome: `OwnedTailParam` +
`AnalysisResults::owned_tail_params` in `src/hir/owned_storage.rs`, consumed by
`tail_loop_enabled` / `try_emit_tail_loop_backedge` / `emit_term_value_tail`.
The release reuses the `ReleaseOwned` lane, so the arena skip and the
`coll_destroy` gate apply unchanged. Evidence:
`benchmarks/results/2026-11-16-tailloop-owned-destruction.md` (flat RSS across a
16× carried-length range; pre-fix quadratic). The route below was superseded on
two points, recorded so the next reader does not re-derive them:

* The carrier plan (`RecursiveCarrierValue`) was NOT the vehicle. The fact is a
  per-defn BTreeMap keyed by callable name, because it must also be readable by
  the ownership pass itself (the census event and the fix must agree) and by the
  gate extension, neither of which sees the carrier plan.
* The first iteration's value is NOT "the caller's and never released here" as
  a blanket rule. It is released whenever the call sites prove transfer; a
  `free_first` bit covers the remaining case (a caller literal). "Never release
  iteration 1" would have left one allocation per top-level call leaking.

The original text follows.

The self-tail exclusion is a real leak with a main-lane test documenting the
intended behaviour (`tests/tier1/owned_tail_cleanup.bv`, comment: "ownership
cleanup after a recursive result must run after the call returns"). It
passes only because the golden checks lengths, not memory.

Why the naive attempt failed (recorded so it is not retried): removing
`!self_tail` in `src/hir/owned_storage.rs:1507` and adding a natural-release
arm emitted `ReleaseOwned` for args the callee already frees — `text` passed
to `load_frame_bin` is freed *inside* the callee, and the caller cannot know
that without interprocedural facts. Result: double free.

**The correct shape** is a frontend fact consumed by the backend, and the
vehicle already exists: `RecursiveCarrierValue` in
`src/analysis/destination_model.rs:407` already carries `arg_classes` and
`requires_backedge_copy` from `src/hir/tail_carrier.rs`. Steps:

1. Add `owned_backedge_args: Vec<bool>` to `RecursiveCarrierValue` (or a
   `RecursiveCarrierArgClass::OwnedTransfer` variant). Set it from the
   owned-storage pass's own per-arg verdict at the self-tail call site —
   never from a name/type heuristic (Rule 19/23).
2. Tail-loop lowerer: after the header rebind of an owned arg slot, emit the
   release of the **previous** iteration's value; at the loop exit emit the
   release of the **final** value. The first iteration's incoming arg is the
   caller's and is never released here.
3. Delete the `!self_tail` exclusion with a provenance comment pointing at
   this plan.
4. Gate: turn `owned_tail_cleanup.bv` into a memory-checked gate (malloc
   counter assertion or a golden that includes a freed-count line), and
   re-assert `strip_frac_zeros` (the real stdlib shape) by inspecting the
   emitted IR for the release-before-backedge.

**Fallback if (1)+(2) proves larger than a day:** apply the source-level
discipline that already worked for `strip_frac_zeros` — make the recursive
arg a non-owned temporary at the source (true tail position) — and record in
the plan that the compiler-side fact remains the durable fix. Do **not**
re-add the exclusion silently; it is documented debt either way.

- **Exit:** `owned_tail_cleanup.bv` is memory-checked and green; the
  exclusion is gone; 500-call soak still flat; `cargo test --lib` green.

### M3. Census 12 → 0 (batched, ordered by leverage)

Each batch is §13 copy discipline; after each batch re-run the census and
commit the new number. Order is chosen so each batch unblocks the next.

**M3a — the parse loop chain (5 of 12).** `parse_dimension_loop` →
`parse_shape_loop` → `parse_tensor_type_loop` → `parse_type_loop` →
`parse_value_name_loop` is one mutual chain; fix the base first
(`parse_dimension_loop`, likely a `fresh_list` over a param-return base or a
loop-rebound accumulator) and the four downstream families discharge
together. Same batch: `packed_ints` (2 families).

**M3b — the small attr helpers (3 of 12).** `arg` (literal `fresh_string("")`
base), `conv_attr4` (returns a 4-element list; make every return path fresh).

**M3c — the tail three (3 of 12).** `method_name` (`unwrap` passthrough
exists; the *Result source* is not fresh — fix upstream in `json_get` /
`resolve_str_val`), `dispatch` (the response-builder pilot already exists;
make all match arms fresh), `select_path` (config String selection).

- **Exit:** `undecided owners (0)`; `releases` strictly increased; ratchet
  baseline updated downward **deliberately** in the same commit.

### M4. Deny at zero

- Update `briev-kiosk/harness/memcheck-baseline.txt` to the new
  `releases`/`undecided`.
- Switch the kiosk CI step from `memcheck` + ratchet comparison to
  `brievc memcheck --deny-memory-ambiguity` **plus** the ratchet (belt and
  braces: the ratchet catches a silent freshness regression even if a future
  undecided reappears with the flag relaxed by accident).
- **Exit:** CI red on a hand-introduced ambiguity (prove it once, locally,
  then revert the probe).

### M5. Guard the other half of the story (new, cheap, high value)

The ratchet today tracks `undecided` and `releases`. `conservative drops`
(2200) is the number that would hide a regression in the opposite direction:
a "fix" that converts undecided into silent permanent leaks would look like
progress. Extend `harness/memcheck_ratchet.sh` to parse and ratchet the
drops count too, with the baseline's current value recorded and a comment
stating the rule: **drops may not grow**.

- **Exit:** the ratchet fails when the drops count grows (prove with a
  temporary probe).

---

## 3. Track X — XPU lane ceilings and promotion

All X items are in-band: each names its kiosk stage or its lane gate. None
starts before M3 lands, except X5 (a cheap record pulled into wave 0).

### X1. f16 coopmat tier: fix or gate it (highest measured leverage)

**2026-11-17 — the denominator is settled (device-free).** Record:
`benchmarks/results/2026-11-17-x1-denominator-audit.md`; run
`bash benchmarks/gpu_workcount_audit.sh`. The dispatch geometry now lives in one
place (`GemmPlan::dispatch_geometry`, consumed by the tier gate too) and shows
`workgroups · tile_m · tile_n == m·n` at both shapes — 64 workgroups at 512³,
4096 at 4096³. The grid is full: **no folded or short dispatch**, so the
"suspect denominator" branch below is refuted.

The *baseline* was the thing that was wrong. Implied rates from the recorded
timings: 512³ coopmat 0.256 / tiled 2.237 TFLOP/s; 4096³ coopmat 0.370 / tiled
3.462 TFLOP/s. Both arms beat both "ceilings" (1.46 f32-SIMD, 1.63 f16-storage)
at 512³, in a 0.12 ms window where a launch artefact cannot manufacture a 9×
gap — so those two figures are REFERENCE-KERNEL achievements, not hardware
bounds, and comparing against them is what made a correct denominator look
impossible. Both arms also scale the same way (1.45×/1.55× from 512³→4096³); a
folded dispatch would collapse at the larger shape instead.

Consequence: coopmat's target is the **tiled arm's own rate** (2.24 TFLOP/s at
512³, 3.46 at 4096³), not 1.63. Whether the recorded wall clocks measure
completion is still the device's question (X4 / `clFinish`) — but it is now the
only open one in that comparison.

The original text follows.

**A/B recipe (needs only the device, no code change).** The tier is already
policy-gated by a config fact consumed at BOTH decision points
(`src/backend/spirv/kernel.rs:309` pair-view fields and `:450` tensor kernel):

```toml
# config/ir-lowering.dbvl
spirv_coopmat: 1;   # coopmat/tensor tier    ← current
spirv_coopmat: 0;   # exact tiled kernel      ← the other arm
```

So the measurement is: build both arms, run the same `ceil_gemm_*_f16.abv` at
512³ and 4096³, and confirm the **work count** of each dispatch matches before
comparing times. (An earlier note in this session claimed that knob was parsed
but never consumed; that was a grep error — `grep "spirv_coopmat\b"` in basic
grep searches for a BACKSPACE, not a word boundary, so it appeared unused. Both
consumers are real.)

Recorded device A/B (Vulkan, `2026-11-09-intel-lane-ceilings`) — f16 GEMM
takes the coopmat tier since the declared-kind + cast-tolerant detection
landed:

| 512³ | 4096³ |
|---|---|
| coopmat 1.05 ms vs f32-tiled 0.12 ms | coopmat 371 ms vs f32-tiled 39.7 ms |

Coopmat is **8.75×** slower at 512³ and **9.3×** at 4096³. Selection is
currently a regression *shipped as a feature*. Two acceptable outcomes, one
of which must be proven with the variant-diff A/B gate
(`benchmarks/deferred_ab_gate.sh`), not a single-arm timing:

- (a) **Fix the microkernel** until coopmat beats f32-tiled at ≥512³ on both
  vendors' lanes (XMX exists to be used; the CL-C ceiling is 1154/1628
  GFLOP/s and coopmat should exceed it, not sit 9× under it).
- (b) **Gate the selection** on measured facts: coopmat only fires where a
  recorded config row says it wins for that shape/arch/dtype. Add the row to
  `config/targets.toml` and to the `gpu_strategy.rs` inputs.

(a) is preferred; (b) is the honest stopgap if (a) needs vendor work. Doing
nothing is not an option — a keyword-beaten default is a compiler bug
(Rule 2).

**Sanity caution on the baseline arm.** 4096³ is 137 GFLOP; 39.7 ms implies
~3.45 TFLOP/s, which is **above** the measured SIMD f32 ceiling (1.46) and the
f16-storage ceiling (1.63) from the OpenCL C references. Either the tiled arm
is genuinely using XMX through a path the CL-C reference did not exercise, or
it is under-counting work (a folded/short dispatch — the exact failure mode the
"no folded/degenerate runs" rule exists for). X1's first action is therefore
to **verify the work count of both arms** before treating 39.7 ms as the
target; a wrong denominator would make the whole comparison meaningless.

- **Exit:** either coopmat wins at the declared shapes with device evidence,
  or the selection is fact-gated and the regression is gone; both recorded in
  a results file with the variant-diff gate.

### X2. NEO/OpenCL residency

L0 residency landed (wall ≈ device). NEO still pays a full projection copy
per dispatch, which is why NEO reads 2.5–5× faster than the naive reference
but not at its own ceiling. Implement `launch_dev`/`launch_dev2d`/batched
dispatch for the NEO driver behind `BrievDeviceDriver`, mirroring the L0
shape.

- **Exit:** end-to-end app numbers stop paying the copy (report wall vs
  device delta before/after on the five non-GEMM gates).

### X3. W2c — GLSL / XMX ceiling harness

The four-number matrix needs the Vulkan ceiling (hand GLSL) to pair with the
existing OpenCL C ceiling, and the XMX ceiling needs a cooperative-matrix
reference (hand GLSL with `coopmat`, or via `spirv-as`). Without these, "the
emitter is N× off the ceiling" is unattributable: we would not know whether
the gap is the emitter or the kernel form.

- **Exit:** `benchmarks/intel_ptl/ceiling/` gains a GLSL reference for
  copy/elem/reduce/GEMM at the fixed shapes plus one coopmat GEMM; results
  row with the same correctness discipline (double-precision CPU reference,
  sampled rows for GEMM).

### X4. The `clFinish` hang (`BUGS.md` 2026-11-09, OPEN)

The exact-fold NEO kernel intermittently hangs inside `clFinish` (~1/3 under
load). This blocks NEO promotion (W4) and makes the NEO lane untrustworthy
for any long run. Harness-first isolation: reproduce under the ceiling
harness (no compiler in the loop) to prove it is the lane, not the emitter;
then bisect the OpenCL C reference vs the Briev-generated image.

- **Exit:** root cause recorded in `BUGS.md`; either fixed or converted into
  a documented lane limitation with a hard timeout that fails loudly instead
  of hanging.

### X5. Stage F promotion gate (cheap — pull into wave 0)

Stage F (OpenCL-flavored SPIR-V) is functionally verified: both Level Zero
and NEO/OpenCL compile and launch the projection with correct results
(`g1_lane_smoke.abv`). Promotion was blocked **only** by the const-N gate
emitting 641 MB / >45 min of IR. W1 fixed exactly that (204 KB / 78 s), so
the promotion gate is now unblocked and unverified. Re-run it and record.

- **Exit:** per-lane promotion evidence in
  `benchmarks/results/2026-11-XX-intel-stage-f-promotion.md`: five non-GEMM
  gates + GEMM lane status, repeated runs, and the resulting lane REQUIRED/
  reported state.

### X6. Workstream D — the Intel dispatch waves (design already exists)

`docs/architecture/intel-dispatch.md` §9 sequences them; they are now
unblocked enough to start:

- **Wave 0:** G0 numbers on PTL with the existing Vulkan/OpenCL drivers, in
  the same harness as the OpenVINO lane, so the first Briev-on-Intel
  comparison is a real row rather than an assertion.
- **Wave 1:** Level Zero behind `BrievDeviceDriver` (G1/G2), reusing the GPU
  projection contract. No emitter change.
- **Wave 2:** coopmat image selection + dual-image fallback (D3/G3), fed by
  the probed capability record (D1) — this is where X1's facts become config.
- **Wave 3:** cost-model row + config schema (D9), then the portfolio rerun.

Rule from that doc, restated: no wave starts before the previous gate passes;
the verification record commits with the step.

### X7. OpenVINO FFI gate on the node

`lib/std/ai/openvino.bv` (372 lines) and `tests/openvino_gate.rs` exist but
the gate skips when the library is absent — so it has almost certainly never
run on PTL. Run it there: GPU compile + infer, shapes and values asserted,
timing via the Briev collector (`benchmarks/intel_ptl/collector.bv`).

- **Exit:** a results row with the runtime version, device, and the
  correctness evidence; state clearly it is a **capability escape hatch**, not
  a codegen path (plan §1 guardrail 1).

---

## 4. Track K — kiosk (the external meter)

### K1. Pin bump + full re-gate (do first)

`briev-kiosk/compiler.lock` pins `ceee6ebcf`, which predates the frame-leak
fix (`00fcd5e90`, `d4cbc9c49`). The fix is proven only against a
hand-built daemon. Bump and re-run every gate:

- `harness/native_smoke.sh`, `harness/ml_smoke.sh`,
  `harness/vad_rpc_smoke.sh`, `harness/vad_sessions_smoke.sh`,
  the dashboard rbv IR verify, and `harness/memcheck_ratchet.sh`.
- Then `harness/soak.py` 500 calls; record in kiosk `docs/progress.md`.
- **Exit:** pin advanced, all gates green, soak flat, progress entry written.

### K2. Capability ledger is stale (rows claim `absent` for work that is native)

Verified against `docs/progress.md`: ResNet-18 executes natively, YuNet
f32+int8 runs in the daemon, Silero VAD runs natively with streaming state
parity, JPEG/PNG decode via stb. The ledger still says `absent` for rows 6,
12, 13, 14, 19, 20, 21.

- Update rows to `native`/`partial` with the gate that proves each, and
  update the retirement ledger's OpenVINO-for-face-detection row.
- **Exit:** every `native`/`partial` row names a runnable gate; no row claims
  `absent` for something `progress.md` records as working.

### K3. Edge SLO budgets (blocking the word "edge-ready")

Unified §7 requires peak RSS, cold start, model load, and binary size inside
locked budgets, measured **under a constrained runner** (the 62 GB box is not
an edge proxy; the PTL OOM at ~10 GB was the warning). Decision 2 in the
unified plan is still open.

- Lock the numbers, add the cgroup runner to CI, and record the measurement.
- **Exit:** `docs/slo.md` gains an edge-budget table with measured values and
  the "edge-ready" claim becomes permissible for the covered stages only.

### K4. PTL runner network drops (owner decision)

Tailscale/disco churn kills the runner for ~15 min tails. Mitigation needs
either sudo on the host (a selfdiff timer) or a runner-side reconnect policy.
Record it as an environment limitation with a named owner; do not let it
masquerade as a test failure.

---

## 5. The grind sequence

Waves are ordered by dependency. Each wave's exit gate must be green before
the next starts; each commits its own results record.

**Wave 0 — unblock and record (no new code).**
M1 (soak truth record) → K1 (pin bump + re-gate) → X5 (Stage F promotion
gate). All three are cheap and unblock everything else.

**Wave 1 — census to zero.**
M3a → M3b → M3c, re-censusing after each batch. This is the critical path.

**Wave 2 — deny at zero + guard the leaks.**
M4 (flip the flag, update the baseline) → M5 (drops ratchet). Proves the
gate, then proves it cannot be gamed.

**Wave 3 — tail-loop correctness.**
M2 (carrier fact → lowerer release → memory-checked gate). The last known
ownership *bug* (as opposed to debt).

**Wave 4 — XPU performance.**
X1 (f16 coopmat A/B) → X2 (NEO residency) → X3 (GLSL/XMX ceiling) →
X4 (`clFinish`).

**Wave 5 — XPU integration.**
X6 waves 0–3 (with X1's facts landing in config at wave 2) → X7 (OpenVINO
gate on node).

**Wave 6 — honest ledger.**
K2 (capability/retirement refresh) → K3 (edge budgets) → K4 (record the
runner limitation).

**Wave 7 — reassess.**
Return to the unified program's critical path: kiosk S2/S3 breadth (STT,
TTS, visemes — kiosk plan §C/D "deferred, need owner decisions"), or the H5
completion leg, whichever the fresh census and SLO table name as the next
blocker.

---

## 6. Gates and measurement discipline (every commit)

- `cargo test --lib` green; `bash scripts/gate.sh quick` green; `full` before
  any push touching shared analysis.
- Conformance sweep green (the slice-view incident is the precedent: a
  freshness arm that looked right corrupted `conv_ext` weights).
- Praetor on changed directories (`--target <dir>`, never a file).
- Kiosk: native/ML/VAD RPC/VAD sessions/dashboard IR verify + the census
  ratchet, every commit that touches the pin or the daemon.
- Any timing or memory number enters a results file **only** with a
  same-commit, same-geometry correctness gate; provisional numbers are
  labelled provisional in the file itself.
- Kernel-selecting knobs (X1's coopmat selection in particular) are A/B'd
  with `benchmarks/deferred_ab_gate.sh`, never a single-arm reference check.
- Device-side addressing/mask/decode changes run the on-device correctness
  gate at a real shape before push (the 2026-09-08 fill-corruption lesson).

---

## 7. Risks and stop conditions

1. **The AST escape map is the only soundness backstop.** Every M3 fix must
   be conservative-or-better: converting an undecided into a conservative
   drop is a *regression* unless the drop count ratchet (M5) says otherwise.
   That is why M5 precedes wave 3.
2. **Non-monotone analyses must not be reintroduced.** Any new interprocedural
   summary pass needs a worklist (or a strictly monotone lattice with
   `summaries` read-only during a pass and swapped atomically). The retired
   walker is the cautionary example, with numbers.
3. **Coopmat selection is user-visible performance.** Shipping a 9× regression
   because the tier is "modern" violates Rule 2. X1 is not optional.
4. **The PTL box is shared.** Benchmark rows taken while avatar/other
   workloads run are invalid; the acceptance concurrency queue exists for
   this. Record the box state in every results header.
5. **Scope creep via K.** The kiosk plan's §C/D (STT/TTS/visemes, NPU
   priority) needs owner decisions; do not start them inside this grind.
6. **Stop condition:** if M2's carrier route exceeds a day, take the
   documented fallback (§M2) and keep the fact as recorded debt rather than
   leaving the exclusion undocumented.
7. **IR reproducibility is not yet achieved (found 2026-11-16).** Two builds of
   the kiosk daemon still differ in `%State` field indices (8 vs 9) and in
   interned-string assignment (`%ip_8` vs `%ip_9`). The release-order half is
   fixed (sorted), the other two are recorded in `BUGS.md` 2026-11-16. Until
   they are fixed, any IR-level A/B must diff the same *process output*, not two
   builds, and the determinism gate's corpus needs a state-heavy fixture with
   many interned strings.

## 7.1 Blocked on the PTL node (needs credentials, not decisions)

X2 (NEO residency), X3 (GLSL/XMX ceiling), X4 (`clFinish`), X5 (Stage F
promotion), X6 (dispatch waves), X7 (OpenVINO gate) all need the Intel device.
The node answers on `100.92.76.67` but only with interactive password auth —
no key, so a session cannot drive it unattended. Every one of those items has
its recipe written down here and in the harvest doc, so they are mechanical
once access exists. Local box is NVIDIA-only (2× RTX 3060), so no Intel
substitute is available.

---

## 8. Documentation updated in the same commit

| Change | Doc |
|---|---|
| Walker retirement + non-monotonicity reason | `docs/architecture/agent-reference.md` (interprocedural summary rule) and this plan §0.1 |
| Census numbers per batch | kiosk `docs/progress.md` |
| Lane state per X item | `docs/plans/INDEX.md` Intel line + `benchmarks/results/2026-11-XX-intel-*.md` |
| Promotion state | `docs/plans/2026-11-08-intel-g1.5-opencl-flavored-spirv.md` |
| Capability/retirement truth | kiosk `docs/capability-ledger.md`, `docs/retirement-ledger.md` |
| Edge budgets | kiosk `docs/slo.md` |

---

## 9. Open decisions (owner)

1. **X1 route:** fix the coopmat microkernel (a) or fact-gate the selection
   (b) first? Recommendation: measure (b) immediately (one config row, kills
   a 9× regression) while (a) proceeds as a real microkernel task.
2. **M2 route:** carrier fact (durable) or source discipline (fast)?
   Recommendation: carrier fact, with the fallback documented.
3. **K3 budgets:** the RSS/startup/size targets and the constrained runner
   size (e.g. 4 GB cgroup). Needed before wave 6 can close.
4. **Wave 7 choice:** kiosk S3 breadth (audio/vision pre-post) or H5
   completion. Recommendation: S3 — it is the external meter and it keeps
   finding compiler bugs.
