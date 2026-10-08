# Web Routing Boundary

**Date:** 2026-10-08
**Status:** implemented (Plan 2026-10-08-file-based-routing.md)
**The one line:** the compiler owns page *identity* + per-file *provenance*; the
stdlib/tool owns *route policy* + *nav generation*.

## The decision

File-based routing is a **framework concern, not a compiler concern**. The
compiler carries the *eternal* (each page's declared identity + the sibling
asset names); the stdlib (`std/web/pages.bv`) and the build tool (`brievc web
<dir>`) carry the *temporal* (which key maps to which URL, and how the shared
nav is generated). The seam is the decisive cut.

Against the three golden questions:

1. **General vs special-case.** A route table is an *algorithm shape*
   (URL→page mapping). Rule 24 (proofs-not-shapes) and Rule 23 (no vocabulary
   matching) forbid the compiler owning it. Emitting "my declared page key" is
   general — the `#Link<name>` pattern (a declared fact, emitted verbatim, never
   interpreted).
2. **Forever vs config.** The `--no-stdlib` test: per-file compilation is
   intrinsic; "which URL maps to which page, in what order, with what layout"
   is **configuration** — `folio.toml [web]` + a stdlib `.bv`, never Rust match
   arms.
3. **Only rule left.** Delete every routing algorithm from the stdlib; an author
   writes next year's routing with contracts; it must hit the ceiling with
   **zero compiler changes**. A compiler-embedded route table fails that test.

Why not pure "hand-link hrefs": the pre-seam `multi_page_a.rbv` hardcoded
`<a href="multi_page_b.html">`. Rename page B and page A breaks — temporal
knowledge leaking into source. The seam kills it: the compiler stamps each
page's declared key + emits a per-file manifest; the stdlib/tool *generates* the
cross-links from those manifests.

## What the compiler owns (Part 1 — the seam)

Thin, general, additive, eternal:

- **`folio.toml [web]` section** (`src/manifest.rs`). `[web.pages]` maps page
  key → `.rbv` file path. The key is the page's declared identity; the file is
  what `brievc build` compiles. The compiler reads it to know the key for the
  file it's compiling — it never *interprets* the key as a route.
- **`data-briev-page="<key>"` on `<body>`** (`src/glue/web_generator.rs`).
  Stamped in both the bundle (`render_bundle_html`) and the split index
  (`render_index_html`). Provenance only — an undeclared page (no `[web]`
  entry) omits the attr entirely.
- **`<stem>.page.json` manifest** (`--split` mode only). `{page, html, wasm,
  shim}` beside the assets. Bundle mode emits no sibling file (the bundle is
  self-contained; the key rides the attr).
- **`brievc web <dir>`** (`src/main.rs`). Thin orchestration over the existing
  per-file `compile_source` (Part 1 seam) — NO new codegen. Builds every page in
  `[web.pages]` + writes `nav.json` (the ordered page set) + `nav.html` (the
  shared `<a href>` set) from the per-file manifests.

## What the framework owns (Part 2 — the route policy)

Temporal, swappable, NOT load-bearing:

- **`lib/std/web/pages.bv`** — the route-policy library. Supplies the URL↔page
  mapping (`page_href`, `route_name`, `current_path`) over the browser host
  (`location`/`navigate` from `glue/web/web.js`). Precedent: `lib/std/web/router.bv`
  already established routing as a stdlib framework over the browser host, no
  compiler keyword.
- **The nav generator** — the `brievc web` tool reads the per-file `page.json`
  manifests and emits the shared nav. This is the *routing algorithm*; it lives
  in the tool/stdlib so it evolves without touching the compiler.

**Not load-bearing:** the compiler's seam works without `pages.bv`. An app can
route with plain `<a href>` to a sibling's `<stem>.html` if it prefers. `pages.bv`
is a convenience that can be deleted or rewritten next year with zero compiler
changes (the Rule 24 test).

## The seam, diagrammed

```
   folio.toml                compiler (Part 1, eternal)          framework (Part 2, temporal)
   [web.pages]              ┌─────────────────────────────┐     ┌──────────────────────────┐
   a = "a.rbv"  ──────────► │  stamp data-briev-page="a"   │     │  pages.bv: page_href,     │
   b = "b.rbv"  ──────────► │  emit a.page.json (split)    │ ──► │  route_name, current_path │
                             │  brievc web: nav.json+html   │     │  brievc web: nav.json/html│
                             └─────────────────────────────┘     └──────────────────────────┘
                                  PROVENANCE (identity+assets)      ROUTE POLICY (URL↔key, nav)
```

The arrow is one-way: the framework *consumes* the compiler's provenance. The
compiler never reads the route policy.

## How to undo it

- **Drop the seam (revert Part 1):** delete `web_page_key` from `BuildOptions`
  + `run_build`/`run_web`/`render_*` call sites; delete `WebConfig` + the `web`
  field from `Manifest`; delete `render_page_manifest`. The pages fall back to
  hand-written `<a href>` (the pre-seam state). No compiler *behavior* change —
  the seam is purely additive.
- **Drop the route policy (revert Part 2):** delete `lib/std/web/pages.bv` + the
  `brievc web` subcommand + the gate step 5b. The seam (Part 1) still stamps the
  key + manifest; an app routes by hand.

## Gate

`bash benchmarks/rbv_gate.sh` — step 5 (self-contained bundles + cross-links)
and step 5b (`brievc web` → nav.json + nav.html + stamped keys). `cargo test
--lib` green.
