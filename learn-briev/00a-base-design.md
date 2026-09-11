# The Briev Mindset

**How to read and think in Briev**

---

## Symbolic Design Philosophy: What the Symbols Mean

Briev's symbols are not arbitrary ASCII choices. Each symbol's **visual shape** maps to a **cognitive metaphor**, which maps to a **systems meaning**. All uses of a given symbol share that core metaphor.

| Symbol | Visual Shape | Cognitive Metaphor | Systems Meaning | Group |
|--------|-------------|-------------------|----------------|-------|
| **`;`** | A dot with a tail falling away | A hard stop, a reset | Universal statement termination. The parser syncs here. | — |
| **`.`** | A single pinpoint | Puncturing, reaching into | Struct field access / method call — you reach into a thing. | — |
| **`->`** | An arrow pointing right | Forward motion, transformation | Dataflow / State transition — something becomes something else. | — |
| **`<-`** | An arrow pointing left | Backward motion, extraction | Mutation / Discard — something comes out of something. | **Transfer** |
| **`:`** | Two stacked dots | Identity, equivalence | Static type / definition — "This IS that." | — |
| **`.^` / `.^^`** | Pinpoint + caret(s) | Reflecting on a value/type | **Reflection** — read compiler-known metadata. `.^` = runtime (length, pointer), `.^^` = compile-time (size, bytes, alignment). | **Reflection** |
| **`[]`** | Brackets that enclose | Containment, boundary | Constraints, bounds, guards — everything inside `[]` is bounded. | **Partition** |
| **`{}`** | Curly braces that hug | Grouping, bundling | Code block / organizational unit. | — |
| **`()`** | Parentheses that cup | Holding, containing | Parameter / argument enclosure. | **Application** |
| **`<>`** | Angle brackets that cradle | Specializing, parameterizing | Type-level specialization — a named kind of the thing. | **Specialization** |
| **`@`** | The at-sign — a loop with an 'a' | Position, location, anchor | Spatial / Temporal / Dimensional / Chronological anchor. | **Anchor** |
| **`&`** | Ampersand — ligature of "et" (and) | Connection, conjunction | Mutation marker — links the name to the mutable location. | — |
| **`!`** | A vertical line with a dot | An exclamation, a warning | Control flow anomaly / boundary — "pay attention." | — |
| **`~`** | A wavy line | Oscillation, flipping | Boolean toggle — flip back and forth like a waveform. | — |
| **`?`** | A hook | A question, a check | Watchdog / timeout — "is this still OK?" | — |
| **`_`** | A small horizontal line | A gap, a placeholder | Ignored / unused value. Works in destructuring: `let (_, value) = pair;` — |

### The Principle: Syntactic Radical Honesty

If an operation has distinct physical, temporal, or compiler-level behavior under the hood, its visual representation must explicitly reflect that boundary. Every boundary-crossing operation uses a different visual symbol. No hidden transformations.

### The Delimiter Semantic Load

Four delimiters, four honest meanings — never swapped:

| Delimiter | Load | Examples |
|-----------|------|----------|
| `<>` | **compile-time type-level specialization** — a named kind of the thing | `Stack<T>`, `String<UTF8>`, `asm<x86_64>`, `sync<group>` |
| `()` | **application & binding** — call it, construct it, bind an implementation to it | `f(a)`, `defn f(x: Int)`, `Person(...)`, `op Add: func(#L,#R)`, `op Add(Float)` |
| `[]` | **containment / bound** — bounded by it | `Int[8]`, `[pre]` guards |
| `{}` | **grouping / definition** — bundle it | blocks, struct literals |

- If the thing in the delimiters is a **compile-time identity or type**, it is `<>` (which variant, which target, which group).
- If it is a **value being applied or bound**, it is `()` (a call, a parameter, a construction).
- `sync<group>` uses `<>` because the group is a compile-time identity — the same shape as `asm<chip>` (which target) and `String<UTF8>` (which variant). `op Add(Float)` stays `()`: `op` is a nested declaration, declarations take params, and it avoids angle-bracket nesting.
- A delimiter used for the wrong load is a design error, not a stylistic choice.

---

## Why This Document Exists

Most language tutorials teach you the **syntax** - what to write. This document teaches you how to **see** Briev code. It's the mental model you need to read Briev the way a Briev developer reads it, understanding intent at a glance.

If you're the kind of learner who picks up languages by "getting a feel" for them, this is for you.

---

## The Core Philosophy

Briev removes familiar imperative constructs **by design**. No `if/else`, no `while`, no `for`.

This is **not a limitation**. It's the foundation of what makes Briev work. When code can't branch unpredictably, the compiler can **prove** properties about it. No races. No unhandled cases. No deadlocks.

**Never-nesting** - Treat each block of code as an individual building block to be strung together, not nested inside other blocks.

---

## What Symbols Mean

### `[ ]` - Brackets for Logic

Square brackets are for **logical checks and verification**. This is the primary way Briev makes decisions.

```briev
[x > 0] {        // Guard: only runs if x is positive
    result = x;
};
```

**Vector format**: `Vector<T, dim1, dim2, ...>` with commas, not angle brackets for dimensions

```briev
let items: Vector<Int, 10>;   // Type declaration - obviously not a check
let x = items[5];             // Index access
let matrix: Vector<Int, 10, 20>;    // 2D vector
```

---

### `< >` - Angle Brackets for Types

When you see angle brackets, think **Type**. This is almost always a generic or comparison.

```briev
HashMap<String, Int>;    // Generic type
Option<String>;         // Optional type
Result<Int, Error>;    // Result type
```

**Exception**: Rendered Briev's inline HTML uses `<tag>` for actual HTML elements:

```briev
<div class="card">     // HTML element in .rbv view block
    <span>Hello</span>
</div>
```

---

### `{ }` - Curly Braces for Code Blocks

Curly braces **divvy up code** - they group statements into logical units:

```briev
node increment [count < 100][count == 100] {
    count = count + 1;   // Block 1
    term;                  // Block 2
};
```

Unlike languages where `{` starts a new scope, in Briev each block is just organization.

---

### `( )` - Parentheses for Arguments

Parentheses are for **arguments** - parameters to functions, transactions, or calls:

```briev
txn withdraw(amount: Int) (...) { ... };
defn max(a: Int, b: Int) -> Int (...) { ... };
io.println("hello");
```

---

### `.` - The Accessor (Field Access & UFCS)

When you see `.`, you're accessing something **inside** a struct or type:

```briev
account.balance         // Field access
list.len()           // UFCS: desugars to len(list)
result.value         // Result unwrapping
```

**UFCS (Uniform Function Call Syntax):** `subject.method(args)` resolves to `method(subject, args)`. There is zero magic — the compiler has no hardcoded knowledge of `.len()` or any method name. `list.len()` becomes `len(list)`, which calls the standard library function that uses `list.^Length`. **2026-08-14 (Universal Operation Language):** every operation has three surfaces — a symbol (`+`, `c[i]`, `<-`), an intrinsic (`OpName#(a, b)`), and the UFCS method form (`a.OpName#(b)`). `a.OpName#(b)` resolves through the intrinsic/op identity when no literal member exists.

**Priority hierarchy:**
1. **Internal struct field/defn** — if `subject` has a field or internal `defn` defined in its struct body, it compiles as a direct access
2. **Operation/intrinsic (`OpName#`)** — `subject.OpName#(args)` resolves to `OpName#(subject, args)` (the op member or registered intrinsic)
3. **UFCS fallback** — otherwise resolves to `method(subject, args)`

**What this means**: Briev is transparent. If you see `something.field`, that struct exists somewhere in the standard library. Nothing is hidden magic.

---

### Reflection (`.^` runtime, `.^^` compile-time)

Reflection reads compiler-known metadata about a value or its type. Think of
it as "ask the compiler for a property of this thing":

```briev
list.^Len;       // runtime length (elements)
x.^^Bytes;       // compile-time storage size
&x;              // verified pointer (the & operator; x.^Ptr is the reflection form)
x.^^Size;        // compile-time element count (Int[8].^^Size → 8)
x.^^Alignment;   // compile-time alignment
x.^^Type;        // compile-time type identity
```

`.^` is **runtime** reflection (value-derived: length, pointer); `.^^` is
**compile-time** reflection (type-derived, foldable: size, bytes, alignment).
Targets are PascalCase compiler-known identifiers — using one with the wrong
operator (or an unknown name) is a compile error. The historical `:>`/`<:`
lens operators and bit-intrinsic projections (Popcount, Absolute, …) were
removed with the hashword-protocol architecture; the LLVM bit intrinsics are
declared but have no operator form. Every `.^`/`.^^` target maps to either a
compile-time constant or a zero-cost intrinsic.

See `learn-briev/13-projections.md` for the complete reference.

---

### `@` — The Universal Anchor

`@` is Briev's universal **Anchor** — a single symbol for spatial and temporal location across every context:

