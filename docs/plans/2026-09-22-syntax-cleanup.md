# Syntax Cleanup — Enum `.` Construction, Barrier/Lemma Removal

**2026-09-22.** Decisions from the syntax-corpus review session. Several SPEC forms
are aspirational, dead, or half-built; the corpus is to document only what the
compiler actually implements. This plan removes the dead weight and makes enum
construction use the natural member-access spelling.

## Decisions

| # | Form | Decision | Evidence |
|---|------|----------|----------|
| A | `Color.RGB(...)` vs `Color::RGB(...)` | `.` is canonical; desugar `.Variant(...)` to the internal `Enum::Variant` call string; remove the `::` token | `src/parser/expressions.rs:466,495,1198` |
| B | `use` vs `import` | `import` is the keyword; SPEC `use` is prose-only, nothing to change | no `Token::Use` |
| C | `vol node` | Not implemented, no use case (volatility is variable-level memory semantics, node classification is scheduling). Document `vol let` only | `vol` only in `vol let` (`statements.rs:35`) |
| D | `machine` block | Never existed; SPEC "machine" is prose for machine-entry (`bootstrap node`). Nothing to delete | no `Token::Machine`, no AST node |
| E | `atomic` | Field-level stays (safe concurrent RMW on shared reactor state); node/let-level has no use case | field parser only (`definitions.rs:3151`), LLVM wired |
| F | `guard` keyword | Never existed; guards are *shapes* (`[cond] stmt;`, `[cond];`, `when { }`). SPEC §10.2 already documents shapes | no `Token::Guard` |
| G | `op` lemma `[commutative]` | **Remove** — dead field, no consumer, redundant with `-ffast-math` + `!> associative` + hardcoded builtin commutativity, contradicts axiom's FFI-boundary justification | `trusted_lemmas` zero readers; parse error on `[` |
| H | `barrier<group>` | **Remove** — contract-only no-op; body emits inline everywhere; group name never read; `[expr];` Gate is the real convergence point | every layer inlines the body |

## Rationale (the why)

