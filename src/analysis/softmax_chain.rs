//! Softmax chain detection (M3, plan 2026-09-20-metaprogrammed-composites).
//!
//! Detects the three-node chain — DOT producer → row-softmax middle →
//! linear-fold consumer — from topology and existing shape proofs. The
//! compiler does NOT synthesize the fused body: per the proof-vs-shape
//! doctrine (Golden Rule 24), the fused form is the EXPANDED declared
//! composite (`softmax_fused!`, Front B) bound to the detected pieces
//! (Front C). This module records the proven chain; the body arrives
//! from the language, never from Rust string-built AST.
//!
//! Proof sources: `detect_reduction` (Dot), `detect_row_softmax`
//! (Softmax), `detect_deferred_normalizer` (M2) — no algorithm or type
//! names are matched (rules 15/23); every match fails closed.

use crate::ast::*;
use crate::analysis::accel::{AccelDecision, AccelEntry, KernelShape, ReductionKind};
use crate::analysis::gpu_schedule::{EdgeKind, GpuSchedule};
use std::collections::HashMap;

/// A proven DOT → SOFTMAX → LINEAR-FOLD chain. Emission geometry: one
/// work item per softmax row (head). The fused body is bound later from
/// the declared composite (plan metaprogrammed-composites, Fronts B/C).
#[derive(Debug, Clone)]
pub struct SoftmaxChain {
    /// The dot node writing the score buffer.
    pub producer: String,
    /// The row-softmax node.
    pub middle: String,
    /// The linear-fold consumer node.
    pub consumer: String,
    /// Producer output == softmax row buffer (dead after fusion).
    pub score_buf: String,
    /// Softmax output == consumer fold input; REUSED as the fused
    /// accumulator scratch (its only reader/writer are both fused away).
    pub mid_out: String,
    /// The consumer's terminal output buffer.
    pub out_buf: String,
    // 2026-09-20 (plan metaprogrammed-composites): the fused body is NOT
    // synthesized in Rust. It will be the EXPANDED declared composite
    // (Front B) bound to the detected pieces (Front C). The detector
    // records the proven chain; the body arrives from the language.
    /// The fused kernel's work-item variable (the producer's head local).
    pub index_var: String,
    /// The fused kernel's count expression (the softmax row count).
    pub count_expr: Expr,
    /// Fused kernel buffer contracts (the scratch is written RMW; it is
    /// both a read and a write of the fused kernel).
    pub read_buffers: Vec<String>,
    pub write_buffers: Vec<String>,
}

/// The producer's proven shape: leading lets (head decomp + others), the
/// dot fold, and the scaled store expression.
struct ScoreProducer {
    head_local: String,
    j_local: String,
    other_lets: Vec<Statement>,
    dot: Statement,
    acc_local: String,
    store_rhs: Expr,
}

/// The consumer's proven shape: leading lets, the fold's non-softmax
/// operand, the d decomposition end, and the terminal store.
struct LinearConsumer {
    other_lets: Vec<Statement>,
    head_local: String,
    d_local: String,
    d_end: Expr,
    fold_other: Expr,
    out_buf: String,
}

/// Detect the chain over eligible accel entries + the schedule DAG.
/// Returns the first proven chain (deterministic: sorted middle names).
pub fn detect_softmax_chain(
    accel: &HashMap<String, AccelEntry>,
    sched: &GpuSchedule,
) -> Option<SoftmaxChain> {
    let mut middles: Vec<&String> = accel
        .iter()
        .filter(|(_, e)| e.decision != AccelDecision::Cpu && is_softmax_middle(&e.shape))
        .map(|(n, _)| n)
        .collect();
    middles.sort();
    for m in middles {
        let Some(chain) = try_middle(m, accel, sched) else {
            continue;
        };
        return Some(chain);
    }
    None
}

