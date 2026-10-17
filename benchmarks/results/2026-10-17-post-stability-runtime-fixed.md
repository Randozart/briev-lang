
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

info: txn 'enqueue' dispatched via per-field phi (2/7 fields written)
warning: #!exit declared but program has no tick loop
note: the exit condition will never be checked
help: add an @link trigger to make the program reactive, or remove #!exit
wrote benchmarks/ring_buffer.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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
info: txn 'simulate' dispatched via vector phi (16/36 fields in 2 groups)
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
info: txn 'simulate' dispatched via vector phi (16/35 fields in 2 groups)
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

info: txn 'fasta' dispatched via per-field phi (2/6 fields written)
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
info: txn 'fan' dispatched via per-field phi (16/20 fields written)
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
info: field(s) 't3', 'cr', 'ci', 't1', 't2' written by txn 'mb' are never read — wasted work
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

info: txn 'tick' dispatched via inline SSA (6 fields)
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

info: txn 'converge' dispatched via per-field phi (3/7 fields written)
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

info: txn 'tick' dispatched via per-field phi (2/9 fields written)
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
    -rwxr-xr-x 1 randozart randozart 32768 17 okt 13:18 /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so
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

=== ring_buffer ===
  Briev: 0.0492s  (min 0.044049s, max 0.054071s)
  C:     0.0512s
  Ratio: 0.96x  →  Briev wins
  correctness: MATCH (output: "9998976
19998976
29998976
39998976
49998")

=== float_math ===
  Briev: 0.0437s  (min 0.041380s, max 0.050630s)
  C:     0.0483s
  Ratio: 0.90x  →  Briev wins
  correctness: MATCH (output: "")

=== float_math_nonzero ===
  Briev: 0.2049s  (min 0.202571s, max 0.206028s)
  C:     0.1653s
  Ratio: 1.24x  →  C wins
  correctness: MATCH (output: "")

=== sparse_dispatch ===
  Briev: 0.0452s  (min 0.044402s, max 0.046099s)
  C:     0.0596s
  Ratio: 0.76x  →  Briev wins
  correctness: MATCH (output: "")

=== print_loop ===
  Briev: 0.0287s  (min 0.028378s, max 0.029101s)
  C:     0.0600s
  Ratio: 0.48x  →  Briev wins
  correctness: MATCH (output: "")

=== nbody_newton ===
  Briev: 8.6723s  (min 8.012163s, max 9.287874s)
  C:     8.9709s
  Ratio: 0.97x  →  Briev wins
  correctness: MATCH (output: "-0.169203341")

=== nbody_newton_accel ===
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
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
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
  Briev: 2.0881s  (min 1.776635s, max 2.277119s)
  C:     0.1744s
  Ratio: 11.97x  →  C wins
  correctness: MATCH (output: "0.50002861")

=== nbody_sqrt ===
  Briev: 2.8581s  (min 2.649800s, max 3.356019s)
  C:     3.9681s
  Ratio: 0.72x  →  Briev wins
  correctness: MATCH (output: "-0.169288814")

=== nbody_sqrt_idio ===
  Briev: 3.4580s  (min 3.252860s, max 3.753439s)
  C:     4.0443s
  Ratio: 0.86x  →  Briev wins
  correctness: MATCH (output: "-0.169289663")

=== fasta ===
  Briev: 0.2007s  (min 0.193073s, max 0.205277s)
  C:     0.3463s
  Ratio: 0.58x  →  Briev wins
  correctness: MATCH (output: "xqjqf")

=== fannkuch_redux ===
  Briev: 0.1075s  (min 0.099355s, max 0.116809s)
  C:     0.0972s
  Ratio: 1.11x  →  C wins
  correctness: MATCH (output: "10")

=== mandelbrot ===
  Briev: 0.9258s  (min 0.893734s, max 0.950831s)
  C:     0.7688s
  Ratio: 1.20x  →  C wins
  correctness: MATCH (output: "73
108")

=== kalman_filter_runtime ===
  Briev: 0.1975s  (min 0.189484s, max 0.215222s)
  C:     0.1836s
  Ratio: 1.08x  →  C wins
  correctness: MATCH (output: "")

=== knucleotide ===
  Briev: 0.2487s  (min 0.245499s, max 0.253197s)
  C:     0.2036s
  Ratio: 1.22x  →  C wins
  correctness: MATCH (output: "3")

=== cancel_math ===
  Briev: 0.0602s  (min 0.055450s, max 0.071361s)
  C:     0.0687s
  Ratio: 0.88x  →  Briev wins
  correctness: MATCH (output: "")

=== bit_clear ===
  Briev: 0.0003s  (min 0.000000s, max 0.000732s)
  C:     0.0005s
  Ratio: 0.60x  →  Briev wins
  correctness: MATCH (output: "0")

=== queue_drain ===
  Briev: 0.0875s  (min 0.084817s, max 0.088972s)
  C:     0.0648s
  Ratio: 1.35x  →  C wins
  correctness: MATCH (output: "")

=== queue_drain_sym ===
  Briev: 0.0319s  (min 0.030111s, max 0.033349s)
  C:     0.0682s
  Ratio: 0.47x  →  Briev wins
  correctness: MATCH (output: "")

=== queue_drain_idio ===
  Briev: 0.0338s  (min 0.031957s, max 0.034961s)
  C:     0.0750s
  Ratio: 0.45x  →  Briev wins
  correctness: MATCH (output: "")

=== stack_push_pop ===
  Briev: 0.0396s  (min 0.033915s, max 0.049971s)
  C:     0.0677s
  Ratio: 0.58x  →  Briev wins
  correctness: MATCH (output: "")

=== interval_step ===
  Briev: 0.0880s  (min 0.085652s, max 0.091956s)
  C:     0.0714s
  Ratio: 1.23x  →  C wins
  correctness: MATCH (output: "")

=== telemetry_stream ===
  Briev: 0.1623s  (min 0.160087s, max 0.165021s)
  C:     0.1896s
  Ratio: 0.86x  →  Briev wins
  correctness: MATCH (output: "")

=== pid_control ===
  Briev: 0.3494s  (min 0.347959s, max 0.350259s)
  C:     0.3512s
  Ratio: 0.99x  →  Briev wins
  correctness: MATCH (output: "")

=== matrix_pipeline ===
  Briev: 0.5929s  (min 0.577888s, max 0.613380s)
  C:     0.5966s
  Ratio: 0.99x  →  Briev wins
  correctness: MATCH (output: "")

=== accumulator_flush ===
  Briev: 0.1449s  (min 0.136904s, max 0.160517s)
  C:     0.1627s
  Ratio: 0.89x  →  Briev wins
  correctness: MATCH (output: "")

=== sweep_sparse ===
  Briev: 0.2052s  (min 0.202854s, max 0.206733s)
  C:     0.1582s
  Ratio: 1.30x  →  C wins
  correctness: MATCH (output: "")

=== sweep_mid ===
  Briev: 0.2405s  (min 0.227753s, max 0.251093s)
  C:     0.1856s
  Ratio: 1.30x  →  C wins
  correctness: MATCH (output: "")

=== sweep_dense ===
  Briev: 0.4323s  (min 0.422647s, max 0.445831s)
  C:     0.3794s
  Ratio: 1.14x  →  C wins
  correctness: MATCH (output: "")

=== sweep_arr ===
  Briev: 0.5118s  (min 0.494640s, max 0.522261s)
  C:     0.4543s
  Ratio: 1.13x  →  C wins
  correctness: MATCH (output: "")

=== series_converge ===
  Briev: 0.0003s  (min 0.000000s, max 0.000894s)
  C:     0.0003s
  Ratio: 1.00x  →  ~tie wins
  correctness: MATCH (output: "0.500050008")

=== global_lifetime ===
  Briev: 0.0370s  (min 0.035215s, max 0.041444s)
  C:     0.0918s
  Ratio: 0.40x  →  Briev wins
  correctness: MATCH (output: "")

=== deep_recursion ===
  Briev: 0.0003s  (min 0.000000s, max 0.000727s)
  C:     0.0004s
  Ratio: 0.75x  →  Briev wins
  correctness: MATCH (output: "15")

=== arena_churn ===
  Briev: 0.0331s  (min 0.031098s, max 0.034568s)
  C:     0.1153s
  Ratio: 0.29x  →  Briev wins
  correctness: MATCH (output: "")

=== linked_list ===
  Briev: 1.3636s  (min 1.249092s, max 1.427826s)
  C:     2.6517s
  Ratio: 0.51x  →  Briev wins
  correctness: MATCH (output: "")

=== hash_ops ===
  Briev: 1.7188s  (min 1.408164s, max 1.884183s)
  C:     2.3162s
  Ratio: 0.74x  →  Briev wins
  correctness: MATCH (output: "")

=== hash_ops_idio ===
  Briev: 0.8964s  (min 0.778698s, max 0.969387s)
  C:     0.8849s
  Ratio: 1.01x  →  C wins
  correctness: MATCH (output: "")

=== enemy_swarm ===
  Briev: 0.1343s  (min 0.124937s, max 0.142276s)
  C:     0.1779s
  Ratio: 0.75x  →  Briev wins
  correctness: MATCH (output: "")

=== recommender ===
  Briev: 0.0118s  (min 0.009903s, max 0.013311s)
  C:     0.0116s
  Ratio: 1.02x  →  C wins
  correctness: MATCH (output: "861
594
427
1103
34
1575")

=== bridge_glue ===
============================================================
GLUE Bridge vs C FFI — String Round-Trip Benchmark
============================================================

[Pure Python]
  native                median=     135ns  mean=     174ns  min=   118ns  max=    7645ns  result='42'

[C FFI]
  c_str_echo            median=   11666ns  mean=   10310ns  min=  6621ns  max=  373025ns  result='42'

[Briev GLUE]
  briev_cstr_rt         median=   11028ns  mean=   10402ns  min=  6394ns  max=  439430ns  result='42'

[Correctness]
  C:      '42'
  Briev:  '42'
  Native: '42'
  ✅ All match

=== bridge_multi ===
  Python ctypes + protocol, Node.js koffi + protocol, shell protocol
    =================================================================
    Multi-Language Bridge Benchmark — Briev export defn from Python
    =================================================================
    
    [Pure Python]
      native add                      median=     234ns  mean=     240ns  min=   126ns  max=  201190ns  result=7
    
    [Python ctypes]
      ctypes add                      median=    2163ns  mean=    1976ns  min=  1102ns  max=  113696ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median= 1116880ns  mean= 1545587ns  min=709319ns  max= 8477635ns  result=7
    
    [Correctness]
      ctypes:  7
      native:  7
      proto:   7
      ✅ All match
    =================================================================
    Multi-Language Bridge Benchmark — Briev export defn from Node.js
    =================================================================
    
    [Pure Node.js]
      native add                      median=     112ns  mean=163.61848ns  min=      58ns  max= 1326260ns  result=7
    
    [Node.js koffi FFI]
      koffi add                       median=     255ns  mean=357.4142ns  min=     176ns  max=  687016ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median= 2933179ns  mean=4107122.87ns  min= 1775050ns  max=22369500ns  result=7
    
    [Correctness]
      koffi:   7
      native:  7
      proto:   7
      ✅ All match
      proto_shim per-call latency (fork+exec+wait)
        avg=4814106ns  min=2637703ns  max=12268898ns  result=7

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
║ ring_buffer               ║ 0.0492s    ║ 0.0512s    ║ 0.96x    ║ Briev  ║ MATCH     ║
║ float_math                ║ 0.0437s    ║ 0.0483s    ║ 0.90x    ║ Briev  ║ MATCH     ║
║ float_math_nonzero        ║ 0.2049s    ║ 0.1653s    ║ 1.24x    ║ C      ║ MATCH     ║
║ sparse_dispatch           ║ 0.0452s    ║ 0.0596s    ║ 0.76x    ║ Briev  ║ MATCH     ║
║ print_loop                ║ 0.0287s    ║ 0.0600s    ║ 0.48x    ║ Briev  ║ MATCH     ║
║ nbody_newton              ║ 8.6723s    ║ 8.9709s    ║ 0.97x    ║ Briev  ║ MATCH     ║
║ nbody_newton_accel        ║ 2.0881s    ║ 0.1744s    ║ 11.97x   ║ C      ║ MATCH     ║
║ nbody_sqrt                ║ 2.8581s    ║ 3.9681s    ║ 0.72x    ║ Briev  ║ MATCH     ║
║ nbody_sqrt_idio           ║ 3.4580s    ║ 4.0443s    ║ 0.86x    ║ Briev  ║ MATCH     ║
║ fasta                     ║ 0.2007s    ║ 0.3463s    ║ 0.58x    ║ Briev  ║ MATCH     ║
║ fannkuch_redux            ║ 0.1075s    ║ 0.0972s    ║ 1.11x    ║ C      ║ MATCH     ║
║ mandelbrot                ║ 0.9258s    ║ 0.7688s    ║ 1.20x    ║ C      ║ MATCH     ║
║ kalman_filter_runtime     ║ 0.1975s    ║ 0.1836s    ║ 1.08x    ║ C      ║ MATCH     ║
║ knucleotide               ║ 0.2487s    ║ 0.2036s    ║ 1.22x    ║ C      ║ MATCH     ║
║ cancel_math               ║ 0.0602s    ║ 0.0687s    ║ 0.88x    ║ Briev  ║ MATCH     ║
║ bit_clear                 ║ 0.0003s    ║ 0.0005s    ║ 0.60x    ║ Briev  ║ MATCH     ║
║ queue_drain               ║ 0.0875s    ║ 0.0648s    ║ 1.35x    ║ C      ║ MATCH     ║
║ queue_drain_sym           ║ 0.0319s    ║ 0.0682s    ║ 0.47x    ║ Briev  ║ MATCH     ║
║ queue_drain_idio          ║ 0.0338s    ║ 0.0750s    ║ 0.45x    ║ Briev  ║ MATCH     ║
║ stack_push_pop            ║ 0.0396s    ║ 0.0677s    ║ 0.58x    ║ Briev  ║ MATCH     ║
║ interval_step             ║ 0.0880s    ║ 0.0714s    ║ 1.23x    ║ C      ║ MATCH     ║
║ telemetry_stream          ║ 0.1623s    ║ 0.1896s    ║ 0.86x    ║ Briev  ║ MATCH     ║
║ pid_control               ║ 0.3494s    ║ 0.3512s    ║ 0.99x    ║ Briev  ║ MATCH     ║
║ matrix_pipeline           ║ 0.5929s    ║ 0.5966s    ║ 0.99x    ║ Briev  ║ MATCH     ║
║ accumulator_flush         ║ 0.1449s    ║ 0.1627s    ║ 0.89x    ║ Briev  ║ MATCH     ║
║ sweep_sparse              ║ 0.2052s    ║ 0.1582s    ║ 1.30x    ║ C      ║ MATCH     ║
║ sweep_mid                 ║ 0.2405s    ║ 0.1856s    ║ 1.30x    ║ C      ║ MATCH     ║
║ sweep_dense               ║ 0.4323s    ║ 0.3794s    ║ 1.14x    ║ C      ║ MATCH     ║
║ sweep_arr                 ║ 0.5118s    ║ 0.4543s    ║ 1.13x    ║ C      ║ MATCH     ║
║ series_converge           ║ 0.0003s    ║ 0.0003s    ║ 1.00x    ║ ~tie   ║ MATCH     ║
║ global_lifetime           ║ 0.0370s    ║ 0.0918s    ║ 0.40x    ║ Briev  ║ MATCH     ║
║ deep_recursion            ║ 0.0003s    ║ 0.0004s    ║ 0.75x    ║ Briev  ║ MATCH     ║
║ arena_churn               ║ 0.0331s    ║ 0.1153s    ║ 0.29x    ║ Briev  ║ MATCH     ║
║ linked_list               ║ 1.3636s    ║ 2.6517s    ║ 0.51x    ║ Briev  ║ MATCH     ║
║ hash_ops                  ║ 1.7188s    ║ 2.3162s    ║ 0.74x    ║ Briev  ║ MATCH     ║
║ hash_ops_idio             ║ 0.8964s    ║ 0.8849s    ║ 1.01x    ║ C      ║ MATCH     ║
║ enemy_swarm               ║ 0.1343s    ║ 0.1779s    ║ 0.75x    ║ Briev  ║ MATCH     ║
║ recommender               ║ 0.0118s    ║ 0.0116s    ║ 1.02x    ║ C      ║ MATCH     ║
║ bridge_glue               ║ done       ║            ║          ║        ║ MATCH     ║
║ bridge_multi              ║ done       ║            ║          ║        ║ PASS      ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝
