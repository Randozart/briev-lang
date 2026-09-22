// ── Expression Evaluation ──────────────────────────────────────────────
// 2026-07-12: Phase 3.1 — Flat dispatch, one arm per Expr variant.
// Call with # suffix dispatches to execute_intrinsic().
// Each complex arm is extracted into a named helper for flat code.

use crate::ast::*;
use crate::errors::RuntimeError;
use crate::interpreter::{
    Atom, bool_to_bits, execute_intrinsic, f64_to_bits, i64_to_bits, zero_bits, Value, VirtualHeap,
};
use std::collections::HashMap;
use std::sync::Arc;

/// 2026-08-06 (fix): the interpreter's scoped bindings + the function registry,
/// bundled so eval helpers stay under the Praetor 6-param gate now that
/// user-function dispatch (root-cause fix) threads the registry through.
pub struct EvalScope<'a> {
    pub bindings: &'a mut HashMap<String, Value>,
    pub functions: &'a HashMap<String, crate::interpreter::FunctionDef>,
}

/// Evaluate an expression to a Value.
/// Flat dispatch: one match arm per Expr variant.
pub fn eval_expr(
    expr: &Expr,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,

) -> Result<Value, RuntimeError> {
    match expr {
        // ── Literals ────────────────────────────────────────────
        Expr::Decimal(n) => Ok(i64_to_bits(*n)),
        Expr::TaggedLiteral(n, _) => Ok(i64_to_bits(*n)),
        Expr::Float(f) => Ok(f64_to_bits(*f)),
        // 2026-08-01 (audit): first-class Bool/Char values — the codegen
        // dispatches Print# by protocol category, so the interpreter must carry
        // char/bool-ness to print characters / true-false. as_i64/as_f64
        // promote both (C-style), so arithmetic/comparisons are unchanged.
        Expr::Bool(b) => Ok(Value::Atom(Atom::Bool(*b))),
        Expr::BeginProgram => Ok(Value::Atom(Atom::Bool(true))),
        Expr::Char(c) => Ok(Value::Atom(Atom::Char(*c))),
        Expr::Quoted(bytes) | Expr::TaggedQuotedLiteral(bytes, _) => Ok(Value::bits(bytes.clone())),

        // ── References ──────────────────────────────────────────
        Expr::Identifier(name) => bindings
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::UndefinedVariable { name: name.clone() }),

        // ── Calls ───────────────────────────────────────────────
        Expr::Call(name, args, _) => eval_call(name, args, heap, bindings, functions),

        // ── Binary operators ─────────────────────────────────────
        Expr::BinaryOp(kind, lhs, rhs) => eval_binary_op(kind, lhs, rhs, heap, &mut EvalScope { bindings: &mut *bindings, functions: functions }),

        // ── Unary operators ──────────────────────────────────────
        Expr::UnaryOp(kind, expr) => eval_unary_op(kind, expr, heap, bindings, functions),

        // ── Block ────────────────────────────────────────────────
        Expr::Block(stmts) => eval_block(stmts, heap, bindings, functions),

        // ── If ───────────────────────────────────────────────────
        Expr::If(cond, then, else_) => eval_if(cond, then, else_, heap, &mut EvalScope { bindings: &mut *bindings, functions: functions }),

        // ── Tuple ────────────────────────────────────────────────
        Expr::Tuple(exprs) => {
            let values: Result<Vec<Value>, _> =
                exprs.iter().map(|e| eval_expr(e, heap, bindings, functions)).collect();
            Ok(Value::product(values?))
        }

        // ── List ─────────────────────────────────────────────────
        Expr::List(exprs) => {
            let values: Result<Vec<Value>, _> =
                exprs.iter().map(|e| eval_expr(e, heap, bindings, functions)).collect();
            Ok(Value::product(values?))
        }

        // ── Field access ─────────────────────────────────────────
        Expr::Field(obj, name) => eval_field(obj, name, heap, bindings, functions),

        // ── Index ────────────────────────────────────────────────
        Expr::Index(obj, index) => eval_index(obj, index, heap, bindings, functions),

        // ── Cast ─────────────────────────────────────────────────
        // 2026-08-01 (audit): the codegen emits a real conversion when the
        // protocol category changes (Bool→Int yields 1/0, Char→Int the code
        // point, Int→Char builds a character), so the interpreter must convert
        // the VALUE, not just reinterpret bits — otherwise `Print#((b as Int))`
        // would print "true" here but "1" in the backend. The target category
        // is resolved through the casting graph (Cast. universe properties,
        // the same source as codegen) — never by type name. Custom types
        // resolve to "Bit" (identity reinterpretation, matching codegen's
        // fallback).
        Expr::Cast(expr, ty) => {
            let v = eval_expr(expr, heap, bindings, functions)?;
            // 2026-08-13 (pack): `x as Bits<N>` is a width assertion — the
            // category fallback below was identity for sub-byte widths
            // (`16 as Bits<4>` held 16). Truncate an integer source to exactly
            // N bits, mirroring the backend's cast path (trunc i{N}).
            if let Some(bits) = crate::type_universe::bits_width(ty) {
                return Ok(eval_bits_cast(v, bits));
            }
            match target_protocol_category(ty).as_str() {
                "Int" => match v {
                    Value::Atom(Atom::Bool(b)) => Ok(Value::Atom(Atom::Int(if b { 1 } else { 0 }))),
                    Value::Atom(Atom::Char(c)) => Ok(Value::Atom(Atom::Int(c as i64))),
                    _ => Ok(v),
                },
                "Float" => match v {
                    Value::Atom(Atom::Bool(b)) => Ok(Value::Atom(Atom::Float(if b { 1.0 } else { 0.0 }))),
                    Value::Atom(Atom::Char(c)) => Ok(Value::Atom(Atom::Float(c as i64 as f64))),
                    _ => Ok(v),
                },
                "Char" => match v {
                    Value::Atom(Atom::Int(n)) => Ok(Value::Atom(Atom::Char(char::from_u32(n as u32).unwrap_or('?')))),
                    _ => Ok(v),
                },
                _ => Ok(v),
            }
        }

        // ── IsType ───────────────────────────────────────────────
        Expr::IsType(expr, ty) => {
            let val = eval_expr(expr, heap, bindings, functions)?;
            crate::interpreter::casts::eval_is_type(&val, ty)
        }

        // ── Within ───────────────────────────────────────────────
        Expr::Within(expr, _scope) => eval_expr(expr, heap, bindings, functions),

        // ── Match ────────────────────────────────────────────────
        Expr::Match(scrutinee, arms) => eval_match(scrutinee, arms, heap, bindings, functions),

        // ── Lambda ───────────────────────────────────────────────
        // 2026-08-06 (Slice E): capture the current bindings as the closure
        // environment. Application binds params into a fresh env seeded from
        // this snapshot (see eval_call), so captures are by-value and
        // re-entrant.
        Expr::Lambda(params, body) => Ok(Value::Closure {
            params: Arc::new(params.clone()),
            body: Arc::new((**body).clone()),
            env: Arc::new(bindings.clone()),
        }),

        // ── Derivation block ─────────────────────────────────────
        // 2026-08-06 (Slice D): a derivation block is a meta-declaration, not
        // a runtime value; codegen emits `add 0, 0` (void) for it, so the
        // interpreter returns Void. Derivation example verification runs
        // through derive::verify_derivation_assertions, not this path.
        Expr::DerivationBlock(_) => Ok(Value::Void),

        // ── Struct literal ───────────────────────────────────────
        Expr::StructLiteral { type_name, fields } => {
            let _ = type_name;
            let mut names = Vec::with_capacity(fields.len());
            let values: Result<Vec<Value>, _> = fields
                .iter()
                .map(|(name, expr)| {
                    names.push(name.clone());
                    eval_expr(expr, heap, bindings, functions)
                })
                .collect();
            Ok(Value::named_product(values?, names))
        }

        // ── Dereference ──────────────────────────────────────────
        // 2026-07-18: Evaluate inner, expect Value::Ref(wrapped), return *wrapped.
        Expr::Deref(inner) => {
            let val = eval_expr(inner, heap, bindings, functions)?;
            match val {
                Value::Ref(wrapped) => Ok((*wrapped).clone()),
                other => Ok(other),
            }
        }
        // ── Address-of ───────────────────────────────────────────
        // 2026-07-18: Wrap the inner value in Value::Ref to represent a pointer.
        Expr::Consume(inner) => eval_expr(inner, heap, bindings, functions),
        // 2026-08-09 (Phase 10): `await task` — the deterministic reference
        // scheduler runs a spawned task inline, so its handle already holds
        // the result; await reads it (the handle's consumption is enforced by
        // the ownership analysis).
        // 2026-08-23 (async Phase A2): await consumes a task handle. In the
        // eager model this was pass-through; now it triggers lazy execution
        // of the pending task's body via the task table.
        // 2026-08-23 (async Phase A3): await triggers round-robin scheduling.
        // 2026-08-26 (async Phase B): segments run through the shared
        // `run_task_segment` executor (CURRENT_TASK set/clear, TaskBlocked
        // translation). A blocked task drops out of the runnable pool until
        // a port fire re-marks it Ready; results are stored ONLY on
        // completion (the A3 code stored transiently every pass).
        Expr::Await(inner) => {
            let inner_val = eval_expr(inner, heap, bindings, functions)?;
            if let Value::Atom(Atom::Int(raw_id)) = &inner_val {
                if *raw_id >= 0 {
                    let target_id = *raw_id as u64;
                    if !crate::interpreter::task_is_done(target_id) {
                        'scheduler: loop {
                            if crate::interpreter::task_is_done(target_id) { break; }
                            let runnable = crate::interpreter::collect_runnable();
                            // Empty pool with the target unfinished = deadlock
                            // posture (every remaining task Waiting): return
                            // the handle (SPEC §12.2 — cooperative scheduler,
                            // no preemption, no hang).
                            if runnable.is_empty() { break; }
                            for (tid, fn_name, arg_vals, segments, current) in &runnable {
                                if crate::interpreter::task_is_done(*tid) { continue; }
                                if *current >= segments.len() { continue; }
                                let job = SegmentJob {
                                    tid: *tid,
                                    fn_name,
                                    args: arg_vals,
                                    segments,
                                    current: *current,
                                };
                                match run_task_segment(&job, heap, functions)? {
                                    SegmentOutcome::Blocked => {}
                                    SegmentOutcome::Term(v) => {
                                        crate::interpreter::mark_done_with_result(*tid, v);
                                        if *tid == target_id { break 'scheduler; }
                                    }
                                    SegmentOutcome::Completed(v) => {
                                        let finished =
                                            crate::interpreter::advance_segment_status(*tid);
                                        if finished {
                                            crate::interpreter::mark_done_with_result(*tid, v);
                                            if *tid == target_id { break 'scheduler; }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(result) = crate::interpreter::get_task_result(target_id) {
                        return Ok(result);
                    }
                }
            }
            Ok(inner_val)
        }
        Expr::AddrOf(inner) => {
            let val = eval_expr(inner, heap, bindings, functions)?;
            Ok(Value::Ref(Box::new(val)))
        }

        // ── Field / reflection / method ─────────────────────────
        Expr::Reflect(recv, name, kind) => eval_reflect(recv, name, *kind, heap, &mut EvalScope { bindings: &mut *bindings, functions: functions }),
        Expr::MethodCall(recv, name, args, _, refs) => {
            if refs.is_empty() {
                eval_method_call(recv, name, args, heap, &mut EvalScope { bindings: &mut *bindings, functions: functions })
            } else {
                eval_method_call_chained(recv, name, args, refs, heap, &mut EvalScope { bindings: &mut *bindings, functions: functions })
            }
        }

        // ── Formatting annotation ────────────────────────────────
        Expr::FormattingAnnotation(_) => Ok(Value::Void),

        // 2026-07-19: Plugin-intercept calls must be resolved by Front plugins
        // before evaluation. The compiler's build path rewrites the lowercase
        // macros (`print!`, `println!`, `get_env!`, `get_env_int!`) at the
        // Parsed stage; this native path keeps direct interpreter use correct
        // (rule #4: the interpreter is the reference) and reports a rename
        // hint for the deprecated PascalCase names.
        Expr::PluginIntercept { name, args, receiver, .. } => eval_intercept(
            name,
            receiver.as_deref(),
            args,
            heap,
            bindings,
            functions,
        ),
        Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
        Expr::Slice { array, start, end, stride } => eval_slice(
            array,
            SliceBounds {
                start: start.as_deref(),
                end: end.as_deref(),
                stride: stride.as_deref(),
            },
            heap,
            bindings,
            functions,
        ),
        // 2026-08-07 (Phase 7): an iterable range value — consumed by
        // `foreach` (SPEC §11.4).
            Expr::Spawn { type_name, args, storage } => {
                // 2026-08-07 (instance pools): the interpreter reference evaluates
                // the spawn args; the handle is a synthetic atom (the codegen
                // allocates the real pool row).
                // 2026-08-09 (Phase 10): `spawn defn(args)` is a TASK spawn — the
                // deterministic reference scheduler runs the task inline, so the
                // handle IS the callable's result (SPEC §12.2). Distinguish a task
                // (a registered function) from an obj base.
                if functions.contains_key(type_name) {
                    return eval_task_spawn(type_name, args, heap, bindings, functions);
                }
            // 2026-08-22 (Phase 7b, SPEC §9.5): a PORTED obj spawn builds a
            // real instance — input ports bind positionally to the evaluated
            // arguments (an EventQ argument SHARES the producer's slot; plain
            // values wrap as an already-ready event), output ports get fresh
            // slots, plain fields default.
            let shape = crate::interpreter::obj_shapes().and_then(|o| o.get(type_name).cloned());
            if let Some(shape) = shape {
                if args.len() != shape.ports_in.len() {
                    return Err(RuntimeError::TypeError {
                        expected: format!(
                            "{} constructor takes {} port argument(s)",
                            type_name,
                            shape.ports_in.len()
                        ),
                        found: format!("{} argument(s)", args.len()),
                    });
                }
                let mut fields: HashMap<String, Value> = HashMap::new();
                for ((pname, _pty), a) in shape.ports_in.iter().zip(args.iter()) {
                    let v = eval_expr(a, heap, bindings, functions)?;
                    let wrapped = match v {
                        Value::EventQ(_) => v,
                        other => Value::EventQ(std::rc::Rc::new(std::cell::RefCell::new(
                            crate::interpreter::EventSlot {
                                ready: true,
                                payload: Some(other),
                                waiters: Vec::new(),
                            },
                        ))),
                    };
                    fields.insert(pname.clone(), wrapped);
                }
                for (oname, _) in &shape.ports_out {
                    fields.insert(
                        oname.clone(),
                        Value::EventQ(std::rc::Rc::new(std::cell::RefCell::new(
                            crate::interpreter::EventSlot {
                                ready: false,
                                payload: None,
                                waiters: Vec::new(),
                            },
                        ))),
                    );
                }
                for (fname, fty) in &shape.fields {
                    fields.insert(fname.clone(), default_for_type(fty));
                }
                return Ok(Value::Instance {
                    type_name: type_name.clone(),
                    fields: std::rc::Rc::new(std::cell::RefCell::new(fields)),
                });
            }
            for a in args {
                eval_expr(a, heap, bindings, functions)?;
            }
            Ok(Value::Atom(Atom::Int(0)))
        }
        Expr::Range { start, end, inclusive } => {
                let s = eval_expr(start, heap, bindings, functions)?
                    .as_i64()
                    .ok_or_else(|| RuntimeError::TypeError {
                        expected: "an integer range bound".into(),
                        found: "non-integer range bound".into(),
                    })?;
                let e = eval_expr(end, heap, bindings, functions)?
                    .as_i64()
                    .ok_or_else(|| RuntimeError::TypeError {
                        expected: "an integer range bound".into(),
                        found: "non-integer range bound".into(),
                    })?;
                Ok(Value::Range { start: s, end: e, inclusive: *inclusive })
            }
            Expr::Named { inner, .. } => eval_expr(inner, heap, bindings, functions),
            Expr::UnitLiteral { value, .. } => Ok(f64_to_bits(*value)),
            Expr::Capture { expr, name } => {
                let val = eval_expr(expr, heap, bindings, functions)?;
                bindings.insert(name.clone(), val.clone());
                Ok(val)
            }

    }
}

/// 2026-08-09 (Phase 10): `spawn defn(args)` — a TASK spawn evaluated inline.
/// The deterministic reference scheduler runs the task to completion, so the
/// returned handle IS the callable's result (SPEC §12.2).
fn eval_task_spawn(
    type_name: &str,
    args: &[Expr],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    // 2026-08-23 (async scheduler Phase A2, SPEC §12.2): LAZY execution —
    // spawn captures the function + evaluated args but does NOT run the body.
    // The first `await` triggers execution. This is the foundation for
    // cooperative scheduling: a spawned task exists as a Ready entry in the
    // task table; `free` before await cancels it (body never runs).
    let mut arg_vals = Vec::new();
    for a in args {
        arg_vals.push(eval_expr(a, heap, bindings, functions)?);
    }
    let defn = functions
        .get(type_name)
        .cloned()
        .ok_or_else(|| RuntimeError::UndefinedFunction(type_name.to_string()))?;
    let task_id = next_task_id();
    // 2026-08-26 (Phase B): parameter names feed the port-read pre-split —
    // a blocking read must head its segment (plan §4).
    crate::interpreter::register_pending_task(
        task_id, type_name.to_string(), arg_vals, defn.body.clone(), &defn.parameters,
    );
    // Return a task-id marker — await triggers lazy segment-by-segment execution.
    Ok(Value::Atom(Atom::Int(task_id as i64)))
}

/// 2026-08-26 (async Phase B): outcome of running ONE segment of a task.
/// - `Completed(v)`: the segment ran off its end — advance the cursor.
/// - `Term(v)`: `term expr;` fired mid-segment — the task is DONE with `v`.
/// - `Blocked`: an unready port read suspended the task (already registered
///   as a slot waiter, status `Waiting`). The cursor does NOT advance; the
///   post-wake re-run starts from this segment's first statement.
enum SegmentOutcome {
    Completed(Value),
    Term(Value),
    Blocked,
}

/// One scheduling unit: a task's next segment plus its identity/captured
/// args, as produced by `collect_runnable`.
struct SegmentJob<'a> {
    tid: u64,
    fn_name: &'a str,
    args: &'a [Value],
    segments: &'a [Vec<Statement>],
    current: usize,
}

/// 2026-08-26 (async Phase B): shared single-segment executor — the ONE
/// place that sets/clears CURRENT_TASK and translates TaskBlocked. Used by
/// the await round-robin. Replaces the duplicated A2-era
/// `execute_pending_task` (dead since the round-robin landed).
fn run_task_segment(
    job: &SegmentJob,
    heap: &mut VirtualHeap,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<SegmentOutcome, RuntimeError> {
    let Some(defn) = functions.get(job.fn_name) else {
        return Ok(SegmentOutcome::Term(Value::Void));
    };
    let mut cb: HashMap<String, Value> = HashMap::new();
    for (i, pname) in defn.parameters.iter().enumerate() {
        if let Some(v) = job.args.get(i) {
            cb.insert(pname.clone(), v.clone());
        }
    }
    crate::interpreter::set_current_task(Some(job.tid));
    let mut out = Ok(SegmentOutcome::Completed(Value::Void));
    for stmt in &job.segments[job.current] {
        match eval_statement(stmt, heap, &mut cb, functions) {
            Ok(v) => out = Ok(SegmentOutcome::Completed(v)),
            Err(RuntimeError::TermReturn(v)) => {
                out = Ok(SegmentOutcome::Term(v));
                break;
            }
            Err(RuntimeError::TaskBlocked) => {
                out = Ok(SegmentOutcome::Blocked);
                break;
            }
            Err(e) => {
                out = Err(e);
                break;
            }
        }
    }
    crate::interpreter::set_current_task(None);
    out
}

// 2026-08-23 (async scheduler Phase A1): thread-local task bookkeeping,
// accessible from deep inside expression evaluation where no &mut Interpreter
// is in scope. Same pattern as OBJ_SHAPES / VARIANT_DEFS.
// 2026-08-26 (Phase B): the dead A1-era TASK_TABLE mirror here is removed —
// the authoritative table lives in interpreter/mod.rs; duplicates silently
// diverge.
use std::cell::Cell;
thread_local! {
    static TASK_ID_COUNTER: Cell<u64> = const { Cell::new(0) };
}

fn next_task_id() -> u64 {
    TASK_ID_COUNTER.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// Evaluate a function/intrinsic call.
fn eval_call(
    name: &str,
    args: &[Expr],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let evaluated: Vec<Value> = args
        .iter()
        .map(|a| eval_expr(a, heap, bindings, functions))
        .collect::<Result<Vec<_>, _>>()?;

    if name.ends_with('#') {
        execute_intrinsic(name, &evaluated, heap)
    } else {
        // 2026-08-06 (Slice E): a closure binding applies — params bind into a
        // fresh env seeded from the captured snapshot, body evaluates in it.
        // A non-closure binding is returned as-is (the simplified Call model);
        // an absent name is an error.
        match bindings.get(name) {
            Some(Value::Closure { params, body, env }) => {
                if params.len() != evaluated.len() {
                    return Err(RuntimeError::TypeError {
                        expected: format!("{} arguments", params.len()),
                        found: format!("{} arguments", evaluated.len()),
                    });
                }
                let mut local: HashMap<String, Value> = (**env).clone();
                for (p, v) in params.iter().zip(evaluated.into_iter()) {
                    local.insert(p.clone(), v);
                }
                eval_expr(body, heap, &mut local, functions)
            }
            Some(v) => Ok(v.clone()),
            // 2026-08-06 (fix): a user-defined function (defn/txn) applies with
            // DYNAMIC scoping — its body reads the CALLER's state, so the local
            // env is seeded from the caller's bindings (not a captured
            // snapshot). A `term <value>` inside the body is the return.
            None => {
                // 2026-08-23 (enum construction): a call naming a declared
                // ENUM VARIANT constructs a Sum — after closures/functions,
                // so user fns shadow variants.
                let is_variant = crate::interpreter::variant_defs()
                    .map(|v| v.contains_key(name))
                    .unwrap_or(false);
                if is_variant {
                    // Qualified calls store the BARE variant — the canonical
                    // tag inside Sum values (patterns normalize likewise).
                    let bare = name.rsplit("::").next().unwrap_or(name).to_string();
                    return Ok(Value::sum(bare, evaluated));
                }
                match functions.get(name) {
                Some(fn_def) => {
                    if fn_def.parameters.len() != evaluated.len() {
                        return Err(RuntimeError::TypeError {
                            expected: format!("{} arguments", fn_def.parameters.len()),
                            found: format!("{} arguments", evaluated.len()),
                        });
                    }
                    let mut local: HashMap<String, Value> = bindings.clone();
                    for (p, v) in fn_def.parameters.iter().zip(evaluated.into_iter()) {
                        local.insert(p.clone(), v);
                    }
                    let block = Expr::Block(fn_def.body.clone());
                    match eval_expr(&block, heap, &mut local, functions) {
                        Err(RuntimeError::TermReturn(v)) => Ok(v),
                        other => other,
                    }
                }
                None => Err(RuntimeError::UndefinedFunction(name.into())),
                }
            }
        }
    }
}

/// 2026-08-06 (Slice B): Index a value. A `Product` is indexed by element
/// position; raw `Bits` by byte offset (String/Data content). Out-of-bounds
/// or non-indexable values are errors — no placeholder, no silent `zero_bits`.
fn eval_index(
    obj: &Expr,
    index: &Expr,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let obj_val = eval_expr(obj, heap, bindings, functions)?;
    let idx_val = eval_expr(index, heap, bindings, functions)?;
    // 2026-08-07 (Phase 7): a Bool-vector index is a MASK — `data[mask]`
    // selects the bytes at the true positions (SPEC §16.5). Otherwise the
    // scalar integer-index path below applies.
    if let Some(mask) = bool_mask_from_value(&idx_val) {
        return eval_masked_index(obj_val, &mask);
    }
    let idx = idx_val
        .as_i64()
        .ok_or_else(|| RuntimeError::TypeError {
            expected: "an integer index".into(),
            found: "non-integer index".into(),
        })?;
    if idx < 0 {
        return Err(RuntimeError::TypeError {
            expected: "a non-negative index".into(),
            found: format!("negative index {}", idx),
        });
    }
    let idx = idx as usize;
    match obj_val {
        Value::Product { fields, .. } => fields
            .get(idx)
            .cloned()
            .ok_or_else(|| index_oob(idx, fields.len())),
        Value::Bits(bytes) => bytes.get(idx).copied().map(|b| Value::bits(vec![b])).ok_or_else(|| index_oob(idx, bytes.len())),
        other => Err(RuntimeError::TypeError {
            expected: "an indexable value (list, tuple, or bits)".into(),
            found: format!("{}", describe_value(&other)),
        }),
    }
}

/// 2026-08-07 (Phase 7): a Bool-vector value (a product of Bool atoms) used
/// as a mask index. Returns the mask bits, or None if the value is not a
/// Bool vector.
fn bool_mask_from_value(v: &Value) -> Option<Vec<bool>> {
    let Value::Product { fields, .. } = v else {
        return None;
    };
    let mut bits = Vec::with_capacity(fields.len());
    for f in fields {
        match f {
            Value::Atom(Atom::Bool(b)) => bits.push(*b),
            _ => return None,
        }
    }
    Some(bits)
}

/// 2026-08-07 (Phase 7): masked select — the elements at the true mask
/// positions, in ascending order. Over a `Product` (an Int/Bool vector) the
/// selected FIELDS form a new product; over byte data the selected bytes form
/// a new Bits value. A mask longer than the source truncates (the mask
/// governs), matching the runtime helpers.
fn eval_masked_index(obj_val: Value, mask: &[bool]) -> Result<Value, RuntimeError> {
    match obj_val {
        Value::Bits(bytes) => {
            let selected: Vec<u8> = bytes
                .iter()
                .enumerate()
                .filter(|(i, _)| mask.get(*i).copied().unwrap_or(false))
                .map(|(_, b)| *b)
                .collect();
            Ok(Value::bits(selected))
        }
        Value::Product { fields, names } => {
            let selected: Vec<Value> = fields
                .iter()
                .enumerate()
                .filter(|(i, _)| mask.get(*i).copied().unwrap_or(false))
                .map(|(_, f)| f.clone())
                .collect();
            let selected_names = names.map(|ns| {
                Arc::new(
                    ns.iter()
                        .enumerate()
                        .filter(|(i, _)| mask.get(*i).copied().unwrap_or(false))
                        .map(|(_, n)| n.clone())
                        .collect(),
                )
            });
            Ok(Value::Product { fields: selected, names: selected_names })
        }
        other => Err(RuntimeError::TypeError {
            expected: "byte data (Data) or a vector (list of elements)".into(),
            found: format!("{}", describe_value(&other)),
        }),
    }
}

/// A user-facing out-of-bounds index error (house style: what's wrong + fix).
fn index_oob(index: usize, len: usize) -> RuntimeError {
    RuntimeError::TypeError {
        expected: format!("an index in 0..{}", len),
        found: format!("index {} (length {})", index, len),
    }
}

/// Short value description for error messages (not Debug — stay terse).
/// 2026-08-22 (Phase 3): does `val` carry the dynamic shape of type `ty`?
/// See the TypedBinding arm of `pattern_match` for the boundary rationale.
fn value_has_type_shape(ty: &crate::ast::Type, val: &Value) -> bool {
    match ty {
        crate::ast::Type::Custom(name) => match name.as_str() {
            "Int" | "UInt" => matches!(val, Value::Atom(Atom::Int(_))),
            "Float" | "Float64" => matches!(val, Value::Atom(Atom::Float(_))),
            "Bool" => matches!(val, Value::Atom(Atom::Bool(_))),
            "Char" => matches!(val, Value::Atom(Atom::Char(_))),
            // String values live as Bits in the interpreter.
            "String" => matches!(val, Value::Bits(_)),
            _ => true,
        },
        _ => true,
    }
}


/// 2026-08-22 (Phase 7b): zero-value defaults for obj slot fields.
fn default_for_type(ty: &crate::ast::Type) -> Value {
    use crate::ast::Type;
    match ty {
        Type::Custom(n) => match n.as_str() {
            "Bool" => Value::Atom(Atom::Bool(false)),
            "Float" | "Float32" => Value::Atom(Atom::Float(0.0)),
            _ => Value::Atom(Atom::Int(0)),
        },
        Type::Bits(_) => Value::Bits(vec![0; ((ty.bit_width() as usize) + 7) / 8]),
        _ => Value::Atom(Atom::Int(0)),
    }
}

fn describe_value(v: &Value) -> String {
    match v {
        Value::Atom(Atom::Int(n)) => format!("integer {}", n),
        Value::Atom(Atom::Float(f)) => format!("float {}", f),
        Value::Atom(Atom::Bool(b)) => format!("boolean {}", b),
        Value::Atom(Atom::Char(c)) => format!("character '{}'", c),
        Value::Bits(_) => "raw bits".into(),
        Value::Product { .. } => "product".into(),
        Value::Dyn { trait_name, .. } => format!("dyn {} value", trait_name),
        // 2026-08-22 (Phase 7b): ports and instances describe by identity.
        Value::EventQ(_) => "event port".into(),
        Value::Instance { type_name, .. } => format!("{} instance", type_name),
        Value::Void => "void".into(),
        Value::Ref(_) => "reference".into(),
        Value::Closure { .. } => "closure".into(),
        Value::Sum { .. } => "sum".into(),
        Value::Range { .. } => "range".into(),
    }
}

/// 2026-08-06 (Slice D): field access on a struct value. The receiver must be
/// a named product (built by a struct literal); the field index is resolved
/// from the value's declared field-name map — no type registry involved.
fn eval_field(
    obj: &Expr,
    name: &str,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let recv = eval_expr(obj, heap, bindings, functions)?;
    match recv {
        Value::Product { fields, names } => match names.as_ref().and_then(|ns| ns.iter().position(|n| n == name)) {
            Some(i) => fields.get(i).cloned().ok_or_else(|| field_oob(name, fields.len())),
            None => Err(RuntimeError::TypeError {
                expected: format!("a field named '{}'", name),
                found: describe_value(&Value::Product { fields, names }),
            }),
        },
        // 2026-08-22 (Phase 7b, SPEC §9.5): instance fields — ports yield
        // their SHARED EventQ handle (the wire), slots their current value.
        Value::Instance { fields, .. } => {
            let f = fields.borrow();
            f.get(name).cloned().ok_or_else(|| RuntimeError::TypeError {
                expected: format!("a declared port or slot named '{}'", name),
                found: format!(
                    "instance with fields {:?}",
                    f.keys().collect::<Vec<_>>()
                ),
            })
        }
        // An EVENT PORT: any name projects the current payload's members.
        // Readiness uses .^Ready (reflection), not .Ready (field).
        Value::EventQ(q) => {
            // 2026-08-26 (async Phase B): decide suspension BEFORE taking the
            // match borrow — block_current_task_on_slot mutates the waiters
            // list and a held borrow would panic.
            if q.borrow().payload.is_none()
                && crate::interpreter::current_task_id().is_some()
            {
                crate::interpreter::block_current_task_on_slot(&q);
                return Err(RuntimeError::TaskBlocked);
            }
            let slot = q.borrow();
            match &slot.payload {
                Some(Value::Product { fields, names }) => {
                    match names
                        .as_ref()
                        .and_then(|ns| ns.iter().position(|n| n == name))
                    {
                        Some(i) => fields
                            .get(i)
                            .cloned()
                            .ok_or_else(|| field_oob(name, fields.len())),
                        None => Err(RuntimeError::TypeError {
                            expected: format!("a payload field named '{}'", name),
                            found: format!(
                                "product with fields {:?}",
                                names.as_ref().map(|ns| ns.as_ref()).unwrap_or(&vec![])
                            ),
                        }),
                    }
                }
                Some(other) => Err(RuntimeError::TypeError {
                    expected: format!("a payload with field '{}'", name),
                    found: describe_value(other),
                }),
                None => {
                    // Unready OUTSIDE a task: strict error stands — top-level
                    // reads gate on .^Ready (SPEC §9.5). TEMP undo: collapse
                    // this whole pre-check + arm to revert Phase B blocking.
                    Err(RuntimeError::TypeError {
                        expected: format!("'{}' on an event port", name),
                        found: "no pending event — the port is not Ready".into(),
                    })
                }
            }
        }
        other => Err(RuntimeError::TypeError {
            expected: "a struct value".into(),
            found: describe_value(&other),
        }),
    }
}

/// A user-facing field-index error (house style: what's wrong + fix).
fn field_oob(name: &str, len: usize) -> RuntimeError {
    RuntimeError::TypeError {
        expected: format!("a field in the declared struct ({} fields)", len),
        found: format!("field '{}'", name),
    }
}

/// 2026-08-06 (Slice D): a method call. The full method-body dispatch needs
/// the typechecker's member registry (codegen uses obj_members), which the
/// dynamic interpreter does not carry. Evaluate the receiver and args for
/// side effects, then: an intrinsic (`name#`) dispatches with recv+args;
/// otherwise the name is looked up as a binding (mirroring the Call
/// simplification). No match returns Void rather than faking success.
fn eval_method_call(
    recv: &Expr,
    name: &str,
    args: &[Expr],
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    let recv_val = eval_expr(recv, heap, scope.bindings, scope.functions)?;
    let arg_vals: Result<Vec<Value>, _> =
        args.iter().map(|a| eval_expr(a, heap, scope.bindings, scope.functions)).collect();
    dispatch_method_value(recv_val, name, arg_vals?, heap, scope)
}

/// 2026-09-16: dispatch a method call given the already-evaluated receiver and
/// arguments (chain back-references prepend leading values here).
fn dispatch_method_value(
    recv_val: Value,
    name: &str,
    arg_vals: Vec<Value>,
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    if name.ends_with('#') {
        let mut all = vec![recv_val];
        all.extend(arg_vals);
        return execute_intrinsic(name, &all, heap);
    }
    // 2026-08-22 (Phase 5b): dyn member dispatch — find the impl body under
    // `Concrete::fn`, thread the receiver when the trait fn's first parameter
    // is `Self`, and run it in a caller-seeded scope (the interpreter's
    // dynamic-scoping convention for user code).
    if let Value::Dyn { concrete, inner, .. } = &recv_val {
        let key = format!("{}::{}", concrete, name);
        let fn_def = scope.functions.get(&key).ok_or_else(|| RuntimeError::UndefinedFunction(key.clone()))?;
        let mut params = fn_def.parameters.clone();
        let mut local: HashMap<String, Value> = scope.bindings.clone();
        // Receiver threading by shape: one more parameter than call-site
        // arguments means the trait fn is Self-first — the object goes in
        // as that parameter.
        if params.len() == arg_vals.len() + 1 {
            let recv_param = params.remove(0);
            local.insert(recv_param, (**inner).clone());
        }
        if params.len() != arg_vals.len() {
            return Err(RuntimeError::TypeError {
                expected: format!("{} arguments", params.len()),
                found: format!("{} arguments", arg_vals.len()),
            });
        }
        for (p, v) in params.iter().zip(arg_vals.into_iter()) {
            local.insert(p.clone(), v);
        }
        let block = Expr::Block(fn_def.body.clone());
        return match eval_expr(&block, heap, &mut local, scope.functions) {
            Err(RuntimeError::TermReturn(v)) => Ok(v),
            other => other,
        };
    }
    // 2026-08-22 (Phase 7b, SPEC §9.5): INSTANCE member dispatch — the body
    // runs with the instance's slots and ports bound as names (the same
    // view the typechecker gives members); slot writes reflect back into
    // the instance so callers holding the identity observe mutations.
    if let Value::Instance { type_name, fields } = &recv_val {
        let key = format!("{}::{}", type_name, name);
        let fn_def = scope
            .functions
            .get(&key)
            .ok_or_else(|| RuntimeError::UndefinedFunction(key.clone()))?;
        let mut params = fn_def.parameters.clone();
        let mut local: HashMap<String, Value> = scope.bindings.clone();
        {
            let f = fields.borrow();
            for (k, v) in f.iter() {
                local.insert(k.clone(), v.clone());
            }
        }
        // A Self-first signature receives the instance itself.
        if params.len() == arg_vals.len() + 1 {
            let recv_param = params.remove(0);
            local.insert(recv_param, recv_val.clone());
        }
        if params.len() != arg_vals.len() {
            return Err(RuntimeError::TypeError {
                expected: format!("{} arguments", params.len()),
                found: format!("{} arguments", arg_vals.len()),
            });
        }
        for (p, v) in params.iter().zip(arg_vals.into_iter()) {
            local.insert(p.clone(), v);
        }
        let block = Expr::Block(fn_def.body.clone());
        let result = match eval_expr(&block, heap, &mut local, scope.functions) {
            Err(RuntimeError::TermReturn(v)) => Ok(v),
            other => other,
        };
        // Write back slot values (ports are EventQ handles — never rebound).
        {
            let mut f = fields.borrow_mut();
            for (k, v) in local.iter() {
                if !matches!(v, Value::EventQ(_)) && f.contains_key(k) {
                    f.insert(k.clone(), v.clone());
                }
            }
        }
        return result;
    }
    Ok(scope.bindings.get(name).cloned().unwrap_or(Value::Void))
}

/// 2026-09-16: evaluate a method call carrying chain back-references.
/// Walks the receiver chain to collect the result stack, resolves `.N` and
/// `.name` references to leading argument values, then dispatches.
fn eval_method_call_chained(
    recv: &Expr,
    name: &str,
    args: &[Expr],
    refs: &[ChainRef],
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    let stack = eval_chain_stack(recv, heap, scope)?;
    let recv_val = stack.last().cloned().unwrap_or(Value::Void);
    let mut leading = Vec::new();
    for cr in refs {
        match cr {
            ChainRef::Positional(n) => {
                let idx = stack.len().checked_sub(*n).filter(|i| *n > 0 && *i < stack.len());
                match idx {
                    Some(i) => leading.push(stack[i].clone()),
                    None => {
                        return Err(RuntimeError::TypeError {
                            expected: "a valid chain back-reference".into(),
                            found: format!(
                                "'.{}>>' — the chain has only {} available result(s); '.1' is the immediately previous result",
                                n, stack.len()
                            ),
                        });
                    }
                }
            }
            ChainRef::Named(name) => {
                let val = scope.bindings.get(name).cloned().ok_or_else(|| RuntimeError::TypeError {
                    expected: "a named chain capture in scope".into(),
                    found: format!(
                        "'.{}>>' — no capture named '{}'; bind one with `expr >> {}`",
                        name, name, name
                    ),
                })?;
                leading.push(val);
            }
        }
    }
    let arg_vals: Result<Vec<Value>, _> =
        args.iter().map(|a| eval_expr(a, heap, scope.bindings, scope.functions)).collect();
    let mut all = leading;
    all.extend(arg_vals?);
    dispatch_method_value(recv_val, name, all, heap, scope)
}

/// 2026-09-16: compute the runtime chain stack `[base, r1, ..., prev]` for an
/// expression. Nested MethodCall/Capture receivers are walked; the stack is
/// the sequence of values produced before the current call.
fn eval_chain_stack(
    expr: &Expr,
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Vec<Value>, RuntimeError> {
    match expr {
        Expr::MethodCall(recv, name, args, _, refs) => {
            let mut stack = eval_chain_stack(recv, heap, scope)?;
            let recv_val = stack.last().cloned().unwrap_or(Value::Void);
            let mut leading = Vec::new();
            for cr in refs {
                match cr {
                    ChainRef::Positional(n) => {
                        let idx = stack.len().checked_sub(*n).filter(|i| *n > 0 && *i < stack.len());
                        match idx {
                            Some(i) => leading.push(stack[i].clone()),
                            None => {
                                return Err(RuntimeError::TypeError {
                                    expected: "a valid chain back-reference".into(),
                                    found: format!(
                                        "'.{}>>' — the chain has only {} available result(s)",
                                        n, stack.len()
                                    ),
                                });
                            }
                        }
                    }
                    ChainRef::Named(name) => {
                        let val = scope.bindings.get(name).cloned().ok_or_else(|| {
                            RuntimeError::TypeError {
                                expected: "a named chain capture in scope".into(),
                                found: format!(
                                    "'.{}>>' — no capture named '{}'; bind one with `expr >> {}`",
                                    name, name, name
                                ),
                            }
                        })?;
                        leading.push(val);
                    }
                }
            }
            let arg_vals: Result<Vec<Value>, _> =
                args.iter().map(|a| eval_expr(a, heap, scope.bindings, scope.functions)).collect();
            let mut all = leading;
            all.extend(arg_vals?);
            let result = dispatch_method_value(recv_val, name, all, heap, scope)?;
            stack.push(result);
            Ok(stack)
        }
        Expr::Capture { expr, name } => {
            // 2026-09-16 (Bug C): an inline capture binds the name so a later
            // `.name>>` back-reference in the same chain resolves at runtime.
            let stack = eval_chain_stack(expr, heap, scope)?;
            if let Some(top) = stack.last() {
                scope.bindings.insert(name.clone(), top.clone());
            }
            Ok(stack)
        }
        _ => Ok(vec![eval_expr(expr, heap, scope.bindings, scope.functions)?]),
    }
}

/// The three optional slice bounds (`array[start:end:stride]`), borrowed.
struct SliceBounds<'a> {
    start: Option<&'a Expr>,
    end: Option<&'a Expr>,
    stride: Option<&'a Expr>,
}

/// 2026-08-06 (Slice F): `array[start:end:stride]` — Python-style slicing
/// (SPEC §16.5) over raw `Bits` (string/bytes content) and `Product`
/// (list/tuple). Bounds follow the CPython algorithm: negative indices wrap,
/// out-of-range bounds clamp, defaults are start=0/end=len/stride=1, and a
/// negative stride walks the sequence in reverse. Stride 0 is an error.
fn eval_slice(
    array: &Expr,
    bounds: SliceBounds<'_>,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let arr = eval_expr(array, heap, bindings, functions)?;
    let start = eval_opt_int(bounds.start, heap, bindings, functions)?;
    let end = eval_opt_int(bounds.end, heap, bindings, functions)?;
    let stride = match bounds.stride {
        Some(s) => eval_expr(s, heap, bindings, functions)?
            .as_i64()
            .ok_or_else(|| RuntimeError::TypeError {
                expected: "an integer slice stride".into(),
                found: "non-integer stride".into(),
            })?,
        None => 1,
    };
    match arr {
        Value::Bits(bytes) => {
            let sel = slice_indices(bytes.len(), start, end, stride)?;
            Ok(Value::bits(sel.into_iter().map(|i| bytes[i]).collect()))
        }
        Value::Product { fields, names } => {
            let sel = slice_indices(fields.len(), start, end, stride)?;
            let sliced: Vec<Value> = sel.iter().map(|&i| fields[i].clone()).collect();
            let sliced_names = names.map(|ns| {
                Arc::new(
                    ns.iter()
                        .enumerate()
                        .filter(|(i, _)| sel.contains(i))
                        .map(|(_, n)| n.clone())
                        .collect(),
                )
            });
            Ok(Value::Product { fields: sliced, names: sliced_names })
        }
        other => Err(RuntimeError::TypeError {
            expected: "a sliceable value (string or list)".into(),
            found: describe_value(&other),
        }),
    }
}

/// Evaluate an optional slice bound expression to an optional integer.
fn eval_opt_int(
    opt: Option<&Expr>,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Option<i64>, RuntimeError> {
    match opt {
        Some(e) => eval_expr(e, heap, bindings, functions)?
            .as_i64()
            .map(Some)
            .ok_or_else(|| RuntimeError::TypeError {
                expected: "an integer slice bound".into(),
                found: "non-integer bound".into(),
            }),
        None => Ok(None),
    }
}

/// CPython `slice.indices` normalization: map (start, stop, step) to the
/// concrete index list. Negative bounds wrap by length; positive-step
/// clamps into [0, len], negative-step into [-1, len-1]; step 0 is an error.
fn slice_indices(
    length: usize,
    start: Option<i64>,
    stop: Option<i64>,
    step: i64,
) -> Result<Vec<usize>, RuntimeError> {
    if step == 0 {
        return Err(RuntimeError::TypeError {
            expected: "a non-zero slice stride".into(),
            found: "stride 0".into(),
        });
    }
    let len = length as i64;
    if step > 0 {
        let lo = start.map(|s| wrap_bound(s, len)).unwrap_or(0).clamp(0, len);
        let hi = stop.map(|s| wrap_bound(s, len)).unwrap_or(len).clamp(0, len);
        let mut out = Vec::new();
        let mut i = lo;
        while i < hi {
            out.push(i as usize);
            i += step;
        }
        Ok(out)
    } else {
        let lo = start
            .map(|s| wrap_bound(s, len))
            .unwrap_or(len - 1)
            .clamp(-1, len - 1);
        let hi = stop
            .map(|s| wrap_bound(s, len))
            .unwrap_or(-1)
            .clamp(-1, len - 1);
        let mut out = Vec::new();
        let mut i = lo;
        while i > hi {
            out.push(i as usize);
            i += step;
        }
        Ok(out)
    }
}

/// Negative slice bounds wrap by the sequence length (Python convention).
fn wrap_bound(v: i64, len: i64) -> i64 {
    if v < 0 { v + len } else { v }
}

/// 2026-08-06 (Slice G): value-side reflection (typechecker's D1 table is the
/// authority on kind/target validity; this is the value computation).
/// Runtime targets: `Len` (String char count, product/sum field count),
/// `Absolute` (Int/Float abs). Compile-time targets: `Size`/`Bytes`
/// (field count or byte length), `Type` (the value's category string, or the
/// sum-variant name for a Sum). Unhandled targets return Void.
fn eval_reflect(
    recv: &Expr,
    name: &str,
    kind: ReflectKind,
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    let val = eval_expr(recv, heap, scope.bindings, scope.functions)?;
    let ct = matches!(kind, ReflectKind::CompileTime);
    match (name, ct) {
        // 2026-08-23 (SPEC sync): Event port readiness — runtime reflection.
        ("Ready", false) => match &val {
            Value::EventQ(q) => {
                let slot = q.borrow();
                Ok(Value::Atom(Atom::Bool(slot.ready)))
            }
            other => Err(RuntimeError::TypeError {
                expected: "an Event port for .^Ready".into(),
                found: describe_value(other),
            }),
        },
        // 2026-08-12 (Iterable protocol): String `Length` = the STORED BYTE
        // count (the [len] header). The UTF8 CHARACTER count is the
        // `CharCount#` intrinsic (a computed scan; SPEC §17.1/§17.3).
        ("Length", false) => match val {
            Value::Bits(_) => {
                let bytes = val.string_bytes(heap).unwrap_or_default();
                Ok(Value::int(bytes.len() as i64))
            }
            (Value::Product { fields, .. } | Value::Sum { payload: fields, .. }) => {
                Ok(Value::int(fields.len() as i64))
            }
            _ => Ok(Value::Void),
        },
        // 2026-08-14 (§6a.1 item 8): `.^^Size` and `.^^Bytes` are SPLIT —
        // `.^^Size` is the compile-time shape (a vector's element count; a
        // Product/Sum's field count; 1 for a scalar), matching the codegen's
        // vector_element_count; `.^^Bytes` is the stored BYTE length of a
        // Bits value. They were wrongly grouped before (Size returned byte
        // length).
        ("Size", true) => match val {
            (Value::Product { fields, .. } | Value::Sum { payload: fields, .. }) => {
                Ok(Value::int(fields.len() as i64))
            }
            _ => Ok(Value::int(1)),
        },
        ("Bytes", true) => match val {
            Value::Bits(bytes) => Ok(Value::int(bytes.len() as i64)),
            (Value::Product { fields, .. } | Value::Sum { payload: fields, .. }) => {
                Ok(Value::int(fields.len() as i64))
            }
            _ => Ok(Value::Void),
        },
        // `Type` (compile-time) is a frozen descriptor: the protocol category
        // code (must match the codegen's type_category_code, rule #4).
        ("Type", true) => Ok(Value::int(reflect_type_code(&val))),
        // 2026-08-14 (boundary plan): `Element` (compile-time) = the ELEMENT
        // type's category code of an iterable value — a `String` operand's
        // chars (`Char`), a Product's field sequence, a Range's counter
        // (`Int`). Matches the codegen's `.^^Element` fold (rule #4 parity).
        ("Element", true) => Ok(Value::int(reflect_element_code(&val))),
        _ => Ok(Value::Void),
    }
}

/// 2026-08-06 (Phase 8): the semantic category code for `Type` reflection —
/// must match backend/llvm/emit_expr.rs type_category_code.
/// Codes: Int=0, Float=1, Bool=2, Char=3, Bits=4, Product=5, Sum=6,
/// Ref=7, Closure=8, Void=9.
fn reflect_type_code(v: &Value) -> i64 {
    match v {
        Value::Atom(Atom::Int(_)) => 0,
        Value::Atom(Atom::Float(_)) => 1,
        Value::Atom(Atom::Bool(_)) => 2,
        Value::Atom(Atom::Char(_)) => 3,
        Value::Bits(_) => 4,
        Value::Product { .. } => 5,
        // A trait object marshals as its inner payload (the concrete value);
        // the trait/concrete names are compile-time dispatch metadata.
        Value::Dyn { inner, .. } => reflect_type_code(inner),
        // Ports marshal through their current payload; instances as products
        // of their field values.
        Value::EventQ(q) => {
            let slot = q.borrow();
            match &slot.payload {
                Some(p) => reflect_type_code(p),
                None => 9, // Void when no event is pending
            }
        }
        Value::Instance { fields, .. } => {
            let vals: Vec<Value> = fields.borrow().values().cloned().collect();
            reflect_type_code(&Value::Product { fields: vals, names: None })
        }
        Value::Sum { .. } => 6,
        Value::Ref(_) => 7,
        Value::Closure { .. } => 8,
        Value::Range { .. } => 10,
        Value::Void => 9,
    }
}

/// 2026-08-14 (boundary plan): the ELEMENT type's category code of an iterable
/// value, for `x.^^Element`. A `String` operand iterates `Char` (SPEC §17.2);
/// a Product's element is the category of its first field (the field sequence
/// is the element sequence); a Range iterates `Int` counters. Must agree with
/// the codegen's fold of the receiver's static element type (rule #4 parity) —
/// the typechecker is the authority on validity; this is the value computation.
fn reflect_element_code(v: &Value) -> i64 {
    match v {
        Value::Bits(_) => 3, // Char — a `String` operand iterates codepoints
        Value::Product { fields, .. } => fields
            .first()
            .map(reflect_type_code)
            .unwrap_or(5), // empty product → its own category (Product)
        Value::Range { .. } => 0, // Int counter
        Value::Sum { .. } => 6,
        other => reflect_type_code(other),
    }
}

/// 2026-08-06 (Slice C): Evaluate a match expression. The scrutinee is
/// matched against each arm in order: a pattern match with the arm's bindings
/// in scope, then a `when` guard if present. The first arm whose pattern
/// matches AND guard passes wins; its body runs with the pattern bindings.
/// No arm matching is a non-exhaustive match error.
fn eval_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let val = eval_expr(scrutinee, heap, bindings, functions)?;
    for arm in arms {
        let mut arm_bindings = bindings.clone();
        if pattern_match(&arm.pattern, &val, &mut arm_bindings) {
            if let Some(guard) = &arm.guard {
                let gv = eval_expr(guard, heap, &mut arm_bindings, functions)?;
                if !gv.is_true() {
                    continue;
                }
            }
            return eval_expr(&arm.body, heap, &mut arm_bindings, functions);
        }
    }
    Err(RuntimeError::NonExhaustiveMatch(describe_value(&val)))
}

/// Match a pattern against a value, inserting pattern bindings into
/// `bindings`. A failed match may leave partial bindings — callers use a
/// scratch binding map per arm. `EnumVariant` matches the derive CEGIS
/// `Sum { name, payload }` carrier; `Tuple` matches `Product`.
pub fn pattern_match(
    pat: &Pattern,
    val: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pat {
        Pattern::Wildcard => true,
        Pattern::Binding(name) => {
            bindings.insert(name.clone(), val.clone());
            true
        }
        // 2026-08-22 (spec-conformance plan Phase 3, SPEC §8.4): a typed
        // binding matches when the VALUE carries the member's dynamic shape.
        // Atom members (Int/Float/Bool/Char) are checked by variant; String
        // members match the Bits representation (the interpreter's string
        // carrier). Types with no dynamic shape in the interpreter (custom
        // structs, generics) bind unconditionally — their membership is
        // guaranteed statically by the typechecker.
        Pattern::TypedBinding(name, ty) => {
            let ok = value_has_type_shape(ty, val);
            if ok {
                bindings.insert(name.clone(), val.clone());
            }
            ok
        }
        Pattern::Literal(lit) => match literal_pattern_value(lit) {
            Some(lv) => &lv == val,
            None => false,
        },
        // 2026-08-06: `start..end` is a half-open range (SPEC §16.4) — matches
        // n in [start, end). Integer ranges only; float/non-integer values
        // fail to match.
        Pattern::Range(start, end) => {
            let s = match literal_pattern_value(start).and_then(|v| v.as_i64()) {
                Some(n) => n,
                None => return false,
            };
            let e = match literal_pattern_value(end).and_then(|v| v.as_i64()) {
                Some(n) => n,
                None => return false,
            };
            val.as_i64().is_some_and(|n| s <= n && n < e)
        }
        // 2026-08-06 (Phase 7): inclusive `a..=b` — n in [start, end].
        Pattern::RangeInclusive(start, end) => {
            let s = match literal_pattern_value(start).and_then(|v| v.as_i64()) {
                Some(n) => n,
                None => return false,
            };
            let e = match literal_pattern_value(end).and_then(|v| v.as_i64()) {
                Some(n) => n,
                None => return false,
            };
            val.as_i64().is_some_and(|n| s <= n && n <= e)
        }
        Pattern::Multi(ps) => ps.iter().any(|sp| pattern_match(sp, val, bindings)),
        Pattern::Tuple(pats) => {
            let items = match val {
                Value::Product { fields, .. } => fields,
                _ => return false,
            };
            items.len() == pats.len()
                && pats
                    .iter()
                    .zip(items.iter())
                    .all(|(p, item)| pattern_match(p, item, bindings))
        }
        Pattern::EnumVariant(name, subpats) => match val {
            Value::Sum { name: cname, payload: fields } => {
                // 2026-08-26 (qualified enum paths): compare by LAST segment
                // so `Res::Ok` patterns match bare-tagged sums and vice versa.
                let pat_last = name.rsplit("::").next().unwrap_or(name);
                let val_last = cname.rsplit("::").next().unwrap_or(cname);
                pat_last == val_last
                    && fields.len() == subpats.len()
                    && subpats
                        .iter()
                        .zip(fields.iter())
                        .all(|(p, field)| pattern_match(p, field, bindings))
            }
            _ => false,
        },
    }
}

/// The value a literal pattern denotes — no evaluation, only literals.
/// Non-literal expressions in a pattern position fail to match.
fn literal_pattern_value(lit: &Expr) -> Option<Value> {
    match lit {
        Expr::Decimal(n) => Some(Value::int(*n)),
        Expr::TaggedLiteral(n, _) => Some(Value::int(*n)),
        Expr::Float(f) => Some(Value::float(*f)),
        Expr::Bool(b) => Some(Value::bool(*b)),
        Expr::Char(c) => Some(Value::char(*c)),
        Expr::Quoted(bytes) | Expr::TaggedQuotedLiteral(bytes, _) => Some(Value::bits(bytes.clone())),
        _ => None,
    }
}

/// 2026-08-01: Native evaluation for plugin intercepts — the reference
/// implementation of the lowercase macros. The build path rewrites these
/// to stdlib/intrinsic calls at the Parsed stage; this path keeps the
/// interpreter correct when it runs the raw AST. Format strings parse
/// identically to codegen via crate::plugin::print_plugin::parse_format.
fn eval_intercept(
    name: &str,
    receiver: Option<&Expr>,
    args: &[Expr],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    // 2026-09-16 (Bug A): a chained plugin call (`obj.print!(x)`) passes the
    // receiver as the first argument (UFCS-style) — it is evaluated (side
    // effects preserved) and reaches the plugin as args[0]. It is no longer
    // silently dropped.
    let full_args: Vec<Expr> = match receiver {
        Some(r) => {
            let mut v = vec![(*r).clone()];
            v.extend(args.iter().cloned());
            v
        }
        None => args.to_vec(),
    };
    match name {
        "print" | "println" => eval_print_macro(name, &full_args, heap, bindings, functions),
        "get_env" => {
            let key = eval_string_arg(&full_args, heap, bindings, functions)?;
            let val = std::env::var(&key).unwrap_or_default();
            Ok(Value::bits(val.into_bytes()))
        }
        "get_env_int" => {
            let key = eval_string_arg(&full_args, heap, bindings, functions)?;
            let val = std::env::var(&key).unwrap_or_default();
            Ok(i64_to_bits(val.parse::<i64>().unwrap_or(0)))
        }
        // Deprecated PascalCase names — rejected by the typechecker with a
        // rename hint in the build path; mirrored here for interpreter parity.
        "Print" | "PrintLn" => Err(RuntimeError::UnsupportedIntrinsic(format!(
            "'{}!' is deprecated — use the lowercase 'print!' / 'println!' macros",
            name
        ))),
        "GetEnvInt" | "GetEnv" | "GetEnvOrDefault" => Err(RuntimeError::UnsupportedIntrinsic(
            format!(
                "'{}!' is deprecated — use the lowercase 'get_env_int!' / 'get_env!' macros",
                name
            ),
        )),
        _ => Err(RuntimeError::UnsupportedIntrinsic(format!(
            "plugin-intercept {}",
            name
        ))),
    }
}

/// Evaluate the print!/println! macro against runtime values.
fn eval_print_macro(
    name: &str,
    args: &[Expr],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let is_println = name == "println";
    if args.is_empty() {
        if is_println {
            // 2026-08-01 (audit): the newline is a Char literal printed through
            // the generic Print# path (line termination is the macro's job).
            print_value(&Value::Atom(Atom::Char('\n')), heap)?;
            return Ok(Value::Void);
        }
        return Err(RuntimeError::UnsupportedIntrinsic(
            "print! requires a value or a format-string argument".to_string(),
        ));
    }

    if let Expr::Quoted(fmt) = &args[0] {
        let parts = crate::plugin::print_plugin::parse_format(fmt)
            .map_err(|msg| RuntimeError::UnsupportedIntrinsic(format!("{msg}")))?;
        let value_args = &args[1..];
        let mut next = 0usize;
        for part in &parts {
            match part {
                crate::plugin::print_plugin::FmtPart::Literal(seg) => {
                    print!("{}", String::from_utf8_lossy(seg));
                }
                crate::plugin::print_plugin::FmtPart::Next => {
                    let value = eval_expr(&value_args[next], heap, bindings, functions)?;
                    print_value(&value, heap)?;
                    next += 1;
                }
                crate::plugin::print_plugin::FmtPart::Position(n) => {
                    let value = eval_expr(&value_args[*n], heap, bindings, functions)?;
                    print_value(&value, heap)?;
                }
            }
        }
    } else {
        let value = eval_expr(&args[0], heap, bindings, functions)?;
        print_value(&value, heap)?;
    }

    if is_println {
        print_value(&Value::Atom(Atom::Char('\n')), heap)?;
    }
    Ok(Value::Void)
}

/// Print a runtime value by its representation: Float, integer, character,
/// boolean, or string bits — mirroring the generic `Print#` dispatch in
/// codegen. Bool prints true/false (its natural representation; an explicit
/// cast to Int is what yields 1/0), matching `__print_bool`.
fn print_value(value: &Value, heap: &mut VirtualHeap) -> Result<(), RuntimeError> {
    match value {
        Value::Atom(Atom::Float(f)) => {
            print!("{}", f);
        }
        Value::Atom(Atom::Int(n)) => {
            print!("{}", n);
        }
        Value::Atom(Atom::Bool(b)) => {
            print!("{}", if *b { "true" } else { "false" });
        }
        Value::Atom(Atom::Char(c)) => {
            print!("{}", c);
        }
        Value::Bits(bytes) => {
            execute_intrinsic("Print#", &[Value::bits(bytes.clone())], heap)?;
        }
        _ => {}
    }
    Ok(())
}

/// Resolve a cast target type's protocol category through the casting graph
/// (Cast. universe properties) — the same source the LLVM backend uses, so
/// the interpreter's value conversion matches codegen. Never type-name
/// matching. The lazily-built default universe covers the bootstrap
/// primitives; custom types resolve to "Bit" (identity reinterpretation).
fn target_protocol_category(ty: &Type) -> String {
    use std::sync::OnceLock;
    static CTX: OnceLock<(
        crate::casting::graph::CastingGraph,
        crate::type_universe::TypeUniverse,
    )> = OnceLock::new();
    let (graph, universe) = CTX.get_or_init(|| {
        (
            crate::casting::graph::CastingGraph::new(),
            crate::type_universe::TypeUniverse::new(),
        )
    });
    graph.type_to_protocol(universe, ty).0
}

/// Evaluate args[0] and decode it as a string (Bits) for env-var keys.
/// 2026-08-13 (pack): `x as Bits<N>` truncates an integer value to N bits
/// (the cast width assertion, mirroring the backend's `trunc i{N}`). Bits<0>
/// is a zero-width domain; Bits<64> is identity.
fn eval_bits_cast(v: Value, bits: u64) -> Value {
    match v {
        Value::Atom(Atom::Int(n)) => {
            let masked = if bits < 64 {
                let mask = if bits == 0 { 0u64 } else { (1u64 << bits) - 1 };
                n as u64 & mask
            } else {
                n as u64
            };
            Value::Atom(Atom::Int(masked as i64))
        }
        other => other,
    }
}

fn eval_string_arg(
    args: &[Expr],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<String, RuntimeError> {
    let value = eval_expr(&args[0], heap, bindings, functions)?;
    match value {
        Value::Bits(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
        other => Err(RuntimeError::TypeError {
            expected: "String".into(),
            found: format!("{other:?}"),
        }),
    }
}

/// Evaluate a binary operation.
fn eval_binary_op(
    kind: &BinaryOpKind,
    lhs: &Expr,
    rhs: &Expr,
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    let lv = eval_expr(lhs, heap, scope.bindings, scope.functions)?;
    let rv = eval_expr(rhs, heap, scope.bindings, scope.functions)?;

    match kind {
        BinaryOpKind::Add => {
            // 2026-08-03: `+` is string concat for String/Blob operands (the
            // Concat op). The backend routes +-on-strings to concat via the
            // string_concat rewrite; the interpreter must match (rule 4).
            if let (Some(a), Some(b)) = (lv.string_bytes(heap), rv.string_bytes(heap)) {
                let mut out = a;
                out.extend_from_slice(&b);
                return Ok(Value::bits(out));
            }
            // Simplified: try arithmetic, fall back to intrinsic
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(i64_to_bits(a.wrapping_add(b))),
                _ => execute_intrinsic("Add#", &[lv, rv], heap),
            }
        }
        BinaryOpKind::Concat => {
            // 2026-08-03: `++`/Concat — string concatenation (interpreter
            // reference for the backend's Concat emitter).
            if let (Some(a), Some(b)) = (lv.string_bytes(heap), rv.string_bytes(heap)) {
                let mut out = a;
                out.extend_from_slice(&b);
                return Ok(Value::bits(out));
            }
            execute_intrinsic("Concat#", &[lv, rv], heap)
        }
        BinaryOpKind::Sub => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(i64_to_bits(a.wrapping_sub(b))),
                _ => execute_intrinsic("Sub#", &[lv, rv], heap),
            }
        }
        BinaryOpKind::Mul => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(i64_to_bits(a.wrapping_mul(b))),
                _ => execute_intrinsic("Mul#", &[lv, rv], heap),
            }
        }
        BinaryOpKind::Eq => {
            // 2026-08-01 (B1): content equality — two Strings are equal when
            // their payload bytes match, not when they live at the same
            // address. string_bytes() derefs both representations (raw Bits
            // and heap handles); when BOTH operands are Strings, compare
            // content. Otherwise fall through to numeric equality (ints,
            // floats, bools). This is the interpreter-first half of B1 (rule
            // #4); the LLVM backend mirrors it in emit_expr.rs.
            match (lv.string_bytes(heap), rv.string_bytes(heap)) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a == b))),
                _ => Ok(Value::Atom(Atom::Bool(lv.as_i64() == rv.as_i64()))),
            }
        }
        BinaryOpKind::Lt => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a < b))),
                _ => Ok(Value::Atom(Atom::Bool(false))),
            }
        }
        BinaryOpKind::Gt => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a > b))),
                _ => Ok(Value::Atom(Atom::Bool(false))),
            }
        }
        BinaryOpKind::Le => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a <= b))),
                _ => Ok(Value::Atom(Atom::Bool(false))),
            }
        }
        BinaryOpKind::Ge => {
            let la = lv.as_i64();
            let ra = rv.as_i64();
            match (la, ra) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a >= b))),
                _ => Ok(Value::Atom(Atom::Bool(false))),
            }
        }
        BinaryOpKind::Neq => {
            // 2026-08-01 (B1): content inequality — mirrors the Eq arm above
            // (deref both Strings, compare payload bytes).
            match (lv.string_bytes(heap), rv.string_bytes(heap)) {
                (Some(a), Some(b)) => Ok(Value::Atom(Atom::Bool(a != b))),
                _ => Ok(Value::Atom(Atom::Bool(lv.as_i64() != rv.as_i64()))),
            }
        }
        BinaryOpKind::And => {
            let lb = lv.is_true();
            let rb = rv.is_true();
            Ok(Value::Atom(Atom::Bool(lb && rb)))
        }
        BinaryOpKind::Or => {
            let lb = lv.is_true();
            let rb = rv.is_true();
            Ok(Value::Atom(Atom::Bool(lb || rb)))
        }
        BinaryOpKind::BitAnd | BinaryOpKind::BitOr | BinaryOpKind::BitXor => {
            // 2026-08-01 (B1): String bitwise defaults — operate on content
            // bytes and return a NEW string of the same length (interpreter
            // half of B1; the backend mirrors it with briev_str_band/bor/bxor).
            // When both operands deref as strings, apply the byte-wise op.
            match (lv.string_bytes(heap), rv.string_bytes(heap)) {
                (Some(a), Some(b)) => {
                    if a.len() != b.len() {
                        return Err(RuntimeError::TypeError {
                            expected: "String bitwise operands of equal length".into(),
                            found: format!("{} vs {} bytes", a.len(), b.len()),
                        });
                    }
                    let result: Vec<u8> = a.iter().zip(b.iter()).map(|(x, y)| match kind {
                        BinaryOpKind::BitAnd => x & y,
                        BinaryOpKind::BitOr => x | y,
                        _ => x ^ y,
                    }).collect();
                    Ok(Value::bits(result))
                }
                _ => {
                    // 2026-08-03: Non-string operands (Int/Ptr) fall through to
                    // the integer bitwise intrinsics (BitAnd#/BitOr#/BitXor#),
                    // restoring the pre-B1 path that B1's string-default match
                    // arm shadowed (ba1d02b4) — it returned bool_to_bits(false)
                    // for every non-string operand, silently zeroing Int & Int.
                    // The backends emit real integer bitwise ops (LLVM and/or/xor);
                    // the interpreter is the reference and must match.
                    execute_intrinsic(&format!("{:?}#", kind), &[lv, rv], heap)
                }
            }
        }
        _ => {
            // Pass through unknown operators as intrinsic calls
            let op_name = format!("{:?}#", kind);
            execute_intrinsic(&op_name, &[lv, rv], heap)
        }
    }
}

