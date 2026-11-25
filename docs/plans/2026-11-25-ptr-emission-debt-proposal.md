# Pointer/width emission debt — research and proposal

**Date:** 2026-11-25
**Question (owner):** "we have a great deal of fixes to do regarding ptr emission. What is your proposal?"
**Status:** research + proposal. Nothing implemented.
**Ground truth:** `main = 44fc3a7e8` (wave-11 S0–S2 landed, CI 4/4, wasm32 52 → 27).

> **ADDENDUM (owner, 2026-11-25): wasm32 is a SUPPORTED target** — it is the web
> build of Briev, i.e. what `.rbv` compiles to. This changes the framing from
> "probe that finds width bugs" to "shipped surface". §13 records the
> measurements that follow from it, §14 the revised priority order.

---

## 1. TL;DR

The debt is not "103 fixes". It is **one chronic question answered in three different places** — "what is this value's type, and how wide is a word?" — plus **one unowned lifetime** that makes the answer go stale.

Three numbers decide the design:

| measurement | count | what it means |
|---|---|---|
| hardcoded `inttoptr i64` | **103** (102 literal, 1 parametrised) | the literal debt |
| inline `i{}", ctx.int_bits` | **65** | the same fix, done correctly by hand |
| places that reset `txn_counter` | **14** | namespace restarts |
| places that clear `value_llvm_ty` | **1** | the channel survives 13 of them |
| writers of the width channel | **11** | small enough to make *sound* |
| readers of the width channel | **12** | |
| `TypedRegister` literals | **458** (96 factory fns) | already carry the Briev type |
| hardcoded `i64` total | **320**, of which **175** are `briev_*` runtime ABI | 175 are correct |

**Proposal in one line:** stop letting the emitter *write* a type it can *ask for*, and stop letting a name-keyed channel outlive the namespace that keys it. Four moves, ~4 days, no IR change until the last step.

| move | what | cost | risk |
|---|---|---|---|
| **M1** | make the register namespace one owned object; a reset cannot orphan the width channel | ½ day | very low — no IR change |
| **M2** | the value answers for its type: derive from `TypedRegister.ty` via the existing casting graph; explicit overrides live in the same object | ½ day | low |
| **M3** | one conversion helper; 102 `inttoptr i64` sites → 1; then `cross_target_ir.sh wasm32` drives the tail to 0 | 2 days | medium (that is what the gates are for) |
| **M4** | a ratchet: CI fails on a *new* width literal, with a curated allowlist documenting the 175 legitimate ones | ½ day | very low |

The correction to my own wave-11 S3 plan is in §5: it proposed adding an `llvm_ty` field that would be a **second cache with the same staleness hazard it was meant to cure**. The type is already there; derive it.

---

## 2. What is actually broken — three defects wearing one costume

### D1 — The literal (mechanical, 102 sites)

```
$ grep -rn 'inttoptr i64' src/backend/llvm/ | wc -l
103
$ grep -rn 'inttoptr i{' src/backend/llvm/ | wc -l
1        # src/backend/llvm/tuple_values.rs:33
```

Distribution: `emit_expr.rs` 42, `intrinsics.rs` 23, `emit_stmt.rs` 11, `tests.rs` 9, `mod.rs` 7, `helpers.rs` 4, `emit_toplevel.rs` 4, `loop_engine/counter.rs` 3.

The same literal is emitted correctly **by hand** in 65 places (`format!("i{}", self.ctx.int_bits)`). So the convention is not unknown — it is *repeated manually and missed 103 times*. That is the definition of a missing abstraction, not of 103 independent mistakes.

### D2 — The channel outlives its namespace (structural, the actual bug class)

`value_llvm_ty` is keyed by register **name**. The namespace that names it is `txn_counter`, which is reset in **14** places:

```
src/backend/llvm/context.rs:1577, 1829      (reset_register_names, reset)
src/backend/llvm/dispatch.rs:501
src/backend/llvm/emit_toplevel.rs:5400, 8757, 8823, 8959, 8989
src/backend/llvm/mod.rs:5982, 6642, 6669, 6694, 6713
src/backend/llvm/loop_engine/…              (counter restarts)
```

Only `reset_register_names` (`context.rs:1577-1580`) also clears the map:

```rust
pub fn reset_register_names(&mut self) {
    self.txn_counter = 0;
    self.value_llvm_ty.clear();
}
```

`reset()` (`context.rs:~1829`) sets `txn_counter = 0` and **does not** clear the map. That ratio — 14 restarts, 1 clear — is the defect stated as a number. Every instance of this bug class has come from it:

