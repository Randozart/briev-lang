# Family K — accel host consolidation (C orchestration → Rust, same ABI)

**2026-09-21.** Follows the 2× root-cause fix (`02418f49`) and the
native-runtime families A–C (already in main — `feat/briev-native-runtime`
is fully contained, stale label deleted). Part of the C-elimination arc
(`2026-09-09-briev-native-runtime-and-family-realignment.md`), extending
its ledger host-side.

## Scope finding

`lib/runtime/briev_accel_rt.c` (1075 lines) is ~93% ORCHESTRATION (~710
lines: driver select, seed gating, dirty-range construction, the lazy
prime + snapshot/restore state machine, batch amortization,
`download_written`, `push_ranges`/`push_strided`, `probe` auto-tune) and
~3% pass-through. The real driver code is the 2616 lines in
`briev_dev_{cuda,vulkan,opencl}.c` — which STAY C (libcuda/libvulkan/
libOpenCL are C APIs; GLUE-FFI discipline: bindings only, no logic).

A second implementation of the same declared-proj-offset ABI already
exists in Rust (`src/gpu_rt.rs`: consumer-side FfiField/FfiKernelDesc
tables + a phase machine) — a Rule-17 duplication whose resolution this
family IS.

Consumers bind by C SYMBOL, not by source: LLVM IR declares
`@briev_accel_*` (`backend/llvm/kernel.rs:424-444`, `emit_toplevel.rs`),
`brievc run` links `libbriev_gpu_rt.a` (`build.rs:107-133`), harness
scripts link by name. Only the single-TU `#include "briev_accel_rt.c"`
idiom is structural (runner template + ~20 hand-written benchmark .c).

## The honest boundary

The loader/launcher is toolchain host software, not language runtime —
unlike families A–J (services to compiled programs), it does not become
Briev stdlib. It becomes ONE typed Rust implementation instead of two
stacks (Rust wrapper + C state machine). The class of bug that
`02418f49` fixed (silent host-state corruption in untyped C globals)
becomes a Rust module with owned state.

## Phases

### Phase 1 — ABI extraction
- `lib/runtime/briev_accel_rt.h`: `BrievField`, `BrievImageDesc`,
  `BrievKernelDesc`, `BrievDeviceDriver`, `BrievStridedCopy`,
  `BrievPushDesc`, capability macros, driver extern symbols.
- `briev_accel_rt.c` includes the header (no duplicate definitions).
- **Delete the stale fork `examples/gpu/briev_accel_rt.c`** (831 vs 1075
  lines, silently diverged — wrong-code hazard) and fix its consumer
  (`examples/gpu/attn_decode_matrix_runner.c`) to reach the canonical
  header.

### Phase 2 — the Rust orchestrator (atomic port)
- `src/accel_rt/` module: `#[repr(C)]` mirror of the ops table + entry
  types; drivers referenced as `extern "C" static` data symbols
  (`briev_dev_cuda` etc.) — no registration protocol, preserves the
  dlopen `available()` model.
- Port the ~710 orchestration lines VERBATIM in behavior: state globals
  become owned statics (`AtomicBool`/`Mutex`-free single-threaded
  assumption documented), the prime snapshot/restore (`02418f49`)
  migrates intact, `probe`'s clocked loops keep their structure.
- Export the IDENTICAL `extern "C"` signatures
  (`briev_accel_init/available/device_name/download/download_written/
  invalidate_resident/launch/launch_resident/launch_resident_2d/
  launch_resident_batch/push_ranges/push_strided/probe/shutdown`) →
  LLVM IR and symbol-bound consumers unchanged. Zero backend work.
- `build.rs`: compile the drivers as today, link the Rust staticlib
  (crate-type staticlib) — replaces `libbriev_gpu_rt.a`.
- `gpu_rt.rs`: `run_program_impl` calls the orchestrator directly (no
  FFI for Rust-side callers); fixes the `download(0)`-only quirk
  structurally.

### Phase 3 — consumer switch (mechanical)
- Runner template (`spirv/runner.rs:548`): include the header; `compile.rs`
  copies header + drivers (not the orchestration .c).
- Harness scripts gain `-lbriev_accel` (softmax_gate.sh, keyword_ab_gate.sh,
  m3_attention_harness.sh, m4_decode_microbench.sh).
- Hand-written benchmark .c files (sweep by grep, not memory): include
  → header + link.

### Phase 4 — deletion + ledger
- `briev_accel_rt.c` orchestration body deleted; the file dies into the
  header. The 2× fix retires with it (no TEMP marker survives).
- Ledger: native-runtime plan gains Family K (host-side, Rust, loader —
  not language runtime); AGENTS.md reference index updated; BUGS.md
  2026-09-20 entry notes the migration.

## Parity gate

The existing corpus IS the parity harness, run unmodified:
- `softmax_gate.sh` both fixtures, both lanes (deferred 2.06e-05-class
  on Vulkan is the regression signal)
- M3 harness both templates, both lanes
- `cargo test --lib` (includes the accel self-test wiring)
- One benchmark smoke (`gemm_bench` CUDA) to prove the probe path.

## Risks

- `repr(C)` function-pointer layout must match the C ops table
  exactly (field order = declaration order; verified by compiling a
  static assert table in the C self-test).
- `probe` is perf-sensitive: port the clocked-loop structure verbatim,
  no "improvements".
- Single-threaded assumption: the C RT relies on it implicitly; the Rust
  port documents it and uses `static mut` behind unsafe blocks with an
  audit comment (no runtime cost, same semantics).
- The ~20 benchmark includes: grep sweep, each switched mechanically;
  any missed file fails at cc time (missing symbols) — self-revealing.
