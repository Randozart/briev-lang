// ── Boundary Ownership Inference ──────────────────────────────────────
// 2026-08-31 (plan 2026-08-31-boundary-ownership-inference.md): classifies
// each export parameter/return and each frgn parameter/return with a
// BoundaryOwnership — Borrowed / Owned / ZeroCopy / Value / ZeroCost — so the
// compiler can reason about, and eventually eliminate, data copies at the FFI
// boundary.
//
// The insight: the compiler already holds the three signals needed to infer
// ownership deterministically (no heuristics):
//   - protocol variant (#String<C_String> vs #String<UTF8>) — via the universe
//     Cast.* properties (the same lookup `frgn_dispatch::lookup_foreign_type`
//     uses)
//   - direction (export = Briev→host; frgn = host→Briev) — from the AST node
//   - calling convention (c_abi / lto / wasm_import) — passed in by the caller
//
// Seeded ownership (pointer-representation types only; scalars are Value):
//   lto                    → ZeroCost (IR merged, no boundary)
//   #String<C_String> Ret  → ZeroCopy (Briev sends the NUL-terminated data ptr)
//   #String<C_String> Param→ Borrowed (host owns; Briev copies to use)
//   #String<UTF8>     any  → Owned (Briev owns the [len][data] handle)
//   unresolved / custom    → Borrowed (conservative; Phase 9 keywords override)
//
// This is ADDITIVE — it never modifies an existing optimization path. It only
// makes ownership queryable. No codegen relies on it until the follow-up
// wiring phase (plan §5) proves it correct (AGENTS Rule 9: tests first).
//
// Undo: if boundary ownership is ever abandoned, delete this module and its
// `boundary_ownership` field on AnalysisResults; no other code depends on it.

use crate::ast::{Definition, Expr, Statement, TopLevel, Type};
use crate::type_universe::TypeUniverse;
use std::collections::{HashMap, HashSet};

/// Ownership of a value crossing the FFI boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryOwnership {
    /// Passed by value (Int/Float/Bool/Char) — no ownership concern.
    Value,
    /// Briev owns the backing memory; the host borrows read-only (arena-lifetime).
    Owned,
    /// A C-string pointer (NUL-invariant) — zero-copy out of Briev's arena.
    ZeroCopy,
    /// Host owns the memory; Briev must copy into its own arena to use it.
    Borrowed,
    /// LLVM LTO — IR merged, no boundary exists. Nothing to reason about.
    ZeroCost,
}

impl BoundaryOwnership {
    /// Most-conservative (strongest obligation). When a value flows to several
    /// consumers, the boundary must satisfy the strictest one.
    ///
    /// Order (weak→strong): Value < Owned ≈ ZeroCopy < Borrowed < ZeroCost.
    /// ZeroCost is "no boundary" and dominates only because there is nothing
    /// to copy; for a value that ALSO crosses a real boundary, the real
    /// boundary's classification wins (see `meet_with_boundary`).
    fn meet(self, other: BoundaryOwnership) -> BoundaryOwnership {
        let rank = |o: BoundaryOwnership| match o {
            BoundaryOwnership::Value => 0,
            BoundaryOwnership::Owned | BoundaryOwnership::ZeroCopy => 1,
            BoundaryOwnership::Borrowed => 2,
            BoundaryOwnership::ZeroCost => 3,
        };
        if rank(self) >= rank(other) { self } else { other }
    }

    /// Meet that treats ZeroCost as a non-constraint: a value which is ZeroCost
    /// (pure LTO) but ALSO crosses a c_abi boundary keeps the c_abi class.
    fn meet_with_boundary(self, other: BoundaryOwnership) -> BoundaryOwnership {
        match (self, other) {
            (BoundaryOwnership::ZeroCost, o) => o,
            (o, BoundaryOwnership::ZeroCost) => o,
            _ => self.meet(other),
        }
    }

    /// Stable text name for the boundary contract (dbvl metadata, reports).
    pub fn as_str(self) -> &'static str {
        match self {
            BoundaryOwnership::Value => "value",
            BoundaryOwnership::Owned => "owned",
            BoundaryOwnership::ZeroCopy => "zero-copy",
            BoundaryOwnership::Borrowed => "borrowed",
            BoundaryOwnership::ZeroCost => "zero-cost",
        }
    }
}

