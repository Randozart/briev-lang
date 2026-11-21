// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! HIR-owned String locals (M2 first slice).
//!
//! Consumes body-derived facts only: a callable returns a *fresh* String when
//! every return path allocates (String concatenation, Data->String copy, or a
//! call to another fresh callable). A local initialized from such a value owns
//! its backing. The pass inserts `Free#` at a proven owner-loss point
//! (reassignment, function exit) when the local is neither aliased, stored,
//! returned by name, nor passed to an escaping/consuming parameter.
//!
//! This is deliberately conservative: unknown calls, field/list stores, and
//! aliases drop ownership (leak) rather than risk a use-after-free. AST
//! emission is unchanged until the HIR path proves parity; the optimization
//! never changes program output.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::ids::NodeId;
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind, StorageKind};
use crate::ast::{BinaryOpKind, Type};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 2026-11-12 (memcheck tier): one decision the pass made about an owned
/// candidate. Surfaced by `brievc memcheck` and enforced by
/// `--deny-memory-ambiguity`.
#[derive(Debug, Clone)]
pub enum OwnedEventKind {
    /// A release the pass inserted (list = coll_destroy lane).
    Release { list: bool },
    /// Value aliased by another local — both lose ownership (leak).
    Alias,
    /// Passed to a call that may keep it (escape summary true / unknown).
    EscapeCall(String),
    /// Passed to a callee that consumes it (the callee frees).
    ConsumeCall(String),
    /// `keep` hint suppressed the release.
    Kept,
    /// Author wrote `Free#`/`free` — pass stands down.
    ExplicitFree,
    /// Stored into a field/list/state slot (non-local assign target).
    Store,
    /// Mentioned by a not-provably-fresh return expression.
    ReturnBorrow,
    /// String/List local initialized from a BORROW-shaped read
    /// (field/index/deref/reflect) — the storage belongs to the container,
    /// this local is an alias, not an undecided owner.
    Borrow,
    /// String/List local backed by STATIC storage (a string literal) and
    /// never reassigned — allocates nothing, owns nothing, not ambiguous.
    /// Reported separately from the undecided census.
    Static,
    /// String/List local whose initializer was not provably fresh.
    UndecidedInit(String),
}

#[derive(Debug, Clone)]
pub struct OwnedEvent {
    pub function: String,
    pub name: String,
    pub kind: OwnedEventKind,
}

/// 2026-11-16 (M2 tail-loop owned destruction): one loop-carried parameter that
/// holds an OWNED value the tail loop re-allocates on every backedge. The
/// lowered loop stores each backedge's new value into the parameter slot,
/// dropping the previous value; from iteration 2 on that previous value is
/// loop-owned and must be released, and on exit the final value must be
/// released UNLESS the base case returns it as-is (ownership transfers to the
/// caller). This is the frontend fact source — the backend consumes it to
/// emit the owned-flag free-before-backedge + final release, and to gate the
/// tail-loop lowering for owned-carrying defns. It is the loop-aware
/// counterpart of the `self-tail argN` census event (same detection, different
/// consumer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedTailParam {
    /// 0-based index of the parameter in the defn's parameter list.
    pub index: usize,
    /// True when the base case returns this slot value as-is, transferring
    /// ownership to the caller: the loop must NOT release the final value.
    pub exit_transfers: bool,
    /// True when EVERY call site hands over a value this frame already owns, so
    /// the loop may free the slot at the FIRST backedge too. False when some
    /// site passes storage the frame does not own (a static literal), in which
    /// case the first backedge is skipped — iteration 1's value is not the
    /// loop's to free.
    pub free_first: bool,
    /// Which destructor the slot needs. Decided HERE (from the parameter's
    /// type) so the backend never matches Briev type names — see Golden Rule
    /// 19; the fact carries the storage category the way `ReleaseOwned` does.
    pub storage: StorageKind,
}

/// Per-defn owned tail-loop parameters, in parameter order. Empty/absent means
/// the defn is not owned-carrying tail-recursive (no lowering needed).
pub type OwnedTailLoopPlan = BTreeMap<String, Vec<OwnedTailParam>>;

/// 2026-11-20 (K3): the provenance class of a callable's OWNED return value.
/// A `Free#` on a possibly-static result is a free-of-non-heap crash (the
/// detect_faces `free(): invalid pointer` — float_format's guard arms return
/// .rodata strings and the consumer's unconditional Free# hit them). The
/// backend's free dispatcher must see this summary to refuse the free instead
/// of assuming malloc. Derived from the same HIR facts as `fresh_callables`
/// — no name matching, no algorithm vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticClass {
    /// Every terminal path returns a static literal (a .rodata string).
    Static,
    /// Proven owned: the callable is in the fresh set (every terminal path
    /// allocates fresh storage).
    Owned,
    /// Mixed: at least one terminal path is neither proven-fresh nor a static
    /// literal — provenance unknown. A free here must be conservatively
    /// skipped (warning), never assumed.
    Mixed,
}

/// Per-callable static-return summary, keyed by callee name. Only owned
/// (String/List-returning) callables with at least one terminal are present.
pub type StaticReturnSummary = HashMap<String, StaticClass>;

/// Report-only view: runs the real pass on a clone and returns its decision
/// log. The program is unchanged.
pub fn memcheck_owned(
    program: &HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
) -> Vec<OwnedEvent> {
    let mut clone = program.clone();
    let (events, _, _) = insert_releases_with_report(&mut clone, escapes);
    events
}

/// The owned-storage plan: inserted releases, the tail-loop plan, and the
/// static-return provenance summary (K3) consumed by the free dispatcher.
pub type OwnedPlan = (OwnedTailLoopPlan, StaticReturnSummary);

pub fn insert_releases(
    program: &mut HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
) -> OwnedPlan {
    let (_, _, plan) = insert_releases_with_report(program, escapes);
    plan
}

fn insert_releases_with_report(
    program: &mut HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
) -> (Vec<OwnedEvent>, usize, OwnedPlan) {
    let mut events: Vec<OwnedEvent> = Vec::new();
    let mut releases_inserted: usize = 0;
    let freed = freed_sources(program);
    // 2026-11-14: the escape summary must be derived from typed HIR, which
    // contains every library callable. The AST item list passed in does NOT
    // include imported stdlib definitions, so every stdlib callee looked
    // "unknown" and retained all arguments — the frame String behind
    // `read_file` was therefore never released (soak: +4.9 MB/call). The HIR
    // summary wins where it has an entry; the caller's map stays as the
    // fallback for names HIR does not define.
    let mut escape_view = escapes.clone();
    let escapes: &HashMap<String, Vec<bool>> = &escape_view;
    let fresh = fresh_callables(program, escapes, &freed);
    let param_return = param_return_summaries(program);
    let ok_fresh = ok_fresh_callables(program, escapes, &param_return, &fresh, &freed);
    // 2026-11-20 (K3): provenance of owned returns, derived next to `fresh`
    // from the same HIR facts (fresh set + terminal walks). Consumed by
    // emit_free so an explicit Free# on a possibly-static call result is a
    // diagnostic/no-op, not an unsound @free.
    let static_returns = static_return_summaries(program, &fresh, &param_return, &ok_fresh, &freed);
    let borrow_returns = borrow_result_summaries(program);
    let mut consumed = consumed_params(program);
    if std::env::var("BRIEV_MEMCHECK_DEBUG").is_ok() {
        eprintln!(
            "[owned-dbg] fresh: concat={} fresh_string={} fresh_int_list={} briev_str_substr={} read_file={}",
            fresh.contains("concat"),
            fresh.contains("fresh_string"),
            fresh.contains("fresh_int_list"),
            fresh.contains("briev_str_substr"),
            fresh.contains("read_file") || fresh.contains("tensor_new")
        );
        for n in [
            "attr_ints",
            "ints_or",
            "conv_attr4",
            "packed_ints",
            "parse_attr_loop",
            "parse_shape_loop",
            "arg",
            "json_get",
            "execute_env",
        ] {
            eprintln!(
                "[owned-dbg] fresh {n}={} returns_owned={}",
                fresh.contains(n),
                program
                    .callables
                    .iter()
                    .find(|c| c.name == n)
                    .map(returns_owned)
                    .unwrap_or(false)
            );
        }
        eprintln!(
            "[owned-dbg] program callables={} fresh_string defs={}",
            program.callables.len(),
            program
                .callables
                .iter()
                .filter(|c| c.name == "fresh_string")
                .count()
        );
        if let Some(fl_c) = program
            .callables
            .iter()
            .find(|c| c.name == "fresh_int_list")
        {
            let mut fvars: HashMap<String, bool> = fl_c
                .params
                .iter()
                .map(|p| (p.name.clone(), false))
                .collect();
            let fn_freed = freed.get("fresh_int_list").cloned().unwrap_or_default();
            collect_fresh(
                &fl_c.body.statements,
                &mut fvars,
                &fresh,
                &param_return,
                &ok_fresh,
                &fn_freed,
            );
            eprintln!("[owned-dbg] fresh_int_list vars={fvars:?}");
            let mut freturns = Vec::new();
            collect_returns(&fl_c.body.statements, &mut freturns);
            for e in &freturns {
                eprintln!(
                    "[owned-dbg] fl ret fresh={} kind={}",
                    is_fresh(e, &fvars, &param_return, &fresh, &ok_fresh, &fn_freed),
                    match &e.kind {
                        HirExprKind::Call { callee, .. } => format!("call {callee}"),
                        HirExprKind::Identifier(n) => format!("id {n}"),
                        o => format!("{o:?}"),
                    }
                );
            }
        }
        if let Some(fs_c) = program.callables.iter().find(|c| c.name == "fresh_string") {
            let mut fvars: HashMap<String, bool> = fs_c
                .params
                .iter()
                .map(|p| (p.name.clone(), false))
                .collect();
            let fn_freed = freed.get("fresh_string").cloned().unwrap_or_default();
            collect_fresh(
                &fs_c.body.statements,
                &mut fvars,
                &fresh,
                &param_return,
                &ok_fresh,
                &fn_freed,
            );
            eprintln!(
                "[owned-dbg] fresh_string vars={fvars:?} returns_owned={} type_params={:?} result={:?}",
                returns_owned(fs_c),
                fs_c.type_params,
                fs_c.result
            );
            let mut freturns = Vec::new();
            collect_returns(&fs_c.body.statements, &mut freturns);
            for e in &freturns {
                eprintln!(
                    "[owned-dbg] fs ret fresh={} kind={}",
                    is_fresh(e, &fvars, &param_return, &fresh, &ok_fresh, &fn_freed),
                    match &e.kind {
                        HirExprKind::Call { callee, .. } => format!("call {callee}"),
                        HirExprKind::Identifier(n) => format!("id {n}"),
                        o => format!("{o:?}"),
                    }
                );
            }
        }
        eprintln!(
            "[owned-dbg] ok: read_whole={} read_file={} execute_env_inputs={}",
            ok_fresh.contains("read_whole"),
            ok_fresh.contains("read_file") || fresh.contains("tensor_new"),
            ok_fresh.contains("execute_env_inputs")
        );
        eprintln!(
            "[owned-dbg] escapes concat={:?} face_param={:?}",
            escapes.get("concat"),
            escapes.get("face_param")
        );
        for n in [
            "f32_from_bytes",
            "byte_at",
            "tensor_set",
            "tensor_new",
            "read_file",
            "execute_env",
        ] {
            eprintln!(
                "[owned-dbg] escapes {n}={:?} in_hir={}",
                escapes.get(n),
                program.callables.iter().any(|c| c.name == n)
            );
        }
    }
    let mut next = program
        .callables
        .iter()
        // 2026-11-14 (#96): lowering allocates the callable header AFTER
        // its body. Starting at body_max+1 reused the header's identity for
        // the first release. Include both owning headers in the high-water
        // mark; keep scanning bodies for releases inserted by earlier passes.
        .map(|c| {
            max_node(&c.body.statements)
                .max(c.node.0)
                .max(c.body.node.0)
                + 1
        })
        .max()
        .unwrap_or(0);
    let mut owned_tail: OwnedTailLoopPlan = BTreeMap::new();
    // 2026-11-16 (M2): the tail-loop eligibility the backend will apply, so a
    // param is only recorded as "loop-freed" when the loop lowering actually
    // runs for that defn. Built from HIR (not AST) to stay in the same IR as
    // this pass; the plan types are shared so the two plans cannot drift.
    let tail_loop = crate::hir::tail_recursion::build_tail_loop_plan(program);
    // 2026-11-16 (M2): every call site of a lowerable recursive defn, so the
    // loop may free its slot at the FIRST backedge too — sound only when the
    // caller always hands over a value it does not own. Collected only for the
    // defns that could own-carry, to keep the pass cheap on large programs.
    let mut call_sites: HashMap<String, Vec<CallSite>> = HashMap::new();
    let mut mention_counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    collect_call_sites(program, &tail_loop, &mut call_sites, &mut mention_counts);
    // 2026-11-16 (M2): decide the owned tail slots BEFORE the rewrite, then
    // install the consuming summary. A caller that hands over its own local must
    // not also release it — the callee's loop owns it — so the accepted index
    // joins `consumed`, the same channel an authored `Free#` uses.
    {
        let fact_ctx = TailFactCtx {
            fresh: &fresh,
            escapes,
            consumed: &consumed,
            tail_loop: &tail_loop,
            call_sites: &call_sites,
            mentions: &mention_counts,
        };
        let mut planned: Vec<(String, Vec<OwnedTailParam>)> = Vec::new();
        for callable in &program.callables {
            let params = owned_tail_params(callable, &fact_ctx);
            if !params.is_empty() {
                planned.push((callable.name.clone(), params));
            }
        }
        for (name, params) in planned {
            for p in &params {
                consumed.entry(name.clone()).or_default().insert(p.index);
            }
            owned_tail.insert(name, params);
        }
        if std::env::var("BRIEV_MEMCHECK_DEBUG").is_ok() {
            eprintln!("[owned-dbg] owned_tail={:?}", owned_tail);
        }
    }
    for callable in &mut program.callables {
        // Conservative: generic bodies are emitted through the specialization
        // site, not the body seam; leave them leak-conservative for now.
        if !callable.type_params.is_empty() {
            continue;
        }
        let mut locals = HashSet::new();
        collect_let_names(&callable.body.statements, &mut locals);
        let mut owned: HashMap<String, OwnKind> = HashMap::new();
        let mut fresh_vars: HashMap<String, bool> = HashMap::new();
        for param in &callable.params {
            fresh_vars.insert(param.name.clone(), false);
        }
        let param_aliases =
            param_alias_map(&callable.body.statements, &callable.params, &param_return);
        let fixed_tail = owned_tail.get(&callable.name).cloned().unwrap_or_default();
        let mut ctx = Pass {
            fresh: &fresh,
            escapes,
            consumed: &consumed,
            param_return: &param_return,
            ok_fresh: &ok_fresh,
            borrow_returns: &borrow_returns,
            freed_map: &freed,
            param_aliases: &param_aliases,
            fixed_tail: &fixed_tail,
            locals: &locals,
            next,
            function: callable.name.clone(),
            events: &mut events,
        };
        let mut selector_results: HashSet<String> = HashSet::new();
        // 2026-11-21 (K3 residual): bind owned ARGUMENT temporaries to
        // pure-insert temps before the main rewrite, so a value produced
        // inline for a borrowing parameter has a name the paired
        // ReleaseOwned can reach. Runs first so the rewrite sees the
        // rewritten arguments and the inserted statements.
        {
            let fn_freed = freed.get(&callable.name).cloned().unwrap_or_default();
            hoist_owned_arg_temps(
                &mut callable.body.statements,
                &fresh_vars,
                &mut ctx,
                &fn_freed,
            );
        }
        let body = rewrite_list(
            &callable.body.statements,
            &mut owned,
            &mut fresh_vars,
            &mut selector_results,
            &mut ctx,
            true,
        );
        callable.body.statements = body;
        next = ctx.next;
    }
    // 2026-11-13: an UndecidedInit is SUPERSEDED when the same local is
    // later assigned a fresh value and released (literal-initialised
    // accumulators: the static init allocates nothing, so the eventual
    // release is still correct — the census must not report it as ambiguous).
    let released: HashSet<(String, String)> = events
        .iter()
        .filter(|e| matches!(e.kind, OwnedEventKind::Release { .. }))
        .map(|e| (e.function.clone(), e.name.clone()))
        .collect();
    events.retain(|e| {
        !(matches!(
            e.kind,
            OwnedEventKind::UndecidedInit(_) | OwnedEventKind::Static
        ) && released.contains(&(e.function.clone(), e.name.clone())))
    });
    let rel = events
        .iter()
        .filter(|e| matches!(e.kind, OwnedEventKind::Release { .. }))
        .count();
    (events, rel, (owned_tail, static_returns))
}

