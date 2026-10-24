# Fullstack application validation campaign — Briev Task Board

**Date:** 2026-10-23
**Status:** ACTIVE — A6a hardening complete; A6b blocked on webstack collection elements.
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

Landed first and committed as `f5998fa7`:

- one-shot HTTP server;
- `GET /health` exact JSON;
- integration gate.

### A2/A3 — in-memory CRUD and sequential loop (2026-10-23)

Landed:

- sequential accept loop with one request per connection;
- `Task` / `TaskStore` objects and state across requests;
- API:
  - `GET /health`;
  - `GET /api/tasks`;
  - `POST /api/tasks`;
  - `POST /api/tasks/:id/toggle`;
  - `DELETE /api/tasks/:id`;
- JSON request parsing and JSON response serialization;
- structured 400/404 responses;
- bounded one-byte read loop with 4 KiB request ceiling.

Gate result: `task board: sequential loop PASS`.

The gate exercises health, empty list, create, persistence across requests,
toggle, invalid body, unknown route, and delete.

### Strict-protocol discoveries

1. **AUTHORING ERROR** — A1 initially returned only a count while advancing a
   value-typed `ByteBuf`; corrected to return the advanced buffer.
2. **SPEC VIOLATION — FIXED** — nested enum payload construction/match layouts
   disagreed, so nested match returned the no-match default. Fixed in the
   compiler with regression fixture
   `tests/tier1/nested_enum_payload_match.bv`.
3. **IMPLEMENTATION BUG — FIXED** — String-concat cleanup blocks inside a
   nested match arm left a stale phi predecessor. Fixed in the backend with
   `tests/tier1/string_match_cleanup_phi.bv`.
4. **STDLIB BUG — FIXED** — `json_get` on an empty object dereferenced an
   uninitialized result because `find_pair` never ran. `handle_object` now
   returns `Err` for the empty-object boundary, pinned by
   `tests/tier1/json_empty_object_lookup.bv`.

No app workaround was added for any compiler/stdlib defect.

## A4 — frontend read-only vertical (2026-10-23)

Landed:

- `frontend/task_board.rbv`;
- reactive `tasks_json`, `status`, and `loaded` state;
- real host `fetch_url` boundary;
- visible Refresh trigger and `b-window:load` wiring;
- generated wasm/html/css/js split artifact.

Gate: `examples/apps/task_board/tests/frontend_gate.sh`.

The gate builds the backend and frontend, seeds one task through the native
API, serves the frontend, then uses real Chromium to click Refresh and assert:

- no page errors;
- no console errors;
- status becomes `ready`;
- exact backend JSON reaches the Briev view state:
  `[{"id":1,"title":"browser read test","done":false}]`.

Web runtime fixes required for the boundary:

- wasm32 host-string returns now have a runtime bump `malloc` import;
- missed `load`/DOMContentLoaded events fire once after async shim boot;
- flush decoders use explicit numeric addressing for wasm32.

Validation: browser gate PASS; web-generator tests 29 green;
`cargo test --lib` 3,303 green.

## Existing failure found

`rbv_gate.sh` is currently blocked by pre-existing wasm32 `obj_init` bad IR
(`i32` value used as `i64` pointer). The pinned pre-A4 compiler reproduces the
same failure; recorded in BUGS.md. It is independent of the frontend gate.

## A5 — frontend mutation vertical (2026-10-23)

Landed:

- method-aware browser HTTP boundary in `lib/std/web/fetch.bv`:
  `http_get`, `http_post`, `http_put`, `http_delete`, plus
  `last_http_status`;
- host implementations in `lib/glue/web/web.js`;
- browser JSON string escaping (`lib/std/web/json.bv`);
- Task Board title input and create action;
- Task ID input and toggle/delete actions;
- backend CORS `OPTIONS` handling;
- real Chromium CRUD gate.

Gate: `examples/apps/task_board/tests/frontend_gate.sh`:

1. loads the page and proves empty GET;
2. fills a title and clicks Create → exact new task JSON;
3. fills ID `1` and clicks Toggle → exact completed JSON;
4. clicks Delete → exact empty list;
5. requires zero page/console errors throughout.

Validation: browser gate PASS; API gate PASS; full `rbv_gate.sh` PASS;
`cargo test --lib` green; determinism quick gate PASS; full runtime suite
all MATCH.

### A5 compiler/runtime defects found and fixed

- **Stateless webstack defns emitted `%state` flushes.** Callable wrappers and
  pure defns have no state parameter. `emit_web_flush_batch` now requires a
  real machine-state transaction, and state GEPs are skipped in stateless
  bodies.
- **wasm32 Int frgn ABI used i64.** Frgn declarations now map platform `Int`
  through `ctx.int_bits`, so browser host functions declare/call `i32` on
  wasm32.
- **String-concat pointer handling used i64.** Operand handles, temp masks,
  mask tags, arena-result narrowing, and free calls now follow the target
  pointer width.
- **Host integer marshalling returned a raw JS Number.** Generated shim
  marshals Briev `Int` through `Number(...) >>> 0` for wasm32.
- **CORS preflight was not handled.** The app backend now answers `OPTIONS`
  before route dispatch.

Regression pins added in `src/backend/llvm/tests.rs` for wasm32 Int frgn ABI
and stateless webstack defn emission.

## A6 — hardening (in progress, 2026-10-23)

### A6a — request framing, limits, and idle-connection budget

Landed:

- explicit 4 KiB request ceiling shared by the read loop and framing check;
- incomplete/truncated requests return structured `400 bad_request` and never
  enter route dispatch;
- over-limit requests return structured `413 payload_too_large`;
- 2-second receive timeout (`SO_RCVTIMEO`) so idle connections cannot pin the
  sequential server;
- server survives framing and idle failures and continues serving the same
  process;
- title length limit (120 chars) with structured `400 title_too_long`;
- frontend maps 400/413 to useful mutation notices.

API gate now covers:

- valid CRUD;
- malformed JSON body;
- overlong title;
- truncated raw connection;
- oversized raw request;
- idle raw connection past the receive budget;
- post-failure health request proving loop survival.

Validation: API gate PASS; browser CRUD gate PASS; conformance sweep green.

### A6b — structured rendering status: BLOCKED

The current webstack `b-each` supports:

- fixed vectors (`[N x T]`);
- generic collections whose element type is `String` or scalar `Int`.

It does **not** support generic collections of struct/obj elements, so
`List<Task>` cannot be iterated with per-item field bindings
(`item.title`, `item.done`). This is a documented webstack follow-up, not a
SPEC violation, but it blocks the planned list rendering.

No backend HTML or app-specific workaround will be added. The raw JSON view
remains until the general element model lands.

## Next

1. Extend collection `b-each` to struct/obj elements with per-item field
   projection, or expose a general dynamic vector view.
2. Then land A6b structured task rendering and UI states.
3. Then A7 PostgreSQL wire protocol.
