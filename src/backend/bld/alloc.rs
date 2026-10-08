// ── .bld register allocation (M4) — virtuals → physical registers ────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan M4): resolve the M3-emitted virtual value
// registers (`vN`, `fvN`) into physical registers before the .bad backend
// sees the program. The .bad backend refuses unknown names on value
// operands, so this pass is what makes non-trivial recipes reach `.s`.
//
// Shape of the pass, per recipe label:
//
// 1. EVENTS — one walk records per-virtual defs/uses, loop spans
//    (`.head:` … `jmp .head`), call positions, and whether the recipe
//    touches `sp`. Only the M3 lowering's own mnemonics carry virtuals;
//    an unknown mnemonic (a `bad { }` block) naming a `vN` is loud —
//    raw assembly works in physical registers.
// 2. INTERVALS — [first def, last def/use], extended to every loop's
//    backedge when the value is defined before the loop and used inside
//    it (loop-carried values must survive iterations; the extension is
//    the conservative fix that keeps linear-order intervals SAFE under
//    backedges). The extension only ever widens intervals, so overlap
//    checks stay conservative (safe), never optimistic (wrong).
// 3. CALL WALLS — a value whose interval contains a `call` cannot sit
//    in a caller-saved register: it takes a callee-saved register or a
//    frame slot. Call-argument staging copies run BEFORE the call, so
//    argument values' intervals end before the wall (no forced spill).
// 4. POOLS — allocatable registers come from the registry (r0-r15/f0-f15
//    that resolve on the family) minus the ABI argument registers and
//    the return registers (r0/f0). Two scratch registers per class are
//    reserved up front for spill reload/stores. Callee-saved registers
//    are push/pop-saved in the prologue and restored in the epilogue —
//    C-ABI callers own those registers.
// 5. LINEAR SCAN — the pure core (`scan`): values sorted by interval
//    start, caller-saved slots preferred for local values, callee-saved
//    slots reserved for call-crossing values, exhaustion → spill (frame
//    slot). No two allocated intervals share a slot: the scan holds the
//    invariant constructively (expiry at `end < start`, one free-list,
//    no slot reuse while active), and the test suite pins the emitted
//    shape. A Kani proof harness was drafted here and dropped on
//    2026-10-08 by decision — Kani is retired on this lane (the repo's
//    kani gate currently verifies nothing; see the plan doc).
// 6. REWRITE — register homes replace the virtual names; spilled values
//    load into a scratch before each use and store after each def (a
//    spilled `mov dst, src_reg` folds to a direct `storeoff`). A frame
//    exists iff something spilled or a callee-saved register was used;
//    a recipe that touches `sp` cannot take a frame, so spill needs
//    there are loud, naming the value.
//
// To undo: delete this file, revert the `allocate` call in
// src/backend/bld/mod.rs, and revert the param stash in
// src/backend/bld/lower.rs (bind_params / stmts_contain_call).

use crate::ast::bad::{
    BadBodyItem, BadBranch, BadInstr, BadLabel, BadOperand, BadProgram, BadSite, BadTopLevel,
};
use crate::backend::bad::registry::{BadRegisters, RegProp};
use super::lower::BldLowerer;
use std::collections::HashMap;

const SPILL_SCRATCHES: usize = 2;

/// Where a virtual lives after allocation.
#[derive(Debug, Clone, PartialEq)]
enum Home {
    Reg(String),
    Slot(usize),
}

/// Per-virtual event record from the analysis walk.
#[derive(Default)]
struct VEvents {
    defs: Vec<u32>,
    uses: Vec<u32>,
    is_float: bool,
}

/// One value handed to the pure allocation core.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanValue {
    pub start: u32,
    pub end: u32,
    pub crossing: bool,
}