- **#119** (`context.rs:1553-1566`): `%t61 = ptr` from a previous body applied to a new body's `%t61` → `getelementptr i64, ptr %t61`.
- **#163 / #164 / the wave-11 S2 regression** (mine, caught by S0 on the first program it saw): `store ptr %t11` where `%t11 = trunc i64 %t10 to i32`.

The channel is **11 writers** and **12 readers** — small. It does not need to be replaced by a 458-site migration; it needs an owner.

### D3 — There is no machine that counts (process)

Rule 15 and Rule 19 are conventions with no enforcement. `cross_target_ir.sh` (from #161) tells you *how many* programs fail, `mistyped_operands` (S0) names *which line*. Neither forbids a **new** literal. So the debt regenerates faster than it is paid.

---

## 3. What comparable backends do

| system | who answers "this value's type" | who answers "target word width" | warns about caller-supplied/remembered types? |
|---|---|---|---|
| **Cranelift** | `DataFlowGraph::value_type()` reads a 14-bit packed type off `ValueData` — `cranelift/codegen/src/ir/dfg.rs:393`, layout `:729`; `values: PrimaryMap<Value, ValueDataPacked>` `:142` | `TargetIsa::pointer_type()` → `Type::int(pointer_bits)`, `isa/mod.rs:473-478`; `TargetFrontendConfig` `:267` | **Yes** — builder methods take *no* type: "The controlling type variable will be inferred from the input values if possible" (`cranelift/codegen/meta/src/gen_inst.rs:1120`, inference at `:1269`). `Value` is a `u32` handle (`entities.rs:66`); `vNN` exists only for printing/parsing. |
| **QBE** | the **definition** carries it: `Ins.cls` (`all.h:235`), `Phi.cls` (`:242`), `Tmp.cls` (`:345`) | no pointer type at all — pointers are "typed by an integer type sufficiently wide" (`doc/il.txt:142`) | **Yes**, and it *rejects* the stale case: `typecheck` errors with `"temporary %s is assigned with multiple types"` (`parse.c:837`). |
| **Go** | `Value.Type *types.Type` (`ssa/value.go:34`), and `"The ID and Type fields must not be modified"` (`:22`) | `Config.PtrSize` "4 or 8" (`ssa/config.go:28`); `SetTypPtrs()` derives `IntPtr`/`TINT` once from `PtrSize` (`:170`; `types/size.go:306`) | — types are immutable once built, so there is nothing to remember. |
| **rustc** | `TyAndLayout` (`rustc_abi/src/layout/ty.rs:77`, `size`/`align` `lib.rs:2273`) | `SimpleCx::new(..., pointer_size)` builds `isize_ty` from `pointer_size.bits()` **once** (`rustc_codegen_llvm/src/context.rs:897-904`); `type_isize()` (`type_.rs:209`); `type_int` from `target.c_int_width` (`rustc_codegen_ssa/src/traits/type_.rs:56-63`) | — derivation, not memory. |
| **Zig** | `Type.intInfo` | `usize`/`isize` → `target.ptrBitWidth()` (`src/Type.zig:2288-2298`, used `src/codegen/llvm.zig:3096`, defined `lib/std/Target.zig:2873`) | — derivation. |
| **LLVM (as a consumer)** | `IRBuilder::CreateIntToPtr(V, DestTy)`: the *source* type comes from `V`, only the destination is the caller's choice | `DataLayout::getIntPtrType` | partly — see §4. |

**The convergent rule: derive, never remember; and when you must remember (a representation override), the memory must live inside the object that owns the namespace.**

Three of five systems (Cranelift, rustc, Zig) have **no lint** banning width literals — because their codegen *cannot* express a width literal without going out of its way. The lint is the second-best fix; making the literal unnecessary is the first. We need both here, because Briev's emitter writes text.

---

## 4. The failure mode nobody's verifier catches

LLVM's `Verifier` catches `"Call parameter type does not match function signature!"` (`lib/IR/Verifier.cpp:4302`); Cranelift verifies block/SSA/CFG/types (`cranelift/codegen/src/verifier/mod.rs:1-57`); MLIR explicitly targets "detecting bugs in compiler transformations" (`mlir/lib/IR/Verifier.cpp:10-19`). But LangRef concedes the verifier "enforces only the structural rules… target-specific… validation is performed by the corresponding backend".

That is exactly our situation: **emitted IR for x86_64 is self-consistent** — `inttoptr i64 <i64>` is perfectly valid there. The bug only exists relative to *another* target's word width. This is why:

- 0 of the wasm32 bugs ever fired on the native path;
- the corpus gate (4632 tests) is green while 27% of the wasm32 corpus is invalid;
- `mistyped_operands` (S0) finds nothing on native — it is a *relative* check that only bites when the operand's declared type contradicts its definition, which on a 64-bit target the word-width literal never does.

**Consequence for the proposal:** correctness here cannot be established by "the tests pass". It needs a second target in the loop (`cross_target_ir.sh wasm32-unknown-wasi`, already wired into CI by #161) plus a check that names the cause. `aarch64-unknown-linux-gnu` is already at **0/100** failures — the debt is specifically *32-bit-pointer targets*.

---

## 5. Correction to my own wave-11 plan

`docs/plans/2026-11-24-wave11-value-level-llvm-typing.md` S3 says: *"give `TypedRegister` its LLVM type (624 sites migrate by construction)"*.

**That is the wrong shape, for three reasons.**

1. **The type is already there.** `TypedRegister { name: String, ty: Type }` (`mod.rs:617-620`) carries the Briev type at all 458 construction sites. `CastingGraph::resolve_llvm_type(universe, ty, int_bits) -> String` (`src/casting/graph.rs:1533`) already maps it to an LLVM spelling, target-aware (`Ptr` → `ptr`, `Bits(n)` → `iN`, `WidthParametric` → `i{bits|int_bits}`). So `llvm_ty` is *derivable*; storing it would be a **second cache of a derivable fact, with the same staleness hazard it was meant to cure** — the exact mistake S1 made and S2 had to retreat from (§D2).
2. **The cache is sometimes genuinely necessary, and that is the real design question.** The emitter deliberately deviates from the declared Briev type: a `String` is emitted as a boxed word handle, not `ptr`. So "derive from `ty`" alone would be wrong at exactly those sites. The correct model is *representation*: a value has a source type **and** an optional representation override, and Cranelift/rustc model it the same way (`ValueDataPacked.type` set at definition; `TyAndLayout`).
3. **458 mechanical edits is a week of risk for a fact that can be derived in a one-line method.** The migration is not where the value is.

What survives from S3 is the *goal* — the value answers for its own representation — reached by asking rather than storing.

---

## 6. The proposal

### Thesis

> **The emitter should be unable to write a type it did not ask for.**

Three authorities already exist in this codebase; nothing new needs inventing:

| question | authority | location |
|---|---|---|
| how do I spell this Briev type for this target? | `CastingGraph::resolve_llvm_type(universe, ty, int_bits)` | `src/casting/graph.rs:1533` |
| how wide is a word here? | `ctx.int_bits` | `context.rs:527` |
| what type is this value, really? | `TypedRegister.ty` + representation override | `mod.rs:617-620` |

Every one of the 102 literals is the emitter answering a question that belongs to one of those three.

---

### M1 — The namespace owns the channel (½ day, no IR change)

Replace the two loose fields with one object:

```rust
pub struct Registers {
    counter: u64,
    repr: HashMap<String, Repr>,   // the width channel, keyed by name — but
}                                  // the ONLY keys that can exist are ones
impl Registers {                   // this counter minted.
    pub fn reset(&mut self) { /* both, together, always */ }
    pub fn mint(&mut self, ty: &Type) -> Reg;
    pub fn llvm_ty(&self, reg: &Reg) -> String;
}
```

- `txn_counter` and `value_llvm_ty` become private fields of one struct; both live behind `Registers`.
- All 14 reset sites become `self.fun.regs.reset()`. The 13 that currently leak cannot compile without meaning "reset both".
- **Verification:** `native IR byte-identical on 12 fixtures` + full gate. No behavioural change is *intended*, which makes it a pure test of the refactor.
- **Why first:** M3's helper will trust the channel. Making it trustworthy before anyone trusts it more is the only ordering that doesn't repeat #164.

This mirrors Cranelift (`PrimaryMap<Value, ValueDataPacked>` — the map and the namespace are the same object) and the standard slotmap/generation-counter answer for stale side tables.

### M2 — The value answers for its representation (½ day)

```rust
impl TypedRegister {
    /// The LLVM spelling of this value's emitted representation.
    fn llvm_ty(&self, ctx: &LlvmContext) -> String {
        ctx.fun.repr_override(&self.name)                       // boxed handle, etc.
            .unwrap_or_else(|| ctx.casting_graph
                .resolve_llvm_type(&ctx.universe, &self.ty, ctx.int_bits))
    }
}
```

- Zero construction-site edits: all 458 sites keep `{ name, ty }`.
- The 11 writers of the override stay, but they now *record a deviation* rather than *the type* — which is honest, and makes each one reviewable: "this value is a boxed word handle although its type says `String`".
- **Verification:** a test that, for the tier1 corpus, `llvm_ty()` agrees with the recorded override wherever one exists, and that every override is one of a small declared set (`ptr` / `i{int_bits}` / `i32` / `double`). Any override outside that set is a code smell and should fail.

### M3 — One conversion, 102 sites → 1 (2 days)

```rust
/// Convert a word-typed handle to `ptr`, asking the value for its own width.
fn word_to_ptr(&mut self, out: &mut String, indent: &str, v: &TypedRegister) -> String
```

- Emits `inttoptr <v.llvm_ty()> <v.name> to ptr`, or nothing when `v.llvm_ty() == "ptr"`.
- The 102 literal sites become calls; `tuple_values.rs:33` (already correct) becomes the helper.
- Then the *tail* is driven by measurement, not grep: `cross_target_ir.sh wasm32-unknown-wasi` after each slice. Expected order of value: the 7 `inttoptr` failures → the 6 call-argument mismatches (`call i32 @briev_getenv_int_impl(i32 …)` against a `ptr` parameter — these need the `adapt_arg_to_param` treatment already used for float params) → the arithmetic tail.
- **Prerequisite, cheap and high-value:** extend `mistyped_operands` (`src/ir_verify.rs`) from `store`/`load`/`gep`/`inttoptr` to **calls, arithmetic, `icmp`, `select`, `ret`**. 6 of the 27 remaining failures are call-argument mismatches that today's check *cannot see*; without this, M3 is still grep-driven. With it, every wasm32 failure reports a named line and both definitions, which is what made S2's regression a 5-minute diagnosis instead of an afternoon.
- **Verification per slice:** wasm32 count strictly decreasing and never increasing; native IR byte-identical; full gate.

### M4 — The ratchet (½ day)

Notion's `eslint-seatbelt` shape is the proven design: per-file, per-rule counts in a checked-in file, CI fails when a count *increases*, auto-decrement when it drops.

```
config/width-literals.toml      # per-file budgets, shrinking
tests/backend_width_ratchet.rs  # fails on increase; prints the diff
```

- `i64` counting rules, with the honest allowlist:
  - **allowed**: `briev_*` runtime C ABI widths (**175 of the 320** — these are genuinely fixed, and the allowlist documents *why*);
  - **allowed**: `double`/`float` bit patterns, `%state` layout;
  - **counted**: everything else, per file.
- The point is not zero. The point is that the number can only go down, and a new literal fails CI with the line that added it.

Why a ratchet rather than a hard ban: 320 literals include 175 legitimate ones, so a blanket ban is wrong; and a hard ban with a big allowlist rots (published guidance is consistent on this — bounded, file-scoped allowlists that require review, never one growing list). The measure-then-shrink form is what lets M1–M3 be interleaved with other work without the debt growing back.

---

## 7. Alternatives, and why not

| alternative | why rejected |
|---|---|
| **A. 458-site `TypedRegister.llvm_ty` migration** (my own S3) | stores a derivable fact; introduces a cache with the same staleness hazard it cures; a week of risk for no correctness the derivation doesn't give. §5. |
| **B. Make register names module-unique (never reset the counter)** | *would* make the name-keyed map sound, and is the smallest possible fix — but it churns every asserted IR string in `tests.rs` (9 `inttoptr` expectations alone) and kills the readability of per-function `%t0`. Rejected as the *first* move; it remains available as a belt-and-braces after M1 if the channel ever proves unreliable again. |
| **C. Patch the 27 failing programs individually** | this is the shape of fix that produced three instances of the bug (#119, #163, #164). The corpus reaches 27 sites; the backend has 102. Patching what the corpus sees is how the tail stays alive. |
| **D. Add wasm32 to the default release path / make it a supported target now** | sequencing. 27% of the corpus is invalid there; "supported" would mean claiming something untrue. M3 makes 0 the acceptance criterion, and *then* it can be supported. |
| **E. Textual post-pass that rewrites `i64`→`i32` in emitted IR** | solves the symptom at the worst layer: it would "fix" the 175 legitimate runtime-ABI widths too, silently corrupting calls into `briev_*`. Rejected outright. |

---

## 8. What could go wrong (failure modes to design against)

1. **M1 hides a real dependency.** Some emitter may rely on the map surviving a counter reset *legitimately* (e.g. a register named before a body restart and used after). The byte-identical IR requirement catches this immediately — that is why M1 is first and IR-neutral.
2. **M2's derivation disagrees with the override at a site that "worked".** Expect a handful. Each disagreement is a genuine finding about representation (usually a handle emitted as a word), and the fix is an override, not a special case in `llvm_ty()`.
3. **M3's helper is called with a `TypedRegister` whose `ty` is coarse** (`Type::Void`, or a `Custom` under no seeded protocol). `resolve_llvm_type` falls back to `int_bits` — safe but silent. Mitigation: count and report those in the ratchet, and treat "unknown representation on a value that reaches a `ptr` context" as a WARNING with the site named.
4. **The owner's rule-24 test.** If the fix works, a *new* program should reach correct emission on every target with no compiler change. The acceptance criterion is written that way in §9.
5. **Ratchet fatigue.** A count file that nobody looks at becomes a rubber stamp. Mitigation: the ratchet test prints the current total and the delta in its failure message, and the plan records the number in BUGS.md at each landing so it trends visibly.

---

## 9. Acceptance criteria

Done means all four, simultaneously:

1. `bash scripts/cross_target_ir.sh wasm32-unknown-wasi` → **0 failed** (from 27), and `aarch64-unknown-linux-gnu` stays **0**.
2. `cargo test --lib -- ir_verify` green with `mistyped_operands` covering **calls, arithmetic, icmp, select, ret** — zero findings on the tier1 corpus and 74 tier2/example programs.
3. Native IR **byte-identical** on the 12-fixture baseline at every step (M1–M3 are IR-neutral; only M3's tail may legitimately change wasm32 output).
4. `tests/backend_width_ratchet.rs` green at a **lower** count than today's baseline, with the 175 runtime-ABI widths named as allowed in `config/width-literals.toml`.

Plus the rule-24 check: write one new program *after* M3 and compile it for both targets with zero compiler changes.

---

## 10. Open questions for the owner

1. **Is wasm32 a target we intend to support, or a probe we use to find width bugs?** The answer changes the framing: support means the acceptance test in §9 is a release gate; probe means it is a lint. (Current evidence says probe — but the debt it found is real either way, so M1–M2 are justified under both answers.)
2. **M1 or B first?** M1 is the insensitive move (no IR change); B (module-unique names) is the smallest possible fix but churns asserted IR strings. My recommendation is M1, with B held in reserve.
3. **Do the 175 `briev_*` widths belong in the allowlist, or should the runtime ABI itself become target-parametric?** Today the C runtime's `long long` is a fixed 64 bits, which is correct on both targets. If the runtime ever ships a 32-bit build with `int`-based widths, the allowlist entries become bugs. Worth one decision now rather than one discovery later.
4. **Should the ratchet count *lines* or *occurrences*?** Occurrences per file is stricter and matches Notion's design; lines are noisier but survive reformatting better.

---

## 11. Citations

**Local** (`main = 44fc3a7e8`):
`src/casting/graph.rs:1533` (the type authority) · `src/backend/llvm/mod.rs:617-620` (`TypedRegister`) · `src/backend/llvm/context.rs:527` (`int_bits`), `:1553-1566` (#119 record), `:1577-1580` (`reset_register_names`), `:~1829` (`reset`, clears the counter and not the map) · `src/backend/llvm/helpers.rs:3603` (`pointer_from_handle`), `:3623` · `src/backend/llvm/tuple_values.rs:33` (the one correct `inttoptr`) · `src/backend/llvm/emit_expr.rs:1197-1960` (literal sites) · `src/ir_verify.rs` (`mistyped_operands`, S0) · `docs/plans/2026-11-24-wave11-value-level-llvm-typing.md` (the plan this corrects) · `BUGS.md` (2026-11-25 entry).

**Upstream**, read during this research pass:
`cranelift/codegen/src/ir/dfg.rs:142,393,547,729` · `cranelift/codegen/meta/src/gen_inst.rs:1120,1269` · `cranelift/codegen/src/isa/mod.rs:267,473-478` · `cranelift/codegen/src/entities.rs:66` · `cranelift/codegen/src/verifier/mod.rs:1-57` · QBE `all.h:80-92,208-214,235,242,345`, `parse.c:799,837`, `doc/il.txt:142` · Go `src/cmd/compile/internal/ssa/value.go:22,34`, `config.go:28,170`, `types/size.go:306`, `ssagen/ssa.go:~20xx` · rustc `compiler/rustc_codegen_llvm/src/context.rs:897-904`, `src/type_.rs:209`, `rustc_codegen_ssa/src/traits/type_.rs:56-63`, `rustc_abi/src/layout/ty.rs:77`, `lib.rs:2273-2274` · Zig `src/Type.zig:2288-2298`, `src/codegen/llvm.zig:3096`, `lib/std/Target.zig:2873` · `lib/IR/Verifier.cpp:4302` · `mlir/lib/IR/Verifier.cpp:10-19`.

**Web:** LLVM Programmer's Manual, "Achieving isolation with LLVMContext" (context-owned uniquing; entities in different contexts cannot interact) · rustc `Symbol::from_u32` docs ("Avoid this except for… deserialization… Use `intern` instead") · Notion engineering, `eslint-seatbelt` (per-file/per-rule count file, auto-decrement, CI blocks increases) · M. Fowler, *ParallelChange* (2014) · Google *Software Engineering at Google* ch. 22 (large-scale change tooling) · `slotmap` generation-counter docs (the textbook stale-index fix) · A. King, *Parse, don't validate*; Y. Minsky, *Effective ML* (make illegal states unrepresentable).

---

## 12. Where this goes next

The four moves are independently landable in the order M1 → M2 → M3 → M4, each with its own commit, quick gate, and native-IR byte-identity check; only M3 touches emitted output, and only on 32-bit targets.

---

## 13. ADDENDUM — wasm32 is shipped, so what is the exposure today?

Measured after the owner's answer:

| surface | programs | wasm32 result |
|---|---|---|
| **`.rbv` — the shipped web path** (`examples/` + `tests/`, excl. archive) | **29** | **29 pass, 0 fail** |
| `.bv` tier1+tier2 — the compiler feature corpus | **100** | **73 pass, 27 fail** |

So the debt is **latent on the web path, active on the feature corpus**. The 27
failures live in arena/collection paths, boxed string handles, `GetEnvInt#`-style
runtime bounds — surfaces the current `.rbv` examples do not exercise. The risk is
that this is one feature away, not that it is broken now.

**The reason it is one feature away, and the actual governance defect:**

1. `scripts/cross_target_ir.sh` sweeps **only** `tests/tier1/*.bv tests/tier2/*.bv`
   (`:47`). **No `.rbv` program is in any cross-target sweep.** The shipped web
   path has never been IR-verified against its own target by a broad gate.
2. wasm32 is **not in PR CI**. `.github/workflows/verification-nightly.yml:11` lists
   "wasm32 cross-backend corpus gate" under *"Queued for later nightly expansion
   (harnesses not yet written)"* — it does not exist.
3. The sweep's own header (`:25-28`) documents wasm32 as "a DIFFERENT capability
   surface… pass them explicitly to inspect a migration, but they are
   intentionally not the default gate". That assumption was reasonable when wasm32
   was aspirational. It is now **wrong**, and the comment must be rewritten rather
   than left to mislead the next reader.
4. Coverage is instead 5 bespoke tests: `tests/test_wasm.rs`,
   `tests/wasm32_parity.rs` (2 fixtures in `tests/fixtures/wasm_parity/`),
   `tests/rbv_list_each_wasm.rs`, `tests/rbv_obj_init_wasm32.rs`,
   `tests/tuple_values_wasm.mjs`.

**A supported target has the weakest gate in the project.** That is why the debt
exists, and it is why the fix must start with the gate: without it, M1–M3 can be
re-broken invisibly, which is exactly what happened once already (#119 shipped
silently on 64-bit).

## 14. Revised priority order (given a shipped target)

| priority | item | why now |
|---|---|---|
| **P0** | **The gate**: extend the cross-target sweep to `.rbv`; add wasm32 to PR CI; correct the script header and the nightly's "not yet written" note | Without it "supported" is an unverified claim. Cheap: the sweep already runs in ~1 min. |
| **P1** | M1 — namespace owns the channel | Kills the class that already shipped once; IR-neutral, ½ day |
| **P2** | M2 + M3 — value answers for its representation; 102 sites → 1; drive 27 → 0 | Pays the debt; makes the literal uninhabitable |
| **P3** | M4 — ratchet | Holds it at zero |

Note the ordering consequence: P0 lands *before* the refactor, so the refactor is
measured by a gate that is already watching, and P0's first run **immediately**
proves the `.rbv` path's 29/29 baseline is real rather than assumed.
