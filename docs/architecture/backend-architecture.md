# LLVM Backend Architecture Guide

**Audience:** AI coding agents and new contributors who need to make correct changes to the LLVM backend without violating the type system's architectural invariants.

**Status:** Living document — updated as the backend evolves.

---

## 1. Core Architecture

### 1.1 Three-Layer Lifetime Model

The backend state is strictly stratified into three lifetimes to prevent state leakage:

| Layer | Type | Scope | Mutability | Contents |
|-------|------|-------|------------|----------|
| **CompilerContext** | `context.rs` | Global (entire compilation) | Read-only during codegen | AST definitions, FFI signatures, target specs, field layouts, casting graph |
| **FunctionContext** | `context.rs` | Per-function/transaction | Mutable per-function | SSA registers (`gen_reg`), phi nodes, type caches, loop state |
| **LlvmBackend** | `mod.rs` | Orchestrator | Delegates to modules | `generate()`, dispatch, helper functions |

**Rules:**
- Never add transient, function-scoped compilation variables to `CompilerContext`.
- All registers must be requested via `self.fun.gen_reg()`. Manual register arithmetic is forbidden.
- `CompilerContext` is read-only during codegen. If you need mutable per-function state, add it to `FunctionContext`.

### 1.2 Module Map

| File | Purpose |
|------|---------|
| `mod.rs` | Entry point (`generate()`), field index assignment, dispatch, helper functions |
| `emit_toplevel.rs` | Top-level IR emission: `@init_state`, `@main`, struct type declarations, runtime declares |
| `emit_expr.rs` | Expression emission: arithmetic, casts, calls, loads, stores |
| `emit_stmt.rs` | Statement emission: assignments, let bindings, control flow |
| `helpers.rs` | Common helpers: `adapt_to_i64`, `load_field_type`, `store_field_type`, `llvm_type` |
| `context.rs` | `CompilerContext` and `FunctionContext` struct definitions |
| `normalizer.rs` | LLVM type resolution and struct layout computation |
| `intrinsics.rs` | Intrinsic call emission (`Sqrt#`, `Malloc#`, etc.) |
| `vector_phi.rs` | Vector phi group detection and emission |
| `loop_engine/` | Loop emission strategies (counter.rs, ssa.rs) |
| `tests.rs` | Backend unit tests |

### 1.2.5 Backend Folder Layout + Normalizer Responsibility

2026-08-10: each live backend lives in its own folder (`llvm/`, `circt/`,
`spirv/`, `webstack/`, `vm/`) as `mod.rs` (generator) + `normalizer.rs`.
The legacy flat backend files (`c.rs`, `rust.rs`, `verilog.rs`, …) were
deleted — they were unreferenced dead code.