/// Ownership of a single boundary (an export or a frgn).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundaryEntry {
    /// ownership of each parameter, by position
    pub params: Vec<BoundaryOwnership>,
    /// ownership of the return value (None = void)
    pub ret: Option<BoundaryOwnership>,
}

/// All boundary ownership results for a program.
#[derive(Debug, Clone, Default)]
pub struct BoundaryOwnershipResult {
    /// exports keyed by defn name
    pub exports: HashMap<String, BoundaryEntry>,
    /// frgns keyed by effective briev name
    pub frgns: HashMap<String, BoundaryEntry>,
}

/// Compute boundary ownership for every export and frgn in a program.
///
/// `convention_of` maps a frgn's effective briev name to its calling
/// convention ("c_abi" / "lto" / "wasm_import"); a None value means "c_abi"
/// (the default, and the only one that needs copy reasoning — lto has no
/// boundary).
pub fn compute_boundary_ownership(
    items: &[TopLevel],
    universe: Option<&TypeUniverse>,
    convention_of: &dyn Fn(&str) -> Option<String>,
) -> BoundaryOwnershipResult {
    let mut result = BoundaryOwnershipResult::default();

    // ── Index definitions, exports, frgns, state fields ────────────────
    let mut exports: HashMap<String, &Definition> = HashMap::new();
    let mut frgns: HashMap<String, Vec<(String, Type)>> = HashMap::new();
    let mut frgn_ret: HashMap<String, Option<Type>> = HashMap::new();
    let mut state_fields: HashSet<String> = HashSet::new();
    // 2026-08-31: declared boundary protocols from `type CStr: #String<C_String>`.
    // The universe registers these during the full compile (normalizer), but the
    // GLUE commands (bindings/extension/export/ownership) use parse_and_check,
    // which does not run the normalizer — so resolve the category/variant from
    // the type declarations as a fallback (same source build_type_protocols uses).
    let mut declared_protocols: HashMap<String, String> = HashMap::new();

    for item in items {
        match item {
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    exports.insert(d.name.clone(), d);
                }
            }
            TopLevel::ForeignBinding(fb) => {
                let name = fb.effective_briev_name().to_string();
                frgns.insert(name.clone(), fb.inputs.clone());
                frgn_ret.insert(
                    name,
                    fb.success_output.first().map(|(_, t)| t.clone()),
                );
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    state_fields.insert(name.clone());
                }
            }
            TopLevel::Constant(c) => {
                state_fields.insert(c.name.clone());
            }
            TopLevel::StateDecl(s) => {
                state_fields.insert(s.name.clone());
            }
            TopLevel::TypeDef(td) => {
                if let Some(p) = td.protocol.as_ref() {
                    declared_protocols.insert(td.name.clone(), p.clone());
                }
            }
            _ => {}
        }
    }

    // ── Seed frgn ownership from protocol variant + convention ──────────
    for (name, params) in &frgns {
        let convention = convention_of(name).unwrap_or_else(|| "c_abi".to_string());
        let param_owns: Vec<BoundaryOwnership> = params
            .iter()
            .map(|(_, t)| seed_from_protocol(t, &convention, Direction::Param, universe, &declared_protocols))
            .collect();
        let ret = frgn_ret
            .get(name)
            .and_then(|o| o.as_ref())
            .map(|t| seed_from_protocol(t, &convention, Direction::Return, universe, &declared_protocols));
        result.frgns.insert(name.clone(), BoundaryEntry {
            params: param_owns,
            ret,
        });
    }

    // ── Propagate export ownership transitively through the call graph ──
    let mut ctx = FlowCtx {
        exports: &exports,
        frgn_owns: &result.frgns,
        state_fields: &state_fields,
        universe,
        declared_protocols: &declared_protocols,
        memo: HashMap::new(),
        visiting: Vec::new(),
    };
    for name in exports.keys() {
        ctx.defn_ownership(name);
    }
    result.exports = ctx.memo;
    result
}

