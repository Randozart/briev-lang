// ── BEAST Serializer ─────────────────────────────────────────────────────
// 2026-07-14: Walk Vec<TopLevel> + TypeUniverse → .beast S-expression text.
// Every function is max 2 levels. Extract helpers for deeper logic.

use std::fmt::Write;
use crate::ast::*;
use crate::type_universe::{ResolvedType, TypeUniverse};
use super::sexpr::{to_string, Atom, SExpr};

/// Serialize a compiled program to BEAST S-expression text.
pub fn to_beast(items: &[TopLevel], universe: &TypeUniverse) -> String {
    let mut exprs: Vec<SExpr> = Vec::new();
    for rt in universe.types.values() {
        exprs.push(emit_universe(rt));
    }
    for item in items {
        exprs.push(emit_toplevel(item));
    }
    to_string(&SExpr::List(exprs))
}

fn emit_universe(rt: &ResolvedType) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom(&rt.name)];
    let maxbits = rt.bytes * 8;
    children.push(list(&[atom("maxbits"), atom(&maxbits.to_string())]));
    children.push(list(&[atom("alignment"), atom(&rt.alignment.to_string())]));
    if !rt.properties.is_empty() {
        let mut props: Vec<SExpr> = vec![atom("properties")];
        for (k, v) in &rt.properties {
            props.push(list(&[atom(k), pv_to_sexpr(v)]));
        }
        children.push(SExpr::List(props));
    }
    SExpr::List(children)
}

fn emit_toplevel(item: &TopLevel) -> SExpr {
    match item {
        TopLevel::Definition(d) => emit_definition(d),
        TopLevel::Transaction(t) => emit_transaction(t),
        TopLevel::StateDecl(s) => emit_statedecl(s),
        TopLevel::Trigger(t) => emit_trigger(t),
        TopLevel::Constant(c) => emit_constant(c),
        TopLevel::TypeDef(t) => emit_typedef(t),
        TopLevel::Init(i) => emit_init(i),
        _ => list(&[atom("toplevel"), atom(&format!("{:?}", item))]),
    }
}

/// Serialize an `init` declaration as:
/// `(init NAME (:bound ...)? TYPE (=expr EXPR)? STMT*)`
/// 2026-08-09: runtime-seeded invariant. The bound set is optional.
fn emit_init(i: &crate::ast::InitDecl) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("init"), atom(&i.name)];
    if let Some(bound) = &i.bound {
        let mut b = vec![atom(":bound")];
        emit_bound_set_into(bound, &mut b);
        children.push(SExpr::List(b));
    }
    children.push(emit_type(&i.ty));
    if let Some(value) = &i.value {
        children.push(SExpr::List(vec![atom("=expr"), emit_expr(value)]));
    }
    for s in &i.body {
        children.push(emit_statement(s));
    }
    SExpr::List(children)
}

fn emit_bound_set_into(bound: &crate::ast::BoundSpec, out: &mut Vec<SExpr>) {
    fn emit_term(t: &crate::ast::BoundTerm) -> SExpr {
        match t {
            crate::ast::BoundTerm::Lit(n) => list(&[atom(":lit"), atom(&n.to_string())]),
            crate::ast::BoundTerm::Ref(n) => list(&[atom(":ref"), atom(n)]),
        }
    }
    match bound {
        crate::ast::BoundSpec::Single(t) => out.push(list(&[atom(":single"), emit_term(t)])),
        crate::ast::BoundSpec::Range(lo, hi) => {
            out.push(list(&[atom(":range"), emit_term(lo), emit_term(hi)]));
        }
        crate::ast::BoundSpec::Choice(parts) => {
            let mut inner = vec![atom(":choice")];
            for p in parts {
                emit_bound_set_into(p, &mut inner);
            }
            out.push(SExpr::List(inner));
        }
    }
}

fn emit_definition(d: &Definition) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("defn"), atom(&d.name), emit_params(&d.parameters)];
    children.push(emit_outputs(&d.outputs));
    // 2026-07-15: Serialize contract (including is_entry) for BEAST round-trip
    children.push(emit_contract(&d.contract));
    for (k, v) in &d.metadata {
        children.push(list(&[atom("metadata"), atom(k), pv_to_sexpr(v)]));
    }
    children.push(list(&[atom("body")]));
    for s in &d.body {
        children.push(emit_statement(s));
    }
    SExpr::List(children)
}

