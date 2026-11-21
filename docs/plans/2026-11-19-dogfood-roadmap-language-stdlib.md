# Briev roadmap from the Knooppunt dogfood — language + stdlib (2026-11-19)

**Status:** EXECUTION RECORD (2026-11-20).

**Update 2026-11-20 (evented-net, PR #147):** the concurrency half of
this roadmap's runtime gaps got its first wave. `feat/evented-net`
lands the composition `server_loop.bv`'s header named: poll-multiplexed
connections (`std/net/evented.bv`), fd linearity as a checker fact
(`attr linear` + analysis::obj_linear), the io/pure strand
classification (analysis::strand — the fact source L1 needs to upgrade
#15's conservative #15 routing), and the chunked provable read (G12b
closed at the stdlib layer). L2 (chunked read) is DONE in this PR;
L1's fact source is landed, convergence pending; S4 (signals) and true
async-I/O-under-await remain open.

WHAT THE CHUNKED READ AND THE OUTBOUND MIRROR TAUGHT the contract
discipline - four rules worth carrying into the next primitives:

1. **Bounds as arguments, not loop-carried state.** `recv_into(socket,
   buf, offset, room, flags)` proves at every call site because the
   caller supplies `offset + room <= buf.cap` from its own locals.
2. **Advance lengths through a contract, never a field write.** A
   mid-body `buf.len = fed + got` leaves no projection the checker can
   carry into the next iteration; `ByteBuf.claim(count)` — an obj method
   whose postcondition names `term.len == sink.len + count` — does. The
   same defn as a FREE function does not project (issue #150), and that
   asymmetry decides whether a primitive can be a function or must hang
   off an obj.
3. **The deny gate lives only in the test suite.** `brievc check` and
   `build` never install `config/contracts.dbvl`, so the conformance
   sweep is the first place an unprovable call site appears — one CI
   round-trip per discovery until issue #149 is fixed.
4. **Obj parameters are references, so a reader can publish progress.**
   A callee's field write lands in the caller's object (through recursive
   frames too), which is what makes `buffer.len` a real out-channel: a
   partial request parks and resumes with
   `fill_request(sock, buf, buf.len, flags)`. The publish must happen in
   the frame that still OWNS the caller's binding - `claim` returns a
   fresh value, so a deeper frame's write lands on that temporary and is
   lost. Linearness of the publish point decides correctness here, and
   the resumption test in `evented_smoke` is the thing that caught it.

OUTBOUND COMPLETION (2026-11-21, same PR): the write-side mirror landed
so the evented lane can finish a transfer, not just intake one.
`send_into` (bounds as arguments, mirroring `recv_into`),
`drain_response` (advance-until-the-kernel-stops, reports the running
count: total = DONE, 0 <= k < total = PARKED, negative = -errno), and
`evented_arm`/`evented_write_ready` (the per-connection IN/OUT phase
switch). The lifecycle pin is machine-dependent by documented design: a
256 KiB payload against the kernel's unix stream queue cap (~219264 bytes
on the default rmem_max 212992 - SO_RCVBUF does not cap unix stream
queues, so the cap IS the machine's buffer size), then park, drain,
OUT-edge, resume, content check. Numeric postconditions on Int results
did project through the deny gate (the `drain_response` recursion
proves from `send_into`'s range), refining the #150 note: the asymmetry
is about posts naming OBJ-FIELD facts on a returned obj, not about Int
ranges.

Rescued from session-only
snapshots and committed after the wave landed. S1/S2/S3 DONE (landed on
main via PR #146 `fee5ce003` — checksum ladder kept in the integration
merge); S5 CANCELLED (argv/env already covered by `cli.bv` + `env.bv`);
L1 partially landed as #145's cycle analysis (the conservative #15
routing stays until the checker proves termination). Everything below
the TL;DR is the original research, kept as written.
**Origin:** bringing `kernel/routing_service.bv` up end-to-end (G14→G17) surfaced
five language-level gaps and seven stdlib holes. Every item below was paid for
in real debugging time this week; nothing is speculative.

## TL;DR

| # | Item | Kind | Size | Value | Order |
|---|---|---|---|---|---|
| L1 | Divergence-aware attributes: checker→backend convergence facts | language | M | unlocks `willreturn` soundly; kills the #15 conservatism | **first** |
| L2 | Chunked provable read (`GrowBuf` request reader) | stdlib+contract | S–M | 200 syscalls/request → ~6; unblocks G12b | **second** |
| L3 | Contracts → LLVM `!range` metadata | language | S | free optimization; machinery half-exists | **third** |
| S1 | `std/checksum.bv` (mod-11, mod-97, Luhn) | stdlib | S | Knooppunt flagship; tiny | parallel |
| S2 | `std/io/logging.bv` (unbuffered stderr + levels) | stdlib | S | kills the buffered-stdout debugging trap | parallel |
| S3 | `std/net/http_server.bv` (route dispatch) | stdlib | S | deletes the per-app predicate tax | parallel |
| S4 | `std/posix/signal.bv` real implementation | stdlib | M | graceful shutdown for sequential servers | after L2 |
| S5 | `std/sys/env.bv` typesafe argv/envp | stdlib | S | trivial; runtime globals already exist | parallel |
| L4 | Uniform error convention (`Result<T,E>` adoption) | language+stdlib | M–L | deletes sentinel-convention confusion | later |
| L5 | Runtime diagnostics (panic-with-location, `--debug-io`) | tooling | M | saves hours per miscompile-class bug | later |

Sequencing logic: L1 and L2 are the compiler-health pair (both were *proven*
blockers this week); the S-items are pure additions that cannot break anything
and make the flagship demo (routing service) a proper stdlib showcase. L4/L5
are program-level and should ride a dedicated campaign.

---

## Language items

### L1 — Checker→backend convergence facts (the #145 generalization)

**What this week proved.** `compute_transitive_may_trap` seeds from
`Trap`/`Halt` statements only, so diverging server loops were emitted with
`willreturn` and -O3 LTO deleted `main` (briev-lang PR #145, closed shape:
`entry → serve_forever → loop2`). The fix routes any defn on a *recursion
cycle* to `#15` — conservative because the backend cannot tell a converging
cycle (`fed >= buf.cap` guard) from a diverging one (accept loop).

**What the checker already knows.** The convergence analysis exists —
`when` guards ending in bare `term;` are classified CHECKPOINT vs TRAILING
(`scripts/g2_census.py`, commit `5a19d6f35`), and the node-completion walk
("no path ends this program" warning) already walks callee-closed reachability.
What's missing is a *plumbing contract*: one `CompilerContext` set
(`defn_may_diverge: HashSet<String>`) computed from the checker's convergence
verdict instead of the backend's syntactic cycle test.

**Plan.**
1. Extend the checker's defn summary with `converges: bool` — a defn converges
   if every recursion cycle through it is guarded by a condition the checker
   can prove terminates (loop-counter, buffer-cap bound, or explicit `hold`).
2. Plumb it into `CompilerContext` alongside `defn_may_trap`.
3. Backend rule becomes: `willreturn` iff `converges || !on_cycle` — #15 stays
   the fallback, so the #145 soundness floor is unchanged.
4. Census first: run the G2 census script over examples/ + lib/ to count how
   many cycles would newly claim `willreturn` (expect: all of `task_board`,
   `server_loop` tests, and the routing service minus the accept loop itself).

**Risk.** The checker's termination proof must be *sound* before the backend
trusts it — an unsound "converges" here recreates #145 with extra steps. Gate:
property test that every `converges=true` defn, when interpreted, terminates
on the existing corpus.

**Test fixtures.** `tests/tier1/divergent_wrapper_smoke.bv` (the #145 repro,
must keep serving), plus a converging-cycle case that gains `willreturn`
(check IR: `attributes #15` absent, `willreturn` present).

### L2 — Provable chunked read (G12b)

**What.** `serve_next_request` reads one byte per `recv()` (~200 syscalls per
JSON control request) because a fixed `ByteBuf` read loop cannot prove
`buf.len <= buf.cap` across iterations. `GrowBuf` already models growth
(`put_u8` postcondition `[term.len == sink.len + 1]`, `grow_to_fit`), but
nothing composes `sock_recv` with `GrowBuf.put_bytes` into a *provable* chunked
reader.

**Plan.**
1. Add to `net/socket.bv`: `sock_recv_grow(conn, buf: GrowBuf, max_chunk) ->
   (GrowBuf, Int)` — recv into scratch, then `put_bytes`. The contract story:
   `[max_chunk >= 0]`, post `[term.0.len >= buf.len]` + recv-count relation.
2. Rewrite `serve_next_request` on top: loop guard becomes
   `parsed.status != COMPLETE && fed <= MAX_REQUEST`; each iteration moves
   ≤ max_chunk bytes. Syscalls per request drop from ~200 to
   `ceil(size/chunk)`.
3. Tier-1 golden: `chunked_read_smoke.bv` — loopback HTTP with a 4 KB request,
   assert identical parse result and count syscalls via strace in the test
   harness (CI: assert `< 10`).
4. Fold back into `server_loop.bv` + `task_board` (delete the duplicated
   one-byte readers).

**Risk.** The `(GrowBuf, Int)` tuple return may hit the tuple-in-contract
checker paths that `http_parse_request` already exercises (`HttpParse` struct)
— prefer a struct (`GrowRead { buf: GrowBuf, fed: Int, status: HttpParse }`)
if tuples fight the convergence analysis.

### L3 — Contracts → LLVM `!range`

**What.** `range_meta` infrastructure exists (`!range` metadata emitted today
for loop counters only, `mod.rs:5632`). Contracts like `[port > 0 && port <
65536]` are checked but discarded at codegen. Feeding literal-bound
preconditions into `!range` on parameters (and postconditions on returns)
lets LLVM delete the redundant runtime guards the emitter lowers from
`check`/`when` convergence — and strengthens branch prediction.

**Plan.**
1. In `emit_toplevel`, for each defn parameter, scan its precondition conjuncts
   for `param OP literal` / `literal OP param` pairs; emit `!range !{i64 lo, hi}`
   (signed-wrapped carefully: `[term >= -4096]` → `range(i64 -4096, …)` needs
   the us saturating form; single-sided bounds use `!range` with `i64` open side).
2. Start with postcondition-only metadata (safe: it describes the *return
   value*, no caller changes needed), then parameters behind a flag.
3. Golden: `range_metadata_smoke.bv` with a `[term >= 0 && term <= 255]`
   defn; assert the emitted `.ll` carries `!range` and the compiled binary
   drops the guard branch (objdump count of `cmp` in the hot path).

**Risk.** Low — metadata is advisory; a wrong range is a miscompile, so only
emit bounds the checker *proved* (literal-bounded conjuncts), never inferred ones.

---

## Stdlib items

### S1 — `std/checksum.bv`

Pure arithmetic, house-pattern contracts. `mod11_bsn(digits: String) -> Int`
(0/1 valid flag), `mod97_iban(iban: String) -> Int`, `luhn(digits: String) ->
Int`. All three are already written inside `routing_service.bv`'s
`bsn_flag`/`iban_flag`/`read_digits` — extract, generalize, add the exhaustive
malformed-input probes (non-digit chars, wrong lengths) as goldens. Knooppunt's
service then imports instead of owning.

### S2 — `std/io/logging.bv`

The debugging trap this week: stdout buffers until exit (`stdout_flush` only in
`.end`), so a live server prints nothing and a dead one prints binary junk.
The module: `log_info/log_warn/log_error(str)` writing **direct-syscall
unbuffered fd 2** (stderr), `log_fmt` thin sugar over `string_builder`. Every
helper keeps the `#13` raw-memory attribute discipline (`write_all` is the
model). Include a `log_flush_stdout()` re-export of `stdout_flush` for
endprogram paths. Test: golden asserting stderr bytes arrive *before* process
exit (kill -TERM mid-run, read the partial log).

### S3 — `std/net/http_server.bv`

The per-app tax `server_loop.bv` deliberately left out (its header says so):
route dispatch. API sketch:

```
obj Route { method: Int; path: String; }
defn route_match(request: HttpRequest, route: Route) [true][term == 0 || term == 1] -> Int
defn dispatch(request: HttpRequest, body: String) -> String   // 404 body fallback
```

Extract from `routing_service.bv` (`is_health`/`is_preview`/`not_handled` +
the status/body writers). Keep it *sequential-loop-shaped* so the convergence
checker accepts the composition (L1 makes this safe even for future
multiplexing). Depends on nothing new; can land immediately.

### S4 — `std/posix/signal.bv` (fill the reserved stub)

The stub header already specifies the shape: sigaction(2) lanes + machine-entry
ISR machinery. Scope for the server use-case only: `sig_term_handler` +
`install_graceful_shutdown(exit_flag: *Int)` — a handler that stores to a
global flag the accept loop polls via `hold`. This needs the ISR/machine-entry
path (the only genuinely new compiler surface in this whole list; everything
else is .bv-only). Do it after L1 so the accept loop's convergence story is
expressible with the flag (`hold exit_flag == 0 { reside; }` shape from the
residency warning text).

### S5 — `std/sys/env.bv`

Runtime already captures `@__briev_argc/@__briev_argv/@__briev_environ`
(emitted in every `@main`). `argv_count() -> Int`, `argv_at(i) -> String`,
`get_env(key: String) -> Option[String]` over the existing cast_lanes walkers.
Half-day item; unblocks `cli.bv` composition.

---

## Program-level (later, separate campaigns)

### L4 — Uniform error convention
`std/result.bv` exists (`Result<T,E>` with native enum construction) but the
net stack returns `-errno` / `-1` sentinels / `Int` flags depending on era.
Campaign: migrate `net/socket.bv`+`server_loop.bv` to `Result<Socket, NetErr>`
where `NetErr` carries the errno. Mechanical but wide; needs the
deprecation-window discipline from the callable-txn migration (2026-10-05).

### L5 — Runtime diagnostics
(a) Panic-with-location: the `check` lowering already knows the source span —
emit it into the exit path (`briev_rt.c` write(2) before exit_group).
(b) `--debug-io`: unbuffered-stdout build flag (every `stdout_byte` becomes a
direct write(2)). Both are small; bundle with any next miscompile-hunt.

---

## Suggested landing order

```
Now (pure adds, zero risk):      S1  S2  S3  S5   (one PR each, house pattern)
Compiler-health pair:            L2 → L1  (L2 exercises the checker surface L1 needs)
Free optimization:               L3 (any slot; metadata-only)
Campaigns (plan first):          S4 → L4 → L5
```

Each stdlib PR carries its tier-1 golden; L-items carry IR-golden + property
test as specified above. The Knooppunt routing service is the integration
checkpoint: after S1+S2+S3 land it imports three stdlib modules instead of
owning hand-rolled copies, and after L2 its request path drops two orders of
magnitude in syscall count.
