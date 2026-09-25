// ── Boundary Marshalling (`CStr ⇄ String` casts → binding calls) ───────
// 2026-08-03 (plan 2026-08-03-protocol-driven-glue-boundary): the casting
// graph resolves the minimal path between a boundary representation and the
// base protocol; codegen must emit the DELTA (the binding call), not a chain.
// But Briev's boxing turns String/CStr values into i64 registers, losing the
// type at codegen — `emit_cast_path` sees `Int` and picks the Int→String lane.
//
// This pass runs AFTER typechecking (like string_concat.rs) and rewrites a
// same-category representation cast (`CStr as String`, `s as CStr`) into the
// graph-resolved binding call (`cstr_to_briev`, `str_to_c`) on the typed AST,
// so the backend emits the marshalling directly. The protocol decision stays
// in the frontend (the graph's minimal path), not hardcoded per type.
//
// Undo: if values ever stop being boxed (registers keep their Briev type),
// delete this pass and let emit_cast_path handle representation casts.

use crate::ast::{Expr, Statement, TopLevel, Type};
use crate::casting::graph::{CastingGraph, LaneKind};
use crate::type_universe::TypeUniverse;

/// Rewrite same-category representation casts into their binding calls, and
/// insert the marshalling call at implicit meld-conversion sites (let-init,
/// term) so codegen emits the delta even without an explicit `as`.
pub fn rewrite_boundary_marshalling(items: &mut [TopLevel], universe: &TypeUniverse) {
    // Build a casting graph from the program's proto declarations so the
    // marshalling decision (minimal path → binding fn) is protocol-driven.
    let mut graph = CastingGraph::new();
    // Type → declared protocol (`type CStr: String<C_String>` → CStr →
    // "String<C_String>"). The universe is not populated until codegen, so
    // the pass resolves custom boundary types from their declarations.
    // 2026-09-02 (plan fundamental-parent-membership): the SHARED AST
    // derivation — bare-parent typedefs derive their category from the
    // parent chain (explicit declarations keep precedence).
    let mut type_protocols: std::collections::HashMap<String, String> =
        crate::casting::graph::derive_type_protocols(items);
    // 2026-08-03 (P3, node bridge): the typechecker admits melded pairs at
    // assignment, call args, and constructor slots too — the marshalling must
    // insert the delta at those sites as well. Pre-collect the types needed to
    // resolve the targets.
    let mut state_types: std::collections::HashMap<String, Type> = std::collections::HashMap::new();
    let mut fn_param_types: std::collections::HashMap<String, Vec<Type>> = std::collections::HashMap::new();
    let mut type_slots: std::collections::HashMap<String, Vec<crate::ast::top::TypeDefSlot>> =
        std::collections::HashMap::new();
    for item in items.iter() {
        match item {
            TopLevel::ProtocolDef(pd) => {
                graph.register_protocol_def(pd);
            }
            TopLevel::TypeDef(td) => {
                if !td.body.slots.is_empty() {
                    type_slots.insert(td.name.clone(), td.body.slots.clone());
                }
            }
            TopLevel::StaticStruct(sd) => {
                let slots: Vec<crate::ast::top::TypeDefSlot> = sd.fields
                    .iter()
                    .map(|(n, ty)| crate::ast::top::TypeDefSlot {
                        name: n.clone(),
                        ty: ty.clone(),
                        bit_range: None,
                    })
                    .collect();
                if !slots.is_empty() {
                    type_slots.insert(sd.name.clone(), slots);
                }
            }
            TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, ty, .. } = stmt.as_ref() {
                    if let Some(t) = ty {
                        state_types.insert(name.clone(), t.clone());
                    }
                }
            }
            TopLevel::Constant(c) => {
                state_types.insert(c.name.clone(), c.ty.clone());
            }
            TopLevel::Definition(d) => {
                fn_param_types.insert(
                    d.name.clone(),
                    d.parameters.iter().map(|(_, t)| t.clone()).collect(),
                );
            }
            TopLevel::Transaction(t) => {
                fn_param_types.insert(
                    t.name.clone(),
                    t.parameters.iter().map(|(_, t)| t.clone()).collect(),
                );
            }
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    fn_param_types.insert(
                        d.name.clone(),
                        d.parameters.iter().map(|(_, t)| t.clone()).collect(),
                    );
                }
            }
            TopLevel::ForeignBinding(fb) => {
                fn_param_types.insert(
                    fb.effective_briev_name().to_string(),
                    fb.inputs.iter().map(|(_, t)| t.clone()).collect(),
                );
            }
            _ => {}
        }
    }
    for item in items.iter_mut() {
        let ctx = MarshallingCtx {
            universe,
            graph: &graph,
            type_protocols: &type_protocols,
            fn_param_types: &fn_param_types,
            type_slots: &type_slots,
        };
        match item {
            TopLevel::Definition(d) => {
                let mut env = param_env(&d.parameters);
                for (n, t) in &state_types {
                    env.insert(n.clone(), t.clone());
                }
                let out = output_type_to_type(d.output_type.as_ref());
                rewrite_body(&ctx, &mut env, &mut d.body, out);
            }
            TopLevel::Transaction(t) => {
                let mut env = param_env(&t.parameters);
                for (n, ty) in &state_types {
                    env.insert(n.clone(), ty.clone());
                }
                let out = output_type_to_type(t.output_type.as_ref());
                rewrite_body(&ctx, &mut env, &mut t.body, out);
            }
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_mut() {
                    let mut env = param_env(&d.parameters);
                    for (n, ty) in &state_types {
                        env.insert(n.clone(), ty.clone());
                    }
                    let out = output_type_to_type(d.output_type.as_ref());
                    rewrite_body(&ctx, &mut env, &mut d.body, out);
                }
            }
            _ => {}
        }
    }
}

