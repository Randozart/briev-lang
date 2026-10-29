# Session report — 2026-10-27

**Last updated:** 2026-10-28T10:56:08+01:00
**Status:** compiler and app build/check green; the browser catalogue gate passes.
Proof-consumption ledger is active.
**Companion PR:** https://github.com/odecode/recommender-app/pull/1 (draft)

This report is the running record. Append a timestamped entry whenever work
lands; do not rewrite history.

## Done

### 2026-10-28 — browser catalogue root cause

- `10:56` Fixed state collection initializers: the `op Init`/`InitEmpty`
  construction header was stack-allocated in `init_state` and its handle stored
  into `%State`; it dangled on return. The header is now heap-owned.
- `10:56` Fixed wasm plain tail-recursion lowering: guarded `when` tails are
  recognized, `i32`/`ptr` parameters round-trip through i64 loop slots, mutable
  parameters reload/store their allocas, and deferred struct allocas flush
  before the tail header.
- `10:56` Fixed callable-txn state leakage: a preceding tail-lowered defn left
  `tail_loop_backedge`/header/slots on the shared `FunctionState`, suppressing
  txn body definitions and producing undefined-register IR.
- Regression: `state_collection_initializer_heap_allocates_header`,
  `guarded_tail_recursion_is_loop_lowerable`, `rbv_list_each_wasm`.
- `cargo test --lib`: 3360 passed.
- App `browser_pending_peer.mjs`: passes end to end.

### 2026-10-26 — app compatibility and compiler prerequisites

- `11:17` `e442f06b` — destructuring `let` propagates inference errors instead
  of silently binding `Int`.
- `11:17` `1c1dd70f` — proof engine reduces `Type { field: value }.field` to
  `value`.
- `11:24` `064aaaa6` — BUGS record for both.
- App `944d4b5` — rename `Matrix`→`FloatMatrix`, `Vector`→`FloatVector`.
- App `435159b` — explicit contract guards across the recommender.
- `13:19` `c380d543` — recompute structural `obj` sizes after registration
  (fixes heap under-allocation, LTO segfault, worker generation-state loss).
- `13:25` `beeb022b` — keep explicit `spec bytes/bits` out of structural resize.
- App `4a33985` — compatibility notes updated.
- `17:07` `7e4e1efb` — PG acceptance findings + nested JSON bug recorded.
- App `4b00d41` — terminal guard ordering and PGSCHEMA gate harness.
- App `444be3b` — receipt payload JSON normalization around the nested
  `JsonValue`-in-`Array` compiler bug.
- `17:26` `b7bdfb08` — `b-window` preserves hyphens (`recommender-live`).
- App `2bd9467`, `7200879` — server gate password/property hook, live-event
  notes.
- `19:44` `1ce97e48` — conjunct fall-through facts, elaboration postconditions,
  constant folding in goals+facts, match-arm fact scoping, difference bounds.
- `19:48` `59683e9c` — proof-completeness plan indexed.
- App `f1b1e7e`, `0707f17`, `81e6068`, `f197620` — first manual guard removal
  passes and contracts/counter changes (source removals later reverted).
- `21:12` `2f4ffc1a` — arithmetic loop-range facts.
- `22:45` `cdd2b7c8` — contract-boundary pass.
- `23:49` `77782feb` — counter induction, bounded Fourier–Motzkin, loop mutation
  invalidation; two prover soundness bugs caught by tests.

### 2026-10-27 — proof-aware optimization and ledger

- `07:21` `e2f06b2c` — compiler eliminates proven assertion guards after
  typechecking. Source guards are retained. AST/IR regression tests added.
- `07:21` App `6903769`, `b12d316` — restore semantic source guards (505 total),
  remove only literal/constant/duplicate noise (107), rely on compiler
  elimination. A/B IR structurally identical to the manual-removal branch.
- `07:40` `177766cc` — Rule 25 corrected to **PROOF LICENSES OPTIMIZATION**;
  lesson recorded; proof-consumption ledger opened.
- `08:07` `ffda5bd0`, `d2ec9e82` — `BRIEV_PROOF_LEDGER=1` guard instrumentation;
  ledger indexed as active workstream.
- `08:14` `eec76a0f` — ledger validation + full gate record.
- `08:33` `141c083d` — contract/check consumers recorded; ledger snapshot:
  - guard: **176 proven / 176 lowered / 0 unconsumed**
  - contract: **1537 typecheck-only**
  - check: **1 typecheck-only**
