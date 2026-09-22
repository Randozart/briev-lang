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
    /// 2026-09-21 (E12): resolved class properties, index-aligned to
    /// `pins`. Defaults apply for unclassed pins.
    pub pin_classes: Vec<PinClassProps>,
    /// Max voltage any pin of this type tolerates — `!> Tolerance: 3.3`.
    /// None = unrated: the pin places no constraint (skeleton semantics;
    /// per-pin ratings are a follow-on).
    pub tolerance: Option<f64>,
    /// 2026-09-12 (power ratings): max watts this part dissipates —
    /// `rating 0.25;` → Some(watts), `rating any;` → Some(INFINITY),
    /// no clause → None (the proven-dissipation violation decides).
    pub rating: Option<f64>,
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
    /// 2026-09-12 (named nets): conflicting names on one equivalence class —
    /// one electrical node cannot carry two names.
    pub net_conflicts: Vec<String>,
    /// True when the program contains any pin-carrying component type.
    pub is_electronics: bool,
    /// Schematic facts per component type name.
    pub type_info: BTreeMap<String, TypeInfo>,
    /// 2026-09-21 (E12): pin-class ascription failures — a pin names a
    /// class type that is not in scope. Hard diagnostics: the backend
    /// refuses to emit.
    pub class_errors: Vec<String>,
    /// 2026-09-21 (E13): decoupling-convention violations — an instance of
    /// a `spec Decouple` type without a bridging `spec Decoupler` part.
    /// Hard diagnostics: the backend refuses to emit.
    pub convention_errors: Vec<String>,
    /// 2026-09-21 (E7): source-pin budget violations — a net's derived
    /// draw past its stated `budget`. Hard diagnostics: the backend
    /// refuses to emit.
    pub budget_errors: Vec<String>,
    /// 2026-09-21 (E14a): drive-intent failures — an intent that could
    /// not complete, or completed ambiguously. Hard diagnostics.
    pub intent_errors: Vec<String>,
    /// 2026-09-21 (E14a): wiring facts the intents established, reported
    /// so synthesized connections carry provenance (design record D3).
    pub intent_proofs: Vec<String>,
    /// 2026-09-21 (D16 phase 2): mechanism bridges — conditional edges
    /// closed when the control net's region holds. The phase-3 complement
    /// check and eventual per-region physics consume these.
    pub conditional_bridges: Vec<String>,
    /// 2026-09-11 (electrical proving): net voltage classes + violations.
    pub voltage: VoltageCheck,
}

/// 2026-09-21 (E12, design record D6): resolved property set for a pin's
/// class fundamental. Defaults apply when the pin carries no ascription:
/// passive KiCad electrical type, no no-connect marking. The class NAMES
/// live in stdlib (`Power`/`Ground`/… in std/electronics.bv); the
/// compiler reads properties generically and knows no class names.
#[derive(Debug, Clone, PartialEq)]
pub struct PinClassProps {
    /// `spec KicadType: "power_in";` on the class fundamental — the KiCad
    /// electrical pin type for symbol emission.
    pub kicad_type: String,
    /// `spec NoConnect: true;` — the pin is intentionally unconnected and
    /// is exempt from the dangling-pin error.
    pub no_connect: bool,
    /// 2026-09-21 (E13): `spec Supply: true;` — a rail pin that the
    /// decoupling convention attaches across (with a return pin).
    pub supply: bool,
    /// 2026-09-21 (E13): `spec Return: true;` — the return-side rail pin.
    pub return_pin: bool,
    /// 2026-09-21 (E14a): `spec CanDrive: true;` — the class may hold a
    /// drive; drive intents enumerate candidates through this property.
    pub can_drive: bool,
    /// 2026-09-21 (D16 phase 2): `spec Control: true;` — a mechanism's
    /// control (gate-class) pin; the when-condition net drives it.
    pub control: bool,
    /// 2026-09-21 (D16 phase 2): `spec Switchable: true;` — a mechanism's
    /// path pin; bridge requests land on it.
    pub switchable: bool,
}

