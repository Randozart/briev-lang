
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
info: field(s) 'cr', 'ci', 't3', 't2', 't1' written by txn 'mb' are never read — wasted work
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
info: field(s) 'x', 'last' written by txn 'converge' are never read — stores eliminated
note: not referenced by any precondition or #!exit condition
info: txn 'converge' folded to O(1)
  info: counter 'i' retains its store (the only live write)
  info: dead fields: 'x', 'last'
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
    mkdir -p /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench
    cc -shared -fPIC -O2 -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libstr_prepend_c.so str_prepend.c
      C .so: /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libstr_prepend_c.so
    cd /home/randozart/Desktop/Projects/briev-lang && \
      /home/randozart/Desktop/Projects/briev-lang/target/release/brievc build pp-types.bv --llvm --out /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench 2>&1 && \
      llc -filetype=obj -relocation-model=pic \
        -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll 2>&1 && \
      cc -c -fPIC -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o lib/runtime/briev_rt.c 2>&1 && \
      cc -shared -fPIC -O2 -o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o && \
      rm -f /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.o /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/pp-types.ll /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/briev_rt.o
    warning: cross-op 'List([Identifier("cstring_concat"), HashL, HashR])' body not found, skipping equivalence check
    
    thread 'main' (611382) panicked at src/backend/llvm/mod.rs:5598:17:
    defn liveness: emitted code calls unreached defn(s) ["str_to_int"] — add the intrinsic→helper row in src/analysis/defn_liveness.rs (intrinsic_helpers) or root the caller; see docs/architecture/defn-liveness.md
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    make: *** [Makefile:41: /home/randozart/Desktop/Projects/briev-lang/target/bridge_bench/libpp_types.so] Error 101
    make: Leaving directory '/home/randozart/Desktop/Projects/briev-lang/benchmarks/bridge'
