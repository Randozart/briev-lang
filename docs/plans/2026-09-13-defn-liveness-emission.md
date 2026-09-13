# Defn Liveness Emission — Plan (2026-09-13)

**Status**: IN PROGRESS. Worktree `briev-rv64-capability`, branch
`feat/rv64-capability-kernel`.

## 0. Goal

The compiler must emit only definitions transitively reachable from live
code. Today it emits a `define` for **every** imported definition and
delegates dead-code elimination to LLVM LTO. That delegation is the last
blocker on the rv64 bare-metal path and a standing cost on every target:
the hello-world program (3 live functions) links 252 defines because the
prelude imports 14 stdlib modules.

One sentence: **imports grant capability; liveness gates emission cost.**

## 1. Diagnosis (evidence chain, 2026-09-13)

Found while closing the rv64 boot gap (Addendum C,
`2026-09-11-rv64-capability-kernel.md`):

| Observation | Evidence |
|---|---|
| Backend emits every imported defn | `src/backend/llvm/mod.rs:3824-3860` — the Definitions loop has no reachability filter |
| `--no-std` is broken for `.bv` | `config/targets.dbvl:16` routes `.bv` to plugin `prelude-native`; `--no-std` disables plugin named `prelude` (`src/pipeline.rs:786-788`) — different name, flag does nothing |
| Without LTO, `-nostdlib` links fail | undefined `malloc`/`memcpy`/soft-float symbols referenced from *emitted-but-unused* stdlib defines (`int_to_str`, `pf_make`, `briev_str_substr`, `__briev_coll_resize`) |
| With LTO, links fail differently | `R_RISCV_HI20 out of range` — LTO-placed string constants break PC-relative relocations under the linker script |
| The `Asm#("Prefetch")` panic | `asm.bv`'s unused `prefetch`/`rdtsc` defns were being *emitted* (and their bodies lowered) — with liveness emission they are never reached, no lowering rows needed |

Interpretation: this is a language gap, not a toolchain problem. Correctness
of a `-nostdlib` link must not depend on an optimizer pass.

## 2. Design

### 2.1 New frontend pass: `src/analysis/defn_liveness.rs`

`DefnLiveness::build(items) -> DefnLiveness { live: HashSet<String> }`,
called from `analyze_program` (`src/backend/mod.rs`), stored as
`AnalysisResults.defn_liveness`. The backend consumes; it never re-derives
(frontend-driven-dispatch pillar).

**Roots** (a root is always emitted):

- Every `Transaction` with `is_reactive` — the reactor fires these by name.
  Descend into `Obj`/`Cell` members (`StructDefinition.members: Vec<TopLevel>`)
  for nested nodes; unwrap `SyncGroup`/`Cfg`/`Fuzzed`.
- Every `Export` inner item — ABI surface.
- Every `IsrHandler` — vector tables reference handlers by symbol.
- Every `AsmFn` — top-level observable asm.
- Every `TypeDefOperator` and `Impl` behavioral member — dispatch names are
  mangled from (type, op) at emission; AST-level closure cannot predict the
  mangled name. Conservative keep (these are few). REFINEMENT (deferred):
  compute the mangling and root precisely.
- Spawn targets: `Expr::Spawn { type_name, .. }` — the spawned callable is
  rooted (fn-pointer tables).
- The synthesized `__init` transaction (top-level `Statement` items are
  wrapped into `node __init [!__booted_N][__booted_N]` before analysis).
- Reflection: if live code contains any `.^^` (`Expr::Reflect` with
  `ReflectKind::CompileTime`), keep ALL defns (coarse, sound; reflection is
  a documented liveness root — it can reach any member by name).
  REFINEMENT (deferred): reflect on the referenced names only.

**Closure** (worklist over live callable bodies): every `Expr::Call(name)`
resolves to either a program defn/txn (add to worklist) or an intrinsic
(name ends `#`, per the intrinsic tables) whose lowering may emit helper
calls → add the helper defns named in the centralized table below.
`Expr::Spawn` targets are rooted during the walk.

### 2.2 Centralized intrinsic→helpers table (Rule 17/18)

Today the mapping "intrinsic lowering may call program defn X" exists only
as scattered `defn_params.contains_key("X")` checks at ~25 emission sites.
The liveness pass needs the same knowledge; re-scattering is forbidden.
`defn_liveness::intrinsic_helpers(intrinsic) -> &'static [&'static str]`
becomes the one table; emission sites keep their `contains_key` behavior
unchanged (their question is calling convention, not liveness — migrating
them is a REFINEMENT, not required for soundness here).

Known rows (audited 2026-09-13 from the `contains_key` grep):

