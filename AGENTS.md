# Briev Compiler — Agent Guidelines

**2026-07-31:** This is the condensed operating manual (~300 lines). The full
pre-rewrite document is preserved in `AGENTS.md.archive`; reference material
(language syntax, contracts, coding standards, backend architecture) lives in
`docs/architecture/agent-reference.md`. Historical context: `AGENTS_HISTORY.md`,
`AGENTS_HISTORY_2.md`.

## Operating Contract

You are building a compiler that must be correct for **all programs** written
in Briev, not just the test case in front of you. Zero tolerance: "probably
fine" is a critical failure. Every edge case, undefined behavior, or bug in a
file you touch is solved completely NOW — never deferred, never "out of scope,"
never "pre-existing."

Every decision passes three questions:

1. **Does this make the compiler more general, or special-case one pattern?**
   A match arm for `"ring_push"` solves today's benchmark; tomorrow's
   `MyQueue<T>` with `InsertAt <~ my_push(#Lh, #Rh)` demands the same treatment.
2. **Does this add knowledge the compiler must carry forever, or push it into
   configuration/stdlib where it can evolve?** The dividing line is
   `--no-stdlib`: if it must work without stdlib, it's an intrinsic; everything
   else belongs in config or `.bv` files.
3. **If this were the only rule left, would the architecture still hold?**
   Removing any one rule must not break the others.

Patches are unacceptable. There is no "go fast and break things."

## Golden Rules

1. **CONTRACT-FIRST**: Contracts are the source of truth. Never weaken
   `[product > 0]` to `[true]` — fix the code, not the contract.
2. **MAXIMUM EFFICIENT DEFAULT**: The compiler MUST pick the most efficient
   codegen strategy for EVERY program automatically — every case, not just the
   benchmark at hand. This covers every codegen decision: tuple slot
   allocation, collection strategy, probe cost, materialization, loop shape.
   A user should never need a strategy keyword to reach competitive
   performance; a keyword-beaten default is a compiler bug (fix the default,
   never let the modifier carry the win). Strategy keywords (`seq`, `vol`,
   `pack`, `async`, `sync<g>`, `atomic`, `union`, `trap`) exist for a
   DIFFERENT purpose — to express intended behaviour that plain efficient
   codegen cannot: `seq` forces a precise declaration order or sequential
   execution that aggressive vectorization/parallelism would break, `vol`
   demands volatile memory the optimizer may not eliminate, others express
   required embedded/inter-language semantics. They are for correctness and
   intent, never for speed. Requiring a keyword to win is a failing default.
3. **NO OBFUSCATION OF SPECIAL TREATMENT**: The compiler has intrinsics,
   hashwords, reflection, and directives — they exist, and pretending they
   don't is a purist trap. What is forbidden is HIDING them behind
   ordinary-looking syntax. Two-part principle:
   - **Avoid accidental complexity.** Essential complexity (SMT, LLVM IR
     emission) is unavoidable and kept. Accidental complexity (heuristic
     trees, hand-rolled passes that fight the design) is stripped, never
     preserved.
   - **Disclose special treatment.** Every compiler-known behavior carries an
     explicit marker: `#` suffix (intrinsic `Sqrt#`), `#` prefix (hashword
     `#Int`), `!` suffix (compile-time expansion `my_macro!`), `.^`/`.^^`
     (reflection). User-facing directives (`seq`, `pack`, `vol`, `async`,
     `sync<g>`, `atomic`, `union`, `trap`) are ordinary keywords — no `#` —
     disclosed at use (their purpose is correctness/intent, never speed —
     see Rule 2).
   - **NEVER hardcode Rust string matches as built-in functions.**
     `is_digit` → `import char from "std/char.bv"`. Primitive types (Int,
     Float, Bool, Ptr, Void) are the sole bootstrap exceptions.
