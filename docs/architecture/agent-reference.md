# Agent Reference — Language Syntax, Contracts, and Backend Conventions

**2026-07-31:** Reference material moved out of `AGENTS.md` during the
guidelines rewrite (AGENTS.md is now the operating rules; `AGENTS.md.archive`
is the full pre-rewrite document). This file is the day-to-day reference for
Briev language syntax, contract/intrinsic conventions, coding standards, and
backend architecture rules.

---

## 1. Briev Language Syntax

### 1.0 Protocol variants

`#Lh`, `#Rh`, `#T` are compiler-internal positional markers for op bindings —
lexed as distinct tokens and resolved at codegen time to concrete registers.
The **fundamental types** (`Data`, `Bit<N>`, `Int`, `UInt`, `Float`,
`String`, `Bool`, `Char`, `Blob`, `Ptr`, `Void`) are compiler-native
primordials — they appear directly in op signatures (`op Add(Int)`) and
carry no `#`. Parameterized protocol variants (`String<UTF8>`,
`Float<IEEE754>`) keep their `#` and select representations; `#Link<name>`
emits `-l<name>`; `#System` is the sole bare protocol hashword. `Data` is
the universal reflective floor (every value observable as raw storage — not
a supertype, no universal inheritance edge); `Bit<N>` is the unified bit type
at any declared width (`Bit` bare = flexible); `Blob` is the `[len][bytes]`
byte buffer. See `docs/plans/2026-08-15-fundamentals-as-types.md`.

**Width resolution** (for `WidthParametric` fundamentals `Int`, `UInt`,
`Bit<N>`): `!> bits: N` (exact) → `!> maxbits: N` (upper bound) →
`!> minbits: N` (lower bound) → `int_bits` (target default). `!> bits: 32`
asserts the type is exactly 32 bits on every target — a hard contract, not
a hint.

Well-known sub-protocols are hardcoded in the casting graph with known LLVM types
(these remove the old `disamb` metadata hack):

| Variant | LLVM type |
|---------|-----------|
| `Float<BFloat>` | `bfloat` |
| `Float<Half>` | `half` |
| `Float<IEEE754>` | `float` |
| `Float<Double>` | `double` |
| `Float<FP128>` | `fp128` |
| `Float<X86_FP80>` | `x86_fp80` |
| `String<UTF8>` | `ptr` (to `[len][bytes]`) |
| `String<ASCII>` | `ptr` (to `[len][bytes]`) |

The file extension determines the default variant (`.bv` → UTF8, `.ebv` →
ASCII); cross-variant calls need explicit disambiguation. If the compiler must
distinguish two representations of the same width, add a hardcoded protocol
variant — never a metadata key that codegen must check.

### 1.1 Naming convention

- **PascalCase**: fundamental types, protocol identifiers, intrinsics
  (`String`, `Float`, `Bit<N>`, `String<UTF8>`, `Float<IEEE754>`, `Sqrt#`,
  `Print#`, `Posit32`, `CastTo(String<UTF8>)`).
- **snake_case**: user functions in `.bv` files and Rust stdlib calls
  (`ascii_to_utf8()`, `from_utf8_lossy()`, `array_map()`).
- The dividing line: if the compiler MUST know the name to function (intrinsic
  registry, fundamental types) it is PascalCase; if a user could rename it and
  the compiler still works it is snake_case.

### 1.2 `<-` arrow operator (2026-08-01, Phase 3)

Statement-level only. The arrow has no `&` marker — the dispatch finds the
collection by the **op binding on each side** (InsertAt on the lhs = insert;
ExtractFrom/CopyFrom on the rhs = read or destructive extract):

- `collection <- value;` — **insert** (push) into the collection
- `dest <- collection;` — **read** (copy) an element out of the collection
- `dest ~<- collection;` — **destructive extract**: copy, then destroy the
  source's backing (`Expr::Consume` on the source)
- `<- value;` / `~<- value;` — **read discard** / **destructive discard**
  (target None)

All arrows are `Statement::ArrowAssign { target, value, consume }`. The element
type of InsertAt/ExtractFrom is generic-substituted (`List<Int>` push's `T` →
`Int`). See `src/typechecker/mod.rs` (`push_element_type` /
`extract_element_type`) and `src/backend/llvm/emit_stmt.rs`.

### 1.3 `&` — address-of (pointer ref)

