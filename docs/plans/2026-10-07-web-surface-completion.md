# Web surface completion — regression gate, obj fix, multi-page (2026-10-07)

**Status:** Active
**Umbrella:** `docs/plans/2026-10-04-three-surfaces-functional.md` Phase 3 (`.rbv`)
**Predecessors:** `docs/plans/2026-10-06-web-routing-and-bundling.md` (Parts
0–4 landed), `docs/plans/2026-10-06-wasm32-pointer-width.md` (the wasm32
codegen fixes that unblocked Part 4),
`docs/architecture/web-host-boundary-decision-record.md` (`#Web` retired; the
browser is a host-module path).

## Goal

Close the web surface: (1) put this session's wasm32 fixes behind an in-repo
regression gate, (2) fix the unpacked-obj codegen defect that blocks the
`Router` obj shape, (3) finish multi-page.

## Workstream 1 — In-repo regression gate

**Problem.** The router smoke fixture and its gates live only in
`/tmp/opencode/rs/` (`rs_tmp.rbv`, `gate.mjs`, `check_calls.py`). Nothing in
the repo guards the pointer-width / void-frgn / view-liveness fixes; a future
change can silently reintroduce a wasm32 trap stub.

**Deliverables.**
- `tests/fixtures/router.rbv` — the router smoke (imports `std/web/router.bv`,
  `path`/`route` state, `go` txn, `b-trigger`/`b-when` view).
- `tests/rbv_router.rs` — Rust integration test (no node): build the fixture
  in a temp dir via `compile_source`, read the `.ll`, run the mechanical
  call-vs-declare check (port of `check_calls.py`), assert zero mismatches and
  that `@go`/`@navigate` are defined. The always-on guard.
- `benchmarks/rbv_gate.sh` + `benchmarks/rbv_gate.mjs` — node runtime gate
  (11 behavioral checks). Mirrors the existing `*_gate.sh` shape.

**Gate.** `cargo test --lib` green; `tests/rbv_router.rs` green;
`bash benchmarks/rbv_gate.sh` exits 0.

## Workstream 2 — Unpacked-obj String-field fix (BUGS.md:8800)

**Defect.** An obj with a txn held as top-level state is unpacked into `%State`
(`[1 x ptr]`), but its `StructLiteral` initializer stores the field value (a
`ptr`) with the aggregate type → `store [1 x ptr] <ptr>, ptr <dst>`. Blocks
the `Router` obj shape.

**Sites.** `emit_init_state` (`src/backend/llvm/emit_toplevel.rs:2503-2614`),
`Expr::StructLiteral` emission (`src/backend/llvm/emit_expr.rs:979,1801`),
`obj_instance_inits` (`src/backend/llvm/mod.rs:6252`).

**Approach (measure first).** Reproduce; dump the `.ll`; form the hypothesis;
fix in the unpacked-instance init path (per-field GEP+store, or box on a
`StructLiteral` init like the plain-obj path); preserve the object-instance
pool path (`self_prefix`, `instance_prefix_for`). Add the obj-form fixture;
then add the `Router` obj to `lib/std/web/router.bv`.

**Gate.** obj-form fixture builds + runs; pool tests green; Praetor clean;
BUGS.md:8800 → FIXED.

## Workstream 3 — Part 5 multi-page

- **B2:** one bundled HTML per `.rbv`, `<a href>` between pages (zero external
  refs); a two-page example.
- **B1:** one bundle + the Part-4 router (the `router.rbv` shape) documented;
  popstate deferred (needs a callback-frgn ABI).
- **File-based routing:** folio `[pages]` over multiple `.rbv` → a generated
  route table (build/framework convention, never a compiler keyword).

**Gate.** Per the web plan — bundle e2e (zero external refs); `rbv_gate.sh`
extended.

## Riders
- Doc sweep: `docs/architecture/features/webstack-intrinsics.md`,
  `docs/architecture/features/rendered-briev-wasm.md` still say `from #Web`.
- Deferred (not here): `node`→`js` rename (D4); popstate.

## Sequencing
`1 → 2 → 3`, continuous commits; per landing `cargo test --lib` green +
Praetor no new diagnostics + docs in the same commit. Risk concentrates in W2
(object-instance-pool codegen is intricate) — measure before building.

## Progress log

- **W1 DONE** `ec3c7cd6` — in-repo router regression gate
  (`tests/fixtures/router.rbv`, `tests/rbv_router.rs`,
  `benchmarks/rbv_gate.{sh,mjs}`, `benchmarks/rbv_ir_check.py`).
