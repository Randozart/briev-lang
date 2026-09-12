pub mod abi;
pub mod builder;
pub(crate) mod coll_scaffold;
pub mod context;
pub mod directive;
pub mod dispatch;
pub mod emit_expr;
pub mod emit_stmt;
pub mod emit_toplevel;
pub(crate) mod kernel;
pub mod helpers;
pub mod intrinsics;
pub mod loop_engine;
pub mod normalizer;
pub mod types;
pub mod strategy;
pub mod vector_phi;

#[cfg(test)]
mod tests;


pub use builder::LLVMBuilder;
pub use context::{CompilerContext, FunctionContext, FunctionGuard};

fn escape_llvm_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\22"),
            b'\n' => out.push_str("\\0a"),
            b'\r' => out.push_str("\\0d"),
            b'\t' => out.push_str("\\09"),
            0x20..=0x7e => out.push(byte as char),
            b => { let _ = write!(out, "\\{:02x}", b); }
        }
    }
    out
}

/// 2026-08-26: every syntactic call name in the program (conservative set —
/// used only to decide whether a plain txn is referenced anywhere).
fn collect_called_names_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match e {
        Expr::Call(name, args, _) => {
            out.insert(name.clone());
            for a in args {
                collect_called_names_expr(a, out);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_called_names_expr(l, out);
            collect_called_names_expr(r, out);
        }
        Expr::UnaryOp(_, inner) => collect_called_names_expr(inner, out),
        Expr::Index(obj, idx) => {
            collect_called_names_expr(obj, out);
            collect_called_names_expr(idx, out);
        }
        Expr::Field(obj, _) => collect_called_names_expr(obj, out),
        Expr::Cast(inner, _) => collect_called_names_expr(inner, out),
        Expr::Tuple(exprs) | Expr::List(exprs) => {
            for x in exprs {
                collect_called_names_expr(x, out);
            }
        }
        Expr::Match(scrut, arms) => {
            collect_called_names_expr(scrut, out);
            for arm in arms {
                collect_called_names_expr(&arm.body, out);
                if let Some(g) = &arm.guard {
                    collect_called_names_expr(g, out);
                }
                collect_pattern_calls(&arm.pattern, out);
            }
        }
        Expr::Named { inner, .. } => collect_called_names_expr(inner, out),
        Expr::UnitLiteral { .. } => {}
        _ => {}
    }
}

fn collect_pattern_calls(p: &crate::ast::Pattern, out: &mut std::collections::HashSet<String>) {
    match p {
        crate::ast::Pattern::Literal(e) => collect_called_names_expr(e, out),
        crate::ast::Pattern::EnumVariant(_, subs)
        | crate::ast::Pattern::Tuple(subs) => {
            for sp in subs {
                collect_pattern_calls(sp, out);
            }
        }
        crate::ast::Pattern::Range(a, b) | crate::ast::Pattern::RangeInclusive(a, b) => {
            collect_called_names_expr(a, out);
            collect_called_names_expr(b, out);
        }
        crate::ast::Pattern::Multi(subs) => {
            for sp in subs {
                collect_pattern_calls(sp, out);
            }
        }
        _ => {}
    }
}

fn collect_called_names_stmt(s: &Statement, out: &mut std::collections::HashSet<String>) {
    match s {
        Statement::Assign(l, r) => {
            collect_called_names_expr(l, out);
            collect_called_names_expr(r, out);
        }
        Statement::Expression(e) => collect_called_names_expr(e, out),
        Statement::Let { expr: Some(e), .. } => collect_called_names_expr(e, out),
        Statement::Term(Some(e)) => collect_called_names_expr(e, out),
        Statement::Guarded(cond, body) => {
            collect_called_names_expr(cond, out);
            for inner in body {
                collect_called_names_stmt(inner, out);
            }
        }
        Statement::Block(body) | Statement::SyncBlock(body) => {
            for inner in body {
                collect_called_names_stmt(inner, out);
            }
        }
        _ => {}
    }
}

impl LlvmBackend {
    /// Warn once per plain txn that can never execute: non-reactive,
    /// callable-shaped (no params/outputs), and referenced by no call in the
    /// program. Fired logic belongs in `node` declarations.
    pub(crate) fn warn_undispatched_txns(
        items: &[TopLevel],
        txns: &[(String, &crate::ast::Transaction)],
        warnings: &mut Vec<String>,
    ) {
        let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
        for item in items {
            match item {
                TopLevel::Definition(d) => {
                    for stmt in &d.body {
                        collect_called_names_stmt(stmt, &mut called);
                    }
                }
                TopLevel::Transaction(t) => {
                    for stmt in &t.body {
                        collect_called_names_stmt(stmt, &mut called);
                    }
                    collect_called_names_expr(&t.contract.pre_condition, &mut called);
                    collect_called_names_expr(&t.contract.post_condition, &mut called);
                }
                TopLevel::Constant(c) => collect_called_names_expr(&c.expr, &mut called),
                _ => {}
            }
        }
        let mut names: Vec<String> = txns
            .iter()
            .filter(|(n, t)| {
                !t.is_reactive
                    && t.parameters.is_empty()
                    && t.output_type.is_none()
                    && t.outputs.is_empty()
                    && !called.contains(n.as_str())
            })
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        for n in names {
            warnings.push(format!(
                "warning: transaction '{}' is never dispatched — plain top-level \
                 txns do not fire in the tick loop. Declare it as 'node {}' to \
                 fire it, or call it explicitly.",
                n, n
            ));
        }
    }
}

pub(crate) fn float_to_llvm_hex(f: f64) -> String {
    let f32_val = f as f32;
    let bits = f32_val.to_bits();
    format!("{}", bits)
}

/// IEEE-754 binary16 encoding of an f32 value as an LLVM half literal
/// (`0xHXXXX`), round-to-nearest-even. 2026-09-02 (plan
/// fundamental-parent-membership): Float16 state slots store native `half`
/// literals. The typechecker's f32_fits_f16 gate admits exact values only,
/// but the encoder rounds correctly for any input (overflow → Inf,
/// underflow → ±0) so a future caller cannot produce garbage. Undo:
/// delete with the Float16 half-slot support in emit_field_init_value.
// 2026-09-02: numeric core of the f16 storage encoding — shared with the
// SPIR-V builder (f16 coopmat accumulator constants need the bit pattern,
// not the .ll hex text).
pub(crate) fn f32_to_f16_hex(v: f64) -> String {
    format!("0xH{:04X}", f32_to_f16_bits(v as f32))
}

pub(crate) fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    let h: u16 = if exp == 255 {
        // Inf / NaN — preserve quietness in the top mantissa bit.
        if mant == 0 { sign | 0x7c00 } else { sign | 0x7e00 | ((mant >> 13) as u16 & 0x03ff) }
    } else {
        let unbiased = exp - 127;
        if unbiased > 15 {
            sign | 0x7c00 // overflow → Inf
        } else if unbiased >= -14 {
            // Normal: round the 23-bit mantissa to 10 bits (RNE).
            let mut m = mant >> 13;
            let rem = mant & 0x1fff;
            if rem > 0x1000 || (rem == 0x1000 && (m & 1) == 1) { m += 1; }
            let mut e = (unbiased + 15) as u16;
            if m == 0x800 { m = 0; e += 1; } // mantissa carry bumps the exponent
            if e >= 31 { sign | 0x7c00 } else { sign | (e << 10) | m as u16 }
        } else if unbiased >= -25 {
            // Subnormal (RNE, including the round-up-to-min-normal boundary).
            let combined = 0x0080_0000u32 | mant;
            let d = (-(unbiased + 1)) as u32;
            let mut f10 = combined >> d;
            let rem = combined & ((1u32 << d) - 1);
            let half = 1u32 << (d - 1);
            if rem > half || (rem == half && (f10 & 1) == 1) { f10 += 1; }
            if f10 >= 0x400 { sign | (1 << 10) } else { sign | f10 as u16 }
        } else {
            sign // underflow → ±0
        }
    };
    h
}

// 2026-06-29: For Float64 literals (f64), bitcast directly to i64 bits
// without truncating through f32. Used by Expr::Float64 emission.
pub(crate) fn float64_to_llvm_hex(f: f64) -> String {
    let bits = f.to_bits();
    format!("{}", bits)
}

/// 2026-07-15: Emit a float constant in the correct LLVM format.
/// For `float` (32-bit): `bitcast (i32 <bits> to float)`
/// For `double` (64-bit): `bitcast (i64 <bits> to double)`
pub(crate) fn float_to_llvm_str(f: f64, llvm_ty: &str) -> String {
    match llvm_ty {
        "float" => format!("bitcast (i32 {} to float)", float_to_llvm_hex(f)),
        _ => format!("bitcast (i64 {} to double)", float64_to_llvm_hex(f)),
    }
}

/// Recursively evaluate a constant expression tree to a concrete f64.
/// Used to fold `const m0: Float = 4.0 * pi * pi` into a literal before
/// global emission, avoiding the `constant float 0` bug.
///
/// 2026-07-31: Phase 3 (§8.4-D4) — float const resolution uses a protocol
/// membership check (`is_protocol_member(ty, "Float")` via the casting graph,
/// supplied by the caller) instead of matching the type name.
fn try_eval_cfloat(
    expr: &Expr,
    constants: &HashMap<String, (Type, Expr)>,
    is_float: &impl Fn(&Type) -> bool,
) -> Option<f64> {
    match expr {
        Expr::Float(f) => Some(*f),
        Expr::Identifier(name) => {
            match constants.get(name) {
                Some((ty, inner)) if is_float(ty) => try_eval_cfloat(inner, constants, is_float),
                _ => None,
            }
        }
        Expr::BinaryOp(kind, l, r) => {
            let lv = try_eval_cfloat(l, constants, is_float)?;
            let rv = try_eval_cfloat(r, constants, is_float)?;
            match kind {
                crate::ast::BinaryOpKind::Add => Some(lv + rv),
                crate::ast::BinaryOpKind::Sub => Some(lv - rv),
                crate::ast::BinaryOpKind::Mul => Some(lv * rv),
                crate::ast::BinaryOpKind::Div => Some(lv / rv),
                _ => None,
            }
        }
        Expr::UnaryOp(kind, inner) => {
            if matches!(kind, crate::ast::UnaryOpKind::Neg) {
                Some(-try_eval_cfloat(inner, constants, is_float)?)
            } else {
                None
            }
        }
        Expr::Named { inner, .. } => try_eval_cfloat(inner, constants, is_float),
        Expr::UnitLiteral { value, .. } => {
            Some(*value)
        }
        _ => None,
    }
}

/// Map an LLVM type string to its byte size. Used by compute_state_size_bytes.
/// 2026-08-07 (object instance pools): `[N x T]` recurses — the byte size of
/// an array column/heap buffer is N times the element size (a member-array
/// row in a dependent pool's malloc), not the flat default.
fn llvm_type_byte_size(t: &str) -> i64 {
    if let Some(rest) = t.strip_prefix('[') {
        if let Some((count, elem)) = rest.split_once("x ") {
            if let Ok(n) = count.trim().parse::<i64>() {
                return n * llvm_type_byte_size(elem.trim_end_matches(']').trim());
            }
        }
        return 8;
    }
    match t {
        "i8" | "i1" => 1,
        "i16" => 2,
        "i32" | "float" => 4,
        "i64" | "double" | "i8*" | "ptr" => 8,
        // For aggregate or unknown types, assume max alignment (8 bytes)
        // to err on the side of larger allocation.
        _ => 8,
    }
}

// ── Swan-song hoist: moved to frontend analysis ─────────────────────
//
// 2026-07-31: hoist_terminating_guard / remap_stmt_identifiers /
// remap_expr_into were removed from this file. The terminating-guard hoist
// and its let-to-field remap now live in src/analysis/swan_song.rs
// (hoist_swan_song), computed once per transaction in analyze_program and
// consumed here via analysis.swan_songs. See
// docs/plans/2026-07-31-frontend-driven-dispatch.md §6.
//
// Preserved rationale from the removed functions:
//   - 2026-07-05: The let-to-state-field mapping (`&field = let_name` →
//     map[let_name] = field_name) exists because a hoisted swan song may
//     reference a let binding (e.g. `nesc` in mandelbrot) whose register is
//     only valid inside the loop body; the value lives in a state field, so
//     identifiers are rewritten to the field name.
//   - 2026-07-04: The hoist fires even when the guard body is empty — the
//     guard may be just `term! -> print_int#(result)` with no preceding
//     statements. Hoisting it empties pending_post_hoist correctly, which
//     unblocks Path A (no-dead-stores) emission in emit_countable_main.

use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Type};

// 2026-07-18: Allocation strategy — tracks how a pointer was allocated.
// Arena: bump-allocated from per-txn arena (Free# is no-op).
// Malloc: heap-allocated via @malloc (Free# must call @free).
// Alloca: stack-allocated via alloca (Free# is no-op).
#[derive(Debug, Clone, PartialEq)]
pub enum AllocStrategy {
    Arena,
    Malloc,
    Alloca,
    /// 2026-07-18: Inline storage in parent struct (SSO/SVO for ≤64B).
    Inline,
    /// 2026-07-18: Circular buffer, overwrite-oldest. Free# is no-op.
    RingBuffer,
    /// 2026-07-18: Named strategy from config/alloc-strategies.dbvl.
    Config(String),
    /// 2026-07-18: User-provided Briev function as allocator.
    Custom(String),
}

#[derive(Debug, Clone)]
pub struct TypedRegister {
    pub name: String,
    pub ty: Type,
}

pub struct FoldParam {
    pub counter_idx: usize,
    pub bound_field_idx: Option<usize>,
    pub bound_const_name: Option<String>,
    pub is_decreasing: bool,
    pub bound_literal: Option<i64>,
}

impl std::fmt::Display for TypedRegister {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

// TypedRegister has no llvm() method — use LlvmBackend::llvm_type() instead.
// See emit_toplevel.rs:298 for the canonical type mapping.

use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};

/// Collect all unique string literal values from the program for global emission.
/// 2026-08-06 (Phase 7): pre-collect raw-bytes Data literals (`#b"..."`) so
/// the `@bstr.N` globals are emitted before the functions reference them.
fn collect_byte_literals(items: &[TopLevel]) -> Vec<Vec<u8>> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Vec<u8>> = Vec::new();
    for item in items {
        collect_bytes_tl(item, &mut seen, &mut out);
    }
    out
}

fn collect_bytes_tl(tl: &TopLevel, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match tl {
        TopLevel::Transaction(t) => {
            collect_bytes_expr(&t.contract.pre_condition, seen, out);
            collect_bytes_expr(&t.contract.post_condition, seen, out);
            for s in &t.body {
                collect_bytes_stmt(s, seen, out);
            }
        }
        TopLevel::Definition(d) => {
            for s in &d.body {
                collect_bytes_stmt(s, seen, out);
            }
        }
        TopLevel::Constant(c) => collect_bytes_expr(&c.expr, seen, out),
        TopLevel::Statement(stmt) => collect_bytes_stmt(stmt, seen, out),
        _ => {}
    }
}

fn collect_bytes_stmt(stmt: &Statement, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                collect_bytes_expr(e, seen, out);
            }
        }
        Statement::Term(Some(expr)) => collect_bytes_expr(expr, seen, out),
        Statement::Assign(_, rhs) => collect_bytes_expr(rhs, seen, out),
        Statement::Expression(e) | Statement::Gate(e) => collect_bytes_expr(e, seen, out),
        Statement::Guarded(_, body) | Statement::Block(body) | Statement::SyncBlock(body) => {
            for s in body {
                collect_bytes_stmt(s, seen, out);
            }
        }
        Statement::Foreach { list, body, .. } => {
            collect_bytes_expr(list, seen, out);
            for s in body {
                collect_bytes_stmt(s, seen, out);
            }
        }
        _ => {}
    }
}

fn collect_bytes_expr(expr: &Expr, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match expr {
        Expr::TaggedQuotedLiteral(bytes, prefix) if prefix == "b" => {
            if seen.insert(bytes.clone()) {
                out.push(bytes.clone());
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_bytes_expr(l, seen, out);
            collect_bytes_expr(r, seen, out);
        }
        Expr::Call(_, args, _) | Expr::List(args) | Expr::Tuple(args) => {
            for a in args {
                collect_bytes_expr(a, seen, out);
            }
        }
        Expr::Cast(i, _) | Expr::IsType(i, _) | Expr::Consume(i) | Expr::Deref(i) | Expr::AddrOf(i) | Expr::Await(i) => {
            collect_bytes_expr(i, seen, out);
        }
        Expr::Field(o, _) | Expr::Index(o, _) => collect_bytes_expr(o, seen, out),
        Expr::Slice { array, start, end, stride, .. } => {
            collect_bytes_expr(array, seen, out);
            for b in [start, end, stride].into_iter().flatten() {
                collect_bytes_expr(b, seen, out);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                collect_bytes_expr(f, seen, out);
            }
        }
        Expr::If(c, t, e) => {
            collect_bytes_expr(c, seen, out);
            collect_bytes_expr(t, seen, out);
            if let Some(e) = e {
                collect_bytes_expr(e, seen, out);
            }
        }
        Expr::Match(s, arms) => {
            collect_bytes_expr(s, seen, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_bytes_expr(g, seen, out);
                }
                collect_bytes_expr(&arm.body, seen, out);
            }
        }
        Expr::Named { inner, .. } => { collect_bytes_expr(inner, seen, out); }
        Expr::UnitLiteral { .. } => {}
        _ => {}
    }
}

/// 2026-08-07 (Phase 7): pre-collect compile-time Boolean mask literals
/// (`data[mask]` where mask is `[true, false, …]`) as 0/1 byte arrays. They
/// are interned as `@bmask.N` globals BEFORE the module header is emitted —
/// pushing during body emission would leave an undefined `@bmask` reference.
fn collect_mask_literals(items: &[TopLevel]) -> Vec<Vec<u8>> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Vec<u8>> = Vec::new();
    for item in items {
        collect_masks_tl(item, &mut seen, &mut out);
    }
    out
}

fn collect_masks_tl(tl: &TopLevel, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match tl {
        TopLevel::Transaction(t) => {
            collect_masks_expr(&t.contract.pre_condition, seen, out);
            collect_masks_expr(&t.contract.post_condition, seen, out);
            for s in &t.body {
                collect_masks_stmt(s, seen, out);
            }
        }
        TopLevel::Definition(d) => {
            for s in &d.body {
                collect_masks_stmt(s, seen, out);
            }
        }
        TopLevel::Constant(c) => collect_masks_expr(&c.expr, seen, out),
        TopLevel::Statement(stmt) => collect_masks_stmt(stmt, seen, out),
        _ => {}
    }
}

fn collect_masks_stmt(stmt: &Statement, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match stmt {
        Statement::Let { expr: Some(e), .. } => collect_masks_expr(e, seen, out),
        Statement::Term(Some(expr)) | Statement::EndProgram(Some(expr)) => collect_masks_expr(expr, seen, out),
        Statement::Assign(_, rhs) => collect_masks_expr(rhs, seen, out),
        Statement::Expression(e) | Statement::Gate(e) => collect_masks_expr(e, seen, out),
        Statement::Guarded(_, body) | Statement::Block(body) | Statement::SyncBlock(body) => {
            for s in body {
                collect_masks_stmt(s, seen, out);
            }
        }
        Statement::Foreach { list, body, .. } => {
            collect_masks_expr(list, seen, out);
            for s in body {
                collect_masks_stmt(s, seen, out);
            }
        }
        _ => {}
    }
}

fn collect_masks_expr(expr: &Expr, seen: &mut std::collections::HashSet<Vec<u8>>, out: &mut Vec<Vec<u8>>) {
    match expr {
        Expr::Index(obj, index) => {
            if let Some(bytes) = const_bool_mask_bytes(index) {
                if seen.insert(bytes.clone()) {
                    out.push(bytes);
                }
            }
            collect_masks_expr(obj, seen, out);
            collect_masks_expr(index, seen, out);
        }
        Expr::BinaryOp(_, l, r) => {
            collect_masks_expr(l, seen, out);
            collect_masks_expr(r, seen, out);
        }
        Expr::Call(_, args, _) | Expr::List(args) | Expr::Tuple(args) => {
            for a in args {
                collect_masks_expr(a, seen, out);
            }
        }
        Expr::Cast(i, _) | Expr::IsType(i, _) | Expr::Consume(i) | Expr::Deref(i) | Expr::AddrOf(i) | Expr::Await(i) => {
            collect_masks_expr(i, seen, out);
        }
        Expr::Field(o, _) => collect_masks_expr(o, seen, out),
        Expr::Slice { array, start, end, stride, .. } => {
            collect_masks_expr(array, seen, out);
            for b in [start, end, stride].into_iter().flatten() {
                collect_masks_expr(b, seen, out);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                collect_masks_expr(f, seen, out);
            }
        }
        Expr::If(c, t, e) => {
            collect_masks_expr(c, seen, out);
            collect_masks_expr(t, seen, out);
            if let Some(e) = e {
                collect_masks_expr(e, seen, out);
            }
        }
        Expr::Match(s, arms) => {
            collect_masks_expr(s, seen, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_masks_expr(g, seen, out);
                }
                collect_masks_expr(&arm.body, seen, out);
            }
        }
        Expr::Named { inner, .. } => { collect_masks_expr(inner, seen, out); }
        Expr::UnitLiteral { .. } => {}
        _ => {}
    }
}

/// The 0/1 bytes of a compile-time Boolean mask literal (`[true, false, …]`),
/// or None if `expr` is not one.
fn const_bool_mask_bytes(expr: &Expr) -> Option<Vec<u8>> {
    let Expr::List(elems) = expr else {
        return None;
    };
    let mut out = Vec::with_capacity(elems.len());
    for e in elems {
        match e {
            Expr::Bool(b) => out.push(if *b { 1 } else { 0 }),
            _ => return None,
        }
    }
    Some(out)
}

fn collect_strings(items: &[TopLevel]) -> Vec<String> {    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    // 2026-07-17: Always start with the empty string at index 0.
    // emit_field_init_value references @str.0 as the uninitialized String
    // sentinel (Site B at line 866). Without this, @str.0 is never emitted
    // when no Expr::Quoted appears in the source, producing "use of undefined
    // value '@str.0'" in 14 of 23 benchmarks.
    out.push("".to_string());
    seen.insert("".to_string());
    for item in items {
        collect_strings_tl(item, &mut seen, &mut out);
    }
    out
}
fn collect_strings_tl(tl: &TopLevel, seen: &mut std::collections::HashSet<String>, out: &mut Vec<String>) {
    match tl {
        TopLevel::Transaction(t) => {
            // 2026-08-01 (Phase 3b): entry!/args! rewrite string literals into
            // the contract precondition (e.g. entry_cmd() == "build") — scan
            // the contract so those @str.N constants are emitted.
            collect_strings_expr(&t.contract.pre_condition, seen, out);
            collect_strings_expr(&t.contract.post_condition, seen, out);
            for s in &t.body { collect_strings_stmt(s, seen, out); }
        }
        TopLevel::Definition(d) => {
            collect_strings_expr(&d.contract.pre_condition, seen, out);
            collect_strings_expr(&d.contract.post_condition, seen, out);
            for s in &d.body { collect_strings_stmt(s, seen, out); }
        }
        TopLevel::Export(e) => collect_strings_tl(&e.inner, seen, out),
        TopLevel::Cell(c) => {
            // 2026-07-13: Field.default removed in new AST.
            for _ in &c.fields { }
            for txn in &c.transactions { for s in &txn.body { collect_strings_stmt(s, seen, out); } }
            for d in &c.definitions { for s in &d.body { collect_strings_stmt(s, seen, out); } }
            for trg in &c.internal_triggers { collect_strings_expr(&Expr::Identifier(trg.name.clone()), seen, out); }
        }
        // 2026-07-26: TopLevel::Statement wraps top-level let bindings
        // that contain string literals (e.g. GetEnvInt!("BOUND")).
        // Without this arm, the string is never collected and @str.N is
        // referenced but undefined, causing clang to fail.
        TopLevel::Statement(stmt) => {
            collect_strings_stmt(stmt, seen, out);
        }
        // 2026-08-13 (obj member string literals): an `obj` member body
        // (append_bool's `"true"`/`"false"`, a member contract) holds quoted
        // strings that the emitted member references. Without this arm the
        // globals are referenced but undefined once the member is actually
        // compiled (it was unreachable before the obj value ABI fix).
        TopLevel::TypeDef(td) => {
            for member in &td.body.members {
                collect_strings_tl(member, seen, out);
            }
        }
        _ => {}
    }
}
fn collect_strings_stmt(stmt: &Statement, seen: &mut std::collections::HashSet<String>, out: &mut Vec<String>) {
    match stmt {
        Statement::Yield => {}
        Statement::Check(_) => {}
        Statement::Let { expr, .. } => { if let Some(e) = expr { collect_strings_expr(e, seen, out); } }
        Statement::Assign(_, expr) => { collect_strings_expr(expr, seen, out); }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target { collect_strings_expr(t, seen, out); }
            collect_strings_expr(value, seen, out);
        }
        Statement::FreeHint(_) | Statement::KeepHint(_) | Statement::Trap | Statement::Halt => {}
        Statement::Break => {}
        Statement::Expression(e) => { collect_strings_expr(e, seen, out); }
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => { collect_strings_expr(e, seen, out); }
        Statement::Term(None) | Statement::EndProgram(None) => {}
        Statement::Guarded(condition, statements) => {
            collect_strings_expr(condition, seen, out);
            for s in statements { collect_strings_stmt(s, seen, out); }
        }
        Statement::Gate(cond) => { collect_strings_expr(cond, seen, out); }
        Statement::Block(body) | Statement::SyncBlock(body)
        | Statement::Defer(body) | Statement::Mutex(body) => {
            for s in body { collect_strings_stmt(s, seen, out); }
        }
        Statement::Barrier { body, .. } => {
            for s in body { collect_strings_stmt(s, seen, out); }
        }
        Statement::Rollback(Some(e)) => { collect_strings_expr(e, seen, out); }
        Statement::Rollback(None) => {}
        Statement::Foreach { list, body, .. } => {
            collect_strings_expr(list, seen, out);
            for s in body { collect_strings_stmt(s, seen, out); }
        }
        Statement::InlineAsm { .. } | Statement::TrgBinding { .. } | Statement::MetadataAssignment(..) | Statement::InlineDefn(_) | Statement::InlineTxn(_) | Statement::Match { .. } => {}
    }
}

fn collect_strings_expr(expr: &Expr, seen: &mut std::collections::HashSet<String>, out: &mut Vec<String>) {
    match expr {
        Expr::Consume(inner) => { collect_strings_expr(inner, seen, out); }
        Expr::Await(inner) => { collect_strings_expr(inner, seen, out); }
        Expr::BeginProgram => {}
        Expr::Quoted(s) => {
            let s_str = String::from_utf8_lossy(s).into_owned();
            if !seen.contains(&s_str) {
                seen.insert(s_str.clone());
                out.push(s_str);
            }
        }
        // 2026-08-06 (Phase 7): `#b"..."` is a raw Data literal — its exact
        // bytes go to byte_constants, NOT the lossy string constants.
        Expr::TaggedQuotedLiteral(s, prefix) if prefix == "b" => {}
        Expr::TaggedQuotedLiteral(s, _) => {
            let s_str = String::from_utf8_lossy(s).into_owned();
            if !seen.contains(&s_str) {
                seen.insert(s_str.clone());
                out.push(s_str);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_strings_expr(l, seen, out);
            collect_strings_expr(r, seen, out);
        }
        Expr::UnaryOp(_, inner) | Expr::Cast(inner, _) | Expr::IsType(inner, _) => {
            collect_strings_expr(inner, seen, out);
        }
        Expr::List(elems) | Expr::Tuple(elems) => {
            for e in elems { collect_strings_expr(e, seen, out); }
        }
        Expr::Field(o, _) | Expr::Index(o, _) => {
            collect_strings_expr(o, seen, out);
        }
        Expr::Call(_, args, _) => {
            for a in args { collect_strings_expr(a, seen, out); }
        }
        Expr::Spawn { args, .. } => {
            for a in args { collect_strings_expr(a, seen, out); }
        }
        Expr::Match(value, arms) => {
            collect_strings_expr(value, seen, out);
            for arm in arms { collect_strings_expr(&arm.body, seen, out); }
        }
        Expr::Block(stmts) => {
            for s in stmts { collect_strings_stmt(s, seen, out); }
        }
        // 2026-08-13: a struct-literal field holds a quoted string
        // (`StringBuilder { buffer: builder.buffer + "false" }`) — collect it
        // or the @str.N global is referenced but undefined.
        Expr::StructLiteral { fields, .. } => {
            for (_, fexpr) in fields { collect_strings_expr(fexpr, seen, out); }
        }
        Expr::If(cond, then_b, else_b) => {
            collect_strings_expr(cond, seen, out);
            collect_strings_expr(then_b, seen, out);
            if let Some(eb) = else_b { collect_strings_expr(eb, seen, out); }
        }
        Expr::Lambda(_, body) => {
            collect_strings_expr(body, seen, out);
        }
        Expr::Within(body, fallback) => {
            collect_strings_expr(body, seen, out);
            collect_strings_expr(fallback, seen, out);
        }
        Expr::DerivationBlock(db) => {
            for ex in &db.examples {
                for inp in &ex.inputs { collect_strings_expr(inp, seen, out); }
                collect_strings_expr(&ex.output, seen, out);
            }
        }
        Expr::Deref(inner) => {
            collect_strings_expr(inner, seen, out);
        }
        Expr::AddrOf(inner) => {
            collect_strings_expr(inner, seen, out);
        }
        Expr::PluginIntercept { args, .. } => {
            for a in args { collect_strings_expr(a, seen, out); }
        }
        // Leaves — no sub-expressions
        Expr::Decimal(_) | Expr::TaggedLiteral(_, _) | Expr::Char(_) | Expr::Bool(_) | Expr::Float(_) | Expr::Identifier(_)
        | Expr::FormattingAnnotation(_) | Expr::StructLiteral { .. } => {}
        Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
            collect_strings_expr(recv, seen, out);
        }
        Expr::MethodCall(recv, _, args, _) => {
            collect_strings_expr(recv, seen, out);
            for a in args { collect_strings_expr(a, seen, out); }
        }
        Expr::Exists(name) => { panic!("compile-time existence check '{}' reached LLVM codegen", name) },
            Expr::Slice { array, start, end, stride } => {
                collect_strings_expr(array, seen, out);
                if let Some(e) = start.as_deref() { collect_strings_expr(e, seen, out); }
                if let Some(e) = end.as_deref() { collect_strings_expr(e, seen, out); }
                if let Some(e) = stride.as_deref() { collect_strings_expr(e, seen, out); }
            }
            Expr::Range { start, end, inclusive: _ } => {
                collect_strings_expr(start, seen, out);
                collect_strings_expr(end, seen, out);
            }
            Expr::Named { inner, .. } => { collect_strings_expr(inner, seen, out); }
            Expr::UnitLiteral { .. } => {}

    }
}

/// LLVM IR backend — the definitive compiler from Briev AST to `.ll`.
///
/// Every lesson from phases 0–5.5 integrated into one coherent pass:
/// - \`noalias nocapture\` on all \`ptr\` — LLVM sees no pointer aliasing
/// - i64-centric expression system — strings/lists become `i64` via `ptrtoint`/`inttoptr`
/// - Bool (i8) fields trunc on store, zext on load; floats via bitcast+zext; char via zext
/// - Unique guard labels, `returns_i64` flag, fused txn terminator filtering
/// - All Expr/Statement/TopLevel variants emit valid IR
/// - Contracts: `!range`, `@llvm.assume` (debug panic / release assume)
/// - Match→switch with phi merge, unification, pattern match
/// - FFI declare+call with C ABI (transparent, no compiler magic)
/// - Transition fusing, trigger sampling by MMIO/linked address
/// - Precondition extraction → internal `i1` functions, dispatch chain
/// - User-provided `frgn __wait_for_event` + `node [true]` for sleep
/// - `@ link` triggers → `external global` + `load volatile`
///
/// Design philosophy: the backend is a single monolithic pass (not a pipeline
/// of small passes) because Briev's contract system provides structural
/// guarantees that LLVM cannot infer from generic IR. By emitting contract-
/// aware IR directly (TBAA, !range, noalias), we avoid the need for an
/// expensive LLVM analysis pass to rediscover what the contracts already state.

/// 2026-07-26: Protocol-driven LLVM type. Reads llvm_type from the
/// type's ResolvedType in the universe. No name matching — the primordial
/// llvm_type property is the single source of truth. Returns "i64" if the
/// universe is unavailable or the type is unknown (safe default).
/// Derive LLVM type string from a Type, using protocol membership + bytes.
/// 2026-07-30: No longer reads llvm_type from universe properties.
/// Protocol-based resolution is handled by CastingGraph::resolve_llvm_type()
/// (accessible via LlvmBackend::llvm_type() in codegen contexts).
/// Ptr<T> and Vector<T,N> are compiler constructs, not universe types.
pub fn protocol_llvm_type(ty: &Type, universe: Option<&crate::type_universe::TypeUniverse>) -> String {
    if matches!(ty, Type::Ptr(_) | Type::Vector(_, _)) {
        return "ptr".to_string();
    }
    // 2026-07-30: String uses the ptr representation (bits model).
    // 2026-07-31: Phase 3 (§8.4-D7) — String detection via the Cast.String
    // protocol property.
    // 2026-08-01 (B4): the structural is_string_like (2-int-field) check was
    // retired — protocol membership is the sole String test (rule #18; a
    // String has no fields under B0, so the structural check was false anyway).
    let is_string_protocol = universe
        .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
        .map_or(false, |rt| rt.properties.contains_key("Cast.String"));
    if is_string_protocol {
        // 2026-08-01 (B0): A String value is a ptr to a length-prefixed
        // [len][bytes] buffer. protocol_llvm_type previously claimed
        // { i64, i64 } here, which made frgn declares disagree with the
        // i64-typed call sites and i64 state slots (the split-brain). Every
        // type-claiming site now says ptr; state slots keep the i64 machine
        // word and convert via adapt_to_i64/ensure_typed_value.
        return "ptr".to_string();
    }
    if let Some(ref u) = universe {
        if let Some(rt) = ty.universe_key().and_then(|k| u.get(k)) {
            // Check protocol membership first — float types get native float/double
            if rt.properties.contains_key("Cast.Float") {
                return if rt.max_bits <= 32 { "float".to_string() }
                       else if rt.max_bits <= 64 { "double".to_string() }
                       else { "i64".to_string() };
            }
            // Use bytes as fallback for non-protocol types
            if rt.bytes > 0 {
                return format!("i{}", rt.bytes * 8);
            }
        }
    }
    "i64".to_string()
}

/// 2026-08-10: Byte width of an emitted LLVM field type string for the
/// webstack state_layout table. Covers the vocabulary the backend emits into
/// %State (iN scalars, float/double, ptr, and `[N x iT]` arrays). Returns 0
/// for anything unrecognized so the caller skips the row rather than lying.
pub fn web_llvm_byte_size(llvm_ty: &str) -> u64 {
    if let Some(bits) = llvm_ty.strip_prefix('i') {
        if let Ok(n) = bits.parse::<u64>() {
            let bytes = n.div_ceil(8);
            return if bytes <= 8 { bytes } else { 0 }; // >i64 not stored in %State words
        }
    }
    match llvm_ty {
        "float" => 4,
        "double" => 8,
        "ptr" => 8,
        _ => {
            // `[N x T]` — N× the element width.
            if llvm_ty.starts_with('[') && llvm_ty.contains(" x ") && llvm_ty.ends_with(']') {
                let inner = &llvm_ty[1..llvm_ty.len() - 1];
                let (n, elem) = inner.split_once(" x ").unwrap_or(("", ""));
                if let (Ok(count), elem_ty) = (n.trim().parse::<u64>(), elem.trim()) {
                    let elem_size = web_llvm_byte_size(elem_ty);
                    if elem_size > 0 {
                        return count * elem_size;
                    }
                }
            }
            0
        }
    }
}

/// 2026-08-11 (Phase 2a3): the per-element byte width of a state field's LLVM
/// type. A vector (`[N x T]`) reports T's width — the b-each renderer derives
/// the item count as total_size / element_width. Non-vectors report the full
/// type width (a scalar field's "element" is itself).
pub(super) fn web_vector_element_size(llvm_ty: &str) -> u64 {
    if llvm_ty.starts_with('[') && llvm_ty.contains(" x ") && llvm_ty.ends_with(']') {
        let inner = &llvm_ty[1..llvm_ty.len() - 1];
        let (_, elem) = inner.split_once(" x ").unwrap_or(("", ""));
        let elem_size = web_llvm_byte_size(elem.trim());
        if elem_size > 0 {
            return elem_size;
        }
    }
    web_llvm_byte_size(llvm_ty)
}

/// 2026-08-10: Size of the webstack flush buffer — the largest transaction
/// write_set (each txn's update batch fits; unused tail entries are zero).
/// Frontend-provided analysis (transition graph write_sets), never re-walked.
pub(super) fn web_max_flush_entries(ctx: &crate::backend::llvm::context::CompilerContext) -> u32 {
    let max = ctx.transition_graph.as_ref()
        .map(|g| g.nodes.iter().map(|n| n.write_set.len()).max().unwrap_or(0))
        .unwrap_or(0);
    max.max(1) as u32
}
pub(super) fn trg_llvm_storage_ty(ty: &Type, universe: Option<&crate::type_universe::TypeUniverse>) -> String {
    protocol_llvm_type(ty, universe)
}

