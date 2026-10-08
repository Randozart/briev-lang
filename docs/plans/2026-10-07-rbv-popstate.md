# `.rbv` popstate — real back/forward routing (2026-10-07)

**Status:** Active
**Umbrella:** `docs/plans/2026-10-04-three-surfaces-functional.md` Phase 3 (`.rbv`)
**Predecessor:** `docs/plans/2026-10-07-web-surface-completion.md` (W1–W3 + the
member-txn fix — the `.rbv` surface is functional).
**Scope:** popstate only — a general window-event directive that enables real
back/forward. Not file-based routing, not the dev loop, not the callback ABI.

## Goal

Browser back/forward buttons change the URL but leave the `.rbv` view stale:
nothing re-reads `location`. Close that gap so the router is a real SPA —
**without a new compiler ABI**. The forward path already works (`go` sets
`path` + `navigate`, which the host module implements as `history.pushState`);
back/forward needs a `popstate` handler.

## Design decision

| Option | Mechanism | Verdict |
|---|---|---|
| **A: `b-window:` directive** (general window events) | A view directive binding a `window` event to a txn, parallel to `b-on:` (element event). Shim emits `window.addEventListener(...)`. | **Chosen** — reuses the trigger + defn-liveness machinery; general (popstate, hashchange, resize…); no vocabulary matching (Rule 23); no new ABI |
| B: callback-frgn (`frgn on_popstate(cb: fn() -> Void)`) | Briev passes a fn to JS; shim calls it on popstate | Needs a wasm `Table` bridge (JS→wasm fn call). The C callback path exists but the JS wrapper doesn't (`docs/architecture/features/callbacks.md`). Bigger; a different capability |

**Chosen: A.** It is the `.rbv` surface's natural mechanism; popstate is one
instance of "a window event fires a txn."

### Syntax

`b-window:<event>="<txn>"` — a window-scoped trigger. Parallel to the existing
`b-trigger:` (element) / `b-on:` (element) triad.

```
obj Router {
    path: String;
    txn go(url: String) [path != url][path == url] { path = url; navigate(url); term; };
    txn sync_route [path != current_path()][path == current_path()] { path = current_path(); term; };
};
let router: Router = Router { path: current_path() };
<view>
    <button b-trigger:click="go('/')">Home</button>
    <button b-window:popstate="sync_route"></button>
    <p b-text="router.path">.</p>
</view>
```

Forward: `go` sets `path` + `navigate` (host `history.pushState`). Back/forward:
the browser fires `popstate` → `b-window:popstate` fires `sync_route` →
`path = current_path()`.

Why `b-window:` and not `b-on:window:popstate`: the event-suffix parser
(`extract_event_suffix`) stops at the first non-alphanumeric, so a nested
`window:popstate` token would parse as `window`. A distinct directive prefix is
clean and keeps the event name opaque (the browser dispatches it; the compiler
does **not** validate event names — that would be vocabulary matching).

## The validity-inference guarantee (the load-bearing requirement)

A `b-window:` trigger MUST go through the **same validity inference** as a
`b-trigger:` — the compiler must still tell valid code from invalid. The design
enforces this by constructing a `Directive::Trigger` through the **same
extraction + validation path**, differing only in a `scope` field. The gates:

| Gate | Where | How `b-window:` is covered |
|---|---|---|
| **Unknown-directive rejection** | `KNOWN_DIRECTIVES` (`view_compiler.rs:26`); unknown `b-*` → `warning[RBV001]: unknown directive` | `b-window:` added to the list — else it warns + dead button |
| **Trigger txn must exist** (strict) | `verify_srbv` (`view_compiler.rs:1811`), `Directive::Trigger { txn, .. }`: undefined → `SRBV004`; trivial `[true][true]` → `SRBV005`; user-triggered `[true]` precondition → `SRBV011` | The `..` ignores the new `scope` field → **automatic** |
| **User-triggered precondition lint** | `user_triggered_txns` (`view_compiler.rs:1073`) → `validate_user_triggered_preconditions` (R001/R002) | `b-window:` inserts the txn into `user_triggered_txns` — a browser event is non-deterministic input, exactly like a click |
| **Liveness root (no dead button)** | `view_trigger_txns` (`pipeline.rs:663`), `Directive::Trigger { txn, .. }` | **Automatic** — the window trigger is rooted → emitted |
| **`b-window:` inside `b-each`** | item-scope extraction (`view_compiler.rs:814`) | **Rejected** with a diagnostic (window events are not per-item) — never silently dead |