/// Linear scan over half-open-at-the-boundary intervals: a value stays
/// allocated THROUGH its end position (a use and a def on the same
/// instruction need distinct registers, so expiry is `end < start`).
/// Crossing values may only take callee slots (`>= caller`); local
/// values prefer caller slots and fall back to callee slots. A value
/// with no admissible free slot returns None (spill). The output vector
/// is in the INPUT order.
pub(crate) fn scan(values: &[ScanValue], caller: usize, callee: usize) -> Vec<Option<usize>> {
    let total = caller + callee;
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by_key(|&i| (values[i].start, i));
    let mut free: Vec<usize> = (0..total).collect();
    let mut active: Vec<(u32, usize)> = Vec::new();
    let mut out = vec![None; values.len()];
    for &i in &order {
        let v = &values[i];
        expire(&mut active, &mut free, v.start);
        let pick = pick_slot(&free, v.crossing, caller);
        if let Some(p) = pick {
            let slot = free.remove(p);
            active.push((v.end, slot));
            out[i] = Some(slot);
        }
    }
    out
}

/// Free every slot whose value died strictly before `start` (a use and
/// a def on the same instruction need distinct registers).
fn expire(active: &mut Vec<(u32, usize)>, free: &mut Vec<usize>, start: u32) {
    let mut k = 0;
    while k < active.len() {
        if active[k].0 < start {
            free.push(active[k].1);
            active.swap_remove(k);
        } else {
            k += 1;
        }
    }
}

/// Crossing values may only take callee slots (`>= caller`); local
/// values prefer caller slots and fall back to any free slot.
fn pick_slot(free: &[usize], crossing: bool, caller: usize) -> Option<usize> {
    if crossing {
        return free.iter().position(|s| *s >= caller);
    }
    free.iter()
        .position(|s| *s < caller)
        .or_else(|| free.first().map(|_| 0))
}

/// The whole-program pass: allocate every label independently.
pub fn allocate(
    prog: BadProgram,
    regs: &BadRegisters,
    family: &str,
) -> Result<BadProgram, String> {
    let mut items = Vec::with_capacity(prog.items.len());
    for item in prog.items {
        items.push(match item {
            BadTopLevel::Label(l) => BadTopLevel::Label(allocate_label(l, regs, family)?),
            other => other,
        });
    }
    Ok(BadProgram { items, span: prog.span })
}

fn allocate_label(l: BadLabel, regs: &BadRegisters, family: &str) -> Result<BadLabel, String> {
    let analysis = analyze(&l.body)?;
    if analysis.order.is_empty() {
        return Ok(l); // no virtuals — physical recipe, untouched
    }
    let plan = Plan::build(&analysis, regs, family, &l.name)?;
    let body = rewrite(&l.body, &plan, l.span)?;
    Ok(BadLabel { body, ..l })
}

// ── phase 1: events ───────────────────────────────────────────────────

struct Analysis {
    /// First-appearance order of the virtuals (deterministic pools).
    order: Vec<String>,
    events: HashMap<String, VEvents>,
    /// (head position, backedge position) per loop.
    spans: Vec<(u32, u32)>,
    /// Positions of `call` instructions.
    calls: Vec<u32>,
    sp_touched: bool,
}

fn analyze(items: &[BadBodyItem]) -> Result<Analysis, String> {
    let mut a = Analysis {
        order: Vec::new(),
        events: HashMap::new(),
        spans: Vec::new(),
        calls: Vec::new(),
        sp_touched: false,
    };
    let mut heads: Vec<(String, u32)> = Vec::new();
    let mut jumps: Vec<(String, u32)> = Vec::new();
    for (pos, item) in items.iter().enumerate() {
        let pos = pos as u32;
        match item {
            BadBodyItem::Local(l) => heads.push((l.name.clone(), pos)),
            BadBodyItem::Instr(ins) => {
                a.walk_instr(ins, pos)?;
                if let Some(target) = jmp_target(ins) {
                    jumps.push((target.to_string(), pos));
                }
                if ins.mnemonic == "call" {
                    a.calls.push(pos);
                }
                if ins.operands.iter().any(|o| matches!(o, BadOperand::Name(n) if n == "sp")) {
                    a.sp_touched = true;
                }
            }
            BadBodyItem::Site(site) => a.walk_site(site)?,
        }
    }
    // A loop's span ends at its LAST jump back to the head (the
    // structural backedge; `continue` jumps land before it).
    for (name, head) in &heads {
        let dotted = format!(".{name}");
        let back = jumps
            .iter()
            .filter(|j| j.0 == dotted && j.1 > *head)
            .map(|j| j.1)
            .max();
        if let Some(back) = back {
            a.spans.push((*head, back));
        }
    }
    a.spans.sort(); // deterministic extension order
    Ok(a)
}