/// Evaluate a unary operation.
fn eval_unary_op(
    kind: &UnaryOpKind,
    expr: &Expr,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let val = eval_expr(expr, heap, bindings, functions)?;
    match kind {
        UnaryOpKind::Neg => {
            let n = val.as_i64().unwrap_or(0);
            Ok(i64_to_bits(n.wrapping_neg()))
        }
        UnaryOpKind::Not => {
            let b = val.is_true();
            Ok(bool_to_bits(!b))
        }
        UnaryOpKind::BitNot => {
            // 2026-08-01 (B1): String unary bitwise default — complement each
            // content byte, same length (interpreter half; the backend emits
            // briev_str_bnot).
            if let Some(bytes) = val.string_bytes(heap) {
                return Ok(Value::bits(bytes.iter().map(|b| !b).collect()));
            }
            let n = val.as_i64().unwrap_or(0);
            Ok(i64_to_bits(!n))
        }
    }
}

/// Evaluate a block of statements.
fn eval_block(
    stmts: &[Statement],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    let mut result = Value::Void;
    for stmt in stmts {
        result = eval_statement(stmt, heap, bindings, functions)?;
    }
    Ok(result)
}

/// Evaluate an if expression.
fn eval_if(
    cond: &Expr,
    then: &Expr,
    else_: &Option<Box<Expr>>,
    heap: &mut VirtualHeap,
    scope: &mut EvalScope,
) -> Result<Value, RuntimeError> {
    let cv = eval_expr(cond, heap, scope.bindings, scope.functions)?;
    if cv.is_true() {
        eval_expr(then, heap, scope.bindings, scope.functions)
    } else if let Some(else_) = else_ {
        eval_expr(else_, heap, scope.bindings, scope.functions)
    } else {
        Ok(Value::Void)
    }
}

