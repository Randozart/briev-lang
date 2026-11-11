//! Arithmetic intensity cost model for GPU offloading decisions.
//!
//! Determines whether offloading a loop body to GPU is worth the PCIe
//! transfer overhead. Used by `#?gpu` to decide between GPU dispatch
//! and CPU fallback, and to emit optimization remarks explaining why.

use crate::ast::*;

/// Thresholds for GPU profitability.
const MIN_ARITHMETIC_INTENSITY: f64 = 0.5; // ops per byte — lower threshold
const MIN_ITERATIONS: u64 = 10_000; // minimum N to justify PCIe
const PCIE_LATENCY_NS: f64 = 100_000.0; // ~100µs host→device round trip
const CPU_CYCLE_NS: f64 = 0.25; // ~4 GHz = 0.25 ns/cycle
const GPU_CYCLE_NS: f64 = 1.0; // ~1 GHz GPU core clock

/// Result of the GPU cost-benefit analysis.
#[derive(Debug, Clone)]
pub struct GpuCostEstimate {
    /// Total arithmetic operations counted in the loop body.
    pub total_ops: u64,
    /// Total bytes transferred over PCIe (all field reads + writes).
    pub total_bytes: u64,
    /// Arithmetic intensity: ops / byte.
    pub arithmetic_intensity: f64,
    /// Estimated CPU execution time in nanoseconds (single core).
    pub estimated_cpu_ns: f64,
    /// Estimated GPU execution time in nanoseconds (including PCIe).
    pub estimated_gpu_ns: f64,
    /// Minimum N where GPU becomes faster than CPU.
    pub crossover_point: u64,
    /// Recommended offload decision.
    pub recommended: OffloadDecision,
}

/// The compiler's decision about whether to offload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OffloadDecision {
    /// GPU is faster — offload.
    Gpu,
    /// CPU is faster — keep on CPU.
    Cpu,
    /// N is runtime-determined — emit dispatch branch with crossover.
    Runtime,
}

/// Run the cost model on a kernel body with the given iteration count.
///
/// `N` is the iteration count (compile-time known or 0 for runtime).
pub fn estimate(body: &[Statement], n: u64) -> GpuCostEstimate {
    let total_ops = count_operations(body);
    let total_bytes = count_bytes_transferred(body);
    let intensity = if total_bytes > 0 {
        total_ops as f64 / total_bytes as f64
    } else {
        f64::MAX
    };

    let arith_intensity = intensity;

    // Estimate per-iteration compute time (operations only, not data transfer).
    // Data transfer (PCIe) is a fixed overhead, not per-iteration.
    let per_iter_cpu_ns = (total_ops as f64) * CPU_CYCLE_NS;
    let estimated_cpu_ns = (n as f64) * per_iter_cpu_ns;

    // Estimate GPU time: PCIe transfer (fixed) + parallel execution
    // GPU has ~256 cores, so per-iteration time = ops / cores * clock
    let per_iter_gpu_ns = (total_ops as f64) * GPU_CYCLE_NS / 256.0;
    let estimated_gpu_ns = PCIE_LATENCY_NS + (n as f64) * per_iter_gpu_ns;

    // Crossover: GPU becomes faster when estimated_gpu_ns < estimated_cpu_ns
    // PCIE_LATENCY_NS + N * per_iter_gpu_ns < N * per_iter_cpu_ns
    // N * (per_iter_cpu_ns - per_iter_gpu_ns) > PCIE_LATENCY_NS
    // N > PCIE_LATENCY_NS / (per_iter_cpu_ns - per_iter_gpu_ns)
    let crossover = if per_iter_cpu_ns > per_iter_gpu_ns {
        let crossover_f = PCIE_LATENCY_NS / (per_iter_cpu_ns - per_iter_gpu_ns);
        if crossover_f.is_finite() {
            crossover_f.ceil() as u64
        } else {
            u64::MAX
        }
    } else {
        u64::MAX
    };

    let recommended = if n == 0 {
        // Runtime-determined N — need runtime dispatch branch.
        OffloadDecision::Runtime
    } else if n < MIN_ITERATIONS && arith_intensity < MIN_ARITHMETIC_INTENSITY {
        // Small loop with low intensity — definitely CPU.
        OffloadDecision::Cpu
    } else if n >= crossover || n >= MIN_ITERATIONS * 1000 {
        // Large enough to justify GPU offload.
        OffloadDecision::Gpu
    } else if arith_intensity < MIN_ARITHMETIC_INTENSITY {
        OffloadDecision::Cpu
    } else {
        OffloadDecision::Cpu
    };

    GpuCostEstimate {
        total_ops,
        total_bytes,
        arithmetic_intensity: arith_intensity,
        estimated_cpu_ns,
        estimated_gpu_ns,
        crossover_point: crossover,
        recommended,
    }
}

