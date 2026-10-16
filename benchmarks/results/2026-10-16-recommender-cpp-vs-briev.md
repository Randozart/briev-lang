# cpp-vs-briev contributor analysis — P0 repro matrix, timings, asm ledger

**Date:** 2026-10-16
**Plan:** `docs/plans/2026-10-16-cpp-vs-briev-contributor-findings.md`
**Source:** github.com/odecode/cpp-vs-briev (Otto Westerlund, commit `714130e`)
**Environment:** Linux, clang **23.1.1**, 8 cores. Otto's environment: WSL,
LLVM **17** — every environment-sensitive verdict below is qualified by this
delta. Local analysis tree: `/tmp/opencode/repro` (probes + inputs).
**Inputs:** real MovieLens-100k `u.data` (grouplens); `simlist.txt`
SYNTHESIZED (943×1682 non-negative integers, `(i*31+j*17)%997`) — Otto's
original is not public. Parity therefore = cross-implementation on identical
synthesized inputs, not vs Otto's published numbers.

## Flaw 4 (NEW, found by P0) — reassigned-let alloca demotion in reactive txn emitters

Before any other claim could be tested, the current tree REJECTED both
recommender programs with invalid IR: "Instruction does not dominate all
uses!" — allocas for `let b = 0;` + `when c { b = …; }` demoted into
`guard.thenN`, reads in `guard.endN`. Root cause: the 2026-08-04 /
2026-10-14 reassigned-let pre-declaration covered `emit_definition` and the
SSA main, but not the reactive `@txn_<name>` (standard + `assume_shape`) nor
`@async_body_<name>` emitters. **Fixed this session**
(`predeclare_reassigned_lets`, four call sites, DRY'd; regression test
`test_reassigned_let_in_reassigned_txn_gets_entry_alloca` — exact name:
`test_reassigned_let_in_reactive_txn_gets_entry_alloca`). All verdicts below
are POST-fix.

## Repro matrix (post-fix, clang 23.1.1)

| Otto's claim (LLVM 17) | Our verdict (clang 23) |
|---|---|
| Global `Float[1586126]` crashes X86 IS | **NOT REPRODUCED** — current lowering emits `%state = alloca %State` + mallocs, no giant aggregate store; `-O1` build + run fine (`7.5`). ENVIRONMENT-DEPENDENT (his clone predates current init lowering) |
| Single giant node → greedy regalloc segfault at -O2/-O3 | **NOT REPRODUCED on 23** — giant single-node sparse (`@txn_run alwaysinline`, ~2400 lines body) compiles + runs at -O0/-O1/-O2/-O3. ENVIRONMENT-DEPENDENT: the IR shape (whole program inlined into one function) is real and live in the tree; LLVM 17-era regalloc would still see it. P2 hardening remains in scope |
| `__wait_for_trigger__` liveness panic / concurrency gate on plain multi-node handoff | **NOT REPRODUCED as runtime panic** — current main rejects the broken shapes at COMPILE time with helpful diagnostics (goal-provability error; "reactive cycle has no provable convergence… fix: state the completion in postconditions"); the convergent-counter handoff shape compiles + runs correctly. Era-dependent; P3 → documentation verdict |
| `to_c_string` needed for file paths | CONFIRMED (stdlib `std/string_c.bv`; used in all ports) |
| Briev ≈ hand-tuned C++ on identical work | CONFIRMED in magnitude (see timings; exact parity is input-dependent) |

## Outputs (parity)

- Briev giantnode -O1 vs multinode -O3: **byte-identical output** modulo the
  timing lines (top-5: 813, 1121, 1188, 1190, 1200 — all 5.0; 1501
  predictions).
- C++ singlepass: **1501 predictions ✓**; top-5 set differs within the 5.0
  tie band ({1190, 1397, 1466, 813, 1121}) — Otto's documented tie-order
  caveat (dozens of items clamp to 5.0; sort permutation among ties differs).
  Synthesized simlist makes many more 5.0 ties than his data.
- Formal per-item parity gate (deterministic tie-break on both sides):
  P4 deliverable.

## Timings — prediction phase, n=7 interleaved, in-program clocks (both sides 2 reads)

Provisional until the P4 gate. All ms.

| Build | p50 | range |
|---|---|---|
| C++ singlepass `-O3 -march=native -ffast-math` | **0.101** | 0.084–0.117 |
| Briev giantnode `-O1` | 0.140 | 0.131–0.220 |
| Briev giantnode `-O3` | 0.140 | 0.131–0.239 |
| Briev multinode `-O1` | 0.143 | 0.137–0.162 |
| Briev multinode `-O3` | 0.137 | 0.132–0.222 |

Briev within ~1.4× of C++ on synthesized inputs (Otto measured neck-and-neck
on his; the delta is input/machine-dependent, not structural). `-O1` ≈ `-O3`
— memory-bound, matching Otto's finding. High-tail outliers (0.22) are
scheduler noise; p50 is the metric.

## P0.5/P0.6 — IR + asm study (all findings class (a): LLVM already wins)

- `predict_item` (multinode -O3 binary): pure scalar, **68 disassembly
  lines, ZERO spills, zero vector regs** — optimal small-kernel shape.
- C++ `main` (weighted_reduce is dead code in the singlepass build; no
  OpenMP compiled): 235 rsp-touching lines — spill-heavy from inlined
  std::vector machinery. Neither side vectorizes: ~32-element blocks with
  an indirect `sim[vv]` gather — vectorization is not profitable, and
  Otto measured SIMD as a pessimization here.
