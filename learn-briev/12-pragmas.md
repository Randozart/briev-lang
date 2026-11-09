# 12. Compiler Pragmas — The Complete Reference

> These directives are Briev's complete set of compiler-intrinsic behavior.
> Everything else — imports, FFI, transactions, contracts — is standard library or user code.
> No hidden magic. If it doesn't appear on this page, the compiler doesn't know about it by name.

## Overview

Pragmas use the `#` prefix to signal "this is a compiler instruction, not application logic."
They are Briev's single, universal escape hatch, exhaustively documented here.

| Directive | Scope | Purpose |
|-----------|-------|---------|
| `#!dispatch(parallel)` | File-level | Enable parallel reactor dispatch |
| `#io` | File-level | Declare an OS-linked trigger |
| `#nowake` | Trigger modifier | Mark a trigger as passive (no wake) |

---

## `#!dispatch(parallel)` — Parallel Reactor Mode

**Syntax at top of file:**
```briev
#!dispatch(parallel)
```

**Effect:** Changes the reactor from sequential (one transaction per tick) to parallel
(multiple non-conflicting transactions per tick). The compiler:
1. Evaluates ALL transaction preconditions upfront
2. Allocates a `fired_mask` accumulator
3. Fires each transaction whose precondition is true AND whose write-mask
   doesn't overlap with already-fired transactions

**Conflict detection:** Two transactions conflict if they write to the same state variable.
The compiler computes a `u64` bitmask for each transaction (bit N = writes field N)
and checks masks at runtime before firing.

**Deprecated forms (kept for migration):**
```briev
#pragma dispatch(parallel)       // Old item-level syntax — use #!dispatch(parallel)
#!pragma dispatch(parallel)      // Old file-level syntax — use #!dispatch(parallel)
```

Use `#!dispatch(parallel)` at the file level or `#!dispatch(sequential)` for
the default first-true-wins semantics. The `#pragma` forms are deprecated.

---

## `#io` — OS Trigger Declarations

Declares an OS-linked trigger without needing to know the underlying C runtime symbol.

**Implicit form** (concept name = trigger name):
```briev
#io sigint;                     // Creates trg sigint: Bool @ link __sigint_flag
#io stdin_ready;                // Creates trg stdin_ready: Bool @ link __stdin_ready
#io stdin_line;                 // Creates trg stdin_line: String @ link __stdin_buffer
```

**Explicit form** (user-chosen name, type validation):
```briev
#io timer(1hz) -> trg clock_tick: Int;    // Validates type, renames trigger
```

**Parametrized concepts:**
```briev
#io timer(1hz);                  // 1-second timer
#io timer(100hz);                // 10ms timer
```

**Duplicates:** Error — each concept may only appear once per file.

**Type mismatch:** Error — if explicit form uses a different type than the registry declares.

### Canonical IO Concepts

| Concept | Type | Runtime Symbol | Description |
|---------|------|----------------|-------------|
| `sigint` | Bool | `__sigint_flag` | SIGINT interrupt (Ctrl+C) |
| `sigterm` | Bool | `__sigterm_flag` | SIGTERM termination signal |
| `sighup` | Bool | `__sighup_flag` | SIGHUP hangup signal |
| `stdin_ready` | Bool | `__stdin_ready` | Stdin has data available |
| `stdin_line` | String | `__stdin_buffer` | Current stdin line buffer |
| `timer(1hz)` | Int | `__timer_1hz` | 1-second timer tick |
| `timer(100hz)` | Int | `__timer_100hz` | 10ms timer tick |
| `io_pending` | Bool | `__io_pending` | Generic IO pending flag |
| `mouse_click` | Bool | `__io_mouse_click` | Mouse button click |
| `key_press` | Char | `__io_key_press` | Keyboard key press |

### Why `#io` exists

OS environments require platform-specific signal handling, timer setup, and event
demultiplexing. The `#io` pragma abstracts these details: the compiler maps each
concept name to the correct runtime symbol per target, and the runtime provides
the actual implementation signal handlers, epoll/kqueue integration, etc.

Embedded/Rendered Briev targets use raw `trg name: Type @ 0xADDRESS` instead,
where triggers are natively wake-capable via hardware interrupt lines.

---

