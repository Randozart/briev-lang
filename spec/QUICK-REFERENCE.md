# Briev Quick Reference

> **SUPERSEDED (2026-09-08).** Stale (v0.10.0, 2026-04-20) — teaches removed
> syntax (`state`, `sig #out`, `frgn!`, `unbinding`, `Result.is_ok()`). The
> current master reference is
> `docs/reference/MASTER-SYNTAX-REFERENCE.md`, generated from code. Kept for
> historical context only.

## Syntax at a Glance

### Basic Declarations

```briev
// State declaration
state <name>: <type> = <expr>?

// Transaction
node <name>(<params>) [pre][post] {
    // body
}

// Function definition
defn <name>(<params>) -> <outputs> [pre][post] {
    // body
}

// Signature (FFI): declares an external symbol
frgn <name>(<params>) -> Result<T, E> from "c";

// Fire-and-forget FFI
frgn! <name>(<params>);

// Observable output (prevents dead-code elimination)
sig #out <name>(<params>) -> T from <path>;

// Inline/pure (safe to fold)
sig #inline <name>(<params>) -> T;

// Import source file to link
import "link/briev_rt.c";
```

### FFI Keywords

| Keyword | Returns | Use |
|---------|---------|-----|
| `frgn` | `Result<T, E>` | Import foreign function, handle errors |
| `frgn!` | (none) | Fire-and-forget FFI call — no return captured |
| `sig #out` | (modifier) | Observable output — prevents DCE |
| `sig #inline` | (modifier) | Pure — safe to fold/eliminate |

### `from` Targets

| Value | Language | Notes |
|-------|----------|-------|
| `"c"` | C/LLVM | Zero-cost inlining via LTO |
| `"rust"` | Rust | Zero-cost inlining via LTO |
| `"js"` | JavaScript | Interpreter only |
| `"python"` | Python | Interpreter only |
| (omitted) | Any | Searches `import "link/..."` targets |

### Output Types

```briev
// Single output
-> Bool

// Tuple (multiple values)
-> Bool, String, Int

// Array of types
-> Bool[]

// Named slots
-> name: String, value: Int

// Union (alternatives)
-> Result<Int, Error> | Timeout
```

### Multi-Output Term

```briev
term a, b, c;          // returns tuple of a, b, c
term item;             // returns single value
term;                  // returns nothing
```

### Import Linking

```briev
import "link/briev_rt.c";    // C source → LLVM IR → llvm-link
import "link/rust_lib.rs";   // Rust library
import "link/zig_lib.zig";   // Zig library
```

### Address Operators

| Operator | Meaning |
|----------|----------|
| `@addr` | Target-dependent address |
| `@raw:0xADDR` | Raw physical address (embedded) |
| `@stack:OFFSET` | Stack-relative |
| `@heap:OFFSET` | Heap-relative |

### Operator Taxonomy

Briev's operators belong to three conceptual groups:

| Group | Operators | Purpose |
|-------|-----------|---------|
| **Lens Operators** | `<:` (Derivation), `:>` (Projection) | Type boundaries and semantic lenses — derivation restricts what conforms, projection reveals meaning |
| **Partition Operators** | `[]`, `@/` | Segment layouts into addressable sub-ranges |
| **Transfer Operator** | `<-` | Directional data movement across boundaries |

The **Anchor** (`@`) is a universal modifier for spatial/temporal location across all contexts.

### Control Flow

```briev
// Guards (branching)
[guard_expr] {
    // executes when guard is true
}

// Pattern matching
unbinding <name>(<pattern>) = <expr>
```

### Result Type Methods

```briev
result.is_ok()     // Bool
result.is_err()    // Bool  
result.value       // Unwrapped value
result.error.code  // Error code
result.error.message  // Error message
```

## Types Quick Reference

| Type | Description |
|------|-------------|
| `Int` | Signed 64-bit int |
| `Float` | 32-bit float |
| `UInt` | Unsigned 64-bit int |
| `Bool` | Boolean (1 bit) |
| `String` | UTF-8 string |
| `Data` | Opaque binary data |
| `Void` | Unit/empty type |
| `Vector[T]` | Fixed-size vector |
| `Option[T]` | Nullable type |
| `Sig[T]` | Signature reference |
| `Result[T, E]` | FFI return type |
| `Ptr` | Bare pointer — Ptr\<Bits @/0..63\> (safe void\*) |
| `Ptr<T>` | Typed pointer to T |
| `Ptr32` | 4-byte pointee (Ptr\<Bits @/0..31\>) |
| `Ptr64` | 8-byte pointee (Ptr\<Bits @/0..63\>) |

## Common Patterns

### Error Handling

```briev
let result = read_file(path);
[result.is_ok()] {
    term result.value;
} [result.is_err()] {
    term "default";
};
```

### Fire-and-Forget FFI

```briev
frgn! send_message(msg: String);
```

### Importing

```briev
import "std/io";
import "std/math" as math;
import {File, Dir} from "std/fs" from "fs.toml";
```

### Multi-Output

```briev
defn get_pair() -> (Int, String) [true] {
    term (42, "answer");
};
```

## See Also

* [Full Specification](SPEC.md)
* [Language Tutorial](LANGUAGE-TUTORIAL.md)
* [FFI Guide](FFI-GUIDE.md)
* [Examples](examples/)

*Quick reference - last updated v0.10.0 (2026-04-20)*
## Machine entry (rv64/embedded, 2026-09-14)

```briev
bootstrap node reset [armed == true] { … }        // authored machine entry
node trap_service @ timer_irq [current >= 0][…] { … }   // machine-serviced event
```

- `bootstrap node` — the program's machine beginning; compiler owns the
  sp/.bss scaffold; the single bracket is the handoff postcondition
  (proven from the body's typed stores); canned `_start` = fallback.
- `node @ vector` — machine-serviced: the vector names a board
  `interrupts.dbvl` entry or a literal slot; the mechanism comes from the
  active target profile (`isr_mechanism`); the body runs once per event —
  straight-line, never a convergence loop.
- Scheduler/kernel logic: plain `defn`s (linear per trap — defn contract
  brackets are documentation, never a convergence loop).
- Everything machine-specific lives in config + board files: the program
  names no mechanism, no register, no C.
