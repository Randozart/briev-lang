# Evented net — poll-multiplexed connections, fd linearity, strand facts, chunked read

**Date:** 2026-11-20
**Status:** PLAN (confirmed by owner; PIPELINE phase 3)
**Worktree:** `/scratch/briev-builds/evented-net` (detached at current `origin/main`),
branch `feat/evented-net`, one commit per stage.
**Dogfood driver:** the Knooppunt routing sidecar (`kernel/routing_service.bv`)
is sequential, one-syscall-per-request-byte, fd-unprotected. This plan makes it
evented, cheap and fd-safe — in Briev, verifiably.

## Problem statement

`server_loop.bv` is sequential: one connection is read, answered, and closed
before the next accept; a slow client pins the loop (bounded only by a 2 s
receive budget); a request costs ~200 syscalls (one `recv` per byte); and a
`Socket` is an unprotected `{fd: Int}` — double-close and use-after-close
pass the checker today. Everything needed to fix all four facts already
exists in-tree except the *composition* and two checker facts:

- `os/poll.bv` — full `poll(2)` wrapper (`PollSet`, `poll_wait` syscall 7,
  finite-timeout only, EINTR returned, stale-event clearing) — **never
  exercised by any test**.
- `net/nonblocking.bv` — `sock_tcp_nonblocking` (SOCK_NONBLOCK|CLOEXEC at
  creation) + `sock_send_once`/`sock_recv_once` (MSG_DONTWAIT).
- Segmented task runtime — `spawn` lowers to `__task_<fn>_seg<k>`
  continuations over a pure-Briev round-robin scheduler (async Phase C).
- Task-handle linearity (`task_linear.rs`) — the ownership discipline to copy.
- Effect-scan with call-graph fixpoint (`purity.rs`) — the foundation for a
  strand check.

`server_loop.bv`'s own header names the gap: *"not poll-multiplexed —
`os/poll.bv` exists but nothing composes them yet (the net campaign's own
next step)."*

## Waves

### Wave 1 — `std/net/evented.bv`: the composition module (stdlib only)

New module composing the existing pieces. No compiler changes.

- `EVENTED_EAGAIN = -11`, `EVENTED_EINTR = -4` — the errno spellings, house
  rule: the ABI gets named once.
- `evented_listener(port) -> Result<Socket,Int>` — nonblocking listener via
  `sock_tcp_nonblocking` + bind/listen (reuseaddr as `server_loop` does).
- `evented_register(set, socket, events)` / reuse of `PollSet` (the
  "polling never owns the fds" contract stays: the PollSet holds refs, the
  program owns sockets).
- `evented_wait(set, timeout_ms) -> Int` — `poll_wait` passthrough with the
  EINTR-is-returned contract surfaced in the postcondition domain.
- `evented_recv_once` / `evented_send_once` — `*_once` wrappers with the
  `EAGAIN` result documented in-contract (`[term >= -4096]`-style errno
  domain like the rest of the net family).