**The event name is deliberately not validated** (Rule 23 — no vocabulary
matching). The compiler validates the *txn* (existence, contract,
precondition); the browser validates the *event*.

## Change surface

### 1. `src/view_compiler.rs`
- `KNOWN_DIRECTIVES` (line 26): add `"b-window:"`.
- New `#[derive(Debug, Clone, PartialEq)] pub enum TriggerScope { Element, Window }`.
- `Directive::Trigger` gains `scope: TriggerScope`.
- Element extraction branch (line 1066): the `attr.starts_with("b-trigger:") ||
  attr.starts_with("b-on:")` condition gains `|| attr.starts_with("b-window:")`;
  the prefix selection gains the `b-window:` arm; the `Directive::Trigger`
  construction sets `scope = Window` for `b-window:`, `Element` otherwise. The
  `user_triggered_txns.insert` (line 1073) is in the shared path → covered.
- `extract_trigger_value` (line 1146): add `.or_else(|| attr.strip_prefix("b-window:"))`.
- `extract_event_suffix` already extracts the event name after the colon
  (`popstate`) — no change.
- Item path (line 814): `b-window:` inside `b-each` → `validation_errors`
  diagnostic (window events are not per-item).

### 2. `src/glue/web_generator.rs`
- `binding_to_js` (line 1007): `Directive::Trigger { event, txn, params, scope }`
  — choose the target: `window` (Window scope) vs `el` (Element). Emit
  `window.addEventListener("<event>", () => this._txn("<txn>")(…))` for Window;
  unchanged `el.addEventListener(...)` for Element.
- Item triggers (693/696/1082/1085) stay Element-only — no change.

### 3. `src/pipeline.rs`
- `view_trigger_txns` (line 663) matches `Directive::Trigger { txn, .. }` —
  window triggers are rooted. **No change.**

### 4. `lib/std/web/router.bv`
- Document the `sync_route` + `b-window:popstate` pattern (the app declares the
  txn; the router supplies `location`/`navigate`/`current_path`).

## Tests

- **view_compiler.rs**
  - Parse `b-window:popstate="sync_route"` → `Directive::Trigger { event: "popstate", txn: "sync_route", scope: Window }`.
  - `b-window:` inside a `b-each` template → validation error (not silently dead).
  - An undefined window trigger is caught by `verify_srbv` (strict) — the `..` arm.
  - A `b-window:` to a `[true][true]` txn → `SRBV005`; to a `[true]`-precondition txn → `SRBV011`.
- **web_generator.rs**
  - Generated shim contains `window.addEventListener("popstate"` and `_txn("sync_route")` (Window scope); Element scope still uses `el.addEventListener`.
- **pipeline.rs**
  - `view_trigger_txns` collects a window-scoped trigger (liveness root).
- **Non-strict unknown-directive guard**
  - A typo `b-windowX:popstate` → `warning[RBV001]: unknown directive` (the
    `KNOWN_DIRECTIVES` gate still fires — the compiler still infers invalidity).

## Gate

- `tests/fixtures/popstate.rbv` (new): the `Router` obj with `go` +
  `sync_route`, wired with `b-trigger:click="go('/about')"` +
  `b-window:popstate="sync_route"`.
- `benchmarks/rbv_gate.sh`: build it; assert (a) the shim binds
  `window.addEventListener("popstate"`, (b) the `sync_route` txn is
  **exported** (liveness root — the class of bug this project guards), and
  (c) IR call/declare agreement. The `_txn` runtime path is already gated by the
  router fixture, so a shim-content assertion is proportionate; an optional
  stretch is a node shim-dispatch smoke (fake `window` → dispatch popstate →
  assert state changed).

## Documentation (same commit)

