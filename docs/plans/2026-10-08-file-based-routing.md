# File-Based Routing — Compiler Seam + Stdlib Nav Generator

**Date:** 2026-10-08
**Status:** active
**Fork:** B (compiler seam + stdlib nav generator)
**Key location:** `folio.toml [web]`

## The decision

File-based routing is a **framework concern, not a compiler concern**. The compiler
carries the *eternal* (page identity + per-file provenance); the stdlib carries the
*temporal* (route policy + nav generation). The seam is the decisive cut.

Against the three golden questions:

1. **General vs special-case:** a route table is an *algorithm shape*
   (URL→page mapping) — Rule 24 (proofs-not-shapes) and Rule 23 (no vocabulary
   matching) forbid the compiler owning it. Emitting "my declared page key" is
   general (the `#Link<name>` pattern).
2. **Forever vs config:** `--no-stdlib` test: per-file compilation is intrinsic;
   "which URL maps to which page, in what order, with what layout" is
   **configuration** — `folio.toml [web]` + a stdlib `.bv`, never Rust match arms.
3. **Only rule left:** delete every routing algorithm from the stdlib; an author
   writes next year's routing with contracts; it must hit the ceiling with **zero
   compiler changes**. A compiler-embedded route table fails that test.

Why not pure "hand-link hrefs": the current `multi_page_a.rbv` hardcodes
`<a href="multi_page_b.html">`. Rename page B and page A breaks — temporal
knowledge leaking into source. The seam kills it: the compiler stamps each page's
declared key + emits a per-file manifest; the stdlib *generates* the cross-links
from those manifests.

## What the compiler already is

- `brievc build <file.rbv>` = one file → one self-contained `.html` (default bundle)
  or `--split` (sibling `.css/.mjs/.wasm/.html`). No directory awareness, no route
  table today.
- Emission site: `src/compile.rs:1337-1359` (`render_bundle_html`, writes
  `<stem>.html`) and `:1193-1221` (`render_index_html` under `--split`).
- `render_bundle_html` / `render_index_html` in `src/glue/web_generator.rs:43-101`.
- `folio.toml` already exists as the project manifest (`src/manifest.rs`) with
  `[project]`, `[dependencies]`, `[target.*]` profiles — the natural home for `[web]`.

## Part 1 — the compiler seam (thin, general, additive)

### 1a. `folio.toml [web]` section

Add a `web` field to `Manifest` (`src/manifest.rs`). Shape (keyed map):

```toml
[web]
[web.pages]
counter = "counter.rbv"
about   = "about.rbv"
```

The **page key** is the map key (`counter`); the **file** is the value. The key is
explicit, not derived from the stem — it survives renames. The compiler only reads
this to (a) know the key for the file it's compiling, and (b) — in the Part 2b
tool — know the sibling set. The compiler never *interprets* the key as a route.

### 1b. Stamp the key into the artifact

When compiling `<stem>.rbv` and its key is known (from `[web]`), the compiler
stamps `data-briev-page="<key>"` on `<body>` in both `render_bundle_html` and
`render_index_html`. Additive: a page with no key is unchanged (attr absent).
Provenance, not routing — the compiler carries its declared identity, nothing more.

### 1c. Per-file `page.json` manifest (`--split` only)

In `--split` mode, write a tiny `<stem>.page.json` beside the assets:

```json
{ "page": "counter", "wasm": "counter.wasm", "html": "counter.html", "exports": [...] }
```

In bundle mode: **no sibling file** (the bundle is self-contained; the key rides
the `data-briev-page` attr). The manifest is what the framework consumes to wire
nav without the compiler knowing what navigation means.

### 1d. No new compiler codegen path

No `brievc build --web-dir`. The directory of pages is already a *project*
(`folio.toml` *is* the project manifest). The "build all pages + wire nav" step is
Part 2b — a **thin build-tool wrapper** that reuses the existing per-file compile
path and adds no new codegen.

## Part 2 — the framework (stdlib + thin tool, temporal, swappable)

### 2a. `lib/std/web/pages.bv` — the nav generator

A stdlib module that reads the per-file `page.json` manifests (via a browser host
fn: `frgn read_manifest(path: String) -> String from "glue/web/web.js"`) and
generates the shared nav — the `<a href>` set — as a view fragment. This is the
*routing algorithm*: it lives in the language, evolves without touching the
compiler, and passes the Rule 24 test (delete it, write next year's routing, zero
compiler changes).

Precedent: `lib/std/web/router.bv` already established routing as a stdlib
framework over `location`/`navigate` from `glue/web/web.js`, no compiler keyword.
`pages.bv` extends that to the *multi-page, file-based* case.

### 2b. `brievc web <dir>` — thin orchestration

