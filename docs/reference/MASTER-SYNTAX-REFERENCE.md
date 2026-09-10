# Briev — Master Syntax Reference

**Date:** 2026-09-08
**Authoritative sources:** `src/lexer.rs` (Token enum), `src/vocab.rs`
(`LanguageVocab::canonical()`), `src/intrinsic_signatures.rs`
(`get_intrinsic_signature`), `docs/architecture/hash-words.md`.
**This document is generated FROM CODE, not from the stale reference docs.**
If a feature is missing here but present in code, it is a bug in this document
(the `code → doc` completeness test in `src/vocab.rs` catches it).

**Companion indexes:** [BACKEND-SUPPORT-MATRIX.md](BACKEND-SUPPORT-MATRIX.md)
(which backend implements what) · [LEGACY-SURFACE-INDEX.md](LEGACY-SURFACE-INDEX.md)
(removed syntax still in the codebase).

---

## 1. Reserved keywords

Lexed as dedicated tokens (`src/lexer.rs:44-311`). Cannot be user identifiers.

### Declarations & type system

| Keyword | Meaning |
|---|---|
| `spec` | Physical-layout metadata (`spec Bits: 64;`) |
| `export` | Visibility / re-export |
| `defn` | Function definition |
| `let` | Local variable / state field binding |
| `const` | Compile-time constant |
| `txn` | Transaction (reactive state mutation) |
| `node` | Reactive node |
| `op` | Operation declaration |
| `type` | Type alias / fundamental extension |
| `trait` | Protocol trait |
| `impl` | Implementation block |
| `cell` | Cell declaration |
| `obj` | Object (collection/instance) declaration |
| `struct` | Struct declaration |
| `enum` | Enum declaration |
| `render` | Rendered Briev declaration |
| `union` | Untagged overlay (fields share storage at offset 0) |
| `coll` | Collection strategy keyword (compiler-owned Length semantics) |
| `meld` | Staged/removed structural-equivalence declaration (lexed, parser rejects) |
| `reg` | Register-file array-lowering pin |
| `extern` | Foreign HDL module import |

### Reactive / program structure

| Keyword | Meaning |
|---|---|
| `async` | Explicit simultaneous-firing acknowledgement |
| `await` | Await a spawned task |
| `spawn` | Spawn a task |
| `trg` | Trigger declaration |
| `within` | Watchdog deadline prefix |
| `term` | Transaction/node termination (result) |
| `endprogram` | Program completion |
| `beginprogram` | Program entry |
| `rollback` | Abort transaction |
| `defer` | Cleanup block (runs on term/rollback/endprogram) |
| `mutex` | Serial section |
| `barrier` | Group barrier block |
| `sync` | Group synchronization modifier (`sync<group>`) — bare `sync { }` block deprecated (§13, use `mutex { }`) |

### Storage / layout / concurrency qualifiers

| Keyword | Meaning |
|---|---|
| `seq` | Ordering/sequential modifier; sequential-consistency atomic default |
| `pack` | Bit-contiguous zero-padding struct modifier |
| `trap` | Hardware abort (never-type) |
| `halt` | Bare-metal stop (never-type) |
| `atomic` | Per-field atomicity modifier |
| `relaxed` | Atomic ordering: relaxed |
| `acquire` | Atomic ordering: acquire |
| `release` | Atomic ordering: release |
| `bartered` | Atomic ordering: acq_rel |
| `vol` | Volatile memory modifier |
| `mem` | Memory-macro array-lowering pin |
| `out` | Observability / liveness-root modifier |
| `accel` | GPU-deferral modifier |

### Import / FFI

| Keyword | Meaning |
|---|---|
| `import` | Module import |
| `from` | Import / FFI source provenance |
| `as` | Rename / cast keyword |
| `frgn` | Foreign function binding |

### Control flow

| Keyword | Meaning |
|---|---|
| `match` | Pattern match |
| `foreach` | Iteration — paren form `foreach (x in list)` deprecated (§13) |
| `break` | Exit nearest foreach |
| `when` | Guarded block |

### Literals & units

| Keyword | Meaning |
|---|---|
| `true` / `false` | Boolean literals |
| `cyc` | Cycle duration unit |
| `ms` | Millisecond duration unit |

### Cell-file keywords

| Keyword | Meaning |
|---|---|
| `input` | Cell parameter port (`.cbv`) |
| `output` | Cell output port (`.cbv`) |

### Compile-time metaprogramming

| Keyword | Meaning |
|---|---|
| `quote` | Quote block |
| `$` | Compile-time stage prefix |
| `$!` | Compile-time forced stage prefix |

### Reserved

| Keyword | Meaning |
|---|---|
| `pvt` | Reserved (unavailable to user identifiers) |
| `sed` | Reserved (unavailable to user identifiers) |

### Internal hash tokens (lexed, not user keywords)

