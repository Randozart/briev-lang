# The Iterable Protocol — Deferred Layout

**2026-08-12.** The architecture reference for iteration, indexing, length,
and `String` in Briev. Read before working on `foreach`, `b-each`, collection
codegen, reflection, or the web snapshot materializer. Plan:
`docs/plans/2026-08-12-iterable-protocol.md`. Spec: §11.4, §15.2/§15.3,
§16.3, §17.1/§17.2, §21.4.

## 1. The principle

SPEC §2.1: *types have no canonical layout*. Physical representation is
selected from operations, target constraints, metadata, and observed access —
never baked into the compiler.

Iteration is the same statement, applied to behavior instead of layout:

> **The compiler holds no collection layout, no collection names, and no magic
> member names.** A type is iterable because it provides the iteration
> operations, structurally — no `#Iterable`, no conformance, no category.

A collection's representation materializes **on observation** from its own
field definitions; iteration materializes **on observation** from its operator
surface. `String`, `List<T>`, `HashMap<K,V>`, and a user's custom collection
are all ordinary types — none is a compiler special case.

## 2. The layer model

```
┌─────────────────────────────────────────────────────────────────────────┐
│ 1. Fundamental Intrinsics (hardware axioms + value-category semantic    │
│    operations)                                                          │
│    Malloc#, Free#, Copy#, Index#, Ptr#, Deref#, Load#, Store#,          │
│    Sqrt#, CharCount#, ...                                               │
├─────────────────────────────────────────────────────────────────────────┤
│ 2. Syntactic Operation Identities (disclosed, op-as-member)            │
│    op Count, op At, op Iter, op Step, op InsertAt,                      │
│    op ExtractFrom, op CopyFrom, op Init                                 │
├─────────────────────────────────────────────────────────────────────────┤
│ 3. Stdlib Definitions (pure Briev)                                      │
│    obj List<T>, obj Stack<T>, obj RingBuffer<T>, obj HashMap<K,V>,      │
│    String (fundamental, Data-refining)                                   │
└─────────────────────────────────────────────────────────────────────────┘
```

The compiler knows layers 1–2. It knows nothing in layer 3. Collections are
stdlib; new collections are user code; neither needs compiler knowledge.
The fundamental types (`Data`, `Int`, `String`, `Blob`, …) are layer 1.5:
compiler-native primordials, iterable through the same structural op surface
(2026-08-15).
A `coll` declaration (`coll obj`/`coll struct`, SPEC §8.10) is the native
strategy keyword: the author writes the ONE sequence member, the compiler
scaffolds the op surface and picks the most effective storage (heap block /
inline array / pooled columns). **`seq coll`** forces the elements into one
contiguous memory block — for a growable `Ptr<T>` coll the data buffer IS
one block; for a fixed `T[N]` coll it forbids the columnar layout.

## 3. Op-as-member

The operator IS the member. No `op X: member(#Y)` binding RHS, no `#Lh`/`#Rh`
operand-category annotations, no bare user-facing member name resolved by the
compiler.

```briv
obj List<T> {
    inner: ListBuffer<T>;
    len: Int;                                      // ordinary slot — inert
    op Count() -> Int { term len; };
    op At(i: Int) -> &T { term inner.data[i]; };
    op InsertAt(v: T) { ... };
    op ExtractFrom() -> T { ... };
    op Init(v: T) { ... };
};
```

The compiler resolves the operator and inlines the member body. The migration
off the binding form is Slice 1 of the plan.

## 4. The two-tier iteration contract

Satisfaction is **structural** — presence of the ops is iterable-ness.

| Tier | Requires | Loop | Covers |
|---|---|---|---|
| **2 — Random Access** | `op Count() -> Int` + `op At(i: Int) -> &T` | counted `0..Count` loop (vectorizable) | `List`, `Stack`, fixed-width `String`, inline vectors |
| **1 — General** | `op Iter() -> Cursor` + `op Step(cur) -> Cursor` + `op IsEnd(cur) -> Bool` + `op Current(cur) -> &T` | external stack cursor: `iter; while !is_end { item = current; …; cur = step; }` | `HashMap`, `LinkedList`, streams, variable-width `String` |

- `foreach`/`b-each` pick the best available tier.
- `c[i]` requires `op At`; anything absent → **compile error**, never panic,
  never a skipped render.
- Tier-1 cursor is an external stack value (`op Iter()` yields the first
  element's cursor or the end sentinel; `op Step(cur)` advances to the next or
  the sentinel; `op IsEnd` tests exhaustion; `op Current` reads the element) —
  re-iterable, reentrant, zero heap. A `LinkedList` cursor is a `Ptr<Node>`; a
  `HashMap` cursor is a slot index.
