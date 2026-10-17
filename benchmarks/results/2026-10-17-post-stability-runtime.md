
================================================
  Building: ring_buffer  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/float_math.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/float_math_nonzero.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/print_loop.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/print_loop
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: nbody_newton  (tag: runtime)
================================================
info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
info: field(s) 'last_energy' written by txn 'simulate' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
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
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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
info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/nbody_sqrt_idio.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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
info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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
info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
info: field(s) 'cr', 'ci', 't1', 't2', 't3' written by txn 'mb' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
wrote benchmarks/mandelbrot.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/kalman_filter_runtime.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/knucleotide.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/cancel_math.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/queue_drain.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/queue_drain_sym.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/queue_drain_idio.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
wrote benchmarks/queue_drain_idio
  Briev binary ready.
  No C source — uses queue_drain_sym_c for cross-reference timing
  BUILD-FAIL: queue_drain_idio_c (C reference — comparison skipped)

================================================
  Building: stack_push_pop  (tag: runtime)
================================================
warning: no path ends this program — it runs until the process is killed
  why:  walked the live program (reactive txns, entry nodes, ISR handlers, and their callees); no endprogram, halt, or trap is reachable
  fix:  end it where it settles — `when <done> { endprogram; };` — or declare open-ended residency: `when true { reside; };`

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/stack_push_pop.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/telemetry_stream.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/pid_control.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/matrix_pipeline.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1) — 0 iterations replaced by single store
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/sweep_sparse.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/sweep_mid.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/sweep_dense.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/sweep_arr.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
info: field(s) 'last', 'x' written by txn 'converge' are never read — stores eliminated
note: not referenced by any precondition or #!exit condition
info: txn 'converge' folded to O(1)
  info: counter 'i' retains its store (the only live write)
  info: dead fields: 'last', 'x'
wrote benchmarks/series_converge.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/global_lifetime.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/arena_churn.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
info: field(s) 'head' written by txn 'build' are never read — wasted work
note: not referenced by any precondition or #!exit; values computed but have no effect
wrote benchmarks/linked_list.ll
warning: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering [-Wpass-failed=transform-warning]
1 warning generated.
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

/usr/bin/ld: warning: LLVM gold plugin: <unknown>:0:0: loop not vectorized: the optimizer was unable to perform the requested transformation; the transformation might be disabled or specified as part of an unsupported transformation ordering
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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

info: program dispatched via direct SSA loop
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
wrote benchmarks/enemy_swarm.ll
/usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'

wrote benchmarks/enemy_swarm
  Briev binary ready.
  Building C reference...
  C binary ready.

================================================
  Building: recommender  (tag: runtime)