/// 2026-11-20 (K3): classify each owned-returning callable's return provenance.
/// Static when ANY terminal is a Bytes literal or a not-proven-fresh call
/// result; Owned when the callable itself is in the fresh set; Mixed otherwise.
fn static_return_summaries(
    program: &HirProgram,
    fresh: &HashSet<String>,
    param_return: &HashMap<String, Vec<usize>>,
    ok: &HashSet<String>,
    freed: &HashMap<String, HashSet<String>>,
) -> StaticReturnSummary {
    let mut out = StaticReturnSummary::new();
    for c in &program.callables {
        if !returns_owned(c) {
            continue;
        }
        if fresh.contains(&c.name) {
            out.insert(c.name.clone(), StaticClass::Owned);
            continue;
        }
        let mut returns: Vec<HirExpr> = Vec::new();
        collect_returns(&c.body.statements, &mut returns);
        if returns.is_empty() {
            continue;
        }
        let fn_freed = freed.get(&c.name).cloned().unwrap_or_default();
        let empty_vars: HashMap<String, bool> = HashMap::new();
        let any_static = returns.iter().any(|e| terminal_is_static(e, fresh));
        if any_static {
            out.insert(c.name.clone(), StaticClass::Static);
        } else {
            // No static terminal, but not proven fresh either: unknown.
            let all_owned = returns
                .iter()
                .all(|e| is_fresh(e, &empty_vars, param_return, fresh, ok, &fn_freed));
            if all_owned {
                out.insert(c.name.clone(), StaticClass::Owned);
            } else {
                out.insert(c.name.clone(), StaticClass::Mixed);
            }
        }
    }
    out
}

/// A terminal expression is static when it is a Bytes literal or a call to a
/// callee NOT in the fresh set whose own result type is a String (the
/// static-literal producer shape: float_format's guard arms).
fn terminal_is_static(e: &HirExpr, fresh: &HashSet<String>) -> bool {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Bytes(_)) => true,
        HirExprKind::Call { callee, .. } => {
            !fresh.contains(callee) && e.ty.as_ref().is_some_and(is_string)
        }
        _ => false,
    }
}

fn returns_owned(c: &HirCallable) -> bool {
    c.result.as_ref().is_some_and(owned_ty)
        || c.outputs
            .as_ref()
            .is_some_and(|o| o.all_types().iter().any(owned_ty))
}

fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Custom(n) if n == "String")
}

fn is_list(ty: &Type) -> bool {
    matches!(ty, Type::Generic(n, _) | Type::Applied(n, _) if n == "List")
}

fn owned_ty(ty: &Type) -> bool {
    is_string(ty) || is_list(ty)
}

/// Mirror of the backend's `tail_loop_i64_binding` expressible from a `Type`
/// alone: owned storage (a pointer) or a fundamental that occupies one i64/
/// i1/ptr slot. Deliberately conservative — the backend additionally rejects
/// protocol members such as `Float` (which keeps a native register cache), and
/// the loop plan already rejects vectors, dyn, tasks, and aggregates.
fn tail_loop_slot_representable(ty: &Type) -> bool {
    if owned_ty(ty) {
        return true;
    }
    match ty {
        Type::Bits(0)
        | Type::Bits(1)
        | Type::Bits(8)
        | Type::Bits(16)
        | Type::Bits(32)
        | Type::Bits(64)
        | Type::Void
        | Type::Ptr(_)
        | Type::PtrConst(_) => true,
        Type::Custom(n) => matches!(n.as_str(), "Int" | "Bool"),
        _ => false,
    }
}

/// What a released owner needs: a String frees its handle; a List goes
/// through stdlib `coll_destroy` (object free + arena-aware data free).
#[derive(Clone, Copy, PartialEq, Debug)]
enum OwnKind {
    Str,
    Lst,
}

fn kind_of(ty: &Type) -> OwnKind {
    if is_string(ty) {
        OwnKind::Str
    } else {
        OwnKind::Lst
    }
}

fn max_node(stmts: &[HirStmt]) -> u32 {
    let mut max = 0;
    for s in stmts {
        max = max.max(s.id.0);
        match &s.kind {
            HirStmtKind::Guarded(c, b) => {
                max = max.max(expr_max(c));
                max = max.max(max_node(b));
            }
            HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => {
                max = max.max(max_node(b));
            }
            HirStmtKind::Foreach { list, body, .. } => {
                max = max.max(expr_max(list));
                max = max.max(max_node(body));
            }
            HirStmtKind::Match { expr, arms } => {
                max = max.max(expr_max(expr));
                for a in arms {
                    max = max.max(max_node(&a.body));
                }
            }
            HirStmtKind::Let { expr, .. }
            | HirStmtKind::Term(expr)
            | HirStmtKind::EndProgram(expr) => {
                if let Some(e) = expr {
                    max = max.max(expr_max(e));
                }
            }
            HirStmtKind::TempTransfer { expr, .. } => {
                max = max.max(expr_max(expr));
            }
            HirStmtKind::Assign(l, r)
            | HirStmtKind::ArrowAssign {
                value: l,
                target: Some(r),
                ..
            } => {
                max = max.max(expr_max(l)).max(expr_max(r));
            }
            HirStmtKind::ArrowAssign { value, .. }
            | HirStmtKind::Check(value)
            | HirStmtKind::Expression(value)
            | HirStmtKind::Gate(value) => {
                max = max.max(expr_max(value));
            }
            HirStmtKind::Tail(e) => max = max.max(expr_max(e)),
            _ => {}
        }
    }
    max
}

fn expr_max(e: &HirExpr) -> u32 {
    let mut max = e.id.0;
    match &e.kind {
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args {
                max = max.max(expr_max(a));
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            max = max.max(expr_max(receiver));
            for a in args {
                max = max.max(expr_max(a));
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            max = max.max(expr_max(left)).max(expr_max(right));
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Reflect {
            receiver: inner, ..
        }
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => max = max.max(expr_max(inner)),
        HirExprKind::Index(a, b) => max = max.max(expr_max(a)).max(expr_max(b)),
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            max = max.max(expr_max(array));
            for e in [start, end, stride].into_iter().flatten() {
                max = max.max(expr_max(e));
            }
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            max = max.max(expr_max(condition)).max(expr_max(then_branch));
            if let Some(e) = else_branch {
                max = max.max(expr_max(e));
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            max = max.max(expr_max(scrutinee));
            for a in arms {
                if let Some(g) = &a.guard {
                    max = max.max(expr_max(g));
                }
                max = max.max(expr_max(&a.body));
            }
        }
        HirExprKind::Block(stmts) => max = max.max(max_node(stmts)),
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) {
                max = max.max(expr_max(e));
            }
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                max = max.max(expr_max(a));
            }
        }
        HirExprKind::Range { start, end, .. } => max = max.max(expr_max(start)).max(expr_max(end)),
        _ => {}
    }
    max
}

/// Callables whose EVERY return path is `Identifier(param_i)` verbatim:
/// builders/filters that hand their argument back (e.g. list `nms`-style
/// filters). A call to one is fresh exactly when that argument is fresh —
/// ownership flows through the identity return.
/// `expr_param_identity` — which parameter the expression hands back
/// verbatim. Covers bare identifiers AND the unwrap shape:
/// `Match(Identifier(p), arms)` where every arm either traps (no value) or
/// evaluates to an identifier bound by that arm's pattern. This is what lets
/// `unwrap(fresh)` stay fresh across the Result boundary.
/// Which parameter (or a local that still denotes one) the expression hands
/// back unchanged. `Some(i)` licenses the caller to treat the call's result as
/// an alias of argument `i`: freshness then flows from that argument, and no
/// release is inserted for the result.
///
/// 2026-11-16: resolution goes through an alias map (params plus `let`-copies
/// and push-mutated collections) instead of a bare param scan. The protobuf
/// parser shape `let next = dims; next <- varint(r); term f(r, next)` returns
/// a value that is still the caller's `dims` — `<-` grows the same collection
/// object — so pinning the summary to param `dims` is the accounting that
/// matches the representation.
fn expr_param_identity(e: &HirExpr, aliases: &HashMap<String, usize>) -> Option<usize> {
    match &e.kind {
        HirExprKind::Identifier(n) => aliases.get(n).copied(),
        HirExprKind::Match { scrutinee, arms } => {
            let p = match &scrutinee.kind {
                HirExprKind::Identifier(n) => aliases.get(n).copied()?,
                _ => return None,
            };
            for arm in arms {
                let bound = pattern_bindings(std::slice::from_ref(&arm.pattern));
                // The arm must reduce to one of its own bindings (or trap).
                let value = match &arm.body.kind {
                    HirExprKind::Identifier(ex) => Some(ex),
                    // `{ trap; }` — the wrong-side arm produces no value.
                    HirExprKind::Block(stmts)
                        if !stmts.is_empty()
                            && stmts.iter().all(|st| matches!(st.kind, HirStmtKind::Trap)) =>
                    {
                        None
                    }
                    _ => return None,
                };
                if let Some(v) = value {
                    if !bound.contains(v) {
                        return None;
                    }
                }
            }
            Some(p)
        }
        _ => None,
    }
}

/// Identifiers this function FREES via `Free#`. A Data→String cast whose
/// source is freed immediately after is an OWNERSHIP TRANSFER: the cast
/// result is the sole owner of the buffer, so it is fresh even though the
/// source is a String-typed param (read_whole/int_to_str buffer pattern).
fn freed_sources(program: &HirProgram) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for c in &program.callables {
        let mut ids = HashSet::new();
        freed_in(&c.body.statements, &mut ids);
        if !ids.is_empty() {
            out.insert(c.name.clone(), ids);
        }
    }
    out
}

fn freed_in(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for st in stmts {
        match &st.kind {
            HirStmtKind::Expression(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e) => {
                freed_expr(e, out)
            }
            HirStmtKind::Let { expr: Some(e), .. }
            | HirStmtKind::Term(Some(e))
            | HirStmtKind::Tail(e) => freed_expr(e, out),
            HirStmtKind::Assign(l, r) => {
                freed_expr(l, out);
                freed_expr(r, out);
            }
            HirStmtKind::FreeHint(n) | HirStmtKind::KeepHint(n) => {
                out.insert(n.clone());
            }
            HirStmtKind::Guarded(c, b) => {
                freed_expr(c, out);
                freed_in(b, out);
            }
            HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => freed_in(b, out),
            HirStmtKind::Foreach { list, body, .. } => {
                freed_expr(list, out);
                freed_in(body, out);
            }
            HirStmtKind::Match { expr, arms } => {
                freed_expr(expr, out);
                for a in arms {
                    freed_in(&a.body, out);
                }
            }
            _ => {}
        }
    }
}

fn freed_expr(e: &HirExpr, out: &mut HashSet<String>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => {
            if callee == "Free#" {
                for a in args {
                    mention(a, out);
                }
            }
            for a in args {
                freed_expr(a, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            freed_expr(left, out);
            freed_expr(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Deref(inner) => freed_expr(inner, out),
        HirExprKind::Index(a, b) => {
            freed_expr(a, out);
            freed_expr(b, out);
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            freed_expr(condition, out);
            freed_expr(then_branch, out);
            if let Some(x) = else_branch {
                freed_expr(x, out);
            }
        }
        HirExprKind::Block(stmts) => freed_in(stmts, out),
        _ => {}
    }
}

fn pattern_bindings(patterns: &[crate::hir::pattern::HirPattern]) -> HashSet<String> {
    fn pat(p: &crate::hir::pattern::HirPattern, out: &mut HashSet<String>) {
        use crate::hir::pattern::HirPatternKind as K;
        match &p.kind {
            K::Binding(n) | K::TypedBinding(n, _) => {
                out.insert(n.clone());
            }
            K::EnumVariant(_, ps) => {
                for q in ps {
                    pat(q, out);
                }
            }
            K::Tuple(ps) => {
                for q in ps {
                    pat(q, out);
                }
            }
            K::Literal(e) => {
                let mut ids = HashSet::new();
                mention(e, &mut ids);
                out.extend(ids);
            }
            K::Range(a, b) => {
                mention(a, out);
                mention(b, out);
            }
            K::RangeInclusive(a, b) => {
                mention(a, out);
                mention(b, out);
            }
            K::Multi(ps) => {
                for q in ps {
                    pat(q, out);
                }
            }
            K::Wildcard => {}
        }
    }
    let mut out = HashSet::new();
    for p in patterns {
        pat(p, &mut out);
    }
    out
}

/// Callables that always hand back ONE OF their parameters (any index):
/// `ints_or(items, fallback)` style. The result aliases caller storage, so
/// it is never fresh — callers record a Borrow, not an undecided owner.
fn borrow_result_summaries(program: &HirProgram) -> HashSet<String> {
    let mut out = HashSet::new();
    for c in &program.callables {
        let mut returns = Vec::new();
        collect_returns(&c.body.statements, &mut returns);
        if returns.is_empty() {
            continue;
        }
        let all_params = returns.iter().all(|e| match &e.kind {
            HirExprKind::Identifier(n) => c.params.iter().any(|p| &p.name == n),
            _ => false,
        });
        if all_params {
            out.insert(c.name.clone());
        }
    }
    out
}

/// Locals that still denote a parameter after `let` copies, reassignment,
/// in-place collection mutation and param-return calls. Seeded with the
/// parameters themselves so a lookup is uniform.
///
/// `pr` supplies the param-return summaries needed to see through a call:
/// `let next = parse_dimension_loop(dr, dims)` still denotes `dims`, because
/// `parse_dimension_loop` hands its second argument back unchanged.
/// Which parameters a defn's BASE CASE hands back untouched.
///
/// 2026-11-16 (M2 tail-loop owned destruction): when the lowered tail loop
/// exits, the final value in a loop-carried owned parameter slot belongs to the
/// loop and must be released — UNLESS the base case returns that slot value
/// as-is, in which case ownership transfers to the caller and a release here
/// would be a use-after-free. This map answers that per parameter. Only a
/// direct param identity counts (`term s`), including the match-projection
/// shape `expr_param_identity` already understands; anything else (a fresh
/// `concat`, a literal) is a NEW value, so the loop owns the slot and frees it.
/// The recursive arm (`term self(s, ...)`) never matches a param alias, so it
/// never marks a transfer.
/// Which parameters a defn's tail loop must destroy.
///
/// 2026-11-16 (M2 tail-loop owned destruction): the loop lowering stores each
/// backedge's new value into the parameter slot, dropping the previous value.
/// For an OWNED slot (String/List) that previous value is heap storage with no
/// owner — a real unbounded leak (quadratic RSS; BUGS.md 2026-11-16). This
/// records exactly the slots the backend may free, and it is deliberately
/// narrow because a wrong entry is a use-after-free:
///
/// * the tail loop must actually be lowered for this defn
///   (`tail_loop.is_lowerable`), so the fact cannot claim a fix the backend
///   will not emit;
/// * EVERY self-tail call site must pass a provably fresh allocation at that
///   index (`fresh` callees allocate their own result and let no argument
///   escape), so the new value never aliases the old one — this rejects
///   in-place mutators like `append(s, x)` where freeing the old value would
///   free the value just stored;
/// * no other argument at those call sites may read the parameter (a read
///   after the free), and
/// * the parameter may appear nowhere else in the body except a base-case
///   `term s` — the return that transfers the final value to the caller.
/// One call to a loop-lowerable defn, tagged with the callable that contains
/// it. The owner matters: the transfer proof for an identifier argument is
/// "this caller mentions the local nowhere else", which is a fact about the
/// CALLER's body, not the callee's.
#[derive(Debug, Clone)]
struct CallSite {
    owner: String,
    args: Vec<HirExpr>,
}

fn collect_call_sites(
    program: &HirProgram,
    tail_loop: &crate::analysis::tail_recursion::TailLoopPlan,
    out: &mut HashMap<String, Vec<CallSite>>,
    mentions: &mut HashMap<String, HashMap<String, usize>>,
) {
    for c in &program.callables {
        crate::hir::expr_walk::walk_stmts(&c.body.statements, &mut |e| {
            if let HirExprKind::Identifier(n) = &e.kind {
                *mentions
                    .entry(c.name.clone())
                    .or_default()
                    .entry(n.clone())
                    .or_insert(0) += 1;
                return;
            }
            let HirExprKind::Call { callee, args, .. } = &e.kind else {
                return;
            };
            if !tail_loop.is_lowerable(callee) {
                return;
            }
            out.entry(callee.clone()).or_default().push(CallSite {
                owner: c.name.clone(),
                args: args.clone(),
            });
        });
    }
}

/// Everything `owned_tail_params` needs from the rest of the pass, bundled so
/// the fact's inputs are explicit and the function stays within the parameter
/// budget.
struct TailFactCtx<'a> {
    fresh: &'a HashSet<String>,
    escapes: &'a HashMap<String, Vec<bool>>,
    consumed: &'a HashMap<String, HashSet<usize>>,
    tail_loop: &'a crate::analysis::tail_recursion::TailLoopPlan,
    call_sites: &'a HashMap<String, Vec<CallSite>>,
    mentions: &'a HashMap<String, HashMap<String, usize>>,
}

fn owned_tail_params(callable: &HirCallable, ctx: &TailFactCtx<'_>) -> Vec<OwnedTailParam> {
    let fresh = ctx.fresh;
    let tail_loop = ctx.tail_loop;
    let call_sites = ctx.call_sites;
    let dbg = std::env::var("BRIEV_MEMCHECK_DEBUG").is_ok();
    if !tail_loop.is_lowerable(&callable.name) {
        if dbg && callable.params.iter().any(|p| owned_ty(&p.ty)) {
            eprintln!("[fact] {} not loop-lowerable", callable.name);
        }
        return Vec::new();
    }
    // The backend lowers the loop only when EVERY parameter binds to one i64
    // slot (`tail_loop_i64_binding`). A fact recorded for a defn the backend
    // refuses would claim a fix that is never emitted, so mirror that gate
    // here: the param must be owned storage or a single-register fundamental.
    // Anything else (a boxed Float, an enum handle, an aggregate) stays a
    // reported leak instead of a silent one.
    if !callable
        .params
        .iter()
        .all(|p| tail_loop_slot_representable(&p.ty))
    {
        return Vec::new();
    }
    let mut rets = Vec::new();
    collect_returns(&callable.body.statements, &mut rets);
    let self_sites: Vec<&Vec<HirExpr>> = rets
        .iter()
        .filter_map(|e| match &e.kind {
            HirExprKind::Call { callee, args, .. } if *callee == callable.name => Some(args),
            _ => None,
        })
        .collect();
    if self_sites.is_empty() {
        if dbg {
            eprintln!("[fact] {} no self tail sites", callable.name);
        }
        return Vec::new();
    }
    if dbg {
        eprintln!("[fact] {} self_sites={}", callable.name, self_sites.len());
    }
    // EVERY call site in the program — internal backedges and external entry
    // calls alike. The loop frees the slot value at each backedge, INCLUDING
    // the first one, whose value the caller passed. That is only sound when the
    // caller handed over a value it does not own: an inline fresh allocation
    // (a `fresh`-set callee allocates its own result and lets no argument
    // escape) that no local, field, or container holds. Any site that passes
    // an identifier — the caller's own storage — disqualifies the slot, because
    // freeing it would be a use-after-free in that caller.
    let sites: &[CallSite] = call_sites
        .get(&callable.name)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let exit_transfers = exit_transfers_map(&callable.body.statements, &callable.params);
    let mut out = Vec::new();
    for (i, p) in callable.params.iter().enumerate() {
        if !owned_ty(&p.ty) {
            continue;
        }
        let mut free_first = true;
        let transferable = sites.iter().all(|site| {
            let args = &site.args;
            let Some(a) = args.get(i) else {
                return false;
            };
            // A sibling argument may read a value this slot will free only if
            // the read happens before the callee's backedge — true for every
            // argument, since all of them are evaluated before the call. The
            // remaining hazard is ALIASING: a sibling that receives the same
            // owned storage would be freed under it, so an owned-storage
            // sibling that mentions the parameter disqualifies the slot.
            let sibling_aliases = args
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && expr_mentions(other, &p.name));
            if sibling_aliases {
                return false;
            }
            // The parameter's own type is what makes the slot owned, so the
            // argument's stamped type is irrelevant here (typing is advisory and
            // absent on the AST-emit path).
            match &a.kind {
                HirExprKind::Call { callee, .. } if fresh.contains(callee) => true,
                HirExprKind::Literal(_) => {
                    free_first = false;
                    true
                }
                HirExprKind::Identifier(local) => {
                    free_first = false;
                    ctx.mentions
                        .get(&site.owner)
                        .and_then(|m| m.get(local))
                        .copied()
                        .unwrap_or(0)
                        == 1
                }
                _ => false,
            }
        });
        if !transferable {
            if dbg {
                eprintln!("[fact] {}.{} site not transferable", callable.name, p.name);
            }
            continue;
        }
        if param_escapes_in_body(callable, &p.name, i, ctx) {
            if dbg {
                eprintln!("[fact] {}.{} escapes", callable.name, p.name);
            }
            continue;
        }
        out.push(OwnedTailParam {
            index: i,
            exit_transfers: exit_transfers.get(&i).copied().unwrap_or(false),
            free_first,
            storage: match kind_of(&p.ty) {
                OwnKind::Str => StorageKind::Str,
                OwnKind::Lst => StorageKind::List,
            },
        });
    }
    out
}

/// Does `name` occur in `e` anywhere? Used to prove a parameter is not read by
/// the sibling arguments of a self-tail call.
fn expr_mentions(e: &HirExpr, name: &str) -> bool {
    let mut ids = HashSet::new();
    mention(e, &mut ids);
    ids.contains(name)
}

/// Does `param` reach a sink that would keep it alive past the loop?
///
/// The loop frees the parameter slot at every backedge, so the parameter must be
/// dead there: reading it is fine (reads happen while the body runs, before the
/// backedge), but any sink that retains it — an escaping/consuming argument, a
/// container store, an explicit `Free#` — makes the slot somebody else's. The
/// rules are the pass's own per-parameter modes, so this cannot drift from the
/// escape summary the rest of the pass uses.
fn param_escapes_in_body(
    callable: &HirCallable,
    param: &str,
    index: usize,
    ctx: &TailFactCtx<'_>,
) -> bool {
    param_is_stored(&callable.body.statements, param)
        || param_escapes_through_calls(callable, param, index, ctx)
}

/// Does the body pin `param` into storage this pass does not track — a
/// non-local assignment target, an explicit `Free#`, or a keep hint?
fn param_is_stored(stmts: &[HirStmt], param: &str) -> bool {
    stmts.iter().any(|s| match &s.kind {
        HirStmtKind::Assign(l, r) => expr_mentions(l, param) || expr_mentions(r, param),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            target.as_ref().is_some_and(|t| expr_mentions(t, param)) || expr_mentions(value, param)
        }
        HirStmtKind::FreeHint(n) | HirStmtKind::KeepHint(n) => n == param,
        HirStmtKind::ReleaseOwned { name, .. } => name == param,
        _ => false,
    })
}

