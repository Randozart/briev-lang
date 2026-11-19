# Next steps — investigation (2026-11-19)

`main` is at `094ead0a`, green. Two follow-ups are worth doing; this doc scopes
them precisely. The first (glue generator) has a subtlety my earlier write-up
missed, and the second (spec drift) is a fast doc-sync.

## A. Issue #132 — the glue generator, corrected

### A.1 The original proposal had a cycle bug

Issue #132's "option 1" was: add an `ops_module` field and have the generator emit
`import "glue/c.bv"` into `protocols.bv`. That is wrong. `lib/glue/c.bv` **imports
`glue/c/protocols.bv`** (line 81), and the import resolver **rejects** cycles
(`src/import_resolver.rs:1117` → "Circular import detected"). So
`protocols.bv → c.bv → protocols.bv` would make every C-glue program a hard error.

### A.2 The real structure (measured)

```
lib/glue/c.bv          lines 43–79  defn str_to_c, cstr_to_briev, cstring_concat
                                  (lane ops — depend ONLY on std/cast_lanes.bv)
                       line  81     import "glue/c/protocols.bv"   (for CStr/C_String
                                  in the later list helpers)
lib/glue/c/protocols.bv (generated) type CStr + proto C_String { axioms naming
                                  str_to_c / cstr_to_briev } — imports NOTHING
lib/glue/c/types.bv    (types_module) header says "place for hand-written helpers";
                       currently just `import "glue/c/protocols.bv"`
```

The lane ops are **self-contained** — `str_to_c(handle: String) -> Int`,
`cstr_to_briev(p: Int) -> String`, `cstring_concat(a: Int, b: Int) -> Int`. None
references `CStr`/`C_String`. So they do not need `protocols.bv` at all.

Blast radius: `render_protocols` requires a class with `boundary`+`protocol`+`base`
+`to_op`+`from_op`. Only **`c`** has all five (python names `to_op`/`from_op` but
no boundary/protocol/base, so it renders no protocols file — confirmed: `c` is the
only `lib/glue/*/` with a `protocols.bv`). The generator change is therefore
effectively C-only.

### A.3 The correct acyclic fix

1. **Move the three lane ops** (`str_to_c`, `cstr_to_briev`, `cstring_concat`)
   from `lib/glue/c.bv` into `lib/glue/c/types.bv` — its header already claims to
   be "the place for hand-written helpers". `types.bv` imports only
   `std/cast_lanes.bv`; drop its `import "glue/c/protocols.bv"` (no longer needed).
2. **Generator** (`src/glue/generator.rs::render_protocols`): when it emits a
   protocols file, prepend `import "<types_module>";`. This is data-driven — it
   reuses the existing `types_module` field, so no new schema field and no
   `if lang == …` (respects the module's stated "no language knowledge" rule).
3. **`c.bv`** keeps `import "glue/c/protocols.bv"` for the boundary types; the
   list helpers now get the lane ops transitively (`c.bv → protocols.bv →
   types.bv`). Remove the three moved defns.

Resulting graph is acyclic: `types.bv → (std only)` · `protocols.bv → types.bv` ·
`c.bv → protocols.bv`.

### A.4 Test/determinism fallout (must be handled together)

- `src/glue/generator.rs` unit tests assert exact rendered output
  (`renders_a_boundary_type_and_its_axioms`, `the_shipped_c_adapter_renders_its_string_lane`,
  `rendering_is_deterministic_and_sorted`) — expected strings gain the import line.
- `render_and_write(…, check=true)` compares rendered vs on-disk, so
  `lib/glue/c/protocols.bv` **must be regenerated** via `brievc render c` (never
  hand-patched) or the check-mode gate fails.
- A regression test should assert the generated file contains
  `import "glue/c/types.bv";` AND that `c.bv` no longer defines the ops, so a
  future revert re-introduces neither the undefined-op nor the cycle.

### A.5 Ordering / ownership note

Randy already hand-added `import "glue/c.bv"` to `generated_glue.bv` /
`handwritten_glue.bv` (the glue *declaration* fixtures), which is why the build
passes today. That is a stopgap at the fixture layer. My PR #131's driver-level
`import "glue/c.bv"` in `parity_*.bv` is now **redundant** (the glue files carry
it) — fold its removal into this PR.

## B. Spec drift — §10.7 still lists `pf_exp` as unproven

`spec/SPEC.md:2563` names six own-unproven declarations and includes `pf_exp` with
the note "stating the domain in the precondition was tried and does not close it".
That is stale: `407c7d79` (W4) **proved `pf_exp`** via tuple call-site facts +
constant negation, and the spec's own "pf_exp ×95" tail (line ~2573) is also gone.
Re-run the census on `094ead0a`, then update the table + counts and drop `pf_exp`
from the named gap list. Small, mechanical, but a spec that still claims an
already-closed gap is wrong in the direction that matters (it undersells the
prover).

## C. Remaining prover frontier (after W4)

`pf_exp` closed. What remains is all "needs a theory, not a gate":

| declaration | file | theory needed |
|---|---|---|
| `ip_v4` | std/net/socket.bv | disjoint-bit OR reasoning |
| `sha1_padded_len` | std/net/sha1.bv | modular arithmetic |
| `is_alpha_loop` / `is_numeric_loop` | std/string.bv | length algebra (`i == s.^Length`) |
| `read32le` | std/ffi/xxhash.bv | un-triaged (newly visible via origin split) |

`read32le` is the one I'd triage first: it was invisible until the origin split
named it, so it has never had a real "is this a gate fix or a theory" pass — unlike
the others, which are all measured theory gaps.

## Recommendation

Do **A** (issue #132) first — it is a real user-facing defect with a known root
cause and a bounded fix, and it closes the loop on both my PR #131 stopgap and the
fixture-layer hand-edit. Then **B** as a quick doc-sync (same or separate PR).
**C** is the larger design swing to take when a bounded-session is not the goal.
