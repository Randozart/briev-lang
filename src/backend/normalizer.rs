// ── Backend Normalizer — Shared Helpers ───────────────────────────────
// 2026-07-14: Walks the AST and attaches backend-specific annotations.
// Shared across all backend normalizers. Max 2 nesting depth.
//
// 2026-10-05 (three-surfaces plan Phase 0.3): the intrinsic harvest moved
// onto `ast::visit` — the ONE exhaustive visitor (Rule 17). The old code
// had no `Statement::Let` arm and only walked statement-ROOT expressions,
// so `let x = Deref#()` and every nested intrinsic call (call arguments,
// cast/binary operands, match patterns, contract halves) bypassed the
// supported-set gate and reached the emitters — where several LLVM
// lowering paths index `args[0]` unguarded and the compiler PANICKED
// (BUGS.md, "intrinsic-arity panic class"). Undo: any change must keep
// `ast::visit` exhaustive (no `_` arms) — a new `Expr`/`Statement` variant
// fails to compile there by design.

use std::collections::HashSet;
use crate::ast::visit;
use crate::ast::*;

/// A collected intrinsic call from the AST.
#[derive(Debug, Clone)]
pub struct IntrinsicCall {
    pub name: String,
}

/// Walk the AST and collect all `Expr::Call` nodes whose name ends with
/// `'#'`, at any nesting depth (statement roots, initializers, call
/// arguments, contracts, pattern literals).
pub fn collect_intrinsic_calls(items: &[TopLevel]) -> Vec<IntrinsicCall> {
    let mut calls = Vec::new();
    for item in items {
        walk_toplevel(item, &mut |e| {
            if let Expr::Call(name, _, _) = e {
                if name.ends_with('#') {
                    calls.push(IntrinsicCall { name: name.clone() });
                }
            }
        });
    }
    calls
}

/// Validate that every intrinsic call in the REACHABLE program surface is in
/// the supported set.
///
/// 2026-09-09 (Family A/B, briev-native runtime): reachability-closed. The
/// prelude (prelude-native) injects cast_lanes.bv + float_fmt.bv into every
/// `.bv` unit; their defn bodies carry `Load#`/`Store#`/`SysCall#` that a
/// GPU/webstack unit never calls. Walking ALL items rejected uncalled
/// library code and failed every offload program. Dead code is not the
/// program (observability doctrine): the surface is txn bodies, exports,
/// constants, ISR bodies, top-level statements, and the members that
/// `analysis::defn_liveness` conservatively KEEPS (op/obj/cell/impl/trait
/// members — their dispatch names are mangled at emission, so they are
/// emitted whether or not a textual call site exists), closed over
/// transitively-called defns.
pub fn validate_intrinsics<'a>(
    items: &'a [TopLevel],
    supported: &HashSet<String>,
) -> Vec<String> {
    // Definitions by name for call-graph expansion. Every definition
    // STRUCTURALLY present in the unit is registered — member methods and
    // impl functions are reachable through dispatch by name (UFCS and op
    // members) — so a name collision keeps ALL bodies with that name.
    let mut bodies: std::collections::HashMap<&'a str, Vec<&'a Definition>> =
        std::collections::HashMap::new();
    collect_definitions(items, &mut bodies);

    let mut intrinsics: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut pending: Vec<Seed<'a>> = Vec::new();

    // Seed with the program surface (see the reachability note above).
    for item in items {
        seed_toplevel(item, &mut pending);
    }

    // BFS: expand defn calls until fixpoint. The queue is passed INTO the
    // harvest callback per walk (never captured), so popping a seed and
    // walking it never alias the same &mut.
    let mut harvester = Harvester {
        intrinsics: &mut intrinsics,
        visited: &mut visited,
        bodies: &bodies,
    };
    while let Some(seed) = pending.pop() {
        let mut cb = |e: &Expr| harvester.visit(&mut pending, e);
        match seed {
            Seed::Def(d) => visit::walk_definition(d, &mut cb),
            Seed::Txn(t) => {
                visit::walk_contract(&t.contract, &mut cb);
                visit::walk_stmts(&t.body, &mut cb);
            }
            Seed::Expr(e) => visit::walk_expr(e, &mut cb),
            Seed::Stmt(s) => visit::walk_stmts(std::slice::from_ref(s), &mut cb),
            Seed::Stmts(s) => visit::walk_stmts(s, &mut cb),
        }
    }

    let mut errors = Vec::new();
    for call in &intrinsics {
        if !supported.contains(call) {
            errors.push(format!("intrinsic '{}' is not supported by this backend", call));
        }
    }
    errors
}

