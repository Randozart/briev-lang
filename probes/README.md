# Briev Probes

Exploration / micro-kernel probes recovered from worktree cleanup
(2026-09-16). These are NOT normative examples — several are intentionally
minimal or expect a specific rejection. They are kept to show what Briev
can do at the edges of the compiler.

They live OUTSIDE the conformance-swept roots (`lib/`, `examples/`,
`benchmarks/`, `.smoke/`) because some are intentional-failure probes.

## `rv64/` — capability-kernel phase probes

Scratch probes from the rv64 capability-kernel work (`feat/rv64-capability-kernel`):

- `probe_k.b.bv` — the Phase 4 micro-kernel capability demo: two user-mode
  tasks printing through an `ecall` syscall boundary; CLINT timer; the
  micro-kernel capability showpiece.
- `probe1.b.bv` — `match` statement dispatch on a state value.
- `probe2.b.bv` — `when` guard statement.
- `probe3.b.bv` — `txn` with an inner `when`.
- `probe4.b.bv` — multiple `defn`s mutating shared state.
- `probe5.b.bv` — `node @ 7` machine-vectored address with a `match`.
- `probe6.b.bv` — `node @ 7` with a single-contract variant.
- `probe7.b.bv` — minimal `node @ 7` with a `term`.

## `codegen/` — backend codegen probes

Recovered from the backend-foundation worktree's `tmp_probe/`:

- `eq_ne_codegen.bv` — `==` / `!=` codegen through a `frgn` print boundary.
- `qualified_enum_circt.bv` — qualified enum paths through the CIRCT
  backend. Expects a clean capability rejection (no enums in register-level
  logic), not a panic.