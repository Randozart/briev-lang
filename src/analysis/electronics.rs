//! 2026-09-11 (Part C, Electronics Briev): contract-inferred netlist.
//!
//! Electronics Briev has NO connection operator. A netlist is the transitive
//! closure of pin-equality obligations stated in transaction PREconditions:
//! `[j1.vcc.voltage == r1.a.voltage]` states that both pins sit on the same
//! electrical node. This pass derives the netlist the compiler ships to the
//! KiCad backend — the backend consumes, it never re-derives (frontend-driven
//! dispatch pillar).
//!
//! Model:
//! - Component instances are top-level `let name: Type = Type { ... };` where
//!   `Type` declares `pin` clauses. Literal fields (e.g. `value: "330"`) are
//!   carried as component properties for the schematic.
//! - Edges come from `BinaryOpKind::Eq` nodes in PREconditions only. A
//!   postcondition states physics (the consequence), never wiring — it does
//!   not create nets.
//! - Nets are the union-find groups of pins co-mentioned through equality
//!   chains, named N1..Nn deterministically (sorted members).
//! - A declared pin on no net is a dangling pin — the #1 PCB error — and is a
//!   hard diagnostic, never silent.
//!
//! Pin access shape accepted in obligations: `inst.pin` or `inst.pin.prop`
//! (e.g. `r1.a` / `r1.a.voltage`). Both operands of an Eq must resolve to
//! declared pins on declared instances.

use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel};
use std::collections::BTreeMap;

/// One component instance in the schematic.
#[derive(Debug, Clone)]
pub struct ComponentInstance {
    pub name: String,
    pub type_name: String,
    /// Struct-literal fields carried as schematic properties (`value` → Value,
    /// `package` → Footprint). Only literal-typed values are carried.
    pub properties: Vec<(String, String)>,
}

/// A pin end of a net, resolved against its type's pin declarations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PinRef {
    pub component: String,
    pub pin: String,
    pub number: u64,
}

/// Per-component-type schematic facts extracted from the type declaration.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// KiCad reference-designator prefix (`R`, `D`, `J`) — `!> Reference` on
    /// the type, defaulting to the type's first letter uppercased.
    pub reference_prefix: String,
    /// Declared pins (name, KiCad number), sorted by number.
    pub pins: Vec<(String, u64)>,
    /// Max voltage any pin of this type tolerates — `!> Tolerance: 3.3`.
    /// None = unrated: the pin places no constraint (skeleton semantics;
    /// per-pin ratings are a follow-on).
    pub tolerance: Option<f64>,
}

/// One derived electrical node.
#[derive(Debug, Clone)]
pub struct Net {
    pub name: String,
    pub pins: Vec<PinRef>,
}

/// The derived netlist plus hard diagnostics.
#[derive(Debug, Default)]
pub struct ElectronicsNetlist {
    pub components: Vec<ComponentInstance>,
    pub nets: Vec<Net>,
    pub dangling: Vec<String>,
    /// True when the program contains any pin-carrying component type.
    pub is_electronics: bool,
    /// Schematic facts per component type name.
    pub type_info: BTreeMap<String, TypeInfo>,
    /// 2026-09-11 (electrical proving): net voltage classes + violations.
    pub voltage: VoltageCheck,
}

/// 2026-09-11 (electrical proving): voltage classes over the derived netlist.
///
/// A transaction that states `[j1.p1.voltage == 5.0]` (pre OR post — the
/// supply is at 5 V before and after firing) DRIVES its net at 5 V. Two
/// different drive values on one net is a shorted supply. Every pin on a
/// driven net whose type declares `!> Tolerance: <max>` must tolerate the
/// class; a 5 V net into a 3.3 V-only pin is a compile error, never a fried
/// board. Unrated pins place no constraint.
#[derive(Debug, Default)]
pub struct VoltageCheck {
    /// Net name → driven voltage class (max of agreeing drives).
    pub net_voltage: BTreeMap<String, f64>,
    /// Net name → derived current class (B4: V/R through series parts).
    pub net_current: BTreeMap<String, f64>,
    /// Physics facts the compiler PROVED (V=IR derivations) — reported in
    /// verification output so a proven bound is distinguishable from an
    /// unchecked one.
    pub proved: Vec<String>,
    /// Hard diagnostics (what/why/fix style).
    pub violations: Vec<String>,
}

/// Union-find over pin keys (`component\x1Fpin`).
struct DisjointSet {
    parent: BTreeMap<String, String>,
}

impl DisjointSet {
    fn new() -> Self {
        Self { parent: BTreeMap::new() }
    }
    fn make(&mut self, k: String) {
        let missing = !self.parent.contains_key(&k);
        if missing {
            self.parent.insert(k.clone(), k);
        }
    }
    fn find(&mut self, k: &str) -> String {
        let mut root = k.to_string();
        while let Some(p) = self.parent.get(&root).cloned() {
            if p == root {
                break;
            }
            root = p;
        }
        // Path compression.
        let mut cur = k.to_string();
        while cur != root {
            let next = match self.parent.get(&cur).cloned() {
                Some(n) if n != cur => n,
                _ => break,
            };
            self.parent.insert(cur.clone(), root.clone());
            cur = next;
        }
        root
    }
    fn union(&mut self, a: &str, b: &str) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }
}

/// Pin key for a map: component + pin name (numbers live in PinRef).
fn pin_key(component: &str, pin: &str) -> String {
    format!("{}\u{1F}{}", component, pin)
}

