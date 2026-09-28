# Umbrella: native daily-use, GPU parity, and the rest

**Date:** 2026-09-28
**Status:** ACTIVE — full umbrella sequencing all three focus areas.
Main tip at planning: `1d0dc01f` (post e14a merge).

**Focus (user):** (1) native branch functional for daily programming,
(2) GPU at parity or better with stated goals, (3) the rest of the language.
Decisions: Front D first (umbrella order kept); bugs first then
briev_rt.c families (interleaved); full umbrella scope.

## Session findings (2026-09-28, this session)

Executed through Phase 2 + usage-readiness. Commits `a94a95e5` →
`6379b394` (10 commits).

**Phase 1 (Front D):** No re-run needed — the A/B was already done
2026-09-25 (`benchmarks/results/2026-09-25-front-d-ab.md`): REJECTED,
plain path 26× slower on composite @4096. The deferred-region matcher
stays. Verified the m3 harness routes through SPIR-V (not PTX), so a
naive knob-flip A/B on that harness measures nothing — the composite
microbench (`benchmarks/composite_decode_microbench.sh`) is the correct
A/B vehicle. Composite baseline at current tip: p50 430.7 µs @4096.

**Phase 2a (tuple-String):** Verified FIXED at tip `1d0dc01f` — the
repro (`defn -> (String, Int)` + destructure) compiles and runs
(`ab3`; 3-element `(String,Int,String)` → `ab3ab7cd`). BUGS.md entry
updated.

**Phase 2b (BEAST TypeDef members):** Fixed (`1300ce92`) —
serialize/deserialize now round-trip `body.members` via a new
`parse_toplevel` dispatch + `(members (member …)*)` list shape.
Round-trip test added. Pre-fix, nested state was silently lost through
`.f` profiles.

**Phase 2c (stale-binary guard):** Shipped (`478940bb`) — `brievc
freshness` command (mtime compare vs `src/`+`config/`, exit 1 + offending
file when stale). `get_env_int_or` INDEX entry confirmed STALE (zero
references remain; env.bv replaced the intrinsic 2026-07-19).

**Phase 2d (briev_rt.c):** The C task/event machine (async Phase C/D,
~200 lines: `briev_task_spawn/cancel/await` +
`briev_event_alloc/read/fire/ready/strict_trap`) was DEAD — Family H
(`7a3b821d`) had already migrated the scheduler to pure-Briev twins in
`cast_lanes.bv` but never deleted the C bodies. Deleted (`173d5aa6`)
+ removed the 7 dead symbols from `kept_runtime_symbol` + the stale
C-symbol declares in `emit_task_runtime` + the dead
`__briev_free_total` global. `briev_rt.c` 511→303 lines. Remaining in
the file: string-bitop helpers, `briev_syscall`/`briev_sysconf`
(non-Linux fallback / zero-user), `ShellCmd`/`__briev_spawn*`/
`__briev_setenv` (libc-flavored, user-facing per plan §2.5), `__print_float64`,
Tamer HCALL host services (KEEP — tamer frgns reference them).

