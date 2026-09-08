// ── Statement Codegen ──────────────────────────────────────────────────
// 2026-07-12: Phase 4 — Emit LLVM IR for all Statement variants.
//
// 2026-07-04: The 15-field %State chunk cap (now config/ir-lowering.toml
// `max_fields_per_alloca`) ensures LLVM's SROA can decompose %State chunks
// into scalars for alias analysis and vectorization.

use crate::ast::{Expr, Statement, Type};
use crate::backend::llvm::{emit_expr::member_briev_name, LlvmBackend, TypedRegister};
use std::fmt::Write;

/// Emit LLVM IR for a statement. Returns the last expression's register.
/// 2026-07-31 (A5): emit a sequence of statements (an obj member body).
pub fn emit_statement_sequence(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmts: &[Statement],
    indent: &str,
) {
    for stmt in stmts {
        emit_statement(backend, out, stmt, indent);
        // 2026-08-01 (Phase 3): a `~op` consumed its operand during this
        // statement — destroy the backing at the statement boundary (after the
        // consuming op has used the value).
        drain_pending_consumes(backend, out, indent);
    }
}

/// 2026-08-01 (Phase 3): emit strategy-aware frees for every register recorded
/// by an `Expr::Consume` since the last boundary. Inline/arena/alloca/ring-buffer
/// backings need no free; heap-backed values are @free'd.
fn drain_pending_consumes(backend: &mut LlvmBackend, out: &mut String, indent: &str) {
    let pending = std::mem::take(&mut backend.fun.pending_consumes);
    for reg in pending {
        emit_destroy_register(backend, out, indent, &reg);
    }
}

/// 2026-08-10: Emit the webstack flush batch at a transaction commit point
/// (`term`/`endprogram`). Replaces the historical `(i32 0, i32 0)` stub with a
/// real update batch: for each field the current transaction writes (its
/// transition-graph write_set, frontend-provided), store a 12-byte record
/// { field_handle, value_ptr, value_len } into @__web_flush_buf, then call
/// __web_flush_state(buf, count). The JS shim's _applyFlush reads exactly this
/// record format (web_generator.rs _applyFlush). value_ptr points at the field
/// slot inside %State — every emission shape commits writes to %State before
/// term, so the slot holds the post-transaction value.
///
/// A transaction whose write_set is empty flushes `(0, 0)` — the JS no-op path
/// stays valid (no DOM mutations to apply).
pub(super) fn emit_web_flush_batch(backend: &mut LlvmBackend, out: &mut String, indent: &str) {
    use std::fmt::Write;
    if !backend.ctx.webstack_enabled {
        return;
    }
    let txn_name = backend.fun.txn_name.clone();
    // 2026-08-10: the write_set is frontend analysis — sorted by field index
    // for deterministic IR (AGENTS.md HashMap iteration rule).
    let mut written: Vec<usize> = backend.ctx.transition_graph.as_ref()
        .and_then(|g| g.nodes.iter().find(|n| n.name == txn_name))
        .map(|n| {
            n.write_set.iter()
                .filter_map(|name| backend.ctx.field_index_map.get(name).copied())
                .collect()
        })
        .unwrap_or_default();
    written.sort_unstable();
    if written.is_empty() {
        writeln!(out, "{}call void @__web_flush_state(i32 0, i32 0)", indent).ok();
        return;
    }
    for (i, idx) in written.iter().enumerate() {
        let field_llvm = backend.ctx.field_types.get(*idx).cloned().unwrap_or_else(|| "i64".to_string());
        let size = super::web_llvm_byte_size(&field_llvm);
        // Skip rows that don't resolve to a word width (matches state_layout).
        if size == 0 {
            continue;
        }
        let gep = backend.emit_state_gep(out, indent, "wf", "%state", *idx);
        // @__web_flush_buf is `[N x { i32, i32, i32 }]` — GEP to record i, field 0
        // for the handle store; fields 1 (value_ptr) and 2 (value_len) GEP below.
        let buf_gep = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr inbounds [{} x {{ i32, i32, i32 }}], ptr @__web_flush_buf, i32 0, i32 {}, i32 0",
            indent, buf_gep, backend.ctx.web_max_entries, i).ok();
        // field_handle
        writeln!(out, "{}store i32 {}, ptr {}", indent, *idx as u32, buf_gep).ok();
        // value_ptr — ptrtoint of the %State slot (resolves at link time in wasm32)
        let val_ptr = backend.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to i32", indent, val_ptr, gep).ok();
        let val_ptr_gep = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr inbounds [{} x {{ i32, i32, i32 }}], ptr @__web_flush_buf, i32 0, i32 {}, i32 1",
            indent, val_ptr_gep, backend.ctx.web_max_entries, i).ok();
        writeln!(out, "{}store i32 {}, ptr {}", indent, val_ptr, val_ptr_gep).ok();
        // value_len
        let len_gep = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr inbounds [{} x {{ i32, i32, i32 }}], ptr @__web_flush_buf, i32 0, i32 {}, i32 2",
            indent, len_gep, backend.ctx.web_max_entries, i).ok();
        writeln!(out, "{}store i32 {}, ptr {}", indent, size as u32, len_gep).ok();
    }
    let count = written.iter().filter(|idx| {
        let field_llvm = backend.ctx.field_types.get(**idx).cloned().unwrap_or_else(|| "i64".to_string());
        super::web_llvm_byte_size(&field_llvm) > 0
    }).count();
    writeln!(out, "{}call void @__web_flush_state(i32 ptrtoint (ptr @__web_flush_buf to i32), i32 {})", indent, count).ok();
    // 2026-08-10: bump the generation counter so the JS `generation` getter
    // observes this commit (HMR/SSR contract in rendered-briev-wasm.md).
    let g = backend.fun.gen_reg();
    let g2 = backend.fun.gen_reg();
    writeln!(out, "{}{} = load i32, ptr @__web_generation", indent, g).ok();
    writeln!(out, "{}{} = add i32 {}, 1", indent, g2, g).ok();
    writeln!(out, "{}store i32 {}, ptr @__web_generation", indent, g2).ok();
}

/// Destroy a consumed register's backing storage — an allocation-strategy-aware
/// free (mirrors the Free# intrinsic). The register is a handle (stored as an
/// i64), widened via inttoptr for the @free call.
pub(super) fn emit_destroy_register(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    reg: &str,
) {
    match backend.fun.alloc_strategies.get(reg) {
        Some(crate::backend::llvm::AllocStrategy::Arena)
        | Some(crate::backend::llvm::AllocStrategy::Alloca)
        | Some(crate::backend::llvm::AllocStrategy::Inline)
        | Some(crate::backend::llvm::AllocStrategy::RingBuffer) => {}
        Some(crate::backend::llvm::AllocStrategy::Malloc)
        | Some(crate::backend::llvm::AllocStrategy::Custom(_)) => {
            // Heap-backed — free the backing pointer.
            let p = backend.fun.gen_reg();
            writeln!(out, "{}  {} = inttoptr i64 {} to ptr", indent, p, reg).ok();
            writeln!(out, "{}call void @free(ptr {})", indent, p).ok();
        }
        Some(crate::backend::llvm::AllocStrategy::Config(_)) | None => {
            // No tracked heap allocation (scalars, inline values, or an
            // unknown strategy) — the consume destroy is a no-op. Frees only
            // what the allocator explicitly recorded; never free a scalar's
            // value as a pointer.
        }
    }
}

/// 2026-08-07 (Phase 7): collect the names assigned inside a `foreach` body —
/// loop-carried locals need memory slots (the body IR is emitted once).
fn collect_foreach_assigned(stmts: &[Statement], out: &mut std::collections::HashSet<String>) {
    for s in stmts {
        match s {
            Statement::Assign(Expr::Identifier(name), _) => { out.insert(name.clone()); }
            Statement::Guarded(_, body) => collect_foreach_assigned(body, out),
            Statement::Block(body) | Statement::SyncBlock(body) => collect_foreach_assigned(body, out),
            Statement::Foreach { body, .. } => collect_foreach_assigned(body, out),
            _ => {}
        }
    }
}

/// 2026-08-07 (Phase 7): the iteration source of a `foreach` — how the loop
/// counter compares against a bound and how each item value is derived.
enum IterKind {
    /// `0..n` / `0..=n` — the item IS the counter.
    Counter { init: String, bound: String, inclusive: bool },
    /// A Data/String byte buffer ([len][bytes] ptr handle).
    Data { ptr: String, len: String },
    /// 2026-08-14 (String unification): a `#String` operand iterates CHARs —
    /// a protocol-keyed char decode lane (`briev_str_next_char`), never a
    /// hardcoded byte walk. `ptr` is the [len][bytes] handle; `len` is the
    /// stored byte length (`.^Length` header) that bounds the loop; the loop
    /// counter is the BYTE offset, advanced in-place by the lane. The item is
    /// the decoded codepoint as Char (SPEC §17.2 `String` → `Char`).
    String { ptr: String, len: String },
    /// A vector state field (`[N x i64]`).
    // 2026-08-28: element_ty — a Vector(String, [N]) state field iterates
    // String items (boxed handles needing unbox + String-typed binding),
    // never bare Ints (the Phase-1b boundary panic).
    VectorField { gep: String, count: String, element_ty: Type },
    /// 2026-08-12 (Iterable protocol, Tier 2): a collection iterated through
    /// its own operator members — `op Count` for the bound, `op At(i)` per
    /// item (SPEC §11.4). The item is the At member's return value.
    OpCollection { count: String, list: Expr, element_ty: Type },
    /// 2026-08-12 (Iterable protocol, Tier 1): an external-cursor collection
    /// (`op Iter`/`op Step`/`op IsEnd`/`op Current` — SPEC §11.4.1). The
    /// cursor is a plain value; the loop advances it and reads the item
    /// through the ops. No Option/union machinery required.
    Tier1Cursor { iter_reg: String, list: Expr, element_ty: Type },
}

/// 2026-08-07 (Phase 7): classify an emitted collection register as a
/// foreach iteration source — a heap List value (`[len, e0, …]` i64 buffer
/// boxed to an i64 handle) or a Data/String byte buffer ([len][bytes] ptr).
/// Anything else is a hard error (no silent wrongness).
impl LlvmBackend {
    /// 2026-08-12 (Iterable protocol, slice 4): try to iterate a collection
    /// through its operator surface — Tier 2 (`op Count` + `op At`) first,
    /// then Tier 1 (the cursor ops). Returns an IterKind or None. Both the
    /// state-field branch AND the local/non-field fallback use this, so the
    /// hardcoded List layout only remains for a genuinely op-less List value.
    fn try_emit_tier_iteration(
        &mut self,
        out: &mut String,
        list: &Expr,
        indent: &str,
    ) -> Option<IterKind> {
        if let Some((element_ty, _base)) = self.tier2_op_collection(list) {
            let out_tmp = self.fun.gen_reg();
            let count_reg = self.emit_method_call(out, &out_tmp, list, "Count", &[], indent);
            // 2026-08-12 (slice 4, wasm32 maze): the foreach's loop counter
            // slot is i64; the Count result on wasm32 is i32 — widen it so the
            // header `icmp slt i64` matches.
            let count64 = if self.llvm_type(&count_reg.ty) != "i64" {
                let w = self.fun.gen_reg();
                writeln!(out, "{}{} = sext {} {} to i64", indent, w,
                    self.llvm_type(&count_reg.ty), count_reg.name).ok();
                w
            } else {
                count_reg.name.clone()
            };
            return Some(IterKind::OpCollection {
                count: count64,
                list: list.clone(),
                element_ty,
            });
        }
        if let Some((element_ty, _base)) = self.tier1_cursor_collection(list) {
            let out_tmp = self.fun.gen_reg();
            let iter_reg = self.emit_method_call(out, &out_tmp, list, "Iter", &[], indent);
            return Some(IterKind::Tier1Cursor {
                iter_reg: iter_reg.name,
                list: list.clone(),
                element_ty,
            });
        }
        None
    }

    fn foreach_collection_kind(
        &mut self,
        out: &mut String,
        lreg: &TypedRegister,
        indent: &str,
    ) -> IterKind {
        // 2026-08-16 (slice-6 deletion): the hardcoded `[len][elems]`
        // `IterKind::List` arm is DELETED — every List/coll iterable routes
        // through try_emit_tier_iteration (op Count/op At) BEFORE reaching
        // here, so a List value that lands here is a compiler bug (the tier
        // path not firing). A `#String` operand is the char-decode lane; a
        // Blob iterates bytes; anything else directs to the iterable contract.
        if self.is_string_operand(&lreg.ty) {
            // 2026-08-14 (String unification): a `#String` operand iterates
            // CHARs via the decode lane — NOT bytes. `lreg.name` is the
            // [len][bytes] handle (ptr); the loop bound is the stored byte
            // length in the header.
            let len = self.fun.gen_reg();
            writeln!(out, "{}{} = load i64, ptr {}", indent, len, lreg.name).ok();
            IterKind::String { ptr: lreg.name.clone(), len }
        } else if self.is_blob_operand(&lreg.ty) {
            let len = self.fun.gen_reg();
            writeln!(out, "{}{} = load i64, ptr {}", indent, len, lreg.name).ok();
            IterKind::Data { ptr: lreg.name.clone(), len }
        } else {
            panic!(
                "foreach iterable must be a range, a #String/#Blob, or a vector field \
                 with the iterable contract (op Count + op At) — got {:?}",
                lreg.ty
            );
        }
    }
}

// 2026-08-13 (guard on boxed Bool): a `when`/`if` condition is always
// semantically Bool (the typechecker enforces it), but its SSA register may be
// i8 (native Bool), i1 (comparison result), or an i64-boxed Bool (a Bool
// parameter widened by emit_box_param). Reduce any integer condition to i1 for
// `br` — truncating a 0/1 value of any width is exact.
fn guard_cond_i1(backend: &mut LlvmBackend, out: &mut String, indent: &str, cond: &TypedRegister) -> String {
    if cond.ty == Type::bool_() {
        let b = backend.fun.gen_reg();
        writeln!(out, "{}{} = trunc i8 {} to i1", indent, b, cond.name).ok();
        b
    } else if cond.ty == Type::int() {
        let b = backend.fun.gen_reg();
        writeln!(out, "{}{} = trunc i64 {} to i1", indent, b, cond.name).ok();
        b
    } else {
        cond.name.clone()
    }
}

