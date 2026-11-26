# Research notes — Goose (aardappel/goose) and what Briev could borrow (2026-11-20)

Question: Goose is "apparently very optimized" — what is it, and which
compiler/language tricks could Briev borrow?

All Goose citations are from the `aardappel/goose` repository (master
branch, 3.7k+ stars), fetched 2026-11-20: README.md, docs/goose_spec.md
(v4 draft), docs/tutorial.md, docs/reference.md, bench/results.md, plus
the independent third-party validation repo `calpeyser/goose-evaluations`.

---

## 1. What Goose is

- Author: Wouter van Oortmerssen (GitHub `aardappel`, aka `wvo`; strlen.com
  — the E language / Amiga E lineage). One author, ~3.75k stars.
- Memory-safe systems language, **no heap allocator, no GC, no lifetime
  annotations, no `unsafe`**. Syntax familiar to C/Rust programmers.
- Compiles to **one C file** (runs anywhere clang/gcc runs); a bundled
  TinyCC backend compiles+runs in-process (JIT, no build step). Can call C
  via `extern fn`.
- Claimed results (bench/results.md, generated 2026-10-16, Ryzen
  9950X3D, whole-process wall clocks, checksum-verified outputs):
  geometric mean over 16 benchmarks — **3.51x vs idiomatic C++ (MSVC),
  3.54x vs idiomatic C++ (clang), 1.24x/1.26x vs expert hand-optimized
  C++, 1.13x/1.19x vs best safe Rust**, and less memory (1.95x vs
  idiomatic, 1.32x vs expert, 1.25x vs Rust).
- **Independently validated**: `calpeyser/goose-evaluations` re-ran four
  real-world workloads (Ninja build-graph load 139MB, JSON tree 300MB,
  YAML 180MB, jq core 315MB) on Graviton/gcc-11 -O2: Goose beats
  idiomatic in all four and the *expert* implementations in three of
  four (Ninja 1.26x vs n2-Rust, YAML 2.4x vs rapidyaml, jq 5.8x vs
  jaq-Rust; loses to yyjson on JSON by 1.2-1.3x — Goose's exact float
  printer is slow). The evaluation attributes the win to data layout:
  "one pool per structure, values only as large as they need to be,
  4-byte links, and nothing to free one piece at a time."

So the "very optimized" reputation is real AND third-party reproduced.

## 2. The compiler tricks (the actual machinery)

### 2.1 Data stacks + the stack invariant (spec §1.2-1.3)
- Memory = the native call stack + **N data stacks**, each a bump pointer
  over a big virtual reservation. Growth is a pointer bump with **no
  capacity checks emitted** — guard pages catch pathological overflow and
  abort safely. Freeing a nested structure = one store (rewind the bump).
- **The stack invariant** (proved statically): values pushed in lifetime
  order, popped en masse; lifetimes on one stack strictly nested; at most
  one *resizable* value live per stack and it is topmost while resizing;
  disjoint sequential lifetimes reuse stacks; fixed-size values live on
  the native stack/registers, resizable length headers live in the frame.

### 2.2 Destination-passing construction (spec §7.3)
- Every value is **constructed at its final destination through any depth
  of calls**: `items.push(parse(line))` writes the parsed record directly
  into the array; returning a growable array by value does not copy.
  `words.push(str("item", i))` writes the formatted element directly into
  the element slot. A named result is "a local that every return of the
  function gives".

### 2.3 Roots + outlives via monomorphization (spec §9)
- The ONLY borrow-checking rule: "a reference must not outlive the
  variable that owns its target". Every reference has static **roots**
  (locals/globals bounding its scope). Functions are compiled in
  call-graph order and **specialized per distinct root** — "the precision
  of Rust lifetimes through monomorphization, with no syntax". Aliasing
  is freely allowed (no shared-memory concurrency, no exclusivity).

