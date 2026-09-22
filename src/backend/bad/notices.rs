// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
// ── .bad W-tier notices — probable-error analysis (never restrictive) ─
//
// The compiler PREDICTS, the author VETOES. Every W<n> below is a
// *probable* error: static-analysis heuristics that cannot prove a bug
// (deliberate stack juggling, a callee that never touches a register, a
// forward reference that resolves). They never block compilation — an
// `^` / `^^` / `^^^` prefix on the offending line acknowledges them.
// Acknowledged notices still print under --trace-lowering as info; the
// author's conscious disagreement is auditable, never silent.
//
// The three-tier boundary (plan 2026-09-22):
//   - Hardware capability  (no imm form, unmapped register)  → hard error
//   - Author-declared contract (preserved/frame violated)     → hard error
//   - Analysis prediction (W1..W6)                            → ack-able
//
// W1 caller-saved live across call · W2 branch-path push/pop imbalance ·
// W3 ret with sp delta · W4 FP-pool scratch collision · W5 defn-inlined
// ret · W6 unresolved local label. (W7, syscall imm arg, was removed:
// the no-imm-form capability guard already hard-errors it — the untouchable
// tier owns that case.)

use crate::ast::bad::*;
use crate::backend::bad::registry::{BadRegisters, RegProp};
use crate::errors::Span;

/// One probable-error notice.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub code: &'static str,
    pub message: String,
    pub line: usize,
    /// True when an `^`/`^^`/`^^^` on the line acknowledged it.
    pub acknowledged: bool,
}

/// The ack scopes a notice may fall under.
fn ack_covers(instr: &BadInstr, code: &str, scope: AckScope) -> bool {
    let Some(ack) = &instr.ack else { return false };
    if !scope_ok(ack.scope, scope) {
        return false;
    }
    // Named warnings restrict the ack; empty = acknowledge all.
    ack.warnings.is_empty() || ack.warnings.iter().any(|w| w == code)
}

fn scope_ok(ack: AckScope, req: AckScope) -> bool {
    match ack {
        AckScope::Instr => req == AckScope::Instr,
        AckScope::Line | AckScope::Override => true,
    }
}

/// A per-instruction scan context for a label body.
struct Scan {
    /// Registers written since the last `call`/`ret` (W1 candidates).
    written_since_call: std::collections::HashSet<String>,
    /// Registers touched since the last FP literal (W4 pool-clobber
    /// collision candidates).
    touched_since_pool: std::collections::HashSet<String>,
    /// Net sp displacement from push/pop/sub/add sp.
    sp_delta: i64,
    /// Whether a `ret` was already seen (stop scanning past it).
    terminated: bool,
}

/// Run the W-tier analysis over a label body. `label` is the enclosing
/// label name (for W6 local-scope messages); `push_width` per target.
pub fn check_label(
    label: &BadLabel,
    regs: &BadRegisters,
    family: &str,
) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut scan = Scan {
        written_since_call: std::collections::HashSet::new(),
        touched_since_pool: std::collections::HashSet::new(),
        sp_delta: 0,
        terminated: false,
    };
    for item in &label.body {
        let BadBodyItem::Instr(instr) = item else { continue };
        if scan.terminated {
            continue;
        }
        check_instr(instr, regs, family, &mut scan, &mut out);
    }
    check_local_refs(label, &mut out);
    out
}

/// W1: caller-saved registers written and live across a `call`.
fn check_call_clobbers(
    instr: &BadInstr,
    regs: &BadRegisters,
    family: &str,
    scan: &Scan,
    out: &mut Vec<Notice>,
) {
    for reg in &scan.written_since_call {
        if regs.property(reg, family) == Some(RegProp::Caller) {
            emit(
                out,
                instr,
                AckScope::Instr,
                "W1",
                format!(
                    "caller-saved `{reg}` is written and live across `call` at line {} - \
                     the callee may clobber it; save it (push/pop) or ack with `^`",
                    instr.span.line
                ),
            );
        }
    }
}

/// W4: the FP literal pool borrows a scratch register per target
/// (aarch64 x9 → r9, riscv64 t0 → r8); a live value there collides.
fn check_pool_scratch(instr: &BadInstr, family: &str, scan: &Scan, out: &mut Vec<Notice>) {
    let Some(s) = pool_scratch(family) else { return };
    if scan.touched_since_pool.contains(s) {
        emit(
            out,
            instr,
            AckScope::Instr,
            "W4",
            format!(
                "FP literal at line {} borrows `{s}` as scratch - a value written \
                 there earlier in this body is still live",
                instr.span.line
            ),
        );
    }
}

