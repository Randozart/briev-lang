// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Lower modular call facts at their evaluation point, never at function entry.
use super::{CallContracts, post_facts_for_atom};
use crate::ast::{BinaryOpKind as Op, Contract, Expr, Statement, visit};
use std::collections::HashSet;

/// Call contracts remain modular assumptions; this phase preserves when and
/// where their facts become available. Generated names cannot alias user state.
pub(super) fn lower<'a>(
    body: &[Statement],
    contracts: &CallContracts,
    self_name: &str,
    contract: &Contract,
    bindings: impl Iterator<Item = &'a String>,
) -> Vec<Statement> {
    let mut used: HashSet<String> = bindings.cloned().collect();
    let mut collect = |expr: &Expr| {
        if let Expr::Identifier(name) = expr {
            used.insert(name.clone());
        }
    };
    visit::walk_contract(contract, &mut collect);
    visit::walk_stmts(body, &mut collect);
    collect_let_names(body, &mut used);
    Lowering {
        contracts,
        self_name,
        used,
        counter: 0,
    }
    .statements(body)
}

/// The shared expression visitor does not visit declaration names. Only lets
/// and guards can introduce nested bindings in the admitted straight-line body.
fn collect_let_names(body: &[Statement], used: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Statement::Let { name, names, .. } => {
                used.insert(name.clone());
                used.extend(names.iter().cloned());
            }
            Statement::Guarded(_, inner) => collect_let_names(inner, used),
            _ => {}
        }
    }
}

struct Lowering<'a> {
    contracts: &'a CallContracts,
    self_name: &'a str,
    used: HashSet<String>,
    counter: usize,
}

impl Lowering<'_> {
    fn statements(&mut self, body: &[Statement]) -> Vec<Statement> {
        body.iter().flat_map(|stmt| self.statement(stmt)).collect()
    }

    /// Prefix facts are executed before this statement can overwrite an argument.
    fn statement(&mut self, stmt: &Statement) -> Vec<Statement> {
        let mut prefix = Vec::new();
        let rewritten = self.statement_value(stmt, &mut prefix);
        prefix.push(rewritten);
        prefix
    }

    fn statement_value(&mut self, stmt: &Statement, prefix: &mut Vec<Statement>) -> Statement {
        if let Statement::Guarded(condition, body) = stmt {
            let condition = self.expression(condition, prefix);
            return Statement::Guarded(condition, self.statements(body));
        }
        let mut e = |expr: &Expr| self.expression(expr, prefix);
        match stmt {
            Statement::Assign(lhs, rhs) => Statement::Assign(lhs.clone(), e(rhs)),
            Statement::Let {
                name,
                names,
                ty,
                expr,
                modifiers,
            } => Statement::Let {
                name: name.clone(),
                names: names.clone(),
                ty: ty.clone(),
                expr: expr.as_ref().map(&mut e),
                modifiers: modifiers.clone(),
            },
            Statement::Term(Some(x)) => Statement::Term(Some(e(x))),
            Statement::EndProgram(Some(x)) => Statement::EndProgram(Some(e(x))),
            Statement::Tail(x) => Statement::Tail(e(x)),
            Statement::Gate(x) => Statement::Gate(e(x)),
            Statement::Check(x) => Statement::Check(e(x)),
            Statement::Expression(x) => Statement::Expression(e(x)),
            _ => stmt.clone(),
        }
    }

    fn expression(&mut self, expr: &Expr, prefix: &mut Vec<Statement>) -> Expr {
        match expr {
            Expr::Call(name, args, id) => self.call(name, args, *id, prefix),
            Expr::BinaryOp(op, left, right) => self.binary(*op, left, right, prefix),
            Expr::UnaryOp(op, inner) => {
                Expr::UnaryOp(*op, Box::new(self.expression(inner, prefix)))
            }
            Expr::Cast(inner, ty) => {
                Expr::Cast(Box::new(self.expression(inner, prefix)), ty.clone())
            }
            Expr::Field(base, field) => {
                Expr::Field(Box::new(self.expression(base, prefix)), field.clone())
            }
            Expr::Index(base, index) => Expr::Index(
                Box::new(self.expression(base, prefix)),
                Box::new(self.expression(index, prefix)),
            ),
            Expr::Consume(inner) => Expr::Consume(Box::new(self.expression(inner, prefix))),
            Expr::MethodCall(recv, name, args, id) => Expr::MethodCall(
                Box::new(self.expression(recv, prefix)),
                name.clone(),
                args.iter()
                    .map(|arg| self.expression(arg, prefix))
                    .collect(),
                *id,
            ),
            Expr::Tuple(items) => Expr::Tuple(
                items
                    .iter()
                    .map(|item| self.expression(item, prefix))
                    .collect(),
            ),
            _ => expr.clone(),
        }
    }

    /// A lazy RHS may supply facts only on the path that evaluates it. Forking
    /// the symbolic path adds no source effects and keeps the skipped path free
    /// of assumptions about an unevaluated call.
    fn binary(&mut self, op: Op, left: &Expr, right: &Expr, prefix: &mut Vec<Statement>) -> Expr {
        let left = self.expression(left, prefix);
        let mut right_prefix = Vec::new();
        let right = self.expression(right, &mut right_prefix);
        if !right_prefix.is_empty() {
            match rhs_guard(op, &left) {
                Some(guard) => prefix.push(Statement::Guarded(guard, right_prefix)),
                None => prefix.extend(right_prefix),
            }
        }
        Expr::BinaryOp(op, Box::new(left), Box::new(right))
    }

    fn call(
        &mut self,
        name: &str,
        args: &[Expr],
        id: Option<usize>,
        prefix: &mut Vec<Statement>,
    ) -> Expr {
        let args: Vec<Expr> = args
            .iter()
            .map(|arg| self.expression(arg, prefix))
            .collect();
        let Some((params, post, arity)) = self
            .contracts
            .get(name)
            .filter(|(params, _, _)| name != self.self_name && params.len() == args.len())
        else {
            return Expr::Call(name.into(), args, id);
        };
        let atoms: Vec<Expr> = (0..(*arity).max(1))
            .map(|field| self.fresh(name, field))
            .collect();
        prefix.extend(
            post_facts_for_atom(params, post, &args, &atoms)
                .into_iter()
                .map(Statement::Gate),
        );
        if atoms.len() == 1 {
            return atoms.into_iter().next().expect("one call result");
        }
        Expr::Tuple(atoms)
    }

    fn fresh(&mut self, callee: &str, field: usize) -> Expr {
        loop {
            self.counter += 1;
            let name = format!("call#{}#{callee}#{field}", self.counter);
            if self.used.insert(name.clone()) {
                return Expr::Identifier(name);
            }
        }
    }
}

