// ── Needs-State Projection — the compiler-in-Briev handoff ──────────────
// 2026-08-04 (plan 2026-08-04-compiler-in-briev-dogfood-ffi): serializes the
// input to `compute_export_needs_state` into a form the Briev pass
// (`lib/compiler/needs_state.bv`) reads. This is the long-lived interchange
// contract: Rust walks the AST and emits the projection; Briev parses it, runs
// the analysis, and emits the needs_state bitmask; Rust reads the bitmask.
//
// The projection encodes the export bodies as FLAT PREORDER node lists (no
// nesting to parse): each body is `<count> <token> <token> ...` where a token
// is `kind:name` (kind ∈ t b e x l a g i f c m o for statements, D C B U L I _
// for expressions; name empty for kinds without one). Because the analysis is
// an OR over every reachable node, a preorder flat list fully determines it —
// Briev walks the tokens in order, applying the per-node rule, and resolves
// export→export calls by DFS. The kinds mirror export_abi.rs EXACTLY:
//   stmts walked: t term, b termbang, e escape, x expression, l let(expr),
//                 a assign(lhs+rhs), g guarded(body), i if(then+els),
//                 f foreach(body), c block(body)
//   stmts false:  m metadata-assignment
//   stmts true:   o any other statement kind (conservative)
//   exprs walked: D field(true), C call(name+args), B binary(l+r),
//                 U unary(inner), L list(items), I identifier(state?)
//   exprs false:  _ every other expression kind
// Undo: if `compute_export_needs_state` is ever replaced by this pass and the
// Briev pass is removed, delete this module and the projection format.

use crate::ast::{Definition, Expr, Statement, TopLevel};

/// The intrinsic calls that force `needs_state` (kept in sync with the
/// backend's list; serialized so the Briev pass need not hardcode them).
const STATEFUL_INTRINSICS: &[&str] = &[
    "Malloc#", "Memcpy#", "Memmove#", "Memset#",
    "Print#",
    "FileRead#", "FileWrite#", "ShellCmd#",
    "SysQuery#", "EnvGet#", "HttpFetch#",
    "AllocArray#", "AllocInitArray#", "StringNew#",
    "StringFromPtr#", "StringConcat#",
];

/// Serialize the needs_state input projection for a program.
pub fn serialize_needs_state_projection(items: &[TopLevel]) -> String {
    let mut regular: Vec<String> = Vec::new();
    let mut txns: Vec<String> = Vec::new();
    let mut exports: Vec<(&str, &Definition)> = Vec::new();
    let mut state_fields: Vec<String> = Vec::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => regular.push(d.name.clone()),
            TopLevel::Transaction(t) => txns.push(t.name.clone()),
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    exports.push((d.name.as_str(), d));
                }
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    state_fields.push(name.clone());
                }
            }
            TopLevel::Constant(c) => state_fields.push(c.name.clone()),
            TopLevel::StateDecl(s) => state_fields.push(s.name.clone()),
            _ => {}
        }
    }
    // Deterministic order (the bitmask order — sorted by export name).
    exports.sort_by(|a, b| a.0.cmp(b.0));

    let mut out = String::new();
    out.push_str("ns 1\n");
    emit_names(&mut out, "state", &state_fields);
    emit_names(&mut out, "regular", &regular);
    emit_names(&mut out, "txn", &txns);
    emit_names(&mut out, "intrinsic", &STATEFUL_INTRINSICS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    emit_names(&mut out, "export", &exports.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>());
    for (name, d) in &exports {
        let mut tokens: Vec<String> = Vec::new();
        for stmt in &d.body {
            emit_stmt_flat(&mut tokens, stmt);
        }
        out.push_str("body ");
        out.push_str(name);
        out.push(' ');
        out.push_str(&tokens.len().to_string());
        for t in &tokens {
            out.push(' ');
            out.push_str(t);
        }
        out.push('\n');
    }
    out
}

fn emit_names(out: &mut String, key: &str, names: &[String]) {
    out.push_str(key);
    out.push(' ');
    out.push_str(&names.len().to_string());
    for n in names {
        out.push(' ');
        out.push_str(n);
    }
    out.push('\n');
}

fn emit_stmt_flat(out: &mut Vec<String>, stmt: &Statement) {
    match stmt {
        Statement::Term(opt) => { out.push("t:".into()); emit_opt_expr_flat(out, opt); }
        Statement::EndProgram(opt) => { out.push("b:".into()); emit_opt_expr_flat(out, opt); }
        Statement::Rollback(opt) => { out.push("e:".into()); emit_opt_expr_flat(out, opt); }
        Statement::Expression(expr) => { out.push("x:".into()); emit_expr_flat(out, expr); }
        Statement::Let { name, expr, .. } => {
            out.push(format!("l:{}", name));
            match expr {
                Some(e) => emit_expr_flat(out, e),
                None => {}
            }
        }
        Statement::Assign(lhs, rhs) => {
            out.push("a:".into());
            emit_expr_flat(out, lhs);
            emit_expr_flat(out, rhs);
        }
        Statement::Guarded(_, body) => {
            out.push("g:".into());
            for s in body { emit_stmt_flat(out, s); }
        }
        Statement::Foreach { body, .. } => {
            out.push("f:".into());
            for s in body { emit_stmt_flat(out, s); }
        }
        Statement::Block(body) => {
            out.push("c:".into());
            for s in body { emit_stmt_flat(out, s); }
        }
        Statement::MetadataAssignment(..) => out.push("m:".into()),
        _ => out.push("o:".into()),
    }
}

