# Wave-5 plan — stdlib quality & portability sweep (2026-11-20)

**Trigger.** The external stdlib critique + our own audit surfaced a set of
real, actionable items. Owner direction: "Don't assume 'not yet'. We found
it, now we are going to really clean up and make it better."

**Status: LANDED 2026-11-20 — see the execution record at the end.**

---

## W5-1. aarch64 syscall emission is broken — correctness fix (P0)

**Verified bugs (src/backend/llvm/intrinsics.rs):**
1. The aarch64 arm emitted `svc #0` with constraints
   `={x0},{x0},{x1},…,{x5},~{memory}` — **no x8 constraint**. The aarch64
   ABI puts the syscall number in w8/x8; `svc #0` reads it from x8. The
   number never reached the kernel. (riscv64 did this correctly: `a7` is
   an explicit constraint. x86_64 works only because number and arg0
   share `rax`.)
2. `resolve_syscall_number` had tables for riscv64 (asm-generic) and
   x86_64 — **no aarch64 table**, so aarch64 resolved numbers from the
   **x86_64 table** (wrong: Read=0 vs 63, Socket=41 vs 198, NanoSleep=35
   vs 101).

**Fix (landed `fe6b8aca7`):** aarch64 + riscv64 now share the asm-generic
table; the x86_64 arm is explicitly gated; other triples resolve `None`
(pairs with the `@briev_syscall` C fallback). The aarch64 asm ties x8 as
the number input (number-first, mirroring the x86_64 number-in-rax
shape). Gate: unit test pins per-triple numbers; an aarch64-triple .ll
build shows svc #0 pulling the number from x8.

## W5-2. Stdlib syscall-number routing through the per-triple tables — PARKED

Needs an abstract-op frontend spelling: bare identifiers are rejected as
undefined before codegen, and `SYS_*` constants are plain ints (parsed as
x86_64 values). The backend's `resolve_syscall_number` op-name path is
unreachable from source today. Scoped in the W5-1 commit message and the
research notes; requires a grammar/typechecker decision (op marker or
const-identifier resolution).

## W5-3. Duplicate-const diagnostic (P1, SPEC §7.2 enforcement) — LANDED `35c650328`

SPEC §7.2 says one declaration per name; the compiler accepted duplicates
silently (last-wins — verified: `const X = 5; const X = 7;` compiled and
read 7). syscall.bv had 21 duplicate names (87 declared / 66 unique), all
value-identical.

- `check_duplicate_constants`: same-module const redeclaration = **error**
  (SPEC-faithful, both identical and conflicting values; syscall.bv was
  deduped in the same commit so no existing source trips it).
- syscall.bv deduped to 66 unique names; `SYS_NANO_SLEEP` (dead export)
  removed.
- Gates: two unit tests pin both cases rejected; conformance sweep green.

## W5-4. socket.bv hygiene — LANDED `1177270ad`

19 raw syscall literals replaced with the imported `SYS_*` names
(`SYS_UNLINK` joins the import list). Zero behavior change;
socket_loopback_smoke verified.

## W5-5. Native list-index OOB semantics — LANDED `4b31bf328` (docs) + trap deferred

Verified divergence: interpreter raises a loud OOB error (eval.rs:1009);
native emits `getelementptr inbounds` = UB (14 sites). Only `PtrAdd#`
OOB=UB was SPEC-documented.

- LANDED: SPEC §11.4 note — native indexed access is bounds-unchecked
  (`inbounds` contract, UB on violation); the reference engine raises a
  loud error; `foreach` is always in-bounds and preferred where the index
  isn't load-bearing.
- DEFERRED: the `--check-indexes` runtime trap. Implementing it surfaced a
  deeper blocker — tier-List elements have TWO live addressing schemes in
  the same emitter (stack-seq slot-0 header vs region-list
  `{data_ptr, len, cap}`), and the emitter cannot tell which construction
  produced a given handle. A trap that reads the wrong header is worse
  than UB. The layout unification (or a layout tag on the handle) is the
  prerequisite — recorded in the research notes §7.

## W5-6. String/Data ABI freeze in SPEC — LANDED `c71c3073f`

`(s as Data) as Int + 8` appears 7× across stdlib and is the same layout
the interpreter Data door and backend rely on. SPEC §16.2 now names the
frozen layout: `[i64 byte-length][bytes][NUL]`, payload at offset 8,
NUL present but uncounted by `.^Length`. A layout change now names its
migration explicitly.

## W5-7. vDSO fast path — PARKED (prerequisite infrastructure now landed)

W5-1/W5-2 infrastructure is the prerequisite (per-triple tables). Gate
for doing it: a hot-path consumer reading clocks in a loop (none today —
kiosk reads clocks in tests; the bench harness times externally via
`/usr/bin/time`).

## W5-8. Stdlib style consistency — LANDED `73a367ed3`

56 identity re-assignments (`x = x + 1;`) across 8 stdlib modules
converted to the compound `x += 1;` (identical desugar — bld.rs
compound-assign). `foreach x in collection` remains available and is
used where the index isn't load-bearing; the range+index form stays
where the index IS semantic (checksum weights by position, the pos
threading from 11288520a).

## Sequencing (as executed)

W5-1 → W5-3 → W5-4 → W5-5 → W5-6 → W5-8. W5-2 parked (frontend design);
W5-7 parked (no hot consumer). CI 15/15 on the final push `73a367ed3`.

## Success criteria — met

- aarch64 syscall numbers resolve per-table and reach x8 ✓
- syscall.bv deduped to 66 unique names; duplicate-const diagnostic
  fires ✓
- socket.bv uses imported names at every call site ✓
- Native index-OOB semantics SPEC-pinned ✓
- String/Data ABI is a named SPEC section ✓
- Style sweep: identity re-assignments converted ✓
- Full lib suite green on every landed commit; CI 15/15 ✓