fn try_middle(
    m: &str,
    accel: &HashMap<String, AccelEntry>,
    sched: &GpuSchedule,
) -> Option<SoftmaxChain> {
    let me = accel.get(m)?;
    let red = me.shape.reduction.as_ref()?;
    let score_buf = red.row_buf.clone();
    let mid_out = red.out_buf.clone();
    // Score buffer flows producer → middle: the middle is its ONLY reader.
    let producer = single_reader_of(sched, &score_buf)?;
    if producer == m || !edge_exists(sched, &producer, m) {
        return None;
    }
    // Softmax output flows middle → consumer: single reader.
    let consumer = single_reader_of(sched, &mid_out)?;
    if consumer == m || consumer == producer || !edge_exists(sched, m, &consumer) {
        return None;
    }
    let pe = accel.get(&producer)?;
    let ce = accel.get(&consumer)?;
    if !pe.shape.eligible || !ce.shape.eligible {
        return None;
    }
    // Producer must be a Dot reduction (the score computation).
    if !matches!(&pe.shape.reduction, Some(r) if r.kind == ReductionKind::Dot) {
        return None;
    }
    let p = match_score_producer(&pe.shape.kernel_stmts, &pe.shape.index_var, &score_buf)?;
    let c = match_linear_consumer(&ce.shape.kernel_stmts, &ce.shape.index_var, &mid_out, &red.inner)?;
    // The consumer's terminal output must be an array nobody else reads.
    if !ce.shape.write_buffers.iter().any(|w| w == &c.out_buf) {
        return None;
    }
    if sched
        .reads
        .iter()
        .any(|(n, rs)| n != &consumer && rs.iter().any(|r| r == &c.out_buf))
    {
        return None;
    }
    let head_count = me.shape.count_expr.clone()?;
    let read_buffers = {
        let mut v: Vec<String> = pe
            .shape
            .read_buffers
            .iter()
            .chain(ce.shape.read_buffers.iter())
            .filter(|b| **b != c.out_buf)
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    };
    Some(SoftmaxChain {
        producer,
        middle: m.to_string(),
        consumer,
        score_buf,
        mid_out: mid_out.clone(),
        out_buf: c.out_buf.clone(),
        index_var: p.head_local,
        count_expr: head_count,
        read_buffers,
        write_buffers: vec![mid_out, c.out_buf],
    })
}

/// A softmax middle: the row-softmax shape PLUS the M2 deferral proof —
/// without the proof the normalize tail is not absorbable and the chain
/// stays three kernels.
fn is_softmax_middle(shape: &KernelShape) -> bool {
    shape.eligible
        && shape.deferred_normalize.is_some()
        && matches!(&shape.reduction, Some(r) if r.kind == ReductionKind::Softmax)
}

fn single_reader_of(sched: &GpuSchedule, field: &str) -> Option<String> {
    let readers: Vec<&String> = sched
        .reads
        .iter()
        .filter(|(_, rs)| rs.iter().any(|r| r == field))
        .map(|(n, _)| n)
        .collect();
    if readers.len() != 1 {
        return None;
    }
    Some(readers[0].clone())
}

fn edge_exists(sched: &GpuSchedule, from: &str, to: &str) -> bool {
    sched
        .edges
        .iter()
        .any(|(a, b, k)| a == from && b == to && *k == EdgeKind::Raw)
}

