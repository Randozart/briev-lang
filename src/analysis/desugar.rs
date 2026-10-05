// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2D/3D index desugar (2026-09-17, plan 2026-09-17-row-2d-index-desugar).
//!
//! `a[i, j]` on a shape-bearing array field parses to the internal marker
//! `__briev_multiindex__(a, i, j)` (see parser/expressions.rs, the simple-
//! index path). This pass rewrites the marker to plain row-major 1D
//! arithmetic — `a[i * C + j]`, C from the field type's `spec Cols` via
//! `TypeUniverse::matrix_shape` — BEFORE typecheck/analysis, so the accel
//! shape detectors and every backend keep matching the plain `Expr::Index`
//! forms they already understand. Zero new backend match arms.
//!
//! v1 surface (documented): state fields + top-level typed lets as bases;
//! statement forms Let/Assign/ArrowAssign/Guarded/Gate/Block/Foreach/
//! Expression/Term/EndProgram/Rollback/SyncBlock. A marker that escapes
//! (unknown base, non-shape type, arity mismatch) is a compile error —
//! fail closed, never a silent miscompile.

use crate::ast::Expr;
use crate::ast::top::{Statement, TopLevel};
use crate::ast::Type;
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

/// The internal marker name (parser + this pass must agree).
pub const MULTIINDEX_MARKER: &str = "__briev_multiindex__";

/// Rewrite every multi-index marker in `items` to 1D row-major arithmetic.
pub fn rewrite_multi_index(items: &mut Vec<TopLevel>) -> Result<(), String> {
    // Shape sources: the shared type registration (same the normalizers
    // call) on a LOCAL universe, plus the field table.
    let mut universe = TypeUniverse::new();
    crate::backend::register_types::register_typedefs(items, &mut universe, 64)?;
    let mut fields: HashMap<String, Type> = HashMap::new();
    for item in items.iter() {
        match item {
            TopLevel::StateDecl(s) => {
                fields.insert(s.name.clone(), s.ty.clone());
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, ty: Some(t), .. } = stmt.as_ref() {
                    fields.insert(name.clone(), t.clone());
                }
            }
            _ => {}
        }
    }

    for item in items.iter_mut() {
        rewrite_item(item, &universe, &fields)?;
    }
    Ok(())
}

