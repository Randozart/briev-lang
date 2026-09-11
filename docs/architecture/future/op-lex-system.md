# op Lex — Extensible Literal System

**Status:** Future — design in progress, not implemented.
**Replaces:** `op Parse` (pre/suf/reg discriminator system)
**Related:** `docs/architecture/casting-protocol.md`, `docs/plans/2026-07-27-protocol-parse-op-system.md`

---

## 1. Problem

The lexer currently has 18 literal token variants with hardcoded type knowledge:

| Token | Pattern | Knowledge |
|-------|---------|-----------|
| `Integer(i64)` | `0x...` or `[0-9]+` | Hex prefix |
| `IntegerI8..U64` | `[0-9]+i8` etc. | Typed suffixes |
| `Float(f64)` | `[0-9]+\.[0-9]+` | Decimal point = float |
| `Float32/64` | `...[f32/f64]` | Typed float suffixes |
| `String(String)` | `"..."` with escapes | Escape sequences |
| `Char(char)` | `'...'` with escapes | Single char semantics |
| `BoolTrue` | `true` | Boolean keyword |
| `BoolFalse` | `false` | Boolean keyword |
| `Identifier(String)` | `[a-zA-Z_#$]...` | Named references |

Every new literal syntax (IPv4 addresses, SI unit suffixes, custom number formats)
requires a lexer change. The `op Parse` system (`docs/plans/2026-07-27-protocol-parse-op-system.md`)
was a step toward extensibility but is still tied to four hardcoded token
categories (`Decimal`, `Float`, `Bare`, `Quoted`) and uses `pre:`/`suf:`/`reg:`
discriminator fields that duplicate what a single regex could express.

## 2. Proposal

Replace `op Parse` with `op Lex` — three entry point variants that let types
self-declare how they interpret source text:

```briev
op Lex(Literal, regex):  fn(#L);   // matches raw text tokens
op Lex(Quoted):          fn(#L);   // matches paired-delimiter strings
op Lex(Keyword, word):   fn(#L);   // subscribes a specific identifier word
```

### 2.1 Lexer simplification

The lexer produces only three token categories:

| Token | What | Example |
|-------|------|---------|
| `Literal(String)` | Bare text — numbers, words, identifiers | `"42"`, `"0xFF"`, `"true"`, `"foo"`, `"0b1010"`, `"42i8"`, `"42km"` |
| `Quoted(Vec<u8>)` | Paired-delimiter with escapes resolved | `"hello\nworld"` → bytes `0x68 0x65 0x6c 0x6c 0x6f 0x0a 0x77...` |
| `Operator` | Single/multi-char punctuation | `+`, `->`, `(`, `{`, `;`, `.`, `,`, `=` |

Removed: `Integer(i64)`, `IntegerI8..U64`, `Float(f64)`, `Float32/64`,
`String(String)`, `Char(char)`, `BoolTrue`, `BoolFalse`, `Identifier(String)`.

The three irreducible lexer hardcodes (cannot be user-defined):
1. **Token boundaries** — operators split tokens (`42+13` → three tokens)
2. **Quote pairing** — matching `"..."` and `'...'` with escape resolution
3. **Comment/whitespace** — `//`, `/* */`, `\n`, ` ` are silently consumed

### 2.2 How it works

When the parser encounters a `Literal("42")` in an expression, it:
1. Checks expected type context (`: Int` annotation on the binding or parameter)
2. Looks up `type Int`'s `op Lex(Literal, ...)` declarations
3. Tests the regexes against `"42"`
4. Calls the matching handler to produce a typed value

For `Literal("0xFF")`:
```briev
type Int { op Lex(Literal, r"^0x[0-9a-fA-F]+$"): parse_hex(#L); };
```

For `Literal("42km")`:
```briev
type KiloMetre { op Lex(Literal, r"^[0-9]+km$"): parse_km(#L); };
```

For `Literal("true")`:
```briev
type Bool { op Lex(Keyword, "true"): parse_true(#L); };
```

### 2.3 Default protocols

Each protocol carries a default `op Lex` so `type Int: Int` works without
declaring one:

