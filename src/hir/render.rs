// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P5 (verification program): deterministic canonical HIR text.
//!
//! Purpose: pass-level snapshots. The renderer is order-stable by
//! construction (program/vec order only, no HashMap iteration) and includes
//! node ids, best-effort types, law markers, and cast paths, so an
//! unintended pass change shows up as a text diff. It is not a source
//! printer and must not be used for parsing.

use super::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirContract, HirGlobal, HirInit, HirProgram, HirTypeDef};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use std::fmt::Write;

pub fn render_program(program: &HirProgram) -> String {
    let mut out = String::new();
    out.push_str("program\n");
    for global in &program.globals {
        render_global(&mut out, global);
    }
    for (name, ty) in &program.const_types {
        line(&mut out, 1, &format!("const {name}: {ty:?}"));
    }
    for (name, expr) in &program.const_exprs {
        line(&mut out, 1, &format!("const-expr {name}"));
        render_expr(&mut out, 2, expr);
    }
    for name in &program.trigger_names {
        line(&mut out, 1, &format!("trigger {name}"));
    }
    for def in &program.type_defs {
        render_type_def(&mut out, def);
    }
    for init in &program.inits {
        render_init(&mut out, init);
    }
    for callable in &program.callables {
        render_callable(&mut out, callable);
    }
    out
}

pub fn render_callable(out: &mut String, c: &HirCallable) {
    let params: Vec<String> = c.params.iter().map(|p| format!("{}: {:?}", p.name, p.ty)).collect();
    line(
        out,
        1,
        &format!(
            "callable {} kind={:?} owner={:?} exported={} symbol={:?} result={:?} type_params={:?} variadic={} sync={:?} trait_default={} impl_fn={} isr={} outputs={:?}",
            c.name,
            c.kind,
            c.owner,
            c.exported,
            c.export_symbol,
            c.result,
            c.type_params,
            c.variadic,
            c.sync_domains,
            c.trait_default,
            c.is_impl_fn,
            c.is_isr,
            c.output_list,
        ),
    );
    line(out, 2, &format!("params [{}]", params.join(", ")));
    if let Some(contract) = &c.contract {
        render_contract(out, contract);
    }
    render_body(out, 2, &c.body.statements);
}

fn render_global(out: &mut String, g: &HirGlobal) {
    line(
        out,
        1,
        &format!(
            "global {} ty={:?} storage={:?} state_decl={} modifiers={:?}",
            g.name, g.ty, g.declared_storage, g.is_state_decl, g.modifiers
        ),
    );
    if let Some(expr) = &g.expr {
        render_expr(out, 2, expr);
    }
}

fn render_type_def(out: &mut String, def: &HirTypeDef) {
    line(
        out,
        1,
        &format!(
            "type {} kind={:?} params={:?} protocol={:?} coll={} traits={:?} ops={:?} binding_fns={:?}",
            def.name,
            def.kind,
            def.type_params,
            def.protocol,
            def.is_coll,
            def.traits,
            def.op_bindings,
            def.binding_fns,
        ),
    );
}

fn render_init(out: &mut String, init: &HirInit) {
    line(out, 1, &format!("init {} bound={:?}", init.name, init.bound));
    if let Some(value) = &init.value {
        render_expr(out, 2, value);
    }
    render_body(out, 2, &init.body);
}

fn render_contract(out: &mut String, contract: &HirContract) {
    line(out, 2, "contract pre");
    render_expr(out, 3, &contract.pre);
    line(out, 2, "contract post");
    render_expr(out, 3, &contract.post);
    if let Some(wd) = &contract.watchdog {
        line(out, 2, &format!("watchdog required={} on_fire={:?}", wd.is_required, wd.on_fire));
        render_expr(out, 3, &wd.condition);
        if let Some(fallback) = &wd.fallback {
            render_expr(out, 3, fallback);
        }
    }
}

fn render_body(out: &mut String, level: usize, stmts: &[HirStmt]) {
    for stmt in stmts {
        render_stmt(out, level, stmt);
    }
}

