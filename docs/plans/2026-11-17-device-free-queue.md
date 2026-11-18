# Device-free queue (2026-11-17)

PTL is unavailable, so this plan covers only work that needs no GPU: one real
codegen bug, one misdiagnosis to correct, one lowering bug to localise statically,
and two follow-ups. Every item is verified on this machine; anything needing a
device is called out as "verify on next device access" and not claimed done.

Order matters: D1 is the highest-value (it blocks a corpus file and rides every
float benchmark), D2 corrects a wrong record before it misleads the next session,
D3 localises a known-wrong OpenCL kernel.

---

## D1 — the loop-carried `Float` backedge (invalid IR)

### Facts (measured today, this machine)

`examples/syntax/meta/defn-composite.bv` still emits, at line 817:

```llvm
%ppf18 = phi float [ %t11, %entry ], [ %pbf14, %.cm_latch ]   ; float phi
...
%t20 = alloca i64, align 8
store i64 %t19, ptr %t20
%t36 = load i64, ptr %t20, align 8
%pbf14 = fadd fast float 0.0, %t36                            ; ← i64 operand
```

The identity-copy idiom is `fadd fast {ty} 0.0, {val}` at **six** sites in
`src/backend/llvm/loop_engine/counter.rs` (811, 1533, 2066, 2126, +2), the float
siblings of the four integer `add {ty} 0, {val}` sites I replaced today with
`emit_carry_copy`. The bug is not the instruction — it is that `val` is an **i64
register for a field whose `field_types` entry is `float`**:

```rust
let val = self.fun.pending_phi_backedge.get(fname)...;
let field_ty = field_types[field_index_map[fname]];   // "float"
if field_ty == "float" || field_ty == "double" {
    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();   // val: i64
```

`%t20` is an **alloca**, not the `%State` slot, so the carried value was spilled
rather than read from the field. That points at the composite expansion producing
a boxed (i64) backedge value for a float local.

### The one question that decides the fix

Is `%t19` (what was stored into the i64 alloca) the **bit pattern of a float**
(then the correct repair is `bitcast i64 %t36 to float` — the value really is a
float in an i64 slot) or an **integer** (then the boxing is a representation bug and
the repair is upstream, in whatever recorded an i64 backedge for a float field)?

**Step 1: find `%t19`'s definition.** If it is `bitcast float %x to i64` or a
`store float` reinterpreted, it is a boxed float. Then:

* **Preferred fix** — record the *native* float in `pending_phi_backedge` when the
  field's LLVM type is a float, so all six sites stop needing a special case and
  `emit_carry_copy` handles them uniformly (it already types the zero by width).
* **Fallback** — unbox at the copy (`bitcast i64 → float`), which is correct only
  if step 1 proves a bit pattern.

If `%t19` is an integer, stop: the defect is in the composite local's storage
class, and the six copy sites are innocent (they would merely be the messenger).
Fixing the messenger would produce a plausible number for the wrong subject — the
defect class this campaign keeps finding.

### Verification

1. `defn-composite.bv` must build **and** produce clang-valid IR — building is not
   enough; `brievc build` never runs clang over the `.ll` (that is how today's f16
   bug hid for weeks). Add `clang -fsyntax-only` to the gate.
