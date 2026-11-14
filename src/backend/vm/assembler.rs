// 2026-07-25: Rust assembler for the Briev VM .lair bytecode format.
// Converts a sequence of instruction emits into a complete .lair binary
// with header, string table, function table, and bytecode sections.
// Handles label resolution for forward/backward jumps.

// 2026-07-25: Phase 1b — most opcodes and emit methods are used by
// Phase 2 (VM backend emit_expr/emit_stmt). Allow dead_code until then.
#![allow(dead_code)]

use std::collections::HashMap;

// ── Opcodes (match interp.h values) ─────────────────────────────────────

pub const OP_NOP: u8 = 0x00;
pub const OP_DROP: u8 = 0x01;
pub const OP_DUP: u8 = 0x02;
pub const OP_SWAP: u8 = 0x03;
pub const OP_OVER: u8 = 0x04;
pub const OP_ROT: u8 = 0x05;
pub const OP_ADD: u8 = 0x06;
pub const OP_SUB: u8 = 0x07;
pub const OP_MUL: u8 = 0x08;
pub const OP_DIV_S: u8 = 0x09;
pub const OP_REM_S: u8 = 0x0A;
pub const OP_AND: u8 = 0x0B;
pub const OP_OR: u8 = 0x0C;
pub const OP_XOR: u8 = 0x0D;
pub const OP_NOT: u8 = 0x0E;
pub const OP_BNOT: u8 = 0x1C;
pub const OP_SHL: u8 = 0x0F;
pub const OP_SHR_S: u8 = 0x10;
pub const OP_EQ: u8 = 0x11;
pub const OP_NE: u8 = 0x12;
pub const OP_LT_S: u8 = 0x13;
pub const OP_LE_S: u8 = 0x14;
pub const OP_GT_S: u8 = 0x15;
pub const OP_GE_S: u8 = 0x16;
pub const OP_LOAD: u8 = 0x17;
pub const OP_STORE: u8 = 0x18;
pub const OP_RET: u8 = 0x19;
pub const OP_TRACE: u8 = 0x1A;
pub const OP_TRAP: u8 = 0x1B;

pub const OP_PUSH_I8: u8 = 0x30;
pub const OP_LOAD_LOCAL: u8 = 0x31;
pub const OP_STORE_LOCAL: u8 = 0x32;
pub const OP_PUSH_FRAME: u8 = 0x33;
pub const OP_POP_FRAME: u8 = 0x34;

pub const OP_PUSH_I16: u8 = 0x50;
pub const OP_JMP: u8 = 0x51;
pub const OP_JZ: u8 = 0x52;
pub const OP_JNZ: u8 = 0x53;
pub const OP_CALL: u8 = 0x54;

pub const OP_PUSH_I32: u8 = 0x70;
pub const OP_HCALL: u8 = 0x71;
pub const OP_CALL_PTR: u8 = 0x72;

pub const OP_PUSH_I64: u8 = 0x90;
pub const OP_ALLOC: u8 = 0x91;
pub const OP_LOAD_OFF: u8 = 0x92;
pub const OP_STORE_OFF: u8 = 0x93;

pub const OP_PUSH_STR: u8 = 0xB0;

// ── .lair constants ─────────────────────────────────────────────────────

const LAIR_HEADER_SIZE: usize = 96;

// ── Function entry (for internal tracking) ───────────────────────────────

#[derive(Debug, Clone)]
pub struct FunctionEntry {
    pub name: String,
    pub name_idx: u32,
    pub local_count: u16,
    pub arg_count: u16,
    pub bytecode_offset: usize,
    pub bytecode_len: usize,
}

#[derive(Debug, Clone)]
pub struct HostFunctionEntry {
    pub name: String,
    pub name_idx: u32,
    pub id: u32,
    /// 2026-08-23 (Plan 1 HCALL slice): argument count for the call — the
    /// interpreter pops exactly this many stack slots before dispatch.
    /// Without it the tamer cannot know where args end (the bytecode
    /// carries only the id).
    pub arity: u32,
}

/// Pending jump patch — the offset in the bytecode where a relative
/// offset needs to be written once the target label's position is known.
#[derive(Debug, Clone)]
struct PendingPatch {
    /// Offset in `bytes` where the 2-byte relative offset goes.
    /// Points to the byte immediately after the opcode.
    patch_offset: usize,
    /// Label name we're jumping to.
    label: String,
}

