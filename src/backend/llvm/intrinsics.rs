// ── Intrinsic Call Expression Codegen ─────────────────────────────────
// 2026-07-14: Config-driven operation dispatch. Uses config/llvm-ops.toml
// for generic operations (Add#, Eq#, etc.) and special-case helpers for
// memory/I/O intrinsics (Malloc#, Print#, etc.) that don't fit templates.
// Flat code: max 2 nesting depth.

use std::sync::LazyLock;
use crate::ast::{Expr, Type};
use crate::backend::llvm::{AllocStrategy, LlvmBackend, TypedRegister as BTypedRegister};
use crate::config::AllocConfig;
use std::fmt::Write;

pub(crate) static ALLOC_CONFIG: LazyLock<AllocConfig> = LazyLock::new(|| AllocConfig::load());
// 2026-09-10 (Family F, Asm#): abstract-asm lowering table.
pub(crate) static ASM_LOWERING: LazyLock<crate::config::AsmLowering> =
    LazyLock::new(|| crate::config::AsmLowering::load());

/// Emit an intrinsic call by name. For generic operations (Add#, Eq#, etc.)
/// looks up the IR template from config/llvm-ops.toml using (op, primitive, bytes)
/// of the first argument. For memory/I/O intrinsics, uses special-case helpers.
pub fn emit_intrinsic_call(
    backend: &mut LlvmBackend,
    out: &mut String,
    v: &str,
    name: &str,
    args: &[Expr],
    analysis_id: Option<usize>,
    indent: &str,
) -> BTypedRegister {
    // Special-case intrinsics that don't fit the template pattern
    match name {
        "Malloc#" => return emit_malloc(backend, out, v, args, indent),
        // 2026-08-12 (Iterable protocol): the UTF8 CHAR count of a String —
        // the scan (a computed property, so an intrinsic; `.^Length` is the
        // stored byte count).
        // 2026-08-12 (Iterable protocol): the UTF8 CHAR count of a String — the
    // scan (a computed property, so an intrinsic; `.^Length` is the stored
    // byte count).
    "CharCount#" => return emit_char_count(backend, out, v, args, indent),
        "Alloc#" => return emit_alloc(backend, out, v, args, indent, analysis_id),
        "Free#" => return emit_free(backend, out, v, args, indent),
        "Now#" => {
            // 2026-08-01 (D2): `Now#` — monotonic clock in ns for the
            // watchdog `within N ms` deadline compare.
            writeln!(out, "{}{} = call i64 @__briev_now(%state)", indent, v).ok();
            let narrowed = narrow_int_result(backend, out, v, indent);
            return BTypedRegister { name: narrowed, ty: Type::int() };
        }
        // 2026-08-27 (Slice C): typed volatile MMIO access — width from the
        // Ptr<T> element type; raw-address access stays with Load#/Store#.
        "VolatileLoad#" => return emit_volatile_load(backend, out, v, args, indent),
        "VolatileStore#" => return emit_volatile_store(backend, out, v, args, indent),
        "Load#" => return emit_load(backend, out, v, args, indent),
        // 2026-08-23 (gpu.bv): workgroup barrier — CPU lowering is a no-op
        // returning true (single thread trivially reaches the barrier).
        // The SPIR-V backend maps this to OpControlBarrier.
        "Barrier#" => {
            writeln!(out, "{}{} = add i64 0, 1", indent, v).ok();
            let narrowed = narrow_int_result(backend, out, v, indent);
            return BTypedRegister { name: narrowed, ty: Type::int() };
        }
        "Store#" => return emit_store(backend, out, v, args, indent),
        "Copy#" => return emit_copy(backend, out, v, args, indent),
        "Fill#" => return emit_fill(backend, out, v, args, indent),
        // 2026-08-15 (coll plan §3.6): the capacity intrinsics — compiler-owned
        // capacity control on a coll handle (`[data, cap, len]`). "without
        // needing to set a property": the hidden cap slot is read/written
        // through these, never a declared field.
        "Capacity#" => return emit_capacity(backend, out, v, args, indent),
        "Resize#" => return emit_resize(backend, out, v, args, indent),
        "EnsureCap#" => return emit_ensure_cap(backend, out, v, args, indent),
        "TrimCap#" => return emit_trim_cap(backend, out, v, args, indent),

        "GetEnv#" => return emit_get_env(backend, out, v, args, indent),
        "GetEnvInt#" => return emit_get_env_int(backend, out, v, args, indent),
        // 2026-08-23 (process.bv revival): process/environment intrinsics.
        // String returns pack as { i64 len, i64 data-ptr } via the same
        // pattern as emit_get_env (helpers return malloc'd C strings).
        "Spawn#" => {
            let cmd = backend.emit_expr(out, &args[0], indent);
            let cmd_ptr = backend.string_ptr(out, indent, &cmd);
            let r = backend.fun.gen_reg();
            writeln!(out, "{}{} = call i64 @__briev_spawn(ptr {})", indent, r, cmd_ptr).ok();
            return BTypedRegister { name: r, ty: Type::int() };
        }
        "SpawnWithOutput#" => {
            let cmd = backend.emit_expr(out, &args[0], indent);
            let cmd_ptr = backend.string_ptr(out, indent, &cmd);
            let cstr = backend.fun.gen_reg();
            writeln!(out, "{}{} = call ptr @__briev_spawn_output(ptr {})", indent, cstr, cmd_ptr).ok();
            let is_null = backend.fun.gen_reg();
            writeln!(out, "{}{} = icmp eq ptr {}, null", indent, is_null, cstr).ok();
            let fb = backend.fun.gen_reg();
            writeln!(out, "{}{} = alloca i8, i64 1", indent, fb).ok();
            writeln!(out, "{}store i8 0, ptr {}", indent, fb).ok();
            let safe_ptr = backend.fun.gen_reg();
            writeln!(out, "{}{} = select i1 {}, ptr {}, ptr {}", indent, safe_ptr, is_null, fb, cstr).ok();
            let len = backend.fun.gen_reg();
            // 2026-09-10 (Family F): cstr_len is a pure-Briev defn (cast_lanes)
            // — removes the last libc strlen reference from GetCwd#.
            if backend.ctx.defn_params.contains_key("cstr_len") {
                writeln!(out, "{}{} = call i64 @cstr_len(ptr %state, ptr {})", indent, len, safe_ptr).ok();
            } else {
                writeln!(out, "{}{} = call i64 @strlen(ptr {})", indent, len, safe_ptr).ok();
            }
            let data_raw = backend.fun.gen_reg();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, data_raw, safe_ptr).ok();
            let data = backend.fun.gen_reg();
            writeln!(out, "{}{} = select i1 {}, i64 0, i64 {}", indent, data, is_null, data_raw).ok();
            let t1 = backend.fun.gen_reg();
            writeln!(out, "{}{} = insertvalue {{ i64, i64 }} undef, i64 {}, 0", indent, t1, data).ok();
            let t2 = backend.fun.gen_reg();
            writeln!(out, "{}{} = insertvalue {{ i64, i64 }} %{}, i64 {}, 1", indent, t2, t1, len).ok();
            return BTypedRegister { name: t2, ty: Type::string() };
        }
        "SetEnv#" => {
            let k = backend.emit_expr(out, &args[0], indent);
            let val = backend.emit_expr(out, &args[1], indent);
            let kptr = backend.string_ptr(out, indent, &k);
            let vptr = backend.string_ptr(out, indent, &val);
            let r = backend.fun.gen_reg();
            writeln!(out, "{}{} = call i64 @__briev_setenv(ptr {}, ptr {})", indent, r, kptr, vptr).ok();
            return BTypedRegister { name: r, ty: Type::int() };
        }
        "GetCwd#" => {
            let cstr = backend.fun.gen_reg();
            if backend.ctx.defn_params.contains_key("__briev_getcwd") {
                let cwd_i = backend.fun.gen_reg();
                writeln!(out, "{}{} = call i64 @__briev_getcwd(ptr %state)", indent, cwd_i).ok();
                writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, cstr, cwd_i).ok();
            } else {
                writeln!(out, "{}{} = call ptr @__briev_getcwd()", indent, cstr).ok();
            }
            let is_null = backend.fun.gen_reg();
            writeln!(out, "{}{} = icmp eq ptr {}, null", indent, is_null, cstr).ok();
            let fb = backend.fun.gen_reg();
            writeln!(out, "{}{} = alloca i8, i64 1", indent, fb).ok();
            writeln!(out, "{}store i8 0, ptr {}", indent, fb).ok();
            let safe_ptr = backend.fun.gen_reg();
            writeln!(out, "{}{} = select i1 {}, ptr {}, ptr {}", indent, safe_ptr, is_null, fb, cstr).ok();
            let len = backend.fun.gen_reg();
            writeln!(out, "{}{} = call i64 @strlen(ptr {})", indent, len, safe_ptr).ok();
            let data_raw = backend.fun.gen_reg();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, data_raw, safe_ptr).ok();
            let data = backend.fun.gen_reg();
            writeln!(out, "{}{} = select i1 {}, i64 0, i64 {}", indent, data, is_null, data_raw).ok();
            let t1 = backend.fun.gen_reg();
            writeln!(out, "{}{} = insertvalue {{ i64, i64 }} undef, i64 {}, 0", indent, t1, data).ok();
            let t2 = backend.fun.gen_reg();
            writeln!(out, "{}{} = insertvalue {{ i64, i64 }} %{}, i64 {}, 1", indent, t2, t1, len).ok();
            return BTypedRegister { name: t2, ty: Type::string() };
        }
        "ChDir#" => {
            let pth = backend.emit_expr(out, &args[0], indent);
            let pptr = backend.string_ptr(out, indent, &pth);
            let r = backend.fun.gen_reg();
            // 2026-09-10 (Family I): the Briev defn declares the path as
            // Int — ptrtoint so the static arg types match (LTO hazard).
            let pi = backend.fun.gen_reg();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, pi, pptr).ok();
            writeln!(out, "{}{} = call i64 @__briev_chdir({}i64 {})", indent, r,
                if backend.ctx.defn_params.contains_key("__briev_chdir") { "ptr %state, " } else { "" }, pi).ok();
            return BTypedRegister { name: r, ty: Type::int() };
        }
        // 2026-08-03: call a function-pointer value (host callback).
        "CallPtr#" => return emit_call_ptr(backend, out, v, args, indent),
        // 2026-09-10 (Family F): Asm# - two-mode asm escape hatch.
        "Asm#" => return emit_asm(backend, out, v, args, indent),
        // 2026-09-10 (task machine migration): segment dispatch - threads
        // the machine defn's hidden %state into the C-flat segment fn,
        // whose body may call state-taking runtime defns (Print# etc.).
        // The machine passes i64 addresses; widen to the ptr ABI.
        "TaskCall#" => {
            let seg = emit_arg(backend, out, &args[0], indent);
            let argv = emit_arg(backend, out, &args[1], indent);
            let outc = emit_arg(backend, out, &args[2], indent);
            let seg_p = backend.fun.gen_reg();
            let argv_p = backend.fun.gen_reg();
            let outc_p = backend.fun.gen_reg();
            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, seg_p, seg).ok();
            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, argv_p, argv).ok();
            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, outc_p, outc).ok();
            writeln!(out, "{}{} = call i64 {}(ptr %state, ptr {}, ptr {})",
                indent, v, seg_p, argv_p, outc_p).ok();
            return BTypedRegister { name: v.to_string(), ty: Type::int() };
        }
        // 2026-08-03: host cancellation flag (process-global atomic).
        "CancelRequested#" => return emit_cancel_requested(backend, out, v, indent),
        "ClearCancel#" => {
            writeln!(out, "{}store atomic i32 0, ptr @__briev_cancel_flag seq_cst, align 4", indent).ok();
            return BTypedRegister { name: v.to_string(), ty: Type::void() };
        }
        "GetGlobalId#" => return emit_get_global_id(backend, out, v, args, indent),
        "GetGlobalSize#" => return emit_external_call(backend, out, v, name, args, indent),
        "GetLocalId#" => return emit_external_call(backend, out, v, name, args, indent),
        "AddressOf#" => return emit_address_of(backend, out, v, args, indent),
        "SysCall#" => return emit_syscall(backend, out, v, args, indent),
        "SysConf#" => return emit_sysconf(backend, out, v, args, indent),
        "Len#" | "Length#" => return emit_len(backend, out, v, args, indent),
        "Concat#" => return emit_external_call(backend, out, v, name, args, indent),
        "Length#" => return emit_external_call(backend, out, v, name, args, indent),
        "Get#" => return emit_external_call(backend, out, v, name, args, indent),
        "Insert#" => return emit_external_call(backend, out, v, name, args, indent),
        // 2026-07-15: Atomic operations (LLVM atomic instructions)
        "AtomicLoad#" => return emit_atomic_load(backend, out, v, args, indent),
        "AtomicStore#" => return emit_atomic_store(backend, out, v, args, indent),
        "AtomicCas#" => return emit_atomic_cas(backend, out, v, args, indent),
        "AtomicXchg#" => return emit_atomic_xchg(backend, out, v, args, indent),
        "AtomicAdd#" => return emit_atomic_add(backend, out, v, args, indent),
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): RMW family
        "AtomicSub#" => return emit_atomic_rmw(backend, out, v, args, indent, "sub"),
        "AtomicOr#" => return emit_atomic_rmw(backend, out, v, args, indent, "or"),
        "AtomicAnd#" => return emit_atomic_rmw(backend, out, v, args, indent, "and"),
        "AtomicXor#" => return emit_atomic_rmw(backend, out, v, args, indent, "xor"),
        "AtomicLoadN#" => return emit_atomic_load_n(backend, out, v, args, indent),
        "AtomicStoreN#" => return emit_atomic_store_n(backend, out, v, args, indent),
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): portable SIMD
        "SimdAdd#" => return emit_simd_binary(backend, out, v, args, indent, SimdKind::Add),
        "SimdSub#" => return emit_simd_binary(backend, out, v, args, indent, SimdKind::Sub),
        "SimdMul#" => return emit_simd_binary(backend, out, v, args, indent, SimdKind::Mul),
        "SimdFma#" => return emit_simd_binary(backend, out, v, args, indent, SimdKind::Fma),
        "Fence#" => return emit_fence(backend, out, v, args, indent),
        // 2026-07-15: Dynamic linker intrinsics
        "DlOpen#" => return emit_dl_open(backend, out, v, args, indent),
        "DlSym#" => return emit_dl_sym(backend, out, v, args, indent),
        "DlClose#" => return emit_dl_close(backend, out, v, args, indent),
        // 2026-07-15: Debugging intrinsics
        "Backtrace#" => return emit_backtrace(backend, out, v, args, indent),
        // 2026-08-01 (audit): one generic `Print#` — dispatch the emission by
        // the argument's protocol category. The four special-cased print
        // intrinsics collapsed into this single type-dispatched intrinsic.
        "Print#" => return emit_intrinsic_print(backend, out, v, args, indent),
        // 2026-07-18: Pointer operations — special-case because they need
        // type-dependent codegen (Deref# needs pointee type, Index# needs
        // element type, Cast# needs target type). Ptr# is a simple inttoptr.
        "Deref#" => return emit_intrinsic_deref(backend, out, v, args, indent),
        "Index#" => return emit_intrinsic_index(backend, out, v, args, indent),
        "Cast#" => return emit_intrinsic_cast(backend, out, v, args, indent),
        "Ptr#" => {
            let a = backend.emit_expr(out, &args[0], indent);
            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, v, a.name).ok();
            return BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::int()) };
        }
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): pointer arithmetic
        "PtrAdd#" => return emit_ptr_add(backend, out, v, args, indent),
        "PtrSub#" => return emit_ptr_sub(backend, out, v, args, indent),
        "PtrDiff#" => return emit_ptr_diff(backend, out, v, args, indent),
        "PtrEq#" => return emit_ptr_eq(backend, out, v, args, indent),
        "PtrLt#" => return emit_ptr_lt(backend, out, v, args, indent),
        _ => {}
    }

    // For generic operations, look up template from config
    let arg_regs: Vec<BTypedRegister> = args.iter()
        .map(|a| backend.emit_expr(out, a, indent))
        .collect();

    if arg_regs.is_empty() {
        return BTypedRegister { name: v.to_string(), ty: Type::void() };
    }

    // Determine llvm type and bytes from the first argument's type
    // 2026-07-20: Hashword protocol — reads llvm_type from universe.
    let llvm_ty = backend.llvm_type(&arg_regs[0].ty);
    let bytes = resolve_arg_bytes(backend, &arg_regs[0]).unwrap_or(8);

    let op_name = name.trim_end_matches('#');
    // 2026-07-17: Directly emit float intrinsics (Sqrt#, Sin#, Cos#, etc.)
    let is_float_unary = matches!(op_name, "Sqrt" | "Sin" | "Cos" | "Fabs" | "Ceil" | "Floor" | "Exp");
    if is_float_unary {
        let llvm_name = op_name.to_lowercase();
        let (float_suffix, float_llvm_ty, ret_ty) = match llvm_ty.as_str() {
            "double" => ("f64", "double", Type::float64()),
            _ => ("f32", "float", Type::float()),
        };
        writeln!(out, "{}{} = call {} @llvm.{}.{}({} {})",
            indent, v, float_llvm_ty, llvm_name, float_suffix, float_llvm_ty, arg_regs[0].name).ok();
        return BTypedRegister { name: v.to_string(), ty: ret_ty };
    }

    // 2026-07-20: Simple hardcoded template dispatch for standard ops.
    // Replaces the old TOML config lookup. Phase 3 will replace this with
    // proper hashword category dispatch from op signatures.
    if let Some(template) = template_for_op(op_name, &llvm_ty, bytes) {
        let ir = template
            .replace("%v", v)
            .replace("%a", &arg_regs.get(0).map(|r| r.name.clone()).unwrap_or_default())
            .replace("%b", &arg_regs.get(1).map(|r| r.name.clone()).unwrap_or_default())
            .replace("%c", &arg_regs.get(2).map(|r| r.name.clone()).unwrap_or_default());
        writeln!(out, "{}  {}", indent, ir).ok();
        let ret_ty = if arg_regs.len() >= 1 {
            if arg_regs[0].ty == Type::float64() { Type::float64() }
            else if arg_regs[0].ty == Type::float() { Type::float() }
            else { Type::int() }
        } else { Type::int() };
        return BTypedRegister { name: v.to_string(), ty: ret_ty };
    }

    // Fallback: generative op-identity dispatch (2026-08-14, UOL §6b.2 step 3).
    // `OpName#` for ANY disclosed operation identity → dispatch to the op
    // member on arg[0]. This is how `At#(c, i)`, `Count#(c)`, `InsertAt#(c, x)`,
    // `Iter#(c)`, etc. work uniformly with the arithmetic `Op#` forms. The
    // identity set mirrors `operation_identities` in src/vocab.rs; `String`
    // has no `op Count`, so `Count#` on it routes to the char scan
    // (`CharCount#`). A name in the set but not declared on the receiver
    // reaches emit_method_call, which reports the missing member.
    if is_operation_identity(op_name) {
        if op_name == "Count" && backend.is_string_operand(&arg_regs[0].ty) {
            // `Count#` on a String operand = its CHAR count (the element
            // count of Iterable<Char>), not a declared `op Count`.
            let p = backend.string_ptr(out, indent, &arg_regs[0]);
            writeln!(out, "{}{} = call i64 @briev_char_len({}ptr {})", indent, v,
            if backend.ctx.defn_params.contains_key("briev_char_len") { "ptr %state, " } else { "" }, p).ok();
            return BTypedRegister { name: v.to_string(), ty: Type::int() };
        }
        if !args.is_empty() {
            let recv = &args[0];
            let rest: Vec<Expr> = args.iter().skip(1).cloned().collect();
            let out_tmp = backend.fun.gen_reg();
            return backend.emit_method_call(out, &out_tmp, recv, op_name, &rest, &[], indent);
        }
    }

    // Fallback: emit as external call
    emit_external_call(backend, out, v, name, args, indent)
}