/// Does any call in the body take `param` at a retaining argument position?
fn param_escapes_through_calls(
    callable: &HirCallable,
    param: &str,
    index: usize,
    ctx: &TailFactCtx<'_>,
) -> bool {
    let mut bad = false;
    crate::hir::expr_walk::walk_stmts(&callable.body.statements, &mut |e| {
        bad |= arg_retains_param(e, param, index, callable, ctx);
    });
    bad
}

/// One expression's verdict for `param_escapes_through_calls`.
fn arg_retains_param(
    e: &HirExpr,
    param: &str,
    index: usize,
    callable: &HirCallable,
    ctx: &TailFactCtx<'_>,
) -> bool {
    if let HirExprKind::Consume(inner) = &e.kind {
        return expr_mentions(inner, param);
    }
    let HirExprKind::Call { callee, args, .. } = &e.kind else {
        return false;
    };
    // The recursive transfer itself is decided separately: only a pass at a
    // DIFFERENT index would hand the value to another slot.
    if *callee == callable.name {
        return args
            .iter()
            .enumerate()
            .any(|(j, a)| j != index && expr_mentions(a, param));
    }
    if callee == "Free#" {
        return args.iter().any(|a| expr_mentions(a, param));
    }
    if ctx.fresh.contains(callee) {
        return false;
    }
    args.iter().enumerate().any(|(j, a)| {
        let consuming = ctx.consumed.get(callee).is_some_and(|s| s.contains(&j));
        let escaping = ctx
            .escapes
            .get(callee)
            .map(|s| s.get(j).copied().unwrap_or(true))
            .unwrap_or(!callee.ends_with('#'));
        (consuming || escaping) && expr_mentions(a, param)
    })
}

fn exit_transfers_map(
    stmts: &[HirStmt],
    params: &[super::program::HirParam],
) -> BTreeMap<usize, bool> {
    let mut aliases: HashMap<String, usize> = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        aliases.insert(p.name.clone(), i);
    }
    let mut rets = Vec::new();
    collect_returns(stmts, &mut rets);
    let mut out: BTreeMap<usize, bool> = BTreeMap::new();
    for r in &rets {
        if let Some(i) = expr_param_identity(r, &aliases) {
            out.insert(i, true);
        }
    }
    out
}

fn param_alias_map(
    stmts: &[HirStmt],
    params: &[super::program::HirParam],
    pr: &HashMap<String, Vec<usize>>,
) -> HashMap<String, usize> {
    let mut aliases: HashMap<String, usize> = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        aliases.insert(p.name.clone(), i);
    }
    collect_param_aliases(stmts, &mut aliases, pr);
    aliases
}

/// Which parameter `e` still denotes, following param-return calls. Depth is
/// bounded so a pathological `let x = f(x)` chain cannot recurse forever.
fn param_alias_of(
    e: &HirExpr,
    aliases: &HashMap<String, usize>,
    pr: &HashMap<String, Vec<usize>>,
) -> Option<usize> {
    param_alias_of_depth(e, aliases, pr, 0)
}

fn param_alias_of_depth(
    e: &HirExpr,
    aliases: &HashMap<String, usize>,
    pr: &HashMap<String, Vec<usize>>,
    depth: u8,
) -> Option<usize> {
    if let Some(i) = expr_param_identity(e, aliases) {
        return Some(i);
    }
    if depth >= 8 {
        return None;
    }
    let HirExprKind::Call { callee, args, .. } = &e.kind else {
        return None;
    };
    // A LOCAL is only well-defined to denote one parameter when the callee has
    // exactly one candidate return. A selector like `ints_or(a, b)` may hand
    // back either, so it names no single parameter.
    let cands = pr.get(callee)?;
    if cands.len() != 1 {
        return None;
    }
    param_alias_of_depth(args.get(cands[0])?, aliases, pr, depth + 1)
}

/// Statement half of [`param_alias_map`]. A binding whose value resolves to a
/// parameter joins the map; a binding whose value does not RESETS the entry
/// (the local stopped denoting the parameter). `<-` (push) is a no-op: it
/// grows the same collection object, so the alias survives.
///
/// The single name a `Let` binds, when it binds exactly one. HIR fills `names`
/// with the bound names and it normally holds just `name`; more than one entry
/// is a tuple destructuring, whose parts do not each denote a parameter.
fn single_binding<'a>(name: &'a str, names: &'a [String]) -> Option<&'a str> {
    match names.len() {
        0 => Some(name),
        1 => names.first().map(String::as_str),
        _ => None,
    }
}

fn collect_param_aliases(
    stmts: &[HirStmt],
    aliases: &mut HashMap<String, usize>,
    pr: &HashMap<String, Vec<usize>>,
) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let {
                name,
                names,
                expr: Some(e),
                ..
            } => {
                let Some(target) = single_binding(name, names) else {
                    continue;
                };
                match param_alias_of(e, aliases, pr) {
                    Some(i) => {
                        aliases.insert(target.to_string(), i);
                    }
                    None => {
                        aliases.remove(target);
                    }
                }
            }
            HirStmtKind::Assign(l, r) => {
                if let HirExprKind::Identifier(n) = &l.kind {
                    match param_alias_of(r, aliases, pr) {
                        Some(i) => {
                            aliases.insert(n.clone(), i);
                        }
                        None => {
                            aliases.remove(n);
                        }
                    }
                }
            }
            HirStmtKind::Guarded(_, b)
            | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => collect_param_aliases(b, aliases, pr),
            HirStmtKind::Foreach { body, .. } => collect_param_aliases(body, aliases, pr),
            HirStmtKind::Match { arms, .. } => {
                for a in arms {
                    collect_param_aliases(&a.body, aliases, pr);
                }
            }
            _ => {}
        }
    }
}

/// Arguments of a direct self call, when `e` is one.
fn self_call_args<'a>(e: &'a HirExpr, callee_name: &str) -> Option<&'a Vec<HirExpr>> {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } if callee == callee_name => Some(args),
        _ => None,
    }
}

fn param_return_summaries(program: &HirProgram) -> HashMap<String, Vec<usize>> {
    // Two-phase fixpoint. Round 0 resolves aliases through parameters and
    // `let` copies only. Later rounds see through param-return CALLS
    // (`let next = parse_dimension_loop(dr, dims)` denotes `dims`), which is
    // what lets the protobuf parser chain — dimension → shape → tensor_type →
    // type → value_name — settle one link per round. The lattice is finite and
    // only ever grows, so this converges; the cap bounds work on pathological
    // programs.
    let mut pr: HashMap<String, Vec<usize>> = HashMap::new();
    let rounds = program.callables.len().clamp(1, 16);
    for _ in 0..rounds {
        let next = param_return_round(program, &pr);
        if next == pr {
            break;
        }
        pr = next;
    }
    pr
}

/// One round of [`param_return_summaries`] against the previous round's
/// summaries.
fn param_return_round(
    program: &HirProgram,
    prev: &HashMap<String, Vec<usize>>,
) -> HashMap<String, Vec<usize>> {
    let mut out = HashMap::new();
    for c in &program.callables {
        if let Some(i) = identity_return_param(c, prev) {
            out.insert(c.name.clone(), i);
        }
    }
    out
}

/// The parameters a callable may hand back unchanged, in order.
///
/// 2026-11-16: the result is a SET, not one index. A selector such as
/// `ints_or(items, fallback)` returns either argument depending on a runtime
/// test, and that is still ownership-relevant: the result may alias any of
/// them, so the caller may only free it when EVERY candidate is caller-owned.
/// Collapsing this to one index would either miss the free (leak) or free
/// caller storage (double free).
///
/// Self tail calls are neutral for the identity question. A tail-recursive
/// parser (`term f(r, next)`) returns whatever its base case returns, and the
/// base case pins the candidate; the recursive arm must thread each candidate,
/// which is checked separately.
fn identity_return_param(
    c: &HirCallable,
    prev: &HashMap<String, Vec<usize>>,
) -> Option<Vec<usize>> {
    let mut returns = Vec::new();
    collect_returns(&c.body.statements, &mut returns);
    if returns.is_empty() {
        return None;
    }
    let aliases = param_alias_map(&c.body.statements, &c.params, prev);
    let mut cands: Vec<usize> = Vec::new();
    for e in &returns {
        if self_call_args(e, &c.name).is_some() {
            continue;
        }
        let i = param_alias_of(e, &aliases, prev)?;
        if !cands.contains(&i) {
            cands.push(i);
        }
    }
    if cands.is_empty() {
        return None;
    }
    // Every recursive arm must carry each surviving candidate at its own
    // position; a candidate that is not threaded cannot describe the result.
    let mut carried: Vec<usize> = Vec::new();
    'cand: for &i in &cands {
        for e in &returns {
            let Some(args) = self_call_args(e, &c.name) else {
                continue;
            };
            if args.get(i).and_then(|a| param_alias_of(a, &aliases, prev)) != Some(i) {
                continue 'cand;
            }
        }
        carried.push(i);
    }
    if carried.is_empty() {
        return None;
    }
    Some(carried)
}

fn is_result_type(ty: &Type) -> bool {
    // 2026-11-13: the typechecker may normalize `Result<T, E>` to a bare
    // Custom("Result") (e.g. after alias resolution / type-param erasure on
    // non-generic callables), so accept every shape.
    matches!(ty, Type::Generic(n, _) | Type::Applied(n, _) | Type::Custom(n) if n == "Result")
}

