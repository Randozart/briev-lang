# Electronics HIR port — close the documented gap (2026-11-12)

Worktree: `/tmp/electronics-port2`, branch `electronics-hir-port2`.
Baseline: `01e0c5f8` (H5 electronics port, 14 parity tests, documented
unsupported list in `src/hir/electronics.rs`).

## Goal

`src/hir/electronics.rs` covers the same surface the consumer uses as
`analysis::electronics` — AST-vs-HIR parity, exact canonical equality.
Consumers are NOT flipped; the backend is not edited; `BRIEV_HIR_EMIT` is not
touched.

## Unsupported list → coverage plan

| Unsupported item | Where the AST reads it | HIR port |
|---|---|---|
| Data tables + table-indexed fields | `TopLevel::Data(DataTable)`, `collect_instances` | `HirProgram.data_tables` (name/dims/cells), lower arm, resolver over `HirExpr` |
| Bus `[a..b]` equalities | `Expr::Range` index | `HirExprKind::Range` already lowers; port `expand_bus_pair` family |
| Wildcard `[*]` equalities | `Expr::Wildcard` | NEW `HirExprKind::Wildcard` (lower + `to_ast` round-trip + walk/verify/stats arms); port wildcard family |
| net<>/stdnet<> membership | `let` modifiers, `collect_net_names` | `HirGlobal.annotations` already carries payloads; port the ladder (expectation → tolerance → propagation → ambiguity) + driven rails (contract drives + law births) + author labels |
| Named modes | `ModeDecl.facts` | `HirTypeDef.mode_decls: Vec<HirModeDecl>` (name + facts); `modes: Vec<String>` stays (TypeInfo parity) |
| Mechanism synthesis | txn bodies (`Assign`/`Guarded`/`Open`/`Expression`/`MetadataAssignment("through"/"hold")`) | `HirCallable.body` already carries all forms; port FactSink walk, condition_control, bridge synthesis, drive intents/assignment, opens |
| Series synthesis (E-series) | probe laws + current obligations + rails | HIR probe elaboration → port `synthesize_series_values` family; E-series ladder shared from `analysis::electronics` (`pub(crate)`) |
| Constitutive-law elaboration | `electronics_laws.rs` | NEW `src/hir/electronics_laws.rs`: HIR recursion, shared pure algebra (dimension arithmetic, BuiltExpression) exposed `pub(crate)` from the AST module (DRY, Rule 17); produces the SAME `ComponentLaws` IR |
| DC solve | `electronics_dc.rs` | NOT ported — the module consumes only shared result types (`ComponentLaws`, `Net`, `PinRef`, `TypeInfo`, `ComponentInstance`); reused verbatim. Documented in the laws module header |
| Current/power proofs | `derive_current`/`derive_power`/`derive_law_power`/`check_max_current`/`check_drive_capability` | Port over shared types; `format_watts`/`format_ohms`/`ohm_prefix` made `pub(crate)` and reused |
| Budgets | `TopLevel::Budget` | `HirProgram.budgets` + lower arm; port `check_budgets` |
| Mode predicate eval / node mode validation | `eval_mode_predicate` over `Expr` | Port over `HirExpr` |

## Pipeline shape (mirrors AST `derive_netlist`)

type pins → instances (+tables) → pin unions (plain/bus/wildcard) → probe
laws → series synthesis → real laws → return topology (participation, return
net, rail membership, decoupler bridges) → intent pass (body facts, bridge
synthesis, obligation forcing, drive assignment, opens) → net labels →
group/partition (labels) → voltage → post-solve (DC states, mode validation,
proofs, budgets) → decoupling + contention.

Drift fix found during reading: the HIR port restricted contract scans to
"bare" transactions (non-exported, no sync). The AST scans ALL
`TopLevel::Transaction` items. The port switches to ALL `Transaction`
callables to match the oracle (parity is the goal).

## Result boundary

`canonical_dump` gains a `component_laws` section (Debug dump in instance
order — deterministic; `ComponentLaws` derives `Debug` + `PartialEq`) so law
programs assert exact equality. Everything else stays.

## Shared (pub(crate)) surfaces opened in the AST modules

- `analysis::electronics_laws`: `LawDimension` ops, `BuiltExpression` +
  leaf algebra, `constant_quantity`, `reject_duplicate_equations`,
  `spec_constant` — all AST-free math/data reused by the HIR elaborator.
- `analysis::electronics`: `e24_base`, `e_series_value_in_window`,
  `format_watts`, `format_ohms`, `ohm_prefix` — config data + formatting.

## Tests

Extend the parity suite: tables (+3 error shapes), bus ranges (+mismatch,
+reversed), wildcards (pin-array, instance-array, broadcast, mismatch),
net/stdnet (inference, expectation, ambiguity, labels), modes (catalog, DC
per-mode, node validation, undeclared, OR-mix), mechanisms (synthesis,
through-Type, ambiguous, redundant), series synthesis (forcing + E-series
pick), current/power proofs (divider, KCL, bounds proven, max current, drive
capability, law power), budgets (pass + violation), laws (elaboration +
DC + errors). Exact canonical equality everywhere.

## Gates

`cargo test --lib electronics`, `cargo test --lib hir::electronics`,
`cargo build --lib`, `bash scripts/gate.sh quick`,
`praetor validate --warn --target src/hir` / `src/analysis`.