2. It must also **run correctly** (the file's prints), not merely link.
3. A tier1 fixture with a `foreach` float accumulator inside a composite, asserted
   by output value.
4. **A/B (§ Rule 20 / Rule 12):** before and after, run the float corpora —
   `bash benchmarks/build_and_bench.sh --runtime`, plus `bash benchmarks/ll_determinism_check.sh --quick`,
   plus the full `gate.sh quick`. Record both sides in the results file. If a
   benchmark regresses, the hypothesis is refuted and no fix lands.
5. Centralise the six float sites into `emit_carry_copy` (DRY, Rule 17) — the same
   instruction must not be authored in six places.

---

## D2 — correct the residency record, then finish the investigation

### The recorded hypothesis is REFUTED (measured today)

`benchmarks/results/2026-11-17-neo-residency-partial.md` says two
`briev_dev_opencl` driver tables exist and the link order decides. That is wrong:

```
target/compiler-in-briv/libbriev_gpu_rt.a : D briev_dev_opencl, D briev_dev_vulkan
target/compiler-in-briv/libbriev_accel_rt.a: U briev_dev_opencl      (undefined ref)
```

There is exactly one table and it is the one this session edited. **The results
file must be corrected** — a wrong diagnosis is worse than an open question, and it
would send the next session chasing a phantom.

### What to instrument instead

`briev_accel_launch_resident` → `_2d` has three ways to silently take the copy
path, and today's run cannot distinguish them:

1. `driver()` returned `None`;
2. `resident_ops_missing(d)` is true (any of `launch_dev2d` / `mapped` /
   `launch_dev` is `None` **as Rust sees the struct**);
3. `mapped` is null after `prime_lazy_buffer`.

Step 1: log which branch fires (one `eprintln!` under `BRIEV_ACCEL_VERBOSE`), and
print the three `Option::is_some()` flags. That single line converts the open
question into a fact.

Step 2, if (2) is the answer: check that the C designated initialiser and the Rust
`BrievDeviceDriver` declaration agree on **field order and count**. A mismatch
reads a valid function pointer as `None` — and would also explain why Vulkan works
while OpenCL does not (designated init sets named fields, but the *tail* fields'
offsets depend on the declaration both sides see).

Step 3 (device needed): only then probe SVM. Note the corrected premise: with
regular `cl_mem` the two projection copies dominate wall time, so residency buys an
allocation, not a copy. The metric is **copies per dispatch**.

---

## D3 — `g0_fsum_approx` is WRONG on the OpenCL lane (localise, don't guess)

Measured fact from the G0 gate run: `g0_fsum_approx on opencl: WRONG (bad=?)`,
while the same fixture is correct on levelzero and `g0_fsum_exact` is correct on
both. The two fixtures differ by **one keyword** (`approx` vs `exact` on the node),
and the data is integer-valued f32 so reassociation is bit-exact — therefore the
reassociation licence is not the problem: the **OpenCL lowering of the subgroup sum
is**.

Static investigation (no device):

1. Emit both flavours (`brievc build … --out`) and `spirv-dis` the
   `_ocl` images.
2. Compare: `OpExecutionMode LocalSize`, `SubgroupSize`/`SubgroupsPerWorkgroup`
   decorations, `OpGroupNonUniformFAdd` scope and operands, `ControlBarrier`
   placement, and whether the shared-memory partials are initialised.
3. The prime suspect is the subgroup width: the Vulkan/levelzero lane runs with
   `subgroupSize = 32` (measured preflight); if the OpenCL flavour emits a
   `SubgroupSize` capability requirement or a lane mask of 32 while NEO's device
   subgroup differs, the fold silently sums the wrong lanes.

Outcome for this plan: a **written diagnosis with the offending SPIR-V** and a
candidate patch. It cannot be *verified* without a device, so it lands as a
diagnosis plus a `g0.sh` note — not as a claimed fix.

---

## D4 — small, fully host-side follow-ups

* **Input-integrity canary** for the tiled-GEMM class: checksum `a`/`b` after the
  GEMM and print `CANARY=` — the only cheap way a value comparison catches the
  out-of-bounds writes the 16× over-dispatch produced. Written now, verified on the
  next device access.
* **`mmap-demo` / `networking` / `process-spawn`** corpus rewrites onto
  `std/os/mem.bv`, `std/net`, `std/process` — the first needs a real
  `std/os/mem.bv` (it is currently a planning stub), which is a stdlib task in its
  own right.
* **`pf_exp`** = phase 2 of `docs/plans/2026-11-15-value-summaries.md` §3.2 (tuple
  value summaries); 172 of 207 corpus goals ride it. Large; start only with a
  fresh plan.

---

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | D1 step 1: find `%t19`'s definition, decide unbox vs upstream | 0.5 h | written finding |
| 2 | D1 fix + centralise the six float copy sites | 2–4 h | clang-valid IR, runs correctly, tier1 fixture |
| 3 | D1 A/B on the float corpora | 1 h | no regression; results file both sides |
| 4 | D2 correction + the branch log | 1 h | results file corrected; branch named |
| 5 | D2 field-order check (if the log says so) | 1 h | flags agree |
| 6 | D3 static SPIR-V diff + diagnosis | 2 h | diagnosis with the offending SPIR-V |
| 7 | D4 canary + corpus rewrites | 2–4 h | canary prints; rewrites build+run |

## Doctrine notes

* **D1 must not "fix" the messenger.** Six well-typed `fadd`s would hide an
  i64-boxed float behind an arithmetic identity; the question of *which* is wrong
  decides which layer moves.
* **Correcting a recorded diagnosis is work, not bookkeeping.** D2 exists because
  today's note guessed at a duplicate table that `nm` disproves.
* **D3 is a diagnosis, not a fix,** until a device confirms it. Writing "fixed"
  from a disassembly would repeat the 16× mistake: a plausible number for the
  wrong subject.

---

## Outcome log (2026-11-18)

### D1 — DIAGNOSED, NOT FIXED. The bug is worse than invalid IR.

The deciding question (is the i64 slot a boxed float?) is answered: **no.**
`%t19 = add i64 0, 0` is the **integer literal** `0`, so the accumulator's slot is
typed from its *initialiser* while the loop phi is typed from its *declaration*.
And the consequence is not just invalid IR — the body round-trips the float through
the integer slot on every iteration:

```llvm
%t29 = load i64, ptr %t20
%t31 = sitofp i64 %t29 to float     ; widen
%t27 = fadd fast float %t31, %t30   ; add
%t32 = fptosi float %t27 to i64     ; TRUNCATE
store i64 %t32, ptr %t20
```

A `Float` accumulator inside a composite loop **loses its fraction every
iteration**. Today this is masked because the module does not link at all.

A fix was implemented and **rejected**: materialising `0` as `0.0` in
`src/plugin/composite.rs::fold_let_stmt` gives valid IR and the right value, but it
changes `lib/std/numeric.bv`'s `softmax_fused` enough that the deferred-normalize
plan stops being detected — five `backend::ptx::tests` fail, i.e. a real
optimization is lost. Causally verified (patch on: red; off: green). Next approach:
type the slot from the declaration at the alloca site, leaving the AST byte-identical.

Recorded in BUGS.md with the rejected fix and the reason.

### D2 — DONE. The residency record is corrected and the branches are named.

`nm` refutes the duplicate-table hypothesis (`D` in `libbriev_gpu_rt.a`, `U` in
`libbriev_accel_rt.a`), and the results file now says so. `briev_accel_launch_resident_2d`
logs which of its three fallbacks fires under `BRIEV_ACCEL_VERBOSE`.

Also found and fixed: **`build.rs` shipped stale runtimes.** On a C or staticlib
failure it printed a warning and returned, leaving whatever archive was on disk —
reproduced here as `undefined symbol: briev_dev_vulkan` at the bin link. Both
branches now `panic!` instead.

### D3 — DONE as a diagnosis (per plan). See
`benchmarks/results/2026-11-18-fsum-approx-opencl-wrong.md`.

`emit_subgroup_reduce` hard-codes `Scope::Subgroup` while the kernel launches a
32-lane workgroup, so the result is correct only when the device's subgroup size is
32 — true on levelzero, not on NEO's OpenCL. Candidate fix: workgroup scope (a
semantic no-op where the sizes match). Not landed: changing kernel lowering without
the on-device gate is how the 16× bug survived.

### D4 — carried forward

The GEMM canary, the three corpus rewrites, and `pf_exp` are untouched; nothing in
tonight's findings changes their priority.