/// 2026-08-14 (UOL §6b.1): the disclosed operation identities — the intrinsic
/// forms (`OpName#`) of every operation. Mirrors `operation_identities` in
/// `src/vocab.rs`; kept as a runtime const here so codegen needs no vocab
/// dependency. The arithmetic set (`Add`..`Shr`) is covered by registered
/// signatures + `template_for_op`; the collection/cursor set below is what
/// the generative dispatch reaches.
fn is_operation_identity(name: &str) -> bool {
    matches!(name,
        "Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg" | "Abs"
        | "Eq" | "Neq" | "Lt" | "Le" | "Gt" | "Ge"
        | "And" | "Or" | "Not"
        | "BitAnd" | "BitOr" | "BitXor" | "BitNot" | "Shl" | "Shr"
        | "At" | "Slice" | "InsertAt" | "ExtractFrom" | "CopyFrom"
        | "Append" | "Prepend"
        | "Count" | "Iter" | "Step" | "IsEnd" | "Current")
}

/// 2026-07-20: Simple IR template dispatch, replacing the old TOML config lookup.
/// Produces the same IR templates that config/llvm-ops.toml provided, but
/// driven by the type's llvm_type rather than CTD metadata.
/// Phase 3 will replace this with proper hashword category dispatch.
pub(crate) fn template_for_op(op_name: &str, llvm_ty: &str, bytes: u64) -> Option<String> {
    let is_float = matches!(llvm_ty, "float" | "double" | "half" | "bfloat" | "fp128");
    // 2026-09-02 (plan fundamental-parent-membership): each float width
    // templates at its own spelling — half folded to float before (invalid
    // IR on half registers); half/bfloat now reach this path since Float16
    // state slots resolve natively. Undo: restore the half/bfloat → float
    // fold.
    let float_llvm = match llvm_ty {
        "half" => "half",
        "bfloat" => "bfloat",
        "float" => "float",
        "double" => "double",
        _ if bytes <= 4 => "float",
        _ => "double",
    };
    // 2026-07-25: Use the passed llvm_ty for integer ops (may be narrowed
    // from value-range inference), falling back to bytes*8 if llvm_ty
    // doesn't look like an integer type (e.g., "ptr", "float").
    let int_llvm = if llvm_ty.starts_with('i') { llvm_ty.to_string() } else { format!("i{}", bytes * 8) };

    match (op_name, is_float) {
        ("Add", true) => Some(format!("%v = fadd fast {} %a, %b", float_llvm)),
        ("Sub", true) => Some(format!("%v = fsub fast {} %a, %b", float_llvm)),
        ("Mul", true) => Some(format!("%v = fmul fast {} %a, %b", float_llvm)),
        ("Div", true) => Some(format!("%v = fdiv fast {} %a, %b", float_llvm)),
        ("Rem", true) => Some(format!("%v = frem fast {} %a, %b", float_llvm)),
        ("Eq", true) => Some(format!("%v = fcmp oeq {} %a, %b", float_llvm)),
        ("Neq", true) => Some(format!("%v = fcmp une {} %a, %b", float_llvm)),
        ("Lt", true) => Some(format!("%v = fcmp olt {} %a, %b", float_llvm)),
        ("Gt", true) => Some(format!("%v = fcmp ogt {} %a, %b", float_llvm)),
        ("Le", true) => Some(format!("%v = fcmp ole {} %a, %b", float_llvm)),
        ("Ge", true) => Some(format!("%v = fcmp oge {} %a, %b", float_llvm)),
        ("Neg", true) => Some(format!("%v = fneg fast {} %a", float_llvm)),
        ("Abs", true) => Some(format!("%v = call {} @llvm.fabs.{}({} %a)", float_llvm, float_llvm, float_llvm)),

        ("Add", false) => Some(format!("%v = add nsw {} %a, %b", int_llvm)),
        ("Sub", false) => Some(format!("%v = sub nsw {} %a, %b", int_llvm)),
        ("Mul", false) => Some(format!("%v = mul nsw {} %a, %b", int_llvm)),
        ("Div", false) => Some(format!("%v = sdiv {} %a, %b", int_llvm)),
        ("Rem", false) => Some(format!("%v = srem {} %a, %b", int_llvm)),
        ("Eq", false) => Some(format!("%v = icmp eq {} %a, %b", int_llvm)),
        ("Neq", false) => Some(format!("%v = icmp ne {} %a, %b", int_llvm)),
        ("Lt", false) => Some(format!("%v = icmp slt {} %a, %b", int_llvm)),
        ("Gt", false) => Some(format!("%v = icmp sgt {} %a, %b", int_llvm)),
        ("Le", false) => Some(format!("%v = icmp sle {} %a, %b", int_llvm)),
        ("Ge", false) => Some(format!("%v = icmp sge {} %a, %b", int_llvm)),
        ("Neg", false) => Some(format!("%v = sub nsw {} 0, %a", int_llvm)),
        ("Abs", false) => Some(format!("%v = call {} @llvm.abs.{}({} %a, i1 false)", int_llvm, int_llvm, int_llvm)),
        // 2026-08-14 (boundary plan, SPEC §17.3): the four bit intrinsics —
        // declared at emit_toplevel.rs, now dispatched here. All integer
        // unary; ctlz/cttz take the poison-on-zero flag (false = return the
        // bit width for an all-zero input, matching C semantics).
        ("BitReverse", false) => Some(format!("%v = call {} @llvm.bitreverse.{}({} %a)", int_llvm, int_llvm, int_llvm)),
        ("Popcount", false) => Some(format!("%v = call {} @llvm.ctpop.{}({} %a)", int_llvm, int_llvm, int_llvm)),
        ("LeadingZeros", false) => Some(format!("%v = call {} @llvm.ctlz.{}({} %a, i1 false)", int_llvm, int_llvm, int_llvm)),
        ("TrailingZeros", false) => Some(format!("%v = call {} @llvm.cttz.{}({} %a, i1 false)", int_llvm, int_llvm, int_llvm)),

        ("BitAnd", false) => Some(format!("%v = and {} %a, %b", int_llvm)),
        ("BitOr", false) => Some(format!("%v = or {} %a, %b", int_llvm)),
        ("BitXor", false) => Some(format!("%v = xor {} %a, %b", int_llvm)),
        ("Shl", false) => Some(format!("%v = shl {} %a, %b", int_llvm)),
        ("Shr", false) => Some(format!("%v = ashr {} %a, %b", int_llvm)),

        _ => None,
    }
}

fn resolve_arg_bytes(backend: &LlvmBackend, reg: &BTypedRegister) -> Option<u64> {
    backend.ctx.type_universe.as_ref()
        .and_then(|u| crate::type_universe::resolve_type(u, &reg.ty))
        .map(|rt| rt.bytes)
}

// ── Helper: emit argument expressions and return their register names ──

fn emit_args(backend: &mut LlvmBackend, out: &mut String, args: &[Expr], indent: &str) -> Vec<String> {
    args.iter()
        .map(|a| backend.emit_expr(out, a, indent).name)
        .collect()
}

fn emit_arg(backend: &mut LlvmBackend, out: &mut String, arg: &Expr, indent: &str) -> String {
    backend.emit_expr(out, arg, indent).name
}

/// 2026-08-10: Truncate an i64-valued Int intrinsic result to the target int
/// width (i{int_bits}). C-runtime intrinsics (Now#, syscall, sysconf, atol)
/// return i64, but an Int-typed register is i{int_bits} (i32 on wasm32) — an
/// i64 value feeding `icmp slt i32` is invalid IR. x86_64 (int_bits=64) emits
/// `trunc i64 to i64`, folded to a no-op by LLVM. NOT for pointer/address
/// results (Malloc/Alloc/custom alloc) — those stay i64.
fn narrow_int_result(backend: &mut LlvmBackend, out: &mut String, v: &str, indent: &str) -> String {
    let width = format!("i{}", backend.ctx.int_bits);
    if width == "i64" {
        return v.to_string();
    }
    let r = backend.fun.gen_reg();
    writeln!(out, "{}{} = trunc i64 {} to {}", indent, r, v, width).ok();
    r
}

