# Codegen bug-hunt strategy

**2026-11-24.** Base: `main` `31456c1b5` (branch `work/codegen-bug-hunt`).
Scope: find codegen defects **systematically** — by root-cause cluster and by
mechanical harness — instead of one reported symptom at a time. No performance
claim is made; this is a correctness-search plan.

## 1. Why a strategy and not a bug list

The last ~25 commits that touch `src/backend/` are almost all correctness fixes,
and they cluster. Reading them as bug **classes** rather than incidents tells us
where the next defects are, and which harness would have caught them for free.

## 2. Codegen map (where the mass is)

| Module | Lines | Role |
|---|---|---|
| `src/backend/llvm/emit_expr.rs` | 14,223 | expression lowering (the largest surface) |
| `src/backend/llvm/mod.rs` | 10,983 | backend driver, protocol/type helpers |
| `src/backend/llvm/emit_toplevel.rs` | 10,939 | defns, state, exports, `llvm_type` |
| `src/backend/llvm/emit_stmt.rs` | 4,768 | statements, `term` adaptation |
| `src/backend/llvm/intrinsics.rs` | 4,471 | intrinsic lowering |
| `src/backend/llvm/helpers.rs` | 3,913 | boxing/adaptation seams |
| `src/backend/llvm/loop_engine/*` | ~5,300 | counted/convergent loops, SSA |
| `src/backend/spirv/*`, `ptx/*` | ~20,000 | GPU tiers (separate gate discipline) |
| others (`electronics`, `bld`, `bad`, `circt`, `vm`, `webstack`) | ~12,000 | non-LLVM surfaces |

Total `src/backend` ≈ **140k lines**. The LLVM path is where most correctness
bugs land, and where the mechanical harnesses below are cheapest.

## 3. Bug classes visible in recent history

| Class | Representative fixed defects |
|---|---|
| **Target-width / ABI width** | wasm32 `Ptr`-param handle at the wrong width (**still OPEN**, §4 C1); `.bv` wasm32 prelude `i64` vs `i32`; FFI `argc` widened |
| **Value-representation seams** | loop-carried `Float` accumulator read as `i64`; `Float16` element write emitted invalid IR; aggregate arg forwarded as an `i64` handle where `ptr` was declared; `Data → Float` bitcast (i64→float) |
| **Silent output/state loss** | counted bare `term` returned without publishing counters; **node body swallowing all output** (OPEN) |
| **Ownership / provenance** | stack struct image outliving its frame; `List<struct>` return handing back a corrupt element store (OPEN); Free# on a possibly-static producer |
| **Control flow** | diverging match arm killed the whole continuation; `willreturn` on a recursion cycle deleted the program |
| **Determinism** | state-field index and interned-string order varying per process |
| **Interpreter/backend divergence** | Float relationals; `Float as String` rendering (OPEN); missing cast-lane prelude (OPEN) |

## 4. Root-cause clusters (ranked by yield × generality)

### C1 — Target-blind width resolution (highest yield; architectural)

Two parallel type-resolution paths exist:

- **Mandated path** (Architecture Pillars): `CastingGraph::resolve_llvm_type(
  universe, ty, int_bits)` — `src/casting/graph.rs:1533`. Protocol-driven;
  width priority `bits → maxbits → minbits → int_bits`. **18 call sites.**
- **Legacy path**: `LlvmBackend::llvm_type` (`emit_toplevel.rs:1422`) →
  `types::lower_type` → `protocol_llvm_type` (`mod.rs:1277`). Name/structure
  matches with `bytes` fallback and a final **`"i64"` default**
  (`mod.rs:1320`, `types.rs:41`). `protocol_llvm_type` **has no `int_bits`
  parameter at all**. **149 call sites** of `self.llvm_type(`.

Consequence: on a 32-bit target, a flexible `Int`/`String`-handle/Bool value
resolved through the legacy path is claimed as `i64` while other sites produce
`i32`. The existing `frgn_llvm_type` (`mod.rs:1473–1478`) is a **patch for
exactly this** (`matches!(ty, Type::Custom(name) if name == "Int") && int_bits == 32`),
i.e. a known symptom of the split.

