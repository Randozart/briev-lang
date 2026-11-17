# FFI contract — remaining work, one grind

**2026-11-16.** Status: **execution plan for everything still open** in the FFI
contract workstream, sequenced so it can be run as one continuous session.

**Parents:** `2026-11-16-ffi-contract-and-metaprogramming.md` (design, decisions
D1–D6, phases S0–S7) and `2026-11-16-s3-protocol-generation.md` (the S3 detail
this plan carries forward). **Doctrine:** `glue-as-abi-generator.md`,
`macro-system.md`, `derivation-not-recognition.md`.

---

## 0. How to run this

- **One step per commit**, each with its gate green before the next starts.
- **Never leave a step half-done across a commit.** If a gate fails twice, stop
  the step, revert *that step's* edits by hand (never `git checkout --`), and
  record the blocker in §6. The tree stays green between steps at all times.
- **Every commit** carries: `scripts/gate.sh quick` pass, Praetor clean on
  touched files (new code only — the baseline is pre-existing), and the doc edit
  listed in that step.
- Full gate (`bash scripts/gate.sh full`) before any push that touches shared
  analysis (S4, S5).

---

## 1. What is already done

| Item | Commit / state |
|---|---|
| S0 spike — generated protocol drives zero-copy marshalling; HIR audit reports `unsupported=0` on a boundary program | measured, in plan §8a |
| S1 — versioned, closed descriptor schema (D3) | `982e86f5f` |
| S2 — six ABI cores + loader + validated `abi:` | `ac934968e` |
| S3.0 — named-map characterisation; all structural hypotheses refuted | `500e1b951` |
| C adapter declares `String` as `borrowed_bytes`/`zero_copy` with `to_op`/`from_op`/`from_copy` | `bd63baac5` |
| Thread lane (A1–A4, B1–B6) closed; CI green | `ecff87a11`… |
| `#!exit` killed with a diagnostic | `bddf015ea` |

---

## 2. Remaining work — master table

Sizes: **S** ≤ ½ day, **M** ≈ 1 day, **L** > 1 day.

| # | Id | Work | Size | Depends on |
|---|---|---|---|---|
| 1 | `P0` | Bisect the multi-entry `values` parse failure; fix the parser | S | — |
| 2 | `G1` | `src/glue/gen.rs` — pure `render_protocols` + provenance header + sorted output | S | P0 |
| 3 | `G2` | `brievc glue render <lang> [--check]` + wired into CI on first generated file | S | G1 |
| 4 | `G3` | Split `lib/glue/c.bv` into ops + `glue/c/protocols.bv` (generated) | S | G2 |
| 5 | `G4` | **IR parity gate** — hand-written vs generated, byte-identical over a both-directions corpus | M | G3 |

> **G4 landed as G4′** (`88ab39526`, 2026-11-16): the original wording compared
> hand-written vs generated *in place*; the gate that actually passed pins
> program **structure** — both variants import a protocol file at the same
> position and depth, differing only in how those declarations were written
> (inline vs imported are different programs to the emitter). Superseded by
> `2026-11-16-f1-verification-and-g4-redesign.md`; the rest of this plan is
> carried forward by `2026-11-16-ffi-handoff-remainder.md`.
| 6 | `G5` | `types_module` — generate `glue/c/types.bv` from `values:` (or delete the key) | S | G3 |
| 7 | `H1` | HIR: `HirProtocol` + lowering + verify | M | — |
| 8 | `H2` | HIR: `HirBoundary` (from/flags/contracts/discipline) on `HirForeign` | M | H1 |
| 9 | `H3` | HIR audit reports **missing** boundary facts (turns the BUGS entry green) | S | H2 |
| 10 | `H4` | Destination model consumes `HirBoundary` instead of reconstructing | M | H2 |
| 11 | `S5` | Stage guard: refuse `proto`/`TypeDef`/`frgn` synthesis after `Resolved` | S | H1 |
| 12 | `PY` | Python adapter as pure data — the closure test | L | G4, H2 |
| 13 | `R1` | Retire `c.bv`'s hand-written axioms; docs matrix | S | G4, PY |
| 14 | `D1` | Stale-surface gate: one removed-form diagnostic path + live-file check | S | — |
| 15 | `D2` | Sweep `#out`, `#frgn`, `frgn` without `from`, top-level `term` | M | D1 |

Steps `H*` (HIR) are independent of `G*` (generation) and `D*` (surface). They can
interleave to rest an expensive build, but **commit boundaries stay clean**.

---

## 3. The order, and why

1. **`P0` first.** Everything in `G*` reads a descriptor; an unexplained parse
   failure in the multi-entry form is a landmine under the whole phase. It is
   also small — a bisection with a test already in place.
2. **`G1`→`G4` next**, because they produce the first *provable* claim: the
   protocol surface is data. `G4` is the gate that makes the claim real, so it
   must not be deferred to the end.
3. **`H1`→`H3` after `G4`**, because until then generated protocols have no HIR
   representation to be checked against — and the audit's `unsupported=0` on
   boundary programs is the defect that lets wrong generated code look fine.
4. **`S5`** closes the complement: HIR can represent the contract (`H1`/`H2`) and
   the frontend refuses what it cannot check.
5. **`PY`** is the acceptance test for the whole design. Last, because it needs
   both the generator and the HIR surface to be worth running.
6. **`D*`** runs whenever a build would otherwise be idle; `D1` (the gate) is
   cheap and prevents new removed-form usage while the rest proceeds.

---

## 4. Per-step detail

### P0 — bisect the parser failure

The failing block is preserved verbatim in the S3 plan §2.1. Method: extend
`dbriev::v2::tests::characterise_named_map_shapes` with the six class names one
field at a time until `Expected named field` reproduces, then read the branch
that fires.

- **Likely outcome A** — a genuine parser bug: fix it in `parse_named_fields_map`
  (`src/dbriev/v2.rs:1219`) and keep every shape case green.
- **Likely outcome B** — a genuine syntax limit: record it in
  `docs/architecture/data-briev.md` and flatten the descriptor further.
- **Gate:** the failing shape becomes a *passing* case (A) or an *expected-error*
  case with the rule documented (B); all `.dbvl`-reading tests green.
- **Risk:** the parser is shared by `config_tuning`, `protocols`, `targets` — any
  change runs those suites in the same commit.

### G1 — pure renderer

`src/glue/gen.rs`: `render_protocols(&GlueTarget, &AbiCore) -> String`.

- Input: `values:` (class, zero_copy, `to_op`/`from_op`/`from_copy`) plus the
  existing `protocols:`/`conversions:` sections.
- Output: a `type <Boundary>: <Base><Proto> { };` plus the `proto … { axiom
  CastTo/CastFrom }` block, **keys sorted**, values sorted.
- Header: `// GENERATED from lib/glue/<lang>/glue.dbv (schema v1) — do not edit.
  Source keys: values.String, protocols.String, …` (D6 as amended: key path
  authoritative, line by lookup).
- Empty descriptor → empty output (not an error): a language with no protocols is
  legal.
- **Gate:** `glue::gen` tests — one class, several, none; a class whose `to_op`
  and `from_op` differ; `from_copy: true`; output identical when rendered twice
  in one process (determinism).

### G2 — CLI + drift check

`brievc glue render <lang> [--check]`, dispatched beside `export`/`bindings`/
`extension` (`src/main.rs:38`).

- `render` writes `lib/glue/<lang>/protocols.bv`.
- `--check` compares on-disk bytes with what would be rendered and exits 1 on
  mismatch, **without writing**.
- **Gate:** `--check` exits 0 on a fresh file, 1 on a stale one; a test creates
  both states in a temp dir. CI gains one `--check` invocation in the same commit
  that first commits a generated file (per the plan's D3 note: drift never gets a
  window).

### G3 — split `c.bv`

Move the `type CStr` / `proto C_String` half into `lib/glue/c/protocols.bv`;
`lib/glue/c.bv` keeps `import "glue/c/protocols.bv";` plus the operation defns
(`str_to_c`, `cstr_to_briev`, `cstring_concat`).

