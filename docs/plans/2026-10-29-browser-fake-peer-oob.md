# Browser fake-peer `movie_cards` OOB

**Date:** 2026-10-29
**Status:** RESOLVED — 2026-10-29
**Scope:** wasm browser runtime, reactive list view materialization, and the
deterministic recommender fake-peer gate.

## Problem

`tests/fixtures/recommender/browser_pending_peer.mjs` fails deterministically
with:

```text
RuntimeError: memory access out of bounds
    at app.wasm.__view_items_movie_cards
```

The failure survives compiler changes from `b1de89fc` through the PR #48 merge
candidate. The real browser/server I11 gate passes at the same commit, so this
is specific to the fake-peer event/response path or a latent wasm list/view
defect that I11 does not reach.

The fault is in the wasm view materializer stack, not Otto’s pooled-object
patch. PR #48 therefore remains unmerged pending this gate.

## Current artifacts

- Compiler merge candidate: local branch
  `local/pr48-validated-pending-browser` at `0b581437`.
- Otto PR #48 head: `e052b070`.
- Compiler `origin/main`: `569cf69b` (enum-return correctness hotfix).
- App branch head: `66f4843`.
- Failing generated app: `target/recommender-pr48-merged/app.wasm`.
- Real-server control: I11 runner passes with `maxLongTaskMs=0`.

## Resolution

Two independent browser-runtime defects combined:

1. **Snapshot under-allocation.** `__view_items_<field>` reserved
   `(count + 1)` i64 words but wrote `row_size` words per item. A four-field
   `MovieCard` row overran by `3n` words; the fake-peer heap frontier exposed
   the overrun at row 8. Allocation now reserves `1 + row_size * count`.
2. **Detached flush DataView.** `_applyFlush` read all records from one
   DataView while applying effects. A b-each effect could materialize a view
   and grow wasm memory, detaching that DataView for the next record. Flush
   records are snapshotted before any effect runs.

I11 previously passed because spare heap followed the under-sized buffer and
its render timing did not force a growth between flush records; both were
latent correctness bugs.

Evidence:

- `webstack_view_snapshot_reserves_all_item_fields`
- `test_type_aware_flush_decode`
- `browser_pending_peer.mjs`: passes twice
- I11: exit 0, `maxLongTaskMs=0`
- all seven recommender PostgreSQL gates: pass
- `cargo test --all`, full determinism, effect-shape: pass

## Goal

Root-cause and fix the `movie_cards` fake-peer OOB without weakening the
1682-item browser gate or Otto’s patch.

Non-goals:

- merging PR #48 with a red browser gate;
- hiding OOB behind a JS bounds check that discards state;
- changing the browser fixture movie count merely to make the failure disappear;
- using I11 as a substitute for the deterministic fake-peer contract.

## Diagnostic plan

### 1. Instrument only generated artifacts

Copy or use `target/recommender-pr48-merged/app.mjs`; do not commit generated
diagnostics. Around the `movie_cards` view effect:

- log `snapPtr`, materialized `n`, and wasm memory length;
- validate the full snapshot span `8 + n * 32`;
- log the snapshot header and, if safe, the first row;
- identify whether the OOB occurs before or after the wasm export returns.

Also inspect the wasm state field directly where practical:

- `%State` field 10 is `movie_cards`;
- log its raw i32 handle and the list header words (`data`, cap, length) before
  invoking `__view_items_movie_cards`.

### 2. Reduce the fake-peer trigger

Create a temporary variant of `browser_pending_peer.mjs` and vary one factor at
a time:

- movie count: `1`, `12`, `13`, `100`, `1682`;
- ratings count: `0`, `1`, `5`;
- initial recommendations event: present/absent;
- delay before catalogue response: `0ms`, `50ms`;
- catalogue search state: default versus forced page/list path.

Record the smallest failing shape and the exact OOB instruction/stack.

### 3. Compare real-server and fake-peer paths

Use I11 as the control:

- compare HTTP response headers/body shapes;
- compare login/state/movies event ordering;
- compare WebSocket recommendation event ordering;
- verify whether `http_event` fires while a prior view flush is pending.

If the same app and payload pass under I11, add the missing ordering or payload
shape to a reduced deterministic browser gate only after the root cause is
known.

## Candidate causes

1. **List publication:**  
   `movie_cards = client_movie_cards_fill(...)` stores a stale/invalid handle.
2. **Recursive fill ABI:**  
   `client_movie_cards_fill` is a lowered tail loop; its returned list handle
   may be frame-owned or incorrectly published.
3. **Slice/page list ownership:**  
   `page.movies` from `browser_catalogue_page` may be a temporary view whose
   backing buffer is not retained by `movie_cards`.
4. **View materializer trust:**  
   `__view_items_movie_cards` trusts list length but not capacity/data validity.
5. **Event ordering:**  
   A pending `http_event` flush uses a list snapshot that another event has
   already reallocated.

## Fix policy

Fix the responsible layer:

- compiler list publication or recursive-return ownership;
- collection/slice retention;
- wasm view ABI;
- browser runtime only if the generated contract itself is wrong.

Do not fix app code to conceal a compiler/runtime lifetime defect.

Add a regression at the lowest reliable level:

- reduced deterministic browser gate, preferred;
- wasm integration fixture, if browser-only is impractical;
- backend IR assertion if the defect is visible before execution.

## Validation

Compiler:

```bash
cargo test --all
bash benchmarks/ll_determinism_check.sh
bash benchmarks/effect_shape_guard.sh --quick
```

Recommender:

- all PostgreSQL gates;
- `browser_pending_peer.mjs`, run at least twice;
- I11 runner, requiring `maxLongTaskMs=0`;
- targeted baseline A/B if list/view codegen changes.

PR #48:

- `pooled_object_replacement_survives_reactor_firings`;
- `top_level_instance_assignment_seeds_columns`;
- merged tree equal to the validated tree.

## Landing and attribution

After green:

1. Land the OOB fix on compiler `main`.
2. Fetch PR #48 again and validate the exact GitHub merge tree.
3. Merge PR #48 through GitHub with a merge commit:
   ```bash
   gh pr merge 48 --repo Randozart/briev-lang --merge
   ```
   Do not squash; preserve Otto’s authorship.
4. Fetch the remote merge commit and verify its tree.
5. Update recommender PR #1’s compiler pin to the final post-merge SHA.
6. Record Otto’s PR SHA, merge SHA, evidence, and the OOB root cause in the
   v0.1 ledger.

Keep `v0.1.0-preview` blocked on the separate `ws_frame` enum-return
performance regression after this lane closes.
