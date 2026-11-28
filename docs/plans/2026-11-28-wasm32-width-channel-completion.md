# wasm32 Invalid-IR Debt (22 → 0): Completing the Width Channel

**Date:** 2026-11-28
**Owner lane:** wasm32 cross-target emission
**Baseline commit:** `main @ 77a7571f7`
**Declared baseline:** `config/cross-target-baseline.toml` — wasm32 = 22, aarch64 = 0
**Predecessor plans:** `docs/plans/2026-11-25-wasm32-debt-27-to-zero.md`,
`docs/plans/2026-11-25-wasm32-supported-target-gate-and-width-refactor.md`

---

## 0. Step zero — restore a clean baseline

The working tree currently holds **uncommitted experimental edits** from a previous
attempt at this debt (`git diff --stat` shows 5 files, +46/−27):

```
src/backend/llvm/context.rs        (BRIEV_DBG_INS eprintln in record_value_width)
src/backend/llvm/emit_expr.rs      (value_width_or_target_width spliced into 4 more sites)
src/backend/llvm/emit_toplevel.rs
src/backend/llvm/mod.rs
src/backend/llvm/tests.rs
```

None of it is a fix; all of it is instrumentation plus mis-indented splices
(`emit_expr.rs:11083` et al.). **Revert to `HEAD` before starting.** Every count
below was measured at `HEAD`, so leaving the edits in place makes the plan's
numbers meaningless.

---

## 1. Evidence — the 22 failures, measured

The sweep artifacts are preserved at `/tmp/xt-plan` (22 `*.failed-src`, their
`.wasm32-unknown-wasi.log`, and the rejected `.ll` modules under
`/tmp/xt-plan/wasm32-unknown-wasi/<fixture>/`). Each row below was read from the
**emitted module**, not inferred.

| # | fixture | failing instruction | family |
|---|---|---|---|
| 1 | hashmap_insert_is_first_statement | `inttoptr i32 %t105` (`%t105 = load i64` from `%State`) | A |
| 2 | nested_local_dominance | `inttoptr i32 %t119` | A |
| 3 | ptr_crossdefn_probe | `inttoptr i32 %t8` | A |
| 4 | slice_basics | `inttoptr i32 %t520` | A |
| 5 | test_hashmap_rehash | `inttoptr i32 %t115` | A |
| 6 | test_hashmap_surface | `inttoptr i32 %t117` | A |
| 7 | spawn_await_segments | `mul i32 %t8, %t12` (`%t8 = load i64` from `argv`) | B |
| 8 | spawn_await_segments_tests | `mul i32 %t8, %t12` | B |
| 9 | spsc_smoke | `add i32 %t8, %t19` | B |
| 10 | test_pooled_member_foreach | `icmp slt i32 %t229, %t225` | B |
| 11 | float16_roundtrip | `store i32 %t63, ptr %t79` | B |
| 12 | plain_ptr_smoke | `store i64 %t0` (`%t0` defined `ptr`) | B |
| 13 | arena_folded_loop | `shl i64 %t35, 1` (`%t35` defined `i32`) | C |
| 14 | arena_free_cast | `shl i64 %t36, 1` | C |
| 15 | state_column_slice | `call … @briev_slice_strided64(… i64 %t65 …)` | C |
| 16 | test_piggybank | `store i64 %t245` (`%t245` defined `i32`) | C |
| 17 | arena_scope_reset | `inttoptr i32 %t39` (`%t39` defined `ptr`) | D |
| 18 | arena_scope_reset_rejected | `inttoptr i32 %t39` | D |
| 19 | arena_smoke | `inttoptr i64 %t0` (`%t0` defined `ptr`) | D |
| 20 | float64_precision | `call ptr @int_to_str(i32 %t17)` (`%t17` defined `i64`) | E |
| 21 | literal_float_coercion | `call ptr @int_to_str(i32 %t12)` | E |
| 22 | str_open_end_slice | `call … @briev_str_substr(… i32 %t35 …)` | E |

