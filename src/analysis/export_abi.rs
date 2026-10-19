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
pub fn compute_defn_needs_state(items: &[TopLevel], plain: &HashSet<String>) -> HashMap<String, bool> {
    // 2026-10-17 (plain storage): plain lets are NOT machine state — a defn
    // touching only plain lets stays stateless (the print chain drops
    // `ptr %state`, unblocking SROA promotion of %State in printing loops).
    // Excluded at the index so the least fixpoint never sees them as
    // stateful leaves.
    let (regular, txns, state_fields, always_stateful) = index_program(items, plain);
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
fn index_program<'a>(
    items: &'a [TopLevel],
    plain: &HashSet<String>,
) -> (
    HashMap<String, &'a Definition>,
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
            // 2026-10-15 (type-dependent member statefulness, found by Design
            // D's GrowBuf): obj MEMBER defns were never indexed here, so a
            // member reaching a stateful intrinsic was invisible to the
            // needs-state fixpoint. A trait default (`put_cstr`) calling a
            // required method (`put_bytes`) by bare name saw `put_bytes` as an
            // external/frgn call → stateless, even when the concrete member
            // (`GrowBuf.put_bytes` → `grow_to_fit` → `Malloc#`) is stateful.
            // Index each member under a TYPE-QUALIFIED key (`GrowBuf::put_bytes`)
            // so ByteBuf's stateless members and GrowBuf's stateful members are
            // DISTINCT fixpoint nodes (a bare-name key would collide them). The
            // member body's calls to free-standing defns (grow_to_fit) resolve
            // through `regular` normally, so the Malloc# path becomes visible.
            // The monomorphization site reads these per-type verdicts (via
            // `type_stateful_members`) to give `put_cstr$GrowBuf` the right
            // state-taking verdict while `put_cstr$ByteBuf` stays stateless.
            TopLevel::TypeDef(td) => {
                index_type_members(&td.name, &td.body.members, &mut regular);
            }
            TopLevel::Impl(i) => {
                index_impl_members(&i.target, &i.functions, &mut regular);
            }
            // 2026-10-15 (fix: Monomorphized trait DEFAULT method wrongly
            // flagged state-taking, BUGS.md): a trait DEFAULT function (a trait
            // fn WITH a body) is a `Definition` the backend monomorphizes per
            // conforming type (`put_two` → `put_two$FixedSink`). The backend
            // registers the mono copy's `needs_state` from the ORIGINAL
            // (unmangled) name's verdict (emit_expr.rs), but the original
            // trait-default name was NEVER indexed here (it lives inside a
            // `TopLevel::Trait`, not as a `TopLevel::Definition`), so the lookup
            // missed and both the call-site registration and the S6 emitter
            // (emit_toplevel.rs) fell back to `unwrap_or(true)` = state-taking.
            // Its stateless caller then supplied no `%state` → undefined
            // `%state` in the IR. Indexing the default here derives its verdict
            // through the same least-fixpoint call-graph walk every other defn
            // gets (a pure default that only calls sibling members + constructs
            // a struct is stateless; one that touches a state field or calls a
            // txn/stateful intrinsic correctly stays state-taking). Required
            // (body-less) trait fns are NOT indexed — they ARE the concrete
            // type's own members, resolved by their obj registration. To undo:
            // remove this arm (the mono copy reverts to `unwrap_or(true)`).
            TopLevel::Trait(t) => {
                for func in &t.functions {
                    // `or_insert` mirrors the backend's "a real top-level defn
                    // of this name wins" rule (mod.rs S6 registration): a
                    // concrete defn already in `regular` is not shadowed by a
                    // same-named trait default.
                    if !func.body.is_empty() {
                        regular.entry(func.name.clone()).or_insert(func);
                    }
                }
            }
            // 2026-08-03: transactions always carry `ptr %state` (the reactor
            // threads state through them) — a caller must supply it.
            TopLevel::Transaction(t) => {
                txns.insert(t.name.clone());
            }
            TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, .. } = stmt.as_ref() {
                    // 2026-10-17 (plain storage): plain lets are not machine
                    // state — excluded from the needs-state leaf set.
                    if !plain.contains(name) {
                        state_fields.insert(name.clone());
                    }
                }
            }
            // 2026-10-17 (plain storage): CONSTANTS are not machine state —
            // they lower to immutable `@name` constant loads, no %State
            // access. Their presence here forced `stdout_byte` stateful via
            // its `STDOUT_CAP` read (the buffered-stdout regression's last
            // link). To undo: restore this insert.
            TopLevel::Constant(_) => {}
            TopLevel::StateDecl(s) => {
                state_fields.insert(s.name.clone());
            }
            _ => {}
        }
    }
    (regular, txns, state_fields, always_stateful)
}

