// ── Interpreter — Value, VirtualHeap, Re-exports ───────────────────────
// 2026-07-12: Phase 3.0 — Bits-only Value, sandboxed VirtualHeap.
// Value::Bits(Vec<u8>) is the ONLY program-value variant for program data.
// Meta-objects (Defn, Expr, etc.) are compiler-internal and never
// reach user code.
//
// 2026-07-14: Re-added List/Enum/Instance/HashMap variants for FFI bridge
// code that marshals structured types between native libraries and the
// interpreter. These are FFI-only — no interpreter eval path produces them.
//
// 2026-07-28: Phase B.1 — Added function registry, load_program(),
// lookup_function(), and call_function() for derivation assertion
// verification. Functions are stored by name and called with raw Value
// arguments via a frame save/restore pattern.
//
// 2026-08-06: Phase 17 (Slice A) — semantic-value migration start. Fold the
// ad-hoc Int/Float/Bool/Char variants into the single `Atom` category and
// drop Enum/Instance/HashMap/Defn, which had no live users (their consumers
// were orphaned features/* modules and PropertyValue false positives). The
// remaining surface (Bits, Atom, Void, Ref, Constructor, List) is the first
// step toward the SPEC 2.2 generic model: atoms, bits, products, sums,
// references, closures, void. Undo: re-introduce per-kind variants only if a
// consumer needs the extra type safety; nothing outside interpreter/derive
// pattern-matches the atom category.

pub mod casts;
mod cells;
mod eval;
mod ffi;
mod intrinsics;

pub use cells::*;
pub use eval::*;
pub use ffi::*;
pub use intrinsics::*;

use crate::ast::*;
use std::collections::HashMap;
use std::sync::Arc;

/// FFI files import RuntimeError from crate::interpreter.
pub use crate::errors::RuntimeError;

/// 2026-07-28: Record for a parsed function definition stored in the
/// interpreter's function registry.
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub parameters: Vec<String>,
    pub body: Vec<Statement>,
}

/// 2026-08-22 (Phase 7b, SPEC §9.5): an obj's PORT/FIELD surface for the
/// interpreter's spawn constructor.
#[derive(Debug, Clone, Default)]
pub struct ObjShape {
    pub ports_in: Vec<(String, crate::ast::Type)>,
    pub ports_out: Vec<(String, crate::ast::Type)>,
    pub fields: Vec<(String, crate::ast::Type)>,
}

/// Compatibility struct bridging old Interpreter-based code to the new
/// function-based eval API. The old `Interpreter` had `state`, `prior_state`,
/// `heap`, `eval_expr()`, and `exec_stmt()`. This shim preserves that API
/// while delegating to the standalone `eval_expr`/`eval_statement` functions.
/// 2026-07-14: Added for reactor.rs and feature code compatibility.
///
/// 2026-07-28: Phase B.1 — Added `functions` registry for call_function().
#[derive(Debug, Clone)]
pub struct Interpreter {
    pub state: HashMap<String, Value>,
    pub prior_state: HashMap<String, Value>,
    pub heap: VirtualHeap,
    /// 2026-07-28: Phase B.1 — Function definitions loaded from the program.
    /// Keyed by function name, used by call_function() for assertion verification.
    pub functions: HashMap<String, FunctionDef>,
    /// 2026-08-09 (init kind, Phase 2): runtime-seeded invariant names. Set by
    /// `load_program` when it seeds each `TopLevel::Init`; reads resolve via
    /// `state`, and a later write to one is a `RuntimeError::ImmutableInit`.
    pub init_names: std::collections::HashSet<String>,
    /// 2026-08-22 (Phase 7b, SPEC §9.5): obj shapes for port-aware spawn —
    /// inputs bind at construction, outputs get fresh slots, plain fields
    /// default. Registered from TypeDef items carrying ports.
    pub objs: HashMap<String, ObjShape>,
    /// 2026-08-23 (enum-construction plan): variant_name → enum_name.
    /// A call naming a variant CONSTRUCTS a Sum value.
    pub variants: HashMap<String, String>,

    /// 2026-08-09 (Phase 10): `defer { ... }` cleanup stack — bodies pushed by
    /// `exec_stmt(Defer)` and flushed LIFO on term/rollback/endprogram. The
    /// reference semantics: cleanup runs exactly once per registered defer,
    /// even when the enclosing firing rolls back.
    pub defer_stack: Vec<Vec<Statement>>,

    /// 2026-08-23 (async scheduler Phase A1, SPEC §12.2): task table.
    /// Every `spawn defn(...)` registers an entry; the handle flows through
    /// await/free/keep which consume it linearly. In the eager reference
    /// model tasks run to completion at spawn — the table provides the
    /// bookkeeping that coroutine scheduling (Phase A2) will sit on.
    pub task_table: HashMap<u64, TaskEntry>,
    next_task_id: u64,
}

