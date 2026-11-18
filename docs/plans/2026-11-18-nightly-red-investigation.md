# The remaining red: `Verification nightly` — three separate problems

**Question asked:** is the HIR/AST defn-liveness drift still a problem?

**Answer:** it is still open, but **it is not what makes the nightly red** — and the
nightly is red for two *other* reasons that are trivial compared to it. I had
attributed the nightly red to the drift twice; both attributions were wrong.

Evidence below is read-only (workflow files, run logs, the local `cargo-nextest
0.9.151` — the same version CI pins).

## 1. The nightly has not actually run its suite since 2026-11-16

`.github/workflows/verification-nightly.yml:45-48`:

```yaml
run: |
  cargo nextest run --all -j 4 \
    -E 'not testname:analysis_carries_hir' \
    -E 'not testname:router_smoke_gate'
```

Two independent bugs in four lines:

* **`testname:` is not nextest syntax.** `cargo nextest help filterset` (local
  0.9.151, the version CI installs) documents predicates `test(…)`,
  `package(…)`, … and matchers `=string`, `~string`, `/regex/`. The parser rejects
  the expression:
  ```
  error: expected open parenthesis
  1 │ not testname:analysis_carries_hir
  error: failed to parse filterset
  ##[error]Process completed with exit code 94
  ```
* **`-E` is repeated.** The flag takes one filterset; the exclusion has to be a
  single expression.

Result: the job dies before running a test. Runs dated 2026-11-16 and 2026-11-17
fail this way. **The suite has been unobserved for two days** — so no claim about
the AST path's health can currently be made from CI.

Correct form (one expression, documented matchers):

```yaml
-E 'not (test(=analysis_carries_hir) or test(=router_smoke_gate))'
```

## 2. Before that, the nightly failed on a missing `/tmp/opencode`

The last run that actually executed tests (2026-11-15, `72ade68d`) failed on
**PTX tensor dump tests**, not on liveness:

```
test backend::ptx::tensor::r16_dump::dump_e5a_b_lookahead ... FAILED
thread '…' panicked at src/backend/ptx/tensor.rs:2702:81
```

The panic site is a test writing a debug artifact to a hardcoded path and
`unwrap()`ing:

```rust
std::fs::write(&format!("/tmp/opencode/tgemm_e5a_{tag}.ptx"), &ptx).unwrap();
```

45 such `/tmp/opencode` references exist across `src/backend/ptx/{tensor,general}.rs`,
`src/compile.rs` and one integration test. CI normally provides the directory via
`.github/actions/compiler-tools/action.yml:22` (`mkdir -p /tmp/opencode target/ci-evidence`)
— but the nightly job uses `actions/checkout` + `dtolnay/rust-toolchain` +
`taiki-e/install-action` and **never invokes that action**, so the directory does
not exist there.

So fixing (1) alone would hand the nightly straight to (2).

### 2b. And then it would fail to *compile*

Fixed during the follow-up, and previously invisible because the parse error
stopped everything earlier: a `#[cfg(test)]` `BuildOptions` initializer in
`src/compile.rs` (`webstack_opts`) was missing two fields added after it was
written:

```
error[E0063]: missing fields `deny_accel_cpu_fallback` and `validate_ir`
              in initializer of `briev_compiler::pipeline::BuildOptions`
   --> src/compile.rs:4009:9
```

Nothing caught it because `cargo test --lib` never compiles the **binary's** test
target; the nightly's `--all` nextest run is the only caller that does.

### 2c. Four blockers in total, and the fourth was a test lying about itself

After (1), (2b) and the tooling were fixed, the suite ran **4443 tests: 4442
passed, 1 failed** — and the last one was informative:

```
enum_list_elements_survive_later_calls ... FAILED
panicked at tests/enum_list_lifetime_regression.rs:27
build failed:            <- empty stderr
```

The test spawned a hardcoded `target/release/brievc`; the nightly builds only
debug, so the file did not exist. **It had also not been testing what it claims
locally**: this checkout held a release binary from 2026-11-17 09:07, so the test
drove a stale compiler. `CARGO_BIN_EXE_brievc` (as `dir_smoke.rs` and
`wasm32_parity.rs` already use) makes the path and profile the ones under test.

