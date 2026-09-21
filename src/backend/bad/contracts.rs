// ── .bad contract checking — boundary proofs ──────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): validates .bad contracts at compile time.
// The MVP proof set (what the register registry's properties license):
//
// - `[rN preserved]` — PROVEN when rN is callee-saved on the target
//   (config property), or when the label body carries balanced push/pop
//   pairs for rN. Caller-saved without pairing = loud error with the fix.
// - `[rN valid]` — PROVEN when rN maps on the target. (Full pointer
//   validity proofs deferred — documented in bad-dialect.md.)
// - Compare chains (`[sp % 16 == 0]`) — the lhs register's existence is
//   checked now; constant folding of the chain lands with the comptime
//   pass. The predicate is validated, not yet proven.
//
// Every failure states what is wrong, why (the config property that
// blocks it), and the concrete fix — house style.

use super::registry::{BadRegisters, RegProp};
use crate::ast::bad::*;


/// Shared checking context — keeps every check function at ≤5 params.
struct Ctx<'a> {
    regs: &'a BadRegisters,
    family: &'a str,
    /// Label name ("" for inline positions).
    label: &'a str,
}

/// Label-level contracts. Returns proven-proof notes (emitted as comments
/// into the .s so the proof trail is observable in the artifact).
pub fn check_label_contracts(
    label: &BadLabel,
    regs: &BadRegisters,
    family: &str,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut proven = Vec::new();
    let ctx = Ctx { regs, family, label: &label.name };
    let preds = label.contracts.iter().flat_map(|c| &c.preds);
    for pred in preds {
        check_pred(pred, &label.body, &ctx, errors, &mut proven);
    }
    proven
}

/// Inline `[expr]` before an instruction.
pub fn check_inline_contract(
    instr: &BadInstr,
    regs: &BadRegisters,
    family: &str,
    errors: &mut Vec<String>,
) {
    let Some(contract) = &instr.contract else { return };
    let ctx = Ctx { regs, family, label: "" };
    let mut proven = Vec::new();
    for pred in &contract.preds {
        check_pred(pred, &[], &ctx, errors, &mut proven);
    }
}

pub fn check_data_label(_d: &BadDataLabel, _errors: &mut Vec<String>) {
    // Data labels carry no contracts in the MVP grammar; the parser
    // validated the identifier and directive shape.
}

fn check_pred(
    pred: &BadContractPred,
    body: &[BadInstr],
    ctx: &Ctx,
    errors: &mut Vec<String>,
    proven: &mut Vec<String>,
) {
    let (regs, family, label) = (ctx.regs, ctx.family, ctx.label);
    match pred {
        BadContractPred::Preserved(reg) => {
            check_preserved(reg, body, ctx, errors, proven);
        }
        BadContractPred::Valid(reg) => {
            if !regs.exists(reg, family) {
                let targets = regs.known_targets(reg).join(", ");
                errors.push(format!(
                    "contract `[{} valid]` on `{}` fails: register `{}` does not exist on \
                     `{}` (it maps only to [{}]) - use a register this target has",
                    reg, where_str(label), reg, family, targets
                ));
            } else {
                proven.push(format!("[{} valid] proven: register is mapped on {}", reg, family));
            }
        }
        BadContractPred::Compare { lhs, ops } => {
            if !regs.exists(lhs, family) {
                errors.push(format!(
                    "contract on `{}` fails: `{}` is not a register on `{}` - compare \
                     chains range over registers",
                    where_str(label),
                    lhs,
                    family
                ));
            }
            if ops.is_empty() {
                errors.push(format!(
                    "contract on `{}` has an empty comparison chain",
                    where_str(label)
                ));
            }
            // Constant folding of the chain lands with the comptime pass;
            // shape is validated here.
        }
    }
}

fn check_preserved(
    reg: &str,
    body: &[BadInstr],
    ctx: &Ctx,
    errors: &mut Vec<String>,
    proven: &mut Vec<String>,
) {
    let (regs, family, label) = (ctx.regs, ctx.family, ctx.label);
    match regs.property(reg, family) {
        Some(RegProp::Callee) => {
            proven.push(format!(
                "[{} preserved] proven: callee-saved on {}",
                reg, family
            ));
        }
        Some(RegProp::Caller) | Some(RegProp::None) => {
            let (pushes, pops) = count_push_pop(reg, body);
            if pushes > 0 && pushes == pops {
                proven.push(format!(
                    "[{} preserved] proven: {} balanced push/pop pair(s)",
                    reg, pushes
                ));
            } else {
                errors.push(format!(
                    "contract `[{} preserved]` on `{}` is unproven: `{}` is caller-saved \
                     on `{}` and the body has {} push / {} pop for it - wrap the clobbering \
                     code in `push {}` ... `pop {}`, or drop the contract",
                    reg, where_str(label), reg, family, pushes, pops, reg, reg
                ));
            }
        }
        Some(RegProp::ReadOnly) => {
            errors.push(format!(
                "contract `[{} preserved]` on `{}` is meaningless: `{}` is read-only on \
                 `{}` - a value cannot be preserved through code that cannot write it",
                reg, where_str(label), reg, family
            ));
        }
        None => {
            let targets = regs.known_targets(reg).join(", ");
            errors.push(format!(
                "contract `[{} preserved]` on `{}` fails: register `{}` does not exist on \
                 `{}` (it maps only to [{}]) - use a register this target has",
                reg, where_str(label), reg, family, targets
            ));
        }
    }
}

/// Count `push reg` / `pop reg` instruction pairs in a body (raw operands,
/// resolved through the portable name only — exceptions are per-target and
/// counted by their textual `reg` too, since a raw body's registers still
/// map through the same table).
fn count_push_pop(reg: &str, body: &[BadInstr]) -> (usize, usize) {
    let mut pushes = 0;
    let mut pops = 0;
    for instr in body {
        if instr.operands.len() == 1 {
            if let BadOperand::Name(name) = &instr.operands[0] {
                if name == reg {
                    match instr.mnemonic.as_str() {
                        "push" => pushes += 1,
                        "pop" => pops += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    (pushes, pops)
}

fn where_str(label: &str) -> String {
    if label.is_empty() {
        "inline position".to_string()
    } else {
        format!("label `{}`", label)
    }
}