The scale of the compensating-patch pattern (all counts on `main` `31456c1b5`):

| Pattern | Sites |
|---|---|
| `self.llvm_type(` (target-blind legacy resolver) | **149** |
| `casting_graph.resolve_llvm_type(…, int_bits)` (mandated path) | **18** |
| `format!("i{}", …int_bits)` (hand-applied target width) | **68** |
| `Type::int()` name comparisons in the LLVM backend (Rule 19) | **17** |

Two ad-hoc twins of the *same* workaround, neither general:
`frgn_llvm_type` (`mod.rs:1473`) and `plain_global_llvm_type`
(`helpers.rs:2920–2930`) each re-special-case the flexible `Int` before
delegating the rest to the target-blind resolver. `emit_expr.rs:7110–7122`
(`Blob` `^^Length`) hand-`load i64`s then `trunc`s to `int_bits`. So the width
knowledge lives in 68+ scattered sites instead of in one target-aware
resolver — the anti-pattern the Operating Contract's question 1 names.

Prediction (falsifiable): a corpus-wide **cross-target IR sweep** finds programs
that pass natively and fail to verify (or fail to build) for `wasm32-*`,
because a declaration and its uses disagree on width. Probe P1 tests this.

Known open instances: `BUGS.md` 2026-11-17 (wasm32 `Ptr`-param handle),
2026-11-11 (wasm32 prelude `i64` vs `i32`).

### C2 — i64 boxing/adaptation seams

`helpers.rs` concentrates the "uniform i64 storage" conversions:
`box_to_i64` (`helpers.rs:2609`), `adapt_to_i64` (`helpers.rs:2713`),
`ensure_typed_value`, `ptrtoint … to i64` (`helpers.rs:715, 972, 2785, 3091`),
`zext … to i64` in ~12 places. Every historical "invalid IR at a boundary" bug
lives on this seam. The primitive is fine; the defects are **sites that box or
unbox without knowing the other side's declared type**.

Separately found while reading (low priority, not claimed as a live defect):
`abi.rs:39–63` `marshal_export_wrapper` takes a `universe` argument, uses it for
`is_bool_type`, but passes **`None`** to `lower_type` for every declared
parameter/return type — so a `Bool` wrapper would declare `i64` and then emit
`trunc i8 %p0 to i1` against it. It currently has **no callers** (the live
`#export` path is `emit_toplevel.rs:9230`), so it is dead rather than wrong-live.
Either retire it or convert it to the target-aware resolver; leaving a
defect-shaped dead emitter is how the next caller inherits it.

### C3 — Type-name matching (Rule 19 violation; 24 sites)

`git grep` for `Type::Custom(…) ==` in `src/backend/llvm` + `src/glue` returns
**24 hits** (Rule 19 requires zero outside the named exceptions). Examples:
`emit_expr.rs:7266–7267` (`"Bool" | "UInt8" | "Int8"` → 2),
`mod.rs:1474` (`"Int"`), `emit_stmt.rs:3069–3075`, `glue/web_generator.rs`
(6 sites). Each is a latent width/category bug of the C1 family: the Data→Float
bitcast defect fixed on 2026-11-17 came from precisely this pattern.

Provisional triage (verdicts still need per-site source verification in P2):

| Site | Provisional verdict |
|---|---|
| `mod.rs:1474` `frgn_llvm_type`; `helpers.rs:2926` `plain_global_llvm_type` | **fix** — retiring these two Int patches is part of C1 |
| `emit_expr.rs:7266–7267` `type_category_code` | **fix** — category must come from protocol membership (`Cast.*`), not names; it mirrors the interpreter, so both sides change together (Rule 5) |
| `glue/web_generator.rs` 6 sites | **fix** — JS type mapping should read the resolved protocol/metadata |
| `emit_expr.rs:7112` `"Blob"` | **fix** — string-like must be protocol membership, not a name |
| `emit_stmt.rs:3069–3075` enum-handle accepts `"Int"` | **verify** — may be a legitimate handle/unbox rule |
| `emit_expr.rs:6690`, `mod.rs:9181` (`"Self"`), `counter.rs:2600`, `emit_stmt.rs:4353`, `emit_stmt.rs:3049` | **keep** — comparing two *dynamic* type values for equality, not asserting a library type's semantics |

