# Briev Language Specification

**Version:** Draft 2026-08-05
**Status:** Normative target specification; implementation conformance is staged
**Authority:** This document is the language contract. Architecture documents explain implementation. Tutorials teach only this syntax.

## 1. Scope and conformance

Briev is a contract-first language for layout-independent values, reactive state machines, target-specialized execution, and adaptable foreign boundaries.

A conforming implementation must:

1. Preserve the semantic behavior defined here across every supported target.
2. Implement a normative construct identically to the reference interpreter or reject it with a compile-time target-capability error.
3. Never substitute placeholder values, silently weaken contracts, or select behavior from user-visible type names.
4. Resolve semantic operations in the frontend and leave only equivalent physical realization to backends.
5. Treat every normative example as a conformance fixture.

A section marked **Staged** is normative design that may not yet be implemented. Until implemented, the compiler must reject it explicitly.

## 2. Core model

### 2.1 Types have no canonical layout

A Briev type denotes semantic behavior, logical fields, invariants, metadata, and operation bindings. It does not own one physical representation.

Physical representation is selected from:

- operations performed on values;
- target constraints;
- declared metadata;
- protocol membership and cast paths;
- ownership and effect requirements;
- observed access, slicing, and traversal patterns;
- explicit boundary descriptors.

The frontend decides meaning. LLVM, SPIR-V, CIRCT, Webstack, and future backends may choose different equivalent layouts and instruction strategies.

Every runtime value selected for a concrete target must be materializable by that target. Failure to resolve a representation is a compile-time error; there is no generic integer fallback.

> **2026-08-13 (Deferred Layout).** When a type must pin its physical
> representation — a C-exposed record, a register file, a packed bitfield —
> the author declares that layout explicitly with the **`spec`** keyword
> (`spec Bits: 12;`, `spec Bytes: 4;`, `spec Alignment: 2;`, `spec Endian: Big;`,
> §8.2/§8.9) or the layout directives of §8.2. This is **deferred layout**: the
> type's abstract semantics never assume a representation, and the physical
> rendering is a declared, backend-compatible choice layered on top. See the
> corresponding plan (`docs/plans/2026-08-13-layout-keywords.md`).

Iteration, length, and indexed access are likewise selected from a type's
operation surface, never from a compiler-known collection layout. The
compiler holds no collection representation: a sequence's elements are read
through the operations the type itself declares (see §11.4). The compiler
also holds no collection or element type names as semantic keys — a type is
iterable because it provides the iteration operations, not because of its
name or an explicit conformance marker.

> **2026-08-15 (`coll`).** The **`coll`** keyword (on `obj`/`struct`
> declarations, §8.10) is the **native strategy keyword for declaring
> collections**, not a trait or conformance marker: it declares that the type
> has **compiler-owned Length semantics** (length and capacity live in hidden
> slots the compiler maintains, never declared fields) and instructs the
> compiler to **scaffold the operation surface** (`op Count`, `op At`,
> `op Init`/`op InsertAt`/`op ExtractFrom`, and the default `op Grow`/
> `op Shrink` strategies). The rule above still holds: the scaffolded
> operations ARE the operation surface — `coll` does not grant iterability by
> name, it *synthesizes* the operations a structural probe then resolves.
> `coll obj MyQueue<T>` is treated identically to any other `coll` type; no
> collection name is a semantic key. A `coll` collection is as fast as the
> compiler can make it — the scaffolded ops fold to hand-written-equivalent
> code. See `docs/plans/2026-08-15-coll-length-semantics.md`.

> **2026-08-12 (Iterable protocol):** `String`, `List<T>`, `Stack<T>`, and a
> user-declared collection are all ordinary types whose iteration resolves
> structurally (§11.4). No collection or string is a compiler special case.

> **2026-08-15 (Fundamentals).** The fundamental types are compiler-native
> primordials, not stdlib redeclarations: `Data`, `Bit<N>`, `Int`, `UInt`,
> `Float`, `Bool`, `Char`, `String`, `Blob`, `Ptr`, `Void`. They need no
> stdlib entry and carry no overloadable ops (`op` is for user types).
> **`Data` is the universal reflective floor** — every value can be observed
> and reflected as its raw storage (the treat-as-bits view); "parent" is a
> reflective category, never an inheritance edge in the casting graph.
> **`Bit<N>` is the unified bit type** at any declared
> width (`Bit` bare = flexible, resolved later); every type is composed of
> bits, and `Bit<N>` names a run of bits directly — there is no separate
> `Bits` type. **`Blob`** is the `[len][bytes]` byte buffer (a `Data` member
> like `String`, but with no encoding interpretation). See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.

### 2.2 Semantic values and materialization

The reference interpreter operates on generic semantic values:

- semantic type identity;
- optimized primitive atoms;
- bits;
- products;
- sums;
- references;
- closures;
- void.

Stdlib concepts such as lists, maps, options, and results are not hardcoded interpreter value variants. Materialization lowers semantic values to target representations.

### 2.3 Frontend/backend boundary

The frontend resolves each syntactic operation to semantic behavior, contracts, effects, and access shape. Backends may choose any equivalent physical realization but must not redispatch by source type name or redefine operation meaning.

Examples:

- LLVM may scalarize, vectorize, reorder, or choose strided storage.
- CIRCT may realize the same operation as registers, memories, or parallel paths.
- SPIR-V may select target-appropriate storage classes and vector operations.

## 3. Source files and target profiles

### 3.1 Base variants

| Extension | Role | Canonical target path |
|---|---|---|
| `.bv` | General Briev — hosted **and** freestanding (triple-driven) | LLVM/native; optional configured offload |
| `.ebv` | Electronics Briev (PCB design, closed-world strict) | Electronics |
| `.abv` | Accelerator Briev | Direct SPIR-V through `rspirv` |
| `.sbv` | Silicon Briev | CIRCT |
| `.rbv` | Rendered Briev | Webstack |
| `.dbv` | Structured Data Briev | Data parser |
| `.dbvl` | Line-oriented Data Briev | Streaming data parser |

`.dbvs`, `.cbv`, `.srbv`, `.sebv`, `.c.bv`, and other compact or legacy variants are not part of the language.

> **2026-09-09 (family realignment).** Embedded Briev is folded into `.bv`:
> the runtime is Briev-native everywhere (§3.4), so freestanding is a target
> configuration, not a source variant. Electronics Briev (`.ebv`) is the
> closed-world strict PCB variant (§3.5). Silicon Briev (`.sbv`) reclaims the
> formerly forbidden `.sbv` extension; Circuit Briev (`.cbv`) is retired.

### 3.2 Dotted profiles

Profiles precede the base extension as separate dotted segments.

```text
main.s.bv
ui.s.rbv
kernel.f.bv
```

#### `.s` — strict verification

`.s` changes acceptance criteria, not runtime semantics or grammar. Strict verification rejects unresolved proof obligations, representation fallbacks, unclassified concurrency, and unresolved lifetime warnings.

Explicitly trusted FFI or protocol axioms are permitted only when:

- trust is visible;
- boundary contracts are complete;
- ownership and effects are declared;
- all mechanically checkable obligations are proven;
- the trust boundary appears in the verification report.

#### Declared authority (`axiom`) — enforcement dial

Every `axiom`-declared site is counted. The strict report renders the full authority ledger regardless of configuration. Outside strict profiles the enforcement dial (`config/axioms.dbv`, `policy`) selects acceptance behavior:

- `allow` — accepted; one info line per site in the warnings stream.
- `warn` — accepted; a prominent warning naming every site rides alongside.
- `deny` — any axiom site is a hard error: prove it or remove the shortcut.

The compiler learns nothing hardcoded about individual axioms. (2026-09-22:
the `lemma_properties` vocabulary is removed with the op-lemma feature — see
§8.8; the axiom facility itself stays.)

#### `.f` — formatted source

`.f` is a strict indentation dialect of the same language.

- Indentation delimits declaration and statement blocks.
- Statement-block braces are forbidden.
- Semicolon terminators are forbidden.
- Literal delimiters retain their normal meanings.
- The layout frontend is token-aware and preserves original source spans.
- `.f` produces the same AST as canonical brace syntax.

### 3.3 Target capability profiles

Target restrictions are declared in configuration and validated once in the frontend. A backend does not independently invent a source-language subset.

Target-specific sibling modules may coexist. Extensionless imports select the variant configured for the active target. Every sibling variant must satisfy the same exported interface and trait contracts.

### 3.4 Briev-native runtime

The default runtime is Briev-native: the compiler and standard library do not
depend on a C runtime (no `briev_rt.c`, no libc) to build or run any program.
System interaction reaches the OS through:

- the `SysCall#` intrinsic, which emits platform inline asm (`syscall` on
  x86_64, `svc #0` on aarch64);
- compiler-captured process state (`argv`, `environ`); and
- Briev-native standard library wrappers over those primitives.

Foreign C interop remains a **user-facing** feature, never a compiler
dependency: `frgn ... from #System` (→ the selected system library),
`from "path"`, `#Link<name>`, and GLUE bridges are unchanged.

Consequently there is one runtime for every target. Freestanding (bare-metal)
compilation is selected by a freestanding target triple — the same triple
families that gate `halt;` — or a target profile override, not by a source
variant. Hosted and freestanding builds differ only in allocator growth (the
hosted arena may grow via `brk`/`mmap`; the freestanding arena is fixed) and
available platform surface. The language surface is identical.

The runtime model is governed by **expressiveness closure**: the compiler's
chosen optimum must never be more powerful than the language. The
compiler-internal allocator keeps only a bootstrap fallback (what `--no-std`
requires); the allocation *strategy* is owned by the standard library
(`alloc-strategies` config resolves strategy names, and the strategy
functions are ordinary Briev definitions). Likewise, machine-level escape
hatches are ordinary intrinsics: `Asm#` lowers either a named abstract
instruction through per-target config tables or a raw asm template, so no
hardware technique permanently requires a compiler change. Anything the
compiler can do, Briev code can build.

### 3.5 Electronics Briev

Electronics Briev (`.ebv`) applies the Briev philosophy — topology, contracts,
nodal reasoning, compile-time proving — to printed circuit boards. You type
(roughly) the same Briev code as any family; the compiler turns those
declarations into their final shape based on how the intentions map onto the
electronics backend (§3.1 doctrine). Forms are syntax: an `obj`, a `struct`,
and a `cell` all exist in `.ebv` as declaration shapes with no domain
semantics — the electronics-ness lives in the fundamentals, the property
clauses, and the backend mapping.

**The bases are electrical.** There is no `Float` here and no numeric tower:
the fundamentals are physical quantities — `Volt`, `Amp`, `Ohm`, `Farad`,
`Henry`, `Hertz`, `Watt`, `Kelvin` — declared as PARENTLESS types in
`std/electronics.bv` and injected by the prelude. Under the fundamentals
doctrine (§4.3) a parentless declared type self-roots as its own category,
so `Volt` stands exactly where `Float` stands in the software family: no
IEEE semantics, no inheritance, no baggage. Quantities are provenance
values — proven at compile time, never executed.

**Properties, not annotations.** Pins, reference designators, and tolerances
are first-class clauses with identical grammar on every declaration form:

```briev
type Led {
    pin a;
    pin k;
    reference "D";      // mandatory whenever pins exist (parse-enforced)
    tolerance 3.6;      // max volts — a rated decision
};
```

`pin a;` auto-numbers (highest-so-far + 1); `pin p1 = 1;` states the
datasheet number (≥ 1, unique per type). Pins are type-level topology,
never instance-construction fields. `tolerance any;` DECLARED unrated — a
pin with no clause at all is an undeclared decision and violates any net
driving it.

**Contracts are the wiring and the physics.** There is no connection
operator. Preconditions state topology — a `==` between two pin accesses
puts both pins on the same electrical node; the netlist is the transitive
closure (union-find). A `==` against a literal drives the net at that
level; disagreeing drives on one net are a shorted supply. A conjunct may
be prefixed `net <name>:` to NAME the equivalence class — inference is
unchanged; the name replaces the auto-generated `N1, N2…` in diagnostics
and KiCad output (net names are contextual: keywords like `out` are valid
names). Postconditions state physics — and the compiler PROVES them:
through a two-pin part with a numeric value, I = V / R is derived at
compile time and the derived current is checked against the stated bound.

Physics literals carry unit suffixes: `3.3V` (volts), `20mA` (→ 0.02 A),
`330R` (ohms), plus `A`, `Ω`, `F`, `H`, `Hz`, `W`, `K`. A suffixed literal
is the value; the suffix selects the conversion (`mA` divides by 1000).
Bare numerics stay valid everywhere a suffixed form is.

```briev
txn powered
    [net vcc: j1.p1.voltage == r1.a.voltage && net out: r1.b.voltage == d1.a.voltage && net gnd: d1.k.voltage == j1.p2.voltage && j1.p1.voltage == 3.3V]
    [d1.a.current > 0.0 && d1.a.current <= 20mA]
{ }
```

Here 3.3 V is proven within the LED's 3.6 V tolerance, and the 20 mA bound
is proven from 3.3 V / 330 Ω = 10 mA — by derivation, not assertion.

**Power ratings.** `rating 0.25;` declares the watts a part may dissipate
(`rating any;` declares it unrated on purpose). Every valued two-pin part
with both endpoints voltage-classed has a PROVEN dissipation P = V × I:
above the rating is a violation; a proven-dissipating part with no rating
clause is an undeclared decision; within the rating records a proof fact.
A part with an unclassed endpoint has no proven drop — nothing is forced.
Precedence: conjoined obligations use `==` (single `=` binds loosest, §4).
A dangling pin — declared but on no net — is a compile error naming the
pin. Compilation emits a KiCad 7 schematic; the backend refuses any board
that is incomplete or electrically violated.

> **2026-09-21 (intent synthesis — PLANNED, non-normative).** The `.ebv`
> surface is being extended toward intent-based synthesis: the author
> declares behaviors and invariants over `volatile` component pins
> (`node name [guard] { drive-maps }`), and the compiler infers both the
> wiring (net membership) and the physics. Planned constructs — pin
> electrical classes declared as **fundamentals in
> `std/electronics.bv`** (`Power`/`Ground`/`In`/`Out`/`Io`/`IoOd`/`Nc`,
> PascalCase — they are types, carrying `spec KicadType`/`spec NoConnect`
> properties the compiler consumes generically), `spec` datasheet-fact
> clauses, pin arrays, population facts (`populated = false`),
> `chain`/`await` sequencing sugar, and ambiguity-lifting modifiers — are
> inventoried with full semantics in
> `docs/plans/2026-09-21-intent-synthesis-node-semantics.md`. Grammar is
> unfrozen pending implementation; this section documents the current
> surface only.

## 4. Lexical conventions

### 4.1 Keywords and identifiers

Keywords are lowercase and case-sensitive. Wrong spelling or casing of compiler-known vocabulary is an error with a suggested correction.

Compiler-known hashwords, intrinsics, and syntactic operation identities also require exact spelling.

User-declared casing is advisory:

- `PascalCase`: types, traits, structs, enums, objs, cells, protocol variants, and operation identities;
- `snake_case`: functions, fields, nodes, variables, and macros;
- `PascalCase#`: intrinsics.

Violations for user-defined names produce informational diagnostics or warnings, not errors.

The names `sed`, `pvt`, and `reg` remain reserved for future language contracts.

### 4.2 Comments

```briev
// line comment
/* block comment */
/// declaration documentation
//! module documentation
```

### 4.3 Sigils

| Form | Meaning |
|---|---|
| `#Target` | target/protocol hashword (`#System`, `#Web`, `#Link<name>`) — routes a declaration to a bridge target; never a type category |
| `Intrinsic#` | compiler intrinsic |
| `$name` | compile-time-only declaration/binding |
| `name!(...)` | explicit compile-time expansion |
| `$(Stage)` | staged compile-time execution |
| `value.^Field` | runtime field reflection |
| `value.^^Field` | compile-time descriptor reflection |

`#` may occur only in recognized prefix hashwords or as the terminal intrinsic suffix. Embedded forms such as `foo#bar` are invalid.

