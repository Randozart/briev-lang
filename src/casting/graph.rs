use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::top::{CastDirection, ProtocolDef};
use crate::ast::Type;
use crate::ast::PropertyValue;
use crate::type_universe::TypeUniverse;

// ── Lane Kinds ──────────────────────────────────────────────────────────

// ── Lane Kinds ──────────────────────────────────────────────────────────

/// The kind of transformation a single cast step performs.
/// Each variant maps to a specific LLVM IR instruction or call pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum LaneKind {
    /// LLVM bitcast: src_ty to dst_ty (same-width reinterpretation)
    Bitcast,
    /// Signed integer to float: sitofp i64 %v to double
    IntToFloat,
    /// Float to signed integer: fptosi double %v to i64
    FloatToInt,
    /// Call an external/intrinsic conversion function: call @fn_name
    ExtCall(&'static str),
    /// Call a proto-binding transform function (owned name — user-declared
    /// `proto C_String: String { CastTo(...) = cstr_to_briev(#L); }`).
    /// 2026-08-03: distinct from ExtCall so seeded base lanes keep their
    /// `&'static str` without changing all call sites.
    ExtCallDyn(String),
    /// Extract first field of a struct: extractvalue {i64,i64} %v, 0
    ExtractData,
    /// Pointer to integer: ptrtoint ptr %v to i64
    PtrToInt,
    /// Integer to pointer: inttoptr i64 %v to ptr
    IntToPtr,
    /// Zero-extend: zext i8 %v to i64
    ZExt,
    /// Truncate: trunc i64 %v to i8 (or i64 to i32, etc.)
    Trunc,
    /// Type-level CastFrom(Bit) override — function name resolved at emission time
    CastFromBitCallback,
    /// 2026-08-03: the Float protocol's width cast — any Float variant casts
    /// to any other Float variant by fpext/fptrunc (width is the only delta).
    FloatWidth,
    /// Composite: chain two consecutive lanes
    Chain(Box<LaneKind>, Box<LaneKind>),
}

// ── Cast Step ───────────────────────────────────────────────────────────

/// A single resolved step in a protocol-to-protocol cast path.
#[derive(Debug, Clone, PartialEq)]
pub struct CastStep {
    /// The lane to traverse
    pub lane: LaneKind,
    /// Source protocol category name (e.g., "Int", "String")
    pub src_category: String,
    /// Source protocol variant (empty for base protocols)
    pub src_variant: String,
    /// Destination protocol category
    pub dst_category: String,
    /// Destination protocol variant
    pub dst_variant: String,
}

// ── LLVM Type Resolver ──────────────────────────────────────────────────

/// How a protocol category + variant maps to an LLVM type string.
#[derive(Debug, Clone, PartialEq)]
pub enum LlvmTypeResolver {
    /// Fixed LLVM type string (e.g., "double", "ptr", "{ i64, i64 }")
    Fixed(&'static str),
    /// Width-parametric: !> bits → !> maxbits → !> minbits → int_bits
    WidthParametric,
    /// 2026-08-03: the Float protocol's width semantics — derive the LLVM
    /// type from the type's `bits` metadata (16 → half/bfloat via disamb,
    /// 32 → float, 64 → double, 80 → x86_fp80, 128 → fp128, default → float).
    /// The protocol owns the width; no type names are hardcoded.
    FloatWidth,
}

// ── SPIR-V Type Resolver ────────────────────────────────────────────────

/// 2026-08-26 (plan 2026-08-23-spirv-kernel-emission §2.4): how a protocol
/// category maps to a SPIR-V scalar type. SPIR-V differs semantically from
/// LLVM — Bool is its own OpTypeBool (not an i8), Int carries an explicit
/// SIGNEDNESS operand, and String/Blob pointers do not exist in a kernel —
/// so the kernel backend gets its own resolver table instead of deriving
/// from the LLVM strings.
#[derive(Debug, Clone, PartialEq)]
pub enum SpirvTypeResolver {
    /// OpTypeInt with width from metadata; bool = signedness operand.
    /// Briev `Int` is signed; `UInt` is unsigned.
    IntWidth(bool),
    /// OpTypeFloat with width from metadata (bits property).
    FloatWidth,
    /// Bool is the dedicated OpTypeBool scalar.
    Fixed(SpirvScalar),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpirvScalar {
    Bool,
}

/// A fully-resolved SPIR-V scalar shape: category + width after the universe
/// metadata ladder. No type names survive this point (rule 19).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpirvShape {
    Int { bits: u32, signed: bool },
    Float { bits: u32 },
    Bool,
}

// ── Casting Graph ───────────────────────────────────────────────────────

/// Protocol-to-protocol casting graph.
///
/// Every base protocol has a hardcoded direct lane to every other base
/// protocol (64 entries). Variant edges from `proto` declarations add
/// additional edges for sub-protocols. BFS resolves variant→variant
/// and variant→base paths through the union of base lanes and variant edges.
///
/// `CastTo(Bit)` is banned at declaration time — the `→ Bit` direction
/// is always a hardcoded mechanical operation (bitcast/extractvalue/ptrtoint).
/// `CastFrom(Bit)` is the sole user-extensible edge direction.
#[derive(Debug, Clone)]
pub struct CastingGraph {
    /// Base protocol → base protocol direct lanes.
    /// Indexed by (src_category, dst_category) where both are base protocol
    /// category names (e.g., ("Int", "Float")).
    base_lanes: HashMap<String, HashMap<String, LaneKind>>,

    /// Per-variant CastTo edges from proto declarations.
    /// Indexed by (category, variant_name).
    variant_edges: HashMap<(String, String), Vec<CastStep>>,

    /// Per-variant reverse edges (from CastFrom declarations).
    variant_reverse: HashMap<(String, String), Vec<CastStep>>,

    /// Default variant per category (e.g., String→UTF8, Float→IEEE754, Char→unicode).
    defaults: HashMap<String, String>,

    /// Type-level CastFrom(Bit) overrides: type_name → function_name.
    cast_from_bit_overrides: HashMap<String, String>,

    /// Protocol (category, variant) → LLVM type resolver.
    /// Used by resolve_llvm_type() to derive LLVM types from protocol + metadata.
    protocol_llvm_types: HashMap<(String, String), LlvmTypeResolver>,

    /// 2026-08-26 (§2.4): protocol (category, variant) → SPIR-V scalar
    /// resolver. Same design as protocol_llvm_types but SPIR-V-native
    /// semantics (Bool = OpTypeBool, Int = signedness operand); used by
    /// resolve_spirv_shape().
    protocol_spirv_types: HashMap<(String, String), SpirvTypeResolver>,

    /// 2026-08-03 (P1.5): proven-inverse variant pairs (category, a, b) —
    /// `b.CastFrom(base)(a.CastTo(base)(x)) == x` was proved symbolically/SMT,
    /// so a cast a → b through the base is a ZERO delta (identity). The
    /// `<<1`/`>>1` example: two sub-types whose encode/decode cancel are 1-to-1.
    inverse_pairs: HashSet<(String, String, String)>,

    /// 2026-08-03 (P1.4): cross-variant op overrides from `proto` declarations
    /// (`proto C_String: String { op Concat(String) = cstring_concat(#L,#R) }`).
    /// (category, variant) → op name → binding fn. An op on a sub-protocol
    /// value prefers its variant's own op (zero cast) — "adopt whatever
    /// operations are most convenient."
    variant_cross_ops: HashMap<(String, String), HashMap<String, String>>,
}

impl CastingGraph {
    /// Create a new casting graph seeded with all base protocol lanes.
    pub fn new() -> Self {
        let mut graph = CastingGraph {
            base_lanes: HashMap::new(),
            variant_edges: HashMap::new(),
            variant_reverse: HashMap::new(),
            defaults: HashMap::new(),
            cast_from_bit_overrides: HashMap::new(),
            protocol_llvm_types: HashMap::new(),
            protocol_spirv_types: HashMap::new(),
            inverse_pairs: HashSet::new(),
            variant_cross_ops: HashMap::new(),
        };
        graph.seed_base_lanes();
        graph.seed_defaults();
        graph.seed_protocol_llvm_types();
        // 2026-08-26 (§2.4): kernel-surface SPIR-V scalars. Deliberately NOT
        // registered: String/Blob/Char/Data — a compute kernel has no heap,
        // no strings, no opaque pointers; resolving them errors naming the fix.
        graph.set_spirv_type("Int", "", SpirvTypeResolver::IntWidth(true));
        graph.set_spirv_type("UInt", "", SpirvTypeResolver::IntWidth(false));
        graph.set_spirv_type("Float", "", SpirvTypeResolver::FloatWidth);
        graph.set_spirv_type("Bool", "", SpirvTypeResolver::Fixed(SpirvScalar::Bool));
        graph
    }

