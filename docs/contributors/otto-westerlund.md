# Otto Westerlund contribution ledger

**Contributor:** Otto Westerlund ([@odecode](https://github.com/odecode))
**Fork:** `odecode/brief-lang`
**Adoption plan:** `docs/plans/2026-10-25-otto-contribution-adoption.md`

This ledger records Otto's merged PRs, closed/superseded PRs, pending work,
and the upstream adoption of each item. Original PR bodies and plans remain the
authoritative design record; this file tracks credit and disposition.

## Merged upstream

| PR | Scope | Merge commit |
|---|---|---|
| #13 | LLVM `memcpy`/`memset` declaration fixes | `61df4852` |
| #15 | static trait-constrained generics | `cb149de7` |
| #16 | `.sbv` synthesizability gate wired into the frontend | `af90279e` |
| #17 | derive `needs_state` for monomorphized trait defaults | `46cb385c` |
| #18 | same-named member liveness closure | `694cf083` |
| #19 | type-dependent `needs_state` for monomorphized trait members | `65079455` |
| #20 | `ByteSink` trait over fixed/growing byte buffers | `301ecad1` |
| #21 | dot dispatch to explicit-receiver obj members | `632678e7` |
| #22 | TCP client/server over raw syscalls | `d026aa5d` |
| #23 | const String global emitted as `@str.N` | `1fbddeb7` |
| #24 | `%state` propagation through stateful dot calls | `760d54dc` |
| #25 | lexer `\r` escape | `8310d9c8` |
| #27 | `Malloc#`-wrapping defn is `memory(readwrite)` | `581d1dad` |
| #28 | Float tuple-destructure unboxing | `1fc4c073` |
| #30 | filesystem-relative import resolution | `9fc2aa6e` |
| #31 | inferred collection literal types at generic calls | `412d84c1` |
| #32 | match values beside terminating arms | `e6c59d4c` |

## Closed / superseded

| PR | Scope | Disposition |
|---|---|---|
| #26 | HTTP/1.1 + WebSocket + Base64 + SHA-1 stdlib | Reconciled into `8d929650`; unified WebSocket decoder retained, canonical vectors pinned, PR closed as superseded. |
| #29 | Volatile raw `Ptr<T>` scalar indexing | Closed as superseded by structural provenance/effect summaries in `9f18bcb7`; evidence retained in the PR and BUGS history. |

## Pending adoption

| Work | Source | Status |
|---|---|---|
| Wrong-side `Result` unwrap trap | `dac3aa4a` / `fix/result-wrong-side-unwrap` | adopted `b4e54d5e` |
| Strict JSON parser and framing | `14f7f666` / `fix/json-parser-progress` | adopted `dc342cad` + `66b33459` |
| JSON stringify | `892dcd6c` / `feat/portable-web-stdlib` | adopted `dc342cad` |
| Ordinary collection rebinding | `d3684191` / `fix/collection-assignment-rebind` | adopted `b1838ed5` |
| Logical short-circuit + mutable parameter slots | `5b9ac7f8` / `codex/boolean-short-circuit` | adopted `3afdc42d` + stale-terminated fix `fba17b79`; nbody gate green |
| Module metadata allocator | `a42faac5` | adopted `faff264a` |
| Target-width constant globals/loads | `a42faac5` | adopted `faff264a` |
| Exhaustive print traversal | `a42faac5` | adopted `ae9b2bf9` |
| UTF-8 view compiler cursor | `a42faac5` | adopted `4827fb16` |
| Browser heap + dynamic snapshots | `2f5ad2bf` / `fix/wasm32-web-runtime` | adopted `de574ea0` + `a2038053` |
| `b-show`/`b-hide` and UTF-8 `b-each` | `2f5ad2bf` | adopted `4827fb16` |
| Helper-write DOM invalidation | `2f5ad2bf` | adopted `a2038053` |
| Lexical elaboration scope | `2f5ad2bf` | adopted `87b9d607` |
| PostgreSQL v3 client | `bfa1e7ef` / `feat/net-db-postgres` | deferred — current contract gate rejects unproved buffer/socket bounds |
| PostgreSQL receive lengths | `db77a61b` / `fix/pg-receive-length` | included in deferred PG branch |
| Portable browser cast/prelude | `8e6c8be1` / `feat/portable-web-stdlib` | pending |
| Browser events/WebSocket/fetch | `8e6c8be1` | pending |
| Matrix/Vector/IntVec primitives | `8e24a679` | adopted `c0ff0617` as `FloatMatrix`/`FloatVector`/`IntVec` |
| Sparse CSR / rating population | `3db9b920` | pending generalization |

## Adopted compiler hardening (2026-10-25)
| Boxed tuple list index stride | discovered while porting strict JSON | `107c3017` |
| Interpreter scalar `.^^Bytes` + `Mod#` | discovered while porting strict JSON | `1317e8bf` |
| Collection `.^Length` target width | discovered while testing wasm stringify | `0d82ed9e` |



| Work | Source | Adopted |
|---|---|---|
| Closed reflection liveness + task-family DCE | `a42faac5` | `0db07d3c` |
| Target-width reflection Int results | `2f5ad2bf` | `0db07d3c` |
| Boxed enum heap ownership through sret callees | discovered while porting JSON | `0db07d3c` |

## Deferred product work

The recommender account service, cache, persistence, server, frontend, app
fixtures and benchmark harnesses remain in `examples/recommender` as reference
and acceptance work. They are not compiler/stdlib adoption targets unless
explicitly requested.

## Adoption rules

- Exact commits are cherry-picked with Otto as author where possible.
- Reconstructed work uses `Co-authored-by: Otto Westerlund (@odecode)` and
  links the original SHA/branch in the commit body.
- New PRs credit the original author and source.
- Existing history is never rewritten; this ledger is the durable attribution
  record.
- New adopted stdlib files carry a credit line below the SPDX header.

## Recommender application

- Repository: `odecode/recommender-app` @ `79234665` (publish `32491a3c`)
- Compiler recovery commit: `odecode/brief-lang` `894fcfa5` (parent `1c1d4028`)
- Plan: `docs/plans/2026-10-25-recommender-fixes-and-app-adoption.md`
- Status: app external/pinned; compiler bundles A–E being adopted separately.
  App licensing/credit must be extended to Otto before any vendoring.
