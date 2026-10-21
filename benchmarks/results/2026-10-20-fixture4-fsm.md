# Stdlib fixture campaign, fixture 4 — TCP-subset FSM: first results

**Date:** 2026-10-20
**Commit:** provenance fix `9f18bcb7` + FSM fixture (this commit)
**Status:** Fixture 4 first pass COMPLETE (module, runtime pin, benchmark, C
reference). One real performance gap recorded, not hidden.

## What was built

- `lib/std/fsm.bv` — closed enums `TcpState`/`TcpEvent`/`FsmError`, exhaustive
  nested `match`, `Result<TcpState, FsmError>` edges, integer-code boundary
  for the stable import surface, and the `fsm_run` event-stream driver.
- `tests/tier1/fsm_smoke.bv` + `fsm_smoke_golden_output` — legal handshake /
  teardown path plus an illegal edge; golden `0|1|3|4|5|6|10|1`.
- `benchmarks/fsm_dispatch.bv` + `benchmarks/fsm_dispatch_c.c` — deterministic
  `i % 7` event stream, same TCP-subset table; harness TAG `runtime`.

## Correctness

Compiled Briev and C agree byte-for-byte:

```
BOUND=5  -> 2000008
BOUND=50M -> 2000008
```

The fixture also exercised the open enum-import surface: named imports of enum
types (`import { TcpState } from "std/fsm"`) are not yet accepted, so the
fixture uses an explicit integer-code boundary around the enum-typed core.
That gap is logged separately; it is not evidence about enum/match lowering.

## Performance — the campaign's finding

First measurement used the integer-code wrapper **inside** the hot loop
(`tcp_step_code` per iteration): Briev 1.7962s vs C 0.2405s, **7.47x**.
Refactoring the driver to keep the loop enum-typed (`fsm_run` in
`lib/std/fsm.bv`, one wrapper call at the boundary) gives:

| Metric | Value |
|---|---|
| Briev | 0.6921s |
| C | 0.2451s |
| Ratio | **2.82x (C wins)** |
| Correctness | MATCH (`2000008`) |

The remaining gap is structural: `Result`/enum returns are materialized as
heap boxes. The emitted `@fsm_run_loop` IR shows a `call @tcp_step`, then
`inttoptr`/`getelementptr`/`load` on the returned box for the tag and payload —
per iteration — while the C reference returns a scalar `int`. The event decode
match adds a second dispatch. This is a compiler representation/codegen gap
(enum/Result unboxing, scalar replacement of the transition result), not a
source trick or a timer artifact.

**Not accepted as normal.** Follow-up: a general unboxed-enum/Result codegen
path (or an ABI that returns small sum types in registers) so an enum-typed
FSM can reach the C switch's shape without a strategy keyword. Recorded as an
open performance discovery, not patched around in the fixture.

## Gates

- `cargo test --lib` (run at the structural-provenance commit; FSM fixture
  re-run in the fixture commit).
- `fsm_smoke_golden_output` PASS.
- `fsm_dispatch` harness row MATCH.
- Determinism/parity remain green at the provenance commit; rerun after the
  fixture commit as usual.


## Amendment 2026-10-20 (P1-3)

Named enum-type imports now work: `fsm_smoke` uses `TcpEvent.PassiveOpen` etc.
directly (no integer-code boundary in the runtime test). The benchmark still
uses `fsm_run` with an event-index decode for a deterministic stream; the
2.86x measurement and the heap-boxed-Result finding are unchanged.