/// 2026-08-23 (async scheduler Phase A1): a single spawned task's lifecycle
/// record. Status transitions: Running → Done (normal) or Cancelled (free).
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub id: u64,
    pub fn_name: String,
    pub status: TaskStatus,
    /// Eager model: the result is set at spawn. Lazy model (Phase A2):
    /// result is None until await triggers execution.
    pub result: Option<Value>,
    /// 2026-08-23 (Phase A2): the captured call bindings for lazy execution.
    /// Set at spawn; consumed by the first `await`.
    pub pending_args: Option<Vec<Value>>,
    /// 2026-08-23 (Phase A3): body segments split at `yield;` checkpoints.
    /// Segment N runs on scheduling pass N. Empty for zero-yield tasks
    /// (single segment = whole body).
    pub segments: Vec<Vec<Statement>>,
    /// Which segment executes next.
    pub current_segment: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    /// Lazy model: spawned but not yet executed — awaiting triggers it.
    Ready,
    /// Phase A3: partially executed — yielded at a checkpoint, more segments remain.
    Yielded,
    /// 2026-08-26 (Phase B): blocked reading an unready event port. Invisible
    /// to `collect_runnable` until a fire on the awaited slot re-marks it
    /// `Ready` (level-triggered wake, SPEC §12.2).
    Waiting,
    /// Task ran to completion.
    Done,
    /// `free task` was called before execution — cancelled, never runs.
    Cancelled,
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            state: HashMap::new(),
            prior_state: HashMap::new(),
            heap: VirtualHeap::new(),
            functions: HashMap::new(),
            init_names: std::collections::HashSet::new(),
            objs: HashMap::new(),
            variants: HashMap::new(),
            defer_stack: Vec::new(),
            task_table: HashMap::new(),
            next_task_id: 0,
        }
    }

    /// 2026-07-28: Phase B.1 — Load all definitions and transactions from
    /// a parsed program into the function registry for call_function().
    pub fn load_program(&mut self, program: &[TopLevel]) {
        // 2026-08-22 (Phase 5b): impl bodies become callables under the
        // qualified key `Concrete::fn_name` for dyn member dispatch. Whether
        // the receiver threads as the first argument is decided AT THE CALL
        // by arity (params == args+1 ⇒ receiver-first), matching the trait's
        // Self-first convention checked statically.
        // 2026-08-22 (Phase 7b, SPEC §9.5): register OBJ SHAPES (ports +
        // fields) and their member functions under `Type::member` so spawn
        // constructs instances and method calls dispatch to bodies.
        let mut registered_objs: HashMap<String, ObjShape> = HashMap::new();
        for item in program {
            if let TopLevel::TypeDef(td) = item {
                if td.ports_in.is_empty() && td.ports_out.is_empty() && td.body.members.is_empty()
                {
                    continue;
                }
                let shape = ObjShape {
                    ports_in: td.ports_in.clone(),
                    ports_out: td.ports_out.clone(),
                    fields: td.body.slots.iter().map(|s| (s.name.clone(), s.ty.clone())).collect(),
                };
                registered_objs.insert(td.name.clone(), shape.clone());
                self.objs.insert(td.name.clone(), shape);
                for m in &td.body.members {
                    match m {
                        TopLevel::Definition(d) => {
                            let key = format!("{}::{}", td.name, d.name);
                            let params: Vec<String> =
                                d.parameters.iter().map(|(n, _)| n.clone()).collect();
                            self.functions.insert(key, FunctionDef {
                                name: d.name.clone(),
                                parameters: params,
                                body: d.body.clone(),
                            });
                        }
                        TopLevel::Transaction(t) => {
                            let key = format!("{}::{}", td.name, t.name);
                            let params: Vec<String> =
                                t.parameters.iter().map(|(n, _)| n.clone()).collect();
                            self.functions.insert(key, FunctionDef {
                                name: t.name.clone(),
                                parameters: params,
                                body: t.body.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
        // 2026-08-26 (Phase B2, plan 2026-08-26-async-phase-b2): CELLS
        // construct like objs — same shape registry (spawn's existing port
        // wiring path applies verbatim) and same `{Cell}::{member}` function
        // keys method dispatch resolves. Sealing stays compile-time
        // (typechecker cell_ports); the reference serves any declared field.
        for item in program {
            if let TopLevel::Cell(c) = item {
                let shape = ObjShape {
                    ports_in: c.ports_in.clone(),
                    ports_out: c.ports_out.clone(),
                    fields: c.fields.clone(),
                };
                registered_objs.insert(c.name.clone(), shape.clone());
                self.objs.insert(c.name.clone(), shape);
                for t in &c.transactions {
                    let key = format!("{}::{}", c.name, t.name);
                    let params: Vec<String> =
                        t.parameters.iter().map(|(n, _)| n.clone()).collect();
                    self.functions.insert(key, FunctionDef {
                        name: t.name.clone(),
                        parameters: params,
                        body: t.body.clone(),
                    });
                }
                for d in &c.definitions {
                    let key = format!("{}::{}", c.name, d.name);
                    let params: Vec<String> =
                        d.parameters.iter().map(|(n, _)| n.clone()).collect();
                    self.functions.insert(key, FunctionDef {
                        name: d.name.clone(),
                        parameters: params,
                        body: d.body.clone(),
                    });
                }
            }
        }
        // 2026-08-23 (enum construction): variant registry for constructors.
        // 2026-08-26 (qualified enum paths): BOTH bare and `Enum::Variant`
        // keys register; construction stores the BARE name in the Sum so
        // patterns normalize on last segment only.
        let mut registered_variants: HashMap<String, String> = HashMap::new();
        for item in program {
            if let TopLevel::TypeDef(td) = item {
                for slot in &td.body.slots {
                    if let Some(vname) = slot.name.strip_prefix("__variant_") {
                        registered_variants.insert(vname.to_string(), td.name.clone());
                        registered_variants.insert(
                            format!("{}::{}", td.name, vname),
                            td.name.clone(),
                        );
                    }
                }
            }
        }
        self.variants = registered_variants.clone();
        VARIANT_DEFS.with(|c| *c.borrow_mut() = Some(registered_variants));
        // 2026-08-23 (async A1): fresh task table per program.
        self.task_table.clear();
        self.next_task_id = 0;
        TASK_TABLE.with(|t| *t.borrow_mut() = Some(HashMap::new()));

        OBJ_SHAPES.with(|c| *c.borrow_mut() = Some(registered_objs));

        for item in program {
            if let TopLevel::Impl(i) = item {
                for d in &i.functions {
                    let key = format!("{}::{}", i.target, d.name);
                    let param_names = d.parameters.iter().map(|(n, _)| n.clone()).collect();
                    self.functions.insert(key, FunctionDef {
                        name: d.name.clone(),
                        parameters: param_names,
                        body: d.body.clone(),
                    });
                }
            }
        }
        for item in program {
            match item {
                TopLevel::Definition(d) => {
                    let param_names = d.parameters.iter().map(|(n, _)| n.clone()).collect();
                    self.functions.insert(d.name.clone(), FunctionDef {
                        name: d.name.clone(),
                        parameters: param_names,
                        body: d.body.clone(),
                    });
                }
                TopLevel::Transaction(t) => {
                    let param_names = t.parameters.iter().map(|(n, _)| n.clone()).collect();
                    self.functions.insert(t.name.clone(), FunctionDef {
                        name: t.name.clone(),
                        parameters: param_names,
                        body: t.body.clone(),
                    });
                }
                _ => {}
            }
        }
        // 2026-08-09 (init kind, Phase 2): seed each init exactly once, before
        // any transaction runs. A seeding failure (or a duplicate init) is a
        // runtime error — the interpreter is the reference codegen must match.
        let _ = self.seed_inits(program);
    }

    /// 2026-08-09 (init kind, Phase 2): evaluate each `TopLevel::Init` seeding
    /// (value expr, or body statements once) into `state` and mark the name in
    /// `init_names`. Re-seeding an already-seeded init is
    /// `RuntimeError::ImmutableInit`. Reads of the seeded value then resolve
    /// through the normal identifier path.
    pub fn seed_inits(&mut self, program: &[TopLevel]) -> Result<(), RuntimeError> {
        for item in program {
            if let TopLevel::Init(init) = item {
                self.seed_init(init)?;
            }
        }
        Ok(())
    }

    /// Seed one `TopLevel::Init` — value form evaluates its expr; body form
    /// runs its statements once (a `term <expr>` yields the value; a bare
    /// `term;` is a convergence checkpoint leaving Void). Re-seeding a seeded
    /// init is `RuntimeError::ImmutableInit`.
    fn seed_init(&mut self, init: &crate::ast::top::InitDecl) -> Result<(), RuntimeError> {
        if self.init_names.contains(&init.name) || self.state.contains_key(&init.name) {
            return Err(RuntimeError::ImmutableInit(init.name.clone()));
        }
        if let Some(value) = &init.value {
            let v = self.eval_expr(value)?;
            self.state.insert(init.name.clone(), v);
        } else {
            let seeded = self.seed_body(&init.body)?;
            self.state.insert(init.name.clone(), seeded);
        }
        self.init_names.insert(init.name.clone());
        Ok(())
    }

    /// Run a body-form init's statements once and return the seeded value.
    fn seed_body(&mut self, body: &[Statement]) -> Result<Value, RuntimeError> {
        let mut seeded = Value::Void;
        for stmt in body {
            match self.exec_stmt(stmt) {
                Ok(v) => seeded = v,
                Err(RuntimeError::TermReturn(v)) => return Ok(v),
                Err(e) => return Err(e),
            }
        }
        Ok(seeded)
    }

    /// 2026-07-28: Phase B.1 — Look up a function definition by name.
    pub fn lookup_function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    /// 2026-07-28: Phase B.1 — Call a function by name with pre-evaluated
    /// argument values. Binds parameters, executes body, returns result.
    /// Saves and restores any state keys that overlap with parameter names.
    pub fn call_function(&mut self, name: &str, args: &[Value]) -> Result<Value, RuntimeError> {
        let defn = self.lookup_function(name)
            .cloned()
            .ok_or_else(|| RuntimeError::UndefinedFunction(name.to_string()))?;

        if defn.parameters.len() != args.len() {
            return Err(RuntimeError::TypeError {
                expected: format!("{} arguments", defn.parameters.len()),
                found: format!("{} arguments", args.len()),
            });
        }

        // Save any existing state keys that overlap with parameter names
        let mut saved: HashMap<String, Option<Value>> = HashMap::new();
        for param in &defn.parameters {
            saved.insert(param.clone(), self.state.get(param).cloned());
        }

        // Bind parameters
        for (i, name) in defn.parameters.iter().enumerate() {
            self.state.insert(name.clone(), args[i].clone());
        }

        // Execute body statements
        // 2026-07-28: Term with value signals early return via TermReturn error.
        // 2026-08-09 (Phase 10): use exec_stmt so `defer` registers cleanup;
        // flush the defer stack (LIFO) on normal termination and on term.
        let mut result = Value::Void;
        for stmt in &defn.body {
            match self.exec_stmt(stmt) {
                Ok(v) => result = v,
                Err(RuntimeError::TermReturn(v)) => {
                    result = v;
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        // 2026-08-09 (Phase 10): deferred cleanup runs on EVERY exit — normal
        // fall-through AND term (the firing completed either way).
        let _ = self.flush_defers();

        // Restore saved state
        for (name, saved_val) in saved {
            match saved_val {
                Some(val) => { self.state.insert(name, val); }
                None => { self.state.remove(&name); }
            }
        }

        Ok(result)
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        eval_expr(expr, &mut self.heap, &mut self.state, &self.functions)
    }

    pub fn exec_stmt(&mut self, stmt: &Statement) -> Result<Value, RuntimeError> {
        // 2026-08-09 (init kind, Phase 2): an `init` is seeded once and
        // immutable — a write to one at runtime is a reference violation
        // (the typechecker rejects it statically; this is the defensive
        // runtime half, matching the backend's seeded-global store).
        if let Some(name) = statement_write_target(stmt) {
            if self.init_names.contains(name) {
                return Err(RuntimeError::ImmutableInit(name.to_string()));
            }
        }
        // 2026-08-09 (Phase 10): `defer { ... }` registers cleanup — push the
        // body onto the defer stack (flushed LIFO on term/rollback/endprogram).
        if let Statement::Defer(body) = stmt {
            self.defer_stack.push(body.clone());
            return Ok(Value::Void);
        }
        eval_statement(stmt, &mut self.heap, &mut self.state, &self.functions)
    }

    /// 2026-08-09 (Phase 10): run all registered `defer` cleanups LIFO. Called
    /// on term/rollback/endprogram; the stack is drained (each defer runs once).
    pub fn flush_defers(&mut self) -> Result<(), RuntimeError> {
        while let Some(body) = self.defer_stack.pop() {
            for stmt in &body {
                self.exec_stmt(stmt)?;
            }
        }
        Ok(())
    }

    /// Pattern match a value against a pattern. Delegates to the full
    /// implementation in eval.rs (used by eval_match and kept here for
    /// API compatibility).
    pub fn pattern_match(pat: &crate::ast::Pattern, val: &Value, bindings: &mut HashMap<String, Value>) -> bool {
        crate::interpreter::pattern_match(pat, val, bindings)
    }

    /// Compute the nesting depth of a product value (for FFI marshalling).
    pub fn list_nesting_depth(val: &Value) -> usize {
        match val {
            Value::Product { fields, .. } => {
                fields.iter().map(|v| Self::list_nesting_depth(v)).max().unwrap_or(0) + 1
            }
            _ => 0,
        }
    }

    /// Expand coordinate dimensions into a flat list of indices (for FFI).
    pub fn expand_coordinates(coords: &[Expr], dims: usize) -> Result<Vec<Value>, RuntimeError> {
        let mut result = Vec::new();
        for coord in coords {
            match coord {
                Expr::Decimal(n) => result.push(Value::int(*n)),
                _ => return Err(RuntimeError::TypeError { expected: "integer".to_string(), found: format!("{:?}", coord) }),
            }
        }
        Ok(result)
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// 2026-08-09 (init kind, Phase 2): the top-level name a statement writes, if
/// it is a plain identifier assignment/arrow write. Used by exec_stmt to
/// reject writes to seeded `init` names (the typechecker rejects them
/// statically; this is the defensive runtime half).
fn statement_write_target<'a>(stmt: &'a Statement) -> Option<&'a str> {
    match stmt {
        Statement::Assign(lhs, _) => match lhs {
            Expr::Identifier(name) => Some(name.as_str()),
            _ => None,
        },
        Statement::ArrowAssign { target, .. } => match target.as_ref() {
            Some(t) => match t.as_ref() {
                Expr::Identifier(name) => Some(name.as_str()),
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

/// Signature for foreign function implementations registered in the FFI registry.
pub type ForeignFn = fn(Vec<Value>) -> Result<Value, RuntimeError>;

/// A primitive atom value. Grouped under `Value::Atom` (SPEC §2.2 "optimized
/// primitive atoms") so the eval path can dispatch on one category while the
/// atom kind stays first-class — `as_i64`/`as_f64`/`as_bool` promote every
/// atom C-style (Bool→0|1, Char→code point), matching the backend's
/// protocol-category dispatch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Atom {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
}

/// The representational value in the Briev interpreter.
///
/// 2026-08-06 (Phase 17, Slice A): `Enum`, `Instance`, `HashMap`, and `Defn`
/// were dropped — they had no live producers/consumers (only orphaned
/// `features/*` and `PropertyValue` false positives). `Int`/`Float`/`Bool`/
/// `Char` now live under `Atom`. `List` survives only for reactor state
/// collections and is slated for replacement by a product in a later slice;
/// `Constructor` is the derive (CEGIS) engine's synthesis shape.
///
/// 2026-08-06 (Slice B): `Product` added as the product value (tuples, list
/// literals, struct fields in declared order). SPEC §2.2 — stdlib list/map
/// behavior is NOT interpreter knowledge; the interpreter holds the field
/// sequence, stdlib owns the semantics.
///
/// 2026-08-22 (Phase 7b): the one-slot event storage behind an
/// `EventQ` value (SPEC §9.5 `.Ready` + payload semantics).
#[derive(Debug, Clone)]
pub struct EventSlot {
    pub ready: bool,
    pub payload: Option<Value>,
    /// 2026-08-26 (async Phase B): task ids blocked reading this slot's
    /// payload while unready. Drained (and each waiter re-marked `Ready`)
    /// when a producer fires the port — SPEC §9.5/§12.2 wake semantics.
    /// TEMP undo: drop the field + `fire_slot_wake` to return to Phase A
    /// strict-error reads.
    pub waiters: Vec<u64>,
}

/// 2026-08-06 (Slice D): struct literals produce a product carrying its
/// declared field names (`names: Some(...)`); field access resolves the
/// index from that map. Tuples and list literals stay unnamed (`names: None`).
#[derive(Debug, Clone)]
pub enum Value {
    /// The sole representational storage cell for opaque program data.
    Bits(Vec<u8>),

    /// 2026-07-14: Typed value variants for generic op dispatch.
    /// These allow the interpreter to distinguish int from float values
    /// when both would be valid as raw Bits(Vec<u8>).
    Atom(Atom),

    /// A product: field sequence in declared order. `names` is present for
    /// struct literals (drives `Expr::Field`), absent for tuples/list
    /// literals (positional only). The interpreter is dynamically typed, so
    /// the name→index map is carried with the value rather than resolved from
    /// type context (which eval has none of).
    Product {
        fields: Vec<Value>,
        names: Option<std::sync::Arc<Vec<String>>>,
    },

    /// 2026-08-22 (Phase 5b): a trait object — `let g: dyn Greeter = d`.
    /// Carries the trait AND the concrete type name so member calls can find
    /// the impl body (`Concrete::fn`) and thread the receiver when the
    /// trait function's first parameter is `Self`.
    Dyn {
        trait_name: String,
        concrete: String,
        inner: Box<Value>,
    },

    /// 2026-08-22 (Phase 7b, SPEC §9.5): an EVENT PORT slot — one event
    /// deep. Shared behind Rc<RefCell> so a producer instance and every
    /// consumer wiring see the SAME storage: `.Ready` reads the flag,
    /// payload projection reads the current event, firing replaces both.
    EventQ(std::rc::Rc<std::cell::RefCell<EventSlot>>),

    /// 2026-08-22 (Phase 7b, SPEC §9.5): an object INSTANCE — named fields
    /// (slots AND ports) behind shared storage so member calls mutate the
    /// same identity the caller holds.
    Instance {
        type_name: String,
        fields: std::rc::Rc<std::cell::RefCell<HashMap<String, Value>>>,
    },

    Void,
    Ref(Box<Value>),

    /// 2026-08-06 (Slice E): a closure — lambda params, body, and the captured
    /// environment snapshot at creation. Application (Expr::Call on a closure
    /// binding) binds params into a fresh env seeded from the captured one;
    /// mutations never leak to the captured env or the caller (re-entrant).
    Closure {
        params: Arc<Vec<String>>,
        body: Arc<Expr>,
        env: Arc<HashMap<String, Value>>,
    },

    // ── Compound types (synthesis engine) ────────────────────────────
    /// 2026-08-06 (Slice H): a sum — an enum variant with payload, produced by
    /// the derive (CEGIS) engine's `evaluate_synthesized` for `Call`/variant
    /// expressions. The variant NAME travels with the value (SPEC §2.2
    /// "sums"): the interpreter has no separate type registry, so name-based
    /// pattern matching (Pattern::EnumVariant) and `Type` reflection resolve
    /// from the value itself. The plan sketched a tag-index + global table;
    /// carrying the name avoids global mutable tag state and keeps derive's
    /// name-based variant dispatch working unchanged. Undo: if a shared type
    /// registry ever reaches the interpreter, replace the name with a tag.
    /// Renamed from `Constructor` (the synthesis-tree name was misleading —
    /// the value is a tagged compound, not a constructor expression).
    Sum { name: String, payload: Vec<Value> },

    /// 2026-08-07 (Phase 7): an iterable range — `start..end` (half-open) or
    /// `start..=end` (inclusive). Produced by `Expr::Range`; consumed by
    /// `foreach` (SPEC §11.4 counted iteration).
    Range { start: i64, end: i64, inclusive: bool },
}

/// 2026-08-22 (Phase 7b): manual equality — shared-storage variants
/// (EventQ, Instance) compare by IDENTITY (same underlying slot), the way
/// wiring semantics demand; everything else compares structurally.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::EventQ(a), Value::EventQ(b)) => std::rc::Rc::ptr_eq(a, b),
            (
                Value::Instance { type_name: ta, fields: fa },
                Value::Instance { type_name: tb, fields: fb },
            ) => ta == tb && std::rc::Rc::ptr_eq(fa, fb),
            _ => match std::mem::discriminant(self) == std::mem::discriminant(other) {
                true => self.debug_eq(other),
                false => false,
            },
        }
    }
}

thread_local! {
    /// 2026-08-23 (enum construction): active variant registry mirror.
    static VARIANT_DEFS: std::cell::RefCell<Option<HashMap<String, String>>> =
        const { std::cell::RefCell::new(None) };
    /// 2026-08-23 (async scheduler A1): active task table mirror.
    static TASK_TABLE: std::cell::RefCell<Option<HashMap<u64, TaskEntry>>> =
        const { std::cell::RefCell::new(None) };
    /// 2026-08-22 (Phase 7b): process-local mirror of the ACTIVE
    /// interpreter's obj shapes, read by the spawn constructor deep inside
    /// expression evaluation where no scope carries them. The interpreter
    /// is single-threaded and one program is active per thread — the last
    /// `load_program` wins.
    static OBJ_SHAPES: std::cell::RefCell<Option<HashMap<String, ObjShape>>> =
        const { std::cell::RefCell::new(None) };
    /// 2026-08-26 (async Phase B): the task id whose segment is executing,
    /// or `u64::MAX` outside any task. Read deep inside Field-on-EventQ to
    /// decide block-in-task vs strict-error-at-top-level. Same thread-local
    /// justification as TASK_TABLE/OBJ_SHAPES.
    static CURRENT_TASK: std::cell::Cell<u64> = const { std::cell::Cell::new(u64::MAX) };
}

/// 2026-08-26 (async Phase B): mark which task's segment is running.
/// `None` restores top-level context (strict unready-read errors).
pub fn set_current_task(id: Option<u64>) {
    CURRENT_TASK.with(|c| c.set(id.unwrap_or(u64::MAX)));
}

/// 2026-08-26 (async Phase B): the running task, if any.
pub fn current_task_id() -> Option<u64> {
    let id = CURRENT_TASK.with(|c| c.get());
    if id == u64::MAX { None } else { Some(id) }
}

/// Active variant registry for constructor calls (Phase 7b/enum plan).
pub fn variant_defs() -> Option<HashMap<String, String>> {
    VARIANT_DEFS.with(|c| c.borrow().clone())
}

/// 2026-08-23 (async Phase A2): register a PENDING task — spawned but not
/// yet executed. The first `await` triggers `execute_and_consume`.
/// 2026-08-26 (Phase B): `param_names` drives the port-read pre-split —
pub fn register_pending_task(
    id: u64,
    fn_name: String,
    args: Vec<Value>,
    body: Vec<Statement>,
    param_names: &[String],
) {
    // 2026-08-26 (Phase C): the splitter is SHARED with the LLVM backend's
    // frontend pass (analysis::task_segments) - one segmentation, both
    // consumers, parity by construction. Split rules live there.
    let segments = crate::analysis::task_segments::split_task_body(&body, param_names);
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            table.insert(id, TaskEntry {
                id,
                fn_name,
                status: TaskStatus::Ready,
                result: None,
                pending_args: Some(args),
                segments,
                current_segment: 0,
            });
        }
    });
}

/// 2026-08-23 (async Phase A2): look up a pending task and return its
/// captured args for execution. Marks the task as Done with the result.
pub fn take_pending_task(id: u64) -> Option<(String, Vec<Value>)> {
    // 2026-08-23 (Phase A3): do NOT change status here — execute_pending_task
    // needs the segments via take_task_segments, which requires Ready/Yielded.
    // Status transitions to Done only after ALL segments complete (mark_done).
    TASK_TABLE.with(|t| {
        let mut table_ref = t.borrow_mut();
        let table = table_ref.as_mut()?;
        let entry = table.get_mut(&id)?;
        if entry.status == TaskStatus::Ready || entry.status == TaskStatus::Yielded {
            if let Some(args) = entry.pending_args.take() {
                return Some((entry.fn_name.clone(), args));
            }
        }
        None
    })
}

/// 2026-08-23 (async Phase A2): store the result after lazy execution.
pub fn complete_task(id: u64, result: Value) {
    TASK_TABLE.with(|t| {
        let mut table_ref = t.borrow_mut();
        if let Some(table) = table_ref.as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                entry.result = Some(result);
            }
        }
    });
}

/// Snapshot of the active obj shapes for spawn construction (Phase 7b).
pub fn obj_shapes() -> Option<HashMap<String, ObjShape>> {
    OBJ_SHAPES.with(|c| c.borrow().clone())
}

impl Value {
    /// Structural fallback used by PartialEq for same-variant pairs without
    /// shared storage. Kept coarse: exact structural equality for the
    /// data-bearing shapes the reactor compares in practice.
    fn debug_eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Bits(a), Bits(b)) => a == b,
            (Atom(a), Atom(b)) => a == b,
            (
                Product { fields: fa, names: na },
                Product { fields: fb, names: nb },
            ) => fa == fb && na == nb,
            (Void, Void) => true,
            (Ref(a), Ref(b)) => a == b,
            (Sum { name: na, payload: pa }, Sum { name: nb, payload: pb }) => {
                na == nb && pa == pb
            }
            (
                Range { start: sa, end: ea, inclusive: ia },
                Range { start: sb, end: eb, inclusive: ib },
            ) => sa == sb && ea == eb && ia == ib,
            (Dyn { trait_name: ta, concrete: ca, inner: ia }, Dyn { trait_name: tb, concrete: cb, inner: ib }) => {
                ta == tb && ca == cb && ia == ib
            }
            _ => format!("{:?}", self) == format!("{:?}", other),
        }
    }
}

// 2026-08-06 (Slice I): the last ad-hoc variant (`List`) is dropped — no
// producer remained (eval yields Product; FFI marshals through
// ffi::marshal_value). The semantic model is now atoms, bits, products,
// sums, references, closures, void (SPEC §2.2).
//
// 2026-07-14: Re-added List/Enum/Instance/HashMap variants for FFI bridge
// code that marshals structured types between native libraries and the
// interpreter. These are FFI-only — no interpreter eval path produces
// them. (All dropped by 2026-08-06 Slice A/I; FFI marshalling lives in
// ffi.rs.)

impl Value {
    pub fn int(n: i64) -> Self {
        Value::Atom(Atom::Int(n))
    }

    pub fn float(f: f64) -> Self {
        Value::Atom(Atom::Float(f))
    }

    pub fn bool(b: bool) -> Self {
        Value::Atom(Atom::Bool(b))
    }

    pub fn char(c: char) -> Self {
        Value::Atom(Atom::Char(c))
    }

    pub fn bits(data: Vec<u8>) -> Self {
        Value::Bits(data)
    }

    pub fn product(fields: Vec<Value>) -> Self {
        Value::Product { fields, names: None }
    }

    pub fn named_product(fields: Vec<Value>, names: Vec<String>) -> Self {
        Value::Product {
            fields,
            names: Some(std::sync::Arc::new(names)),
        }
    }

    pub fn sum(name: String, payload: Vec<Value>) -> Self {
        Value::Sum { name, payload }
    }

    pub fn void() -> Self {
        Value::Void
    }

    /// Extract first 8 bytes as little-endian i64.
    // 2026-07-14: Zero-pad buffers shorter than 8 bytes so small bit
    // values (zero_bits(4), zext of 1-byte bool) convert correctly.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Atom(Atom::Int(n)) => Some(*n),
            Value::Atom(Atom::Bool(b)) => Some(if *b { 1 } else { 0 }),
            Value::Atom(Atom::Char(c)) => Some(*c as i64),
            Value::Bits(bytes) => {
                let mut arr = [0u8; 8];
                let copy_len = bytes.len().min(8);
                arr[..copy_len].copy_from_slice(&bytes[..copy_len]);
                Some(i64::from_le_bytes(arr))
            }
            _ => None,
        }
    }

    /// Extract first 8 bytes as f64.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Atom(Atom::Float(f)) => Some(*f),
            Value::Atom(Atom::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Atom(Atom::Char(c)) => Some(*c as i64 as f64),
            Value::Bits(bytes) if bytes.len() >= 8 => {
                let arr: [u8; 8] = bytes[..8].try_into().ok()?;
                Some(f64::from_le_bytes(arr))
            }
            _ => None,
        }
    }

    /// Extract first byte as bool.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Atom(Atom::Bool(b)) => Some(*b),
            Value::Atom(Atom::Char(c)) => Some(*c != '\0'),
            Value::Bits(bytes) => bytes.first().map(|b| *b != 0),
            _ => None,
        }
    }

    /// Check if the value is truthy (any non-zero byte).
    pub fn is_true(&self) -> bool {
        self.as_bool().unwrap_or(false)
    }

    /// Dereference this value as a Briev String's content bytes.
    ///
    /// 2026-08-01 (B1): A Briev String is either raw bytes (`Value::Bits`,
    /// produced by literals and the get_env! macro) or a heap handle
    /// (`Value::Atom(Atom::Int(addr))` pointing at `[len: i64][payload]`,
    /// produced by FFI marshalling like EnvGet#). This is the single deref
    /// used by content equality (Eq/Ne) and any op that needs the string
    /// payload. Returns `None` when the value is not a String (so numeric
    /// comparisons fall through to their own paths). Undo: if the heap-handle
    /// encoding is ever removed, drop the `Atom::Int` arm and keep the Bits arm.
    pub fn string_bytes(&self, heap: &VirtualHeap) -> Option<Vec<u8>> {
        match self {
            Value::Bits(bytes) => Some(bytes.clone()),
            Value::Atom(Atom::Int(addr)) if *addr > 0 => {
                // 2026-08-01 (B1): i64_to_bits(n) produces an Int atom, so an
                // Int atom is ambiguous between a real int and a heap string
                // handle. Only deref as a heap string when the address is an
                // actual heap allocation with a readable [len: i64] header —
                // otherwise return None so numeric comparisons fall through.
                let len = heap.read(*addr as u64, 8)?.to_vec();
                let len = i64::from_le_bytes(len.try_into().unwrap_or([0u8; 8]));
                if len > 0 {
                    heap.read(*addr as u64 + 8, len as usize).map(|p| p.to_vec())
                } else {
                    Some(Vec::new())
                }
            }
            _ => None,
        }
    }
}