impl Default for PinClassProps {
    fn default() -> Self {
        PinClassProps {
            kicad_type: "passive".to_string(),
            no_connect: false,
            supply: false,
            return_pin: false,
            can_drive: false,
            control: false,
            switchable: false,
        }
    }
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
    /// Membership test (E14a): a pin is connected iff it has been made.
    fn contains(&self, k: &str) -> bool {
        self.parent.contains_key(k)
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

/// Type name → its metadata property bag (`spec Key: value;` clauses land
/// there). One lookup path for every property-driven consumer.
fn collect_type_metadata(
    items: &[TopLevel],
) -> BTreeMap<String, &std::collections::HashMap<String, crate::ast::PropertyValue>> {
    items
        .iter()
        .filter_map(|it| match it {
            TopLevel::TypeDef(td) => Some((td.name.clone(), &td.body.metadata)),
            _ => None,
        })
        .collect()
}

/// 2026-09-21 (E12): metadata property readers — `spec` values arrive as
/// `PropertyValue` variants; these tolerate the identifier spellings too.
fn property_string(pv: &crate::ast::PropertyValue) -> Option<String> {
    match pv {
        crate::ast::PropertyValue::String(s) => Some(s.clone()),
        crate::ast::PropertyValue::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

fn property_bool(pv: &crate::ast::PropertyValue) -> Option<bool> {
    match pv {
        crate::ast::PropertyValue::Bool(b) => Some(*b),
        crate::ast::PropertyValue::Identifier(s) if s == "true" => Some(true),
        crate::ast::PropertyValue::Identifier(s) if s == "false" => Some(false),
        _ => None,
    }
}

/// Resolve one pin's class ascription into its property set (E12/D16).
/// Unknown class type → hard class error + default props (the analysis
/// still proceeds so every ascription failure is reported).
fn resolve_pin_class_props(
    pname: &str,
    declared_class: &std::collections::HashMap<&str, &str>,
    class_defs: &BTreeMap<
        String,
        &std::collections::HashMap<String, crate::ast::PropertyValue>,
    >,
    td: &crate::ast::top::TypeDef,
    class_errors: &mut Vec<String>,
) -> PinClassProps {
    let Some(cname) = declared_class.get(pname) else {
        return PinClassProps::default();
    };
    match class_defs.get(*cname) {
        Some(md) => PinClassProps {
            kicad_type: md
                .get("kicad_type")
                .and_then(property_string)
                .unwrap_or_else(|| "passive".to_string()),
            no_connect: md.get("no_connect").and_then(property_bool).unwrap_or(false),
            supply: md.get("supply").and_then(property_bool).unwrap_or(false),
            return_pin: md.get("return").and_then(property_bool).unwrap_or(false),
            can_drive: md.get("can_drive").and_then(property_bool).unwrap_or(false),
            control: md.get("control").and_then(property_bool).unwrap_or(false),
            switchable: md.get("switchable").and_then(property_bool).unwrap_or(false),
        },
        None => {
            class_errors.push(format!(
                "pin '{}.{}' ascribes class '{}' — no such type is in scope. Declare it (see std/electronics.bv: Power, Ground, In, Out, Io, IoOd, Nc) or drop the ascription",
                td.name, pname, cname
            ));
            PinClassProps::default()
        }
    }
}

/// Extract declared pins per component type from TypeDef bodies, plus the
/// schematic facts (`!> Reference` prefix) each type carries. Also resolves
/// pin-class ascriptions (E12) against the program's declared types —
/// the prelude injects the class fundamentals from std/electronics.bv.
fn collect_type_pins(
    items: &[TopLevel],
) -> (
    BTreeMap<String, Vec<(String, u64)>>,
    BTreeMap<String, TypeInfo>,
    Vec<String>,
) {
    let mut out = BTreeMap::new();
    let mut info = BTreeMap::new();
    // E12: every declared type is a candidate class fundamental — the
    // resolution reads its metadata property bag (spec KicadType /
    // spec NoConnect), nothing else.
    let class_defs = collect_type_metadata(items);
    let mut class_errors: Vec<String> = Vec::new();
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
                let rating = td.body.rating.as_ref().map(|r| match r {
                    crate::ast::top::Rating::Watts(w) => *w,
                    crate::ast::top::Rating::Any => f64::INFINITY,
                });
                let mut pins: Vec<(String, u64)> =
                    td.body.pins.iter().map(|p| (p.name.clone(), p.number)).collect();
                pins.sort_by_key(|&(_, n)| n);
                // 2026-09-21 (E12): resolve each pin's class ascription in
                // the pin's SORTED order, so `pin_classes` is index-aligned
                // to `pins` above.
                let declared_class: std::collections::HashMap<&str, &str> = td
                    .body
                    .pins
                    .iter()
                    .filter_map(|p| p.class_ref.as_deref().map(|c| (p.name.as_str(), c)))
                    .collect();
                let mut pin_classes = Vec::with_capacity(pins.len());
                for (pname, _) in &pins {
                    pin_classes.push(resolve_pin_class_props(
                        pname,
                        &declared_class,
                        &class_defs,
                        td,
                        &mut class_errors,
                    ));
                }
                info.insert(
                    td.name.clone(),
                    TypeInfo { reference_prefix: prefix, pins, pin_classes, tolerance, rating },
                );
            }
        }
    }
    (out, info, class_errors)
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
                Expr::UnitLiteral { value, unit } => Some((fname.clone(), format!("{}{}", value, unit))),
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
    // 2026-09-21 (E11): `u2.gpio[3]` — indexed element of a pin array.
    // The parser expanded the array into elements named `gpio[0]…`, so
    // the element resolves by its bracketed name against the same table.
    let (inst_name, pin_name) = match base.as_ref() {
        Expr::Identifier(inst_name) => (inst_name, pin_name.clone()),
        Expr::Index(inner, idx) => {
            let Expr::Field(inner_base, arr_name) = inner.as_ref() else {
                return None;
            };
            let Expr::Identifier(inst_name) = inner_base.as_ref() else {
                return None;
            };
            let Expr::Decimal(d) = idx.as_ref() else {
                return None;
            };
            (inst_name, format!("{}[{}]", arr_name, d))
        }
        _ => return None,
    };
    let inst = instances.get(inst_name)?;
    let pins = type_pins.get(&inst.type_name)?;
    let (_, number) = pins.iter().find(|(n, _)| n == &pin_name)?;
    Some(PinRef {
        component: inst.name.clone(),
        pin: pin_name,
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
/// Extract a numeric value from an expression — Float, Decimal, or
/// UnitLiteral. For UnitLiteral, the value is returned directly (the
/// caller decides how to interpret the unit).
fn extract_numeric(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::Float(f) => Some(*f),
        Expr::Decimal(d) => Some(*d as f64),
        Expr::UnitLiteral { value, .. } => Some(*value),
        _ => None,
    }
}

/// Extract a voltage value from an expression — V or bare numeric.
fn extract_voltage(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::UnitLiteral { value, unit } if unit == "V" => Some(*value),
        _ => extract_numeric(expr),
    }
}

/// Extract a current value from an expression — A, mA, or bare numeric.
fn extract_current(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::UnitLiteral { value, unit } if unit == "A" => Some(*value),
        Expr::UnitLiteral { value, unit } if unit == "mA" => Some(value / 1000.0),
        _ => extract_numeric(expr),
    }
}

/// Extract a resistance value from an expression — R, Ω, or bare numeric.
fn extract_resistance(expr: &Expr) -> Option<f64> {
    match expr {
        Expr::UnitLiteral { value, unit } if unit == "R" || unit == "Ω" => Some(*value),
        _ => extract_numeric(expr),
    }
}

fn voltage_drive(
    l: &Expr,
    r: &Expr,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<(PinRef, f64)> {
    if let Some(pin) = resolve_voltage_pin(l, instances, type_pins) {
        if let Some(v) = extract_voltage(r) {
            return Some((pin, v));
        }
    }
    if let Some(pin) = resolve_voltage_pin(r, instances, type_pins) {
        if let Some(v) = extract_voltage(l) {
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
    derive_power(&pin_to_net, instances, type_info, &mut check);
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
            // Literal on either side: `[x <= 0.02]`, `[x <= 10mA]`, or `[0.02 >= x]`.
            let (pin_expr, lit) = match (&l, &r) {
                (p, r) if extract_current(r).is_some() => (p, extract_current(r).unwrap()),
                (l, p) if extract_current(l).is_some() => (p, extract_current(l).unwrap()),
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

/// 2026-09-12 (power ratings): P = V × I per part from the FINAL voltage
/// classes. A part with both endpoints classed and ΔV > 0 has PROVEN
/// dissipation — the rating clause must declare the decision (same doctrine
/// as tolerance): exceeding the rating is a violation, no clause is an
/// undeclared decision, within the rating records a proof fact. A part with
/// an unclassed endpoint has no proven ΔV — nothing is forced (the LED in
/// the demo is non-ohmic, so its series resistor stays one-sided there).
fn derive_power(
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    check: &mut VoltageCheck,
) {
    let parts = collect_series_parts(instances, type_info, pin_to_net);
    for p in parts {
        let (Some(va), Some(vb)) = (
            check.net_voltage.get(&p.net_a).copied(),
            check.net_voltage.get(&p.net_b).copied(),
        ) else {
            continue;
        };
        let dv = (va - vb).abs();
        if dv <= f64::EPSILON {
            continue;
        }
        let watts = dv * dv / p.ohms;
        let Some(inst) = instances.get(&p.name) else { continue };
        let Some(info) = type_info.get(&inst.type_name) else { continue };
        match info.rating {
            None => check.violations.push(format!(
                "part '{}' ({}, {}) dissipates a derived {} but its type declares no power rating — \
                 an unstated rating on a proven-dissipating part is an undeclared decision. \
                 fix: add `rating <watts>;` rated above the derived dissipation, or `rating any;` \
                 to declare the part unrated on purpose.",
                p.name, inst.type_name, p.raw, format_watts(watts)
            )),
            Some(rated) if watts > rated + f64::EPSILON => check.violations.push(format!(
                "part '{}' ({}, {}) dissipates a derived {} but is rated {} — the derived physics \
                 exceeds the declared rating. fix: raise the rating to a real part class, spread the \
                 drop across more parts, or lower the drive.",
                p.name, inst.type_name, p.raw, format_watts(watts), format_watts(rated)
            )),
            Some(rated) => check.proved.push(format!(
                "P({}) = {} <= {} rating — within the declared rating",
                p.name, format_watts(watts), format_watts(rated)
            )),
        }
    }
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
fn format_watts(w: f64) -> String {
    if w >= 1.0 {
        format!("{:.2} W", w)
    } else if w >= 1e-3 {
        format!("{:.1} mW", w * 1e3)
    } else {
        format!("{:.1} uW", w * 1e6)
    }
}

fn format_volts(v: f64) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{} V", s)
}

/// Derive the netlist for an electronics program.
/// Union pins connected by precondition equalities; returns the named-net
/// annotations as (pin_key, name) pairs, resolved later against final roots.
fn collect_pin_unions(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    ds: &mut DisjointSet,
) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        // PREconditions are topology. Postconditions are physics — skipped.
        let mut triples = Vec::new();
        collect_eq_triples(&t.contract.pre_condition, &mut triples);
        for (l, r, name) in triples {
            let (Some(lp), Some(rp)) = (
                resolve_pin(&l, instances, type_pins),
                resolve_pin(&r, instances, type_pins),
            ) else {
                continue;
            };
            let lk = pin_key(&lp.component, &lp.pin);
            let rk = pin_key(&rp.component, &rp.pin);
            ds.make(lk.clone());
            ds.make(rk.clone());
            ds.union(&lk, &rk);
            if let Some(name) = name {
                named.push((lk, name));
            }
        }
    }
    named
}

/// Resolve named-net annotations onto FINAL union-find roots (a name keyed
/// during unioning would go stale when later merges change roots). Two
/// DIFFERENT names on one net are a conflict; the same name twice is
/// redundant, not a conflict. Returns root → name plus the raw conflicts
/// (root, first name, conflicting name) for the caller to format.
fn resolve_net_names(
    ds: &mut DisjointSet,
    named: Vec<(String, String)>,
) -> (BTreeMap<String, String>, Vec<(String, String, String)>) {
    let mut net_names: BTreeMap<String, String> = BTreeMap::new();
    let mut raw_conflicts: Vec<(String, String, String)> = Vec::new();
    for (pin_key, name) in named {
        let root = ds.find(&pin_key);
        match net_names.get(&root) {
            Some(existing) if *existing != name => {
                raw_conflicts.push((root, existing.clone(), name));
            }
            Some(_) => {}
            None => {
                net_names.insert(root, name);
            }
        }
    }
    (net_names, raw_conflicts)
}

/// Rail pins of one class kind from a resolved TypeInfo (E13).
fn rail_pins<'a>(ti: &'a TypeInfo, which_return: bool) -> Vec<&'a (String, u64)> {
    ti.pins
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            if which_return {
                ti.pin_classes[*i].return_pin
            } else {
                ti.pin_classes[*i].supply
            }
        })
        .map(|(_, p)| p)
        .collect()
}

