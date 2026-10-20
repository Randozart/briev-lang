# Open work backlog — post-provenance and fixture campaign

**Date:** 2026-10-20
**Status:** ACTIVE
**Scope:** `main` only. Foreign lanes listed in `docs/plans/INDEX.md` remain
untouched.
**Owner instruction:** PR #29 is explicitly deferred. Do not re-open the
volatile/default discussion until asked; the structural fix has landed.

This is the single list of everything known to be unfinished after:

- the contract gate reached `policy: deny` with a zero-announcement census,
- the structural raw-pointer provenance/effect-summary fix landed (`9f18bcb7`),
- Fixtures 1–4 of the stdlib campaign got their first passes (`622cd519` and
  earlier).

It is a backlog, not a historical record. Update statuses in place. Do not
change the historical result files or superseded plans.

## Current ground truth

- `HEAD:` `c4e29158` (docs); last code change `622cd519`.
- `cargo test --lib`: 3,261 green; conformance sweep green under `deny`;
  contract census 0/0.
- `tests/daily_use_smoke`: 13/13 green.
- Six parity corpora PASS; determinism quick gate PASS.
- `cargo test --bin brievc`: RED on a pre-existing test-only construct
  (`src/compile.rs:3541`, missing `variadic_param`). `cargo build` is green.
- No CI workflows exist. All gates are local scripts.
- Baseline and post-change benchmark tables:
  `docs/plans/2026-10-20-pointer-provenance-and-stdlib-fixtures.md`.
- Fixture results:
  - `benchmarks/results/2026-10-20-fixture4-fsm.md`
  - `benchmarks/results/2026-10-20-fixture3-arena.md`
  - `benchmarks/results/2026-10-20-fixture2-spsc.md`
  - `benchmarks/results/2026-10-18-fixture1-ws-frame.md`

## Priority definitions

- **P0** — must be done before calling the current campaign complete: default
  gates red, missing validation, or a misleading document/config.
- **P1** — known structural correctness/representation gaps. Each has a
  reproduction or a concrete failing shape.
- **P2** — competitive-performance workstreams. Each has a measured gap.
- **P3** — fixture depth and broader language surfaces.
- **P4** — release/tooling/platform work that does not block the compiler
  gates.

## P0 — close-out and validation

| ID | Work | Evidence | Acceptance |
|---|---|---|---|
| P0-1 | Rerun the **full runtime harness** including the new rows `fsm_dispatch`, `arena_bump`, `spsc_ring`. **DONE 2026-10-20**: 42 rows, all MATCH; table in the active plan. | `benchmarks/build_and_bench.sh --runtime`; `/tmp/opencode/p0_full_after.log` | Keep the table current on each performance slice. |
| P0-2 | Fix the default `cargo test` path. **PARTIAL 2026-10-20**: `cargo test --bin brievc` is green; obsolete `tests/test_c.rs`/`test_rust.rs` removed; `c_driver_needs_state` API fixed; stale untracked FFI tests archived. Full all-target `cargo test` still fails on `c_driver_node`, `c_driver_python`, and `cli_args_test` (P1-15/P1-16). | `/tmp/opencode/p0_cargo_test_all2.log` | CI runs the targeted gates; either fix the remaining integration failures or document a deliberate exclusion. |
| P0-3 | Update `docs/architecture/features/ptr.md` for the new model. **DONE 2026-10-20**: provenance section, canonical ptr, invalidation, no mutable `!invariant.load`, explicit `vol` surface. | Doc now matches the implementation | Keep in sync with future provenance slices. |
| P0-4 | Fix the stale comment in `config/contracts.dbvl`. **DONE 2026-10-20**: active `deny` row documented; override guidance for experiments. | `config/contracts.dbvl` | Keep the row; do not omit it. |
| P0-5 | Add CI (or an explicit equivalent). **DONE 2026-10-20**: `.github/workflows/ci.yml` runs release build, lib/bin tests, tier-1 E2E, contract census, determinism quick gate, parity. | First run still to be observed on GitHub | Keep the workflow green; add jobs only with a measured need. |
| P0-6 | Worktree triage. **DONE 2026-10-20**: new fixture binaries ignored; user-provided asset/fixtures/results tracked; obsolete `test_c`/`test_rust` removed; untracked FFI tests archived under ignored `tests/archive/`; stale generated benchmark binaries and `benchmarks/parity/_out/` untracked and ignored; retired `utf8_ops` binary removed. | `git status` | Keep generated artifacts out of git; do not reintroduce tracked ELF binaries. |
| P0-7 | Update the active plan/index links. **DONE 2026-10-20**: active plan and `INDEX.md` link this backlog. | docs | Keep the pointer current. |
| P0-8 | Competitive-performance acceptance line. **DONE 2026-10-20** (policy). **POLICY 2026-10-20**: target is C parity on every runtime benchmark. A slower row is a tracked compiler gap with a measured cause in P2; no silent margin, no keyword/doc workaround. | Fixture ratios 1.90x–4.64x | Revisit only if the owner declares a margin; exceptions stay explicit. |