User `TypeDef` registration is shared: `backend/register_types.rs`
`register_typedefs()` populates the `TypeUniverse` uniformly, and **every**
backend normalizer calls it first (LLVM, CIRCT, SPIR-V, webstack, and the
minimal VM pass — the VM is untyped but must not rot the uniform-universe
invariant). After registration each normalizer derives only what its backend
needs:
- LLVM: (nothing more — the casting graph resolves IR types at codegen time)
- CIRCT: `bit_width` from `bytes`, hardware-only intrinsic validation, keep-list
- SPIR-V: kernel flagging + op validation + keep-list
- webstack: `js_type` + `TypeTag` via `protocol_category` (Cast.# lane)
- VM: registration only

The normalizer's one job remains registering types; it does NOT resolve
native types or compute layouts — those are the casting graph's.

### 1.3 Code Generation Flow

```
generate(items)
  │
  ├─ build_field_index(items)        — Assign state slot indices from let declarations
  │     Every let var → (field_index, field_type, briev_type)
  │     Synthetic fields (cycle_count, arena_ptr) appended after
  │
  ├─ declare_struct_types(&mut out)  — Emit universe-registered struct types
  │     (sorted, deterministic). 2026-08-13 (layout-keywords plan): a packed
  │     whole-byte struct emits LLVM's packed aggregate `<{ i48, i48, i16 }>`;
  │     a sub-byte packed struct or a union emits a byte array `{ [N x i8] }`;
  │     a plain struct emits `{ i64, … }`. Zero-width `Bits<0>` padding fields
  │     are filtered from the aggregate. The legacy SmallString64/StaticString/
  │     UTF8View type declarations were retired with their types (2026-08-01 B4).
  │
  ├─ emit_declares(&mut out)         — Declare @llvm.* intrinsics + runtime functions
  │
  ├─ emit_main_or_bootup(&mut out)   — Emit @main and/or __briev_init_state
  │     │
  │     ├─ emit_init_state()         — Function that writes initial values to %State
  │     │     Called from @main's entry block before the loop
  │     │
  │     └─ [Loop emission]           — One of:
  │           • emit_folded_main()     — Inline SSA (small states, dense writes)
  │           • emit_version_dag_main() — Version-DAG (single runtime when guard)
  │           • emit_countable_main()  — Per-field phi nodes (default)
  │
  └─ Metadata + function epilogue
```

## 2. The Golden Rule: Never Match Type Names

This is the most important rule in the backend. Violations produce bugs that are hard to find and fix.

### 2.1 Why Name Matching Is Wrong

Types are **protocol + metadata** (`docs/architecture/bits-thesis.md`). A type has no canonical layout. Its LLVM representation is derived at codegen time by the casting graph from `(protocol, bytes, variant)`.

```rust
// WRONG — matches type name directly:
Type::Custom(name) if name == "String" => { /* emit SSO string */ }

// RIGHT — queries protocol membership:
let (cat, _) = graph.type_to_protocol(universe, ty);
if cat == "String" { /* emit SSO string */ }
```

**Why this matters:**
1. A user-defined `type MyString: String` would NOT match `name == "String"` but WOULD match `Cast.String`. Programs using custom String-like types silently produce wrong codegen.
2. Every name match is a place where type system evolution must be manually tracked.
3. The backend was refactored to protocol-based dispatch (Phases 0-3). Name matches are leftovers.

### 2.2 The Protocol Dispatch Chain

```
Normalizer (normalizer.rs)
  │  Injects Cast.<Category> properties during universe registration
  │  e.g., type String: String → { "Cast.String": true }
  ▼
Casting Graph (graph.rs)
  │  type_to_protocol(universe, ty) — queries Cast.<Category> properties
  │  resolve_llvm_type(universe, ty, int_bits) — resolves LLVM type string
  ▼
LLVM Backend
  │  self.protocol_of(ty) (see §2.3) — NEVER matches type names
  ▼
Code Generation
```

### 2.3 The One Valid Query Pattern

```rust
// Access the casting graph's type_to_protocol result:
fn protocol_of(&self, ty: &Type) -> &str {
    let Some(graph) = self.ctx.casting_graph.as_ref() else { return "Bit" };
    let Some(univ) = self.ctx.type_universe.as_ref() else { return "Bit" };
    graph.type_to_protocol(univ, ty).0
}
```

### 2.4 Exceptions (Permitted by Rule 18)

These `Type` variants are compiler constructs that can be matched directly:

| Type variant | Rule | Reason |
|-------------|------|--------|
| `Type::Ptr(_)` | 18a | Compiler construct — not stored in universe |
| `Type::Bits(N)` | 18a | Width construct — compiler-internal |
| `Type::Vector(_, _)` | 18a | SIMD construct — compiler-internal |

Everything else — `Type::Custom(s)`, `Type::Applied(_, _)` — must go through protocol queries.

### 2.5 TBAA Exception (Rule 18c)

The `tbaa_node` function in `mod.rs` matches LLVM IR type strings (`"i64"`, `"float"`, `"ptr"`), not Briev type names. This is permitted because it operates on LLVM's type system, not Briev's.

### 2.6 Audit Checklist

Before committing any change to the LLVM backend:
```bash
# Must return ZERO results:
grep -rn 'Type::Custom.*if.*==.*"' src/backend/llvm/ | grep -v 'tests\.rs\|tb aa\|Int\b\|Float\b\|Bool\b'
```

Any match that is not `Int`, `Float`, or `Bool` (the bootstrap exceptions) is a violation.

## 3. Casting Graph

### 3.1 Architecture

The casting graph (`src/casting/graph.rs`) has 64 base protocol lanes:

```
Bit — root protocol (compiler axiom)
  ├── Int      → i64       — integer ALU
  ├── UInt     → i64       — unsigned integer ALU  
  ├── Float    → float/double — float ALU
  ├── String   → {i64,i64}  — SSO or heap string
  ├── Bool     → i8         — boolean comparison
  ├── Char     → i32        — unicode scalar
  └── Data     → ptr        — opaque pointer
```

Each protocol has a hardcoded direct lane to every other protocol. Protocol variants (`String<UTF8>`, `Float<IEEE754>`) are expressed through variant edges.

### 3.2 Key Functions

| Function | Location | What it does |
|----------|----------|-------------|
| `type_to_protocol(universe, ty)` | `graph.rs:492` | Returns `(category, variant)` from `Cast.#<Category>` properties |
| `resolve_llvm_type(universe, ty, int_bits)` | `graph.rs:540` | Returns LLVM type string from `(protocol, bytes)` |
| `is_protocol_member(ty, "String")` | Via `type_to_protocol` | Checks protocol membership |
| `find_path(graph, src, dst)` | `graph.rs:??` | BFS for cast path between protocols |

### 3.3 How `type_to_protocol` Works

```rust
// Simplified from graph.rs:492
pub fn type_to_protocol(&self, universe: &TypeUniverse, ty: &Type) -> (String, String) {
    match ty {
        Type::Bits(_) | Type::Void => ("Bit".to_string(), String::new()),
        Type::HashWord(name) => return (name.clone(), String::new()),
        _ => {} // fall through to universe lookup
    }
    let key = ty.universe_key().and_then(|k| universe.get(k));
    let rt = key?;
    // Priority: Float → UInt → Int → String → Bool → Char → Data → Bit
    if rt.properties.contains_key("Cast.Float") { ("Float", "") }
    else if rt.properties.contains_key("Cast.UInt") { ("UInt", "") }
    else if rt.properties.contains_key("Cast.Int") { ("Int", "") }
    else if rt.properties.contains_key("Cast.String") { ("String", "") }
    else if rt.properties.contains_key("Cast.Bool") { ("Bool", "") }
    else if rt.properties.contains_key("Cast.Char") { ("Char", "") }
    else if rt.properties.contains_key("Cast.Data") { ("Data", "") }
    else { ("Bit", "") }
}
```

**Never matches type names.** Always queries `Cast.#<Category>` properties injected by the normalizer.

## 4. State Fields and the %State Struct

### 4.1 How State Fields Work

Every `let` declaration at the top level of a `.bv` file becomes a state field:

```briev
let bound: Int = GetEnvInt!("BOUND");    // state field at index 0
let count: Int = 0;                       // state field at index 1
let bx0: Float32 = 0.0f32;               // state field at index 2
```

The `build_field_index` function in `mod.rs` assigns indices based on declaration order (or SoA-reordered order). Key data structures:

| Structure | Type | Description |
|-----------|------|-------------|
| `field_index_map` | `HashMap<String, usize>` | Field name → state slot index |
| `field_types` | `Vec<String>` | LLVM type per slot (e.g., `"i64"`, `"float"`, `"double"`) |
| `field_briev_types` | `Vec<Type>` | Briev type per slot |
| `idx_to_field_name` | `HashMap<usize, String>` | Reverse: index → field name |

### 4.2 Field Type Storage

All state fields are stored as `i64` in `%State` (from `push_field_type`, `mod.rs:918`). The `adapt_to_i64` / `ensure_typed_value` functions handle conversion between `i64` and the field's natural type at load/store time.

Exception: Float fields (`Cast.Float` types) are sometimes stored as their native `float`/`double` type in `%State`. The `declare_state_type` function uses `protocol_llvm_type` which returns the native type.

### 4.3 Accessing State Fields

```rust
// Load from state (with !range metadata if available):
let (reg_name, briev_type) = self.emit_state_load_i64_by_idx(out, "  ", field_idx);

// Store to state:
self.emit_state_store_i64_by_idx(out, "  ", field_idx, &value_reg);
```

**Never use GEP directly.** The `emit_state_load/store_i64_by_idx` functions handle the correct GEP generation, load/store emission, and metadata attachment.

### 4.4 Struct Type Declarations

The `declare_struct_types` function emits required LLVM struct type declarations:

```llvm
%SmallString64 = type { i64, i64, i64, i64, i64, i64, i64, i64, i64 }
%StaticString = type { i64, i64 }
%String = type { i64, i64 }
%UTF8View = type { i64, i64 }
```

These must always be present. Without them, clang 18.1.3's LICM `sinkRegion` pass segfaults. Universe-registered struct types are also emitted here, but any type matching the hardcoded names above is skipped to prevent duplicate declarations.

## 5. Loop Emission Strategies

### 5.1 Dispatch Logic (`mod.rs`)

Since Phase 1b (2026-07-31), the loop dispatch switches deterministically on the
frontend-computed `LoopShape` (`analysis/loop_shape.rs`) — the backend no longer
re-derives decisions from body re-walks. See
`docs/plans/2026-07-31-frontend-driven-dispatch.md` §6.5:

```
Entry → build_field_index → dispatch on analysis.loop_shapes[name]:
  LoopShape flags:
  1. Pure + const bound + !has_swan_song      → emit_folded_loop_shape (pure O(1) fold)
  2. version-DAG shape                        → emit_version_dag_main (self-deciding)
  3. batch_shape present + counter matches     → emit_countable_countdown_main (A007)
  4. counter_only_writes && !has_swan_song    → emit_folded_main (InlineSsa)
  5. vector groups && carried > regs          → emit_countable_main (VectorPhiGroup label)
  6. _                                        → emit_countable_main (PerFieldPhi)
```

Phase 2 (2026-07-31) additionally moved the measurement decisions into the
frontend (`AnalysisResults`, plan §7) so the backend *reads* instead of *recomputes*:

| Decision | Frontend source | Backend consumer |
|----------|-----------------|------------------|
| `#11 → #0` memory-attr downgrade (dense float) | `analysis/density.rs` `ComputeDensity` (cross-field float ops, `> 4.0` ops/field) | `emit_toplevel.rs` `emit_transaction` |
| Modulo dispatch (rotated vs one-shot switch) | `analysis/modulo_partition.rs` `ModuloPartition` | `loop_engine/ssa.rs` `try_modulo_switch_dispatch` |
| Callable-txn auto-inline | `analysis/inline_cost.rs` `InlineDecision` (weighted cost ≤ 40) | `emit_toplevel.rs` `emit_callable_txn` |
| Reactor tick `#2`/`#12` attr | `transition_graph.has_unguarded_ffi` | `dispatch.rs` `emit_reactor` / `emit_reactor_tick` |

The **composite-node decomposition** (§5.3) runs in the FRONTEND analysis
(`analysis/node_decompose.rs`, `analysis/match_normalize.rs`,
`analysis/loop_carried.rs`) and emits via `emit_version_dag_main`. It handles
single runtime `when` guards; multi-guard or statically-fixed-guard bodies fall
to PerFieldPhi.

### 5.2 PerFieldPhi (Default)

`counter.rs :: emit_countable_main`

Each written state field gets its own phi node in the loop header:

```llvm
.cm_header:
  %cmc = phi i64 [ %init_count, %entry ], [ %next_count, %.cm_latch ]
  %ppf228 = phi float [ %init_bx0, %entry ], [ %pbf193, %.cm_latch ]
  %ppf229 = phi float [ %init_by0, %entry ], [ %pbf194, %.cm_latch ]
  ; ... one phi per written field ...
  ;; body accesses values through phi registers
  ;; latch computes backedge values
```

Key data structures:

| Structure | Type | Description |
|-----------|------|-------------|
| `phi_field_regs` | `HashMap<String, String>` | Field name → phi register name |
| `backedge_field_regs` | `HashMap<String, String>` | Field name → backedge register name |
| `pending_phi_backedge` | `HashMap<String, String>` | Computed backedge values (written by body) |
| `last_val_temps` | `HashMap<String, String>` | Last computed value for each let-binding |
| `last_val_types` | `HashMap<String, Type>` | Type for each last computed value |

### 5.3 Composite-Node Decomposition (Version-DAG)

A reactive transaction whose body contains `when` guards is a **latent multi-node reactor**: each side-effecting guard is a second node trapped inside the first node's body. Briev's reactor design (concurrent firing, the XOR write rule) treats these as separate nodes that should be decomposed.

The decomposition is a **frontend analysis** (`analysis/match_normalize.rs`,
`analysis/node_decompose.rs`, `analysis/loop_carried.rs`) that emits via
`counter.rs::emit_version_dag_main`. It replaces the removed batch-loop
heuristics (`loop_peeling.rs`).

Since `when` guards have **no else chain**, the body is a sequence of segments separated by guards. The compiler runs a **recursive version-DAG decomposition** (see `docs/plans/2026-07-30-flat-node-decomposition.md` §11):

1. **Three-segment split** — partition the body at each top-level `when` guard into `[pre]`, `[guard]`, `[post]`.
2. **Predicate analysis at the split point** — evaluate the guard condition with the state at its exact position in the body. This captures whether the guard observes the counter pre- or post-increment *naturally*; no position scanning, no counter-name matching.
3. **Two-version reconstruction** (neutral framing — neither version is structurally "hot" or "cold"):
   - **Guard-absent version** = `[pre] + [post]` (guard body removed) — no side effects.
   - **Guard-present version** = `[pre] + [guard] + [post]` (guard body included) — contains the side effect.
   Which version dominates at runtime is a **predicate-frequency property**, not structural. For `when count % M == 0` the guard-absent version dominates; for a predicate true most of the time, the guard-present version is the frequent path.
4. **Static predicate simplification** — before versioning, classify each guard predicate: **always-true** → inline the guard body (or keep it apart if more efficient for LLVM); **always-false** → drop (unless observable); **runtime-dependent** → two versions.
5. **Recursion** into nested `when` guards produces a **DAG of self-terminating while loops**.

**Codegen shape** (keeps `graph.nodes.len() == 1` — the folded single-loop path, avoiding the reactor's memcpy-per-tick):

```llvm
entry:  init → br absent_entry
absent_entry:
  %absent = icmp (count < N) && (count % M != 0)
  br %absent → absent_body, → present_entry
absent_body:  [pre]+[post]  → br absent_entry        # self-terminating guard-absent loop (pure compute)
present_entry:
  %present = icmp (count < N) && (count % M == 0)
  br %present → present_body, → end
present_body:  [pre]+[guard]+[post] → br absent_entry  # self-terminating guard-present block (side effect)
end: ...
```

The guard-absent loop is pure compute (no function calls) → LLVM if-converts and vectorizes it. The guard-present block runs once per interval. Count=0 handling falls out of the predicate — the guard-present version fires at the initial state when the predicate holds (pre-increment guards), or at the first interval boundary (post-increment guards). The write-conflict analysis (Phase 1, unconditional read-write checks) makes the guard-present→absent dependency sequential — a guard-present version reading state written by the guard-absent version fires only after the guard-absent version commits.

**Minimal-state / loop purity:** A variable is hot-loop state (a phi register) iff it is loop-carried (written in iteration N, read in iteration N+k) or read by a convergence contract / observable side effect at a different point than its write. Loop-invariant fields are hoisted; boundary-only fields are materialized to %State once at the boundary. The hot loop body must have zero %State load/store so LLVM can prove no cross-iteration dependencies and vectorize. See `docs/architecture/minimal-state-and-purity.md`.

**Match normalization:** Statement-level `match` is normalized to a `when` sequence first, so this pass handles only `when` guards. The fallback arm becomes `when !(c1 ∨ ... ∨ cn)` — the negation of all other arm predicates, **never** `when true` (which would be indistinguishable from an unconditional block to the predicate analysis).

### 5.4 Loop Metadata

Loop vectorization metadata is emitted on the latch branch:

```llvm
br label %.cm_header, !llvm.loop !100
!100 = !{!100, !101, !102}
!101 = !{!"llvm.loop.vectorize.enable", i1 true}
```

To force vectorization, add metadata here. Note that LLVM's loop vectorizer cannot if-convert branches containing opaque function calls (`call @Print#`, etc.). Only pure-compute loops (no function calls in the body) can be vectorized.

## 6. Pointer Handling

### 6.1 Internal Representation

`Ptr<T>` values are stored as `i64` in `%State` (via `ptrtoint` at emission time). When used in load/store/call instructions, they must be converted back to LLVM `ptr` via `inttoptr`:

```llvm
%t0_p = call ptr @malloc(i64 %size)    ; Malloc returns ptr
%t0 = ptrtoint ptr %t0_p to i64         ; Convert to i64 for state storage
; ... later, when using the pointer:
%t_ptr = inttoptr i64 %t0 to ptr         ; Convert back before load
%val = load i64, ptr %t_ptr               ; Load through pointer
```

### 6.2 `!invariant.load` on Ptr Fields

`Ptr<T>` state fields carry `!invariant.load` metadata on their loads (added in `helpers.rs:2635`). This tells LICM the pointer value never changes after initialization, enabling hoisting of the pointer load to the loop preheader.

### 6.3 Where `inttoptr` Is Added

- `emit_expr.rs: Deref` — before loading through a Ptr value
- `emit_stmt.rs: Deref store` — before storing through a Ptr value  
- `counter.rs: Deref store` — same pattern in loop body stores
- `emit_expr.rs: emit_user_call` — Ptr arguments before function calls
- `emit_binop_from_config` — preserves Ptr return type for Ptr+Int Add/Sub

## 7. Common Pitfalls Checklist

Before merging any backend change, verify:

- [ ] **No type name matching** — `git grep 'Type::Custom.*if.*=="' src/backend/llvm/` returns zero results (except for `Int`, `Float`, `Bool` bootstrap exceptions)
- [ ] **No hardcoded String/Data/Char/UInt** — use `is_protocol_member(ty, "String")` or `protocol_of(ty) == "String"`
- [ ] **`llvm_type()` used correctly** — never hardcode `"i64"` or `"float"` as type strings
- [ ] **Phi register use limited to the loop body** — phi registers from `.inner_124` are invalid in `.ox_124`; use state loads instead
- [ ] **Registers via `gen_reg()`** — no hand-written `%tN` register names
- [ ] **`push_field_type` overrides to `"i64"`** — all state fields are i64 unless float
- [ ] **~New field added to `FunctionContext`?** Must be initialized in the default constructor
- [ ] **HashMap iteration determinism** — every HashMap iteration that emits IR must be sorted by key first
- [ ] **`declare_struct_types` has the hardcoded four** — `SmallString64`, `StaticString`, `String`, `UTF8View` must always be emitted

## 8. Testing and Verification

```bash
# Unit tests
cargo test --lib

# Benchmark correctness
bash benchmarks/build_and_bench.sh --correctness

# Runtime performance
bash benchmarks/build_and_bench.sh --runtime

# Compare against baseline worktree
bash benchmarks/compare_baseline.sh <benchmark_name>

# Audit for name matching violations
grep -rn 'Type::Custom.*if.*==.*"' src/backend/llvm/ | grep -v 'tests\.rs\|Int\b\|Float\b\|Bool\b'
```

## 9. Fragile Strategies — What Breaks If You Change It

The backend has several interacting subsystems. A change in one area can silently break another. This section documents each fragile subsystem, what it depends on, and what breaks if those dependencies are violated.

### 9.1 Composite-Node Decomposition (emit_version_dag_main)

**What it does:** For a transaction body with ONE runtime `when` guard, emits
a version-DAG: a guard-absent loop (`[pre] + [post]`, no side effect) and a
guard-present block (`[pre] + [guard] + [post]`, with the side effect). The
guard predicate is evaluated BETWEEN `[pre]` and `[post]` — the split point —
which captures whether the guard observes the counter pre- or post-increment
naturally (no counter-name matching, no position scanning).

**Dependencies (frontend analysis, not backend heuristics):**
- `analysis/match_normalize.rs::normalize_match_to_when` — statement-level `match` → `when` sequence
- `analysis/node_decompose.rs::split_into_segments` — partition body at top-level `when` guards; classify predicates (AlwaysTrue/AlwaysFalse/Runtime)
- `analysis/loop_carried.rs::classify_fields` — minimal-state classification (hoist invariant, drop dead, phi carried)
- `counter.rs::emit_version_dag_main` — the emission

**Pre/post-increment guard position:** Whether the guard observes the counter
pre- or post-increment is determined by WHERE the guard sits in the body
relative to the counter update — NOT by the counter's name. The split at the
guard captures this: pre-increment guards fire at count=0, post-increment
guards fire at count=N. A naive peel that fires every guard at count=0 breaks
post-increment benchmarks (float_math, print_loop, queue_drain, cancel_math,
kalman_filter_runtime, float_math_nonzero).

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Evaluate the guard predicate at the loop header instead of the split point | Post-increment guards fire at the wrong count (pre-increment value). |
| Present block fires without a `count < bound` check at the header | Off-by-one — fires at count == bound, which C references exclude. |
| Present block reads the absent body's post-`[pre]` registers | **Instruction does not dominate all uses** — the present block is a sibling, not a successor, of the absent body. It must read the header phis. |
| Skip materializing written fields to %State in the end block | Post-loop swan song reads the INITIAL value (boundary-only fields like `escapes`). |
| Manually increment the counter in the latch/present | Double increment — `[pre]`/`[post]` already contain the source's `count = count + 1`. |

**The dominance tree for the version-DAG:**

```
entry → header
header [phis + exit check count < bound]
  │
  ├──→ absent_body [runs [pre] → predicate check]
  │      │
  │      ├──→ latch [runs [post] → backedge → header]
  │      └──→ present [runs [pre] [guard] [post] → backedge → header]
  │
  └──→ end [materialize ALL fields to %State → swan song prints → ret]
```

**Key invariant:** The present block and end block must reference the HEADER
phi registers (which dominate them), not the absent body's post-`[pre]`
registers (sibling block, does not dominate). The absent body may update
`phi_field_regs` to post-`[pre]` values for the predicate check, but the
present/end blocks must restore the header phis.

### 9.2 PerFieldPhi Register Tracking

**What it does:** Each written state field gets a phi node. The body writes to `pending_phi_backedge`, the latch reads it for the backedge identity copy.

**Dependencies:**
- `phi_field_regs` — maps field name → phi register name (set in header)
- `backedge_field_regs` — maps field name → backedge register name (set in header)
- `pending_phi_backedge` — maps field name → computed value (set by body)
- `last_val_temps` — maps let-binding name → register name (set by body)

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Add a field to `write_set` without a corresponding phi | **Undefined value** in the backedge — LLVM picks `undef` for the missing phi. |
| Remove a field from `write_set` that's still assigned | **Silent value loss** — the assignment writes to `pending_phi_backedge` but no phi picks it up. The next iteration reads the old value. |
| Clear `phi_field_regs` without clearing `pending_phi_backedge` | **Dominance violation** — stale entries make the body reference registers that no longer exist. |
| Mix up `phi_field_regs` and `backedge_field_regs` in the latch | **Wrong backedge value** — the phi selects the wrong predecessor register. |
| Emit `fadd float 0.0` instead of `add i64 0` for integer fields | **LLVM IR type error** — float operation on i64 type. |
| Forget to sort `sorted_fields` | **Non-deterministic IR** — HashMap iteration order changes every compilation, producing ~9% performance variation between runs. |

### 9.3 SoA Field Reorder (analysis/soa_reorder.rs)

**What it does:** Reorders state field declarations from AoS to SoA layout before `build_field_index` assigns indices.

**Dependencies:**
- Runs between `items` extraction and `build_field_index` in `generate()`
- Uses `parse_numeric_prefix` to detect indexed families (bx0, bx1, etc.)
- Proves data independence between family members

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Move SoA reorder after `build_field_index` | **Wrong indices** — field_index_map is already built with AoS layout. All GEP offsets shift. |
| Skip the independence proof | **Wrong results** — if `bx0` references `bx1` (through let-bindings), reordering changes computation order. |
| Change the field name prefix detection | **Fields not grouped** — renamed fields (e.g., `body0_x`) produce wrong prefixes. |
| Sort `non_float_indices` differently | **Non-deterministic output** — non-field items move around, changing the IR structure. |

### 9.4 Briev-Level LICM (analysis/licm.rs)

**What it runs:** Before the dispatch, identites loop-invariant let-bindings and prepends them to the body.

**Dependencies:**
- Runs after `hoist_terminating_guard` but before the dispatch
- Depends on `write_set` to determine which identifiers are variant

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Remove the `state_fields` parameter check | **Local variables hoisted as loop-invariant** — a let-binding `step = dt * 0.5` uses `dt` (not a state field) so it's not in `write_set`. Without checking `state_fields`, the function thinks `dt` is invariant (it IS, but because it's a const, not because it's a state field). Actually this is correct — `dt` IS invariant. The bug would be if a LOCAL variable that changes each iteration is NOT in `state_fields` — the function would incorrectly mark it as invariant. |
| Hoist expressions containing `Expr::Call` | **Side effects lost** — function calls inside invariant-looking expressions are hoisted, changing execution count. |
| Hoist after the dispatch instead of before | **Different dispatch decisions** — if a hoisted binding changes the body structure, the dispatch might select a different strategy. |

### 9.5 Hoist Terminating Guard (hoist_terminating_guard)

**What it does:** Removes `Term(..)` / `TermBang(..)` statements from the body and places the last guard's body in `post_hoist` if it contains `TermBang`.

**Dependencies:**
- Runs before the dispatch and before the composite-node decomposition
- Must be called before `node_decompose::split_into_segments` because it removes the termination guard from the body

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Swap order with `split_into_segments` | **Guard body emitted in inner loop** — the termination guard's TermBang body (containing `PrintLn!`) ends up in the version-DAG's absent body, blocking if-conversion. |
| Don't check for `TermBang` in the last guard | **Terminating print never fires** — the guard body isn't hoisted to `post_hoist`. |
| Don't remove `Term(..)` from the body | **`emit_countable_body` hits the catch-all** — `Statement::Term(None)` falls through to `_ => {}` and is silently dropped. |

### 9.6 Vector Phi Emission (Dormant — DO NOT RE-ENABLE WITHOUT RESEARCH)

**What it is (2026-07-31 update):** the frontend computes isomorphic vector
groups (`LoopShape.vector_groups` via `slp_isomorphism::analyze_body`); the
dispatch records a "VectorPhi" label when groups exist AND `carried_len >
float_register_count()`, but the emission is still PerFieldPhi (the vector-phi
infrastructure is not wired). The group DETECTION moved frontend-side in
Phase 1b; the EMISSION remains dormant for the reasons below.

**Why it's dormant:** Vector phi emission was disabled because:
1. `<2 x float>` groups added extract/insert overhead that dwarfed register-pressure benefit
2. `<8 x float>` groups triggered AVX lane-crossing latency
3. Naming-based grouping was a fragile heuristic
4. The SoA reorder pass + SLP vectorizer achieves the same result without explicit vector phis

**What breaks if re-enabled without fixing the root cause:**

| Risk | Why |
|------|-----|
| `extractelement_cache` used without clearing | Same extract value used across iterations — stale values. |
| `field_to_phi` conflicts with `phi_field_regs` | Two sets of phi nodes for the same field — which one does the body read? |
| Vector phis + version-DAG interaction | The version-DAG's end block materializes fields to %State. Vector phi values are `<N x float>` — storing them would require deconstructing the vector. |

### 9.7 `push_field_type` i64 Override

**What it does:** Forces all state field LLVM types to `"i64"` regardless of their Briev type.

**Dependencies:**
- `field_types` — used by `emit_state_load_i64_by_idx` / `emit_state_store_i64_by_idx` for GEP+load/store
- `field_briev_types` — used by `llvm_type()` / `protocol_llvm_type()` for protocol-based type resolution

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Remove the i64 override | **%State struct layout changes** — float fields would be `float` instead of `i64`. All GEP offsets stay the same (the struct is reified by `declare_state_type` which uses `protocol_llvm_type`), but the load/store type would mismatch. |
| Change the override to `float` for float fields | **`push_field_type` comment says "always i64"** — code paths that `load i64` from state would get wrong types. The `adapt_to_i64` path expects `i64` loads. |

### 9.8 `!invariant.load` on Ptr Fields

**What it does:** Adds `!invariant.load` metadata to all loads of `Type::Ptr(_)` state fields.

**Dependency:** Asserts that `Ptr<T>` fields are assigned exactly once (at init) and never reassigned.

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Apply `!invariant.load` to non-Ptr fields | **Stale values** — if a field IS modified, LLVM caches the initial load value forever. |
| Remove the `!invariant.load` from Ptr fields | **One extra GEP+load per iteration per Ptr field** — LICM doesn't hoist the pointer load, adding ~4 cycles per iteration. |

### 9.9 `declare_struct_types` Hardcoded Skip Set

**What it does:** Emits `%SmallString64`, `%StaticString`, `%String`, `%UTF8View` as hardcoded type declarations, then skips them during the universe iteration to avoid duplicates.

**Dependency:** The skip set must always include ALL names emitted in the hardcoded block.

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Add a name to the hardcoded block without adding it to the skip set | **Clang error: `redefinition of type '%NewType'`** — duplicate type declaration. |
| Remove a name from the skip set without removing it from the hardcoded block | Same — duplicate declaration. |
| Remove a name from the hardcoded block | **If the universe doesn't have it, clang LICM may segfault** — missing struct type triggers `sinkRegion` crash in clang 18.1.3. |

### 9.10 Constant Float Emission Pattern

**What it does:** Emits `float` literals as a single `bitcast i32 <hex> to float` instruction instead of the old `add i32 0, N` + `bitcast` + `fadd float 0.0` sequence.

**Dependency:** All `float` literals go through `emit_expr.rs::Expr::Float`. The `float_to_llvm_hex` function converts the f32 value to its i32 bit pattern.

**What breaks if changed carelessly:**

| Change | Failure mode |
|--------|--------------|
| Revert to the old pattern | **~2100 extra IR instructions per nbody compilation** — no runtime impact but more work for LLVM's optimizer. |
| Use `float <value>` directly instead of `bitcast` | **LLVM verifier error** — high-precision float literals like `"0.001660076642744037"` have more significant digits than f32 can represent. The bitcast from hex avoids this. |

## 10. Quick Reference: Protocol Categories vs LLVM Types

| Protocol | Default LLVM type | Width (bytes) |
|----------|-------------------|:-------------:|
| `Bit` | `i64` | 8 |
| `Int` | `i64` | 8 |
| `UInt` | `i64` | 8 |
| `Float` | `double` (default 64-bit) | 8 |
| `#Float32` | `float` | 4 |
| `String` | `{ i64, i64 }` | 16 |
| `Bool` | `i8` | 1 |
| `Char` | `i32` | 4 |
| `Data` | `ptr` | 8 |

These are resolved by `resolve_llvm_type()` in the casting graph. Never hardcode them.

## 10. Adding a New Protocol Type

1. Define the type in stdlib `.bv` with protocol membership:
   ```briev
   type MyType: String { spec Bytes: 16; op CastTo(Int): my_parse(#L); };
   ```
2. If a new protocol category is needed, add a lane in `graph.rs::new()`:
   ```rust
   self.set_lane("MyProto", "Bit", LaneKind::Bitcast);
   ```
3. Add normalizer injection for `Cast.#MyProto` in `normalizer.rs`.
4. Add protocol arm in `type_to_protocol` priority chain.
5. Add LLVM type resolution in `resolve_llvm_type`.
6. **No name-based matching in the backend.**
