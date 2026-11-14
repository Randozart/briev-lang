# Proof Result Caching: Findings and Implementation Plan

**Date:** 2026-11-14
**Status:** Research-complete; ready to implement
**Output:** Markdown plan file (no code changes in this session)

---

## 1. Current Architecture

### 1.1 The three SMT call sites

All Z3 invocations in the codebase are routed through `briev-verify/src/smt.rs` (`prove_smt_formula`). There are exactly three call sites:

| Site | Module | Function | Purpose | Calls per item |
|------|--------|----------|---------|----------------|
| **A** | `src/proof_engine/mod.rs` | `prove_contract` | Compile gate: pre/post satisfiability | 1 per contract |
| **B** | `briev-verify/src/verify.rs` | `smt_discharge` | Body verification: Z3 escalation (C4) | ≤1 per symbolic path the symbolic prover cannot decide |
| **C** | `src/analysis/protocol_graph.rs` | `prove_composition_inverse` | Cast round-trip identity | O(n²) over protocol variant pairs |

**Key observation:** Sites A and B use the same underlying `prove_smt_formula(&query, 1000)` call. Site C uses hand-written SMT-LIB from `build_roundtrip_smt` / `build_crossop_smt`.

### 1.2 The Z3 subprocess interface

```rust
// briev-verify/src/smt.rs — each call spawns a new Z3 process
pub fn prove_smt_formula(formula: &str, _timeout_ms: u64) -> SmtResult {
    let Some(bin) = z3_bin() else { return SmtResult::Unknown; };
    match Command::new(&bin).arg("-in").arg("-smt2")... {
        Ok(mut child) => {
            writeln!(stdin, "{}", query)?;
            drop(child.stdin.take()); // flush EOF
            match child.wait_with_output() {
                Ok(output) => parse_solver_output(&String::from_utf8_lossy(&output.stdout)),
                Err(_) => SmtResult::Unknown,
            }
        }
        Err(_) => SmtResult::Unknown,
    }
}
```

Each call spawns a **new Z3 process**, reads its stdout, and exits. There is no incremental solver context. The only optimization lever is **avoiding redundant calls** — not reusing a solver instance.

### 1.3 What already exists (and what's not there)

| Mechanism | Location | Purpose | Reusable for caching? |
|-----------|----------|---------|----------------------|
| `expr_eq` (structural equality) | `proof_engine/mod.rs:422` | Heuristic satisfiability | Partial — no hashing |
| `free_idents` (returns `BTreeSet`) | `verify.rs:367` | Deterministic ident collection | Yes — sorted output |
| `BTreeSet` in `build_vc_query` | `smt.rs:185` | Sorted free identifiers → byte-identical query | **Yes — query string is deterministic** |
| `seen` guard in `substitute_all` | `verify.rs:400` | Cycle detection in symbolic substitution | No — unrelated |
| `check_satisfiable` | `proof_engine/mod.rs:260` | Fast heuristic contradiction check | Already a fast path |

**Conclusion:** No proof result caching exists anywhere. The `seen` variable is a cycle guard, not a cache.

### 1.4 The determinism guarantee

`build_vc_query` already produces byte-identical SMT-LIB output across processes because free identifiers are collected into a `BTreeSet` before emission. This means **the query string itself can serve as the cache key** without any additional normalization.

---

## 2. Optimization Opportunities

### 2.1 Body verification (`--verify-bodies`, site B)

The body verifier calls `smt_discharge` once per symbolic path the symbolic prover cannot decide. The call chain:

```
verify_bodies(items)
  → verify_one(pre, post, body)
    → verdict_for_path(post, state)      // symbolic evaluation first
      → smt_discharge(post, state)       // Z3 escalation (C4), ONE call per undecided path
        → build_vc_query(&facts, &goal) // deterministic string
        → prove_smt_formula(&query, 1000)
```

Cache hits apply when the same `(facts, goal)` pair appears across multiple items or loop obligation variants (init/preserve/exit often share constraints).

### 2.2 Protocol composition (site C)

`find_inverse_pairs` iterates over all protocol variant pairs, calling `prove_composition_inverse` for each candidate. This is O(n²) in the number of protocol variants with cast edges. Cache hits apply when the same forward/inverse body pair is checked multiple times or across recompilations.