/// The assembler converts a sequence of instruction emits into a complete
/// .lair binary. It supports:
/// - Function definition with local/arg counts
/// - All VM opcodes
/// - Forward and backward label references for jumps
/// - String table for function names and string literals
/// - Host function table for FFI declarations
pub struct Assembler {
    /// Accumulated bytecode for the current function.
    bytes: Vec<u8>,
    /// Label positions within the current function's bytecode.
    labels: HashMap<String, usize>,
    /// Jumps that need patching after label positions are known.
    pending_patches: Vec<PendingPatch>,
    /// String table (collected across all functions).
    string_table: Vec<String>,
    /// Function table.
    functions: Vec<FunctionEntry>,
    /// Host function table.
    host_functions: Vec<HostFunctionEntry>,
    /// Index of the function currently being emitted.
    current_fn: usize,
    /// Offset of the current function's bytecode within the total bytecode.
    fn_bytecode_start: usize,
}

impl Assembler {
    pub fn new() -> Self {
        Assembler {
            bytes: Vec::new(),
            labels: HashMap::new(),
            pending_patches: Vec::new(),
            string_table: Vec::new(),
            functions: Vec::new(),
            host_functions: Vec::new(),
            current_fn: 0,
            fn_bytecode_start: 0,
        }
    }

    // ── String table helpers ─────────────────────────────────────────────

