// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Supported terms carry an explicit sort; unsupported syntax has no encoding.
use crate::ast::{BinaryOpKind as Op, Expr, UnaryOpKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sort {
    Boolean,
    Word,
}

pub(super) struct Term {
    pub text: String,
    pub sort: Sort,
}

pub(super) fn encode(expr: &Expr) -> Option<Term> {
    encode_with_term_sort(expr, None)
}

/// Encode with an explicit sort for `term` (2026-11-15 phase 1b).
///
/// `term_sort` is `Some(Sort::Boolean)` for Bool-typed defns so that
/// `Expr::Identifier("term")` encodes as `Sort::Boolean` and the SMT query
/// declares it as `Bool`. `None` keeps the word model (BitVec 64).
pub(super) fn encode_with_term_sort(expr: &Expr, term_sort: Option<Sort>) -> Option<Term> {
    match expr {
        Expr::Bool(value) => Some(Term {
            text: if *value { "(= #b1 #b1)" } else { "(= #b0 #b1)" }.into(),
            sort: Sort::Boolean,
        }),
        Expr::Decimal(value) => Some(Term {
            text: format!("#x{:016x}", *value as u64),
            sort: Sort::Word,
        }),
        Expr::Identifier(name) => {
            let sort = if name == "term" && term_sort == Some(Sort::Boolean) {
                Sort::Boolean
            } else {
                Sort::Word
            };
            Some(Term {
                text: super::symbols::encode(name),
                sort,
            })
        }
        Expr::BinaryOp(op, left, right) => binary(
            *op,
            encode_with_term_sort(left, term_sort)?,
            encode_with_term_sort(right, term_sort)?,
        ),
        Expr::UnaryOp(UnaryOpKind::Not, inner) => {
            let inner = expect_with_term_sort(inner, Sort::Boolean, term_sort)?;
            Some(Term {
                text: format!("(not {inner})"),
                sort: Sort::Boolean,
            })
        }
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
            let inner = expect_with_term_sort(inner, Sort::Word, term_sort)?;
            Some(Term {
                text: format!("(bvneg {inner})"),
                sort: Sort::Word,
            })
        }
        _ => None,
    }
}

pub(super) fn expect(expr: &Expr, sort: Sort) -> Option<String> {
    expect_with_term_sort(expr, sort, None)
}

/// Expect a term of the given sort, with an explicit sort for `term`.
pub(super) fn expect_with_term_sort(
    expr: &Expr,
    sort: Sort,
    term_sort: Option<Sort>,
) -> Option<String> {
    let term = encode_with_term_sort(expr, term_sort)?;
    (term.sort == sort).then_some(term.text)
}

/// The standalone word AST also models directly tracked fields as words.
/// This rank check does not grant them an SMT encoding or source type admission.
pub(super) fn postcondition_sort(expr: &Expr) -> Option<Sort> {
    match expr {
        Expr::Field(base, _) if matches!(base.as_ref(), Expr::Identifier(_)) => Some(Sort::Word),
        Expr::BinaryOp(op, left, right) => {
            binary_sort(*op, postcondition_sort(left)?, postcondition_sort(right)?)
        }
        Expr::UnaryOp(UnaryOpKind::Not, inner) => {
            (postcondition_sort(inner)? == Sort::Boolean).then_some(Sort::Boolean)
        }
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
            (postcondition_sort(inner)? == Sort::Word).then_some(Sort::Word)
        }
        other => encode(other).map(|term| term.sort),
    }
}

fn binary_sort(op: Op, left: Sort, right: Sort) -> Option<Sort> {
    if left != right {
        return None;
    }
    if matches!(op, Op::Eq | Op::Neq) {
        return Some(Sort::Boolean);
    }
    let (_, arguments, result) = signature(op)?;
    (left == arguments).then_some(result)
}

fn binary(op: Op, left: Term, right: Term) -> Option<Term> {
    let sort = binary_sort(op, left.sort, right.sort)?;
    if matches!(op, Op::Eq | Op::Neq) {
        return Some(equality(left, right, op == Op::Neq));
    }
    let (symbol, _, _) = signature(op)?;
    Some(Term {
        text: format!("({symbol} {} {})", left.text, right.text),
        sort,
    })
}

/// Equality supports either sort, while requiring both operands to agree.
fn equality(left: Term, right: Term, negated: bool) -> Term {
    let equal = format!("(= {} {})", left.text, right.text);
    Term {
        text: if negated {
            format!("(not {equal})")
        } else {
            equal
        },
        sort: Sort::Boolean,
    }
}

/// Operator spelling and rank are one table, shared by validation and emission.
fn signature(op: Op) -> Option<(&'static str, Sort, Sort)> {
    use Sort::{Boolean as B, Word as W};
    match op {
        Op::And => Some(("and", B, B)),
        Op::Or => Some(("or", B, B)),
        Op::Lt => Some(("bvslt", W, B)),
        Op::Le => Some(("bvsle", W, B)),
        Op::Gt => Some(("bvsgt", W, B)),
        Op::Ge => Some(("bvsge", W, B)),
        Op::Add => Some(("bvadd", W, W)),
        Op::Sub => Some(("bvsub", W, W)),
        Op::Mul => Some(("bvmul", W, W)),
        Op::Div => Some(("bvsdiv", W, W)),
        Op::Mod => Some(("bvsrem", W, W)),
        _ => None,
    }
}
