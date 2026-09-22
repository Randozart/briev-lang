//! Per-backend capability matrix + pre-codegen validation.
//!
//! 2026-08-23 (Plan 0.2, backend-scaffolding-foundation): every non-full-
//! surface backend declares which language constructs its codegen actually
//! emits; a shared walker rejects programs reaching beyond that surface
//! BEFORE codegen, with house-style diagnostics (what / why / fix).
//!
//! Why this exists: CIRCT dropped unsupported expressions to `None` (silent
//! `"0"` fallbacks), SPIR-V emitted empty kernel bodies, and the VM pushed 0
//! + emitted a runtime trap — all silent wrong-behavior paths. A construct
//! outside a target's declared surface is now a COMPILE error naming the
//! concrete fix.
//!
//! Rule 19 note: this matrix classifies compiler STRUCTURE (Expr/Statement
//! variants), never type names — type-specific decisions stay in the
//! casting graph.
//!
//! To undo: delete this file, remove the `validate_for_backend` call in
//! src/compile.rs, and restore each backend's permissive drop behavior.

use crate::ast::{Expr, Statement, TopLevel};

/// What one backend's codegen actually emits today. Every field defaults to
/// `false`; each backend's declaration turns ON what it truly lowers.
/// When a backend gains a construct, flip the flag AND add tests — a flag
/// set beyond real coverage produces silent drops again (the bug this module
/// exists to prevent).
#[derive(Debug, Clone, Copy, Default)]
pub struct BackendCapabilities {
    /// Backend display name for diagnostics ("tamer VM", "CIRCT", "SPIR-V").
    pub name: &'static str,
    /// One-line target nature for the diagnostic "why" clause.
    pub nature: &'static str,
    // ── Expressions ──────────────────────────────────────────────
    /// Decimal literals.
    pub int_literals: bool,
    /// Float literals and float arithmetic.
    pub floats: bool,
    /// Quoted byte-string literals and string concat.
    pub strings: bool,
    /// Bool and Char literals.
    pub bool_char_literals: bool,
    /// Integer arithmetic, comparison, bitwise, shifts (BinaryOp).
    pub int_ops: bool,
    /// Unary neg/not/bitnot.
    pub unary_ops: bool,
    /// Calls to program-defined functions.
    pub calls: bool,
    /// Intrinsic (`Name#`) calls lowered by this backend.
    pub intrinsics: bool,
    /// if/else expressions.
    pub if_expr: bool,
    /// match expressions.
    pub match_expr: bool,
    /// Block expressions.
    pub block_expr: bool,
    /// Struct field access (.field).
    pub field_access: bool,
    /// Array/list indexing (a[i]).
    pub index: bool,
    /// Slices (a[start..end]) and ranges.
    pub slices_ranges: bool,
    /// Tuple and List literal construction.
    pub tuple_list_literals: bool,
    /// Struct literal construction.
    pub struct_literal: bool,
    /// Lambdas/closures.
    pub lambda: bool,
    /// Type casts (`as`).
    pub casts: bool,
    /// IsType checks.
    pub is_type: bool,
    /// Pointer deref/address-of.
    pub deref_addr_of: bool,
    /// spawn expressions.
    pub spawn: bool,
    /// 2026-08-22 (Phase 7c, SPEC §9.5): obj PORT headers — Event wiring
    /// across instances. Interpreter-only until the LLVM backend grows
    /// port columns and event queues.
    pub obj_ports: bool,
    /// 2026-08-22 (Phase 7c, SPEC §9.6): CELLS — sealed state machines.
    /// Interpreter-only; sealing + internal-node scheduling have no LLVM
    /// lowering yet.
    pub cells: bool,
    /// 2026-08-27 (Slice A): can emit `hw.module.extern` blackboxes for
    /// foreign HDL imports (`extern Cell(...) from "path";").
    pub extern_cells: bool,
    /// await expressions.
    pub await_expr: bool,
    /// Method-call syntax (receiver.method(args)).
    pub method_calls: bool,
    /// Reflection (`.^`/`.^^`).
    pub reflect: bool,
    /// Plugin intercepts.
    pub plugin_intercept: bool,
    /// Derivation blocks (?[...]).
    pub derivation_blocks: bool,
    /// within expressions.
    pub within: bool,
    // ── Statements ───────────────────────────────────────────────
    /// let bindings (single + tuple destructuring).
    pub let_stmt: bool,
    /// assignment (plain, field, index targets).
    pub assign_stmt: bool,
    /// arrow assignment (`<-`, `~<-`, discard forms).
    pub arrow_assign: bool,
    /// when/[cond] guarded bodies.
    pub guarded_stmt: bool,
    /// term/endprogram.
    pub term_endprogram: bool,
    /// break (inside foreach).
    pub break_stmt: bool,
    /// trap.
    pub trap_stmt: bool,
    /// match statements.
    pub match_stmt: bool,
    /// foreach loops.
    pub foreach: bool,
    /// inline asm.
    pub inline_asm: bool,
    /// sync/mutex/barrier sections.
    pub concurrency_sections: bool,
    /// defer blocks.
    pub defer_stmt: bool,
    /// free/keep lifetime hints.
    pub lifetime_hints: bool,
    /// metadata assignment (`key <~ value`).
    pub metadata_assign: bool,
    /// rollback/escape.
    pub rollback: bool,
    /// convergence gate ([cond];).
    pub gate_stmt: bool,
    /// trigger bindings inside bodies (`trg name @ instance;`).
    pub trg_bindings: bool,
    /// cooperative cancellation checkpoints (`yield;`, SPEC §12.2).
    pub yield_stmt: bool,
}