    /// Seed all 64 base protocol → base protocol lanes.
    fn seed_base_lanes(&mut self) {
        // All 8 base protocol categories.
        // "Data" is the root — every other protocol has a direct lane to/from Data.
        // 2026-08-15 (fundamentals): Data (raw storage, the universal parent)
        // replaces Bit as the graph root. Bit<N> remains the bit type; its
        // treat-as-bits membership (Cast.Bit) is distinct from the Cast.Data
        // storage membership every type carries.
        //
        // Convention: we populate both directions for clarity. The graph is
        // symmetric: (A,B) means A→B lane, (B,A) means B→A lane.

        // ── Data ⇄ Int ─────────────────────────────────────────────
        self.set_lane("Data", "Int", LaneKind::Bitcast);
        self.set_lane("Int", "Data", LaneKind::Bitcast);
        // ── Data ⇄ UInt ────────────────────────────────────────────
        self.set_lane("Data", "UInt", LaneKind::Bitcast);
        self.set_lane("UInt", "Data", LaneKind::Bitcast);
        // ── Data ⇄ Float ───────────────────────────────────────────
        self.set_lane("Data", "Float", LaneKind::Bitcast);
        self.set_lane("Float", "Data", LaneKind::Bitcast);
        // ── Data ⇄ String ──────────────────────────────────────────
        // 2026-08-01 (B2): Data→String is the ENCODING DOOR — a CastFrom(Data)
        // callback (UTF8 wrap default, materializing the [len][bytes] header by
        // construction; sub-protocols override via register_cast_from_data). It
        // is NOT a bitcast: wrapping must produce a header-prefixed buffer.
        self.set_lane("Data", "String", LaneKind::CastFromBitCallback);
        // String→Data: the CONTENT VIEW — a String value IS a ptr to
        // [len][bytes], so the cast yields the buffer address (ptrtoint, one
        // instruction). Zero-length/identity at the register level; never
        // overridable (the representation is compiler-guaranteed).
        self.set_lane("String", "Data", LaneKind::PtrToInt);
        // ── Data ⇄ Bool ────────────────────────────────────────────
        self.set_lane("Data", "Bool", LaneKind::Trunc);    // i64 → i8
        self.set_lane("Bool", "Data", LaneKind::ZExt);     // i8 → i64
        // ── Data ⇄ Char ────────────────────────────────────────────
        self.set_lane("Data", "Char", LaneKind::Trunc);    // i64 → i32
        self.set_lane("Char", "Data", LaneKind::ZExt);     // i32 → i64
        // ── Data ⇄ Blob ────────────────────────────────────────────
        self.set_lane("Data", "Blob", LaneKind::IntToPtr); // i64 → ptr
        self.set_lane("Blob", "Data", LaneKind::PtrToInt); // ptr → i64
        // ── Data ⇄ Bit ─────────────────────────────────────────────
        // Bit<N> is the bit type (a Data member). The treat-as-bits view is
        // raw storage reinterpretation.
        self.set_lane("Data", "Bit", LaneKind::Bitcast);
        self.set_lane("Bit", "Data", LaneKind::Bitcast);
        // 2026-08-28 (boundary handle invariant): Data — the universal
        // parent — IS the i64 handle at any boundary; Int ↔ Data is a
        // representation identity (Bitcast), giving pointer-representation
        // variants a repr-only route to Int (CStr → Int = ptrtoint+bitcast,
        // never the base's content parse).
        self.set_lane("Int", "Data", LaneKind::Bitcast);
        self.set_lane("Data", "Int", LaneKind::Bitcast);

        // ── Int ⇄ UInt ────────────────────────────────────────────
        self.set_lane("Int", "UInt", LaneKind::Bitcast); // same representation
        self.set_lane("UInt", "Int", LaneKind::Bitcast);
        // ── Int ⇄ Float ───────────────────────────────────────────
        self.set_lane("Int", "Float", LaneKind::IntToFloat);
        self.set_lane("Float", "Int", LaneKind::FloatToInt);
        // ── Int ⇄ String ──────────────────────────────────────────
        self.set_lane("Int", "String", LaneKind::ExtCall("int_to_str"));
        self.set_lane("String", "Int", LaneKind::ExtCall("str_to_int"));
        // ── Int ⇄ Bool ────────────────────────────────────────────
        self.set_lane("Int", "Bool", LaneKind::Trunc);   // i64 → i8
        self.set_lane("Bool", "Int", LaneKind::ZExt);    // i8 → i64
        // ── Int ⇄ Char ────────────────────────────────────────────
        self.set_lane("Int", "Char", LaneKind::Trunc);   // i64 → i32
        self.set_lane("Char", "Int", LaneKind::ZExt);    // i32 → i64
        // ── Int ⇄ Data ────────────────────────────────────────────
        self.set_lane("Int", "Blob", LaneKind::IntToPtr); // i64 → ptr
        self.set_lane("Blob", "Int", LaneKind::PtrToInt); // ptr → i64

        // ── UInt ⇄ Float ───────────────────────────────────────────
        self.set_lane("UInt", "Float", LaneKind::IntToFloat);
        self.set_lane("Float", "UInt", LaneKind::FloatToInt);
        // ── UInt ⇄ String ─────────────────────────────────────────
        self.set_lane("UInt", "String", LaneKind::ExtCall("uint_to_str"));
        self.set_lane("String", "UInt", LaneKind::ExtCall("str_to_uint"));
        // ── UInt ⇄ Bool ───────────────────────────────────────────
        self.set_lane("UInt", "Bool", LaneKind::Trunc);
        self.set_lane("Bool", "UInt", LaneKind::ZExt);
        // ── UInt ⇄ Char ───────────────────────────────────────────
        self.set_lane("UInt", "Char", LaneKind::Trunc);
        self.set_lane("Char", "UInt", LaneKind::ZExt);
        // ── UInt ⇄ Data ───────────────────────────────────────────
        self.set_lane("UInt", "Blob", LaneKind::IntToPtr);
        self.set_lane("Blob", "UInt", LaneKind::PtrToInt);

        // ── Float ⇄ String ────────────────────────────────────────
        self.set_lane("Float", "String", LaneKind::ExtCall("float_to_str"));
        self.set_lane("String", "Float", LaneKind::ExtCall("str_to_float"));
        // ── Float ⇄ Bool ──────────────────────────────────────────
        // Float→Bool: fptosi i64 + trunc to i8 (chain)
        self.set_lane("Float", "Bool", LaneKind::Chain(
            Box::new(LaneKind::FloatToInt),
            Box::new(LaneKind::Trunc),
        ));
        self.set_lane("Bool", "Float", LaneKind::Chain(
            Box::new(LaneKind::ZExt),
            Box::new(LaneKind::IntToFloat),
        ));
        // ── Float ⇄ Char ──────────────────────────────────────────
        self.set_lane("Float", "Char", LaneKind::FloatToInt);
        self.set_lane("Char", "Float", LaneKind::IntToFloat);
        // ── Float ⇄ Data ──────────────────────────────────────────
        self.set_lane("Float", "Blob", LaneKind::Chain(
            Box::new(LaneKind::FloatToInt),
            Box::new(LaneKind::IntToPtr),
        ));
        self.set_lane("Blob", "Float", LaneKind::Chain(
            Box::new(LaneKind::PtrToInt),
            Box::new(LaneKind::IntToFloat),
        ));

        // ── String ⇄ Bool ─────────────────────────────────────────
        self.set_lane("String", "Bool", LaneKind::ExtCall("str_to_bool"));
        self.set_lane("Bool", "String", LaneKind::ExtCall("bool_to_str"));
        // ── String ⇄ Char ─────────────────────────────────────────
        self.set_lane("String", "Char", LaneKind::ExtCall("str_first_char"));
        self.set_lane("Char", "String", LaneKind::ExtCall("char_to_str"));
        // ── String ⇄ Data ─────────────────────────────────────────
        self.set_lane("String", "Blob", LaneKind::Chain(
            Box::new(LaneKind::ExtractData),
            Box::new(LaneKind::IntToPtr),
        ));
        self.set_lane("Blob", "String", LaneKind::Chain(
            Box::new(LaneKind::PtrToInt),
            Box::new(LaneKind::Bitcast), // bitcast i64 to {i64,i64}
        ));

        // ── Bool ⇄ Char ──────────────────────────────────────────
        self.set_lane("Bool", "Char", LaneKind::ZExt);
        self.set_lane("Char", "Bool", LaneKind::Trunc);
        // ── Bool ⇄ Data ──────────────────────────────────────────
        self.set_lane("Bool", "Blob", LaneKind::Chain(
            Box::new(LaneKind::ZExt),
            Box::new(LaneKind::IntToPtr),
        ));
        self.set_lane("Blob", "Bool", LaneKind::Chain(
            Box::new(LaneKind::PtrToInt),
            Box::new(LaneKind::Trunc),
        ));

        // ── Char ⇄ Data ──────────────────────────────────────────
        self.set_lane("Char", "Blob", LaneKind::Chain(
            Box::new(LaneKind::ZExt),
            Box::new(LaneKind::IntToPtr),
        ));
        self.set_lane("Blob", "Char", LaneKind::Chain(
            Box::new(LaneKind::PtrToInt),
            Box::new(LaneKind::Trunc),
        ));
    }

    /// Seed default variant names per category.
    fn seed_defaults(&mut self) {
        self.defaults.insert("String".to_string(), "UTF8".to_string());
        self.defaults.insert("Float".to_string(), "IEEE754".to_string());
        self.defaults.insert("Char".to_string(), "unicode".to_string());
    }

