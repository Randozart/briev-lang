# pf_exp, the uninitialised float slot, and tonight's PTL queue (2026-11-17)

## Outcomes 2026-11-17

| item | state |
|---|---|
| **B** `store float 0` on an uninitialised float slot | **done** (`4e38abc6a`) — the `None` arm now delegates to `emit_float_literal_store(0.0)`. Every `let x: Float;` in any program was a hard compile failure before this. |
| **A** `pf_exp` | **blocked on phase 2 of an existing plan.** Not a contract bug — a tuple-summary gap already documented in `docs/plans/2026-11-15-value-summaries.md` §3.2 ("`Int, Int` tuples … Deferred — phase 2"). Measured evidence below. No stdlib contract was changed. |
| **C** PTL queue | prepared, unchanged; needs the node. |

### A — the measurements (why this is phase 2, not a contract edit)

`lib/std/cast_lanes.bv` alone: **24 proven / 1 unproven / 141 skipped**.

* the single unproven goal is `pf_exp`'s postcondition, and the corpus's 172 is
  that one goal re-emitted per native unit (the prelude re-contributes it);
* `pf_exp` and `pf_exp_digits` are both listed as *skipped* with "a call to
  `pf_exp_digits`, which has no value summary" — the callee's result is opaque, so
  no bound on `term.0`/`term.1` can reach the caller;
* adding `i >= 0` to `pf_exp`'s precondition changed **nothing** (still path 1 of
  6), so the missing fact I hypothesised is not the missing fact;
* rewriting `pf_exp_digits`' postcondition as the tuple form
  `term.0 >= -1000 && term.0 <= 1000 && term.1 >= 0` moved `pf_exp` from
  **unproven to skipped** and changed no proof at all: `0 unproven / 142 skipped`.
  That is a relabelling, not progress — exactly the failure mode the corpus
  discipline exists to prevent — and it was reverted in full
  (`git diff lib/std/cast_lanes.bv` is empty).

So the ladder the plan proposed (add facts → clamp → callers) cannot close this:
the blocker is the *representation* of a tuple result, which is phase 2 of the
value-summaries plan ("two obligations, one per field, or a record sort in SMT").
The 172-of-207 number is the argument for scheduling that phase, not for editing
the stdlib.

The clamp observation stands and is unchanged by the above: `acc < 1000` allows
`999 * 10 + 9 = 9999` against a commented ±1000 cap. It is unverifiable until a
tuple postcondition means something, so it is recorded, not changed.

Plan for the three items on the board after `docs/plans/2026-11-17-three-open-threads.md`
closed. Two are device-free and can run now; the third is the PTL queue for this
evening, prepared so that access is the only thing missing.

---

## A. `pf_exp` — one function is 172 of 207 unproven goals

### Finding 1: the multiplicity is the prelude, not 172 distinct problems

The corpus table reads `172 pf_exp | (((term.0 >= (-1000)) && (term.0 <= 1000)) && (term.1 >= 0))`.
`pf_exp` lives in `lib/std/cast_lanes.bv` (and its `cast_portable.bv` twin) and is
re-contributed to **every native compilation unit** by the prelude. The separate
`own surface` counter added in the last pass confirms the effect: 5348 obligation
entries collapse to 40 owned proofs. So this is **one contract**, measured 172
times. Discharging it removes ~83 % of the corpus's unproven goals in one change.

### Finding 2: `term.1 >= 0` is not provable from the stated precondition — and should not be

```briev
defn pf_exp(s: String, i: Int, len: Int, acc: Int)
    [acc >= 0 && acc <= 1000]
    [term.0 >= -1000 && term.0 <= 1000 && term.1 >= 0]
    -> (Int, Int)
```

The body returns `(acc, i)` and `i` is a parameter the precondition says nothing
about. The prover is right to refuse: `i` may be negative.

