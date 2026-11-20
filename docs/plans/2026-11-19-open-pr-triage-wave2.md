# Open PR triage, wave 2 — seven PRs (2026-11-19)

**Status:** PLAN / research complete. No repository state touched.

Seven PRs appeared after the wave-1 triage (`2026-11-18-open-pr-triage.md`).
Six are new work by @odecode, one (#143) is the fix for issue #132 that the
compiler-health program listed as remaining. This document answers, per PR:
*is the defect real on today's `main`, is the fix right, and how does it land?*

## TL;DR

| PR | Defect real on main? | Fix sound? | Verdict |
|---|---|---|---|
| **#143** glue `protocols.bv` imports its ops | **yes** — verified import graph | yes, data-driven | **land first** (clean, closes #132) |
| **#139** `arg_count` contract in the wrong slot | **yes** — `[term >= 0]` is a *precondition* | yes, one line | **land** (trivial, no conflict risk) |
| **#136** release archive `config/config/` | **yes** — published v0.2.0 | yes, tested end-to-end | **land** (highest user value) |
| **#138** collection literal in an enum payload panics | **yes** — panic site still live | yes, expectation-threading, no name matching | **land** (a compiler panic) |
| **#142** interpreter `Data` category + JSON recursion | **yes** (interpreter is the reference) | yes, incl. a codegen panic | **land, medium risk** (touches emit paths) |
| **#140** restore `Spawn#`/`SpawnWithOutput#` | **yes, but nothing uses them** | yes, unifies capture | **land last, lowest urgency**; its own premise text is stale and must be corrected |
| **#141** webstack fixture drift | **already fixed** by `c5f5b7ed8` | identical patch | **close as superseded**; salvage its BUGS/plan write-up |

Ordering: `#143 → #139 → #136 → #138 → #142 → #140`, closing `#141` when
convenient. Ascending risk, smallest first.

**Root cause of every red CI in this batch:** `error[E0063]: missing field
'march_override'` — the fixture drift I fixed in `c5f5b7ed8`. It failed before
the PR's own tests could run, so their red badges are *not* evidence about their
content. #142's failing run (`44492529610`) was checked line by line: both
`Workspace tests` and `Quarantined tests` die on E0063 and nothing else.

---

## Evidence per PR

### #143 — glue `protocols.bv` imports the module defining its ops (closes #132)

Verified on `main`:

```
lib/glue/c.bv:81        import "glue/c/protocols.bv";
lib/glue/c/types.bv:9   import "glue/c/protocols.bv";
lib/glue/c/protocols.bv  (no imports at all)
defn cstr_to_briev / str_to_c / cstring_concat  -> defined in lib/glue/c.bv
```

So the generated file is **not self-contained**: its axioms name ops that live in
`c.bv`, and `c.bv` already imports `protocols.bv` — importing back would be a
cycle. The PR's fix is the only acyclic shape: move the lane ops into the
descriptor's `types_module` (`types.bv`, whose header already advertises "a place
for hand-written C-specific helpers"), have `protocols.bv` import it, and make
`types.bv` import only `std/cast_lanes.bv` (it needs `cstr_len`; on main it is a
9-line stub whose only statement is the protocols import). Result:
`types.bv → std` · `protocols.bv → types.bv` · `c.bv → protocols.bv`.

The generator change keeps the "no language knowledge" rule: it reuses the
existing `types_module` descriptor field and emits nothing when it is empty
(`target_with` test targets), with a test asserting the import line appears.
Doctrine-clean; 267/86 across 9 files, and it is the one PR GitHub reports
MERGEABLE/CLEAN with every check green.

Verify on land: `brievc render c --check` (the generator must reproduce the
checked-in file), `cargo test --test glue_gen_parity --test protocols_only_boundary`,
`cargo test --test str_to_c_no_memcpy` (the zero-copy regression that pins
`str_to_c`'s emitted shape — worth watching because the defn moves file).

### #139 — `arg_count` declares `term` in its precondition

On `main`:

```briev
defn arg_count() [term >= 0] -> Int { term Argc#(); };
```

A single bracket is the **precondition**, and `term` (the result) is not bound
before the call. It is nonsense as a requirement and it silently loses the
guarantee the author meant (`term >= 0`) from the postcondition slot. The PR
moves it to `[true][term >= 0]` — exactly right, and the smallest change in the
batch (93/8, mostly docs + a regression that evaluates the precondition before a
result exists and reproduces `UndefinedVariable { name: "term" }`).

Latent, and *maximally* latent: `arg_count` has **no callers at all** — a grep
over `lib/`, `examples/` and `tests/` finds only its own definition and
`ffi/gen_shm.bv`'s unrelated comment. So the fix changes no program's verdict
today; its value is that the helper stops lying about its contract before the
first caller arrives (and `[true][term >= 0]` is what any caller needs to
discharge a downstream `[n >= 0]`).

Watch on land: `tests/cli_args_test.rs` — the PR appends there, and I appended
two `--help` tests in `e1f6f1971`, so the conflict will be at the file tail
(keep both blocks).

### #136 — release packaging nests `config/config/`

The archive copy step copies into a directory it already created, so resources
land at `config/config/…` instead of `config/…`. This is the **most
user-visible** defect of the seven: it shipped in the published v0.2.0 archive,
so anyone unpacking a release gets an unusable layout. The PR's test runs the
real `scripts/package_release.sh` against a fixture build, extracts the archive,
verifies the checksum, direct/nested/hidden resource paths, executable mode and
repeatability after a stale staging file — i.e. the test would have caught it.

Confirmed in `scripts/package_release.sh` on main:

```sh
mkdir -p "$staging/lib" "$staging/config"     # line 25: the dir already exists
cp -R config "$staging/config"                # line 34: copies the DIRECTORY into it
```

`cp -R config dest/` with `dest/` present yields `dest/config/…`, i.e.
`$staging/config/config/…`. Either `cp -R config/. "$staging/config"` or drop the
`mkdir` of the leaf. Conflict surface is only `BUGS.md`. Verify:
`cargo test --test release_package`, `bash -n scripts/package_release.sh`.

### #138 — `Array([IntVal(3)])` panics in LLVM emission

The panic site is still live on `main` (`src/backend/llvm/emit_expr.rs:2413`,
"a bare list literal reached codegen without a collection type"). A compiler
**panic** on a program that type-checks is the most serious class here after a
wrong-code bug, so this lands.

The fix threads the resolved payload type into emission (`TupleElems::Contextual(exprs, types)`,
`emit_contextual_operand`, `try_emit_contextual_literal`) and reuses the path for
typed call arguments and match results. I grepped the backend hunks for name
comparisons (`== "…"`, `matches!`): none — it matches on `Option<&Type>`, so it
satisfies Rules 15/19/23 (no type-name or vocabulary matching).

Risk: moderate (emission ordering), but every change is additive on a path that
currently panics — there is no working behaviour to regress. Overlaps `emit_expr.rs`
and `emit_toplevel.rs`, which my `state_prefix` commit (`1f1e73523`) touched in
different regions; the rebase should be mechanical.

### #142 — interpreter loses `Data` as a category (JSON stringify recurses forever)

Diagnosis is credible and the failure is bad: `int_to_str` binds a buffer address
as `d: Data`, then `d as String` re-selects `int_to_str` → infinite recursion
(a 64 MiB stack also overflows). Because the interpreter is the semantic
reference (Rule 5), a wrong `Data`→`String` dispatch makes the *reference* wrong
for every JSON producer, not just one program.

The PR does three separable things:

1. **Interpreter**: keep `Data` distinct through bindings, calls, FFI
   marshalling, casts and membership; Data→String dispatches on the *value
   category* with an owned copy of the buffer (embedded NULs preserved).
2. **Codegen fix**: `double → i64` was an equal-width bitcast door that
   **panicked** because the guard compared LLVM spellings; now handled.
3. **Codegen fix**: promote mixed Float widths *before* deriving the result
   stamp, so either operand order produces a register/typing match — the old
   path emitted raw-mixed `fcmp`/`fadd` (invalid IR) depending on order.

Item 2 is a panic fix, item 3 is a wrong-IR fix; both are worth having
independently of the interpreter work. The PR also honestly scopes a known gap
(narrow-format raw casts need storage-format metadata the Float atom lacks —
tracked in #137, *not* claimed as fixed).

Risk: the highest of the batch for silent behavioural change (codegen paths), so
it needs determinism + corpus + the full lib suite, and its own
`--test interpreter_data_cast`. Overlaps `emit_expr.rs`/`helpers.rs` with my
`state_prefix` and `watchdog_fail` hunks — different regions.

### #140 — `Spawn#` / `SpawnWithOutput#` are wired but broken at runtime

The PR says "on upstream-derived `efd99d7b`, LLVM rejects both as unsupported".
On today's `main` that sentence is **stale**: both are in the signature registry,
both are lowered in `intrinsics.rs`, and the runtime defines the symbols. The
defect that *is* real is worse than a rejection, because it is silent:

```c
int64_t __briev_spawn(const uint8_t* cmd) { … system((const char*)cmd); … }   // briev_rt.c:269
```

`string_ptr` returns the raw register for a String operand, and a Briev String is
`[i64 len][bytes][NUL]` at a `ptr` — so `system()` is handed the **length
header**, not the command. `Spawn#("ls")` executes garbage. `ShellCmd#` does strip
it (`handle + 8`), which is exactly the inconsistency the PR resolves by sharing
one capture path; it also replaces the `fgets`/`strlen` capture (which loses
embedded NULs and mis-tracks multi-line length) with a byte-preserving
`[i64 len][bytes][NUL]` buffer.

Two consequences for how it lands:

- **Its premise text must be corrected** in the merge/squash message: this is a
  runtime ABI bug (header not skipped; NUL-truncating capture), not an
  "unsupported intrinsic". Provenance that misstates the cause is worse than no
  provenance.
- **Nothing imports `std/process.bv`** — no `.bv` in `lib/`, `examples/` or
  `tests/` does — so this is a dead-code revival. Highest risk (402/115 across
  11 files: C runtime, interpreter, normalizer, stdlib, tests) with the least
  consumer pressure. Land last.

Verify on land: its `tests/process_spawn.rs` (interpreter + native O0/O3, ASan,
lengths 0/4095/4096/4097/8192, binary output), plus a manual sanity run of
`spawn("echo hi")` — there is no other consumer to catch a mistake.

### #141 — duplicate; close it

Its patch is the same fix I landed in `c5f5b7ed8`: inherit `BuildOptions::default()`,
override only `file_path`/`backend: Webstack`/`int_bits: 32`. Same diagnosis, same
three overrides, same reasoning about `cargo test --lib` not building the binary's
test cfg. It cannot merge now (the literal it rewrites is already rewritten).

It carries two things main does not have: a `BUGS.md` entry and
`docs/plans/2026-11-19-webstack-options-defaults.md`. The write-up is worth
keeping **because it is the third recurrence** and it records the reviewer-side
evidence (the hosted job failing at `src/compile.rs:4064`). Salvage those two
files as a docs-only commit, then close the PR pointing at `c5f5b7ed8`.

---

## Landing mechanics

### Conflict map (computed, not guessed)

Files each PR shares with what `main` has changed since the PRs' base
`3dccbfc80`:

| PR | shared files |
|---|---|
| #136 | `BUGS.md` |
| #138 | `BUGS.md`, `src/backend/llvm/emit_expr.rs`, `src/backend/llvm/emit_toplevel.rs` |
| #139 | `BUGS.md`, `tests/cli_args_test.rs` |
| #140 | `BUGS.md`, `src/backend/llvm/intrinsics.rs` |
| #141 | `src/compile.rs` (the duplicate), `BUGS.md` |
| #142 | none (GitHub reports MERGEABLE) |
| #143 | none (MERGEABLE/CLEAN) |

**`BUGS.md` is the whole story for four of them.** Every PR prepends a dated
section at line 1, and `main` prepended one too (`40cad306b`), so they collide
with each other and with main on the same anchor. All the code conflicts are
trivial: `tests/cli_args_test.rs` conflicts only because both sides append at EOF;
`emit_expr.rs`/`emit_toplevel.rs`/`intrinsics.rs` conflicts, if any, are on my
`state_prefix` hunks in different functions.

### Per-PR procedure (identical shape)

1. `git fetch origin` (other sessions push to main concurrently), then merge
   current `origin/main` into the PR branch — never rebase away their history.
2. Resolve `BUGS.md` by keeping **both** dated sections, newest first; never
   drop another author's entry.
3. `cargo check -p briev-compiler --all-targets` (this is the check that was
   missing; it also proves the fixture class cannot recur).
4. Run the PR's own new tests, then `cargo test -p briev-compiler --lib`,
   `bash scripts/gate.sh quick`, `bash benchmarks/ll_determinism_check.sh --quick`.
5. Corpus: `brievc check` over `lib/ examples/ tests/tier1` — expect the current
   floor **286 ok / 36 fail**; a PR may only add to `ok`. (`#140` should improve
   `examples/process-spawn.bv` only if it also declares `getpid`/`getcwd`/
   `to_c_string`, which it does not — do not credit it with that file.)
6. Land with `git push origin HEAD:main` from the PR branch after a fetch, or
   cherry-pick onto main and push; either way the commit message states the
   measured before/after.

### Sequencing rationale

- **#143 first** because it is clean, small, closes an issue, and is independent
  of everything else — so it cannot be blamed if a later landing misbehaves.
- **#139 then #136**: both tiny, both with their own regression tests; #136 is the
  one with outside users.
- **#138 then #142**: the two emit-path PRs, landing after the trivial ones so a
  rebase failure is unambiguous. #142 last of the two because it changes float
  promotion for *every* binary/comparison, and determinism is its real gate.
- **#140 last**: largest surface, zero consumers, and its premise needs correcting.
- **#141 anywhere** — it is a close, not a merge.

## Risks and what I would refuse

- **Do not merge #140 with its stated premise.** The evidence says the intrinsics
  are wired and *silently wrong at runtime*; "LLVM rejects both as unsupported"
  is not true on current main. Landing it under that description would put a false
  root cause in the history.
- **Do not take any PR's "CI is red because of an upstream blocker" on trust.**
  It happens to be true for all four here — I verified `#142`'s log line by line —
  but that is exactly the claim a stale branch makes by default. Re-derive it.
- **Do not land #142 on library tests alone.** Its float-promotion change alters
  emitted IR for every mixed-width comparison; `ll_determinism_check` and the
  corpus are the only things that can see a regression there.
- **Do not rebase the branches.** Merging main into them preserves the authors'
  commits and their plan docs; a rebase would also re-resolve `BUGS.md` for every
  commit in the series.
- **Do not salvage #141 as a merge.** It is a duplicate patch; only its two docs
  files are new.

## Process lesson worth fixing

Five of seven PRs were based on `3dccbfc80`, a commit whose binary test target
**did not compile**. Their CI was red before their first line of code, so:
(a) their authors could not distinguish their own failures from the trunk's, and
(b) every review of them starts by subtracting a known-good-broken signal. That is
what `c5f5b7ed8` (fixture spread + `cargo check --all-targets` in the quick gate)
fixes; it is also the argument for landing trivial PRs quickly instead of letting
them age onto a moving trunk.

Second lesson: **`BUGS.md` is a conflict magnet.** Every PR prepends at line 1, so
N open PRs guarantee N-way conflicts on the same anchor. Cheap fix: require new
entries to be inserted *after* a stable marker line (or append at EOF under a
"newest last" convention), so parallel branches touch different offsets. Worth
adopting before the next batch.
