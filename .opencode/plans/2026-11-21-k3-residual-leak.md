# K3 residual leak — owned argument temporaries (2026-11-21)

Status: **DONE** (compiler `1f9f3c823`, in `main` `b39c151a0`; kiosk `88233f5`).
Follows K3 Phase C (`4caace291`, leak 101.4 → 56.6 KB/call).
Result: 56.6 → 42.0 KB/call and 1,148 → 904 allocs/call on the same geometry
(−58.6% cumulative from 101.4). The remaining 42 KB/call is tensor/enum
payload lifetime, a different mechanism — own plan (see “Out of scope”).

## Why this plan exists

Phase C closed the multi-candidate *selector* leak. The remaining
~56.6 KB/call in ASan was labelled "one-time tensor lifetime" and left.
Re-reading the ASan log with per-object math disproves that label: every
bucket's object count divides evenly by the 30 calls (51, 52, 53 per call),
i.e. every bucket accumulates once per request.

## Retraction (IR-proven)

An earlier reading claimed a "double-free family" in the cast stdlib:
`Alloc#(n)` → build `[len][bytes]` into `buf` → `(buf as Data) as String` →
`Free#(buf)` → `term owned`.

That reading is **wrong**. `(buf as Data) as String` compiles to a copying
owned-return door:

```llvm
%t18_p = call ptr @malloc(i64 %t19)              ; staging (Briev Alloc#)
%t55   = call ptr @briev_bits_to_str(ptr %t58)   ; briev.owned_return  (COPIES)
call void @free(ptr %t62)                         ; frees staging — correct
ret ptr %t55                                      ; caller owns the copy
```

The staging `Free#` is correct at all 11 `; briev.owned_return` sites.
**No stdlib change is made in this plan.** Verified by
`grep -c 'briev.owned_return'` on the emitted IR (≥11) and by
`cast_copy_door_keeps_staging_free` (below).

## Real root cause

An **owned temporary passed as a call argument is never released**:

```llvm
%t102 = call ptr @briev_str_substr(ptr %t99, i64 %arg1, i64 %t76)  ; owned copy
%t95  = call i64 @append_str(i64 %arg2, ptr %t102)                  ; borrowed arg
ret i64 @parse_str_chars(...)                                       ; %t102 leaks
```