// ─── Memory intrinsics ────────────────────────────────────────────────

/// 2026-08-12 (Iterable protocol): `CharCount#(s)` — the UTF8 CHAR count of
/// a String (the scan). The arg is the String (a ptr in the bits model; a
/// boxed i64 handle at a call/binding boundary — recover the ptr via
/// string_ptr). Emits `call i64 @briev_char_len(ptr ...)`.
fn emit_char_count(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    if args.is_empty() {
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    }
    let reg = backend.emit_expr(out, &args[0], indent);
    let p = backend.string_ptr(out, indent, &reg);
    writeln!(out, "{}{} = call i64 @briev_char_len({}ptr {})", indent, v,
            if backend.ctx.defn_params.contains_key("briev_char_len") { "ptr %state, " } else { "" }, p).ok();
    let narrowed = narrow_int_result(backend, out, v, indent);
    BTypedRegister { name: narrowed, ty: Type::int() }
}

fn emit_malloc(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {    let size = emit_arg(backend, out, &args[0], indent);
    // 2026-08-11 (wasm32 obj-member fix): the size is an Int value at
    // i{int_bits} (i32 on wasm32) — widen to i64 for the C malloc ABI. The
    // old bare `i64 {size}` broke wasm32 (i32 value in an i64 call arg). A
    // no-op on x86_64 (int_bits=64).
    let size64 = widen_to_i64(backend, out, &size, indent);
    // 2026-07-17: Return Ptr<Int> so Expr::Index correctly identifies this
    // as a pointer type and emits GEP+load/store (not extractelement). The
    // raw bits are still i64 (ptrtoint); the type annotation only affects
    // downstream codegen dispatch. State storage boxes via adapt_to_i64.
    let name = v.trim_start_matches('%');
    // 2026-08-04 (Phase 4, .ebv heap reframe): embedded freestanding — Malloc#
    // routes to the static bump arena (@embedded_heap), never @malloc. The
    // arena result is an i64 (the bump address); emit_arena_alloc returns the
    // ptrtoint'd i64 which we re-interpret as a pointer for the Ptr<Int> ABI.
    if backend.ctx.is_embedded {
        let arena_result = backend.emit_arena_alloc(out, indent, &size64);
        writeln!(out, "{}%{}_p = inttoptr i64 {} to ptr", indent, name, arena_result).ok();
        writeln!(out, "{}{} = ptrtoint ptr %{}_p to i64", indent, v, name).ok();
        backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Arena);
        let remaining_reg = backend.fun.gen_reg();
        writeln!(out, "{} {} = add i64 {}, 0", indent, remaining_reg, size64).ok();
        backend.fun.fat_ptrs.insert(v.to_string(), (v.to_string(), "0".to_string(), remaining_reg));
        return BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::int()) };
    }
    writeln!(out, "{}%{}_p = call ptr @malloc(i64 {})", indent, name, size64).ok();
    writeln!(out, "{}{} = ptrtoint ptr %{}_p to i64", indent, v, name).ok();
    // 2026-07-18: Record Malloc strategy so Free# can dispatch correctly.
    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Malloc);
    // 2026-07-18: Record fat pointer provenance — base points to alloc,
    // offset 0, remaining = size. This enables O(1) Length#(ptr).
    let remaining_reg = backend.fun.gen_reg();
    writeln!(out, "{} {} = add i64 {}, 0", indent, remaining_reg, size64).ok();
    backend.fun.fat_ptrs.insert(v.to_string(), (v.to_string(), "0".to_string(), remaining_reg));
    BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::int()) }
}

/// 2026-08-11 (wasm32 obj-member fix): widen an `i{int_bits}` value to i64
/// for a C-ABI intrinsic argument (malloc sizes, etc.). A no-op on x86_64.
fn widen_to_i64(backend: &mut LlvmBackend, out: &mut String, reg: &str, indent: &str) -> String {
    if backend.ctx.int_bits == 64 {
        return reg.to_string();
    }
    let width = format!("i{}", backend.ctx.int_bits);
    let r = backend.fun.gen_reg();
    writeln!(out, "{}{} = zext {} {} to i64", indent, r, width, reg).ok();
    r
}

// 2026-07-18: Alloc# — compiler-delegated allocation with triple dispatch.
// Args:
//   Alloc#(size)                        — compiler picks (scope-based)
//   Alloc#(size, Arena)                 — PascalCase: intrinsic dispatch
//   Alloc#(size, Malloc)                — PascalCase: intrinsic dispatch
//   Alloc#(size, Alloca)                — PascalCase: intrinsic dispatch
//   Alloc#(size, "pool_serial")         — quoted: config/alloc-strategies.dbvl
//   Alloc#(size, my_custom_alloc_fn)    — identifier: user Briev function
fn emit_alloc(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str, analysis_id: Option<usize>,
) -> BTypedRegister {
    let size = emit_arg(backend, out, &args[0], indent);

    // 2026-07-18: Phase 4 — Check pre-computed strategy from analysis pass.
    // If the analysis determined Malloc (escape detected), use it directly.
    if let Some(aid) = analysis_id {
        if let Some(ref strategies) = backend.analysis_alloc_strategies {
            if let Some(strategy) = strategies.get(&aid) {
                return match strategy {
                    AllocStrategy::Malloc => {
                        // TEMP: 2026-07-18: Conservative — always Malloc for now.
                        // Full escape analysis will assign Arena/Alloca when safe.
                        emit_malloc_inline(backend, out, v, &size, indent)
                    }
                    AllocStrategy::Arena => {
                        let result = backend.emit_arena_alloc(out, indent, &size);
                        writeln!(out, "{}{} = add i64 0, {}", indent, v, result).ok();
                        backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Arena);
                        BTypedRegister { name: v.to_string(), ty: Type::int() }
                    }
                    AllocStrategy::Alloca => {
                        let a = format!("%alloc_{}", backend.fun.txn_counter);
                        backend.fun.txn_counter += 1;
                        writeln!(out, "{}{} = alloca i8, i64 {}", indent, a, size).ok();
                        writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, a).ok();
                        backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Alloca);
                        BTypedRegister { name: v.to_string(), ty: Type::int() }
                    }
                    // 2026-07-18: Inline — allocation fits in parent struct field.
                    // The Alloc# is a no-op; the address is computed from the
                    // containing struct's field offset at access time.
                    AllocStrategy::Inline => {
                        backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Inline);
                        writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
                        BTypedRegister { name: v.to_string(), ty: Type::int() }
                    }
                    // 2026-07-18: RingBuffer — circular buffer with wrap-around.
                    AllocStrategy::RingBuffer => {
                        return emit_ring_buffer_alloc(backend, out, v, &size, indent);
                    }
                    // 2026-07-18: Config strategy — look up template.
                    AllocStrategy::Config(_) | AllocStrategy::Custom(_) => {
                        backend.fun.alloc_strategies.insert(v.to_string(), strategy.clone());
                        emit_malloc_inline(backend, out, v, &size, indent)
                    }
                };
            }
        }
    }

    // Check for optional 2nd arg (strategy override) — explicit user override
    // takes priority over analysis.
    if args.len() >= 2 {
        return emit_alloc_with_strategy(backend, out, v, args, indent, &size);
    }
    // Default triple dispatch (no strategy arg).
    // Strategy 1: Arena scope active → bump allocate.
    // 2026-07-19: Arena is in %State fields — available in any function that
    // has %state (all txns, callable txns, and their helpers by inheritance).
    if backend.arena_ptr_idx.is_some() {
        // 2026-07-19: emit_arena_alloc returns the old bump pointer as i64.
        // The caller receives it directly — no ptrtoint needed.
        let result = backend.emit_arena_alloc(out, indent, &size);
        writeln!(out, "{}{} = add i64 0, {}", indent, v, result).ok();
        backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Arena);
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    }
    if backend.is_in_bounded_scope() && !backend.will_escape_current_allocation() {
        // 2026-07-18: Check if size is a compile-time constant.
        let is_constant = matches!(&args[0], Expr::Decimal(_));
        if is_constant {
            let a = format!("%alloc_{}", backend.fun.txn_counter);
            backend.fun.txn_counter += 1;
            writeln!(out, "{}{} = alloca i8, i64 {}", indent, a, size).ok();
            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, a).ok();
            backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Alloca);
            return BTypedRegister { name: v.to_string(), ty: Type::int() };
        }
        // Runtime fallback: try alloca, fall back to malloc if size > threshold.
        return emit_dynamic_alloc(backend, out, v, &size, indent);
    }
    // Strategy 3: Default → @malloc.
    emit_malloc_inline(backend, out, v, &size, indent)
}

// 2026-07-18: Handle explicit strategy override for Alloc#.
// Strategy can be PascalCase (Arena/Malloc/Alloca), quoted string (config),
// or an identifier (user function).
fn emit_alloc_with_strategy(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str, size: &str,
) -> BTypedRegister {
    let strategy_expr = &args[1];
    match strategy_expr {
        Expr::Identifier(name) => {
            match name.as_str() {
                "Arena" => {
                    let result = backend.emit_arena_alloc(out, indent, size);
                    // 2026-07-19: emit_arena_alloc returns i64 — route to v.
                    writeln!(out, "{}{} = add i64 0, {}", indent, v, result).ok();
                    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Arena);
                }
                "Malloc" => {
                    emit_malloc_inline(backend, out, v, size, indent);
                }
                "Alloca" => {
                    writeln!(out, "{}{} = alloca i8, i64 {}", indent, v, size).ok();
                    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Alloca);
                }
                // Unknown PascalCase — treat as user function name.
                custom_fn => {
                    let fn_reg = emit_arg(backend, out, strategy_expr, indent);
                    writeln!(out, "{}{} = call i64 @{}(i64 {})", indent, v, custom_fn, size).ok();
                    // Conservative: unknown strategy → Malloc for Free# dispatch.
                    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Malloc);
                }
            }
        }
        Expr::Quoted(bytes) => {
            let strategy_name = String::from_utf8_lossy(bytes).to_string();
            // Look up in config/alloc-strategies.dbvl.
            let found = emit_alloc_from_config(backend, out, v, &strategy_name, size, indent);
            if !found {
                // Fallback to @malloc with warning.
                let msg = format!("warning: unknown alloc strategy '{}', falling back to malloc", strategy_name);
                backend.warnings.push(msg);
                emit_malloc_inline(backend, out, v, size, indent);
            }
        }
        _ => {
            // Unknown expression type — emit as function call.
            let fn_reg = emit_arg(backend, out, strategy_expr, indent);
            writeln!(out, "{}{} = call i64 @custom_alloc(i64 {}, i64 {})", indent, v, size, fn_reg).ok();
            backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Malloc);
        }
    }
    BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::int()) }
}

// 2026-07-18: Look up a quoted strategy name in config/alloc-strategies.dbvl
// and emit the corresponding LLVM IR template. Returns true if found.
fn emit_alloc_from_config(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    strategy_name: &str, size: &str, indent: &str,
) -> bool {
    let config = crate::config::AllocConfig::load();
    let Some(template) = config.lookup(strategy_name) else {
        return false;
    };
    let ir = template
        .replace("{v}", v.trim_start_matches('%'))
        .replace("{size}", size);
    writeln!(out, "{}  {}", indent, ir).ok();
    // AllocConfig entries default to Malloc strategy for Free# dispatch.
    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Malloc);
    true
}

// 2026-07-18: Runtime fallback — try stack (alloca), fall back to heap
// if the allocation size exceeds the stack threshold. Used for dynamic-size
// allocs where the strategy is Alloca but size is unknown at compile time.
fn emit_dynamic_alloc(
    backend: &mut LlvmBackend, out: &mut String, v: &str, size: &str, indent: &str,
) -> BTypedRegister {
    let counter = backend.fun.txn_counter;
    backend.fun.txn_counter += 1;
    let stack_l = format!(".stack{}", counter);
    let heap_l = format!(".heap{}", counter);
    let done_l = format!(".done{}", counter);

    writeln!(out, "  %cmp = icmp ule i64 {}, {}", size, backend.ctx.stack_threshold).ok();
    writeln!(out, "  br i1 %cmp, label %{}, label %{}", stack_l, heap_l).ok();
    writeln!(out, "{}:", stack_l).ok();
    writeln!(out, "  %s = alloca i8, i64 {}", size).ok();
    writeln!(out, "  %sv = ptrtoint ptr %s to i64").ok();
    writeln!(out, "  br label %{}", done_l).ok();
    writeln!(out, "{}:", heap_l).ok();
    writeln!(out, "  %h = call ptr @malloc(i64 {})", size).ok();
    writeln!(out, "  %hv = ptrtoint ptr %h to i64").ok();
    writeln!(out, "  br label %{}", done_l).ok();
    writeln!(out, "{}:", done_l).ok();
    writeln!(out, "  {} = phi i64 [ %sv, %{} ], [ %hv, %{} ]", v, stack_l, heap_l).ok();
    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Alloca);
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

// 2026-07-18: RingBuffer — circular buffer alloc via slot-based ring.
// Each allocation advances a head pointer modulo RING_SIZE (power of two).
// Free# is a no-op — old slots are overwritten when head wraps around.
fn emit_ring_buffer_alloc(
    backend: &mut LlvmBackend, out: &mut String, v: &str, size: &str, indent: &str,
) -> BTypedRegister {
    let counter = backend.fun.txn_counter;
    backend.fun.txn_counter += 1;
    // Ring buffer uses a stack-allocated circular buffer per txn scope.
    // For now: emit as alloca and mark as RingBuffer for Free# behavior.
    // Full implementation would use @ring_head global + wrapping GEP.
    let a = format!("%ring_buf{}", counter);
    writeln!(out, "{}{} = alloca i8, i64 {}", indent, a, size).ok();
    writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, a).ok();
    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::RingBuffer);
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