    /// Get or create a string table entry. Returns the index.
    fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(pos) = self.string_table.iter().position(|x| x == s) {
            return pos as u32;
        }
        self.string_table.push(s.to_string());
        (self.string_table.len() - 1) as u32
    }

    // ── Function management ──────────────────────────────────────────────

    /// Define a new function and make it the current emission target.
    /// Returns the function index.
    pub fn define_function(&mut self, name: &str, local_count: u16, arg_count: u16) -> usize {
        // Finalize previous function's bytecode tracking
        let fn_bytecode_len = self.bytes.len() - self.fn_bytecode_start;
        if let Some(last) = self.functions.last_mut() {
            last.bytecode_len = fn_bytecode_len;
        }

        let idx = self.functions.len();
        let name_idx = self.intern_string(name);
        self.fn_bytecode_start = self.bytes.len();
        self.labels.clear();
        self.pending_patches.clear();

        self.functions.push(FunctionEntry {
            name: name.to_string(),
            name_idx,
            local_count,
            arg_count,
            bytecode_offset: self.fn_bytecode_start,
            bytecode_len: 0, // updated when next function is defined
        });
        self.current_fn = idx;
        // Store name_idx for later use in assemble()
        idx
    }

    // ── Labels ───────────────────────────────────────────────────────────

    /// Mark the current position with a label name.
    pub fn define_label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.bytes.len());
    }

    // ── Host function registration ───────────────────────────────────────

    pub fn register_host_fn(&mut self, name: &str, id: u32) {
        self.register_host_fn_with_arity(name, id, 0);
    }

    /// 2026-08-23 (Plan 1 HCALL slice): registration with argument count.
    /// To undo: revert to `register_host_fn` + drop the arity field.
    pub fn register_host_fn_with_arity(&mut self, name: &str, id: u32, arity: u32) {
        let name_idx = self.intern_string(name);
        self.host_functions.push(HostFunctionEntry {
            name: name.to_string(),
            name_idx,
            id,
            arity,
        });
    }

    // ── Single-byte opcodes ──────────────────────────────────────────────

    fn emit_op(&mut self, op: u8) {
        self.bytes.push(op);
    }

    pub fn emit_nop(&mut self) {
        self.emit_op(OP_NOP);
    }
    pub fn emit_drop(&mut self) {
        self.emit_op(OP_DROP);
    }
    pub fn emit_dup(&mut self) {
        self.emit_op(OP_DUP);
    }
    pub fn emit_swap(&mut self) {
        self.emit_op(OP_SWAP);
    }
    pub fn emit_over(&mut self) {
        self.emit_op(OP_OVER);
    }
    pub fn emit_rot(&mut self) {
        self.emit_op(OP_ROT);
    }
    pub fn emit_add(&mut self) {
        self.emit_op(OP_ADD);
    }
    pub fn emit_sub(&mut self) {
        self.emit_op(OP_SUB);
    }
    pub fn emit_mul(&mut self) {
        self.emit_op(OP_MUL);
    }
    pub fn emit_div_s(&mut self) {
        self.emit_op(OP_DIV_S);
    }
    pub fn emit_rem_s(&mut self) {
        self.emit_op(OP_REM_S);
    }
    pub fn emit_and(&mut self) {
        self.emit_op(OP_AND);
    }
    pub fn emit_or(&mut self) {
        self.emit_op(OP_OR);
    }
    pub fn emit_xor(&mut self) {
        self.emit_op(OP_XOR);
    }
    pub fn emit_not(&mut self) {
        self.emit_op(OP_NOT);
    }
    pub fn emit_bnot(&mut self) {
        self.emit_op(OP_BNOT);
    }
    pub fn emit_shl(&mut self) {
        self.emit_op(OP_SHL);
    }
    pub fn emit_shr_s(&mut self) {
        self.emit_op(OP_SHR_S);
    }
    pub fn emit_eq(&mut self) {
        self.emit_op(OP_EQ);
    }
    pub fn emit_ne(&mut self) {
        self.emit_op(OP_NE);
    }
    pub fn emit_lt_s(&mut self) {
        self.emit_op(OP_LT_S);
    }
    pub fn emit_le_s(&mut self) {
        self.emit_op(OP_LE_S);
    }
    pub fn emit_gt_s(&mut self) {
        self.emit_op(OP_GT_S);
    }
    pub fn emit_ge_s(&mut self) {
        self.emit_op(OP_GE_S);
    }
    pub fn emit_load(&mut self) {
        self.emit_op(OP_LOAD);
    }
    pub fn emit_store(&mut self) {
        self.emit_op(OP_STORE);
    }
    pub fn emit_ret(&mut self) {
        self.emit_op(OP_RET);
    }
    pub fn emit_trace(&mut self) {
        self.emit_op(OP_TRACE);
    }
    pub fn emit_trap(&mut self) {
        self.emit_op(OP_TRAP);
    }
    pub fn emit_pop_frame(&mut self) {
        self.emit_op(OP_POP_FRAME);
    }

    // ── Opcodes with immediates ──────────────────────────────────────────

    pub fn emit_push_i8(&mut self, val: i8) {
        self.bytes.push(OP_PUSH_I8);
        self.bytes.push(val as u8);
    }

    pub fn emit_load_local(&mut self, idx: u8) {
        self.bytes.push(OP_LOAD_LOCAL);
        self.bytes.push(idx);
    }

    pub fn emit_store_local(&mut self, idx: u8) {
        self.bytes.push(OP_STORE_LOCAL);
        self.bytes.push(idx);
    }

    pub fn emit_push_frame(&mut self, slots: u8) {
        self.bytes.push(OP_PUSH_FRAME);
        self.bytes.push(slots);
    }

    pub fn emit_push_i16(&mut self, val: i16) {
        self.bytes.push(OP_PUSH_I16);
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    pub fn emit_jmp(&mut self, target: &str) {
        self.emit_jmp_op(OP_JMP, target);
    }

    pub fn emit_jz(&mut self, target: &str) {
        self.emit_jmp_op(OP_JZ, target);
    }

    pub fn emit_jnz(&mut self, target: &str) {
        self.emit_jmp_op(OP_JNZ, target);
    }

    fn emit_jmp_op(&mut self, op: u8, target: &str) {
        let patch_offset = self.bytes.len() + 1; // after opcode
        self.bytes.push(op);
        // Reserve 2 bytes for the relative offset (placeholder)
        self.bytes.extend_from_slice(&[0u8; 2]);
        self.pending_patches.push(PendingPatch {
            patch_offset,
            label: target.to_string(),
        });
    }

    pub fn emit_call(&mut self, fn_idx: u16) {
        self.bytes.push(OP_CALL);
        self.bytes.extend_from_slice(&fn_idx.to_le_bytes());
    }

    pub fn emit_push_i32(&mut self, val: i32) {
        self.bytes.push(OP_PUSH_I32);
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    pub fn emit_hcall(&mut self, fn_id: u32) {
        self.bytes.push(OP_HCALL);
        self.bytes.extend_from_slice(&fn_id.to_le_bytes());
    }

    pub fn emit_push_i64(&mut self, val: i64) {
        self.bytes.push(OP_PUSH_I64);
        self.bytes.extend_from_slice(&val.to_le_bytes());
    }

    pub fn emit_alloc(&mut self, size: u64) {
        self.bytes.push(OP_ALLOC);
        self.bytes.extend_from_slice(&size.to_le_bytes());
    }

    pub fn emit_load_off(&mut self, offset: i64) {
        self.bytes.push(OP_LOAD_OFF);
        self.bytes.extend_from_slice(&offset.to_le_bytes());
    }

    pub fn emit_store_off(&mut self, offset: i64) {
        self.bytes.push(OP_STORE_OFF);
        self.bytes.extend_from_slice(&offset.to_le_bytes());
    }

    pub fn emit_push_str(&mut self, table_idx: u16) {
        self.bytes.push(OP_PUSH_STR);
        self.bytes.extend_from_slice(&table_idx.to_le_bytes());
    }

    // ── Final assembly ───────────────────────────────────────────────────

    /// Resolve all pending label patches and produce the final .lair binary.
    /// Must be called after all functions have been emitted.
    pub fn assemble(&mut self) -> Vec<u8> {
        // Finalize last function's bytecode tracking
        let fn_bytecode_len = self.bytes.len() - self.fn_bytecode_start;
        if let Some(last) = self.functions.last_mut() {
            last.bytecode_len = fn_bytecode_len;
        }

        // Resolve pending patches
        for patch in &self.pending_patches {
            let target_offset = match self.labels.get(&patch.label) {
                Some(offset) => *offset,
                None => {
                    // Label not found — emit error marker and continue
                    // The caller should validate labels; for MVP, treat as 0.
                    eprintln!("[assembler] warning: unresolved label '{}'", patch.label);
                    0
                }
            };
            // Calculate relative offset from after the 2-byte immediate
            let from_offset = patch.patch_offset + 2;
            let rel_offset = target_offset as i64 - from_offset as i64;
            let rel_i16 = rel_offset.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            self.bytes[patch.patch_offset..patch.patch_offset + 2]
                .copy_from_slice(&rel_i16.to_le_bytes());
        }

        // Build the .lair binary
        let total_bc_len = self.bytes.len();

        // String table data: null-terminated strings concatenated
        let mut str_data = Vec::new();
        let mut str_offsets: Vec<u32> = Vec::new();
        for s in &self.string_table {
            str_offsets.push(str_data.len() as u32);
            str_data.extend_from_slice(s.as_bytes());
            str_data.push(0); // null terminator
        }
        let str_size = str_data.len();

        // Function table data
        let mut fn_data = Vec::new();
        for f in &self.functions {
            // 2026-07-30: name_idx is the string table INDEX; convert to byte OFFSET
            // for the .lair format which expects offset into null-terminated string block.
            let name_offset = *str_offsets.get(f.name_idx as usize).unwrap_or(&0);
            fn_data.extend_from_slice(&name_offset.to_le_bytes()); // name_idx (u32)
            fn_data.extend_from_slice(&(f.bytecode_offset as u64).to_le_bytes()); // bc_off (u64)
            fn_data.extend_from_slice(&(f.bytecode_len as u32).to_le_bytes()); // bc_len (u32)
            fn_data.extend_from_slice(&f.local_count.to_le_bytes()); // local_count (u16)
            fn_data.extend_from_slice(&f.arg_count.to_le_bytes()); // arg_count (u16)
        }
        let fn_size = fn_data.len();

        // Host function table data
        let mut host_data = Vec::new();
        for h in &self.host_functions {
            let name_offset = *str_offsets.get(h.name_idx as usize).unwrap_or(&0);
            host_data.extend_from_slice(&name_offset.to_le_bytes()); // name_idx (u32)
            host_data.extend_from_slice(&h.id.to_le_bytes()); // host_fn_id (u32)
            // 2026-08-23 (Plan 1 HCALL slice): arity rides in the table so
            // the tamer knows how many slots to pop. 12-byte stride.
            host_data.extend_from_slice(&h.arity.to_le_bytes()); // arity (u32)
        }
        let host_size = host_data.len();

        // Calculate section offsets
        let str_off = LAIR_HEADER_SIZE;
        let fn_off = str_off + str_size;
        let bc_off = fn_off + fn_size;
        let host_off = bc_off + total_bc_len;

        // Build header
        let mut output = Vec::with_capacity(host_off + host_size);

        // Magic: "LAIR"
        output.extend_from_slice(b"LAIR");
        // Version (u32 LE)
        output.extend_from_slice(&1u32.to_le_bytes());
        // Endianness (u8 LE = 1)
        output.push(1);
        // Reserved (3 bytes)
        output.extend_from_slice(&[0u8; 3]);
        // Flags (u32 LE)
        output.extend_from_slice(&0u32.to_le_bytes());
        // Section table entries
        output.extend_from_slice(&(str_off as u64).to_le_bytes());
        output.extend_from_slice(&(str_size as u64).to_le_bytes());
        output.extend_from_slice(&(fn_off as u64).to_le_bytes());
        output.extend_from_slice(&(fn_size as u64).to_le_bytes());
        output.extend_from_slice(&(bc_off as u64).to_le_bytes());
        output.extend_from_slice(&(total_bc_len as u64).to_le_bytes());
        output.extend_from_slice(&(host_off as u64).to_le_bytes());
        output.extend_from_slice(&(host_size as u64).to_le_bytes());
        // Inverse table offset/size (zero — no obfuscation in raw .lair)
        output.extend_from_slice(&0u64.to_le_bytes());
        output.extend_from_slice(&0u64.to_le_bytes());

        // Sections
        output.extend_from_slice(&str_data);
        output.extend_from_slice(&fn_data);
        output.extend_from_slice(&self.bytes);
        output.extend_from_slice(&host_data);

        output
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u16(buf: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([buf[offset], buf[offset + 1]])
    }

    fn read_u32(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    fn read_u64(buf: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ])
    }

    #[test]
    fn test_assemble_simple_add() {
        let mut asm = Assembler::new();
        asm.define_function("main", 0, 0);
        asm.emit_push_i64(3);
        asm.emit_push_i64(4);
        asm.emit_add();
        asm.emit_ret();

        let lair = asm.assemble();

        // Verify header
        assert_eq!(&lair[0..4], b"LAIR");
        assert_eq!(read_u32(&lair, 4), 1); // version
        assert_eq!(lair[8], 1); // endianness LE
    }

    #[test]
    fn test_assemble_with_labels() {
        let mut asm = Assembler::new();
        asm.define_function("test", 1, 0);
        // if (false) { 1 } else { 2 }
        asm.emit_push_i8(0); // false
        asm.emit_jz("else");
        asm.emit_push_i64(1);
        asm.emit_jmp("end");
        asm.define_label("else");
        asm.emit_push_i64(2);
        asm.define_label("end");
        asm.emit_ret();

        let lair = asm.assemble();

        // Should be valid .lair
        assert_eq!(&lair[0..4], b"LAIR");
        // Size should be reasonable
        assert!(lair.len() > LAIR_HEADER_SIZE);
    }

    #[test]
    fn test_host_function_registration() {
        let mut asm = Assembler::new();
        asm.register_host_fn("test_fn", 42);
        asm.define_function("main", 0, 0);
        asm.emit_push_i64(10);
        asm.emit_hcall(42);
        asm.emit_ret();

        let lair = asm.assemble();

        assert_eq!(&lair[0..4], b"LAIR");

        // Host function table should be at the expected offset
        let host_off = read_u64(&lair, 64) as usize;
        let host_sz = read_u64(&lair, 72) as usize;
        assert!(host_off > 0);
        assert!(host_sz > 0);
    }

    #[test]
    fn test_multiple_functions() {
        let mut asm = Assembler::new();
        asm.define_function("add", 2, 2);
        asm.emit_load_local(0);
        asm.emit_load_local(1);
        asm.emit_add();
        asm.emit_ret();

        asm.define_function("main", 0, 0);
        asm.emit_push_i64(3);
        asm.emit_push_i64(4);
        asm.emit_call(0); // call add
        asm.emit_ret();

        let lair = asm.assemble();

        assert_eq!(&lair[0..4], b"LAIR");

        // Should have 2 functions
        let fn_off = read_u64(&lair, 32) as usize;
        let fn_sz = read_u64(&lair, 40) as usize;
        assert_eq!(fn_sz, 2 * (4 + 8 + 4 + 2 + 2)); // 2 * 20 bytes per entry
    }
}
