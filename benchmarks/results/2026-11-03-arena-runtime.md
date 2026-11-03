# Arena runtime measurement — 2026-11-03

Companion: `docs/plans/2026-11-03-arena-runtime-parity-and-folding.md`.

## Observable benchmark (arena vs arena)

`benchmarks/arena_churn_sym.bv` — bump arena, **volatile** store/load so the
memory work is observable (no store→load forwarding / dead-allocation fold).
Mirrors `benchmarks/arena_churn_sym_c.c`.

`BOUND=50_000_000`, interleaved ×5, same host, same output
(`12499997500000` checked at `BOUND=5_000_001`).

| run | Briev arena | C arena |
|---|---:|---:|
| 1 | 339 ms | 240 ms |
| 2 | 310 ms | 224 ms |
| 3 | 319 ms | 244 ms |
| 4 | 332 ms | 230 ms |
| 5 | 321 ms | 230 ms |
| **mean** | **~324 ms** | **~234 ms** |

**Briev is ~1.38x C.** Root cause: per allocation, the arena round-trips
`%state` (load ptr/end, store ptr) plus the lazy-init base check. A stored
value through the allocated pointer may alias `%state`, so LLVM cannot keep
the bump in a register. C keeps `used`/`buf`/`cap` in registers.

## Foldable variant (the "prove it away" path)

`benchmarks/arena_churn.bv` (non-volatile) is foldable: with the baseline
compiler it scaled sublinearly (18 ms at 50M) because the allocation was
proven dead. With the current arena codegen the bump's `%state` stores
prevent the fold (linear, ~324 ms). Preserving the fold is workstream O
(dead-allocation elimination).

## Next (R)

Register-resident bump/end (local cursor promoted by SROA, cold out-of-line
grow, scope init/reset) — target ≤ C (~234 ms) and ideally below.

## R2 investigation (2026-11-03): the memory round-trip was not the gap

A local arena cursor (entry allocas, gated to inlined mains with no
arena-using callees) was implemented and measured: **no meaningful change**
(~338 ms vs ~343 ms pre-change, within noise). Inspecting the `-O3` IR showed
why: LLVM's SROA already splits the inlined main's `%State` alloca and promotes
the arena ptr/end/base fields to loop-carried **phis** (`state.sroa.*`), with
zero `alloca i64` remaining. The local-cursor change was therefore **reverted**
(accidental complexity).

The real per-iteration overhead is control flow and address conversion:
- lazy-init check (`aalz`/`aalneed`) and capacity check (`aaok`) branches every
  allocation;
- `i64` ⇄ `ptr` conversions around the bump (inttoptr / getelementptr /
  ptrtoint) each iteration.

**Next lever (proof-licensed):** for a contract-bounded loop, reserve
`N * K` bytes once before the loop (N = bound register, K = constant alloc per
iteration) and **hoist the capacity check out of the loop**; hoist arena init
to the preheader; keep the cursor pointer-typed in registers to avoid the
conversions. This is the "if Briev can prove it, it should" optimization and
the path to ≤ C.

## Correction (2026-11-03, fair-stride comparison)

The original `arena_churn_sym_c.c` allocated **8-byte** slots while the .bv
(`Alloc#(64)`) bumps **64 bytes** per iteration — an 8x stride mismatch, so the
first Briev-vs-C ratio was not like-for-like. The C reference now allocates
64 bytes/iteration (same stride, same volatile store/load).

At BOUND=50M, same output:

| variant | time |
|---|---:|
| Briev, current arena (reset-on-grow **reuse**) | ~315–334 ms |
| C, 64-byte stride, **growing** arena | ~1345–1395 ms |
| Briev, grow-preserving (reservation experiment) | ~1270 ms |

So:
- Against a **correct growing** arena (no reuse), Briev is at **parity** with C
  (~1270 vs ~1370 ms).
- Briev's current ~320 ms comes from **reset-on-grow reuse** — it overwrites
  earlier arena allocations when it grows. That is a **latent soundness bug**
  (any live arena allocation spanning a grow could be corrupted) and it is not
  the same behavior as C's growing arena.

The reservation experiment (reserve N*K, bump unchecked) regressed to
~1270 ms because it grows to N*64 = 3.2 GB with no reuse (cache-hostile);
reverted.

**Conclusion:** runtime parity for a correct arena is met; the fast reuse must
be made *sound* via scope-lifetime proofs (reset at scope boundaries only when
allocations are proven not to cross the scope), which is the next workstream.

## Sound scope-lifetime reset (implemented 2026-11-03)

Grow now preserves the bump (sound). Per-iteration arena reset is emitted only
when `arena_scope_reset_safe` proves every `Alloc#` in the body is a body-local
let used only by non-retaining intrinsics; an allocation whose pointer reaches
an outer variable rejects the proof and takes the grow path.

| variant | time (BOUND=50M) | notes |
|---|---:|---|
| `arena_churn_sym`, safe body | **~35–41 ms** | per-iteration reset, 64 KB hot |
| C, 64-byte growing arena | ~1370 ms | no reuse |
| escaping variant (proof rejected) | correct | grows; pointer preserved |

The ~35 ms is legitimate: the proof establishes the allocation cannot outlive
the iteration, so resetting to base is sound reuse (not an unsound shortcut).
Regression: `arena_scope_reset_gate` — reset present for the safe body, absent
for the escaping one, both sums exact.

Follow-up: the reset is wired into the countdown emitter only; extend it to the
SSA/version-DAG inlined loop emitters.