fn param_env(params: &[(String, Type)]) -> std::collections::HashMap<String, Type> {
    params.iter().cloned().collect()
}

/// The declared output type of a definition/txn, or None.
fn output_type_to_type(ot: Option<&crate::ast::top::OutputType>) -> Option<Type> {
    match ot {
        Some(crate::ast::top::OutputType::Single(t)) => Some(t.clone()),
        _ => None,
    }
}

/// Shared immutable context for the marshalling rewrite (bundled so the
/// recursive pass stays within the param-count guideline).
struct MarshallingCtx<'a> {
    universe: &'a TypeUniverse,
    graph: &'a CastingGraph,
    type_protocols: &'a std::collections::HashMap<String, String>,
    fn_param_types: &'a std::collections::HashMap<String, Vec<Type>>,
    type_slots: &'a std::collections::HashMap<String, Vec<crate::ast::top::TypeDefSlot>>,
}

fn rewrite_body(
    ctx: &MarshallingCtx<'_>,
    env: &mut std::collections::HashMap<String, Type>,
    body: &mut [Statement],
    current_output: Option<Type>,
) {
    for stmt in body {
        match stmt {
            Statement::Term(opt)
            | Statement::EndProgram(opt)
            | Statement::Rollback(opt) => {
                if let Some(expr) = opt.as_mut() {
                    rewrite_expr(ctx, env, expr);
                    // 2026-08-03 (P3): an implicit meld conversion at the
                    // return — `term s;` where s: String and the output is a
                    // melded CStr — needs the marshalling call inserted too.
                    if let Some(out) = &current_output {
                        wrap_if_marshalled(ctx, env, expr, out);
                    }
                }
            }
            Statement::Expression(expr) => rewrite_expr(ctx, env, expr),
            Statement::Let { name, expr, ty, .. } => {
                if let Some(e) = expr.as_mut() {
                    rewrite_expr(ctx, env, e);
                    // 2026-08-03 (P3): `let s: String = name;` (name: CStr) —
                    // the meld admits the pair; insert the marshalling call.
                    if let Some(declared) = ty.as_ref() {
                        wrap_if_marshalled(ctx, env, e, declared);
                    }
                }
                if let Some(t) = ty.as_ref() {
                    env.insert(name.clone(), t.clone());
                }
            }
            Statement::Assign(lhs, expr) => {
                rewrite_expr(ctx, env, expr);
                // 2026-08-03 (P3): `saved = name;` — the meld admits the pair
                // at assignment; the state field type (in env) drives the wrap.
                if let crate::ast::Expr::Identifier(target) = lhs {
                    if let Some(target_ty) = env.get(target).cloned() {
                        wrap_if_marshalled(ctx, env, expr, &target_ty);
                    }
                }
            }
            Statement::Guarded(_, body) => rewrite_body(ctx, env, body, current_output.clone()),
            Statement::Foreach { body, .. } => rewrite_body(ctx, env, body, current_output.clone()),
            Statement::Block(body) => rewrite_body(ctx, env, body, current_output.clone()),
            _ => {}
        }
    }
}

