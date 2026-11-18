# Next session: D1, then the PTL batch (2026-11-18)

PTL is free again. This plan finishes the last invalid-IR file (device-free) and then
clears three device-gated items whose instruments already exist.

## D1 — the loop-carried `Float`: the countable path never pre-declares

### What is now known (three measurements, all recorded in `BUGS.md`)

1. The accumulator's slot is `alloca i64` while its loop phi is `float`, so the body
   round-trips it: `sitofp` → `fadd` → `fptosi` → store. A `Float` accumulator loses
   its fraction every iteration; the invalid backedge (`fadd fast float 0.0, %t36`)
   is what stops the program from linking, and it is the smaller half of the bug.
2. Two fixes were tried and reverted: materialising `0.0` in the compositor (fixes it,
   but loses the deferred-softmax plan — five `backend::ptx::tests`, causally
   verified), and pre-declaring the countable loop's reassigned lets (no effect).
3. `BRIEV_DBG_REASSIGNED=1` shows `collect_reassigned_lets` returns **empty** for
   `defn-composite.bv`.

### The structural finding that ties (3) together

`collect_reassigned_lets` has exactly **two** call sites — `ssa.rs:452` (the SSA main
path) and `emit_toplevel.rs:385` (`emit_definition`). **`loop_engine/counter.rs` calls
it zero times.** `defn-composite`'s node has a countable contract (`[i < 1][i == 1]`),
so it goes through `emit_countable_main`, which therefore pre-declares *nothing* — the
empty map was never about the walk missing a `Block`; that path simply does not run.

So the demotion happens in the plain `let`/`assign` emission inside the countable body,
and the composite's expansion (which produces the `let`/`foreach` pair) is materialised
during statement emission. The `alloca i64` comes from whichever branch allocates there.

### Step 1 — one measurement, and it names the function

Add two env-gated prints (same style as `BRIEV_DBG_REASSIGNED`):

* in `emit_countable_main`, the body's statement kinds (so we see whether the composite
  is already expanded at that point, or arrives as one invocation-shaped statement);
* in each candidate slot-allocating branch of `emit_let_statement` /
  `emit_assign_statement`, the name, the declared `ty`, and the `llvm_ty` chosen —
  printed only when the two disagree.

Run on `defn-composite.bv`. Expected outcome: either (a) the let is expanded and has a
declared `Float`, and some branch ignores it, or (b) the expansion happens *inside* the
emission of the invocation, so the slot is created from the initialiser expression's
type with no declaration in scope.

### Step 2 — the fix, by case

* **Case (a):** the branch must consult the declared type (`let_original_types` is
  already the map for this in `ssa.rs`); slot type wins over initialiser type, and the
  `fptosi`/`sitofp` pair disappears.
* **Case (b):** the composite expansion must carry the declared type into the generated
  `let` **at emission time** — not into the AST (that was attempt 2), but as the
  emitter's own typing for the expanded statements.

Either way the rule is the same and worth stating in the code: **a slot is typed by the
declaration; the initialiser is coerced to it.** The reverse is what produced an i64
slot holding a float.

### Verification (unchanged, and non-negotiable)

1. `defn-composite.bv` builds, emits clang-valid IR (the verifier now says so
   automatically), **and runs correctly** — print the accumulated value.
2. A tier1 fixture: a composite `Float` accumulator printing its value (the fixture
   written on 2026-11-18 is in the plan history; recreate it).
3. `bash benchmarks/build_and_bench.sh --runtime` A/B before/after, both sides recorded
   (Rule 20). A float-benchmark regression refutes the approach; do not land.
4. Remove the entry from `tests/ir_verify_allow.txt` **in the same commit** — the
   ratchet fails on a stale exemption, by design.

Cost: 2–4 h including the A/B.

---

## PTL batch — three items, in this order

### B1. `g0_fsum_approx` is wrong on OpenCL (correctness, one-line candidate)

Diagnosed statically 2026-11-18 (`benchmarks/results/2026-11-18-fsum-approx-opencl-wrong.md`):

| | `g0_fsum_approx` | `g0_fsum_exact` |
|---|---|---|
| `LocalSize` | 32 | 256 |
| reduce | `OpGroupNonUniformFAdd %float %uint_3 Reduce` | none |