| Protocol | Default `op Lex` | Effect |
|----------|-------------------|--------|
| `Int` | `op Lex(Literal, r"^[0-9]+$"): parse_int(#L);` | Matches plain integers |
| `Float` | `op Lex(Literal, r"^[0-9]+\.[0-9]+$"): parse_float(#L);` | Matches decimal-point floats |
| `String` | `op Lex(Quoted): parse_string(#L);` | Accepts any quoted string |
| `Bool` | `op Lex(Keyword, "true"): parse_true(#L);` + `op Lex(Keyword, "false"): parse_false(#L);` | Two keyword entries |

Users can override or extend:
```briev
type HexInt: Int {
    op Lex(Literal, r"^0x[0-9a-fA-F]+$"): parse_hex(#L);
    // Int's default r"^[0-9]+$" still inherited for plain decimals
};
```

Or suppress defaults with an explicit flag if needed.

## 3. Open Design Questions

### 3.1 Keyword resolution timing

Two approaches for `op Lex(Keyword, "true")`:

**(a) Lexer-level resolution.** The lexer maintains a keyword registry populated
at compile time from `op Lex(Keyword, ...)` declarations. It checks each
identifier against the registry at lex time. If `"true"` is registered,
`true` becomes a keyword token; otherwise it's a plain `Literal`. Requires the
lexer to be stateful across compilation units.

**(b) Type-check-time resolution.** `true` is always `Literal("true")`.
When the expected type is `Bool`, `type Bool`'s `op Lex(Keyword, "true")` matches.
When used as a variable reference (`let true = 42;`), it works normally.
No lexer changes for keywords. May produce confusing errors when `true`
appears in an untyped context (the typechecker doesn't know whether it's a
Bool literal or a variable reference until it resolves all `op Lex` candidates).

**Trade-off:** (a) is more predictable (keywords are always keywords regardless
of context) but adds a lexer/typechecker dependency. (b) is architecturally
cleaner but may degrade error messages.

### 3.2 pre/suf — only on Quoted or everywhere?

Raw regex can express prefix and suffix patterns:
```briev
op Lex(Literal, r"^0x[0-9a-fA-F]+$"): parse_hex(#L); // prefix 0x
op Lex(Literal, r"^[0-9]+km$"):       parse_km(#L);  // suffix km
```

But for `Quoted`, the paired-delimiter model makes regex awkward:
```sql"SELECT"``` — with quotes, the raw text is `'"SELECT"'`. The regex
would need to match the opening quote, prefix, content, closing quote.
`pre:` and `suf:` on `Quoted` avoid this:

```briev
type Sql { op Lex(Quoted, pre:"sql"): parse_sql(#L); };
```

**Possible rule:** `Literal` gets regex only (pre/suf are subsumed by regex).
`Quoted` gets optional pre/suf discriminators (regex through paired delimiters
is impractical). `Keyword` has no regex — exact match only.

### 3.3 Single keyword vs combined

```briev
// Option A: one per keyword
op Lex(Keyword, "true"):  parse_true(#L);
op Lex(Keyword, "false"): parse_false(#L);

// Option B: combined
op Lex(Keyword, "true" | "false"): parse_bool(#L);
```

Option A keeps the parser function tail simple (no branching on the matched
keyword). Option B reduces boilerplate for types like `Bool` where both
keywords hit the same function. Option A is simpler to implement and error
messages are clearer (the declaration says exactly which word).

### 3.4 Quoted escape processing

Currently the lexer processes escape sequences (`\n` → `0x0A`, `\x41` → `A`,
`\u{0041}` → `A`). This is type-specific knowledge about what escapes mean.

