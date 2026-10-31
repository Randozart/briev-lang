# D4b inline enum-object payload — rollback 2026-10-31

## Defect

`net_stack_helpers` miscompiled when an object payload (`WsFrame`) was laid out
inside an internal enum image.

- `-O0`: correct
- `-O1`: missing/failed websocket checks
- `-O2`: correct in the final reproduction
- `-O3 -flto`: correct in the final reproduction
- earlier O2/O3 builds also corrupted `HttpParse`/`WsFrame` fields

The failure followed the object pointer through an i64 handle round-trip
(`%object_out` / enum payload). ASan did not report an invalid access; the
optimizer produced stale field values and an oversized payload allocation.

## Action

`OBJECT_PAYLOAD_INLINE_ENABLED = false` in `src/analysis/enum_layout.rs`.
Object payloads stay boxed. D4c object caller destinations remain enabled only
for direct non-retaining consumers; the enum-payload copy is no longer accepted
as a non-retention proof while D4b is off.

## Evidence after rollback

- `net_stack_helpers`: correct at O0/O1/O2/O3.
- ASan with `detect_stack_use_after_return=1`: correct.
- daily-use suite: pass.
- full runtime suite: all outputs MATCH.
- ws_frame: 1.04x C in the rollback run (the old inline-enum D4b timing is
  retained as historical evidence but is not a valid performance target until
  the lifetime proof is rebuilt).