- **Gate:** `tests/str_to_c_no_memcpy` green; all six `import "glue/c.bv"` sites
  compile (`examples/glue-host/*`, `lib/compiler/soa_reorder.bv`).

### G4 — the parity gate (the one that matters)

`tests/glue_gen_parity.rs`: compile a corpus twice — once with the checked-in
hand-written protocols, once with the generated ones — and require
byte-identical `.ll`.

- Corpus must exercise **both directions**: Briev→C (`c_strlen` on a `CStr`) and
  C→Briev (a returned `CStr` consumed as a String), because an inverted
  `#Lh`/`#Rh` only shows in one.
- Mechanism: `brievc build <fixture> --llvm --out <dir>` twice with different
  `lib/glue/c.bv` states (or a `--glue-protocols=<path>` override if cleaner),
  then compare file bytes; report the first differing line.
- **Gate:** identical bytes. Any difference is either a missing descriptor fact or
  a generator bug — never a reason to keep the hand-written file.

### G5 — `types_module`

`glue/c/types.bv` is referenced but **does not exist**. Either render it from
`values:` (it is the same data: boundary type per class) or delete the key and any
code that reads it. Decide by grepping the readers first; deleting an unused key
is preferable to generating a file nothing consumes.

### H1 — `HirProtocol`

`src/hir/program.rs`: `HirProtocol { name, base, variants, casts, ops }`;
`lower_protocol_item` (`src/hir/lower.rs:459`) stops collapsing to a name list and
builds the node; `HirProgram.protocols: Vec<HirProtocol>` alongside the existing
`protocol_binding_fns` (kept for liveness until a proof consumer migrates).
`hir::verify` checks binding fns referenced by casts exist. `to_ast` returns
`None` for protocols (metadata gap, documented) or reconstructs them — decide by
whether round-trip tests need it.

- **Gate:** `hir::verify` green; a test asserts a `proto` with two axioms lowers to
  two `HirCastEdge`s with the right directions and binding names.

### H2 — `HirBoundary`

Extend `HirForeign` with `from: HirFromSpec`, `buffer_mode`, `precondition`,
`postcondition`, `error_type`/`error_fields`, `is_optional`, `is_fire_forget`,
`is_delivery`, `variadic`, and `classes`/`ownership` per position when the
descriptor declares them. Lowering at `src/hir/lower.rs:445`.

- **Gate:** a test per field group asserts the lowered value; `hir::verify` keeps
  the drift assertions green (`BRIEV_HIR_EMIT=0` oracle).

### H3 — the audit stops lying

`unsupported_counts` (`src/hir/stats.rs:31`) gains boundary counters: a `proto`
whose casts cannot be represented, a `frgn` whose `from` cannot be represented.
The BUGS entry's fixture (generated protocol + `#Link` frgn + boundary cast) must
**stop** being `unsupported=0`.

- **Gate:** the BUGS fixture reports ≥1 missing boundary fact *before* H1/H2 and 0
  after — i.e. the test is written first against today's behaviour, then flipped.
  That is the S4 acceptance test the plan promised, now expressible.

### H4 — destination model reads HIR

`src/hir/destination_model.rs`: `boundary_conversion` takes the `HirBoundary`
rather than reconstructing from `HirForeign` + `hir_type_protocols`. Existing
`CStr` classifications must not change.

- **Gate:** `cargo test --lib destination_model` green with identical
  classifications (the existing tests are the oracle).

### S5 — stage guard

In `src/macros/eval.rs` (`Insert$`, `:1079`): a synthesised item of kind
`ProtocolDef`/`TypeDef`/`ForeignBinding` is refused when `stage > Resolved`, with
what/why/fix naming the correct stage. Body-bearing `defn`s remain allowed (they
pass `verify_bodies`).

- **Gate:** compile-fail fixtures for both refusals; the S3-generated protocols
  (inserted at `Resolved`) still build.

### PY — Python as pure data

Add `values:` (and `discipline:`) to `lib/glue/python/glue.dbv` describing
CPython's model — `PyObject*` as `opaque_handle` with `retain_op`/`release_op`,
`String` as an owned conversion (no zero-copy possible), `errors` as an exception
channel. **If any Rust change is needed, the schema is wrong** — that is the test.

