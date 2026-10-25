# Otto contribution adoption

**Date:** 2026-10-25
**Status:** ACTIVE — attribution first; then low-risk compiler/view fixes, JSON,
stdlib expansion, semantics, and browser runtime
**Branch:** `feat/otto-adoption`
**Source:** `odecode/brief-lang` fork, especially
`integration/recommender-i10-i11` (`2c5940e8`) and the focused branches listed
below.

## Goal

Adopt the useful work Otto Westerlund (`@odecode`) produced while building the
recommender application and website, without merging his integration branch
wholesale and without losing his authorship or PR history.

The integration branch merged stale main `965dcedb`; our main is `73854d17`.
It is not a fast-forward source. Extract only missing fixes and stdlib modules,
revalidate each on current main, and preserve attribution at the commit and
repository level.

## Attribution requirements

Every adoption must satisfy all of these:

1. `docs/contributors/otto-westerlund.md` records the original PR, branch,
   commit SHA, adopted commit/PR, any adaptation, and verification.
2. `README.md` credits the full scope of Otto's contributions.
3. `AUTHORS.md` lists him as a contributor.
4. `NOTICE` acknowledges contributed portions without changing the
   Apache-2.0 WITH LLVM-exception terms.
5. Exact commits are cherry-picked with Otto as author where possible.
   Reconstructed work carries `Co-authored-by: Otto Westerlund (@odecode)` and
   the original SHA/branch in the commit body.
6. New PRs state “Originally authored by @odecode in …” and link the source.
7. Adopted source files that are new or substantially his carry a short credit
   line below the SPDX header.
8. Existing merged commits and closed PRs are never rewritten. Credit is added
   through the ledger and contributor files.

## Source inventory

### Already landed — credit only

| PR | Scope | Merge |
|---|---|---|
| #13 | LLVM `memcpy`/`memset` declarations | `61df4852` |
| #15 | static trait-constrained generics | `cb149de7` |
| #16 | `.sbv` synthesizability gate | `af90279e` |
| #17 | trait-default needs-state | `46cb385c` |
| #18 | same-named member liveness | `694cf083` |
| #19 | type-dependent needs-state | `65079455` |
| #20 | ByteSink trait | `301ecad1` |
| #21 | explicit-receiver dispatch | `632678e7` |
| #22 | TCP client/server | `d026aa5d` |
| #23 | const String global | `1fbddeb7` |
| #24 | dot-call stateful member | `760d54dc` |
| #25 | lexer CR escape | `8310d9c8` |
| #27 | `Malloc#` memory attribute | `581d1dad` |
| #28 | Float tuple destructure | `1fc4c073` |
| #30 | relative imports | `9fc2aa6e` |
| #31 | generic call literal types | `412d84c1` |
| #32 | match values beside terminating arms | `e6c59d4c` |

Closed/superseded:

- #26 HTTP/WebSocket/SHA1/Base64 — reconciled and landed in `8d929650`.
- #29 pointer provenance — closed as superseded by the structural provenance
  and effect-summary fix in `9f18bcb7`.

### Compiler/browser fixes still missing from main

| Fix | Source | Files |
|---|---|---|
| Ordinary collection rebinding | `d3684191` | new `collection_assignment.rs`; `emit_stmt.rs`, `mod.rs` |
| Logical short-circuit + mutable parameter slots | `5b9ac7f8` | `context.rs`, `emit_expr.rs`, `emit_toplevel.rs`, new `logical.rs` |
| Module metadata allocator | `a42faac5` | `context.rs`, `emit_toplevel.rs`, `mod.rs`, new `metadata_tests.rs` |
| Target-width constant globals/loads | `a42faac5` | `mod.rs`, `emit_expr.rs`, backend test |
| Exhaustive print traversal | `a42faac5` | new `ast/visit_mut.rs`; `print_plugin.rs` |
| UTF-8 view compiler cursor | `a42faac5` | `view_compiler.rs` |
| Browser heap and dynamic snapshots | `2f5ad2bf` | new `web_heap.js`; `web_generator.rs`; `mod.rs` |
| `b-show`/`b-hide` correction | `2f5ad2bf` | `web_generator.rs` |
| UTF-8 `b-each` strings and row order | `2f5ad2bf` | `web_generator.rs` |
| Helper-write DOM invalidation | `2f5ad2bf` | new `analysis/web_commits.rs`; context/backend |
| Lexical elaboration scope | `2f5ad2bf` | new `typechecker/elaboration_scope.rs`; typechecker |

### Stdlib work still missing from main

| Work | Source | Adoption note |
|---|---|---|
| Wrong-side `Result` unwrap trap | `dac3aa4a` | `lib/std/result.bv` + object gate |
| Strict JSON parser | `14f7f666` | port onto current JSON/wasm path |
| JSON stringify | `892dcd6c` | UTF-8, finite-number and nesting checks |
| PostgreSQL v3 client | `feat/net-db-postgres` `bfa1e7ef` | extended query protocol; trust auth only |
| PostgreSQL receive lengths | `fix/pg-receive-length` `db77a61b` | SQLSTATE/message preservation |
| Portable browser casting | `feat/portable-web-stdlib` `8e6c8be1` | `cast_portable.bv`, `prelude-web.bv`, portable float formatting |
| Browser events/WebSocket/fetch | `feat/portable-web-stdlib` | `web/events.bv`, `web/websocket.bv`, fetch verbs, host JS |
| Matrix/Vector/IntVec | `8e24a679` | rename to avoid `Matrix<T,R,C>` collision; adopt under numeric names |
| Sparse CSR | `3db9b920` | extract general CSR core; keep `RatingPopulation` as recommender example |