## P1 — structural correctness and representation

| ID | Work | Evidence / shape | Acceptance |
|---|---|---|---|
| P1-1 | `obj` literal with a Ptr-typed field stores i64 into a ptr slot. **DONE 2026-10-20**: literal store inttoptrs the handle; field load remains ptr; `ptr_field_smoke` pin; Arena restored to `Ptr<Bit<8>>`. | `BUGS.md`; `tests/tier1/ptr_field_smoke.bv` | Keep the regression; do not reintroduce i64→ptr field stores. |
| P1-2 | `plain let` Ptr load emits `ptr` where consumers expect i64. **DONE 2026-10-20**: plain Ptr globals use the i64 handle representation consistently; `plain_ptr_smoke` pin. | `BUGS.md`; `tests/tier1/plain_ptr_smoke.bv` | Keep declaration/store/load in lockstep. |
| P1-3 | Named imports of enum types/variants are undefined. **DONE 2026-10-20**: import keep-set recognizes enums; pre-typecheck `rewrite_enum_variant_access` lowers importer-qualified constructors; `fsm_smoke` and `enum_import_smoke` pins. | `BUGS.md`; tests | Keep the desugar before typecheck in both build/check pipelines. |
| P1-4 | Canonical pointer coverage is partial: arbitrary local/block i64 values still get a fresh inttoptr. | `canonical_ptr` only seeds args and `%State` Ptr fields | A block-scoped or provenance-threaded cache covers locals and mixed access paths without non-dominating SSA. |
| P1-5 | Validate against the original external recommender repro. | `examples/recommender/reference.bv` is not in this tree; PR #29 used it | A checked-in behavioral repro (or a linked external harness) confirms the `picked[]` stale read is gone under `-O3 -flto`. |
| P1-6 | Mixed raw-memory access paths: `Load#`/`Store#`/`Copy#`/`Set#`, String/List carriers, `%State`, runtime helpers, and raw `Ptr<T>` index/deref. | D+C only made the raw index/deref pair canonical | Tests prove mixed paths cannot alias stale data, or a documented invariant states why they cannot. |
| P1-7 | `!invariant.load` audit beyond Ptr fields. | We removed it for Ptr state fields because they are mutable; other metadata may still lie | Every `!invariant.load`/`!range`/alignment metadata is justified by a proof or removed. |
| P1-8 | Pointer arithmetic / GEP slice is deferred. | Source `Ptr + Int` is rejected; `PtrAdd#` already GEP | Either keep the source restriction documented, or admit `Ptr + Int` through GEP with pinned byte-vs-element semantics. |
| P1-9 | Interpreter gaps that block full parity: callable Briev txns not executed; raw-address→String door; compound bitwise-or residual; enum-payload match hangs. | `BUGS.md` 2026-10-20 entries; Fixture 1 len64/encode deferred | Interpreter runs the parity corpora and the fixture round-trips; backend never “fixes” an interpreter divergence. |
| P1-10 | `dyn Trait` LLVM path and PHI-mismatch open defects. | `BUGS.md` ledger; SPSC benchmark hit a PHI-mismatch shape before refactor | `dyn` lowering has no panic; PHI verifier failures have a regression test. |
| P1-11 | `i64` boxing tax Phase 1 never executed. | `docs/plans/INDEX.md`; `BUGS.md` 2026-06-16 entry | Either execute the native-type refactor or record a measured reason it is retired. |
| P1-12 | Contract-fact constant folding shadowing tests. | Call-gate folds `const` names but skips names in `bindings`; the `total=500` shadow case was found by accident | Unit tests: global const folds, local shadow does not, nested const chains fold. |
| P1-13 | `sock_recv` length sync regression pin. | Fixed 2026-10-20 (`buf.len = n`); loopback golden covers it | Keep the socket golden as the pin; no separate work unless another sink has the same pattern. |
| P1-15 | C-driven Python/Node bridge round-trips fail. | `cargo test --all` log: `python_native_extension_roundtrip` (`greet("world") != "world"`), `python_node_cross_language_bridge` (empty node save). | Determine FFI/ABI regression vs host-toolchain drift; add a deterministic gate or explicitly retire the drivers. |
| P1-16 | CLI entry-command test hangs. | `cli_args_test::entry_cmd_greet_quiet` and `entry_cmd_greet_verbose_name` ran >60s in the all-target run. | Reproduce with timeout, find the wait, fix the driver/test; no infinite hang in the default gate. |

