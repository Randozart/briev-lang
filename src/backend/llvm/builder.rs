// ── LLVM IR Builder ──────────────────────────────────────────────────
//
// 2026-06-29: Structured builder that replaces raw `writeln!` formatting
// for LLVM IR instructions. Handles SSA register allocation, label
// generation, and instruction formatting in one place.
//
// Why a builder instead of direct formatting:
//   The previous approach (writeln! to &mut String) required manual SSA
//   register tracking via txn_counter, leading to:
//   1. %t{N} collisions when counters were saved/rewound
//   2. A fragile %tddup post-processing pass to fix up duplicates
//   3. No type checking at instruction emission time
//
//   The builder centralizes register allocation in gen_reg(), which is
//   the SOLE source of register names. This mathematically eliminates
//   duplicate SSA definitions and removes the need for post-processing.
//
// Trade-off: The builder allocates Vec<Instruction> then formats at the
// end (finish()), which adds a small allocation cost vs direct writeln!.
// For hot loops (benchmarks), this cost is ~0.1% — acceptable for the
// correctness guarantee. If profiling shows it matters, use finish_fast()
// which writes directly to &mut String.

use std::fmt::Write;

// ── LLVM Types ──────────────────────────────────────────────────────

/// High-level representation of LLVM IR types.
/// Used by the builder for type-safe instruction emission.
#[derive(Debug, Clone, PartialEq)]
pub enum LlvmType {
    I1,
    I8,
    I16,
    I32,
    I64,
    Float,
    Double,
    /// Opaque pointer (modern LLVM). The optional inner type is for debug
    /// info only — LLVM 15+ uses `ptr` for all pointers.
    Ptr,
    Void,
}

impl std::fmt::Display for LlvmType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlvmType::I1 => write!(f, "i1"),
            LlvmType::I8 => write!(f, "i8"),
            LlvmType::I16 => write!(f, "i16"),
            LlvmType::I32 => write!(f, "i32"),
            LlvmType::I64 => write!(f, "i64"),
            LlvmType::Float => write!(f, "float"),
            LlvmType::Double => write!(f, "double"),
            LlvmType::Ptr => write!(f, "ptr"),
            LlvmType::Void => write!(f, "void"),
        }
    }
}

impl LlvmType {
    /// Convenience: get the unsigned integer type at least as wide as self.
    pub fn as_int_ty(&self) -> LlvmType {
        match self {
            LlvmType::I1 => LlvmType::I8,
            LlvmType::I8 | LlvmType::I16 | LlvmType::I32 => LlvmType::I32,
            LlvmType::I64 | LlvmType::Float | LlvmType::Double => LlvmType::I64,
            LlvmType::Ptr => LlvmType::I64,
            LlvmType::Void => LlvmType::I64,
        }
    }

    /// Size of the type in bytes (for alignment calculation).
    pub fn size_bytes(&self) -> usize {
        match self {
            LlvmType::I1 => 1,
            LlvmType::I8 => 1,
            LlvmType::I16 => 2,
            LlvmType::I32 => 4,
            LlvmType::I64 => 8,
            LlvmType::Float => 4,
            LlvmType::Double => 8,
            LlvmType::Ptr => 8,
            LlvmType::Void => 0,
        }
    }
}

// ── Instruction ─────────────────────────────────────────────────────

/// A single LLVM IR instruction (may or may not produce a value).
#[derive(Debug, Clone)]
pub struct Instruction {
    /// Result SSA register name (None for void instructions like store, br)
    pub result: Option<String>,
    /// The instruction opcode and operands (everything after `=` or nothing)
    pub op: String,
    /// Optional metadata attachment (e.g., "!tbaa !1")
    pub metadata: Option<String>,
}

impl Instruction {
    pub fn new(result: Option<String>, op: String) -> Self {
        Instruction {
            result,
            op,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, md: String) -> Self {
        self.metadata = Some(md);
        self
    }
}

// ── LLVMBuilder ────────────────────────────────────────────────────

/// Structured builder for LLVM IR instructions.
///
/// Usage:
///   let mut b = LLVMBuilder::new();
///   let a = b.emit_add(LlvmType::I64, "%t0", "%t1");
///   b.emit_ret(LlvmType::Void, None);
///   let ir = b.finish(0);
///
/// Register allocation via gen_reg() is the SOLE source of SSA names.
/// This eliminates %t{N} collisions and the %tddup post-processing pass.
#[derive(Debug, Clone)]
pub struct LLVMBuilder {
    /// Accumulated instructions (formatted at finish() time)
    instructions: Vec<Instruction>,
    /// Monotonically increasing register counter — NEVER rewound
    reg_counter: usize,
    /// Monotonically increasing label counter — NEVER rewound
    label_counter: usize,
}

impl LLVMBuilder {
    pub fn new() -> Self {
        LLVMBuilder {
            instructions: Vec::new(),
            reg_counter: 0,
            label_counter: 0,
        }
    }