pub fn emit_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &Statement, indent: &str) -> TypedRegister {
    match stmt {
        Statement::Let { name, names, ty, expr, modifiers, .. } => {
            let is_vol = modifiers.iter().any(|m| m.name == "vol");
            // 2026-08-26 (async Phase C): a let bound to a defn spawn (or to
            // another task handle) carries the handle — `free` cancels it.
            match expr.as_ref() {
                Some(crate::ast::Expr::Spawn { type_name, .. })
                    if backend.ctx.defn_params.contains_key(type_name.as_str()) =>
                {
                    backend.fun.task_handle_names.insert(name.clone());
                }
                Some(crate::ast::Expr::Identifier(src))
                    if backend.fun.task_handle_names.contains(src) =>
                {
                    backend.fun.task_handle_names.insert(name.clone());
                }
                _ => {}
            }
            let mut via_scaffolded_construction = false;
            let mut val = match expr {
                Some(crate::ast::Expr::Identifier(alias))
                    if backend.fun.closure_lets.contains_key(alias) =>
                {
                    // 2026-08-06 (fix): `let g = f;` where f is a closure —
                    // g aliases the same env block; calls to g go indirect too.
                    let def = backend.fun.closure_lets.get(alias).cloned().unwrap();
                    backend.fun.closure_lets.insert(name.clone(), def);
                    backend.emit_expr(out, &crate::ast::Expr::Identifier(alias.clone()), indent)
                }
                Some(crate::ast::Expr::Lambda(params, body)) => {
                    // 2026-08-06 (fix): escaping closures. The binding value is
                    // a heap env block `[fn_ptr, cap1..capN]`; calls go indirect
                    // through the stored fn_ptr, and the value can be passed
                    // around (a closure is a real first-class value now).
                    let free_vars =
                        crate::backend::llvm::context::collect_free_vars(body, &params);
                    let symbol =
                        format!("briev_closure_{}", backend.ctx.pending_closures.len());
                    backend
                        .fun
                        .closure_lets
                        .insert(name.clone(), crate::backend::llvm::context::ClosureDef {
                            params: params.clone(),
                            body: body.clone(),
                            free_vars: free_vars.clone(),
                        });
                    backend
                        .ctx
                        .pending_closures
                        .push(crate::backend::llvm::context::PendingClosure {
                            symbol: symbol.clone(),
                            params: params.clone(),
                            body: (**body).clone(),
                            free_vars: free_vars.clone(),
                        });
                    // Env block: [fn_ptr][cap1..capN], 8 bytes per slot.
                    let env_size = 8 * (1 + free_vars.len());
                    let alloc = backend.fun.gen_reg();
                    writeln!(out, "{}{}_p = call ptr @malloc(i64 {})", indent, alloc, env_size).ok();
                    writeln!(out, "{}{} = ptrtoint ptr {}_p to i64", indent, alloc, alloc).ok();
                    let env_p = backend.fun.gen_reg();
                    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, env_p, alloc).ok();
                    let fn_reg = backend.fun.gen_reg();
                    writeln!(out, "{}{} = ptrtoint ptr @{} to i64", indent, fn_reg, symbol).ok();
                    writeln!(out, "{}store i64 {}, ptr {}", indent, fn_reg, env_p).ok();
                    for (j, var) in free_vars.iter().enumerate() {
                        let cap = backend
                            .emit_expr(out, &crate::ast::Expr::Identifier(var.clone()), indent);
                        let slot = backend.fun.gen_reg();
                        writeln!(
                            out,
                            "{}{} = getelementptr i64, ptr {}, i64 {}",
                            indent, slot, env_p, 1 + j
                        )
                        .ok();
                        writeln!(out, "{}store i64 {}, ptr {}", indent, cap.name, slot).ok();
                    }
                    TypedRegister { name: alloc, ty: Type::int() }
                }
                Some(e) => {
                    // 2026-08-01 (E): `vol let x = <rhs>` — loads inside the
                    // RHS are emitted `load volatile` (MMIO semantics: the
                    // value may change externally, so it must never be cached
                    // or eliminated). Reset after the let.
                    if is_vol {
                        backend.fun.volatile_read = true;
                    }
                    // 2026-08-12 (Iterable protocol): a LOCAL collection
                    // (`let xs: List<Int> = [2,4,6]`) constructs through the
                    // collection's own ops (op Init/op InsertAt) — never the
                    // hardcoded [len][elem] heap-seq layout the members can't
                    // read. The binding below resolves the value.
                    // 2026-08-15 (coll plan): a `coll obj` let has a Custom
                    // type (`MyQueue`), so a coll-typed binding is a collection
                    // too — construct it through the scaffolded ops.
                    let constructed = {
                        // 2026-08-17 (storage correctness): a LOCAL collection
                        // is a `coll` keyword type OR an op-surface obj
                        // (declares op Init/InsertAt/Count). is_coll_type
                        // covers `coll` storage; is_op_surface_coll covers a
                        // hand-written obj (HashMap). A local collection can
                        // never be an unpacked column, so it constructs
                        // through the ops (List literal → construct_local_collection;
                        // seed → construct_local_collection_seed).
                        let is_coll = ty.as_ref().map(|t| match t {
                            crate::ast::Type::Applied(n, _) | crate::ast::Type::Custom(n) => {
                                backend.is_coll_type(t) || backend.is_op_surface_coll(n)
                            }
                            _ => false,
                        }).unwrap_or(false);
                        if is_coll && matches!(e, crate::ast::Expr::List(_)) {
                            let briev = ty.clone().unwrap_or(crate::ast::Type::int());
                            backend.construct_local_collection(out, indent, &briev, e)
                        } else if is_coll {
                            // 2026-08-16 (hashmap redesign): a LOCAL collection
                            // with a SEED init (`let m: HashMap<K,V> = 0`)
                            // constructs through `op Init(seed)` — the same op
                            // the state-field path uses. Previously a scalar
                            // seed bound the raw value (NULL handle → member
                            // calls segfaulted) for ANY non-coll obj (RingBuffer,
                            // Stack, HashMap). This is the general fix.
                            // 2026-08-18 (Phase C, BUGS.md arrow-push
                            // double-construct): the previous rule routed EVERY
                            // non-list RHS through `op Init` — `let ks: List<Int>
                            // = b.keys()` became `List.init(<the returned List>)`
                            // = `[<list>]`, a 1-element wrapper (a keys() scan
                            // returned 1 of N). Only a PRIMITIVE RHS is a genuine
                            // seed. Emit the RHS ONCE, then decide by its result
                            // type: a collection value binds directly; a scalar
                            // constructs through `op Init` with the emitted
                            // register.
                            let briev = ty.clone().unwrap_or(crate::ast::Type::int());
                            let rhs = backend.emit_expr(out, e, indent);
                            let rhs_is_coll = {
                                let base = match &rhs.ty {
                                    crate::ast::Type::Vector(inner, _) => match inner.as_ref() {
                                        crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.as_str(),
                                        _ => "",
                                    },
                                    crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => {
                                        n.as_str()
                                    }
                                    _ => "",
                                };
                                backend.is_coll_type(&rhs.ty)
                                    || (!base.is_empty() && backend.is_op_surface_coll(base))
                            };
                            if rhs_is_coll {
                                Some(rhs)
                            } else {
                                backend.construct_local_collection_seed(out, indent, &briev, rhs)
                            }
                        } else {
                            None
                        }
                    };
                    via_scaffolded_construction = constructed.is_some();
                    let v = match constructed {
                        Some(v) => v,
                        None => backend.emit_expr(out, e, indent),
                    };
                    backend.fun.volatile_read = false;
                    v
                }
                None => {
                    let v = backend.fun.gen_reg();
                    let llvm_ty = ty.as_ref().map(|t| backend.llvm_type(t)).unwrap_or("i64".into());
                    writeln!(out, "{}{} = alloca {}", indent, v, llvm_ty).ok();
                    TypedRegister { name: v, ty: ty.clone().unwrap_or(Type::int()) }
                }
            };
            // 2026-08-17 (tuple correctness, plan
            // 2026-08-17-hashmap-storage-tuple-correctness.md): TUPLE
            // DESTRUCTURE `let (a, b) = t`. The parser + typechecker already
            // support it (check_let_destructure binds each name to the element
            // type); codegen DROPPED the `names` list (only `name` = names[0]
            // bound, to the WHOLE boxed handle). The value is a boxed i64
            // handle to the emit_tuple `[len, e0, e1, …]` heap block; each
            // name binds to `GEP i64 slot (i+1)` + load. The typechecker bound
            // the element types; recover them from the declared tuple type
            // (fallback Int for unannotated — most tuple elements are scalars).
            if names.len() > 1 {
                let elem_tys: Vec<Type> = match ty {
                    Some(crate::ast::Type::Tuple(ts)) => ts.clone(),
                    _ => vec![Type::int(); names.len()],
                };
                let handle_p = backend.fun.gen_reg();
                writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, handle_p, val.name).ok();
                for (i, n) in names.iter().enumerate() {
                    let slot = backend.fun.gen_reg();
                    writeln!(
                        out,
                        "{}{} = getelementptr i64, ptr {}, i64 {}",
                        indent, slot, handle_p, i + 1
                    )
                    .ok();
                    let elem = backend.fun.gen_reg();
                    writeln!(out, "{}{} = load i64, ptr {}", indent, elem, slot).ok();
                    backend.fun.let_bindings.insert(n.clone(), elem.clone());
                    backend.fun.let_binding_types
                        .insert(n.clone(), elem_tys.get(i).cloned().unwrap_or_else(Type::int));
                    backend.fun.let_original_types
                        .insert(n.clone(), elem_tys.get(i).cloned().unwrap_or_else(Type::int));
                }
                return TypedRegister { name: val.name, ty: Type::void() };
            }
            // 2026-08-04 (compiler-in-Briev): a top-level let that is reassigned
            // later was PRE-BOUND to an entry-block alloca (emit_definition's
            // pre-declaration). Store the value into that alloca and keep the
            // binding — reassignments then store into the same entry alloca,
            // never demoting at the assignment site (dominance violation).
            if backend.fun.reassigned_lets.contains_key(name) {
                if let Some(slot) = backend.fun.let_bindings.get(name).cloned() {
                    let store_ty = backend.llvm_type(&val.ty);
                    let store_val = backend.ensure_typed_value(out, indent, &store_ty, &val.name, Some(val.ty.clone()), backend.ctx.type_universe.clone().as_ref());
                    writeln!(out, "{}store {} {}, ptr {}", indent, store_ty, store_val, slot).ok();
                    backend.fun.let_binding_allocas.insert(slot.clone());
                    // 2026-08-28 (String ABI fix): the pre-declaration
                    // (emit_definition) registered a PROVISIONAL Int type —
                    // the real let knows the declared/inferred type. Update
                    // both type maps so later reads (and field access on
                    // boxed obj handles) resolve the true type; the Int
                    // provisional made `let sb: SB = ...; sb = f(sb);`
                    // reads lose the obj type permanently.
                    let bind_ty = ty.clone().unwrap_or_else(|| val.ty.clone());
                    backend.fun.let_binding_types.insert(name.clone(), bind_ty.clone());
                    backend.fun.let_original_types.insert(name.clone(), bind_ty);
                    return TypedRegister { name: val.name, ty: val.ty.clone() };
                }
            }
            // 2026-07-18: Track alloca bindings so identifier codegen loads values.
            if expr.is_none() {
                backend.fun.let_binding_allocas.insert(val.name.clone());
            }
            // 2026-09-07 (swan-song dominance fix): a let-local READ by the
            // pending post-hoist binds through an alloca (deferred to the loop
            // preheader with the other struct allocas) instead of its body SSA
            // register — the exit-block hoisted print loads the slot, which
            // dominates the exit and holds the last-iteration value. A body
            // register referenced from the exit block is a dominance violation
            // (async-ready-gate: `produced` from briev_await). Mirrors the
            // reassigned-lets entry-alloca path above.
            let is_struct_ty = match &val.ty {
                Type::Custom(n) | Type::Applied(n, _) => backend.ctx.struct_types.contains_key(n),
                _ => false,
            };
            if backend.fun.swan_song_locals.contains(name)
                && !backend.fun.let_binding_allocas.contains(&val.name)
                && !is_struct_ty
            {
                let slot_ty = backend.llvm_type(&val.ty);
                let slot = backend.fun.next_reg_with_prefix("sslv");
                if backend.fun.defer_struct_allocas {
                    backend.fun.pending_struct_allocas.push(
                        format!("  {} = alloca {}, align 8", slot, slot_ty),
                    );
                } else {
                    writeln!(out, "{}{} = alloca {}, align 8", indent, slot, slot_ty).ok();
                }
                let store_val = backend.ensure_typed_value(
                    out, indent, &slot_ty, &val.name,
                    Some(val.ty.clone()), None,
                );
                writeln!(out, "{}store {} {}, ptr {}", indent, slot_ty, store_val, slot).ok();
                backend.fun.let_binding_allocas.insert(slot.clone());
                val = crate::backend::llvm::TypedRegister { name: slot, ty: val.ty.clone() };
            }
            // 2026-07-24: Transfer struct literal alloca tracking from result
            // register to variable name, so &let_var retrieves the stack address.
            if let Some(alloca) = backend.fun.struct_literal_allocas.remove(&val.name) {
                backend.fun.struct_literal_allocas.insert(name.clone(), alloca);
            }
            backend.fun.let_bindings.insert(name.clone(), val.name.clone());
            // 2026-08-01 (E): a `vol let` binds a volatile local — stores
            // THROUGH it (`x[i] = v`, `*x = v`) emit `store volatile`.
            if is_vol {
                backend.fun.volatile_locals.insert(name.clone());
            }
            // 2026-07-31 (A5): a declared type wins over the emitted register's
            // type — a `let st: Stack = Stack()` local must bind as `Stack`
            // (the emitted struct address is i64/Int), so method calls and
            // field access on it resolve the struct.
            let bind_ty = ty.clone().unwrap_or_else(|| val.ty.clone());
            backend.fun.let_binding_types.insert(name.clone(), bind_ty.clone());
            backend.fun.let_original_types.insert(name.clone(), bind_ty);
            // 2026-08-16 (three-track Phase 2, D2 pre-grow): a LOCAL coll whose
            // intra-firing peak exceeds the default cap gets ONE `EnsureCap#(q,
            // peak)` here — at construction, before ANY push. Emitted at the
            // LET site (not the foreach arm) because the coll may be pushed
            // BEFORE the loop too. Only for the scaffolded op construction
            // path (the returned `val.name` is the coll handle then, not a
            // generic heap-seq value) AND a HEAP-GROWABLE coll — a fixed
            // `T[N]` coll has a fixed buffer, EnsureCap# would corrupt it.
            // The grow guard strip elsewhere keys on this same (txn, coll_name)
            // fact.
            let growable_base = ty.as_ref().and_then(|t| match t {
                crate::ast::Type::Custom(n) => Some(n.split('<').next().unwrap_or(n)),
                crate::ast::Type::Applied(n, _) => Some(n.as_str()),
                _ => None,
            }).is_some_and(|b| backend.ctx.coll_storage.get(b)
                == Some(&crate::backend::llvm::coll_scaffold::CollStorage::HeapGrowable));
            if via_scaffolded_construction && growable_base {
                let txn_name = backend.fun.txn_name.clone();
                if let Some(peak) = backend.ctx.coll_pregrow.get(&(txn_name, name.clone())).copied() {
                    let call = crate::ast::Expr::Call(
                        "EnsureCap#".to_string(),
                        vec![
                            crate::ast::Expr::Identifier(name.clone()),
                            crate::ast::Expr::Decimal(peak),
                        ],
                        None,
                    );
                    backend.emit_expr(out, &call, indent);
                }
            }
            TypedRegister { name: val.name, ty: Type::void() }
        }
        Statement::Assign(lhs, rhs) => {
            // 2026-07-17: Pop: `x <- &queue` → Assign(Identifier(x), AddrOf(source)).
            // Detect this pattern BEFORE emitting the RHS (which would get the
            // address, not the popped value). Emit the ring buffer pop directly.
            // 2026-07-18: Pop — emit call @fn_name(handle), store result to lhs.
            // 2026-07-20: Uses find_extract_strategy (reads OperatorDef from context).
            if let Expr::AddrOf(source) = rhs {
                let strat = backend.find_extract_strategy(source)
                    .or_else(|| backend.find_extract_strategy(rhs));
                let Some(op_def) = strat else {
                    return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                };
                let Some(result) = emit_strategy_fn_call(backend, out, indent, source, &op_def.clone(), None) else {
                    return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                };
                // Store popped result to the LHS variable.
                let Expr::Identifier(name) = lhs else {
                    return TypedRegister { name: result, ty: Type::int() };
                };
                if let Some(reg) = backend.fun.let_bindings.get(name) {
                    writeln!(out, "{}store i64 {}, ptr {}", indent, result, reg).ok();
                } else if backend.ctx.field_index_map.contains_key(name) {
                    backend.emit_state_store_i64(out, indent, name, &result);
                }
                return TypedRegister { name: result, ty: Type::int() };
            }

            let val = backend.emit_expr(out, rhs, indent);
            match lhs {
                Expr::Identifier(name) => {
                    // 2026-08-07 (object instance pools): a bare member
                    // target in an UNPACKED member body writes the instance's
                    // top-level slot — `total = 1` in `st`'s member → the
                    // `st.total` field.
                    // 2026-08-18 (param/field shadowing, WRITE side): a
                    // member-body PARAMETER or local shadows a same-named
                    // instance field on WRITES too — symmetric with the read
                    // guard in emit_expr.rs. Without this, `cap = cap + 100`
                    // READ the param but STORED into the `{prefix}.cap`
                    // column (read and write resolved to different storage —
                    // lexical scoping must resolve both sides identically). A
                    // shadowed name falls through to the local-binding store
                    // below (which allocas the param/local). Nulling the
                    // prefix is the flat way to skip the field path.
                    let pooled_prefix = if backend.fun.let_bindings.contains_key(name) {
                        None
                    } else {
                        backend.fun.self_prefix.clone()
                    };
                    if let Some((prefix, row_reg)) = pooled_prefix {
                        let slot = format!("{}.{}", prefix, name);
                        if let Some(&idx) = backend.ctx.field_index_map.get(&slot) {
                            let field_ty = backend.ctx.field_types[idx].clone();
                            // 2026-08-07 (object instance pools): a DEPENDENT
                            // column is a heap buffer — load the buffer
                            // address from the slot and GEP the row inside it
                            // (mirrors emit_instance_column_row's heap path).
                            let gep = if let Some(elem_ty) = backend.ctx.heap_columns.get(&idx).cloned() {
                                let base = backend.emit_state_gep(out, indent, "m", "%state", idx);
                                let addr = backend.fun.gen_reg();
                                writeln!(out, "{}{} = load i64, ptr {}", indent, addr, base).ok();
                                let buf = backend.fun.gen_reg();
                                writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, buf, addr).ok();
                                let row = backend.fun.gen_reg();
                                writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, row, elem_ty, buf, row_reg).ok();
                                let field_ty = elem_ty.clone();
                                row
                            } else {
                                let base = backend.emit_state_gep(out, indent, "m", "%state", idx);
                                let gep = backend.fun.gen_reg();
                                let col_ty = backend.ctx.field_types[idx].clone();
                                writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}", indent, gep, col_ty, base, row_reg).ok();
                                gep
                            };
                            // Mirror the standard top-level field store: use the
                            // slot's actual LLVM type and box Ptr/float values.
                            let val_ty = backend.llvm_type(&val.ty);
                            if val_ty == field_ty {
                                writeln!(out, "{}store {} {}, ptr {}", indent, field_ty, val.name, gep).ok();
                            } else {
                                let boxed = backend.adapt_to_i64(out, indent, &val);
                                writeln!(out, "{}store i64 {}, ptr {}", indent, boxed, gep).ok();
                            }
                            backend.fun.last_val_temps.insert(name.clone(), val.name.clone());
                            backend.fun.last_val_types.insert(name.clone(), val.ty.clone());
                            return TypedRegister { name: val.name, ty: Type::void() };
                        }
                    }
                    // 2026-07-31 (A5): obj member `self` slot write — a bare
                    // slot name in a member body stores to self+offset.
                    // 2026-08-18: same shadow guard as above for the BOXED
                    // self-slot write — a shadowed name writes its own alloca,
                    // never the receiver's field slot.
                    let self_binding = if backend.fun.let_bindings.contains_key(name) {
                        None
                    } else {
                        backend.fun.self_binding.clone()
                    };
                    if let Some((self_type, self_ptr)) = &self_binding {
                        let is_self_slot = backend.ctx.struct_types.get(self_type)
                            .map_or(false, |f| f.iter().any(|(n, _)| n == name));
                        if is_self_slot {
                            let offset = backend.lookup_field_offset(self_type, name);
                            let gep = backend.fun.gen_reg();
                            writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, gep, self_ptr, offset).ok();
                            let (slot_ty, _) = backend.ctx.struct_types.get(self_type)
                                .and_then(|f| f.iter().find(|(n, _)| n == name))
                                .map(|(_, ty)| (ty.clone(), ()))
                                .unwrap_or((Type::int(), ()));
                            // 2026-08-13 (Phase 5): an `atomic` self-slot stores
                            // with an atomic store.
                            if let Some(ordering) = backend.atomic_field_ordering(self_type, name) {
                                let salt = if matches!(slot_ty, Type::Ptr(_)) {
                                    format!("i{}", backend.ctx.int_bits)
                                } else {
                                    backend.llvm_type(&slot_ty)
                                };
                                let sasz = crate::backend::llvm::types::type_size(
                                    &slot_ty, backend.ctx.type_universe.as_ref(),
                                ).max(1);
                                let store_val = backend.ensure_typed_value(
                                    out, indent, &salt, &val.name, Some(val.ty.clone()),
                                    backend.ctx.type_universe.clone().as_ref(),
                                );
                                writeln!(out, "{}store atomic {} {}, ptr {} {}, align {}", indent, salt, store_val, gep, ordering, sasz).ok();
                                backend.fun.last_val_temps.insert(name.clone(), val.name.clone());
                                backend.fun.last_val_types.insert(name.clone(), val.ty.clone());
                                return TypedRegister { name: val.name, ty: Type::void() };
                            }
                            // 2026-08-13 (pack): a packed self-slot stores its
                            // bit-slice (L-M-S for sub-byte, plain aligned
                            // store for whole-byte) — skip the scalar packing.
                            if let Some(pf) = backend.packed_field(self_type, name) {
                                out.push_str(&backend.emit_packed_field_store(indent, &gep, &pf, &val));
                                backend.fun.last_val_temps.insert(name.clone(), val.name.clone());
                                backend.fun.last_val_types.insert(name.clone(), val.ty.clone());
                                return TypedRegister { name: val.name, ty: Type::void() };
                            }
                            // 2026-08-01 (D3): a Ptr-typed self-slot stores the
                            // HANDLE at i{int_bits} (the value is already
                            // ptrtoint'd) — not `ptr`, matching the self-slot
                            // read. 2026-08-11: width-aware — a wasm32 pointer
                            // slot is i32, not hardcoded i64.
                            let llvm_ty = if matches!(slot_ty, Type::Ptr(_)) {
                                format!("i{}", backend.ctx.int_bits)
                            } else {
                                backend.llvm_type(&slot_ty)
                            };
                            let store_val = backend.ensure_typed_value(out, indent, &llvm_ty, &val.name, Some(val.ty.clone()), backend.ctx.type_universe.clone().as_ref());
                            writeln!(out, "{}store {} {}, ptr {}", indent, llvm_ty, store_val, gep).ok();
                            backend.fun.last_val_temps.insert(name.clone(), val.name.clone());
                            backend.fun.last_val_types.insert(name.clone(), val.ty.clone());
                            return TypedRegister { name: val.name, ty: Type::void() };
                        }
                    }
                    // 2026-08-12 (slice 4): a FLAT dotted state field
                    // (`main.count = ...` from the component-instance rewrite)
                    // stores through the state slot. Previously this fell
                    // through and the assignment was silently DROPPED (the web
                    // counter's increment never moved the count; the volatile
                    // swan-song store to ptr 0 was all that remained).
                    if backend.ctx.field_index_map.contains_key(name) {
                        // 2026-08-22 (Phase 3b): a structural-sum FIELD stores
                        // the tagged handle — box a member value at the seam.
                        let fidx = *backend.ctx.field_index_map.get(name).unwrap();
                        let field_briev_ty = backend
                            .ctx
                            .field_briev_types
                            .get(fidx)
                            .cloned()
                            .unwrap_or_else(Type::int);
                        let val = if matches!(field_briev_ty, Type::Union(_))
                            && field_briev_ty != val.ty
                        {
                            backend.wrap_union_value(out, indent, &val, &field_briev_ty)
                        } else {
                            val
                        };
                        // 2026-08-13 (merge fix): adapt the value to the SLOT's
                        // LLVM type before storing. A String/Data value is a
                        // `ptr` that must be ptrtoint'd to the i64 slot, a Bool
                        // value boxes to i64 but the slot is i8 (trunc), a Float
                        // value bitcasts to the i64 slot. Struct/collection
                        // HANDLES are already i64 and pass through unchanged
                        // (ensure_typed_value). Previously the raw value was
                        // stored (`store i64 %ptr`) — an LLVM type error on
                        // String state fields (node_bridge.save, main's
                        // flat-dotted branch).
                        let fidx = *backend.ctx.field_index_map.get(name).unwrap();
                        let fty = backend.ctx.field_types.get(fidx).cloned().unwrap_or_else(|| "i64".to_string());
                        let store_val = backend.ensure_typed_value(
                            out, indent, &fty, &val.name, Some(val.ty.clone()),
                            backend.ctx.type_universe.clone().as_ref(),
                        );
                        backend.emit_state_store_i64(out, indent, name, &store_val);
                        // NOTE: do NOT insert into last_val_temps here —
                        // that map persists across txn emissions (only
                        // emit_member_body saves/restores it), so a stale
                        // register would leak into a later txn's contract
                        // read (the undefined-%t12 reset_0 regression). The
                        // state slot is the source of truth; a subsequent read
                        // loads it.
                        return TypedRegister { name: val.name, ty: Type::void() };
                    }
                    // 2026-07-18: Push — emit call @fn_name(handle, val).
                    // 2026-07-20: Uses find_insert_strategy (reads OperatorDef from context).
                    // 2026-07-31 (A6): when the InsertAt op is bound to an obj
                    // member (`op InsertAt: push(#Lh, #Rh)`), emit a self-bound
                    // member call instead of the free-function marker dispatch.
                    let insert_strat = backend.find_insert_strategy(lhs).cloned();
                    if let Some(op_def) = &insert_strat {
                        if emit_strategy_member_call(backend, out, indent, lhs, op_def, Some(&val.name)).is_none() {
                            emit_strategy_fn_call(backend, out, indent, lhs, op_def, Some(&val.name));
                        }
                    } else if let Some(reg) = backend.fun.let_bindings.get(name).cloned() {
                        // 2026-07-18: If the binding is a value register (not an alloca),
                        // the variable is being mutated — create an alloca and redirect.
                        let is_alloca = backend.fun.let_binding_allocas.contains(&reg)
                            || backend.fun.param_slots.values().any(|s| s == &reg);
                        let slot = if is_alloca {
                            reg
                        } else {
                            let slot = backend.fun.gen_reg();
                            // 2026-07-31: The slot type must match the binding's Briev
                            // type — an outlined guard param that is a FLOAT state field
                            // (e.g. `sum = 0.0` reset in accumulator_flush) is a `float`
                            // register; boxing it as i64 produces
                            // `store i64 %__cp_sum` on a float param (type error).
                            let slot_ty = backend.fun.let_binding_types.get(name)
                                .map(|t| backend.llvm_type(t))
                                .unwrap_or_else(|| "i64".to_string());
                            writeln!(out, "{}{} = alloca {}, align 8", indent, slot, slot_ty).ok();
                            writeln!(out, "{}store {} {}, ptr {}", indent, slot_ty, reg, slot).ok();
                            backend.fun.let_bindings.insert(name.clone(), slot.clone());
                            backend.fun.let_binding_allocas.insert(slot.clone());
                            slot
                        };
                        let store_ty = backend.llvm_type(&val.ty);
                        writeln!(out, "{}store {} {}, ptr {}", indent, store_ty, val.name, slot).ok();
                    // 2026-07-14: Handle MMIO and regular state field assignments
                    } else if let Some(&addr) = backend.ctx.mmio_fields.get(name) {
                        let ptr = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, addr).ok();
                        // 2026-07-14: volatile store type must match val.ty — hardcoded i64 breaks MMIO bools
                        let store_ty = backend.llvm_type(&val.ty);
                        writeln!(out, "{}store volatile {} {}, ptr {}", indent, store_ty, val.name, ptr).ok();
                    } else if let Some(&idx) = backend.ctx.field_index_map.get(name) {
                        let ptr = backend.emit_state_gep(out, indent, "as", "%state", idx);
                        // 2026-07-19: Store with native type from field_types.
                        // When the value's LLVM type matches the field type, store
                        // directly (no boxing). Otherwise box via adapt_to_i64.
                        let field_ty = &backend.ctx.field_types[idx];
                        let val_ty = backend.llvm_type(&val.ty);
                        if val_ty == *field_ty {
                            writeln!(out, "{}store {} {}, ptr {}", indent, field_ty, val.name, ptr).ok();
                        } else {
                            let boxed = backend.adapt_to_i64(out, indent, &val);
                            writeln!(out, "{}store i64 {}, ptr {}", indent, boxed, ptr).ok();
                        }
                    }
                }
                // 2026-07-17: Push: `&queue <- value` → Assign(AddrOf(target), value).
                // The `&` on the LHS is optional — the type-based check above handles
                // the bare-identifier case. The AddrOf arm is kept for explicit usage.
                // 2026-07-17: Dereference-assign: `*ptr = val`. Compute the
                // pointer address and store the value through it. Supports
                // pointer-offset arithmetic (buf + N) via GEP in emit_expr.
                Expr::Deref(inner) => {
                    let ptr_reg = backend.emit_expr(out, inner, indent);
                    let store_ty = backend.llvm_type(&val.ty);
                    // 2026-07-30: Ptr values are stored as i64 internally;
                    // convert back to LLVM ptr before storing through.
                    let store_ptr = if matches!(ptr_reg.ty, Type::Ptr(_)) {
                        let p = backend.fun.gen_reg();
                        backend.emit_inttoptr(out, indent, &p, &ptr_reg.name);
                        p.to_string()
                    } else {
                        ptr_reg.name.clone()
                    };
                    writeln!(out, "{}store {} {}, ptr {}", indent, store_ty, val.name, store_ptr).ok();
                }
                // 2026-07-17: Pointer-indexed store — data[idx] = val.
                // Emits inttoptr + GEP + store for Ptr-typed objects.
                // List/tuple literals need idx+1 (slot 0 = length header).
                Expr::Index(obj, idx) => {
                    // 2026-07-31 (A4): array state-field store
                    // (`f[i] = v` where f: Float[16]) and the Ptr/collection
                    // store. Flattened with guard clauses — see
                    // emit_array_state_store.
                    if emit_array_state_store(backend, out, indent, obj, idx, &val) {
                        return TypedRegister { name: val.name, ty: Type::void() };
                    }
                    // 2026-07-31 (A5): obj member `self` ARRAY slot store —
                    // `data[i] = v` in a member body.
                    if emit_self_slot_array_store(backend, out, indent, obj, idx, &val) {
                        return TypedRegister { name: val.name, ty: Type::void() };
                    }
                    let obj_reg = backend.emit_expr(out, obj, indent);
                    // 2026-08-07 (Phase 7): a multi-dim ROW VIEW store —
                    // `m[i][j] = v`: the outer obj is `Index(m, i)`, whose
                    // register is a ptr into the aggregate (typed Vector with
                    // the remaining dims). GEP the row at `j` + store.
                    if let Type::Vector(inner, dims) = &obj_reg.ty {
                        if !dims.is_empty() {
                            let idx_reg = backend.emit_expr(out, idx, indent);
                            let agg_ty = backend.vector_array_llvm_type(&obj_reg.ty)
                                .unwrap_or_else(|| "i64".to_string());
                            let elem = backend.fun.gen_reg();
                            let gep_idx = backend.gep_index(out, indent, &idx_reg);
                            writeln!(
                                out,
                                "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}",
                                indent, elem, agg_ty, obj_reg.name, gep_idx
                            )
                            .ok();
                            let inner_llvm = backend.llvm_type(inner);
                            let universe = backend.ctx.type_universe.clone();
                            let store_val = backend.ensure_typed_value(
                                out,
                                indent,
                                &inner_llvm,
                                &val.name,
                                Some(val.ty.clone()),
                                universe.as_ref(),
                            );
                            writeln!(out, "{}store {} {}, ptr {}", indent, inner_llvm, store_val, elem).ok();
                            return TypedRegister { name: val.name, ty: Type::void() };
                        }
                    }
                    if matches!(obj_reg.ty, Type::Ptr(_)) {
                        let idx_reg = backend.emit_expr(out, idx, indent);
                        let ptr = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, obj_reg.name).ok();
                        let gep = backend.fun.gen_reg();
                        let offset = backend.fun.gen_reg();
                        // 2026-08-11 (wasm32 obj-member fix): the index is an
                        // `Int` value at i{int_bits} (i32 on wasm32) — widen it
                        // to the i64 GEP index via gep_index (sext on narrow
                        // targets, a no-op on x86_64) instead of using the raw
                        // register. The old `add i64 {idx}, 0` broke wasm32
                        // (`%t38 defined with type 'i32' but expected 'i64'`).
                        let gep_idx = backend.gep_index(out, indent, &idx_reg);
                        if matches!(obj.as_ref(), Expr::List(_) | Expr::Tuple(_)) {
                            writeln!(out, "{}{} = add i64 {}, 1", indent, offset, gep_idx).ok();
                        } else {
                            writeln!(out, "{}{} = add i64 {}, 0", indent, offset, gep_idx).ok();
                        }
                        writeln!(out, "{}{} = getelementptr i64, ptr {}, i64 {}", indent, gep, ptr, offset).ok();
                        // 2026-08-01 (E): `vol let p` — stores through the
                        // local (`p[i] = v`) emit `store volatile` (MMIO
                        // register arrays).
                        let vol_obj = match obj.as_ref() {
                            Expr::Identifier(n) => backend.fun.volatile_locals.contains(n),
                            _ => false,
                        };
                        // 2026-08-04 (compiler-in-Briev): collection slots are
                        // i64 — a String element (a ptr) must be ptrtoint'd
                        // before the store (`inner.data[len] = val`), or
                        // `store i64 <ptr>, ptr` is invalid IR.
                        let e64 = backend.adapt_to_i64(out, indent, &val);
                        writeln!(out, "{}store {}i64 {}, ptr {}", indent,
                            if vol_obj { "volatile " } else { "" }, e64, gep).ok();
                    }
                }
                // 2026-08-01 (D3): field store — `obj.name = val`. The receiver
                // register holds the struct ADDRESS (struct self-slot, local
                // struct, or a nested Field returning the sub-struct address);
                // GEP the field offset and store. Ptr-typed fields store the
                // i64 handle. Previously this fell to `_ => {}` and the store
                // was silently dropped (List's `inner.data = Malloc#(...)`).
                Expr::Field(obj, name) => {
                    // 2026-08-12 (Iterable protocol, slice 2 gap 2): a POOLED
                    // instance target (`c.count = 5`) stores into the
                    // `{base}.{name}` column at row 0 — emitting the receiver
                    // as a box handle produces an undefined `@c` global.
                    if let Expr::Identifier(rname) = obj.as_ref() {
                        if let Some((base, row_reg)) = backend.instance_prefix_for(rname) {
                            let slot = format!("{}.{}", base, name);
                            if let Some(&idx) = backend.ctx.field_index_map.get(&slot) {
                                let col_ty = backend.ctx.field_types[idx].clone();
                                let base_gep = backend.emit_state_gep(out, indent, "m", "%state", idx);
                                let gep = backend.fun.gen_reg();
                                writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}", indent, gep, col_ty, base_gep, row_reg).ok();
                                let field_ty = backend.ctx.field_briev_types[idx].clone();
                                let llvm_ty = if matches!(field_ty, Type::Ptr(_)) {
                                    "i64".to_string()
                                } else {
                                    backend.llvm_type(&field_ty)
                                };
                                let store_val = backend.ensure_typed_value(
                                    out, indent, &llvm_ty, &val.name, Some(val.ty.clone()),
                                    backend.ctx.type_universe.clone().as_ref(),
                                );
                                writeln!(out, "{}store {} {}, ptr {}", indent, llvm_ty, store_val, gep).ok();
                                return TypedRegister { name: val.name, ty: Type::void() };
                            }
                        }
                    }
                    // 2026-08-23 (bugfix): `(*p).field = v` — the receiver
                    // through a deref'd struct pointer is its boxed i64
                    // address (see LlvmBackend::deref_struct_receiver); no
                    // by-value load.
                    let obj_reg = match backend.deref_struct_receiver(out, indent, obj) {
                        Some(r) => r,
                        None => backend.emit_expr(out, obj, indent),
                    };
                    let Some(obj_key) = backend.resolve_obj_key(&obj_reg.ty) else {
                        return TypedRegister { name: val.name, ty: Type::void() };
                    };
                    let offset = backend.lookup_field_offset(&obj_key, name);
                    let ptr = backend.fun.gen_reg();
                    writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, ptr, obj_reg.name).ok();
                    let gep = backend.fun.gen_reg();
                    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, gep, ptr, offset).ok();
                    // 2026-08-13 (Phase 5): an `atomic` state-field store.
                    // `obj.f = obj.f + c` / `obj.f = obj.f - c` lower to
                    // atomicrmw add/sub (read-modify-write); anything else is
                    // an atomic store (SPEC §8.2).
                    if let Some(ordering) = backend.atomic_field_ordering(&obj_key, name) {
                        let field_ty = backend.ctx.struct_types.get(&obj_key)
                            .and_then(|f| f.iter().find(|(n, _)| n == name))
                            .map(|(_, ty)| ty.clone())
                            .unwrap_or_else(|| Type::int());
                        let alt = if matches!(field_ty, Type::Ptr(_)) {
                            format!("i{}", backend.ctx.int_bits)
                        } else {
                            backend.llvm_type(&field_ty)
                        };
                        let asz = crate::backend::llvm::types::type_size(
                            &field_ty, backend.ctx.type_universe.as_ref(),
                        ).max(1);
                        let rmw = match rhs {
                            Expr::BinaryOp(op, l, r)
                                if matches!(op, crate::ast::BinaryOpKind::Add | crate::ast::BinaryOpKind::Sub) =>
                            {
                                let same_field = matches!(l.as_ref(), Expr::Field(r2, f2) if **r2 == **obj && f2 == name);
                                if same_field {
                                    let cval = backend.emit_expr(out, r, indent);
                                    let op_name = if matches!(op, crate::ast::BinaryOpKind::Add) { "add" } else { "sub" };
                                    Some((op_name, cval.name))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };
                        if let Some((op_name, cval)) = rmw {
                            let old = backend.fun.gen_reg();
                            writeln!(out, "{}{} = atomicrmw {} ptr {}, i64 {} {}", indent, old, op_name, gep, cval, ordering).ok();
                        } else {
                            let store_val = backend.ensure_typed_value(
                                out, indent, &alt, &val.name, Some(val.ty.clone()),
                                backend.ctx.type_universe.clone().as_ref(),
                            );
                            writeln!(out, "{}store atomic {} {}, ptr {} {}, align {}", indent, alt, store_val, gep, ordering, asz).ok();
                        }
                        return TypedRegister { name: val.name, ty: Type::void() };
                    }
                    // 2026-08-13 (pack): a packed state-field store writes the
                    // bit-slice into the byte image (L-M-S for sub-byte, plain
                    // aligned store for whole-byte).
                    if let Some(pf) = backend.packed_field(&obj_key, name) {
                        out.push_str(&backend.emit_packed_field_store(indent, &gep, &pf, &val));
                    } else {
                    let field_ty = backend.ctx.struct_types.get(&obj_key)
                        .and_then(|f| f.iter().find(|(n, _)| n == name))
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or_else(|| Type::int());
                    let llvm_ty = if matches!(field_ty, Type::Ptr(_)) {
                        "i64".to_string()
                    } else {
                        backend.llvm_type(&field_ty)
                    };
                    let store_val = backend.ensure_typed_value(
                        out, indent, &llvm_ty, &val.name, Some(val.ty.clone()),
                        backend.ctx.type_universe.clone().as_ref(),
                    );
                    writeln!(out, "{}store {} {}, ptr {}", indent, llvm_ty, store_val, gep).ok();
                    }
                }
                _ => {}
            }
            TypedRegister { name: val.name, ty: Type::void() }
        }
        Statement::Expression(expr) => {
            // 2026-07-17: Discard: `<- &queue` → Expression(AddrOf(source)).
            // Pop from collection and discard the result.
            // 2026-07-20: Uses find_extract_strategy (reads OperatorDef from context).
            // 2026-08-01 (A10): member-bound ExtractFrom (op ExtractFrom: pop(#Rh))
            // dispatches to the self-bound member call first — the free-function
            // dispatch only applies to convention-based fn bindings.
            if let Expr::AddrOf(source) = expr {
                let strat = backend.find_extract_strategy(source)
                    .or_else(|| backend.find_extract_strategy(expr));
                if let Some(op_def) = strat {
                    let op = op_def.clone();
                    if emit_strategy_member_call(backend, out, indent, source, &op, None).is_none() {
                        emit_strategy_fn_call(backend, out, indent, source, &op, None);
                    }
                }
                TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
            } else {
                backend.emit_expr(out, expr, indent)
            }
        }
        Statement::FreeHint(name) => {
            // 2026-08-26 (async Phase C): a freed TASK HANDLE cancels its
            // scheduler entry — the compiled twin of the interpreter's
            // runtime cancellation. Ordinary frees keep the Phase 5 path.
            if backend.fun.task_handle_names.contains(name) {
                if let Some(h) = backend.fun.let_bindings.get(name).cloned() {
                    writeln!(out,
                        "{indent}call void @briev_task_cancel(i64 {h})").ok();
                    let _ = backend.fun.gen_reg();
                    return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                }
            }
            // 2026-08-01 (Phase 5): `free x;` — emit the strategy-aware free
            // of x's backing. The field/local must be heap-backed for the free
            // to be meaningful; emit_destroy_consumed no-ops for inline/arena/
            // scalar backings.
            emit_destroy_consumed(
                backend,
                out,
                indent,
                &crate::ast::Expr::Identifier(name.clone()),
            );
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::KeepHint(_) => {
            // 2026-08-01 (Phase 5): `keep x;` is a scheduler directive — no
            // runtime emission (it suppresses the auto-free at analysis time).
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::ArrowAssign { target, value, consume } => {
            // 2026-08-26 (async Phase D): FIRING an event wire — `w <- v;`
            // where w binds an Event<_> (the i64 slot id). The compiled twin
            // of the interpreter's Phase B fire: payload stored + waiters
            // woken inside briev_event_fire.
            if let Some(Expr::Identifier(wname)) = target.as_ref().map(|t| t.as_ref()) {
                let is_wire = backend
                    .fun
                    .let_original_types
                    .get(wname)
                    .or_else(|| backend.fun.let_binding_types.get(wname))
                    .map(is_event_type)
                    .unwrap_or(false);
                if is_wire {
                    if let Some(id_reg) = backend.fun.let_bindings.get(wname).cloned() {
                        let val = backend.emit_expr(out, value, indent);
                        let boxed = backend.adapt_to_i64(out, indent, &val);
                        writeln!(out,
                            "{indent}call void @briev_event_fire(i64 {id_reg}, i64 {boxed})")
                        .ok();
                        return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                    }
                }
            }
            // 2026-08-01 (Phase 3): the arrow — find the collection by the op
            // binding on each side:
            //   - the TARGET is a stream symbol (#StdOut/#StdErr) → a write;
            //   - the TARGET has an InsertAt binding → INSERT (push): emit the
            //     member/fn insert call with the value;
            //   - the VALUE has an ExtractFrom/CopyFrom binding → READ/EXTRACT
            //     (pop): emit the extract and store the result into the target
            //     (or discard it when target is None);
            //   - otherwise → a plain copy store into the target.
            if let Some(t) = target.as_ref() {
                if let Expr::Identifier(name) = t.as_ref() {
                    if name == "#StdOut" || name == "#StdErr" {
                        // 2026-08-01 (Phase 4): a stream write. `#StdOut` lowers
                        // to the generic Print# (any type); `#StdErr` writes a
                        // String via the stderr printer.
                        if name == "#StdOut" {
                            let call = Expr::Call("Print#".to_string(), vec![(**value).clone()], None);
                            backend.emit_expr(out, &call, indent);
                        } else {
                            let v = backend.emit_expr(out, value, indent);
                            let reg = backend.fun.gen_reg();
                            writeln!(out, "{}{} = call i64 @__eprint_str(ptr {})", indent, reg, v.name).ok();
                        }
                        return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                    }
                }
            }
            if let Some(t) = target.as_ref() {
                if let Some(op_def) = backend.find_insert_strategy(t).cloned() {
                    // 2026-08-18 (Phase D, PiggyBank): only PUSH when the value
                    // actually matches the target's element type (mirrors the
                    // typechecker's arrow_ok). `all ~<- piggy` — target
                    // `all: List<Int>`, value a PiggyBank — must fall through to
                    // the EXTRACT (smash), not push the jar handle as an i64.
                    // The value is emitted into a scratch buffer and only spliced
                    // in when the push actually fires (a type-mismatched value
                    // would otherwise leave dead IR behind).
                    let mut val_buf = String::new();
                    let val = {
                        let out2 = &mut val_buf;
                        backend.emit_expr(out2, value, indent)
                    };
                    let elem_ty = backend.insert_element_type(t);
                    // 2026-08-18: an identifier VALUE that is a pooled instance
                    // (`all ~<- piggy`) emits with the scalar Int FALLBACK type —
                    // resolve its DECLARED type so the insert gate rejects the
                    // push and falls through to the extract.
                    // 2026-09-07 (tuple-literal push): a STRUCT LITERAL value
                    // (`(keys[i], vals[i])`) also emits with the fallback Int
                    // type — the emitted register carries no tuple info. When
                    // the value is a Tuple expr, resolve its element types from
                    // the sub-expressions so the push gate sees the real type.
                    let val_ty = match value.as_ref() {
                        crate::ast::Expr::Identifier(n) => {
                            backend.resolve_id_type(n).unwrap_or_else(|| val.ty.clone())
                        }
                        crate::ast::Expr::Tuple(elems) => {
                            let etys = elems.iter().map(|e| {
                                match e {
                                    crate::ast::Expr::Identifier(n) => {
                                        backend.resolve_id_type(n)
                                            .unwrap_or_else(|| crate::ast::Type::int())
                                    }
                                    _ => crate::ast::Type::int(),
                                }
                            }).collect();
                            crate::ast::Type::Tuple(etys)
                        }
                        _ => val.ty.clone(),
                    };
                    let matches = elem_ty.as_ref().map_or(true, |et| {
                        // 2026-08-18 (Phase D): a BARE generic param (`K` in
                        // `PiggyBank<K>` — the backend instance type carries no
                        // concrete args) can't be compared — allow the push (the
                        // typechecker already validated it; the member body is
                        // type-agnostic, storing i64 handles). Only a CONCRETE
                        // mismatch (`List<Int>` target vs a PiggyBank value) must
                        // fall through to the extract.
                        let bare_generic = matches!(et, crate::ast::Type::Custom(n)
                            if n.len() == 1 && n.chars().next().unwrap().is_uppercase());
                        // 2026-09-07 (tuple-literal push): a Tuple element type
                        // with generic params (e.g. `(K, V)`) matches a concrete
                        // tuple of the same arity — the generic params are
                        // instantiated to the element types at the use site.
                        let tuple_arity_match = matches!((et, &val_ty),
                            (crate::ast::Type::Tuple(a), crate::ast::Type::Tuple(b))
                                if a.len() == b.len());
                        bare_generic
                            || *et == val_ty
                            || tuple_arity_match
                            || (matches!(et, crate::ast::Type::Custom(n) if n == "Int")
                                && matches!(&val_ty, crate::ast::Type::Custom(n) if n == "Int"))
                    });
                    if matches {
                        out.push_str(&val_buf);
                        if emit_strategy_member_call(backend, out, indent, t, &op_def, Some(&val.name)).is_none() {
                            emit_strategy_fn_call(backend, out, indent, t, &op_def, Some(&val.name));
                        }
                        return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                    }
                }
            }
            if let Some(op_def) = backend.find_extract_strategy_for_arrow(value, *consume).cloned() {
                // EXTRACT — the value is the collection. Member-bound
                // ExtractFrom (e.g. the Stack's self-bound `pop`) dispatches to
                // the member call, which returns the popped value; the
                // free-function convention is the fallback. The result is
                // stored into the target (or discarded). 2026-08-18 (Phase D):
                // the CONSUME flag picks CopyFrom (`<-`) vs ExtractFrom (`~<-`).
                let result = match emit_strategy_member_call(backend, out, indent, value, &op_def, None) {
                    Some(r) => r,
                    None => match emit_strategy_fn_call(backend, out, indent, value, &op_def, None) {
                        Some(r) => r,
                        None => return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() },
                    },
                };
                if let Some(t) = target.as_ref() {
                    emit_arrow_store(backend, out, indent, t, &result);
                }
                if *consume {
                    // 2026-08-01 (Phase 3): a destructive extract also destroys
                    // the consumed collection's backing (strategy-aware free).
                    emit_destroy_consumed(backend, out, indent, value);
                }
                return TypedRegister { name: result, ty: Type::int() };
            }
            // Plain copy — the arrow as a normal assignment, emitted only when
            // the target is a resolvable local/state field. When nothing
            // resolves, the statement is a no-op — matching the pre-Phase-3
            // behavior where the unresolvable `<-` fell through silently.
            if let Some(t) = target.as_ref() {
                if let Expr::Identifier(name) = t.as_ref() {
                    // 2026-08-18 (Phase C): a pooled member-field target
                    // (`items` → `p.items`) is resolvable too — the bare name
                    // falls through to the instance column, else the plain
                    // copy would be a silently-dropped no-op.
                    let slot = backend
                        .fun
                        .self_prefix
                        .as_ref()
                        .map(|(prefix, _)| format!("{}.{}", prefix, name))
                        .filter(|s| backend.ctx.field_index_map.contains_key(s));
                    let resolvable = backend.fun.let_bindings.contains_key(name)
                        || backend.ctx.field_index_map.contains_key(name)
                        || slot.is_some();
                    if resolvable {
                        let val = backend.emit_expr(out, value, indent);
                        emit_arrow_store_local(backend, out, indent, name, &val);
                    }
                }
            }
            if *consume {
                emit_destroy_consumed(backend, out, indent, value);
            }
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::Term(val) => {
            // 2026-07-26: Phase 4 — webstack flush at term.
            // 2026-08-10: real update batch — each field the transaction wrote
            // (transition-graph write_set) is stored into @__web_flush_buf as a
            // {handle, value_ptr, value_len} record, then __web_flush_state is
            // called with the buffer + count. The JS shim applies the DOM
            // mutations synchronously before the transaction completes.
            if backend.ctx.webstack_enabled {
                emit_web_flush_batch(backend, out, indent);
            }
            // 2026-08-09 (Phase 10): run deferred cleanup before the firing
            // exits (term is a successful completion).
            backend.flush_defer_cleanup(out, indent);
            if let Some(val) = val {
                let mut reg = backend.emit_expr(out, val, indent);
                // 2026-08-01 (C3): a boxed Float param returned from a defn
                // (`term v`) is an i64 handle — unbox through the float cache so
                // `ret float` receives the actual float, not the handle.
                if let Some(cached) = backend.fun.reg_float_cache.get(&reg.name).cloned() {
                    reg = TypedRegister { name: cached, ty: reg.ty.clone() };
                }
                // 2026-08-01 (D3): a defn MEMBER's `term <expr>` records its
                // value register so emit_member_body can return it (callable
                // txns use callable_txn_result; standalone defns use `ret`).
                // 2026-08-07 (object instance pools): an UNPACKED member body
                // has self_binding = None (the self is the prefix) — the
                // self_prefix path must record member_result too.
                if (backend.fun.self_binding.is_some() || backend.fun.self_prefix.is_some())
                    && backend.fun.callable_txn_result.is_none()
                {
                    backend.fun.member_result = Some((reg.name.clone(), reg.ty.clone()));
                }
                if backend.fun.callable_txn_result.is_some() {
                    // 2026-07-18: In a callable txn, term stores to %result and
                    // branches to post (convergence loop). The 'ret' is at done:.
                    let val_ty = backend.llvm_type(&reg.ty);
                    let store_name = if val_ty != backend.fun.fn_ret_ty {
                        if val_ty == "i64" && backend.fun.fn_ret_ty == "ptr" {
                            let c = backend.fun.gen_reg();
                            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, c, reg.name).ok();
                            c
                        } else if val_ty == "ptr" && backend.fun.fn_ret_ty == "i64" {
                            let c = backend.fun.gen_reg();
                            writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, c, reg.name).ok();
                            c
                        } else {
                            reg.name
                        }
                    } else {
                        reg.name
                    };
                    if let Some(ref result_slot) = backend.fun.callable_txn_result {
                        writeln!(out, "{}store {} {}, ptr {}", indent, backend.fun.fn_ret_ty, store_name, result_slot).ok();
                    }
                    if let Some(ref post_label) = backend.fun.callable_txn_post_label {
                        writeln!(out, "{}br label %{}", indent, post_label).ok();
                    }
                    backend.fun.terminated = true;
                } else if backend.fun.member_result.is_some() {
                    // 2026-08-04 (term-termination-diagnostics): INLINED member
                    // body (emit_member_body -> emit_statement_sequence): this
                    // `term <val>` is the member's return value, captured above
                    // in member_result and taken by emit_member_body. It is NOT
                    // a control-flow exit of the enclosing function — emitting
                    // `ret void` here broke the countdown loop (queue_drain's
                    // `<- queue` pop): the loop emitter keeps emitting after the
                    // ret, producing invalid IR ("value doesn't match function
                    // result type 'i32'"). 2026-08-12 (slice 4): check BEFORE
                    // `fn_ret_ty != "void"` — an inlined member inside a
                    // NON-VOID function (a `__view_items_<field>` materializer
                    // returns i32) previously hit the standalone-ret branch and
                    // terminated the enclosing function after the member's term.
                    // Emit no terminator and leave `terminated` unchanged so the
                    // enclosing body continues.
                } else if backend.fun.fn_ret_ty != "void" {
                    // 2026-07-26: Use actual expression LLVM type, not hardcoded "i64".
                    // Frgn calls may return ptr (for String/Data in C ABI).
                    let val_ty = backend.llvm_type(&reg.ty);
                    let final_name = if val_ty != backend.fun.fn_ret_ty {
                        // 2026-07-20: Insert type conversion when the expression type doesn't
                        // match the function's declared return type (e.g., SysCall# returns i64
                        // but function returns ptr → need inttoptr).
                        if val_ty == "i64" && backend.fun.fn_ret_ty == "ptr" {
                            let c = backend.fun.gen_reg();
                            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, c, reg.name).ok();
                            c
                        } else if val_ty == "ptr" && backend.fun.fn_ret_ty == "ptr"
                                  && matches!(reg.ty, Type::Ptr(_)) {
                            // 2026-07-30: Ptr values stored as i64 internally — register
                            // is i64 but llvm_type(Ptr) returns "ptr". Need inttoptr.
                            let c = backend.fun.gen_reg();
                            writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, c, reg.name).ok();
                            c
                        } else {
                            reg.name
                        }
                    } else if val_ty == "ptr" && matches!(reg.ty, Type::Ptr(_)) {
                        // 2026-07-30: Ptr values stored as i64 internally — register
                        // is i64 but llvm_type(Ptr) returns "ptr". Need inttoptr.
                        let c = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, c, reg.name).ok();
                        c
                    } else {
                        reg.name
                    };
                    writeln!(out, "{}ret {} {}", indent, backend.fun.fn_ret_ty, final_name).ok();
                    backend.fun.terminated = true;
                } else if backend.fun.member_result.is_some() {
                    // 2026-08-04 (term-termination-diagnostics): INLINED member
                    // body (emit_member_body -> emit_statement_sequence): this
                    // `term <val>` is the member's return value, captured above
                    // in member_result and taken by emit_member_body. It is NOT
                    // a control-flow exit of the enclosing function — emitting
                    // `ret void` here broke the countdown loop (queue_drain's
                    // `<- queue` pop): the loop emitter keeps emitting after the
                    // ret, producing invalid IR ("value doesn't match function
                    // result type 'i32'"). Emit no terminator and leave
                    // `terminated` unchanged so the enclosing body continues,
                    // matching the interpreter's member-call frame semantics.
                } else {
                    // 2026-08-04 (term-termination-diagnostics): a value-form
                    // `term <val>`/`term! <val>` in a void function unwinds the
                    // transaction body (interpreter TermReturn in
                    // src/interpreter/eval.rs:646-657). Emit a REAL terminator:
                    // in the SSA main loop, branch to the current txn's
                    // next-txn label (skipping the rest of THIS txn's body); in
                    // per-txn void functions, return. Without a real terminator
                    // the guard.thenN / body block was left dangling whenever
                    // the Guarded handler skipped its convergence branch.
                    if let Some(ref abort) = backend.fun.void_txn_abort_label {
                        writeln!(out, "{}br label %{}", indent, abort).ok();
                    } else {
                        writeln!(out, "{}ret void", indent).ok();
                    }
                    backend.fun.terminated = true;
                }
            } else {
                // 2026-08-04 (term-termination-diagnostics): bare `term;` /
                // `term!;` is a convergence checkpoint, NOT a terminator — the
                // interpreter returns Ok(Void) and continues to the next
                // statement (src/interpreter/eval.rs:646-657, 707-709). Setting
                // terminated=true here made the async/callable/pre void paths
                // stop the body mid-way, diverging from the interpreter. It now
                // stays false so the body keeps emitting; the enclosing
                // epilogue still terminates the function.
            }
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::EndProgram(val) => {
            // 2026-08-06 (endprogram plan): `endprogram` genuinely exits the
            // process (SPEC §11.5) — unlike `term`, which ends the transaction.
            // Emit the value's side effects (the print), then call the
            // runtime's `__exit` (briev_rt.c, runs atexit cleanup) with the
            // value's i64 result as the exit code (adapt_to_i64); the bare
            // form exits 0.
            if backend.ctx.webstack_enabled {
                emit_web_flush_batch(backend, out, indent);
            }
            let code = if let Some(v) = val {
                let reg = backend.emit_expr(out, v, indent);
                backend.adapt_to_i64(out, indent, &reg)
            } else {
                "0".to_string()
            };
            writeln!(out, "{}call void @__exit(i64 {})", indent, code).ok();
            // The process exit never returns — the terminator after it is
            // unreachable, but LLVM requires one.
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::Guarded(cond, body) => {
            let cond_reg = backend.emit_expr(out, cond, indent);
            // 2026-08-22 (Phase 6b): last_val_temps are INTRA-GUARD values —
            // a register computed inside `when` A is not dominated by the
            // blocks of `when` B (a later `endprogram Print#(total_out)`
            // reusing guard A's temp emitted dominance-invalid IR). Scope
            // the cache to the guard: reads after the merge reload from the
            // state slot like any cross-block read. Same save/restore shape
            // as emit_member_body.
            let saved_lvt = backend.fun.last_val_temps.clone();
            let saved_lvt_types = backend.fun.last_val_types.clone();
            // labels need a counter without % prefix — gen_reg() returns %tN
            let label_n = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let then_lbl = format!("guard.then{}", label_n);
            let end_lbl = format!("guard.end{}", label_n);
            let cond_i1 = guard_cond_i1(backend, out, indent, &cond_reg);
            writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, cond_i1, then_lbl, end_lbl).ok();
            writeln!(out, "{}{}:", indent, then_lbl).ok();
            backend.fun.terminated = false;
            for stmt in body {
                emit_statement(backend, out, stmt, indent);
            }
            backend.fun.last_val_temps = saved_lvt;
            backend.fun.last_val_types = saved_lvt_types;
            // 2026-08-04 (term-termination-diagnostics): REWRITTEN from the
            // 2026-07-19 "always emit br" version. That version was a
            // workaround: a value-form term!/term in a void txn set
            // terminated=true WITHOUT emitting a real LLVM terminator, so the
            // guard.thenN block dangled and the unconditional convergence
            // branch was required to produce valid IR — at the cost of
            // falling through past the term (interpreter divergence: TermReturn
            // unwinds the whole body, not just the guard). The void term path
            // now emits a real terminator (emit_stmt.rs value-form void arm),
            // so this convergence branch is emitted only when the body did NOT
            // terminate: the true path skips the rest of the txn body, the
            // false path continues at guard.endN.
            if !backend.fun.terminated {
                writeln!(out, "{}br label %{}", indent, end_lbl).ok();
            }
            writeln!(out, "{}{}:", indent, end_lbl).ok();
            // Reset so the false-path (guard condition not met) continues
            // emitting the rest of the body after guard.endN.
            backend.fun.terminated = false;
            // 2026-08-15 (coll grow-on-full): record the block the emitter is
            // now writing into. The countdown loop's latch phis key their body
            // predecessor on `fun.cur_block` (the FINAL block of the emitted
            // body) — a `when`-ended body must report its merge label, not the
            // loop body block, or the phi's predecessor list mismatches the
            // CFG (invalid IR; clang's LoopDeletionPass then crashes). The
            // countdown engine's own If/Guarded handlers already do this.
            backend.fun.cur_block = Some(end_lbl.clone());
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::Block(stmts) => {
            let mut last = TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
            for stmt in stmts {
                last = emit_statement(backend, out, stmt, indent);
            }
            last
        }
        Statement::Defer(body) => {
            // 2026-08-09 (Phase 10): register the cleanup body on the current
            // firing's defer stack; flush_defer_cleanup emits it LIFO before
            // every exit (term/rollback/fallthrough ret). No code is emitted
            // at the registration point — the cleanup runs later.
            backend.fun.defer_bodies.push(body.clone());
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::SyncBlock(stmts) => {
            // 2026-09-08 (deprecation audit): `sync { }` is the LEGACY spelling
            // of `mutex { }` (lexer.rs "replaces the legacy sync {}"). It was
            // previously swallowed by the `_ =>` catch-all — the body silently
            // vanished. Emit it exactly like Mutex (a serial section emits
            // inline; sequential execution is the default). `sync<group>` is
            // the SEPARATE group-barrier modifier and is unaffected.
            let mut last = TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
            for stmt in stmts {
                last = emit_statement(backend, out, stmt, indent);
            }
            last
        }
        Statement::Mutex(stmts) => {
            // 2026-08-09 (Phase 10): `mutex` is a serial section — sequential
            // execution IS the default (a modifier must never be a speedup),
            // so the body emits inline with no added synchronization.
            let mut last = TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
            for stmt in stmts {
                last = emit_statement(backend, out, stmt, indent);
            }
            last
        }
        Statement::Barrier { body, .. } => {
            // 2026-08-09 (Phase 10): `barrier<group>` holds members until all
            // fire — the no-implicit-concurrency gate classifies the pair. In
            // the single-threaded default the barrier body emits inline (the
            // barrier is a scheduling contract, not a parallelization hint).
            let mut last = TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
            for stmt in body {
                last = emit_statement(backend, out, stmt, indent);
            }
            last
        }
        Statement::Rollback(_) => {
            // 2026-08-09 (Phase 10): deferred cleanup runs on rollback too —
            // the firing aborts but registered cleanup still executes.
            backend.flush_defer_cleanup(out, indent);
            // 2026-08-09: the ret type must match the function's return type
            // (a reactive txn is void; a value-returning defn returns its ty).
            if backend.fun.fn_ret_ty == "void" {
                writeln!(out, "{}ret void", indent).ok();
            } else {
                let zero = if backend.fun.fn_ret_ty == "float" || backend.fun.fn_ret_ty == "double" {
                    "0.0".to_string()
                } else if backend.fun.fn_ret_ty == "ptr" {
                    "null".to_string()
                } else {
                    "0".to_string()
                };
                writeln!(out, "{}ret {} {}", indent, backend.fun.fn_ret_ty, zero).ok();
            }
            backend.fun.terminated = true;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-08-17 (foreach break): `break;` — branch to the innermost
        // enclosing foreach's `end` label (search-until-found early exit).
        // Valid only inside a foreach body (the typechecker enforces it).
        Statement::Break => {
            match backend.fun.foreach_break_labels.last() {
                Some(label) => {
                    writeln!(out, "{}br label %{}", indent, label).ok();
                }
                None => {
                    panic!(
                        "'break' reached codegen with no enclosing foreach — \
                         the typechecker must reject this (BreakOutsideLoop)"
                    );
                }
            }
            backend.fun.terminated = true;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-08-07 (Phase 7): `foreach(item in iterable)` — the sole
        // iteration keyword (SPEC §11.4). This commit lowers ITERABLE
        // RANGES (`0..n` / `0..=n`) as a counted loop; collections (List /
        // Data / vector fields) are the interpreter reference today and a
        // codegen follow-up (hard error here, no silent wrongness).
        Statement::Foreach { item, list, body } => {
            // 2026-08-07 (Phase 7): loop-carried locals — a register-bound let
            // (e.g. `acc`) assigned in the body would read its STALE initial
            // register every iteration (the body IR is emitted once; runtime
            // re-execution never re-resolves). Pre-declare an alloca slot and
            // seed it with the current value so the body reads/writes memory.
            let mut body_assigned = std::collections::HashSet::new();
            collect_foreach_assigned(body, &mut body_assigned);
            for name in body_assigned {
                if let Some(cur) = backend.fun.let_bindings.get(&name).cloned() {
                    if !backend.fun.let_binding_allocas.contains(&cur) {
                        let slot = backend.fun.gen_reg();
                        let llvm_ty = backend.fun.let_binding_types.get(&name)
                            .map(|t| backend.llvm_type(t))
                            .unwrap_or_else(|| "i64".to_string());
                        writeln!(out, "{}{} = alloca {}, align 8", indent, slot, llvm_ty).ok();
                        writeln!(out, "{}store {} {}, ptr {}", indent, llvm_ty, cur, slot).ok();
                        backend.fun.let_bindings.insert(name.clone(), slot.clone());
                        backend.fun.let_binding_allocas.insert(slot.clone());
                    }
                }
            }
            // Determine the iteration source (SPEC §11.4 — ranges AND
            // collections are iterable). The loop counter is a memory slot;
            // the item value is the counter (ranges) or a container element.
            let iter = match list.as_ref() {
                Expr::Range { start, end, inclusive } => {
                    let s = backend.emit_expr(out, start, indent);
                    let e = backend.emit_expr(out, end, indent);
                    IterKind::Counter { init: s.name, bound: e.name, inclusive: *inclusive }
                }
                // 2026-08-12 (Iterable protocol): a POOLED instance
                // (`let m = spawn HashMap(0)`) unpacks into columns — the
                // identifier isn't a field, but its base is a Tier-1/Tier-2
                // collection; run the structural tier resolution for it too.
                Expr::Identifier(name)
                    if backend.ctx.field_index_map.get(name).is_some()
                        || backend.unpacked_instance_prefix(name).is_some() =>
                {
                    let fidx = backend.ctx.field_index_map.get(name).copied();
                    let is_vector = matches!(&fidx, Some(idx) if matches!(
                        backend.ctx.field_briev_types.get(*idx),
                        Some(t) if matches!(t, Type::Vector(_, _))));
                    if is_vector {
                        let fidx = fidx.unwrap();
                        let gep = backend.emit_state_gep(out, indent, "f", "%state", fidx);
                        let field_ty = backend.ctx.field_briev_types.get(fidx).cloned();
                        let element_ty = match &field_ty {
                            Some(Type::Vector(inner, _)) => (**inner).clone(),
                            _ => Type::int(),
                        };
                        let n = field_ty
                            .map(|t| backend.vector_element_count(&t))
                            .unwrap_or(0) as i64;
                        let count = backend.fun.gen_reg();
                        writeln!(out, "{}{} = add i64 0, {}", indent, count, n).ok();
                        IterKind::VectorField { gep, count, element_ty }
                    } else if let Some(iter) = backend.try_emit_tier_iteration(out, list, indent) {
                        iter
                    } else {
                        // A non-vector state field is not iterable.
                        let lreg = backend.emit_expr(out, list, indent);
                        backend.foreach_collection_kind(out, &lreg, indent)
                    }
                }
                _ => {
                    // 2026-08-12 (slice 4, Iterable protocol): a LOCAL
                    // collection (`let xs: List<Int> = ...` then `foreach x in
                    // xs`) iterates through the op surface too — previously the
                    // `_` fallback hardcoded the List layout.
                    if let Some(iter) = backend.try_emit_tier_iteration(out, list, indent) {
                        iter
                    } else {
                        let lreg = backend.emit_expr(out, list, indent);
                        backend.foreach_collection_kind(out, &lreg, indent)
                    }
                }
            };
            let label_n = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let header = format!("foreach.hdr{}", label_n);
            let body_lbl = format!("foreach.body{}", label_n);
            let end_lbl = format!("foreach.end{}", label_n);
            let slot = backend.fun.gen_reg();
            writeln!(out, "{}{} = alloca i64", indent, slot).ok();
            // Header compare setup. Tier 1 (cursor) stores the Iter() result
            // and checks IsEnd (a bool exit); the counter/tier-2 forms use the
            // icmp bound compare.
            let (init_reg, bound_reg, cmp_op) = match &iter {
                IterKind::Counter { init, bound, inclusive } => {
                    (init.clone(), bound.clone(), if *inclusive { "sle" } else { "slt" })
                }
                IterKind::Data { len, .. } | IterKind::String { len, .. } => {
                    let zero = backend.fun.gen_reg();
                    writeln!(out, "{}{} = add i64 0, 0", indent, zero).ok();
                    (zero, len.clone(), "slt")
                }
                IterKind::OpCollection { count, .. } => {
                    let zero = backend.fun.gen_reg();
                    writeln!(out, "{}{} = add i64 0, 0", indent, zero).ok();
                    (zero, count.clone(), "slt")
                }
                IterKind::Tier1Cursor { iter_reg, .. } => {
                    (iter_reg.clone(), String::new(), "")
                }
                IterKind::VectorField { count, .. } => {
                    let zero = backend.fun.gen_reg();
                    writeln!(out, "{}{} = add i64 0, 0", indent, zero).ok();
                    (zero, count.clone(), "slt")
                }
            };
            writeln!(out, "{}store i64 {}, ptr {}", indent, init_reg, slot).ok();
            writeln!(out, "{}br label %{}", indent, header).ok();
            writeln!(out, "{}{}:", indent, header).ok();
            let cur = backend.fun.gen_reg();
            writeln!(out, "{}{} = load i64, ptr {}", indent, cur, slot).ok();
            if let IterKind::Tier1Cursor { list, .. } = &iter {
                // Tier 1: exit when the cursor is past the end — the IsEnd op.
                let cur_tmp = "__foreach_cur".to_string();
                backend.fun.let_bindings.insert(cur_tmp.clone(), cur.clone());
                backend.fun.let_binding_types.insert(cur_tmp.clone(), Type::int());
                backend.fun.let_original_types.insert(cur_tmp.clone(), Type::int());
                let arg = Expr::Identifier(cur_tmp);
                let out_tmp = backend.fun.gen_reg();
                let end = backend.emit_method_call(out, &out_tmp, list, "IsEnd", &[arg], indent);
                let end_i1 = backend.fun.gen_reg();
                // 2026-08-12: the IsEnd op returns a native Bool (i8); the
                // loop condition needs i1 — the standard Bool truncation.
                writeln!(out, "{}{} = trunc i8 {} to i1", indent, end_i1, end.name).ok();
                writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, end_i1, end_lbl, body_lbl).ok();
            } else {
                let cmp = backend.fun.gen_reg();
                writeln!(
                    out,
                    "{}{} = icmp {} i64 {}, {}",
                    indent, cmp, cmp_op, cur, bound_reg
                )
                .ok();
                writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, cmp, body_lbl, end_lbl).ok();
            }
            writeln!(out, "{}{}:", indent, body_lbl).ok();
            // Derive the item value for this iteration.
            let item_reg = match &iter {
                IterKind::Counter { .. } => cur.clone(),
                // 2026-08-12 (Iterable protocol, Tier 2): the item is the
                // collection's `op At(i)` member call — structural iteration,
                // never a hardcoded layout.
                IterKind::OpCollection { list, element_ty, .. } => {
                    let counter_tmp = "__foreach_cur".to_string();
                    // 2026-08-12 (slice 4, wasm32 maze): the loop counter slot
                    // is i64 but the At INDEX param is Int (i32 on wasm32) —
                    // truncate before the call so the member body's i32 index
                    // matches (a raw i64 arg typed Int produced `sext i32 <i64>`).
                    let cur_i = if backend.ctx.int_bits == 32 {
                        let t = backend.fun.gen_reg();
                        writeln!(out, "{}{} = trunc i64 {} to i32", indent, t, cur).ok();
                        t
                    } else {
                        cur.clone()
                    };
                    backend.fun.let_bindings.insert(counter_tmp.clone(), cur_i);
                    backend.fun.let_binding_types.insert(counter_tmp.clone(), Type::int());
                    backend.fun.let_original_types.insert(counter_tmp.clone(), Type::int());
                    let arg = Expr::Identifier(counter_tmp);
                    let out_tmp = backend.fun.gen_reg();
                    let mut at = backend.emit_method_call(out, &out_tmp, list, "At", &[arg], indent);
                    // 2026-08-12 (slice 2 String gap): a String/Data element is
                    // the [len][bytes] ADDRESS stored as an i64 handle in the
                    // collection — the item must be the ptr representation
                    // (inttoptr) so `==`/`briev_str_eq` and string consumers
                    // see a ptr, not the raw handle. `element_ty` is the
                    // DECLARED element type (the At member's return after
                    // substitution), which the load's register type may not be.
                    // 2026-08-28 (Bug #5 family): only unbox a BOXED i64 handle
                    // (at.ty == Int). The At member body may have already
                    // recovered the element to a ptr (the String-typed register
                    // path) — inttoptr of a ptr is a double unbox that reads
                    // garbage lengths downstream.
                    if (backend.is_string_operand(element_ty) || backend.is_blob_operand(element_ty))
                        && at.ty == Type::int()
                    {
                        let p = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, at.name).ok();
                        at.name = p;
                    }
                    at.name
                }
                // 2026-08-12 (Iterable protocol, Tier 1): the item is the
                // collection's `op Current(cur)` member call at the cursor.
                IterKind::Tier1Cursor { list, element_ty, .. } => {
                    let cur_tmp = "__foreach_cur".to_string();
                    backend.fun.let_bindings.insert(cur_tmp.clone(), cur.clone());
                    backend.fun.let_binding_types.insert(cur_tmp.clone(), Type::int());
                    backend.fun.let_original_types.insert(cur_tmp.clone(), Type::int());
                    let arg = Expr::Identifier(cur_tmp);
                    let out_tmp = backend.fun.gen_reg();
                    let mut item = backend.emit_method_call(out, &out_tmp, list, "Current", &[arg], indent);
                    // 2026-08-28: same boxed-only guard as the Tier-2 arm above.
                    if (backend.is_string_operand(element_ty) || backend.is_blob_operand(element_ty))
                        && item.ty == Type::int()
                    {
                        let p = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, item.name).ok();
                        item.name = p;
                    }
                    item.name
                }
                IterKind::Data { ptr, .. } => {
                    let off = backend.fun.gen_reg();
                    writeln!(out, "{}{} = add i64 {}, 8", indent, off, cur).ok();
                    let elem_p = backend.fun.gen_reg();
                    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, elem_p, ptr, off).ok();
                    let raw = backend.fun.gen_reg();
                    writeln!(out, "{}{} = load i8, ptr {}", indent, raw, elem_p).ok();
                    let elem = backend.fun.gen_reg();
                    writeln!(out, "{}{} = zext i8 {} to i64", indent, elem, raw).ok();
                    elem
                }
                // 2026-08-14 (String unification): `#String` iterates CHARs.
                // The loop counter slot holds the BYTE offset; the decode lane
                // reads the codepoint at that offset and advances the slot in
                // place (no separate increment — see the loop step below). The
                // item is the codepoint, truncated to Char's native i32.
                IterKind::String { ptr, .. } => {
                    let cp = backend.fun.gen_reg();
                    writeln!(out, "{}{} = call i64 @briev_str_next_char(ptr {}, ptr {})", indent, cp, ptr, slot).ok();
                    let ch = backend.fun.gen_reg();
                    writeln!(out, "{}{} = trunc i64 {} to i32", indent, ch, cp).ok();
                    ch
                }
                IterKind::VectorField { gep, element_ty, .. } => {
                    let elem_p = backend.fun.gen_reg();
                    writeln!(out, "{}{} = getelementptr i64, ptr {}, i64 {}", indent, elem_p, gep, cur).ok();
                    let elem = backend.fun.gen_reg();
                    writeln!(out, "{}{} = load i64, ptr {}", indent, elem, elem_p).ok();
                    // 2026-08-28: a String/Blob element in the slot is a boxed
                    // i64 handle — unbox to the [len][bytes] ptr so the item
                    // register matches its element type.
                    if backend.is_string_operand(element_ty) || backend.is_blob_operand(element_ty) {
                        let p = backend.fun.gen_reg();
                        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, p, elem).ok();
                        p
                    } else {
                        elem
                    }
                }
            };
            // Bind the loop variable so the body resolves it like a `let`.
            // 2026-08-12 (Iterable protocol, Tier 2): the item type is the
            // collection's ELEMENT type (the At member's return), not a forced
            // Int — `foreach(x in strList)` binds x as String.
            let item_ty = match &iter {
                IterKind::OpCollection { element_ty, .. } => element_ty.clone(),
                IterKind::Tier1Cursor { element_ty, .. } => element_ty.clone(),
                IterKind::VectorField { element_ty, .. } => element_ty.clone(),
                // 2026-08-14 (String unification): a `#String` foreach item is
                // a Char (the decode lane's codepoint), matching the
                // typechecker's `foreach_item_type` derivation.
                IterKind::String { .. } => Type::char_(),
                _ => Type::int(),
            };
            backend.fun.last_val_temps.insert(item.clone(), item_reg.clone());
            backend.fun.last_val_types.insert(item.clone(), item_ty.clone());
            backend.fun.let_bindings.insert(item.clone(), item_reg.clone());
            backend.fun.let_binding_types.insert(item.clone(), item_ty.clone());
            backend.fun.let_original_types.insert(item.clone(), item_ty);
            backend.fun.terminated = false;
            // 2026-08-17 (foreach break): make the innermost foreach's `end`
            // label the break target while the body runs (popped after).
            backend.fun.foreach_break_labels.push(end_lbl.clone());
            for stmt in body {
                emit_statement(backend, out, stmt, indent);
            }
            backend.fun.foreach_break_labels.pop();
            // 2026-08-18 (Phase E, BUGS.md SSA-main destructure): the loop
            // variable is scoped to the foreach BODY — remove its binding after
            // the body, or a later same-named `let`/destructure in the SAME
            // transaction resolves through the stale `last_val_temps` entry
            // (last_val_temps wins over let_bindings in emit_expr). With the
            // leak, `foreach k in ks { }` before an insert whose member
            // destructures `let (k, v) = e` bound k to the POISONED foreach
            // register in the second emission pass (the alwaysinline @txn_go
            // copy + the SSA-main replay), so the hash probe read a register
            // defined by a LATER statement (undefined forward ref, wrong
            // inserts/gets). The item's let_bindings entry is also removed —
            // the item is dead after the loop.
            backend.fun.last_val_temps.remove(item);
            backend.fun.last_val_types.remove(item);
            backend.fun.let_bindings.remove(item);
            backend.fun.let_binding_types.remove(item);
            backend.fun.let_original_types.remove(item);
            if !backend.fun.terminated {
                // 2026-08-14 (String unification): the `#String` decode lane
                // advanced the byte-offset slot IN PLACE — re-storing `cur + 1`
                // would clobber the advance (and re-loop forever). Skip the
                // store and branch straight back.
                if matches!(iter, IterKind::String { .. }) {
                    writeln!(out, "{}br label %{}", indent, header).ok();
                    writeln!(out, "{}{}:", indent, end_lbl).ok();
                    backend.fun.terminated = false;
                    return TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() };
                }
                let next = if let IterKind::Tier1Cursor { list, .. } = &iter {
                    // 2026-08-12 (Iterable protocol, Tier 1): advance the
                    // cursor via the collection's `op Step(cur)` member.
                    let cur_tmp = "__foreach_cur".to_string();
                    backend.fun.let_bindings.insert(cur_tmp.clone(), cur.clone());
                    backend.fun.let_binding_types.insert(cur_tmp.clone(), Type::int());
                    backend.fun.let_original_types.insert(cur_tmp.clone(), Type::int());
                    let arg = Expr::Identifier(cur_tmp);
                    let out_tmp = backend.fun.gen_reg();
                    let step = backend.emit_method_call(out, &out_tmp, list, "Step", &[arg], indent);
                    step.name
                } else {
                    let n = backend.fun.gen_reg();
                    writeln!(out, "{}{} = add i64 {}, 1", indent, n, cur).ok();
                    n
                };
                writeln!(out, "{}store i64 {}, ptr {}", indent, next, slot).ok();
                writeln!(out, "{}br label %{}", indent, header).ok();
            }
            writeln!(out, "{}{}:", indent, end_lbl).ok();
            // 2026-08-17 (P4, plan 2026-08-17-hashmap-storage-tuple-correctness.md):
            // a COUNTDOWN body that ends in a `foreach` (e.g. an inlined
            // collection member's probe loop) must report the foreach's END
            // block so the countdown places its decrement + latch phi there.
            // Without this the latch phi's predecessor set includes the nested
            // loop's internal blocks where the countdown decrement is not
            // defined — llc: "Instruction does not dominate all uses! %cdm337 =
            // sub". Mirrors the Guarded/If arms (1289/1323). Verified: the pure
            // `when`-guard countdown still prints correct sums (the trailing 0
            // from a non-converging `node fin` is pre-existing baseline
            // behavior).
            backend.fun.cur_block = Some(end_lbl.clone());
            backend.fun.terminated = false;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        Statement::Gate(cond) => {
            // 2026-07-26: Convergence gate — if cond is true, continue;
            // otherwise branch to convergence_target (the loop header for retry).
            let cond_reg = backend.emit_expr(out, cond, indent);
            let label_n = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let pass_lbl = format!("gate.pass{}", label_n);
            // 2026-07-30: In a defn (no convergence target), assertions that fail
            // trap via unreachable. In a txn, they branch back to the loop header.
            let has_convergence = backend.fun.convergence_target.is_some();
            let fail_target = if has_convergence {
                backend.fun.convergence_target.as_ref().unwrap().clone()
            } else {
                format!("gate.fail{}", label_n)
            };
            let cond_i1 = if cond_reg.ty == Type::bool_() {
                let b = backend.fun.gen_reg();
                writeln!(out, "{}{} = trunc i8 {} to i1", indent, b, cond_reg.name).ok();
                b
            } else {
                let i1_name = format!("%gi1_{}", label_n);
                writeln!(out, "{}{} = icmp ne i64 {}, 0", indent, i1_name, cond_reg.name).ok();
                i1_name
            };
            writeln!(out, "{0}br i1 {1}, label %{2}, label %{3}",
                indent, cond_i1, pass_lbl, fail_target).ok();
            if !has_convergence {
                // Defn body: assertion failure traps via unreachable
                writeln!(out, "{}{}:", indent, fail_target).ok();
                writeln!(out, "{}  unreachable", indent).ok();
            }
            writeln!(out, "{}{}:", indent, pass_lbl).ok();
            backend.fun.terminated = false;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-08-13 (layout-keywords plan Phase 4): `trap;` — hardware abort
        // (SPEC §8.8). `@llvm.trap` is declared in emit_declares; the
        // `unreachable` terminator marks the rest of the block dead so later
        // statements (which LLVM verifies unreachable) are not emitted.
        Statement::Trap => {
            writeln!(out, "{}call void @llvm.trap()", indent).ok();
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-09-06 (halt slice): `halt;` — the bare-metal stop, trap's
        // deliberate sibling. The wait instruction is TARGET knowledge —
        // `wfi` only assembles on ARM/RISC-V (the same family dispatch the
        // assembler chain uses); an x86-triple build takes the trap abort
        // (loud, portable). The spin loop makes the halt survive spurious
        // wakeups (a bare `wfi` returns on the next interrupt); the
        // entry-branch pattern keeps the entry block predecessor-free.
        // Either way the rest of the block is dead.
        Statement::Halt => {
            let t = &backend.ctx.target_triple;
            let wait_family = ["arm", "thumb", "aarch64", "cortex", "riscv"]
                .iter().any(|fam| t.contains(fam));
            if wait_family {
                let uid = backend.fun.txn_counter;
                backend.fun.txn_counter += 1;
                writeln!(out, "{}br label %halt_wait_{}", indent, uid).ok();
                writeln!(out, "halt_wait_{}:", uid).ok();
                writeln!(out, "  call void asm sideeffect \"wfi\", \"\"()").ok();
                writeln!(out, "  br label %halt_wait_{}", uid).ok();
            } else {
                writeln!(out, "{}call void @llvm.trap()", indent).ok();
            }
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-08-22 (Phase 8): yield; — a cancellation point. No-op in the
        // eager model; the concurrent scheduler replaces it with a suspend.
        Statement::Yield => {
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        // 2026-08-23 (SPEC §10.x): check — runtime assertion. Branches to
        // trap/rollback if the condition is false; falls through otherwise.
        Statement::Check(cond) => {
            let cond_reg = backend.emit_expr(out, cond, indent);
            let cond_i1 = guard_cond_i1(backend, out, indent, &cond_reg);
            // For now: no-op on success. Phase C: branch to a rollback
            // block for unprovable loops.
            let noop = backend.fun.gen_reg();
            writeln!(out, "{}{} = add i64 0, 0", indent, noop).ok();
            TypedRegister { name: noop, ty: Type::void() }
        }
        Statement::Match { expr: scrutinee, arms } => {
            // 2026-08-23 (bugfix, BUGS.md "callable-txn bodies silently drop
            // match"): statement-level match had NO arm here — it fell to the
            // catch-all and vanished, so lib/tamer/vm.bv's exec_op opcode
            // dispatch compiled to an empty convergent loop. Emission:
            // evaluate the scrutinee ONCE; chain per-arm any-of pattern
            // conditions (emit_pattern_condition, same machinery as the
            // expression match); each matched arm binds its patterns, runs
            // its statements, then branches to the merge; no-match falls
            // through the condition chain into the merge.
            // To undo: delete this arm (restores the silent drop).
            let scrut = backend.emit_expr(out, scrutinee, indent);
            let counter = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let merge = format!(".smt_end_{}", counter);
            for (i, arm) in arms.iter().enumerate() {
                let next = format!(".smt_next_{}_{}", counter, i);
                // Any-of semantics: a match arm with several patterns fires
                // when ANY matches. Wildcard/Binding conditions are the
                // constant true — short-circuit the rest of the arm's
                // patterns when one appears.
                let mut cond: Option<String> = None;
                for pat in &arm.patterns {
                    if matches!(pat, crate::ast::Pattern::Wildcard | crate::ast::Pattern::Binding(_)) {
                        cond = Some(format!("{}", backend.fun.gen_reg()));
                        writeln!(out, "  {} = icmp eq i64 0, 0", cond.as_deref().unwrap_or("")).ok();
                        break;
                    }
                    let c = backend.emit_pattern_condition(pat, &scrut.name, &scrut.ty, out, indent);
                    cond = Some(match cond {
                        None => c,
                        Some(prev) => {
                            let r = backend.fun.gen_reg();
                            writeln!(out, "  {} = or i1 {}, {}", r, prev, c).ok();
                            format!("{}", r)
                        }
                    });
                }
                let body_label = format!(".smt_body_{}_{}", counter, i);
                let cond = cond.unwrap_or_else(|| {
                    // Empty-pattern arm: treat as always-matching.
                    let r = backend.fun.gen_reg();
                    writeln!(out, "  {} = icmp eq i64 0, 0", r).ok();
                    r
                });
                writeln!(out, "  br i1 {}, label %{}, label %{}", cond, body_label, next).ok();
                writeln!(out, "{}:", next).ok();
                backend.fun.cur_block = Some(next.clone());
            }
            // No arm matched: fall through to the merge.
            writeln!(out, "  br label %{}", merge).ok();
            for (i, arm) in arms.iter().enumerate() {
                let body_label = format!(".smt_body_{}_{}", counter, i);
                writeln!(out, "{}:", body_label).ok();
                backend.fun.cur_block = Some(body_label.clone());
                // 2026-08-23: an arm ending in `term` sets fun.terminated —
                // reset per arm or every later arm inherits it and emits
                // nothing (same discipline as emit_ssa_main's per-txn reset).
                backend.fun.terminated = false;
                for p in &arm.patterns {
                    backend.bind_pattern(p, &scrut.name, &scrut.ty, out, indent);
                }
                for s in &arm.body {
                    if backend.fun.terminated {
                        break;
                    }
                    emit_statement(backend, out, s, indent);
                }
                if !backend.fun.terminated {
                    writeln!(out, "  br label %{}", merge).ok();
                }
            }
            // 2026-08-23: when EVERY arm terminated (`term`), the merge has
            // no live predecessors — an EMPTY label pair is invalid LLVM
            // ('expected instruction opcode'). The condition chain still
            // branches here, so print the label with an explicit
            // `unreachable` terminator (dead code, removed by opt); a live
            // merge stays a normal open block.
            writeln!(out, "{}:", merge).ok();
            if backend.fun.terminated {
                writeln!(out, "  unreachable").ok();
            } else {
                backend.fun.cur_block = Some(merge);
            }
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
        _ => {
            TypedRegister { name: backend.fun.gen_reg(), ty: Type::void() }
        }
    }
}

/// 2026-07-31 (A5): obj member `self` array-slot store — `data[i] = v` in a
/// member body. GEP self + slot offset + elem-size*idx, then store.
/// Flat guard clauses; returns false when the pattern does not apply.
fn emit_self_slot_array_store(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    obj: &Expr,
    idx: &Expr,
    val: &TypedRegister,
) -> bool {
    let Some((self_type, self_ptr)) = backend.fun.self_binding.clone() else {
        return false;
    };
    let Expr::Identifier(sname) = obj else { return false; };
    let Some((_, s_ty)) = backend
        .ctx
        .struct_types
        .get(&self_type)
        .and_then(|f| f.iter().find(|(n, _)| n == sname))
    else {
        return false;
    };
    let Type::Vector(inner, dims) = s_ty.clone() else { return false; };
    if dims.len() != 1 {
        return false;
    }
    let offset = backend.lookup_field_offset(&self_type, sname);
    let elem_size = crate::backend::llvm::types::type_size(inner.as_ref(), backend.ctx.type_universe.clone().as_ref());
    let elem_llvm = backend.llvm_type(&inner);
    let idx_reg = backend.emit_expr(out, idx, indent);
    let scaled = backend.fun.gen_reg();
    writeln!(out, "{}{} = mul i64 {}, {}", indent, scaled, idx_reg.name, elem_size).ok();
    let total = backend.fun.gen_reg();
    writeln!(out, "{}{} = add i64 {}, {}", indent, total, offset, scaled).ok();
    let gep = backend.fun.gen_reg();
    writeln!(out, "{}{} = getelementptr i8, ptr {}, i64 {}", indent, gep, self_ptr, total).ok();
    let universe = backend.ctx.type_universe.clone();
    let store_val = backend.ensure_typed_value(out, indent, &elem_llvm, &val.name, Some(val.ty.clone()), universe.as_ref());
    writeln!(out, "{}store {} {}, ptr {}", indent, elem_llvm, store_val, gep).ok();
    true
}

/// 2026-07-31 (A4): Array state-field store — `f[i] = v` where `f` is a
/// single-dimension Vector state field. GEP into %State + scalar store.
/// Flat guard clauses; returns false when the pattern does not apply.
pub(super) fn emit_array_state_store(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    obj: &Expr,
    idx: &Expr,
    val: &TypedRegister,
) -> bool {
    let Expr::Identifier(name) = obj else { return false; };
    let Some(&fidx) = backend.ctx.field_index_map.get(name) else { return false; };
    let field_ty = backend.ctx.field_types[fidx].clone();
    if !field_ty.starts_with('[') {
        return false;
    }
    let idx_reg = backend.emit_expr(out, idx, indent);
    let base = backend.emit_state_gep(out, indent, "f", "%state", fidx);
    // 2026-08-10: the index is i{int_bits} (i32 wasm32) — widen to i64 for the
    // GEP (LLVM GEP indices are i64). emit_expr::gep_index is a no-op on x86_64.
    let gep_idx = backend.gep_index(out, indent, &idx_reg);
    let elem = backend.fun.gen_reg();
    writeln!(
        out,
        "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}",
        indent, elem, field_ty, base, gep_idx
    )
    .ok();
    let elem_llvm = field_ty
        .rsplit_once('x')
        .map(|(_, t)| t.trim().trim_end_matches(']').to_string())
        .unwrap_or_else(|| "i64".to_string());
    let universe = backend.ctx.type_universe.clone();
    let store_val = backend.ensure_typed_value(
        out,
        indent,
        &elem_llvm,
        &val.name,
        Some(val.ty.clone()),
        universe.as_ref(),
    );
    writeln!(out, "{}store {} {}, ptr {}", indent, elem_llvm, store_val, elem).ok();
    true
}

/// 2026-07-31 (A6): `<-` dispatch onto an obj MEMBER binding
/// (`op InsertAt: push(#Lh, #Rh)` on an obj). Emits a self-bound member call
/// (receiver + value register) instead of the free-function marker dispatch.
/// Flat guard clauses; returns false when the pattern does not apply.
pub(super) fn emit_strategy_member_call(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    target: &Expr,
    op_def: &crate::ast::top::OperatorDef,
    value: Option<&str>,
) -> Option<String> {
    let fn_name = match op_def.impl_args.as_ref() {
        Some(crate::ast::PropertyValue::Identifier(s)) => s.clone(),
        Some(crate::ast::PropertyValue::List(items)) => match items.first() {
            Some(crate::ast::PropertyValue::Identifier(f)) => f.clone(),
            _ => return None,
        },
        _ => return None,
    };
    let recv = match target {
        Expr::AddrOf(inner) => (**inner).clone(),
        _ => target.clone(),
    };
    let Expr::Identifier(recv_name) = &recv else { return None; };
    // 2026-08-12 (Iterable protocol): a LOCAL collection receiver
    // (`let ys: List<Int> = []; &ys <- 10`) resolves its type from the local
    // binding, not just a state field. 2026-08-18 (Phase C): a pooled
    // member-field receiver (`items`, slot `PiggyBank.items`) resolves through
    // the self prefix; the shared base-name resolver handles both plus the
    // `Vector([Anonymous(1)])` column-type wrapper.
    let type_name = backend.collection_base_type_name(recv_name)?;
    let members = backend.ctx.obj_members.get(&type_name).cloned().unwrap_or_default();
    let member = members.iter().find(|m| member_briev_name(m) == fn_name.as_str()).cloned();
    let Some(member) = member else { return None; };
    // Emit the receiver (the struct address) and pass the value register.
    // 2026-08-18 (Phase D, PiggyBank): a POOLED INSTANCE receiver (`piggy <-
    // 1` on `let piggy: PiggyBank<Int> = 0`) has no top-level slot — emitting
    // the identifier resolves a bogus `@piggy` global. Mirror emit_method_call:
    // resolve the instance prefix and use a dummy receiver register (the member
    // body addresses the pool columns via the prefix), never the identifier.
    let recv_tmp = backend.fun.gen_reg();
    let recv_reg = if let Some((prefix, _row)) = backend.instance_prefix_for(recv_name) {
        writeln!(out, "{}{} = add i64 0, 0", indent, recv_tmp).ok();
        crate::backend::llvm::TypedRegister {
            name: recv_tmp,
            ty: crate::ast::Type::Custom(backend.ctx.obj_instance_inits.get(&prefix)
                .map(|(b, _)| b.clone())
                .unwrap_or_else(|| prefix.clone())),
        }
    } else {
        backend.emit_expr_inner(out, &recv_tmp, &recv, indent)
    };
    let mut arg_regs: Vec<(String, Type)> = Vec::new();
    if let Some(vreg) = value {
        arg_regs.push((vreg.to_string(), Type::int()));
    }
    let out_tmp = backend.fun.gen_reg();
    // 2026-08-01 (A10): resolve the mono key for a generic receiver
    // (`Stack<Int, 256>`) — the generic base layout (`data: T[N]`) computes
    // degenerate self-slot offsets (len at 0). The Init path already resolves
    // the mono key; the member-call path must too, or the push's self-slot
    // GEPs (data[len], len = len + 1) write to the wrong offsets.
    let self_key = backend.resolve_obj_key(&recv_reg.ty).unwrap_or_else(|| type_name.clone());
    // 2026-08-01: the member body's RETURNED register is the result (a defn/
    // txn member's `term` value); emit_member_body ignores `out_tmp` and
    // returns member_result. Side-effect members (push/pop without a term
    // result) return a fresh void register — fine for discards.
    let result_reg = backend.emit_member_body(out, &out_tmp, crate::backend::llvm::emit_expr::MemberInvocation { recv_reg: &recv_reg, type_name: &self_key, member: &member, arg_regs: &arg_regs, prefix: backend.unpacked_instance_prefix(recv_name) }, indent);
    Some(result_reg.name)
}

/// Resolve a strategy property value to a function name and argument markers,
/// compute the handle (pointer to the collection storage), and emit a
/// generic call @fn_name(arg1, arg2, ...) where args are resolved from markers.
/// 2026-07-18: Generic dispatch — no hardcoded function names.
/// 2026-07-20: Handle both generic state-field types (via field_index_map) and
///   let-binding slots. Any type declaring InsertAt/ExtractFrom in operator_defs
///   gets the same <- behavior.
/// Supports: PropertyValue::Identifier("ring_push") for convention-based dispatch,
///   and PropertyValue::List([Identifier("ring_push"), HashL, HashR]) for
///   explicit marker-based dispatch like InsertAt <~ ring_push(#Lh, #Rh).
pub(super) fn emit_strategy_fn_call(backend: &mut LlvmBackend, out: &mut String, indent: &str,
    target: &Expr, op_def: &crate::ast::top::OperatorDef, value: Option<&str>) -> Option<String> {
    let pv = op_def.impl_args.as_ref()?;
    let (fn_name, markers): (&str, &[crate::ast::PropertyValue]) = match pv {
        crate::ast::PropertyValue::Identifier(s) => {
            const EMPTY: &[crate::ast::PropertyValue] = &[];
            (s.as_str(), EMPTY)
        }
        crate::ast::PropertyValue::List(items) => {
            let fn_ident = match items.first()? {
                crate::ast::PropertyValue::Identifier(f) => f.as_str(),
                _ => return None,
            };
            (fn_ident, &items[1..])
        }
        _ => return None,
    };
    let var_name = target.as_var_name()?;

    // 2026-07-20: Compute handle as ptrtoint of the variable's storage location.
    // Derive the handle from the state field or let-binding alloca.
    // 2026-08-14 (ringbuf_inline removed): the inline `_data`-field special case
    // is gone — collection state fields carry the struct handle directly.
    let handle = if let Some(&idx) = backend.ctx.field_index_map.get(var_name) {
        let gep = backend.emit_state_gep(out, indent, "hnd", "%state", idx);
        let h = backend.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, h, gep).ok();
        h
    } else if let Some(slot) = backend.fun.let_bindings.get(var_name).cloned() {
        // Let-binding — use alloca pointer address as handle.
        let h = backend.fun.gen_reg();
        writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, h, slot).ok();
        h
    } else {
        return None; // No storage location found — can't compute handle
    };

    // Resolve markers to argument registers. Convention-based dispatch (no markers)
    // passes (handle, value) for push and (handle) for pop. Marker-based dispatch
    // resolves each marker to the corresponding register.
    let args: Vec<String> = if markers.is_empty() {
        // Convention-based: push = (handle, value), pop = (handle)
        match value {
            Some(val) => vec![handle.clone(), val.to_string()],
            None => vec![handle.clone()],
        }
    } else {
        // Marker-based: resolve #Lh, #Rh, #T to actual registers
        markers.iter().map(|m| match m {
            crate::ast::PropertyValue::HashL => handle.clone(),
            crate::ast::PropertyValue::HashR => value.map(|v| v.to_string()).unwrap_or(handle.clone()),
            crate::ast::PropertyValue::HashT => "1".to_string(), // placeholder — element type
            _ => handle.clone(),
        }).collect()
    };

    // 2026-08-01: the free-function convention boxes every argument to i64
    // (the handle is a ptrtoint'd address; the pushed value is the boxed
    // register). Each arg must carry its LLVM type or the call is malformed
    // (`call i64 @pop(%t39)` — LLVM requires `i64 %t39`).
    let typed_args: Vec<String> = args.iter().map(|a| format!("i64 {}", a)).collect();
    let args_str = typed_args.join(", ");
    let result = backend.fun.gen_reg();
    writeln!(out, "{}{} = call i64 @{}({})", indent, result, fn_name, args_str).ok();
    Some(result)
}

/// 2026-08-01 (Phase 3): store an extracted/popped arrow result into the target
/// — a local binding (alloca) or a state field. Mirrors the Assign-arm pop store.
fn emit_arrow_store(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    target: &crate::ast::Expr,
    result: &str,
) {
    let crate::ast::Expr::Identifier(name) = target else { return };
    if let Some(reg) = backend.fun.let_bindings.get(name) {
        // 2026-08-18 (Phase D, PiggyBank): a local bound to a VALUE register
        // (`let all: List<Int> = []; all ~<- piggy`) must REBIND to the extract
        // result, never `store i64 …, ptr <value-reg>` (a store into a value
        // is invalid IR). Only alloca/param-slot bindings store in place —
        // their address is the persistent slot.
        if is_persistent_slot(backend, reg) {
            writeln!(out, "{}store i64 {}, ptr {}", indent, result, reg).ok();
        } else {
            backend.fun.let_bindings.insert(name.clone(), result.to_string());
        }
        return;
    }
    if backend.ctx.field_index_map.contains_key(name) {
        backend.emit_state_store_i64(out, indent, name, result);
        return;
    }
    if let Some((prefix, row_reg)) = backend.fun.self_prefix.clone() {
        // 2026-08-18 (Phase C): an extract into a pooled member-field target
        // stores through the instance column (`{prefix}.{name}`).
        let slot = format!("{}.{}", prefix, name);
        let val = TypedRegister { name: result.to_string(), ty: crate::ast::Type::int() };
        emit_state_store_self_slot(backend, out, indent, &slot, &row_reg, &val);
    }
}

/// 2026-08-18 (Phase D): a local binding whose register is a REAL slot (a
/// stack alloca or a param slot) — stores through it persist. A plain VALUE
/// register has no address; the arrow rebinds it instead.
fn is_persistent_slot(backend: &LlvmBackend, reg: &str) -> bool {
    backend.fun.let_binding_allocas.contains(reg)
        || backend.fun.param_slots.values().any(|s| s == reg)
}

/// 2026-08-18 (Phase C, BUGS.md member-field arrow): store a value into a
/// POOLED member-field slot (`{prefix}.{name}`). Mirrors the Assign arm's
/// pooled-column store: GEP into the column element — the column type is an
/// aggregate (`[N x i64]`), never stored as a whole. Heap-backed columns load
/// the buffer address from the slot first. The value is stored at the
/// element's native type, boxed via adapt_to_i64 when it doesn't match.
fn emit_state_store_self_slot(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    slot: &str,
    row_reg: &str,
    val: &TypedRegister,
) {
    let Some(&idx) = backend.ctx.field_index_map.get(slot) else { return; };
    let (gep, elem_ty) = if let Some(elem_ty) = backend.ctx.heap_columns.get(&idx).cloned() {
        let base = backend.emit_state_gep(out, indent, "m", "%state", idx);
        let addr = backend.fun.gen_reg();
        writeln!(out, "{}{} = load i64, ptr {}", indent, addr, base).ok();
        let buf = backend.fun.gen_reg();
        writeln!(out, "{}{} = inttoptr i64 {} to ptr", indent, buf, addr).ok();
        let row = backend.fun.gen_reg();
        writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 {}", indent, row, elem_ty, buf, row_reg).ok();
        (row, elem_ty)
    } else {
        let base = backend.emit_state_gep(out, indent, "m", "%state", idx);
        let gep = backend.fun.gen_reg();
        let col_ty = backend.ctx.field_types[idx].clone();
        writeln!(out, "{}{} = getelementptr {}, ptr {}, i64 0, i64 {}", indent, gep, col_ty, base, row_reg).ok();
        (gep, "i64".to_string())
    };
    if backend.llvm_type(&val.ty) == elem_ty {
        writeln!(out, "{}store {} {}, ptr {}", indent, elem_ty, val.name, gep).ok();
    } else {
        let boxed = backend.adapt_to_i64(out, indent, val);
        writeln!(out, "{}store i64 {}, ptr {}", indent, boxed, gep).ok();
    }
}

/// 2026-08-01 (Phase 3): the plain-copy arrow (`dest <- src` when neither side
/// has a collection op binding) — store the value into the local/field.
fn emit_arrow_store_local(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    name: &str,
    val: &TypedRegister,
) {
    if let Some(reg) = backend.fun.let_bindings.get(name) {
        // 2026-08-18 (Phase D): same rebind rule as emit_arrow_store — a
        // value-register local rebinds (SSA); an alloca/param slot stores.
        if is_persistent_slot(backend, reg) {
            let store_ty = backend.llvm_type(&val.ty);
            writeln!(out, "{}store {} {}, ptr {}", indent, store_ty, val.name, reg).ok();
        } else {
            backend.fun.let_bindings.insert(name.to_string(), val.name.clone());
        }
        return;
    }
    if backend.ctx.field_index_map.contains_key(name) {
        let fidx = *backend.ctx.field_index_map.get(name).unwrap();
        let fty = backend.ctx.field_types.get(fidx).cloned().unwrap_or_else(|| "i64".to_string());
        let store_val = backend.ensure_typed_value(
            out, indent, &fty, &val.name, Some(val.ty.clone()),
            backend.ctx.type_universe.clone().as_ref(),
        );
        backend.emit_state_store_i64(out, indent, name, &store_val);
        return;
    }
    if let Some((prefix, row_reg)) = backend.fun.self_prefix.clone() {
        // 2026-08-18 (Phase C): a bare member name in a pooled member body
        // writes through the instance column (`{prefix}.{name}`), mirroring
        // the strategy-lookup resolution in lookup_strategy_type_name.
        let slot = format!("{}.{}", prefix, name);
        emit_state_store_self_slot(backend, out, indent, &slot, &row_reg, val);
    }
}

/// 2026-08-01 (Phase 3): destroy a consumed operand's backing storage. The
/// value is emitted once (its register is the collection's handle) and freed
/// via the allocation-strategy-aware `emit_destroy_register`. A value whose
/// identifier is not resolvable is a no-op — no backing to free anyway.
fn emit_destroy_consumed(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    value: &crate::ast::Expr,
) {
    if let crate::ast::Expr::Identifier(name) = value {
        let resolvable = backend.fun.let_bindings.contains_key(name)
            || backend.ctx.field_index_map.contains_key(name)
            || backend.fun.param_slots.values().any(|s| s == name);
        if !resolvable {
            return;
        }
    }
    let reg = backend.emit_expr(out, value, indent);
    emit_destroy_register(backend, out, indent, &reg.name);
}

/// 2026-08-26 (async Phase D): does this declared type name the event-wire
/// surface (`Event<T>`)? The typechecker's own resolve_field_type matches
/// the `Event` base the same way (SPEC §9.5) — a wire is an i64 slot-id
/// handle at the LLVM level, so this is ABI classification, not layout
/// lookup.
pub(crate) fn is_event_type(ty: &Type) -> bool {
    matches!(ty, Type::Applied(base, _) if base == "Event")
}
