# recommender_corpus — Otto Westerlund's cpp-vs-briev artifacts

Reference corpus for `benchmarks/recommender.bv` + `recommender_c.c` and for
`docs/plans/2026-10-16-cpp-vs-briev-contributor-findings.md` /
`benchmarks/results/2026-10-16-recommender-cpp-vs-briev.md`.

**Upstream:** <https://github.com/odecode/cpp-vs-briev> (commit `714130e`,
2026-10-16), by Otto Westerlund. His README there documents the full
experiment protocol: three Briev ports (dense → sparse → multinode), the
fair-timing re-instrumentation of both sides, and the LLVM-17 crash
workarounds. His headline result — Briev ≈ hand-tuned C++ `-O3` on
identical single-pass work (~0.15 ms prediction phase, MovieLens-100k) —
drove the compiler fixes recorded in the results file.

| File | Provenance |
|------|------------|
| `recommender_multinode.bv` | **Otto's, verbatim.** 7 `async node`s + `defn predict_item`; his -O3-clean shape. Needs `u.data` + `simlist.txt` in cwd (not committed — his simlist is project-private; a deterministic stand-in generator is in the results file) |
| `recommender_singlepass.cpp` | **Otto's, verbatim.** Like-for-like C++ reference (single-pass, no scratch arrays). The bundled VCL (`vectorclass.h`, version2-2.01.04) was not committed upstream; `vectorclass.h` here is a minimal compiling stub — its only consumer, `weighted_reduce`, is dead code in this build |
| `recommender_giantnode.bv` | **Reconstructed** by us from his README (his `recommender_sparse.bv` is not in the upstream repo): the whole pipeline in one convergent node — the whole-program-inlined `@txn_run` shape that crashed LLVM 17's greedy register allocator at -O2/-O3, and the shape P2's `node_inline_weight_threshold` de-inlines |
| `probe_bigarray.bv` | **Reconstructed** from his README: the global `Float[1586126]` probe behind flaw 1 (giant aggregate zero-init store → `llvm.memset`, P1) |
| `vectorclass.h` | Stub, ours (see above) |

Rebuild recipe (results file has the full repro matrix):

```bash
brievc build recommender_corpus/recommender_multinode.bv --llvm \
    --out /tmp/out --stdlib-path lib
clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections \
    -Wl,--gc-sections /tmp/out/recommender_multinode.ll \
    lib/runtime/briev_rt.c -ldl -o /tmp/out/rmn
clang++ -O3 -march=native -ffast-math -Ibenchmarks/recommender_corpus \
    benchmarks/recommender_corpus/recommender_singlepass.cpp -o /tmp/out/cpp
```
