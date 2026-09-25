// ── Export ABI Analysis ────────────────────────────────────────────────
// 2026-08-03: Computes, per exported defn, whether its C-ABI signature
// carries the leading `ptr %state` parameter (body-dependent, non-fragile
// ABI — plan 2026-08-03-host-callable-glue-export).
//
// The result is the single source of truth for both the LLVM backend
// (which emits the signature) and GLUE export wrapper generation (which
// renders the per-function state argument). Wrappers/bindings derive their
// ABI from the per-export metadata, never by re-analyzing the AST.
//
// Rule: a defn needs state if its body (transitively through called Briev
// defns) uses the runtime state. This fixes the prior non-transitive check
// in the LLVM backend, which emitted `call @f(ptr %state, ...)` from a
// "pure" export that had no `%state` parameter → undefined-value IR.
//
// Undo: if the state parameter is ever removed from all defn signatures
// (uniform stateless ABI), delete this module and always emit no state.

use crate::ast::{Definition, Expr, Statement, TopLevel};
use std::collections::{HashMap, HashSet};

/// Compute `needs_state` for every defn (exported AND regular) in a program.
///
/// Indexes regular defns, transactions, and exports, then runs a memoized
/// DFS over the call graph. Transactions ALWAYS carry a `%state` parameter
/// (the reactor threads state through them). REGULAR defns are emitted with
/// `%state` ONLY when they (transitively) need it — a pure helper (e.g. a
/// cstr door over Load#/Alloc#) stays stateless and does NOT force its
/// callers to carry state. Exports follow the same rule.
///
/// The returned map is the single source of truth for:
///   - export ABI (`compute_export_needs_state` — the subset for exports)
///   - regular-defn emission (backend emits `%state` iff needs it)
pub fn compute_defn_needs_state(items: &[TopLevel]) -> HashMap<String, bool> {
    let (regular, txns, state_fields, always_stateful) = index_program(items);
    // 2026-09-23 (stateless-defn mechanism): LEAST FIXPOINT, not DFS. The old
    // memoized-DFS `visiting ⇒ true` cycle rule marked self-recursive
    // STATELESS defns (cstr_len `1 + cstr_len(p+1)`, bytes_equal_at recursion)
    // as stateful, defeating the whole mechanism. Under a least fixpoint a
    // defn is STATELESS iff its body (directly and through called defns) is
    // stateless; a cycle contributes nothing by itself — only a genuinely
    // stateful leaf (state field, stateful intrinsic, txn call, or a defn
    // that reaches one) forces state upward. Iterate to a fixed point:
    // start all-false (all stateless) and flip a defn to stateful the moment
    // its body references state. Cycles resolve correctly because no cycle
    // ever flips a member on its own.
    let mut needs: HashMap<String, bool> = regular.keys().map(|k| (k.clone(), false)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (name, d) in &regular {
            if *needs.get(name).unwrap_or(&false) {
                continue;
            }
            if defn_references_state(d, &regular, &txns, &state_fields, &always_stateful, &needs) {
                needs.insert(name.clone(), true);
                changed = true;
            }
        }
    }
    needs
}

/// Index the program into the sets the needs-state walker needs.
fn index_program(
    items: &[TopLevel],
) -> (
    HashMap<String, &Definition>,
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
) {
    let mut regular: HashMap<String, &Definition> = HashMap::new();
    let mut txns: HashSet<String> = HashSet::new();
    let mut state_fields: HashSet<String> = HashSet::new();
    // 2026-09-23 (stateless-defn mechanism): `bad fn` / `asm fn` bodies are
    // compiled OUT OF BAND (the bad backend) with the implicit `%state`
    // pointer as the FIRST ABI arg — a call to one ALWAYS carries state,
    // regardless of the callee's .bv body (there is none). Track them so a
    // .bv caller (banner calling boot_putc) is forced stateful.
    let mut always_stateful: HashSet<String> = HashSet::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                regular.insert(d.name.clone(), d);
            }
            // 2026-09-23 (stateless-defn mechanism): an exported defn is a
            // defn too — it can be called internally and is emitted as a
            // function. Index it so its needs-state is resolved (otherwise
            // it would default to `true` and every export would be
            // stateful). `compute_export_needs_state` reads the subset.
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    regular.insert(d.name.clone(), d);
                }
            }
            TopLevel::AsmFn(a) => { always_stateful.insert(a.name.clone()); }
            TopLevel::BadFn(b) => { always_stateful.insert(b.name.clone()); }
            // 2026-08-03: transactions always carry `ptr %state` (the reactor
            // threads state through them) — a caller must supply it.
            TopLevel::Transaction(t) => {
                txns.insert(t.name.clone());
            }
            TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, .. } = stmt.as_ref() {
                    state_fields.insert(name.clone());
                }
            }
            TopLevel::Constant(c) => {
                state_fields.insert(c.name.clone());
            }
            TopLevel::StateDecl(s) => {
                state_fields.insert(s.name.clone());
            }
            _ => {}
        }
    }
    (regular, txns, state_fields, always_stateful)
}

