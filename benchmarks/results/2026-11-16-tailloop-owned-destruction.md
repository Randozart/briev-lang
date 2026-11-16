# 2026-11-16 — tail-loop owned destruction (M2): leak closed

Companion to `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §M2 and the
`BUGS.md` entry *"self-tail call leaks every iteration's owned temporary"*.

## What was broken

A self-tail recursion whose recursive call carries OWNED storage (`String`,
`List`) dropped the previous iteration's value at every backedge. The tail-loop
lowering stores the new value into the per-parameter slot and branches to the
header; nothing released the old value, and the frontend census could not see it
because the value is an inline argument, never a local.

The leak is quadratic in the recursion depth: the carried string grows one byte
per iteration, so iteration *i* allocates O(i) and none of it returns.

## Fix

Frontend fact (`src/hir/owned_storage.rs`):

* `OwnedTailParam { index, exit_transfers, free_first, storage }`, planned per
  defn by `owned_tail_params()`, exposed as
  `AnalysisResults::owned_tail_params` and mirrored into the LLVM context.
* Recorded only when the tail loop is actually lowerable for that defn
  (`hir::tail_recursion::build_tail_loop_plan`) **and** every parameter binds to
  one register slot (mirror of the backend's `tail_loop_i64_binding`) — a fact
  that names a defn the backend refuses would claim a fix nobody emits.
* Recorded only when EVERY call site in the program hands the value over:

  | argument shape | verdict |
  |---|---|
  | inline `fresh`-set call (`concat`, `fresh_string`, slice allocator) | handed over, `free_first = true` |
  | identifier the caller's body mentions exactly once (at that argument) | handed over; index joins `consumed` so the caller does not also release; `free_first = false` |
  | literal (static storage) | nothing owns it; `free_first = false` |
  | identifier the caller reads again, field/deref, anything else | **no fact** — the census keeps reporting the leak |

* `param_escapes_in_body()` rejects a parameter that reaches a retaining sink
  (escape/consume argument, container store, `Free#`, keep-hint), using the same
  mode ladder the pass uses everywhere else.

Backend:

* `tail_loop_enabled` admits owned-carrying defns on every target (the plain
  recursive form has nowhere to release).
* `emit_tail_owned_slot_release` (backedge) and `emit_tail_owned_exit_release`
  (base case) release through the same lane a `ReleaseOwned` statement uses, so
  the arena skip (String) and the `coll_destroy` availability gate (List) apply
  unchanged. `free_first == false` slots carry a `first` bit so iteration 1 — the
  caller's literal — is never freed.

## Measurement

Fixture: `tests/tier1/owned_tail_transfer.bv` (a `peel` recursion that shrinks a
`String` by one character per iteration). `SEED` is a runtime-sized environment
string, so nothing folds. `maxRSS` is the child process high-water mark from
`resource.getrusage(RUSAGE_CHILDREN)`.

| seed length | pre-fix maxRSS | post-fix maxRSS | output |
|---|---|---|---|
| 2 000 | 13.0 MB | 12.6 MB | `0` |
| 4 000 | 12.9 MB | 12.8 MB | `0` |
| 8 000 | 33.7 MB | 12.8 MB | `0` |
| 16 000 | 127.9 MB | 12.8 MB | `0` |
| 32 000 | — | 13.0 MB | `0` |

Pre-fix column: the same source compiled with the compiler at `35c0498aa`
(no fact, no loop lowering); its `@peel` is plain recursion. Post-fix: `@peel`
is a lowered loop containing `.tail_header_`, two release calls and no self-call.

Correctness gate: every run prints `0` and exits 0, so a double free or a free of
the caller's value crashes rather than passing quietly. The IR shape is asserted
by `daily_use_smoke::owned_tail_transfer_frees_at_every_backedge`.

## Deliberately not fixed

* A recursion entered with a caller-held local (the caller reads it after the
  call) is still a reported leak. Freeing the slot there would be a
  use-after-free in that caller; the transfer proof refuses it and the census
  keeps blocking `--deny-memory-ambiguity`. This is the honest outcome, not a
  regression.
* `tests/tier1/owned_tail_cleanup.bv` (`peel(source)` then `source .^Length`) is
  exactly that shape, so it stays plain-recursive and stays reported.
