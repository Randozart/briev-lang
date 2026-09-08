// ── Intrinsic Signature Registry ────────────────────────────────────────
// 2026-07-14: Generic operations. Types inferred from arguments.
// Adding a new intrinsic: add one arm here + one arm in execute_intrinsic().
// Every generic op uses vec![] for param_types — type is inferred.
//
// 2026-07-15: ReturnKind replaces return_type: Option<Type> for
// backend-agnostic type dispatch. Native("Int") means "backend's native
// integer type" — LLVM maps to i64 { primitive <~ Int, bytes <~ 8 }.
// Inferred means polymorphic (return matches argument type).

use crate::ast::Type;

/// How an intrinsic's return type is determined.
/// 2026-07-15: Backend-agnostic type dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnKind {
    /// Backend-native type — exact representation depends on target.
    /// LLVM: #Int → i64 { primitive <~ Int, bytes <~ 8 }
    ///       #Float → double { primitive <~ Float, bytes <~ 8 }
    Native(&'static str),
    /// Inferred from argument types (e.g. Add# returns same as input).
    Inferred,
    /// Fixed concrete type (pointer, void, etc.).
    Exact(Type),
    /// 2026-08-17 (plan 2026-08-17-error-intrinsic-piggybank-hashmap-completion.md):
    /// the intrinsic NEVER returns (e.g. `Error#`). A body ending in a Never
    /// call typechecks as any return type without a trailing `term`.
    Never,
}