impl Analysis {
    fn walk_instr(&mut self, ins: &BadInstr, pos: u32) -> Result<(), String> {
        match roles(&ins.mnemonic) {
            Roles::Unknown => {
                if let Some(name) = ins
                    .operands
                    .iter()
                    .find_map(|o| match o {
                        BadOperand::Name(n)
                            if BldLowerer::is_virtual_name(n) =>
                        {
                            Some(n.clone())
                        }
                        _ => None,
                    })
                {
                    return Err(format!(
                        "instruction `{mn}` (line {line}) names `{name}`, a compiler-managed \
                         value register - `bad {{ }}` blocks work in physical registers \
                         (r0-r15, f0-f15); keep `vN` values in the recipe's own statements",
                        mn = ins.mnemonic,
                        line = ins.span.line,
                        name = name,
                    ));
                }
            }
            Roles::Def1Use1 => {
                self.record_def(&ins.operands[0], pos)?;
                self.record_use(&ins.operands[1], pos);
            }
            Roles::Def1Use2 => {
                self.record_def(&ins.operands[0], pos)?;
                self.record_use(&ins.operands[1], pos);
                self.record_use(&ins.operands[2], pos);
            }
            Roles::Use2 => {
                self.record_use(&ins.operands[0], pos);
                self.record_use(&ins.operands[1], pos);
            }
            Roles::None => {}
        }
        Ok(())
    }

    fn walk_site(&mut self, site: &BadSite) -> Result<(), String> {
        for row in &site.rows {
            self.walk_branch(row)?;
        }
        Ok(())
    }

    fn walk_branch(&mut self, branch: &BadBranch) -> Result<(), String> {
        for ins in &branch.body {
            self.walk_instr(ins, 0)?;
        }
        Ok(())
    }

    fn touch(&mut self, name: &str, is_float: bool) -> &mut VEvents {
        if !self.events.contains_key(name) {
            self.order.push(name.to_string());
        }
        let e = self.events.entry(name.to_string()).or_default();
        e.is_float = is_float;
        e
    }

