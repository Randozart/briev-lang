# Plan 2026-09-22: electronics participation (`unpop`), Rule-22 precision, and the static `when` law

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

This session's review (PCB functionality, clock-sensitive concurrency, layout
guidance, component behavior) resolved to three concrete additions plus one
deferred thermal item. Two earlier decisions stand: the D14 lifting slots
were retracted (`2026-09-22-retract-lifting-slots.md`), and nets are named by
physics, never by authors.

## The `when` law — the definitive semantic

> **`when G { F₁; …; Fₙ }` — `X ⟹ Y`, and the compiler must make it so.**

The meaning is decided by **position** (this is the locked boundary):

| Position | `when` means | Behavior |
|---|---|---|
| `defn` / `node` / `txn` | guarded/reactive behavior (**unchanged**) | `Statement::Guarded`; mechanism synthesis (`when … via`); chain actions — untouched |
| Top level / obj / type | **static forced fact** (new) | `X ⟹ Y`: the compiler obliges — propagate + verify, error on any escape |

- Inside a body it guards behavior; outside (declaring a type's or the
  program's nature) it declares a **law**.
- The compiler **propagates** the consequence (derives it downstream into
  voltage/current/power or proof obligations) and **verifies consistency**
  (nothing contradicts an in-force fact under a satisfiable guard).
- If even one satisfiable state escapes, the compiler **refuses** — it never
  silently passes a state it cannot enumerate (the 2ⁿ bound, same discipline
  as `unpop`).
- "Make it so" = propagate + verify, **never synthesize**: the solver adds
  no parts to honor `Y` (D13).

## Slice A — Rule-22 precision for `.ebv`

`concurrency_gate.rs` runs on every reactive txn pair (compile.rs:410). Its
satisfiability probe is software-shaped:

- `const_value` (proof_engine/mod.rs:358) → `None` for `UnitLiteral` —
  `[u1.sclk.voltage == 3.3V]` vs `== 1.8V` seen as co-satisfiable → false
  `async`/`sync` demand on mutually-exclusive clock levels.
- `expr_eq` (proof_engine/mod.rs:378) can't compare pin-access chains
  (`u1.sclk.voltage`).

**Changes:** fold `UnitLiteral { value, .. }` numerically; add structural
`expr_eq` for field-access chains. General, additive (Rule 6). Also powers
Slice C's guard-satisfiability (`check_satisfiable`).

**Tests:** mutually-exclusive voltage guards → no classification demanded;
co-satisfiable + overlapping pin writes → error demanding async/sync;
software `entry_cmd()=="a"/"b"` UNSAT preserved.

## Slice B — `unpop` dual-state + `Wire` + `shortcircuit`

**`unpop <inst>;`** (top-level, mirrors `budget` dispatch, definitions.rs:49):
- Instance excluded from the BOM (`in_bom no`, emitter mod.rs:413).
- **Mandatory dual-state verification**: present (part conducts per its type)
  AND absent (pins open, Nc-exempt from dangling). Both configurations
  checked for shorts / ratings / decoupling / budgets.
- 2ⁿ enumeration up to a bound (≤4 unpop parts = 16 configs); beyond →
  "split or document" error.
- DNP of a required decoupler → absent-state check fails (cannot DNP out of
  a requirement — the strict, honest consequence).

**`type Wire { pin a; pin b; reference "W"; };`** in
`lib/std/electronics.bv` — present state = `a<->b` short. A jumper is
`unpop wire: Wire;`. No compiler knowledge of `Wire` (Rules 14/15).

**`shortcircuit unpop wire: Wire;`** — acknowledges that populating this part
shorts the net: the shorted-supply hard error (classify_drives,
electronics.rs:611) is **suppressed** for that part's present state, recorded
as a proven-and-acknowledged condition.
**`shortcircuit` on a populated part** → **warning** with a suggest-`unpop`
hint.
**`sacrificial`** → documented in the ledger as the future suppressor of that
warning (needs a melting-point/thermal model — B3, D17 boundary).
**Deferred, not built.**

**AST:** `TopLevel::Unpop { instance }`, `TopLevel::ShortCircuit { instance }`.
**Emitter:** DNP → `in_bom no`; acknowledged short → normal emission with a
proof note.

## Slice C — the static `when` law (both worlds, top/obj/type only)

**C1 — electronics** (type-body + top-level): `when` facts are conditional
drives. The compiler:
1. **Propagates**: guard satisfiable → fact's value enters
   `net_voltage`/current/power derivation (line-644 hook), so `out = 3.3V`
   feeds downstream tolerance/budget/rating proofs.
2. **Verifies consistency**: any drive forcing the same net to a different
   value under a jointly-satisfiable guard → contradiction error (guard-aware
   generalization of the shorted-supply check).
3. **Refuses escapes**: a law that cannot hold in a satisfiable state is a
   compile error (conservative via `check_satisfiable`).

**C2 — software** (obj/struct-body + top-level): same law over members.
`when temperature > 100 { thermal_alarm = true; }` — propagate the forced
value; error if any assignment/contract contradicts it under a satisfiable
guard. Consumer is a real proof-engine obligation — **not** the vestigial
`TypeDefBody.constraints` field (empty/dead today).

**Scoping by declaration site:** top-level facts are program-global; obj/type
facts are scoped to the declaration and inherited per-instance (each
`let s: Sensor = …` carries the law).

**Existing `when` in `defn`/`node`/`txn` is not changed.**

## Slice D — deferred, documented

`sacrificial` + melting point → suppresses the `shortcircuit`-on-populated
warning. Needs a thermal model (B3/D17). Ledger entry now, implementation
later.

## Changes / files

- `src/proof_engine/mod.rs` — `const_value`, `expr_eq` (A)
- `src/analysis/concurrency_gate.rs` — tests (A)
- `src/parser/definitions.rs` — `unpop`/`shortcircuit`/top-level-`when`
  dispatch; obj/type-body `when` in `parse_obj_like`/`parse_type_body`
- `src/ast/top.rs` — `TopLevel::Unpop`, `TopLevel::ShortCircuit`, `when`
  behavior clause on `TypeDefBody`
- `src/analysis/electronics.rs` — dual-state check, shortcircuit
  suppression, conditional-drive propagation (C1)
- `src/proof_engine/mod.rs` or `src/typechecker` — software `when` law
  consumer (C2)
- `src/backend/electronics/mod.rs` — `in_bom no`, shortcircuit proof note
- `lib/std/electronics.bv` — `Wire` type
- `spec/SPEC.md` — §3.5 + §9.4/§9.6 `when` positions
- `docs/plans/2026-09-21-hardware-dialect-gaps.md` — ledger amendments
- `docs/plans/2026-09-21-intent-synthesis-node-semantics.md` — design record
  (D13 row 4, D15/D17 re-statements)

## Gates

- `cargo test --lib` green (sole environmental-probe failure unchanged).
- Praetor: no NEW diagnostics in changed files.
- Commit per slice (A → B → C); D is ledger-only.