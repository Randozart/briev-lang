# What's left for Briev — status survey (2026-11-22, research notes)

Question: what work remains? Sources: `docs/plans/INDEX.md` (the standing map),
`BUGS.md`, open GitHub issues/PRs, and the wave-7/8/9 landing records. Remote
tip at survey time: `0c1c9a88c` (the net/G22 lane, actively moving).

## Tier 1 — a live consumer is broken (do first)

1. **Kiosk daemon uncompilable at trunk (BUGS.md 2026-11-21, OPEN).**
   Wave-8's contract tightening refutes 41 call sites in
   `briev-kiosk-avatar/service/daemon.bv` (tensor_get ×17, tensor_set ×13, …).
   Root cause is NOT the consumer: a local assigned a provable value on one
   path and a call result on another (`let c = 0; when … { c = chan(i); };`)
   loses its fact at the join — pre-wave-8 (`4caace291`) accepted it. The fix
   is the checker's fact join across `when` reassignment, NOT 41 daemon-side
   guards. Blocks: kiosk re-pin past `4caace291`, every end-to-end measurement
   (soak/ASan/native smoke) against current trunk.

## Tier 2 — the codegen-bug stream (eight open issues, all recent)

Issues #152–#159, filed 2026-11-21/22, all codegen/lowering defects with
minimal repros, several from the kiosk's real workloads:

- #159 appending a struct local to a `List<Struct>` parameter corrupts fields
- #158 reassigning a struct local copied from a parameter corrupts it (SIGSEGV)
- #157 two `Result<String,String>` locals in a recursive fn alias; both
  unwraps return the second
- #156 empty list literal in an obj field passes `check` then panics
- #155 `json.bv` parses fractional numbers as binary32 — silent binary64 loss
- #154 `Float64 as String` uses the 32-bit formatter, prints garbage
- #153 decimal float literal is f32-rounded even in a `Float64` slot
- #152 local HashMap query lowers to a non-dominating alloca; invalid IR

Grouping: #158/#159 are one ownership-clone family; #153–#155 are ONE
float-width seam (binary64 end-to-end: literal → storage → formatter → JSON);
#157/#152 are local-lifetime/alloca placement; #156 is a missing front-door
check. #157 also looks adjacent to the wave-7 phi-merge class (#148, FIXED).

- **#148 is stale-open**: the dangling-phi defect was FIXED in wave-7
  (`2f4174a97`, `tests/diverging_arm_match.rs`). The issue should be closed
  with the regression-test reference — housekeeping, not engineering.

## Tier 3 — checker discharge gaps (issue #151, feeds Tier 1)

Three fact-establishment behaviours, one root each, all "undischargeable in a
node body / fragile in a defn":

1. `when cond { endprogram … }` does not refine the fall-through in a node
   body (the defn `term` form does refine — wave-8's `stmts_end_defn` covers
   defns only).
2. `trap;` does not refine.
3. A call inside a *diverging* guard branch clears obj-field facts established
   by earlier guards — facts should be monotone across a branch that cannot
   fall through.

These are the same fact-engine extensions that would ALSO fix Tier 1's join
bug — do them as one campaign against the fact engine, not as three patches.

## Tier 4 — parked decisions awaiting the owner

- **Post-discharge flip**: corpus measured 242 files / 210 PROVEN / 18
  UNPROVEN / 34 ERR; flipping makes ~52 files fail — a campaign, needs the
  corpus fixed first (`docs/plans/2026-11-21-wave8-linear-exit.md` §R4).
- **`term` surface decision**: removing `term` from non-void defns /
  `return`-style exits. Evidence gathered (2634 of 2636 value-`term`s are
  block-final; `when` has no `else`, so an early-exit keyword is
  load-bearing); needs a deprecation window + byte-identical-`.ll` A/B.
- **PR #147** (`feat/evented-net`): still open; blocked on one fixture
  (`evented_smoke.bv`) denying under the now-enforced contract gate — the
  fix is documented in the PR comment, waiting on owner/branch-owner word.
- **checkfree follow-ups**: interval reasoning is LANDED (wave-9 delivered it
  via prover delegation); the true index type remains NOT WORTH DOING (needs
  a fallible constructor); generics need a monomorphization hook that does
  not exist.

## Tier 5 — the standing lanes (from INDEX.md, mostly device-gated)

- **X (six XPU items)** and **K3 (edge budgets)**: blocked on PTL node access
  and owner numbers respectively (2026-11-16 next-steps plan).
- **PTL tiled-GEMM correctness bug** (`2026-11-17-ptl-tiled-gemm-dispatch.md`):
  `emit_tiled` decodes tiles with no bounds check while the runner launches
  M·N work items — 16x out-of-bounds stores masked by zero-filled fixtures.
  Plan: non-zero checked fixture (today's compiler must FAIL it) → kernel
  guard → one shared geometry → re-measure. Device work.
- **f16-tier GEMM**: the real 9x deficit is f16-storage vs f32; remedy W3(1)
  f16-storage/f32-accumulate tiled kernel (device).
- **NEO residency (S2)**, **GEMM canary (B3)**, **pin advance (B4/P3)**:
  device or kiosk-session work.
- **M1–M4 diagnostics plan** (`2026-11-18-compiler-diagnostics-ir-verification.md`):
  verify every module by default; `; briev:` breadcrumbs; an `invalid_ir`
  corpus column turning BUGS groups A–E into 17 named diagnostics. Device-free
  and directly would have caught several of today's Tier-2 issues earlier.
- **P1 IR-verify corpus as a CI gate** (`2026-11-18-gate-the-ir-check-and-advance.md`):
  parallelise + workflow + ratchet; plus the "never emit a call to an
  undefined symbol" pass covering the 17 BUGS A–E entries.
- **#2** (C-callable static library export) and **#4** (macOS build): old,
  external-host/feature-scope, still open.

## Suggested order (if the owner asks for one)

1. Tier 1 + Tier 3 as ONE campaign: fact-engine join/monotonicity fixes
   (refine on endprogram/trap diverging guards; keep facts across a call in a
   diverging branch; join facts for a conditional reassignment). This unblocks
   the kiosk and closes #151 together with the 41-refutation regression.
2. The float-width seam (#153+#154+#155) as one campaign.
3. The ownership-clone family (#158+#159), then #157/#152/#156.
4. Close #148 as fixed; the M1–M4 diagnostics plan as the meta-fix that makes
   the next bug class visible in minutes instead of sessions.