| Intrinsic | Helpers (pure-Briev defns) |
|---|---|
| `Print#`, `PrintLn#`, `Eprint#`, char/bool/float prints | `__print`, `__print_str`, `__print_int`, `__print_bool`, `__print_float`, `__print_char`, `__eprint_str`, `write_all` (explicit calls inside these close the rest: `int_to_str`, `float_format`, …) |
| `==` on `#String` values | `briev_str_eq` |
| `Slice#` on strings | `briev_str_substr` |
| foreach over String | `briev_str_next_char` |
| Char arithmetic/length | `briev_char_len` |
| CStr | `cstr_len` |
| coll growth (InsertAt/push family) | `__briev_coll_resize` |
| `BracketOp::Mask` | `briev_mask_select`, `briev_mask_select64` |
| `Free#` / scheduler auto-free | `__briev_free` |
| `Now#` | `__briev_now` |
| env/fs (getenv/chdir adapters) | `__briev_getcwd`, `__briev_chdir` |
| `EndProgram` / exit | `__exit` |
| print buffering (main tail flush) | `__stdout_flush` |

Computed-name sites (`f32sym`, `helper[1..]`, `type_name`, `fname`, `sym`
at emit_expr.rs:1809/1846/2099/6299, emit_stmt.rs:1211, intrinsics.rs:2322)
are audited at implementation time; any that can name a program defn get a
table row or a conservative keep.

### 2.3 Backend gate + safety net

1. Gate the Definitions emission arm (mod.rs:3826): skip defns not in
   `live`. `defn_params` registration stays ALL-defns — call-lowering
   convention checks and the `emit_toplevel` fallbacks keyed on
   `contains_key` must keep seeing the full surface.
2. Gate callable-txn emission the same way (reactive txns are roots, so
   only non-reactive reachability matters). The existing
   "'X' is never dispatched" warning is unaffected.
3. **Safety net (the load-bearing piece)**: after emission, scan the
   generated IR for `call … @<name>` where `name` is a program defn NOT in
   `live` → panic: "liveness pass missed `<name>` (called from emitted
   code) — add the intrinsic→helper row in
   src/analysis/defn_liveness.rs". An incomplete table is thus an
   immediate, actionable compiler bug — never a silent linker mystery.
   Under-approximation cannot ship silently.

Unreferenced `private unnamed_addr constant` string globals from dead
functions die via LLVM module DCE at `-O3` even without LTO — no extra
work needed for the constants.

### 2.4 `--no-std` honesty fix

`src/pipeline.rs` (the `opts.no_stdlib` branch): disable the prelude
FAMILY (`prelude`, `prelude-native`, `prelude-hw`, `prelude-electronics`),
not just `prelude`. Explicit `--disable-plugin <name>` stays exact-match.

With liveness emission, `--no-std` becomes purely a capability statement
("don't even import the prelude") — the two fixes compose at different
layers.

## 3. Phases

1. **Plan + docs** (this file, `docs/architecture/defn-liveness.md`,
   AGENTS.md row) — committed first.
2. **Pass + table + AnalysisResults field** with unit tests
   (dead dropped, transitive kept, every root kind, helper rows kept,
   reflection keep-all).
3. **Backend gate + IR-scan net**; full `cargo test --lib`; fix any fixture
   that asserted dead code IS in the IR (behavioral fix: make it reachable).
4. **`--no-std` family fix** + test.
5. **QEMU validation**: both hello variants must boot —
   `examples/hello_rv64.b.bv` (direct VolatileStore#) and the foreach
   idiom (`foreach c in s { VolatileStore#(addr, c as Int); }` — the
   `briev_str_next_char` chain is `Load#`/`Store#` only, so it must link
   freestanding once emission is liveness-gated).
6. Compile-time smoke on a benchmark (emission set shrinks; link time
   should not regress).

## 4. Risks / undo

- Incomplete helper table → caught by the IR-scan net (loud panic).
- Hosted programs relying on dead-defn emission: none can — the only
  consumers of emitted code are calls, and calls make defns live.
- Library/shared modes: exports are roots; ABI surface unchanged.
- Undo: revert the emission gate (one `if`) and the AnalysisResults field;
  the pass module remains inert. The `--no-std` fix is independent.

## 5. Provenance

- Authored 2026-09-13, agent session (opencode), at user direction
  ("we found a language gap. We need to fix this cleanly" /
  "default on, but eliminate what isn't used").
- Evidence: live greps/reads cited in §1; QEMU boot log in Addendum C.
- Companion: `2026-09-11-rv64-capability-kernel.md` (the vehicle that
  exposed the gap), `docs/architecture/briev-execution-model.md`
  (the reactor model that defines "live").