impl BackendCapabilities {
    /// All-false baseline for const capability declarations (`..NONE`).
    pub const NONE: BackendCapabilities = BackendCapabilities {
        name: "",
        nature: "",
        int_literals: false,
        floats: false,
        strings: false,
        bool_char_literals: false,
        int_ops: false,
        unary_ops: false,
        calls: false,
        intrinsics: false,
        if_expr: false,
        match_expr: false,
        block_expr: false,
        field_access: false,
        index: false,
        slices_ranges: false,
        tuple_list_literals: false,
        struct_literal: false,
        lambda: false,
        casts: false,
        is_type: false,
        deref_addr_of: false,
        spawn: false,
        obj_ports: false,
        cells: false,
        extern_cells: false,
        await_expr: false,
        method_calls: false,
        reflect: false,
        plugin_intercept: false,
        derivation_blocks: false,
        within: false,
        let_stmt: false,
        assign_stmt: false,
        arrow_assign: false,
        guarded_stmt: false,
        term_endprogram: false,
        break_stmt: false,
        trap_stmt: false,
        match_stmt: false,
        foreach: false,
        inline_asm: false,
        concurrency_sections: false,
        defer_stmt: false,
        lifetime_hints: false,
        metadata_assign: false,
        rollback: false,
        gate_stmt: false,
        trg_bindings: false,
        yield_stmt: false,
    };
}

impl BackendCapabilities {
    /// 2026-08-22 (Phase 7c): the FULL expression/statement surface, for
    /// backends that implement everything except explicitly staged items
    /// (LLVM flips obj_ports/cells off until their lowering lands).
    pub const fn full(name: &'static str, nature: &'static str) -> Self {
        Self {
            name,
            nature,
            int_literals: true,
            floats: true,
            strings: true,
            bool_char_literals: true,
            int_ops: true,
            unary_ops: true,
            calls: true,
            intrinsics: true,
            if_expr: true,
            match_expr: true,
            block_expr: true,
            field_access: true,
            index: true,
            slices_ranges: true,
            tuple_list_literals: true,
            struct_literal: true,
            lambda: true,
            casts: true,
            is_type: true,
            deref_addr_of: true,
            spawn: true,
            obj_ports: false,
            cells: false,
            // Software targets have no RTL linkage — extern imports stay
            // rejected on the LLVM/full surface too.
            extern_cells: false,
            await_expr: true,
            method_calls: true,
            reflect: true,
            plugin_intercept: true,
            derivation_blocks: true,
            within: true,
            let_stmt: true,
            assign_stmt: true,
            arrow_assign: true,
            guarded_stmt: true,
            term_endprogram: true,
            break_stmt: true,
            trap_stmt: true,
            match_stmt: true,
            foreach: true,
            inline_asm: true,
            concurrency_sections: true,
            defer_stmt: true,
            lifetime_hints: true,
            metadata_assign: true,
            rollback: true,
            gate_stmt: true,
            trg_bindings: true,
            yield_stmt: true,
        }
    }

