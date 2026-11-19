# Data Types: Collections & Structures

> **Runnable companion:** `learn-briev/examples/05-data-types.bv` compiles, runs, and its output is pinned in CI (`tests/learn_briev_examples.rs`) — the machine-checked face of this chapter.


Briev provides powerful built-in data types with O(1) operations.

## 1. HashMap<K, V>

Hash-based key-value storage with O(1) lookup. The current `std/collections.bv`
HashMap is a **flat open-addressing** obj (keys/vals Ptr arrays) constructed via
`op Init` and mutated through method calls:

```briev
import { HashMap } from "std/collections.bv";

// Construction via op Init
let map: HashMap<Int, Int> = 0;

txn fill [true][map.count > 0] {
    map.insert(1, 42);
    map.insert(2, 100);
    term;
};

// Lookup — the member returns the value for the key's hash slot
defn get_age(m: HashMap<Int, Int>) -> Int {
    term m.get(1);
};
```

### Example: Word Counter

```briev
defn count_words(text: String) -> HashMap<String, Int> {
    let counts = new_map<String, Int>();
    let words = text.split(" ");
    
    let i: Int = 0;
    [i < words .^Len] {
        let word = words[i];
        let current = counts.get(word);
        [current.is_some()] {
            counts = counts.insert(word, current.unwrap() + 1);
        };
        [current.is_none()] {
            counts = counts.insert(word, 1);
        };
        i = i + 1;
    };
    
    term counts;
};
```

## 2. HashSet<T>

Hash-based set with O(1) membership testing.

```briev
// Construction
let set: HashSet<String> = new_set();

// Insert
set = set.insert("apple");
set = set.insert("banana");
set = set.insert("cherry");

// Check membership
[set.contains("apple")] {
    println("Has apple");
};

// Remove
set = set.remove("banana");

// Metadata
let len = set .^Length;
[set.is_empty()] {
    println("Set is empty");
};

// Iteration
let items = set.iter();
```

### Example: Unique Items

```briev
defn unique_items(list: List<String>) -> List<String> {
    let seen = new_set<String>();
    let result: List<String> = [];
    
    let i: Int = 0;
    [i < list .^Len] {
        let item = list[i];
        [!seen.contains(item)] {
            seen = seen.insert(item);
            result = result.append(item);
        };
        i = i + 1;
    };
    
    term result;
};
```

## 3. Stack<T, N>

LIFO (Last-In-First-Out) structure. The current collections (`std/collections.bv`)
are **obj instances** driven by the `<-` operators: `op InsertAt` (push),
`op ExtractFrom` (pop), `op Init` (`let s: Stack<T, N> = 0` constructs).

```briev
import { Stack } from "std/collections.bv";

// Construction via op Init — allocates the instance + its data buffer
let stack: Stack<Int, 64> = 0;

// Push — `<-` dispatches op InsertAt → the self-bound push member
stack <- 1;
stack <- 2;
stack <- 3;

// Pop (discard the value) — `<-` dispatches op ExtractFrom → pop
<- stack;

// Destructive discard — pop and destroy the source's backing
~<- stack;

// Destructive extract into a binding (2026-08-01, Phase 3)
let v2: Int = 0;
v2 ~<- stack;

// Pop into a binding reads the member's returned value
let v: Int = stack.pop();
```

`obj Stack<T, N> { data: T[N]; len: Int; ... }` — a fixed-size, array-backed
stack (no heap in the hot path). `List<T>` is the heap-backed growable form
(`inner: ListBuffer<T>` with a `Malloc#`-allocated buffer); `RingBuffer<T>`
is the fixed-size circular buffer.

### Example: Expression Evaluator

```briev
defn evaluate_rpn(expr: List<String>) -> Int {
    let stack: Stack<Int> = new_stack();
    
    let i: Int = 0;
    [i < expr .^Len] {
        let token = expr[i];
        [token == "+"] {
            let (b, s) = stack.pop().unwrap();
            let (a, s2) = s.pop().unwrap();
            stack = s2.push(a + b);
        };
        [token == "*"] {
            let (b, s) = stack.pop().unwrap();
            let (a, s2) = s.pop().unwrap();
            stack = s2.push(a * b);
        };
        [!is_operator(token)] {
            let num = String(token).to_int();
            stack = stack.push(num);
        };
        i = i + 1;
    };
    
    term stack.pop().unwrap().0;
};
```

## 4. Queue<T>

FIFO (First-In-First-Out) structure.

```briev
// Construction
let queue: Queue<String> = new_queue();

// Enqueue
queue = queue.enqueue("first");
queue = queue.enqueue("second");
queue = queue.enqueue("third");

// Dequeue (returns Option<(T, Queue<T>)>)
let result = queue.dequeue();
[result.is_some()] {
    let (value, new_queue) = result.unwrap();
    // value = "first", new_queue has ["second", "third"]
};

// Front (returns Option<T>)
let front = queue.front();

// Metadata
let len = queue .^Length;
[queue.is_empty()] {
    println("Queue is empty");
};

// Clear
queue = queue.clear();
```

### Example: BFS Traversal

```briev
defn bfs(start: Node) -> List<Node> {
    let visited = new_set<Node>();
    let queue: Queue<Node> = new_queue();
    let result: List<Node> = [];
    
    queue = queue.enqueue(start);
    visited = visited.insert(start);
    
    [!queue.is_empty()] {
        let (node, q) = queue.dequeue().unwrap();
        queue = q;
        result = result.append(node);
        
        let neighbors = node.get_neighbors();
        let i: Int = 0;
        [i < neighbors .^Len] {
            let neighbor = neighbors[i];
            [!visited.contains(neighbor)] {
                visited = visited.insert(neighbor);
                queue = queue.enqueue(neighbor);
            };
            i = i + 1;
        };
    };
    
    term result;
};
```

## 5. StringBuilder

Efficient string concatenation (O(1) amortized append).

```briev
// Construction
let sb = new_builder();

// Append
sb = sb.append_char('H');
sb = sb.append_char('e');
sb = sb.append_str("llo");
sb = sb.append_int(42);
sb = sb.append_bool(true);
sb = sb.append_float(3.14);

// Convert to String
let result = sb.to_string();  // "Hello42true3.14"

// Metadata
let len = sb .^Length;
[sb.is_empty()] {
    println("Builder is empty");
};

// Clear
sb = sb.clear();
```

### Example: CSV Builder

```briev
defn build_csv(rows: List<List<String>>) -> String {
    let sb = new_builder();
    
    let i: Int = 0;
    [i < rows .^Len] {
        let row = rows[i];
        let j: Int = 0;
        [j < row .^Len] {
            [j > 0] {
                sb = sb.append_char(',');
            };
            sb = sb.append_str(row[j]);
            j = j + 1;
        };
        sb = sb.append_char('\n');
        i = i + 1;
    };
    
    term sb.to_string();
};
```

## 6. Enums: Closed Sums

`enum` declares a closed nominal sum — a value that is exactly one of a
fixed set of variants, each optionally carrying a payload:

```briev
enum Result<T, E> {
    Ok(T),
    Err(E),
};

enum Http {
    Ok(Int),
    Fail,
};
```

Construct a variant by calling it as a function; payloads are positional:

```briev
let ok: Result<Int, String> = Ok(42);
let bad: Result<Int, String> = Err("nope");
```

Read a sum back with an exhaustive `match` — every variant needs an arm,
and payload bindings name the carried values:

```briev
defn show(r: Result<Int, String>) -> Int {
  term match r {
    Ok(v) => v,
    Err(msg) => 0 - 1,
  };
}
```

Zero-payload variants (declared without parens) construct with zero
arguments: `Http::Fail`.