/// A queued harvest target. `Def`/`Txn` carry their contract halves; the
/// statement/expr variants are plain surfaces.
enum Seed<'a> {
    Def(&'a Definition),
    Txn(&'a Transaction),
    Expr(&'a Expr),
    Stmt(&'a Statement),
    Stmts(&'a [Statement]),
}

/// Records intrinsic calls and enqueues transitively-called definition
/// bodies — the one place the gate decides what it has seen.
struct Harvester<'a, 'b> {
    intrinsics: &'b mut Vec<String>,
    visited: &'b mut HashSet<String>,
    bodies: &'b std::collections::HashMap<&'a str, Vec<&'a Definition>>,
}

impl<'a, 'b> Harvester<'a, 'b> {
    fn visit(&mut self, queue: &mut Vec<Seed<'a>>, e: &Expr) {
        // Both call shapes name a callee: `f(x)` and the method-dispatch
        // form `a.f(x)` (op members / UFCS — the name is the defn).
        let name = match e {
            Expr::Call(name, _, _) => name,
            Expr::MethodCall(_, name, _, _, _) => name,
            _ => return,
        };
        if name.ends_with('#') {
            self.intrinsics.push(name.clone());
            return;
        }
        if self.visited.contains(name) {
            return;
        }
        if let Some(defs) = self.bodies.get(name.as_str()) {
            self.visited.insert(name.clone());
            for d in defs {
                queue.push(Seed::Def(d));
            }
        }
    }
}

/// Register a slice of definitions by name (loop kept in its own frame so
/// callers stay at loop depth 1).
fn register_defs<'a>(
    defs: &'a [Definition],
    out: &mut std::collections::HashMap<&'a str, Vec<&'a Definition>>,
) {
    for d in defs {
        out.entry(d.name.as_str()).or_default().push(d);
    }
}

/// Register every definition in the unit — top level and nested members —
/// by name, for call-graph expansion.
fn collect_definitions<'a>(
    items: &'a [TopLevel],
    out: &mut std::collections::HashMap<&'a str, Vec<&'a Definition>>,
) {
    for item in items {
        match item {
            TopLevel::Definition(d) | TopLevel::TypeDefOperator(d)
            | TopLevel::CompileTimeDefn(d) => {
                out.entry(d.name.as_str()).or_default().push(d);
            }
            TopLevel::Trait(t) => register_defs(&t.functions, out),
            TopLevel::Impl(i) => register_defs(&i.functions, out),
            TopLevel::Cell(c) => register_defs(&c.definitions, out),
            TopLevel::TypeDef(td) => collect_definitions(&td.body.members, out),
            TopLevel::Export(e) => collect_definitions(std::slice::from_ref(&e.inner), out),
            TopLevel::Cfg(c) => collect_definitions(&c.items, out),
            TopLevel::SyncGroup { item, .. } => {
                collect_definitions(std::slice::from_ref(item), out)
            }
            TopLevel::Fuzzed { item, .. } => {
                collect_definitions(std::slice::from_ref(item), out)
            }
            _ => {}
        }
    }
}

/// Cell container: member transactions and internal triggers are reactive
/// roots; member DEFINITIONS are call-expanded, never seeded (see
/// `seed_toplevel`).
fn seed_cell<'a>(c: &'a crate::ast::top::CellDef, work: &mut Vec<Seed<'a>>) {
    for t in &c.transactions {
        work.push(Seed::Txn(t));
    }
    for t in &c.internal_triggers {
        work.push(Seed::Expr(&t.instance));
    }
}

/// Obj container: transactions are reactive roots; variant contracts ride
/// the type's instances.
fn seed_obj<'a>(o: &'a crate::ast::top::StructDefinition, work: &mut Vec<Seed<'a>>) {
    for t in &o.transactions {
        work.push(Seed::Txn(t));
    }
    for v in &o.variants {
        if let Some(c) = &v.contract {
            work.push(Seed::Expr(&c.pre_condition));
            work.push(Seed::Expr(&c.post_condition));
        }
    }
}

