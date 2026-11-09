// ── Phase F — MCMC Stochastic Superoptimizer ──────────────────────────
// 2026-07-28: Phase F.0 — MCMC configuration, state, sampler loop.
// Flat code: each function max 2 levels of nesting.

use crate::ast::{DerivationExample, Expr, PropertyValue};
use crate::interpreter::Atom;
use crate::derive::engine::SynthesizedProgram;
use crate::derive::SynthesizeError;
use std::collections::HashMap;

// ── F.0 — LCG Random Number Generator ───────────────────────────────
// 2026-07-28: Phase F — Use LCG instead of `rand` crate to avoid adding
// a dependency to the compiler. Deterministic by design — no seed_from_u64.

/// 2026-07-28: Phase F — Simple LCG (Linear Congruential Generator) for
/// MCMC random number generation. Avoids `rand` crate dependency.
/// Parameters from Numerical Recipes (Park & Miller).
pub struct LcgRng {
    state: u64,
}

impl LcgRng {
    pub fn new(seed: u64) -> Self {
        LcgRng { state: if seed == 0 { 1 } else { seed } }
    }

    /// Generate f64 in [0, 1).
    pub fn gen_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let upper = (self.state >> 11) as f64;
        upper / 9007199254740992.0
    }

    /// Generate bool with given probability of true.
    pub fn gen_bool(&mut self, probability: f64) -> bool {
        self.gen_f64() < probability
    }

    /// Generate a random index in [lo, hi).
    pub fn gen_range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo { return lo; }
        lo + (self.gen_f64() * (hi - lo) as f64) as usize
    }
}

// ── F.0 — Configuration ──────────────────────────────────────────────

/// 2026-07-28: Phase F.0 — MCMC superoptimizer configuration.
#[derive(Debug, Clone)]
pub struct McmcConfig {
    pub initial_temperature: f64,
    pub cooling_rate: f64,
    pub max_iterations: usize,
    pub convergence_window: usize,
    pub mutation_weights: MutationWeights,
    pub equivalence: EquivalenceMode,
    pub correctness_weight: f64,
    pub performance_weight: f64,
    pub seed: Option<u64>,
    // ── Phase G.4: Config driven by attr metadata ────────────────
    pub tolerance: Option<f64>,
    pub allowed_mutations: Vec<String>,
    pub cost_fn: CostFn,
}

/// Mutation probability weights.
#[derive(Debug, Clone)]
pub struct MutationWeights {
    pub replace_subtree: f64,
    pub change_operator: f64,
    pub swap_commutative: f64,
    pub fold_constant: f64,
    pub insert_identity: f64,
    pub delete_dead_code: f64,
    pub distribute: f64,
    pub vector_fuse: f64,
}

impl Default for MutationWeights {
    fn default() -> Self {
        MutationWeights {
            replace_subtree: 0.25,
            change_operator: 0.20,
            swap_commutative: 0.10,
            fold_constant: 0.15,
            insert_identity: 0.10,
            delete_dead_code: 0.10,
            distribute: 0.05,
            vector_fuse: 0.05,
        }
    }
}

impl MutationWeights {
    /// 2026-07-28: Phase G.4 — Only linear arithmetic mutations.
    pub fn linear_only() -> Self {
        MutationWeights {
            replace_subtree: 0.0,
            change_operator: 0.40,
            swap_commutative: 0.20,
            fold_constant: 0.40,
            insert_identity: 0.0,
            delete_dead_code: 0.0,
            distribute: 0.0,
            vector_fuse: 0.0,
        }
    }

    /// 2026-07-28: Phase G.4 — Only bitwise logic mutations.
    pub fn bitwise_only() -> Self {
        MutationWeights {
            replace_subtree: 0.30,
            change_operator: 0.40,
            swap_commutative: 0.10,
            fold_constant: 0.20,
            insert_identity: 0.0,
            delete_dead_code: 0.0,
            distribute: 0.0,
            vector_fuse: 0.0,
        }
    }
}

/// How to verify equivalence after mutation.
#[derive(Debug, Clone)]
pub enum EquivalenceMode {
    ExamplesOnly,
    Z3Proof { z3_path: String },
    Hybrid { z3_path: String },
}

/// 2026-07-28: Phase G.4 — Which cost function to use for MCMC search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CostFn {
    Latency,
    Throughput,
    Size,
}