`#Lh` (left operand of `<-`), `#Rh` (right operand of `<-`), `#T`
(generic collection type param), `#Self` (reserved self-reference).

---

## 2. Soft / contextual keywords

Lexed as ordinary identifiers; given meaning in specific parse positions
(`src/parser/`). Legal user identifiers elsewhere.

| Keyword | Recognized in |
|---|---|
| `section` | Placement prefix (`section(".name")`) |
| `optional` | Before `frgn` (`optional frgn`) |
| `proto` | Protocol declaration |
| `asm` | Assembly declaration (`asm<target>`) |
| `isr` | ISR handler declaration |
| `init` | Runtime-seeded invariant / init declaration |
| `$defn` | Compile-time definition |
| `$txn` | Compile-time transaction |
| `$let` | Compile-time variable |
| `$const` | Compile-time constant |
| `free` | Lifetime hint (destroy) |
| `keep` | Lifetime hint (preserve) |
| `variadic` | FFI variadic parameter marker |
| `target` | FFI target clause (`target "c"`) |
| `yield` | Cooperative cancellation |
| `check` | Liveness check |
| `in` | `foreach x in list` binding |
| `dyn` | Trait-object type prefix |
| `box` | Spawn storage class (per-instance heap) |
| `spill` | Spawn storage class (growable buffer) |
| `ns` / `s` / `min` | Duration units after a numeric bound |

### Recognized then rejected (clear error, use modern form)

`fallback` (removed FFI clause), `return` (Briev has no return — use `term`),
`if` / `else` (use `match` / `when`).

### Vocab-only classifications (not parsed)

`program`, `borrow`, `consume`, `owned`, `shared`, `borrowed` — declared in
`vocab.rs` for highlighting/manifest only; no parser recognition.

---

## 3. Intrinsics

Registry: `src/intrinsic_signatures.rs` (115 arms). Call form: `Name#(...)`.
Tagged `[legacy]` = registered/unreachable or emitter-only; see notes.

### Compile-time control

| Intrinsic | Meaning |
|---|---|
| `Error#` | Compile-time failure; message is the diagnostic |

### Arithmetic (shape-inferred, one per operator)

`Add#` `Sub#` `Mul#` `Div#` `Rem#` `Neg#` `Abs#`

### Comparison (→ Bool)

`Eq#` `Neq#` `Lt#` `Gt#` `Le#` `Ge#`

### Bitwise

`BitAnd#` `BitOr#` `BitXor#` `Shl#` `Shr#` (arithmetic) `BitNot#`

### Bit manipulation

`BitReverse#` `Popcount#` `LeadingZeros#` `TrailingZeros#`

### Logical

`Not#` (unary, no short-circuit)

### Pointer

`Deref#` `Index#` `Ptr#` (int→ptr) `PtrAdd#` `PtrSub#` `PtrDiff#` `PtrEq#`
`PtrLt#`

### Collection capacity (compiler-owned hidden `cap` slot)

`Capacity#` `Resize#` `EnsureCap#` `TrimCap#`

### Float math (→ native Float)

`Sqrt#` `Sin#` `Cos#` `Fabs#` `Ceil#` `Floor#` `Exp#`
`Pow#` `[fall-to-external @Pow]`

### Printing / IO

`Print#` — dispatch by argument category (`#String`/`#Char`/`#Bool`/`#Float`/else int)

### Memory

`Malloc#` `Alloc#` `Free#` `Load#` `Store#` `VolatileLoad#` `VolatileStore#`
`Copy#` (memcpy) `Fill#` (memset)

### String & conversion

`Concat#` `[fall-to-external]` `CharCount#` `Length#`
`ToInt#` `[legacy: fall-to-external]` `ToFloat#` `[legacy: fall-to-external]`
`ToString#` `[legacy: fall-to-external]`

### Collections

Legacy: `Get#` `[legacy]` `Insert#` `[legacy]` — use `At#` / `InsertAt#` instead
Generative op-member forms (dispatch to declared `op` members):
`Count#` `At#` `Slice#` `InsertAt#` `ExtractFrom#` `CopyFrom#`

### Generative op identities (callable as `X#`, no registry signature)

`Iter#` `Step#` `IsEnd#` `Current#` `Append#` `Prepend#` `And#` `Or#`

### GPU / SIMT (SPIR-V target; fall to external call on LLVM)

`GetGlobalId#` `GetGlobalSize#` `GetLocalId#` `WorkgroupSize#` `GetGroupId#`
`GetNumGroups#` `Dims#` `SubgroupFAdd#` `Barrier#`

### Process & environment

`Spawn#` `SpawnWithOutput#` `SetEnv#` `GetCwd#` `ChDir#`

### Compile-time address

`AddressOf#` — resolve named device via `config/address-map.dbvl`

### Callbacks