| Context | Example | What it anchors |
|---------|---------|----------------|
| Goal state | `[balance >= 0]` | The postcondition the reactor proves reachable |
| String literal | `@"..."` | Anchors the string to a compile-time memory slot |
| Bit position | `@/0..3` | Anchors a field to an absolute bit offset |
| Hardware link | `trg timer @ 1kHz` | Anchors a timer to a hardware or OS resource |
| Memory address | `let led: Bool @ 0x40020000` | Anchors a variable to a physical address |

```briev
// Goal: balance must stay non-negative after withdrawal
txn withdraw(amount: Int)
    [balance >= amount]
    [balance >= 0]
{
    balance = balance - amount;
    term;
};

// Bit-position anchor — extract bits 0-3
let nibble = word @/0..3;

// String anchor — compile-time memory slot
@"hello world" .^Len   // 11
```

---

### `&` - The Mutation Marker

**You must use `&` to mutate state.** This is mandatory and deliberate.

```briev
count = count + 1;    // Correct - mutates state
count = count + 1;     // Wrong - won't compile
```

This explicit marker exists because mutations are verification-critical. The compiler needs to know exactly what's changing.

---

### `~` — Consumptive Operators (and unary bitwise NOT)

`~` has two honest meanings (2026-08-01, Phase 3):

1. **Unary `~x`** is bitwise NOT — unchanged.
2. **`~` prepended to a binary operator** makes it **consumptive**: the RHS is
   consumed (its backing destroyed) after the op. Only a mutable lvalue can be
   consumed; reading it afterward is a use-after-move compile error.

```briev
a ~= b;      // move-assign: a = b, then b is dead
a ~+ b;      // a = a + b, then b is dead
dest ~<- src;  // destructive extract: copy src's element into dest, then src is dead
~<- src;     // destructive discard
```

The old `~?` (temporal fallback) is removed; the old `~/` term-until token is
now the consumptive divide. "Until this holds" contracts use the `[!/X]` invert
form instead (see 02-contracts.md).

```briev
// Until-ready contracts, the modern form:
[!/ready]                  // pre !ready, post ready (was: [~/ready])
```

---

### `!` - "Something Weird"

The `!` suffix signals **this does something unusual to control flow**:

```briev
frgn! log_message(msg);   // Fire-and-forget - no Result to check, runs and forgets
syscall! exit(code);      // Kernel call that never returns
trg! interrupt();        // Trigger that can fire during any function
term!;                    // Immediate process termination
```

When you see `!`, pause and think: "What makes this call unusual?"

---

### `?` - Watchdog / Timeout

`?` marks a **watchdog** - a timeout or external condition:

```briev
txn long_operation() [true][done] ?[5000ms] {   // Must finish in 5 seconds
    do_work();
    done = true;
    term;
};
```

---

### `->` / `<-` - Directional Dataflow and Transition

Arrows always represent **directional movement, dataflow, or state transitions**:

```briev
&list <- x;                        # Push: x ends up in list
x <- &list;                        # Pop: last element becomes x
<- &list;                          # Discard: pop last element, throw away
term -> order_status = 1;         # Swan song: on successful term, set status
defn double(x: Int) -> Int [...] { # Signature: input transitions to output
    term x * 2;
};
```

`->` means forward (data goes right). `<-` means backward (data comes left). The direction tells you which way values move.

### `< -` The Discard Operator

`<- expr` explicitly discards the result of an expression. This is required for syscall results that you don't want to handle:

```briev
<- syscall! @ 3 (fd);              # Close fd, discard result
```

This ensures no system-level side-effect can ever be silently ignored. The compiler forces you to acknowledge the boundary.

---

### `;` - Universal Statement Termination

`;` is a hard stop. Every statement must end in `;`, including blocks denoted by `{}` (transaction bodies, struct definitions, pragmas):

```briev
node t [x < 10] [x == 10] {
    x = x + 1;
    term;
};
```

The parser uses `;` as an absolute synchronization token during error recovery, preventing cascading errors from a single syntax mistake.

---

## Keywords and Their Abbreviations

Briev has many **full forms** and **abbreviated forms**. The abbreviated ones are used by default. You can write them in all lowercase or all uppercase, but never mixed case.

| Abbrev | Full | Meaning |
|-------|------|--------|
| `txn` | `transaction` | State-changing operation |
| `node` | `reactive` | Auto-fires when precondition met |
| `defn` | `definition` | Function |
| `frgn` | `foreign` | FFI call (returns Result) |
| `frgn!` | `foreign!` | FFI fire-and-forget |
| `syscall` | `syscall` | Kernel call |
| `let` | `let` | Mutable state |
| `const` | `const` | Constant |
| `init` | `init` | Runtime-seeded invariant |
| `term` | `terminate` | Successful end |
| `escape` | `escape` | Rollback |
| `spawn` | `spawn` | Obj-instance/task creation |
| `box` | `box` | Storage: heap-per-instance, not pooled (contextual — only before `spawn`) |
| `spill` | `spill` | Storage: may grow past a static pool column (contextual — only before `spawn`) |

