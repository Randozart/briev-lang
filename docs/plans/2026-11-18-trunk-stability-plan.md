# Trunk Stability Plan — Native Flake, #7 Audit, FFI-into-HIR, PF_exp (2026-11-18)

Companion to `2026-11-17-trunk-next-steps.md`. This plan covers the four
language-stability workstreams identified after landing the issue #7/#8
fixes (`76848f4de`). Ordered by trunk-unblocking value, then substantive
language work.

## Status snapshot (why these four)

- The issue #8 (`term` in reactor guard) and issue #7 Phase 1 (post-outline
  validation) fixes are **landed and verified**: lib 3921/0, quick gate,
  determinism 5/5, glue parity, fixture tests green.
- The **only red CI job is `native`** (accel). Every other gate — library,
  compiler, workspace, browser, contracts, verifier, Fuzz,
  Verify-Bodies-Census — is green on `76848f4de`.
- `native` has been failing on **nearly every push today** (see table below),
  on commits this work never touched. It is the known `-march=native`
  SIGILL flake, not a compiler defect.

| CI run | commit | native |
|---|---|---|
| 44313654211 | `76848f4de` (my #7/#8) | failure |
| 44302229501 | `1e9f7a89d` (#124) | success |
| 44292211677 | `4cf848fd6` | failure |
| 44290443184 | `dc1e726cf` (merge) | failure |
| 44280030077 | `24e7fffc8` | failure |
| 44268017852 | `6d02d4621` | failure |
| 44261489827 | `e96a3ace7` | failure |
| 44171638316 | `519536bf9` | success |

Pattern: success on ~2 runs, failure on most. Intermittent = environment,
not code.

---

## W1 — Quarantine the `native` flake (unblocks the trunk)

### Findings

The `native` gate runs `scripts/ci_native.sh accel` →
`benchmarks/accel_reactor_nonhang.sh`, which builds and runs
`benchmarks/nbody_newton_accel.bv` via `brievc build`. The build link step
(`src/compile.rs:3466`) injects `-march=native -flto -O3 -ffast-math` for
native x86_64/linux triples. The gate then runs the binary under
`timeout -s KILL 30s` and fails on `RC != 0` (which includes the SIGILL
exit). The gate's own non-hang check (`accel reactor non-hang: PASS`)
passes — the failure is the binary *crashing* after building, i.e. the
runner's native codegen miscompiles on AVX/AMX.

**Key discovery: a conservative profile already exists.** `--safe`
(`src/compile.rs:3328`, flag at `src/main.rs:715`, plumbed to
`safe_mode` at `:1058/:1440/:3440`) selects `-O0 -march=x86-64` (generic,
no native CPU features, no `-flto`, no `-ffast-math`) — the exact
conservative counterpart to the flaky native path, added for issue #9
(2026-11-15) precisely to triage large-IR instability "without optimizer
interference." **The native gate simply does not use it.**

### Two-layer fix: stop the flake now, fix the root cause

**A. Immediate (band-aid): run the `accel` gate in `--safe` mode.**
- Change `benchmarks/accel_reactor_nonhang.sh` to build with `--safe`
  (`brievc build … --safe`). This removes `-march=native`/`-flto`/
  `-ffast-math` from the accel gate's binary, so the gate no longer
  depends on the runner's AVX/AMX codegen — it tests the *reactor logic*
  (the thing the gate is actually for: "a `beginprogram && i < nb` entry
  node must fire and make progress"), not the machine's native codegen.
- **Trade-off (honest):** `--safe` also drops `-O3 -flto`, so the gate no
  longer exercises the *optimized* path. That's acceptable as a temporary
  measure because the native-codegen risk is covered by the `--runtime`
  benchmark suite (`-march=native`) and `ll_determinism_check.sh`.
  This is a **stopgap**, not the long-term answer — see B.

**B. Root cause: make `-march` config-driven, default `x86-64-v3`.** ✅ IMPLEMENTED 2026-11-18

Implementation notes:
- The default is `x86-64-v3` — a **feature-set level**, not a concrete CPU
  name. clang/LLVM accepts feature-set levels as `-march` values
  (x86-64-v2/v3/v4 are valid — see clang's own 'valid target CPU values'
  list). v3 = SSE4.2 + AVX + AVX2 + FMA; it does NOT include AVX-512/AMX —
  the feature set that intermittently SIGILLs on GitHub runners under
  `-march=native`. This is exactly the deterministic baseline the plan
  asked for.
- `--march` accepts both `--march=<cpu>` and `--march <cpu>` (mirrors `--accel`).
- **Parser fix** (`src/dbriev/v2.rs::parse_bare_token`): bare tokens now
  terminate at `\n` (line-oriented `.dbvl`), matching `parse_positional_values`.
  Without this, a bare token at end-of-line (no trailing `;`) runs into the
  next line and swallows it — `target.x86_64: …; x86-64-v3` parsed the march
  field as `"x86-64-v3\ntarget.aarch64: 32"`, which is what made the earlier
  `unknown target CPU` error appear to be a name problem. The name was never
  the issue; the parser was. This also corrects the documented `#`-line
  behavior in `config_db::tests::only_double_slash_comments_are_skipped`
  (the `#` line no longer swallows the following keyed line).
- `benchmarks/build_and_bench.sh` was left unchanged — it passes
  `-march=native` explicitly via clang flags in the harness, not via the
  `--march` CLI flag. (The plan's note to thread `--march=native` through
  `build_and_bench.sh` is moot: the harness already controls `-march` at the
  link step directly.)

The deeper problem is that `-march=native` is a **build-host property**,
not a **deployment target** property. The IR is portable; the link step
chooses the ISA. The ISA a binary runs on is a deployment decision, not an
accident of which runner built it. Two runners with different AVX levels
produce different binaries from the same IR — that's non-determinism by
construction.

The correct source of truth is the **target config**
(`config/targets.dbvl` → `config_tuning::TargetSettings`), which already
holds per-target codegen knobs (`float_registers`, `vector_min_width`,
`isr_mechanism`, etc.) looked up by `target_triple` prefix. Add a `march`
field:

- `config/targets.dbvl`: add a `march` column to the tuning rows
  (`target.x86_64: …; <march>;`, `target.aarch64: …; <march>;`).
- `src/config_tuning.rs`: add `pub march: Option<String>` to
  `TargetSettings`, parse it from the tuning row.
- `src/compile.rs:3466` (the `is_native` branch): read the march from
  `TargetSettings` via `target_settings_for(triple)`; fall back to a
  conservative per-triple default (`x86-64-v3` for x86_64-linux — SSE4.2,
  AVX; widely available, no AVX-512/AMX). A `--march=<cpu>` CLI flag
  overrides for explicit deployment targets. `--march=native` remains
  available for local development (fastest on the dev machine) but is
  **never the default**.
- The benchmark harness (`benchmarks/build_and_bench.sh`) passes
  `--march=native` **explicitly** — it's measuring *throughput on the
  host*, which is the right question for a perf benchmark. The CI
  correctness gate does not need that; it needs a deterministic ISA.

**C. Bounded crash-retry (belt-and-braces, keep).**
- Even with `x86-64-v3`, a flaky runner can transiently crash. Add a small
  retry (e.g. 3 attempts) to `accel_reactor_nonhang.sh` *only* for a crash
  (non-zero, non-timeout) — re-running the same binary is cheap and
  disambiguates "flaky" from "regressed."
- **Do NOT retry on timeout** — a timeout is the actual regression the gate
  was written to catch (stale-terminated lazy fold). Retrying that would
  hide the bug.

### Files touched
**Layer A (band-aid):**
- `benchmarks/accel_reactor_nonhang.sh` (add `--safe` build; bounded
  crash-retry).

**Layer B (root cause):**
- `config/targets.dbvl` (add `march` column to tuning rows).
- `src/config_tuning.rs` (`TargetSettings.march: Option<String>` + parsing).
- `src/compile.rs:3466` (read march from config; `--march` CLI flag;
  default `x86-64-v3` for x86_64-linux).
- `src/dbriev/v2.rs` (`parse_bare_token` newline terminator — required for
  the march field to parse correctly as a trailing bare token).
- `benchmarks/build_and_bench.sh` (pass `--march=native` explicitly —
  it's a perf gate, not a correctness gate).

**Layer C:**
- `benchmarks/accel_reactor_nonhang.sh` (bounded crash-retry).

**Docs:**
- `docs/plans/2026-11-17-trunk-next-steps.md` (mark the native flake
  quarantined).
- `docs/architecture/backend-contracts.md` (record the march-as-deployment-
  target decision — the ISA is a config fact, not a build-host accident).

### Verification
- **A:** `bash benchmarks/accel_reactor_nonhang.sh` green locally (builds
  in safe mode, reactor fires, PASS). Confirm the link command has no
  `-march=native`.
- **B:** `brievc build tests/fixtures/minimal.bv` (no `--march` flag) links
  with `-march=x86-64-v3` (inspect the link command). `--march=native` is
  accepted and overrides. The `targets` gate and `toolchain_matrix.sh`
  still exercise the native triple for build/link coverage. Push; the
  `native` CI job goes green (the flake is gone from the path — the gate
  now uses `x86-64-v3` by default, not `native`).
- **C:** crash-retry verified by forcing a transient crash (or trusting the
  existing CI retry pattern).

### Risk
**A: Low** (benchmark script + existing flag, no compiler changes).
**B: Low–Medium** (config plumbing + one branch in `compile.rs`; the
  behavior change is "the default ISA is now deterministic," which is the
  *fix*, not a regression; the benchmark harness explicitly opts into
  `native`).
**C: Low.**

---

## W2 — Issue #7 Phases 2–3 (outline rewrite-completeness audit)

### Findings (from the #7/#8 research doc)

- **Phase 1 is done and green** (`76848f4de`): `validate_cold_body_idents`
  (`src/backend/llvm/emit_toplevel.rs`) fires a loud diagnostic when a
  rewritten cold-guard body references an identifier that is neither a
  `__cp_` param nor a module global (shares `ident_is_module_global`, made
  `pub(crate)` so validation and emission can never disagree). It produced
  **zero false positives** across the corpus (lib 3921/0).
- The residual gap Phase 1 does **not** close: it validates *after* the
  rewrite. If `collect_idents` fails to *capture* an identifier that
  `rewrite_expr_idents` *would* rewrite, the identifier is never in
  `field_names`, so it is never substituted, and — critically — Phase 1's
  validation only flags identifiers that are neither `__cp_` nor a module
  global. An *uncaptured state field* is a module-global-adjacent name that
  *will* resolve (via `%state` GEP) at emission time, so it would pass both
  the rewrite and the validation — but read the **wrong value** (the
  `%state` field, not the captured param copy) in the cold context. That is
  the true silent-correctness shape.

### Plan (phased, gated)

**Phase 2 — total coverage test (no new code, just evidence).**
- Extend `tests/outlined_guard_validation.rs` to exercise *every*
  `Expr`/`Statement` shape `rewrite_expr_idents` / `collect_expr_idents`
  claim to handle *inside an outlinable guard*: `Identifier`, `Call`,
  `BinaryOp`, `UnaryOp`, `Cast`, `Field`, `Index`, `List`/`Tuple`,
  `Block`, `Deref`/`AddrOf`, nested `Guarded`, `Let` with expr.
- For each, assert the emitted cold function (a) has the expected `__cp_*`
  param, (b) reads through it (not `%state`), (c) passes the Phase-1
  validation with no diagnostic.
- **Outcome is binary:** if every shape is covered, Phase 3 is a no-op
  (the rewrite is total — document that in the ledger). If a shape is
  missed, this test *names the exact shape* and Phase 3 has a concrete
  target.

**Phase 3 — repair (only if Phase 2 finds a hole).**
- Add the missing arm to *both* `collect_expr_idents` and
  `rewrite_expr_idents` (they must stay in lockstep — the validation's
  `collect_idents_in_expr` is the third copy; consider consolidating all
  three into one shared walker to guarantee they cannot drift, per DRY
  rule 17).
- If consolidation is chosen, that is the durable fix: one
  `fn each_identifier(expr, f)` used by collect, rewrite, and validate.

**Gating rule (per Rule 20 / performance-recovery protocol):** Phase 3 is
**blocked unless** Phase 2 produces a concrete uncovered shape. A blind
"make the walker total" refactor of a green path risks regressions for no
measured gain. Phase 2's evidence decides.

### Files touched
- `tests/outlined_guard_validation.rs` (Phase 2 coverage matrix).
- Possibly `src/backend/llvm/emit_toplevel.rs` (Phase 3 repair + optional
  walker consolidation).
- `docs/plans/2026-11-17-issue-7-8-deep-research.md` (record Phase 2 outcome).

### Verification
- `cargo test --test outlined_guard_validation` green.
- `cargo test --lib` 3921/0 (no regression from any Phase 3 repair).
- `bash benchmarks/ll_determinism_check.sh --quick` (outline path is backend).

### Risk
**Low (Phase 2), Medium (Phase 3 only if triggered).** Phase 2 is pure
test. Phase 3 touches a hot emission path, so it needs the full lib suite +
determinism, and is gated on real evidence.

---

## W3 — FFI follow-up: thread the protocol *contract* into HIR ✅ IMPLEMENTED 2026-11-18

Implementation: `HirProtocol.contract: Option<HirContract>` added
(`src/hir/program.rs`); `lower_protocol_item_full` lowers
`pd.contract` via the existing `lower_contract` helper (fresh
`IdSupply::default()` scoped to the contract's expressions — protocols are
not callables, so no def/node ids are allocated); `has_unrepresentable_contract`
is now `pd.contract.is_some() && contract.is_none()` (false whenever the
lowering succeeds — which is always, since `lower_contract` handles every
`Contract` shape the parser produces). `count_protocols` unchanged in logic
(counts a proto only when `has_unrepresentable_contract` is true) — now
returns 0 for all protos. The H3/S4 audit story is closed: a
contract-bearing proto is fully represented, 0 unsupported facts.

Test update: `proto_with_contract_is_one_gap` →
`proto_with_contract_is_fully_represented` (asserts
`proto.contract.is_some()`, `has_unrepresentable_contract == false`, pre is a
real `HirExpr` not an `Other` gap, `counts.protocols == 0`, `counts.total() == 0`).

### Findings

- `HirProtocol.has_unrepresentable_contract` (set in
  `lower_protocol_item_full`, `src/hir/lower.rs:172`) is
  `pd.contract.is_some()`. Today it is the *only* protocol fact the audit
  counts as unsupported (`src/hir/stats.rs:61` counts a proto only when the
  flag is true). The doc comment states: *"Always `true` today (no HIR node
  exists yet); becomes `false` when a contract is lowered."*
- **The AST contract is `Option<Contract>`** (`briev-verify/src/ast/top.rs:1563`),
  and `Contract = { pre_condition: Expr, post_condition: Expr, watchdog:
  Option<WatchdogSpec>, explicit, post_authority }` (`:309`).
- **A `HirContract` node already exists** and is reused by `HirCallable`
  (`src/hir/program.rs:90`): `{ pre: HirExpr, post: HirExpr, watchdog:
  Option<HirWatchdog> }`. So lowering a protocol contract is a *direct
  reuse* — no new node type, just a new field + lowering the two `Expr`s to
  `HirExpr` (the same machinery `HirCallable::contract` uses).
- The audit's "one unsupported fact per proto" (`count_protocols`) will
  then drop to the *true* residual (a proto with a contract that IS lowered
  → 0 gaps), closing the H3/S4 audit story.

### Plan

1. Add `pub contract: Option<HirContract>` to `HirProtocol`.
2. In `lower_protocol_item_full`, when `pd.contract.is_some()`, lower
   `pre_condition`/`post_condition` to `HirExpr` (reuse the callable
   contract lowering helper) and `watchdog` to `HirWatchdog`; set
   `contract = Some(…)`. Keep `has_unrepresentable_contract = false` in that
   case (the fact is now represented). When `pd.contract.is_none()`,
   `contract = None`, `has_unrepresentable_contract = false`.
3. Update `count_protocols` (`src/hir/stats.rs:48`): a proto is now
   unsupported iff `has_unrepresentable_contract` is true **OR**
   `contract.is_some()` but not fully lowerable. With step 2 always lowering,
   the count becomes the honest residual (expect 0 for the corpus's
   contract-bearing protos).
4. **Round-trip / parity:** add a test that a protocol with a contract
   lowers to a `HirProtocol` with a non-empty `contract`, and that the
   AST/HIR equivalence (the parity gate) still holds. Update the H3 test
   `proto_with_contract_is_one_gap` → now the gap is represented, so the
   expectation changes to "contract carried, 0 unsupported facts."
5. Keep `BRIEV_HIR_EMIT=0` (AST) path unaffected — this is additive on HIR.

### Files touched
- `src/hir/program.rs` (`HirProtocol.contract`).
- `src/hir/lower.rs` (`lower_protocol_item_full`).
- `src/hir/stats.rs` (`count_protocols` + tests).
- `docs/plans/2026-11-16-ffi-contract-and-metaprogramming.md` (mark the
  contract-into-HIR follow-up done).

### Verification
- `cargo test --lib -- hir` green (the HIR parity + stats tests).
- HIR audit: a contract-bearing proto now reports `unsupported=0` for the
  protocol fact (the contract node is present).
- Full lib + quick gate (HIR is the default path).

### Risk
**Low.** Additive field + lowering of two already-lowerable `Expr`s. The
only subtlety is the audit count flip, which is a test update, not a
behavior change. No backend emission depends on it yet.

---

## W4 — PF_exp induction (prover-side, last)

### Findings

- `docs/plans/2026-11-17-trunk-next-steps.md:85` marks it "tracked but not
  started," lower priority, explicitly "last."
- It is **prover-side** (briev-verify body prover / SMT), not compiler
  codegen. The structural-projection groundwork (field-projection
  `field_assignments`, commit `c732c0b02`) already landed; PF_exp
  induction builds on the body prover's induction support.
- It is only now unblocked: the original gate was "parked until compiler
  issues clear," and the #7/#8 fixes + the native-flake quarantine (W1)
  satisfy that.

### Plan

1. Read the existing induction scaffolding in `briev-verify` (the body
   prover's `induction` path) and the `pf_exp` contract it must discharge.
2. Derive the induction hypothesis for `pf_exp` (the exponential/fixed-point
   shape) using the field-projection support; wire the inductive step into
   the SMT query builder.
3. Keep it **advisory/unproven-safe**: per the standing constraint, the
   prover proves only its conservative pure-integer subset; an unsupported
   `pf_exp` step stays advisory (no CI failure) until the induction covers
   it.
4. Add a Kani harness (standalone crate, standard `assert!`) for the
   safety-critical inductive step, and a `briev-verify` unit test that the
   `pf_exp` contract now discharges.

### Files touched
- `briev-verify/src/` (body prover induction + SMT query).
- `kani/` (new harness).
- A `briev-verify` test.

### Verification
- `cargo test -p briev-verify` green.
- `cd kani && kani prove <harness>` passes.
- `pf_exp` conformance program now PROVEN (not UNPROVEN) in the census.

### Risk
**Medium — and it's deferred by design.** This is the only workstream that
adds new *proof* surface. It must not weaken any existing contract (Rule 1)
and must stay advisory for uncovered cases. It is deliberately last because
it is the only one that can't be verified by a green CI job — it needs the
proof to actually go through.

---

## Sequencing & dependencies

| order | workstream | blocks | est. effort |
|---|---|---|---|
| 1a | **W1-A** native flake band-aid (`--safe`) | none — unblocks the trunk today | small |
| 1b | **W1-B** march config-driven (root cause) | W1-A (need the trunk green first) | small–med |
| 1c | **W1-C** crash-retry | none (independent of A/B) | small |
| 2 | **W3** FFI contract into HIR | none (independent of W1/W2) | small |
| 3 | **W2** #7 Phase 2 audit | W1 (need a green trunk to trust the signal) | small–med |
| 4 | **W2** #7 Phase 3 repair | W2 Phase 2 finding (gated) | med (only if triggered) |
| 5 | **W4** PF_exp induction | W1 (compiler issues clear) | med–large |

**Why this order:** W1 first because a red trunk is masking signal for
*everything else* — every subsequent gate's green must be trustworthy, and
that requires the flake gone. W3 next because it's small, self-contained,
completes a started story (S4/H3), and is independent. W2 after W1 because
its Phase-2 evidence is only meaningful on a stable trunk; Phase 3 is
strictly gated on Phase 2. W4 last because it's the only one whose
verification is a proof going through, not a CI job.

**Parallelizable:** W1 and W3 are independent and could be two lanes. W2
and W4 both need W1's green trunk.

## Out of scope / deferred

- The broader `-march=native` codegen *correctness* (not just the flake) —
  that's the `--runtime` benchmark lane and `toolchain_matrix.sh` (S5-A),
  not a stability fix. W1 only stops the *gate* from flaking.
- Consolidating the three identifier walkers (collect/rewrite/validate) is
  offered *inside* W2 Phase 3 as the durable fix, not a standalone project.
- Cross-toolchain LLVM parity (blocked: only `clang 23.1.5` locally) and
  macOS `SysCall#` validation (needs a Mac host) remain blocked, unchanged.