/// Map a field's LLVM storage type string to its TBAA metadata node index.
/// Returns the !N index into the TBAA tree emitted at end of module.
/// universe is optional: when available, uses the dynamically-generated
/// TBAA tree (sorted alphabetically, Int first).  When None, falls back
/// to the original hardcoded indices for the 5 built-in types.
/// Sort TBAA group names deterministically: alphabetical, with the Int
/// protocol member moved to the front (the fallback for unmatched types).
///
/// 2026-07-31: Phase 3 (§8.4-D6) — the front-member is chosen by Int protocol
/// membership (the `Cast.Int` universe property) instead of the literal type
/// 2026-08-13 (layout-keywords plan Phase 5): read the parser's structured
/// `atomic_fields` metadata (a PropertyValue::List of "field_name:ordering"
/// Strings) and record each `<type>.<field>` slot with its ordering in
/// `ctx.atomic_fields` — the carrier the field load/store emitters consult.
/// Plain (non-atomic) fields are untouched.
fn register_atomic_fields(
    ctx: &mut CompilerContext,
    type_name: &str,
    metadata: &std::collections::HashMap<String, crate::ast::PropertyValue>,
) {
    if let Some(crate::ast::PropertyValue::List(entries)) = metadata.get("atomic_fields") {
        for entry in entries {
            if let crate::ast::PropertyValue::String(field) = entry {
                let (name, ordering) = field.split_once(':').unwrap_or((field.as_str(), "seq"));
                ctx.atomic_fields.insert(
                    format!("{}.{}", type_name, name),
                    ordering.to_string(),
                );
            }
        }
    }
}

/// name "Int". All three TBAA sites use this helper so the metadata
/// declaration and the node-index lookups stay in agreement.
fn sort_tbaa_groups(universe: Option<&crate::type_universe::TypeUniverse>, groups: &mut Vec<String>) {
    groups.sort();
    let is_int_protocol = |name: &str| {
        universe
            .and_then(|u| u.types.get(name))
            .map_or(false, |rt| rt.properties.contains_key("Cast.Int"))
    };
    if let Some(pos) = groups.iter().position(|g| is_int_protocol(g)) {
        groups.swap(0, pos);
    }
}

pub(super) fn tbaa_node(ty_str: &str, universe: Option<&crate::type_universe::TypeUniverse>) -> i32 {
    // Map string to TBAA group name
    let group = match ty_str {
        "i64" => "Int",
        "i8"  => "Bool",
        "i32" => "Char",
        "i8*" | "ptr" => "String",
        "float" | "double" => "Float",
        _ => "Int",  // fallback
    };
    if let Some(u) = universe {
        // 2026-07-13: Inline sorted groups from type names.
        let mut groups: Vec<String> = u.types.keys().cloned().collect();
        sort_tbaa_groups(Some(u), &mut groups);
        groups.iter().position(|g| g == group).map(|i| i as i32 + 1).unwrap_or(1)
    } else {
        // Fallback: hardcoded indices
        match group {
            "Int"    => 1,
            "Bool"   => 2,
            "Char"   => 3,
            "String" => 4,
            "Float"  => 5,
            _ => 1,
        }
    }
}

/// Map a Briev type to its TBAA metadata node index via universe lookup.
/// 2026-07-13: Simplified for new ResolvedType (tbaa_node removed).
/// Uses type name as the TBAA group. Falls back to 1 (Int) when not found.
pub(super) fn tbaa_node_for_type(ty: &Type, universe: &crate::type_universe::TypeUniverse) -> i32 {
    let group = match ty.universe_key() {
        Some(key) if universe.contains(key) => key,
        _ => return 1,
    };
    let mut groups: Vec<String> = universe.types.keys().cloned().collect();
    sort_tbaa_groups(Some(universe), &mut groups);
    groups.iter().position(|g| g == group).map(|i| i as i32 + 1).unwrap_or(1)
}
    /// at the IR level. Without TBAA, all i64 accesses within %State are
    /// MayAlias, preventing GVN load elimination.

/// Why metadata defs must be deferred: LLVM 18+ rejects metadata definitions
/// (!N = !{...}) inside function bodies. Metadata must be defined at module
/// scope. We collect all pending metadata in a Vec and flush it at the end
/// of the module (after the last function definition). This also lets us
/// deduplicate identical metadata nodes across multiple loop headers.

/// Emit `!llvm.loop` metadata for a backedge branch and the branch itself.
///
/// Follows the `foreach.rs` pattern: emit metadata entries, build the
/// self-referencing loop metadata node, and attach it to the backedge branch.
/// Metadata definitions are written to `pending_metadata` (flushed at module
/// end) because LLVM 18+ rejects metadata definitions inside function bodies.
///
/// 2026-06-20: Phase 0a — retrofitted from foreach.rs to main loop paths.
pub(super) fn emit_loop_metadata(
    out: &mut String,
    indent: &str,
    backedge_label: &str,
    metadata_counter: &mut usize,
    pending_metadata: &mut String,
    disable_fold: bool,
) {
    let md_idx = emit_loop_metadata_nodes(metadata_counter, pending_metadata, disable_fold);
    writeln!(out, "{0}br label %{1}, !llvm.loop !{2}", indent, backedge_label, md_idx).ok();
}

/// Same as `emit_loop_metadata` but the caller supplies the backedge text.
/// Use when the backedge is a conditional branch (br i1) or when multiple
/// backedges share the same metadata node.
pub(super) fn emit_loop_metadata_nodes(
    metadata_counter: &mut usize,
    pending_metadata: &mut String,
    disable_fold: bool,
) -> usize {
    let start = *metadata_counter;
    let mut md_count = 1;
    let mut md_entries = Vec::new();
    // Default: vectorize.enable for all counted loops
    let vd_md = start + md_count;
    writeln!(pending_metadata, "!{0} = !{{!\"llvm.loop.vectorize.enable\", i1 true}}", vd_md).ok();
    md_entries.push(format!("!{}", vd_md));
    md_count += 1;

    // 2026-07-21: Loop alignment — prevents DSB/MITE penalty from instructions
    // crossing 32-byte fetch window boundaries (ring_buffer mul instruction).
    let align_md = start + md_count;
    writeln!(pending_metadata, "!{0} = !{{!\"llvm.loop.align\", i32 32}}", align_md).ok();
    md_entries.push(format!("!{}", align_md));
    md_count += 1;

    // 2026-08-07 (instance pools): a loop whose body contains an OBSERVABLE
    // call (an observable intrinsic like Print#, an FFI, or an `out`-marked
    // name) must not be folded/unrolled by LLVM — the observable is a liveness
    // root. !llvm.loop.disable_nonforced forbids non-forced transformations.
    if disable_fold {
        let df_md = start + md_count;
        writeln!(pending_metadata, "!{0} = !{{!\"llvm.loop.disable_nonforced\"}}", df_md).ok();
        md_entries.push(format!("!{}", df_md));
        md_count += 1;
    }

    let entries = md_entries.join(", ");
    writeln!(pending_metadata, "!{0} = !{{!{0}, {1}}}", start, entries).ok();
    *metadata_counter += md_count;
    start
}

fn extract_trigger_keys(pre: &Expr, trigger_names: &std::collections::HashSet<&str>) -> Option<Vec<i64>> {
    let mut keys = Vec::new();
    match pre {
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            let (ident, val) = if let (Expr::Identifier(name), Expr::Decimal(n)) = (l.as_ref(), r.as_ref()) {
                (name.clone(), *n)
            } else if let (Expr::Decimal(n), Expr::Identifier(name)) = (l.as_ref(), r.as_ref()) {
                (name.clone(), *n)
            } else {
                return None;
            };
            if trigger_names.contains(ident.as_str()) {
                keys.push(val);
            } else {
                return None;
            }
        }
        Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
            keys.extend(extract_trigger_keys(l, trigger_names)?);
            keys.extend(extract_trigger_keys(r, trigger_names)?);
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            if let Some(k) = extract_trigger_keys(l, trigger_names) {
                keys.extend(k);
            } else if let Some(k) = extract_trigger_keys(r, trigger_names) {
                keys.extend(k);
            } else {
                return None;
            }
        }
        _ => return None,
    }
    keys.sort_unstable();
    keys.dedup();
    if keys.len() < 2 { None } else { Some(keys) }
}

/// Compute the sparsity ratio of a sorted key set.
/// Dense sets (gap ratio < 4) don't need perfect hashing — standard
/// switch dispatch with consecutive offsets works fine.
fn sparsity_ratio(keys: &[i64]) -> f64 {
    if keys.len() < 2 { return 0.0; }
    let gaps: Vec<u64> = keys.windows(2).map(|w| (w[1] - w[0]) as u64).collect();
    let min_gap = *gaps.iter().min().unwrap_or(&1);
    let max_gap = *gaps.iter().max().unwrap_or(&0);
    if min_gap == 0 { return f64::MAX; }
    max_gap as f64 / min_gap as f64
}

/// Find a multiplicative perfect hash for a set of sparse keys.
/// Returns (multiplier, shift) such that h(k) = (k * M) >> S maps
/// each input key to a unique slot in [0, next_power_of_two(n)).
/// Guaranteed termination: capped at 10,000 iterations.
fn find_perfect_hash(keys: &[i64]) -> Option<(u64, u32)> {
    let n = keys.len();
    let num_slots = n.next_power_of_two();
    let shift = 64 - num_slots.trailing_zeros();
    let mut rng: u64 = 123456789;
    for _ in 0..10000 {
        rng = rng.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        let multiplier = rng | 1;
        let mut seen = vec![false; num_slots];
        let mut ok = true;
        for &k in keys {
            let hash = (k.wrapping_mul(multiplier as i64) as u64) >> shift;
            if seen[hash as usize] { ok = false; break; }
            seen[hash as usize] = true;
        }
        if ok { return Some((multiplier, shift)); }
    }
    None
}

/// Action when a `@ *ptr` dynamic trigger target resolves to null.
/// 2026-07-15: Phase 7i — --error-unresolved-trg threads this to codegen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrgUnresolvedAction {
    /// Warn on null target (default — currently a no-op at codegen).
    Warn,
    /// Emit null check + unreachable to abort at runtime.
    Error,
}

/// 2026-08-22 (Phase 7c): LLVM declares its surface honestly — everything
/// EXCEPT the staged port/cell execution (SPEC §9.5/§9.6 interpreter-only).
pub const CAPABILITIES: crate::backend::capabilities::BackendCapabilities = {
    let mut caps = crate::backend::capabilities::BackendCapabilities::full(
        "the native LLVM target",
        "it compiles Briev to machine code through LLVM",
    );
    caps.obj_ports = true;
    caps.cells = false;
    caps
};

pub struct LlvmBackend {
    // ── Context Architecture (Phase 0) ─────────────────────
    //
    // 2026-06-29: Three-tier context separation. CompilerContext holds global
    // read-only state; FunctionContext holds per-function mutable state; the
    // remaining fields on LlvmBackend are orchestration/output accumulators.
    // See context.rs for details.
    pub ctx: CompilerContext,
    pub fun: FunctionContext,

    // ── Optimization ───────────────────────────────────────
    pgo_guard_idx: usize,

    // ── Async / Thread Pool ────────────────────────────────
    pub(crate) has_async_txns: bool,
    /// 2026-09-11 (buffered stdout): `__stdout_flush` is defined in this
    /// program (the print family is linked) — runtime mains call it before
    /// returning so the buffer tail reaches the fd. Bare/no-stdlib programs
    /// never set it, and no call or declare is emitted.
    pub(crate) has_stdout_flush: bool,
    pub(crate) async_txn_names: Vec<String>,
    pub(crate) async_thread_pool_size: u32,
    pub(crate) is_lightweight_async: bool,

    // ── Program Registry ───────────────────────────────────
    program_txns: Vec<String>,
    pub(crate) fused_to_first: HashMap<String, String>,
    pub(crate) sampled_triggers: HashMap<String, String>,
    // 2026-07-31: Phase 3 (§8.3) — write masks are u128 so the 65th+ field is
    // no longer silently dropped; the EMITTED width is i128 when the program
    // has >64 state fields (write_mask_type), else i64.
    pub(crate) txn_write_masks: HashMap<String, u128>,
    cell_thread_names: Vec<String>,

    // ── Reporting & Diagnostics ────────────────────────────
    pub(crate) report_lines: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) remarks: Vec<crate::backend::llvm::directive::OptimizationRemark>,
    llvm_extra_flags: Vec<String>,
    pub(crate) pending_async_await_count: usize,
    // 2026-08-06 (accel plan): frontend accel analysis (SPEC §9.7), copied
    // from AnalysisResults so emit_toplevel can emit dispatch wrappers.
    pub(crate) accel_entries: HashMap<String, crate::analysis::accel::AccelEntry>,
    /// txn name → descriptor index into @briev_accel_descs.
    pub(crate) accel_kernel_idx: HashMap<String, u32>,
    /// True when at least one SPIR-V kernel blob was embedded → link
    /// briev_accel_rt.c.
    pub(crate) has_accel_kernels: bool,

    // ── Arena Allocator (cross-function) ────────────────────
    // 2026-07-19: Arena system field indices in %State. Set during generate(),
    // used by emit_arena_init/emit_arena_alloc to access arena state.
    pub(crate) arena_ptr_idx: Option<usize>,
    pub(crate) arena_end_idx: Option<usize>,
    pub(crate) arena_base_idx: Option<usize>,

    // ── Dynamic Trigger Safety ─────────────────────────────
    // 2026-07-15: Phase 7i — Controls null-check emission for @ *ptr.
    pub(crate) trg_unresolved_action: TrgUnresolvedAction,

    // ── Allocation Strategy Analysis ─────────────────────────
    // 2026-07-18: Pre-computed strategies from analysis pass.
    // Keyed by analysis_id on Expr::Call("Alloc#", ..., Some(id)).
    pub analysis_alloc_strategies: Option<std::collections::HashMap<usize, AllocStrategy>>,

    // ── SVO List Optimization ────────────────────────────────
    // 2026-08-15 (coll plan §3.5): SVO (feature_svo) REMOVED — never enabled
    // in production; lists construct via the coll scaffolded ops.

    // ── Frgn Dispatch Resolution ──────────────────────────────
    // 2026-07-22: Pre-resolved frgn dispatch strategies computed
    // during the main compilation pass. The backend uses these to
    // decide whether to inline a foreign call or emit a bridge call.
    pub(crate) resolved_frgns: Option<std::collections::HashMap<String, crate::analysis::frgn_dispatch::ResolvedFrgn>>,

    // ── Precomputed Frontend Analysis ─────────────────────────
    // 2026-08-23 (Plan 0.1, backend-scaffolding-foundation): the pipeline
    // computes AnalysisResults ONCE (src/compile.rs codegen) so every backend
    // consumes the same frontend decisions; this field carries it into the
    // LLVM path. generate() consumes it when present, else self-computes as
    // before (direct-construction callers — glue/export.rs, unit tests).
    // To undo: delete with_analysis + this field, restore the inline
    // analyze_program call at the top of generate().
    precomputed_analysis: Option<crate::backend::AnalysisResults>,
}

#[derive(Debug, Clone)]
pub struct ChimeraInfo {
    pub is_chimera: bool,
    pub backing_type: String,
}

/// Configuration for embedded (bare-metal) LLVM codegen.
#[derive(Debug, Clone)]
pub struct EmbeddedConfig {
    pub target_triple: String,
    pub linker_script: Option<String>,
    pub cpu: Option<String>,
    pub freestanding: bool,
    pub halt_on_term: bool,
    pub memory_regions: Vec<MemoryRegion>,
    pub interrupts: Vec<InterruptEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    Sequential,
    Parallel,
}

#[derive(Debug, Clone)]
pub struct MemoryRegion {
    pub name: String,
    pub base: u64,
    pub size: u64,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct InterruptEntry {
    pub name: String,
    pub vector: u32,
    pub trg_name: Option<String>,
}

/// Recursively collect push targets from a statement body.
/// In the new AST, arrow push `queue <- val` parses as
/// `Statement::Assign(Expr::Identifier("queue"), val)`.
/// Over-approximation is safe: preallocating a buffer for a
/// non-push field just allocates unused memory (freed at scope end).
/// 2026-07-18: Fixed — was a stub that never extracted any names.
pub(crate) fn collect_push_targets(body: &[Statement], out: &mut Vec<String>) {
    for stmt in body {
        match stmt {
            Statement::Guarded(_, statements) => {
                collect_push_targets(statements, out);
            }
            Statement::Block(body) | Statement::SyncBlock(body) => {
                collect_push_targets(body, out);
            }
            Statement::Assign(Expr::AddrOf(inner), _) => {
                if let Expr::Identifier(name) = inner.as_ref() {
                    out.push(name.clone());
                }
            }
            _ => {}
        }
    }
}

impl LlvmBackend {
    pub fn new() -> Self {
        LlvmBackend {
            ctx: CompilerContext::new(),
            fun: FunctionContext::new(),
            pgo_guard_idx: 0,
            has_async_txns: false,
            has_stdout_flush: false,
            async_txn_names: Vec::new(),
            async_thread_pool_size: 0,
            is_lightweight_async: false,
            program_txns: Vec::new(),
            fused_to_first: HashMap::new(),
            sampled_triggers: HashMap::new(),
            txn_write_masks: HashMap::new(),
            cell_thread_names: Vec::new(),
            report_lines: Vec::new(),
            warnings: Vec::new(),
            remarks: Vec::new(),
            llvm_extra_flags: Vec::new(),
            pending_async_await_count: 0,
            accel_entries: HashMap::new(),
            accel_kernel_idx: HashMap::new(),
            has_accel_kernels: false,
            trg_unresolved_action: TrgUnresolvedAction::Warn,
            arena_ptr_idx: None,
            arena_end_idx: None,
            arena_base_idx: None,
            analysis_alloc_strategies: None,
            resolved_frgns: None,
            precomputed_analysis: None,
        }
    }

    /// 2026-08-23 (Plan 0.1): accept pipeline-computed frontend analysis so
    /// every backend consumes the same `AnalysisResults` (frontend-driven
    /// dispatch pillar). When unset, generate() self-computes — the
    /// historical path used by direct-construction callers.
    pub fn with_analysis(mut self, analysis: crate::backend::AnalysisResults) -> Self {
        self.precomputed_analysis = Some(analysis);
        self
    }

    pub fn with_alloc_strategies(mut self, strategies: std::collections::HashMap<usize, AllocStrategy>) -> Self {
        self.analysis_alloc_strategies = Some(strategies);
        self
    }

    /// 2026-07-27: Set the set of function names that need arena initialization.
    /// Populated by analyze_arena_need before codegen. When empty, arena fields
    /// in %State and all arena init/fini calls are skipped.
    pub fn with_needs_arena(mut self, needs_arena: std::collections::HashSet<String>) -> Self {
        self.ctx.needs_arena = needs_arena;
        self
    }

    // 2026-08-01 (B4): with_sso_strings removed — SSO (Short String
    // Optimization) retired. A String is always a ptr to [len][bytes].

    // 2026-07-18: Set the stack allocation threshold for runtime fallback.
    pub fn with_stack_threshold(mut self, threshold: u64) -> Self {
        self.ctx.stack_threshold = threshold;
        self
    }

    /// 2026-09-02: Minimum work-item count for GPU dispatch. Shapes with
    /// fewer work items fall to the CPU loop at the dispatch gate
    /// (--accel-cpu-fallback N). None = no threshold (default).
    pub fn with_accel_cpu_fallback(mut self, threshold: Option<u64>) -> Self {
        self.ctx.accel_cpu_fallback = threshold;
        self
    }

    pub fn with_spec(mut self, spec: crate::target_spec::TargetSpec) -> Self {
        self.ctx.spec = Some(spec);
        self
    }

    pub fn with_optimize_budget(mut self, budget: u64) -> Self {
        self.ctx.optimize_budget = budget;
        self
    }

    pub fn with_optimize_report(mut self, report: bool) -> Self {
        self.ctx.optimize_report = report;
        self
    }

    // 2026-06-29: Push a field type, recording both the LLVM type string and
    // the original Briev Type. Parallel to field_types/field_briev_types.
    // The Briev Type is needed when reading fields back from %State to
    // distinguish types that share the same LLVM representation (e.g. Char
    // and Int32 both → "i32", Bool and Int8 both → "i8").
    pub(super) fn push_field_type(&mut self, ty: &Type) {
        // 2026-07-17: ALL state fields are stored as i64 in %State, regardless
        // of their Briev type (Float, Float64, Ptr, etc.). The adapt_to_i64 /
        // ensure_typed_value functions handle the conversion between i64 and
        // the field's natural type at load/store time. Override llvm_type(ty)
        // to always return "i64" for state fields — this keeps %State struct
        // layout uniform and avoids type mismatches in codegen paths that
        // assume i64 (load i64, store i64, add i64, icmp i64, etc.).
        // 2026-08-01 (B4): the SSO String branches were retired. A String is a
        // ptr to [len][bytes] under the bits model and stores as ONE i64 slot
        // (the address) via the generic protocol-derived path below — the old
        // 2-slot SSO claim and the is_string_like (2-field structural) check
        // no longer apply (String has no fields under B0).
        // 2026-08-15 (coll plan §3.5): SVO List field slots REMOVED —
        // feature_svo was never enabled in production.
        // 2026-07-25: Fixed-size array: Int[1024] → [1024 x i64].
        // Emitted as a single LLVM array field. Index accesses become GEPs.
        // 2026-08-06: const-sized `Float[MAXB]` yields Dimension::Named(name,
        // 0) — the parser leaves the size unresolved, so resolve it from the
        // program's compile-time constants (populated before build_field_index).
        if let Type::Vector(inner, dims) = ty {
            // 2026-08-07: MULTI-dim — `T[M][N]` becomes `[M x [N x T]]`
            // (the Matrix<T, Rows, Cols> enabler, SPEC §16.6). Each dim
            // resolves from an anonymous count or a compile-time constant.
            if !dims.is_empty() {
                if let Some(arr_ty) = self.vector_array_llvm_type(ty) {
                    self.ctx.field_types.push(arr_ty);
                    self.ctx.field_briev_types.push(ty.clone());
                    return;
                }
            }
        }
        // 2026-07-26: Derive %State field type from protocol + maxbits.
        // Float types get native half/float/double. Exact integer types (Int8..Int128)
        // get native iN width. Everything else (flexible Int, Bool, Ptr, String)
        // stores as i64 — adapt_to_i64/ensure_typed_value handle conversion.
        // 2026-09-02 (plan fundamental-parent-membership): float slots are
        // WIDTH-driven via float_category_bits (casting-graph membership +
        // bits metadata) — Half/Float16 get `half` slots (the old max_bits
        // ladder gave Half a 32-bit float slot) and bare-parent float
        // typedefs resolve through the base chain. Primordial behavior is
        // unchanged (Float → float, Float64/Double → double).
        let llvm_ty = if let Some(bits) = self.float_category_bits(ty) {
            Self::float_spelling(bits).to_string()
        } else if let Some(ref universe) = self.ctx.type_universe {
            if let Some(rt) = ty.universe_key().and_then(|k| universe.get(k)) {
                if rt.min_bits == rt.max_bits && rt.max_bits > 0 {
                    // Exact integer types get native iN width.
                    let bits = if rt.max_bits <= 8 { 8 }
                        else if rt.max_bits <= 16 { 16 }
                        else if rt.max_bits <= 32 { 32 }
                        else if rt.max_bits <= 64 { 64 }
                        else { 128 };
                    format!("i{}", bits)
                } else if rt.properties.contains_key("Cast.Bool")
                    || rt.properties.contains_key("Cast.String")
                    || rt.properties.contains_key("Cast.Blob")
                    || rt.properties.contains_key("Cast.Char")
                {
                    // 2026-08-10: boxed scalar/pointer types stay i64 —
                    // Bool/Char store as boxed i64, String/Data store the
                    // [len][bytes] address. Changing their slot width would
                    // ripple through the boxed-param and ptr adaptation paths.
                    "i64".to_string()
                } else {
                    // 2026-08-10: flexible Int/UInt store at the TARGET int
                    // width (i{int_bits}) — the `--int-bits` design intent.
                    // i32 on wasm32 (avoids BigInt), i64 on x86_64 (identical
                    // to the old hardcoded i64). Exact-width ints took the
                    // branch above; flexible Int/UInt land here. This makes
                    // %State slots match llvm_type(Int)/binop_int_type() and
                    // activates the loop engines' narrow-counter machinery.
                    format!("i{}", self.ctx.int_bits)
                }
            } else {
                "i64".to_string()
            }
        } else {
            "i64".to_string()
        };
        self.ctx.field_types.push(llvm_ty);
        self.ctx.field_briev_types.push(ty.clone());
    }

    pub fn with_optimize_size(mut self, byte_limit: u64) -> Self {
        self.ctx.optimize_size = Some(byte_limit);
        self.ctx.optimize_report = true;
        self
    }

    pub fn with_dead_info_disabled(mut self, disabled: bool) -> Self {
        self.ctx.dead_info_disabled = disabled;
        self
    }

    pub fn with_explain(mut self, explain: bool) -> Self {
        self.ctx.explain = explain;
        self
    }

    /// 2026-07-26: Enable webstack mode (WASM-first rendering).
    /// When enabled, the backend emits __web_flush_state calls at term
    /// and exports state_layout() for the JS runtime shim.
    /// Only meaningful for .rbv files compiled with BackendKind::Webstack.
    pub fn with_webstack(mut self, enabled: bool) -> Self {
        self.ctx.webstack_enabled = enabled;
        self
    }

    /// 2026-08-10: Build the Rust-side StateLayout consumed by the
    /// GlueWebGenerator (JS shim). Mirrors exactly the rows the backend emits
    /// into the WASM `state_layout` table (same handle/offset/size/tag), plus
    /// the field NAME so view bindings can map signal → handle. Call after
    /// generate() — requires field_index_map/field_types/field_briev_types and
    /// the type universe to be populated.
    pub fn web_state_layout(&self, app_name: &str) -> crate::glue::web_generator::StateLayout {
        use crate::glue::web_generator::{FieldLayout, StateLayout, TypeTag};
        // 2026-08-10: field names sorted by handle (field index) for a stable
        // name→handle map — the binding table lookup is by name, so this order
        // only affects iteration determinism, which we keep anyway.
        let mut named: Vec<(usize, String)> = self.ctx.field_index_map.iter()
            .map(|(name, idx)| (*idx, name.clone()))
            .collect();
        named.sort_by_key(|(idx, _)| *idx);
        let mut fields = Vec::new();
        let mut offset = 0u64;
        for (idx, name) in &named {
            let llvm_ty = self.ctx.field_types.get(*idx).cloned().unwrap_or_else(|| "i64".to_string());
            let size = web_llvm_byte_size(&llvm_ty);
            if size == 0 {
                continue; // matches the WASM table's skip rule
            }
            let cat = match self.ctx.type_universe.as_ref() {
                Some(u) => {
                    let briev_ty = self.ctx.field_briev_types.get(*idx).cloned().unwrap_or_else(Type::int);
                    crate::type_universe::protocol_category(u, &briev_ty)
                }
                None => None,
            };
            let tag = TypeTag::from_protocol_category(cat.as_deref());
            // 2026-08-11 (Phase 2a3): for a vector field (`[N x i32]` on
            // wasm32) the shim's b-each renderer needs the per-ELEMENT byte
            // width to derive the item count from the field's total size —
            // the flat size alone can't (Int slots are i32 here, i64 on
            // x86_64). Non-vector fields report size as the element size.
            let element_size = web_vector_element_size(&llvm_ty);
            fields.push(FieldLayout {
                field_handle: *idx as u32,
                name: name.clone(),
                offset: offset as u32,
                size: size as u32,
                type_tag: tag,
                element_size: element_size as u32,
            });
            offset += size;
        }
        StateLayout {
            app_name: app_name.to_string(),
            generation_offset: 0,   // resolved at link time via ptrtoint; shim re-reads at runtime
            flush_buffer_offset: 0, // ditto
            max_flush_entries: self.ctx.web_max_entries.max(1),
            fields,
        }
    }

    /// Pre-populate MMIO address map from a resolved DBV target binding.
    /// Each alias name maps to a physical u64 address for volatile MMIO access.
    pub fn with_mmio_addresses(mut self, addresses: HashMap<String, u64>) -> Self {
        self.ctx.mmio_fields = addresses;
        self.ctx.mmio_prepopulated = true;
        self
    }

    pub fn with_schema_aliases(mut self, aliases: HashSet<String>) -> Self {
        self.ctx.schema_alias_names = aliases;
        self
    }

    pub fn with_pgo_profile(mut self, profile: crate::analysis::pgo::PgoProfile) -> Self {
        self.ctx.pgo_profile = Some(profile);
        self
    }

    pub fn with_emit_remarks(mut self, emit: bool) -> Self {
        self.ctx.emit_remarks = emit;
        self
    }

    /// 2026-07-25: Set the native integer width for Int protocol.
    /// WASM should use 32 to emit i32 instead of i64 (avoid BigInt).
    pub fn with_int_bits(mut self, bits: u64) -> Self {
        self.ctx.int_bits = bits;
        self
    }

    /// 2026-08-11 (2b2 slice 2b): component-instance slot initializers from
    /// mount props (`Counter.0.count` → 5). Merged into field_initializers by
    /// build_field_index so init_state seeds the instance slots.
    pub fn with_component_initializers(
        mut self,
        inits: std::collections::HashMap<String, crate::ast::Expr>,
    ) -> Self {
        self.ctx.component_initializers = inits;
        self
    }

    /// 2026-08-11 (2b2 lifecycle): the component instances `(component,
    /// index)` — emit a per-instance reset export so a b-when unmount can
    /// re-seed the instance's slots.
    pub fn with_trg_unresolved_action(mut self, action: TrgUnresolvedAction) -> Self {
        self.trg_unresolved_action = action;
        self
    }

    pub fn with_embedded_mode(mut self, enabled: bool) -> Self {
        self.ctx.is_embedded = enabled;
        self
    }

    /// 2026-09-06 (plan 2026-09-06-isr-handlers-and-sections.md): the active
    /// target profile's ISR mechanism — the configured default the backend
    /// consumes for mechanism-less `isr` declarations (the typechecker
    /// validated resolution; the backend CONSUMES the decision).
    pub fn with_isr_mechanism(mut self, mechanism: Option<String>) -> Self {
        self.ctx.isr_mechanism = mechanism;
        self
    }

    pub fn with_type_universe(mut self, tu: crate::type_universe::TypeUniverse) -> Self {
        self.ctx.type_universe = Some(tu);
        self
    }

    /// 2026-07-20: Set operator definitions for <- operator dispatch.
    pub fn with_operator_defs(mut self, defs: HashMap<String, Vec<crate::ast::top::OperatorDef>>) -> Self {
        self.ctx.operator_defs = defs;
        self
    }

    /// 2026-07-30: Register CastFrom(Bit) overrides on the casting graph.
    /// Maps type_name → constructor_function_name for constructing a type
    /// from raw memory bits. This is the sole user-extensible cast edge.
    pub fn with_cast_from_bit_overrides(mut self, overrides: HashMap<String, String>) -> Self {
        if let Some(ref mut graph) = self.ctx.casting_graph {
            for (type_name, fn_name) in overrides {
                graph.register_cast_from_bit(&type_name, &fn_name);
            }
        }
        self
    }

    pub fn with_dump_layout(mut self, v: bool) -> Self {
        self.ctx.dump_layout = v;
        self
    }

    pub fn with_library_mode(mut self, v: bool) -> Self {
        self.ctx.library_mode = v;
        self
    }

    /// 2026-07-18: Build a shared library (.so). When true, no main loop
    /// is emitted; only exported wrappers and reactive convergence entry.
    pub fn with_shared_lib(mut self, v: bool) -> Self {
        self.ctx.is_shared_lib = v;
        self
    }

    /// 2026-07-23: Emit Python C extension module init metadata.
    /// When true, generate() emits PyMethodDef[], PyModuleDef, and PyInit_.
    pub fn with_module_init(mut self, v: bool) -> Self {
        self.ctx.module_init = v;
        self
    }

    /// 2026-07-23: Skip emitting the default main() entry point.
    /// The caller provides its own main (e.g., protocol bridge shim).
    pub fn with_no_main(mut self, v: bool) -> Self {
        self.ctx.no_main = v;
        self
    }

    /// 2026-07-22: Provide pre-resolved frgn dispatch strategies.
    /// The backend uses these to decide how to emit foreign calls.
    pub fn with_resolved_frgns(
        mut self,
        map: std::collections::HashMap<String, crate::analysis::frgn_dispatch::ResolvedFrgn>,
    ) -> Self {
        self.resolved_frgns = Some(map);
        self
    }

    /// Set the LLVM target triple for generated IR.
    /// Also updates the data layout to match and derives int_bits from it.
    /// 2026-07-11: Phase 6 — WASM target support.
    /// 2026-07-29: Wire parse_pointer_width to auto-derive int_bits from data layout.
    pub fn with_target_triple(mut self, triple: &str) -> Self {
        self.ctx.target_triple = triple.to_string();
        self.ctx.data_layout = match triple {
            "wasm32-unknown-wasi" | "wasm32-unknown-unknown" => {
                Some("e-m:e-p:32:32-p10:8:8-p20:8:8-i64:64-n32:64-S128-ni:1:10:20".to_string())
            }
            _ => {
                // Default x86_64 data layout
                Some("e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128".to_string())
            }
        };
        if let Some(ref dl) = self.ctx.data_layout {
            self.ctx.int_bits = CompilerContext::parse_pointer_width(dl);
        }
        self
    }

    /// Set the LLVM data layout string (overrides the auto-derived layout).
    /// Also derives int_bits from the data layout's pointer width.
    /// 2026-07-15: Phase 7 — config-driven from targets.dbvl.
    /// 2026-07-29: Wire parse_pointer_width to auto-derive int_bits from data layout.
    pub fn with_data_layout(mut self, dl: &str) -> Self {
        self.ctx.data_layout = Some(dl.to_string());
        self.ctx.int_bits = CompilerContext::parse_pointer_width(dl);
        self
    }

    /// Emit an inttoptr instruction with the correct pointer-width integer type.
    /// Uses int_bits (from data layout or CLI --int-bits) for the integer side.
    /// 2026-07-27: DataLayout-driven int_bits replaces pointer_llvm_type().
    pub(super) fn emit_inttoptr(&self, out: &mut String, indent: &str, dest: &dyn Display, src: &dyn Display) {
        let int_ty = format!("i{}", self.ctx.int_bits);
        writeln!(out, "{}{} = inttoptr {} {} to ptr", indent, dest, int_ty, src).ok();
    }

    /// Produce a human-readable layout summary for all state fields.
    pub fn dump_layout_str(&self) -> String {
        let mut out = String::new();
        out.push_str("\n=== Field Layout ===\n");
        let mut field_names: Vec<&String> = self.ctx.field_index_map.keys().collect();
        field_names.sort();
        for name in &field_names {
            let idx = self.ctx.field_index_map.get(*name).copied().unwrap_or(0);
            let ty = self.ctx.field_types.get(idx).map(|s| s.as_str()).unwrap_or("?");
            let mode = self.ctx.field_modes.get(*name)
                .map(|m| format!("{:?}", m))
                .unwrap_or_else(|| "Always".to_string());
            out.push_str(&format!("  {} @[{}]: {} (mode: {})", name, idx, ty, mode));
            if let Some(targets) = self.ctx.cache_slots.get(*name) {
                for (target_name, &(cache_idx, valid_idx)) in targets {
                    out.push_str(&format!(" | cache[{}]: [{}](i64), [{}](i8)", target_name, cache_idx, valid_idx));
                }
            }
            out.push('\n');
        }
        out.push_str("=== End Layout ===\n");
        out
    }

    /// Append embedded SPIR-V blobs to the output IR string.
    /// Called at the end of `generate()` after all transactions are emitted.
    /// Emit a bump allocation from the active arena, or fall back to @malloc
    /// if no arena is active. Returns the register name holding the allocated
    /// i8* pointer.
    /// Emit preallocation for a single collection field within a bounded loop.
    /// Shared by both emit_prealloc_for_body and emit_prealloc_for_targets.
    fn emit_prealloc_one_field(&mut self, out: &mut String, indent: &str, field_name: &str, bound_reg: &str) {
        if !self.ctx.field_index_map.contains_key(field_name) {
            return;
        }
        let idx = self.ctx.field_index_map[field_name];
        if self.ctx.field_types[idx] != "i64" {
            return;
        }

        let c = self.fun.arena_counter;
        self.fun.arena_counter += 1;
        let cap = format!("%pcap_{}", c);
        writeln!(out, "{}{} = add i64 0, {}", indent, cap, bound_reg).ok();
        let slot_cnt = format!("%psc_{}", c);
        writeln!(out, "{}{} = add i64 {}, 2", indent, slot_cnt, cap).ok();
        let alloc_sz = format!("%psz_{}", c);
        writeln!(out, "{}{} = mul i64 {}, 8", indent, alloc_sz, slot_cnt).ok();
        let buf_reg = self.emit_arena_alloc(out, indent, &alloc_sz);
        let buf_i64 = format!("%pbp_{}", c);
        // 2026-07-19: emit_arena_alloc returns i64 — inttoptr to get ptr.
        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, buf_i64, buf_reg).ok();
        let base = format!("%pba_{}", c);
        // 2026-07-19: buf_reg is already i64 from emit_arena_alloc.
        writeln!(out, "{}{} = add i64 0, {}", indent, base, buf_reg).ok();
        let data_ptr = format!("%pdv_{}", c);
        writeln!(out, "{}{} = add i64 {}, 16", indent, data_ptr, base).ok();
        let s0 = format!("%ps0_{}", c);
        writeln!(out, "{}{} = getelementptr i64, ptr {}, i64 0", indent, s0, buf_i64).ok();
        writeln!(out, "{}store i64 {}, ptr {}, align 8, !tbaa !1", indent, data_ptr, s0).ok();
        let s1 = format!("%ps1_{}", c);
        writeln!(out, "{}{} = getelementptr i64, ptr {}, i64 1", indent, s1, buf_i64).ok();
        writeln!(out, "{}store i64 0, ptr {}, align 8, !tbaa !1", indent, s1).ok();
        let ap = self.emit_state_gep(out, indent, "pap", "%state", idx);
        let tn = crate::backend::llvm::tbaa_node(&self.ctx.field_types[idx], self.ctx.type_universe.as_ref());
        writeln!(out, "{}store i64 {}, ptr {}, align 8, !tbaa !{}", indent, base, ap, tn).ok();
        self.fun.field_prealloc_info.insert(field_name.to_string(), (cap, buf_i64));
    }

