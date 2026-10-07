# Web Host Boundary — Architecture & Decision Record

**2026-10-07.** Status: **authoritative for the web host boundary.** This
record revises decision #4 of
`docs/plans/2026-10-06-web-routing-and-bundling.md` (the "`#Web`
genericization, Option A" decision). Option A kept `#Web` and called it a
"GLUE target resolved by name"; this record concludes that `#Web` is **not a
protocol** and retires it as one, reframing the browser as a **library over a
host-import namespace** — the shape JavaScript and WebAssembly already use.

**Companions:** `docs/architecture/glue-ffi.md` (the language-bridge
pipeline), `docs/architecture/hash-words.md` (the `#` vocabulary),
`docs/architecture/conditional-ffi.md` (`#System`/`#Link`),
`docs/plans/2026-10-04-three-surfaces-functional.md` (the surface invariant),
`docs/plans/2026-10-06-web-routing-and-bundling.md` (superseded decision #4).
**Feeds** Golden Rules 3 (disclose special treatment), 15 (no knowledge of
specific types), 24 (proofs, not shapes); SPEC §19 (foreign functions,
export, GLUE).

**Status tags:** **DECIDED** (settled here), **OPEN** (recorded, not yet
decided), **DEBT** (agreed direction, retirement-gated).

---

## 0. How to read this document

- **Part I (§1–§4)** is *why*: how JavaScript and the web actually structure
  the language / host / platform split, and what Briev should learn from it.
- **Part II (§5–§8)** is *what*: the diagnosis of `#Web` as it exists today,
  with evidence.
- **Part III (§9)** is the *decision*: D1–D7, each with alternatives and
  rationale.
- **Part IV (§10)** is the *plan*: the migration, phased and gated.
- **Part V (§11)** is the *contract*: open items and references.

---

# Part I — Foundations: how JS/Web handles it

## 1. The three-layer model

The web does **not** put a "web" concept in its language. It separates three
layers, each with its own specification:

| Layer | Spec | Role |
|---|---|---|
| **Language** | ECMAScript (ECMA-262) | syntax + semantics + intrinsics; deliberately **host-agnostic** |
| **Host embedding** | the runtime (HTML for the browser; Node.js, Deno elsewhere) | provides the realm, the global object, and host capabilities |
| **Platform APIs** | **Web IDL** (WHATWG) + the HTML/DOM specs | the surface objects (`Document`, `History`, `Location`, `fetch`, …) and their JS bindings |

ECMA-262 delegates everything environment-specific to **host-defined hooks**
(`HostEnqueuePromiseJob`, …). Even the host-provided properties of the global
object are *implementation-defined* — TC39 has argued explicitly that "host"
and "implementation" mean the same normative thing
([tc39/ecma262#1524](https://github.com/tc39/ecma262/issues/1524)). The HTML
Standard pins the mapping 1:1: realm ↔ global object ↔ environment settings
object, where the global is `Window`, `WorkerGlobalScope`, or
`WorkletGlobalScope`
([HTML Standard, webappapis](https://html.spec.whatwg.org/multipage/webappapis.html)).

**Web IDL is the key artifact.** It is an *interface-definition language*: a
spec that "provides a syntax for specifying the surface APIs of web platform
objects, as well as JavaScript bindings that detail how those APIs manifest
as JavaScript constructs" ([Web IDL Standard](https://webidl.spec.whatwg.org/)).
The DOM is not part of the language; it is an interface described in IDL and
installed on the host's global object.

## 2. How JS "interacts with web invariants"

It does not — at the language level. A JS program calls host globals
(`document.*`, `history.*`, `postMessage`, `fetch`, `SharedArrayBuffer`,
`Atomics`), and the **host enforces the invariants**: same-origin, the event
loop and its task/microtask queues, DOM-tree consistency, realm mapping,
secure contexts, the agent memory model. The language supplies *types*
(`Promise`, `Atomics`); the host supplies *policy*. "The HTML DOM is the host
environment when JavaScript is executed in a web browser"
([MDN, JavaScript execution model](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Execution_model)).

## 3. WebAssembly is the closer analogue

- The core wasm spec defines an abstract **embedding interface** (the
  *embedder*): "By design, the scope of the WebAssembly core specification
  does not include a description of how WebAssembly programs interact with
  their surrounding execution environment." The **WebAssembly JS API**
  (W3C) is the JS embedding
  ([wasm-js-api](https://www.w3.org/TR/wasm-js-api/)).
- Imports are a **two-level namespace** — `(import "js" "import1" …)` —
  resolved from an `importObject` of plain JS values: functions, `Memory`,
  `Global`, `Table` ([MDN, using the JS API](https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Using_the_JavaScript_API)).
- **There is no "web" concept in wasm.** Host functions are just JS.
  Browser-specific behavior is a *separate* spec (the WebAssembly Web API,
  WASMWEB).

So the wasm→host boundary is nothing more than **imports from a host
namespace**. The host is whatever embedder instantiated the module.

## 4. The lesson for Briev

- The true boundary is the **host-import namespace** (the embedder).
  `#System` ≈ `wasi_snapshot_preview1`; the browser host ≈ `env`/`js`. A
  *namespace*, not a protocol.
- The browser API surface is Briev's **Web IDL analogue**: boundary types +
  GLUE config + stdlib (`lib/glue/web/types.bv` + `lib/std/web/*.bv`). An
  interface/library layer — **not** a language protocol.
- The invariants (event loop, DOM, security) live in the **host**. Briev's
  analogue is its own execution model (the reactor) + runtime, not a
  compile-time protocol.

`#Web` corresponds to **nothing** in this model. JavaScript has no
language-level web marker; wasm has no web import concept; the platform is
Web IDL + globals. `#Web` is Briev inventing a middle layer the web itself
never had.

---

# Part II — Diagnosis: what `#Web` actually is today

## 5. A dispatch label, not a protocol

`#Web` is **not consumed as a protocol anywhere in the type system**:

- `git grep '#Web'` across `src/casting/`, `src/type_universe*`,
  `src/analysis/type*`, and the typechecker returns nothing outside tests.
- `type Element: #Web` / `type CanvasContext: #Web`
  (`lib/glue/web/types.bv:9,17`) — that parent is **inert**. The width comes
  from `spec MaxBits: 32`, not the parent.
- The only real references in `src/` are: the shim filter
  (`src/compile.rs:1202`), `host_fns` consumption
  (`src/compile.rs:1250`), the dispatch lookup
  (`src/analysis/frgn_dispatch.rs:106`), and tests.

Its sole effect is a calling convention: `FromSpec::Protocol("#Web")` → GLUE
target `web` → `bridge_kind: wasm_runtime`, `calling_convention: wasm_import`
(`lib/glue/web/glue.dbv:14`) → the web generator emits JS import stubs and
`host_fns` supplies JS bodies. That is a *CC + shim + library*, not a
platform.

## 6. The category error

`lib/glue/` is a **language** registry: the struct field is literally
`language` (`src/glue/config.rs:30`), the guide is *"How to add a new FFI
target — adding a language"* (`docs/guides/add-an-ffi-target.md:1-20`), and
8 of 9 targets are languages (`c`, `csharp`, `go`, `java`, `lua`, `node`,
`python`, `rust`).

`web` breaks the invariant:

| Target | `bridge_kind` | `calling_convention` | "language" |
|---|---|---|---|
| c/csharp/go/java/lua/python/rust | extern_c_crate / cgo_package / jni_module / native_module | c_abi / lto | real languages |
| **node** | esm_module | c_abi | **JavaScript** (ffi-napi / `.node`) |
| **web** | **wasm_runtime** | **wasm_import** | **JavaScript** (browser) |

Two targets claim one language (JS), split only by runtime; `web` is the only
target whose binding is a host import table rather than a compiled wrapper.

## 7. Concrete defects

- **`.mjs` collision → nondeterministic routing.** `web` and `node` both
  declare `extension: "mjs"`; `find_language_by_extension` does
  `targets.values().find(...)` over a **HashMap**
  (`src/glue/config.rs:400-406`, used at `src/analysis/frgn_dispatch.rs:178`).
  So `frgn f() from "x.mjs"` resolves to `node` or `web` arbitrarily — a
  direct violation of the HashMap-determinism rule (AGENTS.md).
- **`lib/glue/node/types.bv` is missing** but referenced by
  `types_module: "glue/node/types.bv"`.
- **`compile.rs:1250`** pulls browser host-function bodies out of the
  *language* registry.

## 8. Documentation inconsistencies

- `docs/architecture/conditional-ffi.md:124` (updated 2026-10-06) — *"any
  other `#<Name>` protocol is a GLUE target resolved by name"* — but `:149`
  still says *"Any bare protocol hashword other than `#System` produces a
  compile error."* They contradict; `:149` is stale pre-genericization text.
  (Fixed in this record's landing.)
- The web-routing plan's decision #4 rationale says *"A specific runtime
  (web, node, deno) is temporal and belongs in GLUE configs"* — i.e. it puts
  **runtimes** into a **language** registry. The rationale states the
  conflation it fails to resolve.
- "`#Web` stays a capability" is aspirational: there is no consumer.

---

# Part III — Decision

## 9. Decisions

**D1 — `#Web` is not a protocol; retire it as a protocol and as a GLUE
target. (DECIDED)**
*Alternative:* keep it (Option A) and reclassify honestly as "the browser
host library." *Rejected:* it names none of language, platform, or protocol
cleanly; the JS/Web precedent has no such layer. *Undo:* keep `#Web` parsing
as a GLUE target name; the migration is the only consumer change.

**D2 — The FFI boundary is the host-import namespace (the embedder), not a
protocol. `#System` is the one base host namespace. (DECIDED)**
`#System` → `wasi_snapshot_preview1` on wasm, libc/libSystem on native. A
browser host is the same kind of thing (`env`/`js`). *Alternative:* a
platform registry separate from `#System` — *OPEN*, see §11.

**D3 — Browser APIs are a library (Web IDL analogue) over the JS host.
(DECIDED)**
`lib/std/web/*.bv` becomes thin stdlib wrappers; `lib/glue/web/types.bv`
boundary types become ordinary opaque stdlib types. The GLUE `.dbv` config +
`types.bv` are Briev's Web IDL analogue — the interface-definition layer.
*Consequence:* `type Element`/`CanvasContext` drop the inert `#Web` parent
(keep `spec MaxBits: 32`).

**D4 — GLUE is languages/hosts only; languages get no hashword. (DECIDED)**
A language is reached by file extension (`from "x.py"`, `from "x.rs"`) or by
name for `brievc export|bindings|extension`. `#` marks compiler-known
boundaries only. **`web` is removed from the registry** — the browser is a
host-module path, not a GLUE target. *Reconsideration (2026-10-07):* the
originally-proposed `node`→`js` rename is **deferred** — the `node` target
is Node-specific (ffi-napi, `.node` native module, `node -p` include probe),
not a generic JS target; a future browser/deno/bun target is a separate
concern. `node` keeps its name. *Rationale:* adding a language is a
config-only folder drop; the `#` vocabulary must never grow with languages.

**D5 — The platform is the target/backend. (DECIDED)**
`.rbv` selects webstack/wasm32, as `.bv`/`.abv`/`.ebv`/`.sbv` select theirs.
*Alternative:* a source-level platform marker — *rejected* as redundant with
the extension.

**D6 — Invariants live in the host/runtime, not the compiler. (DECIDED)**
The event loop, DOM consistency, and security are enforced at runtime (the
reactor + the browser host). The compiler declares and type-checks the
boundary; it does not encode web policy.

**D7 — The host-import provenance is a path to a JS host file. (DECIDED
2026-10-07)**
A browser host import is declared `frgn f(...) from "<path>.js";` — a path
resolved like an `import`, naming a real JS host-module file whose
`export const` bodies are inlined into the generated shim. The file IS the
host module. `#Web` is retired as a protocol. *Alternatives rejected:*
implicit-by-target (weak against explicit provenance); a generic `#Host`
marker (unneeded vocabulary); naming the language (`from js` — conflates
host and language; browser vs node are both JS). *Landed:* the `.js`
dispatch branch (`frgn_dispatch::resolve_host_module_frgn`), the host-module
reader/inliner (`compile.rs::read_web_host_modules`, `web_generator::
with_host_module_src`), `lib/glue/web/web.js`, and the `lib/std/web/*.bv`
migration.

---

# Part IV — Migration plan

## 10. Phases (each gated: `cargo test --lib` green, Praetor no new
diagnostics, docs in the same commit, a bundle/`.rbv` e2e check)

- **Phase 0 — this record + doc corrections (this landing).** Land this
  record; mark web-routing decision #4 superseded; fix
  `conditional-ffi.md:149`; add the INDEX pointer. No behavior change.
- **Phase 1 — host profile.** Introduce the host-import profile concept
  (import model, ABI widths, host-function bodies). Move `host_fns` and the
  `wasm_import` ABI out of `GlueTarget`. `#System` gets a trivial profile;
  the browser host gets its profile. *Open:* extend `config/protocols.dbvl`
  or add `config/hosts.dbvl`.
- **Phase 2 — language registry cleanup.** Rename `node`→`js` (folder,
  `glue.dbv`, tests, docs); remove `web` from `lib/glue/`; make
  `find_language_by_extension` deterministic (one `.mjs` owner); add the
  missing `lib/glue/js/types.bv`.
- **Phase 3 — dispatch repoint.** `frgn_dispatch`: `FromSpec::Protocol(p)`
  → host lookup; extension → language lookup. `compile.rs` reads the host
  profile. Decide D7 and wire the host-import provenance.
- **Phase 4 — stdlib/examples migration.** `lib/std/web/*.bv` keep working
  under the new provenance; drop the `#Web` type parent; migrate
  `lib/glue/web/types.bv`.
- **Phase 5 — retire.** Remove the `#Web` token handling, tests, and doc
  references; flip BUGS.md entries; the `.rbv` bundle gate is the regression
  guard.

**Retirement gate (DEBT):** the browser-host profile may keep an internal
name (`web`) in config; the *source-visible* `#Web` protocol is what retires.
The `.rbv` router smoke fixture (`/tmp`-style) and the runtime gate are the
gate.

**Progress (2026-10-07).** Landed:
- Phase 2a: the `.mjs` extension-routing determinism fix + missing
  `lib/glue/node/types.bv`.
- D7 + Phase 3: the `.js` host-module path end-to-end (`lib/glue/web/web.js`,
  `frgn_dispatch::resolve_host_module_frgn`, `compile.rs::read_web_host_modules`,
  `web_generator::with_host_module_src`); all `lib/std/web/*.bv` migrated;
  `#Web` now **errors** with a fix.
- Phase 1: **the host boundary is identity-marshalled** — the webstack
  backend emits the wasm ABI and the shim (from the host file) unmarshals.
  Verified the old `web` target's protocol map only ever produced Identity
  steps, so `resolve_host_module_frgn` no longer consults it. No separate
  host-profile config is needed.
- Phase 2b: **`web` removed from the language registry**
  (`lib/glue/web/glue.dbv` deleted; `web.js` + `types.bv` remain as the
  host's data). `node`→`js` deferred (see D4).
- Phase 4: the inert `#Web` type parent dropped from
  `lib/glue/web/types.bv`.

Gates: `cargo test --lib` 2917 green; router gate 11/11; `check_calls.py`
clean; all `.rbv` examples build; Praetor no new diagnostics.

- Phase 5: **the `#<Name>` protocol namespace is removed** — `#System` is the
  only protocol hashword; any other `#<Name>` (including `#Web`) errors with
  a fix naming the host-module path. Docs swept (`hash-words.md`,
  `agent-reference.md`, `conditional-ffi.md`, `SPEC.md` §19.2, the router
  header).

**Migration complete.** The browser is a library over a host-module path;
`#Web` is retired as a protocol; the `web` GLUE target is gone; `#System`
is the one protocol hashword.

---

# Part V — Contract

## 11. Open items

1. **D7 — host-import provenance. (DECIDED 2026-10-07)** A path to a JS
   host file (`from "glue/web/web.js"`), resolved like an import; the file
   is the host module. See D7 above.
2. **Platform axis — separate or subsumed?** Is a host/platform concept real
   enough to name, or does the target/backend fully subsume it? Lean:
   target subsumes platform; the only hashword left is `#System`.
3. **`#System` vs browser host on one wasm target.** Both are host import
   namespaces on wasm32 (WASI vs `env`/`js`); the provenance must distinguish
   them.
4. **Naming.** `#Web` vs `#Browser`; the internal host-profile key.
5. **Event loop / invariants integration.** Whether Briev's reactor maps
   onto the host event loop as a declared boundary (future work, not this
   record).

## 12. References

- Web IDL Standard — https://webidl.spec.whatwg.org/
- HTML Standard, Web application APIs — https://html.spec.whatwg.org/multipage/webappapis.html
- ECMAScript host/implementation equivalence — https://github.com/tc39/ecma262/issues/1524
- WebAssembly JS API — https://www.w3.org/TR/wasm-js-api/
- Using the WebAssembly JavaScript API — https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Using_the_JavaScript_API
- MDN, JavaScript execution model — https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Execution_model
- `docs/architecture/glue-ffi.md`, `hash-words.md`, `conditional-ffi.md`
- `docs/plans/2026-10-06-web-routing-and-bundling.md` (decision #4 superseded)
