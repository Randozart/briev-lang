# Plan 2026-09-22: ERC contention, asymmetric switch parts, whole-bus equality

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

The three remaining electronics-dialect gaps from the hardware-dialect-gaps
ledger. Each is independent; implemented as THREE separate commits under one
plan. Ascending surface complexity: ERC class semantics → asymmetric switch
parts (relay coil/contact) → whole-bus equality.

All three are property-driven where possible (Rules 14/15: the compiler
knows no class names — it reads `spec` properties generically).

## Slice 1 — ERC class semantics: contention (D6/D12 landing)

**Gap:** D6 promises `io_od` = open-drain = wired-AND by definition (two
`io` pins on one net = contention error; `io_od` may share); `nc` exempt.
The classes already carry `KicadType`/`CanDrive`/`NoConnect`, but **no
contention consumer runs** — the design record (lines 357-367) says
"semantics ship with the machinery that enforces them," and the machinery
does not exist.

**Changes:**
- `lib/std/electronics.bv`: `IoOd` gains `spec WiredAnd: true;` (open-drain
  may share a net = wired-AND). `Io`/`Out` do not.
- `src/parser/definitions.rs`: add `"WiredAnd" => Some("wired_and")` to
  `spec_name_to_key`.
- `src/analysis/electronics.rs`: `PinClassProps` gains `wired_and: bool`;
  read from `wired_and` metadata. New `check_contention` in `derive_netlist`:
  a net with ≥2 `CanDrive` pins where at least one is NOT `WiredAnd` (and
  not `shortcircuit`-acknowledged) → hard `contention_errors` violation.
  `IoOd`-only nets are legal (wired-AND). `Nc` exemption already handled by
  dangling.
- `ElectronicsNetlist` gains `contention_errors: Vec<String>`; emitter
  refuses on non-empty (same pattern as budget/decoupling).
- **Tests:** two `Io` pins on one net → contention error; two `IoOd` pins →
  legal; a `shortcircuit`-acknowledged pair → legal; `Nc`-exempt → legal.

## Slice 2 — Asymmetric switch parts (relay coil/contact)

**Gap:** `is_mechanism` (line 1894) requires **exactly one** Control pin. A
relay has a **coil pair** (2 Control pins) + 2+ Path pins, so it can't be a
bridge mechanism today.

**Changes** (all in `src/analysis/electronics.rs`):
- `is_mechanism`: accept `controls.len() ∈ {1, 2}`. One control (FET/gate)
  unchanged; two controls (relay coil) — both coil pins are driven from the
  condition net (the coil is one element across both pins).
- `synthesize_one`: for 2 control pins, union the condition net to BOTH coil
  pins; path pins unchanged.
- Update the zero-candidate error text: "one Control pin (or a two-pin coil)
  and at least two Path pins".
- Property-driven: the existing `Control`/`Path` classes distinguish coil vs
  contact. A relay is just a type with 2 `Control` + 2 `Path` pins. No new
  vocabulary.
- **Tests:** a `Relay` type (2 control + 2 path) synthesizes a bridge with
  both coil pins on the condition net; zero-candidate error text updated.

## Slice 3 — Whole-bus equality (new syntax, ranges over pin arrays)

**Gap:** pin arrays exist (`gpio[0]..gpio[7]`, E11) and `Expr::Range` exists
in the AST, but a range-indexed comparison (`u2.gpio[0..7] == u3.data[0..7]`)
does not expand element-wise.

**Syntax (confirmed with author):** range indexing —
`u2.gpio[0..7] == u3.data[0..7]`. Explicit length, element-wise expansion.

**Changes:**
- Parser: `gpio[0..7]` parses as `Field(Index(Field(inst, gpio), Range(0,
  7)), …)` — verify the index parser handles `[a..b]`; extend if needed.
- Analysis: in the precondition/wiring-fact collection, detect a
  range-indexed Field pair in an equality; expand to N element equalities
  (each element pair unions). A length mismatch (7 vs 8) is a hard error
  naming both lengths.
- `resolve_pin` stays element-wise; a new `expand_range_pairs` walks the
  equality and emits the element unions before union-find.
- Emitter: unchanged — the expanded unions produce the nets.
- **Tests:** 8-element bus equality produces the element unions (nets each
  pair); length mismatch errors; a single-index equality still works.

## Gates

- `cargo test --lib` green (sole environmental-probe failure unchanged).
- Praetor: no NEW diagnostics in changed files.
- Three commits: (1) ERC contention, (2) asymmetric relays, (3) whole-bus
  equality.
- Docs: ledger amendments after each slice.