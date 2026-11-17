# Skip-origin reporting — implementation plan

**Goal:** make `--verify-report` say whether each finding belongs to the file
being compiled or to an imported/prelude module, so the census stops publishing
"declaration × compilation unit" as if it were program surface.

**Non-goal:** any change to a verdict. `proven` / `unproven` / `violated` must be
byte-identical after this change; only labels and aggregate counts are added.

## 0. Findings that shape the design

### 0.1 Duplication is real and documented, but nothing acts on it

`BodyReport::to_json` already carries this comment:

```
// The prelude is included in this list, so consumers intersect with
// their own declarations.
```

No consumer intersects. `scripts/verify-bodies-census.sh:72-74` reads
`len(d['skipped'])` and sums across 92 files; `scripts/verify_bodies_corpus.sh:142`
counts one skip line. Hence 2007 lines for 25 prelude declarations
(see `2026-11-17-generics-investigation.md` §2b), and `proven: 2216` is inflated
the same way — the prelude is re-proved in every unit.

### 0.2 `item_origins` exists but cannot be used here

`ImportResolver.item_origins: Vec<Option<u32>>` is a parallel shadow vector over
the **resolver's** output (`None` = root item, `Some(id)` = `modules[id]`, giving
`source_path`). It looks like the obvious mechanism and it is a trap:

`src/compile.rs` mutates `items` five times between resolution (line 380) and the
prover (line 861):

```
extract_inline_stage_blocks(&mut items, &mut pm);
evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
pm.run_ast(StageKind::Resolved, &mut items, &mut TypeUniverse::new())?;
resolve_comptime_refs(&pm, &mut items)?;
expand_composites(&mut items, &pm)?;
```

Index `i` in `item_origins` no longer means `items[i]`. Threading provenance
through all five (and every future mutation) is a much larger, riskier change than
the reporting problem justifies. **Record this so the next person does not try it.**

### 0.3 `Span` has no file identity

`briev_verify::span::Span` is `{start, end, line, column}` — per-file offsets with
no file id, so a merged item cannot be attributed from its span either.

### 0.4 What *is* available: names

- `compile.rs` has the root file's items **before** `resolve_imports` (line 380) —
  the file's own declarations, prelude import statements not yet resolved.
- `ImportResolver.loaded_modules` caches each module's resolved items, keyed by
  specifier — currently private, but names are all we need.

So classification is by **name**, at the report boundary, and both sides strip
`Export`/`SyncGroup` wrappers so the names match what the report prints.

## 1. Design

### 1.1 Keep the verification loop untouched

`BodyReport` stays `Vec<String>` per bucket — 34 call sites and the whole prover
untouched. Labels are computed at serialisation:

```rust
/// Which declarations belong to the unit being compiled.
pub struct OriginTable { own: HashSet<String>, imported: HashSet<String> }

impl BodyReport {
    /// Unchanged behaviour: no origin labels, byte-identical JSON.
    pub fn to_json(&self) -> serde_json::Value { self.to_json_with_origins(&OriginTable::default()) }

    /// Additive: `origin` per finding, `summary` per bucket.
    pub fn to_json_with_origins(&self, t: &OriginTable) -> serde_json::Value { … }
}
```

When the table is empty the `origin` key is **omitted**, so every existing test and
fixture that asserts on the JSON keeps passing byte-for-byte. That is the
regression guard for the schema.

### 1.2 Two labels — `own` wins (revised during implementation)

The plan proposed a third label, `ambiguous`, for a name declared on both sides.
Measurement killed it. The case that actually occurs is not shadowing but a file
re-loaded as its own module: the prelude imports `std/cast_lanes.bv`, which
imports `std/float_fmt.bv`, so compiling `float_fmt.bv` loads it again and all 23
of its declarations landed in `ambiguous` — the file's own work attributed to
nobody.

A genuine cross-file clash cannot reach the prover at all: the import-conflict
rule rejects two modules providing the same name. So `own` wins:

| label | condition |
|---|---|
| `own` | declared in the root AST, or claimed by nobody, or claimed by both |
| `imported` | declared by an imported module and not by the root |

Synthesised decls (comptime/macro) are in no module → `own`, which is correct:
they belong to this unit. Effect of the revision on the corpus: own skipped
262 → 404, i.e. the first version was under-reporting the file's own surface.

### 1.3 Per-finding shape (additive)

```json
{ "name": "iter_map_loop", "reason": "generic or variadic …", "origin": "imported" }
```

plus a top-level block:

```json
"summary": {
  "skipped":  { "own": 91, "imported": 13308, "ambiguous": 0, "total": 13399 },
  "proven":   { "own": 412, "imported": 1804, "ambiguous": 0, "total": 2216 },
  "unproven": { … }, "violated": { … }
}
```

