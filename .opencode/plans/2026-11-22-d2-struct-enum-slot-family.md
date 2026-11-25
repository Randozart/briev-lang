# D2: the struct / enum / slot value-family (six issues, four root causes)

Date: 2026-11-22. Worktree `/scratch/briev-builds/wave5` (branch `wave6`),
target dir `/scratch/briev-builds/wave5-target`.

Predecessor: `2026-11-22-float-width-seam.md` (D1, landed as main `14a911c08`).
This plan lands the six open issues the owner confirmed for this round:
#152, #156, #157, #158, #159, #160, plus harness stage 0 and closing #148.

## The four root causes (research summary, evidence in `## Provenance`)

- **A — struct values are "an i64 stack address": no value semantics, no
  lifetime** (#158, #159). A struct local is `alloca i64`; `let t: S = s`
  stores the *parameter's* address (alias, not copy). Reassignment builds the
  struct on a nested block's stack temp and returns `ptrtoint` of it; append
  does the same into `List<struct>`. After the function returns the frame is
  dead → the caller reads garbage (`140267378403200`, `bad a=98 b=94898917470224`).
- **B — one hidden enum sret slot is shared by every call site** (#157).
  `step`'s own hidden sret parameter `%hsret0` is passed to *both*
  enum-returning calls; the second call overwrites the first → `BBBB/BBBB`
  instead of `AAAA/BBBB`. Per-call slots `%esrt0/%esrt1` exist but are unused.
- **C — a local's alloca is deferred to its first write** (#152).
  `HashMap.get` (`let r: V = 0 as V;`) emits the `alloca` inside `guard.then107`
  while the later `load` sits after `foreach.end68` → `llvm-as`:
  "Instruction does not dominate all uses!" A minimal loop probe without the
  member-init form emits correct entry-block IR.
- **D — context types missing at two points** (#156, #160).
  `#160`: `emit_slice_with_array` String branch folds `end: None` to `hi = 0`;
  the heap-List branch in the same function already uses the length word, and
  the canonical door `str_len_bytes` exists (`cast_lanes.bv:515`). `#156`: the
  checker admits `Box { xs: [] }` but the struct-literal field value is never
  contextualised with the declared field type → deliberate panic guard at
  `emit_expr.rs:2456`.
- **0 — harness defect class** (not an issue): the 5 c_driver FAILs under
  `-j 4` are parallel-load flakes (shared `--out`-style temp dirs racing a
  build; same class as trunk `a53db50da`), not regressions — gate #2 on an
  idle machine was 4613/4613 green and serial `-j1` was 5/5.

## Stage order

0 → D → C → B → A. Each stage: `bash scripts/gate.sh quick` green → its own
commit; stage-end of D2: full gate + `ll_determinism_check.sh --quick` → push.

## Stage 0 — c_driver harness hardening + BUGS.md entry (no issue closes)

- Apply trunk `a53db50da`'s cross-process claim pattern (atomic `create_dir`,
  released on drop, 300 s deadline) around the five c_driver temp dirs
  (`briev_cancel_test`, `briev_c_driver_test`, `briev_py_node_bridge`,
  `briev_boundary_test`, + needs_state dir) in `tests/c_driver_*.rs`.
- BUGS.md: one entry — "c_driver gates race their own temp dirs under
  cargo-nextest parallel load", with the gate #1 (5 FAIL, `-j 4`) vs gate #2
  (4613/4613 idle) + serial 5/5 evidence, and a pointer to the fix.

## Stage D — contextualization gaps (#160, #156)

- **D1 (#160)**: in `emit_slice_with_array`'s String branch, an absent `end`
  resolves to the string's byte length via the canonical door `str_len_bytes`
  (same semantics as the heap-List branch at ~line 3382: "hi absent = through
  the end"). Never fold to 0.
- **D2 (#156)**: a struct literal's field values are contextualised with the
  *declared field type* (the same path a `let`-site with a typed RHS uses), so
  a bare list literal in a field position constructs through the
  collection's ops instead of hitting the panic guard. The guard stays.

## Stage C — local slot materialization dominance (#152)

- Find the pass/phase that defers a local's `alloca` to its first write (grep
  HIR passes: deferred-store / let-insert kinds, incl. `owned_storage`
  `LetInsert` from K3 Phase C).
- Fix: the slot is materialised at the declaration (entry block, or the
  block that dominates every read), or the pass proves the write dominates
  all reads before deferring. Target the `let r: V = 0 as V;` member-init form
  from `lib/std/collections.bv` (`defn get`) — the reproducing shape.

## Stage B — per-call-site enum sret slot (#157)

- An enum-returning call gets a fresh `enum_out` temp allocated in the entry
  block (or a block dominating the site). The enclosing function's hidden sret
  pointer must never be reused as a callee's sret slot. Both unwraps must read
  their own call's payload.

## Stage A — struct value semantics: owned heap object + copy at hand-off
(#158, #159)

Representation decision (owner-confirmed): **(b) owned heap object + copy at
hand-off**. (c) destination-copy stays recorded as the follow-up optimisation.

- A struct value is an owned heap object (`Alloc#`-class allocation through the
  normal ABI), not a frame-local stack temp addressed as an i64 word.
- Hand-off points that must copy the value, not alias it:
  1. `let t: S = <expr>` — copy the expression's object into `t`'s owned slot.
  2. Reassignment `t = S{...}` — build the new object, copy into `t`'s slot.
  3. `return t` / `term t` — copy out (or hand ownership per the existing
     return ABI) before the frame dies.
  4. `list.append(t)` into `List<struct>` — copy the value into the
     collection's storage; the element must survive the caller's frame.
- **Rule 20 pre-build A/B**: before writing code, generate the current `.ll`
  for a struct-heavy fixture and at least one candidate lowering; verify the
  candidate is well-formed (dominance, no dangling) and measure. A refuted
  hypothesis blocks the fix.
- Undo path: the change is additive to the struct emission path; reverting the
  stage commit restores the i64-word representation exactly.

## Invariants

Each is pinned by BOTH a native golden fixture (`tests/tier1/*.bv` + `.golden`)
and, where the shape is expressible in the reference interpreter, a SPEC §8.11
`test { }` block. Native and interpreter checks do not substitute for each
other.

1. **I-160**: an open-ended string slice runs to the end:
   `"abcdef".s[3:] == "def"` (and the closed form is unchanged).
   Pinned by `tests/tier1/str_open_end_slice.bv` golden + `test {}` block.
2. **I-156**: a struct literal with an empty (or mixed) list field compiles
   and round-trips: `Box { xs: [] }` builds, `xs` reads as empty, a populated
   `xs` round-trips. No panic at `emit_expr.rs:2456`.
   Pinned by `tests/tier1/struct_list_field.bv` golden (+ interp if expressible).
3. **I-152**: a local initialised inside a guarded/nested body and read after
   the enclosing loop is valid IR (`llvm-as` clean) and holds the assigned
   value. Shape: the `HashMap.get` member-init form from `collections.bv`.
   Pinned by `tests/tier1/nested_local_dominance.bv` golden (+ interp if
   expressible).
4. **I-157**: two enum-returning calls in one function keep their payloads
   separate: the `step` repro prints `AAAA` then `BBBB`.
   Pinned by `tests/tier1/two_enum_calls.bv` golden (+ interp if expressible).
5. **I-158**: reassigning a struct local inside a guard and returning it
   yields the new value (repro prints `2`, not the dangling i64).
   Pinned by `tests/tier1/struct_reassign_return.bv` golden (+ interp if
   expressible).
6. **I-159**: appending a struct built locally to `List<struct>` and reading
   it back after the producer returns yields the stored value (repro:
   `good a=7 b=2` behaviour for the `add_bad` shape).
   Pinned by `tests/tier1/struct_append_survives.bv` golden (+ interp if
   expressible).
7. **I-0 (harness)**: the five c_driver suites pass under the full
   `cargo nextest run --workspace -j 4` gate (no shared-temp-dir race), and the
   BUGS.md entry documents the defect class. Pinned by the full gate run
   itself; no language fixture.
8. **Determinism**: `bash benchmarks/ll_determinism_check.sh --quick` → ALL
   DETERMINISTIC at each stage boundary (the new emission must not depend on
   HashMap iteration order).

## Named tests

- `tests/tier1/str_open_end_slice.bv` (+ `.golden`) — I-160.
- `tests/tier1/struct_list_field.bv` (+ `.golden`) — I-156.
- `tests/tier1/nested_local_dominance.bv` (+ `.golden`) — I-152.
- `tests/tier1/two_enum_calls.bv` (+ `.golden`) — I-157.
- `tests/tier1/struct_reassign_return.bv` (+ `.golden`) — I-158.
- `tests/tier1/struct_append_survives.bv` (+ `.golden`) — I-159.
- Interpreter `test { }` blocks: added to the matching `.bv` where the shape is
  expressible in the reference interpreter (to be confirmed per issue during
  GRIND; if a shape is not expressible, the native golden alone pins it and
  the plan records why).
- `tests/c_driver_*.rs` — I-0 (harness), verified by the full `-j 4` gate.
- `tests/cast_lane_symbols.rs` unchanged (D1 lane guard still green).
- Existing corpus: `bash scripts/gate.sh full` at D2 end;
  `benchmarks/ll_determinism_check.sh --quick` at each stage boundary.

## Verification per stage

- `bash scripts/gate.sh quick` (nextest, warm ~1–2 min) green before each commit.
- `bash benchmarks/ll_determinism_check.sh --quick` at each stage boundary.
- `cargo build --release` no new warnings; copy the fresh `brievc` into
  `target/debug/brievc` so fixture tests don't run the stale binary.
- `praetor validate --warn --target <changed dir>` (directory, never a file).
- Full gate (`bash scripts/gate.sh full`) at the end of D2, then
  `git fetch` + `git push origin HEAD:main`, CI green.

## Issue bookkeeping

- Close #160, #156, #152, #157, #158, #159 as each stage lands (one close per
  stage commit).
- Close #148 in this round (already fixed by W7-1 `2f4174a97`).
- #151 already fixed (`f6095f09b`) — no action.
- Interpreter cast-lane prelude gap (BUGS.md OPEN) remains out of scope unless
  the owner asks.

## Provenance / evidence (do not delete; rewrite if refactored)

- Repros at `/tmp/probes/p152 p156 p157 p158 p159 p160/` (each `.bv` + `.ll`).
  Minimal loop probe at `/tmp/probes/p152b/` (emits correct entry-block IR,
  narrowing C's trigger to the member-init form).
- IR evidence:
  - #152: `p152.ll:1252 %t117 = alloca i64` inside `guard.then107`; `:1261
    %t123 = load i64, ptr %t117` after `foreach.end68`; `llvm-as` reports
    "Instruction does not dominate all uses!".
  - #157: `step(ptr align 8 %hsret0, ...)`; `%t12 = call i64 @field_str(ptr
    %hsret0, ...)` and `%t24 = call i64 @field_str(ptr %hsret0, ...)` share the
    slot; `%esrt0/%esrt1 = alloca i64, i64 2` are the unused per-call slots.
  - #158: `step(i64 %arg0)`: `%t0 = alloca i64; store i64 %arg0`; guard
    `then10`: `%t13 = alloca i8, i64 16`, `%t22 = ptrtoint ptr %t13 to i64`,
    `store i64 %t22, ptr %t0`; ret loads `%t0` → dead frame.
  - #159: `add_bad`: `%t1 = alloca i8, i64 16`, `%t6 = ptrtoint ptr %t1 to
    i64`, appended into `List<struct>` param.
  - #160: `emit_expr.rs:3233-3245` String branch `end:None → hi = add i64 0, 0`;
    correct heap-List branch at ~`:3382`; canonical door `str_len_bytes`
    (`lib/std/cast_lanes.bv:515`, `lib/std/cast_portable.bv:524`).
  - #156: `emit_expr.rs:2456` deliberate panic guard.
- Harness: gate #1 log `/tmp/gate-merged.log` (5 c_driver FAIL under `-j 4`);
  gate #2 log `/tmp/gate-merged2.log` (`Summary [1689.723s] 4613 tests run:
  4613 passed (4 slow), 11 skipped`, idle machine); serial `-j1` 5/5 PASS.
  Trunk precedent: `a53db50da` (OnceLock per-process race → cross-process
  claim).

## Undo path

- Stage 0: revert the `tests/c_driver_*.rs` claim change + BUGS.md entry.
- Stage D: revert the two small emitter/context changes (String slice `end:None`
  door; struct-literal field contextualisation).
- Stage C: revert the slot-materialization pass change.
- Stage B: revert the per-call `enum_out` allocation.
- Stage A: revert the owned-heap struct emission; the i64-word representation
  is restored exactly (no data migration).
