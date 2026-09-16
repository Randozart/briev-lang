use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, UnaryOpKind};
use std::collections::{HashMap, HashSet, VecDeque};

/// A variable-level dependency graph built from the program's top-level
/// declarations and transactions. Used by the `trg` reactive dirty-flag
/// system to topologically order variables and assign bit indices.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// Variables in topological order (trg inputs first, then dependents).
    pub topo_order: Vec<String>,
    /// Bit index assigned to each variable (0..63 for u64, extendable).
    pub bit_index: HashMap<String, usize>,
    /// Direct dependencies: variable -> variables it reads.
    pub dependencies: HashMap<String, Vec<String>>,
    /// Reverse map: variable -> variables that read it.
    pub dependents: HashMap<String, Vec<String>>,
    /// Variables declared as `trg` (externally driven inputs).
    pub is_trg: HashSet<String>,
    /// All known variable names from StateDecl and Trigger declarations.
    pub all_vars: HashSet<String>,
}

#[derive(Debug)]
pub struct DependencyError {
    pub cycle: Vec<String>,
}

impl DependencyGraph {
    /// Build the dependency graph from a list of top-level items.
    pub fn build(items: &[TopLevel]) -> Result<Self, DependencyError> {
        let mut graph = DependencyGraph {
            topo_order: Vec::new(),
            bit_index: HashMap::new(),
            dependencies: HashMap::new(),
            dependents: HashMap::new(),
            is_trg: HashSet::new(),
            all_vars: HashSet::new(),
        };

        // Pass 1: collect all variable names
        for item in items {
            match item {
                TopLevel::StateDecl(decl) => {
                    graph.all_vars.insert(decl.name.clone());
                }
                TopLevel::Trigger(trg) => {
                    graph.all_vars.insert(trg.name.clone());
                    graph.is_trg.insert(trg.name.clone());
                }
                TopLevel::TriggerBinding { name, .. } => {
                    graph.all_vars.insert(name.clone());
                    graph.is_trg.insert(name.clone());
                }
                TopLevel::Transaction(txn) => {
                    // Transaction names are callable, not state variables
                    // But local variables inside bodies may reference state vars
                }
                TopLevel::Definition(_defn) => {
                    // Definition params are local, not state vars — skip
                }
                _ => {}
            }
        }

        // Pass 2: extract variable reads from transaction bodies
        for item in items {
            if let TopLevel::Transaction(txn) = item {
                for stmt in &txn.body {
                    graph.collect_from_statement(stmt);
                }
                // Also collect from pre/post conditions
                let pre_ids = collect_expr_identifiers(&txn.contract.pre_condition);
                for id in pre_ids {
                    if graph.all_vars.contains(&id) && !graph.is_trg.contains(&id) {
                        // Tracked as dependency reference
                    }
                }
            }
        }

        // Pass 4: topological sort (Kahn's algorithm)
        let topo = graph.topological_sort()?;
        graph.topo_order = topo;

        // Pass 5: assign bit indices
        for (i, var) in graph.topo_order.iter().enumerate() {
            graph.bit_index.insert(var.clone(), i);
        }

        Ok(graph)
    }

    fn collect_from_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(lhs, expr) => {
                let lhs_name = match lhs {
                    Expr::Identifier(n) => Some(n.clone()),
                    _ => None,
                };
                let rhs_ids: Vec<String> = collect_expr_identifiers(expr)
                    .into_iter()
                    .filter(|r| self.all_vars.contains(r))
                    .collect();
                if let Some(lhs) = lhs_name {
                    if self.all_vars.contains(&lhs) && !rhs_ids.is_empty() {
                        self.dependencies.entry(lhs.clone()).or_default().extend(rhs_ids.clone());
                        for dep in &rhs_ids {
                            self.dependents.entry(dep.clone()).or_default().push(lhs.clone());
                        }
                    }
                }
            }
            Statement::Let { name, expr, .. } => {
                if let Some(e) = expr {
                    let rhs_ids: Vec<String> = collect_expr_identifiers(e)
                        .into_iter()
                        .filter(|r| self.all_vars.contains(r))
                        .collect();
                    if self.all_vars.contains(name) && !rhs_ids.is_empty() {
                        self.dependencies.entry(name.clone()).or_default().extend(rhs_ids.clone());
                        for dep in &rhs_ids {
                            self.dependents.entry(dep.clone()).or_default().push(name.clone());
                        }
                    }
                }
            }
            Statement::Guarded(_, statements) => {
                for s in statements {
                    self.collect_from_statement(s);
                }
            }
            Statement::Expression(expr) => {
                // Expression statements may reference state vars (e.g., FFI calls)
                let ids: Vec<String> = collect_expr_identifiers(expr)
                    .into_iter()
                    .filter(|r| self.all_vars.contains(r))
                    .collect();
                // These are reads, tracked passively
                let _ = ids;
            }
            _ => {}
        }
    }

    fn topological_sort(&self) -> Result<Vec<String>, DependencyError> {
        // In-degree count per variable
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for var in &self.all_vars {
            in_degree.entry(var).or_insert(0);
        }
        for (_, deps) in &self.dependencies {
            for dep in deps {
                *in_degree.entry(dep).or_insert(0) += 0; // ensure entry exists
            }
        }
        // Actually, properly compute in_degree: for each var, count deps from others
        let mut in_degree2: HashMap<&str, usize> = HashMap::new();
        for var in &self.all_vars {
            in_degree2.entry(var.as_str()).or_insert(0);
        }
        for (var, deps) in &self.dependencies {
            for _dep in deps {
                // var depends on dep — dep has an outgoing edge to var
                // var's in-degree = number of vars it depends on
                *in_degree2.entry(var.as_str()).or_insert(0) += 1;
            }
        }

        // Kahn's algorithm: start with vars that have in-degree 0 (trg inputs, independent vars)
        let mut queue: VecDeque<&str> = VecDeque::new();
        for var in &self.all_vars {
            if *in_degree2.get(var.as_str()).unwrap_or(&0) == 0 {
                queue.push_back(var.as_str());
            }
        }

        let mut sorted: Vec<String> = Vec::new();
        let mut in_deg = in_degree2.clone();

        while let Some(var) = queue.pop_front() {
            sorted.push(var.to_string());

            // Decrease in-degree for all dependents
            if let Some(deps) = self.dependents.get(var) {
                for dep in deps {
                    if let Some(deg) = in_deg.get_mut(dep.as_str()) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep.as_str());
                        }
                    }
                }
            }
        }

        if sorted.len() != self.all_vars.len() {
            // Find the cycle members
            let unsorted: Vec<String> = self
                .all_vars
                .difference(&sorted.iter().cloned().collect())
                .cloned()
                .collect();
            return Err(DependencyError { cycle: unsorted });
        }

        Ok(sorted)
    }
}

/// Collect all identifier references from an expression (recursive walk).
pub fn collect_expr_identifiers(expr: &Expr) -> Vec<String> {
    let mut ids = Vec::new();
    collect_expr_ids_inner(expr, &mut ids);
    ids
}

