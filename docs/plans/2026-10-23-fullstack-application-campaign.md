# Fullstack application validation campaign — Briev Task Board

**Date:** 2026-10-23
**Status:** ACTIVE — A1 `/health` vertical complete.
**Base:** `main` at `45b8850a`.
**Umbrella:** `docs/plans/2026-10-09-fullstack-stdlib-and-net-stack.md`.

## Purpose

Build a real application to validate Briev outside benchmark-shaped kernels.
The app is a task board with a native `.bv` HTTP backend and an `.rbv`
frontend. The compiler and stdlib are the deliverables; the app is the test
vehicle.

This campaign is app-driven discovery. It does not replace the benchmark
campaign, and it does not treat the current benchmark gaps as closed.

## Strict spec-conformance rule

If behavior is specified by `spec/SPEC.md` or a normative example, and the
implementation does not follow it, the affected slice stops immediately.

The failure is reported to the owner with:

1. SPEC section or normative example;
2. expected behavior;
3. actual behavior;
4. minimal Briev repro;
5. interpreter/backend divergence, if any;
6. proposed fix, if obvious.

No app workaround, stdlib shim, weakened contract, changed test, or silent
spec reinterpretation is allowed.

Failure classification:

- **SPEC VIOLATION** — spec says X, implementation does Y.
- **SPEC GAP** — expected behavior is not specified.
- **IMPLEMENTATION BUG** — spec is clear; implementation is wrong.
- **AUTHORING ERROR** — application misused the language.
- **SPEC AMBIGUITY** — owner decision required.

Only an **AUTHORING ERROR** is fixed in app code. Everything else is escalated
or fixed in the owning layer with a regression pin.

## Product scope v1

Briev Task Board, single user, local server:

- create a task;
- list tasks;
- toggle completion;
- delete a task;
- browser UI;
- JSON API;
- in-memory persistence;
- PostgreSQL persistence as a later phase.

Explicit v1 exclusions:

- authentication;
- multi-user access;
- TLS;
- deployment;
- real OS threads;
- horizontal scaling.

The server is sequential/blocking. This is honest current scope; Briev has no
real OS-thread surface yet.

## App layout

```text
examples/apps/task_board/
  README.md
  backend/
    main.bv
  frontend/
    task_board.rbv
  tests/
    api_gate.sh
```

App code stays out of `benchmarks/`.

## Existing foundation

Already landed on `main`:

- L1 byte buffers: `lib/std/net/bytes.bv`;
- L2 TCP sockets: `lib/std/net/socket.bv`;
- L3 HTTP parsing/response writing: `lib/std/net/http.bv`;
- WebSocket codec: `lib/std/net/websocket.bv`;
- JSON parser: `lib/std/json.bv`;
- frontend router/DOM surface: `lib/std/web/*`.

Known missing for the first app:

- complete backend request loop;
- application store and routes;
- frontend mutation fetch (`GET` only today);
- PostgreSQL wire client.

## Slices

### A0 — campaign charter

- plan and app README;
- strict spec protocol recorded;
- directory scaffold;
- no functional claims.

### A1 — backend `/health`

Scope:

- bind/listen on `127.0.0.1`;
- accept one HTTP connection;
- read incrementally until `http_parse_request` reports complete;
- route `GET /health`;
- return JSON `{"ok":true}`;
- close the connection;
- exit cleanly.

This is deliberately one request, not the final server. It proves the boundary
without pretending to have a server loop.

Gate:

```bash
bash examples/apps/task_board/tests/api_gate.sh
```

Requirements:

- build with `brievc`;
- start backend on an isolated test port;
- poll for readiness;
- issue `GET /health`;
- assert exact response status line and body;
- kill/await backend;
- fail on mismatch, stderr output, or timeout.

### A2 — in-memory task API

- `Task` and `TaskStore` objects;
- create/list/toggle/delete;
- JSON request parsing and response writing;
- structured errors;
- exact curl gate for all endpoints;
- malformed-request survival.

### A3 — server request loop

- sequential accept loop;
- bounded request size;
- request timeout/lifetime policy;
- clean shutdown policy;
- malformed-connection recovery.

This slice may expose an observability/liveness issue: a long-running accept
loop must not be folded away merely because it has no program-local output.
Socket writes are observable effects.

### A4 — frontend read-only

- compile `.rbv` task list UI;
- fetch `GET /api/tasks`;
- render list and empty/error states;
- keep backend API golden.

### A5 — frontend mutation fetch

Extend the frontend boundary:

- `fetch_method`;
- `http_get`/`http_post`/`http_put`/`http_delete`;
- typed errors where possible;
- browser CRUD actions.

### A6 — task app hardening

- JSON error responses;
- route/method errors;
- request-size limits;
- title validation and escaping;
- deterministic ordering;
- WebSocket live updates only after CRUD is stable.

### A7 — PostgreSQL wire client

Implement `lib/std/db/pg.bv` in this order:

1. pure frame encode/decode;
2. offline golden protocol corpus;
3. live `connect + SELECT 1`;
4. simple query;
5. parameterized query;
6. task persistence.

V1 auth: trust or MD5 only. SCRAM is explicitly deferred.

## Validation policy

Every slice must pass:

- `brievc check`;
- app gate;
- `cargo test --lib` when compiler/stdlib changes;
- full runtime suite when codegen changes;
- determinism quick gate when backend changes;
- parity corpora when frontend changes;
- Praetor on changed directories.

Every discovered failure is classified using the strict protocol above.
Historical result files are never rewritten.

## Result

### A1 — backend `/health` (2026-10-23)

Landed in `examples/apps/task_board`:

- backend binds `0.0.0.0:18367`, accepts one HTTP connection, reads bytes
  incrementally, parses the request, routes `GET /health`, sends JSON, closes,
  and exits;
- non-health routes return a structured JSON 404;
- `tests/api_gate.sh` builds/checks through `brievc`, starts the backend on the
  isolated test port, polls readiness, asserts status line, content type, and
  exact body, and fails on stderr/timeout/mismatch.

Gate result: `task board: /health PASS`.

Discovery during A1:

- **AUTHORING ERROR** — the first read loop returned only a count while
  advancing a value-typed `ByteBuf`. The outer binding kept `len == 0`, so
  routing never ran. Corrected the application to return the advanced buffer.
  No compiler or stdlib workaround was introduced.
- No SPEC violation was established in this slice.

Next: A2/A3 — in-memory task store plus the sequential accept/request loop.
