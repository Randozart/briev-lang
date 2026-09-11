# Briev Language Reference Guide

> **SUPERSEDED (2026-09-08).** This reference predates the 2026-08-05
> spec-conformance migration and teaches removed syntax (`state`, `sig`,
> `rstruct`, `syscall`, `meld`, `frgn!`, `|>`, `:>`, `<:`, `~/`, `escape`,
> `#pragma`). The current master reference is
> [MASTER-SYNTAX-REFERENCE.md](MASTER-SYNTAX-REFERENCE.md), generated from
> code (`src/vocab.rs`, `src/lexer.rs`, `src/intrinsic_signatures.rs`).
> Kept for historical context only.

**Version:** v0.16.0
**Date:** 2026-07-09
**Status:** Development (Phase 2/3 complete: Strong Bits thesis, intrinsic reduction)

---

## Table of Contents

1. [Lexical Structure](#lexical-structure)
2. [Types](#types)
3. [State Declarations](#state-declarations)
4. [Constants](#constants)
5. [Top-Level Statements (Scripting Mode)](#top-level-statements-scripting-mode)
6. [Transactions](#transactions)
6. [Contracts](#contracts)
7. [Definitions (defn)](#definitions-defn)
8. [Structs](#structs)
9. [RStructs (Reactive Structs)](#rstructs-reactive-structs)
10. [Enums](#enums)
11. [Signatures (FFI)](#signatures-ffi)
12. [Foreign Bindings](#foreign-bindings)
13. [Resources](#resources)
14. [Triggers (EBV)](#triggers-ebv)
15. [Render Blocks (RBV)](#render-blocks-rbv)
16. [Imports](#imports)
17. [Expressions](#expressions)
18. [Statements](#statements)
19. [Inline Assembly](#inline-assembly)
20. [Time Units](#time-units)

---

## Lexical Structure

### Keywords

| Keyword | Aliases | Description |
|---------|---------|-------------|
| `sig` | `sign`, `signature` | Foreign function signature |
| `defn` | `def`, `definition` | Function/predicate definition |
| `let` | - | State variable declaration |
| `const` | `constant` | Constant declaration |
| `txn` | `transact`, `transaction` | Transaction |
| `node` | - | Reactive transaction |
| `async` | - | Async modifier |
| `term` | - | Termination statement |
| `escape` | - | Escape/return statement |
| `import` | - | Import statement |
| `from` | - | Import path delimiter |
| `as` | - | Alias/rename |
| `frgn` | - | Foreign binding |
| `frgn!` | - | Foreign binding (native) |
| `syscall` | - | System call binding |
| `syscall!` | - | System call binding (native) |
| `resource` | `rsrc` | Resource declaration |
| `struct` | - | Struct definition |
| `rstruct` | - | Reactive struct definition |
| `render` | - | Render block (RBV) |
| `enum` | - | Enum definition |
| `trg` | - | Hardware trigger (EBV) |
| `stage` | - | Pipeline stage |
| `on` | - | Trigger condition |
| `forall` | - | Universal quantifier *(planned)* |
| `exists` | - | Existential quantifier *(planned)* |
| `within` | - | Timeout clause |
| `bank` | - | Memory bank |
| `match` | - | Match expression *(planned/not fully implemented)* |
| `asm` | - | Inline assembly block |
| `some` | `none` | Option variants |

### Type Keywords

| Keyword | Aliases | Description |
|---------|---------|-------------|
| `Int` | - | Signed integer |
| `UInt` | `Unsigned`, `USgn` | Unsigned integer |
| `Signed` | `Sgn` | Signed type |
| `Float` | - | Floating point |
| `String` | - | String type |
| `Bool` | - | Boolean |
| `Data` | - | Raw data |
| `Void` | - | Void/no return |

### Operators

| Operator | Description | Group |
|----------|-------------|-------|
| `=` | Assignment | — |
| `==` | Equality | — |
| `!=` | Inequality | — |
| `<` | Less than | — |
| `<=` | Less or equal | — |
| `>` | Greater than | — |
| `>=` | Greater or equal | — |
| `<<` | Shift left | — |
| `>>` | Shift right | — |
| `&` | Mutable reference / Bitwise AND | — |
| `\|` | Bitwise OR | — |
| `\|\|` | Logical OR | — |
| `&&` | Logical AND | — |
| `!` | Logical NOT | — |
| `-` | Negation | — |
| `~` | Bitwise NOT | — |
| `~/` | Prior state toggle | — |
| `+` | Addition | — |
| `*` | Multiplication | — |
| `/` | Division | — |
| `^` | Bitwise XOR | — |
| `->` | Arrow/return type | — |
| `<-` | Mutation / Discard | **Transfer** |
| `:>` | Metadata extraction / projection | **Lens (Projection)** |
| `<:` | Type derivation | **Lens (Derivation)** |
| `[]` | Brackets / Contracts / Partitioning | **Partition** |
| `@` | Address / Prior state / Anchor | **Anchor** |
| `?` | Optional watchdog prefix | — |

### Punctuation

| Token | Description |
|-------|-------------|
| `[` `]` | Brackets / Contracts |
| `{` `}` | Blocks |
| `(` `)` | Groups / Parameters |
| `:` | Type annotation |
| `,` | Separator |
| `;` | Statement terminator |
| `..` | Range |
| `.` | Field access / Namespace / RStruct method |

---

## Types

### Primitive Types

```briev
let x: Int = 42;
let y: UInt = 100;
let flag: Bool = true;
let name: String = "hello";
let pi: Float = 3.14;
```

### Vector Types

```briev
let buffer: Int[16];        // Fixed-size array
let matrix: Float[4][4];     // 2D array
```

### Constrained Types (Bit-Range)

```briev
let byte: UInt /8;          // 8-bit unsigned
let nibble: UInt /4;        // 4-bit unsigned
let word: Int /16;          // 16-bit signed
let flags: UInt /x8;        // Inferred 8-bit
```

### Pointer Types

```briev
let p: Ptr<Int> = &field;           // Mutable pointer to state field
let r: PtrConst<Int> = &let_var;    // Read-only pointer to let binding
let v = *p;                          // Dereference: reads pointed-to value
```

`Ptr<T>` and `PtrConst<T>` are the types produced by the `&` operator.
- `&state_field` → `Ptr<T>` (mutable — can write through it)
- `&let_binding` → `PtrConst<T>` (read-only)
- `*ptr` → `T` (dereference, type error on non-pointer)

For memory-mapped I/O (volatile access), use the explicit pointer types
described below in "Volatile Pointers".

### Union Types

```briev
let result: Result<Int, String> = Ok(42);
let value: Int | String = "hello";
```

### Custom Types

```briev
let point: Point;  // Declare variable of custom type
let status: Status;  // Declare variable of enum type
```

---

## State Declarations

### Basic Declaration

```briev
let counter: Int = 0;
let name: String = "test";
let enabled: Bool = false;
```

### With Address Mapping (EBV)

```briev
let led: Bool @ 0x4000 = false;           // Memory-mapped at 0x4000
let sensor: UInt @ 0x8000 /8;             // 8-bit sensor at 0x8000
```

### With Bit-Range

```briev
let flags: UInt @ 0x1000 /0..7;           // Bits 0-7 at address 0x1000
let status: UInt /4;                      // 4-bit field
```

### Address with Bit-Range Shorthand

```briev
let data: UInt @ 0x2000 /x16;             // 16-bit value at 0x2000
```

### Memory Regions

```briev
let stack_var: Int @ stack:8;             // Stack offset 8
let heap_ptr: Int @ heap:16;               // Heap offset 16
```

### Vector with Address

```briev
let buffer: UInt[256] @ 0x1000;            // 256-element buffer at 0x1000
```

---

## Constants

```briev
const MAX_SIZE: Int = 100;
const VERSION: String = "1.0.0";
const FLAGS: UInt = 0xFF;
```

---

## Top-Level Statements (Scripting Mode)

Executable statements written at global scope are automatically wrapped in a
synthesized `node __init` that fires once at startup.

```briev
let message: String = "Hello, Briev!";
println(message);           // top-level — no txn wrapper needed
```

The compiler generates a one-shot transaction equivalent to:

```briev
let message: String = "Hello, Briev!";
let __booted_0: Int = 0;
node __init [!__booted_0][__booted_0] {
    println(message);
    __booted_0 = 1;
    term;
};
```

**Rules:**
- All declarations (`let`, `const`, `struct`, `enum`, `defn`, `txn`, `frgn`)
  must precede top-level statements. A declaration after a statement is a
  compile error.
- Statements execute in program order, exactly once, then the program exits.
- `escape` inside a top-level statement atomically rolls back all state
  changes.
- The `__init` transaction is subject to normal optimizer treatment:
  pure const scripts may be precomputed; scripts with FFI emit runtime code.

**Pipeline position:** `Program::synthesize_init_txn()` is called after
import resolution, before desugaring, in `run_llvm_compile`, `run_check`,
and `run_rbv`.

Requires `.bv`, `.sbv`, `.rbv`, `.srbv` extensions (standard or strict tiers).

---

## Transactions

### Basic Transaction

```briev
txn name [precondition] [postcondition] {
    // body
    term;
};
```

### Reactive Transaction (RCT)

```briev
node name [precondition] [postcondition] {
    variable = value;
    term;
};
```

### Async Transaction

```briev
async txn name [precondition] [postcondition] {
    term;
};
```

### Reactive Async Transaction

```briev
async node name [precondition] [postcondition] {
    term;
};
```

### With Parameters

```briev
txn add [a: Int] [b: Int] [result == a + b] {
    term result;
};
```

### Lambda-style (No Body)

```briev
txn identity [x: Int] [result == x];
```

### With Reactor Speed

```briev
node blink @60Hz [true] [led == !led] {
    term;
};
```

### Transaction Method (dot syntax)

```briev
node counter.increment [count < max] [count == @count + 1] {
    count = count + 1;
    term;
};
```

### Transaction Dependencies

Dependencies are inferred from pre/post conditions automatically.

---

## Contracts

### Precondition and Postcondition

```briev
[pre_condition] [post_condition]
```

### Watchdog (Third Contract Bracket)

```briev
[pre][post][watchdog]     // Required watchdog (default)
[pre][post][?watchdog]     // Optional watchdog
```

The watchdog is checked at `term`.

### Prior State Toggle Shorthand

```briev
~/identifier
```

Expands to: `[~identifier][identifier]`

```briev
node toggle [~/ready][ready] {
    ready = !ready;
    term;
};
```

### Examples

```briev
node increment [counter < 10]
  [counter == @counter + 1]
{
    counter = counter + 1;
    term;
};

node guarded [x > 0][y == x * 2]
{
    &y = x * 2;
    term;
};

node with_watchdog [ready == true][done == true][?timeout] {
    done = true;
    term;
};
```

---

## Definitions (defn)

### Predicate Definition

```briev
defn sufficient_funds(amount: Int) [amount > 0][true] -> Bool {
    term amount >= minimum_balance;
};
```

### Function Definition with Contract

```briev
defn square(x: Int) [true] [result == x * x] -> Int {
    term x * x;
};
```

---

## Structs

```briev
struct Point {
    let x: Int = 0;
    let y: Int = 0;
};

struct Rectangle {
    width: Int,
    height: Int,
};

// Struct with embedded transactions
struct Counter {
    let value: Int = 0;

    txn increment [value < 100][value == @value + 1] {
        value = value + 1;
        term;
    };
};
```

### Field Declaration Syntax

```briev
// Using let (required initializer)
let x: Int = 0;
let y: Int = 0;

// Direct field syntax
field_name: Type,
```

---

## RStructs (Reactive Structs)

RStructs automatically namespace transactions with the struct name.

```briev
rstruct Counter {
    let value: Int = 0;

    txn increment [value < 100][value == @value + 1] {
        value = value + 1;
        term;
    };
};
```

After parsing, `increment` becomes `Counter.increment`.

---

## Enums

### Simple Enum

```briev
enum Status {
    Idle,
    Processing,
    Done,
    Error,
};
```

### Enum with Type Parameters

```briev
enum Result<T, E> {
    Ok(T),
    Err(E),
};
```

### Tuple Variants

```briev
enum Value {
    Int(Int),
    Float(Float),
    Pair(Int, Float),
};
```

### Enum Usage

Enums use bare variant names:

```briev
let state: Status = Idle;

// Comparison
[state == Idle] state = Processing;

// Pattern matching via guard
Status(s) = state;
[s == Processing] state = Done;
```

**Note:** Enum variants are compared and assigned using bare names, not namespaced syntax.

---

## Signatures (FFI)

### Basic Signature

```briev
sig my_function: Int -> Bool;
```

### With Source

```briev
sig read: String -> String from "io.fs";
```

### With Binding

```briev
sig process: Int -> Int = complex(x);
```

---

## Foreign Bindings

### New FFI Syntax (v0.12+)

The new FFI system uses language profiles and doesn't require TOML file references:

#### File-Level Attribute

```briev
#![ffi.<lang>, bind("./profile.toml"), import("./script"), map("from","to")]
```

- `ffi.<lang>` - Language target: `c`, `kernel`, `js`, `rust`, `wasm`
- `bind()` - Optional profile TOML path
- `import()` - Optional script/library path  
- `map()` - Inline type mapping overrides

#### Foreign Function (Standard)

```briev
frgn printk(fmt: String) -> Result<Int, Err>;
```

#### Foreign Function (Fire-and-Forget / Void Return)

```briev
frgn! printk(fmt: String);
```

#### Foreign Function with Explicit Address

```briev
frgn read_reg @ 0x40001000 () -> Result<UInt, Err>;
frgn! write_reg @ 0x40001000 (val: UInt);
```

#### Per-Function Type Override

```briev
#[ffi.c, type("Int32Array")]
frgn get_buffer() -> Result<UInt, Err>;
```

### Legacy Syntax (Still Supported)

```briev
frgn fetch(url: String) -> Result<Data, Error> from "http.toml";
```

### System Call

```briev
syscall! read(fd: Int, buf: String) -> Result<Int, Error>;
```

---

## Resources

```briev
resource uart: UART {
    baud_rate: 9600,
    parity: None,
};

rsrc buffer: RingBuffer {
    size: 1024,
    element_type: UInt,
};
```

---

## Triggers (EBV)

Hardware triggers define external input signals.

```briev
trg button: Bool @ 0x4000;
trg sensor: UInt @ 0x8000 /8;
```

Synthesized to: `input logic button;`

---

## Render Blocks (RBV)

```briev
render Counter {
    <div class="counter">
        <span b-text="value">0</span>
        <button b-trigger:click="increment">+</button>
        <button b-trigger:click="decrement">-</button>
    </div>
}
```

### RBV Directives

| Directive | Example | Description |
|-----------|---------|-------------|
| `b-text` | `b-text="count"` | Text content binding |
| `b-show` | `b-show="visible"` | Conditional show |
| `b-hide` | `b-hide="hidden"` | Conditional hide |
| `b-trigger:event` | `b-trigger:click="txn"` | Event trigger |
| `b-on:event` | `b-on:submit="action"` | Event trigger (alt) |
| `b-class` | `b-class="{active: isActive}"` | Dynamic class |
| `b-attr` | `b-attr="disabled: isDisabled"` | Dynamic attribute |
| `b-style` | `b-style="color: fg"` | Dynamic style |
| `b-each` | `b-each="item in items"` | List rendering |

---

## Imports

### Single Import

```briev
import "std/io";
```

### Multiple Imports

```briev
import {
    "std/io",
    "std/strings",
    "custom/utils",
};
```

### With Alias

```briev
import "std/io" as io;
```

---

## Expressions

### Literals

```briev
42          // Integer
3.14        // Float
"hello"     // String
true        // Boolean
false       // Boolean
```

### Identifiers

```briev
counter
max_value
is_enabled
```

### Prior State (@)

```briev
@counter        // Previous value of counter
@x + 1          // Prior x plus 1
```

### Mutable Reference (&)

```briev
&variable       // Mutable reference for assignment
```

### Unary Operations

```briev
!flag           // Logical NOT
-x              // Arithmetic negation
~bits           // Bitwise NOT
&var            // Address-of: creates a Ptr<T> or PtrConst<T> to var
*ptr            // Dereference: reads the value pointed to by ptr
```

### Binary Operations

```briev
x + y           // Addition
x - y           // Subtraction
x * y           // Multiplication
x / y           // Division
x == y          // Equality
x != y          // Inequality
x < y           // Less than
x <= y          // Less or equal
x > y           // Greater than
x >= y          // Greater or equal
x && y          // Logical AND
x || y          // Logical OR
x & y           // Bitwise AND
x | y           // Bitwise OR
x ^ y           // Bitwise XOR
x << n          // Shift left
x >> n          // Shift right
```

### Function Call

```briev
process(data)
max(a, b)
```

### Method Call

```briev
result.validate()
list.length()
```

### Field Access

```briev
point.x
rect.width
```

### Index Access

```briev
buffer[0]
matrix[i][j]
```

### Pattern Matching

Briev uses **guard-based pattern matching** for unions and enums:

```briev
// Extract variant from union type
let result: Int | Error = fetch_data();
Ok(value) = result;
[value > 0] status = Success;

// Enum pattern matching
let state: Status = Idle;
Status(s) = state;
[s == Processing] state = Done;
```

*(Note: `match { }` expression syntax is planned but not yet implemented)*

### Quantifiers *(planned)*

```briev
forall x in range(0, 10) { x >= 0 }
exists y in set { y > 0 }
```

---

## Statements

### Assignment

```briev
x = 42;
counter = counter + 1;
```

### Mutable Assignment (Pointer Write)

```briev
field = new_value;         // Direct state field write
*ptr = new_value;           // Write through pointer dereference
```

The LHS must be `&state_field` (creates a `Ptr<T>`) or `*ptr` where `ptr` has
type `Ptr<T>`. Writing through a `PtrConst<T>` is a compile-time error.

### With Timeout

```briev
result = read_spi() within 10 cycles;
data = fetch(url) within 100 ms;
```

### Guarded Statement

```briev
[condition] statement;
[condition] {
    // multiple statements
};
```

### Pattern Matching (Guard-based)

```briev
// Union type pattern extraction
let result: Int | Error = fetch();
Ok(value) = result;
[value > 0] status = Success;

// Enum variant extraction
let state: Status = Idle;
Status(s) = state;
```

### Term (Termination)

```briev
term;                     // Void termination
term result;              // Return value
term (a, b);              // Multiple outputs
```

### Escape

```briev
escape;                   // Early exit
escape error_code;        // Exit with value
```

### Expression Statement

```briev
process_data();
update_state();
```

### Inline Assembly

Low-level inline assembly for architecture-specific operations. Generates native code via `asm!` (Rust) or `__asm__ __volatile__` (C).

```briev
asm "DC CIVAC X0, X1" { "x0", "x1" };
asm "DSB SY" {};
asm "mov x0, #0" {};
```

**Syntax:**
- `asm "instruction" { clobber_list };`
- Clobbers are comma-separated register names in quotes
- Empty clobbers `{}` allowed for no-clobber instructions
- Generates commented template in Rust (requires nightly for real `asm!`)
- Generates `__asm__ __volatile__("instruction" : : : clobbers)` in C

**Usage:**
```briev
txn flush_cache {
    effect {
        // Flush data cache before DMA transfer
        asm "DC CIVAC X0, X1" { "x0", "x1" };
        asm "DSB SY" {};
    }
}
```

---

## Time Units

| Unit | Aliases | Description |
|------|---------|-------------|
| `cycles` | `cyc` | Clock cycles |
| `ms` | - | Milliseconds |
| `s` | `sec`, `seconds` | Seconds |
| `min` | `minute` | Minutes |

---

## Test Cases Reference

### Core Briev (.bv)

| File | Feature | Status |
|------|---------|--------|
| `core/01_basic_transaction.bv` | Basic transaction | ✅ Pass |
| `core/02_async_transaction.bv` | Async transactions | ✅ Pass |
| `core/03_unary_negation.bv` | Unary negation | ✅ Pass |
| `core/04_union_types.bv` | Union types | ✅ Pass |
| `core/05_guards.bv` | Guards | ✅ Pass |
| `core/06_dependencies.bv` | Transaction dependencies | ✅ Pass |
| `core/07_structs.bv` | Struct syntax | ✅ Pass |
| `core/08_enums.bv` | Enum syntax | ✅ Pass |
| `core/09_sig_type.bv` | Foreign signatures | ✅ Pass |
| `core/10_imports.bv` | Import statements | ✅ Pass |

### Embedded Briev (.bv - extended)

| File | Feature | Status |
|------|---------|--------|
| `embedded/01_vector_types.bv` | Vectors + bit-range | ✅ Pass |
| `embedded/02_watchdog.bv` | Watchdog contracts | ✅ Pass |
| `embedded/03_float_types.bv` | Float (parsing only) | ✅ Pass |
| `embedded/04_triggers.bv` | Trigger syntax | ✅ Pass |
| `embedded/05_within.bv` | Transaction syntax | ✅ Pass |
| `embedded/06_within_clause.bv` | Within clause | ✅ Pass |

---

## Language Variants

| Extension | Name | Description |
|-----------|------|-------------|
| `.bv` | Core Briev | Transactional state machines with FFI |
| `.ebv` | Embedded Briev | Adds vectors, bit-ranges, triggers, hardware mapping |
| `.rbv` | Rendered Briev | Adds UI/view components with reactive bindings |

---

## Compilation Targets

The file extension selects the backend:

| Extension | Variant | Backend | Output |
|-----------|---------|---------|--------|
| `.bv` | Briev | LLVM | Native binary (`llc` + `ld`) |
| `.ebv` | Embedded Briev | LLVM | Microcontroller binary |
| `.abv` | Accelerated Briev | LLVM | SPIR-V GPU kernel |
| `.rbv` | Rendered Briev | Webstack | WASM + JS shim + view bindings |
| `.sbv` | Silicon Briev | CIRCT | Verilog/VHDL (via MLIR) |
| `.dbv` / `.dbvs` / `.dbvl` | Data Briev | (parsed by Briev) | Configuration data |

### LLVM Native (`.bv`)
```bash
briev-compiler build input.bv                # full compile chain
briev-compiler build --llvm input.bv          # emit LLVM IR only
briev-compiler check input.bv                # type-check only
```

### Web Frontend (`.rbv`)
```bash
briev-compiler build input.rbv --backend webstack   # WASM + JS shim
```

### Hardware (`.sbv`)
```bash
briev-compiler build input.sbv               # CIRCT MLIR → Verilog/VHDL
```

### GPU (`.abv`)
```bash
briev-compiler build input.abv               # SPIR-V kernel
```

### Strict Mode (`.sbv`, `.srbv`, `.sebv`)
```bash
briev-compiler build --strict input.sbv      # full contracts required
```
