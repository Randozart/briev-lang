# Plan 2026-09-22: core `chain` / `into` sequencing sugar

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`
**Gate:** parse-time desugar to plain reactor nodes (D9), works in `.bv`
(software) and `.ebv` (hardware), wired through `derive_netlist`.

## Why

D9 (design record) already declares chains "zero private semantics — pure
grouping ergonomics; general across dialects (core desugarer pass,
additive)". They were designed as a core-language construct. This slice
makes them real in the core: `chain` is a top-level block that desugars
to plain `node`s; `into` is the sign-off (ANDed into all subsequent step
guards). The electronics dialect gets the same construct for free, and
`derive_netlist` sees only ordinary nodes.

The word choices are the unification result from this thread (one keyword,
one meaning):

| Concept | Keyword | Status |
|---|---|---|
| Chain block | `chain` | free — new core keyword |
| Chain sign-off | `into` | free — new core keyword |
| Task await | `await` | stays main-language-only (task handles) |
| Ownership transfer | `keep` | stays main-language-only |
| Mechanism strategy | `via` | unchanged (electronics) |
| Persist-tighten (D14) | `bind` | future slice |
| Commit-select (D14) | `store` | future slice |
| Disconnection (D16 p3b) | `open` | future slice |

`await`/`keep` are NOT reused for the sign-off / persist-tighten — they
already mean something else in the core. The D9 `await` spelling and the
bare-`trg` shorthand are superseded: electronics authors write
`into u_buck5.pgood;` and `into pwr_btn;` (never bare `pwr_btn;`).

## Grammar

```
chain <name> [<base-guard>] { <chain-body> }
chain <name> { <chain-body> }            // base-guard optional → [true]
chain-body := <action>* (into <cond>; <action>*)*
```

- `<action>` = any ordinary node-body statement (let / assign / call /
  `when` guarded statement / block / …).
- `into <cond>;` ends the current step; `<cond>` is ANDed into every
  LATER step's guard. Only recognized in chain-body position (contextual,
  like `budget`/`section`).
- A chain must end in an action step (a trailing `into` with nothing
  after it is an error: a sign-off gates a subsequent step, and a final
  one gates nothing).
- Empty chain (no actions) is an error.

## Guard discipline

Every step desugars to a reactor node with the SAME guard discipline as a
hand-written node:

- **Start**: the chain's base guard is step 1's precondition (or
  `[true]` if omitted — matching node defaults).
- **Each sign-off** `into <cond>;` is ANDed into all subsequent steps'
  preconditions: step N's guard = `base ∧ s₁ ∧ … ∧ s_{N-1}`.
- **End**: the final action step's postcondition is `[true]` (the body
  does the work; nothing post-hoc to prove) — exactly what a node with a
  real body carries. The user can also write the completion explicitly by
  giving the chain a trailing guarded action; chains do not invent
  pseudo-facts.

So chains are as guarded as nodes are at the start and end: step 1 is
guarded by the base (or true), every later step is guarded by the full
accumulated sign-off conjunction, and the last step has a normal node
postcondition.

## Desugar rule

```
chain power_up [dc_present] {
    u_buck5.en = high;
    into u_buck5.pgood;
    u_buck3.en = high;
    into u_buck3.pgood;
    u_core.en = high;
};
```
→
```
node power_up_1 [dc_present]                                 { u_buck5.en = high; }
node power_up_2 [dc_present && u_buck5.pgood]                { u_buck3.en = high; }
node power_up_3 [dc_present && u_buck5.pgood && u_buck3.pgood] { u_core.en = high; }
```

- Step names: `<chain>_1`, `<chain>_2`, … (underscore, not dot — a dot
  reads as field access). Nothing references step nodes by name in
  practice; the chain name is the grouping.
- `is_reactive = true` on every step.
- The desugar is **parse-time** (the pipe-chaining precedent): `chain`
  never reaches the AST, so the ~40 exhaustive `TopLevel` matches across
  analysis/backends need zero new arms.
- A `when`-guarded action inside a step desugars naturally — it is just
  a statement in the step's node body; the step's own guard is the
  accumulated conjunction.

## Changes

1. **Parser** (`src/parser/definitions.rs`, `src/parser/statements.rs`):
   - `parse_top_level`: `check_identifier("chain")` → `parse_chain()`
     returning `Vec<TopLevel>` (multiple nodes).
   - `parse_program`: splice the returned nodes into `items` (currently
     pushes one item per `parse_top_level`). Keep error recovery.
   - `parse_chain`: name, optional `[base-guard]`, body. Inside the body,
     a *contextual* `into` identifier splits steps; `into` is recognized
     only in chain-body statement position. Guard accumulation builds
     `Expr::BinaryOp(And, …)` conjunctions.
2. **`src/vocab.rs`**: add `chain`/`into` highlight entries (contextual
   keywords, like `budget`).
3. **Tests** (parser tests in `definitions.rs`):
   - Parse: chain → correct node count + guards; software form; base
     guard optional; nested `when` actions inside a step.
   - Errors: trailing `into`; empty chain; `into` outside a chain body
     (an ordinary identifier — no new reserved word globally).
   - E2E: a software chain that sequences state through the interpreter.
   - Electronics: `.ebv` chain wired through `derive_netlist` — the chain
     desugars to nodes, so the existing node-analysis path proves the
     wiring (no netlist-code changes).
4. **Docs**:
   - This plan.
   - `docs/plans/2026-09-21-intent-synthesis-node-semantics.md` D9:
     `await` → `into`, bare-`trg` shorthand removed, "core desugarer
     pass" now real.
   - `docs/plans/2026-09-21-hardware-dialect-gaps.md`: ledger entry for
     the keyword unification (`chain`/`into` core; `await`/`keep` stay
     main-language; `bind`/`store`/`open` settled; `fix` rejected as a
     false friend).
   - `spec/SPEC.md` line 378: `chain`/`await` → `chain`/`into`; new § for
     the core `chain` construct with the desugar rule and guard
     discipline.
5. **Gates**: full suite green, Praetor per-directory on changed files,
   commit.

## Non-goals

- `bind`/`store`/`open` (D14/D16 p3b) are separate future slices — the
  keywords are settled here, not implemented.
- Cells (D10) holding internal chains — later.
- An `into` inside an expression context stays an ordinary identifier.