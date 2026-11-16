// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Signed word intervals: lower > upper denotes the empty set explicitly.
use crate::ast::BinaryOpKind as Op;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Relation {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Relation {
    pub(super) fn from_ast(op: Op) -> Option<Self> {
        match op {
            Op::Eq => Some(Self::Eq),
            Op::Neq => Some(Self::Ne),
            Op::Lt => Some(Self::Lt),
            Op::Le => Some(Self::Le),
            Op::Gt => Some(Self::Gt),
            Op::Ge => Some(Self::Ge),
            _ => None,
        }
    }

    pub(super) fn mirror(self) -> Self {
        match self {
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
            other => other,
        }
    }

    pub(super) fn negate(self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Lt => Self::Ge,
            Self::Le => Self::Gt,
            Self::Gt => Self::Le,
            Self::Ge => Self::Lt,
        }
    }

    pub(super) fn holds(self, x: i64, y: i64) -> bool {
        match self {
            Self::Eq => x == y,
            Self::Ne => x != y,
            Self::Lt => x < y,
            Self::Le => x <= y,
            Self::Gt => x > y,
            Self::Ge => x >= y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Interval {
    pub lower: i64,
    pub upper: i64,
}

impl Interval {
    pub(super) const FULL: Self = Self {
        lower: i64::MIN,
        upper: i64::MAX,
    };
    const EMPTY: Self = Self { lower: 1, upper: 0 };

    pub(super) fn is_empty(self) -> bool {
        self.lower > self.upper
    }
    pub(super) fn contains(self, value: i64) -> bool {
        self.lower <= value && value <= self.upper
    }
    pub(super) fn point(self) -> Option<i64> {
        (self.lower == self.upper).then_some(self.lower)
    }

    pub(super) fn intersect(self, other: Self) -> Self {
        Self {
            lower: self.lower.max(other.lower),
            upper: self.upper.min(other.upper),
        }
    }

    /// Keep every satisfying member. Interior disequalities retain their hull;
    /// all order/equality restrictions represent the intersection exactly.
    pub(super) fn restrict(self, relation: Relation, value: i64) -> Self {
        if self.is_empty() {
            return self;
        }
        match relation {
            Relation::Eq => self.intersect(Self {
                lower: value,
                upper: value,
            }),
            Relation::Ge => self.intersect(Self {
                lower: value,
                ..Self::FULL
            }),
            Relation::Le => self.intersect(Self {
                upper: value,
                ..Self::FULL
            }),
            Relation::Gt => value.checked_add(1).map_or(Self::EMPTY, |lower| {
                self.intersect(Self {
                    lower,
                    ..Self::FULL
                })
            }),
            Relation::Lt => value.checked_sub(1).map_or(Self::EMPTY, |upper| {
                self.intersect(Self {
                    upper,
                    ..Self::FULL
                })
            }),
            Relation::Ne => self.exclude_endpoint(value),
        }
    }

    fn exclude_endpoint(self, value: i64) -> Self {
        if self.point() == Some(value) {
            return Self::EMPTY;
        }
        if self.lower == value {
            return Self {
                lower: value + 1,
                ..self
            };
        }
        if self.upper == value {
            return Self {
                upper: value - 1,
                ..self
            };
        }
        self
    }

    /// Return a verdict only if every member has that verdict. Empty paths are
    /// handled by the caller, never by inventing a value for an empty interval.
    pub(super) fn classify(self, relation: Relation, value: i64) -> Option<bool> {
        if self.is_empty() {
            return None;
        }
        match relation {
            Relation::Eq => {
                if self.point() == Some(value) {
                    return Some(true);
                }
                (!self.contains(value)).then_some(false)
            }
            Relation::Ne => self.classify(Relation::Eq, value).map(|equal| !equal),
            Relation::Lt => decide(self.upper < value, self.lower >= value),
            Relation::Le => decide(self.upper <= value, self.lower > value),
            Relation::Gt => decide(self.lower > value, self.upper <= value),
            Relation::Ge => decide(self.lower >= value, self.upper < value),
        }
    }
}

fn decide(all_true: bool, all_false: bool) -> Option<bool> {
    if all_true {
        return Some(true);
    }
    all_false.then_some(false)
}

#[cfg(kani)]
mod proofs {
    use super::*;
    fn relation() -> Relation {
        match kani::any::<u8>() % 6 {
            0 => Relation::Eq,
            1 => Relation::Ne,
            2 => Relation::Lt,
            3 => Relation::Le,
            4 => Relation::Gt,
            _ => Relation::Ge,
        }
    }
    fn interval() -> Interval {
        Interval {
            lower: kani::any(),
            upper: kani::any(),
        }
    }

    #[kani::proof]
    fn interval_intersection_is_exact() {
        let a = interval();
        let b = interval();
        let x = kani::any();
        assert_eq!(a.intersect(b).contains(x), a.contains(x) && b.contains(x));
        kani::cover!(a.is_empty());
        kani::cover!(!a.intersect(b).is_empty());
    }

    #[kani::proof]
    fn interval_restriction_is_sound() {
        let old = interval();
        let op = relation();
        let c = kani::any();
        let x = kani::any();
        let new = old.restrict(op, c);
        let satisfies = old.contains(x) && op.holds(x, c);
        assert!(!satisfies || new.contains(x));
        assert!(!new.contains(x) || old.contains(x));
        if op != Relation::Ne {
            assert_eq!(new.contains(x), satisfies);
        }
        kani::cover!(op == Relation::Gt && c == i64::MAX && new.is_empty());
        kani::cover!(op == Relation::Lt && c == i64::MIN && new.is_empty());
        kani::cover!(satisfies);
    }

    #[kani::proof]
    fn interval_classification_is_sound() {
        let bounds = interval();
        let op = relation();
        let c = kani::any();
        let x = kani::any();
        let verdict = bounds.classify(op, c);
        if let Some(value) = verdict {
            assert!(!bounds.contains(x) || op.holds(x, c) == value);
        }
        assert!(!bounds.is_empty() || verdict.is_none());
        kani::cover!(bounds.contains(x) && verdict == Some(true));
        kani::cover!(bounds.contains(x) && verdict == Some(false));
        kani::cover!(!bounds.is_empty() && verdict.is_none());
    }

    #[kani::proof]
    fn interval_mirrored_comparison_is_exact() {
        let op = relation();
        let x = kani::any();
        let y = kani::any();
        assert_eq!(op.holds(x, y), op.mirror().holds(y, x));
    }

    #[kani::proof]
    fn interval_negated_comparison_is_exact() {
        let op = relation();
        let x = kani::any();
        let y = kani::any();
        assert_eq!(op.holds(x, y), !op.negate().holds(x, y));
    }
}
