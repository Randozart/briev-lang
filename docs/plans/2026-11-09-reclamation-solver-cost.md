# Reclamation solver cost

## Evidence and hypothesis

At main `114b2a54`, building the external recommender server with the debug
compiler exceeds 30 minutes, one CPU core and approximately 475 MB RSS. Perf
samples concentrate in BTreeSet insertion; a debugger stack locates the caller
at `Graph::field_contents`, `src/analysis/reclamation.rs:200`, inside propagation.
The solver repeatedly clones and unions node sets until a fixed point, once per
callable root. Node IDs are dense indices; general-purpose ordered trees make
each union perform element-by-element tree operations. No correctness rule may
be disabled or bounded to reduce compilation time.

Hypothesis: exact word-wise unions of dense node sets reduce the cost without
changing the finite monotone fixed point. Constants remain ordered i64 sets.
Before implementation, compare the current tree-union kernel against word-wise
union with identical mathematical results. This is compiler-analysis cost, not
an emitted-IR optimization; generated IR must remain identical afterward.

## Baseline and gates

| Measurement | Before | After |
|---|---|---|
| External server, debug compiler | stopped at 47m20s; 47m16s CPU, no output | still exceeds 600-second diagnostic limit |
| External server, release compiler | timed out at 600.12 s (599.03 s CPU), no emission | successful native build 197.21 s (196.31 s CPU) |
| All runtime benchmarks (`build_and_bench.sh --runtime`) | 44 summary rows pass | 44 summary rows pass; 53 LLVM files identical |
| Set-union kernel (128 overlapping sets, 1024 facts each, 100 unions) | 587.6–638.9 ms | sparse words 2.44–2.76 ms |

Build release at the unchanged base with an isolated target and lockfile. Preserve
that executable before editing the solver. Record the full runtime harness table
as an artifact linked here; do not report a pass while gates remain pending.

## Implementation and proof obligations

Use a centralized deterministic node-set abstraction for points-to and memory
facts if the experiment supports it; all constraint kinds and conservative
escape rules remain. Test against the old ordered-set solver on cyclic graphs,
late aliases, raw/named memory, copies and external escapes. Add bounded Kani
set-algebra proofs because lost facts could permit premature reclamation.
Run focused tests, real server build, identical IR checks, full `cargo test --lib`,
warning-free build, Praetor and determinism. Open a main-based PR only after gates.

Update reclamation architecture documentation and BUGS.md in the same commit.
Preserve all existing rationale comments and add dated representation rationale
with a replacement condition. Application metrics changes remain in their own repo.

## Pre-implementation experiment

`/tmp/opencode/reclamation-union-experiment.rs`, compiled with `rustc -O`,
compares element-wise BTreeSet unions with dense and sparse word unions. Every
trial asserts identical decoded sets. Three interleaved trials: tree 638.917,
587.585, 631.742 ms; sparse BTreeMap-of-u64 2.761, 2.685, 2.438 ms. Dense vectors
were faster (~0.11 ms) but would reserve empty words for sparse high IDs. Choose
sparse words for bounded storage proportional to actual facts. This measures
the union kernel only; no whole-build speedup is claimed from it.

## Focused verification

- `cargo test --lib analysis::reclamation`: 34 passed, including differential
  cyclic raw/named-memory graphs against the old element-wise fixed point.
- Standalone Kani wrapper importing the production node-set module: both
  `word_union_preserves_all_facts_and_reports_exact_growth` and
  `sparse_iteration_preserves_node_ids_and_order` verified (Kani 0.68.0,
  CBMC 6.11.0). This is set-algebra verification, not a claim of full compiler proof.
- Praetor on all four changed/new Rust files: no diagnostics.
- Old/new release compiler LLVM output byte-identical for eight inputs:
  `nested_object_storage`, `test_collections`, `mixed_effect_shape`,
  `global_lifetime`, `recommender`, `recommender_multinode`, `queue_drain`,
  `pp-types`. BOUND=1000, BODYCOUNT=64, identical stdlib/config paths.
  Script and logs: `/tmp/opencode/reclamation-ir-ab.sh`,
  `/tmp/opencode/reclamation-ir-ab.log`; compiler target isolated at
  `/home/otto/.cache/briev-target-reclamation-cost`.

## Initial real-build measurement

Release baseline and candidate use the same main base, source, stdlib/config and
command. Baseline did not emit before the 600-second limit. Candidate emitted
`server.ll`, then failed linking after 212.04 seconds (210.61 CPU), because
`build_and_bench.sh:274` clears the shared `~/.cache/briev-compiler/ffi/*.o`
while the other build is analyzing. This is not a successful native build.
Rerun application verification with an isolated `XDG_CACHE_HOME`; keep this
initial profiled/concurrent measurement provisional. Independent compiler
profiles are not interchangeable: debug's 47-minute lower bound is not a release
baseline.

Isolated-cache rerun completed successfully in 197.21 seconds, peak RSS 468,776
KiB. This establishes a >3.04x wall-time lower-bound improvement against the
timed-out release baseline, not an exact speedup. The rebuilt native server
passed register/login/logout metric checks, Prometheus `promtool check metrics`,
all provisioned Usage/Health queries through Grafana, and the three-user closed
journey gate: completed=3, failed=0, missed=0, rating outcomes ok=15; independent
SQL `15|15|15|15|15|3`. The Health dashboard was also rendered in Chromium.
Application artifacts are under `/tmp/opencode/reclamation-candidate/`.

The new **debug** compiler still exceeded a separate 600-second limit. Do not
claim sub-four-minute debug compilation; use the validated release binary for
large application builds. The exact sparse-word representation improves the
release path without removing checks, but debug compiler overhead and remaining
analysis costs are not eliminated.

Final `cargo test --lib` with real Z3 4.13.3: **3,556 passed**, zero failed or
ignored, 400.91 seconds. Debug and release builds completed without warnings.
The standard quick determinism harness passed its three programs, with only its
compiler path redirected to the isolated candidate target. App observability
changes and live verification were committed separately as `cbb9faa`.

Full before/after runtime tables are in
`benchmarks/results/2026-11-09-reclamation-solver.md`. Both harness runs exited
zero, all 44 summary rows passed, and all 53 collected `.ll` files were
byte-identical. Node.js bridge execution was unavailable in both runs. Runtime
timings are marked provisional because validation jobs overlapped; no runtime
speedup or slowdown is claimed for identical emission.
