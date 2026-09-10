use crate::ast::{BinaryOpKind, Expr, OutputType, Statement, TopLevel, Type};
use crate::backend::llvm::emit_stmt::emit_statement;
use crate::backend::llvm::{float_to_llvm_hex, float64_to_llvm_hex, f32_to_f16_hex, llvm_type_byte_size, protocol_llvm_type, LlvmBackend, TypedRegister};
use crate::type_universe::{ResolvedType, TypeUniverse};
use crate::typechecker::defn_section_name;
use std::collections::HashSet;
use std::fmt::Write;
use std::sync::LazyLock;

/// 2026-08-18 (pooled-member iteration): peel the POOLED-COLUMN wrapper
/// `Vector(inner, [Anonymous(1)])` and split a type into its base name +
/// concrete generic args. A pooled member field (`items` in a pooled obj body)
/// is keyed `{prefix}.items` and typed `Vector(List<Int>, [Anonymous(1)])`;
/// the base/args under the wrapper drive tier-2 iteration and strategy
/// dispatch exactly like a bare `Applied`/`Custom` type.
fn peel_column_type(ty: &crate::ast::Type) -> Option<(String, Vec<crate::ast::Type>)> {
    let ty = match ty {
        crate::ast::Type::Vector(inner, _) => inner.as_ref(),
        other => other,
    };
    match ty {
        crate::ast::Type::Custom(n) => Some((n.clone(), Vec::new())),
        crate::ast::Type::Applied(n, a) => Some((n.clone(), a.clone())),
        _ => None,
    }
}

/// Arguments for one output-equality-gate element comparison (probe).
struct ProbeCmpArg<'a> {
    elem: &'a str,
    x: &'a str,
    y: &'a str,
    next: &'a str,
}

impl LlvmBackend {
    /// 2026-08-04 (compiler-in-Briev): top-level `let` names in a body that are
    /// ASSIGNED later (incl. inside when/if/foreach/sync blocks). Such lets get
    /// an entry-block alloca instead of a bare SSA register, so a reassignment
    /// inside a guard/loop stores into an entry alloca — the old demotion at the
    /// assignment site put the alloca in guard.thenN while the post-guard load
    /// lived in guard.endN (LLVM: "instruction does not dominate all uses").
    /// Only TOP-LEVEL lets are collected (a nested let's alloca would land in
    /// the nested block and re-introduce the same violation).
    fn collect_reassigned_lets(stmts: &[Statement]) -> std::collections::HashMap<String, Option<Type>> {
        use std::collections::HashMap;
        // 2026-08-28 (String ABI fix): recurse into EVERY body-carrying
        // statement form. Previously only top-level lets were pre-declared,
        // so `when c { let out = ""; out = out + " "; };` demoted `out` at
        // the ASSIGN SITE (inside guard.then) while the read at guard.end
        // was not dominated — "Instruction does not dominate all uses".
        // Nested lets that are assigned anywhere now pre-declare entry
        // allocas like top-level ones.
        fn collect_lets(s: &Statement, out: &mut std::collections::HashMap<String, Option<Type>>) {
            if let Statement::Let { name, ty, .. } = s {
                out.insert(name.clone(), ty.clone());
            }
        }
        fn walk(s: &Statement, lets: &mut std::collections::HashMap<String, Option<Type>>, assigned: &mut HashSet<String>) {
            collect_lets(s, lets);
            match s {
                Statement::Assign(Expr::Identifier(name), _) => { assigned.insert(name.clone()); }
                Statement::ArrowAssign { target: Some(e), .. } => {
                    if let Expr::Identifier(name) = e.as_ref() { assigned.insert(name.clone()); }
                }
                Statement::Guarded(_, body) | Statement::Block(body) | Statement::SyncBlock(body)
                | Statement::Defer(body) | Statement::Mutex(body) => {
                    for st in body { walk(st, lets, assigned); }
                }
                Statement::Foreach { body, .. } => {
                    for st in body { walk(st, lets, assigned); }
                }
                Statement::Barrier { body, .. } => {
                    for st in body { walk(st, lets, assigned); }
                }
                Statement::Match { arms, .. } => {
                    for arm in arms {
                        for st in &arm.body { walk(st, lets, assigned); }
                    }
                }
                _ => {}
            }
        }
        let mut lets: HashMap<String, Option<Type>> = HashMap::new();
        let mut assigned: HashSet<String> = HashSet::new();
        for s in stmts { walk(s, &mut lets, &mut assigned); }
        lets.into_iter().filter(|(n, _)| assigned.contains(n)).collect()
    }
    /// Check if any modifier has the given name and extract its export name.
    /// Returns Some(export_name) if #export or #export("name") was found.
    /// 2026-07-14: string_value() removed from Annotation. Extract from tag.value instead.
    pub fn get_export_name(modifiers: &[crate::ast::Annotation]) -> Option<String> {
        for tag in modifiers {
            if tag.name == "export" {
                let export_name = tag.value.as_ref().and_then(|v| {
                    if let Expr::Quoted(bytes) = v {
                        Some(String::from_utf8_lossy(bytes).to_string())
                    } else {
                        None
                    }
                }).unwrap_or_else(|| tag.name.clone());
                if export_name == "export" {
                    return None;
                }
                return Some(export_name);
            }
        }
        None
    }

    /// Emit cleanup calls for all local variables whose type has an
    /// OnExit foreign destructor. Called at scope exit points.
    fn emit_on_exit_cleanup(&mut self, out: &mut String, indent: &str) {
        let Some(ref universe) = self.ctx.type_universe else { return };        for (name, ty) in &self.fun.let_binding_types {
            let type_name = match ty {
                crate::ast::Type::Custom(n) => n,
                crate::ast::Type::Applied(n, _) => n,
                _ => continue,
            };
            let Some(resolved) = universe.types.get(type_name) else { continue };
            // 2026-07-14: on_exit removed from ResolvedType, check properties["on_exit"] instead.
            let on_exit_fn = resolved.properties.get("on_exit").and_then(|pv| {
                if let crate::ast::PropertyValue::Identifier(s) = pv { Some(s.clone()) } else { None }
            });
            let Some(ref on_exit_fn) = on_exit_fn else { continue };
            let Some(reg) = self.fun.let_bindings.get(name) else { continue };
            // Emit: call void @on_exit_fn(i64 %reg)
            writeln!(out, "{}{} = call i64 @{}(i64 {})",
                indent,
                format!("%pcl{}", self.fun.txn_counter),
                on_exit_fn,
                reg
            ).ok();
            self.fun.txn_counter += 1;
        }
    }

    /// 2026-08-09 (Phase 10): emit all registered `defer` bodies LIFO and
    /// clear the stack. Called before every exit of a transaction/reactive
    /// firing (term, rollback, fallthrough ret). A defer body may itself
    /// register more defers — they are collected and flushed too (each runs
    /// exactly once).
    pub(super) fn flush_defer_cleanup(&mut self, out: &mut String, indent: &str) {
        while let Some(body) = self.fun.defer_bodies.pop() {
            for stmt in &body {
                crate::backend::llvm::emit_stmt::emit_statement(self, out, stmt, indent);
            }
        }
    }

    /// 2026-08-18 (Phase C, BUGS.md member-field arrow): the base type name of
    /// a collection identifier — a function local, a state field, or a BARE
    /// member name in a pooled member body (the field slot is `{prefix}.{name}`
    /// like `PiggyBank.items`, never the bare name). A pooled slot's briev type
    /// is the COLUMN type `Vector(inner, [Anonymous(1)])` (e.g. `List<Int>[1]`)
    /// — peel the Vector wrapper before extracting the base. Shared by the
    /// strategy dispatchers (insert/extract) so the member-field arrow finds
    /// its InsertAt/ExtractFrom binding; without it the push was a no-op.
    pub(super) fn collection_base_type_name(&self, var_name: &str) -> Option<String> {
        fn base_name(ty: &crate::ast::Type) -> Option<String> {
            peel_column_type(ty).map(|(b, _)| b)
        }
        if let Some(ty) = self.fun.let_original_types.get(var_name) {
            return base_name(ty);
        }
        if let Some(&idx) = self.ctx.field_index_map.get(var_name) {
            let ty = self.ctx.field_briev_types.get(idx)?;
            return base_name(ty);
        }
        if let Some((prefix, _)) = &self.fun.self_prefix {
            let slot = format!("{}.{}", prefix, var_name);
            if let Some(&idx) = self.ctx.field_index_map.get(&slot) {
                let ty = self.ctx.field_briev_types.get(idx)?;
                return base_name(ty);
            }
        }
        // 2026-08-18 (Phase D, PiggyBank): a POOLED INSTANCE identifier
        // (`all ~<- piggy`, `piggy <- 1`) has no top-level slot — resolve the
        // instance prefix's base so the strategy dispatchers find its
        // ExtractFrom/InsertAt bindings.
        if let Some(ty) = self.resolve_id_type(var_name) {
            return base_name(&ty);
        }
        None
    }

    /// Check the target expression for an InsertAt strategy by looking up
    /// the variable's type in the TypeUniverse.
    fn lookup_strategy_type_name(&self, var_name: &str) -> Option<String> {
        self.collection_base_type_name(var_name)
    }

    pub(super) fn find_insert_strategy(&self, target: &crate::ast::Expr) -> Option<&crate::ast::top::OperatorDef> {
        // 2026-08-12 (slice 2 gap 3): peel AddrOf layers — the documented push
        // form `&items <- 3` lowers the target to AddrOf(Identifier(items));
        // without the peel the strategy lookup returned None and the push was
        // SILENTLY DROPPED (the arrow fell through to a plain assign).
        let mut t = target;
        while let crate::ast::Expr::AddrOf(inner) = t {
            t = inner;
        }
        let var_name = t.as_var_name()?;
        let type_name = self.lookup_strategy_type_name(var_name)?;
        self.ctx.operator_defs.get(&type_name)?
            .iter().find(|d| d.op == "InsertAt")
    }

    /// 2026-08-18 (Phase D): the declared type of an identifier, across locals,
    /// state fields, self-prefixed pooled slots, and pooled INSTANCES (whose
    /// base comes from `obj_instance_inits` — a top-level `let piggy:
    /// PiggyBank<Int> = 0` has no state slot). Used to type the arrow's VALUE
    /// without emitting it (an instance identifier emits as the scalar Int
    /// fallback, which would defeat the insert/extract gate).
    pub(super) fn resolve_id_type(&self, name: &str) -> Option<crate::ast::Type> {
        self.fun.let_original_types.get(name).cloned()
            .or_else(|| self.fun.let_binding_types.get(name).cloned())
            .or_else(|| {
                let &idx = self.ctx.field_index_map.get(name)?;
                self.ctx.field_briev_types.get(idx).cloned()
            })
            .or_else(|| {
                let (prefix, _) = self.fun.self_prefix.as_ref()?;
                let slot = format!("{}.{}", prefix, name);
                let &idx = self.ctx.field_index_map.get(&slot)?;
                self.ctx.field_briev_types.get(idx).cloned()
            })
            .or_else(|| {
                let (prefix, _row) = self.unpacked_instance_prefix(name)?;
                self.ctx.obj_instance_inits.get(&prefix).and_then(|(base, _)| {
                    self.ctx.obj_type_params.get(base).map(|_| {
                        crate::ast::Type::Custom(base.clone())
                    })
                }).or_else(|| Some(crate::ast::Type::Custom(prefix)))
            })
    }

    /// 2026-08-18 (Phase D, PiggyBank): the element type of a target's
    /// `InsertAt` op — mirrors the typechecker's `push_element_type`. The arrow
    /// codegen must compare the value's type against this BEFORE emitting the
    /// push: `all ~<- piggy` (target `all: List<Int>`, value a PiggyBank) must
    /// NOT push — the typechecker already chose the EXTRACT (smash) path, and
    /// an unconditional push would insert the jar handle as an i64.
    pub(super) fn insert_element_type(&self, target: &crate::ast::Expr) -> Option<crate::ast::Type> {
        use crate::ast::TopLevel;
        let mut t = target;
        while let crate::ast::Expr::AddrOf(inner) = t {
            t = inner;
        }
        let name = t.as_var_name()?;
        let full_ty = self.resolve_id_type(name)?;
        let (base, args) = match &full_ty {
            crate::ast::Type::Custom(n) => (n.clone(), Vec::new()),
            crate::ast::Type::Applied(n, a) => (n.clone(), a.clone()),
            _ => return None,
        };
        let members = self.ctx.obj_members.get(&base)?;
        let params = self.ctx.obj_type_params.get(&base).cloned().unwrap_or_default();
        let subst: std::collections::HashMap<String, crate::ast::Type> =
            params.into_iter().zip(args.into_iter()).collect();
        // op-as-member `op InsertAt(v: T) { … }` — first param is the element.
        let member = members.iter().find_map(|m| match m {
            TopLevel::TypeDefOperator(d) if d.name == "InsertAt" => d.parameters.first().map(|(_, ty)| ty.clone()),
            _ => None,
        }).or_else(|| {
            // Binding `op InsertAt: push(#Lh, #Rh)` → the impl member's param.
            let binding = self.ctx.operator_defs.get(&base)?.iter().find(|d| d.op == "InsertAt")?;
            let fn_name = match binding.impl_args.as_ref()? {
                crate::ast::PropertyValue::Identifier(s) => s.clone(),
                crate::ast::PropertyValue::List(items) => match items.first() {
                    Some(crate::ast::PropertyValue::Identifier(f)) => f.clone(),
                    _ => return None,
                },
                _ => return None,
            };
            let m = members.iter().find(|m| crate::backend::llvm::emit_expr::member_briev_name(m) == fn_name)?;
            let params = match m {
                TopLevel::Definition(d) => &d.parameters,
                TopLevel::Transaction(t) => &t.parameters,
                TopLevel::TypeDefOperator(d) => &d.parameters,
                _ => return None,
            };
            params.first().map(|(_, ty)| ty.clone())
        })?;
        Some(crate::typechecker::substitute_type(&member, &subst))
    }

