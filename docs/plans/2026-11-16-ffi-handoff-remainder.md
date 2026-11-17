# FFI contract — session handoff and remaining work

**2026-11-16.** Status: **handoff.** Written so a fresh session can resume without
re-deriving anything: state, decisions already settled, and the exact next steps
with their touch points and gates.

**Parents (in order of authority):**
`2026-11-16-ffi-contract-and-metaprogramming.md` (design, D1–D6),
`2026-11-16-ffi-remaining-grind.md` (the 15-step grind),
`2026-11-16-f1-verification-and-g4-redesign.md` (the F1/G4 corrections),
`2026-11-16-s3-protocol-generation.md` (S3 detail).

---

## 1. State at handoff

`main` = `7234dee82`, synced, tree clean, CI green.

### Landed this session

| What | Commit |
|---|---|
| Named-map parser fix — a map rejected both a trailing `;` before `}` **and** required one after a nested value | `8dc3ef6ea` |
| C adapter's full value-class set (`String` zero-copy out / copying in, scalars with C layouts, `Data`) | `aacb6225a` |
| `render_protocols` + `ValueClass` model + `boundary`/`protocol`/`base` naming fields | `51e5ae1ef` |
| `brievc render <lang> [--check]` + check-mode tests + first generated file | `57caeef84` |
| `c.bv` imports the generated protocols instead of declaring them | `2d9cb0fb0` |
| Determinism corpus widened to the shapes F1 shipped through | `1a5462f16` |
| **G4′ parity gate passes** — generated vs hand-written emit byte-identical IR | `88ab39526` |
| `glue/c/types.bv` created (re-exports generated) + "every `types_module` exists" test | `f3e1b5876` |
| **H1** `HirProtocol`/`HirCastEdge`/`HirProtocolOp` + `HirProgram.protocols` + lowering | `7234dee82` |

### Decisions already settled — do not re-litigate

1. **Ownership is per function** (D1). The descriptor supplies conventions and
   retain/release ops only; `values:` never says who frees a given call.
2. **ABI cores in `config/abi/`**, adapters in `lib/glue/` (D2). Only the C adapter
   names a core; managed adapters carry host-FFI types, not a machine ABI.
3. **Descriptor is versioned, unknown keys are errors** (D3). `GLUE_SCHEMA_VERSION`
   in `src/glue/config.rs`.
4. **The removed-surface gate precedes the generator** (D4) — still open, see §4.
5. **Descriptor declares facts, never strategies** (D5). A fast path is derived.
6. **Generated items trace to a descriptor key path**, not a line (D6 amended —
   `.dbv` values carry no spans, `dbriev/v2.rs:88`).
7. **`values:` is flat** (`to_op`/`from_op`/`from_copy`/`len_op`). The `.dbv` parser
   nests two levels; the third would need a shared-parser change.
8. **The generator is Rust and contains no language knowledge.** "Data, not Rust"
   forbids `if lang == …`, not the implementation language.
9. **A parity claim must pin program *structure*.** An inline declaration and an
   imported one are different programs to the emitter (`collect_strings` interns in
   item order). Never demand byte-identity across that difference.
10. **F1's per-process instability was already fixed** (`BUGS.md:112`, M6). What was
    measured mid-session was import-graph-driven and deterministic — do not reopen.

---

## 2. Remaining work

### H2 — `HirBoundary` (next)

**Touch points (verified):** `HirForeign` at `src/hir/program.rs:229` —
`{ name, link_symbol, params, ret }`; lowered at `src/hir/lower.rs:482`
(`lower_foreign_item`), which today reads `fb.briev_name`, `fb.foreign_name`,
`fb.inputs`, `fb.success_output`.

**Work:** carry the contract the AST already has and HIR drops:

```rust
pub enum HirFromSpec { Literal(String), Registry(String), Protocol(String), Linked(String) }
pub struct HirBoundary {
    pub from: HirFromSpec,          // where the symbol comes from
    pub buffer_mode: Option<String>,
    pub precondition: Option<String>,   // rendered, or a NodeId into a contract table
    pub postcondition: Option<String>,
    pub error_type: Option<String>,
    pub is_optional: bool,
    pub is_fire_forget: bool,
    pub is_delivery: bool,
    pub variadic: bool,
}
```

Attach as `HirForeign.boundary: HirBoundary` (or flatten the fields if the struct
feels heavy — either is fine, but **do not** add a field no pass consumes; H3 is the
consumer that justifies them).

**Gate:** a test per field group asserts the lowered value; `hir::verify` keeps the
AST/HIR drift assertions green (`BRIEV_HIR_EMIT=0` oracle); `cargo test --lib` green.

### H3 — the audit must stop certifying the loss

**Touch points:** `unsupported_counts` at `src/hir/stats.rs:31` walks only
`program.callables[].body.statements`; `UnsupportedCounts` is the struct to extend.

**Work:** add boundary counters — a `proto` whose cast lanes cannot be represented,
a `frgn` whose `from` cannot be represented — and make the **BUGS.md fixture**
(`generated protocol + frgn from #Link<libc> + a boundary cast`) report ≥1 missing
fact while the contract is unrepresented.

