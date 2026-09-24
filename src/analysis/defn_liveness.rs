//! Defn liveness — emission-gating reachability (2026-09-13).
//!
//! The compiler emits only definitions transitively reachable from live
//! code. Imports grant capability; liveness gates emission cost. Dead-code
//! elimination is a compiler decision, never delegated to LTO.
//! Contract: `docs/architecture/defn-liveness.md`; plan:
//! `docs/plans/2026-09-13-defn-liveness-emission.md`.
//!
//! Roots (always emitted): reactive txns/nodes (the reactor fires them by
//! name, top-level AND nested in obj/cell bodies), exports, ISR handlers,
//! asm functions, op members and obj/cell members (dispatch names are
//! mangled at emission — conservative keep), spawn targets, cast-lane
//! functions (seeded base lanes + proto/typedef binding functions), and
//! every callee of top-level statements (the backend emits those directly
//! into the init path).
//!
//! Closure: explicit `Expr::Call` edges plus the intrinsic→helper table
//! (`intrinsic_helpers`) — intrinsic lowerings may emit calls to pure-Briev
//! helper defns that appear nowhere in the AST call graph.
//!
//! SOUNDNESS NET: the LLVM backend scans the final IR for calls to program
//! defns that this pass did not mark live and fails the compile. An
//! incomplete table, a walker gap, or a future Expr variant carrying calls
//! therefore surfaces as an immediate, actionable compiler error — never a
//! silent linker failure. Over-approximation (keeping a few tiny helpers
//! like `briev_str_eq` on any `==`) is deliberate and harmless: the
//! validation targets (bare-metal hello) contain none of the coarse
//! triggers, so their binaries stay minimal.

use crate::ast::{Contract, Expr, PropertyValue, Statement, TopLevel};
use std::collections::{HashMap, HashSet};

/// The liveness verdict for one compilation unit. Consumed by the backend's
/// emission gate via `AnalysisResults.defn_liveness`.
#[derive(Debug, Clone, Default)]
pub struct DefnLiveness {
    /// Names (defns AND txns share one namespace) that must be emitted.
    pub live: HashSet<String>,
    /// Coarse reflection root: live code contains `.^^` (compile-time
    /// reflection), which can reach any member by name. Keep all defns.
    /// REFINEMENT (deferred): reflect only on the referenced names.
    pub keep_all: bool,
}

impl DefnLiveness {
    /// Is `name` emitted? `keep_all` (reflection) pins everything.
    pub fn is_live(&self, name: &str) -> bool {
        self.keep_all || self.live.contains(name)
    }

    /// Compute the live set for a full program item list (post-import,
    /// post-typecheck — the same list `analyze_program` receives).
    pub fn build(items: &[TopLevel]) -> DefnLiveness {
        let mut pass = Builder {
            defns: HashMap::new(),
            txns: HashMap::new(),
            contracts: HashMap::new(),
            type_members: HashMap::new(),
            roots: HashSet::new(),
            live: HashSet::new(),
            keep_all: false,
        };

        // Index + static roots in one walk over the top level.
        for item in items {
            pass.index_item(item);
        }

        // Worklist closure. Roots seed the queue; every live callable's
        // body AND contract contribute explicit calls + intrinsic-implied
        // helpers (2026-09-24: contracts are emitted code — pre/post,
        // watchdog condition/fallback, on-fire handler name).
        let mut queue: Vec<String> = pass.roots.iter().cloned().collect();
        debug_dump("roots", &queue);
        while let Some(name) = queue.pop() {
            if !pass.live.insert(name.clone()) {
                continue;
            }
            if let Some(body) = pass.defns.get(&name) {
                pass.walk_stmts(body, &mut queue);
            }
            if let Some(body) = pass.txns.get(&name) {
                pass.walk_stmts(body, &mut queue);
            }
            if let Some(c) = pass.contracts.get(&name) {
                pass.walk_contract(c, &mut queue);
            }
        }

        DefnLiveness { live: pass.live, keep_all: pass.keep_all }
    }
}

/// Temporary diagnostic (2026-09-13): BRIEV_DEBUG_LIVENESS=1 dumps roots.
/// To undo: delete this fn and its call in `build`.
fn debug_dump(stage: &str, names: &[String]) {
    if std::env::var("BRIEV_DEBUG_LIVENESS").is_ok() {
        let mut sorted = names.to_vec();
        sorted.sort();
        eprintln!("liveness {stage} ({}): {:?}", sorted.len(), sorted);
    }
}

struct Builder<'a> {
    /// defn name → body (top-level Definitions only; these are the
    /// emission candidates the backend gate skips).
    defns: HashMap<String, &'a [Statement]>,
    /// txn name → body (top-level Transactions, reactive and callable —
    /// the closure walks both; reactive ones are roots regardless).
    txns: HashMap<String, &'a [Statement]>,
    /// 2026-09-24 (contract-liveness gap, found by the series_converge
    /// soundness-net panic): contract EXPRESSIONS of a live callable are
    /// emitted code too — pre/post conditions, watchdog condition/fallback,
    /// and the `-> handler(v)` on-fire callback (a bare handler NAME, not
    /// an Expr — invisible to walk_expr). Index every contract beside its
    /// body and walk it when the callable enters the queue; without this,
    /// `?[...] -> print_best(x)` emits a call to a judged-dead defn.
    contracts: HashMap<String, &'a Contract>,
    /// 2026-09-13 (usage-triggered member rooting): type/obj base name →
    /// member defn/txn/op names. Members are rooted when live code
    /// CONSTRUCTS the type (`HashMap { … }`, `spawn Enemy(…)`, ctor-style
    /// call) — NOT blanket-rooted: prelude collection ops carry real
    /// implementation bodies (coll growth → malloc) that a program using
    /// no collections must not pay for.
    type_members: HashMap<String, Vec<String>>,
    roots: HashSet<String>,
    live: HashSet<String>,
    keep_all: bool,
}