## P2 — competitive performance workstreams

These are ranked by repeated evidence across fixtures and the suite.

### P2-1 — Unbox small sum types (`Result` / enums)

**Evidence**
- Fixture 4 FSM: **2.86x C**; IR shows per-transition `tcp_step` returning a
  heap box, then tag/payload loads.
- Fixture 2 SPSC: **1.99x C**; each pop returns a heap-boxed `Result`.
- Fixture 3 Arena is partly call overhead, but the same boxed-small-value
  machinery affects every `Result`-returning API.

**Work**
- Define a general small-sum representation: tag + inline payload slots in
  registers, or scalar replacement of a non-escaping box.
- Return small sums in registers where the ABI allows; keep the boxed form for
  escaping/large payloads.
- Consume in `match`, `is_ok`/`is_err`, `unwrap`, tuple destructuring, and
  method chains.
- No source workaround, no per-type special case, no new keyword.

**Acceptance**
- FSM transition loop loses the per-step heap traffic in the `.ll`.
- FSM/SPSC ratios improve materially; target C parity or a declared margin.
- All fixture goldens and the full suite remain MATCH.

### P2-2 — Small-function inlining and field scalar replacement

**Evidence**
- Fixture 3 Arena: **4.45x C**; the hot path calls `arena_alloc`, reads/writes
  obj fields, then calls `Store#`/`Load#`; C is a direct bump.
- Fixture 1 ws_frame: **3.50x C**; per-byte codec calls.
- `recommender` is 0.93x (Briev wins) only because its hot kernel was already
  extracted; the remaining suite still has call/boxing losses.

**Work**
- Cost-model inlining for small defns at hot call sites.
- Scalar replacement of non-escaping obj/struct fields in a loop.
- Contract-fact specialization at call sites (ranges, alignment, immutability).
- Keep the default efficient; do not require `#inline`/`#unroll`.

**Acceptance**
- Arena and ws_frame IR show the hot loop collapsed as far as the language
  permits; benchmark deltas recorded.
- No regression on benchmarks that currently win.

### P2-3 — ws_frame `_idio` follow-up

**Evidence**
- `benchmarks/results/2026-10-18-fixture1-ws-frame.md` recorded the 1.58x gap;
  the current post-provenance run is 3.14x.
- The codec is correct; the shape is per-byte call/validation.

**Work**
- Word-at-a-time unmask and/or an inlined payload walk.
- Keep the bounds-proven codec as the reference; add `_idio` as a second shape,
  not a replacement.
- Re-run the full corpus and parity.

**Acceptance**
- `_idio` closes most of the gap; the original codec keeps its correctness
  guarantees.

### P2-4 — Loop/vectorization gaps

**Evidence (P0 full rerun)**
- `float_math` 2.74x C, `float_math_nonzero` 1.48x.
- `sweep_sparse` 1.49x, `sweep_mid` 1.31x, `sweep_dense` 1.05x.
- `telemetry_stream` 1.48x, `pid_control` 1.23x.
- Many “loop not vectorized” remarks remain in the result logs.

**Work**
- Diagnose each with `-Rpass`/`-Rpass-missed` on the actual `-O3 -flto` build.
- Feed alias/alignment/trip-count/reduction facts from contracts and analyses.
- Fix canonical loop shapes and phi/`%State` materialization that block SROA.
- Do not use a shape-specific matcher or a strategy keyword as the fix.

**Acceptance**
- Per-benchmark IR diagnosis and measured improvement; no vocabulary matching.

### P2-5 — `nbody_newton_accel` gap

**Evidence**
- 12.38x C in the P0 full rerun. This is the largest single loss.

**Work**
- Determine whether the gap is accel/offload plumbing or CPU codegen.
- If offload: fix the general offload path or document the unsupported shape.
- If CPU: fold into P2-4.

**Acceptance**
- A documented cause and a measured plan; no “GPU/noise” dismissal.

### P2-6 — `i64` boxing tax / handle representation