/// Extract declared pins per component type from TypeDef bodies, plus the
/// schematic facts (`!> Reference` prefix) each type carries.
fn collect_type_pins(items: &[TopLevel]) -> (BTreeMap<String, Vec<(String, u64)>>, BTreeMap<String, TypeInfo>) {
    let mut out = BTreeMap::new();
    let mut info = BTreeMap::new();
    for item in items {
        if let TopLevel::TypeDef(td) = item {
            if !td.body.pins.is_empty() {
                out.insert(
                    td.name.clone(),
                    td.body.pins.iter().map(|p| (p.name.clone(), p.number)).collect(),
                );
                // 2026-09-11 (B3): reference/tolerance are STRUCTURAL clause
                // fields — the !> metadata path is deleted. reference is
                // parse-mandatory when pins exist, so it is always present.
                let prefix = td
                    .body
                    .reference
                    .clone()
                    .unwrap_or_else(|| {
                        td.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "U".to_string())
                    });
                // `tolerance 3.3;` rates the pins; `tolerance any;` declares
                // unrated (a decision); no clause = unrated (B4 wires the
                // driven-net violation for the no-clause case).
                let tolerance = td.body.tolerance.as_ref().map(|t| match t {
                    crate::ast::top::Tolerance::Volts(v) => *v,
                    crate::ast::top::Tolerance::Any => f64::INFINITY,
                });
                let mut pins: Vec<(String, u64)> =
                    td.body.pins.iter().map(|p| (p.name.clone(), p.number)).collect();
                pins.sort_by_key(|&(_, n)| n);
                info.insert(td.name.clone(), TypeInfo { reference_prefix: prefix, pins, tolerance });
            }
        }
    }
    (out, info)
}

