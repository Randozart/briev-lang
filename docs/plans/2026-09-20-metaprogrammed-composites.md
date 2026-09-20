# Metaprogrammed Composites — declared shapes, eternal machinery

**Date:** 2026-09-20
**Status:** active — governs M3/M4 sequencing; supersedes the in-flight
Rust-side synthesis approach for M3
**Doctrine:** `docs/architecture/proof-vs-shape.md`
**Refines:** `docs/plans/2026-09-20-gpu-dialect-beyond-cuda.md` (M3/M4)

---

## The decision

The compiler detects what proofs can reach; the language metaprograms what
not even the compiler could reasonably be expected to detect. Softmax is
temporal — research will rename the hot kernel within two years — so no
algorithm shape may live in the compiler, not even as a declared lowering.
The shape lives in the language (stdlib composite); the compiler keeps
proofs, proof-licensed rewrites, general lowering, and the metaprogramming
layer that expands declared shapes into the full pipeline.

## In-flight disposition (2026-09-20)

Code written before this plan, all UNCOMMITTED:

| Piece | Disposition |
|-------|-------------|
| `src/analysis/softmax_chain.rs` detection core (topology + shape proofs) | SURVIVES |
| `softmax_chain.rs synthesize()` — Rust-built `Max#`/`Exp#` bodies, `-1e30` identity, 2-pass structure | **DELETED — never committed** |
| `gpu_schedule.softmax_chain` field + detection call | SURVIVES (records the proven chain; emission binds to the composite rewrite) |
| `ptx/mod.rs build_softmax_chain_kernel` skeleton | SURVIVES; the synthetic shape carries the EXPANDED composite body, not a synthesized one |
| `runner.rs try_emit_softmax_chain` + `ptx_softmax_chain` knob | SURVIVES |

## Front A — expression-parameterized composites (the language learns)

Normative spec — what the layer OUGHT to do. Current macro machinery is
gap-size information, not design input.

1. **Expression-typed parameters** — a composite takes any `Expr` (the
   score expression references `h`/`j`/`d`), hygienically substituted; no
   accidental capture; ambiguity fails closed.
2. **Expansion before analysis** — one IR. Expansion completes before accel
   analysis so typecheck, contracts, defn-liveness, and both backends see
   the expanded body. No macro-world/analysis-world divergence.
3. **Contracts on composites** — the composite carries `[pre][post]`;
   every instantiation is proof-checked through the expansion. C++
   templates cannot do this; this is the "more intelligent language" part.
4. **Disclosure** — the `!` suffix (existing convention):
   `softmax_fused!(...)` reads as compile-time expansion at the use site.
5. **Stdlib location** — `lib/std/`; the extension mechanism (rule 14).

Syntax (draft): `$defn`-based definition with expression parameters,
invoked as `name!(args...)`. Final syntax reviewed at Front A draft time.

**Gates:** expansion + hygiene unit tests (capture rejection, shadowing,
recursion depth); a contracted composite instantiates and the contract is
checked on the expansion; expansion output flows through accel analysis +
both backends unchanged.

## Front B — declared softmax (M4-lite arrives here)

`lib/std/numeric.bv`: `softmax_fused!` — canonical body = the
deferred-softmax form (the 2-pass structure the flash2p template proved),
parameterized by (score expression, value operand, head/d/j bindings).

**Lane 2 (explicit) gate:** direct call from an `.abv` → expansion →
deferred-region lowering fires structurally (M2-gated) → numerics PASS on
the RTX 3060 (max_rel < 1e-3 vs double reference).

`detect_row_softmax` retires when the composite covers its forms — earlier
than the original ledger planned.

## Front C — M3 chain fusion as a rewrite, not a synthesis

Detector (survives from the in-flight work): producer = pure score
expression; middle = the DECLARED softmax (identity by declaration, not
shape matching); consumer = M2-proven linear fold. Narrow by design —
Lane 2 covers everything detection cannot prove, so narrow auto-detection
no longer caps capability.

Rewrite: three nodes → ONE composite invocation with bound parameters →
expansion → the same deferred lowering. No Rust-built AST bodies anywhere.

**Gates:** m3 harness PASS both lanes; launches 3 → 1; ≤ ~120 µs f32
target (chain today: 202 µs; deferred emitter: 198 µs at gate geometry).

## Front D — Stage 2: the deferred emitter retires

Formal milestone. The general machinery (reduction synthesis, warp
slicing, chain fusion) drives the PLAIN expanded composite — no
deferred-region matcher, no shape knowledge in the backend — to parity
with the deferred emitter's numbers.

**Gate:** composite through general passes ≥ deferred-emitter timing − 10%
at the m3 gate geometry, both correct. Then the deferred emitter and every
structural matcher feeding it retire per the ledger. A structural matcher
in the backend is a loan, not an asset; this repays it.

## Verification discipline

Unchanged from the dialect plan: baseline table before changes, controlled
A/B (rule 12b worktree) before any default flip, `cargo test --lib` per
commit, Praetor on changed files, device correctness at real shapes,
output equality at a bound that crosses a print boundary.

## Risks

| Risk | Mitigation |
|------|------------|
| Expression-param hygiene is a real PL problem (capture, shadowing) | Hygiene rules specced first; fail closed on ambiguity; tests before features |
| Expansion-before-analysis fights existing macro timing | The invariant is ONE IR before accel analysis, not the exact expansion site; the first AST-normalization pass is an acceptable fallback |
| Composite contracts too weak to prove the deferral | M2's proof obligations are the floor and the composite carries them; rejection falls back to the 3-kernel path |
| Stage 2 parity never reached | The deferred emitter stays WITH its retirement gate recorded — an acknowledged loan (doctrine §Existence proofs) |
| The composite syntax grows into a second language | Delimiter doctrine holds: `()` application, `{}` definition, `!` disclosure; expression params are the only new load |