/// Wrap `expr` in its marshalling binding call when it crosses a melded
/// boundary representation (the typechecker admitted the pair without `as`).
/// Only same-category String variants produce a binding (marshalling_fn); any
/// other melded pair is left for the codegen layout machinery.
fn wrap_if_marshalled(
    ctx: &MarshallingCtx<'_>,
    env: &mut std::collections::HashMap<String, Type>,
    expr: &mut Expr,
    target: &Type,
) {
    // 2026-08-09 (Phase 12, SPEC §18.2): the implicit meld conversion is
    // removed — foreign shapes adapt through EXPLICIT protocol cast edges
    // (CastTo/CastFrom), not an implicit meld admission. This wrap is a no-op.
    let _ = (ctx, env, expr, target);
}

fn rewrite_expr(
    ctx: &MarshallingCtx<'_>,
    env: &mut std::collections::HashMap<String, Type>,
    expr: &mut Expr,
) {
    match expr {
        Expr::Cast(inner, target) => {
            rewrite_expr(ctx, env, inner);
            // A same-category representation cast (boundary type ⇄ base) is
            // the marshalling delta — resolve the graph path and emit the
            // binding call in its place.
            let src_ty = expr_type_of(inner, env, ctx.universe);
            if let Some(fn_name) = marshalling_fn(ctx.graph, ctx.universe, ctx.type_protocols, &src_ty, target) {
                let name = crate::ast::Expr::Call(fn_name, vec![(**inner).clone()], None);
                *expr = name;
            }
        }
        Expr::Call(fn_name, args, _) => {
            // 2026-08-03 (P3, node bridge): wrap call args that cross a melded
            // boundary representation — a callee param (frgn or defn) or a
            // struct/obj constructor slot whose type is melded with the arg.
            let callee_params: Option<Vec<Type>> = if ctx.type_slots.contains_key(fn_name) {
                ctx.type_slots.get(fn_name).map(|slots| slots.iter().map(|s| s.ty.clone()).collect())
            } else {
                ctx.fn_param_types.get(fn_name).cloned()
            };
            for (i, arg) in args.iter_mut().enumerate() {
                rewrite_expr(ctx, env, arg);
                if let Some(pts) = &callee_params {
                    if let Some(pt) = pts.get(i) {
                        wrap_if_marshalled(ctx, env, arg, pt);
                    }
                }
            }
        }
        Expr::BinaryOp(kind, l, r) => {
            rewrite_expr(ctx, env, l);
            rewrite_expr(ctx, env, r);
            // 2026-08-03 (P1.4): a concat on a boundary String variant (e.g.
            // `CStr + CStr`, or `++`) uses the VARIANT's own cross-op
            // (cstring_concat) — the generic inline concat treats the value
            // as [len][data], which is wrong for a nul-terminated C string.
            if matches!(kind, crate::ast::BinaryOpKind::Add | crate::ast::BinaryOpKind::Concat) {
                let lt = expr_type_of(l, env, ctx.universe);
                let rt = expr_type_of(r, env, ctx.universe);
                if let Some(fn_name) = variant_concat_fn(ctx.graph, ctx.universe, ctx.type_protocols, &lt, &rt) {
                    let lv = (**l).clone();
                    let rv = (**r).clone();
                    *expr = crate::ast::Expr::Call(fn_name, vec![lv, rv], None);
                }
            }
        }
        Expr::UnaryOp(_, inner) => rewrite_expr(ctx, env, inner),
        Expr::List(items) => {
            for item in items {
                rewrite_expr(ctx, env, item);
            }
        }
        _ => {}
    }
}

