# Open PR triage — 5 open PRs against `main` (2026-11-18)

**Status:** PLAN / research complete. No code changes made.
**Scope:** `#125`, `#126`, `#127`, `#128`, `#129` (all authored by @Randozart).
**Question asked:** do any of these describe work already fixed on `main`?

## TL;DR

| PR | Topic | On `main`? | Verdict |
|----|-------|-----------|---------|
| #129 | `webstack_opts` initializes the new `BuildOptions` fields | **yes** (its two fields) | **Obsolete content, live hazard.** Close it — but `main` is red from the *same helper*, missing `march_override` (mine, `daa0f1479`). Fix on `main` first. |
| #125 | `brievc <cmd> --help` prints usage instead of being read as a file | no | **Genuine.** Land, but the sweep is too blunt and it has no test. |
| #126 | `check` honours `--stdlib-path` | no | **Genuine, well-scoped.** Two defects to tighten first. |
| #127 | no convergence warning on `[beginprogram]` entries | no | **Genuine and reproduced — but incomplete.** Its arm misses `[beginprogram && cond]`, which I reproduced warning. |
| #128 | `net/server_loop.bv` lifecycle module | no | **Genuine new stdlib surface.** Three contracts need review; unverifiable in plan mode. |

**Order matters:** `main` is currently red (`error[E0063]: missing field
'march_override'`), so every branch cut from `main` is red for a reason that
has nothing to do with its own change. Fix that first, then land the rest.

---

## Evidence

### `main` is red, and the cause is `march_override`

CI run `44419325395` (commit `efd99d7b5`):

```
job: Workspace tests            -> failure
  error[E0063]: missing field `march_override` in initializer of
                `briev_compiler::pipeline::BuildOptions`
job: Quarantined tests (non-blocking) -> failure (same error)
job: Default gates              -> failure
  "error": "required jobs did not succeed: workspace (failure)"
```

Every other job in that run is green (Library tests, verifier, contract census,
release build, all five `Native (...)` jobs, Browser gates). So there is exactly
**one** root cause behind the three red jobs.

Static confirmation — `BuildOptions` has 59 fields, the helper sets 58:

```
$ python3 <field-diff script>
struct fields: 59
helper sets:   58
MISSING in helper: ['march_override']
```

`webstack_opts` lives in `#[cfg(test)] mod tests` of **`src/compile.rs`**
(the bin target). So the breakage is visible only to `cargo test --bins` /
`--all-targets` / `--workspace` — which is why `scripts/gate.sh quick` (nextest
`--lib` + focused integration binaries) is green while CI is red.

The field arrived in **my** `daa0f1479` (“make `-march` config-driven (W1-B)”) —
the same commit that added `--march`. This is the **third** time this exact
helper has broken:

1. `verify_ir` (fixed in the helper, comment dated 2026-11-18),
2. `deny_accel_cpu_fallback` + `validate_ir` (that is PR #129 — and both fields
   are already present on `main`, so **#129's patch is already applied**),
3. `march_override` (open, red).

### `brievc <cmd> --help` (G1 / #125) is still broken

```
$ ./target/debug/brievc check --help
error: cannot read '--help': No such file or directory (os error 2)
error: unknown flag: --help
exit=1
$ ./target/debug/brievc build --help
exit=1
```

Top-level `help|--help|-h` exists (`src/main.rs:89`); help *after* a subcommand
does not. `src/main.rs:20-23` only special-cases `args.len() < 2`.

### `check --stdlib-path` (G0 / #126) is worse than “silently ignored”

`src/main.rs::run_check`:

```rust
let validate = args.iter().any(|a| a.as_str() == "--validate");
let file_path = args.iter().find(|a| a.as_str() != "--validate")
    .ok_or("missing file argument")?;
```

`find` returns the **first non-`--validate` token**, so
`brievc check --stdlib-path /x f.bv` uses the literal string `--stdlib-path` as
the *file path* → `cannot read '--stdlib-path'`. And
`brievc check f.bv --stdlib-path /x` reads the right file but drops the path,
because `check_source` → `check_source_for` builds a **default** `BuildOptions`
(`src/pipeline.rs:1023-1035`) that carries no `stdlib_path`. The plumbing itself
is fine once passed: `opts.stdlib_path` reaches the resolver at
`src/pipeline.rs:1222` and `:1347`.

### `[beginprogram]` convergence false alarm (G4 / #127) is real — and wider than the PR

```
$ cat /tmp/prcheck/cli2.bv        # node main [beginprogram][true] { print_str(...); done = 1; term; }
$ ./target/debug/brievc check /tmp/prcheck/cli2.bv
warning: error: reactive txn 'main' has precondition [] but body writes to [done].
The txn does not modify any precondition variable and depends on other txns to
satisfy its convergence. If no other txn changes the precondition to false, this
txn will run forever.
OK
```

The declarative-conjunct form is **also** affected, and #127's arm would not fix it:

```
$ cat /tmp/prcheck/cli3.bv        # node main [beginprogram && done == 0][true] { ... }
$ ./target/debug/brievc check /tmp/prcheck/cli3.bv
warning: error: reactive txn 'main' has precondition [done] but body writes to
[nothing]. ... will run forever.
OK
```

`docs/architecture/briev-execution-model.md` §`beginprogram` — Entry Sugar:

> `beginprogram` is a keyword usable as a **conjunct** in a node's precondition.
> … the compiler desugars it to a per-node entry flag (`@briev_begin_<name>`)
> that is true exactly once at program start.
> **Entry loop rules:** entered exactly once at program start; **precondition
> evaluated once, never re-checked during the loop**.

So “the precondition never advances, therefore the txn re-fires forever” is
simply the wrong question for *any* pre containing a `beginprogram` conjunct,
and the existing helper `strip_beginprogram` (`src/analysis/accel.rs:363-380`,
currently private) already encodes the right decomposition for
`[beginprogram && i < N]`.

`is_reactive` is **not** the lever: it is set by the keyword (`txn` vs `node`)
in `src/parser/definitions.rs:1233-1266`, so a `node` is reactive by
construction. Fixing it there would change `pgo`, `density`, `causality` and
`global_lifetime` semantics — reject that option.

---

## Per-PR findings and required changes

### #129 — close as superseded; fix the hazard on `main` instead

- Both fields it adds (`deny_accel_cpu_fallback`, `validate_ir`) are **already
  on `main`** (the helper even carries the dated comment explaining them).
- Its diagnosis is correct and its lesson is exactly what I re-broke.
- **Do not merge it** (it would be a no-op diff or a conflict). Close with a
  pointer to the fix commit.

Durable fix — replace the 58-field literal with the overrides that actually
differ from `BuildOptions::default()`. Measured differences (script diff of the
helper against `impl Default for BuildOptions`):

| field | helper | default |
|-------|--------|---------|
| `file_path` | `file_path.to_string()` | `String::new()` |
| `backend` | `BackendKind::Webstack` | `BackendKind::Llvm` |
| `int_bits` | `32` | `64` |

(`sysquery_overrides` differs only by path spelling, `HashMap::new()` vs
`std::collections::HashMap::new()` — equivalent.)

```rust
/// A minimal webstack `BuildOptions` for the `compile_view` unit tests.
/// Only the three values that differ from `BuildOptions::default()` are
/// stated; everything else spreads from the default, so a new field can
/// never break the bin test target again (this literal has broken it three
/// times: `verify_ir`, `deny_accel_cpu_fallback`+`validate_ir`, `march_override`).
fn webstack_opts(file_path: &str) -> BuildOptions {
    BuildOptions {
        file_path: file_path.to_string(),
        backend: BackendKind::Webstack,
        int_bits: 32,
        ..BuildOptions::default()
    }
}
```

This matches the intent already documented on `impl Default for BuildOptions`
(“literals that duplicate this field set piecewise … only `check_source_for` is
migrated here”).

**Gate gap to close alongside** (this is the real defect — the same bug has now
slipped through three times because `quick` never builds the bin's test cfg):
add a compile stage to the quick gate. `scripts/gate_selection.py::phases`
returns test *suites* only (`briev-compiler` lib + `QUICK_INTEGRATIONS` +
verifier), and `scripts/gate.py` executes them; neither can express
`cargo check --all-targets`. Two options, cheapest first:

- `scripts/gate.sh quick`: run `cargo check -p briev-compiler --all-targets`
  before `exec python3 -B scripts/gate.py quick`. One line, no gate.py surgery.
- Or a `Check` pseudo-suite in `gate_selection.phases` so the manifest records
  it (consistent with how the gate already accounts for missing tools).

Seconds on a warm tree, and it is precisely the error class `nextest --lib`
cannot see. Treat it as a gate, not a habit: prove it bites by removing a field
from `webstack_opts` and watching `quick` fail.

### #125 — land, with three changes

1. **Blunt sweep.** `args[2..].iter().any(|a| a == "-h" || a == "--help")`
   hijacks *program* arguments: `brievc run app.bv -- --help` is a legitimate
   request to hand `--help` to the compiled program. Restrict the scan to tokens
   **before the first `--`** (nothing after `--` belongs to the compiler).
2. **Usage goes to stderr.** `print_usage` is all `eprintln!`
   (`src/main.rs:127+`). An explicit help request is a successful request and
   belongs on **stdout**, exit 0. Add `print_usage_to(program, &mut impl
   Write)` and route: explicit request → stdout; `args.len() < 2` misuse →
   stderr (unchanged). Small, additive.
3. **No test.** Add one to `tests/cli_args_test.rs`, which already has
   `env!("CARGO_BIN_EXE_brievc")` and a `Command::new` pattern: spawn
   `brievc check --help`, assert exit 0 and that the usage text appears on
   stdout; and spawn `brievc run -- --help`-style case once (1) is implemented.
   Note: these tests build the plain bin target, so they are **not** blocked by
   the `webstack_opts` `E0063` (that is `#[cfg(test)]` of the bin); they can be
   written now.

Also: the PR carries an unrelated `cargo fmt` reflow of `find_changed_rbv`
(the `if mtime > *since` block). Harmless, but it belongs in a `cargo fmt`
commit, not in a G1 commit — mention it in the PR body or drop it.

### #126 — land, with two changes

1. **Silent `break` on a missing value.** `parse_check_flags` does
   `if let Some(value) = args.get(index + 1) { … } else { break; }` — so
   `brievc check --stdlib-path` with no value falls through to
   `missing file argument` (or silently ignores the flag if a file was given
   earlier). `build` already has the honest message
   (`"--stdlib-path requires a path argument"`, `src/main.rs:590`); reuse it.
2. **Dead env fallback.** `flags.stdlib_path.or_else(|| env BRIEV_STDLIB_PATH)`
   is redundant: `ImportResolver::resolve_stdlib_root` already reads
   `BRIEV_STDLIB_PATH` as rung 2 (`src/import_resolver.rs:425`), which is why
   the default-options path never needed it. Either drop the `or_else` (smaller
   diff, one source of truth) or keep it and say in a comment why the resolver's
   rung is insufficient.

Rest of the PR is sound and well-tested:
`check_source_with_options` mirrors the existing `check_source_for` shape
(`..Default::default()` + explicit field) and the tempdir test pins the
out-of-tree case. Keep both. Note `parse_check_flags` only supports the
space-separated form, which matches `build` (`src/main.rs:587`) — consistent,
leave it.

### #127 — land, with the predicate widened

The arm as written:

```rust
if matches!(pre, Expr::BeginProgram) { return results; }
```

misses `[beginprogram && cond]` — reproduced above. Replace it with a *contains*
test. The decomposition already exists as `strip_beginprogram`
(`src/analysis/accel.rs:363-380`, currently private); either make that
`pub(crate)` and use it, or add a 6-line recursive `contains` next to the check.
Several `contains` is clearer for this caller, because for
`[beginprogram && i < N]` the residue is the loop bound, not a convergence goal,
so a “strip then compare to `true`” test would still warn:

```rust
// src/analysis/provenance.rs
/// A `beginprogram` conjunct makes the node a one-shot program entry: its
/// precondition is evaluated ONCE and never re-checked (execution model
/// §`beginprogram` — Entry Sugar), so "the precondition never advances ⇒ it
/// re-fires forever" does not apply. Plain `node`/`txn` declarations are
/// untouched.
fn contains_beginprogram(pre: &Expr) -> bool {
    match pre {
        Expr::BeginProgram => true,
        Expr::BinaryOp(BinaryOpKind::And, a, b) => {
            contains_beginprogram(a) || contains_beginprogram(b)
        }
        _ => false,
    }
}
```

and at the top of `check_convergence_safety`, beside the existing
`Expr::Bool(true)` early return:

```rust
if contains_beginprogram(pre) {
    return results;
}
```

(If you prefer to share the code, make `accel::strip_beginprogram`
`pub(crate)` and assert on its result being reachable rather than deleting the
recursion — but do not duplicate the And-walk in two places, Rule 17.)

Keep both added tests; **add a third** for the conjunct form
(`[beginprogram && done == 0]`) so the widening is pinned — that is the case
that would have silently regressed.

Two adjacent warts to record, **not** to change in this PR:

- `src/pipeline.rs:1459` prints `eprintln!("warning: {severity}: {message}")`
  where `severity` is `"error"` → the doubled `warning: error:` prefix seen
  above, on a diagnostic that does **not** fail the build. Worth a separate
  decision (either make `error` bite, which would break the corpus, or stop
  calling it an error). Log it, don't smuggle it in.
- `unbounded` txns already `continue` at the call site (`src/pipeline.rs:1436`),
  i.e. the call site already special-cases entry-shaped nodes. Fine as is;
  the in-function guard is the better home because it protects all callers.

### #128 — verify, then land with contract review

New surface: `lib/std/net/server_loop.bv` (151 lines) + `tests/tier1/server_loop_smoke.bv`
(168 lines) + a `server_loop_golden_output` test in `tests/daily_use_smoke.rs`.
All the helpers it calls exist on `main` (`sock_tcp`, `sock_close`,
`sock_set_reuseaddr`, `sock_bind_listen` in `lib/std/net/socket.bv`) with the
contracts the module assumes. Doctrine is respected: this is stdlib (Rule 14),
not new Rust, and the header carries an honest scope + undo line.

Unverifiable in plan mode — **first action must be** `brievc check
lib/std/net/server_loop.bv` (and `cargo test --test daily_use_smoke
server_loop_golden_output`) on the branch. If it does not typecheck, that is the
whole review.

Contract review items (contract-first, Rule 1/2 — state the honest bound, do not
weaken):

| defn | stated | issue |
|------|--------|-------|
| `serve_accept(listener)` | `[true]` | reads `listener.fd`; the honest pre is `[listener.fd >= -4096]`, matching `serve_listen`'s post |
| `serve_bounded()` | `[true][term.len == 0 && term.len <= term.cap]` | post claims an **empty** buffer; if the intent is “a fresh buffer”, `term.len <= term.cap` alone is the real fact and `== 0` is a guess that will break when the builder pre-reserves |
| `buf_as_string(buffer)` | `[true][term .^Length == 0 \|\| term .^Length > 0]` | vacuous: `x == 0 \|\| x > 0` is just “non-negative”, which a `Length` already is. This is the `[true][true]` anti-pattern in disguise (the compiler rejects the literal form — “asserts nothing” — but not this). Drop it or state something falsifiable |

Test-shape risks to weigh:

- **Hard-coded port `39481`** (precedent: `39471` in `socket_loopback_smoke.bv`).
  Distinct from the existing test, so no collision between the two; but two
  concurrent CI jobs / a developer's local server on that port make it flaky.
  Cheap hardening: on bind failure print a distinct verdict and treat it as a
  skip, or read the port from an env var with the constant as default.
- The golden asserts `"1111111111200"` — a single character of drift in any of
  the five verdicts fails loudly, which is good; but it also means the test
  tells you almost nothing about *which* step broke. Consider printing the
  failing step's name (stderr) so CI logs are actionable.
- The test only `SKIP`s when `clang` is absent. Sandboxes that forbid loopback
  sockets would fail rather than skip — worth the same guard treatment.

---

## Proposed sequence

Ordered so that nothing is reviewed on top of a red tree.

**S1 — unbreak `main` (do first, alone).**
`src/compile.rs::webstack_opts` → the 3-override spread above.
Acceptance: `cargo check -p briev-compiler --all-targets` clean;
`cargo test --bins -p briev-compiler` compiles and the `compile_view` tests pass;
CI `Workspace tests` + `Default gates` green on `main`.

**S2 — close the gate gap (same PR or the next one).**
Add `cargo check --all-targets` to `scripts/gate.sh quick` / `gate.py` stages.
Acceptance: deliberately removing a field from `webstack_opts` makes `quick`
fail (self-test the gate — a gate that has never failed is untested).

**S3 — #129.** Close with “superseded by <S1 commit>; the hazard is fixed
structurally.” No merge.

**S4 — #126.** Apply the two tightenings, then merge (it is already based on a
green-at-the-time path and touches only `run_check` + `pipeline.rs`).
Acceptance: `brievc check --stdlib-path <lib> <tmp>/main.bv` prints `OK` from a
directory outside the tree; `brievc check --stdlib-path` alone errors with
`--stdlib-path requires a path argument`; existing `check` (no flag) unchanged —
the conformance sweep must stay green.

**S5 — #125.** Apply the `--`-boundary + stdout changes + test, then merge.
Acceptance: `brievc check --help` → usage on stdout, exit 0;
`brievc build --help` same; `brievc run app.bv -- --help` still forwards
`--help` to the program; `tests/cli_args_test.rs` covers both.

**S6 — #127.** Widen to `contains_beginprogram`, add the conjunct test, merge.
Acceptance: `brievc check` on `[beginprogram][true]` and
`[beginprogram && done == 0]` prints **no** convergence warning;
`reactive_body_that_never_advances_still_warns` still passes (the narrowing
test); the full corpus census shows no new diagnostics.

**S7 — #128.** Verify-then-review, then merge.
Acceptance: `brievc check lib/std/net/server_loop.bv` → `OK`;
`cargo test --test daily_use_smoke server_loop_golden_output` passes locally
twice in a row (the SO_REUSEADDR rebind claim); the three contract items above
settled one way or the other (fix or recorded reason); `bash scripts/gate.sh
quick` green.

**S8 — follow-ups (separate, logged not smuggled).**
- `warning: error:` doubled prefix + the fact that `severity == "error"` does
  not fail anything (`src/pipeline.rs:1459`).
- A `BuildOptions` construction-safety story: an `#[deny]`-style lint cannot
  see this, but a `Default`-spread convention plus the S2 gate step covers it.
  Consider documenting the convention on `impl Default for BuildOptions` (it is
  half-documented already) and in `docs/architecture/agent-reference.md`.

## Risks / what I am deliberately not doing

- **Not** rebasing or merging the PR branches in plan mode, and not closing
  anything: this document is the plan.
- **Not** touching `is_reactive` for `beginprogram` nodes (parser-level). It
  would ripple into `pgo`, `density`, `causality`, `global_lifetime`; the
  in-check guard is the contained, correct fix.
- **Not** promoting the convergence `"error"` to a hard failure. It is advisory
  by an explicit earlier decision (“warn, migrate, then error”,
  `src/pipeline.rs:1425-1428`); flipping it now would break the corpus and is a
  separate judgement call.
- **#128's `[true]` preconditions**: fixing them is a contract change that adds
  call-gate obligations at every caller (the W4 lesson). Verify the callers can
  discharge them before tightening; if not, record why rather than weakening.

## Verification log (this research)

Reproduced locally with the already-built `./target/debug/brievc`; nothing in
the repository was modified.

| check | command | result |
|-------|---------|--------|
| `main` CI cause | `gh run view 44419325395 --log-failed` | `E0063 missing field 'march_override'` in `Workspace tests` + `Quarantined tests`; `Default gates` fails transitively |
| field gap | static diff of helper vs `impl Default` | 59 fields vs 58 set; only `march_override` missing |
| #129 already applied | read `src/compile.rs::webstack_opts` | `deny_accel_cpu_fallback` and `validate_ir` present |
| G1 | `brievc check --help` / `brievc build --help` | `cannot read '--help'` / exit 1 |
| G0 | read `run_check` + `pipeline.rs:1023` | first non-`--validate` token taken as the file; `stdio_path` dropped by default opts |
| G4 | `brievc check` on `[beginprogram][true]` | convergence warning reproduced |
| G4 widening | `brievc check` on `[beginprogram && done == 0]` | convergence warning reproduced — #127's arm would not fire |
| #128 deps | `grep` of `lib/std/net/socket.bv` | `sock_tcp`, `sock_close`, `sock_set_reuseaddr`, `sock_bind_listen` all present with matching contracts |