> **2026-09-11 (fundamentals doctrine):** the category hashwords
> (`#Float`, `#Int`, `#String`, …) are RETIRED. The fundamental name is
> both the base type and the protocol: write `Float` as a type, parent,
> protocol category, and op parameter; write `Float<Posit>` for a
> protocol variant. A `#Float`-style spelling in a type position is a
> compile error naming the fix.

Prefix `!value` is Boolean negation. Postfix callable `!` is compile-time expansion. Runtime keyword-bang forms do not exist.

### 4.4 Removed lexical forms

The following are not Briev syntax:

- `rstruct`, `uni`, `like`, `is`, `prop`, `sig`, `state`, `meld`, `syscall`;
- `term!`, `trg!`, `cell!`, `sync!`, `frgn!`, `frgn?`, `frgn?!`, `syscall!`;
- `Ptr!`, `<:`, `:>`, `|>`, `++`;
- legacy pragma/attribute forms `#[`, `#![`, `#pragma`, `#!pragma`, `#?`, `#!`;
- `Ok`, `Err`, `Some`, and `None` as reserved tokens;
- adjacent tagged literals such as `sql"..."`;
- `@`-quoted raw literals;
- import aliases using `as`;
- explicit `[true][true]` contracts.

## 5. Delimiters and arrows

### 5.1 Delimiter load

- `<>`: compile-time specialization, protocol variants, target specialization, and synchronization groups.
- `()`: application, binding, parameters, and construction.
- `[]`: containment, bounds, indexing, contracts, guards, and slicing.
- `{}`: grouping and definitions.

Generic application uses `<...>` only. Square-bracket generic aliases are invalid.

### 5.2 Arrow load

- `->`: result/output flow and directional declarations.
- `<-`: transfer/insertion/extraction behavior.
- `~<-`: destructive transfer/extraction.
- `=>`: match-arm and associative-literal mapping.

`<:` and `:>` do not exist.

## 6. Grammar overview

The grammar below is normative at the structural level. Expression precedence is defined in §15.

```ebnf
program          ::= item*

item             ::= import_decl
                   | export_import_decl
                   | export_decl
                   | const_decl
                   | let_decl
                   | init_decl
                   | type_decl
                   | trait_decl
                   | proto_decl
                   | struct_decl
                   | enum_decl
                   | impl_decl
                   | obj_decl
                   | cell_decl
                   | defn_decl
                   | txn_decl
                   | node_decl
                   | frgn_decl
                   | asm_decl
                   | trigger_decl
                   | render_decl
                   | compile_time_item

identifier       ::= user_identifier
path             ::= string_literal | "<" path_component ("/" path_component)* ">"
```

## 7. Modules and imports

### 7.1 Path model

```briev
import "./local/path.bv";
import <std/collections>;
```

- Quoted paths are exact or project-relative.
- Angle-bracket paths resolve through ordered roots declared by compiler/target configuration.
- Package registries are configured roots, not a separate source-language import category.
- Resolution is deterministic and records the resolved path.

### 7.2 Aliases and selective imports

`as` is reserved for semantic conversion. Import aliases use local-to-source `:` binding.

```briev
import collections: <std/collections>;
import { LocalName: ExportedName, OtherName } from "./module.bv";
```

Conflicting unqualified imports are errors and must be resolved with a module alias or selective rename. Import order never changes meaning.

Glob imports are invalid.

### 7.3 Visibility and re-export

Ordinary imports are private to the importing module.

```briev
export import { PublicName } from "./internal.bv";
```

`export import` is the only re-export form.

### 7.4 Import graph

Diamond dependencies are valid. Genuine import cycles are compile-time errors. Shared declarations must move to an acyclic interface module.

## 8. Declarations

### 8.1 `let`, `const`, and `init`

```briev
let counter: Int = 0;
const MaxRetries: Int = 3;
init BufSize: Int = get_env_int!("BUFSIZE");
```

Top-level `let` declares reactive program state. Local `let` declares a local binding. There is no `state` keyword.

`const` declares an immutable top-level value. `$const` is a separate compile-time-only binding erased before runtime.

`init` declares a **runtime-seeded invariant**: a top-level value set exactly
once, before `beginprogram` or any other transition fires, and provably
immutable for the remainder of the run. It is not compile-time-folded like
`const` — the value is loaded from the environment or target at runtime — but
after that single seeding the compiler treats it as a proof-stable constant:
reassignment of an `init` name anywhere is a compile error.

`init` may declare an expected value set (see "Bounded value sets" below). An
`init` with a declared, provable value set is the preferred substrate for
capacity, bounded loops, and lifetime proofs.

Protocol-supplied defaults may initialize omitted logical fields. Unknown fields, duplicate fields, unresolved defaults, and mismatched constructor arity are errors. There is no generic zero-fill fallback.

**Bounded value sets.** An `init` may declare that its value is *one of* a set
of options, written between `:` and the type so it cannot be confused with an
array dimension:

```briev
init BufferSize: [64 | lo..hi] Int = ...;   // exactly 64, or in [lo,hi]
init BitLayout:  [16 | 32 | 64] Int = ...;  // one of three; target resolves
```

- `[lo..hi]` is a range of expected values; `[a | b | c]` is a discrete union
  of options (values and ranges may mix).
- The set declares the bound over all expected values, giving the compiler a
  finite proof domain (for capacity: the max of the set; for layout: a choice
  the target resolves, falling back to the minimum).
- A bounded `init` at a generic site is a finite proof domain (e.g. a
  size parameter is adapted per-target without unbounded instantiation).
- An unbounded `init` is permitted where the surrounding contract proves
  satisfactory resolution, but is weaker proof: the compiler must fall back to
  a runtime check.

**Proof-vs-decision hierarchy (core philosophy).** Briev compiles through
proof and leaves decisions to the programmer where no single option is best:

1. **Prove**: a provably-best strategy is the default; the compiler is silent.
2. **Guardrail with request for disambiguation**: when selection is genuinely
   ambiguous, the compiler emits a warning and requires the programmer to name
   a strategy explicitly (`init` with a value set, or a storage-strategy
   classification).
3. **Error when provably reckless and/or underdetermined or overdetermined**:
   a chosen strategy that provably conflicts with its use (use-after-free,
   capacity below proven need, an underdetermined or overdefined bound) is a
   compile error. The compiler never ships a provably wrong program.

This underpins concurrency classification (`async`/`sync<group>`), lifetime
management, capacity proofs, and layout selection alike.

**Strategy keywords.** All compiler strategy is expressed in keywords.
Collectively these are **strategy keywords** — what pragmas are in other
languages, but transparent: ordinary words in the program, not hidden
directives, so they carry little to no knowledge tax. They are the inverse of
pragmas in one more sense: you never need one to get correct, efficient code —
the compiler proves the default and *reminds you* when a decision genuinely
needs your input (it warns and asks for a keyword). Rules:

- Strategy is **keyword-shaped**, never an invisible flag.
- Keywords are **transparent** — ordinary syntax, disclosed, well-documented.
- **Zero knowledge tax**: omitting a strategy keyword is the common case;
  derivation is proven, not annotated.
- They **never make code faster**: the default is the efficient path, and a
  keyword-beaten default is a compiler bug (`seq`, `vol`, `async`, `box`,
  `spill`, `storage` all follow this).
- **Disclosed compiler ownership** (`coll`, §8.10): the keyword reveals that
  the compiler owns a property (Length semantics) it would otherwise derive
  structurally — never a speed win, always disclosed.- One shape, `category<mechanism>`: the *category* keyword is
  program-independent (`borrow`, `storage`, `delivery`); the *mechanism* rides
  inside `<>` and is either a compiler-known intrinsic class or a config row
  (`borrowed<source>`, `sync<group>`, `#Link<name>`, `String<UTF8>`,
  `asm<chip>`). Mechanisms resolve through shared config registries; categories
  are keywords — "config learns, compiler teaches." See §14.1 for the ownership
  category and `docs/plans/2026-08-09-init-kind-invariant.md` for the full axis
  table.

**Placement keywords (2026-09-06).** `section(".name")` is a placement
declaration — where the function's or constant's bytes live in the output
image — never a speed hint:

```briev
section(".init") defn startup() -> Int { ... };
section(".rodata") const TABLE: Int = 5;
```

**Modifier order is free (2026-09-22).** Modifier/strategy keywords compose
in any order before a structural identifier: `<keywords>* <identifier>
<name>`. `vol out let x` and `out vol let x` are the same declaration;
`seq accel node`, `accel seq node`, and `sync<g> async node` all parse.
The identifier always sits after the keywords — a modifier never follows
it (`node async` is a node *named* `async`, not a postfix modifier). Two
exceptions:

- `bootstrap` is a fixed compound: `bootstrap node name` only.
- Duplicate modifiers are an error (`seq seq node` does not compile).

Restrictions: `async`/`sync<group>`/`accel` apply to a node or txn;
`vol`/`mem`/`reg` apply to `let` only; `out` applies to a defn, node,
txn, or let; `seq`/`pack`/`coll` before a struct/obj are layout flags the
struct parser owns (`pack seq struct`, `seq pack struct`).

- Applies to `defn` and `const`. State fields live in `%State` (one
  allocation) and have no per-field linker section — a `section` prefix on
  anything else is a parse error.
- **Proof obligations (compile time):** a sectioned `defn` runs where the
  default heap may not exist yet (boot code in `.init`, RAM-copied handlers
  before relocation) — it and everything it REACHES must not allocate
  (`Malloc#`/`Alloc#`/`Realloc#`/`Spawn#` are banned; the proof is transitive
  over the program's call graph, and the error names the reachable path,
  e.g. `startup -> mid -> helper -> Malloc#`).
- The compiler emits the placement as an LLVM section attribute on the
  function/global; the section name is arbitrary and target-linker-owned.

**Storage-strategy markers.** Where the compiler cannot select a single best
storage strategy, the programmer classifies explicitly:

```briev
box    // heap-per-instance storage, not a pooled column; an explicit choice,
       //   not a hidden special case
spill  // a value may grow beyond its static pool column into a growable buffer
mem    // (hardware targets) pin a state array to the memory-macro lowering:
       //   accepts port limits; element postconditions are unavailable
reg    // (hardware targets) pin a state array to per-element registers:
       //   combinational access, full element obligations
```

- These markers carry the same rule as `seq`/`vol`/`async`: they must never
  make code faster than the default — the default is always the efficient
  path, and a marker-beaten default is a compiler bug.
- They exist only to *reveal* a choice the backend would otherwise hide or
  guess, at the points where the compiler genuinely cannot decide a single
  best-fitting, fastest, most-efficient strategy.
- Their absence is the normal case: derived storage is proven, not annotated.
- **Loud defaults**: when a hardware target lowers an unannotated state
  array by policy (e.g. depth ≥ threshold → memory macro), the compiler
  prints ONE aggregated note naming every such array with its reason and
  the pin that silences it. Explicitly pinned arrays never appear in the
  note. Silent storage decisions do not exist.

**Memory-decision audit.** `brievc memcheck` reports every memory decision
point — lifetime, capacity, storage class, dependent versus static columns —
with the strategy chosen, its location, and the proof obligation that
justified it. Silent decisions are auditable; a decision that fell back
(ambiguous → warning tier) is reported as such.

### 8.2 `struct`

A `struct` declares a named data relationship. It does not own methods, operations, identity, or lifecycle.

```briev
struct Point<T> {
    x: T;
    y: T;
};
```

A plain struct is layout-adaptive: a backend may reorder, split, scalarize, or eliminate fields when semantics permit.

```briev
type Length32: Int {
    spec Bits: 32;
};
```

`seq struct` preserves field order and containment. Target protocol/ABI configuration still determines widths, alignment, and padding unless more explicit boundary constraints apply.

> **2026-08-13 (`pack struct`).** `pack struct` is bit-contiguous: fields are
> packed with zero padding in declaration order, so the storage volume is
> `ceil(Σ field widths / 8)` bytes. A packed field must be a scalar bit-width
> (`Bit<N>`, `0 < N ≤ 64`); array (vector) fields are rejected. `pack` and
> `seq` are order-independent prefix flags (`pack seq struct`, `seq pack
> struct`).
>
> ```briev
> pack struct Header {
>     spec Endian: Big;
>     dst: Bit<48>;
>     src: Bit<48>;
>     ethertype: Bit<16>;
> };
> ```
>
> Whole-byte packed fields (width % 8 == 0) lay out exactly like a
> byte-granular struct and use LLVM's native packed aggregate (`<{ ... }>`).
> Sub-byte fields (e.g. `Bit<12>`) occupy a bit-aligned slice of the byte
> image and are accessed with load-shift-mask / load-modify-store; a sub-byte
> packed struct materializes as a byte array. A zero-width `Bit<0>` field is
> padding: it occupies no storage and reads as 0.
>
> Bit order is endian-coupled: default/`Target` is native (the bit at
> position `p` is bit `p % 8` of byte `p / 8`, LSB-first), `Big` is MSB-first
> within each byte with big-endian multi-byte fields. For a sub-byte field at
> stream position `p` (`within = p % 8`, `cov = ceil((within + width)/8)`),
> Big-endian reads shift the covered region by `cov*8 − within − width`.
> Packed alignment defaults to 1 (no inter-element padding); `spec Alignment`
> overrides.
>
> **2026-08-13 (cast width).** `x as Bit<N>` is a width assertion: the value
> truncates to exactly `N` bits (a `Bit<4>` can never hold 16). The reference
> interpreter and the LLVM backend agree on this; packed stores also mask to
> the field width defensively.
>
> **2026-08-13 (field modifiers).** A field may be declared `atomic`
> (`atomic count: Int;`) in a struct or obj/type body. It is a concurrency
> declaration, never a speed path: plain fields stay on the default
> (non-atomic) path. The LLVM backend emits `load atomic`/`store atomic` for
> atomic fields, and lowers `obj.f = obj.f + c` /
> `obj.f = obj.f - c` to `atomicrmw add/sub` (read-modify-write). The
> reference interpreter is single-threaded check mode, so atomic fields behave
> as plain fields there — atomicity is a target concern (the explicit
> `AtomicLoad#`/`AtomicStore#`/`AtomicAdd#` intrinsics remain for address-based
> access).
>
> > **2026-09-06 (atomic ordering).** An atomic field takes an ORDERING — a
> > strategy keyword before `atomic`, or none for the default:
> >
> > ```briev
> > relaxed atomic count: Int;    // memory_order_relaxed — no ordering
> > acquire atomic ready: Bool;   // memory_order_acquire — read barrier
> > release atomic done: Bool;    // memory_order_release — write barrier
> > bartered atomic refs: Int;    // memory_order_acq_rel — RMW exchange
> > seq atomic strict: Int;       // memory_order_seq_cst — explicit default
> > atomic plain: Int;            // identical to seq atomic
> > ```
> >
> > The ordering word is context-sensitive — it is valid only before
> > `atomic`. `seq` is the existing strategy keyword reused: sequential
> > consistency IS sequentialism, so no new word exists for the total-order
> > case. `bartered` names the acquire+release exchange — each side of an
> > RMW both gives visibility (release) and takes it (acquire); a barter
> > between threads. Field accesses and RMW lowering inherit the declared
> > ordering. The default (`seq_cst`) is unchanged from the pre-ordering
> > behavior — every existing atomic field compiles identically.
>
> > **2026-09-06 (ordering-parameterized atomic intrinsics).** The
> > address-based atomic intrinsics take an optional trailing ORDERING
> > argument from the same vocabulary — the words appear bare (no quotes,
> > no hashword):
> >
> > ```briev
> > AtomicLoad#(p, relaxed);                 // default seq_cst when absent
> > AtomicStore#(p, v, release);
> > AtomicAdd#(p, v, bartered);
> > AtomicCas#(p, old, new, bartered, relaxed);  // success, failure orderings
> > AtomicSub#(p, v);  AtomicOr#(p, v);  AtomicAnd#(p, v);  AtomicXor#(p, v);
> > AtomicLoadN#(p, 4, acquire);    // width-parameterized: 1/2/4/8 bytes
> > AtomicStoreN#(p, v, 1, relaxed);
> > Fence#(acquire);                // default seq_cst when absent
> > ```
> >
> > Ordering arguments are consumed as ordering markers — they are not
> > values and name nothing at runtime. Check mode ignores ordering
> > (single-threaded); the words surface as unknown identifiers anywhere
> > except an atomic intrinsic's ordering positions.
>
> **2026-08-13 (`union`).** `union Name { field: Type, … };` is an untagged
> overlay: all fields share storage at offset 0; size is the largest aligned
> field storage and alignment the maximum field alignment. Sub-byte `Bit<N>`
> fields (N % 8 != 0) and zero-width padding are rejected — a bit-sliced
> overlay is ambiguous (deferred). The LLVM backend materializes a union as a
> byte array of its storage size with per-field loads/stores at offset 0
> (the C header exporter already renders all layouts as opaque byte arrays, so
> a union crosses the GLUE boundary identically). `union` is exclusive of
> `seq`/`pack`. The reference interpreter models structs as layout-free named
> products, so a union's fields behave as independent values there — the
> byte-overlay reinterpretation is a target concern.

