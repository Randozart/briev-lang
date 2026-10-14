# Increment-3 close-out + syscallN retirement into one variadic `syscall`

**2026-10-14.** Two commits. Phase 1 closes the daily-driver os/fs increment
(the 5 uncommitted files); Phase 2 executes the typed-variadic plan's
retirement gate (2026-10-13 plan, D-item 5): `syscall0..6` retire into a
single declared stdlib composite.

## Phase 1 — Increment-3 close-out (Commit #1)

The 5 uncommitted files: `lib/std/os/fs.bv` (real file layer + selective
imports), `lib/std/string_c.bv` (`to_c_string`), `src/import_resolver.rs`
(Export-unwrap + selective-import), `src/backend/llvm/intrinsics.rs`
(SysCall# `emit_arg`), `tests/fs_smoke.rs` (2/2).

Blockers and their root cause:

1. **SPIR-V `image_plan_partitions_ssbo_and_writes_texel`**: the COMMITTED
   `plugins/parsed/prelude.bv` (+ `prelude-native.bv`) injects
   `std/os/fs.bv` into every unit; the new fs.bv declares STATE FIELDS, so
   every GPU target's SSBO state struct grows past the `ssbo_members <= 3`
   assertion. The prelude was never dirty — fs.bv's content changed.
2. **Conformance sweep**: historically option/result `unwrap` + syscall6
   collisions, delivered into every unit by the prelude→fs.bv import chain.
   fs.bv already carries selective imports; the residual failure is
   re-diagnosed at execution start (one sweep run) before the fix.

**Fix (both blockers, one cut):** fs.bv is a CAPABILITY, not a universal —
remove `Import$("std/os/fs.bv")` from `prelude.bv` and
`prelude-native.bv`. Programs that want file I/O import it explicitly
(Rule 14: stdlib is the extension mechanism; the prelude carries only
universals — bootstrap types, collections, cast lanes). cli.bv does not use
fs; `tests/fs_smoke.rs` imports explicitly; the `-EBADF` probe reason the
injection existed for is gone with the resolver fix.

Verification before Commit #1: conformance sweep, SPIR-V image_plan,
`fs_smoke`, `cli_args_test`, `daily_use_smoke`, full `cargo test --lib`
(expect 0 known failures), Praetor on changed dirs, BUGS.md entries
(state-field bleed; resolver Export-unwrap), `lib/std/README.md` status.

## Phase 2 — syscallN → variadic `syscall` (Commit #2)

The retirement gate from the variadic plan: the wrapper families are
boilerplate over an intrinsic that IS the API, declared twice with
conflicting contracts. The variadic machinery (shipped 2026-10-14,
`c6bda5ab`) retires them. NO splice-apply — that stays a v1.1 exclusion:
the body dispatches on `args.Count#()`, which the specialization pass
folds to a literal per instantiation, and the optimizer kills the six dead
arms.

### New surface (`lib/std/posix/syscall.bv`)

```briev
export defn syscall(nr: Int, ...args: Int[<=6]) -> Int
    [nr >= 0 && nr < 512][true] {
    term match args.Count#() {
        0 => SysCall#(nr),
        1 => SysCall#(nr, args[0]),
        2 => SysCall#(nr, args[0], args[1]),
        3 => SysCall#(nr, args[0], args[1], args[2]),
        4 => SysCall#(nr, args[0], args[1], args[2], args[3]),
        5 => SysCall#(nr, args[0], args[1], args[2], args[3], args[4]),
        6 => SysCall#(nr, args[0], args[1], args[2], args[3], args[4], args[5]),
    };
};
```

### Contract decision (contract-first)

`lib/std/syscall.bv` carried `[nr > 0][nr < 512]` — **`nr > 0` is a latent
bug**: `SYS_READ = 0` (and `SYS_FORK = 57`? no — the killer is read) fails
it on Linux. The number 0 is a VALID syscall number; the upper bound 512
is a real signal (x86_64 max ~447) and stays. The variadic surface takes
`[nr >= 0 && nr < 512][true]` — stronger than the uncontracted posix
family, corrected against the buggy family. BUGS.md entry documents it.
Postcondition stays unconstrained: syscalls return `-errno` on failure —
that IS the API.

### Deletions + migration

- `lib/std/syscall.bv` — deleted entirely (no `syscall0`, buggy contract;
  its doc comment's content is absorbed).
- `lib/std/posix/syscall.bv` — the `syscall0..6` export blocks deleted;
  `SYS_*` consts stay.
- Callers (mechanical `syscallN(x, …)` → `syscall(x, …)`):
  `lib/std/os/fs.bv` (4 sites), `lib/std/posix/io.bv` (6),
  `lib/std/posix/process.bv` (3+), plus any found by a full grep at
  execution time.

### Verification

- `tests/fs_smoke.rs` gains a `getpid` assertion:
  `syscall(SYS_GETPID) > 0` — E2E through the zero-rest instantiation
  (POSIX guarantees pid > 0, so it is a real assertion, not a smoke).
- IR check: instantiations `syscall__2` (close/exit: nr + 1),
  `syscall__3` (clone: nr + 2), `syscall__4` (open/read/write: nr + 3)
  exist; bodies show the folded single live arm (no match, no dead
  `SysCall#` calls).
- Full `cargo test --lib` (collision dissolved at both ends: one `syscall`
  declared once; fs.bv out of the prelude), conformance sweep, fs_smoke,
  cli_args, daily_use, rbv_gate, Praetor on changed dirs.
- Docs: this plan's status; variadic plan's retirement-gate section flips
  to DONE (executed via folded-match); SPEC §9.1 gains one line naming
  `syscall` as the first retired surface; BUGS.md `nr > 0` entry.

### How to undo

Revert Commit #2: the wrapper families return (they are in history), the
variadic surface is removed, callers re-import `syscallN`. The variadic
feature itself is independent and stays.

## Status (2026-10-14, session close)

**DONE — both phases shipped.**

- Phase 1 (`ee21c3f4` + `99ac1d71`): the PR-15 generic-defn fixes landed
  FIRST (they broke fs_smoke; found because the PR-stack verification ran
  only `cargo test --lib`, not the integration targets — LESSON recorded in
  BUGS.md), then Increment-3 with the prelude capability-scoping.
- Phase 2: the variadic `syscall` surface retired both wrapper families;
  callers migrated (fs.bv, io.bv, process.bv); the zero-rest instantiation
  is E2E-proven by `fs_syscall_variadic_getpid` (POSIX pid > 0).
- TWO more pass gaps surfaced by the sweep (fixed, unit-tested):
  1. `brievc check` never ran the specialization — the wiring had gone
     into `compile_to_typed` (build) only, violating the 2026-08-18
     check/build-divergence discipline. The sweep caught it the moment a
     variadic source joined the corpus.
  2. `export defn` wraps the decl in `TopLevel::Export` — the pass's
     index/rewrite/wipe/lookup sites now unwrap it; an exported variadic
     declares fine and its instantiation stays internal.
  Plus the constant-scrutinee match fold (general machinery: fold the
  Count#-literal dispatch to its live arm before the elems walk, or dead
  arms trip the out-of-bounds rule) — `rewrite_match_constant_fold`.
- Final gates: `cargo test --lib` 3080/0 (14 pass unit tests), fs_smoke 3/3,
  daily_use 3/3, cli_args 5/5, conformance sweep green, Praetor no new
  diagnostics in touched files.

### Fixup (2026-10-14, same day)

`d0705f5f` staged the Phase-2 edits but never staged the DELETION of
`lib/std/syscall.bv` — the pushed main still shipped the old wrapper
family (inert: nothing imports it; buggy `[nr > 0]` contract; contradicts
BUGS.md/SPEC). All local gates had run against the correct working tree
(the file was already deleted on disk), so no behavior changed — this is
a tree-hygiene commit. LESSON (third staging miss): commit by
`git status` sweep, never by file list.
