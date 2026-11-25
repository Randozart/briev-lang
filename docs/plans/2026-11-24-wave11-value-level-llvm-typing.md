# Wave-11 — value-level LLVM typing: retire the width side tables (correctness-first)

**Status:** S0-S2 LANDED; S3-S4 are the funded continuation (not started).
**Trigger:** PR #164 moves wasm32 from 52 → 15 IR-invalid programs but regresses
`rbv_list_each_wasm`, and the regression is a *recurrence*, not a new bug.
**Assessment that motivated this:** `.opencode/plans/2026-11-24-164-assessment.md`.

## The correctness argument

The backend emits textual LLVM IR, so every instruction must state the exact
LLVM type of every operand. Today the emitter answers "what is this operand's
type?" from **five** places that can disagree:

| channel | what it holds | populated where |
|---|---|---|
| `TypedRegister.ty` | the **Briev** type | every expression emitter (624 construction sites) |
| `reg_llvm_ty: HashMap<String,String>` | "actual LLVM register widths … notably enum/Result state loads" (`context.rs:1460-1462`, 2026-10-27) | enum/state load paths |
| `reg_value_ll: HashMap<String,String>` | "ACTUAL LLVM register width for cast-step results" (`context.rs:1514-1520`, 2026-11-12), extended by #164's loads and narrowing | loads, casts, `inttoptr`, one arena site |
| `reg_float_cache` | float width per register | float paths |
| literal `i64` in format strings | — | **320 sites** (119 `call i64`, 111 `store i64`, 90 `inttoptr i64`) |

Two of those maps already record **the same fact** (`name → LLVM type`) under
different names, created three weeks apart for different symptoms. That is the
signature of a missing concept: the value cannot answer for its own type.

`TypedRegister` is `{ name: String, ty: Type }` (`mod.rs:617-620`) — no LLVM
type. Everything above is a workaround for that hole.

### Why this is a correctness problem, not a tidiness problem

A name-keyed type map is unsound by construction, because register names are
reused:

