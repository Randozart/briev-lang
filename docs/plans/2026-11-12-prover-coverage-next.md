# Prover coverage — next leg

**Date:** 2026-11-12
**Status:** Active. P0 implemented this session.
**Predecessors:** `2026-11-11-verify-bodies-body-correctness.md` (C1–C6),
C3 inter-procedural summaries (`3c1a0d59`).

## The measurement that reorders the plan

The eight prelude declarations still reported `unproven` after C3 are **not**
blocked by missing prover capability. Read their bodies:

| Declaration | Body | Postcondition | Why it cannot be proved |
|---|---|---|---|
| `when_raw(a, len)` | `when a<0 {term 0}; when a>len {term len}; term a;` | `[true][term >= 0]` | `len` may be negative |
| `when_hi(b, len)` | `when b>len {term len}; when b<0 {term 0}; term b;` | `[true][term >= 0]` | `len` may be negative |
| `coll_min(a, b)` | `when a>b {term b}; term a;` | `[true][term >= 0]` | both operands unconstrained |
| `coll_hi_fix(h, lo)` | `when h<lo {term lo}; term h;` | `[true][term >= 0]` | both operands unconstrained |
| `coll_len_fix(h, l, len)` | `when h<l {term 0}; term len;` | `[true][term >= 0]` | `len` may be negative |
| `exponent_val(exp8)` | `when exp8==0 {term -149}; term exp8-150;` | `[..][term >= -149 && term <= 104]` | real range is `[-149, 105]` — off by one at `exp8 == 255` |
| `mantissa_val` | guards + `0x800000` | `[..][term >= 1 && term < 1 << 24]` | shift in the goal, not in the body |
| `iter_collect<T>(list)` | `term list;` | `[true][term.Count#() == list.Count#()]` | intrinsic `Count#()` — outside the surface |

Two of the eight are contract bugs (`exponent_val` off-by-one), five are
**optimistic contracts** whose bodies genuinely can return a negative value, and
one is an intrinsic/generic case. The prover is right to refuse all of them.

**So "fields and collections" — my earlier guess — is not the blocker for this
population.** Evidence first, plan second.

## P0 — make `unproven` actionable (this session)

A bare "could not prove the postcondition on N path(s)" tells the author nothing
about *which* path or *what* fact is missing, which is exactly what makes the
stdlib repairs above risky to attempt.

Change: name the first undecided path and render the goal, e.g.

```
could not prove the postcondition on path 2 of 3 (goal: len >= 0) — add a guard or a stronger precondition
```

Zero semantic risk, no change to any verdict. Implemented in `verify_one`; the
`Expr` Display impl already exists (`src/ast/display.rs:12`).

Acceptance: existing suite green; the prelude's unproven entries now carry a
path index and a concrete goal.

## P1 — repair the five optimistic contracts (needs owner review)

CONTRACT-FIRST says fix the code, not the contract, and here the code is the
thing that is wrong: a clamp returning a negative value is nonsense.

For each of `when_raw`, `when_hi`, `coll_min`, `coll_hi_fix`, `coll_len_fix`,
in **both** `lib/std/cast_lanes.bv` and `lib/std/cast_portable.bv`:

```briev
// the clamp is only meaningful for a non-negative bound
when <bound> < 0 { term 0; };
```

Then the declared `[true][term >= 0]` becomes true on every path and the prover
should discharge all five. Behaviour change is confined to negative bounds,
where the old result was itself negative (already outside the documented
contract).

`exponent_val`: the code is right and the contract is off by one — the real
range is `[-149, 105]`. Correct the postcondition, and record why.

**This phase changes shipped stdlib semantics, so it is deliberately separate
from P0 and wants an explicit go-ahead.**

## P2 — surface breadth (the actual capability work)

In priority order, each gated on evidence from real code rather than guesswork:

1. **Intrinsic calls with known semantics** (`Count#`, `Length#`) — cheap,
   unlocks generic containers like `iter_collect`.
2. **Bitwise and shift operators** in `expr_supported` and `encode_expr_smt`
   (`bvsll`/`bvsrl`/`bvand`/`bvor`) — `mantissa_val`'s goal uses `1 << 24`.
3. **State fields** (`Expr::Field`) as symbolic variables, with `@field` as the
   pre-state — needed before any stateful txn body can be proved.

## P3 — carry summaries into Counsel's Rust translator

`counsel::check` currently verifies leaf bodies; a Rust function calling a
helper is rejected as unsupported. The Briev-side summary machinery (C3) is the
model: summarise pure `fn`s, substitute at call sites, refuse cycles.

## Gates

- `cargo test --lib verify` (baseline 40) then `bash scripts/gate.sh quick`.
- Prelude probe before/after: `proven` / `unproven` / `violated` counts.
- `brievc freshness` must report `fresh` before any verdict claim reaches
  Counsel (a stale prover is indistinguishable from a missing feature).