- Briev IR already emits `fadd fast`/`fmul fast` (17 sites in predict_item)
  — LLVM has full reassociation rights; nothing in our lowering blocks it.
  **No technique gaps found; no (b)/(c) items.** The `@gather_*` name hits
  in the Briev binary are runtime helper names, not vector gathers.
- Two IR-hygiene smells noted (not asm-visible post -O3): the deferred
  init-store double-store (`store %t346; store %t366` — dead first store;
  SROA removes), and flaw 4's former guard-scoped allocas (fixed).

## Artifacts

- Repro tree `/tmp/opencode/repro`: `recommender_giantnode.bv` (reconstructed
  from README; single-node sparse with the virtual-newline parse),
  `probe_bigarray.bv`, `handoff_repro{,2,3}.bv`, `vectorclass.h` stub (VCL
  headers not committed upstream; its consumer is dead code in the
  singlepass build), synthesized `simlist.txt`, real `u.data`.
- Otto's `recommender_multinode.bv` copied verbatim (post-fix it compiles).
- Candidate compiler diagnostics (self-assignment warning etc.): BUGS.md
  "Candidate diagnostics" entry — for review, no action taken.

## Fixes landed (2026-10-16, same day)

| Fix | Commit | Gate |
|---|---|---|
| Flaw 4 — reassigned-let alloca demotion (4 emission paths, DRY'd) | `93be0795` | `test_reassigned_let_in_reactive_txn_gets_entry_alloca`; corpus compiles at -O0..-O3 |
| Flaw 1 — `llvm.memset` ≥ `aggregate_zero_init_memset_bytes` (4096) | `a42de3c0` | `test_aggregate_zero_init_memset_threshold`; small arrays byte-identical |
| `.ll` determinism — reassigned-let entry allocas sorted by name | `a42de3c0` | run-to-run .ll stable; pre-change output canonicalized |
| Flaw 2 — `node_inline_weight_threshold` (600) node-inline decision | `6e3a78ef` | multinode byte-identical; giant-node de-inlined, compiles+runs -O1/-O3, timing unchanged (~0.14 ms), output identical |
| Flaw 3 — verdict: era-dependent; current compile-time diagnostics sufficient | doc-only | execution-model note + handoff_repro{,2,3} probes |

## Harness benchmark adopted — `benchmarks/recommender` + `recommender_c.c`

Self-contained isomorphism of Otto's corpus (in-program LCG synthesis of the
rating matrix + sim row — his MovieLens/simlist inputs are not committable);
same algorithm verbatim (sparse layout, single-pass `+first −last`, clamp,
top-5 strictly-greater). Correctness gate = byte-identical output (top-5
item indices + prediction count — integers, exact-5.0-clamp stable):

```
861
594
427
1103
34
1575
```

Briev == C MATCH at default build and at the harness link flags
(`-O3 -flto -march=native -ffast-math …`). Corpus artifacts (his multinode
verbatim, singlepass.cpp, giant-node + bigarray reconstructions, VCL stub)
live in `benchmarks/recommender_corpus/` with full attribution. Timing runs
through `build_and_bench.sh recommender` like every other benchmark; Otto's
in-program prediction-phase numbers above remain the fine-grained record.

**Status: gate passed — provisional marks on the P0 tables above are lifted
for the harness benchmark; Otto's original-input numbers stay recorded as
his claims (his inputs, his machine).**

## Absolute-limit instrumentation (addendum part 2) — `node_inline_weight_threshold` derivation

**Protocol:** `benchmarks/ra_knee_sweep.sh` (weight ladder ×3 clang -O3
median, `###`-pinned concentrated shape) + weight bisection (threshold probe
on the corpus programs). Instrument notes, all fixed en route: dead-code
folding (independent lets vanish — chain them), LLVM strength-reduces the
chained recurrence to a closed form (object size flat — compile TIME is the
metric), `halt` is a reserved word, a `count +=` body makes an unbounded
counter the liveness pass rightly rejects, `set -e` kills `[ ] || [ ] && x=`
on non-improving timings, and beginprogram nodes must be single-pass `[true]`
with a mid node declaring the completion.

**(1) Resource truth — clang 23.1.1, 2026-10-16:**

| body weight | clang -O3 ms (median of 3) |
|---|---|
| 208 | 631 |
| 408 | 546 |
| 808 | 584 |
| 1608 | 536 |
| 3208 | 539 |
| 6408 | 554 |
| 12808 | 620 |
| 25608 | 790 |
| 51208 | 1033 |

Compile cost is LINEAR through 51k weight — no superlinear knee on the floor
toolchain. The resource argument imposes no practical cap.

**(2) Defect-truth bisection (LLVM 17 ledger, weight units):**
- Largest known-HEALTHY node (multinode `txn_report`): weight ∈ (500, 520].
- Known-CRASHING node (giant single node — LLVM 17 greedy-RA segfault):
  weight ∈ (1450, 1480].
- LLVM 17 defect boundary ∈ (510, 1460); valve 600 = boundary_min/2.4 with
  the largest healthy node kept inline (510 < 600 → byte-identical
  emission) and every known-crash shape excluded (1460 > 600).
- Instrumentation facts, not tuning folklore: both weights bisected live;
  re-derive after any toolchain change with the same two probes.

**(3) Verdict:** valve stays 600 — grounded as measured-boundary ÷ 2.4
safety, not a corpus anecdote; retirement per the BUGS.md defect ledger
(floor toolchain > 17 makes it compile-resource hygiene only, at which point
the linear sweep above says even that is belt-and-braces).