**Family counts:** A = 6, B = 6, C = 4, D = 3, E = 3. Total 22.

### 1.1 The canonical mechanism (family A, read verbatim from the module)

```llvm
%t103 = getelementptr inbounds %State, ptr %state, i32 0, i32 3
%t104 = getelementptr [1 x i64], ptr %t103, i64 0, i64 0
%t105 = load i64, ptr %t104        ; the %State slot is an i64 WORD
%t107 = inttoptr i32 %t105 to ptr  ; the consumer declares the target word
```

`%State` field 3 is `[1 x i64]`. `%t105` is genuinely `i64`. The `inttoptr` at
`:5069` is genuinely `i32`. The module is genuinely invalid. The diagnostic is
not a false positive.

### 1.2 Family D is invalid at any width

```llvm
%t41 = inttoptr i32 %t39 to ptr   ; %t39 is defined `ptr`
```

`inttoptr` requires an **integer** operand. Feeding it a `ptr` is structurally
invalid on every target; wasm32 only happens to be where the verifier now says
so. This family is a real emitter defect independent of widths.

### 1.3 Family C is a hardcoded 64-bit shift

`src/backend/llvm/mod.rs:10827` and `:10852` (`emit_arena_grow_hosted`):

```rust
writeln!(out, "{}{} = shl i64 {}, 1", indent, grow_sz, size_reg).ok();
```

`size_reg` is a native-width value (`i32` on wasm32). The shift is spelled `i64`.
The surrounding `ptrtoint ptr … to i64` / `add i64` arithmetic is legitimately
64-bit (byte counts are i64 on every target); only the operand width is wrong.

---

## 2. Root cause — one cause, not twenty-two

**A value's width at a consumer is chosen from a *type-derived rule* instead of
from *what was actually emitted*.**

Three type-derived rules conspire:

| rule | where | gives | correct when |
|---|---|---|---|
| `format!("i{}", ctx.int_bits)` | ~25 sites in `emit_expr.rs`, plus the fallback of `value_width_or_target_width` | `i32` on wasm32 | operand really is a native int |
| `llvm_type(ty)` | `adapt_arg_to_param`, several cast lanes | type's canonical width | value was emitted at its canonical width |
| `_ => "i64"` / `Type::Ptr(_) => 8` | `push_field_type` (`mod.rs:2181`), `type_size` (`types.rs:61`) | `i64` / 8 bytes | target is 64-bit |

The state/argv **word ABI is i64 by design** (a slot must hold a double, an i64,
or a zero-extended 32-bit pointer), and that design is sound. So the defect is
not in the producer — it is that **consumers do not ask the producer**.

### 2.1 The mechanism that would answer it exists — and is dead

`WidthChannel::harvest_new_definitions` (`context.rs:1175`) was built for exactly
this: it re-parses the emitted text and stamps each `%reg` with the LLVM type it
was actually written at. Its own doc comment states the thesis:

> the emitted text is the one recorder that cannot forget — it IS the emission.

**It has zero production call sites.** `git grep harvest_new_definitions` returns
only its definition. It was unwired by the stage-C revert (`cd85b5164`). What
survives is 20 hand-written `record_value_width` calls in production code — the
exact "discipline that failed twice" its comment warns about. Producers such as
`load i64, ptr %slot` never call it, so the channel is blind precisely where the
22 failures live.

### 2.2 The codebase already names the missing piece

`helpers.rs:2686` `adapt_arg_to_param`, integer arm, is deliberately disabled
with a measured rationale:

> the missing piece is not this arm: it is a call-argument path that every call
> site shares, **and records that describe the emitted width**.

That is this plan. Two prior variants were tried and reverted: converting every
integer argument broke four programs via **defn-liveness drift**, and narrowing
by the *declared* Briev type left `net_stack_helpers` red because `%ac3` is
*declared* `Int` but *emitted* `i64`. Both failures are explained by rule 2's
"declared vs emitted" confusion — the declared type is not the emitted width.