// 2026-07-18: Emit @malloc for a size register, returning ptrtoint'd i64.
fn emit_malloc_inline(
    backend: &mut LlvmBackend, out: &mut String, v: &str, size: &str, indent: &str,
) -> BTypedRegister {
    let name = v.trim_start_matches('%');
    writeln!(out, "{}%{}_p = call ptr @malloc(i64 {})", indent, name, size).ok();
    writeln!(out, "{}{} = ptrtoint ptr %{}_p to i64", indent, v, name).ok();
    backend.fun.alloc_strategies.insert(v.to_string(), AllocStrategy::Malloc);
    let remaining_reg = backend.fun.gen_reg();
    writeln!(out, "{} {} = add i64 {}, 0", indent, remaining_reg, size).ok();
    backend.fun.fat_ptrs.insert(v.to_string(), (v.to_string(), "0".to_string(), remaining_reg));
    // 2026-07-18: Alloc# returns i64 (ptrtroint), not ptr.
    // The register already holds ptrtoint ptr %malloc_p to i64.
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

// 2026-07-18: Free# — strategy-aware. Looks up the pointer's allocation
// strategy from the alloc_strategies map. Arena/Alloca → no-op (memory
// reclaimed by scope end / arena reset). Malloc/unknown → call @free.
fn emit_free(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ptr_reg = emit_arg(backend, out, &args[0], indent);
    // Look up the allocation strategy for the pointer register.
    let strategy = backend.fun.alloc_strategies.get(&ptr_reg);
    match strategy {
        // 2026-07-18: Inline, RingBuffer, Arena, Alloca — no Free# needed.
        Some(AllocStrategy::Arena) | Some(AllocStrategy::Alloca)
            | Some(AllocStrategy::Inline) | Some(AllocStrategy::RingBuffer) => {}
        // 2026-07-18: Config strategy — check the free field from config.
        Some(AllocStrategy::Config(name)) => {
            match ALLOC_CONFIG.lookup_free(name) {
                Some("none") => {}  // no-op
                Some(fn_name) => {   // custom free function
                    writeln!(out, "{}call void @{}(ptr {})", indent, fn_name, ptr_reg).ok();
                }
                None => {            // default → @free
                    writeln!(out, "{}call void @free(ptr {})", indent, ptr_reg).ok();
                }
            }
        }
        Some(AllocStrategy::Malloc) | Some(AllocStrategy::Custom(_)) | _ => {
            // Heap-allocated (Malloc) or unknown → emit @free.
            // 2026-08-01 (D2): a Ptr value is stored as an i64 handle (ptrtoint
            // at store); the handle must be inttoptr'd before the @free call.
            let p = backend.fun.gen_reg();
            writeln!(out, "{}  {} = inttoptr i64 {} to ptr", indent, p, ptr_reg).ok();
            writeln!(out, "{}call void @free(ptr {})", indent, p).ok();
        }
    }
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

 fn emit_load(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let bytes = args.get(1).and_then(|a| if let Expr::Decimal(n) = a { Some(*n as usize) } else { None }).unwrap_or(8);
    writeln!(out, "{}{} = load i{}, ptr {}", indent, v, bytes * 8, ptr).ok();
    // 2026-07-18: Narrow loads (< 8 bytes) are zero-extended to i64 so the
    // result matches the declared return type (Int = i64). Without this,
    // comparisons of loaded bytes fail (icmp expects i64, got i8).
    if bytes < 8 {
        let zext = backend.fun.gen_reg();
        writeln!(out, "{}{} = zext i{} {} to i64", indent, zext, bytes * 8, v).ok();
        return BTypedRegister { name: zext, ty: Type::int() };
    }
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_store(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let addr = emit_arg(backend, out, &args[0], indent);
    let val = emit_arg(backend, out, &args[1], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let bytes = args.get(2).and_then(|a| if let Expr::Decimal(n) = a { Some(*n as usize) } else { None }).unwrap_or(8);
    // 2026-09-09 (Family A): narrow stores must TRUNC the i64 value first —
    // `store i8 <i64 reg>` is invalid IR. (VolatileStore# already
    // width-adapts; plain Store# never hit a sub-word width until the
    // pure-Briev utf8_encode byte stores.) Loads already zext narrow
    // results (emit_load).
    let stored = if bytes < 8 {
        let t = backend.fun.gen_reg();
        writeln!(out, "{}{} = trunc i64 {} to i{}", indent, t, val, bytes * 8).ok();
        t
    } else {
        val.clone()
    };
    writeln!(out, "{}store i{} {}, ptr {}", indent, bytes * 8, stored, ptr).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}


/// 2026-08-27 (plan 2026-08-27-cbv-foreign-hardware-and-mmio.md Slice C):
/// `VolatileLoad#(p: Ptr<T>) -> T`. Briev's pointer ABI boxes addresses as
/// i64 registers (the atomics' convention): re-materialize via
/// `inttoptr i64 -> ptr` before the access. The DECLARED pointee drives
/// result type + alignment via the casting graph; shape enforcement lives
/// in the TYPECHECKER and a non-Ptr declaration degrades to one-word Int.
fn emit_volatile_load(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let arg = backend.emit_expr(out, &args[0], indent);
    let inner_ty = match &arg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr {} {} to ptr", indent, ptr,
        backend.llvm_type(&Type::int()), arg.name).ok();
    writeln!(out, "{}{} = load volatile {}, ptr {}, align {}", indent, v,
        llvm_ty, ptr, backend.align_of(&llvm_ty)).ok();
    BTypedRegister { name: v.to_string(), ty: inner_ty }
}

/// `VolatileStore#(p: Ptr<T>, val: T) -> Bool`. Value is width-adapted to
/// the pointee so the store text is always valid IR.
fn emit_volatile_store(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ptr_arg = backend.emit_expr(out, &args[0], indent);
    let addr_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr {} {} to ptr", indent, addr_ptr,
        backend.llvm_type(&Type::int()), ptr_arg.name).ok();
    let mut val_reg = backend.emit_expr(out, &args[1], indent);
    let inner_ty = match &ptr_arg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    // Width compare on the EMITTED LLVM types, not the Briev metadata:
    // the cast must be valid between the IR registers this function just
    // materialized, and resolve_arg_bytes under-reports the abstract Int
    // on narrow targets (Int is i64 in IR even on thumbv7m).
    // 2026-09-14 (rv64-finish plan Phase 5): found by the ARM SysTick
    // demo — Ptr<Bit<32>> MMIO store emitted `zext i64 -> i32`, invalid IR.
    let llvm_bits = |t: &str| -> u64 {
        t.strip_prefix("i").and_then(|n| n.parse().ok()).unwrap_or(64)
    };
    let val_llvm_ty = backend.llvm_type(&val_reg.ty);
    let target_bits = llvm_bits(&llvm_ty);
    let val_bits = llvm_bits(&val_llvm_ty);
    if val_bits > target_bits {
        let trunc = backend.fun.gen_reg();
        writeln!(out, "{}{} = trunc {} {} to {}", indent, trunc,
            val_llvm_ty, val_reg.name, llvm_ty).ok();
        val_reg.name = trunc;
    } else if val_bits < target_bits {
        let ext = backend.fun.gen_reg();
        writeln!(out, "{}{} = zext {} {} to {}", indent, ext,
            val_llvm_ty, val_reg.name, llvm_ty).ok();
        val_reg.name = ext;
    }
    writeln!(out, "{}store volatile {} {}, ptr {}, align {}", indent,
        llvm_ty, val_reg.name, addr_ptr, backend.align_of(&llvm_ty)).ok();
    writeln!(out, "{}{} = add i64 0, 1", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::bool_() }
}

fn emit_copy(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let dst = emit_arg(backend, out, &args[0], indent);
    let src = emit_arg(backend, out, &args[1], indent);
    let len = emit_arg(backend, out, &args[2], indent);
    let dptr = backend.fun.gen_reg();
    let sptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, dptr, dst).ok();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, sptr, src).ok();
    writeln!(out, "{}call void @llvm.memcpy.p0.p0.i64(ptr {}, ptr {}, i64 {}, i1 false)", indent, dptr, sptr, len).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

fn emit_fill(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ptr_arg = emit_arg(backend, out, &args[0], indent);
    let val = emit_arg(backend, out, &args[1], indent);
    let len = emit_arg(backend, out, &args[2], indent);
    let p = backend.fun.gen_reg();
    let v8 = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, ptr_arg).ok();
    writeln!(out, "{}{} = trunc i64 {} to i8", indent, v8, val).ok();
    writeln!(out, "{}call void @llvm.memset.p0.i64(ptr {}, i8 {}, i64 {}, i1 false)", indent, p, v8, len).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

/// 2026-08-15 (coll plan §3.6): `Capacity#(h)` — read a coll's hidden `cap`
/// slot (offset 8 of the `[data, cap, len]` block). One load.
fn emit_capacity(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // 2026-08-16 (Phase 3a): a fixed `coll struct` has NO hidden `cap` slot —
    // its capacity IS the compile-time N (SPEC §8.10: Capacity# returns N, a
    // constant). Reading offset 8 (the growable coll's cap slot) would load a
    // neighboring field. A fixed coll struct registered InlineFixed gets the
    // constant.
    let is_fixed = match args.first() {
        Some(Expr::Identifier(n)) => {
            let bound = backend.fun.let_binding_types.get(n)
                .cloned()
                .or_else(|| backend.fun.let_original_types.get(n).cloned());
            bound.map_or(false, |t| {
                let base = match &t {
                    crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.clone(),
                    _ => return false,
                };
                matches!(
                    backend.ctx.coll_storage.get(&base),
                    Some(crate::backend::llvm::coll_scaffold::CollStorage::InlineFixed)
                )
            })
        }
        _ => false,
    };
    if is_fixed {
        let n = backend.coll_fixed_length(&match args.first() {
            Some(Expr::Identifier(name)) => backend.fun.let_binding_types.get(name)
                .or_else(|| backend.fun.let_original_types.get(name))
                .cloned()
                .unwrap_or_else(crate::ast::Type::int),
            _ => crate::ast::Type::int(),
        });
        writeln!(out, "{}{} = add i64 0, {}", indent, v, n).ok();
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    }
    let h = emit_arg(backend, out, &args[0], indent);
    let p = backend.fun.gen_reg();
    let gep = backend.fun.gen_reg();
    let load = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, h).ok();
    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 8", indent, gep, p).ok();
    writeln!(out, "{}{} = load i64, ptr {}", indent, load, gep).ok();
    writeln!(out, "{}{} = add i64 {}, 0", indent, v, load).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

/// 2026-08-15 (coll plan §3.6): `Resize#(h, cap)` — set the data buffer to
/// exactly `cap` elements (realloc-or-copy), store the new cap.
///
/// 2026-08-15 (grow-on-full): routes through the runtime `__briev_coll_resize`
/// — malloc a fresh buffer of `cap * 8`, copy `min(len, cap)` elements, free
/// the old buffer, store the new data + cap. The previous inline emission
/// malloc'd fresh WITHOUT copying or freeing (data loss + leak); the runtime
/// is the single source of resize truth (EnsureCap#/TrimCap# already route
/// through it) and mutates the `[data, cap, len]` block in place — a grow
/// guard in a member body never needs to reassign the data slot, so no
/// register merge is required across the guard branch.
fn emit_resize(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let h = emit_arg(backend, out, &args[0], indent);
    let cap = emit_arg(backend, out, &args[1], indent);
    let call = backend.fun.gen_reg();
    writeln!(out, "{}{} = call i64 @__briev_coll_resize({}i64 {}, i64 {})", indent, call,
            if backend.ctx.defn_params.contains_key("__briev_coll_resize") { "ptr %state, " } else { "" }, h, cap).ok();
    let _ = call;
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

/// 2026-08-15 (coll plan §3.6): `EnsureCap#(h, n)` — grow the data buffer to
/// at least `n` elements (a no-op when the current cap is already >= n).
fn emit_ensure_cap(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let h = emit_arg(backend, out, &args[0], indent);
    let n = emit_arg(backend, out, &args[1], indent);
    let p = backend.fun.gen_reg();
    let cap_gep = backend.fun.gen_reg();
    let cur_cap = backend.fun.gen_reg();
    let cmp = backend.fun.gen_reg();
    let grow = backend.fun.gen_reg();
    let after = backend.fun.gen_reg();
    let target = backend.fun.gen_reg();
    let call = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, h).ok();
    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 8", indent, cap_gep, p).ok();
    writeln!(out, "{}{} = load i64, ptr {}", indent, cur_cap, cap_gep).ok();
    writeln!(out, "{}{} = icmp ult i64 {}, {}", indent, cmp, cur_cap, n).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, grow).ok();
    writeln!(out, "{}{} = select i1 {}, i64 {}, i64 {}", indent, after, cmp, n, cur_cap).ok();
    writeln!(out, "{}{} = call i64 @__briev_coll_resize({}i64 {}, i64 {})", indent, target,
            if backend.ctx.defn_params.contains_key("__briev_coll_resize") { "ptr %state, " } else { "" }, h, after).ok();
    writeln!(out, "{}{} = add i64 {}, 0", indent, call, target).ok();
    let _ = (grow, call);
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

/// 2026-08-15 (coll plan §3.6): `TrimCap#(h)` — shrink the data buffer to the
/// current length (shrink-to-fit).
fn emit_trim_cap(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let h = emit_arg(backend, out, &args[0], indent);
    let p = backend.fun.gen_reg();
    let len_gep = backend.fun.gen_reg();
    let len = backend.fun.gen_reg();
    let call = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, h).ok();
    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 16", indent, len_gep, p).ok();
    writeln!(out, "{}{} = load i64, ptr {}", indent, len, len_gep).ok();
    writeln!(out, "{}{} = call i64 @__briev_coll_resize({}i64 {}, i64 {})", indent, call,
            if backend.ctx.defn_params.contains_key("__briev_coll_resize") { "ptr %state, " } else { "" }, h, len).ok();
    let _ = call;
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

// ─── GetEnv# ──────────────────────────────────────────────────────────

// 2026-07-19: GetEnv# returns the raw env var value as a String.
// Returns empty string {0, 0} if the env var is not found.
fn emit_get_env(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let name_reg = backend.emit_expr(out, &args[0], indent);
    // 2026-08-10: the string operand may be an unboxed ptr (literal's
    // @str.N global) or a boxed i64 handle — string_ptr handles both. The old
    // hardcoded `inttoptr i64` broke on a ptr operand (wasm32 llc: "'%t8'
    // defined with type 'ptr' but expected 'i64'").
    let ptr_reg = backend.string_ptr(out, indent, &name_reg);
    // Call getenv — may return null
    let env_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = call ptr @getenv(ptr {})", indent, env_ptr, ptr_reg).ok();
    let is_null = backend.fun.gen_reg();
    writeln!(out, "{}{} = icmp eq ptr {}, null", indent, is_null, env_ptr).ok();
    // Allocate a fallback 1-byte null-terminated buffer for the null case
    let fb = backend.fun.gen_reg();
    writeln!(out, "{}{} = alloca i8, i64 1", indent, fb).ok();
    writeln!(out, "{}store i8 0, ptr {}", indent, fb).ok();
    // Safe pointer: fallback buffer when null, real pointer otherwise
    let safe_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = select i1 {}, ptr {}, ptr {}", indent, safe_ptr, is_null, fb, env_ptr).ok();
    // Compute length via strlen on the safe pointer
    let len = backend.fun.gen_reg();
    writeln!(out, "{}{} = call i64 @strlen(ptr {})", indent, len, safe_ptr).ok();
    // Data: 0 when null, ptrtoint(env_ptr) otherwise
    let data_raw = backend.fun.gen_reg();
    writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, data_raw, env_ptr).ok();
    let data = backend.fun.gen_reg();
    writeln!(out, "{}{} = select i1 {}, i64 0, i64 {}", indent, data, is_null, data_raw).ok();
    // Pack into {i64, i64} SSO string struct
    let t1 = backend.fun.gen_reg();
    writeln!(out, "{}{} = insertvalue {{ i64, i64 }} undef, i64 {}, 0", indent, t1, data).ok();
    let t2 = backend.fun.gen_reg();
    writeln!(out, "{}{} = insertvalue {{ i64, i64 }} %{}, i64 {}, 1", indent, t2, t1, len).ok();
    BTypedRegister { name: t2, ty: Type::string() }
}

