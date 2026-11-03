# ws_frame safe fast path — execution plan — 2026-11-03

**Status: ACTIVE.** Companion findings file (evidence, IR diff, refuted
experiment, proof definitions): `2026-11-03-ws-frame-safe-fast-path.md`.
Owner decision 2026-11-03: cover **all steps** (0–3); investigate any
additional bugs found along the way.

## Goal

Recover `ws_frame` baseline speed — and where profitable, beat it — by
**deriving** the fast shape from proofs at every site where it is legal.
Never a benchmark-specific match arm (Rules 2/23/24). Zero tolerance for
"probably fine": every optimization is gated on a proof, every proof on a
measurement.

## Background (one paragraph)

Baseline `42279aaf` ran `ws_frame` at ~0.0100s because enum returns pointed
into dead callee frames (UB; removed after `server_protocol_gate`/
`server_loop_gate` crashes — see findings file history table). Current tree
copies every enum image into the caller slot (`retain_enum_return`) with a
runtime source==dest elision: 16 branch sites, 32 `ret_copy` labels vs
baseline 0. The 7-bit path already threads `%enum_out` (hits `ret_same`);
the 16/64-bit paths (2/3 of corpus frames) birth images in fresh `esrt`
allocas and copy at return. Separately, `ws_partial`'s `malloc(34)` could be
eliminated by the existing object caller-destination proof, blocked by the
whole-enum boundary gate — which is a **self-imposed** predictability
obligation, relaxable where its real safety conditions hold.

## The three proofs (definitions — full text in findings file)

- **Proof 1 — returned-image ownership.** (a) Birth-site threading: a
  `find_sret_call_lets`-eligible `let r = f(...)` may receive the enclosing
  function's `%enum_out` when the slot is free (`enum_out_busy`) and image
  liveness proves no two live images; (b) return-path dataflow: every `term`
  yields `%enum_out` (or an ownership-transferring heap box). (a)∧(b) ⇒
  emission omits `retain_enum_return` statically. Never provable: dead-frame
  pointers (UB).
- **Proof 2 — object consumption.** Implemented in `object_repr.rs`; blocked
  for `Ok(WsFrame)` by `object_payload_is_inline` = false (boundary gate).
  Unlocks `ws_partial` caller-destination (no malloc) — **beats baseline**.