/// Resolve a type's (category, variant) for the marshalling decision.
/// Custom boundary types resolve from their declared protocol (the universe
/// isn't populated before codegen); builtins and hashwords fall through to
/// the casting graph.
fn resolve_category(
    graph: &CastingGraph,
    universe: &TypeUniverse,
    type_protocols: &std::collections::HashMap<String, String>,
    ty: &Type,
) -> (String, String) {
    match ty {
        Type::Custom(name) => {
            // Declared boundary types resolve via their protocol string; the
            // bootstrap String/Data resolve via the graph's Cast. property.
            // No type names are matched (rule 18).
            if let Some(proto) = type_protocols.get(name) {
                if let Some((cat, var)) = CastingGraph::parse_protocol_base(proto) {
                    return (cat, var);
                }
            }
            graph.type_to_protocol(universe, ty)
        }
        _ => graph.type_to_protocol(universe, ty),
    }
}

/// The Concat cross-op binding for an operand pair, when one is a boundary
/// String variant that declares its own concat (e.g. CStr → cstring_concat).
fn variant_concat_fn(
    graph: &CastingGraph,
    universe: &TypeUniverse,
    type_protocols: &std::collections::HashMap<String, String>,
    lt: &Type,
    rt: &Type,
) -> Option<String> {
    let (lcat, lvar) = resolve_category(graph, universe, type_protocols, lt);
    let (rcat, rvar) = resolve_category(graph, universe, type_protocols, rt);
    if lcat != rcat || lcat != "String" {
        return None;
    }
    let variant = if !lvar.is_empty() { lvar } else { rvar };
    if variant.is_empty() {
        return None;
    }
    graph.get_variant_op(&lcat, &variant, "Concat").map(str::to_string)
}

/// Resolve the binding function for a same-category representation cast:
/// `CStr → String` emits `cstr_to_briev`, `String → CStr` emits `str_to_c`.
/// Uses the casting graph's minimal path (a single ExtCallDyn binding step).
fn marshalling_fn(
    graph: &CastingGraph,
    universe: &TypeUniverse,
    type_protocols: &std::collections::HashMap<String, String>,
    src_ty: &Type,
    target: &Type,
) -> Option<String> {
    let (mut src_cat, mut src_var) = resolve_category(graph, universe, type_protocols, src_ty);
    let (mut dst_cat, mut dst_var) = resolve_category(graph, universe, type_protocols, target);
    // A bare base endpoint resolves to its DEFAULT variant (String → UTF8) so
    // the variant↔base path resolves (C_String → UTF8 via cstr_to_briev).
    // The casting graph's BFS cannot reach the bare "" node from a variant.
    if src_var.is_empty() {
        src_var = graph.default_variant(&src_cat).to_string();
    }
    if dst_var.is_empty() {
        dst_var = graph.default_variant(&dst_cat).to_string();
    }
    // Only same-category representation changes (variant ⇄ base) are the
    // boundary marshalling delta; numeric/protocol-crossing casts (Int→Float)
    // stay on the codegen path.
    if src_cat != dst_cat {
        return None;
    }
    if src_var == dst_var {
        return None; // identity (same representation) — no marshalling.
    }
    let path = graph.find_path(&src_cat, &src_var, &dst_cat, &dst_var)?;
    if path.len() == 1 {
        match &path[0].lane {
            LaneKind::ExtCallDyn(name) => return Some(name.clone()),
            LaneKind::ExtCall(name) => return Some(name.to_string()),
            _ => return None,
        }
    }
    None
}