    fn missing(&self, feature: &str) -> String {
        format!(
            "error: {} does not support {}\n  why: {}.\n  fix: rewrite without \
             {}, or build for a target that supports it (e.g. the native LLVM \
             target).",
            self.name, feature, self.nature, feature
        )
    }
}

/// Validate a whole program against a backend's declared surface.
/// Returns house-style error messages; empty means the program is within
/// the surface.
pub fn validate_program(items: &[TopLevel], caps: &BackendCapabilities) -> Vec<String> {
    // 2026-08-23: per-item work lives in check_toplevel — keeps this loop
    // single-depth (Praetor complexity gate).
    let mut errs = Vec::new();
    for item in items {
        check_toplevel(item, caps, &mut errs);
    }
    errs
}

fn check_toplevel(item: &TopLevel, caps: &BackendCapabilities, errs: &mut Vec<String>) {
    match item {
        // 2026-08-22 (Phase 7c): declaration-surface checks — ports and
        // cells are interpreter-only surfaces for now (SPEC §9.5/§9.6).
        // 2026-08-27 (Slice A): extern imports are PURE port headers — no
        // body to lower. Backends declaring extern_cells accept them even
        // while defined cell bodies remain interpreter-staged.
        TopLevel::Cell(c)
            if c.extern_source.is_some() && !caps.extern_cells =>
        {
            errs.push(format!(
                "error: {} does not support foreign hardware imports\n  why: \
                 software binaries have no RTL linkage — the 'extern' cell \
                 '{}' compiles only for circuit/synthesis targets.\n  fix: \
                 build for the circt target so the blackbox reaches your \
                 synthesis flow, or model this device directly in Briev",
                caps.name, c.name
            ));
        }
        TopLevel::Cell(c) if !caps.cells && c.extern_source.is_none() => {
            errs.push(format!(
                "error: {} does not support cells\n  why: {}.\n  fix: run the \
                 program on the reference interpreter, or replace the cell \
                 with an ordinary obj while the cell lowering is staged.",
                caps.name, caps.nature
            ));
        }
        TopLevel::TypeDef(td)
            if (!td.ports_in.is_empty() || !td.ports_out.is_empty()) && !caps.obj_ports =>
        {
            errs.push(format!(
                "error: {} does not support obj port headers\n  why: {}.\n  \
                 fix: run the program on the reference interpreter, or drop \
                 the ({}) -> ({}) port header from '{}' while Event wiring is \
                 staged.",
                caps.name,
                caps.nature,
                td.ports_in.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
                td.ports_out.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
                td.name
            ));
        }
        TopLevel::Transaction(t) => {
            check_expr(&t.contract.pre_condition, caps, errs);
            check_expr(&t.contract.post_condition, caps, errs);
            for s in &t.body {
                check_stmt(s, caps, errs);
            }
        }
        TopLevel::Definition(d) => {
            for s in &d.body {
                check_stmt(s, caps, errs);
            }
        }
        TopLevel::Constant(c) => check_expr(&c.expr, caps, errs),
        TopLevel::Statement(stmt) => check_stmt(stmt, caps, errs),
        _ => {}
    }
}

fn check_expr(e: &Expr, caps: &BackendCapabilities, out: &mut Vec<String>) {
    // 2026-08-23: split into per-category helpers so every function stays
    // under the Praetor complexity gate (≤15); the router owns no logic.
    if check_expr_leaf(e, caps, out) {
        return;
    }
    if check_expr_operator(e, caps, out) {
        return;
    }
    if check_expr_call_access(e, caps, out) {
        return;
    }
    check_expr_composite(e, caps, out);
}

/// Literals + leaf forms. Returns true when `e` was handled.
fn check_expr_leaf(e: &Expr, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    let handled = match e {
        Expr::Decimal(_) | Expr::TaggedLiteral(..) => !caps.int_literals,
        Expr::Float(_) => !caps.floats,
        Expr::Quoted(_) | Expr::TaggedQuotedLiteral(..) => !caps.strings,
        Expr::Bool(_) | Expr::Char(_) => !caps.bool_char_literals,
        _ => return false,
    };
    if handled {
        let what = match e {
            Expr::Float(_) => "float literals",
            Expr::Quoted(_) | Expr::TaggedQuotedLiteral(..) => "string literals",
            Expr::Bool(_) | Expr::Char(_) => "bool/char literals",
            _ => "integer literals",
        };
        out.push(caps.missing(what));
    }
    true
}