### 2.3 Contract compile gate (site A)

`prove_contract` is called once per contract. Cache hits apply across multiple files in a batch compilation or across protocol-related contracts that share constraints.

### 2.4 Quantified opportunity

The highest-leverage site is **site B** (body verification) — each undecided path fires exactly one SMT call, and paths often share constraints. Site C has a structural O(n²) call count that makes caching particularly valuable. Site A is the compile gate and benefits less (1 call per contract).

---

## 3. Design Decisions

### 3.1 Cache key

**Use the query string itself as the key.** The `build_vc_query` function already produces deterministic output with sorted free identifiers. A BTreeMap `String → SmtResult` is sufficient.

Alternative considered: canonicalize the expressions into a hash before building the query. Rejected because it adds a second normalization step and the query string is already deterministic.

### 3.2 Cache scope and lifetime

**In-process memoization.** The cache lives as a `Mutex<FxHashMap<String, CacheEntry>>` in `briev-verify/src/smt.rs`. It persists for the duration of a single compilation but not across invocations. This is the correct scope because:

- Z3 is invoked as a subprocess; the cache avoids spawning the same query twice within one run.
- Cross-compilation persistence would require versioning (compiler version + Z3 version + theory config), which is out of scope for a first implementation.

### 3.3 Cache entry

```rust
struct CacheEntry {
    result: SmtResult,
    z3_version: String, // invalidate if Z3 binary changes
}
```

`z3_version` is determined once at startup by `z3_bin()` and stored in a thread-local or module-level `static`. If Z3 is not available, the cache is bypassed entirely (the current behavior is to return `Unknown` immediately, so caching would just cache `Unknown` — not useful).

### 3.4 Invalidation

Cache is invalidated when the Z3 binary changes (different `--version` output). This is checked lazily: the stored `z3_version` string is compared on each cache hit; a mismatch flushes the map and re-queries.

### 3.5 Thread safety

`std::sync::Mutex` around `FxHashMap`. All current call sites are single-threaded, so contention is not a concern.

### 3.6 Observability

`BRIEV_DIAG_VC` already prints the VC query to stderr. A companion flag `BRIEV_DIAG_PROVE` (or extending the existing flag) prints cache hits/misses with counts.

### 3.7 Memory ceiling

The body verifier is bounded by the number of items × the number of undecided paths. A `usize` counter on the map tracks entry count. A hard cap (e.g., 10,000 entries) prevents unbounded growth on pathological inputs; beyond that, the cache is disabled with a one-time warning.

---

## 4. Files to modify

### 4.1 `briev-verify/src/smt.rs`

Add the cache struct and wire it into `prove_smt_formula`.

**New code:**
```rust
use std::sync::Mutex;
use rustc_hash::FxHashMap;

// Cached Z3 version (lazy-initialized on first resolve)
static Z3_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn z3_version() -> Option<&'static str> {
    Z3_VERSION.get_or_init(|| {
        // Run z3 --version, capture first line, strip newlines
    })
}

struct CacheEntry {
    result: SmtResult,
    version: String,
}

static PROOF_CACHE: Mutex<FxHashMap<String, CacheEntry>> = Mutex::new(FxHashMap::default());

pub fn prove_smt_formula(formula: &str, _timeout_ms: u64) -> SmtResult {
    // Fast path: check cache before spawning Z3
    let version = z3_version();
    if let Some(v) = version {
        if let Ok(cache) = PROOF_CACHE.lock() {
            if let Some(entry) = cache.get(formula) {
                if entry.version == *v {
                    return entry.result.clone();
                }
            }
        }
    }

    // ... existing Z3 invocation logic ...

    // On success, cache the result
    if let Some(v) = version {
        if let Ok(mut cache) = PROOF_CACHE.lock() {
            cache.insert(formula.to_string(), CacheEntry { result: result.clone(), version: v.clone() });
        }
    }
    result
}
```

**Praetor considerations:**
- `Mutex<>` wraps a `FxHashMap` — single lock acquisition per call, no nested locks.
- `PROVE_CACHE` is a module-level `static` — Praetor complexity is 0 (no function calls at static init time).
- The cache insert/lookup is 2 branches — cognitive ≤ 2.