4. **INTRINSICS BEFORE FRGN**: Check `get_intrinsic_signature()` before writing
   `frgn`. All intrinsic names are PascalCase + `#` suffix (`Sqrt#`).
5. **INTERPRETER IS REFERENCE**: If the interpreter runs it correctly, the
   backend must compile it. Fix codegen, never the interpreter.
6. **ADDITIVE ONLY**: Never modify existing optimization paths — new match arms
   only. The `_ => return None;` fallthrough must remain unchanged.
7. **ALWAYS FINISH**: No `todo!()`, `unreachable!()`, `// TODO:`, or stubs in
   committed code. Every feature wired parser → AST → analysis → codegen → tests.
8. **NEVER DISCARD UNCOMMITTED WORK**: `git checkout --`, `git restore`, and
   `git checkout .` DESTROY work permanently — never use them. Commit your own
   changes with targeted `git add`; never stash others' work. `git reset HEAD`
   is safe (unstaging only).
9. **TESTS OR IT DOESN'T EXIST**: Every feature, code path, and match arm needs
   tests. `cargo test --lib` before every commit.
10. **NO PROTOTYPING**: Every optimization is a first-class pass in its proper
    module — never inline analysis into codegen as a shortcut.
11. **EXECUTIVE REQUESTS ARE NOT OPTIONAL**: Told to fix a pattern? Do all of
    it. If prereqs are missing, implement them first.
12. **PLAN WITH BENCHMARKS**: Every performance plan MUST include a baseline
    table of ALL benchmark results at the current commit BEFORE changes, and the
    new results AFTER. Baseline from a clean `cargo build --release` +
    `bash benchmarks/build_and_bench.sh --runtime`.
12b. **PERSISTENT BASELINE WORKTREE**: `../briv-compiler-baseline` holds the
    baseline commit for controlled A/B regression detection
    (`bash benchmarks/compare_baseline.sh <name>`). Never excuse a regression as
    "noise" without this experiment.
13. **DOCUMENTATION MAINTENANCE IN PLANS**: Every plan must specify which doc
    comments, rationale comments, and architecture docs need updating, and how
    to preserve existing commentary when refactoring.
14. **STDLIB IS THE EXTENSION MECHANISM**: New functionality goes in `.bv`
    files, not new Rust match arms. The compiler teaches; stdlib learns.
15. **NO KNOWLEDGE OF SPECIFIC TYPES**: The compiler must never check for
    `Type::string()` or match `"ring_push"`. Type-specific logic lives in config
    and stdlib. Sole exception: the bootstrap primitives.
16. **FULL PROVENANCE TRACKING**: Every rationale comment carries *when, why,
    what pattern it targets, and how to undo it*. `// TEMP: YYYY-MM-DD:` flags
    temporary solutions with a path to permanence.
17. **DRY**: A pattern appearing 3+ times becomes a centralized helper. Grep ALL
    call sites when changing a helper's behavior.
18. **MIGRATE WHEN TOUCHED**: When you modify a file, migrate its hand-rolled
    instances to the centralized helpers at the same time.
19. **NO TYPE NAME MATCHING**: Never match Briev type names (`t == "Int"`) in
    Rust. Derive LLVM type, protocol category, and ABI width from the
    `TypeUniverse` (via `universe_key()`/`Cast.#` properties) + `CastingGraph`.
    Exceptions: `Type::Ptr(_)`/`Type::Vector`/`Type::Bits(N)` (compiler
    constructs) and `tbaa_node` (operates on LLVM IR type strings). A `git
    grep` for `Type::Custom.*==` in `src/backend/llvm/` and `src/glue/` must
    return zero.
20. **MEASURE BEFORE YOU BUILD**: Before implementing any performance fix, run a
    pre-build A/B experiment on the ACTUAL generated IR (see Performance
    Recovery Protocol). A refuted hypothesis blocks the fix. A regression caused
    by removing a fragile-but-correct optimization is fixed by REBUILDING it on
    the current architecture — never accepted, never re-added as heuristics.