/// Does `d`'s body reference state under the current `needs` approximation?
/// 2026-09-23 (scoping): a defn's PARAMS and let-bound locals SHADOW global
/// state fields of the same name (`byte_at(s, i)` vs `let i: Int = 0`).
fn defn_references_state(
    d: &Definition,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always_stateful: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    let mut locals: HashSet<String> = HashSet::new();
    for (pn, _) in &d.parameters {
        locals.insert(pn.clone());
    }
    collect_stmt_locals(&d.body, &mut locals);
    body_needs_state(&d.body, regular, txns, state_fields, always_stateful, &locals, needs)
}

/// Compute `needs_state` for every EXPORTED defn — the subset of
/// [`compute_defn_needs_state`] for GLUE ABI generation. A defn is exported
/// if it appears as a `TopLevel::Export` (either direct or wrapping a defn).
pub fn compute_export_needs_state(items: &[TopLevel]) -> HashMap<String, bool> {
    let all = compute_defn_needs_state(items);
    let mut exports: HashMap<String, bool> = HashMap::new();
    for item in items {
        if let TopLevel::Export(e) = item {
            if let TopLevel::Definition(d) = e.inner.as_ref() {
                exports.insert(
                    d.name.clone(),
                    *all.get(&d.name).unwrap_or(&true),
                );
            }
        }
    }
    exports
}

/// Does `body` reference state, treating every called defn as stateful
/// exactly when `needs` says so (a fixed-point iteration step). The `needs`
/// map is the CURRENT iteration's approximation — never the in-progress
/// visited set, so recursion cannot spuriously force state.
fn body_needs_state(
    body: &[Statement],
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always_stateful: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    body.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
}

/// Collect every name bound by a `let` inside a statement tree — those
/// shadow global state fields of the same name.
fn collect_stmt_locals(stmts: &[Statement], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Statement::Let { name, expr, .. } => {
                out.insert(name.clone());
                if let Some(e) = expr {
                    collect_expr_locals(e, out);
                }
            }
            Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Defer(body)
            | Statement::Mutex(body) => collect_stmt_locals(body, out),
            Statement::Guarded(cond, body) => {
                collect_expr_locals(cond, out);
                collect_stmt_locals(body, out);
            }
            Statement::Foreach { list, body, .. } => {
                collect_expr_locals(list, out);
                collect_stmt_locals(body, out);
            }
            Statement::Match { expr, arms } => {
                collect_expr_locals(expr, out);
                for arm in arms {
                    collect_stmt_locals(&arm.body, out);
                }
            }
            Statement::Assign(l, r) => {
                collect_expr_locals(l, out);
                collect_expr_locals(r, out);
            }
            Statement::ArrowAssign { target, value, .. } => {
                if let Some(t) = target {
                    collect_expr_locals(t, out);
                }
                collect_expr_locals(value, out);
            }
            Statement::Term(Some(e)) | Statement::Check(e) | Statement::Gate(e)
            | Statement::Expression(e) => collect_expr_locals(e, out),
            Statement::EndProgram(Some(e)) | Statement::Rollback(Some(e)) => collect_expr_locals(e, out),
            _ => {}
        }
    }
}