- **Gate:** the Python bridge builds and a minimal call round-trips;
  `tests/glue_test` and `tests/glue_bridge_tests` unchanged; the C path
  byte-identical (G4 re-run).
- **Honest risk:** GIL handoff and pinning may not be expressible yet. If so,
  `discipline:` grows a field and the plan records it — resolved by data, not by
  Rust.

### R1 — retire the hand-written axioms

Delete `lib/glue/c.bv`'s protocol half for real (G3 left it generated but
present). Update the doc matrix from the parent plan §7:
`glue-ffi.md`, `glue-as-abi-generator.md`, `macro-system.md`,
`ir-layers-and-naming.md`, `protocol-model.md`, `casting-protocol.md`,
`MASTER-SYNTAX-REFERENCE.md`, and `docs/plans/INDEX.md`.

### D1 — the removed-surface gate

`src/vocab.rs` already exposes `is_removed_keyword`/`removed_keywords`. Add **one**
diagnostic path for removed surface at the decl/token dispatch (the `#!exit`
pattern, generalised: what went, why, replacement), plus a script/CI check that
fails when a *live* file (not `docs/plans`, not `archive/`) uses a removed form.

- **Gate:** a compile-fail fixture per removed form; the live-file check green on
  today's tree.

### D2 — sweep

`#out` (fails today with `expected LParen, found 'print_int'`), `#frgn`,
`frgn` without `from`, top-level `term`. Same treatment as `#!exit`: remove from
live users, keep the historical records untouched.

- **Gate:** each form produces the new diagnostic; the sweep leaves no live
  `.bv`/doc using them.

---

## 5. Verification matrix

| After | Run |
|---|---|
| any step | `cargo test --lib -- glue::` (or the touched module) + `scripts/gate.sh quick` + Praetor on touched files |
| `P0`, `H1`, `H2` | plus `cargo test --lib` (shared analysis: parser, HIR) |
| `G4`, `R1` | plus `benchmarks/ll_determinism_check.sh --quick` and the parity gate itself |
| `S5` | plus `tests/diagnostics` (compile-fail corpus) |
| before any push | `bash scripts/gate.sh full` once; `git fetch` first (main is shared) |

---

## 6. Stop conditions

Stop the grind and record in `BUGS.md` (with reproduce) if:

- `P0` bisection shows the parser limit is structural and flattening further would
  make descriptors unreadable → escalate as a parser design decision.
- `G4` cannot be made byte-identical after two attempts → the generator is missing
  a fact; record which, do not "fix" by keeping both files.
- `PY` needs Rust → the schema is wrong; record exactly which fact had nowhere to
  live.
- Any step's gate fails twice on pre-existing unrelated failures → that is a
  baseline problem, not this plan's; record and continue with the step's own
  gate.

## 7. Risks, ranked

1. **Generated ≡ hand-written parity is the load-bearing claim.** Everything else
   is plumbing. Treat a parity failure as a design bug.
2. **The shared `.dbv` parser** (`P0`). Small change, wide blast radius.
3. **HIR expansion scope creep.** Only add fields a pass consumes; `H3` is what
   proves consumption.
4. **Python's real cost is GIL/pinning, not marshalling.** `PY` must measure, not
   just pass.
5. **Generated files in git.** Accepted deliberately (reviewability, bisect), with
   `--check` closing drift.

## 8. Definition of done

The grind is done when:

1. `lib/glue/c.bv`'s protocol half is generated, and IR is byte-identical to the
   hand-written version (`G4` green, then `R1` deletes it).
2. HIR represents and *reports* the boundary surface — the BUGS entry's fixture
   audits clean **because the facts are modelled**, not because the audit is
   blind.
3. Adding Python required no Rust (or the plan records precisely why not).
4. No live file uses a removed surface, and the check that keeps it that way runs
   in CI.
5. Docs: the parent plan §7 matrix updated; `INDEX.md` current; every decision
   (D1–D6) traceable to the code that enforces it.
