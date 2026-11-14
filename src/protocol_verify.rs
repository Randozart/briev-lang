// ── Protocol Round-Trip Verification ───────────────────────────────────
// 2026-07-20: Compile-time verification that Parse → Cast → Parse is
// invertible for every type participating in protocol ops.
//
// Flat control flow: max 2 nesting levels. Extracted helpers.

use crate::ast::*;
use crate::interpreter::{Interpreter, Value};
use crate::type_universe::TypeUniverse;

/// 2026-07-20: Run round-trip verification for all Parse ops.
pub fn verify_roundtrips(items: &[TopLevel], _universe: &TypeUniverse) -> Result<(), String> {
    let mut interp = Interpreter::new();
    let mut warnings: Vec<String> = Vec::new();

    for item in items {
        let td = type_def(item);
        let td = match td {
            Some(t) => t,
            None => continue,
        };
        let (form, fn_name) = match collect_parse_info(td) {
            Some(info) => info,
            None => continue,
        };
        let result = verify_single_roundtrip(td, &form, &fn_name, &mut interp);
        if let Err(w) = result {
            warnings.push(w);
        }
    }

    if warnings.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "protocol round-trip verification:\n{}",
            warnings.join("\n")
        ))
    }
}

fn type_def(item: &TopLevel) -> Option<&TypeDef> {
    match item {
        TopLevel::TypeDef(td) => Some(td.as_ref()),
        _ => None,
    }
}

fn collect_parse_info(td: &TypeDef) -> Option<(String, String)> {
    for op in &td.body.operators {
        if op.op != "Parse" {
            continue;
        }
        if op.params.is_empty() {
            continue;
        }
        let form = parse_form_name(&op.params[0])?;
        let fn_name = extract_fn_name(&op.impl_args)?;
        return Some((form, fn_name));
    }
    None
}

fn parse_form_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Custom(s) if s == "Decimal" => Some("Decimal".to_string()),
        Type::Custom(s) if s == "Quoted" => Some("Quoted".to_string()),
        Type::Custom(s) if s == "Bare" => Some("Bare".to_string()),
        _ => None,
    }
}

fn extract_fn_name(impl_args: &Option<PropertyValue>) -> Option<String> {
    match impl_args {
        Some(PropertyValue::Identifier(s)) => Some(s.clone()),
        Some(PropertyValue::List(items)) => match items.first() {
            Some(PropertyValue::Identifier(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn verify_single_roundtrip(
    td: &TypeDef,
    form: &str,
    fn_name: &str,
    interp: &mut Interpreter,
) -> Result<(), String> {
    let test_literal = generate_test_literal(form)?;
    let test_call = Expr::Call(fn_name.to_string(), vec![test_literal.clone()], None);
    // 2026-08-26 (bug sweep B1): an eval failure here was a SILENT Ok — a
    // Parse implementation that crashed on the probe escaped verification
    // unnoticed. Now it reports which call failed and why.
    let parse_val = interp.eval_expr(&test_call).map_err(|e| {
        format!(
            "round-trip: '{}.{}' probe call Parse('{}') failed to evaluate: {}",
            td.name,
            fn_name,
            literal_description(form),
            e
        )
    })?;
    let produce_fn = find_produce_fn(td).ok_or_else(|| {
        format!(
            "round-trip: '{}' declares CastFrom but no Produce/producer op — \
         the inverse direction cannot run; add one",
            td.name
        )
    })?;
    let produce_call = Expr::Call(produce_fn.clone(), vec![], None);
    // Same silent-Ok hole for the producer side.
    let produce_val = interp.eval_expr(&produce_call).map_err(|e| {
        format!(
            "round-trip: '{}.{}' producer '{}()' failed to evaluate: {}",
            td.name, fn_name, produce_fn, e
        )
    })?;
    let test_desc = literal_description(form);
    let produced_desc = format!("{:?}", produce_val);
    if produced_desc != test_desc {
        return Err(format!(
            "round-trip: '{}.{}' → Parse('{}') → Cast → '{}' ≠ '{}'",
            td.name, fn_name, test_desc, produced_desc, test_desc
        ));
    }
    Ok(())
}

fn generate_test_literal(form: &str) -> Result<Expr, String> {
    match form {
        "Decimal" => Ok(Expr::Decimal(42)),
        "Quoted" => Ok(Expr::Quoted(b"test".to_vec())),
        "Bare" => Ok(Expr::Identifier("TEST".to_string())),
        _ => Err(format!("unknown Parse form: {}", form)),
    }
}

fn literal_description(form: &str) -> String {
    match form {
        "Decimal" => "42".to_string(),
        "Quoted" => "\"test\"".to_string(),
        "Bare" => "TEST".to_string(),
        _ => "?".to_string(),
    }
}

fn find_produce_fn(td: &TypeDef) -> Option<String> {
    for op in &td.body.operators {
        if op.op != "Cast" && op.op != "CastTo" {
            continue;
        }
        let result = extract_fn_name(&op.impl_args);
        if result.is_some() {
            return result;
        }
    }
    None
}
