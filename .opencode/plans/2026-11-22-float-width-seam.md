# Float width seam (issues #153/#154/#155) — binary64 campaign

**Status: DONE 2026-11-22.** Worked in `/scratch/briev-builds/wave5` (main),
commits land in logical stages (typechecker → casting graph → LLVM landing →
stdlib lanes → interpreter → fixtures).

## The defect (one seam, three symptoms)

A `Float64` value had to travel through the compiler as binary32 until its
terminal use, and each hop lost information that no later widening can recover:

1. **Literals at the wrong width** (#153): `let a: Float64 = 0.1;
   (a * 10000000000.0) as Int` printed `1000000014`. The literal was encoded
   as `f32` then widened; the decimal→binary32 rounding error survived.
2. **The String door at the wrong width** (#154): `"0.1" as Float64` parsed
   through `str_to_float` (binary32 strtod), then widened — same error.
3. **Payloads encoded at the wrong width** (#155): `FloatVal(1.25)` (an enum
   whose payload declares `Float64`) stored an `f32` bit pattern the JSON
   encoder decoded as `5.27...`.

The common root: **width was never a property of the value's identity**. The
typechecker treated `Float64` and `Float` as different types (mismatch
errors), the casting graph resolved `Float64` to `("Float", "")` and handed
every cast to the category default (the binary32 door), and the LLVM emitter
encoded float literals from the AST's `f32`-biased default regardless of the
declared type.

## The model

**The Float family is one numeric domain; variants are representations
(widths).** `Float`, `Float32`, `Float64`, `Double` are all `Cast.Float`; a
type whose own universe row already carries the category membership but is
NOT the category contributes its **name as the variant** — exactly as
`String<C_String>` carries `C_String`. This is representation metadata, not
a new protocol, and it reuses the existing variant machinery.

Rules:

1. **Typechecker** (`src/typechecker/mod.rs`, `float_variant_admissible`): a
   Float-family variant is admissible wherever the other is. The declared
   width is not weakened — codegen lands it. Applied at term values,
   endprogram payloads, call arguments, let annotations, enum payloads,
   dyn/member payload sites.
2. **Casting graph** (`src/casting/graph.rs`):
   - `type_to_protocol` resolves `Float64` → `(Float, "Float64")` via
     `category_variant` (width-bearing names are variants, bare name is not).
   - **Variant-scoped lanes** (`variant_lanes`, `set_variant_lane`,
     `seed_variant_lanes`): an explicitly registered lane for an exact
     endpoint pair beats any BFS route. Seeded: `String → Float/Float64` =
     `str_to_float64`, `String → Float/Double` = `str_to_float64`,
     `Float/Float64 → String` = `float64_to_str`, same for `Double`.
   - **Crossings to non-Float categories use the category semantic lane**
     (`FloatToInt` = fptosi, etc.), never a `Data`-hub detour: `Float64 as Int`
     is a conversion, not a bit reinterpretation. `Data` is a universal
     storage hub and is excluded from content-departure counting in the
     crossing validator (`src/typechecker/mod.rs`,
     `validate_cast_protocol_crossing`), so `Float64 → Data → Int` (a
     documented bits-view read) is legal without manual chaining.
3. **LLVM landing** (`src/backend/llvm/`):
   - `emit_float_literal_at` + `FunctionContext::pending_float_width`: a float
     literal whose declared width is known (let annotation, enum payload,
     call argument) is encoded at that width in one step — binary64 literals
     never round-trip through `f32`.
   - Call-boundary adaption: a float argument is extended/truncated to the
     declared parameter width before the call (a `Float` literal into a
     `Float64` parameter used to pass a 4-byte value in a 8-byte slot).
   - Term-return adaption at both return sites (`emit_stmt.rs`): the declared
     return width wins; `fpext`/`fptrunc` lands it.
   - Cast-operand fallback: `bitcast` is only for same-width class changes.
     Float width deltas use `fpext`/`fptrunc`; integer width deltas use
     `sext`/`trunc` (`int_width` helper in `src/backend/llvm/mod.rs`).
4. **Stdlib** (`lib/std/cast_lanes.bv`, `lib/std/float_fmt.bv`,
   `lib/std/cast_portable.bv`): binary64 twins of the parse/format lanes —
   `str_to_float64` (strtod, stops at trailing garbage), `float64_to_str`
   (`%g`, precision 6, trailing zeros trimmed), `pf_make64`/`pow10d`/
   `pow10d_small` (the formatter's power-of-ten ladder, all at binary64).
   The portable twins carry the same symbols plus saturation guards
   (|d| > 2100 → zero/infinity, wasm-safe).
5. **Reference interpreter** (`src/interpreter/eval.rs`): the `Float`-target
   cast now handles `Value::Bits` sources with `parse_float_prefix` (the
   strtod contract in Rust, at f64 — the interpreter's `Float` atom is
   already binary64). Before this, `"1.25" as Float64` stayed a String value
   in the reference engine: the interpreter had no String→Float door at all.
   Known remaining gap (BUGS.md, 2026-11-22): the interpreter has no
   cast-lane prelude, so `Float as String` renders `1.25000` where codegen
   renders `1.25` — pre-existing, binary32-reproducible, unrelated to width.

## Invariants (each pinned by a test)

| Invariant | Test |
|---|---|
| `String as Float64` parses at binary64 (`"0.1" × 1e10 → 1e9`) | `tests/tier1/float64_precision.bv` (native golden), `tests/tier1/float64_precision_interp.bv` (interpreter `test { }` blocks) |
| A `Float64`-declared literal keeps its decimal value | same pair of fixtures (`binary64_literal_keeps_its_decimal`) |
| A defn declared `-> Float64` returns at that width | same fixtures (`wide()` defn) |
| `Float64 as String` → `%g` precision 6 (`1.25`), trailing zeros trimmed | native fixture (golden line 3: `1.25`) |
| Round trip `Float64 → String → Float64` preserves the value | interp fixture (`exact_values_survive_the_round_trip`) |
| Parse stops at trailing garbage (`"2.5xyz" → 2.5`) | interp fixture (`parsing_stops_at_trailing_garbage`) |
| Every graph ExtCall lane symbol has a twin in the portable twin | `tests/cast_lane_symbols.rs` (`cast_lane_symbols_have_portable_twins`) |
| `Float64 as Int` is fptosi, not a bit reinterpretation | `mixed_integer_operands`, `tuple_values`, `wasm32_parity` (existing) |
| `Data` hub does not force manual cast chains | `tests/tier1/checkfree` corpus + `contract_gate_parity` |
| IR is deterministic across processes | `benchmarks/ll_determinism_check.sh --quick` (run, ALL DETERMINISTIC) |
| Interpreter String→Float door (this session's interpreter fix) | interp fixture parse tests |

## Verification

- `scripts/gate.sh full`: **4593 passed, 0 failed, 11 skipped** (after all
  edits incl. the interpreter fix).
- `benchmarks/ll_determinism_check.sh --quick`: ALL DETERMINISTIC.
- Probe matrix (native, release-equivalent debug binary):
  `f153c → 1000000000`, `f153 → 1.5`, `f153b → 0.1|2.5`, `s64 → 0.1`,
  `j155 → 1000000000`, `f125 → 1.25|1.25`, `f64f → 0.1`, `f64d → 2.5|2`,
  `f64c → 2.5|2.5`, `f64e → 2.5`, `f64b → 2.5`.
- Targeted suites: `casting`, `typechecker::tests`, `tuple_values`,
  `mixed_integer_operands`, `wasm32_parity`, `saltpetre_conformance`,
  `contract_gate_parity`, `cast_lane_symbols`, `rbv_list_each_wasm`,
  `rbv_router --lib import_resolver` — all green.
- `lib/saltpetre` is **no longer exempt** in `config/contracts.dbvl`: the
  invalid IR it tripped on was a real compiler bug (illegal width bitcasts
  + the missing String door), not a proof-gate failure. Exemptions remain
  only for `tests/saltpetre, tests/fixtures/load`.
- Praetor on changed dirs (`src/casting`, `src/typechecker`,
  `src/interpreter`, `src/backend/llvm`): no new diagnostics from this work
  beyond pre-existing baseline entries (the `type_to_protocol` length entry
  predates the seam; `set_variant_lane` and `category_variant` are clean).

## How to undo

Revert the staged commits in order (fixtures → interpreter → stdlib →
LLVM → casting graph → typechecker). The model has no runtime cost: the
variant-lane lookup is one extra map probe on a path that already did one.