/// Match the score producer: leading lets (one `head = i / inner` decomp,
/// one `j = i - head*inner` / `j = i % inner` decomp, others copied), a
/// zero-init dot accumulator, ONE foreach whose body is the mul-add fold,
/// then the score store whose RHS references the accumulator.
fn match_score_producer(
    stmts: &[Statement],
    index_var: &str,
    score_buf: &str,
) -> Option<ScoreProducer> {
    let mut head_local: Option<String> = None;
    let mut j_local: Option<String> = None;
    let mut other_lets: Vec<Statement> = Vec::new();
    let mut rest: Vec<&Statement> = Vec::new();
    for s in stmts {
        let Statement::Let { name, expr: Some(e), .. } = s else {
            rest.push(s);
            continue;
        };
        if head_local.is_none() {
            if let Expr::BinaryOp(BinaryOpKind::Div, a, _) = e {
                if matches!(a.as_ref(), Expr::Identifier(n) if n == index_var) {
                    head_local = Some(name.clone());
                    continue;
                }
            }
        } else if j_local.is_none()
            && counter_tail(e, index_var, head_local.as_ref().unwrap())
        {
            j_local = Some(name.clone());
            continue;
        }
        other_lets.push(s.clone());
    }
    let head_local = head_local?;
    let j_local = j_local?;
    // The dot fold: `let acc = 0; foreach d { acc = acc + a[..] * b[..] }`.
    let (acc_local, dot) = match rest.first() {
        Some(Statement::Let {
            name,
            expr: Some(Expr::Decimal(0) | Expr::Float(0.0)),
            ..
        }) => (name.clone(), *rest.get(1)?),
        _ => return None,
    };
    let Statement::Foreach { list, body, .. } = dot else {
        return None;
    };
    let Expr::Range { .. } = list.as_ref() else {
        return None;
    };
    if body.len() != 1 || !matches!(&body[0], Statement::Assign(_, _)) {
        return None;
    }
    // The score store: `score_buf[i] = <rhs mentioning acc>`.
    let Statement::Assign(store_lhs, store_rhs) = rest.get(2)? else {
        return None;
    };
    let Expr::Index(b, _) = store_lhs else {
        return None;
    };
    if !matches!(b.as_ref(), Expr::Identifier(bn) if bn == score_buf) {
        return None;
    }
    if rest.len() != 3 || !expr_has_ident(store_rhs, &acc_local) {
        return None;
    }
    Some(ScoreProducer {
        head_local,
        j_local,
        other_lets,
        dot: dot.clone(),
        // dot was a &Statement — cloned above
        acc_local,
        store_rhs: store_rhs.clone(),
    })
}

/// `i - head*k` or `i % k` — the counter's tail decomp (the column local).
fn counter_tail(e: &Expr, index_var: &str, head_local: &str) -> bool {
    match e {
        Expr::BinaryOp(BinaryOpKind::Mod, a, _) => {
            matches!(a.as_ref(), Expr::Identifier(n) if n == index_var)
        }
        Expr::BinaryOp(BinaryOpKind::Sub, a, b) => {
            matches!(a.as_ref(), Expr::Identifier(n) if n == index_var)
                && matches!(b.as_ref(), Expr::BinaryOp(BinaryOpKind::Mul, x, _)
                    if matches!(x.as_ref(), Expr::Identifier(h) if h == head_local))
        }
        _ => false,
    }
}

/// True when `name` appears as an identifier anywhere in `e`.
fn expr_has_ident(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Identifier(n) => n == name,
        Expr::Index(a, i) => expr_has_ident(a, name) || expr_has_ident(i, name),
        Expr::BinaryOp(_, a, b) => expr_has_ident(a, name) || expr_has_ident(b, name),
        Expr::UnaryOp(_, a) => expr_has_ident(a, name),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_has_ident(a, name)),
        Expr::Cast(a, _) => expr_has_ident(a, name),
        _ => false,
    }
}

