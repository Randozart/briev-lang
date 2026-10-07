# Web routing, bundling, and the `#Web` protocol (2026-10-06)

**Status:** Active
**Umbrella:** `docs/plans/2026-10-04-three-surfaces-functional.md` Phase 3 (`.rbv`)
**Predecessors:** Phase 1 (folio package/install, DONE); the `.rbv`-servable
base fix (sibling asset references + `render_index_html`).

## Goal

A stranger can build a `.rbv` into **one self-contained HTML file** (bundle by
default), and route between pages **without any compiler keyword, intrinsic, or
new hashword** — routing is a stdlib framework over standard Briev FFI.

## Decisions (2026-10-06)

1. **Bundle is the default**; `--split` is the opt-in for separate assets.
   (Fewer files after compilation; distribution-friendly.)
2. **Routing is a stdlib framework, not a compiler keyword.** Precedent:
   JavaScript — the web language — has no routing; it is libraries over the
   browser primitives (`location`, `history`, `popstate`). Rule 23/24.
3. **No new intrinsic, no new hashword.** The browser boundary is reached with
   standard Briev `frgn … from #Web`.
4. **`#Web` genericization (Option A):** `#Web` stays a capability but stops
   being a compiler-hardcoded protocol. `from #<Name>` resolves a **GLUE target
   by name**; `#System` remains the single special base protocol.
   - Rationale (the user's framing): `#System` is the C-era base ABI — the
     eternal thing compilers exist to abstract. A *specific runtime* (web,
     node, deno) is temporal and belongs in GLUE configs.
   - **SUPERSEDED 2026-10-07** by
     `docs/architecture/web-host-boundary-decision-record.md`. Option A kept
     `#Web` as a GLUE target; that record concludes `#Web` is **not a
     protocol** and retires it, reframing the browser as a **library over a
     host-import namespace** (the JS/Web + wasm-embedder shape). `#System`
     remains the one base host namespace. This decision item is retained for
     the historical record only.
5. **Host JS shipping: option (c)** — GLUE `[web]` templates gain a per-frgn JS
   body so a framework's host functions ship inside the generated shim
   (bundles for free).
6. **`protocols.dbvl`:** remove the dead `#Web` entry (the bridge path returns
   before `target.rs::resolve`); keep the file for `#System`-class base
   protocols.
7. **Router home:** `lib/std/web/router.bv` (stdlib `web` package).
8. **Multi-page order:** B2 (static, one bundled HTML per page, `<a href>`)
   first; then B1 (SPA in one bundle + the stdlib router). File-based routing
   (folio `[pages]`) is a build/framework convention, never a compiler keyword.

## Part 0 — finish the `.rbv`-servable base

`src/compile.rs`, `src/glue/web_generator.rs`, `src/ssr.rs`: the index HTML and
SSR page reference their emitted sibling artifacts by basename
(`<stem>.css/.mjs/.wasm`), never the old hardcoded `app.css`/`dom-shim.mjs`, and
the wasm `fetch` is relative. `render_index_html` extracted + unit-tested.
Gate: tests green, Praetor, commit.

## Part 1 — `#Web` genericization (Option A)

- `src/analysis/frgn_dispatch.rs`: replace the hardcoded `#Web` branch with a
  generic `FromSpec::Protocol(p)` (p ≠ `#System`) → resolve GLUE target by name
  (`p.trim_start_matches('#').to_lowercase()`), `ResolvedFrgn::Bridge`. Unknown
  → helpful error listing available GLUE targets. `#System` keeps its base-ABI
  branch.
- `src/target.rs`: drop the `#System`/`#Web` whitelist; `#System` stays special,
  other protocols validate generically.
- `config/protocols.dbvl`: remove the dead `#Web` entry; fix the comment.
- Docs: correct the stale "`#System` is the sole protocol" claims
  (`src/ast/top.rs:832`, `config/protocols.dbvl`, `AGENTS.md`); state that
  `#System` is the eternal base ABI and `#<Name>` protocols are GLUE targets.
- Tests: `#Web` frgn still resolves to the web bridge; a bogus `#Nope` errors;
  a new glue folder resolves with zero compiler change.
- Stdlib unchanged: `lib/std/web/*` and `lib/glue/web/types.bv` keep `#Web`.

## Part 2 — bundle-by-default

- Default `.rbv` output = one self-contained `<stem>.html`: inline `<style>`,
  inline shim module, wasm inlined (small dependency-free `base64_encode`). No
  external refs; works offline / `file://`.
- `--split` = today's html + mjs + css + wasm; `.d.ts` split-only.
- `render_bundle_html(view_html, stem, css, shim_src, wasm_bytes)`; flip the
  webstack arm default; thread `--split` through `BuildOptions`.
- File the `encoding::base64_*` no-op-stub bug separately (do not reuse).

## Part 3 — host-JS shipping (c)

GLUE `[web]` template gains a per-frgn JS body: a `#Web` frgn declaration may
carry its host JS, emitted into the generated shim's import stub. The router's
`location`/`navigate`/popstate JS ships this way.

## Part 4 — router framework (`lib/std/web/router.bv`)

- A route-table type; `frgn location() -> String from #Web;`,
  `frgn navigate(url: String) from #Web;`, a popstate frgn → state.
- Page mounting via `b-when`; nav via `b-trigger`; nav lists via `b-each`.
- Pure standard Briev — no compiler keyword, no intrinsic, no new hashword.

**Status 2026-10-06:** framework WRITTEN (`lib/std/web/router.bv`) and
typechecks (conformance green), but BLOCKED at runtime by two wasm32 codegen
bugs (BUGS.md, OPEN): a `#Web` frgn String RETURN lowers to an opaque `ptr`
bridge call (`call ptr @bridge_location()`, needs i32), and an imported `obj`
with a String field emits `store [1 x ptr]`. These are compiler defects; the
router smoke fixture is their regression gate. Popstate (a callback frgn) is
deferred until the String-return ABI is fixed.

## Progress log

- **Part 0** DONE `38ada64b` — serve-able base (sibling asset refs).
- **Part 1** DONE `4e56121d` — `#Web` is a GLUE target by name.
- **Part 2** DONE `033b2300` — bundle-by-default, `--split`.
- **Part 3** DONE `a4bb0393` + `aac37915` — valid `#Web` stubs; host-JS
  shipping from the GLUE `[web]` config.
- **Part 4** framework written `3daa0d90`; blocked on wasm codegen (above).
- **Part 5** not started.

## Part 5 — multi-page

- **B2:** one bundled HTML per `.rbv`, `<a href>` between them. No router.
- **B1:** one bundle + the Part-4 router.
- **File-based routing:** folio `[pages]` over multiple `.rbv` → route table
  (build/framework convention).

**Status 2026-10-07** (plan `2026-10-07-web-surface-completion.md`): **B2
DONE** — `examples/multi_page_{a,b}.rbv` cross-link; `benchmarks/rbv_gate.sh`
asserts self-containment + the cross-link. **B1** covered by the router
fixture in bundle mode. File-based routing deferred.

## Sequencing & gates

0 → 1 → 2 → 3 → B2 → B1.
Per landing: `cargo test --lib` green; Praetor no new diagnostics; docs in the
same commit; a bundle e2e check asserting zero external references.

## Documentation maintenance (Rule 3)

- This plan; `docs/architecture/folio.md` if the manifest grows `[pages]`;
  `docs/architecture/hash-words.md` + `docs/architecture/glue-ffi.md` for the
  `#System`-is-base / `#<Name>`-is-GLUE rule; `docs/architecture/features/
  webstack-intrinsics.md` + `rendered-briev-wasm.md` for bundle-by-default;
  `spec/SPEC.md` for the `.rbv` output contract. Historical records untouched.