---

## What Keywords Signify

### `txn` - A Transaction

Something **will change state**. Transactions are atomic - they either complete fully or roll back.

```briev
node deposit(amount) [amount > 0][balance <= 1000] {
    balance = balance + amount;
    term;
};
```

### `node` - Reactive

This **fires automatically** when its precondition becomes true. No caller needed.

```briev
node auto_save() [dirty && !saving][!dirty] {
    save_to_disk();
    dirty = false;
    term;
};
```

### `defn` - A Pure Function

A calculation that doesn't mutate state (except via return).

```briev
defn absolute(x: Int) -> Int [true][result >= 0] {
    [x < 0] term -x;
    term x;
};
```

### `frgn` / `frgn!` - Foreign

Calls **outside** Briev. Requires error handling unless using `!` (fire-and-forget).

```briev
frgn sqrt(x: Float) -> Result<Float, MathError>;

frgn! log(msg: String) -> void;
```

---

## Reading Patterns

### Guards vs. If/Else

**Old thinking**: "If X, do Y. Otherwise, do Z."

**Briev thinking**: "When X is true, Y fires. When not-X is true, Z fires. Both can fire. I need to ensure my postcondition holds regardless."

```briev
// NOT if/else - these are guards, both CAN fire
[x > 0] positive = true;
[x < 0] negative = true;
```

### Contracts as Documentation

A transaction's contract is its **documentation**:

```briev
txn withdraw(amount)
    [amount > 0 && balance >= amount]      // When can this run?
    [balance >= 0]                          // What must be true after?
```

That precondition says: "You can withdraw if and only if the amount is positive AND you have enough balance." The postcondition says: "After withdrawing, your balance is exactly what it was minus the amount."

### The Postcondition Lies

If you see a weak postcondition like `[true][true]`, something is wrong:

```briev
// Suspicious - promises nothing
txn do_something [true][true] {
    // What does this actually guarantee?
};
```

A strong contract tells you what changed. A weak one is a red flag.

### `[[post]` and `[pre]]` — Contract Sugar

A txn has exactly one precondition and one postcondition: `[pre][post]`. To write
only one side, use sugar that fills the omitted side as `[true]`:

| Form | Meaning |
|------|---------|
| `[[post]` | Postcondition-only: `[true][post]` |
| `[pre]]` | Precondition-only: `[pre][true]` |

`[true][true]` is rejected by the parser — at least one side must be meaningful.

### `struct` and `T[N]` — Data Declarations

Briev distinguishes three declaration keywords:

- **`type`** — Protocols, operator bindings, type system extensibility
  (`type Int: Int { op Add(Int); };`)
- **`struct`** — Pure data, fixed layout, C-compatible, no methods
  (`struct Point { x: Int; y: Int; };`)
- **`obj`** — Full-featured types with methods, contracts, generics

Fixed-size arrays use bracket syntax: `Int[1024]` declares a compile-time-known
size, embedded as `[1024 x i64]` in LLVM IR and auto-vectorized.

---

## Why `term` and `escape`, Not `return` and `break`

Briev deliberately uses different words for ending a transaction: `term` (terminate) and `escape` (rollback).

### The Reasoning

Most languages use `return` and `break` because they assume:
- Code runs once from start to finish
- You might want to exit early from a loop
- The compiler doesn't need to verify your loop will end

Briev assumes:
- Transactions can **loop** (they run until their postcondition is satisfied)
- You might need to exit early AND rollback all changes
- The compiler **must verify** termination

When a node says `[count < 100][count == 100]`, it loops until count reaches exactly 100. The precondition is the loop condition; the goal is the termination state. The compiler proves the goal is reachable.

Thus `term` means "I succeeded, here's my return value." Not "I'm done, get out."

And `escape` means "Rollback everything - pretend this never happened." Not "break early."

### Why Reactive Transactions Are Inherent Loops

`node` can self-verify when to end:

```briev
node fill_buffer() [buffer .^Len < 100][buffer .^Len == 100] {
    buffer = buffer + [new_item];
    term;
};
```

This transaction will fire automatically when the buffer has fewer than 100 items. It keeps adding until the buffer has exactly 100. The **postcondition itself verifies termination** - the compiler proves the loop will end.