/// Collect component instances: top-level `let name: T = T { fields };` where
/// T declares pins. Literal fields become schematic properties.
fn collect_instances(
    items: &[TopLevel],
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<ComponentInstance> {
    let mut out = Vec::new();
    for item in items {
        let TopLevel::Statement(stmt) = item else { continue };
        let Statement::Let { name, ty, expr, .. } = stmt.as_ref() else { continue };
        let Some(Expr::StructLiteral { type_name: lit_ty, fields }) = expr else { continue };
        let declared = ty.as_ref().map_or("", |t| match t {
            crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => n.as_str(),
            _ => "",
        });
        // Accept when the annotated type (or the literal's type) declares pins.
        let type_name = if type_pins.contains_key(declared) {
            declared.to_string()
        } else if type_pins.contains_key(lit_ty) {
            lit_ty.clone()
        } else {
            continue;
        };
        let properties = fields
            .iter()
            .filter_map(|(fname, fexpr)| match fexpr {
                Expr::Quoted(bytes) => Some((fname.clone(), String::from_utf8_lossy(bytes).into_owned())),
                Expr::Decimal(d) => Some((fname.clone(), d.to_string())),
                Expr::Float(f) => Some((fname.clone(), f.to_string())),
                _ => None,
            })
            .collect();
        out.push(ComponentInstance { name: name.clone(), type_name, properties });
    }
    out
}

/// If `expr` is a pin access on a declared instance (`inst.pin` or
/// `inst.pin.prop`), return its PinRef; else None.
fn resolve_pin(expr: &Expr, instances: &BTreeMap<String, &ComponentInstance>, type_pins: &BTreeMap<String, Vec<(String, u64)>>) -> Option<PinRef> {
    // Strip property accesses (`r1.a.voltage` → `r1.a`): walk down while the
    // base is itself a Field — the pin access is the Field whose base is the
    // instance identifier.
    let mut cur = expr;
    while let Expr::Field(base, _) = cur {
        if matches!(base.as_ref(), Expr::Field(_, _)) {
            cur = base.as_ref();
        } else {
            break;
        }
    }
    let Expr::Field(base, pin_name) = cur else { return None };
    let Expr::Identifier(inst_name) = base.as_ref() else { return None };
    let inst = instances.get(inst_name)?;
    let pins = type_pins.get(&inst.type_name)?;
    let (_, number) = pins.iter().find(|(n, _)| n == pin_name)?;
    Some(PinRef {
        component: inst.name.clone(),
        pin: pin_name.clone(),
        number: *number,
    })
}

/// Collect Eq operand pairs from an expression tree (walks And/Or chains and
/// parenthesized/grouped nodes; everything else is a leaf for this purpose).
/// Third element is an optional net name from `net <name>:` annotation.
fn collect_eq_triples(expr: &Expr, out: &mut Vec<(Expr, Expr, Option<String>)>) {
    match expr {
        Expr::Named { name, inner } => {
            // Attach the name to the next Eq we find inside.
            collect_eq_triples_with_name(name, inner, out);
        }
        Expr::BinaryOp(op @ (BinaryOpKind::Eq | BinaryOpKind::And | BinaryOpKind::Or), l, r) => {
            if *op == BinaryOpKind::Eq {
                out.push(((**l).clone(), (**r).clone(), None));
            } else {
                collect_eq_triples(l, out);
                collect_eq_triples(r, out);
            }
        }
        _ => {}
    }
}

/// Like `collect_eq_triples` but propagates a net name downward into the
/// first Eq encountered.
fn collect_eq_triples_with_name(
    name: &str,
    expr: &Expr,
    out: &mut Vec<(Expr, Expr, Option<String>)>,
) {
    match expr {
        Expr::BinaryOp(op @ (BinaryOpKind::Eq | BinaryOpKind::And | BinaryOpKind::Or), l, r) => {
            if *op == BinaryOpKind::Eq {
                out.push(((**l).clone(), (**r).clone(), Some(name.to_string())));
            } else {
                // Name applies to the first Eq in left-to-right order.
                let before = out.len();
                collect_eq_triples_with_name(name, l, out);
                if out.len() == before {
                    collect_eq_triples_with_name(name, r, out);
                }
            }
        }
        Expr::Named { name: inner_name, inner } => {
            // Nested net name — inner wins (closer to the Eq).
            collect_eq_triples_with_name(inner_name, inner, out);
        }
        _ => {}
    }
}

/// If `expr` is `[pin.voltage]` on a declared instance, resolve the pin.
/// (`j1.p1.voltage` → the `j1.p1` PinRef; any other shape → None.)
fn resolve_voltage_pin(expr: &Expr, instances: &BTreeMap<String, &ComponentInstance>, type_pins: &BTreeMap<String, Vec<(String, u64)>>) -> Option<PinRef> {
    let Expr::Field(base, prop) = expr else { return None };
    if prop != "voltage" {
        return None;
    }
    resolve_pin(base, instances, type_pins)
}

/// A voltage DRIVE is `[x.voltage == <literal>]` — pin access on one side,
/// float/int literal on the other, either order. Returns (pin, volts).
fn voltage_drive(
    l: &Expr,
    r: &Expr,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<(PinRef, f64)> {
    if let Some(pin) = resolve_voltage_pin(l, instances, type_pins) {
        let v = match r {
            Expr::Float(f) => Some(*f),
            Expr::Decimal(d) => Some(*d as f64),
            _ => None,
        };
        if let Some(v) = v {
            return Some((pin, v));
        }
    }
    if let Some(pin) = resolve_voltage_pin(r, instances, type_pins) {
        let v = match l {
            Expr::Float(f) => Some(*f),
            Expr::Decimal(d) => Some(*d as f64),
            _ => None,
        };
        if let Some(v) = v {
            return Some((pin, v));
        }
    }
    None
}

/// Derive net voltage classes and prove tolerance compatibility.
fn derive_voltage(
    items: &[TopLevel],
    nets: &[Net],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
) -> VoltageCheck {
    let pin_to_net = pin_net_index(nets);
    let drives = collect_drives(items, &pin_to_net, instances, type_pins);
    let mut check = classify_drives(drives);
    check_tolerance(nets, instances, type_info, &check.net_voltage, &mut check.violations);
    // B4 flagship: I = V / R through series parts, dividers, and KCL sums;
    // then the postcondition current bounds are proven against the result.
    derive_current(&pin_to_net, instances, type_info, check.net_voltage.clone(), &mut check);
    check_current_bounds(items, instances, type_pins, &pin_to_net, &mut check);
    check
}

/// Pin → net name index.
fn pin_net_index(nets: &[Net]) -> BTreeMap<(String, String), String> {
    let mut pin_to_net = BTreeMap::new();
    for net in nets {
        for p in &net.pins {
            pin_to_net.insert((p.component.clone(), p.pin.clone()), net.name.clone());
        }
    }
    pin_to_net
}

/// Collect voltage drives (net name, volts, obligation text) across all
/// transaction contracts — `[x.voltage == <literal>]` in pre OR post.
fn collect_drives(
    items: &[TopLevel],
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<(String, f64, String)> {
    let mut drives = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        for cond in [&t.contract.pre_condition, &t.contract.post_condition] {
            let mut triples = Vec::new();
            collect_eq_triples(cond, &mut triples);
            for (l, r, _name) in triples {
                let Some((pin, v)) = voltage_drive(&l, &r, instances, type_pins) else {
                    continue;
                };
                let Some(net_name) = pin_to_net.get(&(pin.component.clone(), pin.pin.clone())) else {
                    continue;
                };
                drives.push((
                    net_name.clone(),
                    v,
                    format!("'{}.{}.voltage == {}' in txn '{}'", pin.component, pin.pin, format_volts(v), t.name),
                ));
            }
        }
    }
    drives
}

/// Group drives by net: the class is the max — disagreeing drives are a
/// shorted supply, hard error.
fn classify_drives(drives: Vec<(String, f64, String)>) -> VoltageCheck {
    let mut check = VoltageCheck::default();
    let mut by_net: BTreeMap<String, Vec<(f64, String)>> = BTreeMap::new();
    for (net, v, src) in drives {
        by_net.entry(net).or_default().push((v, src));
    }
    for (net_name, ds) in by_net {
        let mut vals = ds.iter().map(|(v, _)| *v).collect::<Vec<_>>();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = *vals.first().unwrap();
        let max = *vals.last().unwrap();
        if (min - max).abs() > f64::EPSILON {
            let sources = ds.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" and ");
            check.violations.push(format!(
                "net '{}' is driven at two different voltages ({} and {}) — that is a shorted supply. \
                 why: {} both drive it. fix: drive the net at one voltage, or separate the levels \
                 with a regulator or switch component.",
                net_name, format_volts(min), format_volts(max), sources
            ));
        }
        check.net_voltage.insert(net_name.clone(), max);
    }
    check
}

/// Every pin on a driven net must tolerate its class. `tolerance any` is a
/// declared decision (never violates); NO tolerance clause on a driven net
/// is an undeclared decision — a violation naming the fix.
fn check_tolerance(
    nets: &[Net],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    net_voltage: &BTreeMap<String, f64>,
    violations: &mut Vec<String>,
) {
    for net in nets {
        let Some(class) = net_voltage.get(&net.name).copied() else { continue };
        for p in &net.pins {
            let Some(inst) = instances.get(&p.component) else { continue };
            let Some(info) = type_info.get(&inst.type_name) else { continue };
            let Some(tol) = info.tolerance else {
                violations.push(format!(
                    "net '{}' is driven at {} but pin '{}.{}' (of {}) has no tolerance clause — \
                     an unrated pin on a driven net is an undeclared decision. \
                     fix: add `tolerance <max>;` rated for {}, or `tolerance any;` to declare \
                     the pin unrated on purpose.",
                    net.name, format_volts(class), p.component, p.pin, inst.type_name, format_volts(class)
                ));
                continue;
            };
            if class > tol + f64::EPSILON {
                violations.push(format!(
                    "net '{}' is driven at {} but pin '{}.{}' (of {}) tolerates only {}. \
                     why: the drive comes from a contract obligation on that net. \
                     fix: lower the drive voltage, or use a component rated for {} on that net.",
                    net.name, format_volts(class), p.component, p.pin, inst.type_name, format_volts(tol), format_volts(class)
                ));
            }
        }
    }
}

/// Parse an ohmic value from a component property: plain (`330`), k/K
/// (kilohm), M (megohm), R (unit marker, `330R`). Returns None for
/// non-numeric values ("red") — no derivation, no error.
fn parse_ohms(raw: &str) -> Option<f64> {
    let s = raw.trim();
    // Suffixes: k/K (kilohm), M (megohm), R/r (unit marker). Plain parse
    // otherwise. Non-numeric values ("red") → None: no derivation, no error.
    let (num, mult) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1e3),
        'M' => (&s[..s.len() - 1], 1e6),
        'R' | 'r' => (&s[..s.len() - 1], 1.0),
        _ => (s, 1.0),
    };
    let v = num.trim().parse::<f64>().ok()?;
    let out = v * mult;
    if out > 0.0 { Some(out) } else { None }
}