`&expr` is genuine address-of (`Ptr<T>`, or `Ptr<const T>` for an immutable
local). It never appears in arrow syntax — the old `&` fake-pointer marker is
gone.

### 1.3.1 Consumptive operators (`~op`, Phase 3)

`~` prepended to a binary operator consumes the RHS after the op:
`a ~= b` (move-assign), `a ~+ b`, `a ~- b`, `a ~* b`, `a ~/ b`, and the arrow's
`dest ~<- src` / `~<- src;`. Unary `~x` stays bitwise NOT. Only a **mutable
lvalue** can be consumed (`~op` on a constant is a compile error); reading a
consumed local is a **use-after-move compile error** (the move pass); a
reassignment revives it. The consumed backing is freed at the statement boundary
via a strategy-aware free (`emit_destroy_register`). The dead `~?`
(temporal-fallback) token is removed; `~/` (term-until) is now the consumptive
divide. "Until this holds" contracts use the `[!/X]` / `[!/!X]` invert form.

### 1.3.2 Stream symbols (Phase 4)

`#StdOut` / `#StdErr` / `#StdIn` are compiler-known stream symbols:
- `#StdOut <- value` — writes any value (lowered to the generic `Print#`).
- `#StdErr <- <String>` — writes a String to stderr (`__eprint_str`).
- `#StdIn` — a `Ptr<Int>` stream-handle value (the trg read composition).

### 1.3.3 Lifetime hints (Phase 5)

`free x;` — a VERIFIED contract: `x`'s backing is freed here; a later read is a
use-after-free compile error; the garbage scheduler excludes `x` from its
auto-free. `keep x;` — SUPPRESS the scheduler's auto-free of `x`; a `keep` on a
field the scheduler would not free anyway is a redundant-keep warning.
`brievc memcheck <file.bv>` reports the scheduler's per-field decisions.
See `docs/plans/2026-08-01-free-check.md`.

### 1.3.4 Triggers (Phase 4)

A trigger is the whole-target form `trg name @ instance;` — the `.port` suffix
is removed. `@ link sym` binds an external runtime symbol.

### 1.4 `frgn` is an import

First name after `frgn` is the C/foreign symbol, `as` gives the Briev name.
`from` is required. `from "libruntime"` is forbidden — use `from "c"` or
`from "link/briev_rt.c"`:

```briev
frgn XXH64(data_ptr: Int, len: Int, seed: Int) -> Int as frgn__xxh64 from "link/xxhash/xxhash.c" fallback 0;
```

### 1.5 Lexer / parser gotchas

- `>>` in nested generics: `Ptr<Ptr<Int> >` (space required).
- `_` discard binding: `let _ = expr;` also works in tuples
  (`let (_, value) = get_pair();`).
- Imports are flat — no `::` module paths. `loader::read_u8(x)` is invalid.
- `import "foo.bv"` is file-relative to the importing file; `"<foo>"` is a
  registry lookup.
- Tuples are heap-allocated (`(1, 2)` calls `@malloc`); SROA promotes small
  tuples in optimized builds.
- `Byte` is defined in `lib/std/types.bv` — import it or use `Int`.
- `defn main()` and bare top-level `let`/`const` bindings run via the
  flat-scripting plugin (2026-08-01): a synthesized one-shot
  `node __script_main [__script_done == false][__script_done]` executes them
  exactly once. Reactive programs start via state-space triggers on `node`
  declarations; CLI subcommands use `entry!`/`args!`.

### 1.6 Import / narrowing

- `Int` narrowing is fundamental-based (`Int`/`UInt` membership via the
  parent chain, never type names). Fixed-width types (`Int8`…`Int64`) cap
  the floor via `!> bits: N`.

### 1.7 Types

- `type Foo: List { … }` inherits, but `Foo<Int>` is NOT automatically
  assignable to `List<Int>` — projections like `.#Size`/`foo[i]` may fail.
- No implicit `Copy` on enums containing `String` (`InsertStrategy::Custom(String)`
  requires removing `Copy`).

---

## 2. Contracts

1. **`defn` needs no contract** — straight-line translation is inherently
   provable. Add contracts only for the optimization leverage they unlock.
2. **`txn` needs at least one contract side** — `[pre][post]`, `[pre]]`, or
   `[[post]`. Convergence must be provable.
3. **Intrinsics have no body** — `Sqrt#(x)` is never declared in source; the
   compiler knows it via `get_intrinsic_signature("Sqrt#")`.
