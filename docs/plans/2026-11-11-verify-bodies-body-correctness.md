# Briev `--verify-bodies` — prove body ⇒ postcondition

**Date:** 2026-11-11
**Status:** DONE (2026-11-11). Implemented and verified:

- `verify_bodies: bool` on `BuildOptions`; `--verify-bodies` CLI flag (build
  route); bounty site stays `false`.
- Pass: `src/analysis/verify_bodies.rs` — worklist path walker with guard
  fall-through, `term`/`Tail` output binding, `Gate`/`Check` recorded as
  facts, interval-bounds verdicts with Kleene and/or/not, `BodyReport`
  three-way aggregation (proven / unproven / violated).
- Wiring: `compile.rs` consumes the report — `violated` fails the build,
  `unproven` is an advisory warning, supported-but-unproven never rejects.
- **Empirical success criteria 1–3 met:** `defn lies_about_post [a>=0][term==0]
  { term 5; }` → exit 1 with `a path returns term = 5, which contradicts the
  contract` under `--verify-bodies`; compiles clean (exit 0) without the flag;
  honest body proven silently; stdlib undecided bodies are warnings only.
  Full `cargo test --lib` green flag-off (3679 passed); 6 unit tests in-module.
- Soundness notes: `Unknown` never equals anything (structural_eq excluded);
  `!(a == b)` is a disjunction and contributes NO interval fact
  (`negated_comparison` returns None); bodies outside the pure-integer
  surface are skipped, never proven, never rejected.

Success criterion 4 (Counsel Phase-4 valve) remains future work in the
Counsel repo.

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
## Machine-readable report (C1, 2026-11-12)

`brievc build … --verify-bodies --verify-report <path>` writes the report as
JSON before the gate returns, so a failing build still produces tooling-ready
detail. `--verify-report` implies `--verify-bodies`; the gate is unchanged.

```json
{
  "proven": 1,
  "unproven": [{ "name": "g", "reason": "could not derive …" }],
  "violated": [{ "name": "f", "reason": "returned value does not imply the postcondition" }]
}
```

Entries are `{ name, reason }` objects; tooling must not parse prose. Tests:
`analysis::verify_bodies::tests::json_report_splits_names_and_reasons` and
`tests/verify_report.rs` (violation + pass, report written both times).

## C4 — solver discharge (2026-11-12)

`--verify-bodies` now escalates an `Undecided` symbolic verdict to Z3 before
reporting "could not prove".

- **Resolution fix (prerequisite).** `prove_smt`/`prove_smt_formula` invoked
  `z3` on PATH only, while `is_z3_available` also accepted the managed install
  directory (`~/.local/share/briev-compiler/bin/z3`, via `brievc install-deps`).
  A managed-only install therefore reported a solver that no query could reach —
  availability true, every query silently `Unknown`. One `z3_bin()` now resolves
  PATH → managed for every call site.
- **`build_vc_query(facts, post)`** asserts the path's facts together with the
  negated postcondition and declares the free identifiers as `(_ BitVec 64)` —
  the universal quantification weakest-precondition reasoning needs, so callers
  need no parameter list.
- **Encoding.** `/` and `%` now encode as `bvsdiv`/`bvsrem`; previously they fell
  through to `(= l r)`, silently asserting equality.
- **Verdict mapping.** `Unsat` ⇒ proven. `Sat` ⇒ refuted **only when no free
  identifier survives substitution** (the body's result is fully determined).
  `Unknown`, or no solver ⇒ unproven, unchanged from before.

### Why parameter-dependent refutations stay advisory

The prelude carries optimistic contracts such as
`defn coll_min(a: Int, b: Int) [true][term >= 0]`. With a parameter-dependent
body result, `facts ∧ ¬post` is satisfiable for some inputs, so Z3 legitimately
finds a model — and enforcing it turned six prelude declarations (`coll_min`,
`when_raw`, `when_hi`, `coll_hi_fix`, `coll_len_fix`, `exponent_val`) into hard
build failures. Refutation is therefore restricted to fully determined results;
an input-dependent counterexample stays a warning. Tightening those contracts is
its own follow-up, not something a prover should turn into a build-breaking
surprise.

### Evidence

- `cargo test --lib verify` → 36 green. `range_bound_does_not_pin` flips from 0
  to 1 proven once a solver is present — that body is `[a >= 0][term == 2 * a]`
  with body `a + a`, which only the solver can decide, so the flip is the proof
  the solver is doing the work.
- `brievc build <f> --library --verify-bodies --verify-report r.json` with the
  solver installed: `violated: []`, exit 0, `proven: 4` on the prelude.
- Without a solver the same source stays `unproven` — no behaviour change for
  machines that have not installed it.
