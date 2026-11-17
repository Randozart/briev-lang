# Trunk next steps — 2026-11-17

State: `main` = `99aae8d8d` (merged `intel-xpu-harvest`; contains the #120
register-width fix `4e7daca61` but **not** its follow-up PR #122). PR #122 is
open and is the correct completion of the wasm fix. This plan supersedes the
"OPEN" framing in BUGS.md for that defect.

## 1. Land PR #122 (narrow the register-width reset) — do this first

**Background.** PR #120 (`4e7daca61`, on `main`) fixed the node-bridge i64/ptr
error by clearing the register-width map (`reg_value_ll`) at every
`txn_counter = 0` reset. That was too broad: only one of the six reset sites
is a function-body entry; `emit_inline_member` and the reactive drivers reuse
`txn_counter` mid-body. Clearing there dropped widths for live registers and
broke `rbv_list_each_wasm` (`'%ac0' defined with type 'i64' but expected
'i32'`). PR #122 narrows the clear to the callable-txn driver only via
`reset_register_names()` and restores the other five resets to
`txn_counter = 0`.

**Verification before merging (fresh, not from the PR body):**

1. `cargo test --test rbv_list_each_wasm` — the regression PR #120 introduced.
2. `cargo test --test c_driver_node python_node_cross_language_bridge` — the
   bug #120 fixed (node-bridge `%t61` i64/ptr).
3. `cargo test --test daily_use_smoke` (44 tests) — broad smoke.
4. `cargo test --test hir_storage_class_parity --test hir_needs_state_parity` —
   the export-let storage fix must stay intact (this session's work).
5. `cargo test --lib -- backend::tests` — fast backend gate.
6. `bash scripts/gate.sh quick` — full per-commit gate.
7. `bash benchmarks/ll_determinism_check.sh --quick` — backend-touching change
   requires the determinism gate.
8. `cargo test --test glue_gen_parity` — FFI parity must stay byte-identical.

**Exit condition:** all eight green. Then merge #122 to `main`.

## 2. CI status after landing

The last red CI jobs were:

- `Workspace tests` / `Default gates` — red on `rbv_list_each_wasm` (the
  #120 regression). PR #122 fixes this.
- `Native (accel)` — known flaky SIGILL under `-march=native`.
- `python_node_cross_language_bridge` — CI-only node-bridge i64/ptr mismatch.
  #120 fixed this; #122 is the companion.

**After #122 is merged**, watch CI. If `Workspace tests` and `Default gates`
turn green and `Native (accel)` passes, the trunk is clean. If `Native
(accel)` flakes again, that's the known `-march=native` SIGILL, not a new
defect.

## 3. FFI contract: S4 (populate `HirProtocol::ops`)

The FFI plan (`docs/plans/2026-11-16-ffi-contract-and-metaprogramming.md`)
has S0–S3, H1–H4, G1–G5, PY, R1, D1–D6 all landed. S4 is the next step:
`HirProtocol::ops` and `HirProtocol::cross_ops` are currently empty, so the
HIR audit counts each `proto` as one unsupported fact (`unsupported=1
(protocols=1 boundary=0)`). S4 populates `cross_ops` from the cast-lane
data so the audit can certify the protocols are lossless.

**What S4 does:**
- Read the cast-lane dispatch (`emit_expr.rs`, the `LaneKind::ExtCall` arm
  and the protocol cast table) and populate `HirProtocol::ops` with the
  operation entries the protocol actually carries.
- The audit should then report `unsupported=0` for the boundary program.
- Add a test: `proto_is_counted_until_cross_ops_populated` becomes the
  pre-S4 test; the post-S4 test asserts `unsupported=0`.

**Exit condition:** HIR audit on the BUGS.md fixture reports
`unsupported=0 (protocols=0 boundary=0)`; the `protocol_is_counted` test
is replaced by a `protocol_is_certified` test.

## 4. Native-stability queue (open issues)

Three open issues from `#odecode` (2026-11-15):

| Issue | Status | Next step |
|---|---|---|
| #5 — `check` succeeds while `build` fails | Open | The check path stops before normalisation and codegen validation. Fix: run the same codegen validation in `check` mode, or document the gap. |
| #7 — unresolved state/local symbols in outlined/nested control flow | Open | Reproduce with a minimal fixture. The emitter is missing a scope declaration in an outlined path. |
| #8 — `term` in transaction guard flow emits contradictory outputs | Open | The guard-flow path doesn't enforce single-decision termination. Fix: the guard flow must have exactly one `term` per branch. |

**Priority:** #5 is the most user-visible (CI gate passes but `build` fails).
#7 and #8 are emitter correctness issues with unclear scope.

## 5. pf_exp induction (lower priority)

The induction proof for `pf_exp` is tracked but not started. This is a
prover-side task, not a compiler-side task. Defer until the native-stability
queue is clear.

## Recommended sequence

1. **Verify + land PR #122** (Step 1). This is self-contained, unblocks CI,
   and is already written — just needs fresh verification before merging.
2. **Watch CI** after #122 merges (Step 2). If green, the trunk is clean.
3. **S4** (Step 3) — the next FFI-contract step.
4. **Native-stability queue** (Step 4) — #5 first, then #7, then #8.
5. **pf_exp induction** (Step 5) — last.