4. **`[true][true]` is rejected** — use `[[post]` or `[pre]]` sugar.
5. **`[[post]` = `[true][post]`** (postcondition-only);
   **`[pre]]` = `[pre][true]`** (precondition-only).
6. **Single-bracket `[expr]` is ambiguous** — parser rejects it.
7. **`[true][term == true || term == false]` is a useless tautology** — write a
   contract that constrains behavior.
8. **Never weaken contracts** — never change `[product > 0]` to `[true]`.
9. **Watchdogs** (`?[...]`/`![...]` after the postcondition) are liveliness
   contracts: the loop continues while the condition holds and fires when it
   stops. The `-> handler(val)` callback receives the last computed value.
   The `within N <unit>` clause (`ms`/`seconds`/`minute`/`cyc`) adds a deadline
   — the fire happens even if the condition never stops holding (via the `Now#`
   monotonic clock or a cycle counter). See
   `learn-briev/02-contracts.md` §7.

### Intrinsic conventions

- PascalCase + `#` suffix: `Sqrt#`, `Malloc#`, `Print#`, `GetEnvInt#`. The
  `#` is part of the identifier lexically. No `_` prefix convention.
- No `inop` keyword — all compiler-known ops are `#` intrinsics with entries in
  `get_intrinsic_signature()` and `execute_intrinsic()`.
- Side-effecting intrinsics MUST declare `!> observable: true` (`Print#`,
  `Malloc#`, `Memcpy#`, …) so DCE cannot eliminate the call.

### The `Print#` convenience intrinsic and the `println!` macro (2026-08-01)

`Print#` is a single generic intrinsic that dispatches the emission by the
argument's **fundamental category** (resolved via `type_to_protocol`, the
`Cast.*` universe properties — never type names): `String` →
`__print_str(ptr)`, `Float` → `__print_float`/`__print_float64`,
`Char` → `__print_char`, `Bool` → `__print_bool` (prints `true`/`false` — a
Bool's natural representation; an explicit `(b as Int)` cast yields
`1`/`0`), else `__print_int`. It replaces the four special-cased
`PrintInt#`/`PrintFloat#`/`PrintStr#`/`PrintChar#` intrinsics.

`print!`/`println!` are **macros** (`print_plugin.rs`): their added value is
format-string argument insertion and (for `println!`) line termination, not
printing. The newline is a Char literal `Print#('\n')`. `PrintChar#` was folded
away.

`Char` is a first-class fundamental (`Cast.Char`): char literals are a distinct
`Expr::Char`, typed `Char`, emitted at their native i32 width (state fields
and cast results match); the interpreter carries `Value::Char`/`Value::Bool`.
`Bool` values are `Value::Bool` in the interpreter (comparisons and `Expr::Bool`
produce them) so `Print#(a < b)` prints `true`, matching the backend.


---

## 3. Guard / Control-Flow Forms

- `[expr];` — **convergence gate** (`Statement::Gate`): compile-time assertion;
  at runtime, if false, branches back to the convergence target.
- `[expr] stmt;` — **guarded single statement**.
- `when expr { body };` — **block guarded body** (preferred for multi-statement
  guards). Guards may chain; a trailing `{ body }` without `when` is the else.
- `[cond] { body }` is **rejected** — use `when cond { body }`.
- Post-body loops `{ body; i = i + 1; } [condition];` only work in `txn`/`node`,
  NOT `defn`.

### Iteration pattern

Iteration requires `txn` with `[pre][post]` convergence, NOT `defn` + `[guard]`
(`Statement::Guarded` is a one-shot conditional):

```briev
txn iter_map<T, U>(list: List<T>, f: T -> U, result: List<U>, i: Int)
    [i < list.^Len][i == list.^Len] -> List<U>
{
    result = result.append(f(list[i]));
    i = i + 1;
    term result;
};

defn iter_map<T, U>(list: List<T>, f: T -> U) -> List<U> {
    term iter_map_loop(list, f, [], 0);
};
```

| Construct | Semantics | When to use |
|-----------|-----------|-------------|
| `defn` | Pure function, straight-line | Stateless computations, wrappers |
| `txn params [pre][post] -> Ret` | Callable convergent loop | Iteration, accumulation, recursion |
| `node [pre][post]` | Reactive, reactor-driven | State machines, event-driven |
| `[guard] { body }` | One-shot conditional | If/else inside a `txn` body |

### `type` vs `struct` vs `obj`

