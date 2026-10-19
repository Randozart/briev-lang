# PR review — Otto's four open PRs + net-stack reconciliation

**Date:** 2026-10-20
**Status:** ACTIVE — Phase 1 (merge #25/#23/#24) approved; #26 held for
reconciliation (Phase 2).

## The four open PRs (all `odecode`, none with CI checks)

| PR | Change | Verdict |
|---|---|---|
| #25 | lexer: decode `\r` in regular-string/char lanes (`"\r\n"` lexed as four literal chars; byte-string lane already correct) | MERGE — small, correct, prerequisite for #26's CRLF parsing |
| #23 | backend: `const N: String = "lit"` emitted `@N = constant ptr null` → every read segfaulted; interns the literal + aliases `@str.N` | MERGE — real bug; touches `src/backend/llvm/mod.rs` (different region than the session's harvester work) |
| #24 | analysis: dot-call to a stateful obj member lost `%state` propagation (`sink.put_bytes(...)` → undefined `%state` in inlined bodies) | MERGE — real bug; adjacent to planned workstream C (obj/state) |
| #26 | HTTP/1.1 server + websocket framing stdlib (`http.bv` 464, `websocket.bv` 240, `sha1.bv` 326, `base64.bv` 112 — pure Briev over L1/L2) | HOLD — reconciliation required (below) |

All four append to BUGS.md → expect trivial append conflicts.

## #26 blockers

1. **`WsFrame` type collision**: `websocket.bv` defines `obj WsFrame`; fixture
   1's codec in `bytes.bv` defines the same name. Any program importing both
   gets duplicate declarations.
2. **Zero tests** for 1,142 lines (no unit/parity/C pins; no CI on branch).
3. **Contract style = the migration census's noise class** (`[x >= 0]`,
   `[data_off >= 0 && plen >= 0]`, `[true][CharCount#(term) >= 0]`) — compiles
   under `policy: allow`, becomes unproven call sites at the `deny` flip.

## Codec shape comparison (owner asked for the absolute best shape)

| Dimension | Ours (`bytes.bv`) | His (`websocket.bv`) |
|---|---|---|
| Decode | `ws_decode(buf, off)` offset-based, **zero-copy**; payload via `ws_payload_byte` | `ws_decode_frame(buf, len)` buffer-start only; **copies payload** to a fresh String per frame |
| Hot path | 14MB corpus, no per-frame alloc (1.58x vs C) | per-frame allocation — wrong for the benchmark |
| Rigor | RFC minimality + top-bit rejection (`Err("protocol")`), bounds-proven | no minimality/top-bit checks; `(-1, 0)` sentinel |
| Ergonomics | header-only; caller walks payload | `payload: String` + `consumed` — ideal for the app layer |
| Direction | masked + unmasked | server-oriented |

**Decision: UNIFY in `lib/std/net/websocket.bv`** — our bounds-proven core
(moved out of `bytes.bv`, which keeps the L1 primitives `get_u64_be`/
`put_u64_be`) plus his API rebuilt as conveniences:
- `ws_message(buf, off) -> WsMessage { status, fin, opcode, payload: String,
  consumed }` (NeedMore/Complete; copy+unmask via `ws_payload_byte`),
- `ws_encode_text(payload: String) -> GrowBuf`,
- his `WsFrame` absorbed into `WsMessage` — collision gone, one decoder,
  both call sites served.
Adopting his outright would retire the validated hot-path surface; keeping
both leaves two RFC 6455 decoders to patch forever.

## Phases

**Phase 1 — merge the three (approved):** #25 → #23 → #24. After each:
`cargo test --lib`, determinism quick gate, add a regression pin where the
PR lacks one. No CI exists on the branches — a red suite is stop-and-
diagnose, never fix-forward. Push completes the PRs.

**Phase 2 — #26 reconciliation (held):**
1. Land `http.bv`, `sha1.bv`, `base64.bv`; add pins — canonical vectors
   (`dGhlIHNhbXBsZSBub25jZQ==` → `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=` for the
   accept key; RFC base64/sha1 known answers) + interpreter parity.
2. Build the unified `websocket.bv` (move core, add conveniences, absorb
   the type); re-point fixture-1 imports.
3. Extend `ws_frame_codec_parity` + the harness round-trip over the
   convenience API; fold his contract conjuncts into the migration ledger.

**Phase 3 — resume the contract-tiers plan:** finish the `prove_conjunct`
Praetor split, then workstream C (plain/state obj-globals, now on top of
#23/#24), then the migration buckets.
