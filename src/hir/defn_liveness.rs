// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.2 — `defn_liveness` over typed HIR.
//!
//! Mirror of `analysis::defn_liveness` (AST, still authoritative until parity):
//! emission-gating reachability with the same roots and the same shape-keyed
//! helper markings, reusing the single `intrinsic_helpers` table.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirCallable, HirCallableKind, HirContract, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::defn_liveness::intrinsic_helpers;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct DefnLiveness {
    pub live: HashSet<String>,
    pub keep_all: bool,
}

impl DefnLiveness {
    pub fn is_live(&self, name: &str) -> bool {
        self.keep_all || self.live.contains(name)
    }

    pub fn build(program: &HirProgram) -> DefnLiveness {
        Self::build_with_roots(program, &HashSet::new())
    }

    pub fn build_with_roots(program: &HirProgram, extra_roots: &HashSet<String>) -> DefnLiveness {
        let mut b = Builder::empty();
        b.index_program(program);
        b.seed_roots(program, extra_roots);
        b.drain();
        DefnLiveness { live: b.live, keep_all: b.keep_all }
    }

}

struct Builder<'a> {
    defns: HashMap<String, &'a [HirStmt]>,
    txns: HashMap<String, &'a [HirStmt]>,
    contracts: HashMap<String, &'a HirContract>,
    type_members: HashMap<String, Vec<String>>,
    member_bodies: HashMap<String, Vec<(&'a [HirStmt], Option<&'a HirContract>)>>,
    roots: HashSet<String>,
    live: HashSet<String>,
    keep_all: bool,
}

impl<'a> Builder<'a> {
    fn empty() -> Builder<'a> {
        Builder {
            defns: HashMap::new(),
            txns: HashMap::new(),
            contracts: HashMap::new(),
            type_members: HashMap::new(),
            member_bodies: HashMap::new(),
            roots: HashSet::new(),
            live: HashSet::new(),
            keep_all: false,
        }
    }

    /// Index every callable plus op-binding implementation fns.
    fn index_program(&mut self, program: &'a HirProgram) {
        for c in &program.callables {
            self.index_callable(c);
        }
        // 2026-11-05: op-binding implementation fns (`op Add: cstring_concat`)
        // join the owner type's member set, matching the AST index. Without
        // this, liveness misses binding fns on constructed types.
        for td in &program.type_defs {
            if td.binding_fns.is_empty() {
                continue;
            }
            self.type_members
                .entry(td.name.clone())
                .or_default()
                .extend(td.binding_fns.iter().cloned());
        }
    }

    /// Seed roots: global-initializer callees, always-stateful names,
    /// protocol binding fns, and caller-supplied extra roots.
    fn seed_roots(&mut self, program: &HirProgram, extra_roots: &HashSet<String>) {
        for g in &program.globals {
            if let Some(e) = &g.expr {
                let mut found = Vec::new();
                collect_call_names_expr(e, &mut found);
                self.roots.extend(found);
            }
        }
        for name in &program.always_stateful {
            self.roots.insert(name.clone());
        }
        self.roots.extend(program.protocol_binding_fns.iter().cloned());
        self.roots.extend(extra_roots.iter().cloned());
    }

    /// Liveness worklist drain.
    fn drain(&mut self) {
        let mut queue: Vec<String> = self.roots.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            if !self.live.insert(name.clone()) {
                continue;
            }
            self.expand_callable(&name, &mut queue);
        }
    }

    /// Walk one live callable's bodies/contracts and enqueue call names.
    fn expand_callable(&mut self, name: &str, queue: &mut Vec<String>) {
        if let Some(body) = self.defns.get(name).copied() {
            self.walk_stmts(body, queue);
        }
        if let Some(body) = self.txns.get(name).copied() {
            self.walk_stmts(body, queue);
        }
        if let Some(c) = self.contracts.get(name).copied() {
            self.walk_contract(c, queue);
        }
        if let Some(entries) = self.member_bodies.get(name).cloned() {
            for (body, contract) in entries {
                self.walk_stmts(body, queue);
                if let Some(c) = contract {
                    self.walk_contract(c, queue);
                }
            }
        }
    }
    fn index_callable(&mut self, c: &'a HirCallable) {
        // 2026-11-05 (H4.1 sweep): the AST index never records trait defaults
        // in `defns`/`type_members`; the backend emits specialized copies under
        // mangled names and tracks their dependencies separately. Bare-name
        // calls to a default must not root it, or HIR over-approximates
        // (caught on byte_sink/trait_generics/net_stack).
        if c.trait_default {
            return;
        }
        match c.kind {
            HirCallableKind::Transaction | HirCallableKind::MemberTransaction => {
                self.index_txn(c);
            }
            HirCallableKind::Defn | HirCallableKind::MemberDefn => self.index_defn(c),
            HirCallableKind::CompileTimeDefn | HirCallableKind::CompileTimeTxn => {}
        }
    }

    fn index_txn(&mut self, c: &'a HirCallable) {
        self.txns.entry(c.name.clone()).or_insert(&c.body.statements);
        self.index_contract(c);
        if let Some(owner) = &c.owner {
            self.type_members
                .entry(owner.clone())
                .or_default()
                .push(c.name.clone());
        }
        self.member_bodies
            .entry(c.name.clone())
            .or_default()
            .push((&c.body.statements, c.contract.as_ref()));
        // A member txn is usage-rooted; only top-level reactive / lifecycle /
        // async txns seed roots (AST parity).
        if c.owner.is_none() {
            if c.is_reactive || c.name.starts_with("__reset_") {
                self.roots.insert(c.name.clone());
            }
            if c.is_async {
                self.roots.insert("wait_for_trigger".into());
            }
        }
    }

    fn index_defn(&mut self, c: &'a HirCallable) {
        self.defns.entry(c.name.clone()).or_insert(&c.body.statements);
        self.index_contract(c);
        if let Some(owner) = &c.owner {
            self.type_members
                .entry(owner.clone())
                .or_default()
                .push(c.name.clone());
            self.member_bodies
                .entry(c.name.clone())
                .or_default()
                .push((&c.body.statements, c.contract.as_ref()));
        }
        if c.exported {
            self.roots.insert(c.name.clone());
        }
    }

    fn index_contract(&mut self, c: &'a HirCallable) {
        if let Some(ct) = &c.contract {
            self.contracts.entry(c.name.clone()).or_insert(ct);
        }
    }

    fn mark(&self, name: &str, queue: &mut Vec<String>) {
        if self.defns.contains_key(name) || self.txns.contains_key(name) {
            queue.push(name.to_string());
        }
    }

    fn on_construction(&self, base: &str, queue: &mut Vec<String>) {
        if let Some(members) = self.type_members.get(base) {
            queue.extend(members.iter().cloned());
        }
    }

    fn root_task_event_family(&self, queue: &mut Vec<String>) {
        for h in [
            "briev_task_spawn_impl",
            "briev_task_cancel_impl",
            "briev_await_impl",
            "briev_event_alloc_impl",
            "briev_event_ready_impl",
            "briev_event_read_impl",
            "briev_event_fire_impl",
        ] {
            self.mark(h, queue);
        }
    }

    fn walk_contract(&mut self, c: &'a HirContract, queue: &mut Vec<String>) {
        self.walk_expr(&c.pre, queue);
        self.walk_expr(&c.post, queue);
        let Some(w) = &c.watchdog else { return };
        self.walk_expr(&w.condition, queue);
        if let Some(fb) = &w.fallback {
            self.walk_expr(fb, queue);
        }
        if let Some(of) = &w.on_fire {
            self.mark(&of.handler, queue);
        }
    }

    fn walk_stmts(&mut self, stmts: &'a [HirStmt], queue: &mut Vec<String>) {
        for s in stmts {
            self.walk_stmt(s, queue);
        }
    }

    fn walk_stmt(&mut self, stmt: &'a HirStmt, queue: &mut Vec<String>) {
        match &stmt.kind {
            HirStmtKind::ArrowAssign { target, value, .. } => {
                if let Some(t) = target {
                    if let HirExprKind::Identifier(name) = &t.kind {
                        if name == "#StdOut" {
                            for h in intrinsic_helpers("Print#") {
                                self.mark(h, queue);
                            }
                            self.mark("print_str", queue);
                        }
                        if name == "#StdErr" {
                            self.mark("eprint_str", queue);
                        }
                    }
                    self.walk_expr(t, queue);
                }
                self.walk_expr(value, queue);
            }
            HirStmtKind::EndProgram(maybe_e) => {
                self.mark("exit_prog", queue);
                if let Some(e) = maybe_e {
                    self.walk_expr(e, queue);
                }
            }
            HirStmtKind::Foreach { list, body, .. } => {
                self.mark("briev_str_next_char", queue);
                self.walk_expr(list, queue);
                self.walk_stmts(body, queue);
            }
            HirStmtKind::FreeHint(_) => {
                self.mark("rt_free", queue);
                self.mark("briev_task_cancel_impl", queue);
            }
            HirStmtKind::Let { expr: Some(e), .. } => self.walk_expr(e, queue),
            HirStmtKind::Assign(l, r) => {
                self.walk_expr(l, queue);
                self.walk_expr(r, queue);
            }
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) | HirStmtKind::Check(e) => {
                self.walk_expr(e, queue)
            }
            HirStmtKind::Guarded(cond, body) => {
                self.walk_expr(cond, queue);
                self.walk_stmts(body, queue);
            }
            HirStmtKind::Gate(e) | HirStmtKind::Expression(e) => self.walk_expr(e, queue),
            HirStmtKind::Block(body)
            | HirStmtKind::Defer(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::SyncBlock(body) => self.walk_stmts(body, queue),
            HirStmtKind::Match { expr, arms } => {
                self.walk_expr(expr, queue);
                for a in arms {
                    self.walk_stmts(&a.body, queue);
                }
            }
            HirStmtKind::Rollback(Some(e)) => self.walk_expr(e, queue),
            HirStmtKind::TrgBinding { instance, .. } => self.walk_expr(instance, queue),
            HirStmtKind::Open(l, r) => {
                self.walk_expr(l, queue);
                self.walk_expr(r, queue);
            }
            HirStmtKind::Other(_) => self.keep_all = true,
            _ => {}
        }
    }

    fn walk_expr(&mut self, expr: &'a HirExpr, queue: &mut Vec<String>) {
        match &expr.kind {
            HirExprKind::Call { callee, args, .. } => self.walk_call(callee, args, queue),
            HirExprKind::MethodCall { receiver, name, args, .. } => {
                self.walk_method_call(receiver, name, args, queue)
            }
            HirExprKind::PluginIntercept { args, .. } => self.walk_lists(args, queue),
            HirExprKind::Spawn { type_name, args, .. } => self.walk_spawn(type_name, args, queue),
            HirExprKind::Reflect { receiver, name, kind } => {
                self.walk_reflect(receiver, name, *kind, queue)
            }
            HirExprKind::Binary { op, left, right } => self.walk_binary(*op, left, right, queue),
            HirExprKind::Index(obj, idx) => self.walk_index(obj, idx, queue),
            HirExprKind::Slice { array, start, end, stride } => {
                self.walk_slice(array, (start, end, stride), queue)
            }
            HirExprKind::StructLiteral { type_name, fields, .. } => {
                self.on_construction(type_name, queue);
                for (_, e) in fields {
                    self.walk_expr(e, queue);
                }
            }
            HirExprKind::Cast { inner, to, .. } => self.walk_cast(inner, to, queue),
            HirExprKind::Unary { inner, .. }
            | HirExprKind::IsType { inner, .. }
            | HirExprKind::Deref(inner)
            | HirExprKind::AddrOf(inner)
            | HirExprKind::Consume(inner)
            | HirExprKind::Await(inner)
            | HirExprKind::Field { receiver: inner, .. }
            | HirExprKind::Lambda { body: inner, .. } => self.walk_expr(inner, queue),
            HirExprKind::Range { start, end, .. } => {
                self.walk_expr(start, queue);
                self.walk_expr(end, queue);
            }
            HirExprKind::Block(stmts) => self.walk_stmts(stmts, queue),
            HirExprKind::If { condition, then_branch, else_branch } => {
                self.walk_expr(condition, queue);
                self.walk_expr(then_branch, queue);
                if let Some(e) = else_branch {
                    self.walk_expr(e, queue);
                }
            }
            HirExprKind::Match { scrutinee, arms } => {
                self.walk_expr(scrutinee, queue);
                for a in arms {
                    self.walk_expr(&a.body, queue);
                }
            }
            HirExprKind::Tuple(items) | HirExprKind::List(items) => {
                self.walk_lists(items, queue)
            }
            HirExprKind::Other(_) => self.keep_all = true,
            _ => {}
        }
    }

    fn walk_call(&mut self, callee: &str, args: &'a [HirExpr], queue: &mut Vec<String>) {
        self.mark(callee, queue);
        self.on_construction(callee, queue);
        for h in intrinsic_helpers(callee) {
            self.mark(h, queue);
        }
        if callee == "Asm#" {
            self.mark_asm_words(args.get(1), queue);
        }
        for a in args {
            self.walk_expr(a, queue);
        }
    }

    /// `Asm#` text literal: mark identifier-shaped words as call names.
    fn mark_asm_words(&mut self, arg: Option<&'a HirExpr>, queue: &mut Vec<String>) {
        let Some(HirExpr {
            kind: HirExprKind::Literal(HirLiteral::Bytes(t)),
            ..
        }) = arg
        else {
            return;
        };
        let text = String::from_utf8_lossy(t);
        let mut words: Vec<String> = Vec::new();
        let mut word = String::new();
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

    fn walk_method_call(
        &mut self,
        receiver: &'a HirExpr,
        name: &str,
        args: &'a [HirExpr],
        queue: &mut Vec<String>,
    ) {
        self.mark(name, queue);
        self.walk_expr(receiver, queue);
        for a in args {
            self.walk_expr(a, queue);
        }
    }

    fn walk_lists(&mut self, items: &'a [HirExpr], queue: &mut Vec<String>) {
        for e in items {
            self.walk_expr(e, queue);
        }
    }

    fn walk_spawn(&mut self, type_name: &str, args: &'a [HirExpr], queue: &mut Vec<String>) {
        self.mark(type_name, queue);
        self.mark("briev_task_spawn_impl", queue);
        self.on_construction(type_name, queue);
        self.root_task_event_family(queue);
        for a in args {
            self.walk_expr(a, queue);
        }
    }

    fn walk_reflect(
        &mut self,
        receiver: &'a HirExpr,
        name: &str,
        kind: crate::ast::ReflectKind,
        queue: &mut Vec<String>,
    ) {
        match kind {
            // 2026-11-04 (PR #62 parity): runtime metadata still evaluates
            // its receiver — calls under `value.^Length` must be live.
            crate::ast::ReflectKind::Runtime => self.walk_expr(receiver, queue),
            crate::ast::ReflectKind::CompileTime
                if matches!(name, "Size" | "Bytes" | "Alignment" | "Type") =>
            {
                self.walk_expr(receiver, queue)
            }
            crate::ast::ReflectKind::CompileTime => self.keep_all = true,
        }
    }

    fn walk_binary(
        &mut self,
        op: crate::ast::BinaryOpKind,
        left: &'a HirExpr,
        right: &'a HirExpr,
        queue: &mut Vec<String>,
    ) {
        self.mark("briev_str_eq", queue);
        self.mark("rt_free", queue);
        if matches!(op, crate::ast::BinaryOpKind::Add) {
            self.mark("coll_resize", queue);
        }
        self.walk_expr(left, queue);
        self.walk_expr(right, queue);
    }

    fn walk_index(&mut self, obj: &'a HirExpr, idx: &'a HirExpr, queue: &mut Vec<String>) {
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

    fn walk_slice(
        &mut self,
        array: &'a HirExpr,
        bounds: (
            &'a Option<Box<HirExpr>>,
            &'a Option<Box<HirExpr>>,
            &'a Option<Box<HirExpr>>,
        ),
        queue: &mut Vec<String>,
    ) {
        let (start, end, stride) = bounds;
        self.mark("briev_slice_range64", queue);
        self.mark("briev_slice_range_f32", queue);
        self.mark("briev_str_substr", queue);
        self.walk_expr(array, queue);
        for opt in [start, end, stride] {
            if let Some(e) = opt {
                self.walk_expr(e, queue);
            }
        }
    }

    fn walk_cast(&mut self, inner: &'a HirExpr, to: &crate::ast::Type, queue: &mut Vec<String>) {
        if matches!(to, crate::ast::Type::Custom(n) if n == "String") {
            for h in [
                "int_to_str",
                "uint_to_str",
                "bool_to_str",
                "float_to_str",
                "char_to_str",
            ] {
                self.mark(h, queue);
            }
        }
        self.walk_expr(inner, queue);
    }
}

/// Collect callable names referenced from a global initializer (AST roots
/// top-level statement callees; `mark` filters to known callables).
fn collect_call_names_expr(expr: &HirExpr, out: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::PluginIntercept { args, .. } => {
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        HirExprKind::Call { callee, args, .. } => {
            out.push(callee.clone());
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        HirExprKind::MethodCall { receiver, name, args, .. } => {
            out.push(name.clone());
            collect_call_names_expr(receiver, out);
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        HirExprKind::Spawn { type_name, args, .. } => {
            out.push(type_name.clone());
            for a in args {
                collect_call_names_expr(a, out);
            }
        }
        HirExprKind::StructLiteral { type_name, fields, .. } => {
            out.push(type_name.clone());
            for (_, e) in fields {
                collect_call_names_expr(e, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_call_names_expr(left, out);
            collect_call_names_expr(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => collect_call_names_expr(inner, out),
        HirExprKind::Index(a, b) => {
            collect_call_names_expr(a, out);
            collect_call_names_expr(b, out);
        }
        HirExprKind::Slice { array, start, end, stride } => {
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
        HirExprKind::Range { start, end, .. } => {
            collect_call_names_expr(start, out);
            collect_call_names_expr(end, out);
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_call_names_expr(condition, out);
            collect_call_names_expr(then_branch, out);
            if let Some(e) = else_branch {
                collect_call_names_expr(e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_call_names_expr(scrutinee, out);
            for a in arms {
                collect_call_names_expr(&a.body, out);
            }
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for e in items {
                collect_call_names_expr(e, out);
            }
        }
        HirExprKind::Block(stmts) => collect_call_names_stmts(stmts, out),
        HirExprKind::Reflect { receiver, .. } => collect_call_names_expr(receiver, out),
        _ => {}
    }
}

fn collect_call_names_stmts(stmts: &[HirStmt], out: &mut Vec<String>) {
    for s in stmts {
        collect_stmt_call_names(s, out);
    }
}

fn collect_stmt_call_names(s: &HirStmt, out: &mut Vec<String>) {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. } => collect_call_names_expr(e, out),
        HirStmtKind::Assign(l, r) => {
            collect_call_names_expr(l, out);
            collect_call_names_expr(r, out);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                collect_call_names_expr(t, out);
            }
            collect_call_names_expr(value, out);
        }
        HirStmtKind::Term(Some(e))
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Gate(e)
        | HirStmtKind::Expression(e) => collect_call_names_expr(e, out),
        HirStmtKind::Guarded(c, body) => {
            collect_call_names_expr(c, out);
            collect_call_names_stmts(body, out);
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_call_names_expr(list, out);
            collect_call_names_stmts(body, out);
        }
        HirStmtKind::Match { expr, arms } => {
            collect_call_names_expr(expr, out);
            for a in arms {
                collect_call_names_stmts(&a.body, out);
            }
        }
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Defer(b)
        | HirStmtKind::Mutex(b) => collect_call_names_stmts(b, out),
        HirStmtKind::TrgBinding { instance, .. } => collect_call_names_expr(instance, out),
        HirStmtKind::Open(l, r) => {
            collect_call_names_expr(l, out);
            collect_call_names_expr(r, out);
        }
        _ => {}
    }
}