**Evidence**
- `BUGS.md` 2026-06-16; `docs/plans/INDEX.md` “Phase 1 never executed”.
- The Arena/FSM call paths still show box/unbox and handle conversions.

**Work**
- Execute the native-type/boxing refactor or retire it with a measured
  replacement.
- Align with P1-1/P1-2 so Ptr/obj/enum storage has one representation.

**Acceptance**
- No redundant `ptrtoint`/`inttoptr`/boxing in hot loops; suite unchanged or
  improved.

### P2-8 — Performance regression triage

**Evidence (fast `3c15268a` → current):**

| Benchmark | Fast | State/plain | Current | Verdict |
|---|---:|---:|---:|---|
| float_math | 0.90x | 2.01x | 2.74x | real regression |
| telemetry_stream | 0.86x | 1.03x | 1.48x | real regression |
| pid_control | 0.99x | 1.23x | 1.23x | real regression |
| sweep_sparse | 1.30x | 1.44x | 1.49x | real regression |
| nbody_newton | 0.97x | 1.13x | 1.08x | real/needs A/B |
| ws_frame | — | — | 3.50x → **1.48x** | fixed 2026-10-20 (P2-8 first slice) |
| queue_drain | 1.35x | 0.45x | 0.47x | improved |
| interval_step | 1.23x | 0.61x | 0.72x | improved |

**Status:** bisected + trigger confirmed. `float_math` fast `3c15268a` =
**0.94x**, first regression at `5aa90185` state/plain = **2.72x** (same
machine/harness); current ~2.74x. A controlled 5aa experiment makes
`stdout_buf`/`stdout_fill` stateful again: float_math **0.92x**, but
queue_drain **1.44x** and interval_step **1.46x** (the wins are lost). Root
cause: stateless print lets SROA promote 13 float fields; register pressure
spills the loop accumulator every iteration. The raw-pointer provenance patch
is not the cause (float_math has no Ptr). Fix must be a general
register-pressure-aware state/loop strategy, not a blanket stateful print.
Immediate `ws_frame` slice is DONE: 3.50x → **1.48x**, better than the old
1.58x, by removing per-byte guards and using a proven direct `Load#`. Full
record: `benchmarks/results/2026-10-20-performance-regression-bisect.md`.

Do not call the remaining rows noise and do not blame provenance without an
A/B. The open question is the state/plain stateless-print/`%State` interaction,
not the matrix arithmetic.

**Work**
1. Re-run `3c15268a`, `5aa90185`, and current interleaved on the same machine,
   7x, same BOUND/geometry; record whether the fast/current gap reproduces.
2. Bisect `3c15268a..fad30da8` for float_math/telemetry/pid/sweep via
   `git bisect run`, correctness-gated at a print boundary.
3. Inspect float_math IR/asm: `%State` promotion/SROA, loop-carried field
   loads/stores, FMA contraction under `-ffast-math`, vectorization, spills.
4. Fix the first reproduced cause in the general pass; no shape matcher, no
   strategy keyword.
5. Refresh the full P0 table after each fix.

**Acceptance**
- Regression rows either return to the fast-era ratio within noise, or have a
  named missing capability with a measured cause.
- `queue_drain`/`interval_step` improvements are not lost while fixing the
  float/stream lane.

### P2-7 — Full-suite re-baseline with new rows

**Evidence**
- Current baseline table predates `fsm_dispatch`, `arena_bump`, `spsc_ring`.
- The post-provenance table predates those three too.

**Work**
- Run the full harness after P2-1/P2-2 land, and at least once now for the
  current state.
- Record before/after tables in the active plan; never compare across shapes
  or machines.

**Acceptance**
- Every benchmark row has a current Briev/C/ratio/MATCH line.

## P3 — fixture depth and language surfaces

| ID | Work | Notes |
|---|---|---|
| P3-1 | Fixture 2 true concurrency | `os/thread.bv` is a stub; SPSC currently tests atomic lowering/ordering, not a race. Add a pthread lane and a race-detector gate before claiming concurrency. |
| P3-2 | Fixture 4 payload variants | v1 states/events are payload-free. Add payload-carrying states/events once enum imports and match lowering are solid; keep illegal edges as `Err`. |
| P3-3 | Fixture 3 general Ptr fields | **DONE 2026-10-20**: Arena uses `base: Ptr<Bit<8>>`; the golden still passes. |
| P3-4 | Fixture 1 `_idio` | See P2-3; also restore the deferred interpreter len64/encode round-trip when P1-9 lands. |
| P3-5 | Fixture campaign result ledger | Update the active plan's status table with each fixture's follow-up state; do not rewrite historical results files. |

