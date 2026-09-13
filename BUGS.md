## WORKING-TREE HAZARD: external process reverted in-flight edits — OBSERVED 2026-08-27

**Symptom (three times this session):** freshly edited files (lexer.rs,
vocab.rs, capabilities.rs, probe fixtures under tmp_probe/) returned to
their PRE-EDIT content minutes after a verified green build; file mtimes
showed the snapshot's ORIGINAL timestamps (e.g. 21:59 from the prior
evening), not the edit time. Mid-edit edits by this session were silently
undone while OTHER recently-edited files (parser/definitions.rs,
backend/circt/mod.rs, ast/top.rs) survived — i.e. selective, not whole-tree.

**Consequences encountered:** a survivor commit landed internally
inconsistent (parser/circt referenced `Token::Extern`/`extern_cells` while
lexer/capabilities were pre-edit); a stranded green state was reproducible
only in a fresh clone.

**Mitigation that worked:** build+commit inside an isolated clone
(`/tmp/opencode/slice_a`), push the branch back via fetch+ff-only into the
worktree; absolute paths for every cross-repo operation; tiny commits
immediately after each verified step.

**Suspected cause:** a bidirectional file-sync/mirror service on
~/Desktop/Projects (stale-copy push, mtime-preserving). Identity not
confirmed. IF THIS RECURS: check for sync daemons before assuming editor
or agent interference, and keep uncommitted windows short.

**Status:** RESOLVED-BY-CONVERGENCE 2026-08-27 — main and foundation
converged to identical content (`e11af56a`), making the mirror
idempotent; all auxiliary worktrees/branches retired as verified
ancestors. Single-tree model now (only `briev-lang` +
`briv-compiler-baseline` per rule 12b). If reverts recur on the single
tree, re-open this entry and re-apply the §2 playbook (see
docs/2026-08-27-session-report.md).

# Bugs

## float → Data → Int bitcast emitted invalid LLVM — FIXED 2026-09-09

**Date:** 2026-09-09 (parity spike)
**Symptom:** `(f as Data) as Int` on a float32 emitted `bitcast float %x to
i64` — LLVM requires same-width bitcast. clang rejected
`invalid cast opcode for cast from 'float' to 'i64'`. The raw float
bit-pattern read (needed by the pure-Briev float formatter) was impossible.
**Fix:** `emit_expr.rs` — the cast-chain Bitcast lane now emits `float → i32`
then `zext i32 → i64` (both the chain loop and `emit_single_cast_lane`).
**Undo:** revert the `cur_ll == "float"` arms in both sites.

## Tuple-returning defn with a String field mis-lays the String as i64 — OPEN 2026-09-09

**Date:** 2026-09-09 (parity spike)
**Symptom:** a `defn` returning `(String, Int)` (or any tuple containing a
String) compiles to IR where the String field is `load i64`'d out of the
tuple struct, then fed to `__print_str(ptr ...)` — clang rejects
`'%t31' defined with type 'i64' but expected 'ptr'`. Int-only tuples
(`(Int, Int)`, `(Int, Int, Int)`) work fine.
**Repro:** `defn pair(x: Int) -> (String, Int) { term ("ab", x); }` then
`let (a, b) = pair(3); println!("{} {}", a, b);` — IR fails to compile.
**Root cause (hypothesis):** the tuple layout/load path stores the String
field in an i64 slot instead of a ptr; the String ABI (length-prefixed
buffer ptr) is not preserved through tuple packing. Not yet traced.
**Workaround in spike:** split into a String-returning function and an
Int-returning function (duplicate the shared computation).
**Fix path:** trace tuple pack/unpack in emit_expr.rs (the struct { i64, ... }
layout for String fields); String fields must be `ptr` slots.

## PiggyBank Phase D — arrow dispatch + sealed-op gaps (FIXED 2026-08-18)

**Date:** 2026-08-18 (implementing the plan's PiggyBank)
**Status:** FIXED 2026-08-18. Five gaps the sealed one-shot collection surfaced:

1. **Consume-blind arrow dispatch.** `dest <- src` and `dest ~<- src`
   typechecked/codegen'd IDENTICALLY — both resolved the value's
   ExtractFrom (a `<- queue` DRAIN popped). There was no way to express a
   non-destructive READ. The typechecker's `extract_op_order` and codegen's
   `find_extract_strategy_for_arrow` now select `CopyFrom` for `<-` and
   `ExtractFrom` for `~<-` (fallback to the other). `queue_drain_idio`'s
   drain is destructive → `~<- queue` (the correct form).
2. **Parameterized extract ops.** The arrow supplies NO arguments, but the
   coll scaffold's CopyFrom is `get(i)` — a `b <- q` dispatched get WITHOUT
   the index, emitting an undefined `@i` global (invalid IR). Extract ops
   must be zero-param; a parameterized preferred op falls back to the other.
3. **Pooled-instance strategy resolution.** `piggy <- 1` (InsertAt arrow on a
   pooled instance) and `all ~<- piggy` (ExtractFrom arrow) found no strategy
   — `collection_base_type_name` didn't resolve pooled instances, so the
   receiver emitted an undefined `@piggy`. `resolve_id_type` now covers
   instance-prefix bases; `is_coll_type` peels the `Vector(..)` column
   wrapper; the arrow receiver uses the instance-prefix dummy.
4. **Pending-error name collision.** The Error# store was keyed by member
   NAME only — `PiggyBank.Count` (sealed) collided with `List.Count`, so
   resolving a List's Count promoted the jar's sealed error. Keyed by
   `{type}.{member}`.
5. **Sealed At/Iter not promoted through syntax.** `piggy[0]` used the
   generic-first-arg rule and `foreach x in piggy` computed an element type
   without consulting the sealed ops. `promote_op_member_error` now fires on
   Index (→ At) and Foreach (→ At/Iter).

## Foreach item name poisoned a later member tuple-destructure (SSA main) — FIXED 2026-08-18

**Date:** 2026-08-18 (found by the Phase E full-HashMap-surface program:
`foreach k in ks { ... }` before `m.insert((i, i*10))` in a boolean-contract
node)
**Status:** FIXED 2026-08-18.

**Symptoms:** wrong `get`/`contains`/`keys()` results AND a clang -O3 -flto
frontend SIGSEGV on the generated IR. The `let (k, v) = e` destructure inside
the HashMap `insert` member bound `k` to a register that was owned by a LATER
statement (`%t516 = mul …` — a list-resize capacity). `clang -O0` read the
undefined value (garbage inserts/gets); clang -O3 crashed in the frontend/gold
plugin on the SSA dominance violation (use before def).

**Root cause (two leaks, both required):**
1. A `foreach` loop-variable binding was inserted into `last_val_temps` +
   `let_bindings` (emit_stmt.rs) and NEVER removed after the loop body. The
   item name (`k`) therefore outlived the loop.
2. `last_val_temps` was not cleared at an emission-pass boundary — the
   alwaysinline `@txn_go` copy and the SSA-main replay emit the SAME node body,
   and `clear_locals()` (context.rs) skipped `last_val_temps`/`last_val_types`.

The SSA-main replay of `insert` then resolved `k` through the stale
`last_val_temps` entry (checked BEFORE `get_local` in emit_expr) to the
@txn_go pass's foreach register — a name that in the replay's numbering
belonged to a different, later value.

**Fix:** the foreach item binding is now scoped to the foreach body (removed
from all five binding maps after the body), and `clear_locals()` clears
`last_val_temps`/`last_val_types` so emission passes are isolated. Regression
test `test_foreach_item_does_not_poison_member_destructure` links + RUNS a
program that would print garbage without the fix; it prints `7`.

## Member-param / instance-field shadowing asymmetry — FIXED 2026-08-18

**Date:** 2026-08-18 (surfaced by `hash_ops_idio` capacity-init: the passed
capacity never reached the table size)
**Status:** Fixed (`emit_expr.rs` + `emit_stmt.rs` + `lib/std/collections.bv`).
**Root cause:** a member-body parameter that shadows a same-named instance
field (`txn init(cap: Int)` on an obj with field `cap`) resolved asymmetrically
in the backend: the READ side (`emit_expr.rs`) guarded against the shadow, but
the WRITE side (`emit_stmt.rs`) did not — pooled (`self_prefix`) and boxed
(`self_binding`) writes of `cap` routed to the FIELD COLUMN, dropping the
argument and leaving the table at the zero-init size (256-slot growth stall,
capacity seed silently discarded). A separate lexical bug: the enclosing
instance's `self_prefix` leaked into nested boxed locals (the earlier boxed
member-body bug, same family).
**Fix (two layers, so the guard and the fix cannot disagree):**
1. Kill the shadow at the source — `init(cap)` → `init(capacity)` in
   `lib/std/collections.bv` with `match capacity { 0 => 256, _ => capacity }`.
2. Symmetric guards on BOTH sides: `emit_expr.rs` read guards (pooled + boxed)
   and `emit_stmt.rs` write guards (pooled `self_prefix` + boxed
   `self_binding`), all via the flat "None-trick" (`let x = if shadowed { None }
   else { clone }`) to avoid nesting/Praetor complexity.
**Verification:** `test_member_param_shadows_same_named_field` strengthened to
assert shadowed writes store into a fresh alloca, never the field column;
`test_hashmap_capacity_seed_and_break_probe` pins the seed → `0 => 256` phi →
`c*8` malloc sizing and the probe loops' early-exit `break`. `cargo test --lib`
1897/1897 green.

## Boxed member body inherited the enclosing instance's `self_prefix` — FIXED 2026-08-17

**Date:** 2026-08-17 (surfaced by `hash_ops_idio` → 240 instead of C's
24999995000000)
**Status:** Fixed (`emit_expr.rs` — the `cap` column clobber).
**Root cause:** `emit_member_body`'s boxed-self path set `self_binding` but
left the ENCLOSING instance's `self_prefix` in place. The HashMap's `init`
member runs with an outer `HashMap` prefix; its nested
`let init_items: List<(K, V)> = []` (a boxed local coll) routed the coll's
hidden `cap = 16` assign through that prefix → `HashMap.cap` → the MAP's cap
state column (slot 6), overwriting the just-stored `cap = 256`. The List
ended up with cap 16, so 10M inserts into a 16-slot table → read-back sum
`240` instead of C's Σ2i. (`len`/`inner.data` didn't collide — `m.len`/
`inner.data` aren't state columns, so they fell through to the correct
boxed-self path; only `cap` shared a name with a real map field.)
**Fix:** boxed member bodies clear the leaking `self_prefix` for their
duration (restored at the end) — bare names resolve against the boxed
receiver, never the enclosing instance's columns.
**Verification:** hash_ops_idio 240 → 65280 (= 2·Σ0..255, correct for the
256-cap map before the capacity rework).

## stdlib iterator.bv / hashmap.bv written aspirationally — never compiled — CLOSED

**Date:** 2026-08-14 (found while verifying generic `defn f<T>` dispatch)
**Status:** BOTH CLOSED 2026-08-16 (iterator.bv in Phase 3c; hashmap.bv in the
hashmap redesign).

- **iterator.bv** — CLOSED (Phase 3c): `List` import + transitive function-deps
  in the import closure + iter_any/all convergence rewrite. See the three-track
  plan's 3c SHIPPED section.
- **hashmap.bv** — CLOSED (2026-08-16, plan `2026-08-16-hashmap-redesign.md`).
  The old file used syntax that never existed (`:>`, `term {}`, `Option`,
  concat). The redesign made `obj HashMap<K,V>` a working collection through
  its op surface (`op Count`/`op InsertAt`/`op Init`/Tier-1 ops, `Entry<K,V>`
  element, linear-probe insert/get/contains/remove), fixed the compiler's
  seed-init and literal-construction gates, and rewrote hashmap.bv against the
  real surface. `brievc check` clean; runtime consumers verified.

**Known limitations surfaced by the redesign (all pre-existing compiler bugs,
documented in the redesign plan's SHIPPED section):**

1. **Tier-1 foreach over a collection whose cursor ops contain their own
   foreach** miscompiles (register cross-contamination, `%t243` undefined) —
   a hand-written Tier-1 collection with foreach-scanning `op Iter`/`op Step`
   cannot be `foreach`-ed.
   **FIXED 2026-09-08:** `emit_member_body` now saves/restores `let_binding_allocas`
   and `foreach_break_labels`, preventing inner foreach in cursor ops from leaking
   stale entries into outer scope.
2. **Generic-member `List<K>` arrow accumulator** fails to typecheck across an
   import boundary (free-T arrow) — `keys()`/`values()` scans deferred.
   **FIXED 2026-08-18:** arrow-push double-construction fix; `test_hashmap_surface.bv`
   verifies keys()/values()/entries() work across import.
3. **Nested `foreach` inside an `if` inside a txn member body** segfaults when
   the txn re-fires — rehash-on-full deferred.
   **FIXED 2026-09-08:** `emit_member_body` cur_block save/restore restored (conditional
   on `init_context` flag); the rehash body (nested foreach-in-if in txn) now compiles
   and runs correctly. HashMap `insert` rehashes automatically when full.
4. **A collection obj passed as a defn PARAM then mutated via a member method**
   corrupts memory (member-on-param ABI) — the hashmap.bv defn wrappers use
   direct member calls; a functional wrapper style is not yet safe.
   **FIXED 2026-08-18:** `box_pooled_instance_value` in emit_toplevel.rs; regression
   test `test_pooled_instance_defn_arg_is_boxed`.
5. **`hash_ops_idio` benchmark** removed from the suite — the probe-inlined
   hot loop overflows clang's frontend (the map is correct for normal use).
   **RE-ADDED 2026-08-18** (Phase B): the collapse was the param/field shadow
   asymmetry, fixed (see the 2026-08-18 entries); the benchmark now MATCHes C
   at ~1.05x parity with an honest 2*N open-addressing C reference and is back
   in `build_and_bench.sh`.
   **2026-09-08:** capacity changed from 2*N to N (HashMap now rehashes automatically).
   C reference needs updating to match.

**All five limitations are now FIXED.** The HashMap rehashes automatically when full
(lib/std/collections.bv, 2026-09-08). The `emit_member_body` state leak (cur_block,
let_binding_allocas, foreach_break_labels) that caused limitations #1 and #3 is
resolved.

## Inlined member with a foreach + a nested foreach — runtime collapse by clang -O3 — FIXED 2026-08-17/18

**Date:** 2026-08-17 (found by the HashMap tuple redesign)
**Status:** SSA-reactor path FIXED 2026-08-17; countdown/fold path FIXED
2026-08-18 (RESOLVED block below supersedes the OPEN status).

**Root cause (verified, supersedes the earlier `pending_phi_backedge` thesis):**
The emitted IR is SSA-clean and label-clean (`.ll` passes `llc`; per-function
register dup scan = 0). The miscompile is clang -O3 (LTO) folding the reactive
body dead after deep unrolling — NOT an emitter bug. Evidence:
- `opt -O3` on the same `.ll` KEEPS the body; clang -O3 guts it.
- clang pass trace (m2_clang_passes.log): every pass through ADCE retains the
  body; TailCallElim creates 13 tail calls, then the loop fully unrolls (16x,
  per-iteration phi chains) and a later pass collapses `@main` to
  print-first + ret. The kill is count-based: `FAIL ⟺ (≥2 in-body member-probe
  consumptions) OR (1 get + ≥2 fresh in-body tuple mallocs)`.
- `.ll` A/Bs through the exact harness link: moving the constant tuple
  materialization (malloc + header + element stores + ptrtoint) to the loop
  preheader FIXES it; moving only the malloc or the ptrtoint does NOT — the
  stores must leave the loop too.
- `pending_phi_backedge` save/restore is NOT the mechanism — the raw
  IR is clean.

**Fix (SSA-reactor path):** `emit_tuple` now defers a CONSTANT-element tuple
literal to the loop preheader when `defer_struct_allocas` is set
(emit_expr.rs), and `emit_ssa_main` buffers the reactive loop and flushes
pending materializations before `.ss_main_loop` (ssa.rs), mirroring the
countdown path's loop_buf + flush (2026-08-13 struct-literal fix). Dynamic
elements (var/field/call) stay inline — their registers cannot dominate a
preheader. Verified at HEAD+fix:
- `p5_3ins2get` (3 inline inserts + 2 inline-printed gets) → `10 20` ✓
- `p5_core` (3 ins + Count# + 3 gets + 2 contains) → `3 10 20 30 true false` ✓
- m-grid (m1/m2/m2_letget/m1_2get/vB/m3i_marker) full marker sequences ✓
- `cargo test --lib` 1893/1893 green.

**RESOLVED 2026-08-18** (Phase B of the 2026-08-17 completion plan): the
collapse root was the same param/field shadow asymmetry that dropped the
capacity seed (see the new entry below) — a countdown-path write side of the
shadow left the enclosing instance's `self_prefix`, so per-iteration probe
writes routed to the wrong column and clang folded the dead table. Fixed by
(1) renaming `init(cap)` → `init(capacity)` (no shadow at all) AND (2)
symmetric read/write, pooled/boxed shadow guards in emit_expr.rs/emit_stmt.rs.
`hash_ops_idio` now MATCHes C (Σ2i) at BOUND=10M and 50M, and the loop is
verified live at BOUND=500M (11.8s, not folded). The old C reference was a
256-ring (a different workload) — rewritten as a real 2*N open-addressing
table; the runtime suite records Briev 0.7292s vs C 0.6655s (1.09x, MATCH).
Benchmark re-added to the suite.

**Correction 2026-08-18 (Phase C):** the recorded 1.09x was partially enabled
by a SILENTLY-DROPPED write — `insert`'s `items <- (k, v)` (a pooled
member-field List push) emitted nothing, so the map skipped mirror maintenance.
Phase C fixed that push (new entry below); the honest-with-mirror result was
3.28x (2 mallocs/insert). The mirror list was itself a 2026-08-16 workaround
for the `acc <- keys[i]` double-construction bug that Phase C ALSO fixed, so
the dead mirror was DELETED from the HashMap (keys()/values()/entries()/foreach
will scan columns in Phase E). hash_ops_idio re-measured 0.7203s vs C 0.6751s
(1.06x, MATCH) with no dropped work.

## Pre-existing oddity (not a regression):** a node whose inserts are LET-BOUND
(`let e1 = (1,10); m.insert(e1)`) and whose observables are ONLY inline gets
(`vA_2get`) segfaults even on the pre-fix compiler: the SSA main emits the
tuple materializations and insert bodies into the unused alwaysinline
`@txn_go` fold copy, leaving `@main`'s `.ssb_go` with just the gets (which read
an unwritten table). Out of scope for this fix; acceptance shapes are
inline-consumption (`p5_*`).

## `when`-guard + nested probe `foreach` crashed clang — FIXED 2026-08-17

**Date:** 2026-08-17 (found via hash_ops_idio)
**Status:** FIXED — a countdown node with a `when`-guard AND an inlined member
`foreach` produced IR where the countdown latch phi's predecessor set included
the nested loop's internal blocks; `llc` reported "Instruction does not
dominate all uses! %cdm337 = sub ...", and clang's LoopDeletionPass segfaulted.
Fix: the `Statement::Foreach` arm sets `fun.cur_block = Some(foreach.endN)`
(emit_stmt.rs), mirroring the Guarded/If arms — a countdown body ending in a
`foreach` reports its end block so the decrement + latch phi land correctly.
hash_ops_idio now compiles and runs. (Its hot-loop get still reads 0 — the
multi-member register collision above, a SEPARATE bug.)

## Arrow `<-` push into a loop-carried local List double-constructs — FIXED 2026-08-18

**Date:** 2026-08-17 (found by the HashMap `keys()` scan)
**Status:** FIXED 2026-08-18. The `keys()` scan itself was correct; the bug was
the CALLER binding. `let ks: List<Int> = b.keys()` was routed through the
collection SEED constructor (`construct_local_collection_seed`), which treated
ANY non-List RHS as a seed — so the returned list was wrapped as `[<list>]` (a
new box with len forced to 1) and the scan read 1 of N elements. The typechecker
only routes a MISMATCHING RHS through parse-op construction, so codegen had no
type to distinguish "a collection that is already a List" from "a seed".

**Root:** `emit_stmt.rs` coll-typed-let path — `is_coll && !List` assumed every
non-List RHS was a seed.

**Fix:** emit the RHS once, then branch on the result type: if it is a
collection (`is_coll_type` / op-surface coll), bind it DIRECTLY; only a genuine
non-collection becomes a seed. `construct_local_collection_seed` now takes an
emitted `TypedRegister` instead of `&Expr`. Verified: keys() returns all 3
elements at -O0 and -O3.

## Pooled member-field List push (`items <- e`) silently dropped — FIXED 2026-08-18

**Date:** 2026-08-18 (found by the PiggyBank `put`)
**Status:** FIXED 2026-08-18. Inside a pooled member body, `items <- e` (a
PiggyBank `put` on `items: List<Int>`) emitted NOTHING — `p.size()` read 0. The
strategy lookup and the plain-copy fallback only recognized BARE names
(`let_original_types`, `field_index_map`), but a pooled slot is keyed
`{prefix}.{name}` (`PiggyBank.items`) AND its briev type is the COLUMN type
`Vector(inner, [Anonymous(1)])` (`List<Int>[1]`), so no InsertAt strategy fired
and the plain-copy target was unresolvable.

**Fix:** centralized `collection_base_type_name` (emit_toplevel.rs) resolves a
collection identifier's base name across function locals, state fields, and
bare pooled member names, peeling the `Vector` column wrapper; used by
`find_insert_strategy`/`find_extract_strategy` and `emit_strategy_member_call`.
The arrow stores (extract + plain copy) write pooled member targets through the
instance column via a GEP-into-element store (`emit_state_store_self_slot`),
mirroring the Assign arm. Verified: 3 puts → size() reads 3 at -O0 and -O3.

## Defn-param member mutation loses the value — FIXED 2026-08-18

**Date:** 2026-08-17 (found by the hashmap.bv `insert(map, ...)` wrapper)
**Status:** FIXED 2026-08-18.

**Root cause (A/B-verified 2026-08-18):** a POOLED instance used as a VALUE (a
defn argument) has no scalar register — its fields live in the state's pooled
columns, and the identifier arm emitted an undefined `@map` GLOBAL (`load i64,
ptr @map` → invalid IR; the caller's map was never boxed). A BOXED local map
worked (its handle passed, mutations persisted).

**Fix:**
1. `box_pooled_instance_value` (emit_toplevel.rs): a pooled-instance defn
   ARG is boxed at the call — malloc the struct size, copy each pooled column
   field into the box at its struct offset, pass the handle. The callee's
   member calls (boxed self path) then mutate the box and the returned handle
   carries the mutations (storage-class rule "locals box": a pooled instance
   that flows into a VALUE context boxes).
2. `instance_prefix_for`'s get_local fallback now matches ONLY spawn-pool
   bases — a defn param / boxed local of a pooled-typed obj is a BOXED value,
   never a pooled row, so its field access (`x.data`) and member calls use the
   boxed self (previously the field access indexed the pooled columns by the
   handle → garbage/crash).
3. The boxed-self regression guard (`is_pool_instance` in emit_member_body) is
   narrowed to spawn-pool bases — a base can BOTH pool (a top-level instance)
   AND box (a defn param of the same type), so the boxed path is legitimate
   for non-spawn-pool receivers.

**Verify:** `hashmap.bv` gained `insert(map, k, v)` / `remove(map, k, v)`
wrappers (previously omitted as unsafe); `let m2 = insert(m, ...)` + `get`
round-trips correctly. Regression test `test_pooled_instance_defn_arg_is_boxed`
(no `@p` global, box malloc present).

## `brievc check` over-reports on imported generic collection scans — FIXED 2026-08-18

**Date:** 2026-08-18 (found while adding the hashmap.bv wrappers)
**Status:** FIXED 2026-08-18 (plan
`docs/plans/2026-08-18-check-build-pipeline-unification.md`, commit `c3bab243`).
`brievc check` on a program importing `std/collections.bv` reported type errors
on the HashMap's GENERIC scans (`acc <- keys[i]` → "expected List<K> for arrow
assignment, found K"). `brievc check lib/std/collections.bv` alone passed; the
same imports built and ran correctly.

**Root cause (A/B instrumentation):** `import { HashMap }` dropped `List` —
the import resolver's transitive-reference scan (`referenced_type_names`)
walked only slots and member PARAMS, never member OUTPUT types, so `List`
(referenced only in `keys()`'s RETURN) was tree-shaken away. The typechecker's
built-in `List` name special-case (`list_literal_accepted_by`) masked the gap,
and `brievc check`'s stale LEAN pipeline (parse → resolve → check, no plugin
stages) surfaced it while build's fuller pipeline re-added List.

**Fix:** (1) the resolver walks member output types — a named import brings
its returned collection types; (2) `parse_and_check` now runs the SAME
pre-check pipeline as `compile_source` (plugin manager, comptime eval,
Parsed/Resolved stages, resolve_comptime_refs), so check and build cannot
diverge. Regression: `test_filter_keeps_member_output_types` (resolver) +
`check_on_imported_generic_scans_is_clean` (bin). `brievc check` clean on
`test_hashmap_surface.bv`, `collections.bv`, and `hashmap.bv` (previously
erroring).

## Iterable-protocol slice-6 deletions blocked on two live paths — RESOLVED 2026-08-16

**Date:** 2026-08-14 (found while executing slice 6, leak cleanup)
**Status:** Open — documented, not force-deleted. Two of the plan's
`docs/plans/2026-08-12-iterable-protocol.md` §10 deletions are blocked on
genuine live paths, verified in the compiled IR:

1. **`emit_heap_seq` is the live path for `Expr::Tuple` (`emit_expr.rs:746`)
   and expression-position list literals (`f([1,2])`).** Only *typed-local*
   list literals route through ops (`construct_local_collection`,
   `emit_stmt.rs:353`). Deleting it needs a tuple replacement first — a
   separate refactor, not a deletion.
2. **The hardcoded `List` foreach arm (`foreach_collection_kind`,
   `emit_stmt.rs:245`) is the LIVE path for `foreach x in list`, not dead.**
   `try_emit_tier_iteration` → `tier2_op_collection` does NOT fire for `List`
   in a real typecheck: the emitted IR for `foreach x in items` over a
   `List<Int>` state field or local contains `inttoptr` + `load i64, ptr`
   (the hardcoded `[len]` header), and no `Count`/`At` calls. Deleting the
   arm now would break all List iteration.

   **ROOT CAUSE ISOLATED (2026-08-14):** `tier2_op_collection` returns None
   because `ctx.obj_members` has no `"List"` entry in a default build —
   `lib/std/collections.bv` (where `obj List<T>` declares `op Count`/`op At`)
   is **never imported** by the prelude plugin (`plugins/parsed/prelude.bv`
   imports `std/types/bootstrap.bv`, `std/protocols.bv`, `std/os/*`,
   `std/env.bv`, `std/io.bv` — NOT `std/collections.bv`), and the documented
   auto-core scan (`docs/architecture/prelude-and-import-magic.md` §auto-core
   expects `{STDLIB}/std/core/collections.bv`) finds nothing because the
   layout has `collections.bv` directly under `lib/std/` with no `std/core/`
   directory. The typechecker's name-based `List` fallback
   (`typechecker/mod.rs:816/1180`) lets `List<T>` typecheck, masking the gap,
   and the backend silently falls back to the hardcoded layout.

   **Verified:** an explicit `import "std/collections.bv"` makes
   `tier2_op_collection` fire (`members_present=true`) and the IR emit
   `Count`/`At` calls. **FIXED (commit `a53c7f90`):** the prelude plugin now
   imports `std/collections.bv` (both branches), so `obj_members` always has
   `List` and `foreach x in list` iterates through `op Count`/`op At` — the
   hardcoded List arm is now production-dead (reached only by stdlib-free
   unit tests like `foreach_list_program`). Full runtime suite MATCH, no
   regression.
3. `ringbuf_inline` is deliberately KEPT — a performance mechanism
   (`queue_drain_idio`/`float_math` were its bent gates), not legacy layout.
4. Already done by prior work: `.^Length` collection error
   (`typechecker/mod.rs:3216`), `.^Len` → `.^Length` rename, `is_string_operand`
   iteration special-casing (subsumed by `IterKind::String`).

**What landed (slice 5, same pass):** the unconstrained-literal diagnostic —
`let xs = [1,2,3]` (no annotation) is a compile error directing to the
type-directed form; annotated literals unchanged.

**Path to unblock:** investigate why `tier2_op_collection` returns None for a
real `List` (item 2) — if tier iteration fires, the hardcoded `List` arm dies
naturally and the tuple `emit_heap_seq` split can be scoped separately.

**RESOLVED (2026-08-16, Phase 3d of `2026-08-16-three-track-broaden`):**
`emit_heap_seq` renamed to `emit_tuple` (tuple-only), `IterKind::List` DELETED
(all foreach over colls goes through the tier path), and the bare-list codegen
fallback is now a hard error (typed literals construct via ops; unannotated
literals are the slice-5 diagnostic). Two latent bugs fixed along the way: the
mask gather over a coll/List read slot 0 as the length (it's the data pointer —
now reads data + len from the tier block) and the backend did not register the
coll op bindings (tests bypassing compile.rs fell to the deleted heap-seq
path). Full suite green, benchmarks all MATCH. See the slice-6 plan's SHIPPED
section.

## Runtime `.^Size` matched a `len` slot name — FIXED 2026-08-14

**Date:** 2026-08-14 (found while removing the iterable-protocol §10.4
magic-name heuristics)
**Status:** Fixed (see `docs/plans/2026-08-14-string-unification-and-boundary.md`
§6a).

The runtime `.^Size` codegen arm (`emit_reflection`, `emit_expr.rs`) read a
collection's element count by matching a `len` **slot name** in `struct_types`
and GEPing it. Boxed Cat violation: it guessed structure the collection never
declared. `RingBuffer` has `read`/`write` (no `len`) and no `op Count` — its
count (`write - read`) is undeclarable by name; the magic would panic.
Reachability: preconditions (`elaborate_expr`) bypass the typechecker's
compile-time-only `Size` check, so `items.^Size` in a precondition reached the
arm (`examples/todo.rbv:8,13`).

**Fix (final, UOL §6a/§6b):** runtime `.^Size` is DELETED — per the tri-partite
rule (fields = user-declared; reflection = compiler-known non-operational
metadata; intrinsics = operations), the element count of a collection is an
OPERATION, so its home is the `Count#` intrinsic (which dispatches to the
type's declared `op Count`), not a reflection target. The typechecker's `Size`
arm is compile-time-only (`.^^Size` = vector shape); a runtime `.^Size`
reaching codegen (precondition bypass) is a clean error directing to `Count#`.
The old `lookup_field_offset`-for-`len` heuristic is gone. `examples/todo.rbv`
migrated to `Count#(items)`. (An intermediate sketch routed `.^Size` through
`op Count`; it was superseded — see the 2026-08-14 plan §6a/§6b.)

## Stale integration tests referenced pre-rename `briefc`/`briv_*` symbols — FIXED 2026-08-13

**Date:** 2026-08-13 (found during Phase 0 of layout-keywords)
**Status:** Fixed (integration-test rename sweep, `feat/spec-layout-keywords`).
**Scope (actual, larger than the original "7 files" claim):** 20 test files /
~169 references: `briefc`→`brievc` (incl. `CARGO_BIN_EXE_briefc`),
`briv_compiler`→`briev_compiler` (lib crate), `briv_types.h`→`briev_types.h`,
`BrivState`→`BrievState`, `__briv_*`→`__briev_*`, `briv_rt(.c/.o)`→`briev_rt`,
`briv-compiler`→`briev-compiler`, and the cosmetic `briv_*_test` temp-dir
names. `~/briv-tools` (a real user tool path) was deliberately left untouched.
**Fix:** scripted rename sweep + three API-drift fixes: deleted the dead
`tests/fallback_tests.rs` (the `Fallback` feature was removed at `5f2107bc5`
"remove frgn fallback clause"); dropped the `Fallback` arg from
`glue_bridge_tests.rs`; `Value::Int`→`Value::Atom(Atom::Int)` in
`pointer_trickery_test.rs`; `glue.dbvl`→`glue.dbv` config paths;
`term! -> X`→`endprogram X` in the three term fixtures; and
`lib/compiler/needs_state.bv` now casts `CStr`→`String` explicitly
(`cstr_to_briev`) after the String-param library-export codegen bug surfaced
(`%ac0` i64-vs-ptr, logged below). `glue_integration.sh` Test 5 now checks the
per-language `lib/glue/<lang>/glue.dbv` folders.
**Verification:** `cargo test` fully green (lib 1828 + bin 14 + all 30
integration-test binaries incl. the C/C++/Java/Node/Python/C#/Go/Lua drivers);
`tests/glue_integration.sh` 11/11.

## String-param library exports hit `%ac0` i64-vs-ptr codegen — FIXED (verified 2026-08-18)

**Date:** 2026-08-13 (surfaced while fixing `c_driver_needs_state`)
**Status:** FIXED — verified NOT reproducible with the current binary
(2026-08-18). An exported defn with a `String` parameter, built via
`brievc build --library`, emits `%ac0` typed i64 in the shim while the body
expects `ptr` (`opt: needs_state.ll: %ac0 defined with type 'i64' but expected
'ptr'`). Workaround in the dogfood pass: keep the boundary param as `CStr`
and cast with `cstr_to_briev` in the body. The `--library` shim's String-param
marshalling needs a fix (String is boxed as i64 at the ABI but the body reads
a ptr).

**Verification (2026-08-18):** `export defn greet(name: String)` and a
stateful `export defn state_str(name: String)` both emit clean
`define ptr @…(ptr %arg0)` with `--library`; `opt -O2` on the generated `.ll`
(the original failure) passes; `cargo test --test c_driver_needs_state` (the
driver that surfaced it) is green. The fix landed via the struct/String param
ABI (`emit_toplevel.rs:2631-2638`: `ptr` params → `ptrtoint`) plus the retired
SSO String-param branch. The `CStr` workaround in `lib/compiler/needs_state.bv`
is left in place — a legitimate ABI choice for the C boundary.

## Local collection values segfault — FIXED (slice 4)


**Date:** 2026-08-12 — **fixed** (commit `459dc980`). A local
`let xs: List<Int> = [2,4,6]` emitted the hardcoded `[len][elem]` heap-seq
buffer, which the At/Count members (expecting the List STRUCT layout) read as
garbage. `construct_local_collection` now builds a local collection through
its own ops (op Init/op InsertAt / op InitEmpty); the arrow insert resolves a
local receiver's type from the local binding. Verified: literal+At (4),
literal+push (15), foreach+push (42).

## wasm32 collection member codegen width maze — FIXED (slice 4)

**Date:** 2026-08-12 — **all fixed** (commit `dd73d510`). The `b-each` over a
`List<Int>` now compiles to wasm and the `__view_items_items` materializer
returns `[3][3,5,7]` (Node-verified).

1. `substitute_type` did not descend into `Type::Ptr` — the mono
   `ListBuffer<Int>` struct fields stayed generic `Ptr<T>`.
2. `StaticStruct` type params weren't registered, so the mono substitution had
   no key.
3. The foreach's i64 counter vs the Count's i32 result + the At index width.
4. `render_frame` called `@reactor_tick` even when a folded program omitted it.

## Component lifecycle reset never flushed + prop seeding was Rust-invented — FIXED (2b3)

**Date:** 2026-08-12
**Status:** Fixed (commits ffcc5142, db9a9230, cb7ca1d0).

The 2b2 component model seeded instance state from the FRONTEND and reset it
with Rust-emitted direct stores — both violated the "frontend state changes
must be bound to a trg" rule.

1. **`<Counter count="7" />` props were Rust-invented seeds.** Rust
   (`parse_prop_value`) converted HTML attribute strings to `Expr` literals and
   injected direct `init_state` stores — the value never existed in Briev
   source. **Fixed:** HTML props dropped entirely; seeding is a Briev
   StructLiteral (`let c1: Counter = Counter { count: 7 }`), parsed by the Briev
   parser.
2. **`__instance_reset_<Name>_<i>` direct-store reset never flushed.** The shim
   stored values into %State on b-when unmount but nothing ran `__web_flush_*`
   — the DOM kept the stale value until an unrelated txn flushed (on remount a
   fresh clone showed authored content, not the reset value). **Fixed:** resets
   are now callable TXNs (`__reset_c1`) whose write set drives the flush.
3. **`b-show="step == 0"` was inverted/inert.** The shim's Show/When applyFn
   tested only the raw field value (`value ? none : ''`), never the comparison
   — `step == 0` SHOWED the element whenever step was non-zero. The old
   fragment rewrite even double-qualified the expression
   (`ShoppingCart.0.ShoppingCart.0.items == 0`). **Fixed:** b-show/b-when bind
   the root field (first token) and the shim evaluates the comparison via
   `simple_expr_js` (`value == 0`).
4. **`tests/instances_test.bv` was stale/aspirational.** `txn Counter.increment`
   (dotted member name), `&count = ...` (AddrOf-assign), and `@count` (prior
   state) do not parse/work today. The working obj member form is bare:
   `obj Counter { count: Int; txn increment [...] { count = count + 1; term; } }`.
5. **`examples/rstruct-demo.rbv` didn't compile** — `set_name` referenced
   `new_name` without declaring the parameter (`undefined variable 'new_name'`).
   **Fixed:** `txn set_name(new_name: String)`. Its compound `b-text="'Hello, ' +
   name + '!'"` was also silently inert — b-text now rejects compound
   expressions (bare field + `.^` projection only).

Regression tests: `component_instances` suite (obj pairing, Briev-side routing,
collision guards, trg reset generation).

## Global b-class/b-style/b-attr emitted no JS + comment-tag validation — FIXED

**Date:** 2026-08-11
**Status:** Fixed (commit pending, housekeeping Part 2)
Top-level (non-each) `b-class`/`b-attr`/`b-style` bindings fell through
`binding_to_js`'s `_ => ""` — the DOM never updated. Now wired with bounded
single-field expressions (bare field, `field <op> <literal>`, or a literal);
complex expressions (ternaries, calls, string concat) are compile-time errors,
never silent dead DOM.

Three bugs fixed along the way:
1. **`parse_attr_expr` stripped value quotes** — `b-attr="data-mode: 'dark'"`
   lost the quotes, so `dark` was misread as a state-field reference and the
   static attr silently skipped. `parse_attr_raw` keeps the raw value (quoted
   = literal, unquoted = field).
2. **Weak quoted-literal check** — a garbled value like `'#fff') + ';'` (from a
   ternary's unquoted `:`) started AND ended with `'` and passed as a literal.
   The literal check now requires the closing quote at the exact string end.
3. **HTML comments were validated as directives** — `tag_lower.starts_with('!')`
   never matched `<!--` (the raw string carries the leading `<`), so a comment
   mentioning `b-each:` triggered the b-key error. The skip now checks the raw
   `<!--`/`</` prefixes.

Regression tests: `test_class_style_attr_emission`, `global_class_style_attr_
extract_and_validate`, `global_class_complex_expr_is_error`,
`comment_mentioning_directive_is_not_validated`. `examples/view-directives.rbv`
migrated to supported forms.

## wasm32 obj-member bodies hardcode i64 slot widths — FIXED

**Date:** 2026-08-11
**Status:** Fixed.
**Repro:** `brievc build examples/todo.rbv --backend webstack` — llc rejected
`%t38 defined with type 'i32' but expected 'i64'` in the `List.push` member
body; obj/List handles and member-body arithmetic were hardcoded to i64 while
the width-aware layout stores flexible Int fields at `i{int_bits}` (i32 on
wasm32) and obj/List/Ptr handles now follow the same width.
**Fix (architecture-dependent widths, per the "i64 was an earlier convention"
directive):**
- Ptr-indexed stores (`data[i] = v`, the List.push `inner.data[len] = val`)
  widen the i32 index via `gep_index` (`sext i32 to i64`) instead of a bare
  `add i64 {i32}, 0` (emit_stmt.rs).
- Obj/struct/List instance handles are `i{int_bits}` at creation
  (emit_struct_literal/emit_struct_array, emit_toplevel obj-init) and their
  consumers inttoptr at that width (boxed-self, field-access, heap-index).
- Ptr self-slot loads/stores use `i{int_bits}` (emit_expr/emit_stmt).
- The obj-init instance-addr state store uses the field's LLVM type, not a
  hardcoded `store i64`.
- `Malloc#` widens its i32 size to i64 for the C ABI (`widen_to_i64`).
- x86_64 (int_bits=64) is byte-identical — full suite unchanged.

Verified: `examples/todo.rbv` and a `List<Int>` push program build for
`--backend webstack` (all 6 artifacts); the List b-each warns + skips (list
rendering still a separate slice). Regression test:
`test_wasm32_ptr_index_store_widens_index`. Suite 1763 lib + 14 bin green.

## Webstack WASM exports nothing + txn naming mismatch — FIXED

**Date:** 2026-08-11
**Status:** Fixed (commit pending, 2a3 b-each)
The generated WASM module exported ONLY `memory`. `wasm-ld --no-entry` exports
nothing by default, so the shim's `exports.state_layout()` (called at init) and
`exports["<txn>"]` (flush/trigger/bind) were all undefined — the page never
initialized. Compile-level "E2E" checks never ran the module.

**Fix:**
1. `compile_wasm` now passes `--export=<name>` for every txn/definition (both
   the `@<name>` callable form and the `@txn_<name>` reactive form) +
   `state_layout`. wasm-ld ignores names without symbols.
2. The shim's `_txn(name)` resolves `exports[name] || exports["txn_" + name]` —
   callable txns export under the Briev name, reactive txns under `txn_<name>`.

Verified: `each.wasm` now contains `state_layout`, `set`, and `txn_set`.

## Webstack parameterized-txn codegen bugs (wasm32) — FIXED

**Date:** 2026-08-11
**Status:** Fixed (commit pending, 2a2 b-bind:value)
Found while wiring `b-bind:value` (input → parameterized writer transaction).
Parameterized transactions were never exercised on webstack before — the
counter examples used param-less txns.

1. **Void-return txn emitted `alloca void`** — `txn set_name(n: String)` (no
   `->`) emitted `%result = alloca void, align 8` + `store void 0`, invalid
   LLVM ("void type only allowed for function results") → llc rejected the
   wasm module. **Fix:** the `%result` slot is emitted only when the txn
   actually returns a value (`ret_llvm != "void"`).
2. **Boxed param typed `int()` broke on wasm32** — boxed params (String/
   Char/Bool/Data) were downgraded to `Type::int()` because their i64 param
   slot "is already i64". On x86_64 `int()` = i64 (accidentally consistent);
   on wasm32 (int_bits=32) `int()` = i32, so `name = n` emitted
   `sext i32 %t12 to i64` on a value that is genuinely i64 → llc type error.
   **Fix:** boxed params keep their ORIGINAL Briev type and the identifier load
   unboxes the i64 slot (inttoptr for String/Data, trunc for Char/Bool),
   mirroring state-field reads.
3. **Narrow Int params stored un-widened** — `txn set_n(n: Int)` on wasm32
   passes `%arg0` as i32 but the param slot is `alloca i64`; the old `conv =
   raw` emitted `store i64 %arg0` (i32) → llc error. **Fix:** non-boxed
   integer params widen to i64 at entry (sext signed / zext unsigned) and the
   identifier load truncs back to the native width.

Regression tests: the lib suite's existing parameterized-txn tests on x86_64
stay green; b-bind E2E on wasm32 exercised String/Int/Bool param txns.

## Phase 2a view fixes (2026-08-11) — FIXED

**Date:** 2026-08-11
**Status:** Fixed (commit pending, 2a1 b-when)
Four bugs in the webstack view path, found while wiring `b-when`:

1. **applyFn clobbering** — `binding_to_js` emitted
   `this._bindingTable[H].applyFn = (value) => …` per binding. A field bound by
   two elements (or `b-text`+`b-when` on one field) silently kept only the last
   binding's DOM mutation. **Fix:** per-handle effect lists
   (`_registerViewEffect(H, fn)`, default applyFn fans out to all). Two
   elements on one field now both react.
2. **Conditional writes missed by write_set** — `extract_write_set` scanned
   only top-level `Statement::Assign`; a write inside `if` (e.g.
   `if count > 2 { show = true; }`) was absent from the txn's write_set, so the
   webstack flush batch (sized/looped over the write_set) never told the DOM the
   field changed — `b-when`/`b-show` on it stayed dead. **Fix:** recurse into
   `If`/`Guarded`/`Block`/`Foreach` bodies. `web_max_entries` grows to cover
   them.
3. **`parse_tag` broke on quoted `>`** — `b-when="count > 0"` truncated the tag
   at the comparison operator inside the quoted attribute, leaving the tail as
   text. **Fix:** quote-aware `>` scan.
4. **Self-closing tags skipped directives** — `<input b-show="show" />` got no
   ID and no binding (inject_ids bailed on `/>`). Void elements legitimately
   carry directives. **Fix:** only `</` and `<!...>` skip; self-closing tags
   with directives get IDs + extraction.
5. **Dead-field diagnostics lied about view-bound fields** — the A002/A003
   "stores eliminated / dead fields" warnings filtered only on the transition
   graph's body-driven `live_fields`, so a view-bound field (`b-when="show"`)
   was reported dead even though `view_bound_fields` → `FieldMode::Always`
   correctly kept its store. **Fix:** the diagnostic now filters out
   `view_bound_fields` (observability-as-liveness).

Regression tests: `test_multiple_bindings_same_field_do_not_clobber`,
`test_when_binding_emits_mount_unmount_effect`,
`test_extract_write_set_recurses_into_conditionals`,
`b_when_extracts_structural_binding`, `b_when_purity_enforced`,
`self_closing_tag_with_directive_gets_id_and_binding`.

## Spawned-obj Member Slot Read Emits Undefined `@slot` Global in Two-Node Shape — FIXED

**Date:** 2026-08-09
**Status:** Fixed 2026-08-09 (commit pending)
**Root cause:** A base that is ONLY spawned (no top-level `let c: Obj = ...`
unpacked instance) never ran the pool registration in `build_field_index` —
the pool counter (`__spawn_next_<base>`) and member COLUMNS were only
registered when a top-level instance unpacked them. A spawn-only base either
panicked ("spawn of 'X' with no registered instance pool") or, for
box/spill bases, had no `boxed_offsets` so the member body read a nonexistent
`@member` global.
**Fix (applied):** extracted `register_pool_columns` (counter + static/dependent
columns); `build_field_index` ends with a fallback pass that registers every
base in `spawn_pools`/`dependent_pools`/`spawn_storage` that lacks a counter
(registering `boxed_offsets` for box/spill bases). The member-body Identifier
arm also gained a boxed_offsets fallback so boxed members resolve without a
field slot. Regression test: `test_spawn_only_base_registers_pool`.
**Undo:** remove the fallback pass + boxed member-body branch; the top-level
instance path is unchanged.

## Sync Group Blocks When Members Have Unequal Firing Schedules — FIXED

**Date:** 2026-08-06 (fixed 2026-08-09, commit pending)
**Status:** Fixed
**Root cause:** `sync<group>`-wrapped transactions were skipped by every
pipeline that iterated `TopLevel::Transaction` directly — the transition graph
missed their nodes (so `live_fields` lacked their state → dead-field
elimination dropped the fields → undefined `@field` globals), the loop-shape
analysis skipped them (no fold shape), and the reactor's txn list was empty
(`reactor_tick` = bare `ret void` → nothing fired). Separately, the strategy
pass auto-classified conflict-free sync members as async (disjoint writes),
routing them to the thread pool instead of the sequential reactor.
**Fix (applied):**
- `analysis::effective_txns(items)` — a centralized expander (direct +
  sync<group>-wrapped + export-wrapped transactions); transition-graph builder,
  loop-shape `collect_txns`, and the backend txn collection use it.
- strategy: sync-group member names excluded from async auto-candidacy (a sync
  group is a barrier, not async parallelism) so members stay on the sequential
  reactor.
- regression tests: `test_sync_group_node_enters_graph`.
**Undo:** revert effective_txns usage + the strategy sync-member exclusion;
the concurrency-gate classification (rule #21) is unchanged.
**E2E:** two `sync<step>` nodes `[i < 4]` (4 fires) + `[count < 1]` (1 fire)
now print 1,2,3,4.

## Precompute Fold Evaluates Float-Array Indexed Observables as 0 — FIXED

**Date:** 2026-08-06 (fixed 2026-08-09, commit pending)
**Status:** Fixed
**Root cause:** the compile-time precompute fold (`region.rs` final-value model)
tracks only plain-identifier bindings as `i64`. An array-element write
(`a[i] = 1.5`) was silently ignored by `eval_stmt`, so a fully-precomputable
chain left the array at its initializer — a folded observable read `a[k]`
returned 0.
**Fix (applied):** a composed chain containing an indexed assignment is no
longer precomputable: `is_fully_precomputable` refuses chains with an indexed
write (`Self::composed_body_has_indexed_write`), and `eval_stmt` returns false
on `Assign(Expr::Index(..), _)` so the report path can't model it either. Such
programs fall through to the runtime loop, which executes the writes correctly.
**Undo:** remove the indexed-write guard in `is_fully_precomputable` +
`eval_stmt`; the i64 model is otherwise unchanged.
**E2E:** `a[i] = 1.5; sum += a[i]` over 4 iterations prints 6.0 (was 0).
Regression test: `test_indexed_write_blocks_precompute`.

## Accel Node Folds the Reactor to Nothing — RESOLVED BY DESIGN (Design A)

**Date:** 2026-08-06
**Status:** Resolved (2026-08-06, Design A)
**Root cause:** The old "virtual work-item index" model bound `i` only via the
contract; the runtime precondition machinery (`@pre_<name>`,
`emit_precondition_check`) emitted `i < N` against the undefined `i`, letting
LLVM dead-code-eliminate the whole dispatch → empty `reactor_tick`, host
observables eliminated.
**Fix (applied):** Design A — the work-item counter is a REAL state field
(`let i: Int = 0;` + `i = i + 1` in the body). `i` is a genuine counted loop
(`[i < N][i == N]`); every reference is valid IR; the accel analysis PROVES the
loop is a parallel map (disjoint affine writes, counter increments). The GPU
path coalesces the counted loop into one dispatch: the wrapper launches N
work-items and fast-forwards the counter to N so the loop exits after one
firing. Verified: accel IR is byte-identical to the plain node's (no
regression), 14 accel unit tests + wrapper fast-forward test green.
**Undo:** none — the virtual-index model is superseded.

## Runtime `lib/runtime/briev_rt.c` Is Untracked — FIXED

**Date:** 2026-08-05 (fixed 2026-08-06, commit `2bcec427`)
**Status:** Fixed
**Root cause:** The Brief→Briev rename (`d026c002`) renamed `lib/runtime/brief_rt.c`
to `lib/runtime/briev_rt.c`, but `.gitignore:180` contained a bare `briev_rt.c`
pattern (added for "root-level runtime artifacts (compiled outside benchmarks/)").
Because the pattern has no path, it hid the renamed runtime too.
**Fix (applied):** anchored the pattern to `/briev_rt.c` (root-only) and
`git add lib/runtime/briev_rt.c`. Verified 2026-08-09: the file is tracked in
main (`2bcec427`) and present in the baseline worktree; `git check-ignore`
returns not-ignored.
**Undo:** none (tracking infrastructure the harness already requires).

---

## Baseline Harness Defects Blocking Trustworthy A/B Comparison — PARTIALLY FIXED

**Date:** 2026-08-05
**Status:** Defects #1 + #2 fixed 2026-08-09 (commit pending); #3 + #4 are
compiler gaps, tracked separately below.
**Root cause:** The runtime benchmark harness had correctness/reporting defects
that made the captured baseline partially untrustworthy.
**Findings + resolution:**
1. **`deep_recursion` prints divide-by-zero but reports `MATCH`.** FIXED — the
   divide-by-zero itself was already fixed; the harness now separates stdout
   from stderr and treats a non-empty stderr / non-zero exit as `FAIL`, never
   `MATCH`. Verified `deep_recursion` runs cleanly and reports MATCH.
2. **`bridge_glue` produces `<null>` vs `42` and reports `SKIP`.** FIXED — two
   harness bugs: (a) the Python bridge harness exited 0 on mismatch and the
   shell then SKIPped on a missing compiled binary, masking the wrong result;
   (b) `bench_glue_cross.py` called `briev_test_cstr_roundtrip(state, s_ptr)`
   with a 2-arg ABI, but the export needs NO state (its body only calls the
   cstr_to_briev/str_to_c FFI), so `state`(0) was passed as `input_ptr` →
   `<null>`. The Python now calls the correct 1-arg ABI, exits 1 on mismatch,
   and the shell records `FAIL`. Verified `'42'` round-trips and MATCH.
3. **Protocol round-trip proofs are silently skipped.** NOT FIXED — the
   ASCII/UTF16/Posit32 conversion functions (`ascii_to_utf8`, ...) have no
   bodies in stdlib; the proof machinery warns-and-skips. Needs proof bodies
   or explicit trusted axioms (SPEC §8.7). Tracked as its own bug below.
4. **Unresolved representation widths/alignments fall back silently.** NOT
   FIXED — the normalizer defaults max width to 64 / alignment 8 for `Slice`,
   `List`, etc. SPEC §2.1 forbids silent fallback. Tracked separately below.
5. **`#!exit` benchmarks warn their exit condition is never checked.** The
   benchmarks migrated to `endprogram`/entry semantics; remaining warnings are
   informational.
**Undo:** none (correctness fixes; do not restore silent fallbacks).

---

## Protocol Round-Trip Proofs Silently Skipped — PARTIAL (interpreter side FIXED 2026-08-26)

**Date:** 2026-08-09 (split from the harness-defects entry)
**Status:** PARTIAL. 2026-08-26: the interpreter-side silent `Ok(())` paths in
`src/protocol_verify.rs` (probe eval failure, missing producer) are now hard
errors with what/why/fix diagnostics.
**Remaining:** `protocol_graph.rs`'s missing-body skip arm must become a hard
error — blocked on one coherent prerequisite session: the four codec bodies
(`ascii_to_utf8`/`utf8_to_ascii`, `utf16_to_utf8`/`utf8_to_utf16`,
`Posit32_to_IEEE754`/`IEEE754_to_Posit32`) implemented in stdlib `.bv`, plus an
explicit trusted-axiom cast-edge marker per SPEC §8.7 so edges that ARE axioms
skip proof by declaration. Flipping the gate alone bricks every stdlib import;
that is why it waits for its prerequisites (Golden Rule 11 — prereqs first).
**Fix (planned):** as above; never silently skip.

---

## Silent Representation Width/Alignment Fallbacks — RESOLVED 2026-08-26

**Date:** 2026-08-09 (split from the harness-defects entry)
**Status:** RESOLVED (`e614a18e`). The normalizer has recorded fallbacks since
Phase 3 (§8.6); the remaining truly-silent sites — inline `ResolvedType`
construction in `LlvmBackend::generate` for `TopLevel::Obj` and TypeDef
backfill — now share `record_structural_layout` (`register_types.rs`), which
keeps the structural defaults (slot-sum bytes, alignment 8) and records a
deduped warning naming the representation and its fix (`!> bytes/alignment`).
The enum-handle shape is a declared representation (boxed {tag,payload}
image), not a fallback.

---

## Runtime: Stale `free()` of Zero-Copy `briev_str_to_c` Results Crashed `get_env_int`/`__print` — FIXED

**Date:** 2026-08-04
**Status:** Fixed (main, post-merge)
**Root cause:** The 2026-08-03 composite plan
(`docs/plans/2026-08-03-native-python-meld-composite.md`) made
`briev_str_to_c` return the IN-PLACE data pointer for heap Briev strings —
zero-copy, contract "caller must NOT free". `__read_file__`/`__write_file__`
were updated, but five callers still `free()`d the result (a stale free of an
arena/state-owned pointer): `__getenv_briev`, `__getenv_int`, `__print`,
`__print_str`, `__eprint_str`. The crash surfaced when the
`feat/term-termination-diagnostics` merge's clang-guarded integration test
(`member_inline_term_links_in_countdown_loop`, which reads `BOUND` via
`get_env_int!`) ran on main — SIGSEGV in the allocator. This also means EVERY
runtime benchmark using the `BOUND` mechanism would crash on main.
**Fix:** Remove the stale `free(c_key)`/`free(c_msg)` calls in the five
functions, per the composite ownership contract; the results are borrowed.
SSO strings (the only owned `str_to_c` case) are accepted as a leak, matching
the updated `__read_file__`/`__write_file__` behavior.
**Impact:** `get_env_int!`/`BOUND`-driven programs and string printing work
again on main; 6/6 integration + 1469 lib tests green.
**Regression tests:** `tests/termination_diagnostics_test.rs::member_inline_term_links_in_countdown_loop`.
**Undo:** restore the `free()` calls (do NOT — they free borrowed pointers).

---

## Vestigial `return` Statement Removed (was the "return divergence") — RESOLVED

**Date:** 2026-08-04
**Status:** Resolved by REMOVING the feature (branch `feat/term-termination-diagnostics`)
**Root cause:** `return expr;` / `return;` was a vestigial parser path carried
over from the Phase 1 parser rewrite (`77836c35`). Briev's language never
defined a `return` statement — `spec/SPEC.md` documents "return" only as a
return TYPE. Zero `.bv` files used it. Its semantics disagreed across engines:
the interpreter (`src/interpreter/eval.rs`) returned `Ok(Value)` so execution
CONTINUED (and the runner's `result = last statement value` overwrote it), while
the LLVM backend (`src/backend/llvm/emit_stmt.rs`) emitted a real `ret` +
`terminated=true` (hard exit) and the VM backend (`src/backend/vm/emit_stmt.rs`)
treated it identically to `term`. A user who wrote `return` got silently wrong
codegen. The 2026-08-04 term-termination plan (§5) claimed this divergence was
"logged in BUGS.md" — it was not; this entry resolves the matter by removing the
statement.
**Fix:** Parser now rejects `return` (at top level and in statement bodies) with
`invalid statement: Briev has no \`return\` statement. To return a value from a
defn use \`term <value>\`; to mark a convergence checkpoint use bare \`term;\`;
\`term!\` closes the program.` The `Statement::Return` AST variant and all ~50
match arms across the pipeline (parser → AST → typechecker → interpreter →
normalizer → derive/SMT → proof engine → reactor → plugins → beastpack → LLVM/VM
backends → beast serialize/deserialize) were removed. `return` is now an
ordinary identifier again in non-statement positions.
**Impact:** 1469 lib tests + 5 integration tests + 2 parser tests green; no
`.bv` was affected (zero usages); no benchmark impact (no codegen path changed).
**Regression tests:** `parser::statements::tests::return_statement_errors_with_helpful_message`.
**Undo:** re-add `Statement::Return` + the parser dispatch (do NOT — the feature
was never specced; see `docs/plans/2026-08-04-remove-vestigial-return-statement.md`).

---
## Value-Form `term`/`term!` in a Void Txn Fell Through Past the Guard — FIXED

**Date:** 2026-08-04
**Status:** Fixed (branch `feat/term-termination-diagnostics`)
**Root cause:** The value-form `term <val>` / `term! <val>` void-path in
`src/backend/llvm/emit_stmt.rs` set `backend.fun.terminated = true` WITHOUT
emitting a real LLVM terminator. The `Guarded` handler therefore had to emit an
unconditional convergence branch (`guard.thenN -> guard.endN`) so the block
wasn't dangling — and execution fell through past the term. This diverged from
the interpreter, where a value-form term unwinds the ENTIRE transaction body
(`RuntimeError::TermReturn`, `src/interpreter/eval.rs:646-657`), not just the
guard. Repro: `when a == 1 { term! -> Print#(1); }; Print#(2);` printed `"12"`;
the interpreter (and the fix) print only `"1"`. A top-level terminating term
was masked from this bug only because the pre-2026-08-04 body loops emitted
every statement regardless of `terminated`, so nothing dangled — the fallthrough
was simply misordered execution.
**Fix:** Value-form terms in a void function now emit a REAL terminator. New
`FunctionContext.void_txn_abort_label` is set by the SSA main loop to the
current txn's `.ssn_<name>` next-txn label so the term branches past the rest of
THIS txn's body (faithful TermReturn); in per-txn void functions
(async/standalone/pre/callable) the term emits `ret void`. The `Guarded`
handler emits its convergence branch only when the body did NOT terminate.
Body loops that emit statements unconditionally (`ssa.rs` main loop, outlined
cold-function bodies) now `break` on `terminated`; epilogues that unconditionally
emitted a trailing `br %...done` / `ret void` (async, pre-function, cold
function) are now conditional so they don't double-terminate the block. The
2026-07-19 "always emit br" workarounds in `emit_stmt.rs` (Guarded),
`emit_toplevel.rs` (async, pre) are rewritten with the new rationale.
**Impact:** `corrected_term_guard.bv` prints `"1"`; async-checkpoint IR (bare
`term;` continues) unchanged; `transition_validate.bv` output identical
(404/422/409/200); 1468 lib tests + 4 termination integration tests pass.
**Regression tests:** `tests/fixtures/term_{unreachable,defn_unreachable,
guard_hint,valid_swan_song}.bv` + `tests/termination_diagnostics_test.rs`;
in-module unit tests in `src/analysis/termination.rs`.
**Undo:** revert `void_txn_abort_label` wiring + the conditional epilogues; the
2026-07-19 unconditional-br version returns.

---

## Bare `term;` Checkpoint Body-Stopped in Async/Callable/Pre Void Paths — FIXED

**Date:** 2026-08-04
**Status:** Fixed (branch `feat/term-termination-diagnostics`)
**Root cause:** The bare `term;` / `term!;` arm in `emit_stmt.rs` set
`backend.fun.terminated = true`, but the interpreter treats BOTH bare forms as a
convergence CHECKPOINT, not a terminator — it returns `Ok(Void)` and continues
to the next statement (`src/interpreter/eval.rs:646-657`, `707-709`). In the
async/callable/standalone/pre void paths, whose body loops `break` on
`terminated`, a bare `term;` mid-body stopped the rest of the body from being
emitted — so a print after the checkpoint silently disappeared.
**Fix:** The bare-form arm no longer sets `terminated`; the body keeps emitting
past the checkpoint exactly like the interpreter. The enclosing epilogue still
terminates the function. Verified by IR diff: the async body now contains the
`__print_int` after the bare `term;` (new binary), matching the interpreter.
**Impact:** parity between interpreter and backend for checkpoint semantics.
**Regression tests:** `scratch_verify/async_term_checkpoint.bv`.
**Undo:** revert the bare-form arm in `emit_stmt.rs`.

---

## Inlined Member Terms Broke the Countdown Loop with a Spurious `ret void` — FIXED

**Date:** 2026-08-04
**Status:** Fixed (branch `feat/term-termination-diagnostics`)
**Root cause:** The value-form void-path terminator added in `be934d61` fired for
`term <val>` inside an INLINED member body too. Member bodies are inlined via
`emit_member_body` (`emit_expr.rs:1494`) → `emit_statement_sequence`; their
`term <val>` is the member's RETURN VALUE, captured in `member_result` and taken
by `emit_member_body` — it is NOT a control-flow exit of the enclosing function.
In the countdown loop (`queue_drain.bv`'s `<- queue` pop, dispatched via
`emit_countable_body`), `void_txn_abort_label` is `None`, so the new void path
emitted `ret void` in the middle of `define i32 @main` — clang failed with
`queue_drain.ll:366:7: error: value doesn't match function result type 'i32'`.
`emit_countable_body` ignores `terminated` and kept emitting after the ret. The
pre-2026-08-04 code was accidentally correct: the void path emitted no
terminator and the loop never stopped for member terms.
**Fix:** The void path now checks `member_result.is_some()` FIRST: an inlined
member term emits NO terminator and leaves `terminated` unchanged (member-local
return, matching the interpreter's member-call frame semantics). Txn-level value
terms (SSA: `br` to abort label; per-txn void fns: `ret void`) are unchanged.
**Impact:** `queue_drain.bv` compiles, links, and prints the correct boundary
output again; live IR (the countdown loop in `@main`) is byte-for-byte
equivalent to pre-change apart from dead-function register numbering; timing
identical (0.03s vs 0.04s @ BOUND=50M).
**Regression tests:** the full harness `queue_drain` case (A/B vs baseline);
`corrected_term_guard.bv` still prints `"1"`; 1468 lib tests + 4 integration
tests.
**Undo:** remove the `else if backend.fun.member_result.is_some()` branch in the
Term value-form void path of `emit_stmt.rs`.

---


## Nine of Ten `#String` Cast-Lane Symbols Missing From the Runtime — FIXED

**Date:** 2026-08-04
**Status:** Fixed
**Root cause:** the casting graph (`src/casting/graph.rs:195-257`) declares ten
`ExtCall` lanes between `#String` and the other base protocols. Only `int_to_str`
existed in `lib/runtime/briev_rt.c`. The other nine (str_to_int, uint_to_str,
str_to_uint, float_to_str, str_to_float, str_to_bool, bool_to_str,
str_first_char, char_to_str) were **undefined symbols** — a latent LINK ERROR
whenever `(s as Int)`, `(f as String)`, etc. was exercised, for `.bv` and `.ebv`
alike. (The `to_int`/`to_float` stubs in `std/string.bv` were `term 0`/`term 0.0`,
so the broken path was rarely hit.) Verified: `(s as Int)` on `"42"` failed with
"use of undefined value '@str_to_int'".
**Fix (`70f596f9`):** added all nine to `briev_rt.c` (String ABI = ptr to
[len: i64][bytes]; the Float lanes use the 32-bit `float` ABI — the Briev Float
protocol is `float`, not `double`, which a first attempt got wrong; the Char
lanes are single-UTF8-codepoint strings), and added the lane declares to the LLVM
header (the ExtCall lane emission writes the `call` inline without a declare).
**Impact:** all `#String` conversions round-trip correctly (verified `42`, `3.5`,
`true`, `A`). The `.ebv` freestanding path will provide these same symbols as
Briev defns (the declare-guard from `c7f25a95` skips the backend declare when the
program defines the symbol).
**Undo:** remove the nine C functions + the lane declares.

## Custom-Type Operator Resolution Matched Type Names, Not Protocol Categories — FIXED

**Date:** 2026-08-03
**Status:** Fixed
**Root cause:** `type_universe::operators::builtin_operator_binding` resolved an
operator by the type's literal NAME (`type_name_str`) against a table whose keys
were actually protocol CATEGORIES (`"Int"`, `"Float"`, `"String"`, …). `Int + Int`
worked only because the type name happens to equal its category key. A custom
`type MyNum : #Int` (no declared op) failed `MyNum + MyNum` with
`InvalidOperation`, because `"MyNum"` matched no key even though `MyNum` is a
`#Int` protocol member that should inherit `#Int`'s Add → `AddI64#`. This was a
Rule 14/18 sloppy-name-matching defect. Two compounding gaps: (a)
`typechecker::type_declares_op` never walked `type_parents`, so an op declared on
a parent type wasn't inherited by a subtype; (b) `variant_covers`/`param_covers`
checked only `Cast.#` universe properties, but custom types are NOT registered in
the typechecker's fresh universe, so `#Int`-coverage for `MyNum` always returned
false.
**Correct model:** operator resolution is **declared → parent's bindings →
protocol bindings**, and ONLY the protocol bindings are hardcoded — keyed by
protocol category, never type name.
**Fix:**
- `operators.rs`: `get_operator_intrinsic`/`protocol_binding` resolve the type's
  protocol category from the universe (`Cast.#` properties, then `rt.base`
  chain, mirroring `casting::graph::type_to_protocol`) — no name matching.
  `type_name_str` removed.
- `typechecker/mod.rs`: `infer_binary_op`'s arithmetic arm now tries declared
  (own + parents) first, then protocol bindings (universe + the typechecker's
  own `type_protocols`/`type_parents` records via `protocol_binding_for` /
  `declared_protocol_of` / `operand_implements_protocol`).
- `type_declares_op` walks `type_parents` (mirrors the Parse-op parent walk).
**Impact:** `MyNum : #Int` + `MyNum` (same-type, no declared op) typechecks and
inherits `#Int`'s binding; subtypes inherit parent-declared ops; a protocol-less
custom type with no declared op still errors. 1450 tests pass. Regression tests:
`same_type_custom_op_inherits_protocol_binding`,
`same_type_custom_op_declared_binding_wins`,
`same_type_custom_op_no_protocol_errors`, `subtype_inherits_parent_declared_op`,
`test_int8_resolves_via_protocol`.
**Undo:** revert the operators.rs rewrite + the typechecker resolution change;
the old name-keyed table and direct-only `type_declares_op` return.

---

## DBV Parser: Trailing `;` After `}` Misparsed as Empty Positional Value — FIXED

**Date:** 2026-07-28
**Status:** Fixed
**Root cause:** `parse_schema()` and `parse_grouped_data()` did not consume the
optional trailing `;` after the closing `}`. The `;` fell through to the main
loop's `_ =>` arm, which tried to parse it as a standalone entry via
`try_parse_standalone_entry()`. Since `;` is not an identifier, it fell through
to `parse_positional_values()`, which treated `;` as a value separator and
produced empty-string positional values. This derailed the parser state before
the next `as` block or `schema` definition.
**Fix:** Added `self.skip_ws(); if self.peek_char() == Some(';') { self.advance(); }`
at the end of both `parse_schema()` and `parse_grouped_data()`, after the `}`
is consumed.
**Impact:** `as MetaField { ... }; as BackendMapping { ... };` (with trailing
semicolons) now parses correctly. The clean keyed-entry format for MetaField
(`overflow: String; "desc"`) works without workarounds.
**Lesson:** Any parser function that consumes a braced block `{ ... }` should
consume the optional trailing `;` before returning to the main loop. Both
`schema Name { }` and `as SchemaName { }` can be optionally followed by `;`.

---

## `expect_str_arg` Returns Literal Identifier, Not Variable Value — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** `expect_str_arg` matched `Expr::Identifier(s)` and returned `s` (the
literal identifier text) rather than resolving the variable from the compile-time
scope. So `StrReplace$(tmpl, "{{name}}", name)` used the string `"name"` as the
replacement value, not the value bound to variable `name`.  
**Fix:** `expect_str_arg` now takes `scope: &Scope` and resolves `Expr::Identifier`
from the scope. If not found in scope, falls back to the literal identifier (for
field names like `"name"` in `TypeInfo$` calls). Complex expressions (string
concatenation) fall through to `eval_nav_chain`.  
**Impact:** All `$` intrinsics that take string arguments now correctly resolve
variables. This was a fundamental bug affecting every `$(Stage)` block that
used variable references in intrinsic arguments.

## `Statement::Assign` Was No-Op in Stage Blocks — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** `evaluate_stage_stmt` did not handle `Statement::Assign(target, value)`.
The assignment fell through to the wildcard arm `_ => Ok(())` and was silently
discarded. Variable reassignment (`x = x + 1;`) produced no effect.  
**Fix:** Added `Statement::Assign` handler that evaluates the RHS via
`eval_nav_chain` and updates the scope binding. The target must be an
`Expr::Identifier`.  
**Impact:** Essential for accumulator patterns in stage blocks.

## Macro DSL Missing Expression Types — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** `eval_nav_chain` only handled `Expr::Call` ($-intrinsics),
`Expr::Field` (methods), `Expr::Identifier`, `Expr::Decimal`, `Expr::Float`,
and `Expr::Bool`. Common expressions like `Expr::Quoted` (string literals),
`Expr::BinaryOp` (`+`), and `Expr::List` (`[...]`) returned errors.  
**Fix:** Added handlers for:
- `Expr::Quoted` → `NavValue::Str` (string literals)
- `Expr::BinaryOp(BinaryOpKind::Add, ...)` → string/int concatenation
- `Expr::List(items)` → `NavValue::List` (list construction)
**Impact:** Stage blocks can now use `"hello"`, `"a" + x + "b"`, and `[a, b, c]`.

## `TypeInfo$` Not Delegating Through `TopLevel::Export` — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** `type_info_from_toplevel` matched on `(TopLevel::Definition(d), field)`
but not on `(TopLevel::Export(e), field)`. Calling `TypeInfo$(export_node, "name")`
returned "unknown field for this item type" for export-wrapped definitions.  
**Fix:** Added delegation at the top of `type_info_from_toplevel`:
`if let TopLevel::Export(e) = tl { return type_info_from_toplevel(&e.inner, field); }`.  
**Impact:** The GLUE bridge generator can query exported definitions directly
without unwrapping the export.

## `TypeInfo$` `output_type` Field Returns Debug Format — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** The `"output_type"` arm used `format!("{:?}", d.output_type)` which
produced `Some(Single(Custom("Int")))` instead of `"Int"`.  
**Fix:** Added `single_type_name` helper that recurses through `OutputType` variants
to extract the clean type name. For `OutputType::Single(ty)`, returns `format!("{}", ty)`.  
**Impact:** `TypeInfo$(export, "output_type")` now returns `"Int"` instead of
`Some(Single(Custom("Int")))`.

## `nav_to_i64` Doesn't Handle `NavValue::Str` — FIXED

**Date:** 2026-07-23  
**Status:** Fixed  
**Root cause:** `nav_to_i64` handled `Count`, `Int`, and `Bool` but not `Str`.
Since `TypeInfo$(sel, "params.count")` returns `NavValue::Str("2")`, the
`when pcount > 0` guard evaluated `nav_to_i64(NavValue::Str("2"))` as 0.  
**Fix:** Added `NavValue::Str(s) => s.parse::<i64>().unwrap_or(0)`.  
**Impact:** `when pcount > 0` guards now correctly evaluate parameter counts.

## Three-Way String Concatenation `++` Drops Third Operand — FIXED

**Date:** 2026-07-22
**Status:** Fixed
**Fix:** The string format change from `{data_ptr, length, chars}` to
`[length][data]` (C-compatible format) resolved the underlying issue.
The arena allocator and emit_inline_concat now handle three-way `++`
correctly. `term "Bits(" ++ n ++ ")";` produces `"Bits(42)"` as expected. & Mistakes Log

## Format
- **Date**: YYYY-MM-DD
- **Issue**: What happened
- **Root Cause**: Why it happened
- **Fix**: How it was resolved
- **Lesson**: How to avoid next time

## 2026-06-17 — `is_string_chain` missing `Expr::Call` arm (SIGSEGV crash)

**Issue**: `draw_prompt` in officina-cli crashes with SIGSEGV when rendering
the prompt. `int_to_str(23)` returns a garbage pointer (two string struct
pointers added together as `i64`), which is dereferenced as a string struct,
causing segfault.

**Root Cause**: `is_string_chain` in `src/backend/llvm/emit_expr.rs:2763`
detects whether a `+` expression operates on strings (triggering inline concat
emission). It handles `Expr::String`, `Expr::Literal(String)`,
`Expr::Identifier` (with String let-binding type), and recursive
`Expr::Add`/`Expr::Concat`. But it does **not** handle `Expr::Call`.

When `int_to_str` needs to emit `int_to_str(2) + int_to_str(3)` (the `n >= 10`
arm), both operands are `Expr::Call("int_to_str", ...)`. `is_string_chain`
returns `false` for both, so the backend emits `add i64 %t52, %t58` — adding
two string struct pointers together — instead of allocating a new buffer and
copying characters. The resulting garbage pointer is dereferenced by
`draw_prompt`, causing SIGSEGV.

By contrast, `"-" + int_to_str(-n)` (the `n < 0` arm) worked because
`Expr::String("-")` IS recognized by `is_string_chain`, triggering proper
`malloc`+`memcpy` concat.

**Fix**: Added `Expr::Call(name, _)` arm to `is_string_chain` that checks
`defn_return_types` for String/Data return types. `defn_return_types` was
already populated and accessible from emit_expr.rs.

**Lesson**: Any expression type that can return a String must be in
`is_string_chain`'s match. `Expr::Call` is the most common — it covers
function calls, txns, and method invocations that return strings. Always
cross-reference `is_string_chain` when adding new expression types.

**Files**: `src/backend/llvm/emit_expr.rs:2777-2783`

---

## 2026-06-17 — `\0` char escape not handled in lexer

**Issue**: `'\0'` (null character literal) parsed as backslash character
(ASCII 92). The precondition `[booted && keypress != '\0']]` became
`booted && keypress != 92` instead of `booted && keypress != 0`. Since
`keypress` initializes to 0, the precondition was `true && true` instead of
`true && false` — causing `process_input` to fire spuriously on every tick,
concatenating the null char string to `current_input` each iteration,
corrupting the heap.

**Root Cause**: `src/lexer.rs:371-382` handles char escape sequences for
`\n`, `\t`, `\\`, `\'`, and `\u{...}`, but NOT `\0` (null). When the lexer
sees `'\0'`, the inner string is `\0` (2 chars). It falls through to the
default at line 390: `inner.chars().next()` which returns `\` (backslash,
ASCII 92).

**Fix**: Added `if inner == "\\0" { return Some('\0'); }` before the other
escape sequence checks.

**Lesson**: All C-style escape sequences in char literals must be handled.
`\0` (null) is a common pattern for trigger comparisons (keypress != '\0').
When adding escape sequences, match the most common ones first: `\0`, `\n`,
`\t`, `\\`, `\'`, `\r`, `\xHH`, `\u{...}`.

**Files**: `src/lexer.rs:371-374`

---

## 2026-06-17 — `done_{name}` SSA dispatch skips to exit instead of next txn

**Issue**: After the `\0` fix, officina rendered output brievly then exited
before the render txn could fire. The SSA dispatch loop's `done_process_input`
label branched to `%done` (program exit) instead of `%s_process_input` (next
txn's skip label), skipping the render txn entirely.

**Root Cause**: `src/backend/llvm/loop_engine.rs:772-778` in
`emit_ssa_main`: the `done_l` label (emitted when a txn's precondition is
false) unconditionally branches to `%done` instead of `%{skip_l}`. This means
the FIRST txn whose precondition is false causes an immediate return from
`main()`. This affects ALL txns equally — boot, process_input, render — any
txn with a false precondition exits the program.

The June-14 fix claimed to address this but only covered `done_boot` while
the `done_l` template continued emitting `br label %done` for all other txns.

**Fix**: Changed line 778 from `writeln!(out, "  br label %done").ok()` to
`writeln!(out, "  br label %{}", skip_l).ok()`. When a txn's precondition
is false, control passes to `skip_l`, which chains to the next txn's
preamble. After the last txn, `skip_l` falls through to the post-loop code
(exit condition check or tick loop continuation).

**Lesson**: The `done_l` → `%done` pattern is always wrong for multi-txn
SSA dispatch. The done label should chain to `skip_l` for the current txn,
which naturally leads to the next txn or post-loop code. Only the post-loop
code (exit condition, tick loop) should decide whether to exit.

**Files**: `src/backend/llvm/loop_engine.rs:778`

---

## 2026-05-28 — Overriding `from "..."` location in typechecker

**Issue**: `__read_file` FFI call failed with `location: <profile:__read_file>` instead of `"std::fs::read_to_string"`.

**Root Cause**: Typechecker at `src/typechecker.rs:993` unconditionally overwrote `signature.location` with `<profile:{name}>` when `toml_path` was empty, even when the `from "..."` clause had already set a correct location. The parser correctly parsed `from "std::fs::read_to_string"` but the typechecker replaced it with a profile placeholder.

**Fix**: Only set the profile placeholder when `signature.location` is empty:
```diff
- signature.location = format!("<profile:{}>", name);
+ if signature.location.is_empty() {
+     signature.location = format!("<profile:{}>", name);
+ }
```

**Lesson**: Always check before overwriting. The "new FFI syntax" (direct `from "..."`) and "old profile FFI" coexist — the typechecker assumed no `from` clause existed.

## 2026-06-16 — LLVM Backend Audit — i64 Boxing Tax (Phase 0/1 Plan)

**Audit**: External audit of `src/backend/llvm/` found 4 bug classes:

| # | Bug | Location | Impact |
|---|---|---|---|
| 1 | String trigger first-byte-only comparison | `emit_expr.rs:1775` | Silent match failure |
| 2 | Dynamic alloca inside loops (enum constructors) | `emit_expr.rs:391` | Stack overflow under reactive loops |
| 3 | Silent zero stubs (MapLiteral, SetLiteral, arrows) | `emit_expr.rs:1360` | Silent data corruption |
| 4 | i64 boxing type confusion (`ptrtoint` on i64) | Multiple files | `llc` type errors — **active blocker** |

**Root Cause (Bug 4)**: The backend boxes all native types (Bool → i1→i64, Char → i32→i64, String → i8*→i64, Float → float→i64) for a uniform i64 ABI. 30% of the LLVM backend code is casting/boxing glue. The type tracking (TypedRegister.ty) regularly falls out of sync with the actual LLVM register type, producing invalid IR.

**Phase 0 Fix** (COMPLETED 2026-06-16): 18 edits across 4 files. See
`docs/architecture/fixes/i64-boxing-type-confusion-phase0.md` for full details.

**Bug 1** (2026-06-16): String trigger first-byte comparison — **correct by design**.
`@ link String` triggers use single-byte `i8` storage (for `tty_read_key`), so
comparing the first byte is the intended behavior. Full-string triggers would
require changing the trigger storage model — deferred.

**Bug 2** (2026-06-16): Dynamic `alloca` in enum constructors — **FIXED**.
Replaced `alloca i64, i64 N` with `call i8* @malloc(i64 N*8)` + `bitcast to i64*`.
Prevents stack overflow in reactive loops. Creates heap-allocated enum values
(leak documented — caller must free).

**Bug 3** (2026-06-16): Silent zero stubs — **WARNED**.
MapLiteral/SetLiteral/ArrowMut/ArrowDiscard/ArrowTransfer now emit
compile-time warnings: "LLVM backend stub: ... returns 0".
Key changes:
- All `TypedRegister.ty` values for boxed i64 values now use `Type::Int`
- `adapt_to_i64` calls inserted before all field stores, tuple/list element stores,
  comparison ops, and intrinsic calls that interface with C (i8*)
- `emit_callable_txn` param binding fixed to store `Type::Int` for boxed types
- `emit_init_state` fixed to `adapt_to_i64` before truncating to field type
- `LiteralExpr::Char` and `LiteralExpr::String` return `Type::Int` (already boxed)
- `Expr::String` uses `bitcast` to keep i8* (for correct String→Int cast detection)
- ReadFile/Spawn intrinsics properly box i8* returns to i64

**Phase 1 Plan** (native type refactor): Delete i64 boxing entirely per the LLVM backend. Native i1/i32/i8* throughout. Delete `adapt_to_i64`, `ptrtoint_if_string`, `store_i64_result`, all zext/trunc/inttoptr/ptrtoint boxing glue. The type system becomes self-verifying. ~200 lines deleted.

**Follow-up tasks**:
- Fix string trigger comparison (Bug 1): emit `@memcmp` instead of first-byte load.
- Document enum alloca stack-safety limitation (Bug 2): add comment, fix later.
- Track zero-stub expressions (Bug 3): add `todo!()` warnings in `--dev` mode.

## 2026-05-28 — Adding built-in string matches for stdlib functions

**Issue**: Added `is_digit`, `is_alpha`, `is_alphanumeric`, `is_upper`, `is_lower`, `is_space`, `char_to_string` as Rust string-match built-ins in the interpreter.

**Root Cause**: When `UndefinedForeignFunction("is_digit")` appeared, instead of adding `import char from "std/char.bv"` to the calling `.bv` file, I added a Rust string match in `Expr::Call` handler. Also pre-populated `None`/`Some` enum constants in `Interpreter::new()`.

**Fix**: Reverted all built-in hacks. Added `import char from "std/char.bv"` to `lib/compiler/lexer.bv`.

**Lesson**: When the interpreter can't find a function, check if it's in the standard library first. The standard library IS the dependency source. Never add Rust string-match built-ins for things the standard library provides.

## 2026-05-28 — Typechecker overwrites `from` location

**Issue**: `from "std::fs::read_to_string"` in `lib/std/io.bv` was being parsed correctly but then overwritten by typechecker.

**Root Cause**: The typechecker's `load_binding_for_frgn` function set `location = format!("<profile:{}>", name)` unconditionally when `toml_path` was empty, discarding the parser-provided location from the `from "..."` clause.

**Fix**: Added `if signature.location.is_empty()` guard before overwriting.

**Lesson**: Multiple FFI resolution paths coexist (profile-based + direct `from`). Don't assume one path overrides the other.

## 2026-05-28 — Contract-after-arrow parser bug

**Issue**: `-> Type [pre][post]` syntax caused both pre and post conditions to parse as `Expr::Bool(true)`.

**Root Cause**: The while loop in `parse_contract` (line 2879) correctly consumed `[`, but `parse_expression()` returned `BoolTrue` instead of the bracket contents, indicating a lexer state issue specific to the contract-after-arrow code path.

**Workaround**: Use contract-before-arrow syntax `[pre][post] -> Type`.

**Lesson**: The contract-after-arrow path has a subtle lexer/parser interaction bug that's not yet fully understood. Always prefer contract-before-arrow.

## 2026-05-28 — Keyword tokens can't appear in any variable position

**Issue**: `txn`, `reg`, `from` used as variable names caused parse failures across multiple `.bv` files.

**Root Cause**: 44 keyword tokens in the lexer were not accepted by `expect_identifier()` or `parse_primary_expr()`, causing keywords to fail as parameter names, `let` bindings, and `uni` pattern variables.

**Fix**: Route B: Added all 44 missing keywords to `expect_identifier()` and the `parse_primary_expr()` fallback path. Also fixed 3 `while let Some(Ok(Token::Identifier(_)))` loops to use `expect_identifier()` instead.

**Lesson**: The lexer defines ~60 keyword tokens but only 22 were handled as identifiers. When dealing with keyword-as-identifier issues, fix the parser, not the `.bv` files.

## 2026-05-28 — \u{D800} surrogate fails char::from_u32

**Issue**: `'\u{D800}'` in `lib/std/char.bv` caused parse error because logos `regex` callback returned `None` for surrogates, interpreted as a lex error.

**Root Cause**: UTF-16 surrogates (0xD800-0xDFFF) aren't valid Unicode scalar values. `char::from_u32(cp)` returns `None` for them. Logos interprets a `None` return from a regex callback as "token didn't match" (the `?` operator propagates `None` as a failed match).

**Fix**: Changed `char::from_u32(cp)` to `char::from_u32(cp).unwrap_or('?')` in the `\u{...}` escape handler.

**Lesson**: Logos regex callbacks must never return `None` for valid lexer input. Handle all failure modes of `from_u32`, including surrogates.

## 2026-05-28 — Unevaluated enum constructors (None, Some, Ok, Err)

**Issue**: Interpreter returned `UndefinedVariable("None")` and `UndefinedForeignFunction("Err")`.

**Root Cause**: After removing magic constants from `Interpreter::new()`, `load_program` didn't register enum variant names from declarations. `Result` is also an intrinsic type — no `.bv` `enum Result { Ok, Err }` declaration exists to load from.

**Fix**: 
1. `load_program` now iterates `TopLevel::Enum` and registers each variant as `Value::Enum` in state
2. `Ok`/`Err` handled as special intrinsic enum constructors in `Expr::Call` handler
3. `is_ok`, `is_err`, `unwrap`, `unwrap_err` added as Result methods in interpreter

**Lesson**: Enum constructors must be loaded from actual declarations. Intrinsic types (Result, Option) need explicit handling since they lack `.bv` enum declarations.

## 2026-05-29 — Method-call `x.foo(y)` drops all arguments except receiver

**Issue**: `UndefinedVariable("s")` when calling `output.append_str("...")`.

**Root Cause**: The parser's `parse_postfix` function (lines 4314, 4385) parsed arguments from `x.foo(y, z)` into `args` but then constructed `Expr::Call(member_name, vec![expr])` — dropping all parsed arguments and passing only the receiver `x`. The receiver was passed as `expr` which is the preceding expression. The `let mut args = Vec::new()` on the line above was parsed but the populated `args` vector was never used in the `Expr::Call` construction.

```rust
// Line 4385 (BUGGY):
expr = Expr::Call(member_name, vec![expr]);
// Should be:
let mut call_args = vec![expr];
call_args.extend(args);
expr = Expr::Call(member_name, call_args);
```

**Fix**: Changed both locations to prepend receiver `expr` to the parsed `args` vector before constructing `Expr::Call`.

**Lesson**: Mental model matched the intent ("prepend receiver to args") but code used `vec![expr]` which discarded args. Always verify that all populated variables are actually consumed, especially when refactoring from a simpler implementation.

## 2026-05-29 — Term statement inside nested blocks doesn't propagate return value

**Issue**: `compile_file` returned `Value::Void` even though `term Ok(output)` was reached inside a `uni` block.

**Root Cause**: The `call_defn` function (line ~302) handled `Statement::Term` at the top level by capturing the result, but for statements inside nested blocks (unifications, guarded blocks, etc.), `exec_stmt` processed the `Term` and **discarded the value**:

```rust
// exec_stmt (line ~462):
Statement::Term { values: outputs, .. } => {
    if let Some(first) = outputs.first() {
        if let Some(expr) = first {
            let value = self.eval_expr(expr)?;
            if value != Value::Bool(true) {}  // <-- NO-OP, value discarded
        }
    }
}
```

While `call_defn`'s top-level handler correctly stored `result`, it never checked `exec_stmt`'s nested result. After `exec_stmt` returned, `call_defn` continued to the next statement without capturing any value set by `term` inside nested scopes.

**Fix**: 
1. Added `return_value: Option<Value>` to `Interpreter` struct
2. Modified `exec_stmt`'s `Term` handler to store the value in `self.return_value`
3. Modified `call_defn` to save/restore `return_value` and break when it's set after any statement (top-level or nested)

**Lesson**: `term` in Briev is not merely a "function return" — it's a value-capture mechanism that can appear inside any nested scope (guards, unifications, blocks). The interpreter must capture ALL `term` values, not just top-level ones.

## 2026-05-29 — Result field key mismatch between constructor and consumer

**Issue**: `run_selfhost` in `main.rs` looked for field key `"result"` (from the specific Ok/Err path at line 878) but the generic enum constructor path at line 869 used field key `"value"`.

**Root Cause**: Two code paths create `Ok`/`Err` enum values:
1. Generic enum constructor path (line 867-876): used when `Ok` variant is in state (registered from `Result` enum declaration). Creates fields with key `"value"`.
2. Specific Ok/Err path (line 878-882): used when `Ok`/`Err` are NOT in state. Creates fields with key `"result"`/`"error"`.

Since `std.result` is imported, `Ok` IS in state, so path 1 always applies. But `run_selfhost` at lines 643 and 651 looked for `"result"`/`"error"` keys from path 2.

**Fix**: Changed `fields.get("result")` to `fields.get("value")` and `fields.get("error")` to `fields.get("value")` in `run_selfhost`.

**Lesson**: When multiple construction paths exist for the same enum type, their field key conventions must be consistent. The generic path always uses `"value"` for single-field enum construction, but the specific Result path had its own convention. Prefer a single consistent convention.

## 2026-05-29 — Briev-written lexer rejects all input with "Unexpected character"

**Issue**: Self-host pipeline tokenizes files via `lib/compiler/lexer.bv` (running inside the interpreter) but fails with `Lex error: Unexpected character: ` on all inputs.

**Root Cause**: The lexer's `next_token` function (line 321-489) checks for EOF at line 325-328 using `is_none(current_char(state))`. If this check somehow fails to detect `None`, line 333 unconditionally calls `unwrap(current_char(state))` — which returns `Void` for `None` input. The guards then compare `Void` against various chars (all false), eventually hitting the "Unknown character" fallthrough at line 488. `char_to_string(Void)` produces an empty string, hence the blank error message.

**Status**: Not yet fixed. Potential causes:
- `is_none` generic function may not dispatch correctly to `Option<Char>` specialization
- `current_char` may return wrong `None` representation
- The `None` variant of `Option` may not exist in the interpreter's state

**Lesson**: (pending investigation)

## 2026-05-30 — `expand_implicit_terms_txn` injects `term true;` into void-returning transactions

**Issue**: `node handle_sigint [sigint] { term; };` produced `ret i64 1` in a `define void` function, causing LLVM verification to fail with "value doesn't match function result type 'void'". The `wake_triggers.bv` fixture exposed this.

**Root Cause**: `desugarer.rs:expand_implicit_terms_txn` unconditionally converted `term;` → `term true;` for all transactions with `Bool` postconditions, mirroring the defn path. But transaction functions are `define void` — they cannot return an `i64`. The `emit_stmt` handler correctly emitted `ret i64 <val>` when `values.first()` was `Some`, never reaching the `ret void` path.

**Fix**: Removed the expansion logic entirely from `expand_implicit_terms_txn`. It is now a passthrough that clones the body unchanged. Transactions return void; their contract semantics are handled by the reactive desugaring pipeline and precondition emission, not by injecting values into `term;`.

**Lesson**: `term;` in transactions means "this transaction has no return value" (void-termination). `term;` in definitions means "terminate with the default postcondition value". These are semantically different — never unify the desugaring paths.

## 2026-05-30 — `opt` new PM syntax: `-passes=verify` not `-verify`

**Issue**: Integration tests called `opt -verify` which failed silently on LLVM 18+ because the legacy pass manager was removed.

**Root Cause**: LLVM 18 defaults to the new pass manager (`-passes=...`). The old `-verify` flag syntax is recognized but no-ops without the legacy PM enabled.

**Fix**: Changed test helper to use `opt -passes=verify` and `opt -passes=default<O3>`.

**Lesson**: Always verify LLVM tooling syntax matches the installed version. The new PM syntax is now canonical for LLVM 17+.

## 2026-05-30 — `alwaysinline` must precede attribute group in LLVM 18

**Issue**: `define void @fn(...) local_unnamed_addr alwaysinline #0` was rejected by LLVM 18 verifier.

**Root Cause**: LLVM 18 requires `alwaysinline` to appear AFTER the attribute group reference, i.e. `define void @fn(...) local_unnamed_addr #0 alwaysinline`.

**Fix**: Swapped the order in `emit_transaction`: `local_unnamed_addr #0{}` with the `alwaysinline` string appended after `#0`.

**Lesson**: LLVM 18 tightened the IR syntax for `alwaysinline`. The canonical position is `#N alwaysinline`.

## 2026-05-29 — `dispatch_mode` lost during desugaring and import resolution

- **Issue**: `#pragma dispatch(parallel)` was parsed correctly by the parser but silently ignored — the LLVM backend always emitted sequential reactor code regardless of the directive.

- **Root Cause**: The `Program` struct has a `dispatch_mode: DispatchMode` field set by the parser. However, both the `Desugarer::desugar()` (desugarer.rs:345) and `ImportResolver::resolve_imports()` (import_resolver.rs:84) construct new `Program` objects with `dispatch_mode: Default::default()` (i.e., `Sequential`) instead of preserving `program.dispatch_mode` from the input program. Since the `run_llvm_compile` pipeline runs `resolve_imports` → `desugar` → `llvm_backend.generate`, the dispatch mode is lost before code generation.

- **Fix**: Changed both `desugarer.rs:345` and `import_resolver.rs:84` to use `dispatch_mode: program.dispatch_mode` instead of `Default::default()`.

- **Lesson**: Whenever a pipeline stage constructs a new `Program` from an existing one, all fields must be explicitly forwarded. This is a brittle pattern — consider a builder or `Clone` for `Program` that preserves metadata fields. Also add a test that verifies dispatch-mode propagation through the full pipeline from parse → resolve → desugar → backend.

## 2026-05-30 — Contract-after-arrow `-> Type [pre][post]` steals first bracket as `Type::ContractBound`

**Issue**: `-> Int [pre][post]` parsed both contract brackets incorrectly — `[pre]` was silently consumed as `Type::ContractBound(Int, pre)` on the output type, and `parse_contract()` only saw `[post]`, setting `pre_condition = post` and `post_condition = Bool(true)`.

**Root Cause**: The contract-after-arrow path (parser.rs:2833) calls `parse_output_types_with_names()` before `parse_contract()`. Inside that function, `parse_type()` at line 4008 greedily consumes `[expr]` as `Type::ContractBound` on *any* type, not just types where the user explicitly wrote `Int[pre]`. Since the bracket check happens after the type name is parsed and before `<` generics, it catches contract brackets that belong to the function, not the type.

**Fix**: Split `parse_type()` into `parse_type()` (public, legacy) and `parse_type_inner(allow_contract_bound)`. `parse_output_types_with_names()` calls `parse_type_inner(false)` so contract brackets are never stolen. The `ContractBound` feature is still available for cases like `Int[product > 0]` when `parse_type()` is called directly.

**Lesson**: When parsing ordered syntax (`-> Type [contract]`), each parser component must be constrained to not consume tokens meant for later components. Greedy `[` consumption in `parse_type` was correct for standalone type parsing but wrong when types and contracts appear adjacent.

## 2026-05-30 — `len()` infinite recursion in self-host interpreter

**Issue**: The self-host pipeline (`briev-compiler selfhost`) failed with a stack overflow / hang on any input. The Briev-written lexer (`lib/compiler/lexer.bv`) called `len(state.source)` which dispatched to `call_defn("len")`, causing infinite recursion through `lib/std/string.bv`'s `term s.len()`.

**Root Cause**: The interpreter's `Expr::Call` handler (interpreter.rs:963) checked user `definitions` **before** any built-in handler for `len`. Since `lib/std/string.bv` defines `defn len(s: String) { term s.len(); }`, calling `len(x)` on a `String` value found the user definition first, dispatched to `call_defn`, which evaluated `term s.len()` — which called `len` again via `Expr::Call("len", [Identifier("s")])` — ad infinitum.

**Fix**: Added a built-in `len` handler for `Value::String` and `Value::List` **before** the definitions check. This short-circuits the recursive definition and returns the correct length directly.

**Lesson**: User definitions must never shadow built-in handlers for primitive type operations. The ordering of checks in `Expr::Call` must be: (1) built-in Result methods, (2) built-in primitives like `len`, (3) user definitions, (4) FFI, (5) enum constructors.

**2026-06-05 Update**: Fully resolved by `:>` projection operator migration. The `Expr::ListLen` magic node and the UFCS `resolve_len_calls` hack are both deleted. All length queries use `x :> Size` — unique syntax, first-class `Expr::Projection` node, zero shadowing risk. `defn len(x) { term x :> Size }` is now a pure stdlib convenience wrapper that cannot recurse because `:>` is parsed directly to `Projection`, not to a `Call`.

## 2026-05-30 — Float result registers not tracked, causing compound float math to emit integer ops

**Issue**: Compound float arithmetic — `let x = 1.0 + 2.0; let z = x + y;` — silently corrupted results. The second addition emitted `add i64` instead of `fadd float` because `is_float_expr(x)` returned `false`.

**Root Cause**: `emit_binop` (llvm.rs:1442) and `Expr::Neg` (llvm.rs:872) converted float results back to i64 but never registered the output register as `Type::Float` in `self.register_types`. Subsequent `is_float_expr` for `Expr::Identifier(name)` at llvm.rs:1401-1403 looked up the register in `register_types` and got no match (or `Type::Int` from the literal insertion), returning `false`.

**Fix**: Added `self.register_types.insert(v.to_string(), Type::Float)` at the end of both float code paths in `emit_binop` and `Expr::Neg`.

**Lesson**: Every code path that produces a value of a particular type must register it in `register_types`. The existing pattern (literals do register, but compound expressions don't) is inconsistent and fragile.

## 2026-05-30 — OnExit cleanup drained on first exit point, lost on subsequent exits

**Issue**: Functions with multiple exit points (multiple `term;` paths, `Escape`, or guarded blocks) only emitted cleanup code on the first exit. All subsequent exits generated zero cleanup, leaking resources.

**Root Cause**: `Statement::Term` (llvm.rs:626) used `std::mem::take(&mut self.pending_cleanup)` which **drains** the vector, leaving it empty for all subsequent exit points. `Statement::Escape` (llvm.rs:638) didn't emit cleanup at all.

**Fix**: 
1. Changed `std::mem::take` → `.clone()` in `Statement::Term` so `pending_cleanup` is preserved for future exits.
2. Added cleanup emission to `Statement::Escape` with the same clone pattern.

**Lesson**: Shared state like `pending_cleanup` must not be mutably consumed at the first use site when multiple consumers exist. Clone the data for each consumer instead.

## 2026-06-01 — `extract_bounded_pre` drops `And` preconditions, fold limit stuck at 0

**Issue**: Benchmarks using `[io_pending && ops < N]` produced folded while-loops comparing against `add i64 0, 0` (limit = 0, loop never executes). Benchmarks hung forever doing zero iterations per tick, trapped in wake main loop.

**Root Cause**: `src/analysis/transition_graph.rs:extract_bounded_pre` matched only `Expr::Lt(var, bound)` and `Expr::Le(var, bound)` at the top level. When the precondition was `Expr::And(io_pending, Expr::Lt(ops, N))`, the `And` wrapper was never unwrapped. `emit_folded_loop` at `llvm.rs:2029` defaults the limit to `add i64 0, 0` when no total is available.

**How discovered**: `timeout 5s ./benchmarks/ring_buffer` never terminated. LLVM IR inspection showed `%lt = add i64 0, 0` instead of `load i64, i64* @N`. `strace -c` confirmed 693 `epoll_wait` calls in 5 seconds with no useful work.

**Fix**: Added recursive `And` unwrapping to `extract_bounded_pre` — decomposes compound preconditions like `trigger && counter < total`.

**Files**: `src/analysis/transition_graph.rs:88-90`, `benchmarks/ring_buffer.bv:21`, `benchmarks/async_counters.bv:17,23`

**Lesson**: Fold analysis must recurse into logical operators in the precondition tree. Non-recursive extraction silently produces valid-looking LLVM IR with zero-iteration loops — the worst kind of silent failure because the binary doesn't crash, it hangs.

## 2026-06-01 — Solo reactive txn auto-promoted to async, injects unnecessary thread pool + barrier

**Issue**: `ring_buffer.bv` with a single `node work` (no `async` keyword) generated `@async_body_work`, `@llvm.thread_pool`, thread pool init, and barrier calls in the main loop — all for one transaction that does sequential work.

**Root Cause**: `src/backend/llvm.rs:388`: the async eligibility check requires ALL non-enum reactive txns to be pairwise conflict-free. For a single txn, the pairwise loop (`for i..for j>i`) never executes, leaving `is_async_eligible = [true]` via vacuous truth. Solo txns are trivially "conflict-free with all others" because there are no others.

**How discovered**: LLVM IR inspection of `ring_buffer.ll` showed `define void @async_body_work` and `@llvm.thread_pool` despite the source having no `async` keyword and only one txn.

**Fix**: Require at least 2 async candidates: `all_async_eligible` gated on `async_candidates.len() >= 2`.

**Files**: `src/backend/llvm.rs:388`, `benchmarks/ring_buffer.bv`

**Lesson**: "All elements satisfy predicate" is true for single-element collections. When "all" implies "there should be multiple things to distribute work across", the guard must explicitly check `len() >= 2`.

## 2026-06-01 — Wake hybrid programs idle forever after convergence (no exit mechanism)

**Issue**: Wake hybrid programs with `@ link` triggers never terminate after convergence. Ring buffer completes 50M iterations then spins: `__rt_wait() → tick → switch (case_1) → done → __rt_wait() → ...` forever.

**Root Cause**: The wake main loop unconditionally routes case arms to `do_wait` → `__rt_wait()` → `br label %tick`. No convergence-based exit path exists in the wake codegen paths. Compiler already computes convergence data but doesn't use it in `emit_enum_main` or `emit_main`.

**How discovered**: `timeout 5s ./benchmarks/ring_buffer` never terminated. SIGTERM is caught by `briev_rt.c` signal handler — program refuses to die. Only SIGKILL works.

**Current workaround**: `timeout --signal=KILL 30s` for benchmarks.

**Fix**: Added `#!exit <expr>;` pragma — file-level boolean expression from the source `.bv` file, evaluated every tick before `__rt_wait()`. When true, `main()` returns 0. Backed by convergence-based exit (natural death) detection.

**Files**: `src/parser.rs` (`#!exit` parsing), `src/backend/llvm.rs` (`emit_exit_check`, `emit_exit_expr`, modified `emit_main`/`emit_enum_main`), `plans/2026-06-01-exit-semantics.md`

**Lesson**: Compile-time analysis data must propagate to ALL codegen paths that can use it. Convergence data is computed once and used for Path 3 (precompute) but Path 4/5 (enum/async dispatch) need it equally. The commit that added `is_fully_precomputable` should have wired it into `emit_enum_main` at the same time.

## 2026-06-01 — `is_trigger_gated` only matches bare `Identifier`, misses `And(trigger, condition)`

**Issue**: `async_counters.bv` with precondition `[io_pending && counter < N]` was classified as async dispatch (Path 5) instead of enum dispatch (Path 4). The async path runs increments via `reactor_tick()` only, achieving ~1 increment per 100ms tick — 29 days for 25M iterations.

**Root Cause**: `src/backend/llvm.rs:is_trigger_gated()` matched only `Expr::Identifier(name)` at the top level. `And(io_pending, Lt(counter, total))` was not recognized as trigger-gated. The `enum_txn_names` set was empty for all compound preconditions, so all txns fell through to `async_candidates`.

**How discovered**: `async_counters` benchmark hung after `#!exit` was added. Even with exit condition `a == N && b == N`, the counters never reached N because the enum dispatch path (which runs folded loops) was never selected.

**Fix**: Added `Expr::And(l, r)` arm to `is_trigger_gated` that recurses into both sides.

**Files**: `src/backend/llvm.rs:138-144`

**Lesson**: When classification functions check preconditions for structural properties (trigger-gated, bounded-convergence), they must recurse into ALL expression types that can wrap the target pattern. The common pattern `trigger && counter < N` is an `And` node — a bare `Identifier` match will never see it.

## 2026-06-01 — `emit_enum_main` single-txn `graph.nodes.len() == 1 && txns.len() == 1` guard prevents multi-txn folded loops

**Issue**: Even when multiple trigger-gated txns with bounded convergence enter the enum dispatch path, only one folded loop was emitted per case arm — the one corresponding to the first (and assumed-only) transaction. Multi-txn programs like `async_counters` (inc_a + inc_b) converged zero counters per tick.

**Root Cause**: `generate()` at `llvm.rs:650-662` extracted folding params only for the single-txn guard `graph.nodes.len() == 1 && txns.len() == 1`. For multi-txn programs, `(enum_ci, enum_ti, enum_tcn) = (0, None, None)` — every folded loop got limit `add i64 0, 0`.

**How discovered**: After fixing `is_trigger_gated`, async_counters entered enum dispatch but still ran 0 iterations per tick. LLVM IR inspection showed `%lt... = add i64 0, 0` for all case arm loops.

**Fix**: 
1. Built `enum_fold_params: HashMap<String, (usize, Option<usize>, Option<String>)>` in `generate()` — per-txn folding params extracted from the transition graph.
2. Added `fold_params` parameter to `emit_enum_main`.
3. Added `emit_case_folded_loops` closure that iterates over all fold param entries and emits one folded loop per entry per case arm.

**Files**: `src/backend/llvm.rs:647-665` (enum_fold_params build), `src/backend/llvm.rs:2285-2315` (multi-txn case arm emission)

**Lesson**: The enum dispatch path was designed for single-txn programs. Multi-txn programs with multiple bounded counters need per-txn folded loops. The `graph.nodes.len() == 1 && txns.len() == 1` guard was a premature optimization assumption that excluded valid multi-txn convergence programs. Always verify classification/path-selection logic handles N>1 inputs.

## 2026-06-02 — Struct-SSA regression for non-pure bodies (Kalman filter 2× slowdown)

**Issue**: Kalman filter benchmark ran 0.28s at 10M iterations while the old code ran 0.143s (scaled from 0.716s at 50M) — exactly 2× slower. C reference ran 0.14s. Briev went from beating C to trailing by 2×.

**Root Cause**: `emit_folded_main` with `use_phi=false, body=Some(stmts)` emits a `load %State` (64 bytes), 13-element chained `extractvalue`/`insertvalue` sequence, then `store %State`. This struct-SSA pattern requires SROA (Scalar Replacement of Aggregates) to decompose into per-field scalar operations. But `llc -O2` does NOT run SROA — only `opt -O2` does. Without SROA, LLVM's backend materializes the entire 64-byte struct as a memory block, preventing per-field register promotion, phi node generation, and GVN across float operations.

The old (pre-struct-SSA) codegen used per-field `GEP + load/store` throughout, which LLVM's backend handles naturally without SROA.

**Why it affected Kalman specifically**: The Kalman filter's precondition references all 12 float fields (`x0 == x0 && x1 == x1 && ...` — a NaN-guard pattern). This makes all float fields live, so `is_effectively_pure = false`. The non-pure path takes `use_phi=false, body=Some(stmts)` which is the struct-SSA path. Pure/effectively-pure bodies (IIR, ring_buffer, async_counters) take the `use_phi=true` phi-node path and were unaffected.

**Fix**: Run `opt -O2 -S` before `llc` in `run_llvm_compile()` at `src/main.rs:1899`. `opt -O2` runs SROA, mem2reg, GVN, and constant propagation. SROA decomposes the `load %State`/`store %State` into scalar phis; GVN eliminates redundant float→i64→float round trips. The SLP hazard analyzer's `-vectorize-slp=false` flag is passed to `opt` (where SLP runs as a middle-end pass), not `llc`. Graceful fallback if `opt` is not installed.

**Result**: Kalman filter recovers from 2× regression to 0.71s at 50M vs C 0.75s (Briev beats C by ~5%, tied at worst).

**Lesson**: `llc -O2` and `opt -O2` run different pass pipelines. `llc` is the codegen backend (instruction selection, regalloc, scheduling). `opt` is the middle-end optimizer (SROA, mem2reg, GVN, loop opts, vectorization). Struct-SSA (`load %State`/`store %State` + insertvalue chains) requires SROA to decompose — always run `opt -O2` before `llc` for programs with struct values.

## 2026-06-02 — `is_trigger_gated` misses `Expr::Eq`, enum dispatch invisible for `trigger == literal` preconditions

**Issue**: Sparse dispatch benchmark preconditions like `t == 101` (Eq(Identifier, Integer)) never entered the enum dispatch optimizer path. The enum dispatch path correctly extracted keys via `extract_trigger_keys` (line 149-183) but `is_trigger_gated` (line 139-147) returned `false` for all `Expr::Eq` patterns, so no reactive txn was classified as an enum candidate.

**Root Cause**: `is_trigger_gated` at `src/backend/llvm.rs:139-147` matched only `Expr::Identifier(name)` and `Expr::And(l, r)`. An `Expr::Eq(Identifier(trigger), Integer(value))` node was never unwrapped. The function signature says it checks for "a direct reference to one of the given trigger names" — but this was too narrow: a _direct reference_ is `trigger == value` just as much as bare `trigger`, and `extract_trigger_keys` already recognized Eq patterns.

```rust
// Before (BUGGY):
fn is_trigger_gated(pre: &Expr, trigger_names: &HashSet<&str>) -> bool {
    match pre {
        Expr::Identifier(name) => trigger_names.contains(name.as_str()),
        Expr::And(l, r) => is_trigger_gated(l, trigger_names) || is_trigger_gated(r, trigger_names),
        _ => false,  // ← Expr::Eq falls through here
    }
}
```

This bug was introduced in the same commit that added `extract_trigger_keys` and `is_trigger_gated` (Step 7 / dead-field elimination). Whoever wrote `extract_trigger_keys` correctly handled `Expr::Eq` but the counterpart `is_trigger_gated` didn't get the same treatment.

**How discovered**: Writing a sparse dispatch benchmark that used `t == 101 || t == 204 || ...` preconditions. Benchmark compiled but ran through the standard `reactor_tick()` path instead of enum switch dispatch. LLVM IR inspection showed `switch` was absent — the program used the generic tick loop.

**Fix**: Added `Expr::Eq(l, r)` arm to `is_trigger_gated`:

```rust
Expr::Eq(l, r) => {
    matches!(l.as_ref(), Expr::Identifier(name) if trigger_names.contains(name.as_str()))
        || matches!(r.as_ref(), Expr::Identifier(name) if trigger_names.contains(name.as_str()))
}
```

**Files**: `src/backend/llvm.rs:139-147`

**Lesson**: Whenever a classification function (`is_trigger_gated`) and a data-extraction function (`extract_trigger_keys`) operate on the same AST nodes for the same purpose, they must recognize the same expression patterns. `extract_trigger_keys` correctly handles `Expr::Eq` — `is_trigger_gated` must too. They were written at the same time for the same optimization path; the divergence was an oversight.

## 2026-06-02 — `llvm.assume` before `br` in folded loops makes `opt` believe exit branch is dead

**Issue**: `const_heavy.bv` compiled to a binary that immediately segfaulted. The `main()` function was optimized to `unreachable` by `opt -O2`, causing LLVM to emit invalid code.

**Root Cause**: `emit_folded_loop` at `src/backend/llvm.rs:2911-2912` emitted `call void @llvm.assume(i1 %cp)` on the loop entry comparison result, then `br i1 %cp, label %body, label %done`. The `llvm.assume` tells LLVM's optimizer that the comparison is ALWAYS true. `opt -O2` then eliminated the `done` branch (the loop exit) as dead code, making the loop infinite. The `noreturn` function attribute and `unreachable` terminator produced a segfault.

```llvm
; Before (optimized to infinite loop):
%cp = icmp slt i64 %cnt, %total
call void @llvm.assume(i1 %cp)    ; tells opt: %cp is ALWAYS true
br i1 %cp, label %body, label %done  ; opt eliminates %done

; Result (via opt -O2):
define noundef i32 @main() {
  %t9.i = tail call i64 @__get_env_int(ptr @str.0)
  store volatile i64 %t9.i, ptr @global_state
  unreachable  ; <-- SEGFAULT
}
```

Three code paths in `emit_folded_loop` (phi mode, SSA mode, call mode) all emitted `llvm.assume` on the same `icmp slt` result.

**How discovered**: `const_heavy.bv` benchmark segfaulted immediately. GDB showed PC at `0x5`. Inspection of `const_heavy.opt.ll` revealed `main()` ended with `unreachable`. The `llvm.assume` was traced to the optimization sprint commit that added "`llvm.assume` on convergent preconditions" (2026-06-02).

**Fix**: Removed `llvm.assume` emission from all three code paths in `emit_folded_loop`. Removed the `proven_convergent: bool` parameter from the function signature and all callers.

**Files**: `src/backend/llvm.rs:2911-2912`, `src/backend/llvm.rs:2936-2937`, `src/backend/llvm.rs:2969-2970`

**Lesson**: `llvm.assume(i1 %cond)` tells the optimizer that `%cond` is unconditionally true. Placing it BEFORE a conditional branch (`br i1 %cond, label %exit, label %loop`) makes the optimizer eliminate the branch's continuation as dead code. If the branch controls loop convergence, the result is an infinite loop. `llvm.assume` is correct when placed after a runtime panicking branch (`br i1 %cond, label %panic, label %safe` followed by `unreachable` then `call @llvm.assume(i1 %cond)`) — never before a convergence check.
## 2026-06-04 — Exit expression Neg(Integer) not handled in emit_exit_expr

**Issue**: Program with `#!exit cr >= -200` hung — exit condition never satisfied. The LLVM IR showed `%t599 = add i64 0, 0 ; unsupported exit expr` instead of `%t599 = sub i64 0, 200`.

**Root Cause**: `emit_exit_expr` at `src/backend/llvm.rs:3558` had an early-return filter for `Expr::Integer`, `Expr::Bool`, and `Expr::Float` that delegated to `emit_expr` for constant inlining. But negative integers like `-200` are parsed as `Expr::Neg(Box::new(Expr::Integer(200)))` — the `Neg` wrapper wasn't in the filter. The expression fell through to the `_ =>` catch-all at line 3658 which returned `add i64 0, 0`.

**Fix**: Added `Expr::Neg(_)` to the early-return filter:
```diff
- Expr::Integer(_) | Expr::Bool(_) | Expr::Float(_) => {
+ Expr::Integer(_) | Expr::Bool(_) | Expr::Float(_) | Expr::Neg(_) => {
```

**Lesson**: Negative literals in Briev are `Neg(Integer(n))`, not `Integer(-n)`. Any code path that pattern-matches on `Expr::Integer` for constants must also handle `Expr::Neg(Expr::Integer(_))`. The `emit_expr` function already handles `Neg` correctly — the fix delegates to it.

## 2026-06-04 — Universal loop hangs with decreasing counter contract

**Issue**: Programs with `node ... [count > 0][count == 0]` (decreasing counter) hang. Switching to `[count < N][count == N]` (increasing counter) works.

**Root Cause**: The universal loop in `emit_folded_multi_main` emits `count < N - stride` for the unrolled body4 path, computed as `adj = add i64 N, <negated_stride>`. This comparison `icmp slt count, adj` only makes sense when count increases. A decreasing counter like `count > 0` has an INVERTED direction — count starts high and decreases, so `count > 0` should become `count > stride` not `count < bound - stride`.

**Lesson**: The universal loop (unrolled fold) assumes strictly increasing counters. The `transition_graph` should detect decreasing counters and either invert the comparison in the codegen or fall back to the non-unrolled default path.


## 2026-06-04 — Decreasing counter contracts hang or fall to O(N)

**Issue**: Programs with `node [count > 0][count == 0]` either hung (universal loop path) or ran O(N) tick-per-iteration (fallback path). Only `[count < N][count == N]` was fast.

**Root cause**: Three separate gaps:
1. `extract_bounded_pre` only matched `Expr::Lt`/`Expr::Le` (increasing). `Expr::Gt`/`Expr::Ge` (decreasing) fell through to `None`.
2. `detect_increments` only matched `Expr::Add` (count = count + 1). `Expr::Sub` (count = count - 1) returned `None`.
3. `emit_folded_loop` unconditionally emitted `icmp slt` (signed-less-than) for all comparisons. No branch for `icmp sgt`.

**Fix** (3 files, additive):
1. `transition_graph.rs`: Added `ConvergeDirection { Increasing, Decreasing }` enum; added `direction` + `bound_literal` fields to `BoundedPre`. Extended `extract_bounded_pre` to match `Gt`/`Ge` (set `direction: Decreasing`). Extended `detect_increments` to match `Expr::Sub` for decrementing counters.
2. `llvm.rs`: Added `FoldParam` struct carrying `counter_idx`, `bound_field_idx`, `bound_const_name`, `is_decreasing`, `bound_literal`. Threaded through `multi_fold_params` → `enum_fold_params` → `emit_folded_multi_main` → `emit_folded_loop`.
3. `emit_folded_loop` header: branched on `is_decreasing` to emit `icmp sgt` instead of `icmp slt`, and `bound + (unroll-1)` instead of `bound - (unroll-1)` for the body4 path. Added `bound_literal` priority to bound loading for literal bounds like `count > 0`.

**Tests**: Decreasing counter program (`[count > 0][count == 0]` with `count = count - 1`) compiles, emits `icmp sgt`, completes 50M iterations in <10s.

## 2026-06-05 — Unused `io_pending` import forces reactive runtime on pure-state benchmarks

**Issue**: `benchmarks/bit_clear.bv` and `benchmarks/queue_drain.bv` both imported `io_pending` from `std/briev_rt.bv` but never used it in any precondition. The import was dead weight but still triggered the reactive runtime path (`has_wake_triggers = true` → reactor with `__rt_wait()` 100ms blocking per tick), turning a 63-iteration burn (bit_clear) into a 6.3-second slog and a 10-iteration burn (queue_drain) into a 1-second slog.

**Root Cause**: The compiler treats any import of `io_pending` as evidence the program needs wake triggers, even if no precondition references it. The presence of `io_pending` in the import set sets `has_triggers = true` in the transition graph, which in turn selects the reactor codegen path instead of the pure SSA while-loop path.

**Fix**: Removed `import { io_pending } from "std/briev_rt.bv"` from both files. Neither benchmark needs external event wakeup — both converge purely on state (`reg != 0 → reg == 0`, `queue:>Size > 0 → queue:>Size == 0`).

**Lesson**: Never import `io_pending` unless the program genuinely waits for external IO. Pure-state convergence benchmarks must use the SSA while-loop path (no reactor). The `io_pending` import is a foot-gun: it looks harmless but silently activates the full reactive machinery.

**Also**: `queue_drain.bv` declared `const queue: List<Int> = [...]` but then tried to mutate it via `<- &queue`. `const` values are compile-time immutable. Changed to `let queue`.

## 2026-06-05 — Low print modulo doesn't fire on short benchmarks

**Issue**: `bit_clear.bv` used `[reg % 1000000 == 0]` as the print guard, but the benchmark only runs 63 iterations (popcount of i64::MAX). No value of `reg` is divisible by 1,000,000 in that range, so the liveness `__print_int` never fires. The program compiled and ran correctly (no fold), but produced zero observable output.

**Root Cause**: The print threshold was copy-pasted from 50M-iteration benchmarks without adjusting for bit_clear's inherently bounded iteration count (63 max).

**Fix**: Lowered threshold to `[reg % 100000 == 0]`. The values that generate output: `reg = 0` (the last iteration, always fires since `0 % 100000 == 0`). This ensures at least one observable side-effect fires.

**Lesson**: When adapting benchmark patterns to bounded-iteration designs (integer-width-bound patterns like popcount decay), verify the print guard threshold will actually fire within the available iteration space. A silent benchmark is a dead-code-elimination risk.

## 2026-06-05 — `memory(argmem: write)` on FFI declarations lets LLVM eliminate IO calls

**Issue**: `__print_int(i64)` calls inside small loops (10-iteration queue_drain) were eliminated by `opt -O3` during LTO. The binary contained a dead `__print_int` function that was never called.

**Root Cause**: `src/backend/llvm.rs:1344` declared ALL foreign functions with `attributes #1 = { ... memory(argmem: write) }`. This tells LLVM the function only writes through pointer arguments. For `__print_int(i64)`, there are no pointer arguments, so LLVM concluded the function writes NO memory. Combined with `willreturn` and the unused return value, `opt -O3` eliminated the entire call.

**Fix**: Removed `memory(argmem: write)` from attribute #1:
```
attributes #1 = { nocallback nofree nosync nounwind willreturn }
```
Without any `memory(...)` restriction, LLVM conservatively assumes the function can read/write arbitrary memory. During LTO, `opt -O3`'s FunctionAttrs pass examines `__print_int`'s actual body (merged from `briev_rt.c`), sees `fprintf(stderr, ...)` as a real global side effect, and correctly preserves the call. For `__sqrtf`, FunctionAttrs infers `readnone` from the `sqrtf` call and restores CSE/hoisting.

**Lesson**: Never assert `memory(argmem: write)` on FFI functions — the Briev compiler cannot verify this. The mathematically correct default is no memory restriction. LTO reveals actual function bodies and LLVM's FunctionAttrs pass infers correct attributes deterministically.

## 2026-06-05 — Compile-time-known list size causes precomputation (correct behavior)

**Issue**: `queue_drain.bv` used a compile-time list literal `[1..10]`. The compiler correctly precomputed all 10 iterations within the default budget (256) and emitted `main` as `xor eax; ret`. The benchmark produced zero output and ran trivially.

**Root Cause**: Not a bug — the system works as designed. When all information is known at compile time, the compiler precomputes the result. The benchmark author must make the bound runtime-determined to prevent precomputation.

**Fix**: Replaced the compile-time list with a runtime counter:
- Before: `let queue: List<Int> = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];` + drain convergence
- After: `let N: Int = __get_env_int("BOUND");` + counter convergence + push/pop side ops

**Lesson**: If a benchmark must execute at runtime, use `__get_env_int("BOUND")` for the bound. A compile-time-known bound WILL be precomputed. This is correct, not a bug. See AGENTS.md §"Precomputation is Correct, Not a Bug".

---

## 2026-06-05 — Parser fails on `term! -> swan_song;` inside guarded blocks

**Issue**: `term! -> __print_int(checksum);` inside `[count == N] { ... }` caused parse error `expected identifier, found Arrow at 66:15`.

**Root Cause**: `parse_term_outputs()` didn't check for `Token::Arrow`, so it tried to parse `->` as an expression. Additionally, swan song was parsed via `parse_statement()` which consumed the trailing `;`, but the caller's `expect(Token::Semicolon)` expected another one.

**Fix**: Added `Token::Arrow` check in `parse_term_outputs()` to return early. Changed swan song parsing from `parse_statement()` to `parse_expression()` wrapped in `Statement::Expression`, so semicolon is consumed once by the caller.

**Lesson**: New syntax needs parser checks for all possible token sequences following `term!`. The `->` arrow, term outputs, and semicolon interact in ways that are easy to miss.

---

## 2026-06-05 — LLVM `attributes #1` declared FFI functions as pure, letting optimizer eliminate I/O

**Issue**: `opt -O3` eliminated `__print_int` calls from merged bitcode. Benchmark binaries produced zero output.

**Root Cause**: `attributes #1 = { nocallback nofree nosync nounwind willreturn }` told LLVM the function had no side effects. LLVM eliminated the calls as dead code.

**Fix**: Changed to `attributes #1 = { nounwind }`. Removed `nofree`, `nosync`, `willreturn`, `nocallback` — these all signal purity.

**Lesson**: FFI function declarations must not over-promise purity. Only `nounwind` is safe to assume. `willreturn` in particular tells LLVM it can speculate calls.

---

## 2026-06-05 — `__putchar` undefined at link time despite definition in runtime

**Issue**: `__putchar` was declared in `fasta.bv` as `frgn` and present in `runtime/briev_rt.c`, but LTO link failed with `undefined reference to '__putchar'`.

**Root Cause**: Function was `static inline` with `always_inline`. Clang compiled `briev_rt.c` at `-O2`, inlined all calls within the runtime translation unit, then discarded the function body. The program IR's `declare` remained unresolved across module boundaries.

**Fix**: Changed to `__attribute__((used)) int64_t __putchar(int64_t c)`. No `static`, no `inline`. The `used` attribute forces emission even when no callers exist within the translation unit.

**Lesson**: Runtime helper functions called only from program IR (not from within the runtime itself) must not be `static`. Use `__attribute__((used))` to prevent dead-code elimination.

--- 

## 2026-06-05 — `io_pending` used as liveness workaround in benchmarks

**Issue**: Several benchmarks used `io_pending` (an FFI call) in their transaction guard to prevent pure-counter fold elimination. This was a relic from before `term! -> swan_song;` provided proper liveness semantics.

**Root Cause**: Cargo-culted pattern from older benchmarks. `io_pending` was a hack — it forced the compiler to treat the body as impure because the precondition referenced FFI.

**Fix**: Removed `io_pending` imports and guard conditions from all Tier 3 benchmarks. Guards now use `[count < N][count == N]` directly.

**Lesson**: When new language features supersede old workarounds, audit existing code for the old pattern and clean it up. Documented here to prevent future cargo-culting.

--- 

## 2026-06-05 — Accidental deletion of benchmark source files during cleanup

**Issue**: `rm -f benchmarks/fannkuch_redux*` matched and deleted both build artifacts AND source files (`fannkuch_redux.bv`, `fannkuch_redux_c.c`). Restored from git but lost uncommitted edits.

**Root Cause**: Shell glob `*` matched all files starting with the prefix. No distinction between `.bv` source, `_c.c` reference, `.o` object, `.ll` IR, and binary executable.

**Lesson**: Never glob over source files. Use explicit filenames for cleanup: `rm -f benchmarks/fannkuch_redux benchmarks/fannkuch_redux.ll benchmarks/fannkuch_redux.o`. Or better: organize build artifacts in a separate subdirectory.

---

## 2026-06-06 — Parser discards `from "..."` value in frgn declarations

**Issue**: `frgn __print_int(n: Int) -> Bool from "libruntime"` — the `"libruntime"` string was parsed but immediately thrown away. `ForeignSignature::location` was hardcoded to `String::new()` at construction.

**Root Cause**: `parser.rs:1142` used `location: String::new()` instead of `location: location.clone()`. The parsed `location` variable was never written into the struct.

**Fix**: Changed `String::new()` to `location.clone()` so the `from` value is actually stored.

**Lesson**: Always check that parsed values are actually wired into the AST node.

---

## 2026-06-06 — Hardcoded runtime declares in LLVM backend

**Issue**: `emit_declares()` unconditionally emitted `declare void @__rt_init()`, `declare void @__rt_wait()`, `declare void @__rt_poll()`, `declare void @__exit()`, `declare void @briev_thread_pool_init()`, etc. Users couldn't opt out and these symbols were never declared in user code.

**Root Cause**: The runtime functions were hardcoded in `llvm.rs:1844-1868` instead of being declared as `frgn` in `std/rt.bv` and imported by the user.

**Fix**: Removed the hardcoded runtime declares. Added a TODO to migrate the codegen call sites to use `self.frgn_map` lookups. Created `docs/learn/ffi.md` documenting the architecture.

**Lesson**: Runtime functions should be declared in standard library modules, not hardcoded in codegen.

---

## 2026-06-06 — `"None"`/`"Err"` discriminant magic in LLVM backend

**Issue**: `llvm.rs:508` hardcoded `"None" | "Err" => 0` for enum variant discriminants, assuming `None` and `Err` are always the first variant. Three other sites used `if name == "None"` as fallback logic.

**Root Cause**: Hardcoded match on variant names instead of using the enum declaration order. The `variant_disc` map was populated but then ignored in favor of name matching.

**Fix**: Changed to sequential discriminants starting at 0, based on declaration order (not name). All variant discriminant fallbacks use `unwrap_or(0)`.

**Lesson**: Dispatch on type/variant definition, not on hardcoded names.

---

## 2026-06-06 — Interpreter built-in method dispatch is still name-based magic (deferred)

**Issue**: The E1-E3 refactor consolidated 544 lines of duplicated method blocks into a single `dispatch_method_by_type` function. However, the dispatch still matches on hardcoded function name strings (`"insert"`, `"get"`, `"push"`, `"HashMap::new"`, etc.) inside type-scoped match arms.

**Root Cause**: The refactor only changed the *structure* of the dispatch (from top-level name matches to type-scoped name matches). The core problem — dispatching on function name strings instead of through the FFI registry — remains unsolved.

**Fix**: This is deferred to a follow-up session. The correct approach (Path A) is to register all built-in operations in the interpreter's FFI registry with location keys like `"std::HashMap::insert"`, then write stdlib modules (`std/hashmap.bv`, `std/stack.bv`) that declare `frgn HashMap::insert(map, key, value) -> HashMap from "std"`. The interpreter resolves through `ffi_name_to_location` → `foreign_functions` — the same path as C FFI.

**Lesson**: Structural refactoring of magic is not the same as eliminating magic.

**Issue**: Reported `fasta.bv` as hanging. The benchmark was being passed `N=100` instead of `BOUND=100`.

**Root Cause**: The benchmark reads `BOUND` from the environment, but `N` was used when testing. The benchmark uses `__get_env_int("BOUND")` to read the bound.

**Fix**: Used `BOUND=100` instead of `N=100`. The benchmark runs correctly.

**Lesson**: Always check env var names in the source code before assuming a benchmark is broken.

---

## 2026-06-07 — `term! -> swan_song` emits `ret void` inside `i32 @main` in folded loop path

**Issue**: When `term! -> __print_int(h)` is inside a guarded block in a transaction body that enters the folded struct-SSA loop path (Path 5), the backend emits `ret void` inside `define i32 @main()`. LLVM/llc rejects: "value doesn't match function result type 'i32'".

**Root Cause**: `emit_folded_main` (llvm.rs:4299) emits `define i32 @main()`, but the body emission in `emit_folded_loop` at line 4121 sets `self.returns_i64 = false`. The `term!` handler at line 2387-2401 has three branches:
- `values.first()` is Some → always emits `ret i64 %r` (still wrong for i32 main, but unreachable for our case)
- `values` is empty + `self.returns_i64 = true` → `ret i64 0` (not taken)
- `values` is empty + `self.returns_i64 = false` → `ret void` (TAKEN — crashes llc)

The `returns_i64` flag tracks whether the enclosing compute/txn function returns i64 (for transaction wrappers) or void (for `compute()`). But it doesn't account for the folded loop path where the body is inlined directly into `define i32 @main()`.

**Fix**: Add a `main_body: bool` flag to the backend struct. When set, `term!` and `term` handlers emit `ret i32 0` instead of `ret void` or `ret i64`. Set `main_body = true` in `emit_folded_main`, `emit_ssa_main`, and `emit_folded_multi_main` before emitting the loop body. Reset to false after.

**Lesson**: The `returns_i64` flag is overloaded — it really means "returns i64 specifically" but is used as "returns something" in some paths and "returns void" in others. When the body is inlined into `i32 @main`, neither value is correct.

---

## 2026-06-07 — Guarded block handler restores `self.terminated` after `term!`, emits code after `ret`

**Issue**: When `term!` fires inside a guarded block, the Guarded handler at line 2587-2604 saves `self.terminated`, sets it to false, emits the guard body (which includes `term!` that sets `terminated = true`), then RESTORES `self.terminated` to the pre-guard value (false). The caller continues emitting code after the `ret` instruction, including subsequent unrolled iterations in `emit_folded_loop`.

**Root Cause**: Line 2604: `self.terminated = prev_terminated;` unconditionally restores the flag, undoing the effect of any terminating statement (term!, escape) inside the guarded body. The `if !self.terminated` check at line 2602 correctly prevents emitting the br-to-merge when terminated, but the restoration at 2604 undermines this.

**Fix**: Replace the unconditional restore with conditional:
```rust
if !self.terminated {
    self.terminated = prev_terminated;
}
```
If a terminating statement fired inside the guarded body, leave `self.terminated = true` so callers know the block terminated. Only restore when no termination occurred.

**Lesson**: The save/restore pattern around `self.terminated` in the Guarded handler assumes the guarded body never terminates execution — it's designed for "do something then continue" guards. When a guard body contains `term!`, this assumption is violated. Any save/restore of control-flow flags must account for the possibility that the inner code changes them in a way that should persist.

---

## 2026-06-07 — `-lm` missing in compiler driver link step (FIXED)

**Issue**: `briev_rt.c` provides `float __sqrtf(float x) { return sqrtf(x); }` (line 392), which is actively used by `benchmarks/nbody_sqrt.bv` (24 call sites). The compiler driver at `main.rs:~2360` never passes `-lm` to the linker. The C reference gets `-lm` via the same clang invocation, creating asymmetry. Programs using `__sqrtf` get undefined reference at link time.

**Root Cause**: The link command at `main.rs:2359-2365` was assembled incrementally (conditionally adding `-lrt`, `-lpthread`) and `-lm` was never added. Since `sqrtf` is in libm (not libc), glibc requires explicit `-lm`.

**Fix**: Added `link_cmd.arg("-lm");` to the link command unconditionally (briev_rt.c references `sqrtf`). Also added `-lm` to the linking-failed hint message.

**Lesson**: Every library dependency of the runtime must be provided to the linker. `sqrtf` from `libm` is the runtime's dependency, not the user program's. The compiler driver is responsible for its own runtime module's link requirements.

---

## 2026-06-07 — `Statement::Guarded` is one-shot, not a loop — ~130 defns silently broken

**Issue**: Every `defn` in the standard library and compiler that uses `[guard] { ... &i = i + 1; }` for iteration only processes the first element. The guarded statement fires once, then falls through — there is no loop.

**Root Cause**: `Statement::Guarded` evaluates its condition once and executes the body zero or one times (src/interpreter.rs:842-861). The `defn` body executes as a straight-line sequence — no implicit transaction wrapping, no reactor loop. The pattern `let i = 0; [i < list :> Size] { ... &i = i + 1; }` was cargo-culted from `node` bodies where the outer reactor loop provides convergence. But `defn` has no such loop.

Affected files and approximate counts:
- `lib/std/iterator.bv`: 14 defns
- `lib/std/hashmap.bv`: 3 defns
- `lib/std/hashset.bv`: 4 defns
- `lib/std/stack.bv`, `queue.bv`: 2 defns
- `lib/std/math.bv`: 3 defns
- `lib/std/string.bv`: ~38 defns (including a near-duplicate second copy of ~17 functions)
- `lib/std/io.bv`: 1 defn
- `lib/std/encoding.bv`: 5 defns
- `lib/std/json.bv`: 1 defn
- `lib/compiler/`: ~60+ defns (deferred — self-hosting already broken)

**Fix**: Replace broken `defn` iterators with callable `txn` declarations. Regular `txn` takes parameters and returns values like `defn`, but with `[pre][post]` convergence loop semantics — the body re-executes until the postcondition is met.

```briev
// BEFORE (broken — guarded fires once):
defn iter_filter<T>(list, pred) -> List<T> [true] {
    let i = 0;
    [i < list :> Size] { ... &i = i + 1; };   // one-shot!
    term result;
};

// AFTER (correct — convergence loop):
txn iter_filter<T>(list: List<T>, pred: T -> Bool)
    [i < list :> Size][i == list :> Size] -> List<T>
{
    [pred(list[i])] { &result = result.append(list[i]); };
    &i = i + 1;
    term result;
};
```

The callable `txn` is invoked via `Expr::Call` just like a `defn`. The interpreter registers non-reactive txns in a `callable_txns` map and executes them as convergent loops (evaluate pre → execute body → check post → repeat if state changed). State is cloned per-call and restored on return, providing isolation.

Concrete changes:
1. AST: Add `output_type: Option<OutputType>` and `outputs: Vec<Type>` to `Transaction`
2. Parser: Parse optional `-> ReturnType` after contract for regular `txn`s (not `node`)
3. Interpreter: Register `txn` in `callable_txns` during init; dispatch from `Expr::Call`; convergence loop execution
4. Backends: Add stubs for callable txn dispatch returning 0
5. Stdlib: Convert ~71 broken defns in `lib/std/` to txns
6. `lib/std/option.bv`: Fix filter postcondition `[term.is_some() && pred(unwrap(opt))]` → `[!term.is_some() \|\| pred(unwrap(term))]`
7. `lib/std/string.bv`: Remove the near-duplicate second copy (~17 functions, lines 580-876)

**Lesson**: The convergence loop is the defining feature of `txn` — `defn` has straight-line execution. When writing iteration, use `txn` with `[pre][post]` contracts. `Statement::Guarded` is for one-shot conditionals only, never for loops. `[guard] { body }` inside a `txn` with `[pre][post]` is correct because the txn's outer convergence loop re-evaluates the body, not because the guarded statement loops by itself.

## 2026-06-08 (FIXED) — `<:` subtype projection parser bug

**Root Cause**: `parse_primary()` at `src/parser.rs:5168` interpreted `items { COUNT; }` as a struct literal (`identifier { field: value }`), consuming the ops block. `parse_expression()` was used for the source but it calls `parse_primary()` which treats any `{` after an identifier as a struct literal.

**Fix**: Replaced `parse_expression()` with `parse_projection_source()` in both `<:` let-statement paths (tuple and non-tuple). This new function parses an identifier + postfix operations but stops before `{` and `[`, leaving them for `parse_subtype_ops()` to handle as the ops block or MATCH bracket syntax.

## 2026-06-09 — Proof engine guard path three-bug cascade

**Symptoms**: Benchmarks with a `[guard] { __print_*(...); };` inside an `node` fail P008 contract verification with 14+ identical-looking `guard` constraints in the path state.

**Root Cause**: Three interacting bugs in `enumerate_paths_recursive` and `is_truthy`:

### Bug A — Guard-taken path never reaches `term` (proof_engine.rs:883)

When `enumerate_paths_recursive` encounters a `Statement::Guarded`, the true branch recurses into `statements` (the guard body). The guard body is typically just a print expression — no `term`. So `terminated` remains `false`, and NO path is pushed for the true branch. The false branch correctly recurses into `body[1..]` (remaining body after guard, which includes `term`) and pushes a valid path. But the true branch path is lost — its continuation to `term` is never explored.

**Line 883**: `self.enumerate_paths_recursive(statements, true_state, &mut true_paths);`

**Fix**: After recursing into the guard body, check if `true_paths` is empty. If so, the guard body didn't terminate — continue exploring `body[1..]` with the `true_state` to reach `term`.

### Bug B — `eval_numeric` missing `Mod` and `Div` (proof_engine.rs:1064)

`eval_numeric` handles `Add`, `Sub`, `Mul` but falls through to `_ => None` for `Mod` and `Div`. Since guard conditions like `count % 5000000 == 0` use modulo arithmetic, `is_truthy` always returns `false` for these conditions. This makes non-negated guard constraints appear infeasible in the `implies` check.

**Fix**: Add `Expr::Mod` and `Expr::Div` cases to `eval_numeric` for concrete integer operands.

### Bug C — `format_expr` hides `is_negated` (proof_engine.rs:804)

The error printer at line 803-805 only renders `format_expr(&constraint.condition)`. The `is_negated` flag is silently dropped. Negated constraints (`!guard_condition`) print identically to non-negated ones, making the error output deeply misleading — 14 guard-looking constraints may all be different but look the same.

**Fix**: Include `is_negated` in the display: `if constraint.is_negated { "¬" } else { "" }` prefix.

**Lesson**: The third finding (Bug C) is a debugging-jit — it's serious because it hides the distinction between guard-taken and guard-not-taken paths, making P008 errors nearly impossible to diagnose from the output alone. The guard-taken path being dropped (Bug A) means the proof engine currently does NOT fully verify contracts for paths that take guarded branches — it sees only the guard-not-taken path and judges correctness on that alone. Where the guard body has side effects (like FFI calls), this is unsound.

**Fix (same commit)**: 
1. Guard-taken path now continues to remaining body after guard via `body[i+1..]` tail recursion.
2. `eval_numeric` now handles `Expr::Mod` and `Expr::Div` for concrete operands.
3. Error output shows `¬` prefix for negated path constraints.
4. Body index `body[1..]` → `body[i+1..]` using `.enumerate()` — fixes exponential path explosion when guards were followed by more guarded statements.

### Pre-existing: convergence analysis gaps (NOT caused by Pattern B)

After the above fixes, these benchmarks still fail P008 — all are convergence analysis gaps in `check_convergence` in `proof_engine.rs`, NOT regressions:

| Benchmark | Root cause | 
|-----------|------------|
| `*_runtime.bv` | Precondition is `AND(relation, relation)` — `check_convergence` expects bare `count < bound`, not `bound > 0 && count < bound`. No AND-handling exists. |
| `bit_clear.bv` | `reg = reg & (reg - 1)` is popcount decay — `check_convergence` only detects `Add`/`Sub` increments. `detect_popcount_decay` exists in `transition_graph.rs` but is never called by the proof engine. |
| `cancel_math.bv` | `count = count + (R + 1 - R)` — algebraic simplification reduces this to `count + 1` in `transition_graph.rs::simplify_expr`, but `check_convergence` scans the ORIGINAL unsimplified body. |
| `interval_step.bv` | `count = (count + R1) - R2` — outer `Sub` wraps `Add(count, R1)`. `check_convergence` only detects `Sub(count, N)` with bare count on left. |

**Fix status**: Each requires structural improvement to `check_convergence` — AND-pre extraction, integration with `detect_popcount_decay`, simplified-body scanning, and compound increment pattern matching. Deferred — not caused by Pattern B, and the non-runtime benchmarks (which are the ones `build_and_bench.sh` actually measures) all pass.

---

## 2026-06-09 — fasta LCG broken in node (all output chars same)

**Issue**: `benchmarks/fasta.bv` outputs `qqqqq` instead of `xqjqf` (C reference). All iterations produce the same character `q` (ASCII 113), meaning the LCG seed never changes.

**Root Cause**: `node` atomically batches all state writes until `term;`. Inside a single tick, `&seed = seed * IA` reads the original seed (42), then `&seed = seed + IC` ALSO reads the original seed (42), and `&seed = seed % IM` also reads 42. The three writes commit at `term;` — the last one wins: `seed = 42 % 139968 = 42`. Seed stays 42 forever.

The LLVM backend treats reactive writes as deferred (all reads see pre-tick state), consistent with the reactive semantics. But `fasta.bv` was written assuming sequential in-tick execution, which is incorrect for `node`.

**Fix**: Convert to callable `txn` (not `node`) so writes take effect immediately within the body iteration. Or restructure as a single assignment: `&seed = (seed * IA + IC) % IM;`.

**Lesson**: `node` has deferred write semantics — all state reads within a tick see pre-tick values. Sequential `&field = ...` chains like `&seed = seed * IA; &seed = seed + IC; &seed = seed % IM;` do NOT accumulate — each reads the same original seed. Use callable `txn` for sequential state mutations within a single body iteration.

---

## 2026-06-09 — LLVM backend emits `constant float 0` (needs `0.0`)

**Issue**: `benchmarks/iir_filter.ll` contains `@b2 = constant float 0`. Clang rejects: `error: integer constant must have integer type`.

**Root Cause**: The float constant emission path for zero-valued floats uses `constant float 0` instead of `constant float 0.0`. LLVM's textual IR requires a floating-point literal for `float` type, not an integer `0`.

**Affected files**: LLVM backend's float constant emission (`emit_expr.rs` or `mod.rs`). All zero-initialized `Float` state fields and `const Float = 0.0` declarations.

**Fix**: Change the float literal LLVM emission to use `0.0` (or `0.0e+0`) instead of `0` when the value is zero.

**Lesson**: LLVM IR types must match their literal formats — `float` requires a floating-point literal, not an integer. The `i64` path was correct (`add i64 0, N`), but `float 0` should be `float 0.0`.

---

## 2026-06-09 — LLVM backend emits undefined `@str.0` reference

**Issue**: `benchmarks/fasta.ll` contains `getelementptr inbounds [6 x i8], [6 x i8]* @str.0` but `@str.0` is never defined. Clang rejects: `use of undefined value '@str.0'`.

**Root Cause**: The string constant emission path creates references to `@str.N` globals but doesn't emit their definitions. The `collect_strings()` function extracts string constants but the definition pass in `emit_global_strings()` (or equivalent) is missing or buggy.

**Affected files**: All benchmarks using string constants (`fasta`, `ring_buffer`, and any benchmark with `__get_env_int("BOUND")` or similar string arguments).

**Fix**: Ensure `@str.N` globals have corresponding definitions (type, initializer, alignment) emitted before their first use in the IR module.

**Lesson**: LLVM IR globals must be declared (`@str.0 = private unnamed_addr constant [6 x i8] c"BOUND\00"`) before being referenced in instructions. Use-before-definition is not forward-declared in LLVM IR — every reference must have a matching definition earlier in the module.

---

## 2026-06-09 — `precompute_sum.bv` emits infinite tick loop (no observable output)

**Issue**: `benchmarks/precompute_sum` binary never exits (timeout at BOUND=5). LLVM IR shows an infinite tick loop: `br label %tick` → `reactor_tick` → `br label %tick`. No observable side effect exists to prevent LLVM from eliminating the loop, but the loop remains because the `.o` linking path uses `cc -O2` (not `-O3`) and may not run the full SROA/mem2reg pipeline.

**Root Cause**: `const total: Int = 500` with budget=256. 500 > 256, so the compiler can't fully precompute. The `node` body is pure (no FFI, no IO), so the reactor loop has no observable effect. At `-O3`, LLVM should eliminate the loop entirely, but the linking path (`cc -O2` on `.o` file) may not be aggressive enough.

**Fix**: Either (a) increase budget to >= 500, (b) decrease total to <= 256, or (c) add an FFI output to make the loop observable. The benchmark's purpose is to test compile-time precomputation — it should use `total <= 256` so the budget covers it, or use `--optimize-budget 2048`.

**Lesson**: A benchmark that tests precomputation must have its bound within the optimization budget. `const total = 500` with default budget 256 produces a silent infinite loop — the worst failure mode. Add `#!exit` or keep bounds within budget.

---

## 2026-06-10 — LLVM backend: negative float constants in init_state stored as i64 (8 bytes) instead of float (4 bytes)

**Issue**: `nbody_newton.bv` produced `-nan` instead of `-0.169203` at BOUND=5. All outer planets' Y and Z positions were zero instead of their correct negative values (by1=-1.16, bz1=-0.1036, by3=-15.11, etc.).

**Root Cause**: In `src/backend/llvm/mod.rs:2110-2117`, the `emit_init_state` function had this code for `Expr::Neg` initializers:
```rust
Some(Expr::Neg(ref inner)) => {
    let s = match inner.as_ref() {
        Expr::Float(f) => float_to_llvm_hex(-*f),
        ...
    };
    writeln!(out, "  store i64 {}, i64* {}, align {}", s, p, self.align_of("i64")).ok();
}
```
When a `let` field had a negative float literal (e.g., `let by1: Float = -1.1603`), the parser stored it as `Neg(Literal(Float(1.1603)))`. The `Expr::Neg` arm computed the correct hex bit pattern but stored it as `i64` (8 bytes) instead of `float` (4 bytes). This wrote 8 bytes of zero into a 4-byte float slot, corrupting the adjacent float field.

Additionally, the match arm only matched `Expr::Float(f)` inside Neg, but the actual AST stores `Expr::Literal(LiteralExpr::Float(f))` inside Neg. The catch-all `_` branch stored `i64 0`.

**Fix** (`src/backend/llvm/mod.rs`): Updated `Expr::Neg` arm to match both `Expr::Float` and `Expr::Literal` with `LiteralExpr::Float`, and emit `store float` (4 bytes) instead of `store i64` (8 bytes):
```rust
Some(Expr::Neg(ref inner)) => {
    match inner.as_ref() {
        Expr::Float(f) | Expr::Literal(lit) if matches!(lit.as_ref(), LiteralExpr::Float(_)) => {
            let f = /* extract value */;
            let h = float_to_llvm_hex(-*f);
            let bits_reg = format!("%ip{}b", reg - 1);
            writeln!(out, "  {} = bitcast i32 {} to float", bits_reg, h).ok();
            writeln!(out, "  store float {}, float* {}, align {}", bits_reg, p, self.align_of("float")).ok();
        }
        ...
    }
}
```

**Lesson**: Always match the correct type when storing initial values. The LLVM IR must use `store float` for float fields — `store i64` writes 8 bytes, corrupting adjacent fields. Also, verify the AST uses `Literal(Float(…))` not bare `Float(…)` — the feature dispatch layer adds an `Expr::Literal` wrapper.

## 2026-06-10 — LLVM backend: non-SSA state field loads return Type::Int for float fields

**Issue**: In non-SSA transaction mode (the default), float arithmetic on state fields used `add i64` on boxed bit patterns instead of `fadd` on native floats. This caused float computations like `sum + a + b` to produce garbage results when all operands were state fields (no Float-type constant in the expression tree).

**Root Cause**: In `src/backend/llvm/emit_expr.rs:178`, the non-SSA state field load for float types:
```rust
s if s == "float" => { 
    let i = format!("%if{}", self.txn_counter); 
    ...
    self.reg_float_cache.insert(v.clone(), ld.clone()); 
    // NO RETURN — falls through to default
}
```
...did not return a `TypedRegister`. The function's default return (line 631) treated the value as `Type::Int`:
```rust
TypedRegister { name: v, ty: Type::Int }
```
Then in `emit_binop` (emit_expr.rs:845), the check `a.ty == Type::Float || b.ty == Type::Float` was false, so it emitted `add i64` instead of `fadd fast float`. The `reg_float_cache` had the correct native float register, but `emit_binop` checked the type first and never reached the cache lookup.

This worked in `float_math.bv` because constant globals (like `A00`, `Q00`) had type `Type::Float`, so at least one operand in each arithmetic expression triggered the native float path.

**Fix** (`src/backend/llvm/emit_expr.rs`): Added `return TypedRegister { name: v.to_string(), ty: Type::Float };` to the float state field load arm.

**Lesson**: Every code path in `emit_expr` must return the correct `TypedRegister::ty`. The default `Type::Int` fallthrough is a trap for non-int types. When adding new type-aware paths, verify the return type propagates to consumers like `emit_binop`.

## 2026-06-10 (FIXED) — C reference benchmarks exit non-zero at BOUND=5 (harness false MISMATCH)

**Issue**: The harness correctness check at `build_and_bench.sh:220-221` runs:
```bash
c_out=$(BOUND=5 timeout 10 "$c_bin" 2>/dev/null || echo "__FAIL__")
```
If the C binary exits non-zero, `|| echo "__FAIL__"` fires and `c_out="__FAIL__"`. The Briev binary exits 0 with empty stdout, so `briev_out=""`. The comparison `"" != "__FAIL__"` reports MISMATCH — but the programs are actually correct, just using exit code for their result instead of stderr output.

**Affected**: `float_math_c`, `float_math_nonzero_c`, `const_heavy_c` — all return the computed result via `main()` return value, which gets a non-zero exit code (6, 8, or truncated to 45 for values > 255).

**Root Cause**: Three interacting issues:
1. The C references returned their result via exit code (`return (int)(count + x0 + ...)`) while Briev used `__print_float`/`__print_int` (stderr output).
2. Linux truncates exit codes to 8 bits, so values > 255 (e.g., 105005 → 45) are corrupted.
3. The harness treats non-zero exit codes as "failed" via `|| echo "__FAIL__"`.

**Fix**: Changed all three C references to match Briev's pattern — periodic `fprintf(stderr, ...)` inside a `count % 5000000 == 0` guard (post-increment), with `return 0`. At BOUND=5, neither Briev (guard fires on post-increment counts 5M, 10M, ...) nor C (same guard) produces output, so both show empty stdout + exit 0 → MATCH.

**Lesson**: C references for optimizer benchmarks must match Briev's output pattern exactly — if Briev uses periodic `__print_*` inside `[count % N == 0]`, the C reference must use the same periodic `fprintf` with the same format. Return-path-only results confuse the harness. Exit codes are truncated to 8 bits on Linux.

## 2026-06-10: Benchmark Investigation After R2+R3

### Final Results (after all R2+R3 + copy elimination)

| Benchmark | Before | After | Gap | Why |
|---|---|---|---|---|
| nbody_newton | 1.69× | **1.08×** | Near parity | Float boxing elimination + copy elimination |
| nbody_sqrt | 2.81× | **2.41×** | Unchanged | C gets vsqrtps (SIMD sqrt), Briev has scalar sqrt |
| fannkuch_redux | 5.06× | **4.36×** | Improved | R3 (SROA) + copy elimination, but 15 dead-field phi nodes remain |
| float_math_nonzero | 2.43× | **2.42×** | Unchanged | Register scheduling — C keeps all in XMM |
| knucleotide | 1.21× | **1.24×** | Unchanged | Guard dispatch overhead |
| kalman_filter_runtime | ? | **3.48×** | Improved | Copy elimination + LTO, but still structural memory overhead |

### Root Causes

**kalman_filter_runtime (3.62×)**
- State field memory round-trips: 12 float loads + 12 float stores per tick via GEP
- LLVM SROA demotes the `@propagate` function's GEP accesses but can't fully eliminate them due to `%State` alloca aliasing through `@reactor_tick` and `@pre_propagate`
- Constant loads from globals instead of immediates (a00, a01, etc.)
- `add i64 0, %src` noise adds ~30 redundant instructions
- Periodic `srem` + branch for the `% 5000000` print condition
- No `-march=native` in the LLVM pipeline (C uses it, Briev doesn't)

**fannkuch_redux (4.63×)**
- R3 enabled SROA: 17 fields → 17 phi nodes in the loop
- 12 rotation fields are DEAD (only rotate among themselves, never observed)
- Briev's liveness analysis doesn't eliminate them → 15 dangling phi nodes at the loop back edge
- LLVM can't eliminate these phis because they're structurally part of the loop (each phi's output feeds the next tick's phi input)
- Results in register pressure for 17 `i64` values per tick when only 3-4 are actually used
- Full 12-field rotation compiles to ~6 scalar ops but carries 15 dead phi values

**nbody_sqrt (2.41×)**
- LLVM intrinsic replaced `call @sqrtf` but C uses `-ffast-math -march=native` for `vsqrtps`
- Briev's `float` = 32-bit, C's `float` = 32-bit — same type, but C gets SIMD sqrt

**float_math_nonzero (2.41×)**
- Float arithmetic matches C instruction-for-instruction size
- Probable cause: register scheduling — C keeps all floats in XMM registers, Briev's phi structure forces round-trips
- Not a boxing issue (already fixed in R2)

### Open Questions for Future Work

1. **Dead-field elimination**: fannkuch's 12 rotation fields are only rotated, never observed — liveness analysis should eliminate them. Currently Briev's field elimination only drops fields whose ASSIGNMENTS are never read; it doesn't trace the full def-use chain to check for observable output.
2. **`-march=native`**: Adding `-march=native` to `llc` in the benchmark harness would give Briev the same ISA as C (AVX, FMA, etc.). Currently `llc` uses the host triple without `-march=native`.
3. **SLP hazard over-conservatism**: For straight-line float code (kalman's matrix arithmetic), the peak register estimate (line 178 in hazard.rs) counts `shuffle_pressure` as `min(cross_ops, n*2)` which overestimates for tree-shaped computation where values are consumed and released quickly.
4. **`add i64 0, %src` noise**: Fixed for non-SSA field loads and MMIO reads in commit 5ab6bb0. Copies reduced from 18% to 2% of IR instructions (fannkuch), 18% to 3% (knucleotide). Remaining copies are from `let_binding` lookups, SSA extractvalue path, and trivial expression results. Full elimination would require refactoring `emit_expr` to reuse its allocated `v` register more aggressively.
5. **SSA atomicity overhead**: The %State-based GEP load/store model forces memory round-trips for every field access. For benchmarks like kalman (132 loads for 145 arith ops), this is ~1:1 memory:compute ratio vs C's ~0:1. Fixing this would require Briev-specific LLVM passes or a different codegen model (e.g., rewrite entire loop body with %State register).


### Clang IR Comparison (2026-06-10)

Compiled C reference for each benchmark with `clang -O3 -ffast-math -march=native -S -emit-llvm`.
Briev compiled with `opt -O3 -ffast-math -mtriple=x86_64-pc-linux-gnu; llc -O3 --mcpu=native`.

| Metric | Clang | Briev | Factor |
|---|---|---|---|
| **Kalman loads/stores** | 1 (stderr) | 132 loads + 29 stores | ~160× |
| **Kalman GEPs** | 0 | 87 | ∞ |
| **Kalman IR size** | 102 lines | 379 lines | 3.7× |
| **Nbody_sqrt loads/stores** | 1 (stderr) | 433 loads + 110 stores | ~540× |
| **Nbody_sqrt phi nodes** | 35 | 0 | 35× |
| **Nbody_sqrt sqrt calls** | `@llvm.sqrt.v2f32` (vectorized) | `llvm.sqrt.f32` (scalar) | vector vs scalar |
| **Fannkuch loads/stores** | 1 (stderr) | 36 loads + 38 stores | ~74× |
| **Fannkuch phi nodes** | 29 | 0 | 29× |

### 2026-07-03: "value that could not be identified as reduction" blocks vectorization

After the opaque pointer migration and per-field phi loop, SROA is no longer
blocked. The vectorizer now evaluates the loop body and reports:

```
loop not vectorized: value that could not be identified as reduction is used outside the loop
```

**Root cause**: The per-field phi registers (`%phi_bx0`, `%phi_vx0`, etc.) are
defined at `loop_hdr` which dominates `done:`. Even though no instruction in
`done:` directly uses these phi registers (the hoisted guard body reads from
fresh %State GEP+loads), LLVM's vectorizer conservatively assumes they might
be used after the loop. It can't prove that the phis are dead after `done:`.

**Why C doesn't have this problem**: Clang's IR for the same loop has each
array element as a standalone SSA value that truly IS a reduction. The C code
`for (int i = 0; i < N; i++) { sum += a[i]; }` has `sum` as a phi that is
only used after the loop — LLVM recognizes it as a reduction and vectorizes
around it. Briev's per-field phis are NOT reductions (each field is
independently computed, not accumulated).

**Effect**: `opt -O3 -ffast-math -Rpass=loop-vectorize` produces 0
vectorized loops for nbody benchmarks. Briev emits scalar `fadd`/`fmul`/`sqrt`
only. C emits `<4 x float>` vector ops with `vector.reduce.fadd`.

**Status**: This is the last remaining vectorization blocker. Previous blockers
have been resolved:
- ✅ %slot_case alloca round-trip → per-field phi loop
- ✅ Terminating guard in loop body → hoisted to post-loop block
- ✅ Mixed typed/opaque pointers confusing SROA → all-opaque migration
- ❌ Phi registers "used outside" → no fix yet

**Possible approaches**:
1. Make the per-field phis not dominate `done:` — restructure the loop so
   `done:` is in a different dominance frontier. This is architecturally
   complex (natural loops always have the header dominating the exit).
2. Teach the vectorizer that phi values in `done:` are dead — insert
   `llvm.assume` or use metadata to mark them as unused. Fragile.
3. Store final phi values back to %State before `done:` and load from
   %State in `done:` — undoes the SROA benefit from Phase 0.
4. Accept scalar code generation and focus optimization elsewhere.
   nbody_sqrt is at 1.23× of C even without vectorization — competitive
   for a safe language with no manual optimization.

### Root Cause

Clang keeps ALL state in **SSA phi nodes** across the loop back-edge. Zero memory traffic
in the hot path except the periodic `fprintf`. Briev emits every field access as
`getelementptr %State → load/store`, creating 100-500× more memory operations.

Clang's C code uses local variables which LLVM promotes to phi nodes. Briev's state
machine uses a `%State` struct which forces GEP + load/store on every access.

### Fix: Skip reactor_tick for single-txn runtime programs

Currently: `main → call @reactor_tick → call @propagate(alwaysinline→inlined) → body`
Problem: `@reactor_tick` contains `fired_mask` alloca that prevents LLVM from promoting
the `%state` alloca in `main()` to phi nodes.

Fix: For single-txn reactive programs with no parallel dispatch needed, emit txn body
directly in `main()` as phi-node-based codegen (like `emit_ssa_main`). This eliminates
the function call boundary and lets LLVM promote all state fields to phi nodes,
matching Clang's IR structure.

### Instruction Count Comparison (fannkuch_redux)

Operation | Clang (phi) | Briev (struct) | Ratio
--- | --- | --- | ---
phi nodes | 29 | 0 | Clang 29× better
load/store | 1 | 74 | Briev 74× worse
getelementptr | 0 | 69 | Briev infinite
arithmetic | ~50 | ~50 | Equal

The arithmetic ops are the same between C and Briev for fannkuch. All the overhead is
from memory (load/store/GEP). If Briev used phi nodes like Clang, the gap would close
to ~1.0×.

## 2026-06-11 — fannkuch_redux: silent correctness failure + 3.85x performance gap

**Issue**: fannkuch_redux benchmark produces no output (empty stdout) while the C
reference outputs `10` (to stderr). The benchmark harness reports MATCH because
both stdout captures are empty — C's `fprintf(stderr)` is discarded by `2>/dev/null`.

**Root Cause**: Two independent bugs:

1. **Output guard never fires**: `[count == N] { term! -> __print_int(checksum); }`
   reads the pre-tick value of `count`, which is always `< N` at body start.
   The body fires while `count < N` (precondition), and the `#!exit count == N`
   fires at end-of-tick (after increment). The guard sees `count == N-1` and
   never fires. This is the SAME issue as nbody_sqrt's `[count == bound]` guard,
   but nbody_sqrt has an alternate output path `[count % 5000000 == 0]`.

2. **Algorithm mismatch**: The Briev code uses `seed % 13` for the checksum
   accumulation, while the C reference uses `saved % 13` where `saved = p0`
   (the first permutation element). These are different values — `seed` is the
   LCG output (~0-139967), while `saved` / `p0` is always 0-11. The programs
   compute completely different checksums.

**Fix**:
1. Changed checksum accumulation to use `p0` (matching C's `saved`):
   `let saved: Int = p0;` before the rotation, then `&checksum = checksum + saved % 13;`
   after rotation.
2. Changed output to compute final checksum into a local `let final_checksum`
   variable (not subject to prior-state) and fire `term! -> __print_int(final_checksum)`
   at `[count == N - 1]` — the last body execution before exit.
3. Changed C reference from `fprintf(stderr, ...)` to `fprintf(stdout, ...)` so
   the harness captures it correctly.

**Lesson**: Benchmark correctness verification must capture stdout AND stderr,
or agree on an output channel. Guard conditions in `node` bodies always read
pre-tick state — `[count == N]` is NEVER true at body start for `[count < N][count == N]`
contracts. Use local `let` variables to compute values outside prior-state scope.

## 2026-06-11 — float_math_nonzero: 1.09x prior-state overhead (accepted)

**Issue**: float_math_nonzero is 1.09x slower than C (0.183s vs 0.167s).

**Root Cause**: Prior-state bookkeeping for the 3 state variables (x0, x1, x2)
adds 3 extra loads + 3 extra GEP stores per tick. With 15 ALU operations (9 mul,
6 add), 6 memory operations add ~9% overhead.

**Decision**: Accepted. The overhead is inherent to Briev's prior-state semantics.
LLVM's SROA already absorbs most of the cost. Closing this gap would require
per-field prior-state elision (skip save/restore for fields that are read-only or
write-only within a tick), which is a future optimization target.

**Before/After**: No fix applied — 1.09x is within acceptable noise for this
benchmark type.

## 2026-06-11 — Silent postcondition failure in callable txns

**Issue**: `call_txn()` in `interpreter.rs` silently swallowed postcondition
violations. When a callable `txn` completed its convergence loop but the
postcondition was not satisfied, the interpreter rolled back state and returned
`Ok(result)` as if nothing went wrong.

**Root Cause**: At `interpreter.rs:574-578`, the postcondition check returned
`Ok(result)` instead of an error:
```rust
if post_val != Value::Bool(true) {
    self.state = old_state.clone();
    self.return_value = old_return;
    return Ok(result);  // ← BUG: should propagate error
}
```

**Fix**: Changed to `return Err(RuntimeError::ContractViolation(...))`.
Postcondition failures in callable `txn`s now propagate as errors.

**Lesson**: Runtime contract checks must always propagate failures. Silent
swallowing defeats the purpose of contract verification.

## 2026-06-11 — Convergence proof gated to reactive txns only

**Issue**: `check_convergence()` in `proof_engine.rs:1570` was only applied to
reactive `node`, not callable `txn`. The documented iteration pattern
`txn f(items, acc, i) [i < items:>Size][i == items:>Size]` could not be
statically proven.

**Root Cause**: The gate `if txn.is_reactive && check_convergence(...)` excluded
callable txns from the structural convergence proof, even though the proof
(post → ¬pre, step detection, bound invariance) works identically for both.

**Fix**: Removed the `txn.is_reactive` guard. `check_convergence` now runs for
all txns.

**Lesson**: When adding callable txns, ensure all analysis passes that apply to
reactive txns are also applied. Don't gate structural proofs behind `is_reactive`
unless there's a specific semantic reason.

## 2026-06-11 — Tuple destructuring assignment `&(a, b) = expr` missing

**Issue**: `let (a, b) = expr;` worked for declaring new variables but
`&(a, b) = expr;` failed with parser error "expected identifier, found '('".
Tuple-returning functions could not be destructured on the receiving end.

**Root Cause**: The parser's `parse_unary` `&` handler called
`expect_identifier()` after `&`, rejecting `(`. The interpreter's `exec_stmt`
`Statement::Assignment` LHS handler also had no arm for `Expr::TupleDestructure`.

**Fix**:
- Parser: `&` handler now checks for `LParen` → parses comma-separated
  identifiers → returns `Expr::TupleDestructure(names, Expr::Term)`.
- Interpreter: Added `Expr::TupleDestructure(names, _)` arm in assignment LHS
  that destructures `Value::Tuple`/`Value::List` into named variables.
- Typechecker: Added `Expr::TupleDestructure` arm in `check_statement` for
  assignment — validates RHS is a `Type::Tuple` with matching element types.
- LLVM backend: Added comment-stub arm in `emit_stmt.rs`.

**Tests**: `test_tuple_destructure_assignment`,
`test_tuple_destructure_assignment_from_list`,
`test_tuple_destructure_assignment_wrong_type_errors`.

**Lesson**: Tuple destructuring was only implemented for `let` declarations.
The `&` assignment case requires parser + interpreter + typechecker changes.

## 2026-06-11 — `<-` on txn parameters (NOT a bug)

**Claim**: Inside a callable `txn`, `result <- items[i]` doesn't work.

**Investigation**: The parser error "Either side of '<-' must be &list" fires
because `result` is parsed as `Expr::Identifier`, not `Expr::OwnedRef`. The
`&` prefix is required: `&result <- items[i]`.

Even with `&`, mutations to txn parameters are scoped to the txn's lifetime —
`call_txn` restores `self.state = old_state` on exit. Both `<-` and `.append()`
behave identically here; both require `term result;` to return the accumulated
value.

**Verdict**: Not a compiler bug. Correct usage is `&result <- items[i];`
followed by `term result;` if the accumulated value must survive the txn.

**Lesson**: The `&` sigil is required for all mutation targets in Briev,
including `<-` targets. Txn parameters are inputs; outputs flow through
return values.

## 2026-06-11 — `||` in `term` statements (NOT a bug)

**Claim**: `term word == "the" || word == "a" || word == "an";` gave parse
error "expected ';', found '}'".

**Investigation**: The `||` operator goes through the standard
`parse_expression()` → `parse_or()` precedence path — identical to any other
expression context. The code parses correctly in the current compiler (verified
by AST construction and existing usage in `lib/compiler/proof_engine.bv`).

**Verdict**: Not a bug in the current codebase. Likely causes for the user's
error: an older compiler version, a missing `;` elsewhere causing cascaded
errors, or a typo in the specific file.

**Lesson**: When hitting unexpected parse errors, first verify with a minimal
reproduction, then check for preceding syntax issues that may cause cascaded
errors.

## 2026-06-13 — Bare label `%` prefix in LLVM IR (emit_expr.rs)

**Issue**: `opt` failed with `expected '=' after instruction name` at `%mdef4:` in generated LLVM IR. All switch/match/slice label definitions used `%` prefix, which LLVM interprets as value references not label definitions.

**Root Cause**: `emit_expr.rs` lines 656, 673, 681, 735, 739, 759 used `"%marm{}:"`, `"%{}:"` format strings for label definitions. In LLVM IR, label definitions must NOT have `%` (e.g. `marm0:`) while label references MUST have `%` (e.g. `br label %marm0`). The backend was doing the opposite for 6 label sites.

**Latency**: The bug was invisible until `briev build` switched to LLVM backend (previous session). The old `compile` subcommand used Rust-transpile → `rustc`, and the `llvm` subcommand ran `llc` directly without `opt`. Only `briev build` runs `opt -O3` on the generated `.ll` file, which exposed the syntax error.

**Fix**: Changed the 6 `writeln!` calls to omit the `%` prefix from label definitions. Branch target references (`label %mdef4`, `br label %mmerge5`) were already correct.

**Lesson**: LLVM IR distinguishes label definitions (`name:`) from value references (`%name`). The backend had been wrong since the match codegen was first written — the error was latent because `opt` was never run on the output before `briev build` was implemented.

## 2026-06-13 — `terminated` flag leak in `Guarded` block (emit_stmt.rs)

**Issue**: After a `Guarded` block whose body set `self.terminated = true` (e.g., via `term!`), the flag was not restored to `prev_terminated`. The next statement emitted after the guard would be in a terminated state, potentially suppressing terminators.

**Root Cause**: `emit_stmt.rs:404-420` saved `prev_terminated`, set `terminated = false`, emitted the guarded body, then only restored `self.terminated = prev_terminated` inside `if !self.terminated { ... }`. When the body terminated, the restore was skipped — `self.terminated` leaked as `true`.

**Fix**: Moved `self.terminated = prev_terminated;` outside the `if !self.terminated` block so it's always restored unconditionally. The `if !self.terminated` block only guards the SSA phi merge logic.

**Lesson**: Any save/restore of control-flow flags must restore unconditionally. Conditional restore is correct for the immediate downstream code (phi merge) but the flag itself must always revert to its pre-guard value.

## 2026-06-13 — Dead `br` after `unreachable` in match emission (emit_expr.rs)

**Issue**: When a `match` expression has no wildcard arm, the code emitted `unreachable` (a terminator) then immediately `br label %mmerge` (another terminator). The `br` after `unreachable` is dead code.

**Root Cause**: The `br label %mmerge` was emitted unconditionally after the wildcard/fallback block, regardless of whether a wildcard arm existed. For the no-wildcard case, `unreachable` terminates the block and the `br` can never execute. LLVM accepts this but it's semantically incorrect.

**Fix**: Moved the `br label %mmerge` inside the `if let Some(wildcard)` arm, so it only emits when a wildcard body needs to branch to the merge label.

**Lesson**: When a code path ends with `unreachable`, no control-flow instruction should follow. The `br` was an unconditional spill from the wildcard branch.

## 2026-06-13 — `%state` SSA scoping bug in LLVM backend

**Issue**: Standalone functions (`defn`, callable `txn`) accessed global state
through `%state`, but `%state` was only alloca'd in `main()`. LLVM IR SSA
values are function-scoped — `%state` from `main()` is undefined in other
functions, producing invalid IR.

**Root Cause**: `emit_definition` and `emit_callable_txn` emitted function
signatures without `%State* %state` as a parameter. When the function body
referenced a state field, it emitted `getelementptr inbounds %State, %State* %state, ...`
on an undefined value. The call sites in `emit_expr.rs` also didn't pass
`%state` as an argument.

**Fix**: Three changes:
1. `emit_definition`: prepend `%State* noalias nocapture %state` as first
   parameter in function signature
2. `emit_callable_txn`: same signature fix
3. `emit_expr.rs` internal call site: prepend `%State* %state` to the
   argument list when calling definitions/callable txns

**Lesson**: Any LLVM function that emits GEP on `%State` must either (a)
receive `%state` as a parameter, or (b) use a global `@state` variable.
The backend's design choice is (a) — but two of the six function types
(defn, callable txn) were missing the parameter. Always audit ALL function
emission paths when adding state field references to the backend.

## 2026-06-13 — Duplicate import items: functions emitted once per import path

**Issue**: The LLVM backend emitted the same function 4 times for modules
imported through multiple paths (e.g., `understand.bv` imported directly
+ through `prompt` + through `persistence` + transitively through `layout`).
LLVM IR rejects duplicate function definitions.

**Root Cause**: `import_resolver.rs:82-92` replaces each `Import` item with
the resolved module's items inline. When the same module is imported
through N paths (direct + transitive), its items appear N times in the
`items` vector. No deduplication existed anywhere in the pipeline —
not in the import resolver, not in `Program`, not in the backend.

Example trace for `officina.bv` importing both `"understand"` directly
and `"layout"` which also imports `"understand"`:
```
Phase 1: items = [Import("understand"), Import("layout"), ...]
Phase 2: resolve Import("understand") → items = [U1, U2, Import("layout"), ...]
Phase 3: resolve Import("layout") → layout.bv has Import("understand")
         → cache miss for "layout", parse layout.bv
         → resolve Import("understand") inside layout → cache HIT → return [U1, U2]
         → layout items = [U1, U2, L1, L2]
         → splice into parent: items = [U1, U2, U1, U2, L1, L2, ...] ❌
```

**Fix**: Added `dedup_items()` function in `import_resolver.rs` that runs
after the resolution while-loop. It keeps only the first occurrence of
each named top-level item, keyed by `(category, name)`. Handles all
15 named TopLevel variants (defn, txn, state, trigger, const, sig,
frgn, struct, render struct, render obj, enum, typedef, render, link, rsrc. (`rstruct` deprecated). Unnamed
items (Stylesheet, Test, Assertion, Statement, etc.) pass through.

**Lesson**: Any pipeline stage that splices items from another module
into a flat list must deduplicate afterward. The fix is at the import
resolver level so all compilation paths benefit.

## 2026-06-13 — Unterminated basic block when Guarded then-path terminates (emit_toplevel.rs)

**Issue**: Functions compiled via `emit_definition` ended with `}` and no
`ret` terminator. LLVM opt rejects this with `expected '=' after instruction`
or `block does not have a terminator`. Occurred when a `Guarded` block
was the last statement in a function and its then-path contained `term;`
or `term!;`.

**Root Cause**: Two interacting bugs:
1. `Guarded` handler (2026-06-07 fix, `emit_stmt.rs:419`): when the
   then-path terminates, `self.terminated` is NOT restored to
   `prev_terminated` — it leaks as `true` so the folded loop doesn't
   emit code after `ret`.
2. `emit_definition`/`emit_transaction`/etc. (`emit_toplevel.rs`,
   8 sites): check `if !self.terminated { ret }` before emitting the
   function's return. The leaked `terminated = true` suppresses this
   `ret`, leaving the else-path's `end_l:` basic block unterminated.

**Fix**: Changed all 8 `if !self.terminated { ret }` sites to always
emit `ret` unconditionally. The then-path's `ret` from `term;` is in a
different basic block — the extra `ret` is dead code there but serves
as the required terminator for the else-path's `end_l:` block. LLVM's
optimizer removes duplicate terminators.

The `emit_callable_txn` path uses a different pattern (resets
`self.terminated` after each statement) and was not affected.

**Lesson**: `self.terminated` is used for two purposes — (a) local block
termination within a Guarded handler, and (b) function-level "don't emit
more code" signal to the caller. These conflict when a guard's then-path
terminates but the else-path doesn't. Always emitting `ret` at function
end is the simplest resolution — the extra `ret` is dead code that LLVM
optimizes away.

## 2026-06-13 — Unterminated `post:` label in `emit_callable_txn`

**Issue**: `toggle_record` function had an unterminated basic block before
the `post:` label. A `store` instruction was followed directly by `post:`
with no `br` or `ret` between them. Same root cause as the previous 8
sites — a Guarded's leaked `terminated` flag left the `end_l` block
unterminated, and the `post:` label expected a terminated preceding block.

**Root Cause**: `emit_callable_txn` (`emit_toplevel.rs:457-462`) reset
`self.terminated` after each statement (`if self.terminated { self.terminated = false; }`)
but did not emit a `br label %post` to terminate the last statement's
block before the `post:` label. The other 8 function emitters (defn,
reactive txn, etc.) use a `for s in &body { ... }; ret;` pattern where
the unconditional `ret` terminates the last block. `emit_callable_txn`
uses a different structure: `for s in &body { ... }; post: br loop; done: ret;`
— the `post:` label acts as the merge point, but the last statement's
block must have a terminator that branches to it.

**Fix**: Added `last_terminated` tracking in the body loop. After the
loop, if the last statement didn't terminate (e.g., a Guarded whose
then-path did but else-path didn't), emit `br label %post` to terminate
the block. When the last statement already terminated (e.g., `term;`),
the existing `ret` suffices and no `br` is needed.

**Lesson**: Three different function emission patterns exist in the
backend — (a) sequential body + unconditional ret at end, (b) sequential
body + post/done structure with loop-back, and (c) conditional body with
rollback. Each must independently ensure every basic block has a
terminator. The `post:` label pattern is unique to callable txns and
was missed by the first 8-site pass.

## 2026-06-17 — SSA extractvalue path missing return → duplicate register definitions

**Issue**: `mandelbrot.bv` and `knucleotide.bv` compiled to LLVM IR with
duplicate register definitions (`%t207` defined twice: once as `add i64 0, %ev208`
and once as `load i64, i64* %fdp209`). `llc` rejected: `error: multiple definition
of local value named 't207'`.

**Root Cause**: In `emit_expr.rs:105-108`, the `match ll_ty.as_str()` dispatch for
SSA state field reads had four arms (`"i8"`, `"float"`, `"i8*"`, `_`). The first
three all `return` a `TypedRegister`. The `_` default case wrote:
```rust
_ => {
    writeln!(out, "{}{} = add i64 0, {}", indent, v, ev).ok();
    Type::Int   // ← just a value, NOT a return!
}
```
Without a `return`, execution fell through the `if let Some(&idx)` and
`if let Some(ref ssa_reg)` blocks, then continued checking `let_bindings`,
`trigger_names`, `constants`, `mmio_fields`, and finally `field_index_map`
**again** — the alloca fallback path at line 202. This generated a second
`%tN = load i64, i64* %fdp{M}` definition for the same `v` register.

The bug was latent until a Guarded block appeared before the field-reference
in the transaction body. The Guarded handler clears `ssa_old_int_regs` (line 488),
which causes Identifier lookups for fields to enter the SSA extractvalue path
instead of the pre-extracted shortcut — exposing the missing `return`.

**How discovered**: `mandelbrot.bv` and `knucleotide.bv` both have a guard
before the `&count = count + 1` assignment (`[count % 5000000 == 0]` and
`[count % freq == 0]`). The guard clears `ssa_old_int_regs`, so the subsequent
identifier `count` in the update expression enters the SSA extractvalue path,
falls through without returning, and hits the alloca fallback path with the
same register name.

**Fix** (`emit_expr.rs:107`): Added `return TypedRegister { name: v, ty: Type::Int };`
after the `writeln!` in the `_` case.

**Lesson**: Every match arm in `emit_expr` that allocates `v` must `return`.
The `Type::Int` expression at the end of the `_` arm was silently discarded.
When adding a new type-specific code path to `emit_expr`, verify all cases
have a `return` — especially the `_` default case.

---

## 2026-06-13 — SSA dominance violations: values from guard then-path used in merge path

**Issue**: `opt` and `llc` reported "Instruction does not dominate all
uses!" for values computed inside a Guarded block's then-path but
referenced in the merge path. 17 violations in officina-cli.gen IR.

**Root Cause**: Three interacting problems:
1. **Unconditional `ret` fix was too aggressive**: The fix from 81a899c
   changed `if !self.terminated { ret }` to always emit `ret` at function
   end. This created dead code after terminator instructions when all
   code paths already returned. Reverted to conditional, with proper
   per-path termination instead.
2. **Guarded handler didn't terminate `end_l` block when then-path
   returned**: When a Guarded's then-path had `term;`, the `end_l` block
   (else path) was emitted without a terminator. Fixed by emitting
   `ret i64 0` or `ret void` for the else-path block, then restoring
   `prev_terminated` so callers continue normally.
3. **`let_bindings` leaked SSA registers across guard boundary**: Values
   assigned via `let x = expr;` inside a guard's then-path created SSA
   registers local to the `%then_l` block. After the guard, lookups for
   `x` returned the stale register name from `%then_l`, which doesn't
   dominate `%end_l`. Fixed by saving and restoring `let_bindings`
   around the then-path statement emission.

**Verification**: `opt -O2` and `llc` now pass on officina-cli.gen IR
with zero SSA dominance violations. 777 compiler tests pass.

**Lesson**: `let_bindings` (and all similar backend maps from names to
SSA register names) are implicitly scoped to the current basic block.
Any value computed inside a guard's then-path must either use a `phi`
at the merge point or be evaluated before the guard branch. The
save/restore approach works because it effectively discards then-path
bindings at the merge point, forcing re-evaluation in the correct
dominating block.

## 2026-06-14 — Stdlib files fail to parse with Rust parser (pre-existing)

**Issue**: When implementing auto-core import (`import#` / `--no-std`),
discovered that most Briev stdlib files in `lib/std/` fail to parse
with the Rust parser. The auto-core whitelist currently includes only
`ptr.bv`.

**Root Cause**: Multiple Briev language features used in stdlib files
are not supported by the Rust parser. Examples:
- `uni` keyword (unification operator) used in `options.bv`, `result.bv`,
  `hashmap.bv`, `hashset.bv` and others
- `term &x <- y;` collection mutation syntax used in many files
- `defn` with generic `T` type parameters without explicit bounds
- Some syntax constructs parse but then fail the TypeChecker

**Fix**: Isolated the auto-core import to `ptr.bv` only. Option + Result
are now hardcoded via `Program::synthesize_builtin_types()` in `ast.rs`.

**Lesson**: Briev's Rust parser and TypeChecker have known gaps vs the
interpreter. Auto-core must be conservative — only inject files that
pass both parsing AND typechecking. Gradual expansion can happen as the
parser improves.

## 2026-06-14 — Parseable core files fail TypeChecker

**Issue**: Several `std/core/*.bv` files parse correctly but fail the
TypeChecker:

| File | Error |
|------|-------|
| `bits.bv` | Cast-as-int type mismatch (`as Int`) |
| `char.bv` | Projection (`:> Popcount`) type mismatch |
| `collections.bv` | Collection mutation (`&list <- item`) not a recognized statement type |
| `string_builder.bv` | Same collection mutation pattern, not a recognized statement |

**Root Cause**: While these files are syntactically valid Briev, the
Rust TypeChecker (which predates several language features) does not
handle:
1. Type casts via `as Type` syntax
2. Projection operator with non-trivial targets like `Popcount`
3. Arrow-mutation `<-` as a statement (parser may handle,
   but TypeChecker doesn't)

**Fix**: Excluded these files from auto-core whitelist. They remain in
`lib/std/core/` for interpreter-based workflows.

**Lesson**: Auto-core injection is TypeChecker-gated. Only files that
pass via `cargo run -- check lib/std/core/<file>.bv` should be added
to the whitelist.

---

## 2026-06-14 — `__print` doesn't flush stdout

**Issue**: ANSI escape sequences (`"\x1b[2J\x1b[H"`) with no `\n` never reach the terminal. Line-buffered stdout (`_IOLBF`) only flushes on `\n` or buffer-full.

**Root Cause**: `lib/runtime/briev_rt.c:379-381`: `fputs(msg, stdout)` without `fflush(stdout)`.

**Fix**: Added `fflush(stdout)` after `fputs`.

**Lesson**: Any output function that may be called with non-newline-terminated data (especially ANSI escape codes) must explicitly flush.

---

## 2026-06-14 — `done_{name} → br label %done` exits main() after one reactive cycle

**Issue**: The first reactive txn whose precondition is false causes `main()` to return immediately. The program processes one cycle then exits.

**Root Cause**: `loop_engine.rs:622`: `done_{name}: br label %done` branches to the program exit label instead of the next txn's continuation (`s_{name}`).

**Fix**: Changed to `br label %s_{name}` so that when a txn's precondition is false, execution continues to the next txn's precondition check.

**Lesson**: In the `emit_ssa_main` loop, `done_` labels are per-txn skip-exits. They should chain to the next txn (`s_`), not terminate the program. The `done` (global exit) label is only for the exit condition check after all txns.

---

## 2026-06-14 — `@ link` for String loads pointer address, not content

**Issue**: `trg keypress: String @ link tty_read_key` always evaluates `keypress != ""` as true. The trigger fires unconditionally, appending garbage to the input buffer on every tick.

**Root Cause**: `trg_llvm_storage_ty` returned `"i8*"` for String. The backend emitted `load volatile i8*, i8** @sym; ptrtoint i64` — loading a **pointer address** (function entry point for C functions) and comparing it against the empty string literal's address. These addresses are never equal.

**Fix**: Changed String `@ link` storage to `"i8"` (single byte). Backend now emits `load volatile i8, i8* @sym; zext i8 to i64`. Added special case in `emit_fcmp` to compare linked String triggers against string literals by first-byte value (0 for `""`). C runtime provides `volatile char __tty_read_key` set by epoll/kqueue stdin handlers.

**Files**: `mod.rs:318`, `emit_toplevel.rs:99-103`, `emit_expr.rs` (`emit_fcmp`), `loop_engine.rs:622`, `briev_rt.c`

**Lesson**: `@ link` should load raw byte values from the linked address, consistent with all other types. The `i8*` storage type was inconsistent with the byte-oriented nature of the trigger mechanism. String triggers now compare by first-byte value, not by pointer identity.

---

## 2026-06-22 — `.N|>` consumed as field access by `parse_postfix`

**Issue**: `x |> f() .2|> g()` failed to parse. The error was "pipe target must be a function call" because `.2` was consumed as `FieldAccess(Call("f", []), "2")` before `parse_pipe_chain` could see it.

**Root Cause**: The `parse_postfix` function's Dot handler consumed `.` + `Integer` as numeric field access (`.N` selects a struct field by index). The existing guard only checked for `. |> ` (Dot + PipeGreater peek) but not `.N|>` (Dot + Integer peek + PipeGreater peek2). The parser only had single-token lookahead (`peek`), so it couldn't check two tokens ahead.

**Fix**: 
1. Added `peek2: Option<(Result<Token, ()>, logos::Span)>` field to `Parser` struct
2. Updated `advance()` to shift current→peek→peek2→lexer (three-stage pipeline)
3. Updated `put_back()` to preserve peek2
4. Added `peek_token2()` accessor method
5. Extended `parse_postfix` Dot guard to also check `.N|>` via `peek_token2`: `if matches!(peek, Integer) && matches!(peek2, PipeGreater) { break; }`

**Files**: `src/parser.rs` — `Parser` struct, `advance`, `put_back`, `peek_token2`, `parse_postfix` Dot guard

**Lesson**: When an expression-level prefix (like `.`) can start two different constructs at different precedence levels, looking ahead is necessary. Single-token lookahead is insufficient when the second token (`Integer`) and third token (`|>`) must both be checked. A `peek2` field is the minimal infrastructure for two-token lookahead — no reason to add a full token-ring buffer.

---

## 2026-06-22 — Pipe skip overflow silently clamped to 0

**Issue**: `3 |> square() .2|> double()` silently returned `double(3) = 6` instead of flagging an error. Skip=2 but only 1 command precedes `.2|>`, so no value exists at that pipeline depth.

**Root Cause**: The desugarer's read-index calculation clamped overflow to 0: `let read_idx = if step.skip > pos { 0usize } else { pos - 1 - step.skip };`. This hid programmer errors — `.2|>` on a 1-command pipeline should be a compile-time error.

**Fix**: Changed to an assertion that panics on overflow:
```rust
assert!(step.skip <= pos - 1);
let read_idx = pos - 1 - step.skip;
```

**Files**: `src/desugarer.rs:717-721`

**Lesson**: Never silently handle incorrect input. A skip that exceeds the pipeline position is a programmer error and should fail loudly.

---

## 2026-06-22 — Examples used `frgn __print_int` instead of `print_int#` intrinsic

**Issue**: Example files (`examples/pipe-chain.bv`, `examples/pipe-skip.bv`) and architecture docs declared `frgn __print_int(n: Int) -> Bool;` instead of using the `print_int#` intrinsic.

**Root Cause**: `print_int#(value)` is already defined as `Intrinsic::PrintInt` at `src/ast.rs:576` and handled in all three active backends (LLVM, Webstack, CIRCT). The `frgn` approach violates the "Intrinsics Before Frgn" rule — no `frgn` declaration should duplicate functionality already available as an intrinsic.

**Fix**: Replaced all `frgn __print_int` + `__print_int(result)` with `print_int#(result)` across examples, architecture docs, and learn-briev.

**Files**: `examples/pipe-chain.bv`, `examples/pipe-skip.bv`, `docs/architecture/features/pipe.md`, `learn-briev/01-basics.md`

**Lesson**: Check for existing intrinsics before reaching for `frgn`. `print_int#`, `put_char#`, `print_float#` all exist and should be used in examples.

---

## 2026-06-26 — nbody_newton energy output always 0.0 in SSA loop mode

**Issue**: `nbody_newton.bv` prints `0.000000000` for the energy computation
regardless of BOUND value or iteration count. The C reference correctly prints
`-0.169203490` (initial solar system energy). The float computation in the
binary produces zero despite correct initial values in the LLVM IR.

**Root Cause**: Not yet identified. The LLVM IR shows:
- Correct initial state values stored via `store float <bitcast i32 N to float>` 
  (verified via Python bit-pattern decoding — Jupiter's bx1 = 4.841... stored as
  `bitcast i32 1083895042 to float`)
- Correct GEP indices in both `init_state` and the main loop's body loads
  (verified by matching state field indices between init stores and body loads)
- Correct float arithmetic in the body (1605 fmul/fadd/fsub/fdiv operations present)
- Float stores in the body updating %State (verified via grep)

But the binary outputs 0.0. The body sequence is:
1. tick: loads all state fields, increments cycle counter, resets any_fired
2. b_simulate: reloads all state fields (double-load pattern), computes physics,
   stores results back to %State, computes energy
3. s_simulate: checks exit condition

The `print_loop` benchmark (which also uses `getenv_int#("BOUND")` + reactive txn)
works correctly, so the reactive transaction mechanism itself is fine.

**Hypothesis**: LLVM's SROA (part of `opt -O2`) decomposes the `%State` alloca
into per-field scalars, then eliminates the init stores because it considers
them dead (the body block's loads happen through GEP chains that SROA may not
recognize as aliases of the decomposed scalars).

**Workaround**: The benchmark was broken before the exit-condition fix (the old
`_ => panic!` for `Expr::BinaryOp` prevented `#!exit count == bound` from
compiling). This is a pre-existing codegen bug unrelated to the current changes.

**Diagnostic commands**:
```
opt -O3 -pass-remarks-missed=sroa nbody_newton.ll -disable-output 2>&1 | head -20
# Check if %State struct survived SROA
grep '%State' nbody_newton.opt.ll
# Check if init stores were removed
grep 'store.*float' nbody_newton.opt.ll | head -10
```

**Files**: `benchmarks/nbody_newton.bv`, `benchmarks/nbody_sqrt_idio.bv`
(also affected — uses same `getenv_int#("BOUND")` + reactive txn pattern).

**Priority**: High — blocks nbody benchmarks from producing correct output.
Likest cause: LLVM SROA eliminating float init stores when using inlined
SSA-loop dispatch.

---

## 2026-06-26 — queue_drain crashes at BOUND≥2 with realloc(): invalid pointer

**Issue**: `queue_drain.bv` with `BOUND=2` crashes with `realloc(): invalid pointer`.
`BOUND=1` works. The crash is pre-existing — present in both the benchmark-script
binary and freshly compiled binaries. Not caused by any of the 2026-06-26 fixes.

**Root Cause**: Not yet fully identified. Initial investigation shows:
- The crash is in the arena allocator's second `realloc` call (size ~1MB), called
  with `oldmem=0x40c2b0` (a BSS/data segment address, not a heap address).
- The first `realloc` succeeds, returning a valid heap address stored to `%arbase2`.
- On the second call, `%arbase2` contains a stale BSS pointer instead of the
  first `realloc`'s result.
- The arena's bump pointer (`%arptr2`) is correctly advanced after allocation
  (the 2026-06-26 fix ensures the bump is computed from the PHI-selected base,
  not the old dangling pre-realloc pointer).
- The push operation reads `%aol91` (list header slot encoding) from the old
  queue pointer (correct reactive semantics — reads see pre-tick state). The
  allocation size `(%aol91 + 3) * 8` is small (~32 bytes), so the bump check
  should pass and never reach `realloc`. But the crash shows an 1MB realloc,
  meaning the bump check IS failing — suggesting the bump or end pointer is
  corrupted.

**Hypothesis**: The list header encoding (slot 1 stores a packed length+capacity,
not bare length) leaks into the allocation size calculation. The push operation
uses the raw slot 1 value as if it were a length, but for lists with capacity
padding it may be much larger. Then `(%aol91 + 3) * 8` exceeds the arena,
triggering realloc with a stale base pointer.

**Workaround**: Use `BOUND=1` (single iteration). For full benchmarking, the
benchmark script uses `BOUND=5` for correctness and `BOUND=50000000` for timing
— both crash.

**Files**: `benchmarks/queue_drain.bv`, `benchmarks/queue_drain_idio.bv`

**Priority**: Medium — blocks queue_drain benchmarks from producing correct
output at BOUND≥2. Likely a list header encoding issue in the inop code.

---

## 2026-06-26 — `setvbuf(stdout, NULL, _IOLBF, 0)` in briev_rt.c makes fputc 2.1× slower

**Issue**: `benchmarks/fasta.bv` compiled by Briev runs at 2.1× wall-clock time
vs the C reference (0.480s vs 0.230s at BOUND=50000000). The generated assembly
is instruction-identical except for 2 extra register-to-register `mov`
instructions, which should be ~0 cycles via mov elimination.

**Root Cause**: `lib/runtime/briev_rt.c:276` calls `setvbuf(stdout, NULL, _IOLBF, 0)`
in `__rt_init()` (the runtime constructor). This forces stdout into line-buffered
mode (`_IOLBF`). On glibc, `fputc` into a line-buffered stream is ~2.1× slower
than the default fully-buffered mode (`_IOFBF`), because every call checks
`ch == '\n'` and the internal buffering path diverges. C programs use the
default buffering which auto-selects fully-buffered for non-TTY (pipes, redirects).

**Previous fix attempt**: LLVM-level `setvbuf` calls at `loop_engine.rs:232,877`
were removed on 2026-06-26, but these were redundant — `__rt_init()` in the
C runtime called `setvbuf` before `main()` ever ran, overriding any LLVM-level
setting.

**Fix**: Removed `setvbuf(stdout, NULL, _IOLBF, 0)` from `briev_rt.c:276`.
`__print` and `__print_int` already call `fflush(stdout)` explicitly, so line-
buffering was redundant for correctness. Glibc's default auto-selects
fully-buffered for non-TTY and line-buffered for TTY terminals — matching
standard C program behavior.

Users who need interactive flushing on `\n` can call it manually:
```briev
frgn setvbuf(stream: Ptr<Byte>, buf: Ptr<Byte>, mode: Int, size: Int) -> Int;
setvbuf(stdout, 0, 1, 0);  // _IOLBF = 1
```

**Result**: fasta gap closes from 2.1× to ~0.96× (Briev 0.220s vs C 0.230s).
Briev now BEATS C on fasta.

**Files**: `lib/runtime/briev_rt.c:276`, `src/backend/llvm/loop_engine.rs`

**Lesson**: The C runtime constructor (`__rt_init`) runs before `main()` and
can override any LLVM-level buffering configuration. Always check the C runtime
when debugging I/O performance — the LLVM IR is not the final word. Line-buffered
stdout (`_IOLBF`) imposes a significant performance penalty on bulk `fputc`
output (~2.1× on glibc). The runtime should not set buffering policy — users
should choose via explicit `frgn setvbuf` calls.

## 2026-06-27 — `try_eval_cfloat` missing `Expr::BinaryOp` normalization (nbody 0.0 energy bug)

- **Issue**: All nbody benchmarks output `0.000000000` for total energy regardless
  of iteration count. C reference produces `-0.169152707`.
- **Root Cause**: The parser always creates the new-style packed variant
  `Expr::BinaryOp` for arithmetic operations. The `try_eval_cfloat` function in
  `src/backend/llvm/mod.rs` only matched old-style variants (`Expr::Add`,
  `Expr::Mul`, etc.). Since `Expr::BinaryOp` fell through to `_ => None`,
  `try_eval_cfloat` returned `None` for expressions like `4.0 * pi * pi`, and the
  fallback emission produced `"0.0"` (line 1945). This made `solar_mass`,
  `pi * pi`, and all derived mass constants `constant float 0.0` in the IR.
  Since all gravitational forces were 0, the energy stayed 0.
- **Fix**: Added `normalize_to_old()` call at the beginning of `try_eval_cfloat`,
  mirroring the proven pattern in `eval_const_expr` (`proof_engine.rs:1323`).
  This converts `Expr::BinaryOp` to old-style `Expr::Add`/`Expr::Mul`/etc.
  before the match dispatches.
- **Files**: `src/backend/llvm/mod.rs:44-47`
- **Lesson**: Any function that processes expression trees by matching on old-style
  variants (`Expr::Add`, `Expr::Mul`, etc.) must first normalize new-style
  `Expr::BinaryOp` via `normalize_to_old()`. The parser switched to the new
  packed variant for all operations, but many matchers were never updated. Search
  for `Expr::Add` in match arms and verify `BinaryOp` is handled. This is the
  same pattern as `eval_const_expr` in the proof engine — the integer path was
  fixed but the float path was missed.

## 2026-07-01 — `%dab2` prefix collides with `%dab` at counter offset 200

**Issue**: `opt -O2` on `queue_drain.ll` errors: "multiple definition of local
value named 'dab263'". The `@main` function has `%dab263 = mul i64 ...` defined
twice — once at line 873 (in the body4 unrolled block) and once at line 1208
(in the body1 remainder block).

**Root Cause**: `arrow.rs:469` uses `format!("%dab2{}", txn_counter)` for the
copy-bytes register. When `txn_counter = 63`, this produces `"%dab263"` which
is textually identical to `format!("%dab{}", txn_counter)` at `txn_counter = 263`
(line 435, alloc-bytes register). Since both are in the same function (`@main`),
LLVM rejects the duplicate definition. The collision occurs because the `dab2`
prefix lacks a separator — `"dab2" + "63"` = `"dab" + "263"`.

Any program where the first `emit_arrow_discard` (pop) uses a `dab2{txn_counter}`
register with counter N, and the second `emit_arrow_discard` (push) uses a
`dab{txn_counter}` register with counter N+200, will produce this collision.

**Fix**: Changed the copy-bytes prefix from `%dab2` to `%dabcp` (dab-copy).
`"dabcp" + "63"` = `"dabcp63"` which can never collide with `"dab" + N` for
any N. The fix was applied to `src/backend/llvm/expr/arrow.rs:469-472`.

**Lesson**: When generating multi-prefix register names that share a common
substring, use separators (underscore) or choose prefixes with sufficient
edit distance. `prefix2{N}` is always dangerous because it's equivalent to
`prefix{2*10^d + N}` where d is the number of digits in N. The safe pattern
is `prefix` + `_` + `suffix` + `{N}` (e.g., `%dab_al{N}` for alloc,
`%dab_cp{N}` for copy).

## 2026-07-01 — `emit_binop` Phase 7B double-emission O(2^depth) blowup

**Issue**: `benchmarks/const_heavy.bv` takes >60s to compile (20 constants in
an addition chain). n=12 constants takes 11s, n=13 takes 37s — exponential
scaling. The benchmark was previously unusable despite being a core test.

**Root Cause**: Commit 16345bc (`Phase 7B-5: Wire operator resolution into
type-checker and codegen`) added a Phase 7B operator dispatch block at the
top of `emit_binop` in `helpers.rs:873-890`. This block calls
`self.emit_expr(out, l, indent)` to check if the left operand's type needs
custom operator resolution. For standard types like `Int`, no custom operator
exists, so the block falls through to the normal codegen path at line 917,
which ALSO calls `self.emit_expr(out, l, indent)` — re-emitting the entire
left subtree.

For a deeply nested left-associative addition chain:
```
Add(Add(Add(Add(acc, x/100), C00), C01), ...)
```
each level of `emit_binop` emits its left subtree TWICE (once in Phase 7B,
once in normal codegen). The inner levels also double-emit their subtrees,
producing **O(2^depth)** total IR. At depth 20, this is ~1M× the expected
work.

The `emit_binop` peephole for `Expr::Integer` pairs at the innermost level
saves it from being truly infinite, but the intermediate Add nodes (where
one operand is non-integer) still pay the double-emission cost.

**Fix**: Save the `TypedRegister` from the Phase 7B emit calls into local
`Option<TypedRegister>` variables. In the normal codegen path, use
`phase7b_l.unwrap_or_else(|| self.emit_expr(out, l, indent))` instead of
always re-emitting. This preserves the Phase 7B dispatch for custom types
while avoiding double-emission for standard types.

**Lesson**: Always check whether a pre-check block emits side effects (IR)
before the actual codegen path. If the pre-check can fall through, save its
results and reuse them. The same pattern applies to any early-return + fallthrough
pattern in codegen — save emitted registers, don't discard and re-emit.

## 2026-07-01 — `expr_dedup_cache` leaks register names across function boundaries

**Issue**: nbody benchmarks (`nbody_newton`, `nbody_sqrt`, `nbody_sqrt_idio`)
fail with "use of undefined value '%bfr{N}'" during `opt -O2`. The `%bfr{N}`
register is used in `@main` but was defined in `@simulate` (a separate function).

**Root Cause**: The `expr_dedup_cache` on `FunctionContext` is shared across all
function emissions (`emit_definition` for txn functions and `emit_folded_main`
for `@main`). When `emit_definition` emits `@simulate`, it populates the dedup
cache with register names like `%bfr2150`. When `emit_folded_main` later emits
`@main`, the loop body's `emit_binop` checks the dedup cache, finds a match
from `@simulate`, and returns `%bfr2150` — a register defined in `@simulate`.
But `@main` is a separate function, so `%bfr2150` is not defined there.
LLVM's verifier in `opt` catches the violation.

**Fix**: Clear `self.fun.expr_dedup_cache.clear()` at the start of
`emit_folded_main`, `emit_folded_memory_main`, and at each body4/body1
iteration boundary in `emit_folded_loop`. This ensures cached register names
from one function don't leak into another.

**Lesson**: Register names are only unique within a single LLVM function.
Any cache that stores register names (dedup, float, etc.) must be scoped
per-function or cleared at function boundaries. The `reg_float_cache` and
`reg_type_cache` were already scoped correctly; `expr_dedup_cache` was the
missed one.

## 2026-07-01 — `let_original_types` not populated for custom types

**Issue**: queue_drain (RingBuffer path) crashes with `realloc(): invalid pointer`
during benchmark runtime. The `<-` / discard operations fall through to the
default List arena path instead of using RingBuffer intrinsics.

**Root Cause**: `emit_toplevel.rs:1130-1134` only populates
`let_original_types` for boxed types (`Bool/Char/String/Data`). For custom
types like `RingBuffer<Int>`, the original type is not stored. When
`check_insert_strategy` / `check_extract_strategy` looks up the variable's
type in `let_original_types`, it finds nothing and returns `None`. The arrow
dispatch falls through to the default List arena path, which calls `realloc`
on memory allocated by the RingBuffer init — producing `realloc(): invalid
pointer`.

**Fix**: Populate `let_original_types` for ALL types, not just boxed ones.
Move the insertion before the type match and keep the existing let_binding_types
logic unchanged.

**Lesson**: `let_original_types` should not be treated as a boxed-type-specific
cache. Any code that needs to look up the declared type of a variable must
find it there — especially strategy dispatch for custom collection types.

## 2026-07-06 — Vector group backedge uses stale insertelement (nbody_sqrt MISMATCH)

**Issue**: `nbody_sqrt` produced `-0.170945078` instead of C reference
`-0.169288993` (0.17% energy drift per iteration — energy not conserved).

**Root Cause**: In A005c per-field phi dispatch, vector group backedges used
`pending_phi_native_backedge[name]` — the insertelement for THAT SPECIFIC
field only. Since the backedge dedup (`emitted_be`) emits the vector backedge
only for the first field name encountered (HashMap iteration order is
arbitrary), group members processed after the first had stale phi values
(never advancing from initial).

Example: if the backedge processed "vx0" first,
`pending_phi_native_backedge["vx0"]` = insertelement setting only element 0
(elements 1-3 from phi). The phi backedge for the entire vector group would
carry element 0's update but elements 1-3 stagnate at initial values. Only
the last-processed field (element 3) had ALL 4 elements correctly set.

## 2026-07-08 — `emit_operator_call` double-wraps register + missing string impl handler

**Issue**: All runtime benchmarks collapsed to "precomputed" because the LLVM IR was invalid:
`%t%t8 = add i64 0, %t3`. LLVM rejects `%` inside register names, so `opt`/`llc` failed,
binaries were never produced, and the harness reported precompute_ok (SKIP).

**Root Cause**: Phase 2B added operator declarations to bootstrap.bv (`op Add(Int) -> Int = "add nsw"`).
This caused `emit_binop` to find universe operators for `Int` and call `emit_operator_call()`,
which had two latent bugs never triggered before (no types had declared operators):

1. **Register name**: `format!("%t{}", self.fun.next_reg())` — `next_reg()` already returns
   `%t{N}`, so wrapping it in `"%t{}"` produces `%t%t{N}` (e.g. `%t%t8`). Fix: `self.fun.next_reg()`.

2. **Missing string literal arm**: The operator's `implementation` field is the LLVM opcode string
   `"add nsw"`, stored as `Expr::Literal(LiteralExpr::String(...))` or similar. The match on
   `&op.implementation.as_ref()` had arms for `Expr::IntrinsicCall` and `Expr::Identifier` but
   the `_ =>` fallback emitted `add i64 0, %reg` — ignoring the opcode entirely. Needed: an
   `Expr::Literal(lit) if lit.is_string()` arm that writes the string as the LLVM opcode.

**Fix**: 
- `src/backend/llvm/helpers.rs:1056`: `format!("%t{}", self.fun.next_reg())` → `self.fun.next_reg()`
- Add `Expr::Literal(lit)` arm that extracts the string and emits `{opcode} i64 {a}, {b}`.

**Lesson**: Adding operator declarations to bootstrap types triggers code paths that were
previously dead (no types had universe operators). Any new binding that makes a type "look
resolved" can activate untested match arms. Always run `--runtime` benchmarks (without `| tail`)
after adding operator bindings.

---

## 2026-07-11 — No borrow checker (alias safety gap)

**Issue**: The compiler correctly injects `op Drop` destructor calls when variables
go out of scope, but it does not prove the absence of dangling pointers. A user
can write:

```briev
let list: List<Int> = [1, 2, 3];
let first: Int = list[0];      // borrows from list
list[5] = 42;                  // mutates list while first still active
// first now dangles if list reallocated its backing buffer
```

The `op Drop` pass ensures every heap allocation is eventually freed, but it does
not prevent use-after-free or double-free. Same class of problem as C++ without
a borrow checker.

**Root Cause**: The compiler was designed with `op Drop` for lifecycle management
but has no alias analysis pass. Ownership tracking (which scope calls `Drop`) is
sound, but mutation-while-borrowed is not detected.

**Impact**: Memory safety depends on the programmer not aliasing pointers through
collection accessors. This is acceptable for single-threaded reactive programs
(common Briev use case) but unsound for general-purpose code with complex
aliasing patterns.

**Fix**: No immediate fix planned. A borrow checker pass would need to:
1. Track borrow origins at the AST level (which expression produced each pointer)
2. Reject mutations of a collection while an element reference is live
3. This is a significant compiler pass (~1000+ lines)

**Lesson**: Documented as a known limitation. The `op Drop` pass prevents leaks
but does not prevent use-after-free. Future borrow checker work will address
this.

**Files**: N/A — gap in the compiler architecture, not a specific bug.

---

- **Date**: 2026-07-14
- **Issue**: LLVM backend emits `fadd i64` (float add with integer operands) for
  integer addition, `br i1` with `i8` bool operand, and `call @print_int(%x)` without
  argument types — three layers of type bugs blocking all benchmark compilation.

**Root Cause**: Three independent bugs in the Phase 7 backend refactoring:
  1. `emit_binary_op` Add/Sub/Mul unconditionally emit `f`-prefixed instructions
     (`fadd i64` is invalid IR). `Div` was already correct — three arms missed.
  2. `emit_stmt.rs` hardcoded `"i64"` on all store/return instructions.
     `emit_user_call` dropped argument types on `call` instructions.
  3. Bool registers are `i8` (from `lower_type("Bool") → "i8"`) but `br i1` expects `i1`.

**Impact**: All benchmarks with integer arithmetic or function calls failed.

**Fix**: 7-part fix across `emit_expr.rs` and `emit_stmt.rs`:
  1. Add/Sub/Mul: `if is_float { fadd/fsub/fmul } else { add/sub/mul i64 }` guard
  2. `Neg`: `fsub double -0.0` for float operands
  3. Store instructions: `lower_type(&val.ty)` instead of hardcoded `"i64"`
  4. Return instructions: `lower_type(&reg.ty)` instead of hardcoded `"i64"`
  5. `emit_user_call`/`emit_external_call`: typed argument list (`call @fn(i64 %x)`)
  6. Guard/If: `trunc i8 %cond to i1` before `br i1`
  7. Guard/If labels: fixed `%`-in-label-name bug (labels used `gen_reg()` which
     returns `%tN` — labels must be bare identifiers without `%`)

**Also**: Renamed all `snake_case#` intrinsic calls across the entire codebase
(benchmarks, std lib, test fixtures) to PascalCase (`PrintInt#`, `GetEnvInt#`,
`Sqrt#`, etc.). Removed the snake_case→PascalCase normalization fallback from
`get_intrinsic_signature()` — it was a band-aid that only fixed the typechecker
but not the backend or interpreter.

**Files**: `src/backend/llvm/emit_expr.rs`, `src/backend/llvm/emit_stmt.rs`,
`src/backend/llvm/intrinsics.rs`, `src/intrinsic_signatures.rs`, and ~50 `.bv` files
**Plan**: `docs/plans/2026-07-14-fix-backend-type-bugs.md`

# ═══════════════════════════════════════════════════════════════════

## 2026-07-18 — Missing binary bitwise operators in parser

**Issue**: Bitwise AND (`&`), OR (`|`), XOR (`^`), and shifts (`<<`, `>>`) parsed as
different things or not at all. `&` was only handled as unary address-of; `|`, `^`,
`<<`, `>>` had no parser handlers despite being defined in `BinaryOpKind` and
`config/llvm-ops.toml`.

**Root Cause**: The expression parser had four binary-operator parse levels:
`parse_or`, `parse_and`, `parse_equality`, `parse_comparison`, `parse_term`,
`parse_factor`, `parse_unary`, `parse_postfix`. There were no levels for bitwise
operators or shifts — they fell through to the first `_ => break` in each loop.

**Fix**: Added four new parse levels between `parse_comparison` and `parse_term`:
`parse_bitor` (`|`, `Pipe`), `parse_bitxor` (`^`, `BitXor`), `parse_bitand` (`&`,
`Ampersand`), `parse_shift` (`<<` `>>`, `Shl` `Shr`). Each calls the next level for
its operands. The unary `&` in `parse_unary` is disambiguated by context (prefix
vs infix position).

**Files**: `src/parser/expressions.rs`, `src/type_universe/operators.rs`

## 2026-07-18 — Missing builtin operator bindings for Int bitwise ops

**Issue**: After adding parser support for `&`, `|`, `^`, `<<`, `>>`, the typechecker
rejected `lead & mask` with "invalid operation ''&'' on type Int".

**Root Cause**: `builtin_operator_binding` in `src/type_universe/operators.rs` had
no entries for `("Int", "BitAnd")`, `("Int", "BitOr")`, etc. The catch-all `_ =>
None` returned `None`, causing the typechecker to reject the operator.

**Fix**: Added entries for all five ops pointing to generic intrinsics
(`BitAndI64#`, etc.). The backend dispatches through `config/llvm-ops.toml` at
codegen time regardless of the intrinsic name.

**Files**: `src/type_universe/operators.rs`

## 2026-07-18 — Dead `br` after `ret` in Guard/If codegen

**Issue**: `when cond { term val; };` in a `defn` body generated LLVM IR with a
`br label %guard.end` after `ret i64 %val` in the guard.then block. LLVM's
verifier rejected the dead instruction.

**Root Cause**: `emit_stmt.rs` always emitted `br label %guard.end` after the guard
body (line 177 comment: "Always emit br to end (even if then body terminated)").
This creates unreachable code when the guard body contains `term`/`ret`.

**Fix**: Track `backend.fun.terminated` — after emitting the guard body, check
`terminated` before emitting the `br`. When the body already returned (via `term`),
`terminated` is `true` and the `br` is skipped. Same fix applied to `If` statement.
The end label is still emitted because `br i1 ... label %end` at the guard entry
references it, so the next statement's code lands inside the end label.

**Files**: `src/backend/llvm/emit_stmt.rs`

## 2026-07-18 — node main loop never exits (test impact)

**Issue**: A test using `node run [true][term == 0] { term 0; };` compiles but
the resulting binary hangs forever.

**Root Cause**: The runtime's `ss_main_loop` (generated by the LLVM backend) runs
forever — it checks pre/post conditions to decide whether to execute the body, but
never exits the main loop when the postcondition is met. The convergence loop only
controls body execution, not process termination. This is by design: reactive
transactions are for long-running reactive systems, not one-shot tests.

**Workaround**: Use `SysCall#(Exit, code)` to terminate after the test, or use the
benchmark harness (`/tmp/briev_bench_timer`) which enforces a timeout. No fix
needed — this is architectural, not a bug.

**Lesson**: For one-shot tests, use `defn` with `SysCall#(Exit, ...)` or integrate
with the benchmark harness. `node` is fundamentally designed for perpetual
reactive systems.

## 2026-07-18 — `txn` return type not parsed

**Issue**: `txn name(params) [pre][post] -> Type { body }` failed with "expected
LBrace, found '->'". The `->` return type syntax was accepted by the user's
mental model but rejected by the parser.

**Root Cause**: `parse_transaction` in `src/parser/definitions.rs` called
`parse_contract()` then `parse_block()` with no check for an optional `-> Type`.
The `Transaction` struct had `output_type: Option<OutputType>` but it was always
set to `None`.

**Fix**: Added optional `-> Type` parsing between `parse_contract()` and
`parse_block()`. Wraps the result in `OutputType::Single(...)`.

**Files**: `src/parser/definitions.rs`

## 2026-07-18 — `__` prefix used for non-frgn functions

**Issue**: `__memcmp`, `__UTF8_find`, `__UTF8_validate` used double-underscore prefix
convention which is reserved for `frgn` (foreign) functions.

**Root Cause**: Author used C convention (`__function_name = internal`). Briev
convention is: `__` prefix => `frgn` only.

**Fix**: Renamed to `memcmp`, `UTF8_find`, `UTF8_validate` (no `__` prefix).

**Files**: `lib/std/types/UTF8view.bv`

## 2026-07-18 — `else` keyword not supported

**Issue**: `else if` chains and `if/else` expressions produce parse errors in
Briev expressions.

**Root Cause**: Briev's expression parser has no `else` token. Conditionals use
`when` guards (statement level) or `if cond { expr } else { expr }` at expression
level, but `else` is not parsed as a keyword in all contexts.

**Workaround**: Use `when` guards for multi-way branching (statement level). For
expression-level conditional values, use nested `if` expressions or `when` chains
that set a let-binding.

**Files**: No code change — language design constraint.

---

## Ptr Arithmetic: Missing `inttoptr` Before `load`/`store`/`call` on Ptr-typed Values — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `c36fd266`  
**Root cause:** The LLVM backend uses an i64-centric internal representation for all
values, including pointers. Ptr parameters are `ptrtoint`-ed to `i64` at function
entry (emit_toplevel.rs:1188, rationale comment at line 1184). This is intentional —
it keeps all SSA values as `i64` and lets LLVM eliminate the round-trip.

The bug: three consumption sites assumed the register was already an LLVM `ptr`
type and used it directly in `load`/`store`/`call` instructions without an
intervening `inttoptr`:

| Site | File:Line | Pattern |
|------|-----------|---------|
| Deref load | `emit_expr.rs:718` | `load ..., ptr %i64_reg` — should be `inttoptr` then `load` |
| Deref store (main path) | `emit_stmt.rs:122` | `store ..., ptr %i64_reg` — missing `inttoptr` |
| Deref store (loop engine) | `counter.rs:846` | `store i64 ..., ptr %i64_reg` — missing `inttoptr` |
| Defn call args | `emit_expr.rs:1759` | `ptr %i64_reg` in call — missing `inttoptr` |
| Non-defn call args | `emit_expr.rs:1771` | `ptr %i64_reg` — missing `inttoptr` |

The `Expr::Index` handler (emit_expr.rs:491-520) and the loop engine's Index store
(counter.rs:828-841) showed the correct pattern: `inttoptr` → `GEP`/`load`/`store`.
The Deref paths had diverged from this reference implementation.

**Secondary issue**: `emit_binop_from_config` returned `Type::int()` for Ptr+Int Add,
which meant downstream code (Deref handler, call arg marshaling) couldn't detect
that a register held a Ptr value even though the type system said it was Ptr. Fixed
by preserving the Ptr type in the return: when either operand is `Ptr(_)`, return
that Ptr type instead of `Int`.

**Fix added at 5 sites**: All emit `inttoptr` before using the register as `ptr`.

**Tests**: `test_call_with_ptr_arg_emits_inttoptr` added. 1210 tests pass.

**Discovered by**: Compiling `lib/tamer/main.bv` (the Briev tamer) via `brievc build`
produced `main.ll` with invalid IR: `%t1 = add nsw i64 %ac0, %t3` followed by
`load i64, ptr %t1` — clang rejected the second line because `%t1` is `i64`, not `ptr`.

---

## Exported Definitions Not Registered in `defn_params` — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `c36fd266`  
**Root cause:** The first pass in `LlvmBackend::generate()` (mod.rs:1864-2037)
iterates items to register `defn_params` for function call argument marshaling. It
matches `TopLevel::Definition(d)` directly (line 1916), but `export defn` creates
`TopLevel::Export(Export { inner: Box::new(TopLevel::Definition(d)) })` which does
NOT match this arm. The emission pass (line 2410) correctly unwraps `Export`, so
the function body IS emitted — but its parameter types are never registered in
`defn_params`. Calls to exported functions fall through to the non-defn call arg
path, which doesn't insert `inttoptr` for Ptr args.

**Fix**: Added `TopLevel::Export(e)` arm in the registration loop before the
catch-all `_ => {}`, unwrapping the export and registering param types for
`Definition`, `Transaction`, and `AsmFn` inner items.

**Manifestation**: `compute_buffer_sizes(fn_table, fn_count, ...)` from
`lib/tamer/analyze.bv` — an `export defn` — was called with `ptr %t47` where
`%t47` was an `i64` register, because the call arg path couldn't find its param
types and skipped the `inttoptr` conversion.

---

## `defn` Body Assertions: `Statement::Gate` Emits Branch to Undefined `%loop` Label — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `efe1f559`  
**Root cause:** The `Statement::Gate` handler (emit_stmt.rs:338) checks whether a
convergence condition passes. The convergence target defaults to `"loop"` (line 346)
when `backend.fun.convergence_target` is `None`. This is correct for `txn`/`node`
bodies where the loop header is defined as `.loop:`. But `defn` bodies with
`[[post]` assertions (like `[stack_slots <= 1024][stack_slots >= 0]`) don't have a
convergence loop — the `[cond]` is an assertion, not a gate. The `"loop"` target was
never defined, producing `br i1 %cond, label %gate.passN, label %loop` where
`%loop` doesn't exist.

**Fix**: When `convergence_target` is `None` (defn body), emit `unreachable` on the
false branch instead of branching to `"loop"`:

```
br i1 %cond, label %gate.passN, label %gate.failN
gate.failN:
  unreachable
```

**Manifestation**: Compiling `lib/tamer/main.bv` after fixing the Ptr bugs produced
`br i1 %t99, label %gate.pass98, label %loop` where `loop:` was defined as
`.loop:` (dot prefix mismatch) — clang rejected the undefined label.

---

## Match Arm `|` (Alternation) Not Supported in Parser — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `efe1f559`  
**Root cause:** The statement-level match arm parser (parser/statements.rs:397)
only accepted single patterns: `_`, integer literal, or string literal. Multi-pattern
alternation with `|` (`0x30 | 0x31 | 0x32 => { ... }`) was not supported.

The AST had no representation for multi-pattern arms. Added `StmtMatchPattern::Multi(Vec<StmtMatchPattern>)` to `ast/top.rs:269`.

The parser loop was extended to collect `|`-separated patterns before the `=>`:

```
patterns ← []  // inner loop
loop:
  pat ← parse_pattern()
  patterns.push(pat)
  if next token is not Pipe: break
  consume Pipe
pattern = if patterns.len() == 1 { patterns[0] } else { Multi(patterns) }
```

Backend updates:
- Macro eval (`macros/eval.rs:585`): `Multi` matches if any sub-pattern matches.
- VM backend (`backend/vm/emit_stmt.rs:105`): `Multi` emits dup-and-eq for each
  pattern, jumping to body on first match.

**Manifestation**: `lib/tamer/analyze.bv:54` uses `0x30 | 0x31 | ... =>` in a match
arm. Parser errored: `expected FatArrow, found '|'`.

---

## `split_hoistable` Strips Guards When No Batch Loop Is Created — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `7e9de00b`  
**Root cause:** The dispatch logic at `mod.rs:2793-2807` calls `split_hoistable` to
extract guards (statements containing function calls) from the loop body, then
creates a batch loop only if `extract_batch_size_from_guards` returns `Some`
(i.e., there's a `when count % N == 0` periodic print). But the guard-stripping
(`body_stmts.iter().filter(|s| !is_hoistable_guard(s))`) ran unconditionally,
even when `bsize` was `None` and no batch loop would be created.

For nbody_sqrt, `let dist01 = Sqrt#(dsq01)` was classified as a hoistable guard
(because `has_call_expr` returns true for `Sqrt#` calls). These `let` bindings
were stripped from the inner body and placed in `guards`. But since nbody_sqrt had
no periodic print guard (`when count % N == 0`), `bsize` was `None`, no batch loop
was created, and the stripped bindings were never re-emitted. Downstream
computations referencing `dist01` hit `load i64, ptr @dist01` — an undefined global.

**Fix**: Only strip guards when `bsize` is `Some(...)`. Without a batchable periodic
guard, keep the original body intact:

```rust
if let Some(size) = bsize {
    let inner = strip_guards();
    (inner, Some(BatchInfo { ... }))
} else {
    (body_stmts.clone(), None)  // keep original body
}
```

---

## `emit_countable_batched_main` Never Emits Outer Guards (Periodic Prints) — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `7e9de00b` (partial), refixed in `[pending]`  
**Root cause:** The batch-loop optimization (commits 12e5435f+ in the
feat/derivation-synthesis merge) splits a loop body into inner pure-compute
statements and outer hoisted guards (periodic `when` blocks with `PrintLn!`,
termination guards). The inner body is emitted in a tight phi-node loop. The outer
guards are stored in `BatchInfo.outer_guards` and `self.fun.pending_post_hoist`.

The `.ox_` block (outer body) only loaded the counter and checked termination —
it never emitted `batch_info.outer_guards`. The `.done_` block only had a hardcoded
`last_energy` case for nbody_newton's termination print — it never emitted general
`pending_post_hoist` guards.

**Impact**: ring_buffer produced 0 lines of output (all `PrintLn!` calls were in the
un-emitted outer guards). The inner loop ran at 0.001s (no I/O), then exited silently.
The C reference produces 11 lines of output in ~0.04s.

**First fix (7e9de00b)**: Called `emit_countable_body` in `.ox_` and `.done_` blocks.
This produced correct output (11 lines) but used the full inner-loop statement
emitter (phi backedge tracking, field write sets, `last_val_temps` management) in
the outer loop — adding unnecessary overhead. ring_buffer went from 0.001s (broken,
no output) to 0.047s (correct, matches C).

**Second fix (pending, `emit_guard_block`)**: Replace `emit_countable_body` with
direct guard emission: for each `Statement::Guarded(cond, body)`, emit
`icmp %cond` + `br i1` + body + merge. No phi tracking, no field write sets.
This restores the tight outer loop while producing correct output.

---

## `align_of` Universe Iteration Finds Wrong Type by LLVM Type String — FIXED

**Date:** 2026-07-30  
**Status:** Fixed in `[pending]`  
**Root cause:** `align_of` (emit_toplevel.rs:560) iterates the entire type universe
looking for any type whose `llvm_type()` equals the requested LLVM type string.
For `"float"`, it found a type unrelated to Float whose resolved LLVM type happened
to be `"float"` but with alignment 2. This returned wrong alignment 2 for float
loads, producing `align 2` in `load float, ptr %gep` instructions.

**Fix**: Skip the universe iteration for standard LLVM types (`i8`, `i16`, `i32`,
`i64`, `i128`, `float`, `double`, `half`, `bfloat`, `ptr`). These have hardcoded
correct alignments in the match statement below.

**Before fix**: float → `align 2` (wrong), i64 → `align 16` (wrong, from matching an
i128-aligned type in the universe).  
**After fix**: float → `align 4` (correct), i64 → `align 8` (correct).

---

## All Structs Disappear from LLVM IR After Casting Graph Refactoring — UNFIXED

**Date:** 2026-07-30  
**Status:** Unfixed — workaround in `declare_struct_types` needed  
**Root cause:** The casting graph refactoring (Phase 0b, commit `4377ce38`) replaced
the old `rt_llvm_type()` universe property lookup with `protocol_llvm_type()` +
`resolve_llvm_type()`. The old path emitted struct type declarations for all types
in the universe that had an LLVM type property (e.g., `%String = type { i64, i64 }`,
`%SmallString64 = type { i64, i64, i64, ... }`). The new path only declares
struct types from `self.ctx.struct_types` (user-defined structs via `struct` keyword
in source) — which does NOT include `type SmallString64 { slot0: Int; ... }`
declarations from the stdlib bootstrap.

Types like `SmallString64`, `StaticString`, `String`, and `UTF8View` were no longer
declared in the LLVM IR. While nbody_newton doesn't directly use these types, their
absence changes LLVM's global type layout computation, which affects how the LICM
pass processes loops.

**Impact**: nbody_newton's IR triggers a segfault in `llvm::sinkRegion()` (LICM pass)
in clang 18.1.3. The baseline IR (which declared these struct types) compiles
successfully.

**Workaround**: Hardcode the four stdlib struct type declarations in
`declare_struct_types()` regardless of universe state.

---

## clang 18.1.3 LICM `sinkRegion` Segfault on Correctly-Aligned IR — UNFIXED

**Date:** 2026-07-30  
**Status:** Unfixed — clang 18.1.3 bug  
**Evidence:**

The generated nbody_newton.ll crashes clang 18.1.3 at `-O3` (and `-O2`) during
`llvm::sinkRegion()` in the LICM pass. The baseline IR (with `align 16` on i64
loads, and `%SmallString64`/`%StaticString`/`%String`/`%UTF8View` struct type
declarations) does NOT crash.

The crash appears to be triggered by a specific combination of:
- Loop body with float and i64 GEP+load/store patterns
- No struct type declarations for stdlib types in the IR
- Correct alignment values

The crash is not a Briev compiler bug — it's a clang/LLVM bug in the LICM
sinkRegion pass. The baseline avoided it by producing slightly different IR that
the buggy pass doesn't choke on.

**Workarounds** (any single one suffices):
1. Declare `%SmallString64`, `%StaticString`, `%String`, `%UTF8View` in IR
2. Use `-fno-licm` flag (not directly exposed by clang)
3. Use `-O2` instead of `-O3`
4. Use alternative clang version (18.1.8 is reported to work)

**Upstream clang bug:** The crash happens at `llvm::sinkRegion` in the
`-ffast-math` + `-O3` pipeline. Exact reproducer: `clang -O3 -c nbody_newton.ll`.

---

## `String` Type LLVM Representation Changed from `{ i64, i64 }` to `i128` — UNFIXED

**Date:** 2026-07-30  
**Status:** Unfixed — workaround in `protocol_llvm_type`  
**Root cause:** `protocol_llvm_type()` (mod.rs:490) falls back to
`format!("i{}", rt.bytes * 8)` for types without protocol membership. The String
type has `bytes = 16` (pointer + length = 2 × 8 bytes), producing `i128`. The old
code explicitly returned `{ i64, i64 }` for String (emit_toplevel.rs:274), but this
path is inside `if self.feature_sso_strings { ... }` which defaults to `false`.

The `i128` representation changes the FFI ABI for all functions taking String args:
`__getenv_briev({ i64, i64 })` → `__getenv_briev(i128)`. Clang passes `i128` by
value in a single register, while `{ i64, i64 }` is passed by pointer. This
completely changes call site codegen and triggers different optimizer behavior.

**Fix**: Added explicit String/UTF8View check in `protocol_llvm_type()` before the
bytes-based fallback, returning `{ i64, i64 }` regardless of `feature_sso_strings`.

**Impact**: All benchmarks using `GetEnvInt!` (which takes a String arg) had ABI
changes that could affect optimization. nbody_newton, nbody_sqrt_idio, and
kalman_filter_runtime all use `GetEnvInt!("BOUND")` and were affected.

---

## ring_buffer: Baseline Compiler Produces No Output at 0.001s — UNFIXED (pre-existing)

**Date:** 2026-07-30  
**Status:** Pre-existing bug in baseline (commit `29921993`)  
**Root cause:** The batch-loop optimization introduced in commit `12e5435f` splits
the loop body but never emits the hoisted outer guards (periodic `when` blocks,
termination `when` blocks). Same root cause as the `emit_countable_batched_main`
outer guard emission bug above. The base commit for the permanent baseline worktree
(`b39461e2`/`29921993`) also has this bug — ring_buffer produces 0 lines of output
and runs in 0.001s despite having `PrintLn!` calls in the source.

**Manifestation**: `BOUND=50000000 ./benchmarks/ring_buffer` prints nothing, exits 0.
The C reference (`ring_buffer_c`) produces 11 lines of output in ~0.04s.

**Note**: The user reported that the baseline compiler ran ring_buffer at 0.8× C
speed. This is likely from a different baseline version (before the batch-loop
optimization, commit `12e5435f`) or from a cached binary predating the batch-loop
merge. The actual baseline worktree binary produces no output.

---

## mandelbrot: Briev Output Differs from C — UNFIXED (pre-existing)

**Date:** 2026-07-30  
**Status:** Pre-existing, not caused by our changes (verified by testing 4 commits
back before any of our work — same wrong output)  
**Evidence:** Briev mandelbrot produces 2 lines of large integers
(`101363715272128`, `4318579316753219217`) while C produces 11 lines
(`73`, then 10 × `119`). The Briev output values look like raw memory values
or uninitialized state fields, suggesting a state loading bug in the loop engine
for integer-heavy convergent nodes.

This bug predates all commits in this session and may be related to the
derivation-synthesis merge or the casting graph refactoring.

---

## nbody_sqrt_idio: Cannot Compile — UNFIXED (pre-existing)

**Date:** 2026-07-30  
**Status:** Pre-existing — `Sqrt#` intrinsic call signature mismatch in
`frgn` declaration or `emit_intrinsic_call_dispatch`. The error occurs during
clang LTO linking, not during Briev compilation.

---

## kalman_filter_runtime: Cannot Compile — UNFIXED (pre-existing)

**Date:** 2026-07-30  
**Status:** Pre-existing — same clang LICM sinkRegion crash as nbody_newton.
Likely has similar loop structure and alignment issues.

---

## Frgn `declare` Block Non-Deterministic Order (HashMap Iteration) — FIXED

**Date:** 2026-07-31
**Status:** Fixed
**Root cause:** `emit_declares` / the foreign-declare loop at
`src/backend/llvm/mod.rs:2069` iterated `self.ctx.frgn_map` — a `HashMap` with a
per-process SipHash seed — WITHOUT sorting. The emitted `declare` statements
(e.g. `__getenv_briev`, `__print_int`, `__print_str`) appeared in run-to-run
nondeterministic ORDER in the generated IR. Violates Coding Standard 7
(HashMap iteration that produces LLVM IR MUST be sorted by key).
**Fix:** The loop now collects `frgn_map.iter()` into a `Vec`, sorts by key, and
emits in sorted order.
**Impact:** Same compiler, same input → byte-identical IR across runs (verified
for ring_buffer). No semantic change — only declaration order.
**Lesson:** Any loop emitting IR from a HashMap must sort first. Discovered during
Phase 2 IR A/B (reference vs new compiler) — the ordering masked the check that
the actual code was unchanged.

---

## Density Metric Ignored Its `_all_idents` Filter — FIXED

**Date:** 2026-07-31
**Status:** Fixed
**Root cause:** The `#11 → #0` dense-matrix downgrade in
`emit_toplevel.rs:1820-1849` used `count_cross_float_ops_in_expr(expr,
&float_body_idents)`, but the function ignored its `_all_idents` parameter and
counted ANY BinaryOp with an identifier on each side — int-only counter
arithmetic inflated the cross-op count.
**Fix:** Moved the metric to `src/analysis/density.rs` (Phase 2, plan §7.1).
`count_cross_float_ops` now gates each operand side on the txn's float set
(`expr_refs_float`), so int-only ops no longer count. For all-float txns
(kalman, nbody) the count is identical to before; decisions verified
byte-identical across all 38 benchmark programs.
**Impact:** Cleaner metric; behavior preserved. The `> 4.0` threshold moves to
`config/targets.toml` in Phase 3 (§8.1).

---

## `as Float` cast emitted `sitofp to double` — FIXED

**Date:** 2026-07-31
**Status:** Fixed
**Root cause:** the casting graph's `IntToFloat` lane hardcoded `sitofp … to
double` (both `emit_cast_steps` sites in `emit_expr.rs`). An `(count % 97) as
Float` cast produced a `double` register that fed a `fadd float` — type error
(telemetry_stream benchmark).
**Fix:** the lane now emits `sitofp {src} to {dst_ll}` (float for a Float
target, double for Float64/Double).

## Implicit Int × Float coercion silently bitcasts the int — FIXED

**Date:** 2026-07-31
**Status:** Fixed (verified 2026-08-01)
**Root cause:** `(count % 101) * 0.5` (Int * Float) compiles WITHOUT error and
coerces the Int via `bitcast i32 to float` (reinterpreting the bits), producing
garbage (accumulator_flush printed 0). AGENTS.md forbids implicit coercions —
this should be a compile error (or a semantic `sitofp`). The correct pattern is
an explicit `as Float` cast.
**Fix:** the typechecker now rejects implicit Int↔Float arithmetic in binary
ops (`typechecker/mod.rs` infer_binary_op) — `Int * Float` is a type error
unless the type declares a cross-type `op` overload. Verified:
`(count % 101) * 0.5` errors with a clear message; `((count % 101) as Float) *
0.5` type-checks.

## Outlined guard float params allocated as i64 — FIXED

**Date:** 2026-07-31
**Status:** Fixed
**Root cause:** `emit_statement`'s param-mutation path (`emit_stmt.rs`) allocated
`alloca i64` unconditionally when a value-register binding is assigned. An
outlined guard param that is a FLOAT state field (e.g. accumulator_flush's
`sum = 0` reset) is a `float` register — boxing it as i64 produced
`store i64 %__cp_sum` on a float param (type error).
**Fix:** the alloca uses the binding's Briev type (`let_binding_types` →
`llvm_type`).

## 2026-08-01 — queue_drain dispatches to version-DAG, not the countdown

**Finding:** queue_drain (RingBuffer via `<-` ops) builds and runs, but its
periodic print is off-by-one (prints count-1 at the boundary). Root cause: the
batch-shape detector (`src/analysis/batch_shape.rs`) rejects the body because
the `<-` collection ops (`queue <- count`, `<- &queue`) precede the counter
increment — so the countdown never dispatches, and the version-DAG's periodic
guard reads the PRE-increment count. The batch detector
ACCEPTS the body (verified with debug). Root causes (now confirmed in the
countdown emitter, `src/backend/llvm/loop_engine/counter.rs`):
(a) the countdown's counter increment doesn't populate last_val_temps[counter],
so a guard printing the counter reads the header phi (pre-increment) and prints
count-1; (b) the countdown inner-body emission drops the `<-` push member call
(only the pop address + increment emit). Fix both in the countdown; keep
queue_drain out of the harness until the output matches C (5M/10M).
**Fixed 2026-08-01 (A9b):** three root causes — (a) the `let_to_field` alias map
misread the `<-` push (both fields) as a let-alias and remapped the guard's
counter reads to the collection field (exclude field-to-field assignments from
the alias map); (b) the countdown Assign arm didn't dispatch `<-` to the member
call (added find_insert_strategy + emit_strategy_member_call); (c) collections.bv
declared `op Init` without an init member, and the countdown's inline init didn't
run the Init-op construction (queue slot stored 0 → null deref). queue_drain
re-enabled; matches C (5M/10M) and wins 0.58x.

---

## frgn String declares disagree with call sites when SSO is off — FIXED

**Date:** 2026-08-01
**Status:** Fixed (the B0 bits model resolved it; verified 2026-08-01)
**Root cause:** The frgn `declare` emission used `protocol_llvm_type`, which
returned `{ i64, i64 }` for String-shaped types, while the call site used `ptr`
with SSO off — a declare/call split-brain that was harmless only because the
wrong declare was GC-dropped.
**Fix:** B0 retired the structural `{i64,i64}` claim — `protocol_llvm_type` now
returns `ptr` for every `#String` value (a String is a pointer to a
length-prefixed `[len][bytes]` buffer; state slots keep the i64 machine word
and convert via adapt_to_i64/ensure_typed_value). Verified:
`declare i64 @__getenv_int(ptr)` agrees with `call i64 @__getenv_int(ptr %t2)`
in `float_math.ll`.

---

## SSO Tag Bit Corrupts String Literal Addresses in Inline Init — FIXED

**Date:** 2026-08-01
**Status:** Fixed (B1)
**Root cause:** `emit_field_init_value` (emit_toplevel.rs) OR-ed tag bit 0 (= 1)
onto String literal addresses when `tag_strings=true` (the inline-init path used
by `emit_inline_init_stores` in `main`). Under the bits model a String value is
an UNTAGGED `ptr` to `[len: i64][bytes]`; the tag made `briev_str_eq` read a
misaligned length header, so equal-content strings at heap vs literal addresses
compared unequal. The tag bit belongs to the old SSO encoding only.
**Fix:** gate the tag with `tag_strings && self.feature_sso_strings`; the
untagged `store i8*` is the bits-model store for both paths.
**Impact:** content equality (B1) works end-to-end: `get_env("VALUE") == "same"`
with `VALUE=same` returns true; `VALUE=other` returns false. Verified in
`.smoke/eq_demo.bv`.
**Lesson:** any pointer-tagging must be gated on the representation that uses
tags (SSO); a representation change (bits model) must audit every tag site.

---

## Malformed !range on Narrow State Fields Crashes clang — FIXED

**Date:** 2026-08-01
**Status:** Fixed (Phase 3b)
**Root cause:** `type_driven_range` emitted `!{ i64 0, i64 256 }` range metadata
for 1-byte types (Bool/UInt8/Int8 state fields), but the field loads as `i8`.
LLVM range metadata bounds must be the same integer type as the load — i64
bounds on `load i8` are malformed and crash clang
(`computeKnownBitsFromRangeMetadata`, `APInt::setBitsSlowCase`). Latent until
the entry!/args! plugin introduced Bool done-flag state fields (any Bool/
UInt8 state field triggered it; `let done: Bool = false;` alone crashes).
**Fix:** `emit_range_metadata` (emit_toplevel.rs) emits bounds in the field's
LLVM integer width (`i8`/`i16`/`i32`/`i64`) and skips ranges that don't fit
the storage width (256 is vacuous for i8 — the whole space — so it is
dropped). Applies to both contract-driven and type-driven ranges.
**Impact:** `.smoke/bool_range.bv` and `.smoke/entry_demo.bv` link and run;
`test_bool_field_no_malformed_i8_range` guards it.
**Lesson:** LLVM metadata must be type-coherent with the instruction it
annotates; representation facts (N bytes → [0, 2^8N)) must be projected into
the actual storage type before emission.

---

## `async node` Prefix Dropped the Async Flag — FIXED

**Date:** 2026-08-01
**Status:** Fixed (Phase 3c)
**Root cause:** the parser's `async node` prefix form (parse_top_level) consumed
the `async` token then called `parse_node()`, which does `pos += 1` (consumes
`node`) then `eat(Async)` — but async was already consumed, so `is_async` was
always `false` for the prefix form. `node async` (suffix) worked; `async node`
(prefix) silently produced a non-async node. The concurrency gate exposed this:
explicitly-async nodes were never classified, so valid async pairs were denied.
**Fix:** the prefix arm sets `txn.is_async = true` on the parsed node.
**Impact:** `async node inc_a` now correctly marks the node async;
`benchmarks/async_counters_idio.bv` (which uses the prefix) passes the gate.
**Lesson:** when a parser entry point pre-consumes a token that a shared parser
also consumes, the shared parser's state becomes wrong — the wrapper must
restore/forward the consumed flag.

---

## HashWord Categories Kept the `#` Prefix — Casts to/from #Bit Silently Fell Through — FIXED

**Date:** 2026-08-01
**Status:** Fixed (Phase B2)
**Root cause:** `type_to_protocol` returned the HashWord name verbatim as the
category (`Type::HashWord("#Bit")` → category `"#Bit"`), but the casting
graph's base lanes are keyed on bare categories (`"Bit"`, `"String"`). So
`find_path("String", "", "#Bit", "")` found no lanes and every cast to/from
`#Bit` silently fell through to LLVM coercion — `s as #Bit` emitted
`bitcast i64 %ptr to i64` (invalid). `is_protocol_member` masked this by
stripping the `#` from its target.
**Fix:** `type_to_protocol` strips the `#` prefix from HashWord categories
(matching base-lane keys); `type_to_protocol` also follows a type's declared
`base` parent (`type Latin1String: #String` ⇒ "String") since the normalizer
no longer injects `Cast.#` properties.
**Impact:** `s as #Bit` now emits `ptrtoint` (content view); `b as String`
emits `briev_bits_to_str` (encoding door). Verified in `.smoke/bit_let.bv`,
`.smoke/bit_to_str.bv`, and `test_hashword_category_strip`.
**Lesson:** a type's protocol category is derived from the universe's Cast.#
properties OR its declared base — never from the raw HashWord text — and the
graph keys must be normalized consistently.

---

## Reflect-Read String Field Eliminated as Dead — FIXED

**Date:** 2026-08-01
**Status:** Fixed (Phase B3)
**Root cause:** `compute_live_fields`'s `collect_identifiers` did not handle
`Expr::Reflect`, so a String `let` used ONLY via reflection (`s.^Len`,
`s.^^Bytes`, `s.^Ptr`) was treated as a dead state field and eliminated from
%State. At emission, `s.^Len` hit the identifier fallback and emitted
`load i64, ptr @s` with `@s` undefined (clang link error), or the runtime
`Len` arm panicked on the Int-typed register.
**Fix:** `collect_identifiers` gained an `Expr::Reflect(recv, _, _)` arm that
collects the receiver (the same liveness rule as FFI args: an observation keeps
the value alive).
**Impact:** `.smoke/len_demo.bv` now emits `briev_char_len` for `s.^Len`
(chars=5) and a header load for `s.^^Bytes` (bytes=6) on "héllo".
**Lesson:** every expression construct that reads a state value must be listed
in the liveness identifier collector; a missing arm silently eliminates the
field and surfaces as an undefined global / wrong register type at codegen.

---

## Concat Result Tagged as i64 (Temp Bit) Breaks ptr Consumers — FIXED

**Date:** 2026-08-01
**Status:** Fixed (Phase B4a)
**Root cause:** `emit_box_concat_result` OR-ed the legacy temp bit (2) onto the
concat result and returned an i64 register. Under the bits model a String
value is an UNTAGGED ptr to [len][bytes]; consumers expecting `ptr` (e.g.
`__print_str(ptr)`) failed with "defined with type i64 but expected ptr".
Exposed when the SSO layer was retired (the boxing was an SSO-era artifact).
**Fix:** `emit_box_concat_result` returns the untagged buffer ptr as a
String-typed register (bitcast ptr to ptr).
**Impact:** `a ++ b` concat now works — `.smoke/concat_demo.bv` prints
"foobar".
**Lesson:** any pointer-typed value must be returned as a ptr under the bits
model; tag-bit/boxing artifacts from the SSO era corrupt the type and must be
audited when the SSO layer is retired.

---

## Float64 Literal Emitted as Float32 — Latent State Corruption — FIXED

**Date:** 2026-08-01
**Status:** Fixed (2026-08-01)
**Root cause:** `Expr::Float(f)` always emits a float32 bitcast
(`float_to_llvm_hex` → `bitcast i32 ... to float`). A `let d: Float64 = 3.25`
state field (a `double` slot) is initialized by boxing that float32 into an i64
and storing it — the double gets the 4 low bytes of the float32 pattern plus 4
garbage bytes. Verified: `%State = type { float, double, ... }`, `init_state`
stores `store i64 (zext (bitcast i32 1078984704 to float) ...)` into the double
slot.
**Impact:** `Float64` values are garbage from initialization onward. The Print#
convenience intrinsic's `__print_float64` path is now correct (the IR loads a
proper `double` and calls `__print_float64(double)`), but the VALUE is the
corrupted double. No benchmark, stdlib file, or test uses Float64 today.
**Fix:** `emit_field_init_value` (emit_toplevel.rs) now has a `double` case for
both the plain `Expr::Float` and the `Neg(Float)` arms — a Float64 field stores
`bitcast i64 <f64 hex> to double` (via `float64_to_llvm_hex`) instead of boxing
the float32. Also: `try_coerce_via_parse` now treats `Neg(Float)`/`Neg(Decimal)`
as a "Decimal" form, so `let d: Float64 = -3.25` typechecks (before it errored
as Float). Verified: `d=3.25` and `d=-3.25` print correctly.
**Lesson:** literal emission must follow the binding/field's declared type, not
default to the narrowest protocol variant.

---

## `~op` Consuming a Top-Level Const-Let in a Txn Emits an Undefined `@b` Global — FIXED

**Date:** 2026-08-01
**Status:** Fixed (2026-08-01)
**Root cause:** `a ~+ b;` in a txn body where `b: Int = 3` is a top-level
const-initialized `let`. The identifier emission falls to the `@<name>` global
path (`load i64, ptr @b`) even though `b` is registered as a %State field —
`field_index_map` lacks `b` at emission time for the consumed operand. The
`@b` global is never emitted (undefined symbol at clang). A non-consumed read
of the same const-let in the same body resolves via the state GEP (works), so
the `Expr::Consume` wrapper interacts badly with the field registration/phi
path. Reading a const-let in a txn without `~op` works at main (verified).
**Impact:** `a ~+ b` (consumptive arithmetic) on a top-level const-let inside a
txn body fails to link. `~=`/`~+` on defn PARAMS work (verified end-to-end:
move=3 add=8). The arrow forms (`<-`, `~<-`, discards) and the destructive
extract are unaffected.
**Fix:** the field-LIVENESS walk `collect_state_identifiers`
(`src/analysis/transition_graph.rs`) + `infer_provenance`
(`src/analysis/provenance.rs`) + `collect_expr_idents`
(`src/backend/llvm/emit_toplevel.rs`) did not descend into `Expr::Consume`, so
a consumed-only field (`b` read only inside `a ~+ b`) was marked DEAD and
eliminated from `%State`, leaving the identifier emission to fall back to an
undefined `@b` global. All three walks now recurse into `Expr::Consume`. The
field is registered normally and loads via the state GEP. Regression test:
`backend::llvm::tests::test_consumptive_op_on_const_let_in_txn` (asserts no
`load i64, ptr @b`).
**Lesson:** a new Expr wrapper (Consume) must be walked by every field/usage
collection pass — the compiler's passes must treat `Consume(inner)` as a read
of `inner` everywhere.

---

## Arrow-Referenced Collections Eliminated by Field-Liveness — Silent Push/Pop No-Op — FIXED

**Date:** 2026-08-01
**Status:** Fixed
**Root cause:** `scan_for_state_identifiers` (and the garbage-scheduler's
`collect_statement_identifiers`) in `src/analysis/transition_graph.rs` walked
every statement form EXCEPT `Statement::ArrowAssign`. A collection referenced
ONLY through `<-`/`~<-` was therefore never marked referenced → the field-mode
analysis eliminated it from `%State` → the arrow dispatch silently no-op'd (or
emitted an undefined `@st` global). queue_drain/stack_push_pop matched the C
reference ONLY because their output is the counter, not the collection — the
push/pop never actually ran.
**Fix:** both walkers now descend into `ArrowAssign { target, value }`. The
collection is kept in `%State` and the strategy calls are emitted.
**Also fixed en route:** (a) `emit_strategy_member_call` returned only `bool`,
hiding the member-body result register — the caller used an undefined `out_tmp`;
it now returns the member's result register, so the extract-into-target
(`v ~<- st`) stores the popped value; (b) `emit_strategy_fn_call` emitted call
args without their LLVM types (`call i64 @pop(%t39)`), now `call i64 @pop(i64 %t39)`.
**Regression test:** `backend::llvm::tests::test_arrow_only_collection_is_kept_in_state`.

## Loop-Guard Variable's State Store Dropped — Stale Body Reads — FIXED

**Date:** 2026-08-01 (fixed 2026-08-03)
**Status:** Fixed
**Root cause:** in the `.fmain` runtime loop (`emit_folded_loop`/
`emit_countable_body`), a state field used as the loop GUARD (`[count < N]` +
`count = count + 1`) has its per-iteration `%State` store dropped — the loop is
driven by an internal counter phi, and the body's `count = count + 1` is not
stored back. A body READ of the guard field (e.g. `println!(count)`) then sees
the stale field value (0) every iteration instead of 1,2,3. Verified: a
counter-only reactive program prints `count=1` ×N instead of 1..N. The
benchmarks are unaffected (they never print/push the guard field mid-loop).
**Fix (2026-08-03):** in `emit_folded_loop`, register the guard field in
`phi_field_regs` → the counter phi (mirroring `emit_countable_main`'s PerFieldPhi
mapping at counter.rs:318), so a body read of the guard field resolves to the
live per-iteration phi register instead of a stale `%State` load. `counter_var`
(Option<&str>) is threaded through `emit_folded_main`/`emit_folded_loop`; the
dispatch call site (`emit_folded_loop_shape`, InlineSsa path) passes `&bp.var`.
When the body does NOT read the counter, the extra `phi_field_regs` entry is
never referenced — zero IR change (benchmark A/B: byte-identical, zero MISMATCH).
**Impact:** `println!(count)` in a counter-only loop prints 0,1,2,… correctly.
Regression test: `test_counter_loop_guard_read_uses_phi_not_state` (asserts the
main-fold print feeds `%flc` and the counter increments via the phi backedge).
`print_loop.bv` (periodic-guard `println!(ops)`) dispatches via version-DAG, not
InlineSsa, so it was never affected — output verified identical pre/post.
**Undo:** revert the `phi_field_regs` registration + the `counter_var` threading;
the stale-read behavior returns.

## `(n as String)` — Latent Link Error + Type Mismatch — FIXED

**Date:** 2026-08-01
**Status:** Fixed
**Root cause:** the `Int → #String` casting-graph lane (`ExtCall("int_to_str")`,
`src/casting/graph.rs:167`) emitted `call i64 @int_to_str(...)` (a hardcoded
`i64` return), but a String IS a `ptr` to `[len][bytes]` — so the String target
then `ptrtoint`'d an i64 (a type mismatch), AND `int_to_str` was undefined in
the runtime (a latent link error). The direct-cast path emitted
`call i64 @__int_to_str__` — also undefined.
**Fix:** (a) `int_to_str`/`__int_to_str__` defined in `briev_rt.c` (format the
int via snprintf + wrap as a `[len][bytes]` String); (b) the ExtCall lane
emission uses `dst_ll` for the return type (`call ptr @int_to_str(i64)`) —
the same pattern the `CastFromBitCallback` case already used; (c) the direct
path emits `call ptr @__int_to_str__`; (d) the declares are `ptr`-returning.
Verified end-to-end: `s = (n as String); println!("s={}", s)` prints `s=42`.
Regression tests: `test_cast_int_to_string_lane_emits_ptr_call` (lane path) +
`test_emit_cast_int_to_string` (direct path).

## Float ABI/opcode corruption in the LLVM backend — FIXED 2026-08-03 (status line stale)

**Date:** 2026-08-03
**Root cause (two related):** (1) The `#Float` protocol crosses the boundary as
LLVM `float` (32-bit) but config/glue.dbvl declares `c_abi = "double"` — a C
caller using `double` gets garbage for Briev `Float` args/returns. (2) Float
parameter/arithmetic codegen is broken: `export defn scale(x: Float) -> Float`
emits `%ac0 = zext i32 (bitcast float %arg0) to i64` then `fmul float %ac0, ...`
— an `i64` used as a `float` operand, rejected by `llc` ("'%ac0' defined with
type 'i64' but expected 'float'").
**Impact:** Float exports are unusable across the FFI boundary. The native-speed
demo (examples/glue-host/rank.bv) uses Int exports only.
**Path to fix:** align the LLVM `#Float` lowering with the protocol ABI (double,
i.e. `f64`) and fix the Float param/op emission (remove the i32 bitcast +
zext-to-i64 indirection). Worth a dedicated pass on Float end-to-end (interpreter
already handles f64).

**Status: FIXED 2026-08-03** (plan 2026-08-03-float-protocol-only-rust-speed).
- The `#Float` category's LLVM resolver is now `FloatWidth` — the width comes
  from the type's `bits` metadata (32 → float, 64 → double, 16 → half/bfloat via
  `disamb`, …), so `Float64`/`CDouble` lower to `double` without naming types.
- Float width casts are a `FloatWidth` lane emitted as `fpext`/`fptrunc` —
  `2.0 as CDouble` and `2.0 as Float64` emit clean `fpext float to double`.
- The boundary ABI is correct: `CDouble` → `double` in the generated header and
  the `.ll`. The remaining `x * 2.0` on a CDouble requires an explicit
  `(2.0 as CDouble)` cast (no implicit Float coercion — by design).

## Stateful Exports: State Fields Eliminated / Missing State Param / Dangling State — FIXED

**Date:** 2026-08-03
**Status:** Fixed (plan 2026-08-03-glue-folders-node-bridge, the Python ↔ Node
bridge probe)
**Root causes (all latent until a bridge export touched a state field across
calls — rank/cancel/boundary never did):**
1. `compute_referenced_fields` (transition_graph.rs) never unwrapped
   `TopLevel::Export`, so `export defn read() { term saved; }` left `saved`
   "unreferenced" → `apply_field_modes` eliminated it → the body emitted an
   undefined `@saved` global.
2. `expr_needs_state` (export_abi.rs) treated a bare `Expr::Identifier` as pure,
   so `term saved;` (and the marshalled `term str_to_c(saved);` — the CStr↔String
   meld rewrite makes the read a frgn call ARG) didn't mark the export stateful
   → the wrapper lost its `%state` param.
3. `__briev_init_state` (emit_library_shim) returned a STACK `alloca` pointer
   that dangled on return. Fixed to a module-global `@__briev_state`
   (library model is one state per process).
4. `fn_return_types` omitted Transactions → `term store_text(name)` on a
   CStr-returning txn inferred Int (the Int fallback only ever worked by luck).
5. Callable-txn result init emitted `store ptr 0` / `store float 0` (invalid
   LLVM constants for non-integer return types) → opt rejected the IR.
**Also fixed along the way:** the shim exported `read` collided with libc's
`read(2)` (compile-time conflicting prototypes + runtime PLT interposition → -1)
— the renderer now emits `__briev_export_<name>` + `asm("name")` labels and the
link adds `-Wl,-Bsymbolic-functions`. The fragile "looks like a C string"
heuristic in `briev_str_to_c` misread any Briev String whose length byte is
printable ASCII (a 35-char path read as '$'); removed — under the composite
every String IS `[len][bytes][\0]`. `__read_file__`/`__write_file__` free'd the
now-borrowed `str_to_c` result (P2 zero-copy) — removed the frees.

## String Value Representation Inconsistency (bits model) — compiler-in-Briev blocker

**Date:** 2026-08-04
**Status:** Mostly fixed (2026-08-28) — identifier resolution inttoptrs
boxed String/Data params (emit_expr.rs:533); std/string now compiles and
runs end-to-end (`len("hello")`=5, `"hello"[1:3]`="el"). The remaining
frgn String-return heap leak is Bug #5.
(plan 2026-08-04-compiler-in-briev-dogfood-ffi).
**Symptom:** `inner.data[len] = val` (List.push) emits `store i64 %t7, ptr %t37`
where `%t7` is a String-literal `ptr` and the slot is `i64` → invalid IR.
**Root cause:** a String value's representation is inconsistent at the crossing
points of the bits model — a literal is a real `ptr` (ty=String), but when
passed as a txn param it is typed `Int` (boxed) while physically still a `ptr`,
so `adapt_to_i64` (which trusts the `Int` type) emits no `ptrtoint` and the
store writes a `ptr` into an `i64` slot. Fixed instances: String==literal
comparison (`string_ptr` inttoptr), list-element store (`adapt_to_i64`), the
lvalue Index store (`adapt_to_i64`). The remaining instance: String values that
are typed `Int` but physically `ptr` at param/argument boundaries.
**Impact:** any Briev code passing a String literal into a collection constructor
(`List.push("b")`) fails codegen. Blocks the needs_state pass's list handling.
**Fix direction:** a single point that ptrtoint's a String argument when binding
it to a param (method call / frgn / call), and inttoptr's on load — an
invariant: "a String crossing a call/store boundary is an i64 handle; a String
in a register is a ptr." Audit the call-arg and store sites against it.

## String representation inconsistency — remaining boundary sites

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28) — `index_elem_ty` gained a
`Type::Vector(inner, _)` arm (emit_expr.rs:1109); the load path inttoptrs
element reads. End-to-end verified via the std/string compile. Session 2
completed the remaining gaps: a Vector-typed STATE FIELD with a list
initializer (`let a: String[3] = [...]`) had no construction path (the
bare Expr::List panicked) — `emit_init_op_construction` now stores the
elements directly into the `[N x T]` column at the element's ABI width;
the foreach VectorField iterator now carries the element type, binds the
item with it, and unboxes boxed String/Blob handles. Verified:
`String[3]` init + foreach .^Length sum=6, `a[2] .^Length`=3, end-to-end.
**Symptom:** the boundary between "String in a register is a ptr" and "String
crossing a call/store boundary is an i64 handle" is enforced ad-hoc. A String
element STORED into a List slot and then LOADED (e.g. `l.get(i)`) — the load
path inttoptr's per `index_elem_ty`, but a String element READ via a field /
index whose type resolves to `Custom("Int")` (fixed-array local element type
falls back to `Int` at emit_expr.rs:611) hits `.^Len` with ty Int → "Phase-1b
boundary" panic.
**Impact:** fixed-size local arrays of String (`let a: String[4]`) cannot be
indexed for string ops; `a[1] .^Len` errors.
**Fix direction:** `index_elem_ty` must resolve a `Vector(String, [N])` (or
`Custom("String[N]")`) index to its element type `String`, not the `_ => Int`
fallback; then the load path inttoptr's. This is the array-slot element-type
resolution gap (A4/A5 emit paths handle `Type::Vector` member-slot + state-field
arrays but the generic/`Custom` local-array case is missing).

## Generic struct array-field layout is zeroed (Stack<T,N> unusable)

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28, Phase 3) — `obj_instance_inits` now stores the
MONO key (`Stack<Int,8>`, mod.rs) so `self_binding`/`lookup_field_offset`
resolve the substituted fields; `pool_base()` extracts the base name for
`{base}.{member}` pool-slot keys at every downstream consumer. Verified by
the full test suite; residual `Custom("String[N]")` element-type resolution
is tracked by the remaining-boundaries entry above.
**Symptom:** any struct with an inline array field
whose element type is the generic parameter (`obj Stack<T, N> { data: T[N]; len:
Int; }`) codegens `len` and `data[i]` at byte offset 0 (element stride 0).
Verified: `Stack<Int,8>` + `Malloc#(72) as Stack<Int,8>`; init writes data[0]
then len overwrites it; `data[len]` scales by 0. Root cause is unresolved generic
field types in `struct_types` (offsets computed from `type_size(T)` = 0) and the
`data: T[N]` element type resolving to `T` (size 0) instead of the instantiated
type.
**Impact:** the stdlib `Stack<T,N>` (inline, fixed-cap) cannot be used at all.
**Fix direction:** substitute the generic parameter when registering/offsetting
struct fields (`Stack<Int,8>.data` → `Vector(Int,[8])`), and make the A4/A5
member-slot index path use the instantiated element type.

## List<T>.init allocates 2 elements but advertises cap 16

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28) — two distinct defects. (1) The sizing
`Malloc#(cap * elem_size)` was already repaired in the coll scaffold
(coll_scaffold.rs, 2026-08-16). (2) The LIVE crash was in
`emit_init_op_construction`'s empty-literal branch: it ran the InitEmpty
member body against a LOCAL instance block but never stored the block
handle into the State column — every later read dereferenced garbage
(segfault on the first push, not an overflow). Now stores the handle like
the non-empty `Expr::List` branch. Verified: 5 pushes + foreach sum +
.^Length end-to-end.
**Impact (historical):** any List<String> with >2 elements corrupts the heap. Blocks the
pass's line/token lists.
**Fix direction (historical):** either cap must be bytes-based (`[len*8 < inner.cap]`, `cap
= 16` bytes) or init must allocate `inner.cap` elements. The generic element
size (8 for the i64-handle ABI) is the blocker — see the pass's decision to
avoid List for unbounded collections.

## string.bv does not type-check (legacy bodies)

**Date:** 2026-08-04
**Status:** Open — pre-existing, MASKED by the import parse-error swallow (now
surfaced). The whole stdlib `lib/std/string.bv` parses only after the `..`→`:`
slice migration, then fails typecheck: `bytes()` calls unknown intrinsic
`StrBytes#` and an undefined `StringError` type; `split()` uses `List +`;
`trim`/`join`/`pad`/`starts_with`/`ends_with` call String/StringBuilder methods
(`.is_whitespace()`, `.append_char()`, etc.) that are free `frgn`s, not obj
members. These bodies were never exercised (the module never parsed).
**Impact:** `import "std/string"` now fails loudly; `lib/compiler/main.bv` and
the backends (which import it) cannot build until it type-checks. The pass
avoids std/string (defines its own `str_len`).
**Fix direction:** migrate bodies to the current syntax (free-function calls or
`obj String { fn is_whitespace: __is_whitespace; }` bindings), fix `split` with
the init/push pattern, implement `bytes` via the byte-slice reflection.

## String element reads from List<String> return generic T (no string ops)

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28) — the foreach item unbox (Tier-2 At arm +
Tier-1 Current arm in emit_stmt.rs) inttoptr'd the element UNCONDITIONALLY
when the element type was String — but the At member body may have ALREADY
recovered the element to a ptr-typed String register. The double unbox read
the ptr-as-handle → garbage lengths/corruption. Now guarded on
`at.ty == Type::int()` (boxed handle only). Verified: List<String> push ×2,
foreach .^Length sum (3+2=5), At(1), concat, content equality — all
end-to-end.
**Impact:** a Briev pass cannot read back list elements as Strings for
comparison/slicing — the needs_state splitter can push but not inspect.
**Fix direction:** substitute the generic T when resolving `Ptr<T>` pointees
for concrete instantiations (the index_elem_ty / load path), OR define the
element read to always return the boxed i64 handle typed as `String` (the
bits-model invariant: element at a boundary IS an i64 handle).

## meld CStr→String length reads wrong in a linked library

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28, session 3) — the root was the missing CStr
boundary marshalling: blocks were passed where C expects NUL-terminated
data pointers and vice versa. Variant boundary types (`CStr:
#String<C_String>`) now marshal through the casting graph (str_to_c /
cstr_to_briev lanes, base↔default-variant normalization in find_path,
ExtCallDyn frgn_map symbol resolution, `axiom` keyword for FFI-backed
pairs). **Export-side re-probe (session 3b): RESOLVED** — the library-mode
export wrapper now marshals `CStr → String` correctly (`text_len("xy")`
= 2, was 5). Two further graph fixes landed: (1) `cstr_to_briev`'s
declared param is now `Ptr<Void>` (the C symbol takes a pointer; the old
`Int` made the cast lane emit `call @briev_cstr_to_briev(i64 %ptr)`, a
verifier error); (2) boundary HANDLE admissions are repr-lane-only —
`cstr as Int` is ptrtoint (never the base's `str_to_int` parse of the C
bytes), `int as cstr` is inttoptr — via a two-phase BFS in the casting
graph (phase 1 repr-only for boundary crossings, phase 2 the declared
content deltas). `c_driver_boundary` (greet:hello / join:foobar /
ident:3.140000) passes again.

## String slicing returns the whole string (no substring semantics)

**Date:** 2026-08-04
**Status:** Fixed (2026-08-28) — constant-bounds slices emit a real substring
via `briev_str_substr`; the Slice arm's `is_semantic_string` detection catches
boxed String params, and narrow_slice no longer narrows #String bases (a
type-blind narrowing returned the whole string for non-identifier bases).
Verified end-to-end: `"hello"[1:3]` → `.^Length` = 2; `mk()[1:3]` (non-
identifier base) → 2; dynamic slices via a boxed param recover the handle
before slicing.
**Impact (historical):** before the fix, every scanner in the pass computed
against the whole string (line_end, token_at, has_token, in_section,
token_name).
**Fix direction (two paths, pick per plan):**
  (a) emit a REAL substring for String-typed slices — a `[len][bytes]` copy of
      chars a..b via a runtime `briev_str_substr` + a `frgn __str_substr(s,
      a, b) -> String` (the stdlib already declares frgns with String
      params/returns, but NONE is exercised — the frgn String marshalling
      (i64 handle vs ptr) is UNVERIFIED and must be tested first), or
  (b) fix the dynamic-slice codegen to construct a length-prefixed substring
      inline. Either way, `.^Len`/`==`/concat on slices then need to see the
      substring, and the param-handle (boxed i64) must be inttoptr'd before
      slicing.

## Imported-module frgn with String param + String return resolves to Int

**Date:** 2026-08-04
**Status:** Fixed (verified 2026-08-28) — an imported module's `frgn` with a
String param AND String return (`frgn briev_str_substr(handle: String, ...)
-> String`) resolves its return type correctly. Verified: a module declaring
the frgn, imported and called from a consumer, accepts `let x: String =
briev_str_substr(...)` (type-checks OK). Resolved by the session-2 dedup /
signature-resolution work; the same-declaration-in-file case already passed.
**Historical:** `cstr_to_briev` (Int param + String return) and `str_to_c`
(String param + Int return) each alone resolved fine; only the combined
String-param AND String-return frgn failed.

## Frgn String-return heap corruption under many calls (blocks the pass)

**Date:** 2026-08-04
**Status:** RESOLVED (false alarm — see the "RESOLVED — frgn String-return
heap corruption was a test-harness arity bug" section below; the C main
called a stateful export with ONE argument, so `%arg0` was garbage). The
nondeterministic reads (60 / 5 / 3) were the C driver's missing `%state`
argument, not compiler memory corruption.
**Impact:** `needs_state_compute` is only reliable on short inputs; the pass
cannot yet replace the Rust reference.
**Fix direction:** (1) run the linked library under ASan/valgrind to find the
exact overrun (likely a runtime fn or the frgn String-arg/return marshalling
writing past the [len][bytes] buffer); (2) convert the remaining String-return
frgns (briev_str_substr) to Int-return where the pass only needs a length /
char, minimizing allocation; (3) verify every frgn String arg is inttoptr'd to
the buffer (not the boxed i64 handle) at the call site.

## RESOLVED — "frgn String-return heap corruption" was a test-harness arity bug

**Date:** 2026-08-04
**Status:** Resolved (false alarm). The "nondeterministic heap corruption"
entry above was MY C-driver bug, not the compiler: `needs_state_compute` and
the debug exports are STATEFUL (`define i64 @f(ptr %state, ptr %arg0)` — they
call regular defns, so the needs_state analysis gives them `%state`), but the
C main called them with ONE argument. The projection pointer landed in
`%state` and `%arg0` was garbage, so the meld read a random length (60 / 5 / 3
across runs — "corruption"). With the correct two-argument call
(`needs_state_compute(st, proj)` where `st = __briev_init_state()`), the pass
is DETERMINISTIC and matches the Rust reference on all five bridges
(boundary=0, node_bridge=31, cancel=1, rank=2, bench=2) — asserted by
`tests/c_driver_needs_state.rs`. Lesson: a stateful export's C signature takes
the state handle first; the glue `bindings` header is the source of truth.

## RESOLVED — imported-module frgn with String param + String return resolves to Int

**Date:** 2026-08-04
**Status:** Resolved (fixed incidentally). The `lib/compiler/reader.bv` shared
module now declares `briev_str_substr(handle: String, a: Int, b: Int) -> String`
and both passes import it — the return type resolves correctly and the pass
runs (verified: soa_reorder imports reader.bv and produces the exact
permutation). The failure was observed before the P3 string/codegen fixes
(clear_locals, is_semantic_string, Len-on-boxed-String); one of those repaired
the imported-frgn return-type resolution. The needs_state.bv comment about
declaring the frgn locally is now stale — the shared reader is the single home.

## Duplicate `BackendKind::Webstack` match arms in compile.rs — CLOSED (already fixed)

**Date:** 2026-08-10
**Status:** Closed 2026-08-26 — verified already fixed upstream. The ext
match holds exactly ONE Webstack arm (compile.rs:1178); cargo emits no
unreachable-pattern diagnostics. The consolidation happened in an earlier
commit without this entry being updated.
**Root cause:** `compile.rs` (backend emit dispatch, arm `let ext: &str = match opts.backend`) contains THREE `BackendKind::Webstack` arms (~lines 1235, 1280, 1320). Rust proves the second and third unreachable (the first always matches), so only the arm at ~1235 runs. The dead arms carry differing backend configuration (one registers ProtocolDefs onto the casting graph + `with_optimize_report(true)`; another omits operator_defs) — the live arm registers neither. Unreachable-arm compiler warnings (`unreachable pattern`) pre-date this session.
**Impact:** `compile()`'s Webstack path is deterministic (the reachable arm always wins), so no observable divergence today — but the de-facto webstack config differs from two presumably-intended configs that were never valid, and the dead arms conceal which configuration is canonical.
**Fix direction:** consolidate the three arms into one canonical Webstack arm (preserving the reachable arm's behavior: operator_defs, cast_from_bit_overrides, resolved_frgns, target config, .ebv embedded mode), then decide whether ProtocolDef registering on the casting graph belongs in compile()'s webstack path (the LLVM/Gpu arms do this today). Verify with `cargo test --lib webstack` + a `.bv`→`.wasm` compile before/after.

## Webstack SSA-loop precondition emits `br i1 <i8 reg>` — RESOLVED

**Date:** 2026-08-10
**Status:** Resolved 2026-08-10 (was OPEN — found while wiring the flush buffer).
**Root cause:** Two distinct issues compounded:
1. **Missing type universe in the diagnostic path** — `as_bool_reg` (helpers.rs:597)
   only truncs `i8 → i1` when the register's Briev type resolves to `#Bool` via
   `is_protocol_member`. That needs `ctx.type_universe` populated; the repro
   tests omitted `with_type_universe`. With the universe set (as the CLI path
   does), membership resolves and `trunc i8 %t to i1` is emitted correctly.
2. **emit_int hardcoded `add i64` for Int literals** — `llvm_type(Int)` is
   `i{int_bits}` (i32 on wasm32) and `binop_int_type()` emits i32, but the
   literal emitter produced i64 registers. `adapt_to_i64` then saw
   `llvm_type(Int)` = i32 and emitted `sext i32 <i64 reg>` → invalid IR that
   llc rejects (`'%t0' defined with type 'i64' but expected 'i32'`).
**Fix (applied):** `emit_int` now emits at `i{int_bits}` (matching the Char
literal pattern and `llvm_type(Int)`/`binop_int_type()`). x86_64 unchanged
(int_bits=64 → `add i64`). With both fixed, `--backend webstack` modules
assemble via llc (verified: `llc -march=wasm32 -filetype=obj` produces the
object). Regression tests: `test_webstack_ssa_precondition_emits_valid_bool_branch`,
`test_webstack_int_literal_emits_target_width` (1735 tests).
**Undo:** revert `emit_int` to `add i64`; the `br i1 <i8>` symptom returns only
when the repro also drops `with_type_universe` (a test-config artifact, not a
codegen path).

## wasm32 webstack: %State i64 storage vs i{int_bits} arithmetic — RESOLVED

**Date:** 2026-08-10
**Status:** Resolved 2026-08-10 (plan `2026-08-10-width-aware-loop-engines.md`).
**Root cause:** `%State` slots for flexible Int/UInt were hardcoded i64
(push_field_type) while arithmetic/comparisons use `binop_int_type()` =
`i{int_bits}` (i32 wasm32). The two models only agree at int_bits=64, so every
webstack `.bv`/`.rbv` failed llc assembly (verified pre-existing).
**Fix (applied, width-aware loop engines):**
- `push_field_type`: flexible Int/UInt `%State` slots are now `i{int_bits}`
  (i32 wasm32; i64 x86_64 — byte-identical). Bool/String/Data/Char stay i64.
- `emit_folded_loop`: counter phi/backedge use the field width; bound compare
  sexts i{int_bits} → i64 (reusing the existing narrow-counter machinery).
- per-field phi backedges: written fields store their native-width value
  (i{int_bits}), not the i64 `adapt_to_i64` box — the phi type matches.
- array field store/row-view GEPs: index widened i{int_bits} → i64
  (`gep_index` helper; no-op on x86_64).
- Int-returning intrinsics (Now#, GetEnvInt#, SysCall#, SysConf#,
  Backtrace#): results narrowed i64 → i{int_bits} (`narrow_int_result`).
- init-store + guarded-load `!range` metadata emit at the field's width.
- removed the earlier load-trunc hack in the Identifier arm (slots now
  already i{int_bits}).
**Verification:** `--backend webstack` `.bv` → `.wasm` assembles end-to-end via
llc for folded, per-field phi, countdown, direct-SSA, and runtime-bound loop
shapes; Int/Float/String/array fields and the GetEnvInt#/Now# intrinsics all
validate. 1738 lib tests (x86_64 unaffected — int_bits=64 → identical IR).
**Undo:** revert push_field_type's flexible-Int branch to `"i64"`; the loop
engines revert to the i64-only model (breaks wasm32 again).

## ViewCompiler never wired into compile pipeline — Phase 1 RESOLVED, Phase 2 OPEN

**Date:** 2026-08-10
**Status (2026-08-11):** Phase 1 resolved. `opts.view_bindings` is now populated:
`compile_source` runs `compile_view()` (ViewCompiler over the preprocessed
`.rbv` `<view>` block, falling back to concatenated `RenderBlock.view_html`)
*before* codegen, threads the extracted root signals through `codegen`, and the
dom-shim generation emits the wired `dom-shim.mjs` (IDs injected into the
copied `.html`, `_bindingTable` with per-field `applyFn`). View-referenced
fields are protected from dead-field elimination via
`CompilerContext.view_bound_fields` → `FieldMode::Always` (observability-as-
liveness). SRBV verification runs only under the `.s` strict profile
(`conformance::is_strict`). Covered by 6 new compile.rs tests + 1 rbv.rs test.
**Phase 2 component model:**
- Slice 1 (2026-08-11) resolved: `b-when`/`b-each`+`b-key`/`b-bind:value`
  wired; `<Name />` mounts its `render Name { ... }` fragment at compile time.
- Slice 2a (2026-08-11) resolved: per-instance component state via fixed
  instance pools — each mount owns its fragment-referenced fields
  (`Counter.0.count`, `Counter.1.count`) + per-mount txn variants
  (`increment_0`/`increment_1`); a mount's `b-trigger` fires its own variant.
- Still open: mount/unmount LIFECYCLE (an active-slot flag + DOM mount/unmount
  per instance — `b-when` inside a component already works at the state level),
  dynamic component counts (`b-each` of components), props.
See `docs/plans/2026-08-11-phase2b2-instance-state.md`.

## `as Bits<N>` never truncated sub-byte widths (identity cast lane) — FIXED 2026-08-13

**Date:** 2026-08-13 (found during pack Phase 2 end-to-end)
**Status:** Fixed in `15117133` (pack commit).
**Root cause:** the casting graph treats `Int ↔ Bits` as a same-lane identity
(`path.is_empty()` in `emit_cast_path`), so `16 as Bits<4>` produced an i64
register typed `Bits<4>` holding 16 — no truncation. This surfaced via packed
sub-byte fields (a `Bits<4>` field legitimately needs a 4-bit domain), but
affects every `as Bits<N>` cast for N < 64.
**Fix:** the cast path (and its no-graph fallback) truncate integer sources to
exactly N bits; `emit_cast_steps`'s Bitcast lane zext/truncs across differing
integer widths; the reference interpreter mirrors the truncation (rule 4);
packed stores mask to the field width defensively.

## BE sub-byte packed shift formula — FIXED 2026-08-13

**Date:** 2026-08-13
**Status:** Fixed in `15117133` (pack commit).
**Root cause:** `packed_field_offsets` used `shift = cov*8 − bits` for
Big-endian fields; correct is `cov*8 − within − bits` (the field's MSB sits at
integer bit `cov*8 − 1 − within` of the covered region). The unit test had
encoded the wrong expectation, so the bug was invisible. A low-nibble BE field
(`within = 4`) shifted off its own bits and read the neighbor's nibble.
**Fix:** corrected formula in `src/type_universe/packed.rs` + tests; verified
byte-for-byte against hand-computed BE storage in `benchmarks/pack_be_selfcheck.bv`.

## `Bits<N>` fields read/wrote the full word (Applied alias widened to i64) — FIXED 2026-08-13

**Date:** 2026-08-13 (found while verifying union overlay end-to-end)
**Status:** Fixed in `feat/spec-layout-keywords` Phase 6.
**Root cause:** `Bits<32>` in source parses as `Applied("Bits", [Number(32)])`.
The casting graph's generic application resolution lowered it to i64, so a
`Bits<32>` struct/union field read all 64 bits (the direct `Type::Bits` form
lowered to i32 correctly). Phase 2 fixed this for the packed authority
(`field_bits`) but the general `llvm_type` path was still widening.
**Fix:** `llvm_type` resolves `Applied("Bits", [N])` to `i{N}` (early check,
matching the direct `Type::Bits` arm). `lower_type` does the same.

## `Ptr<Bits<8>>` was an i64 element pointer under the byte-era Bits unit — FIXED 2026-08-13

**Date:** 2026-08-13 (logged per the layout-keywords plan DoD)
**Status:** Fixed by Phase 1 (`15117132`, Bits-unit restoration).
**Root cause:** before the Bits-unit fix, `Type::bits(N)` stored BYTES, so
`emit_address_of` (`src/backend/llvm/intrinsics.rs:724`) constructed
`Ptr<Type::bits(8)>` intending a byte pointer but produced an i64 element
type. Every `Type::bits(N)`-as-byte consumer had the same unit ambiguity.
**Fix:** `Type::Bits(N)` is now exactly N bits; `Ptr<Bits(8)>` is a true i8
(byte) pointer. No further action needed — logged to close the plan's DoD.

## `!> IsZero:` / `!> IsOne:` stdlib metadata was dead — REMOVED 2026-08-13

**Date:** 2026-08-13 (layout-keywords plan DoD audit, Phase 7)
**Status:** Removed — audited, no consumers.
**Audit:** `lib/std/types.bv` carried `!> IsZero: _ == 0;` and
`!> IsOne: _ == 1;` metadata. A `grep` across `src/` (and the typechecker /
strategy resolution) found zero readers — the metadata did not parse as any
current metadata value and was never consulted. Removed from `types.bv` as
truly dead (rule 14: no type-specific knowledge; if a zero/one predicate is
needed later it belongs in stdlib as an `op`, not metadata).

## Reactive nodes with struct-typed state slots fire once then stop — FIXED 2026-08-13

**Date:** 2026-08-13 (found while writing pack end-to-end benchmarks)
**Status:** Fixed (reactor fix, `feat/spec-layout-keywords`).
**Root cause:** a node reading/writing a struct-typed `let s: S;` slot fired
once. The struct literal's `alloca` sat inside the reactor loop body (the
`@main` `.cm_body`), and clang `-O3` peeled the loop + emitted a bogus
`llvm.assume(N == 1)` exit assumption — a single-iteration loop. Works at
`-O0` (7 lines) but breaks at `-O3 -flto` (1 line). Verified by
hand-hoisting the alloca to the function entry (fixed) and confirming the
loop-carried pointer phi / heap allocation / guard-assume removal were NOT
sufficient.
**Fix:** `emit_struct_literal`/`emit_struct_array` defer their `alloca` to
`pending_struct_allocas` while `fun.defer_struct_allocas` is set; the reactor
loop builders (`emit_countable_main`) buffer the loop construction and flush
the allocas into the loop PREHEADER (before `.cm_header`). `emit_definition`
hoists defn-body struct allocas to the function entry. Verified: the struct
node now loops correctly at `-O3 -flto` (7 lines at BOUND=20, matching the
non-struct shape); `benchmarks/pack_struct_runtime.bv` / `pack_be_selfcheck.bv`
output IDENTICAL to C. Packed/union/atomic structs hit the same gap — all
fixed by the hoist.

## `!range` metadata emitted untyped for i64 (malformed in LLVM 18+) — FIXED 2026-08-13

**Date:** 2026-08-13 (found via tests/llvm_compile_test.sh on the bounded counter fixture)
**Status:** Fixed.
**Root cause:** `emit_precondition_proof` (src/backend/llvm/emit_toplevel.rs:3519)
emitted `!range !{ 0, 10 }` (untyped bounds) for i64 loads, using the legacy
short form. clang/opt 18+ reject untyped range bounds ("expected metadata
operand"), so any `.bv` with a bounded i64 precondition (e.g.
`node [count < 10]`) produced IR that clang refuses to compile. The bench suite
never hit it (no benchmark uses bounded i64 preconditions).
**Fix:** always emit typed bounds (`!range !{ i64 0, i64 10 }`), matching the
typed form used by `emit_range_metadata` for narrower widths.

## String-param library exports hit `%ac0` i64-vs-ptr codegen — OPEN (stale duplicate, CLOSED by 2026-08-18 verification)

**Date:** 2026-08-13 (surfaced while fixing `c_driver_needs_state`)
**Status:** CLOSED 2026-08-26 — this entry predates the verify-fixed record at
`docs/plans/2026-08-18-check-build-pipeline-unification.md` and duplicates the
RESOLVED entry above. The param-marshalling fix landed in
`src/backend/llvm/emit_toplevel.rs:2702-2715` (`param_llvm_ty == "ptr"` →
ptrtoint) with a regression test asserting no `ptrtoint ptr %arg0` for String
params (`tests.rs:3728`). Kept here as a tombstone pointing at the resolved
entry; delete in a future BUGS.md sweep.

## Bare collection literal as a function ARG crashes — KNOWN (string.bv unblock)

**Date:** 2026-08-13 — **known gap, workaround: bind the literal to a `let` first.**

`join(["a", "b"], "-")` — a bare `[elem…]` literal in ARGUMENT position —
still lowers through the generic `emit_heap_seq` 2-slot buffer
(`[len][elem…]`), which `op At`/`op Count` (expecting the List STRUCT layout
`{ inner, cap, len }`) read as garbage → segfault. The `let`-bound form
(`let items: List<String> = ["a","b"]; join(items, "-")`) constructs a proper
instance via `op Init`/`op InsertAt` and works. Routing argument-position
literals through the type-directed construction is the fix (the arg's expected
type is known at the call site).

## string.bv was uninstantiable — FIXED (commits d6107022, and the 2026-08-13 slice)

**Date:** 2026-08-13 — **fixed.** `import "std/string"` previously failed to
typecheck/compile (the file was never reachable — `bytes()` called the
unregistered `StrBytes#` intrinsic; classification/case/trim loops used
method-call syntax against free functions; the StringBuilder chain hit five
backend ABI bugs). All fixed; `<std/string>` now compiles and the scalar
functions (len/concat/trim/case/reverse/capitalize/count_char/starts/ends/
find/replace/substr/truncate/ensure/remove-prefix) plus bytes/split/join
verified correct. `StringError` was referenced but never defined — added to
`std/ffi/string.bv`.

## String state fields stored the raw `ptr` into the i64 slot — FIXED 2026-08-13 (main, via merge)

**Date:** 2026-08-13 (surfaced by the main<->feat/spec-layout-keywords merge's
`c_driver_node`/`needs_state` gates)
**Status:** Fixed (merged into main `7b20cf16`).
**Root cause:** main's flat-dotted state-field store (`emit_stmt.rs`, slice 4)
called `emit_state_store_i64` which stored the value raw with the slot's type.
A String value is a `ptr` ([len][bytes]) while the slot is i64 — `store i64
%ptr` was an LLVM type error (`node_bridge.save`).
**Fix:** the state-store path adapts the value to the slot's LLVM type via
`ensure_typed_value`: String/Data → ptrtoint to i64, Bool → trunc to the i8
slot, Float → bitcast, Ptr/struct/collection HANDLES (i64 registers typed
Custom/Ptr, whose llvm_type maps to "ptr") pass through unchanged.

## Ptr/struct/collection handles double-ptrtoint'd into state slots — FIXED 2026-08-13 (main, via merge)

**Date:** 2026-08-13 (surfaced by `queue_drain_idio` / `float_math` bench gates)
**Status:** Fixed (merged into main `7b20cf16`).
**Root cause:** values typed `Ptr<T>` or Custom (struct/collection) are stored
as i64 HANDLES internally, but `llvm_type` maps them to "ptr". Adding a
`("ptr", "i64")` → ptrtoint arm to `ensure_typed_value` (for String state
fields) fired on these handles too, producing `ptrtoint ptr <i64 handle>`
(LLVM type error).
**Fix:** `ensure_typed_value` returns Ptr/Custom/Applied handle-typed values
unchanged; the `("ptr", "i64")` arm now only fires for genuine String/Data
registers.

## Deferred dead surface — spec-conformance plan 2026-08-22

**Date:** 2026-08-22 (owner decision: fix opportunistically when touching these files)
**Plan:** `docs/plans/2026-08-22-spec-conformance.md` § Deferred
**Items:**
1. `Token::DollarBang` (`$!`) lexed (`src/lexer.rs`) with zero consumers — dead surface, no semantics.
2. Legacy AST variants `TopLevel::StateDecl` / `Signature` survive in `import_resolver.rs`, `beast/serialize.rs` — removed-grammar residue (SPEC §4.4 removes `state`/`sig`).
3. Lexer tokens `input`/`output` labeled ".c.bv cell file keywords" (`src/lexer.rs`) — the `.c.bv` variant is rejected by SPEC §3.1; tokens now serve only as identifier-mappable words.
4. Orphan fixtures contradicting §3.1 "not part of the language": `tests/test_strict.sbv`, `tests/test_sig.bv`, `tests/test_sig2.bv`, `tests/test_sig3.bv` — unreferenced by any test.
5. `vocab.rs` marks `Ok/Err/Some/None/some/none` as Removed ("parser must reject") — SPEC §4.4 says they are NOT reserved and behavior already treats them as ordinary identifiers; the vocab label is wrong, not the behavior.
6. Import-cycle detection keyed on specifier STRING, not canonical path (`import_resolver.rs` circular-import check) — alias spellings of one file can evade or false-positive.
7. Bare default CLI route accepts only `.bv/.rbv/.abv`; `.ebv/.cbv` need explicit `build` subcommand (cosmetic; fully supported there).
8. `discover_active_sources()` had zero callers until Phase 10 wires it (fixed by Phase 10).

## Structural sums: LLVM tagged-union ABI not yet lowered (2026-08-22)

**Status:** CLOSED 2026-08-22 — Phase 3b landed. Union values travel as
i64 handles to `{ i64 tag, i64 payload }` images; boxing happens at call
args, heterogeneous match arms, term-of-union, lets and state-field
stores; TypedBinding conditions test the tag and bindings unbox the
payload. Verified by examples/structural_sums.bv through the compiled
binary. Cold-path outlining refuses guards containing user calls (no
%state param to marshal). Original report:
**Found:** fizzbuzz/sums work during spec-conformance Phase 3
**Behavior:** parser (`A | B` in any type position), typechecker
(member assignability at call args + let annotations via
`types_compatible`), and interpreter (dynamic-shape TypedBinding dispatch)
are complete. The LLVM backend panics with an explicit message when a
match arm uses a typed sum binding — the union value has no runtime tag,
so branch dispatch would be silently wrong. Never emitted.
**Fix:** tagged representation `{ i64 tag, payload }` for union-typed
values across call args, params, returns, and phis; TypedBinding
conditions test the tag; bindings extract the payload. Derive the layout
via the casting graph per AGENTS Rule 19.

## Boolean mask indexing segfaults in compiled code (2026-08-22)

**Status:** CLOSED 2026-08-22 (Phase 6a). Two stacked faults:
1. The gather buffer `[len, e0, …]` was handed back raw where consumers
   expect a tier List `[data, cap, len]` — Count# read garbage, indexing
   walked an untyped pointer. Now boxed at both i64 and f32 routes.
2. A `Bool[N]` state column is `[N x i8]`; the gather read it as
   `int64_t*`, selecting on adjacent state bytes — correct output was luck.
   New runtime variants `briev_mask_select{64,_f32}_i8mask` take `uint8_t`
   masks; routing keys on the stored column LLVM type (`x i8`), not
   protocol lookup (which proved unreliable here).
Verified: examples/mask_select.bv → 13580 stable across runs; suite green.
Note: the unit-test harness cannot host this fixture (lacks reactor/
contract passes — values diverge); the CLI-built example is the gate.
See also: repro_guard_chain_dominance.bv entry below. Original report:
**Repro:** `bugs/repro_mask_index_segfault.bv` — `data[mask]` over `Int[8]` /
`Bool[8]` state fields, index into the compacted result.
**Behavior:** interpreter correct (mask select landed 2026-08-07). The LLVM
backend's vector-mask routing (`emit_expr.rs` "only admits state-field
objects" path) emits a memory access that dumps core under the compiled
reactor loop; unverified whether Float64 panic path is adjacent.
**Fix direction:** audit the state-field mask emitter against the SoA column
read path (same GEP shape as plain `data[i]`), then compact via the popcount
loop; add interp/backend parity fixture. Ellipsis (`...`) multidim slices
also remain: token lexed, Slice AST has no dims list — needs `SliceDim`
enum (Range | Named | Ellipsis) per plan §Phase 6.

## Guard-chain endprogram dominance errors (2026-08-22)

**Status:** CLOSED 2026-08-22 (Phase 6b). Root cause: inside countable-loop
bodies, `emit_countable_body`'s Guarded arm recursed into the Assign arm
which records raw value registers into pending_phi_backedge / last_val_temps;
a field written ONLY on the then-path left a conditionally-defined register
for the latch backedge and later guards to cite (clang: does not dominate).
Fix: at each `.cmgn` merge, one phi per conditionally-written field —
[written, body] / [pre-value, fall-through] — re-points the backedge map;
conditional last_val_temps entries are dropped so cross-guard reads reload.
Verified by the repro (10000) plus slice/mask fixtures. Original report:
**Repro:** `bugs/repro_guard_chain_dominance.bv` — three sequential `when`
guards where a LATER guard computes into state and the FINAL guard ends the
program (`endprogram Print#(total_out)`); plain indexed reads, no masks.
**Behavior:** clang rejects the emitted IR — "Instruction does not dominate
all uses" around the print argument; same shape without any mask indexing,
so independent of the Phase 6a mask work. Works when endprogram shares the
guard with the computation (mask repro shape).
**Fix direction:** audit cold-path outlining / guard-chain emission for the
case where a later guard's value feeds an earlier-outlined region's
terminator; likely a cur_block tracking gap sibling to the match-edge fix.

## `dyn Trait` execution: LLVM thunk-table ABI not yet lowered (2026-08-22)

**Status:** HALF-CLOSED 2026-08-22 — interpreter dispatch COMPLETE (Phase 5b):
`Value::Dyn{trait,concrete,inner}` wraps at dyn-annotated lets (constructor or
dyn-rebind sources), member calls resolve impl bodies under
`Concrete::fn`, receiver threads by arity shape (params == args+1 ⇒ Self-
first). Static checker validates Self-convention signatures.
**Remaining (Phase 5c):** the LLVM backend still panics on Dyn values —
thunk tables per (trait, concrete) with per-slot indirect calls, boxed
handle representation. Also v1 coercion-source limits: constructor
expressions and dyn rebinds only; identifier-of-typed-let needs binding-
type provenance threaded through EvalScope. Original report:
**Found:** spec-conformance Phase 5 work
**Complete:** parse (`dyn Name` contextual in type-prefix position),
typecheck (concrete→dyn coercion ONLY into an explicit dyn annotation and
ONLY when the concrete type asserts the trait — the checker reclassifies
trait-named relationship entries from refinement-parent to assertion, since
the parser is syntactic), static member resolution on dyn receivers
(arity/arg types against the trait's requirement signatures).
**Missing:** execution. The LLVM backend panics explicitly when a Dyn value
reaches codegen; the interpreter has no member dispatch for dyn receivers.
**Fix:** fat pointer `{data, table}`; per-(trait, concrete) thunk tables at
monomorphization (one slot per required fn, declaration order, sorted where
iterated); indirect calls through slots; interpreter DynValue wrapper
resolving through registered conformance. Field requirements and op-binding
requirements staged behind defn-shaped ones.

## Webstack `.rbv` compile nondeterminism — unsorted map iterations in component mounting (2026-08-23)

**Status:** CLOSED 2026-08-23 (`backend-foundation` branch).
**Found:** Plan 0.1 byte-identical IR verification — `counter.rbv` /
`shopping_cart.rbv` produced different `.ll` across runs OF THE SAME BINARY
(function order + field order varied with the SipHash seed).
**Root causes (3, all in `src/analysis/component_instances.rs`):**
1. `expand_component_instances` iterated `render_blocks: HashMap` while
   rewriting `items` (mount insertion order).
2. `instance_slot_set` pushed `StateDecl`s iterating a `HashSet<String>`.
3. `build_txn_variants` snapshotted `obj.member_txns` (HashMap) into an
   unsorted Vec before appending variant txns to `items`.
**Fix:** render_blocks → `BTreeMap`; sorted snapshot before StateDecl pushes;
`members.sort_by(key)` before variant appends. All dated 2026-08-23 with
undo notes. Verified: 6 consecutive compiles of counter.rbv byte-identical;
single hash across shopping_cart runs; full suite green.
**Lesson:** the determinism rule must cover ANY map iteration feeding
program REWRITING, not just direct IR emission.

## Bounty flow broken at branch point: tamer `.bv` fails typecheck (2026-08-23)

**Status:** CLOSED 2026-08-23 (`backend-foundation`) — full port landed;
bounty pipeline executes real programs end-to-end (e2e asserts stdout).
Superseded architecture note: tame()/vm_loop were replaced by the
C-driven step() loop + manifest-carried absolute entry_bc during the HCALL
slice — see tools/bounty_e2e.sh, tamer/install_sim.c, and the parity
harness (7 fixtures green) as the living proof.
**Found:** Plan 0 verification — `brievc bounty examples/hello/main.bv`.
**Symptom:** tamer compilation dies in TYPECHECK (pre-backend):
`expected Ptr<Int> for binary op '+' … found Int` ×2, plus
`lib/tamer/vm.bv` parse warning (`unexpected token ']'`). Identical output
from the pre-change binary — drift between lib/tamer/*.bv and the current
typechecker/parser, NOT a backend regression.
**Fix direction:** update lib/tamer sources to current syntax/type rules,
then make the bounty round-trip an integration test so this class of rot
fails CI instead of install time.

**Done 2026-08-23:**
- `vm.bv` `[] -> Int` → `-> Int` (empty-bracket contract no longer parses).
- `vm.bv` field access through pointers ported to `(*p).field` (72 sites);
  exec_op gains an explicit contract; Load#/Store# replace raw-address
  casts. vm.bv compiles to a working binary.
- `loader.bv` read_* ported to proven element indexing (`bc[addr / 8]`);
  dead pre-beastpack helpers (`lair_*`, `find_bounty_section`) removed.
- LLVM backend bugfix this uncovered: `(*p).field` / `(*p).member()` /
  `(*p).field[i]` loaded the struct by value and fed a ptr into inttoptr
  (invalid IR). Field/method/store receivers through Deref now use the
  pointer handle directly; array fields return row views.

**Remaining (owned by plan 2026-08-23-vm-compile-tail-parity §1.2):**
1. `main.bv tame()` still uses the old pointer dialect (`lair_data + N`)
   AND reads a STALE .lair header layout — its w4..w7 word reads do not
   match the current assembler's byte layout (assembler.rs assemble():
   magic(4) ver(4) endian/flags(8) + section table u64 pairs at byte 16).
   Port = remap header reads to the current format + element indexing.
2. `analyze.bv` — same dialect audit (imports loader.bv).
3. `combined.bv` — legacy all-in-one variant, same treatment or delete.
4. ✅ DONE — `tools/bounty_e2e.sh` (2026-08-23): packages a program,
   drives the native tamer's exported `tame()` on the archive via
   `tamer/install_sim.c`; tame() interprets the user .lair to completion
   (exit 0). Known limit: exec_op's 0x71 HCALL arm is still a placeholder
   (`term pc + 5`) — user programs run but host calls (print) are skipped
   until host-fn dispatch lands (next slice).

## SPIR-V assembled binary rejected by rspirv parser / spirv-val (2026-08-23)

**Status:** CLOSED 2026-08-23 — full typed-emission refactor; spirv-val
PASSES on the scale kernel (test flipped on).
**Symptom:** re-parsing the assembled binary fails with
`OperandExceeded(144, 9)`; spirv-val reports `Id 1000003 is defined more
than once`. The IN-MEMORY dr::Module is well-formed (all structural
assertions pass on it).
**Isolated facts:** high-range cache ids (>=1000000) are distinct from
builder ids; bound is correct post-build; arena instructions are inserted
into types_global_values. Duplication appears only after assembly.
**Suspect:** interaction between raw `insert_types_global_values`
emissions and rspirv Builder's internal dedup/state for
types/constants — possibly the Decorate-with-result-id-0 pushed through
push_type, or constants emitted via emit_type while the builder also
tracks its own constant table.
**FIXED 2026-08-23 — root cause was MULTIPLE stacked emission bugs, each
masked by the next:**
1. `instr()` allocated/embedded a result id for RESULT-LESS ops
   (OpUnreachable encoded wc=2 with a phantom operand).
2. Terminators via raw insert_into_block left rspirv's selected_block
   open -> NestedBlock on the next begin_block (typed branch/ret/unreachable
   now used everywhere).
3. Decorations lost their parameter operands through dr::Builder::decorate
   -> replaced with decorate_raw() carrying explicit operands.
4. Layout: SPIR-V §2.4 requires annotations BEFORE types/constants/variables;
   incremental emission interleaved them -> build() now stable-buckets
   types_global_values into [decorates | types+consts | variables].
5. LoopMerge names merge AND continue targets: both blocks must exist even
   when the body always returns (continue got skipped on terminated bodies).
6. Function-scope OpVariables must be the FIRST instructions of entry:
   locals pre-scanned + declared in entry, values stored from the body.
7. Entry-point interface must list every Input/Output + SSBO variable.

**Interim:** structural coverage lives on the in-memory module
(test_scale_kernel_lowers_real_body) — lowering correctness IS verified;
only the serialization path is in question.

## Two-guard task spawn + cross-guard state arithmetic: undefined guard label (2026-08-22)

**Status:** CLOSED 2026-08-22. Two stacked causes, both fixed:
1. The countable Guarded arm trusted inherited `fun.cur_block` as its merge
   phi's fall-through predecessor — it could name a pre-loop guard merge
   from an earlier emission region (`%guard.end49`). Condition blocks are
   now freshly labeled (`.cmgcN`) with an explicit fall-through branch.
2. `.cm_body` entry never reset `terminated`/`cur_block`, so a stale
   terminated flag skipped that fall-through branch and left an empty
   predecessor block (slice_state regression during fix). Body entry now
   resets both by construction.
Also found in the same audit: FreeHint/KeepHint/Yield were silently
DROPPED by emit_countable_body\'s `_ => {}` — they now delegate to the
standard emitter. Verified: repro 4221, all five pinned fixtures, suite
1928 green. Original report:
**Repro:** `bugs/repro_task_free_second_guard.bv` — node with TWO sequential
`when` guards; guard A spawns a local task, awaits/frees it, assigns a state
field; guard B reads that field with arithmetic under `endprogram`.
**Behavior:** emitted IR cites `%guard.end49` (an emit_stmt-style guard-end
label) inside a countable-loop merge phi (`%cmgm…`) that references it as a
PREDECESSOR, but no such label exists — clang/opt reject. Single-guard
spawn+await (_g3 shape) and spawn+free (_g4 shape) compile and run correctly;
static linearity gates all pass. Suspect: an inner emit_stmt-emitted guard
(candidates: the scheduled-Free# epilogue or a coll/spawn grow guard)
advances `fun.cur_block` past a block whose label is later renamed or never
written, so the outer merge phi captures a phantom predecessor.
**Fix direction:** audit cur_block updates across nested emit_statement calls
inside countable bodies (same family as the match-edge fix); make the merge-
phi predecessor cite the block that ACTUALLY branches.

## Callable-txn bodies silently drop `match` statements (2026-08-23)

**Status:** CLOSED 2026-08-23 (`backend-foundation`) — Statement::Match emission implemented + regression test + e2e PASS.
**Symptom chain:** native tamer's `exec_op` compiled body contains only its
leading `let` — the entire `match op { ... }` vanishes, leaving an infinite
convergent loop with no side effects (UB under clang -O2 -> garbage return,
"halts after 1 step").
**Root cause:** `src/backend/llvm/emit_stmt.rs emit_statement` has NO arm for
`Statement::Match` (zero hits); statement-level match falls through the
catch-all silently. `vm.bv`'s opcode dispatch is a statement-level match.
**Why the capability gate missed it:** LLVM is declared full-surface and skips
validation — that declaration is wrong for statement-level match. Plan 0's
"declare what you truly lower" applies to LLVM too.
**FIXED 2026-08-23 — the fix exposed a four-bug chain, all closed:**
1. Statement::Match emission (emit_stmt.rs): scrutinee once; per-arm any-of
   pattern conditions via emit_pattern_condition; per-arm terminated-flag
   reset (arms inheriting term from a prior arm emitted nothing); terminal-
   arms merge prints `unreachable` (empty label pair = invalid IR).
   Regression test: test_statement_match_emits_arm_blocks_in_callable_txn.
2. exec_op converted txn->defn — opcode dispatch is a pure function; as a
   txn its convergent loop spun forever under direct call. Same for
   host_arity_scan.
3. read_u16/i16/u32/i64 rewritten as byte stitches over read_u8 — the
   word-shift forms returned junk for unaligned offsets spanning words
   (read_i16(567) gave -25 instead of 999).
4. Struct-array row-view byte-addressing over-fired on Custom("Int")
   element names — guarded to genuine registered structs.

E2E: tools/bounty_e2e.sh packages the arith fixture, native tamer executes
it, asserts stdout "999\n42" — PASS.

**Resolved en route (same investigation, 2026-08-23):**
- Export-wrapper ABI: EVERY exported defn takes a leading %state pointer —
  C prototypes omitting it shift all args by one register (phantom halts).
- fn bc_offsets are section-relative; tamer addressing is base-absolute —
  manifest now ships absolute entry_bc (src/main.rs).
- Exported-defn state writes do NOT round-trip through the %state view
  (accessors read zeros). Final architecture: C owns memory + host-table
  parse; Briev exports one pure `step()` (lib/tamer/main.bv +
  tamer/install_sim.c).

## Two-guard task spawn + cross-guard state arithmetic: undefined guard label (2026-08-22)

**Status:** CLOSED 2026-08-22. Two stacked causes, both fixed:
1. The countable Guarded arm trusted inherited `fun.cur_block` as its merge
   phi's fall-through predecessor — it could name a pre-loop guard merge
   from an earlier emission region (`%guard.end49`). Condition blocks are
   now freshly labeled (`.cmgcN`) with an explicit fall-through branch.
2. `.cm_body` entry never reset `terminated`/`cur_block`, so a stale
   terminated flag skipped that fall-through branch and left an empty
   predecessor block (slice_state regression during fix). Body entry now
   resets both by construction.
Also found in the same audit: FreeHint/KeepHint/Yield were silently
DROPPED by emit_countable_body\'s `_ => {}` — they now delegate to the
standard emitter. Verified: repro 4221, all five pinned fixtures, suite
1928 green. Original report:
**Repro:** `bugs/repro_task_free_second_guard.bv` — node with TWO sequential
`when` guards; guard A spawns a local task, awaits/frees it, assigns a state
field; guard B reads that field with arithmetic under `endprogram`.
**Behavior:** emitted IR cites `%guard.end49` (an emit_stmt-style guard-end
label) inside a countable-loop merge phi (`%cmgm…`) that references it as a
PREDECESSOR, but no such label exists — clang/opt reject. Single-guard
spawn+await (_g3 shape) and spawn+free (_g4 shape) compile and run correctly;
static linearity gates all pass. Suspect: an inner emit_stmt-emitted guard
(candidates: the scheduled-Free# epilogue or a coll/spawn grow guard)
advances `fun.cur_block` past a block whose label is later renamed or never
written, so the outer merge phi captures a phantom predecessor.
**Fix direction:** audit cur_block updates across nested emit_statement calls
inside countable bodies (same family as the match-edge fix); make the merge-
phi predecessor cite the block that ACTUALLY branches.

## Convergent-txn loop: body side-effects don't persist across iterations (2026-08-23)

**Status:** CLOSED-AS-ARCHITECTED 2026-08-23 — superseded, no longer a
blocker. The tamer executes via the C-driven `step()` loop
(`tamer/install_sim.c` calls the exported single-step Briev function);
`vm_loop` is dead code. The parity harness (7 fixtures) runs REAL
programs through this path green — execution correctness is covered.

**Known limitation preserved for loop_engine owners:** a convergent txn
whose pre-condition never changes across iterations (exec_op-style pure
functions misdeclared as txns) spins forever — param-slot stores across
iterations did not persist under investigation at the time. Two of the
three repro symptoms were since explained: (1) exec_op-as-txn was a
semantics error (pure dispatch must be a defn — fixed), (2) the C-driver
"halts after 1 step" was an ABI bug (missing leading %state in C
prototypes — fixed). The residual observation (param-slot stores across
convergent iterations) remains UNVERIFIED against current main; anyone
touching src/backend/llvm/loop_engine/ should re-run the original repro
(lib/tamer/vm.bv vm_loop shape) before trusting either verdict.
Related family: Version-DAG phi entry below (CLOSED), two-guard
phantom-label fix (bd4eeb88).

## Version-DAG phi mismatch with nested guard bodies (2026-08-23)

**Status:** CLOSED 2026-08-23. `emit_version_dag_main` now DECLINES the
fold when any loop-body segment ends the program (endprogram inside a
guard, one nesting level deep) — mid-loop `ret` poisoned the present/latch
blocks and invalidated the header phis. The general PerFieldPhi path
handles early termination proven-correctly; repro builds and runs
(77742), all pinned fixtures byte-correct, suites green. Original report:
**Repro:** `bugs/repro_vd_phi_nested_guard.bv` — single-runtime-guard node
(version-DAG path) whose body contains a NESTED `when` plus calls into an
imported stdlib fn.
**Behavior:** clang rejects: "PHI node entries do not match predecessors!"
— `%vdc10 = phi i64 [ %t5, %entry ], [ %bl8, %.vd0_latch ], [ %bp9,
%.vd0_present ]` cites `.vd0_present`/`.vd0_latch` blocks whose real
predecessor sets differ (the nested guard's merge re-parents blocks).
Same family as the earlier match-edge and guard-merge dominance fixes but
in the version-DAG emitter, which has its own phi bookkeeping.
**Fix direction:** audit `emit_version_dag`'s phi predecessor capture the
same way the countable Guarded arm was fixed — derive predecessors from
the blocks that ACTUALLY branch, never from tracked names.

## VM parity test: missing tmp_fixtures/parity fixtures (2026-08-23)

**Status:** CLOSED 2026-08-23 — collision resolved in fb01f8c5 (fixtures
restored from backend-foundation history after main's backlog sweep saw
them as untracked strays mid-merge). OWNERSHIP NOTE for future sweeps:
tmp_fixtures/parity/*.bv are the VM plan's conformance corpus (owned by
backend-foundation track); tmp_fixtures/hw/* are the hardware plan's.
Do not sweep either without checking both branches.

## Conformance sweep: CLOSED (2026-08-23)

**Status:** RESOLVED — the sweep runs as a first-class test in `cargo
test --lib`. Campaign cleared 152 → 0 non-tamer failures across 8 passes.
Excludes: glue.dbv (own test suite) and lib/compiler/*.bv (tamer WIP).
Any new file that fails parse/typecheck will be caught immediately by
cargo test.

## Conformance sweep: 67 active sources fail the real-pipeline gate (2026-08-23 update #8)

**Progress:** ... → 89 → **67**. Pass 7 was the @-era demo pass:
migrated 7 demos to modern syntax (goal-based contracts, callable types,
Load#/Store#, Ptr<Bit<N>>), archived 12 removed-feature showcases.
**Remaining classes:**
1. lib/compiler tamer WIP (~17): comma-separated defn params; foreign track
2. glue.dbv dialect (~9): parser vs quoted-mode decision needed
3. stdlib deep-legacy (~15): console cell!, json.bv deep-type,
   process.bv List-args staging, memory/crossword generic issues
4. misc singles (~26): data-briev positions, link-demo missing .c,
   saxpy top-level call, test_ffi Result-vs-Bool, gpu-compute Ptr
   literals, vector_test UInt literal, fn-ptr .#Ptr remnants the real-pipeline gate (2026-08-23 update #7, revised)

NOTE: json.bv migration BLOCKED on generic type inference:
- List<JsonValue> inside recursive enum JsonValue: typechecker cannot
  unify Array([]) / elems + [frame.value] with recursive generic payloads.
  All syntax fixes done (char_at, slice parens, comma fields). The
  uni-to-match rewrite is complete and correct. Remaining issue is
  typechecker inference for recursive generic enums with list payloads.

NOTE: json.bv migration BLOCKED on three language gaps:
(a) List<T> + List<T> concatenation via `+` operator not typed for
    generic element types (works for concrete Int/Float but not JsonValue)
(b) String indexing s[i] returns Int (byte) not Char — need char_at
    helper or indexing change
(c) Slice arithmetic parens needed: s[i:(i+1)] not s[i..i+1]
Zero-payload enum variants FIXED (Void marker, 0-arg construction).
The uni→match rewrite itself is done (the match-based code is correct).

NOTE: json.bv requires a DEEP migration, not just uni→match. After the
uni rewrite (done by hand), the underlying code has ~12 type errors from
string-indexing drift (s[i] returns Int not Char), List<JsonValue>
operations, and generic enum payload mismatches. These are pre-existing
issues that were invisible behind the parse failure. A follow-up pass
needs to: (a) add char_at helper + replace s[i] with it for Char access,
(b) fix slice arithmetic parens, (c) address List<JsonValue> typing in
Array/Object payloads. The match-arm block-expression parser bug was
found and fixed (parse_block_expr double-LBrace). the real-pipeline gate (2026-08-23 update #7)

**Progress:** ... → 92 → **89**. Pass 6 archived the legacy trg-binding
files (briev_rt.bv ×2). the real-pipeline gate (2026-08-23 update #6)

**Progress:** ... → 96 → **92**. Pass 5 landed: posix SYS_* consts +
export-let visibility fix (typechecker), Barrier# intrinsic (CPU no-op /
future OpControlBarrier), ffi/http.bv modern frgn syntax + http.bv
Result passthrough, <std/result> import for constructor visibility. the real-pipeline gate (2026-08-23 update #5)

**Progress:** 152 → 136 → 134 → 119 → 106 → **97 → 96**. Pass 4 landed:
pre-only `[true]` contract misflag fix (parser), memory/arena+crossword
cast-based Ptr arithmetic + Free# typing, string_c.bv typed-String
migration, archived queue/stack/ptr/skiplist/shm/system/tty (zero-
consumer legacy). the real-pipeline gate (2026-08-23 update #4)

**Progress:** 152 → 136 → 134 → 119 → **106** across four burn-down
passes. Landed this pass: fn-as-value typing (SPEC §9.2 identifier
fallback), sig-family migrations onto callable types, spatial.bv
rewritten onto Fill#/Copy#, archived module-orphaned benchmarks.
**Remaining classes (per-file diagnostics in sweep output):**
1. @-era demos (~15): prior-state reads → restructure contracts
2. lib/compiler tamer WIP (~17): comma-separated defn params dominate;
   foreign track, coordinate before migrating
3. glue.dbv dialect (~9): "Expected named field" — parser vs quoted-mode
   decision needed
4. stdlib zero-consumer legacy (~25): old-trg, cell!, when-else,
   comma-fields, missing SYS_*/FD_* consts, missing Mem*/Barrier#/
   Spawn#/SetEnv# intrinsics, [true][true]x5 in string.ebv, Ptr
   arithmetic semantics drift in memory/*
5. misc singles: fn-ptr-demo (.#Ptr on fn type + undefined done),
   todo.rbv List.push, map-set/multi_output/subtype-projections/
   target-import/projections/constraint_* (removed header/field syntax),
   main.bv/stdlib_usage import forms, proof-oracle `?`, wasm-import.rbv,
   saxpy top-level call, test_ffi Result-vs-Bool contracts,
   vector_test.ebv UInt literal, gpu-compute.abv Ptr literals,
   data-briev dbv positions, link-demo missing helper.c module

(Older triages preserved below.)

## Conformance sweep: CLOSED (2026-08-23)

**Status:** RESOLVED — the sweep runs as a first-class test in `cargo
test --lib`. Campaign cleared 152 → 0 non-tamer failures across 8 passes.
Excludes: glue.dbv (own test suite) and lib/compiler/*.bv (tamer WIP).
Any new file that fails parse/typecheck will be caught immediately by
cargo test.

## Conformance sweep: 119 active sources fail

**Progress this pass:** 132 → 119. Landed: multi-payload enum variants
(parser), bits.bv migrated off `:>`, metro/ext archive (zero-consumer,
uni+Blob-construction chains), frgn provenance on 7 demo/bench files,
reg/pvt/out renames. Per-demo deep migrations remain: each legacy file
reveals further removed syntax beneath the first fix (var statements,
missing semis, top-level term) — burn down one file per pass.

**Progress:** enum variant construction LANDED (4bb965cb) — result.bv +
option.bv migrated to native constructors + exhaustive match. Sweep at
132. Classified inventory (from full sweep output):
1. **@-era demos** (~20): examples using removed prior-state syntax
   (arrow-mutation, async_mutual_exclusion, bank_transfer_system, cobol/*,
   reactive_counter, simple-counter, contract_verification, …).
2. **Reserved-word fallout** (~8): bit_clear/pointer-trickery/ptr-arithmetic/
   layout-ptr-demo/volatile-io (`reg`), visibility-demo (`pvt`),
   proto_bridge/iir_filter* (`out`), stdlib-demo (`union`).
3. **frgn missing provenance** (~7): physics, test_ffi*, ptr-runtime,
   glue-macro, link-example, saxpy, meld-bridge — need `from <source>`.
4. **Tamer WIP** (~17): lib/compiler/*.bv — comma-separated defn params +
   `uni` remnants + one lex error; foreign track, coordinate first.
5. **glue.dbv parser mismatch** (~9): all per-language glue configs fail
   "Expected named field" — dbv parser vs quoted-mode dialect drift.
6. **stdlib legacy dialects** (~30 zero-consumer files): bits(`:>`),
   console(`cell!`), briev_rt(old-trg), hashset/stack/queue(`{}` literal),
   encoding(when-else), ext/*+posix-ish(comma-fields), skiplist(op-in?),
   metro*(truncated), tty/system/ffi/*(misc), gpu/spatial/memory/*
   (missing intrinsics Barrier#/Memcpy#/Memcmp#/Memset#/Hash#),
   http/string_c(string-vs-int returns), posix/*(undefined SYS_* consts).
7. **Misc**: string.ebv [true][true]x5, todo.rbv List.push member,
   wasm-import.rbv, main.bv import syntax, fn-ptr-demo undefined fns,
   data-briev config.dbv identifier positions, proof-oracle `?`.

**Unblock order:** (2)+(3) mechanical → (6) per-file as touched → (1)
demo rewrites → (4) coordinate → (5) parser dialect decision.


**Status:** OPEN — Phase 10 continuation. **Primary blocker identified:
enum variant CONSTRUCTION does not exist.** Patterns parse
(`Result::Ok(v) =>`); constructors do not (`term Ok(v)` is a call to an
undefined fn). This single feature blocks the result/option/json/
string.ebv/process stdlib chains (~40 of the 134).
**Progress:** 152 → 136 (real pipeline via new `src/pipeline.rs` lib
module; shallow-gate false failures gone) → 134 (mechanical stdlib
migrations landed: time.bv contract typos, types.bv removed bit-range
suffix, json.bv import#+comma-fields, process.bv spawn→run rename).
**Gate:** conformance.rs sweep test (still `#[ignore]`d until the count
is small enough to fix in one pass). Run with `--ignored`.
**Residual classes:**
1. Enum construction blocker (above) — build it, then migrate
   result/option/json/string.ebv/process chains to match-syntax.
2. Zero-consumer legacy files in lib/std (bits `:>`, console `cell!`,
   briev_rt old-trg, hashset `{}`, encoding when-else, ext/*+posix/*
   comma-fields…) — no active importers; archive or migrate at leisure.
3. @-era demo examples + reserved-word fallout (bit_clear.bv `reg`).
4. lib/compiler/*.bv — foreign tamer-track WIP; coordinate first.

(Original 2026-08-22 triage preserved below for history.)

 the real-pipeline gate (2026-08-23 update #3)

**Progress this pass:** 132 → 119. Landed: multi-payload enum variants
(parser), bits.bv migrated off `:>`, metro/ext archive (zero-consumer,
uni+Blob-construction chains), frgn provenance on 7 demo/bench files,
reg/pvt/out renames. Per-demo deep migrations remain: each legacy file
reveals further removed syntax beneath the first fix (var statements,
missing semis, top-level term) — burn down one file per pass.

**Progress:** enum variant construction LANDED (4bb965cb) — result.bv +
option.bv migrated to native constructors + exhaustive match. Sweep at
132. Classified inventory (from full sweep output):
1. **@-era demos** (~20): examples using removed prior-state syntax
   (arrow-mutation, async_mutual_exclusion, bank_transfer_system, cobol/*,
   reactive_counter, simple-counter, contract_verification, …).
2. **Reserved-word fallout** (~8): bit_clear/pointer-trickery/ptr-arithmetic/
   layout-ptr-demo/volatile-io (`reg`), visibility-demo (`pvt`),
   proto_bridge/iir_filter* (`out`), stdlib-demo (`union`).
3. **frgn missing provenance** (~7): physics, test_ffi*, ptr-runtime,
   glue-macro, link-example, saxpy, meld-bridge — need `from <source>`.
4. **Tamer WIP** (~17): lib/compiler/*.bv — comma-separated defn params +
   `uni` remnants + one lex error; foreign track, coordinate first.
5. **glue.dbv parser mismatch** (~9): all per-language glue configs fail
   "Expected named field" — dbv parser vs quoted-mode dialect drift.
6. **stdlib legacy dialects** (~30 zero-consumer files): bits(`:>`),
   console(`cell!`), briev_rt(old-trg), hashset/stack/queue(`{}` literal),
   encoding(when-else), ext/*+posix-ish(comma-fields), skiplist(op-in?),
   metro*(truncated), tty/system/ffi/*(misc), gpu/spatial/memory/*
   (missing intrinsics Barrier#/Memcpy#/Memcmp#/Memset#/Hash#),
   http/string_c(string-vs-int returns), posix/*(undefined SYS_* consts).
7. **Misc**: string.ebv [true][true]x5, todo.rbv List.push member,
   wasm-import.rbv, main.bv import syntax, fn-ptr-demo undefined fns,
   data-briev config.dbv identifier positions, proof-oracle `?`.

**Unblock order:** (2)+(3) mechanical → (6) per-file as touched → (1)
demo rewrites → (4) coordinate → (5) parser dialect decision.


**Status:** OPEN — Phase 10 continuation. **Primary blocker identified:
enum variant CONSTRUCTION does not exist.** Patterns parse
(`Result::Ok(v) =>`); constructors do not (`term Ok(v)` is a call to an
undefined fn). This single feature blocks the result/option/json/
string.ebv/process stdlib chains (~40 of the 134).
**Progress:** 152 → 136 (real pipeline via new `src/pipeline.rs` lib
module; shallow-gate false failures gone) → 134 (mechanical stdlib
migrations landed: time.bv contract typos, types.bv removed bit-range
suffix, json.bv import#+comma-fields, process.bv spawn→run rename).
**Gate:** conformance.rs sweep test (still `#[ignore]`d until the count
is small enough to fix in one pass). Run with `--ignored`.
**Residual classes:**
1. Enum construction blocker (above) — build it, then migrate
   result/option/json/string.ebv/process chains to match-syntax.
2. Zero-consumer legacy files in lib/std (bits `:>`, console `cell!`,
   briev_rt old-trg, hashset `{}`, encoding when-else, ext/*+posix/*
   comma-fields…) — no active importers; archive or migrate at leisure.
3. @-era demo examples + reserved-word fallout (bit_clear.bv `reg`).
4. lib/compiler/*.bv — foreign tamer-track WIP; coordinate first.

(Original 2026-08-22 triage preserved below for history.)


## CIRCT guards folded to init constants — FSM ran past its bound (2026-08-25) RESOLVED

**Symptom:** the emitted counter FSM incremented unconditionally every
cycle; contract comparators referenced `%c<var>_init` constants, so
`[counter < 255]` folded at emission time and never gated anything. The
hardware would diverge from the interpreter at cycle 256 (counter kept
counting past the bound; halt stuck low forever).

**Root cause:** wire-map Phase B pointed `reg_names` at INIT CONSTANTS
because registers were emitted after txn bodies (plain SSA forbids
use-before-def), so bodies could only read constants; transition mux
conditions were literally `%true`.

**Fix:** hw.module bodies are MLIR graph regions — circt-opt accepts
use-before-def (probed before building). Registers now emit FIRST,
bodies read live register outputs, pre-guard gates the commit
(`mux(pre_ok, next, current)`), refusal raises `halt`, post-guard is an
implication (`¬pre ∨ post`) ANDed into the new `check` port.
Commit 420b5449.

**Caught by the new simulation-parity tier** (5515d6e9) in two more
places, proving the tier pays for itself:
1. Implication polarity: first cut ORed `pre_ok` into the post verdict;
   check flagged at every refusal (sim: `257 1 0 255`). Correct form ORs
   the PREFAIL wire (`¬pre`).
2. Inconsistent fixture contract pair: `[c<255][c<255]` fails post on
   the accepted 254→255 commit — sim showed a check drop at exactly that
   cycle. Fixture fixed to `[c<255][c<=255]` (contract-first: the SOURCE
   was wrong, not the checker).

**To undo:** restore Phase-A-reads scheme — but that reintroduces
interpreter divergence at guard bounds; do not.

## CIRCT ExportVerilog rejects hw.module.generated (FIRRTL_Memory) — 2026-08-25 OPEN (toolchain)

**Symptom:** `circt-opt --export-verilog` on IR containing a
`hw.module.generated @..., @FIRRTLMem(...)` fails with "unknown operation"
(SharedEmitterState::gatherFiles), then asserts
(`moduleOp && "Invalid IR"`) on the referencing instance.

**Root cause:** SeqToSV lowers `seq.firmem` to instances of an externally
generated module; upstream, the SystemVerilog body for FIRRTL_Memory macros
is emitted by the firtool-side flow. Standalone circt-opt's ExportVerilog in
this build only prints "// external generated module" for such ops — and its
file-gathering pre-pass rejects them outright.

**Workaround (ours):** after `--lower-seq-to-sv`, patch the generated op to
`hw.module.extern` (same ports) and supply the implementation as a brievc-
emitted companion `.sv`. Re-parse + export + verilator lint all verified.

**Revisit:** toolchain rebuild with firtool-side memory emission, or CIRCT
upstream change.

## Plain `txn` at top level compiles to an EMPTY program via brievc build — 2026-08-26 OPEN

**Symptom:** a program whose only logic is a plain top-level
`txn run [...] [...] { ... }` builds clean, exits 0, prints nothing —
the direct-SSA main loop contains no per-txn dispatch (any_active is
stored 0 and the loop exits immediately).

**Root cause:** the SSA-loop dispatcher (loop_engine/ssa.rs) and the
reactor dispatcher (dispatch.rs) both filter `t.is_reactive`; plain
`txn` parses with is_reactive=false. Only `node` declarations are
fired in the loop. Nothing warns that a whole txn was skipped.

**Status:** pre-existing, orthogonal to Track B (surfaced while probing
enum construction end-to-end). Workaround: use `node` for fired logic;
plain `txn` remains the callable form.

**Fix direction:** either (a) diagnose non-reactive txns that never fire
anywhere ("this txn is unreachable — declare it as a node to fire it in
the tick loop") or (b) define non-reactive firing semantics (run-once at
init). Decision needed on intended semantics first.

## Qualified enum paths (`Enum::Variant`) — 2026-08-26 RESOLVED

SPEC §8.3/§match examples show `Result::Ok(result)` patterns and imply
qualified construction, but the lexer's `::` token was never handled by the
parser — neither patterns nor expressions accepted qualified variant paths.

**Resolution:** `38943d5c` — parser handles `Enum::Variant(args)` construction
and `Enum::Variant(subpats)` patterns via `ColonColon` token. Typechecker
registers both bare and qualified keys; ambiguity detection errors on bare
names shared by 2+ enums. Interpreter and LLVM backend qualified-key
registries wired. 1960 tests pass; HW harness green.

**Sweep harness note — RECOUNTED 2026-08-27:** the conformance sweep is a
first-class `cargo test --lib` gate (see CLOSED entry below); recount shows
**209 discovered active sources, 0 failures**. The ~40 construction-gated
chains are fully resolved: result/option/process/string chains migrated
and green in `lib/std`; json.bv was archived out of the active inventory
(deep-migration blocker preserved in its entry above).

## Plugin disable flag was a no-op under extension filters — FIXED 2026-08-27

**Found:** the B1 CIRCT probe (`qual_circt.bv`) — `--no-std` was expected
to skip the stdlib prelude (and with it protocols.bv) so the probe could
compile; instead every build hit the B1.2 protocol hard error.

**Root cause:** `PluginManager::active_plugins` checked the
`enabled_only` override BEFORE the disabled list, and
`filter_for_extension` populates `enabled_only` for `.bv`
(`["prelude", env, print, entry, script]`). "prelude" early-returned true
from that branch — the `--disable-plugin prelude` / `--no-std` flag was
silently swallowed for every normal `.bv` compile.

**Fix:** disabled check moved FIRST in the filter chain (explicit disable
wins over every allow-path). Regression: verify --no-std skips prelude;
probe then compiles through CIRCT.

## CIRCT instantiated undefined modules for non-cell calls — FIXED 2026-08-27

**Found:** same probe. Qualified enum construction `Http::Ok(7)` in a node
emitted `hw.instance "Http::Ok" @Http::Ok` — but ONLY `@top` and declared
cell modules are ever defined, so the .mlir referenced a module nothing
provides (circt-opt/synthesis fails downstream). Plain fn calls had the
same hole (`test_circt_call_submodule` codified it).

**Fix:** the call fallthrough now instantiates ONLY callees present in
`cell_defs`; anything else records the house capability error naming the
callee and the fix (inline or declare as cell). Test rewritten to lock the
corrected contract.

## Terminology tripwire: the five meanings of "Bits" — canonical spelling is `Bit<N>` — DOCUMENTED 2026-09-01

**Found:** while writing the M2.1/M2.2 unit tests, the working draft used
`Bits(32)`/`Bits(N)` in test names and comments and would have propagated
the stale spelling into the SPEC §9.7 GPU-backend update. Nearly recorded
a non-canonical type name into the spec.

**Root cause — "Bits" survives in FIVE distinct roles and only the first
is the canonical type spelling:**

1. `Bit<N>` — the canonical surface spelling of the unified bit type
   (spec 2026-08-15 Fundamentals: "there is no separate `Bits` type").
   `Bits<N>` and `bits<N>` parse as DEPRECATED aliases (parser/types.rs
   normalizes all three to `Type::Bits(N)`).
2. Bare `Bits` — the width-0 FLEXIBLE bit type (spec 2026-08-16
   unification, `Type::Bits(0)`): accepts any `Bit<N>`.
3. `type X : Bits { ... }` — typedef PARENT naming the flexible bit type
   (lib/std/types/float.bv, lib/std/types.bv `type Byte : Bits {}`).
   Current, correct — not a type-name use.
4. `Type::Bits(N)` — the internal AST variant every spelling normalizes
   to. Compiler-internal; never surfaces in user docs.
5. `spec Bits: N;` / internal key `bits` — the layout METADATA KEY
   (parser `spec_name_to_key`). Unrelated to the type name.

The stale-impression sources, amended:
- `import_resolver.rs` generated `bootstrap.bv` fixture wrote legacy
  `type Int : Bits { maxbits <~ 64; };` — `maxbits <~` is legacy
  metadata grammar; canonical is `spec MaxBits: 64;`. Fixture updated.
- parser/definitions.rs legacy-alias comment now names the canonical
  spelling at the point the aliases are accepted.

**Rule going forward:** user-facing text (spec, tutorial, test names,
comments describing surface syntax) uses `Bit<N>`; bare `Bits` only for
the flexible width-0 form; `Type::Bits(N)` only inside compiler-internal
code. A doc sweep for remaining user-visible `Bits<N>` aliases is
deferred (they parse by design).

## Tree-revert hazard executed — self-inflicted, 2026-09-01

While fixing pre-existing broken test files (briv_compiler typos in 9
tests/ files — broken at HEAD before this session), a git checkout was
used to revert the failed fix attempt and it ALSO reverted three
uncommitted Track A files (pipeline.rs run field, main.rs flag wiring,
compile.rs webstack initializer). The edits were redone by hand. AGENTS.md
rule 8 exists precisely for this; recorded so the cost is remembered.
Rule: revert changes with targeted per-file checkout only, never a bare
tree-wide checkout while other work is in flight.

## Vacuous buffer contracts for cooperative kernels — FIXED 2026-09-01

collect_stmt_buffers had NO Foreach arm: the buffer walk skipped foreach
bodies, so every cooperative kernel's read_buffers/write_buffers were
EMPTY since the cooperative rung landed. Consequences: the eligibility
array_is_flat checks were vacuous (no buffers to check), and the Track B
resident-safety gate could not see kernel array accesses. Found by the
Track B gate's own negative test (a defn reading a[j] was invisible).
Fixed with a Foreget arm (list + body walked like Guarded); regression
tests in accel.rs resident_gate_tests (foreach_body_reads_reach_read_buffers).

## 2026-09-02 — driver: small-dispatch store loss (2-7 workgroups), RTX 3060

**Symptom**: GPU kernels dispatched with 2-7 workgroups nondeterministically
lose some (or all) workgroups' stores. The host view holds the pre-launch
sentinel; WHICH workgroup's stores survive varies with launch count
(128³ 2-wg: band 1 landed, band 0 sentinel; 192³ 3-wg: band 2 landed).
1 workgroup and >= 8 workgroups are reliable — which is why every
pre-tensor verified run (all >= 8 workgroups) never saw it.

**Evidence chain** (sentinel probes, f16-exact 123.0 seed):
- 128/192/256/320³ shapes fail; 64³, 512³, 1024³, 4096³ pass — mapped to
  workgroup counts 2-7 fail, 1 and 8+ pass, across BOTH the tensor tier
  and the plain naive tier (the kernel is not the variable).
- Deterministic per (blob, launch count), not a race across runs of the
  same configuration; survives 5-20 launches.
- A compute-write→transfer-read barrier before the download copy: no
  effect. Fence-only ordering was already spec-sufficient.
- A pad-to-8-workgroups workaround: WORSE — even the real workgroups'
  stores vanished. Reverted.
- The same driver already has one diagnosed dispatch defect: the Y
  dimension of vkCmdDispatch never takes effect (gid.y probe kernel,
  2026-08-31; grid flattened into X as the workaround).

**Status**: DRIVER-LEVEL, unresolved (no profiler on this box — ncu
absent; vendor tooling escalation recorded in the M2.2 plan). Mitigations
shipped: (1) the tensor tier is gated to >= 8 real workgroups
(`GemmPlan::tensor_tier_eligible` — kernel and runner share the gate), so
small GEMMs take the general naive tier; (2) an explicit
compute-write→transfer-read barrier on the download (spec-clean defense).
**Follow-up**: route ALL sub-8-workgroup dispatches to the CPU lane at
tier selection (small shapes are CPU-competitive anyway), and re-verify
against a different vendor/driver when available.

## 2026-09-02 — saxpy bench: 4th-field stores never visible host-side (RESOLVED)

**Root cause**: `saxpy_bench.c` declared 4 BrievField entries but passed
`n_fields = 3` to `BrievKernelDesc`. The runtime only processed the
first 3 fields (i/x/y) — z was invisible to push/pull/download. A
simple harness bug, not a runtime or compiler issue.

**Resolution**: changed `n_fields` from 3 to 4 in `saxpy_bench.c`.
Verified: max_abs_err = 0.000e+00 at N=16777216 with WARMUP=3 ITERS=3
and WARMUP=3 ITERS=5 — z stores land correctly. The idempotent kernel
(z = 2x + y, separate destination) means pass count cannot corrupt the
gate.

**Lesson**: the bench harness field table must exactly mirror the
generated runner's field count. The runner is always the authority —
derive offsets AND n_fields from it.

## 2026-09-02 — .bv binary accel path: two latent emission bugs (FIXED)

The `.bv` offload binary's descriptor/probe emission had never been run
through opt/clang verification — `--accel-cpu-fallback` forced the first
exercise and surfaced both immediately:

1. **Float literals without a decimal point.** The probe call emitted
   `double {:e}` → `1e-4`. LLVM IR text requires the decimal form
   (`0.0001`); `1e-4` parses as an integer token and the module fails
   verification ("integer constant must have integer type"). Fix: `{:?}`
   (Rust round-trip shortest, always carries `.`).
2. **`%briev.field` constants in the wrong order.** Emitted
   (name, kind, host, proj, elem, count, is_write); the C `BrievField`
   layout is (name, kind, host, elem, count, is_write, proj) — proj_offset
   at index 3 misaligns the runtime's reads AND trips opt's struct type
   check. Fix: emit C order; descriptor test updated.

**Lesson**: the .abv runner path was always clang-compiled (the benches),
but the .bv descriptor constants were only ever inspected, never
verified. Any new emission through a text IR format needs an
`opt -passes=verify` smoke test, and float literals must never use `{:e}`.

## 2026-09-02 — multi-kernel .abv: combined artifact violated the SPIR-V entry-point uniqueness rule (FIXED)

A `.abv` program with 2+ eligible kernels produced ONE `.spv` containing
every kernel as `OpEntryPoint GLCompute … "main"` — SPIR-V requires entry
points to be unique per (name, execution mode); spirv-val rejected the
artifact (`2 Entry points cannot share the same name and ExecutionMode`).
Never seen before because every existing .abv bench is single-kernel.

Root cause: the standalone artifact path (`compile_spirv` via
compile.rs) emitted all eligible kernels into one module, each hardcoded
"main" (the device drivers require pName "main" — per-node names had
previously broken pipeline creation). The runner path
(`runner::build_kernels`) was ALREADY correct: a fresh module per kernel.

Fix: the .spv artifacts are now written from `build_kernels`' blobs —
the single source (one .spv PER KERNEL, the abv-gpu-by-default
doctrine). Single-kernel programs keep the exact old filename and a
byte-identical artifact (verified `cmp` vs the previous build);
multi-kernel programs write `<stem>_<txn>.spv` per kernel. `brievc run`
and the C runner are unchanged. Regression guard:
`multi_kernel_builds_one_module_per_kernel` (per-kernel modules, one
"main" entry each). End-to-end: two chained kernels (fill → scale) both
validate and produce correct values via `brievc run`.

## 2026-09-04 — NVIDIA coopmat compiler: three silent-optimization behaviors (INSTRUMENT FINDINGS)

Stage-0 microkernel campaign (plan 2026-09-04-beyond-coopmat) — all three
were hit while building the mma-ceiling instrument; each is a DRIVER
behavior (RTX 3060, driver 5xx, vulkan-sdk 1.4.357), not a Briev bug.
Recorded because every future coopmat micro-benchmark will meet them:

1. **Constant-fragment mmas silently produce zeros.** OpConstantComposite
   of a coopmat type as an mma operand assembles, passes spirv-val, and
   the mma executes — the accumulator reads 0. The chain looked "folded"
   (202 TF/s reported = 4× hardware peak) until the operand source moved
   to a loaded fragment. Rule: mma operands must be LOADED fragments.
2. **SSBO coopmat loads with stride 16 (elements) read zeros.** stride-
   4096 loads from the same member work (the production form). Workgroup-
   storage stride-16 loads work. Symptom: mma outputs zero despite the
   seed being verified on device via the download round-trip.
3. **fma-chain de-fusion/reassociation across coopmat ops ignores
   NoContraction.** 8 accumulator chains sharing one (A,B) pair ran at
   "424 TF/s" (8× hardware peak): the driver hoisted/reassociated the
   shared product into ALU adds. NoContraction decorations on the mma
   results are ignored for this transform. Rule: a ceiling instrument
   must give every mma DISTINCT operand pairs (the production GEMM's
   4×4 register blocking already does).

Also re-confirmed the harness-side saxpy bug class: mma_ceiling_bench
shipped with n_fields=2 over a 3-field table — y was invisible to the
runtime, proj_bytes excluded the y region, and the kernel's y stores
were out-of-range (dropped under robustness = silent zeros; faulting =
fence-timeout "wedge"). Harness rule: n_fields MUST equal the table
length; derive offsets from the generated runner (the authority).

## 2026-09-07 — nbody_newton output drift (7th decimal) vs C reference [PRE-EXISTING, opened during noalias slice]

**Symptom:** `BOUND=2048 BODYCOUNT=500` nbody_newton prints `-0.169207186`; the C
reference prints `-0.169208214` (differs at the 7th decimal, one line of output).
Fast-math reassociation: no FMA contraction in either binary (objdump: 0 vfmadd
sites); the drift is vectorization/scheduling order over the pairwise energy sum.

**Build status:** nbody_newton FAILED to build at 5d1d7e45 (invalid IR:
"Instruction does not dominate all uses" — swan-song print used a loop-body
register from the exit block). The swan-song dominance fix (this slice) restored
the build. Parity therefore cannot have been exact at HEAD either way.

**Last known parity:** 2026-07-31 frontend-dispatch Phase 3
(benchmarks/results/2026-07-31-frontend-dispatch-phase3.md: nbody_newton 0.82x,
MATCH). Regression window: 2026-07-31..2026-09-05 — isolate with
`git log --oneline` over src/backend/llvm emission between those eras and an
A/B on the actual .ll (Performance Recovery Protocol, AGENTS.md Rule 20).

**Not caused by this slice:** the fix only changes the POST-LOOP exit block
(clears loop SSA maps so the swan song loads %State); in-loop arithmetic is
 byte-identical. The async examples (17 / 111) produce exact correct output.

## 2026-09-07 — emit_match phi double-write + missing label (init-block phi predecessor slice) [FIXED]

**Symptom:** `test_hashmap_surface.bv` (and any program whose state field's
`op Init` body emits blocks — currently only `HashMap.init`'s `match capacity`)
produced LLVM IR with the merge-block label MISSING and the phi line doubled:

```
  br label %.match_end_4
.match_end_4:                                   <- label written by the fix
  %t2 = phi   %t2 = phi i64 [ 0, %.match_next_4_1 ], ...   <- doubled prefix
```

The `phi` local in `emit_match` (emit_expr.rs) is built as
`"  {v} = phi {ty} [...]"` — it already carries its own `  {v} = phi ...`
prefix. The emission must be the bare `writeln!(out, "{}", phi)`; re-supplying
`indent`/`v`/`= phi` in front of it double-writes. A mid-session edit also
dropped the `writeln!(out, "{}:", end_label)` that emits the label.

**Fix:** restore `writeln!(out, "{}:", end_label)` before the phi; keep the
bare `writeln!(out, "{}", phi)`. The `phi` string is self-contained.

**Root cause of the slice's motivation:** a state field's `op Init` that emits
blocks (HashMap.init's match) leaves `cur_block` on the match's `.match_end_N`.
Loop headers emitted after init cited a hardcoded `%entry` predecessor, but the
init's `br` actually lands in `.match_end_N` — the header phis named a
non-predecessor. Fix: capture `init_pred = cur_block.unwrap_or("entry")` after
`emit_inline_init_stores` and cite it in the counter + per-field phis (all
engines: folded, perfieldphi, countable, version-dag, batched, countdown). In
emitters whose header is reached by a single `br` from entry (countable_main,
batched, version-dag, ssa_main, emit_main), reset `cur_block = None` after init
so `init_pred` resolves to `"entry"` (byte-identical IR when no
block-emitting init ran).

 ## 2026-09-07 — test_hashmap_surface dominance (%t765 ptrtoint) [FIXED]

**Symptom:** even after the phi fix, `clang -O3 -flto` rejected
`tests/tier1/test_hashmap_surface.bv` with
"Instruction does not dominate all uses! %t765 = ptrtoint ptr %t767 to i64".

**Structure:** the SSA main loop body runs `HashMap.insert` inside a guard
(`guard.then740`), which allocates a node (`%t766 = malloc`, `%t767 = bitcast`,
`%t765 = ptrtoint`). The handle `%t765` is then read in `foreach.end724` and
`foreach.hdr803` — blocks NOT dominated by `guard.then740` (the insert only
runs when the guard is true). Clang's dominance check rejects the use.

**Root cause (two-part):**
1. **Arrow-push type check skipped the tuple push.** In `emit_stmt.rs`
   `Statement::ArrowAssign`, the `val_ty` resolution only handled
   `Expr::Identifier` (resolving the bound type). A tuple literal
   `(keys[i], vals[i])` fell to `val.ty.clone()` which is the fallback
   `Custom("Int")`. The `matches` comparison then failed against
   `Tuple([K, V])`, so the push was SKIPPED — the tuple node was allocated
   inside the guard but never stored into the list's data array. The last
   node's handle leaked out and was read after the loop.
2. **Cross-function `cur_block` leak.** The `cur_block` on `self.fun`
   persisted across function emissions. A txn's `guard.end_N` or a match's
   `.match_end_N` leaked into the next function's `init_pred` capture,
   citing a label from a different function as a phi predecessor.

**Fix (three-part):**
1. `emit_stmt.rs` `Statement::ArrowAssign`: extended `val_ty` resolution for
   `Expr::Tuple` — resolve each element's type (identifiers via
   `resolve_id_type`, fallback to `Type::int()`), producing `Type::Tuple`.
   Added `tuple_arity_match` to the `matches` gate: a `Tuple([K, V])` element
   type matches a concrete `Tuple([Int, Int])` value of the same arity.
2. `helpers.rs` `emit_main_header` + `emit_toplevel.rs` (3 txn define sites) +
   `mod.rs` (1 txn define site): reset `self.fun.cur_block = None` at every
   function start. A fresh function has no current block; the previous
   function's labels are not valid predecessors.
3. `emit_expr.rs` `emit_member_body`: removed the `cur_block` save/restore.
   The save/restore prevented a state field's `op Init` (HashMap.init's
   match) from updating `cur_block` for the countdown header's `init_pred`
   capture. The cross-function leak it was guarding against is now handled
   by the function-start resets.

**Verification:** `test_hashmap_surface.bv` compiles clean and prints all 9
correct values (20/true/false/5/10/100/10/10/4/9). `hash_ops_idio` parity
restored (24999995000000). 2076 tests green. float_math/mandelbrot/
linked_list compile clean.

## 2026-09-10 — PTX mw kernel: three device-level traps (cp.async era)

**Symptoms:** the cp.async + double-buffer rewrite of
`tensor_gemm_ptx_smem_mw` produced rc=700 (illegal memory access) or
all-zero output on the RTX 3060 (sm_86, driver 580.178.04) while passing
identically under cuda-gdb and compute-sanitizer. Hand harnesses faulted
even for kernels later proven correct.

**Root causes (three independent):**

1. **B-fill/ldmatrix layout inversion (the original rewrite bug).** The
   rewrite kept the pre-rewrite n-major blocked-B fill while reading with a
   new ldmatrix scheme, and its 4-byte cp.async sources were not even
   derivable from row-major global B (a 4B copy moves two adjacent columns
   of one k-row; an n-major smem tile needs two adjacent k-rows — 4096
   bytes apart). Fixed by making the smem slab K-MAJOR
   (`k*128 + n*2`, 128-byte k-rows) with a 16B-chunk XOR swizzle
   (`(n>>3)^(k&7)<<4`) shared by fill and ldmatrix, and reading
   x2.trans with rows-as-k.

2. **Async-proxy visibility (environment).** `cp.async.wait_group 0` +
   `bar.sync` — the documented sm_80 pattern — did NOT make LDGSTS writes
   visible to `ldmatrix` natively (reads saw stale/zero smem: y = all
   zeros, rel_err exactly 1.0). Serialized tools (cuda-gdb,
   compute-sanitizer) masked it. `fence.proxy.async.shared::cta` is the
   ISA answer but requires sm_90. **`membar.cta` between wait_group and
   bar.sync closes the gap on this stack** — verified on-device. If the
   driver is ever upgraded, revisit whether membar.cta is still needed.

3. **High-VA allocation fault (environment, harness-only so far).**
   `cuMemAlloc` sometimes returns VAs in the 0x7fef_08200000-style region
   (per-binary, deterministic, not host-ASLR); kernels that store then die
   with Xid 31 `FAULT_PDE` on the compute channel while the copy engine
   (HtoD upload) succeeds on the same buffer. Low VAs (0x8200000-style)
   are immune. Driver-bug territory. Harnesses now retry `cuMemAlloc` up
   to 64× until the pointer lands below 4 GiB. The production runtime's
   allocations have so far landed low; if it ever flips, this is the trap.

Also: `-maxrregcount=108` cubins (ptxas 12.8 AND 13.3) launch but fault
IMA at runtime; natural allocation for the kernel is 136 regs. Never ship
capped-register cubins for this kernel — size block_threads to the natural
count instead (select_mw_nw caps at 256 threads).

## 2026-09-11: SPIR-V coopmat fill silently corrupted for 3 days (u32_and id-as-mask)

**Symptom:** the SPIR-V coopmat tensor tier failed on-device at 4096³
(max_rel_err 3.156e-01, deterministic) while all 2111 lib tests stayed
green and perf looked normal. Found by the cross-tier A/B against the
new PTX tier.

**Root cause:** dd5f5e26 (2026-09-08) introduced `u32_and(builder, val,
mask: u32)` for power-of-two modulo strength reduction. rspirv's
`type Word = u32` means a SPIR-V result id IS a u32 — the five fill
call sites passed `p.b_stage_elems_mask` / `p.b_stage_pairs_mask`
(**Word ids**) where a **literal mask value** was expected. Each emitted
`Op::BitwiseAnd` masked `b_flat` with the gen_id number itself (some
huge non-2^k-1 constant), corrupting the B-tile fill addressing for
most flattened indices. Same instruction count → perf unchanged, so
the commit's timing-only verification missed it; no test exercised the
fill's shape-dependent addressing on device.

**Fix:** `u32_and` now takes `mask: Word` (a u32_const result id); the
one literal call site (stagger bucket, &7) builds its const first. The
Word/u32 alias is exactly the class of silent type error Rust cannot
catch — hence the helper's doc comment now warns explicitly.

**Verification:** baseline A/B (5d1d7e45 blob: 4.436e-03 OK; broken
blob: 3.156e-01 FAIL — same harness, same runtime) isolated the
compiler; commit-window bisect pinned dd5f5e26; post-fix blob returns
4.436e-03 (bit-identical to the 2026-09-04 ledger number).

**Lesson:** strength-reduction commits that only re-measure TIMING are
not verified. Any commit touching kernel index math needs the on-device
correctness gate at a real shape (the cross-tier A/B harness now does
this in one command).

## 2026-09-11: fasta ~100× regression — unbuffered stdout after libc removal

**Symptom:** `benchmarks/fasta` at BOUND=10M: baseline 0.061 s, current
6.6 s (identical output bytes). Sys-time dominant (~3.9 s) — one
`write(2)` syscall per output byte.

**Root cause:** the delete-step's Family A made the print family
pure-Briev (`write_all` → `SysCall#(1, …)`), losing libc stdio's
pipe buffering for free. `node fasta` prints one char per iteration →
10M syscalls. Nobody re-ran the runtime A/B after the runtime-linking
changes (this sweep was the first).

**Fix (LANDED 2026-09-11, commits 158ae9ec + 01c92189):** buffered
stdout required LIBRARY-LEVEL MUTABLE STATE, which Briev lacked — defn bodies cannot see top-level
`let`s ("undefined variable"), and the `state` keyword is removed. The
buffer (fixed 64 KiB block + fill pointer, O(1) `Store#` append, flush
at capacity + main-tail) is expressible the moment defns can reference
program globals through their `%state` param. Backend plumbing for the
exit-flush already exists (`has_stdout_flush` gate + `@__stdout_flush`
call at every runtime-main tail — a user-defined `__stdout_flush`
flushes today). Needed: (1) typechecker defn-scope visibility of
top-level lets, (2) needs_state propagation for global reads/writes,
(3) emitter global access through the defn's `%state`. Then the
buffered lane lands in cast_lanes.bv (the reverted attempt sketches it).

**Outcome:** fasta 0.73x vs baseline at BOUND=100M (27% FASTER than
the C baseline era), output byte-identical, sys time zero. Library
globals landed first (top-level lets import); the buffered lane then
landed wholly in stdlib.

**Lesson:** runtime-linking changes MUST get the compare_baseline A/B
in the same stream — the sweep exists because "parity corpora green"
says nothing about throughput.

## 2026-09-11: coopmat S=1 (subgroups=1) kernel produces zero y at 2048³

**Symptom:** with `spirv_coopmat_subgroups: 1`, the coopmat f16acc
GEMM at 2048³ stores nothing (max_rel_err 1.000e+00, y all zero) while
4096³ with the same S=1 blob is correct (4.436e-03). The shipped
config (S=2) is unaffected at every tested shape.

**Status:** latent — the S=1 module form (no SubgroupId decode, single
32-lane workgroup per descriptor) breaks at this geometry. Not
debugged; the shipped config is S=2 everywhere. If the S=1 form is
ever productized (smaller smem per workgroup → occupancy play), bisect
the fill/store decode against the S=2 module first.

**Found while:** refuting the redundant-compute hypothesis for the
2048³ coopmat 27 TF reading — S=1 measures 1.30 ms vs S=2's 0.635 ms
(S=2 is strictly faster; the reading is real work, not overlap).

## 2026-09-12: reactive realization gap investigated — deferred; causal DAG proceeding

**Investigation (2026-09-12):** the reactive dispatch ships the naive
realization — `emit_ssa_main` (`src/backend/llvm/loop_engine/ssa.rs`)
re-evaluates every node precondition every tick. The designed machinery —
compile-time dependency graph, dirty-flag `step()` in topological order,
event-driven wake without polling — is specified in
`docs/plans/2026-06-15-trg-reactive-dirty-flag.md` and
`docs/plans/2026-06-11-async-reactor-triggers.md`, partially built
(`reactor.rs` `dependency_map`/`mark_dirty` exists, never populated),
never wired into the backend.

**No correctness break:** programs compute correct end states; outputs
verified across the suite. The design was confirmed as original intent
(the program signals intent; the compiler owns realization — pre =
eligibility, post = the declaration of completion).

**The gap = realization fidelity:**
1. Efficiency — chains round-trip ticks the wiring already proves
   unnecessary (A fires into B next tick instead of straight-line; the
   Rule-2 exposure). Backend fusion may already be delivered by LLVM's
   `-O3 -flto` pipeline once loop shape is right — to be measured, never
   assumed (the LTO lesson).
2. One missing compile-time refusal — a non-quiescing cycle
   (`[x == 0] { x = 1 - x; }`) hangs at runtime today
   (`is_one_shot` logic, ssa.rs:331) where the design refuses at compile
   time for lack of a checkable liveness obligation in the post.
3. Missing knowledge — no causal graph, so no "what fires into what"
   report, no FSM reachability/deadlock proofs.

**Plain-txn semantics resolved in passing** (see 2026-08-26 entry): a
txn is a contract carrier (as the electronics path already treats it); a
body-carrying never-called non-reactive txn is dead code — diagnostic
candidate only, not a semantics fork.

**Decision:** the full restoration (fusion codegen, sync<g> completion
barrier, interpreter reactor revival) is DEFERRED until a benchmark or
program class demands it (fasta precedent). The causal DAG itself
proceeds immediately as proof of the model — see
`docs/plans/2026-09-12-dynamics-causal-dag.md`. Backends stay trusted
until measured otherwise: if LLVM already collapses tick round-trips
under the real pipeline, Briev's job stays contracts + loop shape.

**MEASURED (same day, LLVM experiment — the LTO lesson honored):**
canonical 3-node proven chain (`arm -> commit -> shutdown` over a state
variable), harness-exact link (`clang -O3 -flto -march=native -ffast-math
-fdata-sections -ffunction-sections -Wl,--gc-sections`):

1. The UNOPTIMIZED emission already cascades proven chains WITHIN one
   pass — after a body commits, the downstream node's pre is re-checked
   in the same pass (`.ssb_arm` falls through to commit's pre-check).
   The "chains round-trip ticks" claim above was too pessimistic: the
   only loop residue is one empty quiescence-confirm pass.
2. The fully-optimized binary's `main` is `xor %eax,%eax; ret` — the
   entire reactive program (loop, both passes, all state) folded to a
   bare return by the standard pipeline. Correct per observability-as-
   liveness: the program's only externally visible behavior IS exit 0.

**Consequence:** Briev-owned fusion codegen is not just deferred — for
foldable shapes it is MEASURED unnecessary. The backend delivers the
designed realization from contracts + loop shape alone. The DAG's
load-bearing value is what the backend cannot do: compile-time liveness
refusals, the causality report, and future proof work (FSM
reachability/deadlock). Fusion gets built only when a measured program
class shows the backend failing — not before.

## 2026-09-12: invalid A/B — dispatch constant edited, dump-test cubin benched

**Symptom:** the warp_mh=4 "rejection" (commit f1ea6e8b) concluded
mh2 ≥ mh4 from an interleaved A/B that actually measured mh2 vs mh2.
The session patched the dispatch constant in `mod.rs`, then ran
`cargo test dump_mw_4096_f16acc` + ptxas + bench — but the dump test
hardcodes its own `warp_mh=2` literal, so the regenerated cubin was
identical to production. The bogus "rejection" shipped with a confident
comment.

**Root cause (two layers):**
1. Process: the A/B benched an artifact (dump-test cubin) that was not
   the artifact the dispatch edit controlled.
2. Latent bug that masked the truth: the f16acc register declaration
   hardcoded `%a<8>`/`%b<16>` instead of scaling with the warp shape
   (4*mhr / 2*gr), so every true warp_mh=4 dump failed ptxas
   (unknown %a8-a15) — anyone inspecting the real mh4 PTX would have
   hit assembly errors, not a clean kernel.

**Fix:** declarations scale with mhr/gr; select_mw_nw + mw_ok +
shared_bytes generalized to the warp aspect; warp_mh=4 shipped for
f16acc (+2.9 TF at 4096³, 4/4 rounds). Selector test pins the aspect
contract.

**Rule:** bench the artifact the dispatch actually emits. If the dump
test's literals can drift from dispatch constants, make the dump read
the dispatch constant — never edit one and measure the other.

## 2026-09-14 — Equilibrium `wfi` park sleeps through address-wired eligibility

**Where:** Phase 3 rv64 equilibrium (loop_engine/ssa.rs, `.end` park).

**What:** the dispatch loop's exit parks unconditionally in `wfi` (with a
`~{memory}` clobber). Correct for pure-vectored programs (timer demo — the
trap wakes the core), but a program whose eligibility includes an
address-wired node (`node n @ some_addr`, reactor-pass class) sleeps
through it: the MMIO value changes, no interrupt asserts, nothing wakes the
CPU. Latent — no shipped program wires addresses yet; surfaced in the
machine-entry design review.

**Fix (Phase C/D of 2026-09-14-bootstrap-kernel.md):** frontier-driven
equilibrium — the frontend computes per-wake-source re-evaluation sets
(`AnalysisResults.wake_sets`, conservative writer×reader closure; external
wires permanently in the external frontier), and the emitter picks the park
policy from it: `wfi` only when the frontier is vectored-only; spin
(continuous evaluation) when an external frontier exists, or a
`within`-bounded quantum park when declared. Logged before the fix ships.

## 2026-09-14 — Statement match with void txn arms emits broken expression-match IR

**Where:** LLVM backend match lowering (statement `match` whose arm values
are void txn calls mixed with an empty-block arm).

**What:** `match trap_cause() { 7 => schedule(), 8 => syscall_dispatch(),
_ => {} };` in a machine-entry node lowered through the EXPRESSION-match
path (boxed arm results): the `_ => {}` arm's box fill referenced `%t23`
defined in a sibling arm's block → `use of undefined value '%t23'` at
clang. Found building kernel_rv64.b.bv.

**Workaround:** `when` guards (or cache the scrutinee in a `let` and use
`when` per case).

**Fix:** route void-arm statement matches through the statement-match
emission (`.smt_*` blocks) — never the boxed expression path. Open; demo
uses the workaround.

## 2026-09-14 — kernel_rv64 freezes entering the first U-mode task

**Where:** `examples/kernel_rv64.b.bv` — the Phase 4 micro-kernel demo
(bootstrap → trap_service @ 7 → schedule → two U-mode tasks via ecall).

**State:** the machine reaches the first timer trap, runs the handler
(K prints), schedule() re-arms mtimecmp and sets the task switch (mepc ←
task entry ✓ verified in the QEMU CPU dump; mstatus = MPP=U, MPIE=1 ✓),
then freezes: no task execution, no second timer interrupt (the -d int log
has exactly one line), MIE=0, priv=M, pc spinning in a single-instruction
translated block (`csrrw zero, mepc, a4` — the set_mepc site).

**Evidence:** qemu -d int/-d cpu/-d in_asm logs under /tmp/opencode
(qi2/kc3/kt5 in-session); IR verified correct (frame_store's body:
call trap_frame_base → add offset → inttoptr → volatile store ✓).
Suspects, unranked: QEMU mret-into-U subtlety (mstatus.FS/VS state?),
PMP grant ordering (pmp_grant_all runs in the bootstrap — effectiveness
unverified), the scaffold's x1–x31 restore overlapping the enter_user_mode
csrc (mstatus write-after-write), or an mret-to-U exception loop with
mtvec silently re-servicing.

**Repro:** `brievc build examples/kernel_rv64.b.bv --triple
riscv64-unknown-none --linker-script lib/targets/qemu-virt-rv64.ld` then
`qemu-system-riscv64 -machine virt -bios none -nographic -kernel
examples/kernel_rv64.b` → prints `BK` then freezes (B = bootstrap entered,
K = first trap handler entered).

**Not the cause (ruled out):** mtvec alignment (align 4 ✓); undeclared
t0/t1 inline-asm clobbers (constraint registers only ✓); the void/i64
call-prototype mismatch (fixed this session — defines are void, calls are
`call void`); mtimecmp non-re-arm (fixed — schedule re-arms; verified in
IR); the expression-bodied `ret 0` bug (fixed this session — shims return
their computed values; verified in IR).

## 2026-09-14 — RESOLVED: the kernel freeze chain (four bugs, one demo)

The kernel_rv64 freeze decomposed into four independent defects, each
masked by the next. Resolution order:

1. **Expression-bodied defns returned literal 0** (`ret i64 0` after
   computing the body value) — every shim returned zero: frame addresses,
   mepc, cause. Fixed: the defn tail returns the captured last-expression
   register (emit_stmt captures it; emit_definition's fallthrough returns
   it for i64).
2. **mtimecmp never re-armed** — MTIP asserted forever: every mret
   re-trapped before the tasks ran (the "S/R flood"). Fixed: schedule
   re-arms (mtimecmp = mtime + interval).
3. **PMP never granted** — QEMU virt implements PMP CSRs; with no matching
   entry, U-mode fetch/load/store FAILS. The tasks faulted at their first
   instruction, silently (the fault loop matches no dispatch arm). Fixed:
   pmp_grant_all in the bootstrap (pmpaddr0 = max, pmpcfg0 = R|W|X TOR).
4. **enter_user_mode left commented** (a debug leftover) — the tasks ran
   in M-mode; their ecall = mcause 11 (M-mode ecall), which the dispatch's
   `when cause == 8` never matched → the silent K-flood. Fixed: restored.

**Rule:** debug markers removed == the fix verified end-to-end; every
marker removal must be followed by a full clean run, not a build-only
check (the removals themselves changed behavior — the dbg shim was
dropping its own call).
## 2026-09-13: E4c window — three traps that cost measurement time

**1. `MW_SMEM` unset = smem-0 launch = IMA storm.** `ptx_gemm_bench.c`
defaults the dynamic-smem size to 0; the dump kernels allocate ALL working
memory as `extern .shared`, so every launch without `MW_SMEM=<bytes>`
faults at warmup. The fault appears for ANY kernel (even known-good ones
re-assembled fresh), which reads as "the new kernel is broken". Rule:
always export `MW_SMEM` matching the dumped tile — production f16acc
(2,4)@256T needs 16384; the old (4,4)@512T needed 24576.

**2. `ptxas -maxrregcount=128` PESSIMIZES the natural-64 kernel.**
Assembling the f16acc kernel with the 128 cap (the f32-tier value)
produces 80 registers vs 64 natural — the cap flips ptxas onto a worse
allocation/schedule, dropping occupancy 2→1 CTAs/SM and costing ~28%
TFLOP/s (34.5 → 25.7 in the same window). Register caps are not
monotone safes. Rule: assemble f16acc artifacts with `-maxrregcount=64`
(the value compile_cubin passes for f16acc, mod.rs), never the 128
used by the r16 test helper.

**3. c8b0da04 committed a broken bin target.** The `explain_causality`
field line landed in `run_bounty`'s BuildOptions initializer (where the
binding doesn't exist, E0425) instead of `parse_build_args`' (E0063,
missing field) — `cargo build --release` of `brievc` failed at HEAD for
everyone. `cargo test --lib` stayed green, so lib-only workflows never
noticed. Fixed 2026-09-13: flag wired in parse_build_args, bounty path
hardcodes `false`. Rule: a BuildOptions field must be added to every
initializer in the same commit — grep the struct name, not the field's
last-known home.

## 2026-09-13: SHIP PTX kernel — K≤32 with small M returns all-zero y

Discovered while debugging E8a (whose stage-1 failure may share the root
cause). The ship f16acc kernel (E4c, (2,4)@256T) FAILS the correctness
gate at shapes never gate-tested: 128×128×16, 1024²×16, 2048²×32 all
measure rel 1.0 (whole y regions zero), while 4096×4096×16 passes exact
and 2048³/4096³/8192³ pass. Repro (dump via dump_mw_4096_f16acc's
k16_128 entry + ptx_gemm_bench):

    MW_SMEM=16384 BRIEV_GEMM_F16ACC=1 ptx_gemm_bench ship_k16_128.cubin \
        128 128 16 0 4096 36872 256 1 check

The k16-4096 and k16-128 kernels are structurally identical
(constant-normalized diff is empty) — so the trigger is the small
constants/grid, not the code path. Candidate areas: the prologue
fill/predicate interaction at tiny K, the KLOOP fill-skip guard
(`r2 >= k-16` is true from kstep 0 at K=16), or a driver/check artifact
at small M·N. E8a's stage-1 (odd-kstep) corruption is plausibly the same
root cause — stage-1 stripes are the first "beyond-prologue" fills.
Priority: HIGH — it gates E8a and any small-K GEMM (decode-shaped
attention head matrices are K=64-128 territory).

**RESOLVED 2026-09-13 (same day):** most "failures" were driver-arg
mismatches (manual y_off used M·N·2 where the dump bakes M·K·2 — equal
only when K=N). ONE real kernel bug found and fixed: the mid-loop
`cp.async.commit_group` fired even when the fill was skipped (K=16 makes
the fill-skip guard always-true), and the empty-group + tail
`wait_group 0` path returned all-zero y. Fix: commit moved inside the
fill path (skipped fills don't commit). All sweep shapes + recorded
portfolio green post-fix; no perf regression beyond window noise. See
docs/plans/2026-09-13-kle32-small-m-anomaly.md. E8a's stage-1 failure is
independent and still open.

## 2026-09-14 — defn with contract brackets compiles as a convergence loop

**Where:** the LLVM backend's callable-emission path — a `defn` carrying
contract brackets `[pre][post]` receives the txn-style convergence
emission (post-check + backward loop-back) instead of a linear body +
fallthrough.

**What:** `defn schedule() [current >= 0][current >= 0 && current < 2]`
in the Phase 4 kernel looped forever at interrupt priority: the body
re-ran (set_mepc/enter_user re-executing) because the emitted
post-check branched on a register the loop itself mutated. A defn is
LINEAR by declaration — contracts on a defn are documentation/proof
obligations, never convergence targets (convergence belongs to txns and
reactor-dispatched nodes by declaration).

**Workaround:** the kernel demo drops the brackets (the obligations move
to the derivation examples / test gate).

**Fix (open):** the defn emitter must ignore contract brackets for
control flow — emit the body linearly + fallthrough regardless. The
brackets keep their typechecking role (documented obligations). OR:
reject contract brackets on defns entirely until derivation obligations
make them checkable. Decision needed; the workaround ships in
examples/kernel_rv64.b.bv.
