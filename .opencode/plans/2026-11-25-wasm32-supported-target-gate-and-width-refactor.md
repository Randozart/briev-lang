# wasm32 is a supported target — gate it, then pay the width debt

**Date:** 2026-11-25
**Owner decision:** wasm32 is the ship target for the web build (`.rbv` → wasm32). Confirmed.
**Predecessor:** `.opencode/plans/2026-11-25-ptr-emission-debt-proposal.md` (research, §13–14)
**Base:** `main = 44fc3a7e8` (wave-11 S0–S2; wasm32 52 → 27)

---

## 0. The problem, stated as three facts

1. wasm32 is a **shipped** target, and it has the **weakest gate** in the project:
   the cross-target sweep covers only `tests/tier1/*.bv tests/tier2/*.bv`
   (`scripts/cross_target_ir.sh:47`) — **no `.rbv` program at all** — is not in PR
   CI, and its own header (`:25-28`) declares wasm32 out of scope. The nightly
   workflow lists "wasm32 cross-backend corpus gate" as *"harnesses not yet
   written"* (`.github/workflows/verification-nightly.yml:11`).
2. Measured today: **`.rbv` 29/29 pass** on wasm32 (latent), **`.bv` 73/100**
   (27 fail). The debt is one feature away from the web path.
3. The debt's shape is not "27 bugs": it is **103 hardcoded `inttoptr i64`**
   against **65 places that already do it correctly by hand**, plus a
   representation channel with **14 namespace restarts and 1 clear** — the hole
   that produced #119, #163, #164 and the S2 regression.

## 1. What this plan does

| stage | deliverable | gate |
|---|---|---|
| **P0** | the gate: `.rbv` in the sweep, wasm32 in PR CI, a declared 27-baseline that may only shrink, corrected docs | sweep runs green at baseline on every PR |
| **P1** | one owner for the register namespace and the representation channel | native IR byte-identical + full gate |
| **P2** | the value answers for its representation; 102 sites → 1 helper; wasm32 **27 → 0**; verifier sees calls/arith | wasm32 0; native byte-identical |
| **P3** | ratchet: a new width literal fails CI | count strictly decreasing |

## 2. Invariants

Each invariant names the test that pins it. An invariant without a test is a
comment, and comments do not fail.

| id | invariant | pinned by |
|---|---|---|
| **I-1** | The register namespace (counter) and the representation channel (`repr`) are one owned object; a namespace restart cannot orphan the channel. | `registers_reset_clears_counter_and_repr_together` (unit, `context.rs`) + `registers_fields_are_private` (compile-time: the fields are not `pub`) |
| **I-2** | No emitter writes a target-dependent integer width as a literal. Widths come from `ctx.int_bits` (word), or from `CastingGraph::resolve_llvm_type` (typed value). | `no_new_width_literals` (ratchet) + `cross_target_ir.sh` for wasm32 and aarch64 |
| **I-3** | A value's emitted representation is *either* derived from its Briev type via the casting graph *or* an explicitly recorded override. Never remembered from a previous body. | `the_value_answers_for_its_own_representation` (unit) |
| **I-4** | Overrides are a small closed, declared set — `ptr`, `i{int_bits}`, `i32`, `double` — not free-form strings. | `every_repr_override_is_a_declared_representation` (unit + corpus) |
| **I-5** | Native emitted IR is byte-identical across P1–P2 (12-fixture baseline in separate processes). Only 32-bit targets may change. | `ll_determinism_check.sh --quick` + the 12-fixture byte diff recorded per stage |
| **I-6** | The wasm32 invalid-IR count is monotonically non-increasing at every commit and reaches **0**. | `wasm32_corpus_baseline_only_shrinks` (reads `config/wasm32-invalid-ir.baseline`) |
| **I-7** | The web path (`.rbv`) is IR-gated against its own target, in PR CI, not nightly. | `cross_target_ir.sh` includes `.rbv`; CI case `cross-target-wasm32` |
| **I-8** | Every cross-target failure names its cause (file, line, both register definitions) rather than a bare count. | `mistyped_operands_sees_call_argument_mismatches` (unit) + `cargo test --lib -- ir_verify` |