21. **DELIMITER SEMANTIC LOAD**: Each delimiter carries one honest meaning:
    `<>` = compile-time type-level specialization (generics `Stack<T>`,
    protocol variants `#String<UTF8>`, targets `asm<chip>`, groups
    `sync<group>`); `()` = application & binding (calls `f(a)`, params
    `defn f(x)`, construction `Person(...)`, op bindings `op Add: func(#Lh,#Rh)`
    — declarations take params, so `op Add(Float)` is `()`); `[]` =
    containment/bound (`Int[8]`, `[pre]`); `{}` = grouping/definition. Never
    use a delimiter for a different load.
22. **NO IMPLICIT CONCURRENCY**: The reactor never silently decides whether two
    reactive nodes may fire together. If the proof engine proves `pre_A ∧ pre_B`
    satisfiable AND there is no XOR read-write overlap between A and B, the
    compiler DEMANDS the developer classify the pair — `async` on both (explicit
    acknowledgement of simultaneous firing) or `sync<group>` on both (group
    barrier: members that fire hold off finishing until all fired members have).
    An unclassified eligible pair is a compile error.

## Performance Recovery Protocol

When a benchmark is at/above parity but a mechanism made it faster before:

1. **Find the fast era.** Read `benchmarks/results/` ratio history for the
   benchmark. Identify the commit/era where it was at or below parity.
2. **Isolate the regression window.** `git log --oneline <fast>..<slow>` over
   the codegen files. Don't assume — verify which commits changed the emission.
3. **Read the removal plan.** The plan that removed the mechanism documents WHY
   and usually the principled alternative. The removal reason decides the
   response: *fragility* ⇒ rebuild on current analysis; *wrongness* ⇒ reject.
   The current plan may describe the intended end-state better than the current
   code implements it.
4. **Derive the principled version** in terms of the CURRENT frontend analysis
   (LoopShape, `node_decompose` segments, CastingGraph) — never re-add the
   removed heuristics verbatim.
5. **Experiment before building.** Transform the actual generated `.ll` when the
   hypothesis is an IR property; use a hand-peeled `.bv` variant only when the
   structure requires it. Link with the harness's exact command. Verify output
   equality at a BOUND that crosses a print boundary before timing. Interleave
   reference/experiment/C timings ×N (`LC_ALL=C /usr/bin/time -f "%e"`) and
   compare averages. Record the full protocol + results in the plan.
6. **The LTO lesson.** `llc -O2` / raw `.ll` inspection does NOT reflect the
   `-O3 -flto` pipeline used by the benchmark harness. Verify every codegen
   claim (loads, hoisting, folding) against the actual linked binary before
   acting on it.

Experiment link command (match the harness exactly):

```bash
clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections \
    -Wl,--gc-sections "<name>.ll" "lib/runtime/briev_rt.c" -o "<name>"
```

## Architecture Pillars

- **Types are protocol + metadata.** Nothing else: no cached LLVM type, no
  precomputed layout, no name-based lookup.
  `type Int32: Int { spec Bits: 32; };` is the complete definition (plain
  parent name — fundamentals carry no `#` in type positions; physical
  metadata is `spec`, §8.2). Everything
  else is derived from `(protocol, metadata)` by the casting graph at codegen
  time.
- **The casting graph is the single source of truth.** Cast paths
  (`find_path()`), LLVM type resolution (`resolve_llvm_type()`), and protocol
  variant membership all live there. Every codegen site asks
  `self.ctx.casting_graph.resolve_llvm_type(universe, ty, int_bits)` — no
  exceptions, no `rt.properties["llvm_type"]` fallbacks.
- **The normalizer's one job** is registering types in the universe. It does
  NOT resolve LLVM types, inject `Cast.#`, or compute layouts.
