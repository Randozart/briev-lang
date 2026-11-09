# 14. Process Boundaries and GPU Deferral (`endprogram`, `beginprogram`, `accel`)

Three keywords control the program's process boundary and where its work runs.

## `endprogram` — exit the process

`endprogram` completes the process boundary — it genuinely exits, unlike
`term`, which only ends the current transaction.

```briev
node report [count == bound][true] {
    endprogram println!(px[0]);   // print, then exit 0
};

node fail [bad][true] {
    endprogram 2;                 // exit with code 2
};
```

- `endprogram;` exits 0; `endprogram code;` exits with `code`.
- Runs `defer` cleanup via the runtime (`__exit` in `briev_rt.c`).
- A node whose precondition stays true cannot re-fire after `endprogram` —
  the process is gone. This is why `report [count == bound][true]` above is
  safe: the first firing exits before the precondition re-checks.
- `endprogram` replaced `exit program` (2026-08-06), which had been conflated
  with `term` in every backend — a node with an always-true precondition used
  to loop forever.

## `beginprogram` — the program entry (entry loop)

`beginprogram` is an optional precondition marker, true exactly once at
program start. A node `[beginprogram && <state>][<goal>]` is an **entry
loop**: it is entered once when its state conditions hold, then the body
loops until the goal. The precondition is evaluated once at entry, never
re-checked during the loop.

```briev
let startingnumber: Int = get_env_int!("env_var");
let i: Int = 0;

node entry [beginprogram && startingnumber == 1][i == 4] {
    i = i + 1;
    term;
};
```

The compiler enforces, at compile time:
- **Goal reachability** — the body must provably progress toward the goal
  (a counter the body advances, or `[true]` for a single pass). An
  unreachable goal is a compile error.
- **Entry conflict** — at most one `beginprogram` node may be eligible at
  program start; overlapping entry conditions are a compile error.

`beginprogram` is scoped to `node`. It replaces the implicit "whichever node
fires first" entry with an explicit one.

## `accel` — defer a counted loop to the GPU

`accel` marks a native counted loop as a **parallel map over work-items**.
The work-item index is a real state counter — no virtual variables.

```briev
let i: Int = 0;
accel node force [i < nb][i == nb] {
    dv[i] = force_on(i);     // per-work-item compute (disjoint affine write)
    i = i + 1;               // counted-loop advance
    term;
};
```

- `[i < N]` is the loop bound; `[i == N]` is the goal ("loop until true").
- The compiler **proves** the map (disjoint per-`i` writes, counter advance,
  pure, flat types). If the proof fails or the speedup is unverified, a
  keyword-marked body stays on the CPU path and always emits a one-line
  compile-time remark naming the reason — never silent. Verbose diagnostics
  add full per-analysis detail.
- On a GPU it launches N work-items once and fast-forwards the counter; on
  CPU the counted loop runs natively (each firing = one work-item).
- **Verifiable speedup only**: in `try` mode the compiler verifies the GPU
  path is faster (statically for known N, or via a runtime auto-tuning probe
  that measures both lanes and checks output equality) before deferring.
- Policy is the compiler mode: `--accel=try-all` makes every eligible body a
  candidate; `--accel=force` requires keyword-marked bodies to offload (errors
  on ineligible); `--accel=try-all-force` combines both. `.abv` files default
  to `try-all`. See SPEC §9.7.

```briev
// build with --accel=try-all

let i: Int = 0;
accel node init_bodies [beginprogram && i < nb][i == nb] {
    px[i] = i as Float * 0.1 + 0.5;
    i = i + 1;
    term;
};
```

Here `beginprogram` marks the entry, and `accel` offloads the init pass.

## Keeping tiny shapes on the CPU (`--accel-cpu-fallback N`)

```bash
brievc build app.bv --accel-cpu-fallback 8
```

Shapes whose work-item count is below `N` run the native CPU counted loop
instead of dispatching to the GPU. Useful when very small dispatches are
common: a sub-8-workgroup launch cannot amortize its own cost, and some
drivers mishandle it. The threshold applies to the `.bv` offload lane
only — `.abv` files are pure GPU by design and never fall back. Const
bounds fold the check at compile time; runtime bounds check per call.

The `.abv` portfolio files under `examples/gpu/` show the supported
kernel shapes end to end (`gemv`, `gemm`, `reduce`, `gather_8`,
`softmax`, `saxpy`).

## What the GPU compiler builds from your loop (2026-09-01)

You write the loop; the compiler picks the kernel. Three forms exist —
picked by the proven shape, never by a keyword you write:

**1. Plain work-item kernel** — every eligible body gets at least this:
one invocation per work-item.

**2. Cooperative row kernel** — for dot-product reductions like GEMV:

```briev
let acc: Float = 0;
foreach k in 0..K {
    acc = acc + a[i * K + k] * x[k];
}
y[i] = acc;
```

Each 32-lane workgroup owns one row; lanes accumulate strided elements
and one subgroup reduce produces the sum. Requirements: the counter `i`
appears BARE as the row (no `i / N`, no `i % N` anywhere), `K` is a
literal multiple of 32, and `a`'s projection offset is 16-byte aligned
(then loads are wide float4 — `x` joins automatically when IT is aligned
too; otherwise it stays scalar and the kernel is still exact).

**3. Tiled GEMM** — for a decomposed counter over a matmul body:

```briev
let m: Int = i / N;
let n: Int = i % N;
foreach k in 0..K {
    acc = acc + a[m * K + k] * b[k * N + n];
}
y[i] = acc;
```

The compiler replaces this with a shared-memory tiled kernel (64×64
tiles, 16×16 workgroups, 4×4 register tiles per invocation, barriers
between panels). Requirements: `M`, `N`, `K` all literal multiples of 64
and the body in exactly the canonical form above. Anything else takes the
plain kernel — correct, just slower. Measured at 4096³: 25.3ms (5250
GFLOP/s) vs 6717ms naive.

**Float16 means tensor cores.** Declare your operands
`Float16[K * M]`-style (or accept the precision) and, on devices with
`VK_KHR_cooperative_matrix`, the GEMM lowers to tensor-core fragments
(fp16 in, fp32 accumulate). Float32 operands keep the exact tiled kernel
— consumer-GPU tensor shapes have no f32×f32 mode, so precision is YOUR
call, expressed as a type.

**Launch cost matters at small sizes.** A synchronous launch costs ~40µs
on this class of device regardless of kernel size; a 64-row GEMV compute
is microseconds. Loop deployments should batch (`launch_resident_batch`):
K launches in one submission ≈ kernel time per call. The portfolio files
under `examples/gpu/` show the working shapes (`gemv.abv`, `gemv_m64.abv`,
`gemm.abv`).
