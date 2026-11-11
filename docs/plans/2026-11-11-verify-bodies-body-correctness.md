# Briev `--verify-bodies` — prove body ⇒ postcondition

**Date:** 2026-11-11
**Status:** Active (BUILD); plan adopted by the language owner.

## Why

Briev's core claim is contract-driven verification, but today it verifies
contract **consistency**, not **body correctness**. Prove it the way a probe
would:

```briev
defn lies_about_post(a: Int) [a >= 0][term == 0] -> Int {
    term 5;   // violates [term == 0]
};
```

`brievc check` → OK, exit 0. `brievc build --library` → `libbody.a`, exit 0. A
function returning `5` against a postcondition of `0` compiles and links.

Source trace:

| Location | Role |
|---|---|
| `src/compile.rs:696` | `prove_contract` runs **only** for `ProtocolDef` invariants |
| `src/compile.rs:720–740` | txn/defn get **only** `detect_tautology` (reject `[true][true]`) |
| `src/typechecker/mod.rs:4902` | `validate_contract` rejects tautology/missing only |

The machinery to prove bodies exists but is dormant: `src/symbolic.rs`
(`SymbolicState`, `enumerate_paths`, `satisfies_postcondition`) has **no
production compile-pipeline caller** — used only by `protocol_graph.rs`
(composition-inverse) and `fuzzing/concolic.rs`.

## The gap in `symbolic.rs`

It is a foundation, not a sufficient prover:

1. **Return value never bound.** `execute_statement` handles `Assign`/`Let`
   only; `Statement::Term(_)` just pushes state. A postcondition referencing
   `term` can never hold — even a correct `[term == a] { term a; }` would fail
   (`term` stays `Identifier("term")`, `a` stays `Identifier("a")`, unequal).
2. **Two-valued verdict.** `satisfies_postcondition -> bool` conflates
   *provably violated* with *could not prove*.
3. **`path_constraints` stored but unused** — never consulted to discharge
   facts (`x > 0 ⇒ x ≥ 1`).
4. **`symbolic_less_than` is conservative** (literal-only).

Fix: bind the returned value, return a tri-state verdict, consult
`path_constraints` plus an optional Z3 arbiter.

## Design

### `--verify-bodies` (default-off; strictly additive)

- `BuildOptions` (`src/pipeline.rs:163`): add `verify_bodies: bool` (default
  `false`). Add `verify_bodies: false` to both construction sites
  (`src/main.rs:550`, `src/main.rs:922`).
- CLI parsing (`src/main.rs:278–330`): add `--verify-bodies` alongside
  `--library`/`--keep-all-defns`.

### New pass `src/analysis/verify_bodies.rs`

Registered in `src/analysis/mod.rs` (first-class, Golden Rule 10). For each
`txn`/`defn` with a contract, when `opts.verify_bodies`:

1. `SymbolicState::new(pre)` binds the precondition.
2. **Bind the returned value**: each terminating path's `Term(Some(e))` /
   `EndProgram(Some(e))` output becomes synthetic `__result` =
   `eval_symbolic(e, state)`. A postcondition depending on `term` with no
   terminating value is rejected.
3. **Weakest-style walk**: fork on every `Guarded` exactly like
   `enumerate_paths`, tracking assignments + `path_constraints`.
4. **Tri-state postcondition**: `enum ProofVerdict { Holds, Violated,
   Undecided }` — symbolic equality/inequality decides the first two;
   all else `Undecided`.
5. **Arbiter**: for `Undecided`, when `is_z3_available()`, query
   `path_constraints ∧ ¬post` via `proof_engine::smt::build_contract_query` /
   `prove_smt_formula`. `Unsat` ⇒ Holds; `Sat(model)` ⇒ counterexample ⇒
   Violated; `Unknown` ⇒ stays Undecided.
6. **Aggregate**: verified iff every terminating path is Holds. On Violated or
   unresolved Undecided, a helpful diagnostic names the declaration, path /
   counterexample, and fix.

Wired into the txn/defn loop at `src/compile.rs:720–740` (the
`detect_tautology` gate), gated on `opts.verify_bodies`. Fail-closed but
opt-in: under the flag an unprovable body is an error; without it unchanged. A
future `.s` strict profile may enable it by default (`is_strict` at
`compile.rs:518`).

### Tests

- Pass `[a >= 0][term == a + 1] { term a + 1; }`.
- Pass path-split `[a >= 0][term >= 0] { when a > 5 { term a; }; term 0; }`.
- **Reject** `[a >= 0][term == 0] { term 5; }` under the flag; **still
  compiles** without it.
- Reject missing term when post depends on `term`.
- Existing suite (3600+) green with the flag off.

## Soundness boundary

The symbolic executor does not model memory/pointers. Bodies with pointers,
`Alloc#`, external `frgn`, or `Call` to unanalysed functions are classified
`Unknown` and **skipped** under verify-bodies (never wrongly proven); the pass
declares coverage per body.

## Phase 4 valve (Counsel)

Once `--verify-bodies` proves body⇒post in Briev, the same gate discharges VCs
generated from a **restricted pure-integer Rust subset**: a `syn`-based
frontend in Counsel translates the function body into public Briev `Expr`/
`Statement` (`src/lib.rs:30` — `pub mod ast`), builds `[pre][post]`, and hands
them to the same pass. `proof_engine` is `pub` (`src/lib.rs:83`), so Z3
discharge is directly reusable. That is the lighter-than-Kani claim's basis.

## Deferred (decoupled)

Cross-ABI fn-ptr SIGSEGV (`emit_call_ptr`, `backend/llvm/intrinsics.rs:1455`,
closure-slot convention vs flat C binding) is a separate backend ABI decision —
do not block verify-bodies on it. Predicates cross as data and that works.

## Success criteria

1. `[a>=0][term==0]{term 5;}` fails under `--verify-bodies`, compiles without.
2. `[a>=0][term==a+1]{term a+1;}` passes under the flag.
3. Existing suite green with the flag off (identical behaviour).
4. Counsel Phase-4 probe discharges a translated VC through the same gate.