- **Frontend-driven dispatch.** The backend CONSUMES decisions; it does not make
  them. Loop shapes, swan-song hoists, density, modulo partitions, inline
  decisions, and unguarded-FFI sets are computed once in the frontend
  (`AnalysisResults`) and read by the backend. Tunables live in
  `config/targets.toml` + `config/ir-lowering.toml`. See
  `docs/plans/2026-07-31-frontend-driven-dispatch.md`.
- **`#Category` hashwords** (`#Int`, `#Float`, `#String`, …) are backend
  directives in op signatures; `#Link<name>` emits `-l<name>`; `#System` is the
  sole bare protocol hashword. See `docs/architecture/hash-words.md`.
- **Intrinsics vs stdlib**: `rm -rf lib/std && brievc --no-stdlib` still
  type-checks `let x: Int = 5` ⇒ intrinsic; else stdlib.

## Observability as Liveness

A program with no observable effect IS dead code — the compiler is right to
eliminate it. **A value is live if an FFI call consumes it.** The fix for a
folded loop is NOT liveness hacks (`x == x`, synthetic exit fields) — it's
`endprogram __print_int(result)` (structurally live swan song) or a
runtime-determined bound (`GetEnvInt#("BOUND")`, never `const N`).
Precomputation is correct, not a bug. `--optimize-budget` (default 256)
controls simulation depth; increase it or use runtime bounds — never weaken
contracts.

## Benchmarks

- **Semantic goals, not syntax**: "Can Briev compute X competitively vs C?" —
  not "Does Briev have feature Y?"
- **Benchmarks exist to find flaws**: a failing benchmark means something is
  missing; a "too good to be true" time means the compiler folded dead code.
- **Symmetric by default**: same output as the C reference. When approaches
  differ fundamentally, create `_sym` (mirrors C step-for-step) and `_idio`
  (idiomatic, Briev-native patterns) variants. Never hobble C with `volatile` —
  fix Briev to match or beat C.
- **Two categories**: `--runtime` (throughput, FFI in hot loop) vs
  `--optimizer` (compile-time folding). The harness detects precomputed
  binaries by `.text` ratio.
- **Useful utilities become stdlib functions.**
- When a C pattern can't be ported directly, find the isomorphism (see
  `docs/architecture/benchmark-strategy.md`).

## Plans & Documentation

1. Write `docs/plans/YYYY-MM-DD-<topic>.md` before starting plan-driven work.
2. Update `docs/architecture/` in the SAME commit as structural changes.
3. Outdated docs are bugs. Update the tutorial, `spec/SPEC.md`, and the syntax
   highlighter when syntax changes.
4. Behavioral tests, not literal tests — a test must pass after refactoring if
   the behavior is preserved. Test the contract, not the implementation.
5. Timestamped records (`docs/plans/`, `benchmarks/results/`, milestones) are
   historical — never retroactively edit them; reference them.

## Working Rules

- **Helpful diagnostics** — every user-facing error/warning must state what is
  wrong, supply the relevant proof/why where one exists (e.g. which obligation
  failed), and give the concrete fix. Never dismiss the code or author, and do
  not reference compiler-internal mechanics or documentation file paths.
  Terse, factual, and kind beats verbose. Supply the trivial proof where the
  failure is about proving (e.g. `[true][true]` → "true ⇒ true is trivial").
  See `src/errors.rs` for the house style; sweep existing messages
  opportunistically when a file is touched.
- **Flat control flow** — max 2 nesting levels. Use `?`, `if let`, guard
  clauses, early returns. Deeper logic goes in named helpers. `else if` chains
  deeper than one level are forbidden.
- **HashMap iteration determinism** — every HashMap iteration producing LLVM IR
  MUST be sorted by key (SipHash seed varies per process, up to ~9% perf
  variation). See `docs/architecture/agent-reference.md` §4.
- **Continuous commits** — commit after each logical step; auto-commit when a
  step is complete and tests pass (do not ask). `git add` only intended files;
  never amend; never use `git checkout --`/`git restore`.
