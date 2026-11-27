# wasm32 Invalid-IR Debt: 22 → 0 (continuation plan for a fresh agent)

**Date:** 2026-11-27
**Owner lane:** wasm32 cross-target emission
**Baseline commit:** main @ be05e225d (stage-C re-landed; per-body epoch live)
**Declared baseline:** `config/cross-target-baseline.toml` — wasm32 = 22, aarch64 = 0
**Sweep command:** `CROSS_TARGET_ARTIFACTS=/tmp/xt-plan bash scripts/cross_target_ir.sh wasm32-unknown-wasi`

---

## 1. Read this first — the three laws this work operates under

These are the lessons bought with three waves of reverts. Do not re-learn them:

1. **The harvest is the recorder; the channel is epoch-scoped.**
   `WidthChannel::harvest_new_definitions` (context.rs) re-parses the emitted
   text and stamps records with the channel's epoch. A define crossing bumps
   the epoch AND clears the map (context.rs `f(None)` arm). Readers use
   `value_width` / `get_current`. **The one hazard:** bodies that never call
   `inttoptr_word`/`harvest` early have no records of their own, and the
   harvest pointer may sit before their prologue — so `get_current` returns
   None and readers fall back to `llvm_type(ty)`, which is wrong for
   custom-handle types (Bytes → 'i64' though the ABI is 'ptr'). The fix shape
   that works: harvest at the consumption site (finish_user_call entry) and
   per-body `begin_epoch` (already landed in emit_definition/emit_callable_txn
   via `begin_epoch_public`).

2. **Producers record; consumers consult.**
   Every site that emits a value with a non-obvious width must call
   `record_value_width(reg, actual_ty)` immediately after the writeln. The
   ExtCall arm at emit_cast_steps (~13562), the ptr-ABI return record in
   finish_user_call, and the two malloc paths are the pattern. A consumer that
   guesses from the Briev type is the bug.

3. **Direction table (measured, current):**
   | direction | count | mechanism |
   |---|---|---|
   | i64→i32 | 14 | a register recorded/defined i64 consumed by an i32-width op (post-`%t176` family; mostly proto_bytes/ExtCall results and their adapt paths) |
   | i32→i64 | 4 | the inverse: an i32 (word-width handle on wasm32) consumed as i64 |
   | ptr→i32 | 2 | arena_scope_reset family: a ptr consumed as i32 (inttoptr-handle shape) |
   | ptr→i64 | 2 | plain_ptr_smoke/arena_smoke: a ptr consumed as i64 |

   All four are the same root defect: the consuming site chose its width from
   a type-derived fallback instead of the record.

## 2. The 22 fixtures (exact list, current shape at be05e225d)

| fixture | shape | likely site family |
|---|---|---|
| arena_folded_loop | i32→i64 | adapt_from_i64 / cast-steps |
| arena_free_cast | i32→i64 | adapt_from_i64 |
| arena_scope_reset | ptr→i32 | inttoptr-handle |
| arena_scope_reset_rejected | ptr→i32 | inttoptr-handle |
| arena_smoke | ptr→i64 | unrecorded call result |
| float16_roundtrip | i64→i32 | ExtCall result (proto_bytes family) |
| float64_precision | i64→i32 | ExtCall result |
| hashmap_insert_is_first_statement | i64→i32 | ExtCall result |
| literal_float_coercion | i64→i32 | ExtCall result |
| nested_local_dominance | i64→i32 | ExtCall result |
| plain_ptr_smoke | ptr→i64 | unrecorded call result |
| ptr_crossdefn_probe | i64→i32 | ExtCall result |
| slice_basics | i64→i32 | ExtCall result |
| spawn_await_segments | i64→i32 | ExtCall result |
| spawn_await_segments_tests | i64→i32 | ExtCall result |
| spsc_smoke | i64→i32 | ExtCall result |
| state_column_slice | i32→i64 | i32 handle consumed as i64 |
| str_open_end_slice | i64→i32 | ExtCall result |
| test_hashmap_rehash | i64→i32 | ExtCall result |
| test_hashmap_surface | i64→i32 | ExtCall result |
| test_piggybank | i32→i64 | i32 handle consumed as i64 |
| test_pooled_member_foreach | i64→i32 | ExtCall result |

The definitive fresh list + shapes: re-run the sweep, then for each
`*.failed-src` read the paired `*.wasm32-unknown-wasi.log` for the
"defined as `X` but … used as `Y`" line (a one-liner extraction loop is in
the git history of this plan's commits; it is a 6-line bash for-loop).

## 3. The general fix (do this once, not 22 times)

The consuming sites must ask the channel before falling back to the type.
Three consumer families are known:

- **`adapt_from_i64` ptr arm** (helpers.rs, the "ptr" match arm): already has
  the record-check pattern (returns `reg` when `value_width(reg)==Some("ptr")`).
  Extend the same pattern to the sibling arms that produce i64→ptr / ptr→i64.
- **ExtCall/Bitcast lane in emit_cast_steps**: `cur_ll` already asks
  `value_width(&src.name)` first (stage C). Verify every lane in that loop
  derives `cur_ll`/`dst_ll` from the record where present, with the type as
  fallback only.
- **finish_user_call arg loops (defn + else)**: the defn loop uses
  `value_width` (flipped); the else loop's `actual_ty`/`reg_is_ptr` were
  flipped in the re-land — confirm they survived the exact-restoration
  (e7d32ef8b) and re-apply if not.

For any remaining fixture after those three: the record is *absent* (not
wrong). The producer of that register must record. Trace the producer with
the backtrace probe pattern:

```rust
// in WidthChannel::insert, temporarily:
if reg == "%tXXX" {
    let bt = std::backtrace::Backtrace::force_capture();
    eprintln!("[bt] {}", bt.to_string().lines()
        .filter(|l| l.contains("briev_compiler")).take(4)
        .collect::<Vec<_>>().join(" | "));
}
```

(BRIEV_DBG_INS gates a version of this; the pattern is proven — it found the
emit_definition producer in minutes.)

## 4. Stage order (each stage = one commit, one gate)

**S-A. Re-verify the baseline reproduction (30 min)**
Run the sweep, extract the 22 shapes, confirm the table above matches at the
current tip. Any fixture that no longer fails → move it out of the declared
baseline (the ratchet only turns one way; lowering is the point).

Gate: sweep says "matches declared baseline" with the corrected number.

**S-B. Consumer-side completion (the three families above)**
For each of the three consumer families: flip the reader to the channel, keep
the type only as fallback. The exact-restoration commit (e7d32ef8b) removed
some of these flips — `git show e7d32ef8b^:src/backend/llvm/emit_expr.rs` has
the flipped forms to re-apply selectively.

Gate: aggregate marshal test + rbv_list_each + saltpetre_tiny green; sweep
count drops (target: the i64→i32 ExtCall family, 14 fixtures, is mostly the
ExtCall result record gap — one record at the ExtCall arm may fix many).

**S-C. Producer-side completion for stragglers**
Any fixture still failing: backtrace probe at the use, find the producer,
add the record. Do them one at a time; each is minutes once the pattern is
known.

Gate: sweep 22 → 0; aarch64 stays 0; native tier1 corpus diffs are the
previously-verified transparent renumbering set only.

**S-D. Lock it**
- Lower the declared baseline to 0 in `config/cross-target-baseline.toml`.
- Add a `tests/cross_target_baseline.rs` assertion that the count stays 0
  (the ratchet only turns one way).
- Update `docs/plans/2026-11-25-wasm32-debt-27-to-zero.md` with the final
  table and point §1's three laws at this plan as the worked example.

Gate: full `scripts/gate.sh full` green; ll determinism ALL DETERMINISTIC.

## 5. Invariants (each pinned by a named test)

| # | invariant | test |
|---|---|---|
| I-1 | a width record is stamped with the epoch of the body that emitted it | `a_record_carries_the_epoch_of_its_body` |
| I-2 | a define crossing makes previous-body facts invisible to current-epoch readers | `a_previous_epoch_record_is_invisible_to_a_current_epoch_read` |
| I-3 | every `inttoptr`/`ptrtoint` consumer consults the record before the type | new: `consumers_consult_the_record_before_the_type` |
| I-4 | the declared baseline can only move down (except with a written justification in the config) | `the_baseline_is_not_raised_silently` |
| I-5 | the emitted IR is byte-identical on x86-64 across the whole change | `ll_determinism_check.sh --quick` + native tier1 corpus diff |
| I-6 | every producer of a width-non-obvious value records it | new: `producers_record_extcall_and_ptr_abi_results` |

## 6. Known traps (do not step on these again)

- **The mixed-state revert.** Reverting emit_expr while keeping stage-C's
  context.rs produced a tree that passed some tests and failed others, and
  cost a full session. If reverting, revert *all* backend/llvm files
  atomically.
- **The budget rides with the source.** `config/width-literals.toml` is
  measured per tree state; a revert that changes counts must carry the budget
  fix or CI fails on the ratchet (this exact failure happened twice).
- **cross-target baseline is a ratchet.** 22 → lower only. If a fix changes
  the set, re-derive the count from the sweep, not from memory.
- **[nd]/[arg] probes are env-gated.** A silent probe usually means the env
  var wasn't set in that run, not that the path didn't run. Grep the env var
  first.
- **`llvm_type(Custom)` is the trap.** For custom handle types (Bytes,
  ProtoReader, Named) it returns a machine-word type, not the ABI type. Any
  consumer falling back to it will guess wrong on exactly the fixtures in
  §2. The record is the truth; the type is the intent.

## 7. Definition of done

- `scripts/cross_target_ir.sh wasm32-unknown-wasi aarch64-unknown-linux-gnu`
  reports 0 failed on both triples.
- `config/cross-target-baseline.toml` declares 0/0.
- `scripts/gate.sh full` green; determinism ALL DETERMINISTIC.
- `docs/plans/2026-11-25-wasm32-debt-27-to-zero.md` final table updated:
  27 → 0.
- BUGS.md entries for the fixed classes moved to resolved with the
  producer/consumer fix references.
