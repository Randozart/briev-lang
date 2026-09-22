// ── Expression Codegen Helper Functions ─────────────────────────
//
// 2026-06-29: Extracted from emit_expr.rs to enable submodule extraction.
// Split via Rust's "impl block split" pattern — multiple files define
// `impl Type { ... }` within the same module without duplicating methods.
//
// 2026-07-13: Flattened to max 2-level nesting with guard clauses,
// doc comments on every definition, removed old-API references
// (IntrinsicCall → Call, Projection projection_target_name removed).
//
// Visibility convention:
//   `pub(crate)`  — visible to entire crate (semi-public API surface)
//   `pub(super)`  — visible to parent `llvm` module + children
//   (private)     — visible only within this file

use crate::ast::{BinaryOpKind, Expr, OutputType, Statement, Type};
use crate::backend::llvm::emit_stmt::emit_statement;
use crate::backend::llvm::*;
use crate::type_universe::ResolvedType;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::LazyLock;

impl LlvmBackend {
    // ═══════════════════════════════════════════════════════════════
    // Section 1: Cell Rewriting
    // ═══════════════════════════════════════════════════════════════

    /// Rewrite all `Identifier` nodes in an expression tree, prefixing
    /// each name with `cell${cell_name}$`. Used when expanding a cell
    /// definition into standalone transactions.
    ///
    /// 2026-06-29: Recursive tree walk — every compound variant recurses
    /// into its children. Leaf variants (literals, metadata) pass through.
    pub(super) fn rewrite_cell_identifiers(expr: &Expr, cell_name: &str) -> Expr {
        let prefix = |name: &str| -> String { format!("cell${}${}", cell_name, name) };
        match expr {
            // Leaves — no identifiers to rewrite
            Expr::Decimal(_)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::BeginProgram
            | Expr::Float(_)
            | Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _)
            | Expr::StructLiteral { .. }
            | Expr::FormattingAnnotation(_)
            | Expr::TaggedLiteral(_, _) => expr.clone(),

            // Identifier leaf
            Expr::Identifier(name) => Expr::Identifier(prefix(name)),

            // Compound — recurse into children
            Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
                *k,
                Box::new(Self::rewrite_cell_identifiers(l, cell_name)),
                Box::new(Self::rewrite_cell_identifiers(r, cell_name)),
            ),
            Expr::UnaryOp(k, e) => {
                Expr::UnaryOp(*k, Box::new(Self::rewrite_cell_identifiers(e, cell_name)))
            }
            Expr::Call(name, args, _) => Expr::Call(
                name.clone(),
                args.iter()
                    .map(|a| Self::rewrite_cell_identifiers(a, cell_name))
                    .collect(),
                None,
            ),
            Expr::Spawn { type_name, args, storage } => Expr::Spawn {
                type_name: type_name.clone(),
                args: args.iter()
                    .map(|a| Self::rewrite_cell_identifiers(a, cell_name))
                    .collect(),
                storage: *storage,
            },
            Expr::Field(obj, field) => Expr::Field(
                Box::new(Self::rewrite_cell_identifiers(obj, cell_name)),
                field.clone(),
            ),
            Expr::Reflect(recv, target, kind) => Expr::Reflect(
                Box::new(Self::rewrite_cell_identifiers(recv, cell_name)),
                target.clone(),
                *kind,
            ),
            Expr::MethodCall(recv, name, args, id, chain_refs) => Expr::MethodCall(
                Box::new(Self::rewrite_cell_identifiers(recv, cell_name)),
                name.clone(),
                args.iter()
                    .map(|a| Self::rewrite_cell_identifiers(a, cell_name))
                    .collect(),
                *id,
                chain_refs.clone(),
            ),
            Expr::Index(obj, idx) => Expr::Index(
                Box::new(Self::rewrite_cell_identifiers(obj, cell_name)),
                Box::new(Self::rewrite_cell_identifiers(idx, cell_name)),
            ),
            Expr::Block(stmts) => Expr::Block(
                stmts
                    .iter()
                    .map(|s| Self::rewrite_cell_stmt_identifiers(s, cell_name))
                    .collect(),
            ),
            Expr::If(cond, then_, else_) => Expr::If(
                Box::new(Self::rewrite_cell_identifiers(cond, cell_name)),
                Box::new(Self::rewrite_cell_identifiers(then_, cell_name)),
                else_
                    .as_ref()
                    .map(|e| Box::new(Self::rewrite_cell_identifiers(e, cell_name))),
            ),
            Expr::Match(value, arms) => Expr::Match(
                Box::new(Self::rewrite_cell_identifiers(value, cell_name)),
                arms.iter()
                    .map(|arm| crate::ast::MatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|g| Self::rewrite_cell_identifiers(g, cell_name)),
                        body: Box::new(Self::rewrite_cell_identifiers(&arm.body, cell_name)),
                    })
                    .collect(),
            ),
            Expr::Tuple(items) | Expr::List(items) => {
                Self::rewrite_tuple_or_list(expr, items, cell_name)
            }
            Expr::Lambda(params, body) => Expr::Lambda(
                params.clone(),
                Box::new(Self::rewrite_cell_identifiers(body, cell_name)),
            ),
            Expr::Cast(e, ty) => Expr::Cast(
                Box::new(Self::rewrite_cell_identifiers(e, cell_name)),
                ty.clone(),
            ),
            Expr::IsType(e, ty) => Expr::IsType(
                Box::new(Self::rewrite_cell_identifiers(e, cell_name)),
                ty.clone(),
            ),
            Expr::Within(body, fallback) => Expr::Within(
                Box::new(Self::rewrite_cell_identifiers(body, cell_name)),
                Box::new(Self::rewrite_cell_identifiers(fallback, cell_name)),
            ),
            Expr::DerivationBlock(db) => Self::rewrite_derivation(db, cell_name),
            Expr::Deref(inner) => {
                Expr::Deref(Box::new(Self::rewrite_cell_identifiers(inner, cell_name)))
            }
            Expr::AddrOf(inner) => {
                Expr::AddrOf(Box::new(Self::rewrite_cell_identifiers(inner, cell_name)))
            }
            Expr::Consume(inner) => {
                Expr::Consume(Box::new(Self::rewrite_cell_identifiers(inner, cell_name)))
            }
            Expr::Await(inner) => {
                Expr::Await(Box::new(Self::rewrite_cell_identifiers(inner, cell_name)))
            }
            Expr::PluginIntercept { name, args, .. } => Expr::PluginIntercept {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| Self::rewrite_cell_identifiers(a, cell_name))
                    .collect(),
                type_args: vec![],
                receiver: None,
                chain_refs: vec![],
            },
            Expr::Exists(name) => { panic!("compile-time existence check '{}' reached LLVM codegen", name) },
            Expr::Slice { array, start, end, stride } => {
                let new_array = Self::rewrite_cell_identifiers(array, cell_name);
                let new_start = start.as_ref().map(|e| Box::new(Self::rewrite_cell_identifiers(e, cell_name)));
                let new_end = end.as_ref().map(|e| Box::new(Self::rewrite_cell_identifiers(e, cell_name)));
                let new_stride = stride.as_ref().map(|e| Box::new(Self::rewrite_cell_identifiers(e, cell_name)));
                Expr::Slice { array: Box::new(new_array), start: new_start, end: new_end, stride: new_stride }
            }
            Expr::Range { start, end, inclusive } => {
                let new_start = Box::new(Self::rewrite_cell_identifiers(start, cell_name));
                let new_end = Box::new(Self::rewrite_cell_identifiers(end, cell_name));
                Expr::Range { start: new_start, end: new_end, inclusive: *inclusive }
            }
            Expr::UnitLiteral { value, unit } => Expr::UnitLiteral { value: *value, unit: unit.clone() },
            Expr::Capture { expr, name } => Expr::Capture {
                expr: Box::new(Self::rewrite_cell_identifiers(expr, cell_name)),
                name: name.clone(),
            },

        }
    }

    /// Shared helper for Tuple/List rewrite to avoid duplicating the
    /// match-guard logic in the parent function.
    fn rewrite_tuple_or_list(expr: &Expr, items: &[Expr], cell_name: &str) -> Expr {
        let mapped: Vec<Expr> = items
            .iter()
            .map(|a| Self::rewrite_cell_identifiers(a, cell_name))
            .collect();
        if matches!(expr, Expr::Tuple(_)) {
            Expr::Tuple(mapped)
        } else {
            Expr::List(mapped)
        }
    }

    /// Rewrite identifiers inside a DerivationBlock.
    fn rewrite_derivation(db: &crate::ast::DerivationBlock, cell_name: &str) -> Expr {
        Expr::DerivationBlock(Box::new(crate::ast::DerivationBlock {
            examples: db
                .examples
                .iter()
                .map(|ex| crate::ast::DerivationExample {
                    inputs: ex
                        .inputs
                        .iter()
                        .map(|i| Self::rewrite_cell_identifiers(i, cell_name))
                        .collect(),
                    output: Box::new(Self::rewrite_cell_identifiers(&ex.output, cell_name)),
                    // 2026-07-28: Preserve tolerance field through rewriting
                    tolerance: ex.tolerance,
                    span: ex.span,
                })
                .collect(),
            synthesized: db
                .synthesized
                .as_ref()
                .map(|s| Box::new(Self::rewrite_cell_identifiers(s, cell_name))),
            postcondition: db.postcondition.clone(),
            precondition: db.precondition.clone(),
            ref_name: db.ref_name.clone(),
            ref_tolerance: db.ref_tolerance,
            chain: db.chain.clone(),
            span: db.span,
        }))
    }

    /// Rewrite all identifiers in a statement with a cell prefix.
    pub(super) fn rewrite_cell_stmt_identifiers(stmt: &Statement, cell_name: &str) -> Statement {
        match stmt {
            Statement::Yield => stmt.clone(),
            Statement::Check(_) => stmt.clone(),
            Statement::Assign(lhs, expr) => Statement::Assign(
                Self::rewrite_cell_identifiers(lhs, cell_name),
                Self::rewrite_cell_identifiers(expr, cell_name),
            ),
            Statement::ArrowAssign { target, value, consume } => Statement::ArrowAssign {
                target: target.as_ref().map(|t| Box::new(Self::rewrite_cell_identifiers(t, cell_name))),
                value: Box::new(Self::rewrite_cell_identifiers(value, cell_name)),
                consume: *consume,
            },
            Statement::FreeHint(name) => Statement::FreeHint(name.clone()),
            Statement::KeepHint(name) => Statement::KeepHint(name.clone()),
            Statement::Guarded(cond, stmts) => Statement::Guarded(
                Self::rewrite_cell_identifiers(cond, cell_name),
                Self::rewrite_cell_stmt_body(stmts, cell_name),
            ),
            Statement::Gate(cond) => Statement::Gate(Self::rewrite_cell_identifiers(cond, cell_name)),
            Statement::Trap | Statement::Halt => stmt.clone(),
            Statement::Break => Statement::Break,
            Statement::Term(e) => Statement::Term(
                e.as_ref()
                    .map(|e| Self::rewrite_cell_identifiers(e, cell_name)),
            ),
            Statement::EndProgram(e) => Statement::EndProgram(
                e.as_ref()
                    .map(|e| Self::rewrite_cell_identifiers(e, cell_name)),
            ),
            Statement::Rollback(e) => Statement::Rollback(
                e.as_ref()
                    .map(|e| Self::rewrite_cell_identifiers(e, cell_name)),
            ),
            Statement::Expression(e) => {
                Statement::Expression(Self::rewrite_cell_identifiers(e, cell_name))
            }
            Statement::Let { name, ty, expr, modifiers, .. } => Statement::Let {
                names: vec![],
                name: name.clone(),
                ty: ty.clone(),
                expr: expr
                    .as_ref()
                    .map(|e| Self::rewrite_cell_identifiers(e, cell_name)),
                modifiers: modifiers.clone(),
            },
            Statement::Block(stmts) => Statement::Block(
                Self::rewrite_cell_stmt_body(stmts, cell_name),
            ),
            Statement::SyncBlock(stmts) => Statement::SyncBlock(
                Self::rewrite_cell_stmt_body(stmts, cell_name),
            ),
            Statement::Defer(stmts) => Statement::Defer(
                Self::rewrite_cell_stmt_body(stmts, cell_name),
            ),
            Statement::Mutex(stmts) => Statement::Mutex(
                Self::rewrite_cell_stmt_body(stmts, cell_name),
            ),
            Statement::Barrier { groups, body } => Statement::Barrier {
                groups: groups.clone(),
                body: Self::rewrite_cell_stmt_body(body, cell_name),
            },
            Statement::InlineAsm { .. } => stmt.clone(),
            Statement::TrgBinding { name, instance } => Statement::TrgBinding {
                name: name.clone(),
                instance: Self::rewrite_cell_identifiers(instance, cell_name),
            },
            Statement::Foreach { item, list, body } => Statement::Foreach {
                item: item.clone(),
                list: Box::new(Self::rewrite_cell_identifiers(list, cell_name)),
                body: Self::rewrite_cell_stmt_body(body, cell_name),
            },
            Statement::MetadataAssignment(..) | Statement::InlineDefn(_) | Statement::InlineTxn(_) | Statement::Match { .. } => stmt.clone(),
            // 2026-09-22 (D16 p3b): `open` — rewrite identifiers in its
            // expressions, else keep intact.
            Statement::Open(lhs, rhs) => Statement::Open(
                Box::new(Self::rewrite_cell_identifiers(lhs, cell_name)),
                Box::new(Self::rewrite_cell_identifiers(rhs, cell_name)),
            ),
        }
    }

    /// Map `rewrite_cell_stmt_identifiers` over a statement slice — the
    /// recursive-body arm shared by guarded/block/sync/defer/mutex/barrier/
    /// foreach. Extracted to keep the caller under the function-length gate.
    fn rewrite_cell_stmt_body(stmts: &[Statement], cell_name: &str) -> Vec<Statement> {
        stmts
            .iter()
            .map(|s| Self::rewrite_cell_stmt_identifiers(s, cell_name))
            .collect()
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 2: Metadata & Structure
    // ═══════════════════════════════════════════════════════════════

    /// Extract all output variable names from an `OutputType` tree.
    /// Recursively collects names from Named, Tuple, and Union wrappers.
    pub(super) fn extract_output_names_llvm(ot: &Option<OutputType>) -> Vec<String> {
        let Some(ot) = ot else {
            return Vec::new();
        };
        match ot {
            OutputType::Named(name, inner) => {
                let mut names = vec![name.clone()];
                names.extend(Self::extract_output_names_llvm(&Some(
                    inner.as_ref().clone(),
                )));
                names
            }
            OutputType::Tuple(types) | OutputType::Union(types) => types
                .iter()
                .flat_map(|t| Self::extract_output_names_llvm(&Some(t.clone())))
                .collect(),
            OutputType::Single(_) | OutputType::Array(_) => Vec::new(),
        }
    }

    /// Emit the `main` function header with CLI argv capture.
    ///
    /// 2026-08-01 (Phase 3): every loop-engine main is emitted as
    /// `define i32 @main(i32 %argc, ptr %argv)` and stores argc/argv into the
    /// module globals `@__briev_argc` / `@__briev_argv`, which the runtime
    /// argv helpers (briev_rt.c) read. The `entry:` label is emitted here
    /// (with the captures inside it) so callers do not write it again.
    /// `capture` is false for the precomputed main (pure-compile-time fold,
    /// no runtime argv needed — it still takes argv params for a uniform
    /// signature so the runtime can link).
    pub(crate) fn emit_main_header(
        &mut self,
        out: &mut String,
        attrs: &str,
        capture: bool,
    ) {
        // 2026-09-07 (init-block phi predecessor fix): a fresh function starts
        // with NO current block — the previous function's cur_block (a txn's
        // guard.end_N or a match's .match_end_N) is a label in that function,
        // not this one. Citing it as a phi predecessor here would name a
        // cross-function block (hash_ops_idio: %guard.end232 in txn_work cited
        // from main's phi). Reset so init_pred resolves to "entry" unless a
        // block-emitting init runs IN THIS function.
        self.fun.cur_block = None;
        // 2026-09-13 (defn-liveness emission, embedded main): on bare-metal
        // targets the argv capture is dead — nothing reads __briev_argc/
        // __briev_argv, and without LTO the surviving stores emit HI20
        // relocations against the 0x80000000-resident globals, which fail
        // the signed-range check at link time. Embedded main takes the
        // uniform signature (QEMU/_start pass junk in a0/a1) but captures
        // nothing.
        let capture = capture && !self.ctx.is_embedded;
        writeln!(
            out,
            "define i32 @main(i32 %argc, ptr %argv) local_unnamed_addr {} {{",
            attrs
        )
        .ok();
        writeln!(out, "entry:").ok();
        if capture {
            writeln!(out, "  store i32 %argc, ptr @__briev_argc").ok();
            writeln!(out, "  store ptr %argv, ptr @__briev_argv").ok();
            // 2026-09-10 (Family F): captured environ — hosted capture. The
            // kernel lays out envp right after argv's NULL terminator:
            // envp = &argv[argc + 1]. (The freestanding _start captures the
            // same pointer from the raw stack; both paths fill the same
            // global, so the getenv adapters work on every target.)
            writeln!(out, "  %argc64 = sext i32 %argc to i64").ok();
            writeln!(out, "  %envp_slot = getelementptr ptr, ptr %argv, i64 1").ok();
            writeln!(out, "  %envp_slot2 = getelementptr ptr, ptr %envp_slot, i64 %argc64").ok();
            // environ = the ADDRESS of the envp[0] slot (the array base),
            // NOT the first entry loaded from it.
            writeln!(out, "  store ptr %envp_slot2, ptr @__briev_environ").ok();
        }
    }

    /// 2026-09-06 (ISR plan): the program's state base — an alloca in the
    /// ordinary case, a zero-GEP alias of the GLOBAL `@__briev_state` when
    /// the program declares ISR handlers. ISR bodies run on the hardware
    /// stack, outside main's frame — they can only share state through a
    /// global. The alias keeps every downstream `%state` reference
    /// unchanged (zero-GEP of a global is a valid ptr with the same
    /// provenance; SROA sees an address-taken local either way).
    pub(crate) fn emit_state_base(&mut self, out: &mut String) {
        if self.ctx.state_is_global {
            writeln!(out, "  %state = getelementptr %State, ptr @__briev_state, i64 0").ok();
        } else {
            writeln!(out, "  %state = alloca %State, align 8").ok();
        }
    }

    /// Emit a `main()` that stores final precomputed values and returns.
    /// EmitPureCounterFold: no runtime loop, no iteration. The region analyzer simulated
    /// all transactions within `--optimize-budget` and produced final values.
    /// This is the most extreme optimization: zero runtime memory traffic.
    pub(crate) fn emit_precomputed_main(
        &mut self,
        out: &mut String,
        final_values: &[(Vec<String>, HashMap<String, i64>)],
    ) {
        self.emit_main_header(out, "#0", false);
        self.emit_state_base(out);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (_txn_id, bindings) in final_values {
            for (var, val) in bindings {
                if !seen.insert(var) {
                    continue;
                }
                if let Some(&idx) = self.ctx.field_index_map.get(var) {
                    let ty = self.ctx.field_types[idx].clone();
                    let gp = self.emit_state_gep(out, "  ", "gp", "%state", idx);
                    Self::emit_precomputed_store(out, &gp, &ty, val);
                } else if let Some(&addr) = self.ctx.mmio_fields.get(var) {
                    let gp = format!("%gp_{}", var);
                    self.emit_inttoptr(out, "  ", &gp, &addr.to_string());
                    writeln!(
                        out,
                        "  store volatile i64 {}, ptr %gp_{}, align 1",
                        val, var
                    )
                    .ok();
                }
            }
        }
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    /// Emit a single store for a precomputed field value.
    fn emit_precomputed_store(out: &mut String, gp: &str, ty: &str, val: &i64) {
        match ty.as_ref() {
            "float" => {
                let bits = *val as i32 as u32;
                writeln!(
                    out,
                    "  store float bitcast (i32 {} to float), ptr {}, align 4",
                    bits, gp
                )
                .ok();
            }
            "i8" => {
                writeln!(out, "  store i8 {}, ptr {}, align 1", val, gp).ok();
            }
            _ => {
                writeln!(out, "  store i64 {}, ptr {}, align 8", val, gp).ok();
            }
        }
    }

    /// Emit LLVM wake trigger metadata.
    /// 2026-07-13: Currently emits empty metadata (wake_triggers pending
    /// implementation in the reactive scheduler).
    pub(crate) fn emit_wake_metadata(&self, out: &mut String) {
        let wake_symbols: Vec<&str> = Vec::new();
        if wake_symbols.is_empty() {
            return;
        }
        let count = wake_symbols.len();
        let sym_list = wake_symbols
            .iter()
            .map(|s| format!("ptr @{}", s))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "@llvm.wake_triggers = constant [{} x ptr] [{}]",
            count, sym_list
        )
        .ok();
        writeln!(out, "!llvm.wake_triggers = !{{!6}}").ok();
        write!(out, "!6 = !{{").ok();
        for (i, sym) in wake_symbols.iter().enumerate() {
            if i > 0 {
                write!(out, ", ").ok();
            }
            write!(out, "!\"{}\"", sym).ok();
        }
        writeln!(out, "}}").ok();
    }

    /// Emit LLVM thread pool metadata for async transactions.
    /// Generates a constant array of function pointers consumed by
    /// `briev_thread_pool_init` at startup.
    /// 2026-09-10 (Family H): the pthread pool is GONE — the async phase
    /// emits direct sequential body calls (deterministic order), so the
    /// @llvm.thread_pool / @thread_pool_fns globals are dead. Kept as a
    /// no-op to preserve the call-site shape.
    pub(crate) fn emit_thread_pool_metadata(&self, _out: &mut String) {
        if !self.has_async_txns || self.is_lightweight_async {
            return;
        }
    }

    /// Emit the async phase calls in main: set state for workers, release
    /// workers, wait for workers.
    ///
    /// 2026-07-01: `reactor_tick` is now a no-op when the thread pool is
    /// active. Worker threads execute async bodies on the correct state
    /// snapshot (set via `__set_async_state__`), synchronized by barriers.
    pub(crate) fn emit_async_phase(&self, out: &mut String, state_var: &str) {
        if !self.has_async_txns || self.is_lightweight_async {
            return;
        }
        // 2026-09-10 (Family H): the pthread pool is replaced by DIRECT
        // sequential body calls — deterministic order, single-threaded, no
        // libc. The bodies ran to completion per tick under the pool too
        // (no cross-tick worker state), so the tick contract is unchanged;
        // only the racy stdout interleaving becomes deterministic. Multicore
        // parallelism is the documented clone+futex follow-on.
        for name in &self.async_txn_names {
            writeln!(out, "  call void @async_body_{}(ptr noalias nocapture {})", name, state_var).ok();
        }
        writeln!(
            out,
            "  call void @reactor_tick(ptr noalias nocapture {})",
            state_var
        )
        .ok();
    }

    /// Detect pairs of reactive transactions that can be fused.
    /// Fusion requires: both reactive, non-async, non-overlapping writes,
    /// no trigger references in the second's precondition.
    pub(crate) fn resolve_fusable_pairs(
        &self,
        txns: &[(String, &crate::ast::Transaction)],
    ) -> Vec<(String, String)> {
        let items: Vec<crate::ast::TopLevel> = txns
            .iter()
            .map(|(_, t)| crate::ast::TopLevel::Transaction((*t).clone()))
            .collect();
        let mut pairs = crate::backend::detect_fusable_pairs(&items);
        pairs.retain(|(a, b)| {
            let Some((_, ta)) = txns.iter().find(|(n, _)| n == a) else {
                return false;
            };
            let Some((_, tb)) = txns.iter().find(|(n, _)| n == b) else {
                return false;
            };
            if ta.is_async || tb.is_async {
                return false;
            }
            if !ta.is_reactive || !tb.is_reactive {
                return false;
            }
            let aw = crate::backend::collect_assigned_identifiers(&ta.body);
            let bw = crate::backend::collect_assigned_identifiers(&tb.body);
            // 2026-07-30: Write-write AND read-write conflicts block fusion.
            // Fusion would recreate a composite node — if A writes a field B
            // reads (or vice versa), fusing them puts the read and write in
            // the same loop body, reintroducing the interleaving the flat-node
            // decomposition removes. Per Briev's reactor design, writing is a
            // XOR condition; a shared read-write dependency means the nodes
            // must stay sequential.
            let ar = crate::backend::collect_read_identifiers(&ta.body);
            let br = crate::backend::collect_read_identifiers(&tb.body);
            if aw.iter().any(|w| bw.contains(w)) {
                return false;
            }
            if aw.iter().any(|w| br.contains(w)) {
                return false;
            }
            if bw.iter().any(|w| ar.contains(w)) {
                return false;
            }
            if self.trg_in_pre(&tb.contract.pre_condition) {
                return false;
            }
            true
        });
        pairs
    }

    /// Check if any trigger name appears in a precondition expression.
    pub(crate) fn trg_in_pre(&self, pre: &Expr) -> bool {
        let mut ids = std::collections::HashSet::new();
        crate::backend::collect_expr_identifiers(pre, &mut ids);
        ids.iter().any(|id| self.ctx.trigger_names.contains(id))
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 3: Cast & Type Conversion
    // ═══════════════════════════════════════════════════════════════

    /// Convert an i64 boxed float to a native float register.
    /// Checks the `reg_float_cache` first to avoid duplicate bitcast chains.
    /// Truncate an i64 (Int) register to i1 (Bool) for branch conditions.
    /// Non-Int types pass through as-is (already bool-width).
    pub(super) fn as_bool_reg(
        &mut self,
        out: &mut String,
        indent: &str,
        reg: &TypedRegister,
    ) -> String {
        // 2026-07-26: Protocol-driven dispatch — no name matching.
        // Int values are i64 — trunc to i1.
        if self.is_protocol_member(&reg.ty, "Int") {
            let t = self.next_reg_with_prefix("tb");
            writeln!(out, "{}{} = trunc i64 {} to i1", indent, t, reg.name).ok();
            t
        } else if self.is_protocol_member(&reg.ty, "Bool") {
            // 2026-07-14: Bool is i8 — trunc to i1 for br
            let t = self.next_reg_with_prefix("tb");
            writeln!(out, "{}{} = trunc i8 {} to i1", indent, t, reg.name).ok();
            t
        } else {
            reg.name.clone()
        }
    }

    /// Convert a String/Data typed register to i64 for C ABI calls.
    /// Int/Bool/Char/Float registers pass through as-is.
    fn ptrtoint_if_string(
        &mut self,
        out: &mut String,
        indent: &str,
        reg: &TypedRegister,
    ) -> String {
        if self.is_protocol_member(&reg.ty, "String")
            || self.is_protocol_member(&reg.ty, "Blob")
        {
            let p = self.next_reg_with_prefix("ptri");
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, p, reg.name).ok();
            p
        } else {
            reg.name.clone()
        }
    }

    /// Allocate a register name with a counter-based prefix.
    /// Convenience wrapper around `next_reg_with_prefix` from the function context.
    fn next_reg_with_prefix(&mut self, prefix: &str) -> String {
        self.fun.next_reg_with_prefix(prefix)
    }

    /// Check if a type is `Ptr<T>` or a layout-constrained pointer.
    fn is_ptr_ty(ty: &Type) -> bool {
        if let Type::Applied(name, _) = ty {
            name == "Ptr"
        } else {
            matches!(ty, Type::LayoutPtr(_))
        }
    }

    /// Look up the LLVM codegen type for a Briev type.
    /// 2026-07-19: Uses stamped `llvm_type` from universe (set by normalizer's
    /// category inference). For float types, this returns the actual LLVM type
    /// (float, double, bfloat, half) instead of guessing from byte width.
    /// Falls back to the previous byte-width heuristic if no universe.
    fn operator_llvm_type(&self, ty: &Type) -> String {
        // 2026-07-19: Read stamped llvm_type from universe first.
        // The normalizer sets this for ALL types via category inference + config.
        if let Some(ref universe) = self.ctx.type_universe {
            if let Some(rt) = ty.universe_key().and_then(|k| universe.get(k)) {
                if let Some(crate::ast::PropertyValue::String(s)) = rt.properties.get("llvm_type") {
                    return s.clone();
                }
                // Fallback: use protocol membership + byte-width for types that
                // bypassed the normalizer.
                // 2026-07-31: Phase 3 (§8.4-D10) — float detection via
                // is_protocol_member(ty, "Float") instead of the legacy `alu`
                // property string match.
                let is_float = self.is_protocol_member(ty, "Float");
                if is_float && rt.bytes <= 4 {
                    return "float".to_string();
                }
                if is_float {
                    return "double".to_string();
                }
                return "i64".to_string();
            }
        }
        // No universe or type not found: hardcoded fallback
        if ty == &Type::float() {
            "float".to_string()
        } else if ty == &Type::float64() {
            "double".to_string()
        } else {
            "i64".to_string()
        }
    }

    /// Check if an expression is a reference to a linked String-like trigger.
    fn is_linked_string_trigger(&self, expr: &Expr) -> bool {
        let Expr::Identifier(name) = expr else {
            return false;
        };
        let Some(trg) = self.ctx.triggers.get(name) else {
            return false;
        };
        // 2026-08-01 (B4): is_string_like (the 2-field structural heuristic)
        // retired — protocol membership only. A trigger whose type is a
        // String or Blob member carries a pointer-typed payload.
        self.is_protocol_member(&trg.ty, "String")
            || self.is_protocol_member(&trg.ty, "Blob")
    }

    /// Emit a cached projection: load valid flag, branch on hit/miss.
    /// Hit: load cached value. Miss: compute, store in cache, set flag.
    /// Phi merges hit/miss paths. Cache slots are appended to %State by
    /// dead-field elimination (apply_field_modes).
    pub(crate) fn try_cached_projection(
        &mut self,
        out: &mut String,
        source_expr: &Expr,
        src_val: &TypedRegister,
        target_name: &str,
        indent: &str,
    ) -> Option<TypedRegister> {
        let field_name = match source_expr {
            Expr::Identifier(n) => n.clone(),
            _ => return None,
        };
        let &(cache_idx, valid_idx) = self
            .ctx
            .cache_slots
            .get(&field_name)
            .and_then(|targets| targets.get(target_name))?;

        let v = self.next_reg_with_prefix("t");
        let valid_gep = self.next_reg_with_prefix("cvp");
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, valid_gep, valid_idx
        )
        .ok();
        let valid_load = self.next_reg_with_prefix("cvv");
        writeln!(
            out,
            "{}{} = load i8, ptr {}, align 1",
            indent, valid_load, valid_gep
        )
        .ok();
        let valid_cond = self.next_reg_with_prefix("cvc");
        writeln!(
            out,
            "{}{} = icmp ne i8 {}, 0",
            indent, valid_cond, valid_load
        )
        .ok();

        let hit_label = format!(".chit{}", self.fun.txn_counter);
        let miss_label = format!(".cmiss{}", self.fun.txn_counter);
        let merge_label = format!(".cmerge{}", self.fun.txn_counter);
        self.fun.txn_counter += 1;
        writeln!(
            out,
            "{}br i1 {}, label %{}, label %{}",
            indent, valid_cond, hit_label, miss_label
        )
        .ok();
        writeln!(out, "{}:", hit_label).ok();
        let cache_gep = self.next_reg_with_prefix("cve");
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, cache_gep, cache_idx
        )
        .ok();
        let cache_val = self.next_reg_with_prefix("cvv");
        writeln!(
            out,
            "{}{} = load i64, ptr {}, align 8, !tbaa !1",
            indent, cache_val, cache_gep
        )
        .ok();
        writeln!(out, "{}br label %{}", indent, merge_label).ok();
        writeln!(out, "{}:", miss_label).ok();
        writeln!(out, "{}{} = add i64 0, {}", indent, v, src_val.name).ok();
        let store_gep = self.next_reg_with_prefix("cse");
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, store_gep, cache_idx
        )
        .ok();
        writeln!(
            out,
            "{}store i64 {}, ptr {}, align 8, !tbaa !1",
            indent, v, store_gep
        )
        .ok();
        let valid_store_gep = self.next_reg_with_prefix("csve");
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, valid_store_gep, valid_idx
        )
        .ok();
        writeln!(
            out,
            "{}store i8 1, ptr {}, align 1",
            indent, valid_store_gep
        )
        .ok();
        writeln!(out, "{}br label %{}", indent, merge_label).ok();
        writeln!(out, "{}:", merge_label).ok();
        let phi_reg = self.next_reg_with_prefix("cp");
        writeln!(
            out,
            "{}{} = phi i64 [ {}, %{} ], [ {}, %{} ]",
            indent, phi_reg, cache_val, hit_label, v, miss_label
        )
        .ok();
        Some(TypedRegister {
            name: phi_reg,
            ty: Type::int(),
        })
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 4: String Operations
    // ═══════════════════════════════════════════════════════════════

    /// Emit inline string concatenation: malloc + header setup + memcpy.
    /// Both operands are i8* (Briev header pointers). Returns i64-tagged.
    ///
    /// Tag convention (2026-06-19):
    ///   bit 0 = static string constant (don't free, don't read header at -16)
    ///   bit 1 = temporary concat result (safe to free when consumed)
    ///   State-loaded strings have both bits clear (heap, state-owned).
    ///
    /// Why inline instead of sprintf/strcat: the compiler knows each
    /// operand's length at emit time (from header slot 1), so it computes
    /// the total allocation and emits memcpy calls that LLVM lowers to
    /// `rep movsb` or inline. sprintf scans for null terminators at runtime,
    /// losing length information.
    pub(crate) fn emit_inline_concat(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
    ) -> TypedRegister {
        // 2026-08-01 (B4): the SSO concat path was retired — a String is a
        // ptr to [len][bytes] under the bits model.
        let a_boxed = self.adapt_to_i64(out, indent, a);
        let b_boxed = self.adapt_to_i64(out, indent, b);
        let a_clean = self.emit_mask_tag(out, indent, &a_boxed, "cam");
        let b_clean = self.emit_mask_tag(out, indent, &b_boxed, "cbm");
        let ha = self.emit_inttoptr_reg(out, indent, "cha", &a_clean);
        let la = self.emit_load_length(out, indent, &ha, "clp", "cla");
        let hb = self.emit_inttoptr_reg(out, indent, "chb", &b_clean);
        let lb = self.emit_load_length(out, indent, &hb, "clq", "clb");
        let total = self.emit_add_const(out, indent, &la, &lb, "ctl");
        let alloc_size = self.compute_alloc_size(out, indent, &total, "chs", "cas");
        let result_i64 = self.emit_arena_alloc(out, indent, &alloc_size);
        // 2026-07-19: emit_arena_alloc returns i64 — inttoptr to ptr for helpers.
        let result_ptr = self.fun.next_reg_with_prefix("crp");
        writeln!(
            out,
            "{}{} = inttoptr i64 {} to ptr",
            indent, result_ptr, result_i64
        )
        .ok();
        self.emit_write_header(out, indent, &result_ptr, &total, "chp", "cls");
        // First data copy: dest = result + 8 (after length prefix)
        let dest1 = self.fun.next_reg_with_prefix("cd1");
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 8", indent, dest1, result_ptr).ok();
        self.emit_copy_data(out, indent, &dest1, &ha, &la, "cac", "cds");
        // Second data copy: dest = result + 8 + la (after first string's data)
        let dest2_off = self.fun.next_reg_with_prefix("cd2");
        writeln!(out, "{}{} = add i64 8, {}", indent, dest2_off, la).ok();
        let dest2 = self.fun.next_reg_with_prefix("cdr");
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, dest2, result_ptr, dest2_off).ok();
        self.emit_copy_data(out, indent, &dest2, &hb, &lb, "cbc", "cdo");
        self.emit_null_terminate(out, indent, &result_ptr, &total, "cnt");
        self.emit_free_temporaries(out, indent, &a_boxed, &b_boxed, "cta", "cia", "ctb", "cib");
        self.emit_box_concat_result(out, indent, &result_ptr, "t")
    }

    /// Mask off tag bits (bit 0 = static, bit 1 = temp) from a boxed string.
    fn emit_mask_tag(&mut self, out: &mut String, indent: &str, val: &str, prefix: &str) -> String {
        let r = self.fun.next_reg_with_prefix(prefix);
        // 2026-08-01 (B4): the SSO 3-bit tagging was retired — a String is an
        // untagged ptr under the bits model; only the legacy 2-bit temp flag
        // remains (bit 1 = temporary concat result).
        let mask = -4i64;
        writeln!(out, "{}{} = and i64 {}, {}", indent, r, val, mask).ok();
        r
    }

    /// Ptrtoint for a string register: load data pointer.
    fn emit_inttoptr_reg(
        &mut self,
        out: &mut String,
        indent: &str,
        prefix: &str,
        val: &str,
    ) -> String {
        let r = self.fun.next_reg_with_prefix(prefix);
        self.emit_inttoptr(out, indent, &r, &val);
        r
    }

    /// Load the length from a string header's slot at index 1.
    fn emit_load_length(
        &mut self,
        out: &mut String,
        indent: &str,
        header_ptr: &str,
        gep_prefix: &str,
        load_prefix: &str,
    ) -> String {
        let lp = self.fun.next_reg_with_prefix(gep_prefix);
        writeln!(
            out,
            "{}{} = getelementptr i64, ptr {}, i64 0",
            indent, lp, header_ptr
        )
        .ok();
        let l = self.fun.next_reg_with_prefix(load_prefix);
        writeln!(out, "{}{} = load i64, ptr {}, align 8", indent, l, lp).ok();
        l
    }

    /// Emit `add i64 a, b`.
    fn emit_add_const(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &str,
        b: &str,
        prefix: &str,
    ) -> String {
        let r = self.fun.next_reg_with_prefix(prefix);
        writeln!(out, "{}{} = add i64 {}, {}", indent, r, a, b).ok();
        r
    }

    /// Compute total allocation size: 8 (length prefix) + total_chars + 1 (null).
    fn compute_alloc_size(
        &mut self,
        out: &mut String,
        indent: &str,
        total_chars: &str,
        header_prefix: &str,
        alloc_prefix: &str,
    ) -> String {
        let hs = self.fun.next_reg_with_prefix(header_prefix);
        writeln!(out, "{}{} = add i64 8, {}", indent, hs, total_chars).ok();
        let as_ = self.fun.next_reg_with_prefix(alloc_prefix);
        writeln!(out, "{}{} = add i64 {}, 1", indent, as_, hs).ok();
        as_
    }

    /// Write the length + data to a new string allocation.
    /// Format: [i64 length][chars\0] — same as C heap string format.
    fn emit_write_header(
        &mut self,
        out: &mut String,
        indent: &str,
        result: &str,
        total: &str,
        hp_prefix: &str,
        len_prefix: &str,
    ) {
        let hp = self.fun.next_reg_with_prefix(hp_prefix);
        writeln!(out, "{}{} = bitcast ptr {} to ptr", indent, hp, result).ok();
        let ls = self.fun.next_reg_with_prefix(len_prefix);
        writeln!(
            out,
            "{}{} = getelementptr i64, ptr {}, i64 0",
            indent, ls, hp
        )
        .ok();
        writeln!(out, "{}store i64 {}, ptr {}, align 8", indent, total, ls).ok();
    }

    /// Copy string data from one allocation to another via memcpy.
    /// dest_ptr is the pre-computed destination pointer (e.g., result+8, result+8+la).
    fn emit_copy_data(
        &mut self,
        out: &mut String,
        indent: &str,
        dest_ptr: &str,
        header: &str,
        length: &str,
        src_prefix: &str,
        dest_prefix: &str,
    ) {
        // Source data is at header + 8 (skip length prefix)
        let src = self.fun.next_reg_with_prefix(src_prefix);
        writeln!(
            out,
            "{}{} = getelementptr i8, ptr {}, i64 8",
            indent, src, header
        )
        .ok();
        // Destination is the provided dest_ptr (caller computes the offset)
        let dest = self.fun.next_reg_with_prefix(dest_prefix);
        writeln!(
            out,
            "{}{} = bitcast ptr {} to ptr",
            indent, dest, dest_ptr
        )
        .ok();
        writeln!(
            out,
            "{}call void @llvm.memcpy.p0i8.p0i8.i64(i8* {}, ptr {}, i64 {}, i1 false)",
            indent, dest, src, length
        )
        .ok();
    }

    /// Write a null terminator at the end of the string data.
    fn emit_null_terminate(
        &mut self,
        out: &mut String,
        indent: &str,
        result: &str,
        total: &str,
        prefix: &str,
    ) {
        let nt = self.fun.next_reg_with_prefix(prefix);
        let nt_off = self.fun.next_reg_with_prefix("no");
        writeln!(out, "{}{} = add i64 8, {}", indent, nt_off, total).ok();
        writeln!(
            out,
            "{}{} = getelementptr i8, ptr {}, i64 {}",
            indent, nt, result, nt_off
        )
        .ok();
        writeln!(out, "{}store i8 0, ptr {}, align 1", indent, nt).ok();
    }

    /// Free heap-allocated temporaries (bit 1 set) when arena is not active.
    /// Static constants (bit 0=1) and state fields (bit 0=0,bit 1=0) preserved.
    fn emit_free_temporaries(
        &mut self,
        out: &mut String,
        indent: &str,
        a_boxed: &str,
        b_boxed: &str,
        tag_a_prefix: &str,
        is_a_prefix: &str,
        tag_b_prefix: &str,
        is_b_prefix: &str,
    ) {
        if self.fun.arena_slots.is_some() {
            return;
        }
        self.emit_free_one_temp(
            out,
            indent,
            a_boxed,
            tag_a_prefix,
            is_a_prefix,
            "free_a",
            "af_a",
        );
        self.emit_free_one_temp(
            out,
            indent,
            b_boxed,
            tag_b_prefix,
            is_b_prefix,
            "free_b",
            "af_b",
        );
    }

    /// Check tag bit 1 (legacy) or bit 2 (SSO) and conditionally free a
    /// temporary string allocation.
    fn emit_free_one_temp(
        &mut self,
        out: &mut String,
        indent: &str,
        boxed: &str,
        tag_prefix: &str,
        is_prefix: &str,
        free_label: &str,
        after_label: &str,
    ) {
        let tag = self.fun.next_reg_with_prefix(tag_prefix);
        // 2026-08-01 (B4): the SSO bit-2 temporary flag was retired; only the
        // legacy bit-1 (value 2) temporary-concat-result flag remains.
        let temp_bit = 2i64;
        writeln!(out, "{}{} = and i64 {}, {}", indent, tag, boxed, temp_bit).ok();
        let is_temp = self.fun.next_reg_with_prefix(is_prefix);
        writeln!(out, "{}{} = icmp ne i64 {}, 0", indent, is_temp, tag).ok();
        let fl = format!("{}_{}", free_label, self.fun.txn_counter);
        let afl = format!("{}_{}", after_label, self.fun.txn_counter);
        self.fun.txn_counter += 1;
        writeln!(
            out,
            "{}br i1 {}, label %{}, label %{}",
            indent, is_temp, fl, afl
        )
        .ok();
        writeln!(out, "{}{}:", indent, fl).ok();
        let clean = self.emit_mask_tag(out, indent, boxed, &format!("cc{}", tag_prefix));
        let free_ptr = self.emit_inttoptr_reg(out, indent, &format!("cf{}", tag_prefix), &clean);
        // 2026-09-10 (Family F): when the arena-aware __briev_free defn
        // exists (cast_lanes), route the temp free through it — a no-op that
        // keeps libc out of the symbol table for freestanding programs (the
        // concat epilogue was the last @free call site). The tag-bit flag
        // remains the gate: arena pointers are never tagged temp, so this
        // branch is dead at runtime either way.
        if self.ctx.defn_params.contains_key("__briev_free") {
            writeln!(out, "{}call i64 @__briev_free(ptr %state, ptr {})", indent, free_ptr).ok();
        } else {
            writeln!(out, "{}call void @free(ptr {})", indent, free_ptr).ok();
        }
        writeln!(out, "{}br label %{}", indent, afl).ok();
        writeln!(out, "{}{}:", indent, afl).ok();
    }

    /// Box a concat result pointer to i64 with temporary tag (bit 1 set).
    fn emit_box_concat_result(
        &mut self,
        out: &mut String,
        indent: &str,
        result: &str,
        prefix: &str,
    ) -> TypedRegister {
        // 2026-08-01 (B4): a String value is an UNTAGGED ptr to [len][bytes]
        // under the bits model — the concat result is the allocated buffer ptr
        // itself. The old OR 2 (temp-bit tag) boxing returned an i64, which
        // broke consumers expecting a ptr (`__print_str(ptr)`).
        let v = self.fun.next_reg_with_prefix(prefix);
        writeln!(out, "{}{} = bitcast ptr {} to ptr", indent, v, result).ok();
        TypedRegister {
            name: v,
            ty: Type::string(),
        }
    }

    /// Check if an identifier resolves to String/Data type via let bindings
    /// or struct field type hints.
    /// 2026-08-01 (B4): is_string_like (2-field structural heuristic) retired
    /// — protocol membership only.
    fn is_string_identifier(&self, name: &str) -> bool {
        let is_like = |t: &Type| -> bool {
            let is_data = self.ctx.type_universe.as_ref()
                .and_then(|u| t.universe_key().and_then(|k| u.get(k)))
                .map(|rt| rt.properties.contains_key("Cast.Blob"))
                .unwrap_or(false);
            self.is_protocol_member(t, "String")
                || is_data
        };
        if self
            .fun
            .let_binding_types
            .get(name)
            .map_or(false, |t| is_like(t))
        {
            return true;
        }
        if self
            .fun
            .let_original_types
            .get(name)
            .map_or(false, |t| is_like(t))
        {
            return true;
        }
        self.ctx
            .field_index_map
            .get(name)
            .and_then(|&idx| self.ctx.field_types.get(idx))
            .map(|ft| ft == "i8*" || ft == "ptr")
            .unwrap_or(false)
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 5: Binary Operations
    // ═══════════════════════════════════════════════════════════════

    /// Emit LLVM IR for a binary operation between two expressions.
    /// Handles constant folding, native float ops, mixed float/int,
    /// fixed-width integer ops, and a generic i64 fallback.
    ///
    /// 2026-07-13: Phase 7B (custom operator dispatch) removed — the
    /// `phase7b_l`/`phase7b_r` variables were always `None` during the
    /// rewrite period. Operators are resolved via the projection system
    /// before reaching this function.
    pub(crate) fn emit_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &Expr,
        r: &Expr,
        int_op: &str,
        float_op: &str,
    ) -> TypedRegister {
        // Constant-fold integer binops at compile time
        let int_op_clean = int_op.strip_suffix(" nsw").unwrap_or(int_op);
        if let Some(folded) = self.try_fold_binop_constants(out, indent, l, r, int_op_clean) {
            return folded;
        }
        let a = self.emit_expr(out, l, indent);
        let b = self.emit_expr(out, r, indent);
        let a_is_native = self.is_native_float(&a.ty);
        let b_is_native = self.is_native_float(&b.ty);
        let dedup_key = self.build_dedup_key(
            Self::dedup_op(a_is_native, b_is_native, int_op, float_op),
            &a,
            &b,
        );
        if let Some(cached) = self.check_dedup_cache(&dedup_key) {
            let result_ty = a.ty.clone();
            return TypedRegister {
                name: cached,
                ty: result_ty,
            };
        }
        let ptr_ty = self.infer_ptr_type(&a.ty, &b.ty);
        if a_is_native && b_is_native && a.ty == b.ty {
            return self.emit_native_float_binop(out, indent, &a, &b, float_op, &dedup_key);
        }
        if a_is_native || b_is_native {
            return self.emit_mixed_binop(out, indent, &a, &b, int_op, &dedup_key, ptr_ty);
        }
        if !self.is_native_float(&a.ty) && a.ty == b.ty {
            return self.emit_fixed_width_binop(out, indent, &a, &b, int_op, &dedup_key);
        }
        self.emit_boxed_fallback_binop(out, indent, &a, &b, int_op, &dedup_key, ptr_ty)
    }

    /// Try to constant-fold a binary operation where both operands are Decimal.
    fn try_fold_binop_constants(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &Expr,
        r: &Expr,
        int_op: &str,
    ) -> Option<TypedRegister> {
        let (Expr::Decimal(li), Expr::Decimal(ri)) = (l, r) else {
            return None;
        };
        let result = match int_op {
            "add" => Some(li.wrapping_add(*ri)),
            "sub" => Some(li.wrapping_sub(*ri)),
            "mul" => Some(li.wrapping_mul(*ri)),
            "sdiv" if *ri != 0 => Some(li / ri),
            "and" => Some(li & ri),
            "or" => Some(li | ri),
            "xor" => Some(li ^ ri),
            "shl" => Some(li.wrapping_shl(*ri as u32)),
            "lshr" => Some((*li as u64).wrapping_shr(*ri as u32) as i64),
            _ => None,
        };
        let folded = result?;
        let v = self.fun.next_reg();
        writeln!(out, "{}{} = add i64 0, {}", indent, v, folded).ok();
        Some(TypedRegister {
            name: v,
            ty: Type::int(),
        })
    }

    /// Check if a type is a native float via the universe.
    /// 2026-07-19: Reads `category` property (set by normalizer's structural
    /// inference) instead of ALU. This handles all float-like types uniformly
    /// (Float, Float64, Bfloat16, FP16, user-defined float types).
    /// 2026-07-26: Check if a type implements a protocol by looking for
    /// Cast.<protocol> in its ResolvedType properties. No name matching.
    pub(super) fn is_protocol_member(&self, ty: &Type, protocol: &str) -> bool {
        // 2026-07-30: Check casting graph first — resolves protocol membership
        // from (type → protocol) via type_to_protocol. Only EXACT category match
        // qualifies as membership (is_protocol_member(Int, "Float") = false,
        // even though Int can be cast to Float — castability ≠ membership).
        if let Some(graph) = self.ctx.casting_graph.as_ref() {
            if let Some(universe) = self.ctx.type_universe.as_ref() {
                let (cat, var) = graph.type_to_protocol(universe, ty);
                let target = protocol.strip_prefix('#').unwrap_or(protocol);
                if cat == target {
                    return true;
                }
                // Variant membership: String<UTF8> is member of String if
                // UTF8 is the default variant for String.
                if !var.is_empty() && target == cat {
                    return true;
                }
            }
        }
        // Fallback: check Cast.* universe properties (primordial backward compat)
        // 2026-08-15 (fundamentals): property keys are `Cast.<Cat>` — strip the
        // `#` so a `Float` hashword matches the `Cast.Float` key.
        let prop_key = format!("Cast.{}", protocol.trim_start_matches('#'));
        self.ctx.type_universe.as_ref()
            .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
            .map(|rt| rt.properties.contains_key(&prop_key))
            .unwrap_or(false)
    }

    fn is_native_float(&self, ty: &Type) -> bool {
        self.is_protocol_member(ty, "Float")
    }

    /// Float-category width of a type — `Some(bits)` when the type is a
    /// Float member, read from its bits/maxbits universe metadata
    /// (16 → half, 32 → float, 64 → double; other widths fall through to
    /// the float spelling — the double branch is split off first). `None`
    /// for non-float types. 2026-09-02 (plan fundamental-parent-membership):
    /// the shape-driven replacement for emit_binary_op's type-name equality
    /// (rule 19) — Half, Float16, and every future float width flow the one
    /// path. Undo: restore the name matches in emit_binary_op.
    pub(super) fn float_category_bits(&self, ty: &Type) -> Option<u64> {
        if !self.is_protocol_member(ty, "Float") {
            return None;
        }
        let universe = self.ctx.type_universe.as_ref()?;
        let rt = ty.universe_key().and_then(|k| universe.get(k))?;
        rt.properties.iter().find_map(|(k, pv)| match (k.as_str(), pv) {
            ("bits", crate::ast::PropertyValue::Int(n))
            | ("maxbits", crate::ast::PropertyValue::Int(n)) => Some(*n as u64),
            _ => None,
        })
        .or(Some(rt.max_bits))
    }

    /// 2026-08-01 (B1): Central String operand check — a Briev String value
    /// is a `ptr` to a length-prefixed `[len: i64][bytes]` buffer (bits model).
    /// This is the single decision point every String op default uses (Eq/Ne
    /// content compare, band/bor/bxor/bnot content ops). Rule #16: the pattern
    /// appeared 7× inline; it lives here so changing the String representation
    /// (or adding a sub-protocol) touches one place. Undo: replace with a bare
    /// is_protocol_member call at each site if the protocol check ever differs
    /// per op.
    pub(super) fn is_string_operand(&self, ty: &Type) -> bool {
        self.is_protocol_member(ty, "String")
    }

    /// 2026-08-07 (Phase 7): is `ty` a Blob operand (the [len][bytes]
    /// byte-buffer protocol)? Resolved via the casting graph — never by type
    /// name (rules 14/18). 2026-08-15 (fundamentals): `Blob` → `Blob`.
    pub(super) fn is_blob_operand(&self, ty: &Type) -> bool {
        self.is_protocol_member(ty, "Blob")
    }

    /// 2026-08-28 (String ABI fix, obj params): the base obj name when `ty`
    /// is a registered struct/obj type (`Custom` or `Applied`) — `None` for
    /// anything else. Used to gate the boxed-obj-param recovery arm (an i64
    /// handle of obj type is a heap block, inttoptr recovers the self ptr).
    pub(super) fn boxed_obj_base(ty: &Type, struct_types: &std::collections::HashMap<String, Vec<(String, Type)>>) -> Option<String> {
        match ty {
            Type::Custom(n) | Type::Applied(n, _) if struct_types.contains_key(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// 2026-08-04 (compiler-in-Briev): is the receiver of a String operation
    /// semantically a String, even if its emitted register was boxed to an
    /// i64 handle (String param, frgn result) and is now typed Int/Custom?
    /// The physical value is still the [len][bytes] pointer. Check the reg
    /// first, then the binding's DECLARED type (a `let line: String = X`
    /// binds line→reg with let_original_types[line] = String).
    pub(super) fn is_semantic_string(&self, recv: &Expr, reg: &TypedRegister) -> bool {
        if self.is_string_operand(&reg.ty) {
            return true;
        }
        if let Expr::Identifier(name) = recv {
            if let Some(orig) = self.fun.let_original_types.get(name) {
                return self.is_string_operand(orig);
            }
            if let Some(orig) = self.fun.let_binding_types.get(name) {
                return self.is_string_operand(orig);
            }
        }
        false
    }

    /// 2026-08-04 (compiler-in-Briev): the pointer form of a string operand
    /// for a content compare. A String operand that survived unboxed (a
    /// literal's `@str.N` global) is already a `ptr`; a boxed one
    /// (adapt_to_i64 lost the String type → i64 handle) must be inttoptr'd
    /// back to the [len][bytes] pointer before `briev_str_eq`.
    pub(super) fn string_ptr(
        &mut self,
        out: &mut String,
        indent: &str,
        reg: &TypedRegister,
    ) -> String {
        if self.is_string_operand(&reg.ty) {
            return reg.name.clone();
        }
        let p = self.fun.gen_reg();
        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, reg.name).ok();
        p
    }

    /// 2026-08-07 (Phase 7): the nested LLVM array type for a Vector —
    /// `Int[2][3]` → `[2 x [3 x i64]]`. Resolves each dimension (anonymous or
    /// a compile-time constant), mirroring push_field_type. Used by the
    /// multi-dim row-view GEPs.
    pub(super) fn vector_array_llvm_type(&self, ty: &Type) -> Option<String> {
        let Type::Vector(inner, dims) = ty else {
            return None;
        };
        if dims.is_empty() {
            return None;
        }
        let mut resolved = Vec::with_capacity(dims.len());
        for d in dims {
            match d {
                crate::ast::Dimension::Anonymous(n) => resolved.push(*n),
                crate::ast::Dimension::Named(name, n) if *n > 0 => resolved.push(*n),
                crate::ast::Dimension::Named(name, _) => {
                    match self.ctx.constants.get(name) {
                        Some((_, Expr::Decimal(v))) if *v > 0 => resolved.push(*v as usize),
                        _ => return None,
                    }
                }
            }
        }
        let inner_llvm = if **inner == crate::ast::Type::float64() {
            "double".to_string()
        } else if **inner == crate::ast::Type::float() {
            "float".to_string()
        } else if matches!(inner.as_ref(), crate::ast::Type::Ptr(_) | crate::ast::Type::PtrConst(_)) {
            // Ptr state slots hold the i64 HANDLE (uniform %State), not a raw
            // ptr — a Ptr member column is `[2 x i64]`.
            "i64".to_string()
        } else {
            // Struct/other inners use their real LLVM type — a member column
            // of `{ ptr, i64 }` must be `[2 x { ptr, i64 }]`, not `[2 x i64]`.
            self.llvm_type(inner).to_string()
        };
        let mut arr_ty = inner_llvm;
        for n in resolved.iter().rev() {
            arr_ty = format!("[{} x {}]", n, arr_ty);
        }
        Some(arr_ty)
    }

    /// Choose the dedup opcode based on float vs int.
    fn dedup_op<'a>(
        a_is_native: bool,
        b_is_native: bool,
        int_op: &'a str,
        float_op: &'a str,
    ) -> &'a str {
        if a_is_native || b_is_native {
            float_op
        } else {
            int_op
        }
    }

    /// Build the dedup cache key if the opcode is long enough.
    fn build_dedup_key(
        &self,
        op: &str,
        a: &TypedRegister,
        b: &TypedRegister,
    ) -> Option<(String, String, String)> {
        if op.len() >= 3 {
            Some((op.to_string(), a.name.clone(), b.name.clone()))
        } else {
            None
        }
    }

    /// Check the expression dedup cache for a previously emitted result.
    fn check_dedup_cache(&self, key: &Option<(String, String, String)>) -> Option<String> {
        let key = key.as_ref()?;
        self.fun.expr_dedup_cache.get(key).cloned()
    }

    /// Infer pointer type preservation through arithmetic.
    fn infer_ptr_type(&self, a_ty: &Type, b_ty: &Type) -> Option<Type> {
        if Self::is_ptr_ty(a_ty) {
            Some(a_ty.clone())
        } else if Self::is_ptr_ty(b_ty) {
            Some(b_ty.clone())
        } else {
            None
        }
    }

    /// Emit a native float binary operation (fadd/fsub/fmul/fdiv).
    fn emit_native_float_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
        float_op: &str,
        dedup_key: &Option<(String, String, String)>,
    ) -> TypedRegister {
        let fa = self.ensure_float_reg(out, indent, a);
        let fb = self.ensure_float_reg(out, indent, b);
        let llvm_ty = self.operator_llvm_type(&a.ty);
        let fr = self.fun.next_reg_with_prefix("bfr");
        writeln!(
            out,
            "{}{} = {} fast {} {}, {}",
            indent, fr, float_op, llvm_ty, fa, fb
        )
        .ok();
        self.fun.reg_float_cache.insert(fr.clone(), fr.clone());
        if let Some(key) = dedup_key {
            self.fun.expr_dedup_cache.insert(key.clone(), fr.clone());
        }
        TypedRegister {
            name: fr,
            ty: a.ty.clone(),
        }
    }

    /// Emit a mixed float/int binary operation (box both to i64).
    fn emit_mixed_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
        int_op: &str,
        dedup_key: &Option<(String, String, String)>,
        ptr_ty: Option<Type>,
    ) -> TypedRegister {
        let v = self.fun.next_reg_with_prefix("t");
        let a_i64 = self.adapt_to_i64(out, indent, a);
        let b_i64 = self.adapt_to_i64(out, indent, b);
        writeln!(out, "{}{} = {} i64 {}, {}", indent, v, int_op, a_i64, b_i64).ok();
        if let Some(key) = dedup_key {
            self.fun.expr_dedup_cache.insert(key.clone(), v.clone());
        }
        TypedRegister {
            name: v,
            ty: ptr_ty.unwrap_or(Type::int()),
        }
    }

    /// Emit a fixed-width integer binary operation (native width, no boxing).
    fn emit_fixed_width_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
        int_op: &str,
        dedup_key: &Option<(String, String, String)>,
    ) -> TypedRegister {
        let v = self.fun.next_reg_with_prefix("t");
        let llvm_ty_str = self.llvm_type(&a.ty).to_string();
        writeln!(
            out,
            "{}{} = {} {} {}, {}",
            indent, v, int_op, llvm_ty_str, a.name, b.name
        )
        .ok();
        if let Some(key) = dedup_key {
            self.fun.expr_dedup_cache.insert(key.clone(), v.clone());
        }
        TypedRegister {
            name: v,
            ty: a.ty.clone(),
        }
    }

    /// Emit a generic i64 boxed binary operation fallback.
    fn emit_boxed_fallback_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
        int_op: &str,
        dedup_key: &Option<(String, String, String)>,
        ptr_ty: Option<Type>,
    ) -> TypedRegister {
        let v = self.fun.next_reg_with_prefix("t");
        let a_i64 = self.adapt_to_i64(out, indent, a);
        let b_i64 = self.adapt_to_i64(out, indent, b);
        writeln!(out, "{}{} = {} i64 {}, {}", indent, v, int_op, a_i64, b_i64).ok();
        if let Some(key) = dedup_key {
            self.fun.expr_dedup_cache.insert(key.clone(), v.clone());
        }
        TypedRegister {
            name: v,
            ty: ptr_ty.unwrap_or(Type::int()),
        }
    }

    /// Emit a resolved operator call.
    /// 2026-07-08: Phase 2D — handles Native storage (float/double) by
    /// calling ensure_float_reg on operands before emitting the opcode.
    ///
    /// The `implementation` expression is one of:
    ///   - `Identifier(name)` → call to function `name`
    ///   - `Quoted(llvm_op)` → inline LLVM instruction (e.g. "add nsw")
    ///   - Fallback → identity (no-op)
    fn emit_operator_call(
        &mut self,
        out: &mut String,
        indent: &str,
        a: &TypedRegister,
        b: &TypedRegister,
        implementation: &Expr,
    ) -> TypedRegister {
        let v = self.fun.next_reg();
        // 2026-07-31: Phase 3 (§8.4-D10) — native-float detection via protocol
        // membership (is_protocol_member(ty, "Float")) instead of the legacy
        // `alu` property string match.
        let is_native = self.is_protocol_member(&a.ty, "Float");
        let (op_a, op_b) = if is_native {
            (
                self.ensure_float_reg(out, indent, a),
                self.ensure_float_reg(out, indent, b),
            )
        } else {
            (a.name.clone(), b.name.clone())
        };
        let llvm_ty = self.operator_llvm_type(&a.ty);
        match implementation {
            Expr::Identifier(name) => {
                writeln!(
                    out,
                    "{}{} = call i64 @{}(i64 {}, i64 {})",
                    indent, v, name, op_a, op_b
                )
                .ok();
            }
            Expr::Quoted(llvm_op) => {
                let llvm_op_str = String::from_utf8_lossy(llvm_op);
                writeln!(
                    out,
                    "{}{} = {} {} {}, {}",
                    indent, v, llvm_op_str, llvm_ty, op_a, op_b
                )
                .ok();
                if is_native {
                    self.fun.reg_float_cache.insert(v.clone(), v.clone());
                }
            }
            _ => {
                writeln!(out, "{}{} = {} 0, {}", indent, v, llvm_ty, op_a).ok();
            }
        }
        TypedRegister {
            name: v,
            ty: a.ty.clone(),
        }
    }

    /// Emit LLVM IR for a comparison between two expressions.
    /// Handles constant folding, string trigger comparisons (compare first
    /// byte), and general float/int dispatch.
    pub(crate) fn emit_fcmp(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &Expr,
        r: &Expr,
        cond: &str,
    ) -> TypedRegister {
        if let Some(folded) = self.try_fold_fcmp_constants(out, indent, l, r, cond) {
            return folded;
        }
        if let Some(result) = self.try_string_trigger_cmp(out, indent, l, r, cond) {
            return result;
        }
        let (a, b) = (
            self.emit_expr(out, l, indent),
            self.emit_expr(out, r, indent),
        );
        let c = self.fun.next_reg_with_prefix("c");
        if self.is_native_float(&a.ty)
            || self.is_native_float(&b.ty)
        {
            let fa = self.ensure_float_reg(out, indent, &a);
            let fb = self.ensure_float_reg(out, indent, &b);
            writeln!(
                out,
                "{}{} = fcmp fast {} float {}, {}",
                indent, c, cond, fa, fb
            )
            .ok();
        } else {
            let icmp_cond = self.fcmp_to_icmp(cond);
            let a_i64 = self.adapt_to_i64(out, indent, &a);
            let b_i64 = self.adapt_to_i64(out, indent, &b);
            writeln!(
                out,
                "{}{} = icmp {} i64 {}, {}",
                indent, c, icmp_cond, a_i64, b_i64
            )
            .ok();
        }
        TypedRegister {
            name: c,
            ty: Type::bool_(),
        }
    }

    /// Fold constant integer comparisons at compile time.
    fn try_fold_fcmp_constants(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &Expr,
        r: &Expr,
        cond: &str,
    ) -> Option<TypedRegister> {
        let (Expr::Decimal(li), Expr::Decimal(ri)) = (l, r) else {
            return None;
        };
        let result = match cond {
            "oeq" => li == ri,
            "one" => li != ri,
            "olt" => li < ri,
            "ole" => li <= ri,
            "ogt" => li > ri,
            "oge" => li >= ri,
            _ => false,
        };
        let v = self.fun.next_reg_with_prefix("t");
        if result {
            writeln!(out, "{}{} = and i8 1, 1", indent, v).ok();
        } else {
            writeln!(out, "{}{} = xor i8 1, 1", indent, v).ok();
        }
        Some(TypedRegister {
            name: v,
            ty: Type::bool_(),
        })
    }

    /// Try to compare a string trigger's first byte against a quoted literal.
    fn try_string_trigger_cmp(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &Expr,
        r: &Expr,
        cond: &str,
    ) -> Option<TypedRegister> {
        let (trigger_expr, quoted) = if let Expr::Quoted(s) = r {
            if self.is_linked_string_trigger(l) {
                (l, s)
            } else {
                return None;
            }
        } else if let Expr::Quoted(s) = l {
            if self.is_linked_string_trigger(r) {
                (r, s)
            } else {
                return None;
            }
        } else {
            return None;
        };
        let a = self.emit_expr(out, trigger_expr, indent);
        let icmp_cond = self.fcmp_to_icmp(cond);
        let p = self.fun.next_reg_with_prefix("fp");
        self.emit_inttoptr(out, indent, &p, &a.name);
        let b = self.fun.next_reg_with_prefix("fb");
        writeln!(out, "{}{} = load i8, ptr {}, align 1", indent, b, p).ok();
        let z = self.fun.next_reg_with_prefix("fz");
        writeln!(out, "{}{} = zext i8 {} to i64", indent, z, b).ok();
        let byte_val = quoted.first().copied().unwrap_or(0u8) as i64;
        let c = self.fun.next_reg_with_prefix("fc");
        writeln!(
            out,
            "{}{} = icmp {} i64 {}, {}",
            indent, c, icmp_cond, z, byte_val
        )
        .ok();
        Some(TypedRegister {
            name: c,
            ty: Type::bool_(),
        })
    }

    /// Convert LLVM float comparison condition to integer icmp condition.
    fn fcmp_to_icmp(&self, cond: &str) -> &'static str {
        match cond {
            "oeq" => "eq",
            "one" => "ne",
            "olt" => "slt",
            "ole" => "sle",
            "ogt" => "sgt",
            "oge" => "sge",
            _ => "eq",
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 6: Projection Fast Path
    // ═══════════════════════════════════════════════════════════════

    /// Emit native LLVM IR for function metadata projections (Address, Name).
    /// Returns `Some(register)` if the target is a function name identifier.
    pub(super) fn try_emit_fn_projection(
        &mut self,
        out: &mut String,
        source: &Expr,
        _target: &str,
        indent: &str,
    ) -> Option<TypedRegister> {
        let name = match source {
            Expr::Identifier(n) => n.clone(),
            _ => return None,
        };
        let v = self.fun.next_reg_with_prefix("fnm");
        writeln!(out, "{}{} = or i64 {}, 0", indent, v, 0).ok();
        Some(TypedRegister {
            name: v,
            ty: Type::int(),
        })
    }

    /// Emit native LLVM IR for well-known projection operations
    /// (Add/Sub/Mul/Div/Eq/Ne/Lt/Le/Gt/Ge on Int/Float/Bool).
    ///
    /// Why this exists: Briev's projection system is generic (any operator
    /// on any type dispatches through UserDefinedWithArg). But for primitive
    /// types, the generic dispatch would load i64 → convert to native →
    /// exec op → convert back. This fast path skips both conversions.
    pub(super) fn try_projection_fast_path(
        &mut self,
        out: &mut String,
        src_val: &TypedRegister,
        name: &str,
        arg_expr: &Expr,
        indent: &str,
        v: &str,
    ) -> Option<TypedRegister> {
        let rhs = self.emit_expr(out, arg_expr, indent);
        // 2026-07-31: Phase 3 (§8.4) — fast-path selection via canonical
        // bootstrap types (Type::int()/float()/bool_()) instead of type-name
        // matching. Only the exact 64-bit Int / 32-bit Float / i8 Bool types
        // take the fast path: the fast-path opcodes are width-specific
        // (add i64 / icmp) and must not fire for Int8/Int32/Float64.
        if src_val.ty == Type::int() {
            self.projection_int_fast_path(out, src_val, &rhs, name, v, indent)
        } else if src_val.ty == Type::float() {
            self.projection_float_fast_path(out, src_val, &rhs, name, v, indent)
        } else if src_val.ty == Type::bool_() {
            self.projection_bool_fast_path(out, src_val, &rhs, name, v, indent)
        } else {
            None
        }
    }

    /// Fast-path projections for Int type.
    fn projection_int_fast_path(
        &mut self,
        out: &mut String,
        src: &TypedRegister,
        rhs: &TypedRegister,
        name: &str,
        v: &str,
        indent: &str,
    ) -> Option<TypedRegister> {
        let (op, is_cmp) = match name {
            "Add" => ("add", false),
            "Sub" => ("sub", false),
            "Mul" => ("mul", false),
            "Div" => ("sdiv", false),
            "Mod" => ("srem", false),
            "Eq" => ("icmp eq", true),
            "Ne" => ("icmp ne", true),
            "Lt" => ("icmp slt", true),
            "Le" => ("icmp sle", true),
            "Gt" => ("icmp sgt", true),
            "Ge" => ("icmp sge", true),
            "BitAnd" | "And" => ("and", false),
            "BitOr" | "Or" => ("or", false),
            "BitXor" => ("xor", false),
            "Shl" => ("shl", false),
            "Shr" => ("lshr", false),
            _ => return None,
        };
        if is_cmp {
            let cmp = self.fun.next_reg_with_prefix("pcmp");
            writeln!(
                out,
                "{}{} = {} i64 {}, {}",
                indent, cmp, op, src.name, rhs.name
            )
            .ok();
            writeln!(out, "{}{} = zext i1 {} to i64", indent, v, cmp).ok();
            Some(TypedRegister {
                name: v.to_string(),
                ty: Type::int(),
            })
        } else {
            writeln!(
                out,
                "{}{} = {} i64 {}, {}",
                indent, v, op, src.name, rhs.name
            )
            .ok();
            Some(TypedRegister {
                name: v.to_string(),
                ty: Type::int(),
            })
        }
    }

    /// Fast-path projections for Float type.
    fn projection_float_fast_path(
        &mut self,
        out: &mut String,
        src: &TypedRegister,
        rhs: &TypedRegister,
        name: &str,
        v: &str,
        indent: &str,
    ) -> Option<TypedRegister> {
        let (op, is_cmp) = match name {
            "Add" => ("fadd", false),
            "Sub" => ("fsub", false),
            "Mul" => ("fmul", false),
            "Div" => ("fdiv", false),
            "Eq" => ("fcmp oeq", true),
            "Ne" => ("fcmp one", true),
            "Lt" => ("fcmp olt", true),
            "Le" => ("fcmp ole", true),
            "Gt" => ("fcmp ogt", true),
            "Ge" => ("fcmp oge", true),
            _ => return None,
        };
        if is_cmp {
            let cmp = self.fun.next_reg_with_prefix("pcmp");
            writeln!(
                out,
                "{}{} = {} float {}, {}",
                indent, cmp, op, src.name, rhs.name
            )
            .ok();
            let ext = self.fun.next_reg_with_prefix("pce");
            writeln!(out, "{}{} = zext i1 {} to i64", indent, ext, cmp).ok();
            writeln!(out, "{}{} = sitofp i64 {} to float", indent, v, ext).ok();
            Some(TypedRegister {
                name: v.to_string(),
                ty: Type::float(),
            })
        } else {
            writeln!(
                out,
                "{}{} = {} float {}, {}",
                indent, v, op, src.name, rhs.name
            )
            .ok();
            Some(TypedRegister {
                name: v.to_string(),
                ty: Type::float(),
            })
        }
    }

    /// Fast-path projections for Bool type.
    fn projection_bool_fast_path(
        &mut self,
        out: &mut String,
        src: &TypedRegister,
        rhs: &TypedRegister,
        name: &str,
        v: &str,
        indent: &str,
    ) -> Option<TypedRegister> {
        match name {
            "And" => {
                writeln!(out, "{}{} = and i1 {}, {}", indent, v, src.name, rhs.name).ok();
                Some(TypedRegister {
                    name: v.to_string(),
                    ty: Type::bool_(),
                })
            }
            "Or" => {
                writeln!(out, "{}{} = or i1 {}, {}", indent, v, src.name, rhs.name).ok();
                Some(TypedRegister {
                    name: v.to_string(),
                    ty: Type::bool_(),
                })
            }
            "Eq" => {
                writeln!(
                    out,
                    "{}{} = icmp eq i1 {}, {}",
                    indent, v, src.name, rhs.name
                )
                .ok();
                Some(TypedRegister {
                    name: v.to_string(),
                    ty: Type::bool_(),
                })
            }
            "Ne" => {
                writeln!(
                    out,
                    "{}{} = icmp ne i1 {}, {}",
                    indent, v, src.name, rhs.name
                )
                .ok();
                Some(TypedRegister {
                    name: v.to_string(),
                    ty: Type::bool_(),
                })
            }
            _ => None,
        }
    }

    // Section 8: Cast Optimization (EOR)
    // ═══════════════════════════════════════════════════════════════

    /// Try to emit an EOR-optimized cast:
    /// `Cast(BinaryOp(Cast(a, T), Cast(b, T)), U)` where U -> T.
    /// If matched, emits the binary op directly without redundant casts.
    ///
    /// 2026-07-03: Cast elimination optimization — when both operands of a
    /// binary op are already the target type (cast_ty), and the outer cast
    /// target has a meld with cast_ty, emit the binary op directly in the
    /// inner types. This eliminates the inner casts entirely.
    pub(super) fn try_emit_eor(
        &mut self,
        out: &mut String,
        v: &str,
        inner: &Expr,
        target_ty: &Type,
        indent: &str,
    ) -> Option<TypedRegister> {
        let (kind, lhs, rhs) = match inner {
            Expr::BinaryOp(k, l, r) => (k, l.as_ref().clone(), r.as_ref().clone()),
            _ => return None,
        };
        let (Expr::Cast(_, lt), Expr::Cast(_, rt)) = (&lhs, &rhs) else {
            return None;
        };
        if lt != rt {
            return None;
        }
        let cast_ty = lt.clone();
        let target_name = target_ty.universe_key()?;
        let tu = self.ctx.type_universe.as_ref()?;
        // 2026-08-09 (Phase 12, SPEC §18.2): the meld gate is removed. The EOR
        // cast optimization now fires only when the cast type IS the target
        // type (the identity case the meld previously admitted).
        if cast_ty.universe_key()? != target_name {
            return None;
        }
        let _ = tu;
        let a = self.emit_expr(out, &lhs, indent);
        let b = self.emit_expr(out, &rhs, indent);
        if self.is_native_float(&cast_ty)
        {
            self.emit_eor_float_path(out, indent, v, kind, &a, &b, &cast_ty)
        } else {
            self.emit_eor_int_path(out, indent, v, kind, &a, &b, &cast_ty)
        }
    }

    /// Emit EOR float path: emit float binary op, bitcast/zext result to i64.
    fn emit_eor_float_path(
        &mut self,
        out: &mut String,
        indent: &str,
        v: &str,
        kind: &crate::ast::BinaryOpKind,
        a: &TypedRegister,
        b: &TypedRegister,
        cast_ty: &Type,
    ) -> Option<TypedRegister> {
        let fl_a = self.ensure_float_reg(out, indent, a);
        let fl_b = self.ensure_float_reg(out, indent, b);
        let fl_op = match kind {
            crate::ast::BinaryOpKind::Add => "fadd",
            crate::ast::BinaryOpKind::Sub => "fsub",
            crate::ast::BinaryOpKind::Mul => "fmul",
            crate::ast::BinaryOpKind::Div => "fdiv",
            _ => return None,
        };
        writeln!(out, "{}{} = {} float {}, {}", indent, v, fl_op, fl_a, fl_b).ok();
        let bi = self.fun.next_reg_with_prefix("eor_bi");
        writeln!(out, "{}{} = bitcast float {} to i32", indent, bi, v).ok();
        let ze = self.fun.next_reg_with_prefix("eor_ze");
        writeln!(out, "{}{} = zext i32 {} to i64", indent, ze, bi).ok();
        self.fun.reg_float_cache.insert(ze.clone(), v.to_string());
        let ret_ty = if cast_ty == &Type::float64() {
            Type::float64()
        } else {
            Type::float()
        };
        Some(TypedRegister {
            name: ze,
            ty: ret_ty,
        })
    }

    /// Emit EOR integer path: emit integer binary op directly.
    fn emit_eor_int_path(
        &mut self,
        out: &mut String,
        indent: &str,
        v: &str,
        kind: &crate::ast::BinaryOpKind,
        a: &TypedRegister,
        b: &TypedRegister,
        cast_ty: &Type,
    ) -> Option<TypedRegister> {
        let i_op = match kind {
            crate::ast::BinaryOpKind::Add => "add",
            crate::ast::BinaryOpKind::Sub => "sub",
            crate::ast::BinaryOpKind::Mul => "mul",
            crate::ast::BinaryOpKind::Div => "sdiv",
            _ => return None,
        };
        writeln!(out, "{}{} = {} i64 {}, {}", indent, v, i_op, a.name, b.name).ok();
        Some(TypedRegister {
            name: v.to_string(),
            ty: cast_ty.clone(),
        })
    }

    // ═══════════════════════════════════════════════════════════════
    // Section 9: Utility Methods (added 2026-07-14 for AST compat)
    // ═══════════════════════════════════════════════════════════════

    /// Width spelling for a Float-category bit width — the shared
    /// half/float/double(/i64-fallback) ladder. 2026-09-02 (plan
    /// fundamental-parent-membership): was repeated at the slot-derivation
    /// site and inside emit_binary_op.
    pub(crate) fn float_spelling(bits: u64) -> &'static str {
        if bits <= 16 { "half" }
        else if bits <= 32 { "float" }
        else if bits <= 64 { "double" }
        else { "i64" }
    }

    /// Mixed float/int binop operands: convert the integer side to the
    /// float width (sitofp) so both fcmp/fadd sides agree. Returns None
    /// when no conversion is needed (same-category operands, non-float
    /// ops). Ptr sides are left alone — the pointer arms handle them.
    /// 2026-09-02 (plan fundamental-parent-membership): the category
    /// protocol AUTHORIZES `y < 100`-style mixed comparisons (`y:
    /// Float16`, contract literals), and both the config templates and
    /// the fallback arms otherwise emit raw-mixed fcmp/fadd (invalid
    /// IR) — masked for Float32 by the folded-contract path taking those
    /// programs. Undo: delete and restore the unconverted operands.
    pub(crate) fn adapt_mixed_float_operands(
        &mut self,
        out: &mut String,
        indent: &str,
        l: &TypedRegister,
        r: &TypedRegister,
    ) -> Option<(TypedRegister, TypedRegister)> {
        let l_fbits = self.float_category_bits(&l.ty);
        let r_fbits = self.float_category_bits(&r.ty);
        let mixed = l_fbits.is_some() != r_fbits.is_some();
        if !mixed {
            return None;
        }
        let width = Self::float_spelling(l_fbits.or(r_fbits).unwrap_or(32));
        let fty = if l_fbits.is_some() { l.ty.clone() } else { r.ty.clone() };
        let lc = if l_fbits.is_some() || matches!(l.ty, Type::Ptr(_)) {
            l.clone()
        } else {
            let t = self.fun.gen_reg();
            let src_ty = self.llvm_type(&l.ty);
            writeln!(out, "{}{} = sitofp {} {} to {}", indent, t, src_ty, l.name, width).ok();
            TypedRegister { name: t, ty: fty.clone() }
        };
        let rc = if r_fbits.is_some() || matches!(r.ty, Type::Ptr(_)) {
            r.clone()
        } else {
            let t = self.fun.gen_reg();
            let src_ty = self.llvm_type(&r.ty);
            writeln!(out, "{}{} = sitofp {} {} to {}", indent, t, src_ty, r.name, width).ok();
            TypedRegister { name: t, ty: fty }
        };
        Some((lc, rc))
    }

    /// Box a typed register to i64 for uniform state storage.
    /// Handles Float64(double)→bitcast→i64, Float(float)→bitcast→i32→zext→i64,
    /// Bool(i8)→zext→i64, String/Data(i8*)→ptrtoint→i64. Int is already i64 (identity).
    pub(crate) fn adapt_to_i64(
        &mut self,
        out: &mut String,
        indent: &str,
        reg: &TypedRegister,
    ) -> String {
        // 2026-07-30: If already i64 (from state load), return as-is.
        // State fields are always stored as i64 regardless of Briev type,
        // so loads from %State produce i64 values that need no conversion.
        if self.llvm_type(&reg.ty) == "i64" {
            return reg.name.clone();
        }
        // 2026-07-26: Protocol-driven dispatch. No name matching.
        let ty = &reg.ty;
        // Float protocol: convert float/double to i64
        if self.is_protocol_member(ty, "Float") {
            let maxbits = self.ctx.type_universe.as_ref()
                .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
                .map(|rt| rt.max_bits).unwrap_or(32);
            if maxbits > 32 {
                let tr = self.fun.gen_reg();
                writeln!(out, "{}{} = bitcast double {} to i64", indent, tr, reg.name).ok();
                return tr;
            } else {
                let tr = self.fun.gen_reg();
                writeln!(out, "{}{} = bitcast float {} to i32", indent, tr, reg.name).ok();
                let ze = self.fun.gen_reg();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, ze, tr).ok();
                return ze;
            }
        }
        // Bool protocol: zext i8 to i64
        if self.is_protocol_member(ty, "Bool") {
            let tr = self.fun.gen_reg();
            writeln!(out, "{}{} = zext i8 {} to i64", indent, tr, reg.name).ok();
            return tr;
        }
        // Char protocol: a Char reg is native i32 (literal/let/field/cast);
        // boxed Char params are i64 and typed Int in SSA, so they never reach
        // this arm (they hit the `llvm_type == i64` early return above).
        if self.is_protocol_member(ty, "Char") {
            let tr = self.fun.gen_reg();
            writeln!(out, "{}{} = zext i32 {} to i64", indent, tr, reg.name).ok();
            return tr;
        }
        // String / Blob protocol: a String is a ptr to [len][bytes] (B0),
        // so adapting to i64 is a ptrtoint. The SSO handle-extraction branch
        // was retired in B4.
        let is_string = self.is_protocol_member(ty, "String");
        let is_data = self.is_protocol_member(ty, "Blob");
        if is_string || is_data {
            let tr = self.fun.gen_reg();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, tr, reg.name).ok();
            return tr;
        }
        // Int / UInt protocol: widen if narrower than i64
        if self.is_protocol_member(ty, "Int") {
            let llvm_ty = self.llvm_type(ty);
            if llvm_ty != "i64" && llvm_ty.starts_with('i') {
                let tr = self.fun.gen_reg();
                let is_unsigned = self.is_protocol_member(ty, "UInt");
                if is_unsigned {
                    writeln!(out, "{}{} = zext {} {} to i64", indent, tr, llvm_ty, reg.name).ok();
                } else {
                    writeln!(out, "{}{} = sext {} {} to i64", indent, tr, llvm_ty, reg.name).ok();
                }
                return tr;
            }
        }
        // Ptr: already i64 (via ptrtoint in emit_malloc).
        if matches!(ty, Type::Ptr(_)) {
            return reg.name.clone();
        }
        reg.name.clone()
    }

    /// Emit a getelementptr for a state field and return the GEP register name.
    /// `prefix` is used to make register names unique within a function.
    /// 2026-07-31 (A4): Is a state field an LLVM aggregate (array) type?
    /// Aggregate fields are memory-resident — they are never loop-carried as
    /// scalar phis (a phi cannot hold a runtime-indexed array). The loop
    /// engines access them via the %State GEP path instead.
    pub(crate) fn is_aggregate_field(&self, name: &str) -> bool {
        self.ctx
            .field_index_map
            .get(name)
            .and_then(|idx| self.ctx.field_types.get(*idx))
            .map_or(false, |t| t.starts_with('['))
    }

    pub(crate) fn emit_state_gep(
        &mut self,
        out: &mut String,
        indent: &str,
        _prefix: &str,
        state_ptr: &str,
        idx: usize,
    ) -> String {
        let r = self.fun.gen_reg();
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr {}, i32 0, i32 {}",
            indent, r, state_ptr, idx
        )
        .ok();
        r
    }

    // 2026-07-19: DRY consolidation helpers — centralized state field access.
    // All 44 hand-rolled GEP+load/store sites should migrate to these.

    /// Load a state field as i64. Returns (register_name, briev_type).
    /// The briev type can be passed to ensure_typed_value for float unboxing.
    /// Load a state field with its native LLVM type. Returns (register_name, briev_type).
    pub(crate) fn emit_state_load_i64(
        &mut self,
        out: &mut String,
        indent: &str,
        name: &str,
    ) -> Option<(String, Type)> {
        let idx = *self.ctx.field_index_map.get(name)?;
        self.load_field_type(out, indent, idx)
    }

    /// Store a value to a state field with the native LLVM type.
    /// The value should match the field's LLVM type (float, double, i64, etc.).
    pub(crate) fn emit_state_store_i64(
        &mut self,
        out: &mut String,
        indent: &str,
        name: &str,
        val: &str,
    ) -> Option<()> {
        let idx = *self.ctx.field_index_map.get(name)?;
        self.store_field_type(out, indent, idx, val)
    }

    /// Load a state field with its native LLVM type by index. Returns (register_name, briev_type).
    pub(crate) fn emit_state_load_i64_by_idx(
        &mut self,
        out: &mut String,
        indent: &str,
        idx: usize,
    ) -> (String, Type) {
        self.load_field_type(out, indent, idx).unwrap_or_else(|| {
            let v = self.fun.next_reg_with_prefix("slf");
            writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
            (v, Type::int())
        })
    }

    /// Store a value to a state field by index with the native LLVM type.
    pub(crate) fn emit_state_store_i64_by_idx(
        &mut self,
        out: &mut String,
        indent: &str,
        idx: usize,
        val: &str,
    ) {
        self.store_field_type(out, indent, idx, val);
    }

    /// Internal helper: load a state field with its native LLVM type.
    fn load_field_type(
        &mut self,
        out: &mut String,
        indent: &str,
        idx: usize,
    ) -> Option<(String, Type)> {
        let briev_ty = self
            .ctx
            .field_briev_types
            .get(idx)
            .cloned()
            .unwrap_or(Type::int());
        let llvm_ty = self
            .ctx
            .field_types
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "i64".to_string());
        let gep = self.fun.gen_reg();
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, gep, idx
        )
        .ok();
        let val = self.fun.gen_reg();
        write!(
            out,
            "{}{} = load {}, ptr {}, align {}",
            indent, val, llvm_ty, gep, self.align_of(&llvm_ty)
        ).ok();
        // 2026-07-27: Append !range metadata if this field has contract-driven or
        // type-driven bounds. The metadata node was created in emit_transaction.
        if let Some(field_name) = self.ctx.idx_to_field_name.get(&idx) {
            if let Some(mi) = self.ctx.field_to_meta_idx.get(field_name) {
                write!(out, ", !range !{}", mi).ok();
            }
        }
        // 2026-07-29: Append !invariant.load for Ptr<T> state fields.
        // Pointers (allocated by Malloc#) are assigned once during init and
        // never reassigned. The stored i64 value is invariant for the entire
        // program lifetime. This allows LICM to hoist the load of the pointer
        // from inside the loop body to the preheader, eliminating one
        // GEP+load per iteration per pointer field.
        // See docs/plans/2026-07-29-frontend-ir-quality-improvements.md §B.
        if matches!(briev_ty, Type::Ptr(_) | Type::PtrConst(_)) {
            let md_idx = self.fun.metadata_counter;
            self.fun.metadata_counter += 1;
            writeln!(self.fun.pending_metadata, "!{} = !{{}}", md_idx).ok();
            write!(out, ", !invariant.load !{}", md_idx).ok();
        }
        writeln!(out).ok();
        Some((val, briev_ty))
    }

    /// Internal helper: store a value to a state field with its native LLVM type.
    fn store_field_type(
        &mut self,
        out: &mut String,
        indent: &str,
        idx: usize,
        val: &str,
    ) -> Option<()> {
        let llvm_ty = self
            .ctx
            .field_types
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "i64".to_string());
        let gep = self.fun.gen_reg();
        writeln!(
            out,
            "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, gep, idx
        )
        .ok();
        writeln!(
            out,
            "{}store {} {}, ptr {}, align {}",
            indent,
            llvm_ty,
            val,
            gep,
            self.align_of(&llvm_ty)
        )
        .ok();
        Some(())
    }

    /// Ensure the value register has the expected LLVM type, inserting
    /// trunc/zext/bitcast as needed. Returns the possibly-converted register name.
    pub(crate) fn ensure_typed_value(
        &mut self,
        out: &mut String,
        indent: &str,
        expected_llvm_ty: &str,
        val: &str,
        briev_ty: Option<Type>,
        _universe: Option<&crate::type_universe::TypeUniverse>,
    ) -> String {
        // 2026-08-13 (merge fix): Ptr<T> and struct/collection HANDLES are
        // stored as i64 (their llvm_type maps to "ptr" but the register is an
        // integer handle) — returned unchanged so a state-slot store never
        // double-ptrtoints them. Only genuine ptr registers (String/Data
        // values) convert in the ("ptr","i64") arm below.
        if let Some(ref bt) = briev_ty {
            if matches!(bt, Type::Ptr(_))
                || matches!(bt, Type::Custom(n) if self.ctx.struct_types.contains_key(n) || self.ctx.obj_types.contains(n))
                || matches!(bt, Type::Applied(n, _) if self.ctx.struct_types.contains_key(n) || self.ctx.obj_types.contains(n))
            {
                return val.to_string();
            }
        }
        let Some(ref bt) = briev_ty else {
            return val.to_string();
        };
        let actual_ty = self.llvm_type(bt);
        if actual_ty == expected_llvm_ty {
            return val.to_string();
        }
        let actual_ty_clone = actual_ty.to_string();
        match (actual_ty.as_ref(), expected_llvm_ty) {
            ("double", "i64") | ("float", "i64") => {
                let r = self.fun.gen_reg();
                writeln!(
                    out,
                    "{}{} = bitcast {} {} to i64",
                    indent, r, actual_ty_clone, val
                )
                .ok();
                r
            }
            ("i64", "double") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = bitcast i64 {} to double", indent, r, val).ok();
                r
            }
            ("i64", "float") => {
                let tr = self.fun.gen_reg();
                writeln!(out, "{}{} = trunc i64 {} to i32", indent, tr, val).ok();
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = bitcast i32 {} to float", indent, r, tr).ok();
                r
            }
            ("i8", "i64") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = zext i8 {} to i64", indent, r, val).ok();
                r
            }
            ("i32", "i64") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, r, val).ok();
                r
            }
            // 2026-08-13 (merge fix): state-slot adaptation. A String/Data
            // value is a `ptr` ([len][bytes]) while the slot is i64 (the
            // %State struct stores most slots as i64) — store the pointer
            // as its integer address. A boxed i64 value into an i8 slot
            // (Bool state fields) or i32 slot truncates. (Callers skip this
            // for struct/collection HANDLES, whose registers are already i64
            // though their type maps to "ptr".)
            ("ptr", "i64") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, r, val).ok();
                r
            }
            ("i64", "i8") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = trunc i64 {} to i8", indent, r, val).ok();
                r
            }
            ("i64", "i32") => {
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = trunc i64 {} to i32", indent, r, val).ok();
                r
            }
            _ => val.to_string(),
        }
    }
}