> `context.rs:1553-1563` (issue #119) — "`iter_chain_loop` starts again at `%t0`
> while the map still held `%t61 = ptr` from the previous body — so
> `canonical_ptr` passed an i64 handle through un-converted and the element GEP
> emitted `getelementptr i64, ptr %t61`, invalid IR."

#164 fixed the symptom of that by clearing the map on `reset_register_names`,
then reproduced the same shape through a different helper:

```rust
// helpers.rs:3628 — "a `ptr` record means: no conversion needed"
let hw = match self.fun.reg_value_ll.get(handle) {
    Some(w) if w == "ptr" => return handle.to_string(),
    _ => self.value_width_or_target_width(handle),
};
```

An *unknown* register resolves to `i{int_bits}` (safe), but a `"ptr"` record
resolves to *"this integer is a pointer"* (unsafe), and the record can be stale,
discarded (`restore_preserve_counters` restores `*fun = self.saved` wholesale
while keeping the counter monotonic — `context.rs:1902`), or set by a path
nobody audited. The regression is the third instance of the same disease.

The wider world does not do this, and says so:

- **LLVM**: the type lives on the value (`Value.h:118`, `getType()` `:257`), and
  the Programmer's Manual states *"names should ONLY be used for debugging …
  they should not be used to keep track of values or map between them."*
  ([PM](https://llvm.org/docs/ProgrammersManual.html#the-value-class))
- **Go**: *"The name of an SSA Value is not semantically significant, and may not
  even be unique within a function"*; side tables key on `ID`.
  ([ssa.go:129](https://github.com/golang/tools/blob/master/go/ssa/ssa.go#L129))
- **Cranelift**: type packed into `ValueDataPacked`, read by `dfg.value_type(v)`.
  ([dfg.rs:393](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/ir/dfg.rs#L393))
- **QBE**: the type is written at each definition (`%x =w add`).
  ([il.txt](https://c9x.me/qbe/doc/il.txt))
- **LLVM MIR** is the nearest exception and it is instructive: `VRegToType` is
  keyed by a **dense vreg index**, not a name, with an explicit
  `clearVirtRegTypes()` invalidation hook.
  ([MRI.h:807](https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/CodeGen/MachineRegisterInfo.h#L807))

Briev's own pillars point the same way: *"the casting graph is the single source
of truth"* for type→LLVM-type. A register-width map is legitimate only as a
cache of **what was emitted**, which means it must be total and authoritative
for every register or it must not exist.

## Design

Three steps, in dependency order, each independently verifiable:

1. **An instrument** — a textual emitted-IR checker that rejects a mistyped
   operand. Without it, every later stage is verified by "the linker didn't
   complain on this fixture", which is how three instances of this bug shipped.
2. **One channel** — merge `reg_llvm_ty` + `reg_value_ll` into a single
   `value_llvm_ty`, with the discipline that makes a name-keyed map sound *for
   one body*: cleared with the names, written by **every** width-producing
   operation (loads, casts, narrowing, `inttoptr`, box/unbox).
3. **The value answers** — `TypedRegister` carries `llvm_ty`; the map becomes a
   derivation fallback, then disappears.

The end state is the LLVM/Cranelift/Go/QBE shape: a conversion site reads
`val.llvm_ty` and cannot be wrong, and the 320 hardcoded `i64` literals become
`llvm_ty` (which is `i64` on hosts, so native IR stays byte-identical — the
repo's own safety net for refactors of this size).

## Invariants

Each is pinned by a named test. Native IR byte-identity is the standing
invariant for every stage: this work must change **only** what happens when the
target word ≠ the hardcoded literal.

1. **I-1 (instrument exists and is green).** Every module the compiler emits
   passes `mistyped_operands` — no `store ptr`, `getelementptr`, `inttoptr`,
   `insertvalue`, or `call` may carry an operand whose declared IR type differs
   from the register's definition type. Pinned by
   `ir_verify_mistyped_operands_detects_the_164_shape` (a literal module with
   `%t11 = trunc … to i32; store ptr %t11, ptr %x`) and
   `ir_verify_mistyped_operands_accepts_a_clean_module`, plus a corpus sweep that
   it reports zero on `main`.
2. **I-2 (one channel).** No code path reads `reg_llvm_ty` when
   `value_llvm_ty` has an entry, and there is exactly one writer API. Pinned by
   a unit test that writes through the API and asserts both old names are gone
   (`grep`-style assertion over `src/backend/llvm/`).
3. **I-3 (total at conversion sites).** `pointer_handle_of` records the width of
   its own result, and `convert_int_width` records its result's width — so an
   inverse operation can never read a stale or absent record for a register it
   just produced. Pinned by
   `narrowing_records_the_result_width` (emit and assert the map entry).
4. **I-4 (no unsafe default).** `pointer_from_handle` never infers "already a
   pointer" from a record alone; the caller's Briev type decides, because a
   `Ptr<T>`/`String` value *is* a pointer and an `Int` handle is a word. Pinned
   by `pointer_conversion_is_decided_by_the_briev_type`.
5. **I-5 (no round-trip).** A pointer that is already a pointer is stored
   directly: no `ptrtoint` → `trunc` → `inttoptr` bracket around a store. Pinned
   by `storing_a_pointer_does_not_round_trip` (assert the emitted body has no
   `ptrtoint` for a `String` field store).
6. **I-6 (#164's win, safely).** wasm32 IR-invalid count is **≤ 15** on the
   corpus after stage S2 and **non-increasing** at every later stage. Pinned by
   `cross_target_ir.sh wasm32-unknown-wasi` recorded in BUGS.md per stage.
7. **I-7 (native untouched).** Emitted `.ll` for the native triple is
   byte-identical to `main` on a ≥12-fixture sample after **every** stage.
   Pinned by the determinism harness plus a byte-compare script run per stage.
8. **I-8 (the regression is gone).** `tests/rbv_list_each_wasm` passes, and the
   specific shape (`store ptr <non-ptr>`) is rejected by I-1's checker rather
   than by a fixture. Pinned by that test plus I-1.
9. **I-9 (`TypedRegister.llvm_ty` is derived, never guessed).** After S3, a
   register's `llvm_ty` comes from the value's Briev type through the casting
   graph, or is set explicitly by the operation that produced it; no site may
   leave it empty. Pinned by a `debug_assert!` in the constructor plus
   `every_typed_register_carries_an_llvm_type` over the corpus.
10. **I-10 (the maps are gone).** After S4, `reg_llvm_ty` and `reg_value_ll` do
    not exist. Pinned by a grep-style test in `src/backend/llvm/tests.rs`.

## Stages

### S0 — the instrument (~half a day)

Add `mistyped_operands(&str) -> Option<String>` to `src/ir_verify.rs`, alongside
`undefined_call_targets` (`:198`) and `mismatched_call_signatures` (`:279`), and
call it in `verify_llvm_ir` (`:41`). One linear pass: build `%name → type` from
definition lines, then check each use site's declared type against it.

It must report **zero** on today's tree; if it reports anything, that is a real
defect and it gets fixed (or recorded in BUGS.md as baseline) before S1 proceeds.
Ship alone: it is pure value even if the rest slips.

### S1 — one channel (~1 day)

Merge `reg_llvm_ty` into `value_llvm_ty` (keep `reg_value_ll`'s richer
doc-comment, extended with #164's findings), one writer (`record_value_width`),
one reader (`value_width`). Clear it wherever names are cleared
(`reset_register_names`, `clear_locals` — both already do). Make the inlined-body
restore path **carry** the map forward explicitly rather than discarding it
(`restore_preserve_counters` keeps counters monotonic; the map must be monotonic
for the same reason, since names are). Native IR byte-identical.

### S2 — remove the unsafe default, land #164's win (~1 day)

- `pointer_from_handle` takes the value's Briev type; `"ptr"` is no longer a
  licence to skip conversion (I-4).
- `pointer_handle_of` / `convert_int_width` record their results (I-3).
- Delete the `ptr→i64→i32→ptr` bracket at `emit_expr.rs:4898-4904`,
  `emit_stmt.rs:3901`, `:4059` for values that are already pointers (I-5).
- Fix the two unpatched `store ptr` arms that hardcode `inttoptr i64`
  (`emit_expr.rs:7494-7500` and the parallel arms at `emit_stmt.rs:3903`/`:4061`),
  or delete them in favour of `emit_non_scalar_store`.

Expected: wasm32 ≤ 15, `rbv_list_each_wasm` green, native IR identical.

### S3 — the value carries its type (~3-4 days, mechanical)

- `TypedRegister { name, ty, llvm_ty }` with a constructor
  `TypedRegister::new(name, ty, backend)` deriving `llvm_ty` from the Briev type
  through the casting graph — so the 624 existing sites migrate by construction,
  and only operations that produce a non-derivable width (boxed handles, narrowed
  words, loaded slots) set it explicitly.
- Convert consumers in dependency order: pointer/handle conversion → call
  argument adaptation → store sites → the 320 hardcoded literals.
- The map stays, consulted only where `llvm_ty` is genuinely unknown, and an
  assertion fires in debug when it is consulted for a value that had one.

### S4 — delete the mechanism (~1 day)

Remove `reg_llvm_ty` / `reg_value_ll`, `narrow_handle_to_target_width`'s guessing,
and the remaining hardcoded `i64`s. I-10.

## Measurement protocol (per stage)

Same discipline as wave-10, which is what made it safe:

```bash
bash scripts/gate.sh quick                              # per stage
bash scripts/cross_target_ir.sh wasm32-unknown-wasi     # I-6
bash scripts/cross_target_ir.sh aarch64-unknown-linux-gnu
bash benchmarks/ll_determinism_check.sh --quick         # I-7
# + byte-compare native .ll against main on ≥12 fixtures (the wave-10 method)
bash scripts/gate.sh full                               # before landing
```

Record the wasm32 number in `benchmarks/results/` and BUGS.md per stage; a stage
that raises it is reverted, not explained.

## Undo path

Per stage commit. S0 is additive (a new check). S1 is a rename plus a writer
API. S2 is the behavioural change and is guarded by I-1/I-8 — if it cannot get
to ≤15 without a regression, it is reverted and #164 stays out. S3/S4 are pure
refactors gated on byte-identical native IR, so a revert restores the maps
exactly.

## Doc maintenance (Rule 13)

- `docs/architecture/backend-type-dispatch.md` gains the value-level rule: *a
  value's LLVM type is a property of the value, not of its name*; widen the
  "single source of truth" paragraph to cover widths.
- `docs/architecture/backend-architecture.md`: the emission invariants section
  gets I-1 (mistyped operands) as a named invariant with its check.
- `src/ir_verify.rs` header comment: a third entry in the "checks LLVM does not
  perform" list, with the #164 shape as the motivating example.
- `context.rs`: the `reg_value_ll` doc comment's #119 history is **preserved and
  extended** (never deleted) — it is the reason this wave exists.
- BUGS.md: the 2026-11-24 wasm32 entry gains the per-stage numbers; the #164
  regression entry is closed by S2 with a pointer to the check that now guards it.

## Risks

1. **S3's blast radius (624 sites) is where a refactor this size goes wrong.**
   Mitigation: the constructor derives, so sites migrate without being touched;
   stage it by consumer group; byte-identical native IR per group.
2. **The checker could be expensive on the compile path.** Mitigation: it is one
   linear text pass, the same class as the two checks beside it; measure the
   compile-time delta on the corpus and, if it matters, gate it behind the
   existing verifier switch.
3. **The map may be load-bearing in ways only the wasm32 corpus reveals.**
   Mitigation: I-6 is monotone and measured, and S2 lands only if it holds.
4. **#164's author is working this area.** Mitigation: S0/S1 do not touch the
   width semantics, so they can land underneath the PR; S2 supersedes it, and the
   PR comment should say so before S2 starts.

## What this does to #164

Adopt its intent, not its patch: S2 reimplements "consult the recorded width"
with the unsafe default removed and the record made total, which is the version
that keeps the 52 → 15 win without the recurrence. Close #164 as superseded with
a pointer to S2, crediting the diagnosis (it found the class) while recording
that the mechanism is what the wave replaces.


## Outcome so far (2026-11-25)

| stage | result |
|---|---|
| **S0** | LANDED `fbd77a891`. \`mistyped_operands\` in \`src/ir_verify.rs\`, six tests, wired into \`verify_llvm_ir\`. **Zero findings** on the tier1 corpus and on 74 tier2/example programs. |
| **S1** | LANDED `f2025f143`. One \`value_llvm_ty\` channel, one writer (\`record_value_width\`), one reader (\`value_width\`), restore carries the channel forward. **wasm32 52 → 27.** Native IR byte-identical (12 fixtures). |
| **S2** | LANDED \`0d260825f\`. \`convert_int_width\` records its own result (I-3); \`pointer_from_handle\` derives \`inttoptr\`'s source width from the record instead of the target word. **wasm32 holds at 27.** Native IR byte-identical. |
| **S3** | NOT STARTED. |
| **S4** | NOT STARTED. |

### What S0 cost, and why it was worth it

Getting the checker to zero found four real parsing subtleties, each now pinned by
a test — and one of them was the #119 lesson applied to the checker itself:

1. The definition table must be scoped **per function** (names restart per body).
   Before scoping it was a false positive on every multi-function module.
2. Fast-math flags precede the type (\`fadd fast float\`).
3. Slicing \`word.len()\` off a string that still has a leading space eats the
   word's first letter: \`" fast float"[4..]\` is \`"t float"\`, parsed as type \`t\`.
4. \`select\` returns its VALUE type (after the first comma), not the condition's
   \`i1\`.

### Why S2 did not reach <=15, measured

The remaining 27 failures classify as: 7 from hardcoded \`inttoptr i64\` emission
sites, 6 call-argument width mismatches (\`call i32 @briev_getenv_int_impl(i32 …)\`
against a \`ptr\` parameter), and a tail of arithmetic widths
(\`mul i32\`, \`icmp slt i32\`, \`call ptr @int_to_str(i32 …)\`).

There are **103 hardcoded \`inttoptr i64\` sites** in the backend (and 320 hardcoded
\`i64\` emissions overall). Patching the seven the corpus happens to reach is
exactly the shape of fix that produced three instances of this bug — which is the
whole thesis of this wave. The tail belongs to S3, where the value carries its
own type and the literal is derived rather than chosen.

### I-4 remains open by design

S2 kept \`pointer_from_handle\`'s \`"ptr"\` pass-through and documented the
precondition it now depends on (every producer records; a narrowed register
records the integer width it was narrowed to). Making the *Briev type* the
decider — I-4 as written — belongs with S3, because the type is what
\`TypedRegister\` will carry. Recording it here so the invariant is not read as met.