impl<'a> Builder<'a> {
    /// Index a defn: body for the call closure, contract for the emitted
    /// pre/post/watchdog expressions (2026-09-24, contract-liveness gap).
    fn index_defn(&mut self, d: &'a crate::ast::Definition) {
        self.defns.entry(d.name.clone()).or_insert(&d.body);
        self.contracts.entry(d.name.clone()).or_insert(&d.contract);
    }

    /// Index a txn: body + contract (same gap; the on-fire handler NAME
    /// lives only in the contract — walk_expr can never see it).
    fn index_txn(&mut self, t: &'a crate::ast::Transaction) {
        self.txns.entry(t.name.clone()).or_insert(&t.body);
        self.contracts.entry(t.name.clone()).or_insert(&t.contract);
    }

    /// obj-like type bodies carry member txns/defns (`parse_obj_like`).
    /// Members are USAGE-ROOTED: registered under the type name and
    /// enqueued when live code constructs the type — prelude collection
    /// implementations (coll growth → malloc) must not leak into programs
    /// that construct no collections. Operator-binding impl fns join the
    /// same bucket.
    fn index_typedef(&mut self, td: &'a crate::ast::TypeDef) {
        let mut members: Vec<String> = Vec::new();
        for m in &td.body.members {
            match m {
                TopLevel::Definition(d) => {
                    self.index_defn(d);
                    members.push(d.name.clone());
                }
                TopLevel::Transaction(t) => {
                    self.index_txn(t);
                    members.push(t.name.clone());
                }
                TopLevel::TypeDefOperator(op) => {
                    members.push(op.name.clone());
                }
                _ => {}
            }
        }
        for op in &td.body.operators {
            if let Some(pv) = &op.impl_args {
                collect_binding_fns(pv, &mut members);
            }
        }
        self.type_members.insert(td.name.clone(), members);
    }

    /// 2026-09-22 (universal-bootstrapper plan): a `bootstrap bad` body
    /// references .bv defns/txns by symbol (`call kernel_bv`,
    /// `addr r0, msg`). Parse the raw body and root every name that is a
    /// known .bv defn/txn, so the handoff target is emitted. A parse
    /// failure is NOT an error here — the bad backend reports it loudly
    /// at compile; liveness just can't see the references.
    fn root_bad_body(&mut self, body: &str) {
        let Ok(program) = crate::parser::bad::parse_bad(body) else {
            return;
        };
        // Flat iterator chain (items → instrs) — no nested `for`.
        for i in program.items.into_iter().flat_map(bad_instructions_of) {
            self.root_sym_operands(&i);
        }
    }

    /// Root any .bv defn/txn a symbol operand of `i` names. Symbol ops are
    /// `call`/`addr`/`jmp` and the branch family.
    fn root_sym_operands(&mut self, i: &crate::ast::bad::BadInstr) {
        use crate::ast::bad::BadOperand;
        if !matches!(
            i.mnemonic.as_str(),
            "call" | "addr" | "jmp" | "jz" | "jnz" | "jlt" | "jle" | "jgt"
                | "jge" | "jlo" | "jls" | "jhi" | "jhs"
        ) {
            return;
        }
        for op in &i.operands {
            let BadOperand::Name(n) = op else { continue };
            if self.defns.contains_key(n) || self.txns.contains_key(n) {
                self.roots.insert(n.clone());
            }
        }
    }

