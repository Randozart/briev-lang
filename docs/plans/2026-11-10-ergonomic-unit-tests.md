# Ergonomic unit tests: `test domain { … };` (2026-11-10)

Status: ACTIVE. Surface is normative in `spec/SPEC.md` §8.11 (written with
this plan). Implementation phases below.

## Decision

```briev
test arithmetic {
    expect add(1, 2) == 3;
    expect gcd(84, 36) == 12;
};

test collections {
    let xs: List<Int> = [3, 1, 2];
    bubble(xs);
    expect xs[0] == 1 && xs[2] == 3;
};
```

Grammar:

```
test_decl   ::= "test" identifier "{" statement* "}" ";"
expect_stmt ::= "expect" expression ";"
```

### Why `test domain {}` (not `test(domain)` / `test<domain>`)

Rule 21 (one delimiter, one load) and declaration precedent:

- `()` = application/binding. A test block has no runtime parameter, so
  `test(domain)` would read as an invocable with an argument.
- `<>` = compile-time type specialization/groups (`Stack<T>`, `sync<group>`,
  `asm<chip>`). A domain is a label, not a specialization.
- A bare declaration head is the language's shape for named declarations:
  `obj Item { … };`, `node main [pre][post] { … };`, `cell cpu { … }`.
  `test domain { … };` follows it; `{}` keeps its definition/grouping load.

`:=` keeps its only meaning (compile-time derivation / reference
implementation, SPEC §18); expectations use `expect <bool>;` so no operator is
overloaded.

## Semantics (v1)

- `test` is a hard keyword; `domain` is a bare identifier (declaration label,
  not a value). One domain per block; domains group and filter tests.
- Bodies are ordinary statements (`let`, `when`, `foreach`, `match`, calls)
  plus `expect`. Allowed to call defns, read globals/state, use imports and
  the prelude.
- **Reactive constructs are rejected inside a test**: `node`, `txn`,
  `endprogram`, `reside`, `term`/`tail` (diagnostic names the fix).
- `expect expr;` requires `expr : Bool`. Failure reports domain, ordinal/line,
  the rendered expression, and the evaluated value.
- **Isolation**: each test gets a fresh interpreter and fresh `seed_inits`;
  program globals/imports are shared read-only. A failing test never affects
  the next one.
- **No codegen/liveness effect**: test blocks are typechecked but excluded
  from every analysis/lowering/emission stage (explicit skip arms, asserted by
  an IR-equality test: compiling with/without a test block yields the same
  `.ll`).
- Deterministic order: source order, domains filtered by exact name.

## Engine (v1 = reference interpreter)

`brievc test <file>`:

1. `pipeline::compile_to_typed` (prelude/imports/desugar/typecheck).
2. Split out `TopLevel::Test` blocks; seed a fresh interpreter from the
   non-test program (the `interpreter_callable_txn` recipe).
3. Execute each test body; `expect` evaluates to Bool; runtime errors fail the
   test with the error rendered.
4. Report per test and a summary; nonzero exit on failure.

Existing behavior is preserved: `brievc test` still runs the file as a program
(exit code + `.golden`/directive checks) unless `--tests-only`. Flags:
`--domain <name>` filters; `--list-tests` lists `domain#n`.

Deferred (separate slices): backend differential execution (compile each test
and compare with the interpreter, P3), `expect_trap { … }`,
`expect_exit(n)`, `expect_stdout("…")`, float `approx` expectations,
`test domain, other {}` multi-tagging, explicit test names.

## Phases

1. **Surface** — `Token::Test`, `Token::Expect`; `TopLevel::TestBlock
   { domain, body, span }`; `Statement::Expect(Expr)`; parser; Display/
   canonical/visitors/diff arms; vocab ledger + version; highlighter; SPEC
   (done in this commit).
2. **Typecheck** — non-reactive test scope, Bool `expect`, reject forbidden
   forms; explicit skip arms in HIR lowering and every analysis; liveness must
   ignore tests.
3. **Engine + CLI** — `src/test_engine.rs`, `brievc test` integration,
   `--domain`/`--list-tests`, isolation, reporting.
4. **Verification** — parser/typecheck negatives; engine pass/fail/isolation;
   CLI golden; IR-equality-with/without-tests gate; no-liveness-delta gate.
5. **Docs** — `learn-briev` testing guide, `docs/guides/`, INDEX entry.

## Gates

`scripts/gate.sh quick`; `cargo test --all`; IR-equality (`.ll` with vs
without the test block); determinism of report order; Praetor on changed
dirs; docs in the same commits.
