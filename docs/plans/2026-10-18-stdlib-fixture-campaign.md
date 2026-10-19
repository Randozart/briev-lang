# Stdlib fixture campaign — four serious implementations as compiler stress tests

**Date:** 2026-10-18
**Status:** ACTIVE — baseline recorded, Fixture 1 next.
**Charter (owner, 2026-10-18):** experimentation. These fixtures are BOTH
stdlib additions AND discovery instruments for language improvement. Any
change that is truly core to the language must be discussed with the owner
BEFORE implementation — discovered gaps are logged here and escalated, never
patched around.

## Trigger

The native Briev code is stable (S1–S6 remediation landed 2026-10-16; suite
2901 green). The owner wants the stability claim exercised by serious
implementations — the four domains where traditional languages produce
catastrophic bugs and where Briev's contracts/topology thesis should shine.
A prior conversation (owner-supplied, 2026-10-18) proposed five fixture
families; four are in scope, one (BILLD HW recipes) deferred.

## Ground truth at tip (`e8c4476a`, verified this session)

| Conversation claim | Actual state |
|---|---|
| SPSC ring `std/sync/ring.bv` | Missing. `RingBuffer<T>` (collections.bv) is a single-node txn ring; `ring_buffer` bench is a modular-index exercise. `os/thread.bv` is an honest STUB (no pthreads). Atomic intrinsics exist with LLVM ordering params (`src/backend/llvm/intrinsics.rs:1796`). Rule 22 concurrency gate is LIVE (`src/analysis/concurrency_gate.rs`, async/sync classification + tests). Async node pair template: `benchmarks/async_counters.bv`. |
| Protocol framing | `net/bytes.bv` L1 exists (ByteBuf/GrowBuf, BE codecs, ByteSink trait) + TCP socket.bv. No framed-protocol decoder anywhere (no WebSocket in tree). |
| Arena | `memory/arena.bv` exists, 47 lines, no alignment. SEMANTICS BUG: `struct Arena` mutated by value in `arena_alloc` — the offset advance is lost to the caller. |
| BILLD HW recipes | `.bld` merged (boot recipes). No I2C/SPI/UART. **Deferred out of this campaign** (platform-target question open). |
| FSM | Missing. Enums (closed sums, exhaustive match), contracts, `check` all exist. |

## Honest corrections to the conversation (scope discipline)

- "Invalid FSM transitions do not compile" — aspirational. Today: exhaustive
  enum match + solver-provable contracts + runtime `check`. A compiler-side
  transition-graph verifier would be a new core workstream → out of scope,
  escalated if the fixture shows the need.
- "Compiler uses bounds to optimize masking" — strength reduction is general
  optimizer behavior; the fixture TESTS whether it fires on contract-derived
  masks.
- FPGA isomorphism — `.sbv` is Experimental; excluded.

## Layout decision (owner): flat

No new directories. Ring extends `lib/std/atomic.bv`; framing extends
`lib/std/net/bytes.bv`; arena upgrades `lib/std/memory/arena.bv` in place;
FSM is one new flat file `lib/std/fsm.bv`.

---

## Fixture 1 — WebSocket frame codec (RFC 6455) on `net/bytes.bv`

- Decode: fin/opcode/mask bit, payload length 7/16/64-bit adaptive, masking
  key apply, bounds proof on EVERY read (`off + n <= buf.len`). Truncated or
  malformed input fails a precondition cleanly — never an over-read.
- Encode: frame writer riding the existing ByteSink trait defaults.
- C reference: expert hand-written RFC 6455 parser. Deterministic mixed
  corpus generated identically in both (16/64-bit lengths, masked/unmasked,
  truncated-reject cases). Tag: runtime.
- Stresses: branchy var-length decode, Load# chains, contract propagation
  through recursion, bounds proofs.

## Fixture 2 — SPSC ring on `std/atomic.bv`