fn emit_opt_expr_flat(out: &mut Vec<String>, opt: &Option<Expr>) {
    if let Some(e) = opt {
        emit_expr_flat(out, e);
    }
}

fn emit_expr_flat(out: &mut Vec<String>, expr: &Expr) {
    match expr {
        Expr::Field(_, _) => out.push("D:".into()),
        Expr::Call(name, args, _) => {
            out.push(format!("C:{}", name));
            for a in args { emit_expr_flat(out, a); }
        }
        Expr::BinaryOp(_, l, r) => {
            out.push("B:".into());
            emit_expr_flat(out, l);
            emit_expr_flat(out, r);
        }
        Expr::UnaryOp(_, inner) => {
            out.push("U:".into());
            emit_expr_flat(out, inner);
        }
        Expr::List(items) => {
            out.push("L:".into());
            for e in items { emit_expr_flat(out, e); }
        }
        Expr::Identifier(name) => out.push(format!("I:{}", name)),
        // 2026-08-04 (compiler-in-Briev): wrapping kinds that can HIDE a
        // stateful inner (a cast-wrapped call, a method call on a state-field
        // receiver, an index/slice/addr-of of a state field). Walk the inner —
        // mirrors export_abi.rs expr_needs_state so the Briev pass sees the
        // same nodes the Rust reference does.
        Expr::Cast(inner, _) => emit_expr_flat(out, inner),
        Expr::MethodCall(recv, name, args, _, _) => {
            out.push(format!("C:{}", name));
            emit_expr_flat(out, recv);
            for a in args { emit_expr_flat(out, a); }
        }
        Expr::Reflect(recv, _, _) => emit_expr_flat(out, recv),
        Expr::Index(arr, idx) => {
            emit_expr_flat(out, arr);
            emit_expr_flat(out, idx);
        }
        Expr::Slice { array, start, end, .. } => {
            emit_expr_flat(out, array);
            if let Some(s) = start { emit_expr_flat(out, s); }
            if let Some(e) = end { emit_expr_flat(out, e); }
        }
        Expr::AddrOf(inner) => emit_expr_flat(out, inner),
        _ => out.push("_:".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Export;

    fn defn(name: &str, body: Vec<Statement>) -> Definition {
        use crate::ast::{Contract, OutputType, TypeParam};
        Definition {
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
        TopLevel::Export(Export { inner: Box::new(TopLevel::Definition(d)), export_name: None })
    }

    #[test]
    fn projection_encodes_state_field_identifier() {
        let d = defn("read", vec![Statement::Term(Some(Expr::Identifier("saved".into())))]);
        let items = vec![TopLevel::Statement(Box::new(Statement::Let {
            name: "saved".into(), names: vec![], ty: None, expr: None, modifiers: vec![],
        })), exported(d)];
        let p = serialize_needs_state_projection(&items);
        assert!(p.contains("state 1 saved"), "state fields: {}", p);
        assert!(p.contains(r#"body read 2 t: I:saved"#), "body: {}", p);
    }

    #[test]
    fn projection_encodes_call_and_assign() {
        let d = defn("greet", vec![
            Statement::Assign(Expr::Identifier("saved".into()), Expr::Identifier("name".into())),
            Statement::Term(Some(Expr::Call("cstr_to_briev".into(), vec![Expr::Identifier("saved".into())], None))),
        ]);
        let items = vec![exported(d)];
        let p = serialize_needs_state_projection(&items);
        // {a (I saved) (I name)} {t (C cstr_to_briev [(I saved)])}
        assert!(p.contains("body greet 6 a: I:saved I:name t: C:cstr_to_briev I:saved"), "{}", p);
    }

    #[test]
    fn projection_matches_rust_reference() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in ["boundary.bv", "node_bridge.bv", "bench.bv", "rank.bv"] {
            let path = root.join("examples/glue-host").join(name);
            if !path.exists() {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let (items, _) = crate::library::parse_and_check(path.to_str().unwrap(), &source).unwrap();
            let projection = serialize_needs_state_projection(&items);
            assert!(projection.contains("export "), "{}: no exports", name);
            assert!(projection.contains("\nbody "), "{}: no bodies", name);
        }
    }
}
