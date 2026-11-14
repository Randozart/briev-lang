# Saltpetre + ownership campaign — session record (2026-11-12 → 2026-11-13)

Status: reconciled at `e00957017` (main == intel). Written as the durable
handoff: what landed, what the numbers are, and what remains.

## 1. What landed

### 1.1 Saltpetre — the ML library (localised in `lib/saltpetre/`)
- Extracted verbatim from the kiosk `briev/ml/` stack: `tensor`, `proto`,
  `onnx`, `exec`, `frame`, `yunet` + `audio` (new). Doctrine: pure `.bv`,
  zero compiler knowledge; op ledger is canonical in
  `docs/architecture/saltpetre.md` (30 dispatch arms, each gated).
- `session.bv` owns the arena lifecycle: `sp_begin` (tensor block from the
  ENTRY region), `sp_open_coll` (request coll block, opened per connection
  AFTER persistent caching), `sp_request_reset`, `sp_run`/`sp_run_env`,
  `sp_end`. Slot-keyed model cache (`sp_cached_model(slot, path)` /
  `sp_prime_model`), slots 0=face / 1=vad, scalar globals by design.
- **Lifetime rules learned the hard way** (now contract comments):
  1. Persistent state must be parsed BEFORE the request coll arena opens
     (the face cache landed in the request block once → first reset rewound
     it → segfault in the JPEG RPC; raw-frame passed vacuously at count 0).
  2. `sp_run` resets the inference arena inside the call → inputs must be
     allocated after the previous reset or be owned.
- Gates: in-repo tiny ONNX corpus (`tests/saltpetre_conformance.rs`,
  atol/rtol 1e-3 — conv accumulation order vs ORT) + kiosk acceptance
  (real models, PTL, differential vs onnxruntime).

### 1.2 Kiosk capability
- Daemon consumes the library; `briev/ml/` deleted; arena choreography
  reduced to `sp_begin` / `sp_open_coll` / `sp_request_reset`.
- **`vad_chunks`** (single-shot streaming Silero over raw f32) and
  **`vad_open`/`vad_feed`/`vad_close`** (8-slot scalar session table,
  state carried across requests). Gates: 187-chunk golden tolerance
  (`vad_rpc_smoke.sh`) and STREAMING EQUIVALENCE (`vad_sessions_smoke.sh`:
  the 6 s fixture fed in two CHUNK-ALIGNED parts reproduces the whole-file
  golden — a 2 s split is half-chunk misaligned and drifts).
- CI resilience: acceptance concurrency group, env-tail (proxy/rbv)
  continue-on-error + warning, artifact uploads tolerate the 403 race,
  `ptl_selfdiff` retries a timed-out daemon start once.
- Soak state: 300-call face soak flat after one allocator staging step;
  latest 179 MB VmRSS / 235 MB HWM (was 189/245, originally 144→187 growing).

### 1.3 Compiler ownership (the memory program, M2)
- String slices: `read_whole`/stdlib copy lanes/strip_frac_zeros release
  intermediates; HIR-owned String locals pass (`hir::owned_storage`) with
  body-derived fresh summaries, param consume/escape modes, `keep`,
  release at reassignment/exit, first-class `ReleaseOwned`/`TempTransfer`
  HIR kinds + two-cursor body seam (`storage_release_view`).
- Owned List: `coll_destroy` (object + arena-aware data via
  `coll_arena_owns` high-water query), `tensor_take`/`owns_dims`
  (dims owner slot), nested-seam conversion.
- Coll Float-collection bug FIXED: `StrategyCall` carries `value_ty`; both
  dispatch paths bind the value at its real LLVM width; `box_scalar`
  pre-box removed. (Pointer-arithmetic aliasing stays conservative — the
  AST walker has no types to distinguish it from String concat.)
- **Memcheck owned tier**: `brievc memcheck [--deny-memory-ambiguity]
  [--stdlib-path <dir>] <file.bv>` — release count, conservative drops
  with reasons (alias/escape/consume/keep/explicit-free/store/
  return-borrow/borrow), undecided-owner census with the named gap.
  Derivations added: String-slice freshness, unwrap-through-Match
  identity, ok_fresh (Ok-side-only, flows through locals), scalar
  literals, list-BLOCK freshness (independent of element ownership),
  borrow-result helpers, init-event supersession.

### 1.4 Bugs filed (BUGS.md)
- OPEN: coll-typed globals (`List<Int> = []`) emit a broken `init_state`
  store (scalars work; only matters for dynamic cache slots).
- FIXED (same day): `List<Float>` push/index bit-reinterpretation —
  two-convention collision (bits-box + semantic sitofp), root cause and
  repro recorded; interpreter-divergence check queued with the scaffold
  fix (no standalone interpreter in this lane; the reference's `<-` push
  was already fixed on the main lane).

