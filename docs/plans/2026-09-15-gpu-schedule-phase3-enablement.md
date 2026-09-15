# gpu_schedule Phase 3 enablement — per-kernel field packing

**2026-09-15.** Opens the `gpu_schedule_buffer_reuse` gate by fixing the
runner's global field-pack model. Phase 3 analysis + kernel aliasing
infrastructure already shipped (`5f2f9f26`, `ddb9c9f6`), gated OFF because
`briev_accel_launch_resident_2d` seeds ALL fields into the shared buffer at
first launch — an aliased slot gets overwritten by the dead partner before
its live consumer reads it.

## Root cause

`emit_runner` emits ONE global `BrievField fields[]`, and every
`BrievKernelDesc` references it with `fields.len()`. The runtime:
- seeds all of the first-launched kernel's fields once (program-level),
- uploads scalars per launch,
- sizes the shared device buffer from the first kernel's `proj_size`
  ("re-prime is impossible — the layout is fixed per program").

With `a`/`c` sharing a slot (reuse), the seed writes `c` over `a` before
the kernel that reads `a` runs. Foreign fields also get packed into every
launch (wasted bandwidth, and the clobber).

## Design decisions

- **Reuse restricted to WRITE-FIRST targets.** A pair `(A dead_slot, B
  new_array)` is valid only when B's first-use txn WRITES B (B is not read
  at first use). This is the load-bearing simplification: it makes the
  one-time seed safe without a deferred-upload primitive.
- **Seed = input arrays only.** A field is an *input* iff its first-use
  txn READS it. Inputs and reuse targets are disjoint (inputs are
  read-first, targets write-first), so the seed never packs a dead field
  over a live slot, and no two inputs share a slot. Kernel-written arrays
  are device-produced (never seeded from stale host). This replaces the
  old "seed all fields" — also less upload bandwidth.
- **Per-kernel field tables.** Each `BrievKernelDesc.fields` = the
  kernel's *touched* set (`read_buffers ∪ write_buffers ∪ scalar_ins ∪
  {index_var}`). No kernel packs a field it does not touch, so aliased
  partners are never both packed by one launch (shared per-launch scalar
  upload, and the `.bv` per-launch pack). Also cuts packing volume.
- **Buffer sized to the program union, not per-kernel.** `BrievKernelDesc`
  gains `program_bytes` = the global layout's max(proj_offset+size).
  All allocations (shared buffer, per-launch `.bv` `briev_accel_launch`)
  use it so the kernel's full SSBO struct always fits. The kernel SSBO
  member layout is UNCHANGED (all fields, global offsets) — no member
  renumbering, no `setup_state_buffer` risk.
- **Track A mirrors the C path.** `gpu_rt.rs` builds per-kernel `FfiField`
  arrays; the seed table is passed alongside.

## Changes

### Frontend — `src/analysis/gpu_schedule.rs`
- Promote `array_first_use` (local in `compute_array_first_use`) to a
  `pub` field on `GpuSchedule`. Extend the
  `array_last_use_tracks_dead_arrays` test to assert it.
- Restrict `compute_reuse_opportunities` to write-first targets: B is
  eligible only when B's first-use txn writes it (not read at first use).

### Runner — `src/backend/spirv/runner.rs`
- `RunnerKernel` gains `touched_fields: Vec<String>`; fill in
  `build_kernels` via `kernel_touched_fields(shape)`.
- `SsboLayout` gains `program_bytes` (max proj_offset+size over buffer
  fields).
- Compute the program **seed set** (input arrays: first-use txn reads
  them); emit `seed_fields[]` and pass it to every desc.
- `emit_runner`: emit per-kernel `k{i}_fields[]` tables; each
  `BrievKernelDesc` references its own table + `seed_fields[]` +
  `program_bytes`.
- `prepare_run`: `RunKernel` gains `touched_fields`; `RunProgram` carries
  `seed_fields` + `program_bytes`.

### Runtime C — `lib/runtime/briev_accel_rt.c`
- `BrievKernelDesc` gains `seed_fields`/`n_seed_fields` and
  `program_bytes` (tail; zero-fill contract).
- `briev_accel_launch_resident_2d`: the one-time seed iterates
  `k->seed_fields` (inputs) instead of `k->fields` (all). Per-launch
  scalar upload unchanged (iterates the kernel's touched table).
- `proj_size(k)` returns `k->program_bytes` when nonzero.

### Track A — `src/gpu_rt.rs`
- `FfiKernelDesc` gains `program_bytes` + seed table; per-kernel
  `FfiField` arrays from `RunKernel.touched_fields`.

### Config + docs
- `config/ir-lowering.dbvl`: `gpu_schedule_buffer_reuse: 1`.
- Plan `2026-09-14-gpu-schedule-pass.md` Phase 3 status → resolved.
- `BUGS.md` entry → FIXED with on-device verdict.

## Verification

1. `cargo test --lib` green; Praetor on changed Rust files.
2. Unit: `array_first_use`; `emit_runner` output has per-kernel tables +
   uploads in the right places; `prepare_run` mirrors; spirv-val clean
   with a reuse_map.
3. On-device gate (Phase 3 verdict): attention-decode parity with
   `gpu_schedule_buffer_reuse: 1` (maxrel vs composition baseline); a
   2-kernel array chain correct on-device.
4. Flip the config only after 1-3 pass.

## Undo

`gpu_schedule_buffer_reuse: 0` restores the global-table runner byte-for-
byte (the per-kernel emission and first-use uploads are additive; with the
flag off `build_kernels` receives an empty reuse map and the upload calls
are no-ops for non-aliased programs since every input is first-used by its
kernel anyway).