/// Evaluate a statement.
/// 2026-08-17 (foreach break): evaluate a `foreach` body once. Returns
/// `false` if the body executed a `break;` (the innermost foreach must stop
/// iterating). A `Break` is swallowed here — it never propagates outward.
/// `result` accumulates the last statement's value (the foreach's return
/// value), matching the pre-break semantics.
fn run_foreach_body(
    body: &[Statement],
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
    result: &mut Value,
) -> Result<bool, RuntimeError> {
    for stmt in body {
        match eval_statement(stmt, heap, bindings, functions) {
            Ok(v) => *result = v,
            Err(RuntimeError::Break) => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

pub fn eval_statement(
    stmt: &Statement,
    heap: &mut VirtualHeap,
    bindings: &mut HashMap<String, Value>,
    functions: &HashMap<String, crate::interpreter::FunctionDef>,
) -> Result<Value, RuntimeError> {
    match stmt {
        // 2026-08-22 (Phase 8): yield; is a no-op in the eager reference
        // scheduler — the concurrent scheduler's future suspension point.
        Statement::Yield => Ok(Value::Void),
        // 2026-08-23 (SPEC §10.x): check — runtime assertion for unprovable
        // loops. Failure triggers rollback.
        Statement::Check(cond) => {
            let v = eval_expr(cond, heap, bindings, functions)?;
            let ok = v.is_true();
            if !ok {
                Err(RuntimeError::ContractViolation(format!(
                    "liveness check failed: {}", cond
                )))
            } else {
                Ok(Value::Void)
            }
        }
        Statement::Let { name, ty, expr, .. } => {
            if let Some(expr) = expr {
                let val = eval_expr(expr, heap, bindings, functions)?;
                // 2026-08-22 (Phase 5b): an explicit `dyn Trait` annotation
                // wraps the value — the coercion site. The concrete type name
                // comes from the expression syntactically (constructor call or
                // another dyn binding); there is no runtime type reflection
                // over plain products.
                // 2026-08-22 (Phase 5b v1): coercions resolve the concrete
                // type from a CONSTRUCTOR expression or another dyn binding.
                // Identifier-of-typed-let needs binding-type provenance
                // threaded through EvalScope — tracked as a Phase 5c limit
                // in BUGS.md; the error says exactly what works today.
                let val = match ty.as_ref().and_then(|t| match t {
                    crate::ast::Type::Dyn(inner) => Some(&**inner),
                    _ => None,
                }) {
                    Some(tr) => {
                        let trait_name = match tr {
                            crate::ast::Type::Custom(n) => n.clone(),
                            other => format!("{}", other),
                        };
                        let concrete = match expr {
                            Expr::StructLiteral { type_name, .. } => type_name.clone(),
                            Expr::Identifier(src_name) => match bindings.get(src_name) {
                                Some(Value::Dyn { concrete, .. }) => concrete.clone(),
                                _ => {
                                    return Err(RuntimeError::TypeError {
                                        expected: format!(
                                            "a constructor or another `dyn` binding to coerce into dyn {}",
                                            trait_name
                                        ),
                                        found: "a plain value — bind it via a dyn-annotated let at the constructor site".into(),
                                    });
                                }
                            },
                            _ => {
                                return Err(RuntimeError::TypeError {
                                    expected: format!(
                                        "a constructor or another `dyn` binding to coerce into dyn {}",
                                        trait_name
                                    ),
                                    found: "this expression form".into(),
                                });
                            }
                        };
                        Value::Dyn { trait_name, concrete, inner: Box::new(val) }
                    }
                    _ => val,
                };
                bindings.insert(name.clone(), val);
            }
            Ok(Value::Void)
        }
        Statement::Assign(lhs, rhs) => {
            let val = eval_expr(rhs, heap, bindings, functions)?;
            match lhs {
                Expr::Identifier(name) => {
                    bindings.insert(name.clone(), val);
                }
                // 2026-08-25 (Plan 3.6): element write `buf[i] = v` —
                // read-modify-write of the bound product (value semantics:
                // lanes are copied out, mutated, reinserted). Previously a
                // silent no-op; now it is either a real write or a real
                // error. The CIRCT backend lowers the same statement to
                // per-lane gated muxes (register file).
                Expr::Index(obj, idx) => {
                    if let Expr::Identifier(name) = obj.as_ref() {
                        let iv = eval_expr(idx, heap, bindings, functions)?;
                        let i = match iv {
                            Value::Atom(Atom::Int(n)) => n,
                            other => {
                                return Err(RuntimeError::TypeError {
                                    expected: "Int index".into(),
                                    found: format!("{:?}", other),
                                })
                            }
                        };
                        let cur = bindings.get(name).cloned().ok_or_else(|| {
                            RuntimeError::UndefinedVariable { name: name.clone() }
                        })?;
                        let mut fields = match cur {
                            Value::Product { fields, .. } => fields,
                            other => {
                                return Err(RuntimeError::TypeError {
                                    expected: format!("indexable value for '{}'", name),
                                    found: format!("{:?}", other),
                                })
                            }
                        };
                        let len = fields.len() as i64;
                        if i < 0 || i >= len {
                            return Err(RuntimeError::HeapError(format!(
                                "index {} out of bounds for '{}' ({} elements)",
                                i, name, len
                            )));
                        }
                        fields[i as usize] = val;
                        bindings.insert(name.clone(), Value::product(fields));
                    } else {
                        return Err(RuntimeError::TypeError {
                            expected: "identifier[index] assignment target".into(),
                            found: "compound index target".into(),
                        });
                    }
                }
                // 2026-08-25: never silent — an unassignable target is a
                // runtime error naming the shape (was a dropped value).
                other => {
                    return Err(RuntimeError::TypeError {
                        expected: "assignable target".into(),
                        found: format!("{:?}", other),
                    })
                }
            }
            Ok(Value::Void)
        }
        Statement::FreeHint(name) => {
            // 2026-08-26 (async Phase B): a freed TASK HANDLE cancels the
            // table entry — otherwise other awaits' round-robins would run a
            // task nobody can observe (SPEC §12.2 cancellation). Non-task
            // frees keep the Phase 5 behavior: local dies, later read errors.
            if let Some(Value::Atom(Atom::Int(raw))) = bindings.get(name) {
                if *raw >= 0 {
                    crate::interpreter::cancel_task(*raw as u64);
                }
            }
            bindings.remove(name);
            Ok(Value::Void)
        }
        Statement::KeepHint(_) => Ok(Value::Void),
        Statement::Trap => Err(RuntimeError::Trap),
        // 2026-09-06 (halt slice): the bare-metal stop — check mode reports
        // it like the backend's wfi-spin/trap emission.
        Statement::Halt => Err(RuntimeError::Halt),
        Statement::ArrowAssign { target, value, consume } => {
            let val = eval_expr(value, heap, bindings, functions)?;
            if let Some(t) = target.as_ref() {
                if let Expr::Identifier(name) = t.as_ref() {
                    // 2026-08-22 (Phase 7b, SPEC §9.5): FIRING an event port —
                    // `died <- value;` where the binding holds an EventQ sets
                    // the shared slot (Ready + payload). The handle itself is
                    // never rebound, so every wired consumer observes it.
                    // 2026-08-26 (Phase B): firing also WAKES tasks blocked on
                    // this slot (drain waiters → Ready). The wake runs after
                    // the slot borrow releases — fire_slot_wake mutates it.
                    let fired_q = match bindings.get(name) {
                        Some(Value::EventQ(q)) => {
                            {
                                let mut slot = q.borrow_mut();
                                slot.ready = true;
                                slot.payload = Some(val.clone());
                            }
                            crate::interpreter::fire_slot_wake(q);
                            true
                        }
                        _ => false,
                    };
                    if !fired_q {
                        bindings.insert(name.clone(), val);
                    }
                }
            }
            if *consume {
                // 2026-08-01 (Phase 3): a consumed value's local is dead after
                // the statement — remove it so a later read errors.
                if let Expr::Identifier(name) = value.as_ref() {
                    bindings.remove(name);
                }
            }
            Ok(Value::Void)
        }
        Statement::Expression(expr) => eval_expr(expr, heap, bindings, functions),
        Statement::Term(val) => {
            match val {
                Some(val) => {
                    // 2026-07-28: Term with value signals early return.
                    let result = eval_expr(val, heap, bindings, functions)?;
                    Err(RuntimeError::TermReturn(result))
                }
                None => {
                    // Term without value is a convergence checkpoint — continue.
                    Ok(Value::Void)
                }
            }
        }
        Statement::Guarded(cond, body) => {
            let cv = eval_expr(cond, heap, bindings, functions)?;
            if cv.is_true() {
                let mut result = Value::Void;
                for stmt in body {
                    result = eval_statement(stmt, heap, bindings, functions)?;
                }
                Ok(result)
            } else {
                Ok(Value::Void)
            }
        }
        Statement::Gate(cond) => {
            // 2026-07-26: Convergence gate — evaluate condition.
            // If false, the caller (txn runner) is expected to retry the body.
            // The compile-time analysis must prove this eventually converges.
            eval_expr(cond, heap, bindings, functions)?;
            Ok(Value::Void)
        }
        Statement::Block(stmts) => {
            let mut result = Value::Void;
            for stmt in stmts {
                result = eval_statement(stmt, heap, bindings, functions)?;
            }
            Ok(result)
        }
        Statement::EndProgram(val) => {
            // 2026-08-06 (endprogram plan): process boundary — a distinct
            // signal from TermReturn (which only ends the transaction). The
            // reactor stops on ProgramExit.
            match val {
                Some(val) => {
                    let result = eval_expr(val, heap, bindings, functions)?;
                    Err(RuntimeError::ProgramExit(result))
                }
                None => Err(RuntimeError::ProgramExit(Value::Void)),
            }
        }
        Statement::Rollback(_) => Ok(Value::Void),
        Statement::MetadataAssignment(_, _) => Ok(Value::Void),
        Statement::InlineAsm { .. } | Statement::InlineDefn(_) | Statement::InlineTxn(_) => Ok(Value::Void),
        Statement::SyncBlock(body) => {
            let mut result = Value::Void;
            for stmt in body {
                result = eval_statement(stmt, heap, bindings, functions)?;
            }
            Ok(result)
        }
        Statement::Mutex(body) => {
            let mut result = Value::Void;
            for stmt in body {
                result = eval_statement(stmt, heap, bindings, functions)?;
            }
            Ok(result)
        }
        // 2026-08-09 (Phase 10): `defer` is handled by the Interpreter wrapper
        // (exec_stmt pushes the body onto the defer stack, flushed LIFO on
        // term/rollback/endprogram). This standalone arm is a defensive
        // no-op for direct eval_statement callers (tests).
        Statement::Defer(_) => Ok(Value::Void),
        // 2026-08-07 (Phase 7): `foreach(item in list)` — the sole iteration
        // keyword (SPEC §11.4). The iterable is an integer range (`0..=n`) or
        // a collection (a Product for lists/vectors, or Bits for Data).
        Statement::Break => {
            // 2026-08-17 (foreach break): signal the innermost enclosing
            // foreach to stop iterating. The foreach evaluator intercepts this
            // (does NOT propagate outward), so it never surfaces to a caller.
            Err(RuntimeError::Break)
        }
        Statement::Foreach { item, list, body } => {
            let iterable = eval_expr(list, heap, bindings, functions)?;
            let mut result = Value::Void;
            match iterable {
                Value::Range { start, end, inclusive } => {
                    let last = if inclusive { end } else { end - 1 };
                    if start <= last {
                        for cur in start..=last {
                            bindings.insert(item.clone(), Value::Atom(Atom::Int(cur)));
                            if !run_foreach_body(body, heap, bindings, functions, &mut result)? {
                                break;
                            }
                        }
                    }
                }
                Value::Product { fields, .. } => {
                    for f in &fields {
                        bindings.insert(item.clone(), f.clone());
                        if !run_foreach_body(body, heap, bindings, functions, &mut result)? {
                            break;
                        }
                    }
                }
                Value::Bits(bytes) => {
                    // 2026-08-14 (String unification): a `String` operand
                    // iterates CHARs — decode UTF8 codepoints, one per
                    // iteration, matching the codegen's `briev_str_next_char`
                    // lane (SPEC §17.2 String → Char). Data (raw bytes) has no
                    // distinct value representation in the interpreter (both
                    // are `Value::Bits`); the Quoted-literal path types as
                    // String, so char decode is the reference semantics.
                    let mut i = 0usize;
                    while i < bytes.len() {
                        let b0 = bytes[i];
                        let (cp, width) = if b0 < 0x80 {
                            (b0 as u32, 1)
                        } else if b0 & 0xE0 == 0xC0 && i + 1 < bytes.len() {
                            (((b0 & 0x1F) as u32) << 6 | (bytes[i + 1] & 0x3F) as u32, 2)
                        } else if b0 & 0xF0 == 0xE0 && i + 2 < bytes.len() {
                            (((b0 & 0x0F) as u32) << 12 | ((bytes[i + 1] & 0x3F) as u32) << 6 | (bytes[i + 2] & 0x3F) as u32, 3)
                        } else if b0 & 0xF8 == 0xF0 && i + 3 < bytes.len() {
                            (((b0 & 0x07) as u32) << 18 | ((bytes[i + 1] & 0x3F) as u32) << 12 | ((bytes[i + 2] & 0x3F) as u32) << 6 | (bytes[i + 3] & 0x3F) as u32, 4)
                        } else {
                            (b0 as u32, 1)
                        };
                        bindings.insert(item.clone(), Value::Atom(Atom::Char(char::from_u32(cp).unwrap_or('?'))));
                        if !run_foreach_body(body, heap, bindings, functions, &mut result)? {
                            break;
                        }
                        i += width;
                    }
                }
                other => {
                    return Err(RuntimeError::TypeError {
                        expected: "an iterable (a range, list, or byte data)".into(),
                        found: format!("{}", describe_value(&other)),
                    });
                }
            }
            Ok(result)
        }
        Statement::TrgBinding { .. } => Ok(Value::Void),
        Statement::Match { .. } => unreachable!("match only in $defn"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval1(expr: &Expr) -> Value {
        eval_expr(
            expr,
            &mut VirtualHeap::new(),
            &mut HashMap::new(),
            &HashMap::new(),
        )
        .unwrap()
    }

    /// Evaluate expecting an error — the error-testing twin of `eval1`.
    fn eval1_err(expr: &Expr) -> String {
        eval_expr(
            expr,
            &mut VirtualHeap::new(),
            &mut HashMap::new(),
            &HashMap::new(),
        )
        .err()
        .unwrap_or_else(|| panic!("expected error for {expr:?}"))
        .to_string()
    }

    // 2026-09-16: chain back-references (`.N>>`) resolve to leading args.
    #[test]
    fn chain_backref_prepends_leading_arg() {
        // `5.Add#(3).1>>Add#(0)` — the `.1` leading ref forwards the previous
        // result (8) as the intrinsic's FIRST operand: Add#(8, 8, 0) = 16.
        // Without leading passing this would be Add#(8, 0) = 8.
        let inner = Expr::MethodCall(
            Box::new(Expr::Decimal(5)),
            "Add#".into(),
            vec![Expr::Decimal(3)],
            None,
            vec![],
        );
        let outer = Expr::MethodCall(
            Box::new(inner),
            "Add#".into(),
            vec![Expr::Decimal(0)],
            None,
            vec![ChainRef::Positional(1)],
        );
        let val = eval1(&outer);
        assert_eq!(val.as_i64(), Some(16), "leading ref must reach the intrinsic");
    }

    #[test]
    fn chain_capture_binds_name() {
        // `expr >> name` binds the value; the interpreter's Capture arm stores it.
        let mut bindings = HashMap::new();
        let cap = Expr::Capture {
            expr: Box::new(Expr::Decimal(42)),
            name: "step".into(),
        };
        let val = eval_expr(
            &cap,
            &mut VirtualHeap::new(),
            &mut bindings,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(val.as_i64(), Some(42));
        assert_eq!(bindings.get("step").and_then(Value::as_i64), Some(42));
    }

    #[test]
    fn chained_plugin_evaluates_receiver() {
        // 2026-09-16 (Bug A): a chained plugin call evaluates its receiver —
        // an undefined receiver must error, not be silently dropped.
        let expr = Expr::PluginIntercept {
            name: "print".into(),
            args: vec![Expr::Decimal(1)],
            type_args: vec![],
            receiver: Some(Box::new(Expr::Identifier("nope".into()))),
            chain_refs: vec![],
        };
        let err = eval1_err(&expr);
        assert!(
            err.contains("nope"),
            "receiver must be evaluated; got: {err}"
        );
    }

    #[test]
    fn inline_capture_binds_for_named_backref() {
        // 2026-09-16 (Bug C): `5 >> mid .mid>>Add#(...)` — an INLINE capture
        // mid-chain binds `mid` so the `.mid>>` reference in the same chain
        // resolves. Previously eval_chain_stack skipped the binding.
        let cap = Expr::Capture {
            expr: Box::new(Expr::Decimal(5)),
            name: "mid".into(),
        };
        let outer = Expr::MethodCall(
            Box::new(cap),
            "Add#".into(),
            vec![Expr::Identifier("mid".into())],
            None,
            vec![ChainRef::Named("mid".into())],
        );
        // receiver=5, leading=mid=5, args=[mid=5] → Add#(5, 5, 5) = 10.
        let val = eval1(&outer);
        assert_eq!(val.as_i64(), Some(10), "inline capture must resolve .mid>>");
    }

    // 2026-08-01 (audit): Char/Bool are first-class values — literals
    // produce Value::Atom(Atom::Char)/Value::Atom(Atom::Bool), and casts convert across categories
    // (mirroring codegen, so Print# prints the same thing on both backends).

    #[test]
    fn test_char_literal_is_char_value() {
        assert_eq!(eval1(&Expr::Char('A')), Value::Atom(Atom::Char('A')));
        assert_eq!(eval1(&Expr::Char('A')).as_i64(), Some(65));
    }

    #[test]
    fn test_bool_literal_is_bool_value() {
        assert_eq!(eval1(&Expr::Bool(true)), Value::Atom(Atom::Bool(true)));
        assert_eq!(eval1(&Expr::Bool(true)).as_i64(), Some(1));
        assert!(eval1(&Expr::Bool(false)).is_true() == false);
    }

    #[test]
    fn test_cast_bool_to_int_is_0_or_1() {
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Bool(true)), Type::int())),
            Value::Atom(Atom::Int(1))
        );
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Bool(false)), Type::int())),
            Value::Atom(Atom::Int(0))
        );
    }

    #[test]
    fn test_cast_int_to_sub_byte_bits_truncates() {
        // 2026-08-13 (pack): `as Bits<N>` asserts width N — the interpreter
        // mirrors the backend's truncation, so `16 as Bits<4>` is 0 and a
        // 12-bit cast masks to 0xFFF. Whole-word widths (Bits<64>) are
        // identity.
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Decimal(16)), Type::bits(4))),
            Value::Atom(Atom::Int(0))
        );
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Decimal(0x1234)), Type::bits(12))),
            Value::Atom(Atom::Int(0x234))
        );
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Decimal(0x1122334455667788)), Type::bits(64))),
            Value::Atom(Atom::Int(0x1122334455667788))
        );
    }


    #[test]
    fn test_cast_char_to_int_is_code_point() {
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Char('A')), Type::int())),
            Value::Atom(Atom::Int(65))
        );
    }

    #[test]
    fn test_cast_int_to_char_builds_character() {
        assert_eq!(
            eval1(&Expr::Cast(Box::new(Expr::Decimal(65)), Type::char_())),
            Value::Atom(Atom::Char('A'))
        );
    }

    #[test]
    fn test_char_promotes_in_arithmetic() {
        let add = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Char('A')),
            Box::new(Expr::Decimal(1)),
        );
        assert_eq!(eval1(&add).as_i64(), Some(66));
    }

    #[test]
    fn test_plus_strings_concat() {
        // 2026-08-03: `+` is string concat for String operands.
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Quoted(b"foo".to_vec())),
            Box::new(Expr::Quoted(b"bar".to_vec())),
        );
        let result = eval1(&expr);
        assert_eq!(result.string_bytes(&VirtualHeap::new()), Some(b"foobar".to_vec()));
    }

    #[test]
    fn test_plus_ints_still_add() {
        // Numeric + stays arithmetic.
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Decimal(20)),
            Box::new(Expr::Decimal(22)),
        );
        assert_eq!(eval1(&expr).as_i64(), Some(42));
    }

    #[test]
    fn test_comparison_returns_bool_value() {
        let eq = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Decimal(42)),
            Box::new(Expr::Decimal(42)),
        );
        assert_eq!(eval1(&eq), Value::Atom(Atom::Bool(true)));
        assert!(eval1(&eq).is_true());
    }

    // 2026-08-06 (Slice B): unnamed products, indexing, and membership.

    #[test]
    fn test_tuple_evaluates_to_product() {
        let t = Expr::Tuple(vec![Expr::Decimal(1), Expr::Decimal(2), Expr::Decimal(3)]);
        assert_eq!(
            eval1(&t),
            Value::product(vec![Value::int(1), Value::int(2), Value::int(3)])
        );
    }

    #[test]
    fn test_list_evaluates_to_product() {
        let l = Expr::List(vec![Expr::Bool(true), Expr::Decimal(7)]);
        assert_eq!(
            eval1(&l),
            Value::product(vec![Value::bool(true), Value::int(7)])
        );
    }

    #[test]
    fn test_index_product_element() {
        let l = Expr::List(vec![Expr::Decimal(10), Expr::Decimal(20), Expr::Decimal(30)]);
        let idx = Expr::Index(Box::new(l), Box::new(Expr::Decimal(1)));
        assert_eq!(eval1(&idx).as_i64(), Some(20));
    }

    #[test]
    fn test_index_product_out_of_bounds_errors() {
        let l = Expr::List(vec![Expr::Decimal(10)]);
        let idx = Expr::Index(Box::new(l), Box::new(Expr::Decimal(5)));
        let err = eval1_err(&idx);
        assert!(err.contains("index"), "got: {err}");
    }

    #[test]
    fn test_index_negative_errors() {
        let l = Expr::List(vec![Expr::Decimal(10)]);
        let idx = Expr::Index(Box::new(l), Box::new(Expr::Decimal(-1)));
        assert!(eval1_err(&idx).contains("negative"));
    }

    #[test]
    fn test_index_bits_byte() {
        let s = Expr::Quoted(b"abc".to_vec());
        let idx = Expr::Index(Box::new(s), Box::new(Expr::Decimal(1)));
        assert_eq!(eval1(&idx), Value::bits(vec![b'b']));
    }

    #[test]
    fn test_enum_construct_match_interpret() {
        // 2026-08-26 (Track B): tuple-variant construction + match through
        // the interpreter — the reference semantics for backends.
        let src = r#"
enum Res {
    Ok(Int),
    Err(String),
};

defn make(i: Int) -> Res {
  term Ok(i);
}

defn go() -> Int {
  let r = make(5);
  term match r {
    Ok(v) => v,
    Err(_) => 0 - 1,
  };
}
"#;
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        let mut interp = crate::interpreter::Interpreter::new();
        interp.load_program(&items);
        let out = interp.call_function("go", &[]).unwrap();
        assert_eq!(
            out,
            crate::interpreter::Value::int(5),
            "got {:?}",
            out
        );
    }

    #[test]
    fn test_enum_unit_variant_construct_interpret() {
        // Zero-payload variants construct with zero args (SPEC §8.3).
        let src = r#"
enum Option {
    Some(Int),
    None,
};

defn get(o: Option) -> Int {
  term match o {
    Some(v) => v,
    None => 0 - 1,
  };
}

defn go() -> Int {
  term get(None()) + get(Some(42));
}
"#;
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        let mut interp = crate::interpreter::Interpreter::new();
        interp.load_program(&items);
        let out = interp.call_function("go", &[]).unwrap();
        assert_eq!(
            out,
            crate::interpreter::Value::int(41),
            "got {:?}",
            out
        );
    }

    #[test]
    fn test_enum_multi_payload_construct_match_interpret() {
        // 2026-08-26 (Track B): SPEC §8.3 — multi-payload variants store a
        // Tuple payload; both bindings extract positionally.
        let src = r#"
enum Reg {
    RegisterOk(String, Int),
    Failed(Int),
};

defn describe(r: Reg) -> Int {
  term match r {
    RegisterOk(name, count) => count,
    Failed(code) => 0 - code,
  };
}

defn go() -> Int {
  term describe(RegisterOk("sensor-a", 42)) + describe(Failed(9)) * 100;
}
"#;
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        let mut interp = crate::interpreter::Interpreter::new();
        interp.load_program(&items);
        let out = interp.call_function("go", &[]).unwrap();
        assert_eq!(
            out,
            crate::interpreter::Value::int(42 - 900),
            "got {:?}",
            out
        );
    }

    #[test]
    fn test_element_assign_writes_bound_product() {
        // 2026-08-25 (Plan 3.6): `buf[i] = v` on a let-bound list — was a
        // silent no-op, now a real read-modify-write.
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert(
            "buf".to_string(),
            Value::product(vec![
                Value::Atom(Atom::Int(1)),
                Value::Atom(Atom::Int(2)),
                Value::Atom(Atom::Int(3)),
            ]),
        );
        let assign = Statement::Assign(
            Expr::Index(
                Box::new(Expr::Identifier("buf".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
            Expr::Decimal(42),
        );
        eval_statement(&assign, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        let read = Expr::Index(
            Box::new(Expr::Identifier("buf".to_string())),
            Box::new(Expr::Decimal(1)),
        );
        assert_eq!(eval_expr(&read, &mut heap, &mut bindings, &HashMap::new()).unwrap(),
                   Value::Atom(Atom::Int(42)));
        // other lanes untouched
        let read0 = Expr::Index(
            Box::new(Expr::Identifier("buf".to_string())),
            Box::new(Expr::Decimal(0)),
        );
        assert_eq!(eval_expr(&read0, &mut heap, &mut bindings, &HashMap::new()).unwrap(),
                   Value::Atom(Atom::Int(1)));
    }

    #[test]
    fn test_element_assign_out_of_bounds_errors() {
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("buf".to_string(), Value::product(vec![Value::Atom(Atom::Int(7))]));
        let assign = Statement::Assign(
            Expr::Index(
                Box::new(Expr::Identifier("buf".to_string())),
                Box::new(Expr::Decimal(3)),
            ),
            Expr::Decimal(1),
        );
        let err = eval_statement(&assign, &mut heap, &mut bindings, &HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("out of bounds"), "got: {err}");
    }

    #[test]
    fn test_foreach_range_inclusive_accumulates() {
        // 2026-08-07 (Phase 7): `foreach(i in 0..=5) acc = acc + i` (SPEC
        // §11.4 counted iteration) accumulates 0+1+2+3+4+5 = 15.
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        let foreach = Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(5)),
                inclusive: true,
            }),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Identifier("i".to_string())),
                ),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(15));
    }

    #[test]
    fn test_foreach_range_exclusive_stops_short() {
        // `0..5` excludes the end — 0+1+2+3+4 = 10.
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        let foreach = Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(5)),
                inclusive: false,
            }),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Identifier("i".to_string())),
                ),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(10));
    }

    #[test]
    fn test_foreach_break_exits_early() {
        // 2026-08-17 (foreach break): `foreach i in 0..100 { when i == 3 { acc =
        // 42; break; } }` stops at i==3 — acc is 42, and the loop does NOT run
        // to 100 (a counter proves it).
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        bindings.insert("seen_last".to_string(), Value::Atom(Atom::Int(-1)));
        let foreach = Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(100)),
                inclusive: false,
            }),
            body: vec![
                Statement::Guarded(
                    Expr::BinaryOp(
                        BinaryOpKind::Eq,
                        Box::new(Expr::Identifier("i".to_string())),
                        Box::new(Expr::Decimal(3)),
                    ),
                    vec![
                        Statement::Assign(
                            Expr::Identifier("acc".to_string()),
                            Expr::Decimal(42),
                        ),
                        Statement::Break,
                    ],
                ),
                Statement::Assign(
                    Expr::Identifier("seen_last".to_string()),
                    Expr::Identifier("i".to_string()),
                ),
            ],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(42));
        // Break fired at i==3, so `seen_last` (set after the if each iteration)
        // is the LAST value seen before the break — 2 — proving the loop did
        // NOT continue past i==3 to 100.
        assert_eq!(bindings.get("seen_last").and_then(|v| v.as_i64()), Some(2));
    }

    #[test]
    fn test_foreach_over_empty_range_skips() {
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(7)));
        let foreach = Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(5)),
                end: Box::new(Expr::Decimal(0)),
                inclusive: false,
            }),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::Decimal(0),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(7));
    }

    #[test]
    fn test_foreach_over_product_iterates_elements() {
        // A collection iterable (a list) binds each element in turn — the
        // reference for `foreach(item in items)`.
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        let foreach = Statement::Foreach {
            item: "x".to_string(),
            list: Box::new(Expr::List(vec![Expr::Decimal(1), Expr::Decimal(2), Expr::Decimal(3)])),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Identifier("x".to_string())),
                ),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(6));
    }

    #[test]
    fn test_foreach_over_string_iterates_chars() {
        // 2026-08-14 (String unification): `foreach c in str` on a `String`
        // operand iterates UTF8 CODEPOINTS as Char values, not raw bytes
        // (SPEC §17.2 String → Char). Multibyte: "hé" is 0x68 0xC3 0xA9 0x65
        // (3 bytes, 2 chars).
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        let foreach = Statement::Foreach {
            item: "c".to_string(),
            list: Box::new(Expr::Quoted("hé".as_bytes().to_vec())),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(2));
    }

    #[test]
    fn test_foreach_over_string_accumulates_codepoints() {
        // The item is a Char carrying the codepoint value, not the byte.
        let mut heap = VirtualHeap::new();
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("acc".to_string(), Value::Atom(Atom::Int(0)));
        let foreach = Statement::Foreach {
            item: "c".to_string(),
            list: Box::new(Expr::Quoted("é".as_bytes().to_vec())),
            body: vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Cast(Box::new(Expr::Identifier("c".to_string())), crate::ast::Type::int())),
                ),
            )],
        };
        eval_statement(&foreach, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(bindings.get("acc").and_then(|v| v.as_i64()), Some(0xE9));
    }

    #[test]
    fn test_index_non_indexable_errors() {
        let idx = Expr::Index(Box::new(Expr::Decimal(42)), Box::new(Expr::Decimal(0)));
        assert!(eval1_err(&idx).contains("indexable"));
    }

    #[test]
    fn test_mask_index_bits_selects_true_positions() {
        // 2026-08-07 (Phase 7): `data[mask]` — a Bool-vector index selects
        // the bytes at the true positions (SPEC §16.5).
        let data = Expr::Quoted(b"\x01\x02\x03\x04\x05".to_vec());
        let mask = Expr::List(vec![
            Expr::Bool(true), Expr::Bool(false), Expr::Bool(true),
            Expr::Bool(false), Expr::Bool(true),
        ]);
        let idx = Expr::Index(Box::new(data), Box::new(mask));
        assert_eq!(eval1(&idx), Value::bits(vec![0x01, 0x03, 0x05]));
    }

    #[test]
    fn test_mask_index_all_false_yields_empty() {
        let data = Expr::Quoted(b"abc".to_vec());
        let mask = Expr::List(vec![Expr::Bool(false), Expr::Bool(false), Expr::Bool(false)]);
        let idx = Expr::Index(Box::new(data), Box::new(mask));
        assert_eq!(eval1(&idx), Value::bits(vec![]));
    }

    #[test]
    fn test_mask_index_longer_than_data_truncates() {
        // A mask longer than the data truncates (the mask governs), matching
        // the runtime helper.
        let data = Expr::Quoted(b"ab".to_vec());
        let mask = Expr::List(vec![Expr::Bool(true), Expr::Bool(false), Expr::Bool(true)]);
        let idx = Expr::Index(Box::new(data), Box::new(mask));
        assert_eq!(eval1(&idx), Value::bits(vec![b'a']));
    }

    #[test]
    fn test_mask_index_non_bits_source_errors() {
        // The mask path requires a byte-buffer source, mirroring the
        // typechecker/codegen boundary (Data only).
        let data = Expr::Decimal(42);
        let mask = Expr::List(vec![Expr::Bool(true)]);
        let idx = Expr::Index(Box::new(data), Box::new(mask));
        assert!(eval1_err(&idx).contains("byte data"), "got: {}", eval1_err(&idx));
    }

    #[test]
    fn test_mask_index_mixed_mask_is_not_a_mask() {
        // A list with a non-Bool element is NOT a mask — it falls to the
        // scalar path and errors on the non-integer index.
        let data = Expr::Quoted(b"abc".to_vec());
        let mask = Expr::List(vec![Expr::Bool(true), Expr::Decimal(7)]);
        let idx = Expr::Index(Box::new(data), Box::new(mask));
        assert!(eval1_err(&idx).contains("integer index"));
    }

    #[test]
    fn test_mask_index_product_selects_fields() {
        // 2026-08-07 (Phase 7): a Boolean mask over a product (a typed
        // vector) yields a new product of the selected fields — the
        // interpreter reference for `Int[N][mask]` → `List<Int>`.
        let v = Expr::List(vec![Expr::Decimal(10), Expr::Decimal(20), Expr::Decimal(30)]);
        let mask = Expr::List(vec![Expr::Bool(true), Expr::Bool(false), Expr::Bool(true)]);
        let idx = Expr::Index(Box::new(v), Box::new(mask));
        match eval1(&idx) {
            Value::Product { fields, .. } => {
                let vals: Vec<i64> = fields.iter().map(|f| f.as_i64().unwrap()).collect();
                assert_eq!(vals, vec![10, 30]);
            }
            other => panic!("expected a product, got {:?}", other),
        }
    }

    #[test]
    fn test_is_type_int_atom_membership() {
        let e = Expr::IsType(Box::new(Expr::Decimal(42)), Type::int());
        assert_eq!(eval1(&e), Value::Atom(Atom::Bool(true)));
    }

    #[test]
    fn test_is_type_float_atom_rejects_int_type() {
        let e = Expr::IsType(Box::new(Expr::Float(1.5)), Type::int());
        assert_eq!(eval1(&e), Value::Atom(Atom::Bool(false)));
    }

    #[test]
    fn test_print_dispatches_bool_and_char() {
        // Print# is the convenience intrinsic: Bool prints true/false, Char
        // prints the character. Both must dispatch without error.
        let mut heap = VirtualHeap::new();
        let r = execute_intrinsic("Print#", &[Value::Atom(Atom::Bool(true))], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(0));
        let r = execute_intrinsic("Print#", &[Value::Atom(Atom::Char('A'))], &mut heap).unwrap();
        assert_eq!(r.as_i64(), Some(0));
    }

    // 2026-08-06 (Slice C): match with patterns, guards, exhaustiveness.

    fn arm(pattern: Pattern, guard: Option<Expr>, body: i64) -> MatchArm {
        MatchArm {
            pattern,
            guard,
            body: Box::new(Expr::Decimal(body)),
        }
    }

    // 2026-08-22 (spec-conformance plan Phase 3, SPEC §8.4): typed
    // bindings of a structural sum dispatch on the value's dynamic shape.
    #[test]
    fn test_match_typed_binding_dispatches_on_member_shape() {
        let int_arm = MatchArm {
            pattern: Pattern::TypedBinding("n".into(), Box::new(Type::int())),
            guard: None,
            body: Box::new(Expr::Identifier("n".into())),
        };
        let str_arm = MatchArm {
            pattern: Pattern::TypedBinding("s".into(), Box::new(Type::string())),
            guard: None,
            body: Box::new(Expr::Decimal(7)),
        };
        let m_int = Expr::Match(
            Box::new(Expr::Decimal(42)),
            vec![int_arm.clone(), str_arm],
        );
        assert_eq!(eval1(&m_int).as_i64(), Some(42));

        let m_str = Expr::Match(
            Box::new(Expr::Quoted(b"hey".to_vec())),
            vec![
                MatchArm {
                    pattern: Pattern::TypedBinding("x".into(), Box::new(Type::int())),
                    guard: None,
                    body: Box::new(Expr::Decimal(1)),
                },
                MatchArm {
                    pattern: Pattern::TypedBinding("s".into(), Box::new(Type::string())),
                    guard: None,
                    body: Box::new(Expr::Decimal(7)),
                },
            ],
        );
        assert_eq!(eval1(&m_str).as_i64(), Some(7));
        let _ = int_arm;
    }

    #[test]
    fn test_match_literal_selects_arm() {
        let m = Expr::Match(
            Box::new(Expr::Decimal(0)),
            vec![
                arm(Pattern::Literal(Expr::Decimal(0)), None, 10),
                arm(Pattern::Wildcard, None, 99),
            ],
        );
        assert_eq!(eval1(&m).as_i64(), Some(10));
    }

    #[test]
    fn test_match_wildcard_fallback() {
        let m = Expr::Match(
            Box::new(Expr::Decimal(5)),
            vec![
                arm(Pattern::Literal(Expr::Decimal(0)), None, 10),
                arm(Pattern::Wildcard, None, 99),
            ],
        );
        assert_eq!(eval1(&m).as_i64(), Some(99));
    }

    #[test]
    fn test_match_guard_false_skips_arm() {
        let m = Expr::Match(
            Box::new(Expr::Decimal(5)),
            vec![
                arm(Pattern::Literal(Expr::Decimal(5)), Some(Expr::Bool(false)), 10),
                arm(Pattern::Wildcard, None, 99),
            ],
        );
        assert_eq!(eval1(&m).as_i64(), Some(99));
    }

    #[test]
    fn test_match_binding_in_body() {
        let m = Expr::Match(
            Box::new(Expr::Decimal(42)),
            vec![MatchArm {
                pattern: Pattern::Binding("x".into()),
                guard: None,
                body: Box::new(Expr::Identifier("x".into())),
            }],
        );
        assert_eq!(eval1(&m).as_i64(), Some(42));
    }

    #[test]
    fn test_match_binding_in_guard() {
        // `x when x > 5 => x` — the guard sees the pattern binding.
        let gt = Expr::BinaryOp(
            BinaryOpKind::Gt,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(5)),
        );
        let m = Expr::Match(
            Box::new(Expr::Decimal(10)),
            vec![MatchArm {
                pattern: Pattern::Binding("x".into()),
                guard: Some(gt),
                body: Box::new(Expr::Identifier("x".into())),
            }],
        );
        assert_eq!(eval1(&m).as_i64(), Some(10));
    }

    #[test]
    fn test_match_binding_guard_fails_then_fallback() {
        // x when x > 5 fails for x = 3; wildcard picks up.
        let gt = Expr::BinaryOp(
            BinaryOpKind::Gt,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(5)),
        );
        let m = Expr::Match(
            Box::new(Expr::Decimal(3)),
            vec![
                MatchArm {
                    pattern: Pattern::Binding("x".into()),
                    guard: Some(gt),
                    body: Box::new(Expr::Decimal(10)),
                },
                arm(Pattern::Wildcard, None, 99),
            ],
        );
        assert_eq!(eval1(&m).as_i64(), Some(99));
    }

    #[test]
    fn test_match_tuple_pattern_binds_fields() {
        let scrut = Expr::List(vec![Expr::Decimal(1), Expr::Decimal(2)]);
        let m = Expr::Match(
            Box::new(scrut),
            vec![MatchArm {
                pattern: Pattern::Tuple(vec![
                    Pattern::Literal(Expr::Decimal(1)),
                    Pattern::Binding("y".into()),
                ]),
                guard: None,
                body: Box::new(Expr::Identifier("y".into())),
            }],
        );
        assert_eq!(eval1(&m).as_i64(), Some(2));
    }

    #[test]
    fn test_match_range_half_open() {
        // 1..5 is [1, 5): 3 matches, 5 does not.
        let m = |n: i64| {
            Expr::Match(
                Box::new(Expr::Decimal(n)),
                vec![
                    arm(
                        Pattern::Range(Expr::Decimal(1), Expr::Decimal(5)),
                        None,
                        7,
                    ),
                    arm(Pattern::Wildcard, None, 0),
                ],
            )
        };
        assert_eq!(eval1(&m(3)).as_i64(), Some(7));
        assert_eq!(eval1(&m(1)).as_i64(), Some(7));
        assert_eq!(eval1(&m(5)).as_i64(), Some(0));
    }

    #[test]
    fn test_match_range_inclusive() {
        // 2026-08-06 (Phase 7): 1..=5 is [1, 5]: 3 and 5 match, 6 does not.
        let m = |n: i64| {
            Expr::Match(
                Box::new(Expr::Decimal(n)),
                vec![
                    arm(
                        Pattern::RangeInclusive(Expr::Decimal(1), Expr::Decimal(5)),
                        None,
                        7,
                    ),
                    arm(Pattern::Wildcard, None, 0),
                ],
            )
        };
        assert_eq!(eval1(&m(3)).as_i64(), Some(7));
        assert_eq!(eval1(&m(5)).as_i64(), Some(7));
        assert_eq!(eval1(&m(6)).as_i64(), Some(0));
    }

    #[test]
    fn test_match_enum_variant_over_sum() {
        let scrut = Value::Sum { name: "Foo".into(), payload: vec![Value::int(1), Value::int(2)] };
        let m = Expr::Match(
            Box::new(Expr::Identifier("scrut".into())),
            vec![MatchArm {
                pattern: Pattern::EnumVariant(
                    "Foo".into(),
                    vec![Pattern::Wildcard, Pattern::Literal(Expr::Decimal(2))],
                ),
                guard: None,
                body: Box::new(Expr::Decimal(88)),
            }],
        );
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("scrut".into(), scrut);
        let r = eval_expr(&m, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(r.as_i64(), Some(88));
    }

    #[test]
    fn test_match_non_exhaustive_errors() {
        let m = Expr::Match(
            Box::new(Expr::Decimal(5)),
            vec![arm(Pattern::Literal(Expr::Decimal(0)), None, 10)],
        );
        let err = eval1_err(&m);
        assert!(err.contains("non-exhaustive"), "got: {err}");
    }

    #[test]
    fn test_match_empty_arms_errors() {
        let m = Expr::Match(Box::new(Expr::Decimal(1)), vec![]);
        assert!(eval1_err(&m).contains("non-exhaustive"));
    }

    // 2026-08-06 (Slice D): struct literals, field access, method calls.

    fn person() -> Expr {
        Expr::StructLiteral {
            type_name: "Person".into(),
            fields: vec![
                ("name".into(), Expr::Quoted(b"ada".to_vec())),
                ("age".into(), Expr::Decimal(36)),
            ],
        }
    }

    #[test]
    fn test_struct_literal_is_named_product() {
        let r = eval1(&person());
        match r {
            Value::Product { fields, names } => {
                assert_eq!(fields, vec![Value::bits(b"ada".to_vec()), Value::int(36)]);
                assert_eq!(
                    names.as_ref().map(|n| n.as_ref()),
                    Some(&vec!["name".to_string(), "age".to_string()])
                );
            }
            other => panic!("expected Product, got {other:?}"),
        }
    }

    #[test]
    fn test_field_access_on_struct_literal() {
        let f = Expr::Field(Box::new(person()), "age".into());
        assert_eq!(eval1(&f).as_i64(), Some(36));
    }

    #[test]
    fn test_field_access_on_binding() {
        let f = Expr::Field(Box::new(Expr::Identifier("p".into())), "age".into());
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert(
            "p".into(),
            Value::named_product(vec![Value::int(36)], vec!["age".into()]),
        );
        let r = eval_expr(&f, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(r.as_i64(), Some(36));
    }

    #[test]
    fn test_field_access_unknown_field_errors() {
        let f = Expr::Field(Box::new(person()), "height".into());
        let err = eval1_err(&f);
        assert!(err.contains("field"), "got: {err}");
    }

    #[test]
    fn test_field_access_non_struct_errors() {
        let f = Expr::Field(Box::new(Expr::Decimal(5)), "x".into());
        assert!(eval1_err(&f).contains("struct"));
    }

    #[test]
    fn test_method_call_intrinsic_dispatches_with_receiver() {
        let m = Expr::MethodCall(Box::new(Expr::Decimal(-7)), "Abs#".into(), vec![], None, vec![]);
        assert_eq!(eval1(&m).as_i64(), Some(7));
    }

    #[test]
    fn test_method_call_binding_lookup() {
        let m = Expr::MethodCall(Box::new(Expr::Decimal(5)), "foo".into(), vec![], None, vec![]);
        assert_eq!(eval1(&m), Value::Void);
        let m2 = Expr::MethodCall(Box::new(Expr::Decimal(5)), "seven".into(), vec![], None, vec![]);
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("seven".into(), Value::int(7));
        let r = eval_expr(&m2, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(r.as_i64(), Some(7));
    }

    // 2026-08-06 (Slice E): closures.

    fn inc_by_one() -> Expr {
        Expr::Lambda(
            vec!["x".into()],
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(1)),
            )),
        )
    }

    #[test]
    fn test_lambda_is_closure_value() {
        assert!(matches!(eval1(&inc_by_one()), Value::Closure { .. }));
    }

    #[test]
    fn test_closure_applies_via_call() {
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("f".into(), eval1(&inc_by_one()));
        let call = Expr::Call("f".into(), vec![Expr::Decimal(41)], None);
        let r = eval_expr(&call, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(r.as_i64(), Some(42));
    }

    #[test]
    fn test_closure_captures_outer_binding() {
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("k".into(), Value::int(5));
        // (x) => x + k  — k is captured from the enclosing scope.
        let lam = Expr::Lambda(
            vec!["x".into()],
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("k".into())),
            )),
        );
        let f = eval_expr(&lam, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        bindings.insert("f".into(), f);
        let call = Expr::Call("f".into(), vec![Expr::Decimal(1)], None);
        assert_eq!(eval_expr(&call, &mut heap, &mut bindings, &HashMap::new()).unwrap().as_i64(), Some(6));
    }

    #[test]
    fn test_closure_arity_mismatch_errors() {
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("f".into(), eval1(&inc_by_one()));
        let call = Expr::Call("f".into(), vec![Expr::Decimal(1), Expr::Decimal(2)], None);
        let err = eval_expr(&call, &mut heap, &mut bindings, &HashMap::new()).err().unwrap().to_string();
        assert!(err.contains("arguments"), "got: {err}");
    }

    #[test]
    fn test_closure_call_unbound_name_errors() {
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        let call = Expr::Call("missing".into(), vec![], None);
        let err = eval_expr(&call, &mut heap, &mut bindings, &HashMap::new()).err().unwrap().to_string();
        assert!(err.contains("undefined function"), "got: {err}");
    }

    #[test]
    fn test_nested_closure_currying() {
        // (x) => (y) => x + y ; f(2)(3) = 5
        let inner = Expr::Lambda(
            vec!["y".into()],
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("y".into())),
            )),
        );
        let outer = Expr::Lambda(vec!["x".into()], Box::new(inner));
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("f".into(), eval1(&outer));
        let call1 = Expr::Call("f".into(), vec![Expr::Decimal(2)], None);
        let g = eval_expr(&call1, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert!(matches!(g, Value::Closure { .. }));
        bindings.insert("g".into(), g);
        let call2 = Expr::Call("g".into(), vec![Expr::Decimal(3)], None);
        assert_eq!(eval_expr(&call2, &mut heap, &mut bindings, &HashMap::new()).unwrap().as_i64(), Some(5));
    }

    #[test]
    fn test_closure_reentrant_applications() {
        // Applying the same closure twice with different args must not leak
        // state between applications.
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("f".into(), eval1(&inc_by_one()));
        let c1 = Expr::Call("f".into(), vec![Expr::Decimal(1)], None);
        assert_eq!(eval_expr(&c1, &mut heap, &mut bindings, &HashMap::new()).unwrap().as_i64(), Some(2));
        let c2 = Expr::Call("f".into(), vec![Expr::Decimal(100)], None);
        assert_eq!(eval_expr(&c2, &mut heap, &mut bindings, &HashMap::new()).unwrap().as_i64(), Some(101));
    }

    // 2026-08-06 (fix): user-defined functions apply through the registry.

    fn term_fn(name: &str, params: Vec<&str>, body: Expr) -> crate::interpreter::FunctionDef {
        crate::interpreter::FunctionDef {
            name: name.to_string(),
            parameters: params.into_iter().map(|p| p.to_string()).collect(),
            body: vec![Statement::Term(Some(body))],
        }
    }

    #[test]
    fn test_user_function_call_applies() {
        // `defn my_add(a, b) { term a + b; }` — a call applies it.
        let add = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Identifier("b".into())),
        );
        let mut functions = HashMap::new();
        functions.insert("my_add".into(), term_fn("my_add", vec!["a", "b"], add));
        let call = Expr::Call("my_add".into(), vec![Expr::Decimal(2), Expr::Decimal(3)], None);
        let r = eval_expr(&call, &mut VirtualHeap::new(), &mut HashMap::new(), &functions).unwrap();
        assert_eq!(r.as_i64(), Some(5));
    }

    #[test]
    fn test_user_function_reads_caller_state_dynamically() {
        // The defn body reads `k` from the CALLER's bindings (dynamic scoping),
        // not a captured snapshot.
        let body = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Identifier("k".into())),
        );
        let mut functions = HashMap::new();
        functions.insert("add_k".into(), term_fn("add_k", vec!["x"], body));
        let call = Expr::Call("add_k".into(), vec![Expr::Decimal(5)], None);
        let mut bindings = HashMap::new();
        bindings.insert("k".into(), Value::int(10));
        let r = eval_expr(&call, &mut VirtualHeap::new(), &mut bindings, &functions).unwrap();
        assert_eq!(r.as_i64(), Some(15));
    }

    #[test]
    fn test_user_function_arity_mismatch_errors() {
        let mut functions = HashMap::new();
        functions.insert("f".into(), term_fn("f", vec!["x"], Expr::Identifier("x".into())));
        let call = Expr::Call("f".into(), vec![Expr::Decimal(1), Expr::Decimal(2)], None);
        let err = eval_expr(&call, &mut VirtualHeap::new(), &mut HashMap::new(), &functions)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("arguments"), "got: {err}");
    }

    // 2026-08-06 (Slice F): slicing.

    fn slice_expr(array: Expr, start: Option<i64>, end: Option<i64>, stride: Option<i64>) -> Expr {
        let bound = |v: i64| Some(Box::new(Expr::Decimal(v)));
        Expr::Slice {
            array: Box::new(array),
            start: start.and_then(bound),
            end: end.and_then(bound),
            stride: stride.and_then(bound),
        }
    }

    fn abcdef() -> Expr {
        Expr::Quoted(b"abcdef".to_vec())
    }

    fn sliced_bytes(r: Value) -> Vec<u8> {
        match r {
            Value::Bits(b) => b,
            other => panic!("expected Bits, got {other:?}"),
        }
    }

    #[test]
    fn test_slice_string_range() {
        let r = eval1(&slice_expr(abcdef(), Some(1), Some(3), None));
        assert_eq!(sliced_bytes(r), b"bc".to_vec());
    }

    #[test]
    fn test_slice_string_open_bounds() {
        let r = eval1(&slice_expr(abcdef(), None, Some(3), None));
        assert_eq!(sliced_bytes(r), b"abc".to_vec());
        let r = eval1(&slice_expr(abcdef(), Some(3), None, None));
        assert_eq!(sliced_bytes(r), b"def".to_vec());
    }

    #[test]
    fn test_slice_string_positive_stride() {
        let r = eval1(&slice_expr(abcdef(), None, None, Some(2)));
        assert_eq!(sliced_bytes(r), b"ace".to_vec());
    }

    #[test]
    fn test_slice_string_reverse() {
        let r = eval1(&slice_expr(abcdef(), None, None, Some(-1)));
        assert_eq!(sliced_bytes(r), b"fedcba".to_vec());
    }

    #[test]
    fn test_slice_string_negative_index() {
        let r = eval1(&slice_expr(abcdef(), Some(-2), None, None));
        assert_eq!(sliced_bytes(r), b"ef".to_vec());
    }

    #[test]
    fn test_slice_string_bounds_clamp() {
        let r = eval1(&slice_expr(abcdef(), Some(0), Some(100), None));
        assert_eq!(sliced_bytes(r), b"abcdef".to_vec());
        let r = eval1(&slice_expr(abcdef(), Some(100), None, None));
        assert_eq!(sliced_bytes(r), b"".to_vec());
    }

    #[test]
    fn test_slice_list_product() {
        let l = Expr::List(vec![Expr::Decimal(10), Expr::Decimal(20), Expr::Decimal(30)]);
        let r = eval1(&slice_expr(l, Some(1), None, None));
        assert_eq!(r, Value::product(vec![Value::int(20), Value::int(30)]));
    }

    #[test]
    fn test_slice_stride_zero_errors() {
        let e = slice_expr(abcdef(), None, None, Some(0));
        let err = eval1_err(&e);
        assert!(err.contains("stride"), "got: {err}");
    }

    #[test]
    fn test_slice_non_sliceable_errors() {
        let e = slice_expr(Expr::Decimal(5), Some(0), Some(1), None);
        assert!(eval1_err(&e).contains("sliceable"));
    }

    // 2026-08-06 (Slice G): reflection value-side.

    #[test]
    fn test_reflect_len_on_product_is_field_count() {
        let r = Expr::Reflect(
            Box::new(person()),
            "Length".into(),
            ReflectKind::Runtime,
        );
        assert_eq!(eval1(&r).as_i64(), Some(2));
    }

    #[test]
    fn test_reflect_size_on_product_is_field_count() {
        let r = Expr::Reflect(
            Box::new(person()),
            "Size".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&r).as_i64(), Some(2));
    }

    #[test]
    fn test_reflect_size_vs_bytes_split() {
        // 2026-08-14 (§6a.1 item 8): `.^^Size` on a scalar is the shape (1),
        // `.^^Bytes` on a Bits value is the byte length — they were wrongly
        // grouped before.
        let size_r = Expr::Reflect(
            Box::new(Expr::Quoted(b"abc".to_vec())),
            "Size".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&size_r).as_i64(), Some(1), ".^^Size on a scalar is its shape (1)");
        let bytes_r = Expr::Reflect(
            Box::new(Expr::Quoted(b"abc".to_vec())),
            "Bytes".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&bytes_r).as_i64(), Some(3), ".^^Bytes on a Bits value is its byte length");
    }

    #[test]
    fn test_reflect_bytes_on_string_is_byte_length() {
        let r = Expr::Reflect(
            Box::new(Expr::Quoted(b"abc".to_vec())),
            "Bytes".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&r).as_i64(), Some(3));
    }

    #[test]
    fn test_reflect_element_on_string_is_char_category() {
        // 2026-08-14 (String unification): `s.^^Element` on a String value is
        // the Char category code (3) — a `String` operand iterates chars.
        let r = Expr::Reflect(
            Box::new(Expr::Quoted("hi".as_bytes().to_vec())),
            "Element".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&r).as_i64(), Some(3));
    }

    #[test]
    fn test_reflect_element_on_range_is_int_category() {
        // A Range iterates Int counters — `Element` folds to 0 (Int).
        let r = Expr::Reflect(
            Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(3)),
                inclusive: false,
            }),
            "Element".into(),
            ReflectKind::CompileTime,
        );
        assert_eq!(eval1(&r).as_i64(), Some(0));
    }

    #[test]
    fn test_reflect_absolute_is_removed() {
        // 2026-08-14 (boundary plan, SPEC §17.3): `.^Absolute` was removed —
        // abs is the `Abs#` intrinsic, not a reflection target. At the eval
        // level (no typechecker), the unhandled target returns Void.
        let r = Expr::Reflect(
            Box::new(Expr::Decimal(-7)),
            "Absolute".into(),
            ReflectKind::Runtime,
        );
        assert_eq!(eval1(&r), Value::Void);
    }

    #[test]
    fn test_reflect_type_on_sum_is_sum_category() {
        let r = Expr::Reflect(
            Box::new(Expr::Identifier("c".into())),
            "Type".into(),
            ReflectKind::CompileTime,
        );
        let mut heap = VirtualHeap::new();
        let mut bindings = HashMap::new();
        bindings.insert("c".into(), Value::sum("Some".into(), vec![Value::int(1)]));
        let out = eval_expr(&r, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(out.as_i64(), Some(6));
    }

    #[test]
    fn test_reflect_type_on_int_atom_is_category() {
        let r = Expr::Reflect(
            Box::new(Expr::Decimal(5)),
            "Type".into(),
            ReflectKind::CompileTime,
        );
        let out = eval1(&r);
        assert_eq!(out.as_i64(), Some(0));
    }

    #[test]
    fn test_reflect_unknown_target_is_void() {
        let r = Expr::Reflect(
            Box::new(Expr::Decimal(5)),
            "Bogus".into(),
            ReflectKind::Runtime,
        );
        assert_eq!(eval1(&r), Value::Void);
    }
}