- `obj SpscRing` (Int slots v1): Malloc# slot array, head/tail via
  `AtomicLoad#/AtomicStore#/AtomicCas#` with explicit orderings; monotonic
  indices (`size = tail - head`, canonical full/empty discipline, no
  wraparound-ambiguity class); power-of-2 capacity precondition →
  `idx & (cap - 1)` masking.
- Exercise: producer/consumer as an `async`-classified node pair (Rule 22
  gate live); checksum at drain via `endprogram`. Template:
  `benchmarks/async_counters.bv`. First step of this fixture: verify async
  pair scheduling semantics against that template.
- C reference: C11-atomics SPSC, fair bar (relaxed/acquire-release).
- Honest limit: the reactor is single-threaded today — this validates
  structure, atomic lowering, and strength reduction. True cross-thread race
  validation is deferred (no pthreads lane); noted, not hidden.
- Generic element sizes (T slots): v1 is Int-only. If T-slot sizing hits a
  compiler gap → file in BUGS.md + escalate; never patch around.
- Stresses: atomic IR with orderings, contract→mask strength reduction,
  monotonic index proofs.

## Fixture 3 — `memory/arena.bv` upgrade

- Fix the value-semantics bug: convert to obj (reference semantics); migrate
  ALL callers (grep `arena_init`/`arena_alloc`/`arena_reset`/`arena_free`).
- Add `arena_alloc_aligned`: power-of-2 align precondition
  (1/2/4/8/16) → backend may emit a bare mask, no defensive branch.
- Tightened lifecycle contracts (init/reset/free).
- C reference: bump allocator benchmark. Stresses: alignment bit math,
  proof-carrying allocation.

## Fixture 4 — `lib/std/fsm.bv`: topological state machine

- Enum states; transitions as contract-gated calls (precondition = current
  state via pattern match); exhaustive match; runtime `check` where the
  solver cannot prove statically.
- Instance: TCP handshake/teardown (RFC 793 subset) — the conversation's own
  example. Benchmark: event-stream transition dispatch throughput; C
  reference: `switch` FSM, same event corpus.
- Honest scope: NOT promising compile-time rejection of illegal transitions;
  the solver rejects some statically, the rest gate at runtime. Statically
  rejected cases get recorded as evidence in the results file.
- Stresses: enum tag dispatch, exhaustive match lowering, contract-gated
  edges.

---

## Sequencing

1. This plan + baseline table → commit.
2. Per fixture (1→2→3→4): implement → interpreter/backend parity test
   (`tests/*_interpreter_parity.rs` pattern) → harness registration
   (TAG array + BENCHES list; `_sym`/`_idio` split if approaches diverge) →
   `cargo test --lib` → `ll_determinism_check.sh --quick` → Praetor on
   changed dirs → results file (`benchmarks/results/2026-10-18-…`) → commit.
3. Close: INDEX.md update + campaign results summary; BUGS.md entries for
   every compiler gap the fixtures expose (the real point).

## Baseline (Rule 12) — commit `e8c4476a`, 2026-10-18

Clean `cargo build --release` (3m09s) + `bash benchmarks/build_and_bench.sh
--runtime`, 5 iterations, BOUND=50000000. Ratio = Briev/C (<1 = Briev wins).
Working tree carried uncommitted benchmark-binaries from a prior run (no
source changes). Full log: captured in session; table below is the record.