## 2. Numbers (the honest census)

`brievc memcheck briev/service/daemon.bv` today: **39 releases, 67
undecided** (was 0/0 before the tier, then 16/147). Remaining 67 families:
`unwrap` chains whose Ok side still isn't provable (14), static literals
never reassigned (9), literal-return helpers (`face_param`,
`http_header_block`, `method_name`, `strip_frac_zeros`, `packed_ints`;
~17), the rest small one-offs. `--deny-memory-ambiguity` is implemented but
NOT wired into kiosk CI — the census must reach a stable interpretation
first (see §3.1).

## 2b. Census campaign outcome (2026-11-13, post-record)

Census **147 → 42 undecided / 16 → 47 releases**, soak now DEAD-FLAT from
call 50 (177 MB, HWM 233 — no staging steps at all). Landed: blanket
escape-gate withdrawal (freshness derives from body shape), ok_fresh
through locals + tail self-recursion, list-BLOCK ownership, borrow-result /
borrow-init classification, Static classification (literal-backed locals
are reported separately, superseded by later releases), memcheck prelude
inlining (cast_lanes participates in the census), §13 copy discipline on
`face_param` misses, `strip_frac_zeros`, `pad_digits`, and the b64/http
literal branches (`fresh_string` now a prelude helper built on the
substring lane). Pointer-arithmetic aliasing stays conservative (test
refuted the non-aliasing attempt; reverted). `--deny-memory-ambiguity`
wiring waits for the remaining ~42 (parser attr-fallback families are the
largest honest block).

## 3. What is left

1. **Census interpretation pass** (next): classify genuinely-static
   returns as `Static` (not "undecided" — they allocate nothing and are
   not ambiguous) and chase the last `unwrap` shape. Target: census in the
   ~30s, then wire `--deny-memory-ambiguity` into kiosk acceptance as the
   hard gate (with the documented exception list if any).
2. **§13 copy discipline** where the census is genuine: helpers whose
   literal/alloc branches are mixed get `owned_literal`-style fresh
   returns (cold paths only). This is the remaining product of the memory
   plan, not a derivation gap.
3. **Coll-typed globals init bug** (BUGS open): root-cause the scaffold
   global-init store, restore List slots if dynamic cache growth is
   wanted. Low urgency.
4. **Kiosk CI**: PTL host contention with interactive avatar workloads is
   the recurring infra failure (runner lost communication / reference
   daemon socket timeout). The concurrency group + env-tail degradation
   contain it; a MIN_AVAIL_GB raise for selfdiff (skip-with-warning
   instead of dying mid-run) is the next lever if it keeps biting.
5. **Whisper/STT native**: needs an artifact decision (ONNX export +
   tokenizer strategy) — the frontier after VAD.
6. **NPU lane** (Silero/YuNet on AI Boost): needs device-access/priority
   decision.
7. **Main-lane coordination**: the `.phase-c-target` history pollution is
   cleaned (verified at reconcile); `.gitignore` now carries the rule.
   Reconcile remained conflict-free except mechanical ones (reclamation
   diagnostic formatting, verify-report test, .gitignore).

## 4. Repro/gates quick reference

- Kiosk: `bash harness/{native_smoke,ml_smoke,vad_rpc_smoke,vad_sessions_smoke}.sh`
  (BRIEV_ROOT + BRIEVC envs), `python3 harness/vad_parity.py --exe <bin>`,
  `bash progress/build.sh && python3 benchmarks/rbv_ir_check.py progress/out/dashboard.ll`.
- Compiler: `cargo test --lib`, `cargo test --test saltpetre_conformance`,
  `brievc memcheck [--deny-memory-ambiguity] [--stdlib-path lib] <file>`.
- Repro programs from this campaign: `/tmp/opencode/{f32list,f32call,concatprobe,plusprobe,okprobe}.bv`,
  failing old dump `/tmp/vdstd3/vad_stream.ll:25966`.

## Correction (2026-11-13, post-campaign): String slices are VIEWS

The census campaign briefly classified `String[a:b]` as fresh ("slice =
copy") — WRONG. Per the slicing doc, a contiguous slice is a free
pointer-rebase VIEW. Consequence caught by the saltpetre conformance gate:
`external_bytes` returns `all[off:off+len]` — a view into the file buffer —
and the pass released `all` at exit → use-after-free → conv_ext weights
corrupted (output[5] 1.527 vs 1.380). Fixed: slices classify as Borrow
(view escapes ⇒ the source is demoted from ownership and never released),
and the slice-fresh arm is removed. Census 19 → 18 undecided (proto_bytes /
face_param / http slice families are honestly borrowed). saltpetre
conformance green again; all other gates re-verified. LESSON: before
asserting "X allocates fresh", read the representation doc — the slicing
doc said "zero-copy view" all along.
