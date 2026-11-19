# Briev Basics

> **Runnable companion:** `learn-briev/examples/01-basics.bv` compiles, runs, and its output is pinned in CI (`tests/learn_briev_examples.rs`) — the machine-checked face of this chapter.


## 1. State Declarations

All state in Briev is declared globally with `let`:

```briev
let counter: Int = 0;
let name: String = "Alice";
let active: Bool = true;
let score: Float = 95.5;
```

**Type Inference:**
```briev
let x = 42;          // Inferred: Int
let s = "hello";     // Inferred: String
let b = true;        // Inferred: Bool
let f = 1.5e-8;      // Inferred: Float (exponent form, C-style: 1e5 works too)
```

**Without Initial Value:**
```briev
let counter: Int;    // Defaults to 0
let name: String;    // Defaults to ""
let active: Bool;    // Defaults to false
```

## 2. Constants

Immutable values with `const`:

```briev
const MAX_SIZE: Int = 100;
const VERSION: String = "1.0.0";
```

## 3. Reactive Nodes

State changes happen in `node`s. A reactive node fires whenever its precondition is met and runs until its goal (postcondition) is reached:

```briev
node increment [count < 100][count == 100] {
    count = count + 1;
    term;
};
```

**Parts of a node:**
- `node` — Keyword (reactive: no parameters, no return value)
- `increment` — Name
- `[count < 100]` — **Precondition** (when it can run)
- `[count == 100]` — **Goal** (termination condition)
- `count = count + 1` — Mutation (plain assignment, no `&`)
- `term;` — Terminate this firing

The reactor fires the node repeatedly until the goal is reached. The compiler proves the goal is reachable.

A callable `txn` may take parameters and return a value, but only runs when called (see chapter 03).

## 4. Scripting Mode (Top-Level Statements)

Top-level statements are wrapped into a one-shot node automatically:

```briev
let message: String = "Hello, Briev!";
println!(message);
```

This is equivalent to a node that runs once and exits.

**Rules:**
- All declarations must come **before** any executable statements
- Statements execute in order, exactly once, then the program exits
- `init` seeds a runtime value exactly once and is immutable after seeding

**When to use scripting vs explicit nodes:**
- **Scripting**: quick scripts, one-shot initialization
- **Explicit nodes**: programs with loops, state machines, reactive chains

## 5. When Guards (No if/else)

Briev uses `when` guards instead of `if/else` chains:

```briev
when x > 0 {
    result = x * 2;
};
when x < 0 {
    result = x * -1;
};
```

For branching over values, use exhaustive `match`:

```briev
match status {
    Ok(v) => Print#(v),
    Err(msg) => Print#(0 - 1),
};
```

The compiler enforces exhaustiveness — missing cases are errors.

## 6. Enums and Constructors

```briev
enum Result<T, E> {
    Ok(T),
    Err(E)
};

defn divide(a: Int, b: Int) -> Result<Int, String> {
    when b == 0 { term Err("division by zero"); };
    term Ok(a / b);
};

defn show(r: Result<Int, String>) -> String {
    term match r {
        Ok(v) => "ok",
        Err(_) => "error",
    };
};
```

Constructors (`Ok(v)`, `Err(e)`) type-check against the enum's declared params. Match arms are checked for exhaustiveness.

## 7. Function Values

Named functions can be passed as arguments to callable-typed parameters:

```briev
defn apply(f: (Int, Int) -> Bool, a: Int, b: Int) -> Bool {
    term f(a, b);
};

defn cmp(a: Int, b: Int) -> Bool { term a == b; };

// Pass cmp directly
let eq = apply(cmp, 5, 5);
```