/// Direction the value crosses the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Host → Briev (a frgn parameter, or an export parameter coming from host)
    Param,
    /// Briev → Host (an export return, or a frgn return coming from Briev's use)
    Return,
}

/// Seed a type's boundary ownership from its protocol variant + convention.
fn seed_from_protocol(
    ty: &Type,
    convention: &str,
    dir: Direction,
    universe: Option<&TypeUniverse>,
    declared_protocols: &HashMap<String, String>,
) -> BoundaryOwnership {
    // 2026-08-31 (lto = IR merger, no boundary): the host and Briev share
    // layout and lifetime, so there is nothing to copy. Undo: if a future
    // lto path introduces a real boundary, drop this arm.
    if convention == "lto" {
        return BoundaryOwnership::ZeroCost;
    }

    let (cat, variant) = match protocol_of(ty, universe, declared_protocols) {
        Some(p) => p,
        // 2026-08-31 (unknown/custom type): conservative — assume the host
        // owns it and Briev may need a copy. Phase 9 `borrow`/`consume`/`owned`
        // keywords override this where the inference cannot see. Undo: if a
        // default-owned class is preferred, return Owned here instead.
        None => return BoundaryOwnership::Borrowed,
    };

    match cat.as_str() {
        // Scalars cross by value — no ownership concern.
        "Int" | "UInt" | "Float" | "Bool" | "Char" | "Bit" => BoundaryOwnership::Value,
        // 2026-08-31 (C-string composite, NUL invariant): a Briev heap string's
        // data region IS a valid NUL-terminated C string, so Briev→host is a
        // zero-copy `ptr+8` (str_to_c). Host→Briev has no length prefix, so
        // Briev must copy (cstr_to_briev). Undo: if CStr loses the NUL
        // invariant, both directions fall back to Owned/Borrowed.
        "String" | "Data" | "Blob" => match (variant.as_str(), dir) {
            ("C_String", Direction::Return) => BoundaryOwnership::ZeroCopy,
            ("C_String", Direction::Param) => BoundaryOwnership::Borrowed,
            _ => BoundaryOwnership::Owned,
        },
        // 2026-08-31 (pointer category): opaque borrowed pointer.
        _ => BoundaryOwnership::Borrowed,
    }
}

/// Resolve a type to its (protocol category, variant).
///
/// Mirrors `frgn_dispatch::lookup_foreign_type`: HashWord/HashWordVariant are
/// used directly; Custom types are resolved through the universe's Cast.*
/// properties (category) and `base` (variant). When the universe has not
/// registered the type (GLUE commands use parse_and_check, no normalizer), the
/// declared `type CStr: #String<C_String>` protocol string is parsed instead.
/// Never matches type names.
fn protocol_of(
    ty: &Type,
    universe: Option<&TypeUniverse>,
    declared_protocols: &HashMap<String, String>,
) -> Option<(String, String)> {
    match ty {
        Type::HashWord(cat) => Some((cat.trim_start_matches('#').to_string(), String::new())),
        Type::HashWordVariant(cat, var) => {
            Some((cat.trim_start_matches('#').to_string(), var.clone()))
        }
        Type::Custom(name) | Type::Applied(name, _) => {
            // Universe first (full compile path registers Cast.* properties).
            if let Some(u) = universe {
                if let Some(rt) = u.types.get(name) {
                    // Category from the first Cast.<Category> property.
                    for key in rt.properties.keys() {
                        if let Some(cat) = key.strip_prefix("Cast.") {
                            return Some((cat.to_string(), rt.base.clone()));
                        }
                    }
                }
            }
            // Declared protocol string fallback (`#String<C_String>`).
            if let Some(p) = declared_protocols.get(name) {
                return parse_declared_protocol(p);
            }
            None
        }
        _ => None,
    }
}

/// Parse a declared protocol string like `#String<C_String>` → (cat, variant).
/// A bare `#String` → ("String", "") → the seed's default-variant handling.
fn parse_declared_protocol(p: &str) -> Option<(String, String)> {
    let p = p.trim().trim_start_matches('#');
    if let Some(open) = p.find('<') {
        if let Some(close) = p.find('>') {
            return Some((p[..open].to_string(), p[open + 1..close].to_string()));
        }
    }
    if p.is_empty() {
        None
    } else {
        Some((p.to_string(), String::new()))
    }
}

