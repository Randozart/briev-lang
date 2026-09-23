// ── Type System Definitions ─────────────────────────────────────────────
// 2026-07-12: Phase 0.2 — New architecture type system.
// All types are Bits(N) with metadata overlays. No built-in primitives.
// 2026-08-13 (layout-keywords plan): Bits(N) stores a BIT count (the original
// thesis restored). From 2026-07-12 through 2026-08-12 it stored BYTES, which
// silently destroyed sub-byte widths (Bits<4> became 4 bytes); that era is
// the "byte-era" referenced by the migration comments below.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Raw bit sequence: Bits(64) = 64 bits = 8 bytes. The sole physical primitive.
    /// Stores a BIT count. Use `Type::from_bits(n)` or `Type::bits(n)` to create.
    /// `Bits(0)` = flexible-width Bits (resolved to a concrete width later).
    Bits(u64),
    /// Void = Bits(0). Not a separate concept.
    Void,
    /// User-named type: Custom("Int"), Custom("String"), Custom("MyType").
    Custom(String),
    /// Generic type reference: Generic("List", vec!["T".to_string()])
    Generic(String, Vec<Type>),
    /// Applied generic: Applied("List", [Custom("Int")])
    Applied(String, Vec<Type>),
    /// Union type: Union([Int, String, Error])
    Union(Vec<Type>),
    /// Tuple type: Tuple([Int, String])
    Tuple(Vec<Type>),
    /// Type variable for generics
    TypeVar(String),
    /// 2026-08-22 (spec-conformance plan Phase 5, SPEC §8.6): a trait object
    /// — `dyn Printable`. Explicit runtime dispatch: only an annotated `dyn`
    /// binding admits the coercion (a concrete value into its asserted
    /// trait's object type); no downcast in v1. Execution lands with the
    /// thunk-table ABI (Phase 5b).
    Dyn(Box<Type>),
    /// 2026-08-22 (spec-conformance plan Phase 8, SPEC §12.2): a linear task
    /// handle — `spawn fn()` yields Task<R>; await unwraps R. Compiler
    /// construct (Ptr precedent): consumed exactly once by await/free/keep;
    /// dropping a live handle is an error.
    Task(Box<Type>),
    /// Pointer type: Ptr(T)
    Ptr(Box<Type>),
    /// Read-only pointer type: Ptr<const T>. Same size as Ptr(T), but the
    /// compiler rejects write-through (*p = val). Created by `&let_var`
    /// where the variable is immutable (local let-binding, not state field).
    /// 2026-07-18: Phase 3 — Const inference from context.
    PtrConst(Box<Type>),
    /// Function type: Function([Int, Int], Box::new(Int))
    Function(Vec<Type>, Box<Type>),
    /// Type-level width: Width(8)
    Width(u64),
    /// 2026-07-31 (A8): compile-time integer type argument — a SIZE parameter
    /// in a generic application (`Stack<Int, 8>`). Substitutes into a Named
    /// vector dimension (`T[N]` → `Int[8]`).
    Number(i64),
    /// Vector type with dimensions
    Vector(Box<Type>, Vec<Dimension>),
    /// Constrained type: Constrained(Bits(8), Range(0, 255))
    Constrained(Box<Type>, BitRange),
    /// Layout-constrained pointer
    LayoutPtr(LayoutConstraint),
    // 2026-09-11 (fundamentals doctrine, Phase A4): `Type::HashWord` and
    // `Type::HashWordVariant` are DELETED. The category hashwords (Float,
    // Int, …) are retired — the fundamental name is both the base type and
    // the protocol (`Float`, `Float<Posit>` = Applied(fundamental, [variant]),
    // `Float32: Float` plain parent). Out-of-scope hashwords (#System, #Link,
    // #Lh/#Rh/#T, field markers) never flowed through these variants.
}

// 2026-07-12: Named type factories. These create Custom() references that
// are resolved through the TypeUniverse at type-check time.
impl Type {
    /// Return the universe lookup key for this type.
    /// Custom("Float") → "Float", Applied("List", [T]) → "List".
    pub fn universe_key(&self) -> Option<&str> {
        match self {
            Type::Custom(name) => Some(name.as_str()),
            Type::Applied(name, _) => Some(name.as_str()),
            _ => None,
        }
    }