    /// 2026-08-12 (Iterable protocol, Tier 2, SPEC §11.4): does the list
    /// expression name a value whose type exposes `op Count` + `op At` as
    /// op-as-member operators? If so, returns `(element type, base name)` —
    /// the element type is the At member's return with the concrete generic
    /// args substituted (`List<String>` At → `T` → `String`). Structural
    /// iteration: the compiler knows only the operator surface, never a layout.
    pub(super) fn tier2_op_collection(&self, list: &crate::ast::Expr) -> Option<(crate::ast::Type, String)> {
        use crate::ast::TopLevel;
        // 2026-08-18: the iterated expression is either a bare name (`items`,
        // a POOLED MEMBER slot `{prefix}.items`) or a FIELD ACCESS
        // (`ledger.items`) — both resolve through the column/slot type and are
        // peeled of the `Vector(inner, [Anonymous(1)])` column wrapper.
        // Previously only bare let/state names resolved and the foreach
        // panicked at emit_stmt.rs:262.
        let full_ty = match list {
            crate::ast::Expr::Identifier(name) => self.resolve_id_type(name)?,
            crate::ast::Expr::Field(recv, field) => {
                let recv_name = recv.as_var_name()?;
                let slot = format!("{}.{}", recv_name, field);
                if let Some(&idx) = self.ctx.field_index_map.get(&slot) {
                    self.ctx.field_briev_types.get(idx).cloned()?
                } else {
                    // A boxed/local receiver's member: the receiver type's
                    // struct field, substituted with the receiver's args.
                    let (base, args) = peel_column_type(&self.resolve_id_type(recv_name)?)?;
                    let fields = self.ctx.struct_types.get(&base)?;
                    let (_, fty) = fields.iter().find(|(n, _)| n == field)?;
                    let params = self.ctx.obj_type_params.get(&base).cloned().unwrap_or_default();
                    let subst: std::collections::HashMap<String, crate::ast::Type> =
                        params.into_iter().zip(args.into_iter()).collect();
                    crate::typechecker::substitute_type(fty, &subst)
                }
            }
            _ => return None,
        };
        let (base, args) = peel_column_type(&full_ty)?;
        let members = self.ctx.obj_members.get(&base)?;
        let has_count = members
            .iter()
            .any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == "Count"));
        let has_at = members
            .iter()
            .any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == "At"));
        if !(has_count && has_at) {
            return None;
        }
        let at = members.iter().find_map(|m| match m {
            TopLevel::TypeDefOperator(d) if d.name == "At" => Some(d),
            _ => None,
        })?;
        let raw_elem = at.output_type
            .as_ref()
            .and_then(|o| o.all_types().into_iter().next())
            .unwrap_or_else(crate::ast::Type::int);
        let params = self.ctx.obj_type_params.get(&base).cloned().unwrap_or_default();
        let subst: std::collections::HashMap<String, crate::ast::Type> =
            params.into_iter().zip(args.into_iter()).collect();
        let element_ty = crate::typechecker::substitute_type(&raw_elem, &subst);
        Some((element_ty, base))
    }

    /// 2026-08-14 (Iterable protocol, slice-6 literal TYPES): does a Briev
    /// TYPE expose the Tier-2 `op Count`/`op At` surface? Used by call-site
    /// literal marshaling (`emit_user_call`): a `[1,2,3]` literal argument to
    /// a param declared `List<T>` constructs through the collection's OWN ops
    /// (`op Init`/`op InsertAt`) — never the stale `[len][elems]` heap-seq
    /// layout the members can't read. The layout tag lives in the type's
    /// member surface, not in a hardcoded name.
    pub(super) fn tier2_collection_type(&self, ty: &crate::ast::Type) -> bool {
        use crate::ast::TopLevel;
        let base = match ty {
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.clone(),
            _ => return false,
        };
        let members = self.ctx.obj_members.get(&base);
        match members {
            Some(members) => {
                let has_count = members
                    .iter()
                    .any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == "Count"));
                let has_at = members
                    .iter()
                    .any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == "At"));
                has_count && has_at
            }
            None => false,
        }
    }

    /// 2026-08-12 (Iterable protocol, Tier 1, SPEC §11.4.1): does the list
    /// expression name a value whose type exposes `op Iter`/`op Step`/
    /// `op IsEnd`/`op Current` as op-as-member operators (and NOT Tier 2's
    /// Count/At)? If so, returns `(element type, base name)` — the element
    /// type is the Current member's return with the concrete args substituted.
    pub(super) fn tier1_cursor_collection(
        &self,
        list: &crate::ast::Expr,
    ) -> Option<(crate::ast::Type, String)> {
        use crate::ast::TopLevel;
        let crate::ast::Expr::Identifier(name) = list else { return None; };
        let full_ty = self.identifier_collection_type(name)?;
        let base = match &full_ty {
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.clone(),
            _ => return None,
        };
        let members = self.ctx.obj_members.get(&base)?;
        let has = |op: &str| {
            members.iter().any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == op))
        };
        // Tier 1 requires the cursor ops; Tier 2 (Count/At) takes priority and
        // is handled by tier2_op_collection first.
        if !(has("Iter") && has("Step") && has("IsEnd") && has("Current")) {
            return None;
        }
        if has("Count") && has("At") {
            return None;
        }
        let current = members.iter().find_map(|m| match m {
            TopLevel::TypeDefOperator(d) if d.name == "Current" => Some(d),
            _ => None,
        })?;
        let raw_elem = current.output_type
            .as_ref()
            .and_then(|o| o.all_types().into_iter().next())
            .unwrap_or_else(crate::ast::Type::int);
        let args = match &full_ty {
            crate::ast::Type::Applied(_, a) => a.clone(),
            _ => Vec::new(),
        };
        let params = self.ctx.obj_type_params.get(&base).cloned().unwrap_or_default();
        let subst: std::collections::HashMap<String, crate::ast::Type> =
            params.into_iter().zip(args.into_iter()).collect();
        let element_ty = crate::typechecker::substitute_type(&raw_elem, &subst);
        Some((element_ty, base))
    }

    /// 2026-08-12 (Iterable protocol): the FULL type of an identifier used as
    /// an iterable — a function local, a state field, or a POOLED instance
    /// (`let m = spawn HashMap(0)` — the identifier isn't a field, so its
    /// base comes from the instance mapping; the args are recovered from the
    /// monomorphized obj key when available).
    fn identifier_collection_type(&self, name: &str) -> Option<crate::ast::Type> {
        if let Some(ty) = self.fun.let_original_types.get(name) {
            return Some(ty.clone());
        }
        if let Some(&idx) = self.ctx.field_index_map.get(name) {
            return self.ctx.field_briev_types.get(idx).cloned();
        }
        if let Some((base, _)) = self.unpacked_instance_prefix(name) {
            // 2026-08-12: a POOLED instance (`let m = spawn HashMap(0)`)
            // unpacks into columns — the identifier isn't a field; the base
            // comes from the instance mapping. The concrete generic args are
            // recovered from the monomorphized member types at the member call
            // (emit_method_call resolves the mono key); the element-type
            // pre-derivation here uses the base only (a generic `V` element
            // type is refined to the concrete type by the Current member's
            // mono emission).
            return Some(crate::ast::Type::Custom(base));
        }
        None
    }

    /// 2026-07-20: Find an OperatorDef for ExtractFrom by looking up the
    /// variable's type in the operator_defs map (populated from AST).
    /// Returns None when the type has no ExtractFrom operator definition.
    /// 2026-08-18 (Phase D): the value-side op of an arrow, selected by the
    /// arrow's CONSUME flag — `dest <- src` (read) prefers `CopyFrom`, the
    /// destructive `dest ~<- src` prefers `ExtractFrom`, the other is the
    /// fallback (mirrors the typechecker's extract_op_order). A PiggyBank's
    /// `all ~<- piggy` therefore finds `ExtractFrom` (smash), while `x <- piggy`
    /// resolves `CopyFrom` (the sealed read error — rejected in typechecking).
    pub(super) fn find_extract_strategy_for_arrow(
        &self,
        target: &crate::ast::Expr,
        consume: bool,
    ) -> Option<&crate::ast::top::OperatorDef> {
        let (first, second) = if consume {
            ("ExtractFrom", "CopyFrom")
        } else {
            ("CopyFrom", "ExtractFrom")
        };
        self.find_extract_strategy_op(target, first)
            .or_else(|| self.find_extract_strategy_op(target, second))
    }

    pub(super) fn find_extract_strategy(&self, target: &crate::ast::Expr) -> Option<&crate::ast::top::OperatorDef> {
        self.find_extract_strategy_op(target, "ExtractFrom")
    }

    fn find_extract_strategy_op(
        &self,
        target: &crate::ast::Expr,
        op: &str,
    ) -> Option<&crate::ast::top::OperatorDef> {
        // 2026-08-01 (A10): peel AddrOf layers — `<- &st` lowers to
        // Expression(AddrOf(AddrOf(Identifier))) and the plain `<- st` to
        // AddrOf(Identifier); the strategy lookup must reach the identifier.
        let mut t = target;
        while let crate::ast::Expr::AddrOf(inner) = t {
            t = inner;
        }
        let var_name = t.as_var_name()?;
        let type_name = self.lookup_strategy_type_name(var_name)?;
        let def = self.ctx.operator_defs.get(&type_name)?
            .iter().find(|d| d.op == op)?;
        // 2026-08-18 (Phase D): the arrow supplies NO arguments — a
        // parameterized member (the coll's CopyFrom `get(i)`) can never be the
        // target. Only a zero-param member reads/extracts.
        let fn_name = match def.impl_args.as_ref()? {
            crate::ast::PropertyValue::Identifier(s) => s.clone(),
            crate::ast::PropertyValue::List(items) => match items.first() {
                Some(crate::ast::PropertyValue::Identifier(f)) => f.clone(),
                _ => return None,
            },
            _ => return None,
        };
        let member = self.ctx.obj_members.get(&type_name)?.iter()
            .find(|m| crate::backend::llvm::emit_expr::member_briev_name(m) == fn_name)?;
        let params = match member {
            crate::ast::TopLevel::Definition(d) => &d.parameters,
            crate::ast::TopLevel::Transaction(t) => &t.parameters,
            crate::ast::TopLevel::TypeDefOperator(d) => &d.parameters,
            _ => return None,
        };
        if params.is_empty() { Some(def) } else { None }
    }

    pub(super) fn emit_header(&self, out: &mut String) {
        writeln!(out, "; ModuleID = 'program.ll'").ok();
        writeln!(out, "source_filename = \"program.bv\"").ok();
        // 2026-07-11: Phase 6 — target triple and data layout are now
        // configurable via CompilerContext.target_triple / .data_layout.
        if let Some(ref dl) = self.ctx.data_layout {
            writeln!(out, "target datalayout = \"{}\"", dl).ok();
        }
        writeln!(out, "target triple = \"{}\"", self.ctx.target_triple).ok();
    }

    /// Emit LLVM struct type declarations for user-defined struct types.
    /// Each struct becomes a named LLVM type so foreign callers (Rust via LTO,
    /// Python via ctypes) can match the memory layout. All fields are boxed
    /// as i64 (Briev's universal scalar storage type).
    ///
    /// Called after `emit_header()` and before `emit_declares()` so that
    /// struct types are available for use in function signatures emitted
    /// later. Structs with no fields are emitted as `{}` (empty type).
    ///
    /// 2026-07-10: Phase 1 — zero-copy GLUE bridge struct type declarations.
    pub(super) fn declare_struct_types(&self, out: &mut String) {
        writeln!(out).ok();
        // 2026-07-30: Emit known struct types from the universe (String, SmallString64,
        // UTF8View, Slice, etc.) BEFORE user-defined struct types. These affect LLVM's
        // struct layout and alignment computation — without them, LICM/sinkRegion in
        // clang 18.1.3 crashes on loops that reference these types through FFI calls.
        // Also hardcode common stdlib types that may not have universe entries if the
        // importing benchmark doesn't reference them directly.
        //
        // 2026-08-01 (B4): the legacy struct-type declarations (SmallString64,
        // StaticString, UTF8View) were retired with their types — nothing
        // references them under the bits model (String is a bare #String ptr).
        let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Some(u) = &self.ctx.type_universe {
            let mut universe_fields: Vec<(String, Vec<String>, bool, bool)> = Vec::new();
            for rt in u.types.values() {
                if rt.fields.is_empty() { continue; }
                if emitted.contains(&rt.name) { continue; }
                // 2026-08-13 (layout-keywords plan): `pack struct` emission.
                // Whole-byte packed (every field width % 8 == 0) uses LLVM's
                // native packed type `<{ ... }>` (no inter-element padding, GEP
                // works as usual). Sub-byte packed collapses to a byte array
                // `{ [N x i8] }` (N = storage bytes) with per-field bit-slices.
                let packed = self.ctx.packed_structs.contains(&rt.name);
                if packed {
                    // 2026-08-13: zero-width `Bits<0>` padding fields are
                    // filtered out of the aggregate (i0 is not a valid LLVM
                    // element type); they occupy no storage. Element types
                    // come from the packed authority (field_bits), so
                    // `Bits<48>` (Applied-form in source) emits i48, not the
                    // generic-application default i64.
                    let field_tys: Vec<String> = rt.fields.iter()
                        .filter(|(_, fty)| crate::type_universe::field_bits(fty, Some(u)) != 0)
                        .map(|(_, fty)| format!("i{}", crate::type_universe::field_bits(fty, Some(u))))
                        .collect();
                    let whole_byte = crate::type_universe::is_whole_byte_packed(&rt.fields, Some(u));
                    universe_fields.push((rt.name.clone(), field_tys, true, whole_byte));
                    continue;
                }
                let field_tys: Vec<String> = rt.fields.iter()
                    .map(|(_, fty)| self.llvm_type(fty))
                    .collect();
                universe_fields.push((rt.name.clone(), field_tys, false, true));
            }
            universe_fields.sort_by_key(|(k, _, _, _)| k.clone());
            for (name, field_tys, packed, whole_byte) in &universe_fields {
                // 2026-08-13 (Phase 6): a union is a byte array of its largest
                // aligned field storage; fields overlay at offset 0.
                if self.ctx.unions.contains(name) {
                    let n = u.types.get(name).map(|r| r.bytes).unwrap_or(1).max(1);
                    writeln!(out, "%{} = type {{ [{} x i8] }}", name, n).ok();
                } else if *packed && !*whole_byte {
                    // Sub-byte packed: hide fields behind a byte array. N is the
                    // registered storage size (ceil(Σ bits / 8)).
                    let n = u.types.get(name).map(|r| r.bytes).unwrap_or(1);
                    writeln!(out, "%{} = type {{ [{} x i8] }}", name, n).ok();
                } else if *packed {
                    writeln!(out, "%{} = type <{{ {} }}>", name, field_tys.join(", ")).ok();
                } else {
                    writeln!(out, "%{} = type {{ {} }}", name, field_tys.join(", ")).ok();
                }
                emitted.insert(name.clone());
            }
        }
        if self.ctx.struct_types.is_empty() {
            return;
        }
        // Iteration order MUST be sorted by key for deterministic IR emission.
        let mut sorted: Vec<(String, Vec<(String, Type)>)> = self.ctx.struct_types.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        sorted.sort_by_key(|(k, _)| k.clone());
        for (name, fields) in &sorted {
            // 2026-07-31: skip if the universe already declared this type.
            if emitted.contains(name.as_str()) { continue; }
            // 2026-08-28 (Phase 3): mono keys ("Stack<Int, 8>") are not valid
            // LLVM identifiers — mangle non-identifier chars. Nothing
            // references the mono declaration by name (obj values are boxed
            // i64 handles, field access is i8-GEP by offset); the sanitized
            // declaration only keeps the type table complete and legal.
            let ident: String = name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '.' })
                .collect();
            if fields.is_empty() {
                writeln!(out, "%{} = type {{}}", ident).ok();
            } else {
                // 2026-08-13: packed structs are always declared via the
                // universe path above; this path stays for plain structs.
                // 2026-08-16 (Phase 3a): field types go through llvm_type so
                // a fixed `T[N]` member declares `[N x i64]`, not the scalar
                // i64 collapse that misaligned inline-array coll structs.
                let field_tys: Vec<String> = fields.iter()
                    .map(|(_, fty)| self.llvm_type(fty))
                    .collect();
                writeln!(out, "%{} = type {{ {} }}", ident, field_tys.join(", ")).ok();
            }
        }
    }

    pub(super) fn emit_declares(&self, out: &mut String) {
        writeln!(out).ok();
        writeln!(out, "declare void @llvm.assume(i1) #1").ok();
        writeln!(out, "declare void @llvm.trap() noreturn").ok();
        // Intrinsic declares used by name#() instrinsic calls in emit_expr.
        // Previously these came from std/llvm.bv via as intrinsic, but the
        // name#() mechanism emits them directly without frgn_map entries.
        writeln!(out, "declare float @llvm.sqrt.f32(float) #1").ok();
        writeln!(out, "declare float @llvm.fabs.f32(float) #1").ok();
        writeln!(out, "declare float @llvm.ceil.f32(float) #1").ok();
        writeln!(out, "declare float @llvm.floor.f32(float) #1").ok();
        // 2026-06-29: Double-precision (Float64) intrinsic variants
        writeln!(out, "declare double @llvm.sqrt.f64(double) #1").ok();
        writeln!(out, "declare double @llvm.fabs.f64(double) #1").ok();
        writeln!(out, "declare double @llvm.ceil.f64(double) #1").ok();
        writeln!(out, "declare double @llvm.floor.f64(double) #1").ok();
        writeln!(out, "declare i64 @llvm.ctpop.i64(i64) #1").ok();
        writeln!(out, "declare i64 @llvm.ctlz.i64(i64, i1) #1").ok();
        writeln!(out, "declare i64 @llvm.cttz.i64(i64, i1) #1").ok();
        writeln!(out, "declare i64 @llvm.abs.i64(i64, i1) #1").ok();
        writeln!(out, "declare i64 @llvm.bitreverse.i64(i64) #1").ok();
        // Runtime support functions
        // (__thread_pool_init__ deleted with the pthread pool - Family H.)
        // 2026-07-01: Stores the current state snapshot pointer for worker threads.
        // Called by main before __barrier_release__ so async body functions receive
        // the correct state argument instead of a garbage pointer.
        writeln!(out, "declare i64 @time(ptr) nounwind").ok();
        // 2026-07-28: atol and getenv used by GetEnvInt# intrinsic.
        writeln!(out, "declare i64 @atol(ptr) nounwind").ok();
        writeln!(out, "declare ptr @getenv(ptr) nounwind").ok();
        writeln!(out, "declare noalias ptr @malloc(i64) nounwind").ok();
        writeln!(out, "declare void @free(ptr) nounwind").ok();
        // 2026-08-09 (Phase 12, SPEC §19.3): `feature.^^Available` — the
        // optional-frgn runtime availability check (briev_rt.c).
        writeln!(out, "declare i64 @briev_symbol_available(ptr)").ok();
        // 2026-08-01 (D2): garbage scheduling — scheduled frees route through
        // __briev_free so a benchmark can assert frees == allocs. argmemonly:
        // the call only touches memory via its pointer arg, so a scheduled
        // free after a loop must not make LLVM treat the whole function's
        // memory as clobbered (without it, hash_ops' loop ran 23x slower —
        // LLVM could not promote the state slots to registers).
        if !self.ctx.defn_params.contains_key("__briev_free") {
            writeln!(out, "declare void @__briev_free(ptr) nounwind argmemonly").ok();
        }
        // 2026-08-01 (D2): `Now#` — monotonic clock for the watchdog
        // `within N ms` deadline compare.
        // 2026-09-10 (Family I): declare-guarded — the pure-Briev defn takes
        // the hidden %state, so an unconditional () declare would conflict.
        if !self.ctx.defn_params.contains_key("__briev_now") {
            writeln!(out, "declare i64 @__briev_now() nounwind").ok();
        }
        // 2026-08-23 (process.bv revival): process/environment helpers.
        writeln!(out, "declare i64 @__briev_spawn(ptr) nounwind").ok();
        writeln!(out, "declare ptr @__briev_spawn_output(ptr)").ok();
        writeln!(out, "declare i64 @__briev_setenv(ptr, ptr) nounwind").ok();
        if !self.ctx.defn_params.contains_key("__briev_getcwd") {
            writeln!(out, "declare ptr @__briev_getcwd()").ok();
        }
        if !self.ctx.defn_params.contains_key("__briev_chdir") {
            writeln!(out, "declare i64 @__briev_chdir(ptr) nounwind").ok();
        }
        // getenv/strlen already declared at line ~638 (GetEnv# path).
        writeln!(out, "declare i64 @strlen(ptr)").ok();
        // 2026-06-26: realloc used by the arena allocator grow path when
        // the bump-allocated buffer is exhausted (emit_arena_alloc in mod.rs).
        writeln!(out, "declare ptr @realloc(ptr, i64) nounwind").ok();
        writeln!(out, "declare i64 @ShellCmd(i64)").ok();
        // 2026-07-26: ~50 dead declares removed — no Rust code path generated
        // calls to them. Only ShellCmd is kept (called via ShellCmd# intrinsic).
        // 2026-07-15: Raw OS syscall (SysCall# intrinsic)
        writeln!(out, "declare i64 @briev_syscall(i64, i64, i64, i64, i64, i64, i64)").ok();
        // 2026-07-15: Runtime system configuration (SysConf# intrinsic)
        writeln!(out, "declare i64 @briev_sysconf(i64)").ok();
        // 2026-07-15: Dynamic linker (DlOpen#/DlSym#/DlClose# intrinsics)
        writeln!(out, "declare ptr @dlopen(ptr, i32) nounwind").ok();
        writeln!(out, "declare ptr @dlsym(ptr, ptr) nounwind").ok();
        writeln!(out, "declare i32 @dlclose(ptr) nounwind").ok();
        // 2026-07-15: Stack backtrace (Backtrace# intrinsic)
        writeln!(out, "declare i64 @briev_backtrace()").ok();
        // 2026-07-15: POSIX socket/ioctl declarations removed — they conflict
        // with the defn wrappers in std/os/ (which now use SysCall# internally).
    }

    /// 2026-07-08: Fallback for types not in the universe (test code,
    /// custom types before registration).
    /// 2026-07-19: Fixed Float→"float" vs Float64→"double" mapping.
    /// The normalizer handles all properly registered types via category
    /// inference + config lookup; this is a last-resort fallback.
    fn fallback_llvm_type(ty: &Type) -> &'static str {
        match ty {
            Type::Ptr(_) => "ptr",
            Type::Void => "void",
            // 2026-08-13 (layout-keywords plan): Bits stores bits; the match
            // arms below are already bit widths, so the byte-era `* 8` is gone.
            Type::Bits(bits) => match bits {
                1 => "i1",
                8 => "i8",
                16 => "i16",
                32 => "i32",
                64 => "i64",
                _ => "i64",
            },
            _ => "i64",
        }
    }

    pub(super) fn llvm_type(&self, ty: &Type) -> String {
        // 2026-08-22 (spec-conformance Phase 3b): a structural sum
        // (`Int | String`) is a TAGGED HANDLE — i64 pointing at a 16-byte
        // image `{ i64 tag, i64 payload }`. Members are boxed/unboxed at the
        // seams (call args, returns, lets, match dispatch); the handle form
        // keeps %State columns and the call ABI uniform. See wrap_union_value.
        if matches!(ty, Type::Union(_)) {
            return "i64".to_string();
        }
        // 2026-08-22 (Phase 5): `dyn Trait` execution needs the thunk-table
        // ABI (fat pointer {data, table} + per-(trait, concrete) tables).
        // Until Phase 5b lands, reject explicitly — never emit silently-
        // wrong dispatch. Parse/typecheck/coercion are complete.
        if matches!(ty, Type::Dyn(_)) {
            panic!(
                "`dyn Trait` values cannot be compiled yet — the thunk-table ABI \
                 is not landed. Track Phase 5b in BUGS.md; the interpreter and \
                 typechecker accept dyn programs."
            );
        }
        // 2026-08-13 (Phase 6): `Bits<N>` (both AST forms) is exactly N bits —
        // resolve the Applied("Bits", [Number(N)]) alias here so a `Bits<32>`
        // struct field reads/writes i32, not the generic i64. The casting
        // graph's generic application path widened it.
        if let Some(n) = crate::type_universe::bits_width(ty) {
            return format!("i{}", n);
        }
        // 2026-08-16 (Phase 3a): a fixed `T[N]` field lowers to an LLVM array
        // `[N x i64]` — `coll struct Fixed { data: Int[4] }` must be
        // `%Fixed = type { [4 x i64] }`, NOT `{ i64 }`. Field GEPs index the
        // array; the old scalar collapse made the literal path misalign reads
        // by the [len] header. State-slot push_field_type already uses
        // vector_array_llvm_type; this makes struct DECLARATIONS agree.
        if matches!(ty, Type::Vector(_, _)) {
            if let Some(arr) = self.vector_array_llvm_type(ty) {
                return arr;
            }
            if let Type::Vector(inner, _) = ty {
                return self.llvm_type(inner);
            }
        }
        // 2026-07-15: Ptr<T> always maps to LLVM opaque pointer type.
        // Must be checked BEFORE the universe lookup because Ptr<T> has no
        // primitive property set (it's a compiler construct, not from bootstrap.bv).
        if matches!(ty, Type::Ptr(_)) {
            return "ptr".to_string();
        }
        // 2026-08-03: a fn value (callback param / CallPtr# operand) is a
        // function pointer — `ptr` under opaque pointers.
        if matches!(ty, Type::Function(_, _)) {
            return "ptr".to_string();
        }
        // 2026-08-17 (tuple correctness, plan
        // 2026-08-17-hashmap-storage-tuple-correctness.md): a TUPLE VALUE is a
        // boxed i64 handle (emit_tuple's `[len, e0, e1, …]` heap block,
        // ptrtoint'd). The universe/casting-graph path resolved a tuple param
        // to the struct type `{ i64, i64 }`, but the CALL SITE passes the boxed
        // i64 handle — an ABI mismatch (`%arg0` defined `{i64,i64}` but used as
        // `inttoptr i64`). Map a Tuple to the boxed handle type, exactly like
        // obj values below.
        if matches!(ty, Type::Tuple(_)) {
            return format!("i{}", self.ctx.int_bits);
        }
        // 2026-07-30: Slice<T> always uses { ptr, i64 } (fat pointer). Must be
        // checked BEFORE the general struct_types check because Slice is also
        // registered as a struct type but should be passed by value.
        // 2026-08-01 (B4): UTF8View removed (legacy type retired).
        if let Type::Custom(name) = ty {
            if name == "Slice" {
                return "{ ptr, i64 }".to_string();
            }
        }
        // 2026-07-10: Phase 1 — check for user-defined struct types first.
        // Struct types are passed by pointer at the FFI boundary, so return
        // "ptr" (LLVM opaque pointer). The named struct type is declared in
        // `declare_struct_types()` for the foreign caller's reference.
        if let Type::Custom(name) = ty {
            // 2026-08-13 (obj value ABI): an `obj` VALUE is a boxed handle
            // (i{int_bits}), NOT a pointer — state slots, struct literals, and
            // field access all use the handle form. Checked BEFORE the
            // struct_types "ptr" (which is the StaticStruct FFI-pointer ABI).
            if self.ctx.obj_types.contains(name) {
                return format!("i{}", self.ctx.int_bits);
            }
            // 2026-08-26 (Track B): an enum value is the boxed-image handle.
            if self.ctx.enum_handle_types.contains(name) {
                return format!("i{}", self.ctx.int_bits);
            }
            if self.ctx.struct_types.contains_key(name) {
                return "ptr".to_string();
            }
        }
        // 2026-08-13: the mono form of an applied obj (`List<Int>`) is a
        // boxed handle too — `llvm_type` maps the applied base through the
        // same obj registry.
        if let Type::Applied(name, _) = ty {
            if self.ctx.obj_types.contains(name) {
                return format!("i{}", self.ctx.int_bits);
            }
            // 2026-08-26 (Track B): an enum value is the boxed-image handle.
            if self.ctx.enum_handle_types.contains(name) {
                return format!("i{}", self.ctx.int_bits);
            }
        }
        // 2026-08-15 (coll plan §3.5): SVO List multi-slot struct type REMOVED
        // — feature_svo was never enabled in production.
        // (fall through to the normal llvm_type path)
        // 2026-07-22: Strings use ptr (opaque pointer) — a Briev String value
        // is a ptr to a length-prefixed [len][bytes] buffer (B0 bits model).
        // 2026-07-31: Phase 3 (§8.4-D7) — #String/#Blob membership instead
        // of the type-name match.
        // 2026-08-01 (B4): the SSO `{ i64, i64 }` branches were retired — a
        // String is never a fat pointer under the bits model.
        if self.is_protocol_member(ty, "String") || self.is_protocol_member(ty, "Blob") {
            return "ptr".to_string();
        }
        // 2026-07-30: Struct-like types derive LLVM type from field shapes.
        // This handles Slice<T> (fields: { Ptr<T>, Int } → { ptr, i64 }),
        // List<T>, and any future struct type without requiring llvm_type
        // metadata or primordial entries. Must come AFTER the SSO/SVO/String
        // special cases but BEFORE the casting graph resolution.
        if let Some(rt) = self.ctx.type_universe.as_ref()
            .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
        {
            if !rt.fields.is_empty() {
                let field_tys: Vec<String> = rt.fields.iter()
                    .map(|(_, fty)| self.llvm_type(fty))
                    .collect();
                return format!("{{ {} }}", field_tys.join(", "));
            }
        }
        // 2026-07-30: Casting graph resolves LLVM type from (protocol, metadata).
        // This replaces the old universe llvm_type property lookup.
        // Types without protocol membership fall through to fallback_llvm_type.
        if let Some(graph) = self.ctx.casting_graph.as_ref() {
            if let Some(universe) = self.ctx.type_universe.as_ref() {
                return graph.resolve_llvm_type(universe, ty, self.ctx.int_bits);
            }
        }
        Self::fallback_llvm_type(ty).to_string()
    }

    /// Is this a boxed primordial type — the set marshaled to i64 for %State
    /// storage? Bool/Char/String/Data are always boxed; the 32-bit Float is
    /// boxed via bitcast(float→i32→i64).
    ///
    /// 2026-07-31: Phase 3 (§8.4-D1) — protocol membership (is_protocol_member)
    /// replaces the box_op hardcoded type-name fallback. Float64 (#Float with
    /// bytes 8) is deliberately EXCLUDED: the legacy box_op only boxed the
    /// 32-bit Float, and Float64 params pass through as native double.
    pub(super) fn is_boxed_type(&self, ty: &Type) -> bool {
        if self.is_protocol_member(ty, "Bool")
            || self.is_protocol_member(ty, "Char")
            || self.is_protocol_member(ty, "String")
            || self.is_protocol_member(ty, "Blob")
        {
            return true;
        }
        self.is_protocol_member(ty, "Float")
            && self.ctx.type_universe.as_ref()
                .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
                .map_or(false, |rt| rt.bytes <= 4)
    }

    /// Is this a boxed NON-float type (Bool/Char/String/Data)? These become
    /// `Type::int()` in let_binding_types because their %State slot is i64.
    /// Float is NOT included — float stays `Type::float()` and is handled by
    /// ensure_float_reg via the reg_float_cache.
    ///
    /// 2026-07-31: Phase 3 (§8.4-D1) — protocol membership instead of the
    /// hardcoded name set.
    pub(super) fn is_boxed_int_type(&self, ty: &Type) -> bool {
        self.is_protocol_member(ty, "Bool")
            || self.is_protocol_member(ty, "Char")
            || self.is_protocol_member(ty, "String")
            || self.is_protocol_member(ty, "Blob")
    }

    /// Box a value of a boxed type to its i64 representation.
    ///
    /// 2026-07-31: Phase 3 (§8.4-D1) — the per-protocol conversion replaces
    /// the box_op name match: Bool → zext i8, Char → zext i32, String/Data →
    /// ptrtoint, Float → bitcast(float→i32) + zext. `float_tmp` is a fresh
    /// register name for the Float bitcast.
    pub(super) fn emit_box_value_to_i64(
        &mut self,
        out: &mut String,
        indent: &str,
        ty: &Type,
        raw: &str,
        conv: &str,
        float_tmp: &str,
    ) {
        if self.is_protocol_member(ty, "Bool") {
            writeln!(out, "{}{} = zext i8 {} to i64", indent, conv, raw).ok();
        } else if self.is_protocol_member(ty, "Char") {
            writeln!(out, "{}{} = zext i32 {} to i64", indent, conv, raw).ok();
        } else if self.is_protocol_member(ty, "String") || self.is_protocol_member(ty, "Blob") {
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, conv, raw).ok();
        } else if self.is_protocol_member(ty, "Float") {
            writeln!(out, "{}{} = bitcast float {} to i32", indent, float_tmp, raw).ok();
            writeln!(out, "{}{} = zext i32 {} to i64", indent, conv, float_tmp).ok();
        }
    }


    pub(super) fn ensure_float_reg(&mut self, out: &mut String, indent: &str, reg: &TypedRegister) -> String {
        // Check cache first — even float-typed registers may have their
        // native float counterpart cached (e.g. parameter marshaling boxes
        // float to i64 at function entry; the cache maps boxed→native).
        if let Some(cached) = self.fun.reg_float_cache.get(&reg.name) {
            return cached.clone();
        }
        // 2026-07-17: If the register is Float (32-bit) but the caller expects
        // double (e.g. Print# passes to printf which variadic-promotes float),
        // emit an fpext to double. All briev floats are represented as float
        // (32-bit), but C variadic functions receive double (64-bit).
        if reg.ty == Type::float() {
            let dbl = self.fun.gen_reg();
            writeln!(out, "{}{} = fpext float {} to double", indent, dbl, reg.name).ok();
            self.fun.reg_float_cache.insert(reg.name.clone(), dbl.clone());
            return dbl;
        }
        // 2026-07-17: For float64 (double) and non-float types, return as-is.
        // The old code used a hardcoded is_native = true path that always
        // returned reg.name directly, bypassing native_float_or_box entirely.
        reg.name.clone()
    }

    /// Emit epoll-based initialization for built-in trigger sources.
    /// Creates an epoll fd, registers each built-in trigger's source fd,
    /// and stores the epfd in a synthetic state field.
    pub(super) fn emit_trg_init(&mut self, out: &mut String) {
        // Need at least one built-in trigger to emit setup
        let has_builtin = self.ctx.triggers.iter().any(|(_, trg)| matches!(
            trg.address,
            crate::ast::LinkRef::Stdin | crate::ast::LinkRef::Timer(_) | crate::ast::LinkRef::Signal(_)
        ));
        if !has_builtin { return; }

        // Constants
        let epoin = 0x01u32; // EPOLLIN
        let epolet = 0x80000000u32; // EPOLLET
        let epoloneshot = 0x40000000u32; // EPOLLONESHOT
        let f_setfl = 4; // F_SETFL
        let o_nonblock = 0x800u32; // O_NONBLOCK
        let clo_monotonic = 1; // CLOCK_MONOTONIC
        let tfd_nonblock = 0x800; // TFD_NONBLOCK = O_NONBLOCK = 2048 on x86_64
        let sfd_nonblock = 0x800; // SFD_NONBLOCK = O_NONBLOCK = 2048 on x86_64
        let sig_block = 0; // SIG_BLOCK

        // epoll_create1(0)
        let epfd = format!("%epfd");
        writeln!(out, "  {} = call i32 @epoll_create1(i32 0)", epfd).ok();

        // Store epfd in epfd_field slot
        let sge = format!("%sge{}", self.fun.txn_counter); self.fun.txn_counter += 1;
        if let Some(epfd_idx) = self.ctx.field_index_map.get("__trg_epfd") {
            // 2026-07-20: Intentionally hand-rolled — stores i32 (not i64); centralized
            // emit_state_store_i64_by_idx only handles i64 stores.
            writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", sge, epfd_idx).ok();
            writeln!(out, "  store i32 {}, ptr {}, align 4", epfd, sge).ok();
        }

        // Per-trigger setup
        for (name, trg) in &self.ctx.triggers {
            let bit = self.ctx.dep_graph.bit_index.get(name).copied().unwrap_or(0);
            match &trg.address {
                crate::ast::LinkRef::Stdin => {
                    // fcntl(0, F_SETFL, O_NONBLOCK)
                    writeln!(out, "  %fcntl_{} = call i32 @fcntl(i32 0, i32 {}, i32 {})", name, f_setfl, o_nonblock).ok();
                    // epoll_event struct on stack: { events: EPOLLIN, data: { u64: bit } }
                    let ev_slot = format!("%ev_{}", name);
                    writeln!(out, "  {} = alloca i8, i64 16, align 8", ev_slot).ok();
                    let ev_events = format!("%eve_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 0", ev_events, ev_slot).ok();
                    let ev_events_i32 = format!("%evei_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_events_i32, ev_events).ok();
                    writeln!(out, "  store i32 {}, ptr {}, align 4", epoin, ev_events_i32).ok();
                    let ev_data = format!("%evd_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 8", ev_data, ev_slot).ok();
                    let ev_data_u64 = format!("%evdu_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_data_u64, ev_data).ok();
                    writeln!(out, "  store i64 {}, ptr {}, align 8", bit, ev_data_u64).ok();
                    // epoll_ctl(epfd, EPOLL_CTL_ADD, 0, &ev)
                    let ctl = format!("%ectl_{}", name);
                    writeln!(out, "  {} = call i32 @epoll_ctl(i32 {}, i32 1, i32 0, ptr {})", ctl, epfd, ev_slot).ok();
                }
                crate::ast::LinkRef::Timer(hz) => {
                    // timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK)
                    let tfd = format!("%tfd_{}", name);
                    writeln!(out, "  {} = call i32 @timerfd_create(i32 {}, i32 {})", tfd, clo_monotonic, tfd_nonblock).ok();
                    // timerfd_settime(tfd, 0, &its, null) — fires at Hz
                    let its_slot = format!("%its_{}", name);
                    writeln!(out, "  {} = alloca i8, i64 32, align 8", its_slot).ok();
                    // itimerspec.it_interval.tv_sec = 0
                    // itimerspec.it_interval.tv_nsec = 1_000_000_000 / hz
                    let interval_nsec = if *hz > 0 { 1_000_000_000u64 / *hz as u64 } else { 0 };
                    // itimerspec.it_value.tv_sec = 0
                    // itimerspec.it_value.tv_nsec = interval_nsec (fire immediately, then at interval)
                    let its_val_sec = format!("%its_vs_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 0", its_val_sec, its_slot).ok();
                    let its_val_sec_i64 = format!("%itsvsi_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", its_val_sec_i64, its_val_sec).ok();
                    writeln!(out, "  store i64 0, ptr {}, align 8", its_val_sec_i64).ok();
                    let its_val_nsec = format!("%its_vn_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 8", its_val_nsec, its_slot).ok();
                    let its_val_nsec_i64 = format!("%itsvni_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", its_val_nsec_i64, its_val_nsec).ok();
                    writeln!(out, "  store i64 {}, ptr {}, align 8", interval_nsec, its_val_nsec_i64).ok();
                    let its_int_sec = format!("%its_is_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 16", its_int_sec, its_slot).ok();
                    let its_int_sec_i64 = format!("%itsisi_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", its_int_sec_i64, its_int_sec).ok();
                    writeln!(out, "  store i64 0, ptr {}, align 8", its_int_sec_i64).ok();
                    let its_int_nsec = format!("%its_in_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 24", its_int_nsec, its_slot).ok();
                    let its_int_nsec_i64 = format!("%itsini_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", its_int_nsec_i64, its_int_nsec).ok();
                    writeln!(out, "  store i64 {}, ptr {}, align 8", interval_nsec, its_int_nsec_i64).ok();
                    writeln!(out, "  %tfd_settime_{} = call i32 @timerfd_settime(i32 {}, i32 0, ptr {}, ptr null)", name, tfd, its_slot).ok();
                    // epoll_ctl(epfd, EPOLL_CTL_ADD, tfd, &ev)
                    let ev_slot = format!("%ev_{}", name);
                    writeln!(out, "  {} = alloca i8, i64 16, align 8", ev_slot).ok();
                    let ev_events = format!("%eve_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 0", ev_events, ev_slot).ok();
                    let ev_events_i32 = format!("%evei_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_events_i32, ev_events).ok();
                    writeln!(out, "  store i32 {}, ptr {}, align 4", epoin, ev_events_i32).ok();
                    let ev_data = format!("%evd_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 8", ev_data, ev_slot).ok();
                    let ev_data_u64 = format!("%evdu_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_data_u64, ev_data).ok();
                    writeln!(out, "  store i64 {}, ptr {}, align 8", bit, ev_data_u64).ok();
                    writeln!(out, "  %ectl_{} = call i32 @epoll_ctl(i32 {}, i32 1, i32 {}, ptr {})", name, epfd, tfd, ev_slot).ok();
                }
                crate::ast::LinkRef::Signal(sig) => {
                    // sigemptyset(&mask)
                    let mask_slot = format!("%mask_{}", name);
                    writeln!(out, "  {} = alloca i8, i64 128, align 8", mask_slot).ok();
                    writeln!(out, "  %sigemptyset_{} = call i32 @sigemptyset(ptr {})", name, mask_slot).ok();
                    // sigaddset(&mask, SIG)
                    let sig_num = sig_number(sig);
                    writeln!(out, "  %sigadd_{} = call i32 @sigaddset(ptr {}, i32 {})", name, mask_slot, sig_num).ok();
                    // sigprocmask(SIG_BLOCK, &mask, null)
                    writeln!(out, "  %sigprocmask_{} = call i32 @sigprocmask(i32 {}, ptr {}, ptr null)", name, sig_block, mask_slot).ok();
                    // signalfd(-1, &mask, SFD_NONBLOCK)
                    let sfd = format!("%sfd_{}", name);
                    writeln!(out, "  {} = call i32 @signalfd(i32 -1, ptr {}, i32 {})", sfd, mask_slot, sfd_nonblock).ok();
                    // epoll_ctl(epfd, EPOLL_CTL_ADD, sfd, &ev)
                    let ev_slot = format!("%ev_{}", name);
                    writeln!(out, "  {} = alloca i8, i64 16, align 8", ev_slot).ok();
                    let ev_events = format!("%eve_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 0", ev_events, ev_slot).ok();
                    let ev_events_i32 = format!("%evei_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_events_i32, ev_events).ok();
                    writeln!(out, "  store i32 {}, ptr {}, align 4", epoin, ev_events_i32).ok();
                    let ev_data = format!("%evd_{}", name);
                    writeln!(out, "  {} = getelementptr i8, ptr {}, i64 8", ev_data, ev_slot).ok();
                    let ev_data_u64 = format!("%evdu_{}", name);
                    writeln!(out, "  {} = bitcast ptr {} to ptr", ev_data_u64, ev_data).ok();
                    writeln!(out, "  store i64 {}, ptr {}, align 8", bit, ev_data_u64).ok();
                    writeln!(out, "  %ectl_{} = call i32 @epoll_ctl(i32 {}, i32 1, i32 {}, ptr {})", name, epfd, sfd, ev_slot).ok();
                }
                _ => {} // Explicit(addr), Linked(sym) — handled by emit_trg_load in step()
            }
        }
    }

    /// Load an external trigger value (MMIO address or C global).
    /// Built-in triggers (@stdin#, @timer#, @signal#) are stored to state
    /// by the event loop — load from the state field.
    pub(super) fn emit_trg_load(&mut self, out: &mut String, indent: &str, dst: &str, address: &crate::ast::LinkRef, trg_ty: &Type) {
        match address {
            crate::ast::LinkRef::Explicit(addr) => {
                let store_ty = super::trg_llvm_storage_ty(trg_ty, self.ctx.type_universe.as_ref());
                let tr_counter = self.fun.txn_counter;
                self.fun.txn_counter += 1;
                let raw = format!("%tr{}", tr_counter);
                writeln!(out, "{}{} = load volatile {}, {}* inttoptr (i64 {} to {}*), align 1", indent, raw, store_ty, store_ty, addr, store_ty).ok();
                self.emit_trg_load_finish(out, indent, dst, raw, trg_ty);
            }
            crate::ast::LinkRef::Linked(sym) => {
                let store_ty = super::trg_llvm_storage_ty(trg_ty, self.ctx.type_universe.as_ref());
                let tr_counter = self.fun.txn_counter;
                self.fun.txn_counter += 1;
                let raw = format!("%tr{}", tr_counter);
                writeln!(out, "{}{} = load volatile {}, {}* @{}", indent, raw, store_ty, store_ty, sym).ok();
                self.emit_trg_load_finish(out, indent, dst, raw, trg_ty);
            }
            // 2026-07-15: @ *ptr dynamic trigger — emit the pointer expression,
            // then load volatile from the resulting pointer value.
            // When --error-unresolved-trg is set, emit a null check before the
            // load that branches to unreachable if the pointer is null.
            crate::ast::LinkRef::Deref(ptr_expr) => {
                let store_ty = super::trg_llvm_storage_ty(trg_ty, self.ctx.type_universe.as_ref());
                let tr_counter = self.fun.txn_counter;
                self.fun.txn_counter += 1;
                let raw = format!("%tr{}", tr_counter);
                let ptr_reg = self.emit_expr(out, ptr_expr, indent);
                if self.trg_unresolved_action == crate::backend::llvm::TrgUnresolvedAction::Error {
                    let err_bb = format!("trg_err_{}", tr_counter);
                    let ok_bb = format!("trg_ok_{}", tr_counter);
                    let null_check = format!("%nullchk_{}", tr_counter);
                    writeln!(out, "{}{} = icmp eq ptr {}, null", indent, null_check, ptr_reg.name).ok();
                    writeln!(out, "  br i1 {}, label %{}, label %{}", null_check, err_bb, ok_bb).ok();
                    writeln!(out, "{}:", err_bb).ok();
                    writeln!(out, "  call void @llvm.trap()").ok();
                    writeln!(out, "  unreachable").ok();
                    writeln!(out, "{}:", ok_bb).ok();
                }
                writeln!(out, "{}{} = load volatile {}, ptr {}, align 1", indent, raw, store_ty, ptr_reg.name).ok();
                self.emit_trg_load_finish(out, indent, dst, raw, trg_ty);
            }
            _ => {
                // Stdin, Timer, Signal — event loop stores values to state.
                // Emit add i64 0, 0 as zero default; callers with field access
                // patterns will have already loaded from state via Identifier -> field_index_map path.
                writeln!(out, "{}{} = add i64 0, 0 ; built-in trigger (loaded by event loop)", indent, dst).ok();
            }
        }
    }

    /// Finish loading a trigger value after the raw load: convert to native LLVM type.
    ///
    /// 2026-07-31: Phase 3 (§8.4-D7) — protocol-membership dispatch replaces
    /// the hardcoded type-name match.
    pub(super) fn emit_trg_load_finish(&self, out: &mut String, indent: &str, dst: &str, raw: String, trg_ty: &Type) {
        if self.is_protocol_member(trg_ty, "Bool") {
            writeln!(out, "{}{} = trunc i8 {} to i1", indent, dst, raw).ok();
        } else if self.is_protocol_member(trg_ty, "Int") || self.is_protocol_member(trg_ty, "UInt") {
            writeln!(out, "{}{} = add i64 0, {}", indent, dst, raw).ok();
        } else if self.is_protocol_member(trg_ty, "Float") {
            writeln!(out, "{}{} = add float 0.0, {}", indent, dst, raw).ok();
        } else if self.is_protocol_member(trg_ty, "Char") {
            writeln!(out, "{}{} = zext i32 {} to i64", indent, dst, raw).ok();
        } else if self.is_protocol_member(trg_ty, "String") || self.is_protocol_member(trg_ty, "Blob") {
            writeln!(out, "{}{} = bitcast ptr {} to ptr", indent, dst, raw).ok();
        } else {
            writeln!(out, "{}{} = add i64 0, {}", indent, dst, raw).ok();
        }
    }

    pub(super) fn align_of(&self, ty: &str) -> u32 {
        // 2026-07-30: For standard LLVM types, use direct alignment (below).
        // Only query the universe for non-standard types (struct types, typed enums),
        // matching by BOTH LLVM type string AND type name to avoid finding wrong
        // entries (e.g., a struct with alignment 2 whose LLVM type resolves to "float").
        if !matches!(ty, "i8" | "i16" | "i32" | "i64" | "i128" | "float" | "double" | "half" | "bfloat" | "ptr") {
            if let Some(u) = &self.ctx.type_universe {
                for rt in u.types.values() {
                    let rt_ll = self.llvm_type(&Type::Custom(rt.name.clone()));
                    if rt_ll == ty && rt.alignment > 0 {
                        return rt.alignment as u32;
                    }
                }
            }
        }
        // 2026-07-29: Derive alignment from LLVM bit width for integer types.
        // Covers i8/i16/i32/i64/i128 and any future integer width — bits / 8.
        if let Some(bits) = ty.strip_prefix('i').and_then(|s| s.parse::<u32>().ok()) {
            let a = bits / 8;
            if a > 0 { return a; }
        }
        // Named floating-point, pointer, and other types.
        match ty {
            "double" | "fp128" => 8,
            "float" => 4,
            "half" | "bfloat" => 2,
            "ptr" => 8,
            _ => 8,
        }
    }

    pub(super) fn declare_state_type(&mut self, out: &mut String) {
        // Emit %CellState.<name> types for persistent cells (used by thread functions)
        // 2026-07-19: Sorted for deterministic IR.
        let mut sorted_cell_types: Vec<&String> = self.ctx.cell_state_types.keys().collect();
        sorted_cell_types.sort();
        for cell_name in &sorted_cell_types {
            let (cs_imap, cs_tys) = &self.ctx.cell_state_types[*cell_name];
            write!(out, "%CellState.{} = type {{ ", cell_name).ok();
            for (i, f) in cs_tys.iter().enumerate() {
                if i > 0 { write!(out, ", ").ok(); }
                write!(out, "{}", f).ok();
            }
            writeln!(out, " }}").ok();
        }

        if self.ctx.field_types.is_empty() {
            writeln!(out, "%State = type {{ i64 }}").ok();
            // 2026-09-06 (ISR plan): the shared state global — after the
            // %State type (globals precede nothing else here; ISR bodies
            // and main's emit_state_base alias both reference it).
            if self.ctx.state_is_global {
                writeln!(out, "@__briev_state = global %State zeroinitializer").ok();
            }
            return;
        }
        // 2026-07-04: Emit chunk struct definitions so SROA can decompose
        // each chunk into scalar registers.  Each chunk has ≤CHUNK fields
        // (15).  The monolithic %State is kept for backward compat with
        // non-routed paths (old EmitInlineSsa/b, @init_state).
        // 2026-07-31: Phase 3 (§8.2) — chunk cap from config/ir-lowering.toml.
        let chunk_size = crate::config_tuning::ir_lowering().max_fields_per_alloca;
        let total = self.ctx.field_types.len();
        let num_chunks = (total + chunk_size - 1) / chunk_size;
        for chunk in 0..num_chunks {
            let start = chunk * chunk_size;
            let end = std::cmp::min(start + chunk_size, total);
            write!(out, "%StateChunk{} = type {{ ", chunk).ok();
            for i in start..end {
                if i > start { write!(out, ", ").ok(); }
                write!(out, "{}", self.ctx.field_types[i]).ok();
            }
            writeln!(out, " }}").ok();
        }
        write!(out, "%State = type {{ ").ok();
        for (i, f) in self.ctx.field_types.iter().enumerate() {
            if i > 0 { write!(out, ", ").ok(); }
            write!(out, "{}", f).ok();
        }
        writeln!(out, " }}").ok();
        // 2026-09-06 (ISR plan): the shared state global — after the %State
        // type definition (ISR bodies run on the hardware stack, outside
        // main's frame; the global is the only state they can share).
        if self.ctx.state_is_global {
            writeln!(out, "@__briev_state = global %State zeroinitializer").ok();
        }
    }

    //
    // WHY emit_init_state as a separate function AND emit_inline_init_stores:
    //   Two callers need init logic. The main reactor loop uses the inline path
    //   (emit_inline_init_stores) so SROA can scalarize %State. But library-mode
    //   and external-C callers (via __briev_init_state) need a callable function
    //   that returns an initialized ptr — those callers don't have an alloca
    //   to inline into, so they need @init_state as a named function. Both share
    //   the same store logic; the tradeoff is SROA opportunity (inline) vs callable
    //   interface (function).
    /// 2026-07-03: Shared field initializer value emitter. Handles the
    /// match-on-init_expr dispatch for field initialization, shared by
    /// emit_init_state and emit_inline_init_stores. Arguments:
    ///   - reg_suffix: suffix for intermediate register names (e.g. "s", "b").
    ///     Use format!("%ip_{}{}", idx, suffix) to generate stable names.
    /// 2026-07-31 (A7): `let x: T = val` where T declares `op Init: init(#Lh,#Rh)`
    /// — allocate the struct, call the Init member (self-bound) with the value,
    /// and store the instance address into the field slot. Returns false when
    /// the field's type has no Init-op binding.
    fn emit_init_op_construction(
        &mut self,
        out: &mut String,
        indent: &str,
        field_name: &str,
        idx: usize,
        init_expr: Option<&Expr>,
    ) -> bool {
        let briev_ty = match self.ctx.field_briev_types.get(idx) {
            Some(Type::Custom(n)) => n.clone(),
            Some(Type::Applied(n, _)) => n.clone(),
            _ => String::new(),
        };
        // 2026-08-28 (fixed-size array init): a Vector-typed field with a
        // list initializer (`let a: String[3] = ["ab", "c", "def"]`)
        // constructs by storing the elements DIRECTLY into the `[N x T]`
        // column — an array type has no op surface (no Init member), and the
        // bare Expr::List otherwise reaches codegen and panics. String/Blob
        // element registers are already ptrs and the column element type
        // (vector_array_llvm_type) is ptr/i64 — store at the column's
        // element ABI width.
        if let (Some(crate::ast::Expr::List(elems)), Some(Type::Vector(inner, _))) =
            (init_expr, self.ctx.field_briev_types.get(idx))
        {
            let col_ty = self.ctx.field_types.get(idx).cloned().unwrap_or_else(|| "i64".to_string());
            // The column's ELEMENT ABI width (vector_array_llvm_type): Ptr
            // inners hold i64 handles; everything else is its llvm_type.
            let elem_llvm = match inner.as_ref() {
                Type::Ptr(_) | Type::PtrConst(_) => "i64".to_string(),
                t => self.llvm_type(t),
            };
            for (i, e) in elems.iter().enumerate() {
                let col_gep = self.fun.gen_reg();
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", col_gep, idx).ok();
                let elem_gep = self.fun.gen_reg();
                writeln!(out, "  {} = getelementptr {}, ptr {}, i64 0, i64 {}", elem_gep, col_ty, col_gep, i).ok();
                let val_tmp = self.fun.gen_reg();
                let val = self.emit_expr_inner(out, &val_tmp, e, indent);
                let store_val = self.ensure_typed_value(out, indent, &elem_llvm, &val.name, Some(val.ty.clone()), self.ctx.type_universe.clone().as_ref());
                writeln!(out, "  store {} {}, ptr {}", elem_llvm, store_val, elem_gep).ok();
            }
            return true;
        }
        // 2026-08-16 (hashmap redesign): a hand-written collection obj
        // (`obj HashMap<K,V>`) state field constructs through its `op Init`
        // member — verified below via operator_defs["<base>"].
        // 2026-07-31 (A8): a generic field (`let q: RingBuffer<Int> = 0`)
        // monomorphizes to the applied key.
        let field_ty_clone = self.ctx.field_briev_types.get(idx).cloned();
        let type_key = match field_ty_clone.as_ref() {
            Some(ty) => self.resolve_obj_key(ty),
            None => None,
        }.unwrap_or_else(|| briev_ty.clone());
        let defs = self.ctx.operator_defs.get(&briev_ty).cloned().unwrap_or_default();
        let init_def = match defs.iter().find(|d| d.op == "Init") {
            Some(d) => d.clone(),
            None => return false,
        };
        let fn_name = match init_def.impl_args.as_ref() {
            Some(crate::ast::PropertyValue::Identifier(s)) => s.clone(),
            Some(crate::ast::PropertyValue::List(items)) => match items.first() {
                Some(crate::ast::PropertyValue::Identifier(f)) => f.clone(),
                _ => return false,
            },
            _ => return false,
        };
        let members = self.ctx.obj_members.get(&type_key).cloned().unwrap_or_default();
        let member = members.iter().find(|m| super::emit_expr::member_briev_name(m) == fn_name).cloned();
        let Some(member) = member else { return false; };
        // 2026-08-16 (slice-6 deletion): an EMPTY literal (`let q: Q = []`)
        // constructs via `op InitEmpty` — the zero-arg construction member,
        // never a hardcoded heap-seq block (a coll obj's value must be the
        // `[data, cap, len]` tier block, and a heap-seq `[0]` block misread
        // data/cap/len garbage). `op Init` is the ONE-ELEMENT construction;
        // an empty init must route to InitEmpty BEFORE the Init path allocates
        // a `data` buffer.
        if matches!(init_expr, Some(crate::ast::Expr::List(e)) if e.is_empty()) {
            let empty_def = defs.iter().find(|d| d.op == "InitEmpty").cloned();
            let empty_member = match empty_def.as_ref().and_then(|d| d.impl_args.as_ref()) {
                Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                    .find(|m| super::emit_expr::member_briev_name(m) == s)
                    .cloned(),
                _ => None,
            };
            if let Some(empty_member) = empty_member {
                let size = self.struct_type_size(&type_key);
                let inst = self.fun.gen_reg();
                writeln!(out, "{}{} = alloca i8, i64 {}", indent, inst, size).ok();
                let addr = self.fun.gen_reg();
                let hw = format!("i{}", self.ctx.int_bits);
                writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, inst, hw).ok();
                let box_recv = crate::backend::llvm::TypedRegister {
                    name: addr.clone(),
                    ty: Type::Custom(type_key.clone()),
                };
                let out_tmp = self.fun.gen_reg();
                self.emit_init_member_body(
                    out,
                    &out_tmp,
                    super::emit_expr::MemberInvocation {
                        recv_reg: &box_recv,
                        type_name: &type_key,
                        member: &empty_member,
                        arg_regs: &[],
                        prefix: None,
                    },
                    indent,
                );
                // 2026-08-28 (List.init bug): the InitEmpty member body fills
                // the LOCAL instance block via the boxed self, but the column
                // store was skipped — the State slot kept garbage and the
                // first insert dereferenced it (segfault on the first push).
                // Store the handle like the Expr::List branch below does.
                let gep = self.fun.gen_reg();
                writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", indent, gep, idx).ok();
                let field_ty = self.ctx.field_types.get(idx).cloned().unwrap_or_else(|| "i64".to_string());
                writeln!(out, "{}store {} {}, ptr {}", indent, field_ty, addr, gep).ok();
                return true;
            }
            return false;
        }
        // Allocate the instance storage and pass its address as `self`.
        let size = self.struct_type_size(&type_key);
        let inst = self.fun.gen_reg();
        writeln!(out, "{}{} = alloca i8, i64 {}", indent, inst, size).ok();
        let addr = self.fun.gen_reg();
        // 2026-08-11 (wasm32 obj-member fix): obj instance handles are
        // i{int_bits} (i32 on wasm32), not hardcoded i64.
        let hw = format!("i{}", self.ctx.int_bits);
        writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, inst, hw).ok();
        let recv_reg = crate::backend::llvm::TypedRegister {
            name: addr.clone(),
            ty: Type::Custom(type_key.clone()),
        };
        let mut arg_regs: Vec<(String, Type)> = Vec::new();
        // 2026-08-12 (Iterable protocol, slice 2 gap 3 / type-directed
        // literals, SPEC §16.3): `let x: List<Int> = [e0, e1]` lowers through
        // the collection's OWN construction members — `op Init(e0)` then
        // `op InsertAt(e1)` — never through a hardcoded `[len][elem]` heap
        // buffer. The old path passed the WHOLE literal as init's single
        // element (storing the buffer/sentinel address into data[0]).
        if let Some(crate::ast::Expr::List(elems)) = init_expr {
            let box_recv = crate::backend::llvm::TypedRegister {
                name: addr.clone(),
                ty: Type::Custom(type_key.clone()),
            };
            if elems.is_empty() {
                // An EMPTY literal (`let x: List<Int> = []`) constructs a
                // zero-length collection via the collection's zero-arg
                // construction member (op InitEmpty) — never a hardcoded
                // empty sentinel.
                let empty_member = {
                    let empty_def = defs.iter().find(|d| d.op == "InitEmpty").cloned();
                    empty_def.and_then(|d| match d.impl_args.as_ref() {
                        Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                            .find(|m| super::emit_expr::member_briev_name(m) == s)
                            .cloned(),
                        _ => None,
                    })
                };
                if let Some(empty_member) = empty_member {
                    let out_tmp = self.fun.gen_reg();
                    self.emit_init_member_body(
                        out,
                        &out_tmp,
                        super::emit_expr::MemberInvocation {
                            recv_reg: &box_recv,
                            type_name: &type_key,
                            member: &empty_member,
                            arg_regs: &[],
                            prefix: None,
                        },
                        indent,
                    );
                }
            } else {
                let arg_tmp = self.fun.gen_reg();
                let first = self.emit_expr_inner(out, &arg_tmp, &elems[0], indent);
                let out_tmp = self.fun.gen_reg();
                self.emit_init_member_body(
                    out,
                    &out_tmp,
                    super::emit_expr::MemberInvocation {
                        recv_reg: &box_recv,
                        type_name: &type_key,
                        member: &member,
                        arg_regs: &[(first.name, first.ty)],
                        prefix: None,
                    },
                    indent,
                );
                let insert_member = {
                    let insert_def = defs.iter().find(|d| d.op == "InsertAt").cloned();
                    insert_def.and_then(|d| match d.impl_args.as_ref() {
                        Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                            .find(|m| super::emit_expr::member_briev_name(m) == s)
                            .cloned(),
                        _ => None,
                    })
                };
                if let Some(insert_member) = insert_member {
                    for e in elems.iter().skip(1) {
                        let arg_tmp = self.fun.gen_reg();
                        let ar = self.emit_expr_inner(out, &arg_tmp, e, indent);
                        let out_tmp = self.fun.gen_reg();
                        self.emit_init_member_body(
                            out,
                            &out_tmp,
                            super::emit_expr::MemberInvocation {
                                recv_reg: &box_recv,
                                type_name: &type_key,
                                member: &insert_member,
                                arg_regs: &[(ar.name, ar.ty)],
                                prefix: None,
                            },
                            indent,
                        );
                    }
                }
            }
            // Store the instance address into the field slot.
            let gep = self.fun.gen_reg();
            writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", indent, gep, idx).ok();
            let field_ty = self.ctx.field_types.get(idx).cloned().unwrap_or_else(|| "i64".to_string());
            writeln!(out, "{}store {} {}, ptr {}", indent, field_ty, addr, gep).ok();
            let _ = field_name;
            return true;
        }
        if let Some(e) = init_expr {
            let tmp = self.fun.gen_reg();
            let vr = self.emit_expr_inner(out, &tmp, e, indent);
            arg_regs.push((vr.name, vr.ty));
        }
        let out_tmp = self.fun.gen_reg();
        self.emit_init_member_body(out, &out_tmp, super::emit_expr::MemberInvocation { recv_reg: &recv_reg, type_name: &type_key, member: &member, arg_regs: &arg_regs, prefix: None }, indent);
        // Store the instance address into the field slot.
        let gep = self.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", indent, gep, idx).ok();
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, self.fun.gen_reg(), inst, 0).ok();
        // 2026-08-11 (wasm32 obj-member fix): the handle is i{int_bits} and
        // the state slot is the same width — store at the field's LLVM type,
        // not a hardcoded i64 (`store i64` into an i32 List slot is invalid).
        let field_ty = self.ctx.field_types.get(idx).cloned().unwrap_or_else(|| "i64".to_string());
        writeln!(out, "{}store {} {}, ptr {}", indent, field_ty, addr, gep).ok();
        let _ = field_name;
        true
    }

    /// 2026-09-08: wrapper around emit_member_body that sets init_context.
    /// The init path (HashMap.init's match) needs cur_block to persist for the
    /// countdown header's init_pred capture — init_context skips the restore.
    fn emit_init_member_body(
        &mut self,
        out: &mut String,
        v: &str,
        inv: super::emit_expr::MemberInvocation<'_>,
        indent: &str,
    ) -> super::TypedRegister {
        self.fun.init_context = true;
        let r = self.emit_member_body(out, v, inv, indent);
        self.fun.init_context = false;
        r
    }

    /// 2026-08-12 (Iterable protocol): construct a LOCAL collection value
    /// (`let xs: List<Int> = [2, 4, 6]` inside a body) through the collection's
    /// OWN construction members — `op Init(first)` + `op InsertAt(rest)`, or
    /// `op InitEmpty` for `[]` — and return the boxed handle. Previously a
    /// local List literal emitted the hardcoded `[len][elem]` heap-seq buffer,
    /// which the At/Count members (expecting the List STRUCT layout) read as
    /// garbage (segfault). Mirrors emit_init_op_construction for locals.
    pub(super) fn construct_local_collection(
        &mut self,
        out: &mut String,
        indent: &str,
        briev_ty: &Type,
        list_literal: &Expr,
    ) -> Option<TypedRegister> {
        use crate::ast::TopLevel;
        let (base, args) = match briev_ty {
            Type::Custom(n) => (n.clone(), Vec::new()),
            Type::Applied(n, a) => (n.clone(), a.clone()),
            _ => return None,
        };
        let members = self.ctx.obj_members.get(&base).cloned().unwrap_or_default();
        let has = |op: &str| {
            members.iter().any(|m| matches!(m, TopLevel::TypeDefOperator(d) if d.name == op))
        };
        // 2026-08-16 (Phase 3a): an INLINE-FIXED coll struct (`coll struct
        // Fixed { data: Int[4] }`) constructs the LITERAL DIRECTLY into its
        // inline `T[N]` array — no malloc-based op members, no [len] header.
        // This is the correct storage for a fixed coll: `.^Length` == N, the
        // data field IS the array, and reads (`f.data[i]`, foreach via At)
        // GEP the same layout. The old path fell to the heap-seq literal
        // (emitted a [len][elems] buffer), misaligning every read by the
        // header.
        if matches!(
            self.ctx.coll_storage.get(&base),
            Some(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed)
        ) {
            return self.construct_local_fixed_collection(out, indent, briev_ty, list_literal);
        }
        // 2026-08-16 (hashmap redesign): a collection literal constructs when
        // the type declares the CONSTRUCTION ops — `op Init`/`op InitEmpty`/
        // `op InsertAt` in operator_defs (a hand-written `obj HashMap<K,V>`),
        // OR the Tier-2 surface (`op Count`+`op At`, a `coll`/List). Previously
        // ONLY Count+At qualified, so a map literal (Init+InsertAt, no At)
        // fell to the deleted heap-seq path. Literal construction is opt-in:
        // it works exactly when the type declares the ops.
        let defs_present = self.ctx.operator_defs.get(&base).cloned().unwrap_or_default();
        let has_construct_ops = defs_present.iter().any(|d| {
            d.op == "Init" || d.op == "InitEmpty" || d.op == "InsertAt"
        });
        if !(has("Count") && has("At")) && !has_construct_ops {
            return None;
        }
        let type_key = self.resolve_obj_key(briev_ty).unwrap_or_else(|| base.clone());
        let defs = self.ctx.operator_defs.get(&base).cloned().unwrap_or_default();
        let size = self.struct_type_size(&type_key);
        let inst = self.fun.gen_reg();
        // 2026-08-13 (collection value lifetime): a LOCAL collection lives on
        // the HEAP, not the stack. A `let result: List<Int> = []` in a defn
        // returned its stack-alloca handle — the caller read a dead frame
        // (bytes()/split()/join() returned garbage). Same fix as struct
        // literals: the handle must survive the constructing function's return.
        writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, inst, size).ok();
        let hw = format!("i{}", self.ctx.int_bits);
        let addr = self.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, inst, hw).ok();
        let box_recv = TypedRegister {
            name: addr.clone(),
            ty: Type::Custom(type_key.clone()),
        };
        let elems: Vec<Expr> = match list_literal {
            Expr::List(e) => e.clone(),
            _ => return None,
        };
        if elems.is_empty() {
            // `op InitEmpty` for the empty literal.
            let empty_member = defs.iter().find(|d| d.op == "InitEmpty").cloned()
                .and_then(|d| match d.impl_args.as_ref() {
                    Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                        .find(|m| super::emit_expr::member_briev_name(m) == s).cloned(),
                    _ => None,
                });
            if let Some(member) = empty_member {
                let out_tmp = self.fun.gen_reg();
                self.emit_member_body(out, &out_tmp,
                    super::emit_expr::MemberInvocation {
                        recv_reg: &box_recv, type_name: &type_key, member: &member,
                        arg_regs: &[], prefix: None,
                    }, indent);
            }
        } else {
            let init_member = defs.iter().find(|d| d.op == "Init").cloned()
                .and_then(|d| match d.impl_args.as_ref() {
                    Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                        .find(|m| super::emit_expr::member_briev_name(m) == s).cloned(),
                    _ => None,
                });
            if let Some(member) = init_member {
                let arg_tmp = self.fun.gen_reg();
                let first = self.emit_expr_inner(out, &arg_tmp, &elems[0], indent);
                let out_tmp = self.fun.gen_reg();
                self.emit_member_body(out, &out_tmp,
                    super::emit_expr::MemberInvocation {
                        recv_reg: &box_recv, type_name: &type_key, member: &member,
                        arg_regs: &[(first.name, first.ty)], prefix: None,
                    }, indent);
            }
            let insert_member = defs.iter().find(|d| d.op == "InsertAt").cloned()
                .and_then(|d| match d.impl_args.as_ref() {
                    Some(crate::ast::PropertyValue::Identifier(s)) => members.iter()
                        .find(|m| super::emit_expr::member_briev_name(m) == s).cloned(),
                    _ => None,
                });
            if let Some(member) = insert_member {
                for e in elems.iter().skip(1) {
                    let arg_tmp = self.fun.gen_reg();
                    let ar = self.emit_expr_inner(out, &arg_tmp, e, indent);
                    let out_tmp = self.fun.gen_reg();
                    self.emit_member_body(out, &out_tmp,
                        super::emit_expr::MemberInvocation {
                            recv_reg: &box_recv, type_name: &type_key, member: &member,
                            arg_regs: &[(ar.name, ar.ty)], prefix: None,
                        }, indent);
                }
            }
        }
        let _ = args;
        Some(TypedRegister {
            name: addr,
            ty: briev_ty.clone(),
        })
    }

    /// 2026-08-16 (hashmap redesign): construct a LOCAL collection VALUE from
    /// a SEED (`let m: HashMap<K,V> = 0`) through its `op Init` member —
    /// allocate the struct, call `init(seed)` self-bound, box the handle.
    /// Mirrors `emit_init_op_construction`'s non-List path for the heap-local
    /// case. Previously a scalar-seed collection-typed local bound the RAW
    /// value (a NULL/0 handle), so any member call (insert/get/Count#)
    /// dereferenced garbage — for ANY collection obj (RingBuffer, Stack,
    /// HashMap). Returns None when the type has no `op Init` binding.
    /// 2026-08-18 (Phase C): the seed REGISTER is passed in — the caller emits
    /// the RHS once and decides whether it is a genuine scalar seed (a
    /// collection-valued RHS binds directly, never wrapped in `op Init`).
    pub(super) fn construct_local_collection_seed(
        &mut self,
        out: &mut String,
        indent: &str,
        briev_ty: &Type,
        seed: TypedRegister,
    ) -> Option<TypedRegister> {
        let (base, type_key) = match briev_ty {
            Type::Custom(n) => (n.clone(), n.clone()),
            Type::Applied(n, _) => {
                let key = self.resolve_obj_key(briev_ty).unwrap_or_else(|| n.clone());
                (n.clone(), key)
            }
            _ => return None,
        };
        let defs = self.ctx.operator_defs.get(&base).cloned().unwrap_or_default();
        let init_def = defs.iter().find(|d| d.op == "Init")?;
        let fn_name = match init_def.impl_args.as_ref() {
            Some(crate::ast::PropertyValue::Identifier(s)) => s.clone(),
            _ => return None,
        };
        let members = self.ctx.obj_members.get(&type_key).cloned().unwrap_or_default();
        let member = members.iter()
            .find(|m| super::emit_expr::member_briev_name(m) == fn_name)
            .cloned()?;
        let size = self.struct_type_size(&type_key);
        let inst = self.fun.gen_reg();
        // Heap lifetime: the handle must survive the constructing function's
        // return (same rule as construct_local_collection).
        writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, inst, size).ok();
        let hw = format!("i{}", self.ctx.int_bits);
        let addr = self.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, inst, hw).ok();
        let box_recv = TypedRegister {
            name: addr.clone(),
            ty: Type::Custom(type_key.clone()),
        };
        let out_tmp = self.fun.gen_reg();
        self.emit_member_body(out, &out_tmp, super::emit_expr::MemberInvocation {
            recv_reg: &box_recv,
            type_name: &type_key,
            member: &member,
            arg_regs: &[(seed.name, seed.ty)],
            prefix: None,
        }, indent);
        Some(TypedRegister {
            name: addr,
            ty: briev_ty.clone(),
        })
    }

    /// 2026-08-18 (BUGS.md defn-param mutation): a POOLED INSTANCE used as a
    /// VALUE (a defn argument, `term` return) has NO scalar register — its
    /// fields live in the state's pooled columns and the identifier arm emits
    /// an undefined `@name` global. Materialize a BOX: malloc the struct size,
    /// copy each pooled column field into the box at its struct offset, and
    /// return the box handle. The callee's member calls then write the box
    /// (the boxed self path) and the returned handle carries the mutations.
    /// This is the storage-class rule "locals box": a pooled instance that
    /// flows into a VALUE context boxes.
    pub(super) fn box_pooled_instance_value(
        &mut self,
        out: &mut String,
        indent: &str,
        name: &str,
    ) -> Option<TypedRegister> {
        let (prefix, row) = self.unpacked_instance_prefix(name)?;
        let base = self
            .ctx
            .obj_instance_inits
            .get(&prefix)
            .map(|(b, _)| b.clone())
            .unwrap_or_else(|| prefix.clone());
        let fields = self.ctx.struct_types.get(&base)?.clone();
        if fields.is_empty() {
            return None;
        }
        let size = self.struct_type_size(&base).max(8);
        let alloc = self.fun.gen_reg();
        writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, alloc, size).ok();
        let hw = format!("i{}", self.ctx.int_bits);
        let addr = self.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, alloc, hw).ok();
        let box_p = self.fun.gen_reg();
        writeln!(out, "{}{} = inttoptr {} {} to ptr", indent, box_p, hw, addr).ok();
        for (fname, fty) in fields {
            // Pooled columns are keyed `{base}.{member}` (shared across
            // instances of the base; the ROW differentiates them).
            let slot = format!("{}.{}", base, fname);
            let Some(&idx) = self.ctx.field_index_map.get(&slot) else { continue; };
            let (row_p, _row_ty, load_ty) = self.emit_instance_column_row(out, indent, idx, &row);
            let val = self.fun.gen_reg();
            writeln!(out, "{}{} = load {}, ptr {}", indent, val, load_ty, row_p).ok();
            let off = self.lookup_field_offset(&base, &fname);
            let gep = self.fun.gen_reg();
            writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, gep, box_p, off).ok();
            // All pooled fields are boxed to i64 words in the instance block.
            let store_val = if load_ty == "i64" {
                val.clone()
            } else {
                self.adapt_to_i64(out, indent, &TypedRegister { name: val.clone(), ty: fty.clone() })
            };
            writeln!(out, "{}store i64 {}, ptr {}", indent, store_val, gep).ok();
        }
        Some(TypedRegister { name: addr, ty: Type::Custom(base.clone()) })
    }

    /// 2026-08-16 (Phase 3a, coll struct literal construction): build a LOCAL
    /// INLINE-FIXED coll value (`let f: Fixed = [1,2,3,4]`) by storing the
    /// literal elements DIRECTLY into the inline `data: T[N]` array. Layout:
    /// malloc(struct_size) → inttoptr → GEP byte offset of the sequence member
    /// → GEP `[N x T], i64 0, i64 i` → store element i. No [len] header, no
    /// cap/len slots (SPEC §8.10: coll struct length == capacity == N).
    /// Mirrors the boxed struct-literal lifetime: a heap malloc so the handle
    /// survives the constructing function's return.
    fn construct_local_fixed_collection(
        &mut self,
        out: &mut String,
        indent: &str,
        briev_ty: &Type,
        list_literal: &Expr,
    ) -> Option<TypedRegister> {
        let (base, type_key) = match briev_ty {
            Type::Custom(n) => (n.clone(), n.clone()),
            Type::Applied(n, _) => {
                let key = self.resolve_obj_key(briev_ty).unwrap_or_else(|| n.clone());
                (n.clone(), key)
            }
            _ => return None,
        };
        let elems: Vec<&Expr> = match list_literal {
            Expr::List(e) => e.iter().collect(),
            _ => return None,
        };
        let n = self.coll_fixed_length(briev_ty) as usize;
        // 2026-08-16: an over-length literal must NEVER fall back to the
        // heap-seq path (it would misalign the inline array by the [len]
        // header and silently construct the wrong value). The typechecker
        // rejects it; codegen defends in depth with a hard error.
        if elems.len() > n {
            panic!(
                "coll struct '{}' capacity is {} elements; literal has {} — \
                 an over-length literal for a fixed coll must be a compile error",
                base,
                n,
                elems.len()
            );
        }
        let size = self.struct_type_size(&type_key);
        let inst = self.fun.gen_reg();
        writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, inst, size).ok();
        let hw = format!("i{}", self.ctx.int_bits);
        let addr = self.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to {}", indent, addr, inst, hw).ok();
        // The sequence member's byte offset within the struct (the data
        // field). For a coll struct with one array member it is offset 0, but
        // compute it so any future layout change stays correct.
        let fields = self.ctx.struct_types.get(&type_key)
            .cloned()
            .unwrap_or_default();
        let mut offset: u64 = 0;
        let mut data_ty: Option<Type> = None;
        let packed = self.ctx.packed_structs.contains(&type_key);
        for (fname, fty) in &fields {
            let _ = fname;
            if data_ty.is_none() && !packed {
                if matches!(fty, Type::Vector(_, _)) {
                    data_ty = Some(fty.clone());
                    break;
                }
                offset += crate::backend::llvm::types::type_size(fty, self.ctx.type_universe.as_ref());
            }
        }
        let data_ty = data_ty?;
        let data_gep = self.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, data_gep, inst, offset).ok();
        let agg_ty = self.vector_array_llvm_type(&data_ty)
            .unwrap_or_else(|| "i64".to_string());
        for (i, e) in elems.iter().enumerate() {
            let idx = i as i64;
            let elem_ptr = self.fun.gen_reg();
            writeln!(
                out,
                "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}",
                indent, elem_ptr, agg_ty, data_gep, idx
            )
            .ok();
            let val_tmp = self.fun.gen_reg();
            let val = self.emit_expr_inner(out, &val_tmp, e, indent);
            let store_val = self.ensure_typed_value(
                out, indent, "i64", &val.name, Some(val.ty.clone()),
                self.ctx.type_universe.clone().as_ref(),
            );
            writeln!(out, "{}store i64 {}, ptr {}", indent, store_val, elem_ptr).ok();
        }
        let _ = base;
        Some(TypedRegister {
            name: addr,
            ty: briev_ty.clone(),
        })
    }

    /// 2026-08-07 (object instance pools): run an UNPACKED obj instance's
    /// Init member against its prefixed top-level slots during init_state —
    /// `let b: Box<Int, 5> = 0` runs `init` with `self_prefix` = "b", so
    /// `data[0] = v` and `total = 1` write the `b.data`/`b.total` slots.
    fn emit_instance_init(
        &mut self,
        out: &mut String,
        indent: &str,
        init: &(String, String, Expr),
    ) {
        let (name, base_raw, init_expr) = init;
        // 2026-08-28 (Phase 3): the tuple's base is the MONO key; the pool
        // slot keys, operator_defs, obj_members, and self_prefix are all
        // keyed by the BASE — normalize once here.
        let base = Self::pool_base(base_raw);
        // 2026-08-12 (Iterable protocol, slice 2 gap 1): a StructLiteral
        // (`Counter { count: 5 }`) seeds the pooled instance's SLOTS directly —
        // no `op Init` member needed. The literal's field values are the slot
        // initial values (SPEC §16.3 type-directed construction for plain objs).
        if let crate::ast::Expr::StructLiteral { fields, .. } = init_expr {
            for (mname, value) in fields {
                let slot = format!("{}.{}", base, mname);
                let Some(&idx) = self.ctx.field_index_map.get(&slot) else { continue };
                let col_ty = self.ctx.field_types[idx].clone();
                let base_gep = self.emit_state_gep(out, indent, "si", "%state", idx);
                let gep = self.fun.gen_reg();
                writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 0, i64 0", indent, gep, col_ty, base_gep).ok();
                let val_tmp = self.fun.gen_reg();
                let val = self.emit_expr_inner(out, &val_tmp, value, indent);
                let field_ty = self.ctx.field_briev_types[idx].clone();
                let llvm_ty = if matches!(field_ty, Type::Ptr(_)) {
                    "i64".to_string()
                } else {
                    self.llvm_type(&field_ty)
                };
                let store_val = self.ensure_typed_value(
                    out, indent, &llvm_ty, &val.name, Some(val.ty.clone()),
                    self.ctx.type_universe.clone().as_ref(),
                );
                writeln!(out, "{}store {} {}, ptr {}", indent, llvm_ty, store_val, gep).ok();
            }
            return;
        }
        let defs = self.ctx.operator_defs.get(base).cloned().unwrap_or_default();
        let init_def = match defs.iter().find(|d| d.op == "Init") {
            Some(d) => d.clone(),
            None => return,
        };
        let fn_name = match init_def.impl_args.as_ref() {
            Some(crate::ast::PropertyValue::Identifier(s)) => s.clone(),
            _ => return,
        };
        let members = self.ctx.obj_members.get(base).cloned().unwrap_or_default();
        let member = members.iter()
            .find(|m| super::emit_expr::member_briev_name(m) == fn_name)
            .cloned();
        let Some(member) = member else { return; };
        let mut arg_regs: Vec<(String, Type)> = Vec::new();
        let tmp = self.fun.gen_reg();
        let vr = self.emit_expr_inner(out, &tmp, init_expr, indent);
        arg_regs.push((vr.name, vr.ty));
        let out_tmp = self.fun.gen_reg();
        let recv_reg = crate::backend::llvm::TypedRegister {
            name: "0".to_string(),
            ty: Type::Custom(base.to_string()),
        };
        self.emit_member_body(out, &out_tmp, super::emit_expr::MemberInvocation { recv_reg: &recv_reg, type_name: base, member: &member, arg_regs: &arg_regs, prefix: Some((base.to_string(), "0".to_string())) }, indent);
    }

    /// 2026-08-07 (object instance pools): allocate the DEPENDENT pools'
    /// heap buffers at program init. Each dependent base's columns are
    /// malloc'd to `(rows) * elem_size`, where `rows = static_rows +
    /// Σ multiplier×bound_expr` (SPEC §16.6 dependent bounds), the sum of all
    /// proven runtime bounds. Runs AFTER the field-init stores (so bound
    /// fields like `N` are readable) and BEFORE the static instance inits
    /// (row 0 lives at the buffer start). The pool is predictably
    /// inexhaustible: the buffer holds the sum of all runtime bounds, and the
    /// bound expressions are the countdown loop limits — the pool can never
    /// be exhausted during the program.
    fn emit_dependent_pool_buffers(&mut self, out: &mut String, indent: &str) {
        // Deterministic IR: iterate the dependent bases in sorted order.
        let mut bases: Vec<String> = self.ctx.dependent_pools.keys().cloned().collect();
        bases.sort();
        for base in &bases {
            // Owned copies (self is mutably borrowed below).
            let terms: Vec<crate::analysis::spawn_pool::DependentTerm> =
                self.ctx.dependent_pools.get(base).cloned().unwrap_or_default();
            let static_rows = self.ctx.spawn_pools.get(base).copied().unwrap_or(0) as i64;
            // rows = static_rows + Σ (multiplier × bound_expr) + 1. The bound
            // expressions read state fields that were initialized earlier in
            // this same function. The +1 is row 0 (the static instance): the
            // __spawn_next_<base> counter starts at 1, so the last spawned row
            // is index `total` — the buffer must hold total + 1 elements
            // (2026-08-08 Bug 3; without it the final row wrote OOB, hidden
            // only by malloc slop in spr.bv).
            let mut acc_reg: Option<String> = None;
            for term in terms {
                let vr = self.emit_expr(out, &term.bound, indent);
                let scaled = if term.multiplier != 1 {
                    let r = self.fun.gen_reg();
                    writeln!(out, "{}{} = mul i64 {}, {}", indent, r, vr.name, term.multiplier).ok();
                    r
                } else {
                    vr.name
                };
                let lhs = match &acc_reg {
                    Some(acc) => acc.clone(),
                    None => format!("{}", static_rows),
                };
                let r = self.fun.gen_reg();
                writeln!(out, "{}{} = add i64 {}, {}", indent, r, lhs, scaled).ok();
                acc_reg = Some(r);
            }
            let rows_base = acc_reg.unwrap_or_else(|| format!("{}", static_rows));
            let rows_reg = self.fun.gen_reg();
            writeln!(out, "{}{} = add i64 {}, 1", indent, rows_reg, rows_base).ok();
            // Allocate a heap buffer per member column of this base.
            let mut columns: Vec<(usize, String)> = self.ctx.heap_columns.iter()
                .filter(|(idx, _)| {
                    let slot_name = self.ctx.field_index_map.iter()
                        .find(|(_, v)| **v == **idx)
                        .map(|(k, _)| k.clone())
                        .unwrap_or_default();
                    slot_name.starts_with(&format!("{}.", base))
                })
                .map(|(&idx, elem)| (idx, elem.clone()))
                .collect();
            columns.sort_by_key(|&(idx, _)| idx);
            for (idx, elem) in &columns {
                // bytes = rows * elem_size; store the malloc'd address in the
                // column slot (member access loads it and GEPs the row).
                let byte_size = llvm_type_byte_size(elem);
                let size_reg = if byte_size != 1 {
                    let r = self.fun.gen_reg();
                    writeln!(out, "{}{} = mul i64 {}, {}", indent, r, rows_reg, byte_size).ok();
                    r
                } else {
                    rows_reg.clone()
                };
                let raw = self.fun.gen_reg();
                writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, raw, size_reg).ok();
                let addr = self.fun.gen_reg();
                writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, addr, raw).ok();
                let slot = self.emit_state_gep(out, indent, "dp", "%state", *idx);
                writeln!(out, "{}store i64 {}, ptr {}", indent, addr, slot).ok();
            }
        }
    }
    pub(crate) fn emit_spawn_init(
        &mut self,
        out: &mut String,
        indent: &str,
        base: &str,
        args: &[Expr],
        row_reg: &str,
    ) {
        // 2026-08-26 (async Phase D): port WIRING runs FIRST and INDEPENDENT
        // of any `op Init` — a ports-only obj (pure event bus) has no Init
        // member yet must still allocate/wire its event-slot columns.
        let (ports_in, ports_out) = self
            .ctx
            .obj_port_wiring
            .get(base)
            .cloned()
            .unwrap_or_default();
        if self.ctx.obj_ports_enabled {
            // 2026-08-26 (async Phase D): pool columns are `[capacity x T]`
            // ROWED arrays — wiring MUST go through emit_instance_column_row so
            // the event-slot id lands in THIS spawn's row (row_reg), not
            // element 0 of the column.
            for pname in &ports_out {
                let id = self.fun.gen_reg();
                let events_i = self.fun.gen_reg();
                writeln!(out, "{indent}{events_i} = ptrtoint ptr @__briev_events to i64").ok();
                writeln!(out, "{indent}{id} = call i64 @briev_event_alloc_impl(ptr %state, i64 {events_i})").ok();
                let slot_idx = match self.ctx.field_index_map.get(&format!("{base}.{pname}")) {
                    Some(&i) => i,
                    None => continue,
                };
                let (row_ptr, _, _) =
                    self.emit_instance_column_row(out, indent, slot_idx, row_reg);
                writeln!(out, "{indent}store i64 {id}, ptr {row_ptr}").ok();
            }
            for (i, pname) in ports_in.iter().enumerate() {
                if let Some(a) = args.get(i) {
                    let tmp = self.fun.gen_reg();
                    let vr = self.emit_expr_inner(out, &tmp, a, indent);
                    let val = self.adapt_to_i64(out, indent, &vr);
                    let slot_idx = match self.ctx.field_index_map.get(&format!("{base}.{pname}")) {
                        Some(&ix) => ix,
                        None => continue,
                    };
                    let (row_ptr, _, _) =
                        self.emit_instance_column_row(out, indent, slot_idx, row_reg);
                    writeln!(out, "{indent}store i64 {val}, ptr {row_ptr}").ok();
                }
            }
        }
        let defs = self.ctx.operator_defs.get(base).cloned().unwrap_or_default();
        let init_def = match defs.iter().find(|d| d.op == "Init") {
            Some(d) => d.clone(),
            None => return,
        };
        let fn_name = match init_def.impl_args.as_ref() {
            Some(crate::ast::PropertyValue::Identifier(s)) => s.clone(),
            _ => return,
        };
        let members = self.ctx.obj_members.get(base).cloned().unwrap_or_default();
        let member = members.iter()
            .find(|m| super::emit_expr::member_briev_name(m) == fn_name)
            .cloned();
        let Some(member) = member else { return; };
        let mut arg_regs: Vec<(String, Type)> = Vec::new();
        for a in args {
            let tmp = self.fun.gen_reg();
            let vr = self.emit_expr_inner(out, &tmp, a, indent);
            arg_regs.push((vr.name, vr.ty));
        }
        let out_tmp = self.fun.gen_reg();
        let recv_reg = crate::backend::llvm::TypedRegister {
            name: "0".to_string(),
            ty: Type::Custom(base.to_string()),
        };
        self.emit_member_body(out, &out_tmp, super::emit_expr::MemberInvocation { recv_reg: &recv_reg, type_name: base, member: &member, arg_regs: &arg_regs, prefix: Some((base.to_string(), row_reg.to_string())) }, indent);
    }
    /// 2026-08-09 (Phase 5): emit a `box`/`spill` spawn — the instance is its
    /// own heap allocation (NOT a pooled column row). A per-instance block
    /// sized to the base's member columns is malloc'd, the Init member runs
    /// against it (row 0 of the block), and the block address is the linear
    /// handle. Member access inttoptrs the handle + GEPs the member offset
    /// (emit_instance_column_row's boxed branch). Box and spill share this
    /// emission — the classification difference (spill may grow) is recorded
    /// in the analysis and drives future growable-buffer re-layout.
    pub(crate) fn emit_spawn_storage(
        &mut self,
        out: &mut String,
        indent: &str,
        base: &str,
        args: &[Expr],
    ) -> crate::backend::llvm::TypedRegister {
        let mut members: Vec<(String, crate::ast::Type)> = Vec::new();
        if let Some(fields) = self.ctx.struct_types.get(base).cloned() {
            members = fields;
        }
        let mut total: u64 = 0;
        for (_mname, mty) in &members {
            total += crate::backend::llvm::types::type_size(mty, self.ctx.type_universe.as_ref());
        }
        let total = total.max(8);
        let raw = self.fun.gen_reg();
        writeln!(out, "{}{} = call ptr @malloc(i64 {})", indent, raw, total).ok();
        let addr = self.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, addr, raw).ok();
        // Run the Init member at the block's row 0.
        self.emit_spawn_init(out, indent, base, args, &addr);
        crate::backend::llvm::TypedRegister {
            name: addr,
            ty: Type::Custom(base.to_string()),
        }
    }

    /// 2026-07-31 (A8): monomorphize an applied obj type (`Stack<Int, 8>`):
    /// register the substituted slots + member bodies under a mono key
    /// (`Stack<Int, 8>`), so layout, self-slot offsets, and member dispatch
    /// use the concrete instance. Returns the mono key.
    pub(crate) fn ensure_mono(&mut self, base: &str, args: &[crate::ast::Type]) -> String {
        let key = format!(
            "{}<{}>",
            base,
            args.iter().map(|t| format!("{}", t)).collect::<Vec<_>>().join(", ")
        );
        if !self.ctx.struct_types.contains_key(&key) {
            let subst = self.mono_subst(base, args);
            let slots = self.ctx.struct_types.get(base).cloned().unwrap_or_default()
                .into_iter()
                .map(|(n, ty)| (n, crate::typechecker::substitute_type(&ty, &subst)))
                .collect::<Vec<_>>();
            self.ctx.struct_types.insert(key.clone(), slots.clone());
            if let Some(members) = self.ctx.obj_members.get(base).cloned() {
                self.ctx.obj_members.insert(key.clone(), members.clone());
                // 2026-08-16 (Phase 3b): a GENERIC `coll struct` re-synthesizes
                // its op surface against the SUBSTITUTED slots — the generic
                // base's `data: T[N]` has an unresolved `Named("N",0)` dim, so
                // its scaffolded Count read a `len` slot (wrong for a fixed
                // coll: `%len` undefined). The mono key's `Int[4]` gives a
                // constant-N Count. Only rebuild the synthesized members that
                // depend on the resolved dim; keep user-declared members.
                if self.ctx.coll_storage.contains_key(base) {
                    let ftd = crate::ast::top::TypeDef {
                        name: base.to_string(), type_params: vec![],
                        parent: None, protocol: None, traits: vec![],
                        bit_range: None, span: None, coll: true, seq: false,
                        ports_in: Vec::new(),
                        ports_out: Vec::new(),
                        body: crate::ast::top::TypeDefBody {
                            slots: slots.iter()
                                .map(|(n, ty)| crate::ast::top::TypeDefSlot {
                                    name: n.clone(), ty: ty.clone(), bit_range: None,
                                })
                                .collect(),
                            metadata: Default::default(), projections: vec![],
                            bindings: vec![], operators: vec![],
                            op_bindings: vec![], constraints: vec![],
                            members: vec![], span: None,
                        },
                    };
                    if let Some((seq_expr, elem_ty)) = crate::backend::llvm::coll_scaffold::derive_sequence_member(
                        &ftd.body.slots,
                        &self.ctx.struct_types,
                    ) {
                        let storage = self.ctx.coll_storage.get(base)
                            .copied()
                            .unwrap_or(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed);
                        let synth = crate::backend::llvm::coll_scaffold::synthesize_members(
                            &ftd, &seq_expr, elem_ty, storage,
                        );
                        // 2026-08-16 (Phase 3b): a `coll struct` has NO user
                        // members (StaticStruct body) — every member is
                        // scaffolded, so the mono set REPLACES the base copy
                        // (whose generic Count read a nonexistent `len` slot).
                        // A `coll obj` keeps its user members + merged synth.
                        let is_coll_struct = matches!(
                            self.ctx.coll_storage.get(base),
                            Some(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed)
                        );
                        let mut members = members;
                        if is_coll_struct {
                            members = synth;
                        } else {
                            for m in synth {
                                let m_name = crate::backend::llvm::emit_expr::member_briev_name(&m);
                                let dup = members.iter().any(|ex| {
                                    crate::backend::llvm::emit_expr::member_briev_name(ex) == m_name
                                });
                                if !dup {
                                    members.push(m);
                                }
                            }
                        }
                        self.ctx.obj_members.insert(key.clone(), members);
                    }
                }
            }
        }
        key
    }

    /// 2026-07-31 (A8): the substitution map for an applied obj type —
    /// declared type params zipped with the concrete args.
    fn mono_subst(&self, base: &str, args: &[crate::ast::Type]) -> std::collections::HashMap<String, crate::ast::Type> {
        let params = self.ctx.obj_type_params.get(base).cloned().unwrap_or_default();
        params.into_iter().zip(args.iter().cloned()).collect()
    }

    /// 2026-07-31 (A8): resolve an obj type name (Custom or Applied) to its
    /// registered struct key, monomorphizing on first use.
    /// 2026-08-07 (object instance pools): `Some(name)` when `name` is an
    /// unpacked obj instance (its Init was recorded) — its member bodies
    /// resolve bare member names against the prefixed top-level slots.
    /// 2026-08-28: extract the base obj name from a mono key.
    /// `"Stack<Int,8>"` → `"Stack"`, `"Stack"` → `"Stack"`.
    pub(crate) fn pool_base(mono_or_base: &str) -> &str {
        mono_or_base.split('<').next().unwrap_or(mono_or_base)
    }

    pub(crate) fn unpacked_instance_prefix(&self, name: &str) -> Option<(String, String)> {
        // 2026-08-28 (Phase 3): obj_instance_inits stores the MONO key
        // ("Stack<Int,8>") for struct lookups, but the PREFIX is the POOL
        // name — `{base}.{member}` slot keys, spawn_pools, and obj_members
        // are all keyed by the BASE. Normalize ONCE here (every prefix
        // consumer flows through this function); struct-lookup sites that
        // need the mono key read obj_instance_inits directly.
        self.ctx.obj_instance_inits.get(name)
            .map(|(base, _)| (Self::pool_base(base).to_string(), "0".to_string()))
    }

    /// 2026-08-07 (instance pools): does a loop body contain an OBSERVABLE
    /// call (an observable intrinsic like Print#, an FFI, or an `out`-marked
    /// name)? Such loops must not be folded/unrolled by LLVM — the observable
    /// is a liveness root, and the countdown loop emitter attaches
    /// `!llvm.loop.disable_nonforced` for them.
    pub(super) fn loop_has_observable(&self, body: &[Statement]) -> bool {
        let observable = &self.ctx.observable_names;
        body.iter().any(|s| Self::has_observable_stmt(s, observable))
    }

    fn has_observable_stmt(stmt: &Statement, observable: &std::collections::HashSet<String>) -> bool {
        match stmt {
            Statement::Let { expr: Some(e), .. }
            | Statement::Expression(e)
            | Statement::Term(Some(e))
            | Statement::EndProgram(Some(e))
            | Statement::Gate(e) => Self::has_observable_expr(e, observable),
            Statement::Assign(_, e) => Self::has_observable_expr(e, observable),
            Statement::Guarded(_, body) | Statement::Block(body) => {
                body.iter().any(|s| Self::has_observable_stmt(s, observable))
            }
            Statement::Foreach { list, body, .. } => {
                Self::has_observable_expr(list, observable)
                    || body.iter().any(|s| Self::has_observable_stmt(s, observable))
            }
            _ => false,
        }
    }

    fn has_observable_expr(expr: &Expr, observable: &std::collections::HashSet<String>) -> bool {
        match expr {
            Expr::Call(name, args, _) => {
                !name.ends_with('#')
                    || crate::intrinsic_signatures::get_intrinsic_signature(name)
                        .map_or(false, |sig| sig.observable)
                    || observable.contains(name)
                    || args.iter().any(|a| Self::has_observable_expr(a, observable))
            }
            Expr::MethodCall(recv, _, args, _) => {
                Self::has_observable_expr(recv, observable)
                    || args.iter().any(|a| Self::has_observable_expr(a, observable))
            }
            Expr::BinaryOp(_, l, r) => {
                Self::has_observable_expr(l, observable) || Self::has_observable_expr(r, observable)
            }
            Expr::Index(o, _) | Expr::Field(o, _) | Expr::Cast(o, _) | Expr::IsType(o, _)
            | Expr::Consume(o) | Expr::Deref(o) | Expr::AddrOf(o) => {
                Self::has_observable_expr(o, observable)
            }
            Expr::List(args) | Expr::Tuple(args) => {
                args.iter().any(|a| Self::has_observable_expr(a, observable))
            }
            Expr::If(c, t, e) => {
                Self::has_observable_expr(c, observable)
                    || Self::has_observable_expr(t, observable)
                    || e.as_ref().map_or(false, |e| Self::has_observable_expr(e, observable))
            }
            Expr::Match(s, arms) => {
                Self::has_observable_expr(s, observable)
                    || arms.iter().any(|a| Self::has_observable_expr(&a.body, observable))
            }
            _ => false,
        }
    }

    pub(crate) fn resolve_obj_key(&mut self, ty: &crate::ast::Type) -> Option<String> {
        match ty {
            crate::ast::Type::Custom(n) if self.ctx.struct_types.contains_key(n) => Some(n.clone()),
            crate::ast::Type::Applied(n, args) if self.ctx.struct_types.contains_key(n) => {
                let n = n.clone();
                let args = args.clone();
                Some(self.ensure_mono(&n, &args))
            }
            _ => None,
        }
    }

    /// 2026-08-01 (B4): tag_strings removed — a String is an untagged ptr to
    /// [len][bytes] under the bits model; no static-tag bit is ever set.
    /// Emit an init store for a FLOAT literal at the field slot's width —
    /// half (f16 RNE hex), float (native), double (native), or the legacy
    /// boxed-i64 dance for anything else. 2026-09-02 (plan
    /// fundamental-parent-membership): extracted from emit_field_init_value
    /// — the Neg arm duplicated this whole chain; the Float16 half arm made
    /// a third copy unjustifiable (rule 17). Undo: inline back.
    fn emit_float_literal_store(&mut self, out: &mut String, indent: &str,
        f: f64, slot: (&str, &str))
    {
        let (ty, gep) = slot;
        if ty == "half" {
            writeln!(out, "{}store half {}, ptr {}, align 2", indent, f32_to_f16_hex(f), gep).ok();
            return;
        }
        if ty == "float" {
            let bits_reg = self.fun.gen_reg();
            writeln!(out, "{}{} = bitcast i32 {} to float", indent, bits_reg, float_to_llvm_hex(f)).ok();
            writeln!(out, "{}store float {}, ptr {}, align 4", indent, bits_reg, gep).ok();
            return;
        }
        if ty == "double" {
            // 2026-08-01: a Float64 field's slot is `double` — emit the
            // literal at double width. The old else-branch boxed the
            // FLOAT32 into the double slot (4 garbage low bytes →
            // corrupted values, e.g. 3.25 → 5.33e-315).
            let bits_reg = self.fun.gen_reg();
            writeln!(out, "{}{} = bitcast i64 {} to double", indent, bits_reg, float64_to_llvm_hex(f)).ok();
            writeln!(out, "{}store double {}, ptr {}, align 8", indent, bits_reg, gep).ok();
            return;
        }
        // Boxed fallback (legacy non-native float slots).
        let bits_reg = self.fun.gen_reg();
        writeln!(out, "{}{} = bitcast i32 {} to float", indent, bits_reg, float_to_llvm_hex(f)).ok();
        let boxed = self.fun.gen_reg();
        writeln!(out, "{}{} = bitcast float {} to i32", indent, boxed, bits_reg).ok();
        let zext = self.fun.gen_reg();
        writeln!(out, "{}{} = zext i32 {} to i64", indent, zext, boxed).ok();
        writeln!(out, "{}store i64 {}, ptr {}, align {}", indent, zext, gep, self.align_of("i64")).ok();
    }

    fn emit_field_init_value(&mut self, out: &mut String, indent: &str,
        init_clone: Option<Expr>, ty: &str, gep: &str, idx: usize)
    {
        let mut field_reg = |suffix: &str| -> String { format!("%ip_{}{}", idx, suffix) };
                // 2026-06-20: Handle LiteralExpr::Float directly, matching Expr::Float arm above.
                // Without this, the catch-all boxes float to i64 and immediately unboxes it
                // back, producing dead IR. LLVM DCE would clean them, but they may cause
                // verifier errors if they cross adapt_to_i64 before DCE runs.
        match init_clone {
            Some(Expr::Decimal(n)) => {
                // 2026-08-10: store at the field's actual LLVM type. Flexible
                // Int/UInt slots are i{int_bits} (i32 on wasm32); the old
                // hardcoded `store i64` broke those slots. Exact ints and i64
                // (x86_64) are unchanged.
                writeln!(out, "{}store {} {}, ptr {}, align {}", indent, ty, n, gep, self.align_of(ty)).ok();
            }
            Some(Expr::Float(f)) => {
                self.emit_float_literal_store(out, indent, f, (ty, gep));
            }
            // 2026-07-14: Float and Float32 unified to Expr::Float(f64).
            // Negative values handled via Expr::UnaryOp(UnaryOpKind::Neg, Expr::Float(f)).
            Some(Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, ref inner)) => {
                match inner.as_ref() {
                    Expr::Float(f) => {
                        self.emit_float_literal_store(out, indent, -*f, (ty, gep));
                    }
                    Expr::Decimal(n) => {
                        writeln!(out, "{}store i64 -{}, ptr {}, align {}", indent, n, gep, self.align_of("i64")).ok();
                    }
                    _ => {
                        writeln!(out, "{}store i64 0, ptr {}, align {}", indent, gep, self.align_of("i64")).ok();
                    }
                }
            }
            Some(Expr::Bool(b)) => {
                let v = if b { "1" } else { "0" };
                writeln!(out, "{}store i8 {}, ptr {}, align {}", indent, v, gep, self.align_of("i8")).ok();
            }
            Some(Expr::Quoted(s)) => {
                // 2026-07-14: Store string constant pointer (Quoted replaces LiteralExpr::String).
                // 2026-08-01 (B4): always the UNTAGGED store — a String value is
                // an untagged ptr to [len][bytes] (bits model); the SSO static-tag
                // (OR 1) path was retired (it misaligned the header for briev_str_eq).
                let s_str = String::from_utf8_lossy(&s);
                let si = self.ctx.string_constants.iter().position(|x| x.as_str() == s_str).unwrap_or(0);
                let g = format!("@str.{}", si);
                let str_p = field_reg("s");
                writeln!(out, "{}{} = bitcast <{{ i64, [{} x i8] }}>* {} to ptr", indent, str_p, s.len() + 1, g).ok();
                writeln!(out, "{}store i8* {}, ptr {}, align {}", indent, str_p, gep, self.align_of("i8*")).ok();
            }
            Some(expr) => {
                let val_reg = self.emit_expr(out, &expr, indent);
                // 2026-07-19: Store with native type when possible. Skip
                // adapt_to_i64 boxing when the value's LLVM type matches the
                // field's LLVM type (float→"float", double→"double").
                let val_ty = self.llvm_type(&val_reg.ty);
                if val_ty == *ty {
                    writeln!(out, "{}store {} {}, ptr {}, align {}", indent, ty, val_reg.name, gep, self.align_of(ty)).ok();
                } else {
                    let boxed = self.adapt_to_i64(out, indent, &val_reg);
                    writeln!(out, "{}store i64 {}, ptr {}, align {}", indent, boxed, gep, self.align_of("i64")).ok();
                }
            }
            None => {
                if ty == "i8*" {
                    // 2026-06-29: Initialize uninitialized String fields to @str.0
                    // (empty string sentinel, untagged) instead of null.
                    // Must NOT add tag bit (OR 1) because all sentinel comparisons
                    // in trim/submit_input/handle_action check against the untagged
                    // @str.0 address. A tagged pointer would shift all struct
                    // field accesses by 1 byte, causing garbage reads and crashes.
                    let str_p = field_reg("s");
                    writeln!(out, "{}{} = bitcast <{{ i64, i64, [1 x i8] }}>* @str.0 to ptr", indent, str_p).ok();
                    writeln!(out, "{}store i8* {}, ptr {}, align {}", indent, str_p, gep, self.align_of("i8*")).ok();
                } else if ty.starts_with('[') {
                    // 2026-07-31 (A4): array state fields initialize to
                    // zeroinitializer (`store [16 x float] 0` is invalid IR).
                    writeln!(
                        out,
                        "{}store {} zeroinitializer, ptr {}, align {}",
                        indent, ty, gep, self.align_of(ty)
                    )
                    .ok();
                } else {
                    writeln!(out, "{}store {} 0, ptr {}, align {}", indent, ty, gep, self.align_of(&ty)).ok();
                }
            }
        }
    }

    pub(super) fn emit_init_state(&mut self, out: &mut String) {
        writeln!(out, "define void @init_state({}) local_unnamed_addr #0 {{", self.ctx.state_ptr_param).ok();
        writeln!(out, "  entry:").ok();
        let mut fields: Vec<(String, usize, String)> = self.ctx.field_index_map.iter()
            .map(|(name, &idx)| (name.clone(), idx, self.ctx.field_types[idx].clone()))
            .collect();
        fields.sort_by_key(|&(_, idx, _)| idx);
        for (name, idx, ty) in fields {
            // 2026-07-20: Intentionally hand-rolled — register name %ip_{idx} is referenced by
            // emit_field_init_value below which passes %ip_{idx} to ringbuf init codegen.
            let p = format!("%ip_{}", idx);
            writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", p, idx).ok();
            let init_clone = self.ctx.field_initializers.get(&name).and_then(|e| e.clone());
            // 2026-07-31 (A7): `op Init: init(#Lh, #Rh)` construction — a
            // struct-typed field initialized from a value is built by calling
            // the Init member (self-bound) instead of storing the raw value.
            if self.emit_init_op_construction(out, "  ", &name, idx, init_clone.as_ref()) {
                continue;
            }
            // 2026-07-31: Phase 3 (§8.5-E4) — the ringbuf-init detection branch
            // (always-false stub after insert_at removal) is deleted. A
            // bracket-list initializer falls through to emit_field_init_value
            // as before.
            self.emit_field_init_value(out, "  ", init_clone, &ty, &p, idx);
        }
        // 2026-08-07 (object instance pools): run the unpacked instances'
        // Init members against their prefixed slots.
        let instance_inits: Vec<(String, String, Expr)> = self.ctx.obj_instance_inits.iter()
            .map(|(n, (b, e))| (n.clone(), b.clone(), e.clone()))
            .collect();
        for init in &instance_inits {
            self.emit_instance_init(out, "  ", init);
        }
        let mmio_inits: Vec<(u64, Expr)> = {
            let mut v = Vec::new();
            for (name, &addr) in &self.ctx.mmio_fields {
                if let Some(Some(expr)) = self.ctx.mmio_initializers.get(name).cloned() {
                    v.push((addr, expr.clone()));
                }
            }
            v
        };
        for (addr, expr) in mmio_inits {
            let p = format!("%mio{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            self.emit_inttoptr(out, "  ", &p, &addr.to_string());
            let val_reg = self.emit_expr(out, &expr, "  ");
            writeln!(out, "  store volatile i64 {}, ptr {}, align 1", val_reg, p).ok();
        }
        // 2026-08-09 (init kind, Phase 2): seed runtime-seeded invariants here,
        // after state-field init stores and before any node fires.
        self.emit_init_seeding_stores(out, "  ");
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
    }

    /// Emit field initialization stores inline in the current function (no function call).
    /// Same logic as emit_init_state but stores are emitted directly, not wrapped in a
    /// separate @init_state function. This prevents the %state alloca from escaping to
    /// init_state, enabling LLVM's SROA to decompose the %State struct.
    ///
    /// WHY inline instead of a separate @init_state call:
    ///   If %state alloca is passed to an external @init_state function, LLVM must
    ///   conservatively assume the call writes to ALL of %State, making SROA analysis
    ///   impossible — the struct stays as one opaque alloca. By inlining the stores,
    ///   every field store is a direct GEP+store that SROA can scalarize and GVN can
    ///   eliminate. This is critical for embedded targets where %State must decompose
    ///   into scalar registers to eliminate the stack alloca entirely.
    ///
    /// WHY every field gets explicit initial values (including zero-initialized ones):
    ///   LLVM's SROA will not promote an alloca to SSA registers if any field is
    ///   undef or has a non-GEP use. Dead fields still need explicit initializers so
    ///   SROA can see every field's lifetime. Zero-initialized fields are store i64 0
    ///   / i8* null explicitly — without these, the field is undef and LLVM may
    ///   introduce poison which inhibits downstream optimizations like load elimination.
    pub(super) fn emit_inline_init_stores(&mut self, out: &mut String, state_ptr: &str) {
        let indent = if state_ptr == "%state" { "  " } else { "" };
        let mut fields: Vec<(String, usize, String)> = self.ctx.field_index_map.iter()
            .map(|(name, &idx)| (name.clone(), idx, self.ctx.field_types[idx].clone()))
            .collect();
        fields.sort_by_key(|&(_, idx, _)| idx);
        for (name, idx, _ty) in &fields {
            let gep_reg = self.emit_state_gep(out, indent, "ip", "%state", *idx);
            let init_clone = self.ctx.field_initializers.get(name).and_then(|e| e.clone());
            let ty = self.ctx.field_types[*idx].clone();
            // 2026-08-01 (A9b): the `op Init` construction must also run for the
            // countdown's inline init stores — otherwise a struct-typed field
            // (`let queue: RingBuffer<Int> = 0`) stores 0 instead of the
            // instance address, and the countdown's member-call body dereferences
            // a null handle. Mirrors emit_init_state's A7 dispatch.
            if self.emit_init_op_construction(out, indent, name, *idx, init_clone.as_ref()) {
                continue;
            }
            // 2026-07-31: Phase 3 (§8.5-E4) — always-false ringbuf-init branch
            // deleted; bracket-list initializers go through emit_field_init_value.
            self.emit_field_init_value(out, indent, init_clone, &ty, &gep_reg, *idx);
        }
        // 2026-08-07 (object instance pools): allocate the DEPENDENT pools'
        // heap buffers here — AFTER the field-init stores (the runtime bound
        // fields like `N` are readable) and BEFORE the static instance inits
        // (row 0 lives at the buffer start, so it must exist first).
        self.emit_dependent_pool_buffers(out, indent);
        // 2026-08-07 (object instance pools): the unpacked instances' Init
        // members run here too (the reactor uses the inline init stores, not
        // @init_state).
        let instance_inits: Vec<(String, String, Expr)> = self.ctx.obj_instance_inits.iter()
            .map(|(n, (b, e))| (n.clone(), b.clone(), e.clone()))
            .collect();
        for init in &instance_inits {
            self.emit_instance_init(out, indent, init);
        }
        // Initialize cache slots for LazyCached fields: cache_value = 0, valid_flag = 0
        // 2026-07-19: Sorted for deterministic IR.
        let mut sorted_cache: Vec<&String> = self.ctx.cache_slots.keys().collect();
        sorted_cache.sort();
        for _field_name in &sorted_cache {
            let targets = &self.ctx.cache_slots[*_field_name];
            let mut sorted_targets: Vec<&String> = targets.keys().collect();
            sorted_targets.sort();
            for _target_name in &sorted_targets {
                let &(cache_idx, valid_idx) = &targets[*_target_name];
                // 2026-07-20: Intentionally hand-rolled — uses parameterized state_ptr (not %state).
                let cp = format!("%icp_{}", cache_idx);
                writeln!(out, "{}{} = getelementptr inbounds %State, ptr {}, i32 0, i32 {}", indent, cp, state_ptr, cache_idx).ok();
                writeln!(out, "{}store i64 0, ptr {}, align {}", indent, cp, self.align_of("i64")).ok();
                let vp = format!("%ivp_{}", valid_idx);
                writeln!(out, "{}{} = getelementptr inbounds %State, ptr {}, i32 0, i32 {}", indent, vp, state_ptr, valid_idx).ok();
                writeln!(out, "{}store i8 0, ptr {}, align {}", indent, vp, self.align_of("i8")).ok();
            }
        }
        // 2026-08-09 (init kind, Phase 2): seed runtime-seeded invariants here,
        // after the field-init stores (a body may read state) and before any
        // node fires — mirrors emit_init_state for the inline-SROA path.
        self.emit_init_seeding_stores(out, indent);
    }

    /// 2026-08-09 (init kind, Phase 2): seed every runtime-seeded invariant by
    /// evaluating its value expr (or running its body statements once) and
    /// storing the result to its mutable global. Runs in the pre-reactor phase
    /// — after state-field init stores (a body may read state fields) and
    /// before any node fires. Reads of the seeded globals then load the value;
    /// nothing writes them again (the typechecker + interpreter enforce that).
    pub(super) fn emit_init_seeding_stores(&mut self, out: &mut String, indent: &str) {
        let mut sorted_inits: Vec<String> = self.ctx.inits.keys().cloned().collect();
        sorted_inits.sort();
        for name in &sorted_inits {
            let init = self.ctx.inits[name].clone();
            let llvm_ty = protocol_llvm_type(&init.ty, self.ctx.type_universe.as_ref());
            if let Some(value) = &init.value {
                let reg = self.emit_expr(out, value, indent);
                writeln!(out, "{}store {} {}, ptr @{}, align 8", indent, llvm_ty, reg.name, name).ok();
            } else {
                self.emit_init_seeding_body(out, indent, name, &init);
            }
        }
    }

    /// Emit a body-form init's seeding statements once, storing the yielded
    /// value to the init's global. A `term <expr>` yields the value (emit_expr
    /// directly — the txn-flavored Term arm emits ret/branch, which has no
    /// meaning in the pre-reactor seeding phase); otherwise the last
    /// statement's value is used.
    fn emit_init_seeding_body(
        &mut self,
        out: &mut String,
        indent: &str,
        name: &str,
        init: &crate::ast::top::InitDecl,
    ) {
        let llvm_ty = protocol_llvm_type(&init.ty, self.ctx.type_universe.as_ref());
        let mut last = "".to_string();
        for stmt in &init.body {
            match stmt {
                crate::ast::Statement::Term(Some(expr)) => {
                    let reg = self.emit_expr(out, expr, indent);
                    last = reg.name;
                    break;
                }
                crate::ast::Statement::Term(None) => break,
                _ => {
                    let reg = crate::backend::llvm::emit_stmt::emit_statement(self, out, stmt, indent);
                    last = reg.name;
                }
            }
        }
        if !last.is_empty() {
            writeln!(out, "{}store {} {}, ptr @{}, align 8", indent, llvm_ty, last, name).ok();
        }
    }

    // ── Parameter Boxing ──────────────────────────────────────────
    //
    // 2026-07-01: Box a function parameter from its native LLVM type to i64
    // using the universe-declared box_op intrinsic. This is called at function
    // entry for parameters whose declared LLVM type differs from i64.
    //
    // Why this exists: Function parameters arrive in their native LLVM type
    // (e.g., i32 for Char, i8 for Bool). We widen them to i64 for uniform SSA
    // register storage. The boxing intrinsic (box_op) from the TypeUniverse
    // tells us exactly which LLVM operation to emit — no more hardcoded
    // Type::char_()/Bool/String/Float match arms.
    //
    // This mirrors adapt_via_box_op in emit_stmt.rs but operates on raw
    // parameter registers rather than TypedRegister SSA values. The key
    // difference: adapt_via_box_op handles already-boxed SSA values (no-op
    // for Char since Char SSA values are always i64), while this method
    // handles native-type parameters that need the initial boxing.
    //
    fn emit_box_param(&mut self, out: &mut String, indent: &str, result: &str, raw: &str, box_op: &str) {
        match box_op {
            // Bool: zext i8 → i64 (Bool parameters arrive as i8)
            "zext.i1.to.i64#" => {
                writeln!(out, "{}{} = zext i8 {} to i64", indent, result, raw).ok();
            }
            // Char / UInt32: zext i32 → i64 (native i32 widened to boxed i64)
            "zext.i32.to.i64#" => {
                writeln!(out, "{}{} = zext i32 {} to i64", indent, result, raw).ok();
            }
            // String/Data: ptrtoint ptr → i64 (native pointer to boxed integer)
            "ptrtoint#" => {
                writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, result, raw).ok();
            }
            // Float: bitcast f32→i32 then zext i32→i64.
            // Also cache the native float register so ensure_float_reg can
            // recover the native float from the boxed i64 later.
            "bitcast.f32.to.i64#" => {
                let m = format!("%ai{}", self.fun.txn_counter); self.fun.txn_counter += 1;
                writeln!(out, "{}{} = bitcast float {} to i32", indent, m, raw).ok();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, result, m).ok();
                self.fun.reg_float_cache.insert(result.to_string(), raw.to_string());
            }
            // Float64: bitcast double→i64 (same width, direct bitcast).
            // Cache the native double register for downstream unboxing.
            "bitcast.f64.to.i64#" => {
                writeln!(out, "{}{} = bitcast double {} to i64", indent, result, raw).ok();
                self.fun.reg_float_cache.insert(result.to_string(), raw.to_string());
            }
            // Signed fixed-width: sext i8/i16/i32 to i64
            op if op.starts_with("sext.") => {
                let llvm_ty = match op {
                    "sext.i8.to.i64#" => "i8",
                    "sext.i16.to.i64#" => "i16",
                    _ => "i32",
                };
                writeln!(out, "{}{} = sext {} {} to i64", indent, result, llvm_ty, raw).ok();
            }
            // Unsigned fixed-width: zext i8/i16 to i64
            op if op.starts_with("zext.") && op != "zext.i1.to.i64#" && op != "zext.i32.to.i64#" => {
                let llvm_ty = match op {
                    "zext.i8.to.i64#" => "i8",
                    "zext.i16.to.i64#" => "i16",
                    _ => "i32",
                };
                writeln!(out, "{}{} = zext {} {} to i64", indent, result, llvm_ty, raw).ok();
            }
            // Unknown box_op — the parameter stays in its native LLVM type
            // (e.g., Native storage types like raw structs). No conversion.
            _ => {}
        }
    }

    pub(super) fn emit_definition(&mut self, out: &mut String, d: &crate::ast::Definition, needs_state: bool) {
        self.fun.pending_cleanup.clear();
        self.fun.clear_locals();
        self.fun.reassigned_lets.clear();
        self.fun.expr_dedup_cache.clear();
        self.fun.is_static_bound = false;
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        // 2026-07-01: Use "i64" for all non-float returns instead of llvm_type().
        // The body always produces i64 values (via adapt_to_i64) and call.rs expects
        // i64 at the call site. Using llvm_type() gave "i8*" for String/Bool returns,
        // creating a type mismatch (ret i64 in a define i8* function) that broke opt/llc.
        let has_ret = d.output_type.is_some() || !d.outputs.is_empty();
        // 2026-07-18: Use the correct LLVM type for the return type instead of
        // always using "i64". Bool returns need "i8" to match term's ret i8.
        let ll_ret_ty = if !has_ret {
            "float".to_string()
        } else {
            d.output_type.as_ref()
                .and_then(|ot| match ot {
                    crate::ast::OutputType::Single(ty) => Some(self.llvm_ret_abi_type(ty)),
                    _ => None,
                })
                .unwrap_or_else(|| "i64".to_string())
        };
        let is_float_fn = ll_ret_ty == "float" || ll_ret_ty == "double";
        self.fun.fn_ret_ty = ll_ret_ty.clone();
        self.fun.returns_i64 = has_ret;
        // 2026-08-05 (Phase 6): there is no `main` in Briev. The entry point is
        // whichever node fires first (reactor-driven). User declarations named
        // `main` are rejected by name-collision with the runtime `@main`, so no
        // renaming is performed here.
        let ll_name = d.name.clone();
        if self.ctx.is_shared_lib {
            write!(out, "define dso_local {} @{}(", ll_ret_ty, ll_name).ok();
        } else {
            write!(out, "define {} @{}(", ll_ret_ty, ll_name).ok();
        }
        if needs_state {
            write!(out, "{}", self.ctx.state_ptr_param).ok();
        }
        for (i, (n, t)) in d.parameters.iter().enumerate() {
            if needs_state || i > 0 {
                let _ = write!(out, ", ");
            }
            // 2026-07-04: dereferenceable(N) for Ptr<T> parameters.
            // LLVM parameter attributes on i64 scalars are rejected by
            // LLVM 18+ (only function-level attributes apply). Ptr<T>
            // parameters are also i64 (opaque pointer addresses) so
            // dereferenceable is ommitted — it only works on pointer
            // types, and Ptr<T> is an i64 at the LLVM level.
            let _ = write!(out, "{} %arg{}", self.llvm_type(t), i);
        }
        // 2026-07-04: Use #8 (argmemonly) for definitions.
        // Definitions never access @link trigger globals — they only
        // read/write through %state. argmemonly tells LLVM the function
        // only accesses memory through its pointer arguments.
        // 2026-09-06 (Phase 8): the section(".name") placement — a define
        // attribute (where the function's bytes live), consumed from the
        // parser's section modifier. The no-alloc obligation is proven at
        // the typecheck (check_section_proofs).
        let section_attr = defn_section_name(d)
            .map(|s| format!(" section \"{}\"", s))
            .unwrap_or_default();
        // 2026-09-09 (Family C, briev-native runtime): #8 (argmem) is a LIE
        // for defns that deref raw Int addresses — Load#/Store# over
        // inttoptr'd args are not "through pointer arguments", and LLVM's
        // optimizer exploits the lie under -O3 LTO (hoisted the foreach slot
        // load, miscompiled every string iteration). Defns with direct
        // raw-memory intrinsics emit with #13 (full memory(readwrite));
        // pure-Briev defns keep #8's alias-analysis benefits.
        let attr_idx = if Self::definition_touches_raw_memory(&d.body) {
            "#13"
        } else {
            "#8"
        };
        writeln!(out, ") local_unnamed_addr {}{} {{", attr_idx, section_attr).ok();
        writeln!(out, "  entry:").ok();
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        for (i, (n, t)) in d.parameters.iter().enumerate() {
            let raw = format!("%arg{}", i);
            let reg: String;
            // 2026-07-01: Use universe-declared box_op for parameter boxing.
            // If llvm_type is already i64, no boxing is needed (Int, UInt).
            // If a box_op exists, use the universe-driven emit_box_param to
            // widen from native type to i64. Otherwise, leave the parameter
            // in its native LLVM type (fixed-width integers like Int32, Native
            // storage types like Float64).
            //
            // This replaces the old pattern of matching on Type::char_()/Bool/etc.
            // The WHETHER-to-box decision is still type-category based (the
            // matches! check below), but the HOW-to-box decision now comes from
            // the universe's box_op field rather than hardcoded LLVM IR.
            let param_llvm_ty = self.llvm_type(t);
            // 2026-07-10: Phase 1 — struct params are passed by pointer (ptr).
            // Convert to i64 via ptrtoint so downstream field access code
            // (which does inttoptr + GEP) works unchanged. The round-trip
            // ptrtoint→inttoptr is eliminated by LLVM's optimizer.
            if param_llvm_ty == "ptr" {
                let conv = format!("%ac{}", i);
                writeln!(out, "  {} = ptrtoint ptr {} to i64", conv, raw).ok();
                reg = conv;
            } else if param_llvm_ty == "i64" {
                reg = raw;
                // 2026-08-01 (B4): the SSO String-param {i64,i64} branch was
                // retired — a String param is a ptr (handled above) or an i64
                // address, never a fat pointer under the bits model.
            } else if self.is_boxed_type(t) {
                // 2026-07-12: box_op removed from ResolvedType — use hardcoded fallback.
                // 2026-07-31: Phase 3 (§8.4-D1) — boxed-type detection via
                // protocol membership (is_boxed_type), conversion via
                // emit_box_value_to_i64, replacing the name-match arm.
                let conv = format!("%ac{}", i);
                self.emit_box_value_to_i64(out, "  ", t, &raw, &conv, &format!("%ai{}", i));
                if self.is_protocol_member(t, "Float") {
                    self.fun.reg_float_cache.insert(conv.clone(), raw.to_string());
                } else {
                    // 2026-08-01 (audit): a boxed Bool/Char/Data param reg is
                    // already i64 in SSA; Print# needs to know it's boxed so it
                    // passes it directly (no zext) to __print_bool/__print_char.
                    self.fun.boxed_scalar_regs.insert(conv.clone());
                }
                reg = conv;
            } else {
                reg = raw;
            }
            self.fun.let_bindings.insert(n.clone(), reg.clone());
            // Boxed params (Bool/Char/String/Data) are stored as i64 in
            // let_bindings after boxing. Register as Type::int() so downstream
            // doesn't treat them as native i1/i32/i8*. Float stays Type::float()
            // (handled specially by ensure_float_reg via the cache).
            // 2026-07-01: This decision is type-category based (Bool/Char/
            // String/Data are semantically boxed types) and cannot be derived
            // from universe data alone — Int32 also has storage="Boxed" but
            // stays native in SSA.
            // 2026-07-01: Always store original type for ALL variable types,
            // not just boxed types. Custom types like RingBuffer<Int> need
            // their original type in let_original_types so that arrow strategy
            // dispatch (check_insert_strategy / check_extract_strategy) can
            // look up the type's InsertAt/ExtractFrom in the TypeUniverse.
            // Without this, <- and discard on RingBuffer would fall through
            // to the default List arena path, causing realloc on non-heap memory.
            self.fun.let_original_types.insert(n.clone(), t.clone());
            if self.is_boxed_int_type(t) {
                self.fun.let_binding_types.insert(n.clone(), Type::int());
            } else {
                self.fun.let_binding_types.insert(n.clone(), t.clone());
            }
        }
        self.fun.txn_counter = 0;
        self.fun.terminated = false;
        // 2026-06-26: in_callable_txn must be true so Statement::Term in the
        // defn body emits a ret instruction (emit_stmt.rs line 78). Without
        // this, Statement::Term becomes a no-op, terminated stays false, and
        // the function falls through to "ret i64 0" — every defn silently
        // returns zero regardless of its actual computation.
        self.fun.in_callable_txn = true;
        // 2026-08-04 (compiler-in-Briev): pre-declare entry-block allocas for
        // top-level lets that are reassigned inside a guard/loop — the 
        // reassignment must NOT demote to an alloca at the assignment site
        // (dominance violation). The alloca is bound BEFORE the body so the
        // let's initial store and later stores both target the entry alloca.
        self.fun.reassigned_lets = Self::collect_reassigned_lets(&d.body);
        let reassigned: Vec<(String, Option<Type>)> = self.fun.reassigned_lets.iter()
            .map(|(n, t)| (n.clone(), t.clone())).collect();
        for (name, ty) in &reassigned {
            let slot = self.fun.gen_reg();
            let slot_ty = ty.as_ref().map(|t| self.llvm_type(t)).unwrap_or_else(|| "i64".to_string());
            writeln!(out, "  {} = alloca {}, align 8", slot, slot_ty).ok();
            self.fun.let_bindings.insert(name.clone(), slot.clone());
            self.fun.let_binding_allocas.insert(slot.clone());
            self.fun.let_binding_types.insert(name.clone(), ty.clone().unwrap_or_else(Type::int));
            self.fun.let_original_types.insert(name.clone(), ty.clone().unwrap_or_else(Type::int));
        }
        for s in &d.body {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }
        // 2026-08-09 (Phase 10): a fallthrough exit runs registered defers.
        self.flush_defer_cleanup(out, "  ");
        // Foreign destructor cleanup: emit OnExit calls before returning
        self.emit_on_exit_cleanup(out, "  ");
        if !self.fun.terminated {
            if is_float_fn {
                writeln!(out, "  ret float 0.0").ok();
            } else if d.outputs.is_empty() && d.output_type.is_none() {
                // 2026-07-23: Check both legacy outputs and modern output_type.
                // Without this, definitions using -> Int (which sets output_type
                // but leaves outputs empty) get ret void while the function
                // signature says i64.
                writeln!(out, "  ret void").ok();
            } else {
                // 2026-07-23: Emit the correct zero value for the return type.
                // Previously always used "ret i64 0", which broke functions
                // returning ptr, float, double, etc.
                // 2026-09-06: `-> Void` (output_type Some(Single(Void))) has a
                // declared but empty return — ret void, no value (was
                // `ret void 0`, rejected by clang).
                if ll_ret_ty == "void" {
                    writeln!(out, "  ret void").ok();
                } else {
                    let zero_val = match ll_ret_ty.as_str() {
                        "ptr" => "null",
                        "float" | "double" => "0.0",
                        _ => "0",
                    };
                    writeln!(out, "  ret {} {}", ll_ret_ty, zero_val).ok();
                }
            }
        }
        writeln!(out, "}}").ok();
        // Phase 4.5: Emit dso_local export wrapper if #export modifier present
        if let Some(export_name) = Self::get_export_name(&d.modifiers) {
            writeln!(out, "define dso_local {} @{}(" , ll_ret_ty, export_name).ok();
            write!(out, "{}", self.ctx.state_ptr_param).ok();
            for (i, (n, t)) in d.parameters.iter().enumerate() {
                write!(out, ", {} %arg{}", self.llvm_type(t), i).ok();
            }
            writeln!(out, ") local_unnamed_addr #0 {{").ok();
            write!(out, "  %res = call {} @{}(", ll_ret_ty, ll_name).ok();
            write!(out, "ptr %state").ok();
            for (i, (n, t)) in d.parameters.iter().enumerate() {
                write!(out, ", {} %arg{}", self.llvm_type(t), i).ok();
            }
            writeln!(out, ") local_unnamed_addr #0").ok();
            writeln!(out, "  ret {} %res", ll_ret_ty).ok();
        writeln!(out, "}}").ok();
    }

}

    // 2026-06-13: Added ptr %state param — definitions can access global state.