**Phase 3 (GPU):** Deferred per user (GPUs in use). Composite baseline
captured: 430.7 µs @4096 (matches the 2026-09-25 deferred-era numbers —
no regression from this session's changes). GEMM 4096³ dispatch failed
on both lanes (device contention / not rebuilt) — needs a clean-GPU
re-run. Stage-5 re-rank still needs the fresh full baseline + Rule 12b
A/B before picking 5a/5b/5c/5d.

**Usage-readiness (daily programming):** Two blockers found and fixed
(`6379b394`):
1. **SyncGroup plugin gap** — `sync<g> node` wraps the item in
   `TopLevel::SyncGroup`; the env, print, and inline_frgn plugins'
   `walk_item` had no SyncGroup arm, so `get_env_int!` / `println!` /
   `inline_frgn!` inside a sync-group node body were never resolved —
   the unresolved PluginIntercept hit the codegen panic at
   `emit_expr.rs:1700`. `async_counters_runtime.bv` (sync + println! +
   get_env_int!) crashed on every build. All three plugins now recurse
   into `SyncGroup.item`.
2. **Struct-typed field init** — `let st: Stack<Int, 256> = 0` where the
   `op Init` construction falls through emitted `store [1 x [256 x i64]] 0`
   (invalid IR). The `zeroinitializer` fix (2026-07-31 A4) only covered
   the `None`-init path; the `Some(Decimal(0))` path now emits
   `zeroinitializer` for struct/array LLVM types.

`tests/tier1/daily_use_smoke.bv` added as the day-one regression gate
(string len, Stack via `<-`, `get_env_int!`, arithmetic, sync<group>
node, println! in a when-guard). Picked up by the conformance sweep.
`cargo test --lib` green (2761) at every commit.

**Remaining usage-readiness gaps (known, not fixed this session):**
- `Stack<Int,256> = 0` + `op Init` construction path: the `op Init`
  (`init` txn: `data[0]=val; len=1`) fires for `= 0` but the subsequent
  `<- st` ExtractFrom returned 0 in the smoke test (CopyFrom vs
  ExtractFrom arrow semantics — pre-existing, `stack_push_pop.bv` uses
  the same pattern and "works" because its benchmark doesn't read the
  popped value). Needs a dedicated investigation.
- `SysConf#` / no-arg `SysCall#()` — zero users; retirement is a
  language decision (intrinsic + C fallback + interpreter + signature
  all reference them).
- `test_collections.bv` is aspirational (parse errors — `new_stack`/
  `new_queue`/`new_map` don't exist; the working constructors are
  `op Init` `= 0` / `= []` + `<-` arrows).

## Phase 1 — Front D (2026-09-24-followup-stages.md stage 1)

1. Fresh full baseline (Rule 12): `cargo build --release` +
   `bash benchmarks/build_and_bench.sh --runtime` +
   `bash benchmarks/compare_baseline.sh front-d-before`.
2. A/B: `ptx_deferred_region` 1→0 (`config/ir-lowering.dbvl:115`), m3 gate
   geometry (`TEMPLATE=examples/gpu/attention_decode_composite.abv
   bash benchmarks/m3_attention_harness.sh 4096`), both lanes, correctness
   gate `max_rel < 1e-3` BEFORE timing, interleave ×N.
3. **PASS** → delete `general::has_deferred_region`
   (`src/backend/ptx/general.rs:3341`), deferred branch
   (`src/backend/ptx/mod.rs:1799-1812`), knob (`src/config_tuning.rs:239,351,612-615`,
   `config/ir-lowering.dbvl:115`).
   **FAIL** → record verified IR property (actual linked binary, not `llc -O2`),
   derive principled version from current analysis. No re-add (Rule 20).
4. Device correctness at real shape (gemm_h_bench 4096³ or m3 4096), both lanes.
5. Docs same commit: this plan's status, `metaprogrammed-composites.md` Front D
   section, `proof-vs-shape.md` matcher ledger,
   `benchmarks/results/YYYY-MM-DD-front-d-ab.md`.

## Phase 2 — Native daily-use (bugs first, then finish briev_rt.c)

### 2a. Tuple-String layout bug (BUGS.md:104)
`defn -> (String, Int)` stores String field in i64 slot; must be `ptr`.
Trace tuple pack/unpack in `src/backend/llvm/emit_expr.rs`; fix layout
derivation; repro case becomes behavioral test.

### 2b. BEAST TypeDef members
INDEX.md open bug: "BEAST drops all TypeDef members (`members: vec![]`
both sides) — nested state cannot round-trip through `.f` profiles;
pre-existing." Trace serialization both sides; fix; round-trip test.
Add BUGS.md entry if the fix is non-trivial.

### 2c. Quick hygiene
- **Stale-binary guard** — mechanical check (bit twice: C0, C2).
- **`get_env_int_or` migration** — INDEX claims ~20 benchmark sources;
  `grep -rn get_env_int_or benchmarks/` finds 0 in `.bv` files — verify
  stale vs relocated, then migrate.

### 2d. Families H+I+J+remainder — finish `briev_rt.c` (511 lines)
Remaining symbols:
- **H** async/event machine: `briev_task_cancel/done/mark_waiting`,
  `briev_event_read/fire/ready/strict_trap`, thread pool.
