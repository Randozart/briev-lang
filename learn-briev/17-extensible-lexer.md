# Extensible Lexer — Literal Parsing via `op Parse`

Briev's literal syntax is not hardcoded. The lexer's job is purely mechanical:
split source text into raw tokens (barewords, numbers, quoted strings) and
identify prefix/suffix discriminators. It never needs to know what any
discriminator means — that is delegated to the type's `op Parse(Form, pre: "suffix")`
declaration.

Adding a new literal syntax is one line in a `.bv` file. No compiler changes.

---

## 1. The Default Parse Ops

Every primordial type comes with `op Parse(#Category)` — the identity parse:

```briev
type Int {   // in lib/std/types/bootstrap.bv
    op Parse(Int);      // 42 → Int (identity, zero-cost)
    op Parse(Decimal);   // also accepts numeric literals
};

type String {
    op Parse(String);     // "hello" → String (identity, zero-cost)
};
```

These give every type a default way to be constructed from source text.

---

## 2. Custom Number Bases (Binary, Octal)

Other number bases are defined in the standard library without touching
the compiler's lexer:

```briev
type Int {
    op Parse(Decimal, pre: "0b") = parse_binary(#L);  // 0b1010 → 10
    op Parse(Decimal, pre: "0o") = parse_octal(#L);   // 0o77 → 63
};
```

The lexer sees `0b1010` as a numeric token with discriminator `0b`.
The typechecker routes it to `op Parse(Decimal, pre: "0b")`.
The function `parse_binary` runs at compile time and returns the value.

---

## 3. Unit Suffixes (Milliseconds, Pixels)

Suffixes attach naturally to numbers and read like natural language:

```briev
type Milliseconds {
    data: Bits<64>;
    op Parse(Decimal, suf: "ms") = parse_ms(#L);
};

type Pixels {
    data: Bits<32>;
    op Parse(Decimal, suf: "px") = parse_px(#L);
};

// Usage:
let timeout = 250ms;       // → Milliseconds{250}
let width = 1080px;        // → Pixels{1080}
```

The lexer tokenizes `250ms` as a numeric value `250` with suffix `ms`.
The typechecker routes it to `Milliseconds.Parse(Decimal, suf: "ms")`.

---

## 4. Color Literals via Prefix

Colors can use clean, readable syntax without `#` (which is reserved for
hashwords):

```briev
type Color {
    r: Bits<8>;
    g: Bits<8>;
    b: Bits<8>;
    a: Bits<8>;
    op Parse(Bare, pre: "rgb") = parse_rgb_color(#L);
};

// Usage:
let accent = rgbFF0055;    // → Color{255, 0, 85, 255}
```

The prefix `rgb` is an alphanumeric identifier — safe, no parser ambiguity.
The lexer sees `rgbFF0055` and routes it to the `Color` type's Parse op.

---

## 5. Regular Expressions as Literals

Raw string-like syntax for regex:

```briev
type Regex {
    data: Bits<64>;  // pointer to compiled regex
    len: Bits<64>;
    op Parse(Quoted, pre: "r") = compile_regex(#L);
};

// Usage:
let email = r"^\\w+@\\w+\\.\\w+$";
// → Regex compiled at compile time from the raw string
```

The `r` prefix on a quoted string routes to `Regex.Parse(Quoted, pre: "r")`.
The regex is compiled at compile time — zero runtime cost.

---

## 6. What Is Not Allowed

The `validate_discriminator` function rejects symbols that conflict with
language operators:

| Symbol | Reason | Example that would fail |
|---|---|---|
| `#` | Hashword prefix for categories and intrinsics | `#FF0000` — would be parsed as hashword `#FF0000` |
| `!` | Terminal bang (`term!`) | `!important` |
| `@` | Expression escape (`@FF00FF`) | already handled by escape syntax |
| `&` | Address-of operator | `&value` — already means pointer |
| `(` `)` `[` `]` `<` `>` | Grouping, indexing, generics | would break parser structure |
| `"` `'` | String/char delimiters | would break literal boundaries |

Only alphanumeric identifiers (`a-z`, `A-Z`, `0-9`) are allowed as
discriminator prefixes and suffixes. This keeps the lexer deterministic:
it never needs to backtrack to figure out whether a token is a discriminator
or something else.

---

## 7. Full Pipeline

```
Source text:
  "250ms"            "rgbFF0055"         "0b1010"
     │                    │                  │
     ▼                    ▼                  ▼
  Lexer:               Lexer:              Lexer:
  Token(250, "ms")     Token("FF0055",     Token(10, "0b")
                        "rgb")
     │                    │                  │
     ▼                    ▼                  ▼
  Typechecker:          Typechecker:        Typechecker:
  Routes to             Routes to           Routes to
  Milliseconds.         Color.              Int.
  Parse(Decimal,        Parse(Bare,         Parse(Decimal,
  suf: "ms")            pre: "rgb")         pre: "0b")
     │                    │                  │
     ▼                    ▼                  ▼
  Codegen:              Codegen:            Codegen:
  i64 250               [8 x i8]            i64 10
```

The lexer produces raw tokens. The typechecker routes via Parse ops.
The normalizer resolves layout. LLVM emits the statically known values.

No proc macros, no compiler plugins, no special-cased lexer states.
A new literal syntax is one line in a `.bv` file.