### A. Enum `.` construction
`Color.RGB(255,128,0)` is the natural spelling and currently fails ("undefined
variable 'Color'" — the typechecker sees member access on an unbound variable).
The parser already desugars `Color::RGB(...)` → `Expr::Call("Color::RGB", ...)`;
the `.` form can desugar to the **identical** AST when the base is an identifier
in `known_types`. Zero downstream change: typechecker `variant_defs`, LLVM
`variant_ctor`, interpreter `registered_variants`, and pattern normalization all
key off the `"Color::RGB"` string. The internal `::` string remains a stable
contract; only display round-trips `::`→`.`. The `::` token itself is a Rust-ism
that should not exist in Briev.

Shadowing caveat (shared with today's `::`): a *variable* named `Color` with a
member `RGB` is mis-desugared because the parser cannot see bindings. Document
in SPEC; same limitation existed for `::`.

### G. Lemma removal — why redundant
1. Nothing reads `trusted_lemmas` (zero consumers in `src/`). It was never
   parsed (`op Add: f(#Lh,#Rh) [commutative];` is a parse error), never
   populated (all sites `vec![]`).
2. No Briev pass does algebraic rewriting (equality-saturation is a stub;
   LICM never reorders operands). The right a lemma would grant has no spender.
3. The float case is already covered by `-ffast-math` on the link line AND
   `!> associative` / `!> fp_math: fast` → LLVM `reassoc`/`fast` attrs
   (`config/meta-vocab.dbv:43`).
4. Builtin commutativity (int add/mul counter detection, deferral algebra) is
   already hardcoded in `transition_graph.rs`/`batch_shape.rs`/`accel.rs` — Rule
   23 forbids per-op vocabulary knowledge.
5. `axiom` on cast edges is justified because FFI impls have no Briev body to
   prove; an op lemma's handler `f` has a body — commutativity could be *proven*
   (SMT, integer) rather than trusted. Trusting it skips a provable proof.
6. float_math is the counter-example: Briev deliberately does NOT reassociate
   reductions to preserve symmetric output vs C (`batch_shape.rs:189`).

Keep: `axiom` on cast edges (`protocol_graph.rs:133`) — the authority mechanism
with a real FFI-boundary need.

### H. Barrier removal — why a no-op
Every layer executes the body inline: reactor (`reactor.rs:351`), interpreter
(`eval.rs:2141`), LLVM (`emit_stmt.rs:1640` "scheduling contract, not a
parallelization hint"). The `groups` field is parsed/cloned/printed but never
read for any decision. Unrelated to GPU (that's `Barrier#` → `OpControlBarrier`)
and to `sync<group>` (Rule 22 lives in `concurrency_gate.rs` on node
preconditions). Half-built from Phase 10 (2026-08-09, commit `60ea9b8f`): only
the "inline" path ever landed; the parallelization path and Kani invariants
were deferred and never delivered. The real convergence point is `[expr];`
Gate — a retry-to-loop-header branch.

## Implementation

### 1. Enum `.` desugar + `::` removal (Task 1)
- `src/parser/expressions.rs`:
  - `.` branch (`:495`): if `expr` is `Expr::Identifier(base)` and `base ∈
    self.known_types`, desugar `Color.RGB(args)` → `Expr::Call("Color::RGB",
    args)`; bare `Color.RGB` → `Expr::Call("Color::RGB", vec![], None)`.
  - Delete `::` expression branch (`:466-494`).
  - `parse_pattern` (`:1198`): mirror — `Color.RGB(subs)` → `Pattern::EnumVariant`.
  - Delete `::` pattern branch (`:1198-1215`).
- `src/lexer.rs`: delete `Token::ColonColon` (`:476`) + display (`:712`).
- Display (`src/ast/display.rs` / canonical): translate `::`→`.` when printing
  `Expr::Call`/`Pattern::EnumVariant` callee names.
- Tests: parser unit tests for `.` desugar (expr + pattern); remove/replace any
  `::`-based tests.

### 2. Migrate `::` call sites (Task 2)
- `examples/syntax/decl/enum.bv`: `Color::RGB(...)` → `Color.RGB(...)`.
- `probes/codegen/qualified_enum_circt.bv`: `Http::Ok`/`Http::Fail` → `Http.Ok`/`Http.Fail`.
- `lib/compiler/backends/x86_64.bv`: `X64Instr::Comment` → `X64Instr.Comment`.
- Mappers: `CString::new`/`JsValue::from_bytes`/`PyInt::from_int`/`PyFloat::from_float`
  → `.` form (foreign-fn qualification rides the same desugar).
- Docs: SPEC §8.3 qualified-variant-paths, `learn-briev/05` (327-339, 555),
  `08` (238-356), `09` (patterns), syntax highlighter `::` rule.

### 3. Remove `barrier` (Task 3)
- AST: `Statement::Barrier` (`src/ast/top.rs:408`), display arm (`ast/display.rs:445`).
- Parser: `parse_barrier_statement` (`statements.rs:362-381`), dispatch (`:91`),
  `Token::Barrier` (`lexer.rs:200`), vocab entry (`vocab.rs:189`).
- Consumers (strip arm): reactor `:351`, interpreter `eval.rs:2141`,
  typechecker `:2385,:3582`, annotator `:115,:512`, capabilities `:655`,
  dataflow `:242`, coll_length `:305,:453,:552`, task_segments `:197,:260`,
  defn_liveness `:339,:672`, LLVM emit_stmt `:1640`, mod `:708,:6673`,
  emit_toplevel `:73`, helpers `:310`, loop_engine/counter `:2121`,
  composite `:540,:548,:588,:732,:930,:969,:1110`, macros/eval `:2125`,
  macros/selection `:466`.
- SPEC §11.6 barrier bullet; delete `examples/syntax/stmt/barrier.bv`.

### 4. Remove `trusted_lemmas` (Task 4)
- AST: `OperatorDef.trusted_lemmas` (`ast/top.rs:1201`), `OperatorBinding.trusted_lemmas` (`:1226`).
- Init sites: `compile.rs:1154,1210`, `typechecker/mod.rs:785`,
  `parser/definitions.rs:2808,2882,3633`, `backend/llvm/mod.rs:3195`.
- SPEC §8.8 lemma paragraph; `config/axioms.dbv` `lemma_properties`
  vocabulary (`:16-26`); `config_tuning.rs` lemma_properties validation
  (`:290-302,631-684`).

### 5. Doc/spec corrections (Task 5)
- SPEC: `import` is the keyword (`use` prose only); §15.3 `vol` documents
  `vol let` only; add "no `machine` block" clarification (prose-only);
  §10.2 guards are shapes (already correct — no change).
- `docs/plans/2026-09-21-syntax-examples-corpus.md`: drop `barrier`/`machine`/
  `guard`/`vol node`/lemma forms; note enum `.` spelling.

### 6. Verify (Task 6)
- `cargo test --lib` green (only pre-existing GPU `accel_rt::self_test`
  failure expected — needs hardware).
- Conformance sweep (`conformance::tests::conformance_sweep...`) passes.
- Corpus examples all `brievc check` OK.

## Documentation maintenance
- SPEC.md §8.3, §8.8, §11.6, §15.3 updated in the same commits as the code.
- `learn-briev/` tutorial updated with `.` spelling.
- Syntax highlighter `::` rule removed.

## Undo
- Enum `.` desugar: revert `expressions.rs` to `::` branch; `ColonColon` token
  returns. (Not recommended — `.` is the agreed canonical spelling.)
- Barrier/lemma removal: resurrect `Statement::Barrier`/`trusted_lemmas` from
  git history (`60ea9b8f` for barrier; SPEC §8.8 for lemmas).