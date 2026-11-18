# The two remainders: residency's branch, and the GEMM canary (2026-11-18)

B2 answered the *behaviour* (levelzero resident, OpenCL copy path) but not the branch, and
B3 was never started. Both are bounded, and both hinge on the same lesson: **verify the
instrument before trusting the answer.**

---

## R1. Name the OpenCL residency fallback (1 h)

### What is already known

From `benchmarks/results/2026-11-18-residency-b2-answer.md`:

* `briev_accel_launch_resident_2d` **is** entered on both lanes (an unconditional probe
  printed);
* levelzero proceeds to `launch_dev2d` (0.19–0.21 ms, 12.6 MB staging);
* OpenCL runs the copy path (`enqueue … local=256`, 0.026 ms);
* **none** of the three gated branch logs fires, even after replacing the cached
  `g_verbose` with a direct `BRIEV_ACCEL_VERBOSE` read.

The Rust-side verbosity is therefore not trustworthy in a generated runner. The C drivers
are: their `[briev_accel/opencl] enqueue …` reaches the terminal in the same process.

### The experiment must not depend on the broken gate

Six **unconditional** prints, no `if verbose` anywhere, then removed in the same commit:

| where | question it answers |
|---|---|
| `_2d` entry (already proven to print) | control |
| after `driver()` | is a driver resolved? |
| after `kernel_ready` | is the kernel ready? |
| after `resident_ops_missing` | which of the three ops reads as `None` |
| inside `prime_lazy_buffer` before/after `briev_accel_launch` | did the prime run, and what did it return |
| C: `briev_dev_opencl_mapped` entry + `briev_dev_opencl_launch_dev2d` entry | does the driver see `mapped`, and does the resident dispatch arrive |

The last row is the decisive one because **the C side is demonstrably verbose**: if
`launch_dev2d` is never entered while `_2d` has passed the prime, the refusal is in
`seed_or_sync_scalars`; if `mapped` is never called, `_2d` returns before that block, which
the source reads as impossible without a logged branch — and *that* would be the finding.

### The likely answer, and why it matters either way

`briev_dev_opencl` advertises `capabilities = 0` ("no zero-copy (regular buffers)"), and the
residency machinery distinguishes shared-state devices (`BRIEV_DEV_CAP_SHARED_STATE`) at
`accel_rt.rs:675`, `:708`, `:1021`, `:1122`. If residency is gated on that capability
somewhere the three branches do not cover, the honest conclusion is that **OpenCL cannot be
resident by design** — no copies are removable without SVM — and the code comment should say
so instead of implying the driver work is merely unwired.

### Deliverable

One of: (a) a named branch and a fix; (b) "OpenCL residency is capability-gated by design" as
a documented fact plus the SVM follow-up (probe `CL_DEVICE_SVM_FINE_GRAIN_BUFFER`, and make
"copies per dispatch" the metric); (c) the surprising case — a branch that fires but does not
log — which becomes a bug in the instrumentation, and the fix there.

**No code lands** beyond the comment/documentation unless (a) names a real bug.

---

## R2. The GEMM input-integrity canary (2 h, plus 1 h to prove it bites)

### Why it exists

`G0_GEMM_BAD=0` was green **before** the `LocalSize` over-dispatch fix. The surplus
workgroups wrote *identical* values into covered tiles and sprayed out of bounds elsewhere,
so sampling outputs cannot see it. An out-of-bounds write corrupts neighbouring state, and
neighbouring state is what has to be checked.

### Design

`benchmarks/intel_ptl/g0_gemm.bv` fills its inputs from a closed form on the CPU lane:

```briev
a[idx] = ((idx * 7) % 13) as Float * 0.125 - 0.5;
b[idx] = ((idx * 3) % 11) as Float * 0.25 - 1.0;
```

Add a `canary` node (phase 3, before `report`) that:

1. samples `NS` spread indices in **`a`** and **`b`**, recomputes the closed form, and counts
   mismatches into `canary_bad` — the arrays precede `y` in `%State`, so a tile overflow past
   `y` can land in them after wrapping the projection;
2. checks the two state scalars the program already depends on (`phase`, `bad` bounds) are
   still sane — cheap, and they sit in the projection too;
3. reports `G0_GEMM_CANARY=ok|BAD` from the existing `report` node.

Note the honest limitation, to be written into the fixture's header: a write past the *end*
of the projection may land outside any checked field (or unmapped → SIGSEGV). The canary
covers the corruption case, the crash covers the other; together they see what `BAD` cannot.

### Proving it bites — the part that makes it a gate

`G0_GEMM_BAD=0` under the old code is the whole argument, so the canary must be shown red
against it:

1. copy `lib/runtime/briev_dev_vulkan.c` to `/tmp/old_vulkan.c`;
2. restore the pre-fix parse there (both dispatch sites using `local_x` instead of
   `X*Y*Z` — the diff is in `2026-11-17-ptl-tiled-gemm-dispatch.md`);
3. build one `_runner` against the vendored translation unit;
4. expect **`G0_GEMM_CANARY=BAD` while `G0_GEMM_BAD=0`** — that contrast is the proof.

A canary never seen failing is decoration; this session already shipped one such test and had
to replace it (a bespoke fixture that passed both with and without the D1 fix).

### Gates

`ok` on levelzero **and** vulkan (the tiled tier is Vulkan-only); `BAD` under the vendored
parse; and the existing checks unchanged (`G0_GEMM_BAD=0`, `GPU_SUM == CPU_SUM`).

---

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | R1: six unconditional prints, one run, read the table | 45 min | the firing point named |
| 2 | R1: fix or document, per the three outcomes | 30 min | code comment matches behaviour |
| 3 | R2: canary node + `ok` on both lanes | 1 h | `CANARY=ok`, existing checks unchanged |
| 4 | R2: vendored old parse | 1 h | `CANARY=BAD` **while** `BAD=0` |

## Doctrine notes

* **R1 is an instrument repair first.** Three sessions lost time to a log that could not
  print; the plan's first step is to prove the instrument works, not to chase the answer.
* **R2's value is the contrast**, not the `ok`. A green canary with no demonstrated red is
  the pattern that let the 16× bug live for weeks.
* **Both are device-gated**: PTL must be up, and the stale-runtime trap applies — after
  editing `lib/runtime/**` or `src/accel_rt*.rs`, force the build (`touch build.rs`) and
  confirm the archive contains the new string before drawing a conclusion from silence.