### 4.2 `briev-verify/src/lib.rs`

Export the cache statistics (hit/miss counts) as a `CacheStats` struct for diagnostic reporting.

---

## 5. Test strategy

### 5.1 Unit tests

1. **Cache hit:** call `prove_smt_formula` twice with the same formula; second call returns cached result without spawning Z3.
2. **Cache miss:** call with a new formula; Z3 is spawned.
3. **Version invalidation:** swap the `z3_version()` return between calls; cache is bypassed on second call.
4. **Thread safety:** `prove_smt_formula` is called concurrently from two threads; no panics or deadlocks.

### 5.2 Integration tests

1. `--verify-bodies` on a program with multiple obligations sharing constraints (e.g., `x >= 0`); verify SMT is called once for the shared constraint.
2. Protocol cast pair verification; verify only unique forward/inverse pairs are queried.

### 5.3 Benchmark

Compile a program with `--verify-bodies` that produces ≥50 undecided paths. Measure:
- SMT call count (via `BRIEV_DIAG_PROVE=1`)
- Wall-clock time

Expected: SMT call count drops from N to ≤N_unique, where N_unique ≤ N and is often substantially smaller for programs with shared guard constraints.

---

## 6. Risk analysis

| Risk | Severity | Mitigation |
|------|----------|------------|
| Cache key collision (different ASTs → same SMT query string) | Low | `build_vc_query` uses sorted BTreeSet; different ASTs produce different sorted free-var sets; unlikely |
| Caching `Unknown` results | Low | Cache stores `Unknown` when Z3 is unavailable; subsequent calls without Z3 return `Unknown` immediately — same behavior, just faster |
| Memory growth on large programs | Low | Hard cap (10,000 entries); map auto-disables beyond cap |
| Version mismatch after Z3 upgrade | Low | Lazy version check on each cache hit; mismatch flushes the cache |
| Incorrect results if Z3 is swapped mid-process | Low | Version string is captured once and compared on every hit |

---

## 7. Related work already done

- `check_satisfiable` in `src/proof_engine/mod.rs` is the existing **heuristic fast path** that avoids SMT for direct contradictions (`false && x`, `0 == 1`, `x > 5 && x <= 3`). This already prevents many SMT calls — the cache targets the remaining cases that reach Z3.
- `detect_tautology` rejects `[true][true]` and functionally-always-true contracts early — this is a policy gate, not an optimization, and caching does not apply.
- The `seen` guard in `substitute_all` / `symbolic_to_expr` is cycle detection for symbolic substitution, not a proof cache.

---

## 8. Implementation order

1. Add `z3_version()` helper to capture Z3 binary version string once.
2. Add `static PROOF_CACHE: Mutex<FxHashMap<String, CacheEntry>>` to `smt.rs`.
3. Pre-cache-check in `prove_smt_formula` (before spawning Z3).
4. Post-result cache insert (after Z3 returns a terminal result: `Sat`/`Unsat`/`Unknown`).
5. Lazy invalidation: compare stored version on hit.
6. Add `#[cfg(test)]` cache-hit/miss/unit tests.
7. Add `BRIEV_DIAG_PROVE=1` diagnostic output (hits/misses/counts).
8. Add hard memory cap with one-time warning when exceeded.

---

## 9. Open questions

1. **Should `Unknown` results be cached?** Currently the code returns `Unknown` when Z3 times out or is unavailable. Caching `Unknown` means repeated identical queries on an unavailable Z3 return `Unknown` immediately — same behavior, less latency. However, if Z3 becomes available mid-compilation (unlikely), stale `Unknown` results could be wrong. Recommendation: cache `Unknown` but log a warning on each cache hit when Z3 was previously unavailable.

2. **Per-call-site cache vs. global cache?** A per-site cache (separate maps for contract gates vs. body verification) adds complexity for marginal benefit. A single global map keyed by the full query string is simpler and correct — different call sites that produce the same query string should share the result.

3. **Persist the cache to disk?** Not in this plan. Cross-compilation caching requires versioning and would be a separate feature. The in-process cache covers the dominant use case (repeated SMT calls within one compilation).