## `#nowake` — Passive Trigger Modifier

All triggers are **wake by default** — the reactor blocks on epoll (for built-in
sources) or `__rt_wait()` (for MMIO-only programs) instead of busy-looping.

Use `#nowake` to mark a trigger as passive — polled on every tick but does not
prevent the reactor from sleeping:

```briev
trg sensor: Bool @ 0x5000 #nowake;             // passive MMIO — polled on demand
trg io_pending: Bool @ link __flag #nowake;    // passive link — doesn't wake
```

**Semantics:** Without `#nowake`, the trigger is wake-capable: the reactor may
use blocking waits (`epoll_wait`, `__rt_wait`) instead of busy-looping. With
`#nowake`, the trigger value is still sampled every tick but the reactor will
not block waiting for it.

---

## Migration Guide: `trg @ link` → `#io`

Old syntax (still works, fully supported):
```briev
trg sigint: Bool @ link __sigint_flag;
trg clock_tick_1hz: Int @ link __timer_1hz;
```

New syntax (preferred):
```briev
#io sigint;
#io timer(1hz) -> trg clock_tick_1hz: Int;
```

**Why migrate:**
1. No need to remember C runtime symbol names
2. Type validation — the compiler checks your declared type matches the registry
3. `#wake` is implied automatically
4. Platform portability — the registry can differ per target

**When NOT to migrate:**
- Custom runtime symbols not in the IO registry
- FPGA/bare-metal with hardware addresses
- Maximum portability (the `@ link` form works on any target without registry support)

---

## Annotations (`#`, `#!`, `#?`) — Compiler Directives

Annotations use `#` before an item to tell the compiler what to do, not what
the item is. They appear on the signature line before the keyword:

```briev
#!out txn write_port() [*][*] { &port = value; term; };
```

| Form | Meaning |
|------|---------|
| `#!out` | Mandatory: this has external effects |
| bare `#?` | Enable pass diagnostics for all decisions on this item |

> **Removed 2026-08-06:** the `#gpu` / `#?gpu` / `#!gpu` directive family and
> the `--gpu-offload` flag are gone — GPU deferral is the `accel` keyword /
> the `--accel` compiler mode (see `14-accel.md`). The `#`-prefix is reserved
> for compiler directives that are *not* performance/offload requests.

Diagnostic output shows the compiler's reasoning at compile time:
```
[my_func] gpu: NOT offloaded (body contains non-GPU-safe intrinsic)
```

## Consumer Metadata (`attr`) in Body Blocks

The `attr key: value;` declaration attaches non-physical consumer metadata
(backend, plugin, tooling) inside type and body blocks. Physical layout is
`spec` (§8.2); behaviour is contracts and operators. `attr` never changes
observable meaning. An optional owner (`attr<llvm> { … }`) names the schema
that claims the keys; the owner is a validation hint — keys are globally
unique.

**Type bodies**: `attr key: value;` declares a consumer property:

```briev
type Foo : Bits {
    spec Bytes: 8;
    spec Alignment: 4;
    attr storage: Native;         // non-physical annotation (SPEC 8.9)
    #volatile;                     // shorthand for attr volatile: true
};
```

**Definition bodies**: `attr` at the top of a function body declares metadata:

```briev
defn process() -> Int {
    attr jira: "FIN-8422";   // Documentation metadata
    attr priority: 2;
    term 42;
};
```

**Guard branches**: `attr` inside a guard block scopes metadata to that branch:

```briev
txn compute [count < N][count == N] {
    [count % 2 == 0] {
        attr priority: 1;
        &even = even + 1;
    };
};
```

> **Removed 2026-11-08:** the `!>` operator is a compile error. Its replacement
> is `attr key: value;` (consumer metadata) or `spec <PascalCase>: value;`
> (physical layout). Module-level `attr` binds to the module.

---

## Observability — `out`, not metadata

A value is live when an FFI call consumes it, so side-effecting intrinsics
(`Print#`, `Malloc#`, `Memcpy#`) are not eliminated. Declarations with external
effects carry the `out` modifier — `observable` is not a metadata key:

```briev
defn print_hello() {
    Print#(42);
};
```

`attr` never opts code out of dead-code elimination; use `out` on the
declaration, or make the value structurally live through `endprogram`.
