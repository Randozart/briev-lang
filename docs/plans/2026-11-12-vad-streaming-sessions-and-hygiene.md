# VAD streaming sessions + record hygiene (2026-11-12)

## Phase 1 — record hygiene

1. Close the 2026-11-10 BUGS entry (`List<Float> <- call()` sitofp type
   error): call-RHS repro, confirm type-valid + value-correct IR post
   `eecf9d66`, close with root-cause pointer; sweep for other
   strategy-convention references.
2. Rule-5 check: `f32list` repro through the interpreter (expected correct
   → pure-codegen bug, no divergence).
3. INDEX/progress queue refresh (VAD RPC landed); saltpetre.md gains the
   canonical op-ledger table (op → tiny fixture → gate).

## Phase 2 — VAD streaming sessions

Daemon-side session table so the native daemon matches the reference's
streaming shape:

- `vad_open {chunk_len?}` → `{"session": id}` — id = counter int; table =
  global lists (ids + owned state tensors), capped at 8 sessions.
- `vad_feed {session, pcm_path}` → `{"count": N, "probs": [...]}` — chunks
  the file through the session's carried state; state persists server-side
  across requests (multi-feed accumulates).
- `vad_close {session}` → `{}` — destroys the session state (tensor_drop +
  slot free). Unknown id / table full → explicit error JSON.

Library touch: split the streaming loop into `vad_feed_chunk(graph, state,
bytes, offset, chunk_len)` — the session table drives the loop across
requests. `vad_chunks` (single-shot) stays, built on the same step fn.

Session state is persistent → allocated OUTSIDE the request coll arena
(persistent-state rule). Feed remainder truncates at chunk_len (matches the
standalone binary). Sequential serve loop makes the table safe by
construction.

Gates: new `harness/vad_sessions_smoke.sh` — open → two feeds (2 s + 4 s of
the 6 s fixture) → close, asserting concatenated probs match the committed
187-chunk golden within tolerance (streaming equivalence: fed-in-parts ==
fed-whole); existing vad_rpc/native/ML/dashboard/soak suite; 3789 lib +
conformance. CI wiring next to vad_rpc_smoke.

## Outcome (2026-11-12)

Memcheck owned tier landed: `brievc memcheck [--deny-memory-ambiguity]
[--stdlib-path <dir>] <file.bv>` prints, per program, the releases the
owned-storage pass inserted and the conservative drops with REASONS
(alias / escape-via-callee / consumed-by-callee / keep / explicit-free /
state-store / return-borrow), plus the undecided-owner census with the
freshness gap named (`call X not proven fresh` / `expression not proven
fresh`). `--deny-memory-ambiguity` fails when the census is non-empty —
that is the kiosk-CI enforcement point once the daemon's census reaches
zero. Freshness summaries gained two derivations en route: String-slice
freshness (slicing doc) and unwrap-shaped Match identity (`Ok(v) => v`,
`Err(_) => { trap; }`), so `unwrap(fresh)` stays fresh across the Result
boundary.

Daemon census (honest worklist, not noise): 16 releases + 147 undecided.
The undecideds are dominated by conditional-ownership families — helpers
with literal-return branches (`read_file` Err strings, `face_param` empty
returns, `b64_char` alphabet literals, `get_env` empty) flowing through
`unwrap` into callers, plus loop/match builders the summaries cannot yet
discharge. The fix is the §13 response-builder copy discipline applied
across stdlib (`owned_literal`-style fresh returns) — tracked as the
follow-up; `--deny-memory-ambiguity` wiring into kiosk CI happens after
that census reaches zero. Nine owned-storage tests (events for release
kinds, keep/alias/undecided).

## Outcome 2 (2026-11-13): Ok-side freshness + honest census

Family 1 became a DERIVATION, not discipline: `ok_fresh` — a
Result-returning callable discharges when every return path is `Ok(fresh)`
or `Err(anything)` (Err payloads are irrelevant to ownership; `unwrap` only
yields the Ok side). Zero stdlib edits, zero behavior change, no cold-path
allocations. Supporting derivations: scalar-literal freshness, per-item
List-literal freshness (the wrap-stasher guard — `Ok([state])` cannot
discharge), Result locals carry freshness into `unwrap`.

The stricter per-item guard surfaced a REAL finding: `run_node` (the
executor) returns `Ok(y)` where `y = tensor_rename(x)` — renamed aliases of
the model inputs stored into the result. The pass now correctly refuses
ownership for the env lists: releases 16 → 14 (the two withdrawn frees were
the documented alias-leak class), undecided 147 → 159. The census is now
dominated by that single sound frontier: env/Tensor handle ownership (the
owned-Tensor-handle design), not derivable gaps.

Also landed: `Ok`/`Err` payload transfers no longer log as arg-escape
drops (they are the function's result); `--deny-memory-ambiguity` stays
unwired in kiosk CI until the env-handle frontier lands. Gates: 3792 lib,
conformance, kiosk native faces / vad_rpc golden / sessions equivalence /
dashboard IR verify.

## Outcome 3 (2026-11-13): list-BLOCK ownership separated from element ownership

The §Outcome-2 per-item List guard was withdrawn as unsound-for-the-question:
a List literal always allocates a fresh BLOCK (coll object + data), and
freeing it (`coll_destroy`) never frees an element's own allocation — element
handles are plain words inside the block. List-block ownership is therefore
independent of element ownership, and the wrap-stasher hazard
(`wrap(s) -> [s]`) is an ARGUMENT-side concern already covered by the escape
gate (`vars[param] == false`). Restoring unconditional List-literal
freshness re-discharged the executor chains (env lists owned again:
releases 14 → 16, undecided 159 → 147), with zero behavior change and no
crash: native faces, VAD sessions equivalence, 3792 lib green; 300-call soak
flat after the allocator staging step (181 MB @300, HWM 237 — within the
characterised variance).