fn render_stmt(out: &mut String, level: usize, stmt: &HirStmt) {
    match &stmt.kind {
        HirStmtKind::Let { name, names, ty, expr, modifiers } => {
            let mods: Vec<&str> = modifiers.iter().map(|m| m.name.as_str()).collect();
            line(out, level, &format!("let#{:?} {name} names={names:?} ty={ty:?} mods={mods:?}", stmt.id));
            if let Some(expr) = expr {
                render_expr(out, level + 1, expr);
            }
        }
        HirStmtKind::Assign(lhs, rhs) => {
            line(out, level, &format!("assign#{:?}", stmt.id));
            render_expr(out, level + 1, lhs);
            render_expr(out, level + 1, rhs);
        }
        HirStmtKind::ArrowAssign { target, value, consume } => {
            line(out, level, &format!("arrow-assign#{:?} consume={consume}", stmt.id));
            if let Some(target) = target {
                render_expr(out, level + 1, target);
            }
            render_expr(out, level + 1, value);
        }
        HirStmtKind::Term(expr) | HirStmtKind::EndProgram(expr) | HirStmtKind::Rollback(expr) => {
            line(out, level, &format!("{}#{:?}", stmt_tag(&stmt.kind), stmt.id));
            if let Some(expr) = expr {
                render_expr(out, level + 1, expr);
            }
        }
        HirStmtKind::Tail(expr)
        | HirStmtKind::Check(expr)
        | HirStmtKind::Expression(expr)
        | HirStmtKind::Gate(expr) => {
            line(out, level, &format!("{}#{:?}", stmt_tag(&stmt.kind), stmt.id));
            render_expr(out, level + 1, expr);
        }
        HirStmtKind::Guarded(cond, body) => {
            line(out, level, &format!("guarded#{:?}", stmt.id));
            render_expr(out, level + 1, cond);
            render_body(out, level + 1, body);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => {
            line(out, level, &format!("{}#{:?}", stmt_tag(&stmt.kind), stmt.id));
            render_body(out, level + 1, body);
        }
        HirStmtKind::Foreach { item, list, body, modifiers } => {
            line(out, level, &format!("foreach#{:?} {item} mods={modifiers:?}", stmt.id));
            render_expr(out, level + 1, list);
            render_body(out, level + 1, body);
        }
        HirStmtKind::Match { expr, arms } => {
            line(out, level, &format!("match-stmt#{:?}", stmt.id));
            render_expr(out, level + 1, expr);
            for arm in arms {
                render_stmt_arm(out, level + 1, arm);
            }
        }
        HirStmtKind::Open(a, b) => {
            line(out, level, &format!("open#{:?}", stmt.id));
            render_expr(out, level + 1, a);
            render_expr(out, level + 1, b);
        }
        HirStmtKind::TrgBinding { name, instance } => {
            line(out, level, &format!("trg-binding#{:?} {name}", stmt.id));
            render_expr(out, level + 1, instance);
        }
        HirStmtKind::MetadataAssignment(key, value) => {
            line(out, level, &format!("metadata#{:?} {key} = {value:?}", stmt.id));
        }
        other => line(out, level, &format!("{}#{:?}", stmt_tag(other), stmt.id)),
    }
}

fn stmt_tag(kind: &HirStmtKind) -> &'static str {
    match kind {
        HirStmtKind::Let { .. } => "let",
        HirStmtKind::Assign(..) => "assign",
        HirStmtKind::ArrowAssign { .. } => "arrow-assign",
        HirStmtKind::Term(_) => "term",
        HirStmtKind::Tail(_) => "tail",
        HirStmtKind::Check(_) => "check",
        HirStmtKind::Expression(_) => "expression",
        HirStmtKind::Guarded(..) => "guarded",
        HirStmtKind::Block(_) => "block",
        HirStmtKind::Foreach { .. } => "foreach",
        HirStmtKind::Match { .. } => "match-stmt",
        HirStmtKind::Break => "break",
        HirStmtKind::Trap => "trap",
        HirStmtKind::Halt => "halt",
        HirStmtKind::Yield => "yield",
        HirStmtKind::EndProgram(_) => "endprogram",
        HirStmtKind::Gate(_) => "gate",
        HirStmtKind::Rollback(_) => "rollback",
        HirStmtKind::FreeHint(_) => "free-hint",
        HirStmtKind::KeepHint(_) => "keep-hint",
        HirStmtKind::SyncBlock(_) => "sync-block",
        HirStmtKind::Defer(_) => "defer",
        HirStmtKind::Mutex(_) => "mutex",
        HirStmtKind::Open(..) => "open",
        HirStmtKind::TrgBinding { .. } => "trg-binding",
        HirStmtKind::MetadataAssignment(..) => "metadata",
        HirStmtKind::InlineAsm { .. } => "inline-asm",
        HirStmtKind::Other(tag) => tag,
    }
}

