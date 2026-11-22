# Saltpetre op-backend ledger — op-level compat with a retirement gate (2026-11-22)

Owner direction (2026-11-22): **op-level**, not model-level. Saltpetre's op
surface is the compat layer today; `.abv`-emitted codegen is what each op
retires toward. Do not pick a lane — adopt a per-op retirement ledger as the
standing mechanism.

Research (read first): `.opencode/plans/2026-11-22-saltpetre-op-surface-research.md`.

## What this plan is NOT

- **Not** a control-flow rewrite of `run_node`. That function carries a
  bit-exact ORT-golden conformance gate (`tests/saltpetre_conformance.rs`) and
  1,268 lines; restructuring it for aesthetics risks the only correctness
  instrument the op surface has. The `when node.op_type == "..."` chain **is**
  the routing table — one arm per op. What is missing is *provenance on the
  table*, not a different table.
- **Not** a compiler change. Zero Rust production code changes; the only Rust
  added is a test harness.
- **Not** model-level (OpenVINO graph forwarding). That stays an escape hatch
  in `lib/std/ai/openvino.bv`, untouched.
- **Not** the device graduation work (P3). That needs the PTL node and is its
  own session; this plan builds the mechanism that makes it measurable and
  prevents an unretired delegation from ever shipping.

## The mechanism

`lib/saltpetre/op-backends.dbvl` — one row per dispatched op:

```
op: Gemm; backend: briev; impl: gemm; graduation: none;
op: Conv; backend: briev; impl: conv2d; graduation: none;
```

`backend` is one of:
- `briev` — served by Briev code today (scalar CPU now; Briev-emitted later).
- `ffi(<lib>)` — delegated to a vendor op-level lib (cuBLAS, cuDNN, oneDNN).
  **Requires a non-empty `graduation`** naming the op/shape/measurement that
  retires it.
- `model(<lib>)` — delegated to a model-level runtime (OpenVINO). Same
  graduation requirement.

`graduation` is the retirement condition: the exact A/B that flips this row to
`briev` and deletes the delegated arm. An `ffi`/`model` row with `graduation:
none;` is a build failure.

This is the `idiom_ledger_gate.rs` pattern (§12 gates 4–5) lifted from idiom
matchers to op backends: adding a delegation without recording its retirement
is a test failure, not a review note.

## Invariants

1. **Every dispatched op has a ledger row.** The set of
   `when node.op_type == "X"` arms in `lib/saltpetre/exec.bv` minus the
   non-dispatch terminal (`unsupported ONNX op`) equals the ledger's op set.
   Pinned by `saltpetre_op_ledger::every_dispatched_op_has_a_row`.
2. **No orphan rows.** Every ledger op is dispatched in source. (Bidirectional
   with 1; without it a row could invent coverage.) Pinned by
   `saltpetre_op_ledger::no_orphan_ledger_rows`.
3. **No delegation without a retirement condition.** Every row whose backend is
   `ffi(…)` or `model(…)` has a `graduation` that is not `none`. Pinned by
   `saltpetre_op_ledger::delegated_ops_name_their_graduation`.
4. **A `briev` row cannot secretly delegate.** For every `briev` row,
   `lib/saltpetre/exec.bv` contains no `frgn` and no `#Link` reference. This is
   the one that makes the ledger truthful rather than decorative. Pinned by
   `saltpetre_op_ledger::briev_rows_are_ffi_free`.
5. **The ledger is well-formed.** Rows parse, ops are unique, `backend` is one
   of the three forms, `impl` names a `defn` that exists in `lib/saltpetre/`.
   Pinned by `saltpetre_op_ledger::ledger_parses_and_impls_exist`.
6. **Op-surface behaviour is unchanged.** No conformance fixture changes
   verdict. Pinned by `tests/saltpetre_conformance.rs` (6 fixtures,
   ORT-golden bit-for-bit) staying green — the existing gate, not a new one.
7. **Generic helpers are defined exactly once.** Every helper this plan
   extracts has one `defn` across `lib/std/**` + `lib/saltpetre/**`. Pinned by
   `saltpetre_op_ledger::generic_helpers_are_single_definition`.