/// Rewrite one top-level item's statement bodies. `async node` parses to
/// Transaction (parser/definitions.rs:917) — nodes need no separate arm.
fn rewrite_item(
    item: &mut TopLevel,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<(), String> {
    match item {
        TopLevel::Statement(s) => rewrite_stmt(s, universe, fields)?,
        TopLevel::Definition(d) | TopLevel::TypeDefOperator(d) => {
            rewrite_body(&mut d.body, universe, fields)?;
        }
        TopLevel::Transaction(t) => {
            rewrite_body(&mut t.body, universe, fields)?;
        }
        _ => {}
    }
    Ok(())
}

fn rewrite_body(
    body: &mut [Statement],
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<(), String> {
    for s in body.iter_mut() {
        rewrite_stmt(s, universe, fields)?;
    }
    Ok(())
}

fn shape_of(universe: &TypeUniverse, ty: &Type) -> Option<(u64, u64, u64)> {
    universe.matrix_shape(ty)
}

/// Fold `idxs` row-major over `shape` and build the 1D index expression.
/// 2D: `i0 * cols + i1`; 3D: `(i0 * cols + i1) * depth + i2`.
fn fold_indices(
    base: &Expr,
    idxs: &[Expr],
    shape: (u64, u64, u64),
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<Expr, String> {
    let (rows, cols, depth) = shape;
    let dims: Vec<u64> = match idxs.len() {
        2 => vec![cols],
        3 if depth > 1 => vec![cols, depth],
        _ => {
            return Err(format!(
                "a[{}, {}] on a shape-bearing field needs 2 indices (rows = {rows}), or 3 when the type declares spec Depth > 1 (depth = {depth})",
                idxs.len(),
                idxs.len(),
            ))
        }
    };
    let _ = rows; // rows bound the first index; not needed in the linear form
    let mut it = idxs.iter();
    let first = rewrite_marker_in(it.next().unwrap().clone(), universe, fields)?;
    let mut acc = first;
    for (dim, e) in dims.iter().zip(idxs[1..].iter()) {
        let e: Expr = rewrite_marker_in(e.clone(), universe, fields)?;
        // acc = acc * dim + e  (Int arithmetic — index space is Int)
        let dim_e = Expr::Decimal(*dim as i64);
        acc = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(Expr::BinaryOp(
                crate::ast::BinaryOpKind::Mul,
                Box::new(acc),
                Box::new(dim_e),
            )),
            Box::new(e),
        );
    }
    Ok(Expr::Index(Box::new(base.clone()), Box::new(acc)))
}

fn rewrite_marker_in(
    e: Expr,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<Expr, String> {
    // A marker nested inside an index expression (a[i, b[j, k]]) — recurse
    // by rebuilding through a one-element statement walk.
    let mut e = e;
    rewrite_expr(&mut e, universe, fields)?;
    Ok(e)
}

fn rewrite_stmt(
    stmt: &mut Statement,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<(), String> {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                rewrite_expr(e, universe, fields)?;
            }
        }
        Statement::Assign(lhs, rhs) => {
            rewrite_expr(lhs, universe, fields)?;
            rewrite_expr(rhs, universe, fields)?;
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rewrite_expr(t, universe, fields)?;
            }
            rewrite_expr(value, universe, fields)?;
        }
        Statement::Guarded(cond, body) => {
            rewrite_expr(cond, universe, fields)?;
            for s in body.iter_mut() {
                rewrite_stmt(s, universe, fields)?;
            }
        }
        Statement::Gate(cond) => rewrite_expr(cond, universe, fields)?,
        Statement::Block(body) | Statement::SyncBlock(body) => {
            for s in body.iter_mut() {
                rewrite_stmt(s, universe, fields)?;
            }
        }
        Statement::Foreach { body, .. } => {
            for s in body.iter_mut() {
                rewrite_stmt(s, universe, fields)?;
            }
        }
        Statement::Expression(e) => rewrite_expr(e, universe, fields)?,
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Rollback(Some(e)) => {
            rewrite_expr(e, universe, fields)?
        }
        _ => {}
    }
    Ok(())
}