| Benchmark | Briev | C | Ratio | Winner | Correct |
|---|---|---|---|---|---|
| ring_buffer | 0.0427s | 0.0424s | 1.01x | C | MATCH |
| float_math | 0.1236s | 0.0430s | 2.87x | C | MATCH |
| float_math_nonzero | 0.2437s | 0.1645s | 1.48x | C | MATCH |
| sparse_dispatch | 0.0424s | 0.0575s | 0.74x | Briev | MATCH |
| print_loop | 0.0279s | 0.0553s | 0.50x | Briev | MATCH |
| nbody_newton | 7.4897s | 6.9353s | 1.08x | C | MATCH |
| nbody_newton_accel | 1.4192s | 0.1287s | 11.03x | C | MATCH |
| nbody_sqrt | 2.6098s | 3.3372s | 0.78x | Briev | MATCH |
| nbody_sqrt_idio | 2.8780s | 3.1265s | 0.92x | Briev | MATCH |
| fasta | 0.1929s | 0.2675s | 0.72x | Briev | MATCH |
| fannkuch_redux | 0.0638s | 0.0552s | 1.16x | C | MATCH |
| mandelbrot | 0.6463s | 0.6798s | 0.95x | Briev | MATCH |
| kalman_filter_runtime | 0.1904s | 0.1791s | 1.06x | C | MATCH |
| knucleotide | 0.1821s | 0.1906s | 0.96x | Briev | MATCH |
| cancel_math | 0.0424s | 0.0604s | 0.70x | Briev | MATCH |
| bit_clear | 0.0005s | 0.0003s | 1.67x | C | MATCH |
| queue_drain | 0.0288s | 0.0585s | 0.49x | Briev | MATCH |
| queue_drain_sym | 0.0284s | 0.0592s | 0.48x | Briev | MATCH |
| queue_drain_idio | 0.0286s | 0.0589s | 0.49x | Briev | MATCH |
| stack_push_pop | 0.0298s | 0.0584s | 0.51x | Briev | MATCH |
| interval_step | 0.0413s | 0.0591s | 0.70x | Briev | MATCH |
| telemetry_stream | 0.2331s | 0.1596s | 1.46x | C | MATCH |
| pid_control | 0.4253s | 0.3482s | 1.22x | C | MATCH |
| matrix_pipeline | 0.4834s | 0.5066s | 0.95x | Briev | MATCH |
| accumulator_flush | 0.1277s | 0.1401s | 0.91x | Briev | MATCH |
| sweep_sparse | 0.2292s | 0.1536s | 1.49x | C | MATCH |
| sweep_mid | 0.2300s | 0.1675s | 1.37x | C | MATCH |
| sweep_dense | 0.3906s | 0.3704s | 1.05x | C | MATCH |
| sweep_arr | 0.4396s | 0.4319s | 1.02x | C | MATCH |
| series_converge | 0.0004s | 0.0001s | 4.00x | C | MATCH |
| global_lifetime | 0.0287s | 0.0664s | 0.43x | Briev | MATCH |
| deep_recursion | 0.0003s | 0.0003s | 1.00x | ~tie | MATCH |
| arena_churn | 0.0283s | 0.0910s | 0.31x | Briev | MATCH |
| linked_list | 1.0233s | 1.9343s | 0.53x | Briev | MATCH |
| hash_ops | 0.8811s | 1.0279s | 0.86x | Briev | MATCH |
| hash_ops_idio | 0.7254s | 0.6708s | 1.08x | C | MATCH |
| enemy_swarm | 0.1013s | 0.1317s | 0.77x | Briev | MATCH |
| recommender | 0.0129s | 0.0117s | 1.10x | C | MATCH |

Context rows for this campaign: `ring_buffer` 1.01x (the existing
modular-index exercise is at parity), `arena_churn` 0.31x (the existing
arena-shaped workload beats C 3x), `queue_drain` family ~0.49x (the generic
obj RingBuffer drain beats C 2x). The NEW fixtures aim at the same or better
on their own shapes — and where they do not, the gap is a finding, not a
failure.

## Status (2026-10-18, end of session)

Fixture 1 (WebSocket codec) COMPLETE: module committed (9707f4bf + fix
c2d1aa62), benchmark + C reference registered (TAG runtime), interpreter
parity test landed (decode scenarios; len64/encode deferred on the open
interpreter bug), full harness 79/79 MATCH. Results:
`benchmarks/results/2026-10-18-fixture1-ws-frame.md` (ws_frame 1.58x — C
wins at the per-byte codec-call design; an _idio variant is the recorded
follow-up).