/// Netlist lookups for the convention checks (E13) — one context parameter
/// instead of a table bundle on every helper.
struct NetlistContext<'a> {
    ds: &'a mut DisjointSet,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    /// Type metadata bags (`spec` clauses) — the decoupling convention
    /// reads `decouple` / `decoupler` from them.
    type_props: BTreeMap<String, &'a std::collections::HashMap<String, crate::ast::PropertyValue>>,
    /// Declared instances — the intent and mechanism machinery look pins
    /// up through them (consolidated so the walkers stay under the
    /// parameter gate).
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
}

impl<'a> NetlistContext<'a> {
    fn new(
        items: &'a [TopLevel],
        ds: &'a mut DisjointSet,
        type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
        type_info: &'a BTreeMap<String, TypeInfo>,
        instances: &'a BTreeMap<String, &'a ComponentInstance>,
    ) -> Self {
        NetlistContext {
            ds,
            type_pins,
            type_info,
            type_props: collect_type_metadata(items),
            instances,
        }
    }
}

/// True when `cap` has two terminals bridging the supply net to the return
/// net (one terminal each; the return-side terminal must not be no_connect).
fn bridges_rail(
    ctx: &mut NetlistContext,
    cap: &ComponentInstance,
    s_root: &String,
    r_root: &String,
) -> bool {
    let Some(cp) = ctx.type_pins.get(&cap.type_name) else {
        return false;
    };
    let Some(cti) = ctx.type_info.get(&cap.type_name) else {
        return false;
    };
    cp.iter().enumerate().any(|(i1, (n1, _))| {
        ctx.ds.find(&pin_key(&cap.name, n1)) == *s_root
            && cp.iter().enumerate().any(|(i2, (n2, _))| {
                i1 != i2
                    && !cti.pin_classes[i2].no_connect
                    && ctx.ds.find(&pin_key(&cap.name, n2)) == *r_root
            })
    })
}

/// 2026-09-21 (E13, design record D5): the decoupling convention — a
/// component type declaring `spec Decouple` must, per instance, have a
/// part whose type declares `spec Decoupler` bridging each supply-class
/// pin to a return-class pin of the same instance. Property-driven
/// throughout: the compiler knows no type or class names, only the
/// property interface (Rules 14/15).
fn check_decoupling(ctx: &mut NetlistContext) -> Vec<String> {
    let mut errors = Vec::new();
    let decouplers: Vec<&ComponentInstance> = ctx
        .instances
        .values()
        .filter(|c| {
            ctx.type_props
                .get(&c.type_name)
                .and_then(|m| m.get("decoupler"))
                .and_then(property_bool)
                .unwrap_or(false)
        })
        .map(|c| *c)
        .collect();
    for inst in ctx.instances.values() {
        let declares = ctx
            .type_props
            .get(&inst.type_name)
            .map(|m| m.contains_key("decouple"))
            .unwrap_or(false);
        if !declares {
            continue;
        }
        let Some(ti) = ctx.type_info.get(&inst.type_name) else {
            continue;
        };
        let supplies = rail_pins(ti, false);
        if supplies.is_empty() {
            errors.push(format!(
                "instance '{}' ({}) declares spec Decouple but its type has no supply-class pin — decoupling attaches across a supply (spec Supply) and a return (spec Return) pin",
                inst.name, inst.type_name
            ));
            continue;
        }
        for (pname, _) in &supplies {
            let s_root = ctx.ds.find(&pin_key(&inst.name, pname));
            let r_roots: Vec<String> = returns_of(ctx, inst);
            let bridged = decouplers.iter().any(|cap| {
                r_roots
                    .iter()
                    .any(|r_root| bridges_rail(ctx, cap, &s_root, r_root))
            });
            if !bridged {
                errors.push(format!(
                    "instance '{}' ({}) declares spec Decouple but no decoupling part bridges supply pin '{}' to a return-class pin — add a capacitor (its type declares spec Decoupler) across the rail",
                    inst.name, inst.type_name, pname
                ));
            }
        }
    }
    errors
}

/// Return-net roots of one instance: the union-find root of each
/// return-class pin (E13).
fn returns_of(
    ctx: &mut NetlistContext,
    inst: &ComponentInstance,
) -> Vec<String> {
    let Some(ti) = ctx.type_info.get(&inst.type_name) else {
        return Vec::new();
    };
    let Some(pins) = ctx.type_pins.get(&inst.type_name) else {
        return Vec::new();
    };
    rail_pins(ti, true)
        .iter()
        .filter_map(|(rname, _)| {
            pins.iter()
                .find(|(n, _)| n == rname)
                .map(|_| ctx.ds.find(&pin_key(&inst.name, rname)))
        })
        .collect()
}

/// 2026-09-21 (E12): pin keys ascribed a no_connect class — intentionally
/// unconnected, exempt from the dangling-pin error. Property-driven from
/// the class fundamentals; the compiler knows no class names (D6).
fn collect_no_connect_pins(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
) -> std::collections::HashSet<String> {
    let mut nc = std::collections::HashSet::new();
    for inst in instances.values() {
        let Some(ti) = type_info.get(&inst.type_name) else {
            continue;
        };
        for (i, (pname, _)) in ti.pins.iter().enumerate() {
            if ti.pin_classes[i].no_connect {
                nc.insert(pin_key(&inst.name, pname));
            }
        }
    }
    nc
}

/// The net (if any) currently carrying this pin key.
fn net_of<'a>(nets: &'a [Net], key: &str) -> Option<&'a Net> {
    nets.iter()
        .find(|n| n.pins.iter().any(|p| pin_key(&p.component, &p.pin) == key))
}

/// 2026-09-21 (E7): total derived current crossing net N — the sum of
/// branch currents of every series part attached to N (the KCL boundary
/// sum, direction-agnostic: a source pin's net has current LEAVING it,
/// which the per-net sink-side map cannot express).
fn net_draw(
    net: &str,
    parts: &[SeriesPart],
    net_voltage: &BTreeMap<String, f64>,
) -> Option<f64> {
    let mut total = 0.0f64;
    let mut any = false;
    for p in parts {
        if p.net_a != net && p.net_b != net {
            continue;
        }
        let va = net_voltage.get(&p.net_a).copied();
        let vb = net_voltage.get(&p.net_b).copied();
        let current = match (va, vb) {
            (Some(a), Some(b)) => (a - b).abs() / p.ohms,
            (Some(a), None) => a / p.ohms,
            (None, Some(b)) => b / p.ohms,
            (None, None) => continue,
        };
        total += current;
        any = true;
    }
    any.then_some(total)
}

/// 2026-09-21 (E7, design record D4): source-pin budgets — `budget
/// u1.out <= 250mA;` caps the derived current draw across the pin's net.
/// The roll-up is the KCL boundary sum over the B4-derived part graph;
/// nets with no derived draw pass vacuously — nothing provable flows.
/// Black-box draws (IC internals) are not yet derivable; the intent
/// machinery owns that later. Violations are hard budget_errors.
fn check_budgets(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    nets: &[Net],
    net_voltage: &BTreeMap<String, f64>,
) -> Vec<String> {
    // resolve_pin wants the (name, number) table; TypeInfo.pins is the
    // same data, number-sorted — derive the view locally.
    let type_pins: BTreeMap<String, Vec<(String, u64)>> = type_info
        .iter()
        .map(|(name, ti)| (name.clone(), ti.pins.clone()))
        .collect();
    let mut errors = Vec::new();
    if !items
        .iter()
        .any(|i| matches!(i, TopLevel::Budget(_)))
    {
        return errors;
    }
    // The part graph, rebuilt from the finished nets (cheap at board
    // scale; the B4 passes own the incremental machinery).
    let mut pin_to_net: BTreeMap<(String, String), String> = BTreeMap::new();
    for net in nets {
        for p in &net.pins {
            pin_to_net.insert((p.component.clone(), p.pin.clone()), net.name.clone());
        }
    }
    let parts = collect_series_parts(instances, type_info, &pin_to_net);
    let mut errors = Vec::new();
    for item in items {
        let TopLevel::Budget(b) = item else {
            continue;
        };
        let Expr::BinaryOp(op, l, r) = &b.contract else {
            errors.push(
                "a budget must state `budget <instance>.<pin> <= <current>;`".to_string(),
            );
            continue;
        };
        // Pin on either side of the comparison — mirror the voltage-drive
        // shape so `[250mA >= u1.out]` states the same budget.
        let (pin_expr, limit_expr) = match op {
            BinaryOpKind::Le => (l.as_ref(), r.as_ref()),
            BinaryOpKind::Ge => (r.as_ref(), l.as_ref()),
            _ => {
                errors.push(
                    "a budget compares a pin with a current limit using `<=` (or `>=`, reversed)"
                        .to_string(),
                );
                continue;
            }
        };
        let Some(pin) = resolve_pin(pin_expr, instances, &type_pins) else {
            errors.push(
                "a budget targets a pin — state `budget <instance>.<pin> <= <current>;`"
                    .to_string(),
            );
            continue;
        };
        let Some(limit) = extract_current(limit_expr) else {
            errors.push(
                "a budget limit is a current — state it with a unit (`250mA`, `1.5A`) or as a bare ampere number"
                    .to_string(),
            );
            continue;
        };
        let key = pin_key(&pin.component, &pin.pin);
        let Some(drawn) = net_of(nets, &key).and_then(|n| net_draw(&n.name, &parts, net_voltage))
        else {
            continue; // no derived draw on this net — nothing to cap
        };
        if drawn > limit {
            errors.push(format!(
                "budget exceeded: '{}.{}' allows {} but its net draws {} — the derived sum over the net (B4 fixpoint) is past the stated limit. Raise the budget or reduce the draw",
                pin.component,
                pin.pin,
                format_amps(limit),
                format_amps(drawn)
            ));
        }
    }
    errors
}