Behavior for a struct is attached through an inherent `impl` in the defining module.

### 8.3 `enum`

An enum is a closed nominal sum. Its physical tag and payload layout are derived.

```briev
enum Result<T, E> {
    Ok(T),
    Err(E),
};
```

Enum behavior is attached through `impl`. Variant names are ordinary identifiers, not compiler keywords.

#### Variant construction

Variants are constructed by calling the variant name as a function:

```briev
term Ok(a / b);
term Err("division by zero");
```

Zero-payload variants (declared without parens) have Void payload and construct with zero arguments:

```briev
Null()
```

Multi-payload variants accept positional arguments stored as a Tuple payload. User-defined fns shadow variants — if a `defn` shares a name with a variant, the function wins. The typechecker binds the enum's type parameters positionally from payload arguments (`Ok(5)` under `Result<T,E>` binds T=Int); remaining params unify against the contextual expected type.

#### Qualified variant paths

A variant may always be constructed or matched through its qualified path:

```briev
let r: Result<Int, String> = Result::Ok(a / b);

term match r {
    Result::Ok(v) => v,
    Result::Err(msg) => 0 - 1,
};
```

Bare construction is ambiguous when two enums declare the same variant name (`Ok` under both `Http` and `Db`): *declaring* it is legal, but a bare call `Ok(5)` fails compilation naming every declaring enum and the qualification fix. The qualified form always resolves. Patterns accept either form and normalize on the variant's last segment.

### 8.4 Structural sums

```briev
Int | String
```

`A | B` is an anonymous structural sum type. It is matched with typed bindings:

```briev
match value {
    number: Int => use_int(number),
    text: String => use_string(text),
};
```

### 8.5 `type`

A type declares semantic identity, logical fields, invariants, metadata/layout hints, functions, and operation bindings.

```briev
type UserId: Int, Comparable<UserId>, Int {
    value: Int;
    [value >= 0]
```

> **2026-08-15 (Fundamentals).** The fundamental types (`Data`, `Bit<N>`,
> `Int`, `UInt`, `Float`, `Bool`, `Char`, `String`, `Blob`, `Ptr`, `Void`)
> are compiler-native primordials — they need **no** `type` declaration and
> carry no overloadable `op` (they are not overloaded; `op` is for user
> types). `Data` is the universal reflective floor — every value can be
> observed as its raw storage; it is not a supertype. `Bit<N>` is the unified
> bit type; `Blob` is the `[len][bytes]` byte buffer.
> See `docs/plans/2026-08-15-fundamentals-as-types.md`.

The relationship list after `:` may contain:

- at most one parent type;
- explicit trait assertions;
- explicit protocol membership.

#### Refinement parent

`type Child: Parent` is refinement-only inheritance.

- There is one parent maximum.
- The child strengthens semantic invariants.
- No layout, object state, lifecycle, or node graph is inherited.
- Child-to-parent conversion is safe erasure.
- Parent-to-child conversion requires proof or checked conversion.
- Overrides may not strengthen preconditions or weaken postconditions.

> **2026-09-02 (fundamental-parent membership):** when the parent (or any
> ancestor in the chain) is a fundamental (`Float`, `Int`, `String`, …),
> the child DERIVES that category's membership — protocol op bindings,
> cast paths, literal admission, and width semantics all follow from the
> declaration; no protocol-category restatement and no per-width arithmetic
> declarations are needed. `type Float16 : Float { spec MaxBits: 16; };`
> is a complete float-typed declaration: its arithmetic lowers
> shape-driven (`fadd half`) from `(Float, 16)`. A literal is admitted
> only when it round-trips through the declared width exactly — the
> precision contract narrows explicitly, never silently.

### 8.6 `trait`

A trait declares reusable behavioral requirements and defaults. It has no target-specific storage meaning.

```briev
trait Sized {
    Size: Int;
};

trait Comparable<T> {
    defn compare(left: Self, right: T) -> Int;

    op Equal(T): equal(#Lh, #Rh);
};
```

Traits may require:

- logical fields;
- function signatures;
- operation signatures;
- contracts;
- effects.

Traits may provide default functions and default operation bindings. Defaults do not establish conformance by themselves.

Conformance is inferred structurally when concrete behavior proves refinement of the trait's signatures, preconditions, postconditions, invariants, and effects. Explicit assertion documents intent and requests direct diagnostics.

Conflicting defaults require an explicit local resolution.

Runtime trait dispatch is explicit:

```briev
let value: dyn Printable = source;
```

Static trait-constrained generics are monomorphized by default.

Trait node templates never activate through inferred conformance. A type or object must import them explicitly.

### 8.7 `proto`

A protocol is a compiler-visible semantic category and cast-coherence domain. Protocols do not prescribe one layout.

```briev
proto UTF8: String {
    !> encoding: UTF8;
};
```

Protocol variants exist only when they change semantic interpretation or validity. Width, alignment, storage class, and target layout belong to frozen descriptors rather than protocol variants.

Every route within one protocol coherence domain must be proven functionally equivalent. The compiler may choose among equivalent routes by target cost.

Each written `as` may traverse:

1. any proven-equivalent path within the source protocol;
2. at most one explicitly declared cross-protocol edge;
3. any proven-equivalent path within the destination protocol.

Crossing multiple semantic protocol categories requires multiple written casts.

```briev
let bits = text as Bit;
let number = text as String as Int;
```

Missing proof evidence is an error unless the edge is declared as a trusted axiom. Axioms are declared, not assumed: the `axiom` contextual keyword prefixes the edge declaration, and the trust enters the verification ledger.

```briev
proto Posit32: Float {
    axiom CastTo(Float<IEEE754>)   = Posit32_to_IEEE754(#Lh);
    axiom CastFrom(Float<IEEE754>) = IEEE754_to_Posit32(#Lh);
};
```

An axiomatic edge must still name an existing binding (the `as` remains callable); what is taken on authority is the equivalence proof, not the route's existence. A round-trip obligation is discharged when either direction is axiomatic; the discharge is recorded as "axiom-discharged" in verification output. `axiom` is a contextual keyword: recognized only directly before `CastTo(`, `CastFrom(`, `defn`, `txn`, `node`, and `op`; in every other position it is an ordinary identifier.

### 8.8 `impl`

`impl` attaches inherent behavior to data-only nominal declarations and imported foreign shapes.

```briev
impl Point<Float> {
    op Add(Point<Float>): add_points(#Lh, #Rh);
};
```

The `axiom` prefix before an `op` binding marks the binding itself as
authoritative — taken on authority instead of derived — and enters the ledger
like every other declared trust site.

> **Lemma property lists are removed (2026-09-22).** SPEC previously allowed
> `op Add: func(#Lh, #Rh) [commutative];`. No parser accepted the grammar, no
> pass consumed the granted rights, and the float reassociation right was
> already available program-wide via `-ffast-math` and the `!> associative` /
> `!> fp_math: fast` function metadata. A user op's handler has a Briev body —
> commutativity can be *proven* rather than trusted; the axiom facility stays
> for the FFI-boundary cases where no body exists to discharge.

Inherent implementations may appear only in the target declaration's module. Explicit trait implementations obey ownership coherence: either the trait or target must be locally owned.

`type`, `obj`, and `cell` keep their cohesive behavior in their own declarations; `impl` does not arbitrarily split those definitions.

### 8.8.1 `trap`

`trap;` is a hardware abort. It is a never-type: the control flow past it is
dead, so it needs no value and unifies with any expected type. It is valid as
a statement, as a guarded (`when`) body, and as a `match`-arm body.

```briev
defn parse(tag: Int) -> Int {
    if tag > 4095 {
        trap;            // protocol violation — abort the process
    };
    term tag;
};
```

> **2026-08-13 (layout-keywords plan Phase 4).** The LLVM backend emits
> `call void @llvm.trap()` followed by `unreachable` (marking subsequent
> statements dead). The reference interpreter raises an abort diagnostic
> (`RuntimeError::Trap`), and the reactor stops on it — matching the process
> abort at runtime.

### 8.8.2 `halt`

`halt;` is the bare-metal stop — the program stops here; no further code
runs. It is a statement and trap's deliberate sibling: `trap` aborts because
something is wrong, `halt` stops because the work is done. Like `trap`, it
is a never-type: the control flow past it is dead.

- ARM/RISC-V target triples: a `wfi` (wait-for-interrupt) spin loop — the
  loop makes the halt survive spurious wakeups (a bare `wfi` returns on the
  next interrupt). On ARM Cortex-M this is the canonical sleep-forever.
- Any other triple (x86_64 host included): the `trap` abort — loud and
  portable; `wfi` would not assemble.
- The reference interpreter raises `RuntimeError::Halt` — check mode reports
  the stop, and the reactor does not fire further transactions.

```briev
isr<arm_cortex_m> handler @ 1: main_tick() [true][done == true] {
    done = true;
    halt;        // work finished — wait forever for the next interrupt
};
```

### 8.9 Metadata

`!>` is the canonical annotation operator for non-physical metadata.

```briev
type Int32: Int {
    !> ctd: Int;      // annotation (e.g. cell-tag dispatch)
};
```

`!> observable: true` is not valid; use the `out` modifier.

> **2026-08-13 (Deferred Layout).** Physical-layout metadata is declared with
> the **`spec`** keyword, not `!>`: `spec Alignment: 2;`, `spec Bits: 12;`,
> `spec MaxBits: 16;`, `spec Bytes: 4;`, `spec Endian: Big;` (see §8.2). `spec`
> keys are PascalCase; values use the same grammar as `!>` values. `!>` remains
> the annotation operator for non-physical metadata (`ctd`, `accel`, …); it no
> longer carries physical-layout keys. `spec` and `!>` share one metadata map —
> a physical key spelled with `!>` still parses for backward compatibility
> (stdlib and examples migrated to `spec` in 2026-08-13), but `spec`
> supersedes it; an unknown `spec` name is an error, never silently accepted.

At module top level, `!>` binds metadata to the module as a whole.
Top-level `!>` is a shortcut for attaching metadata to the script; it never
attaches to the following declaration.

```briev
!> accel: try_all;
```

Metadata keys and values are lowercase. Multiple top-level `!>` bindings merge
into one module metadata map (last binding wins per key). Values use the same
grammar as declaration metadata (identifier, integer, boolean, string, or
list). Module metadata is available to any backend or plugin that consults
the metadata vocabulary.

### 8.10 `coll` — compiler-owned Length semantics

> **2026-08-15 (`coll`).** The **native strategy keyword for declaring
> collections** (prefix on `obj` and `struct` declarations, order-independent
> with `pack`/`seq`). A `coll` type is observable as raw storage through the
> `Data` reflective floor like every other type. Declaring a
> collection is convenient — the author writes the storage shape (the one
> sequence member) and the compiler owns
> the rest: **compiler-owned Length semantics** (length and capacity are
> hidden compiler-managed slots, never declared fields) and a **scaffolded
> operation surface** (§11.4/§15.2) — `op Count`, `op At`,
> `op Init`/`op InsertAt`/`op ExtractFrom`, literal construction, `foreach`,
> `.^Length`, `Count#`, and the default `op Grow`/`op Shrink` growth
> strategies. Iterability is still resolved through the operation surface;
> `coll` synthesizes those operations. A `coll` collection is **as fast as
> the compiler can make it**: the scaffolded ops fold to the same code a
> hand-written collection emits (the default is always the efficient path —
> a `coll`-beaten default is a compiler bug). See
> `docs/plans/2026-08-15-coll-length-semantics.md`.

```briev
coll obj List<T> {
    inner: ListBuffer<T>;   // data: Ptr<T> — the sequence member; cap is compiler-owned
};

coll struct Fixed<T, N> {
    data: T[N];             // fixed T[N] only — length == capacity == N
};
```

- **Length and capacity are compiler properties** — exposed only through
  `.^Length` (stored length, §17.1), `Count#` (element count), and the
  capacity intrinsics `Capacity#`/`Resize#`/`EnsureCap#`/`TrimCap#` (§15.2).
  They are not declared fields; a `coll` type declaring a `len` or `cap`
  slot is an error.
- **`coll obj`** — the compiler appends hidden `cap` and `len` slots and
  scaffolds `op Count`, `op At`, `op Init`/`op InitEmpty`/`op InsertAt`/
  `op ExtractFrom`, plus the default `op Grow`/`op Shrink` growth strategies.
- **Grow-on-full is the default behavior (2026-08-15):** when an insertion
  would exceed capacity (`len == cap`), the compiler's default `op Grow`
  doubles the capacity before the element is stored — an insert past capacity
  is never an out-of-bounds write and never requires the author to call
  `Resize#`/`EnsureCap#` first. A type overrides the policy with its own
  `op Grow` handle-only binding (§15.2).
- **`coll struct`** — fixed `T[N]` only (this slice): length == capacity
  == N, no hidden slots, C ABI preserved. `.^Length` and `Capacity#` both
  return N (a compile-time constant; §17.1). `Ptr<T>`-backed structs are a
  documented follow-up. **Literal construction (2026-08-16):** `let f: Fixed =
  [1,2,3,4]` stores the elements DIRECTLY into the inline `data: T[N]` array —
  the value's layout IS the struct (`%Fixed = type { [4 x i64] }`), with no
  `[len]` heap-seq header and no cap/len slots. The literal must not exceed N
  (an over-length literal is a compile error); an empty `[]` constructs a
  zero-filled N-array. Iteration, `op Count` (= N), `op At` (= `data[i]`),
  and field reads all observe the same inline layout. A **generic**
  `coll struct Fixed<T, N>` application (`Fixed<Int, 4>`) resolves the const
  dimension to a concrete `Int[4]` at monomorphization — the same inline
  construction, bound, and op surface apply.
- **Storage is the compiler's choice** (2026-08-15): the compiler picks the
  most effective representation for each coll from its shape and use — heap
  block (growable `Ptr<T>`), inline array (fixed `T[N]`), or pooled columns
  (fixed `T[N]` named instance). A `coll` is a promise the compiler
  optimizes, not a fixed layout. **`seq coll` adds one hard constraint: the
  elements sit in a single contiguous memory block** — for a `Ptr<T>` coll
  the data buffer IS one block (a hard guarantee of what the shape already
  gives); for a fixed `T[N]` coll `seq` forbids the columnar/pooled layout
  (inline array only).
- **`op Grow`/`op Shrink` are overridable strategy bindings** (handle-only,
  `op Grow: grow(#Lh)`); a binding replaces the compiler's default doubling
  policy. This is the same binding machinery as `op InsertAt`/`op ExtractFrom`
  (§15.3) — a custom Grow/Shrink may rehash-and-expand (e.g. a hash map)
  without the compiler knowing anything about hashing.
- **No `.^Capacity` reflection** — capacity is operational (the Grow/Shrink
  control knob), so it is an intrinsic (`Capacity#`), never a reflection
  target (§17 boundary rule).

## 9. Functions, transactions, nodes, objects, and cells

### 9.1 Functions