    /// Emit preallocation for collection fields that receive `<- push` within
    /// a bounded loop. Scans body for push targets via collect_push_targets.
    pub(crate) fn emit_prealloc_for_body(
        &mut self,
        out: &mut String,
        indent: &str,
        body: &[Statement],
        bound_reg: &str,
    ) {
        let mut push_targets: Vec<String> = Vec::new();
        collect_push_targets(body, &mut push_targets);
        push_targets.sort();
        push_targets.dedup();
        for field_name in &push_targets {
            self.emit_prealloc_one_field(out, indent, field_name, bound_reg);
        }
    }

    /// Emit preallocation for a pre-collected list of push target field names.
    pub(crate) fn emit_prealloc_for_targets(
        &mut self,
        out: &mut String,
        indent: &str,
        push_targets: &[String],
        bound_reg: &str,
    ) {
        let mut targets: Vec<String> = push_targets.to_vec();
        targets.sort();
        targets.dedup();
        for field_name in &targets {
            self.emit_prealloc_one_field(out, indent, field_name, bound_reg);
        }
    }


/// 2026-09-10 (Family E, allocator ownership): emit an INLINE `brk` syscall
/// (SYS_brk = 12) — the libc-free arena primitive. Same inline-asm shape as
/// the SysCall# emission (intrinsics.rs). Returns false when the target has
/// no brk lowering (caller keeps the libc fallback).
pub(crate) fn emit_brk_syscall(&mut self, out: &mut String, v: &str, arg_reg: &str, indent: &str) -> bool {
    let triple = self.ctx.target_triple.clone();
    if triple.starts_with("x86_64") && triple.contains("linux") {
        writeln!(out, "{}{} = call i64 asm sideeffect \"syscall\", \"={{rax}},{{rax}},{{rdi}},~{{rcx}},~{{r11}}\" (i64 12, i64 {})",
            indent, v, arg_reg).ok();
        true
    } else if triple.starts_with("aarch64") && triple.contains("linux") {
        writeln!(out, "{}{} = call i64 asm sideeffect \"svc #0\", \"={{x0}},{{x8}},{{x0}}\" (i64 12, i64 12, i64 {})",
            indent, v, arg_reg).ok();
        true
    } else {
        false
    }
}

    pub(crate) fn emit_arena_alloc(&mut self, out: &mut String, indent: &str, size_reg: &str) -> String {
        // 2026-07-19: Bump-pointer arena allocation via %State fields.
        // Uses next_reg_with_prefix (no closures — avoids borrow conflicts).

        // 2026-07-22: Low budget → direct malloc (simpler IR, faster compile).
        // The --optimize-budget flag (default 256) controls simulation depth;
        // below the config arena_min_budget, skip the bump arena entirely and
        // use heap allocation.
        // 2026-07-31: Phase 3 (§8.2) — threshold from config/ir-lowering.toml.
        if (self.ctx.optimize_budget as u32) < crate::config_tuning::ir_lowering().arena_min_budget {
            let r = self.fun.next_reg_with_prefix("aam");
            writeln!(out, "{}{} = call noalias ptr @malloc(i64 {})", indent, r, size_reg).ok();
            let ri = self.fun.next_reg_with_prefix("aami");
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, ri, r).ok();
            return ri;
        }

