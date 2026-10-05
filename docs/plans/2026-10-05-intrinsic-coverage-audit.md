# Phase 0.3 — `Asm#` two-lane audit + intrinsic coverage matrix

**2026-10-05.** Deliverable of Phase 0.3 of the three-surfaces umbrella
(`2026-10-04-three-surfaces-functional.md`): *"diff the intrinsic registry +
`config/asm-lowering.dbvl` against PTX sm_80/86/90/100 + SPIR-V coverage;
lane asymmetries = filed gaps, fixed in Phase 2."* This record also carries
the intrinsic-coverage matrix the audit mechanically produced, and the three
defect classes it found (two fixed same-day, two filed).

Timestamped record — never retroactively edited; corrections append.

## Method

Ground truth is **source tables + compile probes**, never string-diff alone
(the LLVM dispatch strips `#` and arithmetic lowers as operators — a naive
diff over-reports):

1. **Registry** — `src/intrinsic_signatures.rs` (131 intrinsic arms).
2. **Gate sets** — each backend's `build_supported_ops` +
   `validate_intrinsics` call (`llvm/normalizer.rs` incl. `STANDARD_OPS`,
   `spirv/normalizer.rs`; `webstack`/`circt` have their own; **PTX has no
   normalizer gate**).
3. **Dispatch arms** — quoted intrinsic names per backend source tree
   (`src/backend/{llvm,ptx,spirv}/*.rs`), intersected with the registry.
4. **Probes** — `scripts/intrinsic_probe.py`: one minimal program per
   registry intrinsic × `{.bv, .abv}`, compiled with `target/release/brievc`.
   Build success = the lane's full chain (typecheck → gate → lowering →
   clang/ld or SPIR-V emission) accepted the shape; failure is classified.
   Re-run: `python3 scripts/intrinsic_probe.py`
   (`BRIEV_PROBE_WORK` overrides the scratch dir; needs a release build).

## `Asm#` two-lane verdict

| Lane | Asm# coverage | Evidence |
|------|---------------|----------|
| CPU / LLVM | **works** — abstract ops via `config/asm-lowering.dbvl`, raw mode bypasses the table | `emit_asm` at `src/backend/llvm/intrinsics.rs:1428`; probes `Asm#` .bv = OK |
| PTX (sm_80/86/90/100) | **ZERO coverage — filed gap (Phase 2)** | no `Asm#` match anywhere under `src/backend/ptx/`; no PTX sibling of `asm-lowering.dbvl` |
| SPIR-V | **rejected by gate — filed gap (Phase 2)** | no `Asm#` in `src/backend/spirv/`; `.abv` device-body Asm# → `intrinsic 'Asm#' is not supported by this backend` |

`asm-lowering.dbvl` rows: 2 ops (`Prefetch`, `Rdtsc`), targets
`x86_64/aarch64/riscv64/wasm32` only — no PTX templates, no sm-tier rows, no
SPIR-V/`Op` rows. **Lane asymmetry = the table is CPU-only by construction**
(documented in the table header: "The backend picks the field whose target
prefix matches the triple's first component"; a GPU triple has no match →
capability error). Stdlib wrappers: `lib/std/asm.bv` (`prefetch`, `rdtsc`).

**Asm# forms:** `Asm#("Prefetch", addr)` (abstract, table-lowered),
`Asm#("raw", template, ops…)` (raw inline asm, author-owned clobbers).
Emit error for an unlowered abstract op:
`Asm#: abstract asm op '{mode}' has no '{family}' lowering - add a row to
config/asm-lowering.dbvl (known ops: {known})`
(`src/backend/llvm/intrinsics.rs:1507`).

## Coverage matrix (registry ∩ lane surfaces)

| Surface | Gate | Quoted dispatch | Probe OK |
|---------|------|-----------------|----------|
| LLVM / `.bv` | 89/131 | 79/131 | **49/131** |
| PTX | *(no gate — gap)* | 19/131 | *(shares the `.abv` SPIR-V gate in the real pipeline)* |
| SPIR-V / `.abv` | 28 → **37/131** (synced, below) | 37/131 | 5/131 (+18 runner-v1 artifacts, 6 honest shape diagnostics, 91 intended CPU-only rejects) |
| Interpreter | — | 110/131 | *(default arm = `RuntimeError::UnsupportedIntrinsic`, a diagnostic)* |

Probe classification after the fixes below (`scripts/intrinsic_probe.py`,
release build, 131 names × 2 surfaces):

- **`.bv` — 49 OK, 41 clean gate rejects, 2 clean typecheck rejects,
  4 undefined-symbol failures, 34 arity panics (filed below).**
- **`.abv` — 5 OK; 91 gate rejects (the intended CPU-only surface);
  18 `runner-v1 host-surface` + 5 `untyped-let` = harness artifacts, not
  compiler defects** (host nodes accept only Assign/Term/EndProgram — the
  device route is `a[i] = NAME(...)` as in `examples/gpu/workid.abv`;
  untyped `let _v = …` needs an annotation); **6 SPIR-V `lowering`
  diagnostics are honest shape errors for malformed probes**
  (`Exp# needs an operand`, `AtomicAddAt# takes (buf, i, v)`,
  `builtins take a constant dimension 0..=2`).

Registry facts used by the audit: `parameters: vec![]` for most arms
(arity unreliable — root of the panic class below); return kinds:
Inferred 52 / Native 51 / void 15 / Never 1 (`Error#`); variadic:
`CallPtr#`, `TaskCall#`, `Asm#`, `SysCall#`; only pointer parameter:
`Free#`.

