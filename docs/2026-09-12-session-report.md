# Session Report — 2026-09-12

**Branch:** `feat/electronics-features` (worktree `../briev-electronics-feats`),
merged with main (GPU PTX session's `5205f078`) before merge-back.
**Suite:** 2171 green, exit 0 at every commit. **Baseline worktree
`../briev-compiler-baseline` untouched (`5d1d7e45`).**

## What landed

| Commit | What |
|--------|------|
| `72c2146d` | **Named nets** — `net vcc:` prefixes a precondition conjunct and names the inferred equivalence class (`Expr::Named`, parsed in `parse_and`; 16-pass surface migration) |
| `4cf3c63c` | **Unit suffixes** — `3.3V`, `20mA`, `330R` (`Expr::UnitLiteral`; `is_unit_suffix` distinguishes from TaggedLiteral; `mA`→/1000 in the electronics proving) |
| `db50bf85` | SPEC 3.5 as-built for both |
| `85c3afad` | **Named-net conflicts** — two names on one node = hard error; root-safety fix (names resolve on FINAL union-find roots) |
| `67205aae` | **Power ratings (B5)** — `rating 0.25;`/`rating any;`; proven P = ΔV²/R per part: exceed → violation, missing → undeclared decision, within → proof fact. build.rs fresh-worktree bootstrap fix (`compiler-in-briv` dir) |
| `fa49c0a7` | Docs: power ratings as built |
| `2a9f5756` | BUGS.md — reactive realization gap investigated, deferred, findings stamped |
| `dd272cb3` | Plan: `2026-09-12-dynamics-causal-dag.md` + INDEX |
| `8ccbf54d` | **The causal DAG** — proven/weak edges, Tarjan cycles, liveness refusal, `--explain-causality`, 8 tests |
| `50abb3e4` | BUGS.md — LLVM experiment measured |
| `00918a65` | `docs/architecture/causality.md` — measured backend trust |
| `68ffe5f1` | Merge main (BUGS.md conflict: both entries kept) |
| `9f05b4e1` | DAG followups ledger |

Electronics chain now proven end-to-end: **V** drives → **I** fixpoint
(Ohm, dividers, KCL) → **P** dissipation → bounds + ratings checked — all
compile-time, all named (`net vcc:`), all unit-suffixed (`20mA`).

## Design decisions of record (locked with the author)

1. **The reactive model**: the program signals intent; the compiler owns
   realization. `[pre]` = eligibility; `[post]` = the declaration of
   completion. Compile-time wiring is the ORIGINAL design
   (`2026-06-15-trg-reactive-dirty-flag.md`); the tick loop is the
   fallback, not the semantics. Provable chains fire "into the outcome of
   X" (side effects preserved); provable cycles fold; unprovable cycles
   need a checkable liveness obligation in the post — never silent
   spinning. Rule 22 governs simultaneity.
2. **Plain txn** = contract carrier (as the electronics path already
   treats it); dead-code diagnostic at most — not a semantics fork.
3. **Backends are trusted until measured** (the LTO lesson, applied): no
   fusion codegen without a measured backend failure.

## The LLVM experiment (measured, session-crowning)

Canonical 3-node proven chain, harness-exact link:
1. The unoptimized emission ALREADY cascades proven chains within one
   pass (a committed body falls through to the downstream pre-check); the
   loop's only residue is one empty quiescence-confirm pass.
2. The fully-optimized binary's `main` is `xor %eax,%eax; ret` — the
   standard `-O3 -flto` pipeline folds the entire reactive program.

Consequence: Briev-owned fusion is measured unnecessary for foldable
shapes. The DAG's load-bearing value is what the backend cannot have:
compile-time liveness refusals (the oscillator now refuses instead of
hanging), the `--explain-causality` report, and future FSM proofs.

## End-state map

- **Electronics**: named nets + conflicts, unit suffixes, power ratings;
  deferred: per-pin tolerances, pin roles, LLVM/GPU representation,
  PinDecl beast round-trip
- **Dynamics**: causal DAG landed (proof of the model); followups ledger
  in the plan doc (deep Z3 verifier, FSM proofs, fact enrichment,
  instance fields, dispatch consumption, plain-txn diagnostic); deferred:
  epoll wake, sync\<g\> barrier, interpreter revival, fusion
- **OS-grade arc** (discussed, not started): finish the syscall stdlib
  (port remaining `briev_rt.c` lanes over `lib/std/posix` — fasta-style,
  A/B'd), kill clang (llc+lld, c-independence Phase 2), ISR/vector
  tables (Phase 9 split), epoll wake (intersects dynamics + POSIX)
- **GPU (main)**: warp_mh=4 shipped for f16acc (+2.9 TF at 4096³);
  invalid-A/B process bug recorded by the parallel session

## Working discipline (re-learned this session)

- Design intent lives in the June plans — read the design BEFORE the
  implementation, or you re-derive "semantics" from the fallback and
  misframe the whole model (happened; corrected by the author).
- One fork at a time for architecture decisions; depth over menus.
- Doc user corrections verbatim; the model is theirs.

## Next threads (offered, unpicked)

1. Finish the syscall stdlib (libc exit) — chosen then deferred in favor
   of wrap-up
2. ISR/vector tables (OS-grade flagship)
3. epoll wake (dynamics ∩ POSIX)
4. DAG followups (deep verifier / FSM proofs first)