8. **The census ratchets forward only.** `lib/saltpetre/op-census.txt` records
   `briev=<n> delegated=<n> total=<n>`; the gate fails if `briev` DECREASES or
   `total` changes without the fixture set changing. A retirement can only
   improve the number. Pinned by `saltpetre_op_ledger::census_only_improves`
   (mirrors `harness/memcheck_ratchet.sh`'s direction rules).
9. **The gate rejects what it claims to reject.** A synthetic ledger with an
   ungraduated `ffi` row, an orphan row, and a duplicate op each fail the
   parsing/validation function. Pinned by
   `saltpetre_op_ledger::gate_rejects_bad_ledgers` (in-test fixtures — the
   gate must be observed failing, per the canary discipline).

## Named tests

| test | invariant |
|---|---|
| `saltpetre_op_ledger::every_dispatched_op_has_a_row` | 1 |
| `saltpetre_op_ledger::no_orphan_ledger_rows` | 2 |
| `saltpetre_op_ledger::delegated_ops_name_their_graduation` | 3 |
| `saltpetre_op_ledger::briev_rows_are_ffi_free` | 4 |
| `saltpetre_op_ledger::ledger_parses_and_impls_exist` | 5 |
| `tests/saltpetre_conformance.rs::saltpetre_tiny_corpus_matches_golden_bits` | 6 |
| `saltpetre_op_ledger::generic_helpers_are_single_definition` | 7 |
| `saltpetre_op_ledger::census_only_improves` | 8 |
| `saltpetre_op_ledger::gate_rejects_bad_ledgers` | 9 |

## Steps

1. **S1 — ledger + census.** Write `lib/saltpetre/op-backends.dbvl` with all 30
   dispatched ops (Sign, Relu, Mul, Sub, Div, Pow, Add, Conv, Gemm, MaxPool,
   ReduceMean, GlobalAveragePool, Identity, QuantizeLinear, DequantizeLinear,
   Pad, Concat, Split, Gather, Slice, Unsqueeze, Squeeze, Sqrt, Tanh, Sigmoid,
   Transpose, Resize, Reshape, BatchNormalization, Flatten), all `backend:
   briev`, each `impl:` naming its existing handler, `graduation: none` (a
   `briev` row does not need one). Write `lib/saltpetre/op-census.txt` =
   `briev=30 delegated=0 total=30`.
2. **S2 — extract generic helpers to `lib/std`.** Move the `Tensor`-free leaves
   out of saltpetre, with saltpetre importing them: `dims_product`,
   `strides_of`, `round_even`, `pow_float` → `lib/std/tensor_util.bv`;
   `f32_from_bytes`, `int_from_bytes_le` → `lib/std/encoding.bv` (which already
   owns byte encodings — verify no duplicate before adding). Keep every
   contract and body byte-identical; only `import` lines change in saltpetre.
   Conformance must stay green after this step alone.
3. **S3 — the gate.** `tests/saltpetre_op_ledger.rs`, modelled on
   `tests/idiom_ledger_gate.rs` (textual, no parser dependency, ledger + source
   are the contract). Implements invariants 1–5, 7–9. Prints
   `op-backend census: briev=N delegated=N total=N` on success so the number is
   always visible in test output.
4. **S4 — verify + commit.** `cargo test --lib -- saltpetre_op_ledger`;
   `cargo test --test saltpetre_conformance`; `bash scripts/gate.sh quick`;
   commit per step (S1, S2, S3 separately). Land per the lane protocol
   (fetch → ancestor check → `push origin HEAD:main` → push branch).

## Documented boundary (why this is doctrine-clean)

`docs/architecture/abv-gpu-doctrine.md` §1.3 and
`docs/plans/2026-11-05-intel-xpu-openvino-harvest.md` §1 guardrail 1 both forbid
**the compiler** from delegating ("FFI lanes exist for users; they are not the
codegen path"). This plan touches no compiler code and adds no vendor runtime to
any kernel path. Saltpetre is a library; a library FFI arm is the documented
escape hatch, and invariant 3 is what keeps it temporary. Invariant 4 is what
stops a `briev` row from being a lie.

## Deferred (own sessions, with the ledger as the entry point)

- **P3 — first graduation**: `Gemm` (already has `matmul!`/`matmul_f16!` and
  `attr declared_kind: matmul`). A/B on the measured geometry with the
  same-geometry correctness gate, on PTL; flip the row, add the bench.
  Candidate order after that: `Conv` (`conv2d`), `MaxPool`.
- **Op-level vendor bindings** (cuBLAS / cuDNN / oneDNN): a `.bv` module per
  lib with `frgn … from #Link<…>`, one op at a time, each with a ledger row
  whose `graduation` names its retirement. `lib/std/ai/openvino.bv` is the
  template (24 frgn, handle lifetime documented, gate test in
  `tests/openvino_gate.rs`).
- **Autograd** — explicitly out of scope; a separate conversation.