**Qualified paths.** When two enums declare the same variant name, bare
construction becomes ambiguous — declaring both `Http::Ok(Int)` and
`Db::Ok(Int)` is legal, but a bare `Ok(5)` fails compilation naming the
declaring enums. Qualify to disambiguate; qualified construction and
patterns always resolve:

```briev
Print#(pick(Http::Ok(5)));
term match state {
    Http::Ok(v) => v,
    Http::Fail => 0 - 1,
};
```

A user-defined `defn` sharing a variant's name shadows it — prefer
qualified paths near same-named functions.

## 7. Structs: Pure Data Containers

`struct` declares pure data with fixed layout, C-compatible, no methods or contracts:

```briev
struct Point {
    x: Int;
    y: Int;
};

struct Contact {
    name: String;
    phone: String;
    email: String;
};

let p = Point { x: 10, y: 20 };
let name = p.name;  // field access
```

### Physical Layout Modifiers (2026-08-13, Deferred Layout)

The story term is **Boxed Cat Typing** — a Schrödinger's cat pun: a type's
representation is indeterminate ("in the box") until code observes it or a
modifier pins it. Not literal boxing of values.

A plain struct is layout-adaptive. When the layout must be pinned, it is
**declared** — the type never assumes a representation. `spec` spells the five
physical keys (`Bits`, `MaxBits`, `Bytes`, `Alignment`, `Endian`), and three
modifiers shape struct declarations:

```briev
// Bit-contiguous: fields pack with zero padding in declaration order.
pack struct EthHeader {
    spec Endian: Big;             // MSB-first within byte + BE multi-byte
    dst: Bit<48>;
    src: Bit<48>;
    etype: Bit<16>;
};

// Untagged overlay: all fields share storage at offset 0; size is the
// largest aligned field. Sub-byte Bit<N> fields are rejected (deferred).
union Word {
    u: Bit<64>;
    bytes: Bit<32>;
    lo: Bit<16>;
};

// Per-field concurrency: reads/writes go atomic; `c.count = c.count + 1`
// lowers to an atomic read-modify-write.
struct Counter {
    atomic count: Int;
    other: Int;
};
```

`Bit<N>` is exactly N bits everywhere — a `Bit<48>` field reads 48 bits, not
a rounded word. `x as Bit<N>` truncates to N bits (a `Bit<4>` never holds
16). `Bit` bare is flexible width (resolved later); `Bit<N>` is exact.
There is no separate `Bits` type — multiple bits is just `Bit<N>`. The
reference interpreter models structs as layout-free values; the
byte-level packing, overlay, and atomicity are what the target materializes.

### Fixed-Size Arrays: `Type[N]` and Slice Views

Arrays of compile-time-known size are declared inline. Works for any type:

```briev
struct VMStack {
    data: Int[1024];    // [1024 x i64] in LLVM IR
    len: Int;
};

struct Cache {
    cells: Float[9];    // fixed-size, auto-vectorized
};
```

`Int[1024]` → `[1024 x i64]`, `Frame[256]` → `[256 x %Frame]`.
Bounds proven by contract: `[i >= 0 && i < stack .^Len]`.

#### Hardware state arrays: `mem let` / `reg let`

On hardware targets (`.cbv`), a top-level fixed array is state. The compiler
picks the lowering automatically — registers for small arrays, a memory
macro past the size crossover — and **tells you**: one note per build lists
every array it lowered by default, with the reason. Pin a choice with a
prefix keyword when you want to own it:

```briev
let  small: Int[8]   = [0,0,0,0,0,0,0,0];  // registers; appears in the note
mem  let big: Int[64] = [0, ...];          // memory macro, pinned (silent)
reg  let tab: Int[64] = [0, ...];          // registers, pinned (silent)
```

`mem` accepts port limits and forbids element postconditions; `reg` keeps
combinational access and full obligations. Neither makes code faster —
the default already is.

#### Array Slices: `arr[start:end:stride]`

A slice is a **zero-copy view** into an existing array:

```briev
arr[:]         // Full view
arr[4:]        // From index 4 to end
arr[:8]        // From start to index 8
arr[2:8]       // Range [2, 8), stride 1
arr[2:8:2]     // Every other element
arr[i:j:k]     // Dynamic bounds
```

All components are optional: start defaults to 0, end to array length, stride to 1.

#### SIMD Operations

Element-wise arithmetic on array and slice types:

```briev
let a: Int[4] = ...;
let b: Int[4] = ...;
let sum = a + b;       // <4 x i64> vector add
let doubled = a * 2;   // Scalar broadcast
```

#### View Casts

The `as` operator produces zero-copy views between compatible array types:

```briev
let raw: Int[1024];
let bytes = raw as Byte[8192];  // type-punned: same bytes, different type
let evens = raw[0:1024:2] as Int[512];  // strided: slice recast as sized array
```

**Type-punned view:** `N * sizeof(T) == M * sizeof(U)` enforced at compile time.
Emits LLVM `bitcast`. **Strided view:** slice bounds compute element count,
validated against target size. Both are zero-copy.

#### Stdlib, Not Magic

`map`, `filter`, `fold` are regular txn functions in `lib/std/array.bv`:

```briev
txn array_map<T, U>(arr: Vector<T, N>, f: T -> U, i: Int)
    -> Vector<U, N>
    [i < N][i == N]
{
    result[i] = f(arr[i]);
    i = i + 1;
    term result;
};
```

The LLVM auto-vectorizer recognizes the `[i < N]` convergence contract and
vectorizes the load-apply-store loop without compiler magic.

### `type` vs `struct` vs `obj`

| Keyword | Purpose | Example |
|---------|---------|---------|
| `type` | Protocols, operator bindings, type system extensibility | `type MyInt: Int { spec Bits: 32; };` |
| `struct` | Pure data, fixed layout, C-compatible, no methods | `struct Point { x: Int; y: Int; };` |
| `obj` | Full-featured types with methods, contracts, generics | `obj Channel<T> { ... };` |

`type { field: T }` patterns are being migrated to `struct { field: T }`.

### The Fundamentals (2026-08-15)

The fundamental types are compiler-native — they need no `type` declaration
and are not overloadable (`op` is for user types). The hierarchy:

- **`Data`** — the universal reflective floor. Every value can be observed
  and reflected as its raw storage (the treat-as-bits view); it is not a
  supertype — no universal inheritance edge. Use it as a generic bound for
  "any value": `<T: Data>`.
- **`Bit<N>`** — the bit type at any width. Touch individual bits / exact
  widths. `Bit` bare = flexible (resolved later); `Bit<N>` = exact N.
  There is no separate `Bits` type.
- **`Blob`** — the `[len][bytes]` byte buffer. Hold raw bytes, interpret
  later. Safe, never null (it always carries its length). Cast to `String`
  (lens) or `Bit<N>` (bit view); scalars convert via explicit stdlib fns
  (`blob_to_int`, `int_to_blob`, …).
- **`Int` / `UInt` / `Float` / `Bool` / `Char` / `String` / `Ptr` /
  `Void`** — the numeric/scalar fundamentals. `Double` is `type Double:
  Float` (just Float with more bits).
- **`struct`** — passive fixed record, C-compatible, no behavior.
- **`coll obj` / `coll struct`** — the native strategy keyword for
  collections: declare the one sequence member (the storage shape) and the
  compiler owns the rest — hidden `cap`/`len`, scaffolded `op Count`/`op At`/
  construction/iteration, `.^Length`, `Count#`, and default `op Grow`/
  `op Shrink`. **The compiler picks the most effective storage** (heap block
  for growable `Ptr<T>`, inline array for fixed `T[N]`, pooled columns for a
  fixed named instance). **`seq coll`** forces the elements into one
  contiguous memory block — for a `Ptr<T>` coll the data buffer already IS
  one block; for a fixed `T[N]` coll it forbids the columnar layout.
- **`obj`** — state + behavior + lifecycle.

