# Typed Variadic `defn`/`txn` (2026-10-13)

## Context

`SysCall#` is variadic at the intrinsic level (`variadic: true`,
src/intrinsic_signatures.rs:326; the emitter pads any argument count to 7,
src/backend/llvm/intrinsics.rs:1699), and the language already parses a final
`...rest` parameter (`Parser.pending_variadic`, consumed by `$defn`/`$txn`
composites since 3b2828f4, 2026-09-23). Runtime `defn`/`txn` call
`reject_runtime_variadic` (src/parser/definitions.rs:2523) — an intrinsic can
do something the language cannot express. That is the expressiveness-closure
gap (docs/architecture/briev-capability-frontier.md): the language's optimum
must be expressible in the language.

This plan closes that gap: a TYPED rest parameter on runtime `defn` and
`txn`, specialized per call-site arity at compile time, bounded.

## Design decisions

**D1 — Static-arity specialization (Zig-style), not boxed rest.**
Each distinct call-site arity instantiates one monomorphized copy of the
callable. Zero runtime cost, exact element types, no allocation. Rule 2
(maximum efficient default): the default IS the fastest shape. Rejected
alternative: lowering the rest to a `List<T>` parameter (Swift/TS style) —
costs an allocation + boxing per call, overlaps with just declaring a
`List<T>` param, and degrades the default.

**D2 — Runtime-determined arity stays `List`.** A variadic is sugar over
STATIC arities only. No runtime splice (`f(a, ...list)`) in v1. The
over-bound diagnostic points at `List<T>` as the tool for dynamic counts.

**D3 — Typed only.** `...args: Int` — homogeneous element type.
Heterogeneous rest (`...calls: Expr`) stays the `$defn` compile-time
territory (SPEC §18.2); the two share the `...` syntax but live in separate
declaration kinds.

**D4 — Bound.** Rest element count is bounded:
- `...args: Int[<=6]` — explicit bound, 0 ≤ count ≤ 6.
- No bound → default cap 16 + a warning (warn-the-default / error-when-needed,
  keyword-taxonomy.md soundness rule).
- `...args: Int[>=1]` — minimum only (cap = default 16).
- `...args: Int[>=1, <=6]` — both.
Syntax rides `[]` = containment/bound (delimiter contract, SPEC §5.1,
Rule 21). Parser reads it directly in `parse_parameter_list` (NOT through
`parse_type` — `Type::Vector` is a fixed-size array and a rest is a COUNT
bound on arguments, not an array value; conflating the two would be Rule 3
obfuscation).

**D5 — In-body shape.** The rest name is bound in the body as a
compile-time-constant list of the element type: `args[0]`, `args[1]`, ...,
`args.Count#()` — all static per instantiation. The body is cloned per
specialization with the list literal substituted, so no new IR constructs.
Element access lowers through the existing static List indexing path.

**D6 — Contracts.** v1: contracts may reference the rest name as the
compile-time list (indexing/Count# inside contracts, same static
substitution as the body). `forall`-style quantification over the rest is a
follow-up, not a blocker — every v1 contract evaluates against the
instantiation's literal list, which is what the proof engine already
evaluates.

**D7 — Name hygiene.** Instantiated copies are internal (like generic
instantiations): no `name@N` tokens leak into source position. The
instantiation name is `name::arity` (internal Rust-side de-dup key — `::`
never appears in an authored identifier) and the emitted LLVM symbol is
`name__arity` (`::` is invalid in an LLVM identifier; `__` matches the
`__spawn_next_`/`__view_items_` internal-symbol convention). The original
declaration is retained for diagnostics; errors inside a specialized body
carry the directed messages of the pass (bounds, index, rest-value).

**D8 — `txn` included in v1.** Same specialization machinery; the reactor
wake-set / transition-graph analysis runs per specialization (each is a
normal `txn` after lowering). If the reactor analysis trips on
per-specialization wake sets, `txn` is demoted to v1.1 — `defn` ships alone —
and the plan is amended, not patched around.

**D9 — Arity check.** Call-site argument count < min or > bound → compile
error naming the callable, the declared bound, and the received count,
directing to a `List<T>` param for runtime-determined counts.

