# Rust compiler-warning cleanup

**Date:** 2026-10-18
**Status:** ACTIVE — operator-approved 2026-10-18.
**Gate convention:** `cargo build` output must be warnings-clean (except
explicitly kept, listed) · `cargo test --lib` green · Praetor on changed
files · continuous commits · no `#[allow]` without house provenance
(when/why/undo).

## Scope decisions (locked with the operator, 2026-10-18)

1. **Scope (a):** fix the warnings `cargo build`/`cargo check` currently
   emit. The crate-wide mask in `src/lib.rs:1-2`
   (`#![allow(unused)]` + `#![allow(unused_variables)]`) stays for now —
   inventory will report how many warnings it hides; removal is a
   SEPARATE campaign if the operator later asks (could be dozens).
2. **`build.rs` briefc note:** keep the message (load-bearing for the
   dogfood build) but reword so it is clearly a build-config note, not a
   Rust warning — `cargo:warning=` prints with `warning:` styling.
3. **Stale claim:** `docs/plans/README.md:91`
   (`cargo build # 0 errors, 0 warnings`) corrected to the real gate
   (zero *new* warnings; current total listed once measured).

## Findings entering this plan

- Edition 2024 (`Cargo.toml:4`) → `unsafe_op_in_unsafe_fn` is
  warn-by-default: the observed
  `call to unsafe function … requires unsafe block` warnings come from
  unsafe fns calling unsafe ops without an explicit inner `unsafe {}`
  block (`src/ffi/metropipe.rs` ×8+, `src/accel_rt.rs` ×2, …).
- `src/main.rs` is a separate crate root — two known pre-existing
  unused-variable warnings there (stage5 plan; typed-variadic plan
  §gates), not covered by the lib allows.
- Baselines drifted across docs: "14 lib warnings untouched baseline"
  (`2026-09-30-stage5-re-rank-and-5c.md:122`), "19 pre-existing
  warnings" (`docs/plans/INDEX.md:9`, 2026-10-06). No live inventory —
  step 0 produces it.
- Only lint config: `[lints.rust] unexpected_cfgs`
  (`Cargo.toml:61-62`).

## Steps

- **0. Inventory** — fresh temp `CARGO_TARGET_DIR` (nothing shared
  touched): `cargo check --all-targets` + `cargo build`, capture every
  warning (file:line + lint name), group by lint, record counts here.
  Also probe the size hidden by the lib `allow(unused)` mask (a run
  without the mask in a throwaway copy — report only, do not fix).
- **1. Fix `unsafe_op_in_unsafe_fn`** — explicit `unsafe { }` block per
  call site with a one-line why (bounds/aliasing rationale at that
  site); never `#[allow]`. Behavior-identical; Rule 9 applies only if a
  fix touches logic (it should not).
- **2. Fix `main.rs` unused variables** — remove or `_`-bind per real
  use; the known site is `main.rs:747`.
- **3. Residual lints** — classify individually: correctness fix first;
  intentional pattern → `#[allow(lint)]` with provenance comment
  (when/why/what pattern/undo).
- **4. `build.rs:56` reword** — same information, phrased as an
  informational note so `cargo build` output is pure Rust diagnostics.
- **5. Docs** — `docs/plans/README.md:91` claim corrected; this plan's
  Status appended with results (timestamped, never retroactively
  edited); INDEX warning baselines superseded by the measured number.
- **6. Gates + ship** — `cargo build` clean (kept items listed
  explicitly), `cargo test --lib` green, Praetor on changed dirs,
  continuous commits, push.

## Results (2026-10-18, filled at steps 0/6)

### Step 0 inventory (measured — supersedes every prior baseline claim)

