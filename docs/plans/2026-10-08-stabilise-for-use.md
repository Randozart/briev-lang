# Stabilise the Language for Use

**Date:** 2026-10-08
**Status:** active
**Goal.** A stranger writes a `.bv`/`.rbv`, it type-checks, compiles, runs, and
behaves as expected — with the self-hosting story sound. This is the "use"
bar, distinct from the performance campaigns (GPU/GEMM) and the foreign lanes
(Electronics `.ebv`, bad-dialect).

## The ordered targets (all important; T1 → T3 → T2 → T4)

| # | Target | Why it matters for "use" | Effort |
|---|--------|--------------------------|--------|
| T1 | `dyn Trait` LLVM backend panic (`emit_toplevel.rs:754`, BUGS.md:5148) | Trait-dispatched programs run in the interpreter but **trap in codegen** — a Rule 5 violation. Core expressiveness. | Med |
| T3 | `.rbv` dev loop (rebuild-on-change) + one worked example beyond the counter | The last un-shipped part of the `.rbv` end-to-end story (Phase 3.3 of the three-surfaces umbrella). Makes the web surface *usable*, not just provable. | Small-Med |
| T2 | Self-hosting embryo — 10/12 `lib/compiler/*.bv` fail `brievc check` | "The compiler writes itself" (Rule 24) is the strongest stability claim the language can make. Currently doesn't parse. | Large |
| T4 | `hardware_validator` hookup (BUGS.md:7439) + nbody drift (BUGS.md:5954) | Quick wins, lower centrality. | Small |

**Ordering rationale.** T1 first: it is the clearest "the language isn't stable"
defect — a program the reference runs correctly *traps in the backend*, exactly
the thing Rule 5 says to fix in codegen, never the interpreter. It's scoped (one
panic site) and unblocks a class of trait-dispatched programs. T3 second: small,
completes the web story, and is the "something you want of something that
compiles web pages" the dev loop provides. T2 third: large scope (self-hosting
embryo), the highest-leverage *signal* but the biggest lift. T4 last: quick
wins, opportunistic.

## Dropped from the list (reconsidered 2026-10-08)

- **`i64` boxing tax Phase 1** (`adapt_to_i64`, helpers.rs:2223). On a 64-bit
  host `Int` is **already** 64-bit — the widening helper is an edge (deliberate
  32-bit `Int32` → back), not a "stabilise for use" blocker. Likely a
  misfiled ledger item (a perf micro-optimization dressed as a correctness
  gap). Dropped. Re-verify in the ledger only if a 32-bit-`Int` program
  miscompiles.

## Not "use"-blocking (measurement debts, GPU/driver-gated)

BUGS.md 5827 (2–7-workgroup RTX 3060 dispatch), 6166 (`spirv_coopmat_subgroups=1`
2048³), 6707 (m4 decode microbench), 7460 (driver 615 vs 580 triple). Skip for
"use"; they need an instrument, not a fix.

## Already stable (no action)

`.bv` stranger blockers (list `+`, json parse, unfired-txn silence, package v0
+ install), `.rbv` artifact + stranger probe + file-based routing seam,
conformance sweep green at tip, 30/30 `lib/std` modules `brievc check` PASS,
stale-binary guard (`brievc freshness`).

## T1 — `dyn Trait` LLVM backend panic

**Defect.** A `dyn Trait` value passed to a trait-dispatching call panics in the
LLVM backend at `src/backend/llvm/emit_toplevel.rs:754`. The interpreter
dispatches it correctly, so the backend must compile it (Rule 5).

**Investigate.**
1. Find the panic site + the surrounding match arm. Read the interpreter's
   `dyn Trait` dispatch path (the reference) to learn the exact lowering the
   backend owes.
2. Build a minimal `.bv` repro: a `dyn Trait` value + a trait-dispatching call
   that the interpreter runs and the backend panics on. (Gate it in a test.)
3. Add the repro to `tests/` (force-add — `tests/` is gitignored) + a regression
   test that runs it through the LLVM backend.

**Fix.** Implement the missing `dyn Trait` dispatch lowering in the backend,
mirroring the interpreter. Additive only (new match arm; preserve the fallthrough).
Do NOT weaken the repro's contracts.

**Verify.** Repro compiles + runs in both engines; `cargo test --lib` green;
`rbv_gate.sh` green (if the repro touches the web surface); Praetor base-vs-now
on `emit_toplevel.rs` — no NEW diagnostics.

## T3 — `.rbv` dev loop