/// The type of a cast's source expression (conservative): bound identifiers,
/// string literals, and the result of a string-producing binary op.
fn expr_type_of(
    expr: &Expr,
    env: &std::collections::HashMap<String, Type>,
    universe: &TypeUniverse,
) -> Type {
    match expr {
        Expr::Identifier(name) => env.get(name).cloned().unwrap_or(Type::int()),
        Expr::Quoted(_) => Type::Custom("String".to_string()),
        Expr::Cast(_, target) => target.clone(),
        Expr::BinaryOp(_, l, r) => {
            let lt = expr_type_of(l, env, universe);
            let rt = expr_type_of(r, env, universe);
            if crate::analysis::string_concat::is_string_category(&lt, universe)
                || crate::analysis::string_concat::is_string_category(&rt, universe)
            {
                Type::Custom("String".to_string())
            } else {
                lt
            }
        }
        _ => Type::int(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, CastDirection, Contract, Definition, OutputType, Statement};

    fn cstr_proto() -> TopLevel {
        TopLevel::ProtocolDef(crate::ast::top::ProtocolDef {
            name: "C_String".to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![
                crate::ast::top::CastEdge {
                    direction: CastDirection::CastTo,
                    target_category: "String".to_string(),
                    target_variant: "UTF8".to_string(),
                    binding: Some(crate::ast::top::CastBinding {
                        fn_name: "cstr_to_briev".to_string(),
                        param: "#Lh".to_string(),
                    }),
                trusted_axiom: false},
                crate::ast::top::CastEdge {
                    direction: CastDirection::CastFrom,
                    target_category: "String".to_string(),
                    target_variant: "UTF8".to_string(),
                    binding: Some(crate::ast::top::CastBinding {
                        fn_name: "str_to_c".to_string(),
                        param: "#Lh".to_string(),
                    }),
                trusted_axiom: false},
            ],
            cross_ops: vec![],
            span: None,
        })
    }

    fn cstr_type() -> TopLevel {
        TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
            name: "CStr".to_string(),
            type_params: vec![],
            protocol: Some("String<C_String>".to_string()),
            parent: None,
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
            body: crate::ast::top::TypeDefBody {
                pins: Vec::new(),
                reference: None,
                slots: vec![],
                metadata: std::collections::HashMap::new(),
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                when_laws: vec![],
                modes: vec![],
                span: None,
            },
            span: None,
        }))
    }

    #[test]
    fn cstr_to_string_cast_becomes_binding_call() {
        let universe = TypeUniverse::new();
        let mut items = vec![
            cstr_proto(),
            cstr_type(),
            TopLevel::Definition(Definition {
                    variadic_param: None,
                name: "marshall".to_string(),
                type_params: vec![],
                parameters: vec![("name".to_string(), Type::Custom("CStr".to_string()))],
                output_type: Some(OutputType::Single(Type::Custom("String".to_string()))),
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    span: None,
                    explicit: false,
                post_authority: false},
                body: vec![Statement::Term(Some(Expr::Cast(
                    Box::new(Expr::Identifier("name".to_string())),
                    Type::Custom("String".to_string()),
                )))],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                annotations: vec![],
                span: None,
                doc: None,
            }),
        ];
        rewrite_boundary_marshalling(&mut items, &universe);
        let TopLevel::Definition(d) = &items[2] else { panic!() };
        let Statement::Term(Some(expr)) = &d.body[0] else { panic!() };
        match expr {
            Expr::Call(name, args, _) => {
                assert_eq!(name, "cstr_to_briev", "CStr→String must emit cstr_to_briev");
                assert!(matches!(args[0], Expr::Identifier(ref n) if n == "name"));
            }
            other => panic!("expected a binding call, got {:?}", other),
        }
    }

    #[test]
    fn string_to_cstr_cast_becomes_zero_copy_str_to_c() {
        // 2026-08-31 (boundary ownership plan §5 verification): the ZeroCopy
        // direction (String → CStr) must emit `str_to_c` — the pointer-offset
        // delta that is copy-free — NOT `cstr_to_briev` (the allocating copy).
        // The ownership pass classifies this boundary ZeroCopy; this test locks
        // that the marshalling honors it (no copy introduced).
        let universe = TypeUniverse::new();
        let mut items = vec![
            cstr_proto(),
            cstr_type(),
            TopLevel::Definition(Definition {
                    variadic_param: None,
                name: "marshall_out".to_string(),
                type_params: vec![],
                parameters: vec![("s".to_string(), Type::Custom("String".to_string()))],
                output_type: Some(OutputType::Single(Type::Custom("CStr".to_string()))),
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    span: None,
                    explicit: false,
                post_authority: false},
                body: vec![Statement::Term(Some(Expr::Cast(
                    Box::new(Expr::Identifier("s".to_string())),
                    Type::Custom("CStr".to_string()),
                )))],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                annotations: vec![],
                span: None,
                doc: None,
            }),
        ];
        rewrite_boundary_marshalling(&mut items, &universe);
        let TopLevel::Definition(d) = &items[2] else { panic!() };
        let Statement::Term(Some(expr)) = &d.body[0] else { panic!() };
        match expr {
            Expr::Call(name, args, _) => {
                assert_eq!(name, "str_to_c", "String→CStr must emit str_to_c (zero-copy)");
                assert!(matches!(args[0], Expr::Identifier(ref n) if n == "s"));
            }
            other => panic!("expected a str_to_c binding call, got {:?}", other),
        }
    }
}