`uses()` (`src/hir/owned_storage.rs:2806`) modes each argument of a call,
but the *return value* of a nested call used in a Borrow/Consume position
is neither owned nor released — it has no name for the scope-end sweep to
release. Structurally identical to the Phase C selector-loser ("unnamed
owned storage with no release anchor"), so the fix reuses the landed
`HirStmtKind::LetInsert` pure-insert primitive.

## Stages

| stage | content | gate |
|---|---|---|
| R0 | 600-call soak on the current fixed daemon (both ingress paths) | decide whether the residual accumulates in RSS at all |
| R1 | two-point ASan attribution (10 vs 40 calls) to confirm which buckets scale and by how much per call | names the sites the fix must cover |
| R2 | generalize `LetInsert`: bind owned argument temporaries in Borrow/Consume argument positions to a pure-insert temp owned by the enclosing scope | IR destroy count grows; census releases grow, drops do not |
| R3 | re-measure (ASan 30-call + 600-call soak), reconcile census, land | all invariants below |

R0 exists because ASan's exit report counts arena-reset blocks as
reachable: a flat 30-call RSS curve at 176 MB cannot resolve a 56 KB/call
(0.9%) signal, and an ASan bucket alone does not prove accumulation.
A 56 KB/call leak over 600 calls is ~34 MB — unambiguous.

## Invariants

1. **The cast copy door is untouched.** Every `; briev.owned_return` site
   keeps its paired staging `Free#`. Pinned by `cast_copy_door_keeps_staging_free`
   and `grep -c 'briev.owned_return'` ≥ 11 on the daemon IR.
2. **Every owned argument temporary is released exactly once.** For a call
   argument in a Borrow/Consume position whose expression is an owned
   temporary (fresh-call result, list literal, or owned cast), the pass
   binds it to a `LetInsert` temp and the enclosing scope releases it.
   Pinned by `owned_arg_temp_to_borrowing_param_is_released`.
3. **A consuming parameter is not double-freed.** An owned temp passed to a
   parameter that the callee consumes must not also be released by the
   caller. Pinned by `owned_arg_temp_to_consuming_param_is_not_released`
   (negative test).
4. **No new counted HIR statements.** `LetInsert` stays a pure insert
   (`is_pure_insert`), so the HIR/AST pair walk stays aligned and no body
   falls back to plain AST emission. Pinned by the IR destroy count (a
   fallback zeroes it) and `owned_insert_keeps_pair_walk_aligned`.
5. **No use-after-free / double-free.** ASan 30-call run completes with no
   SEGV and no double-free report. Pinned by the ASan soak artifact.
6. **Census does not regress.** `releases` grows; `drops` does not grow;
   `undecided` stays 0. Pinned by `harness/memcheck_ratchet.sh`.
7. **Behaviour is preserved.** Interpreter corpus parity + `cargo test --lib`
   + `scripts/gate.sh quick` green.

## Named tests

- `owned_arg_temp_to_borrowing_param_is_released` (new, unit) — `f(g(mk()))`
  where `g` borrows its param → `mk()`'s result gets a release.
- `owned_arg_temp_to_consuming_param_is_not_released` (new, unit) — `f(free_or_store(mk()))`
  where the callee consumes → exactly one free, by the callee.
- `owned_insert_keeps_pair_walk_aligned` (new, unit) — a body with an owned
  arg temp still yields a non-empty HIR-sourced emission (no AST fallback).
- `cast_copy_door_keeps_staging_free` (new, unit/golden) — the cast owned-return
  lane keeps `briev_bits_to_str` + `free(staging)`.
- `multi_candidate_selector_releases_inline_candidates`,
  `nested_selector_does_not_double_free_inner_result` (existing, must stay green).
- `harness/memcheck_ratchet.sh` (kiosk), `harness/native_smoke.sh`,
  `scripts/gate.sh quick`, ASan 30-call + 600-call soak.

## Measurement protocol

- R0/R3 soak: `harness/soak_rss_truth.sh` with `SOAK_PARAM=frame_path` and
  `image_path`, 600 calls, sample every 50. Verdict: slope KB/call from
  RssAnon between call 100 and call 600.
- R1/R3 ASan: exit-patched daemon
  (`call i32 @exit(i32 0)` at the `serve` tail + `declare i32 @exit(i32)`),
  `ASAN_OPTIONS=detect_leaks=1:log_path=…`, 10 and 40 calls; a bucket is real
  only if `(objs(40) − objs(10)) / 30` is a positive integer matching the RSS slope.

## Out of scope (own plan)

`run_node` Result enum boxes (~0.85 KB/call) — enum-payload ownership is a
different mechanism from argument temporaries.

## R2 record (2026-11-21)

Mechanism: `mode_for_arg` (extracted from `uses()`, one decision point with two
consumers) asks the escape summary whether an argument position is Borrowed.
`hoist_owned_arg_temps` then walks each callable, and for every Borrowed
position holding an OWNED temporary (`is_owned_temp_expr`: a fresh call, or a
list literal) it binds the value to a pure-insert `LetInsert` BEFORE the
statement and releases it AFTER — top-level in the body and inside loop bodies,
where the per-iteration bind/free pair is what keeps it correct.

Release kind follows the value's type. A list literal has no collection type of
its own in the HIR (the typechecker stamps one only at the source let site), so
`hoist_value_type` derives `List<elem>` from the first element and that
derivation WINS over the literal's own stamp. Two failure modes this prevents,
both observed during the first daemon build:

1. The stamp can be the element type or the declared parameter type, which sent
   the construction to the bare-operand path — and a bare list literal has no
   codegen shape (`emit_expr` panics by design).
2. The stamp can be a String, which chose a `free()` release for a collection
   handle.

A literal whose element type cannot be named is NOT hoisted: it keeps leaking
exactly as it does today. Never miscompiled.

Census (kiosk daemon, `harness/memcheck_ratchet.sh`): releases 80 → 103,
conservative drops flat at 2221, undecided 0, static-backed flat at 16. The
23 added releases carry no drop inflation — the Phase C accounting invariant
holds for the general case too.

## Trunk blocker found during R2 (not this plan's work)

Post-merge trunk `3bf2fc375` cannot compile the kiosk daemon: 41 contract
refutations, all the same shape — a local assigned a provable value on one path
and a call result on the other loses its fact at the join
(`let c = 0; when … { c = chan(i); }; get(8, c)`). Pre-merge `4caace291`
accepts the identical sources. Recorded in `BUGS.md`; the R2 measurement runs
against `4caace291` + this patch in `/scratch/briev-builds/k3-baseline-trunk`
so the ownership change is isolated from the checker regression.

### R2b scoping (measured, 2026-11-21)

Hoisting is restricted to COLLECTION temporaries. A String argument temporary
is deliberately NOT hoisted, because naming it does not survive the emitter:
the binding round-trip records the value in `let_binding_types` and the ordered
argument adapter then converts a register the callee already takes directly —

```
%t20 = call ptr @proto_bytes(i64 %arg0)
%t24 = inttoptr i64 %t20 to ptr        ; invalid IR: %t20 is already ptr
%t22 = call ptr @owned_string(ptr %state, ptr %t24)
```

observed in `parse_attr_loop` of the kiosk daemon (`owned_string(proto_bytes(r))`).
A String temporary therefore keeps leaking exactly as it does before R2 — no
regression, no miscompilation — and the fix belongs in the String binding /
adapter path (bind the DECLARED type, not the register's claimed type, and let
the read path answer `ptr`). That is R2b, its own measured increment.

Consequence for the numbers: R2 removes the collection half of the residual.
The ASan buckets that are String-shaped (`briev_bits_to_str` 225,856 B / 712
allocs per 30 calls) stay until R2b.

## R3 measurements (2026-11-21)

Geometry for every number below: kiosk daemon built from the SAME sources, one
raw f32 CHW frame (640x640), `yunet.onnx`, `detect_faces` over the unix socket.

### ASan two-point (same builder, same links)

| build | 10 calls | 30 calls | slope |
|---|---|---|---|
| Phase C only (control) | 1,416,099 B / 14,490 allocs | 2,627,923 B / 37,451 allocs | **60,591 B/call**, 1,148 allocs/call |
| Phase C + R2 | 1,221,599 B / 11,630 allocs | 2,062,540 B / 29,711 allocs | **42,047 B/call**, 904 allocs/call |

−30.6% bytes and −21.3% allocations per call, on top of Phase C's −44.2%. From
the pre-K3 101.4 KB/call that is **−58.6%** cumulative.

The control run reproduces the previous session's 30-call total to 0.05%
(2,626,752 vs 2,627,923 B) — the instrument is stable, so the difference is the
patch, not the measurement.

Attribution: `conv2d`, `transpose4` and `maxpool` allocation sites still leaked
in the Phase C control and are **absent** from the R2 report; `briev_bits_to_str`
is unchanged (it is the copying cast door, whose staging free is correct).

### IR, same module

| | coll_destroy | free | malloc | lines |
|---|---|---|---|---|
| control | 40 | 56 | 361 | 38,347 |
| R2 | 47 | 62 | **361** | 38,372 |

Releases grow, allocations do not move: the hoisted expression is evaluated
once, never duplicated.

### RSS soak (the staircase problem)

Both builds show the same staircase: flat stretches, then 5–29 MB steps at
unpredictable call counts — and the steps go DOWN as well as up (one recorded
run of this daemon rose 142.7 → 159.0 MB over 300 calls, then fell to 151.5 MB
by call 350 and settled at 154.4 MB). A curve that returns memory cannot
produce a slope; a leak can only add. `MALLOC_ARENA_MAX=1` does NOT remove the steps, so
they are not per-arena retention — but they are large enough to swamp a
~18 KB/call difference inside any 600-call window. The staircase is why the
30-call soak in the Phase C record looked dead flat while ASan showed 56 KB/call
accumulating: RSS on this daemon is a coarse instrument.

Both builds under `MALLOC_ARENA_MAX=1` over the same 400 calls measure
41.2 KB/call (control 173,696 → 188,100 KB RssAnon; R2 151,200 → 167,664 KB) —
identical, because each is one or two step events divided by 400, not a slope.
The one RSS reading that IS consistent and meaningful: R2's absolute footprint
is **~20 MB lower at every sample** (call 50: 151,200 vs 173,696; call 400:
167,664 vs 188,100). Less live heap is exactly what more releases should buy.

So: the per-call verdict rests on the ASan slope (two points, same builder,
reproducible to 0.05%), with RSS corroborating the lower absolute footprint and
refusing to resolve an 18 KB/call difference — never as the primary instrument.

## Geometry note

Every number above is the `frame_path` ingress (raw f32 CHW). `image_path`
(JPEG through the stb decode) was not measured: this checkout has no JPEG with
a face in it, and decoding a face-free image would exercise a different work
path than the one the leak class lives in. The mechanism is
ingress-independent — it fires on any owned temporary in a borrowed argument
position — so the slope is reported once, on the ingress with a real subject.
