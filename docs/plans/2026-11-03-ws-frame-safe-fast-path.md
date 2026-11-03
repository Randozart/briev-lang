# ws_frame safe fast path — proofs, evidence, options — 2026-11-03

**Status: FINDINGS RECORDED.** Execution plan (Steps 0–3, ACTIVE):
`2026-11-03-ws-frame-fast-path-execution.md`. This file captures the
investigation into recovering `ws_frame` baseline speed without
violating compiler rules. Measured results are appended here as they land.

## Context

- Pinned baseline `compiler-baseline-2026-10-23-tail-loop` @ `42279aaf`
  runs `ws_frame` at ~0.0100s. Current tree (`378886c8`) runs ~0.0157–0.0200s
  (A/B ratio 1.83–2.14x; 1.14–1.33x C depending on run).
- The baseline was fast because it returned a pointer into a **dead callee
  frame** (no `retain_enum_return` existed yet). That is UB; it matched output
  only by coincidence. Rule: never re-add it (`docs/plans/2026-10-29-enum-by-value-abi.md`).
- The safe replacement is `retain_enum_return`
  (`src/backend/llvm/helpers.rs:2732`): at every enum-return site, copy the
  image into the caller-owned `%enum_out` unless the source handle already
  equals the destination (runtime `icmp eq` → `ret_same` / `ret_copy`).
- The open question (owner, 2026-11-03): **what proof would the compiler need
  to legally emit the fast shape?** The compiler is allowed to optimize —
  the constraint is *how* it proves safety, not whether.

## Evidence (2026-11-03)

IR artifacts: `/tmp/opencode/ws_frame_base/ws_frame.ll` (from baseline
worktree `../briev-compiler-baseline`), `/tmp/opencode/ws_frame_cur/ws_frame.ll`
(from current release build).

| metric | baseline 42279aaf | current 378886c8 |
|---|---:|---:|
| total `malloc` sites | 44 | 64 |
| `ret_copy` labels | 0 | 32 (16 branch sites, 7 functions) |
| mallocs in decode fns (`ws_decode`+`ws_decode_mask`) | 5+1 | 5+1 (identical) |

- The 20 extra mallocs are in **float formatting** (`exp_digits`,
  `fixed_body`, `exp_style`, …) — cold report path, not the hot loop.
- `ws_partial` and `ws_decode_mask` malloc `malloc(34)` in **both** eras —
  not a regression source.
- The hot-path delta is exclusively the `retain_enum_return` machinery:
  16 branch sites across `ws_decode` (4), `ws_decode_mask` (2),
  `ws_ext_len16` (2), `ws_ext_len64` (2), `ws_mask_key_at` (2),
  `ws_min_len16` (2), `ws_min_len64` (2).
- Executed copies: the 16/64-bit length paths (2/3 of corpus frames, `f%3`)
  birth intermediate images in fresh `esrt` allocas inside `ws_decode`, so the
  final retain copies `esrt → %enum_out` every frame. The 7-bit path
  (`term ws_decode_mask(ptr %enum_out, …)`) already threads the slot and hits
  `ret_same`.

### Experiment (Performance Recovery Protocol step 5) — REFUTED

