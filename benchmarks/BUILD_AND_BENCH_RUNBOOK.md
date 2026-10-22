# Benchmark Runbook

Run this before and after LLVM backend changes to detect regressions.

## Prerequisites

- Baseline worktree at `../briev-compiler-baseline` (locked to
  `compiler-baseline-2026-10-21-carrier`, commit `5a1a0532`; see
  `benchmarks/BASELINE.lock`)
- Both worktrees built with `cargo build --release`

## Steps

### 1. Build the baseline (first time only)

```bash
cd ../briev-compiler-baseline
cargo build --release
```

### 2. Build current

```bash
cargo build --release
```

### 3. Correctness check (fast — no timing)

```bash
bash benchmarks/build_and_bench.sh --correctness
```

### 4. Runtime benchmarks (timed against C reference)

```bash
bash benchmarks/build_and_bench.sh --runtime
```

### 5. Compare against baseline

```bash
bash benchmarks/compare_baseline.sh <benchmark_name>
```

`compare_baseline.sh` verifies the locked baseline commit before timing. An
intentional alternate baseline requires `ALLOW_BASELINE_MISMATCH=1`.

## Policy

- If a benchmark fails correctness: **stop and fix** — the compiler is producing wrong output.
- If a benchmark is slower: **do not revert** — discuss the best codegen for that pattern.
  The DAG analysis and Memory by Proof system should inform the correct strategy.
- If a benchmark is faster: note it and move on.

## Common benchmarks

| Name | Category | Notes |
|------|----------|-------|
| `nbody_newton` | runtime | Gravity simulation, float-heavy |
| `mandelbrot` | runtime | Integer iteration, tight loop |
| `matmul` | runtime | Matrix multiply, memory-heavy |
| `sieve` | runtime | Prime sieve, branch-heavy |
| `xxhash` | runtime | Hash computation, ALU-heavy |