### Product-only / deferred

- recommender account service, cache, persistence, server, browser UI;
- recommender-specific fixtures and benchmark harnesses;
- direct import of the whole `examples/recommender` tree.

These remain reference/acceptance work, not stdlib.

## Adoption order

### Phase 0 — attribution foundation

- add contributor ledger, `AUTHORS.md`, README scope, NOTICE statement;
- record every merged, closed, pending, and deferred item above;
- establish the co-author/source-link rules for the following commits.

### Phase 1 — low-risk correctness

1. `Result` wrong-side trap + object regression.
2. UTF-8 view cursor + accented/CJK regression.
3. `b-show`/`b-hide` display correction.
4. `b-each` UTF-8 decoding, stable order, field refresh, trigger dedup.
5. Exhaustive AST visitor + print plugin replacement.
6. Collection rebinding seam + concat/object regression.

### Phase 2 — JSON conformance

- port strict parser and stringify without discarding current wasm fixes;
- add malformed/incomplete/trailing-input, Unicode, number and nesting gates;
- rerun Task Board structured browser gate and standalone JSON gate.

### Phase 3 — stdlib expansion

- PostgreSQL client + receive fix, byte-level gate, optional live-DB gate;
- portable browser cast/prelude, events, WebSocket, fetch verbs, host JS;
- rename and adopt numeric primitives;
- generalize CSR, keep recommender-specific population as an example.

### Phase 4 — semantics

- logical short-circuit and mutable parameter entry slots;
- loop-exit logical expressions, termination and dominance regressions;
- no eager fallback removal; additive arms only.

### Phase 5 — metadata and target width

- module-wide metadata allocator, TBAA prefix reservation, pending lifetime;
- target-width constant globals/loads;
- large-universe and cross-function reset tests.

### Phase 6 — browser runtime

- `WebHeap` with malloc/free/realloc/calloc/memcpy/memmove/memset and validated
  ranges;
- dynamic collection snapshots replacing `[1024 x i64]`;
- helper-write commit summaries;
- lexical elaboration scope;
- 1100-item Unicode snapshot, heap, Task Board and Chrome gates.

### Phase 7 — deferred product work

Only if requested: recommender account/cache/persistence/server/frontend.
Reusable pieces are adopted in Phases 2–3 first.

## Validation

Per change:

- `cargo test --lib`
- `cargo build --release --bin brievc`
- `bash benchmarks/rbv_gate.sh`
- changed-directory Praetor
- documentation update in the same commit

Phase-specific:

- `Result` object gate, including SIGILL wrong-side cases;
- accented/CJK/Unicode view directives;
- nested foreach/match/operand print expansion;
- collection concat/replacement + explicit arrow insertion;
- JSON malformed/incomplete/trailing/Unicode/stringify gates;
- PostgreSQL encoding, receive prefix, optional live SQLSTATE/recovery;
- matrix `2^53+1` `IntVec` and large allocation round-trip;
- WebHeap range/reuse/coalescing and 1100-item Unicode snapshot;
- Task Board structured Chromium gate;
- logical trap/counter at `-O0`/`-O2`;
- metadata uniqueness on large universes;
- determinism quick/full and full runtime suite before merge.

No performance claim is made by adoption. If logical/metadata/browser changes
alter generated code shape, compare against the pinned baseline before merging.

## Documentation

- Keep Otto's original plans/PR text as reference; never rewrite them.
- Add this plan and the contributor ledger to `docs/plans/INDEX.md`.
- Update architecture docs touched by adopted changes in the same commit.
- Preserve existing rationale comments; rewrite them only when the replacement
  mechanism changes.

## Acceptance

The adoption is complete when:

- every missing compiler/browser fix above is either adopted or explicitly
  rejected with evidence;
- every useful stdlib module is adopted under a conflict-free name or recorded
  as deferred;
- all new code is tested on current main;
- Otto is credited in the repository, in adopted commits, and in every new PR;
- the contributor ledger maps every original SHA/PR to its adopted result.

## Execution log

- 2026-10-25 — attribution foundation committed (`ae4c70c4`): plan, contributor
  ledger, README/AUTHORS/NOTICE.
- `b4e54d5e` — adopted Result wrong-side trap. The fixture forces the payload
  into an exit value so the trap is observable; unused-call elimination is
  logged as a separate OPEN bug.
- `4827fb16` — adopted UTF-8 view byte offsets, `b-show`/`b-hide`, UTF-8
  `b-each` decoding and keyed-row reorder. Task Board Chromium gate green.
- `ae9b2bf9` — adopted exhaustive print traversal; exported-definition walk
  added beyond the source version.
- `b1838ed5` — adopted ordinary collection rebinding; native gate green.
- `0db07d3c` — adopted closed-reflection liveness, target-width reflection
  Int results, and fixed boxed enum heap ownership through sret callees.
- Strict JSON parser/stringify: native parse/stringify gates pass; wasm
  object-field extraction still returns the wrong Bool payload. Parser swap
  deferred (BUGS.md) rather than shipping a broken Task Board. Compiler fixes
  found en route are retained.

- PostgreSQL client adoption stopped at current contract checking: the
  receive-length-corrected module has unproved buffer/socket bounds. Recorded
  in BUGS.md; do not weaken the bounds contracts. The reusable receive-length
  behavior remains part of the deferred PG branch.
- Matrix/Vector/IntVec adopted from `8e24a679` with conflict-free
  `FloatMatrix`/`FloatVector` names for coexistence with `Matrix<T,R,C>`;
  daily compile/link/run gate covers `2^53+1` in `IntVec`.
