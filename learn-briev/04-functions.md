# Functions with Contracts

> **Runnable companion:** `learn-briev/examples/04-functions.bv` compiles, runs, and its output is pinned in CI (`tests/learn_briev_examples.rs`) — the machine-checked face of this chapter.


Functions (`defn`) are pure computations. Unlike transactions, they don't mutate
state outside themselves — their result reaches the outside only through a
caller's assignment.

## 1. Basic Functions

```briev
defn add(a: Int, b: Int) -> Int {
    term a + b;
};

defn is_positive(n: Int) -> Bool {
    term n > 0;
};
```

**Parts:**
- `defn` - Keyword
- `add` - Function name
- `(a: Int, b: Int)` - Parameters with types
- `-> Int` - Return type
- `term a + b` - Return value

A `defn` cannot read or write global state. Its return type is inferred from
`term`, so `-> Int` is optional.

## 2. Explicit vs Implicit Return

A `defn`'s return type may be **explicit** (`-> Int`) or **implicit**
(omitted — inferred from the `term` value):

```briev
// Explicit — self-documenting, useful for public interfaces
defn add(a: Int, b: Int) -> Int {
    term a + b;
};

// Implicit — concise, fine for private helpers
defn add_implicit(a: Int, b: Int) {
    term a + b;
};
```

**When to use explicit `-> Type`:**
- The function is part of a public interface (a library, an FFI surface)
- The return type is not obvious from the body (e.g. a `Bool` predicate
  derived from a numeric comparison, or a tuple)
- You want a compile-time assertion that the `term` value really has the
  declared type

**When to use the implicit form:**
- Private helpers whose body makes the type obvious
- Rapid prototyping before the interface settles
- Contracts already pin down the shape of the result (`result == a / b`
  tells the reader the result is a value of the same type as `a`)

## 3. Contracts (Optional but Recommended)

Contracts come after the parameters and may appear **before** OR **after**
the `-> Type` return type — both are equivalent:

```briev
// ✅ Contract after the return type
defn safe_divide(a: Int, b: Int) -> Int [b != 0][result == a / b] {
    term a / b;
};

// ✅ Contract before the return type (same meaning)
defn safe_divide2(a: Int, b: Int) [b != 0][result == a / b] -> Int {
    term a / b;
};

// ✅ No contract needed for straight-line code
defn divide(a: Int, b: Int) -> Int {
    term a / b;
};
```

When the return type is omitted, the contract follows the parameters
directly:

```briev
defn safe_divide3(a: Int, b: Int) [b != 0][result == a / b] {
    term a / b;
};
```

## 4. Multiple Return Values

```briev
defn div_mod(a: Int, b: Int) [b != 0][quotient * b + remainder == a] -> (Int, Int) {
    term (a / b, a % b);
};

defn get_coords() -> (x: Int, y: Int) {
    term (10, 20);
};

// Usage
let (q, r) = div_mod(10, 3);  // q = 3, r = 1
let (_, remainder) = div_mod(42, 5);  // _ discards the quotient
let coords = get_coords();
let x = coords.0;
let y = coords.1;
```

**Note:** Tuple destructuring works with `_` as a discard placeholder.
For nested generics like `Ptr<Ptr<Int>>`, add a space: `Ptr<Ptr<Int> >`
to avoid the `>>` shift-right token.
```

## 5. Named Return Values

```briev
defn get_person() -> (name: String, age: Int) {
    term ("Alice", 30);
};

let person = get_person();
println(person.name);  // "Alice"
println(person.age);   // 30
```

## 6. Guards in Functions

```briev
defn abs(n: Int) [true][result >= 0] {
    when n < 0 {
        term -n;
    };
    term n;
};

defn max(a: Int, b: Int) [true][result == a || result == b] {
    when a >= b {
        term a;
    };
    term b;
};

defn clamp(val: Int, min_val: Int, max_val: Int) 
    [min_val <= max_val]
    [result >= min_val && result <= max_val] 
{
    when val < min_val {
        term min_val;
    };
    when val > max_val {
        term max_val;
    };
    term val;
};
```

## 7. Recursive Functions

```briev
defn factorial(n: Int) [n >= 0 && n <= 20][result >= 1] {
    when n == 0 || n == 1 {
        term 1;
    };
    term n * factorial(n - 1);
};

defn fibonacci(n: Int) [n >= 0][result >= 0] {
    when n == 0 {
        term 0;
    };
    when n == 1 {
        term 1;
    };
    term fibonacci(n - 1) + fibonacci(n - 2);
};

