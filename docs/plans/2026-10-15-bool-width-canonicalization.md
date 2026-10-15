# Bool-width canonicalization — fix the `or i64` / `trunc i8` class

**2026-10-15.** Unblocks `lib/std/os/dir.bv` (Phase 3 of the core-usable
purge) and every other program whose txn/defn postcondition combines
`Result` protocol tests with `||`.

## Root cause (pinned from source)

The backend's canonical Bool representation is **i8** holding 0/1:

| Site | Evidence |
|---|---|
| Casting graph | `set_llvm_type("Bool", "", Fixed("i8"))` — `src/casting/graph.rs:368` |
| Comparisons | `icmp … ; zext i1 to i8` — `emit_expr.rs` (all arms) |
| `Not` | `xor i8 {}, 1` — `emit_expr.rs:7305` |
| Consumers (11) | `trunc i8 %x to i1` — `abi.rs:20`, `dispatch.rs:141,465,473,489`, `emit_stmt.rs:308,2091,2334`, `emit_toplevel.rs:1210,4245`, convergence gate `emit_toplevel.rs:5171` |
| ABI doc | "Bool: trunc i8 to i1" — `abi.rs:10` |

The invariant this encodes (**I1**): *a register labeled `bool_()` holds
0/1 in an i8.*

**The violators** are the logic arms in `emit_expr.rs`:

- `And` (7208) / `Or` (7221): emit `and/or <llvm_type(&l.ty)>` — the
  **operand's** width — but label the result `bool_()`. When an operand
  register is wide (i64-actual, bool_-labeled), the label lies and the
  consumer's `trunc i8` hits an i64 → clang: *"defined with type 'i64'
  but expected 'i8'"*.