```briev
defn add(left: Int, right: Int) -> Int [true][term == left + right] {
    term left + right;
};
```

`term expression;` completes the current callable or convergence step. Briev has no `return` keyword.

A body-less internal signature uses `defn` with its contracts/effects and is staged until an implementation is supplied. There is no `sig` declaration.

**2026-08-14 (generic `defn f<T>`):** a definition may declare type
parameters — `defn first<T>(xs: List<T>) -> T`. At each call site the
compiler infers the type params from the arguments (`first(items)` where
`items: List<Int>` binds `T = Int`) and substitutes them into the parameter
and return types. Codegen is **type-erased**: the body is emitted once, and
a `T`-typed value is a boxed i64 (matching the boxed ABI); there is no
per-instantiation code duplication. A nullary generic (`new_stack<T>()`)
binds its type param from the enclosing binding's annotation when the
arguments do not constrain it. An argument that cannot unify with the
generic parameter shape is a type error.

### 9.2 Callable types and closures

Callable types use signature shape directly:

```briev
let transform: (Int) -> Int = value => value + 1;
```

Named functions and transactions may be used as values when their effect and ownership requirements fit the expected callable type.

Closures capture lexical bindings. Captures participate in ownership, lifetime, and effect checking.

### 9.3 Transactions

```briev
txn increment()[counter < Max][counter >= 0] {
    counter = counter + 1;
    term;
};
```

Prior-state `@value` references (for postconditions such as
`counter == @counter + 1`) are **staged**: the `@` raw-literal prefix is
removed, and prior-state expression syntax is not yet implemented. Until then,
transactions must express their postconditions without prior-state reads.

A transaction is atomic with respect to its declared state transition. `rollback;` or `rollback reason;` aborts and reverts the current transaction/reactive firing.

```briev
when invalid_input {
    rollback InvalidInput;
};
```

`rollback` is invalid outside rollback-capable transaction/reactive contexts.

### 9.4 Nodes

A node is a reactive transition that may fire when its precondition is satisfied.

```briev
node update [pending][!pending] {
    pending = false;
    term;
};
```

The keyword is `node`; `rct` is not source syntax.

A node's precondition declares **eligibility**; its postcondition declares
**completion**. The compiler derives the causal wiring between nodes from
the contracts (who enables whom) and refuses any reactive cycle whose
nodes declare no completion — a cycle that cannot be shown to quiesce
carries no liveness obligation and does not compile. `--explain-causality`
prints the derived wiring ("what fires into what") for inspection.

### 9.5 Objects

An object owns identity, lifecycle, logical state, ports, and reactive behavior in its parent reactor.

```briev
obj Enemy(damage: Event<Damage>) -> died: Event<EnemyId> {
    health: Int;

    node apply_damage()[damage.Ready][health >= 0] {
        health = health - damage.amount;
        term;
    };
};
```

Objects use traits and composition rather than parent inheritance.

#### Port semantics

Input ports expose two operations:

- **`port.^Ready`** → Bool — runtime reflection on the port's internal state flag. True when a pending event is observable.
- **`port.field`** → payload member projection. Falls through to the payload type's declared fields (`damage.amount` where damage is `Event<Damage>`).

Output ports fire via ArrowAssign: `died <- value;` sets the shared slot's Ready flag and stores the payload. Wired consumers observe the same slot (shared storage — identity, not copy). Delivery order is deterministic — scheduler order, no implicit concurrency.

**Firing wakes blocked tasks (§12.2).** A task reading a payload member off
an unready port does not error — it suspends (`Waiting`) and registers as a
waiter on that slot. Firing drains the waiter list, re-marking each blocked
task schedulable. Wake is level-triggered: slot values are stable once
written, so re-entry converges. Reads outside any task keep the strict
error — top-level code gates on `.^Ready`.

The native backend executes this contract identically: a wire is an `i64`
slot handle into the runtime event table; obj spawning allocates the
instance's output-port slots in its own pool row and binds input ports
positional to caller-supplied wires (a wire IS an integer). An `Event<T>`
parameter fed a plain value wraps it — a fresh slot allocated and fired
immediately, so the callee observes an already-ready event. Ports-only objs
(no members) participate as pools whose columns are exactly their ports.

Cells enforce sealing: external references to cell internals fail at compile time; only declared ports are externally visible.

### 9.6 Cells

A cell is a sealed state machine with an independent convergence membrane.

```briev
cell Timer(period: Duration) -> tick: Event {
    // owned state and internal nodes
};
```

- Communication occurs only through declared ports.
- Internal state is not externally visible.
- Cells and objects share input `(...)` and named output `->` syntax.
- Multiple outputs form a complete named product on every target.

Cells instantiate and dispatch like objects: `spawn CellName(args)` wires
input ports to shared event handles, creates fresh unready output slots,
and defaults internal state; `instance.txn(...)` calls the cell's own
members. Firing a cell's output port wakes tasks blocked reading it
(§12.2) — a cell is an ordinary participant in the reference scheduler.
`trg name @ source;` inside a cell parses into its internal trigger list;
trigger scheduling semantics are staged (typed ports are their
replacement).

### 9.7 Acceleration (`accel`)

`accel` is a keyword that may prefix a `node` or `txn` declaration.
It marks a native counted loop as a *parallel map over work-items*: the work
is expressed as an ordinary loop over a real counter, and the compiler may
coalesce the loop into one GPU dispatch of N work-items.

**Design A — the counter is a real state field, never virtual.** The program
declares and initializes the work-item counter explicitly (`let i: Int = 0;`),
bounds it with a counted-loop contract, and advances it in the body. No
compiler-synthesized variables: everything the loop does is visible in source.

```briev
let i: Int = 0;                       // work-item counter, explicit init
accel node force [i < nbodies][i == nbodies] {
    dv[i] = force_on(i);              // per-work-item compute
    i = i + 1;                        // native counted-loop advance
    term;
};
```

- The precondition `[i < N]` is the loop bound and the firing gate; the
  postcondition `[i == N]` is the goal ("loop until true"). Both reference the
  real counter — valid runtime checks, never an undefined variable.
- The compiler **proves** the counted loop is a parallel map: `i` is the
  counter (incremented in the body), every write targets a slot affine in `i`
  (disjoint across work-items), shared reads are permitted, and value types are
  flat. An unproven body is silently kept on the CPU path with a remark.

**Dispatch.** On the GPU path, one dispatch of N work-items replaces the
N-firing loop: the runtime launches the kernel (the work-item id is the
counter value) and fast-forwards the counter to N so the loop's bound is met
after a single firing. On the CPU path, the loop runs natively — each firing
is one work-item. Cross-work-item data exchange is permitted only through
host-sequenced separate `accel` declarations, never within a single firing.

**Verification requirement.** In try modes, GPU deferral happens only when
the compiler verifies a speedup. The compiler must prove eligibility (bound,
write disjointness, flat value types, purity) and then either

- prove statically, for a compile-time-known N, that N exceeds the device
  crossover, or
- emit a runtime auto-tuning probe that measures both the CPU and the GPU path
  at program start, checks output equality within tolerance, and commits to
  the faster path.

If eligibility cannot be proven or the speedup is not verified, execution
uses the CPU path. An ineligible or unverified `accel` body is never an error
by itself, but a **keyword-marked** body that stays on the CPU path always
emits a default compile-time remark (one line naming the body and the reason —
proof failure or unverified speedup). `!> accel_report: verbose;` adds the
full per-analysis detail; the one-line remark is not opt-outable.

**Force mode.** Under `!> accel: force;` or `!> accel: try_all_force;`, an
`accel`-keyword-marked body must offload: eligibility must be provable or the
compiler rejects the program, the speedup gate is skipped (the developer
asserts GPU wins), and a missing GPU at runtime is a runtime error — never a
silent CPU fallback.

**Module shortcut.** Top-level module metadata `!> accel:` (§8.9) takes
lowercase policy values:

- `try_all` — every eligible body in the module is a candidate, verified;
- `force` — `accel`-keyword bodies must offload (see Force mode);
- `try_all_force` — every body is tried and keyword bodies are forced.

Absent means only bodies carrying the `accel` keyword are candidates, in try
mode. There is no `off` value; absence is the default. `!> accel_report:
verbose;` is a separate observability key that emits an optimization remark
for every analyzed body. The keyword and the module shortcut feed the same
verification pipeline.


### 9.8 GPU kernel synthesis tiers (2026-09-01)

When an `accel` body is proven eligible, the SPIR-V backend selects the
most efficient kernel form from the proven shape — never by keyword, never
by user request. Three tiers exist, plus a planned fourth; each tier's
recognition conditions are exact, and any body not matching a tier lowers
to the plain work-item kernel (always correct).

**Tier 0 — work-item (flat).** One invocation per work-item, local size
256. The counter binds to `GlobalInvocationId`. Every eligible body lands
here at minimum.

**Tier 1 — cooperative row.** A `foreach k in 0..K` whose body is a float
mul-add into a local accumulator, the counter used BARE as the row index
(`y[i] = ...`, `a[i*K + k]` — no `i / N`, no `i % N` anywhere in the
kernel statements), and `K` a literal multiple of 32. The workgroup IS
the row's team: local size 32, lane-strided accumulation
(`lane + t*32`, or `lane*4 + t*128` when vec4-eligible), one
`OpGroupNonUniformFAdd` reduce, one store per row. Requires the
`spirv_row_cooperative` lowering knob (on by default).

**Tier 2 — tiled GEMM.** The counter DECOMPOSED (`m = i / N`,
`n = i % N`) over a canonical dot-product body
`acc = acc + a[m*K + k] * b[k*N + n]`, with `M`, `N`, `K` literals all
divisible by 64. Synthesized: one workgroup per 64×64 output tile
(local size 16×16), A/B k-panels staged through Workgroup shared memory
(two barriers per panel), 4×4 register tiles per invocation, loop-carried
accumulator phis. Falls back to Tier 0 for any non-matching body.

**Tier 3 — cooperative matrix (tensor).** LANDED 2026-09-02 (plan
`2026-09-01-m2-tensor-cores`); smem staging default + f16acc sub-tier
2026-09-04. Operand types choose the hardware path: `Bit`-rooted
**Float16** array fields qualify the GEMM for
`VK_KHR_cooperative_matrix` tensor-core lowering (f16 operands,
16×16×16 fragments). Float32 fields keep the Tier 2 exact kernel — the
3060-class hardware exposes no f32×f32 tensor shape, so tensor
precision is a TYPE-LEVEL decision the author makes, never a silent
compiler trade. Gate: the device extension must be present at runtime;
absent devices take the Tier 2 kernel. Two disclosed sub-forms:
- **smem staging** (default, `spirv_coopmat_smem: 1`): scalar fills
  stage A/B k-panels through Workgroup arrays; cooperative-matrix
  loads read from the staging arrays; 2-stage double-buffer pipeline,
  one barrier pair per panel. 0 = direct SSBO fragment loads (the
  pre-staging form; validated +23% slower at 8192³, parity at 4096³ —
  ledger 2026-09-04b).
- **f16 accumulate** (`spirv_coopmat_f16acc`, gate ≤1e-2 vs the
  f32-acc tier, which stays the correctness reference): the mma
  accumulates in f16 — the Ampere double-pump class. The staged
  pipeline's dispatch geometry follows the SAME dispatch predicate as
  Tier 2 (`(M*N / (16*R*64)) * 32`, R = `spirv_coopmat_tile_rows`,
  default 4 — sweep-confirmed optimal, ledger 2026-09-04b).

**Tier 4 — native backend tier (planned).** Per-vendor native codegen
as a projection of the SAME frontend plans: a Briev-owned PTX emitter
(`mma.sync`/`ldmatrix`/`cp.async`) for NVIDIA tensor-class workloads,
selected by device probe, with the portable Tier 0–3 path remaining
the base tier and the fallback. Motivation is measured, not assumed:
the portable ceiling through vendor SPIR-V lowering is a structural
variable (the f16acc-vs-f32acc delta bounds it); the coopmat ceiling
microkernel measures it per driver era and the number gates this
tier's mandate. Doctrine and tier architecture:
`docs/architecture/abv-gpu-doctrine.md`; campaign:
`docs/plans/2026-09-04-beyond-coopmat.md`.

**Vec4 eligibility gate.** A field participates in wide loads only when
its element is 32-bit float-shaped, its count is a multiple of 4, its
projection offset is 16-byte aligned, AND the load index is provably
4-divisible after substitution (`expr_provably_mod4_zero`: products prove
via either factor, sums via both, division/modulo reject). Unproven
fields load scalar — never speculatively.

**Projection layout rule.** Device projections are name-sorted, packed,
EXCEPT vec4-eligible arrays aligned up to 16B
(`FnLowerer::projection_offsets` — the single definition; kernel member
offsets, runner literals, and the runtime's declared `proj_offset` all
derive from it). Host layouts stay packed; the runtime copies per field.

**Dispatch geometry contract.** The kernel's work mapping and the
launcher's grid derive from ONE predicate per tier
(`is_cooperative_shape`, the tiled plan match) — kernel emission and
runner dispatch can never disagree. Grids are X-flattened (this driver's
Y dispatch dimension is inert); tile coordinates decode from
`gl_WorkGroupID.x`.

**Launch modes.** Per-call synchronous launches pay a fence-wake tax
(~33µs measured on this class of device) on top of ~7µs submission; loop
deployments use batched submission (K identical dispatches, one fence).
Batched launches require launch-invariant host scalar state — the
cooperative counter reset satisfies this by construction.

## 10. Contracts, invariants, and watchdogs

### 10.1 Contracts

Callable and transition contracts use precondition/postcondition brackets:

```briev
defn divide(a: Int, b: Int) -> Int [b != 0][term * b == a] {
    term a / b;
};
```

Omitted clauses retain implicit provenance. The compiler must distinguish omission from an explicitly written tautology.

Explicit `[true][true]` is invalid everywhere: it asserts nothing (`true ⇒ true` is trivial), so it is indistinguishable from an omitted contract and records no obligation.

The `axiom` contextual keyword may prefix a callable or transition declaration. Its effect is scoped to the contract: preconditions remain fully proven, while postconditions are taken on authority — discharged by declaration instead of proof. The postcondition stays visible and usable to every caller (range narrowing, bounds extraction consume it unchanged); what is skipped is only the author's own discharge obligation. The explicit-tautology rejection above applies unchanged under authority: `axiom defn f() [true][true];` is still invalid. Every authoritative contract enters the verification ledger.

```briev
axiom defn codec(x: Int) -> Int [x >= 0][result <= x * 2];
```

Contracts are **mandatory** (present and non-trivial) on `node`, `txn`, and `asm` declarations: the reactor uses the pre/post pair to prove and classify the transition. `defn` contracts are optional; `cell` declarations do not require a contract.

Type invariants must be proven across construction and every mutating transformation.

### 10.2 Inline guards and gates

```briev
[ready] process();
[converged];
```

- `[condition] statement;` guards one statement.
- `[condition];` is a convergence gate.
- `[condition] { ... }` is invalid; use `when` for blocks.

### 10.x Liveness checks

```briev
check <expr>;
```

A liveness check asserts that `<expr>` holds at this point in execution. It serves three roles:

1. **Compile-time proof**: if the solver proves `expr` from known facts (contracts, prior checks), the check is eliminated — zero cost.
2. **Compile-time rejection**: if the solver DISPROVES `expr`, compilation fails with a diagnostic explaining under which conditions the check would be violated.
3. **Runtime assertion**: for unprovable loops, the check evaluates at that point in execution. Failure triggers rollback (same as escape).

After a successful check, the solver records `expr` as a known fact, strengthening downstream proofs and enabling further optimization.

`check` may appear in any function body (`defn`, `txn`, `node`). In looping contexts, it evaluates every pass through that point. In non-looping defns, it documents and verifies the programmer's assumptions about the input domain.

### 10.3 Watchdogs

Watchdogs occupy a dedicated grammar slot after a contract.

```briev
txn poll()[ready][done] ?[progress] within 10ms -> on_timeout() {
    // body
};

txn critical()[ready][done] ![progress] within 100cyc {
    // body
};
```

