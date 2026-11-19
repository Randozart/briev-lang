# Feature-example coverage program (2026-11-19)

**Trigger.** #144 post-mortem discussion: what should the compiler/stdlib have
caught, what work remains, and would per-feature example files be worth it.

**Answer to the third question: yes, but the unit is the SPEC section, and the
gap is a ledger, not the examples themselves.**

## What exists today (verified in tree)

| Inventory | Count | Mechanism | Gap |
|---|---|---|---|
| `spec/SPEC.md` feature sections | ~145 (`##`/`###`) | — the denominator | no mapping from section → test |
| `tests/tier1/*.bv` | 66 | `daily_use_smoke::tier1_corpus_compiles_links_and_runs` — compile, link, **run** with `BOUND=5` | ad hoc (regression-driven names); only flagship fixtures assert golden stdout, most just "must not crash" |
| `learn-briev/*.md` ```briev blocks | 232 | none — never machine-checked (no test/script references learn-briev) | doc rot is invisible |
| `docs/architecture/features/` | 59 | prose docs | no test tie-in |
| `docs/architecture/primitive-coverage.md` | L1 ISA ledger | classes I/S/A/R/X, closure = "a gap is a bug" | the right *shape* — but only for hardware primitives |
| `scripts/conformance.sh` | corpus parse+typecheck per profile | stops before codegen | catches syntax/type rot only |

The recent bug class is exactly what per-feature golden fixtures catch: #133
(nested intrinsic call), #134/#135 (enum payload typing), #142 regressions
(Data ABI), bare-`term` parity, W3-1 coercion — every one was found by a small
focused fixture with a pinned output. None was found by a bulk suite.

## Program: five workstreams

### F1 — SPEC→fixture coverage ledger (`scripts/feature_census.py`)
Mechanical, cheap, do first. Fixtures declare their section in a front-matter
comment (`// spec: §9.6`); the script walks `spec/SPEC.md` headings, maps
declared tags, and reports **sections with zero fixture**, plus fixtures whose
tags don't resolve. Same shape as `g2_census.py`. The census is the deliverable:
it converts "did we test §9.6 cells?" from archaeology into a query.

Baseline expectation: heavy coverage where regressions pushed it (enums,
contracts, lifetimes), thin or none at §9.6 cells, §11.6 critical sections,
§15.4 portable SIMD, §17 reflection, §18 macros/quotation/`$txn`, §12.2
spawn/await, §13.1 MMIO pins, §16.5 slicing, §19.4 variadics, §22 data-briev.

### F2 — fill the gaps, priority order, each with golden output + parity
Every new fixture carries an expected-output file AND runs both engines
(`brievc run --interpret` vs compiled binary) — outputs must match (Rule 5
mechanized; the bare-`term` bug was an interpreter/native divergence a
parity gate would have caught at fixture-add time).
Order: (1) zero-fixture sections with user-facing surface (cells, spawn/await,
slicing, reflection); (2) sections with only typecheck-level coverage (SIMD,
macros, `$txn`, critical sections); (3) profiles (§21 rbv, §22 .dbv/.dbvl —
some already have dedicated tests; the ledger cross-links them instead of
duplicating).

### F3 — upgrade the tier1 harness to assert, not just run
`tier1_corpus_compiles_links_and_runs` grows a `tests/tier1/golden/<name>.out`
convention: fixture has a golden file → assert stdout; no golden → current
behavior (run, require exit 0). Existing flagship goldens migrate. This makes
"really test them" literal: a feature demo that prints the wrong thing fails CI.

### F4 — stdlib header-conformance test (#144 follow-up, stdlib-land per Rule 14)
`tests/stdlib_header_conformance.rs` (Linux CI only, `#[cfg(target_os =
"linux")]`): parse `/usr/include` ground truth (`IPPROTO_TCP`, `AF_*`,
`SOCK_*`, `F_*`, `O_*`, `SIG*`, errno spellings used by stdlib) and diff
against the `const`s declared in `lib/std/net/*.bv`, `lib/std/posix/*.bv`.
Failure names the constant, the stdlib value, and the header value. This is
the oracle that would have caught `IPPROTO_TCP=0` at stdlib-commit time —
and it is stdlib knowledge evolving, not compiler knowledge.

