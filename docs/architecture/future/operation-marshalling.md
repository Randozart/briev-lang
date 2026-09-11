# Operation Marshalling at Compile Time

**Date:** 2026-07-25
**Status:** Hypothesised feature — no implementation yet

## Summary

Instead of marshalling data at runtime (like every other language does at FFI
boundaries), Briev would marshal operations at compile time — adapting the
program's abstract operations to the target platform's native ABI without
changing source code.

Currently the compiler resolves protocol variants to concrete types per-target,
but the operations themselves (Add, Length, etc.) are emitted as the same LLVM
IR regardless of target. This feature would let the backend select different
operation implementations per target based on the resolved protocol variant.

## Key Design Questions

- How does the backend select per-variant operation implementations?
- Can the backend emit different LLVM IR for the same `op Add` depending on
  whether the target protocol is `String<UTF8>` vs `String<UTF16>`?
- How does this interact with the GLUE bridge generator?
- What's the distinction between "marshalling operations" (compile-time) and
  "operation dispatch" (runtime)?

## Dependencies

- Protocol graph (`src/analysis/protocol_graph.rs`) — provides the variant
  resolution that drives operation selection.
- Backend dispatch (`src/backend/llvm/intrinsics.rs`) — where per-operation
  code emission happens; this is where target-aware selection would plug in.

## See Also

- `docs/architecture/protocol-types.md` — protocol declarations foundation
- `docs/architecture/casting-protocol.md` — protocol graph + variant resolution
- `docs/plan/2026-07-23-extensible-protocol-declarations.md` — protocol foundations
