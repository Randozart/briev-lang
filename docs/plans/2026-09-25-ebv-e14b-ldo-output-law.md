# Plan 2026-09-25: E14b slice 8 — LDO output law (`spec Output`) and law-derived rail birth

**Status:** landed — all three slices (analysis `0b1cb778`, fixture + docs)
**Branch:** `feat/e14a-intent-synthesis`
**Design record:** E14b gate delta item 1 — the rail-BIRTH half of
"rails and returns stated as guard equalities". Slice 6 closed the
return half, slice 7 the membership half; this slice deletes the last
boundary condition: `u1.vout.voltage == 3.3V`. E14b's remaining work
after this lands is only E15 (placement/sizing), which is un-gated.

## Why

The fixture still states `u1.vout.voltage == 3.3V` in six places
(guards and posts of four nodes). It is a boundary condition — an
assumption the solver cannot derive — because the regulator declares
nothing about its own output. The record promised: "`u1.vout == 3.3V`
until the LDO output law (`spec Output`) lands." It lands now.

Two parts:

1. **The law itself** — trivially available. `spec Output: Volt` on the
   type + `when true { vout.voltage == Output; }` is an ordinary law
   parameter (the `ForwardVoltage`/`Resistance` path — generic parse,
   instance value folded by `spec_constant`, Volt dimension accepted by
   the DC solver). Zero parser work.

2. **The structural gap** — topology-phase rails come from CONTRACT
   FACTS ONLY. `collect_driven_rails_full` reads `pin.voltage == N`
   equalities from txn contracts; both the membership ladder (E14b-7)
   and the voltage-obligation forcing (E14b-1/2 pull-ups, low-hold)
   consume it. The moment the fixture stops stating `u1.vout == 3.3V`,
   the 3.3 V rail disappears from `rails` → `u2.vdd`/`u3.vdd`/`j2.vcc`
   find zero candidates ("no rail to join") and the pull-up forcing
   finds no rail to pull to. Component laws must be able to birth a
   rail.

## Design (derived from the record's locked intent — "`u1.vout == 3.3V`
until the LDO output law lands"; slice author 2026-09-25)

### 1. `spec Output` — a law parameter, not a new spec key

```briev
type Ldo {
    ...
    spec Output: Volt;              // dimension-only: instances must supply
    when true { vout.voltage == Output; }
};
stdnet<in: VBUS, vout: V3V3> let u1: Ldo = Ldo { value: "AP2112K-3.3",
    package: "SOT-25", spec Output: 3.3V };
```

- Type site: `parse_law_parameter_spec` stores `Identifier("Volt")` —
  the requirement marker. Instance site: `Quantity{3.3, Volt}`.
  `spec_constant` folds it (storage key `output` either way — the
  generic `name.to_lowercase()` fallback matches what a key would map
  to). Missing instance value → the existing named error
  ("references spec 'Output', but instance 'u1' supplies no value").
  Wrong dimension (`10Ohm`) → the existing unsupported-dimension error
  ("DC laws use Volt, Amp, and Ohm quantities"). Nothing new to parse.

### 2. Unconditional law (`when true`)

- Matches the fixture's semantics exactly: today every state's contract
  states `vout == 3.3V` unconditionally — the boundary was never
  gated on `en`. Sibling nodes (`i2c_idle`, `button_*`) do not re-drive
  `en`, so a conditional law (`when en.voltage >= …`) would leave
  vout undetermined exactly where the fixture needs it.
- The `en` pin's electrical effect on regulation stays unmodeled —
  the honest form is a future `mode enabled`/`mode disabled` on the
  LDO (precedent: `Spst` modes), noted as follow-on, not this slice.

### 3. Law-derived rail birth (the mechanism)

Detection over the elaborated law IR (available before topology —
`elaborate_law_ir` runs ahead of `force_return_topology` in
`derive_netlist`), per `ComponentLaws`:

- law guard is `LawGuard::Always` (guarded births are a later slice —
  conditional drives already have a precedent in `collect_when_drives`,
  but type-body guarded births are not modeled here);
- exactly one term in `expression`, the term is
  `LawVariable::Voltage(pin)`, dimension `Volt`, coefficient ≠ 0 →
  `volts = -constant / coefficient`;
- the pin's class carries `Supply: true` (same filter the fact path
  applies — a signal-level constant law is not a rail).

Yields `(net-root → volts)` entries and source pins, merged into
`collect_driven_rails_full`'s output so **both** consumers (membership
ladder, obligation forcing) see law rails with one extension:

- `collect_driven_rails_full(items, laws, ctx)` gains the law slice;
  callers thread it: `force_return_topology` (has it — law IR is
  elaborated upstream) and `collect_intents` → `force_voltage_obligations`
  (needs `laws` passed from `derive_netlist`).