- `Not` (7305): emits `xor i8` (canonical) but labels the result
  `operand.ty.clone()` instead of `bool_()` — the mirror defect (label
  follows the operand, not the op's own contract).

Repro (dir.bv): postcondition `[is_ok(term) || is_err(term)]` on a
`-> Result<List<String>, Int>` callable txn → `%t208 = or i64 …`
feeding `%pc213 = trunc i8 %t208 to i1`. The upstream seam that makes
`is_ok(...)` results wide in contract context is pinned in Phase 0
below (candidates: generic-mono return substitution, the `term` slot
typing, Result member dispatch).

Note the good case already works: `or i8` + `trunc i8` appears in
existing IR (e.g. `target/probe/out/wrap_probe.ll:188`) when operands
are genuinely i8 bools — which is why this class stayed latent.

## Non-goals

- `BitAnd`/`BitOr`/`BitXor` are different ops (numeric/string semantics,
  B1) — untouched.
- The 11 consumer sites are already canonical — untouched.
- SPIR-V `Bool` (OpTypeBool) is a separate resolver — untouched.

## Phase 0 — pin the upstream seam (experiment before building) — FINDINGS

1. The minimal repro (`target/probe/bool_probe.bv`, callable txn
   `step(...) -> Result<List<String>, Int>` with postcondition
   `[is_ok(term) || is_err(term)]`, runtime-driven via `get_env_int!`)
   now emits **canonical IR**: `define i8 @is_ok`, `call i8 @is_ok`,
   `or i8`, `%pc88 = trunc i8 %t83 to i1` (bool_probe.ll:4075-4080).
   The convergence-gate shape is clean at current HEAD.
2. The original `or i64` is not reproducible at HEAD: PR #19's merged
   Field-scoping fix changed needs_state verdicts (old call shapes took
   `%state` and boxed differently), and the syscall.bv totality fix
   (below) unblocked the named-import path. The class was latent — the
   arms now produce canonical IR only because every current producer
   happens to uphold I1.
3. Two pre-existing blockers were found and cleared on the way:
   - `lib/std/posix/syscall.bv:113` — the variadic dispatcher's
     `match args.Count#()` (Int scrutinee, arms 0..6, no fallback) is
     non-total under the exhaustiveness rule. Prelude/keep_all imports
     skip the raw-body re-typecheck (why the suite stayed green);
     NAMED imports re-typecheck the spliced generic body, where
     `Count#()` has not folded — every named-import program importing
     fs/dir/syscall failed typecheck. Fixed with a `_ => 0 - 1` arm
     (unreachable for `Int[<=6]`; -1 follows the -errno convention).
   - `is_ok(r) || is_err(r)` is a TAUTOLOGY on the closed Result enum —
     the optimizer folds it wherever both sides are visible (the gate
     keeps it runtime via the `term` phi). Probe drivers that see a
     concrete Result fold the whole gate away — the repro must keep
     `term` runtime-opaque.
4. Question (b) answered: bare consumers (`if is_ok(r)`) do NOT break —
   the fix is contained to the `And`/`Or`/`Not` arms. Wide-register
   values observed in practice hold 0/1 in the low byte (comparison
   zext, call returns of `define i8`), so `trunc`-to-i8 semantics is
   the right normalization; no `icmp ne 0` needed.

## Phase 1 — central helper + logic arms — hardening (still required)

The arms remain the one producer that can violate I1 by construction
(operand-width emission, `bool_()` label on the result; `Not` labels
`operand.ty.clone()`). Hardened regardless of the current clean IR —
the invariant must hold by construction, not by luck of current call
shapes.

## Phase 1 — central helper + logic arms

- New named helper in `emit_expr.rs` (Rule 17/18):
  `coerce_to_i8_bool(&mut self, out, indent, reg: TypedRegister) ->
  TypedRegister` — passthrough when `reg.ty == bool_()`; otherwise
  convert from `llvm_type(&reg.ty)` to i8 (`trunc` for wider ints, or
  `icmp ne 0` per Phase-0 finding; `zext` for i1).
- `And`/`Or`: normalize BOTH operands through the helper → emit
  `and/or i8`; result stays `bool_()`.
- `Not`: normalize operand → keep `xor i8`; label the result
  `bool_()` unconditionally.
- Undo path: restore the two arms to `llvm_type(&l.ty)` /
  `operand.ty.clone()` (re-introduces the class; the Phase-3 IR test
  fails).

## Phase 2 — upstream producer fix

- Whatever Phase 0 pinned: fix the seam so bool-returning calls produce
  `bool_()`/i8 registers at every call surface.
- Sweep every `bool_()` label site (`grep -n "Type::bool_()" 
  src/backend/llvm/`) and verify each producer upholds I1. Fix any
  additional violators the same way.

## Phase 3 — tests

1. IR unit test (emit_toplevel tests module, `void_defn` idiom):
   postcondition `[is_ok(term) || is_err(term)]` → assert `or i8`
   present, `= or i64` absent, and `define` for the defn present
   (guards the vacuous-pass where liveness drops everything).
2. Behavioral E2E: `fs.bv remove_file` (added unexercised earlier —
   extend `fs_smoke`) and `dir.bv entries()` golden (real dir +
   nonexistent dir). clang `-O0` IR-validity gate at the same commit.
3. Full suite green (baseline 3093/0) + Praetor baseline-diff on
   `src/backend/llvm`.

## Phase 4 — unblocked tail (Phase 3 of the purge)

- Land `lib/std/os/dir.bv` (its smoke now links).
- `lib/std/os/temp.bv` (O_CREAT\|O_EXCL + rand + retry + remove_file).
- Honest headers on the 16 remaining `os/*.bv` stubs.
- BUGS.md entry for the class (cross-ref the :1814 boxing table, root
  cause = label/width desync, fix = I1 enforcement at the arms).
- Invariant I1 documented in `docs/architecture/backend-type-dispatch.md`.

## Gates per commit

`cargo test --lib` · no new warnings · Praetor baseline-diff
(`src/backend/llvm`) · targeted `git add` only · one commit per
coherent unit.

## Status

- [x] Root cause pinned (source reading, this file)
- [x] Phase 0: upstream seam pinned — IR canonical at HEAD; arms hardened
      by construction (below); syscall.bv totality fix landed en route
- [x] Phase 1: helper + And/Or/Not
- [x] Phase 2: producer sweep (emit_fcmp, volatile_store)
- [x] En route finds: gate `term` binding + contract closure (generic
      Result contracts now monomorphize — see lifecycle plan for the
      term-semantics proposal the crash motivated)
- [ ] Phase 3: dir E2E smoke + BUGS.md + commit/push
- [ ] Phase 4: dir.bv landed, temp.bv, stub headers