// ── 2026-08-22 (spec-conformance plan Phase 5b): dyn dispatch ────────────

#[test]
fn dyn_member_call_dispatches_to_impl_with_self_receiver() {
    let src = r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
// SPEC §8.6: the ASSERTION on the type is what admits the coercion.
type Dog: Greeter { base: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
let g: dyn Greeter = Dog { base: 7 };
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut interp = crate::interpreter::Interpreter::new();
    interp.load_program(&items);

    // Execute the top-level lets in order to build the dyn binding.
    // Execute the top-level lets through the interpreter itself.
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interp.exec_stmt(stmt).unwrap();
        }
    }
    let g_val = interp.state.get("g").cloned().expect("g bound");
    match &g_val {
        Value::Dyn { concrete, trait_name, .. } => {
            assert_eq!(concrete, "Dog");
            assert_eq!(trait_name, "Greeter");
        }
        other => panic!("expected dyn value, got {:?}", describe_value(other)),
    }
    // g.greet(3): receiver threaded → 7*100+3 = 703. Run through the real
    // statement path so dispatch, coercion, and scoping are all exercised.
    let tokens2 = crate::lexer::tokenize(
        "let r: Int = g.greet(3);"
    ).unwrap();
    let mut p2 = crate::parser::Parser::new(tokens2, "let r: Int = g.greet(3);");
    let stmts = p2.parse_program().unwrap();
    for item in &stmts {
        if let TopLevel::Statement(st) = item {
            // Seed the interpreter's state with the dyn binding so the
            // statement sees `g`.
            interp.exec_stmt(st).unwrap();
        }
    }
    assert_eq!(interp.state.get("r").and_then(|v| v.as_i64()), Some(703));
}