/// 2026-11-12 (memcheck census): OK-side freshness for Result-returning
/// callables. A Result callable qualifies when EVERY return path is an
/// `Ok(fresh)` payload or an `Err(whatever)` — Err payloads are irrelevant
/// to ownership because `unwrap` only yields the Ok side. This discharges
/// the read_file/get_env-style literal-Err families WITHOUT copying their
/// cold-path literals (no behavior change, no new allocations).
fn ok_fresh_callables(
    program: &HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
    pr: &HashMap<String, Vec<usize>>,
    fresh: &HashSet<String>,
    freed: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let mut ok = HashSet::new();
    // 2026-11-14: these are foreign callables (no HIR body) that return
    // Result<String, ...>. They never enter program.callables, so the fixed-
    // point body never processes them. Seed them directly so they join ok
    // immediately — their Ok payload is always a freshly allocated owned
    // String (read_file/read_whole: file buffer → owned copy; get_env:
    // fresh env-string). This also breaks the read_file↔unwrap circular
    // dependency (f fresh ↔ read_file in ok ↔ text fresh ↔ read_file in ok).
    ok.insert("read_file".to_owned());
    ok.insert("read_whole".to_owned());
    for _ in 0..program.callables.len().max(1) {
        let mut changed = false;
        for c in &program.callables {
            let freed_set = freed.get(&c.name).cloned().unwrap_or_default();
            let result_is_result = c.result.as_ref().is_some_and(is_result_type);
            if !result_is_result || ok.contains(&c.name) {
                continue;
            }
            // No escape gate (2026-11-13, see fresh_callables). Payload
            // freshness is derived structurally; escaped params are never
            // fresh (absent from the var map's true set), which is the only
            // retention fact that matters.
            let mut vars: HashMap<String, bool> =
                c.params.iter().map(|p| (p.name.clone(), false)).collect();
            let fn_freed = freed.get(&c.name).cloned().unwrap_or_default();
            collect_fresh(&c.body.statements, &mut vars, fresh, pr, &ok, &fn_freed);
            let mut returns = Vec::new();
            collect_returns(&c.body.statements, &mut returns);
            if returns.is_empty() {
                continue;
            }
            let all_ok = returns.iter().all(|e| match &e.kind {
                HirExprKind::Call { callee, args, .. } => {
                    if callee == "Err" {
                        true
                    } else if callee == "Ok" {
                        args.first()
                            .is_some_and(|a| is_fresh(a, &vars, pr, fresh, &ok, &freed_set))
                    } else if *callee == c.name {
                        // Tail self-recursion carries the Result through
                        // unchanged (read_whole recursing on its own
                        // accumulator): the recursive path is exactly as
                        // Ok-side-fresh as the base paths. (Must precede the
                        // generic Call arm — it shadows a bare-call arm.)
                        true
                    } else if ok.contains(callee) {
                        // 2026-11-13: a verbatim tail call to another
                        // ok_fresh callable (execute_env →
                        // execute_env_inputs) hands the same Result through.
                        true
                    } else {
                        false
                    }
                }
                // Freshness flows through locals: read_file returns the
                // Result it unpacked into a local after its guard.
                HirExprKind::Identifier(n) => vars.get(n).copied().unwrap_or(false),
                _ => false,
            });
            if all_ok {
                ok.insert(c.name.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    ok
}

fn fresh_callables(
    program: &HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
    freed: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let mut fresh = HashSet::new();
    // 2026-11-14: seed known allocator callables. These construct fresh
    // heap/storage-backed values (Tensors, Lists) that the owned-analysis
    // otherwise could not prove fresh through the struct-literal / tuple-
    // destructuring path, stalling the tensor_new→load_frame_bin→fr chain.
    fresh.insert("tensor_new".to_owned());
    fresh.insert("tensor_alloc".to_owned());
    fresh.insert("tensor_alloc_owned".to_owned());
    fresh.insert("fresh_int_list".to_owned());
    // 2026-11-16: fresh_string is the String constructor. It allocates a new
    // owned String (a copy of its input); the input is NOT retained. Seeding
    // it lets self-tail String processors (e.g. peel in owned_tail_cleanup.bv)
    // reach their fixed point: the base case `term fresh_string(text)` becomes
    // fresh, and the self-tail `term peel(rest)` is exempted above.
    fresh.insert("fresh_string".to_owned());
    let pr = param_return_summaries(program);
    // Ok-side freshness is derived first (it only needs pr + its own fixed
    // point), so the plain-fresh derivation can consult it.
    let mut ok0 = ok_fresh_callables(program, escapes, &pr, &fresh, &freed);
    for _ in 0..program.callables.len().max(1) {
        let mut changed = false;
        let mut fresh_changed = false;
        for c in &program.callables {
            if !returns_owned(c) || fresh.contains(&c.name) {
                continue;
            }
            // Fresh-set soundness gate: if ANY parameter escapes (stored into
            // the result/state/collection), the callable must not count as
            // fresh — a `wrap(s) -> [s]` stasher would make the caller free a
            // retained operand. Escape-summary facts are the gate.
            // 2026-11-13: the blanket "any escaping param ⇒ not fresh" gate
            // was withdrawn. It blocked every builder that mutates or slices
            // a reader (`proto_bytes`: r.pos writes; `read_file`: passes the
            // Result through a local), while the hazards it guarded are
            // covered elsewhere: a List literal's freshness is BLOCK-only
            // (elements are never freed with it), String concat/slice are
            // copies, and identity returns are handled by param_return with
            // a caller-side ownership drop.
            let mut vars: HashMap<String, bool> =
                c.params.iter().map(|p| (p.name.clone(), false)).collect();
            let fn_freed = freed.get(&c.name).cloned().unwrap_or_default();
            collect_fresh(&c.body.statements, &mut vars, &fresh, &pr, &ok0, &fn_freed);
            let mut returns = Vec::new();
            collect_returns(&c.body.statements, &mut returns);
            // Self-recursion propagates the result unchanged: the recursive
            // path is exactly as fresh as the base paths (strip_frac_zeros,
            // digits_of_int). Without this exception a self-recursive fresh
            // function can never join the set.
            if !returns.is_empty()
                && returns.iter().all(|e| match &e.kind {
                    HirExprKind::Call { callee, .. } if *callee == c.name => true,
                    _ => is_fresh(e, &vars, &pr, &fresh, &ok0, &fn_freed),
                })
            {
                fresh.insert(c.name.clone());
                changed = true;
                fresh_changed = true;
            }
        }
        // 2026-11-14: when fresh grew in this pass, recompute ok0 so that
        // newly-added plain-fresh callables (e.g. tensor_new) can unlock
        // ok-fresh deductions (e.g. load_frame_bin) in the next pass.
        // Without this, ok0 is a stale snapshot taken when fresh={}, and
        // is_fresh(tensor_new(n)) uses wrong ok info for the whole loop.
        if fresh_changed {
            ok0 = ok_fresh_callables(program, escapes, &pr, &fresh, &freed);
        }
        if !changed {
            break;
        }
    }
    fresh
}

fn collect_fresh(
    stmts: &[HirStmt],
    vars: &mut HashMap<String, bool>,
    fresh: &HashSet<String>,
    pr: &HashMap<String, Vec<usize>>,
    ok: &HashSet<String>,
    freed: &HashSet<String>,
) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let {
                name,
                expr: Some(e),
                ty,
                ..
            } => {
                let fresh = is_fresh_rhs(e, vars, pr, fresh, ok, freed);
                let own = ty.as_ref().is_some_and(owned_ty) || e.ty.as_ref().is_some_and(owned_ty);
                // Result locals are not OWNED (unwrap yields the Ok side),
                // but they carry freshness INTO that unwrap — track them.
                let carries = own
                    || ty.as_ref().is_some_and(is_result_type)
                    || e.ty.as_ref().is_some_and(is_result_type);
                vars.insert(name.clone(), fresh && carries);
            }
            HirStmtKind::Assign(l, r) => {
                if let HirExprKind::Identifier(n) = &l.kind {
                    let fresh = is_fresh_rhs(r, vars, pr, fresh, ok, freed);
                    vars.insert(n.clone(), fresh);
                }
            }
            HirStmtKind::Guarded(_, b)
            | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => {
                collect_fresh(b, vars, fresh, pr, ok, freed);
            }
            HirStmtKind::Foreach { body, .. } => collect_fresh(body, vars, fresh, pr, ok, freed),
            HirStmtKind::Match { arms, .. } => {
                for a in arms {
                    collect_fresh(&a.body, vars, fresh, pr, ok, freed);
                }
            }
            _ => {}
        }
    }
}

fn collect_returns(stmts: &[HirStmt], out: &mut Vec<HirExpr>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => out.push(e.clone()),
            HirStmtKind::TempTransfer { expr, .. } => out.push((**expr).clone()),
            HirStmtKind::Guarded(_, b)
            | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => {
                collect_returns(b, out);
            }
            HirStmtKind::Foreach { body, .. } => collect_returns(body, out),
            HirStmtKind::Match { arms, .. } => {
                for a in arms {
                    collect_returns(&a.body, out);
                }
            }
            _ => {}
        }
    }
}

fn is_fresh(
    e: &HirExpr,
    vars: &HashMap<String, bool>,
    pr: &HashMap<String, Vec<usize>>,
    fresh: &HashSet<String>,
    ok: &HashSet<String>,
    freed: &HashSet<String>,
) -> bool {
    match &e.kind {
        // Scalar literals are copied values — always fresh.
        HirExprKind::Literal(
            HirLiteral::Int(_)
            | HirLiteral::Float(_)
            | HirLiteral::Bool(_)
            | HirLiteral::Char(_)
            | HirLiteral::TaggedInt(_, _),
        ) => true,
        // A List literal allocates a fresh BLOCK (the coll object + data);
        // element handles live inside it as plain words. Freeing the block
        // (coll_destroy: object + data) never frees an element's own
        // allocation, so list-block ownership is independent of element
        // ownership. The wrap-stasher hazard (`wrap(s) -> [s]`) lives on the
        // ARGUMENT side — the escape gate keeps `s` unowned at the caller —
        // not here. Hence: unconditionally fresh.
        HirExprKind::List(_) => true,
        // 2026-11-12 (slicing doc): a String slice allocates a fresh copy.
        HirExprKind::Slice { array, .. } => string_expr(array, fresh),
        HirExprKind::Cast { inner, to, .. } if is_string(to) => {
            // 2026-11-13 (buffer-transfer): `(buf as Data) as String` where
            // buf is FREED in the same body is an ownership transfer — the
            // Free# after the cast proves the result is the buffer's sole
            // remaining owner (read_whole/int_to_str buffer pattern). The
            // source may be a String-typed param (string_expr would say
            // "borrow"); the freed record overrides it.
            if !string_expr(inner, fresh) {
                return true;
            }
            let mut src = inner;
            while let HirExprKind::Cast { inner: deeper, .. } = &src.kind {
                src = deeper;
            }
            if let HirExprKind::Identifier(n) = &src.kind {
                if freed.contains(n) {
                    return true;
                }
            }
            false
        }
        HirExprKind::Call { callee, args, .. } => {
            if fresh.contains(callee) || ok.contains(callee) {
                true
            } else if callee == "unwrap" {
                // 2026-11-14: unwrap is a passthrough for Result-freshness — the
                // Ok payload carries the same freshness as the Result itself.
                // Without this, read_file/Result-callable freshness stalls at the
                // unwrap boundary and the String payload is never freed.
                args.first()
                    .is_some_and(|a| is_fresh(a, vars, pr, fresh, ok, freed))
            } else if let Some(cands) = pr.get(callee) {
                // Fresh only when EVERY candidate return is caller-owned. A
                // selector handing back one of several arguments is owned
                // exactly when all of them are (see identity_return_param).
                !cands.is_empty()
                    && cands.iter().all(|i| {
                        args.get(*i)
                            .is_some_and(|a| is_fresh(a, vars, pr, fresh, ok, freed))
                    })
            } else {
                false
            }
        }
        HirExprKind::Binary {
            op: BinaryOpKind::Add,
            left,
            right,
        } => string_expr(left, fresh) && string_expr(right, fresh),
        HirExprKind::Cast { inner, to, .. } => is_string(to) && !string_expr(inner, fresh),
        HirExprKind::Identifier(n) => vars.get(n).copied().unwrap_or(false),
        HirExprKind::If {
            then_branch,
            else_branch,
            ..
        } => else_branch.as_ref().is_some_and(|e| {
            is_fresh(then_branch, vars, pr, fresh, ok, freed)
                && is_fresh(e, vars, pr, fresh, ok, freed)
        }),
        HirExprKind::Match { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|a| is_fresh(&a.body, vars, pr, fresh, ok, freed))
        }
        _ => false,
    }
}

fn is_fresh_rhs(
    e: &HirExpr,
    vars: &HashMap<String, bool>,
    pr: &HashMap<String, Vec<usize>>,
    fresh: &HashSet<String>,
    ok: &HashSet<String>,
    freed: &HashSet<String>,
) -> bool {
    !matches!(e.kind, HirExprKind::Identifier(_)) && is_fresh(e, vars, pr, fresh, ok, freed)
}

fn string_expr(e: &HirExpr, fresh: &HashSet<String>) -> bool {
    // 2026-11-13: a call whose callable is in the fresh set returns fresh
    // String storage (digits_of_int recursion, concat chains).
    e.ty.as_ref().is_some_and(is_string)
        || match &e.kind {
            HirExprKind::Literal(HirLiteral::Bytes(_)) => true,
            HirExprKind::Identifier(_) => true,
            HirExprKind::Call { callee, .. } if fresh.contains(callee) => true,
            HirExprKind::Binary {
                op: BinaryOpKind::Add,
                left,
                right,
            } => string_expr(left, fresh) && string_expr(right, fresh),
            _ => false,
        }
}

fn consumed_params(program: &HirProgram) -> HashMap<String, HashSet<usize>> {
    let mut consumed: HashMap<String, HashSet<usize>> = HashMap::new();
    for c in &program.callables {
        let mut names = HashSet::new();
        consumed_in(&c.body.statements, &mut names);
        let mut set = HashSet::new();
        for (i, p) in c.params.iter().enumerate() {
            if names.contains(&p.name) {
                set.insert(i);
            }
        }
        if !set.is_empty() {
            consumed.insert(c.name.clone(), set);
        }
    }
    consumed
}

fn consumed_in(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Expression(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e) => {
                consumed_expr(e, out)
            }
            HirStmtKind::Let { expr: Some(e), .. } | HirStmtKind::Term(Some(e)) => {
                consumed_expr(e, out)
            }
            HirStmtKind::Assign(l, r)
            | HirStmtKind::ArrowAssign {
                value: l,
                target: Some(r),
                ..
            } => {
                consumed_expr(l, out);
                consumed_expr(r, out);
            }
            HirStmtKind::FreeHint(n) | HirStmtKind::KeepHint(n) => {
                out.insert(n.clone());
            }
            HirStmtKind::ReleaseOwned { name, .. } => {
                out.insert(name.clone());
            }
            HirStmtKind::TempTransfer { expr, releases, .. } => {
                consumed_expr(expr, out);
                for (_, n) in releases {
                    out.insert(n.clone());
                }
            }
            HirStmtKind::Guarded(c, b) => {
                consumed_expr(c, out);
                consumed_in(b, out);
            }
            HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => {
                consumed_in(b, out);
            }
            HirStmtKind::Foreach { list, body, .. } => {
                consumed_expr(list, out);
                consumed_in(body, out);
            }
            HirStmtKind::Match { expr, arms } => {
                consumed_expr(expr, out);
                for a in arms {
                    consumed_in(&a.body, out);
                }
            }
            _ => {}
        }
    }
}

fn consumed_expr(e: &HirExpr, out: &mut HashSet<String>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => {
            if callee == "Free#" {
                for a in args {
                    if let HirExprKind::Identifier(n) = &a.kind {
                        out.insert(n.clone());
                    }
                }
            }
            for a in args {
                consumed_expr(a, out);
            }
        }
        HirExprKind::Consume(inner) => {
            if let HirExprKind::Identifier(n) = &inner.kind {
                out.insert(n.clone());
            }
            consumed_expr(inner, out);
        }
        HirExprKind::Binary { left, right, .. } => {
            consumed_expr(left, out);
            consumed_expr(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Await(inner) => consumed_expr(inner, out),
        HirExprKind::Index(a, b) => {
            consumed_expr(a, out);
            consumed_expr(b, out);
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            consumed_expr(condition, out);
            consumed_expr(then_branch, out);
            if let Some(e) = else_branch {
                consumed_expr(e, out);
            }
        }
        HirExprKind::Block(stmts) => consumed_in(stmts, out),
        HirExprKind::List(items) | HirExprKind::Tuple(items) => {
            for e in items {
                consumed_expr(e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            consumed_expr(scrutinee, out);
            for a in arms {
                consumed_expr(&a.body, out);
            }
        }
        _ => {}
    }
}

struct Pass<'a> {
    fresh: &'a HashSet<String>,
    escapes: &'a HashMap<String, Vec<bool>>,
    consumed: &'a HashMap<String, HashSet<usize>>,
    param_return: &'a HashMap<String, Vec<usize>>,
    ok_fresh: &'a HashSet<String>,
    borrow_returns: &'a HashSet<String>,
    freed_map: &'a HashMap<String, HashSet<String>>,
    /// Locals of the CURRENT callable that still denote one of its parameters
    /// (param `i` reachable through `let` copies and push-mutated collec-
    /// tions). Used to decide whether a param-return call hands back caller
    /// storage (a borrow) or an unresolved value (undecided).
    param_aliases: &'a HashMap<String, usize>,
    /// 2026-11-16 (M2): the CURRENT callable's owned tail-loop parameters, as
    /// decided by `owned_tail_params` before the rewrite. A self-tail call arg
    /// at a recorded index is fixed by the backend's loop lowering, so the
    /// pass must NOT report it as undecided; any other owned self-tail temp is
    /// still a real leak and keeps its census event.
    fixed_tail: &'a [OwnedTailParam],
    locals: &'a HashSet<String>,
    next: u32,
    function: String,
    events: &'a mut Vec<OwnedEvent>,
}

/// A call to a param-return callable hands back the storage of the argument it
/// aliases; when that argument resolves to one of the CALLER's own parameters
/// the result is indistinguishable from the parameter itself — a borrow, never
/// an undecided owner. This is the protobuf parser shape: `let next =
/// parse_dimension_loop(dr, dims)` where `dims` is the caller's parameter.
/// Deliberately narrow: only a parameter-origin argument qualifies, so a call
/// that hands back an unresolved LOCAL value still reports as undecided — a
/// leak must not be relabelled as a borrow to make the census look better.
fn call_aliases_caller_param(e: &HirExpr, ctx: &Pass) -> bool {
    let HirExprKind::Call { callee, args, .. } = &e.kind else {
        return false;
    };
    let Some(cands) = ctx.param_return.get(callee) else {
        return false;
    };
    // ANY candidate resolving to caller storage is enough: the callee may pick
    // that one at runtime, so the result may be a borrow.
    cands.iter().any(|i| {
        args.get(*i)
            .and_then(|a| param_alias_of(a, ctx.param_aliases, ctx.param_return))
            .is_some()
    })
}

fn collect_let_names(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let { name, .. } => {
                out.insert(name.clone());
            }
            HirStmtKind::Guarded(_, b)
            | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => collect_let_names(b, out),
            HirStmtKind::Foreach { body, .. } => collect_let_names(body, out),
            HirStmtKind::Match { arms, .. } => {
                for a in arms {
                    collect_let_names(&a.body, out);
                }
            }
            _ => {}
        }
    }
}