- `?[condition]`: optional watchdog enforcement.
- `![condition]`: required watchdog enforcement.
- Canonical duration units are `cyc`, `ns`, `ms`, `s`, and `min`.

The watchdog `?`/`!` forms are contextual and do not conflict with expression propagation or compile-time expansion.

## 11. Control flow

### 11.1 No `if`/`else`

Briev has no `if` or `else`. Conditional branching uses exhaustive `match`. One-sided guarded execution uses `when` or inline guards.

### 11.2 `when`

```briev
when ready {
    process();
};
```

A `when` block has no implicit complementary branch.

### 11.3 `match`

```briev
match value {
    Result::Ok(result) => use(result),
    Result::Err(error) when error.retryable => retry(error),
};
```

- Arms use `=>`.
- Guards use `when`, never `if`.
- Closed sums and enums require exhaustive coverage.
- Open or unknown alternatives require `_ =>`.
- Unreachable arms are errors.
- All expression arms must have compatible result types.
- Patterns include enum variants (`Result::Ok(result)`), tuple patterns
  matching member-wise against a tuple scrutinee (`(true, false) => …`),
  typed sum bindings (`number: Int => …`), literals (integer, string,
  Bool), ranges, and `_`. Alternatives within one arm use `|`; the first
  match wins.

Arm bodies may use block expressions with statements and a tail value:

```briev
match res {
    Ok(val) => {
        let doubled = val * 2;
        doubled
    }
    Err(msg) => { Print#(msg); 0 - 1 }
};
```

A trailing expression without `;` is the block's implicit value.
Zero-payload variant patterns use `Variant()` (with parens) to distinguish
from variable binding patterns.

In `node` and `txn` bodies, `term expr;` (or bare `term;`) means **"check
if this loop is complete."** It is a convergence checkpoint: the reactor
evaluates the goal (postcondition) at this point. If satisfied, the node
stops firing; otherwise it fires again on the next reactor cycle. Without
`term`, the reactor has no point at which to evaluate the goal and decide
whether to continue or converge — which is why it is mandatory in looping
bodies.

In `defn` bodies and match-arm blocks, a trailing expression WITHOUT `;`
is the implicit return value. No convergence check occurs because defns
and match arms are not loops. The two forms are semantically distinct:
`term` invokes a completeness evaluation; a bare tail expression produces
a value.

### 11.4 Iteration

`foreach` is the sole iteration keyword.

```briev
foreach(item in items) {
    consume(item);
};
```

The parens are legacy and tolerated; the binding is the `in` keyword. The
canonical form is parenless — matching the `()`-means-application delimiter
rule and Rust/Python/C++ range-for:

```briev
foreach item in items {
    consume(item);
};
```

There are no `for`, `while`, or `loop` keywords. Counted iteration uses iterable ranges or reactive/transactional structure.

`foreach` is exhaustive by default. A `break;` inside a `foreach` body exits
the innermost enclosing `foreach` immediately — the honest form for
search-until-found probes. It is an **exit form of `foreach`**, not a
`for`/`while`/`loop` keyword. `break` is invalid outside a `foreach` (a
compile error).

```briev
foreach q in 0..cap {
    when keys[q] == target {
        slot = q;
        break;
    };
};
```

> **2026-08-17.** `break` was added for `HashMap` linear-probe termination;
> it complements the reactive/transactional exit that `node`/`txn`
> postcondition convergence provides, at the intra-body grain.

#### 11.4.1 Iteration is structural

A type is iterable if, and only if, it provides the iteration operations
below. Satisfaction is **structural** — there is no `Iterable` keyword, no
conformance marker, and no compiler-known collection type. A type that
provides the operations is iterable; one that does not is rejected with a
compile-time error, never a panic and never a silently skipped loop.

> **2026-08-15 (`coll`).** A `coll` declaration (§8.10) does not grant
> iterability by conformance — it instructs the compiler to *synthesize* the
> iteration operations (`op Count`/`op At`), after which the same structural
> probe resolves. A `coll` type is iterable because it provides the
> operations, exactly like a hand-written one.

Iteration operations are declared **op-as-member**: the operator name is the
member name (`op Count() -> Int { … }`), disclosed by the `op` keyword, and no
bare member name is resolved by the compiler.

The iteration contract has two tiers, selected per type:

- **Tier 2 — Random access sequence.** `op Count() -> Int` and
  `op At(i: Int) -> &T`. `foreach` lowers to a counted `0..Count` loop that
  reads each element through `op At` — eligible for vectorization. Used by
  `List<T>`, `Stack<T>`, inline vectors, and fixed-width `String`.
- **Tier 1 — General iterable.** `op Iter() -> Cursor` returns the first
  element's cursor (or the end sentinel); `op Step(cur) -> Cursor` advances to
  the next element (or the sentinel); `op IsEnd(cur) -> Bool` is true past the
  end; `op Current(cur) -> &T` reads the element at the cursor. `foreach`
  lowers to an external stack cursor loop:
  `let cur = c.iter(); while !is_end(cur) { item = current(cur); …; cur = step(cur); }`.
  Re-iterable, reentrant, zero heap. Used by `HashMap<K,V>`, `LinkedList<T>`,
  streams, and variable-width `String`.
  *(2026-08-12: the cursor + IsEnd + Current form supersedes the original
  `op Step(cur) -> Option<&T>` plan — Option/union returns do not codegen
  natively yet; the cursor form is equivalent and implementable.)*

`foreach` uses the best available tier (Tier 2 when both are present).

Iteration yields **references** (`&T`), not copies. The loop variable binds
the reference; the body reads through it, and an explicit copy is required to
materialize a value. This keeps iteration zero-cost for large or inline
elements.

Indexed read `c[i]` resolves `op At` (Tier 2). `foreach` lowering uses the
iteration operations internally; it never consults the reflection surface.
A collection's logical length is its element count, expressed by the
`Count#` intrinsic (which dispatches to the type's declared `op Count`); the
reflection target `.^Length` (§17.1) is stored-length reflection and is not
the collection-length mechanism. **2026-08-14 (Universal Operation Language,
§6b):** every operation has three invocation surfaces — a symbol (`+`, `c[i]`,
`<-`, `foreach`), an intrinsic (`OpName#(a, b)`), and the UFCS method form
(`a.OpName#(b)`). `OpName#` is recognized for any disclosed operation
identity; `a.OpName#(b)` desugars to `OpName#(a, b)` when no literal member
exists (member wins, then UFCS). Symbols are sugar over the same dispatch.
Metadata that is compiler-known but non-operational is reflection. Transfer
`c <- x` / `x <- c` resolves the mutation operators (§15.3).

**2026-09-16 (universal chaining):** every `.`-suffixed call — method, `#`
intrinsic, `$` compile-time navigation, `!` plugin, `^`/`^^` reflection —
obeys one rule: *the right-side operation applies to the left-side result.*
The suffix is a naming convention selecting the dispatch mechanism (member →
intrinsic → plugin → UFCS fallback); the receiver is preserved as the
operation's input. `a.Nav$(x)` and `Nav$(a, x)` are the same call; a chained
plugin `a.serialize!(x)` carries the receiver. Priority: member, then
operation/intrinsic (`#`), then plugin intercept (`!`), then UFCS fallback.

**Chain captures and back-references (§11.4.1):**

```briev
data.parse() >> input          // capture result as `input`, chain continues
    .validate() >> valid
    .emit(valid, input);

variable
    .op()                      // result 1
    .2>>secondOp()             // secondOp(op_result, variable) — .2 = 2 ops back
    .step_1,1>>combined()      // combined(prev, step_1) — named + positional refs
```

- `expr >> name` captures the chain result into `name`, available for later
  `.name>>` references. A capture does not break the chain.
- `.N>>func(a)` passes the result N operations back as a **leading argument**:
  `func(receiver, <ref>, a)` — the receiver binds `self` (member) or arg 0
  (UFCS), the resolved references precede the written args. `.1` is the
  immediately previous result (the implicit receiver); `.2` is two back.
- `.(Type)>>func()` casts the previous result to `Type` before the call.
- `>>` is a capture only at a chain position (followed by `.`, `;`, `}`, or
  expression end); inside `f(x >> y)` it remains the shift operator. `.N>>`/
  `.name>>` are back-references only when followed by a call head
  (`identifier (`).
- A literal receiver directly before `.N>>` (e.g. `5.1>>f()`) lexes as a
  float; parenthesize (`(5).1>>f()`) or use a named expression.

#### 11.4.2 `String` is `Iterable<Char>`

`String` is a fundamental (`Data`-refining) type — a `[len][bytes]` buffer
interpreted as UTF-8, with no declared fields (§16.2); its layout and
encoding are derived by the casting graph. It satisfies the iteration
contract structurally as a sequence of `Char` with encoding variants
(`String<UTF8>`, `String<UTF16>`, `String<ASCII>`), selecting tiers by
encoding. **2026-08-14 (current): a `String` operand iterates `Char` through
a protocol-keyed char-decode lane** — the loop bound is the stored byte
length (`.^Length` header) and each iteration decodes one UTF8 codepoint
(`Char`); the compiler holds no String layout. `foreach c in str` binds `c`
as `Char` (SPEC §17.2 `String` → `Char`), never a raw byte.
Encoding-selective tiers are the future specialization:

- **Fixed-width encodings** (e.g. `ASCII`): Tier 2 — `op Count` = char count
  (equal to byte count), `op At(i)` is an O(1) byte load.
- **Variable-width encodings** (e.g. `UTF8`): Tier 1 for characters — the
  cursor ops (`op Iter`/`op Step`/`op IsEnd`/`op Current`) decode 1–4-byte
  `Char`s; Tier 2 is available on the byte view (`.Bytes`, a `Slice<U8>`/`Blob`
  over the underlying buffer). Character random access by index on a
  variable-width encoding is a compile error, never a silent O(N) surprise.

`.^Length` on a `String` is the **stored byte count** (header read). The
**character count** is a computed property and is an intrinsic operation —
`CharCount#` — never a reflection target.

### 11.5 Local and process completion

- `term expression;`: complete the current callable/transition.
- `endprogram;`: complete the process boundary normally.
- `endprogram code;`: complete the process with an exit code.
- `defer { ... };`: register cleanup for the enclosing scope.

`endprogram` (formerly `exit program`, and before that the removed `term!`)
runs applicable `defer` cleanup and terminates the process. Unlike `term`,
which only ends the current transaction, `endprogram` exits the process even
when a node's precondition remains satisfiable. Abrupt termination is not
currently a source-language feature.

There is no `main` declaration in Briev. The program entry is either an
explicit `beginprogram` node (§11.5.1) or, absent one, whichever reactive
node fires first: the reactor evaluates node preconditions and the first
satisfiable one fires. A program converges (and exits) when no node can fire.

Beneath the reactor sits the **machine entry** (`bootstrap node`, §13.2): an
authored pre-reactor sequence — set up the machine, seed the initial state —
whose handoff postcondition is proven from its typed stores. On native
targets the compiler emits the canned equivalent (set stack, zero `.bss`,
start the reactor) when no bootstrap is declared. The layering:

- `bootstrap node` — machine entry, pre-reactor (authored or canned);
- `beginprogram` node — first reactor dispatch (§11.5.1): sugar for a
  bootstrapping state variable, genuinely useful on native targets where the
  environment provides the initial state;
- ordinary nodes — reactor dispatch thereafter.

#### 11.5.1 Entry loops (`beginprogram`)

`beginprogram` is a keyword usable as a conjunct in a node's precondition:
`[beginprogram && <state>][<goal>]`. It is a pure marker — true exactly once
at program start — and takes no conditions itself. The node's other
precondition terms are ordinary state expressions over top-level bindings
seeded from the environment or compile time at startup:

```briev
let startingnumber: Int = get_env_int!("env_var");

node entry1 [beginprogram && startingnumber == 1][done] {
    done = 1;
    term;
};
```

A `beginprogram` node is an **entry loop**:

- It is entered exactly once at program start when its state conditions hold.
- The precondition is evaluated once at entry and **never re-checked** during
  the loop.
- The node itself is a loop: the body runs repeatedly until the postcondition
  (goal) is met.
- The goal must be **provably reachable**: a counter comparison whose body
  advances the counter toward the bound, or `[true]` (a single pass). A goal
  that cannot be proven reachable is a compile error.
- At most one `beginprogram` node may be eligible at program start: the
  compiler proves the entry conditions are mutually exclusive. Unprovable
  overlap is a compile error.

`beginprogram` is scoped to `node` declarations. It remains first-class
sugar: on native programs it is the idiomatic "runs first" marker. In
programs with an authored bootstrap (§13.2) the handoff state usually makes
the marker redundant — the bootstrap seeds state such that the next logical
node's precondition already holds — and it may be omitted.

### 11.6 Critical sections

```briev
mutex {
    update_shared_state();
};
```

- `mutex { ... }` is a critical section.
- The `barrier` statement is removed (2026-09-22): it was a no-op wrapper that
  emitted its body inline everywhere and its group name was never consulted.
  Real convergence is expressed by the `[condition];` gate (§10.2) and group
  classification by `sync<group>` (§12.1).
- `sync<group>` is reserved for node classification.

## 12. Concurrency and task lifecycle

### 12.1 No implicit concurrency

If two reactive nodes may fire simultaneously and have no XOR read/write dependency, both must be classified:

- `async` on both, acknowledging simultaneous firing; or
- `sync<group>` on both, establishing a group barrier.

An unclassified eligible pair is a compile-time error.

### 12.2 `spawn` and `await`

```briev
let task = spawn compute(input);
let result = await task;
```

`async` is not a statement-level call modifier. Legacy `async call` and `async await` forms do not exist.

`spawn` creates a persistent task or component instance and returns a linear owned handle. Spawning a callable yields a value of type `Task<R>`, where R is the callable's declared result; `await` unwraps it.

- `await task` consumes a task handle and returns the callable's declared result.
- `free task` requests cancellation/stop and runs `defer` cleanup.
- `keep task` transfers the handle to the enclosing owner/boundary.
- Silently dropping or discarding a live handle is an error.

#### Execution model

Spawn **captures** the function and its evaluated arguments but does NOT execute the body. Bodies are split at cancellation points into segments. The first `await` triggers round-robin segment execution of all non-Done tasks until the target reaches Done.

Deterministic interleaving at yield boundaries: tasks execute one segment per scheduling pass, in spawn order. Single-threaded scheduler — no data races, no nondeterministic ordering.

**Only parameters carry across a segment boundary.** A `let` bound before
`yield;` is not visible in later segments — bind the values a segment needs
as parameters, or recompute them after the boundary. This is what makes
compiled and interpreted task semantics identical: every segment lowers to
a plain function of the task's arguments.

**Blocking port reads are suspension points.** A spawned task that reads a
payload member off an unready event port suspends at that read (status
`Waiting`); it consumes no further scheduling passes until some task fires
the awaited slot, which re-marks it schedulable (§9.5). Registration splits
task bodies so a blocking read heads its own segment — statements before
the read run exactly once, and the post-wake re-run starts AT the read.
A read inside a `foreach` body sits behind the whole loop statement: gate
loop reads with `.^Ready` checks so gated re-entry converges instead of
repeating iterations.

If every remaining task is blocked, no producer will ever fire: `await`
returns the handle unchanged instead of hanging (cooperative scheduler —
no preemption, no deadlock detection; a blocked cycle is a program error).

The native backend keeps this model byte-for-byte in structure: each task
lowers to one function per segment taking an argv block (the parameters —
the only values that cross boundaries), and `await` drives the same
round-robin over a C task table. A blocked segment returns its blocked
aggregate; the cursor stays at the read; the post-wake re-run restarts it.
Event wiring lives in the same runtime table on both paths, so compiled
and interpreted programs interleave identically at every boundary.

`free task` before any await prevents execution entirely (the body never runs). After await has started execution, free sets a cancellation flag checked at each yield boundary. A freed task never resurrects: a later fire on its port skips it.

**Obj-instance spawn storage classes.** `spawn Obj(...)` allocates the
instance from the obj's static pool column (the default, provably
inexhaustible). The storage-strategy markers (§8.1) classify the spawn
explicitly where the pool decoder cannot choose a single best strategy:

```briev
let h = box   spawn Counter();   // heap-per-instance, not a pooled column
let h = spill spawn Counter();   // may grow beyond a static pool column
```

`box`/`spill` are contextual keywords — recognized only immediately before
`spawn`, and legal identifiers elsewhere. A non-pooled spawn consumes no pool
capacity and never triggers the unprovable-spawn error (the user opted out of
the pooled column explicitly).

A **cancellation point** is a `yield;` statement or a `term;` — the places
where stopping a task is safe by construction. `free task` is valid when
the spawned callable's body contains at least one cancellation point,
checked structurally through guards and blocks. Foreign calls are never
interruption points, so active FFI is never cancelled mid-call.

`yield;` marks a cooperative cancellation point inside a function body.
It completes no value and changes no control flow; in the reference
scheduler it executes as nothing and grows into the actual suspension
point of the concurrent scheduler. `yield` is a contextual keyword — legal
as an identifier elsewhere. It may appear in any function body; in a body
never spawned it draws an advisory warning.

### 12.3 Reference scheduling

The reference interpreter uses a deterministic semantic scheduler for normal execution. Verification mode explores all legal interleavings. Host-thread nondeterminism does not define language meaning.

## 13. Triggers and external events

The reactive input keyword is `trg`.

```briev
trg input_ready @ device;
```

`@` binds a trigger to its source. Trigger source forms are target/profile validated. Typed event ports on `obj`/`cell` declarations are the staged replacement for a typed trigger surface.

### 13.1 Addressed triggers are MMIO input pins (2026-08-27)

A trigger with a numeric address (`trg sensor @ 0x1000;`) is an MMIO INPUT
pin whose VALUE is a readable `Int` in transaction and definition bodies on
every target:

- native (`ll`): the read lowers to a `volatile` load at the
  static address through the boxed-pointer ABI (`VolatileLoad#` shares the
  same convention for computed pointers);
- silicon (`sbv`): the pin becomes an `@top` input port; ports emit
  ADDRESS-SORTED so separately compiled partitions agree on bus layout.

Pins are driven by hardware — programs only observe them. Assignment to an
@-addressed trigger is a compile error (declare a separate output `let`
field, or use `VolatileStore#` over a computed pointer for output
registers). Dynamic (`@ *ptr`) and symbolic address forms have no static
pin: on `sbv` they are capability errors; on native surfaces they flow
through the existing pointer/deref paths.

`trg!` does not exist. Local asynchronous suspension uses ports, nodes, spawned tasks, and `await`.

Event fairness assumptions belong to explicit event-port contracts. There is no global `#assume_event` pragma.

### 13.2 Machine-serviced event nodes (2026-09-14; the `isr` keyword, retired)

A node wired to a hardware vector is fired by the machine, not the reactor.
The `@` is Briev's hardware-association delimiter (§13.1 addresses; here the
interrupts namespace), resolving to a literal slot index or a board
`interrupts.dbvl` name:

```briev
node tim2_irq @ 0x1C [acked == false][acked == true] { ack_timer(); };
node timer_tick @ timer_irq [ticks >= 0] { ticks = ticks + 1; };  // board name
```

- **Mechanism inference**: the active target profile's `isr_mechanism`
  (briev.toml `[target.<name>]`) names the mechanism row
  (`config/isr-targets.dbvl`). The compiler never invents a layout — with no
  profile default the error names the profile key to set.