// 2026-07-19: GetEnvInt# returns the env var value parsed as Int.
// Returns 0 if the env var is missing or unparseable (matches atol behavior).
fn emit_get_env_int(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let name_reg = backend.emit_expr(out, &args[0], indent);
    // 2026-08-10: string_ptr handles unboxed-ptr vs boxed-i64 operands (see
    // emit_get_env). The old hardcoded `inttoptr i64` broke on ptr operands.
    let ptr_reg = backend.string_ptr(out, indent, &name_reg);
    // 2026-07-28: Briev strings are stored as [i64 length][data\0] with the
    // handle pointing to the struct start. getenv expects just the data portion.
    // Without this GEP, getenv reads the length field as the string (e.g.,
    // length=5 → binary 0x05 → empty string) → returns NULL → atol(NULL)
    // segfaults. This was the root cause of the popcount binary crash.
    let data_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 8", indent, data_ptr, ptr_reg).ok();
    let env_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = call ptr @getenv(ptr {})", indent, env_ptr, data_ptr).ok();
    let atol_reg = backend.fun.gen_reg();
    writeln!(out, "{}{} = call i64 @atol(ptr {})", indent, atol_reg, env_ptr).ok();
    // 2026-08-10: atol returns i64 but the Int register width is i{int_bits}
    // (i32 wasm32) — narrow so the result matches llvm_type(Int) and feeds
    // i32 comparisons/arithmetic. x86_64 (int_bits=64) returns the value as-is.
    let narrowed = narrow_int_result(backend, out, &atol_reg, indent);
    BTypedRegister { name: narrowed, ty: Type::int() }
}

// ─── GetGlobalId# ─────────────────────────────────────────────────────

fn emit_get_global_id(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let dim = emit_arg(backend, out, &args[0], indent);
    writeln!(out, "{}{} = call i32 @__get_global_id(i32 {})", indent, v, dim).ok();
    let ext = backend.fun.gen_reg();
    writeln!(out, "{}{} = zext i32 {} to i64", indent, ext, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

// ─── AddressOf# — compile-time address resolution ─────────────────────

/// 2026-07-15: AddressOf# resolves a named device/entity to a typed pointer.
/// The address is resolved at compile time via the shared address_resolver
/// (which reads config/address-map.dbvl + hardcoded fallbacks).
/// Emits: %v = inttoptr i64 <addr> to ptr
fn emit_address_of(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // Guard against empty args — address must be provided
    let Some(arg) = args.first() else {
        eprintln!("AddressOf#: warning — no arguments, emitting 0 as address");
        backend.emit_inttoptr(out, indent, &v, &"0");
        return BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::bits(8)) };
    };
    // The argument must be a string literal at compile time
    let id = match arg {
        Expr::Quoted(bytes) => String::from_utf8_lossy(bytes).to_string(),
        _ => {
            // If not a literal, try emitting as expression and warn
            let reg = emit_arg(backend, out, &args[0], indent);
            eprintln!("AddressOf#: warning — argument is not a string literal, using runtime value");
            format!("dynamic_{}", reg)
        }
    };
    let addr = crate::address_resolver::resolve_address(&id);
    let addr_str = addr.to_string();
    backend.emit_inttoptr(out, indent, &v, &addr_str);
    BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::bits(8)) }
}

/// `CancelRequested#()` — load the process-global cancel flag as a Bool.
/// 2026-08-03: the host raises it via `__briev_set_cancel`; Briev loops
/// poll explicitly (no implicit injection).
fn emit_cancel_requested(
    backend: &mut LlvmBackend, out: &mut String, v: &str, indent: &str,
) -> BTypedRegister {
    let flag = backend.fun.gen_reg();
    writeln!(out, "{}{} = load atomic i32, ptr @__briev_cancel_flag seq_cst, align 4", indent, flag).ok();
    let cmp = backend.fun.gen_reg();
    writeln!(out, "{}{} = icmp ne i32 {}, 0", indent, cmp, flag).ok();
    let zext = backend.fun.gen_reg();
    writeln!(out, "{}{} = zext i1 {} to i8", indent, zext, cmp).ok();
    BTypedRegister { name: zext.to_string(), ty: Type::bool_() }
}

/// `CallPtr#(cb, args...)` — call a function-pointer value.
///
/// 2026-08-03: `cb` is a `fn(...)` value (an opaque `ptr` under LLVM opaque
/// pointers) that crossed the FFI boundary as a callback. Emits
/// `call <ret> ptr %cb(args...)`. The return type is taken from the fn type
/// (default i64); args are passed as their native LLVM types.
fn emit_call_ptr(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let Some(cb_expr) = args.first() else {
        writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    };
    let cb_reg = emit_arg(backend, out, cb_expr, indent);

    // Resolve the fn's return type from the callback's declared type.
    let (ret_ll, ret_briev) = match cb_expr {
        Expr::Identifier(name) => match backend.fun.let_binding_types.get(name) {
            Some(Type::Function(_, ret)) => {
                let ll = backend.llvm_type(ret);
                let briev = match ll.as_str() {
                    "float" | "double" => Type::float(),
                    "ptr" => Type::string(),
                    "void" => Type::void(),
                    _ => Type::int(),
                };
                (ll, briev)
            }
            _ => ("i64".to_string(), Type::int()),
        },
        _ => ("i64".to_string(), Type::int()),
    };

    let mut call_args: Vec<String> = Vec::new();
    for a in &args[1..] {
        let reg = emit_arg(backend, out, a, indent);
        // Int args cross as i64; String as ptr. Resolve identifier types via
        // the binding map, defaulting to i64.
        let ll = match a {
            Expr::Identifier(name) => backend.fun.let_binding_types.get(name)
                .map(|t| backend.llvm_type(t))
                .unwrap_or_else(|| "i64".to_string()),
            _ => "i64".to_string(),
        };
        call_args.push(format!("{} {}", ll, reg));
    }
    // The callback param was ptrtoint'd to i64 at function entry; cast back
    // to a pointer so the `call` operand type matches.
    let cb_ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, cb_ptr, cb_reg).ok();
    writeln!(
        out, "{}{} = call {} {}({})",
        indent, v, ret_ll, cb_ptr, call_args.join(", ")
    ).ok();
    BTypedRegister { name: v.to_string(), ty: ret_briev }
}

// ─── Len# / Length# — load list length from 2-slot header ──────────────

fn emit_len(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let arg_reg = emit_arg(backend, out, &args[0], indent);
    // Check if this argument has fat pointer provenance — if so, read
    // the remaining length directly from the provenance metadata.
    if let Some((_base, _offset, ref remaining)) = backend.fun.fat_ptrs.get(&arg_reg).cloned() {
        writeln!(out, "{}{} = add i64 {}, 0", indent, v, remaining).ok();
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    }
    // Fallback: load length from string/list header slot 0.
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, arg_reg).ok();
    writeln!(out, "{}{} = load i64, ptr {}", indent, v, ptr).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

// ─── SysCall# — raw OS syscall ───────────────────────────────────────

/// Resolve a PascalCase abstract op name to a syscall number.
/// 2026-07-15: Single mapping table for all OS operations.
/// 2026-09-13 (rv64 capability kernel): target-aware — riscv64 numbers
/// differ from x86_64 for the same abstract op.
fn resolve_syscall_number(op: &str, triple: &str) -> Option<i64> {
    if triple.starts_with("riscv64") {
        // riscv64 Linux syscall numbers (asm-generic /unistd.h)
        Some(match op {
            "Read" => 63, "Write" => 64, "Open" => 56, "Close" => 57,
            "Stat" => 179, "FStat" => 80, "LSeek" => 62, "Mmap" => 222,
            "Munmap" => 215, "Brk" => 214, "RtSigAction" => 134,
            "RtSigProcmask" => 135, "IoCtl" => 29, "Pipe" => 59,
            "SchedYield" => 124, "NanoSleep" => 101,
            "GetPid" => 172, "GetPPid" => 173, "Socket" => 198,
            "Connect" => 203, "Accept" => 202, "Send" => 211,
            "Recv" => 207, "SendTo" => 206, "RecvFrom" => 209,
            "Bind" => 200, "Listen" => 201, "Exit" => 93,
            "Fcntl" => 25, "FTruncate" => 46, "GetCwd" => 17,
            "ChDir" => 49, "MkDir" => 34, "RmDir" => 35,
            "Unlink" => 36, "Dup" => 22, "Dup2" => 23,
            "FSync" => 72, "MkDt" => 33, "ReadLink" => 78,
            "ChMod" => 55, "ChOwn" => 53, "Clone" => 220,
            "GetEgid" => 177, "GetEuid" => 175, "GetGid" => 176,
            "GetPgid" => 180, "GetSid" => 182,
            "GetSockOpt" => 204, "GetUid" => 174, "Mlock" => 221,
            "Mprotect" => 226, "SetSockOpt" => 208,
            "ShmGet" => 194, "Shutdown" => 210, "UMask" => 166,
            "ShmAt" => 195, "ShmDt" => 196, "SemGet" => 193,
            "SemOp" => 192, "SemCtl" => 191, "ClockGetTime" => 113,
            "ClockSetTime" => 114, "Futex" => 98,
            "GetRandom" => 278, "Openat" => 56,
            "Membarrier" => 283, "CopyFileRange" => 285,
            "PRead" => 67, "PWrite" => 68,
            _ => return None,
        })
    } else {
        // x86_64 Linux syscall numbers (asm/unistd_64.h)
        Some(match op {
            "Read" => 0, "Write" => 1, "Open" => 2, "Close" => 3,
            "Stat" => 4, "FStat" => 5, "LSeek" => 8, "Mmap" => 9,
            "Munmap" => 11, "Brk" => 12, "RtSigAction" => 13,
            "RtSigProcmask" => 14, "IoCtl" => 16, "Pipe" => 22,
            "SchedYield" => 24, "NanoSleep" => 35,
            "GetPid" => 39, "GetPPid" => 40, "Socket" => 41,
            "Connect" => 42, "Accept" => 43, "Send" => 44,
            "Recv" => 45, "SendTo" => 44, "RecvFrom" => 45,
            "Bind" => 49, "Listen" => 50, "Exit" => 60,
            "Fcntl" => 72, "FTruncate" => 77, "GetCwd" => 79,
            "ChDir" => 80, "MkDir" => 83, "RmDir" => 84,
            "Unlink" => 87, "Dup" => 32, "Dup2" => 33,
            "FSync" => 74, "MkDt" => 85, "ReadLink" => 89,
            "ChMod" => 90, "ChOwn" => 92, "Clone" => 56,
            "GetEgid" => 108, "GetEuid" => 107, "GetGid" => 104,
            "GetPgid" => 109, "GetSid" => 124,
            "GetSockOpt" => 55, "GetUid" => 102, "Mlock" => 149,
            "Mprotect" => 10, "SetSockOpt" => 54,
            "ShmGet" => 29, "Shutdown" => 48, "UMask" => 95,
            "ShmAt" => 30, "ShmDt" => 31, "SemGet" => 64,
            "SemOp" => 65, "SemCtl" => 66, "ClockGetTime" => 228,
            "ClockSetTime" => 229, "Futex" => 202,
            "GetRandom" => 318, "Openat" => 257,
            "Membarrier" => 324, "CopyFileRange" => 326,
            "PRead" => 17, "PWrite" => 18,
            _ => return None,
        })
    }
}

