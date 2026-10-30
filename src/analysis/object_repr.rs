// ── Object caller-destination analysis ────────────────────────────────
// 2026-10-30 (D4 slice 4, plan
// docs/plans/2026-10-30-generalized-value-destination-ownership.md): prove when
// an internal object-returning defn can write its result into a caller-owned
// hidden slot instead of heap-allocating inside the callee.
//
// Two facts are required:
// 1. every return form in the defn is a direct object literal, so emission can
//    target the hidden image;
// 2. every call to that defn is immediately consumed by a non-retaining
//    parameter: the consumer only projects fields, or passes the value to an
//    enum constructor whose object payload is inline (a byte copy).
// Anything else keeps the boxed ABI.

use crate::ast::{Expr, OutputType, Statement, TopLevel, Type};
use crate::analysis::enum_layout::EnumLayoutPlanner;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Default)]
pub struct ObjectRepresentationPlan {
    /// Defn names approved for the hidden object destination ABI.
    pub caller_destination_defns: BTreeSet<String>,
}

pub fn build_object_representation_plan(
    items: &[TopLevel],
    layouts: &EnumLayoutPlanner,
) -> ObjectRepresentationPlan {
    let objects = collect_object_names(items);
    let mut candidates = BTreeMap::new();
    for item in items {
        if let Some((name, object, body)) = object_return_definition(item, &objects) {
            if returns_all_object_literals(&body, &object) && !direct_recursive(item, &name) {
                candidates.insert(name.clone(), (object, body.clone()));
            }
        }
    }

    let mut safe = BTreeSet::new();
    for name in candidates.keys() {
        if all_calls_are_consumed(items, name, &candidates[name].0, layouts) {
            safe.insert(name.clone());
        }
    }
    ObjectRepresentationPlan {
        caller_destination_defns: safe,
    }
}

fn collect_object_names(items: &[TopLevel]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in items {
        match item {
            TopLevel::TypeDef(td) => {
                let is_enum = td
                    .body
                    .slots
                    .iter()
                    .any(|slot| slot.name.starts_with("__variant_"));
                if !is_enum && !td.coll && td.parent.is_none() && td.protocol.is_none() {
                    out.insert(td.name.clone());
                }
            }
            TopLevel::StaticStruct(s) => {
                out.insert(s.name.clone());
            }
            TopLevel::Export(export) => {
                if let Some(name) = collect_object_names(std::slice::from_ref(export.inner.as_ref()))
                    .into_iter()
                    .next()
                {
                    out.insert(name);
                }
            }
            _ => {}
        }
    }
    out
}

fn object_return_definition(
    item: &TopLevel,
    objects: &BTreeSet<String>,
) -> Option<(String, String, Vec<Statement>)> {
    match item {
        TopLevel::Definition(d) => declared_object(&d.output_type, objects)
            .map(|object| (d.name.clone(), object, d.body.clone())),
        TopLevel::Export(export) => object_return_definition(export.inner.as_ref(), objects),
        _ => None,
    }
}

fn declared_object(output: &Option<OutputType>, objects: &BTreeSet<String>) -> Option<String> {
    output
        .as_ref()
        .and_then(|output| output.all_types().into_iter().next())
        .and_then(|ty| ty.universe_key().map(str::to_string))
        .filter(|name| objects.contains(name))
}

fn direct_recursive(item: &TopLevel, name: &str) -> bool {
    let TopLevel::Definition(d) = item else {
        return false;
    };
    let mut found = false;
    crate::ast::visit::walk_stmts(&d.body, &mut |expr| {
        found |= matches!(expr, Expr::Call(callee, _, _) if callee == name);
    });
    found
}

fn returns_all_object_literals(body: &[Statement], object: &str) -> bool {
    let mut saw_return = false;
    let mut pending: Vec<&[Statement]> = vec![body];
    while let Some(body) = pending.pop() {
        if !body_returns_object_literals(body, object, &mut saw_return, &mut pending) {
            return false;
        }
    }
    saw_return
}

