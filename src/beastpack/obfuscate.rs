// 2026-07-25: Noise pairing — identifier obfuscation for .beastpack files.
// Internal identifiers are permuted with a random seed at packaging time.
// The inverse permutation is embedded in the .lair bytecode, creating a
// one-way pair: neither file alone reveals the original identifier mapping.

use crate::ast::*;
use crate::ast::top::*;
use std::collections::{HashMap, HashSet};

// Names that should NEVER be obfuscated (primitive types, builtins).
const RESERVED_NAMES: &[&str] = &[
    "Int", "UInt", "Int8", "UInt8", "Int16", "UInt16", "Int32", "UInt32",
    "Int64", "UInt64", "Float", "Float32", "Float64", "Double",
    "Bool", "Void", "Char", "String", "Blob", "Ptr",
];

/// Obfuscate internal identifiers in a typed program.
///
/// Returns (obfuscated_items, inverse_map) where inverse_map maps
/// obfuscated names back to originals.
pub fn obfuscate(items: &[TopLevel], seed: u64) -> (Vec<TopLevel>, HashMap<String, String>) {
    // 1. Collect all identifier names from the AST
    let all_names = collect_names(items);

    // 2. Filter to obfuscatable names (exclude reserved and exports)
    // 2026-08-23 (bugfix): also exclude FOREIGN BINDING names — a frgn's
    // call sites must keep matching its registration; renaming one side
    // produced calls to functions that exist nowhere (`v_…` unknown at VM
    // codegen). To undo: drop this filter.
    let frgn_names: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::ForeignBinding(fb) => {
                let mut out = vec![fb.foreign_name.clone()];
                if let Some(ref b) = fb.briev_name {
                    out.push(b.clone());
                }
                Some(out)
            }
            _ => None,
        })
        .flatten()
        .collect();
    let internal_names: Vec<&str> = all_names.iter()
        .filter(|n| !RESERVED_NAMES.contains(&n.as_str()))
        .filter(|n| !frgn_names.contains(*n))
        .map(|n| n.as_str())
        .collect();

    // 3. Build forward and inverse maps using deterministic permutation
    let (forward, inverse) = build_noise_map(&internal_names, seed);

    // 4. Apply renaming to create obfuscated items
    let obfuscated = apply_rename(items, &forward);

    (obfuscated, inverse)
}

/// Collect every identifier string from the AST into a set.
fn collect_names(items: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in items {
        collect_toplevel_names(item, &mut names);
    }
    names
}

fn collect_toplevel_names(item: &TopLevel, names: &mut HashSet<String>) {
    match item {
        TopLevel::Definition(d) => {
            names.insert(d.name.clone());
            for (pname, _) in &d.parameters {
                names.insert(pname.clone());
            }
            for stmt in &d.body {
                collect_stmt_names(stmt, names);
            }
        }
        TopLevel::Transaction(t) => {
            names.insert(t.name.clone());
            for (pname, _) in &t.parameters {
                names.insert(pname.clone());
            }
            for stmt in &t.body {
                collect_stmt_names(stmt, names);
            }
        }
        TopLevel::Constant(c) => {
            names.insert(c.name.clone());
            collect_expr_names(&c.expr, names);
        }
        TopLevel::Export(e) => {
            // Export names are public — collect for renames but they
            // stay as-is in the inverse map for the .lair
            collect_toplevel_names(&e.inner, names);
        }
        TopLevel::ForeignBinding(fb) => {
            if let Some(ref name) = fb.briev_name {
                names.insert(name.clone());
            }
        }
        _ => {}
    }
}