fn emit_transaction(t: &Transaction) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("txn"), atom(&t.name)];
    if t.is_reactive { children.push(atom(":reactive")); }
    if t.is_async { children.push(atom(":async")); }
    children.push(emit_params(&t.parameters));
    children.push(emit_contract(&t.contract));
    for (k, v) in &t.metadata {
        children.push(list(&[atom("metadata"), atom(k), pv_to_sexpr(v)]));
    }
    children.push(list(&[atom("body")]));
    for s in &t.body {
        children.push(emit_statement(s));
    }
    SExpr::List(children)
}

fn emit_contract(c: &Contract) -> SExpr {
    let mut children = vec![atom("contract")];
    children.push(list(&[atom("pre"), emit_expr(&c.pre_condition)]));
    children.push(list(&[atom("post"), emit_expr(&c.post_condition)]));
    if c.post_authority {
        children.push(atom("post_authority"));
    }
    SExpr::List(children)
}

fn emit_statedecl(s: &StateDecl) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("state"), atom(&s.name), emit_type(&s.ty)];
    SExpr::List(children)
}

fn emit_trigger(t: &Trigger) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("trigger"), atom(&t.name)];

    SExpr::List(children)
}

fn emit_constant(c: &Constant) -> SExpr {
    list(&[atom("constant"), atom(&c.name), emit_type(&c.ty), emit_expr(&c.expr)])
}

fn emit_typedef(t: &TypeDef) -> SExpr {
    let mut children: Vec<SExpr> = vec![atom("typedef"), atom(&t.name)];
    if let Some(parent) = &t.parent {
        children.push(list(&[atom("parent"), emit_expr(parent)]));
    }
    if let Some(protocol) = &t.protocol {
        children.push(list(&[atom("protocol"), atom(protocol)]));
    }
    let mut slots: Vec<SExpr> = vec![atom("slots")];
    for slot in &t.body.slots {
        slots.push(list(&[atom("slot"), atom(&slot.name), emit_type(&slot.ty)]));
    }
    children.push(SExpr::List(slots));
    // 2026-09-11 (Part C): first-class component pins round-trip — the
    // analysis and KiCad backend read them from the AST.
    if !t.body.pins.is_empty() {
        let mut pins: Vec<SExpr> = vec![atom("pins")];
        for p in &t.body.pins {
            pins.push(list(&[
                atom("pin"),
                atom(&p.name),
                atom(&p.number.to_string()),
            ]));
        }
        children.push(SExpr::List(pins));
    }
    for (k, v) in &t.body.metadata {
        children.push(list(&[atom("metadata"), atom(k), pv_to_sexpr(v)]));
    }
    SExpr::List(children)
}

fn emit_statement(s: &Statement) -> SExpr {
    match s {
        Statement::Assign(lhs, rhs) => {
            list(&[atom("assign"), emit_expr(lhs), emit_expr(rhs)])
        }
        Statement::Let { name, expr, .. } => {
            let mut children = vec![atom("let"), atom(name)];
            if let Some(e) = expr {
                children.push(emit_expr(e));
            }
            SExpr::List(children)
        }
        Statement::Term(e) => {
            match e {
                Some(e) => list(&[atom("term"), emit_expr(e)]),
                None => list(&[atom("term")]),
            }
        }
        Statement::EndProgram(e) => {
            match e {
                Some(e) => list(&[atom("term!"), emit_expr(e)]),
                None => list(&[atom("term!")]),
            }
        }
        // 2026-08-22 (Phase 8): yield; round-trips through beast snapshots.
        Statement::Yield => list(&[atom("yield")]),
        Statement::Expression(e) => list(&[atom("expr"), emit_expr(e)]),        Statement::Guarded(cond, body) => {
            let mut children = vec![atom("guarded"), emit_expr(cond)];
            children.push(list(&[atom("body")]));
            for s in body { children.push(emit_statement(s)); }
            SExpr::List(children)
        }
        Statement::Gate(cond) => list(&[atom("gate"), emit_expr(cond)]),
        Statement::Block(body) => {
            let mut children = vec![atom("block")];
            for s in body { children.push(emit_statement(s)); }
            SExpr::List(children)
        }
        Statement::MetadataAssignment(key, val) => {
            list(&[atom("metadata"), atom(key), pv_to_sexpr(val)])
        }
        _ => list(&[atom("statement"), atom(&format!("{:?}", s))]),
    }
}