/// 2026-07-26: Emit SysCall# — first arg is op (Int raw number or PascalCase
/// abstract name), followed by up to 6 Int arguments.
/// On x86_64/aarch64 Linux: emits inline assembly (syscall/svc #0).
/// On other targets: falls back to @briev_syscall from briev_rt.c.
/// 2026-09-10 (Family F, Asm#): the two-mode asm escape hatch.
///
/// Mode 1 (abstract): `Asm#("OpName", ops...)` - the op lowers per target
/// through config/asm-lowering.dbvl. Unknown op or unsupported target =
/// loud compile error with the fix (add a lowering row / gate the call).
///
/// Mode 2 (raw): `Asm#("raw", template, ops...)` - the template is
/// dialect-specific text with `$1..$N` referencing the operands in order;
/// `$0` is the compiler-assigned result register (the i64 return).
/// Structural check: the template's highest operand ref must be covered by
/// the supplied operand count.
///
/// Constraints: result in a compiler-assigned `=r`, every operand in an
/// `r`, memory clobbered. The empty `()` operand tail is REQUIRED (LLVM
/// asm-call syntax).
fn emit_asm(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let Some(first) = args.first() else {
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    };
    let Expr::Quoted(mode_bytes) = first else {
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    };
    let mode = String::from_utf8_lossy(mode_bytes).to_string();
    let operand_args: &[Expr] = if mode == "raw" { &args[2..] } else { &args[1..] };
    let mut regs = Vec::new();
    for a in operand_args {
        let reg = emit_arg(backend, out, a, indent);
        regs.push(reg);
    }
    // Constraint string: earlyclobber output "=&r", one "r" per operand,
    // memory clobber. The & is load-bearing: without it LLVM may alias an
    // input register with $0 when the template writes $0 BEFORE reading the
    // input (e.g. `ldr $0, =sym; str $0, [$1]`) — the input silently
    // becomes the half-written output register. 2026-09-14 (rv64-finish
    // plan Phase 5): found by the ARM SysTick vector-patch template; the
    // rv64 kernel templates never read an input after writing $0, so the
    // missing & was latent there.
    let mut constraints = String::from("=&r");
    for _ in &regs {
        constraints.push_str(",r");
    }
    constraints.push_str(",~{memory}");

    let template: String = if mode == "raw" {
        // Structural check: every $N in the template must have an operand.
        let Some(Expr::Quoted(t)) = args.get(1) else {
            return BTypedRegister { name: v.to_string(), ty: Type::int() };
        };
        let t_text = String::from_utf8_lossy(t).to_string();
        let mut max_ref: i64 = -1;
        let bytes = t_text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                // $$ is an ESCAPED literal dollar (the operand-ref escape is
                // the OTHER direction: $N references operands). Skip it.
                if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let n: i64 = t_text[i + 1..j].parse().unwrap_or(0);
                    max_ref = max_ref.max(n);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        let n_ops = operand_args.len() as i64;
        // 2026-09-13 (off-by-one): valid refs are $0 (the result) plus
        // $1..$N (the operands) — a template referencing $N with N operands
        // supplied is exactly valid. The old `>=` rejected it.
        if max_ref > n_ops {
            panic!(
                "Asm# raw: the template references operand ${max_ref} but only \
                 {n_ops} operand(s) were supplied - add operands or fix the template"
            );
        }
        t_text
    } else {
        // Abstract: consult the lowering table for this op on this target.
        let triple = backend.ctx.target_triple.clone();
        let family = triple.split('-').next().unwrap_or(&triple).to_string();
        let Some(template) = ASM_LOWERING.lookup(&mode, &family) else {
            let known = ASM_LOWERING.known_ops().join(", ");
            panic!(
                "Asm#: abstract asm op '{mode}' has no '{family}' lowering - add a row to \
                 config/asm-lowering.dbvl (known ops: {known})"
            );
        };
        let arity = ASM_LOWERING.arity_of(&mode).unwrap_or(0);
        if (arity as usize) != operand_args.len() {
            panic!(
                "Asm#: op '{mode}' takes {arity} operand(s), got {} - fix the call or \
                 the arity column in config/asm-lowering.dbvl",
                operand_args.len()
            );
        }
        template.to_string()
    };

    // Emit the asm call. The result lands in $0 (a compiler-assigned reg);
    // the operands are passed so LLVM allocates/constrains them ($1..$N);
    // the i64 return discards it for no-result ops.
    let mut operand_list = String::new();
    for r in &regs {
        operand_list.push_str(&format!("i64 {}, ", r));
    }
    let operand_list = operand_list.trim_end_matches(", ");
    writeln!(out, "{}{} = call i64 asm sideeffect \"{}\", \"{}\" ({})",
        indent, v, template, constraints, operand_list).ok();
    // The asm result register IS the i64 value (the add-zero dummy would
    // redefine it).
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_syscall(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    if args.is_empty() {
        writeln!(out, "{}call void @briev_syscall()", indent).ok();
        writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
        return BTypedRegister { name: v.to_string(), ty: Type::int() };
    }
    // Resolve the syscall number from the first argument
    let num_reg = match &args[0] {
        Expr::Decimal(n) => format!("{}", n),
        Expr::Identifier(op) => {
            let n = resolve_syscall_number(op, &backend.ctx.target_triple)
                .map(|n| n.to_string())
                .unwrap_or_else(|| {
                    eprintln!("SysCall#: unknown abstract op '{}', using 0", op);
                    "0".to_string()
                });
            n
        }
        _ => {
            let reg = emit_arg(backend, out, &args[0], indent);
            reg
        }
    };
    // Emit remaining args as i64, padding to 7 total (num + 6 args)
    let mut all_args = vec![format!("i64 {}", num_reg)];
    for i in 1..args.len() {
        let reg = emit_arg(backend, out, &args[i], indent);
        all_args.push(format!("i64 {}", reg));
    }
    while all_args.len() < 7 {
        all_args.push("i64 0".to_string());
    }
    let triple = &backend.ctx.target_triple;
    if triple.starts_with("x86_64") && triple.contains("linux") {
        // x86_64 Linux: inline syscall instruction
        // syscall clobbers rcx (saves RIP) and r11 (saves RFLAGS).
        // Args in rax, rdi, rsi, rdx, r10, r8, r9. Output in rax.
        writeln!(out, "{}{} = call i64 asm sideeffect \"syscall\", \"={{rax}},{{rax}},{{rdi}},{{rsi}},{{rdx}},{{r10}},{{r8}},{{r9}},~{{rcx}},~{{r11}}\" ({}",
            indent, v, all_args.join(", ")).ok();
        writeln!(out, "{}  )", indent).ok();
    } else if triple.starts_with("aarch64") && triple.contains("linux") {
        // aarch64 Linux: inline svc #0
        // Args in x0-x5, output in x0. No clobbers beyond the ABI (kernel preserves).
        writeln!(out, "{}{} = call i64 asm sideeffect \"svc #0\", \"={{x0}},{{x0}},{{x1}},{{x2}},{{x3}},{{x4}},{{x5}}\" ({}",
            indent, v, all_args.join(", ")).ok();
        writeln!(out, "{}  )", indent).ok();
    } else if triple.starts_with("riscv64") {
        // 2026-09-13 (rv64 capability kernel): riscv64 Linux syscall ABI.
        // a7 = syscall number, a0-a5 = args, ecall instruction, result in a0.
        // On bare metal (non-linux), ecall traps to M-mode — same instruction,
        // different handler. The constraint string uses a0-a5 + a7 as inputs,
        // a0 as output. Clobbers: memory (kernel may touch any address).
        writeln!(out, "{}{} = call i64 asm sideeffect \"ecall\", \"={{a0}},{{a7}},{{a0}},{{a1}},{{a2}},{{a3}},{{a4}},{{a5}},~{{memory}}\" ({}",
            indent, v, all_args.join(", ")).ok();
        writeln!(out, "{}  )", indent).ok();
    } else {
        // Non-Linux fallback: call briev_syscall via C runtime
        writeln!(out, "{}{} = call i64 @briev_syscall({})", indent, v, all_args.join(", ")).ok();
    }
    // 2026-08-10: the syscall result is a semantic Int (i64 from the kernel /
    // C runtime) — narrow to the target int width (i32 wasm32) so it matches
    // llvm_type(Int) and feeds i32 comparisons.
    let narrowed = narrow_int_result(backend, out, v, indent);
    BTypedRegister { name: narrowed, ty: Type::int() }
}

// ─── SysConf# — runtime system configuration ──────────────────────────

/// 2026-07-15: Emit SysConf# — resolves POSIX sysconf() values at runtime.
/// First arg is a PascalCase abstract name (e.g., PageSize, CpuCount) or
/// a raw Int constant. Emits call to @briev_sysconf(i64 %name).
fn emit_sysconf(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let name_reg: String = match args.first() {
        Some(Expr::Identifier(name)) => {
            let n: i64 = match name.as_str() {
                "PageSize" => 30,
                "CpuCount" => 83,
                "HostNameMax" => 180,
                "OpenMax" => 4,
                "ArgMax" => 0,
                "ChildMax" => 1,
                "ClkTck" => 2,
                "NGroupsMax" => 3,
                _ => {
                    eprintln!("SysConf#: unknown abstract name '{}', using 0", name);
                    0
                }
            };
            n.to_string()
        }
        Some(Expr::Decimal(n)) => n.to_string(),
        Some(arg) => emit_arg(backend, out, arg, indent),
        None => "0".to_string(),
    };
    writeln!(out, "{}{} = call i64 @briev_sysconf(i64 {})", indent, v, name_reg).ok();
    let narrowed = narrow_int_result(backend, out, v, indent);
    BTypedRegister { name: narrowed, ty: Type::int() }
}

// ─── Atomic operations ───────────────────────────────────────────────
// 2026-07-15: Each maps to a single LLVM atomic instruction.
// 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): ordering is a
// trailing parameter — `AtomicLoad#(p, relaxed)`, `AtomicStore#(p, v,
// release)`, `AtomicCas#(p, old, new, bartered, relaxed)`,
// `AtomicFence#(acquire)`. Vocabulary: relaxed/acquire/release/bartered
// (= acq_rel)/seq. Default (no trailing arg) is seq_cst — backward
// compatible. The interpreter ignores ordering (single-threaded check
// mode); the vocabulary words are consumed as markers, never emitted.
// All use seq_cst ordering. The interpreter does non-atomic loads/stores
// (correct for single-threaded check mode).

/// 2026-09-06: extract an ordering keyword from `args[i]` when it is an
/// identifier naming an ordering. Returns None for any other shape —
/// callers keep seq_cst.
fn ordering_arg(args: &[Expr], i: usize) -> Option<&'static str> {
    let Expr::Identifier(name) = args.get(i)? else { return None; };
    match name.as_str() {
        "relaxed" => Some("unordered"),
        "acquire" => Some("acquire"),
        "release" => Some("release"),
        "bartered" => Some("acq_rel"),
        "seq" => Some("seq_cst"),
        _ => None,
    }
}

fn emit_atomic_load(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 1).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    writeln!(out, "{}{} = load atomic i64, ptr {} {}, align 8", indent, v, ptr, ord).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_atomic_store(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 2).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let val = emit_arg(backend, out, &args[1], indent);
    // 2026-07-15: LLVM 18 syntax: no comma before ordering, align required
    writeln!(out, "{}store atomic i64 {}, ptr {} {}, align 8", indent, val, ptr, ord).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

fn emit_atomic_cas(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let succ_ord = ordering_arg(args, 3).unwrap_or("seq_cst");
    let fail_ord = ordering_arg(args, 4).unwrap_or(succ_ord);
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let exp = emit_arg(backend, out, &args[1], indent);
    let des = emit_arg(backend, out, &args[2], indent);
    let cx = backend.fun.gen_reg();
    writeln!(out, "{}{} = cmpxchg ptr {}, i64 {}, i64 {} {} {}", indent, cx, ptr, exp, des, succ_ord, fail_ord).ok();
    // 2026-07-15: cmpxchg returns {i64, i1} — extract the value
    writeln!(out, "{}{} = extractvalue {{ i64, i1 }} {}, 0", indent, v, cx).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_atomic_xchg(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 2).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let val = emit_arg(backend, out, &args[1], indent);
    writeln!(out, "{}{} = atomicrmw xchg ptr {}, i64 {} {}", indent, v, ptr, val, ord).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_atomic_add(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    emit_atomic_rmw(backend, out, v, args, indent, "add")
}

/// 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): the atomicrmw family
/// unified — add/sub/or/and/xor share one shape: `(ptr, val, order?) -> old`.
fn emit_atomic_rmw(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str, rmw_op: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 2).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let val = emit_arg(backend, out, &args[1], indent);
    writeln!(out, "{}{} = atomicrmw {} ptr {}, i64 {} {}", indent, v, rmw_op, ptr, val, ord).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

/// 2026-09-06: width-parameterized atomic load — AtomicLoadN#(ptr, bytes,
/// order?). LLVM atomics require power-of-two sizes; 8 is the word width.
fn emit_atomic_load_n(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 2).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    // Byte count is a literal (the Load#/Store# convention — Expr::Decimal).
    let bytes = args.get(1).and_then(|a| if let Expr::Decimal(n) = a { Some(*n as usize) } else { None }).unwrap_or(8);
    let (ll_ty, align) = match bytes {
        1 => ("i8", 1),
        2 => ("i16", 2),
        4 => ("i32", 4),
        _ => ("i64", 8),
    };
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let raw = backend.fun.gen_reg();
    writeln!(out, "{}{} = load atomic {}, ptr {} {}, align {}", indent, raw, ll_ty, ptr, ord, align).ok();
    writeln!(out, "{}{} = zext {} {} to i64", indent, v, ll_ty, raw).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

/// 2026-09-06: width-parameterized atomic store — AtomicStoreN#(ptr, val,
/// bytes, order?).
fn emit_atomic_store_n(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 3).unwrap_or("seq_cst");
    let addr = emit_arg(backend, out, &args[0], indent);
    let bytes = args.get(2).and_then(|a| if let Expr::Decimal(n) = a { Some(*n as usize) } else { None }).unwrap_or(8);
    let (ll_ty, align) = match bytes {
        1 => ("i8", 1),
        2 => ("i16", 2),
        4 => ("i32", 4),
        _ => ("i64", 8),
    };
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
    let val_wide = emit_arg(backend, out, &args[1], indent);
    let val = backend.fun.gen_reg();
    writeln!(out, "{}{} = trunc i64 {} to {}", indent, val, val_wide, ll_ty).ok();
    writeln!(out, "{}store atomic {} {}, ptr {} {}, align {}", indent, ll_ty, val, ptr, ord, align).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

fn emit_fence(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ord = ordering_arg(args, 0).unwrap_or("seq_cst");
    writeln!(out, "{}fence {}", indent, ord).ok();
    writeln!(out, "{}{} = add i64 0, 0", indent, v).ok();
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}

// ─── Dynamic linker intrinsics ───────────────────────────────────────
// 2026-07-15: dlopen/dlsym/dlclose are C library functions, not syscalls.
// The backend emits calls to @dlopen/@dlsym/@dlclose which are resolved
// by the system linker at load time.

fn emit_dl_open(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let path = emit_arg(backend, out, &args[0], indent);
    let flags = emit_arg(backend, out, &args[1], indent);
    writeln!(out, "{}{} = call ptr @dlopen(ptr {}, i32 {})", indent, v, path, flags).ok();
    BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::bits(8)) }
}