fn body_returns_object_literals<'a>(
    body: &'a [Statement],
    object: &str,
    saw_return: &mut bool,
    pending: &mut Vec<&'a [Statement]>,
) -> bool {
    for statement in body {
        match statement {
            Statement::Term(Some(expr)) | Statement::Tail(expr) => {
                *saw_return = true;
                if !matches!(
                    expr,
                    Expr::StructLiteral { type_name, .. } if type_name == object
                ) {
                    return false;
                }
            }
            Statement::Match { arms, .. } => push_match_return_bodies(arms, pending),
            statement => {
                if let Some(inner) = nested_statement_body(statement) {
                    pending.push(inner);
                }
            }
        }
    }
    true
}

fn push_match_return_bodies<'a>(
    arms: &'a [crate::ast::StmtMatchArm],
    pending: &mut Vec<&'a [Statement]>,
) {
    for arm in arms {
        pending.push(&arm.body);
    }
}

fn nested_statement_body(statement: &Statement) -> Option<&[Statement]> {
    match statement {
        Statement::Guarded(_, inner)
        | Statement::Block(inner)
        | Statement::SyncBlock(inner)
        | Statement::Mutex(inner)
        | Statement::Defer(inner)
        | Statement::Foreach { body: inner, .. } => Some(inner),
        _ => None,
    }
}

struct CallUseProof<'a> {
    candidate: &'a str,
    object: &'a str,
    layouts: &'a EnumLayoutPlanner,
    definitions: BTreeMap<String, &'a crate::ast::Definition>,
    saw_call: bool,
}

impl<'a> CallUseProof<'a> {
    fn definition_is_clean(&mut self, body: &[Statement]) -> bool {
        body.iter().all(|statement| self.statement_is_clean(statement))
    }

    fn statement_is_clean(&mut self, statement: &Statement) -> bool {
        match statement {
            Statement::Term(Some(expr))
            | Statement::Tail(expr)
            | Statement::Expression(expr)
            | Statement::Check(expr) => self.expression_is_clean(expr),
            Statement::Assign(target, value) => {
                self.expression_is_clean(target) && self.expression_is_clean(value)
            }
            Statement::ArrowAssign { target, value, .. } => {
                target
                    .as_ref()
                    .is_none_or(|target| self.expression_is_clean(target))
                    && self.expression_is_clean(value)
            }
            Statement::Guarded(condition, inner) => {
                self.expression_is_clean(condition) && self.definition_is_clean(inner)
            }
            Statement::Block(inner)
            | Statement::SyncBlock(inner)
            | Statement::Mutex(inner)
            | Statement::Defer(inner)
            | Statement::Foreach { body: inner, .. } => self.definition_is_clean(inner),
            Statement::Match { expr, arms } => {
                self.expression_is_clean(expr)
                    && arms.iter().all(|arm| self.definition_is_clean(&arm.body))
            }
            _ => true,
        }
    }

    fn expression_is_clean(&mut self, expr: &Expr) -> bool {
        // A candidate result is valid only in a position explicitly consumed
        // below. This rejects return/state/list/field/binary positions rather
        // than silently accepting them.
        if matches!(expr, Expr::Call(name, _, _) if name == self.candidate) {
            return false;
        }
        match expr {
            Expr::Call(outer, args, _) => args.iter().enumerate().all(|(index, arg)| {
                if matches!(arg, Expr::Call(name, _, _) if name == self.candidate) {
                    self.saw_call = true;
                    return self.outer_consumes(outer, index);
                }
                self.expression_is_clean(arg)
            }),
            Expr::MethodCall(receiver, _, args, _, _) => {
                self.expression_is_clean(receiver)
                    && args.iter().all(|arg| self.expression_is_clean(arg))
            }
            Expr::BinaryOp(_, left, right) => {
                self.expression_is_clean(left) && self.expression_is_clean(right)
            }
            Expr::Field(inner, _)
            | Expr::Index(inner, _)
            | Expr::Cast(inner, _)
            | Expr::Await(inner) => self.expression_is_clean(inner),
            Expr::StructLiteral { fields, specs, .. } => fields
                .iter()
                .chain(specs.iter())
                .all(|(_, value)| self.expression_is_clean(value)),
            _ => true,
        }
    }

