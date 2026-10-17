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

## 5. Results (filled after implementation — do not edit predictions above)

To be recorded: per-bench table (actual vs prediction, band verdict),
gate outcomes, deviations and their explanations, A/B cross-check vs
`8962a2a1` for the big movers, and the final BUGS.md close-out.