    /// Seed hardcoded protocol (category, variant) → LLVM type mappings.
    /// 2026-07-30: Replaces the normalizer's three-phase llvm_type derivation
    /// and the disamb metadata hack. Protocol variants are first-class graph nodes.
    fn seed_protocol_llvm_types(&mut self) {
        // Base protocols
        self.set_llvm_type("Bit", "",    LlvmTypeResolver::WidthParametric);
        self.set_llvm_type("Int", "",    LlvmTypeResolver::WidthParametric);
        self.set_llvm_type("UInt", "",   LlvmTypeResolver::WidthParametric);
        self.set_llvm_type("Float", "",  LlvmTypeResolver::FloatWidth);
        self.set_llvm_type("Bool", "",   LlvmTypeResolver::Fixed("i8"));
        self.set_llvm_type("Char", "",   LlvmTypeResolver::Fixed("i32"));
        // 2026-08-01 (B0): A Briev String value IS a pointer to a
        // length-prefixed [len: i64][bytes] buffer, in every type-claiming
        // site. The old { i64, i64 } fat-pointer claim caused the 4-way
        // representation split-brain (ptr vs i64 vs {i64,i64} vs i128). The
        // casting graph is the single source of truth, so it now says ptr.
        self.set_llvm_type("String", "", LlvmTypeResolver::Fixed("ptr"));
        self.set_llvm_type("Blob", "",   LlvmTypeResolver::Fixed("ptr"));

        // Float protocol variants (hardcoded — no disamb hack)
        self.set_llvm_type("Float", "IEEE754",  LlvmTypeResolver::Fixed("float"));
        self.set_llvm_type("Float", "Half",     LlvmTypeResolver::Fixed("half"));
        self.set_llvm_type("Float", "BFloat",   LlvmTypeResolver::Fixed("bfloat"));
        self.set_llvm_type("Float", "Double",   LlvmTypeResolver::Fixed("double"));
        self.set_llvm_type("Float", "FP128",    LlvmTypeResolver::Fixed("fp128"));
        self.set_llvm_type("Float", "X86_FP80", LlvmTypeResolver::Fixed("x86_fp80"));
        // 2026-08-03: C-boundary Float widths (FFI). Float<C_Float> is the
        // C `float` (32-bit), Float<C_Double> the C `double` (64-bit) — the
        // boundary types in lib/glue/c.bv declare these variants so an export
        // can request the exact ABI width instead of the default float32.
        self.set_llvm_type("Float", "C_Float",  LlvmTypeResolver::Fixed("float"));
        self.set_llvm_type("Float", "C_Double", LlvmTypeResolver::Fixed("double"));

        // String protocol variants — all encode as ptr to [len][bytes] (B0).
        // The default String variant is UTF8 (seed_defaults); ASCII and any
        // future sub-protocols keep the same pointer representation.
        self.set_llvm_type("String", "UTF8",  LlvmTypeResolver::Fixed("ptr"));
        self.set_llvm_type("String", "ASCII", LlvmTypeResolver::Fixed("ptr"));
    }

    /// Insert a protocol (category, variant) → LLVM type resolver entry.
    fn set_llvm_type(&mut self, category: &'static str, variant: &'static str, resolver: LlvmTypeResolver) {
        self.protocol_llvm_types.insert((category.to_string(), variant.to_string()), resolver);
    }

    /// Get the LLVM type resolver for a (category, variant) pair.
    pub fn get_llvm_type(&self, category: &str, variant: &str) -> Option<&LlvmTypeResolver> {
        self.protocol_llvm_types.get(&(category.to_string(), variant.to_string()))
    }

    /// Insert a base lane between two protocol categories.
    fn set_lane(&mut self, src: &'static str, dst: &'static str, lane: LaneKind) {
        self.base_lanes
            .entry(src.to_string())
            .or_default()
            .insert(dst.to_string(), lane);
    }

    /// Get the lane from src_category to dst_category, if one exists.
    pub fn get_lane(&self, src_category: &str, dst_category: &str) -> Option<&LaneKind> {
        self.base_lanes
            .get(src_category)
            .and_then(|inner| inner.get(dst_category))
    }

    /// Get the default variant for a category (empty string if none).
    pub fn default_variant(&self, category: &str) -> &str {
        self.defaults.get(category).map(|s| s.as_str()).unwrap_or("")
    }

    // ── Proto Declaration Registration ────────────────────────────────

    /// Register a ProtocolDef item (proto declaration) into the graph.
    /// Adds variant edges, reverse edges, and cross-variant op overrides.
    pub fn register_protocol_def(&mut self, pd: &ProtocolDef) {
        let key = (pd.category.clone(), pd.name.clone());

        for edge in &pd.cast_edges {
            // 2026-08-03: a CastBinding is the delta transform. CastTo binds
            // proto → target (e.g. cstr_to_briev); CastFrom binds target →
            // proto (e.g. str_to_c). Each edge gets the binding's function as
            // its lane so emit_cast_steps emits a real call, not a bitcast.
            let lane = match &edge.binding {
                Some(b) => LaneKind::ExtCallDyn(b.fn_name.clone()),
                None => LaneKind::Bitcast, // placeholder for unbound edges
            };
            // Forward edge (proto → target). For a CastFrom-only declaration
            // there is no proto→target transform, so skip the forward edge
            // (the reverse edge below carries the CastFrom binding).
            if edge.direction == CastDirection::CastTo {
                let step = CastStep {
                    lane: lane.clone(),
                    src_category: key.0.clone(),
                    src_variant: key.1.clone(),
                    dst_category: edge.target_category.clone(),
                    dst_variant: edge.target_variant.clone(),
                };
                self.variant_edges.entry(key.clone()).or_default().push(step);
            }

            // Reverse edge from CastFrom: the CastFrom binding converts
            // target → proto (str_to_c: UTF8 → C_String). Stored under the
            // TARGET key; BFS from the target follows it to the proto. The
            // step's src is the PROTO (the neighbor reached), dst the target
            // (the current node) — the lane still transforms current→neighbor.
            if edge.direction == CastDirection::CastFrom {
                let rev_step = CastStep {
                    lane,
                    src_category: key.0.clone(),
                    src_variant: key.1.clone(),
                    dst_category: edge.target_category.clone(),
                    dst_variant: edge.target_variant.clone(),
                };
                self.variant_reverse
                    .entry((edge.target_category.clone(), edge.target_variant.clone()))
                    .or_default()
                    .push(rev_step);
            }
        }

        // 2026-08-03 (P1.4): cross-variant op overrides — `op Concat(String) =
        // cstring_concat(#L, #R)` lets a sub-protocol value use its own
        // operation (zero cast) instead of casting to the base first.
        for op in &pd.cross_ops {
            let Some(fn_name) = Self::cross_op_fn(&op.impl_args) else { continue };
            self.variant_cross_ops
                .entry((pd.category.clone(), pd.name.clone()))
                .or_default()
                .insert(op.op.clone(), fn_name);
        }

        // 2026-08-28 (boundary handle invariant): every variant of a
        // pointer-representation category IS a pointer — Int → variant is
        // inttoptr, regardless of the content encoding (the content delta
        // lives in the CastTo/CastFrom edges to the base). The edge hangs
        // off DATA (the handle parent) so the repr-only BFS route
        // Int → Data → variant never competes with the base's content
        // lanes (int_to_str formatting the handle as decimal digits).
        // NON-DEFAULT variants only: the default variant is the base's own
        // representation — a raw handle is NOT automatically a base value
        // (that admission is the content delta's job).
        // Category-keyed, not type-name-keyed (rule 18).
        if matches!(pd.category.as_str(), "String" | "Blob")
            && self
                .defaults
                .get(&pd.category)
                .map(|d| d.as_str() != pd.name.as_str())
                .unwrap_or(true)
        {
            let step = CastStep {
                lane: LaneKind::IntToPtr,
                src_category: "Data".to_string(),
                src_variant: String::new(),
                dst_category: pd.category.clone(),
                dst_variant: pd.name.clone(),
            };
            self.variant_edges
                .entry(("Data".to_string(), String::new()))
                .or_default()
                .push(step);
        }
    }

    /// Look up a cross-variant op override: (category, variant, op name) →
    /// binding fn. `C_String`/`Concat` → `cstring_concat`.
    pub fn get_variant_op(&self, category: &str, variant: &str, op_name: &str) -> Option<&str> {
        self.variant_cross_ops
            .get(&(category.to_string(), variant.to_string()))
            .and_then(|ops| ops.get(op_name))
            .map(|s| s.as_str())
    }

    // ── Type-Level CastFrom(Bit) Override Registration ────────────────
    /// Register a type-level CastFrom(Bit) override.
    /// `type_name` → `function_name` for constructing the type from raw bits.
    pub fn register_cast_from_bit(&mut self, type_name: &str, function_name: &str) {
        self.cast_from_bit_overrides
            .insert(type_name.to_string(), function_name.to_string());
    }

    /// Check if a type has a CastFrom(Bit) override.
    pub fn get_cast_from_bit(&self, type_name: &str) -> Option<&str> {
        self.cast_from_bit_overrides.get(type_name).map(|s| s.as_str())
    }

    // ── Inverse-Pair Registration (P1.5) ───────────────────────────────

    /// Register a proven-inverse variant pair (category, a, b): a cast
    /// a → b through the base is identity (the delta is nothing).
    pub fn register_inverse_pair(&mut self, category: &str, a: &str, b: &str) {
        self.inverse_pairs.insert((category.to_string(), a.to_string(), b.to_string()));
    }

    /// Compute and register all proven-inverse pairs among the program's
    /// `proto` declarations (cross-type round-trip proof, protocol_graph.rs).
    pub fn register_inverse_pairs_from(&mut self, items: &[crate::ast::TopLevel]) {
        for (cat, a, b) in crate::analysis::protocol_graph::find_inverse_pairs(items) {
            self.register_inverse_pair(&cat, &a, &b);
        }
    }

    /// Whether (category, a, b) is a proven inverse pair (a → b is zero-cost).
    pub fn is_inverse_pair(&self, category: &str, a: &str, b: &str) -> bool {
        self.inverse_pairs.contains(&(category.to_string(), a.to_string(), b.to_string()))
    }

    /// 2026-08-03 (P1.4): extract the binding function name from a cross-op's
    /// `impl_args` (`= cstring_concat(#L, #R)` → "cstring_concat"). Accepts a
    /// bare identifier or a call list whose first element is the function name.
    fn cross_op_fn(impl_args: &Option<PropertyValue>) -> Option<String> {
        match impl_args {
            Some(PropertyValue::Identifier(n)) => Some(n.clone()),
            Some(PropertyValue::List(items)) => match items.first() {
                Some(PropertyValue::Identifier(n)) => Some(n.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    // ── Path Resolution ────────────────────────────────────────────────

    /// Find a protocol cast path from (src_cat, src_var) to (dst_cat, dst_var).
    ///
    /// Returns the sequence of CastSteps if a path exists. For base→base
    /// (no variants), this is O(1) — direct lane lookup. For variant→variant
    /// or variant→base, BFS through variant edges + default fallbacks.
    pub fn find_path(
        &self,
        src_cat: &str,
        src_var: &str,
        dst_cat: &str,
        dst_var: &str,
    ) -> Option<Vec<CastStep>> {
        // 2026-08-28 (Bug #5): the BASE of a category IS its default variant
        // — `String` (no variant) and `String<UTF8>` are the same
        // representation. Normalize the empty variant to the category default
        // so BFS can land on it (a variant edge targets `String<UTF8>`, and
        // without normalization `CStr as String` never reached the goal and
        // fell through to a raw bitcast — the C string was read as a [len]
        // block). Only normalize when a variant is actually involved — the
        // base→base fast path must keep firing for lane lookup (Bit → String
        // etc.), and only normalize when the category HAS a declared default.
        let has_variant = !src_var.is_empty() || !dst_var.is_empty();
        let (src_var, dst_var) = if has_variant {
            (
                if src_var.is_empty() { self.defaults.get(src_cat).map(|s| s.as_str()).unwrap_or(src_var) } else { src_var },
                if dst_var.is_empty() { self.defaults.get(dst_cat).map(|s| s.as_str()).unwrap_or(dst_var) } else { dst_var },
            )
        } else {
            (src_var, dst_var)
        };
        // Fast path: both are base protocols with no variants
        if src_var.is_empty() && dst_var.is_empty() {
            return self.find_base_path(src_cat, dst_cat);
        }

        // 2026-08-03: the Float protocol — any Float variant casts to any
        // other Float variant by a width cast (fpext/fptrunc); the delta is
        // the width, never a representation chain. Handles Float → CDouble
        // (Float<C_Double>), which the variant BFS has no lane for.
        if src_cat == dst_cat && src_cat == "Float" {
            return Some(vec![CastStep {
                lane: LaneKind::FloatWidth,
                src_category: src_cat.to_string(),
                src_variant: src_var.to_string(),
                dst_category: dst_cat.to_string(),
                dst_variant: dst_var.to_string(),
            }]);
        }

        // 2026-08-03 (P1.5): delta collapse — a same-category variant→variant
        // whose endpoints are a PROVEN inverse pair (b.CastFrom(base)(a.
        // CastTo(base)(x)) == x) is a zero delta: the sub-types are 1-to-1.
        // 2026-08-28: checked BEFORE the repr phase — the pair being 1-to-1
        // means identity beats both the content route and the handle route.
        if src_cat == dst_cat && self.is_inverse_pair(src_cat, src_var, dst_var) {
            return Some(vec![]);
        }

        // BFS through variant edges + base lanes
        let path = self.bfs_path(src_cat, src_var, dst_cat, dst_var)?;

        // 2026-08-03 (P1.5): delta collapse — a same-category two-hop
        // `variant → base → variant` whose endpoints are a PROVEN inverse
        // pair (b.CastFrom(base)(a.CastTo(base)(x)) == x, e.g. `<<1`/`>>1`)
        // is a zero delta: emit nothing, the sub-types are 1-to-1.
        // The first hop is a forward edge (src = start variant), the second
        // a reverse edge (src = end variant); both meet at the base.
        if path.len() == 2
            && path[0].src_category == path[0].dst_category
            && path[1].src_category == path[1].dst_category
            && path[0].dst_variant == path[1].dst_variant
            && path[0].src_variant != path[1].src_variant
            && self.is_inverse_pair(&path[0].src_category, &path[0].src_variant, &path[1].src_variant)
        {
            return Some(vec![]);
        }

        Some(path)
    }

    /// O(1) direct lane lookup between two base protocol categories.
    fn find_base_path(&self, src_cat: &str, dst_cat: &str) -> Option<Vec<CastStep>> {
        if src_cat == dst_cat {
            return Some(vec![]); // identity
        }
        if let Some(lane) = self.get_lane(src_cat, dst_cat) {
            return Some(vec![CastStep {
                lane: lane.clone(),
                src_category: src_cat.to_string(),
                src_variant: String::new(),
                dst_category: dst_cat.to_string(),
                dst_variant: String::new(),
            }]);
        }
        // 2026-08-15 (fundamentals): Data is the universal root — a pair with
        // no direct lane routes through Data (src → Data → dst). Every base
        // protocol has a direct lane to/from Data, so the two-hop path always
        // exists. This is what makes Data the hub without naming it at every
        // cross-pair.
        if src_cat != "Data" && dst_cat != "Data" {
            if let (Some(l1), Some(l2)) = (self.get_lane(src_cat, "Data"), self.get_lane("Data", dst_cat)) {
                return Some(vec![
                    CastStep {
                        lane: l1.clone(),
                        src_category: src_cat.to_string(),
                        src_variant: String::new(),
                        dst_category: "Data".to_string(),
                        dst_variant: String::new(),
                    },
                    CastStep {
                        lane: l2.clone(),
                        src_category: "Data".to_string(),
                        src_variant: String::new(),
                        dst_category: dst_cat.to_string(),
                        dst_variant: String::new(),
                    },
                ]);
            }
        }
        None
    }

    /// BFS through variant edges + base lane fallback for the last hop.
    /// 2026-08-28: nodes are CANONICALIZED (an empty variant becomes the
    /// category default when one is declared) so base-lane hops, variant
    /// edges, and reverse edges meet at the same node key. Representation-
    /// level lanes (ptrtoint/inttoptr/bitcast/trunc/zext) fire from ANY
    /// variant of the category — the bits-model handle invariant: a value's
    /// pointer IS the handle regardless of the content encoding behind it.
    /// CONTENT lanes (ExtCall/CastFromBitCallback/…) stay gated to the
    /// base/default — a CStr's NUL-terminated bytes must not be parsed by
    /// the base's str_to_int (the `cstr as Int` garbage-length bug).
    fn bfs_path(
        &self,
        src_cat: &str,
        src_var: &str,
        dst_cat: &str,
        dst_var: &str,
    ) -> Option<Vec<CastStep>> {
        fn is_repr_lane(lane: &LaneKind) -> bool {
            matches!(
                lane,
                LaneKind::PtrToInt
                    | LaneKind::IntToPtr
                    | LaneKind::Bitcast
                    | LaneKind::Trunc
                    | LaneKind::ZExt
            )
        }
        let canon = |cat: &str, var: &str| -> (String, String) {
            if var.is_empty() {
                match self.defaults.get(cat) {
                    Some(d) => (cat.to_string(), d.clone()),
                    None => (cat.to_string(), String::new()),
                }
            } else {
                (cat.to_string(), var.to_string())
            }
        };
        let start = canon(src_cat, src_var);
        let target = canon(dst_cat, dst_var);

        // 2026-08-28 (boundary handle invariant), phase gate: repr-only
        // routes apply when the SOURCE is already the boundary handle — a
        // non-default variant (CStr's pointer IS the raw C pointer) or a
        // handle category casting INTO a variant (the C caller hands us a
        // C pointer as an i64). Content lanes are excluded there: formatting
        // the handle as decimal digits (int_to_str) and re-wrapping is
        // semantic garbage at a boundary. Everything else takes the full
        // BFS (base↔variant deltas, the base's content lanes).
        let src_at_default = self
            .defaults
            .get(&start.0)
            .map(|d| d == &start.1)
            .unwrap_or(start.1.is_empty());
        let src_is_variant = !start.1.is_empty() && !src_at_default;
        let src_is_handle = matches!(start.0.as_str(), "Int" | "UInt" | "Data" | "Bit");
        let dst_at_default = self
            .defaults
            .get(&target.0)
            .map(|d| d == &target.1)
            .unwrap_or(target.1.is_empty());
        let dst_is_variant = !target.1.is_empty() && !dst_at_default;
        // Boundary crossings only: a variant's pointer IS the raw handle
        // (admission OUT), and a handle IS a variant's pointer (admission
        // IN). Same-category variant→variant stays in phase 2 — the
        // declared content deltas decide, never a raw handle round-trip.
        if (src_is_variant && start.0 != target.0) || (src_is_handle && dst_is_variant && start.0 != target.0) {
            if let Some(p) = self.bfs_repr_only(&start, &target, is_repr_lane, canon) {
                return Some(p);
            }
        }

        self.bfs_full(&start, &target, is_repr_lane, canon)
    }

    /// Phase 1: representation-lanes-only BFS (see bfs_path's gate).
    fn bfs_repr_only(
        &self,
        start: &(String, String),
        target: &(String, String),
        is_repr_lane: fn(&LaneKind) -> bool,
        canon: impl Fn(&str, &str) -> (String, String),
    ) -> Option<Vec<CastStep>> {
        let mut visited: HashSet<(String, String)> = HashSet::new();
        let mut queue: VecDeque<((String, String), Vec<CastStep>)> = VecDeque::new();
        visited.insert(start.clone());
        queue.push_back((start.clone(), vec![]));

        while let Some((current, path)) = queue.pop_front() {
            if current == *target {
                return Some(path);
            }
            // Direct repr lane to the destination category.
            if let Some(lane) = self.get_lane(&current.0, &target.0) {
                if is_repr_lane(lane) && target.1.is_empty() {
                    let mut full_path = path.clone();
                    full_path.push(CastStep {
                        lane: lane.clone(),
                        src_category: current.0.clone(),
                        src_variant: current.1.clone(),
                        dst_category: target.0.clone(),
                        dst_variant: target.1.clone(),
                    });
                    return Some(full_path);
                }
            }
            // Variant edges — repr deltas only (the axiom CastTo/CastFrom
            // content edges are ExtCallDyn and excluded here).
            if let Some(edges) = self.variant_edges.get(&current) {
                for edge in edges {
                    if !is_repr_lane(&edge.lane) {
                        continue;
                    }
                    let neighbor = canon(&edge.dst_category, &edge.dst_variant);
                    if visited.insert(neighbor.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(edge.clone());
                        queue.push_back((neighbor, new_path));
                    }
                }
            }
            if let Some(edges) = self.variant_reverse.get(&current) {
                for edge in edges {
                    if !is_repr_lane(&edge.lane) {
                        continue;
                    }
                    let neighbor = canon(&edge.src_category, &edge.src_variant);
                    if visited.insert(neighbor.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(edge.clone());
                        queue.push_back((neighbor, new_path));
                    }
                }
            }
            // Repr base lanes as intermediate hops.
            if let Some(out_lanes) = self.base_lanes.get(&current.0) {
                for (next_cat, lane) in out_lanes {
                    if !is_repr_lane(lane) {
                        continue;
                    }
                    let neighbor = canon(next_cat, "");
                    if visited.insert(neighbor.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(CastStep {
                            lane: lane.clone(),
                            src_category: current.0.clone(),
                            src_variant: current.1.clone(),
                            dst_category: next_cat.clone(),
                            dst_variant: String::new(),
                        });
                        queue.push_back((neighbor, new_path));
                    }
                }
            }
        }
        None
    }

    /// Phase 2: the full BFS — variant edges (content deltas included),
    /// reverse edges, and base lanes (content gated to the base/default,
    /// repr from any variant).
    fn bfs_full(
        &self,
        start: &(String, String),
        target: &(String, String),
        is_repr_lane: fn(&LaneKind) -> bool,
        canon: impl Fn(&str, &str) -> (String, String),
    ) -> Option<Vec<CastStep>> {
        let mut visited: HashSet<(String, String)> = HashSet::new();
        let mut queue: VecDeque<((String, String), Vec<CastStep>)> = VecDeque::new();
        visited.insert(start.clone());
        queue.push_back((start.clone(), vec![]));

        while let Some((current, path)) = queue.pop_front() {
            if current == *target {
                return Some(path);
            }

            // Direct lane to the destination category. A lane from a base
            // node lands at the base ≡ the default variant (canonicalized).
            let at_base = current.1.is_empty() || current.1 == *self.default_variant(&current.0);
            if let Some(lane) = self.get_lane(&current.0, &target.0) {
                let lands_at_target = target.1.is_empty()
                    || target.1 == current.1
                    || (at_base && target.1 == *self.default_variant(&target.0));
                if (at_base || is_repr_lane(lane)) && lands_at_target {
                    let mut full_path = path.clone();
                    full_path.push(CastStep {
                        lane: lane.clone(),
                        src_category: current.0.clone(),
                        src_variant: current.1.clone(),
                        dst_category: target.0.clone(),
                        dst_variant: target.1.clone(),
                    });
                    return Some(full_path);
                }
            }

            // Follow variant edges. The auto boundary-admission edges
            // (Data → variant IntToPtr) are phase-1 territory — phase 2
            // traverses DECLARED content deltas only. Placeholder Bitcast
            // edges (unbound CastTo) stay traversable.
            if let Some(edges) = self.variant_edges.get(&current) {
                for edge in edges {
                    if matches!(edge.lane, LaneKind::IntToPtr) {
                        continue;
                    }
                    let neighbor = canon(&edge.dst_category, &edge.dst_variant);
                    if visited.insert(neighbor.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(edge.clone());
                        queue.push_back((neighbor, new_path));
                    }
                }
            }

            // Follow reverse edges (same admission-edge exclusion).
            if let Some(edges) = self.variant_reverse.get(&current) {
                for edge in edges {
                    if matches!(edge.lane, LaneKind::IntToPtr) {
                        continue;
                    }
                    let neighbor = canon(&edge.src_category, &edge.src_variant);
                    if visited.insert(neighbor.clone()) {
                        let mut new_path = path.clone();
                        new_path.push(edge.clone());
                        queue.push_back((neighbor, new_path));
                    }
                }
            }

            // Base lanes as intermediate hops — from the base/default only
            // (content AND repr alike). The original model: a variant reaches
            // other categories through its declared delta to the base, then
            // the base's lanes. No repr chaining here — a raw handle
            // round-trip between same-category variants would fabricate
            // paths the declaration set never made (B → A regression).
            if at_base {
                if let Some(out_lanes) = self.base_lanes.get(&current.0) {
                    for (next_cat, lane) in out_lanes {
                        let neighbor = canon(next_cat, "");
                        if visited.insert(neighbor.clone()) {
                            let mut new_path = path.clone();
                            new_path.push(CastStep {
                                lane: lane.clone(),
                                src_category: current.0.clone(),
                                src_variant: current.1.clone(),
                                dst_category: next_cat.clone(),
                                dst_variant: String::new(),
                            });
                            queue.push_back((neighbor, new_path));
                        }
                    }
                }
            }

            // Fallback: try default variant of current category
            if let Some(default_var) = self.defaults.get(&current.0) {
                if current.1 != *default_var {
                    let default_target = (current.0.clone(), default_var.clone());
                    if visited.insert(default_target.clone()) {
                        queue.push_back((default_target, path.clone()));
                    }
                }
            }
        }
        None
    }
    // ── Type-to-Protocol Resolution ────────────────────────────────────

    /// Map a Type to its (protocol_category, variant) for graph lookup.
    /// Uses TypeUniverse protocol membership properties rather than type name matching
    /// (per AGENTS.md Rule 18: NO TYPE NAME MATCHING).
    ///
    /// Compiler constructs not stored in the universe (Bits, Ptr, Void, HashWord) are
    /// handled directly as permitted exceptions (Rule 18a).
    pub fn type_to_protocol(&self, universe: &TypeUniverse, ty: &Type) -> (String, String) {
        match ty {
            // Compiler constructs (not in universe) — permitted direct handling per Rule 18a.
            Type::Bits(_) => return ("Bit".to_string(), String::new()),
            Type::Void => return ("Bit".to_string(), String::new()),
            // 2026-07-30: Ptr<T> deliberately NOT mapped to "Blob" here.
            // Mapping Ptr→Data would cause is_protocol_member(Ptr, "Blob")
            // to return true, breaking adapt_to_i64 which expects Ptr fields
            // (stored as i64 in %State) to NOT undergo ptrtoint conversion.
            // resolve_llvm_type() handles Ptr directly before calling this.
            // Type::Ptr(_) => ("Blob", ...) moved to resolve_llvm_type only.
            // 2026-09-11 (fundamentals doctrine, Phase A2): `Float<Posit>` —
            // the fundamental IS the protocol, so an Applied whose base is a
            // fundamental and whose single arg is a plain identifier IS the
            // (category, variant) pair. Replaces Float<Posit> at the AST
            // level. Multi-arg or non-identifier args are genuine generics —
            // fall through to the universe walk.
            Type::Applied(base, args)
                if crate::type_universe::FUNDAMENTAL_TYPES.contains(&base.as_str())
                    && args.len() == 1
                    && matches!(&args[0], Type::Custom(_)) =>
            {
                let variant = match &args[0] {
                    Type::Custom(v) => v.clone(),
                    _ => unreachable!("guarded by the match guard"),
                };
                return (base.clone(), variant);
            }
            Type::Custom(..) | Type::Applied(..) => {} // fall through to universe lookup
            // 2026-08-15 (fundamentals): unknown types fall back to Data (the
            // universal parent), not Bit (which is now the leaf bit type).
            _ => return ("Data".to_string(), String::new()),
        }

        // Resolve protocol category from universe properties.
        // 2026-07-30: Queries Cast.<Category> properties instead of matching type names.
        let key = ty.universe_key().and_then(|k| universe.get(k));
        let Some(rt) = key else {
            return ("Data".to_string(), String::new());
        };

        // 2026-09-02 (plan fundamental-parent-membership): category
        // resolution is ONE walk — Cast.* properties at every hop (the
        // shared CAST_CATEGORY_PROPS table), then the declared base chain.
        // The walk LOOPS through universe entries so multi-level fundamental
        // chains (`MyHalf : Float16 : Float`) resolve to the fundamental's
        // category instead of collapsing to Data (the old code read one
        // level of rt.base, and its Cast.* order was an inline copy of
        // operators::protocol_category). Variants survive the walk: a base
        // `String<C_String>` carries C_String to the resolved lane. Never
        // matches type names (rule 18). Undo: restore the single-level
        // base read and the inline Cast.* if-chain.
        let mut current = rt;
        let mut visited: Vec<String> = Vec::new();
        loop {
            for (prop, cat) in crate::type_universe::CAST_CATEGORY_PROPS {
                if current.properties.contains_key(*prop) {
                    return (cat.to_string(), String::new());
                }
            }
            // 2026-08-01 (B2): no Cast. property (the normalizer no longer
            // injects them) — follow the type's declared `base` parent
            // (`type Latin1String: String` ⇒ base "String"). This makes
            // subtypes resolve to their protocol category so the casting
            // graph's lanes (e.g. Bit → String encoding door with a
            // CastFrom(Bit) override) apply to them.
            // 2026-08-03: variant bases (`type CStr: String<C_String>` ⇒
            // base "String<C_String>") resolve to (category, variant).
            let Some((cat, var)) = Self::parse_protocol_base(&current.base) else {
                return ("Data".to_string(), String::new());
            };
            match cat.as_str() {
                // 2026-08-15 (fundamentals): Data and Bit are base
                // categories too. A type whose base is Data resolves to
                // Data (the universal parent).
                "Float" | "UInt" | "Int" | "String" | "Bool" | "Char" | "Blob"
                | "Data" | "Bit" => {
                    return (cat, var);
                }
                _ => {}
            }
            // 2026-09-11 (fundamentals doctrine, B1): a SELF-base is a
            // parentless declared type — it IS its own category root
            // (`type Volt { }` registers base "Volt" and resolves to
            // (Volt, "")). No table of electrical names exists; this is
            // pure mechanism.
            if cat == current.name {
                return (cat, var);
            }
            // The base names another universe type — follow its
            // (protocol, metadata). A declaration cycle (`A : B, B : A`)
            // must not spin: visited-set guard.
            if visited.contains(&cat) {
                return ("Data".to_string(), String::new());
            }
            visited.push(cat.clone());
            match universe.get(&cat) {
                Some(next) => current = next,
                None => return ("Data".to_string(), String::new()),
            }
        }
    }

    /// Parse a protocol base string (`#Cat`, `#Cat<Variant>`, or bare `Cat`)    /// into `(category, variant)`. Returns None for empty/unparseable strings.
    /// 2026-08-03: `String<C_String>` → `("String", "CString")`; `String` →
    /// `("String", "")`.
    pub fn parse_protocol_base(base: &str) -> Option<(String, String)> {
        let b = base.trim_start_matches('#');
        if let Some(lt) = b.find('<') {
            let cat = b[..lt].to_string();
            let variant = b[lt + 1..].trim_end_matches('>').to_string();
            if !cat.is_empty() {
                return Some((cat, variant));
            }
        } else if !b.is_empty() {
            return Some((b.to_string(), String::new()));
        }
        None
    }

    // ── SPIR-V Type Resolution (§2.4) ───────────────────────────────────

    /// Register the SPIR-V resolver for a protocol (category, variant).
    pub fn set_spirv_type(&mut self, category: &str, variant: &str, r: SpirvTypeResolver) {
        self.protocol_spirv_types
            .insert((category.to_string(), variant.to_string()), r);
    }

    fn get_spirv_type(&self, category: &str, variant: &str) -> Option<&SpirvTypeResolver> {
        self.protocol_spirv_types.get(&(category.to_string(), variant.to_string()))
    }

    /// Resolve a Briev type to its SPIR-V scalar shape from
    /// (protocol, metadata) — the kernel-surface twin of resolve_llvm_type.
    ///
    /// Compiler constructs (Bits/Void/Ptr/Vector/Function) are NOT resolved
    /// here; callers handle them directly before consulting this method.
    ///
    /// Width ladder per category:
    /// - Int/UInt:  !> bits → !> maxbits → !> minbits → default_int_bits
    /// - Float:     !> bits → !> maxbits → !> minbits → 32
    ///
    /// Err carries the protocol CATEGORY and the concrete fix — this is a
    /// capability error, never a silent fallback.
    pub fn resolve_spirv_shape(
        &self,
        universe: &TypeUniverse,
        ty: &Type,
        default_int_bits: u64,
    ) -> Result<SpirvShape, String> {
        let (category, variant) = self.type_to_protocol(universe, ty);
        let resolver = self
            .get_spirv_type(&category, &variant)
            .or_else(|| self.get_spirv_type(&category, self.default_variant(&category)))
            .or_else(|| self.get_spirv_type(&category, ""));
        let Some(resolver) = resolver else {
            return Err(format!(
                "type '{}' lowers to protocol '{}' — GPU kernels support scalar                  state rooted in Int, UInt, Float, or Bool only (no heap,                  strings, or opaque storage in kernel address space)",
                ty, category
            ));
        };
        let bits_of = |keys: &[&str]| -> Option<u64> {
            let key = ty.universe_key().and_then(|k| universe.get(k));
            keys.iter().find_map(|k| {
                key.and_then(|rt| rt.properties.get(*k)).and_then(|pv| match pv {
                    PropertyValue::Int(n) if *n > 0 => Some(*n as u64),
                    _ => None,
                })
            })
        };
        Ok(match resolver {
            SpirvTypeResolver::IntWidth(signed) => {
                let signed = *signed;
                let bits = bits_of(&["bits", "maxbits", "minbits"])
                    .unwrap_or(default_int_bits);
                // Shader capability integer widths (SPV 1.x, no Int8 short form):
                // 8 needs the Int8 capability AND storage-only use; kernels are
                // compute surfaces, so the honest floor is 16.
                match bits {
                    8 | 16 | 32 | 64 => SpirvShape::Int { bits: bits as u32, signed },
                    other => return Err(format!(
                        "integer width {} is not a Vulkan compute width                          (8/16/32/64) — fix the type's bits metadata",
                        other
                    )),
                }
            }
            SpirvTypeResolver::FloatWidth => {
                let bits = bits_of(&["bits", "maxbits", "minbits"]).unwrap_or(32);
                // 2026-09-01 (M2.2, plan 2026-09-01-m2-tensor-cores): 16-bit
                // floats joined the kernel surface — SSBO storage via
                // StorageBuffer16BitAccess, tensor fragments via
                // VK_KHR_cooperative_matrix (fp16 operands, fp32
                // accumulate). No f16 ARITHMETIC is emitted: kernels use f16
                // only for storage and fragments, so shaderFloat16 stays off.
                match bits {
                    16 | 32 | 64 => SpirvShape::Float { bits: bits as u32 },
                    other => return Err(format!(
                        "float width {} is not a kernel float width                          (16/32/64) — declare the state field as                          Float {{ !> bits: 32 }} or Float {{ !> bits: 64 }}",
                        other
                    )),
                }
            }
            SpirvTypeResolver::Fixed(SpirvScalar::Bool) => SpirvShape::Bool,
        })
    }

    // ── LLVM Type Resolution ──────────────────────────────────────────

    /// Resolve the LLVM type string for a given Briev type.
    ///
    /// Derived from (protocol, metadata) by the casting graph. This replaces
    /// the normalizer's three-phase llvm_type derivation and primordial
    /// llvm_type properties.
    ///
    /// Width resolution priority (WidthParametric protocols):
    /// 1. `!> bits: N` — exact width (hard contract)
    /// 2. `!> maxbits: N` — upper bound
    /// 3. `!> minbits: N` — lower bound
    /// 4. `int_bits` — target default (64 for x86_64, 32 for wasm32)
    pub fn resolve_llvm_type(&self, universe: &TypeUniverse, ty: &Type, int_bits: u64) -> String {
        // Compiler constructs handled directly
        match ty {
            Type::Ptr(_) | Type::PtrConst(_) => return "ptr".to_string(),
            Type::Bits(n) => return format!("i{}", n),
            Type::Void => return "void".to_string(),
            _ => {}
        }

        let (category, variant) = self.type_to_protocol(universe, ty);
        // 2026-08-03: an unseeded `#Category<Variant>` falls back to the
        // category's default variant, then the base category — a `String`
        // sub-protocol IS a String (ptr); only its encoding differs. Without
        // this, `type CStr: String<C_String>` resolved to `i64`.
        let resolver = self.get_llvm_type(&category, &variant)
            .or_else(|| self.get_llvm_type(&category, self.default_variant(&category)))
            .or_else(|| self.get_llvm_type(&category, ""));
        match resolver {
            Some(LlvmTypeResolver::Fixed(ty_str)) => return ty_str.to_string(),
            Some(LlvmTypeResolver::WidthParametric) => {
                // Check metadata from universe entry
                let key = ty.universe_key().and_then(|k| universe.get(k));
                let bits = key.and_then(|rt| {
                    // Priority: !> bits → !> maxbits → !> minbits
                    rt.properties.get("bits")
                        .or_else(|| rt.properties.get("maxbits"))
                        .or_else(|| rt.properties.get("minbits"))
                        .and_then(|pv| match pv {
                            PropertyValue::Int(n) => Some(*n as u64),
                            _ => None,
                        })
                }).unwrap_or(int_bits);
                return format!("i{}", bits);
            }
            Some(LlvmTypeResolver::FloatWidth) => {
                // 2026-08-03: the Float protocol owns the width semantics —
                // derive the LLVM type from the type's `bits` metadata.
                // 16-bit is half/bfloat (via disamb); 32→float, 64→double,
                // 80→x86_fp80, 128→fp128, default→float. No type names.
                let key = ty.universe_key().and_then(|k| universe.get(k));
                let bits = key.and_then(|rt| {
                    rt.properties.get("bits")
                        .or_else(|| rt.properties.get("maxbits"))
                        .or_else(|| rt.properties.get("minbits"))
                        .and_then(|pv| match pv {
                            PropertyValue::Int(n) => Some(*n as u64),
                            _ => None,
                        })
                });
                let float_ty = match bits {
                    Some(16) => {
                        // Half vs BFloat are both 2-byte Float variants,
                        // distinguished by the `disamb` metadata value.
                        let disamb = key.and_then(|rt| match rt.properties.get("disamb") {
                            Some(PropertyValue::String(s)) => Some(s.clone()),
                            _ => None,
                        });
                        if disamb.as_deref() == Some("bfloat") {
                            "bfloat".to_string()
                        } else {
                            "half".to_string()
                        }
                    }
                    Some(32) => "float".to_string(),
                    Some(64) => "double".to_string(),
                    Some(80) => "x86_fp80".to_string(),
                    Some(128) => "fp128".to_string(),
                    _ => "float".to_string(),
                };
                return float_ty;
            }
            None => {}
        }

        // Fallback for non-protocol types (plain structs, user types without protocol)
        if let Some(rt) = ty.universe_key().and_then(|k| universe.get(k)) {
            if !rt.fields.is_empty() {
                let field_tys: Vec<String> = rt.fields.iter()
                    .map(|(_, fty)| self.resolve_llvm_type(universe, fty, int_bits))
                    .collect();
                return format!("{{ {} }}", field_tys.join(", "));
            }
        }

        // Ultimate fallback
        "i64".to_string()
    }
}

impl Default for CastingGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Derive the type → declared-protocol map from the AST, pre-universe.
/// Explicit declarations win (`type CStr: String<C_String>` →
/// "String<C_String>"); a bare-parent typedef DERIVES its category by
/// walking the parent chain — to another typedef's declaration or to a
/// fundamental name (`type Float16 : Float` → "Float"). 2026-09-02 (plan
/// fundamental-parent-membership): the glue/FFI and boundary-marshalling
/// passes built this map from explicit declarations only, so a
/// de-hashtagged fundamental join lost its category at the boundary.
/// One helper, both callers (rule 17). Visited-set guards cycles. Undo:
/// restore the explicit-only inline loops in the two callers.
pub fn derive_type_protocols(items: &[crate::ast::TopLevel]) -> std::collections::HashMap<String, String> {
    use crate::ast::TopLevel;
    use std::collections::HashMap;
    // child → (explicit protocol, parent name)
    let mut decls: HashMap<&str, (Option<&str>, Option<&str>)> = HashMap::new();
    for item in items {
        if let TopLevel::TypeDef(td) = item {
            let parent = td.parent.as_ref().and_then(|e| match e.as_ref() {
                crate::ast::Expr::Identifier(n) => Some(n.as_str()),
                _ => None,
            });
            decls.insert(td.name.as_str(), (td.protocol.as_deref(), parent));
        }
    }
    let mut map: HashMap<String, String> = HashMap::new();
    for (name, (proto, _)) in &decls {
        if let Some(p) = proto {
            map.insert(name.to_string(), p.to_string());
            continue;
        }
        let mut current = *name;
        let mut visited: Vec<&str> = Vec::new();
        loop {
            let Some((proto, parent)) = decls.get(current).copied() else {
                break;
            };
            if let Some(p) = proto {
                map.insert(name.to_string(), p.to_string());
                break;
            }
            let Some(parent) = parent else { break };
            if crate::type_universe::FUNDAMENTAL_TYPES.contains(&parent) {
                // 2026-09-11 (Phase A4): bare — the fundamental name IS the
                // category; hashword spellings are retired.
                map.insert(name.to_string(), parent.to_string());
                break;
            }
            if visited.contains(&parent) {
                break;
            }
            visited.push(parent);
            current = parent;
        }
    }
    map
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-02 (plan fundamental-parent-membership): the AST-level
    /// protocol map derives bare-parent typedefs through the parent chain —
    /// explicit declarations keep precedence; non-fundamental parents
    /// continue the walk; unknown parents terminate.
    #[test]
    fn derive_type_protocols_walks_fundamental_parents() {
        use crate::ast::TopLevel;
        use crate::ast::top::TypeDef;
        use crate::ast::Expr;
        let td = |name: &str, proto: Option<&str>, parent: Option<&str>| {
            TopLevel::TypeDef(Box::new(TypeDef {
                name: name.into(),
                type_params: vec![],
                parent: parent.map(|p| Box::new(Expr::Identifier(p.into()))),
                protocol: proto.map(str::to_string),
                traits: vec![],
                bit_range: None,
                coll: false,
                ports_in: vec![],
                ports_out: vec![],
                seq: false,
                body: crate::ast::top::TypeDefBody {
                    pins: Vec::new(),
                    slots: vec![],
                    metadata: Default::default(),
                    projections: vec![],
                    bindings: vec![],
                    operators: vec![],
                    op_bindings: vec![],
                    constraints: vec![],
                    members: vec![],
                    span: None,
                },
                span: None,
            }))
        };
        let items = vec![
            td("CStr", Some("String<C_String>"), None),
            td("Float16", None, Some("Float")),
            td("MyHalf", None, Some("Float16")),
            td("Weird", None, Some("Unrelated")),
        ];
        let map = derive_type_protocols(&items);
        assert_eq!(map.get("CStr").map(String::as_str), Some("String<C_String>"));
        assert_eq!(map.get("Float16").map(String::as_str), Some("Float"));
        assert_eq!(map.get("MyHalf").map(String::as_str), Some("Float"));
        assert!(!map.contains_key("Weird"), "unknown parent terminates the walk");
    }

    #[test]
    fn test_all_base_pairs_have_lanes() {
        let graph = CastingGraph::new();
        // 2026-08-15 (fundamentals): Data is the root; Bit<N> is the bit type
        // (a Data member). Both are in the base-pair mesh.
        let protocols = &["Data", "Bit", "Int", "UInt", "Float", "String", "Bool", "Char", "Blob"];
        for src in protocols {
            for dst in protocols {
                if src == dst {
                    continue; // identity
                }
                let path = graph.find_path(src, "", dst, "");
                assert!(
                    path.is_some(),
                    "missing lane: {} → {}",
                    src,
                    dst
                );
            }
        }
    }

    #[test]
    fn test_identity_path() {
        let graph = CastingGraph::new();
        let path = graph.find_path("Int", "", "Int", "");
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 0);
    }

    #[test]
    fn test_int_to_float() {
        let graph = CastingGraph::new();
        let path = graph.find_path("Int", "", "Float", "");
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 1);
    }

    #[test]
    fn test_string_to_bit() {
        // 2026-08-01 (B2): String → Bit (the CONTENT VIEW) now routes through
        // Data: String→Data is PtrToInt (a String IS a ptr to [len][bytes]),
        // then Data→Bit is a bitcast (treat-as-bits). 2026-08-15 (fundamentals):
        // Data replaced Bit as the root.
        let graph = CastingGraph::new();
        let path = graph.find_path("String", "", "Bit", "");
        assert!(path.is_some());
        assert_eq!(path.unwrap()[0].lane, LaneKind::PtrToInt);
    }

    #[test]
    fn test_bit_to_string() {
        // 2026-08-01 (B2): Bit → String (the ENCODING DOOR) now routes through
        // Data: Bit→Data is a bitcast, then Data→String is the CastFrom(Data)
        // callback (UTF8 wrap default materializing the [len][bytes] header).
        let graph = CastingGraph::new();
        let path = graph.find_path("Bit", "", "String", "");
        assert!(path.is_some());
        // The last hop is the encoding door.
        let last = path.unwrap().last().unwrap().lane.clone();
        assert_eq!(last, LaneKind::CastFromBitCallback);
    }

    #[test]
    fn test_bit_type_walks_to_storage_category() {
        // 2026-09-11 (Phase A4): the retired HashWord("#Bit") special case is
        // gone — the bare `Bit` type walks its Cast.* properties like any
        // other type. Per the 2026-08-15 doctrine Data is the graph ROOT and
        // Bit's storage membership is Cast.Data, so the walk lands on Data —
        // whose lanes are the base bitcast/enc doors. find_path(String → Bit)
        // still resolves: the first hop is String→Data PtrToInt.
        let graph = CastingGraph::new();
        let u = crate::type_universe::TypeUniverse::new();
        let b_ty = crate::ast::Type::Custom("Bit".to_string());
        let (cat, var) = graph.type_to_protocol(&u, &b_ty);
        assert_eq!(cat, "Data");
        assert_eq!(var, "");
        let path = graph.find_path("String", "", "Bit", "");
        assert!(path.is_some(), "String → Bit must route through the Data root");
        assert_eq!(path.unwrap()[0].lane, LaneKind::PtrToInt);
    }

    // 2026-09-11 (fundamentals doctrine, B1): a PARENTLESS declared type is
    // its own category root — pure mechanism, no name tables. `type Volt
    // { }` registers base "Volt" (self) and resolves to (Volt, "").
    #[test]
    fn test_parentless_type_self_roots_as_category() {
        use crate::ast::top::*;
        let items = vec![TopLevel::TypeDef(Box::new(TypeDef {
            name: "Volt".into(),
            type_params: vec![],
            parent: None,
            protocol: None,
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: vec![],
            ports_out: vec![],
            seq: false,
            body: TypeDefBody {
                slots: vec![],
                pins: vec![],
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("bits".to_string(), crate::ast::PropertyValue::Int(32));
                    m
                },
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        }))];
        let mut universe = crate::type_universe::TypeUniverse::new();
        crate::backend::register_types::register_typedefs(&items, &mut universe, 64).unwrap();
        let graph = CastingGraph::new();
        let (cat, var) = graph.type_to_protocol(&universe, &Type::Custom("Volt".into()));
        assert_eq!(cat, "Volt", "parentless type must self-root as its own category");
        assert_eq!(var, "");
    }

    #[test]
    fn test_default_variants() {
        let graph = CastingGraph::new();
        assert_eq!(graph.default_variant("String"), "UTF8");
        assert_eq!(graph.default_variant("Float"), "IEEE754");
        assert_eq!(graph.default_variant("Int"), "");
    }

    #[test]
    fn test_variant_edge() {
        let mut graph = CastingGraph::new();
        // Simulate proto ASCII: String { CastTo(String): ascii_to_utf8(#L); }
        graph.register_protocol_def(&ProtocolDef {
            name: "ASCII".to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![crate::ast::top::CastEdge {
                direction: crate::ast::top::CastDirection::CastTo,
                target_category: "String".to_string(),
                target_variant: "UTF8".to_string(),
                binding: None,
            trusted_axiom: false}],
            cross_ops: vec![],
            span: None,
        });

        let path = graph.find_path("String", "ASCII", "String", "UTF8");
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 1);
    }

    #[test]
    fn test_cast_from_bit_override() {
        let mut graph = CastingGraph::new();
        graph.register_cast_from_bit("MyString", "construct_from_bits");
        assert_eq!(graph.get_cast_from_bit("MyString"), Some("construct_from_bits"));
        assert_eq!(graph.get_cast_from_bit("Other"), None);
    }

    // 2026-09-11 (fundamentals doctrine, Phase A2): `Float<Posit>` — the
    // fundamental-with-variant Applied form resolves to (category, variant),
    // NOT the category default (IEEE754). Guards against the silent
    // variant-drop into the wrong cast lane.
    #[test]
    fn test_applied_fundamental_variant_peel() {
        let graph = CastingGraph::new();
        let universe = crate::type_universe::TypeUniverse::new();
        let posit = Type::Applied(
            "Float".to_string(),
            vec![Type::Custom("Posit".to_string())],
        );
        assert_eq!(
            graph.type_to_protocol(&universe, &posit),
            ("Float".to_string(), "Posit".to_string()),
            "variant must survive the peel — defaulting to IEEE754 would cast in the wrong lane"
        );
        let utf8 = Type::Applied(
            "String".to_string(),
            vec![Type::Custom("C_String".to_string())],
        );
        assert_eq!(
            graph.type_to_protocol(&universe, &utf8),
            ("String".to_string(), "C_String".to_string())
        );
        // Multi-arg Applied is a genuine generic — NOT a variant pair.
        let generic = Type::Applied(
            "Float".to_string(),
            vec![Type::Custom("A".to_string()), Type::Custom("B".to_string())],
        );
        let (cat, var) = graph.type_to_protocol(&universe, &generic);
        assert_eq!(cat, "Float");
        assert_eq!(var, "", "two args cannot be a variant pair");
    }

    #[test]
    fn test_type_to_protocol_primitives() {
        let graph = CastingGraph::new();
        let universe = crate::type_universe::TypeUniverse::new();        // Compiler constructs (no universe needed)
        assert_eq!(graph.type_to_protocol(&universe, &Type::Bits(42)), ("Bit".to_string(), String::new()));
        // 2026-07-30: Ptr<T> is no longer mapped to Data — it's not in
        // type_to_protocol. resolve_llvm_type handles Ptr directly.
        // Ptr fields are stored as i64 in %State to avoid ptrtoint conversion.

        // Universe-resolved types (seeded primordials)
        assert_eq!(graph.type_to_protocol(&universe, &Type::Custom("Int".to_string())), ("Int".to_string(), String::new()));
        assert_eq!(graph.type_to_protocol(&universe, &Type::Custom("Float".to_string())), ("Float".to_string(), String::new()));
        assert_eq!(graph.type_to_protocol(&universe, &Type::Custom("Bool".to_string())), ("Bool".to_string(), String::new()));
        assert_eq!(graph.type_to_protocol(&universe, &Type::Custom("Blob".to_string())), ("Blob".to_string(), String::new()));
        // Fallback — no Cast. properties for unknown types → Data (the
        // universal parent; Bit is now the leaf bit type).
        assert_eq!(graph.type_to_protocol(&universe, &Type::Custom("UnknownType".to_string())), ("Data".to_string(), String::new()));
    }

    #[test]
    fn test_proto_binding_becomes_ext_call_lane() {
        // 2026-08-03: `proto C_String: String { CastTo(String<UTF8>) =
        // cstr_to_briev(#L); CastFrom(String<UTF8>) = str_to_c(#L); }` — the
        // bindings must become real call lanes (ExtCallDyn), not the old
        // Bitcast placeholder.
        let mut graph = CastingGraph::new();
        graph.register_protocol_def(&ProtocolDef {
            name: "C_String".to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![
                crate::ast::top::CastEdge {
                    direction: crate::ast::top::CastDirection::CastTo,
                    target_category: "String".to_string(),
                    target_variant: "UTF8".to_string(),
                    binding: Some(crate::ast::top::CastBinding {
                        fn_name: "cstr_to_briev".to_string(),
                        param: "#Lh".to_string(),
                    }),
                trusted_axiom: false},
                crate::ast::top::CastEdge {
                    direction: crate::ast::top::CastDirection::CastFrom,
                    target_category: "String".to_string(),
                    target_variant: "UTF8".to_string(),
                    binding: Some(crate::ast::top::CastBinding {
                        fn_name: "str_to_c".to_string(),
                        param: "#Lh".to_string(),
                    }),
                trusted_axiom: false},
            ],
            cross_ops: vec![],
            span: None,
        });

        // C_String → UTF8 uses the CastTo binding.
        let path = graph.find_path("String", "C_String", "String", "UTF8")
            .expect("C_String -> UTF8 path");
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].lane, LaneKind::ExtCallDyn("cstr_to_briev".to_string()));

