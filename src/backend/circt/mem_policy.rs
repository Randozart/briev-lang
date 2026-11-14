// ── Array-lowering policy engine (2026-08-25, seq-firmem plan §3) ──────
//
// Pure decision logic for bounded state arrays: register file vs
// seq.firmem memory macro. No emission here — the CIRCT backend collects
// the per-array facts (hint, depth, references, writers, ports, init)
// and calls `decide_array_lowering`.

use crate::ast::{Expr, Statement, TopLevel};

/// The pin from `mem let` / `reg let` annotations.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MemHint {
    #[default]
    None,
    Mem,
    Reg,
}

impl MemHint {
    pub fn from_annotation(name: &str) -> Option<Self> {
        match name {
            "mem" => Some(MemHint::Mem),
            "reg" => Some(MemHint::Reg),
            _ => None,
        }
    }
}

/// The lowering decision for one bounded state array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArrayLowering {
    /// seq.firmem memory macro (+ extern companion at export).
    FirmMem,
    /// Per-lane registers + mux trees (the proven default path).
    RegFile,
}

/// Facts gathered per array by the backend pre-pass.
#[derive(Clone, Debug, Default)]
pub struct ArrayFacts {
    pub depth: usize,
    /// A POSTcondition reads an element of this array (committed-value
    /// obligation — register-only semantics). Pre/guard reads don't count:
    /// they see cycle-start state, which macros serve combinationally.
    pub post_refs: bool,
    /// Number of DISTINCT transactions whose body writes elements.
    pub writers: usize,
    /// Number of distinct READ sites in bodies (rhs/expressions — an
    /// assignment TARGET is served by its writer's write port, not a read
    /// port) plus pre-condition reads. Each site = one macro port.
    pub port_sites: usize,
    /// Any nonzero element in the initializer literal.
    pub nonzero_init: bool,
}

/// Policy knobs (config/ir-lowering.dbvl: circt.firmem_min_depth /
/// circt.firmem_max_ports).
#[derive(Clone, Copy, Debug)]
pub struct MemPolicy {
    pub min_depth: usize,
    pub max_ports: usize,
}

impl Default for MemPolicy {
    fn default() -> Self {
        MemPolicy {
            min_depth: 64,
            max_ports: 4,
        }
    }
}

/// One lowering decision. `why` is Some(reason) ONLY when the DEFAULT
/// policy made the call (drives the aggregated disambiguation note);
/// explicit pins carry no why (they silence the note by definition).
#[derive(Clone, Debug)]
pub struct Decision {
    pub lowering: ArrayLowering,
    pub why: Option<&'static str>,
}

/// Decide the lowering for ONE array. Errors are hard capability errors
/// (pinned choices that cannot be honored); they name the fix.
pub fn decide_array_lowering(
    hint: MemHint,
    facts: &ArrayFacts,
    policy: &MemPolicy,
) -> Result<Decision, String> {
    if matches!(hint, MemHint::Mem) {
        check_mem_pin(facts)?;
    }
    match hint {
        MemHint::Reg => Ok(Decision {
            lowering: ArrayLowering::RegFile,
            why: None,
        }),
        MemHint::Mem => Ok(Decision {
            lowering: ArrayLowering::FirmMem,
            why: None,
        }),
        MemHint::None => Ok(default_decision(facts, policy)),
    }
}

/// Pinned-'mem' gates: the keyword carries intent load, so impossible
/// combinations are hard errors naming the fix.
fn check_mem_pin(facts: &ArrayFacts) -> Result<(), String> {
    if facts.post_refs {
        return Err(
            "a postcondition reads elements of this array and it is pinned \
             'mem let' — memory macros commit at the clock edge, so no \
             combinational would-be value exists for the obligation; use \
             'reg let' for element obligations"
                .to_string(),
        );
    }
    if facts.writers > 1 {
        return Err(format!(
            "{} transactions write this array and it is pinned 'mem let' — \
             multi-writer arbitration is register-file semantics; use \
             'reg let'",
            facts.writers
        ));
    }
    if facts.nonzero_init {
        return Err(
            "this array has a nonzero initializer and is pinned 'mem let' — \
             memory-macro initialization is not supported on this surface; \
             use 'reg let', or zero-init"
                .to_string(),
        );
    }
    Ok(())
}