    fn index_item(&mut self, item: &'a TopLevel) {
        match item {
            TopLevel::Definition(d) => {
                // Top-level: plain insert is the established last-wins
                // semantics — preserve it; the contract rides alongside.
                self.defns.insert(d.name.clone(), &d.body);
                self.contracts.entry(d.name.clone()).or_insert(&d.contract);
            }
            TopLevel::Transaction(t) => {
                self.txns.insert(t.name.clone(), &t.body);
                self.contracts.entry(t.name.clone()).or_insert(&t.contract);
                // Reactive txns are reactor-dispatched by name — roots.
                // Callable txns enter via closure from live callers.
                if t.is_reactive {
                    self.roots.insert(t.name.clone());
                }
                // 2026-09-17 (same class as the stdout-flush gate fix): an
                // async txn makes the program reactive-unbounded — the main
                // tail yields through __wait_for_trigger__ (loop_engine's
                // no-wake/no-exit branch). The call is backend-emitted, so
                // liveness must root its helper alongside the txn.
                if t.is_async {
                    self.roots.insert("__wait_for_trigger__".into());
                }
            }
            TopLevel::Export(e) => {
                // ABI surface: the exported defn/txn is always emitted.
                // 2026-09-23 (soundness-net catch, pp_roundtrip/cancel): the
                // export BODY must join the worklist too — its callees are
                // emitted code and must be live. Rooting only the name left
                // the body unwalked: exports calling imported defns (e.g.
                // pp_type_bits) or txns (e.g. sum_loop) tripped the net with
                // "unreached defn". Index the inner defn/txn like any other.
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    self.index_defn(d);
                    self.roots.insert(d.name.clone());
                }
                if let TopLevel::Transaction(t) = e.inner.as_ref() {
                    self.index_txn(t);
                    self.roots.insert(t.name.clone());
                    if t.is_reactive {
                        self.roots.insert(t.name.clone());
                    }
                    if t.is_async {
                        self.roots.insert("__wait_for_trigger__".into());
                    }
                }
            }
            TopLevel::IsrHandler(isr) => {
                // Vector tables reference handler symbols. The typed body is
                // emitted (and called) by the mechanism's scaffold — root the
                // wrapper AND walk the body so its callees join the closure.
                self.roots.insert(isr.name.clone());
                self.txns.entry(isr.name.clone()).or_insert(&isr.body);
                self.contracts.entry(isr.name.clone()).or_insert(&isr.contract);
            }
            TopLevel::AsmFn(asm) => {
                // Top-level observable asm.
                self.roots.insert(asm.name.clone());
            }
            // 2026-09-21: bad fn — always rooted (body compiled via bad backend).
            TopLevel::BadFn(bf) => {
                self.roots.insert(bf.name.clone());
                // 2026-09-22 (universal-bootstrapper plan): a bootstrap
                // body can CALL a real .bv defn/txn — the loader handoff
                // (`call kernel_bv`). Liveness cannot see asm-level symbol
                // references, so parse the body and root every name it
                // references that is a known .bv defn/txn. Without this,
                // the defn is judged dead, never emitted, and the link
                // fails with an unresolved symbol.
                self.root_bad_body(&bf.body);
            }
            TopLevel::TypeDefOperator(op) => {
                // A BARE top-level `op Count() { … }` has no type context in
                // the AST — conservative root. Type-BODY operators (the
                // prelude collection ops) are NOT rooted here: they enter
                // through `type_members` when live code constructs the type.
                self.roots.insert(op.name.clone());
            }
            TopLevel::Obj(s) => {
                // Obj member txns (`node apply_damage()…` inside
                // `obj Enemy { … }`) — usage-rooted via construction.
                let members = s.transactions.iter().map(|t| {
                    self.index_txn(t);
                    t.name.clone()
                }).collect();
                self.type_members.insert(s.name.clone(), members);
            }
            TopLevel::Cfg(cfg) => {
                // Cfg-guarded items may or may not be active; conservative:
                // index them all (roots from them apply).
                for i in &cfg.items {
                    self.index_item(i);
                }
            }
            TopLevel::Fuzzed { item, .. } => self.index_item(item),
            TopLevel::ProtocolDef(pd) => {
                // Proto cast/op bindings name pure-Briev transform fns
                // (`CastTo(...) = ascii_to_utf8(#L);`) — the casting graph
                // calls them at emission (LaneKind::ExtCallDyn).
                for edge in &pd.cast_edges {
                    if let Some(b) = &edge.binding {
                        self.roots.insert(b.fn_name.clone());
                    }
                }
                for op in &pd.cross_ops {
                    if let Some(pv) = &op.impl_args {
                        collect_binding_fns(pv, &mut self.roots);
                    }
                }
            }
            TopLevel::TypeDef(td) => self.index_typedef(td),
            TopLevel::Statement(stmt) => {
                // Top-level statements are emitted directly into the init
                // path — their callees are live from birth.
                let mut found = Vec::new();
                collect_call_names_stmt(stmt, &mut found);
                for f in found {
                    self.roots.insert(f);
                }
            }
            TopLevel::SyncGroup { item, .. } => self.index_item(item),
            // Leaves: type declarations, imports, state, metadata, … carry
            // no emission candidates. (CompileTimeDefn/Txn/Let/Const are
            // extracted before codegen; StageBlock already expanded.)
            _ => {}
        }
    }

    fn mark(&self, name: &str, queue: &mut Vec<String>) {
        // Mark any callable with this name (defn/txn share one namespace).
        if self.defns.contains_key(name) || self.txns.contains_key(name) {
            queue.push(name.to_string());
        }
    }

    /// Live code CONSTRUCTED a type/obj base — its behavioral members join
    /// the closure (see `type_members`).
    fn on_construction(&self, base: &str, queue: &mut Vec<String>) {
        if let Some(members) = self.type_members.get(base) {
            for m in members {
                queue.push(m.clone());
            }
        }
    }

    /// 2026-09-23 (async Phase D soundness-net catch): a `spawn`/`await` in
    /// live code lowers to briev_task_spawn_impl / briev_await_impl, and the
    /// spawned tasks' port fire/read/ready lower to the event family
    /// (briev_event_*_impl, __briev_event_strict_trap). Root the whole family
    /// — type-agnostic over-approx; `mark` is a no-op for helpers absent from
    /// the unit. The event helpers' own callees (write_all/__print etc.) join
    /// the closure when their bodies are walked from the worklist.
    fn root_task_event_family(&self, queue: &mut Vec<String>) {
        for h in [
            "briev_task_spawn_impl",
            "briev_task_cancel_impl",
            "briev_await_impl",
            "briev_event_alloc_impl",
            "briev_event_ready_impl",
            "briev_event_read_impl",
            "briev_event_fire_impl",
            "__briev_event_strict_trap",
        ] {
            self.mark(h, queue);
        }
    }

    /// Walk a live callable's CONTRACT into the closure. Every expression
    /// here is emitted code the backend runs for this callable: pre/post
    /// conditions, the watchdog condition and its fallback, and — the name
    /// that started this (series_converge soundness-net panic) — the
    /// on-fire callback handler, a bare String identifier walk_expr can
    /// never reach. Dead callables' contracts are never emitted, so only
    /// live entries arrive here.
    fn walk_contract(&mut self, c: &'a Contract, queue: &mut Vec<String>) {
        self.walk_expr(&c.pre_condition, queue);
        self.walk_expr(&c.post_condition, queue);
        let Some(w) = &c.watchdog else { return };
        self.walk_expr(&w.condition, queue);
        if let Some(fb) = &w.fallback {
            self.walk_expr(fb, queue);
        }
        if let Some(of) = &w.on_fire {
            self.mark(&of.handler, queue);
        }
    }

    fn walk_stmts(&mut self, stmts: &'a [Statement], queue: &mut Vec<String>) {
        for s in stmts {
            self.walk_stmt(s, queue);
        }
    }

    fn walk_stmt(&mut self, stmt: &'a Statement, queue: &mut Vec<String>) {
        // The marking-heavy statement forms — stream writes and helpers
        // keyed off statement SHAPE (see inline notes).
        match stmt {
            Statement::ArrowAssign { target, value, .. } => {
                // `#StdOut <- v;` / `#StdErr <- v;` lower to the print
                // family / stderr printer — root their helpers.
                if let Some(t) = target {
                    if let Expr::Identifier(name) = t.as_ref() {
                        if name == "#StdOut" {
                            for h in intrinsic_helpers("Print#") {
                                self.mark(h, queue);
                            }
                        }
                        if name == "#StdErr" {
                            self.mark("__eprint_str", queue);
                        }
                    }
                    self.walk_expr(t, queue);
                }
                // 2026-09-23 (async Phase D soundness-net catch): `w <- v;`
                // on an Event<_> wire lowers to briev_event_fire_impl. The
                // pass is type-agnostic — root the task/event family whenever
                // an arrow is assigned (mark is a no-op when the helpers are
                // absent; a spawn/await program roots them anyway).
                self.root_task_event_family(queue);
                self.walk_expr(value, queue);
            }
            Statement::EndProgram(Some(e)) => {
                // `endprogram code;` → `call @__exit(code)` when defined.
                self.mark("__exit", queue);
                self.walk_expr(e, queue);
            }
            Statement::Foreach { list, body, .. } => {
                // foreach over String lowers to `briev_str_next_char` —
                // rooted whenever a foreach exists and the defn is present
                // (the pass is type-agnostic; coarse-sound over-approx).
                self.mark("briev_str_next_char", queue);
                self.walk_expr(list, queue);
                self.walk_stmts(body, queue);
            }
            Statement::FreeHint(_) => {
                // Scheduler auto-free lowers to `__briev_free` for heap
                // fields; the explicit hint is the AST-visible form.
                // 2026-09-22 (soundness-net catch): a freed TASK HANDLE
                // additionally lowers to `briev_task_cancel_impl` — root
                // both so async programs pass the IR scan.
                self.mark("__briev_free", queue);
                self.mark("briev_task_cancel_impl", queue);
            }
            other => self.walk_stmt_rest(other, queue),
        }
    }

    /// Statements whose liveness contribution is pure subexpression
    /// traversal — no shape-keyed marking.
    fn walk_stmt_rest(&mut self, stmt: &'a Statement, queue: &mut Vec<String>) {
        match stmt {
            Statement::Let { expr: Some(e), .. } => self.walk_expr(e, queue),
            Statement::Assign(lhs, rhs) => {
                self.walk_expr(lhs, queue);
                self.walk_expr(rhs, queue);
            }
            Statement::Term(Some(e)) => self.walk_expr(e, queue),
            Statement::Check(e) => self.walk_expr(e, queue),
            Statement::Guarded(cond, body) => {
                self.walk_expr(cond, queue);
                self.walk_stmts(body, queue);
            }
            Statement::Gate(e) | Statement::Expression(e) => self.walk_expr(e, queue),
            Statement::Block(body) => self.walk_stmts(body, queue),
            Statement::Defer(body) | Statement::Mutex(body) | Statement::SyncBlock(body) => {
                self.walk_stmts(body, queue);
            }
            Statement::Match { expr, arms } => {
                self.walk_expr(expr, queue);
                self.walk_match_arms(arms, queue);
            }
            Statement::Rollback(Some(e)) => self.walk_expr(e, queue),
            Statement::MetadataAssignment(_, pv) => {
                let mut found = HashSet::new();
                collect_binding_fns(pv, &mut found);
                for f in found {
                    self.mark(&f, queue);
                }
            }
            Statement::TrgBinding { instance, .. } => self.walk_expr(instance, queue),
            // Leaves: no calls, no subexpressions (Break/Trap/Halt/Term
            // (none)/Yield/EndProgram (none)/InlineAsm); compile-time inline
            // items are extracted before codegen.
            _ => {}
        }
    }

    fn walk_match_arms(&mut self, arms: &'a [crate::ast::StmtMatchArm], queue: &mut Vec<String>) {
        arms.iter().flat_map(|arm| arm.body.iter()).for_each(|s| self.walk_stmt(s, queue));
    }

    fn walk_expr(&mut self, expr: &'a Expr, queue: &mut Vec<String>) {
        match expr {
            Expr::Call(name, args, _) => {
                // Explicit call edge — or an intrinsic whose lowering may
                // emit helper defns (the table is the one source of that
                // knowledge; Rule 17/18). A Call naming a TYPE is the
                // constructor-call form (`Enemy(3, 5)`) — its members join.
                self.mark(name, queue);
                self.on_construction(name, queue);
                for h in intrinsic_helpers(name) {
                    self.mark(h, queue);
                }
                // 2026-09-14 (machine-entry plan): `Asm#("raw", template)`
                    // templates may reference program symbols (`la $0,
                    // task_a` — machine wiring names handlers). Extract the
                    // template's identifier words and mark any that are
                    // known callables — conservative over-approximation
                    // (substring hits only ever ADD liveness).
                if name == "Asm#" {
                    if let Some(crate::ast::Expr::Quoted(t)) = args.get(1) {
                        let text = String::from_utf8_lossy(t).to_string();
                        let mut word = String::new();
                        let mut words: Vec<String> = Vec::new();
                        for ch in text.chars() {
                            if ch.is_alphanumeric() || ch == '_' {
                                word.push(ch);
                            } else if !word.is_empty() {
                                words.push(std::mem::take(&mut word));
                            }
                        }
                        if !word.is_empty() {
                            words.push(word);
                        }
                        for w in words {
                            self.mark(&w, queue);
                        }
                    }
                }
                for a in args {
                    self.walk_expr(a, queue);
                }
            }
            Expr::MethodCall(recv, name, args, _, _) => {
                // Op-member dispatch — members are conservatively rooted;
                // walk receiver/args for their own edges.
                // 2026-09-16 (Bug F): a UFCS method call (`a.f(x)` → `f(a, x)`)
                // targets a TOP-LEVEL defn, not a member. Mark the name so a
                // defn reached only through UFCS is not eliminated. `mark` is a
                // no-op for genuine member names (they are not defns/txns).
                self.mark(name, queue);
                self.walk_expr(recv, queue);
                for a in args {
                    self.walk_expr(a, queue);
                }
            }
            Expr::Spawn { type_name, args, .. } => {
                // `spawn defn(args)` = task spawn of the defn; `spawn Obj(…)`
                // constructs the obj base — either way its members join.
                // 2026-09-22 (soundness-net catch): a task spawn lowers to
                // `briev_task_spawn_impl` in the backend — root it here so
                // async-tasks-style programs pass the IR scan.
                self.mark(type_name, queue);
                self.mark("briev_task_spawn_impl", queue);
                self.on_construction(type_name, queue);
                // 2026-09-23 (soundness-net catch, async Phase D): `spawn`
                // lowers to briev_task_spawn_impl, and the spawned tasks'
                // await/fire/read lower to the event family. Root the whole
                // task/event helper family — type-agnostic over-approx (mark
                // is a no-op for helpers absent from the unit).
                self.root_task_event_family(queue);
                for a in args {
                    self.walk_expr(a, queue);
                }
            }
            Expr::Reflect(_, _, kind) => {
                // `.^^` compile-time reflection can reach members by name —
                // coarse keep-all (sound; refinement deferred).
                if matches!(kind, crate::ast::ReflectKind::CompileTime) {
                    self.keep_all = true;
                }
            }
            Expr::BinaryOp(_, l, r) => {
                // `==` on `#String` values lowers to `briev_str_eq` when
                // defined; the pass is type-agnostic — any Eq may be one.
                // Coarse-sound: root on every binary op (only fires when
                // the defn exists; hello-style programs have no `==`).
                self.mark("briev_str_eq", queue);
                // 2026-09-23 (soundness-net catch, pp_roundtrip): `+` on a
                // String lowers to inline concat, which frees its temporaries
                // via `__briev_free` at the end of the enclosing statement.
                // Type-agnostic — any Add may be a String concat. Same
                // coarse-sound over-approx as briev_str_eq above.
                self.mark("__briev_free", queue);
                self.walk_expr(l, queue);
                self.walk_expr(r, queue);
            }
            Expr::Index(obj, idx) => {
                // BracketOp::Mask on vector state fields lowers to the
                // briev_mask_select* gathers; type-agnostic over-approx —
                // root the family when an index exists and they are defined.
                for h in [
                    "briev_mask_select",
                    "briev_mask_select64",
                    "briev_mask_select_i8mask",
                    "briev_mask_select64_i8mask",
                    "briev_mask_select_f32",
                    "briev_mask_select_f32_i8mask",
                ] {
                    self.mark(h, queue);
                }
                self.walk_expr(obj, queue);
                self.walk_expr(idx, queue);
            }
            Expr::Slice { array, start, end, stride, .. } => {
                // Vector range gather lowers to briev_slice_range64/_f32.
                self.mark("briev_slice_range64", queue);
                self.mark("briev_slice_range_f32", queue);
                self.walk_expr(array, queue);
                if let Some(s) = start {
                    self.walk_expr(s, queue);
                }
                if let Some(e) = end {
                    self.walk_expr(e, queue);
                }
                if let Some(s) = stride {
                    self.walk_expr(s, queue);
                }
            }
            other => self.walk_expr_rest(other, queue),
        }
    }

    /// Expressions whose liveness contribution is pure subexpression
    /// traversal — no shape-keyed marking.
    fn walk_expr_rest(&mut self, expr: &'a Expr, queue: &mut Vec<String>) {
        match expr {
            Expr::Identifier(_)
            | Expr::Quoted(_)
            | Expr::Decimal(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::TaggedLiteral(_, _)
            | Expr::TaggedQuotedLiteral(_, _)
            | Expr::FormattingAnnotation(_) => {}
            Expr::UnaryOp(_, inner) => self.walk_expr(inner, queue),
            Expr::Field(obj, _) => self.walk_expr(obj, queue),
            Expr::Range { start, end, .. } => {
                self.walk_expr(start, queue);
                self.walk_expr(end, queue);
            }
            Expr::Block(body) => self.walk_stmts(body, queue),
            Expr::If(cond, then_e, else_e) => {
                self.walk_expr(cond, queue);
                self.walk_expr(then_e, queue);
                if let Some(e) = else_e {
                    self.walk_expr(e, queue);
                }
            }
            Expr::Match(scrutinee, arms) => {
                self.walk_expr(scrutinee, queue);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.walk_expr(g, queue);
                    }
                    self.walk_expr(&arm.body, queue);
                }
            }
            Expr::Tuple(elems) | Expr::List(elems) => {
                for e in elems {
                    self.walk_expr(e, queue);
                }
            }
            Expr::StructLiteral { type_name, fields, specs } => {
                // `HashMap { … }` / `Point { … }` — construction roots the
                // type's behavioral members.
                self.on_construction(type_name, queue);
                for (_, e) in fields.iter().chain(specs.iter()) {
                    self.walk_expr(e, queue);
                }
            }
            Expr::Lambda(_, body) => self.walk_expr(body, queue),
            Expr::Cast(inner, _) | Expr::IsType(inner, _) => self.walk_expr(inner, queue),
            Expr::Within(a, b) => {
                self.walk_expr(a, queue);
                self.walk_expr(b, queue);
            }
            Expr::DerivationBlock(_) => {}
            Expr::Deref(inner)
            | Expr::AddrOf(inner)
            | Expr::Consume(inner)
            | Expr::Await(inner) => self.walk_expr(inner, queue),
            Expr::PluginIntercept { args, .. } => {
                for a in args {
                    self.walk_expr(a, queue);
                }
            }
            Expr::Exists(_) => {}
            Expr::UnitLiteral { .. } => {}
            // `beginprogram` — the entry marker conjunct (SPEC §11.5.1).
            Expr::BeginProgram => {}
            // Unreachable: Call/MethodCall/Spawn/Reflect/BinaryOp/Index/
            // Slice/StructLiteral are handled (with their shape-keyed
            // marking) in walk_expr before delegation. If a NEW expression
            // kind carrying calls is added to Expr, `walk_expr`'s own
            // exhaustiveness check forces it into one of the two halves —
            // and the backend's IR-scan net backstops a wrong choice.
            _ => {}
        }
    }
}