---

## 3. What not to redo

Recorded so the next agent does not pay for it again:

1. **Do not fix consumers one at a time by guesswork.** 119 `inttoptr` sites, 376
   `writeln!(out` sites in `emit_expr.rs` alone. Per-site archaeology was
   attempted for three days and converged on nothing.
2. **`%tN` is function-local and rewound per body.** Two `%t105` in one module
   are unrelated (verified: `spawn_await_segments.ll` defines `%t8` in 20
   different functions). Any diagnosis that treats a register name as an identity
   across functions is wrong. `mistyped_operands` already clears per `define`
   and per `}` — that part is correct.
3. **`value_width_or_target_width` with an unpopulated channel is a no-op.** Its
   fallback is `i{int_bits}`; splicing it into more sites changed nothing (this
   is what the current dirty diff did).
4. **Do not change the state/argv word ABI to `i{int_bits}`.** It would need
   `type_size`/`type_alignment` (`Type::Ptr(_) => 8`) made target-aware, would
   break every i64/double slot, and would contradict `adapt_to_i64`'s contract.
5. **Do not weaken the verifier.** `mistyped_operands` caught a real defect; the
   module really is invalid.

---

## 4. Design decision

**Keep the i64 word ABI. Adapt at the boundary.** A slot holds a 64-bit word; on
wasm32 the pointer inside it is 32 bits, so the consumer **truncates** to the
target word before `inttoptr`. This is already the codebase's chosen idiom —
`narrow_handle_to_target_width` (`emit_expr.rs:7271`) does exactly this and is
used at two sites (`11022`, `11428`). The work is to make that the *general* path
rather than the exception.

**Widths are answered by one question: "what was emitted for this register?"**
answered by the harvest, with the type as fallback and `int_bits` last.

---

## 5. Workstreams

### W1 — Wire the harvester (no IR change)

Call `WidthChannel::harvest_new_definitions(&body_text, &mut harvest_state)` so
the channel reflects the emitted text before any consumer asks.

- **Where:** the function-body buffer is appended through a small number of
  helpers. Harvester state is already per-body (`FunctionContext::harvest_state`,
  `context.rs:1240`) and the buffers are per-function, so offsets line up.
  Prefer one choke point over per-site calls; if no single choke point exists,
  harvest at statement granularity in `emit_statement`.
- **Invariant:** pure bookkeeping. `writeln!` output must not change. Gate with a
  full-corpus native IR diff (§6.1).
- **Exit:** an opt-in debug dump (`BRIEV_DBG_WIDTH=1`) shows `%t105 → i64`,
  `%t8 → i64`, `%t35 → i32` at the failing sites.

### W2 — `inttoptr` operand adaptation (family A = 6)

Add one helper on `LlvmBackend`, e.g.

```rust
/// Emit an `inttoptr` of `reg` to `ptr`, adapting the operand to the target
/// word first. Returns the register that was cast, or None when `reg` is
/// already a pointer (an `inttoptr` of a `ptr` is never valid).
fn inttoptr_operand(&mut self, out, indent, reg: &str) -> Option<String>
```

Semantics:

| recorded emitted type of `reg` | action | IR delta |
|---|---|---|
| `i{int_bits}` or unknown | use `reg` as-is | none |
| wider int (`i64` on a 32-bit target) | emit `trunc … to i{int_bits}`, return new reg | `+1 trunc` |
| `ptr` | return `None` — caller must not `inttoptr` | (feeds W5) |

Replace every `inttoptr i{int_bits} %x` emission in `emit_expr.rs` with this
helper. On x86-64 (`int_bits == 64`) branch 1 always matches, so the emission is
**byte-identical**; this is the argument that makes the change safe, and it must
be verified rather than assumed.

