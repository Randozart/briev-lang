# Readiness gates: watchdog honor-or-refuse + web end-to-end audit

**2026-10-17.** Context: with the dispatch fix (821d8155) and the
state/plain storage classes (5aa90185) landed and the full suite green
(21 Briev wins + 4 precomputed + 2 bridges, all MATCH), the question is
whether the language is ready for use. Answer: native is ready EXCEPT
one silent-contract hole; web is usable but unaudited end-to-end. This
plan closes both.

## Gate 1 (native, correctness): the watchdog emission gap

**BUGS.md (OPEN):** a node contract's watchdog `?[cond] -> handler(arg)`
is emitted ONLY on counter-engine dispatches (per-field-phi via
`emit_countable_main`, countdown via `emit_countable_countdown_main`).
Four dispatch paths silently drop it: InlineSsa (`emit_folded_main`),
version-DAG (`emit_version_dag_main`), the direct-SSA loop
(`emit_ssa_main`), and the reactor loop. A user writing a watchdog on a
multi-node or non-foldable program gets no fire and NO diagnostic — a
silent contract violation, the exact class Rule 1 exists for.

**Fix shape — honor-or-refuse, per engine:**

1. Shared helper `emit_watchdog_check(out, indent, wd, exit_label)` —
   evaluates the condition through the normal expression resolver,
   and on true: loads the on-fire argument, calls the handler, branches
   to `exit_label` (the dispatch loop's exit — the same semantics the
   counter engine's `.cmwd_` implements: fire once, call, stop).
2. Wire into all four paths:
   - `emit_ssa_main`: after each txn's body in the tick, check THAT
     txn's watchdog (per-node contract, per-node check).
   - `emit_folded_main` (InlineSsa): check in the folded loop head.
   - `emit_version_dag_main`: check in the loop head.
   - reactor loop: check per tick after the txn fires.
   Any site whose engine shape genuinely cannot support the check must
   instead FAIL the dispatch with a diagnostic naming the node and the
   dropped contract — never a silent drop. (Expected: none; all four
   have an in-loop emission point.)
3. The handler call mirrors the counter engine's `emit_user_call` shape
   (defn call, stateless or stateful per the needs-state verdict —
   plain-storage aware).

**Tests:** one behavioral (link + run) test per engine: a program whose
watchdog-carrying node dispatches to THAT engine must fire the handler
and terminate with the handler's output. Engines are reached via
program shape (multi-node → SSA/reactor; counter-only body writes →
InlineSsa; version-DAG via its guard shape). Plus the existing
series_converge test guarding the counter path (unchanged).

**Reference implementation:** `loop_engine/counter.rs` `.cmwd_*` labels
(around the watchdog emission) — the new helper produces the same
structure, parameterized by exit label.

**Undo:** revert the wiring commits; the helper is inert without
callers.

## Gate 2 (web): end-to-end audit of the .rbv corpus

The web layer's pieces exist and are unit-tested (view compilation,
layout table, wasm32 normalizer, DOM shim generator, host-fn emission
as GLUE config data, rbv router with per-route flush batching), but no
session has run the example corpus through the FULL chain and recorded
what works. Audit steps:

1. Enumerate `examples/*.rbv` (todo-list, shopping_cart, multi_page A/B,
   view-directives, view-bind-edge, counter, rstruct-demo,
   rbv-no-script-tags) — compile each via the webstack/GLUE [web]
   pipeline (`brievc rbv` or the compile.rs view path — whichever the
   CLI exposes), record: compiles / emits shim / emits wasm-or-js.
2. For each emitted artifact: verify the HTML+JS is well-formed enough
   to load (node --check for the JS; presence of the layout table,
   host-fn block, and route table in the shim).
3. Document per-example gaps in this file's results section; file bugs
   for anything that fails. NO feature work in this gate — the audit
   produces the certified list and the bug list.

## Non-goals

- kalman contraction shape, float_math precompute gap (perf queue).
- µs-bench re-measurement (needs a quiet machine).
- Integration-test unblock (belongs to the session that owns the
  untracked test files).

## Results (2026-10-17)