/// 2026-09-09 (Family C, briev-native runtime): does this defn body DIRECTLY
/// call a raw-memory intrinsic? Load#/Store#/Alloc#/Free#/Copy#/Fill#/
/// VolatileLoad#/VolatileStore#/SysCall# deref (or expose) inttoptr'd Int
/// addresses — such defns must emit with #13 (full memory(readwrite)) since
/// memory(argmem)'s "through pointer arguments" contract does not hold for
/// int-address accesses. Helpers called with their own attrs are NOT scanned
/// (the callee's attribute governs its own accesses).
pub(crate) fn definition_touches_raw_memory(stmts: &[Statement]) -> bool {
    const RAW: &[&str] = &[
        "Load#", "Store#", "Alloc#", "Free#", "Copy#", "Fill#",
        "VolatileLoad#", "VolatileStore#", "SysCall#",
    ];
    fn expr_has(e: &Expr, found: &mut bool) {
        if *found { return; }
        match e {
            Expr::Call(name, args, _) => {
                if RAW.iter().any(|r| *r == name.as_str()) {
                    *found = true;
                    return;
                }
                for a in args { expr_has(a, found); }
            }
            Expr::BinaryOp(_, l, r) => { expr_has(l, found); expr_has(r, found); }
            Expr::UnaryOp(_, i) => expr_has(i, found),
            Expr::Index(o, i) => { expr_has(o, found); expr_has(i, found); }
            Expr::Field(o, _) => expr_has(o, found),
            Expr::Cast(i, _) => expr_has(i, found),
            Expr::Tuple(xs) | Expr::List(xs) => {
                for x in xs { expr_has(x, found); }
            }
            _ => {}
        }
    }
    fn stmt_has(s: &Statement, found: &mut bool) {
        if *found { return; }
        match s {
            Statement::Assign(l, r) => { expr_has(l, found); expr_has(r, found); }
            Statement::Expression(e) => expr_has(e, found),
            Statement::Let { expr: Some(e), .. } => expr_has(e, found),
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => expr_has(e, found),
            Statement::Guarded(_, b) | Statement::Block(b) | Statement::SyncBlock(b) => {
                for x in b { stmt_has(x, found); }
            }
            Statement::Mutex(b) => {
                for x in b { stmt_has(x, found); }
            }
            _ => {}
        }
    }
    let mut found = false;
    for s in stmts {
        stmt_has(s, &mut found);
        if found { break; }
    }
    found
}
    // Was missing the state pointer, causing invalid LLVM IR (SSA value out of scope).

    // 2026-07-04: Return the known !range bounds for a Briev type based on
    // its byte size in the type universe.  Narrow integer types have
    // representation-level ranges that LLVM can exploit for bounds-check
    // elimination — no contract precondition required.
    // Two-tier range system:
    // 1. Type-driven: from universe byte size (always available, no contract)
    // 2. Contract-driven: from [pre] constraints (may tighten type-driven range)
    // Returns None for types without a known range (Int is unbounded, Float
    // is not range-constrained).
    fn type_driven_range(universe: &TypeUniverse, ty: &Type) -> Option<(i64, i64)> {
        // 2026-07-14: byte_size removed from TypeUniverse, use ResolvedType.bytes instead.
        let key = ty.universe_key()?;
        let resolved = universe.get(key)?;
        // 2026-07-31: Phase 3 (§8.3) — byte→range is a representation fact
        // (N bytes = unsigned range [0, 2^(8N))), DERIVED from resolved.bytes
        // instead of a hardcoded 1/2-byte table. Behavior unchanged for the
        // 1/2-byte cases the previous table covered.
        match resolved.bytes {
            1 | 2 => Some((0, 1i64 << (resolved.bytes * 8))),
            _ => None,
        }
    }

    /// Emit a range metadata node in the LLVM integer width of the field's
    /// storage type. LLVM range metadata requires its bounds to be the same
    /// integer type as the load they annotate — `!range !{ i64 0, i64 256 }`
    /// on a `load i8` (Bool/UInt8/Int8 fields) is malformed and crashes clang
    /// (computeKnownBitsFromRangeMetadata, APInt::setBitsSlowCase). Returns
    /// the metadata id, or None if the bound does not fit the field width
    /// (the range is vacuous for that type and must be skipped).
    /// 2026-08-01: exposed by the entry!/args! plugin's Bool done-flags.
    fn emit_range_metadata(
        range_meta: &mut Vec<String>,
        field_llvm_ty: &str,
        lo: i64,
        hi: i64,
    ) -> Option<usize> {
        // Extract the integer width from the LLVM type ("i8"/"i16"/"i32"/"i64").
        // 2026-09-02 (plan fundamental-parent-membership): range metadata is
        // INTEGER-only in LLVM — a float storage type ("half"/"float"/
        // "double") must skip it entirely; the old `or(Some(64))` default
        // laundered non-integer types into malformed `!{ half ... }` nodes
        // once Float16 state slots resolved natively. Undo: restore the
        // or(Some(64)) fallback.
        let Some(bits): Option<u32> = field_llvm_ty
            .strip_prefix('i')
            .and_then(|s| s.parse().ok())
        else {
            return None;
        };
        // The range bounds must be representable in `bits`. For a signed load
        // width N, LLVM interprets range bounds as that integer type; hi must
        // fit. If the bound exceeds the width (e.g. 256 in i8), the range is
        // vacuous for this storage type — skip it. 64-bit uses the full i64
        // span (1 << 63 overflows a signed i64, so handle it explicitly).
        let (max_signed, min_signed) = if bits >= 64 {
            (i64::MAX, i64::MIN)
        } else {
            (1i64 << (bits - 1), -(1i64 << (bits - 1)))
        };
        if hi > max_signed || lo < min_signed {
            return None;
        }
        let mi = range_meta.len() + 50;
        range_meta.push(format!("!{} = !{{ {} {}, {} {} }}", mi, field_llvm_ty, lo, field_llvm_ty, hi));
        Some(mi)
    }

    /// 2026-07-27: Rewrite identifier references in a statement to use cold-function
    /// parameter names. Each Expr::Identifier matching a field_name is replaced with
    /// the corresponding parameter name (__cp_{field}).
    fn rewrite_stmt_idents(stmt: &Statement, field_names: &[String], param_names: &[String]) -> Statement {
        use crate::ast::*;
        match stmt {
            Statement::Let { name, names, ty, expr, modifiers } => {
                Statement::Let {
                    name: name.clone(),
                    names: names.clone(),
                    ty: ty.clone(),
                    expr: expr.as_ref().map(|e| Self::rewrite_expr_idents(e, field_names, param_names)),
                    modifiers: modifiers.clone(),
                }
            }
            Statement::Expression(e) => Statement::Expression(Self::rewrite_expr_idents(e, field_names, param_names)),
            Statement::Assign(lhs, rhs) => {
                Statement::Assign(
                    Self::rewrite_expr_idents(lhs, field_names, param_names),
                    Self::rewrite_expr_idents(rhs, field_names, param_names),
                )
            }
            Statement::Term(Some(e)) => Statement::Term(Some(Self::rewrite_expr_idents(e, field_names, param_names))),
            Statement::EndProgram(Some(e)) => Statement::EndProgram(Some(Self::rewrite_expr_idents(e, field_names, param_names))),
            Statement::Guarded(cond, body) => {
                Statement::Guarded(
                    Self::rewrite_expr_idents(cond, field_names, param_names),
                    body.iter().map(|s| Self::rewrite_stmt_idents(s, field_names, param_names)).collect(),
                )
            }
            _ => stmt.clone(),
        }
    }

    fn rewrite_expr_idents(expr: &Expr, field_names: &[String], param_names: &[String]) -> Expr {
        use crate::ast::*;
        match expr {
            Expr::Identifier(name) => {
                if let Some(pos) = field_names.iter().position(|f| f == name) {
                    Expr::Identifier(param_names[pos].clone())
                } else {
                    Expr::Identifier(name.clone())
                }
            }
            Expr::Call(name, args, id) => {
                Expr::Call(name.clone(),
                    args.iter().map(|a| Self::rewrite_expr_idents(a, field_names, param_names)).collect(),
                    *id)
            }
            Expr::BinaryOp(op, lhs, rhs) => {
                Expr::BinaryOp(*op,
                    Box::new(Self::rewrite_expr_idents(lhs, field_names, param_names)),
                    Box::new(Self::rewrite_expr_idents(rhs, field_names, param_names)))
            }
            Expr::UnaryOp(op, e) => Expr::UnaryOp(*op, Box::new(Self::rewrite_expr_idents(e, field_names, param_names))),
            Expr::Cast(inner, target) => Expr::Cast(Box::new(Self::rewrite_expr_idents(inner, field_names, param_names)), target.clone()),
            Expr::Field(obj, f) => Expr::Field(Box::new(Self::rewrite_expr_idents(obj, field_names, param_names)), f.clone()),
            Expr::Index(obj, idx) => Expr::Index(Box::new(Self::rewrite_expr_idents(obj, field_names, param_names)), Box::new(Self::rewrite_expr_idents(idx, field_names, param_names))),
            Expr::Block(stmts) => Expr::Block(stmts.iter().map(|s| Self::rewrite_stmt_idents(s, field_names, param_names)).collect()),
            Expr::List(elems) | Expr::Tuple(elems) => {
                let n: Vec<Expr> = elems.iter().map(|e| Self::rewrite_expr_idents(e, field_names, param_names)).collect();
                if let Expr::List(_) = expr { Expr::List(n) } else { Expr::Tuple(n) }
            }
            Expr::Deref(inner) => Expr::Deref(Box::new(Self::rewrite_expr_idents(inner, field_names, param_names))),
            Expr::AddrOf(inner) => Expr::AddrOf(Box::new(Self::rewrite_expr_idents(inner, field_names, param_names))),
            _ => expr.clone(),
        }
    }

    pub(super) fn emit_transaction(&mut self, out: &mut String, txn: &crate::ast::Transaction, name: &str, range_meta: &mut Vec<String>) {
        let has_output = txn.output_type.is_some() || !txn.outputs.is_empty();
        if !txn.is_reactive && (!txn.parameters.is_empty() || has_output) {
            self.emit_callable_txn(out, txn, name);
            return;
        }
        // 2026-07-27: Set txn_name for per-function arena gating.
        self.fun.txn_name = name.to_string();
        self.fun.pending_cleanup.clear();
        self.ctx.range_bounds = Self::extract_ranges_with_constants(
            &txn.contract.pre_condition, &self.ctx.constants);
        self.ctx.field_to_meta_idx.clear();
        self.ctx.idx_to_field_name.clear();
        for (name, &idx) in &self.ctx.field_index_map {
            self.ctx.idx_to_field_name.insert(idx, name.clone());
        }
        // 2026-08-01: A contract-derived !range is only sound for fields that
        // no node body writes. The range is extracted from the *precondition*
        // (e.g. [tick < 1] ⇒ tick ∈ [MIN, 1)), but emit_state_load attaches it
        // to EVERY load of the field — including the dispatch-loop guard that
        // re-reads state each tick. If the node body writes tick = 1, the value
        // leaves the range on the next iteration, which is UB in LLVM semantics;
        // clang then assumes the guard always fires and the reactor never
        // converges (observed: format-demo infinite loop). The write_set (computed
        // once in the transition graph) is the exact guard: if no node writes the
        // field, the precondition range is loop-invariant and sound. Type-driven
        // ranges below are still emitted for written fields — a UInt8 field holds
        // [0, 256) by type construction regardless of writes.
        let written_fields = self.ctx.transition_graph.as_ref()
            .map(|g| g.nodes.iter().flat_map(|n| n.write_set.iter().cloned()).collect::<HashSet<String>>())
            .unwrap_or_default();
        for (f, &(lo, hi)) in &self.ctx.range_bounds {
            if hi < i64::MAX && !written_fields.contains(f) {
                // 2026-07-18: Offset by 50 to avoid collision with TBAA metadata nodes
                // (!0 through ~!20). LLVM metadata IDs must be unique across the module.
                // 2026-08-01: bounds emitted in the field's LLVM integer width
                // (range metadata must match the load type — see emit_range_metadata).
                let field_llvm = self.ctx.field_types.get(
                    self.ctx.field_index_map.get(f).copied().unwrap_or(0),
                ).cloned().unwrap_or_else(|| "i64".to_string());
                if let Some(mi) = Self::emit_range_metadata(range_meta, &field_llvm, lo, hi) {
                    self.ctx.field_to_meta_idx.insert(f.clone(), mi);
                }
            }
        }
        // 2026-07-04: Type-driven !range for narrow integer types.
        // The type universe provides byte size information for each type.
        // Types with known ranges (UInt8, UInt16, UInt32, Int8, Char) get
        // automatic !range metadata on their field loads — no contract
        // precondition required. This is information the type system
        // provides for free that LLVM uses to eliminate bounds checks.
        if let Some(ref universe) = self.ctx.type_universe {
            for (field_name, &idx) in &self.ctx.field_index_map {
                let briev_ty = self.ctx.field_briev_types.get(idx);
                let Some(briev_ty) = briev_ty else { continue; };
                let Some(range_bounds) = Self::type_driven_range(universe, briev_ty) else { continue; };
                // Only add if no contract-driven range already exists
                // (contract ranges are tighter and take priority).
                if !self.ctx.field_to_meta_idx.contains_key(field_name) {
                    // 2026-07-18: Offset by 50 to avoid TBAA node collision.
                    // 2026-08-01: bounds emitted in the field's LLVM width —
                    // `!{ i64 0, i64 256 }` on a load i8 (Bool/UInt8/Int8)
                    // crashes clang. Skip when the bound is vacuous for the
                    // storage type (e.g. 256 does not fit i8).
                    let field_llvm = self.ctx.field_types.get(idx)
                        .cloned().unwrap_or_else(|| "i64".to_string());
                    if let Some(mi) = Self::emit_range_metadata(
                        range_meta, &field_llvm, range_bounds.0, range_bounds.1,
                    ) {
                        self.ctx.field_to_meta_idx.insert(field_name.clone(), mi);
                    }
                }
            }
        }
        // Resolve #inline / #?inline directives from transaction modifiers.
        // #inline forces alwaysinline regardless of cycles.
        // #?inline emits inlinehint (safe with cycles).
        // If neither, fall back to cycle-based alwaysinline.
        let alwaysinline = {
            let dirs = super::directive::resolve_directives(
                &txn.modifiers,
                super::directive::DirectiveCtx::Transaction,
            );
            let inline_attr = dirs.iter().find_map(|e| {
                if let super::directive::DirectiveEffect::FunctionAttribute(a) = e {
                    Some(a.as_str())
                } else {
                    None
                }
            });
            // Emit remarks for speculative #?inline directives.
            if txn.modifiers.iter().any(|m| m.name == "inline" && m.name.starts_with('?')) {
                let remark = match inline_attr {
                    Some("inlinehint") => {
                        super::directive::OptimizationRemark::applied("inline",
                            format!("inlinehint applied to txn '{}'", name))
                    }
                    _ => {
                        super::directive::OptimizationRemark::skipped("inline",
                            "inlinehint not applicable for this context".to_string())
                    }
                };
                self.push_remark(remark);
            }
            match inline_attr {
                Some("alwaysinline") => " alwaysinline",
                Some("inlinehint") => " inlinehint",
                _ => {
                    if !self.ctx.has_cycles { " alwaysinline" } else { "" }
                }
            }
        };
        // 2026-07-28: Phase H.0 — Emit function-level attributes from !> metadata.
        // Iterates over txn.metadata (sorted for deterministic IR), converts each
        // PropertyValue to a string, and looks up the LLVM IR attribute via the
        // MetadataRegistry. The registry's wildcard pattern "*" matches any value.
        let meta_attrs: String = {
            let registry = &self.ctx.metadata_registry;
            let mut attrs = String::new();
            let mut keys: Vec<&String> = txn.metadata.keys().collect();
            keys.sort();
            for key in keys {
                let v = &txn.metadata[key];
                let value_str = match v {
                    crate::ast::PropertyValue::Bool(b) => { if *b { "true".to_string() } else { "false".to_string() } }
                    crate::ast::PropertyValue::Int(n) => n.to_string(),
                    crate::ast::PropertyValue::Float(f) => f.to_string(),
                    crate::ast::PropertyValue::String(s) => s.clone(),
                    crate::ast::PropertyValue::Identifier(s) => s.clone(),
                    _ => continue,
                };
                if let Some(llvm_attr) = registry.llvm_attr(key, &value_str) {
                    attrs.push(' ');
                    attrs.push_str(llvm_attr);
                }
            }
            attrs
        };
        // 2026-07-27: txn_attr for assume_action path (no outlining — rare path).
        // The non-assume_action path below computes its own attr from reordered body.
        let txn_attr = "#0";

        // ── Helper: collect identifiers from a statement ────────────────
        /// 2026-07-28: Check if an expression references the named induction variable.
        fn get_induction_var_name(e: &Expr, var: &str) -> Option<String> {
            match e {
                Expr::Identifier(name) if name == var => Some(name.clone()),
                Expr::BinaryOp(_, lhs, rhs) => {
                    get_induction_var_name(lhs, var).or_else(|| get_induction_var_name(rhs, var))
                }
                Expr::UnaryOp(_, inner) => get_induction_var_name(inner, var),
                _ => None,
            }
        }
        fn collect_let_names(stmt: &Statement, names: &mut Vec<String>) {
            match stmt {
                Statement::Let { name, names: extra, .. } => {
                    names.push(name.clone());
                    for n in extra { names.push(n.clone()); }
                }
                Statement::Guarded(_, body) => { for s in body { collect_let_names(s, names); } }
                Statement::Block(body) => { for s in body { collect_let_names(s, names); } }
                _ => {}
            }
        }
        fn has_ffi_call(stmt: &Statement, observable: &std::collections::HashSet<String>) -> bool {
            match stmt {
                Statement::Let { expr: Some(e), .. } => is_ffi_call(e, observable),
                Statement::Expression(e) => is_ffi_call(e, observable),
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => is_ffi_call(e, observable),
                Statement::Assign(_, e) => is_ffi_call(e, observable),
                Statement::Guarded(_, body) => body.iter().any(|s| has_ffi_call(s, observable)),
                Statement::Block(body) => body.iter().any(|s| has_ffi_call(s, observable)),

                _ => false,
            }
        }
        fn is_ffi_call(expr: &Expr, observable: &std::collections::HashSet<String>) -> bool {
            match expr {
                Expr::Call(name, _, _) => {
                    !name.ends_with('#')
                    // 2026-07-28: Observable intrinsics (Print#, Malloc#, etc.)
                    // also need outlining — they create memory barriers in guard
                    // bodies even though they're not FFI calls. Check the intrinsic
                    // signature's observable flag to distinguish from inert intrinsics
                    // like Add#, Sub#, etc.
                    || crate::intrinsic_signatures::get_intrinsic_signature(name)
                        .map_or(false, |sig| sig.observable)
                    // 2026-08-04 (out-observability plan): a call to an `out`
                    // defn/txn is an observable boundary — it must be outlined
                    // out of guard bodies (same memory-barrier rationale as the
                    // observable intrinsics) and survive DCE.
                    || observable.contains(name)
                }
                Expr::Block(stmts) => stmts.iter().any(|s| has_ffi_call(s, observable)),
                _ => false,
            }
        }
        // 2026-08-18 (Phase E, HashMap surface): a guard body may be outlined
        // into a cold function ONLY if it is a pure scalar read of its captured
        // idents. The cold function has no `%state` param — every state access
        // must flow through the rewritten idents — and it CANNOT write state
        // (a rewritten `done = true` would store to the param copy and vanish).
        // Member calls inline member bodies that read POOLED columns via
        // `%state`, so any method call, reflection, index, tuple, block,
        // if/match, or foreach in the guard body defeats outlining. The one
        // supported shape is `println!(sum)`-style FFI/observable prints over
        // scalar idents (and `let`-derived scalars).
        fn outlinable_expr(expr: &Expr) -> bool {
            match expr {
                Expr::Identifier(_)
                | Expr::Quoted(_)
                | Expr::Decimal(_)
                | Expr::Char(_)
                | Expr::TaggedLiteral(_, _)
                | Expr::TaggedQuotedLiteral(_, _)
                | Expr::Bool(_)
                | Expr::Float(_) => true,
                Expr::Call(_, args, _) | Expr::PluginIntercept { args, .. } => {
                    args.iter().all(outlinable_expr)
                }
                Expr::BinaryOp(_, lhs, rhs) => outlinable_expr(lhs) && outlinable_expr(rhs),
                Expr::UnaryOp(_, e) => outlinable_expr(e),
                Expr::Cast(inner, _) => outlinable_expr(inner),
                _ => false,
            }
        }
        fn outlinable_stmt(stmt: &Statement) -> bool {
            match stmt {
                Statement::Expression(e) => outlinable_expr(e),
                Statement::Let { expr: Some(e), .. } => outlinable_expr(e),
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => outlinable_expr(e),
                _ => false,
            }
        }
        fn collect_idents(stmt: &Statement, names: &mut Vec<String>) {
            match stmt {
                Statement::Let { expr: Some(e), .. } => collect_expr_idents(e, names),
                Statement::Expression(e) => collect_expr_idents(e, names),
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => collect_expr_idents(e, names),
                Statement::Assign(lhs, rhs) => { collect_expr_idents(lhs, names); collect_expr_idents(rhs, names); }
                Statement::Guarded(_, body) => { for s in body { collect_idents(s, names); } }
                Statement::Block(body) => { for s in body { collect_idents(s, names); } }
                _ => {}
            }
        }
        fn collect_expr_idents(expr: &Expr, names: &mut Vec<String>) {
            match expr {
                Expr::Identifier(name) => { names.push(name.clone()); }
                Expr::Call(_, args, _) => { for a in args { collect_expr_idents(a, names); } }
                Expr::BinaryOp(_, lhs, rhs) => { collect_expr_idents(lhs, names); collect_expr_idents(rhs, names); }
                Expr::UnaryOp(_, e) => collect_expr_idents(e, names),
                Expr::Cast(inner, _) => collect_expr_idents(inner, names),
                Expr::Field(obj, _) => collect_expr_idents(obj, names),
                Expr::Index(obj, idx) => { collect_expr_idents(obj, names); collect_expr_idents(idx, names); }
                Expr::Block(stmts) => { for s in stmts { collect_idents(s, names); } }
                Expr::List(elems) | Expr::Tuple(elems) => { for e in elems { collect_expr_idents(e, names); } }
                Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::IsType(inner, _) | Expr::Consume(inner) => collect_expr_idents(inner, names),
                _ => {}
            }
        }

        let assume_action: Option<String> = txn.modifiers.iter()
            .find(|m| m.name == "assume_shape")
            .and_then(|m| m.value.as_ref().and_then(|v| {
                if let Expr::Quoted(bytes) = v {
                    Some(String::from_utf8_lossy(bytes).to_string())
                } else {
                    None
                }
            }))
            .and_then(|v| {
                let parts: Vec<&str> = v.splitn(2, ", ").collect();
                if parts.len() == 2 {
                    let action = parts[1].trim();
                    if action == "run" || action == "exit" { Some(action.to_string()) } else { Some("escape".to_string()) }
                } else {
                    Some("escape".to_string())
                }
            });

        // 2026-07-31: Phase 3 (§8.5-E5) — the unconditional `br i1 true, label
        // %body, label %rollback` made the rollback block (and its
        // exit/run/escape action handling) unreachable dead code; both removed.
        // The assume_shape modifier still selects this emission path.
        if assume_action.is_some() {
            // 2026-07-17: Use @txn_<name> prefix so call sites in emit_ssa_loop
            // and emit_folded_multi_main can reference the function (they call
            // @txn_{name}). Without this, the definition is @<name> but the
            // call is @txn_<name> — undefined reference error at link time.
            // 2026-09-07: reset cur_block — a fresh function has no current
            // block; the previous function's label is not a valid predecessor.
            self.fun.cur_block = None;
            writeln!(out, "define void @txn_{}({}) local_unnamed_addr {}{}{} {{", name, self.ctx.state_ptr_param, txn_attr, alwaysinline, meta_attrs).ok();
            writeln!(out, "  entry:").ok();
            // Arena for body emission — same rationale as the standard path:
            // the reactor dispatch calls @txn_name as a separate function,
            // so arena allocas must live here, not in main().
            self.emit_arena_init(out, "  ");            self.fun.ssa_old_int_regs.clear();
            self.fun.ssa_old_float_regs.clear();
            // 2026-06-28: Do NOT reset txn_counter here — this emits into the
            // existing @main() function. Resetting would produce duplicate
            // %t{N} registers across inlined transactions, violating SSA.
            // The counter keeps incrementing across all inlined transactions.
            self.fun.clear_locals();
            self.fun.terminated = false;
            // 2026-06-26: Reset in_callable_txn — emit_definition may have left
            // it true from a prior TopLevel::Definition. Reactive transactions
            // use the non-callable code path (emit_stmt.rs:162). Without this,
            // term!/TermBang inside guards takes the callable path, emits no
            // ret/br, and leaves basic blocks unterminated.
            self.fun.in_callable_txn = false;
            self.fun.returns_i64 = false;
            self.fun.fn_ret_ty = "void".to_string();
            if !matches!(txn.contract.pre_condition, Expr::Bool(true)) {
                self.emit_precondition_check(out, &txn.contract.pre_condition, "  ");
            }
            // 2026-07-29: Statement reordering removed — proven counterproductive.
            // LLVM's scheduler does this better within each basic block.
        for s in &txn.body {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }
            // 2026-08-09 (Phase 10): a fallthrough exit runs registered defers.
            self.flush_defer_cleanup(out, "  ");
            if !self.fun.terminated {
                self.emit_beginprogram_goal_check(out, txn);
                self.emit_arena_fini(out, "  ");
                // 2026-08-01 (D2): garbage scheduling — a non-loop transaction
                // that is a heap-backed field's last consumer frees it after
                // its body. The free fires exactly once, after the body (the
                // reactor fires the node once).
                if let Some(fields) = self.ctx.global_free_after.get(name).cloned() {
                    self.emit_scheduled_frees(out, &fields);
                }
                writeln!(out, "  ret void").ok();
            } else if let Some(fields) = self.ctx.global_free_after.get(name) {
                // 2026-08-06 (diagnostics): the body ended in `term`, so the
                // free block above is skipped and the scheduled frees are
                // never emitted — the heap fields leak. (A free inside the
                // body would be a use-after-free for a node the reactor fires
                // more than once, so the skip is the conservative choice.)
                for f in fields {
                    self.warnings.push(format!(
                        "warning: heap state field '{}' is provably dead after '{}' but \
                         the node ends in a term — the planned free has no sound emission \
                         point and the field will leak",
                        f, name
                    ));
                }
            }
            writeln!(out, "}}").ok();
        } else {
            // 2026-07-29: Statement reordering removed — proven counterproductive.
            // Use body directly (reordering removed 2026-07-29).
            // The `reordered` alias preserves existing code patterns below.
            let body = &txn.body;
            let reordered = body;
            // 2026-07-27: Three-category outlining — identifiers can be state
            // fields (GEP+load), let bindings (lookup at emission time), or
            // compile-time constants (ctx.constants). Only `Unknown` blocks
            // outlining. This lets ring_buffer (references CAP) and nbody
            // (references energy) get #11 = memory(argmem: readwrite).
            #[derive(Debug, Clone)]
            enum ParamSrc {
                StateField(usize),
                LetBinding(String), // LLVM type string ("float" or "i64")
                Constant(Expr, Type), // value expression + Briev type
            }
            // Scan body for FFI guards that can be outlined.
            let outlined_info: Vec<(usize, String, Vec<(String, String, ParamSrc)>)> = {
                let mut ffi_guard_indices: Vec<usize> = Vec::new();
                let mut guard_bodies: Vec<&[Statement]> = Vec::new();
                for (ri, s) in reordered.iter().enumerate() {
                    if let Statement::Guarded(_, body) = s {
                        // 2026-08-18: only PURE scalar-read guard bodies may be
                        // outlined — see outlinable_stmt above. A mutating or
                        // member-calling guard must stay inline (the cold copy
                        // would reference the undefined `%state` or lose writes).
                        if body.iter().any(|s| has_ffi_call(s, &self.ctx.observable_names))
                            && body.iter().all(outlinable_stmt)
                        {
                            ffi_guard_indices.push(ri);
                            guard_bodies.push(body);
                        }
                    }
                }
                let mut can_outline_all = true;
                let mut param_sets: Vec<Vec<(String, String, ParamSrc)>> = Vec::new();
                // Collect let-names defined in the txn body before each guard
                // (pre-scan: which identifiers are let bindings?)
                let mut txn_let_names: Vec<String> = Vec::new();
                let mut txn_let_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                for s in reordered.iter() {
                    collect_let_names(s, &mut txn_let_names);
                    if let Statement::Let { name, ty: Some(t), .. } = s {
                        // 2026-07-31: Phase 3 (§8.4) — float let-param LLVM type
                        // derived from #Float protocol membership + byte width
                        // (4 → float, 8 → double) instead of the type-name match.
                        // The casting graph's Fixed("float") for the #Float
                        // category doesn't distinguish Float64, so width is read
                        // from the universe bytes. Other widths (e.g. BFloat)
                        // stay i64, matching the prior name-based behavior.
                        let llvm_ty = if self.is_protocol_member(t, "Float") {
                            let bytes = self.ctx.type_universe.as_ref()
                                .and_then(|u| t.universe_key().and_then(|k| u.get(k)))
                                .map(|rt| rt.bytes).unwrap_or(4);
                            if bytes == 8 { "double" } else if bytes == 4 { "float" } else { "i64" }
                        } else {
                            "i64"
                        };
                        txn_let_types.insert(name.clone(), llvm_ty.to_string());
                    }
                }
                txn_let_names.sort();
                txn_let_names.dedup();
                for body in &guard_bodies {
                    // 2026-08-22 (Phase 3b): a USER CALL inside the outlined
                    // body marshals `ptr %state` as its first argument — the
                    // cold function has no %state param, so outlining would
                    // emit a reference to an undefined value. Leave such
                    // guards inline (intrinsics/FFI calls are unaffected).
                    fn expr_has_user_call(
                        e: &Expr,
                        frgn: &std::collections::HashMap<String, crate::ast::ForeignSignature>,
                    ) -> bool {
                        match e {
                            Expr::Call(name, args, _) => {
                                (!name.ends_with('#') && !frgn.contains_key(name.as_str()))
                                    || args.iter().any(|a| expr_has_user_call(a, frgn))
                            }
                            Expr::BinaryOp(_, l, r) => {
                                expr_has_user_call(l, frgn) || expr_has_user_call(r, frgn)
                            }
                            Expr::UnaryOp(_, x) => expr_has_user_call(x, frgn),
                            Expr::Cast(x, _) => expr_has_user_call(x, frgn),
                            Expr::Field(o, _) => expr_has_user_call(o, frgn),
                            Expr::Index(o, i) => {
                                expr_has_user_call(o, frgn) || expr_has_user_call(i, frgn)
                            }
                            Expr::List(es) | Expr::Tuple(es) => {
                                es.iter().any(|x| expr_has_user_call(x, frgn))
                            }
                            Expr::Deref(x)
                            | Expr::AddrOf(x)
                            | Expr::IsType(x, _)
                            | Expr::Consume(x) => expr_has_user_call(x, frgn),
                            _ => false,
                        }
                    }
                    let stmt_has = body.iter().any(|stmt| match stmt {
                        Statement::Let { expr: Some(e), .. }
                        | Statement::Expression(e)
                        | Statement::Term(Some(e))
                        | Statement::EndProgram(Some(e)) => expr_has_user_call(e, &self.ctx.frgn_map),
                        Statement::Assign(_, r) => expr_has_user_call(r, &self.ctx.frgn_map),
                        _ => false,
                    });
                    if stmt_has {
                        can_outline_all = false;
                        break;
                    }
                    let mut idents: Vec<String> = Vec::new();
                    let mut local_lets: Vec<String> = Vec::new();
                    for stmt in *body {
                        collect_idents(stmt, &mut idents);
                        collect_let_names(stmt, &mut local_lets);
                    }
                    idents.sort(); idents.dedup();
                    local_lets.sort(); local_lets.dedup();
                    idents.retain(|i| !local_lets.contains(i));
                    let mut params: Vec<(String, String, ParamSrc)> = Vec::new();
                    for ident in &idents {
                        if let Some(&idx) = self.ctx.field_index_map.get(ident) {
                            // Ptr<T> fields are stored as i64 in %State (opaque handles).
                            // Float fields use the native float type. All others use i64.
                            let briev_ty = self.ctx.field_briev_types.get(idx).cloned().unwrap_or(Type::int());
                            // 2026-07-31 (A4): aggregate (array) fields cannot be
                            // outlined as scalar cold-function params — the guard
                            // must stay inline so it reads them via the %State GEP.
                            if matches!(briev_ty, Type::Vector(_, _)) {
                                can_outline_all = false;
                                break;
                            }
                            let llvm_ty = if matches!(briev_ty, Type::Ptr(_)) {
                                "i64".to_string()
                            } else {
                                self.llvm_type(&briev_ty)
                            };
                            params.push((ident.clone(), llvm_ty, ParamSrc::StateField(idx)));
                        } else if txn_let_names.contains(ident) {
                            let llvm_ty = txn_let_types.get(ident).cloned().unwrap_or_else(|| "i64".to_string());
                            params.push((ident.clone(), llvm_ty.clone(), ParamSrc::LetBinding(llvm_ty)));
                        } else if let Some((const_ty, val_expr)) = self.ctx.constants.get(ident.as_str()) {
                            // 2026-07-31: Phase 3 (§8.4) — the old `== "Float" →
                            // "float"` special case is redundant: llvm_type(Float)
                            // already resolves to "float" via the casting graph,
                            // so the const param LLVM type is derived uniformly.
                            let llvm_ty = self.llvm_type(const_ty);
                            params.push((ident.clone(), llvm_ty, ParamSrc::Constant(val_expr.clone(), const_ty.clone())));
                        } else {
                            can_outline_all = false;
                            break;
                        }
                    }
                    if !can_outline_all { break; }
                    if !params.is_empty() {
                        param_sets.push(params);
                    }
                }
                if !can_outline_all { Vec::new() }
                else {
                    ffi_guard_indices.into_iter().zip(param_sets.into_iter())
                        .enumerate()
                        .map(|(ci, (ri, params))| {
                            (ri, format!("txn_{}_cold_{}", name, ci), params)
                        })
                        .collect()
                }
            };
            let local_outlined = !outlined_info.is_empty();
            let mut local_txn_attr = if local_outlined { "#11".to_string() } else { txn_attr.to_string() };
            // 2026-07-28: Dense matrix detection — if #11 would be selected but
            // the txn has dense cross-field float computation (cross-per-field > 8),
            // force #0 = memory(readwrite) instead. LLVM's auto-vectorizer creates
            // expensive wide vectors (<12 x float>) for dense matrices that cause
            // register spilling — the kalman 3.5x regression.
            // 2026-07-31: Phase 2 — the measurement is computed once in the
            // frontend (src/analysis/density.rs, plan §7.1) instead of re-counting
            // cross ops here. The frontend version FIXES the old metric's gap:
            // count_cross_float_ops_in_expr ignored its _all_idents set, so int-only
            // counter arithmetic inflated the count. Only txns with dense cross-field
            // FLOAT computation downgrade.
            // 2026-07-31: Phase 3 (§8.1) — the threshold comes from
            // config/targets.dbvl `dense_compute_density` (default 4.0).
            if local_txn_attr == "#11" {
                if let Some(d) = self.ctx.density.get(name) {
                    let threshold = crate::config_tuning::target_settings_for(&self.ctx.target_triple)
                        .dense_compute_density;
                    if d.float_idents > 4 && d.per_field > threshold {
                        local_txn_attr = "#0".to_string();
                    }
                }
            }

            // Emit the txn function. An accel body's CPU path is emitted as
            // @txn_<name>_cpu; @txn_<name> becomes the dispatch wrapper the
            // reactor calls (2026-08-06, accel plan).
            let is_accel = self.accel_kernel_idx.contains_key(name);
            let cpu_name = if is_accel {
                format!("txn_{}_cpu", name)
            } else {
                format!("txn_{}", name)
            };
            // 2026-09-07: reset cur_block for the fresh function.
            self.fun.cur_block = None;
            writeln!(out, "define void @{}({}) local_unnamed_addr {}{}{} {{", cpu_name, self.ctx.state_ptr_param, local_txn_attr, alwaysinline, meta_attrs).ok();
            writeln!(out, "  entry:").ok();
            self.fun.ssa_old_int_regs.clear();
            self.fun.ssa_old_float_regs.clear();
            self.fun.clear_locals();
            self.fun.terminated = false;
            self.fun.in_callable_txn = false;
            self.fun.returns_i64 = false;
            self.fun.fn_ret_ty = "void".to_string();
            self.emit_arena_init(out, "  ");
            if !matches!(txn.contract.pre_condition, Expr::Bool(true)) {
                self.emit_precondition_check(out, &txn.contract.pre_condition, "  ");
            }
            // Emission loop with guard substitution
            for (ri, s) in reordered.iter().enumerate() {
                if self.fun.terminated { break; }
                if let Some((_, cold_name, params)) = outlined_info.iter().find(|(idx, _, _)| *idx == ri) {
                    if let Statement::Guarded(cond, _) = s {
                        let cond_reg = self.emit_expr(out, cond, "  ");
                        let label_n = self.fun.txn_counter;
                        self.fun.txn_counter += 1;
                        let then_lbl = format!("guard.then{}", label_n);
                        let end_lbl = format!("guard.end{}", label_n);
                        let cond_i1 = if cond_reg.ty == Type::bool_() {
                            let b = self.fun.gen_reg();
                            writeln!(out, "  {} = trunc i8 {} to i1", b, cond_reg.name).ok();
                            b
                        } else {
                            cond_reg.name.clone()
                        };
                        // 2026-07-27: Compute !prof branch weights from guard
                        // condition and postcondition bound. For a modulo condition
                        // like `count % N == C` with postcondition `[count == total]`,
                        // the guard fires `ceil(total / N)` times out of `total`.
                        let prof_meta = {
                            // 2026-07-28: Extended !prof computation — uses transition graph
                            // data (bounded_pre, increments) and iter_bounds for more precise
                            // weights. Falls back to postcondition + modulo analysis.
                            let txn_name = name;
                            let bounded_pre = self.ctx.transition_graph.as_ref()
                                .and_then(|tg| tg.nodes.iter().find(|n| n.name == txn_name))
                                .and_then(|n| n.bounded_pre.as_ref());
                            let increment = self.ctx.transition_graph.as_ref()
                                .and_then(|tg| tg.nodes.iter().find(|n| n.name == txn_name))
                                .and_then(|n| n.increments.as_ref());
                            let iter_bound = self.ctx.iter_bounds.get(txn_name).copied();

                            // Helper: compute and scale !prof weights
                            // 2026-07-31: Phase 3 (§8.3) — the cap is i32-range
                            // normalization, not a tunable: LLVM branch_weights
                            // are i32, and the cap is a power of two near
                            // i32::MAX / 2 (2^30). Scaling keeps the SUM ≤ cap so
                            // every weight fits i32 with no overflow and minimal
                            // ratio rounding — more precise than the old 1000 cap.
                            let scale_weights = |taken: u64, not_taken: u64| -> Option<(u32, u32)> {
                                let max_w = 1u64 << 30;
                                let total = taken + not_taken;
                                if taken == 0 || not_taken == 0 { return None; }
                                let (wt, wn) = if total <= max_w {
                                    (taken as u32, not_taken as u32)
                                } else {
                                    let ratio = total as f64 / max_w as f64;
                                    ((taken as f64 / ratio).ceil() as u32,
                                     (not_taken as f64 / ratio).ceil() as u32)
                                };
                                if wt > 0 && wn > 0 { Some((wt, wn)) } else { None }
                            };
                            let format_weights = |wt: u32, wn: u32| -> String {
                                format!(", !prof !{{!\"branch_weights\", i32 {}, i32 {}}}", wt, wn)
                            };

                            // Strategy 1: Use transition graph bounded_pre + increments
                            if let (Some(bp), Some(inc), Some(ib)) = (bounded_pre, increment, iter_bound) {
                                let step = inc.delta.unsigned_abs() as u64;
                                // Match guard condition referencing the induction variable
                                let guard_var = match cond {
                                    Expr::BinaryOp(BinaryOpKind::Eq, lhs, _) => {
                                        get_induction_var_name(lhs, &bp.var)
                                    }
                                    _ => None,
                                };
                                if guard_var.is_some() && step > 0 && ib > 0 {
                                    // Extract divisor from modulo: var % N == C
                                    let mod_n = match cond {
                                        Expr::BinaryOp(BinaryOpKind::Eq, lhs, _) => match lhs.as_ref() {
                                            Expr::BinaryOp(BinaryOpKind::Mod, _, d) => match d.as_ref() {
                                                Expr::Decimal(n) if *n > 0 => Some(*n as u64),
                                                _ => None,
                                            },
                                            _ => None,
                                        },
                                        _ => None,
                                    };
                                    if let Some(mn) = mod_n {
                                        // Modulo pattern: count % N == C
                                        let taken = ib / (step * mn);
                                        let not_taken = ib.saturating_sub(taken);
                                        if let Some((wt, wn)) = scale_weights(taken, not_taken) {
                                            format_weights(wt, wn)
                                        } else { String::new() }
                                    } else {
                                        // General induction variable constraint:
                                        // Use iter_bound / step as total, guard fires 1 per step
                                        let taken = ib / step;
                                        let not_taken = ib.saturating_sub(taken);
                                        if let Some((wt, wn)) = scale_weights(taken, not_taken) {
                                            format_weights(wt, wn)
                                        } else { String::new() }
                                    }
                                } else { String::new() }
                            } else {
                                // Strategy 2: Postcondition + modulo (original Phase 2)
                                // Extract postcondition bound: [x == N]
                                let post_bound: Option<i64> = (|| {
                                    let (lhs, rhs) = match &txn.contract.post_condition {
                                        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => (l.as_ref(), r.as_ref()),
                                        _ => return None,
                                    };
                                    let n = match rhs {
                                        Expr::Decimal(n) => Some(*n),
                                        Expr::Identifier(id) => self.ctx.constants.get(id.as_str())
                                            .and_then(|(_, e)| if let Expr::Decimal(v) = e { Some(*v) } else { None }),
                                        _ => None,
                                    };
                                    n.filter(|&n| n > 0)
                                })();
                                // Extract modulo divisor from guard condition: x % N == C
                                let mod_div: Option<i64> = (|| {
                                    let lhs = match cond {
                                        Expr::BinaryOp(BinaryOpKind::Eq, l, _) => l.as_ref(),
                                        _ => return None,
                                    };
                                    match lhs {
                                        Expr::BinaryOp(BinaryOpKind::Mod, _, d) => match d.as_ref() {
                                            Expr::Decimal(n) if *n > 0 => Some(*n),
                                            _ => None,
                                        },
                                        _ => None,
                                    }
                                })();
                                match (post_bound, mod_div) {
                                    (Some(bound), Some(mod_n)) => {
                                    let taken = (bound as f64 / mod_n as f64).ceil() as u64;
                                    let not_taken = (bound as u64).saturating_sub(taken);
                                    // 2026-07-31: Phase 3 (§8.3) — i32-range
                                    // normalization cap, power of two near
                                    // i32::MAX / 2 (see scale_weights above).
                                    let max_w = 1u64 << 30;
                                    let total = taken + not_taken;
                                    let (wt, wn) = if total <= max_w {
                                        (taken as u32, not_taken as u32)
                                    } else {
                                        let ratio = total as f64 / max_w as f64;
                                        ((taken as f64 / ratio).ceil() as u32,
                                         (not_taken as f64 / ratio).ceil() as u32)
                                    };
                                    // Only emit if weights are meaningful
                                    if wt > 0 && wn > 0 {
                                        // First weight = true branch (guard fires, cold),
                                        // second weight = false branch (guard doesn't fire, hot)
                                        format!(", !prof !{{!\"branch_weights\", i32 {}, i32 {}}}", wt, wn)
                                    } else { String::new() }
                                }
                                _ => String::new(),
                            }
                        }
                        };
                        writeln!(out, "  br i1 {}, label %{}, label %{}{}",
                            cond_i1, then_lbl, end_lbl, prof_meta).ok();
                        writeln!(out, "  {}:", then_lbl).ok();
                        // Emit load for each param — handles three cases:
                        // StateField: GEP+load from %state
                        // LetBinding: lookup register from let_bindings table
                        // Constant: emit literal value (Expr::Decimal or Expr::Float)
                        let mut param_regs: Vec<String> = Vec::new();
                        for (p_name, llvm_ty, src) in params {
                            match src {
                                ParamSrc::StateField(idx) => {
                                    let gep_reg = self.fun.gen_reg();
                                    let load_reg = self.fun.gen_reg();
                                    writeln!(out, "    {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}", gep_reg, idx).ok();
                                    writeln!(out, "    {} = load {}, ptr {}", load_reg, llvm_ty, gep_reg).ok();
                                    param_regs.push(load_reg);
                                }
                                ParamSrc::LetBinding(_) => {
                                    if let Some(reg) = self.fun.let_bindings.get(p_name) {
                                        param_regs.push(reg.clone());
                                    } else {
                                        // Fallback: load from state (may fail)
                                        let fallback = self.fun.gen_reg();
                                        writeln!(out, "    {} = add i64 0, 0", fallback).ok();
                                        param_regs.push(fallback);
                                    }
                                }
                                ParamSrc::Constant(val_expr, _) => {
                                    let const_reg = self.fun.gen_reg();
                                    match val_expr {
                                        Expr::Decimal(v) => {
                                            writeln!(out, "    {} = add i64 0, {}", const_reg, v).ok();
                                        }
                                        Expr::Float(v) => {
                                            let hex = crate::backend::llvm::float_to_llvm_hex(*v);
                                            writeln!(out, "    {} = bitcast i32 {} to float", const_reg, hex).ok();
                                        }
                                        _ => {
                                            writeln!(out, "    {} = add i64 0, 0", const_reg).ok();
                                        }
                                    }
                                    param_regs.push(const_reg);
                                }
                            }
                        }
                        let mut typed_args: Vec<String> = Vec::new();
                        for (fi, (_, llvm_ty, _)) in params.iter().enumerate() {
                            typed_args.push(format!("{} {}", llvm_ty, param_regs[fi]));
                        }
                        writeln!(out, "    call void @{}({})", cold_name, typed_args.join(", ")).ok();
                        writeln!(out, "    br label %{}", end_lbl).ok();
                        writeln!(out, "  {}:", end_lbl).ok();
                        self.fun.terminated = false;
                    }
                } else {
                    emit_statement(self, out, s, "  ");
                }
            }
            if !self.fun.terminated {
                // 2026-08-06 (beginprogram plan): clear the entry flag on goal.
                self.emit_beginprogram_goal_check(out, txn);
                self.emit_arena_fini(out, "  ");
            }
            // 2026-08-04 (term-termination-diagnostics): a value-form term in
            // this void txn emits `ret void` itself (terminated=true); guard
            // against emitting a second ret after it.
            if !self.fun.terminated {
                writeln!(out, "  ret void").ok();
            }
            writeln!(out, "}}").ok();

            // 2026-08-06 (accel plan): emit the dispatch wrapper for an accel
            // body (Gpu: device-availability gate; Probe: verdict global).
            if is_accel {
                self.emit_accel_dispatch_wrapper(out, name);
            }

            // Emit cold functions after the txn function
            for (ri, cold_name, fields) in &outlined_info {
                let body = match &reordered[*ri] {
                    Statement::Guarded(_, body) => body.clone(),
                    _ => continue,
                };
                let saved_let_bindings = self.fun.let_bindings.clone();
                let saved_let_binding_types = self.fun.let_binding_types.clone();
                let saved_let_original_types = self.fun.let_original_types.clone();
                let saved_reg_float_cache = self.fun.reg_float_cache.clone();
                let saved_reg_type_cache = self.fun.reg_type_cache.clone();
                self.fun.let_bindings.clear();
                self.fun.let_binding_types.clear();
                self.fun.let_original_types.clear();
                self.fun.reg_float_cache.clear();
                self.fun.reg_type_cache.clear();

                // Build param list — register with correct Briev type so that
                // emit_statement uses pointer semantics (inttoptr+GEP+load) rather
                // than vector semantics (extractelement) for Ptr<Int> fields.
                let field_names: Vec<String> = fields.iter().map(|(f, _, _)| f.clone()).collect();
                let cp_names: Vec<String> = fields.iter().map(|(f_name, _, _)| format!("__cp_{}", f_name)).collect();
                let mut param_sig: Vec<String> = Vec::new();
                for (fi, (f_name, llvm_ty, src)) in fields.iter().enumerate() {
                    param_sig.push(format!("{} %{}", llvm_ty, cp_names[fi]));
                    let briev_ty = match src {
                        ParamSrc::StateField(idx) => {
                            self.ctx.field_briev_types.get(*idx).cloned().unwrap_or(Type::int())
                        }
                        ParamSrc::LetBinding(llvm_ty) => {
                            if llvm_ty == "float" { Type::float() } else { Type::int() }
                        }
                        ParamSrc::Constant(_, briev_ty) => briev_ty.clone(),
                    };
                    self.fun.let_bindings.insert(cp_names[fi].clone(), format!("%{}", cp_names[fi]));
                    self.fun.let_binding_types.insert(cp_names[fi].clone(), briev_ty.clone());
                    self.fun.let_original_types.insert(cp_names[fi].clone(), briev_ty);
                }
                writeln!(out, "define void @{}({}) local_unnamed_addr #0 {{", cold_name, param_sig.join(", ")).ok();

                // Rewrite guard body: replace ident references with param names
                self.fun.terminated = false;
                for stmt in &body {
                    if self.fun.terminated { break; }
                    let rewritten = Self::rewrite_stmt_idents(stmt, &field_names, &cp_names);
                    emit_statement(self, out, &rewritten, "  ");
                }
                if !self.fun.terminated {
                    writeln!(out, "  ret void").ok();
                }
                writeln!(out, "}}").ok();
                writeln!(out).ok();

                self.fun.let_bindings = saved_let_bindings;
                self.fun.let_binding_types = saved_let_binding_types;
                self.fun.let_original_types = saved_let_original_types;
                self.fun.reg_float_cache = saved_reg_float_cache;
                self.fun.reg_type_cache = saved_reg_type_cache;
            }
        }
    }

    /// 2026-08-06 (accel plan): emit `@txn_<name>` as the dispatch wrapper for
    /// an accel body. The reactor calls `@txn_<name>` by name at every
    /// dispatch site (loop_engine counter/ssa paths), so the wrapper — lazy
    /// registry init + device/verdict gate → `briev_accel_launch`, else the CPU
    /// body `@txn_<name>_cpu` — is picked up automatically.
    pub(super) fn emit_accel_dispatch_wrapper(&mut self, out: &mut String, name: &str) {
        let Some(&idx) = self.accel_kernel_idx.get(name) else { return; };
        // Copy the decision data up front — the emission methods below borrow
        // `self` mutably (gen_reg, emit_state_gep, emit_work_item_count).
        let forced = self.accel_entries[name].forced;
        let n_kernels = self.accel_kernel_idx.len();
        // 2026-09-07: reset cur_block for the fresh function.
        self.fun.cur_block = None;
        writeln!(out, "define void @txn_{}({}) local_unnamed_addr {{", name, self.ctx.state_ptr_param).ok();
        writeln!(out, "entry:").ok();
        writeln!(out, "  %ready = load i32, ptr @briev_accel_ready").ok();
        writeln!(out, "  %need = icmp eq i32 %ready, 0").ok();
        writeln!(out, "  br i1 %need, label %accel_init, label %accel_gate").ok();
        writeln!(out, "accel_init:").ok();
        writeln!(out, "  %ok = call i32 @briev_accel_init(ptr @briev_accel_descs, i32 {})", n_kernels).ok();
        writeln!(out, "  store i32 1, ptr @briev_accel_ready").ok();
        // 2026-08-06 (Design A / probe): a Probe-decision body runs the
        // auto-tuning probe once here (device + workload measurement, output
        // equality gate), committing its own verdict global. Gpu-decision
        // bodies skip the probe (static crossover already decided).
        if !forced {
            writeln!(out, "  call void @briev_accel_run_probe_{}(ptr %state)", name).ok();
        }
        writeln!(out, "  br label %accel_gate").ok();
        writeln!(out, "accel_gate:").ok();
        // Gpu decision → gate on device availability. Probe → gate on the
        // committed verdict global (set by briev_accel_run_probe_<name>).
        if forced {
            writeln!(out, "  %dev = call i32 @briev_accel_available()").ok();
        } else {
            writeln!(out, "  %dev = load i32, ptr @briev_accel_verdict_{}", name).ok();
        }
        writeln!(out, "  %use = icmp ne i32 %dev, 0").ok();
        writeln!(out, "  br i1 %use, label %accel_gpu, label %accel_cpu").ok();
        writeln!(out, "accel_gpu:").ok();
        let work = self.emit_work_item_count(out, name);
        // 2026-09-02 (--accel-cpu-fallback N, plan 2026-09-02-gpu-portfolio-and-fallback):
        // shapes below the work-item threshold fall to the CPU loop. The
        // NVIDIA 576.99 driver nondeterministically loses stores on small
        // dispatches (BUGS.md) and a tiny grid cannot amortize the launch.
        // Const-bound shapes fold the gate at compile time; runtime-bound
        // shapes icmp+branch. Only the .bv mixed lane carries this — .abv
        // stays GPU-only by charter.
        let folded_to_cpu = matches!(
            self.ctx.accel_cpu_fallback,
            Some(threshold)
                if work.parse::<u64>().map(|lit| lit < threshold).unwrap_or(false)
        );
        if folded_to_cpu {
            writeln!(out, "  br label %accel_cpu").ok();
        } else {
            if let Some(threshold) = self.ctx.accel_cpu_fallback {
                if work.parse::<u64>().is_err() {
                    // Runtime-bound shape: gate at run time.
                    writeln!(out, "  %wg_ok = icmp uge i64 {}, {}", work, threshold).ok();
                    writeln!(out, "  br i1 %wg_ok, label %accel_dispatch, label %accel_cpu").ok();
                    writeln!(out, "accel_dispatch:").ok();
                }
            }
        // 2026-09-01 (Track B): kernel-pinned programs use the RESIDENT path
        // (push dirty scalars, dispatch, pull written fields back) instead of
        // the full-copy launch (pack ALL fields, push, pull, unpack ALL).
        if self.ctx.accel_resident_ok {
            writeln!(out, "  %r = call i32 @briev_accel_launch_resident(i32 {}, ptr %state, i64 {})", idx, work).ok();
            writeln!(out, "  %dl = call i32 @briev_accel_download_written(i32 {}, ptr %state)", idx).ok();
        } else {
            writeln!(out, "  %r = call i32 @briev_accel_launch(i32 {}, ptr %state, i64 {})", idx, work).ok();
        }
        // 2026-08-06 (Design A): the accel node is a NATIVE counted loop over
        // the counter (`[i < N]`, `i = i + 1`). One GPU dispatch covers all N
        // work-items, so fast-forward the counter to N — the loop's `i < N`
        // exit is then satisfied after this single firing, coalescing the
        // N-firing loop into one dispatch without touching loop emission.
        let counter_var = self.accel_entries[name].shape.index_var.clone();
        if let Some(&cfidx) = self.ctx.field_index_map.get(&counter_var) {
            let gp = self.emit_state_gep(out, "  ", "ff", "%state", cfidx);
            writeln!(out, "  store i64 {}, ptr {}, align 8", work, gp).ok();
        }
        writeln!(out, "  ret void").ok();
        } // folded_to_cpu else
        writeln!(out, "accel_cpu:").ok();
        writeln!(out, "  call void @txn_{}_cpu(ptr %state)", name).ok();
        // Track B coherence: the CPU body wrote HOST state — the VRAM
        // working set is stale. Clear the seed so the next resident launch
        // re-pushes ALL fields from the host.
        writeln!(out, "  call void @briev_accel_invalidate_resident()").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        // Probe-decision bodies (not forced) get the auto-tuning probe
        // functions: CPU lane, GPU lane, output-equality gate, run_probe.
        if !forced {
            self.emit_accel_probe_functions(out, name);
        }
    }

    /// The kernel's work-item count (the `[i < N]` bound) as an i64 register
    /// or constant. State scalars are loaded; literals inline; CONST bounds
    /// (2026-08-31, plan abv-gpu-by-default: `[i < N2]` with `const N2`) are
    /// inlined from the module's constant table; anything else resolves to 0
    /// (the kernel dispatch then no-ops — callers should verify the bound
    /// shape at analysis time).
    fn emit_work_item_count(&mut self, out: &mut String, name: &str) -> String {
        let entry = &self.accel_entries[name];
        match &entry.shape.count_expr {
            Some(Expr::Decimal(n)) => n.to_string(),
            Some(Expr::Identifier(f)) => {
                if let Some(&fidx) = self.ctx.field_index_map.get(f) {
                    let (reg, _) = self.emit_state_load_i64_by_idx(out, "  ", fidx);
                    return reg;
                }
                // 2026-08-31: a CONST bound is not a state field — inline its
                // literal value so the dispatch isn't silently 0 workgroups.
                if let Some((_, Expr::Decimal(n))) = self.ctx.constants.get(f) {
                    return n.to_string();
                }
                "0".to_string()
            }
            Some(expr) => {
                // 2026-08-31: a literal-valued const EXPRESSION (`const N:
                // Int = 64 * 64;` folded by the typechecker to a Decimal)
                // arrives here pre-folded; anything else is 0.
                if let Expr::Decimal(n) = expr {
                    n.to_string()
                } else {
                    "0".to_string()
                }
            }
            _ => "0".to_string(),
        }
    }

    /// Emit the auto-tuning probe functions for one Probe-decision accel body
    /// (Design A): CPU lane (run the counted loop to completion), GPU lane (one
    /// dispatch + counter fast-forward), output-equality gate (write buffers
    /// within tolerance), and the run_probe that times both lanes and commits
    /// the verdict global. The runtime (`briev_accel_probe`) manages the two
    /// lane state copies.
    pub(super) fn emit_accel_probe_functions(&mut self, out: &mut String, name: &str) {
        let Some(&idx) = self.accel_kernel_idx.get(name) else { return; };
        let entry = &self.accel_entries[name];
        let counter = entry.shape.index_var.clone();
        let Some(&cfidx) = self.ctx.field_index_map.get(&counter) else { return; };
        let state_size = self.ctx.state_size_bytes;

        // ── CPU lane: reset the counter to 0, then run the counted loop to
        //    completion (each @txn_<name>_cpu firing advances `i`).
        writeln!(out, "define void @briev_accel_probe_cpu_{}(ptr %state) {{", name).ok();
        writeln!(out, "entry:").ok();
        let cg = self.emit_state_gep(out, "  ", "pc", "%state", cfidx);
        writeln!(out, "  store i64 0, ptr {}", cg).ok();
        writeln!(out, "  br label %pc.cond").ok();
        writeln!(out, "pc.cond:").ok();
        let ic = self.fun.gen_reg();
        writeln!(out, "  {} = load i64, ptr {}", ic, cg).ok();
        let n = self.emit_work_item_count(out, name);
        let cmp = self.fun.gen_reg();
        writeln!(out, "  {} = icmp ult i64 {}, {}", cmp, ic, n).ok();
        writeln!(out, "  br i1 {}, label %pc.body, label %pc.done", cmp).ok();
        writeln!(out, "pc.body:").ok();
        writeln!(out, "  call void @txn_{}_cpu(ptr %state)", name).ok();
        writeln!(out, "  br label %pc.cond").ok();
        writeln!(out, "pc.done:").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();

        // ── GPU lane: reset the counter, one dispatch of N work-items, then
        //    fast-forward the counter so the state matches the CPU lane's.
        writeln!(out, "define void @briev_accel_probe_gpu_{}(ptr %state) {{", name).ok();
        writeln!(out, "entry:").ok();
        let cg2 = self.emit_state_gep(out, "  ", "pg", "%state", cfidx);
        writeln!(out, "  store i64 0, ptr {}", cg2).ok();
        let ng = self.emit_work_item_count(out, name);
        writeln!(out, "  %r = call i32 @briev_accel_launch(i32 {}, ptr %state, i64 {})", idx, ng).ok();
        writeln!(out, "  store i64 {}, ptr {}", ng, cg2).ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();

        // ── Output-equality gate: compare every write buffer element-wise
        //    within tolerance (Float) / exactly (Int). The gate doubles as the
        //    safety net against GPU codegen bugs (D7).
        writeln!(out, "define i8 @briev_accel_gpu_ok_{}(ptr %a, ptr %b, double %tol) {{", name).ok();
        writeln!(out, "entry:").ok();
        self.emit_probe_ok_body(out, name, cfidx);
        writeln!(out, "fail:").ok();
        writeln!(out, "  ret i8 0").ok();
        writeln!(out, "}}").ok();

        // ── run_probe: time both lanes on separate state copies via the
        //    runtime, commit the verdict global. Tunables from
        //    config/ir-lowering.dbvl (config_tuning::ir_lowering).
        let tuning = crate::config_tuning::ir_lowering();
        writeln!(out, "define void @briev_accel_run_probe_{}(ptr %state) {{", name).ok();
        writeln!(out, "entry:").ok();
        writeln!(out, "  %v = call i32 @briev_accel_probe(").ok();
        writeln!(out, "    ptr @briev_accel_probe_cpu_{}, ptr @briev_accel_probe_gpu_{},", name, name).ok();
        //    {:?} (not {:e}): LLVM IR float literals must carry a decimal
        //    point — `1e-4` parses as an integer token and fails clang/opt
        //    ("integer constant must have integer type"); Rust's Debug
        //    format prints 1e-4 as "0.0001" (round-trip shortest, always
        //    with '.').
        writeln!(out, "    ptr %state, i64 {}, i64 {}, double {:?}, double {:?},", state_size, tuning.accel_probe_k, tuning.accel_probe_tolerance, tuning.accel_probe_margin).ok();
        writeln!(out, "    ptr @briev_accel_gpu_ok_{})", name).ok();
        writeln!(out, "  store i32 %v, ptr @briev_accel_verdict_{}", name).ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
    }

    /// Emit the write-buffer comparison body of the output-equality gate.
    /// Each write buffer is checked element-wise; the first mismatch branches
    /// to the shared `fail:` label (which the caller emits after this body).
    fn emit_probe_ok_body(&mut self, out: &mut String, name: &str, counter_fidx: usize) {
        let entry = &self.accel_entries[name];
        // Collect the element checks (buffer field, element index, type) so the
        // control flow can chain them with deterministic next labels.
        let checks = Self::probe_ok_checks(
            &entry.shape,
            &self.ctx.field_index_map,
            &self.ctx.field_types,
            counter_fidx,
        );
        let n_checks = checks.len();
        for (ci, (fidx, k, elem, is_float)) in checks.into_iter().enumerate() {
            if ci > 0 {
                writeln!(out, "ok.{}:", ci).ok();
            }
            let next = if ci + 1 < n_checks {
                format!("ok.{}", ci + 1)
            } else {
                "ok.final".to_string()
            };
            let xa = self.fun.gen_reg();
            let ya = self.fun.gen_reg();
            let x = self.fun.gen_reg();
            let y = self.fun.gen_reg();
            if k == 0 && !self.ctx.field_types[fidx].starts_with('[') {
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %a, i32 0, i32 {}", xa, fidx).ok();
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %b, i32 0, i32 {}", ya, fidx).ok();
            } else {
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %a, i32 0, i32 {}, i32 {}", xa, fidx, k).ok();
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %b, i32 0, i32 {}, i32 {}", ya, fidx, k).ok();
            }
            writeln!(out, "  {} = load {}, ptr {}", x, elem, xa).ok();
            writeln!(out, "  {} = load {}, ptr {}", y, elem, ya).ok();
            self.emit_probe_cmp(
                out,
                ProbeCmpArg { elem: &elem, x: &x, y: &y, next: &next },
            );
        }
        writeln!(out, "ok.final:").ok();
        writeln!(out, "  ret i8 1").ok();
    }

    /// The output-equality gate's element checks: (field index, element index,
/// LLVM element type, is-float) for every write-buffer element, skipping the
/// work-item counter (it diverges by design — the GPU lane fast-forwards it).
/// Flat iteration keeps the gate's emission single-loop.
fn probe_ok_checks(
    shape: &crate::analysis::accel::KernelShape,
    field_index_map: &std::collections::HashMap<String, usize>,
    field_types: &[String],
    counter_fidx: usize,
) -> Vec<(usize, u64, String, bool)> {
    shape
        .write_buffers
        .iter()
        .filter_map(|buf| field_index_map.get(buf).copied())
        .filter(|&fidx| fidx != counter_fidx)
        .flat_map(|fidx| {
            let ty = &field_types[fidx];
            if let Some(rest) = ty.strip_prefix('[') {
                let mut parts = rest.split(" x ");
                let count: u64 = parts.next().and_then(|n| n.trim().parse().ok()).unwrap_or(0);
                let elem = parts.next().unwrap_or("i64").trim_end_matches(']').trim().to_string();
                let is_float = elem == "float" || elem == "double";
                (0..count.max(1))
                    .map(move |k| (fidx, k, elem.clone(), is_float))
                    .collect::<Vec<_>>()
            } else {
                let elem = ty.clone();
                let is_float = elem == "float" || elem == "double";
                vec![(fidx, 0u64, elem, is_float)]
            }
        })
        .collect()
}

    /// Emit `if !(x within tol of y) br fail, else br <next>` — Float within
    /// tolerance, Int exact equality.
    fn emit_probe_cmp(&mut self, out: &mut String, a: ProbeCmpArg<'_>) {
        let elem = a.elem;
        let x = a.x;
        let y = a.y;
        let next = a.next;
        let is_float = elem == "float" || elem == "double";
        let cmp = self.fun.gen_reg();
        if is_float {
            let tol = self.fun.gen_reg();
            let lo = self.fun.gen_reg();
            let hi = self.fun.gen_reg();
            let g1 = self.fun.gen_reg();
            let g2 = self.fun.gen_reg();
            let cast = if elem == "float" { "fptrunc double %tol to float" } else { "double %tol" };
            writeln!(out, "  {} = {}", tol, cast).ok();
            writeln!(out, "  {} = fsub {} {}, {}", lo, elem, y, tol).ok();
            writeln!(out, "  {} = fadd {} {}, {}", hi, elem, y, tol).ok();
            writeln!(out, "  {} = fcmp oge {} {}, {}", g1, elem, x, lo).ok();
            writeln!(out, "  {} = fcmp ole {} {}, {}", g2, elem, x, hi).ok();
            writeln!(out, "  {} = and i1 {}, {}", cmp, g1, g2).ok();
        } else {
            writeln!(out, "  {} = icmp eq i64 {}, {}", cmp, x, y).ok();
        }
        let not = self.fun.gen_reg();
        writeln!(out, "  {} = xor i1 {}, true", not, cmp).ok();
        writeln!(out, "  br i1 {}, label %fail, label %{}", not, next).ok();
    }

    /// Emit the beginprogram entry-loop goal check: after a firing, evaluate
    /// the postcondition; when it is met, clear `@briev_begin_<name>` so the
    /// precondition (which reads the flag) stops gating future ticks. This
    /// makes `[beginprogram && <state>]` drive a one-shot entry loop without a
    /// phase gate.
    pub(crate) fn emit_beginprogram_goal_check(&mut self, out: &mut String, txn: &crate::ast::Transaction) {
        if !Self::expr_has_beginprogram(&txn.contract.pre_condition) {
            return;
        }
        let goal = self.emit_expr(out, &txn.contract.post_condition, "  ");        let goal_i1 = if goal.ty == Type::bool_() {
            let r = self.fun.gen_reg();
            writeln!(out, "  {} = trunc i8 {} to i1", r, goal.name).ok();
            r
        } else {
            return;
        };
        let label_n = self.fun.txn_counter;
        self.fun.txn_counter += 1;
        let done_lbl = format!("begin.done{}", label_n);
        let cont_lbl = format!("begin.cont{}", label_n);
        writeln!(out, "  br i1 {}, label %{}, label %{}", goal_i1, done_lbl, cont_lbl).ok();
        writeln!(out, "{}:", done_lbl).ok();
        writeln!(out, "  store i1 false, ptr @briev_begin_{}", txn.name).ok();
        writeln!(out, "  br label %{}", cont_lbl).ok();
        writeln!(out, "{}:", cont_lbl).ok();
    }

    /// Whether an expression contains the `beginprogram` marker (an entry-loop
    /// precondition conjunct).
    pub(crate) fn expr_has_beginprogram(e: &Expr) -> bool {
        match e {
            Expr::BeginProgram => true,
            Expr::BinaryOp(BinaryOpKind::And, a, b) => {
                Self::expr_has_beginprogram(a) || Self::expr_has_beginprogram(b)
            }
            _ => false,
        }
    }

    pub(super) fn emit_callable_txn(&mut self, out: &mut String, txn: &crate::ast::Transaction, name: &str) {
        // 2026-07-27: Set txn_name for per-function arena gating.
        self.fun.txn_name = name.to_string();
        self.fun.pending_cleanup.clear();
        self.fun.let_bindings.clear();
        self.fun.let_binding_types.clear();
        self.fun.let_original_types.clear(); self.fun.reg_float_cache.clear();
        self.fun.reg_type_cache.clear();
        self.fun.expr_dedup_cache.clear();
        // 2026-07-18: Detect bounded pre-condition (e.g. x < N) — enables
        // Alloca strategy for stack-allocated temporaries within the loop.
        self.fun.is_static_bound = matches!(&txn.contract.pre_condition,
            Expr::BinaryOp(kind, left, _) if matches!(kind,
                crate::ast::BinaryOpKind::Lt | crate::ast::BinaryOpKind::Le
                    | crate::ast::BinaryOpKind::Lt | crate::ast::BinaryOpKind::Ge)
                && matches!(left.as_ref(), Expr::Identifier(_)));
        self.fun.param_slots.clear();
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();

        let has_return = if let Some(ref ot) = txn.output_type {
            match ot {
                crate::ast::OutputType::Single(ty) => !matches!(ty, Type::Void),
                crate::ast::OutputType::Tuple(ts) => !ts.is_empty(),
                _ => false,
            }
        } else {
            !txn.outputs.is_empty() && !matches!(txn.outputs.first(), Some(Type::Void))
        };
        // 2026-07-18: Use correct LLVM type for return instead of always "i64".
        let ret_llvm = if has_return {
            txn.output_type.as_ref()
                .and_then(|ot| match ot {
                    crate::ast::OutputType::Single(ty) => Some(self.llvm_ret_abi_type(ty)),
                    _ => None,
                })
                .unwrap_or_else(|| "i64".to_string())
        } else {
            "void".to_string()
        };

        // Resolve #inline / #?inline directives for callable txns.
        let inline_attr = {
            let dirs = super::directive::resolve_directives(
                &txn.modifiers,
                super::directive::DirectiveCtx::CallableTxn,
            );
            dirs.iter().find_map(|e| {
                if let super::directive::DirectiveEffect::FunctionAttribute(a) = e {
                    Some(format!(" {}", a))
                } else {
                    None
                }
            })
        };
        // 2026-07-19: Auto-inline small callable txns — few params, small body,
        // no frgn calls. Lets LLVM optimize cross-function (e.g. memcmp_loop).
        // 2026-07-31: Phase 2 — the decision is computed once in the frontend
        // (src/analysis/inline_cost.rs, plan §7.3) with a weighted body cost
        // (call=10, binop=1) instead of the `params < 8 && body < 20` statement
        // count. The `term`-statement gate is preserved (has_ffi_or_trigger_stmt
        // treats Term as ffi-like), so callable txns that return via `term` are
        // still never auto-inlined — matching prior behavior.
        let auto_inline = if inline_attr.is_none()
            && self.ctx.inline_decisions.get(name).map_or(false, |d| {
                matches!(*d, crate::analysis::inline_cost::InlineDecision::AlwaysInline)
            })
        {
            " alwaysinline"
        } else {
            ""
        };
        let inline_str = inline_attr.as_deref().unwrap_or(auto_inline);

        write!(out, "define {} @{}(", ret_llvm, name).ok();
        write!(out, "{}", self.ctx.state_ptr_param).ok();
        for (i, (n, t)) in txn.parameters.iter().enumerate() {
            // 2026-07-04: Parameter-level attributes are omitted because
            // Ptr<T> is i64 at the LLVM level (not a pointer type). LLVM
            // function-level attributes (#8) provide the important guarantees
            // (argmemonly, nofree, nosync, nounwind).
            let _ = write!(out, ", {} %arg{}", self.llvm_type(t), i);
        }
        // 2026-07-04: Use #8 (argmemonly) for callable transactions.
        // Callable txns never access @link trigger globals — they only
        // read/write through %state. argmemonly tells LLVM the function
        // only accesses memory through its pointer arguments.
        writeln!(out, ") local_unnamed_addr #8{} {{", inline_str).ok();
        writeln!(out, "  entry:").ok();

        // 2026-07-18: Use the function's return type for %result, not always i64.
        // Bool-returning functions need i8 result type to match the define i8 signature.
        // 2026-08-11 (Phase 2a2 fix): a parameterized VOID txn (e.g.
        // `txn set_name(n: String)`) emitted `alloca void` + `store void 0` —
        // invalid LLVM ("void type only allowed for function results") that
        // made llc reject the wasm module. The result slot only exists when
        // the txn actually returns a value.
        if ret_llvm != "void" {
            writeln!(out, "  %result = alloca {}, align 8", ret_llvm).ok();
            // 2026-08-03 (node bridge): a callable txn may return ptr (CStr),
            // float, or double — the zero constant must match the LLVM type or
            // opt rejects it ("integer constant must have integer type" /
            // invalid float literal).
            let init_val = match ret_llvm.as_str() {
                "ptr" => "null".to_string(),
                "float" | "double" => "0.0".to_string(),
                _ => "0".to_string(),
            };
            writeln!(out, "  store {} {}, ptr %result, align 8", ret_llvm, init_val).ok();
        }

        for (i, (n, t)) in txn.parameters.iter().enumerate() {
            let raw = format!("%arg{}", i);
            let conv: String;
            // 2026-07-01: Use universe-declared box_op for parameter boxing.
            // Same approach as emit_definition — keeps signature and boxing
            // consistent through universe data.
            let param_llvm_ty = self.llvm_type(t);
            // 2026-07-10: Phase 1 — struct params are passed by pointer (ptr).
            // Convert to i64 via ptrtoint for storage in param_slots.
            if param_llvm_ty == "ptr" {
                let ac = format!("%ac{}", i);
                writeln!(out, "  {} = ptrtoint ptr {} to i64", ac, raw).ok();
                conv = ac;
            } else if param_llvm_ty == "i64" {
                conv = raw;
            } else if self.is_boxed_type(t) {
                // 2026-07-12: box_op removed from ResolvedType — use hardcoded fallback.
                // 2026-07-31: Phase 3 (§8.4-D1) — protocol-membership boxed-type
                // detection + emit_box_value_to_i64 replacing the name match.
                let ac = format!("%ac{}", i);
                self.emit_box_value_to_i64(out, "  ", t, &raw, &ac, &format!("%ai{}", i));
                conv = ac;
            } else if param_llvm_ty.starts_with('i') {
                // 2026-08-11 (Phase 2a2 fix): a narrow integer param — `Int`
                // on wasm32 is i32 (width-aware slots, int_bits=32) but the
                // param slot is always i64. The old `conv = raw` emitted
                // `store i64 %arg0` where %arg0 was i32, which llc rejected
                // ("defined with type 'i32' but expected 'i64'"). Widen to the
                // slot width: signed Int sexts, unsigned UInt zexts.
                let ac = format!("%ac{}", i);
                if self.is_protocol_member(t, "UInt") {
                    writeln!(out, "  {} = zext {} {} to i64", ac, param_llvm_ty, raw).ok();
                } else {
                    writeln!(out, "  {} = sext {} {} to i64", ac, param_llvm_ty, raw).ok();
                }
                conv = ac;
            } else {
                conv = raw;
            }
            let slot = format!("%p{}_s", i);
            writeln!(out, "  {} = alloca i64, align 8", slot).ok();
            writeln!(out, "  store i64 {}, ptr {}, align 8", conv, slot).ok();
            self.fun.param_slots.insert(n.clone(), slot);
        }

        writeln!(out, "  br label %loop").ok();
        writeln!(out, "loop:").ok();
        // 2026-07-26: Set convergence target so [expr]; gates inside the txn
        // body branch back to loop: when their condition is false.
        self.fun.convergence_target = Some("loop".to_string());

        for (i, (n, t)) in txn.parameters.iter().enumerate() {
            let slot = format!("%p{}_s", i);
            let loaded = format!("%p{}_l{}", i, self.fun.txn_counter);
            self.fun.txn_counter += 1;
            writeln!(out, "  {} = load i64, ptr {}, align 8", loaded, slot).ok();
            // 2026-07-18: Store the SLOT (alloca) in let_bindings, not the loaded
            // register. This makes assigns to the parameter (`i = i + 1`) store
            // through the alloca correctly. Reads still load from the slot, but
            // LLVM's optimizer merges redundant loads via SROA.
            self.fun.let_bindings.insert(n.clone(), slot.clone());
            // 2026-08-11 (Phase 2a2 fix): boxed params keep their ORIGINAL
            // Briev type — the old `Type::int()` downgrade made a boxed String
            // param look like an Int, and on wasm32 (int_bits=32) `int()` maps
            // to i32, so `name = n` emitted `sext i32` on a value that is
            // genuinely i64 (the boxed address). The identifier load now unboxes
            // the i64 slot into the native representation (inttoptr for
            // String/Data, trunc for Char/Bool) — same as state-field reads.
            self.fun.let_binding_types.insert(n.clone(), t.clone());
            self.fun.let_original_types.insert(n.clone(), t.clone());
        }

        // 2026-07-20: Set fn_ret_ty so term codegen stores with the correct type
        // (Bool → i8, Int → i64, etc.) instead of whatever the previous function left.
        self.fun.fn_ret_ty = ret_llvm.clone();
        self.fun.callable_txn_result = Some("%result".to_string());
        self.fun.callable_txn_post_label = Some("post".to_string());
        self.fun.in_callable_txn = true;
        self.fun.txn_counter = 0;
        self.fun.terminated = false;
        self.fun.returns_i64 = has_return;

        if !matches!(txn.contract.pre_condition, Expr::Bool(true)) {
            let cond = self.emit_expr(out, &txn.contract.pre_condition, "  ");
            let i1 = format!("%pc{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            if cond.ty == Type::bool_() {
                writeln!(out, "  {} = trunc i8 {} to i1", i1, cond).ok();
            } else {
                writeln!(out, "  {} = icmp ne i64 {}, 0", i1, cond).ok();
            }
            writeln!(out, "  br i1 {}, label %body, label %done", i1).ok();
        } else {
            writeln!(out, "  br label %body").ok();
        }

        writeln!(out, "body:").ok();

        for s in &txn.body {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }

        // Foreign destructor cleanup: emit OnExit calls before loop exit
        self.emit_on_exit_cleanup(out, "  ");

        if !self.fun.terminated {
            writeln!(out, "  br label %post").ok();
        }
        writeln!(out, "post:").ok();
        writeln!(out, "  br label %loop").ok();

        writeln!(out, "done:").ok();
        if has_return {
            let ret = format!("%ret{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            writeln!(out, "  {} = load {}, ptr %result, align 8", ret, ret_llvm).ok();
            writeln!(out, "  ret {} {}", ret_llvm, ret).ok();
        } else {
            writeln!(out, "  ret void").ok();
        }
        writeln!(out, "}}").ok();

        self.fun.callable_txn_result = None;
        self.fun.callable_txn_post_label = None;
        self.fun.in_callable_txn = false;
        self.fun.param_slots.clear();
    }

    //
    // WHY both br...unreachable AND @llvm.assume / !range are needed:
    //   Two distinct correctness requirements: (1) If the precondition is false,
    //   the program must stop — unreachable tells LLVM the false path is dead
    //   and enables fold elimination. (2) If the precondition is true, LLVM must
    //   know the bound so it can optimize subsequent loads/stores. The
    //   br...unreachable establishes the contract violation as UB (LLVM can DCE
    //   the entire tick). The assumption/range gives the optimizer a known bound.
    //
    // WHY !range metadata instead of @llvm.assume:
    //   @llvm.assume is a control-flow barrier — it prevents LLVM from moving
    //   instructions across it, which blocks GVN and LICM. !range metadata on a
    //   load has no such barrier: GVN eliminates the redundant load, LLVM's
    //   ValueTracking propagates the range, and passes like LICM are not blocked.
    //   But !range only works for the pattern "x = load before check; check x < N;
    //   use x" — you need a load to attach the metadata to, hence the re-load.
    //
    // WHY only Expr::Lt(x, Integer(N)) gets the fast path:
    //   !range metadata natively expresses "value in [lo, hi)" — a single
    //   lt-against-constant maps directly. Any other pattern (le, gt, ge,
    //   compound and/or) cannot be represented as a single !range entry.
    //   Complex patterns fall back to @llvm.assume, which is correct (the
    //   optimizer still gets the info) but slightly slower (barrier cost).
    pub(super) fn emit_precondition_check(&mut self, out: &mut String, pre: &Expr, indent: &str) {
        let cond = self.emit_expr(out, pre, indent);
        let i1 = format!("%pi{}", self.fun.txn_counter); self.fun.txn_counter += 1;
        if cond.ty == Type::bool_() {
            // 2026-07-14: bool is i8 — trunc to i1 for br/assume
            writeln!(out, "{}{} = trunc i8 {} to i1", indent, i1, cond).ok();
        } else {
            writeln!(out, "{}{} = icmp ne i64 {}, 0", indent, i1, cond).ok();
        }
        let panic_l = format!("pp{}", self.fun.txn_counter); self.fun.txn_counter += 1;
        let safe_l = format!("ps{}", self.fun.txn_counter); self.fun.txn_counter += 1;
        writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, i1, safe_l, panic_l).ok();
        writeln!(out, "{}{}:", indent, panic_l).ok();
        writeln!(out, "{}  unreachable", indent).ok();
        writeln!(out, "{}{}:", indent, safe_l).ok();
        // Replace @llvm.assume with !range metadata for simple patterns.
        // Pattern: [x < N] on a state field known to ssa_old_int_regs.
        // We emit a re-load of x with !range { 0, N } — the extra load is
        // GVN-eliminated if the bound is already provable by LLVM.
        match pre {
            Expr::BinaryOp(BinaryOpKind::Lt, lhs, rhs) if matches!(rhs.as_ref(), Expr::Decimal(_)) => {
                // 2026-07-04: Unwrap Cast(Identifier, Int) for Ptr<T> fields.
                // Ptr<T> fields use "ptr_field as Int" in precondition contracts
                // (e.g., [ptr as Int >= BASE && ptr as Int < END]). The Cast
                // wrapper must be unwrapped to find the state field name.
                let field_name = match lhs.as_ref() {
                    // 2026-07-31: Phase 3 (§8.4) — cast-to-Int via the canonical
                    // Type::int() primitive instead of the type-name string.
                    Expr::Cast(inner, t) if *t == Type::int() => match inner.as_ref() {
                        Expr::Identifier(n) => Some(n.clone()),
                        _ => None,
                    },
                    Expr::Identifier(name) => Some(name.clone()),
                    _ => None,
                };
                if let Some(ref name) = field_name {
                    if let Some(&idx) = self.ctx.field_index_map.get(name.as_str()) {
                        let bound = if let Expr::Decimal(b) = rhs.as_ref() { *b } else { 0 };
                        let gep = format!("%prg{}", self.fun.txn_counter); self.fun.txn_counter += 1;
                        let ty = self.ctx.field_types[idx].clone();
                        // 2026-08-10: range metadata must match the load width —
                        // flexible Int slots are i{int_bits} (i32 wasm32). The
                        // old hardcoded `i64` range on an i32 load fails llc's
                        // verifier. A bound that doesn't fit the width (e.g.
                        // 300 in i8) is clamped to the width's max (the load
                        // physically can't exceed it).
                        // 2026-09-02 (plan fundamental-parent-membership):
                        // range metadata is INTEGER-only — float storage
                        // ("half"/"float"/"double") skips the range reload and
                        // falls back to @llvm.assume (the old unwrap_or(64)
                        // emitted malformed `!{ half ... }` nodes once Float16
                        // slots resolved natively).
                        let bits: Option<u32> = ty.strip_prefix('i')
                            .and_then(|s| s.parse().ok());
                        let Some(bits) = bits else {
                            writeln!(out, "{}call void @llvm.assume(i1 {})", indent, i1).ok();
                            return;
                        };
                        let max_signed = if bits >= 64 { i64::MAX } else { (1i64 << (bits - 1)) - 1 };
                        let rhi = bound.min(max_signed);
                        let idx_val = idx;
                        let gep = self.emit_state_gep(out, indent, "prg", "%state", idx_val);
                        let rl = format!("%prl{}", self.fun.txn_counter); self.fun.txn_counter += 1;
                        let tn = crate::backend::llvm::tbaa_node(&ty, self.ctx.type_universe.as_ref());
                        // LLVM range syntax requires TYPED bounds in every
                        // width (`!range !{ i64 0, i64 100 }`). The untyped
                        // `!{ 0, 100 }` short form for i64 was accepted by
                        // older LLVM but is rejected by clang/opt 18+
                        // (2026-08-13, found via tests/llvm_compile_test.sh on
                        // the bounded-counter fixture).
                        let range = format!("{{ {} {}, {} {} }}", ty, 0i64, ty, rhi);
                        writeln!(out, "{}{} = load {}, ptr {}, align {}, !tbaa !{}, !range !{}",
                            indent, rl, ty, gep, self.align_of(&ty), tn, range).ok();
                    } else {
                        writeln!(out, "{}call void @llvm.assume(i1 {})", indent, i1).ok();
                    }
                } else {
                    writeln!(out, "{}call void @llvm.assume(i1 {})", indent, i1).ok();
                }
            }
            _ => {
                writeln!(out, "{}call void @llvm.assume(i1 {})", indent, i1).ok();
            }
        }
    }

    //
    // WHY preconditions are extracted into separate @pre_* functions:
    //   The fast-path registry (try_projection_fast_path) and the main dispatch
    //   loop both need to check the same precondition before deciding whether to
    //   fire a txn. Extracting it into @pre_*(ptr) avoids duplicating the
    //   check IR across 7+ dispatch paths (folded, SSA, reactor, parallel, etc.).
    //   LLVM will inline @pre_* into its single caller (alwaysinline), so there
    //   is zero runtime cost — the extraction is purely an IR-size optimization
    //   during codegen, not a runtime abstraction.
    //
    // WHY ptr noalias nocapture on ptr:
    //   noalias tells LLVM that no other pointer aliases %state during @pre_*'s
    //   execution — enables load/store reordering and redundant load elimination
    //   across the call boundary. nocapture means @pre_* does not store %state
    //   in a global or return it, which lets LLVM's -mem2reg promote stack
    //   allocas that would otherwise escape to the @pre_* call.
    pub(super) fn emit_pre_function(&mut self, out: &mut String, txn: &crate::ast::Transaction, name: &str) {
        if matches!(txn.contract.pre_condition, Expr::Bool(true)) { return; }
        // 2026-08-06 (beginprogram plan): the precondition may read the node's
        // `@briev_begin_<name>` entry flag — set the txn name so
        // `Expr::BeginProgram` resolves it.
        self.fun.txn_name = name.to_string();
        // 2026-07-04: Use #7 (memory(readonly)) for @pre_* functions.
        // Precondition expressions never write to %State — they only read
        // state fields via GEP+load. readonly tells LLVM the function has
        // no memory writes, enabling CSE of redundant pre_ calls and load
        // hoisting past precondition checks.
        // Other paths: #0 for definitions and callable txns (they may read
        // and write through %state), #2 for reactor_tick (always writes
        // the state copy), #3 for @main (writes through reactor tick loop).
        // 2026-07-04: Use #10 (argmemonly + readonly) for @pre_*.
        // Precondition functions never write to %State and never access
        // @link trigger globals. argmemonly + readonly is the tightest
        // constraint — tells LLVM the function only reads memory through
        // its pointer arguments.
        writeln!(out, "define internal i8 @pre_{}({}) #10 {{", name, self.ctx.state_ptr_param).ok();
        writeln!(out, "  entry:").ok();
        self.fun.txn_counter = 0;
        self.fun.clear_locals();
        // 2026-06-27: Clear ssa_old int/float regs so identifier lookups fall
        // through to GEP+load from %state. Without this, stale entries from a
        // prior emit (main function) produce forward references to registers
        // not defined in this function (precompute_sum: %t28 undefined).
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        let cond = self.emit_expr(out, &txn.contract.pre_condition, "  ");
        if cond.ty == Type::bool_() {
            writeln!(out, "  ret i8 {}", cond).ok();
        } else {
            let i1 = format!("%ri{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            let i8_reg = format!("%r8{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            writeln!(out, "  {} = icmp ne i64 {}, 0", i1, cond).ok();
            writeln!(out, "  {} = zext i1 {} to i8", i8_reg, i1).ok();
            writeln!(out, "  ret i8 {}", i8_reg).ok();
        }
        writeln!(out, "}}").ok();
    }

    //
    // WHY async bodies have their own function with noalias nocapture ptr:
    //   Async dispatch spawns concurrent evaluation of multiple txns. Each async
    //   task operates on the same ptr but with thread-level interleaving
    //   guarantees (barriers between reads and writes). Giving each async body
    //   its own LLVM function with noalias nocapture per-task allows ThreadSanitizer
    //   and LLVM's alias analysis to reason about independent regions within %State.
    //   Without the separate function, the inlined barrier call sites would appear
    //   as unstructured control flow that LLVM cannot analyze for race conditions.
    //
    // WHY the pre-check + body structure mirrors the sequential path:
    //   Async txns have the same contract semantics as sequential ones — the
    //   precondition must be checked (contracts are not a "correctness tax"),
    //   and the body must execute atomically with respect to the pre-check.
    //   Mirroring the sequential emit ensures identical semantics under both
    //   dispatch strategies. The only difference is the function boundary, which
    //   enables the async runtime to call each body independently.
    pub(super) fn emit_async_body(&mut self, out: &mut String, txn: &crate::ast::Transaction, name: &str) {
        // 2026-08-06 (beginprogram plan): the precondition may read the node's
        // `@briev_begin_<name>` entry flag — bind the txn name for the body's
        // precondition check.
        self.fun.txn_name = name.to_string();
        let async_name = format!("async_body_{}", name);
        let async_attr = "#0".to_string();
        writeln!(out, "define void @{}({}) local_unnamed_addr {} {{", async_name, self.ctx.state_ptr_param, async_attr).ok();
        writeln!(out, "  entry:").ok();
        self.fun.txn_counter = 0;
        self.fun.clear_locals();
        // 2026-06-27: Clear ssa_old int/float regs so identifier lookups fall
        // through to GEP+load from %state (same rationale as emit_pre_function).
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        let cond = self.emit_expr(out, &txn.contract.pre_condition, "  ");
        let i1 = if cond.ty == Type::bool_() {
            let b = format!("%ric{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            writeln!(out, "  {} = trunc i8 {} to i1", b, cond).ok();
            b
        } else {
            let i1 = format!("%ri{}", self.fun.txn_counter); self.fun.txn_counter += 1;
            writeln!(out, "  {} = icmp ne i64 {}, 0", i1, cond).ok();
            i1
        };
        let txn_fire_l = format!("txn_fire_{}", self.fun.txn_counter + 1);
        writeln!(out, "  br i1 {}, label %{}, label %{}_done", i1, txn_fire_l, async_name).ok();
        writeln!(out, "{}:", txn_fire_l).ok();
        self.fun.terminated = false;
        self.fun.returns_i64 = false;
            self.fun.fn_ret_ty = "void".to_string();
        // 2026-08-16 (multi-node internal fold, Direction 3): an async worker
        // whose whole bounded pass is folded into @txn_<name> (a noinline
        // countdown) just calls it once per pass instead of inlining the body.
        if self.ctx.internal_fold_txns.contains(name) {
            writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
            writeln!(out, "  br label %{}_done", async_name).ok();
            writeln!(out, "{}_done:", async_name).ok();
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
            return;
        }
        // 2026-08-31 (plan abv-gpu-by-default): an ACCEL kernel body must go
        // through its dispatch wrapper (@txn_<name> — lazy runtime init +
        // device/verdict gate + launch + counter fast-forward), NOT be
        // inlined here. The async path (2026-08-26) postdates the accel work
        // and silently bypassed the wrapper — programs ran CPU-only with the
        // wrapper functions dead in the module.
        if self.accel_kernel_idx.contains_key(name) {
            writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
            writeln!(out, "  br label %{}_done", async_name).ok();
            writeln!(out, "{}_done:", async_name).ok();
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
            return;
        }
        for s in &txn.body {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }
        // 2026-08-04 (term-termination-diagnostics): REWRITTEN from the
        // 2026-07-19 "always branch" version. The void value-form term now
        // emits a real terminator (`ret void` in this void async fn), so this
        // convergence branch is emitted only when the body did NOT terminate.
        // The 2026-07-19 version emitted an unconditional br that left the
        // block dangling whenever the body ended in a terminator.
        if !self.fun.terminated {
            writeln!(out, "  br label %{}_done", async_name).ok();
        }
        writeln!(out, "{}_done:", async_name).ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
    }

    //
    // WHY bodies are concatenated (not analyzed for dependencies):
    //   Fused txns come from the fusion pass, which has already proven that txn A
    //   and txn B have non-conflicting read/write sets (their union is conflict-
    //   free). Concatenation is correct because the fusion analysis already did the
    //   dependency work — re-analyzing at emit time would duplicate that analysis
    //   and risk desynchronizing with the fusion pass. The fusion pass guarantees
    //   no false dependencies between A's state mutations and B's, so straight-line
    //   concatenation is safe.
    //
    // WHY terminators (term/term!/escape) are filtered out:
    //   In a fused pair, A's terminator would prevent B from executing. The fusion
    //   analysis verified that A cannot terminate before B runs (both preconditions
    //   must be satisfied simultaneously), so A's terminator is dead code in the
    //   fused context. Filtering it avoids emitting unreachable IR that would
    //   confuse LLVM's control-flow analysis (specifically the structurizercfg pass).
    //
    // WHY a single ptr is shared:
    //   A and B operate on the same %State struct. Creating separate %State allocas
    //   would require merging them after both bodies execute, which would need
    //   explicit memcpy — defeating the purpose of fusion by doubling memory
    //   traffic. A single pointer is correct because the fusion pass guaranteed
    //   that A and B do not conflict on any state field.
    pub(super) fn emit_fused(&mut self, out: &mut String, a: &crate::ast::Transaction, b: &crate::ast::Transaction, name: &str) {
        let body_a: Vec<Statement> = a.body.iter()
            .filter(|s| !matches!(s, Statement::Term(..) | Statement::EndProgram(..) | Statement::Rollback(_)))
            .cloned().collect();
        let combined: Vec<Statement> = body_a.into_iter().chain(b.body.iter().cloned()).collect();
        let fused_attr = "#0";
        writeln!(out, "define void @{}(ptr noalias nocapture align 8 %state) local_unnamed_addr {} {{", name, fused_attr).ok();
        writeln!(out, "  entry:").ok();
        self.fun.txn_counter = 0; self.fun.let_bindings.clear(); self.fun.let_binding_types.clear(); self.fun.let_original_types.clear(); self.fun.reg_float_cache.clear(); self.fun.reg_type_cache.clear(); self.fun.terminated = false; self.fun.returns_i64 = false;
            self.fun.fn_ret_ty = "void".to_string();
        for s in &combined {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }
        if !self.fun.terminated { writeln!(out, "  ret void").ok(); }
        writeln!(out, "}}").ok();
    }

    pub(super) fn emit_fused_composed(&mut self, out: &mut String, body: &[Statement], name: &str) {
        let fused_attr = "#0";
        writeln!(out, "define void @{}({}) local_unnamed_addr {} {{", name, self.ctx.state_ptr_param, fused_attr).ok();
        writeln!(out, "  entry:").ok();
        self.fun.txn_counter = 0; self.fun.let_bindings.clear(); self.fun.let_binding_types.clear(); self.fun.let_original_types.clear(); self.fun.reg_float_cache.clear(); self.fun.reg_type_cache.clear(); self.fun.terminated = false; self.fun.returns_i64 = false;
            self.fun.fn_ret_ty = "void".to_string();
        for s in body {
            if self.fun.terminated { break; }
            emit_statement(self, out, s, "  ");
        }
        if !self.fun.terminated { writeln!(out, "  ret void").ok(); }
        writeln!(out, "}}").ok();
    }
}

/// Map a Briev signal name (e.g. "SIGWINCH", "SIGINT") to its POSIX number.
fn sig_number(name: &str) -> i32 {
    match name {
        "SIGHUP" => 1,
        "SIGINT" => 2,
        "SIGQUIT" => 3,
        "SIGILL" => 4,
        "SIGTRAP" => 5,
        "SIGABRT" => 6,
        "SIGBUS" => 7,
        "SIGFPE" => 8,
        "SIGKILL" => 9,
        "SIGUSR1" => 10,
        "SIGSEGV" => 11,
        "SIGUSR2" => 12,
        "SIGPIPE" => 13,
        "SIGALRM" => 14,
        "SIGTERM" => 15,
        "SIGSTKFLT" => 16,
        "SIGCHLD" => 17,
        "SIGCONT" => 18,
        "SIGSTOP" => 19,
        "SIGTSTP" => 20,
        "SIGTTIN" => 21,
        "SIGTTOU" => 22,
        "SIGURG" => 23,
        "SIGXCPU" => 24,
        "SIGXFSZ" => 25,
        "SIGVTALRM" => 26,
        "SIGPROF" => 27,
        "SIGWINCH" => 28,
        "SIGIO" => 29,
        "SIGPWR" => 30,
        "SIGSYS" => 31,
        _ => 0,
    }
}

impl LlvmBackend {
    /// Emit a library shim — no main function, only `__briev_init_state`
    /// and dso_local wrappers for #export functions.
    /// 2026-07-19: Emit wrappers for exported functions in shared library.
    /// In --shared mode, exported functions keep their original names (e.g., @add)
    /// with dso_local visibility. The C caller passes a %State pointer as the first
    /// argument. This is the simplest ABI; future work may add a %State-less wrapper.
    pub(super) fn emit_shared_lib_exports(&mut self, out: &mut String, items: &[TopLevel]) {
        for item in items {
            if let TopLevel::Export(e) = item {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    // The function is already emitted by emit_definition with its
                    // original name. In --shared mode, it has dso_local visibility
                    // (set by emit_definition). No additional wrapper needed.
                    // Future: emit a wrapper that allocates %State on the caller's
                    // behalf, avoiding the need for the host to pass a state pointer.
                }
            }
        }
        writeln!(out, "define dso_local void @__briev_init_state({}) #0 {{", self.ctx.state_ptr_param).ok();
        // 2026-08-09 (init kind, Phase 2): library mode seeds runtime-seeded
        // invariants before any exported function runs.
        self.emit_init_seeding_stores(out, "  ");
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "define void @__briev_init() #0 {{").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "define void @__briev_fini() #0 {{").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "@llvm.global_ctors = appending global [1 x {{ i32, ptr, ptr }}] [{{ i32, ptr, ptr }} {{ i32 65535, ptr @__briev_init, ptr null }}]").ok();
    }

    /// Called when `self.ctx.library_mode` is true.

    /// 2026-08-04 (compiler-in-Briev): the LLVM ABI return type for a
    /// function. Struct/obj values are pointer handles (alloca + ptrtoint),
    /// so a function returning `List<T>` or any struct returns the i64
    /// HANDLE, not the struct ABI type — `llvm_type(List<String>)` resolves
    /// to `{ { ptr, i64 }, i64 }`, but the value emitted is the i64 pointer,
    /// which opt rejects ("defined with type 'i64' but expected '{...}'").
    pub(super) fn llvm_ret_abi_type(&self, ty: &crate::ast::Type) -> String {
        let base = match ty {
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n,
            _ => return self.llvm_type(ty),
        };
        // 2026-08-13 (obj value ABI): an `obj` return is the boxed i{int_bits}
        // handle (llvm_type); a StaticStruct return keeps the legacy i64 box.
        if self.ctx.obj_types.contains(base) {
            return self.llvm_type(ty);
        }
        if self.ctx.struct_types.contains_key(base) {
            "i64".to_string()
        } else {
            self.llvm_type(ty)
        }
    }

    pub(super) fn emit_library_shim(&mut self, out: &mut String, txns: &[(String, &crate::ast::Transaction)]) {
        // The #export wrappers are already emitted by emit_definition (called
        // earlier in generate()). We only need to add __briev_init_state.
        // 2026-08-03 (node bridge): __briev_init_state MUST return a
        // long-lived state pointer — the previous `alloca %State` returned a
        // stack address that dangled as soon as the call returned. It stayed
        // latent while no export touched a state field (rank.bv), but any
        // stateful export (save/read on `saved`) read garbage/crashed. The
        // library model is one state per process, so a module global is
        // correct; init_state fills it, __glue_release stays a no-op.
        writeln!(out, "@__briev_state = global %State zeroinitializer").ok();
        writeln!(out, "define dso_local i64 @__briev_init_state() local_unnamed_addr #0 {{").ok();
        writeln!(out, "  call void @init_state(ptr @__briev_state)").ok();
        writeln!(out, "  %ptr = ptrtoint ptr @__briev_state to i64").ok();
        writeln!(out, "  ret i64 %ptr").ok();
        writeln!(out, "}}").ok();
        // Also emit a __glue_release placeholder (no-op for arena-free bridge)
        writeln!(out, "define dso_local void @__glue_release(i64 %frame_tag) local_unnamed_addr #0 {{").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        // 2026-08-03: host cancellation — raise/clear the process-global
        // flag that CancelRequested#() polls. The state pointer is accepted
        // (future: per-state flag) but unused; the flag is process-global.
        writeln!(out, "define dso_local void @__briev_set_cancel(ptr %state, i32 %flag) local_unnamed_addr #0 {{").ok();
        writeln!(out, "  store atomic i32 %flag, ptr @__briev_cancel_flag seq_cst, align 4").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out, "define dso_local void @__briev_clear_cancel(ptr %state) local_unnamed_addr #0 {{").ok();
        writeln!(out, "  store atomic i32 0, ptr @__briev_cancel_flag seq_cst, align 4").ok();
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
    }

    /// Emit a standalone `@cell_persistent_ticks(ptr %state)` function that runs
    /// one convergence pass on each registered persistent cell. Called from the
    /// main reactor loop after `@reactor_tick`.
    pub(super) fn emit_persistent_cell_ticks(&mut self, out: &mut String) {
        // 2026-06-28: Clear let_bindings at function entry. Stale bindings from
        // the convergence loop (loop_engine.rs) leak across function boundaries
        // because let_bindings is a shared HashMap. This causes emit_expr to
        // return registers that were defined in a different basic block, producing
        // SSA dominance violations ("Instruction does not dominate all uses").
        self.fun.let_bindings.clear();
        self.fun.let_binding_types.clear();
        let names: Vec<String> = self.ctx.cell_defs.iter()
            .filter(|(_, c)| c.is_persistent)
            .map(|(name, _)| name.clone())
            .collect();
        if names.is_empty() { return; }

        // Emit a standalone @cell_persistent_ticks(ptr %state) that runs one
        // convergence pass on each registered persistent cell. This is called
        // from @reactor_tick (see dispatch.rs) so it fires every main loop
        // iteration regardless of which dispatch path the program takes.
        //
        // Why a separate function instead of inlining in each dispatch path?
        // The LLVM backend has 7+ dispatch strategies (folded, SSA, reactor,
        // parallel, etc.). A single tick function avoids duplicating the cell
        // evaluation logic across all of them. The function takes ptr so
        // it reads/writes the same %State struct as the rest of the program,
        // using the cell$name$field prefixed slots registered in
        // build_field_index.
        writeln!(out, "define void @cell_persistent_ticks({}) local_unnamed_addr #2 {{", self.ctx.state_ptr_param).ok();
        writeln!(out, "  entry:").ok();

        let prev_state = self.fun.state_reg_name.clone();
        self.fun.state_reg_name = "%state".to_string();

        for name in &names {
            let cell = self.ctx.cell_defs.get(name).unwrap().clone();

            // Evaluate internal triggers before running transactions
            for trg in &cell.internal_triggers {
                let trg_key = format!("cell${}${}", name, trg.name);
                let trg_idx_opt = self.ctx.field_index_map.get(&trg_key).copied();
                if let Some(trg_idx) = trg_idx_opt {
                    let trg_ll_ty = self.ctx.field_types[trg_idx].clone();
                    // 2026-07-14: Trigger LinkRef removed; use TtyReadKey# for all trigger reads
                    let read_expr = crate::ast::Expr::Call("TtyReadKey#".to_string(), vec![], None);
                    let result = self.emit_expr(out, &read_expr, "  ");
                    let conv = format!("%cit_{}_{}", self.fun.txn_counter, trg.name);
                    self.fun.txn_counter += 1;
                    // tty_read_key returns i64; trunc to match the state slot's type
                    let ll_storage_ty = &trg_ll_ty;
                    if ll_storage_ty == "i8" {
                        writeln!(out, "  {} = trunc i64 {} to i8", conv, result.name).ok();
                    } else {
                        // i32 for Char, i64 for Int, etc.
                        writeln!(out, "  {} = trunc i64 {} to {}", conv, result.name, ll_storage_ty).ok();
                    }
                    let gep = format!("%cit_gep_{}_{}", self.fun.txn_counter, trg.name);
                    self.fun.txn_counter += 1;
                    writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                        gep, trg_idx).ok();
                    writeln!(out, "  store {} {}, ptr {}, align 1", trg_ll_ty, conv, gep).ok();
                }
            }

            for txn in &cell.transactions {
                let pre = Self::rewrite_cell_identifiers(
                    &txn.contract.pre_condition, name);
                let cond = self.emit_expr(out, &pre, "  ");

                let fire_l = format!(".cpt_{}_{}", name, txn.name);
                let skip_l = format!(".cpt_{}_{}_s", name, txn.name);

                let cond_i1 = {
                    let r = format!("%cpct_{}_{}", name, txn.name);
                    self.fun.txn_counter += 1;
                    if cond.ty == Type::bool_() {
                        let t = format!("%cpct_{}_{}_t", name, txn.name);
                        writeln!(out, "  {} = trunc i8 {} to i1", t, cond.name).ok();
                        writeln!(out, "  {} = and i1 {}, true", r, t).ok();
                    } else {
                        writeln!(out, "  {} = icmp ne i64 {}, 0", r, cond.name).ok();
                    }
                    r
                };
                writeln!(out, "  br i1 {}, label %{}, label %{}",
                    cond_i1, fire_l, skip_l).ok();
                writeln!(out, "{}:", fire_l).ok();

                for stmt in &txn.body {
                    let rewritten = Self::rewrite_cell_stmt_identifiers(stmt, name);
                    emit_statement(self, out, &rewritten, "  ");
                }

                writeln!(out, "  br label %{}", skip_l).ok();
                writeln!(out, "{}:", skip_l).ok();
            }
            // Phase 4: propagate cell-to-cell wires after convergence
            // Only propagate when the source cell is NOT threaded (threaded cells
            // store outputs to channel globals; propagation from those requires
            // a separate channel-read path in the main loop).
            let is_threaded = self.cell_thread_names.contains(name);
            if !is_threaded {
                for (from_cell, from_port, to_cell, to_param) in &self.ctx.cell_wires.clone() {
                    if from_cell != name { continue; }
                    let src_prefixed = format!("cell${}${}", from_cell, from_port);
                    let dst_prefixed = format!("cell${}${}", to_cell, to_param);
                    if let Some(&src_idx) = self.ctx.field_index_map.get(&src_prefixed) {
                        if let Some(&dst_idx) = self.ctx.field_index_map.get(&dst_prefixed) {
                            let src_ll_ty = &self.ctx.field_types[src_idx];
                            let dst_ll_ty = &self.ctx.field_types[dst_idx];
                            let src_gep = format!("%cpw_src_{}_{}", self.fun.txn_counter, from_cell);
                            let dst_gep = format!("%cpw_dst_{}_{}", self.fun.txn_counter, from_cell);
                            let src_val = format!("%cpw_val_{}_{}", self.fun.txn_counter, from_cell);
                            self.fun.txn_counter += 1;
                            writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                                src_gep, src_idx).ok();
                            writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                                dst_gep, dst_idx).ok();
                            writeln!(out, "  {} = load {}, ptr {}, align 8", src_val, src_ll_ty, src_gep).ok();
                            writeln!(out, "  store {} {}, ptr {}, align 8", dst_ll_ty, src_val, dst_gep).ok();
                        }
                }
            }
            // Sync cell output ports to parent trigger bindings
            for (trg_name, cell_name, port_name) in &self.ctx.cell_trigger_bindings.clone() {
                if cell_name != name { continue; }
                let src_key = format!("cell${}${}", cell_name, port_name);
                let dst_key = trg_name.clone();
                if let Some(&src_idx) = self.ctx.field_index_map.get(&src_key) {
                    if let Some(&dst_idx) = self.ctx.field_index_map.get(&dst_key) {
                        let src_ll_ty = &self.ctx.field_types[src_idx];
                        let dst_ll_ty = &self.ctx.field_types[dst_idx];
                        let src_gep = format!("%cos_src_{}_{}", self.fun.txn_counter, cell_name);
                        let dst_gep = format!("%cos_dst_{}_{}", self.fun.txn_counter, cell_name);
                        let src_val = format!("%cos_val_{}_{}", self.fun.txn_counter, cell_name);
                        self.fun.txn_counter += 1;
                        writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                            src_gep, src_idx).ok();
                        writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                            dst_gep, dst_idx).ok();
                        writeln!(out, "  {} = load {}, ptr {}, align 8", src_val, src_ll_ty, src_gep).ok();
                        writeln!(out, "  store {} {}, ptr {}, align 8", dst_ll_ty, src_val, dst_gep).ok();
                    }
                }
            }

            // Phase 4 (threaded): propagate wires from THREADED source cells.
            // Threaded cells store outputs to atomic channel globals. We read
            // those globals here and store the value into the target cell's
            // state slot. This runs after all cell convergence passes.
            for (from_cell, from_port, to_cell, to_param) in &self.ctx.cell_wires.clone() {
                if !self.cell_thread_names.contains(from_cell) { continue; }
                let dst_prefixed = format!("cell${}${}", to_cell, to_param);
                if let Some(&dst_idx) = self.ctx.field_index_map.get(&dst_prefixed) {
                    let dst_ll_ty = &self.ctx.field_types[dst_idx];
                    let ch_val = format!("%ctw_val_{}_{}", self.fun.txn_counter, from_cell);
                    let ch_gep = format!("%ctw_ch_{}_{}", self.fun.txn_counter, from_cell);
                    let dst_gep = format!("%ctw_dst_{}_{}", self.fun.txn_counter, from_cell);
                    self.fun.txn_counter += 1;
                    // Volatile load from channel global
                    writeln!(out, "  {} = load volatile {}, ptr @chan_val_{}_{}, align 8",
                        ch_val, dst_ll_ty, from_cell, from_port).ok();
                    writeln!(out, "  {} = getelementptr %State, ptr %state, i32 0, i32 {}",
                        dst_gep, dst_idx).ok();
                    writeln!(out, "  store {} {}, ptr {}, align 8", dst_ll_ty, ch_val, dst_gep).ok();
                }
            }
        }

        }

        self.fun.state_reg_name = prev_state;
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    /// Emit a thread function for a persistent cell that loops with nanosleep.
    /// The function takes a ptr to a %CellState struct, runs convergence,
    /// and stores outputs to atomic channel globals.
    pub(super) fn emit_cell_thread(&mut self, out: &mut String, cell: &crate::ast::CellDef) {
        let cell_name = &cell.name;
        // Thread function receives a private %CellState.<name>* allocated by the caller
        // in emit_main. We temporarily replace field_index_map and field_types with
        // the cell-local versions so all GEP loads/stores resolve against the
        // %CellState.<name> type instead of %State.
        let saved_imap = self.ctx.field_index_map.clone();
        let saved_types = self.ctx.field_types.clone();
        let saved_state_reg = self.fun.state_reg_name.clone();

        if let Some((cs_imap, cs_tys)) = self.ctx.cell_state_types.get(cell_name) {
            self.ctx.field_index_map = cs_imap.clone();
            self.ctx.field_types = cs_tys.clone();
        }
        writeln!(out, "define ptr @cell_thread_{}(ptr %state) local_unnamed_addr #0 {{", cell_name).ok();
        writeln!(out, "  entry:").ok();
        self.fun.state_reg_name = "%state".to_string();

        let tick_ns = 1_000_000; // 1kHz default
        writeln!(out, "  %ts_sec = alloca i64, align 8").ok();
        writeln!(out, "  %ts_nsec = alloca i64, align 8").ok();
        writeln!(out, "  store i64 0, ptr %ts_sec").ok();
        writeln!(out, "  store i64 {}, ptr %ts_nsec", tick_ns).ok();

        writeln!(out, "  br label %loop").ok();
        writeln!(out, "loop:").ok();

        // nanosleep for tick interval
        writeln!(out, "  call i32 @nanosleep(ptr %ts_sec, ptr null)").ok();

        // Run convergence on cell state
        for txn in &cell.transactions {
            let pre = Self::rewrite_cell_identifiers(&txn.contract.pre_condition, cell_name);
            let cond = self.emit_expr(out, &pre, "  ");
            let fire_l = format!(".ct_{}_{}", cell_name, txn.name);
            let skip_l = format!(".ct_{}_{}_s", cell_name, txn.name);
            let cond_i1 = {
                let r = format!("%cti_{}_{}", cell_name, txn.name);
                self.fun.txn_counter += 1;
                if cond.ty == Type::bool_() {
                    writeln!(out, "  {} = and i1 {}, true", r, cond.name).ok();
                } else {
                    writeln!(out, "  {} = icmp ne i64 {}, 0", r, cond.name).ok();
                }
                r
            };
            writeln!(out, "  br i1 {}, label %{}, label %{}", cond_i1, fire_l, skip_l).ok();
            writeln!(out, "{}:", fire_l).ok();
            for stmt in &txn.body {
                let rewritten = Self::rewrite_cell_stmt_identifiers(stmt, cell_name);
                emit_statement(self, out, &rewritten, "  ");
            }
            writeln!(out, "  br label %{}", skip_l).ok();
            writeln!(out, "{}:", skip_l).ok();
        }

        // Atomic store outputs to channel globals
        // Use %CellState.<name> type for GEPs since cell fields are in the cell state
        let cell_state_type = format!("%CellState.{}", cell_name);
        let output_names = Self::extract_output_names_llvm(&cell.output_type);
        for port_name in &output_names {
            let prefixed = format!("cell${}${}", cell_name, port_name);
            if let Some(&idx) = self.ctx.field_index_map.get(&prefixed) {
                let ll_ty = &self.ctx.field_types[idx];
                let gep = format!("%ctg_{}_{}", cell_name, port_name);
                writeln!(out, "  {} = getelementptr {}, ptr {}, i32 0, i32 {}", gep, cell_state_type, self.fun.state_reg_name, idx).ok();
                let val = format!("%ctv_{}_{}", cell_name, port_name);
                writeln!(out, "  {} = load {}, ptr {}, align 8", val, ll_ty, gep).ok();
                writeln!(out, "  store atomic {} {}, ptr @chan_val_{}_{} seq_cst, align 8", ll_ty, val, cell_name, port_name).ok();
            }
        }
        // Set dirty flag
        writeln!(out, "  store atomic i8 1, ptr @chan_dirty_{} seq_cst, align 1", cell_name).ok();

        self.fun.state_reg_name = saved_state_reg;
        self.ctx.field_index_map = saved_imap;
        self.ctx.field_types = saved_types;
        writeln!(out, "  br label %loop").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    /// Emit channel globals for a persistent cell's output ports.
    pub(super) fn emit_cell_channel_globals(&mut self, out: &mut String, cell: &crate::ast::CellDef) {
        let cell_name = &cell.name;
        let output_names = Self::extract_output_names_llvm(&cell.output_type);
        // For persistent cells, look up field types in cell_state_types
        if let Some((cs_imap, cs_tys)) = self.ctx.cell_state_types.get(cell_name) {
            for port_name in &output_names {
                let prefixed = format!("cell${}${}", cell_name, port_name);
                if let Some(&idx) = cs_imap.get(&prefixed) {
                    let ll_ty = &cs_tys[idx];
                    let init = if ll_ty.contains('*') { "null" } else { "0" };
                    writeln!(out, "@chan_val_{}_{} = global {} {}, align 8", cell_name, port_name, ll_ty, init).ok();
                }
            }
        } else {
            // Fall back to field_index_map for non-persistent cells (shouldn't happen)
            for port_name in &output_names {
                let prefixed = format!("cell${}${}", cell_name, port_name);
                if let Some(&idx) = self.ctx.field_index_map.get(&prefixed) {
                    let ll_ty = &self.ctx.field_types[idx];
                    let init = if ll_ty.contains('*') { "null" } else { "0" };
                    writeln!(out, "@chan_val_{}_{} = global {} {}, align 8", cell_name, port_name, ll_ty, init).ok();
                }
            }
        }
        writeln!(out, "@chan_dirty_{} = global i8 0, align 1", cell_name).ok();
    }

    // ── AsmFn Emission ──────────────────────────────────────────────

    /// 2026-07-29: Emit an LLVM function from an AsmFn declaration.
    /// Each instruction in the body becomes a `call asm sideeffect` call.
    /// The function signature matches the declared params and return type.
    /// Registers are allocated by LLVM's register allocator (use "=r"/"r" constraints).
    pub(super) fn emit_asm_fn(&mut self, out: &mut String, af: &crate::ast::AsmFn) {
        // Determine LLVM return type
        let ll_ret = self.llvm_type(&af.ret_type);
        let ll_name = &af.name;

        // Emit the function header
        writeln!(out, "define {} @{}(", ll_ret, ll_name).ok();
        for (i, (_, t)) in af.params.iter().enumerate() {
            if i > 0 { write!(out, ", ").ok(); }
            write!(out, "{} %arg{}", self.llvm_type(t), i).ok();
        }
        writeln!(out, ") local_unnamed_addr #8 {{").ok();
        writeln!(out, "  entry:").ok();

        if af.body.is_empty() {
            // No instructions — return 0
            writeln!(out, "  ret {} 0", ll_ret).ok();
            writeln!(out, "}}").ok();
            return;
        }

        // Build constraint string and operand list for each instruction.
        // Each instruction uses "=r" for result and "r" for each param.
        let num_operands = af.params.len();
        let result_operand = if af.ret_type != crate::ast::Type::Void { 1 } else { 0 };

        for (ins_idx, instruction) in af.body.iter().enumerate() {
            // Substitute {param} → $N and {result} → $0
            let mut asm_text = instruction.clone();
            // Replace {result} with $0 (output operand)
            if result_operand > 0 {
                asm_text = asm_text.replace("{result}", "$0");
            }
            // Replace {param_name} with $N+1 (input operand)
            let mut offset = result_operand;
            for (p_idx, (p_name, _)) in af.params.iter().enumerate() {
                let placeholder = format!("{{{}}}", p_name);
                asm_text = asm_text.replace(&placeholder, &format!("${}", offset));
                offset += 1;
            }

            let is_last = ins_idx == af.body.len() - 1;

            if result_operand > 0 {
                // Build constraint: "=r" for output, "r" for each input, then clobbers
                let mut constraint = "=r".to_string();
                for _ in &af.params {
                    constraint.push_str(",r");
                }
                if !af.params.is_empty() {
                    constraint.push(',');
                }
                constraint.push_str("~{dirflag},~{fpsr},~{flags}");

                let mut args = Vec::new();
                for (_, t) in &af.params {
                    args.push(format!("{} %arg{}", self.llvm_type(t), args.len()));
                }

                writeln!(out, "  %r{} = call {} asm \"{}\", \"{}\"({})",
                    ins_idx, ll_ret, asm_text, constraint, args.join(", ")).ok();

                if is_last {
                    writeln!(out, "  ret {} %r{}", ll_ret, ins_idx).ok();
                }
            } else {
                // No return value (void function or intermediate)
                let mut constr = String::new();
                for (i, (_, _)) in af.params.iter().enumerate() {
                    if i > 0 { constr.push(','); }
                    constr.push('r');
                }
                if !af.params.is_empty() {
                    constr.push(',');
                }
                constr.push_str("~{dirflag},~{fpsr},~{flags}");
                let args: Vec<String> = af.params.iter().enumerate()
                    .map(|(i, (_, t))| format!("{} %arg{}", self.llvm_type(t), i))
                    .collect();
                writeln!(out, "  call void asm sideeffect \"{}\", \"{}\"({})",
                    asm_text, constr, args.join(", ")).ok();
            }
        }

        // Close function if no ret was emitted
        if result_operand == 0 || af.body.len() > 1 {
            writeln!(out, "  ret {} 0", ll_ret).ok();
        }
        writeln!(out, "}}").ok();
    }

    // ── ISR handlers + vector tables (2026-09-06, ISR plan) ────────────
    //
    // The MECHANISM (explicit in `<>`, else the profile's isr_mechanism —
    // ctx.isr_mechanism) owns the calling convention and table layout; the
    // compiler owns the emission. Body shape:
    //
    //   define void @<name>() nounwind ["interrupt"="IRQ"] {
    //     call void @__isr_body_<name>();
    //     ret void
    //   }
    //
    // The interrupt attribute (or x86_intrcc) makes LLVM emit the
    // mechanism's prologue/epilogue around the CALL — the body itself is
    // an ordinary function emitted through emit_definition (one statement
    // pipeline, DRY; the call layer optimizes away under -O3).

    /// Resolve the ISR's mechanism from the declaration or the profile.
    fn isr_resolve_mechanism(
        &self, isr: &crate::ast::top::IsrHandler,
    ) -> Result<(String, crate::target::IsrMechanism), String> {
        let registry = crate::target::IsrMechanismConfig::load();
        let name = isr.mechanism.as_deref()
            .or(self.ctx.isr_mechanism.as_deref());
        let Some(name) = name else {
            return Err(format!(
                "isr '{}': no ISR mechanism — the typecheck should have \
                 rejected this; declare isr<mechanism> or set isr_mechanism \
                 in the target profile",
                isr.name
            ));
        };
        registry.get(name)
            .cloned()
            .map(|m| (name.to_string(), m))
            .ok_or_else(|| format!(
                "isr '{}': mechanism '{}' is not a row of \
                 config/isr-targets.dbvl",
                isr.name, name
            ))
    }

    /// Resolve the ISR's vector slot — literal index or board-file name.
    pub(crate) fn isr_resolve_slot(&self, isr: &crate::ast::top::IsrHandler) -> Result<u64, String> {
        match &isr.vector {
            crate::ast::Expr::Decimal(n) => Ok(*n as u64),
            crate::ast::Expr::Identifier(name) => {
                crate::address_resolver::resolve_isr_vector(name).ok_or_else(|| {
                    format!(
                        "isr '{}': vector name '{}' has no row in the active \
                         board's interrupts.dbvl",
                        isr.name, name
                    )
                })
            }
            _ => Err(format!("isr '{}': vector must be a literal or a board-file name", isr.name)),
        }
    }

    /// Emit one ISR handler: body function + interrupt-convention wrapper.
    pub(crate) fn emit_isr_handler(
        &mut self, out: &mut String, isr: &crate::ast::top::IsrHandler,
    ) -> Result<(), String> {
        let (mech_name, mech) = self.isr_resolve_mechanism(isr)?;
        let _ = mech_name;
        let body_name = format!("__isr_body_{}", isr.name);

        // The body as an ordinary definition — one statement pipeline.
        // Void return: an ISR never returns a value; without an explicit
        // output_type the defn emitter would default the ret type.
        let body_defn = crate::ast::top::Definition {
            name: body_name.clone(),
            type_params: vec![],
            parameters: isr.params.clone(),
            output_type: Some(crate::ast::top::OutputType::Single(crate::ast::Type::Void)),
            outputs: vec![],
            contract: isr.contract.clone(),
            body: isr.body.clone(),
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: Some(isr.span.clone()),
            doc: Some(format!("ISR body for {} (plan 2026-09-06-isr-handlers-and-sections.md)", isr.name)),
        };
        // needs_state=true — the body shares the program state (the global
        // when ISR handlers exist; the wrapper passes it explicitly).
        self.emit_definition(out, &body_defn, self.ctx.state_is_global);
        writeln!(out).ok();

        // The interrupt wrapper — mechanism calling convention inline.
        let _ = mech_name;
        let define_line = match mech.convention {
            crate::target::IsrConv::X86Intr =>
                format!("define x86_intrcc void @{}() nounwind {{", isr.name),
            crate::target::IsrConv::RiscvInterrupt =>
                format!("define void @{}() nounwind \"interrupt\"=\"machine\" {{", isr.name),
            crate::target::IsrConv::ArmIrq =>
                format!("define void @{}() nounwind \"interrupt\"=\"IRQ\" {{", isr.name),
        };
        writeln!(out, "{}", define_line).ok();
        writeln!(out, "entry:").ok();
        if self.ctx.state_is_global {
            writeln!(out, "  call void @{}(ptr @__briev_state)", body_name).ok();
        } else {
            writeln!(out, "  call void @{}()", body_name).ok();
        }
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        Ok(())
    }

    /// Emit the vector table(s) + default handlers — one table per
    /// mechanism group, after all handler definitions. Layout per the
    /// mechanism row: entry stride, optional SP slot 0, thumb bit.
    pub(crate) fn emit_isr_vector_tables(&mut self, out: &mut String, items: &[TopLevel]) {
        let isr_items: Vec<&crate::ast::top::IsrHandler> = items.iter().filter_map(
            |i| match i { TopLevel::IsrHandler(h) => Some(h), _ => None },
        ).collect();
        if isr_items.is_empty() {
            return;
        }
        // Group by resolved mechanism (sorted for deterministic emission).
        let mut groups: std::collections::BTreeMap<String, Vec<(&crate::ast::top::IsrHandler, u64)>> = Default::default();
        for isr in &isr_items {
            let Ok((mech_name, _)) = self.isr_resolve_mechanism(isr) else { continue };
            let Ok(slot) = self.isr_resolve_slot(isr) else { continue };
            groups.entry(mech_name).or_default().push((isr, slot));
        }
        let registry = crate::target::IsrMechanismConfig::load();
        for (mech_name, mut handlers) in groups {
            let Some(mech) = registry.get(&mech_name) else { continue };
            // SP slot (ARM): slot 0 is the initial stack pointer — the
            // startup code sets the real value; the table reserves the
            // slot with 0.
            let sp_pad = if mech.sp_slot { 1 } else { 0 };
            handlers.sort_by_key(|(_, slot)| *slot);
            let max_slot = handlers.iter().map(|(_, s)| *s).max().unwrap_or(0);
            let table_len = max_slot + 1 + sp_pad;
            let word = if mech.entry_stride == 4 { "i32" } else { "i64" };

            // Default handler — one spin-loop definition per group, emitted
            // only when at least one slot is undeclared.
            let has_gaps = (0..table_len).any(|s| {
                let effective = if mech.sp_slot && s == 0 { u64::MAX } else { s };
                !handlers.iter().any(|(_, slot)| *slot == effective)
                    && !(mech.sp_slot && s == 0)
            });
            let default_name = mech.default_handler.clone();
            if has_gaps {
                // Entry block must have no predecessors — branch into the
                // spin label rather than looping on the entry itself.
                writeln!(out, "define void @{}() nounwind {{", default_name).ok();
                writeln!(out, "entry:").ok();
                writeln!(out, "  br label %spin_{}", default_name).ok();
                writeln!(out, "spin_{}:", default_name).ok();
                writeln!(out, "  br label %spin_{}", default_name).ok();
                writeln!(out, "}}").ok();
                writeln!(out).ok();
            }

            // The table itself.
            let entries: Vec<String> = (0..table_len).map(|s| {
                if mech.sp_slot && s == 0 {
                    return format!("{} 0", word);
                }
                let handler = handlers.iter().find(|(_, slot)| *slot == s)
                    .map(|(h, _)| h.name.clone())
                    .unwrap_or_else(|| default_name.clone());
                // Plain ptrtoint constexpr — the linker applies the ISA bit
                // (Thumb) via the symbol relocation (R_ARM_ABS32 sets bit 0
                // for Thumb symbols); LLVM 18 rejects `or` constexprs in
                // globals, and faking the bit in IR would fight the linker.
                format!("{} ptrtoint (ptr @{} to {})", word, handler, word)
            }).collect();
            let section = if mech.table_section.is_empty() {
                String::new()
            } else {
                format!(", section \"{}\"", mech.table_section)
            };
            writeln!(out,
                "@__vector_table_{} = global [{} x {}] [ {} ]{}, align {}",
                mech_name, table_len, word,
                entries.join(", "),
                section, mech.entry_stride).ok();
            writeln!(out).ok();
        }
    }
}

// ═══ 2026-08-26 (async Phase C, plan 2026-08-26-async-phase-c-segmented-
//     lowering.md): compiled task concurrency — segmented continuations ═══
//
// A spawned defn lowers to N segment functions `__task_<fn>_seg<k>` over a
// shared argv block (params only — locals die at yield, the probed reference
// rule), plus a private fn-pointer table. Spawn/await/free call into the C
// scheduler in briev_rt.c, which drives segments round-robin exactly like
// the interpreter's Await handler.
//
// TEMP undo: delete this block + the Await/spawn/FreeHint arms + the rt.c
// block to return to eager-inline spawns.

impl crate::backend::llvm::LlvmBackend {
    /// Emit everything the segmented task runtime needs: extern declares,
    /// per-task segment functions, and the segment-table globals.
    pub(super) fn emit_task_runtime(&mut self, out: &mut String) {
        writeln!(out,
            "declare i64 @briev_task_spawn(ptr, i32, i32, ptr)").ok();
        // 2026-08-26 (async Phase D): event-port runtime surface.
        let tm = |n: &str| self.ctx.defn_params.contains_key(n);
        if !tm("briev_event_alloc_impl") {
            writeln!(out, "declare i64 @briev_event_alloc()").ok();
        }
        if !tm("briev_event_fire_impl") {
            writeln!(out, "declare void @briev_event_fire(i64, i64)").ok();
        }
        if !tm("briev_event_read_impl") {
            writeln!(out, "declare i32 @briev_event_read(i64, ptr)").ok();
        }
        if !tm("briev_event_ready_impl") {
            writeln!(out, "declare i32 @briev_event_ready(i64)").ok();
        }
        if !tm("__briev_event_strict_trap") {
            writeln!(out, "declare void @briev_event_strict_trap()").ok();
        }
        writeln!(out,
            "declare i64 @briev_await(i64)").ok();
        writeln!(out,
            "declare void @briev_task_cancel(i64)").ok();
        writeln!(out).ok();

        let mut names: Vec<String> = self.ctx.task_segments.keys().cloned().collect();
        names.sort();
        for name in &names {
            let (segments, params): (Vec<Vec<Statement>>, Vec<(String, Type)>) =
                self.ctx.task_segments.get(name).unwrap().clone();
            let nseg = segments.len();
            // Segment functions first, then the table referencing them.
            for (k, seg) in segments.iter().enumerate() {
                let is_last = k + 1 == nseg;
                self.emit_task_segment_fn(out, name, k, seg, &params, is_last);
                writeln!(out).ok();
            }
            let seg_names: Vec<String> = (0..nseg)
                .map(|k| format!("ptr @__task_{name}_seg{k}"))
                .collect();
            writeln!(out,
                "@__task_{name}_segments = private constant [{} x ptr] [{}]",
                nseg,
                seg_names.join(", "))
            .ok();
            writeln!(out).ok();
        }
    }

    /// One segment function: `define {i64,i32} @__task_<fn>_seg<k>(ptr %argv)`.
    /// Params load from the argv block as i64 slots — the SAME representation
    /// defn parameters carry after entry adaptation (Ptr arrives as an i64
    /// address; boxed scalars as i64). Only the FINAL segment (or a
    /// `term expr;`) reports finished=1.
    fn emit_task_segment_fn(
        &mut self,
        out: &mut String,
        task_name: &str,
        k: usize,
        body: &[Statement],
        params: &[(String, Type)],
        is_last: bool,
    ) {
        self.fun.pending_cleanup.clear();
        self.fun.clear_locals();
        self.fun.reassigned_lets.clear();
        self.fun.expr_dedup_cache.clear();
        self.fun.is_static_bound = false;
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        self.fun.fn_ret_ty = "i64".to_string();
        self.fun.returns_i64 = true;
        self.fun.terminated = false;
        self.fun.is_task_segment = true;
        // 2026-09-10 (task machine migration): C-flat ABI - the machine is
        // a Briev defn dispatching through TaskCall# (i64 status return);
        // the segment stores its value into the caller's out cell. %state
        // is threaded because segment bodies call state-taking defns.
        writeln!(out,
            "define internal i64 @__task_{task_name}_seg{k}({}, ptr %argv, ptr %out) {{",
            self.ctx.state_ptr_param).ok();
        writeln!(out, "entry:").ok();
        for (i, (pname, pty)) in params.iter().enumerate() {
            let slot = self.fun.gen_reg();
            writeln!(out,
                "  {slot} = getelementptr inbounds [8 x i64], ptr %argv, i32 0, i32 {i}")
            .ok();
            let val = self.fun.gen_reg();
            writeln!(out, "  {val} = load i64, ptr {slot}").ok();
            // Real declared types — Phase D arms dispatch on Event(_) here;
            // Int-typed wires and scalars share the i64 representation, so
            // the LOAD stays uniform either way.
            self.fun.let_bindings.insert(pname.clone(), val);
            self.fun.let_original_types.insert(pname.clone(), pty.clone());
            self.fun.let_binding_types.insert(pname.clone(), pty.clone());
        }
        let mut last_val: Option<String> = None;
        // 2026-08-27 fix: `term expr;` is intercepted BEFORE the generic
        // statement emitter — its defn-style `ret i64` would both bypass this
        // function's {i64,i32} ABI and strand dead code. Bare `term;` is a
        // convergence checkpoint (interpreter: Ok(Void), continue).
        let mut saw_term = false;
        for s in body {
            if self.fun.terminated {
                break;
            }
            match s {
                Statement::Term(Some(e)) => {
                    let reg = self.emit_expr(out, e, "  ");
                    let val = self.adapt_to_i64(out, "  ", &reg);
                    last_val = Some(val);
                    saw_term = true;
                    break;
                }
                Statement::Term(None) => {}
                other => {
                    let reg =
                        crate::backend::llvm::emit_stmt::emit_statement(self, out, other, "  ");
                    if self.llvm_type(&reg.ty) == "i64" {
                        last_val = Some(reg.name.clone());
                    } else {
                        last_val = Some(self.adapt_to_i64(out, "  ", &reg));
                    }
                }
            }
        }
        let value_reg = match last_val {
            Some(v) => v,
            None => {
                let z = self.fun.gen_reg();
                writeln!(out, "  {z} = add i64 0, 0").ok();
                z
            }
        };
        let finished: i32 = if is_last || saw_term || self.fun.terminated { 1 } else { 0 };
        let outv = self.fun.gen_reg();
        writeln!(out, "  {outv} = getelementptr i64, ptr %out, i64 0").ok();
        writeln!(out, "  store i64 {value_reg}, ptr {outv}").ok();
        let outs = self.fun.gen_reg();
        writeln!(out, "  {outs} = getelementptr i64, ptr %out, i64 1").ok();
        writeln!(out, "  store i64 {finished}, ptr {outs}").ok();
        writeln!(out, "  ret i64 {finished}").ok();
        writeln!(out, "}}").ok();
    }
}