A small `brievc` subcommand: for each `.rbv` in `folio.toml [web] pages`, invoke the
existing `compile_source` (Part 1's seam), then run the Part 2a nav pass to produce
the shared nav. **Build orchestration, not a new backend** — reuses the existing
per-file compile path, adds no new codegen.

### 2c. Retire the hand-written hrefs

`examples/multi_page_a.rbv` / `multi_page_b.rbv` are **regenerated** by Part 2b
instead of hand-linking. The brittle `<a href="multi_page_b.html">` is replaced by
nav generated from the manifests. The `B2` gate step is updated to assert the
generated nav.

## Docs (same commit as the structural change)

- `docs/architecture/web-routing-boundary.md` — the decision record (the seam, the
  three-golden-question derivation, the boundary diagram).
- `spec/SPEC.md` §21.1 — the `[web]` section, the `data-briev-page` stamp, the
  `page.json` manifest, the nav-generator contract.
- `docs/plans/INDEX.md` — add the entry.
- `docs/architecture/features/rendered-briev-wasm.md` — the boundary note
  (cross-link to the decision record).

## Gates

- `cargo test --lib` green.
- `bash benchmarks/rbv_gate.sh` green (updated B2 step + browser smoke).
- Praetor base-vs-now on `src/manifest.rs`, `src/compile.rs`,
  `src/glue/web_generator.rs` — no NEW diagnostics.

## Sequencing (each a separate commit, tests green before next)

1. **Part 1a** — `folio.toml [web]` section (manifest parse + tests).
2. **Part 1b** — stamp `data-briev-page` into the artifact (tests).
3. **Part 1c** — `page.json` manifest in `--split` (tests).
4. **Part 2a** — `lib/std/web/pages.bv` nav generator + `web.js` host fn (tests).
5. **Part 2b** — `brievc web <dir>` thin wrapper (tests).
6. **Part 2c** — regenerate `multi_page_a/b`, update the B2 gate (gate green).
7. **Docs** — decision record + SPEC + INDEX + feature doc.

## Resolved open questions

1. `[web] pages` shape → **keyed map** (key = page name, value = file path). The
   key is explicit, not derived from the stem — survives renames.
2. Part 2b location → **`brievc web <dir>`** subcommand (thin orchestration, reuses
   `compile_source` per file + runs the nav pass). More discoverable for the
   stranger-gate.
3. `page.json` in bundle mode → **no sibling file** (bundle is self-contained; key
   rides the `data-briev-page` attr). In `--split` mode, emit `page.json` beside the
   assets for tooling/nav.

## Progress log

- **2026-10-08 — Part 1a done:** `WebConfig` + `web` field in `Manifest`
  (`src/manifest.rs`); `folio.toml [web.pages]` parses (2 tests); the two
  `resolver.rs` test initializers updated for the new field.
- **2026-10-08 — Part 1b done:** `web_page_key: Option<String>` in `BuildOptions`
  (`src/pipeline.rs`); resolved from `[web.pages]` by file name in `run_build`
  (`src/main.rs`); `data-briev-page="<key>"` stamped on `<body>` in both
  `render_bundle_html` + `render_index_html` (`src/glue/web_generator.rs`, 1 test);
  all `BuildOptions`/`Manifest` struct literals across `main.rs`, `resolver.rs`,
  `spirv/mod.rs` updated for the new fields.
- **2026-10-08 — Part 1c done:** `render_page_manifest` (`src/glue/web_generator.rs`);
  `<stem>.page.json` written beside the `--split` assets when the page has a
  declared key (`src/compile.rs`, 1 test). Bundle mode emits no sibling file.
- **2026-10-08 — Part 2a done:** `lib/std/web/pages.bv` — the route-policy library
  (`page_href`, `route_name`, `current_path`, `ends_with`) over the browser host.
  Type-checks; covered by the conformance sweep (`lib/std` root). Swappable, NOT
  load-bearing.
- **2026-10-08 — Part 2b done:** `brievc web <dir>` subcommand (`src/main.rs`) —
  thin orchestration over the per-file `compile_source` (Part 1 seam), NO new
  codegen. Builds every `[web.pages]` page + writes `nav.json` + `nav.html`.
  Verified end-to-end (bundle + `--split` modes).
- **2026-10-08 — Part 2c done:** `examples/folio.toml` declares the page set;
  `rbv_gate.sh` step 5b runs `brievc web` and asserts nav.json + nav.html +
  stamped keys. Gate green (6 steps + 5b + browser smoke).
- **2026-10-08 — Docs done:** decision record
  (`docs/architecture/web-routing-boundary.md`), SPEC §21.1, INDEX entry, feature
  doc boundary note.