/// Shared, mutable state for the transitive ownership propagation.
/// Bundles the borrows into one value so the recursive walkers take ≤2 params
/// (Praetor: max 6).
struct FlowCtx<'a> {
    exports: &'a HashMap<String, &'a Definition>,
    frgn_owns: &'a HashMap<String, BoundaryEntry>,
    state_fields: &'a HashSet<String>,
    universe: Option<&'a TypeUniverse>,
    declared_protocols: &'a HashMap<String, String>,
    memo: HashMap<String, BoundaryEntry>,
    visiting: Vec<String>,
}

impl<'a> FlowCtx<'a> {
    /// Memoized transitive ownership for an export defn.
    /// `visiting` breaks call cycles conservatively (in-progress ⇒ Borrowed).
    fn defn_ownership(&mut self, name: &str) -> BoundaryEntry {
        if let Some(v) = self.memo.get(name) {
            return v.clone();
        }
        if self.visiting.iter().any(|n| n == name) {
            // 2026-08-31 (cycle): conservative Borrowed, mirroring export_abi's
            // needs-state cycle handling. Undo: refine to a fixpoint if needed.
            return BoundaryEntry {
                params: vec![BoundaryOwnership::Borrowed],
                ret: Some(BoundaryOwnership::Borrowed),
            };
        }
        let d = self.exports.get(name).copied();
        let Some(d) = d else {
            // Not an exported Briev defn — no boundary of its own.
            self.memo.insert(name.to_string(), BoundaryEntry::default());
            return BoundaryEntry::default();
        };

        // Param ownership: seed each parameter by its type (Briev→host boundary,
        // so an export param is a host→Briev value = Param direction).
        let params: Vec<BoundaryOwnership> = d
            .parameters
            .iter()
            .map(|(_, t)| seed_from_protocol(t, "c_abi", Direction::Param, self.universe, self.declared_protocols))
            .collect();

        self.visiting.push(name.to_string());
        // The DECLARED return type is the boundary contract — seed from it, then
        // refine with the body's terminal flow (which may carry a stricter class,
        // e.g. a frgn CStr return → ZeroCopy). A literal in an `-> Int` export is
        // Value; a literal in an `-> String` export is Owned.
        let decl_ret = output_type_ownership(&d.output_type, self.universe, self.declared_protocols);
        let flow_ret = self.body_ownership(&d.body);
        self.visiting.pop();

        let ret = match (decl_ret, flow_ret) {
            (Some(d), Some(f)) => Some(d.meet_with_boundary(f)),
            (Some(d), None) => Some(d),
            (None, f) => f,
        };

        let entry = BoundaryEntry { params, ret };
        self.memo.insert(name.to_string(), entry.clone());
        entry
    }

    /// Ownership of the value a body returns (its terminal expr). Returns None
    /// when no statement carries an ownership signal (pure literals /
    /// arithmetic) — the DECLARED return type classifies those.
    fn body_ownership(&mut self, body: &[Statement]) -> Option<BoundaryOwnership> {
        fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
    }

    fn stmt_ownership(&mut self, stmt: &Statement) -> Option<BoundaryOwnership> {
        match stmt {
            Statement::Term(opt)
            | Statement::EndProgram(opt)
            | Statement::Rollback(opt) => {
                opt.as_ref().and_then(|e| self.expr_ownership(e))
            }
            Statement::Expression(expr) => self.expr_ownership(expr),
            Statement::Let { expr, .. } => expr.as_ref().and_then(|e| self.expr_ownership(e)),
            Statement::Assign(_, expr) => self.expr_ownership(expr),
            Statement::Guarded(_, body) => {
                fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
            }
            Statement::Foreach { body, .. } => {
                fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
            }
            Statement::Block(body) => {
                fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
            }
            // 2026-08-31: compile-time only — no runtime value crosses the boundary.
            Statement::MetadataAssignment(..) => None,
            // Conservative: unknown statement produces no boundary value.
            _ => None,
        }
    }

