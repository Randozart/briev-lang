# Syntax Corpus Completion — close the implementable gaps

**2026-09-22.** Follows `2026-09-21-syntax-examples-corpus.md` (19 files
landed) and `2026-09-22-syntax-cleanup.md` (aspirational forms retired).
This plan closes the remaining IMPLEMENTED-but-unexemplified SPEC forms.
Zero compiler risk: every file is enforced by the conformance sweep
(`src/conformance.rs` `active_roots()` → `examples/`).

## Scope — forms to exemplify

Audited as implemented (parser/lexer accept them), not yet in the corpus:

| Form | SPEC § | Status in code |
|------|--------|----------------|
| `$defn` composites | §18 | implemented (comptime fold + generation, Phase 1+2) |
| `$(Stage)` blocks | §18 | implemented (`parse_stage_block`) |
| `.^^` reflection | §17 | implemented (compile-time reflect, `ReflectKind::CompileTime`) |
| `watchdog` txn | §10.3 | implemented (`within` + `-> handler`) |
| `atomic`/`vol`/`seq` field directives | §15.3 | field modifiers implemented |
| `vol let` | §15.3 | implemented |
| quotation / derivation | §18 | implemented |
| `import` selective/aliased | §7 | implemented |
| `render` block | §21 | implemented |
| `op` declarative (in `type`) | §8.8 | implemented (`op Add(Point<Float>): ...`) |

## Excluded (audited as NOT implemented — do not force)

`barrier` (removed), `machine` block (never existed), `guard` keyword,
`vol node`, `atomic` node/let, `use` keyword, op lemma `[commutative]`,
`Enum::Variant` (retired — `.` spelling is canonical).

## Method

For each form:
1. **Confirm the exact grammar from source** (`src/parser/*.rs`), not from
   SPEC prose — the SPEC has drifted before.
2. Write ONE standalone `.bv` under `examples/syntax/<section>/<feature>.bv`.
3. Verify with `brievc check` (debug binary, stdlib enabled).
4. If a form turns out to be broken (parse gap), STOP and report — do not
   paper over it with a degraded example. A broken form is a compiler bug
   (conformance-sweep doctrine), not a corpus omission.

## Files to land

- `examples/syntax/meta/defn-composite.bv` — `$defn` + `name!(args)` use.
- `examples/syntax/meta/stage-block.bv` — `$(Stage) { ... }` (if the first
  stage-block probe still parses; verify).
- `examples/syntax/decl/reflect.bv` — `.^^Size` / `.^^Element` compile-time.
- `examples/syntax/stmt/watchdog.bv` — `txn ... within 10ms -> handler`.
- `examples/syntax/decl/directives.bv` — `atomic`/`vol`/`seq` field modifiers.
- `examples/syntax/stmt/vol-let.bv` — `vol let` (observability pin).
- `examples/syntax/meta/quotation-derivation.bv` — `'` quotation + derivation.
- `examples/syntax/decl/import-forms.bv` — selective `{ name }` + alias `:`.
- `examples/syntax/render/block.bv` — `render Name { <html> }`.
- `examples/syntax/decl/op-declarative.bv` — `op Add(Point<Float>): handler`
  in a `type` body.

## Gates

- Every file `brievc check` OK.
- `cargo test --lib` green (conformance sweep enforces the corpus).
- No parser/compiler changes unless a probe exposes a genuine bug — then
  that becomes its own focused fix (plan amendment), never a corpus
  workaround.

## Documentation

- Update `docs/plans/2026-09-21-syntax-examples-corpus.md`'s "Remaining
  gaps" list after landing.
- No SPEC changes expected (these forms are already documented).