- Source-pin treatment: `u1.vout` lands in `source_pins` → exempt from
  membership obligations AND its `stdnet<V3V3>` attachment binds there
  (the same loop that binds fact-source pins today). Proof line for
  the birth, D3 provenance — e.g. `rail born: u1.vout drives <net> at
  3.3V (component law)`.
- Rails take the max over contributors (fact + law) — agrees by
  construction when consistent.

Scope lines (documented, not implemented):

- **No participation awareness** — a `spec Output` rail persists in
  the absent state of OTHER parts exactly as fact drives do today
  (drives are pooled globally, not state-scoped). Unpop regulators
  remain unmodeled — same family as contract facts.
- **Modes do not birth rails** — only `laws`, not `modes`, are
  scanned (Spst-style equipotential/zero-current equations are not
  constant births anyway).

### 4. Drive-vs-law consistency is a hard error

The DC solver currently lets `values_to_solution` override a
law-solved net voltage with the contract drive silently. With
`spec Output` that disagreement becomes reachable by authoring
(`spec Output: 1.8V` + a surviving `vout == 3.3V` fact). Add the check
where the drive overrides the solved value: mismatch beyond epsilon →
hard law error naming both values and the fix (align the spec or
delete the fact). Consistent values (the fixture's case, if any fact
survives) stay silent.

### 5. Fixture migration — six sites, zero topology left

| Site | Today | After |
|---|---|---|
| `usb_powered` guard | `… && u1.vout.voltage == 3.3V && …` | deleted (rail births from the law) |
| `usb_powered` post | `[u1.vout.voltage == 3.3V && j1.vbus.voltage == 5.0V]` | `[j1.vbus.voltage == 5.0V]` — external reality stays forever |
| `i2c_idle` guard | `[u1.vout.voltage == 3.3V]` | `[usb_attached]` — powered-ness restated as the external trigger (§3.2's `usb_powered.up`, the landed doctrine: behavior members restate as facts; the port is the boundary the board does not own) |
| `i2c_idle` post | `[u1.vout.voltage == 3.3V]` | omitted (contract brackets are optional; a post would re-feed `collect_drives` and mask the law path) |
| `button_pressed` guard | `[u1.vout.voltage == 3.3V && sw1.closed]` | `[usb_attached && sw1.closed]` |
| `button_released` guard | `[u1.vout.voltage == 3.3V && !sw1.closed]` | `[usb_attached && !sw1.closed]` |

Posts of the button nodes carry no vout fact — untouched. The guard
equalities that are SIGNAL WIRING (`u1.vout == r_btn.a`,
`gpio[3] == sw1.a`, …) stay — they are the explicit statements E15's
placement backlog owns, not rail birth.

After migration **no `u1.vout.voltage == 3.3V` fact exists anywhere** —
so the fixture itself is the end-to-end proof that rails, membership
(`u2.vdd`/`u3.vdd`/`j2.vcc` join by expectation + refutation against
the LAW-driven rail), pull-up forcing, budgets, and the emitter all run
on component physics.

## Failure modes that must stay hard

- `spec Output: 1.8V` + a contract fact `vout == 3.3V` → drive/law
  mismatch error (new check, §4).
- `spec Output: 10Ohm` in a Volt law → existing dimension error.
- Instance omits `spec Output` → existing missing-parameter error
  naming `u1`.
- A follower supply pin with no rail to join (law deleted, no facts)
  → existing "no rail to join" enumerating driven rails.

## Slices

1. **Plan doc** (this file) — committed before code.
2. **Analysis**: law-rail birth in `collect_driven_rails_full` +
   threading + proof line + drive-vs-law mismatch check + tests:
   - `ldo_output_law_births_the_rail` — no contract fact names vout;
     follower supply pin joins (proof line) and post-solve
     `net_voltage` shows 3.3;
   - `law_rail_participates_in_expectation_refutation` — follower with
     `stdnet<V3V3>` joins the law-driven rail (expectation path);
   - `drive_disagreeing_with_a_component_law_is_a_hard_error`.
3. **Fixture + docs**: the six-site migration; emit gates; design
   record (E14b row → landed except E15, delta item 1 CLOSED);
   hardware-dialect ledger Amendment XXI; frontend doc dated entry;
   fixture header comment ("rail BIRTH … until the LDO output law
   lands" → landed).

## Gates (every commit)

- `cargo test --lib` green.
- Emits: `usb_sensor.ebv` 0 errors (membership proofs intact — the
  §3.2 guard form still compiles), `led_blinker`/`operating_states`/
  `spst_modes` 0, `operating_states_bound`/`spst_unknown_mode` exactly 1.
- Praetor normalized diff clean vs `/tmp/opencode/pr6-analysis-base.txt`
  (analysis) and `/tmp/opencode/pm2-parser-base.txt` (parser — should
  be untouched).
- Commit + push per slice.