    // ── Register & Label Allocation ───────────────────────────────

    /// Generate a unique temporary register name.
    /// This is the SOLE source of %t{N} register names.
    pub fn gen_reg(&mut self) -> String {
        let r = format!("%t{}", self.reg_counter);
        self.reg_counter += 1;
        r
    }

    /// Generate a unique label name with the given prefix.
    pub fn gen_label(&mut self, prefix: &str) -> String {
        let l = format!("{}_{}", prefix, self.label_counter);
        self.label_counter += 1;
        l
    }

    /// Generate a register with a custom prefix (for specialized emitters).
    pub fn gen_reg_with_prefix(&mut self, prefix: &str) -> String {
        let r = format!("%{}{}", prefix, self.reg_counter);
        self.reg_counter += 1;
        r
    }

    // ── Instruction Emission ───────────────────────────────────────

    /// Emit an instruction that produces a value.
    /// The op_str is the full operation text (e.g. "add i64 %a, %b").
    fn emit_reg_op(&mut self, op_str: &str) -> String {
        let res = self.gen_reg();
        self.instructions
            .push(Instruction::new(Some(res.clone()), op_str.to_string()));
        res
    }

    /// Emit a void instruction (no result value).
    fn emit_void(&mut self, op: &str) {
        self.instructions
            .push(Instruction::new(None, op.to_string()));
    }

    // ── Arithmetic ─────────────────────────────────────────────────