fn emit_dl_sym(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let handle = emit_arg(backend, out, &args[0], indent);
    let symbol = emit_arg(backend, out, &args[1], indent);
    writeln!(out, "{}{} = call ptr @dlsym(ptr {}, ptr {})", indent, v, handle, symbol).ok();
    BTypedRegister { name: v.to_string(), ty: Type::ptr(Type::bits(8)) }
}

fn emit_dl_close(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let handle = emit_arg(backend, out, &args[0], indent);
    writeln!(out, "{}{} = call i32 @dlclose(ptr {})", indent, v, handle).ok();
    let ext = backend.fun.gen_reg();
    writeln!(out, "{}{} = sext i32 {} to i64", indent, ext, v).ok();
    BTypedRegister { name: ext.to_string(), ty: Type::int() }
}

// ─── Backtrace intrinsic ─────────────────────────────────────────────
// 2026-07-15: backtrace() walks the stack. Emits call to C runtime
// function @briev_backtrace() which uses glibc's backtrace().

fn emit_backtrace(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    writeln!(out, "{}{} = call i64 @briev_backtrace()", indent, v).ok();
    let narrowed = narrow_int_result(backend, out, v, indent);
    BTypedRegister { name: narrowed, ty: Type::int() }
}

// 2026-07-18: Deref# — load through pointer. The pointee type is resolved
// from the ptr argument's Type::Ptr(inner) and used as the LLVM load type.
fn emit_intrinsic_deref(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ptr_reg = backend.emit_expr(out, &args[0], indent);
    let inner_ty = match &ptr_reg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    writeln!(out, "{}{} = load {}, ptr {}, align {}", indent, v, llvm_ty, ptr_reg.name,
        backend.align_of(&llvm_ty)).ok();
    BTypedRegister { name: v.to_string(), ty: inner_ty }
}

// 2026-07-18: Index# — get element at index. GEP + load through pointer.
fn emit_intrinsic_index(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let obj_reg = backend.emit_expr(out, &args[0], indent);
    let idx_reg = backend.emit_expr(out, &args[1], indent);
    let inner_ty = match &obj_reg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    let gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, gep, llvm_ty, obj_reg.name, idx_reg.name).ok();
    writeln!(out, "{}{} = load {}, ptr {}, align {}", indent, v, llvm_ty, gep,
        backend.align_of(&llvm_ty)).ok();
    BTypedRegister { name: v.to_string(), ty: inner_ty }
}

// ── Cast Resolution Pipeline ───────────────────────────────────────────
// 2026-07-30: Cast#(source, target) resolved by casting graph.
// Falls back to LLVM bitcast when no graph path exists.

fn emit_intrinsic_cast(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    if args.len() < 2 { return BTypedRegister { name: v.to_string(), ty: Type::int() }; }
    let src = backend.emit_expr(out, &args[0], indent);

    // Extract target type from second argument
    let target = extract_type_from_expr(&args[1]).unwrap_or(Type::int());

    // Try casting graph path first
    if let Some(result) = backend.emit_cast_path(out, v, &src, &target, indent) {
        return BTypedRegister { name: result.name, ty: target };
    }

    // Fallback: LLVM bitcast
    let src_ll = backend.llvm_type(&src.ty);
    let target_ll = backend.llvm_type(&target);
    writeln!(out, "{}{} = bitcast {} {} to {}", indent, v, src_ll, src.name, target_ll).ok();
    BTypedRegister { name: v.to_string(), ty: target }
}

/// Extract a Type from an Expr that represents a type name.
fn extract_type_from_expr(expr: &Expr) -> Option<Type> {
    match expr {
        Expr::Identifier(name) => Some(Type::Custom(name.clone())),
        _ => None,
    }
}

// ─── Pointer arithmetic intrinsics ──────────────────────────────────
// 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): Ptr<T> arithmetic.
// PtrAdd#/PtrSub# emit GEP inbounds (out-of-bounds = UB caught by LLVM).
// PtrDiff# computes byte distance between two pointers from the same allocation.
// PtrEq#/PtrLt# are simple pointer comparisons.

fn emit_ptr_add(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // 2026-09-06: the Briev pointer ABI is uniformly BOXED i64 handles
    // (`&` ptrtoints, Malloc# boxes) — the atomics' inttoptr convention.
    // PtrAdd# re-materializes the base, GEPs inbounds (out-of-bounds = UB
    // caught by LLVM), and re-boxes. The pointee drives the GEP step.
    let ptr_reg = backend.emit_expr(out, &args[0], indent);
    let offset = backend.emit_expr(out, &args[1], indent);
    let inner_ty = match &ptr_reg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    let base = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, base, ptr_reg.name).ok();
    let gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr inbounds {}, ptr {}, i64 {}", indent, gep, llvm_ty, base, offset.name).ok();
    writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, gep).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_ptr_sub(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // Negative PtrAdd# — one GEP with the negated offset.
    let ptr_reg = backend.emit_expr(out, &args[0], indent);
    let offset = backend.emit_expr(out, &args[1], indent);
    let inner_ty = match &ptr_reg.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let llvm_ty = backend.llvm_type(&inner_ty);
    let base = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, base, ptr_reg.name).ok();
    let neg_offset = backend.fun.gen_reg();
    writeln!(out, "{}{} = sub i64 0, {}", indent, neg_offset, offset.name).ok();
    let gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr inbounds {}, ptr {}, i64 {}", indent, gep, llvm_ty, base, neg_offset).ok();
    writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, gep).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_ptr_diff(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // Byte distance between two handles, divided by the element size —
    // both pointers must derive from the same allocation (proof
    // obligation; a cross-allocation diff is meaningless, not unsafe).
    let ptr1 = backend.emit_expr(out, &args[0], indent);
    let ptr2 = backend.emit_expr(out, &args[1], indent);
    let inner_ty = match &ptr1.ty { Type::Ptr(i) => *i.clone(), _ => Type::int() };
    let byte_diff = backend.fun.gen_reg();
    writeln!(out, "{}{} = sub i64 {}, {}", indent, byte_diff, ptr1.name, ptr2.name).ok();
    let elem_size = crate::backend::llvm::types::type_size(&inner_ty, backend.ctx.type_universe.as_ref()).max(1);
    writeln!(out, "{}{} = sdiv i64 {}, {}", indent, v, byte_diff, elem_size).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

fn emit_ptr_eq(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    // Handles are i64 — compare directly (no inttoptr round-trip).
    let ptr1 = backend.emit_expr(out, &args[0], indent);
    let ptr2 = backend.emit_expr(out, &args[1], indent);
    writeln!(out, "{}{} = icmp eq i64 {}, {}", indent, v, ptr1.name, ptr2.name).ok();
    BTypedRegister { name: v.to_string(), ty: Type::bool_() }
}

fn emit_ptr_lt(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let ptr1 = backend.emit_expr(out, &args[0], indent);
    let ptr2 = backend.emit_expr(out, &args[1], indent);
    writeln!(out, "{}{} = icmp ult i64 {}, {}", indent, v, ptr1.name, ptr2.name).ok();
    BTypedRegister { name: v.to_string(), ty: Type::bool_() }
}

// ─── Portable SIMD intrinsics ───────────────────────────────────────
// 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): memory-to-memory
// element-wise intrinsics — SimdAdd#(dst, a, b, count) and siblings.
//
// WHY memory-to-memory: Briev's Type::Vector lowers to LLVM `[N x T]`
// arrays (aggregates — no arithmetic), so SSA vector registers cannot
// escape into the type model. The chunked form works entirely in the
// memory model. Element-wise chunking is OVERLAP-SAFE (per chunk, all
// loads precede the store) — dst may alias a/b, the exact case where
// the auto-vectorizer's profitability heuristics decline. The intrinsic
// FORCES the vector shape: constant count → straight-line <4 x T>
// chunks + inline scalar tail; runtime count → counted chunk loop +
// scalar tail loop (alloca induction variables — SROA promotes them to
// registers at -O3, so the loop shape costs nothing).
//
// PORTABILITY: `<4 x float>`/`<4 x i64>`/`<2 x double>` are legal LLVM
// IR on every target — ISel lowers to AVX/SSE/NEON, or scalarizes on
// targets with no vector unit. Fma emits mul+add fast pairs; with
// fast-math, ISel contracts them to vfmadd/fmadd on FMA targets and
// keeps unfused mul+add elsewhere — the portable optimal choice.

/// The SIMD op family — one shape, four arithmetic kinds.
#[derive(Clone, Copy)]
enum SimdKind { Add, Sub, Mul, Fma }

impl SimdKind {
    /// (int op, float op) for <W x T> vector arithmetic. Float uses the
    /// `fast` flag — contraction to FMA requires it.
    fn op(self, is_float: bool) -> &'static str {
        match (self, is_float) {
            (SimdKind::Add, true) => "fadd fast",
            (SimdKind::Sub, true) => "fsub fast",
            (SimdKind::Mul, true) => "fmul fast",
            (SimdKind::Fma, true) => "fmul fast",
            (SimdKind::Add, false) => "add",
            (SimdKind::Sub, false) => "sub",
            (SimdKind::Mul, false) => "mul",
            (SimdKind::Fma, false) => "mul",
        }
    }
    /// The follow-up op for Fma (mul then add); None for pure ops.
    /// Float gets the `fast` pair; int gets plain add.
    fn followup(self, is_float: bool) -> Option<&'static str> {
        match (self, is_float) {
            (SimdKind::Fma, true) => Some("fadd fast"),
            (SimdKind::Fma, false) => Some("add"),
            _ => None,
        }
    }
}

/// Alignment for a SIMD element spelling (safe-element alignment —
/// vector loads use the ELEMENT alignment, never an over-alignment the
/// base pointer may not satisfy).
fn simd_elem_align(elem: &str) -> u64 {
    match elem {
        "float" => 4,
        "double" => 8,
        _ => 8,
    }
}

/// SIMD element shape from a pointee type — (llvm elem type, vector
/// width, is_float). Storage size drives the int shapes: the intrinsic
/// adds the values AS STORED (a 12-bit field in 2 bytes adds as i16 —
/// modular on the container, correct for the buffer-add use case).
/// The typechecker gates non-scalar pointees out before codegen.
fn simd_elem_shape(ty: &Type, backend: &LlvmBackend) -> (&'static str, usize, bool) {
    let fbits = backend.float_category_bits(ty);
    match fbits {
        Some(32) => return ("float", 4, true),
        Some(64) => return ("double", 2, true),
        _ => {}
    }
    match crate::backend::llvm::types::type_size(ty, backend.ctx.type_universe.as_ref()) {
        1 => ("i8", 16, false),
        2 => ("i16", 8, false),
        4 => ("i32", 8, false),
        _ => ("i64", 4, false),
    }
}

/// Materialize a Ptr from an argument — the Briev pointer ABI is
/// uniformly boxed i64 handles (`&` ptrtoints, Malloc# boxes, and
/// `as Ptr<T>` retypes without changing the SSA value), so the base is
/// ALWAYS re-materialized via inttoptr. The atomics' convention.
fn simd_ptr_arg(
    backend: &mut LlvmBackend, out: &mut String, arg: &Expr, indent: &str,
) -> (String, Type) {
    let reg = backend.emit_expr(out, arg, indent);
    let ptr = backend.fun.gen_reg();
    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, reg.name).ok();
    (ptr, reg.ty.clone())
}

/// One vector chunk at element offset `idx_reg`: loads → compute →
/// store (that order is what makes overlapping dst/a/b safe).
#[allow(clippy::too_many_arguments)]
fn simd_vec_chunk(
    backend: &mut LlvmBackend, out: &mut String, indent: &str,
    kind: SimdKind, elem: &str, is_float: bool,
    dst: &str, srcs: &[String], idx_reg: &str,
) {
    let width = if elem == "double" { 2 } else { 4 };
    let vec_ty = format!("<{} x {}>", width, elem);
    let align = simd_elem_align(elem);
    let dst_gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, dst_gep, elem, dst, idx_reg).ok();
    let mut vecs: Vec<String> = Vec::with_capacity(srcs.len());
    for src in srcs {
        let g = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, g, elem, src, idx_reg).ok();
        let l = backend.fun.gen_reg();
        writeln!(out, "{}{} = load {}, ptr {}, align {}", indent, l, vec_ty, g, align).ok();
        vecs.push(l);
    }
    let acc = match kind.followup(is_float) {
        Some(follow) => {
            let m = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, m, kind.op(is_float), vec_ty, vecs[0], vecs[1]).ok();
            let a = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, a, follow, vec_ty, m, vecs[2]).ok();
            a
        }
        None => {
            let r = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, r, kind.op(is_float), vec_ty, vecs[0], vecs[1]).ok();
            r
        }
    };
    writeln!(out, "{}store {} {}, ptr {}, align {}", indent, vec_ty, acc, dst_gep, align).ok();
}

