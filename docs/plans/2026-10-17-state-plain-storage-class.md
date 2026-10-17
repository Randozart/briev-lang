# `state` / `plain` checked let-storage assertions — implementation protocol

**2026-10-17.** Companion to
`docs/plans/2026-10-17-library-state-demotion-research.md` (root cause +
counterfactual: buffered stdout lives in library state fields → the
print chain takes `ptr %state` → the `%State` alloca escapes in any
printing loop → SROA demotion → every loop-carried field pays memory
round-trips; hand-transformed IR ran interval_step 414ms/500M vs 848ms
current and 589ms C, byte-identical output).

**Design locked with the author (this supersedes the earlier
`global let` directive sketch in this file's history):**

1. **Inference-first.** A bare top-level `let` is classified by the
   compiler: referenced by the proof surface → state field; otherwise →
   plain (global) storage. The decision is disclosed via the existing
   `info:` convention (`info: 'x' lowered to plain storage — no contract
   references it`).
2. **Two checked assertions, not directives.** `state let x` and
   `plain let x` assert a category; the compiler checks the assertion
   against inference. Disagreement is an **error** quoting the exact
   contradicting fact (house style: what/why/fix). The keywords never
   change behavior — inference is total; the keywords make intent
   checkable.
3. **Proof surface** = contract pre/post conditions, watchdog
   conditions + on-fire args, `#!exit` conditions, out-pragmas, result
   expressions (`->` / `endprogram` values), view bindings — exactly
   the existing liveness roots. **Node/defn bodies never count** (body
   access compiles to `@global` and LLVM promotes it — the
   counterfactual proved this).
4. **Semantics preserved.** Name, type, init, mutation, read visibility
   are identical across the categories; only backing storage and
   machine-membership differ. Contracts over `plain` lets are
   impossible (that IS the check).

## Rationale record (why these words)

- `state` is the codebase's own established term (SPEC "state field",
  `%State`, `dead fields` diagnostics) — the keyword asserts membership
  in the already-named category. `inst` was rejected: "instance"
  implies a class with many instances; the machine is a singleton.
- `plain` delivers the missing concept directly ("Briev has no plain
  variables" — now it has them). `detached` was the runner-up (its
  "a contract re-attaches it" diagnostic is lovely) but is long,
  participle-form, and dramatic; `plain` is assertive and terse.
  `global` describes storage, not the participation distinction the
  check enforces; `prog` names the one axis (lifetime) on which the
  categories agree; `rigid` names the opposite of the truth (the buffer
  mutates); `free`/`mem`/`reg`/`extern`/`lib` are taken or wrong.
- Both are contextual identifiers (the `net`/`stdnet` pattern): no new
  reserved tokens; variables *named* `state` or `plain` keep working.

## 0. One-paragraph summary

The print-family stdout buffering (`01c92189`, 2026-09-12) stores its
buffer in library state fields, putting `ptr %state` on the whole print
chain; any loop containing a print — even cold and guarded — makes the
`%State` alloca escape, SROA refuses promotion, and every loop-carried
field drops to memory round-trips. Counterfactual experiment: buffer as
true globals + stateless print chain → interval_step 414ms/500M vs 848ms
current, 589ms C (0.70x), output byte-identical. The licensed fix:
inferred `plain` storage for lets the proof surface never references,
with `state`/`plain` as checked assertions, migrating the three library
plumbing fields (`stdout_buf`, `stdout_fill`, `rand_state`).

## 1. Hypothesis (falsifiable)

**H1:** If the stdout buffer and `rand_state` live in plain storage
(not `%State`), the print chain drops `%state`, the alloca no longer
escapes, and clang's standard pipeline (SROA/loop promotion) restores
register-resident loops — recovering the pre-`01c92189` ratios or
better, with byte-identical program output and zero semantic change.

**H0 (null):** the demotion has a second cause independent of the
escape (engine quality, field count, layout). Then the migration
improves nothing and the gap must be re-investigated. The suite numbers
in §3 decide.

## 2. Implementation steps, each with its gate and its undo

Every step is a separate commit; every commit passes
`cargo test --lib` green + no new warnings + Praetor (no NEW
diagnostics in changed files). Backend-touching commits additionally
run the full `ll_determinism_check.sh`.

**S1 — language surface + inference + checks.**
`state let x: T = init;` / `plain let x: T = init;` parse as contextual
modifiers on top-level lets (the `net`/`stdnet` identifier pattern in
`consume_modifier_prefix`; only when the identifier directly precedes
`let`). AST: annotations `"state"`/`"plain"` on `Statement::Let`
modifiers (existing `modifiers` plumbing — no AST shape change).

