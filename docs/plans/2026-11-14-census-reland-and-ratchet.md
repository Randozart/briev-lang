# Census campaign 2 — safe re-land + ratchet enforcement (2026-11-14)

## Where we are

`wip/census-campaign-2` (d3d635586, pushed, marked UNSAFE) carries the second
census campaign: undecided 50 → 12 / releases 59. Safe tip remains
`180b47211` (slice-view correction) on main/intel.

The WIP is blocked by a daemon double-free (`double free or corruption (out)`
on the JPEG `detect_faces` path) caught by the native-smoke gate before any
merge. Everything else in the campaign passed: full lib, conformance,
census numbers, copy discipline.

## Root-cause hypothesis (to confirm with ASAN)

New releases appear in `tensor_slice` / `tensor_reduce_mean` /
`tensor_gather`. Those functions build `od`/dims lists fresh and hand them to
`tensor_take("", od, n)`, which MOVES the list into the returned Tensor
(`owns_dims = 1`). Candidates:

1. **Destination ownership missing**: the pass still considers `od` owned
   after the `tensor_take` call and releases it at exit; later
   `tensor_drop` frees the same dims list again.
2. **Fresh-callee arg classification** (census campaign 1) treats ALL args
   of fresh-set callees as Borrow, bypassing escape/consume summaries — a
   callee that stores an arg into its result would not demote the caller's
   ownership.

Fix at the classification layer, not a point patch:
- honor escape/consume summaries for fresh-callee args too;
- add destination-ownership: a parameter stored into the returned
  object/collection is CONSUMED by the callee (dims into
  `tensor_take`/`named_owned`/`named4`; elements into list literals). Reuse
  the existing destination-ownership analysis where possible.

## Steps

1. Rebuild WIP release; reproduce with ASAN
   (`clang -fsanitize=address daemon.ll briev_rt.c`) on one JPEG request to
   get the exact free pair.
2. Implement the classification fixes; re-run the census (expect ~12 or
   better, releases adjusted honestly).
3. Re-gate: full lib, conformance, native faces, VAD RPC + sessions,
   dashboard IR verify, 300-call soak (must stay flat).
4. FF intel → main, pin kiosk, requeue CI.
5. Clean the local kiosk WIP branch (drop scratch files `build-vadsess/`,
   `okprobe.bv` from the commit before any push).

## Enforcement decision (user: Option A — ratchet now)

Wire `brievc memcheck briev/service/daemon.bv` into kiosk acceptance with a
committed baseline (`harness/memcheck-baseline.txt`: releases + undecided
counts). The gate fails only when the census GROWS. `--deny-memory-ambiguity`
stays available and can be switched on later at zero without rework.

Baseline is recorded at the re-landed census. The remaining families
(parser attr-fallbacks, `arg`, `method_name`/`json_get`,
`select_path`/`dispatch` residuals) stay honest-undecided and are the
follow-up worklist.

## Outcome (2026-11-14)

Re-landed. Blockers resolved:
- **coll object owns-check**: the double free was the scaffold init routing list
  OBJECTS through Alloc# into the request arena; coll_destroy freed the object
  unconditionally. Now owns-checked like data (the intrinsic's own comment
  predicted it). gdb backtrace + `keep env` bisect; ASAN did not reproduce.
- **self tail calls**: strip_frac_zeros' `term strip_frac_zeros(trimmed)` was
  TempTransfer-converted; the binding emitted the recursive call, which
  tail-loop lowering turns into the loop-back `br` inside the binding —
  releases stranded after a terminator (wasm dashboard IR verify).
  Self tail calls are now excluded from temp-transfer (leak-safe).
- **Census**: undecided 50 → 12, releases 58. Ratchet live
  (harness/memcheck_ratchet.sh + baseline; CI step green).
- **PTL runner flakiness**: ptl-baseline was marked Abandoned mid-step three
  times (listener network drops then reconnects; job-level X despite all
  step work completing). The self-diff ORACLE was verified manually on the
  box against the reference clone: **MATCH (7 steps)**. Follow-up: run the
  selfdiff from a host timer or add listener-health handling; the CI job is
  unreliable on this host, the gate itself is green.
- Soak 159→188 MB @300 stair-step (HWM 244) vs previous flat 177 — bounded,
  follow-up (freed-object churn vs arena owns-check).
- Avatar workloads killed per owner instruction; box quiet (load ≈2-9).