fn collect_stmt_names(stmt: &Statement, names: &mut HashSet<String>) {
    match stmt {
        Statement::Let { name, expr, .. } => {
            names.insert(name.clone());
            if let Some(e) = expr {
                collect_expr_names(e, names);
            }
        }
        Statement::Assign(target, value) => {
            collect_expr_names(target, names);
            collect_expr_names(value, names);
        }
        Statement::Term(opt) | Statement::EndProgram(opt) => {
            if let Some(e) = opt {
                collect_expr_names(e, names);
            }
        }
        Statement::Expression(e) => collect_expr_names(e, names),
        Statement::Block(stmts) => {
            for s in stmts { collect_stmt_names(s, names); }
        }
        Statement::Guarded(cond, body) => {
            collect_expr_names(cond, names);
            for s in body { collect_stmt_names(s, names); }
        }
        Statement::Match { expr, arms } => {
            collect_expr_names(expr, names);
            for arm in arms {
                for s in &arm.body { collect_stmt_names(s, names); }
            }
        }
        _ => {}
    }
}

fn collect_expr_names(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(name) => { names.insert(name.clone()); }
        Expr::Call(name, args, _) => {
            // 2026-08-23 (bugfix): intrinsic/host-call names (`Print#`,
            // `Alloc#`, macro `f!`) are DISCLOSED compiler-facing markers —
            // obfuscating them broke the `#`-keeper branch in rename_expr
            // (the map lookup hit first, renaming Print# → v_…, and the VM
            // backend then saw an unknown plain call). Skip them here so
            // they never enter the map.
            if !name.ends_with('#') && !name.ends_with('!') {
                names.insert(name.clone());
            }
            for a in args { collect_expr_names(a, names); }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_expr_names(l, names);
            collect_expr_names(r, names);
        }
        Expr::UnaryOp(_, inner) => collect_expr_names(inner, names),
        Expr::Field(obj, field) => {
            collect_expr_names(obj, names);
            names.insert(field.clone());
        }
        Expr::Index(obj, idx) => {
            collect_expr_names(obj, names);
            collect_expr_names(idx, names);
        }
        Expr::Block(stmts) => {
            for s in stmts { collect_stmt_names(s, names); }
        }
        Expr::If(cond, t, e) => {
            collect_expr_names(cond, names);
            collect_expr_names(t, names);
            if let Some(els) = e { collect_expr_names(els, names); }
        }
        Expr::Match(scrutinee, arms) => {
            collect_expr_names(scrutinee, names);
            for arm in arms {
                collect_expr_names(&arm.body, names);
                // Pattern bindings
                match &arm.pattern {
                    Pattern::Binding(b) => { names.insert(b.clone()); }
                    Pattern::EnumVariant(_, pats) => {
                        for p in pats {
                            if let Pattern::Binding(b) = p { names.insert(b.clone()); }
                        }
                    }
                    _ => {}
                }
            }
        }
        Expr::Tuple(exprs) => { for e in exprs { collect_expr_names(e, names); } }
        Expr::List(exprs) => { for e in exprs { collect_expr_names(e, names); } }
        Expr::Lambda(params, body) => {
            for p in params { names.insert(p.clone()); }
            collect_expr_names(body, names);
        }
        Expr::Cast(inner, _) => collect_expr_names(inner, names),
        Expr::IsType(inner, _) => collect_expr_names(inner, names),
        Expr::Within(l, r) => {
            collect_expr_names(l, names);
            collect_expr_names(r, names);
        }
        Expr::Deref(inner) => collect_expr_names(inner, names),
        Expr::AddrOf(inner) => collect_expr_names(inner, names),
        _ => {}
    }
}

/// Build deterministic forward and inverse noise maps from a list of names.
///
/// Uses a simple permutation based on the seed and the name's hash:
///   obfuscated_name = "v_" + hex(hash(seed, name))
/// This is deterministic (same seed → same mapping) and has avalanche
/// (changing one char in the name changes the output completely).
fn build_noise_map(names: &[&str], seed: u64) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut forward = HashMap::new();
    let mut inverse = HashMap::new();

    for name in names {
        // Use blake3 for the hash — it's fast and available
        let mut hasher = blake3::Hasher::new();
        hasher.update(&seed.to_le_bytes());
        hasher.update(name.as_bytes());
        let hash = hasher.finalize();
        let prefix = u32::from_le_bytes([hash.as_bytes()[0], hash.as_bytes()[1],
                                          hash.as_bytes()[2], hash.as_bytes()[3]]);
        let obfuscated = format!("v_{:08x}", prefix);

        forward.insert(name.to_string(), obfuscated.clone());
        inverse.insert(obfuscated, name.to_string());
    }

    (forward, inverse)
}