| You want to… | Use |
|---|---|
| touch individual bits / exact width | `Bit<N>` |
| hold raw bytes, interpret later | `Blob` |
| accept *any* value | `<T: Data>` (the root) |
| passive fixed record | `struct` |
| state + behavior + lifecycle | `obj` |

The overlaps are the design: `Blob` and `Bit<N>` both are bytes, differ in
intent (buffer vs pattern); `struct` and `obj` both carry fields, differ in
behavior (passive vs active); `Data` underlies all four; the casting graph
moves between them with zero ceremony. Absence is `Option::None` — Blob is
never null.

### Collections with `coll` (a worked example)

`coll` is the native strategy keyword for declaring collections. You write
the one sequence member (the storage shape); the compiler owns the length,
capacity, and the op surface.

```briev
// MyQueue — a growable queue backed by a heap buffer.
// `data: Ptr<Int>` is the one sequence member; `cap` and `len` are
// compiler-owned hidden slots (the compiler appends them).
coll obj MyQueue {
    data: Ptr<Int>;
};

// `seq coll` — the same shape, but the compiler must keep the elements in
// one contiguous memory block (for a Ptr<T> coll the data buffer already
// is one block, so this is a hard guarantee of what the shape gives).
seq coll obj RingQueue {
    data: Ptr<Int>;
};

// A fixed-size collection: `coll struct` is `T[N]` only this slice —
// length == capacity == N, no hidden slots, C ABI preserved.
coll struct Fixed {
    data: Int[4];
};

let done: Int = 0;
node go [done == 0][done == 1] {
    let q: MyQueue = [5, 6, 7];   // literal → op Init + op InsertAt
    q <- 8;                        // push → op InsertAt (default cap 16)
    let n: Int = q.Count#();       // 4 — element count
    let len: Int = q.^Length;      // 4 — stored length (the hidden len slot)
    let cap: Int = Capacity#(q);   // 16 — the hidden capacity
    let first: Int = q[0];         // 5 — op At
    let sum: Int = 0;
    foreach x in q { sum = sum + x; }  // 5+6+7+8 = 26
    println!(sum);
    done = 1;
    term;
};
```

**Custom growth with `op Grow`/`op Shrink`.** The compiler's default growth
policy doubles capacity when full. A type may override it with a strategy
binding that takes the collection handle only (`#Lh`):

```briev
coll obj GeometricQueue {
    data: Ptr<Int>;
    op Grow: geometric_grow(#Lh);   // override the default doubling
    op Shrink: geometric_shrink(#Lh);
};

// A custom grow can read/write capacity through the capacity intrinsics
// without touching a property — "set capacity without setting a property".
defn geometric_grow(q: GeometricQueue) {
    let cur: Int = Capacity#(q);
    Resize#(q, cur * 3);            // triple, not double
};
```

Grow-on-full is the default: when an insertion would exceed capacity
(`len == cap`), the compiler doubles the capacity before the store — an
insert past capacity is never an out-of-bounds write. A type overrides the
policy with its own `op Grow` binding; `Resize#`/`EnsureCap#` remain the
explicit-control intrinsics.

**Capacity control.** The four capacity intrinsics are the interface to the
compiler-owned capacity: `Capacity#(h)` reads it, `Resize#(h, cap)` sets it,
`EnsureCap#(h, n)` grows to at least `n`, `TrimCap#(h)` shrinks to `len`.
They are intrinsics (operations), never reflection — there is no
`.^Capacity`.

## 8. Complete Example: Contact Manager