fn collect_expr_locals(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Block(stmts) => collect_stmt_locals(stmts, out),
        Expr::Call(_, args, _) | Expr::Tuple(args) | Expr::List(args) => {
            for a in args { collect_expr_locals(a, out); }
        }
        Expr::BinaryOp(_, l, r) | Expr::Within(l, r) => {
            collect_expr_locals(l, out);
            collect_expr_locals(r, out);
        }
        Expr::UnaryOp(_, i) | Expr::Deref(i) | Expr::AddrOf(i) | Expr::Consume(i)
        | Expr::Await(i) | Expr::Cast(i, _) | Expr::IsType(i, _)
        | Expr::Capture { expr: i, .. } | Expr::Reflect(i, _, _) => collect_expr_locals(i, out),
        Expr::Field(o, _) => collect_expr_locals(o, out),
        Expr::Index(o, i) => {
            collect_expr_locals(o, out);
            collect_expr_locals(i, out);
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            collect_expr_locals(recv, out);
            for a in args { collect_expr_locals(a, out); }
        }
        Expr::Slice { array, start, end, stride } => {
            collect_expr_locals(array, out);
            if let Some(s) = start { collect_expr_locals(s, out); }
            if let Some(e) = end { collect_expr_locals(e, out); }
            if let Some(s) = stride { collect_expr_locals(s, out); }
        }
        Expr::Range { start, end, .. } => {
            collect_expr_locals(start, out);
            collect_expr_locals(end, out);
        }
        Expr::Spawn { args, .. } => { for a in args { collect_expr_locals(a, out); } }
        Expr::If(c, t, e) => {
            collect_expr_locals(c, out);
            collect_expr_locals(t, out);
            if let Some(els) = e { collect_expr_locals(els, out); }
        }
        Expr::Match(scrut, arms) => {
            collect_expr_locals(scrut, out);
            for arm in arms {
                if let Some(g) = &arm.guard { collect_expr_locals(g, out); }
                collect_expr_locals(&arm.body, out);
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) { collect_expr_locals(e, out); }
        }
        Expr::Lambda(_, body) => collect_expr_locals(body, out),
        Expr::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver { collect_expr_locals(r, out); }
            for a in args { collect_expr_locals(a, out); }
        }
        _ => {}
    }
}

fn stmt_needs_state(
    stmt: &Statement,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always_stateful: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    // 2026-09-23 (stateless-defn mechanism): this walker gates whether a
    // defn is emitted WITHOUT `%state`. A missed expression here marks a
    // stateful defn stateless → undefined `%state` in the IR (opt rejects).
    // Every expression-bearing variant must be visited, not just the body.
    let any_expr = |e: &Expr| expr_needs_state(e, regular, txns, state_fields, always_stateful, locals, needs);
    match stmt {
        Statement::Term(opt)
        | Statement::EndProgram(opt)
        | Statement::Rollback(opt) => {
            opt.as_ref().is_some_and(|e| any_expr(e))
        }
        Statement::Expression(expr) => any_expr(expr),
        Statement::Check(expr) | Statement::Gate(expr) => any_expr(expr),
        Statement::Let { expr, .. } => {
            expr.as_ref().is_some_and(|e| any_expr(e))
        }
        // 2026-08-03 (node bridge): the assignment TARGET may be a state field
        // (`saved = name;`) — check it too, not just the RHS.
        Statement::Assign(lhs, expr) => {
            any_expr(lhs) || any_expr(expr)
        }
        Statement::ArrowAssign { target, value, .. } => {
            // 2026-09-23 (stateless-defn mechanism): `<-` on an Event wire
            // lowers DIRECTLY to briev_event_fire_impl(ptr %state, ...) —
            // a hardcoded state emission the call-graph walk cannot see
            // (the callee is not a `Call` node). Conservative: any arrow
            // assign forces state. (Pure exports using `<-` are rare; the
            // cstr doors do not.)
            let _ = (target, value);
            true
        }
        // 2026-09-23: the CONDITION is an expression too — a stateful call in
        // a `when` guard (`when bytes_equal_at(...)`) was previously invisible
        // (the arm ignored it), marking a stateful defn stateless.
        Statement::Guarded(cond, body) => {
            any_expr(cond)
                || body.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
        }
        Statement::Foreach { list, body, .. } => {
            any_expr(list)
                || body.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
        }
        Statement::Match { expr, arms } => {
            any_expr(expr)
                || arms.iter().any(|arm| {
                    arm.body.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
                })
        }
        Statement::TrgBinding { instance, .. } => any_expr(instance),
        // 2026-09-22 (D16 p3b): `open` records disconnection, but its pin
        // expressions can still name state fields; visit them conservatively.
        Statement::Open(lhs, rhs) => any_expr(lhs) || any_expr(rhs),
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            body.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
        }
        // No runtime state: pure terminators, lifetime hints, and compile-time
        // metadata. (`trap`/`halt` never fall through; `yield` is a scheduler
        // point, not a state access.)
        Statement::Break
        | Statement::Trap
        | Statement::Halt
        | Statement::Yield
        | Statement::MetadataAssignment(..)
        | Statement::FreeHint(_)
        | Statement::KeepHint(_)
        | Statement::InlineAsm { .. }
        | Statement::InlineDefn(_) => false,
    }
}