/// 2026-10-15 (type-dependent member statefulness): the fixpoint key for an
/// obj member — `Type::member`. Keeping members under a type-qualified key
/// (never the bare member name) is what lets `ByteBuf::put_bytes` (stateless)
/// and `GrowBuf::put_bytes` (stateful) be DISTINCT nodes. `type_member_key`
/// is the single source of this format so the producer (indexing) and the
/// consumer (`type_stateful_members`) cannot drift.
pub fn type_member_key(type_name: &str, member: &str) -> String {
    format!("{type_name}::{member}")
}

/// 2026-10-19 (dot-call statefulness): does the method named `method` need
/// state on AT LEAST ONE indexed type? A `recv.method(args)` call site has no
/// resolved receiver type in the AST at this analysis stage, so we take the
/// union over every indexed `Type::method` member's current fixpoint verdict.
/// Sound over-approximation: if no type's `method` is stateful the call is
/// treated stateless (precise); if one is, every dot-call of that name is
/// treated stateful (conservative — a harmless unused `%state` param at worst,
/// the pass's standing stance). `regular` is scanned for member keys of the
/// shape `*::method`; `needs` holds the current iteration's verdicts.
fn method_is_stateful_on_any_type(
    method: &str,
    regular: &HashMap<String, &Definition>,
    needs: &HashMap<String, bool>,
) -> bool {
    let suffix = format!("::{method}");
    regular.keys().any(|key| {
        key.ends_with(&suffix) && *needs.get(key).unwrap_or(&false)
    })
}

/// Index an obj type's member defns under type-qualified keys. Only body-ful
/// `Definition` members are nodes (a body-less required-method stub carries no
/// calls); member txns are reactor-threaded and handled by the txn set.
fn index_type_members<'a>(
    type_name: &str,
    members: &'a [TopLevel],
    regular: &mut HashMap<String, &'a Definition>,
) {
    for member in members {
        if let TopLevel::Definition(d) = member {
            if !d.body.is_empty() {
                regular.insert(type_member_key(type_name, &d.name), d);
            }
        }
    }
}

/// Index an `impl Target { … }` block's functions under type-qualified keys
/// (same treatment as inherent obj members — inherent behavior attached to a
/// data-only declaration).
fn index_impl_members<'a>(
    target: &str,
    functions: &'a [Definition],
    regular: &mut HashMap<String, &'a Definition>,
) {
    for d in functions {
        if !d.body.is_empty() {
            regular.insert(type_member_key(target, &d.name), d);
        }
    }
}

