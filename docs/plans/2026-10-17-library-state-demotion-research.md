# The library-state demotion regression — research report (2026-10-17)

**Status: root cause proven, counterfactual verified, no fix landed yet.**
This document is the complete understanding required before any patch
(Rule 20: experiment before building; the user's directive: no quick
fixes until fully understood).

## 1. The discovery path

The post-fix benchmark sweep (see `2026-10-17-dispatch-regression-report.md`)
left a set of C-wins. Suite-vs-suite ratio comparison suggested
regressions, but C reference times themselves drifted +6–43% across eras
at the same clang (23.1.1) — machine drift contaminates any
across-days comparison. Per Rule 12b, the only admissible measurements
are same-session interleaved A/Bs against reconstructed eras of the
compiler.

Three eras were reconstructed in the baseline worktree
(`../briev-compiler-baseline`; binaries parked in
`.parked-untracked-aug/`, never deleted):

| era | commit | date | known ratio (suite) |
|---|---|---|---|
| fast | `8962a2a1` | 08-01 | interval_step 0.97x, float_math_nonzero 0.95x, knucleotide 0.99x, kalman 0.85x |
| buffering landed | `01c92189` | 09-12 | (the commit under investigation) |
| pre-dispatch-fix | `13ee67fa` | 09-30 | interval_step 1.40x, float_math_nonzero 1.21x, knucleotide 1.26x |
| current | `463e6124` | 10-17 | interval_step 1.23x, float_math_nonzero 1.24x, knucleotide 1.22x |

Interleaved 5×5 A/Bs (BOUND=500M, fresh builds of both sides — see the
stale-binary fix `463e6124`; note the baseline compiler binary is named
`briefc` before the rename, `compare_baseline.sh` needs the symlink or a
name-aware build):

