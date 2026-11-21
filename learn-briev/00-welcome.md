# Welcome to Briev! 🎉

**Briev** is a declarative, contract-enforced logic language designed for building verifiable state machines.

## What Makes Briev Different?

### 1. Contracts First
Every transaction declares what must be true **before** and **after** it runs:

```briev
txn withdraw(amount: Int) 
    [amount > 0 && balance >= amount]  // Precondition
    [balance == @balance - amount]      // Postcondition
{
    &balance = balance - amount;
    term;
};
```

The compiler **verifies** that your code actually satisfies these contracts.

### 1b. Three kinds of declaration: `node`, `txn`, and `defn`

Briev has two kinds of state-changing declarations plus a pure function form:

| | `node` | `txn` | `defn` |
|---|--------|-------|--------|
| **Runs** | Reactively — the main loop fires it whenever its precondition is met | Only when called by a `txn` or `node` | Only when called |
| **Parameters** | None | Optional | Optional |
| **Return value** | None | Optional | Required (it's a pure function) |
| **State mutation** | May read/write global state | May read/write global state | None outside itself — its result feeds back into state only through a caller's `&state = defn(...)` |
| **Contracts** | `[pre][post]` | `[pre][post]` | Optional (straight-line translation is inherently provable) |
| **Typical use** | The program's driver (`node tick`, `node process`) | Callable operations (`withdraw`, `push`) | Pure computation (`next(x)`, `score(p)`) |

```briev
node tick [count < total][count == total] {
    count = count + 1;
    term;
};

txn deposit(amount: Int) [amount > 0][balance == @balance + amount] {
    &balance = balance + amount;
    term;
};

defn next(x: Int) -> Int {
    term x + 1;
};
```

`node` and `txn` are **not** interchangeable: a reactive driver has no
parameters and no return value; a callable operation may have both — but a
`txn` never fires by itself, it only runs when a `txn` or `node` calls it.
A `defn` cannot mutate state outside itself; the only way its computation
reaches the outside world is through a caller assigning its return value.

### 2. Reactive by Default
`node` declarations fire automatically when their preconditions are met:

```briev
node auto_save [dirty && !saving][!dirty] {
    save_to_disk();
    &dirty = false;
    term;
};
```

No event handlers. No polling. Just logic.

### 3. Zero-Nesting Logic
No `if/else` chains. Use `when` guards instead:

```briev
// Instead of: if x > 0 { ... } else if x < 0 { ... }
when x > 0 {
    &result = x * 2;
};
when x < 0 {
    &result = x * -1;
};
```

### 4. Compile-Time Verification
The compiler proves:
- ✅ No race conditions
- ✅ No unintended side effects
- ✅ All contracts are satisfied
- ✅ No deadlocks in async code

## Quick Start

### Your First Program

Create `hello.bv`:

```briev
let message: String = "Hello, Briev!";

println(message);     // scripting — no transaction wrapper needed
```

Briev lets you write statements directly at global scope. The compiler
automatically wraps them in a synthesized `node __init` that fires
once on start. No boilerplate needed.

Run it:

```bash
brievc check hello.bv
```

For more complex programs, you can still write explicit transactions
(see [01-basics.md](01-basics.md)). Scripting mode is syntactic sugar
for simple programs — all the same safety guarantees apply.

### Learning Path

This folder contains a complete Briev tutorial:

1. **01-basics.md** - Variables, types, transactions
2. **02-contracts.md** - Preconditions, postconditions, @ prior state
3. **03-reactive.md** - Reactive transactions, auto-firing logic
4. **04-functions.md** - Functions with contracts
5. **05-data-types.md** - HashMap, HashSet, Stack, Queue
6. **06-string.md** - String manipulation, StringBuilder
7. **07-ffi.md** - Foreign function interface
8. **08-examples.md** - Complete examples
9. **09-patterns.md** - Common patterns
10. **10-best-practices.md** - Best practices
11. **11-triggers.md** - Triggers and events
12. **12-pragmas.md** - Pragmas and directives
13. **13-projections.md** - Metadata projections (`^^Len`, `^^Ptr`, …)

## Next Steps

Start with [01-basics.md](01-basics.md) to learn the fundamentals!

---

*Last updated: 2026-07-31*
