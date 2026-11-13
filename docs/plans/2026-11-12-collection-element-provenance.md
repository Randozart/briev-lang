# Collection element provenance — 2026-11-12

## Evidence and objective

Base: `94eb8583`. The external load-harness collector's functional gates pass but
its Valgrind losses scale at 224 bytes/sample (96 direct, 128 indirect),
reproduced at 1/5/20/40 samples under both emission paths. Returning/printing the
parser's directly computed `pid` is clean; changing only the selected field to
the collection-derived `minor_faults` introduces a barrier in `proc_stat_parse`.
A standalone loop-grown `List<Int>` whose element is returned in an object and
printed leaks 24 + 128 bytes; the single-append variant does not.

## Diagnosis

`BRIEV_DIAG_RECLAMATION=1` isolates the mechanism. With the direct scalar the
container's owning callable is not an escape barrier and its allocations are
reclaimed by the caller's region. With the collection element the owner appears
in the root graph's `escape_barriers`; as a barrier it suspends the caller's
region, its allocations fall back to untracked `malloc`, and the header and
backing store leak.

Cause: `<-` models the fresh backing store as an allocation stored through the
handle (`Constraint::Store(target, backing)`), so the storage proxy lands in the
handle's `memory`. `Constraint::Load` (index/deref/foreach) returned
`contents(base)` including that proxy, so a scalar element read appeared to alias
the container's storage and forced an escape barrier when the value escaped.

## Fix

Separate container *storage* from container *contents*. Storage proxies are
recorded in a dedicated `container_storage` set and excluded from
`Constraint::Load`; they remain in `memory` so `root_contents`/`reachable` keep a
container's storage alive when the container itself escapes. Applied to both the
AST (`analysis/reclamation`) and HIR (`hir/reclamation`) graphs to preserve the
parity gate. No type-name or vocabulary matching; the rule is structural.

Soundness of the exclusion: a container that escapes still reaches its proxies
through `reachable` (used for `escape_barriers`, `classify`'s returned/incoming
checks, and closed regions); a slice/copy and a borrowed syscall still use
`contents`, which keeps the proxies; only direct element reads drop them.

## Verification

- Standalone loop-grown repro: 24 + 128 bytes lost before, 0 after.
- Collector `collect.bv` at 1/5/20/40 samples: 96/480/1920/3840 direct before,
  0 at every count after.
- New native regression
  `tests/reclamation_runtime.rs::collection_element_reads_do_not_escape_their_container`
  (fixture `reclamation_collection_element.bv`), asserting output `63` and no
  definite/indirect/possible loss; fails on base, passes after the fix.
- `cargo test --lib`, `cargo test --test reclamation_runtime --test
  hir_reclamation_parity`, and the quick/full gates recorded in the branch.

## Documentation

`BUGS.md` and `docs/architecture/llvm-memory-management.md` record the storage
vs contents contract and the `container_storage` exclusion set. No contract or
codegen strategy changed; this is an analysis-correctness fix.

External evidence and remaining leak investigations (scheduled driver, server
exit) are tracked in the application repository's
`docs/plans/2026-11-12-load-main-revalidation.md` and
`docs/plans/2026-11-12-load-server-memory.md`.