/// The signature of a compiler-known # intrinsic.
pub struct Signature {
    pub name: &'static str,
    pub parameters: Vec<(&'static str, Type)>,
    pub return_kind: ReturnKind,
    /// If true, this intrinsic has observable side effects (DCE guard).
    pub observable: bool,
    /// If true, this intrinsic accepts additional arguments beyond declared
    /// parameters (e.g., SysCall# takes num + up to 6 more). Default false.
    /// 2026-07-26: Added for variadic intrinsics to avoid false arity errors.
    pub variadic: bool,
}

/// Look up the signature of a # intrinsic by name.
pub fn get_intrinsic_signature(name: &str) -> Option<Signature> {
    match name {
        // 2026-08-17 (plan 2026-08-17-error-intrinsic-piggybank-hashmap-completion.md):
        // `Error#("msg")` is a COMPILE-TIME failure — a reachable error means
        // the program does not compile (the message is the diagnostic); a
        // provably-unreachable one is dead code, eliminated. The typechecker
        // handles the reachability + message; this registers the name so
        // `infer_call` routes to it and Never makes an error-only body
        // typecheck as any return type.
        "Error#" => Some(Signature {
            name: "Error#", parameters: vec![("message", Type::string())],
            return_kind: ReturnKind::Never, observable: false, variadic: false,
        }),
        // ── Arithmetic (return matches input type) ────────────────────
        "Add#" => Some(Signature { name: "Add#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Sub#" => Some(Signature { name: "Sub#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Mul#" => Some(Signature { name: "Mul#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Div#" => Some(Signature { name: "Div#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Rem#" => Some(Signature { name: "Rem#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Neg#" => Some(Signature { name: "Neg#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Abs#" => Some(Signature { name: "Abs#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),

        // ── Comparison (returns Bool, but type-inferred) ──────────────
        "Eq#"  => Some(Signature { name: "Eq#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Neq#" => Some(Signature { name: "Neq#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Lt#"  => Some(Signature { name: "Lt#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Gt#"  => Some(Signature { name: "Gt#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Le#"  => Some(Signature { name: "Le#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Ge#"  => Some(Signature { name: "Ge#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),

        // ── Bitwise (return matches input type) ─────────────────────────
        "BitAnd#" => Some(Signature { name: "BitAnd#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "BitOr#" => Some(Signature { name: "BitOr#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "BitXor#" => Some(Signature { name: "BitXor#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Shl#" => Some(Signature { name: "Shl#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Shr#" => Some(Signature { name: "Shr#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "BitNot#" => Some(Signature { name: "BitNot#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // ── Bit manipulation (SPEC §17.3; vocab:260 lists the names, the
        //   signatures were the gap) ───────────────────────────────────────
        "BitReverse#" => Some(Signature { name: "BitReverse#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Popcount#" => Some(Signature { name: "Popcount#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "LeadingZeros#" => Some(Signature { name: "LeadingZeros#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "TrailingZeros#" => Some(Signature { name: "TrailingZeros#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // ── Logical (unary, no short-circuit) ────────────────────────────
        "Not#" => Some(Signature { name: "Not#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // ── Pointer operations ─────────────────────────────────────────
        "Deref#" => Some(Signature { name: "Deref#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Index#" => Some(Signature { name: "Index#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Ptr#" => Some(Signature { name: "Ptr#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): pointer arithmetic
        "PtrAdd#" => Some(Signature { name: "PtrAdd#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "PtrSub#" => Some(Signature { name: "PtrSub#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "PtrDiff#" => Some(Signature { name: "PtrDiff#", parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "PtrEq#" => Some(Signature { name: "PtrEq#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "PtrLt#" => Some(Signature { name: "PtrLt#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // ── Capacity (coll plan §3.6; compiler-owned coll capacity) ─────
        // Capacity#(h) -> Int — read the hidden cap slot.
        "Capacity#" => Some(Signature { name: "Capacity#", parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        // Resize#(h, cap), EnsureCap#(h, n), TrimCap#(h) — capacity writes
        // (return void; the coll handle is mutated in place).
        "Resize#" => Some(Signature { name: "Resize#", parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: false, variadic: false }),
        "EnsureCap#" => Some(Signature { name: "EnsureCap#", parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: false, variadic: false }),
        "TrimCap#" => Some(Signature { name: "TrimCap#", parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: false, variadic: false }),

        // ── Float math (returns native Float) ─────────────────────────
        "Sqrt#"  => Some(Signature { name: "Sqrt#",  parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Sin#"   => Some(Signature { name: "Sin#",   parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Cos#"   => Some(Signature { name: "Cos#",   parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Fabs#"  => Some(Signature { name: "Fabs#",  parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Ceil#"  => Some(Signature { name: "Ceil#",  parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Floor#" => Some(Signature { name: "Floor#", parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Exp#"   => Some(Signature { name: "Exp#",   parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "Pow#"   => Some(Signature { name: "Pow#",   parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),

        // ── Runtime (observable) ────────────────────────────────────
        // 2026-07-19: GetEnv#/GetEnvInt# moved to stdlib env.bv via ! plugin.
        // 2026-07-28: one generic `Print#` (2026-08-01 audit) — the backend
        // dispatches the emission by the argument's protocol category
        // (#String → __print_str, #Float → __print_float, else __print_int).
        // Empty parameters = type-inferred (any arg); observable prevents DCE.
        // PrintChar# remains the INTERNAL newline/char primitive (there is no
        // distinct Char type — a char is an Int code point, so it cannot be
        // type-dispatched; the print plugin's newline uses it).
        "Print#"     => Some(Signature { name: "Print#",     parameters: vec![], return_kind: ReturnKind::Exact(Type::int()), observable: true, variadic: false }),

        // ── Memory (observable) ─────────────────────────────────────
"Malloc#"  => Some(Signature { name: "Malloc#",  parameters: vec![("size", Type::int())], return_kind: ReturnKind::Exact(Type::ptr(Type::bits(8))), observable: true, variadic: false }),
        "Alloc#"   => Some(Signature { name: "Alloc#",   parameters: vec![], return_kind: ReturnKind::Exact(Type::ptr(Type::bits(8))), observable: true, variadic: false }),
        "Free#"    => Some(Signature { name: "Free#",    parameters: vec![("ptr", Type::ptr(Type::bits(8)))], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),
        "Load#"    => Some(Signature { name: "Load#",    parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: true, variadic: false }),
        "Store#"   => Some(Signature { name: "Store#",   parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),
        // 2026-08-27 (plan 2026-08-27-cbv-foreign-hardware-and-mmio.md Slice C):
        // typed MMIO access — pointee width comes from the Ptr<T> argument's
        // element type (NOT a byte-count argument like raw Load#/Store#).
        // Rule 4 naming: PascalCase + #. VolatileStore# yields Bool (success
        // by construction on targets that reach it; call sites use it as an
        // expression statement or bind the result).
        "VolatileLoad#"  => Some(Signature { name: "VolatileLoad#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: true, variadic: false }),
        "VolatileStore#" => Some(Signature { name: "VolatileStore#", parameters: vec![], return_kind: ReturnKind::Exact(Type::bool_()), observable: true, variadic: false }),
        "Copy#"    => Some(Signature { name: "Copy#",    parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),
        "Fill#"    => Some(Signature { name: "Fill#",    parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),

        // ── String ───────────────────────────────────────────────────
        "Concat#"    => Some(Signature { name: "Concat#",    parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        // ── Value-category semantics ──────────────────────────────────
        // 2026-08-12 (Iterable protocol): the UTF8 CHAR count of a String —
        // a COMPUTED property (the scan), so it is an intrinsic, not
        // reflection. `.^Length` is the stored byte count (SPEC §17.1).
        "CharCount#" => Some(Signature { name: "CharCount#", parameters: vec![("s", Type::string())], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "Length#"    => Some(Signature { name: "Length#",    parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "ToInt#"     => Some(Signature { name: "ToInt#",     parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "ToFloat#"   => Some(Signature { name: "ToFloat#",   parameters: vec![], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "ToString#"  => Some(Signature { name: "ToString#",  parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),

        // ── Collection ───────────────────────────────────────────────
        "Get#"    => Some(Signature { name: "Get#",    parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Insert#" => Some(Signature { name: "Insert#", parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),
        // 2026-08-14 (UOL §6b): the intrinsic forms of the disclosed collection
        // ops — `Count#`/`At#`/`Slice#`/`InsertAt#`/`ExtractFrom#`/`CopyFrom#`
        // dispatch to the type's declared op members (generative rule, §6b.2).
        // Signatures give precise arity errors; the typechecker infers the
        // receiver/return from the op member. `observable` marks the mutating
        // forms (DCE guard, §6b.3). (The legacy `Get#`/`Insert#` are
        // reconciled with the op surface by the §11 intrinsic audit.)
        "Count#"      => Some(Signature { name: "Count#",      parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "At#"         => Some(Signature { name: "At#",         parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "Slice#"      => Some(Signature { name: "Slice#",      parameters: vec![], return_kind: ReturnKind::Inferred, observable: false, variadic: false }),
        "InsertAt#"   => Some(Signature { name: "InsertAt#",   parameters: vec![], return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false }),
        "ExtractFrom#" => Some(Signature { name: "ExtractFrom#", parameters: vec![], return_kind: ReturnKind::Inferred, observable: true, variadic: false }),
        "CopyFrom#"   => Some(Signature { name: "CopyFrom#",   parameters: vec![], return_kind: ReturnKind::Inferred, observable: true, variadic: false }),

        // ── GPU ───────────────────────────────────────────────────────
        "GetGlobalId#"   => Some(Signature { name: "GetGlobalId#",   parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "SubgroupFAdd#"  => Some(Signature { name: "SubgroupFAdd#",  parameters: vec![("v", Type::float())], return_kind: ReturnKind::Native("Float"), observable: false, variadic: false }),
        "GetGlobalSize#" => Some(Signature { name: "GetGlobalSize#", parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "GetLocalId#"    => Some(Signature { name: "GetLocalId#",    parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "WorkgroupSize#" => Some(Signature { name: "WorkgroupSize#", parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        // 2026-07-15: Additional GPU intrinsics
        "GetGroupId#"    => Some(Signature { name: "GetGroupId#",    parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        "GetNumGroups#"  => Some(Signature { name: "GetNumGroups#",  parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),
        // 2026-08-23: workgroup barrier — GPU-target only; CPU fallback is a
        // no-op (single-threaded execution trivially passes the barrier).
        "Barrier#"       => Some(Signature { name: "Barrier#",       parameters: vec![], return_kind: ReturnKind::Exact(Type::bool_()), observable: true, variadic: false }),
        // 2026-08-23 (process.bv revival): process/environment intrinsics.
        "Spawn#"             => Some(Signature { name: "Spawn#",             parameters: vec![("command", Type::string())], return_kind: ReturnKind::Exact(Type::int()), observable: true, variadic: false }),
        "SpawnWithOutput#"   => Some(Signature { name: "SpawnWithOutput#",   parameters: vec![("command", Type::string())], return_kind: ReturnKind::Exact(Type::string()), observable: true, variadic: false }),
        "SetEnv#"            => Some(Signature { name: "SetEnv#",            parameters: vec![("name", Type::string()), ("value", Type::string())], return_kind: ReturnKind::Exact(Type::int()), observable: true, variadic: false }),
        "GetCwd#"            => Some(Signature { name: "GetCwd#",            parameters: vec![], return_kind: ReturnKind::Exact(Type::string()), observable: true, variadic: false }),
        "ChDir#"             => Some(Signature { name: "ChDir#",             parameters: vec![("path", Type::string())], return_kind: ReturnKind::Exact(Type::int()), observable: true, variadic: false }),
        "Dims#"          => Some(Signature { name: "Dims#",          parameters: vec![], return_kind: ReturnKind::Native("Int"), observable: false, variadic: false }),

        // ── Pointers (compile-time address resolution) ──────────────
        "AddressOf#" => Some(Signature {
            name: "AddressOf#",
            parameters: vec![("id", Type::string())],
            return_kind: ReturnKind::Exact(Type::ptr(Type::bits(8))),
            observable: false,
            variadic: false,
        }),

        // ── Callbacks ────────────────────────────────────────────────
        // 2026-08-03: call a function-pointer value: CallPtr#(cb, args...).
        // `cb` is a fn(...) value (opaque pointer across the FFI boundary).
        // Fully variadic — the typechecker infers the return from cb's fn
        // type (see ReturnKind::Inferred special-case).
        "CallPtr#" => Some(Signature {
            name: "CallPtr#",
            parameters: vec![],
            return_kind: ReturnKind::Inferred,
            observable: false,
            variadic: true,
        }),

        // ── Host cancellation ─────────────────────────────────────────
        // 2026-08-03: per-process cancel flag the host can raise via
        // __briev_set_cancel. A long-running Briev loop polls
        // CancelRequested#() explicitly (no implicit polling) and bails.
        "CancelRequested#" => Some(Signature {
            name: "CancelRequested#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Bool"),
            observable: false,
            variadic: false,
        }),
        "ClearCancel#" => Some(Signature {
            name: "ClearCancel#",
            parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()),
            observable: false,
            variadic: false,
        }),

        // ── OS SysCall (observable, variadic) ──────────────────────────
        // 2026-07-15: Returns native Int (result or errno).
        // 2026-07-26: variadic — first arg is syscall number, up to 6 more args.
        "SysCall#" => Some(Signature {
            name: "SysCall#",
            parameters: vec![("num", Type::int())],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: true,
        }),

        // ── POSIX SysConf (observable) ─────────────────────────────────
        // 2026-07-15: Returns native Int (page size, CPU count, etc.).
        "SysConf#" => Some(Signature {
            name: "SysConf#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),

        // ── Atomic operations (LLVM atomicrmw / load atomic / fence) ──
        // 2026-07-15: All return native Int except Fence# (void).
        "AtomicLoad#" => Some(Signature {
            name: "AtomicLoad#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: false,
            variadic: false,
        }),
        "AtomicStore#" => Some(Signature {
            name: "AtomicStore#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicCas#" => Some(Signature {
            name: "AtomicCas#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicXchg#" => Some(Signature {
            name: "AtomicXchg#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicAdd#" => Some(Signature {
            name: "AtomicAdd#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): the RMW family
        // completion + width-parameterized load. All accept a trailing
        // ordering argument (relaxed/acquire/release/bartered/seq).
        "AtomicSub#" => Some(Signature {
            name: "AtomicSub#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicOr#" => Some(Signature {
            name: "AtomicOr#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicAnd#" => Some(Signature {
            name: "AtomicAnd#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        "AtomicXor#" => Some(Signature {
            name: "AtomicXor#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),
        // AtomicLoadN#(ptr, bytes, order?) — 1/2/4/8-byte atomic load.
        // LLVM atomics require power-of-two sizes up to word width.
        "AtomicLoadN#" => Some(Signature {
            name: "AtomicLoadN#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: false,
            variadic: false,
        }),
        // AtomicStoreN#(ptr, val, bytes, order?) — 1/2/4/8-byte atomic store.
        "AtomicStoreN#" => Some(Signature {
            name: "AtomicStoreN#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),

        // ── Portable SIMD (plan 2026-09-06-cpp-expressiveness.md) ─────
        // 2026-09-06: memory-to-memory element-wise intrinsics —
        // SimdAdd#(dst, a, b, count) etc. Chunked <4 x T> vector ops with a
        // scalar tail; dst may alias a/b (overlap-safe chunking). Forced
        // vectorization for the cases the auto-vectorizer's profitability
        // heuristics decline (unknown aliasing). Void return (results flow
        // through memory).
        "SimdAdd#" => Some(Signature {
            name: "SimdAdd#", parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false,
        }),
        "SimdSub#" => Some(Signature {
            name: "SimdSub#", parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false,
        }),
        "SimdMul#" => Some(Signature {
            name: "SimdMul#", parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false,
        }),
        "SimdFma#" => Some(Signature {
            name: "SimdFma#", parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()), observable: true, variadic: false,
        }),
        "Fence#" => Some(Signature {
            name: "Fence#",
            parameters: vec![],
            return_kind: ReturnKind::Exact(Type::void()),
            observable: true,
            variadic: false,
        }),

        // ── Dynamic linker (platform library functions) ──────────────
        // 2026-07-15: Return pointers (DlOpen#, DlSym#) or Int (DlClose#).
        "DlOpen#" => Some(Signature {
            name: "DlOpen#",
            parameters: vec![],
            return_kind: ReturnKind::Exact(Type::ptr(Type::bits(8))),
            observable: true,
            variadic: false,
        }),
        "DlSym#" => Some(Signature {
            name: "DlSym#",
            parameters: vec![],
            return_kind: ReturnKind::Exact(Type::ptr(Type::bits(8))),
            observable: false,
            variadic: false,
        }),
        "DlClose#" => Some(Signature {
            name: "DlClose#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),

        // ── String operations (runtime + compile-time) ──────────────
        // 2026-07-25: Migrated from $ intrinsics — usable at runtime.
        "StrSplit#" => Some(Signature {
            name: "StrSplit#",
            parameters: vec![("s", Type::string()), ("pat", Type::string())],
            return_kind: ReturnKind::Inferred,
            observable: false,
            variadic: false,
        }),
        "EnvGet#" => Some(Signature {
            name: "EnvGet#",
            parameters: vec![("name", Type::string())],
            return_kind: ReturnKind::Inferred,
            observable: false,
            variadic: false,
        }),

        // ── System queries (observable — reads host state) ──────────
        "SysQuery#" => Some(Signature {
            name: "SysQuery#",
            parameters: vec![("query", Type::string())],
            return_kind: ReturnKind::Inferred,
            observable: false,
            variadic: false,
        }),
        "TimeNow#" => Some(Signature {
            name: "TimeNow#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: false,
            variadic: false,
        }),

        // ── External I/O (observable — side effects) ────────────────
        "HttpFetch#" => Some(Signature {
            name: "HttpFetch#",
            parameters: vec![("url", Type::string())],
            return_kind: ReturnKind::Inferred,
            observable: true,
            variadic: false,
        }),
        "ShellCmd#" => Some(Signature {
            name: "ShellCmd#",
            parameters: vec![("cmd", Type::string())],
            return_kind: ReturnKind::Inferred,
            observable: true,
            variadic: false,
        }),

        // ── Debugging (stack trace) ──────────────────────────────────
        // 2026-07-15: Returns frame count (native Int).
        "Backtrace#" => Some(Signature {
            name: "Backtrace#",
            parameters: vec![],
            return_kind: ReturnKind::Native("Int"),
            observable: true,
            variadic: false,
        }),

        _ => None,
    }
}

/// 2026-09-08: every name the registry serves. Kept in one place so the
/// master-syntax-reference completeness test (src/vocab.rs) can iterate the
/// registry without string-scraping the match. MUST be updated when an
/// intrinsic is added — the test that consumes it will not find the new name
/// in the doc otherwise, which is exactly the drift it exists to catch.
pub const REGISTERED_INTRINSICS: &[&str] = &[
    "Error#",
    "Add#", "Sub#", "Mul#", "Div#", "Rem#", "Neg#", "Abs#",
    "Eq#", "Neq#", "Lt#", "Gt#", "Le#", "Ge#",
    "BitAnd#", "BitOr#", "BitXor#", "Shl#", "Shr#", "BitNot#",
    "BitReverse#", "Popcount#", "LeadingZeros#", "TrailingZeros#",
    "Not#",
    "Deref#", "Index#", "Ptr#", "PtrAdd#", "PtrSub#", "PtrDiff#", "PtrEq#", "PtrLt#",
    "Capacity#", "Resize#", "EnsureCap#", "TrimCap#",
    "Sqrt#", "Sin#", "Cos#", "Fabs#", "Ceil#", "Floor#", "Exp#", "Pow#",
    "Print#",
    "Malloc#", "Alloc#", "Free#", "Load#", "Store#",
    "VolatileLoad#", "VolatileStore#", "Copy#", "Fill#",
    "Concat#", "CharCount#", "Length#", "ToInt#", "ToFloat#", "ToString#",
    "Get#", "Insert#",
    "Count#", "At#", "Slice#", "InsertAt#", "ExtractFrom#", "CopyFrom#",
    "GetGlobalId#", "GetGlobalSize#", "GetLocalId#", "WorkgroupSize#",
    "GetGroupId#", "GetNumGroups#", "Dims#", "SubgroupFAdd#", "Barrier#",
    "Spawn#", "SpawnWithOutput#", "SetEnv#", "GetCwd#", "ChDir#",
    "AddressOf#", "CallPtr#",
    "CancelRequested#", "ClearCancel#",
    "SysCall#", "SysConf#",
    "AtomicLoad#", "AtomicStore#", "AtomicCas#", "AtomicXchg#", "AtomicAdd#",
    "AtomicSub#", "AtomicOr#", "AtomicAnd#", "AtomicXor#",
    "AtomicLoadN#", "AtomicStoreN#",
    "SimdAdd#", "SimdSub#", "SimdMul#", "SimdFma#",
    "Fence#",
    "DlOpen#", "DlSym#", "DlClose#",
    "StrSplit#", "EnvGet#", "SysQuery#", "TimeNow#",
    "HttpFetch#", "ShellCmd#",
    "Backtrace#",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_intrinsics_have_signatures() {
        let intrinsics = [
            "Add#", "Sub#", "Mul#", "Div#", "Rem#", "Neg#", "Abs#",
            "BitReverse#", "Popcount#", "LeadingZeros#", "TrailingZeros#",
            "Eq#", "Neq#", "Lt#", "Gt#", "Le#", "Ge#",
            "Sqrt#", "Sin#", "Cos#", "Fabs#", "Ceil#", "Floor#", "Exp#", "Pow#",
            "Malloc#", "Alloc#", "Free#", "Load#", "Store#", "Copy#", "Fill#",
            "VolatileLoad#", "VolatileStore#",
            "Concat#", "Length#", "ToInt#", "ToFloat#", "ToString#",
            "CharCount#",
            "Get#", "Insert#",
            // 2026-08-14 (UOL §6b): the collection-op intrinsic forms.
            "Count#", "At#", "Slice#", "InsertAt#", "ExtractFrom#", "CopyFrom#",
            // 2026-09-01 (plan 2026-09-01-cooperative-row-kernels): the
        // SPIR-V backend lowers this to OpGroupNonUniformFAdd (subgroup
        // scope) — bit-exact fixed-tree reduction, no atomics.
        "SubgroupFAdd#",        "GetGlobalId#", "GetGlobalSize#", "GetLocalId#", "WorkgroupSize#",
            "GetGroupId#", "GetNumGroups#", "Dims#",
            "AddressOf#", "SysCall#", "SysConf#",
            "AtomicLoad#", "AtomicStore#", "AtomicCas#", "AtomicXchg#", "AtomicAdd#", "Fence#",
            "DlOpen#", "DlSym#", "DlClose#",
            "Backtrace#",
            "StrSplit#", "EnvGet#", "SysQuery#", "TimeNow#", "HttpFetch#", "ShellCmd#",
        ];
        for name in &intrinsics {
            let sig = get_intrinsic_signature(name);
            assert!(sig.is_some(), "missing signature for {}", name);
        }
    }

    #[test]
    fn test_unknown_intrinsic_returns_none() {
        assert!(get_intrinsic_signature("NonExistent#").is_none());
        assert!(get_intrinsic_signature("AddI64#").is_none());
        assert!(get_intrinsic_signature("").is_none());
        assert!(get_intrinsic_signature("user_function").is_none());
    }

    #[test]
    fn test_observable_intrinsics() {
        assert!(get_intrinsic_signature("Malloc#").unwrap().observable);
        assert!(get_intrinsic_signature("Free#").unwrap().observable);
        assert!(get_intrinsic_signature("Load#").unwrap().observable);
        assert!(get_intrinsic_signature("Copy#").unwrap().observable);
        assert!(get_intrinsic_signature("Insert#").unwrap().observable);
        assert!(!get_intrinsic_signature("Add#").unwrap().observable);
        assert!(!get_intrinsic_signature("Eq#").unwrap().observable);
        assert!(get_intrinsic_signature("AtomicStore#").unwrap().observable);
        assert!(get_intrinsic_signature("DlOpen#").unwrap().observable);
        assert!(get_intrinsic_signature("Backtrace#").unwrap().observable);
    }

    #[test]
    fn test_address_of_signature() {
        let sig = get_intrinsic_signature("AddressOf#").unwrap();
        assert_eq!(sig.name, "AddressOf#");
        assert_eq!(sig.parameters.len(), 1);
        assert_eq!(sig.parameters[0].0, "id");
        assert!(!sig.observable);
        assert!(matches!(sig.return_kind, ReturnKind::Exact(_)));
    }
}