**D10 — Interpreter parity (Rule 5).** The interpreter evaluates the
specialized body the same way the backend does: arity-checked call, rest
bound to the argument slice, body runs. No interpreter-only fallback path.

## Implementation

1. **Parser** (src/parser/definitions.rs):
   - `parse_parameter_list`: after reading `...name: Type`, consume an
     optional rest-bound suffix `[<=N]` / `[>=M]` / `[>=M, <=N]` (direct
     token reads, not `parse_type`).
   - `pending_variadic` widened from `Option<String>` to
     `Option<VariadicParam { name, element_type, min: usize, max: Option<usize> }>`.
     All existing `$defn`/`$txn` consumers updated (they read the name/type
     fields; the bound is ignored there — composite rest has no bound).
   - `reject_runtime_variadic` deleted from the `defn`/`txn` paths; kept for
     `isr` and `op member` (those stay compile-time-only in v1, error text
     unchanged).
   - Rest bound validation: `max >= min`, `max` ≤ 64 (hard ceiling; beyond
     that the diagnostic directs to `List<T>`), `min ≤ 64`.
2. **AST** (src/ast/top.rs): `Definition.variadic_param` becomes
   `Option<VariadicParam>`; `Transaction` gains `variadic_param` (it is
   parsed by the shared `parse_parameter_list` and currently dropped —
   check parse_transaction).
3. **Specialization pass** (new `src/analysis/variadic_specialize.rs`,
   registered in the analysis pipeline BEFORE typecheck, mirroring how
   composites expand before typecheck — SPEC §18.2 rule):
   - Walk top-level `Call` sites (and any expression position) that resolve to
     a variadic `defn`/`txn`; check arity against min/max; clone the
     declaration per distinct arity with the rest list literal substituted
     (params + N element params `__va_0..N-1` of the element type; body
     rewrites `rest[i]` → `__va_i`, `rest.Count#()` → `N`; contract
     likewise).
   - Instantiations are registered as ordinary definitions/transactions in
     the item list (internal, not exported), provenance-carrying.
   - The original declaration stays for diagnostics and name resolution at
     other call sites.
   - Deterministic ordering: process call sites in source order; the
     arity→instantiation map is keyed by arity (sorted on emit, HashMap
     iteration discipline).
4. **Typecheck/codegen/interpreter:** untouched — they see ordinary
   fixed-arity definitions after the pass (additive-only, Rule 6: no match
   arms changed in the backend).
5. **Syscall surface cleanup** (follow-on, same increment): replace the
   `syscall0`–`syscall6` wrapper families with direct `SysCall#` calls in
   `lib/std/posix/io.bv`, `lib/std/posix/process.bv`, `lib/std/os/fs.bv`
   (option B from the 2026-10-13 discussion — the emitter already
   specializes; the wrappers are boilerplate on top of an intrinsic that IS
   the API). `lib/std/syscall.bv` and the `export defn syscallN` blocks in
   `lib/std/posix/syscall.bv` are removed; the `SYS_*` consts stay.
   NOTE: this cleanup is INDEPENDENT of the variadic work — even with a
   variadic `defn syscall(...)`, the syscall case wants direct `SysCall#`
   calls (splicing a rest into an intrinsic call is v1.1 at the earliest).
   The variadic feature ships with its own test surface; the syscall
   cleanup ships as a separate commit so each gate is attributable.

## Tests

- Unit (parser): rest parses with no bound / `<=N` / `>=M` / `>=M, <=N`;
  malformed bounds error; isr/op-member still reject `...` with the exact
  old message (behavioral, not literal).
- Unit (specialization): one declaration, call sites at arities 0/1/3 →
  three instantiations; arity over bound → the directed error (count + bound
  + List pointer); two call sites at the same arity share one
  instantiation.
- Unit (interpreter): a variadic defn `sum(...xs: Int[<=4]) -> Int` runs
  through the interpreter with each arity; a variadic `txn` with a
  postcondition on the rest (`[xs.Count#() <= 4]`) converges.
- Behavioral E2E (compiled, golden stdout): the `sum` fixture above,
  arities 1 and 3, plus the over-bound error case (compile-time, asserted on
  the error text).
- Syscall cleanup (follow-on commit): existing fs_smoke + posix I/O tests
  stay green; a new unit test asserts the `SysCall#` direct-call path emits
  the same IR as the old `syscall3` wrapper (IR-equivalence, same shape).