- *(2026-08-12: the cursor + IsEnd + Current form supersedes the plan's
  original `op Step(cur) -> Option<&T>` — Option/union returns do not codegen
  natively yet; the cursor form is equivalent and implementable.)*

## 5. Borrow semantics

Iteration ops yield `&T` (`Ptr<T>`), not copies. `foreach(item in c)` binds
**the reference**; the body reads through it (existing Ptr field-access);
`let v = item` copies explicitly. Zero-cost for large/inline elements. The web
materializer copies only at the boundary (per flush, via `At`).

## 6. Reflection vs intrinsic governance

> **Reflection observes; it never computes.** A property that must be derived
> is an intrinsic (`PascalCase#`), called explicitly. Reflection reads
> stored/descriptor properties only, and only where no declared member reaches
> them. (SPEC §17.3.)

| Surface | Meaning | Example |
|---|---|---|
| `.^Length` | **stored** length: `Data` byte header, `String` byte header, `Vector` descriptor count | `str.^Length` → byte count |
| `CharCount#` | **computed** char count (operation intrinsic, protocol-dispatched) | `CharCount#(str)` |
| `Abs#` | **computed** absolute value (intrinsic; SPEC §17.3) | `Abs#(-5)` |
| `op Count` | **element count** (iteration contract) | `foreach`/`b-each` lowering |
| `Count#` | **element count** — the intrinsic form of `op Count` (UOL §6b) | `Count#(c)` / `c.Count#()` |
| `At#` | **element at index** — the intrinsic form of `op At` | `At#(c, i)` / `c.At#(i)` / `c[i]` |
| `.^^Element` | element type — **implemented** (single-source proof form, §7) | `List<String>` → `String` |
| `.^^Ops` | the type's operator surface (descriptor read) | iteration capability |

**2026-08-14 (Universal Operation Language, §6b):** every operation has three
invocation surfaces — a symbol (`+`, `c[i]`, `<-`, `foreach`), an intrinsic
(`OpName#(a, b)`), and the UFCS method form (`a.OpName#(b)`). `OpName#` is
recognized for any disclosed operation identity (the `operation_identities`
vocab); `a.OpName#(b)` desugars to `OpName#(a, b)` when no literal member
exists (member wins, then UFCS). Symbols are sugar over the same dispatch.
Metadata that is compiler-known but non-operational is reflection. Runtime
`.^Size` is DELETED — the element count of a collection is an operation, so
its home is the `Count#` intrinsic, not a reflection target (§6a).

`.^Length` never routes to an intrinsic. A `List`/`HashMap`/custom `.^Length`
is a compile error — that length is member-managed or computed, not intrinsic.
**2026-08-14:** `.^Absolute` was REMOVED (after a one-release deprecated alias)
— abs is a computed truth, so its home is the `Abs#` intrinsic, not a
reflection target (SPEC §17.3). The bit transformations are intrinsics too:
`BitReverse#`/`Popcount#`/`LeadingZeros#`/`TrailingZeros#`. `Bytes#` is not
added — byte length is stored, `.^Length` reads it.

## 7. Element type

Derived from the generic args: `List<String>` → `String`; `Stack<T,N>` → `T`
(width args skipped); `HashMap<K,V>` → `V`; `String` → `Char` (a frozen
`String` fundamental fact). Single-source proof form: for op-bearing types the
element type IS the read op's return (never a second derivation to drift); a
`String` operand is `Char` by fundamental. Drives the `foreach` item binding and
the web element decode.

## 8. String unification

`String` is a fundamental (`Data`-refining) type: the value is a
`[len][bytes]` pointer, layout/encoding derived by the casting graph. `String`
is `Iterable<Char>`. (2026-08-15: category `#` removed from fundamentals —
`String` not `String`; protocol variants `String<UTF8>` keep theirs.)

**2026-08-14 (current mechanism):** a `String` operand iterates `Char`
through a **protocol-keyed char-decode lane** — the loop bound is the stored
byte length (`.^Length` header) and each iteration calls `briev_str_next_char`
(UTF8 decode + advance) producing one `Char`. The compiler holds no String
layout and no name match; `String` membership is the sole key
(`is_string_operand`, a casting-graph protocol check). `Blob` (the renamed
byte buffer, formerly `Data`) keeps its byte iteration (element `Int`).
`foreach c in str` binds `c` as `Char` (SPEC §17.2 `String` → `Char`).

Encoding-selective tiers are the future specialization:

- **Fixed-width** (ASCII): Tier 2 — `op Count` = char count = byte count,
  `op At(i)` = O(1) byte load.
- **Variable-width** (UTF8): Tier 1 for chars (`op Step` = the 1–4-byte
  decoder → `Char`); Tier 2 on the byte view (`.Bytes`, a `Slice<U8>`/`Blob`).
  Character random access by index on UTF8 is a compile error.