`CallPtr#` — variadic host function-pointer call

### Host cancellation

`CancelRequested#` `ClearCancel#`

### Raw OS

`SysCall#` (variadic, inline syscall asm or `@briev_syscall`)
`SysConf#` (POSIX sysconf)

### Atomics (trailing order word: `relaxed`/`acquire`/`release`/`bartered`/`seq`)

`AtomicLoad#` `AtomicStore#` `AtomicCas#` `AtomicXchg#` `AtomicAdd#`
`AtomicSub#` `AtomicOr#` `AtomicAnd#` `AtomicXor#` `AtomicLoadN#`
`AtomicStoreN#`

### Portable SIMD (memory-to-memory element-wise)

`SimdAdd#` `SimdSub#` `SimdMul#` `SimdFma#`

### Fence

`Fence#`

### Dynamic linker

`DlOpen#` `DlSym#` `DlClose#`

### String / env / sys query

`StrSplit#` `EnvGet#` `SysQuery#` `TimeNow#`

### External I/O

`HttpFetch#` `ShellCmd#`

### Debugging

`Backtrace#`

### Emitter-only / internal (not in the user-callable registry)

| Intrinsic | Status |
|---|---|
| `Cast#` | `[legacy]` compiler-internal cast pipeline; `(Type)expr` lowers to `Expr::Cast`, not a call |
| `GetEnv#` | `[legacy]` removed; use `get_env!` |
| `GetEnvInt#` | `[legacy]` removed; use `get_env_int!` |
| `TaskCall#` | async segment dispatch: threads the machine's state into a segment fn (Family H machine) |
| `Len#` | `[legacy]` alias; use `Length#` |
| `Now#` | `[internal]` monotonic clock, emitted by watchdog machinery; use `TimeNow#` |

---

## 4. Operators

### Arithmetic

`+` `-` `*` `/` `%`

### Comparison

`==` `!=` `<` `>` `<=` `>=`

### Logical

`||` `&&` `!` (prefix)

### Bitwise

`|` `^` `&` `<<` `>>` `~` (prefix bit-not)

### Assignment & compound

`=` `+=` `-=` `*=` `/=`

### Consumptive (destroy RHS)

`~=` `~+` `~-` `~*` `~/`

### Arrows

| Symbol | Meaning |
|---|---|
| `<-` | Insert/extract dispatch (by op binding on each side) |
| `~<-` | Destructive arrow (consume) |
| `->` | Lambda / result flow |
| `=>` | Match arm / named-slice / associative literal |

### Unary

`&` (address-of) `*` (deref) `!` (not) `~` (bit-not)

### Cast

`as` — `expr as Type`; `(Type)expr` — C-style cast (→ `Expr::Cast`)

### Ranges & bounds

`..` `..=` `[a:b:c]` slicing `[i]` indexing `Int[8]` containment

### Other

`?` — existence check after identifier

---

## 5. Delimiters

| Delimiter | Load |
|---|---|
| `<>` | Compile-time type-level specialization (generics `Stack<T>`, variants `#String<UTF8>`, targets `asm<chip>`, groups `sync<group>`) |
| `()` | Application & binding (calls `f(a)`, params `defn f(x)`, construction `Person(...)`, op bindings `op Add: func(#Lh,#Rh)`) |
| `[]` | Containment/bound (`Int[8]`, contracts `[pre]`) |
| `{}` | Grouping/definition |

---

## 6. Directives / storage keywords

All ordinary lowercase keywords (no `#`), prefix-position, intent-bearing.

`seq` `vol` `pack` `async` `sync<group>` `atomic` `union` `trap` `halt`

Supporting markers: `box` `spill` (spawn storage), `mem` `reg` (array-lowering
pins), `coll` (collection), `out` (liveness root), `accel` (GPU deferral),
`section(".name")` (placement), `free` / `keep` (lifetime hints).

---

## 7. Hashwords

### Op-binding markers (lexed as tokens)

`#Lh` `#Rh` `#T` `#Self` (reserved)

### Stream symbols (compiler-known, lex as identifiers)

`#StdIn` `#StdOut` `#StdErr`

### Protocol variants (parameterized fundamentals)

`#String<UTF8>` `#String<ASCII>` `#String<C_String>`
`#Float<IEEE754>` `#Float<Half>` `#Float<Double>` `#Float<BFloat>` `#Float<FP128>`
`#Float<X86_FP80>` `#Float<C_Double>`

### FFI / backend directives

`#Link<name>` (emit `-l<name>`) `#System` (bare protocol hashword)

### Rule

Fundamental types appear **bare** in op signatures (`op Add(Int)`, not
`op Add(#Int)`): `Data` `Bit<N>` `Int` `UInt` `Float` `String` `Bool` `Char`
`Blob` `Ptr` `Void`. No `#`-prefixed word is ever a user identifier.

