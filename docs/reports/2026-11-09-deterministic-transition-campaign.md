# Deterministic transition campaign — four confirmed defects

## Environment and method

- Compiler revision: `114b2a5494d6ddf75248eb8654b76851be0422ad`, branch
  `integration/journey-main`; only test/documentation/CI changes in this session.
- Native target: x86_64 Linux; Ubuntu clang 21.1.8.
- Harness: `tests/deterministic_transitions.rs`, Proptest, seed 42.
- Thirteen transition families, eight requested cases per family. Nine families
  completed all eight cases: **72 passing generated program/event cases**, each
  compared against native code linked separately at O0 and O3. Four families
  stopped on failures and underwent bounded automatic shrinking. Do not count
  their unexecuted cases as tested.
- A second seed (2026) ran four more cases in each of the nine passing families:
  **36 additional passes, 108 passing generated cases total**, plus the four
  failure/shrink campaigns and independent confirmations.
- Events: 1–16 per case, values -20..20; initial state -100..100;
  bias -10..10; threshold -50..50. These bounds avoid signed overflow.
- Synthetic fault injection was explicitly disabled for all discovery and
  confirmation runs.
- Every finding was replayed from saved JSON with the **full typed frontend**
  before interpretation, excluding direct-parser adapter omissions. Native
  reproducers were separately rebuilt and run at both O0 and O3.

This is bounded transition testing, not exhaustive verification or reactor
scheduling coverage. The native adapter launches one process per event and
feeds its returned state into the next invocation.

## Results by family

| Family | Tested behavior | Seed-42 result |
|---|---|---|
| 0 | Guarded arithmetic | 8/8 pass |
| 1 | Signed division by positive constant | 8/8 pass |
| 2 | Signed remainder by positive constant | 8/8 pass |
| 3 | Nested guarded early returns | 8/8 pass |
| 4 | Guarded local mutation | 8/8 pass |
| 5 | Short-circuit protection of division | **Native SIGFPE** |
| 6 | Runtime list indexing | 8/8 pass |
| 7 | Tuple projection | **Interpreter error** |
| 8 | List element mutation | 8/8 pass |
| 9 | Mutation through second list binding | **Silent interpreter/native disagreement** |
| 10 | List returned by helper | 8/8 pass |
| 11 | Tuple destructuring | **Interpreter error** |
| 12 | Match-arm guarded return/fallthrough | 8/8 pass |

## 1. Short-circuit arithmetic speculated before the guard

Minimal transition:

```briev
defn step(s: Int, e: Int) -> Int {
    when e != 0 && s / e > 0 { term s; };
    term s - e;
};
```

Input `s=0, e=0`: interpreter returns **0**; both O0 and O3 native binaries
terminate with **SIGFPE (signal 8)**. Also reproduced at `s=7, e=0`.
Nonzero control inputs run successfully (`7,2` → `7`, `7,-2` → `9`).

**Root cause:** `src/backend/llvm/logical.rs:81` selects eager register-level
logical evaluation when `expr_is_side_effect_free` approves the RHS. At
lines 168–169 that predicate treats *all* binary operators recursively as pure,
including potentially trapping division. Side-effect freedom does not establish
that speculation is safe. The emitted `step` contains this ordering:

```llvm
%t4 = icmp ne i64 %arg1, 0
%t6 = sdiv i64 %arg0, %arg1
; ... compare division result ...
%t13 = and i1 %t11, %t12
br i1 %t16, label %guard.then15, label %guard.end15
```

The division precedes any conditional branch. The failure is already present
in Briev's generated IR, not introduced solely by clang optimization.

**Priority:** first fix. It turns valid guarded code into a process crash.
A principled repair needs a speculation-safety proof (including trap/poison
conditions), not merely a side-effect check or a special case for this example.

Replay: `tests/fixtures/simulation/short_circuit_division.json`.

## 2. List binding/mutation changes observable semantics

```briev
defn step(s: Int, e: Int) -> Int {
    let values: List<Int> = [s, e];
    let alias: List<Int> = values;
    alias[0] = e;
    term values[0] + alias[0];
};
```