        let Some(aptr_idx) = self.arena_ptr_idx else {
            let r = self.fun.next_reg_with_prefix("aam");
            writeln!(out, "{}{} = call noalias ptr @malloc(i64 {})", indent, r, size_reg).ok();
            let ri = self.fun.next_reg_with_prefix("aami");
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, ri, r).ok();
            return ri;
        };
        let aend_idx = self.arena_end_idx.unwrap();
        let abase_idx = self.arena_base_idx.unwrap();

        // Helper: emit GEP+load+inttoptr for an arena %State field → returns ptr reg.
        macro_rules! load_state_ptr { ($idx:expr, $pfx:expr) => {{
            let _gep = self.fun.next_reg_with_prefix(concat!($pfx, "g"));
            writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
                indent, _gep, $idx).ok();
            let _vi = self.fun.next_reg_with_prefix(concat!($pfx, "i"));
            writeln!(out, "{}{} = load i64, ptr {}", indent, _vi, _gep).ok();
            let _vp = self.fun.next_reg_with_prefix(concat!($pfx, "p"));
            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, _vp, _vi).ok();
            _vp
        }}; }
        // Helper: emit GEP+ptrtoint+store for an arena %State field.
        macro_rules! store_state_ptr { ($idx:expr, $pfx:expr, $ptr:expr) => {{
            let _gep = self.fun.next_reg_with_prefix(concat!($pfx, "g"));
            writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
                indent, _gep, $idx).ok();
            let _pi = self.fun.next_reg_with_prefix(concat!($pfx, "pi"));
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, _pi, $ptr).ok();
            writeln!(out, "{}store i64 {}, ptr {}, align 8", indent, _pi, _gep).ok();
        }}; }

        let ok_l_n = self.fun.txn_counter;
        self.fun.txn_counter += 1;
        let check_l = format!("aacheck_{}", ok_l_n);
        let grow_l = format!("aagrow_{}", ok_l_n);

        // Load current ptr and end from %State
        let cur = load_state_ptr!(aptr_idx, "aac");
        let end = load_state_ptr!(aend_idx, "aae");

        writeln!(out, "{}br label %{}", indent, check_l).ok();
        writeln!(out, "{}{}:", indent, check_l).ok();
        let new_ptr = self.fun.next_reg_with_prefix("aanew");
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, new_ptr, cur, size_reg).ok();
        let ok_val = self.fun.next_reg_with_prefix("aaok");
        writeln!(out, "{}{} = icmp ule ptr {}, {}", indent, ok_val, new_ptr, end).ok();
        writeln!(out, "{}br i1 {}, label %aaok_{}, label %{}", indent, ok_val, ok_l_n, grow_l).ok();

        // Grow path
        writeln!(out, "{}{}:", indent, grow_l).ok();
        // 2026-08-04 (Phase 4): embedded freestanding has NO realloc — the
        // static bump heap is fixed-size. A bump past the end yields null
        // (the allocator contract: null ⇒ allocation failed); the caller's
        // bounds checks handle it. No @realloc/@free on bare metal.
        let grow_incoming;
        if self.ctx.is_embedded {
            grow_incoming = "null".to_string();
        } else {
            // 2026-09-10 (Family E, allocator ownership): libc-free growth.
            // The brk-backed arena EXTENDS IN PLACE — `brk(end + min_sz)`
            // moves the break; the base never moves and the copied data
            // stays put (zero-copy, unlike @realloc). The bump restarts at
            // base — the same grow contract as the realloc path. Fallback:
            // @realloc for targets without brk lowering.
            let end_g = load_state_ptr!(aend_idx, "aaeg");
            let end_i64g = self.fun.next_reg_with_prefix("aagi");
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, end_i64g, end_g).ok();
            let grow_sz = self.fun.next_reg_with_prefix("aags");
            writeln!(out, "{}{} = shl i64 {}, 1", indent, grow_sz, size_reg).ok();
            let min_sz = self.fun.next_reg_with_prefix("aams");
            writeln!(out, "{}{} = add i64 {}, {}", indent, min_sz, grow_sz, self.ctx.arena_initial_size).ok();
            let want = self.fun.next_reg_with_prefix("aabw");
            writeln!(out, "{}{} = add i64 {}, {}", indent, want, end_i64g, min_sz).ok();
            let ok = self.fun.next_reg_with_prefix("aabo");
            let have_brk = self.emit_brk_syscall(out, &ok, &want, indent);
            if have_brk {
                let new_end = self.fun.next_reg_with_prefix("aane");
                writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, new_end, end_g, min_sz).ok();
                store_state_ptr!(aend_idx, "aaes", &new_end);
                let base = load_state_ptr!(abase_idx, "aaob");
                store_state_ptr!(aptr_idx, "aaps", &base);
                grow_incoming = base;
            } else {
                let base = load_state_ptr!(abase_idx, "aaob");
                let grow_sz2 = self.fun.next_reg_with_prefix("aags2");
                writeln!(out, "{}{} = shl i64 {}, 1", indent, grow_sz2, size_reg).ok();
                let min_sz2 = self.fun.next_reg_with_prefix("aams2");
                writeln!(out, "{}{} = add i64 {}, {}", indent, min_sz2, grow_sz2, self.ctx.arena_initial_size).ok();
                let new_base = self.fun.next_reg_with_prefix("aanb");
                writeln!(out, "{}{} = call ptr @realloc(ptr {}, i64 {})", indent, new_base, base, min_sz2).ok();
                store_state_ptr!(aptr_idx, "aaps", &new_base);
                store_state_ptr!(abase_idx, "aabs", &new_base);
                let new_end = self.fun.next_reg_with_prefix("aane");
                writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, new_end, new_base, min_sz2).ok();
                store_state_ptr!(aend_idx, "aaes", &new_end);
                grow_incoming = new_base;
            }
            writeln!(out, "{}br label %aaok_{}", indent, ok_l_n).ok();
        }

        // OK path
        writeln!(out, "aaok_{}:", ok_l_n).ok();
        // 2026-08-01 (D): the arena realloc-grow inserts control flow (check /
        // grow / ok) inside whatever block the Alloc# sits in. The countdown's
        // latch phis use cur_block as the body predecessor — it must point at
        // the REAL final block (aaok_), not the block the Alloc# started in.
        self.fun.cur_block = Some(format!("aaok_{}", ok_l_n));
        let phi = self.fun.next_reg_with_prefix("aaphi");
        writeln!(out, "{}{} = phi ptr [ {}, %{} ], [ {}, %{} ]",
            indent, phi, cur, check_l, grow_incoming, grow_l).ok();
        let new_bump = self.fun.next_reg_with_prefix("aanbp");
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, new_bump, phi, size_reg).ok();
        store_state_ptr!(aptr_idx, "aaps2", &new_bump);
        let result = self.fun.next_reg_with_prefix("aapi");
        writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, result, phi).ok();
        result
    }

    /// Emit arena initialization at scope entry. Allocates the initial
    /// 64KB arena buffer, sets up ptr/end/base alloca slots.
    pub(crate) fn emit_arena_init(&mut self, out: &mut String, indent: &str) {
        // 2026-07-27: Global gate — if no function in the program needs arena,
        // skip emission entirely. The field-injection guard ensures arena_ptr_idx
        // is None when needs_arena is empty, but this early return avoids even
        // entering the function dispatch logic.
        if self.ctx.needs_arena.is_empty() {
            return;
        }
        let Some(aptr_idx) = self.arena_ptr_idx else { return; };
        let Some(aend_idx) = self.arena_end_idx else { return; };
        let Some(abase_idx) = self.arena_base_idx else { return; };
        // 2026-07-19: Uses next_reg_with_prefix (txn_counter) for unique names.
        // Eliminates arena_counter — consistent with emit_arena_alloc pattern.
        if self.has_async_txns {
            writeln!(out, "{}%arena_mutex = alloca i64, align 8", indent).ok();
            writeln!(out, "{}store i64 0, ptr %arena_mutex, align 8", indent).ok();
        }
        // 2026-08-04 (Phase 4): embedded freestanding — point the bump pointer
        // at the static @embedded_heap global (no @malloc, no heap growth).
        if self.ctx.is_embedded {
            let base = self.fun.next_reg_with_prefix("arib");
            writeln!(out, "{}{} = ptrtoint ptr @embedded_heap to i64", indent, base).ok();
            self.emit_state_store_i64_by_idx(out, indent, aptr_idx, &base);
            self.emit_state_store_i64_by_idx(out, indent, abase_idx, &base);
            let init_end = self.fun.next_reg_with_prefix("arie");
            writeln!(out, "{}{} = getelementptr i8, ptr @embedded_heap, i64 {}",
                indent, init_end, self.ctx.arena_initial_size).ok();
            let end_i64 = self.fun.next_reg_with_prefix("ariei");
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, end_i64, init_end).ok();
            self.emit_state_store_i64_by_idx(out, indent, aend_idx, &end_i64);
            return;
        }
        // 2026-09-10 (Family E, allocator ownership): the hosted arena is
        // brk-backed — INLINE syscall, no libc. base = brk(0); brk(base +
        // initial) extends the break; the region never moves (zero-copy
        // growth later). The @malloc fallback remains for targets without
        // brk lowering (non-linux).
        let init_i64 = self.fun.next_reg_with_prefix("arii");
        let b0 = self.fun.next_reg_with_prefix("arb0");
        let have_brk = self.emit_brk_syscall(out, &b0, "0", indent);
        if have_brk {
            let want = self.fun.next_reg_with_prefix("arbw");
            writeln!(out, "{}{} = add i64 {}, {}", indent, want, b0, self.ctx.arena_initial_size).ok();
            let ok = self.fun.next_reg_with_prefix("arbo");
            self.emit_brk_syscall(out, &ok, &want, indent);
            writeln!(out, "{}{} = add i64 0, {}", indent, init_i64, b0).ok();
        } else {
            let init = self.fun.next_reg_with_prefix("arinit");
            writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, init, self.ctx.arena_initial_size).ok();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, init_i64, init).ok();
        }
        // Store arena_ptr, arena_base, arena_end
        self.emit_state_store_i64_by_idx(out, indent, aptr_idx, &init_i64);
        self.emit_state_store_i64_by_idx(out, indent, abase_idx, &init_i64);
        let end_i64 = self.fun.next_reg_with_prefix("arie");
        writeln!(out, "{}{} = add i64 {}, {}", indent, end_i64, init_i64, self.ctx.arena_initial_size).ok();
        self.emit_state_store_i64_by_idx(out, indent, aend_idx, &end_i64);
    }

    /// Emit arena reset: rewinds the bump pointer to the base, preserving
    /// the allocated memory for reuse in the next scope iteration.
    /// 2026-07-19: Arena state is in %State — reload base from there.
    pub(crate) fn emit_arena_reset(&mut self, out: &mut String, indent: &str) {
        // 2026-07-27: Global gate — skip if no function needs arena.
        if self.ctx.needs_arena.is_empty() {
            return;
        }
        let Some(aptr_idx) = self.arena_ptr_idx else { return; };
        let Some(abase_idx) = self.arena_base_idx else { return; };
        let (base_val, _) = self.emit_state_load_i64_by_idx(out, indent, abase_idx);
        let base_ptr = self.fun.next_reg_with_prefix("arbp");
        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, base_ptr, base_val).ok();
        self.emit_state_store_i64_by_idx(out, indent, aptr_idx, &base_val);
    }

    /// Emit arena teardown at program exit. Frees the arena buffer.
    pub(crate) fn emit_arena_fini(&mut self, out: &mut String, indent: &str) {
        // 2026-07-27: Global gate — skip if no function needs arena.
        if self.ctx.needs_arena.is_empty() {
            return;
        }
        let Some(abase_idx) = self.arena_base_idx else { return; };
        // 2026-08-04 (Phase 4): embedded freestanding — the static @embedded_heap
        // global lives for the program's lifetime; nothing to free.
        if self.ctx.is_embedded {
            return;
        }
        let (base_val, _) = self.emit_state_load_i64_by_idx(out, indent, abase_idx);
        let base_ptr = self.fun.next_reg_with_prefix("afp");
        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, base_ptr, base_val).ok();
        writeln!(out, "{}call void @free(ptr {})", indent, base_ptr).ok();
    }

    // 2026-07-18: Check if we're in a scope with a known contract-proven bound.
    // Used by Alloc# to decide between arena bump vs alloca vs @malloc.
    pub(crate) fn is_in_bounded_scope(&self) -> bool {
        self.fun.is_static_bound
    }

    // 2026-07-18: Check if the allocation result escapes the current scope.
    // Simplified: checks if the result register is stored to %State or returned.
    // TEMP: Always returns false (assume no escape) until Phase 4 (Ptr Level 3
    // borrow checker) provides provenance-based escape analysis.
    pub(crate) fn will_escape_current_allocation(&self) -> bool {
        false
    }

    /// Emit Python C extension module init metadata: PyMethodDef[],
    /// PyModuleDef, and PyInit_<name>. The struct layouts are read from
    /// the type universe (injected by InjectTypeLayout$ at compile time).
    /// 2026-07-23: Config-driven, zero Rust per-language code.
    pub(crate) fn push_remark(&mut self, remark: crate::backend::llvm::directive::OptimizationRemark) {
        if self.ctx.emit_remarks {
            self.remarks.push(remark);
        }
    }

    pub fn remarks(&self) -> &[crate::backend::llvm::directive::OptimizationRemark] {
        &self.remarks
    }

    /// Scan the typed program for constructs that are forbidden in embedded mode.
    ///
    /// 2026-08-04 (Phase 4, .ebv heap reframe): heap types (String/Blob/List/
    /// HashMap/…) are now LEGAL on the embedded target — the static bump arena
    /// (@embedded_heap) provides a heap without @malloc/briev_rt.c. The old
    /// hard rejection was a vestige of the pre-split .ebv/.sbv entanglement
    /// (the .sbv CIRCT target synthesizes hardware and truly has no heap; .ebv
    /// is LLVM embedded and does). We still WARN when the program uses heap
    /// types so a bare-metal developer knows the static arena is finite, but
    /// it is not a TargetError. Threading intrinsics remain forbidden (bare
    /// metal has no threads) and unbounded recursion still warns (no stack
    /// growth).
    pub(crate) fn check_embedded_restrictions(&mut self, items: &[TopLevel]) {
        let threading_intrinsics: &[&str] = &[
            "ThreadCreate#", "ThreadJoin#", "ThreadExit#",
            "MutexLock#", "MutexUnlock#",
            "CondvarWait#", "CondvarSignal#", "CondvarBroadcast#",
        ];
        for item in items {
            match item {
                TopLevel::StateDecl(decl) => {
                    if self.type_is_heap_allocated(&decl.ty) {
                        self.warnings.push(format!(
                            "TargetWarning: heap-typed state '{}' ({:?}) on 'Embedded' uses the static bump arena (@embedded_heap, {} bytes) — finite, no free until arena reset",
                            decl.name, decl.ty, self.ctx.arena_initial_size
                        ));
                    }
                }
                TopLevel::Transaction(txn) => {
                    for stmt in &txn.body {
                        self.check_stmt_embedded(stmt, &txn.name, threading_intrinsics);
                    }
                }
                TopLevel::Definition(defn) => {
                    for stmt in &defn.body {
                        self.check_stmt_embedded(stmt, &defn.name, threading_intrinsics);
                    }
                }
                _ => {}
            }
        }
        if self.ctx.has_cycles {
            self.warnings.push(
                "TargetWarning: unbounded recursion detected — call graph has cycles, which are not supported on target 'Embedded'".to_string()
            );
        }
    }

    fn type_is_heap_allocated(&self, ty: &Type) -> bool {
        // 2026-08-01 (B4): is_string_like (2-field structural) retired —
        // protocol membership only. A String value is a ptr to a
        // heap-allocated [len][bytes] buffer (allocated at init/FFI time).
        // Blob values are also pointers. UTF8View/StaticString/SmallString64
        // (legacy stack types) are retired.
        if self.is_protocol_member(ty, "String") {
            return true;
        }
        if self.is_protocol_member(ty, "Blob") {
            return true;
        }
        ty.universe_key()
            .and_then(|k| self.ctx.type_universe.as_ref().and_then(|u| u.get(k)))
            .map(|rt| rt.properties.contains_key("Cast.HeapAllocated"))
            .unwrap_or(false)
    }

    fn check_stmt_embedded(&mut self, stmt: &Statement, ctx_name: &str, threading_intrinsics: &[&str]) {
        match stmt {
            Statement::Let { name, ty, expr, .. } => {
                if let Some(t) = ty {
                    if self.type_is_heap_allocated(t) {
                        // 2026-08-04 (Phase 4): heap types are legal now (static
                        // bump arena) — warn, don't error, mirroring the StateDecl
                        // downgrade.
                        self.warnings.push(format!(
                            "TargetWarning: heap-typed local '{}' in '{}' ({:?}) uses the static bump arena (@embedded_heap)",
                            name, ctx_name, t
                        ));
                    }
                }
                if let Some(e) = expr {
                    self.check_expr_embedded(e, ctx_name, threading_intrinsics);
                }
            }
            Statement::Assign(_, expr) => {
                self.check_expr_embedded(expr, ctx_name, threading_intrinsics);
            }
            Statement::Expression(e) => {
                self.check_expr_embedded(e, ctx_name, threading_intrinsics);
            }
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                self.check_expr_embedded(e, ctx_name, threading_intrinsics);
            }
            Statement::Term(None) | Statement::EndProgram(None) => {}
            Statement::Guarded(condition, statements) => {
                self.check_expr_embedded(condition, ctx_name, threading_intrinsics);
                for s in statements {
                    self.check_stmt_embedded(s, ctx_name, threading_intrinsics);
                }
            }
            Statement::Block(body) | Statement::SyncBlock(body) => {
                for s in body { self.check_stmt_embedded(s, ctx_name, threading_intrinsics); }
            }
            Statement::Foreach { list, body, .. } => {
                self.check_expr_embedded(list, ctx_name, threading_intrinsics);
                for s in body {
                    self.check_stmt_embedded(s, ctx_name, threading_intrinsics);
                }
            }
            Statement::Rollback(Some(e)) => {
                self.check_expr_embedded(e, ctx_name, threading_intrinsics);
            }
            Statement::Rollback(None) => {}
            _ => {}
        }
    }

    fn check_expr_embedded(&mut self, expr: &Expr, ctx_name: &str, threading_intrinsics: &[&str]) {
        match expr {
            Expr::Call(name, args, _) => {
                if threading_intrinsics.contains(&name.as_str()) {
                    self.warnings.push(format!(
                        "TargetWarning: threading intrinsic not supported on target 'Embedded' — '{}' in '{}'",
                        name, ctx_name
                    ));
                }
                for arg in args {
                    self.check_expr_embedded(arg, ctx_name, threading_intrinsics);
                }
            }
            Expr::BinaryOp(_, l, r) => {
                self.check_expr_embedded(l, ctx_name, threading_intrinsics);
                self.check_expr_embedded(r, ctx_name, threading_intrinsics);
            }
            Expr::UnaryOp(_, inner) | Expr::Cast(inner, _) | Expr::IsType(inner, _) => {
                self.check_expr_embedded(inner, ctx_name, threading_intrinsics);
            }
            Expr::Field(target, _) | Expr::Index(target, _) => {
                self.check_expr_embedded(target, ctx_name, threading_intrinsics);
            }
            Expr::Block(stmts) => {
                for s in stmts {
                    self.check_stmt_embedded(s, ctx_name, threading_intrinsics);
                }
            }
            Expr::If(cond, then_b, else_b) => {
                self.check_expr_embedded(cond, ctx_name, threading_intrinsics);
                self.check_expr_embedded(then_b, ctx_name, threading_intrinsics);
                if let Some(eb) = else_b {
                    self.check_expr_embedded(eb, ctx_name, threading_intrinsics);
                }
            }
            Expr::Match(value, arms) => {
                self.check_expr_embedded(value, ctx_name, threading_intrinsics);
                for arm in arms {
                    self.check_expr_embedded(&arm.body, ctx_name, threading_intrinsics);
                }
            }
            Expr::Tuple(elems) | Expr::List(elems) => {
                for e in elems {
                    self.check_expr_embedded(e, ctx_name, threading_intrinsics);
                }
            }
            Expr::Lambda(_, body) => {
                self.check_expr_embedded(body, ctx_name, threading_intrinsics);
            }
            Expr::Spawn { args, .. } => {
                for a in args {
                    self.check_expr_embedded(a, ctx_name, threading_intrinsics);
                }
            }
            Expr::Within(body, fallback) => {
                self.check_expr_embedded(body, ctx_name, threading_intrinsics);
                self.check_expr_embedded(fallback, ctx_name, threading_intrinsics);
            }
            Expr::DerivationBlock(db) => {
                for ex in &db.examples {
                    for inp in &ex.inputs {
                        self.check_expr_embedded(inp, ctx_name, threading_intrinsics);
                    }
                    self.check_expr_embedded(&ex.output, ctx_name, threading_intrinsics);
                }
            }
            Expr::Named { inner, .. } => {
                self.check_expr_embedded(inner, ctx_name, threading_intrinsics);
            }
            Expr::UnitLiteral { .. } => {}
            _ => {}
        }
    }

    pub fn generate(&mut self, items: &[TopLevel], exit_condition: Option<Box<Expr>>) -> String {
        // 2026-09-11 (buffered stdout): gate the epilogue flush on the
        // stdlib lane actually being present — bare programs get no call.
        self.has_stdout_flush = items.iter().any(|i| {
            matches!(i, TopLevel::Definition(d) if d.name == "__stdout_flush")
        });
        // 2026-07-31: Phase 3 (§8.1) — warn once when the target triple's prefix
        // is unknown to config/targets.dbvl, so the x86_64 tuning fallback is
        // never applied silently to a foreign target.
        if !crate::config_tuning::known_target_triple(&self.ctx.target_triple) {
            self.warnings.push(format!(
                "warning: target triple '{}' has no [target.<prefix>] entry in \
                 config/targets.dbvl — using x86_64 tuning defaults",
                self.ctx.target_triple
            ));
        }
        // 2026-07-31: Phase 3 (§8.5-E6) — surface normalizer diagnostics (silent
        // size/width/alignment fallbacks recorded on the universe) into the
        // backend warning report so they are never silent.
        if let Some(u) = self.ctx.type_universe.as_ref() {
            for w in &u.warnings {
                self.warnings.push(w.clone());
            }
        }
        // 2026-08-23 (Plan 0.1): consume the pipeline-computed analysis when
        // the caller provided it (.with_analysis); otherwise self-compute —
        // identical inputs, identical result, so both paths agree.
        let mut analysis = match self.precomputed_analysis.take() {
            Some(a) => a,
            None => crate::backend::analyze_program(
                items,
                false,
                // 2026-07-31: Phase 3 (§8.1) — vector-phi promotion gate from
                // config/targets.dbvl `vector_min_width` for this target.
                crate::config_tuning::target_settings_for(&self.ctx.target_triple).vector_min_width,
                // 2026-08-06 (accel plan): the populated TypeUniverse for the
                // flat-type proof in src/analysis/accel.rs (rule 18 — never name
                // matching). Built by the normalizer before the backend runs.
                self.ctx.type_universe.as_ref(),
            ),
        };
        self.ctx.dep_graph = analysis.dependency_graph.clone();
        self.ctx.global_free_after = analysis.global_lifetime.free_after.clone();
        self.ctx.observable_names = analysis.observable_names.clone();
        self.ctx.coll_safe_txns = analysis.coll_safe_txns.clone();
        self.ctx.coll_pregrow = analysis.coll_pregrow.clone();
        // 2026-08-31 (plan abv-gpu-by-default): pre-register the accel kernel
        // index BEFORE host emission. The dispatch wrapper is decided at
        // txn-emission time via accel_kernel_idx — with kernel collection at
        // the END of generate() that map was always empty and NO GPU dispatch
        // wrapper was ever emitted: programs silently ran CPU-only. Candidate
        // order = sorted names = the deterministic descriptor order below.
        // To undo: revert to registering accel_kernel_idx after collection
        // (and accept that no wrapper is ever emitted).
        self.accel_entries = analysis.accel.clone();
        {
            let mut candidates: Vec<&String> = self
                .accel_entries
                .iter()
                .filter(|(_, e)| {
                    e.shape.eligible
                        && matches!(
                            e.decision,
                            crate::analysis::accel::AccelDecision::Gpu
                                | crate::analysis::accel::AccelDecision::Probe
                        )
                })
                .map(|(k, _)| k)
                .collect();
            candidates.sort();
            self.accel_kernel_idx = candidates
                .iter()
                .enumerate()
                .map(|(i, k)| ((*k).clone(), i as u32))
                .collect();
        }
        // 2026-09-01 (Track B): resident-launch soundness gate — the whole
        // program goes resident only when every array any kernel touches is
        // kernel-pinned (all-readers-are-kernels). Mixed resident/full-copy
        // is unsound (full-copy packs the stale staging into VRAM).
        {
            let info = crate::analysis::accel::build_program_info(items);
            self.ctx.accel_resident_ok = crate::analysis::accel::analyze_resident_safety(
                items,
                &analysis.accel,
                &info,
                self.ctx.type_universe.as_ref().unwrap_or(&crate::type_universe::TypeUniverse::new()),
            )
            .resident_ok;
        }
        // 2026-08-26 (async Phase C): consume frontend segmentation — spawn
        // targets lower to segment continuations over the C task table.
        let defn_param_pairs: HashMap<String, Vec<(String, Type)>> = items
            .iter()
            .filter_map(|i| match i {
                TopLevel::Definition(d) => Some((d.name.clone(), d.parameters.clone())),
                _ => None,
            })
            .collect();
        self.ctx.task_segments = analysis
            .task_segments
            .iter()
            .filter_map(|(name, segs)| {
                defn_param_pairs
                    .get(name)
                    .map(|params| (name.clone(), (segs.clone(), params.clone())))
            })
            .collect();
        // 2026-08-03: Per-export ABI (needs_state) computed once up front by
        // the export ABI analysis — the backend only consumes the decision.
        // 2026-08-04 (compiler-in-Briev, P4): the Briev pass (briev_pass.rs)
        // computes it through the GLUE C ABI when its library is present;
        // otherwise the Rust reference runs.
        self.ctx.export_needs_state =
            crate::glue::briev_pass::compute_export_needs_state(items);
        // 2026-08-01 (Phase 5): a `keep x;` on a field the scheduler would not
        // auto-free anyway is redundant — surface it as a warning.
        for k in &analysis.global_lifetime.redundant_keeps {
            self.warnings.push(format!(
                "warning: 'keep {};' is redundant — the scheduler would not auto-free '{}'",
                k, k
            ));
        }

        analysis.region_analyzer.compose_chains();
        analysis.region_analyzer.build_budget_plan(self.ctx.optimize_budget);

        // ── Precomputation check (EmitPureCounterFold) ──────────────────────────────
        //
        // Before emitting any runtime loop, check if the entire program can be
        // precomputed at compile time. If all inputs are const and the body
        // converges within --optimize-budget, we emit O(1) final-value stores.
        // If budget is exceeded but no FFI exists, warn and fall through to
        // runtime loop. If FFI exists, warn that compile-time eval is blocked.

        let precomputed_final_values = if analysis.region_analyzer.is_fully_precomputable(self.ctx.optimize_budget) {
            analysis.region_analyzer.collect_final_values(&items)
        } else if !analysis.region_analyzer.composed_chains.is_empty() {
            // A002 already covers empty-chains case: no precomputable program.
            // Only emit A001 when chains exist but budget/FFI prevents evaluation.
            let has_ffi = analysis.region_analyzer.composed_chains.iter().any(|cc|
                crate::analysis::region::has_ffi_or_trigger_stmt_in_chain(&cc.composed_body));
            if has_ffi {
                self.warnings.push("info: program not fully precomputed — FFI calls in transaction body prevent compile-time evaluation".into());
            } else {
                self.warnings.push(format!(
                    "info: program not fully precomputed — budget {} exceeded by composed chain product. Emitting runtime loop.",
                    self.ctx.optimize_budget));
            }
            None
        } else {
            None
        };

        // 2026-07-28: Persist transition graph for !prof computation in emit_toplevel.
        // iter_bounds populated later (at txn processing loop) — txns not in scope yet.
        self.ctx.transition_graph = Some(analysis.transition_graph.clone());
        // 2026-08-10: webstack flush buffer sized to the largest write_set —
        // frontend analysis, computed once so the term-site batch emitters and
        // the @__web_flush_buf declaration agree.
        self.ctx.web_max_entries = web_max_flush_entries(&self.ctx);
        self.ctx.spawn_pools = analysis.spawn_pools.clone();
        self.ctx.dependent_pools = analysis.dependent_pools.clone();
        // 2026-08-09 (Phase 5): non-pooled spawn storage classes (box/spill).
        self.ctx.spawn_storage = analysis.spawn_storage.clone();
        // 2026-07-31: Phase 2 measurement passes (plan §7.5) — persist so the
        // emission consumers (density downgrade, modulo dispatch, auto-inline)
        // read frontend analysis instead of re-walking bodies.
        self.ctx.density = analysis.density.clone();
        self.ctx.inline_decisions = analysis.inline_decisions.clone();
        self.ctx.modulo_partition = analysis.modulo_partition.clone();

        let cg = &analysis.call_graph;
        self.ctx.has_cycles = cg.has_cycle();
        // 2026-09-06 (ISR plan): ISR bodies share the program's state through
        // a GLOBAL — they run on the hardware stack, outside main's frame.
        // Main's alloca sites alias the global via emit_state_base.
        self.ctx.state_is_global = items.iter().any(
            |i| matches!(i, TopLevel::IsrHandler(_)));
        let sb = self.compute_state_size_bytes() as u64;
        self.ctx.state_size_bytes = sb;
        self.ctx.state_ptr_param = if sb > 0 {
            format!("ptr noundef dereferenceable({}) noalias nocapture align 8 %state", sb)
        } else {
            "ptr noundef noalias nocapture align 8 %state".to_string()
        };

        if self.ctx.is_embedded {
            self.check_embedded_restrictions(items);
        }

        self.ctx.exit_condition = exit_condition;
        // 2026-07-13: normalize_to_old_recursive removed in new AST.
        // BinaryOp/UnaryOp exit conditions are already the canonical form.
        // 2026-07-29: Reorder state field declarations to SoA layout before
        // index assignment. This makes same-component fields (bx0..bx4) have
        // consecutive indices, enabling the backend to form <N x float> vector
        // phi groups. See analysis/soa_reorder.rs for safety verification.
        // 2026-08-04 (compiler-in-Briev, P5): the Briev soa_reorder pass
        // computes the permutation through the GLUE C ABI when its library is
        // present; otherwise the Rust reference runs.
        let reordered_items = crate::analysis::soa_reorder::reorder_fields_briev(items);
        // 2026-08-06: populate compile-time constants BEFORE build_field_index
        // so const-sized array dimensions (`Float[MAXB]` → Dimension::Named)
        // resolve their size from `const MAXB: Int = 4096;`. Without this the
        // derivation falls back to a scalar i64 / `[0 x T]`.
        self.ctx.constants.clear();
        self.ctx.inits.clear();
        for item in items {
            if let TopLevel::Constant(c) = item {
                self.ctx
                    .constants
                    .insert(c.name.clone(), (c.ty.clone(), c.expr.clone()));
                // 2026-09-06 (Phase 8): the section(".name") placement rides
                // a side map — the constants map stays (ty, expr).
                if let Some(ref s) = c.section {
                    self.ctx.constant_sections.insert(c.name.clone(), s.clone());
                }
            }
            if let TopLevel::Init(i) = item {
                self.ctx.inits.insert(i.name.clone(), i.clone());
            }
        }
        // 2026-08-07 (object instance pools): pre-register the struct/obj
        // member-field lists so build_field_index can unpack obj instances
        // into prefixed slots (`st.data`, `st.len`). The full registration
        // (universe, obj_members, ...) runs later in generate() — this pass
        // only seeds the field lists build_field_index needs.
        for item in items {
            match item {
                TopLevel::StaticStruct(s) => {
                    let mut fields: Vec<(String, Type)> = s.fields.iter()
                        .map(|(n, t)| (n.clone(), t.clone()))
                        .collect();
                    // 2026-08-15 (coll plan §3.3): a `coll struct` is fixed
                    // `T[N]` — length == capacity == N, no hidden slots, C ABI
                    // preserved. No append here; register InlineFixed storage.
                    if s.coll {
                        self.ctx.coll_storage.insert(
                            s.name.clone(),
                            crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed,
                        );
                    }
                    self.ctx.struct_types.entry(s.name.clone()).or_insert(fields);
                }
                // 2026-08-26 (Track B): ENUMS are excluded from struct
                // registration — all-`__variant_*` slots are variant tags,
                // not fields; enum values live as boxed-image i64 handles.
                TopLevel::TypeDef(td) if (!td.body.slots.is_empty() || !td.ports_out.is_empty()) && (td.body.slots.is_empty() || !td.body.slots.iter().all(|s| s.name.starts_with("__variant_"))) => {
                    let mut fields: Vec<(String, Type)> = td.body.slots.iter()
                        .map(|s| (s.name.clone(), s.ty.clone()))
                        .collect();
                    // 2026-08-15 (coll plan §3.3): a `coll obj` appends two
                    // hidden trailing slots — `cap` then `len` (compiler-owned
                    // capacity + length). For `List` (sequence member
                    // `inner.data: Ptr<T>`) this reproduces the canonical
                    // `[inner.data, cap, len]` layout byte-for-byte.
                    if td.coll {
                        fields.push(("cap".to_string(), Type::int()));
                        fields.push(("len".to_string(), Type::int()));
                    }
                    // 2026-08-26 (async Phase D): port columns join the layout
                    // as i64 event-slot-id slots — a ports-only obj (pure event
                    // bus) registers here even with zero data slots.
                    let port_ins: Vec<String> =
                        td.ports_in.iter().map(|(n, _)| n.clone()).collect();
                    let port_outs: Vec<String> =
                        td.ports_out.iter().map(|(n, _)| n.clone()).collect();
                    for pname in &port_ins {
                        fields.push((pname.clone(), Type::int()));
                    }
                    for pname in &port_outs {
                        fields.push((pname.clone(), Type::int()));
                    }
                    if !port_ins.is_empty() || !port_outs.is_empty() {
                        self.ctx
                            .obj_port_wiring
                            .entry(td.name.clone())
                            .or_insert((port_ins, port_outs));
                    }
                    self.ctx.struct_types.entry(td.name.clone()).or_insert(fields);
                    // 2026-08-13 (obj value ABI): a slotted `obj`/`type` VALUE
                    // is a boxed i{int_bits} handle (struct literals box, state
                    // slots store the handle, field access inttoprs it), NOT a
                    // pointer. Registered alongside struct_types so llvm_type,
                    // defn params, and defn returns all agree on the handle.
                    self.ctx.obj_types.insert(td.name.clone());
                    if !td.type_params.is_empty() {
                        self.ctx.obj_type_params.entry(td.name.clone())
                            .or_insert_with(|| td.type_params.iter().map(|p| p.name.clone()).collect());
                    }
                    // 2026-08-07 (object instance pools): only OBJS (types
                    // with members) unpack into instance pools — a List<T> or
                    // other Applied collection with a slot layout must NOT
                    // unpack (it is a plain field).
                    if !td.body.members.is_empty() {
                        self.ctx.obj_members.entry(td.name.clone()).or_insert_with(|| td.body.members.clone());
                    }
                }
                _ => {}
            }
        }
        self.build_field_index(&reordered_items);

        // Scan for cell-to-cell wires from TrgBinding statements
        self.scan_cell_wires(items);

        // Inject synthetic __trg_epfd field if program has built-in triggers
        let has_builtin_trg = false;
        // 2026-07-13: New AST triggers don't have address field.
        // Built-in trigger detection (stdin/timer/signal) is TODO.
        // For now, has_builtin_trg is always false.
        if has_builtin_trg && !self.ctx.field_index_map.contains_key("__trg_epfd") {
            let idx = self.ctx.field_index_map.len();
            self.ctx.field_index_map.insert("__trg_epfd".to_string(), idx);
            self.ctx.field_types.push("i32".to_string());
            self.ctx.field_briev_types.push(Type::int());
            self.ctx.field_initializers.insert("__trg_epfd".to_string(), None);
        }
        // Inject synthetic cycle_count field for watchdog timing
        if !self.ctx.field_index_map.contains_key("cycle_count") {
            let idx = self.ctx.field_index_map.len();
            self.ctx.field_index_map.insert("cycle_count".to_string(), idx);
            self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
            self.ctx.field_initializers.insert("cycle_count".to_string(), Some(Expr::Decimal(0)));
        }
        // 2026-07-27: Arena system fields — injected only when program needs arena.
        // Skipping these saves 24 bytes in %State and eliminates the 64KB malloc
        // for benchmarks with no Alloc# calls. When needs_arena is empty, no function
        // code references these fields so they are safely omitted.
        if !self.ctx.needs_arena.is_empty() {
            let aptr = self.ctx.field_index_map.len();
            self.ctx.field_index_map.insert("__arena_ptr".to_string(), aptr);
            self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
            self.arena_ptr_idx = Some(aptr);

            let aend = self.ctx.field_index_map.len();
            self.ctx.field_index_map.insert("__arena_end".to_string(), aend);
            self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
            self.arena_end_idx = Some(aend);

            let abase = self.ctx.field_index_map.len();
            self.ctx.field_index_map.insert("__arena_base".to_string(), abase);
            self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
            self.arena_base_idx = Some(abase);
        }
        self.validate_schema_types();
        self.ctx.triggers.clear();
        self.ctx.trigger_names.clear();
        self.program_txns.clear();
        self.ctx.defn_params.clear();
        self.ctx.defn_return_types.clear();
        self.ctx.constants.clear();
        self.ctx.inits.clear();
        self.ctx.string_constants = collect_strings(items);
        self.ctx.byte_constants = collect_byte_literals(items);
        self.ctx.mask_constants = collect_mask_literals(items);

        let mut txns: Vec<(String, &crate::ast::Transaction)> = Vec::new();
        for item in items {
            match item {
                TopLevel::Constant(c) => {
                    self.ctx.constants.insert(c.name.clone(), (c.ty.clone(), c.expr.clone()));
                }
                TopLevel::Init(i) => {
                    self.ctx.inits.insert(i.name.clone(), i.clone());
                }
                TopLevel::Transaction(t) => {
                    txns.push((t.name.clone(), t));
                    self.program_txns.push(t.name.clone());
                    // Register callable txn param types for Expr::Call marshaling
                    let has_output = t.output_type.is_some() || !t.outputs.is_empty();
                    if !t.is_reactive && (!t.parameters.is_empty() || has_output) {
                        let tys: Vec<Type> = t.parameters.iter().map(|(_, ty)| ty.clone()).collect();
                        self.ctx.defn_params.insert(t.name.clone(), tys);
                        // 2026-07-18: Populate from output_type as well.
                        let ret_tys = if !t.outputs.is_empty() {
                            t.outputs.clone()
                        } else if let Some(ref ot) = t.output_type {
                            ot.all_types()
                        } else {
                            vec![]
                        };
                        self.ctx.defn_return_types.insert(t.name.clone(), ret_tys);
                    }
                }
                // 2026-08-09 (Bug 2): a `sync<group> node ...` is a reactive
                // node wrapped in a group barrier — it MUST enter the reactor's
                // txn list or the dispatch is empty and nothing fires. The
                // group membership is a concurrency-gate classification (rule
                // #21); the transaction itself dispatches like any reactive
                // node.
                TopLevel::SyncGroup { item: inner, .. } => {
                    if let TopLevel::Transaction(t) = inner.as_ref() {
                        txns.push((t.name.clone(), t));
                        self.program_txns.push(t.name.clone());
                    }
                }
                TopLevel::Trigger(trg) => {
                    // 2026-07-14: Convert new AST Trigger to TriggerDeclaration.
                    // The new Trigger struct has name/instance/port/span fields.
                    // 2026-07-15: Support @ *ptr dynamic triggers — map Expr::Deref
                    // to LinkRef::Deref so emit_trg_load emits a load from the pointer.
                    // 2026-08-27 (Slice B): numeric @-addresses are real MMIO
                    // pins — keep the value (the old code collapsed every
                    // non-Deref form to address 0) and register the VALUE-READ
                    // table so body reads lower to volatile loads.
                    let address = match &trg.instance {
                        Expr::Deref(ptr_expr) => {
                            crate::ast::LinkRef::Deref(ptr_expr.clone())
                        }
                        Expr::Decimal(n) => crate::ast::LinkRef::Explicit(*n as u64),
                        _ => crate::ast::LinkRef::Explicit(0),
                    };
                    if let crate::ast::LinkRef::Explicit(a) = &address {
                        if *a > 0 {
                            self.ctx
                                .trg_addresses
                                .insert(trg.name.clone(), *a);
                        }
                    }
                    let trg_decl = crate::ast::TriggerDeclaration {
                        name: trg.name.clone(),
                        ty: crate::ast::Type::string(),
                        address,
                        bit_range: None,
                        stages: vec![],
                        condition: None,
                        // 2026-07-14: Triggers whose name starts with __wake are wake triggers
                        is_wake: trg.name.starts_with("__wake"),
                        is_const: false,
                        span: trg.span.clone(),
                        annotations: vec![],
                        modifiers: vec![],
                    };
                    self.ctx.triggers.insert(trg.name.clone(), trg_decl);
                    self.ctx.trigger_names.push(trg.name.clone());
                }
                TopLevel::Definition(d) => {
                    let tys: Vec<Type> = d.parameters.iter().map(|(_, t)| t.clone()).collect();
                    self.ctx.defn_params.insert(d.name.clone(), tys);
                    // 2026-07-18: Populate return types from output_type (-> Type) as
                    // well as legacy outputs. output_type is the primary metadata source.
                    let ret_tys = if !d.outputs.is_empty() {
                        d.outputs.clone()
                    } else if let Some(ref ot) = d.output_type {
                        ot.all_types()
                    } else {
                        vec![]
                    };
                    self.ctx.defn_return_types.insert(d.name.clone(), ret_tys);
                }
                TopLevel::AsmFn(asm_fn) => {
                    let tys: Vec<Type> = asm_fn.params.iter().map(|(_, t)| t.clone()).collect();
                    self.ctx.defn_params.insert(asm_fn.name.clone(), tys);
                    let ret_tys = vec![asm_fn.ret_type.clone()];
                    self.ctx.defn_return_types.insert(asm_fn.name.clone(), ret_tys);
                }
                // 2026-09-06 (ISR plan): pre-register handler signatures so
                // body emission sees the param types.
                TopLevel::IsrHandler(isr) => {
                    let tys: Vec<Type> = isr.params.iter().map(|(_, t)| t.clone()).collect();
                    self.ctx.defn_params.insert(isr.name.clone(), tys);
                }
                TopLevel::ForeignBinding(fb) => {
                    let sig = crate::ast::ForeignSignature {
                        name: fb.foreign_name.clone(),
                        from: fb.from.clone(),
                        inputs: fb.inputs.clone(),
                        result_type: crate::ast::ResultType::Projection(fb.success_output.iter().map(|(_, t)| t.clone()).collect()),
                        wasm_impl: fb.wasm_impl.clone(),
                        wasm_setup: fb.wasm_setup.clone(),
                        span: fb.span,
                    };
                    self.ctx.frgn_map.insert(fb.foreign_name.clone(), sig.clone());
                    // 2026-07-22: Also index by Briev name so call resolution
                    // (Expr::Call uses the Briev name, e.g. "frgn__getenv_briev")
                    // finds the frgn entry. The declare loop emits only for the
                    // foreign_name key to avoid duplicate declarations.
                    let briev_name = fb.effective_briev_name();
                    if briev_name != fb.foreign_name {
                        self.ctx.frgn_map.insert(briev_name.to_string(), sig);
                    }
                }
                TopLevel::Obj(s) => {
                    let fields: Vec<(String, Type)> = s.fields.iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect();
                    // 2026-08-13 (obj value ABI): an `obj` value is a boxed
                    // i{int_bits} handle, not an FFI pointer — register the
                    // name separately from struct_types so llvm_type/params/
                    // returns agree on the handle representation.
                    self.ctx.obj_types.insert(s.name.clone());
                    self.ctx.struct_types.insert(s.name.clone(), fields.clone());
                    if let Some(ref mut universe) = self.ctx.type_universe {
                        if !universe.types.contains_key(&s.name) {
                            // 2026-08-26 (bug sweep B2): shared recorded fallback —
                            // SPEC §2.1 forbids silent representation defaults.
                            let rt = crate::backend::register_types::record_structural_layout(
                                universe, &s.name, "Data", &fields,
                            );
                            universe.types.insert(s.name.clone(), rt);
                        }
                    }
                }
                // 2026-07-24: StaticStruct (C-compatible struct) registration.
                // These use `struct Name { ... }` syntax.
                TopLevel::StaticStruct(s) => {
                    let fields: Vec<(String, Type)> = s.fields.iter()
                        .map(|(n, t)| (n.clone(), t.clone()))
                        .collect();
                    self.ctx.struct_types.insert(s.name.clone(), fields.clone());
                    // 2026-08-13 (layout-keywords plan): record `pack struct`
                    // so type emission/field access consult the packed layout.
                    if s.pack {
                        self.ctx.packed_structs.insert(s.name.clone());
                    }
                    if s.union {
                        self.ctx.unions.insert(s.name.clone());
                    }
                    // 2026-08-13 (Phase 5): `atomic` field slots.
                    register_atomic_fields(&mut self.ctx, &s.name, &s.metadata);
                    // 2026-08-12 (slice 4, wasm32 maze): register a StaticStruct's
                    // type params so the mono substitution (`ListBuffer<Int>`'s
                    // `Ptr<T>` → `Ptr<Int>`) can derive the wasm32 element width.
                    if !s.type_params.is_empty() {
                        self.ctx.obj_type_params.insert(
                            s.name.clone(),
                            s.type_params.iter().map(|p| p.name.clone()).collect(),
                        );
                    }
                    // 2026-08-16 (Phase 3a): a `coll struct` gets the SAME
                    // synthesized collection surface as a `coll obj` — op
                    // Count/At (iteration), Init/InitEmpty/InsertAt (literal
                    // construction). A fixed `T[N]` sequence member derives via
                    // derive_sequence_member's Vector arm, so `let f: Fixed =
                    // [1,2,3,4]` constructs through the scaffolded ops instead
                    // of the heap-seq literal (which misaligned the inline
                    // array reads by the [len] header). InlineFixed storage was
                    // registered at the pre-seed pass (mod.rs:2205).
                    if s.coll {
                        let td_slots: Vec<crate::ast::top::TypeDefSlot> = s.fields
                            .iter()
                            .map(|(n, ty)| crate::ast::top::TypeDefSlot {
                                name: n.clone(), ty: ty.clone(), bit_range: None,
                            })
                            .collect();
                        if let Some((seq_expr, elem_ty)) = crate::backend::llvm::coll_scaffold::derive_sequence_member(
                            &td_slots,
                            &self.ctx.struct_types,
                        ) {
                            let storage = self.ctx.coll_storage.get(&s.name)
                                .copied()
                                .unwrap_or(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed);
                            let ftd = crate::ast::top::TypeDef {
                                name: s.name.clone(), type_params: s.type_params.clone(),
                                parent: None, protocol: None, traits: vec![],
                                bit_range: None, span: None, coll: true, seq: false,
                                ports_in: Vec::new(),
                                ports_out: Vec::new(),
                                body: crate::ast::top::TypeDefBody {
                                    slots: td_slots.clone(), pins: Vec::new(), reference: None, tolerance: None,
            rating: None, metadata: Default::default(),
                                    projections: vec![], bindings: vec![],
                                    operators: vec![], op_bindings: vec![],
                                    constraints: vec![], members: vec![], span: None,
                                },
                            };
                            let synth = crate::backend::llvm::coll_scaffold::synthesize_members(
                                &ftd, &seq_expr, elem_ty, storage,
                            );
                            self.ctx.obj_members.entry(s.name.clone())
                                .or_insert_with(|| synth.clone());
                        }
                    }
                    if let Some(ref mut universe) = self.ctx.type_universe {
                        if !universe.types.contains_key(&s.name) {
                            // 2026-08-13 (layout-keywords plan): the spec-aware
                            // sizing lives in register_types.rs (single
                            // precedence authority, shared with tests). Only
                            // register the struct here if the type universe
                            // does not already have it.
                            let rt = crate::backend::register_types::static_struct_resolved_ty(s, universe);
                            universe.types.insert(s.name.clone(), rt);
                        }
                    }
                }
                // 2026-07-24: Handle TopLevel::TypeDef with slots as struct types.
                // This handles `obj` declarations (which parse to TypeDef) and
                // other type declarations with field slots.
                // 2026-08-26 (Track B): ENUMS get their own arm — parser
                // enums are TypeDefs whose slots are all `__variant_<Name>`;
                // their runtime image is the boxed {tag, payload} pair behind
                // an i64 handle (Phase 5d), NEVER a struct. Registering them
                // as structs emitted invalid aggregates (`{ i64, void }` for
                // zero-payload variants) and wrong by-value layouts.
                TopLevel::TypeDef(td)
                    if !td.body.slots.is_empty()
                        && td.body.slots.iter().all(|s| s.name.starts_with("__variant_")) =>
                {
                    let ctor_variants: Vec<&crate::ast::top::TypeDefSlot> = td
                        .body
                        .slots
                        .iter()
                        .filter(|s| s.name.starts_with("__variant_"))
                        .collect();
                    self.ctx.enum_handle_types.insert(td.name.clone());
                    for (idx, slot) in ctor_variants.iter().enumerate() {
                        let vname =
                            slot.name.trim_start_matches("__variant_").to_string();
                        self.ctx.variant_ctor.insert(vname.clone(), (td.name.clone(), idx));
                        // 2026-08-26: qualified `Enum::Variant` paths resolve
                        // to the same tag index.
                        self.ctx.variant_ctor.insert(
                            format!("{}::{}", td.name, vname),
                            (td.name.clone(), idx),
                        );
                    }
                }
                TopLevel::TypeDef(td)
                    if !td.body.slots.is_empty()
                        && !td.body.slots.iter().all(|s| s.name.starts_with("__variant_")) =>
                {
                    let mut fields: Vec<(String, Type)> = td.body.slots.iter()
                        .map(|s| (s.name.clone(), s.ty.clone()))
                        .collect();
                    // 2026-08-15 (coll plan §3.3): a `coll obj` appends the two
                    // hidden trailing slots here too, so the universe's `bytes`
                    // and layout agree with the field registration above.
                    if td.coll {
                        fields.push(("cap".to_string(), Type::int()));
                        fields.push(("len".to_string(), Type::int()));
                    }
                    // 2026-07-24: Register struct type in both struct_types and universe
                    self.ctx.struct_types.insert(td.name.clone(), fields.clone());
                    // 2026-08-13 (Phase 5): `atomic` field slots (obj/type body).
                    register_atomic_fields(&mut self.ctx, &td.name, &td.body.metadata);
                    // 2026-08-15 (coll plan): classify the coll's storage from
                    // its sequence member shape — HeapGrowable (Ptr<T>, never
                    // pooled) vs InlineFixed (T[N], may pool). This drives
                    // instance_prefix_for and build_field_index.
                    if td.coll {
                        if let Some(mode) = crate::backend::llvm::coll_scaffold::coll_storage_mode(
                            &td.body.slots,
                            &self.ctx.struct_types,
                        ) {
                            self.ctx.coll_storage.insert(td.name.clone(), mode);
                        }
                    }
                    // 2026-07-31 (A5): register obj members for MethodCall codegen.
                    // 2026-08-15 (coll plan §3.4): a `coll obj` gets the
                    // synthesized collection surface appended — `op Count`/
                    // `op At` (iteration), `op Init`/`op InsertAt`/
                    // `op ExtractFrom`/`op CopyFrom` construction/mutation
                    // members. These make the existing structural probes
                    // (`tier2_op_collection`, `construct_local_collection`,
                    // `foreach`, `Count#`, `<-`) fire for any coll type.
                    let mut coll_members = td.body.members.clone();
                    if td.coll {
                        if let Some((seq_expr, elem_ty)) = crate::backend::llvm::coll_scaffold::derive_sequence_member(
                            &td.body.slots,
                            &self.ctx.struct_types,
                        ) {
                            let storage = self.ctx.coll_storage.get(&td.name)
                                .copied()
                                .unwrap_or(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed);
                            let synth = crate::backend::llvm::coll_scaffold::synthesize_members(
                                td, &seq_expr, elem_ty, storage,
                            );
                            // Keep user-declared members; only add a synthesized
                            // one whose member name isn't already declared.
                            for m in synth {
                                let m_name = crate::backend::llvm::emit_expr::member_briev_name(&m);
                                let dup = coll_members.iter().any(|ex| {
                                    crate::backend::llvm::emit_expr::member_briev_name(ex) == m_name
                                });
                                if !dup {
                                    coll_members.push(m);
                                }
                            }
                        }
                    }
                    self.ctx.obj_members.entry(td.name.clone())
                        .or_insert_with(|| coll_members.clone());
                    // 2026-08-23 (Phase 5d): ENUM VARIANT CONSTRUCTION
                    // registry — parser enums are TypeDefs whose slots are
                    // __variant_<Name>; declaration order is the tag index.
                    let ctor_variants: Vec<&crate::ast::top::TypeDefSlot> = td
                        .body
                        .slots
                        .iter()
                        .filter(|s| s.name.starts_with("__variant_"))
                        .collect();
                    if !ctor_variants.is_empty() {
                        self.ctx.enum_handle_types.insert(td.name.clone());
                    }
                    for (idx, slot) in ctor_variants.iter().enumerate() {
                        let vname =
                            slot.name.trim_start_matches("__variant_").to_string();
                        self.ctx.variant_ctor.insert(vname.clone(), (td.name.clone(), idx));
                        // 2026-08-26: qualified path resolves to the same tag.
                        self.ctx.variant_ctor.insert(
                            format!("{}::{}", td.name, vname),
                            (td.name.clone(), idx),
                        );
                    }
                    // 2026-08-16 (slice-6 deletion): register the coll's
                    // default op BINDINGS in the backend too — compile.rs
                    // builds them, but tests and direct `backend.generate`
                    // calls bypass compile.rs. Without them, `op InitEmpty`/
                    // `op Init`/`op InsertAt` are unresolvable and a top-level
                    // `let q: Q = []` (or any coll literal) fell to the
                    // deleted heap-seq path. The backend owns the scaffolded
                    // MEMBERS; it must own the corresponding BINDINGS so the
                    // op dispatch and construction stay consistent everywhere.
                    if td.coll {
                        let mut coll_defs = self.ctx.operator_defs
                            .get(&td.name)
                            .cloned()
                            .unwrap_or_default();
                        for (op, impl_name) in [
                            ("InitEmpty", "init_empty"),
                            ("Init", "init"),
                            ("InsertAt", "push"),
                            ("ExtractFrom", "pop"),
                            ("CopyFrom", "get"),
                        ] {
                            if coll_defs.iter().any(|d| d.op == op) {
                                continue;
                            }
                            coll_defs.push(crate::ast::top::OperatorDef {
                                op: op.to_string(),
                                params: vec![],
                                pre: None,
                                suf: None,
                                impl_args: Some(
                                    crate::ast::PropertyValue::Identifier(impl_name.to_string()),
                                ),
                                impl_name: op.to_string(),
                                // 2026-08-27 (axiom WIP completion): no lemmas.
                                trusted_lemmas: vec![],
                                trusted_axiom: false,
                                span: None,
                            });
                        }
                        self.ctx.operator_defs.insert(td.name.clone(), coll_defs);
                    }
                    // 2026-07-31 (A8): register obj type parameters for
                    // monomorphization (`Stack<T, N>` → ["T", "N"]).
                    if !td.type_params.is_empty() {
                        self.ctx.obj_type_params.insert(
                            td.name.clone(),
                            td.type_params.iter().map(|p| p.name.clone()).collect(),
                        );
                    }
                    if let Some(ref mut universe) = self.ctx.type_universe {
                        if !universe.types.contains_key(&td.name) {
                            // 2026-08-26 (bug sweep B2): shared recorded fallback —
                            // SPEC §2.1 forbids silent representation defaults.
                            let rt = crate::backend::register_types::record_structural_layout(
                                universe, &td.name, "Data", &fields,
                            );
                            universe.types.insert(td.name.clone(), rt);
                        }
                    }
                }
                TopLevel::Enum(e) => {
                    self.ctx.enum_types.insert(e.name.clone(), e.clone());
                }
                // 2026-07-30: Unwrap Export to register defn/txn/asm_fn params
                // from exported definitions. Without this, emit_user_call can't
                // find the parameter types and falls through to the non-defn path
                // which doesn't insert inttoptr for Ptr args.
                TopLevel::Export(e) => {
                    match e.inner.as_ref() {
                        TopLevel::Definition(d) => {
                            let tys: Vec<Type> = d.parameters.iter().map(|(_, t)| t.clone()).collect();
                            self.ctx.defn_params.insert(d.name.clone(), tys);
                            let ret_tys = if !d.outputs.is_empty() {
                                d.outputs.clone()
                            } else if let Some(ref ot) = d.output_type {
                                ot.all_types()
                            } else {
                                vec![]
                            };
                            self.ctx.defn_return_types.insert(d.name.clone(), ret_tys);
                        }
                        TopLevel::Transaction(t) => {
                            let tys: Vec<Type> = t.parameters.iter().map(|(_, ty)| ty.clone()).collect();
                            self.ctx.defn_params.insert(t.name.clone(), tys);
                            let ret_tys = if !t.outputs.is_empty() {
                                t.outputs.clone()
                            } else if let Some(ref ot) = t.output_type {
                                ot.all_types()
                            } else {
                                vec![]
                            };
                            self.ctx.defn_return_types.insert(t.name.clone(), ret_tys);
                        }
                        TopLevel::AsmFn(af) => {
                            let tys: Vec<Type> = af.params.iter().map(|(_, t)| t.clone()).collect();
                            self.ctx.defn_params.insert(af.name.clone(), tys);
                            self.ctx.defn_return_types.insert(af.name.clone(), vec![af.ret_type.clone()]);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
 
        // 2026-07-13: struct_layout removed from ResolvedType in new AST.
        // TypeUniverse-based struct population is a no-op until slot syntax is reintroduced.

        // Verify all #!exit identifiers exist as state fields or constants.
        // Run BEFORE field elimination so we see the full field set.
        if let Some(ref cond) = self.ctx.exit_condition {
            let errors = self.check_exit_condition_idents(cond);
            if !errors.is_empty() {
                for err in &errors {
                    eprintln!("{}", err);
                }
                std::process::exit(1);
            }
        }

        // Phase 1: Apply adaptive layout — eliminate Never fields, append cache slots.
        // Must run AFTER trigger_names is populated (above) to prevent trigger field elimination.
        {
            let projection_usage = crate::analysis::transition_graph::compute_projection_usage(items);
            self.apply_field_modes(items, &analysis.transition_graph.live_fields, &projection_usage);
        }

        // W001: Detect dead cache slots — allocated but no loop context (one-shot program).
        if !self.ctx.cache_slots.is_empty() {
            // Check if ANY transaction has bounded convergence (loop context)
            let has_loop_context = analysis.transition_graph.nodes.iter().any(|n| {
                n.bounded_pre.is_some()
                    && n.increments.as_ref().map_or(false, |i| i.delta > 0)
            });
            if !has_loop_context {
                let field_list: Vec<String> = self.ctx.cache_slots.keys().cloned().collect();
                self.warnings.push(format!(
                    "W001: dead cache slot(s) — `{}` allocated cache slots but the program has no loop context.\n\
                      note: cache slots are only useful when a projection appears multiple times in a loop body.\n\
                      help: remove the meld routes that trigger dual-lens access, or add a loop.",
                    field_list.join(", "),
                ));
            }
        }

        // Build variant → (enum_name, discriminant, field_count) mapping.
        for (enum_name, edef) in &self.ctx.enum_types {
            let mut next_disc: u64 = 0;
            for v in &edef.variants {
                let (vname, field_count) = match v {
                    crate::ast::EnumVariant::Unit(n) => (n.clone(), 0),
                    crate::ast::EnumVariant::Tuple(n, fields) => (n.clone(), fields.len()),
                    crate::ast::EnumVariant::Struct(n, fields) => (n.clone(), fields.len()),
                };
                let disc = next_disc;
                next_disc += 1;
                self.ctx.variant_disc.insert(vname, (enum_name.clone(), disc, field_count));
            }
        }

        // Fold complex float constant expressions (e.g. const m0: Float = 4.0 * pi * pi)
        // into simple Expr::Float(f64) literals so the global emission path
        // produces valid LLVM IR instead of `constant float 0`.
        let consts_snapshot: Vec<(String, (Type, Expr))> = self.ctx.constants.iter()
            .map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, (ty, expr)) in consts_snapshot {
            // 2026-07-13: Expr::Float64 removed — all floats use Expr::Float.
            if ty == Type::float() || ty == Type::float64() {
                // 2026-07-31: Phase 3 (§8.4-D4) — float const resolution via
                // protocol membership instead of name matching.
                if let Some(val) = try_eval_cfloat(&expr, &self.ctx.constants, &|t| self.is_protocol_member(t, "Float")) {
                    self.ctx.constants.insert(name, (ty.clone(), Expr::Float(val)));
                }
            }
        }

        // Select optimization strategy via extracted decision tree
        let strategy = self.select_optimization_strategy(items, &analysis, &txns);
        // 2026-08-01 (Phase E): `seq node`/`seq txn` forces SEQUENTIAL dispatch
        // (no emit_parallel_reactor). Per the never-faster contract a modifier
        // must never win — if the default parallel path is slower than seq,
        // that is a compiler bug to fix in the default, not a seq win.
        let has_seq_modifier = txns.iter().any(|(_, t)| {
            t.modifiers.iter().any(|m| m.name == "seq")
        });
        let dispatch_mode = if has_seq_modifier {
            DispatchMode::Sequential
        } else {
            strategy.dispatch_mode
        };
        let has_wake_triggers = strategy.has_wake_triggers;
        let enumerable = strategy.enumerable;
        let enum_keys = strategy.enum_keys;
        let enum_txn_names = strategy.enum_txn_names;

        let mut out = String::new();
        self.emit_header(&mut out);
        // 2026-08-06 (beginprogram plan): per-node entry flags — true until the
        // node's goal is met; the precondition reads the flag, the body clears
        // it on goal (one-shot entry loop).
        for item in items {
            if let TopLevel::Transaction(t) = item {
                if LlvmBackend::expr_has_beginprogram(&t.contract.pre_condition) {
                    writeln!(out, "@briev_begin_{} = private global i1 1", t.name).ok();
                }
            }
        }
        // 2026-07-10: Phase 1 — emit struct type declarations before
        // function definitions so foreign callers see named types.
        self.declare_struct_types(&mut out);
        self.emit_declares(&mut out);

        // Emit foreign declares inline (frgn_map is populated from the scan above)
        // Skip names that are also linked triggers — they'll be emitted as global variables below.
        let trigger_linked_symbols: std::collections::HashSet<&str> = self.ctx.triggers.iter()
            .filter_map(|(_, t)| match &t.address {
                crate::ast::LinkRef::Linked(sym) => Some(sym.as_str()),
                _ => None,
            })
            .collect();
        // 2026-07-22: Deduplicate by foreign_name — frgn_map may have dual
        // keys (foreign_name + effective_briev_name) but declares use only the
        // C linker symbol name (sig.name = foreign_name).
        // 2026-07-31: Sort by key before emitting — frgn_map is a HashMap with
        // a per-process SipHash seed; unsorted iteration produced run-to-run
        // nondeterministic declare ORDER in the IR (Coding Standard 7).
        let mut declared: std::collections::HashSet<&str> = std::collections::HashSet::new();
        // 2026-08-28: the runtime-support prelude (emit_declares) already
        // declares these libc symbols with `nounwind` — a frgn import of the
        // same symbol emitted a SECOND declare with attribute set #6, which
        // LLVM rejects as a redefinition (getenv via `frgn getenv(...) from "c"`).
        declared.extend([
            "time", "atol", "getenv", "malloc", "free", "strlen", "realloc",
        ].into_iter());
        let mut frgn_sorted: Vec<(&String, &crate::ast::ForeignSignature)> =
            self.ctx.frgn_map.iter().collect();
        frgn_sorted.sort_by_key(|(name, _)| (*name).clone());
        for (name, sig) in frgn_sorted {
            if trigger_linked_symbols.contains(name.as_str()) { continue; }
            // Dedup: skip if we already emitted a define for this foreign_name
            if !declared.insert(&sig.name) { continue; }
            // 2026-09-10 (Family F): the captured-environ adapters take over
            // the two env symbols — a C-ABI DEFINE (loading the environ the
            // owned _start captured) replaces the declare. The impl bodies
            // live in cast_lanes.bv; the strategy (KEY= walk, digit parse)
            // is Briev, the global is compiler-owned.
            if sig.name == "__getenv_int" || sig.name == "__getenv_briev" {
                let impl_name = if sig.name == "__getenv_int" {
                    "briev_getenv_int_impl"
                } else {
                    "briev_getenv_briev_impl"
                };
                if self.ctx.defn_params.contains_key(impl_name) {
                    let ret = if sig.name == "__getenv_int" { "i64" } else { "ptr" };
                    writeln!(out, "define {} @{}(ptr %key) local_unnamed_addr #8 {{", ret, sig.name).ok();
                    writeln!(out, "  %env = load ptr, ptr @__briev_environ").ok();
                    writeln!(out, "  %r = call {} @{}(ptr null, ptr %key, ptr %env)", ret, impl_name).ok();
                    writeln!(out, "  ret {} %r", ret).ok();
                    writeln!(out, "}}").ok();
                    continue;
                }
            }
            let ret_ty: String = match sig.result_type {
                crate::ast::ResultType::VoidType | crate::ast::ResultType::TrueAssertion => "void".into(),
                crate::ast::ResultType::Projection(ref ts) => {
                    if ts.is_empty() || ts.iter().any(|t| matches!(t, Type::Void)) { "void".into() }
                    else {
                        // 2026-07-26: Use protocol-driven LLVM type.
                        let first_ret = &ts[0];
                        protocol_llvm_type(first_ret, self.ctx.type_universe.as_ref())
                    }
                }
            };
            let param_tys: Vec<String> = sig.inputs.iter().map(|(_, t)| {
                // 2026-07-26: Use protocol-driven LLVM type.
                protocol_llvm_type(t, self.ctx.type_universe.as_ref())
            }).collect();
            write!(out, "declare {} @{}(", ret_ty, sig.name).ok();
            for (pi, pt) in param_tys.iter().enumerate() {
                if pi > 0 { write!(out, ", ").ok(); }
                write!(out, "{}", pt).ok();
            }
            writeln!(out, ") #6").ok();
        }
        // 2026-07-15: POSIX declares removed — they conflict with defn wrappers
        // (getpid, sigprocmask, close, nanosleep, sched_yield, getuid, etc.)

        // Declare cast helper functions
        // 2026-08-04 (Phase 3): the `__int_to_str__`/`__str_to_int` declares
        // are REMOVED — the casting graph's ExtCall lanes call `int_to_str` /
        // `str_to_int` (no double-underscore) and emit their own declares via
        // emit_external_call. The old hardcoded cast arms were dead code.
        //
        // 2026-08-04 (Phase 4): the ExtCall lane emission (emit_cast_steps,
        // LaneKind::ExtCall) writes the `call` inline WITHOUT a declare, so
        // every lane symbol must be declared here or clang errors "use of
        // undefined value". The C definitions live in briev_rt.c (the .bv
        // path); the .ebv freestanding path provides the same symbols as
        // Briev defns in lib/std/*.ebv — in that case the program DEFINES the
        // symbol (a Briev defn lowers to a global with the same name), so the
        // declare must be skipped or clang errors "invalid redefinition".
        // Signatures match the lane ABI:
        //   String ABI = ptr to [len: i64][bytes]; Int/Bool = i64; Float = float.
        let defined: std::collections::HashSet<String> = items.iter().filter_map(|it| match it {
            crate::ast::TopLevel::Definition(d) => Some(d.name.clone()),
            crate::ast::TopLevel::Export(e) => match e.inner.as_ref() {
                crate::ast::TopLevel::Definition(d) => Some(d.name.clone()),
                _ => None,
            },
            _ => None,
        }).collect();
        
        let mut declare = |out: &mut String, name: &str, ret: &str, args: &str| {
            if !defined.contains(name) {
                writeln!(out, "declare {} @{}({}) #1", ret, name, args).ok();
            }
        };
        declare(&mut out, "__chr_to_str", "i8*", "i32");
        declare(&mut out, "int_to_str", "ptr", "i64");
        declare(&mut out, "uint_to_str", "ptr", "i64");
        declare(&mut out, "float_to_str", "ptr", "float");
        declare(&mut out, "bool_to_str", "ptr", "i64");
        declare(&mut out, "char_to_str", "ptr", "i64");
        declare(&mut out, "str_to_int", "i64", "ptr");
        declare(&mut out, "str_to_uint", "i64", "ptr");
        declare(&mut out, "str_to_float", "float", "ptr");
        declare(&mut out, "str_to_bool", "i64", "ptr");
        declare(&mut out, "str_first_char", "i64", "ptr");
        declare(&mut out, "__str_bytes__", "i64", "i64");

        // 2026-08-04 (Phase 4, .ebv heap reframe): the embedded freestanding
        // target gets a STATIC bump heap — a zero-initialized `.bss` global
        // (no @malloc/@free, no briev_rt.c). Size is the configurable
        // ir-lowering arena_initial_size (default 64KB). The bump pointer
        // lives in %State (arena_ptr_idx); emit_arena_init points it at this
        // buffer and emit_arena_alloc bumps within it. Grow-on-overflow is
        // impossible (no realloc) — a bump past the end is a compile-time
        // warning + runtime trap to 0, matching a fixed-heap bare-metal model.
        if self.ctx.is_embedded {
            writeln!(out, "@embedded_heap = private global [{} x i8] zeroinitializer",
                self.ctx.arena_initial_size).ok();
        }

        // 2026-07-08: Phase 3 — briev_rt.c wrapper function declarations
        // These are called by inop declarations in lib/std/os/*.bv.
        // All take/return i64 (boxed value) matching Briev's ABI.
        // 2026-08-15 (coll plan §3.6): the coll capacity resize helper.
        if !defined.contains("__briev_coll_resize") {
            writeln!(out, "declare i64 @__briev_coll_resize(i64, i64) #1").ok();
        }
        writeln!(out, "declare i64 @briev_open(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_close(i64) #1").ok();
        writeln!(out, "declare i64 @briev_read(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_write(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_lseek(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_pread(i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_pwrite(i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_stat(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_fstat(i64) #1").ok();
        writeln!(out, "declare i64 @briev_truncate(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_ftruncate(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_fsync(i64) #1").ok();
        writeln!(out, "declare i64 @briev_dup(i64) #1").ok();
        writeln!(out, "declare i64 @briev_dup2(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_fcntl(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_socket(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_bind(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_listen(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_accept(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_connect(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_send(i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_recv(i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_sendto(i64, i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_recvfrom(i64, i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_setsockopt(i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_getsockopt(i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_shutdown(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_mkdir(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_rmdir(i64) #1").ok();
        writeln!(out, "declare i64 @briev_unlink(i64) #1").ok();
        writeln!(out, "declare i64 @briev_rename(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_symlink(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_link(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_chdir(i64) #1").ok();
        writeln!(out, "declare i64 @briev_chmod(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_chown(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_umask(i64) #1").ok();
        writeln!(out, "declare i64 @briev_access(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_mmap(i64, i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_munmap(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_mprotect(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_brk(i64) #1").ok();
        writeln!(out, "declare i64 @briev_mlock(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_pipe(i64) #1").ok();
        writeln!(out, "declare i64 @briev_shm_open(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_shm_unlink(i64) #1").ok();
        writeln!(out, "declare i64 @briev_sem_open(i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_sem_wait(i64) #1").ok();
        writeln!(out, "declare i64 @briev_sem_post(i64) #1").ok();
        writeln!(out, "declare i64 @briev_getpid() #1").ok();
        writeln!(out, "declare i64 @briev_getppid() #1").ok();
        writeln!(out, "declare i64 @briev_clock_gettime(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_nanosleep(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_getenv(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_setenv(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_unsetenv(i64) #1").ok();
        // 2026-08-06 (endprogram plan): process exit for `endprogram` — the
        // runtime wrapper (lib/runtime/briev_rt.c) runs atexit cleanup.
        // 2026-09-09 (Family B): skipped when a pure-Briev defn provides the
        // symbol (the declare-guard `defined` set) — a declare whose
        // signature differs from the define is an LLVM redefinition error.
        if !defined.contains("__exit") {
            writeln!(out, "declare void @__exit(i64) #6").ok();
        }
        writeln!(out, "declare i64 @briev_futex(i64, i64, i64, i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @__ioctl__(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @__isatty__(i64) #1").ok();
        if !defined.contains("__print") {
            writeln!(out, "declare i64 @__print(ptr) #1").ok();
        }
        // 2026-08-01 (B0): Print# intrinsic runtime symbol. The dead frgn
        // declaration in lib/std/ffi/io.bv was removed (it declared a wrong
        // symbol and a { i64, i64 } String type); the intrinsic owns this
        // call site, so the backend declares the ABI: String = ptr to a
        // length-prefixed [len][bytes] buffer.
        if !defined.contains("__print_str") {
            writeln!(out, "declare i64 @__print_str(ptr) #1").ok();
        }
         // 2026-08-13 (dynamic String slice): `s[a:b]` emits a byte-wise
         // substring (the runtime's briev_str_substr; bounds clamp to [0,len]).
         // Skipped when a `frgn briev_str_substr` import already declares it
         // (lib/compiler/reader.bv does) — a duplicate `declare` is an LLVM
         // redefinition error (surfaced in the main<->layout-keywords merge,
         // c_driver_needs_state).
         if !self.ctx.frgn_map.contains_key("briev_str_substr")
             && !defined.contains("briev_str_substr") {
             writeln!(out, "declare ptr @briev_str_substr(ptr, i64, i64) #1").ok();
         }
        // 2026-08-01 (B1): content equality for String operands. The compiler
        // emits a call to briev_str_eq(ptr, ptr) instead of `icmp eq ptr`
        // (address comparison) when both operands are String — see
        // emit_binary_op's Eq/Ne arms. Takes two ptrs to [len][bytes].
        if !defined.contains("briev_str_eq") {
            writeln!(out, "declare i64 @briev_str_eq(ptr, ptr) #1").ok();
        }
        // 2026-08-01 (B1): content bitwise ops for String operands — return a
        // new heap [len][bytes] buffer with the per-byte op applied (band/bor/
        // bxor/bnot). Same ABI as briev_str_eq: ptr to [len][bytes].
        writeln!(out, "declare ptr @briev_str_band(ptr, ptr) #1").ok();
        writeln!(out, "declare ptr @briev_str_bor(ptr, ptr) #1").ok();
        writeln!(out, "declare ptr @briev_str_bxor(ptr, ptr) #1").ok();
        writeln!(out, "declare ptr @briev_str_bnot(ptr) #1").ok();
        // 2026-08-01 (B2): the Bit → String ENCODING DOOR default. The bits
        // are a Briev [len][bytes] buffer (a String's content view); wrapping
        // re-materializes the header by construction (the bits carry their own
        // length — not a null-terminated C string). Sub-protocols override via
        // CastFrom(Bit).
        writeln!(out, "declare ptr @briev_bits_to_str(ptr) #1").ok();
        // 2026-08-28 (Bug #5, frgn String-return): a `frgn f(...) -> String`
        // boundary contract is "returns a NUL-terminated C string" — the
        // compiler converts it to the Briev [len][bytes][\0] form at the
        // call site via briev_cstr_to_briev. Skipped when a frgn import
        // already declares the symbol (duplicate declare = redefinition).
        if !self.ctx.frgn_map.contains_key("briev_cstr_to_briev") {
            writeln!(out, "declare ptr @briev_cstr_to_briev(ptr) #1").ok();
        }
        // 2026-08-28 (Bug #5, frgn String ABI): block → C data pointer for
        // plain-C frgn params (zero-copy +8; the NUL invariant is guaranteed
        // by every Briev String allocation).
        if !self.ctx.frgn_map.contains_key("briev_str_to_c") {
            writeln!(out, "declare ptr @briev_str_to_c(ptr) nounwind").ok();
        }
        // 2026-08-01 (B3): UTF8 character count for the String `Size` prop
        // default (the O(1) byte-length header read is the `Bytes` prop).
        if !defined.contains("briev_char_len") {
            writeln!(out, "declare i64 @briev_char_len(ptr) #1").ok();
        }
        // 2026-08-14 (String unification): decode the UTF8 codepoint at a byte
        // offset of a Briev String and advance the offset — the per-iteration
        // lane of `foreach c in str` (a String operand iterates CHARs, SPEC
        // §17.2). Takes the [len][bytes] handle and the byte-offset slot.
        if !defined.contains("briev_str_next_char") {
            writeln!(out, "declare i64 @briev_str_next_char(ptr, ptr) #1").ok();
        }
        // 2026-08-07 (Phase 7): boolean mask select over a Data buffer —
        // `data[mask]` returns a new [len][bytes] buffer (SPEC §16.5).
        if !defined.contains("briev_mask_select") {
            writeln!(out, "declare i64 @briev_mask_select(ptr, ptr, i64) #1").ok();
        }
        // 2026-08-07 (Phase 7): typed mask select — an Int/Bool vector state
        // field (`[N x i64]`) masked into a new heap List buffer.
        if !defined.contains("briev_mask_select64") {
            writeln!(out, "declare i64 @briev_mask_select64(ptr, i64, ptr, i64) #1").ok();
        }
        // 2026-08-22 (Phase 6a): i8-mask variants — a Bool[N] state column is
        // [N x i8]; reading it as i64 walked past the column (garbage/segfault).
        if !defined.contains("briev_mask_select64_i8mask") {
            writeln!(out, "declare i64 @briev_mask_select64_i8mask(ptr, i64, ptr, i64) #1").ok();
        }
        // 2026-08-07 (Phase 7): Float (f32) mask select — a `Float[N]` vector
        // state field masked into a new heap List<Float> (i64 bit-pattern
        // slots, matching how heap List<Float> stores floats).
        if !defined.contains("briev_mask_select_f32") {
            writeln!(out, "declare i64 @briev_mask_select_f32(ptr, i64, ptr, i64) #1").ok();
        }
        if !defined.contains("briev_mask_select_f32_i8mask") {
            writeln!(out, "declare i64 @briev_mask_select_f32_i8mask(ptr, i64, ptr, i64) #1").ok();
        }
        // 2026-08-22 (Phase 6b): contiguous range slice over a state column —
        // `data[lo:hi]` (and the full-copy forms `data[:]` / `data[...]`).
        if !defined.contains("briev_slice_range64") {
            writeln!(out, "declare i64 @briev_slice_range64(ptr, i64, i64, i64) #1").ok();
        }
        if !defined.contains("briev_slice_range_f32") {
            writeln!(out, "declare i64 @briev_slice_range_f32(ptr, i64, i64, i64) #1").ok();
        }
        // 2026-08-01 (Phase 3): CLI argv capture. The emitted main stores
        // its argc/argv into these globals; the runtime argv helpers
        // (briev_rt.c) read them as externs. The compiler OWNS the globals
        // (it stores to them), so they are external (non-internal) for the
        // C runtime to link against. Helper FUNCTION signatures are declared
        // by lib/std/cli.bv's frgns (Int→i64, String→ptr).
        writeln!(out, "@__briev_argc = global i32 0").ok();
        writeln!(out, "@__briev_argv = global ptr null").ok();
        // 2026-09-10 (Family F): captured environ — written by main's argv
        // capture (hosted) or the owned _start (freestanding); read by the
        // getenv adapters. Unconditional like its siblings.
        writeln!(out, "@__briev_environ = global ptr null").ok();
        // 2026-09-10 (task machine migration): the linked-list HEAD BLOCKS
        // ([head, tail, current-task] for tasks; [head, tail] for events) —
        // bootstrapped lazily by the machine defns in cast_lanes.
        writeln!(out, "@__briev_sched = global ptr null").ok();
        writeln!(out, "@__briev_events = global ptr null").ok();
        // 2026-08-03: host cancellation flag — CancelRequested#() loads it,
        // __briev_set_cancel/__briev_clear_cancel (library shim) write it.
        writeln!(out, "@__briev_cancel_flag = global i32 0").ok();
        writeln!(out, "declare i64 @briev_getuid() #1").ok();
        writeln!(out, "declare i64 @briev_geteuid() #1").ok();
        writeln!(out, "declare i64 @briev_getgid() #1").ok();
        writeln!(out, "declare i64 @briev_getegid() #1").ok();
        writeln!(out, "declare i64 @briev_sched_yield() #1").ok();
        writeln!(out, "declare i64 @briev_getpriority(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_setpriority(i64, i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_getrlimit(i64) #1").ok();
        writeln!(out, "declare i64 @briev_setrlimit(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_pagesize() #1").ok();
        writeln!(out, "declare i64 @briev_cpu_count() #1").ok();
        writeln!(out, "declare i64 @briev_ring_push(i64, i64) #1").ok();
        writeln!(out, "declare i64 @briev_ring_pop(i64) #1").ok();
        writeln!(out, "declare i64 @cpu_count() #1").ok();
        writeln!(out, "declare i64 @pagesize() #1").ok();

        // Format string constants for benchmark intrinsics (print_int#, print_float#)

        // Error message for read_file# — returned as Err's String payload
        writeln!(out, "@STR_READFILE_ERR = private unnamed_addr constant [15 x i8] c\"file not found\\00\"").ok();
        // Declare libc functions used by direct-libc intrinsics
        // 2026-07-05: dso_local prevents LLVM globalopt from treating
        // @stdout as null (LLVM 18 assumes external globals without
        // initializer are zero = null). Without dso_local, LLVM's
        // function-attributor deduces fprintf(stdout) has a null pointer
        // argument → UB → entire body is dead → knucleotide prints nothing.
        writeln!(out, "@stdout = external dso_local global ptr").ok();


        // 2026-07-15: Async dispatch runtime functions
        // 2026-07-15: Removed conflicting POSIX declares (getuid, sched_yield,
        // nanosleep, exit, etc.) — replaced by Briev defn wrappers using SysCall#.

        // Emit external global declarations for linked triggers (fixes bug 4B)
        for (name, trg) in &self.ctx.triggers {
            if let crate::ast::LinkRef::Linked(sym) = &trg.address {
                let store_ty = trg_llvm_storage_ty(&trg.ty, self.ctx.type_universe.as_ref());
                let align = if store_ty == "i64" { 8 } else if store_ty == "i32" { 4 } else { 1 };
                writeln!(out, "@{} = external global {}, align {}", sym, store_ty, align).ok();
                // Warn if a linked trigger symbol is also declared as a frgn function
                if self.ctx.frgn_map.contains_key(sym.as_str()) {
                    eprintln!("warning: '{}' is declared as a frgn function but used as a @ link trigger. \
                               Use a volatile C variable for triggers, or built-in sources like @stdin#.", sym);
                }
                // Warn on unsupported trigger types
                // 2026-07-31: Phase 3 (§8.4) — supported-set via protocol
                // membership (is_boxed_int_type + Int/UInt) instead of the
                // hardcoded type-name list.
                let supported = self.is_boxed_int_type(&trg.ty)
                    || self.is_protocol_member(&trg.ty, "Int")
                    || self.is_protocol_member(&trg.ty, "UInt");
                if !supported {
                    eprintln!("warning:{}:{}: trigger '{}' has type {:?} which the LLVM runtime does not fully support; using i8 storage",
                        trg.span.as_ref().map(|s| s.line).unwrap_or(0),
                        trg.span.as_ref().map(|s| s.column).unwrap_or(0),
                        name, trg.ty);
                }
            }
        }
        if self.ctx.triggers.iter().any(|(_, t)| matches!(t.address, crate::ast::LinkRef::Linked(_))) {
            writeln!(out).ok();
        }

        // Emit constant globals for TopLevel::Constant declarations.
        // Deduplicate identical constants to avoid redundant cache lines.
        // LLVM `@alias` maps multiple names to the same global without
        // allocating separate storage.
        let mut dedup_map: HashMap<String, String> = HashMap::new(); // key → canonical_name
        let mut alias_map: HashMap<String, String> = HashMap::new(); // name → canonical_name
        // 2026-07-19: Sorted for deterministic IR order.
        let mut sorted_constants: Vec<String> = self.ctx.constants.keys().cloned().collect();
        sorted_constants.sort();
        for name in &sorted_constants {
            let (ty, expr) = &self.ctx.constants[name];
            let llvm_ty = protocol_llvm_type(ty, self.ctx.type_universe.as_ref());
            let key = match expr {
                Expr::Float(f) => format!("{}:{}", llvm_ty, float_to_llvm_str(*f, &llvm_ty)),
                Expr::Decimal(n) => format!("{}:{}", llvm_ty, n),
                Expr::Bool(b) => format!("{}:{}", llvm_ty, if *b { "true" } else { "false" }),
                Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => match inner.as_ref() {
                    Expr::Float(f) => format!("{}:{}", llvm_ty, float_to_llvm_str(-*f, &llvm_ty)),
                    Expr::Decimal(n) => format!("{}:-{}", llvm_ty, n),
                    _ => format!("{}:neg:{}", llvm_ty, name),
                },
                Expr::Quoted(_) => format!("{}:null", llvm_ty),
                _ => format!("{}:unresolved:{}", llvm_ty, name),
            };
            if let Some(canonical) = dedup_map.get(&key) {
                alias_map.insert(name.clone(), canonical.clone());
            } else {
                dedup_map.insert(key, name.clone());
                alias_map.insert(name.clone(), name.clone());
            }
        }
        // Emit declaration for canonical names only; emit alias for duplicates
        // 2026-07-19: Sorted for deterministic IR order.
        let mut sorted_constants2: Vec<String> = self.ctx.constants.keys().cloned().collect();
        sorted_constants2.sort();
        for name in &sorted_constants2 {
            let (ty, expr) = &self.ctx.constants[name];
            let canonical = alias_map.get(name).cloned().unwrap_or_else(|| name.clone());
            if canonical != *name {
                let llvm_ty = protocol_llvm_type(ty, self.ctx.type_universe.as_ref());
                writeln!(out, "@{} = alias {}, {}* @{}", name, llvm_ty, llvm_ty, canonical).ok();
                continue;
            }
            let llvm_ty = protocol_llvm_type(ty, self.ctx.type_universe.as_ref());
            let val_str = match expr {
                Expr::Float(f) => float_to_llvm_str(*f, &llvm_ty),
                Expr::Decimal(n) => n.to_string(),
                Expr::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
                Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => match inner.as_ref() {
                    Expr::Float(f) => float_to_llvm_str(-*f, &llvm_ty),
                    Expr::Decimal(n) => format!("-{}", n),
                    _ => if *ty == Type::float() { "0.0".to_string() } else { "0".to_string() },
                },
                Expr::Quoted(_) => "null".to_string(),
                _ => {
                    if *ty == Type::float() {
                        "0.0".to_string()
                    } else {
                        "0".to_string()
                    }
                },
            };
            // 2026-09-06 (Phase 8): the section(".name") placement on the
            // global constant (a define attribute on data — where the bytes
            // live). Sorted iteration keeps emission deterministic.
            let const_section = self.ctx.constant_sections.get(name)
                .map(|s| format!(" section \"{}\"", s))
                .unwrap_or_default();
            writeln!(out, "@{} = constant {} {}{}", name, llvm_ty, val_str, const_section).ok();
        }
        if !self.ctx.constants.is_empty() { writeln!(out).ok(); }

        // 2026-08-09 (init kind, Phase 2): runtime-seeded invariants — mutable
        // globals, seeded once in the pre-reactor phase (emit_init_state /
        // emit_inline_init_stores / __briev_init_state), then read-only for the
        // run. Declared `global <zero>` (NOT `constant`) because the seeding
        // store happens at runtime; the global holds the seeded value.
        let mut sorted_inits: Vec<String> = self.ctx.inits.keys().cloned().collect();
        sorted_inits.sort();
        for name in &sorted_inits {
            let init = &self.ctx.inits[name];
            let llvm_ty = protocol_llvm_type(&init.ty, self.ctx.type_universe.as_ref());
            let zero = if llvm_ty == "float" { "0.0" } else if llvm_ty == "double" { "0.0" } else if llvm_ty == "ptr" { "null" } else { "0" };
            writeln!(out, "@{} = global {} {}", name, llvm_ty, zero).ok();
        }
        if !self.ctx.inits.is_empty() { writeln!(out).ok(); }

        self.declare_state_type(&mut out);
        // %State no longer has a module-level global. Instead, main()
        // allocates it on the stack as an alloca and passes it to all
        // internal functions as a noalias nocapture parameter. This
        // guarantees SROA promotes all fields to scalar registers.
        writeln!(out, "; %State is allocated on the stack in main() as %state = alloca %State").ok();
        writeln!(out).ok();

        // Emit string constants in C-compatible format: [i64 length][chars\0]
        // The handle points to the start, so handle[0] is the length.
        for (si, s) in self.ctx.string_constants.iter().enumerate() {
            let escaped = escape_llvm_string(s);
            let len = s.len();
            writeln!(out, "@str.{} = private unnamed_addr constant <{{ i64, [{} x i8] }}> <{{ i64 {}, [{} x i8] c\"{}\\00\" }}>, align 8",
                si, len + 1, len, len + 1, escaped).ok();
        }
        if !self.ctx.string_constants.is_empty() { writeln!(out).ok(); }
        // 2026-08-06 (Phase 7): raw-bytes Data literals (`#b"..."`) — exact
        // bytes via `\xHH`, no NUL terminator, no lossy re-encoding.
        for (si, bytes) in self.ctx.byte_constants.iter().enumerate() {
            // Explicit i8 array — avoids all C-string escape ambiguity.
            let elems: Vec<String> = bytes.iter().map(|b| format!("i8 {}", *b as i8)).collect();
            writeln!(out, "@bstr.{} = private unnamed_addr constant <{{ i64, [{} x i8] }}> <{{ i64 {}, [{} x i8] [{}] }}>, align 8",
                si, bytes.len(), bytes.len(), bytes.len(), elems.join(", ")).ok();
        }
        if !self.ctx.byte_constants.is_empty() { writeln!(out).ok(); }

        // 2026-08-07 (Phase 7): Boolean mask constants for `data[mask]` —
        // i64 slots (0/1), matching the uniform %State slot width of
        // Bool-vector state fields and the runtime helper's ABI.
        for (mi, mask) in self.ctx.mask_constants.iter().enumerate() {
            let elems: Vec<String> = mask.iter().map(|b| format!("i64 {}", *b as i64)).collect();
            writeln!(out, "@bmask.{} = private unnamed_addr constant [{} x i64] [{}]",
                mi, mask.len(), elems.join(", ")).ok();
        }
        if !self.ctx.mask_constants.is_empty() { writeln!(out).ok(); }

        // 2026-08-15 (coll plan §3.3 #4): @ll_empty_list DELETED — a shared
        // sentinel aliases across every `[]` user (a `<-` push on one list
        // would corrupt the shared block). Every empty sequence constructs a
        // fresh heap block (emit_heap_seq) or `op InitEmpty` (coll).
        writeln!(out).ok();

        // 2026-07-28: Populate iter_bounds for !prof computation (txns is now in scope).
        // 2026-07-29: SLP hazard and isomorphism analysis removed — proven counterproductive.
        // LLVM's SLP vectorizer has its own cost model. See §7 of recovery plan.
        self.ctx.iter_bounds.clear();
        for (name, _) in &txns {
            if let Some(bound) = analysis.region_analyzer.iteration_bound_of(name) {
                self.ctx.iter_bounds.insert(name.clone(), bound);
            }
        }

        let mut range_meta: Vec<String> = Vec::new();

        // Definitions
        for item in items {
            match item {
                TopLevel::Definition(d) => {
                    self.emit_definition(&mut out, d, true);
                    writeln!(out).ok();
                }
                TopLevel::Export(e) => {
                    if let TopLevel::Definition(d) = e.inner.as_ref() {
                        // 2026-08-03: needs_state comes from the first-class
                        // export ABI analysis (transitive call graph). Pure
                        // exports keep a clean C ABI; exports calling any
                        // Briev defn carry %state.
                        let needs_state = self.ctx.export_needs_state.get(&d.name).copied()
                            .unwrap_or(false);
                        self.emit_definition(&mut out, d, needs_state);
                        writeln!(out).ok();
                    }
                }
                // 2026-07-29: Emit asm function bodies.
                // Each AsmFn is emitted as a define function with
                // call asm sideeffect body.
                TopLevel::AsmFn(asm_fn) => {
                    self.emit_asm_fn(&mut out, asm_fn);
                    writeln!(out).ok();
                }
                // 2026-09-06 (ISR plan): emit ISR handler bodies (calling
                // convention per mechanism) + the vector table after all
                // ordinary definitions (the table references handler symbols).
                TopLevel::IsrHandler(isr) => {
                    if let Err(e) = self.emit_isr_handler(&mut out, isr) {
                        self.warnings.push(e);
                    }
                    writeln!(out).ok();
                }
                _ => {}
            }
        }
        // 2026-09-06 (ISR plan): vector tables + default handlers — after
        // every handler definition (the table references the symbols).
        self.emit_isr_vector_tables(&mut out, items);
        // 2026-08-26 (async Phase C): segment continuations + fn-pointer
        // tables for every SPAWN-TARGETED defn, plus the runtime declares.
        // Emitted after ordinary definitions so the segment bodies reuse the
        // same statement emitter against a fresh scope.
        if !self.ctx.task_segments.is_empty() {
            self.emit_task_runtime(&mut out);
            writeln!(out).ok();
        }
        // Transactions
        // 2026-08-16 (multi-node internal fold, Direction 3): a counted-loop
        // node in a MULTI-node program is emitted as an internal PerFieldPhi
        // countdown — `@txn_<name>` runs the whole bounded pass, so the reactor
        // sequences the phase-transition nodes once per pass instead of
        // re-dispatching every iteration (the per-firing dispatch inlines the
        // body each iteration, and after LTO the shared counter is memory-
        // resident — the countdown keeps it in a phi register). The single-node
        // case is the existing main fold. The eligibility gate
        // (internal_fold_info) proves no OTHER node's pre can fire mid-pass —
        // folding would starve it.
        self.ctx.internal_fold_txns.clear();
        for (name, _) in &txns {
            if self.internal_fold_info(name, &analysis).is_some() {
                self.ctx.internal_fold_txns.insert(name.clone());
            }
        }
        for (name, txn) in &txns {
            if self.ctx.internal_fold_txns.contains(name) {
                if let Some(info) = self.internal_fold_info(name, &analysis) {
                    self.emit_internal_fold_txn(&mut out, name, txn, &analysis, info);
                } else {
                    self.emit_transaction(&mut out, txn, name, &mut range_meta);
                }
            } else {
                self.emit_transaction(&mut out, txn, name, &mut range_meta);
            }
            writeln!(out).ok();
        }
        // Precondition functions (skip callable txns — no ptr)
        for (name, txn) in &txns {
            let has_output = txn.output_type.is_some() || !txn.outputs.is_empty();
            if !txn.is_reactive && (!txn.parameters.is_empty() || has_output) { continue; }
            self.emit_pre_function(&mut out, txn, name);
        }
        // Async body functions — simple pre→fire wrapper for worker threads
        for (name, txn) in &txns {
            if self.async_txn_names.iter().any(|n| n.as_str() == name.as_str()) && !self.is_lightweight_async {
                self.emit_async_body(&mut out, txn, name);
                writeln!(out).ok();
            }
        }
        // Fused transactions
        let fusable = self.resolve_fusable_pairs(&txns);
        for (a, b) in &fusable {
            if let (Some(ta), Some(tb)) = (
                txns.iter().find(|(n, _)| n == a).map(|(_, t)| t),
                txns.iter().find(|(n, _)| n == b).map(|(_, t)| t),
            ) {
                self.emit_fused(&mut out, ta, tb, &format!("{}_{}_fused", a, b));
                writeln!(out).ok();
            }
        }
        // Composed chain functions
        // Extract all-internal counter info before taking composed_chains.
        //
        // ── Composed chain extraction ─────────────────────────────────
        //
        // Chains of reactive transactions that are all-internal (no FFI, no
        // external triggers) have their final counter values stored directly
        // as O(1) stores inside enum dispatch case arms. Non-all-internal
        // chains emit a fused composed function.
        let all_internal_counter: HashMap<String, (usize, i64)> = analysis.region_analyzer.composed_chains
            .iter()
            .filter(|cc| cc.all_internal)
            .filter_map(|cc| {
                let cv = cc.counter_var.as_ref()?;
                let ci = *self.ctx.field_index_map.get(cv)?;
                let bound = analysis.region_analyzer.iteration_bound_of(&cc.chain[0])? as i64;
                let base = format!("{}_fused_txn", cc.chain.join("_"));
                let fn_name = if let Some(ref tv) = cc.trigger_values {
                    let suffix: String = tv.iter().map(|(_, v)| v.to_string()).collect::<Vec<_>>().join("_");
                    format!("{}_trg_{}", base, suffix)
                } else { base };
                Some((fn_name, (ci, bound)))
            })
            .collect();

        let composed_chains = std::mem::take(&mut analysis.region_analyzer.composed_chains);
        let mut composed_fn_map: HashMap<String, String> = HashMap::new();
        let mut composed_by_trig: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        for cc in &composed_chains {
            let base = format!("{}_fused_txn", cc.chain.join("_"));
            let fused_name = if let Some(ref tv) = cc.trigger_values {
                let variant_suffix: String = tv.iter()
                    .map(|(_, v)| v.to_string())
                    .collect::<Vec<_>>().join("_");
                format!("{}_trg_{}", base, variant_suffix)
            } else {
                base.clone()
            };
            composed_fn_map.insert(cc.chain[0].clone(), fused_name.clone());
            if let Some(ref tv) = cc.trigger_values {
                for (_, val) in tv {
                    composed_by_trig.entry(cc.chain[0].clone()).or_default().push((*val, fused_name.clone()));
                }
            }
            // Skip emitting fused function for all-internal chains — the
            // per-case arms in emit_enum_main will store the final counter
            // value directly instead of calling the folded loop.
            if !cc.all_internal {
                self.emit_fused_composed(&mut out, &cc.composed_body, &fused_name);
                writeln!(out).ok();
            }
        }
        // Init
        self.fun.txn_counter = 0;
        self.fun.within_counter = 0;
        self.emit_init_state(&mut out);
        self.emit_persistent_cell_ticks(&mut out);
        writeln!(out).ok();

        // Emit cell channel globals and persistent cell thread functions
        // 2026-07-19: Sorted for deterministic IR.
        let mut persistent_cells: Vec<crate::ast::CellDef> = self.ctx.cell_defs.values()
            .filter(|c| c.is_persistent)
            .cloned()
            .collect();
        persistent_cells.sort_by_key(|c| c.name.clone());
        for cell in &persistent_cells {
            if self.cell_thread_names.contains(&cell.name) {
                self.emit_cell_channel_globals(&mut out, cell);
            }
        }
        for cell in &persistent_cells {
            if self.cell_thread_names.contains(&cell.name) {
                self.emit_cell_thread(&mut out, cell);
            }
        }
        writeln!(out).ok();

        // Reactor — sequential or parallel
        // Enumeration and wake trigger detection were computed above
        // in the auto-categorization step (lines ~312-380).

        // Reactor tick — use folded path when a single bounded-counter txn
        // with no triggers can be collapsed into a canonical while loop.
        let graph = &analysis.transition_graph;

        // Natural death: auto-exit for wake-triggered programs where ALL reactive
        // transactions have proven bounded convergence (bounded_pre + increments).
        // When every reactive txn is foldable, the program is guaranteed to converge
        // and can safely exit without an explicit #!exit pragma.
        //
        // We build a synthetic exit condition: for each foldable bounded-counter txn,
        // check `counter >= bound`. When ALL counters reach their bounds, no txn can
        // fire again, and main() returns 0.
        //
        // 2026-07-31: The derivation moved to the frontend pass
        // loop_shape::program_convergence — the backend consumes the derived
        // (counter, bound_var) pairs instead of re-walking the txn list. The
        // 2026-07-18 behavior is preserved exactly: the synthetic exit is built
        // for ALL programs (not just wake-triggered ones) whenever every reactive
        // txn is foldable and no explicit exit condition exists.
        let explicit_exit = self.ctx.exit_condition.clone();
        let program_conv = crate::analysis::loop_shape::program_convergence(
            graph, items, explicit_exit.is_some(),
        );
        self.ctx.has_natural_exit = program_conv.has_natural_exit;
        if explicit_exit.is_none() && !program_conv.counter_ge_bounds.is_empty() {
            // 2026-08-08 (two-node reactor fix): the bound is already an Expr
            // (Decimal for literal bounds, Identifier for field/const bounds).
            let combined = program_conv.counter_ge_bounds.into_iter()
                .map(|(counter, bound)| Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Ge,
                    Box::new(Expr::Identifier(counter)),
                    Box::new(bound),
                ))
                .reduce(|a, b| Expr::BinaryOp(crate::ast::BinaryOpKind::And, Box::new(a), Box::new(b)))
                .unwrap();
            self.ctx.exit_condition = Some(Box::new(combined));
        }

        // ── Loop emission strategy selection ──────────────────────
        //
        // This is the core decision tree that maps a reactive transaction's
        // structure to the optimal LLVM IR emission strategy:
        //
        //   Single txn + bounded counter:
        //     → check = pure counter fold (EmitPerFieldPhi), folded SSA (EmitInlineSsa), or
        //        folded memory (EmitMemoryCounter)
        //   All-const inputs:
        //     → precompute (EmitPureCounterFold) — no runtime loop
        //   Multi-txn all-pure:
        //     → multi-txn pure fold (O(1) per counter)
        //   Sequential bounded multi-txn:
        //     → SSA register pipeline (EmitSequentialSsa)
        //   Enumerable triggers:
        //     → switch-dispatch per-key folded loops
        //   Reactive with triggers:
        //     → reactor tick loop (sequential or parallel)
        //
        // The decision is driven by the transition graph (analysis), not by
        // runtime profiling data. Contracts provide the bound/liveness info.

        // 2026-07-10: EmitPerFieldPhi per-field phi loop is optimal for 1-4
        // fields; beyond that the GEP+load+store per tick overhead exceeds the
        // phi register benefit. (The old `active_writes` body re-walk that fed
        // this heuristic was removed 2026-07-31 as dead — the write set comes
        // from the transition graph's node.write_set, and the dispatch decision
        // now comes from the LoopShape, not a field count.)
        let foldable = graph.nodes.len() == 1
            && !graph.has_triggers
            && txns.len() == 1
            && graph.nodes[0].bounded_pre.is_some()
            && graph.nodes[0].increments.is_some()
            && graph.nodes[0].is_reactive;
        let folded = if foldable {
            let node = &graph.nodes[0];
            let bp = node.bounded_pre.as_ref().unwrap();
            let inc = node.increments.as_ref().unwrap();
            if bp.var != inc.var {
                false
            } else {
                match self.ctx.field_index_map.get(&bp.var) {
                    None => false,
                    Some(&counter_idx) => {
                        // 2026-07-31: Dispatch now switches on the frontend-computed
                        // LoopShape (analysis.loop_shapes) instead of re-deriving
                        // decisions from write_density/total_fields body re-walks.
                        // Every foldable bounded-counter reactive node has a shape
                        // (build_loop_shapes mirrors this gate); a missing shape
                        // falls through to the conservative reactor path.
                        // See docs/plans/2026-07-31-frontend-driven-dispatch.md §6.5.
                         match analysis.loop_shapes.get(&node.name) {
                             None => {
                                 // 2026-08-06 (diagnostics): a scheduled free
                                 // whose last consumer has no bounded-loop shape
                                 // is never emitted (the reactive path has no
                                 // sound free point) — the field leaks.
                                 if let Some(fields) = analysis
                                     .global_lifetime
                                     .free_after
                                     .get(&node.name)
                                 {
                                     for f in fields {
                                         self.warnings.push(format!(
                                             "warning: heap state field '{}' is provably \
                                              dead after '{}' but that node has no \
                                              bounded-loop shape — the planned free has no \
                                              sound emission point and the field will leak",
                                             f, node.name
                                         ));
                                     }
                                 }
                                 false
                             }
                            Some(shape) => {
                                // 2026-07-31: Swan-song hoist consumed from the
                                // frontend analysis (swan_song.rs) — the stripped
                                // body + post-loop hoist pair replaces the backend
                                // hoist_terminating_guard body re-walk.
                                let (txn_body, post_hoist) = match analysis.swan_songs.get(&node.name) {
                                    Some((stripped, hoisted)) => (stripped.clone(), hoisted.clone()),
                                    None => (txns[0].1.body.clone(), Vec::new()),
                                };
                                let folded = self.emit_folded_loop_shape(
                                    &mut out, &analysis, node, counter_idx, shape, &txn_body, post_hoist,
                                    txns[0].1.contract.watchdog.as_ref(),
                                );
                                // 2026-08-06 (diagnostics): the fold was
                                // attempted but could not be emitted for this
                                // bounded counter — the scheduler-planned frees
                                // fall through to the reactive emission, which
                                // has no sound free point, so the fields leak.
                                if !folded {
                                    if let Some(fields) = analysis
                                        .global_lifetime
                                        .free_after
                                        .get(&node.name)
                                    {
                                        for f in fields {
                                            self.warnings.push(format!(
                                                "warning: heap state field '{}' is provably \
                                                 dead after '{}' but that node cannot fold — \
                                                 the planned free has no sound emission point \
                                                 and the field will leak",
                                                f, node.name
                                            ));
                                        }
                                    }
                                }
                                folded
                            }
                        }
                    }
                }
            }
        } else { false };

        // Emit the trg step() function if the program has trigger declarations.
        // The step() function recomputes dependent variables in topological order
        // when trigger inputs change. It is called from the event loop.
        if !self.ctx.trigger_names.is_empty() {
            let trg_names = self.ctx.trigger_names.clone();
            self.emit_trg_step(&mut out, &analysis.dependency_graph, &trg_names);
        }

        if !folded {
            let precomputed = if let Some(ref final_values) = precomputed_final_values {
                // EmitPureCounterFold: fully precomputed — no runtime loop emitted
                self.warnings.push("info: program fully precomputed — no runtime loop emitted. If this is unexpected, increase --optimize-budget or add frgn calls for observability.".into());
                self.emit_precomputed_main(&mut out, final_values);
                true
            } else { false };

            if !precomputed {
                // 2026-08-26 (bug sweep B4): the never-dispatched plain-txn
                // warning must fire for EVERY program dispatch mode, not just
                // EmitSequentialSsa — enum switch-dispatch, parallel/sequential
                // reactors, and folded mains all fire is_reactive txns only.
                // Library/shared-lib shims legitimately export plain txns as
                // symbols, so those two modes keep silence.
                if !self.ctx.library_mode
                    && !self.ctx.is_shared_lib
                    && !txns.is_empty()
                {
                    Self::warn_undispatched_txns(items, &txns, &mut self.warnings);
                }
                // A004: warn when a runtime loop has zero observability
                if !txns.is_empty() {
                    let any_has_ffi = txns.iter().any(|(_, t)|
                        t.body.iter().any(|s| crate::analysis::transition_graph::statement_contains_ffi(s)));
                    if !any_has_ffi {
                        self.warnings.push(
                            "warning: emitted runtime loop has no observable side effects — \
                             LLVM may eliminate it entirely. Add frgn calls for output, \
                             or this program may run without producing results.".into());
                    }
                }
                // Multi-txn all-pure folding: when NO triggers exist and ALL
                // reactive async txns have bounded_pre + increments with pure
                // bodies, fold them into a single register-pipeline main loop.
                // 2026-07-19: Removed is_pure_body requirement. The multi-txn
                // fold calls @txn_<name> which handles guard checks including
                // observable effects. We only need deterministic convergence
                // (bounded_pre + increments) so the loop bounds are known.
                // 2026-08-08 (two-node countdown bug): the multi-txn fold emits
                // ONE loop driven by a single counter slot and bound
                // (emit_folded_multi_main hardcodes counter_idx/bound from the
                // call site). It is only sound when every async txn counts the
                // SAME counter field to the SAME bound — otherwise the loop
                // either runs once (bound fell back to 1) or drives the wrong
                // field, silently dropping iterations. Async txns with
                // disjoint counters (e.g. two `[ticksA < N]`/`[ticksB < N]`
                // nodes) must fall through to the per-txn SSA dispatch, which
                // drives each counter independently.
                let mut shared_counter: Option<String> = None;
                let mut shared_bound: Option<String> = None;
                let mut multi_foldable = enumerable.is_none()
                    && !has_wake_triggers
                    && !self.async_txn_names.is_empty();
                if multi_foldable {
                    for name in &self.async_txn_names {
                        let node = graph.nodes.iter().find(|n| n.name == *name);
                        let Some(node) = node else { multi_foldable = false; break };
                        let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) else {
                            multi_foldable = false; break;
                        };
                        if inc.var != bp.var {
                            // The fold drives a counter with `counter + 1`;
                            // a txn that increments a different field than
                            // it counts can't be folded.
                            multi_foldable = false; break;
                        }
                        match &shared_counter {
                            None => shared_counter = Some(bp.var.clone()),
                            Some(c) if *c == bp.var => {}
                            Some(_) => { multi_foldable = false; break; }
                        }
                        match &shared_bound {
                            None => shared_bound = Some(bp.bound_var.clone()),
                            Some(b) if *b == bp.bound_var => {}
                            Some(_) => { multi_foldable = false; break; }
                        }
                    }
                }
                let mut multi_fold_params: HashMap<String, FoldParam> = HashMap::new();
                if multi_foldable {
                    for txn_name in &self.async_txn_names {
                        if let Some(node) = graph.nodes.iter().find(|n| n.name == *txn_name) {
                            if let Some(ref bp) = node.bounded_pre {
                                if let Some(&cidx) = self.ctx.field_index_map.get(&bp.var) {
                                    let tidx = self.ctx.field_index_map.get(&bp.bound_var).copied();
                                    let tcname = if tidx.is_none() {
                                        if self.ctx.constants.contains_key(&bp.bound_var) {
                                            Some(bp.bound_var.clone())
                                        } else { None }
                                    } else { None };
                                    multi_fold_params.insert(txn_name.clone(), FoldParam {
                                        counter_idx: cidx,
                                        bound_field_idx: tidx,
                                        bound_const_name: tcname,
                                        is_decreasing: bp.direction == crate::analysis::transition_graph::ConvergeDirection::Decreasing,
                                        bound_literal: bp.bound_literal,
                                    });
                                }
                            }
                        }
                    }
                }
                if !multi_fold_params.is_empty() {
                    // EmitAdaptive: multi-txn pure fold
                    let txn_list: Vec<&str> = multi_fold_params.keys().map(|s| s.as_str()).collect();
                    self.warnings.push(format!("info: txns [{}] dispatched via multi-txn pure fold (all-internal, async)", txn_list.join(", ")));
                    // 2026-08-08: the fold drives ONE shared counter to ONE
                    // bound (the gate above guarantees every async txn shares
                    // them). Derive the driver from the shared fields so the
                    // loop actually runs to the bound instead of `counter < 1`.
                    let mf_counter = shared_counter.as_ref()
                        .and_then(|c| self.ctx.field_index_map.get(c.as_str()).copied())
                        .unwrap_or(0);
                    let (mf_bound_field, mf_bound_const, mf_bound_literal) = match shared_bound.as_ref() {
                        Some(b) if self.ctx.field_index_map.contains_key(b.as_str()) => {
                            (self.ctx.field_index_map.get(b.as_str()).copied(), None, None)
                        }
                        Some(b) => {
                            // A shared literal bound is recorded as a synthetic
                            // name in bound_var? No — bp.bound_var is the field/
                            // const NAME for field/const bounds; a literal bound
                            // is carried by bp.bound_literal on each node. Recover
                            // it from the first FoldParam.
                            (None, Some(b.as_str()), None)
                        }
                        None => (None, None, None),
                    };
                    let mf_bound_literal = mf_bound_literal.or_else(|| {
                        multi_fold_params.values().find_map(|p| p.bound_literal)
                    });
                    self.emit_folded_multi_main(&mut out, &txns, &[], &HashMap::new(), &multi_fold_params,
                        &HashMap::new(), mf_counter, mf_bound_field, mf_bound_const, mf_bound_literal, None, None, None, false);
                    self.emit_thread_pool_metadata(&mut out);
                } else if dispatch_mode == DispatchMode::Sequential && !txns.is_empty()
                    && enumerable.is_none() && !has_wake_triggers
                {
                    // EmitAdaptive: SSA register pipeline (or modulo-switch dispatch)
                    // 2026-07-09: Removed bounded_pre + increments requirement —
                    // emit_ssa_main correctly handles txns without bounded_pre via
                    // emit_ssa_txn_with_precond (per-tick pre check + any_fired).
                    // The EmitAdaptive fold path is independently guarded by multi_fold_params.
                    // The EmitSequentialSsa fallback at line 2632 handles the same codegen path,
                    // so entering here vs EmitSequentialSsa produces identical IR for non-foldable
                    // programs. This change allows mixed bounded/unbounded reactive txns
                    // to share the SSA pipeline instead of falling through to EmitSequentialSsa.
                    self.emit_ssa_main(&mut out, &txns, false);
                } else if let Some(ref enum_sizes) = enumerable {
                // Enumerable triggers — emit switch-dispatch main
                // This path handles triggers with small compile-time-known value sets.
                // We emit a single @main that samples triggers once, then switch
                // dispatches to per-value folded loops.

                // Build per-txn folding params for all enum-candidate txns.
                // Each trigger-gated bounded-counter txn gets its own folded
                // loop in the case arm.  Multi-txn programs (e.g. async_counters)
                // need this to converge in O(1) ticks instead of one increment
                // per tick via reactor_tick.
                let enum_fold_params: HashMap<String, FoldParam> = {
                    let mut m = HashMap::new();
                    for txn_name in &enum_txn_names {
                        if let Some(node) = graph.nodes.iter().find(|n| n.name == *txn_name) {
                            if let Some(ref bp) = node.bounded_pre {
                                if let Some(&cidx) = self.ctx.field_index_map.get(&bp.var) {
                                    let tidx = self.ctx.field_index_map.get(&bp.bound_var).copied();
                                    let tcname = if tidx.is_none() {
                                        if self.ctx.constants.contains_key(&bp.bound_var) {
                                            Some(bp.bound_var.clone())
                                        } else { None }
                                    } else { None };
                                    let inc = node.increments.as_ref();
                                    if inc.map_or(false, |i| i.var == bp.var && i.delta > 0) {
                                        m.insert(txn_name.clone(), FoldParam {
                                            counter_idx: cidx,
                                            bound_field_idx: tidx,
                                            bound_const_name: tcname,
                                            is_decreasing: bp.direction == crate::analysis::transition_graph::ConvergeDirection::Decreasing,
                                            bound_literal: bp.bound_literal,
                                        });
                }
            }
        }
    }
                    }
                    m
                };
                // Companion map: pure-body flag + total value for each foldable txn.
                // When a txn is pure and its bound is a compile-time constant, the
                // case arm can store the total directly instead of looping 50M times.
                let enum_fold_pure: HashMap<String, (bool, Option<i64>)> = {
                    let mut m = HashMap::new();
                    for txn_name in &enum_txn_names {
                        if let Some(node) = graph.nodes.iter().find(|n| n.name == *txn_name) {
                            let total_val = node.bounded_pre.as_ref().and_then(|bp| {
                                self.ctx.field_initializers.get(&bp.bound_var)
                                    .and_then(|e| e.as_ref())
                                    .and_then(|e| if let Expr::Decimal(n) = e { Some(*n) } else { None })
                                    .or_else(|| {
                                        self.ctx.constants.get(&bp.bound_var).and_then(|(_, e)| {
                                            if let Expr::Decimal(n) = e { Some(*n) } else { None }
                                        })
                                    })
                            });
                            m.insert(txn_name.clone(), (node.is_pure_body, total_val));
                        }
                    }
                    m
                };
                // Legacy single-txn params for chain composition (unchanged)
                let (enum_ci, enum_ti, enum_tcn): (usize, Option<usize>, Option<String>) = if graph.nodes.len() == 1 && txns.len() == 1 {
                    if let Some(bp) = graph.nodes[0].bounded_pre.as_ref() {
                        if let Some(&cidx) = self.ctx.field_index_map.get(&bp.var) {
                            let tidx = self.ctx.field_index_map.get(&bp.bound_var).copied();
                            let tcname = if tidx.is_none() {
                                if self.ctx.constants.contains_key(&bp.bound_var) {
                                    Some(bp.bound_var.clone())
                                } else { None }
                            } else { None };
                            (cidx, tidx, tcname)
                        } else { (0, None, None) }
                    } else { (0, None, None) }
                } else { (0, None, None) };

                // Emit the reactor_tick function (needed for residual fallback path)
                match dispatch_mode {
                    DispatchMode::Parallel => {
                        self.build_write_masks(items);
                        self.emit_parallel_reactor(&mut out, &txns, &fusable);
                    }
                    DispatchMode::Sequential => {
                        self.emit_reactor(&mut out, &txns, &fusable);
                    }
                }

                // Emit enum main with switch dispatch
                let enum_tcn_ref = enum_tcn.as_deref();
                let composed_fn = txns.first()
                    .and_then(|(n, _)| composed_fn_map.get(n))
                    .map(|s| s.as_str());
                let composed_trig_ref: Option<&HashMap<String, Vec<(i64, String)>>> = if !composed_by_trig.is_empty() {
                    Some(&composed_by_trig)
                } else { None };
                let all_int_ref: Option<&HashMap<String, (usize, i64)>> = if all_internal_counter.is_empty() {
                    None
                } else {
                    Some(&all_internal_counter)
                };
                // Declare __rt_wait for wake-triggered programs.
                // trg owns its runtime — the compiler emits the declare implicitly.
                // EmitAdaptive: enum dispatch
                self.warnings.push(format!("info: program dispatched via enum trigger dispatch ({} trigger keys)", enum_keys.len()));
                if has_wake_triggers {
                    writeln!(out, "declare void @__rt_wait() local_unnamed_addr").ok();
                }
                self.emit_folded_multi_main(
                    &mut out,
                    &txns,
                    enum_sizes,
                    &enum_keys,
                    &enum_fold_params,
                    &enum_fold_pure,
                    enum_ci,
                    enum_ti,
                    enum_tcn_ref,
                    enum_fold_params.values().find_map(|p| p.bound_literal),
                    composed_fn,
                    composed_trig_ref,
                    all_int_ref,
                    has_wake_triggers,
                );

                if has_wake_triggers {
                    self.emit_wake_metadata(&mut out);
                }
                self.emit_thread_pool_metadata(&mut out);
            } else if self.ctx.library_mode {
                self.emit_library_shim(&mut out, &txns);
            } else if self.ctx.is_shared_lib {
                // 2026-07-19: Shared library mode — emit export wrappers and
                // reactive entry points. No main loop, no reactor tick.
                self.emit_shared_lib_exports(&mut out, items);
            } else if !txns.is_empty()
                && self.async_txn_names.is_empty()
                && self.ctx.mmio_fields.is_empty()
            {
                // EmitSequentialSsa: Direct phi-based loop — no async, no MMIO.
                // Inline all txn bodies directly in main() instead of reactor_tick.
                // Triggers are sampled inline via lazy emit_trg_load, wake path uses
                // __rt_wait between ticks. LLVM promotes %State fields to phi nodes.
                if has_wake_triggers {
                    writeln!(out, "declare void @__rt_wait() local_unnamed_addr").ok();
                }
                self.warnings.push(
                    "info: program dispatched via direct SSA loop".into()
                );
                self.fun.txn_counter = 0;
                self.fun.within_counter = 0;
                self.emit_ssa_main(&mut out, &txns, has_wake_triggers);
            } else if !txns.is_empty() {
                // reactor loop fallback — only reached for async dispatch or MMIO
                // (all other programs go through EmitSequentialSsa direct SSA loop above)
                self.warnings.push(format!("info: program dispatched via reactor loop ({})", match dispatch_mode {
                    DispatchMode::Parallel => "parallel thread pool",
                    DispatchMode::Sequential => "sequential tick loop",
                }));
                match dispatch_mode {
                    DispatchMode::Parallel => {
                        self.build_write_masks(items);
                        self.emit_parallel_reactor(&mut out, &txns, &fusable);
                    }
                    DispatchMode::Sequential => {
                        self.emit_reactor(&mut out, &txns, &fusable);
                    }
                }
                // Main
                if has_wake_triggers {
                    writeln!(out, "declare void @__rt_wait() local_unnamed_addr").ok();
                }
                self.fun.txn_counter = 0;
                self.fun.within_counter = 0;
                if !self.ctx.no_main {
                    self.emit_main(&mut out, has_wake_triggers);
                }
                // Wake trigger metadata
                if has_wake_triggers {
                    self.emit_wake_metadata(&mut out);
                }
                self.emit_thread_pool_metadata(&mut out);
            } else {
        writeln!(out, "define void @reactor_tick({}) local_unnamed_addr #2 {{", self.ctx.state_ptr_param).ok();
        self.ctx.has_reactor_tick = true;
                writeln!(out, "  entry:").ok();
                writeln!(out, "  ret void").ok();
                writeln!(out, "}}").ok();
                writeln!(out).ok();
                // Main
                self.fun.txn_counter = 0;
                self.fun.within_counter = 0;
                if !self.ctx.no_main {
                    self.emit_main(&mut out, false);
                }
            }
            }
        }

        // ── DEAD-FIELD INFO DIAGNOSTICS (A002/A003) ─────────
        if !self.ctx.dead_info_disabled {
            for node in &graph.nodes {
                // 2026-08-11 (view wiring): view-bound fields are consumed by
                // the DOM — observability-as-liveness. The transition graph's
                // live_fields is body/contract-driven and knows nothing about
                // web bindings, so filter them out of the "dead" diagnostics
                // (the store is NOT eliminated; the message would be a lie).
                let dead_fields: Vec<&String> = node.write_set.iter()
                    .filter(|f| !graph.live_fields.contains(*f))
                    .filter(|f| !self.ctx.view_bound_fields.contains(*f))
                    .collect();

                if !dead_fields.is_empty() {
                    let dead_list: Vec<String> = dead_fields.iter()
                        .map(|f| format!("'{}'", f))
                        .collect();
                    if node.is_effectively_pure {
                        // Folded txn — these stores are genuinely eliminated
                        self.warnings.push(format!(
                            "info: field(s) {} written by txn '{}' are never read — stores eliminated\n\
                              note: not referenced by any precondition or #!exit condition",
                            dead_list.join(", "),
                            node.name,
                        ));
                    } else {
                        // Non-folded txn — stores still execute but value is wasted
                        self.warnings.push(format!(
                            "info: field(s) {} written by txn '{}' are never read — wasted work\n\
                              note: not referenced by any precondition or #!exit; values computed but have no effect",
                            dead_list.join(", "),
                            node.name,
                        ));
                    }
                }

                // A003: pure-counter fold info
                if node.is_effectively_pure {
                    let inc = node.increments.as_ref().unwrap();
                    let bp = node.bounded_pre.as_ref().unwrap();
                    let total_str = self.ctx.constants.get(&bp.bound_var)
                        .and_then(|(_, e)| if let Expr::Decimal(n) = e { Some(n.to_string()) } else { None })
                        .or_else(|| self.ctx.field_initializers.get(&bp.bound_var)
                            .and_then(|e| e.as_ref())
                            .and_then(|e| if let Expr::Decimal(n) = e { Some(n.to_string()) } else { None }));
                    let iterations_msg = match &total_str {
                        Some(s) => format!(" — {} iterations replaced by single store", s),
                        None => String::new(),
                    };
                    let dead_list: Vec<String> = dead_fields.iter()
                        .map(|f| format!("'{}'", f))
                        .collect();
                    let mut msg = format!(
                        "info: txn '{}' folded to O(1){}",
                        node.name, iterations_msg,
                    );
                    msg.push_str(&format!(
                        "\n  info: counter '{}' retains its store (the only live write)",
                        inc.var,
                    ));
                    if !dead_list.is_empty() {
                        msg.push_str(&format!("\n  info: dead fields: {}", dead_list.join(", ")));
                    }
                    self.warnings.push(msg);
                }
            }
        }

        // ── EXIT CONDITION DIAGNOSTICS ───────────────────────

        // Warning: #!exit on a one-shot program that never checks it.
        // Folded and precomputed paths exit without a tick loop; enum dispatch without
        // wake has no exit_check label. The standard reactor path (emit_main) always
        // checks, so we warn only when we know the check is unreachable.
        if self.ctx.exit_condition.is_some() {
            let is_one_shot = folded
                || precomputed_final_values.is_some()
                || (enumerable.is_some() && !has_wake_triggers);
            if is_one_shot {
                self.warnings.push(format!(
                    "warning: #!exit declared but program has no tick loop\n\
                      note: the exit condition will never be checked\n\
                      help: add an @link trigger to make the program reactive, or remove #!exit"
                ));
            }
        }

        // Warning: wake-triggered program without any exit path.
        // Without #!exit or natural death, the program will idle forever after all
        // reactive transactions converge. Natural death (auto-exit) is automatically
        // applied when all reactive txns have bounded convergence, so this warning
        // only fires for programs with persistent (non-foldable) reactive txns.
        if has_wake_triggers && self.ctx.exit_condition.is_none() {
            self.warnings.push(format!(
                "warning: program has wake triggers but no exit path\n\
                  note: after all transactions converge, the program will spin forever\n\
                  help: add `#!exit <condition>;` at the top of the file"
            ));
        }

        // Attributes
        if !self.fun.pending_metadata.is_empty() {
            writeln!(out, "; Loop metadata").ok();
            out.push_str(&self.fun.pending_metadata);
        }
        writeln!(out).ok();
        // 2026-07-21: Fast-math attributes (tried for nbody vrcpps conversion, but
        // vrcpps only works on vector types — all nbody divisions are scalar fdiv).
        // Reverted — function-level attrs alone don't enable vrcpps for scalar ops.
        // A future optimization could emit vector <4 x float> divisions and use
        // SLP vectorization to pack scalar ops into vector vrcpps calls.
        writeln!(out, "attributes #0 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind memory(readwrite)").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "attributes #1 = {{ nocallback nofree nosync nounwind willreturn memory(readwrite) }}").ok();
        writeln!(out, "attributes #2 = {{ mustprogress nofree norecurse nosync nounwind memory(readwrite) }}").ok();
        writeln!(out, "attributes #3 = {{ nofree norecurse nosync nounwind memory(readwrite) }}").ok();
        // 2026-07-27: SLP hazard attribute variants #4/#5 removed — manual SLP
        // vector emission is disabled (counter.rs), so there's no conflict with
        // LLVM's auto-vectorizer. All functions use #0 or #3 without disable-slp.
        // The hazard analysis code in hazard.rs is retained for future re-evaluation.
        writeln!(out, "attributes #6 = {{ nounwind }}").ok();
        // 2026-07-04: #7 = readonly for @pre_* functions.
        // Precondition expressions never write to %State — they only read
        // state fields via GEP+load. readonly tells LLVM the function has
        // no memory writes, enabling CSE of redundant pre_ calls and load
        // hoisting past precondition checks.
        // Other paths: #0 for definitions and callable txns (they may read
        // and write through %state), #2 for reactor_tick (always writes
        // the state copy), #3 for @main (writes through reactor tick loop).
        // 2026-07-04: #7 = memory(read) for @pre_* functions.
        // LLVM 18+ uses lowercase access kinds: memory(read) not memory(readonly).
        writeln!(out, "attributes #7 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind willreturn memory(read)").ok();
        writeln!(out, "}}").ok();
        // 2026-07-04: #8 = argmemonly variant of #0 for definitions/callable txns.
        // Briev functions only access memory through pointer arguments (%state).
        // No globals, no heap (beyond arena which is stack-allocated), no trigger
        // globals for defns. argmemonly lets LLVM promote allocas and eliminate
        // redundant loads across call boundaries.
        // Not used for reactive txns (they may read @link trigger globals).
        writeln!(out, "attributes #8 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind willreturn memory(argmem: readwrite)").ok();
        writeln!(out, "}}").ok();
        // 2026-07-27: #9 = memory(readwrite) for @main.
        // Main calls FFI functions (__print_int, __print_float, __print_str) that
        // access @stdout (a global). memory(argmem: readwrite) would be a lie —
        // telling LLVM main only accesses %state causes incorrect alias analysis
        // and spurious register spill across FFI calls. Per-txn functions (#8)
        // can safely use argmem:readwrite since they only access %state.
        // This attribute is deliberately HIGH-numbered (#9) to avoid
        // collision with clang-generated bitcode attributes (#0-#8)
        // during LTO merging (llvm-link renumbers but keeps #9).
        writeln!(out, "attributes #9 = {{").ok();
        writeln!(out, "    nofree norecurse nosync nounwind memory(readwrite)").ok();
        writeln!(out, "}}").ok();
        // 2026-07-04: #10 = argmem:read + willreturn for @pre_* functions.
        // Precondition functions only read state through %state and never
        // access trigger globals. Combines the benefits of #7 (memory(read))
        // and #8 (memory(argmem: readwrite)).
        writeln!(out, "attributes #10 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind willreturn memory(argmem: read)").ok();
        writeln!(out, "}}").ok();
        // 2026-07-27: #11 = argmem:readwrite for reactive txns after cold-path
        // outlining. When FFI-containing guard blocks are outlined into separate
        // cold functions, the remaining hot body is pure-Briev (only accesses %state).
        // Unlike #8, does NOT include willreturn — reactive txns may loop forever
        // if their convergence condition is never met (though all benchmarks converge).
        writeln!(out, "attributes #11 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind memory(argmem: readwrite)").ok();
        writeln!(out, "}}").ok();
        // 2026-07-27: #12 = argmem:readwrite for reactor_tick when all txns are
        // FFI-free after outlining. Includes willreturn because the reactor loop
        // always converges (all txns converge).
        writeln!(out, "attributes #12 = {{").ok();
        writeln!(out, "    mustprogress nofree norecurse nosync nounwind willreturn memory(argmem: readwrite)").ok();
        writeln!(out, "}}").ok();
        // 2026-09-09 (Family C, briev-native runtime): #13 = full
        // memory(readwrite) for definitions whose body derefs RAW ADDRESSES
        // (Load#/Store#/Copy#/Fill#/Volatile*/SysCall# over inttoptr'd Int
        // args — the cast_lanes byte primitives and the print family).
        // memory(argmem) claims accesses go "through pointer arguments"; an
        // inttoptr of an i64 arg is NOT based on a pointer, so LLVM was free
        // to conclude these defns never touch the caller's allocas — and
        // under -O3 LTO it hoisted the foreach slot load out of the loop and
        // miscompiled every string iteration. High-numbered per the #9 LTO
        // convention.
        writeln!(out, "attributes #13 = {{").ok();
        writeln!(out, "    mustprogress nofree nosync nounwind willreturn memory(readwrite)").ok();
        writeln!(out, "}}").ok();
        // Range metadata
        if !range_meta.is_empty() {
            writeln!(out).ok();
            for m in &range_meta {
                writeln!(out, "{}", m).ok();
            }
        }

        // TBAA metadata tree for type-based alias analysis
        // 2026-06-29: Dynamic generation from TypeUniverse. Each unique
        // tbaa_node group name gets its own TBAA node. "Int" is always
        // first (index 1) since it's the fallback for unmatched types.
        writeln!(out).ok();
        writeln!(out, "!0 = !{{!\"Briev\"}}").ok();
        if let Some(ref universe) = self.ctx.type_universe {
            let mut groups: Vec<String> = universe.types.keys().cloned().collect();
            // 2026-07-31: Phase 3 (§8.4-D6) — shared sort (alphabetical, Int
            // protocol member first) keeps the declaration in agreement with
            // the tbaa_node / tbaa_node_for_type index lookups.
            sort_tbaa_groups(Some(universe), &mut groups);
            for (i, group) in groups.iter().enumerate() {
                writeln!(out, "!{} = !{{!\"{}\", !0}}", i + 1, group).ok();
            }
        } else {
            // Fallback: hardcoded nodes for built-in types
            writeln!(out, "!1 = !{{!\"Int\", !0}}").ok();
            writeln!(out, "!2 = !{{!\"Bool\", !0}}").ok();
            writeln!(out, "!3 = !{{!\"Char\", !0}}").ok();
            writeln!(out, "!4 = !{{!\"String\", !0}}").ok();
                writeln!(out, "!5 = !{{!\"Float\", !0}}").ok();
        }
        // 2026-07-04: StateAliasScope — a distinct !{} node used by !noalias
        // metadata on Ptr<T> volatile load/store instructions.  This scope
        // represents "accesses to %State memory."  By annotating volatile
        // accesses through Ptr<T> with !noalias !{!StateScope}, we tell LLVM
        // that the Ptr<T> access does NOT alias with any %State field access.
        // The scope ID is fixed at 99 to avoid conflicts with TBAA (!0..!N),
        // range metadata (!{i}), and function-scoped metadata (starting at 100).
        self.ctx.state_alias_scope_md = 99;
        writeln!(out, "!99 = distinct !{{}} ; StateAliasScope").ok();

        // Build optimization report if requested
        if self.ctx.optimize_report {
            self.report_lines.push("=== Optimization Report ===".to_string());
            self.report_lines.push(format!("Optimize budget: {}", self.ctx.optimize_budget));
            let enum_count = enumerable.as_ref().map(|e| e.len()).unwrap_or(0);
            self.report_lines.push(format!("Triggers found: {} (enumerable: {})", self.ctx.trigger_names.len(), enum_count));
            if let Some(ref sizes) = enumerable {
                let mut total_combos: u64 = 1;
                self.report_lines.push("".to_string());
                self.report_lines.push("Trigger variable value sets:".to_string());
                for (tn, sz) in sizes {
                    let s = sz.unwrap_or(0);
                    self.report_lines.push(format!("  {}: {} values", tn, s));
                    total_combos = total_combos.saturating_mul(s);
                }
                self.report_lines.push(format!("Total combinations: {}", total_combos));
                if total_combos <= self.ctx.optimize_budget {
                    self.report_lines.push(format!("  ✅ Within budget ({} ≤ {})", total_combos, self.ctx.optimize_budget));
                    self.report_lines.push("  → Switch-dispatch enumeration enabled".to_string());
                } else {
                    self.report_lines.push(format!("  ❌ Exceeds budget ({} > {})", total_combos, self.ctx.optimize_budget));
                    self.report_lines.push("  → Standard reactor path used".to_string());
                }

                // Size estimation when --optimize-size is set
                if let Some(byte_limit) = self.ctx.optimize_size {
                    self.report_lines.push("".to_string());
                    self.report_lines.push("Size estimation:".to_string());
                    let bytes_per_combo: u64 = 80; // approximate bytes per switch case + folded loop
                    let base_estimate: u64 = 5000;
                    let enum_estimate = base_estimate + total_combos.saturating_mul(bytes_per_combo);
                    self.report_lines.push(format!("  Base binary (standard reactor): ~{} KB", base_estimate / 1024));
                    self.report_lines.push(format!("  With {} enumerated combos: ~{} KB", total_combos, enum_estimate / 1024));
                    if enum_estimate <= byte_limit {
                        self.report_lines.push(format!("  ✅ Enumeration fits within {} KB limit", byte_limit / 1024));
                        self.report_lines.push(format!("  Recommended budget: {}", total_combos));
                    } else {
                        self.report_lines.push(format!("  ❌ Enumeration exceeds {} KB limit", byte_limit / 1024));
                        let max_fit: u64 = if bytes_per_combo > 0 {
                            (byte_limit.saturating_sub(base_estimate)) / bytes_per_combo
                        } else { 0 };
                        self.report_lines.push(format!("  Max combos within limit: {}", max_fit));
                        self.report_lines.push(format!("  Recommended budget: {} (partial enumeration)", max_fit));
                    }
                }
            }
            if let Some(ref graph) = Some(&analysis.transition_graph) {
                self.report_lines.push("".to_string());
                self.report_lines.push("Transaction graph:".to_string());
                self.report_lines.push(format!("  Nodes: {}", graph.nodes.len()));
                self.report_lines.push(format!("  Has triggers: {}", graph.has_triggers));
                if let Some(node) = graph.nodes.first() {
                    if let Some(ref bp) = node.bounded_pre {
                        self.report_lines.push(format!("  Bounded pre: {} < {}", bp.var, bp.bound_var));
                    }
                    self.report_lines.push(format!("  Is reactive: {}", node.is_reactive));
                    self.report_lines.push(format!("  Pure body: {}", node.is_pure_body));
                    if let Some(ref inc) = node.increments {
                        self.report_lines.push(format!("  Increment: {} += {}", inc.var, inc.delta));
                    }
                }
            }
            let chains = &analysis.region_analyzer.linear_chains;
            if !chains.is_empty() {
                self.report_lines.push("".to_string());
                self.report_lines.push("Linear transaction chains detected:".to_string());
                for (i, chain) in chains.iter().enumerate() {
                    let chain_str: String = chain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" -> ");
                    self.report_lines.push(format!("  Chain {}: {}", i + 1, chain_str));
                }
            }
            let scores = &analysis.region_analyzer.region_scores;
            if !scores.is_empty() {
                self.report_lines.push("".to_string());
                self.report_lines.push("Optimization priority ranking:".to_string());
                for (rank, score) in scores.iter().enumerate() {
                    let class_str = format!("{:?}", score.complexity);
                    let gpu = if score.gpu_eligible { " GPU" } else { "" };
                    let chain_tag = if score.chain_composed { " Chain" } else { "" };
                    let score_str = if score.optimization_score.is_infinite() || score.optimization_score <= 0.0 {
                        "—".to_string()
                    } else {
                        format!("{:.1}", score.optimization_score)
                    };
                    let vs_size = match score.value_set_size {
                        Some(n) => format!("{}", n),
                        None => "∞".to_string(),
                    };
                    let txn_list = score.txn_names.join(",");
                    self.report_lines.push(format!(
                        "  #{:<3} R{:<4} {:<20} {:<9} {:<7} {:<8} {:<8} {:<8} {}{}",
                        rank + 1, score.region_id, txn_list, class_str,
                        score.body_weight, score.iteration_count, vs_size, score_str,
                        chain_tag, gpu
                    ));
                }
            }
            if let Some(ref plan) = analysis.region_analyzer.budget_plan {
                self.report_lines.push("".to_string());
                self.report_lines.push(format!("Budget plan (budget={}):", plan.total_budget));
                let spent: u64 = plan.total_budget - plan.residual_budget;
                self.report_lines.push(format!("  Allocated: {} regions, spent {}/{}", plan.allocated.len(), spent, plan.total_budget));
                for (rid, cls, cost, score) in &plan.allocated {
                    self.report_lines.push(format!("    R{}: {:?}, cost={}, score={:.1}", rid, cls, cost, score));
                }
                self.report_lines.push(format!("  Residual: {} budget units", plan.residual_budget));
                if !plan.skipped.is_empty() {
                    self.report_lines.push(format!("  Skipped: {} regions (unbounded or exceeds budget)", plan.skipped.len()));
                    for (rid, cls, _) in &plan.skipped {
                        self.report_lines.push(format!("    R{}: {:?}", rid, cls));
                    }
                }
            }
            if !composed_chains.is_empty() {
                self.report_lines.push("".to_string());
                self.report_lines.push("Composed chains:".to_string());
                for cc in &composed_chains {
                    let chain_str: String = cc.chain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" → ");
                    let triggers = if cc.root_triggers.is_empty() {
                        "none".to_string()
                    } else {
                        cc.root_triggers.join(", ")
                    };
                    let tv_str = match &cc.trigger_values {
                        Some(tv) => tv.iter().map(|(n, v)| format!("{}={}", n, v)).collect::<Vec<_>>().join(", "),
                        None => if cc.root_triggers.is_empty() { "—".to_string() } else { "unbranched".to_string() },
                    };
                    let internal = if cc.all_internal { " all-internal" } else { "" };
                    self.report_lines.push(format!(
                        "  {} (link vars: {}, triggers: {}, values: [{}], fused weight: {}{})",
                        chain_str, cc.link_vars.join(","), triggers, tv_str, cc.fused_weight, internal
                    ));
                }
            }
            if precomputed_final_values.is_some() {
                self.report_lines.push("".to_string());
                self.report_lines.push("Precomputed (compile-time evaluation):".to_string());
                self.report_lines.push("  All state values determined at compile time — O(1) runtime.".to_string());
                if let Some(ref fv) = precomputed_final_values {
                    self.report_lines.push(format!("  {} chains precomputed.", fv.len()));
                    for (chain, bindings) in fv {
                        let chain_str: String = chain.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" → ");
                        let vars: Vec<String> = bindings.iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect();
                        self.report_lines.push(format!("    {} → final state: [{}]", chain_str, vars.join(", ")));
                    }
                }
            }
        }

        // 2026-08-06 (accel plan): emit + embed SPIR-V kernels for the
        // frontend's Gpu/Probe accel bodies (AnalysisResults.accel). Runs after
        // all host emission so the shared emitter's function state is free.
        // 2026-08-31 (plan abv-gpu-by-default): the candidate set + order were
        // pre-registered at the TOP of generate() (the dispatch wrappers need
        // the index during host emission). collect_accel_kernels preserves
        // that order — a failed kernel keeps its slot with an EMPTY blob so
        // wrapper launch indices stay valid (the runtime rejects empty blobs
        // and falls back to CPU for that kernel).
        let kernel_blobs = self.collect_accel_kernels(&analysis.accel, items);
        self.has_accel_kernels = !kernel_blobs.is_empty();
        for blob in &kernel_blobs {
            out.push_str(&kernel::embed_spirv_blob(&blob.bytes, &blob.txn_name));
        }
        // Descriptor tables + ABI declares the host dispatch wrappers use.
        let (desc_ir, idx_of) = kernel::emit_accel_descriptors(self, &kernel_blobs, items);
        self.accel_kernel_idx = idx_of;
        out.push_str(&desc_ir);

        // Phase 4: --layout diagnostic flag — print field layout after generation
        if self.ctx.dump_layout {
            eprintln!("{}", self.dump_layout_str());
        }

        // 2026-06-29: %tddup post-processing pass removed — all register
        // allocation now goes through FunctionContext::next_reg() which
        // guarantees unique %t{N} names by construction. The old pass
        // redefined duplicate %t{N} registers as %tddup{N} but did NOT
        // rename subsequent uses, creating SSA violations.
        // See docs/plans/2026-06-29-llvm-backend-refactoring.md#p2.

        // 2026-07-26: Phase 4 — Webstack exports (state_layout + generation)
        // and __web_flush_state import. Emitted when webstack_enabled is true.
        if self.ctx.webstack_enabled {
            // Declare the __web_flush_state import — called at each term;
            writeln!(out, "declare void @__web_flush_state(i32, i32)").ok();
            // Generation counter — incremented after each txn commit
            writeln!(out, "@__web_generation = global i32 0").ok();
            // Export generation counter (WASM global export)
            writeln!(out, "@__web_generation_export = hidden global ptr @__web_generation").ok();
            // 2026-08-10: real flush buffer — the JS shim's _applyFlush reads
            // count × 12-byte records { field_handle, value_ptr, value_len }
            // starting at the pointer __web_flush_state receives. Sized to the
            // largest transaction write_set (each txn's update batch fits; the
            // shim loops count records, so unused tail entries are harmless).
            let max_entries = self.ctx.web_max_entries.max(1);
            writeln!(out, "@__web_flush_buf = private global [{} x {{ i32, i32, i32 }}] zeroinitializer", max_entries).ok();
            // State layout function — returns ptr to a constant layout table
            // consumed by the JS shim (glue/web_generator.rs). Table layout:
            //   +0 field_count u32, +4 generation_off u32, +8 flush_off u32,
            //   +12 max_entries u32, then field_count × 16 bytes:
            //   handle u32, offset u32, size u32, type_tag u32.
            // 2026-08-10: real per-field rows. handle = field index, offset =
            // structural byte offset within the emitted %State (fields laid
            // out in LLVM struct order), size = LLVM byte width, type_tag from
            // the field's Briev type via protocol category (rule 18 — never
            // matched by type name). flush_off/max_entries now describe the
            // real @__web_flush_buf (resolved via ptrtoint at link time).
            let mut rows = String::new();
            let mut types_ll = Vec::new();
            let mut offset = 0u64;
            let mut field_count = 0u32;
            for (i, briev_ty) in self.ctx.field_briev_types.iter().enumerate() {
                let ty = self.ctx.field_types[i].clone();
                let size = web_llvm_byte_size(&ty);
                if size == 0 { continue; } // skip non-word rows (unresolved)
                let cat = match self.ctx.type_universe.as_ref() {
                    Some(u) => crate::type_universe::protocol_category(u, briev_ty),
                    None => None,
                };
                let tag: u32 = match crate::glue::web_generator::TypeTag::from_protocol_category(cat.as_deref()) {
                    crate::glue::web_generator::TypeTag::Int => 0,
                    crate::glue::web_generator::TypeTag::Float => 1,
                    crate::glue::web_generator::TypeTag::Bool => 2,
                    crate::glue::web_generator::TypeTag::String => 3,
                };
                rows.push_str(&format!("i32 {}, i32 {}, i32 {}, i32 {}, ", i as u32, offset, size, tag));
                types_ll.push(format!("i32, i32, i32, i32"));
                offset += size;
                field_count += 1;
            }
            // The LLVM struct TYPE is plain i32 fields; the INITIALIZER body
            // carries the values (including link-time-resolved ptrtoint of the
            // generation counter and flush buffer). A ptrtoint in the type
            // position is invalid LLVM.
            let layout_ty = format!("i32, i32, i32, i32{}",
                if types_ll.is_empty() { String::new() } else { format!(", {}", types_ll.join(", ")) });
            // Body: field_count, generation_off, flush_off, max_entries, then rows.
            let mut body = format!("i32 {}, i32 ptrtoint (ptr @__web_generation to i32), i32 ptrtoint (ptr @__web_flush_buf to i32), i32 {}", field_count, max_entries);
            if !rows.is_empty() {
                body.push_str(", ");
                body.push_str(rows.trim_end_matches(", "));
            }
            writeln!(out, "@__web_layout = private constant {{ {} }} {{ {} }}", layout_ty, body).ok();
            writeln!(out, "define i32 @state_layout() {{").ok();
            writeln!(out, "  ret i32 ptrtoint ({{ {} }}* @__web_layout to i32)", layout_ty).ok();
            writeln!(out, "}}").ok();
            // 2026-08-12 (Iterable protocol, slice 4): the web runtime's state
            // passing was never wired — the shim called txn exports with no
            // args, so `%state` defaulted to 0 and the txns operated on garbage
            // at the wasm heap base (silently wrong; the 2b2 demos never
            // exercised interactive clicks). Provide a long-lived `@__web_state`
            // global, a `__briev_state_ptr()` the shim passes to EVERY export,
            // a `__web_boot()` that runs init_state, and a `render_frame()` that
            // ticks the reactor (the shim's `_startRenderLoop` calls it).
            writeln!(out, "@__web_state = global %State zeroinitializer").ok();
            writeln!(out, "define i32 @__briev_state_ptr() {{").ok();
            writeln!(out, "  ret i32 ptrtoint (%State* @__web_state to i32)").ok();
            writeln!(out, "}}").ok();
            writeln!(out, "define void @__web_boot() {{").ok();
            writeln!(out, "  call void @init_state(ptr @__web_state)").ok();
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
            writeln!(out, "define void @render_frame() {{").ok();
            // 2026-08-12 (slice 4): a folded program (no live reactive nodes)
            // omits @reactor_tick — emit a no-op frame in that case.
            if self.ctx.has_reactor_tick {
                writeln!(out, "  call void @reactor_tick(ptr @__web_state)").ok();
            }
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
            self.emit_view_materializers(&mut out);
        }

        // 2026-08-06 (fix): escaping closures — emit the collected closure
        // functions at the end of the module.
        self.emit_pending_closures(&mut out);

        // ── Owned _start + captured environ (Family F, 2026-09-10) ──────
        // When the program references NOTHING from briev_rt.c or libc, the
        // backend owns the process entry: a module-asm `_start` reads
        // argc/argv/environ off the initial kernel stack (the only place
        // they exist), stores the environ pointer for the stdlib getenv
        // family, calls @main, and exits via syscall. compile.rs sees the
        // `module asm` marker and links -nostdlib. Kept-C programs keep the
        // crt path (libc owns the entry there) and the C getenv pair.
        if self.ctx.target_triple.contains("linux")
            && out.lines().any(|l| {
                l.contains("call ") && Self::kept_runtime_symbol(l)
            })
        {
            // Not libc-free: emit the environ global + the getenv adapters
            // reading it ONLY when a _start captured it — skipped here, the
            // frgn path (ffi/env.bv -> C getenv) serves the program.
            // (The global itself is emitted with the _start below.)
        } else if self.ctx.target_triple.contains("linux") {
            // (The environ global + the two getenv adapters emit from the
            // frgn loop above — they take over ffi/env.bv's frgn names.)
            let triple = self.ctx.target_triple.clone();
            // A NAKED function (not module asm — module asm is dropped by
            // the LTO link): the body is raw asm, no prologue/epilogue, so
            // %rsp at the first instruction is the kernel's entry stack.
            // Entry rsp is 16-aligned; `call main` pushes the return address
            // giving main its ABI-expected alignment.
            // @llvm.used pins main + the environ global against LTO
            // internalization — the _start asm references them by name, and
            // LTO renaming would dangle those references.
            writeln!(out, "@llvm.used = appending global [2 x ptr] [ptr @main, ptr @__briev_environ]").ok();
            if triple.starts_with("x86_64") {
                writeln!(out, "define void @_start() naked noinline {{").ok();
                writeln!(out, "entry:").ok();
                writeln!(out, "  call void asm sideeffect \"movq (%rsp), %rax; leaq 8(%rsp), %rsi; leaq 16(%rsp,%rax,8), %rdx; movq %rdx, __briev_environ(%rip); xorl %ebp, %ebp; call main; movl %eax, %edi; movl $$231, %eax; syscall\", \"~{{rdi}},~{{rsi}},~{{rdx}},~{{rax}},~{{rcx}},~{{r11}},~{{memory}}\" ()").ok();
                writeln!(out, "  unreachable").ok();
                writeln!(out, "}}").ok();
            } else if triple.starts_with("aarch64") {
                writeln!(out, "define void @_start() naked noinline {{").ok();
                writeln!(out, "entry:").ok();
                writeln!(out, "  call void asm sideeffect \"ldr x0, [sp]; add x1, sp, 8; ldr x2, [sp]; add x3, sp, 16; add x2, x3, x2, lsl 3; adrp x4, __briev_environ; str x2, [x4, :lo12:__briev_environ]; bl main; mov x8, 94; svc 0\", \"~{{x0}},~{{x1}},~{{x2}},~{{x3}},~{{x4}},~{{memory}}\" ()").ok();
                writeln!(out, "  unreachable").ok();
                writeln!(out, "}}").ok();
            }
        }

        out
    }

    /// 2026-09-10 (Family F): does this call line target a symbol that is
    /// still implemented in briev_rt.c or by libc? Used by the owned-_start
    /// gate — a program touching any of these keeps the crt path.
    fn kept_runtime_symbol(line: &str) -> bool {
        const KEPT: &[&str] = &[
            // briev_rt.c kept families
            "briev_str_to_c", "briev_cstr_to_briev", "briev_free_briev_str",
            "briev_bits_to_str", "briev_str_band", "briev_str_bor",
            "briev_str_bxor", "briev_str_bnot", "briev_symbol_available",
            "briev_syscall", "briev_sysconf", "ShellCmd", "__briev_setenv",
            "__print_float64", "briev_cstring_concat",
            "briev_host_print_int", "briev_host_fail", "briev_host_table_set",
            "briev_host_arity_of", "briev_task_spawn", "briev_task_cancel",
            "briev_await", "briev_event_alloc", "briev_event_read",
            "briev_event_fire", "briev_event_ready", "briev_event_strict_trap",
            // __getenv_int/__getenv_briev are NOT here: they are adapter-
            // served (the backend emits C-ABI defines over the captured
            // environ). Same for the __argv_* family (retired with cli.bv).
            // libc
            "malloc", "free", "realloc", "calloc", "getenv", "printf",
            "briev_getenv_int_impl", "briev_getenv_briev_impl",
            "fprintf", "strlen", "strdup", "sysconf", "dlsym", "popen",
            "signal", "atexit", "clock_gettime", "setenv", "syscall",
            "pclose", "fputs", "putchar", "exit",
        ];
        KEPT.iter().any(|s| line.contains(&format!("@{s}(")))
    }

    /// 2026-08-12 (Iterable protocol, slice 4): for each `b-each` iterable that
    /// is a Tier-2 collection (op Count + op At as op-as-member members), emit a
    /// `__view_items_<field>(ptr %state)` snapshot materializer. It drives the
    /// collection's own ops to fill `@__web_view_buf` as `[len][word…]` and
    /// returns the buffer pointer — the shim's b-each renderer reads the
    /// snapshot instead of vector layout bytes (which cannot index a heap
    /// collection). Structural: the compiler knows only the op surface, never a
    /// collection layout.
    fn emit_view_materializers(&mut self, out: &mut String) {
        use crate::ast::TopLevel;
        if self.ctx.collection_iterables.is_empty() {
            return;
        }
        writeln!(out, "@__web_view_buf = private global [1024 x i64] zeroinitializer").ok();
        let mut names: Vec<String> = self.ctx.collection_iterables.iter().cloned().collect();
        names.sort_unstable();
        for field in &names {
            let Some(&idx) = self.ctx.field_index_map.get(field) else { continue };
            let briev_ty = self.ctx.field_briev_types.get(idx).cloned().unwrap_or(crate::ast::Type::int());
            let base = match &briev_ty {
                crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.clone(),
                _ => continue,
            };
            let members = self.ctx.obj_members.get(&base).cloned().unwrap_or_default();
            let has = |op: &str| {
                members.iter().any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == op))
            };
            if !(has("Count") && has("At")) {
                continue;
            }
            let fn_name = format!("__view_items_{}", field);
            writeln!(out, "define i32 @{}(ptr noundef noalias nocapture align 8 %state) local_unnamed_addr #0 {{", fn_name).ok();
            let count_tmp = self.fun.gen_reg();
            let count = self.emit_method_call(out, &count_tmp, &crate::ast::Expr::Identifier(field.clone()), "Count", &[], "  ");
            // The Int width on wasm32 is i32; the At index + loop counter are
            // Int, so use i32 (the count register may be i32 already or i64 —
            // adapt).
            let count_is_64 = self.llvm_type(&count.ty) == "i64";
            let n_i32 = if count_is_64 {
                let t = self.fun.gen_reg();
                writeln!(out, "  {} = trunc i64 {} to i32", t, count.name).ok();
                t
            } else {
                count.name.clone()
            };
            writeln!(out, "  %hdr = alloca i32").ok();
            writeln!(out, "  store i32 0, ptr %hdr").ok();
            writeln!(out, "  br label %hlbl").ok();
            writeln!(out, "hlbl:").ok();
            writeln!(out, "  %c = load i32, ptr %hdr").ok();
            writeln!(out, "  %cmp = icmp slt i32 %c, {}", n_i32).ok();
            writeln!(out, "  br i1 %cmp, label %blbl, label %elbl").ok();
            writeln!(out, "blbl:").ok();
            // word = field.At(c)
            let cur_tmp = "__view_cur".to_string();
            self.fun.let_bindings.insert(cur_tmp.clone(), "%c".to_string());
            self.fun.let_binding_types.insert(cur_tmp.clone(), crate::ast::Type::int());
            self.fun.let_original_types.insert(cur_tmp.clone(), crate::ast::Type::int());
            let arg = crate::ast::Expr::Identifier(cur_tmp);
            let at_tmp = self.fun.gen_reg();
            let word = self.emit_method_call(out, &at_tmp, &crate::ast::Expr::Identifier(field.clone()), "At", &[arg], "  ");
            // 2026-08-12 (slice 4, String elements): a String/Data element's At
            // return is a PTR (the [len][bytes] address) — box it to the i64
            // snapshot word via adapt_to_i64 (ptrtoint); an Int element zexts
            // from i32 on wasm32.
            let word64 = if self.llvm_type(&word.ty) == "i64" {
                word.name.clone()
            } else if self.is_string_operand(&word.ty) || self.is_blob_operand(&word.ty) {
                let p = self.fun.gen_reg();
                writeln!(out, "  {} = ptrtoint {} {} to i64", p, self.llvm_type(&word.ty), word.name).ok();
                p
            } else {
                let z = self.fun.gen_reg();
                writeln!(out, "  {} = zext i32 {} to i64", z, word.name).ok();
                z
            };
            writeln!(out, "  %w32 = add i32 %c, 1").ok();
            writeln!(out, "  %w = sext i32 %w32 to i64").ok();
            writeln!(out, "  %p = getelementptr [1024 x i64], ptr @__web_view_buf, i64 0, i64 %w").ok();
            writeln!(out, "  store i64 {}, ptr %p", word64).ok();
            writeln!(out, "  %next = add i32 %c, 1").ok();
            writeln!(out, "  store i32 %next, ptr %hdr").ok();
            writeln!(out, "  br label %hlbl").ok();
            writeln!(out, "elbl:").ok();
            // buf[0] = n (the real len, at i64)
            let n64 = self.fun.gen_reg();
            writeln!(out, "  {} = sext i32 {} to i64", n64, n_i32).ok();
            writeln!(out, "  store i64 {}, ptr @__web_view_buf", n64).ok();
            writeln!(out, "  ret i32 ptrtoint ([1024 x i64]* @__web_view_buf to i32)").ok();
            writeln!(out, "}}").ok();
        }
    }

    /// 2026-08-06 (fix): emit the top-level closure functions collected during
    /// emission. Each reads its captured vars from the env block (slots 1..N)
    /// and returns the body's value; params arrive as i64 arguments. The env is
    /// the hidden first parameter.
    fn emit_pending_closures(&mut self, out: &mut String) {
        let closures = std::mem::take(&mut self.ctx.pending_closures);
        if closures.is_empty() {
            return;
        }
        let saved_fun = self.fun.clone();
        for c in &closures {
            self.emit_one_closure(out, c);
        }
        self.fun = saved_fun;
    }

    /// Emit a single closure function `define i64 @symbol(ptr %env, i64 %p..)`.
    fn emit_one_closure(
        &mut self,
        out: &mut String,
        c: &crate::backend::llvm::context::PendingClosure,
    ) {
        self.fun = crate::backend::llvm::context::FunctionContext::new();
        let param_list: Vec<String> = (0..c.params.len())
            .map(|i| format!("i64 %p{}", i))
            .collect();
        let params = if param_list.is_empty() {
            String::new()
        } else {
            format!(", {}", param_list.join(", "))
        };
        writeln!(out, "define i64 @{}(ptr %env{}) {{", c.symbol, params).ok();
        for (i, p) in c.params.iter().enumerate() {
            self.fun.let_bindings.insert(p.clone(), format!("%p{}", i));
            self.fun.let_binding_types.insert(p.clone(), Type::int());
            self.fun.let_original_types.insert(p.clone(), Type::int());
        }
        for (j, v) in c.free_vars.iter().enumerate() {
            let slot = self.fun.gen_reg();
            writeln!(out, "  {} = getelementptr i64, ptr %env, i64 {}", slot, 1 + j).ok();
            let cap = self.fun.gen_reg();
            writeln!(out, "  {} = load i64, ptr {}", cap, slot).ok();
            self.fun.let_bindings.insert(v.clone(), cap.clone());
            self.fun.let_binding_types.insert(v.clone(), Type::int());
            self.fun.let_original_types.insert(v.clone(), Type::int());
        }
        let result = self.emit_expr(out, &c.body, "  ");
        // 2026-08-14 (stdlib-cleanup): the closure ABI boxes every return to
        // i64 (params/env slots are i64 too). A Bool predicate body emits an
        // i8 register (`x % 2 == 0`) — zext it to i64 so `ret i64` stays valid.
        // The indirect-call site truncs back to i8. This keeps the whole
        // closure ABI uniform i64 (a Bool is as boxed as a Ptr/List handle).
        // 2026-08-15: check the Briev type directly — `lower_type(Bool, None)`
        // returns "i64" (no universe), so a type-based check was dead.
        if result.ty == crate::ast::Type::bool_() {
            let boxed = self.fun.gen_reg();
            writeln!(out, "  {} = zext i8 {} to i64", boxed, result.name).ok();
            writeln!(out, "  ret i64 {}", boxed).ok();
        } else {
            writeln!(out, "  ret i64 {}", result.name).ok();
        }
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    /// Emit the folded single-bounded-counter `main()` for a node, selecting the
    /// emission strategy from its frontend-computed `LoopShape`.
    ///
    /// Returns `true` when a folded `main()` was emitted; `false` when the shape
    /// says the node cannot be folded (unresolvable bound), so the caller falls
    /// through to the conservative reactor path.
    ///
    /// # Dispatch
    ///
    /// 2026-07-31: The four-way dispatch replaces the previous body of
    /// heuristics (write_density, total_fields thresholds). The decision is now
    /// structural and backend-agnostic:
    ///
    /// ```text
    /// Pure && const bound            → EmitPureCounterFold (O(1) store)
    /// single runtime guard           → emit_version_dag_main (self-deciding)
    /// counter_only && !swan_song     → emit_folded_main (InlineSsa)
    /// vector groups && carried > regs→ emit_countable_main (VectorPhiGroup label)
    /// _                              → emit_countable_main (PerFieldPhi)
    /// ```
    ///
    /// The InlineSsa guardrail (2026-07-29, dispatch-bug-analysis.md) is encoded
    /// structurally: `counter_only_writes` is true exactly when the write set is
    /// `{counter}`, so emit_folded_loop's empty write_set never silently discards
    /// a non-counter state write. version-DAG is tried before InlineSsa/PerFieldPhi
    /// so a body with a single runtime `when` guard (e.g. print_loop) keeps its
    /// guard-absent/guard-present split — see
    /// docs/plans/2026-07-30-flat-node-decomposition.md §11.
    ///
    /// Why per-field phis instead of the old EmitInlineSsa/EmitMemoryCounter paths:
    ///   EmitInlineSsa (inline SSA) used a %slot_case alloca round-trip:
    ///     load %State → extractvalue×N → insertvalue×N → store
    ///     — 33-field struct load/store per iteration hid fields
    ///       from LLVM's induction variable analysis.
    ///   EmitMemoryCounter (memory) kept the counter in %State via GEP+load+store:
    ///     — 3 extra memory uops per tick for the counter alone.
    ///   EmitPerFieldPhi creates per-field phi nodes at the loop header so LLVM
    ///   sees a canonical loop structure (phi + icmp slt + add) that enables
    ///   induction variable analysis, SROA, and loop vectorization. With Path A
    ///   (needs_state_stores_in_body=false) it emits zero memory traffic
    ///   regardless of field count (2026-07-04, MAX_FIELDS_PER_ALLLOCA=15 chunk
    ///   allocas keep SROA happy past 15 phis).
    ///
    /// # Arguments
    ///
    /// * `txn_body` — the swan-song-stripped transaction body (from
    ///   `analysis.swan_songs`); LICM hoisting runs inside.
    /// * `post_hoist` — the hoisted post-loop swan-song tail, assigned to
    ///   `pending_post_hoist` for Path B emission.
    /// 2026-08-16 (multi-node internal fold, Direction 3): is `name` a
    /// counted-loop node whose whole bounded pass can run inside `@txn_<name>`
    /// without starving any other node? The reactor calls the txn once per
    /// pass and sequences the phase-transition nodes around it.
    ///
    /// SOUNDNESS: folding X's pass into one txn call starves any node Y whose
    /// precondition becomes true MID-pass (the reactor never gets control
    /// between X's iterations). The gate proves no such Y exists:
    ///   - X writes nothing Y's pre reads, except the counter itself;
    ///   - Y's pre references the counter only at the pass boundary
    ///     (`i == bound` / `i >= bound`) — an interior `<`/`<=`/`>` reference
    ///     would be true mid-pass — unless Y's pre is gated by `beginprogram`
    ///     (true only at program entry, cleared before any later pass).
    fn internal_fold_info(
        &self,
        name: &str,
        analysis: &crate::backend::AnalysisResults,
    ) -> Option<InternalFoldInfo> {
        let nodes = &analysis.transition_graph.nodes;
        if nodes.len() <= 1 {
            return None; // the single-node main fold handles this
        }
        if self.accel_kernel_idx.contains_key(name) {
            return None; // the accel dispatch wrapper owns the CPU path
        }
        let node = nodes.iter().find(|n| n.name == *name)?;
        if !node.is_reactive {
            return None;
        }
        let shape = match analysis.loop_shapes.get(name) {
            Some(s) => s,
            None => return None,
        };
        let bp = match node.bounded_pre.as_ref() {
            Some(b) => b,
            None => return None,
        };
        let inc = match node.increments.as_ref() {
            Some(i) => i,
            None => return None,
        };
        if bp.var != inc.var {
            return None;
        }
        let counter = &bp.var;
        let (total_idx, total_const_name) = match &shape.bound {
            crate::analysis::loop_shape::Bound::Field(f) => {
                (self.ctx.field_index_map.get(f.as_str()).copied(), None)
            }
            crate::analysis::loop_shape::Bound::Const(c)
            | crate::analysis::loop_shape::Bound::Init(c) => (None, Some(c.clone())),
            _ => (None, None),
        };
        if total_idx.is_none() && total_const_name.is_none() {
            return None;
        }
        // The fields X writes OTHER than the counter — anything Y's pre reads
        // here would be clobbered mid-pass. The transition graph's write_set
        // deliberately EXCLUDES array-element writes (`px[i] = ...`, 2026-07-21
        // — pointer writes change memory AT the pointer), so collect the roots
        // from the body directly.
        let writes_except_counter: std::collections::HashSet<String> =
            collect_written_fields(&node.body)
                .into_iter()
                .filter(|f| f != counter)
                .collect();
        for other in nodes {
            if other.name == *name {
                continue;
            }
            let mut reads = std::collections::HashSet::new();
            crate::backend::collect_expr_identifiers(&other.precondition, &mut reads);
            if reads.iter().any(|r| writes_except_counter.contains(r)) {
                return None;
            }
            if !self.pre_counter_safe(&other.precondition, counter, bp) {
                return None;
            }
        }
        let bound_literal = bp.bound_literal;
        Some(InternalFoldInfo {
            counter_idx: *self.ctx.field_index_map.get(counter)?,
            total_idx,
            total_const_name,
            bound_literal,
            counter_var: counter.clone(),
        })
    }

    /// Emit `@txn_<name>` as an internal PerFieldPhi countdown: the whole
    /// bounded pass runs inside the txn, `%state` is the parameter (no alloca /
    /// init stores — the reactor already initialized state).
    fn emit_internal_fold_txn(
        &mut self,
        out: &mut String,
        name: &str,
        txn: &crate::ast::Transaction,
        analysis: &crate::backend::AnalysisResults,
        info: InternalFoldInfo,
    ) {
        // 2026-09-07: reset cur_block for the fresh function.
        self.fun.cur_block = None;
        writeln!(out, "define void @txn_{}({}) local_unnamed_addr #0 noinline {{", name, self.ctx.state_ptr_param).ok();
        writeln!(out, "  entry:").ok();
        self.fun.txn_name = name.to_string();
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        self.fun.clear_locals();
        self.fun.terminated = false;
        self.fun.in_callable_txn = false;
        self.fun.returns_i64 = false;
        self.fun.fn_ret_ty = "void".to_string();
        self.emit_arena_init(out, "  ");
        // The swan-song-stripped body + post-loop hoist, mirroring the
        // single-node fold (analysis.swan_songs).
        let (txn_body, post_hoist) = match analysis.swan_songs.get(name) {
            Some((stripped, hoisted)) => (stripped.clone(), hoisted.clone()),
            None => (txn.body.clone(), Vec::new()),
        };
        self.fun.pending_post_hoist = post_hoist;
        self.fun.pending_phi_backedge.clear();
        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
        self.ctx.global_free_after = analysis.global_lifetime.free_after.clone();
        let write_set: std::collections::HashSet<String> =
            analysis.transition_graph.nodes.iter()
                .find(|n| n.name == *name)
                .map(|n| n.write_set.clone())
                .unwrap_or_default();
        self.emit_countable_loop_wrapped(
            out,
            name,
            info.counter_idx,
            info.total_idx,
            info.total_const_name.as_deref(),
            info.bound_literal,
            &txn_body,
            &write_set,
            false,
            Some(&info.counter_var),
            txn.contract.watchdog.as_ref(),
            false,
        );
        // The countdown left per-function register caches populated (the
        // counter `i` → its loop register). They must not leak into the NEXT
        // txn function — `emit_transaction`-style emissions resolve `i` via
        // these and would reference a register defined in ANOTHER function
        // (undefined-value IR). The single-node main fold never had this issue
        // (main() is emitted last).
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        self.fun.pending_phi_backedge.clear();
        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
    }

    /// 2026-08-16 (multi-node internal fold): a node's precondition may only
    /// reference the pass counter at the pass boundary (`i == bound` /
    /// `i >= bound`) — a strict-interior reference (`i < bound`, `i <= k`)
    /// would be true mid-pass and the fold would starve that node. The one
    /// exception: a `beginprogram && ...` conjunct (the entry flag is cleared
    /// before any later pass, so the whole pre is false mid-pass).
    fn pre_counter_safe(
        &self,
        pre: &Expr,
        counter: &str,
        bp: &crate::analysis::transition_graph::BoundedPre,
    ) -> bool {
        if contains_beginprogram_conjunct(pre) {
            return true;
        }
        !expr_has_unsafe_counter_ref(pre, counter, &bp.bound_var, bp.bound_literal)
    }

    fn emit_folded_loop_shape(        &mut self,
        out: &mut String,
        analysis: &crate::backend::AnalysisResults,
        node: &crate::analysis::transition_graph::ReactorNode,
        counter_idx: usize,
        shape: &crate::analysis::loop_shape::LoopShape,
        txn_body: &[Statement],
        post_hoist: Vec<Vec<Statement>>,
        watchdog: Option<&crate::ast::top::WatchdogSpec>,
    ) -> bool {
        let bp = node.bounded_pre.as_ref().unwrap();
        // 2026-07-31: Bound resolution maps the structured Bound to the backend's
        // own index/const tables (field first, then const; literal/unknown →
        // neither) — mirroring the old total_idx / total_const_name lookup so
        // literal-bound txns reach emit_countable_main with both None exactly as
        // before (emit_countable_load_bound falls back to `add i64 0, 1`).
        // 2026-08-09 (init kind, Phase 3): an `init` bound reuses the
        // total_const_name slot — emit_countable_load_bound loads the seeded
        // global the same way it loads a const global (the Init global is
        // mutable-but-seeded-once, so the load reads the seeded value).
        let (total_idx, total_const_name) = match &shape.bound {
            crate::analysis::loop_shape::Bound::Field(name) => {
                (self.ctx.field_index_map.get(name.as_str()).copied(), None)
            }
            crate::analysis::loop_shape::Bound::Const(name)
            | crate::analysis::loop_shape::Bound::Init(name) => (None, Some(name.as_str())),
            crate::analysis::loop_shape::Bound::Literal(_) | crate::analysis::loop_shape::Bound::Unknown(_) => {
                (None, None)
            }
        };
        // 2026-07-31: Only fold when the bound resolves to something the emitters
        // can load. Reproduces the old
        // `total_idx.is_some() || total_const_name.is_some() || bound_literal.is_some()`
        // gate exactly: a Field bound missing from field_index_map, or an Unknown
        // bound, falls through to the reactor path.
        let bound_resolvable = total_idx.is_some()
            || total_const_name.is_some()
            || matches!(shape.bound, crate::analysis::loop_shape::Bound::Literal(_));
        if !bound_resolvable {
            return false;
        }
        // 2026-07-29: Briev-level LICM — hoist loop-invariant let-bindings.
        // The hoisted bindings are prepended to the body so LLVM's LICM
        // can hoist them to the preheader. See analysis/licm.rs.
        let state_fields: HashSet<String> = self.ctx.field_index_map.keys().cloned().collect();
        let (hoisted_reordered, body_stmts) = crate::analysis::licm::hoist_loop_invariants(
            txn_body, &node.write_set, &state_fields,
        );
        // Prepend hoisted bindings to body (they appear at loop entry).
        let body_stmts: Vec<Statement> = hoisted_reordered.into_iter()
            .chain(body_stmts).collect();

        // ── EmitPureCounterFold ─────────────────────────────────────
        // Pure body + constant bound → O(1) counter-only store, no loop.
        // 2026-07-03: A swan song makes the body non-pure — the hoisted print
        // would be silently dropped by the fold, so the fold is blocked exactly
        // when the swan-song hoist fires (frontend has_swan_song).
        if !shape.has_swan_song && shape.is_pure {
            // 2026-07-31: `total_val` mirrors the old field_initializers-then-
            // constants lookup exactly — a Literal bound never folds (the old
            // total_val was None for it), so literal-bound pure txns still reach
            // the version-DAG / InlineSsa / PerFieldPhi emitters below. This keeps
            // Phase 1b strictly behavior-preserving ("pure fold stays as-is",
            // plan §6.2).
            let total_val = match &shape.bound {
                crate::analysis::loop_shape::Bound::Field(name) => self.ctx.field_initializers
                    .get(name.as_str())
                    .and_then(|e| e.as_ref())
                    .and_then(|e| if let Expr::Decimal(n) = e { Some(*n) } else { None }),
                crate::analysis::loop_shape::Bound::Const(name) => self.ctx.constants
                    .get(name.as_str())
                    .and_then(|(_, e)| if let Expr::Decimal(n) = e { Some(*n) } else { None }),
                crate::analysis::loop_shape::Bound::Literal(_) | crate::analysis::loop_shape::Bound::Unknown(_) => None,
                // 2026-08-09 (init kind, Phase 3): an init's value is only
                // known at runtime — the O(1) pure-counter fold (a compile-time
                // store count) cannot fire; the seeded-bound loop path runs.
                crate::analysis::loop_shape::Bound::Init(_) => None,
            };
            if let Some(tv) = total_val {
                // 2026-07-14: Wrap in define i32 @main() so emitted IR is valid.
                self.warnings.push(format!("info: txn '{}' dispatched via pure counter fold ({} iterations, O(1) store)", node.name, tv));
                self.emit_main_header(out, "#9", true);
                self.emit_state_base(out);
                self.emit_inline_init_stores(out, "%state");
                self.emit_folded_pure_counter(out, counter_idx, tv);
                if self.ctx.exit_condition.is_some() {
                    self.emit_exit_check(out);
                    // 2026-08-06 (fix): emit_exit_check emits a bare
                    // `.continue:` label intended for a loop body. The pure
                    // fold has no loop — the O(1) store already finished, so
                    // the continue path just falls through to exit. Without
                    // this bridge, `.continue:` is an empty, unterminated block
                    // and clang rejects the module (`.end:` "expected
                    // instruction opcode").
                    writeln!(out, "  br label %.end").ok();
                    writeln!(out, ".end:").ok();
                }
                writeln!(out, "  ret i32 0").ok();
                writeln!(out, "}}").ok();
                return true;
            }
        }

        // 2026-07-31: Countdown-loop decomposition — a SINGLE tight loop with a
        // loop-carried `%rem` counter, a cold guard block firing every N
        // iterations. Tried before version-DAG. The hypothesis (plan
        // 2026-07-31-fmn-countdown-vs-batch-and-new-benchmarks §3): the
        // countdown is universal for periodic post-increment guards — it
        // removes the version-DAG's modulo + body-split, and its `%fire`
        // conditional blocks LLVM's mis-vectorization of cross-indexed matrix
        // bodies (which is why the batch's pure inner loop regressed fmn).
        // A/B vs the batch + version-DAG is recorded in the plan's §10.
        let is_decreasing_vd = bp.direction == crate::analysis::transition_graph::ConvergeDirection::Decreasing;
        if !is_decreasing_vd {
            if let Some(batch) = analysis.batch_shape.as_ref() {
                if batch.counter == bp.var {
                    self.fun.pending_post_hoist = post_hoist.clone();
                    self.warnings.push(format!(
                        "info: txn '{}' dispatched via countdown loop (N={}, {} fields)",
                        node.name, batch.batch_size, node.write_set.len()
                    ));
                    self.emit_countable_countdown_main(
                        out, &node.name, counter_idx, total_idx, total_const_name, bp.bound_literal,
                        &node.write_set, &bp.var, batch,
                        watchdog,
                        analysis.global_lifetime.free_after.get(&node.name)
                            .map(|v| v.as_slice()).unwrap_or(&[]),
                    );
                    return true;
                }
            }
        }
        // 2026-07-31: Composite-node decomposition (version-DAG). Tried after the
        // batch-loop — a body with a single runtime `when` guard is handled by
        // the guard-absent/guard-present emission and supersedes PerFieldPhi.
        // See docs/plans/2026-07-30-flat-node-decomposition.md §11.
        self.fun.pending_post_hoist = post_hoist.clone();
        if self.emit_version_dag_main(
            out, counter_idx, total_idx, total_const_name, bp.bound_literal,
            &body_stmts, &node.write_set, is_decreasing_vd, Some(&bp.var),
            analysis.global_lifetime.free_after.get(&node.name)
                .map(|v| v.as_slice()).unwrap_or(&[]),
        ) {
            return true;
        }
        // 2026-07-31: version-DAG did not handle the body — remaining paths use the
        // FULL body (no guard stripping) with plain PerFieldPhi (batch_info = None).
        let inner_body = body_stmts.clone();

        // ── InlineSsa ───────────────────────────────────────────────
        // counter-only write sets are the ONLY safe input (emit_folded_loop passes
        // an empty write_set to emit_countable_body and would silently discard any
        // non-counter state write). `counter_only_writes` encodes that structurally.
        if shape.counter_only_writes && !shape.has_swan_song {
            let total_fields = self.ctx.field_index_map.len();
            self.fun.pending_post_hoist = post_hoist.clone();

            self.warnings.push(format!("info: txn '{}' dispatched via inline SSA ({} fields)", node.name, total_fields));

            self.emit_folded_main(out, &node.name, counter_idx, total_idx, total_const_name, bp.bound_literal, false, Some(&body_stmts), Some(&bp.var));
            return true;
        }

        // ── VectorPhiGroup vs PerFieldPhi ───────────────────────────
        // Isomorphic groups (with the backend's same-type gate applied) on a wide
        // carried set select the vector-phi label. Emission is identical today —
        // emit_countable_main clears active_vector_groups (counter.rs:241) — so the
        // split only records the intended strategy for future vector-phi emission.
        let vg = self.shape_vector_groups(&shape.vector_groups, &node.write_set);
        let carried_len = shape.carried_fields.len();
        let regs = self.ctx.float_register_count();
        let total_fields = self.ctx.field_index_map.len();
        let write_count = node.write_set.len();
        if !vg.is_empty() && carried_len > regs {
            let field_count: usize = vg.iter().map(|g| g.width).sum();
            self.warnings.push(format!("info: txn '{}' dispatched via vector phi ({}/{} fields in {} groups)", node.name, field_count, total_fields, vg.len()));
        } else {
            self.warnings.push(format!("info: txn '{}' dispatched via per-field phi ({}/{} fields written)", node.name, write_count, total_fields));
        }
        self.fun.pending_post_hoist = post_hoist;
        let is_decreasing = bp.direction == crate::analysis::transition_graph::ConvergeDirection::Decreasing;
        self.emit_countable_main(
            out, &node.name, counter_idx, total_idx, total_const_name, bp.bound_literal,
            &inner_body, &node.write_set, is_decreasing, Some(&bp.var),
            watchdog,
        );
        true
    }

    /// Convert frontend structural vector groups to backend `VectorPhiGroup`s,
    /// applying the LLVM same-type gate the structural pass cannot express.
    ///
    /// 2026-07-31: The frontend `LoopShape` carries isomorphic groups already
    /// filtered for write-set membership, power-of-2 width, duplicate fields, and
    /// overlap (loop_shape::detect_vector_groups_structural). The backend re-applies
    /// the same-LLVM-type check it used in `detect_vector_groups` so a mixed-type
    /// group never influences dispatch. See
    /// docs/plans/2026-07-31-frontend-driven-dispatch.md §6.2.
    fn shape_vector_groups(
        &self,
        groups: &[crate::analysis::loop_shape::VectorGroup],
        write_set: &HashSet<String>,
    ) -> Vec<crate::backend::llvm::vector_phi::VectorPhiGroup> {
        let mut accepted: HashSet<String> = HashSet::new();
        let mut out_groups: Vec<crate::backend::llvm::vector_phi::VectorPhiGroup> = Vec::new();
        for g in groups {
            // All fields must be unconditionally written (in write_set).
            if !g.fields.iter().all(|f| write_set.contains(f)) {
                continue;
            }
            let element_ty = match g.fields.first() {
                Some(first) => {
                    let Some(&idx) = self.ctx.field_index_map.get(first.as_str()) else { continue; };
                    self.ctx.field_types.get(idx).cloned().unwrap_or_else(|| "i64".to_string())
                }
                None => continue,
            };
            // All fields must have the same LLVM type.
            if !g.fields.iter().all(|f| {
                self.ctx.field_index_map.get(f.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .map_or(false, |t| t == &element_ty)
            }) {
                continue;
            }
            // Skip if ANY field is already in an accepted group (no overlap).
            if g.fields.iter().any(|f| accepted.contains(f)) {
                continue;
            }
            for f in &g.fields {
                accepted.insert(f.clone());
            }
            out_groups.push(crate::backend::llvm::vector_phi::VectorPhiGroup {
                name: g.name.clone(),
                element_ty,
                width: g.width,
                fields: g.fields.clone(),
                phi_reg: String::new(),
                backedge_reg: String::new(),
            });
        }
        out_groups
    }

    /// Return the optimization report lines collected during `generate()`.
    pub fn report(&self) -> &[String] {
        &self.report_lines
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Return any extra flags needed for `opt`. Currently emits
    /// `-slp-vectorize-hor=false` when SLP hazards exist, since LLVM 18's
    /// 2026-07-29: SLP hazard gating removed — proven counterproductive.
    /// No global SLP-disable flags needed. LLVM's auto-vectorizer runs freely.
    pub fn llvm_extra_flags(&self) -> Vec<String> {
        Vec::new()
    }

    /// Select the LLVM attribute group for a function, adjusting for SLP hazard.
    /// Non-hazardous functions use #0 or #3; hazardous functions use #4 or #5
    /// (which disable SLP vectorization). See hazard.rs for the analysis.
    /// Check if an expression produces a `Ptr<T>` value.
    /// Used by `ListIndex` to decide between direct pointer GEP vs 2-slot header load.
    fn is_ptr_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Field(_, target) => target == "Ptr",
            Expr::Identifier(name) => {
                self.fun.let_binding_types.get(name)
                    .map(|t| matches!(t, Type::Applied(n, _) if n == "Ptr")
                        || matches!(t, Type::LayoutPtr(_)))
                    .unwrap_or(false)
            }
            _ => false,
        }
    }

    /// 2026-07-26: Removed type-level validation — schema types were from .dbvs era.
    /// The V2 parser's FieldType handles all type compatibility at parse time.
    fn validate_schema_types(&mut self) {
        // No-op: alias validation is now name-only via schema_alias_names.
    }

    // ── Field index ───────────────────────────────────────────
    fn build_field_index(&mut self, items: &[TopLevel]) {
        self.ctx.field_index_map.clear();
        self.ctx.field_types.clear();
        self.ctx.field_initializers.clear();
        if !self.ctx.mmio_prepopulated {
            self.ctx.mmio_fields.clear();
            self.ctx.mmio_initializers.clear();
        }
        for item in items {
            if let TopLevel::StateDecl(s) = item {
                // 2026-07-14: Preserve prepopulated MMIO addresses from with_mmio_addresses()
                let addr = if self.ctx.mmio_prepopulated && self.ctx.mmio_fields.contains_key(&s.name) {
                    *self.ctx.mmio_fields.get(&s.name).unwrap()
                } else {
                    0u64
                };
                self.ctx.mmio_fields.insert(s.name.clone(), addr);
                self.ctx.mmio_initializers.insert(s.name.clone(), None);
                if self.ctx.mmio_prepopulated && self.ctx.mmio_fields.contains_key(&s.name) {
                    if self.ctx.schema_alias_names.is_empty() || self.ctx.schema_alias_names.contains(&s.name) {
                        self.ctx.mmio_initializers.insert(s.name.clone(), None);
                    } else {
                        // Not in any imported schema — remove from mmio_fields to prevent
                        // accidental MMIO routing in reads/writes.
                        self.ctx.mmio_fields.remove(&s.name);
                        self.ctx.field_index_map
                            .insert(s.name.clone(), self.ctx.field_types.len());
                        self.push_field_type(&s.ty);
                        self.ctx.field_initializers.insert(s.name.clone(), None);
                    }
                } else {
                    self.ctx.field_index_map
                        .insert(s.name.clone(), self.ctx.field_types.len());
                    self.push_field_type(&s.ty);
                    self.ctx.field_initializers.insert(s.name.clone(), None);
                }
            } else if let TopLevel::Trigger(trg) = item {
                // 2026-07-14: Trigger.ty removed - use string as default type.
                self.ctx.field_index_map
                    .insert(trg.name.clone(), self.ctx.field_types.len());
                self.push_field_type(&Type::string());
                self.ctx.field_initializers.insert(trg.name.clone(), None);
            } else if let TopLevel::TriggerBinding { name, ty, .. } = item {
                // Trigger bindings (trg name: Type @ Console!) get a state slot
                // like regular triggers, so emit_expr can load their value.
                let trig_ty = ty.clone().unwrap_or(crate::ast::Type::string());
                self.ctx.field_index_map
                    .insert(name.clone(), self.ctx.field_types.len());
                self.push_field_type(&trig_ty);
                self.ctx.field_initializers.insert(name.clone(), None);
            // 2026-07-14: Top-level `let name: Type = expr;` — register as state field.
            // The initializer expr is stored in field_initializers so emit_init_state
            // can evaluate and store the runtime value at startup.
            } else if let TopLevel::Statement(stmt) = item {
                if let crate::ast::Statement::Let { name, ty, expr, .. } = stmt.as_ref() {
                    let field_ty = ty.clone().unwrap_or(crate::ast::Type::int());
                    // 2026-08-07 (object instance pools): a top-level obj
                    // instance (`let st: Stack<Int, 256> = 0`) UNPACKS its
                    // members into prefixed top-level slots (`st.data`,
                    // `st.len`) — no instance address slot, no boxed struct.
                    // Member types are substituted with the concrete const
                    // args (the mono_subst map ensure_mono uses).
                    let unpacked: Option<Vec<(String, Type)>> = match &field_ty {
                        // Only OBJ bases unpack — List<T> and other Applied
                        // collections with a slot layout stay plain fields.
                        // A LIST-LITERAL initializer (`let q: List<Int> =
                        // [0]`) constructs a heap collection VALUE, not an
                        // instance pool.
                        // 2026-08-15 (coll plan): a GROWABLE coll (`Ptr<T>`
                        // sequence) is a heap value — never unpacked. A FIXED
                        // `T[N]` coll may unpack (the Stack shape). `seq` on a
                        // fixed coll forces inline (no pool columns).
                        Type::Applied(base, args)
                            if self.ctx.obj_members.contains_key(base)
                                && !self.is_heap_coll(base)
                                && !matches!(expr, Some(Expr::List(_)) | Some(Expr::Tuple(_))) =>
                        {
                            let params = self.ctx.obj_type_params.get(base).cloned().unwrap_or_default();
                            let subst: std::collections::HashMap<String, Type> =
                                params.into_iter().zip(args.iter().cloned()).collect();
                            Some(
                                self.ctx.struct_types.get(base).unwrap().iter()
                                    .map(|(mname, mty)| {
                                        (mname.clone(), crate::typechecker::substitute_type(mty, &subst))
                                    })
                                    .collect(),
                            )
                        }
                        // 2026-08-07: a non-generic obj (`let c: Counter = 0`)
                        // — no const args to substitute.
                        Type::Custom(base) if self.ctx.obj_members.contains_key(base) && !self.is_heap_coll(base) => {
                            Some(
                                self.ctx.struct_types.get(base).unwrap().iter()
                                    .map(|(mname, mty)| (mname.clone(), mty.clone()))
                                    .collect(),
                            )
                        }
                        _ => None,
                    };
                    if let Some(slots) = unpacked {
                        let base = match &field_ty {
                            Type::Applied(base, _) | Type::Custom(base) => base.clone(),
                            _ => String::new(),
                        };
                        // 2026-08-09 (Phase 5): a box/spill base has NO shared
                        // pool — every instance is its own heap block. Register
                        // the per-instance member layout (member → byte offset
                        // within the block) so member access can inttoptr the
                        // handle + GEP the offset. The static top-level instance
                        // keeps its unpacked slots; spawned handles allocate.
                        if let Some(_storage) = self.ctx.spawn_storage.get(&base) {
                            let mut offsets: std::collections::HashMap<String, (u64, crate::ast::Type)> =
                                std::collections::HashMap::new();
                            let mut off = 0u64;
                            for (mname, mty) in &slots {
                                offsets.insert(mname.clone(), (off, mty.clone()));
                                off += crate::backend::llvm::types::type_size(
                                    mty, self.ctx.type_universe.as_ref(),
                                );
                            }
                            self.ctx.boxed_offsets.insert(base.clone(), offsets);
                        }
                        // 2026-08-07 (object instance pools): register the
                        // allocator counter + per-member COLUMNS. Row 0 is the
                        // static instance; spawn allocates the free rows.
                        // 2026-08-09 (Bug 1): a base with NO top-level instance
                        // (spawn-only) has no unpacked registration — the
                        // fallback pass at the end of build_field_index calls
                        // this for every base in spawn_pools/dependent_pools.
                        self.register_pool_columns(&base, &slots);
                        // 2026-08-28: store the MONO key (e.g. "Stack<Int,8>")
                        // so downstream struct lookups get substituted fields
                        // with correct byte offsets (Phase 3 fix).
                        let mono_key = match &field_ty {
                            Type::Applied(b, args) => self.ensure_mono(b, args),
                            _ => base.clone(),
                        };
                        if let Some(init_expr) = expr {
                            self.ctx.obj_instance_inits.insert(name.clone(), (mono_key, init_expr.clone()));
                        }
                    } else {
                        self.ctx.field_index_map
                            .insert(name.clone(), self.ctx.field_types.len());
                        self.push_field_type(&field_ty);
                        self.ctx.field_initializers.insert(name.clone(), expr.clone());
                    }
                }
            } else if let TopLevel::Cell(c) = item {
                // Cell fields are handled differently depending on whether the
                // cell is persistent (threaded) or sync (non-threaded).
                //
                // For threaded cells: fields go into a separate %CellState.<name>
                // type so the thread function can operate on its own struct without
                // sharing %State. The %CellState.* definitions are built here and
                // emitted in declare_state_type.
                //
                // For non-threaded persistent cells (used by @cell_persistent_ticks):
                // fields stay in %State as prefixed slots, accessed via the same
                // GEP path as program state fields.
                //
                // Sync CellCall codegen always allocates %CellState.* locally and
                // accesses fields through the cell_state_types map.
                if c.is_persistent {
                    // Build cell-state type info for this persistent cell
                    let mut cs_imap: HashMap<String, usize> = HashMap::new();
                    let mut cs_tys: Vec<String> = Vec::new();
                    for (field_name, field_ty) in &c.fields {
                        let prefixed = format!("cell${}${}", c.name, field_name);
                        cs_imap.insert(prefixed.clone(), cs_tys.len());
                        cs_tys.push(self.llvm_type(field_ty).to_string());
                        // Also register in %State for cell_persistent_ticks access
                        self.ctx.field_index_map.insert(prefixed.clone(), self.ctx.field_types.len());
                        self.push_field_type(field_ty);
                        self.ctx.field_initializers.insert(prefixed, None);
                    }
                    for (param_name, param_ty) in &c.parameters {
                        let prefixed = format!("cell${}${}", c.name, param_name);
                        cs_imap.insert(prefixed.clone(), cs_tys.len());
                        cs_tys.push(self.llvm_type(param_ty).to_string());
                        // Also register in %State
                        self.ctx.field_index_map.insert(prefixed.clone(), self.ctx.field_types.len());
                        self.push_field_type(param_ty);
                        self.ctx.field_initializers.insert(prefixed, None);
                    }
                    // Register internal trigger fields in %State and %CellState.*
                    // 2026-07-14: Trigger.ty removed - use string as default type.
                    for trg in &c.internal_triggers {
                        let prefixed = format!("cell${}${}", c.name, trg.name);
                        cs_imap.insert(prefixed.clone(), cs_tys.len());
                        cs_tys.push(self.llvm_type(&Type::string()).to_string());
                        self.ctx.field_index_map.insert(prefixed.clone(), self.ctx.field_types.len());
                        self.push_field_type(&Type::string());
                        self.ctx.field_initializers.insert(prefixed, None);
                    }
                    self.ctx.cell_state_types.insert(c.name.clone(), (cs_imap, cs_tys));
                    // 2026-07-14: reactor_speed removed from Transaction. Thread speed not available.
                    let has_thread_speed = false;
                    if has_thread_speed {
                        self.cell_thread_names.push(c.name.clone());
                    }
                } else {
                    // Non-persistent (sync) cell: add to %State as prefixed slots
                    for (field_name, field_ty) in &c.fields {
                        let prefixed = format!("cell${}${}", c.name, field_name);
                        self.ctx.field_index_map.insert(prefixed.clone(), self.ctx.field_types.len());
                        self.push_field_type(field_ty);
                        self.ctx.field_initializers.insert(prefixed, None);
                    }
                    for (param_name, param_ty) in &c.parameters {
                        let prefixed = format!("cell${}${}", c.name, param_name);
                        self.ctx.field_index_map.insert(prefixed.clone(), self.ctx.field_types.len());
                        self.push_field_type(param_ty);
                        self.ctx.field_initializers.insert(prefixed, None);
                    }
                }
            }
        }
    }

    // ── Chimera tracking (Phase 3) ──────────────────────────
    /// Check if a register holds a chimera value.
    pub(crate) fn is_chimera(&self, reg_name: &str) -> bool {
        self.fun.chimera_map.get(reg_name).map_or(false, |c| c.is_chimera)
    }

    /// Get the backing type of a chimera value, if any.
    pub(crate) fn chimera_backing(&self, reg_name: &str) -> Option<&str> {
        self.fun.chimera_map.get(reg_name)
            .filter(|c| c.is_chimera)
            .map(|c| c.backing_type.as_str())
    }

    /// Mark a register as a chimera value with the given backing type.
    pub(crate) fn mark_chimera(&mut self, reg_name: &str, backing_type: &str) {
        if let Some(c) = self.fun.chimera_map.get_mut(reg_name) {
            c.is_chimera = true;
            c.backing_type = backing_type.to_string();
        } else {
            self.fun.chimera_map.insert(reg_name.to_string(), ChimeraInfo { is_chimera: true, backing_type: backing_type.to_string() });
        }
    }

    /// Compute the total size of the %State struct in bytes from field_types.
    /// Used by the memcpy round-trip SROA optimization in emit_main.
    pub(crate) fn compute_state_size_bytes(&self) -> i64 {
        self.ctx.field_types.iter().map(|t| llvm_type_byte_size(t)).sum()
    }

    // ── Adaptive Layout — apply field modes ──────────────────
    /// Apply field liveness analysis to eliminate dead fields and add
    /// cache slots for lazily-computed projections.
    ///
    /// Why this runs after the transition graph is built: the transition
    /// graph's live_fields analysis can only be accurate after the full
    /// region analysis (which determines which txns fire under which
    /// conditions). Running dead-field elimination earlier would conservatively
    /// keep all fields, missing elimination opportunities.
    pub(crate) fn apply_field_modes(&mut self, items: &[TopLevel],
        live_fields: &std::collections::HashSet<String>,
        projection_usage: &std::collections::HashMap<String, std::collections::HashSet<String>>)
    {
        let all_state_fields: std::collections::HashSet<String> = self.ctx.field_index_map.keys().cloned().collect();
        let referenced_fields = crate::analysis::transition_graph::compute_referenced_fields(items);
        // Union with live_fields to prevent elimination of precondition-only,
        // postcondition-only, or exit-condition-only fields. live_fields includes
        // fields referenced in contracts and #!exit expressions.
        let referenced_fields: std::collections::HashSet<String> = referenced_fields.union(live_fields).cloned().collect();
        self.ctx.field_modes = crate::analysis::transition_graph::assign_field_modes(
            &all_state_fields, &referenced_fields, projection_usage);
        // Triggers must never be eliminated — they're accessed by the event loop,
        // not by txn body identifiers.
        for name in &self.ctx.trigger_names {
            self.ctx.field_modes.insert(name.clone(), crate::analysis::FieldMode::Always);
        }
        // Cell fields and parameters must never be eliminated — they're accessed
        // by Expr::CellCall codegen with cellular$name prefixed names.
        for name in self.ctx.field_index_map.keys() {
            if name.starts_with("cell$") {
                self.ctx.field_modes.insert(name.clone(), crate::analysis::FieldMode::Always);
            }
        }
        // Synthetic cycle_count field must never be eliminated — it's maintained
        // by the tick loop, not by txn body code.
        if let Some(idx) = self.ctx.field_index_map.get("cycle_count") {
            self.ctx.field_modes.insert("cycle_count".to_string(), crate::analysis::FieldMode::Always);
        }
        // 2026-08-11 (view wiring): fields referenced by web view bindings are
        // consumed by the DOM — observable, hence live (observability-as-liveness).
        // They can be read-only (a `b-text` on a setup `let` never written by any
        // txn), so the body-driven liveness scan would prune them. Never do so:
        // the shim's binding table maps these names to handles.
        for name in &self.ctx.view_bound_fields {
            if self.ctx.field_index_map.contains_key(name) {
                self.ctx.field_modes.insert(name.clone(), crate::analysis::FieldMode::Always);
            }
        }
        // 2026-07-19: Arena system fields must never be eliminated — they're
        // accessed by emit_arena_alloc via %State field indices, not by identifiers.
        for arena_name in &["__arena_ptr", "__arena_end", "__arena_base"] {
            if let Some(idx) = self.ctx.field_index_map.get(*arena_name) {
                self.ctx.field_modes.insert(arena_name.to_string(), crate::analysis::FieldMode::Always);
            }
        }
        self.ctx.cache_slots.clear();

        // Phase 1: Remove Never fields from field_index_map and field_types.
        // Rebuild both from scratch to handle index shifting correctly.
        // IMPORTANT: sort by original index to preserve deterministic field ordering.
        let mut old_pairs: Vec<(String, usize)> = self.ctx.field_index_map.drain().collect();
        let old_types = std::mem::take(&mut self.ctx.field_types);
        let old_briev_types = std::mem::take(&mut self.ctx.field_briev_types);
        self.ctx.field_index_map.reserve(old_pairs.len());
        self.ctx.field_types.reserve(old_types.len());
        self.ctx.field_briev_types.reserve(old_briev_types.len());
        old_pairs.sort_by_key(|(_, idx)| *idx);

        for (name, _old_idx) in &old_pairs {
            // 2026-07-16: Default to Always for fields without an explicit mode.
            // Fields referenced only in defn bodies (not txn bodies) don't get a
            // field_modes entry — changing Never→Always prevents their accidental
            // elimination while still allowing explicitly-marked fields to be pruned.
            // 2026-08-07 (object instance pools): unpacked instance slots are
            // ALWAYS live (the field-liveness scan does not yet walk member
            // bodies — pruning them would drop the instance's state).
            let mode = if self.ctx.instance_slots.contains(name) {
                crate::analysis::FieldMode::Always
            } else {
                self.ctx.field_modes.get(name).copied().unwrap_or(crate::analysis::FieldMode::Always)
            };
            match mode {
                crate::analysis::FieldMode::Never => {
                    // Eliminate this field from %State entirely
                }
                crate::analysis::FieldMode::Always | crate::analysis::FieldMode::LazyCached { .. } => {
                    let new_idx = self.ctx.field_types.len();
                    let orig_type_idx = old_pairs.iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, i)| *i)
                        .unwrap_or(0);
                    self.ctx.field_index_map.insert(name.clone(), new_idx);
                    self.ctx.field_types.push(old_types[orig_type_idx].clone());
                    // 2026-06-29: Preserve the original Briev type alongside LLVM type
                    self.ctx.field_briev_types.push(
                        old_briev_types.get(orig_type_idx).cloned().unwrap_or(Type::int())
                    );
                }
            }
        }

        // 2026-07-21: Update stale arena/ringbuf indices after field elimination.
        // The rebuild above may have shifted field indices; re-read them from
        // the new field_index_map to prevent out-of-bounds GEPs.
        self.arena_ptr_idx = self.ctx.field_index_map.get("__arena_ptr").copied();
        self.arena_end_idx = self.ctx.field_index_map.get("__arena_end").copied();
        self.arena_base_idx = self.ctx.field_index_map.get("__arena_base").copied();

        // Phase 2: Append cache slots — one per (field, projection_target) pair.
        for (name, mode) in &self.ctx.field_modes {
            if let crate::analysis::FieldMode::LazyCached { cache_index: _ } = mode {
                let targets = projection_usage.get(name);
                let mut target_map: HashMap<String, (usize, usize)> = HashMap::new();
                if let Some(targets) = targets {
                    for target_name in targets {
                        let cache_idx = self.ctx.field_types.len();
                        self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
                        let valid_idx = self.ctx.field_types.len();
                        self.ctx.field_types.push("i8".to_string());
                        self.ctx.field_briev_types.push(Type::bool_());
                        target_map.insert(target_name.clone(), (cache_idx, valid_idx));
                    }
                }
                // Fallback: at least one cache slot even if projection_usage is empty
                if target_map.is_empty() {
                    let cache_idx = self.ctx.field_types.len();
                    self.ctx.field_types.push("i64".to_string());
            self.ctx.field_briev_types.push(Type::int());
                    let valid_idx = self.ctx.field_types.len();
                    self.ctx.field_types.push("i8".to_string());
                        self.ctx.field_briev_types.push(Type::bool_());
                    target_map.insert("_".to_string(), (cache_idx, valid_idx));
                }
                self.ctx.cache_slots.insert(name.clone(), target_map);
            }
        }

        // 2026-08-09 (Bug 1): a base that is ONLY spawned (no top-level
        // `let c: Obj = ...` unpacked instance) never ran the pool registration
        // above. Register the allocator counter + member columns for every
        // base the frontend spawn analysis knows about, so `spawn Obj()`
        // doesn't panic on a missing pool and the member bodies resolve their
        // columns instead of a nonexistent `@member` global.
        let pool_bases: Vec<String> = self.ctx.spawn_pools.keys()
            .chain(self.ctx.dependent_pools.keys())
            .chain(self.ctx.spawn_storage.keys())
            .cloned()
            .collect();
        let mut pool_bases = pool_bases;
        pool_bases.sort();
        pool_bases.dedup();
        for base in &pool_bases {
            // 2026-08-26 (async Phase D): a PORTS-ONLY obj (no members —
            // e.g. a pure event bus) still gets a pool: its port columns are
            // the whole instance surface.
            if !self.ctx.obj_members.contains_key(base)
                && !self.ctx.obj_port_wiring.contains_key(base)
            {
                continue;
            }
            // 2026-08-15 (coll plan): a growable coll (Ptr<T>) is a heap
            // value — it never gets pool columns (SPAWN-only or not).
            if self.is_heap_coll(base) {
                continue;
            }
            let slots: Vec<(String, Type)> = self.ctx.struct_types.get(base)
                .cloned()
                .unwrap_or_default();
            if slots.is_empty() {
                continue;
            }
            // Box/spill bases register the per-instance member layout (their
            // spawns are heap blocks, not pool columns) and never get a
            // counter/column.
            if let Some(_storage) = self.ctx.spawn_storage.get(base) {
                if !self.ctx.boxed_offsets.contains_key(base) {
                    let mut offsets: std::collections::HashMap<String, (u64, Type)> =
                        std::collections::HashMap::new();
                    let mut off = 0u64;
                    for (mname, mty) in &slots {
                        offsets.insert(mname.clone(), (off, mty.clone()));
                        off += crate::backend::llvm::types::type_size(
                            mty, self.ctx.type_universe.as_ref(),
                        );
                    }
                    self.ctx.boxed_offsets.insert(base.clone(), offsets);
                }
                continue;
            }
            let counter_name = format!("__spawn_next_{}", base);
            if self.ctx.field_index_map.contains_key(&counter_name) {
                continue;
            }
            self.register_pool_columns(base, &slots);
        }

        // 2026-08-11 (2b2 slice 2b): seed the component-instance slots from
        // mount props — overrides the None the StateDecl registration inserted.
        for (slot, init) in &self.ctx.component_initializers {
            if self.ctx.field_index_map.contains_key(slot) {
                self.ctx.field_initializers.insert(slot.clone(), Some(init.clone()));
            }
        }
    }

    /// 2026-08-07 (object instance pools): register a base's allocator counter
    /// (`__spawn_next_<base>`, starts at row 1) and per-member COLUMNS —
    /// `[capacity x T]` for a scalar, `[capacity x [N x T]]` for a member
    /// array, or a dependent heap buffer when the spawn count is runtime-bound
    /// (SPEC §16.6). Row 0 is the static instance (or the first spawned row for
    /// spawn-only bases); the pool is provably inexhaustible. Idempotent — the
    /// counter check skips bases already registered via a top-level instance.
    /// 2026-08-15 (coll plan): is `base` a growable (heap-backed) coll? A
    /// `Ptr<T>`-sequenced coll value must outlive the creating scope, so it is
    /// never unpacked to pool columns (SPEC §8.10 storage matrix).
    fn is_heap_coll(&self, base: &str) -> bool {
        // 2026-08-17 (storage correctness, plan 2026-08-17-hashmap-storage-tuple-correctness.md):
        // storage is the compiler's efficiency decision. A `coll` with a
        // `Ptr<T>` sequence is a heap value (never unpacked). A hand-written
        // `obj` (HashMap, Stack) UNPACKS to SoA columns — the intentional
        // instance-pool design. The 2026-08-16 op-surface check is REVERTED:
        // forcing a collection obj onto the boxed path contradicted the
        // design (emit_expr.rs:2484 "a pool instance must NEVER reach the
        // boxed path").
        matches!(
            self.ctx.coll_storage.get(base),
            Some(crate::backend::llvm::coll_scaffold::CollStorage::HeapGrowable)
        )
    }

    /// 2026-08-17 (storage correctness): is a type a collection by its OP
    /// SURFACE — a hand-written `obj` declaring `op InsertAt`/`op Count`/
    /// `op Init`/`op Iter`? STORAGE-INDEPENDENT: unlike `is_heap_coll` (which
    /// decides SoA-unpack vs box for STATE fields), this decides whether a
    /// LOCAL let constructs through the collection ops (Init/InsertAt) instead
    /// of binding a raw scalar. A local collection can never be an unpacked
    /// column (per-firing), so a local op-surface collection boxes.
    fn is_op_surface_coll(&self, base: &str) -> bool {
        self.ctx.operator_defs.get(base).map_or(false, |defs| {
            defs.iter().any(|d| {
                d.op == "InsertAt" || d.op == "Count" || d.op == "Init" || d.op == "Iter"
            })
        })
    }

    /// 2026-08-15 (coll plan §3.4.6): is `ty` a `coll` type (compiler-owned
    /// Length)? Checks the base's storage classification — any coll, growable
    /// or fixed. 2026-08-18 (Phase D, PiggyBank): a POOLED member-field read of
    /// a collection carries the COLUMN type `Vector(inner, [Anonymous(1)])` —
    /// peel the wrapper before the base check so `let all: List<K> = items`
    /// binds the returned handle directly instead of re-seeding a `[<list>]`.
    fn is_coll_type(&self, ty: &crate::ast::Type) -> bool {
        let base = match ty {
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n,
            crate::ast::Type::Vector(inner, _) => match inner.as_ref() {
                crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n,
                _ => return false,
            },
            _ => return false,
        };
        self.ctx.coll_storage.contains_key(base)
    }

    /// 2026-08-15 (coll plan §3.4.6): the fixed length N of a `coll struct`
    /// (fixed T[N]) — from its one sequence member's array dimension.
    /// Returns 0 if not determinable.
    ///
    /// 2026-08-16 (Phase 3b): a GENERIC `coll struct Fixed<T, N>` resolves its
    /// dimension through the MONO key (`Fixed<Int, 4>`), not the generic base
    /// — the base's `data: T[N]` still holds `Named("N", 0)` (unresolved).
    /// ensure_mono inserts the substituted key whose slot is `Int[4]`; reading
    /// the base entry alone returns 0 for every generic coll.
    fn coll_fixed_length(&self, ty: &crate::ast::Type) -> i64 {
        let key = match ty {
            crate::ast::Type::Applied(n, args) if self.ctx.obj_type_params.contains_key(n.as_str()) => {
                format!(
                    "{}<{}>",
                    n,
                    args.iter().map(|t| format!("{}", t)).collect::<Vec<_>>().join(", ")
                )
            }
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.clone(),
            _ => return 0,
        };
        let fields = self.ctx.struct_types.get(&key).cloned().unwrap_or_default();
        for (_, fty) in &fields {
            if let crate::ast::Type::Vector(_, dims) = fty {
                if let Some(crate::ast::Dimension::Anonymous(n)) = dims.first() {
                    return *n as i64;
                }
            }
        }
        0
    }

    fn register_pool_columns(&mut self, base: &str, slots: &[(String, Type)]) {
        let counter_name = format!("__spawn_next_{}", base);
        if !self.ctx.field_index_map.contains_key(&counter_name) {
            self.ctx.field_index_map
                .insert(counter_name.clone(), self.ctx.field_types.len());
            self.push_field_type(&Type::int());
            self.ctx.field_initializers.insert(counter_name.clone(), Some(Expr::Decimal(1)));
            self.ctx.instance_slots.insert(counter_name);
        }
        let capacity = self.ctx.spawn_pools.get(base)
            .map(|n| n + 1)
            .unwrap_or(1);
        let is_dependent = self.ctx.dependent_pools.contains_key(base);
        for (mname, mty) in slots {
            let slot_name = format!("{}.{}", base, mname);
            if self.ctx.field_index_map.contains_key(&slot_name) {
                continue;
            }
            if is_dependent {
                // 2026-08-07 (object instance pools): a DEPENDENT column is a
                // heap buffer — the slot stores the malloc'd buffer address (an
                // i64), sized at init to the runtime-bound capacity (SPEC
                // §16.6). Member access loads the pointer and GEPs the row
                // inside the buffer (see emit_instance_column_row).
                let slot_idx = self.ctx.field_types.len();
                self.ctx.field_index_map.insert(slot_name.clone(), slot_idx);
                self.push_field_type(&Type::int());
                self.ctx.field_briev_types[slot_idx] = mty.clone();
                self.ctx.field_initializers.insert(slot_name.clone(), None);
                self.ctx.instance_slots.insert(slot_name);
                let elem_ty = if matches!(mty, Type::Vector(_, _)) {
                    self.vector_array_llvm_type(mty)
                        .unwrap_or_else(|| "i64".to_string())
                } else {
                    self.llvm_type(mty)
                };
                self.ctx.heap_columns.insert(slot_idx, elem_ty);
                continue;
            }
            let column = match mty {
                Type::Vector(inner, dims) => Type::Vector(
                    inner.clone(),
                    std::iter::once(crate::ast::Dimension::Anonymous(capacity))
                        .chain(dims.iter().cloned())
                        .collect(),
                ),
                _ => Type::Vector(
                    Box::new(mty.clone()),
                    vec![crate::ast::Dimension::Anonymous(capacity)],
                ),
            };
            self.ctx.field_index_map
                .insert(slot_name.clone(), self.ctx.field_types.len());
            self.push_field_type(&column);
            self.ctx.field_initializers.insert(slot_name.clone(), None);
            self.ctx.instance_slots.insert(slot_name);
        }
    }

    /// Scan the program for cell-to-cell wires from TrgBinding statements.
    fn scan_cell_wires(&mut self, items: &[TopLevel]) {
        for item in items {
            self.scan_item_for_wires(item);
        }
    }

    fn scan_top_level_body(&mut self, body: &[Statement]) {
        for stmt in body {
            self.scan_trg_stmt(stmt);
        }
    }

    fn scan_trg_stmt(&mut self, stmt: &Statement) {
        if let Statement::TrgBinding { instance, .. } = stmt {
            if let Expr::Call(callee, args, _) = instance {
                if !self.ctx.cell_defs.contains_key(callee) { return; }
                let cell = &self.ctx.cell_defs[callee];
                for (i, arg) in args.iter().enumerate() {
                    if let Expr::Field(inner, port_name) = arg {
                        if let Expr::Identifier(src_cell) = inner.as_ref() {
                            if self.ctx.cell_defs.contains_key(src_cell) {
                                if let Some(param_name) = cell.parameters.get(i) {
                                    self.ctx.cell_wires.push((
                                        src_cell.clone(),
                                        port_name.clone(),
                                        callee.clone(),
                                        param_name.0.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn scan_item_for_wires(&mut self, item: &TopLevel) {
        match item {
            TopLevel::Statement(stmt) => self.scan_trg_stmt(stmt.as_ref()),
            TopLevel::Transaction(t) => self.scan_top_level_body(&t.body),
            TopLevel::Definition(d) => self.scan_top_level_body(&d.body),
            TopLevel::Cell(c) => {
                for txn in &c.transactions {
                    self.scan_top_level_body(&txn.body);
                }
            }
            TopLevel::SyncGroup { item: inner, .. } => {
                self.scan_item_for_wires(inner);
            }
            // 2026-07-23: Export wraps an inner item (defn, txn, etc.).
            TopLevel::Export(e) => self.scan_item_for_wires(&e.inner),
            _ => {}
        }
    }
}

/// 2026-08-16 (multi-node internal fold, Direction 3): the resolved loop inputs
/// for folding a counted-loop node's whole bounded pass into `@txn_<name>`.
struct InternalFoldInfo {
    counter_idx: usize,
    total_idx: Option<usize>,
    total_const_name: Option<String>,
    bound_literal: Option<i64>,
    counter_var: String,
}

/// 2026-08-16 (multi-node internal fold): the state-field roots a body WRITES,
/// INCLUDING array-element writes (`px[i] = ...` → `px`). The transition
/// graph's write_set deliberately excludes Index roots (pointer writes change
/// memory AT the pointer — 2026-07-21); the fold gate needs the full picture
/// to prove no other node's pre is clobbered mid-pass.
fn collect_written_fields(body: &[Statement]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    collect_written_fields_inner(body, &mut out);
    out
}

fn collect_written_fields_inner(body: &[Statement], out: &mut std::collections::HashSet<String>) {
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, _) => insert_write_root(lhs, out),
            Statement::ArrowAssign { target, value, .. } => {
                if let Some(t) = target.as_deref() {
                    insert_write_root(t, out);
                }
                insert_write_root(value, out);
            }
            Statement::Expression(Expr::MethodCall(recv, name, _, _)) => {
                if name == "push" || name == "pop" {
                    insert_write_root(recv, out);
                }
            }
            Statement::Guarded(_, body) | Statement::Block(body)
            | Statement::Defer(body) | Statement::Mutex(body) | Statement::SyncBlock(body) => {
                collect_written_fields_inner(body, out);
            }
            Statement::Barrier { body, .. } => collect_written_fields_inner(body, out),
            Statement::Foreach { body, .. } => collect_written_fields_inner(body, out),
            _ => {}
        }
    }
}

/// Insert the root variable of a write target (an expression that is assigned
/// or arrow-mutated) into the written-fields set.
fn insert_write_root(e: &Expr, out: &mut std::collections::HashSet<String>) {
    if let Some(root) = write_root(e) {
        out.insert(root);
    }
}

/// Root variable of an assignment target, descending through Index and Field.
fn write_root(e: &Expr) -> Option<String> {
    match e {
        Expr::Identifier(n) => Some(n.clone()),
        Expr::Index(base, _) => write_root(base),
        Expr::Field(base, _) => write_root(base),
        Expr::AddrOf(inner) => write_root(inner),
        _ => None,
    }
}

/// 2026-08-16 (multi-node internal fold): true when the precondition is a
/// top-level conjunction containing the program-entry flag `beginprogram` —
/// the flag is cleared after the entry pass, so the conjunct is false during
/// any later pass and interior counter references in the pre are moot.
fn contains_beginprogram_conjunct(e: &Expr) -> bool {
    match e {
        Expr::BeginProgram => true,
        Expr::BinaryOp(crate::ast::BinaryOpKind::And, l, r) => {
            contains_beginprogram_conjunct(l) || contains_beginprogram_conjunct(r)
        }
        _ => false,
    }
}

/// 2026-08-16 (multi-node internal fold): true when `e` references `counter`
/// in a comparison that is NOT at the pass boundary — `i < bound`, `i <= k`,
/// `i > bound`, `i != bound`, a bare `i` use, or a boundary comparison against
/// a value that isn't THIS node's bound. A boundary `i == bound` / `i >= bound`
/// (either operand order) against the node's own bound is safe.
fn expr_has_unsafe_counter_ref(e: &Expr, counter: &str, bound_var: &str, bound_lit: Option<i64>) -> bool {
    match e {
        Expr::Identifier(n) => n == counter,
        Expr::BinaryOp(kind, l, r) => {
            let l_is = matches!(l.as_ref(), Expr::Identifier(n) if n == counter);
            let r_is = matches!(r.as_ref(), Expr::Identifier(n) if n == counter);
            if l_is || r_is {
                return counter_compare_is_unsafe(*kind, l_is, l, r, bound_var, bound_lit);
            }
            expr_has_unsafe_counter_ref(l, counter, bound_var, bound_lit)
                || expr_has_unsafe_counter_ref(r, counter, bound_var, bound_lit)
        }
        _ => {
            // Recurse into single-child containers (UnaryOp, Index, etc.).
            let mut unsafe_ref = false;
            walk_expr_children(e, &mut |child| {
                if expr_has_unsafe_counter_ref(child, counter, bound_var, bound_lit) {
                    unsafe_ref = true;
                }
            });
            unsafe_ref
        }
    }
}

/// A binary comparison where exactly one operand is the counter: safe only as
/// a boundary `==`/`>=` (counter on the left) or `==`/`<=` (counter on the
/// right, i.e. `bound <= i`) against THIS node's bound value.
fn counter_compare_is_unsafe(
    kind: crate::ast::BinaryOpKind,
    counter_on_left: bool,
    l: &Expr,
    r: &Expr,
    bound_var: &str,
    bound_lit: Option<i64>,
) -> bool {
    let other = if counter_on_left { r } else { l };
    let boundary_kind = if counter_on_left {
        matches!(kind, crate::ast::BinaryOpKind::Eq | crate::ast::BinaryOpKind::Ge)
    } else {
        matches!(kind, crate::ast::BinaryOpKind::Eq | crate::ast::BinaryOpKind::Le)
    };
    if !boundary_kind {
        return true;
    }
    let matches_bound = matches!(other, Expr::Identifier(n) if n == bound_var)
        || matches!(other, Expr::Decimal(n) if Some(*n) == bound_lit);
    !matches_bound
}

/// Visit the immediate children of an expression that could contain the counter.
fn walk_expr_children(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::UnaryOp(_, inner) => f(inner),
        Expr::Index(base, idx) => {
            f(base);
            f(idx);
        }
        Expr::Field(base, _) => f(base),
        Expr::MethodCall(recv, _, args, _) => {
            f(recv);
            for a in args {
                f(a);
            }
        }
        Expr::Call(_, args, _) => {
            for a in args {
                f(a);
            }
        }
        Expr::Reflect(base, _, _) => f(base),
        Expr::List(elems) => {
            for el in elems {
                f(el);
            }
        }
        Expr::Tuple(elems) => {
            for el in elems {
                f(el);
            }
        }
        Expr::Slice { array, start, end, .. } => {
            f(array);
            if let Some(s) = start {
                f(s);
            }
            if let Some(en) = end {
                f(en);
            }
        }
        Expr::Range { start, end, .. } => {
            f(start);
            f(end);
        }
        Expr::If(c, t, els) => {
            f(c);
            f(t);
            if let Some(ee) = els {
                f(ee);
            }
        }
        Expr::Match(scrutinee, arms) => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    f(g);
                }
            }
        }
        _ => {}
    }
}