/// Count the total number of arithmetic operations in a statement list.
fn count_operations(body: &[Statement]) -> u64 {
    let mut count = 0;
    for stmt in body {
        count += count_stmt_ops(stmt);
    }
    count
}

fn count_stmt_ops(stmt: &Statement) -> u64 {
    match stmt {
        Statement::Assign(_, expr) => count_expr_ops(expr),
        Statement::Guarded(condition, statements) => {
            count_expr_ops(condition) + count_operations(statements)
        }
        _ => 0,
    }
}

fn count_expr_ops(expr: &Expr) -> u64 {
    match expr {
        Expr::BinaryOp(BinaryOpKind::Add, l, r)
        | Expr::BinaryOp(BinaryOpKind::Sub, l, r)
        | Expr::BinaryOp(BinaryOpKind::Mul, l, r)
        | Expr::BinaryOp(BinaryOpKind::Div, l, r)
        | Expr::BinaryOp(BinaryOpKind::Mod, l, r) => 1 + count_expr_ops(l) + count_expr_ops(r),
        Expr::BinaryOp(BinaryOpKind::And, l, r)
        | Expr::BinaryOp(BinaryOpKind::Or, l, r)
        | Expr::BinaryOp(BinaryOpKind::Eq, l, r)
        | Expr::BinaryOp(BinaryOpKind::Neq, l, r)
        | Expr::BinaryOp(BinaryOpKind::Lt, l, r)
        | Expr::BinaryOp(BinaryOpKind::Le, l, r)
        | Expr::BinaryOp(BinaryOpKind::Gt, l, r)
        | Expr::BinaryOp(BinaryOpKind::Ge, l, r) => 1 + count_expr_ops(l) + count_expr_ops(r),
        Expr::BinaryOp(BinaryOpKind::BitAnd, l, r)
        | Expr::BinaryOp(BinaryOpKind::BitOr, l, r)
        | Expr::BinaryOp(BinaryOpKind::BitXor, l, r) => 1 + count_expr_ops(l) + count_expr_ops(r),
        Expr::BinaryOp(BinaryOpKind::Shl, l, r) | Expr::BinaryOp(BinaryOpKind::Shr, l, r) => {
            1 + count_expr_ops(l) + count_expr_ops(r)
        }
        Expr::UnaryOp(UnaryOpKind::Not, e)
        | Expr::UnaryOp(UnaryOpKind::Neg, e)
        | Expr::UnaryOp(UnaryOpKind::BitNot, e) => 1 + count_expr_ops(e),
        // Calls — recurse into args
        Expr::Call(_, args, _) => args.iter().map(|a| count_expr_ops(a)).sum(),
        _ => 0,
    }
}

/// Count bytes transferred over PCIe for all field accesses.
fn count_bytes_transferred(body: &[Statement]) -> u64 {
    let mut fields: Vec<String> = Vec::new();
    for stmt in body {
        collect_field_refs(stmt, &mut fields);
    }
    // Each field is 8 bytes (i64). PCIe round trip: read + write.
    fields.len() as u64 * 8 * 2
}

fn collect_field_refs(stmt: &Statement, fields: &mut Vec<String>) {
    match stmt {
        Statement::Assign(lhs, expr) => {
            if let Expr::Identifier(f) = lhs {
                if !fields.contains(f) {
                    fields.push(f.clone());
                }
            }
            collect_expr_field_refs(expr, fields);
        }
        Statement::Guarded(condition, statements) => {
            collect_expr_field_refs(condition, fields);
            for s in statements {
                collect_field_refs(s, fields);
            }
        }
        _ => {}
    }
}