### W3 — Shared call-argument path (family E = 3, family B part)

Implement what `adapt_arg_to_param`'s disabled integer arm asks for: convert an
integer argument to the **declared parameter width** using the **recorded emitted
width**, not the declared Briev type.

- Locate the single path every call site shares (the comment says one exists and
  that some call sites bypass `adapt_arg_to_param` — find it before writing code).
- Convert with `trunc`/`zext`/`sext` per the parameter's signedness.
- **Risk (measured, §2.2):** adding conversions previously shifted defn liveness
  for four programs. Mitigation in §7.

### W4 — Store/word adaptation (family B remainder = 3)

`store i32 %t63, ptr %t79` where the slot is an i64 word, and `store i64 %t0`
where `%t0` is `ptr`. Route stores through the same "adapt to the recorded slot
width" helper. `plain_ptr_smoke` (`store i64 %t0`, `%t0 : ptr`) is the mirror
case and may need `ptrtoint` rather than a width conversion.

### W5 — Emitter defects independent of widths (family D = 3)

`inttoptr` of a `ptr` operand. Trace the three sites (`arena_scope_reset`,
`arena_scope_reset_rejected`, `arena_smoke`) and remove the bogus cast — either
pass the pointer through, or `ptrtoint` before the width adaptation if an integer
form is genuinely required downstream.

### W6 — Arena growth operand width (family C part = 2)

`mod.rs:10827`, `:10852`: widen `size_reg` to `i64` (or compute the shift in the
native width) before `shl i64`. Byte counts stay i64; only the operand is wrong.

### W7 — Residual family C (2)

`state_column_slice` (`briev_slice_strided64` arg) and `test_piggybank`
(`store i64 %t245`) — both are "a native i32 value reaches an i64-typed
position". Like W3, the answer is the recorded width, not the declared type.
Expect these to fall out of W1+W3; if not, diagnose individually with the W1 dump.

---

## 6. Invariants

1. **Native x86-64 IR byte-identical.** `int_bits == 64` on x86-64, so every new
   branch degenerates to today's emission. Verify, don't assume: compile the full
   corpus before/after and `diff` the `.ll`.
2. **aarch64 stays 0.** It is also 64-bit, so invariant 1 should carry it;
   re-run the aarch64 sweep anyway.
3. **The verifier is not weakened.** No change to `src/ir_verify.rs`'s checks.
4. **`.ll` determinism.** `bash benchmarks/ll_determinism_check.sh --quick` — the
   harvest map must never be iterated in an IR-affecting walk (it is keyed by
   register; use `iter_sorted` if any walk is ever added).
5. **No defn-liveness drift.** The emitted-defn set for the whole corpus must be
   unchanged (§7 mitigation).
6. **Baseline only decreases.** `config/cross-target-baseline.toml` wasm32 falls
   22 → … → 0, never rises, and `cargo test --test cross_target_baseline` keeps
   the record honest.

---

## 7. Risks and mitigations

| risk | evidence | mitigation |
|---|---|---|
| Integer-argument conversion shifts defn liveness | measured: 4 programs lost `@put_bytes`/`@put_u8` in a prior attempt (`helpers.rs:2704`) | Before W3, snapshot the emitted-defn list for the whole corpus. After, diff it. Any delta is a bug in the change, not an acceptable cost. |
| `%ac3`-shaped values: declared `Int`, emitted `i64` | measured (`helpers.rs:2712`) | This is why W1 comes first: the record, not the declared type, must answer. If the harvester shows `i64` for a declared-`Int` register, the *producer* of that register is the site to fix. |
| The harvest choke point does not exist | `emit_expr.rs` has 376 `writeln!(out` calls | Harvest at statement granularity if no single point exists; the harvester is incremental (`HarvestState` carries the position), so per-statement calls are cheap. |
| `harvest_state` offset desync when bodies are buffered then spliced | `harvest_state` is per-body and buffers are per-function | Add a test that harvests a two-function module and asserts the first body's facts are gone after the second `define`. |
| Family D fix hides a real bug | `inttoptr` of `ptr` is invalid everywhere | Fix the emission; add the three fixtures to a positive test asserting no `inttoptr` has a `ptr` operand in any emitted module. |