## P4 — release, tooling, platform

| ID | Work | Notes |
|---|---|---|
| P4-1 | Release tags/version | `Cargo.toml` is `0.1.0`; no tags. Decide the first tagged release and platform matrix. |
| P4-2 | Default test green + CI | See P0-2/P0-5; this is the daily-driver blocker. |
| P4-3 | Briev-native runtime completion | `lib/runtime/briev_rt.c` still participates in links; the family migration is active. Document what remains or finish it. |
| P4-4 | OS/thread/process surface | `os/thread.bv`, `os/process.bv`, and 16 `os/*` stubs; decide which are supported vs explicitly out of scope. |
| P4-5 | Self-hosting | 10/12 `lib/compiler/*.bv` fail `brievc check`; keep the green-tested remainder truthful and track the path. |
| P4-6 | Platform support | Primary x86_64 Linux; wasm functional; embedded/GPU/silicon experimental. Add Windows/macOS only with a measured support decision. |
| P4-7 | Spec sync | `spec/SPEC.md` is Draft; keep syntax/import/ABI changes (canonical ptr, enum imports, Ptr fields) synchronized. |
| P4-8 | Formatting/lint gate | Decide whether `cargo fmt --check` / Clippy joins the per-commit gate; currently not documented as required. |
| P4-9 | Toolchain matrix | Run `benchmarks/toolchain_matrix.sh` before release/backend-touching commits. |
| P4-10 | Driver decomposition / `CompilationContext` | Extract the 1,171-line `compile_source` and 864-line `codegen` into staged drivers; replace the 15-parameter bundle with a structured compilation context. Preserve behavior and pass the full gates. |
| P4-11 | Typed linker/backend metadata | Replace `ll_text.contains(...)` checks for freestanding `_start`, `; linker:`, and runtime symbols with structured backend-emitted hints (a `LinkHints`/artifact descriptor). |
| P4-12 | Toolchain abstraction | Centralize `clang`/`opt`/`llc`/`wasm-ld`/`ar`/`cc` discovery, version probing, flags, and error mapping; support explicit overrides and fail with actionable diagnostics. |
| P4-13 | Driver diagnostics | Keep structured diagnostics in the frontend; return typed driver errors and format them once at the CLI boundary instead of `String` at every helper. |
| P4-14 | Architecture review verification | Keep the table above current when the driver is refactored; do not repeat the inaccurate “tarball”/size claims. |
| P4-15 | IR emission ergonomics | Evaluate a small IRBuilder/instruction-helper layer instead of a blanket multi-line `write!` refactor. Fixed boilerplate may use `write!` with escaped braces; dynamic/conditional instruction streams stay line-per-instruction; measure code size/readability, not runtime. |

### External `compile.rs` review — verification

An external review (2026-10-20) graded the language concepts A+ and the driver
architecture C/C-. Checked against the tree:

| Claim | Verdict | Evidence |
|---|---|---|
| `compile_source` is a massive god-orchestrator (~500 lines) | **Accurate, understated** | `src/compile.rs:283-1454` ≈ 1,171 lines, not ~500. |
| `codegen` is another ~400-line function | **Accurate, understated** | `src/compile.rs:1648-2511` ≈ 864 lines. |
| `codegen` takes 15 parameters, including nested aggregates | **Accurate** | 15 inputs (`items` … `init_roots`), including `&mut Option<HashMap<String, Result<BindRoute, String>>>`. “Nested tuples” is imprecise; these are nested generics. |
| Driver shells out to `clang`/`opt`/`llc`/`wasm-ld`/`ar`/`cc` | **Accurate** | `src/compile.rs:2767, 2811, 3008, 3017, 3033, 3051, 3066, 3087, 3100`. |
| Toolchain version is unpinned / unchecked | **Accurate** | direct `Command::new`, no `--version` probe or compatibility gate for clang/opt/llc; error text only says “is clang installed?”. |
| Driver does string inspection of emitted LLVM IR | **Accurate** | `ll_text.contains("define void @_start() naked")`, `; linker: `, `@briev_bits_to_str(` etc. (`compile.rs:2850-2947`). |
| Driver manages macro capability lockfiles | **Accurate** | `compile.rs:284-305` (macro-lock), plus cache/object handling. |
| Driver extracts tarballs | **Inaccurate for `compile.rs`** | no tar/gz/zip/unpack path in this file; `extract_inline_stage_blocks` extracts source stage blocks, not archives. |
| Errors are stringly typed everywhere | **Partly accurate** | the driver is largely `Result<_, String>` (18 sites); the compiler already has structured `errors::Diagnostic`/typed error enums (`src/errors.rs`) and `diagnostics::Block`. The critique overgeneralizes if applied to the whole compiler. |
| SMT-backed contracts, tautology rejection, `b-bind` sole-writer, causality/liveness | **Accurate** | `compile.rs:675-740`, `1262-1283`; analyses in `src/analysis/`. |
| Unified backend model (LLVM/PTX/SPIR-V/CIRCT/Wasm/KiCad) | **Accurate at the architecture level** | backend/config surfaces; individual targets remain at different maturity. |
| BEAST snapshots are first-class | **Accurate** | `compile.rs:2511-2550` and stage snapshots throughout `compile_source`. |

