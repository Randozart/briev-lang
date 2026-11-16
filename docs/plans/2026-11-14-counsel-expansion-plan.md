# Counsel expansion plan — what's next after PR #98/#99/#100

Status: PLAN (research-first). No non-markdown files touched.

## Where we are

- PR #98 merged: `counsel = 0.5.0 (macros)` hard dep; `total` (stats.rs) + `is_null`
  (ffi/error.rs) proven.
- PR #99 merged: 5 seed contracts + `scripts/counsel-check.sh` + CI over `src/`
  (non-blocking).
- PR #100 open: 4 more advisory contracts.
- Current census (counsel 0.5.0, fresh install at ~/.cargo/bin/counsel-check):
  - PROVEN: `contains`, `is_null`, `total`  (≈3 — the real ceiling today)
  - unproven (advisory): `node_count`, `push_width`, `is_empty`, `len`,
    `pointer_bytes`
  - not checked: `size` (as-cast), `count_statements_recursive` (while-let),
    `canonical_rid` (HashMap::get opaque), most everything else
  - Zero Violated throughout.

## What counsel 0.5.0 actually accepts (read from counsel-crate source)

The admission gate (ARCHITECTURE.md + build.rs/admission.rs) is deliberate and
conservative. Confirmed by running the 0.5.0 binary, not by guessing:

| Construct | Verdict | Evidence |
|-----------|---------|----------|
| `i64`/`u64` `+ - * /` on **known i64** operands | ✅ | symbolic.rs fold_binary |
| Comparisons on known i64 | ✅ | bounds flow, Z3 QF_BV |
| `if/else`, `let`, `return`, trailing expr | ✅ | walk.rs |
| `for lo..hi` / `lo..=hi` (literal bounds) + `#[invariant]` | ✅ | LoopShape in build.rs |
| Struct **field** access (flattened symbols) | ✅ | L1 commit; `total`, `is_null` proven via fields |
| **Collection *parameter*** `.len()` | ✅ as length fact | P2 commit; precondition injects `len_param >= 0` |
| Collection push/pop/clear/extend/is_empty | ✅ | M1/M2 |
| Integer `as` cast | ❌ refused | "casts ... are refused" |
| `match` expression | ❌ refused | admission; seen on arith_op_count helpers |
| `while let` / arbitrary while | ❌ refused | "a while loop in value position" |
| Method call not modelled (e.g. `HashMap::get`) | ❌ opaque | "unmodelled method call is opaque" |
| Struct-field `.len()` (not a fn parameter) | ❌ not a length fact | P2 only targets params → `node_count` unproven |
| Unsigned ordering/arithmetic beyond range facts | ❌ refused | numeric semantics section |

So the prover is a narrow but *sound* bit-vector interval + Z3 engine. It is
not a general Rust verifier. The provable surface in briev-lang is small but
real.

## Why the seed is already the "right" layer, not dead weight

- `#[pre]/#[post]` are inert identity attrs (macros expand to nothing). Zero
  runtime/compile behaviour change; `cargo check` is the consumer gate.
- The seed encodes honest specs that counsel *cannot yet discharge*. That is
  the intended shape: proven where it can be, honest-elsewhere, never weakened,
  never a false proof. Violated is the only thing that must stay at zero, and it
  does.
- CI runs `counsel-check` over all of `src/` (non-blocking) with an artifact, so
  every future counsel release re-audits the whole tree for free.

## The two directions worth pursuing (pick one, or sequence them)

### A. Expand *provably* (no refactors, pure upside) — recommended first

Goal: grow the PROVEN count from 3 without touching code shape.

1. **Find the in-subset functions.** Grep for `pub fn` returning a scalar whose
   body is only field reads / scalar arithmetic / `if/else` / `for lo..hi`.
   Candidates to eyeball: interval/stats accessors, `is_null`-style enum
   discriminant checks, pure arithmetic helpers. Add `#[pre]`/`#[post]` and let
   counsel-check confirm proven.
2. **Use the P2 length-fact trick where it applies.** If a function takes a
   collection *parameter* and returns something tied to its length, counsel
   already knows `len_param >= 0`. That can discharge `#[post(term >= 0)]` or a
   `term <= param.len()`-style bound. (This is why `node_count` on a struct
   field is unproven, but a `fn f(v: &Vec<T>) -> usize { v.len() }` style fn
   would prove.)
3. **Re-run the census and record the delta** in the scratchpad + a one-line
   census comment in the PR.

Effort: low. Risk: none (inert attrs, cargo check gates).

### B. Prove *Briev* bodies via the dormant `--verify-bodies` (the bigger prize)

This is a different mechanism — it does **not** use Rust `#[pre]/#[post]`. It
proves that each Briev `defn`/`txn` **body** satisfies its **own** `[pre][post]`
contracts, using the `briev-verify` crate (the "dormant symbolic.rs" path).

Current state (read from src/):
- `brievc build <file.bv> --verify-bodies` already exists (main.rs:474-502).
- `src/analysis/verify_bodies.rs` is a thin `pub use briev_verify::verify::*`
  with tests that parse real Briev source and assert proven/unproven/violated.
- The prover lives in `briev-verify` (crate) so it runs without the compiler.
- Known red baseline: default `cargo test` is 4116 passed / 76 failed (pre-existing,
  not caused by the counsel layer).

Plan (staged, each stage independently valuable):
1. **Baseline run** on a small corpus of `.br`/`.rbv` specs; record
   proven/unproven/violated counts. Establish the current coverage number.
2. **Close the biggest unproven bucket.** Triage unproven defns by pattern
   (loop bounds? missing pre? unsupported op?) and add the cheapest pre/post
   that makes the body discharge — *strengthen specs, never weaken*.
3. **Wire a non-blocking CI step** that runs `--verify-bodies` over the spec
   corpus and archives the JSON report (mirrors the counsel CI step).
4. Only after that is stable, consider making it blocking.

This is the path that turns Briev's own contract language from "checked by
interpreter" into "proved at build time," and it's the direction MEMORY already
flagged as the planned improvement.

### C. (Later, bigger) Counsel-side capability growth

Only if A/B hit a wall and we specifically want `match`/cast/while support:
that is a counsel-crate change (new admission + lowering in build.rs, new
prover rules in symbolic/walk). It's real work and has its own test/release
bar (counsel publishes macros then counsel; acceptance suite must pass).
Not recommended as the next step — A and B give more value per unit of effort
and keep briev-lang the owner of its specs.

## Recommended sequence

1. **A now** (≈1 session): expand proven count, record census. Low risk.
2. **B next** (multi-session): baseline `--verify-bodies`, close biggest
   unproven bucket, non-blocking CI. This is the strategic win.
3. C only if a concrete blocker forces it.

## Explicit non-goals

- Do **not** weaken any contract to force a proof (contract-first).
- Do **not** make either CI step blocking until the corpus is clean.
- Do **not** claim the existing Rust test suite is green (it is not; 76
  pre-existing failures).

## Files this plan would touch (when executed in BUILD mode)

- A: `src/**/*.rs` (add `#[pre]/#[post]` only), plus scratchpad/PR note.
- B: spec corpus (`*.br`/`*.rbv`), `src/analysis/verify_bodies.rs` (report wiring),
  `.github/workflows/ci.yml` (new non-blocking step), `docs/plans/*` for records.
- C: `counsel-crate` (separate repo, separate release) — out of scope now.