    pub fn int() -> Type {
        Type::Custom("Int".to_string())
    }
    pub fn float() -> Type {
        Type::Custom("Float".to_string())
    }
    pub fn float64() -> Type {
        Type::Custom("Float64".to_string())
    }
    pub fn bool_() -> Type {
        Type::Custom("Bool".to_string())
    }
    pub fn string() -> Type {
        Type::Custom("String".to_string())
    }
    pub fn char_() -> Type {
        Type::Custom("Char".to_string())
    }
    pub fn blob() -> Type {
        Type::Custom("Blob".to_string())
    }
    pub fn void() -> Type {
        Type::Void
    }
    /// Bits(n) with n BITS. 2026-08-13: unit restored to bits (byte-era ctor
    /// sites that meant bytes were migrated to `bits(n * 8)` at the same
    /// commit — search `layout-keywords` in history for the sweep list).
    pub fn bits(bits: u64) -> Type {
        Type::Bits(bits)
    }
    /// Create a Bits type from a bit count. 2026-08-13: identity (the old
    /// `div_ceil(bits, 8)` byte round is gone — sub-byte widths survive).
    pub fn from_bits(bits: u64) -> Type {
        Type::Bits(bits)
    }
    /// Return the bit width of this Bits type. Returns 0 for flexible-width Bits.
    pub fn bit_width(&self) -> u64 {
        match self {
            Type::Bits(bits) => *bits,
            _ => 0,
        }
    }
    pub fn ptr(inner: Type) -> Type {
        Type::Ptr(Box::new(inner))
    }
    pub fn ptr_const(inner: Type) -> Type {
        Type::PtrConst(Box::new(inner))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BitRange {
    Single(usize),
    Range(usize, usize),
    Any(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Dimension {
    Anonymous(usize),
    Named(String, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutConstraint {
    pub bytes: u64,
    pub alignment: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimeUnit {
    Cycles,
    Ms,
    Seconds,
    Minutes,
    Nanoseconds,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Formatting {
    /// Accepts "..." quoted literals
    Quoted,
    /// Accepts numeric literals (42, 3.14)
    Decimal,
    /// Accepts bare identifiers (Red, Green)
    Bare,
    /// Accepts all three forms
    Any,
    /// No literal syntax — constructor functions only
    None,
}

impl Formatting {
    pub fn from_name(name: &str) -> Option<Formatting> {
        match name {
            "Quoted" => Some(Formatting::Quoted),
            "Decimal" => Some(Formatting::Decimal),
            "Bare" => Some(Formatting::Bare),
            "Any" => Some(Formatting::Any),
            "None" => Some(Formatting::None),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Formatting::Quoted => "Quoted",
            Formatting::Decimal => "Decimal",
            Formatting::Bare => "Bare",
            Formatting::Any => "Any",
            Formatting::None => "None",
        }
    }
}

/// Value type for metadata properties in `<~` declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Identifier(String),
    List(Vec<PropertyValue>),
    /// 2026-09-23 (quantities plan): a first-class quantity spec value —
    /// `spec Decouple: 100n;` — stored in SI with its dimension. Bare
    /// quantities, never quoted strings (the doctrine: quotes imply
    /// arbitrary; quantities are physics).
    Quantity { si: f64, dimension: QuantityDim },
    // 2026-07-18: Compiler-internal hash words for strategy op bindings.
    // #L = left operand, #R = right operand, #T = type parameter.
    HashL,
    HashR,
    HashT,
}

/// The physics dimensions of a quantity spec value — compiler-intrinsic
/// (PascalCase doctrine), independent of the stdlib base type names
/// (Rule 15: no type-name matching in the compiler).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuantityDim {
    Volt,
    Amp,
    Ohm,
    Farad,
    Henry,
    Hertz,
    Watt,
    Kelvin,
}

impl QuantityDim {
    /// The lowercase canonical name (`volt`, `amp`, …) — used by the
    /// beast serialization tag and its reverse lookup.
    pub fn name(&self) -> &'static str {
        match self {
            QuantityDim::Volt => "volt",
            QuantityDim::Amp => "amp",
            QuantityDim::Ohm => "ohm",
            QuantityDim::Farad => "farad",
            QuantityDim::Henry => "henry",
            QuantityDim::Hertz => "hertz",
            QuantityDim::Watt => "watt",
            QuantityDim::Kelvin => "kelvin",
        }
    }
}

/// How an operator binding resolves to an implementation.
#[derive(Debug, Clone, PartialEq)]
pub enum OpBinding {
    /// Intrinsic function: AddI64#, Sqrt#, Malloc#
    Intrinsic(String),
    /// User-defined Briev function: my_add
    Function(String),
}

/// How a type is defined in the type universe.
#[derive(Debug, Clone)]
pub enum TypeKind {
    Struct(Vec<Field>),
    Enum(Vec<Variant>),
    Codec {
        formatting: Formatting,
        parse: Option<String>,
    },
    Alias(Box<Type>),
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub metadata: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
    pub metadata: HashMap<String, PropertyValue>,
}

/// The constraint language for contract pre/post conditions.
/// Used by the SMT proof engine. Mirrors the Bool-valued Expr subset.
#[derive(Debug, Clone)]
pub enum Constraint {
    Eq(Box<Constraint>, Box<Constraint>),
    Neq(Box<Constraint>, Box<Constraint>),
    Lt(Box<Constraint>, Box<Constraint>),
    Gt(Box<Constraint>, Box<Constraint>),
    Le(Box<Constraint>, Box<Constraint>),
    Ge(Box<Constraint>, Box<Constraint>),
    And(Box<Constraint>, Box<Constraint>),
    Or(Box<Constraint>, Box<Constraint>),
    Not(Box<Constraint>),
    Implies(Box<Constraint>, Box<Constraint>),
    Forall(String, Box<Constraint>),
    Exists(String, Box<Constraint>),
    Literal(Box<crate::ast::Expr>),
    Bool(bool),
}
