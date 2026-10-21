# Full-stack web app: Briev net stack + stdlib build-out (2026-10-09)

**Status:** ACTIVE - planning. Branch: feat/fullstack-webapp-demo.
**Companion app spec:** .kiro/specs/fullstack-webapp/design.md (the demo app that
drives this work; the app is a vehicle, the stdlib/compiler work below is the goal).

## Purpose

Build a full-stack web application entirely in Briev - frontend (.rbv), backend
(.bv native server), and a real database connection (PostgreSQL over its wire
protocol) - with NO other languages (no Python, no Node, no C library for the DB).
The app is small on purpose; the point is to grow the standard library and surface
compiler bugs. Every capability the app needs that the stdlib lacks becomes a
tracked stdlib deliverable here.

## Ground truth (verified against the tree at 475bb749)

- SysCall# is REAL and sufficient: lib/std/syscall.bv wraps SysCall#(nr,a1..a6)
  (observable:true) as syscall1..syscall6 with [nr>0][nr<512] contracts.
  examples/glue-host/node_bridge.bv already does file I/O with literal syscall
  numbers (open=2, read=0, write=1, close=3, lseek=8) plus Alloc#/Load#/Store#.
  Sockets are reachable the same way (socket=41, connect=42, bind=49, listen=50,
  accept=43, sendto=44/write=1, recvfrom=45/read=0, setsockopt=54).
- os/net.bv and os/fs.bv are EMPTY STUBS: both contain only repeated SYS_WRITE/
  SYS_READ const lines, no functions. (BUG to log - see Risks.)
- Frontend surface is real: lib/std/web/{dom,router,fetch,console}.bv, b-trigger/
  b-when/b-window directives, frgn ... from glue/web/web.js. fetch(url)->String
  exists; we extend it for methods/headers/body.
- Real stdlib we build on: string, string_builder, json, collections, hashmap,
  iterator, encoding, time, result, option.

## Architecture: five layers, bottom-up

Each layer is a stdlib module (or a documented extension of one). Every new module
ships with contracts, behavioral tests, and a short architecture note, per AGENTS.md.

```
L5  app       .kiro demo app: .rbv frontend  +  .bv backend
L4  http      lib/std/net/http.bv    HTTP/1.1 request parse + response build
L3  pg        lib/std/db/pg.bv       PostgreSQL v3 wire client
L2  socket    lib/std/net/socket.bv  TCP sockets over SysCall#
L1  bytes     lib/std/net/bytes.bv   byte buffer: read/write ints, slices
L0  syscall   lib/std/syscall.bv     EXISTS (SysCall# wrappers)
```

### L1 - bytes (lib/std/net/bytes.bv) NEW

Raw byte buffers backed by Malloc#/Load#/Store#/Copy#. The wire protocols need
precise byte-level framing that String concat cannot express.

CORRECTION (2026-10-12): Alloc# is NOT in the LLVM backend's supported-intrinsic
set — bytes.bv uses Malloc#/Free# (Alloc# worked only on the GLUE host path).

DESIGN D (2026-10-15): the derived network-byte-order writers are shared across
two buffer flavors via ONE trait (SPEC §8.6), not duplicated:

- trait ByteSink
  - REQUIRED (each buffer provides, with its own contract):
    put_u8(sink, v) ; put_bytes(sink, src_addr, n)
  - DEFAULT (written once in the trait, over the required primitives):
    put_u16_be(sink, v) ; put_u32_be(sink, v) ; put_cstr(sink, s: String)
- obj ByteBuf { ptr, len, cap } — FIXED: put_u8 keeps a `len < cap` room
  precondition; ptr is stable (never moves). buf_new(cap) / buf_free(b).
  Readers: get_u8 / get_u16_be / get_u32_be.
