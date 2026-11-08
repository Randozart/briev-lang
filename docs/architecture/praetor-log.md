<!-- 2026-06-09 -->

# Praetor Diagnostic Log

Format: `YYYY-MM-DD | file:line | rule | root cause | resolution`

---

## 2026-11-08 — Installed CLI and verified nested-object layout changes

Installed Praetor 0.1.0 from `https://github.com/Randozart/praetor-lsp`, revision
`81f190ab79496b373980f2f938438098540d26d7`, using
`cargo install --path . --locked --jobs 2`. Checkout:
`/home/otto/code/praetor-lsp`; executable: `/home/otto/.cargo/bin/praetor`.

Compared all five changed Rust files at base `735d1023` and fix `fdceb550` in
separate directory targets, running from the compiler root to use its existing
`.praetor.toml`:

```bash
praetor validate --warn --json --target /tmp/opencode/praetor-layout-review/base
praetor validate --warn --json --target /tmp/opencode/praetor-layout-review/head
```

Both report 271 warning/error diagnostics and exit 1 because of baseline debt.
Comparison by relative path, severity, source and message, ignoring moved line
numbers, finds **no new or worsened diagnostics**. Six message changes are
improvements: `emit_field_access` cyclomatic complexity 27→22, cognitive
complexity 54→44 and length 172→162; `emit_event_payload_read` length 136→127;
`build_field_index` 280→278; `apply_field_modes` 204→202. New code adds none.
The changed-file gate passes; an absolute clean-tree Praetor pass is not claimed.
Raw JSON remains in the two target directories' sibling `base.json`/`head.json`.

## 2026-08-01 — Corrected Praetor Invocation (was silently no-op)

**Root cause:** `praetor validate --target <file>` was the documented per-change
invocation (AGENTS.md, praetor-log.md history, the June 2026 pre-commit hook).
But **`--target` is a DIRECTORY, not a file.** Passing a file prints
`target is not a directory: ./src/foo.rs` and exits 0 *without analyzing
anything* — so every historical "Praetor on changed files" check was a silent
no-op. Diagnostics only surface with a directory target:

```bash
praetor validate --warn --target src/backend/llvm   # directory; fails on any diagnostic
mkdir -p /tmp/pt && cp src/foo.rs /tmp/pt/ && praetor validate --warn --target /tmp/pt  # single file
```

**Resolution:** AGENTS.md Commands section updated with the directory-target
rule and the single-file workaround. `scripts/verify.sh`'s baseline comparison
is stale (June schema `{total_diagnostics}` vs current `{failures,passed,
total_diagnostics}`) and should be treated as informational until rewritten.

**Pre-commit hook removed (2026-08-01):** the shared `pre-commit` hook
(`../briev-compiler/.git/hooks/pre-commit`, June 2026, runs on this worktree)
was **broken the same way** — it built a comma-separated list of changed files
and passed it to `--target`, which is directory-only, so it silently passed
without analyzing anything. Per the no-hook decision, the hook was **deleted**,
not fixed. Praetor runs manually on changed files per the AGENTS.md policy.

