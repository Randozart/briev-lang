# General workstream: countable-body statement drops + queued fold work

**2026-11-08.** Status: ACTIVE.
**Provenance:** the §12 idiom workstream (`2026-11-06-idiom-recognition-doctrine.md`,
W3/W3.1) surfaced a host-side backend bug while wiring the `g0_minmax` XPU
gate. This plan owns the **general compiler/language** follow-ups. The XPU
roadmap (G1.5 Level Zero / OpenCL IL projection) is explicitly out of scope
here and stays in `docs/architecture/intel-dispatch.md`.

## P0 — `emit_countable_body` silently drops nested `Foreach` (correctness)

**Status:** LANDED 2026-11-08 — Foreach arm + exhaustive audit
(`loop_engine/counter.rs`), regression test
(`tests/countable_body_foreach.rs`), `g0_minmax` required + PASS on PTL
(`bad=0 MIN=ok MAX=ok`), BUGS.md updated.

**Evidence (2026-11-06, `/tmp/opencode/mm_allplain.ll`):** a plain counted
node `maxrow { let acc = a[j*K]; foreach k in 1..K { acc = Max#(acc, a[j*K+k]) }; ymax[j] = acc; j = j+1 }`,
folded into `@txn_maxrow`, emitted `ymax[j] = a[j*K]` — the inner `foreach`
**vanished**. The sibling `minrow` (same shape, alwaysinline path) kept its
loop. The walker
`src/backend/llvm/loop_engine/counter.rs:1974 emit_countable_body` has no
`Statement::Foreach` arm; its `_ => {}` catch-all silently drops it. The
file's own comments record the same bug class twice before (free/keep hints,
statement-match, arrow-assign) — each fixed by delegating to
`emit_stmt::emit_statement`.

**Impact:** silent wrong results for ANY folded/counted node containing a
nested loop, on every backend (CPU included). Exposed by the W3.1 gate, not
caused by it.

**Work items:**
1. Add a `Statement::Foreach` arm delegating to
   `super::emit_stmt::emit_statement` (provenance comment + "To undo").
2. Exhaustiveness audit: enumerate every `Statement` variant against the
   walker's arms; executable statements must have an explicit arm or a
   documented deliberate decision — no silent fallthrough.
3. Nested-foreach **field writes**: the walker's field-phi bookkeeping
   (`pending_phi_backedge` / `last_val_temps`) does not see writes made
   inside a nested loop. First pass: local-accumulator folds must be
   correct (the gate shape); a field write inside a nested foreach must
   either work or produce an actionable diagnostic. Test both.
4. Regression tests: IR test pinning that the folded node contains the
   nested loop and its ops (`llvm.smin/smax` in the max folD); re-enable
   `g0_minmax` as a **required** Vulkan rung in `benchmarks/intel_ptl/g0.sh`.
5. Full suite + PTL gate; BUGS.md entry updated with the IR excerpt.

**Acceptance:** `g0_minmax` PASS on Vulkan (bad=0, exact Int extrema);
`cargo test --lib` green; no silent `_ => {}` for executable statements in
the walker.

## P1 — Float folds: the `approx` / `exact` ambiguity-keyword pair

**2026-11-08 operator decisions (recorded):** default precise; `exact`
optional assertion (not mandatory classification); `exact` also pins FMA
contraction off; node-level now, per-`foreach` later; `!>` stays out of the
author surface (full `!>` removal deferred to its own plan); `exact` +
*proven* exactness may use the parallel lane once P1b lands (until then
`exact` is strictly sequential).

### Semantics

| Declaration | Meaning | Compiler behavior |
|---|---|---|
| *(none)* — **default** | result must follow source-order evaluation | precise: no reassociation, no contraction, no fast attrs |
| `approx node` | result may deviate within FP rounding | float folds take the warp-slice lane; emits the internal fast attrs (lowering detail) |
| `exact node` | the precise sequential result is required | no reassociation/contraction for this node; suppresses the float fold lane; strips inherited fast attrs |
| both | contradiction | **parse error** (mirrors `seq seq`) |

`approx`/`exact` are **ambiguity keywords** (keyword-taxonomy): declared at
the site of confusion, resolved by author intent, disclosed in diagnostics,
never speed keywords. `exact` is an assertion, not a requirement — the
default is already precise; `exact` defends against future module policies /
link flags and documents intent.

### `!>` stance (operator: `!>` is being retired from language use)

`approx`/`exact` are the **only documented author surface** for numeric
semantics. The `associative` / `fp_math` `!>` keys stop being author
vocabulary; `config/meta-vocab.dbv` + the registry remain the internal
lowering table (mechanical, not documented for authors). The SPEC §8.8 note
that says the reassociation right is "available via `!> associative` /
`!> fp_math: fast`" is rewritten to name the keywords. No new `!>`
dependence is introduced. Full `!>` retirement: separate plan.

### Precondition gate: `-ffast-math` A/B (recorded in this plan)

