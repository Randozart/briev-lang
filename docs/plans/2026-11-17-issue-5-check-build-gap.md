# Issue #5 — `check` succeeds while `build` fails (2026-11-17)

## The divergence, precisely located

`brievc check` and `brievc build` share the **frontend** — both funnel through
`pipeline::parse_and_check` (src/pipeline.rs:1270), which runs the full
plugin-manager pipeline, typecheck, and the whole acceptance battery
(watchdog, termination, casing, strict, task-linear, spawn-pool). The
2026-08-18 doc at the top of that fn records the long history of closing
frontend divergences.

They diverge at the **seam after `parse_and_check` returns**
(`Ok((items, universe))`, src/pipeline.rs:1465):

| | `check` (`check_source_for`, pipeline.rs:998) | `build` (`compile_source`, compile.rs:249) |
|---|---|---|
| Frontend (`parse_and_check`) | ✅ | ✅ |
| **Backend normalizer** (compile.rs:682/689/700/707) | ❌ | ✅ |
| **Codegen → `.ll`** | ❌ | ✅ |
| **`verify_emitted_ir`** (`opt -passes=verify`, compile.rs:3334) | ❌ | ✅ |
| clang link / binary | ❌ (correct — check is static) | ✅ |

So the bug class is: **the frontend accepts a program whose *emitted IR*
fails LLVM verification** (missing sret slot, width mismatch, undefined SSA —
exactly the `verify_emitted_ir` comment at compile.rs:3329). `check` returns
`OK` because it never generates the IR; `build` fails in `opt`/`clang`.

This is the same class I've been hitting all session — the wasm `%ac0`
i64/ptr mismatch and the `export let` `load i64, ptr @K` with no declaration
both *would* have been caught by `check` had it run IR validation. Issue #5
is the mechanism that lets them escape the static gate.

## Fix

Give `check` a `--validate` flag that runs the backend normalizer + codegen +
`verify_emitted_ir` **without** linking.

### Changes (3 files)

1. **`src/compile.rs`** — extract the normalizer + codegen + `verify_emitted_ir`
   span from `compile_source` into a reusable fn:
   ```rust
   pub fn validate_emitted_ir(
       file_path: &str, items: Vec<TopLevel>, universe: TypeUniverse,
       opts: &BuildOptions,
   ) -> Result<(), String>
   ```
   It does exactly what `compile_source` does from the backend-normalizer
   call (compile.rs:682) through `verify_emitted_ir`, but stops before
   `compile_ll_to_binary` / clang. No new analysis — it re-runs the *same*
   codegen `build` runs. `compile_source` can keep its current structure
   (ADDITIVE-ONLY, Rule 6: no existing optimization path is touched) or call
   the helper; either way the build path's emission is unchanged.

2. **`src/pipeline.rs`** — `check_source_for` takes a `validate: bool` (or a
   new `check_source_validate` wrapper). When set, after
   `parse_and_check` succeeds it calls `validate_emitted_ir` with the
   returned `(items, universe)`.

3. **`src/main.rs`** — `run_check` parses a `--validate` flag and threads it
   through. Usage line added to the help text.

### Why a flag, not always-on

`check` is used by the conformance sweep (`verify_bodies_corpus.sh`,
`packages.rs:434`) over ~240 files. Always-on IR validation would add a full
codegen pass per file and slow the sweep. A flag keeps `check` cheap for the
sweep and lets the user (or CI) opt into full validation. The default stays
`check`-as-is; `--validate` is the strict mode.

### Test

A fixture whose frontend typechecks but whose emitted IR is invalid. The
cleanest source: a program that triggers a known lowering bug. Since the
wasm/`export let` bugs are now fixed, the test should be **structural**:
- Add a test in `tests/` that calls `check_source_for(file, src, None)`
  (asserts `Ok` — frontend accepts it) AND
  `validate_emitted_ir(...)` (asserts it either succeeds on a *good* program
  or fails on a *constructed* bad one).

The minimal, honest test: a **good** program where `check` and
`check --validate` both pass (proves the wiring), plus a **known-bad** IR
case if one can be constructed without re-introducing a bug. The exit
condition is that `brievc check --validate <file>` runs the normalizer +
codegen + `opt -passes=verify` and returns the `verify_emitted_ir` diagnostic
on invalid IR.

## Exit condition

- `brievc check --validate <good.bv>` → `OK` (frontend + IR validation pass).
- `brievc check --validate <bad-IR.bv>` → the `verify_emitted_ir` error
  (emitted IR failed LLVM verification), not a silent `OK`.
- `brievc check <file>` (no flag) → unchanged (frontend only, no regression).
- `cargo test --lib` green; `bash scripts/gate.sh quick` green.

## Scope / risk

- **Medium risk.** The change is additive: it adds a new code path
  (`--validate`), it does not alter the existing `check` or `build` emission.
  The only shared surface is `validate_emitted_ir` calling the same
  normalizer+codegen as `build` — if that refactor is done carefully
  (extract, don't rewrite), `build`'s output is byte-identical
  (determinism gate will confirm).
- **Do NOT** change the default `check` behavior. The conformance sweep and
  any tooling relying on `check`-is-cheap must keep working.
- **Do NOT** add linking to `check --validate` — it's a static validation,
  not a build.

## Files to touch

| File | Change |
|---|---|
| `src/compile.rs` | Extract `validate_emitted_ir` (normalizer + codegen + `verify_emitted_ir`, no link) |
| `src/pipeline.rs` | `check_source_for` gains a `validate` path that calls `validate_emitted_ir` |
| `src/main.rs` | `run_check` parses `--validate`, threads it through; help text |
| `tests/check_validate.rs` (new) | good-program wiring test + bad-IR diagnostic test |

## Documentation

- `spec/SPEC.md`: add `--validate` to the `check` command description.
- `docs/guides/` (if a CLI guide exists): note that `check` is frontend-only
  by default and `--validate` runs IR validation.
- This file.