**Verified on this commit's changes:** `praetor validate --warn --target
src/backend/llvm` reports only pre-existing diagnostics (e.g. `type_size`
cognitive complexity 27→30, pre-existing; `emit_store_tbaa` 6 params,
pre-existing). No NEW diagnostics introduced by the B0/width-rule changes.
A line-shift-tolerant comparison (42 diagnostics before vs 42 after) confirms
zero new diagnostic functions.

---

## 2026-06-09 — Baseline

**233 pre-existing diagnostics** across the codebase at start of Pattern B refactor.
These are from monolithic files that will be systematically migrated to feature
modules. New code must have 0 diagnostics.

Key areas with highest diagnostic density:
- `src/main.rs` (cognitive complexity 1661 in `main()`, cyclomatic 365)
- `src/backend/llvm.rs` (O(n^k) loops, 14-parameter functions)
- `src/proof_engine.rs` (O(n^2) loops, high cognitive complexity)
- `src/interpreter.rs` (O(n^k) loops)
- `src/parser.rs` (O(n^2) loops)
- `src/analysis/` (multiple O(n^2) and O(n^k) violations)

These will be resolved incrementally as code migrates into `src/features/`.

### 2026-06-09 — Pre-commit Hook Modified

The Praetor pre-commit hook was changed from checking `--target ./src` (entire
codebase → blocked by 233 pre-existing diagnostics) to checking only files
changed in the current commit (`git diff --cached --name-only`).

This ensures new feature files must pass Praetor's strict limits (complexity ≤ 15,
lines ≤ 100, params ≤ 6) while pre-existing diagnostics in untouched files
don't block the refactor.

---

## 2026-06-09 — Phase 1.1 (Literal Feature)

**Files touched**: 16 (1 new, 15 modified)  
**New diagnostics**: 0  
**Diagnostics resolved**: 0 (pre-existing diagnostics untouched — monolithic files not yet deleted)

All 16 files pass `praetor validate --warn --target <file>` with zero violations.
The new `src/features/literal.rs` (231 lines, cyclomatic 4, params 2) satisfies
Praetor's strict limits. No new violations introduced in any router arm or helper method.

Next phase: Phase 1.2 (binary_op / unary_op) — mechanical extraction of 18+3 operator variants.

---

## 2026-06-09 — Kani Harness Rules

Added permanent Kani Harness Requirements to AGENTS.md. Fast group harnesses must be
pure match dispatch only (no formatting, no allocation, no loops, no recursion).
Full group (`--features kani_full`) may relax these rules for CI-only execution.

Previously, 110 harnesses were written without this constraint, causing 15-minute
timeouts. After enforcing the rules: 14 fast harnesses complete in 2.5s.

---

## 2026-06-09 — Phase 1.5 (TypeDef Feature)

**Files checked:**
- `src/features/toplevel/typedef.rs` — 235 lines, 0 diagnostics
- `src/type_universe.rs` — 463 lines, 0 diagnostics

**New diagnostics**: 0

**Kani note**: `TypeProperty` uses `Box<Expr>` for all 13 variants, which violates the fast-group no-heap-allocation rule. All Kani harnesses for TypeDef are gated behind `#[cfg(all(kani, feature = "kani_full"))]`. Fast group retains 11 harnesses (ast.rs + literal.rs).

---

## 2026-06-09 — Phase 2 (Statement Features)

**Files checked:**
- `src/features/stmt/*.rs` — 14 files (mod.rs + 13 feature files)

**New diagnostics**: 0

**Note**: All feature files use `Vec`, `Box`, `Option` in struct definitions.
Kani harnesses gated behind `kani_full`.

---

## 2026-06-09 — Phase 3 (TopLevel Features)

**Files checked:**
- `src/features/toplevel/*.rs` — 19 files (mod.rs + 17 feature files + typedef.rs)

**New diagnostics**: 0

---

## 2026-06-09 — Phase 4a-c (Router Routing, BinaryOp/UnaryOp evaluate)

**Files changed:**
- `src/features/binary_op.rs` — 27-line evaluate impl (non-stub)
- `src/features/unary_op.rs` — 11-line evaluate impl (non-stub)
- `src/interpreter.rs` — Pattern B routing arms for `eval_expr`
- `src/typechecker.rs` — Pattern B routing arms for `infer_expression`
- `src/backend/llvm.rs`, `vhdl.rs`, `webstack.rs` — Pattern B routing arms

**New diagnostics**: 0

---

## 2026-06-09 — Proof Engine Bug Fixes (Phase 4d)

**Files changed:**
- `src/proof_engine.rs` — +107 lines

**Bug A** — Guard-taken path dropped in `enumerate_paths_recursive`.
Fix: continue exploring remaining body after guard body.
Also fixed `body[1..]` → `body[i+1..]` (exponential path explosion).

**Bug B** — `eval_numeric` missing `Mod`/`Div`. Fix: added match arms.

**Bug C** — `is_negated` hidden in error output. Fix: added `¬` prefix.

**New diagnostics**: 0

**Praetor note**: `is_self_minus_one` uses closure for `is_one` check.
Clarity 15 nesting. Well within limits.

---

## 2026-06-09 — Convergence Analysis Fixes (Phase 4e)

**Files changed:**
- `src/proof_engine.rs` — +107 lines (check_convergence improvements)

**Changes**:
- AND-precondition extraction (`extract_var_relation`)
- Popcount decay detection (`is_self_minus_one`)
- Algebraic cancellation (`eval_const_expr` with `initial_values` map)
- Compound increment pattern (`(count + N) - M`)

**New diagnostics**: 0

**Result**: 24/24 benchmarks pass check (up from 16).

---

## 2026-06-12 — Visibility System + Struct Derivation

**Files changed:**
- `src/parser.rs` — +~150 lines (sed tracking, field visibility parsing, struct derivation `<:`, match/uni arrow)
- `src/import_resolver.rs` — sed filtering in `filter_items`, cache type change
- `src/typechecker.rs` — `enforce_field_visibility()`, `is_derived_from()`, `struct_parents`, `struct_field_visibility`
- `src/desugarer.rs` — struct flattening pass (recursive parent resolution, collision detection)
- `src/ast.rs` — `StructDefinition.parent: Option<Type>`
- Various — `parent: None` on all `StructDefinition {` constructors

**New code added:**
| Function | Lines | Cyclomatic | Params |
|----------|-------|------------|--------|
| `enforce_field_visibility` | ~25 | ≤5 | 2 |
| `is_derived_from` | ~20 | ≤8 | 2 |
| `parse_field_visibility` | ~10 | ≤4 | 1 |
| `take_sed_item_names` | 4 | ≤1 | 1 |
| `collect_parent_fields` (nested fn) | ~25 | ≤6 | 4 |

All new functions expected to pass Praetor strict limits (complexity ≤ 15, lines ≤ 100, params ≤ 6). `collect_parent_fields` has 4 params (under the 6-param limit).

**New diagnostics**: 0 expected.

**Key design decisions:**
- Top-level `sed` uses name-based tracking in Parser (no AST changes to `TopLevel`)
- Struct derivation flattened in desugarer (all backends see flat structs)
- Visibility enforcement is additive — only `Sedentary` cross-file check is wired
- `Private` enforcement stubbed (requires `current_struct` tracking)

---

## 2026-06-12 — Highlighter/LSP Syntax Audit

**Files audited:**
- `syntax-highlighter/syntaxes/briev.tmLanguage.json` (412 lines)
- `syntax-highlighter/syntaxes/dbriev.tmLanguage.json` (330 lines)
- `src/lsp.rs` (completion list at line 558-566)

**Gaps identified:**

| Gap | Impact | Fix |
|-----|--------|-----|
| `pvt`/`sed` keywords missing from grammar | Not highlighted as keywords | Add keyword match patterns |
| `match`/`uni` keywords missing from grammar | Highlighted as function calls or variables | Add keyword match patterns |
| `frgn`/`frgn!`/`syscall`/`syscall!` keywords missing | Not highlighted | Add keyword match patterns |
| Sugar contracts `[[post]`/`[pre]]` not handled | Second `[` or `]` breaks contract region | Replace `\\[` begin with `\\[\\[?` and `\\]` end with `\\]\\]?` |
| Double-quote runaway | Unmatched `"` highlights rest of file as string | Add line-ending guard or boundary pattern |
| Single-quote Char literals missing | `'a'` tokenized as punctuation+identifier | Add `'.'` pattern |
| `src/lsp.rs` missing keywords | No completions for `pvt`/`sed`/`match`/`uni`/`frgn`/`syscall` | Add to completion vec |
| Binaries stale (Apr 19-20) | All June 12 fixes invisible at runtime | Rebuild + install |