`.^Length` on `String` = stored byte count. `CharCount#` = char count. The
`is_string_operand` special-casing is gradually subsumed by the Iterable
dispatch.

## 9. Established syntax → resolution

| Syntax | Resolves via |
|---|---|
| `foreach(item in c)` | tier pick → Tier 2 (`op Count`+`op At`) counted loop, Tier 1 (`op Iter`+`op Step`+`op IsEnd`+`op Current`) cursor loop, or `String` char decode lane (ops internally, never `.^Length`) |
| `b-each:item="c"` | web snapshot materializer driving the same ops |
| `c[i]` | `op At` (indexed borrow) |
| `c.^Length` | stored-length reflection (Data/String-byte/Vector); error elsewhere |
| `c <- x` / `x <- c` | `op InsertAt` / `op ExtractFrom` / `op CopyFrom` (unchanged) |
| `let x: List<Int> = [1, 2, 3]` | type-directed literal → `op Init` + `op InsertAt`; unconstrained `= []` → "type annotation required" |
| view `items.^Length` | the materialized array's stored `.length` (target capability) |

Read-vs-extract stays distinct: bracket read → `op At` (borrow); arrow extract
→ `op ExtractFrom`/`op CopyFrom` (value out). The `"[]" => "ExtractFrom"`
rune conflation (`src/type_universe/operators.rs:45`) is resolved by this
split.

## 10. Web materialization (`b-each`)

The analysis produces an `IterablePlan` (tier, element type, ops). The backend
emits a snapshot materializer — generated code that drives the same iteration
ops to fill `[len][word…]` in a scratch buffer. The shim reads it by the
element type tag (numbers raw, strings as pointers, objects as handles). View
`.^Length` reads the materialized array's stored `.length`. `Applied`/`Vector`
fields classify structurally (mirror `signal_type_for`).

Implementation notes:

- `CompiledView.collection_iterables` / `collection_string_iterables` carry
  the iterable fields per view; the backend emits one
  `__view_items_<field>(%state)` snapshot materializer per iterable field.
  It calls `op Count`/`op At` (or the Tier-1 cursor ops) and stores
  `[len][word…]` into a scratch buffer. The materializer is a **snapshot**:
  it runs per flush, so the view sees a consistent array even if the
  collection mutates between flushes (never a dangling handle).
- The shim (`emit_collection_each`) reconciles the `[len][word…]` snapshot
  into the DOM with a **stable `b-key`** (index by default); children are
  keyed so insertion/removal/reorder updates the minimum number of nodes.
- **String elements** (`collection_string_iterables`): the materializer boxes
  each `op At` result `String` (a `[len][bytes]` pointer) via `ptrtoint` and
  the shim decodes it by the string header — `[len][bytes]` is emitted as a
  JS string. Numeric elements are raw words; object elements are boxed
  handles.
- `signal_type_for` classifies `Applied` vs `Vector` fields structurally so
  the materializer knows whether the iteration ops exist at all; a
  non-iterable `b-each` iterable is a compile error, never a skipped render.

## 10a. Local collection construction

A collection literal in a **local** scope (not an instance field) is
constructed through the same operator surface as everywhere else:

- `construct_local_collection` emits `op InitEmpty`/`op Init` then one
  `op InsertAt` per element, returning the boxed handle. The local is a
  pointer to the collection object, like any pooled-instance field.
- Arrow-inserts against a local receiver (`&items <- 3`) resolve the local
  bound type via `emit_strategy_member_call` (no state field required) —
  a local collection is usable exactly like a field collection.
- Verified end-to-end: local literal + `op At` read, local literal + push,
  `foreach` over a local-built collection (native and web).

## 10b. Web interactive state

The rendered backend keeps the whole program state in one `@__web_state`
global; every `_txn` export takes `%state` first and the shim passes
`__briev_state_ptr()`:

- `__briev_state_ptr()` returns the state pointer (the shim marshals each
  transaction's input into the state then reads the output fields back out).
- `__web_boot()` runs any boot initialization before the first flush.
- `render_frame()` is the flush driver — the JS side calls it in a
  `requestAnimationFrame` loop (see `rendered-briev-wasm.md`).
- If the program's observable side effects fold away, `render_frame()` and
  `reactor_tick` are still emitted (a folded program has no state to render,
  but the shim contract is stable).

## 11. Deletion map (old site → new mechanism)

| Old hardcode | Location | Replaced by |
|---|---|---|
| `foreach` `List` layout `[len][elem…]` i64, item forced `Int`, `panic!` | `emit_stmt.rs:186,1136,1166,197` | structural `op Count`+`op At`/`op Iter`+`op Step` loop, borrow-typed item |
| `[]` `List` layout read | `emit_expr.rs:1017` | `op At` dispatch (inline member call) |
| `.^Len` collection panic; runtime `.^Size` `name == "len"` slot heuristic | `emit_expr.rs:2477,2383-2393` | `.^Length` stored-length reflection; `CharCount#`; **2026-08-14 (§6a): runtime `.^Size` DELETED — the element count is the `Count#` intrinsic (element-bearing op return), never a `len` slot-name guess. `.^^Size` (compile-time) keeps the vector shape** |
| `ringbuf_inline` ("any `op InsertAt` type is a 4-slot ring") | `context.rs:75`, `mod.rs:4601/4867`, `emit_stmt.rs:1402` | deleted after generic layout-on-observation |
| `emit_heap_seq` / `emit_svo_list` / `emit_svo_index` | `emit_expr.rs:1573/1627/1686` | type-directed literals via `op Init`+`op InsertAt`; generic indexing |
| `is_string_operand` special-casing | throughout | String's Iterable surface |
| ~20 `"List"` string matches | `src/typechecker/`, `src/backend/llvm/`, `src/ssr.rs`, `src/lexer.rs` | structural resolution |
| `.^Len` → `.^Length` rename | `resolve_reflect` (`typechecker/mod.rs:3125`), `emit_reflection` (`emit_expr.rs:2441`), shim | language-wide |

Deletions strictly follow their replacements (Rule 5, additive only).

## 12. The intrinsic-audit roadmap (follow-up plan)

Briev already covers most of the platform-agnostic intrinsic surface (atomics
`Atomic*#`/`Fence#`, memory `Malloc#`/`Free#`/`Copy#`, float `Sqrt#`/`Pow#`,
GPU workgroup, pointer/index `Ptr#`/`Index#`/`Deref#`). The audit adds, with
stdlib wrappers and software fallbacks per target:

- Bit manipulation: `Clz#`, `Ctz#`, `Popcount#`, `Bswap#`, `Rotl#`, `Rotr#`,
  `BitReverse#`.
- Overflow-checked / wide arithmetic: `AddOverflow#`, `SubOverflow#`,
  `MulOverflow#`, `MulWide#`, `CarryingAdd#`.
- Float extras: `Fma#`, `Copysign#`, `MinNum#`, `MaxNum#`, `Trunc#`, `Round#`.
- Portable SIMD primitives: splat/extract/insert, vadd/vsub/vmul/vdiv,
  vand/vor/vxor/vandnot, vcmpeq/vcmplt/vcmpgt, shuffle/blend, masked load/store.
- Crypto (optional): `CRC32#`, AES round.
- Governance: intrinsic → stdlib wrapper → software fallback → target-
  capability rejection. Audit how the existing `Length#`/`Get#`/`Insert#`
  relate to the op-as-member surface.

## 13. Hard constraints

- **No collection intrinsics** (`ElemCount#`, `At#`, `RingPush#`).
- **No `#Iterable`** category or hashword.
- **No magic member names** — the compiler resolves operators, never `len`/
  `get`/`push`-style bare names.
- **Reflection never computes**; computed properties are intrinsics.
- **Never panic or skip** — a non-iterable is a compile error with guidance.
- **Deletions follow replacements.**

## 14. Design decisions (2026-08-12)

### 14.1 `foreach` is parenless (`foreach x in list { … }`)

The `in` keyword IS the binding; the `( item in list )` parens were redundant
call-lookalike syntax with no disambiguation role (the list expression
terminates unambiguously at `{`). The parenless form matches the
`()`-means-application delimiter rule (#20) and Rust/Python/C++ range-for.
The paren form is tolerated as legacy and gradually removed. **Why:** the
parens implied a function call where none exists; the delimiter rule reserves
`()` for application and binding, and `in` already binds.

### 14.2 The `b-` directive prefix is kept (not legacy)

`b-text`, `b-show`, `b-when`, `b-each`, `b-class`, `b-bind`, `b-trigger` use
the `b-` prefix as the **attribute-namespace separator** — the same pattern as
Vue's `v-`, Alpine's `x-`, Angular's `*ng-`. **Why it is not legacy:**

1. **Namespace separation** — `b-` marks compiler-owned attributes; every
   other attribute passes through as plain HTML. Without it, `class="x"` is
   ambiguous (an HTML class or a binding?), and the view compiler would have
   to know every HTML attribute to avoid clobbering user markup.
2. **Disclosure** (Rule 3) — `b-text` is visibly special; no hidden magic in
   an ordinary-looking attribute.
3. **Passthrough** — `.s.rbv` views being HTML-compatible is the point;
   non-`b-` attributes work as HTML.

The `b` letter is a style choice (like Vue's `v`); the *separator* is the
substance. Removing it would force every attribute through the directive
parser and break HTML passthrough.