/// 2026-10-15 (type-dependent member statefulness): the set of concrete type
/// names that have AT LEAST ONE state-taking member, derived from the same
/// least-fixpoint as [`compute_defn_needs_state`]. The monomorphization site
/// consults this: a trait default/generic specialized to a type whose members
/// are stateful (e.g. `put_cstr$GrowBuf`, because `GrowBuf.put_u8/put_bytes`
/// reach `Malloc#` via `grow_to_fit`) must itself take `%state`, while the
/// same default on a stateless type (`put_cstr$ByteBuf`) stays stateless.
///
/// Over-approximation is deliberate and sound: a specialization gets a (valid,
/// threaded) `%state` param even if it happens not to call the stateful member
/// — the pass's standing "conservative keeping is harmless" stance. The ALT of
/// walking each specialized body to resolve exactly which members it calls was
/// rejected as more fragile for no observable benefit (an unused leading
/// pointer param is free). Keyed by bare type name (`GrowBuf`), the shape the
/// backend's `subst` yields.
pub fn type_stateful_members(items: &[TopLevel]) -> HashSet<String> {
    let per_defn = compute_defn_needs_state(items, &crate::analysis::storage_class::classify_plain_lets(items, None, &HashSet::new()));
    let mut out: HashSet<String> = HashSet::new();
    for (key, needs) in &per_defn {
        if !needs {
            continue;
        }
        if let Some((type_name, _member)) = key.split_once("::") {
            out.insert(type_name.to_string());
        }
    }
    out
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
pub fn compute_export_needs_state(items: &[TopLevel], plain: &HashSet<String>) -> HashMap<String, bool> {
    let all = compute_defn_needs_state(items, plain);
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
            Statement::Term(Some(e)) | Statement::Tail(e) | Statement::Check(e) | Statement::Gate(e)
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
        Statement::Tail(e) => any_expr(e),
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
        // 2026-09-28 (E3): the `[*]` selector carries no runtime state — it is
        // a leaf, resolved by eager expansion before codegen.
        Expr::Wildcard => false,
        // 2026-10-15 (field-access scoping): a field read needs `%state` iff
        // its BASE chain does. The previous `Expr::Field(_, _) => true` was
        // unconditional — it predates the 2026-09-23 param-shadowing scoping
        // (see the Identifier arm) and was never migrated, so ANY member
        // touching a field of its receiver (`Store#(sink.ptr, val, 1)`, the
        // bread-and-butter of every obj member) was judged state-taking even
        // though the load reads the param's pointer, never `%state`. Sound
        // but over-conservative: it blanket-defeated the stateless-defn
        // mechanism for all obj members. Now: `sink.ptr` → rec(Identifier
        // sink) → param → false; `global.field` → rec(Identifier global) →
        // state field → true (the dde481af protection is preserved — a bare
        // state-field base still forces state). Every base shape (Call,
        // MethodCall, Index, Deref, Cast, StructLiteral, …) carries its own
        // recursive arm, so the base chain is fully walked. Undo: restore
        // `Expr::Field(_, _) => true` (re-introduces the member-wide
        // statefulness over-approximation; their test
        // obj_member_reaching_stateful_intrinsic_is_indexed_stateful fails).
        Expr::Field(recv, _) => rec(recv),
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
        // 2026-10-19 (fix: dot-call to a stateful obj member left the caller
        // stateless → undefined %state, BUGS.md): a `recv.method(args)` call
        // reaches the concrete member `Type::method`, whose statefulness the
        // 2026-10-15 indexing already computes under a type-qualified key. The
        // previous arm only walked `recv` and `args` — it never consulted the
        // member's own verdict, so `sink.put_bytes(src, n)` where
        // `GrowBuf::put_bytes` reaches `Malloc#` via `grow_to_fit` was judged
        // stateless; its caller (`put_str` → the response writers) then got no
        // `%state` param while the inlined member body referenced `%state`.
        // The MethodCall AST carries no resolved receiver type here, so we OR
        // in whether the method is stateful on ANY indexed type — the same
        // sound over-approximation `type_stateful_members` already relies on
        // ("an unused leading pointer param is free"). Undo: restore
        // `rec(recv) || args.iter().any(|a| rec(a))` (reopens the undefined
        // `%state` class for dot-called stateful members).
        Expr::MethodCall(recv, method, args, _, _) => {
            rec(recv)
                || args.iter().any(|a| rec(a))
                || method_is_stateful_on_any_type(method, regular, needs)
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
        let map = compute_export_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("add"), Some(&false));
    }

    #[test]
    fn export_calling_stateful_intrinsic_needs_state() {
        let d = defn("f", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn dot_call_to_stateful_member_forces_state() {
        // 2026-10-19 (PR #24 pin): a dot-call to a stateful obj member must
        // force %state on the CALLER. The MethodCall form previously bypassed
        // the needs-state walk — the inlined member body referenced %state
        // while the caller was emitted without it (undefined %state IR). The
        // free-call and dot-call forms of the same callee must agree.
        let src = r#"obj Sink {
            defn grow_member(s: Sink) -> Int { term StringNew#(); };
        };"#;
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let mut items = p.parse_program().unwrap();
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::MethodCall(
                Box::new(Expr::Identifier("sink".to_string())),
                "grow_member".to_string(),
                vec![],
                None,
                Vec::new(),
            ),
        ))]);
        items.push(exported(caller));
        let map = compute_export_needs_state(&items, &HashSet::new());
        assert_eq!(
            map.get("f"),
            Some(&true),
            "dot-call to a stateful member must propagate state: {map:?}"
        );
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
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_defn_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
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
        let map = compute_export_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("a"), Some(&true));
        assert_eq!(map.get("b"), Some(&true));
    }

    #[test]
    fn export_calling_txn_needs_state() {
        // Transactions always carry %state — a caller must supply it.
        use crate::ast::{Contract, PropertyValue, Transaction, TypeParam};
        use crate::errors::Span;
        let txn = Transaction {
            variadic_param: None,
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
        let map = compute_export_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("f"), Some(&true));
    }

    // 2026-10-15 (fix: Monomorphized trait DEFAULT method wrongly flagged
    // state-taking, BUGS.md): index_program now indexes trait DEFAULT methods
    // (body-carrying trait fns) into the needs-state call graph, so the
    // monomorphized copy (`put_two$FixedSink`) inherits the ORIGINAL default's
    // DERIVED verdict instead of falling back to state-taking. These tests
    // pin that behaviour at the analysis layer where the fix lives.

    /// Build a `TopLevel::Trait` named `trait_name` with one DEFAULT function
    /// (`func_name`, carrying `body`) — mirrors a `trait Sink { defn put_two(..)
    /// { .. } }`. Required (body-less) members are not modelled; they are the
    /// concrete type's own obj members, not needs-state call-graph nodes.
    fn trait_with_default(trait_name: &str, func_name: &str, body: Vec<Statement>) -> TopLevel {
        use crate::ast::top::TraitDef;
        TopLevel::Trait(TraitDef {
            name: trait_name.to_string(),
            type_params: vec![],
            functions: vec![defn(func_name, body)],
            op_bindings: vec![],
            fields: vec![],
            span: None,
        })
    }

    #[test]
    fn pure_trait_default_is_stateless() {
        // The repro default: `put_two` only calls a sibling REQUIRED member
        // (`put_one`, resolved per concrete type — NOT a regular defn, txn, or
        // stateful intrinsic) over its own params. It touches no program
        // state, so its verdict must be `false`. Before the fix the default's
        // base name was never indexed, so the lookup missed and the
        // monomorphized `put_two$FixedSink` was emitted state-taking while its
        // stateless caller supplied none → undefined `%state`.
        let put_two = trait_with_default(
            "Sink",
            "put_two",
            vec![Statement::Term(Some(Expr::Call(
                "put_one".to_string(),
                vec![Expr::Call(
                    "put_one".to_string(),
                    vec![
                        Expr::Identifier("sink".to_string()),
                        Expr::Identifier("val".to_string()),
                    ],
                    None,
                ),
                Expr::Identifier("val".to_string())],
                None,
            )))],
        );
        let map = compute_defn_needs_state(&[put_two], &HashSet::new());
        assert_eq!(map.get("put_two"), Some(&false));
    }

    #[test]
    fn stateful_trait_default_keeps_state() {
        // Gate D: a trait DEFAULT that reaches program state (here a stateful
        // intrinsic) MUST remain state-taking — the fix derives the verdict,
        // it does not blanket-flip defaults to stateless.
        let put_logged = trait_with_default(
            "Sink",
            "put_logged",
            vec![Statement::Term(Some(Expr::Call(
                "StringNew#".to_string(),
                vec![],
                None,
            )))],
        );
        let map = compute_defn_needs_state(&[put_logged], &HashSet::new());
        assert_eq!(map.get("put_logged"), Some(&true));
    }

    #[test]
    fn trait_default_state_propagates_to_caller() {
        // Transitivity: a generic defn that calls a STATELESS trait default
        // stays stateless; one that calls a STATEFUL default becomes stateful.
        // This is the chain `write_pair → put_two` that the mono emitter walks.
        let pure_default = trait_with_default(
            "Sink",
            "put_two",
            vec![Statement::Term(Some(Expr::Call(
                "put_one".to_string(),
                vec![Expr::Identifier("sink".to_string())],
                None,
            )))],
        );
        let caller = defn(
            "write_pair",
            vec![Statement::Term(Some(Expr::Call(
                "put_two".to_string(),
                vec![Expr::Identifier("sink".to_string())],
                None,
            )))],
        );
        let items = vec![pure_default, TopLevel::Definition(caller)];
        let map = compute_defn_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("put_two"), Some(&false));
        assert_eq!(map.get("write_pair"), Some(&false));
    }

    #[test]
    fn concrete_defn_wins_over_same_named_trait_default() {
        // The `or_insert` precedence: a real top-level defn of the same name as
        // a trait default is NOT shadowed by the default's body (mirrors the
        // backend's "a real top-level defn of this name wins" S6 rule). Here a
        // STATELESS concrete `put` must not be overwritten by a STATEFUL
        // same-named default.
        let concrete = defn("put", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let default = trait_with_default(
            "Sink",
            "put",
            vec![Statement::Term(Some(Expr::Call(
                "StringNew#".to_string(),
                vec![],
                None,
            )))],
        );
        let items = vec![TopLevel::Definition(concrete), default];
        let map = compute_defn_needs_state(&items, &HashSet::new());
        assert_eq!(map.get("put"), Some(&false));
    }

    // 2026-10-15 (type-dependent member statefulness, found by Design D's
    // GrowBuf): obj member defns are now indexed under type-qualified keys so a
    // member reaching a stateful intrinsic is visible, and `type_stateful_members`
    // reports which concrete types have such a member. The monomorphization site
    // (emit_expr.rs) ORs that into a trait default/generic's per-instantiation
    // verdict so `put_cstr$GrowBuf` is state-taking while `put_cstr$ByteBuf`
    // stays stateless. These tests pin the analysis the backend consumes.

    /// Parse real Briev source into top-level items — behavioral test input
    /// (AGENTS: test the contract, not the implementation).
    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    #[test]
    fn obj_member_reaching_stateful_intrinsic_is_indexed_stateful() {
        // A member whose body reaches a stateful intrinsic (Malloc#, via a
        // free-standing helper) is a type-qualified fixpoint node and is
        // judged state-taking; a sibling member that only does Store# is not.
        // Before the fix, obj members were never indexed, so BOTH were
        // invisible (and any default calling them was wrongly stateless).
        let src = r#"
            defn grow_it(addr: Int) -> Int { let fresh: Int = Malloc#(16) as Int; term fresh; };
            obj Fixed {
                ptr: Int;
                defn put(sink: Fixed, val: Int) -> Fixed {
                    Store#(sink.ptr, val, 1);
                    term Fixed { ptr: sink.ptr };
                };
            };
            obj Growing {
                ptr: Int;
                defn put(sink: Growing, val: Int) -> Growing {
                    let moved: Int = grow_it(sink.ptr);
                    Store#(moved, val, 1);
                    term Growing { ptr: moved };
                };
            };
        "#;
        let items = parse(src);
        let map = compute_defn_needs_state(&items, &HashSet::new());
        // Type-qualified member keys exist and carry the right verdict.
        assert_eq!(
            map.get(&type_member_key("Growing", "put")),
            Some(&true),
            "Growing.put reaches Malloc# via grow_it and must be state-taking"
        );
        assert_eq!(
            map.get(&type_member_key("Fixed", "put")),
            Some(&false),
            "Fixed.put only does Store# and must stay stateless"
        );
    }

    #[test]
    fn dot_call_to_stateful_member_forces_caller_state() {
        // 2026-10-19 (fix: dot-call to a stateful obj member left the caller
        // stateless → undefined %state): a free-standing defn that invokes a
        // stateful member through a DOT method call (`sink.put(val)`, where
        // `Growing::put` reaches Malloc# via grow_it) must itself be
        // state-taking. Before the fix the MethodCall arm walked only the
        // receiver and args, never the member's verdict, so the caller was
        // emitted without `%state` while the inlined member body referenced it.
        let src = r#"
            defn grow_it(addr: Int) -> Int { let fresh: Int = Malloc#(16) as Int; term fresh; };
            obj Growing {
                ptr: Int;
                defn put(sink: Growing, val: Int) -> Growing {
                    let moved: Int = grow_it(sink.ptr);
                    Store#(moved, val, 1);
                    term Growing { ptr: moved };
                };
            };
            defn app(sink: Growing, val: Int) -> Growing {
                term sink.put(val);
            };
        "#;
        let items = parse(src);
        let map = compute_defn_needs_state(&items, &HashSet::new());
        assert_eq!(
            map.get("app"),
            Some(&true),
            "app dot-calls Growing.put (reaches Malloc#) and must take %state"
        );
    }

    #[test]
    fn dot_call_to_stateless_member_keeps_caller_stateless() {
        // The soundness/precision counterpart: a free defn that dot-calls ONLY
        // a stateless member (Store# over its own param, no Malloc#) stays
        // stateless. Pins that the fix does not blanket-flip every dot call to
        // state-taking — only names stateful on some indexed type.
        let src = r#"
            obj Fixed {
                ptr: Int;
                defn put(sink: Fixed, val: Int) -> Fixed {
                    Store#(sink.ptr, val, 1);
                    term Fixed { ptr: sink.ptr };
                };
            };
            defn app(sink: Fixed, val: Int) -> Fixed {
                term sink.put(val);
            };
        "#;
        let items = parse(src);
        let map = compute_defn_needs_state(&items, &HashSet::new());
        assert_eq!(
            map.get("app"),
            Some(&false),
            "app dot-calls only the stateless Fixed.put and must stay stateless"
        );
    }

    #[test]
    fn type_stateful_members_reports_only_types_with_a_stateful_member() {
        // type_stateful_members: the set the monomorphization site consults.
        // Growing (member reaches Malloc#) is in it; Fixed (stateless member)
        // is not. This is what makes `put_cstr$GrowBuf` state-taking while
        // `put_cstr$ByteBuf` stays stateless at the specialization site.
        let src = r#"
            defn grow_it(addr: Int) -> Int { let fresh: Int = Malloc#(16) as Int; term fresh; };
            obj Fixed {
                ptr: Int;
                defn put(sink: Fixed, val: Int) -> Fixed {
                    Store#(sink.ptr, val, 1);
                    term Fixed { ptr: sink.ptr };
                };
            };
            obj Growing {
                ptr: Int;
                defn put(sink: Growing, val: Int) -> Growing {
                    let moved: Int = grow_it(sink.ptr);
                    Store#(moved, val, 1);
                    term Growing { ptr: moved };
                };
            };
        "#;
        let items = parse(src);
        let stateful = type_stateful_members(&items);
        assert!(stateful.contains("Growing"), "Growing has a stateful member");
        assert!(!stateful.contains("Fixed"), "Fixed has no stateful member");
    }

    #[test]
    fn global_state_field_member_access_stays_stateful() {
        // 2026-10-15 (field-access scoping control): the Field arm now walks
        // its base chain — this pins the SOUNDNESS direction of that change.
        // A member reading a field of a GLOBAL state field (`global.ptr`)
        // loads through `%state` and must stay state-taking; only param/local
        // bases are exempt (their pointers arrive as arguments). Reverting
        // the base-chain walk to `Expr::Field(_, _) => true` keeps this green
        // but blanket-stateful; replacing it with `_ => false` would break it
        // (the dde481af undefined-`%state` class).
        let src = r#"
            let counter: Fixed = Fixed { ptr: 0 };
            obj Fixed {
                ptr: Int;
                defn bump(sink: Fixed) -> Fixed {
                    let cur: Int = counter.ptr;
                    term Fixed { ptr: cur };
                };
            };
        "#;
        let items = parse(src);
        let map = compute_defn_needs_state(&items, &HashSet::new());
        assert_eq!(
            map.get(&type_member_key("Fixed", "bump")),
            Some(&true),
            "bump reads counter.ptr through the global state field and must \
             take %state"
        );
    }
}
