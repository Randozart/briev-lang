# BUGS.md ledger sweep — open vs stale, verified (2026-10-06)

**Purpose:** the BUGS.md ledger (328 entries) had 150 headers with no
status marker, so "how close is the language to usable" could not be
answered honestly. This sweep classifies every unmarked entry against the
*current* codebase and records the evidence, so the distance estimate
rests on verified facts instead of 8 500 lines of history.

**Evidence standard (the contract of this document):** a verdict without
evidence stays OPEN. `STALE-FIXED` requires one of: a covering test, the
named symbol absent from `src/`, a fixing commit, or a benchmark results
row showing the defect gone. `OPEN` requires the described pattern to
still exist (`file:line`) or a repro to still fail. `OPEN-UNVERIFIED` =
plausibly open but needs hardware/toolchain; the verification method is
named so the next session does not re-investigate. Entries were never
rewritten — only headers gain a `[LEDGER …]` tag.

Method: three parallel read-only verification passes over the unmarked
entries (source greps, `git log -S`, results-file reads, and for the
high-value items a live `brievc` repro), plus a direct stdlib and
conformance check. Repros for this session live in the repo as
`tmp_vs_*.bv` scratch files (deleted on commit).

## Verdict summary

| Verdict | Count | Meaning |
|---|---:|---|
| `OPEN` | 6 | still true today — evidence cited |
| `OPEN-UNVERIFIED` | 5 | needs hardware/toolchain to confirm; method named |
| `STALE-FIXED` | 94 | fixed since filing; evidence cited |
| `RESOLVED` | 24 | entry already carried its own resolution marker |
| `SUPERSEDED` | 7 | a later entry/feature removal replaces it |
| `BY-DESIGN` | 4 | intentional; cited design doc or entry verdict |
| `NOT-A-BUG` | 3 | the entry itself concludes it is not a bug |
| `N/A-RECORD` | 9 | incident/measurement/doc record, not a live bug |
| **total** | **152** | unmarked entries classified |

**6 are genuinely open**, 5 more need
hardware/toolchain to confirm. Everything else was stale, self-resolved,
superseded, by-design, or not a bug at all — i.e. ~87% of the unmarked
ledger was noise.

## V1 — conformance and excluded roots (verified first)

| Check | Result |
|---|---|
| `cargo test --lib conformance_sweep_every_active_source_parses_and_checks` | **ok** (98.7 s) — 0 failures at tip |
| Roots swept | `lib/std`, `lib/compiler`, `lib/glue`, `examples`, `benchmarks`, `.smoke` (`src/conformance.rs:187`) |
| Excluded: `lib/glue/*/glue.dbv` | covered by `glue::config` tests (quoted mode), green in suite |
| Excluded: `lib/compiler/*.bv` (tamer WIP) | **10 of 12 FAIL `brievc check`** — only `reader.bv`, `token.bv` pass |
| `lib/std/*.bv` (all 30, `brievc check`) | **30/30 PASS** — including `string.bv`, `json.bv`, `process.bv`, `collections.bv` |

`lib/compiler` is the one real coverage hole: `main.bv` (10 parse errors),
`parser.bv` (45), `ast.bv` (47), `proof_engine.bv` (40), `typechecker.bv` (13),
`call_graph.bv` (9), `range.bv` (10) fail to parse; `lexer.bv`,
`needs_state.bv`, `soa_reorder.bv` parse but fail typecheck. This is the
known "compiler-in-Briev dogfood" gap — it stays excluded from the sweep
by design (`src/conformance.rs:227`), so do not treat a green sweep as
evidence that the self-hosting embryo typechecks.

## Critical open findings (stranger-blocking order)

### 1. `List<T> + List<T>` silently miscompiles — FIXED 2026-10-06

```
let a: List<Int> = [1, 2];
let b: List<Int> = [3];
let c: List<Int> = a + b;
println!(c.Count#());   // printed 1   (expected 3)
println!(c[0]);         // printed 188769584955600 — garbage
```

`brievc check` **and** `brievc build` both succeeded; the program ran and
printed wrong values. Root cause: `resolve_binary_op_binding` returned
`OpBinding::Intrinsic("list_concat")` (`src/typechecker/mod.rs:419`) — the
only reference to that name in the whole tree. `elaborate_ops` rewrites
only `OpBinding::Function` (`mod.rs:2474`), so the `BinaryOp` survived to
codegen and lowered as integer `add` on two list handles. Neither the LLVM
backend nor the interpreter implemented `list_concat` (grep: zero hits).
Worse: the binding matched on the type NAME `"List"` (`mod.rs:415-419`),
which is the Rule-15 shape the codebase forbids.

**This was the worst failure class in the ledger — silent wrong answer, no
diagnostic.** Fixed by binding `List + List` to the existing stdlib
`iter_chain` as an `OpBinding::Function` (extracted to
`TypecheckContext::list_concat_binding`), so `elaborate_ops` rewrites the
operator into a call and the `defined_fns` guard turns a missing stdlib into a
typecheck error; mismatched element types now hit the ordinary
`InvalidOperation` diagnostic. `std/iterator.bv` is imported by the native
prelude only (its `Count#` bodies are rejected by the SPIR-V normalizer).
Backend repro now prints `2 1 3 1 2 3`. Filed as its own BUGS.md entry
(`List<T> + List<T> silently miscompiles — FIXED`).