/// Intrinsic → pure-Briev helper defns its lowering may call. THE one table
/// (Rule 17/18): this knowledge exists nowhere else — the emission sites'
/// `defn_params.contains_key` checks ask the calling-convention question,
/// this table answers the liveness question. Adding a row is a data change.
/// An incomplete row is caught by the backend's IR-scan net (loud compile
/// error naming the missing helper).
///
/// Rows only matter when the helper defn is present in the unit (the gate
/// `mark` is a no-op otherwise) — e.g. with `--no-std` none of these exist
/// and the intrinsics keep their C symbols.
fn intrinsic_helpers(intrinsic: &str) -> &'static [&'static str] {
    match intrinsic {
        // Print# lowers per argument type (Int/Float/Float64/Bool/Char/
        // String) — root the whole family when any print appears. The
        // main-tail `__stdout_flush` and `__print` (generic printer) are
        // backend-emitted, not AST calls.
        "Print#" => &[
            "__print",
            "__print_str",
            "__print_int",
            "__print_bool",
            "__print_float",
            "__print_float64",
            "__print_char",
            "__stdout_byte",
            "__stdout_flush",
            // 2026-09-23 (soundness-net catch, term fixtures/async): the
            // print/string lowering may emit these even in a trivial
            // `Print#(r)` program — the cast lanes route String↔Int through
            // str_to_int/int_to_str, the scheduler auto-free lowers to
            // __briev_free, and Slice# on a String routes through
            // briev_str_substr. Root them with the family so a minimal print
            // program does not trip the net.
            "str_to_int",
            "int_to_str",
            "__briev_free",
            "briev_str_substr",
        ],
        // Slice# on `#String` values routes through the substring helper.
        "Slice#" => &["briev_str_substr"],
        // Count# on a String operand = char count (briev_char_len scan).
        "Count#" | "CharCount#" => &["briev_char_len", "briev_str_next_char"],
        // Collection growth family realloc-or-copy via the runtime helper.
        "Resize#" | "EnsureCap#" | "TrimCap#" | "InsertAt#" | "Insert#" => {
            &["__briev_coll_resize"]
        }
        // Working directory / environment.
        "GetCwd#" => &["cstr_len", "__briev_getcwd"],
        "ChDir#" => &["__briev_chdir"],
        "GetEnv#" | "GetEnvInt#" => &["__briev_getcwd"],
        // 2026-09-23 (frgn-elimination round 2): get_env!/get_env_int!
        // expand to stdlib defns (env.bv) whose bodies call the pure-Briev
        // environ walkers briev_getenv_{briev,int}_impl (cast_lanes.bv),
        // threading the compiler-owned @__briev_environ via `Environ#()`.
        // The deleted C __getenv_int must NOT come back. Root the impls so
        // a runtime get_env!() keeps them emitted.
        "Environ#" => &["briev_getenv_briev_impl", "briev_getenv_int_impl"],
        // Lifetime + time (the backend's free/now emissions prefer the
        // pure-Briev defns when present).
        "Free#" => &["__briev_free"],
        "Now#" => &["__briev_now"],
        _ => &[],
    }
}