fn render_stmt_arm(out: &mut String, level: usize, arm: &HirStmtMatchArm) {
    line(out, level, "arm");
    for pattern in &arm.patterns {
        render_pattern(out, level + 1, pattern);
    }
    render_body(out, level + 1, &arm.body);
}

fn render_pattern(out: &mut String, level: usize, pattern: &HirPattern) {
    match &pattern.kind {
        HirPatternKind::Literal(expr) | HirPatternKind::Range(expr, _) | HirPatternKind::RangeInclusive(expr, _) => {
            line(out, level, &format!("pattern#{:?} {:?}", pattern.id, pattern_tag(&pattern.kind)));
            render_expr(out, level + 1, expr);
        }
        HirPatternKind::EnumVariant(name, pats) => {
            line(out, level, &format!("pattern#{:?} variant {name}", pattern.id));
            for pat in pats {
                render_pattern(out, level + 1, pat);
            }
        }
        HirPatternKind::Tuple(pats) | HirPatternKind::Multi(pats) => {
            line(out, level, &format!("pattern#{:?} {:?}", pattern.id, pattern_tag(&pattern.kind)));
            for pat in pats {
                render_pattern(out, level + 1, pat);
            }
        }
        other => line(out, level, &format!("pattern#{:?} {:?}", pattern.id, pattern_tag(other))),
    }
}

fn pattern_tag(kind: &HirPatternKind) -> &'static str {
    match kind {
        HirPatternKind::Wildcard => "wildcard",
        HirPatternKind::Literal(_) => "literal",
        HirPatternKind::Binding(_) => "binding",
        HirPatternKind::TypedBinding(..) => "typed-binding",
        HirPatternKind::EnumVariant(..) => "variant",
        HirPatternKind::Tuple(_) => "tuple",
        HirPatternKind::Range(..) => "range",
        HirPatternKind::RangeInclusive(..) => "range-inclusive",
        HirPatternKind::Multi(_) => "multi",
    }
}

fn render_expr(out: &mut String, level: usize, expr: &HirExpr) {
    let ty = expr.ty.as_ref().map(|t| format!("{t:?}")).unwrap_or_else(|| "-".to_string());
    match &expr.kind {
        HirExprKind::Literal(lit) => {
            line(out, level, &format!("expr#{:?} ty={ty} lit {}", expr.id, literal_tag(lit)));
        }
        HirExprKind::Identifier(name) => {
            line(out, level, &format!("expr#{:?} ty={ty} ident {name}", expr.id));
        }
        HirExprKind::Call { callee, args, analysis_id } => {
            line(out, level, &format!("expr#{:?} ty={ty} call {callee} analysis={analysis_id:?}", expr.id));
            for arg in args {
                render_expr(out, level + 1, arg);
            }
        }
        HirExprKind::Binary { op, left, right } => {
            line(out, level, &format!("expr#{:?} ty={ty} binary {op:?}", expr.id));
            render_expr(out, level + 1, left);
            render_expr(out, level + 1, right);
        }
        HirExprKind::Unary { op, inner } => {
            line(out, level, &format!("expr#{:?} ty={ty} unary {op:?}", expr.id));
            render_expr(out, level + 1, inner);
        }
        HirExprKind::Field { receiver, name } => {
            line(out, level, &format!("expr#{:?} ty={ty} field {name}", expr.id));
            render_expr(out, level + 1, receiver);
        }
        HirExprKind::Cast { inner, to, via } => {
            line(out, level, &format!("expr#{:?} ty={ty} cast to={to:?} via={via:?}", expr.id));
            render_expr(out, level + 1, inner);
        }
        _ => render_expr_structural(out, level, expr, &ty),
    }
}