/// State of the MCMC sampler.
#[derive(Debug, Clone)]
pub struct McmcState {
    pub current: SynthesizedProgram,
    pub best: SynthesizedProgram,
    pub current_cost: f64,
    pub best_cost: f64,
    pub temperature: f64,
    pub iteration: usize,
    pub iterations_without_improvement: usize,
    pub rng_seed: u64,
}

impl Default for McmcConfig {
    fn default() -> Self {
        McmcConfig {
            initial_temperature: 1.0,
            cooling_rate: 0.999,
            max_iterations: 100_000,
            convergence_window: 1000,
            mutation_weights: MutationWeights::default(),
            equivalence: EquivalenceMode::Hybrid { z3_path: "z3".to_string() },
            correctness_weight: 1000.0,
            performance_weight: 1.0,
            seed: None,
            tolerance: None,
            allowed_mutations: Vec::new(),
            cost_fn: CostFn::Latency,
        }
    }
}

// ── F.3 — Cost Function ──────────────────────────────────────────────

/// Estimate performance cost by counting operations.
pub fn performance_cost(program: &SynthesizedProgram) -> u64 {
    program.body.iter().map(|e| count_expr_ops(e)).sum()
}

fn count_expr_ops(expr: &Expr) -> u64 {
    match expr {
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Identifier(_) => 0,
        Expr::UnaryOp(_, inner) => 1 + count_expr_ops(inner),
        Expr::BinaryOp(_, lhs, rhs) => 1 + count_expr_ops(lhs) + count_expr_ops(rhs),
        Expr::If(cond, then_, else_) => {
            1 + count_expr_ops(cond) + count_expr_ops(then_) + else_.as_ref().map(|e| count_expr_ops(e)).unwrap_or(0)
        }
        _ => 1,
    }
}

// ── F.3 — MCMC Sampler ──────────────────────────────────────────────

/// Run the MCMC superoptimizer.
pub fn optimize(
    initial: SynthesizedProgram,
    examples: &[DerivationExample],
    config: &McmcConfig,
) -> Result<SynthesizedProgram, SynthesizeError> {
    let mut rng = LcgRng::new(config.seed.unwrap_or(42));
    let initial_cost = cost_function(&initial, examples);

    let mut state = McmcState {
        current: initial.clone(),
        best: initial.clone(),
        current_cost: initial_cost,
        best_cost: initial_cost,
        temperature: config.initial_temperature,
        iteration: 0,
        iterations_without_improvement: 0,
        rng_seed: config.seed.unwrap_or(42),
    };

    // Phase 3a: Correctness search — if initial is incorrect
    if state.current_cost >= config.correctness_weight {
        state = correctness_search(state, examples, config, &mut rng)?;
    }

    // Phase 3b: Performance optimization — if initial IS correct
    if state.current_cost < config.correctness_weight {
        state = performance_optimization(state, examples, config, &mut rng)?;
    }

    Ok(state.best)
}

/// Phase 3a: Correctness search — random mutations until program satisfies all examples.
fn correctness_search(
    mut state: McmcState,
    examples: &[DerivationExample],
    config: &McmcConfig,
    rng: &mut LcgRng,
) -> Result<McmcState, SynthesizeError> {
    for i in 0..config.max_iterations {
        state.iteration = i;

        let proposal_body: Vec<Expr> = state.current.body.iter()
            .map(|e| super::mutate::apply_random_mutation(e, &config.mutation_weights, rng, 3))
            .collect();
        let proposed_prog = SynthesizedProgram { body: proposal_body, ..state.current.clone() };

        let proposal_cost = cost_function(&proposed_prog, examples);
        let delta = proposal_cost - state.current_cost;

        if delta < 0.0 || rng.gen_f64() < (-delta / state.temperature.max(1e-10)).exp() {
            state.current = proposed_prog;
            state.current_cost = proposal_cost;
            state.iterations_without_improvement = 0;

            if proposal_cost < state.best_cost {
                state.best = state.current.clone();
                state.best_cost = proposal_cost;
            }
        } else {
            state.iterations_without_improvement += 1;
        }
        state.temperature *= config.cooling_rate;

        if state.iterations_without_improvement >= config.convergence_window {
            break;
        }
        if state.current_cost < config.correctness_weight {
            break;
        }
    }
    Ok(state)
}