```briev
// contacts.bv

struct Contact {
    name: String,
    phone: String,
    email: String
};

let contacts: HashMap<String, Contact> = new_map();

txn add_contact(name: String, phone: String, email: String)
    [!contacts.contains_key(name)]
    [contacts.contains_key(name)]
{
    let contact = Contact {
        name: name,
        phone: phone,
        email: email
    };
    contacts = contacts.insert(name, contact);
    term;
};

txn remove_contact(name: String)
    [contacts.contains_key(name)]
    [!contacts.contains_key(name)]
{
    contacts = contacts.remove(name);
    term;
};

txn lookup(name: String) [name != ""][contacts == contacts] {
    let contact = contacts.get(name);
    when contact.is_some() {
        let c = contact.unwrap();
        println("Name: " + c.name);
        println("Phone: " + c.phone);
        println("Email: " + c.email);
    };
    when contact.is_none() {
        println("Contact not found");
    };
    term;
};

txn list_all() [true][contacts == contacts] {
    let names = contacts.keys();
    let i: Int = 0;
    when i < names .^Len {
        println(names[i]);
        i = i + 1;
    };
    term;
};
```

## Exercises

1. Implement a cache using HashMap with LRU eviction
2. Create a palindrome checker using Stack
3. Build a task scheduler using Queue with priorities
4. Implement a simple template engine using StringBuilder

---

## 9. Vectors: Multidimensional Arrays

Vectors are fixed-size, contiguous memory arrays optimized for hardware and SIMD operations.

### Declaration

```briev
// 1D vector
let vec: Vector<Int, 100>;

// 2D matrix
let mat: Vector<Int, 10, 20>;

// 3D tensor
let tensor: Vector<Float, 3, 32, 32>;

// Named dimensions (for readability)
let persons: Vector<Person, width:50, height:50, depth:40, time:10>;
```

### Syntax

- `Vector<T, dim1, dim2, ...>` - First argument is the element type, rest are dimensions
- Dimensions can be **anonymous** (just numbers) or **named** (`name:size`)
- Names are purely syntactic - they don't affect memory layout
- Total elements = product of all dimensions

### Accessing Elements

```briev
let mat: Vector<Int, 10, 20>;

// 2D access
let x = mat[5, 10];

// Slicing - get a row
let row: Vector<Int, 20> = mat[5, :];

// Range slicing
let sub: Vector<Int, 5, 5> = mat[0..5, 0..5];

// Named dimension slicing
let persons: Vector<Person, width:50, height:50, time:10>;
let frame: Vector<Person, 50, 50> = persons[time:5];
let slice: Vector<Person, 50> = persons[time:5, width:10];
```

### Slicing Syntax

Briev supports powerful slicing with commas for multiple dimensions:

```briev
let mat: Vector<Int, 10, 20>;

// Single index per dimension
mat[5, 10]           // Returns element at [5][10]

// Range slicing
mat[0..5, 0..10]     // Returns Vector<Int, 5, 10>
mat[5.., ..10]       // From 5 to end, from 0 to 10

// Striding
mat[::2, ::4]        // Every 2nd row, every 4th column
mat[0..10:2, ::3]    // Range with stride

// Named dimensions
persons[time:5, width:0..10]
persons[time::2, width:5]  // Every 2nd time step, at width=5
```

### Slicing Syntax

Briev supports powerful slicing with commas for multiple dimensions:

```briev
let mat: Vector<Int, 10, 20>;

// Single index per dimension
mat[5, 10]           // Returns element at [5][10]

// Range slicing
mat[0..5, 0..10]     // Returns Vector<Int, 5, 10>
mat[5.., ..10]       // From 5 to end, from 0 to 10

// Striding
mat[::2, ::4]        // Every 2nd row, every 4th column
mat[0..10:2, ::3]    // Range with stride

// Named dimensions
persons[time:5, width:0..10]
persons[time::2, width:5]  // Every 2nd time step, at width=5
```

### Filtering with Semicolon

The semicolon separates coordinates from filter conditions:

```briev
let persons: Vector<Person, 50, 50>;

// Filter: all persons where age > 18
persons[: age > 18].adult = true;

// Slice + filter: at row 5, where age > 18
persons[5, ; age > 18].processed = true;

// Range + stride + filter
persons[0..50:2; city == "NYC"].region = "East";
```

### Why Vectors Instead of Lists