        // UTF8 → C_String uses the CastFrom binding.
        let rev = graph.find_path("String", "UTF8", "String", "C_String")
            .expect("UTF8 -> C_String path");
        assert_eq!(rev.len(), 1);
        assert_eq!(rev[0].lane, LaneKind::ExtCallDyn("str_to_c".to_string()));
    }

    #[test]
    fn test_inverse_pair_collapse() {
        // 2026-08-03 (P1.5): A.CastTo(String) is `<< 1`, B.CastFrom(String)
        // is `>> 1` — the composition is identity, so A → B through the base
        // is a ZERO delta (the sub-types are 1-to-1).
        let mut graph = CastingGraph::new();
        let proto = |name: &str, dir: CastDirection, fn_name: &str| ProtocolDef {
            name: name.to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![crate::ast::top::CastEdge {
                direction: dir,
                target_category: "String".to_string(),
                target_variant: "UTF8".to_string(),
                binding: Some(crate::ast::top::CastBinding {
                    fn_name: fn_name.to_string(),
                    param: "#Lh".to_string(),
                }),
            trusted_axiom: false}],
            cross_ops: vec![],
            span: None,
        };
        graph.register_protocol_def(&proto("A", CastDirection::CastTo, "shift_left"));
        graph.register_protocol_def(&proto("B", CastDirection::CastFrom, "shift_right"));

        // Without the inverse pair, A → UTF8 → B is two steps.
        let plain = graph.find_path("String", "A", "String", "B");
        assert!(plain.as_ref().is_some_and(|p| p.len() == 2));

        // Register the proven inverse pair → the cast collapses to identity.
        graph.register_inverse_pair("String", "A", "B");
        let collapsed = graph.find_path("String", "A", "String", "B");
        assert_eq!(collapsed, Some(vec![]));
        // The reverse direction has no path at all (only A.CastTo + B.CastFrom
        // are declared) — the collapse is asymmetric, keyed to the proven pair.
        let rev = graph.find_path("String", "B", "String", "A");
        assert!(rev.is_none(), "B → A has no edges; only A.CastTo and B.CastFrom exist");
    }

