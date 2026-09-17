// ── Intrinsic Execution ────────────────────────────────────────────────
// 2026-07-14: Generic operations. For polymorphic ops (Add#, Eq#, etc.)
// the implementation checks argument types to dispatch int vs float.
// Flat dispatch: one match arm per intrinsic name, first match wins.

use crate::errors::RuntimeError;
use crate::interpreter::{Atom, bool_to_bits, f64_to_bits, i64_to_bits, zero_bits, Value, VirtualHeap};

// 2026-07-15: SysCall# abstract op → syscall number mapping (x86_64)
fn resolve_syscall_number_interp(name: &str) -> Option<i64> {
    Some(match name {
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

/// Execute a named intrinsic with the given evaluated arguments.
pub fn execute_intrinsic(
    name: &str,
    args: &[Value],
    heap: &mut VirtualHeap,
) -> Result<Value, RuntimeError> {
    match name {
        // 2026-08-12 (Iterable protocol): `CharCount#(s)` — the UTF8 CHAR
        // count of a String (a computed property, so an intrinsic; `.^Length`
        // is the stored byte count).
        "CharCount#" => {
            let s = args.first().ok_or_else(|| crate::errors::RuntimeError::HeapError(
                "CharCount# takes one argument".into(),
            ))?;
            let bytes = s.string_bytes(heap).unwrap_or_default();
            let chars = bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count();
            Ok(Value::int(chars as i64))
        }
        // 2026-08-14 (UOL §6b): the collection-op intrinsic forms — value
        // parity with the codegen's op-member dispatch. A collection is a
        // `Value::Product` (fields = the element sequence or the collection's
        // slots); `Count#` is its field count, `At#` the field at an index,
        // `Slice#` a positional sub-product, and the mutation ops read/write
        // the field list. `Count#` on a String value is the char count (the
        // `String` case the codegen routes to `CharCount#`).
        "Count#" => {
            let v = args.first().ok_or_else(|| crate::errors::RuntimeError::HeapError(
                "Count# takes one argument".into(),
            ))?;
            match v {
                Value::Product { fields, .. } => Ok(Value::int(fields.len() as i64)),
                Value::Bits(bytes) => {
                    let chars = bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count();
                    Ok(Value::int(chars as i64))
                }
                _ => Ok(Value::int(0)),
            }
        }
        // 2026-08-15 (coll plan §3.6, ambiguity #6): the capacity intrinsics.
        // The interpreter's collection value is a Product (a Vec) with no
        // capacity concept — `Capacity#(product)` is its field count (a Vec is
        // exact-fit); the write forms are NO-OPs (a Vec grows freely, capacity
        // is not observable). Parity with the codegen's hidden cap slot.
        "Capacity#" => {
            let v = args.first().ok_or_else(|| crate::errors::RuntimeError::HeapError(
                "Capacity# takes one argument".into(),
            ))?;
            match v {
                Value::Product { fields, .. } => Ok(Value::int(fields.len() as i64)),
                _ => Ok(Value::int(0)),
            }
        }
        "Resize#" | "EnsureCap#" | "TrimCap#" => {
            // No-op: capacity is not observable in the interpreter's exact-fit
            // Product representation (a Vec grows freely). Returns Void,
            // matching the codegen's void return kind.
            let _ = args.first();
            Ok(Value::Void)
        }
        "At#" => {
            if args.len() < 2 {
                return Err(RuntimeError::HeapError("At# takes (collection, index)".into()));
            }
            match &args[0] {
                Value::Product { fields, .. } => {
                    let i = args[1].as_i64().ok_or_else(|| RuntimeError::HeapError("At# index must be Int".into()))?;
                    fields.get(i as usize).cloned().ok_or_else(|| RuntimeError::HeapError("At# index out of range".into()))
                }
                _ => Err(RuntimeError::HeapError("At# receiver must be a collection".into())),
            }
        }
        "Slice#" => {
            if args.len() < 3 {
                return Err(RuntimeError::HeapError("Slice# takes (collection, lo, hi)".into()));
            }
            match &args[0] {
                Value::Product { fields, .. } => {
                    let lo = args[1].as_i64().unwrap_or(0) as usize;
                    let hi = args[2].as_i64().unwrap_or(fields.len() as i64) as usize;
                    Ok(Value::Product {
                        fields: fields[lo.min(fields.len())..hi.min(fields.len())].to_vec(),
                        names: None,
                    })
                }
                _ => Err(RuntimeError::HeapError("Slice# receiver must be a collection".into())),
            }
        }
        "InsertAt#" => {
            // 2026-08-14 (UOL §6b): value-based interpreter — the receiver is
            // immutable (`&[Value]`), so an in-place push is unrepresentable.
            // Return Void (the caller's rebind in real codegen mutates state);
            // the collection-op VALUE parity for reads is what the interpreter
            // exercises (Count#/At#/Slice#).
            if args.len() < 2 {
                return Err(RuntimeError::HeapError("InsertAt# takes (collection, value)".into()));
            }
            Ok(Value::Void)
        }
        "ExtractFrom#" | "CopyFrom#" => {
            let v = args.first().ok_or_else(|| RuntimeError::HeapError("ExtractFrom# takes a collection".into()))?;
            match v {
                Value::Product { fields, .. } => {
                    let last = fields.last().cloned().ok_or_else(|| RuntimeError::HeapError("extract from empty collection".into()))?;
                    Ok(last)
                }
                _ => Err(RuntimeError::HeapError("ExtractFrom# receiver must be a collection".into())),
            }
        }
        // ── Arithmetic (type-polymorphic) ───────────────────────────
        "Add#" => exec_binop(args, |a, b| a + b, |a, b| a + b),
        "Sub#" => exec_binop(args, |a, b| a - b, |a, b| a - b),
        "Mul#" => exec_binop(args, |a, b| a * b, |a, b| a * b),
        "Div#" => exec_div(args),
        "Rem#" => exec_rem(args),
        "Neg#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.wrapping_neg()))
        }
        "Abs#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.wrapping_abs()))
        }
        // 2026-08-14 (boundary plan, SPEC §17.3): the four bit intrinsics —
        // interpreter parity with the LLVM llvm.bitreverse/ctpop/ctlz/cttz
        // lanes (rule #4). All operate on the i64 word.
        "BitReverse#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.reverse_bits()))
        }
        "Popcount#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.count_ones() as i64))
        }
        "LeadingZeros#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.leading_zeros() as i64))
        }
        "TrailingZeros#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(a.trailing_zeros() as i64))
        }

        // ── Comparison (type-polymorphic) ───────────────────────────
        "Eq#"  => exec_cmp(args, |a, b| a == b, |a, b| (a - b).abs() < 1e-10),
        "Neq#" => exec_cmp(args, |a, b| a != b, |a, b| (a - b).abs() >= 1e-10),
        "Lt#"  => exec_cmp(args, |a, b| a < b,  |a, b| a < b),
        "Gt#"  => exec_cmp(args, |a, b| a > b,  |a, b| a > b),
        "Le#"  => exec_cmp(args, |a, b| a <= b, |a, b| a <= b),
        "Ge#"  => exec_cmp(args, |a, b| a >= b, |a, b| a >= b),

        // ── Bitwise (int-only — no float path) ────────────────────────
        "BitAnd#" => { let a = arg_as_i64(args, 0)?; let b = arg_as_i64(args, 1)?; Ok(i64_to_bits(a & b)) }
        "BitOr#" => { let a = arg_as_i64(args, 0)?; let b = arg_as_i64(args, 1)?; Ok(i64_to_bits(a | b)) }
        "BitXor#" => { let a = arg_as_i64(args, 0)?; let b = arg_as_i64(args, 1)?; Ok(i64_to_bits(a ^ b)) }
        "Shl#" => {
            let a = arg_as_i64(args, 0)?;
            let b = arg_as_i64(args, 1)?;
            Ok(i64_to_bits(a.wrapping_shl(b as u32)))
        }
        "Shr#" => {
            let a = arg_as_i64(args, 0)?;
            let b = arg_as_i64(args, 1)?;
            Ok(i64_to_bits(a.wrapping_shr(b as u32)))
        }
        "BitNot#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(!a))
        }
        // ── Logical ──────────────────────────────────────────────────
        "Not#" => {
            let a = arg_as_i64(args, 0)?;
            Ok(i64_to_bits(if a == 0 { 1 } else { 0 }))
        }
        // ── Pointer operations (interpreter: evaluate inner) ──────────
        "Deref#" => {
            let ptr_val = args.get(0).cloned().unwrap_or(Value::Atom(Atom::Int(0)));
            // In the interpreter, Deref# just returns the value (identity).
            Ok(ptr_val)
        }
        "Cast#" => {
            // Cast# is identity in the interpreter — type reinterpretation only.
            Ok(args.get(0).cloned().unwrap_or(Value::Atom(Atom::Int(0))))
        }
        "Ptr#" => {
            // Ptr# is identity — inttoptr doesn't change bits.
            Ok(args.get(0).cloned().unwrap_or(Value::Atom(Atom::Int(0))))
        }
        // ── Float math ──────────────────────────────────────────────
        "Sqrt#"  => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.sqrt())) }
        "Sin#"   => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.sin())) }
        "Cos#"   => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.cos())) }
        "Fabs#"  => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.abs())) }
        "Max#"   => { let a = arg_as_f64(args, 0)?; let b = arg_as_f64(args, 1)?; Ok(f64_to_bits(a.max(b))) }
        "Min#"   => { let a = arg_as_f64(args, 0)?; let b = arg_as_f64(args, 1)?; Ok(f64_to_bits(a.min(b))) }
        "Ceil#"  => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.ceil())) }
        "Floor#" => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.floor())) }
        "Exp#"   => { let x = arg_as_f64(args, 0)?; Ok(f64_to_bits(x.exp())) }
        "Pow#"   => { let a = arg_as_f64(args, 0)?; let b = arg_as_f64(args, 1)?; Ok(f64_to_bits(a.powf(b))) }
        // ── Warp shuffle (CPU fallback: identity — single lane) ───
        "ShuffleDown#" | "ShuffleXor#" => {
            args.first().cloned().ok_or_else(|| RuntimeError::HeapError("shuffle needs a value".into()))
        }
        "SubgroupFAdd#" => {
            let x = arg_as_f64(args, 0)?;
            Ok(f64_to_bits(x))
        }
        "SubgroupFMax#" => {
            let x = arg_as_f64(args, 0)?;
            Ok(f64_to_bits(x))
        }
        "SubgroupFMin#" => {
            let x = arg_as_f64(args, 0)?;
            Ok(f64_to_bits(x))
        }
        "Fma#" => {
            let a = arg_as_f64(args, 0)?;
            let b = arg_as_f64(args, 1)?;
            let c = arg_as_f64(args, 2)?;
            Ok(f64_to_bits(a.mul_add(b, c)))
        }
        "SubgroupBallot#" => {
            // CPU fallback: single lane, always satisfied → bitmask 1
            Ok(i64_to_bits(1))
        }
        "SubgroupBroadcast#" => {
            // CPU fallback: identity (single lane, val is from lane 0)
            args.first().cloned().ok_or_else(|| RuntimeError::HeapError("broadcast needs a value".into()))
        }

        // ── Memory (observable) ─────────────────────────────────────
        "Malloc#" => {
            let size = arg_as_i64(args, 0)?;
            let addr = heap.allocate(size as usize);
            Ok(i64_to_bits(addr as i64))
        }
        "Alloc#" => {
            let size = arg_as_i64(args, 0)?;
            let addr = heap.allocate(size as usize);
            Ok(i64_to_bits(addr as i64))
        }
        "Free#" => {
            let ptr = arg_as_i64(args, 0)?;
            heap.free(ptr as u64)
                .map_err(|_| RuntimeError::HeapError("free failed".into()))?;
            Ok(Value::Void)
        }
        // 2026-08-03: host cancellation. In-process there is no host to raise
        // the flag, so CancelRequested# is always false (the backend's
        // __briev_set_cancel is the real path).
        "CancelRequested#" => Ok(Value::Atom(Atom::Bool(false))),
        "ClearCancel#" => Ok(Value::Void),
         "Load#" => {
            let addr = arg_as_i64(args, 0)? as u64;
            let bytes = args.get(1).and_then(|a| if let Value::Atom(Atom::Int(n)) = a { Some(*n as usize) } else { None }).unwrap_or(8);
            let data = heap.read(addr, bytes)
                .ok_or_else(|| RuntimeError::HeapError("Load# read failed".into()))?;
            let mut buf = data.to_vec();
            buf.resize(8, 0);
            let val = i64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
            Ok(i64_to_bits(val))
        }
        "Store#" => {
            let addr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let bytes = args.get(2).and_then(|a| if let Value::Atom(Atom::Int(n)) = a { Some(*n as usize) } else { None }).unwrap_or(8);
            let data = val.to_le_bytes()[..bytes].to_vec();
            heap.write(addr, &data)
                .map_err(|_| RuntimeError::HeapError("Store# write failed".into()))?;
            Ok(Value::Void)
        }
        // 2026-08-27 (Slice C): typed volatile MMIO over the interpreter
        // heap. Width = one word (the interpreter stores words; element-type
        // nuance is a backend concern — parity tests compare word values).
        "VolatileLoad#" => {
            let addr = arg_as_i64(args, 0)? as u64;
            let data = heap.read(addr, 8)
                .ok_or_else(|| RuntimeError::HeapError("VolatileLoad# read failed".into()))?;
            let val = i64::from_le_bytes(data.try_into().unwrap());
            Ok(i64_to_bits(val))
        }
        "VolatileStore#" => {
            let addr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            heap.write(addr, &val.to_le_bytes())
                .map_err(|_| RuntimeError::HeapError("VolatileStore# write failed".into()))?;
            Ok(Value::Atom(Atom::Bool(true)))
        }
        "Copy#" => {
            let dst = arg_as_i64(args, 0)? as u64;
            let src = arg_as_i64(args, 1)? as u64;
            let n = arg_as_i64(args, 2)? as usize;
            let data = heap.read(src, n)
                .ok_or_else(|| RuntimeError::HeapError("Copy# read failed".into()))?;
            let data_vec = data.to_vec();
            heap.write(dst, &data_vec)
                .map_err(|_| RuntimeError::HeapError("Copy# write failed".into()))?;
            Ok(Value::Void)
        }
        "Fill#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let n = arg_as_i64(args, 2)? as usize;
            let data = vec![val as u8; n];
            heap.write(ptr, &data)
                .map_err(|_| RuntimeError::HeapError("Fill# failed".into()))?;
            Ok(Value::Void)
        }

        // ── String / Conversion ─────────────────────────────────────
        "Concat#"   => Err(RuntimeError::UnsupportedIntrinsic("Concat#".to_string())),
        "Length#"   => Err(RuntimeError::UnsupportedIntrinsic("Length#".to_string())),
        "ToInt#"    => { let f = arg_as_f64(args, 0)?; Ok(i64_to_bits(f as i64)) }
        "ToFloat#"  => { let n = arg_as_i64(args, 0)?; Ok(f64_to_bits(n as f64)) }
        "ToString#" => {
            let s = if let Ok(n) = arg_as_i64(args, 0) {
                n.to_string()
            } else if let Ok(f) = arg_as_f64(args, 0) {
                f.to_string()
            } else {
                arg_as_string(args, 0)?
            };
            let bytes = s.into_bytes();
            let len = bytes.len() as i64;
            let mut header = len.to_le_bytes().to_vec();
            header.extend_from_slice(&bytes);
            let addr = heap.allocate(header.len());
            if heap.write(addr as u64, &header).is_ok() {
                Ok(i64_to_bits(addr as i64))
            } else {
                Ok(i64_to_bits(0))
            }
        },

        // ── Collection ──────────────────────────────────────────────
        "Get#"    => Err(RuntimeError::UnsupportedIntrinsic("Get#".to_string())),
        "Insert#" => Err(RuntimeError::UnsupportedIntrinsic("Insert#".to_string())),

        // ── GPU ─────────────────────────────────────────────────────
        "GetGlobalId#"   => Err(RuntimeError::UnsupportedIntrinsic("GetGlobalId#".to_string())),
        "GetGlobalSize#" => Err(RuntimeError::UnsupportedIntrinsic("GetGlobalSize#".to_string())),
        "GetLocalId#"    => Err(RuntimeError::UnsupportedIntrinsic("GetLocalId#".to_string())),
        "AddressOf#" => {
            let id = arg_as_string(args, 0)?;
            let addr = resolve_address_for_interp(&id);
            Ok(i64_to_bits(addr as i64))
        }

        // ── POSIX sysconf (observable) ────────────────────────────────
        "SysConf#" => {
            if args.is_empty() { return Ok(i64_to_bits(0)); }
            let name: i64 = match &args[0] {
                Value::Atom(Atom::Int(n)) => *n,
                Value::Bits(b) => {
                    let mut arr = [0u8; 8];
                    let copy_len = b.len().min(8);
                    arr[..copy_len].copy_from_slice(&b[..copy_len]);
                    i64::from_le_bytes(arr)
                }
                other => {
                    let s = format!("{:?}", other);
                    match s.as_str() {
                        "PageSize" => 30,
                        "CpuCount" => 83,
                        "HostNameMax" => 180,
                        "OpenMax" => 4,
                        _ => {
                            eprintln!("SysConf#: unknown abstract name '{}', using 0", s);
                            0
                        }
                    }
                }
            };
            let result = unsafe { libc::sysconf(name as i32) };
            Ok(i64_to_bits(result as i64))
        }

        // ── OS Syscall (observable) ───────────────────────────────────
        // 2026-07-15: SysCall# executes a raw OS syscall via libc::syscall().
        // In check mode, returns 0 for successful execution or -1 for error.
        // The first arg is the syscall number (Int) or PascalCase abstract op
        // name (resolved to a number here).
        "SysCall#" => {
            if args.is_empty() { return Ok(zero_bits(0)); }
            let mut sysno: i64 = 0;
            match &args[0] {
                Value::Atom(Atom::Int(n)) => { sysno = *n; }
                Value::Bits(b) => {
                    // Convert Vec<u8> to i64 (little-endian)
                    let mut arr = [0u8; 8];
                    let copy_len = b.len().min(8);
                    arr[..copy_len].copy_from_slice(&b[..copy_len]);
                    sysno = i64::from_le_bytes(arr);
                }
                other => {
                    let name = format!("{:?}", other);
                    sysno = resolve_syscall_number_interp(&name).unwrap_or(0);
                }
            }
            // Extract up to 6 Int arguments, default 0
            let arg_val = |i: usize| -> i64 {
                args.get(i).map(|v| match v {
                    Value::Atom(Atom::Int(n)) => *n,
                    Value::Bits(b) => {
                        let mut arr = [0u8; 8];
                        let copy_len = b.len().min(8);
                        arr[..copy_len].copy_from_slice(&b[..copy_len]);
                        i64::from_le_bytes(arr)
                    }
                    _ => 0
                }).unwrap_or(0)
            };
            let a1 = arg_val(1); let a2 = arg_val(2); let a3 = arg_val(3);
            let a4 = arg_val(4); let a5 = arg_val(5); let a6 = arg_val(6);
            let result = unsafe { libc::syscall(sysno, a1, a2, a3, a4, a5, a6) };
            Ok(i64_to_bits(result as i64))
        }

        // ── Atomic operations (non-atomic in check mode) ──────────────
        // 2026-07-15: Single-threaded check mode — just do regular heap
        // load/store. Full LLVM atomicrmw is emitted at runtime.
        "AtomicLoad#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let data = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            Ok(i64_to_bits(data))
        }
        "AtomicStore#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let bytes = val.to_le_bytes();
            heap.write(ptr, &bytes).ok();
            Ok(Value::Void)
        }
        "AtomicCas#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let expected = arg_as_i64(args, 1)?;
            let desired = arg_as_i64(args, 2)?;
            let current = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            if current == expected {
                heap.write(ptr, &desired.to_le_bytes()).ok();
            }
            Ok(i64_to_bits(current))
        }
        "AtomicXchg#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            heap.write(ptr, &val.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        "AtomicAdd#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            let new = old.wrapping_add(val);
            heap.write(ptr, &new.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): RMW family
        // completion + width-parameterized access. Single-threaded check
        // mode: plain heap ops; ordering args positionally ignored.
        "AtomicSub#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            let new = old.wrapping_sub(val);
            heap.write(ptr, &new.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        "AtomicOr#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            let new = old | val;
            heap.write(ptr, &new.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        "AtomicAnd#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            let new = old & val;
            heap.write(ptr, &new.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        "AtomicXor#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let old = heap.read(ptr, 8).map(|b| i64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]])).unwrap_or(0);
            let new = old ^ val;
            heap.write(ptr, &new.to_le_bytes()).ok();
            Ok(i64_to_bits(old))
        }
        "AtomicLoadN#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let n = arg_as_i64(args, 1)? as usize;
            let data = heap.read(ptr, n).map(|b| {
                let mut arr = [0u8; 8];
                arr[..n.min(8)].copy_from_slice(&b[..n.min(8)]);
                i64::from_le_bytes(arr)
            }).unwrap_or(0);
            Ok(i64_to_bits(data))
        }
        "AtomicStoreN#" => {
            let ptr = arg_as_i64(args, 0)? as u64;
            let val = arg_as_i64(args, 1)?;
            let n = arg_as_i64(args, 2)? as usize;
            let bytes = val.to_le_bytes();
            heap.write(ptr, &bytes[..n.min(8)]).ok();
            Ok(Value::Void)
        }
        "Fence#" => {
            Ok(Value::Void)
        }
        // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): portable SIMD
        // family — check-mode semantics are word-wise (i64) element loops
        // over the heap, the same single-threaded convention as the other
        // memory intrinsics. Element-type nuance (f32 chunks, FMA) is a
        // backend concern; parity tests compare word values.
        "SimdAdd#" | "SimdSub#" | "SimdMul#" | "SimdFma#" => {
            let is_fma = name == "SimdFma#";
            let dst = arg_as_i64(args, 0)? as u64;
            let a = arg_as_i64(args, 1)? as u64;
            let b = arg_as_i64(args, 2)? as u64;
            let c = if is_fma { arg_as_i64(args, 3)? as u64 } else { 0 };
            let count_idx = if is_fma { 4 } else { 3 };
            let count = arg_as_i64(args, count_idx)? as usize;
            let word = 8usize;
            for k in 0..count {
                let off = k * word;
                let av = heap.read(a + off as u64, 8).map(|b| i64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
                let bv = heap.read(b + off as u64, 8).map(|b| i64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
                let cv = if is_fma {
                    heap.read(c + off as u64, 8).map(|b| i64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
                } else { 0 };
                let r = match name {
                    "SimdAdd#" => av.wrapping_add(bv),
                    "SimdSub#" => av.wrapping_sub(bv),
                    "SimdFma#" => av.wrapping_mul(bv).wrapping_add(cv),
                    _ => av.wrapping_mul(bv),
                };
                heap.write(dst + off as u64, &r.to_le_bytes()).ok();
            }
            Ok(Value::Void)
        }

        // ── Dynamic linker intrinsics (observable) ────────────────────
        // 2026-07-15: In check mode, call host libc functions.
        "DlOpen#" => {
            let path_ptr = arg_as_i64(args, 0)? as *const libc::c_char;
            let flags = arg_as_i64(args, 1)? as libc::c_int;
            let result = unsafe { libc::dlopen(path_ptr, flags) };
            Ok(i64_to_bits(result as i64))
        }
        "DlSym#" => {
            let handle = arg_as_i64(args, 0)? as *mut libc::c_void;
            let symbol_ptr = arg_as_i64(args, 1)? as *const libc::c_char;
            let result = unsafe { libc::dlsym(handle, symbol_ptr) };
            Ok(i64_to_bits(result as i64))
        }
        "DlClose#" => {
            let handle = arg_as_i64(args, 0)? as *mut libc::c_void;
            let result = unsafe { libc::dlclose(handle) };
            Ok(i64_to_bits(result as i64))
        }

        // ── Backtrace intrinsic (observable) ──────────────────────────
        // 2026-07-15: In check mode, stub returning 0.
        "Backtrace#" => {
            Ok(i64_to_bits(0))
        }

        // ── Runtime string operations (2026-07-25) ─────────────────────
        // In check mode these execute host calls directly.
        "StrSplit#" => {
            // Runtime: return first segment as string (list representation TBD).
            let s = arg_as_string(args, 0)?;
            let pat = arg_as_string(args, 1)?;
            let first = s.split(&pat).next().unwrap_or("").to_string();
            let bytes = first.into_bytes();
            let len = bytes.len() as i64;
            let mut header = len.to_le_bytes().to_vec();
            header.extend_from_slice(&bytes);
            let addr = heap.allocate(header.len());
            if heap.write(addr as u64, &header).is_ok() {
                Ok(i64_to_bits(addr as i64))
            } else {
                Ok(i64_to_bits(0))
            }
        }
        "EnvGet#" => {
            let name = arg_as_string(args, 0)?;
            let val = std::env::var(&name).unwrap_or_default();
            let bytes = val.into_bytes();
            let len = bytes.len() as i64;
            let mut header = len.to_le_bytes().to_vec();
            header.extend_from_slice(&bytes);
            let addr = heap.allocate(header.len());
            if heap.write(addr as u64, &header).is_ok() {
                Ok(i64_to_bits(addr as i64))
            } else {
                Ok(i64_to_bits(0))
            }
        }

        // ── System query intrinsics (2026-07-25) ────────────────────────
        "SysQuery#" => {
            let query = arg_as_string(args, 0)?;
            // Return the same format as eval_nav_call SysQuery$ does,
            // but via the Value interface.
            match query.as_str() {
                "cpu.cores" => {
                    let cores = std::thread::available_parallelism()
                        .map(|n| n.get() as i64).unwrap_or(1);
                    Ok(i64_to_bits(cores))
                }
                "cpu.arch" => {
                    let s = std::env::consts::ARCH.to_string();
                    let bytes = s.into_bytes();
                    let len = bytes.len() as i64;
                    let mut header = len.to_le_bytes().to_vec();
                    header.extend_from_slice(&bytes);
                    let addr = heap.allocate(header.len());
                    heap.write(addr as u64, &header).ok();
                    Ok(i64_to_bits(addr as i64))
                }
                "os" => {
                    let s = std::env::consts::OS.to_string();
                    let bytes = s.into_bytes();
                    let len = bytes.len() as i64;
                    let mut header = len.to_le_bytes().to_vec();
                    header.extend_from_slice(&bytes);
                    let addr = heap.allocate(header.len());
                    heap.write(addr as u64, &header).ok();
                    Ok(i64_to_bits(addr as i64))
                }
                _ => Ok(i64_to_bits(0))
            }
        }
        "TimeNow#" => {
            let ts = std::process::Command::new("git")
                .args(["log", "-1", "--format=%ct"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok()
                    } else { None }
                })
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                });
            Ok(i64_to_bits(ts))
        }

        // ── External I/O intrinsics (observable, 2026-07-25) ────────────
        "HttpFetch#" => {
            let url = arg_as_string(args, 0)?;
            let resp = ureq::get(&url).call()
                .map_err(|e| RuntimeError::HeapError(format!("HttpFetch#: {}", e)))?;
            let body = resp.into_string()
                .map_err(|e| RuntimeError::HeapError(format!("HttpFetch#: {}", e)))?;
            let bytes = body.into_bytes();
            let len = bytes.len() as i64;
            let mut header = len.to_le_bytes().to_vec();
            header.extend_from_slice(&bytes);
            let addr = heap.allocate(header.len());
            if heap.write(addr as u64, &header).is_ok() {
                Ok(i64_to_bits(addr as i64))
            } else {
                Ok(i64_to_bits(0))
            }
        }
        "ShellCmd#" => {
            let cmd = arg_as_string(args, 0)?;
            let cmd_args: Vec<String> = args[1..].iter()
                .filter_map(|a| a.as_i64())
                .map(|n| n.to_string())
                .collect();
            let output = std::process::Command::new(&cmd)
                .args(&cmd_args)
                .output()
                .map_err(|e| RuntimeError::HeapError(format!("ShellCmd#: {}", e)))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RuntimeError::HeapError(format!("ShellCmd#: '{}' failed: {}", cmd, stderr)));
            }
            let body = String::from_utf8_lossy(&output.stdout).to_string();
            let bytes = body.into_bytes();
            let len = bytes.len() as i64;
            let mut header = len.to_le_bytes().to_vec();
            header.extend_from_slice(&bytes);
            let addr = heap.allocate(header.len());
            if heap.write(addr as u64, &header).is_ok() {
                Ok(i64_to_bits(addr as i64))
            } else {
                Ok(i64_to_bits(0))
            }
        }

        // ── Print intrinsics (observable) ──────────────────────────────
        // 2026-08-01 (audit): one generic `Print#` — dispatch by the RUNTIME
        // value's representation (Float / integer / string bits), mirroring
        // the codegen's universe-category dispatch. PrintChar# remains the
        // internal newline/char primitive.
        "Print#" => {
            // 2026-08-01 (audit): the convenience intrinsic — dispatch by the
            // VALUE's category, mirroring the codegen's protocol-category
            // dispatch. Bool prints true/false (natural); Char prints the
            // character; an explicit cast to Int is what yields 1/0.
            match args.get(0).cloned().unwrap_or(Value::Void) {
                Value::Atom(Atom::Float(f)) => {
                    print!("{}", f);
                }
                Value::Bits(bytes) => {
                    print!("{}", String::from_utf8_lossy(&bytes));
                }
                Value::Atom(Atom::Int(n)) => {
                    print!("{}", n);
                }
                Value::Atom(Atom::Bool(b)) => {
                    print!("{}", if b { "true" } else { "false" });
                }
                Value::Atom(Atom::Char(c)) => {
                    print!("{}", c);
                }
                Value::Void => {}
                _ => {}
            }
            Ok(i64_to_bits(0))
        }
        _ => Err(RuntimeError::UnsupportedIntrinsic(name.to_string())),
    }
}