### C4 — Silent output/state loss in the reactor/region path

`BUGS.md` 2026-11-23: a node whose guarded block **starts** with a collection
member call (`m.insert(...)`) prints nothing and exits 0; a preceding `println!`
makes the same program correct. Emitted `.ll` is complete, so the loss is at
run time in the flush/region path. Silent-wrong-output is the highest-severity
class: nothing fails, the answer is simply absent.

### C5 — Ownership / provenance reclamation

Open: `List<struct>` create-append-return handing back a corrupt element store
(2026-11-22); plus the still-open conservative-drop census drift. Recently fixed
siblings prove the area is fertile.

### C6 — Interpreter/backend divergence (Rule 5 direction)

Open: interpreter lacks the cast-lane prelude (`1.25 as String` → `1.25000` vs
`1.25`); Float width is inconsistent end-to-end (f32 codegen vs f64 interpreter
and 64-bit operator bindings). These block an exact native-float differential
family, which is itself the strongest oracle we want.

## 5. Search machinery (harnesses)

| # | Harness | Finds | Cost | Status |
|---|---|---|---|---|
| **H1** | **Cross-target IR-validity corpus sweep** — compile every `tests/tier1/*.bv` (+fixtures) for each supported triple, require success and LLVM verification | C1, C3, width/ABI seams | minutes | **new (P1)** |
| H2 | Optimizer-level differential (`-O0`/`-O2`/`-O3` stdout equality) | miscompiles by opt level | nightly already does tier1 only | extend to full corpus |
| H3 | Interpreter-vs-native differential over the corpus | reference divergences (C6) | documented gap in `verification-nightly.yml` | **new** |
| H4 | Well-typed generator differential (`fuzzing` feature, `src/typed_program.rs`) | C2/C5 at scale | harness exists; generator surface is shallow | strengthen shapes |
| H5 | `.ll` byte-determinism gate | nondeterministic emission | exists (`benchmarks/ll_determinism_check.sh`) | run full mode |
| H6 | Panic/crash robustness (`crash_regressions.rs`, fuzz targets) | ICEs on valid-but-unusual input | exists | extend |
| H7 | Memory campaigns (Valgrind/ASan, `tests/memory`) | C5 leaks/UAF | exists | extend to new shapes |

H1 is first because it is **mechanical, deterministic, cheap, and predicted to
have a high hit rate** from C1 alone — and it needs no oracle: a program that
fails LLVM verification or fails to build for a declared target is a defect
regardless of intent.

## 6. Probes, acceptance criteria, and first fixes

- **P1 (run now).** Sweep the corpus across all declared triples; classify each
  failure as (a) invalid IR, (b) build error, (c) compiler panic. Success
  criterion: a reproducible failure list with a minimal reproducer per class.
- **P2.** Audit the 24 Rule-19 sites into a table: keep (compiler construct /
  mirrored reference) vs fix (derive from universe/casting graph). Deliverable:
  the audit, then C1's fix in the same commit as the sites it repairs.
- **P3.** Reproduce C4 with a 10-line program; trace the flush/region path
  (frontend emission order vs runtime), then fix at the general site.
- **P4.** Give the interpreter the backend's lane prelude (C6), turning the
  existing number-only fixture into a string-equality fixture.