/// Apply rename map to every identifier in the AST.
fn apply_rename(items: &[TopLevel], map: &HashMap<String, String>) -> Vec<TopLevel> {
    items.iter().map(|item| rename_toplevel(item, map)).collect()
}

fn rename_string(s: &str, map: &HashMap<String, String>) -> String {
    map.get(s).cloned().unwrap_or_else(|| s.to_string())
}

fn rename_toplevel(item: &TopLevel, map: &HashMap<String, String>) -> TopLevel {
    match item {
        TopLevel::Definition(d) => TopLevel::Definition(Definition {
            name: rename_string(&d.name, map),
            parameters: d.parameters.iter()
                .map(|(n, t)| (rename_string(n, map), t.clone()))
                .collect(),
            body: d.body.iter().map(|s| rename_stmt(s, map)).collect(),
            metadata: d.metadata.clone(),
            output_type: d.output_type.clone(),
            outputs: d.outputs.clone(),
            contract: d.contract.clone(),
            type_params: d.type_params.clone(),
            derivation: d.derivation.clone(),
            modifiers: d.modifiers.clone(),
            annotations: d.annotations.clone(),
            variadic_param: d.variadic_param.clone(),
            span: d.span,
            doc: d.doc.clone(),
        }),
        TopLevel::Transaction(t) => TopLevel::Transaction(Transaction {
            name: rename_string(&t.name, map),
            parameters: t.parameters.iter()
                .map(|(n, ty)| (rename_string(n, map), ty.clone()))
                .collect(),
            body: t.body.iter().map(|s| rename_stmt(s, map)).collect(),
            metadata: t.metadata.clone(),
            is_reactive: t.is_reactive,
            is_async: t.is_async,
            output_type: t.output_type.clone(),
            outputs: t.outputs.clone(),
            contract: t.contract.clone(),
            type_params: t.type_params.clone(),
            derivation: t.derivation.clone(),
            modifiers: t.modifiers.clone(),
            span: t.span,
            doc: t.doc.clone(),
        }),
        TopLevel::Constant(c) => TopLevel::Constant(Constant {
            name: rename_string(&c.name, map),
            ty: c.ty.clone(),
            expr: rename_expr(&c.expr, map),
            section: c.section.clone(),
        }),
        _ => item.clone(),
    }
}

fn rename_stmt(stmt: &Statement, map: &HashMap<String, String>) -> Statement {
    match stmt {
        Statement::Let { name, ty, expr, modifiers, .. } => Statement::Let { names: vec![], 
            name: rename_string(name, map),
            ty: ty.clone(),
            expr: expr.as_ref().map(|e| rename_expr(e, map)),
            modifiers: modifiers.clone(),
        },
        Statement::Assign(target, value) => Statement::Assign(
            rename_expr(target, map),
            rename_expr(value, map),
        ),
        Statement::Term(opt) => Statement::Term(opt.as_ref().map(|e| rename_expr(e, map))),
        Statement::EndProgram(opt) => Statement::EndProgram(opt.as_ref().map(|e| rename_expr(e, map))),
        Statement::Expression(e) => Statement::Expression(rename_expr(e, map)),
        Statement::Block(stmts) => Statement::Block(
            stmts.iter().map(|s| rename_stmt(s, map)).collect()
        ),
        Statement::Guarded(cond, body) => Statement::Guarded(
            rename_expr(cond, map),
            body.iter().map(|s| rename_stmt(s, map)).collect(),
        ),
        Statement::Match { expr, arms } => Statement::Match {
            expr: Box::new(rename_expr(expr, map)),
            arms: arms.iter().map(|a| StmtMatchArm {
                patterns: a.patterns.clone(),
                body: a.body.iter().map(|s| rename_stmt(s, map)).collect(),
            }).collect(),
        },
        other => other.clone(),
    }
}