That's why `node` is different from a `while` loop: the reactor pattern includes its own exit condition in the contract.

---

## Why Lists and Vectors, Not Regular Arrays

Briev provides **`List<T>`** and **`Vector<T, dim1, dim2, ...>`**, not traditional arrays. This is deliberate.

### Spatial Thinking, Not Sequential

Regular arrays force you to think sequentially: `array[0]`, `array[1]`, `array[2]`. This pattern is hard to parallelize - SIMD operations need to see multiple elements at once.

Lists and Vectors encourage **spatial thinking**:

```briev
let items: List<Int> = [1, 2, 3, 4];                      // List - growable
let buffer: Vector<Int, 100>;                             // 1D vector - fixed size
let matrix: Vector<Int, 10, 20>;                         // 2D matrix
let tensor: Float, 3, 32, 32>;                         // 3D tensor
let persons: Vector<Person, width:50, height:50>;           // Named dimensions
```

When Briev compiles to hardware (`.ebv` → SystemVerilog/VHDL) or uses SIMD operations, the compiler can reason about the **entire structure at once**, not just iterate sequentially.

### The Difference

- **`List<T>`** - Dynamic, growable, heap-allocated
- **`Vector<T, dims...>`** - Fixed-size, contiguous memory, multidimensional, hardware-friendly

### Vector Declaration Syntax

Vectors use **angle brackets** with commas for multiple dimensions:
- First argument is the **element type**
- Remaining arguments are **dimensions**
- Dimensions can be **anonymous** (just numbers) or **named** (`name:size`)

```briev
Vector<Int, 10, 20>                             // 2D, 10x20
Vector<Person, width:50, height:50, time:10>      // Named dimensions
```

Choosing between List and Vector forces you to think about your access pattern upfront.

---

## Why Multiple File Extensions

Briev splits into **file extensions by target** to keep syntax clean and bake in base assumptions:

| Extension | Purpose | Assumptions |
|----------|---------|-----------|
| `.bv` | Pure Briev | Universal - assumes some architecture exists |
| `.rbv` | Rendered Briev | MUST run in browser - HTML/CSS/SVG embeddable |
| `.ebv` | Embedded | Bare-metal or FPGA - memory-mapped I/O, hardware triggers |
| `.dbv` | Data Briev | Configuration data - cleaner to audit than regular Briev |
| `.dbvs` | Data Briev Schema | Schema definitions for `.dbv` |

Each extension bakes in **what the target expects**:

- `.rbv` knows it's going to the browser - so view blocks use HTML syntax
- `.ebv` knows it's going to hardware - so addresses and triggers are first-class
- `.dbv` knows it's data - so syntax is streamlined for auditability

When you compile, these assumptions are already baked in. You don't need to specify "this is for the browser" - the file extension already says it.

---

## The Feel Summary

When you see **square brackets** `[ ]` → think **verification check**

When you see **angle brackets** `< >` → think **type**

When you see **curly braces** `{ }` → think **code block**

When you see **parentheses** `( )` → think **arguments**

When you see **dot** `.` → think **struct field / UFCS**

When you see **arrow** `->` → think **dataflow / state transition**

When you see **arrow** `<-` → think **mutation / discard**

When you see **colon** `:` → think **type identity**

When you see **dot-caret** `.^` / `.^^` → think **reflection** — compiler-known metadata about a value/type

When you see **semicolon** `;` → think **statement boundary**

When you see **ampersand** `&` → think **mutation (required)**

When you see **at sign** `@` → think **prior state / address anchor**

When you see **tilde** `~` → think **boolean toggle**

When you see **exclamation** `!` → think **control flow anomaly**

When you see **question mark** `?` → think **timeout/watchdog**

When you see **`txn`** → think **state change (atomic)**

When you see **`node`** → think **auto-fire on condition (inherent loop)**

When you see **`defn`** → think **pure calculation**

When you see **`frgn`** → think **external call**

When you see **`term`** → think **succeeded, return value**

When you see **`escape`** → think **rollback everything**

When you see **`List`** → think **dynamic, spatial**

When you see **`Vector`** → think **fixed, contiguous, hardware-friendly**

When you see **`.bv`** → think **pure Briev (universal)**

When you see **`.rbv`** → think **rendered (browser)**

When you see **`.ebv`** → think **embedded (hardware)**

When you see **`.dbv`** → think **data (audit-friendly)**

---

## Next Steps

Now that you have the feel, continue to [01-basics.md](01-basics.md) to learn the syntax.

---

*Last updated: 2026-05-08  
Version: Briev v0.12.0*