================================================
info: txn 'iter_take_loop' folded to O(1)
  info: counter 'i' retains its store (the only live write)
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
    info: txn 'iter_take_loop' folded to O(1)
      info: counter 'i' retains its store (the only live write)
    wrote /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll
    lib/runtime/briev_rt.c:175:40: warning: ‘always_inline’ function might not be inlinable unless also declared ‘inline’ [-Wattributes]
      175 | __attribute__((always_inline)) int64_t __print_float64(double f) {
          |                                        ^~~~~~~~~~~~~~~
      Briev .so: /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so
    -rwxr-xr-x 1 randozart randozart 32768 17 okt 10:46 /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'

================================================
  Building: bridge_multi  (tag: runtime)
================================================
  No .bv source — custom build
  bridge_multi: building Briev .so + protocol shim...
    make: Entering directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'
    cd /home/randozart/Desktop/Projects/briev-lang && \
      /home/randozart/Desktop/Projects/briev-lang/target/release/brievc build benchmarks/multi_lang/export_add.bv --shared --out /home/randozart/Desktop/Projects/briev-lang/target/multi_lang 2>&1
    info: txn 'iter_take_loop' folded to O(1)
      info: counter 'i' retains its store (the only live write)
    wrote /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/export_add.ll
    /usr/bin/ld: warning: LLVM gold plugin: Linking two modules of different target triples: '/home/randozart/.cache/briev-compiler/ffi/d76b1e7ebf20910aa66d2d38a2e34e5c2678ca600291d61c779c1e43c641ba68.o' is 'x86_64-pc-linux-gnu' whereas 'ld-temp.o' is 'x86_64-unknown-linux-gnu'
    
    wrote /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/export_add.so
    cc -O2 -o /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/proto_shim /home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang/proto_shim.c -ldl
      proto shim: /home/randozart/Desktop/Projects/briev-lang/target/multi_lang/proto_shim
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/multi_lang'

================================================
  RUNNING BENCHMARKS
================================================

=== ring_buffer ===
  Briev: 0.0598s  (min 0.055691s, max 0.062522s)
  C:     0.0498s
  Ratio: 1.20x  →  C wins
  correctness: MATCH (output: "9998976
19998976
29998976
39998976
49998")

=== float_math ===
  Briev: 0.1298s  (min 0.125863s, max 0.134695s)
  C:     0.0474s
  Ratio: 2.74x  →  C wins
  correctness: MATCH (output: "")

=== float_math_nonzero ===
  Briev: 0.1608s  (min 0.158628s, max 0.162278s)
  C:     0.1663s
  Ratio: 0.97x  →  Briev wins
  correctness: MATCH (output: "")

=== sparse_dispatch ===
  Briev: 0.0461s  (min 0.044564s, max 0.048054s)
  C:     0.0619s
  Ratio: 0.74x  →  Briev wins
  correctness: MATCH (output: "")

=== print_loop ===
  Briev: 0.0311s  (min 0.029605s, max 0.032422s)
  C:     0.0600s
  Ratio: 0.52x  →  Briev wins
  correctness: MATCH (output: "")

=== nbody_newton ===
  Briev: 7.6345s  (min 7.437940s, max 7.970369s)
  C:     6.9898s
  Ratio: 1.09x  →  C wins
  correctness: MATCH (output: "-0.169203341")

=== nbody_newton_accel ===
# gpu_time: 0.003 ms
# gpu_time: 0.009 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.002 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
# gpu_time: 0.003 ms
  Briev: 1.6938s  (min 1.366883s, max 2.558984s)
  C:     0.1314s
  Ratio: 12.89x  →  C wins
  correctness: MATCH (output: "0.50002861")

=== nbody_sqrt ===
  Briev: 2.1669s  (min 2.111237s, max 2.273069s)
  C:     3.3661s
  Ratio: 0.64x  →  Briev wins
  correctness: MATCH (output: "-0.169288844")

=== nbody_sqrt_idio ===
  Briev: 3.3793s  (min 3.242097s, max 3.521529s)
  C:     3.1798s
  Ratio: 1.06x  →  C wins
  correctness: MATCH (output: "-0.169289678")

=== fasta ===
  Briev: 0.2275s  (min 0.224473s, max 0.229874s)
  C:     0.2653s
  Ratio: 0.86x  →  Briev wins
  correctness: MATCH (output: "xqjqf")

=== fannkuch_redux ===
  Briev: 0.2639s  (min 0.258359s, max 0.270140s)
  C:     0.0590s
  Ratio: 4.47x  →  C wins
  correctness: MATCH (output: "10")

=== mandelbrot ===
  Briev: 0.8715s  (min 0.866220s, max 0.878670s)
  C:     0.6969s
  Ratio: 1.25x  →  C wins
  correctness: MATCH (output: "73
108")

=== kalman_filter_runtime ===
  Briev: 0.3346s  (min 0.333238s, max 0.335930s)
  C:     0.1808s
  Ratio: 1.85x  →  C wins
  correctness: MATCH (output: "")

=== knucleotide ===
  Briev: 0.2327s  (min 0.230879s, max 0.233857s)
  C:     0.1958s
  Ratio: 1.19x  →  C wins
  correctness: MATCH (output: "3")

=== cancel_math ===
  Briev: 0.0848s  (min 0.083031s, max 0.085745s)
  C:     0.0609s
  Ratio: 1.39x  →  C wins
  correctness: MATCH (output: "")

=== bit_clear ===
  Briev: 0.0003s  (min 0.000000s, max 0.000563s)
  C:     0.0004s
  Ratio: 0.75x  →  Briev wins
  correctness: MATCH (output: "0")

=== queue_drain ===
  Briev: 0.0859s  (min 0.075495s, max 0.106095s)
  C:     0.0625s
  Ratio: 1.37x  →  C wins
  correctness: MATCH (output: "")

=== queue_drain_sym ===
  Briev: 0.0312s  (min 0.030020s, max 0.032037s)
  C:     0.0612s
  Ratio: 0.51x  →  Briev wins
  correctness: MATCH (output: "")

=== stack_push_pop ===
  Briev: 0.0450s  (min 0.043344s, max 0.046708s)
  C:     0.0629s
  Ratio: 0.72x  →  Briev wins
  correctness: MATCH (output: "")

=== interval_step ===
  Briev: 0.0837s  (min 0.082999s, max 0.084198s)
  C:     0.0629s
  Ratio: 1.33x  →  C wins
  correctness: MATCH (output: "")

=== telemetry_stream ===
  Briev: 0.1586s  (min 0.157417s, max 0.159161s)
  C:     0.1710s
  Ratio: 0.93x  →  Briev wins
  correctness: MATCH (output: "")

=== pid_control ===
  Briev: 0.3468s  (min 0.344341s, max 0.349398s)
  C:     0.3482s
  Ratio: 1.00x  →  ~tie wins
  correctness: MATCH (output: "")

=== matrix_pipeline ===
  Briev: 0.4907s  (min 0.474599s, max 0.520723s)
  C:     0.4927s
  Ratio: 1.00x  →  ~tie wins
  correctness: MATCH (output: "")

=== accumulator_flush ===
  Briev: 0.1369s  (min 0.132411s, max 0.141834s)
  C:     0.1465s
  Ratio: 0.93x  →  Briev wins
  correctness: MATCH (output: "")

=== sweep_sparse ===
  Briev: 0.1504s  (min 0.148075s, max 0.151860s)
  C:     0.1562s
  Ratio: 0.96x  →  Briev wins
  correctness: MATCH (output: "")

=== sweep_mid ===
  Briev: 0.2296s  (min 0.227825s, max 0.235059s)
  C:     0.1711s
  Ratio: 1.34x  →  C wins
  correctness: MATCH (output: "")

=== sweep_dense ===
  Briev: 0.3839s  (min 0.379496s, max 0.393641s)
  C:     0.3728s
  Ratio: 1.03x  →  C wins
  correctness: MATCH (output: "")

=== sweep_arr ===
  Briev: 0.4477s  (min 0.440366s, max 0.455134s)
  C:     0.4403s
  Ratio: 1.02x  →  C wins
  correctness: MATCH (output: "")

=== series_converge ===
  Briev: 0.0004s  (min 0.000000s, max 0.000613s)
  C:     0.0002s
  Ratio: 2.00x  →  C wins
  correctness: MISMATCH
    briev: ""
    c:     "0.500050008"

=== global_lifetime ===
  Briev: 0.0466s  (min 0.043940s, max 0.049074s)
  C:     0.0696s
  Ratio: 0.67x  →  Briev wins
  correctness: MATCH (output: "")

=== deep_recursion ===
  Briev: 0.0004s  (min 0.000000s, max 0.000801s)
  C:     0.0002s
  Ratio: 2.00x  →  C wins
  correctness: MATCH (output: "15")

=== arena_churn ===
  Briev: 0.0860s  (min 0.084729s, max 0.087724s)
  C:     0.0998s
  Ratio: 0.86x  →  Briev wins
  correctness: MATCH (output: "")

=== linked_list ===
  Briev: 1.0818s  (min 1.022376s, max 1.125185s)
  C:     1.9594s
  Ratio: 0.55x  →  Briev wins
  correctness: MATCH (output: "")

=== hash_ops ===
  Briev: 0.9513s  (min 0.874266s, max 0.989373s)
  C:     1.0474s
  Ratio: 0.91x  →  Briev wins
  correctness: MATCH (output: "")

=== hash_ops_idio ===
  Briev: 0.8024s  (min 0.788009s, max 0.826482s)
  C:     0.7064s
  Ratio: 1.14x  →  C wins
  correctness: MATCH (output: "")

=== enemy_swarm ===
  Briev: 0.1178s  (min 0.111821s, max 0.123543s)
  C:     0.1302s
  Ratio: 0.90x  →  Briev wins
  correctness: MATCH (output: "")

=== recommender ===
  Briev: 0.0140s  (min 0.011729s, max 0.016318s)
  C:     0.0134s
  Ratio: 1.04x  →  C wins
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
  native                median=     152ns  mean=     158ns  min=   127ns  max=    8993ns  result='42'

[C FFI]
  c_str_echo            median=    6699ns  mean=    7125ns  min=  6268ns  max=  347504ns  result='42'

[Briev GLUE]
  briev_cstr_rt         median=    6739ns  mean=    7200ns  min=  6357ns  max=   85894ns  result='42'

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
      native add                      median=     169ns  mean=     181ns  min=   151ns  max=    9584ns  result=7
    
    [Python ctypes]
      ctypes add                      median=    1115ns  mean=    1266ns  min=  1022ns  max=   16918ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median=  644998ns  mean=  655943ns  min=536671ns  max=  871897ns  result=7
    
    [Correctness]
      ctypes:  7
      native:  7
      proto:   7
      ✅ All match
    =================================================================
    Multi-Language Bridge Benchmark — Briev export defn from Node.js
    =================================================================
    
    [Pure Node.js]
      native add                      median=      68ns  mean=90.98352ns  min=      56ns  max=  728607ns  result=7
    
    [Node.js koffi FFI]
      koffi add                       median=     135ns  mean= 155.999ns  min=     125ns  max=   12942ns  result=7
    
    [Protocol Bridge (subprocess)]
      proto_shim add                  median= 1725102ns  mean=1765499.11ns  min= 1436998ns  max= 2647735ns  result=7
    
    [Correctness]
      koffi:   7
      native:  7
      proto:   7
      ✅ All match
      proto_shim per-call latency (fork+exec+wait)
        avg=2260513ns  min=1981630ns  max=2799113ns  result=7

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
║ ring_buffer               ║ 0.0598s    ║ 0.0498s    ║ 1.20x    ║ C      ║ MATCH     ║
║ float_math                ║ 0.1298s    ║ 0.0474s    ║ 2.74x    ║ C      ║ MATCH     ║
║ float_math_nonzero        ║ 0.1608s    ║ 0.1663s    ║ 0.97x    ║ Briev  ║ MATCH     ║
║ sparse_dispatch           ║ 0.0461s    ║ 0.0619s    ║ 0.74x    ║ Briev  ║ MATCH     ║
║ print_loop                ║ 0.0311s    ║ 0.0600s    ║ 0.52x    ║ Briev  ║ MATCH     ║
║ nbody_newton              ║ 7.6345s    ║ 6.9898s    ║ 1.09x    ║ C      ║ MATCH     ║
║ nbody_newton_accel        ║ 1.6938s    ║ 0.1314s    ║ 12.89x   ║ C      ║ MATCH     ║
║ nbody_sqrt                ║ 2.1669s    ║ 3.3661s    ║ 0.64x    ║ Briev  ║ MATCH     ║
║ nbody_sqrt_idio           ║ 3.3793s    ║ 3.1798s    ║ 1.06x    ║ C      ║ MATCH     ║
║ fasta                     ║ 0.2275s    ║ 0.2653s    ║ 0.86x    ║ Briev  ║ MATCH     ║
║ fannkuch_redux            ║ 0.2639s    ║ 0.0590s    ║ 4.47x    ║ C      ║ MATCH     ║
║ mandelbrot                ║ 0.8715s    ║ 0.6969s    ║ 1.25x    ║ C      ║ MATCH     ║
║ kalman_filter_runtime     ║ 0.3346s    ║ 0.1808s    ║ 1.85x    ║ C      ║ MATCH     ║
║ knucleotide               ║ 0.2327s    ║ 0.1958s    ║ 1.19x    ║ C      ║ MATCH     ║
║ cancel_math               ║ 0.0848s    ║ 0.0609s    ║ 1.39x    ║ C      ║ MATCH     ║
║ bit_clear                 ║ 0.0003s    ║ 0.0004s    ║ 0.75x    ║ Briev  ║ MATCH     ║
║ queue_drain               ║ 0.0859s    ║ 0.0625s    ║ 1.37x    ║ C      ║ MATCH     ║
║ queue_drain_sym           ║ 0.0312s    ║ 0.0612s    ║ 0.51x    ║ Briev  ║ MATCH     ║
║ queue_drain_idio          ║ BUILD-FAIL ║            ║          ║        ║ SKIP      ║
║ stack_push_pop            ║ 0.0450s    ║ 0.0629s    ║ 0.72x    ║ Briev  ║ MATCH     ║
║ interval_step             ║ 0.0837s    ║ 0.0629s    ║ 1.33x    ║ C      ║ MATCH     ║
║ telemetry_stream          ║ 0.1586s    ║ 0.1710s    ║ 0.93x    ║ Briev  ║ MATCH     ║
║ pid_control               ║ 0.3468s    ║ 0.3482s    ║ 1.00x    ║ ~tie   ║ MATCH     ║
║ matrix_pipeline           ║ 0.4907s    ║ 0.4927s    ║ 1.00x    ║ ~tie   ║ MATCH     ║
║ accumulator_flush         ║ 0.1369s    ║ 0.1465s    ║ 0.93x    ║ Briev  ║ MATCH     ║
║ sweep_sparse              ║ 0.1504s    ║ 0.1562s    ║ 0.96x    ║ Briev  ║ MATCH     ║
║ sweep_mid                 ║ 0.2296s    ║ 0.1711s    ║ 1.34x    ║ C      ║ MATCH     ║
║ sweep_dense               ║ 0.3839s    ║ 0.3728s    ║ 1.03x    ║ C      ║ MATCH     ║
║ sweep_arr                 ║ 0.4477s    ║ 0.4403s    ║ 1.02x    ║ C      ║ MATCH     ║
║ series_converge           ║ 0.0004s    ║ 0.0002s    ║ 2.00x    ║ C      ║ MISMATCH  ║
║ global_lifetime           ║ 0.0466s    ║ 0.0696s    ║ 0.67x    ║ Briev  ║ MATCH     ║
║ deep_recursion            ║ 0.0004s    ║ 0.0002s    ║ 2.00x    ║ C      ║ MATCH     ║
║ arena_churn               ║ 0.0860s    ║ 0.0998s    ║ 0.86x    ║ Briev  ║ MATCH     ║
║ linked_list               ║ 1.0818s    ║ 1.9594s    ║ 0.55x    ║ Briev  ║ MATCH     ║
║ hash_ops                  ║ 0.9513s    ║ 1.0474s    ║ 0.91x    ║ Briev  ║ MATCH     ║
║ hash_ops_idio             ║ 0.8024s    ║ 0.7064s    ║ 1.14x    ║ C      ║ MATCH     ║
║ enemy_swarm               ║ 0.1178s    ║ 0.1302s    ║ 0.90x    ║ Briev  ║ MATCH     ║
║ recommender               ║ 0.0140s    ║ 0.0134s    ║ 1.04x    ║ C      ║ MATCH     ║
║ bridge_glue               ║ done       ║            ║          ║        ║ MATCH     ║
║ bridge_multi              ║ done       ║            ║          ║        ║ PASS      ║
╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝

================================================
  BUILD FAILURES (1)
================================================
  - queue_drain_idio:C
  (runs skipped for failed builds; suite still completed.)