Input `s=0, e=1`: interpreter returns **1**; native returns **2** at both
O0 and O3. Native `s=7, e=-2` returns `-4` (both reads see the mutation).

**Mechanism:** interpreter products have value semantics. Identifier evaluation
clones the value (`src/interpreter/eval.rs:52`); indexed mutation reads, modifies,
and reinserts the bound product (`eval.rs:2569` explicitly documents this).
Native emission shares the list storage between the two bindings. The mismatch
also persists when the interpreter receives fully typed/normalized input.

This is a confirmed language-execution parity defect. Under the project's
interpreter-is-reference rule, native execution must preserve the independent
value behavior; the fix must reconcile ownership/copy semantics generally.
This report does not claim to have traced every backend ownership decision or
established an allocation/lifetime corruption: the observed bug is wrong output.

Replay: `tests/fixtures/simulation/list_alias_mutation.json`.

## 3. Interpreter rejects numeric tuple projection

```briev
defn step(s: Int, e: Int) -> Int {
    let pair = (s + e, s - e);
    when e > 0 { term pair.0; };
    term pair.1;
};
```

Input `0,0`: interpreter errors `expected a field named '1', got product`.
Native code returns **0** at both optimization levels. Additional native checks:
`0,1` → `1`; `7,-2` → `9`.

**Root cause:** `src/interpreter/eval.rs:1056` handles product field access only
by searching optional field *names*. An unnamed tuple has positional fields,
so `.1` cannot resolve. Existing native tuple tests establish that numeric
projection is supported syntax. The full typed frontend does not remove the
reference failure.

Replay: `tests/fixtures/simulation/tuple_projection.json`.
An explicitly ignored, expected-to-fail reference regression is also available:
`interpreter_tuple_projection_reproducer`.

## 4. Interpreter drops tuple destructuring bindings

```briev
defn step(s: Int, e: Int) -> Int {
    let pair = (s + e, s - e);
    let (left, right) = pair;
    term left - right;
};
```

Input `0,0`: interpreter errors `undefined variable 'right'`.
Native code returns **0** at O0/O3; native `0,1` → `2`, `7,-2` → `-4`.

**Root cause:** `Statement::Let` carries both `name` and `names`
(`src/ast/top.rs:386`). The interpreter's function-statement evaluator at
`src/interpreter/eval.rs:2556` matches `name, ty, expr, ..` and inserts only
`name`, ignoring destructured `names`. This is a separate missing operation
from numeric projection; repairing projection alone will not bind `right`.

Replay: `tests/fixtures/simulation/tuple_destructuring.json`.
Explicitly ignored reference regression: `interpreter_tuple_destructuring_reproducer`.

## Reproduce a finding

These commands **intentionally fail while the reported bugs remain open**:

```bash
BRIEV_SIM_TYPED_REFERENCE=1 \
BRIEV_SIM_REPLAY=tests/fixtures/simulation/short_circuit_division.json \
BRIEV_SIM_ARTIFACTS=target/simulation-short-circuit \
cargo test --test deterministic_transitions scripted_integer_transitions -- --nocapture
```

Replace the JSON name to select another finding. The artifact contains source,
failure text, metadata, and IR if compilation was reached. Reference failures
occur before native compilation; independent native confirmation was performed
separately for both tuple defects.

Local campaign logs: `/tmp/opencode/transition-campaign/`.
Fresh typed-reference/native confirmations: `/tmp/opencode/transition-confirmation/`.
These temporary directories are supplemental evidence; durable minimal inputs
and the observed results are recorded above and in the fixture directory.

## Recommended next work

1. Fix eager logical speculation; extend the check to remainder, unsafe reads,
   and other potentially trapping/poison-producing operations.
2. Resolve list-copy/alias behavior consistently using the reference semantics.
3. Complete interpreter tuple projection and destructuring independently.
4. Promote the saved cases into mandatory native/reference regression tests
   once fixed, then broaden the generator to aliases across calls and reactive
   event scheduling.