    fn outer_consumes(&mut self, outer: &str, index: usize) -> bool {
        if self.layouts.object_payload_is_inline(outer, self.object) {
            return true;
        }
        let Some(callee) = self.definitions.get(outer) else {
            return false;
        };
        let Some((param_name, param_type)) = callee.parameters.get(index) else {
            return false;
        };
        let Some(object) = param_type.universe_key() else {
            return false;
        };
        parameter_is_non_retaining(&callee.body, param_name, object, self.layouts)
    }
}

fn all_calls_are_consumed(
    items: &[TopLevel],
    candidate: &str,
    object: &str,
    layouts: &EnumLayoutPlanner,
) -> bool {
    let mut definitions = BTreeMap::new();
    for item in items {
        if let TopLevel::Definition(d) = item {
            definitions.insert(d.name.clone(), d);
        }
    }
    let mut proof = CallUseProof {
        candidate,
        object,
        layouts,
        definitions,
        saw_call: false,
    };
    let mut clean = true;
    for item in items {
        if let TopLevel::Definition(d) = item {
            clean &= proof.definition_is_clean(&d.body);
        }
    }
    clean && proof.saw_call
}
fn parameter_is_non_retaining(
    body: &[Statement],
    param_name: &str,
    object: &str,
    layouts: &EnumLayoutPlanner,
) -> bool {
    let mut allowed = 0usize;
    let mut bare = 0usize;
    crate::ast::visit::walk_stmts(body, &mut |expr| match expr {
        Expr::Field(inner, _) => {
            if matches!(inner.as_ref(), Expr::Identifier(name) if name == param_name) {
                allowed += 1;
            }
        }
        Expr::Call(variant, args, _) => {
            if args.len() == 1
                && matches!(args[0], Expr::Identifier(ref name) if name == param_name)
                && layouts.object_payload_is_inline(variant, object)
            {
                allowed += 1;
            }
        }
        Expr::Identifier(name) if name == param_name => bare += 1,
        _ => {}
    });
    bare <= allowed
}


#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        crate::parser::Parser::new(tokens, src)
            .parse_program()
            .expect("parse")
    }

    #[test]
    fn consumed_object_return_uses_caller_destination() {
        let items = parse(
            r#"
obj F { a: Int; b: Int; };
enum R { Ok(F), Err(Int) };
defn make(x: Int) -> F { term F { a: x; b: 0 }; };
defn consume(f: F) -> R { term Ok(f); };
defn caller(x: Int) -> R { term consume(make(x)); };
"#,
        );
        let layouts = EnumLayoutPlanner::new(&items, 2);
        let plan = build_object_representation_plan(&items, &layouts);
        assert!(plan.caller_destination_defns.contains("make"));
    }

    #[test]
    fn unconsumed_object_call_stays_boxed() {
        let items = parse(
            r#"
obj F { a: Int; b: Int; };
enum R { Ok(F), Err(Int) };
defn make(x: Int) -> F { term F { a: x; b: 0 }; };
defn consume(f: F) -> R { term Ok(f); };
defn caller(x: Int) -> R {
    let direct: F = make(x);
    term consume(direct);
};
"#,
        );
        let layouts = EnumLayoutPlanner::new(&items, 2);
        let plan = build_object_representation_plan(&items, &layouts);
        assert!(!plan.caller_destination_defns.contains("make"));
    }

    #[test]
    fn retained_object_return_stays_boxed() {
        let items = parse(
            r#"
obj F { a: Int; b: Int; };
defn make(x: Int) -> F { term F { a: x; b: 0 }; };
defn keep(f: F) -> F { term f; };
defn caller(x: Int) -> F { term keep(make(x)); };
"#,
        );
        let layouts = EnumLayoutPlanner::new(&items, 2);
        let plan = build_object_representation_plan(&items, &layouts);
        assert!(!plan.caller_destination_defns.contains("make"));
    }

    #[test]
    fn non_literal_object_return_stays_boxed() {
        let items = parse(
            r#"
obj F { a: Int; b: Int; };
defn make(f: F) -> F { term f; };
"#,
        );
        let layouts = EnumLayoutPlanner::new(&items, 2);
        let plan = build_object_representation_plan(&items, &layouts);
        assert!(!plan.caller_destination_defns.contains("make"));
    }
}