### 1b. Interpreter `<-` never pushed — FIXED 2026-10-06

Found while adding the Rule-5 parity test for finding 1: the reference
interpreter rebound the arrow target instead of pushing, so every list
accumulator in the interpreter kept only the last element while the backend
pushed (`iter_chain([1,2],[3])` → `Int(3)` vs `[1,2,3]`; `HashMap.keys()` and
json's `elems <- …` likewise). Fixed in `src/interpreter/eval.rs`
(`arrow_write_target`): a positional-`Product` target now grows by one element,
matching the typechecker's `op InsertAt` dispatch. Residual: the static
type-driven arrow cases (destructive extract, CopyFrom read, handle-valued
collections like `Stack`/`PiggyBank`) still need the frontend-driven dispatch
recorded on the AST — see the BUGS.md entry.

### 1c. json.bv array parsing hangs in the interpreter — FIXED 2026-10-06

`parse_value("[1]", 0)` and `parse_array_elems("[1]", 1, [])` hung (> 60 s);
`parse_value("1", 0)` errored on an end-of-input read. Three independent Rule-5
divergences in `src/interpreter/`, all fixed the same day:
(1) `eval_match` evaluated arm bodies against a CLONED bindings map, so a
block-bodied arm's writes were discarded — `parse_array_elems` never advanced
`pos` and its `txn` postcondition never held (the hang);
(2) `eval_binary_op` evaluated both operands eagerly, so `&&`/`||` did not
short-circuit and contract guards read past the end;
(3) `run_body_once` dropped the trailing expression's value, so defns without
`term` (json's `json_parse`/`json_length`) returned the initial `Int(0)`.
The reference now parses `[1,2,3]` to length 3, matching the backend. Tests in
`src/interpreter/mod.rs` (`json_interpreter_tests`); suite 2891 → 2895. Filed as
its own BUGS.md entry (now FIXED).

### 2. BUGS.md:5698 — a program of only plain `txn`s builds to nothing (OPEN)

Re-verified 2026-10-06: a file whose only item is a `txn` builds clean,
emits 3 functions (`init_state`, `main`, `_start`), never defines the txn,
runs, and exits 0 printing nothing. The only diagnostic is the generic
"runtime loop has no observable side effects" warning — nothing says *your
txn never fires*. Liveness (dead defns are not emitted) makes the missing
symbol correct; what is missing is the diagnosis the entry itself proposes.
Semantics decision still needed: diagnose unfired txns, or run-once-at-init.

### 3. Other confirmed open

| Entry | Finding |
|---|---|
| `BUGS.md:632` | PHI-mismatch IR repro still reproduces (`let` & inline variants); pre-existing |
| `BUGS.md:5954` | nbody_newton 7th-decimal drift re-measured today: -0.169207186 vs C -0.169208258 |
| `BUGS.md:1704` | i64 boxing tax: `adapt_to_i64` live (helpers.rs:2223), Phase 1 never executed |
| `BUGS.md:6740` | `briev_dev_cuda.c:614` `push_strided` returns 0 under CUDA 13.4 legacy dlopen |
| `BUGS.md:7439` | `hardware_validator` has zero call sites — the `.sbv` synthesizability gate never runs |
| `BUGS.md:5148` | `dyn Trait`: interpreter dispatch done, **LLVM backend still panics** (`emit_toplevel.rs:754-760`) — HALF-CLOSED |
| `BUGS.md:8404` | `list_concat` — subsumed by finding 1; FIXED 2026-10-06 |

### 4. Open-unverified (name the instrument, do not re-investigate)

| Entry | Verification method |
|---|---|
| `BUGS.md:5678` | install CIRCT, run `--export-verilog` on FIRRTL_Memory IR |
| `BUGS.md:5827` | 2–7-workgroup dispatch on an RTX 3060 |
| `BUGS.md:6166` | `spirv_coopmat_subgroups=1` coopmat GEMM at 2048³ |
| `BUGS.md:6707` | `m4_decode_microbench.sh` attention_decode on GPU |
| `BUGS.md:7460` | gemm_h / mma_ceiling / gemv triple on driver 615 vs 580 |

## Classification table

| BUGS.md line | Header | Verdict | Evidence |
|---:|---|---|---|
| 632 | Pre-existing oddity (not a regression):** a node whose inserts are LET-BOUND | `OPEN` | repro gives PHI-mismatch IR (let & inline variants); pre-existing, unfixed |
| 1704 | 2026-06-16 — LLVM Backend Audit — i64 Boxing Tax (Phase 0/1 Plan) | `OPEN` | i64 boxing deletion never done; `adapt_to_i64` live (helpers.rs:2223) |
| 5698 | Plain `txn` at top level compiles to an EMPTY program via brievc build — 2026-08-26 OPEN | `OPEN` | repro 2026-10-06: txn-only program → 3 defines, no `@run`, silent exit 0; only a generic observability warning |
| 5954 | 2026-09-07 — nbody_newton output drift (7th decimal) vs C reference [PRE-EXISTING, opened during noalias slice] | `OPEN` | re-ran 2026-10-06: -0.169207186 vs C -0.169208258 — 7th-decimal drift persists |
| 6740 | 2026-09-18: CUDA 13.4 cuMemcpy2D silently no-ops under legacy-context dlopen [OPEN] | `OPEN` | briev_dev_cuda.c:614 `push_strided` still returns 0; quirk unresolved |
| 7439 | 2026-09-28 — `hardware_validator` is dead code (the .sbv synthesizability gate never runs) | `OPEN` | src/lib.rs:57 sole reference; `hardware_validator::` has zero call sites |
| 5678 | CIRCT ExportVerilog rejects hw.module.generated (FIRRTL_Memory) — 2026-08-25 OPEN (toolchain) | `OPEN-UNVERIFIED` | circt-opt/firtool absent; verify: install CIRCT, export the FIRRTL_Memory IR |
| 5827 | 2026-09-02 — driver: small-dispatch store loss (2-7 workgroups), RTX 3060 | `OPEN-UNVERIFIED` | driver-level; verify: 2-7-workgroup dispatch on RTX 3060 (≥8-wg gate shipped) |
| 6166 | 2026-09-11: coopmat S=1 (subgroups=1) kernel produces zero y at 2048³ | `OPEN-UNVERIFIED` | verify: `spirv_coopmat_subgroups=1` coopmat GEMM 2048³ on device |
| 6707 | 2026-09-17: Multi-node resident programs — prime full-upload clobbers device arrays (OPEN) | `OPEN-UNVERIFIED` | m3 harness reportedly PASS (5a results:219); verify `m4_decode_microbench.sh` on GPU |
| 7460 | 2026-09-30 — NVIDIA driver 615.71.09 regressed the workgroup-smem + barrier compute path ~2.5× (RTX 3060, Vulkan) [OPEN — vendor] | `OPEN-UNVERIFIED` | vendor; verify gemm_h/mma_ceiling/gemv triple on driver 615 vs 580 |
| 1591 | 2026-06-17 — `is_string_chain` missing `Expr::Call` arm (SIGSEGV crash) | `STALE-FIXED` | `is_string_chain` deleted; concat type-driven (`emit_inline_concat`, helpers.rs:869) |
| 1628 | 2026-06-17 — `\0` char escape not handled in lexer | `STALE-FIXED` | `\0` handled at src/lexer.rs:570 |
| 1656 | 2026-06-17 — `done_{name}` SSA dispatch skips to exit instead of next txn | `STALE-FIXED` | false-pre branches to next-txn label (loop_engine/ssa.rs:416) |
| 1688 | 2026-05-28 — Overriding `from "..."` location in typechecker | `STALE-FIXED` | `<profile:` placeholder pattern absent from src/ |
| 1750 | 2026-05-28 — Adding built-in string matches for stdlib functions | `STALE-FIXED` | string-match builtins absent (reverted, as the entry states) |
| 1760 | 2026-05-28 — Typechecker overwrites `from` location | `STALE-FIXED` | duplicate of 1688; no `<profile:...>` overwrite remains |
| 1770 | 2026-05-28 — Contract-after-arrow parser bug | `STALE-FIXED` | tests/test_contract.rs:13 asserts arrow-contract parses as BinaryOp |
| 1780 | 2026-05-28 — Keyword tokens can't appear in any variable position | `STALE-FIXED` | `keyword_as_identifier` covers keywords (parser/helpers.rs:783) |
| 1790 | 2026-05-28 — \u{D800} surrogate fails char::from_u32 | `STALE-FIXED` | surrogate fallback `unwrap_or('?')` (src/lexer.rs:582) |
| 1800 | 2026-05-28 — Unevaluated enum constructors (None, Some, Ok, Err) | `STALE-FIXED` | variants registered from declarations (interpreter/mod.rs:258-273) |
| 1813 | 2026-05-29 — Method-call `x.foo(y)` drops all arguments except receiver | `STALE-FIXED` | `MethodCall(recv, name, args…)` keeps args (ast/expr.rs:72) |
| 1832 | 2026-05-29 — Term statement inside nested blocks doesn't propagate return value | `STALE-FIXED` | `Term` propagates `TermReturn` through nested stmts (eval.rs:2239) |
| 1859 | 2026-05-29 — Result field key mismatch between constructor and consumer | `STALE-FIXED` | `run_selfhost` deleted with the selfhost pipeline |
| 1886 | 2026-05-30 — `expand_implicit_terms_txn` injects `term true;` into void-returning transactions | `STALE-FIXED` | `expand_implicit_terms_txn` no longer exists |
| 1896 | 2026-05-30 — `opt` new PM syntax: `-passes=verify` not `-verify` | `STALE-FIXED` | `opt -passes=verify` in tests/llvm_compile_test.sh:149 |
| 1906 | 2026-05-30 — `alwaysinline` must precede attribute group in LLVM 18 | `STALE-FIXED` | `#0` then `alwaysinline` (emit_toplevel.rs:3395,3548) |
| 1926 | 2026-05-30 — Contract-after-arrow `-> Type [pre][post]` steals first bracket as `Type::ContractBound` | `STALE-FIXED` | `ContractBound` type gone; tests/test_contract.rs:11-26 passes |
| 1936 | 2026-05-30 — `len()` infinite recursion in self-host interpreter | `STALE-FIXED` | entry's own 2026-06-05 update: resolved (`len` = `CharCount#`) |
| 1948 | 2026-05-30 — Float result registers not tracked, causing compound float math to emit integer ops | `STALE-FIXED` | TypedRegister / reg_float_cache; float_math 0.98x (results 2026-09-30) |
| 1958 | 2026-05-30 — OnExit cleanup drained on first exit point, lost on subsequent exits | `STALE-FIXED` | cleanup flushed at every exit (emit_toplevel.rs:140; emit_stmt.rs:1434) |
| 1970 | 2026-06-01 — `extract_bounded_pre` drops `And` preconditions, fold limit stuck at 0 | `STALE-FIXED` | recursive `And` unwrap (transition_graph.rs:287-303) + test :1488 |
| 1984 | 2026-06-01 — Solo reactive txn auto-promoted to async, injects unnecessary thread pool + barrier | `STALE-FIXED` | `async_candidates.len() >= 2` gate (strategy.rs:263) |
| 1998 | 2026-06-01 — Wake hybrid programs idle forever after convergence (no exit mechanism) | `STALE-FIXED` | natural-death synthetic exit (mod.rs:4236-4268), tests.rs:3574 |
| 2014 | 2026-06-01 — `is_trigger_gated` only matches bare `Identifier`, misses `And(trigger, condition)` | `STALE-FIXED` | `And` arm in `is_trigger_gated` (strategy.rs:338) |
| 2028 | 2026-06-01 — `emit_enum_main` single-txn `graph.nodes.len() == 1 && txns.len() == 1` guard prevents multi-txn folded loops | `STALE-FIXED` | per-txn `enum_fold_params` (mod.rs:4558) replaces the guard |
| 2045 | 2026-06-02 — Struct-SSA regression for non-pure bodies (Kalman filter 2× slowdown) | `STALE-FIXED` | `opt -passes=default<O3>` before llc (compile.rs:2758-2760) |
| 2061 | 2026-06-02 — `is_trigger_gated` misses `Expr::Eq`, enum dispatch invisible for `trigger == literal` preconditions | `STALE-FIXED` | `Eq` arm in `is_trigger_gated` (strategy.rs:334-337) |
| 2095 | 2026-06-02 — `llvm.assume` before `br` in folded loops makes `opt` believe exit branch is dead | `STALE-FIXED` | `llvm.assume` emitted after br+unreachable (emit_toplevel.rs:4816-4820) |
| 2147 | 2026-06-04 — Decreasing counter contracts hang or fall to O(N) | `STALE-FIXED` | ConvergeDirection + Gt/Ge extraction (transition_graph.rs:227-240) |
| 2163 | 2026-06-05 — Unused `io_pending` import forces reactive runtime on pure-state benchmarks | `STALE-FIXED` | benchmarks/bit_clear.bv has no `io_pending` import |
| 2175 | 2026-06-05 — Low print modulo doesn't fire on short benchmarks | `STALE-FIXED` | bit_clear.bv:25 uses `when next_reg % 100000 == 0` |
| 2185 | 2026-06-05 — `memory(argmem: write)` on FFI declarations lets LLVM eliminate IO calls | `STALE-FIXED` | `memory(argmem: write)` gone; FFI `#6 {nounwind}` (mod.rs:4908) |
| 2213 | 2026-06-05 — Parser fails on `term! -> swan_song;` inside guarded blocks | `STALE-FIXED` | fixture tests/fixtures/term_guard_value_form.bv passes `brievc check` |
| 2225 | 2026-06-05 — LLVM `attributes #1` declared FFI functions as pure, letting optimizer eliminate I/O | `STALE-FIXED` | foreign declares `#6 = { nounwind }` (mod.rs:3566,4896) |
| 2237 | 2026-06-05 — `__putchar` undefined at link time despite definition in runtime | `STALE-FIXED` | `__putchar` symbol absent from lib/ and src/ |
| 2249 | 2026-06-05 — `io_pending` used as liveness workaround in benchmarks | `STALE-FIXED` | no `io_pending` guards in benchmarks/*.bv |
| 2261 | 2026-06-05 — Accidental deletion of benchmark source files during cleanup | `STALE-FIXED` | benchmarks/fannkuch_redux.bv restored and present |
| 2271 | 2026-06-06 — Parser discards `from "..."` value in frgn declarations | `STALE-FIXED` | `location` replaced by `FromSpec`; parser stores it (definitions.rs:695) |
| 2283 | 2026-06-06 — Hardcoded runtime declares in LLVM backend | `STALE-FIXED` | `emit_declares` no longer hardcodes the runtime (mod.rs:4653) |
| 2295 | 2026-06-06 — `"None"`/`"Err"` discriminant magic in LLVM backend | `STALE-FIXED` | declaration-order discriminants via `variant_disc` (mod.rs:3427; tests.rs:2993) |
| 2307 | 2026-06-06 — Interpreter built-in method dispatch is still name-based magic (deferred) | `STALE-FIXED` | no push/insert name matches; `#`-intrinsic + fn-table dispatch (eval.rs:1017) |
| 2327 | 2026-06-07 — `term! -> swan_song` emits `ret void` inside `i32 @main` in folded loop path | `STALE-FIXED` | `ret void` only in void fns; main ends `ret i32 0` |
| 2344 | 2026-06-07 — Guarded block handler restores `self.terminated` after `term!`, emits code after `ret` | `STALE-FIXED` | guarded handler convergence branch only if `!terminated` (emit_stmt.rs:1630-1641) |
| 2362 | 2026-06-07 — `-lm` missing in compiler driver link step (FIXED) | `STALE-FIXED` | `-lm` passed (compile.rs:2639) |
| 2374 | 2026-06-07 — `Statement::Guarded` is one-shot, not a loop — ~130 defns silently broken | `STALE-FIXED` | stdlib converted: `txn iter_filter_loop` (lib/std/iterator.bv:31) |
| 2431 | 2026-06-09 — Proof engine guard path three-bug cascade | `STALE-FIXED` | proof engine rewritten to SMT (proof_engine/mod.rs:434) |
| 2480 | 2026-06-09 — fasta LCG broken in node (all output chars same) | `STALE-FIXED` | fasta.bv single-assignment LCG (line 17) |
| 2494 | 2026-06-09 — LLVM backend emits `constant float 0` (needs `0.0`) | `STALE-FIXED` | `constant float 0` avoided (mod.rs:255,3433) |
| 2508 | 2026-06-09 — LLVM backend emits undefined `@str.0` reference | `STALE-FIXED` | `collect_strings` + definitions emitted (mod.rs:2846,3976) |
| 2522 | 2026-06-09 — `precompute_sum.bv` emits infinite tick loop (no observable output) | `STALE-FIXED` | precompute_sum.bv has observable `endprogram println!` (line 30) |
| 2534 | 2026-06-10 — LLVM backend: negative float constants in init_state stored as i64 (8 bytes) instead of float (4 bytes) | `STALE-FIXED` | negative float init via `emit_float_literal_store(-f)` (emit_toplevel.rs:2425-2430) |
| 2570 | 2026-06-10 — LLVM backend: non-SSA state field loads return Type::Int for float fields | `STALE-FIXED` | float state loads return `Type::float()` (emit_expr.rs:667-685) |
| 2614 | 2026-06-10: Benchmark Investigation After R2+R3 | `STALE-FIXED` | gaps closed: fannkuch 0.96x, kalman 0.93x, nbody_sqrt 0.71x (results 2026-09-30) |
| 2757 | 2026-06-11 — fannkuch_redux: silent correctness failure + 3.85x performance gap | `STALE-FIXED` | fannkuch_redux.bv:32,40 saved%13 + println at count==N; matches C |
| 2809 | 2026-06-11 — Silent postcondition failure in callable txns | `STALE-FIXED` | `call_txn` gone; `ContractViolation` propagates (errors.rs:1136, eval.rs:2059) |
| 2832 | 2026-06-11 — Convergence proof gated to reactive txns only | `STALE-FIXED` | `check_convergence` ungated (proof_engine/mod.rs:475) |
| 2914 | 2026-06-13 — Bare label `%` prefix in LLVM IR (emit_expr.rs) | `STALE-FIXED` | bare label defs (emit_expr.rs:917-929); no `%marm` pattern in src/ |
| 2926 | 2026-06-13 — `terminated` flag leak in `Guarded` block (emit_stmt.rs) | `STALE-FIXED` | terminated reset unconditional (emit_stmt.rs:1637-1643) |
| 2936 | 2026-06-13 — Dead `br` after `unreachable` in match emission (emit_expr.rs) | `STALE-FIXED` | `br` emitted only if `!terminated` (emit_stmt.rs:1637) |
| 2946 | 2026-06-13 — `%state` SSA scoping bug in LLVM backend | `STALE-FIXED` | state passed as `state_ptr_param` (emit_toplevel.rs:2799-2801) |
| 2972 | 2026-06-13 — Duplicate import items: functions emitted once per import path | `STALE-FIXED` | `dedup_items_with_origins` (import_resolver.rs:526,1765) |
| 3008 | 2026-06-13 — Unterminated basic block when Guarded then-path terminates (emit_toplevel.rs) | `STALE-FIXED` | per-path termination restored; conditional ret sites (emit_toplevel.rs:2969) |
| 3042 | 2026-06-13 — Unterminated `post:` label in `emit_callable_txn` | `STALE-FIXED` | guarded `br label %post` before the label (emit_toplevel.rs:4761-4767) |
| 3073 | 2026-06-17 — SSA extractvalue path missing return → duplicate register definitions | `STALE-FIXED` | knucleotide/mandelbrot MATCH (results/2026-08-01-plugin-rework-final.md:36,38) |
| 3117 | 2026-06-13 — SSA dominance violations: values from guard then-path used in merge path | `STALE-FIXED` | entry self-verifies: zero SSA dominance violations, 777 tests pass |
| 3152 | 2026-06-14 — Stdlib files fail to parse with Rust parser (pre-existing) | `STALE-FIXED` | stdlib parses/typechecks; benchmarks import std/io.bv; suite 2886 green |
| 3175 | 2026-06-14 — Parseable core files fail TypeChecker | `STALE-FIXED` | collections.bv imported+MATCH; ArrowAssign (typechecker/mod.rs:3457), Cast (:1210) |
| 3204 | 2026-06-14 — `__print` doesn't flush stdout | `STALE-FIXED` | C `__print` removed; stdout buffered in .bv (`__stdout_flush`, cast_lanes.bv:614) |
| 3216 | 2026-06-14 — `done_{name} → br label %done` exits main() after one reactive cycle | `STALE-FIXED` | `done_{name}` gone; re-enters `.ss_main_loop` (loop_engine/ssa.rs:481-514) |
| 3228 | 2026-06-14 — `@ link` for String loads pointer address, not content | `STALE-FIXED` | linked-String compare by first byte (helpers.rs:1796-1803) |
| 3279 | 2026-06-22 — Examples used `frgn __print_int` instead of `print_int#` intrinsic | `STALE-FIXED` | no `frgn __print_int` in examples/ or learn-briev/ |
| 3293 | 2026-06-26 — nbody_newton energy output always 0.0 in SSA loop mode | `STALE-FIXED` | fixed by 3427; nbody_newton MATCH (results 2026-09-30) |
| 3345 | 2026-06-26 — queue_drain crashes at BOUND≥2 with realloc(): invalid pointer | `STALE-FIXED` | fixed via 3546; queue_drain MATCH (results/2026-07-31…, 2026-09-30) |
| 3384 | 2026-06-26 — `setvbuf(stdout, NULL, _IOLBF, 0)` in briev_rt.c makes fputc 2.1× slower | `STALE-FIXED` | no `setvbuf` in lib/runtime/briev_rt.c |
| 3427 | 2026-06-27 — `try_eval_cfloat` missing `Expr::BinaryOp` normalization (nbody 0.0 energy bug) | `STALE-FIXED` | `try_eval_cfloat` matches `Expr::BinaryOp` (mod.rs:271-275) |
| 3452 | 2026-07-01 — `%dab2` prefix collides with `%dab` at counter offset 200 | `STALE-FIXED` | no `%dab`/`%dab2` register prefixes remain |
| 3481 | 2026-07-01 — `emit_binop` Phase 7B double-emission O(2^depth) blowup | `STALE-FIXED` | Phase 7B dispatch removed (helpers.rs:1219-1220) |
| 3520 | 2026-07-01 — `expr_dedup_cache` leaks register names across function boundaries | `STALE-FIXED` | `expr_dedup_cache.clear()` (emit_toplevel.rs:2761,4524,5945) |
| 3546 | 2026-07-01 — `let_original_types` not populated for custom types | `STALE-FIXED` | `let_original_types` populated for all lets (emit_stmt.rs:555,613) |
| 3569 | 2026-07-06 — Vector group backedge uses stale insertelement (nbody_sqrt MISMATCH) | `STALE-FIXED` | nbody_sqrt MATCH (results/2026-08-01…:32 through 2026-09-30) |
| 3587 | 2026-07-08 — `emit_operator_call` double-wraps register + missing string impl handler | `STALE-FIXED` | `emit_operator_call` uses `next_reg()` + Quoted opcode arm (helpers.rs:1644,1666) |
| 3692 | 2026-07-18 — Missing binary bitwise operators in parser | `STALE-FIXED` | parse_bitor/bitxor/bitand/shift levels (parser/expressions.rs:121-165) |
| 3712 | 2026-07-18 — Missing builtin operator bindings for Int bitwise ops | `STALE-FIXED` | (`Int`,`BitAnd`)→`BitAndI64#` (type_universe/operators.rs:79-80) |
| 3727 | 2026-07-18 — Dead `br` after `ret` in Guard/If codegen | `STALE-FIXED` | guard/If `br` skipped when terminated (emit_stmt.rs:1637) |
| 3764 | 2026-07-18 — `txn` return type not parsed | `STALE-FIXED` | `parse_transaction` gets output type (parser/definitions.rs:1145+) |
| 3780 | 2026-07-18 — `__` prefix used for non-frgn functions | `STALE-FIXED` | the `__`-prefixed defs referenced are absent from the repo |
| 4018 | All Structs Disappear from LLVM IR After Casting Graph Refactoring — UNFIXED | `STALE-FIXED` | universe struct emission restored (emit_toplevel.rs:550-599); workaround retired (:546-548) |
| 4045 | clang 18.1.3 LICM `sinkRegion` Segfault on Correctly-Aligned IR — UNFIXED | `STALE-FIXED` | toolchain now clang 23.1.1; harness `-O3 -flto`; benchmarks MATCH (results 2026-09-30/10-05) |
| 4076 | `String` Type LLVM Representation Changed from `{ i64, i64 }` to `i128` — UNFIXED | `STALE-FIXED` | `protocol_llvm_type` returns `ptr` for `#String` (mod.rs:876-883) |
| 4100 | ring_buffer: Baseline Compiler Produces No Output at 0.001s — UNFIXED (pre-existing) | `STALE-FIXED` | ring_buffer MATCH 1.18x (results/2026-08-01…:26; MATCH 2026-09-30) |
| 4121 | mandelbrot: Briev Output Differs from C — UNFIXED (pre-existing) | `STALE-FIXED` | mandelbrot MATCH 1.02x (results/2026-08-01…:36) |
| 4137 | nbody_sqrt_idio: Cannot Compile — UNFIXED (pre-existing) | `STALE-FIXED` | nbody_sqrt_idio compiles+MATCH (results/2026-08-01…:33) |
| 4146 | kalman_filter_runtime: Cannot Compile — UNFIXED (pre-existing) | `STALE-FIXED` | kalman_filter_runtime MATCH (results/2026-08-01…:37) |
| 4231 | 2026-08-01 — queue_drain dispatches to version-DAG, not the countdown | `STALE-FIXED` | entry self-notes "Fixed 2026-08-01 (A9b)" (line 4246) |
| 1161 | Accel Node Folds the Reactor to Nothing — RESOLVED BY DESIGN (Design A) | `RESOLVED` | entry "RESOLVED BY DESIGN (Design A)"; 14 accel tests green |
| 1230 | Protocol Round-Trip Proofs Silently Skipped — PARTIAL (interpreter side FIXED 2026-08-26) | `RESOLVED` | remaining half landed: protocol_graph.rs:174-199 hard error + axiom skip; test :324 |
| 1287 | Vestigial `return` Statement Removed (was the "return divergence") — RESOLVED | `RESOLVED` | entry "RESOLVED" — `return` removed from the language |
| 4788 | RESOLVED — "frgn String-return heap corruption" was a test-harness arity bug | `RESOLVED` | entry "Resolved (false alarm)" — harness arity; tests/c_driver_needs_state.rs |
| 4852 | wasm32 webstack: %State i64 storage vs i{int_bits} arithmetic — RESOLVED | `RESOLVED` | entry "Resolved 2026-08-10" — width-aware loop engines |
| 4955 | `!> IsZero:` / `!> IsOne:` stdlib metadata was dead — REMOVED 2026-08-13 | `RESOLVED` | entry "Removed — audited, no consumers" |
| 5012 | Bare collection literal as a function ARG crashes — KNOWN (string.bv unblock) | `RESOLVED` | repro `iter_sum([1,2,3])` prints 6, exit 0 (verified 2026-10-06) |
| 6099 | 2026-09-11: SPIR-V coopmat fill silently corrupted for 3 days (u32_and id-as-mask) | `RESOLVED` | entry fix + on-device A/B (4.436e-03 post-fix) |
| 6132 | 2026-09-11: fasta ~100× regression — unbuffered stdout after libc removal | `RESOLVED` | fix landed 158ae9ec+01c92189; fasta 0.73x, output identical |
| 6277 | 2026-09-14 — Equilibrium `wfi` park sleeps through address-wired eligibility | `RESOLVED` | entry RESOLVED marker; ssa.rs:507-510 address_wired spins, no wfi |
| 6308 | 2026-09-14 — Statement match with void txn arms emits broken expression-match IR | `RESOLVED` | repro 2026-10-06 builds + clang-valid + prints 1; fixed by c98b7189 |
| 6328 | 2026-09-14 — kernel_rv64 freezes entering the first U-mode task | `RESOLVED` | superseded by line 6362 "RESOLVED: kernel freeze chain" (four bugs fixed) |
| 6416 | 2026-09-13: SHIP PTX kernel — K≤32 with small M returns all-zero y | `RESOLVED` | entry "RESOLVED 2026-09-13" — driver args + cp.async commit fix |
| 6449 | 2026-09-14 — defn with contract brackets compiles as a convergence loop | `RESOLVED` | repro 2026-10-06: `@f` = linear add+ret, no `loop:`; runs and prints 1 |
| 6478 | 2026-09-14 — Asm# output lacks earlyclobber: input aliased into output | `RESOLVED` | fix `=&r`; test src/backend/llvm/tests.rs:9469 |
| 6496 | 2026-09-14 — VolatileStore# narrowing emitted invalid `zext i64 to i32` | `RESOLVED` | fix emitted-width compare; test src/backend/llvm/tests.rs:9443 |
| 6514 | 2026-09-14 — ARM bare-metal: .data never copied, begin_boot read 0 | `RESOLVED` | fix shipped: startup.S:47-49 .data LMA→VMA copy |
| 6531 | 2026-09-14 — resume scheduler: ctx_save/ctx_restore misrouted the pc through the kernel-stack slot | `RESOLVED` | entry "Fix SHIPPED 2026-09-14" — mepc CSR save/restore |
| 6827 | 2026-09-19: `n_dirty == 0` triggered a full-projection HtoD on every launch | `RESOLVED` | fix in briev_dev_cuda.c:554-558 (`n_dirty` loop only if `!full_sync`) |
| 6849 | 2026-09-20: composite-expanded softmax body 2×es on the knob-off lane-reduction path [RESOLVED 2026-09-21 — runtime, not compiler] | `RESOLVED` | entry RESOLVED 2026-09-21 — prime snapshot/restore, 02418f49 |
| 7627 | dot_row 'Float' typecheck failure [CLOSED — NOT a compiler defect; the test harness corrupted itself] | `RESOLVED` | entry CLOSED — harness corruption, not a compiler defect |
| 7687 | Typechecker: `dot!` composite invocation — "undefined variable 'Float'" flips on shape/name [OPEN — 2026-10-03; SUPERSEDED 2026-10-03: CLOSED as harness corruption at :7627 — the corrupted reference file, not the compiler] | `RESOLVED` | superseded by line 7627 (corrupted reference file) |
| 7863 | Naive-lane f16 GEMM under-accumulates at (M·N ≤ 4096, K ≥ 128): y = the k=0 term only [RESOLVED 2026-10-04 (same day) — harness artifact, not a compiler defect; header corrected 2026-10-04] | `RESOLVED` | entry RESOLVED — fixture seed overflow, not a compiler defect |
| 8418 | json.bv round 2 — callable `txn` convergence is linear (blocks json runtime) + 6 more codegen fixes 2026-10-05 | `RESOLVED` | superseded by line 8463 FIXED; repro `sum_to` converges → prints 10 |
| 1873 | 2026-05-29 — Briev-written lexer rejects all input with "Unexpected character" | `SUPERSEDED` | selfhost CLI gone; lib/compiler excluded as tamer-WIP (conformance.rs:227) |
| 1916 | 2026-05-29 — `dispatch_mode` lost during desugaring and import resolution | `SUPERSEDED` | `#pragma` tokens removed (lexer.rs:436); DispatchMode derived (strategy.rs:64) |
| 2124 | 2026-06-04 — Exit expression Neg(Integer) not handled in emit_exit_expr | `SUPERSEDED` | `#!exit` pragma removed (lexer.rs:436, SPEC.md:767) |
| 2138 | 2026-06-04 — Universal loop hangs with decreasing counter contract | `SUPERSEDED` | fixed by the 2147 entry (Gt/Ge + is_decreasing) |
| 2850 | 2026-06-11 — Tuple destructuring assignment `&(a, b) = expr` missing | `SUPERSEDED` | `Expr::TupleDestructure` removed (6867b50a); `let (a,b)` documented (learn-briev/04-functions.md:109) |
| 3242 | 2026-06-22 — `.N\|>` consumed as field access by `parse_postfix` | `SUPERSEDED` | pipe `\|>` removed (docs/architecture/universal-chaining.md:5) |
| 3261 | 2026-06-22 — Pipe skip overflow silently clamped to 0 | `SUPERSEDED` | pipe-chain desugarer gone with the feature |
| 2793 | 2026-06-11 — float_math_nonzero: 1.09x prior-state overhead (accepted) | `BY-DESIGN` | entry decision: Accepted; now 1.21x (results 2026-09-30-b1-rebaseline.md:17) |
| 3617 | 2026-07-11 — No borrow checker (alias safety gap) | `BY-DESIGN` | docs/architecture/features/ptr.md:11,165 — safety by contracts, explicitly no borrow checker |
| 3745 | 2026-07-18 — node main loop never exits (test impact) | `BY-DESIGN` | entry: architectural — a node is a perpetual reactive system |
| 3792 | 2026-07-18 — `else` keyword not supported | `BY-DESIGN` | parser rejects if/else with a SPEC §11.1 diagnostic (parser/statements.rs:117-135) |
| 2199 | 2026-06-05 — Compile-time-known list size causes precomputation (correct behavior) | `NOT-A-BUG` | entry states precomputation correct by design; observability razor |
| 2876 | 2026-06-11 — `<-` on txn parameters (NOT a bug) | `NOT-A-BUG` | title/verdict: not a compiler bug — `&result <- items[i]` is correct usage |
| 2896 | 2026-06-11 — `\|\|` in `term` statements (NOT a bug) | `NOT-A-BUG` | title/verdict: not a bug; `\|\|` parses via parse_or |
| 5064 | Deferred dead surface — spec-conformance plan 2026-08-22 | `N/A-RECORD` | deferred dead-surface checklist + plan pointer — an owner decision, not a bug |
| 5438 | Conformance sweep: 67 active sources fail the real-pipeline gate (2026-08-23 update #8) | `N/A-RECORD` | burn-down progress log; sweep CLOSED (line 5430), 0 failures (line 5731) |
| 5768 | Terminology tripwire: the five meanings of "Bits" — canonical spelling is `Bit<N>` — DOCUMENTED 2026-09-01 | `N/A-RECORD` | terminology tripwire — a documented rule, not a bug |
| 5805 | Tree-revert hazard executed — self-inflicted, 2026-09-01 | `N/A-RECORD` | tree-revert hazard incident record; the rule is documented |
| 5923 | 2026-09-04 — NVIDIA coopmat compiler: three silent-optimization behaviors (INSTRUMENT FINDINGS) | `N/A-RECORD` | instrument findings — NVIDIA driver behaviours, not Briev defects |
| 6056 | 2026-09-10 — PTX mw kernel: three device-level traps (cp.async era) | `N/A-RECORD` | incident record: three device traps; env workarounds shipped |
| 6183 | 2026-09-12: reactive realization gap investigated — deferred; causal DAG proceeding | `N/A-RECORD` | investigation/decision record — fusion measured unnecessary |
| 6249 | 2026-09-12: invalid A/B — dispatch constant edited, dump-test cubin benched | `N/A-RECORD` | invalid-A/B incident record; the measurement rule is documented |
| 6387 | 2026-09-13: E4c window — three traps that cost measurement time | `N/A-RECORD` | measurement-trap rules record; item 3 fixed 2026-09-13 |

## Effect on the distance estimate

`docs/plans/INDEX.md`'s open-bug list was largely stale. Corrected picture
for `.bv` stranger-usability:

- **Language core:** sound. 118 of 152 unmarked ledger entries are
  stale/resolved/by-design; all 30 stdlib modules typecheck; the
  conformance sweep is green at tip.
- **Fixed during this sweep:** the `list_concat` silent miscompile (finding
  1), the interpreter's missing `<-` push that broke every list accumulator in
  the reference (finding 1b), the json-interpreter array hang and its two
  companion Rule-5 divergences (finding 1c), and the 12 stale normalizer
  warnings. Suite 2888 → 2895.
- **Blocking (remaining):** the empty-program diagnosis for unfired txns, then
  packaging (package/module v0) and the install story — the last two are
  Phase 1.2/1.3 of the three-surfaces plan and are pure infrastructure.
- **Not blocking but real:** `lib/compiler` dogfood (10/12 fail), `dyn Trait`
  LLVM lowering, `hardware_validator` dead gate.

