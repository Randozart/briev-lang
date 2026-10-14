# Briev Standard Library

**Version:** 0.11.0  
**Status:** Actively developed  
**Tier 1 Status:** ✅ COMPLETE

---

## Core Types (Tier 1 - Complete)

### Primitive Types
- **Char** - Unicode codepoint (`'a'`, `'\n'`, `'\u{1F600}'`)
- **Int**, **UInt**, **Float**, **Bool**, **String**, **Data**, **Void**

### Collection Types
- **HashMap<K, V>** (`hashmap.bv`) - O(1) key-value lookup
  - `new_map()`, `insert()`, `get()`, `contains_key()`, `remove()`
  - `len()`, `is_empty()`, `keys()`, `values()`, `iter()`
  
- **HashSet<T>** (`hashset.bv`) - O(1) membership testing
  - `new_set()`, `insert()`, `contains()`, `remove()`
  - `len()`, `is_empty()`

- **Stack<T>** (`stack.bv`) - LIFO structure
  - `new_stack()`, `push()`, `pop()`, `peek()`
  - `len()`, `is_empty()`, `clear()`

- **Queue<T>** (`queue.bv`) - FIFO structure
  - `new_queue()`, `enqueue()`, `dequeue()`, `front()`
  - `len()`, `is_empty()`, `clear()`

### String Types
- **StringBuilder** (`string_builder.bv`) - Efficient string building
  - `new_builder()`, `append_char()`, `append_str()`
  - `append_int()`, `append_bool()`, `append_float()`
  - `to_string()`, `clear()`, `len()`, `is_empty()`

### Error Handling
- **Option<T>** (`option.bv`) - Nullable types
  - `Some(T)`, `None`
  - `is_some()`, `is_none()`, `unwrap()`, `unwrap_or()`
  - `option_map()`, `option_and_then()`, `option_or_else()`
  - `option_filter()`, `option_or()`, `option_xor()`

- **Result<T, E>** (`result.bv`) - Error handling
  - `Ok(T)`, `Err(E)`
  - `is_ok()`, `is_err()`, `unwrap()`, `unwrap_err()`
  - `result_map()`, `result_map_err()`, `result_and_then()`
  - `result_or_else()`, `result_or()`, `result_filter()`

---

## Standard Modules

### Math (`math.bv`)
**Status:** ✅ 100% native

```briev
// Basic
defn abs(n: Int) -> Int
defn min(a: Int, b: Int) -> Int
defn max(a: Int, b: Int) -> Int
defn square(n: Int) -> Int
defn cube(n: Int) -> Int

// Division
defn div(a: Int, b: Int) -> Int
defn mod(a: Int, b: Int) -> Int
defn gcd(a: Int, b: Int) -> Int
defn lcm(a: Int, b: Int) -> Int

// Advanced
defn factorial(n: Int) -> Int
defn fibonacci(n: Int) -> Int
defn powi_int(base: Int, exp: Int) -> Int
defn sum_range(start: Int, end: Int) -> Int

// Predicates
defn is_even(n: Int) -> Bool
defn is_odd(n: Int) -> Bool
defn is_positive(n: Int) -> Bool
defn is_negative(n: Int) -> Bool
defn is_zero(n: Int) -> Bool
```

### String (`string.bv`)
**Status:** ⚠️ 65% native, 35% FFI

```briev
// Native
defn len(s: String) -> Int
defn concat(a: String, b: String) -> String
defn contains(haystack: String, needle: String) -> Bool
defn find(s: String, needle: String) -> Int
defn starts_with(s: String, prefix: String) -> Bool
defn ends_with(s: String, suffix: String) -> Bool
defn substr(s: String, start: Int, end: Int) -> String
defn char_at(s: String, index: Int) -> String
defn replace(s: String, old: String, new: String) -> String
defn split(s: String, delim: String) -> List<String>
defn lines(s: String) -> List<String>
defn join(list: List<String>, delim: String) -> String
defn repeat(s: String, count: Int) -> String
defn pad_left(s: String, width: Int) -> String
defn pad_right(s: String, width: Int) -> String

// FFI (to be made native in Tier 2)
frgn __to_lower(s: String) -> Result<String, StringError>
frgn __to_upper(s: String) -> Result<String, StringError>
frgn __trim(s: String) -> Result<String, StringError>
frgn __UTF8_len(s: String) -> Result<Int, StringError>
```