The prover already supports the shape — `src/analysis/verify_bodies.rs:1366` is a
passing test whose contract is
`[acc >= 1 && k >= 0][term.0 >= 1 && term.1 >= 0]`. It has the `k >= 0` that
`pf_exp` lacks. So this is a **missing contract fact**, not a prover gap, and the
fix is the contract-first one: state the fact.

### Finding 3: the clamp does not establish `term.0 <= 1000` — a real off-by-an-order

```briev
when acc < 1000 {
    term pf_exp_digits(s, i + 1, len, acc * 10 + (b - 48), sg);
};
```

With `acc = 999` and a digit of 9 the next accumulator is **9999**, against a
declared postcondition of `<= 1000` and a comment that claims "e is clamped at
+/-1000". To guarantee `<= 1000` after `acc * 10 + d` (d ≤ 9) the guard must be
`acc < 100`. Impact is bounded — f32 saturates long before exponent 1000, and the
comment itself says the cap only guards `i64 d` — but it is a contract that the
code contradicts, which is exactly what the prover is reporting.

### Plan

1. **Fixtures before edits.** A minimal pair under `tests/tier1/` — one provable
   (`pf_exp` with the added facts), one that must stay unproven — so the prover's
   behaviour is pinned independently of the stdlib.
2. **`pf_exp`**: add `i >= 0` to the precondition (and `i <= len` if the
   `term.1` argument needs it — decide from the prover's own counterexample
   message, do not guess). `term.1 >= 0` then follows for the `(acc, i)` arms; the
   `pf_exp_digits` arms need their own facts (next step).
3. **`pf_exp_digits`**: precondition `i >= 0 && acc >= 0 && acc <= 99`; the clamp
   becomes `acc < 100`. Its postcondition should be the tuple form
   (`term.0`, `term.1`) rather than a bare `term`, which on a tuple-returning defn
   reads ambiguously — check how the prover currently binds it before changing the
   text, because a bare `term` may be why its own goal does not appear in the
   unproven table.
4. **Callers**: `pf_num`'s `i2` feeds `pf_exp`, so `i2 >= 0` must become a fact
   there too, and so on up the chain to the entry that establishes it. This is the
   real work; treat it as a small ladder, one function at a time, re-running
   `bash scripts/verify_bodies_corpus.sh lib/std` after each rung.
5. **Do not weaken the postcondition.** If a rung turns out to be genuinely false
   of the code (the clamp is), fix the code. If a fact is unprovable because the
   prover cannot model something, record *that* as the finding and leave the
   contract alone.

**Gates:** `goals_unproven` for the corpus falls from 207 to roughly 35 (172 → 0
for this goal, others unchanged); `files_unproven` falls from 176 proportionally;
`goals_violated` stays 0; `cargo nextest --lib` and `daily_use_smoke` green (the
parser is on the hot path of every float parse — the golden outputs are the net).

**Risk.** `cast_portable.bv` is an excluded corpus file (browser prelude), so
changes there do not show in the counters; keep the two files textually in step or
the browser lane silently diverges. Also: adding preconditions to stdlib functions
is a *contract* change that callers outside the corpus might not satisfy — grep the
whole tree for `pf_exp`/`pf_num` call sites before landing.

---

## B. `defn-composite` — the second `store float 0` site (found)

`examples/syntax/meta/defn-composite.bv` declares `let r: Float;` — **no
initializer**. That takes `emit_field_init_value`'s `None` arm
(`src/backend/llvm/emit_toplevel.rs`), whose final `else` is:

```rust
writeln!(out, "{}store {} 0, ptr {}, align {}", indent, ty, gep, self.align_of(&ty));
```