---

## 8. Macros (compile-time expansion, `name!(...)`)

Rust plugin macros (`src/plugin/`):

| Macro | Plugin |
|---|---|
| `print!(...)` | print_plugin — single-value form `print!(value)` deprecated (§13) |
| `println!(...)` | print_plugin — single-value form deprecated (§13) |
| `get_env!(name)` | env_plugin |
| `get_env_int!(name)` | env_plugin |
| `get_env_or_default!(name, dflt)` | env_plugin (stdlib-backed) |
| `entry!(...)` | entry_plugin |
| `args!(...)` | entry_plugin |

---

## 9. Annotations / attributes

| Form | Meaning |
|---|---|
| `!> key: value;` | Module/declaration metadata `[deprecated → spec]` (see §13) |
| `spec <PascalCase>: value;` | Physical-layout metadata (`Bits`, `MaxBits`, `Bytes`, `Align`, `Endian`) |
| `[pre][post]` | Contract pair on defn/txn/node |
| `[pre]]` | Pre-only contract |
| `[[post]` | Post-only contract |
| `[!/X]` / `[!/!X]` | Two-in-one invert contract |
| `?[cond]` / `![cond]` | Watchdog (optional / required liveliness) |
| `within N <unit>` | Watchdog deadline (`cyc`/`ns`/`ms`/`s`/`min`) |
| `[cond];` | Convergence gate |
| `[cond] stmt;` / `when cond { body };` | Guarded statement / block |
| `@ link sym` | Bind external runtime symbol (trg) |
| `op Parse(Decimal, pre:"0x")` / `suf:` / `reg:` | Literal discriminator attributes |

---

## 10. Reflection

| Form | Meaning |
|---|---|
| `a.^Field` | Runtime reflection (value-derived) |
| `a.^^Field` | Compile-time reflection (type-derived, foldable) |
| `a.^Length` `a.^Ptr` | Runtime value-derived properties |
| `a.^^Size` `a.^^Bytes` | Compile-time type-derived properties |

---

## 11. FFI / import / export

| Form | Meaning |
|---|---|
| `import "path.bv";` | Literal file import |
| `import <std/collections>;` | Registry lookup |
| `import { a, b: Renamed } from "module";` | Selective import + rename |
| `import alias: <path>;` | Module alias tag |
| `export defn foo ...;` | Re-export / visibility |
| `frgn sym(params) -> Ret from "src";` | Foreign binding (`from` required) |
| `frgn local: external from ...` | `:` binds a different link symbol |
| `variadic args: ForeignArgs` | Explicit variadic FFI parameter |
| `extern Name(ports) -> outs from "path";` | Foreign HDL module (`.cbv`) |

---

## 12. Removed / reserved surface

Full index deferred to a follow-up document. For diagnostics only, the
vocab (`src/vocab.rs:240-269`) records these as **removed**: `sig`, `state`,
`rstruct`, `uni`, `is`, `like`, `prop`, `meld`, `syscall`, `escape`, `term!`,
`trg!`, `cell!`, `sync!`, `frgn!`, `syscall!`, `Ptr!`, `Ok`, `Err`, `Some`,
`None`, `some`, `none`, `cycles`, `seconds`, `minute`, `minutes`,
`nanoseconds`. Removed lexical forms include `:>`, `<:`, `|>`, `++`, `#pragma`,
`#!exit`, `#?`, `#[`, and legacy duration aliases.

---

## 13. Deprecated (still works, do not use)

These forms still parse and compile, but are explicitly marked legacy in code
or docs. Use the modern replacement. Deprecated ≠ removed: a removed form is
rejected with an error (§12); a deprecated form still works but may disappear
in a future release.

| Deprecated form | Modern replacement | Deprecation marker |
|---|---|---|
| `foreach (item in list)` paren form | `foreach item in list` | `parser/statements.rs:286` "tolerated legacy form" |
| `sync { }` block | `mutex { }` | `lexer.rs:190` "replaces the legacy sync {}" |
| `!> key: value;` annotation metadata | `spec <PascalCase>: value;` | `parser/definitions.rs:2443` "annotation form (legacy)" |
| `print!(value)` / `println!(value)` single-value | `print!("fmt {0}", args)` format form | `plugin/print_plugin.rs:16` "legacy single-value form" |
| `Get#` / `Insert#` intrinsics | `At#` / `InsertAt#` | `intrinsic_signatures.rs:167` "legacy Get#/Insert#" |
| `ToInt#` / `ToFloat#` / `ToString#` | casts (`as`) / stdlib | tagged `[legacy: fall-to-external]` |
| `maxbits <~ N;` grammar | `spec MaxBits: N;` | `import_resolver.rs:1376` "legacy" |

**Tolerated, not deprecated:** `node name()` empty parens (`node name` is
primary; the parens are legal and skipped).