### Collections (`collections.bv`)
**Status:** ✅ 100% native

```briev
// List operations
defn len<T>(list: List<T>) -> Int
defn append<T>(list: List<T>, item: T) -> List<T>
defn prepend<T>(item: T, list: List<T>) -> List<T>
defn concat<T>(a: List<T>, b: List<T>) -> List<T>
defn get<T>(list: List<T>, index: Int) -> T
defn set<T>(list: List<T>, index: Int, item: T) -> List<T>
defn remove<T>(list: List<T>, index: Int) -> List<T>
defn slice<T>(list: List<T>, start: Int, end: Int) -> List<T>
defn contains<T>(list: List<T>, item: T) -> Bool
defn find<T>(list: List<T>, item: T) -> Int
defn take<T>(list: List<T>, n: Int) -> List<T>
defn drop<T>(list: List<T>, n: Int) -> List<T>
defn is_empty<T>(list: List<T>) -> Bool

// FFI (requires higher-order functions)
frgn __filter<T>(list: List<T>, pred: T -> Bool) -> Result<List<T>, CollectionsError>
frgn __map<T, U>(list: List<T>, transform: T -> U) -> Result<List<U>, CollectionsError>
frgn __reduce<T, U>(list: List<T>, initial: U, reducer: (U, T) -> U) -> Result<U, CollectionsError>
```

### Time (`time.bv`)
**Status:** ✅ 100% native

```briev
defn duration_seconds(secs: Int) -> Int
defn duration_millis(ms: Int) -> Int
defn duration_minutes(mins: Int) -> Int
defn duration_hours(hours: Int) -> Int
defn duration_days(days: Int) -> Int

defn add_seconds(timestamp: Int, secs: Int) -> Int
defn add_minutes(timestamp: Int, mins: Int) -> Int
defn add_hours(timestamp: Int, hours: Int) -> Int
defn add_days(timestamp: Int, days: Int) -> Int

defn diff_seconds(t1: Int, t2: Int) -> Int
defn diff_days(t1: Int, t2: Int) -> Int
```

### IO (`io.bv`)
**Status:** ⚠️ FFI (requires OS access)

```briev
defn print(msg: String) -> Bool
defn println(msg: String) -> Bool
defn input() -> String

// File I/O (FFI)
frgn __read_file(path: String) -> Result<String, IOError>
frgn __write_file(path: String, content: String) -> Result<Void, IOError>
frgn __file_exists(path: String) -> Result<Bool, IOError>
```

### OS / File System (`os/fs.bv`)
**Status:** ✅ Native (2026-10-14, Increment 3) — real syscall file layer
over `SysCall#` (open/read/write/close, `SYS_*` numbers from
`std/posix/syscall.bv` via selective imports). A CAPABILITY module: NOT in
the prelude — import it explicitly where file I/O is used.

```briev
import "std/os/fs.bv";

defn read_file(path: String) -> Result<String, Int>
defn write_file(path: String, content: String) -> Result<Int, Int>  // Ok: bytes written (O_TRUNC)
defn file_exists(path: String) -> Bool
defn read_lines(path: String) -> Result<List<String>, Int>
defn read_line_count(path: String) -> Result<Int, Int>
defn count_lines(content: String) -> Int
```

E2E: `tests/fs_smoke.rs` (write→read→read_lines round trip, O_TRUNC
overwrite, missing-file `Err`).

### JSON (`json.bv`)
**Status:** ✅ 100% native

```briev
defn to_json(value: Object) -> String
defn from_json(json: String) -> Result<Object, String>
defn parse(json: String) -> Result<Object, String>
defn stringify(obj: Object) -> String
```