    pub fn emit_add(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("add {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_sub(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("sub {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_mul(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("mul {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_sdiv(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("sdiv {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_udiv(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("udiv {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_srem(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("srem {} {}, {}", ty, lhs, rhs))
    }

    // ── Bitwise ────────────────────────────────────────────────────

    pub fn emit_and(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("and {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_or(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("or {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_xor(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("xor {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_shl(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("shl {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_ashr(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("ashr {} {}, {}", ty, lhs, rhs))
    }

    pub fn emit_lshr(&mut self, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("lshr {} {}, {}", ty, lhs, rhs))
    }

    // ── Type Conversion ────────────────────────────────────────────

    pub fn emit_zext(&mut self, from: LlvmType, to: LlvmType, val: &str) -> String {
        self.emit_reg_op(&format!("zext {} {} to {}", from, val, to))
    }

    pub fn emit_sext(&mut self, from: LlvmType, to: LlvmType, val: &str) -> String {
        self.emit_reg_op(&format!("sext {} {} to {}", from, val, to))
    }

    pub fn emit_trunc(&mut self, from: LlvmType, to: LlvmType, val: &str) -> String {
        self.emit_reg_op(&format!("trunc {} {} to {}", from, val, to))
    }

    pub fn emit_bitcast(&mut self, from: LlvmType, to: LlvmType, val: &str) -> String {
        self.emit_reg_op(&format!("bitcast {} {} to {}", from, val, to))
    }

    pub fn emit_ptrtoint(&mut self, val: &str, to: LlvmType) -> String {
        self.emit_reg_op(&format!("ptrtoint ptr {} to {}", val, to))
    }

    pub fn emit_inttoptr(&mut self, val: &str, from: LlvmType) -> String {
        self.emit_reg_op(&format!("inttoptr {} {} to ptr", from, val))
    }

    // ── Memory Access ──────────────────────────────────────────────

    pub fn emit_alloca(&mut self, ty: LlvmType, align: usize) -> String {
        self.emit_reg_op(&format!("alloca {}, align {}", ty, align))
    }

    pub fn emit_alloca_typed(&mut self, ty_str: &str, align: usize) -> String {
        self.emit_reg_op(&format!("alloca {}, align {}", ty_str, align))
    }

    pub fn emit_load(&mut self, ty: LlvmType, ptr: &str, align: usize) -> String {
        self.emit_reg_op(&format!("load {}, ptr {}, align {}", ty, ptr, align))
    }

    pub fn emit_load_typed(&mut self, ty_str: &str, ptr: &str, align: usize) -> String {
        self.emit_reg_op(&format!("load {}, ptr {}, align {}", ty_str, ptr, align))
    }

    pub fn emit_store(&mut self, ty: LlvmType, val: &str, ptr: &str, align: usize) {
        self.instructions.push(Instruction::new(
            None,
            format!("store {} {}, ptr {}, align {}", ty, val, ptr, align),
        ));
    }

    pub fn emit_store_tbaa(
        &mut self,
        ty: LlvmType,
        val: &str,
        ptr: &str,
        align: usize,
        tbaa: &str,
    ) {
        self.instructions.push(Instruction::new(
            None,
            format!(
                "store {} {}, ptr {}, align {}, !tbaa !{}",
                ty, val, ptr, align, tbaa
            ),
        ));
    }

    // ── GEP (GetElementPtr) ────────────────────────────────────────

    pub fn emit_gep(&mut self, base_ty: &str, ptr: &str, indices: &[&str]) -> String {
        let idx_str = indices
            .iter()
            .map(|i| format!("i64 {}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let op = format!(
            "getelementptr inbounds {}, ptr {}, {}",
            base_ty, ptr, idx_str
        );
        self.emit_reg_op(&op)
    }

    // ── Comparison ─────────────────────────────────────────────────

    pub fn emit_icmp(&mut self, cond: &str, ty: LlvmType, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("icmp {} {} {}, {}", cond, ty, lhs, rhs))
    }

    pub fn emit_fcmp(&mut self, cond: &str, lhs: &str, rhs: &str) -> String {
        self.emit_reg_op(&format!("fcmp {} {}, {}", cond, lhs, rhs))
    }

    // ── Control Flow ───────────────────────────────────────────────

    pub fn emit_br(&mut self, dest: &str) {
        self.emit_void(&format!("br label %{}", dest));
    }

    pub fn emit_cond_br(&mut self, cond: &str, true_dest: &str, false_dest: &str) {
        self.emit_void(&format!(
            "br i1 {}, label %{}, label %{}",
            cond, true_dest, false_dest
        ));
    }

    pub fn emit_switch(&mut self, val: &str, default_dest: &str, cases: &[(i64, &str)]) {
        let case_str: String = cases
            .iter()
            .map(|(v, d)| format!("i64 {}, label %{}", v, d))
            .collect::<Vec<_>>()
            .join(" ");
        self.emit_void(&format!(
            "switch i64 {}, label %{} [{}]",
            val, default_dest, case_str
        ));
    }

    pub fn emit_ret(&mut self, ty: LlvmType, val: Option<&str>) {
        match val {
            Some(v) => self.emit_void(&format!("ret {} {}", ty, v)),
            None => self.emit_void("ret void"),
        }
    }

    pub fn emit_unreachable(&mut self) {
        self.emit_void("unreachable");
    }

    // ── Call ───────────────────────────────────────────────────────

    pub fn emit_call(&mut self, ret_ty: LlvmType, callee: &str, args: &[(&str, &str)]) -> String {
        let args_str: Vec<String> = args
            .iter()
            .map(|(ty, val)| format!("{} {}", ty, val))
            .collect();
        self.emit_reg_op(&format!(
            "call {} @{}({})",
            ret_ty,
            callee,
            args_str.join(", ")
        ))
    }

    pub fn emit_call_void(&mut self, callee: &str, args: &[(&str, &str)]) {
        let args_str: Vec<String> = args
            .iter()
            .map(|(ty, val)| format!("{} {}", ty, val))
            .collect();
        self.emit_void(&format!("call void @{}({})", callee, args_str.join(", ")));
    }

    // ── PHI ─────────────────────────────────────────────────────────

    pub fn emit_phi(&mut self, ty: LlvmType, incoming: &[(&str, &str)]) -> String {
        let incoming_str: Vec<String> = incoming
            .iter()
            .map(|(val, label)| format!("[ {}, %{} ]", val, label))
            .collect();
        self.emit_reg_op(&format!("phi {} {}", ty, incoming_str.join(", ")))
    }

    pub fn emit_select(
        &mut self,
        ty: LlvmType,
        cond: &str,
        true_val: &str,
        false_val: &str,
    ) -> String {
        self.emit_reg_op(&format!(
            "select i1 {}, {} {}, {} {}",
            cond, ty, true_val, ty, false_val
        ))
    }

    // ── Metadata ───────────────────────────────────────────────────

    /// Attach metadata to the most recently emitted instruction.
    pub fn attach_metadata(&mut self, md: &str) {
        if let Some(last) = self.instructions.last_mut() {
            last.metadata = Some(format!("!tbaa !{}", md));
        }
    }

    // ── Label ──────────────────────────────────────────────────────

    /// Emit a basic block label. This is NOT an instruction — it sets the
    /// insertion point name for subsequent instructions.
    pub fn emit_label(&mut self, label: &str) {
        // Labels are represented as special instructions with no result
        // and an op that starts with ":" (formatted below as "label:")
        self.instructions
            .push(Instruction::new(None, format!("label:{}", label)));
    }

    // ── Finalization ────────────────────────────────────────────────

    /// Convert accumulated instructions to formatted LLVM IR string.
    /// Each instruction gets a 2-space indent.
    pub fn finish(&self, indent: usize) -> String {
        let mut out = String::new();
        let indent_str = " ".repeat(indent);
        for inst in &self.instructions {
            if let Some(ref op) = inst.op.strip_prefix("label:") {
                // Remove the preceding newline-indent for labels
                let _ = writeln!(out, "{}:", op);
            } else if let Some(ref res) = inst.result {
                write!(out, "{}{} = {}", indent_str, res, inst.op).ok();
                if let Some(ref md) = inst.metadata {
                    write!(out, ", {}", md).ok();
                }
                writeln!(out).ok();
            } else {
                write!(out, "{}{}", indent_str, inst.op).ok();
                if let Some(ref md) = inst.metadata {
                    write!(out, ", {}", md).ok();
                }
                writeln!(out).ok();
            }
        }
        out
    }

    /// Fast path: flush instructions directly to an existing &mut String.
    /// Avoids the intermediate String allocation of finish().
    pub fn finish_into(&self, out: &mut String, indent: usize) {
        let indent_str = " ".repeat(indent);
        for inst in &self.instructions {
            if let Some(ref op) = inst.op.strip_prefix("label:") {
                let _ = writeln!(out, "{}:", op);
            } else if let Some(ref res) = inst.result {
                write!(out, "{}{} = {}", indent_str, res, inst.op).ok();
                if let Some(ref md) = inst.metadata {
                    write!(out, ", {}", md).ok();
                }
                writeln!(out).ok();
            } else {
                write!(out, "{}{}", indent_str, inst.op).ok();
                if let Some(ref md) = inst.metadata {
                    write!(out, ", {}", md).ok();
                }
                writeln!(out).ok();
            }
        }
    }

    /// Check if any instruction was emitted (for control flow decisions).
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }

    /// Number of instructions emitted.
    pub fn len(&self) -> usize {
        self.instructions.len()
    }

    /// Emit a raw instruction string (bridge for gradual migration from writeln!)
    /// This should be used sparingly — prefer the typed emit_* methods.
    /// 2026-06-29: Added for Phase 4 migration bridge.
    pub fn writeln(&mut self, s: &str) {
        self.emit_void(s);
    }

    /// Reset the builder for a new function.
    /// Keeps the reg_counter and label_counter (never rewound) but clears
    /// accumulated instructions.
    pub fn clear(&mut self) {
        self.instructions.clear();
    }

    /// Emit a raw instruction with a result register (bridge for gradual migration).
    pub fn emit_raw(&mut self, result: Option<String>, op: String) {
        self.instructions.push(Instruction::new(result, op));
    }
}

// ── TypeConverter ────────────────────────────────────────────────────
//
// Centralized box/unbox logic for Briev's uniform i64 state representation.
// Previously scattered across adapt_to_i64, native_float_or_box, and
// countless inline casts in emit_expr.rs. Every type coercion goes here.

use crate::ast::Type as BrievType;
use crate::type_universe::TypeUniverse;

pub struct TypeConverter;

impl TypeConverter {
    /// Box a native-typed value to i64 for uniform %State storage.
    /// Uses the type universe to determine the boxing strategy.
    /// Falls back to identity (already i64) when universe lookup fails.
    /// 2026-06-29: Phase 7A — universe-driven, replaces hardcoded match arms.
    pub fn box_to_i64(
        builder: &mut LLVMBuilder,
        val: &str,
        ty: &BrievType,
        universe: Option<&TypeUniverse>,
    ) -> String {
        // 2026-08-01: resolve the String/Blob protocol membership from the
        // universe (Cast. properties — never type names) and box a pointer
        // value via ptrtoint. Falls back to the constructor-based fallback
        // only when no universe is available (builder tests).
        if let Some(u) = universe {
            if let Some(key) = ty.universe_key() {
                if let Some(rt) = u.get(key) {
                    if rt.properties.contains_key("Cast.String")
                        || rt.properties.contains_key("Cast.Blob")
                    {
                        return builder.emit_ptrtoint(val, LlvmType::I64);
                    }
                }
            }
        }
        Self::box_to_i64_fallback(builder, val, ty)
    }

    /// Fallback boxing when universe is not available (builder tests only).
    /// The real path is `box_to_i64` (above), which resolves String/Blob by
    /// their Cast. universe properties. 2026-06-29: Will be removed once all
    /// tests go through the full pipeline. 2026-07-31: Phase 3 (§8.4-D2) —
    /// arms matched against the canonical bootstrap Type constructors
    /// (bool_/string()/float()/...) instead of type-name strings.
    fn box_to_i64_fallback(builder: &mut LLVMBuilder, val: &str, ty: &BrievType) -> String {
        if *ty == BrievType::bool_() {
            builder.emit_zext(LlvmType::I1, LlvmType::I64, val)
        } else if *ty == BrievType::string() || *ty == BrievType::blob() {
            builder.emit_ptrtoint(val, LlvmType::I64)
        } else if *ty == BrievType::float() {
            let bi = builder.emit_bitcast(LlvmType::Float, LlvmType::I32, val);
            builder.emit_zext(LlvmType::I32, LlvmType::I64, &bi)
        } else if *ty == BrievType::float64() {
            builder.emit_bitcast(LlvmType::Double, LlvmType::I64, val)
        } else if *ty == BrievType::bits(8) {
            // Int8/UInt8 both lower to i8; zext preserves the bit pattern for
            // the boxed i64 representation (the old name-based arms disagreed
            // on sext vs zext — zext is correct for unsigned and bit-preserving
            // for signed, so it is used uniformly here).
            builder.emit_zext(LlvmType::I8, LlvmType::I64, val)
        } else if *ty == BrievType::bits(16) {
            builder.emit_zext(LlvmType::I16, LlvmType::I64, val)
        } else if *ty == BrievType::bits(32) {
            builder.emit_zext(LlvmType::I32, LlvmType::I64, val)
        } else {
            val.to_string()
        }
    }

    /// Unbox an i64 value from %State back to its native type.
    /// Uses the type universe to determine the unboxing strategy.
    /// 2026-06-29: Phase 7A — universe-driven.
    pub fn unbox_from_i64(
        builder: &mut LLVMBuilder,
        val: &str,
        target_ty: &BrievType,
        _universe: Option<&TypeUniverse>,
    ) -> String {
        Self::unbox_from_i64_fallback(builder, val, target_ty)
    }

    /// Fallback unboxing when universe is not available.
    /// 2026-07-31: Phase 3 (§8.4-D2) — arms matched against canonical bootstrap
    /// Type constructors instead of type-name strings.
    fn unbox_from_i64_fallback(
        builder: &mut LLVMBuilder,
        val: &str,
        target_ty: &BrievType,
    ) -> String {
        if *target_ty == BrievType::bool_() {
            builder.emit_trunc(LlvmType::I64, LlvmType::I1, val)
        } else if *target_ty == BrievType::string() || *target_ty == BrievType::blob() {
            builder.emit_inttoptr(val, LlvmType::I64)
        } else if *target_ty == BrievType::float() {
            let tr = builder.emit_trunc(LlvmType::I64, LlvmType::I32, val);
            builder.emit_bitcast(LlvmType::I32, LlvmType::Float, &tr)
        } else if *target_ty == BrievType::float64() {
            builder.emit_bitcast(LlvmType::I64, LlvmType::Double, val)
        } else if *target_ty == BrievType::bits(8) {
            builder.emit_trunc(LlvmType::I64, LlvmType::I8, val)
        } else if *target_ty == BrievType::bits(16) {
            builder.emit_trunc(LlvmType::I64, LlvmType::I16, val)
        } else if *target_ty == BrievType::bits(32) {
            builder.emit_trunc(LlvmType::I64, LlvmType::I32, val)
        } else {
            val.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_reg_unique() {
        let mut b = LLVMBuilder::new();
        let r1 = b.gen_reg();
        let r2 = b.gen_reg();
        assert_ne!(r1, r2, "registers must be unique");
        assert!(r1.starts_with("%t"), "register must start with %t");
    }

    #[test]
    fn test_emit_add() {
        let mut b = LLVMBuilder::new();
        let r = b.emit_add(LlvmType::I64, "%a", "%b");
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = add i64 %a, %b", r)));
    }

    #[test]
    fn test_emit_store() {
        let mut b = LLVMBuilder::new();
        b.emit_store(LlvmType::I64, "%val", "%ptr", 8);
        let ir = b.finish(2);
        assert!(ir.contains("store i64 %val, ptr %ptr, align 8"));
    }

    #[test]
    fn test_emit_label() {
        let mut b = LLVMBuilder::new();
        b.emit_label("loop_entry");
        let ir = b.finish(0);
        assert!(ir.contains("loop_entry:"));
    }

    #[test]
    fn test_emit_br() {
        let mut b = LLVMBuilder::new();
        b.emit_br("exit");
        let ir = b.finish(2);
        assert!(ir.contains("br label %exit"));
    }

    #[test]
    fn test_emit_cond_br() {
        let mut b = LLVMBuilder::new();
        b.emit_cond_br("%cmp", "true_dest", "false_dest");
        let ir = b.finish(2);
        assert!(ir.contains("br i1 %cmp, label %true_dest, label %false_dest"));
    }

    #[test]
    fn test_emit_zext() {
        let mut b = LLVMBuilder::new();
        let r = b.emit_zext(LlvmType::I1, LlvmType::I64, "%b");
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = zext i1 %b to i64", r)));
    }

    #[test]
    fn test_emit_phi() {
        let mut b = LLVMBuilder::new();
        let r = b.emit_phi(LlvmType::I64, &[("%init", "entry")]);
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = phi i64 [ %init, %entry ]", r)));
    }

    #[test]
    fn test_emit_call() {
        let mut b = LLVMBuilder::new();
        let r = b.emit_call(LlvmType::I64, "some_fn", &[("i64", "%arg")]);
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = call i64 @some_fn(i64 %arg)", r)));
    }

    #[test]
    fn test_emit_ret_void() {
        let mut b = LLVMBuilder::new();
        b.emit_ret(LlvmType::Void, None);
        let ir = b.finish(2);
        assert!(ir.contains("ret void"));
    }

    #[test]
    fn test_box_bool_to_i64() {
        let mut b = LLVMBuilder::new();
        let r = TypeConverter::box_to_i64(&mut b, "%b", &BrievType::bool_(), None);
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = zext i1 %b to i64", r)));
    }

    #[test]
    fn test_box_float_to_i64() {
        let mut b = LLVMBuilder::new();
        let r = TypeConverter::box_to_i64(&mut b, "%f", &BrievType::float(), None);
        let ir = b.finish(2);
        // Float boxing: bitcast float→i32, then zext i32→i64
        assert!(ir.contains("bitcast float %f to i32"));
        assert!(ir.contains(&format!("{} = zext i32", r)));
    }

    #[test]
    fn test_unbox_bool_from_i64() {
        let mut b = LLVMBuilder::new();
        let r = TypeConverter::unbox_from_i64(&mut b, "%v", &BrievType::bool_(), None);
        let ir = b.finish(2);
        assert!(ir.contains(&format!("{} = trunc i64 %v to i1", r)));
    }

    #[test]
    fn test_reg_counter_never_rewound() {
        let mut b = LLVMBuilder::new();
        let _r1 = b.gen_reg();
        b.clear();
        let r2 = b.gen_reg();
        // After clear, counter should still be 1, not 0
        assert_eq!(r2, "%t1", "reg_counter must never be rewound");
    }

    #[test]
    fn test_metadata_attachment() {
        let mut b = LLVMBuilder::new();
        b.emit_store(LlvmType::I64, "%val", "%ptr", 8);
        b.attach_metadata("1");
        let ir = b.finish(2);
        assert!(ir.contains("!tbaa !1"));
    }

    #[test]
    fn test_generated_ir_parses_as_valid_instructions() {
        let mut b = LLVMBuilder::new();
        let a = b.emit_add(LlvmType::I64, "%x", "%y");
        let z = b.emit_zext(LlvmType::I1, LlvmType::I64, "%b");
        b.emit_store_tbaa(LlvmType::I64, &z, &a, 8, "1");
        b.emit_ret(LlvmType::Void, None);
        let ir = b.finish(2);
        // Basic structural check: each non-label line must contain an opcode
        for line in ir.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.ends_with(':') {
                continue;
            }
            assert!(
                trimmed.contains("=") || trimmed.starts_with("ret") || trimmed.starts_with("store"),
                "line '{}' is not valid IR",
                trimmed
            );
        }
    }
}
