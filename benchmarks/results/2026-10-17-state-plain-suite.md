
================================================
  Building: iir_filter  (tag: optimizer)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'process' dispatched via per-field phi (5/6 fields written)
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
info: txn 'compute' dispatched via per-field phi (3/4 fields written)
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
  Building: ring_buffer  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: 'head' lowered to plain storage — no contract references it
info: txn 'enqueue' dispatched via per-field phi (2/4 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/ring_buffer.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/ring_buffer
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: float_math  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 13 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/float_math.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/float_math
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: float_math_nonzero  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 7 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/float_math_nonzero.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/float_math_nonzero
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: sparse_dispatch  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

wrote benchmarks/sparse_dispatch.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sparse_dispatch
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: print_loop  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=100000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/print_loop.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/print_loop
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: nbody_newton  (tag: runtime)
================================================
info: txn 'simulate' dispatched via vector phi (16/34 fields in 2 groups)
info: field(s) 'last_energy' written by txn 'simulate' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/nbody_newton.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/nbody_newton
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: nbody_newton_accel  (tag: runtime)
================================================
info: txn 'init_bodies' folded to O(1)
  info: counter 'i' retains its store (the only live write)
info: txn 'step_bodies' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/nbody_newton_accel.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/nbody_newton_accel
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: nbody_sqrt  (tag: runtime)
================================================
info: txn 'simulate' dispatched via vector phi (16/33 fields in 2 groups)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/nbody_sqrt.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/nbody_sqrt
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: nbody_sqrt_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/nbody_sqrt_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/nbody_sqrt_idio
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: fasta  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'fasta' dispatched via per-field phi (2/4 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/fasta.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/fasta
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: fannkuch_redux  (tag: runtime)
================================================
info: txn 'fan' dispatched via per-field phi (16/18 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/fannkuch_redux.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/fannkuch_redux
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: mandelbrot  (tag: runtime)
================================================
info: field(s) 't3', 'ci', 't1', 't2', 'cr' written by txn 'mb' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/mandelbrot.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/mandelbrot
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: kalman_filter_runtime  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'propagate' dispatched via countdown loop (N=5000000, 13 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/kalman_filter_runtime.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/kalman_filter_runtime
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: knucleotide  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/knucleotide.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/knucleotide
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: cancel_math  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/cancel_math.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/cancel_math
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: bit_clear  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/bit_clear.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/bit_clear
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: queue_drain  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: queue_drain_sym  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain_sym.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain_sym
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: queue_drain_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain_idio
  Briev binary ready.
  No C source — uses queue_drain_sym_c for cross-reference timing

================================================
  Building: stack_push_pop  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/stack_push_pop.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/stack_push_pop
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: interval_step  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/interval_step.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/interval_step
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: telemetry_stream  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=1000000, 3 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/telemetry_stream.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/telemetry_stream
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: pid_control  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: 'sp' lowered to plain storage — no contract references it
info: txn 'tick' dispatched via countdown loop (N=1000000, 5 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/pid_control.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/pid_control
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: matrix_pipeline  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 17 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/matrix_pipeline.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/matrix_pipeline
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: accumulator_flush  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=100000, 4 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/accumulator_flush.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/accumulator_flush
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: sweep_sparse  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=100000, 5 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_sparse.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_sparse
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: sweep_mid  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=1000000, 9 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_mid.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_mid
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: sweep_dense  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 17 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_dense.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_dense
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: sweep_arr  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via inline SSA (4 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_arr.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/sweep_arr
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: series_converge  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'converge' dispatched via per-field phi (3/5 fields written)
info: field(s) 'x', 'last' written by txn 'converge' are never read — stores eliminated
note: not referenced by any precondition or #!exit condition
info: txn 'converge' folded to O(1)
  info: counter 'i' retains its store (the only live write)
  info: dead fields: 'x', 'last'
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/series_converge.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/series_converge
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: global_lifetime  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'life' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/global_lifetime.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/global_lifetime
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: deep_recursion  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/deep_recursion.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/deep_recursion
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: arena_churn  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'churn' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/arena_churn.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/arena_churn
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: linked_list  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'build' dispatched via countdown loop (N=5000000, 3 fields)
info: field(s) 'head' written by txn 'build' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/linked_list.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/linked_list
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: hash_ops  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'hash' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/hash_ops.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/hash_ops
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: hash_ops_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/hash_ops_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/hash_ops_idio
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: enemy_swarm  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via per-field phi (2/7 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/enemy_swarm.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/enemy_swarm
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: recommender  (tag: runtime)
================================================
info: field(s) 'lcg', 'n_ratings' written by txn 'build_data' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
wrote benchmarks/recommender.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/recommender
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: bridge_glue  (tag: runtime)
================================================
  No .bv source — custom build
  bridge_glue: building C + Briev .so files...
    make: Entering directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'
    cd /home/randozart/Desktop/Projects/briev-lang && \
      /home/randozart/Desktop/Projects/briev-lang/target/release/brievc build pp-types.bv --llvm --out /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench 2>&1 && \
      llc -filetype=obj -relocation-model=pic \
        -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll 2>&1 && \
      cc -c -fPIC -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o lib/runtime/briev_rt.c 2>&1 && \
      cc -shared -fPIC -O2 -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o && \
      rm -f /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o
    warning: cross-op 'List([Identifier("cstring_concat"), HashL, HashR])' body not found, skipping equivalence check
    normalizer: type 'CStr' has no primordial entry and no width metadata — defaulting max width to the target int width (64); declare `spec Bits: N;` if a specific width is required
    normalizer: type 'CStr' has no size metadata and no primordial entry — assuming 8 bytes; declare `spec Bytes: N;` or `spec Bits: N;` if the size matters
    normalizer: type 'CStr' has no alignment metadata and no primordial entry — assuming alignment 8
    normalizer: type 'CDouble' has no primordial entry and no width metadata — defaulting max width to the target int width (64); declare `spec Bits: N;` if a specific width is required
    normalizer: type 'CDouble' has no size metadata and no primordial entry — assuming 8 bytes; declare `spec Bytes: N;` or `spec Bits: N;` if the size matters
    normalizer: type 'CDouble' has no alignment metadata and no primordial entry — assuming alignment 8
    warning: no path ends this program — it runs until the process is killed
      why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
      fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`
    
    info: program dispatched via direct SSA loop
    wrote /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll
    lib/runtime/briev_rt.c:175:40: warning: ‘always_inline’ function might not be inlinable unless also declared ‘inline’ [-Wattributes]
      175 | __attribute__((always_inline)) int64_t __print_float64(double f) {
          |                                        ^~~~~~~~~~~~~~~
      Briev .so: /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so
    -rwxr-xr-x 1 randozart randozart 36936 17 okt 20:23 /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'

================================================
  Building: bridge_multi  (tag: runtime)
================================================
  No .bv source — custom build
  bridge_multi: building Briev .so + protocol shim...
    make: Entering directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'
    cd /home/randozart/Desktop/Projects/briev-lang && \
      /home/randozart/Desktop/Projects/briev-lang/target/release/brievc build benchmarks/multi_lang/export_add.bv --shared --out /home/randozart/Desktop/Projects/briev-lang/target/multi_lang 2>&1
    wrote /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/export_add.ll
    /usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'
    
    wrote /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/export_add.so
    cc -O2 -o /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/proto_shim /home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang/proto_shim.c -ldl
      proto shim: /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/proto_shim
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'

================================================
  RUNNING BENCHMARKS
================================================

=== iir_filter ===
  briev binary: 1219B  (precompute_ok — skip runtime)
  c binary:     1323B
  correctness: MATCH (output: "")

=== precompute_sum ===
  briev binary: 2119B  (precompute_ok — skip runtime)
  c binary:     1462B
  correctness: MATCH (output: "249500")

=== const_heavy ===
  briev binary: 2857B  (precompute_ok — skip runtime)
  c binary:     1901B
  correctness: MATCH (output: "")

=== async_counters_idio ===
  briev binary: 3063B  (precompute_ok — skip runtime)
  c binary:     1610B
  correctness: MATCH (output: "10000000
20000000
30000000
40000000
5000")

=== ring_buffer ===
  Briev: 0.0559s  (min 0.048939s, max 0.061103s)
  C:     0.0682s
  Ratio: 0.82x  →  Briev wins
  correctness: MATCH (output: "9998976
19998976
29998976
39998976
49998")

=== float_math ===
  Briev: 0.1246s  (min 0.123701s, max 0.126153s)
  C:     0.0620s
  Ratio: 2.01x  →  C wins
  correctness: MATCH (output: "")

=== float_math_nonzero ===
  Briev: 0.2471s  (min 0.240382s, max 0.250865s)
  C:     0.1676s
  Ratio: 1.47x  →  C wins
  correctness: MATCH (output: "")

=== sparse_dispatch ===
  Briev: 0.0481s  (min 0.047231s, max 0.049127s)
  C:     0.0686s
  Ratio: 0.70x  →  Briev wins
  correctness: MATCH (output: "")

=== print_loop ===
  Briev: 0.0313s  (min 0.028920s, max 0.034612s)
  C:     0.0739s
  Ratio: 0.42x  →  Briev wins
  correctness: MATCH (output: "")

=== nbody_newton ===
  Briev: 10.6751s  (min 10.647274s, max 10.701662s)
  C:     9.4422s
  Ratio: 1.13x  →  C wins
  correctness: MATCH (output: "-0.169203341")

=== nbody_newton_accel ===
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.001 ms
  Briev: 2.6514s  (min 2.630488s, max 2.671801s)
  C:     0.2013s
  Ratio: 13.17x  →  C wins

================================================
  Building: kalman_filter_runtime  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'propagate' dispatched via countdown loop (N=5000000, 13 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/kalman_filter_runtime.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/kalman_filter_runtime
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== kalman_filter_runtime ===
  Briev: 0.2402s  (min 0.232698s, max 0.253312s)
  C:     0.1879s
  Ratio: 1.28x  →  C wins
  correctness: MATCH (output: "")

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
║ kalman_filter_runtime     ║ 0.2402s    ║ 0.1879s    ║ 1.28x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: nbody_sqrt_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/nbody_sqrt_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/nbody_sqrt_idio
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== nbody_sqrt_idio ===
  Briev: 4.5414s  (min 4.497703s, max 4.634393s)
  C:     4.7692s
  Ratio: 0.95x  →  Briev wins
  correctness: MATCH (output: "-0.169289663")

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
║ nbody_sqrt_idio           ║ 4.5414s    ║ 4.7692s    ║ 0.95x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: fasta  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'fasta' dispatched via per-field phi (2/4 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/fasta.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/fasta
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== fasta ===
  Briev: 0.2012s  (min 0.196749s, max 0.204952s)
  C:     0.3322s
  Ratio: 0.61x  →  Briev wins
  correctness: MATCH (output: "xqjqf")

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
║ fasta                     ║ 0.2012s    ║ 0.3322s    ║ 0.61x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: fannkuch_redux  (tag: runtime)
================================================
info: txn 'fan' dispatched via per-field phi (16/18 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/fannkuch_redux.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/fannkuch_redux
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== fannkuch_redux ===
  Briev: 0.0964s  (min 0.090202s, max 0.099900s)
  C:     0.0907s
  Ratio: 1.06x  →  C wins
  correctness: MATCH (output: "10")

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
║ fannkuch_redux            ║ 0.0964s    ║ 0.0907s    ║ 1.06x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: mandelbrot  (tag: runtime)
================================================
info: field(s) 'ci', 't3', 'cr', 't2', 't1' written by txn 'mb' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/mandelbrot.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/mandelbrot
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== mandelbrot ===
  Briev: 0.7536s  (min 0.729386s, max 0.767235s)
  C:     0.8254s
  Ratio: 0.91x  →  Briev wins
  correctness: MATCH (output: "73
108")

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
║ mandelbrot                ║ 0.7536s    ║ 0.8254s    ║ 0.91x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: knucleotide  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/knucleotide.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/knucleotide
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== knucleotide ===
  Briev: 0.2053s  (min 0.195958s, max 0.214190s)
  C:     0.2231s
  Ratio: 0.92x  →  Briev wins
  correctness: MATCH (output: "3")

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
║ knucleotide               ║ 0.2053s    ║ 0.2231s    ║ 0.92x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: cancel_math  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/cancel_math.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/cancel_math
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== cancel_math ===
  Briev: 0.0463s  (min 0.041985s, max 0.049088s)
  C:     0.0801s
  Ratio: 0.58x  →  Briev wins
  correctness: MATCH (output: "")

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
║ cancel_math               ║ 0.0463s    ║ 0.0801s    ║ 0.58x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: bit_clear  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/bit_clear.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/bit_clear
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== bit_clear ===
  Briev: 0.0007s  (min 0.000000s, max 0.001280s)
  C:     0.0005s
  Ratio: 1.40x  →  C wins
  correctness: MATCH (output: "0")

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
║ bit_clear                 ║ 0.0007s    ║ 0.0005s    ║ 1.40x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: queue_drain  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== queue_drain ===
  Briev: 0.0337s  (min 0.032171s, max 0.035551s)
  C:     0.0742s
  Ratio: 0.45x  →  Briev wins
  correctness: MATCH (output: "")

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
║ queue_drain               ║ 0.0337s    ║ 0.0742s    ║ 0.45x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: queue_drain_sym  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain_sym.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain_sym
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== queue_drain_sym ===
  Briev: 0.0349s  (min 0.033952s, max 0.035545s)
  C:     0.0774s
  Ratio: 0.45x  →  Briev wins
  correctness: MATCH (output: "")

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
║ queue_drain_sym           ║ 0.0349s    ║ 0.0774s    ║ 0.45x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: queue_drain_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/queue_drain_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/queue_drain_idio
  Briev binary ready.
  No C source — uses queue_drain_sym_c for cross-reference timing

================================================
  RUNNING BENCHMARKS
================================================

=== queue_drain_idio ===
  Briev: 0.0315s  (min 0.030661s, max 0.032510s)
  C:     0.0686s
  Ratio: 0.46x  →  Briev wins
  correctness: MATCH (output: "")

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
║ queue_drain_idio          ║ 0.0315s    ║ 0.0686s    ║ 0.46x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: stack_push_pop  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/stack_push_pop.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/stack_push_pop
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== stack_push_pop ===
  Briev: 0.0391s  (min 0.032998s, max 0.040984s)
  C:     0.0692s
  Ratio: 0.57x  →  Briev wins
  correctness: MATCH (output: "")

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
║ stack_push_pop            ║ 0.0391s    ║ 0.0692s    ║ 0.57x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: interval_step  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/interval_step.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/interval_step
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== interval_step ===
  Briev: 0.0471s  (min 0.042709s, max 0.048956s)
  C:     0.0775s
  Ratio: 0.61x  →  Briev wins
  correctness: MATCH (output: "")

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
║ interval_step             ║ 0.0471s    ║ 0.0775s    ║ 0.61x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: telemetry_stream  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=1000000, 3 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/telemetry_stream.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/telemetry_stream
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== telemetry_stream ===
  Briev: 0.2369s  (min 0.235479s, max 0.238230s)
  C:     0.2305s
  Ratio: 1.03x  →  C wins
  correctness: MATCH (output: "")

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
║ telemetry_stream          ║ 0.2369s    ║ 0.2305s    ║ 1.03x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: pid_control  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: 'sp' lowered to plain storage — no contract references it
info: txn 'tick' dispatched via countdown loop (N=1000000, 5 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/pid_control.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/pid_control
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== pid_control ===
  Briev: 0.4305s  (min 0.426729s, max 0.433385s)
  C:     0.3496s
  Ratio: 1.23x  →  C wins
  correctness: MATCH (output: "")

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
║ pid_control               ║ 0.4305s    ║ 0.3496s    ║ 1.23x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: matrix_pipeline  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 17 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/matrix_pipeline.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/matrix_pipeline
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== matrix_pipeline ===
  Briev: 0.6424s  (min 0.620809s, max 0.673041s)
  C:     0.6868s
  Ratio: 0.94x  →  Briev wins
  correctness: MATCH (output: "")

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
║ matrix_pipeline           ║ 0.6424s    ║ 0.6868s    ║ 0.94x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: accumulator_flush  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=100000, 4 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/accumulator_flush.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/accumulator_flush
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== accumulator_flush ===
  Briev: 0.1611s  (min 0.156546s, max 0.167743s)
  C:     0.2232s
  Ratio: 0.72x  →  Briev wins
  correctness: MATCH (output: "")

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
║ accumulator_flush         ║ 0.1611s    ║ 0.2232s    ║ 0.72x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: sweep_sparse  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=100000, 5 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_sparse.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_sparse
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== sweep_sparse ===
  Briev: 0.2396s  (min 0.237869s, max 0.241038s)
  C:     0.1663s
  Ratio: 1.44x  →  C wins
  correctness: MATCH (output: "")

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
║ sweep_sparse              ║ 0.2396s    ║ 0.1663s    ║ 1.44x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: sweep_mid  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=1000000, 9 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_mid.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_mid
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== sweep_mid ===
  Briev: 0.2687s  (min 0.258817s, max 0.285092s)
  C:     0.2025s
  Ratio: 1.33x  →  C wins
  correctness: MATCH (output: "")

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
║ sweep_mid                 ║ 0.2687s    ║ 0.2025s    ║ 1.33x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: sweep_dense  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via countdown loop (N=5000000, 17 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_dense.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/sweep_dense
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== sweep_dense ===
  Briev: 0.4669s  (min 0.458934s, max 0.474521s)
  C:     0.3869s
  Ratio: 1.21x  →  C wins
  correctness: MATCH (output: "")

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
║ sweep_dense               ║ 0.4669s    ║ 0.3869s    ║ 1.21x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: sweep_arr  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via inline SSA (4 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/sweep_arr.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/sweep_arr
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== sweep_arr ===
  Briev: 0.5269s  (min 0.502173s, max 0.542520s)
  C:     0.4578s
  Ratio: 1.15x  →  C wins
  correctness: MATCH (output: "")

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
║ sweep_arr                 ║ 0.5269s    ║ 0.4578s    ║ 1.15x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: series_converge  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'converge' dispatched via per-field phi (3/5 fields written)
info: field(s) 'last', 'x' written by txn 'converge' are never read — stores eliminated
note: not referenced by any precondition or #!exit condition
info: txn 'converge' folded to O(1)
  info: counter 'i' retains its store (the only live write)
  info: dead fields: 'last', 'x'
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/series_converge.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/series_converge
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== series_converge ===
  Briev: 0.0006s  (min 0.000000s, max 0.001715s)
  C:     0.0004s
  Ratio: 1.50x  →  C wins
  correctness: MATCH (output: "0.500050008")

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
║ series_converge           ║ 0.0006s    ║ 0.0004s    ║ 1.50x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: global_lifetime  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'life' dispatched via countdown loop (N=5000000, 1 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/global_lifetime.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/global_lifetime
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== global_lifetime ===
  Briev: 0.0414s  (min 0.036156s, max 0.043637s)
  C:     0.0847s
  Ratio: 0.49x  →  Briev wins
  correctness: MATCH (output: "")

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
║ global_lifetime           ║ 0.0414s    ║ 0.0847s    ║ 0.49x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: deep_recursion  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
wrote benchmarks/deep_recursion.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/deep_recursion
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== deep_recursion ===
  Briev: 0.0003s  (min 0.000000s, max 0.001276s)
  C:     0.0002s
  Ratio: 1.50x  →  C wins
  correctness: MATCH (output: "15")

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
║ deep_recursion            ║ 0.0003s    ║ 0.0002s    ║ 1.50x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: arena_churn  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'churn' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/arena_churn.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/arena_churn
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== arena_churn ===
  Briev: 0.0321s  (min 0.031274s, max 0.033487s)
  C:     0.1240s
  Ratio: 0.26x  →  Briev wins
  correctness: MATCH (output: "")

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
║ arena_churn               ║ 0.0321s    ║ 0.1240s    ║ 0.26x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: linked_list  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'build' dispatched via countdown loop (N=5000000, 3 fields)
info: field(s) 'head' written by txn 'build' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/linked_list.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/linked_list
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== linked_list ===
  Briev: 1.3458s  (min 1.292679s, max 1.381599s)
  C:     2.7632s
  Ratio: 0.49x  →  Briev wins
  correctness: MATCH (output: "")

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
║ linked_list               ║ 1.3458s    ║ 2.7632s    ║ 0.49x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: hash_ops  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'hash' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/hash_ops.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/hash_ops
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== hash_ops ===
  Briev: 1.3101s  (min 1.275382s, max 1.343494s)
  C:     1.7929s
  Ratio: 0.73x  →  Briev wins
  correctness: MATCH (output: "")

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
║ hash_ops                  ║ 1.3101s    ║ 1.7929s    ║ 0.73x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: hash_ops_idio  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'work' dispatched via countdown loop (N=5000000, 2 fields)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/hash_ops_idio.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/hash_ops_idio
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== hash_ops_idio ===
  Briev: 0.9751s  (min 0.955019s, max 0.998231s)
  C:     0.9288s
  Ratio: 1.05x  →  C wins
  correctness: MATCH (output: "")

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
║ hash_ops_idio             ║ 0.9751s    ║ 0.9288s    ║ 1.05x    ║ C      ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: enemy_swarm  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'tick' dispatched via per-field phi (2/7 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/enemy_swarm.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/enemy_swarm
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== enemy_swarm ===
  Briev: 0.1675s  (min 0.159751s, max 0.177606s)
  C:     0.2107s
  Ratio: 0.79x  →  Briev wins
  correctness: MATCH (output: "")

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
║ enemy_swarm               ║ 0.1675s    ║ 0.2107s    ║ 0.79x    ║ Briev  ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: recommender  (tag: runtime)
================================================
info: field(s) 'n_ratings', 'lcg' written by txn 'build_data' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
wrote benchmarks/recommender.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/recommender
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  RUNNING BENCHMARKS
================================================

=== recommender ===
  Briev: 0.0155s  (min 0.012638s, max 0.017295s)
  C:     0.0122s
  Ratio: 1.27x  →  C wins

================================================
  Building: iir_filter  (tag: optimizer)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: txn 'process' dispatched via per-field phi (5/6 fields written)
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
  RUNNING BENCHMARKS
================================================

=== iir_filter ===
  briev binary: 1219B  (precompute_ok — skip runtime)
  c binary:     1323B
  correctness: MATCH (output: "")

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
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: precompute_sum  (tag: optimizer)
================================================
info: txn 'compute' dispatched via per-field phi (3/4 fields written)
info: field(s) 'acc_a', 'acc_b' written by txn 'compute' are never read — wasted work
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
  RUNNING BENCHMARKS
================================================

=== precompute_sum ===
  briev binary: 2119B  (precompute_ok — skip runtime)
  c binary:     1462B
  correctness: MATCH (output: "249500")

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
║ precompute_sum            ║ precomputed ║            ║          ║        ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

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
  RUNNING BENCHMARKS
================================================

=== const_heavy ===
  briev binary: 2857B  (precompute_ok — skip runtime)
  c binary:     1901B
  correctness: MATCH (output: "")

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
║ const_heavy               ║ precomputed ║            ║          ║        ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

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
  RUNNING BENCHMARKS
================================================

=== async_counters_idio ===
  briev binary: 3063B  (precompute_ok — skip runtime)
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
║ async_counters_idio       ║ precomputed ║            ║          ║        ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: bridge_glue  (tag: runtime)
================================================
  No .bv source — custom build
  bridge_glue: building C + Briev .so files...
    make: Entering directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'
    make: Nothing to be done for 'all'.
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'

================================================
  RUNNING BENCHMARKS
================================================

=== bridge_glue ===
============================================================
GLUE Bridge vs C FFI — String Round-Trip Benchmark
============================================================

[Pure Python]
  native                median=     221ns  mean=     272ns  min=   120ns  max= 1625779ns  result='42'

[C FFI]
  c_str_echo            median=   13915ns  mean=   15798ns  min=  6734ns  max= 2264399ns  result='42'

[Briev GLUE]
  briev_cstr_rt         median=   12499ns  mean=   15013ns  min=  6798ns  max= 3501854ns  result='42'

[Correctness]
  C:      '42'
  Briev:  '42'
  Native: '42'
  ✅ All match

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
║ bridge_glue               ║ done       ║            ║          ║        ║ MATCH     ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  Building: bridge_multi  (tag: runtime)
================================================
  No .bv source — custom build
  bridge_multi: building Briev .so + protocol shim...
    make: Entering directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'
    make: Nothing to be done for 'all'.
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'

================================================
  RUNNING BENCHMARKS
================================================

=== bridge_multi ===
  Python ctypes + protocol, Node.js koffi + protocol, shell protocol
    =================================================================
    Multi-Language Bridge Benchmark — Briev export defn from Python
    =================================================================
    
    [Pure Python]
      native add                      median=     297ns  mean=     375ns  min=   223ns  max= 2099645ns  result=7
    
    [Python ctypes]
      ctypes add                      median=    2163ns  mean=    2363ns  min=  1046ns  max=  813086ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median= 1543427ns  mean= 1913480ns  min=698286ns  max= 5944512ns  result=7
    
    [Correctness]
      ctypes:  7
      native:  7
      proto:   7
      ✅ All match
    =================================================================
    Multi-Language Bridge Benchmark — Briev export defn from Node.js
    =================================================================
    
    [Pure Node.js]
      native add                      median=     120ns  mean=174.37864ns  min=      58ns  max= 2017301ns  result=7
    
    [Node.js koffi FFI]
      koffi add                       median=     248ns  mean=338.8016ns  min=     174ns  max=  706076ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median= 5004100ns  mean=8330346.93ns  min= 2107953ns  max=47512812ns  result=7
    
    [Correctness]
      koffi:   7
      native:  7
      proto:   7
      ✅ All match
      proto_shim per-call latency (fork+exec+wait)
        avg=7435843ns  min=3408439ns  max=13214497ns  result=7

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
║ bridge_multi              ║ done       ║            ║          ║        ║ PASS      ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝
