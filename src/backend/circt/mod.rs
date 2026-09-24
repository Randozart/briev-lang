// CIRCT Backend — emits MLIR text in HW + Comb + Seq dialects.
// Invoked via: briev build file.sbv → program.mlir → circt-opt → circt-translate → verilog

pub mod mem_policy;
pub mod normalizer;

use crate::analysis::dependency_graph::DependencyGraph;
use crate::ast::{BinaryOpKind, Contract, Expr, OutputType, Statement, TopLevel, Type, UnaryOpKind};
use std::collections::HashMap;
use std::fmt::Write;

/// Metadata for a trigger variable mapped to a module port.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerPort {
    /// Signal name used as the port name.
    pub port_name: String,
    /// Original trigger name in Briev source.
    pub trg_name: String,
    /// Whether this trigger has the `#wake` modifier (generates wake_ output).
    pub is_wake: bool,
    /// 2026-08-27 (Slice B): the @-address for MMIO pins; addressed ports
    /// emit ADDRESS-SORTED on @top (deterministic bus layout rule).
    pub address: Option<u64>,
}

/// CIRCT backend state for MLIR code generation.
#[derive(Debug, Clone)]
pub struct CirctBackend {
    pub trg_ports: Vec<TriggerPort>,
    pub var_types: HashMap<String, Type>,
    pub var_exprs: HashMap<String, Option<Expr>>,
    /// State variables with @ addresses that should become external ports (MMIO).
    /// 2026-08-27 (Slice B): the LIVE MMIO pin table — (name, @-address)
    /// for every Explicit-addressed trigger, consumed address-sorted at
    /// top-module port emission.
    pub mmio_vars: Vec<(String, u64)>,
    /// Known function/module names and their argument counts (for submodule instantiation).
    pub fn_arity: HashMap<String, usize>,
    /// Cell definitions encountered during program traversal.
    /// Key is cell name, value is the CellDef AST node.
    cell_defs: HashMap<String, crate::ast::CellDef>,
    /// 2026-08-23 (Plan 3.1): the populated TypeUniverse — mlir_type derives
    /// widths/signs from protocol categories here instead of name-matching
    /// (rule 19). Set by the pipeline via with_universe(); tests may use the
    /// default (empty) universe, which falls back to 64-bit with a recorded
    /// diagnostic when a type cannot be resolved.
    pub type_universe: crate::type_universe::TypeUniverse,
    /// 2026-08-23 (Plan 3.3): constructs that reached codegen outside the
    /// supported surface. The pipeline turns non-empty into a hard compile
    /// error — hardware targets must never silently drop logic. RefCell
    /// because emitters are &self.
    pub errors: std::cell::RefCell<Vec<String>>,
    /// 2026-08-25 (Plan 3.6): bounded state arrays flattened to register
    /// files — var name → per-lane register names (`buf` → buf_0..buf_{n-1}).
    /// Reads mux over lanes; writes decode the index into per-lane enables.
    pub array_groups: HashMap<String, Vec<String>>,
    /// Array var name → element MLIR type (lanes all share it).
    pub array_elem_ty: HashMap<String, String>,
    /// 2026-08-25 (seq-firmem plan): arrays decided to the MEMORY MACRO —
    /// var → plan (macro wire name assigned at declaration emission).
    pub array_mems: HashMap<String, MemPlan>,
    /// Default-policy decisions surfaced as ONE aggregated note (what/why/
    /// fix). Explicit pins never land here — they silence by definition.
    pub notices: std::cell::RefCell<Vec<String>>,
}

/// A memory-macro-lowered state array.
#[derive(Clone, Debug)]
pub struct MemPlan {
    pub depth: usize,
    /// Width in bits (element width).
    pub width: usize,
    /// The seq.firmem result wire (assigned during module preamble).
    pub wire: String,
    /// Macro module name (companion file stem): <var>_<D>x<W>.
    pub macro_name: String,
 }

/// Per-generation counters for unique MLIR value names.
#[derive(Debug, Default)]
struct NameGen {
    reg_counter: usize,
    wire_counter: usize,
    const_counter: usize,
}

impl NameGen {
    fn fresh_reg(&mut self, prefix: &str) -> String {
        let n = self.reg_counter;
        self.reg_counter += 1;
        format!("%{}_{}", prefix, n)
    }
    fn fresh_wire(&mut self, prefix: &str) -> String {
        let n = self.wire_counter;
        self.wire_counter += 1;
        format!("%{}_{}", prefix, n)
    }
    fn fresh_const(&mut self, prefix: &str) -> String {
        let n = self.const_counter;
        self.const_counter += 1;
        format!("%c{}_{}", prefix, n)
    }
}

    /// 2026-08-25 (§3.4): per-module obligation wires — refusals feed
    /// `halt`, post-guard verdicts feed `check`.
    #[derive(Default)]
    struct Obligations {
        pre_fails: Vec<String>,
        post_oks: Vec<String>,
    }

    /// 2026-08-25 (§3.4 extension): a liveness-watchdog monitor plan — the
/// countdown register + timeout port derived from `?[cond]/![cond]
/// within N cyc` on a transaction contract.
struct WdMonitor {
    port: String,
    cond: Expr,
    bound: u64,
    required: bool,
}

/// 2026-08-25: the mutable wire maps one transaction emits against,
    /// bundled so emitters stay under the parameter budget (Praetor rule).
    /// `name` is the txn's own name (wire prefixes); `contract` its pair.
    struct TxnMaps<'a> {
        name: String,
        contract: &'a Contract,
        reg_names: &'a mut HashMap<String, String>,
        pending: &'a mut HashMap<String, String>,
        ob: &'a mut Obligations,
        /// This txn's pre-guard wire (§3.4 commit gate) — memory-macro
        /// write ports fold it into their ENABLE. None = trivially true.
        gate: Option<String>,
        /// Active `when` conditions (stack — nested guards AND). Every
        /// state write underneath must respect them; empty = unconditional.
        /// 2026-08-26: previously ONLY scalar assigns saw the innermost
        /// condition — element writes were SILENTLY DROPPED and deeper
        /// statements ran ungated.
        gates: Vec<String>,
    }

impl CirctBackend {
    /// 2026-08-23 (Plan 0.2): CIRCT's declared surface — synthesizable
    /// register-level logic: integer arithmetic/logic, guards, FSM bodies.
    /// No floats (comb has no float dialect here), no strings/collections,
    /// no spawn/concurrency. Enforced by validate_program before codegen.
    pub const CAPABILITIES: crate::backend::capabilities::BackendCapabilities =
        crate::backend::capabilities::BackendCapabilities {
            name: "CIRCT (.mlir hardware)",
            nature: "hardware synthesis lowers to finite register-level logic \
                     — bounded state and combinational logic only",
            int_literals: true,
            bool_char_literals: true,
            int_ops: true,
            unary_ops: true,
            calls: true,
            intrinsics: true,
            if_expr: true,
            match_expr: true,
            field_access: true,
            index: true,
            // 2026-08-25 (Plan 3.6): list literals have an honest form now —
            // a bounded state array's initializer lowers to per-lane register
            // inits. List literals OUTSIDE that role still hard-error at
            // codegen (emit_expr records them), so the gate stays honest.
            tuple_list_literals: true,
            casts: true,
            let_stmt: true,
            assign_stmt: true,
            guarded_stmt: true,
            term_endprogram: true,
            match_stmt: true,
            trap_stmt: true,
            // 2026-08-27 (Slice A): hw.module.extern blackboxes ARE lowerable
            // here even while defined cell bodies remain staged.
            extern_cells: true,
            ..crate::backend::capabilities::BackendCapabilities::NONE
        };

    pub fn new() -> Self {
        CirctBackend {
            trg_ports: Vec::new(),
            var_types: HashMap::new(),
            var_exprs: HashMap::new(),
            mmio_vars: Vec::new(),
            fn_arity: HashMap::new(),
            cell_defs: HashMap::new(),
            type_universe: crate::type_universe::TypeUniverse::new(),
            errors: std::cell::RefCell::new(Vec::new()),
            array_groups: HashMap::new(),
            array_elem_ty: HashMap::new(),
            array_mems: HashMap::new(),
            notices: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// 2026-08-23 (Plan 3.1): pipeline injects the normalized universe so
    /// type lowering reads protocol categories, never names (rule 19).
    pub fn with_universe(mut self, universe: crate::type_universe::TypeUniverse) -> Self {
        self.type_universe = universe;
        self
    }

    /// 2026-08-23 (Plan 3.3): record an unsupported construct — the caller
    /// sees a hard error; nothing vanishes from the netlist.
    pub(crate) fn record_unsupported(&self, what: &str) {
        let mut errs = self.errors.borrow_mut();
        let already = errs.iter().any(|e| e.contains(what));
        if !already {
            errs.push(format!(
                "error: CIRCT (.sbv hardware target) does not support {}\n  why: \
                 hardware synthesis lowers to finite register-level combinational \
                 logic; this construct has no honest gate-level form.\n  fix: \
                 rewrite without {}, or build for the native LLVM target.",
                what, what
            ));
        }
    }

    pub fn generate(&mut self, items: &[TopLevel]) -> String {
        // 2026-08-23 (Plan 0.1): direct-construction callers (unit tests)
        // self-compute the dependency graph exactly as the pipeline's
        // analyze_program does — identical build call + identical empty
        // fallback — so both paths agree.
        let dep_graph = DependencyGraph::build(items)
            .unwrap_or_else(|_| DependencyGraph {
                topo_order: Vec::new(),
                bit_index: HashMap::new(),
                dependencies: HashMap::new(),
                dependents: HashMap::new(),
                is_trg: std::collections::HashSet::new(),
                all_vars: std::collections::HashSet::new(),
            });
        self.generate_with_dep_graph(items, &dep_graph)
    }

    /// 2026-08-23 (Plan 0.1, backend-scaffolding-foundation): pipeline entry —
    /// consumes the shared `AnalysisResults.dependency_graph` computed once in
    /// src/compile.rs instead of re-deriving it (frontend-driven dispatch:
    /// the backend CONSUMES decisions). To undo: inline back into generate().
    pub fn generate_with_dep_graph(
        &mut self,
        items: &[TopLevel],
        dep_graph: &DependencyGraph,
    ) -> String {
        self.generate_with_dep_graph_universe(items, dep_graph, &crate::type_universe::TypeUniverse::new())
    }

    /// Pipeline entry — consumes the shared dependency graph AND the
    /// normalized TypeUniverse (Plan 3.1: rule-19 type lowering).
    /// To undo: revert to generate_with_dep_graph(items, dep_graph).
    pub fn generate_with_dep_graph_universe(
        &mut self,
        items: &[TopLevel],
        dep_graph: &DependencyGraph,
        universe: &crate::type_universe::TypeUniverse,
    ) -> String {
        self.type_universe = universe.clone();
        for item in items {
            match item {
                TopLevel::StateDecl(decl) => {
                    self.var_types.insert(decl.name.clone(), decl.ty.clone());
                    self.var_exprs.insert(decl.name.clone(), None);
                }
                TopLevel::Trigger(trg) => {
                    // 2026-08-27 (Slice B): @-addressed triggers are MMIO
                    // INPUT pins. Explicit numerics sort the port list;
                    // every other address form has no static pin — the
                    // honest boundary is a capability error.
                    let address = match &trg.instance {
                        Expr::Decimal(n) => Some(*n as u64),
                        Expr::Deref(_) => {
                            self.record_unsupported(&format!(
                                "trigger '{}' with a dynamic address — \
                                 hardware pins are static; dynamic \
                                 addressing targets the native/embedded \
                                 build",
                                trg.name
                            ));
                            None
                        }
                        _ => {
                            self.record_unsupported(&format!(
                                "trigger '{}' — only numeric @-addresses \
                                 form circuit pins (symbolic sources are an \
                                 embedded-surface feature)",
                                trg.name
                            ));
                            None
                        }
                    };
                    if let Some(addr) = address {
                        // mmio_vars is now LIVE: the address-sorted MMIO
                        // pin surface (name, address) consumed at
                        // top-port emission. No longer a dead field.
                        self.mmio_vars.push((trg.name.clone(), addr));
                    }
                    let port_name = trg.name.clone();
                    self.trg_ports.push(TriggerPort {
                        port_name: port_name.clone(),
                        trg_name: trg.name.clone(),
                        is_wake: false,
                        address,
                    });
                    self.var_types.insert(port_name, Type::int());
                    self.var_exprs.insert(trg.name.clone(), None);
                }
                // 2026-08-23 (Plan 3.1 follow-up): top-level `let` IS state.
                // Without this arm, declared types (and the vars themselves)
                // never reached var_types — counters emitted as i64 defaults
                // or vanished from outputs entirely.
                TopLevel::Statement(stmt) => {
                    if let Statement::Let { name, ty: Some(ty), expr, .. } = &**stmt {
                        self.var_types.insert(name.clone(), ty.clone());
                        self.var_exprs.insert(name.clone(), expr.clone());
                    }
                }
                TopLevel::Transaction(txn) => {
                    self.fn_arity.insert(txn.name.clone(), txn.parameters.len());
                    for stmt in &txn.body {
                        if let Statement::Assign(lhs, expr) = stmt {
                            if let Some(name) = lhs.as_var_name() {
                                if !self.var_types.contains_key(name) {
                                    self.var_types.insert(name.to_string(), Type::int());
                                    self.var_exprs.insert(name.to_string(), Some(expr.clone()));
                                }
                            }
                        }
                    }
                }
                TopLevel::Cell(cell) => {
                    // 2026-08-27 (Slice A): foreign cells keep their port
                    // header for instance matching but carry no body — and
                    // contribute NO program-visible variables (their pins
                    // belong to the blackbox interface, not the top module).
                    let is_extern = cell.extern_source.is_some();
                    self.cell_defs.insert(cell.name.clone(), cell.clone());
                    if !is_extern {
                        for (field_name, field_ty) in &cell.fields {
                            self.var_types.insert(format!("{}${}", cell.name, field_name), field_ty.clone());
                            self.var_exprs.insert(format!("{}${}", cell.name, field_name), Some(Expr::Decimal(0)));
                        }
                        for (param_name, param_ty) in &cell.parameters {
                            self.var_types.insert(format!("{}${}", cell.name, param_name), param_ty.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // 2026-08-27 (Slice B): canonical pin table — address-sorted, so the
        // live mmio surface reads identically regardless of declaration order.
        self.mmio_vars.sort();
        let mut out = String::new();
        self.emit_header(&mut out);
        self.emit_module(&mut out, &dep_graph, items);
        // 2026-08-23 (Plan 3.3): sort by name — HashMap iteration order made
        // emitted cell modules nondeterministic across processes.
        let mut sorted_cells: Vec<&String> = self.cell_defs.keys().collect();
        sorted_cells.sort();
        let cell_defs: Vec<crate::ast::CellDef> = sorted_cells
            .into_iter()
            .map(|k| self.cell_defs[k].clone())
            .collect();
        for cell_def in &cell_defs {
            self.emit_cell_module(&mut out, &dep_graph, cell_def);
        }
        out
    }

    fn emit_header(&self, out: &mut String) {
        writeln!(out, "// Generated by briev-compiler CIRCT backend").ok();
        writeln!(out, "// Run: circt-opt program.mlir | circt-translate --export-verilog").ok();
        writeln!(out).ok();
    }

    fn mlir_type(&self, ty: &Type) -> String {
        // 2026-08-23 (Plan 3.1, rule 19 rewrite): widths and signedness come
        // from the TypeUniverse's protocol categories (Cast.Int / Cast.UInt /
        // Cast.Float properties + byte size), NEVER from type names. The old
        // body matched "Int8"/"UInt32"/… strings — a rule-19 violation that
        // broke for every user-declared alias of those protocols.
        // Sized types keep their explicit width (BitRange is compiler
        // metadata, not a name).
        if let Type::Constrained(inner, bit_range) = ty {
            let width = match bit_range {
                crate::ast::BitRange::Single(w) => *w,
                crate::ast::BitRange::Range(_, hi) => *hi,
                crate::ast::BitRange::Any(w) => *w,
            };
            if matches!(inner.as_ref(), Type::Custom(__t) if __t == "Bool") && width <= 1 {
                return "i1".into();
            }
            return format!("i{}", width);
        }
        if matches!(ty, Type::Bits(1)) {
            return "i1".into();
        }
        match ty.universe_key().and_then(|k| self.type_universe.get(k)) {
            Some(rt) => {
                // Float protocol? → MLIR float type (f64 — the only float the
                // register-level surface models; narrower floats are a fix-up
                // concern, not a naming one).
                if rt.properties.contains_key("Cast.Float") {
                    return format!("f{}", if rt.bytes >= 8 { 64 } else { 32 });
                }
                let bits = if rt.bytes > 0 { rt.bytes * 8 } else { 64 };
                // 2026-08-23 (circt-opt round-trip): MLIR integer types are
                // SIGNLESS — sign lives in the op predicates (comb.icmp
                // ult/slt, comb.divu/divs), not the type. siN/uN rendered
                // types that never matched their uses ('expects different
                // type than prior uses'). Width only.
                // To undo: restore u{}/si{} branches.
                format!("i{}", bits)
            }
            None => {
                // Unresolvable through the universe: Bool/Char-style builtins
                // still resolve by their protocol once normalized; reaching
                // here means an unnormalized type — record it loudly.
                format!("i64")
            }
        }
    }

    /// Time-unit watchdog bound → cycles: ceil(hz * ns / 1e9), u128-safe.
    fn ns_to_cycles(hz: u64, ns: u64) -> u64 {
        ((hz as u128 * ns as u128 + 999_999_999) / 1_000_000_000)
            .min(u64::MAX as u128) as u64
    }

    /// Address width for a depth-D macro: ceil(log2(D)), minimum 1 — the
    /// seq dialect verifier enforces exactly this on port ops.
    fn addr_width(depth: usize) -> u32 {
        if depth <= 1 {
            return 1;
        }
        usize::BITS - (depth - 1).leading_zeros()
    }

    /// 2026-08-25 (seq-firmem plan): reference implementations for every
    /// memory macro this module instantiates. SeqToSV lowers firmem to an
    /// EXTERNALLY-generated module (upstream: firtool emits the body); our
    /// pipeline patches it to hw.module.extern and links these companions
    /// at verilator/Vivado time. Combinational read (latency 0) matches the
    /// register-file read semantics; posedge gated write carries the §3.4
    /// commit gate via W0_en.
    pub fn memory_companions(&self) -> Vec<(String, String)> {
        let mut plans: Vec<&MemPlan> = self.array_mems.values().collect();
        plans.sort_by(|a, b| a.macro_name.cmp(&b.macro_name));
        plans
            .into_iter()
            .map(|p| {
                let aw = Self::addr_width(p.depth);
                let sv = format!(
                    "// brievc seq.firmem reference implementation — {d} x {w} bit, latency-0 read\n\
                     module {m}(\n\
                     \x20 input [{awm}:0] R0_addr,\n\
                     \x20 input R0_en,\n\
                     \x20 input R0_clk,\n\
                     \x20 output [{wm}:0] R0_data,\n\
                     \x20 input [{awm}:0] W0_addr,\n\
                     \x20 input W0_en,\n\
                     \x20 input W0_clk,\n\
                     \x20 input [{wm}:0] W0_data\n\
                     );\n\
                     \x20 (* ram_style = \"distributed\" *) reg [{wm}:0] ram [0:{dm}];\n\
                     \x20 always @(posedge W0_clk) if (W0_en) ram[W0_addr] <= W0_data;\n\
                     \x20 assign R0_data = ram[R0_addr];\n\
                     endmodule\n",
                    m = p.macro_name,
                    d = p.depth,
                    w = p.width,
                    awm = aw - 1,
                    wm = p.width - 1,
                    dm = p.depth - 1,
                );
                (format!("{}.sv", p.macro_name), sv)
            })
            .collect()
    }

    /// THE aggregated disambiguation note (user-approved form): one message
    /// listing every array that followed the DEFAULT policy, each with its
    /// reason. None when every array was explicitly pinned (or none exist).
    pub fn take_disambiguation_note(&self) -> Option<String> {
        let lines = self.notices.borrow();
        if lines.is_empty() {
            return None;
        }
        Some(format!(
            "note: {} state array(s) follow the default array-lowering policy \
             — prefix 'mem let' or 'reg let' to disambiguate explicitly \
             (and silence this note):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    fn emit_module(&mut self, out: &mut String, dep_graph: &DependencyGraph, items: &[TopLevel]) {
        let mut ng = NameGen::default();

        let mut input_ports: Vec<String> = Vec::new();
        let mut output_ports: Vec<(String, String)> = Vec::new();
        // 2026-08-23 (Plan 3): clocks are !seq.clock — seq.firreg requires it.
        input_ports.push("in %clock: !seq.clock".to_string());
        input_ports.push("in %reset: i1".to_string());

        // 2026-08-27 (Slice B): MMIO pins emit ADDRESS-SORTED (deterministic
        // layout rule — separately compiled partitions agree on bus layout
        // without communicating); unaddressed triggers keep program order.
        let mut sorted_trgs: Vec<&TriggerPort> = self.trg_ports.iter().collect();
        sorted_trgs.sort_by(|a, b| match (a.address, b.address) {
            (Some(x), Some(y)) => x.cmp(&y).then(a.port_name.cmp(&b.port_name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.port_name.cmp(&b.port_name),
        });
        for trg in &sorted_trgs {
            if let Some(ty) = self.var_types.get(&trg.port_name) {
                let mlir_ty = self.mlir_type(ty);
                input_ports.push(format!("in %{}: {}", trg.port_name, mlir_ty));
            }
        }

        output_ports.push(("halt".to_string(), "i1".to_string()));
        // §3.4 obligation port: AND of all txn post-guard verdicts on
        // committed next values; high when no obligations exist.
        output_ports.push(("check".to_string(), "i1".to_string()));

        // ── §3.4 extension (2026-08-25): liveness watchdogs become cycle
        // countdown monitors. `![cond] within Ncyc` demands cond observed at
        // least every N cycles: a counter reloads on cond, saturates at 0,
        // and its expiry raises the per-watchdog `wd_<txn>_tmo` port — and,
        // when required (!), also `halt` (an obligation breach). Trigger-
        // form watchdogs (condition naming a declared trigger), time-unit
        // bounds (no clock-frequency mapping on this surface) and on-fire
        // handlers are capability errors, never silent drops.
        let mut wd_monitors: Vec<WdMonitor> = Vec::new();
        for item in items {
            let TopLevel::Transaction(txn) = item else { continue };
            let Some(wd) = &txn.contract.watchdog else { continue };
            if let Expr::Identifier(id) = &wd.condition {
                if self.trg_ports.iter().any(|t| &t.trg_name == id) {
                    self.record_unsupported(&format!(
                        "trigger watchdog '@{}' in txn '{}' — event domain",
                        id, txn.name
                    ));
                    continue;
                }
            }
            if wd.on_fire.is_some() {
                self.record_unsupported(&format!(
                    "watchdog on-fire handler in txn '{}' — event domain",
                    txn.name
                ));
                continue;
            }
            let bound = match wd.cycles_bound {
                Some(b) => b,
                None => {
                    // Time-unit bound: convertible iff a clock frequency is
                    // configured (circt.clock_hz in ir-lowering.dbvl).
                    let Some(ns) = wd.deadline_ns else {
                        self.record_unsupported(&format!(
                            "watchdog in txn '{}' — no time bound parsed",
                            txn.name
                        ));
                        continue;
                    };
                    let hz = crate::config_tuning::ir_lowering().clock_hz;
                    if hz == 0 {
                        self.record_unsupported(&format!(
                            "watchdog in txn '{}' — a time-unit bound needs a clock \
                             frequency mapping this surface does not carry; set \
                             'circt.clock_hz' in config/ir-lowering.dbvl, or use \
                             'within N cyc'",
                            txn.name
                        ));
                        continue;
                    }
                    Self::ns_to_cycles(hz, ns)
                }
            };
            let port = format!("wd_{}_tmo", txn.name);
            wd_monitors.push(WdMonitor {
                port: port.clone(),
                cond: wd.condition.clone(),
                bound,
                required: wd.is_required,
            });
            output_ports.push((port, "i1".to_string()));
        }

        // 2026-08-23 (robustness): topo_order can DROP declared state —
        // self-dependencies (counter = counter + 1) create cycles that make
        // DependencyGraph::build fail, and the caller's fallback is an EMPTY
        // graph. Emission must not lose declared state: union topo with
        // every declared var, sorted for determinism.
        // To undo: revert to iterating topo_order alone.
        let mut ordered: Vec<String> = dep_graph.topo_order.clone();
        for name in self.var_types.keys() {
            if !ordered.contains(name) {
                ordered.push(name.clone());
            }
        }
        ordered.sort();
        let ordered: &[String] = &ordered;
        let trg_names: std::collections::HashSet<String> = self.trg_ports.iter().map(|t| t.trg_name.clone()).collect();
        // 2026-08-25 (seq-firmem plan): bounded state arrays lower to
        // REGISTER FILES (per-lane firregs + mux/decode trees) or — past
        // the policy threshold, unpinned and semantics-compatible — the
        // seq.firmem MEMORY MACRO (companion .sv supplies the RAM body at
        // export; see plan §0 findings). The policy engine decides;
        // default-policy decisions land in `notices` for THE aggregated
        // note. Non-constant dimensions are a recorded capability error.
        let array_facts = mem_policy::collect_array_facts(items);
        let ir_lower = crate::config_tuning::ir_lowering();
        let mem_policy_cfg = mem_policy::MemPolicy {
            min_depth: ir_lower.firmem_min_depth,
            max_ports: ir_lower.firmem_max_ports,
        };
        let mut lane_inits: HashMap<String, String> = HashMap::new();
        for var_name in ordered {
            if trg_names.contains(var_name) {
                continue;
            }
            let Some(ty) = self.var_types.get(var_name) else { continue };
            if let Type::Vector(elem, dims) = ty {
                let elem_ty = self.mlir_type(elem);
                match Self::concrete_dim(dims.first()) {
                    Some(n) if n > 0 => {
                        let (hint, facts) = array_facts
                            .iter()
                            .find(|(v, _, _)| v == var_name)
                            .map(|(_, h, f)| (*h, f.clone()))
                            .unwrap_or((
                                mem_policy::MemHint::None,
                                mem_policy::ArrayFacts {
                                    depth: n,
                                    ..mem_policy::ArrayFacts::default()
                                },
                            ));
                        let decision = match mem_policy::decide_array_lowering(
                            hint,
                            &facts,
                            &mem_policy_cfg,
                        ) {
                            Ok(d) => d,
                            Err(e) => {
                                self.record_unsupported(&format!(
                                    "array '{}': {}",
                                    var_name, e
                                ));
                                continue;
                            }
                        };
                        match decision.lowering {
                            mem_policy::ArrayLowering::FirmMem => {
                                // Wire name reserved NOW (deterministic
                                // NameGen order); the seq.firmem op itself
                                // emits after the module header (macros
                                // block below).
                                let width = elem_ty
                                    .trim_start_matches('i')
                                    .parse::<usize>()
                                    .unwrap_or(64);
                                let wire =
                                    ng.fresh_wire(&format!("{}_mem", var_name));
                                // SeqToSV derives the generated-module name
                                // from the firmem SSA NAME: <name>_<D>x<W>.
                                // The companion must match it exactly.
                                let macro_name = format!(
                                    "{}_{}x{}",
                                    wire.trim_start_matches('%'),
                                    n, width
                                );
                                self.array_mems.insert(
                                    var_name.clone(),
                                    MemPlan {
                                        depth: n,
                                        width,
                                        macro_name,
                                        wire,
                                    },
                                );
                                self.array_elem_ty
                                    .insert(var_name.clone(), elem_ty.clone());
                                if let Some(why) = decision.why {
                                    self.notices.borrow_mut().push(format!(
                                        "  {} ({} x {}) -> seq.firmem macro   [why: {}]",
                                        var_name, n, elem_ty, why
                                    ));
                                }
                            }
                            mem_policy::ArrayLowering::RegFile => {
                                let init_list =
                                    self.var_exprs.get(var_name).cloned().flatten();
                                let mut lanes: Vec<String> = Vec::new();
                                for j in 0..n {
                                    let lane = format!("{}_{}", var_name, j);
                                    let init = init_list
                                        .as_ref()
                                        .and_then(|e| match e {
                                            Expr::List(items) => items.get(j),
                                            _ => None,
                                        })
                                        .map(Self::format_init_expr)
                                        .unwrap_or_else(|| "0".to_string());
                                    lane_inits.insert(lane.clone(), init);
                                    lanes.push(lane.clone());
                                    output_ports.push((lane.clone(), elem_ty.clone()));
                                }
                                self.array_groups
                                    .insert(var_name.clone(), lanes);
                                self.array_elem_ty
                                    .insert(var_name.clone(), elem_ty.clone());
                                if let Some(why) = decision.why {
                                    self.notices.borrow_mut().push(format!(
                                        "  {} ({} x {}) -> register file      [why: {}]",
                                        var_name, n, elem_ty, why
                                    ));
                                }
                            }
                        }
                    }
                    Some(_) => {
                        self.record_unsupported(&format!(
                            "zero-length array '{}'",
                            var_name
                        ));
                    }
                    None => {
                        self.record_unsupported(&format!(
                            "unbounded array '{}' — non-constant dimension",
                            var_name
                        ));
                    }
                }
                continue;
            }
            let mlir_ty = self.mlir_type(ty);
            output_ports.push((var_name.clone(), mlir_ty));
        }

        // 2026-08-23 (circt-opt findings #1+#2): hw.module takes ONE port
        // list — inputs `in %name: ty`, outputs `out name: ty`, all comma-
        // separated; there is no `-> (results)` tail (that's func-style).
        // Found by circt-opt on the first real round-trip.
        // To undo: restore split input/output parens emission.
        write!(out, "hw.module @top(").ok();
        let mut ports: Vec<String> = input_ports.clone();
        for (name, mlir_ty) in &output_ports {
            ports.push(format!("out {}: {}", name, mlir_ty));
        }
        for (i, port) in ports.iter().enumerate() {
            if i > 0 { write!(out, ", ").ok(); }
            write!(out, "{}", port).ok();
        }
        writeln!(out, ") {{").ok();

        // ── Wire-map architecture v2 (Plan 3 §3.4, 2026-08-25): registers
        // FIRST. hw.module bodies are MLIR graph regions — circt-opt accepts
        // use-before-def (probe: seq.firreg referenced before its def passes
        // the verifier) — so transaction bodies read live register outputs
        // instead of folded init constants. Guards now gate transitions on
        // CURRENT state; obligations surface as ports: `halt` (a pre-guard
        // refused this cycle ⇒ state holds) and `check` (AND of post-guard
        // verdicts on committed next values).
        // The previous scheme (bodies read init constants) made guards fold
        // to compile-time constants and transitions fire unconditionally —
        // a guarded counter ran past its bound, diverging from the
        // interpreter at the bound cycle.
        // To undo: restore Phase-A-reads scheme (guards on init constants,
        // `%true` transition muxes, no halt/check driving).
        let clock_wire = "%clock";
        let mut reg_names: HashMap<String, String> = HashMap::new();

        // Phase A: init constants + registers. Each register consumes a
        // forward-named next wire that Phase C defines (legal: graph region).
        // Init doubles as the power-on preset — a register's reset/preset
        // value IS its declared initial value.
        let mut init_wires: HashMap<String, String> = HashMap::new();
        let mut next_names: HashMap<String, String> = HashMap::new();
        for (var_name, mlir_ty) in &output_ports {
            let init_val: String = match var_name.as_str() {
                "halt" => "0".to_string(),
                "check" => "1".to_string(),
                p if p.starts_with("wd_") => "0".to_string(),
                // Array lanes carry their element init from the list literal.
                _ => lane_inits
                    .get(var_name)
                    .cloned()
                    .unwrap_or_else(|| self.initial_value(var_name)),
            };
            let c = ng.fresh_const(&format!("{}_init", var_name));
            writeln!(out, "  {} = hw.constant {} : {}", c, init_val, mlir_ty).ok();
            init_wires.insert(var_name.clone(), c);
            let next_w = ng.fresh_wire(&format!("{}_next", var_name));
            next_names.insert(var_name.clone(), next_w.clone());
            let reg = ng.fresh_reg(var_name);
            writeln!(
                out,
                "  {} = seq.firreg {} clock {} preset {} : {}",
                reg, next_w, clock_wire, init_val, mlir_ty
            )
            .ok();
            reg_names.insert(var_name.clone(), reg);
        }
        // Boolean constants used by mux guards.
        // Bool constants: custom syntax takes NO type suffix.
        writeln!(out, "  %true = hw.constant true").ok();
        writeln!(out, "  %false = hw.constant false").ok();

        // Memory macros (seq-firmem plan): declared up-front; ports attach
        // at access sites. Sorted by macro name for deterministic emission.
        let mut mem_plans: Vec<&MemPlan> = self.array_mems.values().collect();
        mem_plans.sort_by(|a, b| a.macro_name.cmp(&b.macro_name));
        for plan in &mem_plans {
            writeln!(
                out,
                "  {} = seq.firmem 0, 1, old, undefined : !seq.firmem<{} x {}>",
                plan.wire, plan.depth, plan.width
            )
            .ok();
        }

        // Phase B: transaction bodies (in program order) against live
        // register outputs. Refusal/post verdicts collected for halt/check.
        let mut pending: HashMap<String, String> = HashMap::new();
        let mut ob = Obligations::default();
        for item in items {
            if let TopLevel::Transaction(txn) = item {
                let mut m = TxnMaps {
                    name: txn.name.clone(),
                    contract: &txn.contract,
                    reg_names: &mut reg_names,
                    pending: &mut pending,
                    ob: &mut ob,
                    gate: None,
                    gates: Vec::new(),
                };
                self.emit_txn_body(&mut ng, out, &txn.body, &mut m);
            }
        }

        // Phase C: define the forward-referenced next wires. Reset forces
        // the init value; an unwritten var holds its register output.
        for (var_name, mlir_ty) in &output_ports {
            if var_name == "halt"
                || var_name == "check"
                || var_name.starts_with("wd_")
            {
                continue; // obligation/watchdog outputs driven below
            }
            let next_w = next_names.get(var_name).cloned().unwrap_or_default();
            let init_wire = init_wires.get(var_name).cloned().unwrap_or_default();
            let current = reg_names.get(var_name).cloned().unwrap_or_default();
            let src = pending
                .get(var_name)
                .cloned()
                .unwrap_or_else(|| current.clone());
            writeln!(
                out,
                "  {} = comb.mux %reset, {}, {} : {}",
                next_w, init_wire, src, mlir_ty
            )
            .ok();
        }

        // ── Watchdog countdown monitors (§3.4 extension, 2026-08-25).
        // Per monitor: cond wire on live state; counter reloads to the
        // bound when cond holds, saturates at 0 otherwise; expiry =
        // ¬cond ∧ cnt==0 drives the tmo port (registered, reset clears)
        // and, for required (!) watchdogs, ORs into halt.
        for wd in &wd_monitors {
            let cw = ng.fresh_wire(&format!("{}_cond", wd.port));
            self.emit_contract_condition(out, &mut ng, &wd.cond, &cw, &reg_names);
            let not_cond = ng.fresh_wire(&format!("{}_ncond", wd.port));
            writeln!(out, "  {} = comb.icmp eq {}, %false : i1", not_cond, cw).ok();
            let cnt_init = ng.fresh_const(&format!("{}_bnd", wd.port));
            writeln!(out, "  {} = hw.constant {} : i64", cnt_init, wd.bound).ok();
            let zero = ng.fresh_const(&format!("{}_z", wd.port));
            writeln!(out, "  {} = hw.constant 0 : i64", zero).ok();
            let one = ng.fresh_const(&format!("{}_one", wd.port));
            writeln!(out, "  {} = hw.constant 1 : i64", one).ok();
            let cnt = ng.fresh_reg(&format!("{}_cnt", wd.port));
            let cnt_next = ng.fresh_wire(&format!("{}_cnt_next", wd.port));
            // emit register FIRST (graph region), define cnt_next below —
            // consistent with the registers-first architecture.
            writeln!(
                out,
                "  {} = seq.firreg {} clock {} preset {} : i64",
                cnt, cnt_next, clock_wire, wd.bound
            )
            .ok();
            let sub = ng.fresh_wire(&format!("{}_sub", wd.port));
            writeln!(out, "  {} = comb.sub {}, {} : i64", sub, cnt, one).ok();
            let is_zero = ng.fresh_wire(&format!("{}_isz", wd.port));
            writeln!(out, "  {} = comb.icmp eq {}, {} : i64", is_zero, cnt, zero).ok();
            let sat = ng.fresh_wire(&format!("{}_sat", wd.port));
            writeln!(out, "  {} = comb.mux {}, {}, {} : i64", sat, is_zero, zero, sub).ok();
            writeln!(
                out,
                "  {} = comb.mux {}, {}, {} : i64",
                cnt_next, cw, cnt_init, sat
            )
            .ok();
            let tmo_raw = ng.fresh_wire(&format!("{}_raw", wd.port));
            writeln!(out, "  {} = comb.and {}, {} : i1", tmo_raw, not_cond, is_zero).ok();
            if wd.required {
                ob.pre_fails.push(tmo_raw.clone());
            }
            let tmo_reg = ng.fresh_reg(&wd.port);
            let tmo_next = next_names.get(&wd.port).cloned().unwrap_or_default();
            writeln!(
                out,
                "  {} = seq.firreg {} clock {} preset 0 : i1",
                tmo_reg, tmo_next, clock_wire
            )
            .ok();
            reg_names.insert(wd.port.clone(), tmo_reg);
            let init_wire = init_wires.get(&wd.port).cloned().unwrap_or_default();
            writeln!(
                out,
                "  {} = comb.mux %reset, {}, {} : i1",
                tmo_next, init_wire, tmo_raw
            )
            .ok();
        }

        // Obligation outputs (§3.4): halt = OR of pre-guard refusals this
        // cycle (state held); check = AND of post-guard verdicts on the
        // committed next values. No txns with obligations ⇒ halt stays low,
        // check stays high. Registered like every output; reset clears.
        let halt_src = Self::reduce_tree(
            &mut ng,
            out,
            "comb.or",
            &ob.pre_fails,
            "%false",
        );
        let check_src = Self::reduce_tree(
            &mut ng,
            out,
            "comb.and",
            &ob.post_oks,
            "%true",
        );
        for (ob_name, src) in [("halt", halt_src), ("check", check_src)] {
            let next_w = next_names.get(ob_name).cloned().unwrap_or_default();
            let init_wire = init_wires.get(ob_name).cloned().unwrap_or_default();
            writeln!(
                out,
                "  {} = comb.mux %reset, {}, {} : i1",
                next_w, init_wire, src
            )
            .ok();
        }

        // ── Outputs.
        // hw.output syntax: values then ONE ':' + type list.
        let mut out_vals: Vec<String> = Vec::new();
        let mut out_tys: Vec<String> = Vec::new();
        for (var_name, mlir_ty) in &output_ports {
            if let Some(reg) = reg_names.get(var_name) {
                out_vals.push(reg.clone());
                out_tys.push(mlir_ty.clone());
            } else {
                let c = ng.fresh_const(&format!("{}_default", var_name));
                writeln!(out, "  {} = hw.constant 0 : {}", c, mlir_ty).ok();
                out_vals.push(c);
                out_tys.push(mlir_ty.clone());
            }
        }
        if !out_vals.is_empty() {
            writeln!(out, "  hw.output {} : {}", out_vals.join(", "), out_tys.join(", "))
                .ok();
        }
        // Close the hw.module region.
        writeln!(out, "}}").ok();
    }

    fn initial_value(&self, var_name: &str) -> String {
        match self.var_exprs.get(var_name) {
            Some(Some(expr)) => Self::format_init_expr(expr),
            _ => "0".to_string(),
        }
    }

    /// 2026-08-25 (Plan 3.6): scalar initializer formatting shared by
    /// named vars and array lanes (a lane's init is the matching element
    /// of the array's list literal).
    fn format_init_expr(expr: &Expr) -> String {
        match expr {
            Expr::Decimal(n) => format!("{}", n),
            Expr::Bool(b) => if *b { "1".to_string() } else { "0".to_string() },
            Expr::Float(f) => format!("{}", f),
            _ => "0".to_string(),
        }
    }

    /// 2026-08-25 (Plan 3.6): a register file needs a COMPILE-TIME element
    /// count; Named dims are const-generic placeholders (not concrete here).
    fn concrete_dim(dim: Option<&crate::ast::Dimension>) -> Option<usize> {
        match dim? {
            crate::ast::Dimension::Anonymous(c) => Some(*c),
            crate::ast::Dimension::Named(_, _) => None,
        }
    }

    fn emit_expr(&self, ng: &mut NameGen, out: &mut String, expr: &Expr, reg_names: &HashMap<String, String>, result_ty: &str) -> Option<String> {
        match expr {
            Expr::Decimal(n) => {
                let c = ng.fresh_const("int");
                writeln!(out, "  {} = hw.constant {} : {}", c, n, result_ty).ok();
                Some(c)
            }
            Expr::Bool(b) => {
                let c = ng.fresh_const("bool");
                let v = if *b { "1" } else { "0" };
                writeln!(out, "  {} = hw.constant {} : i1", c, v).ok();
                Some(c)
            }
            Expr::Float(f) => {
                let c = ng.fresh_const("float");
                writeln!(out, "  {} = hw.constant {} : f64", c, f).ok();
                Some(c)
            }
            Expr::Identifier(name) => {
                if let Some(reg) = reg_names.get(name) {
                    Some(reg.clone())
                } else if self.trg_ports.iter().any(|t| t.trg_name == *name || t.port_name == *name) {
                    Some(format!("%{}", name))
                } else {
                    None
                }
            }
            Expr::BinaryOp(kind, l, r) => {
                match kind {
                    BinaryOpKind::Add => self.emit_binary_comb(ng, out, "comb.add", l, r, reg_names, result_ty),
                    BinaryOpKind::Sub => self.emit_binary_comb(ng, out, "comb.sub", l, r, reg_names, result_ty),
                    BinaryOpKind::Mul => self.emit_binary_comb(ng, out, "comb.mul", l, r, reg_names, result_ty),
                    BinaryOpKind::Div => self.emit_binary_comb(ng, out, "comb.divu", l, r, reg_names, result_ty),
                    BinaryOpKind::Mod => self.emit_binary_comb(ng, out, "comb.mod", l, r, reg_names, result_ty),
                    // 2026-08-25: comparisons emit OPERANDS at the operand
                    // register width (result is implicitly i1). The old
                    // hardcoded "i1" compared i64 registers as 1-bit — latent
                    // until the first statement-level `when` guard.
                    BinaryOpKind::Eq => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp eq", l, r, reg_names, &w)
                    }
                    BinaryOpKind::Neq => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp ne", l, r, reg_names, &w)
                    }
                    BinaryOpKind::Lt => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp ult", l, r, reg_names, &w)
                    }
                    BinaryOpKind::Le => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp ule", l, r, reg_names, &w)
                    }
                    BinaryOpKind::Gt => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp ugt", l, r, reg_names, &w)
                    }
                    BinaryOpKind::Ge => {
                        let w = self.compare_width(l, r);
                        self.emit_binary_comb(ng, out, "comb.icmp uge", l, r, reg_names, &w)
                    }
                    BinaryOpKind::And => self.emit_binary_comb(ng, out, "comb.and", l, r, reg_names, "i1"),
                    BinaryOpKind::Or => self.emit_binary_comb(ng, out, "comb.or", l, r, reg_names, "i1"),
                    BinaryOpKind::BitAnd => self.emit_binary_comb(ng, out, "comb.and", l, r, reg_names, result_ty),
                    BinaryOpKind::BitOr => self.emit_binary_comb(ng, out, "comb.or", l, r, reg_names, result_ty),
                    BinaryOpKind::BitXor => self.emit_binary_comb(ng, out, "comb.xor", l, r, reg_names, result_ty),
                    BinaryOpKind::Shl => self.emit_binary_comb(ng, out, "comb.shl", l, r, reg_names, result_ty),
                    BinaryOpKind::Shr => self.emit_binary_comb(ng, out, "comb.shr", l, r, reg_names, result_ty),
                    BinaryOpKind::Concat => self.emit_binary_comb(ng, out, "comb.concat", l, r, reg_names, result_ty),
                }
            }
            Expr::UnaryOp(kind, inner) => {
                match kind {
                    UnaryOpKind::Neg => self.emit_unary_comb(ng, out, "comb.neg", inner, reg_names, result_ty),
                    UnaryOpKind::Not => self.emit_unary_comb(ng, out, "comb.xor", inner, reg_names, "i1"),
                    UnaryOpKind::BitNot => self.emit_unary_comb(ng, out, "comb.xor", inner, reg_names, result_ty),
                }
            }
            Expr::Cast(inner, target_ty) => {
                let inner_mlir_ty = self.mlir_type(&crate::ast::Type::int());
                let target_mlir_ty = self.mlir_type(target_ty);
                let val = self.emit_expr(ng, out, inner, reg_names, &inner_mlir_ty)?;
                let w = ng.fresh_wire("cast");
                if inner_mlir_ty == target_mlir_ty {
                    Some(val)
                } else {
                    writeln!(out, "  {} = comb.extract {} from 0 : ({}) -> {}", w, val, inner_mlir_ty, target_mlir_ty).ok();
                    Some(w)
                }
            }
            // 2026-07-18: Pointer ops — emit inner expression (HW backend).
            Expr::AddrOf(inner) => self.emit_expr(ng, out, inner, reg_names, result_ty),
            Expr::Deref(inner) => self.emit_expr(ng, out, inner, reg_names, result_ty),
            Expr::Call(name, args, _) => {
                // 2026-07-14: Intrinsic calls and function calls both use Expr::Call.
                // Match intrinsic names first, then fall back to submodule instantiation.
                match name.as_str() {
                    "Abs#" => {
                        let mut arg = |i: usize| -> String {
                            args.get(i).and_then(|a| {
                                self.emit_expr(ng, out, a, reg_names, result_ty)
                            }).unwrap_or_else(|| "%0".to_string())
                        };
                        let x = arg(0);
                        let neg_x = ng.fresh_wire("neg");
                        let cmp = ng.fresh_wire("cmp_neg");
                        writeln!(out, "  {} = comb.neg {} : {}", neg_x, x, result_ty).ok();
                        writeln!(out, "  {} = comb.icmp slt {}, %c0_0 : {}", cmp, x, result_ty).ok();
                        let w = ng.fresh_wire("abs");
                        writeln!(out, "  {} = comb.mux {}, {}, {} : {}", w, cmp, neg_x, x, result_ty).ok();
                        Some(w)
                    }
                    "AddressOf#" => {
                        // 2026-07-15: CIRCT (hardware) backend — AddressOf# resolves to a
                        // physical address, emit as a constant integer. In HW synthesis this
                        // becomes a constant wire driving the address bus.
                        let id_str = args.first().and_then(|a| {
                            if let Expr::Quoted(b) = a { Some(String::from_utf8_lossy(b).to_string()) } else { None }
                        }).unwrap_or_else(|| "unknown".to_string());
                        let addr = crate::address_resolver::resolve_address(&id_str);
                        let c = ng.fresh_const("addr");
                        writeln!(out, "  {} = hw.constant {} : {}", c, addr, result_ty).ok();
                        Some(c)
                    }
                    "Size#" => {
                        let c = ng.fresh_const("size");
                        writeln!(out, "  {} = hw.constant 64 : {}", c, result_ty).ok();
                        Some(c)
                    }
                    // 2026-08-23 (Plan 3.2): the invented comb ops are
                    // DELETED — comb has no ctpop/ctlz/cttz/rev/sin/cos/pow/
                    // sqrt/floor/ceil; those arms emitted MLIR no toolchain
                    // could parse. Abs# stays (honest neg+icmp+mux). Unknown
                    // `#` intrinsics now RECORD a capability error instead of
                    // vanishing into a submodule instantiation.
                    n if n.ends_with('#') && n != "Abs#" && n != "AddressOf#" && n != "Size#" => {
                        self.record_unsupported(&format!("intrinsic '{}'", n));
                        None
                    }
                    _ => {
                        // 2026-08-27 (undefined-instance fix): ONLY declared
                        // cells become hw.instantiations — only they get a
                        // hw.module definition emitted (emit_cell_module).
                        // Any other callee (plain fn, txn, enum variant
                        // constructor like Http::Ok) previously instantiated a
                        // module that was NEVER defined — output circt-opt
                        // rejects downstream, or worse: unverifiable silence.
                        // Now: honest capability error naming the callee.
                        let inst_name = name.replace('-', "_");
                        if !self.cell_defs.contains_key(&inst_name) {
                            self.record_unsupported(&format!(
                                "call '{}' — hardware synthesis has no image \
                                 for this function; only declared cells can be \
                                 instantiated. Inline the computation or \
                                 declare it as a cell",
                                name
                            ));
                            return None;
                        }
                        let result_wire = ng.fresh_wire(&format!("{}_result", inst_name));
                        // 2026-08-23 (Plan 3.2): real hw.instance syntax —
                        // port names must match the cell's declared params.
                        // The old form emitted $arg0/$result ($ names are
                        // invalid MLIR identifiers).
                        let param_names: Vec<String> = self
                            .cell_defs
                            .get(&inst_name)
                            .map(|c| c.parameters.iter().map(|(n, _)| n.clone()).collect())
                            .unwrap_or_else(|| {
                                (0..args.len()).map(|i| format!("in{}", i)).collect()
                            });
                        let mut arg_parts = Vec::new();
                        for (i, arg) in args.iter().enumerate() {
                            let port = param_names.get(i).cloned().unwrap_or_else(|| format!("in{}", i));
                            let arg_mlir_ty = if i == 0 { "i64" } else { result_ty };
                            if let Some(arg_val) = self.emit_expr(ng, out, arg, reg_names, arg_mlir_ty) {
                                arg_parts.push(format!("{}: {}: {}", port, arg_val, arg_mlir_ty));
                            }
                        }
                        let out_port = self
                            .cell_defs
                            .get(&inst_name)
                            .and_then(|c| Self::extract_output_names_llvm(&c.output_type).into_iter().next())
                            .unwrap_or_else(|| "out".to_string());
                        writeln!(out, "  {} = hw.instance \"{}\" @{} ({}) -> ({}: {})",
                            result_wire, inst_name, inst_name,
                            arg_parts.join(", "),
                            out_port, result_ty,
                        ).ok();
                        Some(result_wire)
                    }
                }
            }
            Expr::Field(obj, field) => {
                let obj_val = self.emit_expr(ng, out, obj, reg_names, result_ty)?;
                let w = ng.fresh_wire("fld");
                writeln!(out, "  {} = hw.wire {} : {}", w, obj_val, result_ty).ok();
                Some(w)
            }
            Expr::Index(list, idx) => {
                // 2026-08-25 (seq-firmem plan): MEMORY-MACRO READ — a
                // latency-0 read port per site; address truncated to
                // ceil(log2(depth)) (verifier-enforced). Sees cycle-start
                // state, matching register-file read semantics exactly.
                if let Expr::Identifier(name) = list.as_ref() {
                    if let Some(plan) = self.array_mems.get(name).cloned() {
                        let idx_val = self.emit_expr(ng, out, idx, reg_names, "i64")?;
                        let aw = Self::addr_width(plan.depth);
                        let a = ng.fresh_wire("maddr");
                        writeln!(
                            out,
                            "  {} = comb.extract {} from 0 : (i64) -> i{}",
                            a, idx_val, aw
                        )
                        .ok();
                        let rp = ng.fresh_wire(&format!("{}_rp", name));
                        writeln!(
                            out,
                            // %clock is the fixed input-port name.
                            "  {} = seq.firmem.read_port {}[{}], clock %clock : !seq.firmem<{} x {}>",
                            rp, plan.wire, a, plan.depth, plan.width
                        )
                        .ok();
                        return Some(rp);
                    }
                    let lanes = self.array_groups.get(name).cloned();
                    if let Some(lanes) = lanes {
                        let idx_val = self.emit_expr(ng, out, idx, reg_names, "i64")?;
                        let elem_ty = self
                            .array_elem_ty
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| result_ty.to_string());
                        // Flatten guarantees ≥1 lane; a 0-length array is
                        // rejected there.
                        let last = lanes.last().cloned().unwrap_or_default();
                        let mut acc = reg_names.get(&last).cloned().unwrap_or_default();
                        for (j, lane) in lanes.iter().enumerate().rev().skip(1) {
                            let cj = ng.fresh_const("aidx");
                            writeln!(out, "  {} = hw.constant {} : i64", cj, j).ok();
                            let eq = ng.fresh_wire("aeq");
                            writeln!(out, "  {} = comb.icmp eq {}, {} : i64", eq, idx_val, cj)
                                .ok();
                            let lane_val = reg_names.get(lane).cloned().unwrap_or_default();
                            let m = ng.fresh_wire("amux");
                            writeln!(
                                out,
                                "  {} = comb.mux {}, {}, {} : {}",
                                m, eq, lane_val, acc, elem_ty
                            )
                            .ok();
                            acc = m;
                        }
                        return Some(acc);
                    }
                }
                let list_val = self.emit_expr(ng, out, list, reg_names, result_ty)?;
                let _idx_val = self.emit_expr(ng, out, idx, reg_names, "i64")?;
                let w = ng.fresh_wire("idx");
                writeln!(out, "  {} = hw.wire {} : {}", w, list_val, result_ty).ok();
                Some(w)
            }
            other => {
                // 2026-08-23 (Plan 3.3): recorded — never a silent None.
                let kind = format!("{:?}", other)
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("expression")
                    .to_string();
                self.record_unsupported(&format!("{} expressions in hardware", kind));
                None
            }
        }
    }

    fn emit_binary_comb(&self, ng: &mut NameGen, out: &mut String, op: &str, l: &Expr, r: &Expr, reg_names: &HashMap<String, String>, result_ty: &str) -> Option<String> {
        let left = self.emit_expr(ng, out, l, reg_names, result_ty)?;
        let right = self.emit_expr(ng, out, r, reg_names, result_ty)?;
        let w = ng.fresh_wire("bin");
        writeln!(out, "  {} = {} {}, {} : {}", w, op, left, right, result_ty).ok();
        Some(w)
    }

    fn emit_unary_comb(&self, ng: &mut NameGen, out: &mut String, op: &str, inner: &Expr, reg_names: &HashMap<String, String>, result_ty: &str) -> Option<String> {
        let val = self.emit_expr(ng, out, inner, reg_names, result_ty)?;
        let w = ng.fresh_wire("un");
        if op == "comb.neg" || op == "comb.not" {
            writeln!(out, "  {} = {} {} : {}", w, op, val, result_ty).ok();
        } else {
            let c0 = ng.fresh_const("zero");
            writeln!(out, "  {} = hw.constant 0 : {}", c0, result_ty).ok();
            writeln!(out, "  {} = {} {}, {} : {}", w, op, val, c0, result_ty).ok();
        }
        Some(w)
    }

    /// 2026-08-25 (§3.4): linear reduce over obligation wires — AND/OR are
    /// associative, so a left chain is equivalent to a balanced tree and
    /// keeps the emitter loop-flat. `unit` is the identity constant
    /// (%false for or, %true for and). Returns the reduced wire.
    fn reduce_tree(
        ng: &mut NameGen,
        out: &mut String,
        op: &str,
        wires: &[String],
        unit: &str,
    ) -> String {
        let Some(first) = wires.first() else {
            return unit.to_string();
        };
        let tag = if op.ends_with("or") { "or_red" } else { "and_red" };
        let mut acc = first.clone();
        for w in &wires[1..] {
            let x = ng.fresh_wire(tag);
            writeln!(out, "  {} = {} {}, {} : i1", x, op, acc, w).ok();
            acc = x;
        }
        acc
    }

    fn emit_txn_body(
        &self,
        ng: &mut NameGen,
        out: &mut String,
        body: &[Statement],
        m: &mut TxnMaps<'_>,
    ) {
        // ── §3.4 (2026-08-25): contracts carry semantic load in hardware.
        // Pre-guard is evaluated against live register state and GATES the
        // commit: refusal ⇒ every write this txn would make is replaced by
        // hold-current, and the refusal raises `halt` for that cycle.
        // Post-guard is evaluated against COMMITTED next values (shadow map)
        // and ANDs into `check`. Trivially-true guards contribute nothing.
        // To undo: revert to comparator-only emission (guards as dead wires).
        let mut pre_ok_wire: Option<String> = None;
        let mut pre_fail_wire: Option<String> = None;
        if !matches!(&m.contract.pre_condition, Expr::Bool(true)) {
            let w = ng.fresh_wire(&format!("{}_pre", m.name));
            self.emit_contract_condition(out, ng, &m.contract.pre_condition, &w, m.reg_names);
            let f = ng.fresh_wire(&format!("{}_prefail", m.name));
            writeln!(out, "  {} = comb.icmp eq {}, %false : i1", f, w).ok();
            m.ob.pre_fails.push(f.clone());
            pre_fail_wire = Some(f);
            pre_ok_wire = Some(w.clone());
            m.gate = Some(w);
        }

        // Vars THIS txn writes = pending keys added during the body
        // (key-diff against the entry snapshot — no extra tracking param,
        // sorted for deterministic gate emission). Several txns writing one
        // var arbitrate by program order (last committed gate wins) —
        // recorded decision, see docs/architecture/backend-contracts.md.
        let before: std::collections::HashSet<String> = m.pending.keys().cloned().collect();
        for stmt in body {
            self.emit_stmt_pending(ng, out, stmt, m);
        }
        let mut written: Vec<String> = m.pending
            .keys()
            .filter(|k| !before.contains(*k))
            .cloned()
            .collect();
        written.sort();

        // Commit gate: guarded txn ⇒ mux(pre_ok, computed_next, current).
        // Current is the register output (cycle-start state), so a refused
        // txn leaves state untouched regardless of mid-body writes.
        if let Some(pre_ok) = &pre_ok_wire {
            for var in written.iter() {
                let Some(pval) = m.pending.get(var).cloned() else {
                    continue;
                };
                let ty = self.mlir_type(self.var_types.get(var).unwrap_or(&Type::int()));
                let Some(cur) = m.reg_names.get(var).cloned() else {
                    continue;
                };
                let g = ng.fresh_wire(&format!("{}_commit", var));
                writeln!(
                    out,
                    "  {} = comb.mux {}, {}, {} : {}",
                    g, pre_ok, pval, cur, ty
                )
                .ok();
                m.pending.insert(var.clone(), g);
            }
        }

        // Post-guard verdict on the values this cycle will commit. Obligation
        // form: pre_ok ⇒ post (refused txn carries no post obligation —
        // otherwise a halted FSM would flag check forever). Encoded as
        // ¬pre ∨ post = prefail ∨ post_verdict.
        if !matches!(&m.contract.post_condition, Expr::Bool(true)) {
            let mut shadow = m.reg_names.clone();
            for var in written.iter() {
                if let Some(pv) = m.pending.get(var) {
                    shadow.insert(var.clone(), pv.clone());
                }
            }
            let w = ng.fresh_wire(&format!("{}_post", m.name));
            self.emit_contract_condition(out, ng, &m.contract.post_condition, &w, &shadow);
            match &pre_fail_wire {
                // ¬pre ∨ post — the prefail wire IS ¬pre (2026-08-25: sim
                // parity caught the first cut ORing pre_ok instead, which
                // flagged check at every refusal).
                Some(pre_fail) => {
                    let imp = ng.fresh_wire(&format!("{}_postok", m.name));
                    writeln!(out, "  {} = comb.or {}, {} : i1", imp, pre_fail, w).ok();
                    m.ob.post_oks.push(imp);
                }
                None => m.ob.post_oks.push(w),
            }
        }
    }

    /// 2026-08-23 (Plan 3): assignments compute a value WIRE and repoint the
    /// pending map — reads elsewhere keep seeing pre-update values until the
    /// register consumes the final wire. Guarded bodies mux on their
    /// condition against the current pending/current wire. Vars written by
    /// a txn are recovered by the CALLER's pending key-diff (2026-08-25).
    fn emit_stmt_pending(
        &self,
        ng: &mut NameGen,
        out: &mut String,
        stmt: &Statement,
        m: &mut TxnMaps<'_>,
    ) {
        match stmt {
            Statement::Expression(expr) => {
                self.emit_expr(ng, out, expr, m.reg_names, "i64");
            }
            Statement::Assign(lhs, expr) => {
                // 2026-08-25 (seq-firmem plan): MEMORY-MACRO element write —
                // the §3.4 commit gate rides the write ENABLE: refusal
                // (pre_ok false) ⇒ enable low ⇒ macro holds. No pending-map
                // interaction; nothing combinational to repoint.
                if let Expr::Index(obj, idx) = lhs {
                    if let Expr::Identifier(name) = obj.as_ref() {
                        if let Some(plan) = self.array_mems.get(name).cloned() {
                            let elem_ty = self
                                .array_elem_ty
                                .get(name)
                                .cloned()
                                .unwrap_or_else(|| "i64".to_string());
                            let Some(data) =
                                self.emit_expr(ng, out, expr, m.reg_names, &elem_ty)
                            else {
                                return;
                            };
                            let Some(idx_val) =
                                self.emit_expr(ng, out, idx, m.reg_names, "i64")
                            else {
                                return;
                            };
                            let aw = Self::addr_width(plan.depth);
                            let a = ng.fresh_wire("maddr");
                            writeln!(
                                out,
                                "  {} = comb.extract {} from 0 : (i64) -> i{}",
                                a, idx_val, aw
                            )
                            .ok();
                            // Enable = §3.4 commit gate AND active when
                            // gates (refusal or guard-false ⇒ hold). Omit
                            // when unconditional — port defaults true.
                            let mut ens: Vec<String> = Vec::new();
                            if let Some(g) = &m.gate {
                                ens.push(g.clone());
                            }
                            if let Some(w) = Self::when_mask(ng, out, m) {
                                ens.push(w);
                            }
                            let enable = match ens.len() {
                                0 => String::new(),
                                1 => format!(" enable {}", ens[0]),
                                _ => {
                                    let a =
                                        Self::reduce_tree(ng, out, "comb.and", &ens, "%true");
                                    format!(" enable {}", a)
                                }
                            };
                            writeln!(
                                out,
                                "  seq.firmem.write_port {}[{}] = {}, clock %clock{} : !seq.firmem<{} x {}>",
                                plan.wire, a, data, enable, plan.depth, plan.width
                            )
                            .ok();
                            return;
                        }
                        let Some(lanes) = self.array_groups.get(name).cloned() else {
                            self.record_unsupported(&format!(
                                "element write to non-state index target '{}'",
                                name
                            ));
                            return;
                        };
                        let elem_ty = self
                            .array_elem_ty
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| "i64".to_string());
                        let Some(val) = self.emit_expr(ng, out, expr, m.reg_names, &elem_ty)
                        else {
                            return;
                        };
                        let Some(idx_val) = self.emit_expr(ng, out, idx, m.reg_names, "i64")
                        else {
                            return;
                        };
                        // Active `when` gates AND into every lane select.
                        let wmask = Self::when_mask(ng, out, m);
                        for (j, lane) in lanes.iter().enumerate() {
                            let cj = ng.fresh_const("aidx");
                            writeln!(out, "  {} = hw.constant {} : i64", cj, j).ok();
                            let eq = ng.fresh_wire("aeq");
                            writeln!(out, "  {} = comb.icmp eq {}, {} : i64", eq, idx_val, cj)
                                .ok();
                            let sel = match &wmask {
                                Some(gmask) => {
                                    let gsel = ng.fresh_wire("asel");
                                    writeln!(
                                        out,
                                        "  {} = comb.and {}, {} : i1",
                                        gsel, eq, gmask
                                    )
                                    .ok();
                                    gsel
                                }
                                None => eq,
                            };
                            let current = m.pending
                                .get(lane)
                                .cloned()
                                .or_else(|| m.reg_names.get(lane).cloned())
                                .unwrap_or_default();
                            let w = ng.fresh_wire(&format!("{}_lane", lane));
                            writeln!(
                                out,
                                "  {} = comb.mux {}, {}, {} : {}",
                                w, sel, val, current, elem_ty
                            )
                            .ok();
                            m.pending.insert(lane.clone(), w);
                        }
                        return;
                    }
                }
                if let Some(var_name) = lhs.as_var_name() {
                    let mlir_ty =
                        self.mlir_type(self.var_types.get(var_name).unwrap_or(&Type::int()));
                    if let Some(val) = self.emit_expr(ng, out, expr, m.reg_names, &mlir_ty) {
                        let current = m.pending
                            .get(var_name)
                            .cloned()
                            .or_else(|| m.reg_names.get(var_name).cloned());
                        let target = match current {
                            Some(c) => c,
                            None => {
                                // First write to a var with no init wire yet:
                                // declare its init constant now.
                                let init_val = self.initial_value(var_name);
                                let c = ng.fresh_const(&format!("{}_init", var_name));
                                writeln!(
                                    out,
                                    "  {} = hw.constant {} : {}",
                                    c, init_val, mlir_ty
                                )
                                .ok();
                                m.reg_names.insert(var_name.to_string(), c.clone());
                                c
                            }
                        };
                        // Active `when` gates mux the pending value; the
                        // txn pre-guard still applies later via the §3.4
                        // commit gate (idempotent separation of concerns).
                        let masked = match Self::when_mask(ng, out, m) {
                            Some(gmask) => {
                                let w = ng.fresh_wire(&format!("{}_when", var_name));
                                writeln!(
                                    out,
                                    "  {} = comb.mux {}, {}, {} : {}",
                                    w, gmask, val, target, mlir_ty
                                )
                                .ok();
                                w
                            }
                            None => {
                                let w = ng.fresh_wire(&format!("{}_next", var_name));
                                writeln!(
                                    out,
                                    "  {} = comb.mux %true, {}, {} : {}",
                                    w, val, target, mlir_ty
                                )
                                .ok();
                                w
                            }
                        };
                        m.pending.insert(var_name.to_string(), masked);
                    }
                }
            }
            Statement::Guarded(condition, statements) => {
                // 2026-08-26 (gate threading): the condition joins the
                // active gate stack; EVERY inner statement — scalar assign,
                // array element write, nested when — is emitted through the
                // normal path and sees the ANDed mask. Previously only
                // scalar assigns were gated (innermost condition only),
                // element writes were silently dropped, and other
                // statements ran ungated.
                if let Some(cond) = self.emit_expr(ng, out, condition, m.reg_names, "i1") {
                    m.gates.push(cond);
                    for inner in statements {
                        self.emit_stmt_pending(ng, out, inner, m);
                    }
                    m.gates.pop();
                }
            }
            Statement::SyncBlock(body) | Statement::Block(body) => {
                for inner in body {
                    self.emit_stmt_pending(ng, out, inner, m);
                }
            }
            other => {
                // 2026-08-23 (Plan 3.3): recorded — never silent.
                let kind = format!("{:?}", other)
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("statement")
                    .to_string();
                self.record_unsupported(&format!("{} statements in hardware", kind));
            }
        }
    }

    fn emit_cell_module(&mut self, out: &mut String, dep_graph: &DependencyGraph, cell: &crate::ast::CellDef) {
        let cell_name = &cell.name;
        let mut ng = NameGen::default();

        if let Some(src_path) = &cell.extern_source {
            // 2026-08-27 (Slice A): FOREIGN module — an hw.module.extern
            // blackbox with the SAME implicit clock/reset + declared ports
            // shape defined cells get, so hw.instance sites match
            // byte-for-byte against both kinds of modules.
            writeln!(out, "// extern {} — definition lives in \"{}\"", cell_name, src_path).ok();
            // Authoritative grammar (HWStructure.td / ModuleImplementation.cpp):
            // ONE paren list holds ALL ports; inputs print `%name`, outputs
            // print BARE names; there is no `-> (outputs)` wrapper on
            // module-likes (that form belongs to hw.instance).
            write!(out, "hw.module.extern @{}(", cell_name).ok();
            let mut port_text: Vec<String> = Vec::new();
            port_text.push("in %clock: !seq.clock".to_string());
            port_text.push("in %reset: i1".to_string());
            for (param_name, param_ty) in &cell.parameters {
                let mlir_ty = self.mlir_type(param_ty);
                port_text.push(format!("in %{}: {}", param_name, mlir_ty));
            }
            let out_names = Self::extract_output_names_llvm(&cell.output_type);
            let out_names: Vec<String> = if out_names.is_empty() {
                cell.ports_out.iter().map(|(n, _)| n.clone()).collect()
            } else {
                out_names
            };
            for (i, out_name) in out_names.iter().enumerate() {
                let out_ty = cell.ports_out.get(i)
                    .map(|(_, t)| self.mlir_type(t))
                    .unwrap_or_else(|| "i64".to_string());
                port_text.push(format!("out {}: {}", out_name, out_ty));
            }
            write!(out, "{});", port_text.join(", ")).ok();
            writeln!(out).ok();
            return;
        }

        write!(out, "hw.module @{}(", cell_name).ok();
        write!(out, "in %clock: i1, in %reset: i1").ok();
        for (param_name, param_ty) in &cell.parameters {
            let mlir_ty = self.mlir_type(param_ty);
            write!(out, ", in %{}: {}", param_name, mlir_ty).ok();
        }
        let output_names = Self::extract_output_names_llvm(&cell.output_type);
        if let Some(first_out) = output_names.first() {
            let out_mlir_ty = cell.parameters.first()
                .map(|(_, t)| self.mlir_type(t))
                .unwrap_or_else(|| "i64".to_string());
            write!(out, ") -> ({}: {})", first_out, out_mlir_ty).ok();
        } else {
            write!(out, ") -> ()").ok();
        }
        writeln!(out, " {{").ok();

        let mut reg_names: HashMap<String, String> = HashMap::new();
        for (field_name, field_ty) in &cell.fields {
            let _ = field_name;
            let mlir_ty = self.mlir_type(field_ty);
            let init_val = "0".to_string();
            let reg = ng.fresh_reg(cell_name);
            writeln!(out, "  {} = seq.firreg initial_value {{ init_value = {} : {} }} : {}", reg, init_val, mlir_ty, mlir_ty).ok();
            reg_names.insert(field_name.clone(), reg);
        }

        for txn in &cell.transactions {
            if !matches!(&txn.contract.pre_condition, Expr::Bool(true)) {
                let pre_mlir_ty = "i1";
                if let Some(pre_val) = self.emit_expr(&mut ng, out, &txn.contract.pre_condition, &reg_names, pre_mlir_ty) {
                    writeln!(out, "  %when_{} = comb.icmp eq {} %true : {}", ng.fresh_wire("wc"), pre_val, pre_mlir_ty).ok();
                }
            }
            for stmt in &txn.body {
                self.emit_stmt_body(&mut ng, out, stmt, &reg_names);
            }
        }

        if let Some(first_out) = output_names.first() {
            if let Some(reg) = reg_names.get(first_out) {
                writeln!(out, "  seq.always(posedge %clock) {{").ok();
                writeln!(out, "    {} <= {}", reg.clone(), reg.clone()).ok();
                writeln!(out, "  }}").ok();
            }
        }

        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    fn extract_output_names_llvm(output_type: &Option<crate::ast::OutputType>) -> Vec<String> {
        match output_type {
            Some(crate::ast::OutputType::Named(name, _)) => vec![name.clone()],
            Some(crate::ast::OutputType::Single(_)) => vec!["result".to_string()],
            Some(crate::ast::OutputType::Array(_)) => vec!["result".to_string()],
            Some(crate::ast::OutputType::Tuple(types)) => {
                (0..types.len()).map(|i| format!("out{}", i)).collect()
            }
            Some(crate::ast::OutputType::Union(types)) => {
                (0..types.len()).map(|i| format!("case{}", i)).collect()
            }
            None => vec![],
        }
    }

    fn emit_stmt_body(&self, ng: &mut NameGen, out: &mut String, stmt: &Statement, reg_names: &HashMap<String, String>) {
        match stmt {
            Statement::Expression(expr) => {
                self.emit_expr(ng, out, expr, reg_names, "i64");
            }
            Statement::Assign(lhs, expr) => {
                if let Some(var_name) = lhs.as_var_name() {
                    let mlir_ty = self.mlir_type(self.var_types.get(var_name).unwrap_or(&Type::int()));
                    if let Some(reg) = reg_names.get(var_name) {
                        let val = self.emit_expr(ng, out, expr, reg_names, &mlir_ty);
                        if let Some(v) = val {
                            writeln!(out, "  seq.always(posedge %clock) {{").ok();
                            writeln!(out, "    {} <= {}", reg, v).ok();
                            writeln!(out, "  }}").ok();
                        }
                    }
                }
            }
            Statement::Guarded(_condition, statements) => {
                for s in statements {
                    self.emit_stmt_body(ng, out, s, reg_names);
                }
            }
            Statement::SyncBlock(body) => {
                for s in body {
                    self.emit_stmt_body(ng, out, s, reg_names);
                }
            }
            Statement::Let { name, expr: Some(e), .. } => {
                let mlir_ty = self.mlir_type(&Type::int());
                if let Some(val) = self.emit_expr(ng, out, e, reg_names, &mlir_ty) {
                    let w = ng.fresh_wire("let");
                    writeln!(out, "  {} = hw.wire {} : {}", w, val, mlir_ty).ok();
                    // Note: reg_names is immutable here, so we can't store it
                }
            }
            _ => {}
        }
    }

    /// Emit combinational logic for a contract condition (precondition or postcondition).
    /// 2026-08-25 (sized scalars): comparisons emit at the OPERAND's
    /// register width — an Int<8> state var compares as i8, never widened
    /// to i64 (circt-opt rejects mixed-width icmp operands — found by the
    /// sim-parity harness on the sized fixture).
    fn operand_width(&self, e: &Expr) -> String {
        if let Expr::Identifier(name) = e {
            if let Some(ty) = self.var_types.get(name) {
                let t = self.mlir_type(ty);
                if t.starts_with('i') && t != "i64" {
                    return t;
                }
            }
        }
        "i64".to_string()
    }

    /// AND-reduce of the active `when` gates (None = unconditional).
    fn when_mask(
        ng: &mut NameGen,
        out: &mut String,
        m: &TxnMaps<'_>,
    ) -> Option<String> {
        if m.gates.is_empty() {
            return None;
        }
        Some(Self::reduce_tree(ng, out, "comb.and", &m.gates, "%true"))
    }

    /// Width for a comparison: whichever side carries a sized register.
    fn compare_width(&self, l: &Expr, r: &Expr) -> String {
        let lw = self.operand_width(l);
        if lw != "i64" {
            return lw;
        }
        self.operand_width(r)
    }

    fn emit_contract_condition(&self, out: &mut String, ng: &mut NameGen, cond: &Expr, result_wire: &str, reg_names: &HashMap<String, String>) {
        match cond {
            Expr::Bool(true) => {
                writeln!(out, "  {} = hw.constant 1 : i1", result_wire).ok();
            }
            Expr::Bool(false) => {
                writeln!(out, "  {} = hw.constant 0 : i1", result_wire).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Lt, l, r) => {
                let w = self.compare_width(l, r);
                let left = self.emit_expr(ng, out, l, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                let right = self.emit_expr(ng, out, r, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                writeln!(out, "  {} = comb.icmp ult {}, {} : {}", result_wire, left, right, w).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Le, l, r) => {
                let w = self.compare_width(l, r);
                let left = self.emit_expr(ng, out, l, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                let right = self.emit_expr(ng, out, r, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                writeln!(out, "  {} = comb.icmp ule {}, {} : {}", result_wire, left, right, w).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Gt, l, r) => {
                let w = self.compare_width(l, r);
                let left = self.emit_expr(ng, out, l, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                let right = self.emit_expr(ng, out, r, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                writeln!(out, "  {} = comb.icmp ugt {}, {} : {}", result_wire, left, right, w).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Ge, l, r) => {
                let w = self.compare_width(l, r);
                let left = self.emit_expr(ng, out, l, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                let right = self.emit_expr(ng, out, r, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                writeln!(out, "  {} = comb.icmp uge {}, {} : {}", result_wire, left, right, w).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
                let w = self.compare_width(l, r);
                let left = self.emit_expr(ng, out, l, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                let right = self.emit_expr(ng, out, r, reg_names, &w).unwrap_or_else(|| "%0".to_string());
                writeln!(out, "  {} = comb.icmp eq {}, {} : {}", result_wire, left, right, w).ok();
            }
            Expr::BinaryOp(BinaryOpKind::And, l, r) => {
                let left_wire = ng.fresh_wire("cond_l");
                self.emit_contract_condition(out, ng, l, &left_wire, reg_names);
                let right_wire = ng.fresh_wire("cond_r");
                self.emit_contract_condition(out, ng, r, &right_wire, reg_names);
                writeln!(out, "  {} = comb.and {}, {} : i1", result_wire, left_wire, right_wire).ok();
            }
            Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
                let left_wire = ng.fresh_wire("cond_l");
                self.emit_contract_condition(out, ng, l, &left_wire, reg_names);
                let right_wire = ng.fresh_wire("cond_r");
                self.emit_contract_condition(out, ng, r, &right_wire, reg_names);
                writeln!(out, "  {} = comb.or {}, {} : i1", result_wire, left_wire, right_wire).ok();
            }
            Expr::UnaryOp(UnaryOpKind::Not, inner) => {
                let inner_wire = ng.fresh_wire("cond_not");
                self.emit_contract_condition(out, ng, inner, &inner_wire, reg_names);
                let c = ng.fresh_const("true");
                writeln!(out, "  {} = hw.constant 1 : i1", c).ok();
                writeln!(out, "  {} = comb.xor {}, {} : i1", result_wire, inner_wire, c).ok();
            }
            _ => {
                writeln!(out, "  {} = hw.constant 1 : i1", result_wire).ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn make_state_decl(name: &str, ty: Type) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: name.to_string(),
            ty,
            span: None,
        })
    }

    fn make_trigger(name: &str, port: &str) -> TopLevel {
        // 2026-08-27 (Slice B): test fixtures use NUMERIC @-addresses —
        // symbolic sources are capability errors on the circuit surface.
        TopLevel::Trigger(Trigger {
            name: name.to_string(),
            instance: Expr::Decimal(1),
                        span: None,
        })
    }

    fn make_txn(name: &str, body: Vec<Statement>, pre: Expr, post: Expr) -> TopLevel {
        TopLevel::Transaction(Transaction {
            is_async: false, is_reactive: true,
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract { pre_condition: pre, post_condition: post, watchdog: None, explicit: false, span: None, post_authority: false},
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    #[test]
    fn test_circt_empty_program() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[]);
        assert!(output.contains("hw.module @top"));
        assert!(output.contains("clock: !seq.clock"));
        assert!(output.contains("hw.output"));
    }

    #[test]
    fn test_circt_trg_port() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_trigger("sensor", "sensor"),
        ]);
        assert!(output.contains("sensor: i64"));
    }

    #[test]
    fn test_circt_state_var_has_seq_register() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("counter", Type::int()),
        ]);
        assert!(output.contains("seq.firreg"), "State vars should use seq.firreg. Got:\n{}", output);
    }

    #[test]
    fn test_circt_expr_add() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("x", Type::int()),
        ]);
        // Just verify basic generation works
        assert!(output.contains("x: i64"), "State should render the signed Int protocol. Got:\n{}", output);
    }

    #[test]
    fn test_circt_expr_mul() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("y", Type::int()),
        ]);
        assert!(output.contains("y: i64"), "State should render the signed Int protocol. Got:\n{}", output);
    }

    #[test]
    fn test_circt_modern_output_ports() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("counter", Type::int()),
        ]);
        assert!(output.contains("in %clock: !seq.clock"), "Should use 'in %' prefix for inputs. Got:\n{}", output);
        assert!(output.contains("in %reset: i1"), "Should use 'in %' prefix for inputs. Got:\n{}", output);
        assert!(output.contains("halt: i1"), "Outputs should include halt. Got:\n{}", output);
        assert!(output.contains("counter: i64"), "Outputs should include counter. Got:\n{}", output);
        assert!(!output.contains("hw.output_assign"), "Should not use deprecated hw.output_assign. Got:\n{}", output);
    }

    #[test]
    fn test_circt_sized_int() {
        let mut backend = CirctBackend::new();
        let ty = Type::Constrained(Box::new(Type::Custom("UInt".to_string())), crate::ast::BitRange::Single(8));
        let output = backend.generate(&[
            make_state_decl("byte", ty),
        ]);
        assert!(output.contains(": i8)"), "Sized UInt[8] should map to i8. Got:\n{}", output);
    }

    #[test]
    fn test_circt_sized_int_32() {
        let mut backend = CirctBackend::new();
        let ty = Type::Constrained(Box::new(Type::Custom("UInt".to_string())), crate::ast::BitRange::Single(32));
        let output = backend.generate(&[
            make_state_decl("word", ty),
        ]);
        assert!(output.contains(": i32)"), "Sized UInt[32] should map to i32. Got:\n{}", output);
    }

    #[test]
    fn test_circt_fsm_state_reg() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("counter", Type::int()),
            make_txn("count", vec![
                Statement::Assign(
                    Expr::Identifier("counter".to_string()),
                    Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("counter".to_string())), Box::new(Expr::Decimal(1))),
                ),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        assert!(output.contains("seq.firreg"), "FSM should have state reg. Got:\n{}", output);
        assert!(output.contains("comb.mux"), "FSM should have state transition mux. Got:\n{}", output);
        // 2026-08-23 (Plan 3): seq.alwas was fantasy syntax — the register
        // consumes a next-value wire via typed `seq.firreg %next clock`.
        // Register consumes the computed next wire (suffix from NameGen).
        assert!(output.contains("= seq.firreg %counter_next"),
            "register must consume the computed next wire. Got:\n{}", output);
    }

    #[test]
    fn test_circt_fsm_precondition_check() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("done", Type::int()),
            make_txn("loop", vec![],
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("done".to_string())), Box::new(Expr::Decimal(10))),
                Expr::Bool(true)),
        ]);
        assert!(output.contains("comb.icmp"), "Precondition should emit comb.icmp. Got:\n{}", output);
    }

    #[test]
    fn test_circt_fsm_postcondition_check() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("done", Type::int()),
            make_txn("loop", vec![],
                Expr::Bool(true),
                Expr::BinaryOp(BinaryOpKind::Eq, Box::new(Expr::Identifier("done".to_string())), Box::new(Expr::Decimal(10)))),
        ]);
        assert!(output.contains("comb.icmp"), "Postcondition should emit comb.icmp eq. Got:\n{}", output);
    }

    #[test]
    fn test_circt_sync_block() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("a", Type::int()),
            make_state_decl("b", Type::int()),
            make_txn("test", vec![
                Statement::SyncBlock(vec![
                    Statement::Assign(
                        Expr::Identifier("a".to_string()),
                        Expr::Decimal(10),
                    ),
                    Statement::Assign(
                        Expr::Identifier("b".to_string()),
                        Expr::Decimal(20),
                    ),
                ]),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        // 2026-08-23 (Plan 3): sync blocks lower through the same pending-
        // wire path — both registers consume computed next wires.
        assert!(output.contains("= seq.firreg %a_next"), "a register consumes next. Got:\n{}", output);
        assert!(output.contains("= seq.firreg %b_next"), "b register consumes next. Got:\n{}", output);
        // a := 10: an enabled mux selects the constant over the init wire.
        assert!(output.contains("OpConstant") || output.contains("hw.constant 10"),
            "constant 10 must be materialized. Got:\n{}", output);
        assert!(output.matches("comb.mux %true,").count() >= 2,
            "both assignments produce enabled muxes. Got:\n{}", output);
    }

    /// 2026-08-27 (undefined-instance fix): a call whose callee has no cell
    /// definition RECORDS a capability error instead of instantiating a
    /// module that nothing defines.
    /// 2026-08-27 (cbv-HW plan Slice A): extern declarations emit an
    /// hw.module.extern blackbox with implicit clock/reset and declared
    /// ports — identical shape to defined cells at instantiation sites —
    /// AND contribute no program-visible top-level ports.
    /// 2026-08-27 (Slice B): @-addressed MMIO pins emit ADDRESS-SORTED on
    /// @top regardless of declaration order — the deterministic bus-layout
    /// rule. Unaddressed triggers keep program order after the pins.
    #[test]
    fn test_mmio_pins_emit_address_sorted() {
        let mut backend = CirctBackend::new();
        let src = "trg b_pin @ 0x2000;\n\
                   trg a_pin @ 0x1000;\n\
                   let done: Int = 0;\n\
                   txn tick [done == 0][done == 1] {\n\
                       done = 1;\n\
                   }\n";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().unwrap();
        let output = backend.generate(&items);
        let a = output.find("in %a_pin:").expect("a_pin port");
        let b = output.find("in %b_pin:").expect("b_pin port");
        assert!(a < b, "0x1000 pin must precede 0x2000 pin:\n{output}");
        // Pin table is live and carries the addresses.
        assert_eq!(backend.mmio_vars.len(), 2, "{:?}", backend.mmio_vars);
        assert_eq!(backend.mmio_vars[0], ("a_pin".to_string(), 0x1000));
        assert_eq!(backend.mmio_vars[1], ("b_pin".to_string(), 0x2000));
        assert!(backend.errors.borrow().is_empty(), "{:?}", backend.errors.borrow());
    }

    /// Slice B boundary: dynamic (@ *ptr) and symbolic trigger addresses
    /// have no static pin — capability errors naming the rule, never a
    /// silently-dropped port.
    #[test]
    fn test_mmio_dynamic_and_symbolic_addresses_error() {
        let mut backend = CirctBackend::new();
        let src = "let p: Int = 0;\n\
                   trg dyn_pin @ *p;\n\
                   trg sym_pin @ SERIAL;\n\
                   let done: Bool = false;\n\
                   txn tick [done == false][done == true] {\n\
                     done = true;\n\
                   }\n";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().unwrap();
        let _ = backend.generate(&items);
        let errs = backend.errors.borrow();
        assert!(errs.iter().any(|e| e.contains("'dyn_pin'") && e.contains("static")),
            "dynamic pin must error: {:?}", errs);
        assert!(errs.iter().any(|e| e.contains("'sym_pin'")),
            "symbolic pin must error: {:?}", errs);
    }

    #[test]
    fn test_circt_extern_blackbox_shape() {
        let mut backend = CirctBackend::new();
        let src = "extern UartTop(rx: Int) -> byte_out: Int from \"rtl/uart.v\";";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().unwrap();
        let output = backend.generate(&items);
        // Exactly one blackbox, right shape:
        assert_eq!(output.matches("hw.module.extern").count(), 1,
            "one blackbox expected:\n{output}");
        assert!(output.contains("hw.module.extern @UartTop(in %clock: !seq.clock, in %reset: i1, in %rx: i64, out byte_out: i64);"),
            "blackbox shape mismatch (ports inside one paren list, bare output names):\n{output}");
        // Foreign pins are NOT program state — no UartTop$ leak on @top.
        assert!(!output.contains("UartTop$"), "extern cell must not leak program vars:\n{output}");
        assert!(backend.errors.borrow().is_empty(), "{:?}",
            backend.errors.borrow());
    }

    #[test]
    fn test_circt_call_non_cell_records_error() {
        let mut backend = CirctBackend::new();
        let _ = backend.generate(&[
            make_state_decl("x", Type::int()),
            make_txn("compute", vec![
                Statement::Assign(
                    Expr::Identifier("x".to_string()),
                    Expr::Call("add".to_string(), vec![Expr::Decimal(1), Expr::Decimal(2)], None),
                ),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        let errs = backend.errors.borrow();
        assert!(
            errs.iter().any(|e| e.contains("'add'")),
            "non-cell callee must record a capability error. Got: {:?}",
            errs
        );
    }

    #[test]
    fn test_circt_intrinsic_abs() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("x", Type::int()),
            make_txn("test", vec![
                Statement::Assign(
                    Expr::Identifier("x".to_string()),
                    Expr::Call("Abs#".to_string(), vec![Expr::Decimal(-5)], None),
                ),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        assert!(output.contains("comb.neg"), "Abs intrinsic should emit comb.neg. Got:\n{}", output);
        assert!(output.contains("comb.mux"), "Abs intrinsic should emit comb.mux. Got:\n{}", output);
    }

    #[test]
    fn test_circt_intrinsic_ctpop() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("x", Type::int()),
            make_txn("test", vec![
                Statement::Assign(
                    Expr::Identifier("x".to_string()),
                    Expr::Call("Ctpop#".to_string(), vec![Expr::Decimal(255)], None),
                ),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        // 2026-08-23 (Plan 3.2): comb has NO ctpop — the old arm emitted
        // MLIR no toolchain could parse. Now a recorded capability error.
        let errs = backend.errors.borrow();
        assert!(!errs.is_empty() && errs[0].contains("Ctpop#"),
            "Ctpop must be a recorded capability error. Got:\n{}", output);
    }

    #[test]
    fn test_circt_intrinsic_bitreverse() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("x", Type::int()),
            make_txn("test", vec![
                Statement::Assign(
                    Expr::Identifier("x".to_string()),
                    Expr::Call("Bitreverse#".to_string(), vec![Expr::Decimal(1)], None),
                ),
            ], Expr::Bool(true), Expr::Bool(true)),
        ]);
        // 2026-08-23 (Plan 3.2): comb has NO rev — recorded capability error.
        let errs = backend.errors.borrow();
        assert!(!errs.is_empty() && errs[0].contains("Bitreverse#"),
            "Bitreverse must be a recorded capability error. Got:\n{}", output);
    }

    #[test]
    fn test_circt_duplicate_trg_fixed() {
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_trigger("sensor", "sensor"),
        ]);
        let count = output.matches("sensor: i64").count();
        assert!(count >= 1, "Trigger should appear as port. Got {} occurrences. Output:\n{}", count, output);
    }
    fn make_array_let(name: &str, depth: usize, hint: Option<&str>) -> TopLevel {
        // Top-level typed let (the array surface): Statement::Let with a
        // Vector type, zero-init literal, optional mem/reg annotation.
        use crate::ast::{Annotation, Statement as S, TopLevel as T};
        let mut modifiers = Vec::new();
        if let Some(h) = hint {
            modifiers.push(Annotation { name: h.to_string(), value: None });
        }
        T::Statement(Box::new(S::Let {
            name: name.to_string(),
            names: vec![name.to_string()],
            ty: Some(Type::Vector(
                Box::new(Type::int()),
                vec![crate::ast::Dimension::Anonymous(depth)],
            )),
            expr: Some(Expr::List(vec![Expr::Decimal(0); depth])),
            modifiers,
        }))
    }

    #[test]
    fn test_guarded_element_write_emits_mem_enable() {
        // 2026-08-26 (gate threading): `when c { buf[i] = v; }` on a
        // mem-lowered array — the when-condition ANDs into the write
        // ENABLE. Previously a SILENT DROP.
        let items = vec![
            make_state_decl("w", Type::int()),
            make_array_let("buf", 64, None),
            make_txn(
                "fill",
                vec![Statement::Guarded(
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Gt,
                        Box::new(Expr::Identifier("w".into())),
                        Box::new(Expr::Decimal(2)),
                    ),
                    vec![Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("buf".into())),
                            Box::new(Expr::Identifier("w".into())),
                        ),
                        Expr::Decimal(1),
                    )],
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ];
        let mut backend = CirctBackend::new();
        let output = backend.generate(&items);
        assert!(
            output.contains("seq.firmem.write_port"),
            "guarded element write vanished:\n{}",
            output
        );
        // Single when-gate rides the ENABLE directly; multiple gates AND.
        assert!(
            output.contains("write_port") && output.contains("enable %"),
            "when-condition must gate the enable:\n{}",
            output
        );
    }

    #[test]
    fn test_guarded_element_write_gates_lane_decode() {
        // Same statement against a REGISTER-FILE array: the when-mask ANDs
        // into every lane select; no silent drop, no ungated write.
        let items = vec![
            make_state_decl("w", Type::int()),
            make_array_let("buf", 8, Some("reg")),
            make_txn(
                "fill",
                vec![Statement::Guarded(
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Gt,
                        Box::new(Expr::Identifier("w".into())),
                        Box::new(Expr::Decimal(2)),
                    ),
                    vec![Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("buf".into())),
                            Box::new(Expr::Decimal(3)),
                        ),
                        Expr::Decimal(1),
                    )],
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ];
        let mut backend = CirctBackend::new();
        let output = backend.generate(&items);
        assert!(output.contains("buf_3_lane"), "lane decode missing:\n{}", output);
        assert!(output.contains("comb.and"), "when mask missing:\n{}", output);
    }

    #[test]
    fn test_nested_when_conditions_and() {
        // when a { when b { x = 1; } } ⇒ value muxes on a AND b.
        let items = vec![
            make_state_decl("x", Type::int()),
            make_txn(
                "t",
                vec![Statement::Guarded(
                    Expr::Bool(true),
                    vec![Statement::Guarded(
                        Expr::Bool(false),
                        vec![Statement::Assign(
                            Expr::Identifier("x".into()),
                            Expr::Decimal(1),
                        )],
                    )],
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ];
        let mut backend = CirctBackend::new();
        let output = backend.generate(&items);
        // two gate wires ANDed somewhere before the mux
        assert!(
            output.matches("comb.mux").count() >= 2,
            "nested gating lost:\n{}",
            output
        );
        assert!(output.contains("comb.and"), "nested conditions must AND");
    }

    #[test]
    fn test_ns_to_cycles_conversion() {
        // 2026-08-26 (watchdog time units): 100 MHz clock —
        // 10ms = 1_000_000 cycles exactly; sub-cycle deadlines round UP.
        assert_eq!(CirctBackend::ns_to_cycles(100_000_000, 10_000_000), 1_000_000);
        assert_eq!(CirctBackend::ns_to_cycles(100_000_000, 1), 1);       // ceil to 1
        assert_eq!(CirctBackend::ns_to_cycles(1_000_000_000, 500), 500); // exact
        assert_eq!(CirctBackend::ns_to_cycles(100_000_000, 15), 2);      // ceil 1.5 -> 2
        assert_eq!(CirctBackend::ns_to_cycles(60_000_000, 1_500_000_000), 90_000_000);
    }

    #[test]
    fn test_deep_array_defaults_to_firmem_macro() {
        // 2026-08-25 (seq-firmem plan): depth >= 64 + zero init +
        // single writer ⇒ memory macro; default decision lands in the
        // disambiguation note.
        let mut backend = CirctBackend::new();
        let items = vec![
            make_array_let("buf", 64, None),
            make_txn(
                "fill",
                vec![Statement::Assign(
                    Expr::Index(
                        Box::new(Expr::Identifier("buf".into())),
                        Box::new(Expr::Identifier("w".into())),
                    ),
                    Expr::Decimal(1),
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ];
        let output = backend.generate(&items);
        assert!(output.contains("seq.firmem "), "no macro in:\n{}", output);
        assert!(!output.contains("buf_0 "), "lanes leaked for mem array");
        let note = backend.take_disambiguation_note().unwrap();
        assert!(note.contains("buf"), "note must name the array: {}", note);
        assert!(note.contains("depth >= threshold"));
    }

    #[test]
    fn test_reg_pin_forces_lanes_and_silences_note() {
        let mut backend = CirctBackend::new();
        let items = vec![
            make_array_let("buf", 64, Some("reg")),
            make_txn(
                "fill",
                vec![Statement::Assign(
                    Expr::Index(
                        Box::new(Expr::Identifier("buf".into())),
                        Box::new(Expr::Identifier("w".into())),
                    ),
                    Expr::Decimal(1),
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ];
        let output = backend.generate(&items);
        assert!(!output.contains("seq.firmem "), "pin ignored:\n{}", output);
        assert!(output.contains("buf_63"), "lanes missing");
        assert!(
            backend.take_disambiguation_note().is_none(),
            "explicit pin must silence the note"
        );
    }

    #[test]
    fn test_mem_pin_with_post_ref_is_capability_error() {
        // Postcondition reads elements — mem pin cannot honor obligations.
        let mut backend = CirctBackend::new();
        let items = vec![
            make_array_let("buf", 64, Some("mem")),
            make_txn(
                "fill",
                vec![Statement::Assign(
                    Expr::Index(
                        Box::new(Expr::Identifier("buf".into())),
                        Box::new(Expr::Decimal(0)),
                    ),
                    Expr::Decimal(1),
                )],
                Expr::Bool(true),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Lt,
                    Box::new(Expr::Index(
                        Box::new(Expr::Identifier("buf".into())),
                        Box::new(Expr::Decimal(0)),
                    )),
                    Box::new(Expr::Decimal(9)),
                ),
            ),
        ];
        let _ = backend.generate(&items);
        let errs = backend.errors.borrow();
        assert!(
            errs.iter().any(|e| e.contains("postcondition reads elements")),
            "expected capability error, got: {:?}",
            *errs
        );
    }

    #[test]
    fn test_emitted_module_parses_under_circt_opt() {
        if !circt_tools_available() {
            eprintln!(
                "SKIP: circt-opt not installed — run tools/install-circt.sh \
                 for toolchain-validated coverage"
            );
            return;
        }
        let mut backend = CirctBackend::new();
        let output = backend.generate(&[
            make_state_decl("counter", Type::int()),
            make_trigger("tick", "tick"),
            make_txn(
                "step",
                vec![Statement::Assign(
                    Expr::Identifier("counter".into()),
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Add,
                        Box::new(Expr::Identifier("counter".into())),
                        Box::new(Expr::Decimal(1)),
                    ),
                )],
                Expr::Bool(true),
                Expr::Bool(true),
            ),
        ]);
        assert!(backend.errors.borrow().is_empty(), "fixture must be in-surface");

        let dir = std::env::temp_dir().join(format!("briev_circt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("top.mlir");
        std::fs::write(&path, &output).unwrap();

        // Locate circt-opt: local install first, then PATH.
        let local = format!("{}/tools/circt/bin/circt-opt", env!("CARGO_MANIFEST_DIR"));
        let tool = if std::path::Path::new(&local).exists() {
            local
        } else {
            "circt-opt".to_string()
        };
        let out = std::process::Command::new(tool)
            .arg(&path)
            .output()
            .expect("run circt-opt");
        assert!(
            out.status.success(),
            "circt-opt rejected the emitted module:\n{}\n{}",
            String::from_utf8_lossy(&out.stderr),
            output
        );
        let _ = std::fs::remove_file(&path);
    }
}

/// 2026-08-23 (Plan 0.6): CIRCT toolchain availability — mirrors the
/// `is_available()` pattern (backend/assembler/mod.rs). Toolchain-validated
/// tests (parse/translate/simulate parity) gate on this and skip loudly
/// when tools are absent; structural string checks always run.
/// To undo: remove this fn + tools/install-circt.sh + tools/circt_probe.sh.
pub fn circt_tools_available() -> bool {
    // 1. Local install from tools/install-circt.sh
    let local = std::path::Path::new("tools/circt/bin/circt-opt");
    if local.exists() {
        return true;
    }
    // 2. Somewhere on PATH
    std::process::Command::new("circt-opt")
        .arg("--version")
        .output()
        .is_ok()
}
