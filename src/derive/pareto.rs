// ── Phase F.4 — Pareto Frontier ───────────────────────────────────────
// 2026-07-28: Phase F.4 — Pareto frontier tracking and knee selection
// for MCMC multi-objective optimization (correctness × performance).
// Flat code: each function max 2 levels of nesting.

use crate::derive::engine::SynthesizedProgram;

/// 2026-07-28: Phase F.4 — A point on the Pareto frontier.
#[derive(Debug, Clone)]
pub struct ParetoPoint {
    pub program: SynthesizedProgram,
    pub error_count: u64,
    pub op_count: u64,
    pub runtime_ns: Option<u64>,
}

impl ParetoPoint {
    /// Does this point dominate another?
    /// A dominates B if it is no worse in all objectives and strictly better
    /// in at least one (standard Pareto dominance).
    pub fn dominates(&self, other: &ParetoPoint) -> bool {
        let no_worse = self.error_count <= other.error_count && self.op_count <= other.op_count;
        let strictly_better =
            self.error_count < other.error_count || self.op_count < other.op_count;
        no_worse && strictly_better
    }
}

/// 2026-07-28: Phase F.4 — Pareto frontier (set of non-dominated programs).
#[derive(Debug, Clone)]
pub struct ParetoFrontier {
    pub points: Vec<ParetoPoint>,
}

impl ParetoFrontier {
    pub fn new() -> Self {
        ParetoFrontier { points: Vec::new() }
    }

    /// Add a point and update the frontier.
    /// Returns true if the point is on the frontier (non-dominated).
    pub fn insert(&mut self, point: ParetoPoint) -> bool {
        self.points.retain(|p| !point.dominates(p));
        if self.points.iter().any(|p| p.dominates(&point)) {
            return false;
        }
        self.points.push(point);
        true
    }

    /// Select the "knee" of the Pareto frontier using the angle method.
    /// The knee is the point farthest from the line connecting the
    /// min-error (high-ops) and min-ops (high-error) extremes.
    pub fn select_knee(&self) -> Option<&ParetoPoint> {
        if self.points.is_empty() {
            return None;
        }
        if self.points.len() <= 2 {
            return self.points.iter().min_by_key(|p| p.error_count);
        }

        let min_err = self.points.iter().map(|p| p.error_count).min().unwrap_or(0) as f64;
        let max_err = self
            .points
            .iter()
            .map(|p| p.error_count)
            .max()
            .unwrap_or(1)
            .max(1) as f64;
        let min_op = self.points.iter().map(|p| p.op_count).min().unwrap_or(0) as f64;
        let max_op = self
            .points
            .iter()
            .map(|p| p.op_count)
            .max()
            .unwrap_or(1)
            .max(1) as f64;

        let extreme_a = (min_err, max_op);
        let extreme_b = (max_err, min_op);

        self.points
            .iter()
            .map(|p| {
                let px = p.error_count as f64;
                let py = p.op_count as f64;
                let dist = point_line_distance((px, py), extreme_a, extreme_b);
                (dist, p)
            })
            .max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, p)| p)
    }
}

/// Distance from a point to a line defined by two points.
fn point_line_distance(point: (f64, f64), line_a: (f64, f64), line_b: (f64, f64)) -> f64 {
    let (px, py) = point;
    let (ax, ay) = line_a;
    let (bx, by) = line_b;
    let dx = bx - ax;
    let dy = by - ay;
    let numerator = ((dy * px - dx * py) + (bx * ay - by * ax)).abs();
    let denominator = (dx * dx + dy * dy).sqrt();
    if denominator < 1e-10 {
        0.0
    } else {
        numerator / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;

    fn make_point(program: SynthesizedProgram, errors: u64, ops: u64) -> ParetoPoint {
        ParetoPoint {
            program,
            error_count: errors,
            op_count: ops,
            runtime_ns: None,
        }
    }

    #[test]
    fn test_pareto_insert_dominated() {
        let mut frontier = ParetoFrontier::new();
        let prog = SynthesizedProgram {
            body: vec![Expr::Decimal(0)],
            cost: 0,
            depth: 0,
            helpers: vec![],
        };
        let p1 = make_point(prog.clone(), 5, 10);
        let p2 = make_point(prog.clone(), 5, 15); // more ops, same errors → dominated
        assert!(frontier.insert(p1));
        assert!(!frontier.insert(p2)); // dominated → not on frontier
    }

    #[test]
    fn test_pareto_insert_non_dominated() {
        let mut frontier = ParetoFrontier::new();
        let prog = SynthesizedProgram {
            body: vec![Expr::Decimal(0)],
            cost: 0,
            depth: 0,
            helpers: vec![],
        };
        let p1 = make_point(prog.clone(), 5, 10);
        let p2 = make_point(prog.clone(), 3, 12); // fewer errors, more ops → not dominated
        assert!(frontier.insert(p1));
        assert!(frontier.insert(p2)); // non-dominated → on frontier
    }

    #[test]
    fn test_pareto_knee_selection() {
        let mut frontier = ParetoFrontier::new();
        let prog = SynthesizedProgram {
            body: vec![Expr::Decimal(0)],
            cost: 0,
            depth: 0,
            helpers: vec![],
        };
        frontier.insert(make_point(prog.clone(), 0, 20)); // low error, high ops
        frontier.insert(make_point(prog.clone(), 10, 2)); // high error, low ops
        frontier.insert(make_point(prog.clone(), 3, 8)); // knee: middle trade-off
        let knee = frontier.select_knee().unwrap();
        assert_eq!(knee.error_count, 3);
        assert_eq!(knee.op_count, 8);
    }

    #[test]
    fn test_pareto_knee_single_point() {
        let mut frontier = ParetoFrontier::new();
        let prog = SynthesizedProgram {
            body: vec![Expr::Decimal(0)],
            cost: 0,
            depth: 0,
            helpers: vec![],
        };
        frontier.insert(make_point(prog.clone(), 2, 5));
        let knee = frontier.select_knee().unwrap();
        assert_eq!(knee.error_count, 2);
    }

    #[test]
    fn test_pareto_empty_frontier() {
        let frontier = ParetoFrontier::new();
        assert!(frontier.select_knee().is_none());
    }
}