/// Collect op-binding implementation function names from a PropertyValue
/// (`op Add: func(#Lh,#Rh)` — the fn name is an Identifier among the
/// binding's property values). `mark`-side existence checks make a stray
/// identifier harmless.
fn collect_binding_fns(pv: &PropertyValue, out: &mut impl Extend<String>) {
    match pv {
        PropertyValue::Identifier(name) => {
            out.extend(std::iter::once(name.clone()));
        }
        PropertyValue::List(items) => {
            for i in items {
                collect_binding_fns(i, out);
            }
        }
        _ => {}
    }
}

/// Collect every `Expr::Call` callee name in a statement tree (top-level
/// statement seeding — names only, no body walking: their bodies join the
/// closure through the worklist when the callee is a defn/txn).
fn collect_call_names_stmt(stmt: &Statement, out: &mut Vec<String>) {
    let mut exprs = Vec::new();
    let mut substmts: Vec<&Statement> = Vec::new();
    match stmt {
        Statement::Let { expr: Some(e), .. } => exprs.push(e),
        Statement::Assign(l, r) => {
            exprs.push(l);
            exprs.push(r);
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                exprs.push(t);
            }
            exprs.push(value);
        }
        Statement::Term(Some(e)) | Statement::Check(e) | Statement::Expression(e) => {
            exprs.push(e)
        }
        Statement::EndProgram(e) => {
            if let Some(e) = e {
                exprs.push(e);
            }
        }
        Statement::Guarded(c, body) => {
            let cond: &Expr = c;
            let mut found2 = Vec::new();
            collect_call_names_expr(cond, &mut found2);
            out.extend(found2);
            for s in body {
                collect_call_names_stmt(s, out);
            }
        }
        Statement::Foreach { list, body, .. } => {
            collect_call_names_expr(list, out);
            for s in body {
                collect_call_names_stmt(s, out);
            }
        }
        Statement::Gate(e) | Statement::Rollback(Some(e)) => exprs.push(e),
        Statement::Block(body)
        | Statement::Defer(body)
        | Statement::Mutex(body)
        | Statement::SyncBlock(body) => substmts.extend(body.iter()),
        Statement::Match { expr, arms } => {
            exprs.push(expr);
            arms.iter().flat_map(|arm| arm.body.iter()).for_each(|s| substmts.push(s));
        }
        Statement::TrgBinding { instance, .. } => exprs.push(instance),
        _ => {}
    }
    for e in exprs {
        collect_call_names_expr(e, out);
    }
    for s in substmts {
        collect_call_names_stmt(s, out);
    }
}