- `evented_close` — idempotent, documented close (defensive: sets the local
  copy's fd, but see Wave 2 for the *checked* shape).
- A tier1 fixture `evented_smoke.bv`: two clients, interleaved segments —
  golden pins the interleaving order (deterministic: poll order is fd order,
  the scheduler round-robin is fd-ordered).

**No behavior change to `server_loop`** — it stays for simple apps; evented
is opt-in.

### Wave 2 — fd linearity: `[[linear]]` objects

Extend the ownership machinery to objects:

- New annotation accepted on `obj` declarations: `[[linear]]`
  (`Annotation{name}` on `StructDefinition` — the mechanism exists).
- `lib/std/net/socket.bv` marks `Socket [[linear]]`; `evented.bv`'s
  connection obj likewise.
- New analysis `src/analysis/obj_linear.rs` (sibling of `task_linear.rs`,
  same pipeline call sites): for each binding whose type is a linear obj:
  - assignment moves (old binding becomes a tombstone),
  - a consuming call (a defn whose postcondition/consume-registry says it
    consumes — v1: any call whose *callee name* is in the declared consume
    list of the type: `sock_close` for Socket) kills the binding,
  - any later use of a killed binding = error (use-after-close),
  - scope end with a live, unconsumed linear binding = error
    (fd-leak = compile error),
  - `fork`/copy semantics: passing a linear obj BY VALUE to a defn that does
    not consume it is an error unless the defn returns it (v1: require
    consume-or-return; document).
- Errors are house-style diagnostics (what/why/fix), ERROR level.
- Existing stdlib updated in the same commit: `sock_close` becomes the
  declared consumer; the net family's internal helpers updated to the
  move discipline (mechanical; the compiler enforces it).

### Wave 3 — the io/pure strand check (checker fact, not lint)

Extend `purity.rs`'s effect fixpoint with a **strand classification**:

- A defn is `io` if its body (transitively) reaches a blocking-syscall
  lane: the `sock_*` blocking family (`sock_recv` with no MSG_DONTWAIT),
  `SYS_POLL` with infinite timeout, `Spawn#`, `Wait#` — the registry is a
  named list in one module so the classification is auditable.
- Diagnostics (warning level, opt-in to error via the existing warn-policy
  mechanism): a `spawn`ed segment whose *next segment* (the code after a
  `yield;`) contains a direct `io` call — i.e., a defn that holds the
  round-robin hostage across a scheduling point. The honest framing: "this
  segment blocks the scheduler between yield points; annotate `[[yielding]]`
  or restructure to evented."
- This upgrades #145's lesson: `willreturn` conservatism (#15 routing) can
  cite strand facts once the checker knows them (full convergence stays an
  L1 roadmap item; this wave lands the fact *source*).

### Wave 4 — chunked provable read (G12b)

`std/net/chunked.bv` (or folded into `evented.bv`):

- `recv_into(buf, offset, room) -> Int` — chunked recv into a fixed
  `ByteBuf` region, contract `[offset >= 0 && room >= 1 && offset + room <=
  buf.cap]`, postcondition `[term.len <= buf.cap]` — the shape the sweep
  rejected before is now expressible because the bounds are *arguments*,
  not loop-carried obj state (the exact lesson from the checksum sweep).
- `evented_fill_request(conn, buf, fed) -> HttpParse` — chunked replacement
  for `serve_next_request`'s byte loop: one `recv_into` per poll-readiness
  event. ~200 syscalls/request → ~6.
- A/B test: same HTTP fixtures through both readers, byte-identical
  `HttpParse` results.

## Invariants (each pinned by a named test)

1. **Deterministic event order.** For a fixed registration order and fixed
   simulated readiness, `evented_wait`+round-robin visits connections in
   fd-registration order, every run, both engines.
   → `tests/tier1/evented_smoke.bv` + `.golden` (byte-exact interleaving
   trace; corpus harness asserts it) + `evented_ab_parity` (below).
2. **No lost readiness.** After `poll_wait` reports fd F readable, an
   `evented_recv_once` on F returns > 0 or a named errno (EAGAIN impossible
   for that same edge — the kernel's readiness contract); the module's
   contract documents it and the smoke fixture demonstrates it.
   → `evented_smoke` second section: recv after POLL_IN never returns
   EAGAIN (asserted on the golden path).
3. **EINTR honesty.** `evented_wait` returns `EVENTED_EINTR` when a signal
   arrives; the loop demonstrably retries without losing the PollSet state
   (poll.bv already clears stale events *before* the syscall).
   → `evented_smoke` third section: signal-driven EINTR path (kill(2) to
   self via `SYS_KILL=62`), retry, succeed.
4. **fd safety is static.** Under `[[linear]] Socket`: double-close,
   use-after-close, and scope-end-with-live-socket each fail to COMPILE
   with what/why/fix diagnostics.
   → `tests/tier1/linear_socket_fail*.bv` (three negative fixtures, each
   asserting the exact diagnostic text via the existing negative-fixture
   pattern) — plus `linear_socket_ok.bv` proving the happy path still
   compiles and runs.
5. **Both engines agree on evented semantics.** Every evented fixture runs
   through the interpreter AND natively with byte-identical stdout.
   → the corpus harness already asserts this for every tier1 fixture;
   `evented_*` fixtures join it automatically.
6. **Strand facts are sound.** A defn the strand check calls `io` provably
   reaches a registered blocking lane; a defn it calls pure provably does
   not (fixpoint soundness — false negatives allowed, false positives are
   bugs).
   → `src/analysis` unit tests: table of defns (direct syscall, transitively
   blocking, pure, mixed) × expected strand; plus the diagnostic fixture
   `strand_blocks_scheduler.bv` (warning appears, text pinned).
7. **Chunked read ≡ byte read.** `evented_fill_request` and
   `serve_next_request` produce identical `HttpParse` results for the same
   wire bytes (partial, complete, over-long, and EOF shapes).
   → `evented_ab_parity` in the smoke fixture + the corpus HTTP fixtures
   unchanged (they must stay green through the new reader).
8. **No regression in the existing net family.** `server_loop` semantics,
   the routing-service build, and all existing goldens are untouched.
   → `scripts/gate.sh quick` + full `daily_use_smoke` + the conformance
   sweep (every active source must still check — the sweep caught three
   contract lies in the checksum wave; it gets to vet this one too).

## Named tests

- `tests/tier1/evented_smoke.bv` + `evented_smoke.golden` — the interleaving
  trace, readiness contract, EINTR retry (invariants 1, 2, 3, 5).
- `tests/tier1/linear_socket_ok.bv` — happy-path move/consume/close.
- `tests/tier1/linear_socket_fail_close.bv` — double-close (negative).
- `tests/tier1/linear_socket_fail_use.bv` — use-after-close (negative).
- `tests/tier1/linear_socket_fail_leak.bv` — scope-end live (negative).
- `src/analysis` strand unit tests (invariant 6).
- `strand_blocks_scheduler.bv` diagnostic fixture (invariant 6, wire shape).
- `evented_ab_parity` section in the smoke fixture (invariant 7).
- Existing suites as regression fence (invariant 8): `cargo test -p
  briev-compiler --lib`, `--test daily_use_smoke`, `scripts/gate.sh quick`.

## Non-goals (this plan)

- epoll — poll(7) is enough for the sidecar's connection counts and keeps
  the wave stdlib-only; epoll(232/213) is a follow-up once the composition
  proves out.
- Preemptive threads for request handlers (`os/thread.bv` stays a stub;
  the reactive `on thread <N>` model is untouched).
- Keep-alive/pipelining (server_loop's HTTP/1.1-close posture is retained;
  evented changes *who waits*, not the protocol).
- Making the whole stdlib `[[linear]]` — Socket and the evented connection
  obj only; broader adoption is deliberately left to follow-ups.

## Risks

- **The sweep on `[[linear]]`:** the net family passes `Socket` by value
  internally (e.g. `serve_accept` returns it through several helpers). Wave
  2 must either model "return re-blesses the binding" or update those
  helpers to the move discipline — decided during implementation by what
  the sweep accepts, with the constraint that the ERROR is never silenced,
  only the discipline refined.
- **Determinism of poll across engines:** the interpreter does not run
  real poll(2); its evented fixtures simulate readiness through the
  existing intrinsics model. The smoke fixture is written so the same
  source runs both engines (the corpus harness guarantees this runs, or it
  fails loudly).
- **Strand false positives:** registry-driven (named blocking lanes only),
  warning-level, never blocking a build until promoted — mirroring the G2
  promotion discipline.

## Success criteria

- All named tests exist and pass; both engines byte-identical on evented
  fixtures; the three negative linear fixtures fail compilation with the
  pinned diagnostics; lib suite + smoke + sweep + quick gate green.
- The routing sidecar adopts `evented_fill_request` + the evented loop in
  the Knooppunt tree (adoption commit in knooppunt, built against this
  branch) — the dogfood proves the model the same week it lands.