// ── 2026-08-22 (spec-conformance Phase 7b): obj ports end-to-end ─────────

#[test]
fn spec_object_ports_example_runs() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/object_ports.bv"
    ))
    .unwrap();
    let tokens = crate::lexer::tokenize(&src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, &src);
    let items = p.parse_program().unwrap();
    let mut interp = crate::interpreter::Interpreter::new();
    interp.load_program(&items);

    // Drive the node body statement-by-statement with a persistent scope.
    let node_body: Vec<Statement> = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == "run" => Some(t.body.clone()),
            _ => None,
        })
        .expect("node run present");

    let mut bindings: HashMap<String, Value> = HashMap::new();
    let mut results: Vec<Value> = Vec::new();
    for stmt in &node_body {
        let _ = eval_statement(stmt, &mut interp.heap, &mut bindings, &interp.functions);
    }
    let first = bindings
        .get("first")
        .and_then(|v| v.as_i64())
        .expect("first hit bound");
    assert_eq!(first, 90, "first hit: 100 - damage.amount 10");
    let second = bindings
        .get("second")
        .and_then(|v| v.as_i64())
        .expect("second hit bound");
    assert_eq!(second, 80, "second hit sees mutated health");

    // The instance identity: died fired with the LAST health (80).
    let e = bindings.get("e").cloned().expect("instance bound");
    if let Value::Instance { fields, .. } = &e {
        let f = fields.borrow();
        let health = f.get("health").and_then(|v| v.as_i64());
        assert_eq!(health, Some(80), "slot write-back persists");
        if let Some(Value::EventQ(q)) = f.get("died") {
            let slot = q.borrow();
            assert!(slot.ready, "fired port is Ready");
            assert_eq!(
                slot.payload.as_ref().and_then(|p| p.as_i64()),
                Some(80),
                "last fired event observable through the shared slot"
            );
        } else {
            panic!("died port missing");
        }
        // Wiring shares storage: the input port still holds the Damage event.
        if let Some(Value::EventQ(q)) = f.get("damage") {
            let slot = q.borrow();
            assert!(slot.ready);
            match &slot.payload {
                Some(Value::Product { fields, names }) => {
                    let idx = names.as_ref().and_then(|ns| ns.iter().position(|n| n == "amount"));
                    assert_eq!(idx, Some(0));
                    assert_eq!(fields[0].as_i64(), Some(10));
                }
                other => panic!("damage payload shape {:?}", other.is_some()),
            }
        } else {
            panic!("damage port missing");
        }
    } else {
        panic!("expected instance");
    }
}