- **Wiring classes** — the board-file namespace of the `@` reference decides
  how the node is dispatched:
  - *interrupts namespace* (`interrupts.dbvl` names, literal slot numbers):
    machine-vectored — the machine preempts and enters the node through the
    mechanism's entry convention; the node is never reactor-dispatched and
    takes no `within` bound (its latency is the hardware's).
  - *addresses namespace* (`addresses.dbvl` names, address literals,
    `@ *ptr`): reactor-pass — the node stays in reactor dispatch, gated by
    the wiring (an inline trigger; the wiring is the eligibility, so `[pre]`
    may be omitted). Dynamic pointers are always this class.
    2026-09-14 (plan `2026-09-14-rv64-finish.md` Phase 4b): the `@ *<ptr>`
    form is SHIPPED — parses as a reactor-pass Transaction carrying the
    `address_wired` marker, and the equilibrium park spins (never `wfi`)
    while such a node exists (an external frontier changes without an
    interrupt). `addresses.dbvl` names and bare address literals remain
    pending (literal disambiguation against vector slots is unresolved).
- **Latency contracts**: a memory-mapped value changing does not notify the
  CPU, so reactor-pass nodes fire within one dispatch pass. A declared bound
  moves the equilibrium tradeoff into the open: `node n @ alert within 1 ms`
  (watchdog deadline syntax) makes the compiler meet the bound — spin, or
  park with a timer quantum no larger than it. The no-bound default is
  deterministic: re-evaluate every pass.
- **Frontier-driven equilibrium**: the frontend computes, per wake source,
  which preconditions that source can affect (typed writer sets × reader
  sets, conservative). Programs whose only frontiers are vectored park in
  `wfi` with no periodic evaluation and re-check only the trap's dependent
  set on wake; state-sequenced next steps are chained without re-checks
  (proven fallthrough); only an external frontier re-reads hardware, every
  pass by default. Omitted checks are proven unable to change.
- **Entry convention**: the mechanism row supplies the scaffold around the
  body. The default convention is the machine's own partial save; the
  `full_context` convention saves the full register file + sp to the
  compiler-owned `@__briev_trap_frame` (pinned, documented ABI —
  `docs/architecture/machine-entry.md`), runs the body on a kernel stack,
  restores, and returns with the mechanism's return instruction.
- **Table mechanisms** (link-time tables): the compiler derives the table
  from the declared handler set — gaps bind the mechanism's default handler
  (a spin loop), the table lands in the mechanism's linker section, and the
  SP slot (ARM convention) is reserved. The Thumb bit is a linker semantic.
- **Body obligations** are proven at compile time: no allocation, no
  spawn/threading/dynamic-linking, no floating point unless the mechanism's
  `fpu_context` row stacks FP context, bounded frame. The body shares the
  program state — an interrupt-servicing program's state is a global, and
  the reactor and every handler operate on the same instance.
- **Contracts are mandatory** — ordinary state obligations, solver-checked;
  delegation obligations live on the called transactions.
- Machine-fired nodes are excluded from reactor dispatch and the concurrency
  classification (§12.1), and are always live. One vector, one handler — a
  duplicate slot is a compile error.

## 14. Ownership, lifetimes, and effects

### 14.1 Universal ownership algebra

Boundary and callable ownership uses:

- `borrow`: caller retains ownership; callee cannot retain beyond the call;
- `consume`: ownership transfers to the callee;
- `owned`: caller receives ownership;
- `borrowed<source>`: returned lifetime is bounded by a named input;
- `shared`: ownership uses a declared retain/release policy.

```briev
frgn parse(
    borrow input: Ptr<Byte>,
    consume arena: Arena
) -> owned Node from #System;

frgn view(borrow source: Buffer) -> borrowed<source> Slice from #System;
```

Allocation and destruction policy is configured rather than hardcoded into the ownership keyword. Read/write permission belongs to effects.

These five words are **strategy keywords** (§8.1): `borrow`/`consume`/`owned`/
`shared` are the program-independent *category* — their meaning is intrinsic and
must compile even with `--no-stdlib` and no configuration; `borrowed<source>` is
the mechanism form, where `<>` carries the lifetime source input. What each
category permits at a boundary (retain-after-call, who frees, exclusivity
obligation) resolves through the shared mechanism registry
(`config/alloc-strategies.dbvl` and per-category rows), the same way allocation
policy does for owned results and consumed inputs.

### 14.2 Pointer safety

- Pointer types are `Ptr` and `Ptr<T>`.
- `&value` requests addressability and returns a pointer.
- `*pointer` dereferences.
- Dangling pointers are hard errors in every profile.
- Mutable access requires proven exclusive provenance.
- Intentional shared mutation requires atomic/synchronization behavior or a cell boundary.

There is no `Ptr!` alias and no `.^Address` acquisition form.

> **2026-09-06 (pointer arithmetic).** Pointer arithmetic exists as
> compiler-known intrinsics, never as bare operators on `Ptr<T>`:
>
> | Intrinsic | Shape | Semantics |
> |-----------|-------|-----------|
> | `PtrAdd#(p, n)` | `(Ptr<T>, Int) -> Ptr<T>` | `p` advanced `n` ELEMENTS (`getelementptr inbounds`); out-of-bounds is undefined behavior, caught by LLVM's `inbounds` contract |
> | `PtrSub#(p, n)` | `(Ptr<T>, Int) -> Ptr<T>` | `p` moved back `n` elements |
> | `PtrDiff#(a, b)` | `(Ptr<T>, Ptr<T>) -> Int` | element distance between two pointers — both must derive from the SAME allocation (a cross-allocation diff is meaningless, and the caller owns the proof) |
> | `PtrEq#(a, b)` | `(Ptr<T>, Ptr<T>) -> Bool` | address equality |
> | `PtrLt#(a, b)` | `(Ptr<T>, Ptr<T>) -> Bool` | address ordering (unsigned) |
>
> Pointers cross the ABI as boxed handles, so `PtrAdd#`/`PtrSub#` return
> boxed handles like every other pointer value. There are no `ptr + int`
> operators — arithmetic on addresses is always an explicit, named
> operation (disclosed special treatment, Rule of the `#` marker).

### 14.3 `free` and `keep`

```briev
free value;
keep value;
```

- Proven last use may be scheduled automatically.
- `free` requests a release point and requires proof of no later or aliased use.
- `keep` transfers the value to boundary/owner lifetime.
- Unresolved lifetime is a warning in normal profiles and a hard error in `.s`.
- A boundary collector may service warned unresolved lifetimes in normal profiles.

### 14.4 Unified effects

The frontend infers one effect set covering at least:

- reads and writes;
- allocation and release;
- spawn and await;
- FFI and I/O;
- blocking;
- purity;
- cancellation behavior.

Traits and contracts may constrain effects. Compile-time reflection exposes `.^^Effects`.

## 15. Expressions and operations

### 15.1 Operation dispatch

Syntax maps to operation identities, which types bind to functions.

```briev
type Number: Int {
    op Add(Number): add(#Lh, #Rh);
};
```

Compiler-known operand hashwords include:

- `#Lh`: left operand;
- `#Rh`: right operand;
- `term`: result placeholder in post-conditions (`[term == left + right]`);
- `#T`: type parameter;
- `#Self`: semantic self-reference.

The compiler knows operation identities but not stdlib collection type names.

### 15.2 Operator classes

Implementations may bind the following syntax families:

- arithmetic: `+`, `-`, `*`, `/`, `%`;
- comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`;
- Boolean: `&&`, `||`, prefix `!`;
- bitwise: `&`, `|`, `^`, `~`, `<<`, `>>`;
- indexing/slicing: `[]`;
- transfer: `<-`, `~<-`;
- assignment/update forms such as `+=` where supported.

List<T> + List<T> concatenates (both operands must be `Applied("List", [T])` with matching element types); it resolves as an intrinsic binding (`list_concat`). Other collection types resolve through an ordinary operation binding.

Operations are declared **op-as-member**: the operator name is a member name
on the type (`op Count() -> Int { … }`, `op At(i: Int) -> &T { … }`), disclosed
by the `op` keyword. The compiler resolves the operator and inlines the
member body; it never resolves a bare user-facing member name as a semantic
key. The iteration operators are:

- `op Count() -> Int` — Tier-2 element count;
- `op At(i: Int) -> &T` — Tier-2 indexed read (a borrow, not a copy);
- `op Iter() -> Cursor` — Tier-1 iterator creation;
- `op Step(cur) -> Cursor` — Tier-1 cursor advance (returns the next cursor or
  the end sentinel);
- `op IsEnd(cur) -> Bool` — Tier-1 cursor exhaustion test;
- `op Current(cur) -> &T` — Tier-1 element at the cursor;
- `op InsertAt(v: T)`, `op ExtractFrom() -> T`, `op CopyFrom() -> T` —
  mutation and value-out (see §15.3);
- `op Init(v: T)` — type-directed construction (§16.3);
- `op Grow`, `op Shrink` — **capacity strategy bindings** (handle-only,
  `op Grow: grow(#Lh)`); a `coll` type may override the compiler's default
  growth policy with these. They are strategy entries like `InsertAt`, not
  member bodies. The default growth policy fires **automatically** when an
  insertion would exceed capacity — the capacity doubles before the store
  (grow-on-full, §8.10); an override's binding is called in its place. The
  capacity control intrinsics are `Capacity#(h)`, `Resize#(h, cap)`,
  `EnsureCap#(h, n)`, `TrimCap#(h)` (compiler-owned capacity; §8.10).

The indexing family distinguishes read from extract: bracket read `[]`
resolves `op At` (a borrow); the transfer arrow `<-` resolves the extraction
operators (a value out).

### 15.3 Transfer arrows

```briev
list <- value;
value <- list[index];
value ~<- list[index];
map[key] <- value;
```

The complete semantic shape of insertion/extraction is carried to the resolved operation binding. The compiler does not hardcode `List`, `Map`, `Entry`, stack, or queue behavior.

Insertion `c <- x` resolves the type's `op InsertAt`; extraction `x <- c`
resolves `op ExtractFrom` (destructive) or `op CopyFrom` (value out). Both are
op-as-member declarations (§15.2).

### 15.4 Portable SIMD (2026-09-06)

The portable SIMD family is element-wise arithmetic over POINTERS — forced
vectorization with a deterministic shape, for the cases where the
auto-vectorizer's profitability heuristics decline (notably: destination
may alias a source):

```briev
SimdAdd#(dst, a, b, count);        // dst[i] = a[i] + b[i]
SimdSub#(dst, a, b, count);        // dst[i] = a[i] - b[i]
SimdMul#(dst, a, b, count);        // dst[i] = a[i] * b[i]
SimdFma#(dst, a, b, c, count);     // dst[i] = a[i] * b[i] + c[i]
```

- Every pointer argument must be `Ptr<scalar>` (a type whose universe entry
  has no fields — int/float/bit shapes); element-wise arithmetic on a struct
  pointee is a compile error.
- The element shape derives from the DESTINATION pointee's storage: `Float`
  lowers as `<4 x float>` chunks, `Float64` as `<2 x double>`, fixed-width
  ints by byte width (`i64`/`i32`/`i16`/`i8` word shapes). The values are
  added AS STORED — a `Bit<12>` field in a 2-byte container adds as `i16`.
- Chunking is overlap-safe: within each chunk, all loads precede the store,
  so `dst` may fully alias `a`/`b`/`c`. The results are exactly the
  element-wise map.
- Constant counts lower to straight-line vector chunks plus an inline scalar
  tail; runtime counts lower to a counted chunk loop plus a scalar tail loop.
- Portability is structural: the chunk shapes are legal LLVM IR on every
  target — ISel lowers them to the best vector unit (AVX/SSE/NEON) or
  scalarizes where none exists. No target names appear in source.
  `SimdFma#` emits fused-pair arithmetic under fast-math; the backend
  contracts to hardware FMA where the target provides it.
- Check mode (the reference interpreter) evaluates word-wise element loops —
  single-threaded, no vector shape.

There is no `simd`/`nosimd` keyword and no explicit vector type: the default
already vectorizes every counted loop, `seq` is the sequentialism opt-out,
and these intrinsics are the forced-shape escape hatch.

### 15.5 Precedence

From highest to lowest:

1. application, indexing, field access, reflection;
2. prefix operators and postfix propagation/expansion;
3. multiplicative operators;
4. additive operators;
5. shifts;
6. comparisons;
7. equality;
8. bitwise operators;
9. Boolean `&&`, then `||`;
10. ranges;
11. transfers and assignment.

## 16. Literals, ranges, and slicing

### 16.1 Numeric literals

Normative numeric forms are:

- decimal integers and floats;
- float exponent form, C-style maximal munch: `1.0e-8`, `1e5`, `65536e-16` —
  the exponent (`e`/`E`, optional `+`/`-`) binds to the literal, never the
  surrounding expression; `1e + 5` (spaced) remains arithmetic;
- hexadecimal `0x`;
- binary `0b`;
- octal `0o`;
- canonical duration suffixes.

Physical width is expressed through type annotation or cast, not `i32`, `u8`, or `f64` lexer tokens.

Custom parse-prefix/suffix bindings are not currently exposed. Unknown prefixes and suffixes are errors.

### 16.2 Strings and bytes

```briev
"escaped string"
#r"raw \ text"
#b"\x89PNG\r\n"
```

- `#r`: raw string; escapes are not interpreted.
- `#b`: byte literal; byte escapes are interpreted.
- Formatting/interpolation uses explicit compile-time expansion such as `format!(...)`.
- Lexer-level interpolated strings and adjacent tagged literals do not exist.

### 16.3 Ordered and associative literals

```briev
[1, 2, 3]
["one" => 1, "two" => 2]
```

Both are type-directed. Associative literals lower through expected-type construction/insertion behavior; they do not imply a compiler-known hash map.

An ordered literal lowers to the expected type's `op Init` (first element) and
`op InsertAt` (remaining elements):

```briev
let x: Stack<Int> = [1, 2, 3];
// lowers to: op Init(1); op InsertAt(2); op InsertAt(3);
```

> **2026-08-15 (`coll`).** For a `coll` type the `op Init`/`op InsertAt`
> ops are scaffolded (§8.10), so `let xs: List<Int> = [1,2,3]` constructs
> through the scaffolded ops — the compiler owns the produced layout. Every
> `[]` literal constructs a fresh value via `op InitEmpty` (pre-allocated
> capacity); there is no shared empty sentinel.

An associative literal lowers through the expected type's construction and
insertion bindings. A literal with no observable expected type
(`let x = [1, 2, 3]` with `x` unannotated) is a compile error — type
annotation required for an unconstrained collection literal.

There is no universal `null` or `nil`. Absence uses an ordinary sum variant such as `Option::None`. The `Blob` byte buffer is never null: it always carries its `[len]`, so an empty Blob is a valid empty value, not a null pointer. `Blob` is the safe universal byte-carrier — it holds raw bytes with no interpretation; it does not signify absence.

### 16.4 Ranges

- `start..end`: half-open range.
- `start..=end`: inclusive range.
- `...`: multidimensional slice ellipsis only.

### 16.5 Python-style slicing

```briev
tensor[start:stop:step, ..., time => 5, width => 0:10]
array[mask]
array[start:stop][mask]
```

- Slice coordinates use `start:stop:step`.
- Named dimensions use `name => selector`.
- Boolean masks use ordinary mask indexing.
- The legacy `range; condition` slice form is invalid.

### 16.6 Fixed containment and const dimensions

```briev
Int[8]
Matrix<T, Rows, Cols>
```

`T[N]` expresses fixed containment. Const generics and dependent bounds permit dimensions to be compile-time parameters. Bounds are proven during specialization.

## 17. Reflection

### 17.1 Runtime reflection

```briev
value.^Field
```

Runtime reflection reads a declared/materialized logical field. Missing fields are compile-time errors for the selected program/target.

`value.^Length` is **stored-length reflection**: it reads a length that is an
intrinsic property of the value's representation and unreachable through the
declared member surface — the `Blob` byte header, the `String` byte header,
the `Vector` descriptor count, or a `coll` type's hidden length slot
(§8.10). It is valid only for those value kinds; on any other receiver (a
hand-written `List`, a `HashMap`, a custom collection) it is a compile-time
error, because that length is member-managed or computed, not intrinsic.
**Reflection never routes to an operation:** a length that must be computed
(for example a UTF8 character count) is an intrinsic — `CharCount#` — called
explicitly, not a reflection target (§17.3). Character-level String access
uses `char_at(s, i) -> Char` (stdlib), since raw indexing returns Int.

> **2026-08-15 (`coll`).** A `coll obj`'s `.^Length` is its hidden length
> slot (O(1) header read); a `coll struct`'s is its fixed constant `N`
> (folded at compile time). The element count is the `Count#` intrinsic,
> never `.^Length` — the two coincide when the stored unit is the element and
> diverge otherwise, exactly like String's stored bytes vs `CharCount#`.
>
> > **2026-08-16 (a hand-written collection obj).** A hand-written `obj`
> > (no `coll` keyword) is a collection VALUE when it declares the collection
> > op surface (`op Count`/`op InsertAt`/`op Init`/Tier-1 cursor ops) — the
> > compiler's dispatch keys on the ops, never the `coll` keyword or a type
> > name. A `HashMap<K,V>` (stdlib/collections.bv) is the reference example:
> > `let m: HashMap<K,V> = 0` constructs via `op Init`, `Count#` reads
> > `op Count`, `<-`/literals route through `op InsertAt`. `.^Length` stays a
> > compile error on it (no compiler-owned length).

> > **2026-08-23 (`.^Ready` on event ports).** An `Event<T>` port exposes
> > `.^Ready` → Bool — runtime reflection on the port's internal state flag.
> > True when a pending event is observable. Payload members project through
> > plain field access (`damage.amount`), NOT through reflection — `.^Ready`
> > reflects on the PORT; `.amount` reaches into the PAYLOAD.

> **2026-08-15 (boundary rule).** **Reflection (`.^`) = stored/frozen facts
> that "observe and never compute"; intrinsics (`X#`) = operations.** A value
> with one notion that is operational is an intrinsic, not reflection.
> Capacity is that case: it is the Grow/Shrink control knob, so it is
> `Capacity#`/`Resize#`/`EnsureCap#`/`TrimCap#` — **no `.^Capacity`
> reflection exists.**

> **2026-08-15 (the hierarchy).** `Data` is the universal reflective floor —
> every value can be observed and reflected as its raw storage (the
> treat-as-bits view); "parent" is a reflective category, never a supertype
> edge in the casting graph. `Bit<N>` is the unified bit
> type at any declared width (every type is composed of bits; `Bit<N>` names
> a run directly — there is no separate `Bits`). `Blob` is the
> `[len][bytes]` byte buffer, a `Data` member like `String` but with no
> encoding interpretation. Absence is `Option::None` (§16.3); Blob is never
> null — it always carries its length. See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.
>
> > **2026-08-16 (`Bit<N>` ↔ `Bits` unification).** A bare `Bits` is the
> > FLEXIBLE bit type (`Type::Bits(0)`): it accepts a value of any `Bit<N>`,
> > and a declared `Bit<N>` pins an inferred flexible width. This is the
> > "unified bit type" fact — there is no separate `Bits` type, only the
> > width-0 flexible form. (A runtime cast FROM a flexible-width `Bits`
> > value to a specific width is a separate limitation: the value's runtime
> > width is not tracked.)

`value[i]` resolves the receiver type's `op At` (an indexed borrow, §15.2).

### 17.2 Compile-time descriptor reflection

```briev
value.^^Type
value.^^Ops
value.^^Bytes
value.^^Alignment
value.^^Effects
```

Compile-time reflection occurs after semantic/layout freezing. Reflection-driven specialization may inspect the frozen descriptor but may not introduce new layout requirements that invalidate the freeze.

Descriptor fields include, where applicable:

- `Type`, `Ops`, `Effects`;
- `Bytes`, `Bits`, `MaxBits`, `Alignment`, `Endian`, `StorageClass`, `AddressSpace`, `Addressable`;
- declaration metadata `Name`, `Params`, `Returns`, `Arity`, `Loc`, `FnSpan`, `Doc`, `Hash`, `Contracts`, `Module`, `IsPure`.

`value.^^Element` is the element type of an iterable receiver. **2026-08-14:
single-source proof form** — the element type IS the read op's return type
(`op At` Tier 2 / `op Current` Tier 1, §11.4) substituted with the concrete
generic args (`List<String>` → `String`, `HashMap<K,V>` → `V`), or the frozen
`String` → `Char` fundamental fact. There is no second derivation to drift
against; a non-iterable receiver is a compile error, never a silent `Int`.
The descriptor folds to the element type's category code at compile time.
Iteration capability is inspected via `value.^^Ops` (the type's operator
surface, §15.2).

Declaration/source metadata is compile-time-only unless explicitly materialized.

`Alignment` and `Endian` describe a selected materialization, not a universal property of an abstract type.

### 17.3 Transformations are not projections

`Absolute`, `BitReverse`, `Popcount`, `LeadingZeros`, and `TrailingZeros` are
**explicit intrinsics** — `Abs#`, `BitReverse#`, `Popcount#`, `LeadingZeros#`,
`TrailingZeros#` (SPEC §18 intrinsics, dispatch to `llvm.abs`/`llvm.bitreverse`/
`llvm.ctpop`/`llvm.ctlz`/`llvm.cttz`) — not universal projection names and not
reflection targets. A computed truth is an intrinsic; reflection only observes
stored/frozen facts. **2026-08-14:** the reflection target `.^Absolute` was
removed after a one-release deprecated alias — it is now an unknown-target
error directing to `Abs#`.

`Values` and `Elements` are ordinary logical fields when declared. `AsStack` and `AsQueue` are type-defined conversions.

## 18. Compile-time execution and macros

### 18.1 Compile-time-only bindings

```briev
$const Limit = 32;
$let current = 0;
$defn build(...) { ... };
```

`$` declarations exist only during compilation and are erased before runtime.

### 18.2 Expansion

```briev
format!("value: {}", value)
regex!(#r"[a-z]+")
```

`name!(...)` performs explicit compile-time expansion. Its arguments may follow macro-specific, noncanonical syntax because the compile-time expansion defines their parse contract.

Privileged macros declare capabilities at definition. Calls still use `name!(...)`; `$!name` does not exist.

**`execute_many!`** — repeated application with heterogeneous literal blocks
(2026-09-18, **retired 2026-09-22**): superseded by the variadic composite
form below — the composite reaches the same behavior in the language, so the
Rust-side macro is removed. The block shape is retained by the composite:

```briev
execute_many!(callee, block₁, block₂, …);
// block := "(" [expr ("," expr)*] ")"
```

**Variadic composites (2026-09-22).** A `$defn`/`$txn` may declare a final
TypeScript-style rest parameter (`...name: Expr`) that binds ALL trailing
call-site arguments as a compile-time list. It is the sanctioned compile-time
iteration channel: a `foreach` over the rest name unrolls at expansion, one
emission per element. A plain `Expr` parameter is a RUNTIME quantity and
never unrolls; only a rest parameter declares compile-time iteration.

```briev
$defn execute_many(...calls: Expr) {
    foreach c in calls { c; }
};
// execute_many!(f(a), f(b), f(c)) → f(a); f(b); f(c);
```

Rules:
- `...` must be the FINAL parameter.
- Compile-time-only: a runtime `defn` declaring `...` is an error.
- Each call-site argument is emitted in order (sequential calls, order
  guaranteed).
- Zero trailing arguments is a mistake, not a no-op — the expansion errors
  naming the composite.
- The rest name is an exposed binder (hygiene, §18.7): the loop variable is
  body-local, not a caller capture.
- Composites expand BEFORE typecheck: typecheck, contracts, and both
  backends see the emitted calls as if hand-written.

**Value-returning composites (2026-09-22).** A composite declaring `-> Type`
may be invoked in EXPRESSION position (`let r = f!(x)`, `r = f!(x)`,
`g(f!(x))`, a match scrutinee or arm). The expansion produces an
`Expr::Block` whose value is the composite's return: the body's trailing
`term v` becomes the block's value statement, never a function return, so
the caller's node is not truncated. A value-position call of a composite
whose body yields no value is an error naming the composite. Statement
position (the body as a splice) remains the default for body-only
composites; the two are distinguished by whether the call site needs a
value.

```briev
$defn aligned_size(n: Expr) -> Int {
    term (n + 15) & ~15;
};
let r: Int = aligned_size!(4);   // r = 20 — computed at the call site
```

### 18.3 Stages

```briev
$(Parsed) { ... }
$(Allocated) { ... }
```

Stage blocks state when compile-time work executes. Stage vocabulary is compiler-known and exact.

### 18.4 `$txn` — the convergent loop (2026-09-22)

`$txn name(params) [pre][post] -> Type { body }` declares a compile-time
CONVERGENT loop: the body runs repeatedly until `[post]` holds, at most a
compiler-bound iteration limit. The iteration count is UNKNOWN at the call
site — decided by the computation — the one compile-time loop `foreach`
cannot express (a `foreach` needs a finite list; there is no comptime
`while`).

Rules:
- `$txn` declares at TOP LEVEL only. There is no inline `$txn`
  declaration. Inline INVOCATION (`$txn name(args)` from a `$(Stage)`
  block) is the call path.
- `[pre]` false → the loop is skipped (converged trivially). `[post]`
  true after a pass → converged.
- `term v` in the body is an EARLY RETURN (like `$defn`): the loop's value
  is `v` immediately — the guarded-return shape (`{ when x >= 50 { x =
  100; term x; }; }`).
- A body without `term` converges to `Void` when `[post]` holds; the final
  state lives in the caller's `$let` scope.
- Exceeding the iteration bound without convergence is a compile error
  naming the limit.
- `$txn` may declare a `...` rest parameter (SPEC §18.2 rules apply); the
  trailing arguments bind as the compile-time list.

```briev
$txn scale(x: Int) [x < 16][x >= 16] {
    x = x * 2;
};
```

### 18.5 Quotation

Quotation and interpolation operate on AST values during compile time. They must preserve hygiene unless an explicit compiler capability requests generated names.

### 18.6 Derivation

`:=` introduces compile-time derivation/synthesis examples or a reference implementation.

```briev
defn parity(x: Int) -> Bool
    := { 0 => false; 1 => true; }
    := parity_reference;
```

Generated behavior must satisfy the declared contracts and reference obligations. Derivation never weakens a contract.

### 18.7 `Error#` — compile-time failure (2026-08-17)

`Error#("message")` is a COMPILE-TIME failure, not a runtime value. Its
semantics follow reachability:

- **Reachable → the program does not compile.** The message is the
  diagnostic. A top-level `defn`/`txn` body that (statically) reaches an
  `Error#` is live code and fails immediately.
- **A MEMBER body's `Error#` is usage-gated.** Declaring a type whose op
  members contain `Error#` compiles — this is how a "sealed" collection (a
  `PiggyBank`) declares un-supported operations with a helpful message. The
  error PROMOTES (fails the compile) only when that member is actually
  invoked via a method call, a generative op (`Count#(x)`), an arrow
  extract (`x <- piggy` → the sealed `CopyFrom`), or the SYNTAX that
  consults a sealed op — indexing `piggy[0]` promotes `op At`, and
  `foreach x in piggy` promotes `op At`/`op Iter`. A member never called is
  provably unreachable — its `Error#` is dead code, eliminated.
- **Provably-dead branches** (constant-false conditions, statements after a
  `term`) do not record an error.
- Unprovable reachability resolves conservatively: if the compiler cannot
  prove the call is dead, it fails.

`Error#` returns nothing; a body ending in it typechecks as any return type
(`ReturnKind::Never`). It has no runtime meaning — the compile fails before
code generation.

The canonical use is a `PiggyBank` (stdlib/collections.bv): a WORM-like,
opaque, one-shot collection whose only in is `<-` (a declared `op InsertAt`)
and whose only out is `~<-` (a declared `op ExtractFrom`) that returns
everything and self-frees. Its other collection ops (`CopyFrom`, `At`, `Count`,
`Iter`) are declared as members whose bodies call `Error#` — so `piggy[0]`,
`foreach x in piggy`, `piggy.Count#()`, and `x <- piggy` each fail at compile
time with a message directing the user to smash the jar. This proves the
`#` intrinsic surface is op-driven: `Count#()` dispatches through the DECLARED
`op Count`, never implicitly reading a `count` field.

**2026-08-18 (Phase D):** the arrow's CONSUME flag selects the value-side op —
`dest <- src` (a non-destructive read) resolves `op CopyFrom`, while the
destructive `dest ~<- src` resolves `op ExtractFrom` (the other is the
fallback). Only a ZERO-PARAM member is a valid arrow target (the arrow
supplies no arguments — the coll scaffold's parameterized `get(i)` CopyFrom
can never read). A drain must therefore be the tilde form: `~<- queue`.

## 19. Foreign functions, export, and GLUE

Process/environment intrinsics: `Spawn#`, `SpawnWithOutput#`, `SetEnv#`,
`GetCwd#`, `ChDir#`, and `Barrier#` are compiler-known with **Briev-native
runtime** backing — they lower to `SysCall#` inline asm
(`fork`/`execve`/`getcwd`/`chdir`/`clock_gettime`) and compiler-captured
process state, never to a C runtime dependency (§3.4).

### 19.1 Foreign declaration

```briev
frgn local_name(
    borrow input: Ptr<Byte>,
    consume arena: Arena
) -> owned Node: external_symbol from #System;
```

The declaration name is the local Briev name. `:` binds a different external symbol. `as` is not an alias operator.

A `frgn` signature declares the actual Briev-visible return type. Foreign calls are never implicitly wrapped in `Result`.

GLUE configuration explicitly maps errno, status codes, exceptions, or delivery failures into `Result` when required.

### 19.2 Provenance

Exactly four provenance forms exist:

```briev
from "path"
from <configured/path>
from #Link<name>
from #System
```

- Quoted paths are exact/project-relative.
- Angle paths use ordered configured roots.
- `#Link<name>` declares a linker dependency.
- `#System` uses the selected system/runtime profile.

Platform families such as POSIX are target configuration, not source hashwords.

### 19.3 Optional symbols

```briev
optional frgn feature(...) -> T from #System;

when feature.^^Available {
    use(feature(...));
};
```

There is no `frgn?` or declaration-level `fallback` clause. Fallback behavior uses ordinary typed control flow.

### 19.4 Variadics

```briev
frgn log(format: String, variadic args: ForeignArgs) -> Void from #System;
```

A variadic foreign signature has an explicit final named variadic parameter. GLUE supplies ABI behavior. `...` is reserved for slicing.

### 19.5 Raw system transitions

Named system APIs use `frgn ... from #System`. Raw target-specific kernel transitions use an explicit intrinsic such as `SysCall#(...)`. There is no `syscall` keyword.

### 19.6 Foreign layouts

Exact foreign field order, width, alignment, calling convention, and release policy live in GLUE/Data Briev configuration.

There is no `meld` declaration. Foreign shapes adapt through configured descriptors, declared protocol cast edges, ownership contracts, and effects.

### 19.7 MMIO

`frgn name @ address` is invalid. Memory-mapped I/O uses configured device/cell ports or explicit pointer/address intrinsics.

### 19.8 Export

```briev
export defn add(left: Int, right: Int) -> Int {
    term left + right;
};
```

`export` is the sole export syntax. `#export` does not exist.

## 20. Assembly declarations

Inline assembly is declared with the `bad` keyword — a portable
assembly function whose body is real `.bad` grammar, compiled per-target
through the bad backend:

```briev
bad add_words(left: Int, right: Int) -> Int [result == left + right] {
    Add r0, r5, r4
}
```

`bad name(params) -> Ret [pre] [post] { body }` is an ordinary top-level
declaration. The body is compiled through the bad backend for the build
target, assembled to an object file, and linked into the binary; the LLVM
IR references the symbol via `declare`. Params bind per target through
the `abi_args`/`abi_args_fp` maps (Int → r-regs, Float → f-regs); the
body references the logical param names, which resolve to the target's
C-ABI registers. The trailing bracket is an implied postcondition — a
full Briev expression over the params and `result` (matching `.defn`
contract semantics); a leading group is the precondition. Call-site
contracts are checked by the ordinary contract machinery.

`bad` replaces the earlier `asm<target>` declaration (see §20.1 note);
`asm<Target>` is retained for backward compatibility and deprecated.

### 20.1 The .bad assembly dialect

`.bad` files are portable assembly programs compiled by their own backend
(`brievc bad <file.bad>`); they never enter the .bv pipeline. A universal
core ISA — integer arithmetic/logic/shifts, the signed and unsigned
compare-and-branch families, sub-width and offset memory ops, stack
pairs, and a double-precision FP register class with its own branch
family — is lowered per target through `config/bad-isa.dbvl`; portable
registers `r0`-`r15`, `sp`, `pc`, `f0`-`f15` map through
`config/bad-registers.dbvl` with caller/callee/ro proof properties and
width tokens (`.w8`/`.w16`/`.w32`). Target-specific optimization is
expressed as bare `target => ...` exception rows — attached to a single
instruction, or forming a `defn`'s branch table (`default =>` carries
the universal core syntax). `defn` bodies are inlined as-is.
Compile-time constants ride `.const NAME expr` and `.struct` layouts
through the comptime expression evaluator; local labels (`.name:`)
scope to their enclosing global label; `import "path.bad"` inlines
other .bad files at the import line. Contracts are **positional** — a
single bracket group is the postcondition (implied, matching `.bv`
function contracts), two groups are pre then post (`[r0 valid]
[r10 preserved]`); `[frame: N]` keeps its keyword (a different proof
kind) — and are proven at compile time from the register properties
(e.g. `[r10 preserved]` via
callee-saved status or balanced push/pop pairing; `[frame: N]` via
static sp tracking with 16-alignment at calls); proven proofs are
emitted as comments into the assembly. Friendly mnemonic aliases
(`Move`, `Add`, `JumpIfGreaterOrEqual`, …) load by default from
`std/bad/friendly.bad`; `brievc bad --raw` opts out (`_start` has no
alias — it is the universal entry). `;` separates instructions on one
line in every body context. Float literals ride a deduped `.rodata`
literal pool; `syscall` takes a NAMED kernel call (`syscall write, ...`)
whose per-target numbers live in config, routing the call through each
target's syscall ABI. `.export` names a C-ABI entry
point (argument registers per the `abi_args` map; args 7+ on the stack
at the `abi_stack_arg_base` offset); `--with-libc`
links libc; `--run` executes natively or under qemu per target with
cross toolchains from config. A pure-.bad stdlib lives in `std/bad/`. The grammar is
strictly line-oriented with no braces; the full dialect reference is
`docs/architecture/bad-dialect.md`.