fn emit_expr(e: &Expr) -> SExpr {
    match e {
        Expr::Decimal(n) => SExpr::Atom(Atom::Int(*n)),
        Expr::Float(f) => SExpr::Atom(Atom::Float(*f)),
        Expr::Bool(b) => SExpr::Atom(Atom::Bool(*b)),
        Expr::Quoted(bytes) => {
            let s = String::from_utf8_lossy(bytes).to_string();
            list(&[atom("string"), atom(&s)])
        }
        Expr::Identifier(name) => list(&[atom("ident"), atom(name)]),
        Expr::Call(name, args, _) => {
            let mut children = vec![atom("call"), atom(name)];
            for a in args { children.push(emit_expr(a)); }
            SExpr::List(children)
        }
        Expr::BinaryOp(kind, l, r) => {
            list(&[atom("binop"), atom(&format!("{:?}", kind)), emit_expr(l), emit_expr(r)])
        }
        Expr::UnaryOp(kind, inner) => {
            list(&[atom("unop"), atom(&format!("{:?}", kind)), emit_expr(inner)])
        }
        Expr::Field(obj, name) => {
            list(&[atom("field"), emit_expr(obj), atom(name)])
        }
        Expr::Reflect(recv, target, kind) => {
            let kind_atom = match kind {
                ReflectKind::Runtime => "reflect_runtime",
                ReflectKind::CompileTime => "reflect_compile",
            };
            list(&[atom(kind_atom), emit_expr(recv), atom(target)])
        }
        Expr::MethodCall(recv, name, args, _, refs) => {
            let mut children = vec![atom("method"), emit_expr(recv), atom(name)];
            children.push(emit_chain_refs(refs));
            for a in args { children.push(emit_expr(a)); }
            SExpr::List(children)
        }
        Expr::Capture { expr, name } => {
            list(&[atom("capture"), emit_expr(expr), atom(name)])
        }
        Expr::PluginIntercept { name, args, type_args: _, receiver, chain_refs } => {
            let mut children = vec![atom("plugin"), atom(name)];
            match receiver {
                Some(r) => children.push(emit_expr(r)),
                None => children.push(list(&[atom("none")])),
            }
            children.push(emit_chain_refs(chain_refs));
            for a in args { children.push(emit_expr(a)); }
            SExpr::List(children)
        }
        Expr::Index(obj, idx) => {
            list(&[atom("index"), emit_expr(obj), emit_expr(idx)])
        }
        Expr::Tuple(items) => {
            let mut children = vec![atom("tuple")];
            for item in items { children.push(emit_expr(item)); }
            SExpr::List(children)
        }
        Expr::List(items) => {
            let mut children = vec![atom("list")];
            for item in items { children.push(emit_expr(item)); }
            SExpr::List(children)
        }
        Expr::Cast(expr, ty) => {
            list(&[atom("cast"), emit_expr(expr), emit_type(ty)])
        }
        Expr::Deref(inner) => {
            list(&[atom("deref"), emit_expr(inner)])
        }
        Expr::AddrOf(inner) => {
            list(&[atom("addrof"), emit_expr(inner)])
        }
        Expr::If(cond, t, f) => {
            let mut children = vec![atom("if"), emit_expr(cond), emit_expr(t)];
            if let Some(fe) = f { children.push(emit_expr(fe)); }
            SExpr::List(children)
        }
        _ => atom(&format!("{:?}", e)),
    }
}

fn emit_type(ty: &Type) -> SExpr {
    atom(&format!("{}", ty))
}

/// 2026-09-16 (Bug D): serialize a chain's back-references as
/// `(refs (pos N) (named X) ...)`.
fn emit_chain_refs(refs: &[ChainRef]) -> SExpr {
    let mut items = vec![atom("refs")];
    for r in refs {
        match r {
            ChainRef::Positional(n) => {
                items.push(list(&[atom("pos"), SExpr::Atom(Atom::Int(*n as i64))]))
            }
            ChainRef::Named(name) => items.push(list(&[atom("named"), atom(name)])),
        }
    }
    SExpr::List(items)
}

fn emit_params(params: &[(String, Type)]) -> SExpr {
    let mut children = vec![atom("params")];
    for (name, ty) in params {
        children.push(list(&[atom("param"), atom(name), emit_type(ty)]));
    }
    SExpr::List(children)
}

fn emit_outputs(outputs: &[Type]) -> SExpr {
    let mut children = vec![atom("outputs")];
    for ty in outputs {
        children.push(emit_type(ty));
    }
    SExpr::List(children)
}