**Judgment:** the conceptual/architecture split is fair: the language and proof
model are unusually strong; the driver is carrying real orchestration debt.
The specific sizes and a few details were wrong, but the direction is right.
The maintainability grade is a reasonable warning, not a verdict that the
compiler is unsuitable for its current research/prototype stage.

## Dependency graph and recommended sequence

```
P0-1 full harness ─┬─ P0-2 default test ─┬─ P0-5 CI
                   ├─ P0-3 ptr docs      │
                   ├─ P0-4 config comment│
                   └─ P0-6 hygiene      ─┘
P1-1 obj Ptr field ─┬─ P3-3 Arena field
P1-2 plain Ptr ─────┘
P1-3 enum imports ── P3-2 payload variants
P1-4 canonical coverage ─ P1-5 external repro / P1-6 mixed paths
P2-1 sum unboxing ── P2-2 inlining ── P2-3 ws_frame ── P2-4 vectorization
P3-1 threading ── true SPSC
```

Recommended order:

1. **P0-1/P0-2/P0-3/P0-4/P0-6** — make the current state honest and green.
2. **P1-1/P1-2/P1-3** — close the three small representation/import bugs; they
   are prerequisites for clean fixture depth.
3. **P2-1** — small-sum unboxing. It is the single highest-leverage performance
   change and affects three fixtures.
4. **P2-2** — small-function inlining/scalar replacement; then P2-3.
5. **P1-4/P1-5/P1-6** — provenance coverage and mixed-path validation; do this
   before trusting any raw-buffer performance result.
6. **P2-4/P2-5/P2-6** — loop/vectorization, accel gap, boxing tax.
7. **P3/P4** — fixture depth, threading, release/CI/platform.

## Gates (unchanged house standard)

- `cargo test --lib`
- `cargo test --lib conformance_sweep`
- contract census = 0 under `deny`
- `tests/daily_use_smoke` (all tier-1 fixtures)
- `bash benchmarks/ll_determinism_check.sh --quick`
- parity corpora + full harness when codegen changes
- Praetor on changed directories
- BUGS.md entry for every discovered compiler gap
- plan/results update with measured numbers
- commit per logical step

## Decisions still needed

1. **Competitive line:** resolved 2026-10-20 — C parity target; slower rows are
   explicit P2 gaps, no silent margin (P0-8).
2. **Enum/Result ABI:** register-returned small sums vs scalar replacement vs
   both; scope of the first slice.
3. **Ptr representation:** is the canonical-cache + i64-storage model the
   permanent ABI, or is H/A (native ptr) still on the table?
4. **Threading:** is a real pthread lane required for the language, or is SPSC
   explicitly a single-threaded lowering test?
5. **CI provider:** GitHub Actions vs an external runner; what gets gated on
   every PR vs release-only.
6. **Platform scope:** Windows/macOS, or Linux-first with wasm as the only
   second target?

## Explicit non-goals / rejected

- Volatile-by-default or any optimizer fence for ordinary memory (PR #29
  approach deferred by owner).
- Shape/vocabulary-specific performance match arms.
- Source workarounds in fixtures that hide a compiler gap.
- Strategy keywords as the way to reach competitive performance.
- Retroactively editing historical results/plans.
- Touching live foreign lanes.

## Appendix — current dirty worktree (not from this backlog)

Generated/modified benchmark binaries, deleted `benchmarks/utf8_ops`,
untracked `tests/ffi_*`, `tests/fixtures/*`, `benchmarks/results/2026-10-16-post-stability-runtime.md`,
and `assets/billd-icon.svg` predate or sit outside this work. Triage under
P0-6; never commit them accidentally.