- **W2 DONE** — `BUGS.md:8800` fixed: `emit_instance_init`'s StructLiteral
  branch stored the column's row-0 element with the column ARRAY type
  (`[1 x ptr]`) instead of the inner type; now derives `load_ty` from the
  column's LLVM string (mirrors `emit_instance_column_row`). Gate:
  `tests/fixtures/obj_init.rbv` built by `rbv_gate.sh`; `cargo test --lib`
  2917 green; Praetor no new diagnostics. **Follow-up filed:** a member txn
  is not a top-level wasm export, so a view `b-trigger` cannot fire it — the
  `Router` obj is expressible but not yet view-bindable (BUGS.md, new entry).
- **W3 DONE (B2)** — multi-page: `examples/multi_page_{a,b}.rbv` cross-link
  by `<a href>`; each bundles to one self-contained HTML. `rbv_gate.sh`
  asserts self-containment (zero external refs) + the cross-link. **B1** is
  covered by the router fixture built in bundle mode (the SPA shape). **File-
  based routing (folio `[pages]`) deferred** — a larger build/framework item.
- **Bonus fix** — `warn_undispatched_txns` (backend) false-warned a
  view-`b-trigger`-bound no-param txn ("never dispatched"); now skips live
  txns (a live-but-uncalled txn is root-dispatched). Surfaced by the B2
  example; gated in `rbv_gate.sh`.

## Workstream 4 — Member txn on a plain top-level obj var (BUGS.md:8921)

**Defect.** A member txn on a plain top-level obj var (no `render` block) is
not emitted as a top-level export; the view's bare-name `b-trigger` resolves a
missing wasm export at runtime.

**Root cause.** `collect_instance_lets` only consumed `let <name>: <Obj>` where
`<Obj>` has a `render` block. A plain var was never consumed, so its member
txns were never emitted and the bare-name `b-trigger` had no mount tag to
rewrite through.

**Fix.** Extend `collect_instance_lets` to also consume `let <name>: <Obj>`
where the view references a member txn by bare name. `build_plain_var_instance`
emits the variant (`@go_<var>`); the view compiler rewrites the top-level view's
directive values (`top_level_variants`). The consumed `let`'s initializer
callees are re-rooted via `plan.init_roots` (the `let` is removed before
liveness indexes it). Gate: `tests/fixtures/obj_router.rbv` (step 3 in
`rbv_gate.sh`).

**Status:** DONE — `cargo test --lib` 2919 green; `rbv_gate.sh` OK; Praetor no
new diagnostics.

## Workstream 5 — Stranger-loads-page probe (Phase 3.1 gate) — 2026-10-08

**Goal.** Prove the Phase 3 acceptance criterion — "a stranger loads a `.rbv`
page" — end-to-end in a real browser. The node gate (W1) stubs the host and
cannot prove the page loads from `file://`, the boot flush lands, or a real
click round-trips to the DOM.

**Deliverables.**
- `benchmarks/rbv_browser_smoke.mjs` — Playwright/Chromium smoke: build
  `counter.rbv` in bundle mode, load the `.html` from `file://`, assert
  (a) zero console/page errors, (b) the seeded `b-text` reflects the Briev-side
  seed on load, (c) the `+`/`Reset` click round-trip, (d) the anonymous
  `<Counter />` instance renders.
- `rbv_gate.sh` step 6 — runs the smoke if Playwright/Chromium is available
  (browser downloads to `~/.cache/ms-playwright` via `npx playwright install
  chromium` — no sudo/pacman); skips gracefully otherwise.

**Two real stranger-relevant bugs found + fixed (BUGS.md, 2026-10-08):**
1. **`createApp` null-exports race** — the constructor fired `_init` (async)
   without awaiting it, then `createApp` read `runtime._instance.exports`
   synchronously (`null.exports`) → a page error in a real browser. Fix: store
   the init promise as `_ready` and await it in `createApp`.
2. **`__web_boot` initial-flush gap** — `__web_boot` ran `init_state` but never
   flushed the initial state, so the seeded `b-text` showed the HTML literal
   (`0`), not the Briev-side seed (`5`). Fix: `__web_boot` emits a per-field
   initial flush (flush buffer sized to `max(largest txn write_set,
   field_count)`), AND the shim reorders `_loadStateLayout()` before
   `__web_boot()` so the flush lands against a populated binding table.

**Docs (same commit):** `b-` = binding rationale (SPEC §21.4 + feature doc);
artifact story + Phase 3 gate (SPEC §21.1); BUGS.md entries; INDEX.

**Status:** DONE — `cargo test --lib` 2928 green; `rbv_gate.sh` OK (incl. the
new step 6); Praetor no new diagnostics.