### 2.4 Variable-sized enums (per-variant sizing)
- `Shape..` (variable mode) stores each value at **its own variant's
  size**, not the largest variant's. Claimed 4x less memory and 2x the
  speed of a Rust `enum` on the benchmark exercising it.

### 2.5 Relative references (narrow links)
- `T&<u8>/<u16>/<u32>/<u64>/<varint>` — typed, checked links stored as
  1-4 byte offsets; self-relative (position-independent → **saving a
  structure is one write, loading is one read + a verification pass**
  that rejects malformed/hostile data: `from_bytes` checks framing, tags,
  lengths, and that every link points at an element start) or
  pool-relative (`T&<u32 in pool>`). A BST in one grow-only array with
  4-byte children is the tutorial example.

### 2.6 Index types + checkfree (spec §15, §7.14, §10.5)
- `u32 in a` / `u32 upto a` — an index TYPE bound to a specific array `a`:
  static bounds by construction; arrays of index types
  `(u32 in a)[>..]`.
- **`checkfree fn`** — the compiler must PROVE no bounds check remains
  anywhere in the function's text (per specialization, per simd version,
  per inline copy) or reject the program. The BCE (§10.5) is part of the
  compilation model: sound-only elimination ("every check it removes is
  one that could not have aborted"), required only to be complete for
  `checkfree`.
- This is the answer to the `bundle_total rows[0]/rows[1]` class: the
  bounds fact is a TYPE, checked once at construction, then the loop body
  compiles check-free.

### 2.7 `return err from f` — multi-frame returns
- Checked statically in call-graph order (every call site of the returning
  function must lie within the dynamic extent of an `f` call); each frame
  between gains a hidden return discriminant; no unwinder, no Result type,
  no `?` per call. Lightweight opt-in "exceptions" for deep
  parser/loader errors.

### 2.8 No-shared-memory threads
- A worker compiles as a **separate program with its own memory**; flat
  values pass between threads through typed queues via `memcpy`. Data
  races, locks, atomics, memory orderings do not exist in the language.

### 2.9 resumability inference (§7.3)
- The checker infers per specialization whether a function resumes its
  caller (a `fn fail() { abort(...) }` callee allows the caller to omit
  its own result) — a control-flow fact, not a value type.

## 3. Where Goose and Briev ALREADY agree

- **Arena/region model**: Briev's hosted arena grows via brk/mmap,
  freestanding is fixed (SPEC:255-257); per-worker arenas
  (2026-11-16 plan) = Goose's per-thread data stacks. Briev's `borrow /
  consume / owned / borrowed<source>` ownership algebra (SPEC §14.1) is
  the same idea as Goose's roots/outlives, spelled as annotations.
- **Contract-first bounds**: Briev's contracts + `check` vs Goose's
  `checkfree` — inversions of the same idea (Briev: prove what must hold;
  Goose: prove checks are gone). Goose's BCE is "sound-only" — the same
  soundness-only contract Briev's verifier already works under.
- **Specialization**: Briev monomorphizes (syscall wrapper folds
  Count# per call site); Goose specializes per root/instantiation.
- **Compile-to-C is NOT portable to Briev** (Briev compiles to LLVM IR
  directly and needs contracts/proofs in the pipeline), but TinyCC-JIT is
  an interesting UX idea, orthogonal to correctness.

## 4. What Briev could actually borrow (ranked by fit)

| Trick | Fit | Where it lands in Briev |
|---|---|---|
| **Index types** (`u32 in a`) — bounds become part of the type; arrays of indices; checkfree-style proofs | **HIGH** — directly answers the `bundle_total rows[0]/rows[1]` OOB class AND the wave-5 two-layout trap blocker: an index type could carry WHICH layout's length to bound-check | typechecker (new index-type kind) + a `Count#`-derived constructor; SPEC §15-analog |
| **checkfree-style "no bounds checks left" proof obligation** | HIGH — turns the G2-style advisory pattern (prove-or-migrate) into a compile contract; fits the proof-engine pipeline | analysis pass after BCE; a `checkfree` modifier on defn/nodes |
| **Per-variant-sized enums** | MEDIUM-HIGH — Briev enums are fixed-max-variant today; the payload work (#138) touched exactly this area | enum layout in codegen |
| **`return err from f` multi-frame returns** | MEDIUM — cleaner than Briev's Result-via-match for deep loaders; static call-extent check fits the verifier | control-flow lowering + call-extent validation |
| **Destination-passing construction guarantee** (already partially true in Briev: `term Socket { fd }` constructs in place?) — worth VERIFYING in Briev's pipeline rather than assuming | MEDIUM — if Briev copies, a Guaranteed-Elision-style pass is a win | pipeline analysis |
| **Relative/narrow links** (`T&<u32>`, varint fields) | MEDIUM — Briev has no serialization story; position-independent structures + verify-on-load would slot into the ABI work | new storage kinds; ABI section |
| **TinyCC-style JIT** | LOW priority — UX, not correctness; Briev has an interpreter as the reference engine already | separate lane |
| **Guard-page growth with no capacity checks** | MEDIUM — Briev's arena is brk/mmap-backed; hosted-only guard-page reservation is plausible | allocator growth path |

## 5. Cautions (where borrowing would clash with Briev)

1. **Goose has no contracts** — its safety comes from the stack invariant
   + roots. Briev's contracts are the SUPERIOR mechanism for stating
   domain facts; adopting Goose's machinery must not displace them.
   The two compose: contracts state domains, Goose-style index types
   make bounds static.
2. **Goose forbids shared-memory concurrency**; Briev's reactor has
   `sync<group>`/`async` classification (SPEC §12). Borrowing Goose's
   worker-isolation wholesale would conflict with Briev's reactor
   convergence model. The typed-queue memcpy idea is compatible with
   Briev's ports.
3. **Compile-to-C** is a Goose-specific escape hatch; Briev's LLVM
   pipeline + proof machinery is the differentiator. TinyCC JIT is the
   only borrowable piece here.
4. **Guard-page no-check growth** trades bounded-memory guarantees for
   speed; Briev's freestanding targets have FIXED arenas — guard pages
   only fit the hosted lane.

## 6. Concrete near-term adoption sketch

1. **Index types pilot**: `u32 in xs` bound at construction from
   `xs.Count#()`; a subscript `xs[i]` with `i: u32 in xs` compiles
   WITHOUT a bounds check (the type IS the proof); the interpreter keeps
   its loud error (defense in depth); native keeps zero-cost. Pins the
   wave-5 §11.4 note with a positive mechanism instead of a warning.
2. **checkfree analog**: `defn f(...) checkfree` — verifier proves the
   body's subscripts are all index-typed or statically in-bounds; fails
   compilation otherwise. Directly reuses the existing proof engine.
3. **Per-variant enum sizing**: measure first (a fixture in the
   `bundle_total` style), adopt if the win reproduces on Briev's enums.

## Sources

- github.com/aardappel/goose (README, master branch):
  - README.md — claims list, no-heap summary, TinyCC, compile-to-C.
  - docs/goose_spec.md — §1.2-1.3 data stacks + stack invariant; §3.6-3.9
    (varint, strings, ADTs, relative refs); §7.3 (return placement +
    resumability); §7.14 checkfree; §9 (roots/outlives/monomorphized
    lifetimes); §10.5 (sound-only BCE); §11 (modules/threads).
  - docs/tutorial.md §15 (to_bytes/from_bytes verify-before-value).
  - docs/reference.md (type table: fixed/variable enums, index types,
    relative refs).
  - bench/results.md — environment, methodology (checksum-verified,
    whole-process, best-of-3), geometric means.
- github.com/calpeyser/goose-evaluations (README): the four independent
  workloads, wins/losses, the data-layout attribution, methodology.
- Briev in-tree: spec/SPEC.md:255-257 (arena growth), §14.1 (ownership
  algebra), §15-analog gap (no index types — Pattern grammar has no list
  pattern, briev-verify/src/ast/expr.rs:234-250; OOB native = inbounds
  UB per wave-5 W5-5 SPEC §11.4 note, landed 4b31bf328).

---

# PART 2 (2026-11-20, extended): full borrowables + intrinsics + memory-ergonomics audit

Owner asked for the complete picture: what to borrow, which intrinsics
would be nice, and how to make Briev's memory management more ergonomic.
Everything below is verified in-tree at `3fb10bc7a` (or cited from the
Goose repo).

## 7. Briev's current surface (the baseline to borrow INTO)

### 7.1 Intrinsic registry (src/intrinsic_signatures.rs:1141+, 71+ names)
Arithmetic/comparison/bit (Add#…Popcount#, LeadingZeros#, TrailingZeros#),
pointer family (Deref#, Ptr#-less Index#, PtrAdd#, PtrSub#, PtrDiff#,
PtrEq#, PtrLt#), memory (Malloc#, Alloc#, Free#, Load#, Store#,
VolatileLoad#, VolatileStore#, Copy#, Fill#), collection (Concat#,
CharCount#, Length#, Count#, At#, Slice#, Get#, Insert#, InsertAt#,
ExtractFrom#, CopyFrom#, Resize#, EnsureCap#, TrimCap#, Capacity#),
atomics (AtomicAddAt#/SubAt#/XchgAt#/CasAt#), math (Sqrt#…Fma#),
environment (Environ#, Argc#, Args#), text (ToInt#, ToFloat#, ToString#),
errors (Error#).

### 7.2 Memory ergonomics today
- `obj ByteBuf` (fixed, never moves — put_* precondition `len + n <= cap`)
  and `obj GrowBuf` (grow_to_fit reallocates; **ptr and cap both move on
  growth** — net/bytes.bv:151-190).
- `coll_arena.bv`: module-level bump arena (begin/reset; Alloc#-backed).
- SPEC §14.3 `free`/`keep` with an **auto-free scheduler that already
  exists** (defn_liveness.rs:625 "Scheduler auto-free lowers to rt_free";
  mod.rs:3662 warns when `keep` is redundant because the scheduler would
  auto-free). Goose's `defer` is the manual escape hatch for the residue;
  Briev's equivalent is the scheduler + explicit `free`.
- Caller-destination elision for constructor-shaped object returns already
  exists (object_repr.rs:22, 352-364: `consume(make(x))` constructs
  make's result at the caller slot) — **Goose's §7.3 destination-passing
  through calls is partially landed in Briev for obj returns.**
- Region-list vs stack-seq: two live List addressing schemes (the wave-5
  §7 two-layout blocker).

### 7.3 What Briev LACKS that Goose has
- No varint storage kind; no index types (`u32 in a`); no relative
  references (`T&<u32>`); no variable-sized enums (fixed-max-variant
  today); no checkfree-style proof obligation on bounds; no position-
  independent save/load (to_bytes/from_bytes verify-on-load); no
  multi-frame `return err from f`; no defer keyword for the unprovable
  residue; no guard-page no-check growth.

## 8. Borrowable-by-category (the complete ranked list)

### 8.1 INTRINSICS (nice-to-have additions, none duplicate the registry)
1. **`FromBytes#`/`ToBytes#` verify-on-load family** — Goose's
   from_bytes checks framing + tags + lengths + link targets before bytes
   become a value ("a corrupt or hostile file gives false and an empty
   array, never a wild reference"). For Briev: verify against the
   frozen String/Data ABI (SPEC §16.2) — length word sane, NUL present,
   region in-bounds. Pairs with the region-list layout question.
2. **`WithCap#`-style guard-page growth intrinsic** (hosted lane only) —
   Goose's stackalloc prototype: reserve large address ranges, commit on
   touch, guard gaps so runaway growth aborts cleanly (spec §10.4).
   Briev's Alloc#(Malloc) could gain a `Alloc#(bytes, Guarded)` kind on
   Linux (mmap PROT_NONE reservations). Freestanding excluded (fixed
   arenas).
3. **`VarIntEncode#`/`VarIntDecode#`** — ULEB128 (+zigzag for signed) as
   intrinsics IF varint storage is adopted (§8.3); as plain stdlib
   helpers otherwise (byte_at/Store# already suffice — no intrinsic
   strictly needed, Goose has them as a storage kind not intrinsics).
4. **`ClockMonotonicNs#`** — today `rt_now_ns` in cast_lanes goes through
   the raw syscall (with the documented aarch64=113 TODO). An intrinsic
   could (a) fix the arch table in one place, (b) later route to vDSO
   (§8.4) behind the same name — the W5-2 prerequisite made concrete.
5. **NOT worth intrinsics**: to_bytes/from_bytes as *intrinsics* (they
   are stdlib-level walks over a known ABI); varint encode/decode;
   string builders (ByteBuf/GrowBuf exist).

### 8.2 MEMORY-ERGONOMICS (the deeper wins)
1. **`defer` statement** (Goose reference: runs at block end however it
   ends — fallthrough, break, continue, return, multi-frame returns
   passing through; per-iteration in loops; latest-first). Briev has
   free/keep + the auto-free scheduler, but the scheduler covers only
   PROVEN last uses; the stdlib's ~30 manual `Free#` sites are the
   unprovable residue (kernel fds, opaque handles, scratch bufs). A
   `defer` statement is the honest spelling for that residue — Goose's
   per-iteration and latest-first semantics are the right SPEC shape.
   Lowest-risk highest-ergonomics item on this list.
2. **Index types** (`u32 in xs`) — Goose §15: bounds become part of the
   type; subscripts through index-typed values compile without checks;
   arrays of index types `(u32 in a)[>..]`. In Briev this directly fixes
   the `bundle_total rows[0]/rows[1]` OOB class AND composes with the
   existing proof engine (a `checkfree`-style proof obligation reuses
   it). Briev's contracts remain the domain-statement mechanism; index
   types make the bounds static. Requires: new type kind, Count#-derived
   construction, subscript rule, (optionally) the checkfree modifier.
3. **Variable-sized enums** — store each value at its variant's size.
   Briev enums are fixed-max-variant today. Goose claims 4x memory, 2x
   speed on the exercising benchmark. Needs layout planning (the enum
   lowerer) + a measure-first fixture.
4. **Relative references** (`T&<u32>`, pool-relative) — typed narrow
   links; position-independent structures → to_bytes/from_bytes becomes
   trivial and verify-on-load meaningful. Big surface (storage kinds,
   load/store re-encoding, null-in-0 optionals). Park behind varint.
5. **Region-list unification** (Briev-side prerequisite, not a borrow):
   the two List layouts block bounds traps AND complicate every new
   memory feature. Any Goose-style memory work presupposes this.

### 8.3 STORAGE-KIND candidates (bigger than intrinsics, smaller than
### a new backend)
1. **`varint`** (ULEB128 inline storage; reads decode to i64; writes
   only at construction; refs read-only) — Goose §3.6. Fits Briev's
   Data/Bytes storage lane; useful for the net paths (varint-length
   prefixed frames are ubiquitous in wire formats).
2. **Guarded allocations** (§8.1.2 above).
3. **Root structs** (Goose §C.2 "root struct" = independent field storage
   + reference descriptor) — the honest fix for the two-list-layout
   blocker if Briev ever unifies region lists: the descriptor carries
   which layout, so bounds checks read the right header.

### 8.4 vDSO + clock (the earlier discussion, concrete)
`ClockMonotonicNs#` intrinsic (§8.1.4) is the RIGHT naming layer: the
intrinsic can route per-triple (x86_64 syscall 228 / aarch64 113 — the
existing table infra from W5-1) today, and gain a vDSO path later
without touching callers. vDSO itself: parse AT_SYSINFO_EHDR (auxv.bv
extension), locate the symbol, indirect call — real work, only worth it
when a hot-path consumer exists (kiosk test files use rt_now_ns; the
serving path does not read clocks).

### 8.5 What NOT to borrow
- Goose's "no aliasing rules because no shared memory" — Briev's reactor
  has sync<group>/async classification (SPEC §12); wholesale worker
  isolation would clash with the convergence model. Typed-queue memcpy
  ideas are compatible with ports.
- Compile-to-C + TinyCC: Briev's LLVM+proof pipeline is the
  differentiator; a JIT lane is UX polish, not correctness.
- "No contracts" — Goose's safety is the stack invariant + roots;
  Briev's contracts state domain facts Goose cannot. Compose, don't
  replace: index types make bounds static, contracts keep domains.

## 9. Ranked adoption roadmap (what this research proposes)

| # | Item | Type | Effort | Payoff |
|---|---|---|---|---|
| 1 | `defer` statement | language stmt | M | kills the ~30 manual Free# leak-on-early-exit class; standard escape hatch |
| 2 | Index types (`u32 in xs`) + checkfree modifier | type system + analysis | L | bounds proofs static; solves bundle_total; composes with contracts |
| 3 | `ClockMonotonicNs#` intrinsic (arch-correct, vDSO-ready name) | intrinsic | S | fixes aarch64=113 TODO in one place; vDSO later behind same name |
| 4 | FromBytes#/ToBytes# stdlib family (over the frozen §16.2 ABI) | stdlib | M | verify-on-load for Data regions; hostile-input safety story |
| 5 | Per-variant enum sizing | enum layout | M-L | memory+speed; measure-first fixture required |
| 6 | varint storage kind | storage kind | L | wire-format ergonomics; net paths |
| 7 | Relative refs + to_bytes/from_bytes | storage kinds + stdlib | XL | position-independent save/load; biggest new surface |
| 8 | Guarded allocations (hosted lane) | allocator | M | runaway-growth safety; hosted-only |
| 9 | Region-list layout unification | Briev-side prerequisite | L | unblocks bounds traps; simplifies all list memory work |

## 10. Sources (local + web, this part)

- src/intrinsic_signatures.rs:1141-1230 (registry, 71+ names)
- src/analysis/defn_liveness.rs:625, 1004 (auto-free scheduler exists)
- src/backend/llvm/mod.rs:3662-3666 (redundant-keep warning proves the
  scheduler), :3912-3922 (sret/hidden-result machinery),
  src/backend/llvm/mod.rs:3909 + src/analysis/object_repr.rs:22,46,
  352-364 (caller-destination elision tests — dest-passing partially
  landed for obj returns)
- lib/std/net/bytes.bv:100-190 (ByteBuf fixed vs GrowBuf moving growth)
- lib/std/coll_arena.bv (module-level bump arena)
- lib/std/net/socket.bv:407-412 (imports-vs-literals), os/fs.bv:183,
  string_c.bv:27, string.bv:411/423, cast_lanes.bv:460/462 (+8 ABI sites)
- spec/SPEC.md §14.3 (free/keep + auto-scheduling), §16.2 (the frozen
  String/Data ABI, landed c71c3073f), :255-257 (arena growth model),
  §12 (concurrency classification)
- Goose: docs/goose_spec.md §C.1-C.2 (control block, layouts, varint
  ULEB128+zigzag, root structs), §10.4 (startup reservations, guard
  gaps, 32TB budget, wasm fallback), §7.14 (checkfree), §9.2 (roots),
  §3.9 (relative refs), docs/reference.md `defer` (per-iteration,
  latest-first), docs/tutorial.md §15 (to_bytes/from_bytes),
  github.com/aardappel/stackalloc (the reservation prototype)
- calpeyser/goose-evaluations README (independent validation: 3/4 beat
  expert, JSON loss = exact float printer cost)
