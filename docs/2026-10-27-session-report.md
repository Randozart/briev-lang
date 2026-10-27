# Session report — 2026-10-27

**Last updated:** 2026-10-27T13:58:46+01:00
**Status:** compiler and app build/check green; all local recommender gates pass
except the browser catalogue gate. Proof-consumption ledger is active.
**Companion PR:** https://github.com/odecode/recommender-app/pull/1 (draft)

This report is the running record. Append a timestamped entry whenever work
lands; do not rewrite history.

## Done

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

## Evidence / current state

- Compiler main: `141c083d` on `Randozart/briev-lang`.
- App branch: `compat/briev-main-2026-10-25` at `b12d316`.
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

- Browser catalogue failure is unresolved and blocks PR readiness.
- Static layout/IR invariants are not yet enforced by the compiler.
- Proof soundness is heuristic; two bugs were caught by tests, not formal
  verification.
- App compiler pin is not upstream-synchronized.
- I11/property acceptance scripts are missing from current main.
