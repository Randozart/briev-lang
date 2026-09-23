# Cross-dialect interop — the whole computing stack through Briev

**2026-09-21.** QUEUED — end of queue (after Front D retirement).
Discussion-derived design; nothing here is built. Prerequisite for wave 1:
import provenance (the resolver currently only tries `.bv` candidates —
`src/import_resolver.rs:800-866` — and no per-module origin survives the
splice; the "tries .ebv" error text at `:857` is stale).

## Vision

A native way to interact across the entire computing stack through Briev:
`.sbv` ports into `.ebv` electronics in a predictable, compiler-reasonable
way; `.rbv` views into `.bv` programs; `.rbv` into `.sbv` and `.rbv` into
`.abv`. Import a file from another dialect and it works.

## The interface table (why this is reason-able)

Every dialect already has a typed, DIRECTED interface concept — the
bridge projects between them:

| Dialect | Unit | Connection point | Direction | Runtime? |
|---|---|---|---|---|
| `.ebv` | component | pin | source/sink/bidir | **no** — netlist + KiCad, proven-not-executed |
| `.sbv` | module | port (address-sorted) | in/out/inout | no — synthesizes (CIRCT) |
| `.abv` | kernel | buffer | producer/consumer | GPU only |
| `.bv` | node | state field | read/write | the reactor (universal conductor) |
| `.rbv` | view | binding | display / write-contract | browser/wasm |

## The spine model (user-confirmed)

`.bv` is the composition spine; the runtime topology is

```
.rbv → .bv → .abv
           → .sbv → .ebv
```

**Adjacent pairs are DECLARED; distant pairs are DERIVED.** The compiler
derives non-adjacent bridges by composing adjacent edges and REFUSES
direct edges that skip a runtime — with one exception, below. Refusal is
a feature: it makes the runtime topology of every program explicit.

**The artificial driver (user requirement):** a distant pair
(`.rbv`→`.sbv`) is served by a compiler-SYNTHESIZED bridge node — the
spine segment the author could have written. Constraints:

1. It is a REAL node: in the reactor, schedulable, contract-capable.
2. It is DISCLOSED: diagnostics + artifacts name it
   (`// synthesized: bridge rbv→sbv for 'fpga'`).
3. It is DERIVED, never authored — built from the adjacent edge tables.
   A hand-written bridge node remains available; when the author writes
   the host segment themselves, the compiler stands down (the compiler's
   optimum must be expressible in the language).

## Syntax: bare import, binding carries intent (user-confirmed)

No new keyword (Rule 2 spirit; extension-as-intent is already doctrine —
`.abv` injects `!> accel: try_all`). The §7 import grammar suffices:

```briev
import "led.ebv";                    // graft: declarations enter the program
import board = "fpga.sbv";           // alias: interface reachable as board.*
import {core} = "fpga.sbv";          // selective (§7)
```

Imported interface points surface as ordinary host things: `.sbv` ports →
MMIO-backed state under the alias (`board.core.leds = x;` compiles to the
volatile store — `address_resolver` board packs already do half of this);
`.abv` kernels graft as accel nodes; `.ebv` contributes nets, constraints,
projections — nothing runs. A `// bridge: <relation>` annotation is the
reserved escape hatch if a default relation is ever wrong — NOT a second
import form (two forms for one fact is the `Mmio#` mistake).

## The three mechanism tiers

- **Owned by the compiler (eternal)**: provenance tagging; the
  interface-point algebra (typed directed points; direction preservation —
  an output port never projects to an input pin); obligation transport
  (contracts cross the bridge, conjunction-checked); capability
  intersection (`src/backend/capabilities.rs` validates each imported
  module against intersection(host surface, source surface));
  transitive derivation.
- **Declared per pair (temporal)**: what a port becomes — port→pin map,
  MMIO address→designator, buffer↔state layout. Starts compiler-side
  (type-graph knowledge, like the casting graph); migrates to declared
  tables if it grows. Honest tension, revisited per pair.
- **Forbidden, named loudly**: execution semantics across non-executing
  dialects. A `.bv` node cannot "call" a net; `.ebv` participates by
  constraint and projection only — its charter, not a limitation.

## Per-pair status

| Pair | Status |
|---|---|
| `.rbv`→`.bv` | exists informally (view bindings + write-contract routing) — formalize as the FIRST declared edge |
| `.bv`↔`.abv` | exists (accel mixed lane, kernel↔state, `.abv` GPU-only charter) — declare it |
| `.bv`↔`.sbv` | half-exists (MMIO `@addr` on the shared AST, board packs) — formalize port↔state-region |
| `.sbv`→`.ebv` | NEW — the structural projection: silicon module → electronics COMPONENT; ports → pins (direction map); port contracts → electrical constraints (`reference`/`tolerance` clauses exist in `.ebv` Volt); emits hierarchical KiCad symbol/sheet. Purest pair: both static, neither executes |
| `.ebv`→`.sbv` | reverse direction, v2 (pin constraints → port-level electrical rules) |
| `.rbv`→`.sbv`, `.rbv`→`.abv` | DERIVED (spine composition) or the synthesized bridge node |

## Waves

- **Wave 1 — provenance + first edge**: resolver classifies resolved
  imports by dialect (`classify()` exists in conformance.rs), tags spliced
  items with per-module SourceKind (the missing data structure);
  per-module extension-keyed semantics (accel default, profiles, prelude
  filtering) apply per imported module, not root-only; fix the stale
  `.ebv` error text. **Name-collision rule: a `.bv` and an `.abv` both
  exporting `kernel` is an ERROR or forces aliasing — silent last-wins
  across dialects is a correctness trap.** Formalize `.rbv`→`.bv`.
- **Wave 2 — the static pair + runtime pairs**: `.sbv`→`.ebv` projection
  (the headline case); declare `.bv`↔`.abv` and `.bv`↔`.sbv`.
- **Wave 3 — derivation**: transitive bridges + synthesized bridge nodes;
  diagnostics name the first failing hop with the fix.

## Queue position

END of queue (user decision 2026-09-21), after Front D retirement.
Running order: examples corpus (see `2026-09-21-syntax-examples-corpus.md`)
→ reflection conditions (`2026-09-21-comptime-fold-expansion.md` phase 2)
→ `$txn` topology templates (`2026-09-20-metaprogrammed-composites.md`)
→ Front D retirement → this plan, waves 1-3.