Fixtures 2–4 (SPSC ring, arena, FSM) NOT STARTED — next session. The
compiler-fix harvest below substantially de-risked them: the splice, gate,
and stride machinery they would have tripped over is now fixed and pinned.

## Discovery log (experimentation charter — the campaign's core yield)

Every fixture logs: language gaps, awkward constructs, missing intrinsics,
proof-engine misses, codegen opportunities. Core-language items were
escalated to the owner BEFORE any core change (the `::` ruling came from
the owner mid-session); everything else was fixed in place with pins.

| # | Fixture | Finding | Class | Disposition |
|---|---|---|---|---|
| 1 | ws_frame | `check` is a NO-OP in the LLVM backend (interpreter enforces, backend discards) | compiler-gap, CORE-adjacent | OPEN — deferred; logged emit_stmt.rs:2471. Needs owner discussion (trap semantics vs assume/UB model) |
| 2 | ws_frame | Contract violations lower to `unreachable` (assume/UB), not clean traps | CORE-escalate | OPEN — design question: is UB-on-violation the intended contract runtime? |
| 3 | ws_frame | Global obj FIELD READ segfaults (plain-let obj globals) | compiler-gap | OPEN — avoid obj-typed globals in benchmarks; needs a proper fix |
| 4 | ws_frame | Import splice: match-arm + member-body calls dropped from closure | compiler-gap | FIXED bde072ac (ast::visit) |
| 5 | ws_frame | Qualified variant patterns impossible in importers; `::` mis-parses | language + parser | FIXED 9a43fcb2 — owner ruling: `::` illegal, dot form everywhere, SPEC corrected |
| 6 | ws_frame | SPEC §8.3/§11.3 showed `::` examples (stale vs 2026-09-22 dot form) | docs | FIXED 9a43fcb2 |
| 7 | ws_frame | ~~Consts in reactive contracts break cycle analysis~~ WITHDRAWN — the real cause was #8 | — | withdrawn |
| 8 | ws_frame | Causality harvester blind to nested/indexed writes (false refusals + unsound Rule 22 disjointness); goal-reachability rejected all provable completions (gate-vs-gate conflict) | compiler-gap | FIXED acbff4bb |
| 9 | ws_frame | Element stores/reads through raw pointers hardcoded i64 stride (invalid IR for byte pointees; recommender regression via Float read/write stride mismatch) | compiler-gap | FIXED acbff4bb + 855a9c1e |
| 10 | ws_frame | SOURCE lesson: sequential-guard fall-through — discarded tail values are not returns (the codec's own if/else-if habit) | authoring | FIXED c2d1aa62; BUGS.md lesson recorded |
| 11 | ws_frame | Interpreter: compound bitwise-or over non-literal operands yields stale values (also GrowBuf growth) in spliced programs | compiler-gap (interpreter) | OPEN — BUGS.md; len64/encode parity scenarios deferred |
| 12 | ws_frame | ws_frame 1.58x: per-byte codec-call design vs C's inlined loop | perf opportunity | recorded — _idio variant (inlined/batched unmask) is the follow-up |

## Risks / open items

- Async node pair semantics for producer/consumer: verify against
  `async_counters.bv` scheduling BEFORE Fixture 2 lands (first step of that
  fixture).
- SpscRing generic-over-T: possible expressiveness gap (element-size
  polymorphism over raw memory) — file + escalate if hit.
- Arena obj conversion changes the module's public shape; the benchmark
  `arena_churn` and any other callers must migrate in the same commit.
- The websocket codec is the chosen frame fixture (over DNS): streaming
  framing + fits bytes.bv's PostgreSQL/HTTP heritage. TCP chosen as the FSM
  instance (over tokenizer): the conversation's own example.
