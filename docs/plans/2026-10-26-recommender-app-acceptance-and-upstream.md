# Recommender app acceptance + compiler upstream

**Date:** 2026-10-26
**Status:** ACTIVE — local PostgreSQL/MovieLens/Playwright available; most gates
pass; browser catalogue scan OOB remains
**App branch:** `compat/briev-main-2026-10-25` @ `864c314`
**App PR:** https://github.com/odecode/recommender-app/pull/1 (draft)
**Compiler main:** `faed1779` on `Randozart/briev-lang`

## Goal

Carry the recommender app from "renames + guards compile" to "acceptance
gates pass and the PR can leave draft", and make the two compiler
prerequisites available where the app is reviewed.

## Current state

| Item | State |
|------|-------|
| App rename | `944d4b57` (Matrix/Vector bridge names) |
| App contract guards | `435159b` (23 files, +768/−28) |
| Server check | `brievc check server.bv` → OK |
| Server build | links `target/recommender-demo/server` |
| Browser build | `app.rbv --split` → wasm/html/mjs |
| Compiler destructure fix | `e442f06b` |
| Compiler literal-field proof fix | `1c1dd70f` |
| BUGS.md record | `064aaaa6` |
| `cargo test --lib` | 3345 passed, 0 failed (2026-10-26) |

The app's contract errors went 119 → 0 with explicit `when !(...) { trap; };`
guards. No stdlib contract was weakened. The two compiler fixes are required:
tuple-destructuring inference errors are now propagated, and
`Type { field: value }.field` reduces to `value` in the proof engine.

## Environment probe (2026-10-26)

The local machine cannot run the full acceptance path:

- no `psql` / PostgreSQL client or server on PATH;
- no `u.data` / `u.item` in the app checkout;
- no `playwright` Node module.

Do not mark PR #1 ready until these are available and the gates pass.

## Gate matrix

### Run without external services

```sh
export BRIEVC=/home/randozart/Desktop/Projects/briev-lang/target/release/brievc
export BRIEV_STDLIB=/home/randozart/Desktop/Projects/briev-lang/lib/std
cd /tmp/opencode/recommender-app-pr

tests/fixtures/recommender/run_active_scratch.sh
tests/fixtures/recommender/run_active_profile.sh
tests/fixtures/recommender/run_worker.sh
```

Compiler hard gate:

```sh
cd /home/randozart/Desktop/Projects/briev-lang
cargo test --all
```

### Require PostgreSQL / MovieLens / Playwright

```sh
PGUSER=briev_test PGDATABASE=briev_recommender_test \
UDATA_PATH=/path/u.data UITEM_PATH=/path/u.item \
bash tests/fixtures/recommender/run.sh
bash tests/fixtures/recommender/run_accounts.sh
bash tests/fixtures/recommender/run_server.sh

PGUSER=briev_test PGDATABASE=briev_recommender_test \
PLAYWRIGHT_MODULE=/path/to/node_modules/playwright \
python3 benchmarks/recommender_responsiveness/run_i11_browser.py \
  --compiler "$BRIEVC" --output /tmp/recommender-i11-browser-run
```

`run_cache.sh` and `run_prediction_workers.sh` also need the PostgreSQL URL
and belong in this tier.

## Upstream compiler decision

`fork/main` (`odecode/brief-lang` @ `e8c4476a`) is an ancestor of the current
base (`8b48132d`), so it does not contain the two fixes. Two acceptable paths:

1. **Open a Briev PR upstream.** Branch from latest `odecode/brief-lang` main,
   cherry-pick `e442f06b` + `1c1dd70f` (+ the BUGS record), run
   `cargo test --lib` and `cargo test --all`, and link it from app PR #1.
2. **Pin the app to `Randozart/briev-lang` main.** Keep the companion commit
   list in the app compatibility doc and require reviewers to use that compiler.

Prefer (1) if upstream accepts small proof/typechecker fixes; otherwise (2).

## Exit criteria

- `cargo test --all` green after the compiler fixes.
- Non-PostgreSQL recommender gates green, results recorded below.
- PostgreSQL/MovieLens/Playwright gates green on an environment that has them.
- App PR body links the landed compiler change (upstream PR or pinned main).
- PR #1 leaves draft only after the runtime gates pass.

## After this lane

Return to `docs/plans/2026-10-25-stability-v0.1.md` Phase 2: interpreter
callable txns, raw-address → String, large `ws_encode` reactor path,
differential interpreter/backend gate, generic callable-txn specialization,
`dyn Trait` / PHI defects.

## Validation log

- 2026-10-26: server `check` OK; server build OK; browser `--split` build OK;
  `cargo test --lib` 3345 passed.
- 2026-10-26: fixtures still used `Vector`; renamed the seven fixture files to
  `FloatVector` before running gates.
- 2026-10-26: `run_active_scratch.sh` PASS (`empty=true short=true truncated=true`).
- 2026-10-26: `run_active_profile.sh` PASS under harness-exact LTO after the
  structural-size fix. Before the fix it passed without LTO and SIGSEGV'd at
  LTO; the stale `obj` size was the cause (BUGS.md).
- 2026-10-26: `run_worker.sh` now passes start/predict/install, generation-2
  prediction, shutdown and reap (`generation_two_result=true`). Recovery still
  reports `persistent_ipc=false` because it spawns a persistence worker that
  needs PostgreSQL; that path is expected to be blocked locally.
- 2026-10-26: guard-order fixes applied for terminal sentinels in
  `knn_argmax`/`knn_zero`/`mean_*`/`impute_*`/`sim_*`/`lookup_row`/`sc_fill_*`
  and frame-size/decode short-buffer paths.
- 2026-10-26: `cargo test --all` PASS (whole workspace).
- 2026-10-26: local acceptance environment: user-space PostgreSQL 18.6 on
  127.0.0.1:5432 (trust auth, `briev_test`/`briev_recommender_test`),
  MovieLens 100k at `/tmp/opencode/ml-100k/ml-100k`, Playwright Chromium.
- 2026-10-26: `run_profile_admission.sh` PASS.
- 2026-10-26: `run.sh` PASS; `run_accounts.sh` PASS; `run_cache.sh` PASS;
  `run_operation_store.sh` PASS; `run_prediction_workers.sh` PASS;
  `run_worker.sh` PASS including recovery.
- 2026-10-26: `run_server.sh` PASS: catalogue/reactor/json/api/io/protocol/
  loop and malformed-WebSocket-frame gates; the property test is absent from
  the repository and is reported skipped.
- 2026-10-26: `browser_pending_peer.mjs`: `Live` binding fixed (compiler
  `b7bdfb08`, `b-window` hyphen truncation); it now reaches the catalogue and
  fails waiting for the first recommendation, with the catalogue still
  loading. `browser_live_server.mjs`/I11 not run yet; I11 runner is absent from
  current main (lives on the fork I10/I11 branch).
- Gate fixes applied: terminal guard ordering (udata, baseline_flush,
  account_state(_ratings), reactor init/close), PGSCHEMA export for workers,
  receipt payload JSON round-trip, missing `result_object_gate` fixture,
  removed stale `fs_read_lifecycle_gate` reference.
- Remaining compiler bug: constructed `JsonValue` Object inside Array loses
  payload through a call boundary (BUGS.md); parsed JSON works.