## 3. Named tests (the proof obligations)

New tests to write; each must fail before its stage and pass after.

| test | where | asserts |
|---|---|---|
| `registers_reset_clears_counter_and_repr_together` | `src/backend/llvm/context.rs` (`#[cfg(test)]`) | `reset()` clears both; there is no path that clears one |
| `the_value_answers_for_its_own_representation` | `src/backend/llvm/tests.rs` | `TypedRegister::llvm_ty` derives from `ty` via the graph; a recorded override wins; target width respected (32 vs 64) |
| `every_repr_override_is_a_declared_representation` | `src/backend/llvm/tests.rs` | every `record_value_width` argument in the source is in the declared set |
| `word_to_ptr_asks_the_value_for_its_width` | `src/backend/llvm/tests.rs` | emits `inttoptr i32 %v` for a 32-bit-typed value, `inttoptr i64` for 64-bit, and nothing for `ptr` |
| `mistyped_operands_sees_call_argument_mismatches` | `src/ir_verify.rs` | `call i32 @f(i64 %t)` where `%t = trunc … to i32` is rejected, with line + both definitions |
| `wasm32_corpus_baseline_only_shrinks` | `tests/cross_target_baseline.rs` | current failure count ≤ `config/wasm32-invalid-ir.baseline`; prints delta |
| `no_new_width_literals` | `tests/width_literal_ratchet.rs` | per-file counts ≤ `config/width-literals.toml` |
| `web_path_compiles_for_wasm32` | `tests/rbv_wasm_corpus.rs` | every `.rbv` in `examples/` + `tests/` builds `--triple wasm32-unknown-wasi` |

## 4. Stages

### P0 — the gate (lands first, alone)

1. `scripts/cross_target_ir.sh`: extend the corpus to `examples/*.rbv` and
   `tests/**/*.rbv` (same self-calibrating rule: native must accept).
2. Rewrite the header comment: wasm32 is a shipped target, not an inspection
   extra. Fix `.github/workflows/verification-nightly.yml:11` (no longer queued).
3. `config/wasm32-invalid-ir.baseline` = the measured count, with the failing
   program names recorded in the file's comment block.
4. `tests/cross_target_baseline.rs`: reads the baseline, runs the sweep, fails on
   an increase, prints the delta and the newly-failing names (I-6).
5. `.github/workflows/ci.yml`: a `cross-target-wasm32` case running the sweep
   (I-7).

Exit: sweep green at baseline; baseline is 27 *for `.bv`*; `.rbv` **0**.

### P1 — one owner for the namespace (I-1)

`FunctionContext.txn_counter` + `value_llvm_ty` fold into `Registers { counter,
repr }` with private fields and one `reset()`. The 14 reset sites are classified
**before** editing: each is either a function boundary (must clear both) or a
mid-body restart (must either clear both or justify keeping stale entries). The
classification is recorded in the commit message — the `emit_toplevel.rs:8155`
comment documents one case where clearing at every reset broke
`rbv_list_each_wasm`, so this stage is where that comment is either honoured or
proven outdated.

Exit: native IR byte-identical, full gate green, `rbv_list_each_wasm` green.

### P2 — the value answers for its representation (I-2, I-3, I-4, I-8)

1. `TypedRegister::llvm_ty(&ctx)` — derive via `resolve_llvm_type`, honour an
   override. Replace the 11 `record_value_width` writers with `set_repr` on the
   declared set (`ptr` / word / `i32` / `double`), so each deviation is reviewable
   as *"this value is emitted as a boxed word although its type says String"*.
2. `mistyped_operands` extended to calls, arithmetic, `icmp`, `select`, `ret`.
3. `word_to_ptr(&mut out, indent, v: &TypedRegister)`; 102 literal sites → calls.
4. Drive wasm32 27 → 0 one class at a time: `inttoptr` (7) → call-argument widths
   (6, via the existing `adapt_arg_to_param` treatment) → arithmetic tail. The
   baseline file decrements at each commit.

Exit: `cross_target_ir.sh wasm32-unknown-wasi` → 0; native byte-identical;
`ir_verify` finds nothing on tier1 + 74 tier2/example programs.