`total` is included so the invariant `own + imported + ambiguous == total ==
len(bucket)` is checkable by a consumer without re-counting.

### 1.4 One human summary line, wording chosen to not collide

The corpus runner greps `/verify-bodies skipped:/`; the census prints per-line.
Add:

```
info: --verify-bodies: 2216 proven (412 own), 13308 skipped (91 own)
```

Note the colon placement ("`--verify-bodies:`", not "`--verify-bodies skipped:`")
so no existing grep matches it — a summary that silently changes a downstream
count is exactly the class of bug this change exists to fix.

## 2. Work items

| # | file | change | risk |
|---|---|---|---|
| P1 | `briev-verify/src/verify.rs` | `OriginTable`, `to_json_with_origins`, summary; `to_json` delegates | low — pure addition |
| P2 | `src/import_resolver.rs` | `pub fn imported_decl_names(&self) -> HashSet<String>` over `loaded_modules`, wrappers stripped | low — read-only accessor |
| P3 | `src/compile.rs` | capture `own_names` before line 380; pass table at the report branch (861) | low |
| P4 | `src/compile.rs` | one summary `eprintln!` in the existing block | trivial |
| P5 | `scripts/verify-bodies-census.sh` | sum `summary.*.own`; print raw **and** own; keep violated-exit | low |
| P6 | `scripts/verify_bodies_corpus.sh` | same source of truth for its bucket line | low |
| P7 | `spec/SPEC.md` §10.7 | counting rule + schema rows + restated numbers | medium — normative |
| P8 | tests | see §3 | — |
| P9 | `CHANGELOG.md` + version | see §5 | — |

## 3. Tests (the point of the exercise is a measurements change, so prove it)

1. **Schema stability** — `to_json()` output for a fixed `BodyReport` is identical
   to today's (assert no `origin`/`summary` keys). Guards every existing consumer.
2. **Classification** — `OriginTable` unit test: own-only, imported-only, both
   (`ambiguous`), neither (`own`), plus `Export`-wrapped names.
3. **Invariant** — for any report, `own + imported + ambiguous == total` and
   `total == len(bucket)`, asserted inside `to_json_with_origins` as a debug
   assertion *and* by a test over a real report.
4. **Verdicts unchanged (the safety property)** — compile a fixture that imports
   `std/iterator.bv`, run with and without the origin table, assert
   `proven`/`unproven`/`violated` **counts are equal**, and only labels differ.
   Without this test the change cannot be distinguished from a prover edit.
5. **End-to-end** — a fixture with its own generic + an imported one: `own` count
   is 1, `imported` > 0, and the prelude's 25 iterator generics land in
   `imported` (this is the case that produced the 2007).
6. **Script** — census on a 2-file list reports both raw and own totals.

## 4. Measurement (done — 2026-11-17)

Corpus: `find lib/std -name "*.bv"` → 92 files, release binary (a **debug** build
trips the debug-only HIR/AST liveness assert on `std/os/dir.bv` and loses that
file; the 0.2.0 baseline was release, so the comparison must be release).

| | proven | unproven | skipped | violated | errors |
|---|---|---|---|---|---|
| raw entries (decl × unit) | 2 216 | 101 | 13 308 | 0 | 1 |
| **own** (corpus file's own) | **40** | **6** | **404** | 0 | — |

Two results, both load-bearing:

1. **The raw totals reproduce 0.2.0 exactly** — 2 216 / 101 / 13 308 / 0 / 1 — so
   the change is metadata-only, proven rather than asserted.
2. **97% of the skip surface was imported code.** Quotes in the roadmap and spec
   had been reading 13 308 as program surface; it is 404.

The attribution also surfaced a gap that had been invisible: `read32le`
(`std/ffi/xxhash.bv`) is one of the six own unproven declarations and had never
been named in any bug list.

The single build error is `std/cast_portable.bv` (pre-existing import conflict with
`cast_lanes.bv` over `int_to_str`), not a verification result.

## 5. Version and spec decision

**Decided: 0.3.0.** The report schema gains fields and the published census
semantics change, so the artefact's output surface widened — a pre-1.0 minor bump
by the project's own rule.

## 6. Deliberately excluded

- **`prelude` as a third origin label.** It needs the prelude specifier set in
  Rust (today it lives in `plugins/parsed/prelude-*.bv` as `Import$` data). Nice
  to have; `imported` + per-file reports answer the census question already.
  Revisit only if a consumer needs prelude-vs-user-import.
- **Deduplicating verification itself** (prove each prelude body once). Sound in
  principle — proof context differs per unit only via types and globals — but it is
  a prover change with a real soundness surface, and it is not needed to fix the
  numbers. Keep it as a separate note.
- **`item_origins`.** See §0.2.
