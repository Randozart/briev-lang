# wasm32 pointer width in LLVM codegen (2026-10-06)

**Status:** Active
**Blocks:** web routing (Part 4/5), any `.rbv` runtime string comparison
**Bugs:** BUGS.md 2026-10-06 "runtime string comparison emits hardcoded-i64
pointer casts" (and the unpacked-obj String-field entry, a sibling).

## Goal

The wasm32 backend must lower pointer↔integer conversions at the **target
pointer width** (32 bits on wasm32, 64 elsewhere) so runtime string comparison
(`briev_str_eq` → `str_len_bytes`) and the router compile and run.

## Root cause

`int_bits` is already the pointer width: it is derived from the data layout
(`CompilerContext::parse_pointer_width`, `pointer_bytes`) and set to 32 for
wasm (compile.rs `with_int_bits(32)`). But **168 sites** in
`src/backend/llvm/` hardcode `i64`:

```
grep -c 'ptrtoint ptr .* to i64|inttoptr i64' src/backend/llvm/*.rs  → 168
```

On wasm32 these emit `ptrtoint ptr %p to i64` / `inttoptr i64 %i to ptr`,
which `llc` rejects when the integer is actually i32 (the `str_len_bytes`
failure: `%t1 = trunc i64 %t2 to i32; %t5 = inttoptr i64 %t1 to ptr`).

Why it hid: compile-time-FOLDED string comparisons (both args constant) DCE the
helper, so simple `.rbv` examples built; only RUNTIME comparisons (a value from
`current_path()`) reach the bad emission.

## Design

**One source of truth:** the pointer width. `int_bits` already carries it
(32 on wasm). Introduce a small helper on the emitter — `ptr_int_ty()` →
`format!("i{}", self.ctx.int_bits)` — and use it for every **pointer↔integer**
conversion and pointer-sized arithmetic.

**Do NOT blanket-replace `i64`.** Two distinct widths coexist:

- **Pointer-width (use `ptr_int_ty()`):** `ptrtoint`/`inttoptr`, address
  arithmetic, `Load#`/`Store#` address operands, function-pointer/`Data`↔`Int`
  casts, handle boxing of pointers, `%state`-pointer math.
- **ABI-fixed i64 (leave):** the String block header (`@str.N = <{ i64, [N x
  i8] }>`, the `[len]` field), and 64-bit data values (f64 bit patterns, `Bits<64>`,
  the `Int` protocol when a target keeps 64-bit ints).

On native x86_64 both widths are 64, so the change is a no-op there — the risk
is limited to wasm32, which is currently broken.

## Inventory (categorize before editing)

Grep the 168 sites and bucket each:
1. `ptrtoint ptr X to i64` / `inttoptr i64 X to ptr` → pointer width.
2. `i64` used as a String/Data handle through state slots → pointer width.
3. `i64` for a String `[len]` header or an f64/Bits<64> value → leave.
4. `i64` in `@str.N` constant types → leave.

## Sequencing

1. Add `ptr_int_ty()` (+ unit test that it is i32 when int_bits=32).
2. Fix the **cast lanes** (`(s as Data) as Int`, `Data as Int`) and `Load#`/
   `Store#` address lowering — the exact `str_len_bytes` path.
3. Gate: the router smoke fixture builds; a runtime string-compare `.rbv`
   builds AND prints correct results (compile + run on the host wasm lane).
4. Sweep the remaining pointer-width sites bucket-by-bucket, re-running the
   gate each time.
5. The unpacked-obj String-field store (sibling bug) — fix the store type to the
   element type at the unpacked slot.

## Gates

- `cargo test --lib` green per landing (baseline 2909).
- Praetor no new diagnostics on changed files.
- The router smoke + a runtime-string-compare fixture build and run.
- Native suite unaffected (the pointer width is 64 there — assert in the test).
- On-device gate for `.abv` is NOT in scope (this is wasm32/LLVM only).

## Documentation (Rule 3)

- This plan; a note in `docs/architecture/backend-architecture.md` on the
  pointer-width rule; BUGS.md entries flipped to FIXED.
- The `.rbv` output contract in `spec/SPEC.md` if behavior is user-visible.

## Progress log

**2026-10-07 — pointer width + the two signature/liveness classes; router
runtime gate green.**

- Cast lanes + `Load#`/`Store#` + `emit_load`/`store`/`copy`/`fill`/`free`/
  `malloc` + `Eq`/`Neq` frgn returns all derive width from `int_bits`
  (`ptr_int_ty`, `adapt_to_ptr_width`, `widen_to_i64`, `narrow_int_result`).
  `helpers.rs` `__briev_free` fix. `cargo test --lib` 2916 green.
- **Void-frgn signature agreement** (BUGS.md 2026-10-07): one predicate
  `ResultType::is_void()` shared by the declare loop and all three frgn call
  emitters (`emit_direct_frgn_call`, cast-lane `ExtCall`,
  `emit_bridge_frgn_call`); `callee_declares_void()` unifies the user-call
  path; shared `frgn_call_line()` builds the call text. `call void @navigate`
  now matches `declare void @navigate`.
- **View-surface liveness roots** (BUGS.md 2026-10-07):
  `DefnLiveness::build_with_roots` + `pipeline::view_trigger_txns` /
  `view_external_roots`; webstack `compile.rs` seeds them; `__reset_*` rooted;
  pre-fn/async emission loops gated on `live_defns`.
- **Gates:** router runtime `gate.mjs` 11/11; `check_calls.py` clean on
  router + view-directives + view-bind-edge IR and 25 buildable native
  fixtures; view-directives emits exactly its three correct unfired warnings;
  view-bind-edge no false warning. Praetor: no new diagnostics on changed
  files (`index_item` improved 2→1 by extracting `root_txn_dispatch`).
- **Still OPEN (unchanged):** unpacked-obj String-field store. The other
  hardcoded-`i64` call sites were MEASURED (2026-10-07): `__briev_coll_resize`
  and `briev_str_next_char` are `out defn` external-symbol ABI — their `i64`
  calls are correct and must stay. The genuine latent sites are
  `emit_on_exit_cleanup` and `emit_operator_call`'s Identifier arm (both
  unexercised; need a gated ABI-convention change).