// ── Polymorphic operation dispatchers ──────────────────────────────────

/// Execute a binary arithmetic operation. Tries float first, then int.
fn exec_binop(
    args: &[Value],
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
) -> Result<Value, RuntimeError> {
    if let (Ok(a), Ok(b)) = (arg_as_f64(args, 0), arg_as_f64(args, 1)) {
        return Ok(f64_to_bits(float_op(a, b)));
    }
    let a = arg_as_i64(args, 0)?;
    let b = arg_as_i64(args, 1)?;
    Ok(i64_to_bits(int_op(a, b)))
}

/// Execute a comparison. Tries float first, then int.
fn exec_cmp(
    args: &[Value],
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
) -> Result<Value, RuntimeError> {
    if let (Ok(a), Ok(b)) = (arg_as_f64(args, 0), arg_as_f64(args, 1)) {
        return Ok(bool_to_bits(float_cmp(a, b)));
    }
    let a = arg_as_i64(args, 0)?;
    let b = arg_as_i64(args, 1)?;
    Ok(bool_to_bits(int_cmp(a, b)))
}

/// Division with zero check. Tries float first, then int.
fn exec_div(args: &[Value]) -> Result<Value, RuntimeError> {
    if let (Ok(a), Ok(b)) = (arg_as_f64(args, 0), arg_as_f64(args, 1)) {
        if b == 0.0 { return Err(RuntimeError::DivisionByZero); }
        return Ok(f64_to_bits(a / b));
    }
    let a = arg_as_i64(args, 0)?;
    let b = arg_as_i64(args, 1)?;
    if b == 0 { return Err(RuntimeError::DivisionByZero); }
    Ok(i64_to_bits(a.wrapping_div(b)))
}

