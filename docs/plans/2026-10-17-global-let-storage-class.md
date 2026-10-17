# `global let` storage class — implementation protocol & pre-registered results

**2026-10-17.** Companion to
`docs/plans/2026-10-17-library-state-demotion-research.md` (root cause,
counterfactual experiment, edge cases). This document is the
pre-registration: predictions are written BEFORE implementation, with
acceptance bands and falsifiers. If the feature succeeds or causes
problems elsewhere, this file + the research doc are the complete
record of how and why.

## 0. One-paragraph summary

The print-family stdout buffering (`01c92189`, 2026-09-12) stores its
buffer in library state fields, putting `ptr %state` on the whole print
chain; any loop containing a print — even cold and guarded — makes the
`%State` alloca escape, SROA refuses promotion, and every loop-carried
field drops to memory round-trips. Counterfactual experiment: buffer as
true globals + stateless print chain → interval_step 414ms/500M vs 848ms
current, 589ms C (0.70x), output byte-identical. The licensed fix is a
`global let` storage class: disclosed ordinary syntax, no contract
surface, needs-state fixpoint excludes global-only defns.

## 1. Hypothesis (falsifiable)

**H1:** If the stdout buffer and `rand_state` live in global storage
(not `%State`), the print chain drops `%state`, the alloca no longer
escapes, and clang's standard pipeline (SROA/loop promotion) restores
register-resident loops — recovering the pre-`01c92189` ratios or
better, with byte-identical program output and zero semantic change.

**H0 (null):** the demotion has a second cause independent of the
escape (engine quality, field count, layout). Then global-lets improve
nothing and the gap must be re-investigated. The suite numbers in §3
decide.

## 2. Implementation steps, each with its gate and its undo

Every step is a separate commit; every commit passes
`cargo test --lib` green + no new warnings + Praetor (no NEW
diagnostics in changed files). Backend-touching commits additionally
run the full `ll_determinism_check.sh`.

**S1 — language surface: `global` as a contextual modifier.**
`global let name: T = init;` parses; `global` is a contextual
identifier (the `net`/`stdnet` pattern — no reserved word, so existing
programs using `global` as a name keep working; the arm fires only when
`global` directly precedes `let` in a declaration prefix, where no
comparison can occur). AST: annotation `"global"` on `Statement::Let`
modifiers. Typechecker: (a) global-lets are NOT registered as state
fields; (b) a contract referencing a global-let is a diagnostic (what/
why/fix, house style); (c) initializer must be a constant expression.
Gate: parser/typechecker unit tests (parse, non-state registration,
contract diagnostic, non-const-init diagnostic).
Undo: revert S1; no other step depends on a half-state.

**S2 — backend: globals lowering + needs-state narrowing.**
build_field_index skips global-lets; a new global table
(name → type/init). Identifier resolution emits `@<name>` load/store.
`@name = global i64 0` (const-folded literal init when given).
`compute_defn_needs_state` excludes global-only defns → the print
chain loses `ptr %state` automatically (no special case: the fixpoint
learns one storage class, not one function — Rule 15).
Gate: (i) unit test — defn over global-only lets emits with NO
`%state` param; (ii) THE REGRESSION TEST — a node loop with a guarded
`println!` compiles to a print chain without `%state` (the
`call i64 @print_int(i64)` signature, mirroring the counterfactual);
(iii) a defn touching BOTH global and program state still takes
`%state`; (iv) full determinism.
Undo: revert S2 (S1 alone is inert — global lets would be unlowered;
S1+S2 always land together in practice, separate commits only for
reviewability).

**S3 — interpreter.** Globals map in the interpreter with identical
read/modify/write semantics; state-field paths untouched.
Gate: interpreter behavioral tests for stdout/rand unchanged;
byte-parity of the print corpus.
Undo: revert S3 (backend-only sessions still tested via --llvm).

**S4 — stdlib migration.** `lib/std/cast_lanes.bv`:
`global let stdout_buf/stdout_fill`; `lib/std/os/rand.bv`:
`global let rand_state`. No other library lets migrate (the `SYS_*`
lets in posix/process.bv are never mutated — cosmetic, out of scope).
Gate: full corpus builds; print byte-parity; `--no-stdlib` still
type-checks bootstrap primitives.

**S5 — docs.** SPEC storage class; keyword-taxonomy note (storage
class ≠ strategy keyword: disclosed ordinary syntax, no speed claim —
Rule 3/21); agent-reference; BUGS.md entries closed with pointer here;
benchmark-strategy note (the C isomorphism: libc's stdout buffer is a
global; this makes Briev's buffering structurally identical).

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
  noise — would indicate a global lowering side-effect on unrelated
  codegen.
- Any correctness gate fails: MATCH regressions, byte-parity diffs,
  lib-test failures, determinism failures.

**Correctness gates (all mandatory):**
- `cargo test --lib` green (3154+new).
- Full `ll_determinism_check.sh` ALL DETERMINISTIC.
- Print corpus byte-parity (stdout bytes identical pre/post for every
  MATCH bench — the harness's correctness gate covers this).
- Praetor: no NEW diagnostics in changed files.
- `bash benchmarks/toolchain_matrix.sh` (backend-touching, per-release
  discipline) if a release is cut from this.

## 4. Risk register (what could break elsewhere, how we'd know, how to undo)

| # | risk | detection | response |
|---|---|---|---|
| R1 | Program reads an imported global-let (`stdout_fill`) — identifier must resolve identically | behavioral tests + any suite MISMATCH | resolution is name-identical; if broken, S2 resolution is wrong — fix S2 |
| R2 | needs_state narrowing too aggressive: a defn touching global AND program fields drops `%state` | unit test (iii) in S2 + export/shared-lib tests | fixpoint root-set condition wrong — fix, never widen blindly |
| R3 | Contract over a global-let silently allowed | S1 diagnostic test | tighten typechecker |
| R4 | Main-tail flush gate (`has_stdout_flush`) stops firing (it may key on library state presence) | exit-flush tests; `printf`-loop benches flush at exit | re-key the gate on live_defns of the print chain (unchanged), not on state fields |
| R5 | GPU/SPIR-V normalizer: globals in kernels | SPIR-V corpus tests (print family already rejected there; `rand_state` GPU use would be new) | reject with what/why/fix diagnostic; globals are CPU-runtime plumbing for now |
| R6 | wasm/web backend globals lowering | webstack tests | same treatment as `@__briev_argc` (already emitted there) |
| R7 | BEAST serialization of the new modifier | beast round-trip tests | modifier rides existing annotation serialization |
| R8 | Init order: globals vs `init_state` | stdout tests (buffer alloc is lazy in `stdout_byte`; init is plain zero) | zero-init globals at module scope; no ordering hazard |
| R9 | Name collision global vs state field (the 10-12 clobber bug class) | typechecker duplicate-registration diagnostic | global namespace is separate from field namespace; diagnostic on collision |
| R10 | Performance surprises on print-free benches | §3 falsifiers | investigate before touching anything else |

## 5. Results (filled after implementation — do not edit predictions above)

To be recorded: per-bench table (actual vs prediction, band verdict),
gate outcomes, deviations and their explanations, A/B cross-check vs
`8962a2a1` for the big movers, and the final BUGS.md close-out.