    /// Ownership of an `Expr::Call`: the callee's return ownership (a frgn's
    /// declared CStr return, or a called export's inferred return), met with
    /// the ownership of the argument expressions' flow.
    fn call_ownership(&mut self, name: &str, args: &[Expr]) -> Option<BoundaryOwnership> {
        let from_call = if self.frgn_owns.contains_key(name) {
            self.frgn_owns.get(name).and_then(|e| e.ret)
        } else if self.exports.contains_key(name) {
            Some(self.defn_ownership(name).ret.unwrap_or(BoundaryOwnership::Owned))
        } else {
            None
        };
        let args_own = fold_ownership(args.iter().filter_map(|a| self.expr_ownership(a)));
        meet_option(from_call, args_own)
    }

    /// Ownership of a value produced by an expression.
    fn expr_ownership(&mut self, expr: &Expr) -> Option<BoundaryOwnership> {
        match expr {
            // 2026-08-31: a bare state-field read yields a Briev-owned value
            // (the arena owns it; the host may borrow read-only).
            Expr::Identifier(name) => {
                if self.state_fields.contains(name) {
                    Some(BoundaryOwnership::Owned)
                } else {
                    None
                }
            }
            // 2026-08-31: an export calling a frgn whose return is a C string
            // inherits the frgn's ZeroCopy return ownership (transitive).
            Expr::Call(name, args, _) => self.call_ownership(name, args),
            Expr::BinaryOp(_, lhs, rhs) => {
                let l = self.expr_ownership(lhs);
                let r = self.expr_ownership(rhs);
                meet_option(l, r)
            }
            Expr::UnaryOp(_, inner) => self.expr_ownership(inner),
            Expr::List(items) => fold_ownership(items.iter().filter_map(|e| self.expr_ownership(e))),
            Expr::Cast(inner, _) => self.expr_ownership(inner),
            Expr::MethodCall(recv, _, args, _) => {
                let r = self.expr_ownership(recv);
                let args_own = fold_ownership(args.iter().filter_map(|a| self.expr_ownership(a)));
                meet_option(r, args_own)
            }
            Expr::Reflect(recv, _, _) => self.expr_ownership(recv),
            Expr::Index(arr, idx) => {
                let a = self.expr_ownership(arr);
                let i = self.expr_ownership(idx);
                meet_option(a, i)
            }
            Expr::Slice { array, start, end, .. } => {
                let mut acc = self.expr_ownership(array);
                for e in [start.as_ref(), end.as_ref()] {
                    if let Some(e) = e {
                        acc = meet_option(acc, self.expr_ownership(e));
                    }
                }
                acc
            }
            Expr::AddrOf(inner) => self.expr_ownership(inner),
            // 2026-08-31: literals and fresh constructions carry no ownership
            // signal of their own — the DECLARED return type classifies them
            // (an `-> Int` literal is Value; an `-> String` literal is Owned).
            // Only real sources (frgn returns, state-field reads) emit a signal.
            _ => None,
        }
    }
}

/// Seed the return ownership from the declared output type (the boundary
/// contract). A `Single` type resolves via the protocol category; unions and
/// tuples take the most-conservative member (a copy must satisfy every part).
fn output_type_ownership(
    output: &Option<crate::ast::OutputType>,
    universe: Option<&TypeUniverse>,
    declared_protocols: &HashMap<String, String>,
) -> Option<BoundaryOwnership> {
    use crate::ast::OutputType;
    let o = output.as_ref()?;
    match o {
        OutputType::Single(t) => Some(seed_from_protocol(t, "c_abi", Direction::Return, universe, declared_protocols)),
        OutputType::Union(v) => v.iter().filter_map(|x| output_type_ownership(&Some(x.clone()), universe, declared_protocols))
            .reduce(BoundaryOwnership::meet),
        OutputType::Tuple(v) => v.iter().filter_map(|x| output_type_ownership(&Some(x.clone()), universe, declared_protocols))
            .reduce(BoundaryOwnership::meet),
        OutputType::Array(inner) => output_type_ownership(&Some((**inner).clone()), universe, declared_protocols),
        OutputType::Named(_, inner) => output_type_ownership(&Some((**inner).clone()), universe, declared_protocols),
    }
}