/// W6: a `.name` operand referencing a local label this label never
/// declares — the lowerer's local_label_name will not meet it.
fn check_local_refs(label: &BadLabel, out: &mut Vec<Notice>) {
    let local_names: std::collections::HashSet<&str> = label
        .body
        .iter()
        .filter_map(|item| match item {
            BadBodyItem::Local(l) => Some(l.name.as_str()),
            BadBodyItem::Instr(_) => None,
        })
        .collect();
    for item in &label.body {
        let BadBodyItem::Instr(instr) = item else { continue };
        for op in &instr.operands {
            let BadOperand::Name(n) = op else { continue };
            let Some(local) = n.strip_prefix('.') else { continue };
            if local.starts_with('L') {
                continue; // already-gensym'd refs from expansions.
            }
            if !local_names.contains(local) {
                emit(
                    out,
                    instr,
                    AckScope::Instr,
                    "W6",
                    format!(
                        "local label `{n}` at line {} has no `.{local}:` in label `{}` - \
                         the reference will not resolve",
                        instr.span.line, label.name
                    ),
                );
            }
        }
    }
}

fn check_instr(
    instr: &BadInstr,
    regs: &BadRegisters,
    family: &str,
    scan: &mut Scan,
    out: &mut Vec<Notice>,
) {
    let m = instr.mnemonic.as_str();
    match m {
        "call" => {
            // W1: caller-saved registers live across the call.
            check_call_clobbers(instr, regs, family, scan, out);
            scan.written_since_call.clear();
        }
        "ret" => {
            // W3: returning with a nonzero sp delta.
            if scan.sp_delta != 0 {
                emit(
                    out,
                    instr,
                    AckScope::Instr,
                    "W3",
                    format!(
                        "`ret` at line {} with sp off by {} - the stack was not restored to \
                         its entry position",
                        instr.span.line, scan.sp_delta
                    ),
                );
            }
            scan.terminated = true;
        }
        "fmov" => {
            // W4: the FP literal pool borrows a scratch register per
            // target (x9 on aarch64, t0 on riscv64); a live value there
            // collides.
            check_pool_scratch(instr, family, scan, out);
        }
        "push" | "push2" => {
            scan.sp_delta -= regs.push_width(family);
        }
        "pop" | "pop2" => {
            scan.sp_delta += regs.push_width(family);
        }
        "sub" => {
            // sp, sp, imm is the stack-frame idiom (W2/W3 tracking).
            if is_sp_delta_instr(instr, -1) {
                if let Some(v) = imm_of(instr) {
                    scan.sp_delta -= v;
                }
            }
        }
        "add" => {
            if is_sp_delta_instr(instr, 1) {
                if let Some(v) = imm_of(instr) {
                    scan.sp_delta += v;
                }
            }
        }
        _ => {}
    }
    // Track written registers (Name operands that resolve to registers).
    for op in &instr.operands {
        let BadOperand::Name(n) = op else { continue };
        if regs.exists(n, family) {
            scan.written_since_call.insert(n.clone());
            scan.touched_since_pool.insert(n.clone());
        }
    }
}

/// The portable name of the target's FP-literal-pool scratch register
/// (aarch64 x9 → r9, riscv64 t0 → r8), if the target discloses one.
fn pool_scratch(family: &str) -> Option<&'static str> {
    if family.starts_with("aarch64") {
        Some("r9")
    } else if family.starts_with("riscv64") {
        Some("r8")
    } else {
        None
    }
}

/// Whether an instr is `(add|sub) sp, sp, imm` — the stack-frame idiom.
fn is_sp_delta_instr(instr: &BadInstr, sign: i64) -> bool {
    if instr.operands.len() < 3 {
        return false;
    }
    let BadOperand::Name(dst) = &instr.operands[0] else { return false };
    let BadOperand::Name(a) = &instr.operands[1] else { return false };
    if dst != "sp" || a != "sp" {
        return false;
    }
    match &instr.operands[2] {
        BadOperand::Int(_) => true,
        // `sub sp, sp, N` has positive N; `add` has negative N baked in
        // by the author (the dialect has no negated-imm form).
        _ => sign > 0 && instr.mnemonic == "add",
    }
}

fn imm_of(instr: &BadInstr) -> Option<i64> {
    match &instr.operands[2] {
        BadOperand::Int(v) => Some(*v),
        _ => None,
    }
}