/// Match the linear-fold consumer: leading lets (head + d decomps, others
/// copied), a zero-init accumulator, ONE foreach whose body folds
/// `mid_out[head*inner + j] * other` (either mul order) into the
/// accumulator, then the terminal store `out[i] = acc`.
fn match_linear_consumer(
    stmts: &[Statement],
    index_var: &str,
    mid_out: &str,
    inner: &Expr,
) -> Option<LinearConsumer> {
    let mut head_local: Option<String> = None;
    let mut d_local: Option<String> = None;
    let mut d_end: Option<Expr> = None;
    let mut other_lets: Vec<Statement> = Vec::new();
    let mut rest: Vec<&Statement> = Vec::new();
    for s in stmts {
        let Statement::Let { name, expr: Some(e), .. } = s else {
            rest.push(s);
            continue;
        };
        if head_local.is_none() {
            if let Expr::BinaryOp(BinaryOpKind::Div, a, _) = e {
                if matches!(a.as_ref(), Expr::Identifier(n) if n == index_var) {
                    head_local = Some(name.clone());
                    continue;
                }
            }
        } else if d_local.is_none() {
            if let Some(end) = tail_end(e, index_var, head_local.as_ref().unwrap()) {
                d_local = Some(name.clone());
                d_end = Some(end);
                continue;
            }
        }
        other_lets.push(s.clone());
    }
    let head_local = head_local?;
    let d_local = d_local?;
    let d_end = d_end?;
    let (acc_local, fold) = match rest.first() {
        Some(Statement::Let {
            name,
            expr: Some(Expr::Decimal(0) | Expr::Float(0.0)),
            ..
        }) => (name.clone(), *rest.get(1)?),
        _ => return None,
    };
    let Statement::Foreach { list, body, .. } = fold else {
        return None;
    };
    let Expr::Range { .. } = list.as_ref() else {
        return None;
    };
    if body.len() != 1 {
        return None;
    }
    let Statement::Assign(_, fold_rhs) = &body[0] else {
        return None;
    };
    // The fold: acc + X * Y — one side self-accumulates, exactly one side
    // of the mul reads mid_out with the canonical row index, and the other
    // mentions the fold var and the d local.
    let Expr::BinaryOp(BinaryOpKind::Add, a, b) = fold_rhs else {
        return None;
    };
    let is_self = |e: &Expr| matches!(e, Expr::Identifier(n) if *n == acc_local);
    let mul = match (is_self(a), is_self(b)) {
        (true, false) => b,
        (false, true) => a,
        _ => return None,
    };
    let Expr::BinaryOp(BinaryOpKind::Mul, l, r) = mul.as_ref() else {
        return None;
    };
    let fold_other = if reads_row(l, mid_out, &head_local, inner) {
        r
    } else if reads_row(r, mid_out, &head_local, inner) {
        l
    } else {
        return None;
    };
    if !expr_has_ident(fold_other, &d_local) {
        return None;
    }
    // The terminal store: `out[i] = acc`.
    let Statement::Assign(out_lhs, out_rhs) = rest.get(2)? else {
        return None;
    };
    let Expr::Index(ob, oi) = out_lhs else {
        return None;
    };
    let Expr::Identifier(out_buf) = ob.as_ref() else {
        return None;
    };
    let out_buf = out_buf.clone();
    if !matches!(oi.as_ref(), Expr::Identifier(n) if n == index_var) {
        return None;
    }
    if !matches!(out_rhs, Expr::Identifier(n) if *n == acc_local) {
        return None;
    }
    if rest.len() != 3 {
        return None;
    }
    Some(LinearConsumer {
        other_lets,
        head_local,
        d_local,
        d_end,
        fold_other: fold_other.as_ref().clone(),
        out_buf,
    })
}

fn obn_of(ob: &Expr) -> String {
    match ob {
        Expr::Identifier(n) => n.clone(),
        _ => String::new(),
    }
}

/// `mid_out[head*inner + <j term>]` — the canonical row-locked fold index.
fn reads_row(e: &Expr, mid_out: &str, head_local: &str, inner: &Expr) -> bool {
    let Expr::Index(b, i) = e else {
        return false;
    };
    if !matches!(b.as_ref(), Expr::Identifier(bn) if bn == mid_out) {
        return false;
    }
    let Expr::BinaryOp(BinaryOpKind::Add, row, _) = i.as_ref() else {
        return false;
    };
    let Expr::BinaryOp(BinaryOpKind::Mul, h, k) = row.as_ref() else {
        return false;
    };
    matches!(h.as_ref(), Expr::Identifier(n) if n == head_local)
        && format!("{:?}", k) == format!("{:?}", inner)
}

/// Extract the d decomposition end from `i - head*k` or `i % k`.
fn tail_end(e: &Expr, index_var: &str, head_local: &str) -> Option<Expr> {
    let _ = head_local;
    match e {
        Expr::BinaryOp(BinaryOpKind::Mod, a, k) => {
            if matches!(a.as_ref(), Expr::Identifier(n) if n == index_var) {
                Some(k.as_ref().clone())
            } else {
                None
            }
        }
        Expr::BinaryOp(BinaryOpKind::Sub, a, b) => {
            match (a.as_ref(), b.as_ref()) {
                (Expr::Identifier(n), Expr::BinaryOp(BinaryOpKind::Mul, _, k))
                    if n == index_var =>
                {
                    Some(k.as_ref().clone())
                }
                _ => None,
            }
        }
        _ => None,
    }
}