/// Meet a sequence of ownership signals, preserving None when none exist.
fn fold_ownership(
    mut it: impl Iterator<Item = BoundaryOwnership>,
) -> Option<BoundaryOwnership> {
    let first = it.next()?;
    Some(it.fold(first, BoundaryOwnership::meet_with_boundary))
}

/// Meet two optional ownership signals.
fn meet_option(
    a: Option<BoundaryOwnership>,
    b: Option<BoundaryOwnership>,
) -> Option<BoundaryOwnership> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.meet_with_boundary(y)),
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, OutputType, TypeParam};

    fn defn(name: &str, body: Vec<Statement>) -> Definition {
        defn_typed(name, crate::ast::Type::Custom("Int".into()), body)
    }

    fn defn_typed(name: &str, ret: crate::ast::Type, body: Vec<Statement>) -> Definition {
        Definition {
            name: name.to_string(),
            type_params: Vec::<TypeParam>::new(),
            parameters: vec![],
            output_type: Some(OutputType::Single(ret)),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
                post_authority: false,
            },
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

    fn no_convention(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn scalar_export_is_value() {
        // `-> #Int` returning a literal → Value (declared scalar type).
        let d = defn_typed(
            "add",
            Type::HashWord("Int".into()),
            vec![Statement::Term(Some(Expr::Decimal(5)))],
        );
        let items = vec![exported(d)];
        let r = compute_boundary_ownership(&items, None, &no_convention);
        let e = &r.exports["add"];
        assert!(e.params.is_empty());
        assert_eq!(e.ret, Some(BoundaryOwnership::Value));
    }

    #[test]
    fn string_literal_return_is_owned() {
        // `-> #String<UTF8>` returning a literal → Owned (declared type wins).
        let d = defn_typed(
            "hello",
            Type::HashWordVariant("String".into(), "UTF8".into()),
            vec![Statement::Term(Some(Expr::Quoted(b"hi".to_vec())))],
        );
        let items = vec![exported(d)];
        let r = compute_boundary_ownership(&items, None, &no_convention);
        assert_eq!(r.exports["hello"].ret, Some(BoundaryOwnership::Owned));
    }

    #[test]
    fn cstr_export_return_is_zero_copy() {
        // `type CStr: #String<C_String>` — a CStr return seeded as ZeroCopy.
        // Simulated here as a direct HashWordVariant so the test needs no
        // populated universe.
        let d = defn("echo", vec![Statement::Term(Some(
            Expr::Identifier("x".into()),
        ))]);
        // The output_type is CStr (as #String<C_String>); the pass seeds params
        // from d.parameters — for a parameterless CStr return, no params.
        let mut items = vec![exported(d)];
        let _ = &mut items;
        // Directly exercise the seed function:
        let ty = Type::HashWordVariant("String".into(), "C_String".into());
        let o = seed_from_protocol(&ty, "c_abi", Direction::Return, None, &HashMap::new());
        assert_eq!(o, BoundaryOwnership::ZeroCopy);
    }

    #[test]
    fn cstr_param_is_borrowed() {
        let ty = Type::HashWordVariant("String".into(), "C_String".into());
        let o = seed_from_protocol(&ty, "c_abi", Direction::Param, None, &HashMap::new());
        assert_eq!(o, BoundaryOwnership::Borrowed);
    }

    #[test]
    fn utf8_string_is_owned() {
        let ty = Type::HashWordVariant("String".into(), "UTF8".into());
        assert_eq!(seed_from_protocol(&ty, "c_abi", Direction::Return, None, &HashMap::new()), BoundaryOwnership::Owned);
        assert_eq!(seed_from_protocol(&ty, "c_abi", Direction::Param, None, &HashMap::new()), BoundaryOwnership::Owned);
    }

    #[test]
    fn lto_is_zero_cost() {
        let ty = Type::HashWordVariant("String".into(), "C_String".into());
        assert_eq!(seed_from_protocol(&ty, "lto", Direction::Return, None, &HashMap::new()), BoundaryOwnership::ZeroCost);
    }

    #[test]
    fn unknown_type_is_borrowed() {
        // Bits and Void are compiler constructs → Borrowed via the fallback
        // (they resolve to no protocol category here).
        assert_eq!(seed_from_protocol(&Type::Bits(64), "c_abi", Direction::Return, None, &HashMap::new()), BoundaryOwnership::Borrowed);
    }

    #[test]
    fn scalar_seed_is_value() {
        let ty = Type::HashWord("Int".into());
        assert_eq!(seed_from_protocol(&ty, "c_abi", Direction::Return, None, &HashMap::new()), BoundaryOwnership::Value);
        let f = Type::HashWord("Float".into());
        assert_eq!(seed_from_protocol(&f, "c_abi", Direction::Param, None, &HashMap::new()), BoundaryOwnership::Value);
    }

    #[test]
    fn frgn_cstr_return_propagates_to_export() {
        // export f -> CStr { term cstr_fn(); }  where cstr_fn is a frgn
        // returning CStr → the export inherits ZeroCopy.
        let fb = crate::ast::top::ForeignBinding::new(
            "cstr_fn".into(),
            None,
            crate::ast::top::FromSpec::Literal(std::path::PathBuf::from("lib.c")),
            crate::ast::top::ForeignTarget::Native,
        );
        // Set the return type to CStr variant.
        let mut fb = fb;
        fb.success_output = vec![("r".into(), Type::HashWordVariant("String".into(), "C_String".into()))];
        let d = defn_typed(
            "f",
            Type::HashWordVariant("String".into(), "C_String".into()),
            vec![Statement::Term(Some(Expr::Call("cstr_fn".into(), vec![], None)))],
        );
        let items = vec![TopLevel::ForeignBinding(fb), exported(d)];
        let r = compute_boundary_ownership(&items, None, &no_convention);
        assert_eq!(r.exports["f"].ret, Some(BoundaryOwnership::ZeroCopy));
    }

    #[test]
    fn state_field_return_is_owned() {
        // top-level `let saved: String = "";` + export `-> #String<UTF8>`
        // returning it → Owned (both declared type and state-field read agree).
        let let_stmt = Statement::Let {
            name: "saved".into(),
            names: vec![],
            ty: Some(Type::Custom("String".into())),
            expr: Some(Expr::Quoted(b"".to_vec())),
            modifiers: vec![],
        };
        let d = defn_typed(
            "read",
            Type::HashWordVariant("String".into(), "UTF8".into()),
            vec![Statement::Term(Some(Expr::Identifier("saved".into())))],
        );
        let items = vec![TopLevel::Statement(Box::new(let_stmt)), exported(d)];
        let r = compute_boundary_ownership(&items, None, &no_convention);
        assert_eq!(r.exports["read"].ret, Some(BoundaryOwnership::Owned));
    }

    #[test]
    fn cstr_typedef_declared_protocol_end_to_end() {
        // Realistic program: `type CStr: #String<C_String>` + an export
        // `echo(name: CStr) -> CStr { term name; }`. The CStr type resolves via
        // the DECLARED protocol (no universe — GLUE commands run parse_and_check,
        // which does not run the normalizer). Param → borrowed (host owns the C
        // string); return → zero-copy (Briev sends the NUL-terminated data ptr).
        let typedef = TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
            name: "CStr".to_string(),
            type_params: vec![],
            parent: None,
            protocol: Some("#String<C_String>".to_string()),
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: vec![],
            ports_out: vec![],
            seq: false,
            body: crate::ast::top::TypeDefBody {
                pins: Vec::new(),
                slots: vec![],
                metadata: std::collections::HashMap::new(),
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        }));
        let d = Definition {
            name: "echo".to_string(),
            type_params: vec![],
            parameters: vec![("name".to_string(), Type::Custom("CStr".to_string()))],
            output_type: Some(OutputType::Single(Type::Custom("CStr".to_string()))),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
                post_authority: false,
            },
            body: vec![Statement::Term(Some(Expr::Identifier("name".into())))],
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        };
        let items = vec![typedef, exported(d)];
        let r = compute_boundary_ownership(&items, None, &no_convention);
        let e = &r.exports["echo"];
        assert_eq!(e.params, vec![BoundaryOwnership::Borrowed]);
        assert_eq!(e.ret, Some(BoundaryOwnership::ZeroCopy));
    }
}
