
================================================
  Building: iir_filter  (tag: optimizer)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'process' dispatched via per-field phi (5/8 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/iir_filter.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/iir_filter
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: precompute_sum  (tag: optimizer)
================================================
info: txn 'compute' dispatched via per-field phi (3/6 fields written)
info: field(s) 'acc_b', 'acc_a' written by txn 'compute' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/precompute_sum.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/precompute_sum
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: const_heavy  (tag: optimizer)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/const_heavy.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/const_heavy
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: async_counters_idio  (tag: optimizer)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via reactor loop (parallel thread pool)
wrote benchmarks/async_counters_idio.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
2 warnings generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/async_counters_idio
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: UTF8_ops  (tag: optimizer)
================================================
  No .bv source — custom build
  BUILD-FAIL: UTF8_ops_c (C reference — comparison skipped)

================================================
  RUNNING BENCHMARKS
================================================

=== iir_filter ===
  briev binary: 1219B  (precompute_ok — skip runtime)
  c binary:     1323B
  correctness: MATCH (output: "")

=== precompute_sum ===
  briev binary: 2071B  (precompute_ok — skip runtime)
  c binary:     1462B
  correctness: MATCH (output: "249500")

=== const_heavy ===
  briev binary: 2873B  (precompute_ok — skip runtime)
  c binary:     1901B
  correctness: MATCH (output: "")

=== async_counters_idio ===
  briev binary: 3079B  (precompute_ok — skip runtime)
  c binary:     1610B
  correctness: MATCH (output: "10000000
20000000
30000000
40000000
5000")

================================================
  SUMMARY
================================================
  5 iterations per benchmark, avg wall clock via CLOCK_MONOTONIC.
  BOUND=50000000. Nanosecond-precision fork+exec timing harness.
  Tags: runtime=FFI in hot loop, optimizer=precompute_ok.
================================================

╔═══════════════════════════╦════════════╦════════════╦══════════╦════════╦═══════════╗
║ Benchmark                 ║ Briev      ║ C          ║ Ratio    ║ Winner ║ Correct   ║
╠═══════════════════════════╬════════════╬════════════╬══════════╬════════╬═══════════╣
║ iir_filter                ║ precomputed ║            ║          ║        ║ MATCH     ║
║ precompute_sum            ║ precomputed ║            ║          ║        ║ MATCH     ║
║ const_heavy               ║ precomputed ║            ║          ║        ║ MATCH     ║
║ async_counters_idio       ║ precomputed ║            ║          ║        ║ MATCH     ║
║ UTF8_ops                  ║ BUILD-FAIL ║            ║          ║        ║ SKIP      ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  BUILD FAILURES (1)
================================================
  - UTF8_ops:C
  (runs skipped for failed builds; suite still completed.)