// ── 2026-08-26 (async Phase B): concurrent acceptance example ───────────

#[test]
fn async_phase_b_example_runs() {
    // docs/plans/2026-08-26-async-phase-b.md acceptance: the concurrent
    // program runs — one await interleaves BOTH tasks; the consumer's
    // blocked read is woken by the producer's port fire.
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/async-events.bv"
    ))
    .unwrap();
    let tokens = crate::lexer::tokenize(&src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, &src);
    let items = p.parse_program().unwrap();
    let mut interp = crate::interpreter::Interpreter::new();
    interp.load_program(&items);

    let node_body: Vec<Statement> = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == "run" => Some(t.body.clone()),
            _ => None,
        })
        .expect("node run present");

    let mut bindings: HashMap<String, Value> = HashMap::new();
    for stmt in &node_body {
        let _ = eval_statement(stmt, &mut interp.heap, &mut bindings, &interp.functions);
    }
    assert_eq!(
        bindings.get("v").and_then(|v| v.as_i64()),
        Some(7),
        "await(consume) sees the producer's fired payload"
    );
    assert_eq!(
        bindings.get("produced").and_then(|v| v.as_i64()),
        Some(1),
        "producer completed through the same scheduler"
    );
    // Both tasks reached Done; nothing left schedulable.
    let snapshot = crate::interpreter::task_table_snapshot().unwrap();
    assert!(
        snapshot.values().all(|e| e.status == crate::interpreter::TaskStatus::Done),
        "every spawned task finished: {snapshot:?}"
    );
}