/// Disjoint-set membership: a pin is CONNECTED when it has been made in
/// the union-find (only wiring facts make pins).
fn pin_connected(ds: &DisjointSet, key: &str) -> bool {
    ds.contains(key)
}

/// A mechanism bridge request (D16 phase 2): a conditional connection
/// A-B, closed when the control pin's region holds.
struct BridgeRequest {
    node: String,
    a: PinRef,
    b: PinRef,
    control: PinRef,
    via: Option<String>,
}

/// Diagnostic sink for body-fact collection (E14a/D16) — keeps the
/// walker's parameter list flat.
struct FactSink<'a> {
    node: &'a str,
    proofs: &'a mut Vec<String>,
    errors: &'a mut Vec<String>,
    intents: &'a mut Vec<(String, String)>,
    bridges: &'a mut Vec<BridgeRequest>,
}

/// The region context threading through a guarded-fact walk (D16):
/// `control` Some means mechanizable — wiring facts become bridge
/// requests under it. `cond` Some (no control) means signal-level but
/// not mechanizable — the D7 gate. `via` is this region's strategy
/// selection.
#[derive(Clone, Copy)]
struct RegionCtx<'a> {
    cond: Option<&'a str>,
    control: Option<&'a PinRef>,
    via: Option<&'a str>,
}

/// Does this expression mention a pin of a declared instance? (E14a/D16:
/// pin-mentioning conditions are SIGNAL-LEVEL — the conditional-mechanism
/// trigger. Conditions referencing no pin are region-level facts.)
fn mentions_pin(expr: &Expr, ctx: &NetlistContext) -> bool {
    if resolve_pin(expr, ctx.instances, ctx.type_pins).is_some() {
        return true;
    }
    match expr {
        Expr::BinaryOp(_, l, r) => mentions_pin(l, ctx) || mentions_pin(r, ctx),
        Expr::Field(base, _) => mentions_pin(base, ctx),
        Expr::Index(l, r) => mentions_pin(l, ctx) || mentions_pin(r, ctx),
        _ => false,
    }
}

/// The control pin of a mechanizable condition (D16 phase 2): a single
/// comparison `Expr::BinaryOp` whose one operand resolves to a pin — the
/// condition's net feeds the mechanism control. Anything else → None
/// (region-level or non-mechanizable signal-level).
fn condition_control(expr: &Expr, ctx: &NetlistContext) -> Option<PinRef> {
    let Expr::BinaryOp(_, l, r) = expr else {
        return None;
    };
    resolve_pin(l, ctx.instances, ctx.type_pins)
        .or_else(|| resolve_pin(r, ctx.instances, ctx.type_pins))
}

/// Pull a trailing `via <Name>` marker out of a guarded body (D16 phase
/// 2): returns the strategy and the body without it.
fn take_via_strategy(body: &[Statement]) -> (Option<String>, Vec<Statement>) {
    let mut strategy = None;
    let mut rest = Vec::with_capacity(body.len());
    for stmt in body {
        if let Statement::MetadataAssignment(k, crate::ast::PropertyValue::Identifier(name)) = stmt
        {
            if k == "via" {
                strategy = Some(name.clone());
                continue;
            }
        }
        rest.push(stmt.clone());
    }
    (strategy, rest)
}