Fix order follows blast radius: **C1/P2 → C4 → C5 → C6**. Each fix lands with
(a) a tier1 fixture + `test { }` block or a corpus entry that fails before and
passes after, (b) the harness that would have caught it wired into a gate
(H1 into `scripts/ci_native.sh`'s `targets` check), and (c) a `BUGS.md` record.

## 7. Documentation and retirement obligations

- Architecture docs updated in the same commit as structural change:
  `docs/architecture/backend-type-dispatch.md` (record the legacy-path
  retirement), `docs/architecture/backend-contracts.md` §8 (known limitations).
- Every temporary containment carries a dated rationale plus its retirement
  gate (e.g. `frgn_llvm_type`'s `"Int"` special case retires when
  `protocol_llvm_type` is target-aware).
- Tests are behavioral (contract-shaped), not literal-IR, except where the IR
  property *is* the contract (invalid-IR emission).
- New source files carry the LLVM-exception SPDX header.

## 8. Execution record

### P1 — cross-target IR sweep: running

`/tmp/opencode/codegen-bughunt/cross_target_sweep.py` compiles every
`tests/tier1/*.bv` + `tests/fixtures/*.bv` + `tests/tier2/*.bv` for
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`riscv64-none`-style and `wasm32-unknown-wasi`, classifying each failure as
invalid IR / build error / unsupported target / compiler panic. Results are
appended below when the run completes (`sweep/failures.json`).

### P3 — C4 reproduced, root cause pinned (DONE)

The 2026-11-23 silent-node bug is live on `main` `31456c1b5` and its root cause
is now known: **an emitted read of uninitialized `__briev_region_malloc`
storage folds to `undef` under `-O2+`, and `SCCP` deletes the whole observable
continuation.** Full evidence and fix direction: `BUGS.md` (2026-11-24 entry).

Cheapest regression gate established by the probe: the same source must print
the same bytes at `--safe` (`-O0`) and at the default `-O3 -flto` profile. The
2026-11-23 fixture's golden is `7\n0\n`.

The repro fixture + golden are prepared at
`/tmp/opencode/codegen-bughunt/c4/inlined_member_first_statement.{bv,golden}`
and land in `tests/tier1/` **with the fix** — a known-red corpus entry must not
be committed on its own, since `tier1_corpus_compiles_links_and_runs` gates CI.

Raw artifacts: `/tmp/opencode/codegen-bughunt/c4/{silent,control,run.out,
safe/run.out,silent.ll,lto/dumps_full.txt,lto/opt1.ll,lto/opt2.ll}`.

### P1 (first pass) + P4 — two defects found, both fixed

The first sweep pass was polluted: it ran against this worktree while the
stdlib edit was mid-flight, so its aarch64 "type errors" were my own broken
intermediate state. The re-check after the edit settled gave the real payload.
x86_64 was a clean control (13 failures, all fixtures that legitimately do not
build: parse-error probes, termination/liveness probes).

**Defect 1 (C4, fixed in the program).** The 2026-11-23 silent-node loss: see
the `BUGS.md` 2026-11-24 entry. `Malloc#` is pinned to `@malloc`, so the
emitted IR was faithful and the defect was `HashMap.init`'s un-zeroed
occupancy array; a shared `defn new_occupancy` now allocates-and-clears for
both `init` and rehash. Zeroing in the codegen allocator was rejected as a
semantics change to a pinned intrinsic plus a memset on every allocation.

**Defect 2 (C1, fixed in codegen).** The aarch64 arena `brk` asm passed three
operands for two input constraints, so **every** arena/collection program was
uncompilable for aarch64. Fixed in `emit_brk_syscall`; pinned by a behavioral
operand/constraint parity test that fails on the pre-fix emission.

**New general harness (H1), wired into CI.** `scripts/cross_target_ir.sh` — a
self-calibrating cross-target IR-validity sweep (native acceptance decides
corpus membership, so no skip list can rot) and a new independent
`cross-target` case in the `native` CI matrix. This is the machinery that found
defect 2; the `targets` case's one-fixture-per-triple check is now subsumed.

**Retirement / follow-ups.** (a) wasm32 + bare-metal remain a *different*
capability surface and are reported, not gated. (b) The interpreter heap
zero-fills, which is why the C4 class hides from `brievc test` — an
interpreter-semantics decision for its owner. (c) The two rival type resolvers
(C1) are untouched: the aarch64 sweep reported only the `brk` arity defect, so
the target-blind-width hypothesis needs the *wasm32* arm to show itself.