/// Collect current-bound obligations from POSTconditions:
/// `[x.pin.current <= B]` (upper) / `[x.pin.current >= B]` (lower).
fn collect_current_bounds(items: &[TopLevel], instances: &BTreeMap<String, &ComponentInstance>, type_pins: &BTreeMap<String, Vec<(String, u64)>>) -> Vec<(PinRef, f64, bool)> {
    let mut out = Vec::new();
    let resolve_current = |expr: &Expr| -> Option<PinRef> {
        let Expr::Field(base, prop) = expr else { return None };
        if prop != "current" {
            return None;
        }
        // Strip to the pin root (`d1.a.current` → `d1.a`).
        resolve_pin(base, instances, type_pins)
    };
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        let mut pairs = Vec::new();
        collect_compare_pairs(&t.contract.post_condition, &mut pairs);
        for (l, op, r) in pairs {
            // Literal on either side: `[x <= 0.02]` or `[0.02 >= x]`.
            let (pin_expr, lit) = match (&l, &r) {
                (p, Expr::Float(f)) => (p, *f),
                (p, Expr::Decimal(d)) => (p, *d as f64),
                (Expr::Float(f), p) => (p, *f),
                (Expr::Decimal(d), p) => (p, *d as f64),
                _ => continue,
            };
            let is_upper = matches!(op, BinaryOpKind::Le | BinaryOpKind::Lt);
            if let Some(pin) = resolve_current(pin_expr) {
                out.push((pin, lit, is_upper));
            }
        }
    }
    out
}

/// Collect comparison operand triples (`op` ∈ Le/Lt/Ge/Gt) from an expression
/// tree — the postcondition physics bounds.
fn collect_compare_pairs(expr: &Expr, out: &mut Vec<(Expr, BinaryOpKind, Expr)>) {
    match expr {
        Expr::BinaryOp(op @ (BinaryOpKind::Le | BinaryOpKind::Lt | BinaryOpKind::Ge | BinaryOpKind::Gt), l, r) => {
            out.push(((**l).clone(), *op, (**r).clone()));
        }
        Expr::BinaryOp(BinaryOpKind::And | BinaryOpKind::Or, l, r) => {
            collect_compare_pairs(l, out);
            collect_compare_pairs(r, out);
        }
        _ => {}
    }
}

/// B4 flagship — Ohm's-law current derivation: a two-pin part with a numeric
/// value strung between a driven net and an undriven net carries
/// I = V / R. The undriven net inherits the current class; postcondition
/// current bounds on that net are PROVEN (or violated) against it.
#[allow(clippy::too_many_arguments)]
/// One two-pin valued part in the derivation graph.
struct SeriesPart {
    name: String,
    raw: String,
    ohms: f64,
    net_a: String,
    net_b: String,
}

/// Collect the series parts: two-pin instances with numeric values.
fn collect_series_parts(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    pin_to_net: &BTreeMap<(String, String), String>,
) -> Vec<SeriesPart> {
    let mut parts = Vec::new();
    for inst in instances.values() {
        let Some(info) = type_info.get(&inst.type_name) else { continue };
        if info.pins.len() != 2 {
            continue;
        }
        let Some(raw) = inst.properties.iter().find(|(k, _)| k == "value").map(|(_, v)| v.clone())
        else {
            continue;
        };
        let Some(ohms) = parse_ohms(&raw) else { continue };
        if ohms <= 0.0 {
            continue;
        }
        let p0 = (inst.name.clone(), info.pins[0].0.clone());
        let p1 = (inst.name.clone(), info.pins[1].0.clone());
        let (Some(n0), Some(n1)) = (pin_to_net.get(&p0), pin_to_net.get(&p1)) else {
            continue;
        };
        if n0 == n1 {
            continue;
        }
        parts.push(SeriesPart {
            name: inst.name.clone(),
            raw,
            ohms,
            net_a: n0.clone(),
            net_b: n1.clone(),
        });
    }
    parts
}

/// The part graph the derivation runs over.
struct PartGraph {
    parts: Vec<SeriesPart>,
    attached: BTreeMap<String, Vec<usize>>,
    voltage: BTreeMap<String, f64>,
    divided: std::collections::HashSet<String>,
}

