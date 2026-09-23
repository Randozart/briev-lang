# Order-Independent Modifier Keywords

**2026-09-22.** `<keywords>* <identifier> <name>` — modifier/strategy keywords
compose in any order, then exactly one structural identifier, then the name.

## Decisions (confirmed with the author)

1. **`bootstrap`** sits directly before `node` (`bootstrap node name`) — a
   fixed compound, NOT order-free.
2. **`async`** is prefix-only: `async node` / `async txn` accepted; the
   `node async` postfix (`parse_node_item:944` `eat(Async)`) is REMOVED.
3. **Unknown keyword** before a recognized modifier → hard parse error.
4. **`vol`/`mem`/`reg`** restricted to `let` (statement-level). No `vol node`.
5. **`async txn`** is allowed (txn is a reactive body too).
6. **Duplicate modifiers** (`seq seq node`) → error, not idempotent.

## The problem

Every site hand-rolls orderings as `if matches!(tokens.get(pos+1)…)` guards:

- Top-level (`definitions.rs:98-272`): `seq struct`, `pack seq struct`,
  `seq coll obj`, `coll seq obj`, `seq node`, `seq txn`, `async node`,
  `async accel node`, `accel node/txn`, `out defn/node/txn/let/vol`,
  `mem let`, `reg let`, `coll obj`, `coll struct`.
- Statement (`statements.rs:15-66`): `let`, `mem let`, `reg let`, `vol let`,
  `out let`, `out vol let`.
- Postfix: `node async` (`parse_node_item:944`).
- Field prefixes (`definitions.rs:3150`): `atomic relaxed`/`relaxed atomic`
  (already order-free, 2026-09-22), `seq atomic`.

Combinations like `vol out let`, `coll pack seq struct`, `seq accel node`
fail today; each new modifier adds more `pos+N` guards.

## The fix — one shared prefix scanner

On `Parser`, a new private fn (in `definitions.rs`, callable from
`statements.rs`):

```rust
struct ModifierPrefix {
    annotations: Vec<Annotation>,     // seq, pack, coll, accel, out, mem, reg, vol
    is_async: bool,                   // async (node | txn)
    sync_groups: Option<Vec<String>>, // sync<g> (node | txn)
}
fn consume_modifier_prefix(&mut self) -> Result<ModifierPrefix, SyntaxError>
```

Loop over peeked tokens:
- `seq`/`pack`/`coll`/`accel`/`out`/`mem`/`reg`/`vol` → consume, push
  Annotation. Duplicate → error (decision 6).
- `async` → consume, `is_async = true`. Duplicate → error.
- `sync` → consume `sync<group>`, set `sync_groups`. Duplicate → error.
- `bootstrap` → STOP (fixed compound, own arm).
- Structural identifier (`node`/`txn`/`let`/`struct`/`obj`/`type`/`defn`)
  → STOP (dispatcher handles).
- Anything else → STOP. If a modifier was already consumed and the next
  token is NOT a valid structural identifier for the collected set, the
  dispatcher errors (decision 3).

The dispatcher then validates the identifier against the modifier set:
- `sync<g>` + non-`node`/`txn` → error.
- `accel` + non-`node`/`txn` → error.
- `vol`/`mem`/`reg` at top level → must be `let` (else error).
- `bootstrap` handled separately.

## Replacement

1. **Top-level dispatch** (`definitions.rs:98-272`): collapse the ~15
   guard-chain arms into:
   - `Some(Token::Bootstrap)` → `parse_bootstrap_node()` (unchanged).
   - `Some(m)` where m ∈ {Seq, Pack, Coll, Accel, Async, Out, Sync, Mem,
     Reg, Vol} → `consume_modifier_prefix()`, then match the structural
     token and delegate, applying annotations + `is_async` + sync wrap.
2. **Statement-level** (`statements.rs:15-66`): `let` + modifier arms →
   consume modifier prefix, require `let`, call `parse_let_statement`, push
   annotations. `out vol let` and `vol out let` both parse.
3. **`async` postfix removal**: `parse_node_item:944` drops `eat(Async)`.

## Compatibility

All currently-valid orderings keep working (tests pin `seq coll obj`, `pack
seq struct`, `out vol let`, `async node`, `seq node`). Newly valid:
`vol out let`, `coll pack seq struct`, `seq accel node`, `async txn`,
`sync<g> async node`, etc.

## Tests

- Order permutations: `vol out let`, `out vol let`, `coll pack seq struct`,
  `seq coll pack struct`, `seq accel node`, `async txn`, `sync<g> async
  node`, `accel seq node`.
- Rejections: `node bootstrap`, `node async` (postfix), `seq foo`,
  `sync<g> let`, duplicate `seq seq node`, duplicate `async async node`.
- Existing ordering tests stay green.

## Files

- `src/parser/definitions.rs` — scanner + top-level dispatch + postfix removal.
- `src/parser/statements.rs` — statement-level let dispatch.
- `src/parser/helpers.rs` — plumbing if needed.
- `spec/SPEC.md` §9/§15.3 — document the ordering rule.
- `docs/plans/2026-09-22-syntax-corpus-completion.md` — vol-let example
  unblocked by the statement dispatch.

## Gates

- `cargo test --lib` green (conformance sweep included).
- Corpus `vol-let.bv` example now parses (`vol let` at statement level).