// ── 2026-08-26 (async Phase D): compiled-twin interpreter parity ────────
// examples/async-events-compiled.bv is the SAME program with observable
// output (acc = consume(7) + produce(1)*10 = 17). The reference interpreter
// must derive the identical wake path on this file that the native target
// proves via tests/async_compiled_events_test.rs — one semantics, two
// backends (AGENTS golden rule 5).
#[test]
fn async_phase_d_compiled_twin_interpreter_parity() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/async-events-compiled.bv"
    ))
    .unwrap();
    let tokens = crate::lexer::tokenize(&src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, &src);
    let items = p.parse_program().unwrap();
    let mut interp = crate::interpreter::Interpreter::new();
    interp.load_program(&items);

    let node_body: Vec<Statement> = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == "run" => Some(t.body.clone()),
            _ => None,
        })
        .expect("node run present");

    // Seed the node-local binding map with the evaluated top-level
    // initializers (`i = 0`, `acc = 0`) — plain `x = ...` assigns bind into
    // the statement-level map, exactly as the reactor driver's per-firing
    // frame does.
    let mut bindings: HashMap<String, Value> = HashMap::new();
    for item in &items {
        if let TopLevel::Statement(s) = item {
            if let Statement::Let { name, expr: Some(e), .. } = s.as_ref() {
                if let Ok(v) = eval_expr(e, &mut interp.heap, &mut bindings, &interp.functions) {
                    bindings.insert(name.clone(), v);
                }
            }
        }
    }
    for stmt in &node_body {
        let _ = eval_statement(stmt, &mut interp.heap, &mut bindings, &interp.functions);
    }
    // acc is a top-level field: the node body's assignments land in
    // interpreter state, not local bindings. The plain driver adds nothing
    // further because i==1 makes every later firing fail the precondition.
    assert_eq!(
        bindings.get("acc").and_then(|v| v.as_i64()),
        Some(17),
        "reference derives 17 identically to the compiled binary"
    );
}

