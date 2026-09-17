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
