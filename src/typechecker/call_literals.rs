//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

//! Preserve inferred collection types at generic call boundaries.

use super::*;

// 2026-10-21: a generic callee's raw parameter is List<T>, even when inference
// has proved T=String. Codegen constructs literal arguments before specializing
// the call, so passing that raw type loses the element representation. Elaborate
// the literal to an ordinary typed value block using the SAME inference as call
// checking. This applies to every declared collection, not a known function name.
// Remove this bridge only when typed expressions carry their inferred types
// directly through lowering. The block stays at the argument's evaluation site;
// it neither hoists effects nor evaluates a literal twice.
pub(super) fn elaborate(
    expr: &mut Expr,
    ctx: &mut TypecheckContext,
    errors: &mut Vec<TypeError>,
) {
    let Expr::Call(name, args, _) = expr else { return };
    if !args.iter().any(|arg| matches!(arg, Expr::List(_))) { return; }
    let Some(params) = ctx.fn_type_params.get(name).cloned() else { return };
    if params.is_empty() { return; }
    let concrete = match infer_defn_type_args(name, &params, args, ctx) {
        Ok(Some(types)) => types,
        Ok(None) => return,
        Err(error) => { errors.push(error); return; }
    };
    let subst: HashMap<String, Type> = params.into_iter().zip(concrete).collect();
    let declared = ctx.fn_param_types.get(name).cloned().unwrap_or_default();
    for (arg, param) in args.iter_mut().zip(declared) {
        let ty = substitute_type(&param, &subst);
        preserve_literal_type(arg, ty, ctx);
    }
}

fn preserve_literal_type(arg: &mut Expr, ty: Type, ctx: &TypecheckContext) {
    let Expr::List(elems) = arg else { return };
    if !list_literal_accepted_by(&ty, elems, ctx) { return; }
    // '.' cannot occur in a source binding identifier, so this block-local
    // temporary cannot capture an author binding. Nested blocks finish their
    // initializer before binding their own result under the same internal name.
    let name = "$call.collection".to_string();
    let literal = arg.clone();
    *arg = Expr::Block(vec![
        Statement::Let {
            name: name.clone(), names: vec![], ty: Some(ty),
            expr: Some(literal), modifiers: vec![],
        },
        Statement::Expression(Expr::Identifier(name)),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(src: &str) -> Vec<TopLevel> {
        crate::library::parse_and_check("scratch/call_literal_regression.bv", src)
            .expect("generic collection argument must check").0
    }

    fn literal_types(items: &[TopLevel], function: &str) -> Vec<Type> {
        let mut types = vec![];
        for item in items {
            let (name, body) = match item {
                TopLevel::Definition(defn) => (&defn.name, &defn.body),
                TopLevel::Transaction(txn) => (&txn.name, &txn.body),
                _ => continue,
            };
            if name != function { continue; }
            crate::ast::visit::walk_stmts(body, &mut |expr| {
                let Expr::Block(stmts) = expr else { return };
                if let Some(Statement::Let { ty: Some(ty), expr: Some(Expr::List(_)), .. }) = stmts.first() {
                    types.push(ty.clone());
                }
            });
        }
        types
    }

    #[test]
    fn operator_generated_call_preserves_string_literal_type() {
        let items = checked(r#"
import "std/collections.bv";
import "std/iterator.bv";
defn append_text(values: List<String>, text: String) -> List<String> {
    term values + [text];
};
defn append_empty(values: List<String>) -> List<String> {
    term values + [""];
};
"#);
        assert_eq!(literal_types(&items, "append_text"),
            vec![Type::Applied("List".into(), vec![Type::string()])]);
        assert_eq!(literal_types(&items, "append_empty"),
            vec![Type::Applied("List".into(), vec![Type::string()])]);
    }

    #[test]
    fn direct_generic_call_infers_first_argument_literal() {
        let items = checked(r#"
import "std/collections.bv";
defn identity<T>(values: List<T>) -> List<T> { term values; };
defn strings() -> List<String> { term identity(["hello", ""]); };
defn integers() -> List<Int> { term identity([11, 22]); };
"#);
        assert_eq!(literal_types(&items, "strings"),
            vec![Type::Applied("List".into(), vec![Type::string()])]);
        assert_eq!(literal_types(&items, "integers"),
            vec![Type::Applied("List".into(), vec![Type::int()])]);
    }

    #[test]
    fn nested_generic_literal_call_keeps_both_concrete_types() {
        let items = checked(r#"
import "std/collections.bv";
defn identity<T>(values: List<T>) -> List<T> { term values; };
defn nested() -> List<List<String>> {
    term identity([identity(["hello"])]);
};
"#);
        let types = literal_types(&items, "nested");
        let strings = Type::Applied("List".into(), vec![Type::string()]);
        assert!(types.contains(&strings), "inner argument must remain typed: {types:?}");
        assert!(types.contains(&Type::Applied("List".into(), vec![strings])),
            "outer argument must retain its nested element type: {types:?}");
    }

    #[test]
    fn empty_tuple_accumulators_keep_declared_return_context() {
        let items = checked(r#"
import "std/collections.bv";
import "std/iterator.bv";
txn enumerate_again<T>(values: List<T>)
    [values.Count#() > 0][term.Count#() >= 0] -> List<(Int, T)>
{
    term iter_enumerate_loop(values, [], 0);
};
"#);
        let type_param = Type::Custom("T".into());
        let enumerated = Type::Applied("List".into(),
            vec![Type::Tuple(vec![Type::int(), type_param.clone()])]);
        assert_eq!(literal_types(&items, "iter_enumerate"), vec![enumerated.clone()]);
        assert_eq!(literal_types(&items, "enumerate_again"), vec![enumerated]);
        let zipped = Type::Applied("List".into(),
            vec![Type::Tuple(vec![type_param, Type::Custom("U".into())])]);
        assert_eq!(literal_types(&items, "iter_zip"), vec![zipped]);
    }
}