**Gate (write it first):** today that fixture prints
`HIR_AUDIT ok … unsupported=0`. The test asserts it does **not** say ok, and flips
to "clean because the facts are modelled" after H2. Precedent: the HIR-fidelity work
(`Within`/`Exists`/`FormattingAnnotation`); `DerivationBlock` stays an explicit gap.

### H4 — destination model reads the boundary

**Work:** `src/hir/destination_model.rs::boundary_conversion` takes `HirBoundary`
instead of reconstructing from `HirForeign` + `hir_type_protocols`. Classifications
for `CStr` must not change.

**Gate:** `cargo test --lib destination_model` green with identical classifications
(those tests are the oracle).

### S5 — stage guard

**Work:** in `src/macros/eval.rs`, the `Insert$` path (`:1079`): refuse a synthesised
`ProtocolDef`/`TypeDef`/`ForeignBinding` when `stage > Resolved`, naming the correct
stage. Body-bearing `defn`s stay allowed (they face `verify_bodies`, `compile.rs:838`).

**Gate:** compile-fail fixtures both ways; the generated protocols (inserted at
`Resolved`) still build.

### PY — Python as pure data (the closure test)

**Work:** add `values:`/`discipline:` to `lib/glue/python/glue.dbv`: `PyObject*` as
`opaque_handle` with `retain_op`/`release_op`, `String` as an owned conversion (no
zero-copy possible), errors as an exception channel.

**Gate:** the Python bridge builds and a minimal call round-trips; `tests/glue_test`
and `tests/glue_bridge_tests` unchanged; **C path re-run through G4′ byte-identical**.
**If any Rust change is needed, the schema is wrong** — that is the test. Likely
pressure point: GIL handoff / pinning. If `discipline:` cannot express it, grow that
section — by data, not Rust.

### R1 — retire the hand-written axioms

G3 left `lib/glue/c/protocols.bv` generated **and** committed; the hand-written half
is already gone from `c.bv`. R1's remaining work is the **doc matrix** from the parent
plan §7: `glue-ffi.md`, `glue-as-abi-generator.md`, `macro-system.md`,
`ir-layers-and-naming.md`, `protocol-model.md`, `casting-protocol.md`,
`MASTER-SYNTAX-REFERENCE.md`, `docs/plans/INDEX.md`.

### D1 / D2 — removed-surface gate, then sweep

**D1:** `src/vocab.rs` already exposes `is_removed_keyword`/`removed_keywords`. Add
**one** diagnostic path for removed surface at the decl/token dispatch (the `#!exit`
pattern, `bddf015ea`, generalised), plus a live-file check (not `docs/plans`, not
`archive/`) in CI. **Gate:** a compile-fail fixture per form; the live-file check
green on today's tree.

**D2:** sweep `#out` (fails today with `expected LParen, found 'print_int'`),
`#frgn`, `frgn` without `from`, top-level `term` — remove from live users, leave
historical records alone.

---

## 3. Verification recipe (unchanged, for whoever resumes)

- Per step: touched-module tests + `bash scripts/gate.sh quick` + Praetor on changed
  files (`--target <dir>`, never a file).
- `H*` steps: full `cargo test --lib` (shared analysis).
- Any step touching emission: `bash benchmarks/ll_determinism_check.sh --quick`
  (now five programs) **and**, for glue, `cargo test --test glue_gen_parity`.
- Before a push: `git fetch` first; land with `git push origin HEAD:main` when `main`
  is held by another worktree, then fast-forward.

---

## 4. Deferred / blocked (do not start without the named prerequisite)

| Item | Blocker |
|---|---|
| `D1` removed-surface **gate** | none — it is queued, and D4 says it precedes any further generation |
| Follow-up wave 0 (`F2` Data→Float bitcast, `F7` Kani cast-graph proofs) | none — small, independent; `F1` is now "verify + gate", already done |
| Follow-up `F3` corpus ERR floor (17 host IR regressions, groups A–E) | slices, each with its own reproduce |
| Follow-up `F6` HIR port queue (`destination_ownership` first) | **after** H1/H2 — porting a family to an HIR that lacks boundary facts relocates the blindness |
| Follow-up `F8` export ABI v2 | needs the consumer's sign-off on the ABI shape (like D1's ownership decision) |
| Follow-up `F5` toolchain matrix in CI | needs a CI host with ≥2 clangs; the baseline table + flip rule *can* be written now |
| `F9` issues #5/#7/#8 | external reproducers from `test-project-otto` / `transition_validate` |
| `F11` `h5-phase-c4` branch | owner must rebase (measured: 28 hunks / 1359 lines in `emit_expr.rs`) |
| Python GIL/pinning, if `PY` cannot express it | decide the `discipline:` field shape from the measurement, not in advance |

---

## 5. First 30 minutes for a fresh session

1. `git log --oneline -12` — confirm the landed set above.
2. `cargo test --test glue_gen_parity` — the load-bearing gate; if it passes, the
   foundation is intact.
3. Read `HirForeign` (`src/hir/program.rs:229`) and `lower_foreign_item`
   (`src/hir/lower.rs:482`) — H2 starts there.
4. Start H2 with the test: assert a `frgn … from #Link<libc>` lowers with its
   `from` spec, so the first red is the missing fact rather than a compile error.