- `spec/SPEC.md` §21.4: add `b-window:event` to the canonical directive list.
- `docs/architecture/features/rendered-briev-wasm.md`: document window events +
  the popstate router pattern.
- `lib/std/web/router.bv`: the worked popstate pattern.
- `docs/plans/INDEX.md`: the `.rbv` workstream status (popstate landed).

## Sequencing & gates

1 → 2 → 3 → 4 → tests → gate → docs.
Per landing: `cargo test --lib` green; `rbv_gate.sh` OK; Praetor no new
diagnostics; docs in the same commit.

## Risks

- **Contract with an FFI call** (`path == current_path()`): legal — contracts
  are emitted code; `current_path` stays live via `walk_contract`. The author
  writes the invariant; the compiler typechecks it.
- **Two writers of `path`** (`go`, `sync_route`): fine — `path` is read by
  `b-text`/`b-when`, not `b-bind`, so no unique-writer requirement.
- **`b-window:` on a `b-each` container**: ALLOWED as a single global window
  binding (not per-item) + an informational `note[RBV012]`. See the amendment
  below. The inner-element case is likewise a Window-scope binding.
- **Validity inference preserved**: `b-window:` shares the `b-trigger:`
  extraction + validation path (the table above) — the compiler still tells
  valid from invalid identically.

## Amendment (2026-10-08): `b-window:` on a `b-each` container — allow + note

**Decision.** A `b-window:` directive produces a Window-scope trigger binding
**wherever it appears** — a plain element, an inner `b-each` element, or a
`b-each` container. There is one `window`, so there is exactly one listener;
the container case emits a single global `Trigger { scope: Window }` binding,
not a per-item one. The container case additionally emits an informational
`note[RBV012]` explaining the semantics, because the likely author intent
("each item reacts to a window event") does not map to DOM reality.

**Why allow, not reject.**
- **Consistency.** We already allow `b-window:` on an inner `b-each` element
  (Window-scope binding) and `b-trigger:` on a `b-each` container (item-scoped
  `ItemDirective::Trigger`). Rejecting `b-window:` on a container specifically
  would be a one-off special case — the kind of arbitrary rule the architecture
  forbids. The uniform rule ("a `b-window:` directive is always a Window-scope
  trigger, wherever it appears") is simpler to state and to maintain.
- **Opportunity.** The directive covers `popstate`, `hashchange`, `resize`,
  `keydown`, `online`, `message`, … Allowing it everywhere keeps the door open
  for legitimate year-two uses (e.g. a global `keydown` declared inside a list
  template) without a per-location exception or a compiler change
  (proofs-not-shapes, Rule 24).
- **The `note[RBV012]` is the teaching mechanism.** It converts the
  "works but maybe-not-what-you-thought" case into "works *and* you now know
  exactly what it does." The compiler stays permissive where the code is valid
  and informative where the intent is likely mistaken.

**The DRY payoff.** The container case now *uses* the shared
`trigger_scope_and_prefix` helper (instead of a rejection branch), so the
helper is the single source of truth for "what scope does this trigger have?"
in both `extract_directives` and `capture_item_directives`. Adding a fourth
scope later (e.g. `b-document:`) is a one-line change to the helper, not a
two-site edit (Rule 17).

**Change surface (this amendment).**
1. `src/view_compiler.rs`:
   - `trigger_scope_and_prefix(attr) -> Option<(TriggerScope, &'static str)>`
     helper (near the other trigger helpers).
   - `extract_directives`: replace the inline three-way chain with the helper.
   - `capture_item_directives`: when it sees `b-window:` on a `b-each`
     container, push a `Trigger { scope: Window }` into the global bindings
     list (not the item list) and emit `note[RBV012]`.
   - Tests: `b_window_on_b_each_container_is_a_global_trigger` (container →
     one Window-scope binding + the note), `b_trigger_on_b_each_container_
     still_works` (regression: `b-trigger:` on a container → item-scoped
     `ItemDirective::Trigger`).
2. `spec/SPEC.md` §21.4: `b-window:` is global — one listener regardless of
   location; on a `b-each` container it is a single global binding (not
   per-item), with an informational note.
3. `docs/plans/INDEX.md`: note the container-case decision.