impl FactSink<'_> {
    /// Walk statements, threading the region context (E14a/D16).
    fn walk(
        &mut self,
        stmts: &[Statement],
        reg: RegionCtx,
        ctx: &mut NetlistContext,
    ) {
        for stmt in stmts {
            match stmt {
                Statement::Guarded(c, inner) => {
                    walk_guarded(c, inner, reg, ctx, self);
                }
                Statement::Assign(target, value) => {
                    if let Some(ctrl) = reg.control {
                        // Mechanism mode: wiring facts become bridges.
                        let (Some(a), Some(b)) = (
                            resolve_pin(target, ctx.instances, ctx.type_pins),
                            resolve_pin(value, ctx.instances, ctx.type_pins),
                        ) else {
                            self.errors.push(format!(
                                "wiring under a mechanism (node '{}') must be a pin-to-pin bridge",
                                self.node
                            ));
                            continue;
                        };
                        self.bridges.push(BridgeRequest {
                            node: self.node.to_string(),
                            a,
                            b,
                            control: ctrl.clone(),
                            via: reg.via.map(|s| s.to_string()),
                        });
                        continue;
                    }
                    if let Some(desc) = reg.cond {
                        self.errors.push(format!(
                            "wiring inside `when {desc}` (node '{}') is conditional — copper cannot be. \
                             State the condition in the node guard (making the wiring unconditional in \
                             that region), or make it a single pin comparison and declare a switching \
                             part (`when ... via Type;`, mechanism synthesis)",
                            self.node
                        ));
                        continue;
                    }
                    if let (Expr::Identifier(name), Expr::Bool(true)) = (target, value) {
                        self.intents.push((self.node.to_string(), name.clone()));
                        continue;
                    }
                    if let (Some(lp), Some(rp)) = (
                        resolve_pin(target, ctx.instances, ctx.type_pins),
                        resolve_pin(value, ctx.instances, ctx.type_pins),
                    ) {
                        let lk = pin_key(&lp.component, &lp.pin);
                        let rk = pin_key(&rp.component, &rp.pin);
                        ctx.ds.make(lk.clone());
                        ctx.ds.make(rk.clone());
                        ctx.ds.union(&lk, &rk);
                        self.proofs.push(format!(
                            "wired in node '{}': {}.{} <-> {}.{}",
                            self.node, lp.component, lp.pin, rp.component, rp.pin
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The child condition description for a guarded region (D16): None
/// when the region is mechanizable or a plain carve; a compound
/// description when signal-level (D7 propagates through nested carves).
fn child_cond_str(
    control: &Option<PinRef>,
    reg_cond: Option<&str>,
    inner_mentions_pin: bool,
    c: &Expr,
) -> Option<String> {
    if control.is_some() {
        return None;
    }
    if reg_cond.is_none() && !inner_mentions_pin {
        return None;
    }
    Some(match reg_cond {
        Some(outer) => format!("{outer} && {c}"),
        None => format!("{c}"),
    })
}

/// The Guarded arm of the walk (E14a/D16): classify the condition —
/// mechanizable (single pin comparison → bridges), signal-level but
/// not mechanizable (D7 error), or region-level carve. The signal
/// description propagates through nested region carves.
fn walk_guarded(
    c: &Expr,
    inner: &[Statement],
    reg: RegionCtx,
    ctx: &mut NetlistContext,
    sink: &mut FactSink,
) {
        let (via, inner_rest) = take_via_strategy(inner);
        let control = condition_control(c, ctx);
        let child_control = if control.is_some() {
            control.clone()
        } else if reg.control.is_some() {
            reg.control.cloned()
        } else {
            None
        };
        let child_cond = child_cond_str(
            &child_control,
            reg.cond,
            mentions_pin(c, ctx),
            c,
        );
        sink.walk(
            &inner_rest,
            RegionCtx {
                cond: child_cond.as_deref(),
                control: child_control.as_ref(),
                via: via.as_deref().or(reg.via),
            },
            ctx,
        );
    }

/// Body facts of one reactive transaction (E14a/D16).
fn body_facts(
    t: &crate::ast::top::Transaction,
    ctx: &mut NetlistContext,
    sink: &mut FactSink,
) {
    sink.walk(&t.body, RegionCtx { cond: None, control: None, via: None }, ctx);
}

/// Eligibility of one declared instance as a bridge mechanism (D16): a
/// type with exactly one Control pin and at least two Switchable pins,
/// whose control pin is unconnected or already on the condition's net
/// (pre-wired disambiguation).
fn is_mechanism(
    ctx: &mut NetlistContext,
    inst: &ComponentInstance,
    ti: &TypeInfo,
    ctrl_root: &String,
) -> bool {
    let controls: Vec<&(String, u64)> = ti
        .pins
        .iter()
        .enumerate()
        .filter(|(i, _)| ti.pin_classes[*i].control)
        .map(|(_, p)| p)
        .collect();
    let switchables = ti
        .pins
        .iter()
        .enumerate()
        .filter(|(i, _)| ti.pin_classes[*i].switchable)
        .count();
    if controls.len() != 1 || switchables < 2 {
        return false;
    }
    let ckey = pin_key(&inst.name, &controls[0].0);
    !(ctx.ds.contains(&ckey) && ctx.ds.find(&ckey) != *ctrl_root)
}

/// Candidate mechanisms for a bridge request (D16 phase 2): declared
/// instances whose type has exactly one Control-class pin and at least
/// two Switchable-class pins, whose control pin is unconnected or
/// already on the condition's net (pre-wired disambiguation), narrowed
/// by the `via` strategy when given (type name).
fn mechanism_candidates(
    ctx: &mut NetlistContext,
    via: &Option<String>,
    control: &PinRef,
) -> Vec<String> {
    let ctrl_root = ctx.ds.find(&pin_key(&control.component, &control.pin));
    let mut out: Vec<String> = Vec::new();
    for inst in ctx.instances.values() {
        let Some(ti) = ctx.type_info.get(&inst.type_name) else {
            continue;
        };
        if let Some(t) = via {
            if inst.type_name != *t {
                continue;
            }
        }
        if is_mechanism(ctx, inst, ti, &ctrl_root) {
            out.push(inst.name.clone());
        }
    }
    out
}

/// Wire one mechanism (D16 phase 2): control <- condition net, the two
/// path pins <- the bridged pins. Records the conditional bridge with
/// provenance; the schematic emits the switch as ordinary copper.
fn synthesize_one(
    ctx: &mut NetlistContext,
    mech_name: &str,
    br: &BridgeRequest,
    proofs: &mut Vec<String>,
) {
    let Some(mech) = ctx.instances.get(mech_name) else {
        return;
    };
    let ti = &ctx.type_info[&mech.type_name];
    let cpin = ti
        .pins
        .iter()
        .enumerate()
        .find(|(i, _)| ti.pin_classes[*i].control)
        .map(|(_, p)| p)
        .expect("candidate guarantees one control pin");
    let mut paths: Vec<&(String, u64)> = ti
        .pins
        .iter()
        .enumerate()
        .filter(|(i, _)| ti.pin_classes[*i].switchable)
        .map(|(_, p)| p)
        .collect();
    paths.sort_by_key(|(_, n)| *n);
    let union_all = |ctx: &mut NetlistContext, names: Vec<(String, String)>| {
        let keys: Vec<String> = names
            .iter()
            .map(|(c, p)| pin_key(c, p))
            .collect();
        for k in &keys {
            ctx.ds.make(k.clone());
        }
        for k in keys.windows(2) {
            ctx.ds.union(&k[0], &k[1]);
        }
    };
    let mut members = vec![
        (mech.name.clone(), cpin.0.clone()),
        (br.control.component.clone(), br.control.pin.clone()),
    ];
    members.push((mech.name.clone(), paths[0].0.clone()));
    members.push((br.a.component.clone(), br.a.pin.clone()));
    members.push((mech.name.clone(), paths[1].0.clone()));
    members.push((br.b.component.clone(), br.b.pin.clone()));
    union_all(ctx, members);
    proofs.push(format!(
        "mechanism '{}' bridges {}.{} <-> {}.{} under {}.{} (node '{}')",
        mech.name, br.a.component, br.a.pin, br.b.component, br.b.pin,
        br.control.component, br.control.pin, br.node
    ));
}

/// Resolve every bridge request (D16 phase 2): exactly one candidate
/// mechanism after narrowing — synthesize; none, or several — hard
/// error naming the candidates and the strategy form (D13, never a
/// silent pick).
fn synthesize_bridges(
    ctx: &mut NetlistContext,
    bridges: &[BridgeRequest],
    proofs: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut synthesized = Vec::new();
    for br in bridges {
        let a_key = pin_key(&br.a.component, &br.a.pin);
        let b_key = pin_key(&br.b.component, &br.b.pin);
        let a_root = ctx.ds.find(&a_key);
        let b_root = ctx.ds.find(&b_key);
        if a_root == b_root {
            errors.push(format!(
                "bridge {}.{} <-> {}.{} under {}.{} (node '{}') is redundant: the pins are already unconditionally connected — the switch can never open them. Remove the unconditional wiring or the mechanism",
                br.a.component, br.a.pin, br.b.component, br.b.pin,
                br.control.component, br.control.pin, br.node
            ));
            continue;
        }
        let mut candidates = mechanism_candidates(ctx, &br.via, &br.control);
        candidates.sort();
        match candidates.len() {
            0 => {
                let via_hint = match &br.via {
                    Some(t) => format!(
                        "strategy names '{}' but no qualifying mechanism of that type is declared — declare one with exactly one Control pin and at least two Path pins",
                        t
                    ),
                    None => "no qualifying mechanism is declared — declare one with exactly one Control pin and at least two Path pins, or narrow with `when ... via Type;`"
                        .to_string(),
                };
                errors.push(format!(
                    "bridge {}.{} <-> {}.{} under {}.{} (node '{}') cannot be synthesized: {}",
                    br.a.component, br.a.pin, br.b.component, br.b.pin,
                    br.control.component, br.control.pin, br.node, via_hint
                ));
            }
            1 => {
                synthesize_one(ctx, &candidates[0], br, proofs);
                let via = br.via.as_deref().unwrap_or("-");
                synthesized.push(format!(
                    "{}.{} <-> {}.{} under {}.{} via {} (node '{}')",
                    br.a.component, br.a.pin, br.b.component, br.b.pin,
                    br.control.component, br.control.pin, via, br.node
                ));
            }
            n => {
                errors.push(format!(
                    "bridge {}.{} <-> {}.{} under {}.{} (node '{}') is ambiguous: {} mechanisms could synthesize it ({}). Disambiguate with the strategy clause: `when ... via <Type>;` or pre-wire one mechanism's control pin",
                    br.a.component, br.a.pin, br.b.component, br.b.pin,
                    br.control.component, br.control.pin, br.node, n, candidates.join(", ")
                ));
            }
        }
    }
    synthesized
}

/// Complete one drive intent (E14a): the instance must have exactly one
/// open pin, and exactly one unconnected drive-capable pin (spec
/// CanDrive) may exist elsewhere. One of each — the intent wires them.
/// Anything else is an Err naming the facts (candidates included, sorted
/// — never a silent choice, D13).
fn complete_intent(
    inst_name: &str,
    node: &str,
    ctx: &mut NetlistContext,
) -> Result<String, String> {
    let Some(inst) = ctx.instances.get(inst_name) else {
        return Err(format!(
            "drive intent '{} = true' (node '{}') names no declared instance",
            inst_name, node
        ));
    };
    let Some(ti) = ctx.type_info.get(&inst.type_name) else {
        return Ok(format!(
            "intent '{} = true' (node '{}'): typeless — recorded",
            inst_name, node
        ));
    };
    let Some(pins) = ctx.type_pins.get(&inst.type_name) else {
        return Ok(format!(
            "intent '{} = true' (node '{}'): typeless — recorded",
            inst_name, node
        ));
    };
    let open: Vec<&(String, u64)> = pins
        .iter()
        .filter(|(n, _)| !pin_connected(ctx.ds, &pin_key(inst_name, n)))
        .collect();
    if open.is_empty() {
        return Ok(format!(
            "intent '{} = true' (node '{}'): fully wired — recorded",
            inst_name, node
        ));
    }
    if open.len() > 1 {
        let names: Vec<String> =
            open.iter().map(|(n, _)| format!("{}.{}", inst_name, n)).collect();
        return Err(format!(
            "drive intent '{} = true' (node '{}') cannot complete: {} pins of '{}' are unconnected ({}). Wire all but one — the intent completes the last open pin",
            inst_name,
            node,
            open.len(),
            inst_name,
            names.join(", ")
        ));
    }
    let mut candidates: Vec<String> = Vec::new();
    for (other, oi) in ctx.instances {
        if other == inst_name {
            continue;
        }
        let Some(oti) = ctx.type_info.get(&oi.type_name) else {
            continue;
        };
        for (i, (n, _)) in oti.pins.iter().enumerate() {
            let key = pin_key(other, n);
            if oti.pin_classes[i].can_drive && !pin_connected(ctx.ds, &key) {
                candidates.push(format!("{}.{}", other, n));
            }
        }
    }
    candidates.sort();
    match candidates.len() {
        0 => Err(format!(
            "drive intent '{} = true' (node '{}') has no completion: no unconnected drive-capable pin (spec CanDrive) remains in scope",
            inst_name, node
        )),
        1 => {
            let mut parts = candidates[0].splitn(2, '.');
            let (co, cp) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let open_pin = &open[0].0;
            let lk = pin_key(inst_name, open_pin);
            let rk = pin_key(co, cp);
            ctx.ds.make(lk.clone());
            ctx.ds.make(rk.clone());
            ctx.ds.union(&lk, &rk);
            Ok(format!(
                "wired by intent '{} = true' (node '{}'): {}.{} <-> {} — single drive-capable candidate",
                inst_name, node, inst_name, open_pin, candidates[0]
            ))
        }
        n => Err(format!(
            "drive intent '{} = true' (node '{}') is ambiguous: {} drive-capable pins could complete '{}.{}' ({}). State the connection explicitly in the node guard",
            inst_name, node, n, inst_name, open[0].0, candidates.join(", ")
        )),
    }
}

/// 2026-09-21 (E14a slice 1, design record D2/D3): node-body intents.
/// Two body fact forms on the electronics surface:
/// - `a = b;` where both sides resolve to pins — a wiring fact (union).
/// - `inst = true;` — a DRIVE INTENT: the instance participates; its
///   wiring must complete. Exactly one open pin plus exactly one
///   unconnected drive-capable pin completes; anything else is a hard
///   intent_error — never a silent choice (D13).
fn collect_intents(
    ctx: &mut NetlistContext,
    items: &[TopLevel],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut proofs = Vec::new();
    let mut intents: Vec<(String, String)> = Vec::new();
    let mut bridges: Vec<BridgeRequest> = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else {
            continue;
        };
        let mut sink = FactSink {
            node: t.name.as_str(),
            proofs: &mut proofs,
            errors: &mut errors,
            intents: &mut intents,
            bridges: &mut bridges,
        };
        body_facts(t, ctx, &mut sink);
    }
    let synthesized = synthesize_bridges(ctx, &bridges, &mut proofs, &mut errors);
    for (node, inst_name) in &intents {
        match complete_intent(inst_name, node, ctx) {
            Ok(proof) => proofs.push(proof),
            Err(e) => errors.push(e),
        }
    }
    (errors, proofs, synthesized)
}

/// Every declared pin of every instance, ready for union-find grouping
/// (E13 extraction: keeps derive_netlist a coordinator, not a worker).
fn collect_all_pins(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<PinRef> {
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
    all_pins
}

/// The conditional bridges as display strings (D16 phase 2) — the
/// record the phase-3 complement check and per-region physics consume.
fn conditional_bridge_strings(bridges: &[BridgeRequest]) -> Vec<String> {
    bridges
        .iter()
        .map(|br| {
            let via = br.via.as_deref().unwrap_or("-");
            format!(
                "{}.{} <-> {}.{} under {}.{} via {} (node '{}')",
                br.a.component, br.a.pin, br.b.component, br.b.pin,
                br.control.component, br.control.pin, via, br.node
            )
        })
        .collect()
}

/// Group the union-find roots into named nets and report unconnected
/// non-nc pins as dangling — the netlist B-half, extracted so
/// derive_netlist stays a coordinator (E7 refactor).
fn partition_nets(
    groups: &BTreeMap<String, Vec<PinRef>>,
    net_names: &mut BTreeMap<String, String>,
    nc_pins: &std::collections::HashSet<String>,
) -> (Vec<Net>, Vec<String>) {
    let mut nets = Vec::new();
    let mut dangling = Vec::new();
    let mut net_index = 0;
    for (root, members) in groups {
        if members.len() >= 2 {
            net_index += 1;
            let name = net_names.remove(root).unwrap_or_else(|| format!("N{}", net_index));
            nets.push(Net { name, pins: members.clone() });
        } else {
            let p = &members[0];
            if nc_pins.contains(&pin_key(&p.component, &p.pin)) {
                continue;
            }
            dangling.push(format!(
                "pin '{}.{}' (pin {}) is on no net — it never appears in a precondition pin equality. \
                 State its connection, e.g. [{}.{}.voltage == other.pin.voltage]",
                p.component, p.pin, p.number, p.component, p.pin
            ));
        }
    }
    (nets, dangling)
}

pub fn derive_netlist(items: &[TopLevel]) -> ElectronicsNetlist {
    let (type_pins, type_info, class_errors) = collect_type_pins(items);
    if type_pins.is_empty() {
        return ElectronicsNetlist::default();
    }
    let instance_list = collect_instances(items, &type_pins);
    let instances: BTreeMap<String, &ComponentInstance> = instance_list
        .iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    let mut ds = DisjointSet::new();
    let named = collect_pin_unions(items, &instances, &type_pins, &mut ds);
    let (mut net_names, raw_conflicts) = resolve_net_names(&mut ds, named);
    // 2026-09-21 (E14a): node-body intents — body wiring facts union
    // first; drive intents then complete the last open pin.
    let (intent_errors, intent_proofs, conditional_bridges) = {
        let mut ictx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
        collect_intents(&mut ictx, items)
    };

    // Group members by root; sort everything for determinism (HashMap rule).
    let mut groups: BTreeMap<String, Vec<PinRef>> = BTreeMap::new();
    let mut all_pins = collect_all_pins(&instances, &type_pins);
    all_pins.sort();
    // 2026-09-21 (E12): pins ascribed a no_connect class are intentionally
    // unconnected — exempt from the dangling-pin error below.
    let nc_pins = collect_no_connect_pins(&instances, &type_info);
    for p in &all_pins {
        let key = pin_key(&p.component, &p.pin);
        ds.make(key.clone());
        let root = ds.find(&key.clone());
        groups.entry(root).or_default().push(p.clone());
    }

    let (nets, dangling) = partition_nets(&groups, &mut net_names, &nc_pins);

    let net_conflicts = raw_conflicts
        .into_iter()
        .map(|(root, a, b)| {
            let pins = groups
                .get(&root)
                .map(|ms| {
                    ms.iter()
                        .map(|p| format!("{}.{}", p.component, p.pin))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!(
                "the net [{pins}] is named both '{a}' and '{b}' — one electrical node \
                 cannot carry two names. fix: use one name for the net, or split it \
                 into separate nets."
            )
        })
        .collect();

    // Voltage classes need the nets and instances before they move into the
    // result struct.
    let voltage = derive_voltage(items, &nets, &instances, &type_pins, &type_info);

    // 2026-09-21 (E13): the decoupling convention runs on the finished
    // netlist — every union is final when it fires.
    let mut ctx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
    let convention_errors = check_decoupling(&mut ctx);
    // 2026-09-21 (E7): source-pin budgets run last — they read the
    // derived per-net currents the voltage fixpoint just produced.
    let budget_errors =
        check_budgets(items, &instances, &type_info, &nets, &voltage.net_voltage);

    ElectronicsNetlist {
        components: instance_list,
        nets,
        dangling,
        net_conflicts,
        is_electronics: true,
        class_errors,
        convention_errors,
        budget_errors,
        intent_errors,
        intent_proofs,
        conditional_bridges,
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

    // ── 2026-09-21 (E12): pin electrical classes ──────────────────────────

    #[test]
    fn no_connect_pin_is_exempt_from_dangling() {
        let src = r#"
            type Power { spec KicadType: "power_in"; };
            type Nc { spec KicadType: "no_connect"; spec NoConnect: true; };
            type Chip { pin vdd: Power; pin prog: Nc; reference "U"; };
            type Header { pin p1: Power; pin gnd; reference "J"; };

            let u1: Chip = Chip { value: "x" };
            let j1: Header = Header { value: "y" };

            txn on
                [j1.p1.voltage == u1.vdd.voltage && j1.gnd.voltage == u1.vdd.voltage]
                [u1.vdd.current >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(
            nl.dangling.is_empty(),
            "nc pin exempt from dangling: {:?}",
            nl.dangling
        );
        let ti = &nl.type_info["Chip"];
        assert_eq!(ti.pin_classes[0].kicad_type, "power_in");
        assert!(!ti.pin_classes[0].no_connect);
        assert_eq!(ti.pin_classes[1].kicad_type, "no_connect");
        assert!(ti.pin_classes[1].no_connect);
    }

    #[test]
    fn unknown_class_type_is_a_hard_error() {
        // A class ascription to a type that is not in scope refuses —
        // an unresolvable class can never prove anything downstream.
        let src = r#"
            type Widget { pin x: Bogus; reference "W"; };
        "#;
        let nl = analyze(src);
        assert_eq!(nl.class_errors.len(), 1, "{:?}", nl.class_errors);
        assert!(
            nl.class_errors[0].contains("Bogus"),
            "{}",
            nl.class_errors[0]
        );
    }

    #[test]
    fn pin_array_elements_wire_by_index() {
        // 2026-09-21 (E11): indexed element access `u1.gpio[3]` resolves to
        // the parser-expanded element — each equality forms its own
        // per-element net, exactly like the manual per-pin clauses.
        let src = r#"
            type Chip { pin gpio[4]; reference "U"; };
            type Header { pin p[4]; reference "J"; };

            let u1: Chip = Chip { value: "x" };
            let j1: Header = Header { value: "y" };

            txn on
                [j1.p[0].voltage == u1.gpio[0].voltage && j1.p[1].voltage == u1.gpio[1].voltage &&
                 j1.p[2].voltage == u1.gpio[2].voltage && j1.p[3].voltage == u1.gpio[3].voltage]
                [u1.gpio[0].current >= 0.0 && u1.gpio[0].current <= 0.02]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert_eq!(nl.nets.len(), 4, "four indexed equalities → four nets");
        assert!(nl.nets.iter().all(|n| n.pins.len() == 2));
    }

    // ── 2026-09-21 (E13): decoupling convention ───────────────────────────

    const DECOUPLED_BOARD: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: "100n"; };
        type Cap { pin p; pin n; reference "C"; spec Decoupler: true; };

        let u1: Chip = Chip { value: "mcu" };
        let c1: Cap = Cap { value: "100n" };

        txn on
            [u1.vdd.voltage == c1.p.voltage && u1.vss.voltage == c1.n.voltage]
            [u1.vdd.current >= 0.0]
        { }
    "#;

    #[test]
    fn conventional_board_passes_decoupling() {
        let nl = analyze(DECOUPLED_BOARD);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(
            nl.convention_errors.is_empty(),
            "cap across the rail satisfies the convention: {:?}",
            nl.convention_errors
        );
        let ti = &nl.type_info["Chip"];
        assert!(ti.pin_classes[0].supply);
        assert!(ti.pin_classes[1].return_pin);
    }

    #[test]
    fn missing_decoupling_is_a_convention_error() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: "100n"; };

            let u1: Chip = Chip { value: "mcu" };
        "#;
        let nl = analyze(src);
        assert_eq!(
            nl.convention_errors.len(),
            1,
            "{:?}",
            nl.convention_errors
        );
        assert!(
            nl.convention_errors[0].contains("u1") && nl.convention_errors[0].contains("vdd"),
            "{}",
            nl.convention_errors[0]
        );
    }

    #[test]
    fn non_decoupler_part_does_not_satisfy_convention() {
        // A resistor across the rail is not a decoupling part: the
        // property interface (spec Decoupler) decides, never the type name.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: "100n"; };
            type Res { pin a; pin b; reference "R"; };

            let u1: Chip = Chip { value: "mcu" };
            let r1: Res = Res { value: "10k" };

            txn on
                [u1.vdd.voltage == r1.a.voltage && u1.vss.voltage == r1.b.voltage]
                [u1.vdd.current >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.convention_errors.len(), 1, "{:?}", nl.convention_errors);
    }

    #[test]
    fn decouple_without_supply_pin_is_an_error() {
        let src = r#"
            type Chip { pin a; pin b; reference "U"; spec Decouple: "100n"; };

            let u1: Chip = Chip { value: "mcu" };
        "#;
        let nl = analyze(src);
        assert_eq!(nl.convention_errors.len(), 1, "{:?}", nl.convention_errors);
        assert!(
            nl.convention_errors[0].contains("supply-class"),
            "{}",
            nl.convention_errors[0]
        );
    }

    // ── 2026-09-21 (E7): source-pin budgets ───────────────────────────────

    const BUDGET_BOARD: &str = r#"
        type Resistor { pin a; pin b; reference "R"; };
        type Led { pin a; pin k; reference "D"; tolerance 3.6; };
        type Connector { pin vcc; pin gnd; reference "J"; };

        let j1: Connector = Connector { value: "JST-2" };
        let r1: Resistor = Resistor { value: "330" };
        let d1: Led = Led { value: "red" };

        budget j1.vcc <= 0.05;

        txn on
            [j1.vcc.voltage == 3.3V && j1.vcc.voltage == r1.a.voltage &&
             r1.b.voltage == d1.a.voltage && d1.k.voltage == j1.gnd.voltage]
            [d1.a.current > 0.0 && d1.a.current <= 0.02]
        { }
    "#;

    #[test]
    fn budget_under_derived_draw_passes() {
        // 3.3 V / 330 R = 10 mA derived across the source net; 50 mA caps it.
        let nl = analyze(BUDGET_BOARD);
        assert!(nl.budget_errors.is_empty(), "{:?}", nl.budget_errors);
    }

    #[test]
    fn budget_exceeded_is_an_error() {
        let src = BUDGET_BOARD.replace("budget j1.vcc <= 0.05;", "budget j1.vcc <= 0.005;");
        let nl = analyze(&src);
        assert_eq!(nl.budget_errors.len(), 1, "{:?}", nl.budget_errors);
        assert!(
            nl.budget_errors[0].contains("j1.vcc") && nl.budget_errors[0].contains("exceeded"),
            "{}",
            nl.budget_errors[0]
        );
    }

    #[test]
    fn budget_reversed_form_and_unit_literals() {
        // `250mA >= pin` states the same budget; unit literals are A-normalized.
        let src = BUDGET_BOARD.replace("budget j1.vcc <= 0.05;", "budget 50mA >= j1.vcc;");
        let nl = analyze(&src);
        assert!(nl.budget_errors.is_empty(), "{:?}", nl.budget_errors);
    }

    #[test]
    fn budget_on_unknown_pin_is_an_error() {
        let src = BUDGET_BOARD.replace("budget j1.vcc <= 0.05;", "budget j1.nope <= 0.05;");
        let nl = analyze(&src);
        assert_eq!(nl.budget_errors.len(), 1, "{:?}", nl.budget_errors);
        assert!(
            nl.budget_errors[0].contains("targets a pin"),
            "{}",
            nl.budget_errors[0]
        );
    }

    // ── 2026-09-21 (E14a slice 1): node-body intents ──────────────────────

    #[test]
    fn drive_intent_completes_single_candidate() {
        // d1.a is the one open pin; u1.gpio0 is the one unconnected
        // drive-capable pin — the intent wires them and records proof.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin gpio0: Io; pin vdd: Power; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin p1: Power; pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.p1.voltage == 3.3V && j1.p1.voltage == u1.vdd.voltage &&
                 j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                d1 = true;
            }
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("wired by intent") && p.contains("d1.a")),
            "proof recorded: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn ambiguous_drive_intent_enumerates_candidates() {
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin gpio0: Io; pin gpio1: Io; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                d1 = true;
            }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(e.contains("ambiguous") && e.contains("u1.gpio0") && e.contains("u1.gpio1"), "{}", e);
        assert!(e.contains("node 'on'"), "{}", e);
    }

    #[test]
    fn drive_intent_without_candidates_is_an_error() {
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Mcu { pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                d1 = true;
            }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("no unconnected drive-capable"),
            "{}",
            nl.intent_errors[0]
        );
    }

    #[test]
    fn body_wiring_facts_union_pins() {
        // `d1.a = u1.gpio0;` in a node body is a wiring fact — the net
        // forms even without any precondition equality stating it.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin gpio0: Io; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                d1.a = u1.gpio0;
            }
        "#;
        let nl = analyze(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert!(
            nl.nets.iter().any(|n| {
                let ps: Vec<&str> = n.pins.iter().map(|p| p.pin.as_str()).collect();
                ps.contains(&"a") && ps.contains(&"gpio0")
            }),
            "body wiring forms the net: {:?}",
            nl.nets
        );
        assert!(nl.intent_proofs.iter().any(|p| p.contains("wired in node 'on'")));
    }

    #[test]
    fn region_level_when_applies_wiring() {
        // `when true { ... }` — a pin-free condition carves a region whose
        // facts are ordinary wiring (D16 phase 1).
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin gpio0: Io; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                when true {
                    d1.a = u1.gpio0;
                };
            }
        "#;
        let nl = analyze(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert!(
            nl.nets.iter().any(|n| {
                let ps: Vec<&str> = n.pins.iter().map(|p| p.pin.as_str()).collect();
                ps.contains(&"a") && ps.contains(&"gpio0")
            }),
            "region fact wired: {:?}",
            nl.nets
        );
    }

    #[test]
    fn signal_level_when_demands_mechanism() {
        // A pin-referencing condition makes the wiring conditional —
        // copper cannot be (D7/D16): hard error naming the condition.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Mcu { pin gpio0: Io; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin p1: Power; pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                when j1.p1.voltage > 3.0V && j1.p1.voltage < 3.6V {
                    d1.a = u1.gpio0;
                };
            }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(e.contains("copper cannot be"), "{}", e);
        assert!(e.contains("&&"), "{}", e);
    }

    #[test]
    fn nested_whens_compound_conditions() {
        // `when a { when b { f } }` — the error names the conjunction.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Mcu { pin gpio0: Io; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin p1: Power; pin gnd: Ground; reference "J"; };

            let u1: Mcu = Mcu { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            node on
                [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
                [true]
            {
                when j1.p1.voltage > 3.0V && j1.p1.voltage < 3.6V {
                    when true {
                        d1.a = u1.gpio0;
                    };
                };
            }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(
            e.contains("&&") && e.contains("true"),
            "compound condition named: {}",
            e
        );
    }

    // ── 2026-09-21 (D16 phase 2): mechanism synthesis ─────────────────────

    const MECH_BOARD: &str = r#"
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Control { spec KicadType: "input"; spec Control: true; };
        type Path { spec KicadType: "passive"; spec Switchable: true; };
        type Fet { pin gate: Control; pin d: Path; pin s: Path; reference "Q"; };
        type Chip { pin gpio0; pin gnd: Ground; reference "U"; };
        type Led { pin a; pin k; reference "D"; };
        type Conn { pin gnd: Ground; reference "J"; };

        let q1: Fet = Fet { value: "bs170" };
        let u1: Chip = Chip { value: "x" };
        let d1: Led = Led { value: "red" };
        let j1: Conn = Conn { value: "y" };

        node on
            [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage]
            [true]
        {
            when u1.gpio0.voltage == 3.3V {
                d1.a = u1.gnd;
            };
        }
    "#;

    #[test]
    fn mechanism_synthesis_wires_bridge() {
        // A single switch: the when-clause synthesizes control <- gpio0,
        // path1 <- d1.a, path2 <- u1.gnd, with provenance.
        let nl = analyze(MECH_BOARD);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert_eq!(nl.conditional_bridges.len(), 1, "{:?}", nl.conditional_bridges);
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("mechanism 'q1'") && p.contains("d1.a")),
            "provenance: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn ambiguous_mechanism_requests_strategy() {
        let src = MECH_BOARD.replace(
            "let q1: Fet = Fet { value: \"bs170\" };",
            "let q1: Fet = Fet { value: \"bs170\" };
        let q2: Fet = Fet { value: \"bs170\" };",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(e.contains("ambiguous") && e.contains("q1") && e.contains("q2"), "{}", e);
        assert!(e.contains("via <Type>"), "{}", e);
    }

    #[test]
    fn via_strategy_narrows_to_type() {
        // Two switches, different types: `via Fet` selects q1.
        let src = MECH_BOARD.replace(
            "let q1: Fet = Fet { value: \"bs170\" };",
            "let q1: Fet = Fet { value: \"bs170\" };
        type Relay { pin coil: Control; pin c1: Path; pin c2: Path; reference \"K\"; };
        let k1: Relay = Relay { value: \"g5le\" };",
        );
        let src = src.replace(
            "            };\n        }",
            "            } via Fet;\n        }",
        );
        let nl = analyze(&src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("mechanism 'q1'")),
            "{:?}",
            nl.intent_proofs
        );
        assert!(
            !nl.intent_proofs.iter().any(|p| p.contains("k1")),
            "via Fet must not pick the relay: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn via_with_no_declared_instance_is_an_error() {
        let src = MECH_BOARD.replace(
            "            };\n        }",
            "            } via Relay;\n        }",
        );
        let src = src.replace(
            "let q1: Fet = Fet { value: \"bs170\" };",
            "type Relay { pin coil: Control; pin c1: Path; pin c2: Path; reference \"K\"; };
        let q1: Fet = Fet { value: \"bs170\" };",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("no qualifying mechanism") && nl.intent_errors[0].contains("Relay"),
            "{}",
            nl.intent_errors[0]
        );
    }

    #[test]
    fn mechanism_redundant_when_unconditionally_wired() {
        // d1.a = u1.gnd also stated unconditionally in the body — the
        // mechanism bridge is redundant; the switch can never open them.
        let src = MECH_BOARD.replace(
            "            };\n        }",
            "            };\n            d1.a = u1.gnd;\n        }",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(
            e.contains("redundant") && e.contains("d1.a") && e.contains("u1.gnd"),
            "{}",
            e
        );
        assert!(
            nl.conditional_bridges.is_empty(),
            "redundant bridge must not be synthesized: {:?}",
            nl.conditional_bridges
        );
    }

    #[test]
    fn intent_on_unknown_instance_is_an_error() {
        let src = r#"
            type Led { pin a; pin k; reference "D"; };
            node on [true] [true] { ghost = true; }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("no declared instance"),
            "{}",
            nl.intent_errors[0]
        );
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
            type Power { pin vout; pin gnd; reference "P"; tolerance any; rating any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; rating 0.25; };
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
        assert!(nl.voltage.violations.is_empty(), "rated divider board is clean: {:?}", nl.voltage.violations);
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
            type Power { pin vout; pin gnd; reference "P"; tolerance any; rating any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; rating 0.25; };
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
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
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
    fn keyword_net_names_are_valid() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [net out: j1.p1.voltage == r1.a.voltage && net gnd: j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.nets.iter().any(|n| n.name == "out"), "keyword 'out' should be a valid net name");
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

    #[test]
    fn conflicting_net_names_are_a_violation() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [net vcc: j1.p1.voltage == r1.a.voltage && net gnd: j1.p2.voltage == r1.b.voltage && net vcc: r1.b.voltage == j1.p2.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.net_conflicts.len(), 1, "vcc+gnd on one node is a conflict: {:?}", nl.net_conflicts);
        assert!(nl.net_conflicts[0].contains("'vcc'"), "conflict names both: {}", nl.net_conflicts[0]);
        assert!(nl.net_conflicts[0].contains("'gnd'"), "conflict names both: {}", nl.net_conflicts[0]);
        assert!(nl.net_conflicts[0].contains("j1.p2"), "conflict names the pins: {}", nl.net_conflicts[0]);
    }

    #[test]
    fn repeated_same_net_name_is_fine() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [net vcc: j1.p1.voltage == r1.a.voltage && net vcc: j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.net_conflicts.is_empty(), "same name twice is redundant, not a conflict: {:?}", nl.net_conflicts);
        assert!(nl.nets.iter().any(|n| n.name == "vcc"));
    }

    #[test]
    fn derived_power_above_rating_violates() {
        // 12 V across a single 330-ohm part: 0.44 W — above the 0.25 W
        // 0805-class rating. The compiler refuses by derivation.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; tolerance any; rating any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; rating 0.25; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330" };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == p1.gnd.voltage && p1.vout.voltage == 12.0 && p1.gnd.voltage == 0.0]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.iter().any(|v| v.contains("rated 250.0 mW")), "0.44 W > 0.25 W must violate: {:?}", nl.voltage.violations);
    }

    #[test]
    fn proven_dissipation_without_rating_is_undeclared() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; tolerance any; rating any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330" };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == p1.gnd.voltage && p1.vout.voltage == 5.0 && p1.gnd.voltage == 0.0]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.iter().any(|v| v.contains("declares no power rating")), "proven dissipation forces the decision: {:?}", nl.voltage.violations);
    }

    #[test]
    fn zero_drop_needs_no_rating() {
        // Both pins on one driven net: a strap — ΔV = 0, no proven
        // dissipation, no rating decision forced.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; tolerance any; rating any; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330" };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == r1.a.voltage && p1.vout.voltage == 5.0]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.is_empty(), "strap dissipates nothing provable: {:?}", nl.voltage.violations);
    }

    #[test]
    fn unit_suffix_voltage_drive() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { };
            txn powered
                [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3V]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.voltage.net_voltage.get("N1"), Some(&3.3), "3.3V drive should produce 3.3V on the net");
    }

    #[test]
    fn unit_suffix_current_bound() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; tolerance any; };
            let j1: Connector = Connector { };
            let r1: Resistor = Resistor { value: "330R"; };
            txn powered
                [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage && j1.p1.voltage == 3.3]
                [r1.b.current > 0.0 && r1.b.current <= 20mA]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.voltage.violations.is_empty(), "20mA should be 0.02A, within bounds: {:?}", nl.voltage.violations);
    }
}