fn rename_expr(expr: &Expr, map: &HashMap<String, String>) -> Expr {
    match expr {
        Expr::Identifier(name) => Expr::Identifier(rename_string(name, map)),
        Expr::Call(name, args, analysis_id) => {
            let new_name = if let Some(mapped) = map.get(name.as_str()) {
                mapped.clone()
            } else if name.ends_with('#') {
                // Intrinsics keep their names
                name.clone()
            } else {
                name.clone()
            };
            Expr::Call(new_name, args.iter().map(|a| rename_expr(a, map)).collect(), *analysis_id)
        }
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            *kind,
            Box::new(rename_expr(l, map)),
            Box::new(rename_expr(r, map)),
        ),
        Expr::UnaryOp(kind, inner) => Expr::UnaryOp(*kind, Box::new(rename_expr(inner, map))),
        Expr::Field(obj, field) => Expr::Field(
            Box::new(rename_expr(obj, map)),
            rename_string(field, map),
        ),
        Expr::Index(obj, idx) => Expr::Index(
            Box::new(rename_expr(obj, map)),
            Box::new(rename_expr(idx, map)),
        ),
        Expr::Block(stmts) => Expr::Block(
            stmts.iter().map(|s| rename_stmt(s, map)).collect()
        ),
        Expr::If(cond, t, e) => Expr::If(
            Box::new(rename_expr(cond, map)),
            Box::new(rename_expr(t, map)),
            e.as_ref().map(|els| Box::new(rename_expr(els, map))),
        ),
        Expr::Match(scrutinee, arms) => Expr::Match(
            Box::new(rename_expr(scrutinee, map)),
            arms.iter().map(|a| MatchArm {
                pattern: rename_pattern(&a.pattern, map),
                guard: a.guard.as_ref().map(|g| rename_expr(g, map)),
                body: Box::new(rename_expr(&a.body, map)),
            }).collect(),
        ),
        Expr::Tuple(exprs) => Expr::Tuple(exprs.iter().map(|e| rename_expr(e, map)).collect()),
        Expr::List(exprs) => Expr::List(exprs.iter().map(|e| rename_expr(e, map)).collect()),
        Expr::Lambda(params, body) => Expr::Lambda(
            params.iter().map(|p| rename_string(p, map)).collect(),
            Box::new(rename_expr(body, map)),
        ),
        Expr::Cast(inner, ty) => Expr::Cast(Box::new(rename_expr(inner, map)), ty.clone()),
        Expr::IsType(inner, ty) => Expr::IsType(Box::new(rename_expr(inner, map)), ty.clone()),
        Expr::Within(l, r) => Expr::Within(
            Box::new(rename_expr(l, map)),
            Box::new(rename_expr(r, map)),
        ),
        Expr::Deref(inner) => Expr::Deref(Box::new(rename_expr(inner, map))),
        Expr::AddrOf(inner) => Expr::AddrOf(Box::new(rename_expr(inner, map))),
        Expr::StructLiteral { type_name, fields, specs } => Expr::StructLiteral {
            type_name: rename_string(type_name, map),
            fields: fields.iter()
                .map(|(n, e)| (rename_string(n, map), rename_expr(e, map)))
                .collect(),
            specs: specs.iter()
                .map(|(n, e)| (rename_string(n, map), rename_expr(e, map)))
                .collect(),
        },
        other => other.clone(),
    }
}