| Feature | List<T> | Vector<T, dims> |
|---------|---------|--------------|
| Size | Dynamic | Fixed at compile time |
| Memory | Heap-allocated | Contiguous buffer |
| Access | O(n) scan | O(1) direct |
| Hardware | Limited | Full SIMD support |
| Use case | Runtime data | Buffers, tensors |

### Example: Image Processing

```briev
struct Pixel {
    r: UInt,
    g: UInt,
    b: UInt
}

let frame: Vector<Pixel, width:1920, height:1080>;

// Brighten all red pixels > 200
frame[: r > 200].r = 255;

// Process every 4th pixel
frame[width::4, height::4].r = frame[width::4, height::4].r / 2;
```

---

## 10. Pointer Types (`Ptr<T>`)

`Ptr<T>` is a verified pointer whose safety is proven at compile time.
Creation is via the **address-of** operator `&` — the compiler tracks
provenance, so there is no way to forge a `Ptr<T>` without the compiler
knowing its origin.

### Creating Pointers

```briev
// Verified pointer (compiler tracks bounds, guarantees non-null)
let p: Ptr<Int> = &x;

// From a collection's first element
let list_ptr: Ptr<Int> = &my_list[0];

// Reflection form (same meaning as &x)
let p2: Ptr<Int> = x.^Ptr;
```

### Dereferencing

Use bracket indexing — `ptr[i]` — or the `*` dereference operator:

```briev
let val: Int = p[0];          // Read element 0 — bounds-checked at compile time
p[0] = 42;                    // Write element 0 — bounds-checked
let first: Int = *p;          // Dereference — first element
```

The compiler emits the same raw `load`/`store` instructions as C, but only
after proving the access is within bounds.

### Safety Guarantees

| Property | Guaranteed by |
|----------|---------------|
| Bounds | `i * sizeof(T) < ptr.^^Bytes` is proven by the SMT solver |
| Non-null | `Ptr<T>` from `&x` or `&list[0]` is always valid |
| Alignment | Address is always aligned to `T.^^Alignment` |
| No use-after-free | Briev has no `free` — global state lives forever |

### Standard Library

`std/ptr.bv` provides convenient wrappers with explicit contracts:

```briev
import { read_i64, write_i64, copy, address } from "std/ptr.bv";

// Safe read — precondition: i >= 0 && (i+1)*8 <= p .^^Bytes
let v = read_i64(p, 0);

// Safe write — same precondition
write_i64(p, 0, 99);

// Block copy — precondition: non-overlapping ranges → llvm.memcpy
copy(dest, src, count);

// Get raw address
let addr = address(p);
```

Every function has a contract that the `PointerVerifier` pass checks at
compile time. If the caller cannot prove the precondition, compilation fails
with a `ProofError`.

---

## 11. Type/Metadata Checks: `is`, `from`, `like`

Briev provides three infix operators for inspecting types and structure at runtime.

### `is` — Type or Variant Check

```briev
let x: Int = 42;
let is_int = x is Int;    // → true

let y: Option[Int] = some(42);
let is_some = y is some;  // → true
let is_none = y is none;  // → false
```

The RHS of `is` can be a type name (`Int`, `String`) or a variant keyword (`some`, `none`, `ok`, `err`).

### `from` — Derivation Check

```briev
struct Foo { x: Int; }
struct Bar : Foo { y: Int; }

let obj = Bar { x: 1, y: 2 };
let is_from_foo = obj from Foo;   // → true
```

Checks whether the value's type is or derives from the target type.

### `like` — Structural Equality

```briev
42 like 42             // → true
[1, 2] like [1, 2]     // → true (recursive comparison)
"hi" like "hi"         // → true
42 like 1              // → false
```

Compares structural layout, not nominal type. Two structs with different names
but identical fields can be `like` each other.

### Precedence

```briev
!x is Some      → !(x is Some)
x is Some == true → (x is Some) == true
```

Binds tighter than `==`/`!=` but looser than unary `!`.

---

*Next: [06-string.md](06-string.md) - String manipulation and operations*
