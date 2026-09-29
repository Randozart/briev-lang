# EBV Capability + Scale Probe (2026-09-28)

Probe boards in `probe/electronics/`. Compiled with release `brievc`, gated with
`kicad-cli` 10.0.6 (`sch erc --severity-error`). Goal: measure the extent of PCBs
the compiler can produce — not correctness (that's the E-series gate fixtures' job).

## Boards

| Board | Shape | Compile | Sheets | KiCad ERC | Notes |
|---|---|---|---|---|---|
| `probe1_led_i2c.ebv` | mixed passive+active, intent synth, `unpop`, SeriesPart, fab | ~1s | 1 | **6** | 14 PCB footprints; deterministic |
| `probe2_amp_chain.ebv` | dual op-amp ×2 (multi-unit) + pin array | ~1s | 1 | **9** | 6 unit instances (2amp×3unit) |
| `scale_1024.ebv` | 32×32 wildcard matrix | 1.2s | 17 | 0 | |
| `scale_4096.ebv` | 64×64 wildcard matrix | 21s | 65 | 0 | at parser cap |
| `scale_8192_2array.ebv` | 2× 4096 arrays | 119s | 129 | 0 | past the per-array cap |

All scale boards: **byte-identical across 2 runs** (determinism holds to 8192).

## Scale / extent

- **Hard cap:** 4096 elements per instance-array declaration — parser,
  `src/parser/statements.rs:232` (`total > 4096`). Larger = multiple arrays, each
  ≤4096, wired by separate `t[*]` broadcasts.
- **Sheets:** 1 master + `ceil(N/64)` children (BANK_SIZE=64) + 1 connector sheet.
  4096 → 65 children; 8192 → 129.
- **Compile time** is superlinear (net-wiring is O(nets·pins)): 1.2s / 4.4s / 21s
  at 1024/2048/4096, then **119s at 8192** (2× parts, 5.6× time).
- **Child sheet** is uniform (792 lines at 64 resistors) regardless of board size.

## ERC findings (the real "extent" limit)

**"ERC-clean" in the E-series gate = netlist-level zero hard errors (Rust
`assert_clean`), NOT KiCad ERC.** KiCad ERC is a stricter, separate check the
emitter is not gated on. Concretely:

1. **`power_pin_not_driven`** — every power symbol emits a `power_in` pin
   (`mod.rs:682`). KiCad wants each `power_in` driven by a `power_out`; none are,
   so every rail symbol flags. The committed gate fixture `usb_sensor` (11 power
   symbols) → **5 of these**. `tile_512` is ERC-clean **only because it has zero
   power symbols** (512 resistors on 2 labeled nets).
2. **`pin_not_connected`** — `unpop`'d parts emit open pads (the documented DNP
   shape). `usb_sensor`'s `unpop c_dnp` → 2; a second manual cap on a net the
   decoupling-convention already bridged also dangles.
3. **`label_dangling`** — some net shapes place the global label in the
   inter-column channel not touching a wire endpoint.

So: **boards with no power symbols (resistor/tile matrices) are fully ERC-clean.
Any board with rails (i.e. real boards) is not** — until the emitter emits
`power_out` sources (fix for finding 1) and `unpop`/label handling is adjusted
(findings 2–3).

## Repro

```
cd briev-e14a && cargo build --release
./target/release/brievc build probe/electronics/scale_4096.ebv --out /tmp/p
kicad-cli sch erc /tmp/p/scale_4096_1.kicad_sch --severity-error
```
