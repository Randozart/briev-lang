// ── Expression AST Definitions ─────────────────────────────────────────
// 2026-07-12: Phase 0.2 — New architecture expression types.
// No IntrinsicCall variant — Sqrt#(x) is Call("Sqrt#", [x]).
// No "feature" wrapper types — unified Expr enum only.

use crate::ast::Type;
use crate::errors::Span;

/// The storage class of a spawned obj instance (2026-08-09, Phase 5).
/// Strategy-keyword surface: the default (Pooled) is the efficient path — a
/// keyword may only *reveal* a choice the pool decoder cannot make alone,
/// never beat a working default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnStorage {
    /// The instance lives in the obj's static pool column (the default).
    Pooled,
    /// `box spawn Obj(...)` — per-instance-heap: the instance is its own
    /// heap allocation, NOT a row in the pool. Explicit class when the pool
    /// decoder is ambiguous.
    Box,
    /// `spill spawn Obj(...)` — allowed to grow into a growable buffer when a
    /// static pool column can't hold the proven worst case.
    Spill,
}

impl SpawnStorage {
    /// The storage-class keyword for display, or "" for the default (pooled).
    pub fn keyword(&self) -> &'static str {
        match self {
            SpawnStorage::Pooled => "",
            SpawnStorage::Box => "box",
            SpawnStorage::Spill => "spill",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // ── Literals ────────────────────────────────────────────────
    Quoted(Vec<u8>), // "..." raw bytes
    Decimal(i64),    // 42, 16711680
    /// 2026-08-01: a character literal `'a'` — a `Char` protocol value (the
    /// code point). The `Cast.Char` universe property makes it type-distinct
    /// from Int, so `Print#` dispatches it to `__print_char`. The code point is
    /// stored as i64 (boxed, like Decimal).
    Char(char),
    /// 2026-07-20: Tagged literal with discriminator prefix/suffix.
    /// Second field is the discriminator tag: "0x", "h", "bf", etc.
    /// Used by op Parse(Decimal, pre: "0x") for routing.
    TaggedLiteral(i64, String), // 0xFF00FF, FF00FFh, 1.5bf
    /// 2026-07-27: Tagged quoted string: sql"SELECT", my"hello"
    /// First field is the string content, second is the prefix tag.
    TaggedQuotedLiteral(Vec<u8>, String),
    Bool(bool),      // true, false
    Float(f64),      // 3.14
    /// 2026-08-06 (beginprogram plan): the `beginprogram` precondition marker —
    /// true exactly once at program start. Only meaningful in a node's
    /// precondition; the node it annotates is an entry loop.
    BeginProgram,

    // ── References ──────────────────────────────────────────────
    Identifier(String), // foo, Sqrt#, AddI64#

    // ── Operations ──────────────────────────────────────────────
    Call(String, Vec<Expr>, Option<usize>), // f(x), Sqrt#(x) — analysis_id for Alloc# strategy
    BinaryOp(BinaryOpKind, Box<Expr>, Box<Expr>),
    UnaryOp(UnaryOpKind, Box<Expr>),
    Field(Box<Expr>, String),
    /// 2026-07-31: Method call with a receiver: a.f(x). The receiver is
    /// preserved and bound to the `self` parameter of the obj member.
    /// 2026-09-16: Fifth field carries chain back-references (`.N>>` / `.name>>`).
    MethodCall(Box<Expr>, String, Vec<Expr>, Option<usize>, Vec<ChainRef>),
    /// 2026-07-31: Reflection access: `a.^Length` (runtime) / `a.^^Size`
    /// (compile-time). The receiver is preserved; the target is a PascalCase
    /// compiler-known identifier resolved by the D1 reflection table.
    Reflect(Box<Expr>, String, ReflectKind),
    Index(Box<Expr>, Box<Expr>),
    /// arr[start:end:stride] — zero-copy slice view
    Slice {
        array: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        stride: Option<Box<Expr>>,
    },
    /// 2026-08-07 (Phase 7): an iterable range — `start..end` (half-open) or
    /// `start..=end` (inclusive). Consumed by `foreach` (SPEC §11.4 counted
    /// iteration).
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
    /// 2026-08-07 (object instance pools): `spawn Obj(args)` — create an obj
    /// instance from its Init member + return a linear handle (the pool row
    /// id). SPEC §12.2.
    /// 2026-08-09 (Phase 5): `box spawn Obj(args)` / `spill spawn Obj(args)`
    /// set the storage class — per-instance-heap (not pooled) or growable
    /// (a static pool column that can't hold the worst case). Default Pooled.
    Spawn {
        type_name: String,
        args: Vec<Expr>,
        storage: SpawnStorage,
    },

    // ── Control flow ────────────────────────────────────────────
    Block(Vec<super::Statement>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Match(Box<Expr>, Vec<MatchArm>),

    // ── Compound values ─────────────────────────────────────────
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    /// struct Name { field: expr; ... } — struct literal.
    /// 2026-07-24: Constructs a value of a static struct type.
    StructLiteral {
        type_name: String,
        fields: Vec<(String, Expr)>,
    },

    // ── Functions ───────────────────────────────────────────────
    Lambda(Vec<String>, Box<Expr>),

    // ── Type operations ─────────────────────────────────────────
    Cast(Box<Expr>, Type),
    IsType(Box<Expr>, Type),

    // ── Scope ───────────────────────────────────────────────────
    Within(Box<Expr>, Box<Expr>),

    // ── Derivation ──────────────────────────────────────────────
    DerivationBlock(Box<DerivationBlock>),

    // ── Pointers ────────────────────────────────────────────────
    Deref(Box<Expr>),
    AddrOf(Box<Expr>),

    /// 2026-08-01 (Phase 3): a consumptive operand — the value is used by the
    /// enclosing op (`a ~= b`, `a ~+ b`, `dest ~<- src`, `~<- src;`), then its
    /// backing storage is destroyed. Use-after-move of the consumed local is a
    /// compile error (the move pass tracks it). Only a mutable lvalue can be
    /// consumed — `~op` on a constant is a compile error.
    Consume(Box<Expr>),
    /// 2026-08-09 (Phase 10): `await task` — consume a task handle and yield
    /// the callable's declared result (SPEC §12.2). The reference scheduler is
    /// deterministic, so the handle already holds the result; `await` reads it
    /// and marks the handle consumed (a later use is an error).
    Await(Box<Expr>),

    // ── Plugin intercept ────────────────────────────────────────
    // 2026-07-19: name!(args). Resolved by Front or Mid stage plugins.
    // 2026-09-16: `receiver` supports chained plugin calls: obj.plugin!(args).
    // `chain_refs` supports positional/named back-references in chains.
    PluginIntercept {
        name: String,
        args: Vec<Expr>,
        type_args: Vec<Type>,
        receiver: Option<Box<Expr>>,
        chain_refs: Vec<ChainRef>,
    },

    // ── Chain capture ──────────────────────────────────────────
    // 2026-09-16: `expr >> name;` — capture a chain result into a named variable.
    Capture {
        expr: Box<Expr>,
        name: String,
    },

    // ── Metadata ────────────────────────────────────────────────
    FormattingAnnotation(super::Formatting),
    /// 2026-07-25: fn? — compile-time existence check. Evaluates to
    /// Bool(true) if the function linked, Bool(false) otherwise.
    /// Used for guarding frgn?/frgn!/frgn?! calls.
    Exists(String),

    // ── Electronics ─────────────────────────────────────────────
    /// `net <name>: <expr>` — named net annotation on a precondition
    /// equality. The name is metadata attached to the equivalence class;
    /// inference is unchanged. Only meaningful in contract preconditions.
    Named { name: String, inner: Box<Expr> },
    /// `<number><unit>` — a numeric literal with a physics unit suffix.
    /// `3.3V` (volts), `10mA` (milliamps), `330R` (ohms), etc.
    /// Parsed from adjacent numeric + identifier tokens; the analysis
    /// pass interprets the unit.
    UnitLiteral { value: f64, unit: String },
}

/// 2026-07-31: Reflection kind — distinguishes value-derived (runtime)
/// reflection from type-derived (compile-time, foldable) reflection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectKind {
    /// `x.^Length`, `x.^Ptr` — runtime value-derived.
    Runtime,
    /// `x.^^Size`, `x.^^Bytes` — compile-time type-derived, foldable.
    CompileTime,
}

/// 2026-09-16: A reference to a previous chain result — positional or named.
/// Used in `MethodCall` and `PluginIntercept` to support back-references
/// (`.N>>`) and named capture references (`.name>>`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChainRef {
    /// `.N>>` — N operations back (1-indexed, .1 = previous).
    Positional(usize),
    /// `.name>>` — reference to a named capture from a `>> name` statement.
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpKind {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOpKind {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Literal(Expr),
    Binding(String),
    /// 2026-08-22 (spec-conformance plan Phase 3, SPEC §8.4): a typed
    /// binding arm of a structural sum — `number: Int => use_int(number)`.
    /// Matches only when the scrutinee's type is this sum member; binds the
    /// name at the member's type.
    TypedBinding(String, Box<crate::ast::Type>),
    EnumVariant(String, Vec<Pattern>),
    Tuple(Vec<Pattern>),
    Range(Expr, Expr),
    /// 2026-08-06 (Phase 7): inclusive `a..=b` range pattern.
    RangeInclusive(Expr, Expr),
    /// 2026-08-23 (F1 unified dispatch): `|` alternatives within one arm.
    Multi(Vec<Pattern>),
}

/// A derivation block attached to a definition or transaction via `:=`.
/// Contains input-output examples for inductive synthesis.
/// 2026-07-28: Added tolerance field for FP relaxed equivalence — optional
/// f64 relative tolerance. When Some(tol), the synthesized expression must
/// produce output within tol * |expected| of each example's expected output.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivationExample {
    pub inputs: Vec<Expr>,
    pub output: Box<Expr>,
    /// 2026-07-28: Optional relative tolerance for FP relaxed equivalence.
    /// Syntax: `input -> [tol] output;` in derivation blocks.
    pub tolerance: Option<f64>,
    pub span: Span,
}

/// 2026-07-29: A segment in the := verification chain.
/// Each segment is one body: an asm function name, a derivation block,
/// or a reference function name.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainSegment {
    /// Reference to an asm function or defn: := name
    Ref(String),
    /// Derivation block with examples and optional reference
    Derivation(Box<DerivationBlock>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DerivationBlock {
    pub examples: Vec<DerivationExample>,
    pub synthesized: Option<Box<Expr>>,
    /// 2026-07-28: Optional postcondition for full-spec CEGIS verification.
    /// Parsed from [[post]] syntax: [[ post ] or [[ pre ]][ post ].
    /// The postcondition is a predicate using #Term to refer to the
    /// function's return value (e.g., [[ #Term >= 0 ]).
    pub postcondition: Option<Expr>,
    /// 2026-07-29: Optional precondition stating valid input states.
    /// Parsed from [[ pre ]] syntax before the postcondition block.
    /// Example: [[ x0 >= 0 ]][ #Term >= 0 ] — only verify for non-negative inputs.
    pub precondition: Option<Expr>,
    /// 2026-07-29: Optional reference function for correctness verification.
    /// `verifying ref_fn` declares an existing function as the oracle for CEGIS.
    /// The synthesized function must match ref_fn for ALL inputs (within tolerance).
    /// ref_name: name of the reference function (must be a defn in the same unit).
    /// ref_tolerance: allowed deviation (default 0.0 for exact match).
    pub ref_name: Option<String>,
    pub ref_tolerance: Option<f64>,
    /// 2026-07-29: Multi-segment verification chain.
    /// If non-empty, body is selected by cross-verification at compile time.
    pub chain: Vec<ChainSegment>,
    pub span: Span,
}

impl Expr {
    /// Return the identifier name if this expression is a simple variable reference.
    pub fn as_var_name(&self) -> Option<&str> {
        if let Expr::Identifier(name) = self {
            Some(name.as_str())
        } else {
            None
        }
    }

    /// Collect all variable (Identifier) names referenced in this expression tree.
    pub fn collect_vars(&self) -> Vec<String> {
        let mut vars = Vec::new();
        self.collect_vars_into(&mut vars);
        vars
    }

    fn collect_vars_into(&self, acc: &mut Vec<String>) {
        match self {
            Expr::Identifier(name) => acc.push(name.clone()),
            Expr::BinaryOp(_, l, r) => {
                l.collect_vars_into(acc);
                r.collect_vars_into(acc);
            }
            Expr::UnaryOp(_, e) => e.collect_vars_into(acc),
            Expr::Field(e, _) => e.collect_vars_into(acc),
            Expr::MethodCall(recv, _, args, _, _) => {
                recv.collect_vars_into(acc);
                for a in args {
                    a.collect_vars_into(acc);
                }
            }
            Expr::Reflect(recv, _, _) => recv.collect_vars_into(acc),
            Expr::Index(l, r) => {
                l.collect_vars_into(acc);
                r.collect_vars_into(acc);
            }
            Expr::Call(_, args, _) => {
                for a in args {
                    a.collect_vars_into(acc);
                }
            }
            Expr::Block(_stmts) => {}
            Expr::If(c, t, e) => {
                c.collect_vars_into(acc);
                t.collect_vars_into(acc);
                if let Some(e) = e {
                    e.collect_vars_into(acc);
                }
            }
            Expr::Match(e, arms) => {
                e.collect_vars_into(acc);
                for arm in arms {
                    arm.body.collect_vars_into(acc);
                    if let Some(g) = &arm.guard {
                        g.collect_vars_into(acc);
                    }
                }
            }
            Expr::Tuple(items) => {
                for i in items {
                    i.collect_vars_into(acc);
                }
            }
            Expr::List(items) => {
                for i in items {
                    i.collect_vars_into(acc);
                }
            }
            Expr::Lambda(_, body) => body.collect_vars_into(acc),
            Expr::Cast(e, _) => e.collect_vars_into(acc),
            Expr::IsType(e, _) => e.collect_vars_into(acc),
            Expr::Within(l, r) => {
                l.collect_vars_into(acc);
                r.collect_vars_into(acc);
            }
            Expr::DerivationBlock(d) => {
                for ex in &d.examples {
                    for inp in &ex.inputs {
                        inp.collect_vars_into(acc);
                    }
                }
            }
            Expr::Deref(inner) | Expr::AddrOf(inner) | Expr::Consume(inner) | Expr::Await(inner) => inner.collect_vars_into(acc),
            _ => {}
        }
    }
}