Hand-transformed the actual current `.ll`: forced all 16 retain branches to
`ret_same` (skip every copy = baseline's shape), linked with the harness's
exact `-O3 -flto -march=native` command:

- **Output `01092016384`, expected `0016384` — WRONG.**
- Verdict: the copies are load-bearing where source ≠ destination (image born
  in a local `esrt` alloca). "Just skip the copy" is the same unsafe pattern
  already rejected. Hypothesis refuted ⇒ that fix is blocked; the fast path
  must be rebuilt as *destination threading* (proof below), not as copy
  deletion.

### Attribution caveat

Static IR math (~11K executed 2-word copies + branch overhead) may not fully
explain a 5.7 ms gap; harness run variance on the identical tree was observed
(0.0157s vs 0.0200s). Interleaved A/B ×5 + instruction/perf counts are
required before any fix is sized. Do not treat the current ratio as fully
attributed to the retain copies.

### Attribution RESULT (2026-11-03) — the regression is a phantom

Full data: `benchmarks/results/2026-11-03-ws-frame-attribution.md`.

**The locked baseline is numerically wrong; the current tree is correct and
at C parity.** Measured with `perf_event_open` + interleaved ×8 timing,
`F_TOTAL=4096, PASSES=4`:

| impl | correct? | instructions | wall |
|---|---|---:|---:|
| C reference | yes | 78.65 M | 20.4 ms |
| current | **yes** | 85.22 M | 23.0 ms |
| locked baseline `42279aaf` | **no** | 23.60 M | 12.6 ms |

Correctness proof: `PASSES` is compile-time; at the benchmark's even
`PASSES=4`, `acc ^= sum` is 0 for every implementation, so the benchmark
cannot see a wrong checksum. Rebuilding at `PASSES=1` shows the C reference
and the current compiler both print `acc=103005804` (exact match), while the
baseline compiler prints `acc=7441`. The baseline also segfaults on a
payload-heavy variant and under any `LD_PRELOAD` — the recorded dead-frame
UB. Its decoded `payload_len` is garbage, so it checksums a truncated range:
fast **because** it is wrong.

Phase isolation (`PASSES=1` vs `=4`, solving for per-frame cost): build is at
parity (~4,878 vs ~5,013 instr/frame, identical SIMD fill); decode is
~221 (baseline, bogus) vs ~3,947 (current) instr/frame, and the current
decode cost scales with payload bytes (~1.34 instr/byte) exactly as the C
reference does. A `SIGALRM`/`ucontext` sampler puts ~48% of current samples in
the two `ws_payload_byte` checksum loops; the baseline sampler output is
truncated by the crash.

Consequences for the plan:

- **The baseline gap cannot be recovered by any legal proof** — it is the cost
  of the checksum work the baseline skipped via UB. Step 2's target is void.
- The only legitimate headroom is the **current-vs-C** gap: ~6.6 M
  instructions / ~2.6 ms (~8%). The retain copies are a *candidate* for it
  but are not shown to be the whole residual; measure the retain blocks alone
  before sizing Proof 1.
- **Proof 1 ceiling ≈ 2.6 ms** (to C parity), not ~10 ms.
- **Proof 2/3 (I11, boundary gate) are real correctness work** but are **not
  required for `ws_frame`** — correct-code parity with C already holds.

Benchmark bug recorded (BUGS.md, 2026-11-03): even `PASSES` makes the XOR
accumulator identically 0, hiding checksum errors; a correctness gate needs
odd `PASSES` or a non-XOR accumulator.

## The three proofs

All are ownership/alias facts derived from existing analysis — **no type-name
or algorithm-vocabulary matching** (Rules 15/23).

### Proof 1 — returned-image ownership (removes `ret_copy` blocks)

Today: unknown ownership ⇒ copy (emit_stmt.rs:2041). To omit the copy,
prove **source == destination by construction**:

- **(a) Birth-site threading.** For `let r = f(...)` whose uses are only
  local match/tag/payload — already identified by `find_sret_call_lets`
  (emit_toplevel.rs:3655) — `f` may receive the enclosing function's
  `%enum_out` instead of a fresh `esrt` alloca, provided:
  - the slot is free at that point (`enum_out_busy` already models this for
    constructors, emit_expr.rs:6245);
  - image-slot liveness holds: the intermediate image is dead before the
    next write to the slot; never two live images in one slot.
- **(b) Return-path dataflow.** Every `term` in the defn yields an image that
  *is* `%enum_out` (or a heap box whose ownership transfers).

(a) ∧ (b) ⇒ source == destination statically ⇒ emission skips
`retain_enum_return` entirely — no branch, no copy. Fallback when slots
overlap: current behavior (fresh alloca + copy) stays; correctness floor
unchanged.

**Never provable:** baseline's pointer into a dead callee frame. UB — not an
optimization. The fast path is rebuilt as threading, never restored as-is.

### Proof 2 — object consumption (removes `ws_partial`'s `malloc(34)`)

Already implemented (`src/analysis/object_repr.rs`): every return is an object
literal ∧ every call consumed by field-projection / non-retaining parameter /
inline-payload enum constructor ⇒ callee may write into a caller-owned hidden
slot. Missing link: `object_payload_is_inline("Ok", "WsFrame")` = false
because `Result<WsFrame,String>` is a boundary enum
(`enum_layout.rs:107` `allow_object_inline = !boundary_enums.contains(name)`,
gate at `:333`, 2026-11-02 whole-enum escape gate).

**Scope note: not needed for `ws_frame` parity** — baseline malloc'd in
`ws_partial` too. Relevant only for further wins beyond baseline.

### Proof 3 — boundary-gate relaxation (prereq for Proof 2)

**Condition 2 escape confirmed 2026-11-03** (details: execution plan Step 0
RESULT, BUGS.md "Enum image escapes to state without owning"): the state-store
paths (`emit_stmt.rs` `Assign` → `emit_state_store_i64` and variants) publish
an enum handle without `own_enum_value`, so a defn-local sret image stored to
global/node state dangles after the defn returns. Minimal repro
`defn_store.bv` prints garbage under relaxation and `42` after hand-inserting
`malloc+memcpy` (owning) before the store. PR51 match-binding materialization
covers only the match class; **non-match escapes (state, call args, returns)
were never materialized.** Not layout divergence — minimal shapes are
layout-consistent and the validators enforce it.

Recorded fact (`docs/plans/2026-11-01-canonical-enum-payload-and-selective-inline.md`
Phase E): relaxing the gate while keeping PR51 owned extraction **still
corrupts I11** (`malloc(): invalid next size` — a size-mismatch write). The
supporting proof is incomplete; its missing piece must be diagnosed first.
Candidate shape: single resolved layout consumed by *every* image
materialization site (constructor, match, `retain_enum_return` word count,
`own_enum_value`, sret alloca sizing, FFI export) + no inline-payload view
outlives its image slot. Step 0 of any gate work = reproduce and root-cause
the I11 corruption.

## Options (for discussion)

**Decision 2026-11-03 (owner):** all steps — attribution first, then
Proof 1, then Proof 0/3/2 workstream; investigate any bugs found en route.
Execution: `2026-11-03-ws-frame-fast-path-execution.md`. Options below
retained as the record of what was on the table.

1. **Attribution only (no code).** Run interleaved A/B ×5 + perf/instruction
   counts; if the retain copies are a small fraction, the baseline gap is
   mostly noise/other drift and the exception stands as recorded in
   `2026-11-02-language-stabilization.md`.
2. **Proof 1 implementation.** Image-slot liveness + birth-site threading +
   static retain elision. Medium-sized backend change (emit_stmt /
   emit_toplevel / helpers); additive (fallback path untouched); needs the
   full gate set (runtime A/B, determinism, ASan, I11, Praetor).
3. **Proof 2/3 workstream (separate).** Start with the I11 root-cause; only
   then move the boundary gate. Not required for `ws_frame` parity.
4. **Re-lock/exception baseline.** Owner has ruled this out for now
   (2026-11-03: no re-lock, no exception; find the legal path).

## Next steps (pending decision)

1. Attribution measurements (option 1) — read-only, always safe to run.
2. If Proof 1 is chosen: write the detailed design (which emission sites,
   liveness proof inputs, test plan) as an amendment to
   `2026-10-30-generalized-value-destination-ownership.md` or a new plan.
3. Record measured results in this file's evidence section (append-only).

## Gates (whenever code changes)

`cargo test --lib` · full `--runtime` A/B (ws_frame within tolerance, no
other regression) · `ll_determinism_check --quick` · ASan incl.
stack-use-after-return · I11 · Praetor on changed dirs · Kani if new
safety-critical analysis code.
