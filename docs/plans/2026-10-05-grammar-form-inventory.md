# Phase 0.6(d) — Mechanical grammar-form inventory

**2026-10-05.** First output of the promotion sweep (Phase 0.6d of
`2026-10-04-three-surfaces-functional.md`): the mechanical classification of
one-surface grammar forms into **core / surface-owned / licensed-stretch**,
with the gate-location evidence per row. Timestamped record — corrections
append.

## Method

1. **Cross-surface probe** — `scripts/grammar_probe.py`: canonical REAL
   programs per surface (never hand-rewritten), each copied under every
   active execution-surface extension (`.bv/.ebv/.abv/.rbv/.sbv`) and built
   with `brievc build`. The failure class per cell (ok / parse / type /
   capability / other-error) locates the gate: parse = extension-gated
   grammar; capability = backend/profile diagnostic; ok = core-legal.
2. **AST mapping** — the `TopLevel`/`Statement` variant inventory
   (`src/ast/top.rs`) crossed with the parser (one grammar — no parser
   branch keys on surface except the `.rbv` presentation pre-pass in
   `src/import_resolver.rs`).

## The matrix (2026-10-05, release build)

| program ↓ / surface → | `.bv` | `.ebv` | `.abv` | `.rbv` | `.sbv` |
|---|---|---|---|---|---|
| core (eor-demo.bv) | ok | capability¹ | capability² | parse³ | capability⁴ |
| core-list (error-handling.bv) | ok | capability¹ | capability² | parse³ | capability⁴ |
| electronics (usb_sensor.ebv) | **panic**⁵ | ok | capability¹ | **panic**⁵ | parse |
| electronics-min (led_blinker.ebv) | **panic**⁵ | ok | capability¹ | **panic**⁵ | parse |
| gpu (reduce.abv) | **mislowered**⁶ | parse⁸ | ok | parse³ | capability⁷ |
| gpu-gemm (gemm_small.abv) | **mislowered**⁶ | parse⁸ | ok | parse³ | capability⁷ |
| render (counter.rbv) | parse⁹ | parse⁹ | parse⁹ | ok | parse⁹ |
| render-struct (rstruct-demo.rbv) | parse⁹ | parse⁹ | parse⁹ | ok | parse⁹ |
| silicon (sensor_die.sbv) | ok | ok | capability² | ok | ok |

1. `electronics does not support term/endprogram` — clean gate, why+fix.
2. runner-v1 / CIRCT surface gates — clean diagnostics.
3. true parse errors: the `.rbv` presentation layer (render HTML) is
   extension-gated at the resolver pre-pass (`src/import_resolver.rs`,
   `SourceKind::Rendered`), not in the shared parser.
4. `CIRCT normalizer: intrinsic 'Print#' is not supported` — gate ✓.
5. **PANIC** `src/backend/llvm/emit_expr.rs:2575` — "field access `.vbus`
   on non-struct type `UsbMicro` reached codegen" (rc=101). BUGS.md.
6. **Mislowering**: the GPU program parses on `.bv` (one grammar), the LLVM
   lane ignores `.abv`-owned modifiers, and the program reaches LLVM
   emission/clang with broken types — clang rejects, the compiler does not
   diagnose. Same root family as the undefined-symbol class (BUGS.md): gate
   membership ⇔ arm, per lane.
7. `CIRCT (.mlir hardware) does not support foreach loops / ranges` — clean
   gates, why+fix ✓.
8. parse-level rejection of GPU forms under `.ebv` (electronics pipeline).
9. parse rejection of render forms on every non-`.rbv` surface — the
   declared `.rbv`→`.bv` edge is one-way, as designed.

## Classification

| Form class | Home | Classification | Evidence |
|---|---|---|---|
| `node`/`txn [pre][post]`, `let`/`const`, `defn`, obj/cell/type, contracts, triggers, foreach/match/guarded, ops | everywhere | **core** | builds on `.bv`; clean capability gates elsewhere |
| `budget`, `unpop`, `shortcircuit`, top-level `when` laws, pin classes, quantity literals (`250mA`), KiCad output | `.ebv` | **surface-owned** (electronics analysis pipeline) | ok only on `.ebv`; parse/capability elsewhere |
| `scope<…>`, `shared`, shape modifiers (`tile`/`stage`), `Tensor<…>`, kernel builtins (`GetGlobalId#`…), subgroup ops | `.abv` | **surface-owned** (GPU backend surface) | ok only on `.abv`; the LLVM lane must REJECT the intrinsic members (see ⁶ defect) |
| `render`, HTML blocks, `b-text`/`b-trigger`, stylesheet/svg/fab | `.rbv` | **surface-owned** (presentation pre-pass + webstack backend) | parse-gated off `.rbv` via the resolver |
| `mem let`/`reg let`, state-array lowering policy, pin-class-as-type boundary pins | `.sbv` | **licensed-stretch** — NOT surface-owned syntax: pin classes are core-legal valueless marker types; silicon-ness = the CIRCT backend + disambiguation prefixes | `sensor_die.sbv` builds on FOUR surfaces; the `.sbv`-specific parts are backend semantics, not grammar |
| `asm` decls, `Asm#`, `$(Stage)`, `$let`/`$defn`, `cfg`, `syncgroup`, `fuzz`, protocol/codec, `###` | everywhere | **core** (compile-time/escape machinery; per-lane lowering gated by the intrinsic/backend gates) | intrinsic-coverage audit (2026-10-05) |

## Defects found by the probe (filed, not fixed in this commit)

- **P1 — electronics-under-`.bv`/`.rbv` panic** (`emit_expr.rs:2575`):
  component field access reaches codegen on a non-struct type. Must be a
  diagnostic (the typechecker should reject the field access generically —
  field access on a non-struct type needs no surface knowledge). BUGS.md.
- **P2 — GPU-under-`.bv` mislowering**: `.abv`-owned programs parse on
  `.bv` and reach clang with broken IR instead of a gate diagnostic. Fix
  path = the same gate-membership⇔arm rule as the undefined-symbol class
  (BUGS.md): a name the LLVM lane has no arm for is a normalizer error, not
  a plain-call fallthrough.

## Consequences

- The frozen invariant (SPEC §3.6) is confirmed mechanically: one grammar,
  surface-owned regions locatable to their gate, `.sbv` graded
  licensed-stretch (no exclusive grammar to freeze).
- `scripts/grammar_probe.py` joins `scripts/intrinsic_probe.py` as a
  permanent regression probe for the surface register.

## Correction 2026-10-05 (Phase 0.6b follow-up)

The classification row puts quantity literals (`250mA`) in the
`.ebv`-owned column ("ok only on `.ebv`"). That was inferred, not probed
— no canonical probe program carries a bare quantity literal. Direct
probe closes it:

- `let x: Float = 250mA;` → `.bv` **builds**, prints `0.25` (correct SI).
- `brievc check` on `let x: Float = 10ms;` → **OK on all five** surfaces
  (`.bv/.ebv/.abv/.rbv/.sbv`).

So the quantity-literal FORM is **core**, not electronics-owned: the
dimension enum + suffix machinery live in the shared frontend
(`src/parser/quantity.rs`), and the magnitude now scales to SI at every
consumer (BUGS.md 2026-10-05). Electrical dimensions remain available;
`QuantityDim::Time` was added 2026-10-05 (`c1f1fc9f`). What stays
`.ebv`-owned is the electrical *analysis pipeline* that consumes spec
quantities — not the literal syntax.

**Undo:** revert this correction only if the literal form is re-gated off
core surfaces; the parser/quantity.rs machinery has no surface branch.