### Gate 1 — watchdog honor-or-refuse: LANDED

Five dispatch paths had to be mapped, not four: the multi-node survey
found the **async-body phase** (Family H sequential; all-eligible
non-conflicting node sets classify as async) as a fifth silent dropper.

- **Honoring (shared `emit_watchdog_gate`, helpers.rs):**
  - direct-SSA loop (`emit_ssa_main`): per-txn gate after each body
    (deadline clock captured in the preheader); also after the
    internal-fold and accel-kernel single-call paths.
  - InlineSsa (`emit_folded_loop`): gate between the bound check and the
    body; the gate's condition resolves the counter to its LIVE PHI
    (the %State slot is stored once after the loop — reading memory
    compared 0 forever and never fired; found by the behavioral test).
  - version-DAG (`emit_version_dag_main_inner`): gate between the bound
    check and the absent/pre block (the gate's hold label IS that block).
  - async-body phase (`emit_async_phase`): per-node gate after each
    body; the fire branches to `.end`, halting the main convergence loop
    (Family H is sequential → deterministic halt). Lightweight-async
    watchdog nodes are refused (that phase never runs the gate).
- **Refusing (fatal, naming the node and the fix):**
  - reactor tick — the fire must latch the node off across ticks;
    machinery the tick does not have.
  - `within N cycles` on non-counter dispatches (SSA/version-DAG/async —
    a cycle bound needs the counter-fold's counter). `within N ms` is
    HONORED everywhere (phase-entry clock).

**Tests:** `watchdog_fires_on_{direct_ssa,inline_ssa,version_dag}_dispatch`
— behavioral (compile → link with a printf stub → run → assert the
handler's output and termination). One discovery per test: the SSA
fixture needed provable contracts (the fixture-legality gate rejects
`[true][true]`), the InlineSsa gate needed live-phi resolution, and the
SSA-shape program actually dispatched async (which found the fifth
dropper). The reactor/async-lightweight refusals call `exit(1)` and are
verified by inspection, not in-process.

**Gates:** `cargo test --lib` 3162/0 (3 new); full
`ll_determinism_check.sh` ALL DETERMINISTIC; Praetor — no new
diagnostics on previously-clean functions (the two introduced by the
first cut — `emit_async_phase` 27, `emit_watchdog_gate` 26/8-params —
were removed by extracting `watchdog_bounds_conj`,
`refuse_lightweight_watchdogs`, `async_phase_deadline_start`,
`emit_async_node_watchdog_gate`, and the `WatchdogGateLabels` struct;
the residue is metric drift on `emit_ssa_main`, an already-failing
function, accepted per the 67dae3f8 precedent).

### Gate 2 — web end-to-end audit: DONE (2026-10-17)

| check | result |
|---|---|
| `examples/*.rbv` compile (9/9) | all exit 0, bundled HTML + embedded wasm |
| JS validity | 8/9 `node --check` clean; **rbv-no-script-tags FAILS** (bug filed) |
| state-layout table | present in all (runtime calls `state_layout()`) |
| wasm embedding | present in all |
| host-fn chain (console probe) | `console_log` at module scope + wasm import binding + valid JS — the 10-06 host-fn shipping works end-to-end |
| router fixture (`tests/fixtures/router.rbv`) | compiles + emits |
| **real-browser smoke** | **rbv_browser_smoke OK + rbv_gate OK** — stranger-loads-page over file://, click round-trip, counter + router |

**Verdict: the web layer is READY for the covered surface** (views,
bindings, triggers, routing, host modules, wasm embed) — verified by
the existing real-browser gate, now re-confirmed. One bug filed
(BUGS.md: rbv-no-script-tags emits raw source into JS — the
"Briev-as-default" format is silently unparsed).

**Audit process notes:** the canonical web imports are the plain-path
form (`import "std/web/console.bv";`); the JS default-import form
(`import console from ...`) is unsupported — the parse error is clear
but could name the correct form (minor UX, not filed). The host-fn
examples in the corpus don't exercise console/dom (they use b-text
bindings), which is why the audit added the explicit host probe.