- Accessors `ivec_get`/`vec_get`/`mat_get` emit direct loads; no runtime bounds
  check exists. Recorded as `NONE (static contract; no runtime check)`.
- `10:37` Browser catalogue investigation:
  - `Live` binding works; three HTTP events reach the host.
  - login/state/movies requests are issued, but the UI remains at
    "Loading the movie catalogue…"; state count remains 0.
  - a drain probe observed `pending=1 remain=0` after the login response; the
    two follow-up handles were issued but were not present in `http_requests`
    when the next drain ran.
  - a deferred-load transaction (scheduled after login) did not fix it; the
    experiment was reverted. App source is clean at `b12d316`.
  - root cause: wasm32 collection stride mismatch (above), not a transaction
    race.
  - Next isolation: determine whether this is a host-event/transaction race or
    a compiler bug in global list mutation across nested calls. Add a minimal
    browser/WASM reproduction before changing app code.
- `11:05` **Found and fixed a wasm32 collection-stride compiler bug.** Minimal
  `List<(Int,String,Int,String)>` browser repro: first tuple stored, second
  zeroed. Portable `coll_resize` used 8-byte elements; list/Ptr reads used
  i64; non-scalar Ptr stores wrote i64. All now use target width (4 on wasm32,
  8 native). Reproducer now reports `len=2 first=catalogue second=state h=3`.
  `BUGS.md` updated. Browser app now applies the profile state (ratings 5)
  and the catalogue loads (Movies 1–12, page 1/141, 1682 films).
