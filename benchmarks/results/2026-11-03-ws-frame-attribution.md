# ws_frame attribution — 2026-11-03

Execution plan: `docs/plans/2026-11-03-ws-frame-fast-path-execution.md`
(Step 1). Findings file: `docs/plans/2026-11-03-ws-frame-safe-fast-path.md`.

**Verdict: the "2x vs baseline" regression is a phantom. The locked
baseline is numerically WRONG (UB dead-frame pointers) and is fast only
because it skips the correct checksum. The current tree is CORRECT and is
within ~8% of the C reference in retired instructions (~13% wall clock).**

## Headline numbers (correctness-gated)

Machine: same host, `clang 23.1.3`, harness link line
`-O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections -Wl,--gc-sections`.
Instruction/branch/cache counts from `perf_event_open`
(`PERF_COUNT_HW_INSTRUCTIONS|BRANCH_MISSES|CACHE_MISSES`, user-space only).
`F_TOTAL=4096`, `PASSES=4`, `BOUND=50000000`.

| impl | correct? | instructions | branch misses | cache misses | wall (ms) |
|---|---|---:|---:|---:|---:|
| C reference (`ws_frame_c.c`) | yes | 78,650,700 | 70.6k | 752k | 20.4 |
| **current** (`briev-lang` working tree) | **yes** | 85,222,500 | 74.2k | 770k | 23.0 |
| locked baseline (`42279aaf`) | **NO** | 23,597,500 | 36.9k | 11.9k | 12.6 |

Wall = interleaved ×8 (`C, base, cur` round-robin), averages above.
`current` vs `C`: **+8.3% instructions, +5% branch misses, +2.4% cache
misses, ~1.13x wall** — effectively at C parity.

## Correctness evidence (decisive)

`PASSES` is a compile-time constant. With the real benchmark's `PASSES=4`
the printed accumulator is `acc ^= sum` over four identical passes per
frame, so `acc` is **always 0** regardless of correctness — the benchmark
cannot distinguish a correct from a wrong checksum. Rebuilding at
`PASSES=1` (same source, same compilers) exposes the difference:

| impl (PASSES=1) | printed `acc\|invalid\|total` | matches C? |
|---|---|---|
| C reference | `103005804\|0\|4096` | — |
| current compiler | `103005804\|0\|4096` | **yes, exact** |
| baseline compiler | `7441\|0\|4096` | **no** |

The baseline compiler computes a different payload checksum. It also
**segfaults** (exit 139) on a payload-heavy variant (all frames 64-bit,
7000–9999 byte payloads) and under any `LD_PRELOAD` (it crashed under
`bump.so`, `prof.so`, `maps.so`). This is the recorded dead-frame UB: the
baseline returns a pointer into a dead callee frame, so the decoded
`payload_len`/payload range it checksums is garbage — shorter than the
real payload, hence "fast" and wrong.

The benchmark's even `PASSES` masks this (see BUGS.md entry added
2026-11-03). Any correctness gate for `ws_frame` must use an odd
`PASSES` (or a non-XOR accumulator).

## Attribution (where time goes)

Phase isolation by compiling `F_TOTAL=4096` at `PASSES=1` vs `PASSES=4`
and solving the linear system (build runs once per frame; decode runs
`PASSES` times):

| phase | baseline (instr/frame) | current (instr/frame) |
|---|---:|---:|
| build (corpus construction, SIMD fill) | ~4,878 | ~5,013 |
| decode (ws_decode + validate + payload checksum) | ~221 | ~3,947 |

- **Build phase is at parity** (±3%). The earlier suspicion that the
  build fill loop differs is refuted: the hot `vpmovzxbq`/`vpaddq`
  build-fill loop is instruction-for-instruction equivalent.
- **Decode phase differs 18x** — but the baseline's number is bogus (it
  checksums a truncated range). Against the *correct* reference the real
  question is cur-vs-C, where the gap is only ~8%.
- Payload-size scaling confirms the decode gap is payload-byte
  proportional: forcing all frames to 7-bit payloads (≤125 B) drops
  current to 12.8 M instr; forcing all to 64-bit (7000–9999 B) raises it
  to 198.6 M. Current costs ~1.34 instr/payload-byte in the masked
  checksum (`ws_payload_byte`), and the C reference tracks it closely.
- A `SIGALRM`/`ucontext` PC sampler (no perf) shows current spends ~48%
  of samples in the two checksum loops (`main+0x830` vectorised,
  `main+0x980` scalar masked tail); the baseline's sampler output is
  truncated because it crashes under `LD_PRELOAD`.

## Sizing verdict for the plan

The plan's Step 2 (Proof 1, `retain_enum_return` elision) targets the
baseline gap. That target does not exist: the ~10 ms "gap" is the cost of
doing the checksum the baseline skipped via UB, which no proof can
legally remove. The only legitimate headroom is the **cur-vs-C gap**:

- `current` trails `C` by **~6.6 M instructions (~8%) = ~2.6 ms**.
- The retain copies (16 branch sites, 32 `ret_copy` labels) are a
  candidate but not proven to be the whole 8%; measuring that requires an
  IR A/B of the retain blocks alone (to do before sizing any emission
  work).
- **Proof 1 ceiling: ≤ ~2.6 ms**, only if the retain blocks account for
  that residual. It cannot recover the baseline's 12.6 ms.
- **Proof 2/3 (I11, boundary gate) are real compiler-correctness work**
  but are **not** required for `ws_frame` — parity with the correct C
  reference already holds.

## Reproduce

Artifacts under `/tmp/opencode/`: `wsv/` (variant builds),
`ws_frame_base|cur/*.ll`, `d_base_full.asm`, `d_cur_full.asm`,
`pe.c` (instruction counter), `prof.c` (PC sampler). Variants regenerate
by editing `const F_TOTAL` / `const PASSES` in `benchmarks/ws_frame.bv`
and compiling with each worktree's `target/release/brievc` using the
harness link line above.