/// Default policy: macro past the threshold when semantics-compatible;
/// otherwise register file WITH the reason (drives THE note).
fn default_decision(facts: &ArrayFacts, policy: &MemPolicy) -> Decision {
    let reg = |why| Decision {
        lowering: ArrayLowering::RegFile,
        why: Some(why),
    };
    if facts.depth < policy.min_depth {
        return reg("depth < threshold");
    }
    if facts.post_refs {
        return reg("a postcondition reads elements");
    }
    if facts.writers > 1 {
        return reg("multiple writing transactions");
    }
    if facts.port_sites > policy.max_ports {
        return reg("port sites above budget");
    }
    if facts.nonzero_init {
        return reg("nonzero initializer");
    }
    Decision {
        lowering: ArrayLowering::FirmMem,
        why: Some("depth >= threshold"),
    }
}

/// Walk every `Index(Identifier(name), _)` occurrence in an expression,
/// including inside index sub-expressions.
pub fn for_each_index_ref(expr: &Expr, f: &mut impl FnMut(&str)) {
    match expr {
        Expr::Index(obj, idx) => {
            if let Expr::Identifier(name) = obj.as_ref() {
                f(name);
            } else {
                for_each_index_ref(obj, f);
            }
            for_each_index_ref(idx, f);
        }
        Expr::BinaryOp(_, l, r) => {
            for_each_index_ref(l, f);
            for_each_index_ref(r, f);
        }
        Expr::UnaryOp(_, inner) => for_each_index_ref(inner, f),
        Expr::Cast(inner, _) => for_each_index_ref(inner, f),
        Expr::Field(obj, _) => for_each_index_ref(obj, f),
        _ => {}
    }
}

/// Collect array facts across the whole program: which top-level lets are
/// arrays (with hints/inits), who writes them, how many READ sites exist,
/// whether postconditions reference them.
pub fn collect_array_facts(items: &[TopLevel]) -> Vec<(String, MemHint, ArrayFacts)> {
    let mut arrays = collect_array_decls(items);
    if arrays.is_empty() {
        return Vec::new();
    }
    for item in items {
        if let TopLevel::Transaction(txn) = item {
            fold_txn_into_facts(txn, &mut arrays);
        }
    }
    let mut out: Vec<(String, MemHint, ArrayFacts)> = arrays
        .into_iter()
        .map(|(name, (hint, facts))| (name, hint, facts))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic order
    out
}

type ArrayMap = std::collections::HashMap<String, (MemHint, ArrayFacts)>;

fn collect_array_decls(items: &[TopLevel]) -> ArrayMap {
    let mut arrays: ArrayMap = std::collections::HashMap::new();
    for item in items {
        let TopLevel::Statement(stmt) = item else {
            continue;
        };
        let Statement::Let {
            name,
            ty: Some(ty),
            expr,
            modifiers,
            ..
        } = &**stmt
        else {
            continue;
        };
        let crate::ast::Type::Vector(_, dims) = ty else {
            continue;
        };
        let depth = dims
            .first()
            .map(|d| match d {
                crate::ast::Dimension::Anonymous(c) => *c,
                crate::ast::Dimension::Named(_, _) => 0,
            })
            .unwrap_or(0);
        let hint = modifiers
            .iter()
            .find_map(|a| MemHint::from_annotation(&a.name))
            .unwrap_or(MemHint::None);
        let nonzero_init = match expr {
            Some(Expr::List(list)) => list.iter().any(|e| !is_zero_literal(e)),
            _ => true, // unknown init ⇒ conservative
        };
        arrays.insert(
            name.clone(),
            (
                hint,
                ArrayFacts {
                    depth,
                    nonzero_init,
                    ..ArrayFacts::default()
                },
            ),
        );
    }
    arrays
}

fn fold_txn_into_facts(txn: &crate::ast::Transaction, arrays: &mut ArrayMap) {
    fold_writers(txn, arrays);
    fold_post_refs(txn, arrays);
    fold_read_sites(txn, arrays);
}

fn fold_writers(txn: &crate::ast::Transaction, arrays: &mut ArrayMap) {
    // Distinct array names element-written by this txn.
    let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
    for stmt in &txn.body {
        let Some(name) = element_write_target(stmt) else {
            continue;
        };
        if !arrays.contains_key(name) || !written.insert(name.clone()) {
            continue;
        }
        if let Some((_, facts)) = arrays.get_mut(name) {
            facts.writers += 1;
        }
    }
}

fn fold_post_refs(txn: &crate::ast::Transaction, arrays: &mut ArrayMap) {
    // Post refs: obligation semantics — register-only.
    for_each_index_ref(&txn.contract.post_condition, &mut |n| {
        bump(arrays, n, |f| f.post_refs = true);
    });
}

fn fold_read_sites(txn: &crate::ast::Transaction, arrays: &mut ArrayMap) {
    // Pre-condition reads + body rhs/expressions are PORT SITES.
    for_each_index_ref(&txn.contract.pre_condition, &mut |n| {
        bump(arrays, n, |f| f.port_sites += 1);
    });
    for stmt in &txn.body {
        walk_stmt_read_exprs(stmt, &mut |n| bump(arrays, n, |f| f.port_sites += 1));
    }
}