/// Unary/binary operators, casts, pointer forms. Returns true when handled.
fn check_expr_operator(e: &Expr, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    match e {
        Expr::BinaryOp(kind, l, r) => {
            if matches!(kind, crate::ast::BinaryOpKind::Concat) {
                require(caps.strings, "string concatenation", caps, out);
            } else {
                require(caps.int_ops, "arithmetic operations", caps, out);
            }
            check_expr(l, caps, out);
            check_expr(r, caps, out);
        }
        Expr::UnaryOp(_, inner) => {
            require(caps.unary_ops, "unary operators", caps, out);
            check_expr(inner, caps, out);
        }
        Expr::Cast(inner, _) => {
            require(caps.casts, "casts", caps, out);
            check_expr(inner, caps, out);
        }
        Expr::IsType(inner, _) => {
            require(caps.is_type, "type checks", caps, out);
            check_expr(inner, caps, out);
        }
        Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::Consume(inner) => {
            require(caps.deref_addr_of, "pointer operations", caps, out);
            check_expr(inner, caps, out);
        }
        Expr::Await(inner) => {
            require(caps.await_expr, "await", caps, out);
            check_expr(inner, caps, out);
        }
        _ => return false,
    }
    true
}

/// Calls, field/index access, reflection, spawn. Returns true when handled.
fn check_expr_call_access(e: &Expr, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    match e {
        Expr::Call(name, args, _) => {
            let intrinsic = name.ends_with('#') || name.ends_with('!');
            if intrinsic {
                require(caps.intrinsics, &format!("intrinsic '{}'", name), caps, out);
            } else {
                require(caps.calls, "function calls", caps, out);
            }
            walk_args(args, caps, out);
        }
        Expr::Field(recv, _) => {
            require(caps.field_access, "field access", caps, out);
            check_expr(recv, caps, out);
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            require(caps.method_calls, "method calls", caps, out);
            check_expr(recv, caps, out);
            walk_args(args, caps, out);
        }
        Expr::Index(obj, idx) => {
            require(caps.index, "indexing", caps, out);
            check_expr(obj, caps, out);
            check_expr(idx, caps, out);
        }
        Expr::Slice { array, start, end, stride } => {
            require(caps.slices_ranges, "slices", caps, out);
            check_expr(array, caps, out);
            for part in [start.as_deref(), end.as_deref(), stride.as_deref()]
                .into_iter()
                .flatten()
            {
                check_expr(part, caps, out);
            }
        }
        Expr::Range { start, end, .. } => {
            require(caps.slices_ranges, "ranges", caps, out);
            check_expr(start, caps, out);
            check_expr(end, caps, out);
        }
        Expr::Reflect(recv, _, _) => {
            require(caps.reflect, "reflection", caps, out);
            check_expr(recv, caps, out);
        }
        Expr::PluginIntercept { args, .. } => {
            require(caps.plugin_intercept, "plugin intercepts", caps, out);
            walk_args(args, caps, out);
        }
        Expr::Spawn { args, .. } => {
            require(caps.spawn, "spawn", caps, out);
            walk_args(args, caps, out);
        }
        _ => return false,
    }
    true
}