fn collect_expr_ids_inner(expr: &Expr, ids: &mut Vec<String>) {
    match expr {
        Expr::Identifier(n) => {
            ids.push(n.clone());
        }
        Expr::BinaryOp(_, a, b) => {
            collect_expr_ids_inner(a, ids);
            collect_expr_ids_inner(b, ids);
        }
        Expr::UnaryOp(_, a) | Expr::Cast(a, _) | Expr::IsType(a, _) => {
            collect_expr_ids_inner(a, ids);
        }
        Expr::Call(_, args, _) | Expr::Spawn { args, .. } => {
            for arg in args {
                collect_expr_ids_inner(arg, ids);
            }
        }
        Expr::List(items) | Expr::Tuple(items) => {
            for item in items {
                collect_expr_ids_inner(item, ids);
            }
        }
        Expr::Index(list, index) => {
            collect_expr_ids_inner(list, ids);
            collect_expr_ids_inner(index, ids);
        }
        Expr::Field(obj, _) => {
            collect_expr_ids_inner(obj, ids);
        }
        Expr::Match(value, arms) => {
            collect_expr_ids_inner(value, ids);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_ids_inner(guard, ids);
                }
                collect_expr_ids_inner(&arm.body, ids);
            }
        }
        Expr::Block(stmts) => {
            for stmt in stmts {
                statement_ids(stmt, ids);
            }
        }
        Expr::If(cond, then, else_) => {
            collect_expr_ids_inner(cond, ids);
            collect_expr_ids_inner(then, ids);
            if let Some(else_) = else_ {
                collect_expr_ids_inner(else_, ids);
            }
        }
        Expr::Lambda(_, body) => {
            collect_expr_ids_inner(body, ids);
        }
        Expr::Within(inner, _) => {
            collect_expr_ids_inner(inner, ids);
        }
        // Literal leaves — no identifiers
        Expr::Decimal(_) | Expr::TaggedLiteral(_, _) | Expr::Char(_) | Expr::Float(_) | Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) | Expr::Bool(_) | Expr::BeginProgram => {}
        Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::Consume(inner) | Expr::Await(inner) => {
            collect_expr_ids_inner(inner, ids);
        }
        Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
            collect_expr_ids_inner(recv, ids);
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            collect_expr_ids_inner(recv, ids);
            for a in args { collect_expr_ids_inner(a, ids); }
        }
        Expr::FormattingAnnotation(_) | Expr::DerivationBlock(_) | Expr::StructLiteral { .. } => {}
        Expr::PluginIntercept { args, .. } => {
            for a in args { collect_expr_ids_inner(a, ids); }
        }
        Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
        Expr::Slice { array, start, end, stride } => {
            collect_expr_ids_inner(array, ids);
            if let Some(e) = start.as_deref() { collect_expr_ids_inner(e, ids); }
            if let Some(e) = end.as_deref() { collect_expr_ids_inner(e, ids); }
            if let Some(e) = stride.as_deref() { collect_expr_ids_inner(e, ids); }
        }
        Expr::Range { start, end, inclusive: _ } => {
            collect_expr_ids_inner(start, ids);
            collect_expr_ids_inner(end, ids);
        }
        Expr::Named { inner, .. } => {
            collect_expr_ids_inner(inner, ids);
        }
        Expr::UnitLiteral { .. } => {}
        Expr::Capture { expr, .. } => {
            collect_expr_ids_inner(expr, ids);
        }

    }
}

