# Next steps — research findings (2026-11-17)

Follows `docs/plans/2026-11-17-trunk-next-steps.md`. PR #122 is landed and the
trunk is green. This document records the research done to decide the next
workstream, with file:line evidence for each.

## A. S4 — populate `HirProtocol::ops` (next FFI step, LOW RISK)

### What HIR carries today
`src/hir/program.rs:379` — `HirProtocol { name, category, casts, ops }`.
`ops: Vec<HirProtocolOp>` where `HirProtocolOp { op: String, binding: String }`.

### What the lowering drops
`src/hir/lower.rs:154` — `let ops = Vec::new();` is a **hardcoded empty vec**.
The AST `ProtocolDef` (`briev-verify/src/ast/top.rs:1560`) carries:
- `cross_ops: Vec<OperatorDef>` — each has `op`, `params`, `impl_args: Option<PropertyValue>`, `impl_name`, `pre`, `suf`, `trusted_axiom` (top.rs:1518–1545)
- `contract: Option<Contract>` — NOT lowered (not in HIR)
- Cast edges carry `trusted_axiom: bool` (top.rs:1580) — NOT in `HirCastEdge`

So the dropped facts are: **op bindings** (the `ops` field, trivially
populatable), **protocol contracts** (a `Contract` — a larger struct), and
**`trusted_axiom` markers** on ops and cast edges.

### What the audit counts
`src/hir/stats.rs:57–62` — `count_protocols` counts **every** proto as
unsupported, unconditionally. The comment says "counts each proto until S4
populates them". So the fix is two-part:
1. **Populate `ops`** in `lower_protocol_item_full` (lower.rs:154) from
   `pd.cross_ops` — for each `OperatorDef`, take `op` and the fn name from
   `impl_args` (via `collect_binding_fns`, which already extracts it).
2. **Change `count_protocols`** to only count protos that still have a gap:
   a proto with `ops` populated but no `contract` still has a gap
   (contract is not representable). A proto with no ops AND no contract is
   fully represented → count 0. The `boundary_facts` counter stays 0 (H2 is
   complete).

### Consumer impact
`protocol_binding_fns` (lower.rs:31, 161, 172) already collects op binding
fn names via `collect_binding_fns` — so liveness (`src/hir/defn_liveness.rs:99`)
already sees them. Populating `ops` does NOT change liveness. It only makes
the HIR audit honest.

**Exit condition:** HIR audit on the BUGS.md boundary fixture reports
`unsupported=0 (protocols=0 boundary=0)` for a proto that has ops but no
contract; `unsupported=1` for a proto with a contract. New test
`protocol_ops_populated_and_certified` replaces the old
`proto_is_counted_until_cross_ops_populated`.

## B. Issue #5 — `check` vs `build` gap (MEDIUM RISK)

### What I know from the issue
"check succeeds while build fails because check path stops before
normalization and codegen validation."

### What I need to find
- The CLI dispatch in `src/main.rs` — which subcommand runs which phases.
- The `check` path: parser → typecheck → (stop). The `build` path: parser →
  typecheck → normalize → codegen → opt → (stop).
- Whether there's an existing flag that makes `check` run codegen.

**Fix plan (pending dispatch trace):** Add a `--validate` flag to `check` that
runs the codegen validation step (emit IR, run `opt -verify`) without
linking. Add a test that compiles a fixture with `check --validate` and
asserts it fails where `check` alone passes.

**Risk:** Medium. Changing the `check` path could affect the corpus
(`verify_bodies_corpus.sh`) if it relies on `check` being lightweight.

## C. Issue #8 — `term` in transaction guard flow (HIGH RISK)

### What I know from the issue
"`term` in transaction guard flow does not enforce single-decision
termination and can emit contradictory outputs."

### Hypothesis
A `Statement::Guarded(cond, body)` inside a transaction can have `term` in
both the `cond`-true and `cond`-false paths, emitting two `ret` instructions
to the same basic block. LLVM's `opt -verify` would reject this, but the
emitter doesn't check.

**Fix plan (pending code trace):** In the guarded-statement emitter, ensure
that if the `term` is in the guard body, the else-path falls through to the
end of the transaction (or emits a `ret` with a default value). Add a test
fixture with a guard that has `term` in both branches and assert the IR
validates.

**Risk:** High. Guarded statement emission is one of the most intricate
parts of the emitter. A wrong fix could silently change control flow.

## D. Issue #7 — unresolved symbols in outlined/nested paths (HIGH RISK)

### What I know from the issue
"LLVM emission can reference unresolved state or local symbols in outlined
and nested control-flow paths."

### Hypothesis
An outlined call (or nested block) references a local binding that was
declared in a different scope, or a state field that wasn't registered in
the `%State` struct for that function. The emitter doesn't track which
symbols are in scope for each outlined path.

**Fix plan (pending code trace):** Track the set of declared symbols per
outlined/nested path. Add a validation pass that checks every symbol
reference in the emitted IR is declared in the current function scope.
Add a test fixture that triggers the unresolved reference.

**Risk:** High. Same as #8 — emitter correctness, no clear scope.

## E. PF_exp induction (LOW PRIORITY, PROVER-SIDE)

Tracked but not started. This is a prover task (briev-verify), not a compiler
task. Defer until B/C/D are clear.

## Recommended order

| Priority | Task | Risk | Why |
|---|---|---|---|
| 1 | **S4** (A) | Low | Self-contained, unblocks the FFI audit, no emitter risk |
| 2 | **#5** (B) | Medium | User-visible, needs a flag not a refactor |
| 3 | **#8** (C) | High | Correctness bug, needs careful tracing |
| 4 | **#7** (D) | High | Correctness bug, needs careful tracing |
| 5 | **PF_exp** (E) | — | Prover-side, defer |

## Open questions before BUILD

1. **S4**: Does `HirProtocolOp` need `trusted_axiom`? The AST has it on
   `OperatorDef` (top.rs:1544). If the audit is supposed to certify
   "trusted" vs "derived" ops, the flag needs to be in HIR.
2. **#5**: What exactly does `check` run today? Need to trace the CLI
   dispatch in `src/main.rs` before writing the fix.
3. **#8**: Does the guarded-statement emitter already handle `term` in the
   else-path? Need to read `emit_stmt.rs` `Statement::Guarded` arm.
4. **#7**: What does "outlined" mean in this context? Is it a specific
   emitter path (outlined calls) or a general scope issue?
