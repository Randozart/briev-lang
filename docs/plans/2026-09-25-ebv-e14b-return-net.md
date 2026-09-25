# Plan 2026-09-25: E14b slice 6 — return-net inference & decoupler auto-bridging

**Status:** landed 2026-09-25
**Branch:** `feat/e14a-intent-synthesis`
**Design record:** `2026-09-21-intent-synthesis-node-semantics.md` — E14b gate
delta item 1 ("rails and returns stated as guard equalities — E14b deletes
them"), sliced.

## Why

The gate fixture's `usb_powered` guard carries 27 equalities; 17 of them are
return-side bookkeeping (ground unions, decoupler cap attachments, the switch
return path). Every one is choice-free — the compiler can derive it from two
already-declared interfaces:

1. **Return class** (D6): a pin whose class declares `spec Return: true`
   (stdlib `Ground`) is the board's return. One board, one return net —
   all Return-class pins of populated instances union. This is class
   semantics, not inference-with-alternatives: there is nothing to enumerate.
   An author who needs isolated returns does not ascribe `Ground` (split
   returns stay explicit under ordinary equalities — documented, not solved).
2. **Decoupling convention** (E13): every `spec Decouple` instance must have
   a populated `spec Decoupler` part bridging each supply-class pin to a
   return-class pin. Today this is CHECK-only on the finished netlist. The
   obligation is itself the forcing rule: for each unbridged supply pin, a
   two-pin Decoupler part wires its return-side pin to THE return net and its
   supply-side pin to the supply pin's net. Symmetric pins → the pick is
   immaterial (deterministic, same rule as bus assembly's bijection).

Supply-rail *membership* (which driven rail a `spec Supply` pin joins when
several are tolerance-compatible — `u1.in`, `u3.vdd`, `j2.vcc`) is the
remaining fork and is explicitly OUT of scope; it needs a design decision
(ambiguity policy) and stays explicit in the fixture.

## Scope

### Slice 1 — return-net union

- New pass in `derive_netlist`, after `collect_participation` (needs `unpop`),
  before `group_pins`: union all pins whose resolved class declares
  `spec Return: true` across populated instances into one net.
- Unpopulated instances contribute no pins (absent state has no copper).
- Proof line per instance ("return pin `x.gnd` on the return net") —
  wiring-report provenance, D3.
- Zero Return pins → no return net; Slice 2 obligations report it (D13).

### Slice 2 — decoupler auto-bridging (check → force)

- Upgrade the E13 convention: before checking, for each populated
  `spec Decouple` instance supply pin whose net lacks a bridge, take a free
  populated Decoupler part (all pins unconnected) and wire pin[0] → return
  root, pin[1] → supply root (sorted pin names; two-pin parts only).
- A Decoupler type without exactly two connectable pins is not auto-wired —
  the existing check reports the unbridged supply pin (what/why/fix).
- No return net (Slice 1 empty) and a bridge demanded → hard error naming
  the obligation.
- `unpop` decouplers never bridge (landed E14a gate rule) and their pads
  stay exempt from dangling (participation absent-state).
- Low-hold forcing (`force_low`) gains the p1-pre-wired case: a Switchable
  part with one path pin on the obligation net and the other free wires the
  free pin to the return root (today only the fully-free and p2-pre-wired
  forms are handled). Same D13 surface.

### Slice 3 — fixture + docs

- `usb_sensor.ebv`: delete the 17 return-side equalities; update the
  `// E14b:` marker comment (rails-membership equalities remain, marked as
  the next slice); note the DNP cap's pads now dangle by construction.
- Ledger Amendment XVIII; design record E14b row (slice 6 landed, remaining:
  supply-rail membership + series placement); frontend doc dated entry;
  this plan's Status.

## Out of scope (backlog)

- Supply-rail membership inference (the tolerance-refutation /
  enumerated-ambiguity fork — needs a design decision).
- LDO output law (`spec Output` + `when` law so `u1.vout == 3.3V` stops
  being a guard fact) — component-laws follow-on.
- Series-resistor placement (E15, trigger-gated), value-aware pull-up
  matching (landed), split/analog returns.

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics on touched surfaces
(baseline-diff method) · fixture derives clean · emits unchanged for
`led_blinker` · new tests: return pins union; unpop contributes nothing;
decoupler auto-bridges both sides; unpop decoupler never bridges; two-pin
requirement; no-return-net error; p1-pre-wired switch completes to return;
fixture guard shrinks (17 equalities deleted) with identical nets.

## Doc maintenance

Ledger Amendment (E14b-6 landed). Design record E14b row + gate delta
note. Frontend doc entry. Fixture comments.

## Landed deviations (same day)

- **Bridge test is net-level** (inherited from `check_decoupling`): pins
  sharing an already-bridged net demand no new part, so the fixture's
  caps size to DISTINCT supply nets (`c[i:2]` for VBUS and 3v3), not to
  supply pins. Spare caps would dangle — dangling is still a hard error.
- **Button low bound moved into the node body**: postcondition bounds
  VERIFY, they do not force — `wire_low` only sees body obligations
  (the landed E14b-2 surface, same as the i2c node).
- **Stdlib `Spst` pins ascribe `: Path` again** — the E14a fixture's
  local Switch had the mechanism class; the SPST-modes migration dropped
  it, leaving the button invisible to low-hold forcing.
- Error-matrix case 2 (dropped switch return) now COMPILES — the
  obligation forces the path — so its test asserts the forcing proof;
  case 3 (removed decap) means shrinking the population (`c[i:2]` →
  `c[i:1]`), which leaves u1.vout's net unbridgeable.
