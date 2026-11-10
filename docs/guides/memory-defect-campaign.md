# Memory-defect campaign

Hunts **memory-safety defects in generated native code** — use-after-free,
double-free, invalid/overflowing access, uninitialized reads, and leaks. It is
the memory-safety counterpart to `docs/guides/deterministic-testing.md` (which
checks *values*); both are needed, because a memory bug can produce a harmless
answer and a wrong answer need not be a memory bug.

Detection is by sanitizers, not by comparing output, so a defect surfaces even
when the result happens to look right.

## Why not `cargo-fuzz`

`cargo-fuzz` runs the compiler in-process, so it cannot see bugs in *emitted*
code. This campaign compiles real `.ll`, links it under sanitizers, and runs it
— the pipeline that historically exposed the region-reclamation and object
boundary bugs (`BUGS.md`).

## Detection stack

- **Valgrind** memcheck — the **primary oracle**: catches use-after-free,
  invalid/overflowing reads and writes, and uninitialized use, with **no
  instrumentation of the emitted IR**. Runs on a plain `-O1` build, default-on.
- **ASan** (`-O0` and `-O1`) via `clang -fsanitize=address` — adds leak
  detection (LSan) and `free`-family errors (double-free, invalid free) through
  malloc/free interception.
- A **finding** = a Valgrind `ERROR SUMMARY`/invalid access, a sanitizer marker
  in stderr (`AddressSanitizer`, `LeakSanitizer`, `runtime error`, `SUMMARY:`),
  a fatal signal, or (for the generated families only) a nonzero exit.

### Why Valgrind is primary (2026-11-09 methodology finding)

`clang -fsanitize=address` on a **`.ll` input does not instrument memory
accesses** — it only links the runtime. Verified: a minimal `.ll` load compiled
with ASan had **zero** shadow checks, and the emitted Briev module likewise.
So an ASan-linked Briev binary catches only interceptor-based errors
(leaks, double/invalid free) and **misses use-after-free reads and overflows**.

`opt -passes=asan` is the IR instrumenter, but on the emitted Briev module it
registered globals and set a `nosanitize_address` module flag without adding
access checks. Until that is understood, **Valgrind is the access-check
oracle**. This is why the campaign reports the exact detector for every finding.

UBSan is not available as an `opt` pass (there is no `ubsan` pass), so it is
not part of the IR pipeline; only Valgrind + interceptor ASan run here.

## Run

```bash
# Bounded smoke (also part of `cargo test --all`)
cargo test --test memory_campaign existing_memory_fixtures_are_sanitizer_clean -- --nocapture
cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture

# Aggressive, per family, with Valgrind
MEM_FAMILY=0 MEM_CASES=16 MEM_SEED=7 MEM_VALGRIND=1 \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
```

Cache a real finding and shrink it:

```bash
MEM_FAMILY=0 MEM_SEED=7 MEM_CASES=16 \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
# artifacts land in target/memory-failure/ (case.json, memory.bv, memory.ll, failure.txt)
MEM_REPLAY=target/memory-failure/case.json \
  cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
```

## Knobs

| Var | Meaning | Default |
|---|---|---|
| `MEM_FAMILY` | 0–4 fixed shape (see below) | 0 |
| `MEM_CASES` | generated cases per run | 4 |
| `MEM_SEED` | deterministic campaign seed | 1 |
| `MEM_VALGRIND` | Valgrind oracle. Generated families: on by default, `0` disables. Corpus sweep: opt-in via `1`. | on (generated) |
| `MEM_ARTIFACTS` | failure output dir | `target/memory-failure` |
| `MEM_REPLAY` | replay a saved `case.json` | unset |

Families: `0` escape-from-callee, `1` publish-into-parameter, `2` list
return/alias/mutate, `3` defn image stored into node state, `4` per-iteration
allocation in a bounded loop, `5` enum payload allocation (Sum image
ownership), `6` allocation in a nested object field, `7` two allocations held
in a tuple then projected out. Families 5–7 were added 2026-11-10 to cover the
historically buggy aggregate-escape sinks (BUGS.md 2026-11-03).

## Corpus sweep

`existing_memory_fixtures_are_sanitizer_clean` runs the same stack over
ownership-heavy fixtures (`reclamation_aliases`, `reclamation_bounded`,
`structured_lifetimes`, `tuple_values`, `file_descriptor_lifetime`). Exit codes
are ignored there (fixtures use them as result channels); only markers and
signals count.

## Confirmed finding (2026-11-09)

The campaign independently found a **definite memory leak**:

- `MEM_FAMILY=1` (publish-into-parameter): **12/12 cases across 3 seeds** report
  a Valgrind definite leak. Minimal repro:
  `tests/fixtures/memory/publish_into_parameter_leak.bv` → *16 bytes in 1 block
  definitely lost*.
- Root cause: `src/analysis/reclamation/walk.rs:354` disables region reclamation
  for storage published into a callee's incoming parameter (`:786` calls the
  over-approximation "safe"; disabling reclamation *is* a leak). The same shape
  in `reclamation_aliases.bv` makes the standing guard
  `tests/reclamation_runtime.rs::native_aliases_borrowed_payloads_and_nested_returns_are_memory_safe`
  **red** under Valgrind (CI misses it because Default-gates does not install
  Valgrind). See `BUGS.md` (2026-11-09) and
  `docs/reports/2026-11-09-memory-defect-campaign.md`.

Family 1 is therefore a **standing finding**: excluded from the default family,
run in the nightly lane with `continue-on-error`.

## Triage rules

- **Compile failure is a generator bug**, not a finding — fix the template, do
  not weaken detection.
- **Leaks are not automatically defects.** A leak report must be checked against
  the ownership contract: `Malloc#` with no `Free#` and no reclamation may be
  intended. Record intended leaks explicitly; only contract violations are bugs.
- **A marker is a defect candidate**, not a verdict. Minimize with shrinking or
  `MEM_REPLAY`, then confirm the same report on the minimized program before
  filing in `BUGS.md`.
- **Do not weaken the sanitizer stack to make a case pass.**

## CI

`cargo test --all` (the default gate) runs the bounded smoke.
`.github/workflows/memory.yml` runs the long, sharded, non-blocking sweep nightly
and on demand, uploading artifacts per family.