/// TypeDef: type-level expressions (parent, constraints, op_bindings,
/// projections, when-laws, modes) are emitted only while their TYPE is
/// live; without the liveness set at the gate, seeding them would reject
/// programs whose types are dead (over-seed = the prelude bug class).
/// They are reached through call expansion when actually used — the
/// remaining gap is dispatch-only op members (Phase 0.3 audit). Member
/// TRANSACTIONS are reactive roots (defn_liveness) — unconditional, so
/// they are seeded through the recursion; member definitions fall into
/// the no-op arm of `seed_toplevel`.
fn seed_typedef<'a>(td: &'a crate::ast::top::TypeDef, work: &mut Vec<Seed<'a>>) {
    for member in &td.body.members {
        seed_toplevel(member, work);
    }
}

/// Fuzzed-case bindings and expected values are evaluated by the fuzzer
/// harness; split out to keep `seed_toplevel` within the 100-line budget.
fn seed_fuzz_cases<'a>(
    cases: &'a [crate::ast::top::FuzzCase],
    work: &mut Vec<Seed<'a>>,
) {
    for case in cases {
        case.bindings.iter().for_each(|(_, e)| work.push(Seed::Expr(e)));
        work.push(Seed::Expr(&case.expected));
    }
}

/// Seed the harvest queue from one top-level item: every surface whose
/// expressions the emitters actually consume. Compile-time-only forms
/// (`$(stage)` blocks, `$defn`/`$let`/`$const`, `cfg` conditions, asm line
/// bodies) are reached through their expansion output or never reach
/// codegen, and are listed explicitly so the boundary is visible.
fn seed_toplevel<'a>(item: &'a TopLevel, work: &mut Vec<Seed<'a>>) {
    match item {
        // ── Emitted surfaces (seeded) ───────────────────────────────
        TopLevel::Transaction(t) => work.push(Seed::Txn(t)),
        TopLevel::IsrHandler(h) => work.push(Seed::Stmts(h.body.as_slice())),
        TopLevel::Statement(s) => work.push(Seed::Stmt(s)),
        TopLevel::Constant(c) => work.push(Seed::Expr(&c.expr)),
        TopLevel::Export(e) => seed_toplevel(&e.inner, work),
        TopLevel::Trigger(t) => work.push(Seed::Expr(&t.instance)),
        TopLevel::TriggerBinding { instance, .. } => work.push(Seed::Expr(instance)),
        TopLevel::Budget(b) => work.push(Seed::Expr(&b.contract)),
        TopLevel::Definition(_) | TopLevel::CompileTimeDefn(_) => {
            // Root definitions are the DEAD-code surface — reached only
            // through call expansion (bodies map), never seeded here.
        }
        TopLevel::TypeDefOperator(_) | TopLevel::CompileTimeTxn(_) => {
            // Op-member BODIES are reached through dispatch (method-call
            // expansion in the Harvester) — seeding them here would gate
            // dead members that `analysis::defn_liveness` prunes from
            // emission (the stdlib `PiggyBank` sealed-op `Error#` members
            // are the worked example). Compile-time `$txn` never reaches
            // backend emission.
        }
        TopLevel::WhenLaw(w) => {
            work.push(Seed::Expr(&w.guard));
            work.push(Seed::Stmts(w.facts.as_slice()));
        }
        TopLevel::Cell(c) => seed_cell(c, work),
        TopLevel::Obj(o) => seed_obj(o, work),
        TopLevel::TypeDef(td) => seed_typedef(td, work),
        TopLevel::Trait(_)
        | TopLevel::Impl(_)
        | TopLevel::ProtocolDef(_)
        | TopLevel::Codec(_) => {
            // Same liveness boundary as TypeDef: trait/impl functions and
            // op bindings emit only when reached through dispatch; the
            // Harvester's call/method-call expansion covers the reached
            // ones. See the Phase 0.3 audit for the dispatch-only gap.
        }
        TopLevel::Init(i) => {
            if let Some(v) = &i.value {
                work.push(Seed::Expr(v));
            }
            work.push(Seed::Stmts(i.body.as_slice()));
        }
        TopLevel::Assertion { pre, .. } => work.push(Seed::Expr(pre)),
        TopLevel::Fuzzed { item, cases, .. } => {
            seed_fuzz_cases(cases, work);
            seed_toplevel(item, work);
        }
        TopLevel::SyncGroup { item, .. } => seed_toplevel(item, work),
        TopLevel::Cfg(c) => {
            for inner in &c.items {
                seed_toplevel(inner, work);
            }
        }

        // ── Compile-time only: never reaches backend emission ───────
        // `$(stage)` bodies execute in the staged front end; `$let`,
        // `$const` are macro-level bindings expanded BEFORE the backend
        // (their expansions surface elsewhere in the AST and are seeded
        // through those forms). `cfg` conditions are string comparisons
        // resolved at parse time; asm-fn bodies are raw instruction TEXT
        // (no Expr nodes).
        TopLevel::StageBlock(_) | TopLevel::CompileTimeLet(_, _)
        | TopLevel::CompileTimeConst(_, _) | TopLevel::AsmFn(_) => {}

        // ── Declarations with no executable expression ──────────────
        TopLevel::Unpop(_)
        | TopLevel::ShortCircuit(_)
        | TopLevel::Import(_)
        | TopLevel::Signature(_)
        | TopLevel::StateDecl(_)
        | TopLevel::ResourceDecl(_)
        | TopLevel::LinkDependency(_)
        | TopLevel::StaticStruct(_)
        | TopLevel::ForeignBinding(_)
        | TopLevel::Enum(_)
        | TopLevel::BadFn(_)
        | TopLevel::RenderBlock(_)
        | TopLevel::FabBlock(_)
        | TopLevel::Stylesheet(_)
        | TopLevel::SvgComponent { .. }
        | TopLevel::Data(_)
        | TopLevel::ModuleConfig(_)
        | TopLevel::ModuleMetadata(_) => {}
    }
}