// ── 2026-08-26 (async Phase D): `^Ready` gate parity ────────────────────
// examples/async-ready-gate.bv observes the wake transition through top-level
// reflection. acc = !before(1) + after(10) = 11 on the reference too.
#[test]
fn async_phase_d_ready_gate_interpreter_parity() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/async-ready-gate.bv"
    ))
    .unwrap();
    let tokens = crate::lexer::tokenize(&src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, &src);
    let items = p.parse_program().unwrap();
    let mut interp = crate::interpreter::Interpreter::new();
    interp.load_program(&items);

    let node_body: Vec<Statement> = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == "run" => Some(t.body.clone()),
            _ => None,
        })
        .expect("node run present");

    let mut bindings: HashMap<String, Value> = HashMap::new();
    for item in &items {
        if let TopLevel::Statement(s) = item {
            if let Statement::Let { name, expr: Some(e), .. } = s.as_ref() {
                if let Ok(v) = eval_expr(e, &mut interp.heap, &mut bindings, &interp.functions) {
                    bindings.insert(name.clone(), v);
                }
            }
        }
    }
    for stmt in &node_body {
        let _ = eval_statement(stmt, &mut interp.heap, &mut bindings, &interp.functions);
    }
    assert_eq!(
        bindings.get("acc").and_then(|v| v.as_i64()),
        Some(11),
        "reference gate sees false→true identically"
    );
}