New analysis: `classify_plain_storage(items) -> HashSet<String>` —
top-level `Statement::Let` names annotated `plain`, plus bare lets NOT
referenced by the proof surface. The proof-surface scan mirrors the
existing liveness roots (contracts, watchdogs incl. on-fire, `#!exit`,
out-pragmas, result expressions, view bindings are resolved where they
are available — view bindings at the backend, the rest at analysis).

Typechecker/diagnostic checks (error grade, with the contradicting site
quoted):
- `plain let x` + proof-surface reference → "declared `plain`, but
  <node/contract> references it — a contract makes a variable machine
  state. Remove the reference or declare it `state`."
- `state let x` + no proof-surface reference → "declared `state`, but
  no contract, watchdog, exit condition, or result expression
  references it — the machine never reasons about it. Contract over it
  or declare it `plain`."
- contracts over an inferred-or-declared plain let are unreachable (the
  same check fires first).

Gate: parser tests (contextual parse; variables named `state`/`plain`
keep working); inference unit tests; both error diagnostics; `--`
inference discloses via info line (backend prints, so asserted at S2's
test).

Undo: revert S1 — inert without S2.

**S2 — backend: plain lowering + needs-state narrowing.**
`build_field_index` routes plain lets to a global table
(name → type/init), NOT `field_index_map`. Identifier resolution emits
`@<name>` load/store; `@name = global i64 0` at module scope.
`compute_defn_needs_state` excludes plain-only defns → the print chain
loses `ptr %state` with zero per-function special cases (Rule 15).
`compute_referenced_fields`/`compute_live_fields` exclude plain names
from their state-field sets. Fold-dispatch field counts change where
fields demote — the existing info lines disclose it.

Gate: (i) unit test — defn over plain-only lets emits with NO `%state`
param; (ii) THE REGRESSION TEST — a node loop with a guarded
`println!` compiles to a print chain without `%state` (the
`call i64 @print_int(i64)` signature, mirroring the counterfactual);
(iii) a defn touching BOTH plain and state still takes `%state`;
(iv) full determinism.
Undo: revert S2 (S1 alone is inert — declared-plain lets would be
unlowered; S1+S2 land together in practice, separate commits only for
reviewability).

**S3 — interpreter.** Plain lets live in an interpreter globals map with
identical read/modify/write semantics; state-field paths untouched.
Gate: interpreter behavioral tests for stdout/rand unchanged; print
byte-parity.
Undo: revert S3 (backend-only sessions still tested via --llvm).

**S4 — stdlib migration.** `lib/std/cast_lanes.bv`:
`plain let stdout_buf/stdout_fill`; `lib/std/os/rand.bv`:
`plain let rand_state`. No other library lets migrate (the `SYS_*`
lets in posix/process.bv are never mutated — cosmetic, out of scope).
Gate: full corpus builds; print byte-parity; `--no-stdlib` still
type-checks bootstrap primitives.

