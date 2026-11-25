# Landing the @odecode PR stack (#161 → #162 → #163 → #164), 2026-11-24

**Status:** PLAN CONFIRMED — owner trusts the author, lands each PR subject to
SPEC compliance, and fixes remaining bugs as we go.
**Promote to** `docs/plans/2026-11-24-pr-stack-landing.md` on landing.

## Trigger

Four contributor PRs accumulated while wave-9/wave-10/D2 were in flight. All
branch from `31456c1b5` (the commit immediately before wave-10), so integration
must be verified rather than assumed.

## Research findings (evidence)

| PR | Scope | Finding |
|---|---|---|
| **#161** `fix/aarch64-brk-syscall` | +178/−3, 6 files | aarch64 arena `brk` asm passed **3 operands for 2 input constraints**; LLVM rejected every arena/collection module for `aarch64-unknown-linux-gnu`. CI's `targets` step compiled one trivial fixture per triple and never reached the arena path. Adds `scripts/cross_target_ir.sh` (self-calibrating: native success decides corpus membership, so no skip list can rot), a `cross-target` CI case, and a behavioural parity test. CI green on its branch. |
| **#162** `fix/hashmap-occupancy-init` | +154/−5, 6 files | **Root cause of the BUGS.md 2026-11-23 silent-node entry.** `HashMap.init` allocated `occupied` with `Malloc#` and never cleared it; every probe reads before writing; `Malloc#` is `@malloc`; LLVM folded the first load to `undef`, `br i1 undef` is UB, SCCP deleted the node's observable continuation. Fixed by one `new_occupancy(n)` owner shared by `init` and the rehash path, which explicitly declines to zero the allocator ("that would silently strengthen `Malloc#`'s declared semantics"). CI green. |
| **#163** `fix/target-aware-type-resolution` | +968/−21, 12 files | wasm32 width class: seven print-family call sites emitted `call i64 @sym` against an `i32` callee on 32-bit. **50 → 25** wasm32 IR-invalid; aarch64 0; native IR byte-identical on 11 fixtures; tier1 corpus ok. Adds plan records. CI green. |
| **#164** `fix/wasm32-read-narrowing` | +1370/−125, 13 files | Stacked on #163. Narrow handles at read: widths recorded by the producing loads and consulted at read/dereference. **25 → 17**; `cargo test --lib` 4013 pass; native tier1 ok; IR byte-identical ×8. **No CI on its own branch** (base is a branch). |

Verified by me before this plan:

- **No PR reintroduces `^^`** — zero added lines across all four (checked with a
  grep over each diff). Wave-10 is not undone.
- **Rule 19 clean**: the wasm width dispatch uses
  `struct_types.contains_key(n) || obj_types.contains(n)` — universe membership,
  not type-name literals.
- **Conflicts are exactly two files**, both from append-at-EOF:
  `BUGS.md` and `src/backend/llvm/tests.rs`.
- **Rehearsed**: #161 + #162 merged on a scratch branch, quick gate green, and
  #162's fixture prints `7`/`0` correctly on top of `::`.

## SPEC compliance audit (owner's condition)

Each PR is checked against the SPEC and the architecture pillars *before* it
lands. Named checks:

- **#161 → SPEC inline-asm rule.** An `asm` operand list must carry exactly one
  operand per non-clobber input constraint, and the syscall number must occupy a
  register constraint. If SPEC is silent, the rule is asserted by the new test
  and the gap is recorded in `docs/architecture/backend-contracts.md`.
- **#162 → `Malloc#` contract.** Confirm SPEC defines `Malloc#` as *uninitialized*
  (`@malloc`). If SPEC said zeroed, the fix direction would be wrong and the
  allocator, not `HashMap`, would be the defect. This is the load-bearing check.
- **#163 → width doctrine.** `Int` is the target word; a call site must match its
  callee's declared signature. Confirm the fix derives the return type from the
  signature rather than from a literal, and that
  `docs/architecture/backend-type-dispatch.md` reflects it.
- **#164 → "casting graph is the single source of truth" pillar.** *"Widths are
  now recorded by the producing loads"* could be a shadow type map. Audit that
  it records an SSA-value width for narrowing, not a second declaration of a
  type's representation, and that no codegen site learns a type name to do it.

## Invariants

