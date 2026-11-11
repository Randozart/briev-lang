# Unit test blocks (`test domain { expect …; };`)

SPEC §8.11. Tests are source-level declarations, typechecked like the rest of
the program but never lowered or emitted.

```briev
defn add(a: Int, b: Int) -> Int { term a + b; };

test arithmetic {
    expect add(1, 2) == 3;
    expect add(-1, 1) == 0;
};
```

## Rules

- `test` is a top-level declaration; `domain` is a bare identifier label (one
  domain per block). Blocks run in source order.
- `expect <bool>;` asserts; a false value fails the test and reports the
  expression. `expect` must typecheck as `Bool`.
- Reactive/terminating forms are rejected inside a test (`term`, `tail`,
  `endprogram`, `reside`, `node`/`txn` declarations): call a defn instead.
- Each test runs in a **fresh reference interpreter** with fresh `init`
  seeding; program globals/imports are shared read-only. State changes never
  leak between tests.
- Test blocks do not affect liveness, state layout, or emitted IR. Compiling a
  program with and without a test block emits byte-identical `.ll`.

## Running

```bash
brievc test path/to/file.bv --tests-only          # only test blocks
brievc test path/to/file.bv --list-tests          # labels, no execution
brievc test path/to/file.bv --domain arithmetic   # exact domain filter
brievc test tests/ --tests-only                   # a directory
```

Output:

```
TEST arithmetic#1 PASS
TEST arithmetic#2 FAIL — expectation failed: `expect (add(1, 2) == 4)` evaluated to false
FAIL math — arithmetic#2: ...
1/2 tests passed; 0/1 files passed
```

Exit code is nonzero when any test fails. A `--domain` that matches nothing is
an error listing the available domains.

Without `--tests-only`, `brievc test` also runs the file as a program (exit
code + `.golden`/comment directives), preserving the pre-test-block behavior.

## Not in v1

Backend differential execution (compiled vs interpreter), `expect_trap`,
`expect_exit`, `expect_stdout`, float `approx` expectations, explicit test
names, and multi-domain tags. `:=` remains compile-time derivation
(SPEC §18), never an expectation operator.