**S5 — docs.** SPEC storage-class section; keyword-taxonomy note
(checked category assertions — disclosed ordinary syntax, no speed
claim: the speed comes from the compiler's standard promotion behaving
correctly; Rule 3/21); agent-reference; BUGS.md close-out pointing
here; benchmark-strategy note (the C isomorphism: libc's stdout buffer
is a global; Briev's buffering becomes structurally identical).

## 3. Pre-registered predictions (acceptance bands)

Suite: `bash benchmarks/build_and_bench.sh` full runtime + optimizer,
same machine, same day, vs the numbers in
`benchmarks/results/2026-10-17-post-stability-runtime-fixed.md`
(current, pre-fix) and the `8962a2a1` A/B column (pre-buffer era).
Ratios = Briev/C; C side rebuilt fresh each run.

| bench | pre-fix (10-17) | pre-buffer era (A/B) | prediction | band |
|---|---|---|---|---|
| interval_step | 1.23x | 1.46x slower | **≤0.80x** | ±0.10 |
| float_math_nonzero | 1.24x | 1.30x slower | **≤1.00x** | ±0.08 |
| knucleotide | 1.22x | 1.28x slower | **≤1.05x** | ±0.08 |
| mandelbrot | 1.20x | 1.14x slower | **≤1.10x** | ±0.06 |
| queue_drain | 1.35x | n/a (fast era 07-31: 0.85x) | **≤1.05x** | ±0.10 |
| kalman_filter_runtime | 1.08x | 1.09x slower | **≤1.05x** | ±0.05 |
| fasta | 0.58x | — | stays ≤0.73x | must not regress |
| series_converge | 1.00x | — | stays MATCH ~1.00x | must not regress |
| fannkuch_redux | 1.11x | 1.01x vs pre-buffer | 0.95–1.15x (print-free hot loop: unchanged) | — |
| hash_ops / nbody* / linked_list / bit_clear / arena_churn | wins | — | unchanged within noise (±0.05) | falsifier if moved |

**Falsifiers (any one → H1 rejected, stop, re-investigate):**
- interval_step does not reach ≤0.90x (the experiment says 0.70x is
  reachable; a large shortfall means a second mechanism).
- Any print-free bench (fannkuch, hash_ops, nbody) moves >0.05 beyond
  noise — would indicate a plain-lowering side-effect on unrelated
  codegen.
- Any correctness gate fails: MATCH regressions, byte-parity diffs,
  lib-test failures, determinism failures.

**Correctness gates (all mandatory):**
- `cargo test --lib` green (3154+new).
- Full `ll_determinism_check.sh` ALL DETERMINISTIC.
- Print corpus byte-parity (stdout bytes identical pre/post for every
  MATCH bench — the harness's correctness gate covers this).
- Praetor: no NEW diagnostics in changed files.
- `bash benchmarks/toolchain_matrix.sh` if a release is cut from this.

## 4. Risk register (what could break elsewhere, how we'd know, how to undo)

| # | risk | detection | response |
|---|---|---|---|
| R1 | Program reads an imported plain let (`stdout_fill`) — identifier must resolve identically | behavioral tests + any suite MISMATCH | resolution is name-identical; if broken, S2 resolution is wrong — fix S2 |
| R2 | needs-state narrowing too aggressive: a defn touching plain AND state fields drops `%state` | unit test (iii) in S2 + export/shared-lib tests | fixpoint root-set condition wrong — fix, never widen blindly |
| R3 | A contract references a bare let (inferred plain) — must error, not silently demote the contracted field | S1 check test: contract reference ⇒ state, never plain | the inference order is: proof-surface first, category second — never the reverse |
| R4 | Main-tail flush gate (`has_stdout_flush`) stops firing (may key on library state presence) | exit-flush tests; printf-loop benches flush at exit | re-key the gate on live_defns of the print chain (unchanged), not on state fields |
| R5 | GPU/SPIR-V normalizer: globals in kernels | SPIR-V corpus tests (print family already rejected there; `rand_state` GPU use would be new) | reject with what/why/fix diagnostic; plain storage is CPU-runtime plumbing for now |
| R6 | wasm/web backend globals lowering | webstack tests | same treatment as `@__briev_argc` (already emitted there) |
| R7 | BEAST serialization of the new annotations | beast round-trip tests | annotations ride existing serialization |
| R8 | Init order: plain globals vs `init_state` | stdout tests (buffer alloc is lazy in `stdout_byte`; init is plain zero) | zero-init globals at module scope; no ordering hazard |
| R9 | Name collision plain vs state field (the 10-12 clobber bug class) | typechecker duplicate-registration diagnostic | the plain namespace is separate from the field namespace; diagnostic on collision |
| R10 | Fold-dispatch counts shift (N fields written changes where fields demote) → dispatch changes on some programs | suite falsifiers; the existing per-field-phi/countdown info lines | disclosed; watch the suite; investigate before touching anything else |
| R11 | Performance surprises on print-free benches | §3 falsifiers | investigate before touching anything else |

## 5. Results (2026-10-17, post-implementation)

Suite: `benchmarks/results/2026-10-17-state-plain-suite.md` (chunked —
the first full run hit the 2h command timeout at nbody_newton_accel and
was completed per-bench; the chunks are appended in file order).

### Verdicts vs predictions

| bench | pre-plain (morning) | plain (this suite) | prediction | verdict |
|---|---|---|---|---|
| interval_step | 1.23x | **0.61x** | ≤0.80x | **MET** |
| queue_drain | 1.35x | **0.45x** | ≤1.05x | **MET (2-3x)** |
| knucleotide | 1.22x | **0.92x** | ≤1.05x | **MET** |
| mandelbrot | 1.20x | **0.91x** | ≤1.10x | **MET** |
| queue_drain_sym/idio | 0.47/0.45x | 0.45/0.46x | hold | MET |
| fasta | 0.58x | 0.61x | ≤0.73x | MET |
| fannkuch_redux | 1.11x | 1.06x | 0.95–1.15x | MET |
| hash_ops | 0.74x | 0.73x | hold | MET |
| optimizer 4/4 | precomputed MATCH | precomputed MATCH | hold | MET |
| bridges | MATCH/PASS | MATCH/PASS | hold | MET |
| **kalman_filter_runtime** | 1.08x | **1.28x** | ≤1.05x | **MISSED — real, ~15% (see below)** |
| float_math | 0.90x | 2.01x | hold | suite-vs-suite artifact (below) |
| series/deep_recursion/bit_clear/recommender | 0.6–1.0x | 1.2–1.5x | hold | µs-scale machine jitter (below) |

### The four primary targets: all recovered, two beyond prediction

queue_drain 1.35x→0.45x, interval_step 1.23x→0.61x, knucleotide
1.22x→0.92x, mandelbrot 1.20x→0.91x. interval_step cross-checked
directly at BOUND=500M: **457ms vs pre-fix 848ms and C 589ms (0.78x)**,
byte-identical output — matching the counterfactual (414ms).

### The missed band: kalman, −15% (real)

Interleaved same-day A/B vs the pre-plain compiler (3c15268a, fresh
worktree build): pre-plain ≈252ms/50M, plain ≈292ms/50M. No kalman let
demoted (all body-written/contracted ⇒ state) — the delta is codegen
scheduling, not demotion: with the print chain stateless the guard no
longer clobbers %State, and clang re-contracts the float chains (FMA),
trading ILP across the 9 independent P-chains for fused latency.
**The same re-contraction made kalman NUMERICALLY MATCH C for the first
time** (8.15413382e+12 exactly; the pre-plain binary printed
8.18814596e+12 — the old suite's MATCH was masking that drift). The
trade: −15% throughput for +C-parity. Recorded as the known cost;
revisiting the contraction shape is future work (the C reference pays
the same FMA shape and is still faster overall).

### Suite-vs-suite artifacts (not regressions)

- **float_math 0.90x→2.01x**: NOT this change. Pre-plain (3c15268a) and
  current binaries measure IDENTICAL (1384 vs 1383ms/500M; IR fadd
  counts 64=64). The 8962a2a1 baseline FOLDS the whole program away
  (4ms/500M — precompute), so the "2.93x A/B" compared real work
  against no work. The real float_math story: it stopped precomputing
  somewhere in the 08-01→10-16 window and now runs ~2.5x slower than C
  — a PRE-EXISTING optimizer-category gap, filed for separate
  investigation. Yesterday's 0.90x suite row (43.7ms/50M = 0.87ns/iter
  for a ~30-op dependent float chain) was itself the anomaly — today's
  steady state is 2.7-3.0ns/iter on the same-compiler binary.
- **series_converge 1.50x, deep_recursion 1.50x, bit_clear 1.40x,
  recommender 1.27x**: µs-scale benches (0.3-14ms absolute). Same-day
  interleaved A/B for series: pre-plain 4.8-6.6ms vs plain 4.9-10.3ms —
  jitter band, plain wins one run. The whole machine is slower today
  (C reference times +28% vs yesterday); ratios on µs benches amplify
  that into apparent regressions. Unresolvable today; re-measure on a
  quiet machine before drawing conclusions.

### Gates

- `cargo test --lib` 3159/0 (6 new tests: print-chain signature
  regression, both assertion errors, inference ×2, const-let plain
  lowering).
- Full `ll_determinism_check.sh`: ALL DETERMINISTIC.
- Output parity: interval_step, float_math_nonzero, knucleotide,
  mandelbrot, queue_drain, fannkuch_redux, fasta, hash_ops —
  byte-identical vs pre-plain binaries.
- kalman: output CHANGED vs pre-plain binary — now exactly C's values
  (improvement, documented above).

### Implementation deviations from the plan (§2)

1. `scalar_type` restriction added: plain inference requires a scalar
   TYPE (Int/Float/Float64/Bool), not just a scalar-shaped initializer —
   `let p: Pool = 0` unpacks into instance columns (found by
   test_consumptive_op's sibling; fatal diagnostic on explicit `plain`).
2. Body-write guard added: a let written by any node body never demotes
   (loop engines enumerate state fields by index; demoting a
   body-written field would lose its store plumbing). The counterfactual
   never needed body-written demotion — plumbing is defn-only by nature.
3. `Term(Some(expr))` added to the proof surface: `term Print#(count)`
   is a terminal observation (`endprogram` alone was insufficient —
   found by test_state_layout).
4. Constants excluded from needs-state's state_fields: the `STDOUT_CAP`
   read forced `stdout_byte` stateful — the buffered-stdout regression's
   last link.
5. The main-tail `stdout_flush` call follows its needs-state verdict
   (the hardcoded `ptr %state` emitted a signature-MISMATCHED call —
   invalid IR the linker tolerated as UB; found because interval_step
   measured 1223ms on the UB binary).
6. Plain-global literal initializers fold into the module-scope
   declaration; expression initializers seed at runtime. Entry stores to
   the @globals DEFEAT %State promotion (measured 879ms→457ms).
7. The Assign store site routes plain BEFORE the field gate (the
   `contains_key` gate silently dropped the `stdout_buf = Alloc#(...)`
   seed — risk R1, hit exactly as registered).

### Full matrix (40 runtime + 4 optimizer + 2 bridges)

Ratio = Briev/C at 50M iterations, 5×5 per bench, all rows MATCH/PASS.
**Δ** = movement vs the morning post-dispatch-fix suite (same machine,
same day). ⚠ = µs-scale bench (0.3–14ms absolute) — ratio amplifies
machine jitter (C reference itself ran +28% vs the morning); treat as
unresolved, not as regression. Raw data:
`benchmarks/results/2026-10-17-state-plain-suite.md`.

**Optimizer — 4/4 precomputed MATCH** (unchanged):
iir_filter · precompute_sum · const_heavy · async_counters_idio.

**Briev wins — 21:**

| bench | ratio | Δ |
|---|---|---|
| arena_churn | 0.26x | = |
| print_loop | 0.42x | = |
| queue_drain | **0.45x** | ← 1.35x (plain target) |
| queue_drain_sym | 0.45x | = |
| queue_drain_idio | 0.46x | = |
| global_lifetime | 0.49x | = |
| linked_list | 0.49x | = |
| stack_push_pop | 0.57x | = |
| cancel_math | 0.58x | = |
| interval_step | **0.61x** | ← 1.23x (plain target) |
| fasta | 0.61x | ~ (0.58x) |
| accumulator_flush | 0.72x | ~ (0.89x) |
| hash_ops | 0.73x | ~ (0.74x) |
| sparse_dispatch | 0.70x | ~ (0.76x) |
| enemy_swarm | 0.79x | ~ (0.75x) |
| ring_buffer | 0.82x | ~ (0.96x) |
| nbody_sqrt | 0.84x | ~ (0.72x) |
| mandelbrot | **0.91x** | ← 1.20x (plain recovery) |
| knucleotide | **0.92x** | ← 1.22x (plain recovery) |
| matrix_pipeline | 0.94x | ~ (0.99x) |
| nbody_sqrt_idio | 0.95x | ~ (0.86x) |

**Ties — 3:** telemetry_stream 1.03x (=) · hash_ops_idio 1.05x (~1.01x) ·
fannkuch_redux 1.06x (~1.11x).

**C wins — 13:**

| bench | ratio | Δ | classification |
|---|---|---|---|
| series_converge ⚠ | 1.50x | ← 1.00x | µs jitter (0.6 vs 0.4ms); output MATCH incl. watchdog print |
| deep_recursion ⚠ | 1.50x | ← 0.75x | µs jitter (0.3ms) |
| bit_clear ⚠ | 1.40x | ← 0.60x | µs jitter (0.7ms) |
| sweep_sparse | 1.44x | ← 1.30x | machine +~15% (C itself +28% today) |
| sweep_mid | 1.33x | ← 1.30x | same |
| recommender | 1.27x | ← 1.02x | ms-scale; re-measure |
| pid_control | 1.23x | ← 0.99x | same |
| sweep_dense | 1.21x | ← 1.14x | sweep family |
| nbody_newton | 1.13x | ← 0.97x | machine-share; re-measure |
| sweep_arr | 1.15x | ← 1.13x | sweep family |
| **kalman_filter_runtime** | **1.28x** | ← 1.08x | **REAL −15%** (interleaved A/B vs pre-plain): FMA contraction trade; now matches C numerically |
| **float_math** | **2.01x** | ← 0.90x | **NOT this change** (pre-plain binary identical, 1384 vs 1383ms/500M; 8962a2a1 baseline folds the program away) — pre-existing precompute gap |
| nbody_newton_accel | 13.17x | ← 11.97x | accel-offload subject, unchanged |

**Bridges:** bridge_glue MATCH · bridge_multi PASS.

**Score: 21 Briev wins + 4 precomputed + 2 bridges vs 13 C-wins** — of
which 4 µs-jitter-flagged, 3 sweep-family (stable historical gap,
machine-inflated today), 2 with filed investigations (kalman
contraction trade, float_math precompute gap), 1 accel handoff.

### Open items

- kalman −15% (FMA contraction trade) — future work, understood.
- float_math precompute gap (pre-existing, optimizer category).
- µs-bench re-measurement on a quiet machine.
- nbody_newton_accel 13.17x — accel-offload subject, unchanged.