fn element_write_target(stmt: &Statement) -> Option<&String> {
    let Statement::Assign(lhs, _) = stmt else {
        return None;
    };
    let Expr::Index(obj, _) = lhs else {
        return None;
    };
    match obj.as_ref() {
        Expr::Identifier(name) => Some(name),
        _ => None,
    }
}

fn bump(arrays: &mut ArrayMap, name: &str, g: fn(&mut ArrayFacts)) {
    if let Some((_, facts)) = arrays.get_mut(name) {
        g(facts);
    }
}

/// Walk READ expressions of a statement (assignment targets excluded —

/// Walk READ expressions of a statement (assignment targets excluded —
/// they ride the writer's write port, not a read port).
fn walk_stmt_read_exprs(stmt: &Statement, f: &mut impl FnMut(&str)) {
    match stmt {
        Statement::Assign(_, rhs) => for_each_index_ref(rhs, f),
        Statement::Expression(e) => for_each_index_ref(e, f),
        Statement::Let { expr: Some(e), .. } => for_each_index_ref(e, f),
        Statement::Block(body) | Statement::SyncBlock(body) => {
            for s in body {
                walk_stmt_read_exprs(s, f);
            }
        }
        _ => {}
    }
}

fn is_zero_literal(e: &Expr) -> bool {
    matches!(e, Expr::Decimal(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(depth: usize) -> ArrayFacts {
        ArrayFacts {
            depth,
            ..ArrayFacts::default()
        }
    }

    #[test]
    fn default_policy_threshold_64() {
        let p = MemPolicy::default();
        let d = decide_array_lowering(MemHint::None, &facts(64), &p).unwrap();
        assert_eq!(d.lowering, ArrayLowering::FirmMem);
        assert!(d.why.is_some(), "default decisions carry the note reason");
        let d = decide_array_lowering(MemHint::None, &facts(63), &p).unwrap();
        assert_eq!(d.lowering, ArrayLowering::RegFile);
        assert_eq!(d.why, Some("depth < threshold"));
    }

    #[test]
    fn explicit_pins_silence_and_override() {
        let p = MemPolicy::default();
        let d = decide_array_lowering(MemHint::Reg, &facts(256), &p).unwrap();
        assert_eq!(d.lowering, ArrayLowering::RegFile);
        assert!(d.why.is_none());
        let d = decide_array_lowering(
            MemHint::Mem,
            &ArrayFacts {
                depth: 8,
                ..facts(8)
            },
            &p,
        )
        .unwrap();
        assert_eq!(d.lowering, ArrayLowering::FirmMem);
        assert!(d.why.is_none());
    }

    #[test]
    fn mem_pin_with_post_refs_is_capability_error() {
        let p = MemPolicy::default();
        let f = ArrayFacts {
            post_refs: true,
            ..facts(128)
        };
        let err = decide_array_lowering(MemHint::Mem, &f, &p).unwrap_err();
        assert!(err.contains("reg let"), "err: {err}");
    }

    #[test]
    fn mem_pin_multi_writer_is_capability_error() {
        let p = MemPolicy::default();
        let f = ArrayFacts {
            writers: 2,
            ..facts(128)
        };
        assert!(decide_array_lowering(MemHint::Mem, &f, &p).is_err());
    }

    #[test]
    fn default_gates_fall_back_to_regfile_with_reasons() {
        let p = MemPolicy::default();
        let post = ArrayFacts {
            post_refs: true,
            ..facts(128)
        };
        let d = decide_array_lowering(MemHint::None, &post, &p).unwrap();
        assert_eq!(
            (d.lowering, d.why),
            (
                ArrayLowering::RegFile,
                Some("a postcondition reads elements")
            )
        );

        let writers = ArrayFacts {
            writers: 2,
            ..facts(128)
        };
        let d = decide_array_lowering(MemHint::None, &writers, &p).unwrap();
        assert_eq!(d.why, Some("multiple writing transactions"));

        let ports = ArrayFacts {
            port_sites: 5,
            ..facts(128)
        };
        let d = decide_array_lowering(MemHint::None, &ports, &p).unwrap();
        assert_eq!(d.why, Some("port sites above budget"));

        let init = ArrayFacts {
            nonzero_init: true,
            ..facts(128)
        };
        let d = decide_array_lowering(MemHint::None, &init, &p).unwrap();
        assert_eq!(d.why, Some("nonzero initializer"));
    }
}