/// Phase 3b: Performance optimization — strict improvement on a correct program.
fn performance_optimization(
    mut state: McmcState,
    examples: &[DerivationExample],
    config: &McmcConfig,
    rng: &mut LcgRng,
) -> Result<McmcState, SynthesizeError> {
    state.temperature = 0.01;

    for i in 0..config.max_iterations {
        state.iteration = i;

        let proposal_body: Vec<Expr> = state.current.body.iter()
            .map(|e| super::mutate::apply_random_mutation(e, &config.mutation_weights, rng, 3))
            .collect();
        let proposed_prog = SynthesizedProgram { body: proposal_body, ..state.current.clone() };

        let is_equivalent = super::equivalence::check_equivalence(
            &proposed_prog, &state.current, examples, &config.equivalence,
        );

        if is_equivalent {
            let proposal_perf = performance_cost(&proposed_prog);
            let current_perf = performance_cost(&state.current);

            if proposal_perf < current_perf {
                state.current = proposed_prog;
                state.current_cost = proposal_perf as f64;
                state.iterations_without_improvement = 0;

                if state.current_cost < state.best_cost {
                    state.best = state.current.clone();
                    state.best_cost = state.current_cost;
                }
            } else {
                state.iterations_without_improvement += 1;
            }
        } else {
            state.iterations_without_improvement += 1;
        }

        if state.iterations_without_improvement >= config.convergence_window {
            break;
        }
    }
    Ok(state)
}

/// Combined cost function: correctness violations + performance.
fn cost_function(
    program: &SynthesizedProgram,
    examples: &[DerivationExample],
) -> f64 {
    let mut violations = 0u64;
    for ex in examples {
        let mut ctx = crate::derive::engine::SynthesisEvalContext::new();
        for (i, input_expr) in ex.inputs.iter().enumerate() {
            let val = expr_to_value(input_expr);
            ctx.bind(&format!("x{}", i), val);
        }
        let result = program.body.first()
            .and_then(|e| crate::derive::engine::evaluate_synthesized(e, &mut ctx).ok());
        let expected = crate::derive::engine::evaluate_synthesized(&ex.output, &mut crate::derive::engine::SynthesisEvalContext::new()).ok();
        match (result, expected) {
            (Some(r), Some(e)) => {
                if !crate::interpreter::values_within_tolerance(&r, &e, ex.tolerance.unwrap_or(0.0)) {
                    violations += 1;
                }
            }
            _ => violations += 1,
        }
    }
    violations as f64 * 1000.0 + performance_cost(program) as f64
}

fn expr_to_value(expr: &Expr) -> crate::interpreter::Value {
    match expr {
        Expr::Decimal(n) => crate::interpreter::Value::Atom(Atom::Int(*n)),
        Expr::Float(f) => crate::interpreter::Value::Atom(Atom::Float(*f)),
        Expr::Bool(b) => crate::interpreter::Value::Bits(vec![if *b { 1 } else { 0 }]),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => match expr_to_value(inner) {
            crate::interpreter::Value::Atom(Atom::Int(n)) => crate::interpreter::Value::Atom(Atom::Int(-n)),
            crate::interpreter::Value::Atom(Atom::Float(f)) => crate::interpreter::Value::Atom(Atom::Float(-f)),
            _ => crate::interpreter::Value::Atom(Atom::Int(0)),
        },
        _ => crate::interpreter::Value::Atom(Atom::Int(0)),
    }
}