`%uint_3` is **ScopeSubgroup** and `src/backend/spirv/lower.rs::emit_subgroup_reduce`
hard-codes it. Correct only when the device's subgroup width equals the 32-lane
workgroup — true on levelzero (preflight measured 32), not on NEO's OpenCL (16 on Xe),
where each subgroup reduces its own half and both write the row.

**Change:** emit the reduce at `Scope::Workgroup`. Where `LocalSize == subgroup size` it
is semantically identical, so levelzero cannot regress; where they differ it fixes the
lane. `GroupNonUniformArithmetic` is already declared.

**Gate:** `bash benchmarks/intel_ptl/g0.sh` — `g0_fsum_approx` on **opencl** must print
`bad=0 SUM=ok`, and levelzero's four fixtures must be unchanged (0.153 / 0.574 / 0.572 /
0.295 ms, the numbers recorded on 2026-11-17).

**Also verify the licence still means something:** `ceil_reduce_f16` (unlicensed) must be
unchanged, and `ceil_reduce_approx_f16` must stay at 0.106 ms ± noise. If workgroup scope
changes the licensed lane's performance materially, that is a finding, not a regression to
hide.

### B2. Which residency fallback fires? (one run, instrument already in)

`briev_accel_launch_resident_2d` now logs three distinct reasons under
`BRIEV_ACCEL_VERBOSE`:

```
resident: kernel N not ready -> copy path
resident: driver missing ops (launch_dev2d=… mapped=… launch_dev=…) -> copy path
resident: driver mapped() still null after prime -> copy path
```

Run `ceil_elem_f32.abv` on `BRIEV_ACCEL_DEVICE=opencl` and read which one prints. That
answers a question open for two sessions (`2026-11-17-neo-residency-partial.md` has a
corrected diagnosis and a refuted duplicate-table hypothesis).

**Then, by answer:**

* *missing ops:* the C designated initialiser and the Rust `BrievDeviceDriver` declaration
  disagree on field order/count — compare them; a mismatch reads a live pointer as `None`
  and would explain Vulkan working while OpenCL does not.
* *mapped still null:* OpenCL has no device-mapped host pointer; the resident path needs
  SVM (`clSVMAlloc` + `CL_MEM_SVM_FINE_GRAIN_BUFFER`, probed, never assumed). Record
  whether NEO reports fine-grained SVM, and treat "copies per dispatch" as the metric,
  not wall ≈ device.

No code lands unless the answer says the C entries are on the path; the driver work is
already written and marked inert.

### B3. GEMM input-integrity canary (makes the 16× class observable)

The tiled GEMM's value check passes with `G0_GEMM_BAD=0` **even under the old 16×
over-dispatch**, because the surplus workgroups wrote identical values into the covered
tiles and sprayed out of bounds elsewhere. A value comparison cannot see that; an
**input checksum can** — if a kernel writes past `y`, it corrupts its neighbours.

Add to `benchmarks/intel_ptl/g0_gemm.bv`, after the GEMM:

* a checksum of `a` and `b` computed on the CPU lane, compared against the values the
  init node wrote (recompute the same closed form, don't hash — a hash of a corrupted
  buffer would also need the expected value);
* print `G0_GEMM_CANARY=ok|BAD`.

**Gate:** `ok` on levelzero *and* vulkan; and prove it bites by temporarily building the
pre-fix `local_total` parse (vendor the old `.c` copy) — the canary must report `BAD`.
A canary that has never been seen to fail is decoration.

---

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | D1 step 1: the two env-gated prints | 30 min | case (a) or (b) named |
| 2 | D1 fix + tier1 fixture | 2 h | valid IR, right value, allowance entry removed |
| 3 | D1 A/B on the float corpora | 1 h | no regression; both sides recorded |
| 4 | B1 workgroup-scope reduce | 1 h | fsum_approx opencl bad=0; levelzero unchanged |
| 5 | B2 residency branch read (+ fix only if the answer is concrete) | 1 h | the firing branch named |
| 6 | B3 GEMM canary + prove it bites | 2 h | ok on both lanes; BAD under the old parse |

## Doctrine notes

* **D1's rule generalises**: a slot is typed by its declaration, the initialiser is
  coerced. Both earlier attempts inverted that or worked around it; writing the rule down
  is what stops a fourth.
* **B1 is the cheapest real bug in the queue** — one instruction's scope, a lane that
  currently returns a wrong sum, and a semantic no-op on the lane that works.
* **B3 exists because a green check was not evidence.** `BAD=0` was true before the 16×
  fix too; the canary is the check that would have been red.