fn rewrite_expr(
    e: &mut Expr,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<(), String> {
    // Rewrite nested markers FIRST so a[i, b[j, k]] resolves inner-most.
    match e {
        Expr::BinaryOp(_, l, r) => {
            rewrite_expr(l, universe, fields)?;
            rewrite_expr(r, universe, fields)?;
        }
        Expr::UnaryOp(_, i) => rewrite_expr(i, universe, fields)?,
        Expr::Field(o, _) => rewrite_expr(o, universe, fields)?,
        Expr::MethodCall(o, _, args, _, _) => {
            rewrite_expr(o, universe, fields)?;
            for a in args.iter_mut() {
                rewrite_expr(a, universe, fields)?;
            }
        }
        Expr::Index(o, i) => {
            rewrite_expr(o, universe, fields)?;
            rewrite_expr(i, universe, fields)?;
        }
        Expr::Call(name, args, _) => {
            for a in args.iter_mut() {
                rewrite_expr(a, universe, fields)?;
            }
            if name == MULTIINDEX_MARKER {
                if args.is_empty() {
                    return Err(format!("{MULTIINDEX_MARKER} needs (base, i0, i1, ...)"));
                }
                let Expr::Identifier(field) = &args[0] else {
                    return Err(format!(
                        "{MULTIINDEX_MARKER}: the base must be a state field name (v1 surface)"
                    ));
                };
                let Some(ty) = fields.get(field) else {
                    return Err(format!(
                        "{MULTIINDEX_MARKER}: '{}' is not a state field or typed let",
                        field
                    ));
                };
                let Some(shape) = shape_of(universe, ty) else {
                    return Err(format!(
                        "'{}' does not carry a shape — declare it as a shape-bearing type (e.g. Matrix<Float, R, C>) to index it as {field}[i, j]",
                        field
                    ));
                };
                *e = fold_indices(&args[0].clone(), &args[1..], shape, universe, fields)?;
            }
        }
        Expr::List(es) | Expr::Tuple(es) => {
            for x in es.iter_mut() {
                rewrite_expr(x, universe, fields)?;
            }
        }
        Expr::AddrOf(i) | Expr::Deref(i) | Expr::Consume(i) | Expr::Await(i) | Expr::Within(i, _) => {
            rewrite_expr(i, universe, fields)?
        }
        Expr::If(c, t, f) => {
            rewrite_expr(c, universe, fields)?;
            rewrite_expr(t, universe, fields)?;
            if let Some(f) = f {
                rewrite_expr(f, universe, fields)?;
            }
        }
        Expr::Cast(x, _) | Expr::IsType(x, _) => rewrite_expr(x, universe, fields)?,
        _ => {}
    }
    Ok(())
}


// ── Wildcard element lift (2026-10-05, three-surfaces plan Phase 0.6a) ──
//
// `[*]` selects EVERY element of a fixed-size array in an expression
// (SPEC §15). The plan's decided default: an AST-level desugar to an
// explicit lift — the whole statement is replicated once per element in
// declaration (row-major, E1) order with the wildcard index in scope — so
// the interpreter and every backend see only plain `Expr::Index` forms
// they already lower. ONE implementation, zero new backend arms (the
// multi-index desugar above is the precedent and the architecture).
//
// v1 surface (documented, fail closed):
//   - `d = <expr over X[*]>` — d a declared ARRAY let/state of the same
//     extent; the rhs lifts element-wise (all wildcards in the statement
//     share one index; mixed extents are a hard error, the electronics
//     rule);
//   - `X[*] = <expr>` — element-wise store; a wildcard-free rhs is the
//     broadcast form (the E3 rail-to-bank analog);
//   - `let d: T[N] = <expr over Y[*]>;` — split into the declaration and
//     the lifted assign.
// A wildcard anywhere else in these bodies (bare expression statements,
// term values, nested wildcard levels, non-array or runtime-length bases,
// extents above 1024) is a compile error naming the fix — never a silent
// miscompile. Wildcards in CONTRACT halves are left untouched: the
// electronics netlist consumes them as wiring topology (SPEC §13), a
// different semantic domain than the value lift.

/// The unroll bound: a lift replicates its statement once per element, so
/// the extent must stay bounded. 1024 matches the largest state arrays in
/// the corpus with headroom; raise with evidence, never silently.
const WILDCARD_LIFT_MAX_EXTENT: u64 = 1024;

/// Rewrite every `[*]` wildcard in statement bodies to the explicit lift.
pub fn rewrite_wildcard_lift(items: &mut Vec<TopLevel>) -> Result<(), String> {
    // Extent sources: the shared type registration on a LOCAL universe
    // (same the multi-index pass builds) plus every typed let/state field.
    let mut universe = TypeUniverse::new();
    crate::backend::register_types::register_typedefs(items, &mut universe, 64)?;
    let mut fields: HashMap<String, Type> = HashMap::new();
    let mut counter = 0usize;
    for item in items.iter() {
        match item {
            TopLevel::StateDecl(st) => {
                fields.insert(st.name.clone(), st.ty.clone());
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, ty: Some(t), .. } = stmt.as_ref() {
                    fields.insert(name.clone(), t.clone());
                }
            }
            TopLevel::Transaction(t) => collect_let_types(&t.body, &mut fields),
            TopLevel::Definition(d) | TopLevel::TypeDefOperator(d) => {
                collect_let_types(&d.body, &mut fields);
            }
            _ => {}
        }
    }
    for item in items.iter_mut() {
        match item {
            TopLevel::Statement(stmt) => {
                let mut out = Vec::new();
                expand_stmt(stmt, &universe, &fields, &mut counter, &mut out)?;
                *stmt = Box::new(join_block(out));
            }
            TopLevel::Transaction(t) => {
                expand_body(&mut t.body, &universe, &fields, &mut counter)?;
            }
            TopLevel::Definition(d) | TopLevel::TypeDefOperator(d) => {
                expand_body(&mut d.body, &universe, &fields, &mut counter)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Typed lets feed the extent lookup — walk a body collecting them BEFORE
/// any rewrite (a lift may reference a let declared later in the body).
fn collect_let_types(body: &[Statement], fields: &mut HashMap<String, Type>) {
    for st in body {
        if let Statement::Let { name, ty: Some(t), .. } = st {
            fields.insert(name.clone(), t.clone());
        }
    }
}

fn expand_body(
    body: &mut Vec<Statement>,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
    counter: &mut usize,
) -> Result<(), String> {
    let mut idx = 0;
    while idx < body.len() {
        let mut out = Vec::new();
        expand_stmt(&mut body[idx], universe, fields, counter, &mut out)?;
        let replace = out.len() != 1;
        if replace {
            let tail = body.split_off(idx + 1);
            body.truncate(idx);
            body.extend(out);
            body.extend(tail);
        }
        idx += 1;
    }
    Ok(())
}

/// Expand one statement into zero-or-more plain statements. `out` receives
/// the expansion (usually the statement itself, possibly rewritten).
fn expand_stmt(
    stmt: &mut Statement,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
    counter: &mut usize,
    out: &mut Vec<Statement>,
) -> Result<(), String> {
    // `let d: T[N] = <init>;` with a wildcard init becomes a declaration
    // plus an assign, so the single assign path serves both forms.
    if let Statement::Let { name, names, ty: Some(ty), expr: Some(init), modifiers } = stmt {
        if expr_has_wildcard(init) {
            let decl = Statement::Let {
                name: name.clone(),
                names: names.clone(),
                ty: Some(ty.clone()),
                expr: None,
                modifiers: modifiers.clone(),
            };
            let assign = Statement::Assign(Expr::Identifier(name.clone()), init.clone());
            out.push(decl);
            let mut a = assign;
            expand_stmt(&mut a, universe, fields, counter, out)?;
            return Ok(());
        }
    }
    match stmt {
        Statement::Assign(lhs, rhs) => {
            expand_assign(lhs, rhs, universe, fields, counter, out)
        }
        Statement::Guarded(cond, body) => {
            if expr_has_wildcard(cond) {
                return Err(
                    "a `[*]` wildcard cannot lift a guard condition - select elements with a bounded range `[lo..hi]` instead".to_string(),
                );
            }
            expand_body(body, universe, fields, counter)?;
            out.push(stmt.clone());
            Ok(())
        }
        Statement::Block(body) | Statement::SyncBlock(body) => {
            expand_body(body, universe, fields, counter)?;
            out.push(stmt.clone());
            Ok(())
        }
        Statement::Foreach { body, .. } => {
            expand_body(body, universe, fields, counter)?;
            out.push(stmt.clone());
            Ok(())
        }
        Statement::Expression(_)
        | Statement::Term(_)
        | Statement::EndProgram(_)
        | Statement::Rollback(_)
        | Statement::Let { .. } => {
            if stmt_has_wildcard(stmt) {
                return Err(
                    "a `[*]` wildcard lift produces an array - assign it to a declared array of the same extent (`d = a[*] + b;`) or store element-wise (`d = a[*];`)".to_string(),
                );
            }
            out.push(stmt.clone());
            Ok(())
        }
        _ => {
            if stmt_has_wildcard(stmt) {
                return Err(
                    "a `[*]` wildcard in this statement form has no lift - state the element-wise form explicitly".to_string(),
                );
            }
            out.push(stmt.clone());
            Ok(())
        }
    }
}

/// The assign forms: lifted rhs into an array lhs, or an element-wise /
/// broadcast store through a wildcard lhs.
fn expand_assign(
    lhs: &mut Expr,
    rhs: &mut Expr,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
    counter: &mut usize,
    out: &mut Vec<Statement>,
) -> Result<(), String> {
    let lhs_wild = as_wildcard_base(lhs);
    let rhs_wilds = wildcard_bases(rhs);
    if lhs_wild.is_none() && rhs_wilds.is_empty() {
        out.push(Statement::Assign(lhs.clone(), rhs.clone()));
        return Ok(());
    }
    // Resolve every wildcard base to (elem, extent); all must agree on the
    // extent (the element-wise rule) and stay within the unroll bound.
    let mut extent: Option<u64> = None;
    let mut bases = Vec::new();
    for name in lhs_wild.iter().chain(rhs_wilds.iter()) {
        let (elem, n) = array_extent(name, universe, fields)?;
        if let Some(prev) = extent {
            if prev != n {
                return Err(format!(
                    "wildcard lift length mismatch: '{name}' ranges {n} elements but the statement also ranges {prev} - the two sides must agree element-wise"
                ));
            }
        }
        extent = Some(n);
        bases.push((name.clone(), elem));
    }
    let n = extent.unwrap();
    // The lhs root (wildcard base, or the declared destination array): the
    // lift STORES element-wise into `root[i]`.
    let root = match &lhs_wild {
        Some(name) => name.clone(),
        None => {
            let Expr::Identifier(dname) = lhs else {
                return Err(
                    "the destination of a `[*]` lift must be a declared array (v1 surface)".to_string(),
                );
            };
            let (elem, dn) = array_extent(dname, universe, fields)?;
            if dn != n {
                return Err(format!(
                    "wildcard lift length mismatch: destination '{dname}' holds {dn} elements but the rhs ranges {n} - the extents must agree"
                ));
            }
            let _ = elem;
            dname.clone()
        }
    };
    for i in 0..n {
        let idx = Expr::Decimal(i as i64);
        let new_lhs = Expr::Index(
            Box::new(Expr::Identifier(root.clone())),
            Box::new(idx.clone()),
        );
        let mut new_rhs = rhs.clone();
        substitute_wildcards(&mut new_rhs, &bases, &idx);
        out.push(Statement::Assign(new_lhs, new_rhs));
    }
    Ok(())
}

/// `X[*]` at the ROOT of an assignment target.
fn as_wildcard_base(e: &Expr) -> Option<String> {
    match e {
        Expr::Index(base, idx) => {
            if matches!(idx.as_ref(), Expr::Wildcard) {
                if let Expr::Identifier(name) = base.as_ref() {
                    return Some(name.clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// Every `X[*]` base in the expression, in source order (duplicates
/// included — one base may range several positions).
fn wildcard_bases(e: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    walk_wildcards(e, &mut out);
    out
}

fn walk_wildcards(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Index(base, idx) => {
            walk_wildcards(base, out);
            if matches!(idx.as_ref(), Expr::Wildcard) {
                if let Expr::Identifier(name) = base.as_ref() {
                    out.push(name.clone());
                }
            }
        }
        Expr::BinaryOp(_, l, r) => {
            walk_wildcards(l, out);
            walk_wildcards(r, out);
        }
        Expr::UnaryOp(_, i) => walk_wildcards(i, out),
        Expr::Field(o, _) => walk_wildcards(o, out),
        Expr::Call(_, args, _) => {
            for a in args {
                walk_wildcards(a, out);
            }
        }
        Expr::MethodCall(o, _, args, _, _) => {
            walk_wildcards(o, out);
            for a in args {
                walk_wildcards(a, out);
            }
        }
        Expr::Cast(x, _) | Expr::IsType(x, _) => walk_wildcards(x, out),
        _ => {}
    }
}

fn expr_has_wildcard(e: &Expr) -> bool {
    !wildcard_bases(e).is_empty()
}

fn stmt_has_wildcard(stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign(l, r) => expr_has_wildcard(l) || expr_has_wildcard(r),
        Statement::Let { expr: Some(e), .. } => expr_has_wildcard(e),
        Statement::Expression(e) | Statement::Term(Some(e))
        | Statement::EndProgram(Some(e)) | Statement::Rollback(Some(e)) => {
            expr_has_wildcard(e)
        }
        _ => false,
    }
}

/// Replace every `X[*]` in `e` with `X[idx]` — the SAME index for every
/// base (the element-wise pairing rule).
fn substitute_wildcards(e: &mut Expr, bases: &[(String, Type)], idx: &Expr) {
    match e {
        Expr::Index(base, i) => {
            if matches!(i.as_ref(), Expr::Wildcard) {
                if let Expr::Identifier(name) = base.as_ref() {
                    if bases.iter().any(|(b, _)| b == name) {
                        *e = Expr::Index(
                            Box::new(Expr::Identifier(name.clone())),
                            Box::new(idx.clone()),
                        );
                        return;
                    }
                }
            }
            substitute_wildcards(base, bases, idx);
            substitute_wildcards(i, bases, idx);
        }
        Expr::BinaryOp(_, l, r) => {
            substitute_wildcards(l, bases, idx);
            substitute_wildcards(r, bases, idx);
        }
        Expr::UnaryOp(_, i) => substitute_wildcards(i, bases, idx),
        Expr::Field(o, f) => {
            substitute_wildcards(o, bases, idx);
            let _ = f;
        }
        Expr::Call(_, args, _) | Expr::List(args) | Expr::Tuple(args) => {
            for a in args.iter_mut() {
                substitute_wildcards(a, bases, idx);
            }
        }
        Expr::MethodCall(o, _, args, _, _) => {
            substitute_wildcards(o, bases, idx);
            for a in args.iter_mut() {
                substitute_wildcards(a, bases, idx);
            }
        }
        Expr::Cast(x, _) | Expr::IsType(x, _) => substitute_wildcards(x, bases, idx),
        _ => {}
    }
}

/// Extent and element type of a declared fixed-size array. A `T[N]` and a
/// `T[N][M]` both lift: the extent is the row-major element count (E1).
fn array_extent(
    name: &str,
    universe: &TypeUniverse,
    fields: &HashMap<String, Type>,
) -> Result<(Type, u64), String> {
    let Some(ty) = fields.get(name) else {
        return Err(format!(
            "wildcard lift base '{name}' is not a state field or typed let - declare it (`let {name}: T[N];`) to range it with `[*]`"
        ));
    };
    let Type::Vector(elem, dims) = ty else {
        return Err(format!(
            "wildcard lift base '{name}' is not a fixed-size array (type {ty}) - `[*]` ranges arrays with a compile-time extent"
        ));
    };
    let mut n: u64 = 1;
    for d in dims {
        // Dimensions are compile-time by construction (Anonymous(N) or a
        // named spec extent resolved at registration).
        let x = match d {
            crate::ast::Dimension::Anonymous(x) => *x as u64,
            crate::ast::Dimension::Named(_, x) => *x as u64,
        };
        n = n.saturating_mul(x);
    }
    if n == 0 {
        return Err(format!(
            "wildcard lift base '{name}' has a zero extent - nothing to range"
        ));
    }
    if n > WILDCARD_LIFT_MAX_EXTENT {
        return Err(format!(
            "wildcard lift base '{name}' ranges {n} elements - above the {WILDCARD_LIFT_MAX_EXTENT} unroll bound; state the loop explicitly"
        ));
    }
    Ok(((**elem).clone(), n))
}

/// Rebuild a single statement from an expansion (used for top-level
/// statements, which are boxed singles).
fn join_block(mut out: Vec<Statement>) -> Statement {
    if out.len() == 1 {
        return out.pop().unwrap();
    }
    Statement::Block(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    const TYPE_DECL: &str = "type Matrix<T, R, C> { spec Rows: R; spec Cols: C; };\nlet m: Matrix<Float, 4, 4>;\n";

    fn node_stmt(src: &str) -> Statement {
        // Parse `async node go [false][false] { <src> };` and take the
        // FIRST body statement (the node is a Transaction in the AST).
        let prog = parse(&format!(
            "{TYPE_DECL}async node go [false][false] {{ {} }};",
            src
        ));
        match &prog[2] {
            TopLevel::Transaction(t) => t.body[0].clone(),
            other => panic!("expected transaction, got {other:?}"),
        }
    }

    #[test]
    fn desugar_2d_equals_manual_row_major() {
        let mut sugar_prog = parse(&format!(
            "{TYPE_DECL}async node go [false][false] {{ m[2, 3] = 1.0; }};"
        ));
        rewrite_multi_index(&mut sugar_prog).unwrap();
        let TopLevel::Transaction(t) = &sugar_prog[2] else {
            panic!("expected transaction");
        };
        let manual = node_stmt("m[2 * 4 + 3] = 1.0;");
        assert_eq!(format!("{:?}", t.body[0]), format!("{:?}", manual));
    }

    #[test]
    fn desugar_2d_read_equals_manual() {
        let mut sugar_prog = parse(&format!(
            "{TYPE_DECL}async node go [false][false] {{ pick = m[1, 2]; }};"
        ));
        // `pick` must be a known field for the base lookup of the marker —
        // but here the MARKER is on m; the plain Assign rhs contains it.
        rewrite_multi_index(&mut sugar_prog).unwrap();
        let TopLevel::Transaction(t) = &sugar_prog[2] else {
            panic!("expected transaction");
        };
        let manual = node_stmt("pick = m[1 * 4 + 2];");
        assert_eq!(format!("{:?}", t.body[0]), format!("{:?}", manual));
    }

    // ── Wildcard lift (Phase 0.6a) ──────────────────────────────────

    fn lift_prog(body: &str) -> Vec<TopLevel> {
        parse(&format!(
            "let a: Int[4];\nlet b: Int[4];\nlet d: Int[4];\nlet s: Int;\nasync node go [false][false] {{ {} }};\n",
            body
        ))
    }

    fn lift_body(body: &str) -> Vec<Statement> {
        let mut prog = lift_prog(body);
        rewrite_wildcard_lift(&mut prog).unwrap();
        let TopLevel::Transaction(t) = &prog[4] else {
            panic!("expected transaction");
        };
        t.body.clone()
    }

    fn stmt_strs(stmts: &[Statement]) -> Vec<String> {
        stmts.iter().map(|s| format!("{s:?}")).collect()
    }

    /// The element-wise lift: one unrolled store per index, every wildcard
    /// sharing the SAME index, declaration (row-major) order.
    #[test]
    fn lift_element_wise_assign() {
        let out = lift_body("d = a[*] + b[*];");
        assert_eq!(out.len(), 4, "{:?}", stmt_strs(&out));
        let first = format!("{:?}", out[0]);
        assert!(first.contains("Index(Identifier(\"d\"), Decimal(0))"), "{first}");
        assert!(first.contains("Index(Identifier(\"a\"), Decimal(0))"), "{first}");
        assert!(first.contains("Index(Identifier(\"b\"), Decimal(0))"), "{first}");
        let last = format!("{:?}", out[3]);
        assert!(last.contains("Decimal(3)"), "{last}");
    }

    /// A wildcard-free rhs with a wildcard lhs is the broadcast form.
    #[test]
    fn lift_broadcast_store() {
        let out = lift_body("a[*] = s + 1;");
        assert_eq!(out.len(), 4);
        let first = format!("{:?}", out[0]);
        assert!(first.contains("Index(Identifier(\"a\"), Decimal(0))"), "{first}");
        assert!(first.contains("Identifier(\"s\")"), "{first}");
    }

    /// `let d: Int[4] = a[*] + s;` splits into declaration + lifted assign.
    #[test]
    fn lift_let_init_splits() {
        let mut prog = lift_prog("let d2: Int[4] = a[*] + s;");
        // d2 joins the field map: re-collect happens inside the pass; here
        // the let itself is IN the node body — the collect phase walks it.
        rewrite_wildcard_lift(&mut prog).unwrap();
        let TopLevel::Transaction(t) = &prog[4] else {
            panic!("expected transaction");
        };
        // decl + 4 stores
        assert_eq!(t.body.len(), 5, "{:?}", stmt_strs(&t.body));
        assert!(matches!(t.body[0], Statement::Let { expr: None, .. }));
    }

    /// Mixed extents are the hard error (the element-wise rule).
    #[test]
    fn lift_length_mismatch_is_an_error() {
        let mut prog = parse(
            "let a: Int[4];\nlet c: Int[8];\nlet d: Int[8];\nasync node go [false][false] { d = c[*] + a[*]; };",
        );
        let err = rewrite_wildcard_lift(&mut prog).unwrap_err();
        assert!(err.contains("length mismatch"), "got: {err}");
    }

    /// Unknown bases and runtime-length bases fail closed with the fix.
    #[test]
    fn lift_fails_closed_on_bad_bases() {
        let mut prog = lift_prog("d = unknown[*] + 1;");
        let err = rewrite_wildcard_lift(&mut prog).unwrap_err();
        assert!(err.contains("not a state field or typed let"), "got: {err}");

        let mut prog = parse(
            "let l: List<Int>;\nlet d: Int[2];\nasync node go [false][false] { d = l[*]; };",
        );
        let err = rewrite_wildcard_lift(&mut prog).unwrap_err();
        assert!(err.contains("not a fixed-size array"), "got: {err}");
    }

    /// A wildcard outside an assign form has no lift — the error names the
    /// element-wise form; contract halves stay untouched (electronics
    /// wiring).
    #[test]
    fn lift_scope_boundary() {
        let mut prog = lift_prog("pick(a[*]);");
        let err = rewrite_wildcard_lift(&mut prog).unwrap_err();
        assert!(err.contains("no lift") || err.contains("produces an array"), "got: {err}");

        // A contract wildcard survives the pass untouched.
        let mut prog = parse(
            "let t: Int[2];\nnode go [t[*] > 0][true] { term; };",
        );
        rewrite_wildcard_lift(&mut prog).unwrap();
        let TopLevel::Transaction(t) = &prog[1] else {
            panic!("expected transaction");
        };
        let pre = format!("{:?}", t.contract.pre_condition);
        assert!(pre.contains("Wildcard"), "contract wildcard must survive: {pre}");
    }

    #[test]
    fn plain_vector_multiindex_is_an_error() {
        let mut prog = parse("let v: Float[16];\nasync node go [false][false] { v[1, 2] = 3.0; };");
        let err = rewrite_multi_index(&mut prog).unwrap_err();
        assert!(err.contains("does not carry a shape"), "got: {err}");
    }

    #[test]
    fn marker_is_gone_after_desugar() {
        let mut prog = parse(&format!(
            "{TYPE_DECL}async node go [false][false] {{ m[0, 1] = 2.0; }};"
        ));
        rewrite_multi_index(&mut prog).unwrap();
        let dump = format!("{:?}", prog);
        assert!(!dump.contains(MULTIINDEX_MARKER));
    }
}
