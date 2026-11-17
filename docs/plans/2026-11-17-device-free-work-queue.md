# Device-free work queue (2026-11-17)

## Status 2026-11-17 (end of first execution pass)

| # | item | state | evidence |
|---|---|---|---|
| 1 | corpus ERR triage + slice B | **done** | `benchmarks/results/2026-11-17-corpus-err-triage.md`; two runner defects fixed, floor re-measured at 243/59/207/0/**25** |
| 2 | `--config-dir` authority | **done** | `abi_dir()` consults the resolver chain, `run_build` publishes the flag; both directions verified; `deferred_ab_gate.sh` still green |
| 3 | X1 denominator audit | **done** | `benchmarks/results/2026-11-17-x1-denominator-audit.md` — geometry full, reference "ceilings" were not hardware bounds |
| 4 | K3 constrained runner | **done, blocked on environment** | kiosk `harness/edge_budget.sh` + `docs/results/2026-11-17-k3-edge-budget-harness.md`; the host cannot enforce `MemoryMax`, so the harness refuses to score |
| 5 | Data→Float invalid bitcast | **done** | `tests/tier1/float_bit_reinterpret.bv` + gate test |
| — | corpus slice C (ptr register through `inttoptr i64`) | **open, next** | documented with the exact IR in the triage results file |

Four workstreams that need no PTL access, ordered by value. Written after the
2026-11-16/17 memory lane closed (M2 tail-loop destruction, M6 determinism, M7
RSS truth) so the next session does not idle on the blocked XPU lanes.

**Blocked, not planned here:** X2 (NEO residency), X3 (GLSL/XMX ceiling), X4
(`clFinish`), X5 (Stage F), X6 (dispatch waves), X7 (OpenVINO). All need the
Intel device at `100.92.76.67`, which answers only with interactive password
auth. The real unblocker is an SSH key on that host — a one-line ask to its
owner, not engineering.

**Lane safety.** The FFI-contract session owns
`docs/plans/2026-11-16-ffi-followups.md` (F1–F11). F1 is done (M6). **F3 is
theirs in principle**, so item 1 below is scoped to *triage + the slices they
have not claimed*, and every slice names the files it touches so a collision is
visible before the first edit. Items 2–4 touch nothing on their path.

---

## Item 1 — Corpus ERR triage and the first slices (F3-lite)

### What is actually known

`scripts/verify_bodies_corpus.sh` (single reproducible runner, one compiler
process per file, sorted order) tallies six buckets. The recorded floor is
242 files / 210 PROVEN / 18 UNPROVEN / **34 ERR**. The "17 host IR regressions
in groups A–E" label exists only in session notes — **there is no A–E taxonomy
written down anywhere in the tree** (grepped `docs/` and `BUGS.md`). So F3 starts
as a classification task, which is exactly the followups plan's own warning:
"F3 is a fog, not a task."

### Step 0 — produce the ERR list (do this first, it is the whole gate)

The runner's `error:` bucket loses the file association: `awk '/error:/'` prints
bare lines, and the `FILE:` headers interleave only in the raw stream. Run the
full corpus once with the raw stream kept, then pair headers:

```bash
BRIEVC=./target/release/brievc bash scripts/verify_bodies_corpus.sh --full \
    > /tmp/corpus-full.txt 2>&1
# pair each `error:` line with the nearest preceding `FILE:` header
awk '/^FILE: /{f=$2} /error:/{print f"\t"$0}' /tmp/corpus-full.txt | sort -u
```

Deliverable: `benchmarks/results/2026-11-XX-corpus-err-triage.md` with

* the six counters at this commit (the new floor line),
* one row per ERR file: path, first error string, and a **class** chosen from
  the observed text, not from guesswork.

Expected classes (from the recorded BUGS entries, to be confirmed against the
run, not assumed): invalid IR (LLVM verification), missing callable /
undefined symbol, type-check failure on a stale surface, and
`--verify-bodies` internal errors. Anything that does not fit becomes its own
class rather than being forced into a letter.

### Then — one slice per class, smallest first

Each slice:

1. a minimal repro file under `tests/compile_fail/` (if the outcome is "must be
   rejected") or `tests/tier1/` (if it must build);
2. the fix in the module the repro names;
3. re-run the corpus and **quote the before/after counters in the commit
   message** — "the corpus is greener" is not a gate, the floor line is.

Slicing rule from the followups plan, kept: do not batch classes. A slice that
moves PROVEN up while leaving ERR flat is a failed slice, not neutral.

**Gates per slice:** `cargo nextest --lib`, the corpus run (counters diffed),
`bash scripts/gate.sh quick`.

**Risk.** Classes may share a root cause (e.g. one removed surface causing 20 of
the ERR lines). If step 0 shows that, the plan collapses to one fix — better
outcome, and the triage table will prove it rather than assert it.

---

## Item 2 — Make `--config-dir` authoritative for data lookups

### What is broken

Yesterday's fix made `glue::abi::abi_dir()` resolve `config/abi/` from
`BRIEV_HOME`, then the binary's own tree, then the caller's cwd. It still does
not honour `--config-dir`. Meanwhile the compiler already has a full resolution
chain in `src/config_resolver.rs`:

```
--config-dir  →  BRIEV_CONFIG_DIR  →  ./.briev/config  →  ~/.config/briev-compiler/active_profile  →  "__baked__"
```

and `run_build` (`src/main.rs:1332`) applies the flag to exactly two consumers:
`config_tuning::set_ir_lowering_from_dir` and `load_target_config`. The
glue/ABI data (`config/abi/*.dbvl`, `lib/glue/*`) is not on that list, so
`--config-dir` means two different things depending on which loader asks.

This is not theoretical: `benchmarks/deferred_ab_gate.sh` builds variants with
`--config-dir "$DIR"` where `$DIR` contains **only** `ir-lowering.dbvl`.

### Design

Keep one chain, do not add a second:

1. Make `ConfigResolver::resolve_config_dir` `pub(crate)`.
2. `abi_dir()` consults it **first** (so the flag and env both work), then
   falls through to the current order. A `--config-dir` that has no `abi/` must
   *fall through*, not fail — `deferred_ab_gate.sh` proves that case.
3. In `run_build`, after parsing, also export `BRIEV_CONFIG_DIR` for the
   process when the flag is present, so every future loader inherits the flag
   without new plumbing. One line, documented as such.

Rationale for (3) over threading a `&Path` through `load_core` /
`core_names` / `validate_abi_core`: those are called from config validation
itself, so threading would reach every descriptor entry point for one value the
process already resolves once. A process-scoped env var is the mechanism
`config_resolver` was built around.

### Tests

* `abi_dir` unit tests for each rung of the chain (already have one for the
  binary-tree rung).
* An integration test that builds a trivial fixture with `--config-dir <tmp>`
  where `<tmp>` holds only `ir-lowering.dbvl`, asserting the build still
  resolves the ABI cores (the `deferred_ab_gate.sh` shape).
* Run `bash benchmarks/deferred_ab_gate.sh` once by hand before and after.

**Gates:** library suite, the deferred gate, and one kiosk daemon build from the
kiosk repo with `--config-dir` added (it must be a no-op there).

---

## Item 3 — X1's first action, done without the device: the denominator audit

### Why this is device-free

X1's stated first action is *"verify the work count of both arms before treating
39.7 ms as the target; a wrong denominator would make the whole comparison
meaningless."* The work count is a property of the **emitted dispatch geometry**,
not of the device. `src/backend/spirv/gemm.rs:60` computes it in the open:

```rust
let workgroups = (self.m / (16 * r as i64)) * (self.n / 64);
```

At 4096³ with `r = 4` that is 64 × 64 = 4096 workgroups, each covering a 64×64
output tile — full coverage of the output, and a k-loop over 4096. That already
contradicts "folded run" **at the geometry level**, and it sharpens the
question rather than answering it:

> 137 GFLOP in 39.7 ms is 3.45 TFLOP/s, which is **above** both reference
> ceilings named in X1 (SIMD f32 1.46, f16-storage 1.63). Two explanations
> remain: the tiled arm really does reach XMX through a path the CL-C reference
> did not exercise, or the wall clock is not measuring compute (the open
> `clFinish` item, X4, is the obvious suspect for "measured launch, not
> completion").

### What to build

A small auditor, `benchmarks/gpu_workcount_audit.sh` + a Rust-side reporter:

1. For a given `.abv` fixture and shape, print: strategy chosen, workgroups,
   per-workgroup output tile, k-loop bound, total MACs, implied FLOP, and the
   TFLOP/s implied by a supplied time.
2. Flag any implied rate above the recorded reference ceiling for that dtype as
   **SUSPECT DENOMINATOR**, with both candidate causes named.
3. Make the reference ceilings data (`config/` row), so a new device updates a
   number, not a script.

This converts X1 from "run the device and squint" into "the device run either
agrees with the auditor or tells us which of the two explanations is true".

### Also device-free: the fact-gate row shape (outcome (b))

Outcome (b) of X1 is "gate the selection on measured facts". The *structure* can
land now: a `config/targets.toml` row keyed by (arch, dtype, shape-class) that
`gpu_strategy.rs` consumes, seeded with the one recorded fact we already have —
coopmat loses by ~9× at 512³ and 4096³ on the recorded Vulkan device. Landing
the row with the *recorded* numbers and a `provisional` marker keeps the honest
default (tiled) while making the eventual flip a data edit.

**Gate:** unit tests over `gpu_strategy::select` for the seeded rows; the audit
script run on the existing fixtures produces a table that matches the recorded
workgroups arithmetic. No timing claim enters a results file without the
variant-diff gate (Rule: kernel-selecting knobs are A/B'd, never single-arm).

---

## Item 4 — K3 constrained-runner harness, with the budgets as data

K3 needs owner numbers (peak RSS, cold start, model load, binary size, cgroup
size). It does **not** need them to build the runner.

### What to build

`briev-kiosk-avatar/harness/edge_budget.sh`:

* reads `harness/edge-budgets.toml` — a file with every key present and
  `value = null` until the owner fills them;
* runs the daemon under a cgroup v2 memory limit via
  `systemd-run --user --scope -p MemoryMax=…` (available here: `systemd-run`
  present, cgroup v2 with the `memory` controller mounted);
* reports peak RSS (`memory.peak`), cold start (bind to first successful RPC),
  model load (first `detect_faces` latency), binary size;
* **passes only when budgets are filled in** — an unfilled budget is
  `UNMEASURED`, never a pass. That is the K3 rule ("no stage may be called
  edge-ready") encoded in the tool instead of in a paragraph.

### Validation without owner numbers

Run it twice with a *synthetic* limit: once at a limit that must fail (e.g.
256 MB — the daemon should be OOM-killed or refuse) and once at a plausible one
(4 GB). That proves the harness bites, which is the only claim that can be made
before the budgets exist. Record both as `provisional`, never as edge-ready.

**Gate:** the negative control must fail; the positive control must produce the
four numbers with the same geometry as the 2026-11-17 soak (640×640 f32 frame,
yunet).

---

## Item 5 — Small, unblocked, and on the boundary path: the invalid-width bitcast (F2)

`BUGS.md` 2026-11-15: `(0x7F800000 as Data) as Float` emits
`bitcast i64 … to float`, which LLVM rejects. An unequal-width bitcast must be
diagnosed or resized per the declared cast contract — it may never emit invalid
IR.

Two-line repro, a cast-lane fix, and a `tests/compile_fail/` fixture. It belongs
in this queue because F2 is on the FFI path (a validator that emits invalid IR
poisons every descriptor-driven lane) and because it is the cheapest possible
demonstration that the casting graph is the only place a width decision lives.

**Gate:** the fixture fails to compile with a diagnostic naming the widths (not
an LLVM verifier dump), plus the library suite.

---

## Order, and why

| # | Item | Size | Why here |
|---|---|---|---|
| 1 | Corpus ERR triage (+ first slices) | L, but the triage itself is S | Largest live bug surface in the tree; the triage is a single run and decides everything after it |
| 2 | `--config-dir` authority | S | I introduced a partial fix yesterday; leaving two meanings of one flag is a trap for the next lane |
| 3 | X1 denominator audit | M | The only XPU progress that is real without the device, and X1's own first action |
| 4 | K3 constrained runner | M | Turns the owner's numbers into a data edit instead of a project |
| 5 | Data→Float bitcast | S | Cheapest boundary-path correctness win |

Item 2 first if the FFI session objects to item 1 on lane grounds — it is
independent and takes an afternoon.

## Documentation duties (same commit as the change)

* Item 1: results file with the counters; `docs/plans/INDEX.md` note.
* Item 2: `BUGS.md` (the flag's scope), `docs/architecture/glue-ffi.md`
  (where data is resolved from).
* Item 3: `benchmarks/results/` for the audit table; `config/targets.toml`
  comment naming the fact's provenance and that it is provisional.
* Item 4: the kiosk `docs/progress.md` entry, explicitly `UNMEASURED`.
* Item 5: `BUGS.md` closed with the diagnostic, not just "fixed".

## Stop conditions

* Item 1: if the triage shows a single root cause, stop slicing and ship the one
  fix — do not manufacture a taxonomy.
* Item 2: if making the flag authoritative breaks any consumer that relies on
  the partial-directory behaviour, revert to the fall-through-only design
  (chain consulted, never authoritative) and record why.
* Item 3: if the auditor cannot read the dispatch geometry without a device,
  stop — do not substitute an estimate for a measurement.
* Item 4: if `systemd-run --user` cannot enforce `MemoryMax` on this host, stop
  and record that K3 needs a runner host; a fake limit is worse than none.