## 21. Rendered Briev

### 21.1 Document structure

An `.rbv` document contains Briev source plus `<view>` and optional `<style>` blocks. Legacy `<script>` wrappers are invalid.

### 21.2 View attachment

```briev
render Counter {
    <button b-trigger:click="increment">
        <span b-text="count"></span>
    </button>
};
```

`render Name { ... }` is the sole attachment form. The compiler resolves whether `Name` is a struct, type, obj, or cell and applies the relevant visibility/lifecycle rules.

### 21.3 Components

Components ARE objects. `obj Name` owns the component's state slots and member
transactions; `render Name { ... }` is the view fragment bound to that obj. The
fragment's directives bind only the obj's slots and trigger only its member
transactions — `render Name` requires `obj Name`, and any other reference is a
compile error, never silently dead DOM.

There are two mount forms, split by ownership:

- **Briev-side instance — the program owns it.** `let c1: Counter = Counter { count: 5 };`
  creates an instance seeded in Briev (the literal's field values are the
  initial state; the frontend invents nothing). `<c1 />` mounts the fragment
  routed to that instance's slots (`count` → `c1.count`); its `b-trigger`
  fires the per-instance txn variant (`increment_c1`).
- **HTML-side spawn — the reactor owns it.** `<Counter />` spawns an anonymous,
  pool-indexed instance (`Counter.0.count`, `Counter.1.count`, …) with
  zero-init defaults. It is not referenceable by Briev code; only its txn
  variants touch it.

There are NO HTML props (`<Counter count="5" />` is invalid). All seeding is
Briev source.

The tag namespace resolves deterministically: a declared instance var
(`<c1 />`) mounts the instance; else a component type (`<Counter />`) spawns
an anonymous instance; else a lowercase HTML element; else an unknown-tag
warning. An instance var shadowing a component type name or a reserved HTML
element name is a compile error — the namespaces stay separated.

A `render Name { ... }` block is a reusable view fragment: a mount splices the
fragment's HTML at that position (each mount gets its own element IDs; nested
fragments mount recursively). A render cycle (`A` mounts `B` mounts `A`) is a
compile error.

Every instance is created at compile time as a fixed pool of dotted state
fields; each mount owns its own copy of the fields its fragment references,
plus per-mount variants of the member transactions that write them —
incrementing one counter does not move another.

`b-when` unmounting a subtree releases the component instances inside it: the
shim fires each instance's **reset txn** (`__reset_c1`, `__reset_Counter_0`),
a callable transaction that re-applies the instance's initial state (the Briev
seeds for a Briev-side instance, the type defaults for an HTML-side spawn).
The reset flows through the reactive machinery — its contract is carried and
its write set drives the flush, so the DOM updates immediately; a slot with
neither a seed nor a type default is a compile error, never silently left
stale.

### 21.4 Directives

Canonical directives include:

- `b-text`;
- `b-show`;
- `b-when`;
- `b-style`;
- `b-class`;
- `b-each:name`;
- `b-key`;
- `b-bind:value`;
- `b-trigger:event`.

`b-if` is invalid.

`b-when` structurally mounts/unmounts a subtree. `b-show` changes presentation only and preserves identity/state.

Dynamic repetition requires a stable `b-key` whenever children may be inserted, removed, or reordered.

`b-each` renders over **any structurally iterable** value (§11.4): a type that
provides the iteration operations — Tier 2 (`op Count`+`op At`), Tier 1
(`op Iter`+`op Step`+`op IsEnd`+`op Current`), or a `String` operand (chars)
— `List<T>`, `Stack<T>`, `HashMap<K,V>`, `String` (chars), vectors, or a
user-declared collection. It never depends on a compiler-known collection
type. The web backend materializes the iterable into an array by driving the
same iteration operations; the shim decodes each element by its type tag. A
view expression `.^Length` on an iterable field reads the materialized array's
stored length; a non-iterable `b-each` iterable is a compile error, never a
silently skipped render.

Reactive component nodes and view-event handlers obey the same no-implicit-concurrency rule as every other node: eligible simultaneous pairs require `async` or `sync<group>` classification.

`b-bind:value` accepts only an assignable logical field with a proven write contract. Computed expressions use separate value and trigger handlers.

The compiler resolves `b-bind:value`'s writer at build time: the target must be written by exactly one transaction (from the transition-graph write sets), and that transaction must take exactly one parameter — the input value is marshalled by that parameter's type. No writer, multiple writers, or a wrong-arity writer is a compile error, never an inert input.

### 21.5 View expressions

Every directive expression is canonical Briev, not a JavaScript-like mini-language. Ternaries and brace object literals are invalid.

View expressions are pure/read-only. Mutation, FFI, allocation, and spawning occur only in explicit event handlers or compiler-managed component lifecycle.

### 21.6 Web representations

View-bound values are not restricted to compiler-known primitive names. Web GLUE configuration supplies protocol casts and layout descriptors. Unsupported values are rejected by the target capability validator.

## 22. Data Briev

### 22.1 Shared principles

`.dbv` and `.dbvl` share one value/schema core but have distinct document grammars.

Values remain raw until interpreted by an asserted schema. Without a schema, arbitrary raw/scraped data is valid.

Quoted values are always supported.

### 22.2 `.dbv`

`.dbv` is structured/category data. `>` introduces entries under the current category.

```dbv
schema Person from "person.dbv";

> people
name: "Ada";
age: 37;
```

Schema imports use:

```dbv
schema Name from "file.dbv";
```

### 22.3 `.dbvl`

`.dbvl` is line-oriented. Each non-instruction physical line is exactly one record. Records may not span lines or merge across lines.

`>` introduces non-data instructions.

```dbvl
>schema Person from "person.dbv";
Ada,37
Grace,85
```

A schema key field derives the lookup key. Missing or duplicate keys are errors.

`.dbvl` supports lazy/streaming reads and append-only canonical writes.

### 22.4 Schema types

Canonical collection schema forms are:

```text
T[N]
List<T>
Map<K, V>
Option<T>
field?: T
```

`Vec[T]`, `T[]`, `Array<T>`, bare `Map`, and semicolon-separated generic arguments are invalid.

### 22.5 Validation

When a schema is asserted, validation covers:

- required and unknown fields;
- raw-token conversion;
- field types;
- constraints;
- named schemas;
- optional values;
- key presence and uniqueness.

### 22.6 Canonical serialization

Data Briev defines deterministic field/key ordering, quoting, numeric spelling, and instruction placement for reproducible builds and hashing.

`briev check file.dbv` and `briev check file.dbvl` select the correct parser mode and perform schema validation when asserted.

### 22.7 GLUE files

Human-authored per-language GLUE configuration is structured multiline `.dbv`:

```text
lib/glue/<language>/glue.dbv
```

Generated `bridge-exports.dbvl` remains line-oriented machine metadata.

## 23. Diagnostics, tooling, and documentation

### 23.1 Shared language manifest

Lexer vocabulary, LSP vocabulary, highlighter grammar, extensions, profiles, and reserved words derive from one machine-readable manifest.

### 23.2 LSP

The LSP uses the compiler's real Briev/Data Briev parsers and semantic analyses. It does not maintain an independent language grammar.

### 23.3 Formatter

The canonical formatter must satisfy parse-format-parse AST equivalence. SPEC examples and repository rewrites use that formatter.

### 23.4 Repository conformance

CI parses and typechecks every active shipped `.bv`, `.ebv`, `.abv`, `.sbv`, `.rbv`, `.dbv`, and `.dbvl` file under its declared target/profile.

Excluded legacy material belongs under `archive/`.

### 23.5 Documentation hierarchy

1. `spec/SPEC.md` is normative.
2. `docs/architecture/` explains implementation and rationale.
3. `learn-briev/` teaches normative syntax.
4. Timestamped plans are historical records and are not retroactively rewritten.

## 24. Standard-library boundary

The compiler knows bootstrap primitives, semantic operation identities,
hashwords, intrinsics, grammar, and the `coll` scaffolding surface (§8.10).
Everything else — regex, formatting, options/results, platform handles,
host-language types, and collection *policy* — belongs to stdlib, plugins, or
configuration.

The dividing line is **what the compiler matches**: the compiler never matches
on collection *type names*. `coll` is the one sanctioned compiler-owned
collection scaffold (keyword-based, §8.10): the compiler owns sequence
scaffolding — hidden length/capacity, `op Count`/`op At`, construction, and
the grow-on-full capacity strategy. Collection *policy* — hashing, load
factors, rehashing, occupancy — is type-specific and stays in stdlib; the
`coll` surface hands the author the `op Grow`/`op Shrink` hooks and the
capacity intrinsics, never a hash-aware default.

Examples:

- Regex is implemented through `regex!(#r"...")` and plugins/stdlib, not a `/.../` lexer literal.
- Associative literals are type-directed and do not imply `HashMap`.
- Stack/queue conversions are type-defined.
- DOM handles and host ABI categories are GLUE configuration, not Rust type-name matches.
- A HashMap's load factor, occupancy, and rehash policy are stdlib logic, not
  compiler behavior; the compiler provides the `op Grow`/`op Shrink` hooks and
  `Capacity#`/`Resize#`/`EnsureCap#`/`TrimCap#`, never a hash-aware default.

## 25. Implementation staging

This specification supersedes active contradictory syntax documentation. Implementation must proceed through explicit migration phases.

Until a normative feature is implemented, the compiler must:

- reject it with a precise staged-feature diagnostic; or
- continue accepting only already-conforming subsets.

It must not retain removed aliases in the normal parser merely for compatibility. Briev is pre-adoption; active repository source is rewritten directly to canonical syntax.

No compatibility parser or `briev migrate` tool is part of this migration. The canonical parser accepts only this specification.

The implementation plan following this specification defines parser, AST, analysis, interpreter, backend, stdlib, tooling, documentation, and verification order.