- `type`: protocols, operator bindings, type extensibility
  (`type MyInt: Int { op Add(Int); };`).
- `struct`: pure data, fixed layout, C-compatible, no methods/contracts
  (`struct VMStack { data: Int[1024]; len: Int; };`). Receives fixed-size
  arrays and the bracket SIMD syntax.
- `obj`: full-featured types with methods, contracts, type params, visibility.

### Physical layout (2026-08-13, Deferred Layout)

Deferred Layout's story term is **Boxed Cat Typing** — a Schrödinger's cat pun:
a type's representation is indeterminate ("in the box") until a backend
materializes it or `pack`/`seq`/`spec` pins it. It is NOT literal i64 boxing;
the register boxes in `backend-strategy.md` are coincidental.

Physical layout is DECLARED, never assumed: a type carries protocol + metadata
only, and the backend derives the representation at materialization time.
`spec <PascalCase>` is the canonical spelling for the five physical keys
(`spec Bits/MaxBits/Bytes/Alignment/Endian`); the legacy `!> bits:`/`!> maxbits:`
etc. write the same lowercase keys and stay readable. Three layout modifiers on
struct declarations:

- `pack struct` — bit-contiguous, zero padding; sub-byte `Bit<N>` fields slice
  out of a byte array (`{ [N x i8] }`), whole-byte fields use `<{ ... }>`;
  `spec Endian: Big` couples bit order (MSB-first) to big-endian multi-byte.
- `atomic <field>` — the field reads/writes atomically (`load atomic`/`store
  atomic`, `obj.f = obj.f + c` → `atomicrmw add`).
- `union Name { … }` — untagged overlay; all fields at offset 0, size = largest
  aligned field storage.

The single authority for packed offsets is `src/type_universe/packed.rs`
(`packed_field_offsets`); the casting graph resolves every LLVM type
(`resolve_llvm_type`), never a name match. `Bit<N>` is exactly N bits in both
AST forms (`Type::Bits(n)` and the `Applied("Bit",[n])` alias). `Bit` bare is
flexible width (resolved later); there is no separate `Bits` type.

### Bracket arrays / SIMD

`Int[1024]` is a compile-time fixed array (`[1024 x i64]` in LLVM). Slice
syntax `arr[start:end:stride]` (any component optional); `arr1 + arr2`,
`arr * 2` on `Vector<T, N>`/`Slice<T>`; contiguous slice lvalues use `memcpy`,
strided use element loops (LLVM vectorizes); `raw as Byte[8192]` is a
zero-copy view cast (validates `N * sizeof(T) == M * sizeof(U)`). `map`,
`filter`, `fold`, `any`, `all`, `sum`, `product` are regular txns in
`lib/std/array.bv`, vectorized via the `[i < N]` convergence contract.