fn collect_call_names_expr(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Call(name, args, _) => {
            out.push(name.clone());
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        Expr::MethodCall(recv, name, args, _, _) => {
            // 2026-09-16 (Bug F): a UFCS method call targets a top-level defn
            // (`a.f(x)` → `f(a, x)`); collect the name so the defn is rooted.
            out.push(name.clone());
            collect_call_names_expr(recv, out);
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        Expr::Spawn { type_name, args, .. } => {
            out.push(type_name.clone());
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_call_names_expr(l, out);
            collect_call_names_expr(r, out);
        }
        Expr::UnaryOp(_, i) => collect_call_names_expr(i, out),
        Expr::Field(o, _) | Expr::Deref(o) | Expr::AddrOf(o) | Expr::Consume(o)
        | Expr::Await(o) | Expr::IsType(o, _) | Expr::Cast(o, _)
        => collect_call_names_expr(o, out),
        Expr::Index(o, i) => {
            collect_call_names_expr(o, out);
            collect_call_names_expr(i, out);
        }
        Expr::Slice { array, start, end, stride, .. } => {
            collect_call_names_expr(array, out);
            if let Some(s) = start {
                collect_call_names_expr(s, out);
            }
            if let Some(e) = end {
                collect_call_names_expr(e, out);
            }
            if let Some(s) = stride {
                collect_call_names_expr(s, out);
            }
        }
        Expr::Range { start, end, .. } => {
            collect_call_names_expr(start, out);
            collect_call_names_expr(end, out);
        }
        Expr::Block(b) => {
            for s in b {
                collect_call_names_stmt(s, out);
            }
        }
        Expr::If(c, t, e) => {
            collect_call_names_expr(c, out);
            collect_call_names_expr(t, out);
            if let Some(e) = e {
                collect_call_names_expr(e, out);
            }
        }
        Expr::Match(s, arms) => {
            collect_call_names_expr(s, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_call_names_expr(g, out);
                }
                collect_call_names_expr(&arm.body, out);
            }
        }
        Expr::Tuple(es) | Expr::List(es) => {
            for e in es {
                collect_call_names_expr(e, out);
            }
        }
        Expr::StructLiteral { type_name, fields, specs } => {
            out.push(type_name.clone());
            for (_, e) in fields.iter().chain(specs.iter()) {
                collect_call_names_expr(e, out);
            }
        }
        Expr::Lambda(_, body) => collect_call_names_expr(body, out),
        Expr::Within(a, b) => {
            collect_call_names_expr(a, out);
            collect_call_names_expr(b, out);
        }
        Expr::PluginIntercept { args, .. } => {
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn contract() -> Contract {
        Contract {
            pre_condition: Expr::Bool(true),
            post_condition: Expr::Bool(true),
            watchdog: None,
            span: None,
            explicit: true,
            post_authority: false,
        }
    }

    fn txn(name: &str, is_reactive: bool, body: Vec<Statement>) -> TopLevel {
        txn_with_contract(name, is_reactive, body, contract())
    }

    /// A txn with a caller-built contract (contract-liveness tests).
    fn txn_with_contract(
        name: &str,
        is_reactive: bool,
        body: Vec<Statement>,
        contract: Contract,
    ) -> TopLevel {
        TopLevel::Transaction(crate::ast::Transaction {
            name: name.to_string(),
            is_reactive,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract,
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    /// WatchdogSpec with the common zeros filled in.
    fn watchdog(
        condition: Expr,
        fallback: Option<Expr>,
        on_fire: Option<WatchdogOnFire>,
    ) -> WatchdogSpec {
        WatchdogSpec {
            condition,
            is_required: false,
            cycles_bound: None,
            seconds_bound: None,
            deadline_ns: None,
            is_proven: false,
            retries: 0,
            fallback: fallback.map(Box::new),
            on_fire,
        }
    }

    fn defn(name: &str, body: Vec<Statement>) -> TopLevel {
        TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: contract(),
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    fn call(name: &str) -> Statement {
        Statement::Expression(Expr::Call(name.to_string(), vec![], None))
    }

    #[test]
    fn dead_defn_is_not_live() {
        // A reactive txn calling nothing; an orphan defn nobody reaches.
        let items = vec![
            txn("main_node", true, vec![Statement::Term(None)]),
            defn("orphan", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("main_node"));
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn transitive_call_chain_stays_live() {
        let items = vec![
            txn("main_node", true, vec![call("a")]),
            defn("a", vec![call("b")]),
            defn("b", vec![Statement::Term(None)]),
            defn("orphan", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("a"));
        assert!(l.is_live("b"));
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn callable_txn_live_only_when_reached() {
        let items = vec![
            txn("main_node", true, vec![call("helper")]),
            txn("helper", false, vec![Statement::Term(None)]),
            txn("unused_helper", false, vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("helper"));
        assert!(!l.is_live("unused_helper"));
    }

    #[test]
    fn intrinsic_helpers_are_rooted() {
        let items = vec![
            txn("main_node", true, vec![Statement::Expression(Expr::Call(
                "Print#".to_string(),
                vec![Expr::Decimal(1)],
                None,
            ))]),
            defn("__print_int", vec![Statement::Term(None)]),
            defn("__print_str", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("__print_int"));
        assert!(l.is_live("__print_str"));
    }

    #[test]
    fn missing_helper_row_is_harmless() {
        // An intrinsic with no defns present: nothing to mark, no panic.
        let items = vec![
            txn("main_node", true, vec![Statement::Expression(Expr::Call(
                "VolatileStore#".to_string(),
                vec![],
                None,
            ))]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("main_node"));
    }

    #[test]
    fn export_and_isr_are_roots() {
        let isr = crate::ast::IsrHandler {
            mechanism: None,
            vector: crate::ast::Expr::Decimal(0x1C),
            name: "tim2_irq".to_string(),
            params: vec![],
            contract: contract(),
            body: vec![Statement::Term(None)],
            span: crate::errors::Span::new(0, 0, 0, 0),
        };
        let items = vec![
            defn("orphan", vec![Statement::Term(None)]),
            TopLevel::IsrHandler(isr),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("tim2_irq"));
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn reflection_pins_everything() {
        let items = vec![
            txn("main_node", true, vec![Statement::Expression(Expr::Reflect(
                Box::new(Expr::Identifier("x".into())),
                "Size".into(),
                crate::ast::ReflectKind::CompileTime,
            ))]),
            defn("maybe_reflected", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.keep_all);
        assert!(l.is_live("maybe_reflected"));
    }

    #[test]
    fn foreach_roots_string_iteration_helper() {
        let items = vec![
            txn("main_node", true, vec![Statement::Foreach {
                item: "c".to_string(),
                list: Box::new(Expr::Identifier("s".into())),
                body: vec![Statement::Term(None)],
            }]),
            defn("briev_str_next_char", vec![Statement::Term(None)]),
            defn("orphan", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("briev_str_next_char"));
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn spawn_roots_task_target() {
        let items = vec![
            txn("main_node", true, vec![Statement::Expression(Expr::Spawn {
                type_name: "worker".to_string(),
                args: vec![],
                storage: crate::ast::SpawnStorage::Pooled,
            })]),
            defn("worker", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("worker"));
    }

    #[test]
    fn top_level_statement_callees_are_roots() {
        let items = vec![
            TopLevel::Statement(Box::new(call("init_fn"))),
            defn("init_fn", vec![Statement::Term(None)]),
            defn("orphan", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("init_fn"));
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn watchdog_on_fire_roots_handler() {
        // The series_converge shape (2026-09-24 soundness-net panic):
        // `node converge [pre][post] ?[cond] -> print_best(x) { ... }`.
        // The handler is a bare NAME in WatchdogOnFire — walk_expr can
        // never see it — only the contract walk marks it.
        let mut c = contract();
        c.watchdog = Some(watchdog(
            Expr::Bool(true),
            None,
            Some(WatchdogOnFire { handler: "print_best".into(), arg: Some("v".into()) }),
        ));
        let items = vec![
            txn_with_contract("converge", true, vec![Statement::Term(None)], c),
            defn("print_best", vec![Statement::Term(None)]),
            defn("orphan", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("converge"));
        assert!(l.is_live("print_best"), "on-fire handler must join the closure");
        assert!(!l.is_live("orphan"));
    }

    #[test]
    fn watchdog_condition_and_fallback_calls_are_live() {
        // Contract EXPRESSIONS are emitted code: `?[progress()]` and the
        // fallback both run for a live callable — their callees must be live.
        let mut c = contract();
        c.watchdog = Some(watchdog(
            Expr::Call("progress".into(), vec![], None),
            Some(Expr::Call("recover".into(), vec![], None)),
            None,
        ));
        let items = vec![
            txn_with_contract("guarded", true, vec![Statement::Term(None)], c),
            defn("progress", vec![Statement::Term(None)]),
            defn("recover", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("progress"), "watchdog condition callee must be live");
        assert!(l.is_live("recover"), "watchdog fallback callee must be live");
    }

    #[test]
    fn pre_and_post_condition_calls_are_live() {
        let mut c = contract();
        c.pre_condition = Expr::Call("ready".into(), vec![], None);
        c.post_condition = Expr::Call("settled".into(), vec![], None);
        let items = vec![
            txn_with_contract("checked", true, vec![Statement::Term(None)], c),
            defn("ready", vec![Statement::Term(None)]),
            defn("settled", vec![Statement::Term(None)]),
        ];
        let l = DefnLiveness::build(&items);
        assert!(l.is_live("ready"), "precondition callee must be live");
        assert!(l.is_live("settled"), "postcondition callee must be live");
    }
}

/// The instructions of a .bad top-level item (labels' bodies and sequence
/// defns carry them; branch defns and directives carry none).
fn bad_instructions_of(item: crate::ast::bad::BadTopLevel) -> Vec<crate::ast::bad::BadInstr> {
    use crate::ast::bad::{BadBodyItem, BadDefnShape, BadTopLevel};
    match item {
        BadTopLevel::Label(l) => l
            .body
            .iter()
            .filter_map(|i| match i {
                BadBodyItem::Instr(x) => Some(x.clone()),
                BadBodyItem::Local(_) => None,
            })
            .collect(),
        BadTopLevel::Defn(d) => {
            if let BadDefnShape::Sequence(seq) = &d.shape {
                seq.iter()
                    .filter_map(|i| match i {
                        BadBodyItem::Instr(x) => Some(x.clone()),
                        BadBodyItem::Local(_) => None,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}