---

## 8. Named tests (add before or with the fix)

Repo rule: behavioural, not literal; each must fail before and pass after.

1. `wasm32_inttoptr_of_state_word_truncates` — a program with a `List<Int>`
   foreach over a state-stored collection emits `trunc i64 … to i32` before
   `inttoptr i32` on `wasm32-unknown-wasi`, and emits **no** trunc on x86-64.
   Fixture: `tests/tier1/hashmap_insert_is_first_statement.bv`.
2. `inttoptr_operand_is_never_a_pointer` — across the corpus, no emitted
   `inttoptr` takes a `%reg` defined as `ptr` in the same function. Covers D.
3. `harvest_clears_at_define_boundary` — unit test on `harvest_new_definitions`:
   two `define`s, same `%t8` name, different types; the second must not see the
   first's fact. (Extends the existing `the_trace_never_crosses_a_function_boundary`
   test to the *channel*, not just the diagnostic.)
4. `call_arguments_adapt_to_the_recorded_width` — `int_to_str` call site with an
   i64 argument on wasm32 emits a trunc; the declared-defn liveness set is
   unchanged from the pre-change snapshot.
5. `arena_growth_shift_operand_matches_native_width` — `emit_arena_grow_hosted`
   emits a widened operand for `shl i64` on wasm32; byte-identical on x86-64.
6. `width_channel_answers_the_failing_sites` — with the debug dump enabled, the
   registers from §1 resolve to their emitted widths (`%t105 → i64`, `%t35 → i32`).

Fail-at-zero floors (gate rule, AGENTS.md "Working Rules"): the sweep gate must
assert `checked >= 129` and `failed <= declared_baseline`, so an empty sweep
cannot report success.

---

## 9. Ordering and commits

| step | work | commit | gate |
|---|---|---|---|
| 0 | revert the dirty tree to `HEAD` | — | `git status` clean |
| 1 | W1 harvest wiring + dump | `emitter(wasm32): harvest emitted widths before consumers ask` | native IR diff empty; `cargo test --lib` |
| 2 | W2 `inttoptr_operand` + replace sites | `emitter(wasm32): adapt the inttoptr operand to the target word` | fixtures 1-6 green; native IR identical |
| 3 | W5 `inttoptr`-of-`ptr` removal | `emitter: never inttoptr a pointer operand` | fixtures 17-19 green |
| 4 | W6 arena shift operand | `emitter: widen the arena growth operand, not the shift` | fixtures 13-14 green |
| 5 | W3 shared call-argument adaptation | `emitter(wasm32): adapt integer call arguments to the recorded width` | liveness snapshot unchanged |
| 6 | W4 + W7 residual | `emitter(wasm32): adapt store and slice arguments to the slot width` | sweep = 0 |
| 7 | baseline record + docs | `docs: wasm32 invalid-IR baseline 22 → 0` | `cargo test --test cross_target_baseline` |

Each step is independently revertible and each ends with a green sweep of the
fixtures it claims. If a step cannot reach its own fixtures, revert it and record
the finding in `BUGS.md` rather than leaving a partial.

---

## 10. Verification commands

```bash
# Step 0
git diff --stat            # must be empty before starting

# Sweep (authoritative; ~4 min, parallel)
rm -rf /tmp/xt-plan && CROSS_TARGET_ARTIFACTS=/tmp/xt-plan \
  bash scripts/cross_target_ir.sh wasm32-unknown-wasi
rm -rf /tmp/xt-arch && CROSS_TARGET_ARTIFACTS=/tmp/xt-arch \
  bash scripts/cross_target_ir.sh aarch64-unknown-linux-gnu

# Native IR byte-identity (§6.1)
#   compile the corpus with the pre-change and post-change binary, then
#   diff the .ll trees.

# Determinism
bash benchmarks/ll_determinism_check.sh --quick

# Library tests
cargo test --lib

# Gate
bash scripts/gate.sh quick
```