- **Per-commit checklist**: `cargo test --lib` green; `cargo build` no new
  warnings; Praetor on changed files (complexity ≤ 15, lines ≤ 100, params ≤ 6;
  `praetor validate --warn --target <dir>` per changed directory — **`--target`
  takes a DIRECTORY, never a file**); Kani harnesses for safety-critical code;
  update architecture docs if API contracts changed; log bugs in BUGS.md.
- **Regression guard**: inspect every match arm (silent regressions come from
  removed arms); verify optimized IR, not just tests; update architecture
  comments; never delete rationale comments — rewrite them.
- **Kernel index math needs the on-device gate**: any commit touching
  generated-kernel addressing, masks, swizzles, or decode runs the device
  correctness check at a real shape (e.g. gemm_h_bench 4096³) before push —
  timing-only verification missed a 3-day fill corruption (dd5f5e26, 2026-09-08;
  BUGS.md). Emitted-SPIR-V bitwise ops are additionally pinned by
  `coopmat_fill_bitwise_rhs_are_mask_consts`.
- **System-level changes**: trace the full data flow; verify claims in source
  (file:line), not memory; check `git diff --stat` between eras; map ALL
  benchmarks not just the regressed one; identify every gate on the path and the
  single decision point that matters; state the hypothesis AND its verification
  test, then RUN it.
- **Interpretation of benchmark numbers**: never blame "noise" or "HashMap
  iteration order" without a controlled A/B (old vs new compiler, full suite,
  same machine). Document results before corrective action.

## Commands

- **Build**: `cargo build` · **Test**: `cargo test --lib`
- **Test backend registry**: `cargo test --lib -- backend::tests`
- **Compile RBV**: `./target/release/brievc rbv <file.rbv>`
- **Benchmark**: `bash benchmarks/build_and_bench.sh` — always use this harness.
  Ad-hoc timing produces false hangs and imprecise numbers.
- **Compare against baseline**: `bash benchmarks/compare_baseline.sh <name>`
- **Praetor (changed files)**: `praetor validate --warn --target <dir>` where
  `<dir>` is the **directory** containing your changed files (e.g.
  `praetor validate --warn --target src/backend/llvm`). `--target` is a
  directory, **not** a file — `--target src/foo.rs` prints "target is not a
  directory" and **silently passes without analyzing anything** (exit 0). To
  check a single file, copy it to a temp dir and target that dir:
  `mkdir -p /tmp/pt && cp src/foo.rs /tmp/pt/ && praetor validate --warn --target /tmp/pt`.
  Pre-existing diagnostics in untouched files are the project baseline — the
  gate is "no NEW diagnostics in changed files", not a clean tree.
- **Praetor (full project)**: `praetor report` (markdown/html to stdout or
  `--output`). `praetor validate` exits 1 on ERROR-level diagnostics;
  `--warn` also fails on warnings. See `docs/architecture/praetor-log.md`.
- **Praetor policy (2026-08-01)**: run Praetor on changed files as part of the
  per-commit checklist to **keep the codebase clean**; there is **no pre-commit
  git hook** (decided — do not re-enable). `[intent]` validation stays
  **disabled** in `.praetor.toml` (do not enable).

## Reference Index

