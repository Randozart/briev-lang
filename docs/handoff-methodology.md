# Handoff: the Rigorous Methodology

**2026-07-31.** This is a REQUIRED-READING companion to `AGENTS.md`. It captures
the precise, academically rigorous methodology the Briev compiler demands, with
this session's frontend-driven-dispatch work as the worked example. Where
`AGENTS.md` states the rules, this document shows the *practice*: the exact loop,
the evidence standard, and the failure modes.

Read it before any plan-driven work, any performance investigation, or any
"trust me, this is faster" change.

---

## 1. The methodology loop

Every non-trivial change goes through six stages. Skipping a stage is how
regressions happen.

```
INVESTIGATE → PLAN → EXPERIMENT → IMPLEMENT → VERIFY → DOCUMENT → commit
    └──────────────┬───────────────────────────────┘
                   └─ evidence at every stage, file:line or measured number
```

### 1.1 Investigate — find the truth before forming the hypothesis

- **Read history first.** `git log`, `benchmarks/results/*.md`, `docs/plans/`,
  `BUGS.md`. The fast era, the regression window, and the removal rationale
  already exist somewhere. In this session: kalman's 0.99×/1.01× era was found
  in the results docs; the batch-loop's removal rationale was IN the
  flat-node-decomposition plan — the plan itself stated the principled
  alternative ("the io boundary IS the precondition interval").
- **Read the plan that removed a mechanism.** It documents WHY and usually the
  principled alternative. The removal reason decides the response: *fragility*
  ⇒ rebuild on current analysis; *wrongness* ⇒ reject.
- **Verify claims in source, not memory.** Every "X does Y" must cite
  `file:line`. Example: "the const identifier path discards the value" was
  pinned to `emit_expr.rs:242` before acting on it.
- **Prefer the real pipeline over inspection.** `llc -O2` and raw `.ll` do NOT
  reflect `clang -O3 -flto`. This session's const-inlining hypothesis looked
  correct under `llc -O2` and was refuted by the actual linked binary.

### 1.2 Plan — write it before you code

- `docs/plans/YYYY-MM-DD-<topic>.md` with: Goal, **baseline table (all
  benchmarks, Golden Rule 11)**, investigation evidence, the fix mechanism,
  risks/trade-offs, documentation requirements, implementation phases, and a
  results section to fill as you go.
- A plan is a living record: results get appended after each phase (the plan's
  `§9/§10` were filled with the measured A/B).

### 1.3 Experiment — validate the hypothesis BEFORE building (Golden Rule 19)

- Transform the ACTUAL generated `.ll` when the hypothesis is an IR property;
  use a hand-peeled `.bv` only when the structure requires it.
- Link with the harness's EXACT command:
  `clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections
  -Wl,--gc-sections <name>.ll lib/runtime/briev_rt.c`.
- **Verify output equality at a BOUND that crosses a print boundary before
  timing.** (BOUND=10M for a 5M-periodic guard.)
- **Interleave** reference/experiment/C timings ×N and compare averages
  (`LC_ALL=C /usr/bin/time -f "%e"`). Isolate the sensitive loops and count
  instructions in the disassembly — a 14-instruction vectorized loop can be
  slower than a 29-instruction scalar one (the batch-fmn finding).
- **A refuted hypothesis blocks the fix.** This session: const-inlining was
  refuted and NOT implemented; the batch loop was validated, implemented, then
  the countdown was A/B'd against it on every periodic-guard benchmark and
  won, so the batch was replaced.
- Record the full protocol + results in the plan. `/tmp` artifacts die between
  sessions; the plan survives.

### 1.4 Implement — no stubs, no special cases

- Every optimization is a first-class pass in its proper module (no inline
  analysis in codegen). Wire parser → AST → analysis → codegen → tests.
- **Frontend-driven:** the backend CONSUMES decisions; it does not re-derive
  them from body re-walks or hardcoded names. This session moved loop shapes,
  swan songs, density, modulo partitions, inline decisions, and batch shapes
  into `AnalysisResults`, and eliminated every `Type::Custom.*==` match.
- When you discover a latent bug in a file you touch, FIX IT NOW (the
  implicit-coercion bug, the `sitofp to double` cast, the float-param alloca —
  all found by writing the new benchmarks and fixed in the same session).

### 1.5 Verify — the gates

- `cargo test --lib` green, `cargo build` no new warnings, Praetor clean.
- **Verify the OPTIMIZED IR, not just the tests.** The accumulator_flush
  printed 0 because `Int * Float` silently bitcast — a test of the benchmark
  output caught it only because the C reference was compared.
- **Correctness is checked at the harness's BOUND (5) — which is often vacuous
  for periodic-guard benchmarks (no output).** Do NOT trust the harness for
  float value equality; cross-check values against an exact (`-O0`)
  computation and the C reference at a print boundary.
- Full `--runtime` A/B vs the plan's baseline table, zero MISMATCH.

### 1.6 Document — same commit as the change

- Architecture docs in the same commit as structural changes.
- Rationale comments at every site (`// 2026-07-31: …`) with what it targets,
  why, and how to undo it.
- Findings and bug root-causes in `BUGS.md`.
- Results in `benchmarks/results/`.
- Never delete rationale comments — rewrite them.

---

## 2. The evidence standard

A claim is either **pinned** (`file:line`, or a measured number with the exact
command that produced it) or **unsupported**. Unsupported claims are rewritten
or dropped. Examples from this session:

| Claim | Evidence |
|-------|----------|
| "kalman's gap is the guard, not the compute" | pure-loop experiment: 0.1575s vs version-DAG 0.1962s; C 0.1600s |
| "the batch mis-vectorizes fmn" | fmn batch inner loop = 14 instructions (`vmulps`+shuffles), slower than the 29-instruction scalar version-DAG loop (disassembly) |
| "the countdown is universal" | A/B on all five periodic-guard benchmarks; then the sweep family disproved it for cross-indexed chains |
| "the version-DAG emits 5M+1 computes" | its BOUND=5M output (8.188e12) vs the exact `-O0` 5M computation (8.139e12) |

## 3. Failure modes (learned the hard way)

1. **Inspection over measurement.** `llc -O2` said const-inlining was a win;
   the `-flto` pipeline said no. Always measure the real binary.
2. **A benchmark that gets too fast too quickly is a signal.** 0.09× on a real
   workload usually means reassociation or a fold changed the output. Check the
   values, not just the ratio.
3. **The guard conditional is not a scheduling tool.** It happens to block
   mis-vectorization on some bodies and not others. Don't rely on it — derive
   the dispatch from the body's structure.
4. **Tests at BOUND=5 don't test periodic guards.** Verify at a print boundary.
5. **Don't trust the type system to catch mixed arithmetic.** It didn't
   (silent bitcast). Enforce it (the new type error).
6. **A "universal" optimization is a hypothesis until a counterexample.** The
   sweep benchmarks exist to find the counterexample.
7. **Timing-only verification is not verification.** dd5f5e26 corrupted the
   coopmat fill for three days while reading "GPU time unchanged" — the
   instruction count didn't change, so timing couldn't see it. Any commit
   that touches kernel index math (addressing, masks, swizzles, decode)
   needs the on-device correctness gate at a real shape before push.
8. **Tests green ≠ device correct for shape-dependent addressing.** No unit
   test exercised the fill's `b_flat_within` modulo at the real 4096³
   geometry; the conformance sweep parses code, it doesn't run it. The
   standing catch is the cross-tier A/B harness (gemm_h_bench per tier,
   one command each) plus the emitted-SPIR-V bitwise-RHS guard test
   (backend::spirv::tests::coopmat_fill_bitwise_rhs_are_mask_consts).
9. **Aliases erode type safety exactly where ids and values meet.**
   rspirv's `type Word = u32` let five call sites pass result ids into a
   literal-mask parameter — compiles clean, corrupts silently. When a
   helper takes a primitive in a literal position, audit its call sites
   and prefer taking the const's Word instead; the guard test now pins
   the emitted output.

## 4. The discipline in one paragraph

Understand the history before you propose. Write the plan with a baseline.
Validate with a cheap experiment on the real IR before building. Build the
principled mechanism, not a special case. Verify the optimized IR and the
values, not just the tests. Document in the same commit. If a benchmark
regresses, rebuild the mechanism on the current architecture — never accept
it, never excuse it as noise without a controlled A/B.

## 5. The design principles (2026-07-31)

Three principles govern every language and compiler decision. They are the
*why* behind the `#`/`!`/`.^` markers, the modifier family, and the concurrency
gate. Apply them to any design before writing code.

### 5.1 Avoid accidental complexity; no obfuscation of special treatment

"<b>No magic</b>" is a naive purist trap — every compiler has intrinsics and
special cases. The honest rule has two parts:

1. **Avoid accidental complexity.** Essential complexity (SMT, LLVM IR
   emission) is kept; accidental complexity (heuristic trees, hand-rolled
   passes that fight the design) is stripped, never preserved. Ask: does this
   code solve a real problem, or does it fight the architecture?
2. **No obfuscation of special treatment.** Compiler-known behavior is
   *disclosed*, never hidden: `#` (intrinsic `Sqrt#`, hashword `#Int`),
   `!` (compile-time expansion `my_macro!`), `.^`/`.^^` (reflection). A
   developer never has to guess whether `x + y` is a standard op or a macro —
   the markers make it visible.

### 5.2 The never-faster contract

No instruction may ever make code faster. Modifiers (`seq`, `vol`, `async`,
`sync<group>`) exist only to *restrict* the optimizer or demand a specific
behaviour. The default is always the efficient path. If a modifier-beaten
program is faster than the default, that is a **compiler bug** — fix the
default, never let the modifier be the win. This is what keeps the language
free of an optimization layer only advanced compiler engineers understand.
When you see a modifier "winning" a benchmark, treat it as a bug to fix, not a
feature to adopt.

### 5.3 No implicit concurrency

The reactor never silently decides whether two reactive nodes may fire
together. If the proof engine proves `pre_A ∧ pre_B` satisfiable AND there is
no XOR read-write overlap, the compiler DEMANDS the developer classify the
pair: `async` on both (explicit acknowledgement of simultaneous firing) or
`sync<group>` on both (a group barrier). An unclassified eligible pair is a
compile error. The compiler's job is to *prove* safety or *demand* a decision —
never to guess.

### 5.4 The delimiter semantic load

Each delimiter carries one honest meaning: `<>` = compile-time type
specialization (`Stack<T>`, `#String<UTF8>`, `asm<chip>`, `sync<group>`);
`()` = application & binding (`f(a)`, `Person(...)`, `op Add: func(#L,#R)`);
`[]` = containment/bound (`Int[8]`, `[pre]`); `{}` = grouping/definition.
A delimiter used for the wrong load is a design error.