/// Is `net` a divider mid node? Unclassed, not driven, exactly two attached
/// parts, both far sides classed, and the far sides are DIFFERENT nets (a
/// parallel pair shares one far net — that is not a divider).
fn is_divider_mid(net: &str, g: &PartGraph, drives: &BTreeMap<String, f64>) -> bool {
    if g.voltage.contains_key(net) || drives.contains_key(net) || g.divided.contains(net) {
        return false;
    }
    let Some(idxs) = g.attached.get(net) else { return false };
    if idxs.len() != 2 {
        return false;
    }
    let (ia, ib) = (idxs[0], idxs[1]);
    let (pa, pb) = (&g.parts[ia], &g.parts[ib]);
    let fa = if pa.net_a == net { &pa.net_b } else { &pa.net_a };
    let fb = if pb.net_a == net { &pb.net_b } else { &pb.net_a };
    fa != fb && g.voltage.contains_key(fa) && g.voltage.contains_key(fb)
}

/// (a) Divider pass: class every eligible mid net. Returns true when
/// something was classed.
fn divider_pass(
    g: &mut PartGraph,
    drives: &BTreeMap<String, f64>,
    proved: &mut Vec<String>,
) -> bool {
    let mid_nets: Vec<String> = g
        .attached
        .keys()
        .filter(|net| is_divider_mid(net, g, drives))
        .cloned()
        .collect();
    let mut any = false;
    for mid in mid_nets {
        let idxs = &g.attached[&mid];
        let (ia, ib) = (idxs[0], idxs[1]);
        let (pa, pb) = (&g.parts[ia], &g.parts[ib]);
        let far_a = if pa.net_a == mid { &pa.net_b } else { &pa.net_a };
        let far_b = if pb.net_a == mid { &pb.net_b } else { &pb.net_a };
        let va = g.voltage[far_a];
        let vb = g.voltage[far_b];
        // General two-resistor divider — orientation-free, handles a 0 V
        // ground reference on either side.
        let v_mid = (va / pa.ohms + vb / pb.ohms) / (1.0 / pa.ohms + 1.0 / pb.ohms);
        g.voltage.insert(mid.clone(), v_mid);
        g.divided.insert(mid.clone());
        any = true;
        proved.push(format!(
            "V({}) = {} — voltage divider ({} = {} and {} = {})",
            mid, format_volts(v_mid), far_a, format_volts(va), far_b, format_volts(vb)
        ));
    }
    any
}

/// (b)+(c) Contribution + KCL pass: per-part Ohm currents rebuilt from
/// scratch against the current voltage classes, then SUMMED per net
/// (parallel branches add — Kirchhoff's current law). Records a proof
/// fact per part and a sum fact per multi-branch net.
fn contribution_pass(g: &mut PartGraph, check: &mut VoltageCheck) -> bool {
    let mut contributions: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for p in &g.parts {
        let va = g.voltage.get(&p.net_a).copied();
        let vb = g.voltage.get(&p.net_b).copied();
        let (target, source_net, across, current) = match (va, vb) {
            (Some(a), Some(b)) if (a - b).abs() <= f64::EPSILON => continue,
            (Some(a), Some(b)) if a > b => (&p.net_b, &p.net_a, a - b, (a - b) / p.ohms),
            (Some(a), Some(b)) => (&p.net_a, &p.net_b, b - a, (b - a) / p.ohms),
            (Some(a), None) => (&p.net_b, &p.net_a, a, a / p.ohms),
            (None, Some(b)) => (&p.net_a, &p.net_b, b, b / p.ohms),
            (None, None) => continue,
        };
        contributions.entry(target.clone()).or_default().push(current);
        check.proved.push(format!(
            "I({} -> {}) = {} / {} ({} = {}) = {} — Ohm's law through the series part",
            source_net, target, format_volts(across), format_volts(p.ohms), p.name, p.raw,
            format_amps(current)
        ));
    }
    let mut net_current: BTreeMap<String, f64> = BTreeMap::new();
    for (net, contribs) in contributions {
        let total: f64 = contribs.iter().sum();
        net_current.insert(net.clone(), total);
        if contribs.len() > 1 {
            check.proved.push(format!(
                "I({}) = {} — Kirchhoff: {} parallel branches sum",
                net,
                format_amps(total),
                contribs.len()
            ));
        }
    }
    let changed = check.net_current != net_current;
    check.net_current = net_current;
    changed
}

/// B4 flagship — Ohm's-law derivation over the part graph, to a fixpoint:
/// series current, voltage dividers, and Kirchhoff parallel summing. See
/// the pass functions for the individual rules.
fn derive_current(
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    net_voltage: BTreeMap<String, f64>,
    check: &mut VoltageCheck,
) {
    let parts = collect_series_parts(instances, type_info, pin_to_net);
    let mut attached: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, p) in parts.iter().enumerate() {
        attached.entry(p.net_a.clone()).or_default().push(idx);
        attached.entry(p.net_b.clone()).or_default().push(idx);
    }

    let mut g = PartGraph {
        parts,
        attached,
        voltage: net_voltage,
        divided: Default::default(),
    };
    let drives = g.voltage.clone();

    let max_passes = g.parts.len() + 2;
    for _ in 0..max_passes {
        let divider_changed = divider_pass(&mut g, &drives, &mut check.proved);
        let current_changed = contribution_pass(&mut g, check);
        if !divider_changed && !current_changed {
            break;
        }
    }
    // Voltage classes from drives AND dividers both feed tolerance checks.
    check.net_voltage = g.voltage;
}