/// One scalar element at element offset `idx_reg` (the tail path).
fn simd_scalar_element(
    backend: &mut LlvmBackend, out: &mut String, indent: &str,
    kind: SimdKind, elem: &str, is_float: bool,
    dst: &str, srcs: &[String], idx_reg: &str,
) {
    let align = simd_elem_align(elem);
    let dst_gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, dst_gep, elem, dst, idx_reg).ok();
    let mut loads: Vec<String> = Vec::with_capacity(srcs.len());
    for src in srcs {
        let g = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, g, elem, src, idx_reg).ok();
        let l = backend.fun.gen_reg();
        writeln!(out, "{}{} = load {}, ptr {}, align {}", indent, l, elem, g, align).ok();
        loads.push(l);
    }
    let acc = match kind.followup(is_float) {
        Some(follow) => {
            let m = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, m, kind.op(is_float), elem, loads[0], loads[1]).ok();
            let a = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, a, follow, elem, m, loads[2]).ok();
            a
        }
        None => {
            let r = backend.fun.gen_reg();
            writeln!(out, "{}{} = {} {} {}, {}", indent, r, kind.op(is_float), elem, loads[0], loads[1]).ok();
            r
        }
    };
    writeln!(out, "{}store {} {}, ptr {}, align {}", indent, elem, acc, dst_gep, align).ok();
}

/// 2026-09-06: the portable SIMD binary/FMA family. Shape:
/// `SimdAdd#(dst, a, b, count)` / `SimdFma#(dst, a, b, c, count)` —
/// `dst[i] = a[i] op b[i] (+ c[i])` for i in 0..count. Constant counts
/// emit straight-line vector chunks + inline scalar tail; runtime
/// counts emit counted chunk + tail loops with alloca induction
/// variables. All pointers may alias.
fn emit_simd_binary(
    backend: &mut LlvmBackend, out: &mut String, v: &str,
    args: &[Expr], indent: &str, kind: SimdKind,
) -> BTypedRegister {
    let (dst_reg, dst_ty) = simd_ptr_arg(backend, out, &args[0], indent);
    let (a_reg, _) = simd_ptr_arg(backend, out, &args[1], indent);
    let (b_reg, _) = simd_ptr_arg(backend, out, &args[2], indent);
    let is_fma = matches!(kind, SimdKind::Fma);
    let (c_reg, srcs): (String, Vec<String>) = if is_fma {
        let (c, _) = simd_ptr_arg(backend, out, &args[3], indent);
        (c.clone(), vec![a_reg, b_reg, c])
    } else {
        (String::new(), vec![a_reg, b_reg])
    };
    let count_idx = if is_fma { 4 } else { 3 };
    let count_const = args.get(count_idx).and_then(|a| {
        if let Expr::Decimal(n) = a { Some(*n as usize) } else { None }
    });

    // Element shape from the DESTINATION pointee (results flow into dst).
    let pointee = match &dst_ty { Type::Ptr(i) => (**i).clone(), _ => Type::int() };
    let (elem, width, is_float) = simd_elem_shape(&pointee, backend);

    match count_const {
        Some(n) => {
            // Constant count: straight-line chunks + inline scalar tail.
            let chunks = n / width;
            let rem = n % width;
            for c in 0..chunks {
                let idx = backend.fun.gen_reg();
                writeln!(out, "{}{} = add i64 0, {}", indent, idx, c * width).ok();
                simd_vec_chunk(backend, out, indent, kind, elem, is_float, &dst_reg, &srcs, &idx);
            }
            for e in (chunks * width)..n {
                let idx = backend.fun.gen_reg();
                writeln!(out, "{}{} = add i64 0, {}", indent, idx, e).ok();
                simd_scalar_element(backend, out, indent, kind, elem, is_float, &dst_reg, &srcs, &idx);
            }
        }
        None => {
            // Runtime count: counted chunk loop + scalar tail loop, with
            // alloca induction variables (SROA promotes them at -O3).
            let count_reg = emit_arg(backend, out, &args[count_idx], indent);
            let i_slot = backend.fun.gen_reg();
            writeln!(out, "{}{} = alloca i64, align 8", indent, i_slot).ok();
            writeln!(out, "{}store i64 0, ptr {}", indent, i_slot).ok();
            // Label names: one counter slice per intrinsic call site
            // (the emit_alloc_tri uniqueness pattern — no gen_label API).
            let uid = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let hdr = format!("simd_vec_{}", uid);
            let body = format!("simd_body_{}", uid);
            let tail = format!("simd_tail_{}", uid);
            writeln!(out, "{}br label %{}", indent, hdr).ok();
            writeln!(out, "{}:", hdr).ok();
            let i = backend.fun.gen_reg();
            writeln!(out, "{}{} = load i64, ptr {}", indent, i, i_slot).ok();
            let limit = backend.fun.gen_reg();
            writeln!(out, "{}{} = sub i64 {}, {}", indent, limit, count_reg, width).ok();
            let cond = backend.fun.gen_reg();
            writeln!(out, "{}{} = icmp sle i64 {}, {}", indent, cond, i, limit).ok();
            writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, cond, body, tail).ok();
            writeln!(out, "{}:", body).ok();
            simd_vec_chunk(backend, out, indent, kind, elem, is_float, &dst_reg, &srcs, &i);
            let i_next = backend.fun.gen_reg();
            writeln!(out, "{}{} = add i64 {}, {}", indent, i_next, i, width).ok();
            writeln!(out, "{}store i64 {}, ptr {}", indent, i_next, i_slot).ok();
            writeln!(out, "{}br label %{}", indent, hdr).ok();
            // Scalar tail: from the first index without a full chunk.
            writeln!(out, "{}:", tail).ok();
            let j_slot = backend.fun.gen_reg();
            writeln!(out, "{}{} = alloca i64, align 8", indent, j_slot).ok();
            writeln!(out, "{}store i64 {}, ptr {}", indent, i, j_slot).ok();
            let thdr = format!("simd_scal_{}", uid);
            let tbody = format!("simd_scal_body_{}", uid);
            let end = format!("simd_end_{}", uid);
            writeln!(out, "{}br label %{}", indent, thdr).ok();
            writeln!(out, "{}:", thdr).ok();
            let j = backend.fun.gen_reg();
            writeln!(out, "{}{} = load i64, ptr {}", indent, j, j_slot).ok();
            let cond2 = backend.fun.gen_reg();
            writeln!(out, "{}{} = icmp slt i64 {}, {}", indent, cond2, j, count_reg).ok();
            writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, cond2, tbody, end).ok();
            writeln!(out, "{}:", tbody).ok();
            simd_scalar_element(backend, out, indent, kind, elem, is_float, &dst_reg, &srcs, &j);
            let j_next = backend.fun.gen_reg();
            writeln!(out, "{}{} = add i64 {}, 1", indent, j_next, j).ok();
            writeln!(out, "{}store i64 {}, ptr {}", indent, j_next, j_slot).ok();
            writeln!(out, "{}br label %{}", indent, thdr).ok();
            writeln!(out, "{}:", end).ok();
        }
    }
    let _ = c_reg;
    BTypedRegister { name: v.to_string(), ty: Type::void() }
}


fn emit_external_call(
    backend: &mut LlvmBackend, out: &mut String, v: &str, name: &str,
    args: &[Expr], indent: &str,
) -> BTypedRegister {
    let typed_regs: Vec<BTypedRegister> = args.iter()
        .map(|a| backend.emit_expr(out, a, indent))
        .collect();
    // 2026-07-25: External calls expect i64 arguments. Always pass as i64
    // to match the C ABI. The type checker may narrow to i8/i16/i32, but
    // the actual SSA value is already i64 from ptrtoint.
    let arg_strs: Vec<String> = typed_regs.iter().map(|reg| {
        format!("i64 {}", reg.name)
    }).collect();
    let clean_name = name.trim_end_matches('#');
    writeln!(out, "{}{} = call i64 @{}({})", indent, v, clean_name, arg_strs.join(", ")).ok();
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

/// 2026-08-01 (audit): the generic `Print#` convenience intrinsic — dispatch
/// the emission by the argument's protocol category, resolved via the casting
/// graph's type_to_protocol (Cast. universe properties, never type names).
/// `String` → `__print_str(ptr)`, `Char` → `__print_char`, `Bool` →
/// `__print_bool` (true/false — an explicit cast to Int is what yields 1/0),
/// `Float` → `__print_float`/`__print_float64`, else `__print_int`.
///
/// A boxed Bool/Char param is registered as `Type::int()` in SSA (its reg is
/// the boxed i64), so the category must come from the DECLARED type
/// (`let_original_types`) for identifier args — that is what carries the
/// `Bool`/`Char` protocol. Boxed scalar regs are already i64 and are passed
/// directly; native regs (i8 Bool, i32 Char) are widened to the i64 ABI.
fn emit_intrinsic_print(
    backend: &mut LlvmBackend,
    out: &mut String,
    v: &str,
    args: &[Expr],
    indent: &str,
) -> BTypedRegister {
    let a = backend.emit_expr(out, &args[0], indent);
    let dispatch_ty = match &args[0] {
        Expr::Identifier(name) => backend
            .fun
            .let_original_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| a.ty.clone()),
        _ => a.ty.clone(),
    };
    let (category, _variant) = match backend.ctx.type_universe.as_ref() {
        Some(u) => backend
            .ctx
            .casting_graph
            .as_ref()
            .map(|g| g.type_to_protocol(u, &dispatch_ty))
            .unwrap_or_else(|| ("Bit".to_string(), String::new())),
        None => ("Bit".to_string(), String::new()),
    };
    match category.as_str() {
        "String" => {
            // A Briev String value IS the ptr to a length-prefixed
            // [len][bytes] buffer; __print_str takes that pointer.
            let sym = frgn_symbol(backend, "frgn__print_str", "__print_str");
            writeln!(out, "{}{} = call i64 @{}({}ptr {})", indent, v, sym, defn_state_prefix(backend, &sym), a.name).ok();
        }
        "Char" => {
            if backend.fun.boxed_scalar_regs.contains(&a.name) {
                // A boxed Char param is already i64 — pass directly.
                let sym = frgn_symbol(backend, "frgn__print_char", "__print_char");
                writeln!(out, "{}{} = call i64 @{}({}i64 {})", indent, v, sym, defn_state_prefix(backend, &sym), a.name).ok();
            } else {
                // Native Char regs are i32 (literal/let/field/cast) —
                // widen to the i64 ABI before the call.
                let wide = backend.fun.gen_reg();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, wide, a.name).ok();
                let sym = frgn_symbol(backend, "frgn__print_char", "__print_char");
                writeln!(out, "{}{} = call i64 @{}({}i64 {})", indent, v, sym, defn_state_prefix(backend, &sym), wide).ok();
            }
        }
        "Bool" => {
            if backend.fun.boxed_scalar_regs.contains(&a.name) {
                // A boxed Bool param is already i64 0/1 — pass directly.
                let sym = frgn_symbol(backend, "frgn__print_bool", "__print_bool");
                writeln!(out, "{}{} = call i64 @{}({}i64 {})", indent, v, sym, defn_state_prefix(backend, &sym), a.name).ok();
            } else {
                // Bool regs are i8 (Expr::Bool emits `add i8 0, 1/0`);
                // widen to the i64 ABI before the call.
                let wide = backend.fun.gen_reg();
                writeln!(out, "{}{} = zext i8 {} to i64", indent, wide, a.name).ok();
                let sym = frgn_symbol(backend, "frgn__print_bool", "__print_bool");
                writeln!(out, "{}{} = call i64 @{}({}i64 {})", indent, v, sym, defn_state_prefix(backend, &sym), wide).ok();
            }
        }
        "Float" => {
            // A boxed Float param (i64 handle boxed at defn entry) must be
            // unboxed through the float cache before the call (the
            // 2026-08-01 C3 fix).
            let unboxed = backend.fun.reg_float_cache.get(&a.name).cloned()
                .unwrap_or_else(|| a.name.clone());
            let (arg_llvm, briev_name, fallback_c) = if a.ty == Type::float64() {
                ("double", "frgn__print_float64", "__print_float64")
            } else {
                ("float", "frgn__print_float", "__print_float")
            };
            let sym = frgn_symbol(backend, briev_name, fallback_c);
            writeln!(out, "{}{} = call i64 @{}({}{} {})", indent, v, sym, defn_state_prefix(backend, &sym), arg_llvm, unboxed).ok();
        }
        _ => {
            let llvm_ty = backend.llvm_type(&a.ty);
            let sym = frgn_symbol(backend, "frgn__print_int", "__print_int");
            writeln!(out, "{}{} = call i64 @{}({}{} {})", indent, v, sym, defn_state_prefix(backend, &sym), llvm_ty, a.name).ok();
        }
    }
    BTypedRegister { name: v.to_string(), ty: Type::int() }
}

/// 2026-09-08 (anti-pattern audit): resolve the C linker symbol for a print
/// runtime function through the frgn_map (declared by lib/std/ffi/io.bv as
/// `frgn frgn__print_int ... : __print_int`). Previously the C symbol was
/// hardcoded here — three copies of the same name across intrinsics.rs,
/// loop_engine/analysis.rs, and emit_stmt.rs. The `--no-stdlib` fallback is
/// the C symbol itself (keeps the intrinsic working without stdlib, per the
/// intrinsics-vs-stdlib rule). `__print_str` is the backend-declared B0
/// exception: its stdlib frgn was removed (dead + broken), so the lookup
/// always falls back for String.

/// 2026-09-09 (Family A/B, briev-native runtime): when the callee symbol is
/// a pure-Briev defn (present in defn_params), the call must pass the
/// enclosing function's %state as the hidden first parameter — every Briev
/// definition is emitted with the state pointer (emit_definition,
/// needs_state) and Briev-level call sites pass it (emit_user_call). The C
/// symbols take no state. Returns the prefix for the argument list.
fn defn_state_prefix(backend: &LlvmBackend, sym: &str) -> &'static str {
    if backend.ctx.defn_params.contains_key(sym) { "ptr %state, " } else { "" }
}

fn frgn_symbol(backend: &LlvmBackend, briev_name: &str, fallback_c: &str) -> String {
    backend.ctx.frgn_map.get(briev_name)
        .map(|sig| sig.name.clone())
        .unwrap_or_else(|| fallback_c.to_string())
}
