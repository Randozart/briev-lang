# Verification Kernels — Research Findings (2026-11-15)

## Context
User asked: *"Would it be unreasonable to have a suite of verification kernels?"*

## What "kernel" means in this codebase

"Briev kernel" has three distinct meanings depending on context:

| sense | description | examples |
|-------|-------------|----------|
| **hardware kernel** | Bootstrap/entry-point system, machine-wired nodes | `bootstrap node`, `node @ vector`, `kernel_rv64.b.bv` |
| **GPU compute kernel** | GPU offload targets with performance benchmarks | GEMM, attention, cooperative-row dot-product plans |
| **verification kernel** (potential) | A pre-verified set of proof obligations (contracts/lemmas) forming a stdlib for the proof engine | not yet implemented |

The "verification" prefix on "kernels" most naturally reads as: **a standard library of contracts and lemmas that the proof engine can reuse without re-running Z3** — building on the proof-result cache just implemented.

## Current verification architecture

```
prove_contract()     [proof_engine/mod.rs]  — entry, pre/post satisfiability
  smt_discharge()    [briev-verify/verify.rs] — Z3-driven verification
  prove_smt()        [briev-verify/smt.rs]     — Expr → SMT-LIB → Z3
  prove_smt_formula()[briev-verify/smt.rs]     — String → SMT-LIB → Z3

BODY verification   [briev-verify/verify.rs]  — tri-state: Holds/Violated/Undecided
Proof ledger       [proof_engine/ledger.rs]   — tracks family × consumer
```

The proof-result cache (`PROOF_CACHE`) was just implemented — it caches Z3 query results keyed by SMT-LIB string, version-invalidated. A "verification kernels" suite would be the **layer above**: known-correct proof obligations (contracts/lemmas) that are pre-verified and cached.

## What a verification kernels suite could be

### Option A: Proven stdlib of contracts (highest value, clear scope)
A set of `lemma_properties:` declarations and proven contracts in the stdlib that:
- Cover the fundamental invariants of `Array`, `Vector`, `Map`, `Iterator`, etc.
- Are verified once (per Z3 version) and stored in a structured kernel file
- Are loaded by the proof engine before running the solver
- The proof engine matches contract patterns against known-correct kernels instead of re-running Z3

**Pros:** Deterministic, fast, leverages the existing `PROOF_CACHE`; correctness guaranteed by matching Z3 results
**Cons:** Requires maintaining kernel files across Z3 upgrades; "proved once" could silently drift if stdlib contracts change

### Option B: Verified GPU kernel reference suite
A corpus of GPU kernels (GEMM, attention, softmax) with:
- A C/Python reference implementation
- A Briev port with proof obligations
- A correctness gate that verifies Briev output matches reference at the profiled geometry
- This already exists informally in `benchmarks/results/` — the question is whether it should be formalized as a "kernel suite"

**Pros:** Already proven valuable in practice (the GEMM comparison documents); correctness gates ensure no silent regressions
**Cons:** More about performance correctness than proof obligation correctness; overlaps with existing benchmark corpus

### Option C: Proof obligation kernels (pure)
A set of primitive proof lemmas that are proven once and added to the constraint system as axioms:
```
kernel: associative_reduce
lemma: reduce_assoc(x, y, z) ≡ reduce(reduce(x, y), z) == reduce(x, reduce(y, z))
proved_by: induction_on_length
```

**Pros:** Genuinely new capability; enables the proof engine to cite previously proved lemmas
**Cons:** Requires a proof-citation system (the ledger tracks consumers but not citations); high complexity, unclear ROI

## Recommendation

**Option A (proven stdlib contracts)** is the most natural successor to the proof-caching work and the most valuable. The implementation path:

1. Audit `lib/std/` for contracts that appear frequently in proof obligations (array bounds, iterator validity, map key existence)
2. Define a `.bvk` (Briev VerifKernels) file format: known-correct query-result pairs indexed by SMT-LIB string hash
3. Write a `load_verification_kernels(path)` that seeds `PROOF_CACHE` from a pre-computed kernel file
4. Run `BRIEF_PROVE_KERNEL_DUMP=1 brievc --verify-bodies std/` to produce the kernel file for the current Z3 version
5. The proof engine loads kernels at startup; mismatched Z3 version triggers recompute with `BRIEV_DIAG_PROVE` warning

**Option B** (verified GPU kernels) already has a de facto suite in the benchmark results — could be formalized but isn't primarily a proof-engine concern.

## Open questions for user

1. **What meaning of "verification kernels" did you have in mind?** The proven-stdlib-contracts interpretation (A) is my best guess given the context of the proof-caching work.
2. **Should Option A use persistent kernel files** (serialized on disk, loaded on startup) **or just extend the in-process cache** (the cache is already the "kernel store")?
3. **Is the goal correctness** (no silent proof failures) **or performance** (avoid re-running Z3)?

## Files consulted
- `src/proof_engine/mod.rs` — prove_contract entry point
- `briev-verify/src/verify.rs` — tri-state body prover
- `briev-verify/src/smt.rs` — SMT integration + PROOF_CACHE (just implemented)
- `src/proof_engine/ledger.rs` — proof consumption ledger
- `docs/plans/2026-09-01-cooperative-row-kernels.md` — GPU kernel concept
- `docs/plans/2026-09-14-bootstrap-kernel.md` — hardware kernel concept
- `archive/plans/active/KERNEL_TARGET_PLAN.md` — kernel-space verification (archived)
- `docs/plans/INDEX.md` — current live plans index