- Existing gates: `cargo test --lib` (3059 baseline at 74ba6742),
  conformance sweep, Praetor no-new-diagnostics on changed dirs,
  `cargo build` no new warnings.

## Documentation

- SPEC.md §9.1: add the **typed variadic** paragraph after the 2026-08-14
  generic-`defn` block — syntax, D1–D10 in condensed form, the `List<T>`
  boundary (D2), the bound syntax (D4), and the rule that heterogeneous
  rest stays §18.2 (`$defn`) territory. Update the §18.2 "Compile-time-only:
  a runtime `defn` declaring `...` is an error" line — it is no longer
  true; the two rest kinds are distinguished by the declaration kind and
  the element type (`Expr` vs a value type).
- SPEC.md §9.3 (Transactions): one line — txn takes the same rest rule.
- docs/architecture/agent-reference.md: the defn/txn declaration table gains
  the rest row.
- MASTER-SYNTAX-REFERENCE.md: the `...` rest row (runtime + composite).
- This plan's status footer flips to DONE when the gates pass.

## Retirement gate

The syscall-wrapper retirement (D-item 5) is the first consumer: if the
variadic machinery proves itself on the `sum`-style fixture, the `syscallN`
surface is deleted in the follow-on commit. If per-arity specialization
proves too heavy for the reactor (D8 fallback), the feature is scoped to
`defn` and this plan is AMENDED (not silently narrowed) with the new scope
and a follow-up plan for `txn`.

## Scope exclusions (v1.1 candidates)

- Splice-apply: `SysCall#(nr, ...args)` inside a variadic body.
- `isr` / `op member` rest parameters.
- `forall` contract quantification over the rest.
- Heterogeneous runtime rest (`...args` untyped).
- Runtime splice from a `List`.

## Implementation log (2026-10-14)

### Landing state

- Pass written: `src/analysis/variadic_specialize.rs` (static-arity
  monomorphization, D1), wired in `src/compile.rs` + `src/pipeline.rs`
  after `expand_composites`, before `check_types`. Parser + AST wired
  (`VariadicParam { name, element_type, min, max }` on `Definition` +
  `Transaction`).
- IR verified: single-arity `sum(1,2,3)` → `@sum__3` (body
  `arg0+arg1+arg2`); two-arity `sum(10,20)` + `sum(1,2,3)` → `@sum__2` +
  `@sum__3` with distinct bodies and correct call sites in `txn_main`;
  original `@sum` emitted body-less; `--keep-all-defns` correct.

### Bugs found and fixed

1. **Unconditional body-wipe**: `specialize_variadic` emptied the body of
   EVERY `Definition`/`Transaction`, including `node main`. Fixed: guard
   on `variadic_param.is_some()`.
2. **Ghost rest param**: the parser pushes the rest param into
   `parameters` as the LAST entry (parser/definitions.rs). `fixed_count`
   now subtracts it, and the instantiation `pop()`s the ghost before
   appending `__va_0..N-1`.
3. **Invalid LLVM identifiers**: internal key `name::arity`, emitted
   symbol `name__arity` (`::` and `@` are invalid in LLVM IR; `__`
   matches the `__spawn_next_` convention). `is_instantiation_name`
   guards reprocessing.
4. **Out-of-bounds constant index**: `xs[2]` at arity 2 fell through the
   in-bounds rewrite and left a dangling `Index(xs, 2)` → typecheck error
   `undefined variable 'xs'`. Fixed in `rewrite_expr_elems`: an
   out-of-bounds constant index is a DIRECTED ERROR ("the body indexes
   'xs[2]' but this call supplies only 2 rest argument(s)"). An earlier
   draft replaced the index with a `0` placeholder — REJECTED: a
   placeholder computes silently wrong results; the error names the fix.
5. **Dead recursion (return-match)**: converting the elems rewriter to
   `Result` initially turned the special-forms match into
   `return match … ;`, which made the child-recursion match dead code —
   no rest use was rewritten at all (`undefined variable 'xs'` for every
   probe). Fixed: first match handles special forms via `return` and falls
   through (`_ => {}`); recursion runs after; the fn ends with `Ok(())`.
6. **Foreach unroll renamed the iterable**: the `foreach x in xs` arm
   cloned `list` (the iterable `xs`) as the rename source — the loop
   variable `x` survived (`undefined variable 'x'`) and the unrolled copies
   would have corrupted `xs` references. Fixed: rename the `item: String`
   loop variable; the iterable is dropped with the statement (the unrolled
   `Block` replaces the `Foreach`).
7. **False "never called or fired" warning**: a variadic original txn is
   called THROUGH its instantiations (all sites retarget to `name__N`), so
   `unfired_txn_warnings` saw an unreachable original. Fixed:
   `live_instantiation_exists` (defn_liveness.rs) skips the warning when a
   live `name__N` exists — naming convention via
   `variadic_specialize::is_instantiation_name` (single source of truth).

The elems rewriters (`rewrite_stmt_elems`/`rewrite_expr_elems`) now return
`Result<(), String>` — the three unsupported shapes (bare rest, non-constant
rest index, out-of-bounds constant index) are compile ERRORS with directed
fix messages, never compiler panics and never silent miscompiles.

### Test-fixture fix (this session's gate)

Adding `Transaction.variadic_param` broke **136 exhaustive
`Transaction { … }` struct literals** in `#[cfg(test)]` modules (E0063,
missing field). Verified facts: all in `src/` (0 in tests/benches/examples),
all multi-line, none use `..` spread; lib builds clean — test-target only.
HEAD baseline: `cargo test --lib --no-run` = 0 errors, so this change is
the sole cause.

**Fix**: insert `variadic_param: None,` at all 136 sites (script-assisted:
insert after the `Transaction {` opening line, indent-matched, descending
line order per file; iterate `cargo test --lib --no-run` to zero).

**Rejected**: `#[derive(Default)]` + `..Default::default()` spread
(per-site edits anyway; `Contract`/`DerivationBlock` not `Default`);
`Transaction::new()` builder (136 call-site edits + new API surface).
`None` is the semantically correct value — these fixtures are
non-variadic txns, same as their existing `span: None, doc: None`.

**Zero production-code edits**: all sites are `#[cfg(test)]` literals; the
pass, parser, and wiring are untouched. Post-fix verification: all 6
probes (`is1`–`is3`, `p3`, `base2`, `probe`) re-emitted and diffed.

### Session close (2026-10-14)

- **Walker split (Praetor)**: the three AST walkers were single large
  two-match fns (special forms + child recursion). Split uniformly:
  `rewrite_expr_calls` → `_form` + `_children`; `rewrite_expr_elems` →
  `_form` + `_children`; `rename_expr` → inline special form +
  `rename_expr_children`. Remaining Praetor diagnostics are the
  AST-enum-size floor (cyclo 16 vs max 15 on ~20-variant child dispatchers;
  the foreach-unroll O(n²) is inherent to unrolling) — the same shape the
  existing analysis visitors carry as project baseline.
- **Verified E2E (IR-level)**: `ma.bv` multi-arity (`sum__2` + `sum__3`,
  both call sites in `txn_main` retargeted); `fe.bv` foreach unroll +
  `Count#()` fold (`total__3` = `(0 + a0 + a1 + a2) + 3`); `txn1.bv`
  variadic txn (`score__2`/`score__3` with `%state`, postcondition
  `term >= xs[0]` → `__va_0`); `under.bv`/`over.bv` directed bounds errors;
  `is2.bv`/`probe.bv` directed out-of-bounds index errors.
- **Gates**: `cargo test --lib` 3069 passed / 2 failed — both failures are
  the Increment-3 (fs.bv prelude + conformance collisions) workstream,
  present before this work and fixed in that stream. 12 new unit tests.
  `cargo build` no new warnings in changed files (the two `main.rs`
  unused-variable warnings are pre-existing). Probes re-emitted after
  every change.
- **Unit tests** (`#[cfg(test)]` in variadic_specialize.rs): naming split,
  instantiation-name guard, single-arity rewrite (body + call site +
  wiped original), same-arity dedup, two-arity split, below-min error,
  over-max error (+ `List<T>` hint), OOB index error (placeholder
  regression guard), non-variadic body preserved (Bug-1 guard),
  `Count#()` fold, foreach unroll (Bug-6 guard), bare-rest error.