/// The RHS execution predicate for the two short-circuit operators; eager
/// operators have no additional guard. This constructor is used by lowering.
fn rhs_guard(op: Op, left: &Expr) -> Option<Expr> {
    match op {
        Op::And => Some(left.clone()),
        Op::Or => Some(Expr::UnaryOp(
            crate::ast::UnaryOpKind::Not,
            Box::new(left.clone()),
        )),
        _ => None,
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;
    /// Check the production guard constructor, not a separate scheduling model.
    /// Scope: Boolean leaves and And/Or/eager-add; recursive ASTs/state are unproved.
    #[kani::proof]
    #[kani::unwind(3)]
    fn contracted_rhs_guard_matches_short_circuit_execution() {
        let left: bool = kani::any();
        let kind: u8 = kani::any();
        kani::assume(kind < 3);
        let op = match kind {
            0 => Op::And,
            1 => Op::Or,
            _ => Op::Add,
        };
        let guard = std::mem::ManuallyDrop::new(rhs_guard(op, &Expr::Bool(left)));
        let evaluated = match guard.as_ref() {
            None => true,
            Some(Expr::Bool(value)) => *value,
            Some(Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner)) => {
                let Expr::Bool(value) = inner.as_ref() else {
                    panic!("unexpected guard shape")
                };
                !*value
            }
            _ => panic!("unexpected guard shape"),
        };
        assert_eq!(guard.is_some(), kind < 2);
        assert_eq!(
            evaluated,
            match kind {
                0 => left,
                1 => !left,
                _ => true,
            }
        );
        kani::cover!(kind == 0 && !evaluated);
        kani::cover!(kind == 1 && !evaluated);
        kani::cover!(evaluated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_callee_names_cannot_impersonate_generated_atoms() {
        let atom = Expr::Identifier("call#actual".into());
        let post = Expr::BinaryOp(
            Op::Eq,
            Box::new(Expr::Identifier("term".into())),
            Box::new(Expr::Identifier("call#foreign".into())),
        );
        assert!(post_facts_for_atom(&[], &post, &[], &[atom]).is_empty());
    }
}