| bench | current vs 8962a2a1 | current vs 13ee67fa |
|---|---|---|
| interval_step | **1.46x** | 1.00x |
| float_math_nonzero | **1.30x** | 0.99x |
| knucleotide | **1.28x** | 1.00x |
| mandelbrot | **1.14x** | 1.03x |
| kalman_filter_runtime | 1.09x | 1.04x |
| fannkuch_redux | 1.01x | 1.02x |
| queue_drain | (08-01 source doesn't typecheck; fast era is 07-31 `d5ac5509`) | 0.98x |

Reading: **the regressions happened between 08-01 and 09-30**, not in
the recent dispatch window. `13ee67fa` already carries the full loss;
that is why the earlier A/B round saw parity.

## 2. Root cause (proven at three levels)

**Commit: `01c92189` (2026-09-12, "feat(runtime): buffered stdout lane —
the fasta fix").** It fixed a real catastrophe — after libc removal,
per-byte stdout was one syscall per byte and fasta ran 81x slower than C
— by buffering stdout in a 64 KiB block. The buffer lives in **library
state**: `lib/std/cast_lanes.bv` declares

    let stdout_buf: Int = 0;
    let stdout_fill: Int = 0;

Top-level `let`s are program state: they become fields of `%State` and
every defn that touches them takes the hidden `ptr %state` parameter
(defn-needs-state fixpoint). The entire print chain —
`print_int/print_float/put_char → stdout_byte → write_all → stdout_flush`
— now carries `%state`.

**Level 1 — IR.** At `8962a2a1` the chain is `call i64 @__print_int(i64)`
(no state). Today every link is `call i64 @print_int(ptr %state, …)`.
Engine dispatch is UNCHANGED for the affected benches (interval_step:
SSA loop both eras; float_math_nonzero: countdown both eras) — the
engines are innocent; only the signatures changed. State-call census:
base 2 `%state` calls per module (init-time `get_env_int`), current 13.

**Level 2 — asm.** The base binary's hot loop (interval_step) is six
instructions, everything in registers (`count=%r14, acc=%r15, N=%rbx`),
print = cold call. The current binary's loop is ~11 instructions with
**acc living in a stack slot** (`mov 0x10(%rsp)` / `mov %rcx,0x10(%rsp)`
per iteration). Mechanism: passing the `%state` alloca's address to a
call — even a cold, guarded one — makes the alloca escape. SROA runs
before the regular inliner, refuses an escaping alloca, and the whole
`%State` struct stays memory-resident; per-loop promotion is also
refused because the call is a potential in-loop clobber. Every
loop-carried field of every program that prints inside a loop pays two
memory round-trips per iteration. (The base era's `get_env_int(ptr
%state,…)` escape is in main's entry, outside any loop — harmless.)

**Level 3 — counterfactual experiment (Rule 20).** Hand-transformed the
current `interval_step.ll`: buffer promoted to true globals
(`@stdout_buf`/`@stdout_fill`), the five print-chain functions drop the
`%state` parameter, all call sites updated. Linked with the harness's
exact command; **output byte-identical** to the current binary.

| variant | 500M iterations |
|---|---|
| current compiler | 848 ms |
| fast era (`8962a2a1`) | 594 ms |
| **globals experiment** | **414 ms** |
| C reference (`-O3 -march=native -ffast-math`) | 589 ms |

The experiment does not merely recover 2026-07 speeds — it **beats the
fast era by 43% and C by 30%** (0.70x), because full register promotion
now composes with the buffered-stdout win that `01c92189` was aiming
for. The user's thesis — the compiler already has the information to
emit faster-than-C code — is confirmed concretely: the fast code was
always derivable; the alloca escape was hiding it.

## 3. Blast radius (same mechanism, one census)

Any program whose loop body contains a print (even guarded) pays the
demotion. Confirmed from suite history + A/B: interval_step, 
float_math_nonzero, knucleotide, mandelbrot (prints per row), kalman
(mild), queue_drain. Benches with print-free hot loops are unaffected:
fannkuch (1.01x — its prints sit outside the hot loops), hash_ops,
nbody, bit_clear, linked_list — all at or better than their historical
ratios. fasta — the bench the buffering was built for — still wins
(0.58x) and is expected to improve further.

The same trap exists for the only other mutable library state:
`lib/std/os/rand.bv`'s `rand_state` (mutated by `rand_u64`/`rand_range`,
contract-invisible). Any future RNG-in-a-loop program would demote the
same way. And library state has already produced one correctness bug:
the `stdout_fill` shared-slot clobber (2026-10-12, noted in
`lib/std/os/fs.bv`). The pattern "mutable library plumbing as top-level
lets" is a standing hazard.

## 4. Edge cases considered (fix design constraints)

1. **Observability.** A program may `import` `cast_lanes.bv` and read
   `stdout_fill` directly today (it is an ordinary imported state
   field). Moving the storage class must preserve name, type, init,
   and mutation semantics — only the backing storage changes.
   Single-threaded observable behavior is identical; multi-threaded is
   identical too (today `%State` is one shared instance, so state
   fields are already cross-worker shared).
2. **Contracts.** `stdout_buf`/`stdout_fill`/`rand_state` appear in NO
   contract — only `term`/parameter conditions (verified). The buffer
   is invisible to the contract system. The fix must keep it that way:
   contracts over global-storage lets should be a diagnostic (they are
   plumbing, not proof obligations over program state).
3. **ABI.** `%State` layout is pinned for exports/shared-lib
   (opaque `%state` ptr). Removing the buffer fields shrinks `%State`;
   hosts never construct it, exported signatures are unchanged. The
   `dereferenceable(N)` annotations are compiler-internal.
4. **Embedded/ISR.** The flush-at-main-tail gate (`has_stdout_flush`)
   and `__exit` flush are CPU-runtime specifics; globals work in
   static-storage targets the same way. No ISR prints today; nothing
   in the design prevents them (uniprocessor).
5. **Interpreter parity (Rule 5).** The interpreter executes the
   buffer as state fields today. Global-storage lets need an
   interpreter representation with identical semantics; behavioral
   byte-parity corpus (print formats) must stay green.
6. **needs_state fixpoint.** Defns touching only global-storage lets
   must NOT take `%state` — this is precisely the fixpoint's
   "defn takes state" computation narrowing its root set. Defns that
   touch BOTH (none today) keep `%state` and simply also touch
   globals.
7. **Not a strategy keyword.** Storage class is disclosed, ordinary
   syntax (Rule 3/21): the lets keep their names; nothing is hidden.
   No speed claim attaches to the keyword — the speed comes from the
   compiler's standard promotion behaving correctly.
8. **What NOT to do.**
   - `alwaysinline` on the print family (inlined before SROA would
     also fix promotion) — rejected: it papers over the storage-model
     defect, bloats every call site, and every future library-state
     defn re-trips the hazard.
   - Splitting `%State` into program/library allocas — rejected for
     now: large ABI/layout churn to solve what a storage class solves
     directly; revisit only if a defn legitimately needs both.
   - Reinforcing loop promotion in the backend engines (phi-carrying
     state in the countdown/SSA engines) — rejected: reimplements
     SROA against the accidental-complexity rule; the LLVM pipeline
     already promotes when the alloca doesn't escape.

## 5. The licensed fix (design summary, not yet implemented)

A storage-class modifier for lets — `global let name: T = init;`:

- Parser/AST: `global` let modifier (disclosed, ordinary keyword).
- Typechecker: global-lets cannot appear in contracts (diagnostic with
  the proof-style message); init must be a constant expression;
  duplicate vs state-field registration rules as today.
- Transition graph/liveness: global-lets are not state fields (not in
  `field_index_map`, not live_fields, no write_set).
- Backend: identifier resolution lowers to `@<name>` global
  load/store; the needs-state fixpoint excludes global-only defns from
  `%state`; globals emitted as `@name = global i64 0` (+ const-folded
  init when literal).
- Interpreter: a globals map; identical read/modify/write semantics.
- Stdlib: `stdout_buf`, `stdout_fill`, `rand_state` migrate.
- Tests: (a) needs-state test — defn over global-only state emits
  without `%state`; (b) the regression test — a loop with a guarded
  `println!` must not pass `%state` to the print chain (IR-level
  signature check, mirroring the experiment); (c) contract-references-
  global diagnostic; (d) byte-parity of the print corpus; (e)
  behavioral rand/stdout tests unchanged.
- Docs: SPEC storage class, keyword-taxonomy note, agent-reference,
  benchmark-strategy note; BUGS.md entry closing this research.
- Expected results (from the experiment): interval_step ≤0.75x vs C,
  float_math_nonzero/knucleotide/mandelbrot/queue_drain recovering
  their pre-09-12 ratios or better; fasta improving further; ALL
  gates (lib tests, determinism, Praetor, toolchain matrix) green.

## 6. Open items beyond this fix

- `nbody_newton_accel` 1.10x vs pre-buffer era, ~12x vs C — the
  accel-offload subject; unchanged by this research.
- fannkuch/kalman residual vs C (1.11x/1.08x suite, 1.01/1.09 vs
  pre-buffer era) — machine drift + sub-10% real components; revisit
  after the storage-class fix lands (kalman's prints sit in its loop).
- Library `let`s that are really constants (`SYS_*` in
  posix/process.bv) — cosmetic; no hot-loop impact.