    #[test]
    fn test_float_width_resolution() {
        // 2026-08-03: the Float protocol owns the width semantics — derived
        // from the type's `bits` metadata, no type names matched.
        let graph = CastingGraph::new();
        let universe = crate::type_universe::TypeUniverse::new();
        assert_eq!(graph.resolve_llvm_type(&universe, &Type::Custom("Float".to_string()), 64), "float");
        assert_eq!(graph.resolve_llvm_type(&universe, &Type::Custom("Float64".to_string()), 64), "double");
        assert_eq!(graph.resolve_llvm_type(&universe, &Type::Custom("Half".to_string()), 64), "half");
        // Float→Float<C_Double> is a width lane (fpext), not a chain.
        let path = graph.find_path("Float", "", "Float", "C_Double");
        assert!(path.is_some_and(|p| matches!(p[0].lane, LaneKind::FloatWidth)));
    }

    #[test]
    fn test_resolve_llvm_type_variant_fallback() {
        // 2026-08-03, migrated 2026-09-11 (Phase A4): unseeded
        // `String<C_String>` must resolve like any other String (ptr), not
        // fall through to i64; `Float<C_Double>` → double.
        let graph = CastingGraph::new();
        let universe = crate::type_universe::TypeUniverse::new();
        let applied = |base: &str, variant: &str| {
            Type::Applied(base.to_string(), vec![Type::Custom(variant.to_string())])
        };
        assert_eq!(
            graph.resolve_llvm_type(&universe, &applied("String", "C_String"), 64),
            "ptr"
        );
        assert_eq!(
            graph.resolve_llvm_type(&universe, &applied("Float", "C_Double"), 64),
            "double"
        );
        assert_eq!(
            graph.resolve_llvm_type(&universe, &applied("String", "UTF8"), 64),
            "ptr"
        );
        assert_eq!(
            graph.resolve_llvm_type(&universe, &applied("Float", "Double"), 64),
            "double"
        );
    }