`src/compile.rs:2798,2883` passes `-ffast-math` unconditionally to clang. If
that relaxes an `.ll` whose float ops carry no fast flags, the precise
default is false and `exact` cannot be honored — the flag must be
gated/removed. Protocol: a rounding-sensitive float program, same `.ll`,
link with and without `-ffast-math`, compare result bits. Record the outcome
here. **Result:** see "A/B record" below.

### Implementation

- Lexer/vocab/parser: `approx`, `exact` tokens; modifier arms; contradiction
  check in `record_annotation`; keyword-accounting gate.
- Analysis: `KernelShape.approx_declared` / `exact_declared`; outcome gains
  `ApproxDeclared` (Ok) and `ExactPinned` (suppressed info, not an error);
  why-not text names both fixes.
- SPIR-V: float synthesis (`SubgroupFAdd#`/`FMul#`/`FMin#`/`FMax#`, Float
  types); Int path unchanged.
- LLVM: `approx` emits the internal fast attrs; `exact` strips them and
  suppresses the lane; disclosure strings.
- Tests: parser (pair, contradiction both orders, order-free), analysis
  license/suppression, IR (FAdd group op under `approx`; no group op under
  `exact`; attrs), PTL gates (`g0_fsum_approx`, `g0_fsum_exact`), A/B.
- Docs: SPEC (new ambiguity-keyword category + numeric-semantics subsection +
  §8.8 rewrite), keyword-taxonomy, optimization-hints/meta-vocab marking,
  tutorial, highlighter, ledger rows.

### P1 status (2026-11-08)

**LANDED.** `approx`/`exact` parse as the numeric ambiguity pair (lexer/vocab/
parser, contradiction check); the fold lane consumes `approx` (float) and
`exact` suppresses it; Float synthesis emits `SubgroupFAdd#`/`FMul#`/
`FMin#`/`FMax#` with Float accumulators; diagnostics disclose the license.
Default precise. The pair does **not** emit LLVM function attributes — the
registry's `reassoc`/`fast` mappings are instruction flags and are invalid in
the function-attribute position (verified; BUGS.md 2026-11-08, also latent on
the `!>` path). Tests: parser pair/contradiction, analysis license/suppression,
IR (`GroupNonUniformFAdd` under approx; none under exact), PTL gates
`g0_fsum_approx` (reassoc lane, bit-exact integer-valued data) and
`g0_fsum_exact` (pinned sequential), both `BAD=0`.

**Queued:** P1b exactness-proof tier (bounded-range/count proof licenses the
float lane with no declaration — needs the range-fact surface design);
per-`foreach` modifier granularity (AST work); `!>` numeric-key retirement
(the docs now mark them internal; the global `!>` removal is its own plan).

### A/B record (2026-11-08)

Program: sequential f32 sum of `[2^24, 1.0 × 1023]` (reassociation would
produce 16778240; source order stays 16777216). Same emitted `.ll`, linked
with and without `-ffast-math`:

```
== no -ffast-math ==   16777216
== -ffast-math ==      16777216
```

**Verdict:** the link flag does not add fast-math flags to the emitted IR —
the precise default holds at the binary level. The flag stays (codegen
policy only), and `exact` is enforceable. This also means future LLVM
changes could make the flag load-bearing; the A/B is the standing check
(rerun if the toolchain matrix changes).

## W4 completion + `emit_stmt` audit — LANDED 2026-11-08

- **W4 detection-level why-nots:** `detect_idiom_fold` now classifies
  fold-shaped near-misses (multi-statement body; operand not a plain buffer
  read; index reads the accumulator; unsupported operator) and the blob site
  discloses the named reason via `diagnose_idiom_fold`. Also fixed the old
  Sum-only guard that silenced Product/Min/Max why-nots. Live check:
  `info: fold-idiom not applied … — the folded operand must be a plain buffer
  read …` (and the multi-statement equivalent).
- **`emit_stmt` wildcard audit (P0 sibling):** the statement emitter's match
  is exhaustive — compile-time/analysis-only forms (`MetadataAssignment`,
  `InlineDefn`, `TrgBinding`, `Open`) are explicit documented no-ops; the
  `asm "..."` STATEMENT has no LLVM lowering and is rejected at the capability
  gate (`CAPABILITIES.inline_asm = false`; `Asm#` is the sanctioned intrinsic).
  Regression: `llvm_rejects_inline_asm_statement_via_capability_gate`.
- **Tests:** `idiom_near_miss_diagnosis_names_the_fact` (four near-miss
  classes + a non-fold control) plus the capability test. Suite: 3558 green.

## P2 — W5 idiom candidates (queued)

Scan (prefix/inclusive), transpose-in-GEMM (register-shuffle load path),
map-chain fusion. Each requires its own plan instance + §8 ledger row
(`tests/idiom_ledger_gate.rs` enforces the row).

## P3 — AGENTS.md §12 pointer (queued)

Rule 23 gains a one-line pointer to `derivation-not-recognition.md` §12.
The branch is merged into main, so the merge-time blocker is gone.

## Verification protocol

Every P0 rung: correctness before timing; same-commit gate evidence in
`benchmarks/results/` when the PTL node is involved; `cargo test --lib`
green; Praetor on changed files (house checklist). BUGS.md entry updated
with the root cause, the fix, and the undo path.
