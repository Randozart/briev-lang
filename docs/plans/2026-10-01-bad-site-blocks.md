# `.bad` site blocks — anonymous target dispatch; attached exceptions retired

**2026-10-01.** Status: **active, this commit.** Design session decision
(same day): the `=>` attachment syntax (`target => rows` after an
instruction) reads terribly — overrides hang *backwards* on an anchor
line that gives no visual signal, multiple `ptx =>` rows silently
attach to the same anchor and all but the first are dead (`find()`
takes the first). The gating semantics were correct (replace-on-match,
core row otherwise, `default =>` inline = loud category error); the
SYNTAX was the defect. Replacement: an anonymous, inline **branch site**
— the existing branch-defn switch (`BadDefnShape::Branch`: one row per
target, `default =>` fallback, exactly one row lowers) surfaced without
the defn ceremony.

Companions: `2026-10-01-bad-ptx-family.md` (this unblocks the M4
device gate — the fixture's f32 load/store pair needed an insertion
form, not a replacement form), `bad-dialect.md` (grammar table, same
commit), `2026-09-21-bad-ergonomics-bv-integration.md` (historical:
branch defns, positional contracts).

## 1. Pinned grammar

```
site
    <bare lines>        ── default row (only when they precede any header;
                           `default =>` is the explicit spelling)
x86_64 =>
    <lines>             ── target row (raw emission, register-token substitution)
ptx =>
    <lines>
end site

site ptx =>             ── head form: the `raw <target>` head convention —
    <lines>                the head line opens the first row (design-session
end site                   addition, user request 2026-10-01)
```

- Keyword `site` — the compiler's own diagnostic vocabulary already says
  "gate the use site with an exception" (`lower.rs` step-3 error); the
  keyword names what the docs already name.
- Close: **`end site`** — named closes for error precision (a stray or
  mismatched close fails immediately, naming the construct; bare `end`
  orphans the rest of the body into a far-away "no owner" error). For
  uniformity, `raw <target> … end` migrates to `end raw` — one
  convention, full words, no abbreviations (no `end s`/`end r`: the
  close is a disambiguator, not a payload; two spellings double the
  grammar/docs/tests for 3 characters).
- Rows dispatch by header in any order; leading bare lines = default
  row; `default =>` = explicit spelling; duplicate target rows = loud
  error; nested `site` = error; `raw` blocks stay top-level (v1: not
  body items).
- **Exactly one row lowers**: matching target row, else default; else
  loud capability error naming the family AND the available targets.
- Default row re-enters the core pipeline (contracts, register
  existence, ISA rows — validated like ordinary code); target rows are
  raw (target-owned text, operands resolved through env/register table,
  the per-line `;` suffix rule applies).
- **No default row = no fallback, loudly** — the use-site capability
  doctrine: a ptx-only f32 load/store pair has NO portable lowering, so
  a default row would be a lie; omission + loud error on unmatched
  families is the honest form. (`_ => _` spellings rejected — omission
  already says it.)
- Legal in label bodies and defn **sequence** bodies. Not in branch-defn
  rows (those are raw text). Sites do not nest (v1).
- **Attached inline exceptions are REMOVED**: `target => ` after an
  instruction is a parse error with a migration hint ("moved to site");
  `BadInstr.exceptions`, lower step 2, and the inline `default =>`
  category-error check are deleted. Migration surface: doc examples,
  dialect tests, `examples/gpu/bad_override.abv`.

**Status: DONE 2026-10-01** — all milestones in one commit; the bridge
device gate (`benchmarks/bad_ptx_gate.sh`) passed both lanes with exact
equality the same day. Two extra discoveries landed with it: the M3
geometry-directive EMIT-side leak (§2 below) and the deeper M3 bug the
gate then exposed — PTX virtual registers were never DECLARED (ptxas
allocates physical registers only for declared virtuals). Fixed
data-driven via the `declare` row (bad-registers.dbvl, `%rd<16>` bank
form) + reserved temps `%p1`/`%rt1`/`%fs1` (f32).

## 2. The bug this session also fixes

M3 leak (found by the bridge device work): `.blockthreads`/
`.sharedbytes` are consumed in pass 1 (`collect_geometry`) but
`emit_directive` has no arm — the default arm pushes them **verbatim
into the PTX**, ptxas rejects the unknown directive, `compile_cubin`
returns None, the bridge silently falls back to PTX text. Fix: emit-side
skip arm + a regression test asserting the unit compiles to an ELF
cubin (not text) on a machine with ptxas.

## 3. Milestones (this commit unless noted)

- **S1** — AST: `BadSite` (reusing `BadBranch` rows),
  `BadBodyItem::Site`; `BadInstr.exceptions` removed.
- **S2** — parser: `site`/`end site` in bodies (leading-bare default,
  `default =>` alias, dup-row + nesting errors, `end raw` for raw
  blocks, attached-exception removal with hint).
- **S3** — lowerer: site dispatch (reuse the branch row-pick path),
  delete step 2 + category error, geometry emit-skip.
- **S4** — tests: site forms (default+override, ptx-only, dup, mismatch,
  in-defn-body), migrated exception tests, geometry-leak regression.
- **S5** — docs + examples: bad-dialect.md grammar table, site section,
  `end raw`; grep-sweep every `.bad`/`.bv`/doc example for attached
  exceptions; rewrite `examples/gpu/bad_override.abv` with a ptx-only
  site; verify the bridge produces an ELF cubin again.
- **M4 resume** (next commit): device gate for the bridge
  (softmax_gate-style injection, both lanes), then the M4/M5 close-out.

## 4. Gates

Suite green; warnings 19; gemm_h byte-identity (the dialect is not on
the default .abv path); Praetor no new rows. Docs same commit.