If `String`'s `op Lex(Quoted)` handles the `#L` raw text, it must also
process escapes. This means the lexer can no longer resolve escapes (it
doesn't know what type will claim the `Quoted` token) — it must preserve
the raw text `"hello\nworld"` (with backslash-n, not the newline byte) and let
the type's parser function handle escape resolution.

But the lexer needs to produce one `Quoted` token per `"..."` — it must find
the closing `"` without treating `\"` as a terminator. This is the one place
where the lexer must understand `\` as an escape character to find token
boundaries, even if it doesn't interpret the escape.

**Design question:** Does the lexer resolve escapes (current behavior) and
pass processed bytes to `op Lex(Quoted)`, or does it preserve raw escaped text
and let the type's parser function handle interpretation? The former means
the lexer has escape knowledge. The latter means the lexer must at least
know that `\"` is not a token terminator.

### 3.5 TaggedLiteral interaction

Currently `42km` is lexed as `Integer(42)` with a suffix peek-ahead producing
`Expr::TaggedLiteral(42, "km")`. With `op Lex`, the entire string `"42km"` is
a single `Literal("42km")`. The regex `r"^[0-9]+km$"` matches the whole thing,
so the suffix becomes part of the raw text rather than a discriminator.

This means the parser function receives `#L = "42km"` and strips the `km`
suffix itself. That's cleaner (no special-case `TaggedLiteral` AST node) but
means the parser function needs to parse both the number and the suffix.

**Question:** Can we remove `Expr::TaggedLiteral` and `Expr::TaggedQuotedLiteral`
entirely, since suffixes are now part of the `Literal` raw text?

### 3.6 Inheritance semantics

When a type declares `type Binary: Int { op Lex(Literal, r"^0b[01]+$"): parse_binary(#L); }`,
does `Binary` also inherit `Int`'s default `r"^[0-9]+$"` regex? If yes, plain
integers are also valid `Binary` values. If no, `Binary` can only parse binary
syntax and plain decimals fail.

**Question:** Do explicit `op Lex` declarations fully replace the protocol
default, or append to it?

### 3.7 Ambiguity resolution

Multiple types may have `op Lex` declarations matching the same literal.
For `let x = 42;` without a type annotation, the parser must decide which
type wins. Options:

(a) **Expected type wins.** If the context declares `Int`, only `Int`'s
`op Lex` declarations are checked. No ambiguity.

(b) **First declared wins.** Iterate in declaration order, use the first match.
Fragile — sensitive to import order.

(c) **Error on ambiguity.** Report which types matched and require a type
annotation. Most predictable but most verbose.

(d) **Quantifier narrowing.** The type with the longest/most specific regex
wins. Hard to define "specificity" across independently developed types.

(e) **Protocol hierarchy priority.** A type closer to the match in the
protocol DAG wins. `type HexInt: Int` overrides `Int`'s default.

### 3.8 Optimizer fast path

With `op Lex`, every literal requires regex matching against type declarations.
For `for (i in 0..1000000) { ... }` the constant `0` must be parsed once at
compile time, not per-iteration. But the initial parse of `42` in `let x: Int = 42`
still requires at least one regex match.

A heuristic fast path in the compiler could try the most common patterns
(`r"^[0-9]+$"` for int, `r"^[0-9]+\.[0-9]+$"` for float) without running
the full regex engine. But this heuristic is a performance optimization only —
correctness must always go through the authoritative `op Lex` resolution.

**Question:** Is a heuristic fast path worth the complexity, or can the regex
engine handle the common cases fast enough?

## 4. Migration Path

**Phase 1:** Add `op Lex` as a parallel system to `op Parse`. Both work.
Stdlib types declare both.

**Phase 2:** Migrate all stdlib types from `op Parse` to `op Lex`.
Deprecate `op Parse` with a compiler warning.

**Phase 3:** Simplify the lexer: remove typed integer/float suffixes,
remove `BoolTrue`/`BoolFalse` keywords, keep only `Literal(String)` +
`Quoted(Vec<u8>)` + operators. Delegate interpretation to `op Lex`.

**Phase 4:** Remove `op Parse` and the old lexer token variants.
Remove `Expr::TaggedLiteral` and `Expr::TaggedQuotedLiteral`.

## 5. Summary

| Aspect | Current (`op Parse`) | Proposed (`op Lex`) |
|--------|----------------------|---------------------|
| Token categories | 4 hardcoded forms | 3 token types from lexer |
| Discriminators | pre/suf/reg fields | Regex on Literal; pre/suf on Quoted |
| Extensibility | Via 4 forms only | Any regex pattern |
| Lexer knowledge | 18 literal variants | 3 token types |
| Bool keywords | Hardcoded tokens | `op Lex(Keyword, ...)` |
| Typed suffixes | Hardcoded tokens | Regex on Literal |
| Hex/binary/octal | Hardcoded in lexer regex | User-defined via type |
| Escape processing | In lexer | In `String` type's parser fn (TBD: or lexer?) |