**Goal.** `brievc watch <file>.rbv` (or `brievc dev <dir>`) rebuilds the bundle
on change and the browser picks up the new wasm — the dev loop for the web
surface. Plus one worked example beyond the counter (a multi-component page
that exercises `b-each` + a member txn).

**Approach.** Thin orchestration over the existing `compile_source` (no new
codegen). A file-watch loop (notify crate, or a poll fallback if notify is not
already a dep — check Cargo.toml) re-runs the per-file build on change; the
output dir is served (or the bundle reloaded). Default stays the static bundle;
the dev loop is an additive convenience (matches the three-surfaces default:
".rbv = static WASM bundle; dev-server later if the model demands it").

**Gate.** `rbv_gate.sh` step (build → mutate → rebuild → the new artifact
differs). The worked example compiles + passes the browser smoke.

**Verify.** `cargo test --lib` green; `rbv_gate.sh` green; the worked example is a
real multi-component page (not a re-skin of the counter).

## T2 — Self-hosting embryo (`lib/compiler/*.bv`)

**Goal.** Get the 10 failing `lib/compiler/*.bv` files to pass `brievc check`.
Only `reader.bv` + `token.bv` pass today. This is the compiler-in-Briev
dogfood (Rule 24): the language proving itself by compiling its own passes.

**Approach.**
1. Inventory the 10 failures (run `brievc check` per file; capture the
   diagnostics). Classify: syntax gaps vs type gaps vs analysis gaps vs
   backend gaps (most are `brievc check` = parse+typecheck, so the backend is
   out of scope for this target).
2. Fix the language gaps the failures expose (parser/typechecker/analysis) —
   the *compiler* side, not by weakening the `.bv` files. The `.bv` files are
   the source of truth; if one is genuinely malformed, fix the file AND the
   diagnostic that should have caught it.
3. Incremental: get `ast.bv` → `lexer.bv` → `parser.bv` → `typechecker.bv`
   → the rest passing, one commit each.

**Gate.** A `brievc check` sweep over `lib/compiler/*.bv` (12/12 PASS) — add to
the conformance sweep or a dedicated test. **Do not** weaken the conformance
sweep's deliberate `lib/compiler` exclusion to make this "pass" — the point is
the files actually check.

**Verify.** 12/12 `brievc check` PASS; `cargo test --lib` green; Praetor
base-vs-now on the touched frontend files.

## T4 — Quick wins

- **`hardware_validator` hookup** (BUGS.md:7439): the `.sbv`
  synthesizability gate never runs (`src/lib.rs:57` is its only reference). Wire
  it into the `.sbv` build path so it actually gates. Small, mechanical.
- **nbody_newton 7th-decimal drift** (BUGS.md:5954): re-measure; if still open,
  diagnose the numeric source (FP op ordering, cast lane) and fix. Lower
  centrality — last.

## Gates (per target)

- `cargo test --lib` green.
- `bash benchmarks/rbv_gate.sh` green (T1/T3 touch the web surface).
- Praetor base-vs-now on changed files — no NEW diagnostics.
- Conformance sweep green (T2's explicit goal).

## Progress log

### T3 — `.rbv` dev loop (2026-10-08) — DONE

- **`brievc watch <dir> [--split] [--once]`** subcommand (`src/main.rs`).
  Thin orchestration over the per-file build (Part 1 seam) + the existing
  `src/watch.rs` debouncer (notify 6.1, already a dep) — NO new codegen, NO new
  deps.
  - `--once`: build the page set once + write nav.json/nav.html, exit. The
    testable path (CI / gate).
  - default: initial build, then a watch loop — poll for a changed `.rbv` /
    `folio.toml` (mtime-based, `find_changed_rbv` + `walk_rbv`), debounce to
    300 ms, rebuild + rewrite the nav on change.
- **Worked example beyond the counter** — `examples/todo-list.rbv`: a
  multi-component page exercising `b-each` over an obj vector + a member txn
  on an obj-backed component (TodoItem). Type-checks; the gate builds it.
- **Gate** — `rbv_gate.sh` step 5c: `brievc watch --once` builds the page set
  (incl. the worked example), asserts nav.json + the stamped `todo` key.
- **Verified:** `cargo test --lib` 2932 pass; `rbv_gate.sh` OK (6 steps + 5b +
  5c + browser smoke); Praetor base-vs-now on `src/main.rs` — no NEW
  diagnostics (refactored `run_watch` → `built_pages`/`write_nav_into`/
  `run_watch_loop`/`rebuild_and_write_nav`/`find_changed_rbv`/`walk_rbv` +
  `is_watchable`/`path_modified` helpers to stay at the complexity/param
  limits).