fn collect_expr_field_refs(expr: &Expr, fields: &mut Vec<String>) {
    match expr {
        Expr::Identifier(name) => {
            if !fields.contains(name) {
                fields.push(name.clone());
            }
        }
        Expr::BinaryOp(BinaryOpKind::Add, l, r)
        | Expr::BinaryOp(BinaryOpKind::Sub, l, r)
        | Expr::BinaryOp(BinaryOpKind::Mul, l, r)
        | Expr::BinaryOp(BinaryOpKind::Div, l, r)
        | Expr::BinaryOp(BinaryOpKind::And, l, r)
        | Expr::BinaryOp(BinaryOpKind::Or, l, r)
        | Expr::BinaryOp(BinaryOpKind::Eq, l, r)
        | Expr::BinaryOp(BinaryOpKind::Neq, l, r)
        | Expr::BinaryOp(BinaryOpKind::Lt, l, r)
        | Expr::BinaryOp(BinaryOpKind::Le, l, r)
        | Expr::BinaryOp(BinaryOpKind::Gt, l, r)
        | Expr::BinaryOp(BinaryOpKind::Ge, l, r)
        | Expr::BinaryOp(BinaryOpKind::BitAnd, l, r)
        | Expr::BinaryOp(BinaryOpKind::BitOr, l, r)
        | Expr::BinaryOp(BinaryOpKind::BitXor, l, r)
        | Expr::BinaryOp(BinaryOpKind::Shl, l, r)
        | Expr::BinaryOp(BinaryOpKind::Shr, l, r) => {
            collect_expr_field_refs(l, fields);
            collect_expr_field_refs(r, fields);
        }
        Expr::UnaryOp(UnaryOpKind::Not, e)
        | Expr::UnaryOp(UnaryOpKind::Neg, e)
        | Expr::UnaryOp(UnaryOpKind::BitNot, e) => {
            collect_expr_field_refs(e, fields);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assign(lhs: &str, expr: Expr) -> Statement {
        Statement::Assign(Expr::Identifier(lhs.to_string()), expr)
    }

    #[test]
    fn test_simple_add_is_low_intensity() {
        // data = data + 1 → 1 add op, 1 field read + 1 write = 16 bytes
        let body = vec![assign(
            "x",
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let est = estimate(&body, 100);
        assert!(
            est.arithmetic_intensity < 1.0,
            "1 op / 16 bytes should be < 1"
        );
        assert_eq!(est.recommended, OffloadDecision::Cpu);
    }

    #[test]
    fn test_heavy_math_is_high_intensity() {
        // x = x * y + z / w — 3 ops on 4 fields
        let body = vec![assign(
            "x",
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::BinaryOp(
                    BinaryOpKind::Mul,
                    Box::new(Expr::Identifier("x".to_string())),
                    Box::new(Expr::Identifier("y".to_string())),
                )),
                Box::new(Expr::BinaryOp(
                    BinaryOpKind::Div,
                    Box::new(Expr::Identifier("z".to_string())),
                    Box::new(Expr::Identifier("w".to_string())),
                )),
            ),
        )];
        let est = estimate(&body, 100_000);
        // 4 fields * 8 * 2 = 64 bytes; 3 ops; intensity = 3/64 ≈ 0.047
        // Small loop body — still CPU for simple math
        assert!(
            est.recommended == OffloadDecision::Cpu || est.recommended == OffloadDecision::Runtime,
            "heavy math on 4 fields should not auto-offload without high N"
        );
    }

    #[test]
    fn test_large_n_recommends_gpu() {
        let body = vec![assign(
            "x",
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        // N = 10^8 — large enough to justify PCIe even for simple math
        let est = estimate(&body, 100_000_000);
        assert_eq!(
            est.recommended,
            OffloadDecision::Gpu,
            "large N should recommend GPU"
        );
        assert!(
            est.estimated_gpu_ns < est.estimated_cpu_ns,
            "GPU should be faster than CPU at large N"
        );
    }

    #[test]
    fn test_runtime_n_returns_runtime_decision() {
        let body = vec![assign("x", Expr::Decimal(42))];
        let est = estimate(&body, 0); // N=0 means runtime-determined
        assert_eq!(est.recommended, OffloadDecision::Runtime);
    }

    #[test]
    fn test_crossover_point_monotonic() {
        let body = vec![assign(
            "x",
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let est = estimate(&body, 1_000_000_000);
        assert!(est.crossover_point > 0, "crossover should be positive");
        // At 1B iterations, GPU should be recommended
        assert_eq!(est.recommended, OffloadDecision::Gpu);
    }
}