/// Push a notice unless acknowledged; always record the acknowledgement.
fn emit(
    out: &mut Vec<Notice>,
    instr: &BadInstr,
    scope: AckScope,
    code: &'static str,
    message: String,
) {
    let acknowledged = ack_covers(instr, code, scope);
    out.push(Notice {
        code,
        message,
        line: instr.span.line,
        acknowledged,
    });
}

/// Run the W-tier analysis over a defn's bodies (W5: inlined `ret`; W2:
/// branch-path push/pop imbalance). Branch defns carry one row per path.
pub fn check_defn(d: &BadDefn) -> Vec<Notice> {
    let mut out = Vec::new();
    match &d.shape {
        BadDefnShape::Sequence(body) => {
            for item in body {
                let BadBodyItem::Instr(instr) = item else { continue };
                if instr.mnemonic == "ret" {
                    out.push(Notice {
                        code: "W5",
                        message: format!(
                            "defn `{}` inlines a `ret` at line {} - the return exits the ENCLOSING \
                             function, not the defn; keep returns at top level or ack with `^`",
                            d.name,
                            instr.span.line
                        ),
                        line: instr.span.line,
                        acknowledged: ack_covers(instr, "W5", AckScope::Instr),
                    });
                }
            }
        }
        BadDefnShape::Branch(rows) => {
            for row in rows {
                // W2: a branch row that pushes but never pops (or vice
                // versa) leaves the stack off by one on that path.
                let mut delta = 0i64;
                for instr in &row.body {
                    match instr.mnemonic.as_str() {
                        "push" | "push2" => delta -= 1,
                        "pop" | "pop2" => delta += 1,
                        _ => {}
                    }
                }
                if delta != 0 {
                    let row_label = if row.is_default { "default" } else { &row.target };
                    out.push(Notice {
                        code: "W2",
                        message: format!(
                            "branch row `{row_label} =>` of defn `{}` leaves the stack off by \
                             {delta} - a push/pop imbalance on this path",
                            d.name
                        ),
                        line: row.span.line,
                        acknowledged: row
                            .body
                            .iter()
                            .any(|i| ack_covers(i, "W2", AckScope::Instr)),
                    });
                }
            }
        }
    }
    out
}

/// Whether a stale ack (a named warning that never fired) is an error.
/// Called from the lowerer after all notices are collected: for every
/// ack that names warnings, each named warning must have fired somewhere.
pub fn stale_acks<'a>(program: &'a BadProgram, notices: &[Notice]) -> Vec<String> {
    let mut acks: Vec<(AckScope, &'a String, Span)> = Vec::new();
    for item in &program.items {
        collect_acks(item, &mut acks);
    }
    let fired: Vec<&str> = notices.iter().map(|n| n.code).collect();
    let mut stale = Vec::new();
    for (_, name, span) in acks {
        if name.starts_with('W') && !fired.contains(&name.as_str()) {
            stale.push(format!(
                "acknowledge `{name}` at line {} names no warning that fired - the marker \
                 is stale; drop it or fix the warning name",
                span.line
            ));
        }
    }
    stale
}

fn collect_acks<'a>(
    item: &'a BadTopLevel,
    out: &mut Vec<(AckScope, &'a String, Span)>,
) {
    let instrs: Vec<&BadInstr> = match item {
        BadTopLevel::Label(l) => l
            .body
            .iter()
            .filter_map(|i| match i {
                BadBodyItem::Instr(x) => Some(x),
                BadBodyItem::Local(_) => None,
            })
            .collect(),
        BadTopLevel::Defn(d) => {
            if let BadDefnShape::Sequence(body) = &d.shape {
                body.iter()
                    .filter_map(|i| match i {
                        BadBodyItem::Instr(x) => Some(x),
                        BadBodyItem::Local(_) => None,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => return,
    };
    for i in instrs {
        if let Some(a) = &i.ack {
            for w in &a.warnings {
                out.push((a.scope, w, a.span));
            }
        }
    }
}

/// Format a notice for display: `W1 at line N: message`.
pub fn format(notice: &Notice) -> String {
    format!("W-tier {} at line {}: {}", notice.code, notice.line, notice.message)
}

/// Format an acknowledged notice as an info line (never silent).
pub fn format_acknowledged(notice: &Notice) -> String {
    format!(
        "info: {} (acknowledged at line {})",
        notice.message, notice.line
    )
}