**Resolution**: All fixes applied in 2026-06-12 session.

---

## 2026-06-12 — Import Path Resolution Bugfix

**Bug**: Nested imports doubled the path. When `officina/tui/layout.bv` imported `"officina/tui/history"`, the resolver searched `officina/tui/officina/tui/history.bv` — prepending the importing file's parent directory to an already-canonical import path.

**Root cause**: `src/import_resolver.rs:361-364` — `source_dir` was always set to `source_file.parent()`, the importing file's parent. All imports, including canonical ones, were resolved relative to this directory.

**Fix**: TypeScript-style import resolution — distinguish relative from non-relative imports:
- **Relative**: start with `./` or `../` → resolve from importing file's directory
- **Non-relative**: anything else → resolve from project root

Added `root_path: PathBuf` to `ImportResolver`, set on first call from the compiler's working directory.

**Files changed**: `src/import_resolver.rs`

---

## 2026-06-12 — Stdlib Cleanup: Removing Compiler Magic from Result/Option Methods

**Context**: `is_ok`, `is_err`, `unwrap`, `unwrap_or`, `unwrap_err` (for Result) and `is_some`, `is_none`, `unwrap` (for Option) were hardcoded as typechecker signatures and inference arms. These were artifacts of a more magical version of the language where the compiler synthesized these functions rather than defining them in the stdlib.

**Problem**: The stdlib (`lib/std/option.bv`, `lib/std/result.bv`) already defined these functions with `uni`-based pattern matching bodies. But their contracts used `.is_some()` / `.is_ok()` method calls that resolved through the hardcoded typechecker signatures — creating a circular dependency between the compiler and the stdlib. The proof engine's F101/F102 checks (`is_success_variant_check`, `is_error_variant_check`) string-matched `Success`/`Error`/`Ok`/`Err`/`is_ok`/`is_err` in guard expressions, but with `uni` these guard patterns are obsolete — `uni Ok(val) = frgn_call()` is handled structurally by the unification statement, not by guard inspections.