---

## 11. Open question for the owner

W2's "trunc then `inttoptr i32`" and the alternative "`inttoptr i64` directly"
both produce valid wasm32 IR. Trunc matches the existing
`narrow_handle_to_target_width` idiom and keeps all downstream pointer arithmetic
in the target word; `inttoptr i64` relies on LLVM truncating for you and leaves an
i64 in the IR where a pointer is meant. This plan chooses **trunc**. Say so if
the intent is the other reading — it changes W2/W4's helper shape, not the plan's
structure.

---

## 12. Status & decision record (2026-11-28)

### §11 decision: inttoptr at the operand's RECORDED width, not a forced trunc

The owner closed §11 with "do what generally makes sense, and record it." The
chosen idiom is **cast at the operand's actually-emitted width** (the
`inttoptr_word` convention), NOT a `trunc`-then-`inttoptr i32` rewrite:

- `inttoptr i64 <i64 reg>` is valid on every triple and keeps the value
  honest; LLVM folds the pointer-width mismatch at the next GEP.
- A forced `trunc i64 → i32` before the cast would leave an i64 in the IR
  where a pointer is meant, and re-introduces the "invented width" failure the
  record channel exists to prevent.

So `emit_inttoptr` and `canonical_ptr` now read the operand's recorded width:
`ptr` record → `bitcast` identity; `iN` record → `inttoptr iN`; unknown → keep
today's `i{int_bits}` (never invent). This is exactly what the plan called the
"general path" and it is what `inttoptr_word` already did for its own call
sites.

### The tooling bug that masqueraded as non-convergence

Every "no change" sweep in this effort was testing a **stale
`target/release/brievc` (built 27 Nov)** while the compiler was built into
`/scratch/briev-builds/h5-phase-a-target`. `scripts/cross_target_ir.sh`
resolves `BRIEVC=${BRIEVC:-./target/release/brievc}`. Once the sweep was run
with `BRIEVC=/scratch/.../brievc` the record-aware central `inttoptr` fix
registered immediately (the `hashmap_insert_is_first_statement` pair changed
from `inttoptr i32 %t105` to `inttoptr i64 %t105`). **Always export `BRIEVC`
to the freshly built binary before sweeping.**

### Measured state at this commit

- wasm32: **22 → 21** (central `inttoptr` + `canonical_ptr` record-aware,
  `value_width_or_target_width` harvest wiring, load recordings).
- aarch64: **0** (unchanged).
- x86-64 native: byte-identical (434 `backend::llvm` tests pin native IR text,
  all green; `ll_determinism_check.sh --quick` clean; 4047 `cargo test --lib`).

### Remaining 21, classified by root cause

| class | count | shape | mechanism |
|---|---|---|---|
| **RC1 value-width** | ~11 | `load i64` word consumed by a native `i32` op | operand must `trunc` to target word (argv/state word ABI, arena GEP, store/icmp) |
| **RC2 runtime Int ABI** | ~5 | C/stdlib lane declared or computed at `i64` where the target Int is `i32` | runtime declares + `fptosi`/call-arg target-parametric |
| **#165 float-slot lane** | 3 | `store i32` into a `[N x i32]` float-slot | **LIVE FOREIGN LANE — do not touch this pass** |

RC1 needs a single primitive — `adapt_emitted_to_width(reg, to)` (harvest,
record-first, `convert_int_width`), wired into the arith/store/icmp/GEP
operand positions. RC2 needs the runtime Int lanes to use `i{int_bits}`.
Neither is a `trunc` at `inttoptr`; both stay byte-identical on x86-64
because every native record is already `i64`.