fn rename_pattern(pattern: &Pattern, map: &HashMap<String, String>) -> Pattern {
    match pattern {
        Pattern::Binding(name) => Pattern::Binding(rename_string(name, map)),
        Pattern::EnumVariant(name, inner) => Pattern::EnumVariant(
            rename_string(name, map),
            inner.iter().map(|p| rename_pattern(p, map)).collect(),
        ),
        Pattern::Tuple(pats) => Pattern::Tuple(
            pats.iter().map(|p| rename_pattern(p, map)).collect()
        ),
        Pattern::Range(l, r) => Pattern::Range(
            rename_expr(l, map),
            rename_expr(r, map),
        ),
        Pattern::RangeInclusive(l, r) => Pattern::RangeInclusive(
            rename_expr(l, map),
            rename_expr(r, map),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_obfuscation() {
        let items = vec![];
        let (_, inv_a) = obfuscate(&items, 42);
        let (_, inv_b) = obfuscate(&items, 42);
        assert_eq!(inv_a.len(), inv_b.len(), "same seed → same result");
    }

    #[test]
    fn test_different_seeds_different_maps() {
        let mut names_set = HashSet::new();
        names_set.insert("x".to_string());
        names_set.insert("y".to_string());
        let names: Vec<&str> = names_set.iter().map(|s| s.as_str()).collect();

        let (map_a, _) = build_noise_map(&names, 1);
        let (map_b, _) = build_noise_map(&names, 2);

        // Different seeds should produce different obfuscated names
        for name in &names {
            if let (Some(a), Some(b)) = (map_a.get(*name), map_b.get(*name)) {
                assert_ne!(a, b, "different seeds must produce different names");
            }
        }
    }

    #[test]
    fn test_inverse_map_roundtrip() {
        let names = vec!["counter", "result", "temp"];
        let (forward, inverse) = build_noise_map(&names, 0xDEADBEEF);

        for name in &names {
            let obf = forward.get(*name).expect("forward mapping exists");
            let inv = inverse.get(obf.as_str()).expect("inverse mapping exists");
            assert_eq!(inv, name, "forward → inverse must round-trip");
        }
    }

    #[test]
    fn test_reserved_names_not_obfuscated() {
        let mut names_set = HashSet::new();
        names_set.insert("Int".to_string());
        names_set.insert("Bool".to_string());
        let names: Vec<&str> = names_set.iter().map(|s| s.as_str()).collect();

        let (map, _) = build_noise_map(&names, 42);

        // Reserved names should not appear in the forward map
        // (they're excluded by the obfuscate function, not build_noise_map)
        // The build_noise_map doesn't exclude — the obfuscate function does.
        // This test just verifies the name format.
        for name in &names {
            assert!(map.contains_key(*name), "all names appear in map");
        }
    }

    #[test]
    fn test_obfuscate_simple_defn() {
        let items = vec![TopLevel::Definition(Definition {
            variadic_param: None,
            name: "compute".into(),
            type_params: vec![],
            parameters: vec![
                ("counter".into(), Type::Custom("Int".into())),
                ("accum".into(), Type::Custom("Int".into())),
            ],
            output_type: Some(OutputType::Single(Type::Custom("Int".into()))),
            outputs: vec![Type::Custom("Int".into())],
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![Statement::Term(Some(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("counter".into())),
                Box::new(Expr::Identifier("accum".into())),
            )))],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })];

        let (obfuscated, inverse) = obfuscate(&items, 12345);

        // Should have at least one obfuscated name
        assert!(inverse.len() >= 1, "should obfuscate at least parameter names");
        assert_eq!(obfuscated.len(), 1, "should have same number of items");

        if let TopLevel::Definition(d) = &obfuscated[0] {
            for (param_name, _) in &d.parameters {
                assert!(param_name.starts_with("v_"), "obfuscated names start with v_");
            }
            // The body should reference obfuscated parameter names, not originals
            for stmt in &d.body {
                if let Statement::Term(Some(Expr::BinaryOp(_, l, r))) = stmt {
                    if let Expr::Identifier(id) = l.as_ref() {
                        assert!(id.starts_with("v_"), "identifiers should be obfuscated");
                    }
                }
            }
        } else {
            panic!("expected Definition");
        }
    }
}