fn pv_to_sexpr(pv: &PropertyValue) -> SExpr {
    match pv {
        PropertyValue::Identifier(s) => atom(s),
        PropertyValue::String(s) => atom(s),
        PropertyValue::Int(n) => SExpr::Atom(Atom::Int(*n)),
        PropertyValue::Float(f) => SExpr::Atom(Atom::Float(*f)),
        PropertyValue::Bool(b) => SExpr::Atom(Atom::Bool(*b)),
        PropertyValue::List(items) => {
            let mut children = vec![atom("list")];
            for item in items { children.push(pv_to_sexpr(item)); }
            SExpr::List(children)
        }
        PropertyValue::HashL => atom("#Lh"),
        PropertyValue::HashR => atom("#Rh"),
        PropertyValue::HashT => atom("#T"),
    }
}

fn atom(s: &str) -> SExpr {
    SExpr::Atom(Atom::String(s.to_string()))
}

fn list(items: &[SExpr]) -> SExpr {
    SExpr::List(items.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beast::from_beast;

    #[test]
    fn test_roundtrip_simple() {
        let items = vec![
            TopLevel::StateDecl(StateDecl {
                name: "counter".into(), ty: Type::int(), span: None,
            }),
        ];
        let universe = TypeUniverse::new();
        let ir = to_beast(&items, &universe);
        let (restored, _) = from_beast(&ir).unwrap();
        assert_eq!(items.len(), restored.len());
        match (&items[0], &restored[0]) {
            (TopLevel::StateDecl(a), TopLevel::StateDecl(b)) => {
                assert_eq!(a.name, b.name);
            }
            _ => panic!("expected StateDecl"),
        }
    }

    #[test]
    fn test_roundtrip_typedef_pins() {
        // 2026-09-11 (Part C): component pins survive the BEAST round-trip —
        // the analysis and KiCad backend read them from the AST.
        let items = vec![TopLevel::TypeDef(Box::new(TypeDef {
            name: "Resistor".into(),
            type_params: vec![],
            parent: None,
            protocol: None,
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: vec![],
            ports_out: vec![],
            seq: false,
            body: TypeDefBody {
                reference: None,
                tolerance: None,
            rating: None,
                slots: vec![TypeDefSlot { name: "r".into(), ty: Type::int(), bit_range: None }],
                pins: vec![
                    crate::ast::top::PinDecl { name: "a".into(), number: 1, span: None },
                    crate::ast::top::PinDecl { name: "b".into(), number: 7, span: None },
                ],
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("reference".to_string(), crate::ast::PropertyValue::String("R".to_string()));
                    m
                },
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        }))];
        let universe = TypeUniverse::new();
        let ir = to_beast(&items, &universe);
        let (restored, _) = from_beast(&ir).unwrap();
        match (&items[0], &restored[0]) {
            (TopLevel::TypeDef(a), TopLevel::TypeDef(b)) => {
                assert_eq!(a.body.pins.len(), b.body.pins.len());
                assert_eq!(b.body.pins[0].name, "a");
                assert_eq!(b.body.pins[0].number, 1);
                assert_eq!(b.body.pins[1].name, "b");
                assert_eq!(b.body.pins[1].number, 7);
                // 2026-09-11: slots + metadata round-trip restored — the old
                // flat-parts parse loop never matched the nested emit shape.
                assert_eq!(b.body.slots.len(), 1);
                assert_eq!(b.body.slots[0].name, "r");
                assert_eq!(
                    b.body.metadata.get("reference"),
                    Some(&crate::ast::PropertyValue::String("R".to_string()))
                );
            }
            _ => panic!("expected TypeDef"),
        }
    }

    #[test]
    fn test_serialize_deref() {
        let expr = Expr::Deref(Box::new(Expr::Identifier("ptr".into())));
        let sexpr = emit_expr(&expr);
        let s = to_string(&sexpr);
        assert!(s.contains("deref"));
        assert!(s.contains("ptr"));
    }

    #[test]
    fn test_contract_roundtrip() {
        // 2026-08-01 (Phase 2): is_entry removed — a contract round-trips as
        // pre/post only (no `(entry)` atom). Verifies the BEAST serializer no
        // longer emits the removed marker.
        let contract = Contract {
            pre_condition: Expr::Decimal(1),
            post_condition: Expr::Bool(true),
            watchdog: None,
            explicit: true,
            post_authority: false,
            span: None,
        };
        let items = vec![
            TopLevel::Definition(Definition {
                variadic_param: None,
                name: "main".into(),
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![Type::int()],
                contract,
                body: vec![],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                annotations: vec![],
                span: None,
                doc: None,
            }),
        ];
        let universe = TypeUniverse::new();
        let ir = to_beast(&items, &universe);
        assert!(
            !ir.contains("entry"),
            "BEAST output must not contain the removed (entry) marker; got:\n{ir}"
        );
        let (restored, _) = from_beast(&ir).unwrap();
        assert_eq!(items.len(), restored.len());
        match &restored[0] {
            TopLevel::Definition(d) => {
                assert_eq!(d.contract.pre_condition, Expr::Decimal(1));
                assert_eq!(d.contract.post_condition, Expr::Bool(true));
            }
            _ => panic!("expected Definition"),
        }
    }

    #[test]
    fn test_roundtrip_deref() {
        let expr = Expr::Deref(Box::new(Expr::Identifier("x".into())));
        let sexpr = emit_expr(&expr);
        let s = to_string(&sexpr);
        let tokens = crate::beast::sexpr::tokenize(&s).unwrap();
        let parsed = crate::beast::sexpr::parse(&tokens).unwrap();
        let restored = crate::beast::deserialize::parse_expr(&parsed).unwrap();
        assert_eq!(expr, restored);
    }

    #[test]
    fn test_roundtrip_chain_backref_and_capture() {
        // 2026-09-16 (Bug D): BEAST must round-trip a MethodCall carrying chain
        // back-references, a PluginIntercept receiver, and a Capture — the
        // serializer previously dropped the 5th field and routed Capture to a
        // Debug atom.
        let method = Expr::MethodCall(
            Box::new(Expr::Identifier("c".into())),
            "pick".into(),
            vec![Expr::Decimal(1)],
            None,
            vec![ChainRef::Positional(2), ChainRef::Named("mid".into())],
        );
        let s = to_string(&emit_expr(&method));
        let tokens = crate::beast::sexpr::tokenize(&s).unwrap();
        let parsed = crate::beast::sexpr::parse(&tokens).unwrap();
        let restored = crate::beast::deserialize::parse_expr(&parsed).unwrap();
        assert_eq!(method, restored, "MethodCall chain refs must round-trip");

        let cap = Expr::Capture {
            expr: Box::new(Expr::Decimal(42)),
            name: "mid".into(),
        };
        let s = to_string(&emit_expr(&cap));
        let tokens = crate::beast::sexpr::tokenize(&s).unwrap();
        let parsed = crate::beast::sexpr::parse(&tokens).unwrap();
        let restored = crate::beast::deserialize::parse_expr(&parsed).unwrap();
        assert_eq!(cap, restored, "Capture must round-trip");

        let plugin = Expr::PluginIntercept {
            name: "print".into(),
            args: vec![Expr::Decimal(1)],
            type_args: vec![],
            receiver: Some(Box::new(Expr::Identifier("obj".into()))),
            chain_refs: vec![],
        };
        let s = to_string(&emit_expr(&plugin));
        let tokens = crate::beast::sexpr::tokenize(&s).unwrap();
        let parsed = crate::beast::sexpr::parse(&tokens).unwrap();
        let restored = crate::beast::deserialize::parse_expr(&parsed).unwrap();
        assert_eq!(plugin, restored, "PluginIntercept receiver must round-trip");
    }

    #[test]
    fn test_roundtrip_init() {
        // 2026-08-09: init decl with a bounded value set round-trips through
        // BEAST preserving name, bound, type, and seeding expr.
        let items = vec![
            TopLevel::Init(crate::ast::InitDecl {
                name: "BufSize".into(),
                bound: Some(crate::ast::BoundSpec::Choice(vec![
                    crate::ast::BoundSpec::Single(crate::ast::BoundTerm::Lit(64)),
                    crate::ast::BoundSpec::Range(
                        crate::ast::BoundTerm::Lit(128),
                        crate::ast::BoundTerm::Ref("LO".into()),
                    ),
                ])),
                ty: Type::int(),
                value: Some(Expr::Identifier("compute".into())),
                body: vec![],
                span: None,
                doc: None,
            }),
        ];
        let universe = TypeUniverse::new();
        let ir = to_beast(&items, &universe);
        let (restored, _) = from_beast(&ir).unwrap();
        match &restored[0] {
            TopLevel::Init(i) => {
                assert_eq!(i.name, "BufSize");
                assert_eq!(i.ty, Type::int());
                assert_eq!(i.value, Some(Expr::Identifier("compute".into())));
                assert_eq!(
                    i.bound,
                    Some(crate::ast::BoundSpec::Choice(vec![
                        crate::ast::BoundSpec::Single(crate::ast::BoundTerm::Lit(64)),
                        crate::ast::BoundSpec::Range(
                            crate::ast::BoundTerm::Lit(128),
                            crate::ast::BoundTerm::Ref("LO".into()),
                        ),
                    ]))
                );
            }
            _ => panic!("expected Init"),
        }
    }
}