/// Walk a TopLevel item, calling `f` on every Expr encountered — contract
/// halves included. Delegates to `ast::visit` so the two walkers cannot
/// drift apart (Rule 17).
fn walk_toplevel<F>(item: &TopLevel, f: &mut F)
where
    F: FnMut(&Expr),
{
    match item {
        TopLevel::Definition(d) | TopLevel::TypeDefOperator(d)
        | TopLevel::CompileTimeDefn(d) => visit::walk_definition(d, f),
        TopLevel::Transaction(t) | TopLevel::CompileTimeTxn(t) => {
            visit::walk_contract(&t.contract, f);
            visit::walk_stmts(&t.body, f);
        }
        TopLevel::Constant(c) => visit::walk_expr(&c.expr, f),
        TopLevel::IsrHandler(h) => visit::walk_stmts(&h.body, f),
        TopLevel::Statement(s) => visit::walk_stmts(std::slice::from_ref(s.as_ref()), f),
        TopLevel::Trigger(t) => visit::walk_expr(&t.instance, f),
        TopLevel::TriggerBinding { instance, .. } => visit::walk_expr(instance, f),
        TopLevel::Budget(b) => visit::walk_expr(&b.contract, f),
        TopLevel::Export(e) => walk_toplevel(&e.inner, f),
        _ => {
            // The remaining variants are declarations or compile-time
            // forms; seeding rules live in `seed_toplevel` (the gate) and
            // are documented there. `collect_intrinsic_calls` uses this
            // walker for inventory scans, not for gating.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    fn gate_errors(src: &str, supported: &[&str]) -> Vec<String> {
        let items = parse(src);
        let set: HashSet<String> = supported.iter().map(|s| s.to_string()).collect();
        validate_intrinsics(&items, &set)
    }

    /// 2026-10-05 (three-surfaces plan Phase 0.3, intrinsic-coverage audit):
    /// an intrinsic inside a `let` INITIALIZER must be gated. The pre-audit
    /// harvester had no `Statement::Let` arm, so this class bypassed the
    /// gate and reached emitters that index `args[0]` unguarded (BUGS.md
    /// intrinsic-arity panic class). With an empty supported set the gate
    /// must name the intrinsic — not stay silent.
    #[test]
    fn gate_catches_intrinsic_inside_let_initializer() {
        let src = r#"
node report [true][true] {
    let x: Int = Popcount#(5);
    term;
};
"#;
        let errs = gate_errors(src, &[]);
        assert!(
            errs.iter().any(|e| e.contains("Popcount#")),
            "let-initializer intrinsic must be gated, got: {errs:?}"
        );
    }

    /// 2026-10-05: a NESTED intrinsic (inside another call's arguments)
    /// must be gated even when the outer call is supported — the old walker
    /// only saw statement-root expressions.
    #[test]
    fn gate_catches_intrinsic_nested_in_supported_call() {
        let src = r#"
node report [true][true] {
    let x: Int = Abs#(Popcount#(5));
    term;
};
"#;
        let errs = gate_errors(src, &["Abs#"]);
        assert!(
            errs.iter().any(|e| e.contains("Popcount#")),
            "nested intrinsic must be gated, got: {errs:?}"
        );
        assert!(
            !errs.iter().any(|e| e.contains("'Abs#'")),
            "the supported outer intrinsic must not be flagged, got: {errs:?}"
        );
    }

    /// 2026-10-05: contract halves are part of the gate surface.
    #[test]
    fn gate_catches_intrinsic_in_contract() {
        let src = r#"
node report [Popcount#(5) > 0][true] {
    term;
};
"#;
        let errs = gate_errors(src, &[]);
        assert!(
            errs.iter().any(|e| e.contains("Popcount#")),
            "contract-half intrinsic must be gated, got: {errs:?}"
        );
    }

    /// 2026-10-05: method-dispatch expansion — an intrinsic inside an op
    /// member body is gated when the member is reached through a method
    /// call (the dispatch name is the defn name).
    #[test]
    fn gate_reaches_op_member_body_through_method_call() {
        let src = r#"
obj Box {
    b: Int;
    op At(i: Int) -> Int {
        let v: Int = Popcount#(i);
        term v;
    };
};
let bx: Box = Box { b: 0 };
txn t [true][true] {
    let v: Int = bx.At(1);
    term;
};
"#;
        let errs = gate_errors(src, &[]);
        assert!(
            errs.iter().any(|e| e.contains("Popcount#")),
            "op-member body reached via method call must be gated, got: {errs:?}"
        );
    }

    /// 2026-10-05: the over-seed boundary — a NEVER-CALLED root definition
    /// stays out of the gate (dead code is not the program), but a called
    /// one is gated through call expansion.
    #[test]
    fn gate_skips_uncalled_root_defn_but_gates_called_one() {
        let src = r#"
defn dead_helper(x: Int) -> Int { term Popcount#(x); };
defn live_helper(x: Int) -> Int { term Abs#(x); };
node report [true][true] {
    let x: Int = live_helper(5);
    term;
};
"#;
        let errs = gate_errors(src, &["Abs#", "Popcount#"]);
        assert!(
            errs.is_empty(),
            "all intrinsics are supported, got: {errs:?}"
        );
        // Flip the supported set: only the UNCALLED defn's intrinsic
        // stays unsupported — it must not be flagged (liveness boundary).
        let errs = gate_errors(src, &["Abs#"]);
        assert!(
            errs.is_empty(),
            "dead defn bodies are outside the gate, got: {errs:?}"
        );
        // The called defn's intrinsic IS gated.
        let errs = gate_errors(src, &["Popcount#"]);
        assert!(
            errs.iter().any(|e| e.contains("Abs#")),
            "called defn body must be gated, got: {errs:?}"
        );
        assert!(
            !errs.iter().any(|e| e.contains("Popcount#")),
            "dead defn body must stay ungated, got: {errs:?}"
        );
    }
}
