# Phase 2b — length facts for handles (`.^Length`, `CharCount#`)

**Status:** PLAN → implementing.
**Follows:** Phase 2a (`feat/verify-string-handles`, merged) which admitted
`String`/`Data` as opaque word handles and erased representation casts.

## 1. Where we are (measured, 92-file `lib/std`, release `brievc`)

```
proven 1015 | unproven 5 | skipped 14438 | violated 0 | errors 2
```

Phase 2a gained +90 proven and opened one leaf (`byte_raw`). The next gate is
what those bodies hit now:

| defn | current reason | needs |
|---|---|---|
| `byte_at` | `reflection` | `s.^Length` as an integer |
| `str_to_bool` | `reflection` | `s.^Length` |
| `truncate` | handle-returning skip | `CharCount#(term)` related to a length |
| `count_lead`, `briev_char_len` | `a call to … no value summary` | `byte_at`'s summary |

`.^Length` is **stored-length reflection** (spec §17.1): on a `String` it is the
byte count read from the header. It is already a *property-shaped* expression —
`s.^Length` (no space) — and the parser accepts that spelling.

## 2. Design

### 2.1 A handle carries a length atom

A handle value `h` gets a companion integer atom `len(h)`. Nothing else: the
prover still knows nothing about the bytes.

- `Expr::Field(base, "^Length")` — the AST form of `s.^Length` — evaluates to a
  fresh atom keyed by the *base expression*, so `s.^Length == s.^Length` holds
  structurally while `a.^Length` and `b.^Length` stay distinct.
- `CharCount#(h)` is the same atom for `String` handles (character count equals
  stored length for the ASCII default; a variant with a different encoding is a
  separate gate — see §2.4).

### 2.2 The facts that make it useful

Two rules, both exact:

1. **Non-negativity.** `len(h) >= 0` for any handle — a stored length is a
   count. This discharges `[term >= 0]` for `byte_at`'s sibling accessors.
2. **Bounds from `Load#`.** `str_len_bytes` reads the header with an 8-byte load;
   that is a full word and carries no unsigned bound (phase 2a's deliberate
   limit), so no rule is added there. `byte_at` does not need it.

### 2.3 What this unlocks

`byte_at`'s body is the shape that matters:

```briev
defn byte_at(s: String, i: Int) [true][term >= 0 && term <= 255] -> Int {
    let addr: Int = (s as Data) as Int;
    when i < 0 || i >= s.^Length { term 0; };
    term Load#(addr + 8 + i, 1);
};
```

With handles admitted (2a) and `s.^Length` modelled as an integer atom (2b), the
guard `i >= s.^Length` no longer mentions reflection, so the body reaches the
path walk, and both paths discharge `[term >= 0 && term <= 255]`:
`term 0` trivially, and the load through the `Load#` width range.

### 2.4 Explicit non-goals

- **Content** — no byte-level reasoning about what is stored.
- **Cross-encoding lengths** — `CharCount#` is modelled as the stored length;
  a `String<UTF16>`-style variant whose character count differs from its byte
  count is *not* covered. The atom is only claimed for the default handle shape,
  and a contract that relates `CharCount#` to a non-default variant stays
  skipped rather than proved.
- **`Capacity#`** — a different header field, no facts stated.
- **String concatenation algebra** — `CharCount#(a + b) == CharCount#(a) +
  CharCount#(b)` is true but is a property of `op Concat`, not of the header; out
  of scope, stays skipped.

## 3. Implementation shape

1. `briev-verify/src/symbolic.rs`: `Expr::Field(base, f)` where `f == "^Length"`
   → `SymbolicValue::Identifier(format!("len#{base}"))`. Keyed on the base's
   *symbolic* value, so distinct handles stay distinct and identical bases unify.
2. `Call("CharCount#", [x])` → the same atom as `Field(x, "^Length")`.
3. `rel_verdict`/`bounds_of`: fold `len(h) >= 0` into the bounds map whenever the
   atom appears, so `>= 0` goals discharge without a solver round trip.
4. Admission: `.^Length` and `CharCount#` stop being "reflection"/"unsupported"
   for admitted handles; a non-handle base (e.g. `.^Length` on an `Int`) stays
   refused.

## 4. Gates (must all hold before merge)

- `brievc build lib/std/byte_at.bv --verify-bodies` reports **no** skip for
  `byte_at`, and the corpus `proven` rises.
- **`violated` stays 0** on the 92-file corpus.
- A test asserts `s.^Length` on a handle is `>= 0`, and that two different
  handles' lengths are **not** interchangeable.
- A test asserts `CharCount#` on a non-admitted (Float) value is still refused.
- `unproven` must not grow: a length fact may only *discharge* goals, never
  refute one.

## 5. Risks

- **Atom identity.** Keying on the symbolic base is what keeps `a.^Length` and
  `b.^Length` apart; keying on the *name* alone would let `x.^Length` unify
  across a rebinding. The base is already an immutable entry atom in
  `SymbolicState`, which is exactly the identity wanted.
- **`CharCount#` ≠ bytes for non-ASCII variants.** Stated as a non-goal and
  guarded by admitting the atom only for the default handle model.