- **Proof 3 — boundary three-condition proof** replacing the blanket gate:
  1. **layout agreement** — every image-materialization site reads the same
     `EnumLoweringPlan`/`EnumLayoutPlanner` entry (no legacy-boxed fallback
     divergence);
  2. **image lifetime ownership** — the enum handle cannot outlive its image:
     escape into longer-lived storage (state, aggregate, closure, unknown
     call, return) requires the image be heap-owned (`own_enum_value`) first.
     This is the recorded I11 hazard (`BUGS.md`: "escape without a match
     binding, carrying the inline object pointer through the enum handle");
  3. **no external raw-byte consumer** — not exported/FFI-visible,
     reflection-serialized, or otherwise read by a party outside the plan.
  Boundary-crossing itself creates none of these obligations; the current
  gate (`enum_layout.rs:333`) is a conservative proxy for all three.

## Steps

### Step 0 — Root-cause the I11 gate-relaxation corruption

*Why first-class:* two recorded probes (2026-11-01/02) relaxed the gate
with PR51 owned extraction and I11 still aborted `malloc(): invalid next
size`. Proof 3 cannot ship until this escape path is found and either
proved impossible under new conditions or fixed.

1. Read the probe records in full (`BUGS.md` I11 entry, plan Phase E,
   `2026-11-01-canonical-enum-payload-and-selective-inline.md:100-111`).
2. Enumerate candidate escape paths for an inline-payload enum handle:
   handle stored to `let` state / collection / object field / closure /
   passed to unknown or FFI callee / returned from a txn node — each
   without `own_enum_value` first. Map to actual emitter sites
   (`emit_stmt.rs` state stores, `own_enum_value` call sites).
3. Layout-divergence alternative: grep every `image_words`/`malloc(bytes)`
   site (evidence in findings file) for paths that bypass
   `EnumLoweringPlan` (legacy boxed fallback in `enum_lowering.rs` join).
4. Reproduce under a gate: relax gate behind an env flag (temporary,
   `// TEMP:` provenance per Rule 16), run I11/ASan, bisect the escape by
   logging image alloc/free + handle stores (ASan use-after-free with
   `detect_stack_use_after_return=1` first).
5. Record root cause here + BUGS.md; the fix becomes a Proof 3 condition
   (or a precondition bug fix, if it's a plain ownership bug independent
   of the gate).

**Exit criterion:** named escape path (or refuted: gate relaxation safe
under stated conditions — then conditions must encode why).

### Step 1 — ws_frame attribution (measure before building)

*Why:* static IR math (~11K executed 2-word copies + branches) may not
explain the 0.0057–0.0100s gap; harness variance on the identical tree was
0.0157 vs 0.0200. The 10-29 forwarding experiment showed "no win" without
explaining where time went — do not repeat that.

1. Interleaved A/B ×5: `bash benchmarks/compare_baseline.sh ws_frame`
   (protocol already pins baseline); record all five ratios in a dated
   results file.
2. `perf stat` + `perf record` (or `valgrind --tool=callgrind` if perf
   unavailable) on current binary: instructions, branches, branch-misses,
   cache misses; attribute time to `ws_decode`/`ws_decode_mask`/retain
   sites vs payload walk (`ws_payload_byte`) vs node/txn overhead.
3. Dynamic copy count: count executed `ret_copy` (not `ret_same`) — e.g.
   temporary counter instrumentation or perf probe on the copy blocks.
4. Compare against baseline binary under the same counters.
5. Write `benchmarks/results/2026-11-03-ws-frame-attribution.md`: gap
   decomposition (copies / branches / other), and the sizing verdict for
   Proof 1: expected recovery in ms.

**Exit criterion:** a number — "Proof 1 can recover at most X ms of the
Y ms gap" — and the dominant cost named. If copies are a minor fraction,
Step 2 scope shrinks (or the gap is declared noise with evidence) and we
say so before writing emission code.

### Step 2 — Proof 1: image ownership threading

*Depends on:* Step 1 verdict (proceed/scope). *Additive only* (Rule 6):
fresh-alloca + copy fallback stays for every unproved site.

**Analysis (frontend, `src/analysis/`):**

1. New facts in the destination-ownership family (extend
   `destination_model.rs` or a sibling module — no inline analysis in
   codegen, Rule 10):
   - `image_slot_free_at(defn, let_site)`: the enclosing defn's result slot
     holds no live image at the call site (seed from `enum_out_busy`
     semantics; liveness: intermediate image dead before next slot write;
     never two live images in one slot).
   - `returns_destination_image(defn)`: every `term` yields the incoming
     sret slot handle or an ownership-transferring heap box (dataflow over
     term expressions; conservative join at merges).
2. Both facts consume existing structures (`find_sret_call_lets` shapes,
   `term` forms) — no type names, no callable names (Rule 15).

**Emission (`src/backend/llvm/`):**

3. `emit_toplevel.rs` sret-slot selection: when
   `image_slot_free_at` ∧ caller has `enum_out_slot` ∧ callee
   `returns_destination_image` ⇒ pass `enum_out_slot` instead of hoisting
   `esrt` (currently `find_sret_call_lets` unconditionally hoists).
4. `helpers.rs::retain_enum_return` / `emit_stmt.rs:2045`: when
   `returns_destination_image` holds for the current defn, skip emitting
   the retain entirely (static elision — the runtime `icmp` guard remains
   for all other sites).
5. Preserve: `enum_out_busy` set/clear around constructors; loop-carried
   carrier rules (tail-loop path); heap-box returns unchanged.

**Tests (Rule 9):**

6. Unit: analysis facts — positive (term-chains into slot, single live
   image), negative (two live images, term of local identifier, term of
   call result with unknown summary, state-stored image).
7. IR-shape test: compiled `ws_frame` has zero `ret_copy` labels in the
   7-function family (golden or grep-style assertion), and still correct.
8. Behavioral: `cargo test --lib`; `ws_frame` output `0016384`; full
   `--runtime` A/B; determinism quick; ASan incl.
   stack-use-after-return; I11; daily; Praetor on changed dirs.
9. Perf gate: interleaved A/B ×5 — must land within tolerance of baseline
   **and** beat current; record in results file. If it does not beat
   current meaningfully, the change still ships only if it is
   complexity-neutral and correctness-clean — otherwise revert (record
   the refutation).

**Exit criterion:** `ws_frame` at parity (≤1.05x baseline, no other
benchmark regressed), all gates green.

### Step 3 — Proof 3 conditions → gate replacement → Proof 2

*Depends on:* Step 0 root cause (its fix is encoded as condition 2's
implementation). Separate PR from Step 2.

1. **Condition 2 implementation:** escape analysis for enum handles —
   every escape sink (state store, collection/tuple/object-field store,
   closure capture, unknown/FFI call, cross-node publication, return from
   txn) requires `own_enum_value` (heap-owned image) unless the image is
   provably already the destination slot owned by a longer-lived frame.
   Reuse/extend R1 parameter escape facts + `non_escaping_params`.
2. **Condition 1 audit:** single-source layout — every `image_words` /
   image-malloc site reads `EnumLoweringPlan`; fix any legacy-fallback
   divergence found (grep list in findings file).
3. **Condition 3:** `collect_boundary_enums` narrows to export/FFI-visible
   enums only (`TopLevel::Export` arms stay; plain Definition/Transaction
   signature arms go). Internal signatures no longer force boxing.
4. Gate becomes: `allow_object_inline = conditions1∧2∧3` (proof result,
   not name rule). Keep the old blanket gate as the fallback whenever any
   condition is unknown — "no proof ⇒ box" (plan Safety rule).
5. **Proof 2 follows mechanically:** `object_payload_is_inline("Ok",
   "WsFrame")` becomes true under the new proof ⇒ `ws_partial` joins
   `caller_destination_defns` ⇒ no `malloc(34)` ⇒ below-baseline
   `ws_frame`. Verify `object_repr` tests + add new ones.
6. Full gates as in Step 2 plus: net_stack O0–O3, ASan, **real I11**,
   browser, JSON, FFI ABI unchanged (public export layout must not move —
   condition 3 protects it), copy-elision probes stay disabled (PR51
   unconditional owned extraction remains the floor).

**Exit criterion:** gate replaced by proof, I11 green with relaxation,
`ws_frame` below baseline, zero regressions.

## Ordering, commits, records

- Step 1 runs first (read-only, sizes Step 2). Step 0 runs in parallel
  with Step 1 (forensics, no code except a TEMP env-gated probe).
- Commit after each logical step; `cargo test --lib` green per commit;
  Praetor on changed dirs; docs (`architecture/` if API contracts change)
  in the same commit.
- Append measured results to the findings file (append-only) + dated
  `benchmarks/results/` entries. Any newly found bug → BUGS.md with root
  cause, then triage: fix here if on the path, else record.

## Risks

- **Step 1 shows copies are not the cost** → Step 2 shrinks or is
  refuted with evidence; gap attribution becomes the deliverable.
- **Step 0 finds a deep escape class** → Step 3 scope grows; do not
  rush the gate — two prior refutations exist.
- **Threading introduces aliasing** (the 10-29 full-forwarding failure:
  distinct live images aliasing one destination) → prevented by the
  liveness precondition; ASan + server gates are the tripwire; fallback
  path unchanged.
- **Determinism:** all new analysis iteration sorted (BTreeMap/sorted
  HashMap) per house rule.