/// Prove (or violate) the postcondition current bounds against the derived
/// current classes.
fn check_current_bounds(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    pin_to_net: &BTreeMap<(String, String), String>,
    check: &mut VoltageCheck,
) {
    for (pin, bound, is_upper) in collect_current_bounds(items, instances, type_pins) {
        let Some(net_name) = pin_to_net.get(&(pin.component.clone(), pin.pin.clone())) else {
            continue;
        };
        let Some(derived) = check.net_current.get(net_name).copied() else {
            continue;
        };
        if is_upper && derived > bound + f64::EPSILON {
            check.violations.push(format!(
                "pin '{}.{}' on net '{}' carries a derived current of {} but its postcondition \
                 bounds it at {} — the bound is violated by the derived physics. \
                 fix: raise the series resistance, lower the drive voltage, or use a part \
                 rated for the derived current.",
                pin.component, pin.pin, net_name, format_amps(derived), format_amps(bound)
            ));
        }
    }
}

/// Current formatting for diagnostics.
fn format_amps(a: f64) -> String {
    if a < 1.0 {
        format!("{:.1} mA", a * 1e3)
    } else {
        format!("{:.2} A", a)
    }
}

/// Voltage formatting for diagnostics: trim trailing zeros (5.0 → "5 V",
/// 3.3 → "3.3 V").
fn format_volts(v: f64) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{} V", s)
}

