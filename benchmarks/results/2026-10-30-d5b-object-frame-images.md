# D5b proven object frame images — 2026-10-30

Branch: `feat/generalized-value-destination-ownership`
Current: D5a facts + alias-wide non-escaping object frame images
Parent: `c0ac468b` (D5a facts, boxed object literals)

## Safety gate

- Object literals use `alloca` only when every observed object value of that
  owner/type has no escape.
- Any object construction inside `foreach` records `LoopLifetime`.
- `&object` records the new explicit `AddressEscape`.
- Unrecognized call positions remain escapes from D4a.
- ASan on `ws_frame` with `detect_stack_use_after_return=1`: exit 0,
  output `0016384`.
- Backend IR test: proven literal emits frame image and no matching malloc.

## Full runtime A/B

Same machine, immediate parent worktree, harness timings (all outputs MATCH).

| Benchmark | Parent (s) | Current (s) | Direction |
|---|---:|---:|---|
| ws_frame | 0.0157 | 0.0136 | current 1.15x faster; now 0.93x C |
| recommender | 0.0156 | 0.0127 | current faster |
| arena_churn | 0.0189 | 0.0203 | within run variation |
| queue_drain | 0.0174 | 0.0202 | investigate if reproduced |
| queue_drain_sym | 0.0236 | 0.0170 | current faster |
| arena_bump | 0.3176 | 0.3195 | within run variation |
| accumulator_flush | 0.1616 | 0.1603 | tie |
| telemetry_stream | 0.1652 | 0.1660 | tie |

No reproduced material regression. The `ws_frame` win is direct: its header
object now uses a frame image where proven, in addition to the enum-payload and
caller-destination paths from D4.