### HTTP (`http.bv`)
**Status:** ⚠️ FFI (requires network)

```briev
defn http_get(url: String) -> Result<String, String>
defn http_post(url: String, body: String) -> Result<String, String>
```

### Encoding (`encoding.bv`)
**Status:** ⚠️ Partial FFI

```briev
// Native
defn base64_encode(data: Data) -> String
defn base64_decode(s: String) -> Result<Data, String>
defn hex_encode(data: Data) -> String
defn hex_decode(s: String) -> Result<Data, String>
```

---

## Implementation Status

| Module | Total Functions | Native | FFI | % Native |
|--------|----------------|--------|-----|----------|
| **math** | 60 | 60 | 0 | 100% |
| **string** | 57 | 37 | 20 | 65% |
| **collections** | 43 | 30 | 13 | 70% |
| **time** | 12 | 12 | 0 | 100% |
| **io** | 6 | 3 | 3 | 50% |
| **json** | 4 | 4 | 0 | 100% |
| **http** | 2 | 0 | 2 | 0% |
| **encoding** | 4 | 2 | 2 | 50% |
| **option** | 12 | 12 | 0 | 100% |
| **result** | 14 | 14 | 0 | 100% |
| **hashmap** | 12 | 12 | 0 | 100% |
| **hashset** | 7 | 7 | 0 | 100% |
| **stack** | 8 | 8 | 0 | 100% |
| **queue** | 8 | 8 | 0 | 100% |
| **string_builder** | 12 | 12 | 0 | 100% |
| **TOTAL** | **299** | **229** | **40** | **77%** |

**Goal for Tier 2:** Increase native string functions from 65% → 95%

---

## Usage Examples

### HashMap
```briev
import "std/hashmap";

let map = new_map<String, Int>();
map = map.insert("age", 42);
map = map.insert("count", 100);

let age = map.get("age");
[age.is_some()] {
    let val = age.unwrap();
    println("Age: " + String(val));
};
```

### StringBuilder
```briev
import "std/string_builder";

let sb = new_builder();
sb = sb.append_str("Hello");
sb = sb.append_char(',');
sb = sb.append_char(' ');
sb = sb.append_str("World");
sb = sb.append_char('!');

let message = sb.to_string();  // "Hello, World!"
println(message);
```

### Stack
```briev
import "std/stack";

let s = new_stack<Int>();
s = s.push(1);
s = s.push(2);
s = s.push(3);

let (val, s) = s.pop();  // val = Some(3)
let top = s.peek();  // Some(2)
```

### Result Combinators
```briev
import "std/result";

let result: Result<Int, String> = Ok(42);

// Chain operations
let doubled = result_map(result, |x| x * 2);
let chained = result_and_then(doubled, |x| if x > 0 { Ok(x) } else { Err("negative") });

// Fallback
let fallback = result_or_else(chained, |err| Ok(0));
```

---

## File Organization

```
lib/std/
├── README.md              # This file
├── from-bits.bv           # How every type derives from Bits (educational)
├── math.bv                # Mathematical functions
├── string.bv              # String manipulation
├── collections.bv         # List operations
├── time.bv                # Time operations
├── io.bv                  # I/O (FFI)
├── http.bv                # HTTP client (FFI)
├── json.bv                # JSON serialization
├── encoding.bv            # Data encoding
├── option.bv              # Option type extensions
├── result.bv              # Result type extensions
├── hashmap.bv             # HashMap<K,V>
├── hashset.bv             # HashSet<T>
├── stack.bv               # Stack<T>
├── queue.bv               # Queue<T>
└── string_builder.bv      # StringBuilder
```

---

## Contributing

When adding new functions:
1. Prefer native implementations (`defn`) over FFI (`frgn`)
2. Include pre/post conditions
3. Add comprehensive tests
4. Update this README

---

*Last updated: 2026-05-06*
*Version: 0.11.0*