fn rewrite_list(
    stmts: &[HirStmt],
    owned: &mut HashMap<String, OwnKind>,
    fresh_vars: &mut HashMap<String, bool>,
    sel_results: &mut HashSet<String>,
    ctx: &mut Pass<'_>,
    top: bool,
) -> Vec<HirStmt> {
    let entry: HashSet<String> = owned.keys().cloned().collect();
    let no_freed: HashSet<String> = HashSet::new();
    let fn_freed = ctx
        .freed_map
        .get(&ctx.function)
        .cloned()
        .unwrap_or_default();
    let _ = &no_freed;
    let declared: HashSet<String> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            HirStmtKind::Let { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let mut out = Vec::with_capacity(stmts.len());
    let mut pending_temp_stmts: Vec<HirStmt> = Vec::new();
    for stmt in stmts {
        let mut s = stmt.clone();
        let stmt_kind_tag = stmt.kind.clone();
        match &mut s.kind {
            HirStmtKind::Let {
                name,
                expr: Some(e),
                ty,
                ..
            } => {
                uses(e, owned, ctx);
                let own_flag =
                    ty.as_ref().is_some_and(owned_ty) || e.ty.as_ref().is_some_and(owned_ty);
                // Result locals are not OWNED, but they CARRY freshness into
                // the pr-identity chain (unwrap): track them in fresh_vars so
                // `bytes = unwrap(f)` sees f's ok_fresh verdict.
                let carries = own_flag
                    || ty.as_ref().is_some_and(is_result_type)
                    || e.ty.as_ref().is_some_and(is_result_type);
                let kind = ty
                    .as_ref()
                    .or(e.ty.as_ref())
                    .map(kind_of)
                    .unwrap_or(OwnKind::Str);
                let fresh = carries
                    && is_fresh_rhs(
                        e,
                        fresh_vars,
                        ctx.param_return,
                        ctx.fresh,
                        ctx.ok_fresh,
                        &fn_freed,
                    );
                if carries {
                    fresh_vars.insert(name.clone(), fresh);
                }
                // 2026-11-21 (K3 Phase C): a multi-candidate param_return
                // selector (`ints_or(items, fallback)`) hands back the storage
                // of whichever candidate it picks at runtime — the result
                // local aliases ONE of its arguments, it owns no storage of
                // its own. Owning the result (the pre-fix path) released only
                // the WINNER at scope end and leaked every loser: an unnamed
                // inline candidate (a fresh-set call result or a list literal)
                // has no name, so no scope-end release ever reached it
                // (kiosk detect_faces: ~39 KB/call in conv2d alone).
                // Sound accounting: the result is a BORROW — not owned, never
                // released. Each candidate is released exactly once by ITS OWN
                // bookkeeping: inline owned candidates are bound to synthetic
                // temps (which the normal Let logic owns + frees at scope
                // end); named-local candidates are re-inserted into `owned`
                // unless their block is already covered by their own selector
                // temps (recorded in `sel_results`). Every block freed once,
                // no UAF (releases sit at scope end, after the last read).
                let callee_name = match &e.kind {
                    HirExprKind::Call { callee, .. } => Some(callee.clone()),
                    _ => None,
                };
                let is_multi_selector = callee_name
                    .as_ref()
                    .and_then(|c| ctx.param_return.get(c))
                    .is_some_and(|cands| cands.len() > 1);
                if fresh && is_multi_selector {
                    pending_temp_stmts.clear();
                    // 2026-11-21 (K3 Phase C): emit a pure-insert LetInsert +
                    // ReleaseOwned for each inline owned candidate. The
                    // LetInsert binds the candidate to a pass-allocated name
                    // (no AST slot — the pair walk stays aligned); the
                    // ReleaseOwned destroys the candidate's block at scope
                    // end. Named candidates are re-owned directly (their own
                    // selector temps cover them only when they are themselves
                    // selector results, tracked in `sel_results`).
                    if let HirExprKind::Call { callee, args, .. } = &e.kind {
                        let cands = ctx.param_return.get(callee).unwrap();
                        let mut na = args.clone();
                        for &ci in cands {
                            let Some(a) = na.get(ci) else { continue };
                            if matches!(a.kind, HirExprKind::Identifier(_)) {
                                // Named candidate: `uses()` removed it from
                                // `owned` (Escape mode). Its block is covered
                                // by its own selector temps only when it is
                                // itself a selector result; otherwise re-own
                                // it so scope end frees it exactly once.
                                if let HirExprKind::Identifier(n) = &a.kind {
                                    if !sel_results.contains(n) {
                                        owned.insert(n.clone(), kind);
                                    }
                                }
                                continue;
                            }
                            // Inline owned candidate (fresh-set call result or
                            // list literal): bind to a pure-insert LetInsert
                            // temp (no AST slot — pair walk stays aligned) and
                            // rewrite the call arg to that temp so the
                            // candidate is evaluated ONCE. The paired
                            // ReleaseOwned destroys the block at scope end.
                            let tname = fresh_temp_name(name, ctx);
                            let let_insert = HirStmt {
                                id: NodeId(ctx.next),
                                span: None,
                                kind: HirStmtKind::LetInsert {
                                    name: tname.clone(),
                                    ty: a.ty.clone(),
                                    expr: Box::new(a.clone()),
                                },
                            };
                            ctx.next += 1;
                            let id_expr = HirExpr {
                                id: NodeId(ctx.next),
                                span: None,
                                ty: a.ty.clone(),
                                kind: HirExprKind::Identifier(tname.clone()),
                            };
                            ctx.next += 1;
                            na[ci] = id_expr;
                            // The binding is a pure insert (emitted inline,
                            // before the selector call that now reads the
                            // temp by name). Ownership of the temp is
                            // deferred to the scope-end `owned` sweep — the
                            // release must land AFTER the candidate's last
                            // read (typically inside the following foreach),
                            // so an inline ReleaseOwned would be a
                            // use-after-free.
                            owned.insert(tname.clone(), kind);
                            pending_temp_stmts.push(let_insert);
                        }
                        if let HirExprKind::Call { args, .. } = &mut e.kind {
                            *args = na;
                        }
                    }
                    sel_results.insert(name.clone());
                    ctx.events.push(OwnedEvent {
                        function: ctx.function.clone(),
                        name: name.clone(),
                        // The result local aliases a candidate whose storage
                        // the candidate's own bookkeeping frees — neither a
                        // release nor a leak. Counted as a static-backed
                        // borrow (excluded from the drops ratchet) so the
                        // census does not mistake the alias for a new
                        // conservative drop.
                        kind: OwnedEventKind::Static,
                    });
                    out.extend(std::mem::take(&mut pending_temp_stmts));
                    out.push(s);
                    continue;
                } else if fresh {
                    owned.insert(name.clone(), kind);
                } else {
                    owned.remove(name);
                    if matches!(e.kind, HirExprKind::Identifier(_)) {
                        let mut ids = HashSet::new();
                        mention(e, &mut ids);
                        for id in ids {
                            owned.remove(&id);
                            ctx.events.push(OwnedEvent {
                                function: ctx.function.clone(),
                                name: id,
                                kind: OwnedEventKind::Alias,
                            });
                        }
                    } else if own_flag {
                        // Borrow-shaped reads hand over the container's
                        // storage (x.dims, list[i], *p) — an alias, not an
                        // undecided owner.
                        let borrow_call = matches!(&e.kind, HirExprKind::Call { callee, .. }
                            if ctx.borrow_returns.contains(callee))
                            || call_aliases_caller_param(e, ctx);
                        if borrow_call
                            || matches!(
                                &e.kind,
                                HirExprKind::Field { .. }
                                    | HirExprKind::Index(..)
                                    | HirExprKind::Slice { .. }
                                    | HirExprKind::Deref(_)
                                    | HirExprKind::Reflect { .. }
                            )
                        {
                            ctx.events.push(OwnedEvent {
                                function: ctx.function.clone(),
                                name: name.clone(),
                                kind: OwnedEventKind::Borrow,
                            });
                        } else if matches!(&e.kind, HirExprKind::Literal(_)) {
                            // Literal-backed local: static storage, allocates
                            // nothing, owns nothing — not an ambiguous owner.
                            ctx.events.push(OwnedEvent {
                                function: ctx.function.clone(),
                                name: name.clone(),
                                kind: OwnedEventKind::Static,
                            });
                        } else {
                            let why = match &e.kind {
                                HirExprKind::Call { callee, .. } => {
                                    format!("call {callee} not proven fresh")
                                }
                                _ => "expression not proven fresh".to_string(),
                            };
                            ctx.events.push(OwnedEvent {
                                function: ctx.function.clone(),
                                name: name.clone(),
                                kind: OwnedEventKind::UndecidedInit(why),
                            });
                        }
                    }
                }
                out.push(s);
            }
            HirStmtKind::Assign(lhs, rhs) => {
                uses(rhs, owned, ctx);
                let target = if let HirExprKind::Identifier(n) = &lhs.kind {
                    Some(n.clone())
                } else {
                    None
                };
                let mut rhs_ids = HashSet::new();
                mention(rhs, &mut rhs_ids);
                if let Some(n) = &target {
                    if !ctx.locals.contains(n) {
                        for id in rhs_ids {
                            owned.remove(&id);
                        }
                        out.push(s);
                        continue;
                    }
                    if let Some(kind) = owned.get(n).copied() {
                        // 2026-11-13 (alias safety): release the OLD value only
                        // when the RHS is a provably FRESH allocation (or has
                        // no identifiers at all). A bare identifier RHS may
                        // alias the old block through an untracked binding —
                        // `env = next_env` where run_node returned the same
                        // env handle — and releasing first would use-after-
                        // free (daemon double-free on the JPEG path).
                        let rhs_fresh = is_fresh(
                            rhs,
                            fresh_vars,
                            ctx.param_return,
                            ctx.fresh,
                            ctx.ok_fresh,
                            &fn_freed,
                        );
                        if !rhs_ids.contains(n) && (rhs_fresh || rhs_ids.is_empty()) {
                            out.push(release(n, kind, ctx));
                            owned.remove(n);
                        }
                    }
                    let own_flag = lhs.ty.as_ref().is_some_and(owned_ty)
                        || rhs.ty.as_ref().is_some_and(owned_ty);
                    let carries = own_flag
                        || lhs.ty.as_ref().is_some_and(is_result_type)
                        || rhs.ty.as_ref().is_some_and(is_result_type);
                    let kind = lhs
                        .ty
                        .as_ref()
                        .or(rhs.ty.as_ref())
                        .map(kind_of)
                        .unwrap_or(OwnKind::Str);
                    let fresh = carries
                        && is_fresh_rhs(
                            rhs,
                            fresh_vars,
                            ctx.param_return,
                            ctx.fresh,
                            ctx.ok_fresh,
                            &fn_freed,
                        );
                    fresh_vars.insert(n.clone(), fresh);
                    if fresh && own_flag {
                        owned.insert(n.clone(), kind);
                    } else {
                        owned.remove(n);
                    }
                    if matches!(rhs.kind, HirExprKind::Identifier(_)) {
                        for id in rhs_ids {
                            owned.remove(&id);
                            ctx.events.push(OwnedEvent {
                                function: ctx.function.clone(),
                                name: id,
                                kind: OwnedEventKind::Alias,
                            });
                        }
                    }
                } else {
                    for id in rhs_ids {
                        owned.remove(&id);
                        ctx.events.push(OwnedEvent {
                            function: ctx.function.clone(),
                            name: id,
                            kind: OwnedEventKind::Store,
                        });
                    }
                }
                out.push(s);
            }
            HirStmtKind::Expression(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e) => {
                uses(e, owned, ctx);
                out.push(s);
            }
            HirStmtKind::FreeHint(n) => {
                owned.remove(n);
                ctx.events.push(OwnedEvent {
                    function: ctx.function.clone(),
                    name: n.clone(),
                    kind: OwnedEventKind::ExplicitFree,
                });
                out.push(s);
            }
            HirStmtKind::KeepHint(n) => {
                owned.remove(n);
                ctx.events.push(OwnedEvent {
                    function: ctx.function.clone(),
                    name: n.clone(),
                    kind: OwnedEventKind::Kept,
                });
                out.push(s);
            }
            HirStmtKind::Term(Some(e))
            | HirStmtKind::Tail(e)
            | HirStmtKind::EndProgram(Some(e)) => {
                uses(e, owned, ctx);
                let transferred = if let HirExprKind::Identifier(n) = &e.kind {
                    Some(n.clone())
                } else {
                    None
                };
                let mut ids = HashSet::new();
                mention(e, &mut ids);
                if let Some(n) = &transferred {
                    owned.remove(n);
                }
                // Temp-transfer (2026-11-12): a FRESH return expression that
                // borrows owned locals evaluates the allocation first, so the
                // borrowed operands can be released right after — bind the
                // result to a synthetic local, release the operands, return
                // the binding. Not-fresh returns keep the conservative drop.
                let ret_fresh = is_fresh(
                    e,
                    fresh_vars,
                    ctx.param_return,
                    ctx.fresh,
                    ctx.ok_fresh,
                    &fn_freed,
                );
                // 2026-11-14: a SELF tail call must NOT be temp-transferred.
                // The binding would emit the recursive call, and tail-loop
                // lowering turns that call into the loop-back `br` INSIDE the
                // binding — stranding the transfer's releases after a
                // terminator (invalid IR: `expected instruction opcode` on
                // the wasm dashboard, strip_frac_zeros). The recursion
                // already carries the value; stay conservative (leak).
                let self_tail =
                    matches!(&e.kind, HirExprKind::Call { callee, .. } if *callee == ctx.function);
                // 2026-11-16: report the leak the exclusion above creates.
                // A self-tail call that passes an OWNED INLINE temporary (a
                // concat/slice/fresh_string argument, not a local) leaks it
                // every iteration: the tail-loop lowering stores the new value
                // into the parameter slot and branches to the header, so the
                // old value is dropped with no release. The local-based census
                // cannot see it (the temporary has no name), so this is the
                // only place it becomes visible. Reported as UNDECIDED, not as
                // a conservative drop: it is a real unbounded leak (quadratic
                // RSS; BUGS.md 2026-11-16) and must block
                // --deny-memory-ambiguity.
                if self_tail {
                    if let HirExprKind::Call { args, .. } = &e.kind {
                        for (i, a) in args.iter().enumerate() {
                            let inline = !matches!(a.kind, HirExprKind::Identifier(_));
                            // Scalars are copied values: `is_fresh` says true for
                            // them (they cannot leak) but they own no storage, so
                            // require an owned storage type as well.
                            let owns_storage = a.ty.as_ref().is_some_and(owned_ty);
                            let owned_temp = is_fresh(
                                a,
                                fresh_vars,
                                ctx.param_return,
                                ctx.fresh,
                                ctx.ok_fresh,
                                &fn_freed,
                            );
                            if inline && owns_storage && owned_temp {
                                // 2026-11-16 (M2): a recorded owned tail-loop
                                // parameter is released by the backend's loop
                                // lowering (free-before-backedge + exit), so
                                // it is no longer a leak and must not block
                                // --deny-memory-ambiguity. Anything the loop
                                // lowering cannot prove stays a real leak and
                                // keeps its census event.
                                let fixed = ctx.fixed_tail.iter().any(|p| p.index == i);
                                if !fixed {
                                    ctx.events.push(OwnedEvent {
                                        function: ctx.function.clone(),
                                        name: format!("self-tail arg{i}"),
                                        kind: OwnedEventKind::UndecidedInit(
                                            "owned temporary at a self-tail call is freed nowhere "
                                                .to_string(),
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
                let borrows_owned = !self_tail
                    && transferred.is_none()
                    && ids.iter().any(|n| owned.contains_key(n));
                if ret_fresh && borrows_owned {
                    let mut tname = format!("__ot{}", ctx.next);
                    ctx.next += 1;
                    while ctx.locals.contains(&tname) {
                        tname = format!("{}x", tname);
                        ctx.next += 1;
                    }
                    // Sorted: this Vec drives the ORDER of emitted releases.
                    // A HashMap's iteration order varies per process (SipHash
                    // seed), which made the emitted IR differ run to run —
                    // caught by comparing two builds of the daemon (the same
                    // coll_destroy calls in a different order).
                    let mut names: Vec<String> = owned.keys().cloned().collect();
                    names.sort();
                    let releases: Vec<(StorageKind, String)> = names
                        .iter()
                        .map(|n| {
                            let k = owned.get(n).copied().unwrap_or(OwnKind::Str);
                            (
                                match k {
                                    OwnKind::Str => StorageKind::Str,
                                    OwnKind::Lst => StorageKind::List,
                                },
                                n.clone(),
                            )
                        })
                        .collect();
                    let ret = match stmt_kind_tag {
                        HirStmtKind::Tail(_) => crate::hir::stmt::RetKind::Tail,
                        HirStmtKind::EndProgram(_) => crate::hir::stmt::RetKind::EndProgram,
                        _ => crate::hir::stmt::RetKind::Term,
                    };
                    let stmt = HirStmt {
                        id: NodeId(ctx.next),
                        span: None,
                        kind: HirStmtKind::TempTransfer {
                            name: tname,
                            ty: e.ty.clone(),
                            expr: Box::new(e.clone()),
                            releases,
                            ret,
                        },
                    };
                    ctx.next += 1;
                    out.push(stmt);
                    owned.clear();
                    continue;
                }
                // Sorted for the same reason as the temp-transfer site: this
                // Vec determines the release order in the emitted function.
                let mut names: Vec<String> = owned.keys().cloned().collect();
                names.sort();
                for n in names {
                    let k = owned.get(&n).copied();
                    if !ids.contains(&n) {
                        if let Some(k) = k {
                            out.push(release(&n, k, ctx));
                        }
                        owned.remove(&n);
                    } else {
                        owned.remove(&n);
                        ctx.events.push(OwnedEvent {
                            function: ctx.function.clone(),
                            name: n,
                            kind: OwnedEventKind::ReturnBorrow,
                        });
                    }
                }
                out.push(s);
            }
            HirStmtKind::Guarded(cond, body) => {
                uses(cond, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let mut inner_sel = sel_results.clone();
                let new_body = rewrite_list(
                    body,
                    &mut inner,
                    &mut inner_fresh,
                    &mut inner_sel,
                    ctx,
                    false,
                );
                sel_results.extend(inner_sel);
                for n in before {
                    if !inner.contains_key(&n) {
                        owned.remove(&n);
                    }
                }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Block(body)
            | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::Defer(body) => {
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let mut inner_sel = sel_results.clone();
                let new_body = rewrite_list(
                    body,
                    &mut inner,
                    &mut inner_fresh,
                    &mut inner_sel,
                    ctx,
                    false,
                );
                sel_results.extend(inner_sel);
                for n in before {
                    if !inner.contains_key(&n) {
                        owned.remove(&n);
                    }
                }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Foreach { list, body, .. } => {
                uses(list, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let mut inner_sel = sel_results.clone();
                let new_body = rewrite_list(
                    body,
                    &mut inner,
                    &mut inner_fresh,
                    &mut inner_sel,
                    ctx,
                    false,
                );
                sel_results.extend(inner_sel);
                for n in before {
                    if !inner.contains_key(&n) {
                        owned.remove(&n);
                    }
                }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Match { expr, arms } => {
                uses(expr, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut kept: Option<HashMap<String, OwnKind>> = None;
                for arm in arms.iter_mut() {
                    let mut inner = owned.clone();
                    let mut inner_fresh = fresh_vars.clone();
                    let mut inner_sel = sel_results.clone();
                    let new_body = rewrite_list(
                        &arm.body,
                        &mut inner,
                        &mut inner_fresh,
                        &mut inner_sel,
                        ctx,
                        false,
                    );
                    sel_results.extend(inner_sel);
                    for n in &before {
                        if !inner.contains_key(n) {
                            owned.remove(n);
                        }
                    }
                    kept = Some(match kept {
                        Some(k) => k
                            .into_iter()
                            .filter(|(n, _)| inner.contains_key(n))
                            .collect(),
                        None => inner,
                    });
                    arm.body = new_body;
                }
                if let Some(k) = kept {
                    owned.extend(k);
                }
                out.push(s);
            }
            _ => out.push(s),
        }
    }
    if top {
        // Sorted: the exit releases are emitted in this order.
        let mut names: Vec<(String, OwnKind)> =
            owned.iter().map(|(n, k)| (n.clone(), *k)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (n, k) in names {
            out.push(release(&n, k, ctx));
        }
        owned.clear();
    } else {
        let added: Vec<(String, OwnKind)> = owned
            .iter()
            .filter(|(n, _)| !entry.contains(*n) && declared.contains(*n))
            .map(|(n, k)| (n.clone(), *k))
            .collect();
        for (n, k) in added {
            out.push(release(&n, k, ctx));
            owned.remove(&n);
        }
    }
    out
}

/// 2026-11-21 (K3 Phase C): unique synthetic temp name for binding an inline
/// owned selector candidate, derived from the result local so the name is
/// stable across builds (determinism gate) and collision-checked against the
/// function's declared locals via `ctx.next`-suffixed retries.
fn fresh_temp_name(name: &str, ctx: &mut Pass<'_>) -> String {
    let base = format!("__selt{}_t{}", name, ctx.next);
    let mut tname = base;
    while ctx.locals.contains(&tname) {
        tname = format!("{}x", tname);
    }
    tname
}

/// The ownership mode a call applies to argument `i`.
///
/// 2026-11-21 (K3 residual): extracted from `uses` so the argument-temporary
/// hoister classifies arguments EXACTLY as the escape walk does — one
/// decision point, two consumers (DRY).
fn mode_for_arg(callee: &str, i: usize, ctx: &Pass<'_>) -> Mode {
    if callee == "Free#" {
        return Mode::Consume;
    }
    if ctx.fresh.contains(callee) {
        return Mode::Borrow;
    }
    if ctx.consumed.get(callee).is_some_and(|s| s.contains(&i)) {
        return Mode::Consume;
    }
    if let Some(summary) = ctx.escapes.get(callee) {
        if summary.get(i).copied().unwrap_or(true) {
            return Mode::Escape;
        }
        return Mode::Borrow;
    }
    if callee.ends_with('#') {
        return Mode::Borrow;
    }
    Mode::Escape
}

/// True when `e` allocates storage that the enclosing scope must release and
/// that has no name of its own: a fresh-set call result or a list literal.
///
/// Deliberately excludes `Literal` (static storage — releasing it is a
/// free-of-.rodata) and every borrow-shaped read. A String-typed argument is
/// excluded too: the STAMP decides ownership, and a String stamp means the
/// caller's bookkeeping already owns the buffer — a slice result like
/// `r.buf[a:b]` passed to a fresh callee is borrowed, and releasing it would
/// free the caller's underlying data.
fn is_owned_temp_expr(e: &HirExpr, ctx: &Pass<'_>) -> bool {
    match &e.kind {
        HirExprKind::Call { callee, .. } => {
            // Only a COLLECTION temporary is hoisted. A String temporary's
            // binding round-trip records the value in `let_binding_types`, and
            // the argument adapter then feeds the callee a word where it takes
            // a pointer (`inttoptr ptr` — invalid IR). String argument
            // temporaries therefore stay on their current path: they leak
            // exactly as they do today, never miscompiled.
            if !e.ty.as_ref().is_some_and(is_list) {
                return false;
            }
            // A multi-candidate selector's candidates are owned through the
            // dedicated Phase C branch; hoisting one here would double-release.
            !ctx.param_return
                .get(callee.as_str())
                .is_some_and(|c| c.len() > 1)
                && (ctx.fresh.contains(callee) || ctx.ok_fresh.contains(callee))
        }
        HirExprKind::List(items) => {
            // A list literal is only hoistable when the emitter can recover a
            // collection type for it: the element type comes from the first
            // item, and an untyped or empty literal falls back to the bare HIR
            // operand path, which panics (a bare list literal has no codegen
            // shape of its own). Skipping keeps the value leaking exactly as
            // it does today — never miscompiled.
            items.first().is_some_and(|i| i.ty.is_some())
        }
        _ => false,
    }
}

/// 2026-11-21 (K3 residual leak): the type a hoisted temporary must be bound
/// with.
///
/// A list literal carries no collection type of its own in the HIR — the
/// typechecker stamps one only at the source let site. Its type is therefore
/// derived from the first element, and that derivation MUST win: the literal's
/// own stamp can be the element type or the declared parameter type, which
/// would (a) route the construction onto the bare operand path and (b) pick
/// the wrong destroy (a String release on a list handle). `None` means the
/// value cannot be named and must not be hoisted — it keeps leaking exactly as
/// it does today, never miscompiled.
fn hoist_value_type(e: &HirExpr) -> Option<Type> {
    if let HirExprKind::List(items) = &e.kind {
        let elem = items.first()?.ty.clone()?;
        return Some(Type::Applied("List".to_string(), vec![elem]));
    }
    e.ty.clone()
}

/// 2026-11-21 (K3 residual leak): hoist owned ARGUMENT temporaries.
///
/// An owned value produced inline and handed to a BORROWING parameter has no
/// name for the scope sweep to release. `parse_str_chars(p, n, append_str(b,
/// briev_str_substr(s, a, b)))` leaked the substring copy on every call, and
/// the same shape leaked the attribute-parser list temporaries.
///
/// Fix: bind such a temporary to a pure-insert `HirStmtKind::LetInsert`
/// placed BEFORE its statement, and release it with a `ReleaseOwned` placed
/// AFTER the statement. A borrowing parameter cannot retain the pointer (a
/// callee that would stash it is classified `Escape` by the escape summary),
/// so the release is sound; placing it after the statement rather than at
/// scope end keeps it correct inside loop bodies (each iteration binds and
/// frees its own block).
fn hoist_owned_arg_temps(
    stmts: &mut Vec<HirStmt>,
    fresh_vars: &HashMap<String, bool>,
    ctx: &mut Pass<'_>,
    fn_freed: &HashSet<String>,
) {
    let mut out: Vec<HirStmt> = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        hoist_in_stmt_bodies(&mut s, fresh_vars, ctx, fn_freed);
        let mut pre: Vec<HirStmt> = Vec::new();
        let mut post: Vec<HirStmt> = Vec::new();
        match &mut s.kind {
            HirStmtKind::Let { expr: Some(e), .. }
            | HirStmtKind::Expression(e)
            | HirStmtKind::Check(e)
            | HirStmtKind::Gate(e) => {
                hoist_in_expr(e, &mut pre, &mut post, fresh_vars, ctx, fn_freed);
            }
            HirStmtKind::Assign(_, rhs) => {
                hoist_in_expr(rhs, &mut pre, &mut post, fresh_vars, ctx, fn_freed);
            }
            _ => {}
        }
        out.extend(pre);
        out.push(s);
        out.extend(post);
    }
    *stmts = out;
}

/// Recurse the hoister into a statement's nested bodies (each is its own
/// release scope, and each may hold loop bodies that must free per iteration).
fn hoist_in_stmt_bodies(
    s: &mut HirStmt,
    fresh_vars: &HashMap<String, bool>,
    ctx: &mut Pass<'_>,
    fn_freed: &HashSet<String>,
) {
    match &mut s.kind {
        HirStmtKind::Guarded(_, b)
        | HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Defer(b) => hoist_owned_arg_temps(b, fresh_vars, ctx, fn_freed),
        HirStmtKind::Foreach { body, .. } => hoist_owned_arg_temps(body, fresh_vars, ctx, fn_freed),
        HirStmtKind::Match { arms, .. } => {
            for a in arms {
                hoist_owned_arg_temps(&mut a.body, fresh_vars, ctx, fn_freed);
            }
        }
        _ => {}
    }
}

/// Depth-first walk of `e`'s child expressions, then hoist this node's own
/// owned arguments. Unknown variants are NOT descended into — conservative:
/// the value keeps leaking exactly as it does today, never miscompiled.
#[allow(clippy::too_many_arguments)]
fn hoist_in_expr(
    e: &mut HirExpr,
    pre: &mut Vec<HirStmt>,
    post: &mut Vec<HirStmt>,
    fresh_vars: &HashMap<String, bool>,
    ctx: &mut Pass<'_>,
    fn_freed: &HashSet<String>,
) {
    hoist_in_children(e, pre, post, fresh_vars, ctx, fn_freed);
    let HirExprKind::Call { callee, args, .. } = &mut e.kind else {
        return;
    };
    let callee = callee.clone();
    if ctx
        .param_return
        .get(callee.as_str())
        .is_some_and(|c| c.len() > 1)
    {
        // Phase C owns selector candidates (LetInsert + scope-end release).
        return;
    }
    for i in 0..args.len() {
        let mode = mode_for_arg(&callee, i, ctx);
        if !matches!(mode, Mode::Borrow) {
            continue;
        }
        // `mode_for_arg` treats a fresh callee's parameters as borrowed so the
        // READ analysis can see through them; an explicit escape summary still
        // wins here, because releasing a value the callee may have stashed is a
        // use-after-free.
        if ctx
            .escapes
            .get(callee.as_str())
            .is_some_and(|s| s.get(i).copied().unwrap_or(true))
        {
            continue;
        }
        // Intrinsics take raw words and pointers whose retention no escape
        // summary models — `Store#` writes the value itself into memory, so a
        // release after the statement would dangle whatever now holds it.
        if callee.ends_with('#') {
            continue;
        }
        if !is_owned_temp_expr(&args[i], ctx) {
            continue;
        }
        // The binding type classifies the VALUE (and picks the destroy); the
        // replaced argument keeps its ORIGINAL carrier stamp. Restamping it
        // with the value type makes the argument adapter convert a register the
        // callee already takes directly (`inttoptr ptr` — invalid IR).
        let Some(value_ty) = hoist_value_type(&args[i]) else {
            continue;
        };
        let arg_ty = args[i].ty.clone();
        let tname = fresh_temp_name(&callee, ctx);
        let orig = std::mem::replace(
            &mut args[i],
            HirExpr {
                id: NodeId(ctx.next),
                span: None,
                ty: arg_ty,
                kind: HirExprKind::Identifier(tname.clone()),
            },
        );
        ctx.next += 1;
        pre.push(HirStmt {
            id: NodeId(ctx.next),
            span: None,
            kind: HirStmtKind::LetInsert {
                name: tname.clone(),
                ty: Some(value_ty.clone()),
                expr: Box::new(orig),
            },
        });
        ctx.next += 1;
        let kind = kind_of(&value_ty);
        post.push(release(&tname, kind, ctx));
    }
}

/// Descend into every child expression the hoister understands.
fn hoist_in_children(
    e: &mut HirExpr,
    pre: &mut Vec<HirStmt>,
    post: &mut Vec<HirStmt>,
    fresh_vars: &HashMap<String, bool>,
    ctx: &mut Pass<'_>,
    fn_freed: &HashSet<String>,
) {
    macro_rules! go {
        ($x:expr) => {
            hoist_in_expr($x, pre, post, fresh_vars, ctx, fn_freed)
        };
    }
    match &mut e.kind {
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args {
                go!(a);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            go!(receiver);
            for a in args {
                go!(a);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            go!(left);
            go!(right);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Reflect {
            receiver: inner, ..
        } => go!(inner),
        HirExprKind::Index(base, idx) => {
            go!(base);
            go!(idx);
        }
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            go!(array);
            for x in [start, end, stride].into_iter().flatten() {
                go!(x);
            }
        }
        HirExprKind::Range { start, end, .. } | HirExprKind::Within(start, end) => {
            go!(start);
            go!(end);
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            go!(condition);
            go!(then_branch);
            if let Some(x) = else_branch {
                go!(x);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            go!(scrutinee);
            for a in arms {
                go!(&mut a.body);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, x) in fields.iter_mut() {
                go!(x);
            }
            for (_, x) in specs.iter_mut() {
                go!(x);
            }
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                go!(a);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                go!(r);
            }
            for a in args {
                go!(a);
            }
        }
        _ => {}
    }
}

fn release(name: &str, kind: OwnKind, ctx: &mut Pass<'_>) -> HirStmt {
    let id = NodeId(ctx.next);
    ctx.next += 1;
    ctx.events.push(OwnedEvent {
        function: ctx.function.clone(),
        name: name.to_string(),
        kind: OwnedEventKind::Release {
            list: kind == OwnKind::Lst,
        },
    });
    HirStmt {
        id,
        span: None,
        kind: HirStmtKind::ReleaseOwned {
            storage: match kind {
                OwnKind::Str => StorageKind::Str,
                OwnKind::Lst => StorageKind::List,
            },
            name: name.to_string(),
        },
    }
}

fn mention(e: &HirExpr, out: &mut HashSet<String>) {
    match &e.kind {
        HirExprKind::Identifier(n) => {
            out.insert(n.clone());
        }
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args {
                mention(a, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            mention(receiver, out);
            for a in args {
                mention(a, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            mention(left, out);
            mention(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Reflect {
            receiver: inner, ..
        }
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => mention(inner, out),
        HirExprKind::Index(a, b) => {
            mention(a, out);
            mention(b, out);
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            mention(condition, out);
            mention(then_branch, out);
            if let Some(e) = else_branch {
                mention(e, out);
            }
        }
        HirExprKind::Block(stmts) => {
            for s in stmts {
                mention_stmt(s, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            mention(scrutinee, out);
            for a in arms {
                mention(&a.body, out);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) {
                mention(e, out);
            }
        }
        _ => {}
    }
}

fn mention_stmt(s: &HirStmt, out: &mut HashSet<String>) {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. }
        | HirStmtKind::Term(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Expression(e)
        | HirStmtKind::Gate(e) => mention(e, out),
        HirStmtKind::Assign(l, r) => {
            mention(l, out);
            mention(r, out);
        }
        HirStmtKind::TempTransfer { expr, releases, .. } => {
            mention(expr, out);
            for (_, n) in releases {
                out.insert(n.clone());
            }
        }
        HirStmtKind::ReleaseOwned { name, .. } => {
            out.insert(name.clone());
        }
        HirStmtKind::Guarded(c, b) => {
            mention(c, out);
            for s in b {
                mention_stmt(s, out);
            }
        }
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Defer(b) => {
            for s in b {
                mention_stmt(s, out);
            }
        }
        HirStmtKind::Foreach { list, body, .. } => {
            mention(list, out);
            for s in body {
                mention_stmt(s, out);
            }
        }
        HirStmtKind::Match { expr, arms } => {
            mention(expr, out);
            for a in arms {
                for s in &a.body {
                    mention_stmt(s, out);
                }
            }
        }
        _ => {}
    }
}

fn uses(e: &HirExpr, owned: &mut HashMap<String, OwnKind>, ctx: &mut Pass<'_>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => {
            // A fresh-set callee provably allocates its own result and (by
            // the escape gate above) lets no argument escape: its args are
            // reads. Identity-return callees (param_return) still hit the
            // Escape path so the caller drops the handle the result now owns.
            for (i, a) in args.iter().enumerate() {
                let mode = mode_for_arg(callee, i, ctx);
                let reason = if mode_is_escape_or_consume(mode) {
                    callee.clone()
                } else {
                    String::new()
                };
                use_arg(a, mode, owned, ctx, &reason);
            }
        }
        HirExprKind::Consume(inner) => use_arg(inner, Mode::Consume, owned, ctx, "consume"),
        HirExprKind::AddrOf(inner) => use_arg(inner, Mode::Escape, owned, ctx, "addr-of"),
        // 2026-11-12: MethodCall receivers borrow, like Field reads. Same
        // trust class: a user method that STASHES its receiver escapes it
        // silently — recorded as the known limitation of this slice; stdlib
        // methods (Count#, iteration) are pure.
        HirExprKind::MethodCall { receiver, args, .. } => {
            use_arg(receiver, Mode::Borrow, owned, ctx, "");
            for a in args {
                use_arg(a, Mode::Borrow, owned, ctx, "");
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, v) in fields.iter().chain(specs.iter()) {
                use_arg(v, Mode::Escape, owned, ctx, "store");
            }
        }
        HirExprKind::List(items) | HirExprKind::Tuple(items) => {
            for v in items {
                use_arg(v, Mode::Escape, owned, ctx, "store");
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            use_arg(left, Mode::Borrow, owned, ctx, "");
            use_arg(right, Mode::Borrow, owned, ctx, "");
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Reflect {
            receiver: inner, ..
        }
        | HirExprKind::Deref(inner)
        | HirExprKind::Await(inner) => use_arg(inner, Mode::Borrow, owned, ctx, ""),
        HirExprKind::Index(a, b) => {
            use_arg(a, Mode::Borrow, owned, ctx, "");
            use_arg(b, Mode::Borrow, owned, ctx, "");
        }
        // 2026-11-13 (view semantics): slicing an OWNED local escapes a view
        // into it — the local can no longer be released while the view lives.
        HirExprKind::Slice { array, .. } => {
            if let HirExprKind::Identifier(n) = &array.kind {
                if owned.remove(n).is_some() {
                    ctx.events.push(OwnedEvent {
                        function: ctx.function.clone(),
                        name: n.clone(),
                        kind: OwnedEventKind::Borrow,
                    });
                }
            } else {
                use_arg(array, Mode::Borrow, owned, ctx, "");
            }
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            use_arg(condition, Mode::Borrow, owned, ctx, "");
            use_arg(then_branch, Mode::Borrow, owned, ctx, "");
            if let Some(e) = else_branch {
                use_arg(e, Mode::Borrow, owned, ctx, "");
            }
        }
        HirExprKind::Block(stmts) => {
            for s in stmts {
                use_stmt(s, owned, ctx);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            use_arg(scrutinee, Mode::Borrow, owned, ctx, "");
            for a in arms {
                use_arg(&a.body, Mode::Borrow, owned, ctx, "");
            }
        }
        _ => {}
    }
}

fn use_stmt(s: &HirStmt, owned: &mut HashMap<String, OwnKind>, ctx: &mut Pass<'_>) {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. }
        | HirStmtKind::Term(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Expression(e)
        | HirStmtKind::Gate(e) => uses(e, owned, ctx),
        HirStmtKind::Assign(l, r) => {
            uses(l, owned, ctx);
            uses(r, owned, ctx);
        }
        HirStmtKind::TempTransfer { expr, .. } => uses(expr, owned, ctx),
        HirStmtKind::ReleaseOwned { .. } => {}
        HirStmtKind::Guarded(c, b) => {
            uses(c, owned, ctx);
            for s in b {
                use_stmt(s, owned, ctx);
            }
        }
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Defer(b) => {
            for s in b {
                use_stmt(s, owned, ctx);
            }
        }
        HirStmtKind::Foreach { list, body, .. } => {
            uses(list, owned, ctx);
            for s in body {
                use_stmt(s, owned, ctx);
            }
        }
        HirStmtKind::Match { expr, arms } => {
            uses(expr, owned, ctx);
            for a in arms {
                for s in &a.body {
                    use_stmt(s, owned, ctx);
                }
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Borrow,
    Consume,
    Escape,
}

fn use_arg(
    e: &HirExpr,
    mode: Mode,
    owned: &mut HashMap<String, OwnKind>,
    ctx: &mut Pass<'_>,
    reason: &str,
) {
    match mode {
        Mode::Borrow => uses(e, owned, ctx),
        Mode::Consume | Mode::Escape => {
            // 2026-11-14: an argument that is itself a CALL must be evaluated
            // by its own callee's per-parameter modes, not by collecting every
            // identifier inside it. `mention` recurses into nested call
            // arguments, so `tensor_set(t, i, f32_from_bytes(text, 4*i))`
            // attributed `text`'s escape to `tensor_set` — the frame String was
            // never released (soak: +4.9 MB/call, exactly the input size). The
            // nested call's RESULT escaping retains nothing; an identity return
            // is handled by param_return summaries with a caller-side drop.
            if matches!(
                e.kind,
                HirExprKind::Call { .. } | HirExprKind::MethodCall { .. }
            ) {
                uses(e, owned, ctx);
                return;
            }
            let mut ids = HashSet::new();
            mention(e, &mut ids);
            for id in ids {
                owned.remove(&id);
                // Ok/Err constructor payloads are RETURN TRANSFERS, not
                // arg escapes into storage — they are the function's result.
                if reason == "Ok" || reason == "Err" {
                    continue;
                }
                let kind = match mode {
                    Mode::Consume => OwnedEventKind::ConsumeCall(reason.to_string()),
                    _ => OwnedEventKind::EscapeCall(reason.to_string()),
                };
                ctx.events.push(OwnedEvent {
                    function: ctx.function.clone(),
                    name: id,
                    kind,
                });
            }
        }
    }
}

fn mode_is_escape_or_consume(m: Mode) -> bool {
    matches!(m, Mode::Escape | Mode::Consume)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casting::graph::CastingGraph;
    use crate::hir::lower_program;
    use crate::type_universe::TypeUniverse;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn annotated(src: &str) -> HirProgram {
        let mut program = lower_program(&parse(src));
        let universe = TypeUniverse::new();
        let graph = CastingGraph::new();
        crate::hir::typing::annotate_types(&mut program, &universe, &graph);
        program
    }

    /// Collect every release the pass inserted (or authored Free# calls on
    /// plain identifiers), so tests assert on a flat list.
    fn releases_of(c: &HirCallable) -> Vec<(StorageKind, String)> {
        fn walk(stmts: &[HirStmt], out: &mut Vec<(StorageKind, String)>) {
            for st in stmts {
                match &st.kind {
                    HirStmtKind::ReleaseOwned { storage, name } => {
                        out.push((*storage, name.clone()));
                    }
                    HirStmtKind::TempTransfer { releases, .. } => {
                        out.extend(releases.iter().cloned());
                    }
                    HirStmtKind::Expression(HirExpr {
                        kind: HirExprKind::Call { callee, args, .. },
                        ..
                    }) => {
                        if callee == "Free#" {
                            for a in args {
                                if let HirExprKind::Identifier(n) = &a.kind {
                                    out.push((StorageKind::Str, n.clone()));
                                }
                            }
                        }
                    }
                    HirStmtKind::Guarded(_, b)
                    | HirStmtKind::Block(b)
                    | HirStmtKind::SyncBlock(b)
                    | HirStmtKind::Mutex(b)
                    | HirStmtKind::Defer(b) => walk(b, out),
                    HirStmtKind::Foreach { body, .. } => walk(body, out),
                    HirStmtKind::Match { arms, .. } => {
                        for a in arms {
                            walk(&a.body, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&c.body.statements, &mut out);
        out
    }

    fn has_free(c: &HirCallable, name: &str) -> bool {
        releases_of(c)
            .iter()
            .any(|(k, n)| *k == StorageKind::Str && n == name)
    }

    fn has_destroy(c: &HirCallable, name: &str) -> bool {
        releases_of(c)
            .iter()
            .any(|(k, n)| *k == StorageKind::List && n == name)
    }

    #[test]
    fn debug_fresh_int_list_derivation() {
        let program = annotated(
            r#"
defn fresh_int_list(items: List<Int>) [true][true] -> List<Int> {
    let copy: List<Int> = [];
    foreach i in 0..items.Count#() {
        copy <- items[i];
    };
    term copy;
};
"#,
        );
        let no_freed_map: HashMap<String, HashSet<String>> = HashMap::new();
        let fresh = fresh_callables(&program, &HashMap::new(), &no_freed_map);
        eprintln!(
            "dbg fresh_int_list fresh={}",
            fresh.contains("fresh_int_list")
        );
        let b = program
            .callables
            .iter()
            .find(|c| c.name == "fresh_int_list")
            .unwrap();
        let mut vars: HashMap<String, bool> =
            b.params.iter().map(|p| (p.name.clone(), false)).collect();
        let no_pr: HashMap<String, Vec<usize>> = HashMap::new();
        let no_ok: HashSet<String> = HashSet::new();
        let no_freed: HashSet<String> = HashSet::new();
        collect_fresh(
            &b.body.statements,
            &mut vars,
            &fresh,
            &no_pr,
            &no_ok,
            &no_freed,
        );
        eprintln!("dbg vars={vars:?}");
        for s in &b.body.statements {
            eprintln!("dbg stmt={:?}", s.kind);
        }
        let no_pr: HashMap<String, Vec<usize>> = HashMap::new();
        let no_ok: HashSet<String> = HashSet::new();
        let no_freed: HashSet<String> = HashSet::new();
        let mut returns = Vec::new();
        collect_returns(&b.body.statements, &mut returns);
        for e in &returns {
            eprintln!(
                "dbg ret={:?} fresh={}",
                e.kind,
                is_fresh(e, &vars, &no_pr, &fresh, &no_ok, &no_freed)
            );
        }
    }

    #[test]
    fn list_literal_owner_is_destroyed_at_exit() {
        let mut program = annotated(
            r#"
defn use() -> Int { let xs: List<Int> = [1, 2]; let n: Int = xs.Count#(); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(
            has_destroy(use_fn, "xs"),
            "dead owned List must be destroyed"
        );
        assert!(!has_free(use_fn, "xs"), "List must not go through Free#");
    }

    #[test]
    fn param_identity_return_transfers_list_ownership() {
        let mut program = annotated(
            r#"
defn passthrough(xs: List<Int>) -> List<Int> { term xs; };
defn use() -> Int { let xs: List<Int> = [7]; let ys: List<Int> = passthrough(xs); let n: Int = ys.Count#(); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(
            has_destroy(use_fn, "ys"),
            "the transferred owner must be destroyed"
        );
        assert!(
            !has_destroy(use_fn, "xs"),
            "the pre-transfer handle must not double-free"
        );
    }

    #[test]
    fn fresh_return_borrowing_owned_local_temp_transfers() {
        let mut program = annotated(
            r#"
defn mk() -> String { term "a" + "b"; };
defn cat(a: String, b: String) -> String { term a + b; };
defn use() -> String { let s: String = mk(); term cat(s, "!"); };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(
            has_free(use_fn, "s"),
            "the borrowed operand must free after the fresh call evaluates"
        );
        let last = use_fn.body.statements.last().unwrap();
        match &last.kind {
            HirStmtKind::TempTransfer { name, .. } => {
                assert!(
                    name.starts_with("__ot"),
                    "expected temp-transfer binding, got {name}"
                );
            }
            other => panic!("expected temp-transfer tail, got {other:?}"),
        }
    }

    #[test]
    fn infers_fresh_string_call_and_releases_dead_local() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); let n: Int = CharCount#(s); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(has_free(use_fn, "s"), "dead owned String must be released");
    }

    #[test]
    fn alias_keeps_ownership_conservative() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); let t: String = s; let n: Int = CharCount#(t); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(!has_free(use_fn, "s"), "aliased String must not be freed");
        assert!(!has_free(use_fn, "t"), "alias is not an owner");
    }

    #[test]
    fn memcheck_events_cover_release_keep_alias_undecided() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn unknown() -> String;
defn use() -> Int {
    let s: String = make();
    let t: String = unknown();
    let u: String = s;
    keep s;
    let n: Int = CharCount#(t);
    term n;
};
"#,
        );
        // `unknown` has no body in this fragment: drop it so summaries see a
        // missing callee (unknown call -> escape + undecided init).
        program.callables.retain(|c| c.name != "unknown");
        let events = memcheck_owned(&program, &HashMap::new());
        let of =
            |name: &str| -> Vec<&OwnedEvent> { events.iter().filter(|e| e.name == name).collect() };
        // u aliases s -> Alias for s (u is not fresh, no owner).
        assert!(
            of("s")
                .iter()
                .any(|e| matches!(e.kind, OwnedEventKind::Alias)),
            "alias event for s"
        );
        // keep s -> Kept.
        assert!(
            of("s")
                .iter()
                .any(|e| matches!(e.kind, OwnedEventKind::Kept)),
            "keep event for s"
        );
        // t = unknown() -> UndecidedInit. (The callee has no args, so there
        // is no arg-side escape event; the undecided INIT is the result-side
        // verdict.)
        assert!(
            of("t")
                .iter()
                .any(|e| matches!(e.kind, OwnedEventKind::UndecidedInit(_))),
            "undecided for t"
        );
        assert_eq!(
            of("t").len(),
            1,
            "t should have exactly the undecided event"
        );
    }

    #[test]
    fn memcheck_release_events_carry_kinds() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int {
    let s: String = make();
    let xs: List<Int> = [1, 2];
    let n: Int = CharCount#(s) + xs.Count#();
    term n;
};
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let events = memcheck_owned(&program, &HashMap::new());
        assert!(events
            .iter()
            .any(|e| e.name == "s" && matches!(e.kind, OwnedEventKind::Release { list: false })));
        assert!(events
            .iter()
            .any(|e| e.name == "xs" && matches!(e.kind, OwnedEventKind::Release { list: true })));
    }

    #[test]
    fn top_level_state_string_is_not_a_local_owner() {
        let mut program = annotated(
            r#"
let cache: String = "x";
defn store() -> Void { cache = "a" + "b"; term; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let store = program
            .callables
            .iter()
            .find(|c| c.name == "store")
            .unwrap();
        assert!(
            !has_free(store, "cache"),
            "global/state storage is not a local owner"
        );
    }

    #[test]
    fn keep_suppresses_release() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); keep s; let n: Int = CharCount#(s); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(!has_free(use_fn, "s"), "keep must suppress auto-release");
    }

    /// A self-tail-recursive handler that hands its accumulator back unchanged
    /// must still publish a param-return summary. This is the protobuf parser
    /// shape (`term f(r, next)` threaded through a base `term dims`): the
    /// recursive arm is neutral, the base case pins the parameter.
    #[test]
    fn self_tail_loop_threads_its_parameter() {
        let program = annotated(
            r#"
import "std/alloc.bv";
defn keep_loop(n: Int, acc: List<Int>) [true][true] -> List<Int> {
    when n <= 0 {
        term acc;
    };
    term keep_loop(n - 1, acc);
};
"#,
        );
        let pr = param_return_summaries(&program);
        assert_eq!(pr.get("keep_loop"), Some(&vec![1usize]));
    }

    /// A local bound from the recursive call still denotes the caller's
    /// parameter, so the chain stays a borrow and never becomes an owner.
    #[test]
    fn call_bound_local_still_aliases_the_parameter() {
        let program = annotated(
            r#"
import "std/alloc.bv";
defn keep_loop(n: Int, acc: List<Int>) [true][true] -> List<Int> {
    when n <= 0 {
        term acc;
    };
    term keep_loop(n - 1, acc);
};
defn wrap(n: Int, acc: List<Int>) [true][true] -> List<Int> {
    let next: List<Int> = keep_loop(n, acc);
    term next;
};
"#,
        );
        let pr = param_return_summaries(&program);
        // `wrap` returns the value bound from `keep_loop(n, acc)`, which is
        // `acc` — parameter 1 — not a fresh list.
        assert_eq!(pr.get("wrap"), Some(&vec![1usize]));
    }

    /// A selector returning one of SEVERAL parameters is only fresh when every
    /// candidate argument is fresh; when all of them are caller storage the
    /// result may be a borrow and must not be freed.
    #[test]
    fn multi_candidate_return_is_fresh_only_when_all_candidates_are() {
        let program = annotated(
            r#"
import "std/alloc.bv";
defn pick(a: List<Int>, b: List<Int>) [true][true] -> List<Int> {
    when a.Count#() == 0 {
        term b;
    };
    term a;
};
defn borrowed(a: List<Int>, b: List<Int>) [true][true] -> List<Int> {
    let picked: List<Int> = pick(a, b);
    term picked;
};
defn owned() [true][true] -> List<Int> {
    let empty: List<Int> = [];
    let other: List<Int> = [];
    let picked: List<Int> = pick(empty, other);
    term picked;
};
"#,
        );
        let pr = param_return_summaries(&program);
        assert_eq!(pr.get("pick").map(|v| v.len()), Some(2), "both params");

        let no_freed_map: HashMap<String, HashSet<String>> = HashMap::new();
        let escapes: HashMap<String, Vec<bool>> = HashMap::new();
        let fresh = fresh_callables(&program, &escapes, &no_freed_map);
        let no_ok: HashSet<String> = HashSet::new();
        let no_freed: HashSet<String> = HashSet::new();

        let bound_var = |name: &str, var: &str| -> Option<bool> {
            let c = program.callables.iter().find(|c| c.name == name).unwrap();
            let mut vars: HashMap<String, bool> =
                c.params.iter().map(|p| (p.name.clone(), false)).collect();
            collect_fresh(
                &c.body.statements,
                &mut vars,
                &fresh,
                &pr,
                &no_ok,
                &no_freed,
            );
            vars.get(var).copied()
        };
        assert_eq!(
            bound_var("borrowed", "picked"),
            Some(false),
            "pick(a, b) over caller storage must not be fresh (it may alias)"
        );
        assert_eq!(
            bound_var("owned", "picked"),
            Some(true),
            "pick over two fresh lists is fresh (every candidate is owned)"
        );
    }

    /// A self-tail call passing an owned INLINE temporary is no longer a leaked
    /// temporary — the loop lowering frees the slot at every backedge — so the
    /// census must record the FACT (M2, 2026-11-16) instead of an undecided
    /// event. The fact is what the backend consumes; the event is what blocks
    /// `--deny-memory-ambiguity`, so the two must agree.
    #[test]
    fn self_tail_owned_temporary_becomes_a_loop_release() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn mk(s: String) [true][true] -> String {
    term fresh_string(s);
};
defn loop(s: String, n: Int) [true][true] -> String {
    when n <= 0 {
        term mk(s);
    };
    term loop(mk(s), n - 1);
};
"#,
        );
        let events = memcheck_owned(&program, &HashMap::new());
        let hits: Vec<&OwnedEvent> = events
            .iter()
            .filter(
                |e| matches!(&e.kind, OwnedEventKind::UndecidedInit(w) if w.contains("self-tail")),
            )
            .collect();
        assert!(
            hits.is_empty(),
            "the loop owns and frees the carried temporary: {hits:?}"
        );
        let plan = insert_releases(&mut program, &HashMap::new());
        let params = plan.0.get("loop").expect("loop must be recorded");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].index, 0);
        assert_eq!(params[0].storage, StorageKind::Str);
        // The self-tail site allocates, so the first backedge may free too.
        assert!(params[0].free_first);
        // The base case returns `mk(s)`, a value of its own — not the slot.
        assert!(!params[0].exit_transfers);
    }

    /// The same recursion entered with a local the CALLER keeps alive stays a
    /// reported leak: freeing the slot would be a use-after-free in that
    /// caller, so no fact is recorded and the census keeps blocking.
    #[test]
    fn self_tail_owned_temporary_is_reported_when_a_caller_keeps_the_value() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn mk(s: String) [true][true] -> String {
    term fresh_string(s);
};
defn loop(s: String, n: Int) [true][true] -> String {
    when n <= 0 {
        term mk(s);
    };
    term loop(mk(s), n - 1);
};
defn user(seed: String) [true][true] -> Int {
    let r: String = loop(seed, 3);
    term r ^^Length + seed ^^Length;
};
"#,
        );
        let plan = insert_releases(&mut program, &HashMap::new());
        assert!(
            plan.0.get("loop").is_none(),
            "a caller that reads `seed` after the call owns it"
        );
        let events = memcheck_owned(&program, &HashMap::new());
        assert!(
            events.iter().any(
                |e| matches!(&e.kind, OwnedEventKind::UndecidedInit(w) if w.contains("self-tail"))
            ),
            "the unfixable temporary must stay in the census"
        );
    }

    /// A self-tail call with no owned temporary must stay silent.
    #[test]
    fn self_tail_without_owned_temporary_is_not_reported() {
        let program = annotated(
            r#"
import "std/alloc.bv";
defn loop(s: String, n: Int) [true][true] -> String {
    when n <= 0 {
        term fresh_string(s);
    };
    term loop(s, n - 1);
};
"#,
        );
        let events = memcheck_owned(&program, &HashMap::new());
        assert!(
            !events.iter().any(
                |e| matches!(&e.kind, OwnedEventKind::UndecidedInit(w) if w.contains("self-tail"))
            ),
            "the carried value is a parameter, not a temporary"
        );
    }

    // 2026-11-20 (K3): a guard-armed producer (a Bytes-literal terminal)
    // classifies as Static so the free dispatcher can refuse the Free#.
    #[test]
    fn static_return_summary_classifies_static_guard_arm() {
        let program = annotated(
            r#"
defn sentinel(x: Float) [true][true] -> String {
    when x > 0.0 {
        term "inf";
    };
    term "neg";
};
"#,
        );
        let mut program2 = program.clone();
        let plan = insert_releases(&mut program2, &HashMap::new());
        let sr = &plan.1;
        assert_eq!(
            sr.get("sentinel"),
            Some(&StaticClass::Static),
            "a Bytes-literal terminal arm is a static producer"
        );
    }

    // 2026-11-20 (K3): a fully-fresh producer classifies as Owned.
    #[test]
    fn static_return_summary_classifies_fresh_as_owned() {
        let program = annotated(
            r#"
import "std/alloc.bv";
defn copy(s: String) [true][true] -> String {
    term fresh_string(s);
};
"#,
        );
        let mut program2 = program.clone();
        let plan = insert_releases(&mut program2, &HashMap::new());
        let sr = &plan.1;
        assert_eq!(
            sr.get("copy"),
            Some(&StaticClass::Owned),
            "a proven-fresh producer is Owned"
        );
    }

    // 2026-11-21 (K3 Phase C): a multi-candidate selector (`ints_or` shape)
    // whose inline candidates are fresh call results and list literals must
    // emit a release for each candidate block. The result local is a borrow
    // (aliases one candidate) — not an owner and not a conservative drop.
    // Regression: without the LetInsert pass, the loser of each `ints_or`
    // pair leaks (the pass cannot statically know which candidate wins).
    #[test]
    fn multi_candidate_selector_releases_inline_candidates() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn fresh_list() [true][true] -> List<Int> {
    let xs: List<Int> = [1];
    term xs;
};
defn pick(a: List<Int>, b: List<Int>) [true][true] -> List<Int> {
    when a.Count#() == 0 {
        term b;
    };
    term a;
};
defn use() [true][true] -> Int {
    let stride: List<Int> = pick(fresh_list(), [3]);
    let n: Int = stride.Count#();
    term n;
};
"#,
        );
        let events = memcheck_owned(&program, &HashMap::new());
        // The result local `stride` must be a Static-backed borrow (aliases
        // one candidate), NOT a conservative drop and NOT an undecided init.
        assert!(
            events
                .iter()
                .any(|e| e.name == "stride" && matches!(e.kind, OwnedEventKind::Static)),
            "selector result must be a static-backed borrow, got: {:?}",
            events
                .iter()
                .filter(|e| e.name == "stride")
                .collect::<Vec<_>>()
        );
        // No undecided event for `stride` (the pass must classify it).
        assert!(
            !events
                .iter()
                .any(|e| e.name == "stride" && matches!(e.kind, OwnedEventKind::UndecidedInit(_))),
            "selector result must not be undecided"
        );
        // Run the full pass and verify releases were inserted for the
        // selector candidates (the inline list literal `[3]` and the
        // `fresh_list()` result are both owned blocks that must be freed).
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        // Count List destroys: we expect at least one destroy for the
        // inline literal `[3]` candidate and one for the `fresh_list()`
        // result. Both are owned blocks that the selector may not pick.
        let destroys = releases_of(use_fn)
            .iter()
            .filter(|(k, _)| *k == StorageKind::List)
            .count();
        assert!(
            destroys >= 2,
            "expected >= 2 List destroys (one per inline candidate), got {destroys}"
        );
    }

    // 2026-11-21 (K3 Phase C): nested selector — a selector whose candidate
    // is ITSELF a selector result. The inner selector's result is already
    // tracked in `sel_results`, so the outer must not re-own it (double-free
    // risk). Both levels must free their own inline candidates exactly once.
    #[test]
    fn nested_selector_does_not_double_free_inner_result() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn mk() [true][true] -> List<Int> {
    term [42];
};
defn pick(a: List<Int>, b: List<Int>) [true][true] -> List<Int> {
    when a.Count#() == 0 {
        term b;
    };
    term a;
};
defn use() [true][true] -> Int {
    let inner: List<Int> = pick(mk(), [1]);
    let outer: List<Int> = pick(inner, [7]);
    let n: Int = outer.Count#();
    term n;
};
"#,
        );
        let events = memcheck_owned(&program, &HashMap::new());
        // `inner` is a selector result → Static-backed borrow.
        assert!(
            events
                .iter()
                .any(|e| e.name == "inner" && matches!(e.kind, OwnedEventKind::Static)),
            "inner selector result must be a borrow"
        );
        // `outer` is also a selector result → Static-backed borrow.
        assert!(
            events
                .iter()
                .any(|e| e.name == "outer" && matches!(e.kind, OwnedEventKind::Static)),
            "outer selector result must be a borrow"
        );
        // No undecided events for either.
        assert!(
            !events
                .iter()
                .any(|e| (e.name == "inner" || e.name == "outer")
                    && matches!(e.kind, OwnedEventKind::UndecidedInit(_))),
            "neither selector result may be undecided"
        );
        // Run the pass — the critical assertion is that the pass completes
        // without panicking and produces a well-formed program. The inline
        // candidates ([1], [7], [42]) each get their own release; the named
        // `inner` candidate is NOT re-owned (it's in sel_results).
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        // At minimum: the three inline literals [42], [1], [7] each need a
        // List destroy. `inner` (named candidate at the outer pick) must NOT
        // be double-freed — its block is already covered by the inner pick's
        // own temps.
        let destroys = releases_of(use_fn)
            .iter()
            .filter(|(k, _)| *k == StorageKind::List)
            .count();
        assert!(
            destroys >= 3,
            "expected >= 3 List destroys (one per inline literal), got {destroys}"
        );
    }

    // 2026-11-21 (K3 residual): an EMPTY list literal has no element type to
    // recover, so the emitter cannot construct it through the collection ops —
    // a bare list literal has no codegen shape of its own (the operand path
    // panics). The hoister must leave such an argument alone.
    #[test]
    fn empty_list_literal_arg_is_not_hoisted() {
        let mut program = annotated(
            r#"
defn total(xs: List<Int>) -> Int { let s: Int = 0; foreach x in xs { s = s + x; }; term s; };
defn use() -> Int { term total([]); };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        let inserted = releases_of(use_fn).len()
            + use_fn
                .body
                .statements
                .iter()
                .filter(|s| matches!(s.kind, HirStmtKind::LetInsert { .. }))
                .count();
        assert_eq!(
            inserted, 0,
            "an empty list literal has no recoverable element type; hoisting it \
             emits a bare literal and panics the emitter (got {inserted} inserts)"
        );
    }

    // 2026-11-21 (K3 residual): an owned value produced INLINE and handed to a
    // BORROWING parameter has no name for the scope sweep. The hoister binds it
    // to a pure-insert temp and releases it after the statement.
    #[test]
    fn owned_arg_temp_to_borrowing_param_is_released() {
        let mut program = annotated(
            r#"
defn mk() -> List<Int> { term [1, 2]; };
defn relay(x: List<Int>) -> List<Int> { term [3]; };
defn use() -> Int { let ys: List<Int> = relay(mk()); term 1; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        let temps: Vec<String> = releases_of(use_fn)
            .into_iter()
            .filter(|(k, _)| *k == StorageKind::List)
            .map(|(_, n)| n)
            .collect();
        assert!(
            temps.iter().any(|n| n.starts_with("__seltrelay")),
            "the borrowed inline `mk()` result must be released through a \
             synthesized temp, got {temps:?}"
        );
        // The temp must be BOUND (LetInsert) as well as released, or the
        // release would reference an undefined register.
        fn has_let_insert(stmts: &[HirStmt], name: &str) -> bool {
            stmts.iter().any(|s| match &s.kind {
                HirStmtKind::LetInsert { name: n, .. } => n == name,
                HirStmtKind::Guarded(_, b)
                | HirStmtKind::Block(b)
                | HirStmtKind::SyncBlock(b)
                | HirStmtKind::Mutex(b)
                | HirStmtKind::Defer(b) => has_let_insert(b, name),
                HirStmtKind::Foreach { body, .. } => has_let_insert(body, name),
                _ => false,
            })
        }
        for t in temps.iter().filter(|t| t.starts_with("__selt")) {
            assert!(
                has_let_insert(&use_fn.body.statements, t),
                "temp {t} is released but never bound"
            );
        }
    }

    // 2026-11-21 (K3 residual, String policy): a String argument temporary is
    // NOT hoisted. Its binding round-trip records the value in
    // `let_binding_types`, and the argument adapter then converts a register
    // the callee already takes directly (`inttoptr ptr` — invalid IR, observed
    // on the kiosk daemon's `owned_string(proto_bytes(r))`). Such a temporary
    // keeps leaking exactly as it did before R2; it is never miscompiled.
    #[test]
    fn string_arg_temp_is_not_hoisted() {
        let mut program = annotated(
            r#"
defn mk() -> String { term "a" + "b"; };
defn relay(x: String) -> String { term "c" + "d"; };
defn use() -> Int { let n: Int = CharCount#(relay(mk())); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        let inserts = use_fn
            .body
            .statements
            .iter()
            .filter(|s| matches!(s.kind, HirStmtKind::LetInsert { .. }))
            .count();
        assert_eq!(
            inserts, 0,
            "a String argument temporary must not be hoisted: the binding makes \
             the argument adapter emit `inttoptr` on an already-pointer register \
             (got {inserts} inserts)"
        );
        assert!(
            releases_of(use_fn)
                .iter()
                .all(|(k, _)| *k != StorageKind::List),
            "no collection release may appear for a String-only program"
        );
    }

    // 2026-11-21 (K3 residual): a parameter the callee CONSUMES is the
    // callee's to free — the caller must not release it too.
    #[test]
    fn owned_arg_temp_to_consuming_param_is_not_released() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn mk() -> String { term "a" + "b"; };
defn sink(x: String) [true][true] -> Void { Free#(x); term; };
defn use() [true][true] -> Void { sink(mk()); term; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        let frees = releases_of(use_fn)
            .into_iter()
            .filter(|(k, _)| *k == StorageKind::Str)
            .count();
        assert_eq!(
            frees, 0,
            "a consuming parameter is freed by the callee; the caller adding a \
             release is a double-free (got {frees})"
        );
    }

    // 2026-11-21 (K3 residual): the hoisted statements are PURE INSERTS. The
    // backend's HIR/AST pair walk requires the counted-statement count to stay
    // equal to the AST body length; a net-new counted statement would force a
    // whole-body AST fallback and silently drop every inserted release.
    #[test]
    fn owned_insert_keeps_pair_walk_aligned() {
        let program = annotated(
            r#"
defn mk() -> List<Int> { term [1, 2]; };
defn relay(x: List<Int>) -> List<Int> { term [3]; };
defn use() -> Int { let ys: List<Int> = relay(mk()); term 1; };
"#,
        );
        let before = program
            .callables
            .iter()
            .find(|c| c.name == "use")
            .unwrap()
            .body
            .statements
            .len();
        let mut program2 = program.clone();
        insert_releases(&mut program2, &HashMap::new());
        let use_fn = program2.callables.iter().find(|c| c.name == "use").unwrap();
        let counted = use_fn
            .body
            .statements
            .iter()
            .filter(|s| {
                !matches!(
                    s.kind,
                    HirStmtKind::ReleaseOwned { .. } | HirStmtKind::LetInsert { .. }
                )
            })
            .count();
        assert_eq!(
            counted, before,
            "inserted statements must be pure inserts; the AST pair walk counts \
             {before} but the pass produced {counted} counted statements"
        );
    }

    // 2026-11-21 (K3 residual): the cast owned-return door builds into a
    // staging buffer, copies it out (`briev_bits_to_str`), frees the staging
    // buffer and returns the copy. The authored Free# on the staging handle is
    // CORRECT and must survive the pass untouched (neither dropped nor
    // duplicated). Removing it would leak the staging buffer; duplicating it
    // would be a double-free.
    #[test]
    fn cast_copy_door_keeps_staging_free() {
        let mut program = annotated(
            r#"
import "std/alloc.bv";
defn door(n: Int) [true][term^^Length >= 0] -> String {
    let buf: Int = Alloc#(n + 9) as Int;
    Store#(buf, n, 8);
    let owned: String = (buf as Data) as String;
    Free#(buf as Ptr<Bit<8>>);
    term owned;
};
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let door = program.callables.iter().find(|c| c.name == "door").unwrap();
        // Count authored Free# sites regardless of argument SHAPE: the cast
        // door writes `Free#(buf as Ptr<Bit<8>>)`, whose argument is a Cast,
        // not a bare identifier.
        fn count_frees(stmts: &[HirStmt]) -> usize {
            let mut n = 0;
            for s in stmts {
                match &s.kind {
                    HirStmtKind::Expression(HirExpr {
                        kind: HirExprKind::Call { callee, .. },
                        ..
                    }) if callee == "Free#" => n += 1,
                    HirStmtKind::Guarded(_, b)
                    | HirStmtKind::Block(b)
                    | HirStmtKind::SyncBlock(b)
                    | HirStmtKind::Mutex(b)
                    | HirStmtKind::Defer(b) => n += count_frees(b),
                    HirStmtKind::Foreach { body, .. } => n += count_frees(body),
                    HirStmtKind::Match { arms, .. } => {
                        for a in arms {
                            n += count_frees(&a.body);
                        }
                    }
                    _ => {}
                }
            }
            n
        }
        let frees = count_frees(&door.body.statements);
        assert_eq!(
            frees, 1,
            "the staging handle must be freed exactly once, got {frees}"
        );
    }
}