> **2026-09-06 (plan 2026-09-06-cpp-expressiveness.md).** Pointer
> arithmetic + portable SIMD + atomic ordering:
>
> - **Pointer intrinsics** (SPEC §14.2): `PtrAdd#(p, n)`/`PtrSub#(p, n)`
>   (element steps, `getelementptr inbounds`), `PtrDiff#(a, b)`
>   (same-allocation element distance), `PtrEq#`/`PtrLt#` (handle
>   comparison). The Briev pointer ABI is uniformly BOXED i64 handles —
>   `&` ptrtoints, `Malloc#` boxes, `as Ptr<T>` retypes without changing
>   the SSA value — so these intrinsics unconditionally `inttoptr` the
>   base and re-box (the atomics' convention). No `ptr + int` operators.
> - **Portable SIMD** (SPEC §15.4): `SimdAdd#/SimdSub#/SimdMul#/
>   SimdFma#(dst, a, b[, c], count)` — memory-to-memory element-wise
>   family. `Type::Vector` lowers to `[N x T]` LLVM arrays, so SSA
>   vector registers cannot escape; the memory-to-memory ABI is the
>   honest form. Chunking is overlap-safe (loads precede the store
>   per chunk) — dst may alias sources, the case the auto-vectorizer
>   declines. The TYPECHECKER gates `Ptr<scalar>` pointees (universe
>   entry with no fields); the emitter derives the shape from storage
>   size + float bits. Runtime counts use alloca induction variables
>   (never phis from an unlabeled current block).
> - **Atomic ordering** (SPEC §8.2): context-sensitive keywords
>   `relaxed`/`acquire`/`release`/`bartered` before `atomic`
>   (`bartered atomic refs: Int;`), or as trailing atomic-intrinsic
>   args (`AtomicLoad#(p, relaxed)`). `seq` is the default (reused
>   strategy keyword). New RMW intrinsics: `AtomicSub#/Or#/And#/Xor#`,
>   width-parameterized `AtomicLoadN#/AtomicStoreN#`.
> - **ISR handlers** (SPEC §13.2, plan 2026-09-06-isr-handlers-and-sections.md):
>   `isr[<mechanism>] handler @ (literal | Name): name() [pre][post] { … };`
>   — mechanism registry in config/isr-targets.dbvl (explicit → profile
>   `isr_mechanism` → error with both fixes); named vectors via the board's
>   interrupts.dbvl; body restrictions proven at compile time (no
>   alloc/spawn/float-per-fpu_context); vector tables + default spin handler
>   emitted per mechanism; ISR programs share state through the global
>   `@__briev_state` (emit_state_base aliases it at every former alloca
>   site).

### `op Parse` discriminators

`op Parse(Decimal, pre:"0x")`, `suf:"km"`, `reg:"[0-9a-fA-F]+"`,
`op Parse(Quoted)`, `op Parse(Bare)`. Resolution order: form → pre/suf →
regex; ambiguity = error. `sql"SELECT"` → `Expr::TaggedQuotedLiteral`;
`42km`/`3.14f` → `Expr::TaggedLiteral`.

---

## 4. Modifiers and the concurrency gate (2026-07-31)

User-facing directives are **ordinary keywords** (no `#`); they **must never
make code faster** — a modifier-beaten default is a compiler bug (Golden Rule
2 "MAXIMUM EFFICIENT DEFAULT", AGENTS.md; also §6). All modifiers
are **prefix** (`async node`; `node async` is rejected). See
`docs/architecture/concurrency-and-modifiers.md`.

| Modifier | Meaning |
|----------|---------|
| `seq struct Name` | declared field layout preserved — `apply_field_modes` does not reorder/compact/eliminate |
| `seq txn foo` / `seq node foo` | sequential dispatch — never the parallel reactor |
| `seq Int[N]` / `seq foreach` | sequential access — `!llvm.loop.vectorize.enable = false` |
| `vol let x` | every access is `load volatile`/`store volatile` |
| `out defn foo` / `out node foo` / `out txn foo` | the callable's calls are liveness roots — the compiler must not eliminate them (the stdlib-side twin of the intrinsic `observable: true` flag); the body is still fully optimized, only the call boundary survives. Direct-only: a pure function calling an `out` function is not itself pinned. |
| `out let x` | the variable's reads/writes are liveness roots (never eliminated); does NOT force volatile memory semantics. `vol` implies `out`; `out vol let x` is legal. |
| `async node foo` | explicit acknowledgement of simultaneous firing (not a hint) |
| `sync<group> node foo` | group barrier — members that fire hold off finishing until all fired members have |

**The concurrency gate (NO IMPLICIT CONCURRENCY):** for reactive nodes A and B,
if the proof engine proves `pre_A ∧ pre_B` satisfiable AND there is no XOR
read-write overlap, the compiler DEMANDS `async` on both or `sync<group>` on
both — an unclassified eligible pair is a hard error.

**Delimiter semantic load:** `<>` = compile-time type specialization
(`Stack<T>`, `String<UTF8>`, `asm<chip>`, `sync<group>`); `()` = application &
binding (`f(a)`, `Person(...)`, `op Add: func(#Lh,#Rh)`, `op Add(Float)` —
declarations take params); `[]` = containment/bound; `{}` = grouping.

## 5. Coding Standards (details)

### Doc comments on every definition

Every `fn`/`struct`/`enum`/`trait`/`type`/`const`/`mod` needs a `///` comment
explaining intent, invariants, usage. Write for a reader who knows Rust but not
the domain. Non-negotiable — reject in review.

### Input validation & defensive checks

- Check array/vector bounds before indexing.
- Assert struct invariants after construction/mutation.
- Print diagnostic context (function, values, expected vs actual) on failure.
- Check NaN/Inf at FFI boundaries.
- `debug_assert!` on hot paths; `assert!` for safety-critical invariants.

### Need-to-know dependency injection

Pass only the data a function needs, not large context structs.

```rust
// Avoid:  fn emit_binop(ctx: &CompilerContext, state: &State) -> Result<()>;
// Prefer: fn emit_binop(builder: &mut LlvmBuilder, op: BinOp, lhs: Type, rhs: Type) -> Result<String>;
```

### Metropolitan FFI / export

- `briev export` generates wrappers from `lib/glue.toml` templates — no Rust
  knows specific languages. GLUE = compile-time bridge; Metropipe =
  runtime shared-memory IPC (`src/ffi/metropipe.rs`).
- `briev export` calls `LlvmBackend::generate()` — the same path as
  `briev build --llvm`. No `ret i64 0` stubs.
- Strings in LLVM: `[i64 length][data\0]`; globals use `<{ i64, [N x i8] }>`;
  `emit_load_length` reads `handle[0]`; `briev_str_to_c` strips tag bits `& ~3`.
- Protocol paths via BFS (`find_cast_path()` from `layout_optimizer.rs`);
  fall back to `Cast(Bit<N>)`; `emit_protocol_chain()` emits real IR.

### HashMap iteration determinism

Every HashMap iteration that produces LLVM IR MUST be sorted by key — SipHash
seed differs per process (up to ~9% perf variation). Applies to
`field_index_map`, `phi_field_regs`, `backedge_field_regs`, `last_val_temps`,
`done_needs_fields`, `pending_phi_backedge`, `pending_phi_native_backedge`,
`vector_phi_groups`, `vector_phi_current`, etc. HashMaps used only for O(1)
lookups are fine. Reference: commit `139c345`,
`docs/plans/2026-07-06-ir-determinism-and-benchmark-strategy.md`.

---

## 5. Anti-Patterns (NEVER DO)

- Changing `[product > 0]` to `[true]` because code doesn't set product
- Generic contracts like `[true]`; postconditions that don't guarantee outcomes
- Rust string-match built-ins when stdlib/import should be used
- Pre-populating interpreter state with enum constants (None, Some, Ok, Err)
- `x == x` self-references to force liveness; synthetic exit-condition fields
- Hardcoded `from "libruntime"` (use `from "c"` / `from "link/briev_rt.c"`);
  missing `from` on `frgn`
- `#export` (use `export defn`); `#out` (use the `out` keyword)
- Hardcoded runtime declares (`__rt_init` must be `frgn` in `std/rt.bv`)
- Name-based interpreter dispatch (dispatch on `Value::HashMap`, not names);
  `"None"`/`"Err"` discriminant magic (use declaration order); runtime type
  tags for dispatch
- Implicit coercions — all type reinterpretations explicit via `as`
- Dynamic optimization path switching — choose layouts at compile time
- Transitive compatibility inference — declare each compatibility explicitly
- Weakening existing optimization paths — new match arms only
- Blaming regressions on "system noise" / "HashMap iteration order" without a
  controlled A/B experiment (old vs new compiler, full suite, same machine)
- Old-style `Expr::Add`/`Expr::Mul` matches without
  `expr.normalize_to_old()` first — silent wrong output otherwise
  (`try_eval_cfloat` returning `None` for `4.0 * pi * pi` → `constant float 0.0`)

---

## 6. Optimization Philosophy

### The maximum-efficient default (foundational)

The compiler MUST pick the most efficient codegen strategy for every program
automatically — every case, not just the benchmark at hand (Golden Rule 2,
AGENTS.md). This covers not just modifiers but every codegen decision: tuple
slot allocation, collection strategy, probe cost, materialization, loop shape.
A heuristic beaten by a strategy keyword on the *same program* is a compiler
bug — fix the default, never require the user to reach for a keyword to be
competitive. Strategy keywords exist to express **intended behaviour** that
plain efficient codegen cannot deliver (embedded/inter-language semantics,
precise declaration order, volatile memory, sequential execution) — never to
win on speed. A benchmark whose efficient path requires a modifier or a
non-idiomatic program shape is surfacing a default-codegen gap, not a valid
comparison.

### Emergent optimization — performance flows from analysis (2026-09-15)

Kernel fusion is an **emergent property of the analysis**, not a hand-coded
pattern. The analysis already holds the evidence — a RAW chain, dead
intermediates (single reader), an elementwise middle, a terminal output — and
the fused shape *falls out* of it: "this intermediate never needs to exist in
HBM; the consumer consumes it on-chip." The scheduler records a **chain
topology**, never operand names; the GEMM operands are derived at codegen
from the node shapes. Any `GEMM → elementwise → GEMM` chain qualifies — not
one named pattern (`no `FusedAttention`, no `q_field`/`kt_field`).

Three standing rules for this and future work:

1. **Generalize, never for purity.** Structural detection (a foreach
   accumulation over two arrays; an elementwise middle) is general without
   being vague. A future middle (softmax row-op, bias-add) is the *same*
   mechanism — the codegen learns the op, never a new pattern.
2. **Never at the cost of performance.** A fused kernel is not a
   correctness-only toy: it reuses the tuned machinery (mma, cp.async
   panels) and must be competitive with the composition. The analysis picks
   the best of the available shapes (3-kernel, 2-kernel, 1-kernel) from its
   own evidence — if the on-chip tile does not fit the budget or the recompute
   would lose, the efficient shape is the smaller fusion, automatically.
3. **Performance emerges, it is never decorated.** The most efficient fused
   shape is the automatic default when the analysis proves it; no keyword
   makes a fusion appear, and a benchmark beaten only by a keyword is a
   default-codegen bug. The analysis's proofs ARE the fusion's justification.

### Long-term best optimization

Emit the IR that produces the BEST FINAL CODE after LLVM's full pipeline
(SROA + GVN + DSE + LICM + vectorizer), not the cleanest-looking IR. Prefer
patterns LLVM recognizes (phi + icmp + add induction, extractvalue/insertvalue
struct decomposition). Check `opt -O3 -S unopt.ll` and count remaining
instructions — that is the true cost.

### Regression prevention

Every optimization decision must leave a comment: what pattern it targets, what
it gains (benchmarks, expected improvement), what it costs (IR bloat, compile
time, edge cases), why the trade-off is optimal, and what breaks if removed.
Before every commit: does this affect an existing optimization path? If yes,
verify it still fires (IR + benchmark), update comments, run tests AND
benchmarks. The cost of a missed optimization is measured in months.

### Regression watch / trade-off analysis

- Consider ALL code paths, not just the target. Identify the pattern, when it
  would hurt, and eliminate trade-offs by detecting-and-branching in the
  compiler when runtime detection is possible.
- Consult `docs/plans/`, `docs/architecture/`, `BUGS.md`, `git log` before
  attributing a regression. Never blame "noise" without a controlled A/B.
- Benchmark both paths before/after vs C; document the cost when a trade-off is
  kept.
- When a heuristic chooses a codegen strategy, record which strategy was chosen
  per transaction (a `bool` on `LlvmBackend` + `report_lines`) so regressions
  are diagnosable.

---

## 7. Backend Architecture Rules

### Context stratification (three lifetimes)

1. **CompilerContext (global)** — read-only during codegen: AST defs, FFI
   signatures, target specs, layout.
2. **FunctionContext (per-function)** — local variables, types, SSA register
   counter; must never outlive the function.
3. **LLVMBuilder (instruction builder)** — the sole writer of IR; raw
   `writeln!` formatting of standard instructions is forbidden.

Rules: no global-state pollution (no function-scoped transient vars on the
backend struct); all registers via `builder.gen_reg()` — no manual
`format!("%t{}", counter)`.

### Defensive codegen

- No untyped casts — every coercion goes through a centralized conversion
  helper.
- Unique temp filenames for `llc` (process/thread IDs) to avoid parallel
  collisions.
- Validate pointer-tagging assumptions (mask off low 2 bits of string ptrs)
  against target alignment.
- Every foreign function has an explicit LLVM declaration; resolve C-vs-LLVM
  return-size mismatches (bool/i32) with trunc/zext to avoid ABI register
  corruption.

### Dual-path / adaptive optimizations

When a feature has two implementations each better under different workloads,
support BOTH with a static compile-time decision tree (e.g., stack/arena vs
heap; folded/O(1) vs vectorized loop; enum/switch vs sequential reactor).

### Backend integration contract (2026-08-23)

Normative: `docs/architecture/backend-contracts.md`. Headlines:

- AnalysisResults computed ONCE in the pipeline; backends consume it.
- Partial-surface backends declare `CAPABILITIES`; the pipeline rejects
  out-of-surface programs BEFORE codegen. LLVM's full-surface claim must
  stay true (the Statement::Match regression proved why).
- Emission-time gaps go to a backend error accumulator
  (`VmBackend.errors`, `CirctBackend.errors`) -> pipeline hard error.
- Determinism law: any HashMap iteration feeding output or program
  rewriting is sorted/BTreeMap.
- Per-backend emission invariants (SPIR-V typed-builder-only, CIRCT
  wire-map + annotations-before-types, VM .lair absolute addressing) are
  documented with their failure histories in backend-contracts.md §3–§7.

### Capability matrix (required integration shape)

`src/backend/capabilities.rs` — `BackendCapabilities` struct declares which
AST constructs a backend's codegen actually emits. `validate_for_backend()`
walks the AST before codegen and rejects out-of-surface programs with
house-style diagnostics (what / why / fix).

When adding a new backend or extending an existing one:
1. Flip the relevant flags in the backend's `capabilities()` declaration.
2. Add tests for the new constructs.
3. A flag set beyond real coverage produces silent drops (the bug this
   module exists to prevent).

LLVM is full-surface (`capabilities()` returns a struct with all flags `true`);
partial-surface backends (VM, SPIR-V, CIRCT, Webstack) declare only what
they lower. See `docs/architecture/backend-contracts.md` §3–§7 for emission
invariants per backend.

### Frontend constructs are abstract — backends give meaning

| Construct | Universal meaning | LLVM | SPIR-V | CIRCT |
|-----------|------------------|------|--------|-------|
| `sync(d) {}` | Atomic exec + sync | Txn ordering | `OpControlBarrier` | Handshake stall |
| `txn` | Convergent state loop | Phi + br | Work-item loop | Clock cycle |
| `let x` | Named binding | Stack/register | Register | Wire |
| `[pre][post]` | State convergence | Branch invariants | Guard predicates | Setup/teardown |

Before adding a `#` intrinsic, check if a frontend construct already carries
the semantics (`Barrier#()` was wrong — `sync` already means synchronize).

### Dead backends — zero fixes

`verilog.rs`, `vhdl.rs`, `c.rs`, `rust.rs`, `cobol.rs`, `x86_64.rs`,
`aarch64.rs`, `wasm.rs`, `tcl_generator.rs`. If a shared API change
mechanically breaks them, use `#[allow(unused_variables)]` / `_ => {}` /
`todo!()` with a `// dead backend` comment — do not implement the feature.

---

## 8. Commenting Mandate (Backend Updates)

**Never delete rationale comments when refactoring.** Every rationale comment is
institutional memory — rewrite it to explain the new structure, never delete it
silently. Every backend code change must include a comment at the site:

```
// YYYY-MM-DD: <short description of why this exists>
// <what problem it solves, what pattern it targets>
```

Trade-offs (faster path A but slower path B) must be documented with why the
chosen approach is optimal for the targeted situation.

---

## 9. Compiler Registry

`~/.briev/registry/` (or `dirs::data_dir()/briev/registry/`) is the per-user
directory for installing Briev modules and foreign sources. Managed by
`brievc registry {add,list,remove}`:

- `brievc registry add ./my-lib.bv` — copies the file (version-locked, no symlink)
- `brievc registry add ./xxhash/ --name xxhash` — copies a directory tree
- `brievc registry list` — enumerates contents
- `brievc registry remove <name>` — deletes the matching entry

Lookup order for `import <name>` / `from <name>`:
1. Project-local `.briev/registry/<name>` (if it exists)
2. User-wide `~/.briev/registry/<name>`
3. `config/module-registry.dbvl` (for imports)
4. Stdlib path (for `from <name>` and `import <name>` fallback)

See `docs/plans/2026-07-26-tamer-zero-c-and-static-memory.md` §1f.

## 10. LLVM Diagnostic Commands (when optimizer fails)

```bash
# SROA failures (struct not decomposed into scalars)
opt -O3 -pass-remarks-missed=sroa unopt.ll -disable-output 2>&1
# Loop vectorization failures
opt -O3 -pass-remarks-missed=loop-vectorize unopt.ll -disable-output 2>&1
# Alias analysis / GVN failures
opt -O3 -pass-remarks-missed=gvn unopt.ll -disable-output 2>&1
# All optimization remarks at once
opt -O3 -pass-remarks-missed=sroa,gvn,licm,loop-vectorize unopt.ll -disable-output 2>&1
# Inspect IR before/after
opt -S -O3 unopt.ll -o opt.ll
diff <(grep -v '^;' unopt.ll | grep -v '^$') <(grep -v '^;' opt.ll | grep -v '^$')
# Check if %State struct survived SROA
grep '%State' opt.ll
```