### P3 — the ratchet (I-2, long-term)

`config/width-literals.toml` (per-file counts; 175 `briev_*` runtime-ABI widths
declared allowed with a reason) + `tests/width_literal_ratchet.rs` failing on
increase with the offending line.

## 5. Acceptance

1. `cross_target_ir.sh wasm32-unknown-wasi` → **0 failed**; `aarch64` stays **0**.
2. `.rbv` corpus is in that sweep and in PR CI.
3. `ir_verify` covers calls/arith with zero findings on the corpus.
4. Native IR byte-identical throughout (12 fixtures, separate processes).
5. Ratchet green at a count **lower** than 27's complement (i.e. the remaining
   literals are declared, not accidental).
6. Rule-24 check: write one new program after P2 and compile it for both targets
   with zero compiler changes.

## 6. Risks

| risk | mitigation |
|---|---|
| P1's "one owner" breaks `rbv_list_each_wasm` (documented precedent) | classify all 14 sites first; byte-identical IR requirement makes the breakage immediate and local |
| P2's derivation disagrees with an override at a site that "worked" | each disagreement is a real representation finding → declare an override, never special-case the derivation |
| The 27 sit in paths whose fix is a *feature*, not a width | measure per class; if a class needs frontend work, record it in BUGS.md rather than weakening the target |
| Ratchet rots | the test prints the total and delta in its failure message; BUGS.md records the number at each landing |

---

## 7. Outcome (measured, 2026-11-25, at `f63ef64c2`)

| stage | landed | measured result |
|---|---|---|
| **P0** gate | `1cd202299` | corpus 100 → **129** programs (29 `.rbv` added); sweep parallel (295 s → ~4 min for TWO triples); baseline file + CLI claim; wasm32 + aarch64 in PR CI |
| **P1** one reset path | `399cfc326` | 11 inline `txn_counter = 0` sites → 3 named methods; no site writes the field; 2 new pins; native IR byte-identical |
| **P2a** verifier + measurement | `8b6818544` | `mistyped_operands` covers calls/arith/`icmp`; refusals separated from IR debt; **debt 27 → 12** |
| **P2b** operand width + ratchet | `f63ef64c2` | `operand_width` helper (ask the value, then the type); `config/width-literals.toml` + ratchet test |
| **P2c** drive 12 → 0 | **NOT DONE** | 12 classified in BUGS.md; each needs its own site |
| **P3** ratchet | `f63ef64c2` | green at the measured table |

### What changed about the plan, and why

1. **I-1 was measured false as written.** "One owned object whose reset clears
   both" cannot be built yet, and the reasons are in `context.rs` as comments: (a)
   clearing the whole channel breaks `%ac{i}` names, which are position-derived and
   whose recorded width stays true across a boundary; (b) the records themselves
   are not truthful yet (`emit_expr.rs:13137` stamps a cast step's *canonical*
   width, not the emitted one), so dropping a stale entry can make wasm32 worse.
   P1 therefore landed as "no site writes the counter, the three scopes are named",
   with the clearing explicitly deferred and pinned by a test that keeps the
   reasoning in the tree.
2. **The debt number was wrong by 2.25×.** Counting a refusal (no module emitted)
   as an invalid-IR failure inflated 12 into 27. Fixed in the gate.
3. **The web path is clean.** `.rbv` 29/29 — the debt is latent there, active on
   the feature corpus. That is the opposite of the assumption the work started
   from, and it means the shipped surface is not currently broken; the risk is that
   it is one feature away.
4. **`.rbv` was never gated at all** — the single most important finding. A
   supported target with no corpus gate is how 12 invalid programs lived
   undetected while 4632 tests passed.

### Remaining work (owned by the next stage)

* **P2c**: 12 programs. Eight word-width-vs-declared-32, two `ptr`-as-`i64`, two
  inverse. Needs per-site diagnosis, not shape patching.
* **The 15 refusals**: why does the termination walk reject 15 programs on wasm32
  and accept them on x86-64? Not a width bug; possibly a capability difference.
* The channel clear in `begin_function`, once records describe their emission.
