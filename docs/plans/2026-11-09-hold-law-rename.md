# `when` law → `hold` law (2026-11-09)

## Decision

The static obligation form (`G ⟹ F₁ ∧ … ∧ Fₙ`, "the compiler must make it
so") is renamed from `when` to **`hold`**. `when` remains the runtime guard
inside `defn` / `node` / `txn` bodies — that meaning is unchanged.

```briev
// software, top level
hold usb_attached { j1.vbus.voltage = 5.0V; }

// software, obj body — a law of how the object must work
obj Sensor { hold temperature > 100 { thermal_alarm = true; } }

// lifecycle
hold drained { endprogram; }

// electronics, type body — conditional drive
type Regulator { pin in: Power; pin out: Power; pin gnd: Ground;
    hold in.voltage >= 5V { out.voltage = 3.3V; } }

// electronics, node body — conditional mechanism (strategy clause required)
hold u1.vout.voltage == 5.0V { d1.a = rly.c1; } through Relay;
```

**`hold` means one thing everywhere: a condition under which a fact holds by
obligation.** A bare `hold` is legal wherever a static law is a legal item —
top level, `obj` bodies, `type` bodies (and lifecycle arms at top level). In a
statement position (node/txn body) a bare `hold` is an error; the mechanism
form `hold … through <Type>` is the one statement-position shape, because the
mechanism wiring is compile-time structure, not runtime execution.

## Why

- The name described the *guard*, not the construct. The construct is an
  implication obligation with enforcement: propagate + verify, never
  synthesis.
- `pred` was rejected: 44 identifier collisions in shipped `.bv` code
  (`let pred: Float` in the recommender benchmarks) and a predicate is a
  boolean, not an obligation.
- `when` was positionally overloaded: top-level/`obj`/`type` = static law,
  body = runtime guard. One keyword now carries one load (`when` = execute
  when; `hold` = fact holds when). This follows Rule 3/21 (disclose special
  treatment, one semantic load).
- Electronics reads naturally: `hold condition { … } through <Type>`.

## Migration

Hard cut, no deprecation window:

- Top-level or `obj`/`type`-body `when` → compile error with the fix
  ("static laws use `hold`; `when` is the runtime guard inside bodies").
- `thru` → `through` (strategy marker; `thru` errors with the fix). The
  earlier `via → thru` rationale moves with the word: `via` is the PCB
  layer-hole term.
- Shipped corpus migrated: `lib/compiler/typechecker.bv`, electronics
  fixtures under `tests/electronics/`, `examples/electronics/`, inline test
  sources, and any spec/doc example.

## Scope of the change

- Lexer: `Token::Hold` (hard keyword). `Token::When` unchanged.
- Parser: `parse_top_level_hold_law`, `parse_hold_law` (obj/type body),
  `parse_hold_statement` (mechanism form, requires `through`).
- AST: `WhenLawDecl` → `HoldLawDecl`, `TopLevel::WhenLaw` → `HoldLaw`,
  `when_laws` → `hold_laws`; `analysis/when_law.rs` → `hold_law.rs`.
- Printers: `Display` and `canonical` emit `hold` for laws and for
  mechanism-guarded statements; `through` for the strategy marker.
- Diagnostics: every "when law" string becomes "hold law"; the runtime-guard
  wording keeps `when`.
- Vocabulary ledger (`src/vocab.rs`): `hold` canonical; version bump.
- Highlighter grammars (`.bv`, `.ebv`, `.sbv`, `.rbv`) + grammar validation.

## Semantics unchanged

The construct still means: under every satisfiable state where the guard
holds, the facts hold; a contradiction is a compile error; propagation is
derive-and-verify, never part synthesis. Position still scopes the law
(program-global vs declaration/instance). The rename carries no runtime or
codegen consequences; emitted IR for programs that do not use laws is
byte-identical.

## Verification

- Parser accept/reject matrix: `hold` top level / obj / type / lifecycle;
  `hold … through X;` mechanism; top-level `when` rejected with fix;
  `obj`-body `when` rejected with fix; body `when` still accepted; bare
  `hold` in a body rejected; `thru` rejected suggesting `through`.
- Lexer unit test for `Token::Hold`.
- Canonical/Display round-trip tests (law and mechanism forms).
- `cargo test --lib`, `cargo test --all`.
- Corpus sweep (141 files) and electronics parity fixtures.
- `benchmarks/hir_emit_ab.sh --quick` for unaffected-program byte identity.
- Praetor on changed directories.