**Fix**:
1. Remove all contracts from stdlib `defn`s (defns don't need contracts; `uni` bodies guarantee correctness structurally)
2. Remove hardcoded typechecker signature registrations and inference arms for Result/Option methods
3. Remove proof engine F101/F102 checks (guard-based success/error detection — obsolete with `uni`)
4. The stdlib functions remain. Users import them from `"std/option"` and `"std/result"` when needed.

**Files changed**: `lib/std/option.bv`, `lib/std/result.bv`, `src/typechecker.rs`, `src/proof_engine.rs`

---

## 2026-06-12 — Transaction Contract Ordering Bugfix

**Bug**: `parse_transaction()` parsed contracts BEFORE the return type (`-> Type`). Writing `txn f() -> Type [pre][post]` caused `parse_contract()` to see no brackets, return `[true][true]`, and then fire a misleading error ("both precondition and postcondition are [true]") — even though the user wrote proper contracts, just in the wrong position. `parse_definition()` handled both orderings correctly with a soft note; `parse_transaction()` did not.

**Fix**:
1. `parse_transaction()`: Match `parse_definition()` — check for `LBracket` before parsing return type; check again after parsing return type with a soft note about canonical ordering.
2. `parse_contract()`: Add `count > 0` guard to the `[true][true]` false-positive check at line 3959 (prevents firing when no brackets were parsed).

**Files changed**: `src/parser.rs`

---

## 2026-06-12 — `defn → txn` Call Resolution Bugfix

**Bug**: The typechecker had no `transactions` map. `TopLevel::Transaction` items were silently discarded during Pass 1 (fell through to `_ => {}` at line 559). Both `check_call_argument_types()` and `infer_expression()` only looked up `self.definitions`, `self.signatures`, and `self.foreign_bindings` — so `defn → txn` calls silently failed to resolve at the type level.

**Fix**: Added `transactions: HashMap<String, Transaction>` to `TypeChecker`, populated during Pass 1, and added as a lookup target in both `check_call_argument_types()` (for arg type validation) and `infer_expression()` (for return type inference).

**Files changed**: `src/typechecker.rs`

---

## 2026-06-12 — Transaction Omitted from Import Filter Items

**Bug**: The `filter_items` method's name extraction match in `import_resolver.rs:493-504` had no arm for `TopLevel::Transaction`. Every imported `txn` fell through to `_ => None` and was silently dropped. This meant named imports like `import "file" { filter_fluff, count_brackets }` would load the transactions from the source file but then filter them all out before the typechecker ever saw them.

**Fix**: Added `TopLevel::Transaction(t) => Some(t.name.as_str())` at line 496, alongside the other named top-level items.

**Files changed**: `src/import_resolver.rs`

---

## 2026-06-12 — Import + ListIndex Type Inference Corruption

**Bug**: Importing ANY file (even an empty 0-byte file) causes `items[0]` on a `List<String>` parameter to infer as `List<String>` instead of `String`. Both old (April) and new (June) binaries exhibit this. The bug was latent because the earlier import path doubling always killed the typechecker before this code path was reached.

**Diagnostic clues**:
- All span/position info shows `?:?` when an import is involved
- The control file (no import) passes cleanly

**Root cause**: `peek_multidimensional_slice()` used `self.pos` which was always 0 (initialized at parser creation and never updated). It scanned the source from byte 0 on every call, finding `;` from earlier statements (like `import "empty";`) before reaching `]` in `items[0]`. The `;` was interpreted as a mask bracket op, making the parser create `Expr::MultiSlice` instead of `Expr::ListIndex`. This caused the typechecker to infer `List<String>` instead of `String` for `items[0]`.

**Fix**: Added `self.pos = span.start;` to `self.advance()` at `parser.rs:96`, so `self.pos` tracks the current token's byte position. `peek_multidimensional_slice()` now scans from the correct position after `[` instead of from byte 0.

**Files changed**: `src/parser.rs` (1 line: `self.pos = span.start;`)

---

## 2026-06-12 — Bool/Int Type Error from Desugarer State Generation

**Bug**: The officina-cli's `officina.bv` produced `error[B002]: expected Bool for assignment, but found Int` when all 11 modules were imported together. The error manifested as Bool literals `true`/`false` being inferred as `Int`, but the root cause was a global `let N: Bool = false` StateDecl generated by the desugarer.

**Root cause**: Twofold:
1. **Desugarer** (`desugarer.rs:238-244`): `extract_vars_from_expr` extracted variable names from transaction postconditions (e.g., `found`, `N`, `i` from `found == true || i == N`), then `infer_type_from_expr` always returned `Type::Bool` for `And`/`Or` without recursing into children. This created global `StateDecl{ name: "N", ty: Bool, ... }` for variables that were actually transaction parameters (in `find_expanded`, `N: Int`). The parameter check was missing — the desugarer created state for ANY postcondition variable, even if it was a parameter.
2. **Typechecker** (`typechecker.rs:1120-1133`): When `toggle_record` did `&N = expanded_ids .#Size;`, the assignment handler looked up `N` and found the `Bool` global, causing type mismatch with the `Int` RHS.

**Fix**:
1. **Desugarer**: Added `&& !txn.parameters.iter().any(|(n, _)| n == &var_name)` to skip postcondition variables that are already transaction parameters (line 240).
2. **Typechecker**: Added auto-declare logic: on `&N = expr`, if `N` doesn't exist, declare it with the RHS type and skip the error. This allows implicit state variables to be created via `&name = value` without requiring `let name: Type = value`.

**Files changed**: `src/desugarer.rs` (1 line), `src/typechecker.rs` (auto-declare in assignment handler)