- **I** process/spawn/misc: `ShellCmd`, `__briev_spawn`,
  `__briev_spawn_output`, `__briev_setenv`.
- **E-remainder**: `briev_syscall`, `briev_sysconf`.
- **J** Tamer HCALL: `briev_host_print_int`, `briev_host_table_set`,
  `briev_host_arity_of`, `briev_host_fail`.
- **C-remainder** string helpers: `briev_free_briev_str`,
  `briev_bits_to_str`, `briev_str_band/bor/bxor/bnot`,
  `briev_symbol_available`.

Per family, plan §6.3 gate order (from
`2026-09-09-briev-native-runtime-and-family-realignment.md`):
implement Briev-native → `cargo test --lib` → parity harness byte-identical →
`compare_baseline.sh` no regression → delete C symbols.
- H → event machine Briev-native (declare-guard), cooperative yields;
  A/B on async benchmarks; if regression → `SysCall#(SYS_clone)`+futex
  substrate (documented follow-on).
- I → `SysCall#(fork/execve)` or user-facing frgn to libc (allowed).
- E → `SysCall#` inline asm (mechanism proven by Family E merge).
- J → Tamer stdlib (`.dbv`/`.bv`), low priority.

Then: **Part B fold** (embedded→triple-driven, `is_bare()`,
`prefer_ebv` removal) — verify current state first.
Then: **delete step** — `briev_rt.c` → `.legacy.c` → gone; dead declares,
dead ffi frgns, 4 link sites.

### 2e. Logo swap (Part E)
Recolor/swap `c-briev-*`↔`e-briev-*` assets per plan §7. Cosmetic, anytime.

## Phase 3 — GPU to stated goals

### 3a. Re-baseline (Rule 12)
Fresh full baseline at post-Phase-1 commit. Update
`2026-09-24-followup-stages.md` stage 5 — it's stale: softmax retirement
(`b1730dd7`, `7ee9597a`) and attention composite path (`0051d920`,
`dd70f144`) have landed.

### 3b. 5c — warp-slice threshold retirement (mechanical)
`has_warp_slice` span≥512 / span%4==0 heuristics →
`config/targets.toml` or composite params. Hardware facts (warp=32,
`shfl`) stay in the backend.

### 3c. 5b — S3b cp.async GEMM (biggest GPU prize)
4096³ 23.4 → 38+ TF. Gate: on-device correctness both lanes, real shape,
before timing. Unlocks S4 correctness + S5 perf + S6 auto-tune.
4096³ target 42+ TF requires ≤56-reg kernel body (ceiling, not tuning).

### 3d. 5a — attention 198→125 µs
Deferred-2pass → composite parity gate. Retire remaining fused-attention
family matcher entries (~1400 lines).

### 3e. 5d remainder + M3
M4 ladder rest (`detect_reduction`→`GemmPlan`, each behind perf A/B).
M3 producer-consumer chain fusion (3→1 kernel ≤120 µs f32).

## Phase 4 — Rest of language (post-parity queue)

Ordered by value/dependency, independent of phases 1–3:

1. **Wave 2b runtime pairs** — approved design (alias = interface-instance
   binding). Biggest interop deliverable.
2. **Asm# two-mode intrinsic** — unlocks prefetch/SIMD/rdtsc tier.
3. **Allocator ownership** — `lib/std/alloc.bv` + `alloc-strategies.dbvl`
   points at Briev defns; compiler keeps only `--no-std` bootstrap heap.
4. **Wave 3 derivation** — transitive bridges, synthesized bridge node.
5. **`inline_frgn!` plugin**.
6. **Collections Phase E** — `seq`/`vol`/`async`/`sync<g>` + Rule 22 gate
   (user-deferred).
7. **Silicon** — `hardware_validator` hookup (quick win), `.cbv` retirement,
   CIRCT `ExportVerilog` gap.
8. **Quick wins** — C4 pinout records, ray graphics milestone B,
   causal DAG, C++ expressiveness remainder.

## Standing gates (every commit)

`cargo test --lib` green · no new warnings · Praetor on changed dirs ·
Kani for safety-critical · GPU: on-device correctness both lanes before any
timing claim · docs in same commit · never `git checkout --`/`git restore`.