/// 2026-07-28: Phase G.4 — Build an McmcConfig from attr metadata.
/// Reads search_space, cost_model, tolerance, and allowed_mutations keys.
pub fn mcmc_config_from_metadata(metadata: &HashMap<String, PropertyValue>) -> McmcConfig {
    let mut config = McmcConfig::default();

    if let Some(PropertyValue::String(space)) = metadata.get("search_space") {
        match space.as_str() {
            "linear" => config.mutation_weights = MutationWeights::linear_only(),
            "bitwise" => config.mutation_weights = MutationWeights::bitwise_only(),
            _ => {}
        }
    }

    if let Some(PropertyValue::String(model)) = metadata.get("cost_model") {
        match model.as_str() {
            "latency" => config.cost_fn = CostFn::Latency,
            "throughput" => config.cost_fn = CostFn::Throughput,
            "size" => config.cost_fn = CostFn::Size,
            _ => {}
        }
    }

    if let Some(PropertyValue::Float(tol)) = metadata.get("tolerance") {
        config.tolerance = Some(*tol);
    }

    if let Some(PropertyValue::List(mutations)) = metadata.get("allowed_mutations") {
        config.allowed_mutations = mutations.iter()
            .filter_map(|v| match v {
                PropertyValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOpKind;

    #[test]
    fn test_lcg_rng_f64() {
        let mut rng = LcgRng::new(42);
        let val = rng.gen_f64();
        assert!(val >= 0.0 && val < 1.0);
    }

    #[test]
    fn test_lcg_rng_deterministic() {
        let mut a = LcgRng::new(42);
        let mut b = LcgRng::new(42);
        for _ in 0..100 {
            assert_eq!(a.gen_f64(), b.gen_f64());
        }
    }

    #[test]
    fn test_lcg_rng_range() {
        let mut rng = LcgRng::new(42);
        for _ in 0..100 {
            let val = rng.gen_range(3, 7);
            assert!(val >= 3 && val < 7);
        }
    }

    #[test]
    fn test_mcmc_config_default() {
        let cfg = McmcConfig::default();
        assert!((cfg.initial_temperature - 1.0).abs() < 1e-10);
        assert_eq!(cfg.max_iterations, 100_000);
        assert_eq!(cfg.convergence_window, 1000);
    }

    #[test]
    fn test_mutation_weights_sum_to_one() {
        let w = MutationWeights::default();
        let sum = w.replace_subtree + w.change_operator + w.swap_commutative
            + w.fold_constant + w.insert_identity + w.delete_dead_code
            + w.distribute + w.vector_fuse;
        assert!((sum - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_performance_cost_constant() {
        let prog = SynthesizedProgram { body: vec![Expr::Decimal(42)], cost: 0, depth: 0, helpers: vec![] };
        assert_eq!(performance_cost(&prog), 0);
    }

    #[test]
    fn test_performance_cost_binary_op() {
        let prog = SynthesizedProgram {
            body: vec![Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(1)),
            )],
            cost: 0, depth: 0,
            helpers: vec![],
        };
        assert_eq!(performance_cost(&prog), 1);
    }

    // ── Phase G.4 Tests ─────────────────────────────────────────

    #[test]
    fn test_mut_weight_linear_only() {
        let w = MutationWeights::linear_only();
        assert_eq!(w.replace_subtree, 0.0);
        assert!((w.change_operator + w.swap_commutative + w.fold_constant - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_mut_weight_bitwise_only() {
        let w = MutationWeights::bitwise_only();
        assert_eq!(w.insert_identity, 0.0);
        assert_eq!(w.delete_dead_code, 0.0);
    }

    #[test]
    fn test_mcmc_config_from_metadata_search_space_linear() {
        let mut meta = HashMap::new();
        meta.insert("search_space".into(), PropertyValue::String("linear".into()));
        let cfg = mcmc_config_from_metadata(&meta);
        assert_eq!(cfg.mutation_weights.replace_subtree, 0.0);
    }

    #[test]
    fn test_mcmc_config_from_metadata_cost_model() {
        let mut meta = HashMap::new();
        meta.insert("cost_model".into(), PropertyValue::String("throughput".into()));
        let cfg = mcmc_config_from_metadata(&meta);
        assert_eq!(cfg.cost_fn, CostFn::Throughput);
    }

    #[test]
    fn test_mcmc_config_from_metadata_tolerance() {
        let mut meta = HashMap::new();
        meta.insert("tolerance".into(), PropertyValue::Float(0.001));
        let cfg = mcmc_config_from_metadata(&meta);
        assert!((cfg.tolerance.unwrap() - 0.001).abs() < 1e-10);
    }

    #[test]
    fn test_mcmc_config_from_metadata_allowed_mutations() {
        let mut meta = HashMap::new();
        meta.insert("allowed_mutations".into(), PropertyValue::List(vec![
            PropertyValue::String("replace_subtree".into()),
            PropertyValue::String("fold_constant".into()),
        ]));
        let cfg = mcmc_config_from_metadata(&meta);
        assert_eq!(cfg.allowed_mutations.len(), 2);
        assert!(cfg.allowed_mutations.contains(&"replace_subtree".into()));
    }

    #[test]
    fn test_mcmc_config_from_metadata_empty() {
        let meta: HashMap<String, PropertyValue> = HashMap::new();
        let cfg = mcmc_config_from_metadata(&meta);
        assert!(cfg.tolerance.is_none());
        assert!(cfg.allowed_mutations.is_empty());
        assert_eq!(cfg.cost_fn, CostFn::Latency);
    }
}