### F5 — learn-briev companion examples (doc-rot kill)
Per chapter, the runnable examples become real files in `learn-briev/examples/`
(`ch05_cell.bv`, …), tested by the same F3 harness; the chapter blocks either
include the file's content or are edited to match it byte-for-byte, with a
conformance check script (extract ```briev blocks that carry a `complete`
marker → must equal the companion file). 232 blocks is too many to machine-
check as-is (most are fragments); the marker convention keeps fragments
honest about being fragments.

## Explicitly deferred from #144 discussion
- **Alias-dedup removal for Int consts**: IR-ergonomics only (an `i64`
  constant costs 8 bytes; the alias saves nothing and made correct IR look
  corrupt). Worth an A/B against `ll_determinism_check` + corpus size, but it
  is not correctness. Separate small plan if wanted.
- **Width-exact `mismatched_call_signatures` re-tighten** — still gated on the
  full corpus pass (unchanged from wave-3 notes).

## What else is left (context map, from INDEX + open threads)
- **#137** Float32 storage width in interpreter Data casts — small, scoped,
  rides the `adapt_arg_to_param` machinery question.
- **G2 promotion** — after migrating the 7 checkpoint sites (census exists).
- **A1/A2** of the production-correctness strategy (controls + full
  observations) — the umbrella's own queue.
- **XPU X items, K3 edge budgets** — blocked on PTL access / owner numbers.
- **Kiosk pin advance** — other session, blocked on the +1 RSS question.
- **#4 (macOS), #2 (C static library)** — external host / feature scope.

## Sequencing
F1 census first (a day), F4 conformance test next (small, prevents the next
#144), then F2/F3 together (the harness upgrade makes the new fixtures
assert), F5 last (largest edit surface, mostly docs). Each lands with its own
commit + quick gate; F2 fills in subsequent commits as gaps are named.

## Success criterion
`scripts/feature_census.py` reports 0 sections in the active surface without a
fixture tag; every tier1 fixture either has a golden or is a library-surface
build; stdlib numeric constants provably equal the platform headers on Linux
CI; tutorial companions compile+run with pinned outputs.


## Execution record (2026-11-19, same day)

All five workstreams landed (F1–F5) plus two REAL bugs the campaign caught:

- **F1** `79aaa1a1b` — `scripts/feature_census.py`; 66 fixtures tagged,
  baseline 20/143 sections covered → 21 after F2. The census's own
  first query named §16.5 slicing as uncovered.
- **F4** `ae69c2b4f` — `tests/stdlib_header_conformance.rs` (compiled C
  probe + full SYS_* sweep vs `unistd_64.h`). Bite-proven; immediately
  caught **SYS_SETRLIMIT = 98 (getrusage!) → 160**.
- **F3** `dbdbedaa6` — tier1 `<name>.golden` sidecars in the daily-use
  harness; 9 pins (4 new bug-class + 5 revived orphans with stale
  trailing newlines corrected). Bite-proven.
- **F2** `69cd99c18` — §16.5 fixtures caught TWO bugs:
  (1) heap-`List` slicing silently returned the WHOLE list (no arm in
  `emit_slice_with_array`; fixed: tier-list `{data_ptr, len, cap}` arm
  through the strided gather);
  (2) the strided gather helper computed INCLUSIVE stop (`span/s + 1`)
  against the SPEC's Python-style EXCLUSIVE — fixed in both stdlib
  copies (`coll_strided_len`), repairing the pre-existing state-column
  route too. Full account in BUGS.md.
- **F5** `c8ecdd6ed` — four chapter companions (01/04/05/06) with
  byte-pinned goldens via `tests/learn_briev_examples.rs`; chapters
  link to them; bite-proven.

Census after the campaign: **21/143 sections covered**; the uncovered
list is the standing work queue (9.6 cells, 11.6 critical sections,
12.2 spawn/await, 13.1 MMIO, 17.x reflection, 18.x macros, 22.x
data-briev, ...).
