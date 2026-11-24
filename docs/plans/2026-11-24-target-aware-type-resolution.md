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

## Baseline (measured 2026-11-24, before any change)

Command, on this branch's tree with the debug compiler built from it:

```
BRIEVC=./target/debug/brievc bash scripts/cross_target_ir.sh wasm32-unknown-wasi
→ cross-target-ir: wasm32-unknown-wasi — 99 programs checked, 50 failed
```

**50 of 99 native-accepted programs cannot emit valid IR for wasm32.** The
`aarch64` arm of the same sweep is green (100/100): the conflict needs a 32-bit
target to appear, which is exactly why the sibling-Linux CI case cannot see it
and a wasm32 arm is the campaign's acceptance gate.

Mismatch direction (the declared-vs-produced width pair):

| Count | Direction | Reading |
|---|---|---|
| 43 | `i64 → i32` | a **signature/slot** expects the target width (i32); the **value** arrived as a boxed i64 |
| 5 | `i32 → i64` | the reverse: the slot is i64, the value came through a target-aware path |
| 2 | `ptr → i64` | a handle/pointer where a word was declared |

Offending instruction (first error per program, normalised):

| Count | Instruction |
|---|---|
| **25** | `%x = sext iN %x to iN` — a widening emitted at the wrong width |
| 6 | `%x = call iN @g(...)` — call argument/return width |
| 6 | `%x = inttoptr iN %x to ptr` — pointer conversion |
| 2 each | `getelementptr`, `icmp`, `store`, `mul` |

Inventory behind the dominant shape: **165 hardcoded `to i64` widenings** in the
LLVM backend (`helpers.rs` alone has ~15 `sext`/`zext`/`ptrtoint … to i64`), the
mechanical form of "i64 is the uniform box" against a target whose word is 32
bits.

**First slice (decided by this measurement, not by reading):** the 25
`sext/zext … to i64` producers, since they account for the bulk of the 43
`i64 → i32` failures. Each is a value crossing into a slot/call whose declared
width is already available at the site; the fix is to widen to *that* width (or
to `int_bits`, where the box genuinely is a machine word) and let the declared
side and the produced side agree — which is the plan's step 1 in miniature.

Failing programs, per-program diagnostics: `/tmp/opencode/codegen-bughunt/wasm32-baseline/`
(the same list is in the sweep output).

## Dominant remaining class — ROOT CAUSE, with the fix site (2026-11-24)

The chain, fully evidenced:

1. `bare_term_value_defn` (wasm32) emits
   `%t28 = call i64 @print_str(ptr %t30)` then `%t31 = sext i32 %t28 to i64`
   — a widening whose SOURCE type is `i32` for a value that is physically i64.
2. The widening comes from `adapt_to_i64` (`helpers.rs`), whose Int arm uses
   `self.llvm_type(ty)` as the source width and emits `sext` for a signed Int.
3. `llvm_type(Int)` is target-aware — **i32 on wasm32** — while the physical
   result of the print intrinsics is i64: `intrinsics.rs` emits
   `call i64 @print_int(...)` and then constructs
   `BTypedRegister { name: v, ty: Type::int() }` (`intrinsics.rs:4348`).
   Declared width (i32) ≠ physical width (i64) ⇒ invalid `sext`.

**The fix (general, and a no-op on 64-bit targets):** a register's declared
Briev type must describe its PHYSICAL width, so an i64-returning intrinsic's
result must not be typed as the flexible `Int` (which follows the target word).
On x86_64 `llvm_type(Int)` is already i64, so nothing changes there; on wasm32
the bogus `sext` disappears and `adapt_to_i64` correctly early-returns.

Scope: every intrinsic whose declared Briev result is `Int` but whose emitted
call returns i64. The print family is the repro (`intrinsics.rs:4326–4349`);
find the rest by the same shape — `call i64 @…` followed by
`ty: Type::int()` — rather than fixing the print family alone (that would be the
per-site patch slice 1 already showed to be insufficient: the error simply
moves to the next consumer).

Verification for the step (unchanged): `cross_target_ir.sh
wasm32-unknown-wasi` failure count must fall, aarch64 stays 0, and
`daily_use_smoke tier1_corpus_compiles_links_and_runs` stays green (plus
byte-identical native IR for the affected fixtures).

## Slice 1 (in progress, 2026-11-24)

## Slice 1 (in progress, 2026-11-24)

Measured, in this order (Rule 20 — the baseline above came first):

1. **First mismatch class is gone.** The raw `store i64 <i32 handle>` in the
   plain-global publish path is fixed: `emit_state_store_i64`'s plain branch,
   the plain-init publish, and the body-result publish now box a handle at the
   boundary through one helper (`box_handle_for_slot`, plus
   `convert_int_width`), using the documented rule "a handle is target-width in
   SSA, boxed at the slot". On a 64-bit target the widths already agree, so
   nothing new is emitted there.
2. **The error moved to the next class**: `%t0 = inttoptr i32 %t1 to ptr`
   stored into an i64 slot (a genuine `ptr` value where the box wants an
   integer). So the mismatch has (at least) two faces: a target-width *integer*
   handle (fixed: `zext`) and a genuine `ptr` (needs `ptrtoint`).
3. **Why the helper cannot yet decide.** `reg_value_ll` records widths for only
   ~6 registers, so the box helper must fall back to the target word width —
   and that is wrong for a value that is a genuine `ptr`. The blame is upstream
   of the box: `Malloc#(8) as Ptr<Int>` and `0 as Ptr<Int>` reach the store as
   **i32** in one place and **ptr** in another, i.e. the handle *producers* are
   not normalized. This is the same unit the 2026-11-17 entry describes ("the
   handle's width, `reg_llvm_ty`/`reg_value_ll`, and BOTH consumers must
   agree").

**Next slice:** normalize the handle producers — pick ONE SSA form for a handle
(either `ptr`, or its target-width integer) at the cast/`Malloc#`/call
boundaries and make every producer emit it, then the box helper's choice
becomes decidable instead of heuristic. The sweep is the oracle for every step:
no program moved from fail to pass yet, so slice 1 is a mechanism, not a win.

## Slice 2 — DONE and verified (2026-11-24)

`d0d7acde4` — a print-family call must return its callee's declared width.

| Measure | Before | After |
|---|---|---|
| wasm32 corpus (IR-invalid programs) | **50 / 99** | **25 / 99** |
| aarch64 arm | 0 | **0** (PASS) |
| native tier1 corpus gate (build+link+run+goldens) | ok | **ok** |
| native IR, 11 fixtures byte-compared | — | **identical** |
| subset of 18 fixtures | 4 passing | **10 passing** |

The defect: the seven print arms emitted `call i64 @sym` while the callee is a
Briev `defn` (`lib/std/cast_lanes.bv`, `-> Int`) emitted at the target word
(`define i32 @print_str` on wasm32) — the call disagreed with its own callee and
every consumer of the result emitted an invalid `sext i32 %v to i64`. The return
type now derives from the callee's declared signature (`print_call_ret_ty`); on
64-bit targets it equals the old literal, hence the byte-identical native IR.

**Next (slice 3):** the remaining 25, whose dominant shape is
`inttoptr i32 %x to ptr` where `%x` came from an **i64** box load — a
dereference site taking its width from `int_bits` while the value is boxed. The
codebase's dominant form is already `inttoptr i64` (13 sites), so these sites are
the anomalies; make them use the loaded value's width, then re-measure.

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