/// Derive the netlist for an electronics program.
pub fn derive_netlist(items: &[TopLevel]) -> ElectronicsNetlist {
    let (type_pins, type_info) = collect_type_pins(items);
    if type_pins.is_empty() {
        return ElectronicsNetlist::default();
    }
    let instance_list = collect_instances(items, &type_pins);
    let instances: BTreeMap<String, &ComponentInstance> = instance_list
        .iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    let mut ds = DisjointSet::new();
    // Net names from `net <name>:` annotations — maps union-find root → name.
    let mut net_names: BTreeMap<String, String> = BTreeMap::new();
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        // PREconditions are topology. Postconditions are physics — skipped.
        let mut triples = Vec::new();
        collect_eq_triples(&t.contract.pre_condition, &mut triples);
        for (l, r, name) in triples {
            let (Some(lp), Some(rp)) = (
                resolve_pin(&l, &instances, &type_pins),
                resolve_pin(&r, &instances, &type_pins),
            ) else {
                continue;
            };
            let lk = pin_key(&lp.component, &lp.pin);
            let rk = pin_key(&rp.component, &rp.pin);
            ds.make(lk.clone());
            ds.make(rk.clone());
            ds.union(&lk, &rk);
            // Record the net name on the resulting root.
            if let Some(name) = name {
                let root = ds.find(&lk);
                net_names.entry(root).or_insert(name);
            }
        }
    }

    // Group members by root; sort everything for determinism (HashMap rule).
    let mut groups: BTreeMap<String, Vec<PinRef>> = BTreeMap::new();
    let mut all_pins: Vec<PinRef> = Vec::new();
    for inst in instances.values() {
        for (pname, number) in &type_pins[&inst.type_name] {
            all_pins.push(PinRef {
                component: inst.name.clone(),
                pin: pname.clone(),
                number: *number,
            });
        }
    }
    all_pins.sort();
    for p in &all_pins {
        let key = pin_key(&p.component, &p.pin);
        ds.make(key.clone());
        let root = ds.find(&key.clone());
        groups.entry(root).or_default().push(p.clone());
    }

    let mut nets = Vec::new();
    let mut dangling = Vec::new();
    let mut net_index = 0;
    for (root, members) in groups {
        if members.len() >= 2 {
            net_index += 1;
            let name = net_names.remove(&root).unwrap_or_else(|| format!("N{}", net_index));
            nets.push(Net { name, pins: members });
        } else {
            let p = &members[0];
            dangling.push(format!(
                "pin '{}.{}' (pin {}) is on no net — it never appears in a precondition pin equality. \
                 State its connection, e.g. [{}.{}.voltage == other.pin.voltage]",
                p.component, p.pin, p.number, p.component, p.pin
            ));
        }
    }

    // Voltage classes need the nets and instances before they move into the
    // result struct.
    let voltage = derive_voltage(items, &nets, &instances, &type_pins, &type_info);

    ElectronicsNetlist {
        components: instance_list,
        nets,
        dangling,
        is_electronics: true,
        voltage,
        type_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;
    use crate::lexer::tokenize;

    fn analyze(src: &str) -> ElectronicsNetlist {
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        derive_netlist(&items)
    }

    const LED_CIRCUIT: &str = r#"
        struct Pin { voltage: Float; current: Float; };
        type Resistor { pin a; pin b; reference "R"; };
        type Led { pin a; pin k; reference "D"; };
        type Connector { pin vcc; pin gnd; reference "J"; };

        let j1: Connector = Connector { value: "JST-2" };
        let r1: Resistor = Resistor { value: "330" };
        let d1: Led = Led { value: "red" };

        txn powered
            [j1.vcc.voltage == r1.a.voltage && r1.b.voltage == d1.a.voltage && d1.k.voltage == j1.gnd.voltage]
            [d1.a.current > 0.0 && d1.a.current <= 0.02]
        {
        }
    "#;


    #[test]
    fn derives_chain_nets_from_precondition_equalities() {
        let nl = analyze(LED_CIRCUIT);
        assert!(nl.is_electronics);
        assert_eq!(nl.components.len(), 3);
        assert_eq!(nl.nets.len(), 3, "three equality chains → three nets");
        assert!(nl.dangling.is_empty(), "all six pins connected: {:?}", nl.dangling);
        // Each net has exactly two pins in this circuit.
        for net in &nl.nets {
            assert_eq!(net.pins.len(), 2);
        }
    }

    #[test]
    fn transitive_closure_merges_chain_into_one_net() {
        let src = r#"
            struct Pin { voltage: Float; };
            type A { pin x; pin y;     reference "A";
};
            type B { pin z;     reference "B";
};
            let a: A = A { };
            let b: B = B { };
            txn t
                [a.x.voltage == a.y.voltage && a.y.voltage == b.z.voltage]
                [a.x.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.nets.len(), 1, "transitive chain is ONE net");
        assert_eq!(nl.nets[0].pins.len(), 3);
    }

    #[test]
    fn postcondition_does_not_create_nets() {
        let src = r#"
            struct Pin { voltage: Float; };
            type A { pin x; pin y;     reference "A";
};
            let a: A = A { };
            txn t
                [a.x.voltage >= 0.0]
                [a.x.voltage == a.y.voltage]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.nets.is_empty(), "postcondition equality is not wiring");
        assert_eq!(nl.dangling.len(), 2);
    }

    #[test]
    fn dangling_pin_is_reported() {
        let src = r#"
            struct Pin { voltage: Float; };
            type A { pin x; pin y;     reference "A";
};
            type B { pin z;     reference "B";
};
            let a: A = A { };
            let b: B = B { };
            txn t
                [a.x.voltage == a.y.voltage]
                [a.x.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.nets.len(), 1);
        assert_eq!(nl.dangling.len(), 1, "b.z never wired");
        assert!(nl.dangling[0].contains("b.z"), "diagnostic names the pin: {}", nl.dangling[0]);
    }

    #[test]
    fn net_naming_is_deterministic() {
        let a = analyze(LED_CIRCUIT);
        let b = analyze(LED_CIRCUIT);
        let names: Vec<_> = a.nets.iter().map(|n| n.name.clone()).collect();
        let names2: Vec<_> = b.nets.iter().map(|n| n.name.clone()).collect();
        assert_eq!(names, names2);
        assert_eq!(names, vec!["N1", "N2", "N3"]);
    }

    // ── 2026-09-11 (electrical proving): voltage classes ─────────────

    #[test]
    fn contract_equality_drives_net_voltage() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; tolerance any; };
            type Load { pin vin; reference "L"; tolerance 5.5; };
            let p1: Power = Power { };
            let l1: Load = Load { };
            txn apply
                [p1.vout.voltage == l1.vin.voltage && p1.vout.voltage == 5.0]
                [l1.vin.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.voltage.net_voltage.len(), 1);
        let (net, v) = nl.voltage.net_voltage.iter().next().unwrap();
        assert_eq!(net, "N1");
        assert!((v - 5.0).abs() < 1e-9, "drive class 5.0, got {}", v);
        assert!(nl.voltage.violations.is_empty(), "5.0 V into a 5.5 V-rated pin: {:?}", nl.voltage.violations);
    }

    #[test]
    fn overvoltage_into_rated_pin_is_a_violation() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout;     reference "P";
};
            type Led { pin a; pin k; tolerance 3.3;     reference "L";
};
            let p1: Power = Power { };
            let d1: Led = Led { };
            txn apply
                [p1.vout.voltage == d1.a.voltage && p1.vout.voltage == 5.0]
                [d1.a.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.iter().any(|v| v.contains("5 V") && v.contains("d1.a") && v.contains("3.3")),
            "overvoltage diagnostic names net, pin, drive and rating: {:?}", nl.voltage.violations);
    }

    #[test]
    fn disagreeing_drives_are_a_shorted_supply() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Rail { pin hi; pin lo; reference "R"; tolerance any; };
            type Load { pin vin; reference "L"; tolerance any; };
            let r1: Rail = Rail { };
            let l1: Load = Load { };
            txn apply
                [r1.hi.voltage == l1.vin.voltage && r1.lo.voltage == l1.vin.voltage && r1.hi.voltage == 5.0 && r1.lo.voltage == 3.3]
                [l1.vin.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.iter().any(|v| v.contains("shorted supply") && v.contains("5 V") && v.contains("3.3")),
            "conflict diagnostic names both levels: {:?}", nl.voltage.violations);
    }

    #[test]
    fn unrated_on_driven_net_needs_a_declared_decision() {
        // 2026-09-11 (B4, decision D8): no tolerance clause on a driven net
        // is an UNDECLARED decision — a violation; `tolerance any;` is the
        // declared opt-out.
        let unrated = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; };
            type Load { pin vin; reference "L"; };
            let p1: Power = Power { };
            let l1: Load = Load { };
            txn apply
                [p1.vout.voltage == l1.vin.voltage && p1.vout.voltage == 12.0]
                [l1.vin.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(unrated);
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("undeclared decision") && v.contains("l1.vin")),
            "unrated pin on a driven net must violate: {:?}", nl.voltage.violations
        );
        assert!(nl.voltage.net_voltage.values().any(|v| (*v - 12.0).abs() < 1e-9));

        let declared_any = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; tolerance any; };
            type Load { pin vin; reference "L"; tolerance any; };
            let p1: Power = Power { };
            let l1: Load = Load { };
            txn apply
                [p1.vout.voltage == l1.vin.voltage && p1.vout.voltage == 12.0]
                [l1.vin.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(declared_any);
        assert!(nl.voltage.violations.is_empty(), "`tolerance any` declares the decision: {:?}", nl.voltage.violations);
    }

    // ── B4 flagship: Ohm's-law current derivation ────────────────────

    #[test]
    fn ohms_law_derives_current_and_proves_bounds() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; tolerance any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Led { pin a; pin k; reference "D"; tolerance 3.6; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330" };
            let d1: Led = Led { };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == d1.a.voltage && p1.vout.voltage == 3.3]
                [d1.a.current > 0.0 && d1.a.current <= 0.02]
            { }
        "#;
        let nl = analyze(src);
        // I = 3.3 V / 330 ohm = 10 mA, derived through the series part.
        let ic = nl.voltage.net_current.values().next().copied();
        assert!(ic.map_or(false, |i| (i - 0.01).abs() < 1e-9), "derived 10 mA, got {:?}", ic);
        assert!(
            nl.voltage.proved.iter().any(|p| p.contains("Ohm's law") && p.contains("330")),
            "the derivation is a recorded PROOF fact: {:?}", nl.voltage.proved
        );
        assert!(nl.voltage.violations.is_empty(), "10 mA within the 20 mA bound: {:?}", nl.voltage.violations);
    }

    #[test]
    fn voltage_divider_classes_the_mid_net() {
        // 5 V across 1k + 1k: the mid node sits at 2.5 V — derived, so a
        // 2.0 V-rated pin there violates and a 3.3 V-rated one passes.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; tolerance any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Sensor { pin s; reference "S"; tolerance 2.0; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "1k" };
            let r2: Resistor = Resistor { value: "1k" };
            let s1: Sensor = Sensor { };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == r2.a.voltage && r2.b.voltage == p1.gnd.voltage && r1.b.voltage == s1.s.voltage]
                [p1.vout.voltage == 5.0 && p1.gnd.voltage == 0.0]
            { }
        "#;
        let nl = analyze(src);
        let mid = nl.voltage.net_voltage.values().find(|v| (**v - 2.5).abs() < 1e-6);
        assert!(mid.is_some(), "mid node derives to 2.5 V: {:?}", nl.voltage.net_voltage);
        assert!(
            nl.voltage.proved.iter().any(|p| p.contains("voltage divider")),
            "the divider is a recorded PROOF fact: {:?}", nl.voltage.proved
        );
    }

    #[test]
    fn parallel_branches_sum_kirchhoff() {
        // Two 660-ohm parts in parallel from 3.3 V: each draws 5 mA, the
        // shared net carries the SUM — 10 mA — and a 8 mA bound violates
        // while a 12 mA bound holds.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; tolerance any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "660" };
            let r2: Resistor = Resistor { value: "660" };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == p1.gnd.voltage && r1.a.voltage == r2.a.voltage && r2.b.voltage == p1.gnd.voltage && p1.vout.voltage == 3.3 && p1.gnd.voltage == 0.0]
                [p1.vout.current <= 0.008]
            { }
        "#;
        let nl = analyze(src);
        // Both branches land on the gnd net: 5 mA + 5 mA = 10 mA total.
        assert!(
            nl.voltage.net_current.values().any(|i| (*i - 0.01).abs() < 1e-6),
            "parallel currents sum: {:?}", nl.voltage.net_current
        );
        assert!(
            nl.voltage.proved.iter().any(|p| p.contains("Kirchhoff")),
            "the sum is a recorded PROOF fact: {:?}", nl.voltage.proved
        );
    }

    #[test]
    fn ohms_law_violates_an_exceeded_current_bound() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; tolerance any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Led { pin a; pin k; reference "D"; tolerance 3.6; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "33" };
            let d1: Led = Led { };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == d1.a.voltage && p1.vout.voltage == 3.3]
                [d1.a.current > 0.0 && d1.a.current <= 0.02]
            { }
        "#;
        let nl = analyze(src);
        // 3.3 V / 33 ohm = 100 mA — the postcondition is violated by the
        // DERIVED physics, not by any explicit statement.
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("100.0 mA") && v.contains("violated by the derived physics")),
            "over-current must violate against the derivation: {:?}", nl.voltage.violations
        );
    }

    #[test]
    fn agreeing_drives_do_not_conflict() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Rail { pin hi; pin lo; reference "R"; tolerance any; };
            type Load { pin vin; reference "L"; tolerance any; };
            let r1: Rail = Rail { };
            let l1: Load = Load { };
            txn apply
                [r1.hi.voltage == l1.vin.voltage && r1.lo.voltage == l1.vin.voltage && r1.hi.voltage == 5.0 && r1.lo.voltage == 5.0]
                [l1.vin.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.is_empty(), "same drive twice is redundant, not a short: {:?}", nl.voltage.violations);
    }

    #[test]
    fn non_electronics_program_yields_default_netlist() {
        let nl = analyze("let x: Int = 5;");
        assert!(!nl.is_electronics);
        assert!(nl.nets.is_empty());
    }

    #[test]
    fn named_net_appears_in_netlist() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [net vcc: j1.p1.voltage == r1.a.voltage && net gnd: j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        let vcc = nl.nets.iter().find(|n| n.name == "vcc");
        let gnd = nl.nets.iter().find(|n| n.name == "gnd");
        assert!(vcc.is_some(), "expected net 'vcc', got: {:?}", nl.nets.iter().map(|n| &n.name).collect::<Vec<_>>());
        assert!(gnd.is_some(), "expected net 'gnd', got: {:?}", nl.nets.iter().map(|n| &n.name).collect::<Vec<_>>());
        assert_eq!(vcc.unwrap().pins.len(), 2, "vcc should connect j1.p1 and r1.a");
        assert_eq!(gnd.unwrap().pins.len(), 2, "gnd should connect j1.p2 and r1.b");
    }

    #[test]
    fn unnamed_nets_still_get_generated_names() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.nets.iter().any(|n| n.name.starts_with('N')), "unnamed nets should get N<N> names");
    }
}