fn expr_needs_state(
    expr: &Expr,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always_stateful: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    // 2026-09-23 (stateless-defn mechanism): this walker decides whether a
    // defn can be emitted WITHOUT `%state`. A missed inner expression would
    // mark a stateful defn stateless → undefined `%state` in the IR. Every
    // variant that can carry a sub-expression is therefore visited; only
    // genuine literals / no-runtime-state markers return false.
    let rec = |e: &Expr| expr_needs_state(e, regular, txns, state_fields, always_stateful, locals, needs);
    let rec_stmts = |stmts: &[Statement]| {
        stmts.iter().any(|s| stmt_needs_state(s, regular, txns, state_fields, always_stateful, locals, needs))
    };
    match expr {
        // Field access always needs state (reads struct metadata)
        Expr::Field(_, _) => true,
        Expr::Call(name, args, _) => {
            // Observable/stateful intrinsics need state (unchanged from the
            // backend's original list).
            let name_needs = if matches!(name.as_str(),
                "Malloc#" | "Memcpy#" | "Memmove#" | "Memset#"
                | "Print#"
                | "FileRead#" | "FileWrite#" | "ShellCmd#"
                | "SysQuery#" | "EnvGet#" | "HttpFetch#"
                | "AllocArray#" | "AllocInitArray#" | "StringNew#"
                | "StringFromPtr#" | "StringConcat#"
                // 2026-09-23 (stateless-defn mechanism): intrinsics whose
                // lowering emits `ptr %state` directly (task-segment dispatch,
                // cwd read). Missing these let a stateful defn (await_one_pass
                // calling TaskCall#) be marked stateless → undefined %state.
                | "TaskCall#" | "GetCwd#" | "ChDir#"
            ) {
                true
            } else if txns.contains(name.as_str()) {
                // Transactions always carry %state (the reactor threads it) —
                // a caller must supply it.
                true
            } else if always_stateful.contains(name.as_str()) {
                // 2026-09-23 (stateless-defn mechanism): `bad fn` / `asm fn`
                // take the implicit %state first ABI arg (compiled out of
                // band) — a call forces state on the caller.
                true
            } else if regular.contains_key(name.as_str()) {
                // 2026-09-23 (stateless-defn mechanism): a regular defn
                // forces state on its callers ONLY when it itself (or a
                // transitively-called defn) needs state. Pure helpers (cstr
                // doors over Load#/Alloc#, arithmetic lanes) stay stateless
                // and keep GLUE exports on a clean C ABI. Under the least
                // fixpoint, `needs` already holds the current approximation
                // for the callee.
                *needs.get(name.as_str()).unwrap_or(&true)
            } else {
                // frgn / other external calls do not need state at the boundary
                // — BUT their arguments may read/write state fields. The
                // marshalling rewrites `term saved;` → `term str_to_c(saved);`
                // (the CStr<->String meld), so a bare state-field read becomes
                // a frgn call ARG. Check the args too.
                false
            };
            name_needs || args.iter().any(|a| rec(a))
        }
        Expr::BinaryOp(_, lhs, rhs) => rec(lhs) || rec(rhs),
        Expr::UnaryOp(_, inner) => rec(inner),
        Expr::List(items) | Expr::Tuple(items) => items.iter().any(|e| rec(e)),
        // 2026-08-04 (compiler-in-Briev): wrapping expression kinds that can
        // HIDE a stateful inner — a cast-wrapped call (`token_at(t, 1) as Int`),
        // a method call on a state-field receiver, an index/slice/addr-of of a
        // state field. Previously the `_ => false` arm made these invisible, so
        // an export calling a regular defn through a cast got a STATELESS shim
        // that referenced `%state` (opt: "use of undefined value '%state'").
        Expr::Cast(inner, _) | Expr::IsType(inner, _) => rec(inner),
        Expr::MethodCall(recv, _, args, _, _) => {
            rec(recv) || args.iter().any(|a| rec(a))
        }
        Expr::Reflect(recv, _, _) => rec(recv),
        Expr::Index(arr, idx) => rec(arr) || rec(idx),
        Expr::Slice { array, start, end, stride } => {
            rec(array)
                || start.as_ref().is_some_and(|e| rec(e))
                || end.as_ref().is_some_and(|e| rec(e))
                || stride.as_ref().is_some_and(|e| rec(e))
        }
        Expr::Range { start, end, .. } => rec(start) || rec(end),
        Expr::Spawn { args, .. } => {
            // 2026-09-23 (stateless-defn mechanism): `spawn` lowers to
            // briev_task_spawn_impl + event helpers with `%state` — always
            // stateful, regardless of the args' own state use.
            true
        }
        Expr::Await(inner) => {
            // `await` lowers to briev_await_impl(ptr %state, ...) — stateful.
            let _ = rec(inner);
            true
        }
        Expr::Block(stmts) => rec_stmts(stmts),
        Expr::If(cond, then, els) => {
            rec(cond) || rec(then) || els.as_ref().is_some_and(|e| rec(e))
        }
        Expr::Match(scrut, arms) => {
            rec(scrut)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(|g| rec(g)) || rec(&arm.body)
                })
        }
        Expr::StructLiteral { fields, specs, .. } => {
            fields.iter().chain(specs.iter()).any(|(_, e)| rec(e))
        }
        Expr::Lambda(_, body) => rec(body),
        Expr::Within(a, b) => rec(a) || rec(b),
        Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::Consume(inner) => {
            rec(inner)
        }
        Expr::PluginIntercept { args, receiver, .. } => {
            receiver.as_ref().is_some_and(|r| rec(r)) || args.iter().any(|a| rec(a))
        }
        Expr::Capture { expr, .. } => rec(expr),
        // Derivation blocks are compile-time construction scaffolding — treat
        // conservatively (they may reference state through field derivation).
        Expr::DerivationBlock(_) => true,
        // 2026-08-03 (node bridge): a bare read of a state field needs the
        // `%state` handle even though no intrinsic/call is involved
        // (`term saved;`). 2026-09-23 (scoping): a defn's own param/local
        // SHADOWS a same-named state field (`byte_at(s, i)` with a global
        // `let i`) — a param reference is not a state access.
        Expr::Identifier(name) => state_fields.contains(name) && !locals.contains(name),
        // Literals / no-runtime-state markers.
        Expr::Quoted(_)
        | Expr::Decimal(_)
        | Expr::Char(_)
        | Expr::TaggedLiteral(_, _)
        | Expr::TaggedQuotedLiteral(_, _)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::BeginProgram
        | Expr::FormattingAnnotation(_)
        | Expr::Exists(_)
        | Expr::UnitLiteral { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defn(name: &str, body: Vec<Statement>) -> Definition {
        use crate::ast::{Contract, OutputType, TypeParam};
        Definition {
                variadic_param: None,
            name: name.to_string(),
            type_params: Vec::<TypeParam>::new(),
            parameters: vec![],
            output_type: Some(OutputType::Single(crate::ast::Type::Custom("Int".to_string()))),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        }
    }

    fn exported(d: Definition) -> TopLevel {
        use crate::ast::Export;
        TopLevel::Export(Export {
            inner: Box::new(TopLevel::Definition(d)),
            export_name: None,
        })
    }

    #[test]
    fn pure_export_needs_no_state() {
        let d = defn("add", vec![Statement::Term(Some(Expr::Decimal(5)))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("add"), Some(&false));
    }

    #[test]
    fn export_calling_stateful_intrinsic_needs_state() {
        let d = defn("f", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn export_calling_pure_regular_defn_needs_no_state() {
        // 2026-09-23 (stateless-defn mechanism): a PURE regular defn stays
        // stateless, so its caller does too — the cstr-door property that
        // keeps GLUE exports on a clean C ABI.
        let helper = defn("helper", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::Call("helper".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Definition(helper), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&false));
    }

    #[test]
    fn export_calling_stateful_regular_defn_needs_state() {
        // A helper that touches a stateful intrinsic forces its caller to
        // carry state (transitive).
        let helper = defn("helper", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::Call("helper".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Definition(helper), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn guard_condition_stateful_call_forces_state() {
        // 2026-09-23: a stateful call in a `when` CONDITION must be seen —
        // the walker previously ignored the guard condition and marked the
        // defn stateless (undefined %state in the IR).
        let helper = defn("helper", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let caller = defn("f", vec![Statement::Guarded(
            Expr::Call("helper".to_string(), vec![], None),
            vec![Statement::Term(Some(Expr::Decimal(1)))],
        )]);
        let items = vec![TopLevel::Definition(helper), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn stateless_defn_map_includes_all_defns() {
        // compute_defn_needs_state covers regular defns too (the emission
        // gate), not just exports.
        let pure = defn("pure_helper", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let impure = defn("impure_helper", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Definition(pure), TopLevel::Definition(impure)];
        let map = compute_defn_needs_state(&items);
        assert_eq!(map.get("pure_helper"), Some(&false));
        assert_eq!(map.get("impure_helper"), Some(&true));
    }

    #[test]
    fn pure_export_calling_pure_export_needs_no_state() {
        let inner = defn("inner", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let outer = defn("outer", vec![Statement::Term(Some(
            Expr::Call("inner".to_string(), vec![], None),
        ))]);
        let items = vec![exported(inner), exported(outer)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("inner"), Some(&false));
        assert_eq!(map.get("outer"), Some(&false));
    }

    #[test]
    fn export_calling_frgn_needs_no_state() {
        // cstr_to_briev is a frgn, not a Briev defn → no state.
        let d = defn("f", vec![Statement::Term(Some(
            Expr::Call("cstr_to_briev".to_string(), vec![], None),
        ))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&false));
    }

    #[test]
    fn mutual_recursion_of_pure_defns_is_stateless() {
        // 2026-09-23 (stateless-defn mechanism): under the least fixpoint, a
        // mutual-recursion cycle of PURE defns stays stateless — a cycle alone
        // forces nothing (the old DFS `visiting ⇒ true` rule wrongly marked
        // self-recursive stateless defns like cstr_len as stateful). Only a
        // genuinely stateful leaf propagates state up through the cycle.
        let a = defn("a", vec![Statement::Term(Some(
            Expr::Call("b".to_string(), vec![], None),
        ))]);
        let b = defn("b", vec![Statement::Term(Some(
            Expr::Call("a".to_string(), vec![], None),
        ))]);
        let items = vec![exported(a), exported(b)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("a"), Some(&false));
        assert_eq!(map.get("b"), Some(&false));
    }

    #[test]
    fn mutual_recursion_reaching_state_is_stateful() {
        // A cycle where ONE member reaches state flips the whole SCC (the
        // fixpoint propagates through the cycle edges).
        let leaf = defn("leaf", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let a = defn("a", vec![Statement::Term(Some(
            Expr::Call("b".to_string(), vec![], None),
        ))]);
        let b = defn("b", vec![Statement::Term(Some(
            Expr::Call("a".to_string(), vec![], None),
        ))]);
        // `a` is in the cycle AND reaches the stateful leaf — the SCC flips.
        // (Both `a` and `b` are exported; `a` also calls `leaf`, so the
        // fixpoint propagates state into the cycle.)
        let a = defn("a", vec![
            Statement::Term(Some(Expr::Call("b".to_string(), vec![], None))),
            Statement::Term(Some(Expr::Call("leaf".to_string(), vec![], None))),
        ]);
        let b = defn("b", vec![Statement::Term(Some(
            Expr::Call("a".to_string(), vec![], None),
        ))]);
        let items = vec![
            TopLevel::Definition(leaf),
            exported(a),
            exported(b),
        ];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("a"), Some(&true));
        assert_eq!(map.get("b"), Some(&true));
    }

    #[test]
    fn export_calling_txn_needs_state() {
        // Transactions always carry %state — a caller must supply it.
        use crate::ast::{Contract, PropertyValue, Transaction, TypeParam};
        use crate::errors::Span;
        let txn = Transaction {
            name: "loop_txn".to_string(),
            is_reactive: false,
            is_async: false,
            type_params: Vec::<TypeParam>::new(),
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: vec![],
            metadata: std::collections::HashMap::<String, PropertyValue>::new(),
            derivation: None,
            modifiers: vec![],
            span: Option::<Span>::None,
            doc: None,
        };
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::Call("loop_txn".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Transaction(txn), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }
}