1. **I-1 — no retired spelling returns.** After every merge, `grep -rn '\^\^'`
   over `lib tests examples benchmarks` (excluding `docs/archive`, `archive`,
   timestamped plans) returns zero live sites. Pinned by
   `docs_mention_only_the_current_spelling` and a corpus grep in the gate.
2. **I-2 — wave-10 is intact.** `x::Prop` still parses and emits; pinned by
   `frame_accessor_parses_every_property`,
   `frame_accessor_emits_identical_ir`-shape test, and the tier1 corpus.
3. **I-3 — goldens unchanged.** No tier1 `.golden` is edited by this landing.
4. **I-4 — native IR unchanged per PR.** For each of #163/#164, byte-compare the
   emitted `.ll` for a fixture set against `main` before that merge; the PRs claim
   identity on 11 and 8 fixtures respectively — re-verify, do not trust.
5. **I-5 — each merge gates.** `bash scripts/gate.sh quick` green after every
   individual merge commit, not just at the end.
6. **I-6 — the remaining wasm32 failures are recorded, not accepted silently.**
   #163 leaves 25 and #164 leaves 17 IR-invalid wasm32 programs. BUGS.md carries
   the current list with the `cross_target_ir.sh wasm32-unknown-wasi` command, so
   the number is reproducible and cannot silently grow.
7. **I-7 — CI substitute for #164.** #164 has no CI of its own; the full gate
   plus `ll_determinism_check.sh --quick` on the stacked tree is the substitute,
   and that fact is stated in the merge commit.
8. **I-8 — attribution preserved.** Merge commits (not squash), so the
   contributor's authorship and reasoning survive in history.
9. **I-9 — no bug is left OPEN that a PR closes.** BUGS.md statuses updated in
   the merge that closes them (the 2026-11-23 silent-node entry becomes FIXED,
   pointing at the 2026-11-24 root cause).

## Named tests / commands

| check | what it proves |
|---|---|
| `cargo test --lib -- inline_asm_syscalls_have_one_operand_per_input_constraint` | I-1/#161 rule |
| `tests/tier1/hashmap_insert_is_first_statement.bv` → `7`/`0` | #162 fix on top of `::` |
| `bash scripts/cross_target_ir.sh wasm32-unknown-wasi` | I-6: the recorded failure count |
| `bash scripts/cross_target_ir.sh aarch64-unknown-linux-gnu` | #163/#164 claim of 0 |
| `cargo nextest run --test daily_use_smoke tier1_corpus_compiles_links_and_runs` | I-3 |
| `bash benchmarks/ll_determinism_check.sh --quick` | I-4/I-7 |
| `bash scripts/gate.sh quick` per merge; `full` at the end | I-5 |

## Stages

- **S0.** Prepare: `git fetch` the PR refs; confirm `origin/main == 14dfd671d`.
- **S1. Merge #161.** Resolve `BUGS.md` + `tests.rs` (append-at-EOF: keep both
  entries/tests, newest-first for the log). SPEC check on inline asm. Quick gate.
- **S2. Merge #162.** SPEC check on `Malloc#` (uninitialized). Run the fixture.
  Update the 2026-11-23 BUGS entry to FIXED. Quick gate.
- **S3. Merge #163.** Re-verify native IR identity on the claimed fixture set.
  SPEC/doctrine check on signature-derived return types. Record the wasm32 count.
  Quick gate.
- **S4. Merge #164.** Audit the width recording against the casting-graph pillar.
  Re-verify IR identity. Quick gate + full gate + determinism (I-7).
- **S5. Land.** Push to `main`, watch CI 4/4, comment on and close each PR with
  what was verified.

## Undo path

Each stage is its own merge commit on `main`; reverting one merge reverts exactly
that PR. `BUGS.md`/`tests.rs` resolutions are additive, so a revert cannot lose
another entry.

## Risks

1. **#164's width map becoming a second source of truth.** Mitigation: the audit
   in S4; if it is a shadow type table, refactor it to read the casting graph
   rather than merge it as-is (owner's "fix remaining bugs as we go" covers this).
2. **`tests.rs` conflict resolution corrupting the file** (it happened in
   rehearsal — a hand-resolved brace left the module unclosed). Mitigation:
   resolve mechanically as `main's file + each PR's added block`, never by hand
   splicing.
3. **A partial wasm32 migration becoming permanent.** Mitigation: I-6 records the
   count and the command in BUGS.md so it is a visible work list.
4. **#164 untouched by CI.** Mitigation: I-7.