- obj GrowBuf { ptr, len, cap } — GROWING: put_u8/put_bytes have NO room
  precondition; on overflow grow_to_fit reallocates (Malloc# double-or-enough,
  Copy#, Free#, new ptr). grow_new(cap) / grow_free(b) / grow_get_u8.

Conformance is structural: an obj providing the required methods (with refining
contracts) satisfies the trait — no impl block. A function generic over
`<S: ByteSink>` serves both buffers with one implementation (see
examples/net-bytes-sink.bv).

WATCH-FOR: a growing put MOVES ptr. Length-prefix backpatching (Postgres, Task 3)
must use the FIXED ByteBuf or store OFFSETS, never cached interior addresses.

Contracts: offsets in [0,len); big-endian is explicit (Postgres and HTTP chunk
sizes are network byte order).

Compiler surface exercised: Malloc#/Load#/Store#/Copy# with computed offsets,
Int<->byte masking (bits.bv shifts/and at the language level), obj with mutable
Int fields, pointer-as-Int, SPEC §8.6 trait defaults + structural conformance
monomorphized onto two concrete types. (En route this exercised — and fixed —
three compiler bugs: trait-default %state, defn-liveness member-name collision,
and type-dependent needs_state for monomorphized members; see BUGS.md.)

### L2 - socket (lib/std/net/socket.bv) NEW

TCP client + server sockets via raw syscalls (Linux x86_64 numbers; documented as
literals because SysCall# resolves literals/abstract op names, NOT named consts).

- tcp_connect(host_ip: String, port: Int) -> Int   (returns fd, or <0 on error)
    socket(AF_INET=2, SOCK_STREAM=1, 0) then connect() with a packed sockaddr_in
- tcp_listen(port: Int, backlog: Int) -> Int   (socket+bind+listen, returns fd)
- tcp_accept(listen_fd: Int) -> Int   (returns client fd)
- sock_send(fd, ptr, n) ; sock_recv(fd, ptr, n) ; sock_close(fd)

sockaddr_in is a 16-byte struct built in an Alloc# block: family(u16 LE)=2,
port(u16 BE), addr(u32 BE), 8 zero bytes. IPv4 dotted-quad parse lives here.

Scope cut: no DNS, no IPv6, no TLS. Postgres and the browser talk to 127.0.0.1.
Blocking sockets only; the server handles one connection at a time in v1.

### L3 - pg: PostgreSQL v3 wire client (lib/std/db/pg.bv) NEW - headline deliverable

Pure-Briev client speaking the PostgreSQL frontend/backend protocol v3.0 over a
tcp socket. No libpq. Scope bar (agreed): connect, parameterized query, read typed
rows. Transactions and full type decoding are stretch.

Messages implemented (minimum viable client):

- Startup: StartupMessage(user, database). Handle AuthenticationOk; support
  cleartext + MD5 auth (encoding.bv has hex). SCRAM is the modern default - see
  Risks; may need a scram helper or a trust/md5 pg_hba for the demo.
- Simple query: Query -> RowDescription, DataRow*, CommandComplete, ReadyForQuery.
- Extended query (parameterized, the scope bar): Parse/Bind/Describe/Execute/Sync
  -> ParseComplete, BindComplete, RowDescription, DataRow*, ReadyForQuery. Params
  sent as text format; results read as text (format code 0) in v1.
- Terminate on close.

Row model: obj Row { cols: [String] } with typed accessors row_int(r,i) /
row_str(r,i) that parse the text column. obj PgConn { fd, ... }.

Compiler surface exercised: the hardest test - stateful binary protocol framing,
message-type dispatch (match on a byte), length-prefixed reads, loops over a
variable row count. This is where we expect to find the most bugs.

### L4 - http (lib/std/net/http.bv) NEW

Minimal HTTP/1.1 over a tcp socket. Server-side for the backend.

- parse_request(fd) -> Request { method, path, headers, body }  (read to blank
  line, then Content-Length bytes). Reuses encoding.bv/string.bv.
- write_response(fd, status, content_type, body).
- Helpers: json_response(fd, status, body), not_found(fd). Routing is the app.

Scope cut: one request per connection, no keep-alive, Content-Length only.

### L5 frontend - extend web/fetch.bv

Current fetch(url)->String is GET-only. Extend the glue + stdlib so the frontend
can POST/PUT/DELETE JSON:

- frgn fetch_method(url, method, body) -> String  (glue/web/web.js addition)
- http_get/http_post/http_put/http_delete wrappers in web/fetch.bv
- The .rbv app renders the task list from JSON (json.bv parse) and calls these on
  b-trigger:click.

NOTE: the browser cannot open a raw TCP socket, so the frontend talks HTTP to the
.bv backend; only the BACKEND speaks the Postgres wire protocol. This is the
correct and only architecture.

## Deliverables ledger (track here)

| ID | Module | New/Extend | Scope-bar gate |
|----|--------|-----------|----------------|
| S1 | lib/std/net/bytes.bv  | NEW ✓ DONE 2026-10-15 | ByteSink trait: ByteBuf (fixed) + GrowBuf (growing) share derived BE writers as defaults; round-trip u8/u16/u32/cstr on both; generic `<S: ByteSink>` serves both. Gate: tests/tier1/byte_sink_trait_smoke.bv (golden "18527210501852721050" in daily_use_smoke.rs) + examples/net-bytes-sink.bv |
| S2 | lib/std/net/socket.bv | NEW ✓ DONE 2026-10-16 | TCP client + server over raw syscalls: sock_tcp/connect/send/recv (partial-transfer loops, MSG_NOSIGNAL), sock_bind_listen/accept, set_nonblocking + SO_REUSEADDR; 16-byte sockaddr_in built from the L1 ByteSink writers via dot chaining; -errno via obj Socket. Gate: tests/tier1/socket_loopback_smoke.bv loopback round-trip (golden in daily_use_smoke.rs), brievc check clean. Two compiler bugs found+fixed (BUGS.md 2026-10-16): dot-dispatch explicit-receiver args; -> Void defn called as a statement. |
| S3 | lib/std/db/pg.bv      | NEW    | connect + SELECT + parameterized query, typed rows |
| S4 | lib/std/net/http.bv   | NEW    | parse a real browser GET, write a JSON 200 |
| S5 | lib/std/web/fetch.bv  | EXTEND | POST/PUT/DELETE JSON from .rbv to backend |
| S6 | docs/architecture/net-stack.md | NEW | document layer contracts + syscall ABI |

Each S-item: contracts + behavioral tests (cargo test --lib and/or a .bv gate) +
brievc check green + Praetor no-new-diagnostics on changed dirs + BUGS.md entries
for anything found. New .bv stdlib files must pass the conformance sweep.

## Risks and open questions (the interesting part)

1. os/net.bv + os/fs.bv are corrupted stubs (repeated SYS_WRITE consts). Likely a
   generator bug or bad merge. ACTION: log in BUGS.md; build the real modules under
   lib/std/net/ rather than editing the stubs.
2. SysCall# literal-vs-const resolution: node_bridge.bv warns a named const
   (SYS_OPEN) is NOT resolved and silently falls to 0. We MUST use literal numbers.
   A sharp edge worth a compiler diagnostic.
3. Postgres auth: modern PG defaults to SCRAM-SHA-256 (HMAC/SHA-256 + base64 +
   nonce). Mitigation for the demo: configure PG with trust or md5 auth. SCRAM is a
   stretch stdlib item (lib/std/crypto/). Log the gap.
4. Byte-accurate Int<->bytes needs bits.bv shifts/masks on Int. Verify bits.bv has
   shl/shr/and/or on Int; if not, that is a prerequisite stdlib addition.
5. Socket reads return partial data; readers loop until expected length. A txn with
   [got<need][got==need] is the natural shape.
6. Does the LLVM backend allow a long-running accept-loop program (no fold to dead
   code)? The server never halts; observability comes from send() FFI. Confirm the
   optimizer does not elide the loop (observability-as-liveness, AGENTS.md).
7. String is UTF-8/Char-based; raw bytes live in ByteBuf, never String. Keep the
   byte boundary strict: String only at the text edges.

## Suggested sequencing

1. S1 bytes (+ any bits.bv prereqs) - pure, unit-testable, no network.
2. S2 socket - prove tcp_connect against the local PG port accepting a connection.
3. S3 pg - connect + SELECT 1 first, then RowDescription/DataRow, then parameterized.
4. S4 http + S5 fetch - backend serves the app; frontend calls it.
5. S6 docs + wire the .kiro demo app end to end.

## Bug / finding log (append as we go)

- 2026-10-09: os/net.bv and os/fs.bv are empty stubs (repeated consts). Pending
  BUGS.md entry.

(Compiler bugs found during implementation get a BUGS.md entry AND a line here with
the date, the .bv snippet that triggered it, and the workaround.)

### 2026-10-21 — relative code-import path bug (verification pending)

Task 10.1 gate fails before typechecking: `../examples/recommender/udata.bv`
becomes `///examples/recommender/udata.bv`. Minimal trigger is the first import
in `scratch/udata_gate.bv`. The code resolver expands namespace dots inside
filesystem-relative paths. Fix authored on `fix/relative-import-paths` with
regression coverage; no application workaround. Human verification pending.

### 2026-10-21 — relative import fix verified

`fix/relative-import-paths`: preserve filesystem-relative path spelling via
code_module_path; bare dotted module names retain their namespace expansion.
Human reports post-refactor cargo check, cargo test --lib, and cargo build green;
the native parser gate previously passed all six checks. Controlled Praetor
before/after diagnostics match. Compiler PR is prepared before feature work resumes.

### 2026-10-21 — schema gate exposes pointer-valued collection store (OPEN)

Native schema-gate build fails inside pg_decode_fields before any DB connection:
a singleton List<String> stores its ptr-valued element as i64 without conversion.
Generated IR proves the type mismatch for both literal and decoded text.
Controlled Int/String singleton probes authored, human verification pending.
No pg.bv workaround. See the pointer-valued-collection-store investigation plan.


### 2026-10-21 — generic literal argument type loss isolated

Int list concatenation succeeds; String list concatenation reproduces the
schema-gate ptr/i64 failure without net/db imports. The compiler constructs
inline generic-call arguments using unspecialized List<T>. Frontend fix authored
on fix/generic-call-literal-types, preserving the inferred collection type in a
typed value block. Generic-call and operator-call paths share the helper.
Unit/native verification pending human execution; application code unchanged.

### 2026-10-21 — generic literal fix regression corrected, retest pending

Full-suite output exposed empty tuple accumulators losing their declared return
context during elaboration. Initialize current_output_type for defns and txns
as ordinary typechecking already does. Dedicated accumulator regression added;
no tests disabled or stdlib workarounds. Human filtered/full-suite reruns pending.

### 2026-10-21 — generic literal argument fix verified

Human reports full suite green after declared-return context correction. Native
String concat gate: four true checks; Int control: two true checks. Reduced
original failure is resolved without stdlib changes. Praetor has no increased
metrics or added diagnostics. Commit/PR approval pending; full schema gate will
be rerun on the integration branch after the separate compiler PR handoff.