fn render_expr_structural(out: &mut String, level: usize, expr: &HirExpr, ty: &str) {
    match &expr.kind {
        HirExprKind::MethodCall { receiver, name, args, analysis_id } => {
            line(out, level, &format!("expr#{:?} ty={ty} method {name} analysis={analysis_id:?}", expr.id));
            render_expr(out, level + 1, receiver);
            for arg in args {
                render_expr(out, level + 1, arg);
            }
        }
        HirExprKind::Reflect { receiver, name, kind } => {
            line(out, level, &format!("expr#{:?} ty={ty} reflect {name} {kind:?}", expr.id));
            render_expr(out, level + 1, receiver);
        }
        HirExprKind::Index(a, b) => {
            line(out, level, &format!("expr#{:?} ty={ty} index", expr.id));
            render_expr(out, level + 1, a);
            render_expr(out, level + 1, b);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            line(out, level, &format!("expr#{:?} ty={ty} slice", expr.id));
            render_expr(out, level + 1, array);
            for opt in [start, end, stride].into_iter().flatten() {
                render_expr(out, level + 1, opt);
            }
        }
        HirExprKind::Range { start, end, inclusive } => {
            line(out, level, &format!("expr#{:?} ty={ty} range inclusive={inclusive}", expr.id));
            render_expr(out, level + 1, start);
            render_expr(out, level + 1, end);
        }
        HirExprKind::Spawn { type_name, args, storage } => {
            line(out, level, &format!("expr#{:?} ty={ty} spawn {type_name} storage={storage:?}", expr.id));
            for arg in args {
                render_expr(out, level + 1, arg);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            line(out, level, &format!("expr#{:?} ty={ty} if", expr.id));
            render_expr(out, level + 1, condition);
            render_expr(out, level + 1, then_branch);
            if let Some(otherwise) = else_branch {
                render_expr(out, level + 1, otherwise);
            }
        }
        HirExprKind::Lambda { params, body } => {
            line(out, level, &format!("expr#{:?} ty={ty} lambda params={params:?}", expr.id));
            render_expr(out, level + 1, body);
        }
        HirExprKind::IsType { inner, ty: tested } => {
            line(out, level, &format!("expr#{:?} ty={ty} is-type {tested:?}", expr.id));
            render_expr(out, level + 1, inner);
        }
        HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => {
            line(out, level, &format!("expr#{:?} ty={ty} {}", expr.id, expr_tag(&expr.kind)));
            render_expr(out, level + 1, inner);
        }
        HirExprKind::UnitLiteral { value, unit } => {
            line(out, level, &format!("expr#{:?} ty={ty} unit {value} {unit}", expr.id));
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            line(out, level, &format!("expr#{:?} ty={ty} {}", expr.id, expr_tag(&expr.kind)));
            for item in items {
                render_expr(out, level + 1, item);
            }
        }
        HirExprKind::PluginIntercept { name, args, type_args, receiver } => {
            line(out, level, &format!("expr#{:?} ty={ty} plugin {name} type_args={type_args:?}", expr.id));
            if let Some(receiver) = receiver {
                render_expr(out, level + 1, receiver);
            }
            for arg in args {
                render_expr(out, level + 1, arg);
            }
        }
        HirExprKind::StructLiteral { type_name, fields, specs } => {
            line(out, level, &format!("expr#{:?} ty={ty} struct {type_name} specs={}", expr.id, specs.len()));
            for (name, value) in fields.iter().chain(specs.iter()) {
                line(out, level + 1, &format!("field {name}"));
                render_expr(out, level + 2, value);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            line(out, level, &format!("expr#{:?} ty={ty} match", expr.id));
            render_expr(out, level + 1, scrutinee);
            for arm in arms {
                render_expr_arm(out, level + 1, arm);
            }
        }
        HirExprKind::Block(body) => {
            line(out, level, &format!("expr#{:?} ty={ty} block", expr.id));
            render_body(out, level + 1, body);
        }
        _ => line(out, level, &format!("expr#{:?} ty={ty} other", expr.id)),
    }
}

fn render_expr_arm(out: &mut String, level: usize, arm: &HirMatchArm) {
    line(out, level, "expr-arm");
    render_pattern(out, level + 1, &arm.pattern);
    if let Some(guard) = &arm.guard {
        render_expr(out, level + 1, guard);
    }
    render_expr(out, level + 1, &arm.body);
}

fn expr_tag(kind: &HirExprKind) -> &'static str {
    match kind {
        HirExprKind::Deref(_) => "deref",
        HirExprKind::AddrOf(_) => "addr-of",
        HirExprKind::Consume(_) => "consume",
        HirExprKind::Await(_) => "await",
        HirExprKind::Tuple(_) => "tuple",
        HirExprKind::List(_) => "list",
        _ => "expr",
    }
}

fn literal_tag(lit: &HirLiteral) -> String {
    match lit {
        HirLiteral::Bytes(bytes) => format!("bytes({})", bytes.len()),
        HirLiteral::TaggedBytes(bytes, tag) => format!("tagged-bytes({tag},{})", bytes.len()),
        other => format!("{other:?}"),
    }
}

fn line(out: &mut String, level: usize, text: &str) {
    let _ = writeln!(out, "{}{}", "  ".repeat(level), text);
}