## Defects found — fixed same day (2026-10-05)

1. **Gate harvest bypass → LLVM emitter panics** (45 probe names,
   `args[0]` out-of-bounds). Root: no `Statement::Let` arm, statement-root-
   only walks. Fix: new `src/ast/visit.rs` (the ONE exhaustive visitor) +
   `src/backend/normalizer.rs` rebuilt on it (call AND method-call
   expansion; liveness-unconditional seeds only — the over-seed boundary
   is documented in-code and pinned by tests). BUGS.md entry + 5 tests
   (`backend::normalizer::tests`).
2. **Typechecker volatile arity panic** (`VolatileLoad#()` → `args[0]`).
   Fix: `args.first()` guard returning `TypeError::TypeMismatch` with the
   fix text; 2 tests.
3. **SPIR-V gate drift (28 vs 37 arms)** — worked fixtures
   (`atomic_inc.abv`, `workid.abv`) only passed via bypass 1, then failed
   closed. Fix: `build_supported_ops` synced to the emitter's real arm set
   with the in-code rule *gate membership ⇔ lowering arm + device fixture*;
   all 67 `examples/gpu/*.abv` gate-clean.

## Defects found — filed (Phase 0.6 / Phase 2)

- **LLVM registry-arity panic class — 34 names** (`Load#()`, `Print#()`,
  `PtrAdd#()`, `SimdAdd#()`, …). Registry claims no parameters ⇒ typechecker
  skips the arity check ⇒ emitter `args[i]` panics. Fix path = complete the
  registry `parameters` (Phase 0.6 promotion sweep); probe repro
  `scripts/intrinsic_probe.py`. BUGS.md entry.
- **Undefined-symbol class — `Concat#`, `GetGlobalSize#`, `Backtrace#`**:
  pass the LLVM gate, emit calls to undeclared/undefined symbols
  (`@Concat`, `@GetGlobalSize`, `briev_backtrace`) that clang/ld reject.
  Root: no lowering arm, plain-call fallthrough, gate checks membership
  only. Fix path = arm or de-list (same rule as the SPIR-V sync). BUGS.md.
- **PTX has no normalizer gate** — PTX builds ride the `.abv` SPIR-V gate;
  a `.bv`-shaped program cannot reach PTX, but the absence is an
  architecture asymmetry to close when the PTX lane grows independent
  entry (Phase 2).
- **Asm# GPU-lane coverage (PTX sm tiers + SPIR-V)** — the headline Phase
  0.3 asymmetry: extending `asm-lowering.dbvl` with `ptx:*`/`spirv:*`
  template fields (or an equivalent `Asm#` lowering table per lane) is
  Phase 2 work.
- **Dispatch-only op-member gap** — the gate reaches op bodies through
  method-call expansion at CALL SITES; an op member invoked only through
  mangled dispatch (no textual call site) stays outside the gate, relying
  on the emitter's own diagnostic (pre-existing behaviour, documented at
  `seed_toplevel` in `src/backend/normalizer.rs`). Closing it properly
  needs the liveness set in the gate (frontend-driven dispatch), not more
  blanket seeds.

## Consequences of this audit

- `src/ast/visit.rs` (new) — the exhaustive AST walker; contract: no `_`
  arms, new variants fail to compile.
- `src/backend/normalizer.rs` — rewritten harvest/seeding with the
  over-seed boundary documented; 5 behavioral tests.
- `src/typechecker/mod.rs` — volatile arity guard; 2 tests.
- `src/backend/spirv/normalizer.rs` — gate synced 28 → 37 with the
  membership⇔arm rule.
- `scripts/intrinsic_probe.py` (new) — the reusable matrix probe.
- Suite: **2863 green** (2856 baseline + 7 new).

## Correction 2026-10-05 (later same day) — filed LLVM defects now CLOSED

The two filed LLVM defect classes are fixed on `main`; `scripts/intrinsic_probe.py`
re-run (release build, 131 names × 2 surfaces) confirms:

- **Registry-arity panic class — CLOSED** (`2dcdf464`): `declared_min_arity`
  (src/intrinsic_signatures.rs) supplies the minimum arity for
  parameterless intrinsics, checked in the typechecker BEFORE any `args[i]`
  indexing. Probe: **0 `.bv` panics**; `Load#()`, `Print#()`, `PtrAdd#()`,
  `SimdAdd#()` all report a clean `expected at least N arguments` diagnostic.
  (The fix path taken is a min-arity table, not filling `parameters` — the
  registry keeps `parameters: vec![]` for polymorphic inference by design;
  the table is the parallel source of truth for minimum arity.)
- **Undefined-symbol class — CLOSED** (`6571c840`): `Concat#` de-listed from
  the registry; `GetGlobalSize#`/`Backtrace#` now fail at the LLVM gate with
  `intrinsic '…' is not supported by this backend`. Probe: **0 undefined-symbol
  / clang-fail cells**.

Current `.bv` probe classification: **49 ok / 44 clean gate rejects / 36
clean arity typecheck rejects** (was 49/41/2 + 4 undefined + 34 panics).

Still open (Phase 2, architecture — not defects): PTX has no independent
normalizer gate; `Asm#` has no PTX/SPIR-V lowering table; the dispatch-only
op-member gap (pre-existing, documented).