fn statement_ids(stmt: &Statement, ids: &mut Vec<String>) {
    match stmt {
        Statement::Assign(lhs, expr) => {
            // 2026-07-09: Handle AddrOf LHS for pointer writes.
            if let Some(n) = lhs.as_var_name() {
                ids.push(n.to_string());
            }
            collect_expr_ids_inner(expr, ids);
        }
        Statement::Let { name, expr, .. } => {
            ids.push(name.clone());
            if let Some(e) = expr {
                collect_expr_ids_inner(e, ids);
            }
        }
        Statement::Guarded(_, statements) => {
            for s in statements {
                statement_ids(s, ids);
            }
        }
        Statement::Expression(expr) => {
            collect_expr_ids_inner(expr, ids);
        }
        Statement::TrgBinding { name, .. } => {
            ids.push(name.clone());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn make_state_decl(name: &str, ty: Type) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: name.to_string(),
            ty,
            span: None,
        })
    }

    fn make_trigger(name: &str, ty: Type) -> TopLevel {
        TopLevel::Trigger(Trigger {
            name: name.to_string(),
            instance: Expr::Identifier("__io".to_string()),
            span: None,
        })
    }

    // 2026-07-14: Helper to create a transaction whose body records
    // a state variable dependency: lhs depends on rhs_names.
    fn make_txn_assign(lhs: &str, rhs: &[&str]) -> TopLevel {
        let rhs_expr = if rhs.len() == 1 {
            Expr::Identifier(rhs[0].to_string())
        } else {
            let binary = rhs.iter().map(|n| Expr::Identifier(n.to_string())).collect::<Vec<_>>();
            Expr::BinaryOp(BinaryOpKind::Add, Box::new(binary[0].clone()), Box::new(binary[1].clone()))
        };
        TopLevel::Transaction(Transaction {
            name: format!("txn_{}", lhs),
            is_reactive: false,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![Statement::Assign(Expr::Identifier(lhs.to_string()), rhs_expr)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    #[test]
    fn test_empty_program() {
        let graph = DependencyGraph::build(&[]).unwrap();
        assert!(graph.topo_order.is_empty());
        assert!(graph.all_vars.is_empty());
    }

    #[test]
    fn test_trg_variable_only() {
        let graph = DependencyGraph::build(&[
            make_trigger("sensor", Type::int()),
        ]).unwrap();
        assert_eq!(graph.topo_order.len(), 1);
        assert!(graph.is_trg.contains("sensor"));
        assert_eq!(graph.bit_index["sensor"], 0);
    }

    #[test]
    fn test_simple_dependency() {
        let graph = DependencyGraph::build(&[
            make_trigger("sensor", Type::int()),
            make_state_decl("derived", Type::int()),
            make_txn_assign("derived", &["sensor"]),
        ]).unwrap();
        assert_eq!(graph.topo_order.len(), 2);
        let sensor_pos = graph.topo_order.iter().position(|v| v == "sensor").unwrap();
        let derived_pos = graph.topo_order.iter().position(|v| v == "derived").unwrap();
        assert!(sensor_pos < derived_pos, "trg must come before dependent");
        assert_eq!(graph.bit_index["sensor"], 0);
        assert_eq!(graph.bit_index["derived"], 1);
    }

    #[test]
    fn test_chain_dependency() {
        let graph = DependencyGraph::build(&[
            make_trigger("a", Type::int()),
            make_state_decl("b", Type::int()),
            make_state_decl("c", Type::int()),
            make_txn_assign("b", &["a"]),
            make_txn_assign("c", &["b"]),
        ]).unwrap();
        let pos_a = graph.topo_order.iter().position(|v| v == "a").unwrap();
        let pos_b = graph.topo_order.iter().position(|v| v == "b").unwrap();
        let pos_c = graph.topo_order.iter().position(|v| v == "c").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c, "topological order broken");
    }

    #[test]
    fn test_cycle_detection() {
        let result = DependencyGraph::build(&[
            make_state_decl("a", Type::int()),
            make_state_decl("b", Type::int()),
            make_txn_assign("a", &["b"]),
            make_txn_assign("b", &["a"]),
        ]);
        assert!(result.is_err(), "cycle should be detected");
        if let Err(e) = result {
            assert!(!e.cycle.is_empty(), "cycle should have members");
        }
    }

    #[test]
    fn test_multiple_trgs() {
        let graph = DependencyGraph::build(&[
            make_trigger("a", Type::int()),
            make_trigger("b", Type::int()),
            make_state_decl("sum", Type::int()),
            make_txn_assign("sum", &["a", "b"]),
        ]).unwrap();
        assert_eq!(graph.topo_order.len(), 3);
        assert!(graph.bit_index["a"] < graph.bit_index["sum"]);
        assert!(graph.bit_index["b"] < graph.bit_index["sum"]);
    }

    #[test]
    fn test_independent_vars() {
        let graph = DependencyGraph::build(&[
            make_trigger("x", Type::int()),
            make_state_decl("y", Type::int()),
        ]).unwrap();
        assert_eq!(graph.topo_order.len(), 2);
    }
}
