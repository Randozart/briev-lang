# Target-Aware Protocol Resolution

**Date:** 2026-07-25
**Status:** Hypothesised feature — no implementation yet

## Summary

Currently `String` resolves to `String<UTF8>` unconditionally regardless of
target platform. This feature would make the default protocol variant
target-dependent: `String` → `String<UTF16>` on Windows, `String<UTF8>` on
Linux, etc.

## Key Design Questions

- How does `config/targets.toml` declare default variants per target?
- How does the frontend pass the target's default variant to the protocol
  graph resolver?
- What happens to hardcoded `#Category<variant>` references when the target
  doesn't support that variant?
- Does this change the GLUE export pipeline (which currently uses TOML config)?

## Dependencies

- Protocol graph (`src/analysis/protocol_graph.rs`) — the BFS can already
  resolve variant-aware paths; this feature just changes which variant is
  the starting node.
- `config/targets.toml` — needs schema for `[target.xxx.default_protocols]`

## See Also

- `docs/architecture/protocol-types.md` — protocol declarations foundation
- `docs/architecture/casting-protocol.md` — protocol graph + variant resolution
