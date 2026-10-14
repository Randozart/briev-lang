# Daily-driver tier 2 — payload ABI unification, loop locals, test runner, time

**2026-10-14 · Status: ACTIVE.** Follow-on to
`2026-10-12-daily-driver-quick-wins.md` (all four increments landed). This
plan covers the next four items in the daily-driver queue, in dependency
order. Every phase: probes before fixes (Rule 20), behavioral tests linked
at `-O0` as the IR-validity gate (the LTO lesson, inverted), full gates per
commit.

## Phase 1 — Let-bound collections + Result payload ABI (one root cause)

**Symptoms (all one family — a value crosses a boundary and loses/gains a
box):**

1. `h.push(Dog{..})` on an empty-init dyn collection stores the RAW handle —
   the push argument is not boxed at the member-call boundary
   (`2026-10-11-coll-element-size.md` ADDENDUM 3, known hole, LOUD failure).
2. A dyn list as a defn CALL ARGUMENT constructs via the legacy path (no
   binding name at the call site) → same hole (same source).
3. A let-bound `List` construct-and-insert mis-lowers in node bodies
   (BUGS.md 2026-10-12; the fs_smoke roundtrip test works around it in a
   comment).
4. Result payload access is SHAPE-SENSITIVE: `Result<Int,Int>` takes
   predicate+unwrap, `Result<String,Int>` needs `match`, a List-payload
   Result consumed through a let binding mis-lowers (fs_smoke comment).

**Method:** reproduce each on a minimal probe; dump the IR at `-O0`;
identify the boundary where the boxed-ABI contract breaks (push argument
site, call-argument collection literal, let-bound payload consumption);
fix at the GENERAL boundary level (the push site / the arg site / the
payload read), never a type-name arm (Rules 15/23). The dyn-box pre-pass +
`construct_local_dyn_boxed` helpers (ADDENDUM 3 item 2) are the template:
one contract, every construction/consumption site.

**Fix acceptance:** each probe compiles at `-O0`, links, runs, prints the
bound value at a print boundary (exit 0); a compiled behavioral test per
symptom lands in `tests/`; the fs_smoke workarounds that can now be
expressed directly get rewritten to the direct form.

## Phase 2 — Reactive-loop local let (alloca dominance)

**Symptom:** loop-carried locals across multiple `when` blocks in a node —
`let i: Int = 0; when c1 { i = 1; }; when i < n { i = i + 1; }` — emit a
load/store in a block that does not dominate its use (clang: `Instruction
does not dominate all uses!`; BUGS.md 9303).

**Method:** probe first; inspect the alloca placement for a local written
in a nested `when` block. Standard fix direction: locals assigned anywhere
in the fn get their alloca at ENTRY (the alloca-entry pattern), or the
block's value state becomes phi-based — choose by reading the actual
emission, not by design taste.

**Fix acceptance:** a behavioral test with a real 3-iteration loop in a
node prints the carried value correctly; `examples/cli-args.bv`'s dropped
loop shape re-added and linked.

## Phase 3 — `brievc test` runner

**Gap:** running Briev-level tests today = a Rust integration test that
shells `brievc build` per fixture (12+ copies of the same harness in
`tests/`).

**Design (v1, thin):**
- `brievc test <file.bv | dir>` — each `*.bv` under the path is one test:
  compile (llvm, in-process `compile_source`), link with `lib/runtime/`
  (the harness's exact clang line), run.
- **PASS = exit 0** and, when a sibling `<name>.golden` file exists, stdout
  byte-equal to it (the daily_use_smoke golden convention, generalized).
- Discovery skips `*.ll`/dot-files; output: one line per test
  (`PASS/FAIL name`), non-zero exit on any failure; `--out <dir>` for
  scratch (default `target/briev_test/`).
- No new language surface — no `@test` annotations in v1 (a test IS a
  program whose success is its exit code; contracts already assert).

**Fix acceptance:** `tests/lsp_smoke.rs`-style integration test drives the
subcommand over a directory with one passing + one failing + one golden
fixture; the existing `tests/tier1/*.bv` corpus runs green under it.

## Phase 4 — time.bv clock/sleep (pure stdlib over the variadic syscall)

**Gap:** `time.bv` is a stub; every CLI wants clock + sleep.

**Design:** NO new intrinsics — Rule 14 in action, and the first real
consumer of the syscall retirement: `SYS_CLOCK_GETTIME` (228) and
`SYS_NANOSLEEP` (35) through the variadic `syscall`, with a 16-byte
timespec built via `Alloc#`/`Store#`/`Load#`:
- `defn now_ns() -> Int` — CLOCK_MONOTONIC (1); contract `[true][term >= 0]`
  (monotonic clocks never go back).
- `defn sleep_ms(ms: Int) -> Int` — nanosleep loop; returns residual ms
  (0 when complete); contract `[ms >= 0][true]`.

**Fix acceptance:** behavioral test asserts `now_ns()` non-decreasing
across calls and that `sleep_ms(30)` + timestamps measures ≥ 25 ms; the
timespec memory is freed (`Free#`) — no leak per iteration.

## Gates (every phase, every commit)

- `cargo test --lib` green (new tests included); new behavioral tests
  linked at `-O0`.
- `bash benchmarks/rbv_gate.sh` PASS.
- Conformance sweep + fs_smoke + daily_use + cli_args green.
- Praetor: no NEW diagnostics in changed files (by-status sweep, never a
  file list — three misses today taught that).
- BUGS.md: each fixed symptom gets a dated FIXED entry with repro +
  fix; open ones stay referenced.

## Ordering and commit plan

Phase 1 → 2 → 3 → 4, one commit each (probe evidence in the plan doc's
session log as work proceeds). Phase 3 after 1–2 because the runner makes
their regression tests writable in Briev; it is built in Rust regardless —
the tests it RUNS are Briev.

## How to undo

Each phase is one commit; revert restores the prior behavior. Phase 4
deletes back to a stub; Phase 3 removes a subcommand; Phases 1–2 fix
codegen at boundaries — revert returns to the miscompiles, which are
documented in BUGS.md.