Four blockers, then, all between CI and an observation: the filterset, the stale
`BuildOptions` initializer, the job's hand-rolled toolchain list (no
`spirv-tools`, no `/tmp/opencode`), and this hardcoded binary path. Each one
looked like a code failure. With all three fixed the
suite runs **4442 tests, 4442 passed, 13 skipped (916 s)** — the AST path itself
is healthy, and the 2026-11-15 red was the `/tmp/opencode` panic, not liveness.

## 3. The HIR/AST defn-liveness drift is still open — and nothing covers it

Unchanged since `BUGS.md:417` ("debug-only assert; release ships the smaller
set"):

```rust
// src/backend/mod.rs:641
debug_assert_eq!(h.live, ast.live, "HIR/AST defn-liveness drift");
```

On `lib/std/os/dir.bv` the HIR live set omits `dir_open`/`dir_close`/`dir_loop`,
so the subtree below them disappears; emission is unaffected (the 56 differing
names are still emitted and called in the release `.ll`), so the damage lands on
*classification* — boundary ownership and reclamation hints read `DefnLiveness`
and give live defns the non-live treatment.

Why CI never sees it:

* `tests/hir_defn_liveness_parity.rs` is **curated, not corpus-wide** — hand-written
  snippets (`dead`, `export`, `reactive-txn`, `closure`, `endprogram`), no
  `dir.bv`. `BUGS.md` already records that its probe shapes all agree.
* `tests/dir_smoke.rs` and `tests/mini_cli.rs` **import** `std/os/dir.bv`; the
  assert only fires when dir.bv is the compilation root (as a manual
  `brievc build lib/std/os/dir.bv` in a debug build does).

So: real, open, and invisible to every gate — the worst combination, but also
not urgent in release terms.

**Measured 2026-11-18 (still live).** `cargo test --test hir_defn_liveness_parity
-- --ignored` reproduces it on current `main`:

```
assertion `left == right` failed: HIR/AST defn-liveness drift
  AST: 83 names (incl. dir_open, dir_close, dir_loop, entries, float_to_str, …)
  HIR: 24 names
```

A new `corpus_defn_liveness_parity` walks `tests/tier1` + `lib/std` (154 files,
0 skipped) and is **green** — because it compares the *raw parse*, where the two
walks agree. The drift lives on the **resolved** program (prelude merged), which
is what `src/backend/mod.rs:641` actually compares. That asymmetry is the reason
the curated shapes never reproduced it either, and it is worth knowing before
someone writes a fourth test that cannot fail.

## Plan

Ordered by cost, and by "does it restore the ability to see"? 

| # | change | why first |
|---|---|---|
| 1 | Fix the filterset (one `-E`, documented matchers) | until the suite runs, nobody knows the AST path's state; it is a 2-line workflow edit |
| 2 | Add `mkdir -p /tmp/opencode` to the nightly job (or reuse `compiler-tools`) | without it, fixing (1) just moves the red to the PTX dumps |
| 2b | Fix the stale `BuildOptions` test initializer | without it the suite cannot compile at all under `--all` |
| 3 | Re-run the nightly, then triage whatever is genuinely red on the AST path | **done**: 4442/4442 pass, so nothing is genuinely red |
| 4 | *Then* the drift: `corpus_defn_liveness_parity` added (green, raw parse) + `dir_bv_as_root_does_not_trip_the_liveness_assert` added **ignored** because it fails | visibility without a red `main`; the ignored test is the repro, to be un-ignored by whoever fixes the drift |

Items 1–2 are workflow-only and cheap. Item 3 is measurement. Item 4 is the
real work (the `BUGS.md` bisect: `names <- …`, `Free#(x as Ptr<…>)`, or the
`Result<List<String>, Int>` output type) and should follow, not precede, making
it visible.

**Not recommended:** adding `mkdir` *and* filterset *and* a corpus parity test in
one PR — the first two determine whether the third's failures are even reachable
in CI.