- `cargo build` at campaign start: **14 lib warnings** = 11 ×
  `unsafe_op_in_unsafe_fn` + 3 × `private_interfaces`; **5 bin
  warnings** (`main.rs` ×3, `test_runner.rs` ×2); **9 test-only** (4 ×
  duplicated `#[test]` attribute, 5 × non_snake_case); 1 unused import
  (`tests/c_driver_callback.rs`); 5 `cargo:warning` build-script notes
  (briefc ×5; gpu-rt ×5 more fire only on degraded environments).
- The `metropipe.rs` guess in Findings above did not materialize — the
  E0133 sites were `accel_rt.rs` (1 stmt → 3 warnings) and `gpu_rt.rs`
  `run_program_impl` (8 call sites).

### Step 0 hidden-mask probe (report only — mask kept per decision (a))

**593 warnings** hide behind `src/lib.rs:1-2`
(`#![allow(unused)]` + `#![allow(unused_variables)]`): 200 unused
variable, 137 unused import, 47 unused mut, 37 unused `Result`, 31
unreachable pattern, ~178 long tail (never-used items, fields, dead
assignments). Cataloged in `docs/reference/LEGACY-SURFACE-INDEX.md`
territory — cleanup is the dead-code campaign's job, not this one.

### Fixes applied (all behavior-preserving)

| Fix | Files | What |
|---|---|---|
| E0133 ×11 | `src/accel_rt.rs` (`seed_program_fields`), `src/gpu_rt.rs` (`run_program_impl`) | explicit `unsafe { }` per site + SAFETY one-liners (bounds/ownership rationale at each site) |
| private_interfaces ×3 | `src/analysis/accel.rs` (`prove_kernel_pub`, `analyze_resident_safety` → `pub(crate)`), `src/typechecker/mod.rs` (`elaborate_ops` → private; sole caller same file) | visibility narrowed to ≤ parameter type |
| duplicated attribute ×4 | `src/backend/ptx/tensor.rs`, `general.rs`, `mod.rs`, `src/parser/definitions.rs` | removed the stray leading `#[test]` that sat above an inserted doc comment; zero prose lost |
| non_snake_case ×5 | `src/encoding_registry.rs` ×4 (UTF8/ASCII → utf8/ascii), `src/backend/llvm/tests.rs` (`escape_non_ascii`) | test-fn renames; grep confirmed zero external references |
| unused ×6 | `src/main.rs` (duplicate bounty preflight read removed — the post-options read at the same error text drives compile; `merge_overrides` `mut` dropped; stray `nav_json_path` in `run_watch_loop` — `rebuild_and_write_nav` computes its own), `src/test_runner.rs` (`link_and_run` `exe` param dropped + caller; `_src` readability precheck with why), `tests/c_driver_callback.rs` (unused `Path` import) | |
| build.rs reword | `build.rs` | the 3 expected/info briefc paths (no-prebuilt, 2 × ready) demoted to plain `[build]` stdout (visible with `cargo build -v`); genuine failure paths (pass build failed, gpu-rt degradation) stay `cargo:warning` — healthy builds emit pure Rust diagnostics |

### Gates

- `cargo build`: **0 warnings, 0 errors**
- `cargo test --lib`: **3164 passed, 0 failed** (count is 4 below the
  pre-campaign 3168 because each of the 4 duplicated-`#[test]` fixes
  un-registers one duplicate test entry — the deduplication itself)
- `tests/daily_use_smoke.rs`: **5 passed, 0 failed**; conformance sweep
  green (inside `--lib`)
- Praetor: HEAD-vs-current kinds diff on all 13 changed files,
  number-normalized — **zero new diagnostic kinds** (6 = 6)

### Notes

- `docs/plans/README.md:91` (`cargo build # 0 errors, 0 warnings`) is
  **true again post-campaign** — left unchanged; the step-5 task
  resolved itself.
- `tests/ffi_comprehensive_tests.rs` / `tests/ffi_stdlib_tests.rs`:
  foreign untracked WIP (E0433 against nonexistent modules) — excluded
  from `--all-targets` runs, never touched here.