defn gcd(a: Int, b: Int) [a >= 0 && b >= 0][result >= 0] {
    when b == 0 {
        term a;
    };
    term gcd(b, a % b);
};
```

## 8. Functions with Generics

```briev
defn identity<T>(x: T) [true][result == x] {
    term x;
};

defn swap<T, U>(a: T, b: U) [true][result.0 == b && result.1 == a] {
    term (b, a);
};

defn first<T, U>(pair: (T, U)) -> T {
    term pair.0;
};

// Note: `[true][true]` is rejected. Use `[[post]` (post-only) or `[pre]]` (pre-only).
// The `[[post]` and `[pre]]` sugar fill the omitted side as `[true]`.
```

**2026-08-14:** at each call site the compiler infers the type params from
the arguments — `identity(5)` binds `T = Int`, `identity("hi")` binds
`T = String` — and substitutes them into the parameter and return types.
Codegen is type-erased: the body is emitted once, and a `T`-typed value is a
boxed i64. A nullary generic (`new_map<T, U>()`) binds its params from the
enclosing binding's annotation when the arguments don't constrain them. An
argument that can't unify with the parameter shape is a type error.

```briev
defn first_of<T>(xs: List<T>) -> T [Count#(xs) > 0][true] {
    term At#(xs, 0);
};
let items: List<Int> = [1, 2, 3];
let n: Int = first_of(items);   // T = Int, n is Int
```

## 9. Derivation Blocks — Synthesis by Example

Functions can be defined by examples rather than by hand-writing a body.
The compiler searches for an expression that matches all examples.

```briev
defn add(x: Int, y: Int) -> Int := {
    2, 3 -> 5;
    0, 0 -> 0;
    10, -3 -> 7;
};
```

Run `briev derive` to synthesize the body, which produces a `.derive.bv`
shadow file. Review it, then run `briev accept` to fold it back.

### Reference Function

Use an existing function as the specification:

```briev
defn popcount_ref(x: Int) -> Int {
    term ((x & 1) + ((x >> 1) & 1));
};
defn popcount(x: Int) -> Int := popcount_ref;
```

The `:= ref_fn` form copies the reference's body directly.
Combined with examples for verification:

```briev
defn popcount(x: Int) -> Int := { 0 -> 0; 1 -> 1; } := popcount_ref;
```

### Postcondition Contracts

The `[[post]` syntax defines postconditions verified during synthesis:

```briev
defn popcount(x: Int) -> Int := {
    0 -> 0; 1 -> 1;
} [[ #Term >= 0 && #Term < 64 ];
```

`#Term` is a hashword that refers to the function's return value.

### Tolerance for Floating-Point

Each example can specify a tolerance for fuzzy comparison:

```briev
defn sqrt(x: Float) -> Float := {
    1.0 -> [0.01] 1.0;
    4.0 -> [0.01] 2.0;
} [[ #Term >= 0 ];
```

### CLI

```bash
brievc derive file.bv                 # Synthesize all derivation blocks
brievc derive --stochastic file.bv    # Also run MCMC superoptimizer
brievc derive --enumerative-depth 4   # Search deeper for better formulas
brievc accept file.bv                 # Fold bodies back into source
brievc build file.derive.bv           # Build with assertion verification
```

## 10. Complete Example

```briev
// math_utils.bv

defn power(base: Int, exp: Int) 
    [exp >= 0]
    [result >= 1]
{
    when exp == 0 {
        term 1;
    };
    when exp == 1 {
        term base;
    };
    let half = power(base, exp / 2);
    when exp % 2 == 0 {
        term half * half;
    };
    term base * half * half;
};

defn is_prime(n: Int) 
    [n >= 0]
    [result == true || result == false]
{
    when n < 2 {
        term false;
    };
    when n == 2 {
        term true;
    };
    when n % 2 == 0 {
        term false;
    };
    
    let i: Int = 3;
    when i * i <= n {
        when n % i == 0 {
            term false;
        };
        i = i + 2;
    };
    term true;
};

defn sum_range(start: Int, end: Int) 
    [start <= end]
    [result >= 0]
{
    let n: Int = end - start + 1;
    term (start + end) * n / 2;
};

node run [true][p == 1024 && prime && sum == 5050] {
    let p = power(2, 10);  // 1024
    let prime = is_prime(17);  // true
    let sum = sum_range(1, 100);  // 5050
    term;
};
```

## Exercises

1. Write a `min` function that returns the smaller of two values
2. Implement `is_even` and `is_odd` with contracts
3. Create a `fibonacci_fast` function using tail recursion
4. Write a `quadratic_formula` function that returns both roots

---

*Next: [05-data-types.md](05-data-types.md) - HashMap, HashSet, Stack, Queue*