    fn record_def(&mut self, op: &BadOperand, pos: u32) -> Result<(), String> {
        match op {
            BadOperand::Name(n) if Self::is_virtual(n) => {
                let is_float = n.starts_with("fv");
                self.touch(n, is_float).defs.push(pos);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn record_use(&mut self, op: &BadOperand, pos: u32) {
        if let BadOperand::Name(n) = op {
            if Self::is_virtual(n) {
                let is_float = n.starts_with("fv");
                self.touch(n, is_float).uses.push(pos);
            }
        }
    }

    fn is_virtual(name: &str) -> bool {
        BldLowerer::is_virtual_name(name)
    }
}

fn jmp_target(ins: &BadInstr) -> Option<&str> {
    if ins.mnemonic != "jmp" {
        return None;
    }
    match ins.operands.first() {
        Some(BadOperand::Name(t)) => Some(t.trim_start_matches('.')),
        _ => None,
    }
}

/// The operand-shape roles of the mnemonics the .bld lowering emits.
/// Anything else is raw assembly (a `bad { }` block) — physical only.
enum Roles {
    /// `op dst, a` — mov/fmov/neg/not/fneg/itof/ftoi.
    Def1Use1,
    /// `op dst, a, b` — the three-operand arithmetic.
    Def1Use2,
    /// `op a, b, .label` — the fused compare-branches.
    Use2,
    /// `jmp`/`call`/`ret` — no virtual operands.
    None,
    /// Raw assembly: virtuals are forbidden, everything else untouched.
    Unknown,
}

fn roles(mn: &str) -> Roles {
    match mn {
        "mov" | "fmov" | "neg" | "not" | "fneg" | "itof" | "ftoi" => Roles::Def1Use1,
        "add" | "sub" | "mul" | "div" | "mod" | "and" | "or" | "xor" | "shl" | "shr" | "slt"
        | "fadd" | "fsub" | "fmul" | "fdiv" => Roles::Def1Use2,
        "jz" | "jnz" | "jlt" | "jle" | "jgt" | "jge" | "fjz" | "fjnz" | "fjlt" | "fjle"
        | "fjgt" | "fjge" | "store" => Roles::Use2,
        "load" => Roles::Def1Use1,
        "jmp" | "call" | "ret" => Roles::None,
        _ => Roles::Unknown,
    }
}

// ── phase 2-4: intervals, walls, pools, scan ──────────────────────────

struct Plan {
    homes: HashMap<String, Home>,
    slots: usize,
    callee_used: Vec<String>,
    scratches_int: [String; SPILL_SCRATCHES],
    scratches_float: [String; SPILL_SCRATCHES],
    frame_needed: bool,
    frame_size: i64,
}

impl Plan {
    fn build(
        a: &Analysis,
        regs: &BadRegisters,
        family: &str,
        label: &str,
    ) -> Result<Plan, String> {
        let pools = Pools::load(regs, family);
        let intervals = build_intervals(a)?;
        let homes = assign_homes(&intervals, &pools)?;
        // Deterministic lists: intervals carry first-appearance order —
        // never iterate the homes map for output (HashMap order varies).
        let spilled = spilled_names(&intervals, &homes);
        let callee_used = callee_names(&intervals, &homes, &pools);
        let slots = spilled.len();
        let frame_needed = slots > 0 || !callee_used.is_empty();
        // Callee-saved SAVES also touch sp (push/pop), so ANY frame
        // collides with a recipe that manages sp itself.
        if frame_needed && a.sp_touched {
            return Err(format!(
                "recipe `{label}` touches `sp`, so the compiler cannot open a frame - the \
                 live values ({}) need spill slots or callee-saved saves; keep `sp` work out \
                 of the recipe, or reduce the values live at once",
                spilled.join(", ")
            ));
        }
        guard_scratches(&intervals, &homes, &pools, label, family)?;
        let frame_size = match frame_needed {
            false => 0,
            true => {
                let w = regs.push_width(family);
                let raw = slots as i64 * 8 + w * callee_used.len() as i64;
                align16(raw) - w * callee_used.len() as i64
            }
        };
        Ok(Plan {
            homes,
            slots,
            callee_used,
            scratches_int: pools.scratch_int,
            scratches_float: pools.scratch_float,
            frame_needed,
            frame_size,
        })
    }
}

/// A spill with no scratch register cannot reload (the class's pool
/// exhausted before the scratch reserve).
fn guard_scratches(
    intervals: &[(String, ScanValue, bool)],
    homes: &HashMap<String, Home>,
    pools: &Pools,
    label: &str,
    family: &str,
) -> Result<(), String> {
    let class_needs_scratch = |want: bool| {
        intervals
            .iter()
            .any(|(n, _, f)| *f == want && matches!(homes.get(n), Some(Home::Slot(_))))
    };
    let int_ok = pools.scratch_int.iter().all(|s| !s.is_empty());
    let float_ok = pools.scratch_float.iter().all(|s| !s.is_empty());
    if (class_needs_scratch(false) && !int_ok) || (class_needs_scratch(true) && !float_ok) {
        return Err(format!(
            "recipe `{label}` needs to spill, but the `{family}` target leaves no scratch \
             register for the reload - reduce the values live at once, or write the hot \
             sequence in a `bad {{ }}` block"
        ));
    }
    Ok(())
}

/// Spilled value names in first-appearance order.
fn spilled_names(
    intervals: &[(String, ScanValue, bool)],
    homes: &HashMap<String, Home>,
) -> Vec<String> {
    intervals
        .iter()
        .filter(|(n, _, _)| matches!(homes.get(n), Some(Home::Slot(_))))
        .map(|(n, _, _)| n.clone())
        .collect()
}

/// Callee-saved registers in use, in first-appearance order, deduped.
fn callee_names(
    intervals: &[(String, ScanValue, bool)],
    homes: &HashMap<String, Home>,
    pools: &Pools,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (n, _, _) in intervals {
        if let Some(Home::Reg(r)) = homes.get(n) {
            let in_callee = pools.callee_int.contains(r) || pools.callee_float.contains(r);
            if in_callee && !out.contains(r) {
                out.push(r.clone());
            }
        }
    }
    out
}

fn align16(n: i64) -> i64 {
    (n + 15) / 16 * 16
}

/// Allocatable register pools for one family, scratches reserved first.
struct Pools {
    caller_int: Vec<String>,
    callee_int: Vec<String>,
    caller_float: Vec<String>,
    callee_float: Vec<String>,
    scratch_int: [String; SPILL_SCRATCHES],
    scratch_float: [String; SPILL_SCRATCHES],
}

impl Pools {
    fn load(regs: &BadRegisters, family: &str) -> Pools {
        let abi_gp = regs.abi_args(family);
        let abi_fp = regs.abi_args_fp(family);
        let mut int_pool: Vec<String> = (0..=15)
            .map(|i| format!("r{i}"))
            .filter(|r| {
                regs.resolve(r, family).is_some()
                    && !abi_gp.contains(r)
                    && r != "r0"
            })
            .collect();
        let mut float_pool: Vec<String> = (0..=15)
            .map(|i| format!("f{i}"))
            .filter(|r| {
                regs.resolve(r, family).is_some()
                    && !abi_fp.contains(r)
                    && r != "f0"
            })
            .collect();
        let scratch_int = take_scratches(&mut int_pool, regs, family);
        let scratch_float = take_scratches(&mut float_pool, regs, family);
        let split = |pool: Vec<String>| -> (Vec<String>, Vec<String>) {
            let mut caller = Vec::new();
            let mut callee = Vec::new();
            for r in pool {
                if matches!(regs.property(&r, family), Some(RegProp::Callee)) {
                    callee.push(r);
                } else {
                    caller.push(r);
                }
            }
            (caller, callee)
        };
        let (caller_int, callee_int) = split(int_pool);
        let (caller_float, callee_float) = split(float_pool);
        Pools {
            caller_int,
            callee_int,
            caller_float,
            callee_float,
            scratch_int,
            scratch_float,
        }
    }

    fn pool_names(&self, is_float: bool) -> (&[String], &[String]) {
        if is_float {
            (&self.caller_float, &self.callee_float)
        } else {
            (&self.caller_int, &self.callee_int)
        }
    }
}

/// Reserve up to two scratch registers per class, taken from the
/// callee-saved tail first. Scratches are never live across anything —
/// a spill reload feeds the very next instruction — so their
/// caller/callee property is irrelevant; taking them from the callee
/// tail keeps the caller-saved registers available for short-lived
/// temps (the common recipe allocates with no frame at all).
fn take_scratches(
    pool: &mut Vec<String>,
    regs: &BadRegisters,
    family: &str,
) -> [String; SPILL_SCRATCHES] {
    let mut picked: Vec<String> = Vec::new();
    pick_pass(pool, regs, family, &mut picked, true);
    pick_pass(pool, regs, family, &mut picked, false);
    let mut out: [String; SPILL_SCRATCHES] = [String::new(), String::new()];
    for (i, p) in picked.into_iter().enumerate() {
        out[i] = p;
    }
    out
}

/// One scratch pass: walk the pool from the tail, removing registers
/// whose callee-ness matches `want` until the reserve is full.
fn pick_pass(
    pool: &mut Vec<String>,
    regs: &BadRegisters,
    family: &str,
    picked: &mut Vec<String>,
    want_callee: bool,
) {
    let mut i = pool.len();
    while i > 0 && picked.len() < SPILL_SCRATCHES {
        i -= 1;
        let is_callee = matches!(regs.property(&pool[i], family), Some(RegProp::Callee));
        if is_callee == want_callee {
            picked.push(pool.remove(i));
        }
    }
}

fn build_intervals(a: &Analysis) -> Result<Vec<(String, ScanValue, bool)>, String> {
    let mut out = Vec::with_capacity(a.order.len());
    for name in &a.order {
        let e = a.events.get(name).ok_or_else(|| {
            format!("value `{name}` was walked but never recorded - report this")
        })?;
        let Some(start) = interval_start(name, e)? else {
            continue;
        };
        let end = e
            .defs
            .iter()
            .chain(e.uses.iter())
            .copied()
            .max()
            .unwrap_or(start);
        let end = extend_over_loops(start, end, &e.uses, &a.spans);
        let crossing = a.calls.iter().any(|c| start <= *c && *c <= end);
        out.push((name.clone(), ScanValue { start, end, crossing }, e.is_float));
    }
    Ok(out)
}

/// First position the value exists at; None for the impossible empty
/// event set.
fn interval_start(name: &str, e: &VEvents) -> Result<Option<u32>, String> {
    let def0 = e.defs.iter().min().copied();
    let use0 = e.uses.iter().min().copied();
    match (def0, use0) {
        (Some(d), Some(u)) => Ok(Some(d.min(u))),
        (Some(d), None) => Ok(Some(d)),
        (None, Some(_)) => Err(format!(
            "value `{name}` is used but never defined - report this"
        )),
        (None, None) => Ok(None),
    }
}

/// Loop extension: defined before the loop, used inside → the value
/// must survive every iteration (the backedge runs past the use's
/// linear position). The extension only ever widens intervals, so the
/// scan's overlap checks stay conservative (safe), never optimistic.
fn extend_over_loops(start: u32, end: u32, uses: &[u32], spans: &[(u32, u32)]) -> u32 {
    let mut end = end;
    for (head, back) in spans {
        let used_inside = uses.iter().any(|u| *u >= *head && *u <= *back);
        if start < *head && used_inside {
            end = end.max(*back);
        }
    }
    end
}

fn assign_homes(
    intervals: &[(String, ScanValue, bool)],
    pools: &Pools,
) -> Result<HashMap<String, Home>, String> {
    let mut homes = HashMap::new();
    assign_class(intervals, pools, false, &mut homes)?;
    assign_class(intervals, pools, true, &mut homes)?;
    // Number spill slots deterministically in first-appearance order.
    let mut next = 0usize;
    for name in intervals.iter().map(|(n, _, _)| n) {
        if let Some(Home::Slot(usize::MAX)) = homes.get(name) {
            homes.insert(name.clone(), Home::Slot(next));
            next += 1;
        }
    }
    Ok(homes)
}

/// Scan one class's values against that class's pools.
fn assign_class(
    intervals: &[(String, ScanValue, bool)],
    pools: &Pools,
    is_float: bool,
    homes: &mut HashMap<String, Home>,
) -> Result<(), String> {
    let class_vals: Vec<ScanValue> = intervals
        .iter()
        .filter(|(_, _, f)| *f == is_float)
        .map(|(_, v, _)| *v)
        .collect();
    if class_vals.is_empty() {
        return Ok(());
    }
    let (caller, callee) = pools.pool_names(is_float);
    let assigned = scan(&class_vals, caller.len(), callee.len());
    let mut vals = intervals.iter().filter(|(_, _, f)| *f == is_float);
    for slot in assigned {
        let (name, _, _) = vals.next().ok_or_else(|| {
            "scan returned more assignments than values - report this".to_string()
        })?;
        let home = match slot {
            Some(s) if s < caller.len() => Home::Reg(caller[s].clone()),
            Some(s) => Home::Reg(callee[s - caller.len()].clone()),
            None => Home::Slot(usize::MAX), // numbered below
        };
        homes.insert(name.clone(), home);
    }
    Ok(())
}

// ── phase 6: rewrite ──────────────────────────────────────────────────

fn rewrite(
    items: &[BadBodyItem],
    plan: &Plan,
    span: crate::errors::Span,
) -> Result<Vec<BadBodyItem>, String> {
    let mut out: Vec<BadBodyItem> = Vec::new();
    if plan.frame_needed {
        for r in &plan.callee_used {
            out.push(instr("push", vec![BadOperand::Name(r.clone())], span));
        }
        if plan.slots > 0 {
            out.push(instr(
                "sub",
                vec![
                    BadOperand::Name("sp".to_string()),
                    BadOperand::Name("sp".to_string()),
                    BadOperand::Int(plan.frame_size),
                ],
                span,
            ));
        }
    }
    for item in items {
        match item {
            BadBodyItem::Local(_) => out.push(item.clone()),
            BadBodyItem::Instr(ins) => {
                // The frame restores BEFORE the ret — an epilogue after
                // it would be unreachable.
                if ins.mnemonic == "ret" {
                    push_epilogue(plan, &mut out, span);
                }
                rewrite_instr(ins, plan, span, &mut out)?;
            }
            BadBodyItem::Site(_) => out.push(item.clone()),
        }
    }
    // Fall-off-the-end exit: restore the frame (no auto-ret — the naked
    // semantics stand; only the frame the compiler opened is undone).
    let ends_with_exit = items.last().is_some_and(|i| match i {
        BadBodyItem::Instr(ins) => ins.mnemonic == "ret" || ins.mnemonic == "jmp",
        _ => false,
    });
    if plan.frame_needed && !ends_with_exit {
        push_epilogue(plan, &mut out, span);
    }
    Ok(out)
}

fn push_epilogue(plan: &Plan, out: &mut Vec<BadBodyItem>, span: crate::errors::Span) {
    if plan.slots > 0 {
        out.push(instr(
            "add",
            vec![
                BadOperand::Name("sp".to_string()),
                BadOperand::Name("sp".to_string()),
                BadOperand::Int(plan.frame_size),
            ],
            span,
        ));
    }
    for r in plan.callee_used.iter().rev() {
        out.push(instr("pop", vec![BadOperand::Name(r.clone())], span));
    }
}

/// A synthesized frame instruction; `span` is the recipe label's, so a
/// capability error on a frame row points at the recipe.
fn instr(mn: &str, ops: Vec<BadOperand>, span: crate::errors::Span) -> BadBodyItem {
    BadBodyItem::Instr(BadInstr {
        mnemonic: mn.to_string(),
        operands: ops,
        contract: None,
        ack: None,
        span,
    })
}

/// Per-instruction spill state: which spilled slot rides which scratch,
/// and whether the destination spilled.
struct SpillCtx<'a> {
    homes: &'a HashMap<String, Home>,
    scratches: &'a [String; SPILL_SCRATCHES],
    scratch_of_slot: Vec<(usize, usize)>,
    scratch_next: usize,
    dst_spill: Option<usize>,
}

impl SpillCtx<'_> {
    /// The scratch for a spilled slot, emitting its reload on first
    /// touch in this instruction.
    fn scratch_for(
        &mut self,
        slot: usize,
        span: crate::errors::Span,
        out: &mut Vec<BadBodyItem>,
    ) -> usize {
        if let Some((_, sc)) = self.scratch_of_slot.iter().find(|(s, _)| *s == slot) {
            return *sc;
        }
        let sc = self.scratch_next.min(SPILL_SCRATCHES - 1);
        self.scratch_next += 1;
        self.scratch_of_slot.push((slot, sc));
        out.push(instr(
            "loadoff",
            vec![
                BadOperand::Name(self.scratches[sc].clone()),
                BadOperand::Name("sp".to_string()),
                BadOperand::Int(slot as i64 * 8),
            ],
            span,
        ));
        sc
    }

    /// Point one operand at its home: a register directly, a spilled
    /// use through its scratch, a spilled def through scratch 0 (the
    /// store happens after the instruction).
    fn resolve_operand(
        &mut self,
        op: &mut BadOperand,
        is_def: bool,
        span: crate::errors::Span,
        out: &mut Vec<BadBodyItem>,
    ) {
        let BadOperand::Name(n) = op else {
            return;
        };
        let Some(home) = self.homes.get(n).cloned() else {
            return;
        };
        match home {
            Home::Reg(r) => *op = BadOperand::Name(r),
            Home::Slot(slot) => {
                if is_def {
                    self.dst_spill = Some(slot);
                    *op = BadOperand::Name(self.scratches[0].clone());
                } else {
                    let sc = self.scratch_for(slot, span, out);
                    *op = BadOperand::Name(self.scratches[sc].clone());
                }
            }
        }
    }
}

fn rewrite_instr(
    ins: &BadInstr,
    plan: &Plan,
    span: crate::errors::Span,
    out: &mut Vec<BadBodyItem>,
) -> Result<(), String> {
    if matches!(roles(&ins.mnemonic), Roles::Unknown) {
        out.push(BadBodyItem::Instr(ins.clone()));
        return Ok(());
    }
    let is_float_instr = ins.mnemonic.starts_with('f');
    let scratches = if is_float_instr {
        &plan.scratches_float
    } else {
        &plan.scratches_int
    };
    let mut ops = ins.operands.clone();
    // Per-instruction scratch assignment: distinct spilled slots load
    // into distinct scratches; the def reuses scratch 0 (a destination
    // may overlap a source — the instruction reads its sources before
    // writing the destination).
    let def_count = match roles(&ins.mnemonic) {
        Roles::Def1Use1 | Roles::Def1Use2 => 1usize,
        _ => 0,
    };
    let mut ctx = SpillCtx {
        homes: &plan.homes,
        scratches,
        scratch_of_slot: Vec::new(),
        scratch_next: 0,
        dst_spill: None,
    };
    for (idx, op) in ops.iter_mut().enumerate() {
        ctx.resolve_operand(op, idx < def_count, span, out);
    }
    let dst_spill = ctx.dst_spill;
    // A spilled `mov dst, src_reg` folds to a direct store.
    let folded = match (dst_spill, ops.as_slice()) {
        (Some(slot), [src @ BadOperand::Name(_), _])
            if ins.mnemonic == "mov" || ins.mnemonic == "fmov" =>
        {
            out.push(instr(
                "storeoff",
                vec![
                    src.clone(),
                    BadOperand::Name("sp".to_string()),
                    BadOperand::Int(slot as i64 * 8),
                ],
                span,
            ));
            true
        }
        _ => false,
    };
    if !folded {
        out.push(BadBodyItem::Instr(BadInstr {
            mnemonic: ins.mnemonic.clone(),
            operands: ops,
            contract: ins.contract.clone(),
            ack: ins.ack.clone(),
            span: ins.span,
        }));
        if let Some(slot) = dst_spill {
            out.push(instr(
                "storeoff",
                vec![
                    BadOperand::Name(scratches[0].clone()),
                    BadOperand::Name("sp".to_string()),
                    BadOperand::Int(slot as i64 * 8),
                ],
                span,
            ));
        }
    }
    Ok(())
}