/// Convert i64 to an Int atom value.
pub fn i64_to_bits(n: i64) -> Value {
    Value::int(n)
}

/// Convert f64 to a Float atom value.
pub fn f64_to_bits(f: f64) -> Value {
    Value::float(f)
}

/// Convert bool to a Bool atom value.
pub fn bool_to_bits(b: bool) -> Value {
    Value::bool(b)
}

/// Create a zero-filled Bits value of the given byte size.
pub fn zero_bits(size: usize) -> Value {
    Value::Bits(vec![0u8; size])
}

/// 2026-07-28: Phase B.0 — Compare two values within relative tolerance.
/// Used by derivation assertion verification for FP relaxed equivalence.
/// For Float values, checks relative error: |a - e| / max(|e|, 1e-10) <= tol.
/// Non-float values must match exactly.
pub fn values_within_tolerance(actual: &Value, expected: &Value, tol: f64) -> bool {
    match (actual, expected) {
        (Value::Atom(Atom::Float(a)), Value::Atom(Atom::Float(e))) => {
            let diff = (a - e).abs();
            let mag = e.abs().max(1e-10);
            diff / mag <= tol
        }
        _ => actual == expected,
    }
}

/// Sandboxed compile-time heap for pointer arithmetic and allocation.
/// 2026-07-12: Phase 3.0 — Bounds-checked read/write.
#[derive(Debug, Clone)]
pub struct VirtualHeap {
    allocations: HashMap<u64, Vec<u8>>,
    next_address: u64,
}