For `ty == "float"` that is `store float 0` — invalid IR ("integer/byte constant
must have integer/byte type"), the same message the Decimal arm produced before
last pass's fix. So there are two emitters of one bug, and the zero-initialiser
path is the survivor.

**Fix:** in that `else`, dispatch by `ty`: `float`/`half`/`double` → delegate to
`emit_float_literal_store(out, indent, 0.0, (ty, gep))`, which already emits the
correct `bitcast (i32 <hex> to float)` form; everything else keeps the integer
zero. Delegating rather than hand-rolling a second hex path is the point — the
two must not be able to drift again.

**Gate:** `examples/syntax/meta/defn-composite.bv` links; corpus
`files_failing` 5 → 4; `cargo nextest --lib` + `daily_use_smoke`.

**Note.** This is worth doing even though only one example hits it: every
uninitialised `Float` state field in any program takes that path, so an
uninitialised float field anywhere is currently a compile failure.

---

## C. Tonight's PTL queue (prepared, not run)

Access shape: `BRIEV_PTL_SSH="ssh user@host"` (see
`benchmarks/intel_ptl/run.sh`; no credentials in the repo). Fixtures for the
ceiling work already exist: `benchmarks/intel_ptl/ceiling/ceil_gemm_{512,4096}_{f16,f32}.abv`.

### C1. X1 — f16 coopmat A/B (first; it is the measured 9× regression)

The denominator question is **already settled** by
`benchmarks/results/2026-11-17-x1-denominator-audit.md`: the grid is full
(4096 workgroups at 4096³, covering the output exactly once), so this run measures
kernel efficiency, not a folded dispatch. Two facts from that audit drive the run:

* the target rate is the **tiled arm's own** (2.24 TFLOP/s at 512³, 3.46 at
  4096³) — not the 1.46/1.63 "ceilings", which are reference-kernel figures;
* the open question is only whether the recorded wall clocks measure completion
  (`clFinish`, X4). So run each arm **twice**, once with an explicit finish and
  once without: if the numbers differ, that settles X4 and retroactively explains
  the 371 ms figure.

Recipe: build both arms via `--config-dir` with a one-line
`spirv_coopmat: 0|1` override (the config-dir authority fix landed this pass makes
that a pure data edit), verify the **work count** of each arm with
`bash benchmarks/gpu_workcount_audit.sh` before timing, then A/B with
`benchmarks/deferred_ab_gate.sh` so the verdict comes from a variant diff, never a
single arm.

### C2. X2 — NEO residency (needs `launch_dev`/`launch_dev2d`/batched dispatch)

Read `BrievDeviceDriver`'s L0 shape and mirror it for NEO. Evidence format: wall
time vs device time per dispatch (the L0 landed at wall ≈ device, which is the
target shape).

### C3. X3 — GLSL/XMX ceiling

Needs the CL-C reference numbers measured on the same node, so the comparison is
same-device. Only meaningful after C1, because C1 tells us whether the coopmat
tier is even selected correctly.

### C4. Record, do not conclude

Every number goes to a results file with the exact `BRIEV_PTL_SSH` form, the
device name, driver version, and the work-count table. Kernel-selecting knobs are
A/B'd, never single-arm. If C1/C2 produce nothing usable, that is itself a
recorded outcome — the lane must not end with an unrecorded evening.

### Not tonight

X4's fix (the `clFinish` hang), X5 (Stage F), X6 (dispatch waves) and X7
(OpenVINO) all depend on C1–C3 results or on more device time than one evening
gives. K3 still needs the owner's numbers *and* a runner where `MemoryMax`
actually bites — tonight's node may or may not be one; check with
`harness/edge_budget.sh`, which refuses to score an unenforced limit.

---

## Order

| # | item | size | why |
|---|---|---|---|
| 1 | B (`store float 0`, None arm) | S | 3 lines, one example goes clean, and any uninitialised Float field anywhere is currently a hard failure |
| 2 | A step 1–2 (`pf_exp` facts + fixtures) | M | 172 of 207 unproven goals; the largest single number on the corpus board |
| 3 | A step 3–4 (clamp + caller ladder) | M | the contract the code contradicts, then the chain that makes it provable |
| 4 | C (PTL, evening) | L | needs the device; everything above it is already prepared |

Start with B — it is small, and it closes `defn-composite` so the corpus's
`files_failing` reflects only the example/link lane afterwards.
