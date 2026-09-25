# Plan 2026-09-22: retract the D14 "lifting slots" — honest subtraction

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Why

The D14/D16-p3b slice (commit `6886c577`) added `bind`, `store`, and
`open` as "lifting slots". Review found that three of the four constructs
carry no semantics the engine can act on, and the fourth (`store net` /
`net <name>:`) duplicates knowledge the compiler already derives:

| Construct | Honest? | What it actually does | Verdict |
|---|---|---|---|
| `bind a.pin = b.pin` | No | A plain body wiring fact `a = b` (both pins) does the identical union (electronics.rs Assign arm). `bind` only changes proof wording. | **Remove** |
| `store inst.field = value` | No | Nothing. Physics (series-part Ohm's law) and the emitter read the `let` literal, never the store. Inert proof string. | **Remove** |
| `store net(pin) = "name"` | No | Duplicates the `net <name>:` precondition annotation. Both assert names the compiler derives from physics. | **Remove** |
| `net <name>:` annotation | No | Nets are already identified by `derive_voltage` (`net_voltage`) and by pin classes (`Supply`/`Return`). A name adds nothing; a name that contradicts physics would be a lie we'd trust. | **Remove** |
| `open a.pin, b.pin` | **Yes** | A **negative** constraint. The netlist is the transitive closure of positive wiring facts — "these two must NOT connect" can never be inferred from absence. The complement gate to the phase-3 redundancy check. | **Keep** |

The engine is deterministic (single solution, no value solver, no
candidate enumeration). "Persist-tighten" and "commit-select" presume a
solver that does not exist. Rule 2 says: if the compiler could have
inferred it, using the keyword is a bug report — here the keywords
themselves are the bug.

Nets are **named by what they are, not what an author calls them**: the
emitter derives labels from pin classes + derived voltage.

## Changes

1. **AST** (`src/ast/top.rs`, `src/ast/expr.rs`):
   - Drop `Statement::Bind`, `Statement::StoreValue`, `Statement::StoreNet`.
   - Keep `Statement::Open`.
   - Drop `Expr::Named` (only existed for `net <name>:`). Update the
     `collect_eq_triples` name plumbing.
2. **Parser** (`src/parser/statements.rs`, `src/parser/expressions.rs`,
   `src/parser/definitions.rs`, `src/parser/helpers.rs`):
   - Drop the `bind`/`store` contextual arms; keep `open`.
   - Drop `parse_store_statement`, `parse_store_target`,
     `check_next_lparen` (only store used it).
   - Drop `net <name>:` parsing in `parse_expression` / AND-LHS.
3. **Analysis** (`src/analysis/electronics.rs`):
   - Remove `stored`/`stored_nets` sink fields, `lift_stmt`'s
     Bind/Store arms, `fold_stored_net_names`, `net_names` resolution,
     `raw_conflicts`/`net_conflicts`, `stored_net_names` threading.
   - Keep the `opens` collection + verification.
   - `collect_pin_unions` no longer collects `net <name>:` — the
     precondition topology unions stay; the name third-element goes.
   - `Net.name`: `partition_nets` assigns structural labels only
     (`GND` for return-class nets, `V{x}` for supply-class nets from
     derived voltage, `N{index}` otherwise) — or derive in the emitter.
     Chosen: derive in the emitter from `net` pins + `voltage.net_voltage`
     so the analysis stays topology-only.
4. **Emitter** (`src/backend/electronics/mod.rs`):
   - `emit_net` labels from derived identity: return-class → `GND`;
     supply-class → `V{volts}`; else `N{index}`.
5. **Match-arm sweep** — remove the added arms from the last slice in:
   annotator.rs (2), analysis/dataflow.rs, backend/llvm/helpers.rs
   (+ the lift-identifiers + stmt-body helpers), backend/llvm/mod.rs,
   interpreter/eval.rs, macros/eval.rs, macros/selection.rs, reactor.rs,
   typechecker/mod.rs, ast/display.rs, beast serialize/deserialize.
   `Statement::Open` arms stay.
6. **Tests**: drop the `bind`/`store`/`net <name>:` tests (parser
   `lifting_slot_tests`, electronics bind/store/net tests, the
   `net vcc:` precondition tests); keep `open` tests; add emitter-label
   tests (GND / V3V3 / N#).
7. **Docs**: ledger amendment (D14 slots deferred until a solver exists —
   record the "keywords were the bug" finding), design record (D13 table
   rows 2/3, D14, §3.2 fixture), SPEC §3.5 + the earlier `net <name>:`
   references.

## Non-goals

- No value solver is added. `open` is the only survivor.
- `Statement::Open` remains fully wired (analysis, match arms, tests).

## Gates

Full suite green (sole environmental probe failure unchanged), Praetor no
new diagnostics, commit.