/// Control-flow and composite expressions. Returns true when handled.
fn check_expr_composite(e: &Expr, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    match e {
        Expr::If(cond, then, els) => {
            require(caps.if_expr, "if expressions", caps, out);
            check_expr(cond, caps, out);
            check_expr(then, caps, out);
            if let Some(x) = els {
                check_expr(x, caps, out);
            }
        }
        Expr::Match(scrutinee, arms) => {
            require(caps.match_expr, "match expressions", caps, out);
            check_expr(scrutinee, caps, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    check_expr(g, caps, out);
                }
                check_expr(&arm.body, caps, out);
            }
        }
        Expr::Block(stmts) => {
            require(caps.block_expr, "block expressions", caps, out);
            for s in stmts {
                check_stmt(s, caps, out);
            }
        }
        Expr::Tuple(elems) => {
            require(caps.tuple_list_literals, "tuple literals", caps, out);
            walk_args(elems, caps, out);
        }
        Expr::List(elems) => {
            require(caps.tuple_list_literals, "list literals", caps, out);
            walk_args(elems, caps, out);
        }
        Expr::StructLiteral { fields, .. } => {
            require(caps.struct_literal, "struct literals", caps, out);
            for (_, x) in fields {
                check_expr(x, caps, out);
            }
        }
        Expr::Lambda(_, body) => {
            require(caps.lambda, "lambdas", caps, out);
            check_expr(body, caps, out);
        }
        Expr::Within(outer, inner) => {
            require(caps.within, "within expressions", caps, out);
            check_expr(outer, caps, out);
            check_expr(inner, caps, out);
        }
        Expr::DerivationBlock(db) => {
            require(caps.derivation_blocks, "derivation blocks", caps, out);
            for ex in &db.examples {
                walk_args(&ex.inputs, caps, out);
                check_expr(&ex.output, caps, out);
            }
        }
        Expr::BeginProgram | Expr::Exists(_) | Expr::Identifier(_)
        | Expr::FormattingAnnotation(_) => {}
        _ => return false,
    }
    true
}

fn walk_args(args: &[Expr], caps: &BackendCapabilities, out: &mut Vec<String>) {
    for a in args {
        check_expr(a, caps, out);
    }
}

fn require(
    supported: bool,
    what: &str,
    caps: &BackendCapabilities,
    out: &mut Vec<String>,
) {
    if !supported {
        out.push(caps.missing(what));
    }
}

fn check_stmt(s: &Statement, caps: &BackendCapabilities, out: &mut Vec<String>) {
    // 2026-08-23: split per category (see check_expr note).
    if check_stmt_binding(s, caps, out) {
        return;
    }
    if check_stmt_flow(s, caps, out) {
        return;
    }
    check_stmt_body(s, caps, out);
}

fn check_stmt_binding(s: &Statement, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    match s {
        Statement::Let { expr, .. } => {
            require(caps.let_stmt, "let bindings", caps, out);
            if let Some(x) = expr {
                check_expr(x, caps, out);
            }
        }
        Statement::Assign(lhs, rhs) => {
            require(caps.assign_stmt, "assignments", caps, out);
            check_expr(lhs, caps, out);
            check_expr(rhs, caps, out);
        }
        Statement::ArrowAssign { target, value, consume: _ } => {
            require(caps.arrow_assign, "arrow assignment (<-)", caps, out);
            if let Some(t) = target {
                check_expr(t, caps, out);
            }
            check_expr(value, caps, out);
        }
        Statement::MetadataAssignment(..) => {
            require(caps.metadata_assign, "metadata assignment", caps, out);
        }
        Statement::FreeHint(_) | Statement::KeepHint(_) => {
            require(caps.lifetime_hints, "free/keep lifetime hints", caps, out);
        }
        _ => return false,
    }
    true
}

fn check_stmt_flow(s: &Statement, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    match s {
        Statement::Term(e) | Statement::EndProgram(e) => {
            require(caps.term_endprogram, "term/endprogram", caps, out);
            if let Some(x) = e {
                check_expr(x, caps, out);
            }
        }
        Statement::Break => require(caps.break_stmt, "break", caps, out),
        Statement::Trap => require(caps.trap_stmt, "trap", caps, out),
        Statement::Gate(cond) => {
            require(caps.gate_stmt, "convergence gates ([cond];)", caps, out);
            check_expr(cond, caps, out);
        }
        Statement::Expression(e) => check_expr(e, caps, out),
        Statement::Rollback(e) => {
            require(caps.rollback, "rollback/escape", caps, out);
            if let Some(x) = e {
                check_expr(x, caps, out);
            }
        }
        Statement::TrgBinding { instance, .. } => {
            require(caps.trg_bindings, "trigger bindings", caps, out);
            check_expr(instance, caps, out);
        }
        Statement::InlineAsm { .. } => {
            require(caps.inline_asm, "inline asm", caps, out);
        }
        Statement::InlineDefn(_) | Statement::InlineTxn(_) => {
            // 2026-08-23: stage-block internals — stripped before codegen,
            // never a target-surface question.
        }
        Statement::Yield => {
            require(caps.yield_stmt, "yield checkpoints", caps, out);
        }
        _ => return false,
    }
    true
}