impl VirtualHeap {
    pub fn new() -> Self {
        VirtualHeap {
            allocations: HashMap::new(),
            next_address: 0x1000, // start at page-aligned address
        }
    }

    /// Allocate a block of the given size. Returns virtual address.
    /// The allocation is zero-filled.
    pub fn allocate(&mut self, size: usize) -> u64 {
        let addr = self.next_address;
        self.allocations.insert(addr, vec![0u8; size]);
        self.next_address += size as u64 + 16; // small gap between allocations
        addr
    }

    /// Read bytes from the given address. Returns None if address not found
    /// or the read would go out of bounds.
    pub fn read(&self, addr: u64, size: usize) -> Option<&[u8]> {
        let (base, data) = self.find_block(addr)?;
        let offset = (addr - base) as usize;
        if offset + size > data.len() {
            return None;
        }
        Some(&data[offset..offset + size])
    }

    /// Write bytes at the given address. Returns error if address not found
    /// or the write would go out of bounds.
    pub fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), String> {
        let (base, block) = self
            .find_block_mut(addr)
            .ok_or("heap: address not allocated")?;
        let offset = (addr - base) as usize;
        if offset + data.len() > block.len() {
            return Err("heap: write out of bounds".into());
        }
        block[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Free a previously allocated block. Returns error if not found.
    pub fn free(&mut self, addr: u64) -> Result<(), String> {
        // Find the block that contains this address
        let base = self
            .allocations
            .keys()
            .filter(|k| **k <= addr)
            .max()
            .copied()
            .ok_or("heap: address not found")?;
        self.allocations
            .remove(&base)
            .ok_or("heap: address not found")?;
        Ok(())
    }

    /// Check if an address is allocated.
    pub fn contains(&self, addr: u64) -> bool {
        self.allocations.keys().any(|k| *k <= addr)
    }

    /// Find the block base and data for an address (read).
    fn find_block(&self, addr: u64) -> Option<(u64, &Vec<u8>)> {
        self.allocations
            .iter()
            .filter(|(k, v)| **k <= addr && addr < **k + v.len() as u64)
            .map(|(k, v)| (*k, v))
            .next()
    }

    /// Find the block base and data for an address (write).
    fn find_block_mut(&mut self, addr: u64) -> Option<(u64, &mut Vec<u8>)> {
        for (k, v) in &mut self.allocations {
            if *k <= addr && addr < *k + v.len() as u64 {
                return Some((*k, v));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atom_constructors() {
        assert_eq!(Value::int(42), Value::Atom(Atom::Int(42)));
        assert_eq!(Value::float(1.5), Value::Atom(Atom::Float(1.5)));
        assert_eq!(Value::bool(true), Value::Atom(Atom::Bool(true)));
        assert_eq!(Value::char('x'), Value::Atom(Atom::Char('x')));
    }

    #[test]
    fn test_atom_promotion_c_style() {
        assert_eq!(Value::bool(true).as_i64(), Some(1));
        assert_eq!(Value::bool(false).as_i64(), Some(0));
        assert_eq!(Value::char('A').as_i64(), Some(65));
        assert_eq!(Value::bool(true).as_f64(), Some(1.0));
        assert_eq!(Value::char('A').as_bool(), Some(true));
        assert!(Value::char('\0').as_bool() == Some(false));
    }

    #[test]
    fn test_atom_category_matches_protocol_dispatch() {
        let b = Value::bool(true);
        assert!(matches!(b, Value::Atom(Atom::Bool(true))));
        let c = Value::char('z');
        assert!(matches!(c, Value::Atom(Atom::Char('z'))));
    }

    #[test]
    fn test_value_as_i64() {
        let v = i64_to_bits(42);
        assert_eq!(v.as_i64(), Some(42));
    }

    #[test]
    fn test_value_as_f64() {
        let v = f64_to_bits(3.14);
        let result = v.as_f64().unwrap();
        assert!((result - 3.14).abs() < 1e-10);
    }

    #[test]
    fn test_value_as_bool() {
        assert!(bool_to_bits(true).as_bool().unwrap());
        assert!(!bool_to_bits(false).as_bool().unwrap());
    }

    #[test]
    fn test_virtual_heap_alloc_read_write() {
        let mut heap = VirtualHeap::new();
        let addr = heap.allocate(16);
        assert!(heap.contains(addr));

        heap.write(addr, &[1, 2, 3, 4]).unwrap();
        let data = heap.read(addr, 4).unwrap();
        assert_eq!(data, &[1, 2, 3, 4]);
    }

    #[test]
    fn test_virtual_heap_read_out_of_bounds() {
        let mut heap = VirtualHeap::new();
        let addr = heap.allocate(8);
        assert!(heap.read(addr, 16).is_none());
    }

    #[test]
    fn test_virtual_heap_write_out_of_bounds() {
        let mut heap = VirtualHeap::new();
        let addr = heap.allocate(4);
        assert!(heap.write(addr, &[0u8; 8]).is_err());
    }

    #[test]
    fn test_virtual_heap_free() {
        let mut heap = VirtualHeap::new();
        let addr = heap.allocate(8);
        heap.free(addr).unwrap();
        assert!(!heap.contains(addr));
    }

    #[test]
    fn test_virtual_heap_free_nonexistent() {
        let mut heap = VirtualHeap::new();
        assert!(heap.free(999).is_err());
    }

    #[test]
    fn test_virtual_heap_sequential_allocs_give_different_addresses() {
        let mut heap = VirtualHeap::new();
        let a1 = heap.allocate(8);
        let a2 = heap.allocate(8);
        assert_ne!(a1, a2);
    }

    #[test]
    fn test_virtual_heap_non_contained_address() {
        let heap = VirtualHeap::new();
        assert!(!heap.contains(0xDEAD));
    }

    #[test]
    fn test_zero_bits() {
        let v = zero_bits(4);
        assert_eq!(v.as_i64(), Some(0));
    }

    #[test]
    fn test_deref_unwraps_ref() {
        // Deref of a Ref returns the wrapped value.
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        let inner_val = Value::int(42);
        bindings.insert("x".to_string(), Value::Ref(Box::new(inner_val)));
        let expr = Expr::Deref(Box::new(Expr::Identifier("x".to_string())));
        let result = eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(result.as_i64(), Some(42));
    }

    #[test]
    fn test_deref_non_ref_passthrough() {
        // Deref of a non-Ref value just returns the value as-is.
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("x".to_string(), Value::int(42));
        let expr = Expr::Deref(Box::new(Expr::Identifier("x".to_string())));
        let result = eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(result.as_i64(), Some(42));
    }

    #[test]
    fn test_addrof_wraps_in_ref() {
        // AddrOf wraps the value in a Ref.
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("x".to_string(), Value::int(42));
        let expr = Expr::AddrOf(Box::new(Expr::Identifier("x".to_string())));
        let result = eval_expr(&expr, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        match result {
            Value::Ref(wrapped) => assert_eq!(wrapped.as_i64(), Some(42)),
            _ => panic!("expected Ref, got {:?}", result),
        }
    }

    #[test]
    fn test_addrof_deref_roundtrip() {
        // *(x) wraps then unwraps.
        let mut heap = VirtualHeap::new();
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("x".to_string(), Value::int(99));
        let addrof = Expr::AddrOf(Box::new(Expr::Identifier("x".to_string())));
        let deref = Expr::Deref(Box::new(addrof));
        let result = eval_expr(&deref, &mut heap, &mut bindings, &HashMap::new()).unwrap();
        assert_eq!(result.as_i64(), Some(99));
    }

    // ── init seeding reference (2026-08-09, Phase 2) ──────────────────

    fn parse_program(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn packed_struct_fields_round_trip_in_interpreter() {
        // 2026-08-13 (pack): the interpreter models structs as layout-free
        // named products — a `pack struct` program must run and return the
        // semantic field values (the backend adds the physical bit-packing;
        // both agree on the value domain).
        let program = parse_program(
            "pack struct Nib { a: Bits<12>; b: Bits<4>; c: Bits<8>; };\n\
             defn pkmix() -> Int {\n\
               let n: Nib = Nib { a: 0xABC as Bits<12>, b: 0xF as Bits<4>, c: 0xFF as Bits<8> };\n\
               term (n.a as Int) + (n.b as Int) * 5 + (n.c as Int) * 11;\n\
             };\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let v = interp.call_function("pkmix", &[]).unwrap();
        assert_eq!(v.as_i64(), Some(0xABC + 0xF * 5 + 0xFF * 11));
    }

    /// 2026-09-02 (plan fundamental-parent-membership): Float16 arithmetic
    /// runs in the reference interpreter through the dynamic f64 model —
    /// exact f16-representable inputs (guaranteed by the typechecker's
    /// literal-exactness gate) yield exact results. The interpreter has no
    /// type-directed narrowing for ANY width (Int8 is i64 too — the
    /// deliberate dynamic model); the compiled backend rounds to f16 at
    /// storage. For add/sub/mul of f16-exact values the two agree
    /// bit-for-bit (f64 compute of f16-exact operands is exact; a single
    /// final rounding in the backend lands on the same value).
    #[test]
    fn float16_arithmetic_is_exact_in_the_reference() {
        let program = parse_program(
            "type Float16 : Float { spec MaxBits: 16; spec Alignment: 2; };\n\
             defn add16() -> Float16 {\n\
               let a: Float16 = 1.5;\n\
               let b: Float16 = 2.25;\n\
               term a + b;\n\
             };\n\
             defn mul16() -> Float16 {\n\
               let a: Float16 = 1.5;\n\
               let b: Float16 = 2.0;\n\
               term a * b;\n\
             };\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let v = interp.call_function("add16", &[]).unwrap();
        assert_eq!(v.as_f64(), Some(3.75), "1.5 + 2.25 must be exactly 3.75");
        let v = interp.call_function("mul16", &[]).unwrap();
        assert_eq!(v.as_f64(), Some(3.0), "1.5 * 2.0 must be exactly 3.0");
    }

    #[test]
    fn trap_statement_aborts_interpreter() {
        // 2026-08-13 (layout-keywords plan Phase 4): `trap;` raises the abort
        // diagnostic in the reference interpreter (SPEC §8.8), mirroring the
        // LLVM `llvm.trap` + `unreachable` sequence.
        let program = parse_program(
            "defn f() -> Int {\n  trap;\n  term 1;\n};\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let err = interp.call_function("f", &[]).unwrap_err();
        assert!(
            matches!(err, crate::errors::RuntimeError::Trap),
            "trap; must raise RuntimeError::Trap, got {err}"
        );
    }

    #[test]
    fn halt_statement_raises_halt() {
        // 2026-09-06 (halt slice): `halt;` — the bare-metal stop — the
        // program stops here; check mode reports RuntimeError::Halt (the
        // backend emits the wfi spin loop on ARM-family targets, the trap
        // abort elsewhere).
        let program = parse_program(
            "defn f() -> Int {\n  halt;\n  term 1;\n};\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let err = interp.call_function("f", &[]).unwrap_err();
        assert!(
            matches!(err, crate::errors::RuntimeError::Halt),
            "halt; must raise RuntimeError::Halt, got {err}"
        );
    }

    #[test]
    fn init_value_form_seeds_and_reads() {
        // Value form: the expr is evaluated once and the name resolves.
        let program = parse_program("init BufSize: Int = 64;\n");
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        assert_eq!(interp.state.get("BufSize").and_then(|v| v.as_i64()), Some(64));
        let expr = Expr::Identifier("BufSize".into());
        let v = interp.eval_expr(&expr).unwrap();
        assert_eq!(v.as_i64(), Some(64));
    }

    #[test]
    fn init_body_form_term_yields_seed() {
        // Body form: statements run once; `term <expr>` yields the seed.
        let program = parse_program(
            "init Layout: [16 | 32 | 64] Int { term 32; };\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        assert_eq!(interp.state.get("Layout").and_then(|v| v.as_i64()), Some(32));
    }

    #[test]
    fn init_reassign_is_a_runtime_error() {
        // The reference: a write to a seeded init is rejected at runtime too.
        let program = parse_program("init BufSize: Int = 64;\n");
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let stmt = Statement::Assign(
            Expr::Identifier("BufSize".into()),
            Expr::Decimal(128),
        );
        let err = interp.exec_stmt(&stmt).unwrap_err();
        assert!(
            format!("{}", err).contains("immutable for the run"),
            "expected ImmutableInit, got {err}"
        );
    }

    #[test]
    fn duplicate_init_seed_is_a_runtime_error() {
        // Set-once: seeding the same init twice is an error.
        let program = parse_program(
            "init BufSize: Int = 64;\ninit BufSize: Int = 128;\n",
        );
        let mut interp = Interpreter::new();
        let err = interp.seed_inits(&program).unwrap_err();
        assert!(
            format!("{}", err).contains("immutable for the run"),
            "expected ImmutableInit, got {err}"
        );
    }

    // ── 2026-08-09 (Phase 10): defer cleanup ─────────────────────────

    #[test]
    fn defer_runs_lifo_on_normal_termination() {
        // `defer` bodies run LIFO on normal termination (fall-through past the
        // body or explicit term). The reference: cleanup once, inner-first.
        let program = parse_program(
            "defn f() -> Int {\n\
             \x20 defer { let _d1: Int = 1; };\n\
             \x20 let r: Int = 7;\n\
             \x20 term r;\n\
             };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        // The defer registers; after a term the stack drains (empty = ran).
        let v = interp.call_function("f", &[]).unwrap();
        assert_eq!(v.as_i64(), Some(7));
        assert!(interp.defer_stack.is_empty(), "defers must drain after term");
    }

    #[test]
    fn defer_registers_then_flushes() {
        let program = parse_program("defn f() -> Int { defer { term 1; }; term 2; };");
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        interp.call_function("f", &[]).unwrap();
        assert!(
            interp.defer_stack.is_empty(),
            "defer stack must be empty after the firing completes"
        );
    }

    // ── 2026-08-09 (Phase 10): task spawn / await ────────────────────

    #[test]
    fn task_spawn_runs_inline_and_await_reads_result() {
        // 2026-08-23 (async Phase A2): LAZY execution — spawn captures the
        // fn+args as a Ready task entry; the first `await` triggers execution
        // and returns the real result. The spawn handle is a task-id marker.
        let program = parse_program("defn compute(x: Int) -> Int { term x * 2; };");
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        // Spawn creates a Ready task entry, returns a task-id marker.
        let handle = interp.eval_expr(&Expr::Spawn {
            type_name: "compute".to_string(),
            args: vec![Expr::Decimal(21)],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        let task_id = handle.as_i64().expect("handle is task-id Int");
        assert!(task_id >= 0, "task id is non-negative");
        // Verify the task table has a Ready entry (thread-local mirror).
        let snapshot = crate::interpreter::task_table_snapshot().unwrap();
        assert_eq!(
            snapshot.get(&(task_id as u64)).map(|e| e.status.clone()),
            Some(crate::interpreter::TaskStatus::Ready),
            "spawn must register a Ready task"
        );
        // Bind the handle, then await triggers lazy execution.
        interp.state.insert("t".to_string(), handle);
        let awaited = interp.eval_expr(&Expr::Await(Box::new(Expr::Identifier("t".to_string())))).unwrap();
        assert_eq!(awaited.as_i64(), Some(42), "await must yield the task result");
        // After await, the table shows Done.
        let snapshot = crate::interpreter::task_table_snapshot().unwrap();
        assert_eq!(
            snapshot.get(&(task_id as u64)).map(|e| e.status.clone()),
            Some(crate::interpreter::TaskStatus::Done),
            "await must transition the task to Done"
        );
    }

    // ── 2026-08-26 (async Phase B): port wake / block / cancel ────────
    // SPEC §12.2: a task reading an unready port suspends; firing the port
    // re-marks it Ready; `free` cancels so no executor resurrects it.

    /// Fresh unready event slot — the consumer-side wire.
    fn unready_port() -> Value {
        Value::EventQ(std::rc::Rc::new(std::cell::RefCell::new(EventSlot {
            ready: false,
            payload: None,
            waiters: Vec::new(),
        })))
    }

    fn status_of(id: u64) -> Option<TaskStatus> {
        task_table_snapshot().and_then(|t| t.get(&id).map(|e| e.status.clone()))
    }

    fn damage_literal(amount: i64) -> Expr {
        Expr::StructLiteral {
            type_name: "Damage".to_string(),
            fields: vec![("amount".to_string(), Expr::Decimal(amount))],
            specs: Vec::new(),
        }
    }

    fn fire_stmt(target: &str, amount: i64) -> Statement {
        Statement::ArrowAssign {
            target: Some(Box::new(Expr::Identifier(target.to_string()))),
            value: Box::new(damage_literal(amount)),
            consume: false,
        }
    }

    #[test]
    fn blocked_read_suspends_then_fire_wakes() {
        let program = parse_program(
            "struct Damage { amount: Int }; \
             defn consume(d: Damage) -> Int { term d.amount; };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        interp.state.insert("wire".to_string(), unready_port());
        let handle = interp.eval_expr(&Expr::Spawn {
            type_name: "consume".to_string(),
            args: vec![Expr::Identifier("wire".to_string())],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        let tid = handle.as_i64().unwrap() as u64;
        interp.state.insert("t".to_string(), handle.clone());
        let await_t = || Expr::Await(Box::new(Expr::Identifier("t".to_string())));

        // Await #1: the only runnable task blocks on the unready port. The
        // pool empties → await returns the HANDLE (deadlock posture, no
        // hang) and the task sits in Waiting.
        let first = interp.eval_expr(&await_t()).unwrap();
        assert_eq!(first, handle, "blocked pool returns the handle");
        assert_eq!(status_of(tid), Some(TaskStatus::Waiting), "block suspends");

        // Top-level fire: wire <- Damage{amount:42}; wakes the waiter.
        let fired = eval_statement(
            &fire_stmt("wire", 42),
            &mut interp.heap, &mut interp.state, &interp.functions,
        );
        assert!(fired.is_ok());
        assert_eq!(
            status_of(tid), Some(TaskStatus::Ready),
            "fire must wake the blocked task"
        );

        // Await #2: the SAME segment re-runs from its head — the read now
        // succeeds — and yields the payload member.
        let v = interp.eval_expr(&await_t()).unwrap();
        assert_eq!(v.as_i64(), Some(42), "post-wake read sees the payload");
        assert_eq!(status_of(tid), Some(TaskStatus::Done));
    }

    #[test]
    fn single_await_drives_producer_wake_chain() {
        // The acceptance shape: one await(consumer) interleaves BOTH tasks —
        // the consumer blocks, the producer's fire revives it mid-loop. The
        // result 7 is reachable ONLY if produce executed between consume's
        // block and its completion.
        let program = parse_program(
            "struct Damage { amount: Int }; \
             defn consume(d: Damage) -> Int { term d.amount; }; \
             defn produce(p: Damage) -> Int { p <- Damage{amount:7}; term 1; };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        interp.state.insert("wire".to_string(), unready_port());
        for (name, fname) in [("c", "consume"), ("p", "produce")] {
            let h = interp.eval_expr(&Expr::Spawn {
                type_name: fname.to_string(),
                args: vec![Expr::Identifier("wire".to_string())],
                storage: crate::ast::SpawnStorage::Pooled,
            }).unwrap();
            interp.state.insert(name.to_string(), h);
        }
        let v = interp.eval_expr(&Expr::Await(Box::new(Expr::Identifier("c".to_string())))).unwrap();
        assert_eq!(v.as_i64(), Some(7), "producer's fire fed the consumer");
        // Both sides finished; nothing left schedulable.
        assert!(collect_runnable().is_empty(), "no zombie tasks remain");
    }

    #[test]
    fn free_cancels_ready_and_blocked_tasks() {
        let program = parse_program(
            "struct Damage { amount: Int }; \
             defn slow(d: Damage) -> Int { yield; term d.amount; };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        interp.state.insert("wire".to_string(), unready_port());

        // (a) free BEFORE any execution: never runs.
        let h1 = interp.eval_expr(&Expr::Spawn {
            type_name: "slow".to_string(),
            args: vec![Expr::Identifier("wire".to_string())],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        let t1 = h1.as_i64().unwrap() as u64;
        interp.state.insert("t1".to_string(), h1);
        eval_statement(
            &Statement::FreeHint("t1".to_string()),
            &mut interp.heap, &mut interp.state, &interp.functions,
        ).unwrap();
        assert_eq!(status_of(t1), Some(TaskStatus::Cancelled));

        // (b) free AFTER a block: Waiting → Cancelled.
        let h2 = interp.eval_expr(&Expr::Spawn {
            type_name: "slow".to_string(),
            args: vec![Expr::Identifier("wire".to_string())],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        let t2 = h2.as_i64().unwrap() as u64;
        interp.state.insert("t2".to_string(), h2);
        interp.eval_expr(&Expr::Await(Box::new(Expr::Identifier("t2".to_string())))).unwrap();
        assert_eq!(status_of(t2), Some(TaskStatus::Waiting));
        eval_statement(
            &Statement::FreeHint("t2".to_string()),
            &mut interp.heap, &mut interp.state, &interp.functions,
        ).unwrap();

        // Firing the port must NOT resurrect either task.
        eval_statement(
            &fire_stmt("wire", 99),
            &mut interp.heap, &mut interp.state, &interp.functions,
        ).unwrap();
        assert_eq!(status_of(t1), Some(TaskStatus::Cancelled));
        assert_eq!(status_of(t2), Some(TaskStatus::Cancelled));
        assert!(collect_runnable().is_empty(), "freed tasks never schedule");

        // Await of a freed handle returns the handle, not a result. The
        // binding is gone (freed locals read as errors), so await the raw
        // task id.
        let v = interp.eval_expr(&Expr::Await(Box::new(Expr::Decimal(t1 as i64)))).unwrap();
        assert_ne!(v.as_i64(), Some(99), "a cancelled task has no result");
    }

    #[test]
    fn unready_read_outside_task_stays_strict() {
        // Top-level reads keep the SPEC §9.5 gate: .^Ready first, else error.
        let mut interp = Interpreter::new();
        interp.state.insert("wire".to_string(), unready_port());
        let err = interp.eval_expr(&Expr::Field(
            Box::new(Expr::Identifier("wire".to_string())),
            "amount".to_string(),
        )).unwrap_err();
        match err {
            RuntimeError::TypeError { found, .. } => {
                assert!(found.contains("not Ready"), "{found}");
            }
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn registration_presplits_segments_before_port_reads() {
        // Plan §4: the blocking read must HEAD its segment so the post-wake
        // re-run never repeats side effects that preceded it. The fire below
        // is NOT a param-field read (bare identifier target) → segment 0;
        // `term d.amount` is → boundary before it.
        let program = parse_program(
            "struct Damage { amount: Int }; \
             defn job(d: Damage, o: Damage) -> Int { o <- Damage{amount:1}; term d.amount; };",
        );
        let body = program
            .iter()
            .filter_map(|i| match i {
                TopLevel::Definition(def) => Some(def.body.clone()),
                _ => None,
            })
            .next()
            .expect("job parsed");
        // The registration mirror is seeded per program — load first.
        Interpreter::new().load_program(&program);
        register_pending_task(
            9_001, "job".to_string(), Vec::new(), body,
            &["d".to_string(), "o".to_string()],
        );
        let (segments, current) = take_task_segments(9_001).expect("registered");
        assert_eq!(current, 0);
        assert_eq!(segments.len(), 2, "fire | port-read split");
        assert!(
            matches!(segments[0][0], Statement::ArrowAssign { .. }),
            "segment 0 heads with the side effect"
        );
        assert!(
            matches!(segments[1][0], Statement::Term(Some(_))),
            "segment 1 HEADS with the blocking read"
        );
    }



    #[test]
    fn coll_count_and_capacity_intrinsic_parity() {
        // The interpreter's coll value is a Product with no capacity concept
        // (SPEC §8.10, §3.6): a 21-element coll holds 21 fields exactly. The
        // backend grows past the default cap (16) and must hold the same 21
        // elements — `Count#` agrees on both, and `Capacity#(product)` is its
        // field count (a Vec is exact-fit). The >16 count is the grow-on-full
        // parity anchor: pre-fix the backend wrote OOB past the 16-slot buffer.
        let program = parse_program(
            "coll obj MyQueue { data: Ptr<Int>; };\n\
             defn fill() -> Int {\n\
               let q: MyQueue = [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20];\n\
               let n: Int = q.Count#();\n\
               let cap: Int = Capacity#(q);\n\
               let sum: Int = 0;\n\
               foreach x in q { sum = sum + x; };\n\
               term n + cap + sum;\n\
             };\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let v = interp.call_function("fill", &[]).unwrap();
        assert_eq!(
            v.as_i64(),
            Some(21 + 21 + 210),
            "Count# 21 + Capacity# 21 (exact-fit) + sum 0..=20 = 210"
        );
    }

    #[test]
    fn coll_struct_literal_semantics_parity() {
        // 2026-08-16 (Phase 3a): a fixed `coll struct` is a Product in the
        // interpreter (a list literal's value). Count# == element count == N,
        // Capacity# == N (exact-fit), foreach sums the elements. The backend's
        // inline-array coll struct must agree on every observable.
        let program = parse_program(
            "coll struct Fixed { data: Int[4]; };\n\
             defn fill() -> Int {\n\
               let f: Fixed = [1, 2, 3, 4];\n\
               let n: Int = f.Count#();\n\
               let cap: Int = Capacity#(f);\n\
               let sum: Int = 0;\n\
               foreach x in f { sum = sum + x; };\n\
               term n + cap + sum;\n\
             };\n",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let v = interp.call_function("fill", &[]).unwrap();
        assert_eq!(
            v.as_i64(),
            Some(4 + 4 + 10),
            "Count# 4 + Capacity# 4 (fixed N) + sum 1+2+3+4 = 10 → 18"
        );
    }
}

/// 2026-08-23 (async A1): read-only snapshot of the thread-local task table.
pub fn task_table_snapshot() -> Option<HashMap<u64, TaskEntry>> {
    TASK_TABLE.with(|t| t.borrow().clone())
}

/// 2026-08-23 (async Phase A3): take segments + current position for
/// execution. Called by await before running the task's next segment.
pub fn take_task_segments(id: u64) -> Option<(Vec<Vec<Statement>>, usize)> {
    TASK_TABLE.with(|t| {
        let mut table_ref = t.borrow_mut();
        let table = table_ref.as_mut()?;
        let entry = table.get_mut(&id)?;
        if entry.status == TaskStatus::Ready || entry.status == TaskStatus::Yielded {
            Some((entry.segments.clone(), entry.current_segment))
        } else {
            None
        }
    })
}

/// 2026-08-23 (async Phase A3): advance the segment counter after executing.
pub fn advance_segment(id: u64) {
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                entry.current_segment += 1;
                if entry.current_segment < entry.segments.len() {
                    entry.status = TaskStatus::Yielded;
                }
            }
        }
    });
}

/// 2026-08-23 (async Phase A3): mark a task as Done.
pub fn mark_done(id: u64) {
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                entry.status = TaskStatus::Done;
            }
        }
    });
}

// ── 2026-08-23 (async Phase A3 restored): scheduler helpers ────────────

/// Collect runnable tasks (Ready/Yielded) in id order for round-robin.
pub fn collect_runnable() -> Vec<(u64, String, Vec<Value>, Vec<Vec<Statement>>, usize)> {
    TASK_TABLE.with(|t| {
        let table_ref = t.borrow();
        let Some(table) = table_ref.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        let mut ids: Vec<&u64> = table.keys().collect();
        ids.sort();
        for id in ids {
            let entry = &table[id];
            if entry.status == TaskStatus::Ready || entry.status == TaskStatus::Yielded {
                out.push((
                    *id,
                    entry.fn_name.clone(),
                    entry.pending_args.clone().unwrap_or_default(),
                    entry.segments.clone(),
                    entry.current_segment,
                ));
            }
        }
        out
    })
}

/// Check whether a task has reached Done status.
pub fn task_is_done(id: u64) -> bool {
    TASK_TABLE.with(|t| {
        t.borrow()
            .as_ref()
            .and_then(|table| table.get(&id))
            .map(|e| e.status == TaskStatus::Done)
            .unwrap_or(false)
    })
}

/// Get a completed task's result.
pub fn get_task_result(id: u64) -> Option<Value> {
    TASK_TABLE.with(|t| {
        t.borrow()
            .as_ref()
            .and_then(|table| table.get(&id))
            .and_then(|e| e.result.clone())
    })
}

/// Advance a specific task's segment counter; returns true when fully done.
pub fn advance_segment_status(id: u64) -> bool {
    TASK_TABLE.with(|t| {
        let mut table_ref = t.borrow_mut();
        let Some(table) = table_ref.as_mut() else { return false };
        if let Some(entry) = table.get_mut(&id) {
            entry.current_segment += 1;
            if entry.current_segment >= entry.segments.len() {
                entry.status = TaskStatus::Done;
                return true;
            }
            entry.status = TaskStatus::Yielded;
            return false;
        }
        false
    })
}

/// Store a task's final result.
pub fn store_task_result(id: u64, result: Value) {
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                entry.result = Some(result);
                entry.status = TaskStatus::Done;
            }
        }
    });
}

// ── 2026-08-26 (async Phase B): event wake / block / cancel ────────────

/// 2026-08-26 (Phase B): the current task blocks reading an unready slot —
/// register it as a waiter and mark `Waiting`. Called from Field-on-EventQ
/// when `CURRENT_TASK` is set. The segment executor catches the resulting
/// `TaskBlocked` WITHOUT advancing the segment, so post-wake the same
/// segment re-runs (its read now succeeds — level-triggered, SPEC §12.2).
pub fn block_current_task_on_slot(slot: &std::rc::Rc<std::cell::RefCell<EventSlot>>) {
    let Some(tid) = current_task_id() else { return };
    slot.borrow_mut().waiters.push(tid);
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&tid) {
                entry.status = TaskStatus::Waiting;
            }
        }
    });
}

/// 2026-08-26 (Phase B): a producer fired this slot — drain waiters,
/// flipping each still-`Waiting` task back to `Ready`. Cancelled/Done ids
/// are skipped (a freed task never resurrects). Call AFTER releasing the
/// slot's `borrow_mut` (the fire path holds one).
pub fn fire_slot_wake(slot: &std::rc::Rc<std::cell::RefCell<EventSlot>>) {
    let waiters = std::mem::take(&mut slot.borrow_mut().waiters);
    if waiters.is_empty() { return; }
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            for tid in waiters {
                if let Some(entry) = table.get_mut(&tid) {
                    if entry.status == TaskStatus::Waiting {
                        entry.status = TaskStatus::Ready;
                    }
                }
            }
        }
    });
}

/// 2026-08-26 (Phase B): runtime cancellation. `free t;` removes the local,
/// but the table entry stayed schedulable — other awaits' round-robins
/// would run a task nobody can observe. Mark `Cancelled` so every executor
/// skips it permanently.
pub fn cancel_task(id: u64) {
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                if !matches!(entry.status, TaskStatus::Done | TaskStatus::Cancelled) {
                    entry.status = TaskStatus::Cancelled;
                }
            }
        }
    });
}

/// 2026-08-26 (Phase B): store a final result AND transition to Done
/// atomically. The A3 code stored transiently after every segment pass —
/// `task_is_done` briefly lied mid-round; with blocked tasks in the pool a
/// transient Done could skip a not-yet-woken task's turn.
pub fn mark_done_with_result(id: u64, result: Value) {
    TASK_TABLE.with(|t| {
        if let Some(table) = t.borrow_mut().as_mut() {
            if let Some(entry) = table.get_mut(&id) {
                entry.result = Some(result);
                entry.status = TaskStatus::Done;
            }
        }
    });
}

// ── 2026-08-26 (Phase B2): cell runtime ────────────────────────────────

#[cfg(test)]
mod cell_b2_tests {
    use super::*;

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    fn bindings_instance(interp: &Interpreter) -> std::rc::Rc<std::cell::RefCell<HashMap<String, Value>>> {
        match interp.state.get("src").expect("instance bound") {
            Value::Instance { fields, .. } => fields.clone(),
            other => panic!("not an instance: {:?}", std::mem::discriminant(other)),
        }
    }

    #[test]
    fn cell_spawn_wires_ports_and_dispatches_members() {
        let program = parse_program(
            "struct Sample { value: Int }; \
             cell Source() -> evt: Event<Sample> { \
                 count: Int; \
                 txn emit(v: Int) -> Bool { count = count + 1; evt <- Sample{value:v}; term true; } \
             };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        // Cell spawn constructs an instance with a fresh unready OUT port.
        let h = interp.eval_expr(&Expr::Spawn {
            type_name: "Source".to_string(),
            args: vec![],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        interp.state.insert("src".to_string(), h.clone());
        match &h {
            Value::Instance { type_name, fields } => {
                assert_eq!(type_name, "Source");
                let f = fields.borrow();
                let unready = matches!(&f["evt"], Value::EventQ(q) if !q.borrow().ready);
                assert!(unready, "out port starts unready");
                assert!(f.contains_key("count"), "field defaulted");
            }
            other => panic!("expected instance, got {:?}", std::mem::discriminant(other)),
        }
        // Member dispatch mutates state and fires the port.
        let fired = interp.eval_expr(&Expr::MethodCall(
            Box::new(Expr::Identifier("src".to_string())),
            "emit".to_string(),
            vec![Expr::Decimal(42)],
            None,
            vec![],
        )).unwrap();
        assert_eq!(fired.as_i64(), Some(1), "txn returns true (Bool as 1)");
        let src = bindings_instance(&interp);
        {
            let f = src.borrow();
            assert_eq!(f.get("count").and_then(|v| v.as_i64()), Some(1), "slot write-back");
            if let Some(Value::EventQ(q)) = f.get("evt") {
                let slot = q.borrow();
                assert!(slot.ready, "emit fired the port");
                match &slot.payload {
                    Some(Value::Product { fields, names }) => {
                        let i = names.as_ref().unwrap().iter().position(|n| n == "value");
                        assert_eq!(i.map(|i| fields[i].as_i64()), Some(Some(42)));
                    }
                    other => panic!("payload shape {:?}", other.is_some()),
                }
            } else {
                panic!("out port missing");
            }
        }
    }

    #[test]
    fn phase_b_consumer_wakes_through_cell_port() {
        // The B2 acceptance composition: consumer blocks on the CELL's out
        // port; a producer TASK drives the cell txn whose fire wakes it. One
        // await interleaves both through the Phase B round-robin.
        let program = parse_program(
            "struct Sample { value: Int }; \
             cell Source() -> evt: Event<Sample> { \
                 count: Int; \
                 txn emit(v: Int) -> Bool { count = count + 1; evt <- Sample{value:v}; term true; } \
             }; \
             defn consume(d: Sample) -> Int { term d.value; }; \
             defn produce(s: Source) -> Int { term s.emit(9); };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let h = interp.eval_expr(&Expr::Spawn {
            type_name: "Source".to_string(),
            args: vec![],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        interp.state.insert("src".to_string(), h);

        // Wire extraction: src.out yields the SHARED EventQ handle.
        let wire = interp.eval_expr(&Expr::Field(
            Box::new(Expr::Identifier("src".to_string())),
            "evt".to_string(),
        )).unwrap();
        interp.state.insert("wire".to_string(), wire);

        for (name, fname, arg) in [("c", "consume", "wire"), ("p", "produce", "src")] {
            let th = interp.eval_expr(&Expr::Spawn {
                type_name: fname.to_string(),
                args: vec![Expr::Identifier(arg.to_string())],
                storage: crate::ast::SpawnStorage::Pooled,
            }).unwrap();
            interp.state.insert(name.to_string(), th);
        }
        let v = interp.eval_expr(&Expr::Await(Box::new(Expr::Identifier("c".to_string())))).unwrap();
        assert_eq!(v.as_i64(), Some(9), "cell fire woke the blocked consumer");

        // Both tasks Done; the cell's count ticked exactly once.
        let snapshot = task_table_snapshot().unwrap();
        assert!(snapshot.values().all(|e| e.status == TaskStatus::Done));
        let inst = bindings_instance(&interp);
        assert_eq!(
            inst.borrow().get("count").and_then(|v| v.as_i64()),
            Some(1)
        );
    }
}

#[cfg(test)]
mod phase_c_probe_tests {
    use super::*;

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn locals_die_at_a_yield_boundary() {
        // Reference rule (SPEC §12.2, Phase C prerequisite): ONLY parameters
        // carry across a cancellation point — a `let` before `yield;` is
        // undefined in later segments. This is what makes segmented-
        // continuation lowering exactly faithful (plan 2026-08-26-async-
        // phase-c §Reference).
        let program = parse_program(
            "defn job(n: Int) -> Int { let acc: Int = n * 2; yield; term acc; };",
        );
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let h = interp.eval_expr(&Expr::Spawn {
            type_name: "job".to_string(),
            args: vec![Expr::Decimal(5)],
            storage: crate::ast::SpawnStorage::Pooled,
        }).unwrap();
        interp.state.insert("t".to_string(), h);
        let v = interp.eval_expr(&Expr::Await(Box::new(Expr::Identifier("t".to_string()))));
        match v {
            Err(RuntimeError::UndefinedVariable { name }) => {
                assert_eq!(name, "acc", "the pre-yield local is gone");
            }
            Ok(x) => panic!("locals must NOT survive yield, got {:?}", x.as_i64()),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