/// Remainder with zero check.
fn exec_rem(args: &[Value]) -> Result<Value, RuntimeError> {
    let a = arg_as_i64(args, 0)?;
    let b = arg_as_i64(args, 1)?;
    if b == 0 { return Err(RuntimeError::DivisionByZero); }
    Ok(i64_to_bits(a.wrapping_rem(b)))
}

// ── Argument extraction helpers ────────────────────────────────────────

fn arg_as_i64(args: &[Value], index: usize) -> Result<i64, RuntimeError> {
    args.get(index)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| RuntimeError::TypeError {
            expected: "Int".into(),
            found: format!("{:?}", args.get(index)),
        })
}

fn arg_as_f64(args: &[Value], index: usize) -> Result<f64, RuntimeError> {
    args.get(index)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| RuntimeError::TypeError {
            expected: "Float".into(),
            found: format!("{:?}", args.get(index)),
        })
}

fn arg_as_string(args: &[Value], index: usize) -> Result<String, RuntimeError> {
    match args.get(index) {
        Some(Value::Bits(bytes)) => Ok(String::from_utf8_lossy(bytes).to_string()),
        _ => Err(RuntimeError::TypeError {
            expected: "String".into(),
            found: format!("{:?}", args.get(index)),
        }),
    }
}

/// 2026-07-15: Resolve a named address to its numeric value.
/// Delegates to the shared address_resolver module used by both
/// the interpreter and LLVM backend.
fn resolve_address_for_interp(id: &str) -> u64 {
    crate::address_resolver::resolve_address(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_add_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Add#", &[i64_to_bits(2), i64_to_bits(3)], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(5));
    }

    #[test]
    fn test_sub_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Sub#", &[i64_to_bits(10), i64_to_bits(3)], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(7));
    }

    #[test]
    fn test_mul_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Mul#", &[i64_to_bits(4), i64_to_bits(5)], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(20));
    }

    #[test]
    fn test_div_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Div#", &[i64_to_bits(10), i64_to_bits(3)], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(3));
    }

    #[test]
    fn test_div_by_zero() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Div#", &[i64_to_bits(1), i64_to_bits(0)], &mut heap);
        assert!(r.is_err());
    }

    #[test]
    fn test_eq_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Eq#", &[i64_to_bits(42), i64_to_bits(42)], &mut heap).unwrap();
        assert!(r.is_true());
        let r = execute_intrinsic("Eq#", &[i64_to_bits(42), i64_to_bits(43)], &mut heap).unwrap();
        assert!(!r.is_true());
    }

    /// 2026-08-27 (Slice C): VolatileStore#/VolatileLoad# round-trip through
    /// the virtual heap — the interpreter reference for MMIO semantics.
    #[test]
    fn test_volatile_roundtrip() {
        let mut heap = VirtualHeap::new();
        let addr = heap.allocate(8);
        let store = execute_intrinsic("VolatileStore#", &[i64_to_bits(addr as i64), i64_to_bits(0x41)], &mut heap).unwrap();
        assert!(store.is_true(), "store yields Bool true");
        let load = execute_intrinsic("VolatileLoad#", &[i64_to_bits(addr as i64)], &mut heap).unwrap();
        assert_eq!(load.as_i64(), Some(0x41));
    }

    #[test]
    fn test_volatile_load_unmapped_errors() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("VolatileLoad#", &[i64_to_bits(0x40010000)], &mut heap);
        assert!(r.is_err(), "unmapped hardware-style address errors in interp");
    }

    #[test]
    fn test_string_content_eq_literals() {
        // 2026-08-01 (B1): content equality on String operands. Two distinct
        // literal expressions with the same payload must compare equal; a
        // differing payload (or length) must compare unequal. This pins the
        // interpreter-first half of B1 (rule #4).
        use crate::ast::{BinaryOpKind, Expr};
        use crate::interpreter::eval_expr;
        let eq = |a: &str, b: &str| -> bool {
            let mut heap = VirtualHeap::new();
            let mut bindings = std::collections::HashMap::new();
            let expr = Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Quoted(a.as_bytes().to_vec())),
                Box::new(Expr::Quoted(b.as_bytes().to_vec())),
            );
            eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true()
        };
        assert!(eq("hello", "hello"));
        assert!(!eq("hello", "world"));
        // Differing lengths are unequal even when one is a prefix of the other.
        assert!(!eq("ab", "abc"));
        assert!(!eq("abc", "ab"));
        // Empty strings are equal to each other.
        assert!(eq("", ""));
    }

    #[test]
    fn test_string_content_eq_heap_handles() {
        // 2026-08-01 (B1): two equal-content strings at DIFFERENT heap
        // addresses must compare equal (the acceptance case for B1 — address
        // equality would be false here). Handles are [len: i64][payload].
        use crate::ast::{BinaryOpKind, Expr};
        use crate::interpreter::eval_expr;
        let alloc_str = |heap: &mut VirtualHeap, s: &str| -> i64 {
            let bytes = s.as_bytes();
            let mut header = (bytes.len() as i64).to_le_bytes().to_vec();
            header.extend_from_slice(bytes);
            let addr = heap.allocate(header.len());
            heap.write(addr, &header).unwrap();
            addr as i64
        };
        let mut heap = VirtualHeap::new();
        let a1 = alloc_str(&mut heap, "same content");
        let a2 = alloc_str(&mut heap, "same content");
        assert_ne!(a1, a2, "test must use two distinct addresses");
        let mut bindings = std::collections::HashMap::new();
        let expr = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Identifier("b".into())),
        );
        bindings.insert("a".into(), Value::Atom(Atom::Int(a1)));
        bindings.insert("b".into(), Value::Atom(Atom::Int(a2)));
        assert!(eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true());

        // Ne on the same two handles is false.
        let expr_ne = Expr::BinaryOp(
            BinaryOpKind::Neq,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Identifier("b".into())),
        );
        assert!(!eval_expr(&expr_ne, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true());

        // Differing content at distinct addresses is unequal.
        let c = alloc_str(&mut heap, "different content");
        bindings.insert("c".into(), Value::Atom(Atom::Int(c)));
        let expr2 = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Identifier("c".into())),
        );
        assert!(!eval_expr(&expr2, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true());
    }

    #[test]
    fn test_string_content_eq_numeric_fallthrough() {
        // 2026-08-01 (B1): a String compared against a non-String must NOT
        // hit the string path (string_bytes returns None for it) — the
        // numeric fallback decides. This guards against the deref helper
        // swallowing int comparisons.
        use crate::ast::{BinaryOpKind, Expr};
        use crate::interpreter::eval_expr;
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        let expr = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Decimal(5)),
            Box::new(Expr::Decimal(5)),
        );
        assert!(eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true());
        let expr = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Decimal(5)),
            Box::new(Expr::Decimal(6)),
        );
        assert!(!eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap().is_true());
    }

    #[test]
    fn test_string_bitwise_ops() {
        // 2026-08-01 (B1): & | ^ ~ on String operands operate on content bytes
        // and produce a same-length result (interpreter parity with the
        // backend's briev_str_band/bor/bxor/bnot calls).
        use crate::ast::{BinaryOpKind, Expr, UnaryOpKind};
        use crate::interpreter::eval_expr;
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        let mut eval = |kind, a: &str, b: &str| -> Vec<u8> {
            let expr = Expr::BinaryOp(
                kind,
                Box::new(Expr::Quoted(a.as_bytes().to_vec())),
                Box::new(Expr::Quoted(b.as_bytes().to_vec())),
            );
            match eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap() {
                Value::Bits(bytes) => bytes,
                other => panic!("expected Bits, got {other:?}"),
            }
        };
        // AND: 'a'(0x61) & 'd'(0x64) = 0x60, 'b'(0x62) & 'e'(0x65) = 0x60,
        // 'c'(0x63) & 'f'(0x66) = 0x62
        assert_eq!(eval(BinaryOpKind::BitAnd, "abc", "def"), vec![0x60, 0x60, 0x62]);
        // OR: 'a' | 'd' = 0x65
        assert_eq!(eval(BinaryOpKind::BitOr, "a", "d"), vec![0x65]);
        // XOR: 'a' ^ 'a' = 0
        assert_eq!(eval(BinaryOpKind::BitXor, "a", "a"), vec![0]);
        // ~ on content bytes (unary)
        let un = Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(Expr::Quoted(b"a".to_vec())));
        match eval_expr(&un, &mut heap, &mut bindings, &HashMap::new()).unwrap() {
            Value::Bits(bytes) => assert_eq!(bytes, vec![0x9E]), // ~0x61 = 0x9E
            other => panic!("expected Bits, got {other:?}"),
        }
    }

    #[test]
    fn test_lt_i64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Lt#", &[i64_to_bits(1), i64_to_bits(2)], &mut heap).unwrap();
        assert!(r.is_true());
        let r = execute_intrinsic("Lt#", &[i64_to_bits(2), i64_to_bits(1)], &mut heap).unwrap();
        assert!(!r.is_true());
    }

    #[test]
    fn test_fadd_f64() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Add#", &[f64_to_bits(1.5), f64_to_bits(2.5)], &mut heap).unwrap();
        assert!((r.as_f64().unwrap() - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_sqrt() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Sqrt#", &[f64_to_bits(16.0)], &mut heap).unwrap();
        assert!((r.as_f64().unwrap() - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_malloc_free() {
        let mut heap = VirtualHeap::new();
        let addr = execute_intrinsic("Malloc#", &[i64_to_bits(16)], &mut heap).unwrap();
        let ptr = addr.as_i64().unwrap() as u64;
        assert!(heap.contains(ptr));
        execute_intrinsic("Free#", &[addr], &mut heap).unwrap();
        assert!(!heap.contains(ptr));
    }

    #[test]
    fn test_alloc_interpreter() {
        let mut heap = VirtualHeap::new();
        let addr = execute_intrinsic("Alloc#", &[i64_to_bits(32)], &mut heap).unwrap();
        let ptr = addr.as_i64().unwrap() as u64;
        assert!(heap.contains(ptr));
    }

    #[test]
    fn test_unknown_intrinsic() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("NonExistent#", &[], &mut heap);
        assert!(r.is_err());
    }

    #[test]
    fn test_collection_op_intrinsics_parity() {
        // 2026-08-14 (UOL §6b): `Count#`/`At#`/`Slice#` value parity with the
        // codegen's op-member dispatch. A collection is a Product of elements.
        let mut heap = VirtualHeap::new();
        let coll = Value::Product {
            fields: vec![Value::int(10), Value::int(20), Value::int(30)],
            names: None,
        };
        assert_eq!(
            execute_intrinsic("Count#", &[coll.clone()], &mut heap).unwrap().as_i64(),
            Some(3)
        );
        assert_eq!(
            execute_intrinsic("At#", &[coll.clone(), Value::int(1)], &mut heap).unwrap().as_i64(),
            Some(20)
        );
        let sliced = execute_intrinsic("Slice#", &[coll.clone(), Value::int(0), Value::int(2)], &mut heap).unwrap();
        assert_eq!(execute_intrinsic("Count#", &[sliced], &mut heap).unwrap().as_i64(), Some(2));
    }

    #[test]
    fn test_bit_intrinsics_parity() {
        // 2026-08-14 (boundary plan, SPEC §17.3): interpreter parity with the
        // LLVM llvm.ctpop/ctlz/cttz/bitreverse lanes (rule #4). Value 0b0110
        // (= 6): 2 set bits, 61 leading zeros, 1 trailing zero; bit-reversed
        // = 0x6000000000000000.
        let mut heap = VirtualHeap::new();
        assert_eq!(
            execute_intrinsic("Popcount#", &[i64_to_bits(6)], &mut heap).unwrap().as_i64(),
            Some(2)
        );
        assert_eq!(
            execute_intrinsic("LeadingZeros#", &[i64_to_bits(6)], &mut heap).unwrap().as_i64(),
            Some(61)
        );
        assert_eq!(
            execute_intrinsic("TrailingZeros#", &[i64_to_bits(6)], &mut heap).unwrap().as_i64(),
            Some(1)
        );
        assert_eq!(
            execute_intrinsic("BitReverse#", &[i64_to_bits(6)], &mut heap).unwrap().as_i64(),
            Some(0x6000000000000000i64)
        );
    }

    #[test]
    fn test_float_to_int() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("ToInt#", &[f64_to_bits(3.7)], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(3));
    }

    #[test]
    fn test_int_to_float() {
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("ToFloat#", &[i64_to_bits(42)], &mut heap).unwrap();
        assert!((r.as_f64().unwrap() - 42.0).abs() < 1e-10);
    }

    #[test]
    fn test_address_of_known() {
        let mut heap = VirtualHeap::new();
        let uart_str = Value::bits("uart".as_bytes().to_vec());
        let r = execute_intrinsic("AddressOf#", &[uart_str], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(0xFFE01000i64));
    }

    #[test]
    fn test_address_of_unknown_defaults() {
        let mut heap = VirtualHeap::new();
        let dev_str = Value::bits("unknown_device".as_bytes().to_vec());
        let r = execute_intrinsic("AddressOf#", &[dev_str], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(0xFE000000i64));
    }

    #[test]
    fn test_reflect_len_and_bytes() {
        // 2026-08-12 (Iterable protocol): `x.^Length` = the STORED byte count
        // (the [len] header), `x.^^Bytes` = byte length, `CharCount#` = the
        // UTF8 char count. 'héllo' is 5 chars / 6 bytes.
        use crate::ast::Expr;
        use crate::interpreter::eval_expr;
        use crate::interpreter::ReflectKind;
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("s".into(), Value::bits("héllo".as_bytes().to_vec()));
        let len_expr = Expr::Reflect(
            Box::new(Expr::Identifier("s".into())),
            "Length".into(),
            ReflectKind::Runtime,
        );
        let len = eval_expr(&len_expr, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(len.as_i64(), Some(6), "héllo is 6 bytes (stored header)");
        let chars = eval_expr(
            &Expr::Call("CharCount#".into(), vec![Expr::Identifier("s".into())], None),
            &mut heap,
            &mut bindings,
            &HashMap::new(),
        ).unwrap();
        assert_eq!(chars.as_i64(), Some(5), "héllo has 5 UTF8 chars (CharCount#)");
        let bytes_expr = Expr::Reflect(
            Box::new(Expr::Identifier("s".into())),
            "Bytes".into(),
            ReflectKind::CompileTime,
        );
        let bytes = eval_expr(&bytes_expr, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bytes.as_i64(), Some(6), "héllo is 6 bytes");
    }

    /// 2026-08-15 (coll plan §3.6, ambiguity #6): the capacity intrinsics on
    /// a Product value — `Capacity#` = field count (a Vec is exact-fit); the
    /// write forms are no-ops (capacity not observable).
    #[test]
    fn capacity_intrinsics_on_product() {
        let mut heap = VirtualHeap::new();
        let coll = Value::Product {
            fields: vec![Value::int(10), Value::int(20), Value::int(30)],
            names: None,
        };
        let cap = execute_intrinsic("Capacity#", &[coll.clone()], &mut heap).unwrap();
        assert_eq!(cap.as_i64(), Some(3), "Capacity# on a 3-field product = 3");
        let resized = execute_intrinsic(
            "Resize#",
            &[coll.clone(), Value::int(64)],
            &mut heap,
        ).unwrap();
        assert!(matches!(resized, Value::Void), "Resize# is a no-op on a product");
        let trimmed = execute_intrinsic("TrimCap#", &[coll], &mut heap).unwrap();
        assert!(matches!(trimmed, Value::Void), "TrimCap# is a no-op on a product");
    }
}