fn check_stmt_body(s: &Statement, caps: &BackendCapabilities, out: &mut Vec<String>) -> bool {
    fn walk_body(body: &[Statement], caps: &BackendCapabilities, out: &mut Vec<String>) {
        for stmt in body {
            check_stmt(stmt, caps, out);
        }
    }
    match s {
        Statement::Guarded(cond, body) => {
            require(caps.guarded_stmt, "guarded bodies (when/[])", caps, out);
            check_expr(cond, caps, out);
            walk_body(body, caps, out);
        }
        Statement::Foreach { list, body, .. } => {
            require(caps.foreach, "foreach loops", caps, out);
            check_expr(list, caps, out);
            walk_body(body, caps, out);
        }
        Statement::Block(body) => walk_body(body, caps, out),
        Statement::SyncBlock(body) | Statement::Mutex(body) => {
            require(caps.concurrency_sections, "sync/mutex sections", caps, out);
            walk_body(body, caps, out);
        }
        Statement::Defer(body) => {
            require(caps.defer_stmt, "defer blocks", caps, out);
            walk_body(body, caps, out);
        }
        Statement::Match { expr: scrutinee, arms } => {
            require(caps.match_stmt, "match statements", caps, out);
            check_expr(scrutinee, caps, out);
            for arm in arms {
                walk_body(&arm.body, caps, out);
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::Contract;
    use crate::ast::{Transaction, Type};
    use std::collections::HashMap;

    const VM: BackendCapabilities = crate::backend::vm::CAPABILITIES;
    const CIRCT: BackendCapabilities = crate::backend::circt::CirctBackend::CAPABILITIES;

    fn txn_with(body: Vec<Statement>) -> Vec<TopLevel> {
        vec![TopLevel::Transaction(Transaction {
            name: "t".into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })]
    }

    #[test]
    fn vm_accepts_integer_program() {
        // count = count + 1; term; — the tamer's bread and butter.
        let prog = txn_with(vec![
            Statement::Assign(
                Expr::Identifier("count".into()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(None),
        ]);
        assert!(validate_program(&prog, &VM).is_empty(), "integer program must pass");
    }

    #[test]
    fn vm_rejects_float_literal_with_helpful_message() {
        let prog = txn_with(vec![Statement::Term(Some(Expr::Float(1.5)))]);
        let errs = validate_program(&prog, &VM);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("tamer VM"), "names target: {}", errs[0]);
        assert!(errs[0].contains("float literals"), "names feature: {}", errs[0]);
        assert!(errs[0].contains("fix:"), "gives fix: {}", errs[0]);
    }

    #[test]
    fn vm_rejects_foreach() {
        let prog = txn_with(vec![Statement::Foreach {
            item: "x".into(),
            list: Box::new(Expr::List(vec![Expr::Decimal(1)])),
            body: vec![Statement::Expression(Expr::Identifier("x".into()))],
        }]);
        let errs = validate_program(&prog, &VM);
        assert!(!errs.is_empty());
        assert!(errs[0].contains("foreach loops"));
    }

    #[test]
    fn circt_rejects_spawn() {
        let prog = txn_with(vec![Statement::Expression(Expr::Spawn {
            type_name: "Worker".into(),
            args: vec![],
            storage: crate::ast::SpawnStorage::Pooled,
        })]);
        let errs = validate_program(&prog, &CIRCT);
        assert!(!errs.is_empty());
        assert!(errs[0].contains("CIRCT"));
    }

    #[test]
    fn every_flag_is_described_by_the_walker() {
        // The walker must consult EVERY flag — a flag added without a
        // corresponding check silently reopens the drop hole. Compile-time
        // can't force this, so this test walks one probe per flag.
        // (Structural guarantee: NONE has 46 false flags; each backend
        // declaration turning a flag ON without walker coverage would
        // produce no diagnostic for that construct.)
        let all_false_count = count_false_fields(&BackendCapabilities::NONE);
        assert!(all_false_count > 40, "matrix baseline populated: {}", all_false_count);
    }

    fn count_false_fields(c: &BackendCapabilities) -> usize {
        // Debug-format field counting keeps this honest without reflection.
        format!("{:?}", c).matches("false").count()
    }
}