| Resource | Location |
|----------|----------|
| **Briev-native runtime + family realignment (active 2026-09-09)** | `docs/plans/2026-09-09-briev-native-runtime-and-family-realignment.md` — briev_rt.c elimination family-by-family (A+B+C DONE on `feat/briev-native-runtime`: cast lanes, print family, string ops — 14/14 parity corpora), embedded fold, Electronics `.ebv`, `.sbv` Silicon, logo swap. Parity harness: `bash benchmarks/parity/run.sh`. Amendments (2026-09-10): expressiveness closure, allocator ownership, `Asm#` two-mode intrinsic, `inline_frgn!` plugin |
| **Briev capability frontier (2026-09-10)** | `docs/architecture/briev-capability-frontier.md` — expressiveness-closure principle (the compiler's optimum must be expressible in the language), tier table, contracts-vs-UB inversion, self-hosting endgame |
| **GPU backend handoff (.abv/.bv, active 2026-08-31)** | `docs/HANDOFF-2026-08-31-gpu.md` — SPIR-V kernel backend + offload chain state, doctrine (.abv = pure GPU, .bv = CPU + verified offload), next steps (M1 GEMV bench, O2-O6 ladder), trap list. Also: `docs/plans/2026-08-31-vitriol-gemm-comparison.md` (benchmark target + ledger), `docs/plans/2026-08-31-abv-gpu-by-default.md` (route fixes log)
| **GPU CODEGEN DOCTRINE (.abv = peak everywhere, active 2026-09-04)** | `docs/architecture/abv-gpu-doctrine.md` — one program → peak on every probed device; Briev-owned codegen only (no nvcc/cuBLAS in the path); backend tier architecture (portable SPIR-V + per-vendor projections from ONE frontend plan); coopmat ceiling physics + the standing microkernel measurement. Campaign: `docs/plans/2026-09-04-beyond-coopmat.md` (Stage 0 ceiling truth → Stage 1 portable extraction → Stage 2 Briev PTX tier, gated)
| **cbv foreign HW + MMIO (active 2026-08-27)** | `docs/plans/2026-08-27-cbv-foreign-hardware-and-mmio.md` — extern HDL imports + `@addr` MMIO pins on .cbv; Slice C (VolatileLoad#/Store#) DONE |
| **Backend scaffolding set (active 2026-08-23)** | `docs/plans/2026-08-23-backend-scaffolding-foundation.md` (foundation, first) + parallel: `-vm-compile-tail-parity`, `-spirv-kernel-emission`, `-circt-toolchain-validation`, `-webstack-v2-completion` |
| **Capability matrix (backend surfaces)** | `src/backend/capabilities.rs` — declare before emitting, reject out-of-surface programs with what/why/fix |
| **Backend contracts & decision record** | `docs/architecture/backend-contracts.md` — per-backend charters, emission invariants, ABI decisions, known limitations |
| **Rigorous methodology (REQUIRED reading)** | `docs/handoff-methodology.md` — the investigate→plan→experiment→implement→verify→document loop, evidence standard, and failure modes, with the frontend-driven-dispatch session as the worked example |
| **Language syntax, contracts, coding standards, backend rules** | `docs/architecture/agent-reference.md` |
| **Full pre-rewrite guidelines** | `AGENTS.md.archive` |
| **Historical context** | `AGENTS_HISTORY.md`, `AGENTS_HISTORY_2.md` |
| **Session report (2026-08-27)** | `docs/2026-08-27-session-report.md` — end-state map, working discipline (tree-revert hazard), live gates, deferred index, first tasks |
| **Session report (2026-07-31)** | `docs/2026-07-31-session-report.md` |
| **Bug diagnoses** | `BUGS.md` |
| **Architecture overview** | `docs/architecture/overview.md` |
| **GPU backend strategy (full optimization landscape)** | `docs/architecture/gpu-backend-strategy.md` — Roofline reality, emitter-route evaluation (hand-PTX vs LLVM NVPTX vs SPIR-V), async pipelines (cp.async/ldmatrix), Briev "beat CUDA" levers (cross-node fusion, zero-divergence vectorization, bank-conflict synthesis), multi-vendor matrix, roadmap |
| **GPU strategy selection (shape-driven, 2026-09-16)** | `docs/architecture/gpu-strategy-selection.md` — the cost model (`src/analysis/gpu_strategy.rs`) that picks tile/stages/load-path per shape; wins small shapes (64³–256³) vs cuBLAS, parity at 1024³/2048³, loses 512³/4096³ (kernel-efficiency gap); fused-attention 10×-regression finding. Full lever ledger: `docs/plans/2026-09-16-gpu-strategy-findings-and-levers.md` |
| **Backend type dispatch** | `docs/architecture/backend-type-dispatch.md` — read first before backend type code |
| **LLVM backend architecture** | `docs/architecture/backend-architecture.md` — read first before LLVM backend changes |
| **Casting protocol** | `docs/architecture/casting-protocol.md` |
| **Hash words** | `docs/architecture/hash-words.md` |
| **Benchmark strategy** | `docs/architecture/benchmark-strategy.md` |
| **GLUE FFI (how to link/export/import/add a language)** | `docs/architecture/glue-ffi.md` + `docs/guides/ffi-and-export.md` |
| **Intrinsics vs stdlib** | `docs/architecture/intrinsics-vs-stdlib.md` |
| **Iterable protocol (iteration/length/String/reflection)** | `docs/architecture/iterable-protocol.md` — read before foreach/b-each/collection/reflection code; plan: `docs/plans/2026-08-12-iterable-protocol.md` |
| **String unification + boundary (implemented 2026-08-14)** | `docs/plans/2026-08-14-string-unification-and-boundary.md` — `#String` is `Iterable<Char>`, `.^^Element`, `Abs#`/bit-intrinsic migration, slice-6 blockers |
| **Universal chaining (implemented 2026-09-16)** | `docs/architecture/universal-chaining.md` — dot unifies all call surfaces (member/`#`/`$`/`!`/reflection); power features `.N>>`/`.name>>` back-refs, `>> name` captures, `.(Type)>>` casts. Plan: `docs/plans/2026-09-16-universal-chaining-rules.md` |
| **Frontend-driven dispatch (active plan)** | `docs/plans/2026-07-31-frontend-driven-dispatch.md` |
| **kalman/float_math parity (active plan)** | `docs/plans/2026-07-31-regain-kalman-float-math-parity.md` |
| **accel GPU offload (implemented 2026-08-06)** | `docs/plans/2026-08-06-accel-gpu-offload.md` |
| **endprogram/beginprogram (active plan)** | `docs/plans/2026-08-06-endprogram-beginprogram.md` |
| **Execution model (reactor/pseudo-loop)** | `docs/architecture/briev-execution-model.md` — the reactor IS the scheduler; no `main()`, `beginprogram` is sugar, equilibrium = idle; read before any embedded or reactive work |
| **Defn liveness emission** | `docs/architecture/defn-liveness.md` — imports grant capability, liveness gates emission; dead defns are never emitted (no LTO dependence); intrinsic→helper table in `src/analysis/defn_liveness.rs` |
| **Machine entry (bootstrap, @ wiring, wake sets)** | `docs/architecture/machine-entry.md` — bootstrap node = authored reset; `node @ vector` = machine-vectored, `node @ address` = reactor-pass; frontier-driven equilibrium (wake sets); register shim pattern; pinned symbol ABI. Plan: `docs/plans/2026-09-14-bootstrap-kernel.md` (+ Addendum A) |
| **Spec / tutorial** | `spec/SPEC.md`, `learn-briev/` |

## For OpenCode

1. Read CLAUDE.md and this file for full context.
2. Follow Contract-First Philosophy — never weaken contracts.
3. Test with `cargo test --lib` before committing.
4. Document bugs and root causes in BUGS.md.
5. Never add Rust built-ins for things the standard library provides.
6. **No prototyping**: every optimization is a first-class pass in its module.
7. **Never weaken C benchmarks**: fix Briev to match or beat C.
8. **Interpreter IS the reference**: add to interpreter first, then codegen.
9. Write `docs/plans/YYYY-MM-DD-<topic>.md` before plan-driven work.
10. Update `docs/architecture/` in the same commit as structural changes.
11. Add Kani proof harnesses for all new safety-critical code.
12. Run Praetor on new/changed files.