- `12:20` Browser gate now fails later: the live WebSocket recommendation event
  does not render a shortlist card (body shows "Your shortlist is taking
  shape"). Catalogue + ratings are correct; next isolate the live-event
  delivery/decode path.
- `12:40` Updated the wasm tuple-list regression test to pin the target-width
  stride; minimal fixture renders `second=2`. `cargo test --all` and daily-use
  smoke are green. Browser state/catalogue now work; the live recommendation
  event is the next blocker.
- `12:55` Live-event isolation: the page receives the WebSocket `message` event
  (`LIVECOUNT=1` after dispatch), but the app's `ws_pending` remains 0, so
  `client_receive_event` never sees the recommendation. Next: compare the host
  `webSockets` message queue against the app's channel handle; likely a
  wasm32/host-boundary queue issue, not the protocol decoder.
- `13:10` Corrected that finding: host queue receives the message (`q=1`) and
  app `ws_pending=1`; `client_receive_event` runs and
  `client_refresh_recommendation_cards` observes `CARDS n=1`. The UI still shows
  `cold_start`/no cards, so the remaining issue is post-transaction view state
  (cold_start/recommendation_cards commit or render), not message delivery.
- `13:50` Deeper probe hit wasm traps: `browser_catalogue_scan` memory access
  out of bounds during `client_refresh_rating_view`, and
  `str_to_float`/JSON depth `unreachable` on the live event payload. The current
  blocker is therefore additional wasm collection/float ABI behavior, not the
  view binding. Next: isolate catalogue scan indexing and float parsing with a
  minimal wasm fixture.
- `14:10` Generic wasm fixtures (no recommender names):
  - `List<Item>` + recursive scan: native `count=3 page=2`; wasm
    `count=3 page(view)=0 local page_len=2`. So the list mutation is correct;
    the UI `.^Size` path sees a stale/incorrect list state.
  - `"4.5" as Float`: native `4.5`, wasm `0`; JSON containing `4.5` also
    fails (`json_ok=false`). This is a generic portable string/float/tuple ABI
    issue in `str_to_float`/JSON parsing.
  - Next: fix generic list view state freshness and mixed Int64/Int tuple or
    float parse layout; app remains the corpus, not the mechanism.
- `14:35` Added `tests/fixtures/generic_wasm_scan.rbv` and a wasm compile
  regression. Runtime evidence: list mutation is correct (`local page=2`) but
  view `.^Size` is stale; `"4.5" as Float` is 0 on wasm. A generic call-width
  coercion (i32 literal into i64 Int64 param) exposed a specialized-call ABI
  gap (`is_ok$JsonValue$String` receives i64 for i32); attempted fix reverted
  to keep the compiler green. No app-specific code used.
- `15:49` Retried generic call-width adaptation with mangled/base parameter
  lookup. The specialized `is_ok` ABI still wants i32 while the generic
  `Result` metadata reports i64; reverted again rather than shipping invalid
  IR. Generic fixture compiles; runtime list-view/Float fixes remain open.
- `15:58` Root-caused the specialized call: the callee wants i32 and the
  argument metadata says i32, but the actual SSA value is an **i64 state-field
  load** of a Result. Fix enum/Result state-load register width (or track
  actual register width), not `is_ok`. Compiler left green.
- `16:20` Landed generic enum state-register width tracking (`reg_llvm_ty`):
  on wasm32, enum/Result state loads truncate to i32 and call emission uses
  the actual register width. Generic fixture now builds through `is_ok`.
  `rbv_list_each_wasm` and daily smoke pass. Runtime Float (`4.5`→0) and
  list-view `.^Size` staleness remain.
- `16:45` Landed generic call-argument width coercion (actual register width +
  enum-aware callee ABI). Generic `"4.5" as Float` now passes (`f_ok=true`)
  and JSON parse passes (`jsonp=true`). Browser gate now advances through
  login, state, 1682-film catalogue, live recommendation render (Movie 41/42,
  one card) and reaches the offline save fallback.
- Remaining browser failure: after socket close, clicking save does not issue
  `/v1/ratings`; next isolate the offline fallback transaction/state.
- `16:55` Added a wait for `Reconnecting` before clicking save in the browser
  gate; it still never issues `/v1/ratings`. So this is not a timing race; the
  app fallback save path needs isolation next.
- `17:10` Root cause: the app reconnects (250 ms retry) before the save click;
  `connection=Live` and `client_submit_operation` sends the rating over the new
  WebSocket (`peer.sent`), not HTTP. The gate asserts a forced `/v1/ratings`
  fallback and is racy. Next: make the gate either keep the socket offline
  deterministically or accept the WS-send path, then assert the same operation
  semantics.
- `17:30` Browser gate made timer-deterministic: hold reconnect timers, verify
  HTTP fallback, then release and explicitly dispatch reconnect. Both offline
  submit and failure paths now pass. Remaining failure: wasm OOB in
  `browser_catalogue_scan` during the reconnect state hydration. This is the
  generic list/scan issue, not transport timing.
- `17:50` Confirmed: offline save, accepted event, reconnect, saved event,
  second offline save and failure/status path all pass. Final error is exactly
  the wasm catalogue scan OOB on the second state hydration. Generic
  1682-item/two-scan fixture does not OOB, so the remaining trigger is the
  parsed `List<struct>`/state shape (still generic, no app branch).
- `18:05` Generic parsed-JSON struct list exposes an earlier trap: 1682 objects
  parse (`parsed=1682`) but the build loop into `List<Item>` leaves `items=0`
  with wasm `unreachable` before scan. Literal 1682-item/two-scan is clean.
  So the remaining path is parser/struct-construction, not list sizing or
  transport timing. App still uses protocol helpers; instrument its scan
  lengths next.
- `18:35` Opened `docs/plans/2026-10-27-eternal-abi-and-temporal-transport.md`.
  Phase C landed: emitted IR is verified with `opt -passes=verify` before
  clang/llc, and structural sizes assert size ≥ field end. `cargo test --lib`
  3357 passed; release rebuilt. Browser OOB remains; next is the physical
  ABI/collection model (Phase B/D).
- `19:05` Small generic parsed-JSON list narrows the trap: array length is 2,
  item indexing reaches step 2, then `json_get(item, key)` traps. stdlib
  `find_pair` is a transaction called from a defn; converting it to a pure
  recursive defn made parsing return empty/trap, so the fix needs the generic
  JSON accessor/transaction ABI, not a one-off rewrite. Reverted; compiler
  unchanged.
- `19:30` Tried two minimal stdlib fixes: relax `find_pair`'s false
  postcondition (`i >= count`) and inline a `foreach` lookup in `json_get`.
  Both changed parsing to `parsed=0` and still trapped, so neither is the fix.
  Reverted; stdlib/compiler unchanged. Need a source-level repro of why changing
  `json_get` alters `json_parse` execution before changing stdlib again.
- `20:45` Corrected the small-fixture diagnosis: `json_get` returns STRING
  values only; querying numeric `item_id` correctly returns `Err`. The
  2-object string-title path passes. The 1682 path still fails.
- `21:10` Found and fixed a real generic bug: `emit_struct_literal` hoisted a
  dynamic struct allocation to the loop preheader, so every appended struct
  aliased the last iteration. `List<Item>` parsed from JSON read the final
  title for every element. Dynamic-field literals now allocate per evaluation
  (`faed1779`). Native and wasm 2-item fixtures read `alpha` then `beta`.
  `cargo test --lib`: 3358 passed.
- `21:10` Remaining 1682-item failure is earlier: protocol-style parsed list
  reaches item 1363 then OOB; replacing stdlib `find_pair` with a pure defn
  makes `json_parse` fail immediately (`parsed=0`). A protocol 1682 fixture
  also fails after adding lookup defns. This is a compiler-wide
  function/liveness/stack interaction, not a JSON contract bug. Reverted all
  stdlib experiments; app browser gate still OOB in
  `browser_catalogue_scan`.
- `22:30` Stack A/B confirms the parser failure class. Protocol fixture:
  2/100/1000 parse clean on the default 64 KiB wasm stack; 1682 traps.
  Relinking the same 1682 object with `wasm-ld -z stack-size=262144` (and
  524288/1048576) parses and scans all 1682 items. The app still traps at
  `browser_catalogue_scan` at 512 KiB and 1 MiB; 2/4 MiB runs no longer report
  OOB but hang in the gate. So parser recursion is stack-bound; the app scan
  has at least one additional issue.
- `22:45` Attempted a broad plain wasm tail-loop enable. It exposed an
  unrelated i32/i64 loop-engine mismatch (`iter_chain_loop`) and was reverted
  before commit. Compiler remains at `a33f8137`; `faed1779` is the only
  codegen fix.

## Evidence / current state

- Compiler main: `faed1779` on `Randozart/briev-lang`.
- App branch: `compat/briev-main-2026-10-25` at `864c314`.
- Source guards: 505; compiler elides 176; 107 source cleanup; 283 fewer
  codegen guard branches than the original 612.
- `cargo test --all`: PASS (~3,350+ tests).
- `ll_determinism_check.sh --quick`: ALL DETERMINISTIC.
- Local environment: PostgreSQL 18.6 user-space trust cluster on
  `127.0.0.1:5432`, MovieLens 100k, Playwright Chromium.
- Recommender gates PASS: run, accounts, cache, operation-store,
  prediction-workers, worker, server, active-scratch, active-profile,
  profile-admission.
- Gate caveats: PGSCHEMA must be set for prediction-worker recovery;
  `recommender_server_properties` and I11 runner are absent from current main.

## To do

1. **Browser catalogue blocker. IN PROGRESS.** `Live` works, but state/catalogue
   responses are not reflected in the UI; the handle list loses the follow-up
   requests before drain. Isolate host event/transaction race vs compiler
   global-list mutation with a minimal WASM/browser repro before app changes.
2. **Compiler hardening invariants.**
   - assert allocation size >= max emitted field end after structural sizing;
   - verify emitted IR before invoking clang (invalid `%esrt0` class);
   - enforce ledger `unconsumed=0` in CI;
   - add proof differential/fuzz checks for prover soundness.
3. **Ledger direct consumers.**
   - loop/constructor facts: no direct runtime consumer yet (indirect via guard
     elimination);
   - alias/lifetime/independence proofs: reload removal/hoisting/vectorization.
4. **App acceptance.**
   - `browser_live_server.mjs` / I11 runner (bring from fork if needed);
   - `recommender_server_properties` property test (absent);
   - then mark PR #1 ready.
5. **Compiler delivery.** Decide upstream PR to `odecode/brief-lang` vs pinning
   the app to `Randozart/briev-lang` main.
6. **Stability Phase 2.** Interpreter callable txns, raw-address → String,
   large `ws_encode`, differential interpreter/backend gate, generic callable
   txn specialization, `dyn Trait`/PHI defects.
7. **Maintain this report.** Add a timestamped entry after each landed step.

## Open risks

- Static layout/IR invariants are not yet enforced by the compiler.
- Proof soundness is heuristic; two bugs were caught by tests, not formal
  verification.
- App compiler pin is not upstream-synchronized.
- I11/property acceptance scripts are missing from current main.