    #[test]
    fn test_parse_protocol_base_variants() {        // 2026-08-03: variant bases — the FFI boundary types declare
        // `type CStr: String<C_String>` ⇒ base "String<C_String>".
        assert_eq!(CastingGraph::parse_protocol_base("String<C_String>"),
            Some(("String".to_string(), "C_String".to_string())));
        assert_eq!(CastingGraph::parse_protocol_base("Float<C_Double>"),
            Some(("Float".to_string(), "C_Double".to_string())));
        assert_eq!(CastingGraph::parse_protocol_base("Int<C_I32>"),
            Some(("Int".to_string(), "C_I32".to_string())));
        assert_eq!(CastingGraph::parse_protocol_base("String"),
            Some(("String".to_string(), String::new())));
        assert_eq!(CastingGraph::parse_protocol_base("String"),
            Some(("String".to_string(), String::new())));
        assert_eq!(CastingGraph::parse_protocol_base(""), None);
        assert_eq!(CastingGraph::parse_protocol_base("<"), None);
    }
}

#[cfg(test)]
mod cross_op_tests {
    use super::*;
    use crate::ast::top::{CastDirection, CastEdge, CastBinding, OperatorDef, ProtocolDef};
    use crate::ast::PropertyValue;

    #[test]
    fn test_get_variant_op() {
        let mut graph = CastingGraph::new();
        graph.register_protocol_def(&ProtocolDef {
            name: "C_String".to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![CastEdge {
                direction: CastDirection::CastFrom,
                target_category: "String".to_string(),
                target_variant: "UTF8".to_string(),
                binding: Some(CastBinding {
                    fn_name: "str_to_c".to_string(),
                    param: "#Lh".to_string(),
                }),
            trusted_axiom: false}],
            cross_ops: vec![OperatorDef {
                op: "Concat".to_string(),
                params: vec![],
                pre: None,
                suf: None,
                impl_args: Some(PropertyValue::List(vec![
                    PropertyValue::Identifier("cstring_concat".to_string()),
                    PropertyValue::HashL,
                    PropertyValue::HashR,
                ])),
                impl_name: String::new(),
                span: None,
                trusted_axiom: false,
            trusted_lemmas: vec![]}],
            span: None,
        });
        assert_eq!(graph.get_variant_op("String", "C_String", "Concat"), Some("cstring_concat"));
        assert_eq!(graph.get_variant_op("String", "C_String", "Add"), None);
        assert_eq!(graph.get_variant_op("String", "Other", "Concat"), None);
    }
}
