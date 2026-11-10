# Exhaustive defect-discovery campaign (2026-11-10)

Status: ACTIVE. Branch `test/semantic-differential-discovery`, base
`0f66fe48` (`origin/main`).

## Goal

Run a large, measured bug hunt across independent parts of the compiler and
eliminate what it finds. "Exhaustive" means every harness runs to a recorded
input count with coverage reported; it does **not** mean all Briev programs are
enumerated (that is undecidable). The deliverable is findings, fixes, and
regression tests — plus an honest statement of what remains untested.

## Surfaces (independent oracles)

| Surface | Harness | Oracle | Cost |
|---|---|---|---|
| Lexer/parser | `cargo-fuzz` target `frontend` | no panic/hang | very fast, in-process |
| Interpreter semantics | `cargo-fuzz` target `arithmetic` | independent Rust model | fast, in-process |
| Interpreter tuples | `cargo-fuzz` target `interpreter` | same model, tuple ops | fast, in-process |
| Native codegen | `tests/deterministic_transitions.rs` family 13 | model + interpreter, `-O0`/`-O3` | slow (real clang link) |
| Memory safety | `tests/memory_campaign.rs` | Valgrind + ASan | slow |

The in-process targets carry the "millions" load; the native and memory
harnesses are the expensive, higher-signal confirmation layer.

## Method (per `docs/handoff-methodology.md`)

1. Baseline: record existing artifacts/corpus; note known open defects so they
   are not re-reported as new.
2. Run each surface to a fixed budget; capture `-print_final_stats=1`
   (executions, coverage, corpus size).
3. Triage every artifact: replay, minimize (`fuzz tmin`), confirm on the
   minimized input, and check `BUGS.md` for rediscovery.
4. Fix the defect at its root (no patch, per AGENTS.md); add a regression test
   (in-process case for a semantic bug, native fixture for a codegen bug).
5. Rerun the surface to prove the finding is gone and nothing new appeared.
6. Document counts and gaps in this plan; log defects in `BUGS.md`.

## Known open defects (excluded from "new")

- Interpreter tuple projection and tuple destructuring (BUGS.md 2026-11-09);
  reproduced by `fuzz/artifacts/interpreter/crash-*` and the two `#[ignore]`
  reproducers in `tests/deterministic_transitions.rs`.
- `publish-into-parameter` reclamation leak (fixed by #81; family 1 is a
  non-blocking sentinel).

## Runbook

```sh
# in-process, high volume (the "millions")
HOST=$(rustc -vV | sed -n 's/^host: //p')
cargo +nightly fuzz build
cargo +nightly fuzz run --target "$HOST" frontend    fuzz/corpus/frontend    -- -max_total_time=300 -fork=8 -print_final_stats=1
cargo +nightly fuzz run --target "$HOST" arithmetic  fuzz/corpus/arithmetic  -- -max_total_time=300 -fork=8 -print_final_stats=1

# native differential (real codegen), composed integer family
BRIEV_SIM_FAMILY=13 BRIEV_SIM_CASES=64 BRIEV_SIM_SEED=1 cargo test --test deterministic_transitions scripted_integer_transitions -- --nocapture

# memory safety (Valgrind primary)
MEM_FAMILY=<0..4> MEM_CASES=16 MEM_SEED=7 MEM_VALGRIND=1 cargo test --test memory_campaign generated_memory_programs_are_memory_safe -- --nocapture
```

`scripts/defect_campaign.sh` wraps the above with logging.

## Results

**2026-11-10, first campaign (base `0f66fe48`).**

| Surface | Executions | Findings |
|---|---|---|
| frontend (in-process, 240 s ×8) | ~1.46 M | 5 `slow-unit` inputs → **parser DoS** (below) |
| arithmetic (in-process, 240 s ×8) | ~210 K | none (coverage plateaued at 1532) |
| native differential family 13 (48 cases) | — | none |
| memory families 0/2/3/4 (Valgrind) | — | none |

**Defects found and fixed:**

1. **Parser DoS — exponential backtracking.** `parse_block_expr` /
   `parse_tail_body` re-parse the same span on failure; malformed nested input
   is exponential in depth (88-byte input ≈ 20 s). Delta-reduced to 42 bytes
   (`fuzz/cases/pathological-nesting.input`).
2. **Parser DoS — stack overflow.** ~1000 nested `{` (or 300 nested `(`)
   aborted the compiler with `fatal runtime error: stack overflow`.
   Fix for both: `steps_remaining` (linear work budget) + `MAX_PARSE_DEPTH`
   (= 24), returning `SyntaxError::BacktrackLimit`.
3. **Interpreter tuple projection** (`pair.1` on a tuple) and **tuple
   destructuring** (`let (a, b) = pair`) — the two `#[ignore]` reproducers;
   fixed in `src/interpreter/eval.rs`, un-ignored, now green.

**Gates:** `cargo test --lib` 3573 pass; `cargo test --test fuzz_frontend` 14
pass; `cargo test --test deterministic_transitions` 9 pass; conformance sweep
passes within the depth cap.

## Gaps / not covered

- Multi-node reactor scheduling, `async`/`sync<group>` classification, and
  concurrency interleavings.
- Float/NaN semantics; strings; enums beyond tuple projection.
- Backends other than native LLVM (WASM, SPIR-V, CIRCT, VM).
- Compiler-pass configuration variation (only LLVM `-O0`/`-O2`/`-O3` is varied).
