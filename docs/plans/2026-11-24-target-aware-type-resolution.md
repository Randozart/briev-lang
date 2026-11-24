# Target-aware type resolution (retire the target-blind resolver)

**2026-11-24.** Discovered by the cross-target IR sweep
([bug-hunt strategy](2026-11-24-codegen-bug-hunt-strategy.md) §4 C1). Status:
**OPEN — plan only, no code changed.** Found by
`scripts/cross_target_ir.sh wasm32-unknown-wasi`.

## Symptom

Programs the native triple accepts emit **invalid IR for `wasm32-unknown-wasi`**
because one value is resolved at two different widths. Sampling tier1, 5 of 12
probed programs fail; three distinct sites in one class:

| Fixture | Diagnostic | The two claimants |
|---|---|---|
| `plain_ptr_smoke` | `'%t21' defined with type 'i32' but expected 'i64'` on `store i64 %t21, ptr @p` | the global's declared type (i64) vs the value path (i32) |
| `ptr_crossdefn_probe` | `'%t7' defined with type 'i64' but expected 'i32'` | a defn signature vs its caller's value |
| `slice_basics` | `inttoptr i32 %t520 to ptr` where `%t520` is i64 | the pointer-conversion site (int_bits) vs the value's producer (i64) |

Pre-fix diagnostics: `/tmp/opencode/codegen-bughunt/wasm32-probe/`.

## Root cause (measured on `main` `31456c1b5`)

Two resolvers disagree about the target width:

| Path | Sites | Target-aware? |
|---|---|---|
| `CastingGraph::resolve_llvm_type(universe, ty, int_bits)` — the mandated path (`docs/architecture/backend-type-dispatch.md`, Architecture Pillars) | **18** | yes: `bits → maxbits → minbits → int_bits` |
| `LlvmBackend::llvm_type` → `types::lower_type` → `protocol_llvm_type` (`mod.rs:1277`) | **149** | **no**: no `int_bits` parameter; final fallback is the literal `"i64"` (`mod.rs:1320`, `types.rs:41`) |

68 further sites hand-apply `format!("i{}", self.ctx.int_bits)` to compensate,
and two functions re-special-case the flexible `Int` before delegating
(`frgn_llvm_type` `mod.rs:1473`; `plain_global_llvm_type` `helpers.rs:2920`).
The 2026-11-17 wasm32 `Ptr`-parameter handle bug — still open — is another
instance ("the fix is a unit": handle width, `reg_llvm_ty`/`reg_value_ll`, and
both consumers must agree).

## Work list (measured inventory of the 149 target-blind sites)

Ten files; 126 of 149 in three of them, so the migration is reviewable in slices
ordered by concentration:

| Sites | File |
|---|---|
| 72 | `src/backend/llvm/emit_expr.rs` |
| 34 | `src/backend/llvm/emit_toplevel.rs` |
| 20 | `src/backend/llvm/helpers.rs` |
| 12 | `src/backend/llvm/mod.rs` |
| 4 | `src/backend/llvm/integer_operands.rs` |
| 2 | `src/backend/llvm/emit_stmt.rs` |
| 2 | `src/backend/llvm/loop_engine/counter.rs` |
| 1 each | `kernel.rs`, `logical.rs`, `loop_engine/ssa.rs` |

Hot spots by enclosing function (migration order = highest first, since each is
a distinct value-boundary decision): `emit_binop_from_config` (13),
`export_boundary_parameter_types` (11), `emit_identifier_load` (8),
`emit_index_with_obj` (6), and 5 each in `finish_user_call`, `emit_cast_steps`,
`fallback_llvm_type`, `definition_ret_abi_ty`. `fallback_llvm_type` is the one
that must survive as a *target-aware* last resort rather than be deleted.

## Fix strategy (general; no per-type special cases)

1. **Make the legacy resolver target-aware.** Give the resolution chain the
   `int_bits` it is missing and have it delegate to the casting graph — i.e.
   `LlvmBackend::llvm_type` becomes a thin call to
   `resolve_llvm_type(universe, ty, int_bits)`. This is the *retirement* the
   Architecture Pillars already mandate, not a new mechanism.
   - Judge every arm instead of translating mechanically: arms that encode a
     **compiler construct** (`Type::Ptr`/`Vector`/`Bits(N)`, `Union` handle,
     `Dyn` fat pointer, `Function` pair) stay; arms that encode **library type
     knowledge** (`"Blob"`, `"Bool"|"Int8"|"UInt8"`, `"Char"|"Byte"`, `"Int"` at
     a width) are replaced by protocol membership. That simultaneously clears
     the 24 `Type::Custom(…) ==` sites Rule 19 requires at zero.
2. **Then delete the compensating patches**: `frgn_llvm_type`'s `Int` branch and
   `plain_global_llvm_type`'s `Int` branch; the 68 `int_bits` sites become
   redundant and shrink toward the few that are genuinely ABI-local.
3. **Prove it by measurement, not by reading** (Rule 20): the acceptance oracle
   is the emitted IR itself — `cross_target_ir.sh` over the whole corpus for
   `wasm32-unknown-wasi` must go from failing to **0**, with `aarch64` and
   native unchanged. Each migrated site is an IR A/B (before/after emission),
   not a code-review claim.

## Acceptance and gates

- `bash scripts/cross_target_ir.sh wasm32-unknown-wasi` → 0 failures (today:
  5-of-12 in a sample; the full arm is the measurement).
- `aarch64-unknown-linux-gnu` stays 0 (recorded: 100/100), `x86_64` native
  corpus stays green (`tier1_corpus_compiles_links_and_runs`).
- `scripts/gate.sh full` (shared analysis) and `bash scripts/gate.sh quick`.
- `benchmarks/ll_determinism_check.sh` full mode: the resolver is on the
  emission path, so byte-determinism must hold.
- Add the wasm32 arm to the `cross-target` CI case **only after** it is clean
  for the whole corpus; a permanently-red gate is worse than a documented one.
- Behavioral regressions: the three fixtures above join the sweep, plus a new
  tier1 fixture per consumer fixed (handle width, `term` adaptation, global
  declarator).

## Documentation obligations (same commit as the change)

- `docs/architecture/backend-type-dispatch.md`: record that the legacy
  `lower_type`/`protocol_llvm_type` path is retired and the casting graph is the
  single resolver; keep the historical tables as reference with a dated note.
- `docs/architecture/backend-contracts.md` §8 and the 2026-11-17 `BUGS.md`
  entry (the `Ptr`-handle bug) close with this work.
- Rationale comments at every deleted special case name *what pattern it
  targeted and why it is now unnecessary*; none may be silently dropped.

## Risks

- 149 call sites is a wide blast radius; the mitigation is that the *only*
  behavior that may legitimately differ is width on non-native targets, and the
  IR A/B plus the three gates above make that visible per site.
- The `"i64"` fallback also serves genuinely unknown types ("raw Bits(8) is
  always safe" per the design doc). The new resolver must keep a *target-aware*
  last resort (`i{int_bits}`), not delete the fallback.
