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
    /// 2026-09-22 (Slice B): instances declared `unpop` — absent from the
    /// BOM, pins open (Nc-exempt) in the absent state.
    pub unpop: std::collections::HashSet<String>,
    /// 2026-09-22 (Slice B): instances with an acknowledged intentional
    /// short (`shortcircuit unpop …` — present-state short suppressed).
    pub shortcircuit: std::collections::HashSet<String>,
    /// 2026-09-22 (Slice B): dual-state verification notes — the board was
    /// checked in both the present and absent configurations.
    pub participation_notes: Vec<String>,
    /// 2026-09-22 (Slice B): participation warnings — a `shortcircuit` on a
    /// populated part (a definite short) with the suggest-`unpop` hint; a
    /// participation fact naming an undeclared instance.
    pub participation_warnings: Vec<String>,
    /// 2026-09-22 (ERC contention, D6/D12): a net with two drive-capable
    /// pins that are not open-drain — contention. Hard diagnostics.
    pub contention_errors: Vec<String>,
    /// 2026-09-22 (whole-bus equality, Slice 3): a range-indexed equality
    /// with mismatched or reversed bus lengths — a hard error.
    pub bus_errors: Vec<String>,
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
    /// 2026-09-22 (ERC contention, D6/D12): `spec WiredAnd: true;` — the
    /// class may SHARE a driven net (open-drain = wired-AND by definition,
    /// `IoOd`). A net with two drive-capable pins where at least one is NOT
    /// WiredAnd is contention — a hard error. Read generically; the
    /// compiler knows no class names.
    pub wired_and: bool,
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
            wired_and: false,
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
pub(crate) fn collect_type_metadata(
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
            wired_and: md.get("wired_and").and_then(property_bool).unwrap_or(false),
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
/// `r_pu[0]` / `m[0][1]` — flatten an index chain rooted at an identifier
/// into the expanded element's name (2026-09-23, E1 instance arrays). A
/// bare identifier or a pin-array access (rooted at a Field, `u.gpio[3]`)
/// is not an instance array.
fn instance_array_name(expr: &Expr) -> Option<String> {
    let mut suffix = String::new();
    let mut cur = expr;
    loop {
        match cur {
            Expr::Index(inner, idx) => {
                let Expr::Decimal(d) = idx.as_ref() else {
                    return None;
                };
                suffix = format!("[{}]{}", d, suffix);
                cur = inner;
            }
            Expr::Identifier(name) if !suffix.is_empty() => {
                return Some(format!("{}{}", name, suffix));
            }
            _ => return None,
        }
    }
}

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
    // 2026-09-23 (E14a gate): a BARE indexed pin — `u1.en = u2.gpio[0]`,
    // `j2.sig[0] = u2.swdio` — has shape Index(Field(inst, arr), i) with no
    // outer Field to carry the property strip. Normalize it to the
    // bracketed element name against the same tables.
    if let Expr::Index(inner, idx) = cur {
        let Expr::Field(inst_base, arr_name) = inner.as_ref() else {
            return None;
        };
        let Expr::Identifier(inst_name) = inst_base.as_ref() else {
            return None;
        };
        let Expr::Decimal(d) = idx.as_ref() else {
            return None;
        };
        let pin_name = format!("{}[{}]", arr_name, d);
        return lookup_pin(inst_name, &pin_name, instances, type_pins);
    }
    let Expr::Field(base, pin_name) = cur else { return None };
    // 2026-09-21 (E11): `u2.gpio[3]` — indexed element of a pin array.
    // The parser expanded the array into elements named `gpio[0]…`, so
    // the element resolves by its bracketed name against the same table.
    // 2026-09-23 (E1): `r_pu[0].a` — indexed element of an INSTANCE array;
    // same expansion, names like `r_pu[0]`.
    let (inst_name, pin_name) = match base.as_ref() {
        Expr::Identifier(inst_name) => (inst_name.clone(), pin_name.clone()),
        Expr::Index(inner, idx) => match inner.as_ref() {
            Expr::Field(inner_base, arr_name) => {
                let Expr::Identifier(inst_name) = inner_base.as_ref() else {
                    return None;
                };
                let Expr::Decimal(d) = idx.as_ref() else {
                    return None;
                };
                (inst_name.clone(), format!("{}[{}]", arr_name, d))
            }
            _ => match instance_array_name(base.as_ref()) {
                Some(name) => (name, pin_name.clone()),
                None => return None,
            },
        },
        _ => return None,
    };
    lookup_pin(&inst_name, &pin_name, instances, type_pins)
}

/// Shared pin-table lookup: instance name + expanded pin name → PinRef.
fn lookup_pin(
    inst_name: &str,
    pin_name: &str,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<PinRef> {
    let inst = instances.get(inst_name)?;
    let pins = type_pins.get(&inst.type_name)?;
    let (_, number) = pins.iter().find(|(n, _)| n == pin_name)?;
    Some(PinRef {
        component: inst.name.clone(),
        pin: pin_name.to_string(),
        number: *number,
    })
}

/// Collect Eq operand pairs from an expression tree (walks And/Or chains and
/// parenthesized/grouped nodes; everything else is a leaf for this purpose).
/// Naming (`net <name>:`) is gone (2026-09-22): nets are identified by
/// physics (derived voltage + pin classes), never author labels.
fn collect_eq_triples(expr: &Expr, out: &mut Vec<(Expr, Expr)>) {
    match expr {
        Expr::BinaryOp(op @ (BinaryOpKind::Eq | BinaryOpKind::And | BinaryOpKind::Or), l, r) => {
            if *op == BinaryOpKind::Eq {
                out.push(((**l).clone(), (**r).clone()));
            } else {
                collect_eq_triples(l, out);
                collect_eq_triples(r, out);
            }
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
/// 2026-09-22 (Slice B): inputs to the voltage fixpoint — the derived nets
/// and the acknowledged-short instances. Bundled so `derive_voltage` stays
/// under the parameter gate.
struct VoltageInputs<'a> {
    nets: &'a [Net],
    shortcircuit: &'a std::collections::HashSet<String>,
}

fn derive_voltage(
    items: &[TopLevel],
    inputs: &VoltageInputs<'_>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
) -> VoltageCheck {
    let pin_to_net = pin_net_index(inputs.nets);
    let drives = collect_drives(items, &pin_to_net, instances, type_pins);
    // 2026-09-22 (Slice C): conditional drives from the static when laws —
    // guard-aware, joined with the unconditional contract drives.
    let when_drives = collect_when_drives(items, &pin_to_net, instances, type_pins);
    let mut check = classify_drives(drives, when_drives, inputs.nets, inputs.shortcircuit);
    check_tolerance(inputs.nets, instances, type_info, &check.net_voltage, &mut check.violations);
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
            for (l, r) in triples {
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

/// A conditional voltage drive from a static `when` law (Slice C): when the
/// guard holds, the net is driven at the value. The law says `G ⟹ (net at
/// volts)`; the compiler must make it so — propagate the consequence and
/// error if anything contradicts it under a jointly-satisfiable guard.
struct ConditionalDrive {
    net: String,
    volts: f64,
    guard: Expr,
    source: String,
}

/// 2026-09-22 (Slice C): collect conditional drives from the static when
/// laws — top-level (`TopLevel::WhenLaw`) and type-body (`TypeDefBody.
/// when_laws`, instantiated per instance so `out` becomes `inst.out`).
/// A law fact that is a pin-voltage assignment (`out.voltage = 3.3V`) or a
/// pin-voltage equality (`out.voltage == 3.3V`) becomes a conditional drive.
fn collect_when_drives(
    items: &[TopLevel],
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<ConditionalDrive> {
    let ctx = DriveCtx { pin_to_net, instances, type_pins };
    let mut out = Vec::new();
    // Top-level laws reference instance pins directly.
    for item in items {
        let TopLevel::WhenLaw(w) = item else { continue };
        collect_when_fact_drives(&w.guard, &w.facts, &ctx, "top-level", &mut out);
    }
    // Type-body laws are inherited per instance: qualify bare pin names with
    // the instance so `out.voltage` reads `inst.out.voltage`.
    for item in items {
        let TopLevel::TypeDef(td) = item else { continue };
        if td.body.when_laws.is_empty() {
            continue;
        }
        for (inst_name, inst) in instances {
            if inst.type_name != td.name {
                continue;
            }
            for law in &td.body.when_laws {
                let guard = qualify_pin_refs(&law.guard, inst_name, &td.body.pins);
                let facts = law
                    .facts
                    .iter()
                    .map(|s| qualify_pin_refs_stmt(s, inst_name, &td.body.pins))
                    .collect::<Vec<_>>();
                collect_when_fact_drives(
                    &guard, &facts, &ctx,
                    &format!("type '{}' instance '{}'", td.name, inst_name), &mut out,
                );
            }
        }
    }
    out
}

/// Extract conditional drives from one law's facts. A fact is a pin-voltage
/// assignment or equality; the law's guard gates it.
/// Lookup tables for drive extraction (pin→net, instance, pin table).
/// Bundled so the when-law drive collectors stay under the parameter gate.
struct DriveCtx<'a> {
    pin_to_net: &'a BTreeMap<(String, String), String>,
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
}

fn collect_when_fact_drives(
    guard: &Expr,
    facts: &[Statement],
    ctx: &DriveCtx<'_>,
    source: &str,
    out: &mut Vec<ConditionalDrive>,
) {
    for fact in facts {
        let pair: Option<(&Expr, &Expr)> = match fact {
            Statement::Assign(l, r) => Some((l, r)),
            Statement::Expression(e) => match e {
                Expr::BinaryOp(crate::ast::BinaryOpKind::Eq, l, r) => Some((l, r)),
                _ => None,
            },
            _ => None,
        };
        let Some((l, r)) = pair else { continue };
        let Some((pin, v)) = voltage_drive(l, r, ctx.instances, ctx.type_pins) else { continue };
        let Some(net) = ctx.pin_to_net.get(&(pin.component.clone(), pin.pin.clone())) else { continue };
        out.push(ConditionalDrive {
            net: net.clone(),
            volts: v,
            guard: guard.clone(),
            source: format!("when-law ({}) '{}.{}'", source, pin.component, pin.pin),
        });
    }
}

/// Rewrite bare pin identifiers (`out`) in an expression to instance-qualified
/// form (`inst.out`) for a type-body law inherited per instance.
fn qualify_pin_refs(
    expr: &Expr,
    instance: &str,
    pins: &[crate::ast::top::PinDecl],
) -> Expr {
    let is_pin = |name: &str| pins.iter().any(|p| p.name == name);
    match expr {
        Expr::Identifier(name) if is_pin(name) => {
            Expr::Field(Box::new(Expr::Identifier(instance.to_string())), name.clone())
        }
        Expr::Field(base, name) => Expr::Field(
            Box::new(qualify_pin_refs(base, instance, pins)),
            name.clone(),
        ),
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(qualify_pin_refs(l, instance, pins)),
            Box::new(qualify_pin_refs(r, instance, pins)),
        ),
        _ => expr.clone(),
    }
}

/// Rewrite pin refs inside a statement (the law facts).
fn qualify_pin_refs_stmt(
    stmt: &Statement,
    instance: &str,
    pins: &[crate::ast::top::PinDecl],
) -> Statement {
    match stmt {
        Statement::Assign(l, r) => Statement::Assign(
            qualify_pin_refs(l, instance, pins),
            qualify_pin_refs(r, instance, pins),
        ),
        Statement::Expression(e) => {
            Statement::Expression(qualify_pin_refs(e, instance, pins))
        }
        _ => stmt.clone(),
    }
}

/// Group drives by net: the class is the max — disagreeing drives are a
/// shorted supply, hard error. 2026-09-22 (Slice B): a net whose pins touch
/// a `shortcircuit`-acknowledged instance is EXEMPT — the author stated the
/// intentional short. 2026-09-22 (Slice C): conditional drives from when
/// laws join the same net — two in-force drives at different voltages are a
/// contradiction UNLESS the guards are mutually exclusive.
fn classify_drives(
    drives: Vec<(String, f64, String)>,
    conditional: Vec<ConditionalDrive>,
    nets: &[Net],
    shortcircuit: &std::collections::HashSet<String>,
) -> VoltageCheck {
    let mut check = VoltageCheck::default();
    // (volts, source). Conditional drives carry their guard for the
    // joint-satisfiability check; unconditional drives (guard None) always
    // apply.
    let mut by_net: BTreeMap<String, Vec<(f64, Option<Expr>, String)>> = BTreeMap::new();
    for (net, v, src) in drives {
        by_net.entry(net).or_default().push((v, None, src));
    }
    for cd in conditional {
        by_net.entry(cd.net.clone()).or_default().push((cd.volts, Some(cd.guard), cd.source));
    }
    for (net_name, ds) in by_net {
        let (in_force, contradictions) = drive_conflicts(&ds);
        if !contradictions.is_empty() {
            // 2026-09-22 (Slice B): an acknowledged short suppresses the
            // shorted-supply error — the author stated the intentional short.
            let acknowledged = nets
                .iter()
                .filter(|n| n.name == net_name)
                .flat_map(|n| n.pins.iter())
                .any(|p| shortcircuit.contains(&p.component));
            if !acknowledged {
                let min = in_force.iter().cloned().fold(f64::INFINITY, f64::min);
                let max = in_force.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                check.violations.push(format!(
                    "net '{}' is driven at two different voltages ({} and {}) — that is a shorted supply. \
                     why: {} both drive it. fix: drive the net at one voltage, or separate the levels \
                     with a regulator or switch component, or make the conflicting guards mutually exclusive.",
                    net_name, format_volts(min), format_volts(max), contradictions.join("; ")
                ));
            }
        }
        // The net class is the max in-force drive (agreeing drives collapse).
        check.net_voltage.insert(
            net_name.clone(),
            in_force.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        );
    }
    check
}

/// Classify one net's drives: which are in force, and which pairs
/// contradict (both could hold simultaneously at different voltages).
/// Extracted so `classify_drives` stays under the cognitive-complexity gate.
fn drive_conflicts(
    ds: &[(f64, Option<Expr>, String)],
) -> (Vec<f64>, Vec<String>) {
    let mut in_force: Vec<f64> = Vec::new();
    let mut contradictions: Vec<String> = Vec::new();
    for i in 0..ds.len() {
        let (vi, guard_i, si) = &ds[i];
        // A drive is in force if unconditional or its guard is satisfiable.
        let applies = match guard_i {
            Some(g) => crate::proof_engine::check_satisfiable(g, &Expr::Bool(true)),
            None => true,
        };
        if applies {
            in_force.push(*vi);
        }
        for j in (i + 1)..ds.len() {
            let (vj, guard_j, sj) = &ds[j];
            if (vi - vj).abs() <= f64::EPSILON {
                continue;
            }
            let sat = match (guard_i, guard_j) {
                (None, None) => true,
                (None, Some(gj)) => {
                    crate::proof_engine::check_satisfiable(gj, &Expr::Bool(true))
                }
                (Some(gi), None) => {
                    crate::proof_engine::check_satisfiable(gi, &Expr::Bool(true))
                }
                (Some(gi), Some(gj)) => crate::proof_engine::check_satisfiable(gi, gj),
            };
            if sat {
                contradictions.push(format!("'{}' vs '{}'", si, sj));
            }
        }
    }
    (in_force, contradictions)
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
) -> Vec<String> {
    let mut errors = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        // PREconditions are topology. Postconditions are physics — skipped.
        let mut pairs = Vec::new();
        collect_eq_triples(&t.contract.pre_condition, &mut pairs);
        for (l, r) in pairs {
            // 2026-09-22 (whole-bus equality, Slice 3): a range-indexed pair
            // (`u2.gpio[0..7] == u3.data[0..7]`) expands to N element unions.
            // A length mismatch is a hard error naming both lengths.
            match expand_bus_pair(&l, &r, instances, type_pins) {
                BusPair::Pairs(expanded) => {
                    for (lp, rp) in expanded {
                        let lk = pin_key(&lp.component, &lp.pin);
                        let rk = pin_key(&rp.component, &rp.pin);
                        ds.make(lk.clone());
                        ds.make(rk.clone());
                        ds.union(&lk, &rk);
                    }
                    continue;
                }
                BusPair::LengthMismatch(e) => {
                    errors.push(e);
                    continue;
                }
                BusPair::NotBus => {}
            }
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
        }
    }
    errors
}

/// 2026-09-22 (whole-bus equality, Slice 3): if both sides of an equality
/// are RANGE-indexed pin accesses (`u2.gpio[0..7] == u3.data[0..7]`),
/// expand to the element pairs. `Ok` = the element pairs (both sides are
/// range-indexed buses); `Err` = a bus comparison with mismatched lengths;
/// `NotBus` = at least one side is not a range-indexed pin access (the
/// caller falls back to single-pin resolution).
enum BusPair {
    NotBus,
    Pairs(Vec<(PinRef, PinRef)>),
    LengthMismatch(String),
}

fn expand_bus_pair(
    l: &Expr,
    r: &Expr,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> BusPair {
    let (Some((li, la, lo, hi, lincl)), Some((ri, ra, ro, rh, rincl))) =
        (range_index_pin(l), range_index_pin(r))
    else {
        return BusPair::NotBus;
    };
    // Length match: half-open [lo..hi) has hi-lo elements; inclusive has
    // hi-lo+1.
    let llen = if lincl { hi - lo + 1 } else { hi - lo };
    let rlen = if rincl { rh - ro + 1 } else { rh - ro };
    if llen != rlen {
        return BusPair::LengthMismatch(format!(
            "whole-bus equality '{}' ({} elements) vs '{}' ({} elements) — the buses must be the \
             same length. fix: use matching ranges (e.g. `[0..7]` on both sides).",
            l, llen, r, rlen
        ));
    }
    if lo > hi || ro > rh {
        return BusPair::LengthMismatch(format!(
            "whole-bus equality '{}' vs '{}' — an empty or reversed range",
            l, r
        ));
    }
    let mut out = Vec::with_capacity(llen as usize);
    for k in 0..llen {
        let (Some(lp), Some(rp)) = (
            range_pin_ref(&li, &la, lo + k, instances, type_pins),
            range_pin_ref(&ri, &ra, ro + k, instances, type_pins),
        ) else {
            return BusPair::NotBus;
        };
        out.push((lp, rp));
    }
    BusPair::Pairs(out)
}

/// `Expr::Field(Index(Field(Identifier(inst), arr), Range(lo, hi)), "voltage")`
/// — a range-indexed pin access `u2.gpio[0..7].voltage` → (inst, arr, lo,
/// hi, inclusive). Any other shape → None.
fn range_index_pin(expr: &Expr) -> Option<(String, String, u64, u64, bool)> {
    let Expr::Field(inner, prop) = expr else { return None };
    if prop != "voltage" {
        return None;
    }
    let Expr::Index(base, idx) = inner.as_ref() else { return None };
    let Expr::Range { start, end, inclusive } = idx.as_ref() else { return None };
    let Expr::Field(inst, arr) = base.as_ref() else { return None };
    let Expr::Identifier(inst_name) = inst.as_ref() else { return None };
    let (Expr::Decimal(lo), Expr::Decimal(hi)) = (start.as_ref(), end.as_ref()) else {
        return None;
    };
    Some((
        inst_name.clone(),
        arr.clone(),
        *lo as u64,
        *hi as u64,
        *inclusive,
    ))
}

/// Resolve a single element `inst.arr[k].voltage` of a range-indexed pin
/// access — the same shape `resolve_pin` handles for a decimal index.
fn range_pin_ref(
    inst_name: &str,
    arr_name: &str,
    k: u64,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<PinRef> {
    let expr = Expr::Field(
        Box::new(Expr::Index(
            Box::new(Expr::Field(
                Box::new(Expr::Identifier(inst_name.to_string())),
                arr_name.to_string(),
            )),
            Box::new(Expr::Decimal(k as i64)),
        )),
        "voltage".to_string(),
    );
    resolve_pin(&expr, instances, type_pins)
}

/// Resolve named-net annotations onto FINAL union-find roots (a name keyed
/// during unioning would go stale when later merges change roots). Two
/// DIFFERENT names on one net are a conflict; the same name twice is
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
/// 2026-09-23 (E14a gate): an `unpop` decoupler does NOT bridge — the
/// convention is a PRESENT-state obligation (the populated board must
/// have the capacitor); its pads stay on the sheet, but an absent part
/// carries no capacitance.
fn check_decoupling(
    ctx: &mut NetlistContext,
    unpop: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let decouplers: Vec<&ComponentInstance> = ctx
        .instances
        .values()
        .filter(|c| {
            !unpop.contains(&c.name)
                && ctx
                    .type_props
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

/// 2026-09-22 (ERC contention, D6/D12): a net with TWO drive-capable pins
/// is contention UNLESS every drive-capable member is WiredAnd (open-drain
/// wired-AND — `IoOd` may share; `Io`/`Out` may not). Property-driven: the
/// compiler reads `spec WiredAnd`, never a class name. An acknowledged
/// short (`shortcircuit unpop …`) on the net is the author's stated intent
/// and is exempt.
fn check_contention(
    nets: &[Net],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    shortcircuit: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for net in nets {
        let mut can_drive: Vec<(&str, &str, bool)> = Vec::new();
        for p in &net.pins {
            if shortcircuit.contains(&p.component) {
                continue;
            }
            let Some(inst) = instances.get(&p.component) else { continue };
            let Some(ti) = type_info.get(&inst.type_name) else { continue };
            let Some((i, _)) = ti.pins.iter().enumerate().find(|(_, (pn, _))| *pn == p.pin)
            else {
                continue;
            };
            let cls = &ti.pin_classes[i];
            if cls.can_drive {
                can_drive.push((&p.component, &p.pin, cls.wired_and));
            }
        }
        if can_drive.len() < 2 {
            continue;
        }
        let any_contending = can_drive.iter().any(|(_, _, wired)| !wired);
        if any_contending {
            let names = can_drive
                .iter()
                .map(|(c, p, w)| format!("{}.{}", c, p))
                .collect::<Vec<_>>()
                .join(", ");
            errors.push(format!(
                "net '{}' is driven by {} pins that are not open-drain ({}). Two drive-capable \
                 pins on one net is contention — a short unless the class declares `spec \
                 WiredAnd: true` (open-drain wired-AND). fix: make the extra drivers open-drain, \
                 or declare the short intentional with `shortcircuit unpop …;`",
                net.name, can_drive.len(), names
            ));
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
    strategy: Option<String>,
}

/// Diagnostic sink for body-fact collection (E14a/D16) — keeps the
/// walker's parameter list flat.
struct FactSink<'a> {
    node: &'a str,
    proofs: &'a mut Vec<String>,
    errors: &'a mut Vec<String>,
    intents: &'a mut Vec<(String, String)>,
    /// 2026-09-23 (E14a gate): pin-level drive intents `u1.en = true;`.
    pin_intents: &'a mut Vec<(String, PinRef)>,
    /// 2026-09-23 (E14b): voltage obligations `inst.pin.voltage >= V;`
    /// (min — pull-up forcing) and `inst.pin.voltage <= V;` (max —
    /// low-hold forcing); consumed after the wiring facts settle.
    obligations: &'a mut Vec<VoltageObligation>,
    bridges: &'a mut Vec<BridgeRequest>,
    /// 2026-09-22 (D16 p3b): disconnections from `open a.pin, b.pin;`.
    opens: &'a mut Vec<(Expr, Expr)>,
}

/// 2026-09-23 (E14b): a voltage obligation from a node body —
/// `inst.pin.voltage OP <literal>V;`. `min` obligations (>=) force an
/// external pull-up on a released net; `max` obligations (<=) force a
/// low-hold path to the return rail.
struct VoltageObligation {
    node: String,
    pin: PinRef,
    volts: f64,
    min: bool,
}

/// The region context threading through a guarded-fact walk (D16):
/// `control` Some means mechanizable — wiring facts become bridge
/// requests under it. `cond` Some (no control) means signal-level but
/// not mechanizable — the D7 gate. `thru` is this region's strategy
/// selection.
#[derive(Clone, Copy)]
struct RegionCtx<'a> {
    cond: Option<&'a str>,
    control: Option<&'a PinRef>,
    strategy: Option<&'a str>,
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

/// The control pin of a mechanizable condition (D16 phase 2), plus an
/// optional validation error. A single comparison `Expr::BinaryOp` whose
/// one operand resolves to a pin — the condition's net feeds the
/// mechanism control. The exact shape is validated: the pin operand must
/// be a `.voltage` access and the other operand a voltage literal
/// (`UnitLiteral`). A mechanism-SHAPED condition with the wrong detail
/// (e.g. `x = banana`, `x = 3.3V`, `x.voltage == banana`, or level-name
/// sugar `x = high` — deferred, see ledger Decision 2026-09-22) yields
/// `(None, Some(err))`; the caller records the error and skips the region
/// body (no cascade). `(None, None)` = region-level or non-mechanizable.
fn condition_control(expr: &Expr, ctx: &NetlistContext) -> (Option<PinRef>, Option<String>) {
    let Expr::BinaryOp(_, l, r) = expr else {
        return (None, None);
    };
    let lpin = resolve_pin(l, ctx.instances, ctx.type_pins);
    let rpin = resolve_pin(r, ctx.instances, ctx.type_pins);
    let (pin_side, other, ctrl) = match (lpin, rpin) {
        (Some(p), None) => (l, r, p),
        (None, Some(p)) => (r, l, p),
        _ => return (None, None),
    };
    if voltage_operand(pin_side, ctx).is_some()
        && matches!(other.as_ref(), Expr::UnitLiteral { .. })
    {
        return (Some(ctrl), None);
    }
    let shape = format!("{}.{}", ctrl.component, ctrl.pin);
    (
        None,
        Some(format!(
            "mechanism condition must be a single pin voltage comparison (e.g. `{shape}.voltage == 3.3V`), or a mechanism-bodied `when ... thru Type;` — got `{shape} = {}`, where the pin side must be a `.voltage` access and the non-pin side a voltage literal",
            other
        )),
    )
}

/// `expr` is a `.voltage` access on a declared pin (`u1.gpio0.voltage`),
/// resolved to the underlying pin. Anything else — bare `u1.gpio0`, a
/// non-voltage field, a non-pin operand — → None.
fn voltage_operand(expr: &Expr, ctx: &NetlistContext) -> Option<PinRef> {
    let Expr::Field(base, field) = expr else {
        return None;
    };
    if field != "voltage" {
        return None;
    }
    resolve_pin(base, ctx.instances, ctx.type_pins)
}

/// Pull a trailing `thru <Name>` marker out of a guarded body (D16 phase
/// 2): returns the strategy and the body without it.
fn take_thru_strategy(body: &[Statement]) -> (Option<String>, Vec<Statement>) {
    let mut strategy = None;
    let mut rest = Vec::with_capacity(body.len());
    for stmt in body {
        if let Statement::MetadataAssignment(k, crate::ast::PropertyValue::Identifier(name)) = stmt
        {
            if k == "thru" {
                strategy = Some(name.clone());
                continue;
            }
        }
        rest.push(stmt.clone());
    }
    (strategy, rest)
}

impl FactSink<'_> {
    /// One body assignment (E14a/D16): mechanism bridge, conditional-carve
    /// error, drive intent (instance or pin form), or pin-to-pin wire fact —
    /// every form contributes to the netlist or errors; nothing drops
    /// silently.
    fn fact_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
        reg: RegionCtx,
        ctx: &mut NetlistContext,
    ) {
        if reg.control.is_some() {
            self.mechanism_bridge(target, value, reg, ctx);
            return;
        }
        if let Some(desc) = reg.cond {
            self.errors.push(format!(
                "wiring inside `when {desc}` (node '{}') is conditional — copper cannot be. \
                 State the condition in the node guard (making the wiring unconditional in \
                 that region), or make it a single pin comparison and declare a switching \
                 part (`when ... thru Type;`, mechanism synthesis)",
                self.node
            ));
            return;
        }
        if let (Expr::Identifier(name), Expr::Bool(true)) = (target, value) {
            self.intents.push((self.node.to_string(), name.clone()));
            return;
        }
        if let Expr::Bool(b) = value {
            self.pin_drive_intent(target, *b, ctx);
            return;
        }
        self.wire_fact(target, value, ctx);
    }

    /// A wiring fact under a mechanizable condition becomes a bridge
    /// request (D16) — both sides must resolve to pins. The region carries
    /// the control pin and the optional strategy (bundled, not passed as
    /// loose parameters).
    fn mechanism_bridge(
        &mut self,
        target: &Expr,
        value: &Expr,
        reg: RegionCtx,
        ctx: &mut NetlistContext,
    ) {
        let Some(ctrl) = reg.control else {
            self.errors.push(format!(
                "wiring under a mechanism (node '{}') must be a pin-to-pin bridge",
                self.node
            ));
            return;
        };
        let (Some(a), Some(b)) = (
            resolve_pin(target, ctx.instances, ctx.type_pins),
            resolve_pin(value, ctx.instances, ctx.type_pins),
        ) else {
            self.errors.push(format!(
                "wiring under a mechanism (node '{}') must be a pin-to-pin bridge",
                self.node
            ));
            return;
        };
        self.bridges.push(BridgeRequest {
            node: self.node.to_string(),
            a,
            b,
            control: ctrl.clone(),
            strategy: reg.strategy.map(|s| s.to_string()),
        });
    }

    /// 2026-09-23 (E14a gate): pin-level drive intent `u1.en = true;` —
    /// the same completion rules (D13) as the instance form, addressed at
    /// one pin. A Bool RHS is ALWAYS an intent form: a target that is not
    /// a pin errors instead of falling through silently.
    fn pin_drive_intent(&mut self, target: &Expr, on: bool, ctx: &NetlistContext) {
        let Some(lp) = resolve_pin(target, ctx.instances, ctx.type_pins) else {
            self.errors.push(format!(
                "Bool assignment to '{}' (node '{}') is not an electronics fact — write a \
                 drive intent (`inst = true;` or `inst.pin = true;`) or a pin-to-pin wire \
                 (`a = b;`)",
                target, self.node
            ));
            return;
        };
        if !on {
            self.errors.push(format!(
                "drive intent '{}.{} = false' (node '{}') has no form — the drive intent is \
                 `= true` (participation); to state a connection, write the wire itself \
                 (`a = b;`)",
                lp.component, lp.pin, self.node
            ));
            return;
        }
        self.pin_intents.push((self.node.to_string(), lp));
    }

    /// A wiring fact: both sides resolve to declared pins and union.
    fn wire_fact(&mut self, target: &Expr, value: &Expr, ctx: &mut NetlistContext) {
        let (Some(lp), Some(rp)) = (
            resolve_pin(target, ctx.instances, ctx.type_pins),
            resolve_pin(value, ctx.instances, ctx.type_pins),
        ) else {
            self.errors.push(format!(
                "assignment '{}' = '{}' (node '{}') is not an electronics fact — both sides \
                 must resolve to declared pins (`u1.vout = r1.a;`), or use a drive intent \
                 (`inst = true;` / `inst.pin = true;`)",
                target, value, self.node
            ));
            return;
        };
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

    /// 2026-09-23 (E14b): a body voltage obligation. A MIN bound
    /// (`pin.voltage >= V`) forces a pull-up; a MAX bound (`<=`/`<`)
    /// forces a low-hold path (both via `force_*`). Any other bare
    /// expression is not an electronics fact.
    fn obligation_fact(&mut self, expr: &Expr, ctx: &NetlistContext) {
        let Expr::BinaryOp(kind, l, r) = expr else {
            self.not_a_fact(expr);
            return;
        };
        let min = match kind {
            BinaryOpKind::Ge | BinaryOpKind::Gt => true,
            BinaryOpKind::Le | BinaryOpKind::Lt => false,
            _ => {
                self.not_a_fact(expr);
                return;
            }
        };
        let Some((pin, volts)) = voltage_obligation_pair(l, r, ctx) else {
            self.not_a_fact(expr);
            return;
        };
        self.obligations.push(VoltageObligation {
            node: self.node.to_string(),
            pin,
            volts,
            min,
        });
    }

    fn not_a_fact(&mut self, expr: &Expr) {
        self.errors.push(format!(
            "expression '{}' (node '{}') is not an electronics fact — write a drive intent \
             (`inst = true;` / `inst.pin = true;`), a pin-to-pin wire (`a = b;`), or a \
             voltage obligation (`inst.pin.voltage >= 3.3V;`)",
            expr, self.node
        ));
    }

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
                    self.fact_assign(target, value, reg, ctx);
                }
                // 2026-09-22 (D16 p3b): author-expressed disconnection.
                Statement::Open(a, b) => {
                    self.opens.push(((**a).clone(), (**b).clone()));
                }
                // 2026-09-23 (E14b slice 1): a body voltage obligation.
                Statement::Expression(expr) => {
                    self.obligation_fact(expr, ctx);
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
        let (strategy, inner_rest) = take_thru_strategy(inner);
        let (control, cond_err) = condition_control(c, ctx);
        if let Some(e) = cond_err {
            // A malformed mechanism condition: record the shape error and
            // SKIP the region body — recursing would only cascade the
            // "copper cannot be conditional" wiring error beneath it.
            sink.errors.push(e);
            return;
        }
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
                strategy: strategy.as_deref().or(reg.strategy),
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
    sink.walk(&t.body, RegionCtx { cond: None, control: None, strategy: None }, ctx);
}

/// Eligibility of one declared instance as a bridge mechanism (D16): a
/// type with one Control pin (a gate/FET) or TWO Control pins (a relay
/// coil — the coil is one element across both pins) and at least two
/// Switchable pins, whose control pin(s) are unconnected or already on the
/// condition's net (pre-wired disambiguation). 2026-09-22 (asymmetric
/// switch parts): 1 or 2 control pins accepted — relays.
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
    if !(controls.len() == 1 || controls.len() == 2) || switchables < 2 {
        return false;
    }
    // Every control pin must be unconnected or already on the condition's
    // net (a relay coil's other pin may be on the condition net too).
    controls.iter().all(|(cname, _)| {
        let ckey = pin_key(&inst.name, cname);
        !(ctx.ds.contains(&ckey) && ctx.ds.find(&ckey) != *ctrl_root)
    })
}

/// Candidate mechanisms for a bridge request (D16 phase 2): declared
/// instances whose type has exactly one Control-class pin and at least
/// two Switchable-class pins, whose control pin is unconnected or
/// already on the condition's net (pre-wired disambiguation), narrowed
/// by the `via` strategy when given (type name).
fn mechanism_candidates(
    ctx: &mut NetlistContext,
    strategy: &Option<String>,
    control: &PinRef,
) -> Vec<String> {
    let ctrl_root = ctx.ds.find(&pin_key(&control.component, &control.pin));
    let mut out: Vec<String> = Vec::new();
    for inst in ctx.instances.values() {
        let Some(ti) = ctx.type_info.get(&inst.type_name) else {
            continue;
        };
        if let Some(t) = strategy {
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
    let controls: Vec<&(String, u64)> = ti
        .pins
        .iter()
        .enumerate()
        .filter(|(i, _)| ti.pin_classes[*i].control)
        .map(|(_, p)| p)
        .collect();
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
    // Control side: the condition net drives every control pin — one gate
    // pin (FET) or the relay's two-pin coil.
    let mut members = vec![(br.control.component.clone(), br.control.pin.clone())];
    for cpin in &controls {
        members.push((mech.name.clone(), cpin.0.clone()));
    }
    members.push((mech.name.clone(), paths[0].0.clone()));
    members.push((br.a.component.clone(), br.a.pin.clone()));
    members.push((mech.name.clone(), paths[1].0.clone()));
    members.push((br.b.component.clone(), br.b.pin.clone()));
    union_all(ctx, members);
    proofs.push(format!(
        "mechanism '{}' bridges {}.{} <-> {}.{} under {}.{} (node '{}') with {} control pin(s)",
        mech.name, br.a.component, br.a.pin, br.b.component, br.b.pin,
        br.control.component, br.control.pin, br.node, controls.len()
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
        let mut candidates = mechanism_candidates(ctx, &br.strategy, &br.control);
        candidates.sort();
        match candidates.len() {
            0 => {
                let strategy_hint = match &br.strategy {
                    Some(t) => format!(
                        "strategy names '{}' but no qualifying mechanism of that type is declared — declare one with one Control pin (or a two-pin coil) and at least two Path pins",
                    t
                ),
                    None => "no qualifying mechanism is declared — declare one with one Control pin (or a two-pin coil) and at least two Path pins, or narrow with `when ... thru Type;`"
                        .to_string(),
                };
                errors.push(format!(
                    "bridge {}.{} <-> {}.{} under {}.{} (node '{}') cannot be synthesized: {}",
                    br.a.component, br.a.pin, br.b.component, br.b.pin,
                    br.control.component, br.control.pin, br.node, strategy_hint
                ));
            }
            1 => {
                synthesize_one(ctx, &candidates[0], br, proofs);
                let strategy = br.strategy.as_deref().unwrap_or("-");
                synthesized.push(format!(
                    "{}.{} <-> {}.{} under {}.{} thru {} (node '{}')",
                    br.a.component, br.a.pin, br.b.component, br.b.pin,
                    br.control.component, br.control.pin, strategy, br.node
                ));
            }
            n => {
                errors.push(format!(
                    "bridge {}.{} <-> {}.{} under {}.{} (node '{}') is ambiguous: {} mechanisms could synthesize it ({}). Disambiguate with the strategy clause: `when ... thru <Type>;` or pre-wire one mechanism's control pin",
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
/// 2026-09-23 (E14a gate, D13): free drive-capable candidate pins for an
/// intent completion — unconnected `spec CanDrive` pins of every OTHER
/// instance, sorted so diagnostics are deterministic.
fn free_can_drive_candidates(ctx: &NetlistContext, exclude_inst: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for (other, oi) in ctx.instances {
        if other == exclude_inst {
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
    candidates
}

/// 2026-09-23 (E14b slice 4): batch drive assignment — several open
/// drive intents competing for interchangeable free drive-capable pins
/// are a perfect matching, wired deterministically (sorted intent ↔
/// sorted pin). When supply < demand, the pins are mixed-class, or fewer
/// than two intents are completable, the intents fall through to the
/// per-intent completions unchanged (their D13 errors stand). Returns the
/// intents the batch did NOT resolve.
fn solve_drive_assignment(
    ctx: &mut NetlistContext,
    intents: &[(String, String)],
    pin_intents: &[(String, PinRef)],
    proofs: &mut Vec<String>,
) -> (Vec<(String, String)>, Vec<(String, PinRef)>) {
    let mut unmatched_inst: Vec<(String, String)> = Vec::new();
    let mut unmatched_pin: Vec<(String, PinRef)> = Vec::new();
    let mut demands: Vec<DriveDemand> = Vec::new();
    for (node, inst_name) in intents {
        match instance_single_open(ctx, inst_name) {
            Some(pin) => demands.push(DriveDemand {
                node: node.clone(),
                target: PinRef {
                    component: inst_name.clone(),
                    pin,
                    number: 0,
                },
                kind: DriveKind::Instance,
            }),
            None => unmatched_inst.push((node.clone(), inst_name.clone())),
        }
    }
    for (node, pin) in pin_intents {
        if pin_connected(ctx.ds, &pin_key(&pin.component, &pin.pin)) {
            unmatched_pin.push((node.clone(), pin.clone()));
        } else {
            demands.push(DriveDemand {
                node: node.clone(),
                target: pin.clone(),
                kind: DriveKind::Pin,
            });
        }
    }
    let batchable = demands.len() >= 2;
    if batchable {
        let supply = free_can_drive_pins(ctx);
        if supply.len() < demands.len() || !pins_interchangeable(ctx, &supply) {
            reclaim_demands(demands, &mut unmatched_inst, &mut unmatched_pin);
            return (unmatched_inst, unmatched_pin);
        }
        wire_drive_matching(ctx, demands, &supply, proofs);
        return (unmatched_inst, unmatched_pin);
    }
    reclaim_demands(demands, &mut unmatched_inst, &mut unmatched_pin);
    (unmatched_inst, unmatched_pin)
}

/// Return unresolved demands to the per-intent lists (their D13 errors
/// stand) — the single-intent and mixed/shortage fall-through paths.
fn reclaim_demands(
    demands: Vec<DriveDemand>,
    unmatched_inst: &mut Vec<(String, String)>,
    unmatched_pin: &mut Vec<(String, PinRef)>,
) {
    for d in demands {
        match d.kind {
            DriveKind::Instance => {
                unmatched_inst.push((d.node, d.target.component));
            }
            DriveKind::Pin => unmatched_pin.push((d.node, d.target)),
        }
    }
}

/// Wire the perfect matching — sorted demand ↔ sorted supply.
fn wire_drive_matching(
    ctx: &mut NetlistContext,
    mut demands: Vec<DriveDemand>,
    supply: &[(String, String)],
    proofs: &mut Vec<String>,
) {
    demands.sort_by(|a, b| {
        (a.node.as_str(), a.target.component.as_str(), a.target.pin.as_str())
            .cmp(&(b.node.as_str(), b.target.component.as_str(), b.target.pin.as_str()))
    });
    for (i, d) in demands.iter().enumerate() {
        let (scomp, spin) = &supply[i];
        let tkey = pin_key(&d.target.component, &d.target.pin);
        let skey = pin_key(scomp, spin);
        ctx.ds.make(tkey.clone());
        ctx.ds.make(skey.clone());
        ctx.ds.union(&tkey, &skey);
        proofs.push(format!(
            "wired by drive assignment (node '{}'): {}.{} <-> {}.{} — matched among interchangeable free drive-capable pins",
            d.node, d.target.component, d.target.pin, scomp, spin
        ));
    }
}

/// The exactly-one unconnected pin of an instance, if any — the drive
/// intent completes that pin.
fn instance_single_open(ctx: &NetlistContext, inst_name: &str) -> Option<String> {
    let inst = ctx.instances.get(inst_name)?;
    let pins = ctx.type_pins.get(&inst.type_name)?;
    let open: Vec<&String> = pins
        .iter()
        .filter(|(n, _)| !pin_connected(ctx.ds, &pin_key(inst_name, n)))
        .map(|(n, _)| n)
        .collect();
    if open.len() == 1 {
        Some(open[0].clone())
    } else {
        None
    }
}

/// All unconnected drive-capable pins, sorted — the batch's supply.
fn free_can_drive_pins(ctx: &NetlistContext) -> Vec<(String, String)> {
    let mut pins: Vec<(String, String)> = Vec::new();
    for (other, oi) in ctx.instances {
        collect_free_drive_pins(ctx, other, oi, &mut pins);
    }
    pins.sort();
    pins
}

/// The free drive-capable pins of one instance, appended to the list.
fn collect_free_drive_pins(
    ctx: &NetlistContext,
    other: &str,
    oi: &ComponentInstance,
    out: &mut Vec<(String, String)>,
) {
    let Some(oti) = ctx.type_info.get(&oi.type_name) else {
        return;
    };
    for (i, (n, _)) in oti.pins.iter().enumerate() {
        if oti.pin_classes[i].can_drive && !pin_connected(ctx.ds, &pin_key(other, n)) {
            out.push((other.to_string(), n.clone()));
        }
    }
}

/// The free pins are all interchangeable — same PinClassProps — so any
/// matching is electrically equivalent and a deterministic pick is not a
/// silent choice (D13: the choice never matters).
fn pins_interchangeable(ctx: &NetlistContext, pins: &[(String, String)]) -> bool {
    let mut props: Option<&PinClassProps> = None;
    for (comp, name) in pins {
        let pin = PinRef {
            component: comp.clone(),
            pin: name.clone(),
            number: 0,
        };
        let Some(c) = pin_classes_of(ctx, &pin) else {
            return false;
        };
        match props {
            Some(p) if p != c => return false,
            None => props = Some(c),
            _ => {}
        }
    }
    true
}

/// One open drive intent awaiting a pin — bundled for the assignment sort.
struct DriveDemand {
    node: String,
    target: PinRef,
    kind: DriveKind,
}

/// Whether the demand came from an instance intent or a pin intent.
enum DriveKind {
    Instance,
    Pin,
}

/// 2026-09-23 (E14b slice 1): the pin+volts of a voltage obligation pair —
/// one side is a `.voltage` pin access, the other a voltage literal,
/// either orientation.
fn voltage_obligation_pair(
    l: &Expr,
    r: &Expr,
    ctx: &NetlistContext,
) -> Option<(PinRef, f64)> {
    if let (Some(p), Some(v)) = (
        resolve_voltage_pin(l, ctx.instances, ctx.type_pins),
        extract_voltage(r),
    ) {
        return Some((p, v));
    }
    if let (Some(p), Some(v)) = (
        resolve_voltage_pin(r, ctx.instances, ctx.type_pins),
        extract_voltage(l),
    ) {
        return Some((p, v));
    }
    None
}

/// 2026-09-23 (E14b): the voltage-obligation forcing pass — bundles
/// the working set (nets context, driven rails, diagnostic sinks) so the
/// per-obligation methods stay under the parameter gate (FactSink
/// pattern). MIN obligations (`>=`) force a pull-up to the lowest
/// qualifying driven rail; MAX obligations (`<=`) force a low-hold path
/// to the return rail. Same-root obligations dedup per side.
struct ObligationForcing<'a, 'b> {
    ctx: &'a mut NetlistContext<'b>,
    rails: BTreeMap<String, f64>,
    errors: &'a mut Vec<String>,
    proofs: &'a mut Vec<String>,
}

impl<'a, 'b> ObligationForcing<'a, 'b> {
    fn new(
        ctx: &'a mut NetlistContext<'b>,
        rails: BTreeMap<String, f64>,
        errors: &'a mut Vec<String>,
        proofs: &'a mut Vec<String>,
    ) -> Self {
        ObligationForcing { ctx, rails, errors, proofs }
    }

    fn run(&mut self, obligations: &[VoltageObligation]) {
        if obligations.is_empty() {
            return;
        }
        self.assemble_buses(obligations);
        let mut mins: BTreeMap<String, (f64, String, Vec<String>)> = BTreeMap::new();
        let mut maxs: BTreeMap<String, (f64, String, Vec<String>)> = BTreeMap::new();
        for ob in obligations {
            let root = self.ctx.ds.find(&pin_key(&ob.pin.component, &ob.pin.pin));
            let table = if ob.min { &mut mins } else { &mut maxs };
            let entry = table.entry(root).or_insert((0.0, String::new(), Vec::new()));
            if ob.volts > entry.0 {
                entry.0 = ob.volts;
            }
            entry.1 = ob.pin.component.clone();
            entry.2.push(ob.node.clone());
        }
        for (root, (vmin, _inst, nodes)) in mins {
            self.force_pull_up(&root, vmin, &nodes);
        }
        for (root, (vmax, inst, nodes)) in maxs {
            self.force_low(&root, &inst, vmax, &nodes);
        }
    }

    /// Bus assembly (E14b-3): MIN obligations on same-name WiredAnd-class
    /// pins at the SAME voltage union into one net — the pin name is the
    /// author's signal identity, WiredAnd is the class that may share a
    /// driven net, and the shared obligation is the coupling. Different
    /// names, voltages, or non-WiredAnd pins never union; each keeps its
    /// own net and pull-up (honest D13). Runs before the per-root dedup.
    fn assemble_buses(&mut self, obligations: &[VoltageObligation]) {
        let mut groups: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
        let mut volts: BTreeMap<(String, i64), f64> = BTreeMap::new();
        for ob in obligations {
            if !ob.min {
                continue;
            }
            let Some(classes) = pin_classes_of(self.ctx, &ob.pin) else {
                continue;
            };
            if !classes.wired_and {
                continue;
            }
            let key = (ob.pin.pin.clone(), (ob.volts * 1000.0).round() as i64);
            volts.entry(key.clone()).or_insert(ob.volts);
            groups
                .entry(key)
                .or_default()
                .push(pin_key(&ob.pin.component, &ob.pin.pin));
        }
        for (key, mut keys) in groups {
            keys.sort();
            keys.dedup();
            if keys.len() < 2 {
                continue;
            }
            self.union_keys(&keys);
            self.proofs.push(format!(
                "bus assembled by shared min obligation '>= {}V': {} — same-name WiredAnd pins",
                volts[&key],
                keys.join(" <-> ")
            ));
        }
    }

    /// Union a list of pins onto one net (the first key is the root).
    fn union_keys(&mut self, keys: &[String]) {
        let Some(first) = keys.first() else {
            return;
        };
        for k in &keys[1..] {
            self.ctx.ds.make(first.clone());
            self.ctx.ds.make(k.clone());
            self.ctx.ds.union(first, k);
        }
    }

    /// A MIN obligation: wire a free `spec PullUp` part between the net
    /// and the lowest qualifying driven rail. D13: no part or no rail →
    /// hard error; distinct-value parts (or rails tied at the minimal
    /// volts) → enumerated ambiguous error.
    fn force_pull_up(&mut self, root: &str, vmin: f64, nodes: &[String]) {
        if self.rails.get(root).map_or(false, |v| *v >= vmin) {
            self.proofs.push(format!(
                "voltage obligation '>= {}V' (node '{}'): net already driven at {}V — satisfied",
                vmin, nodes.join(", "), self.rails[root]
            ));
            return;
        }
        if net_has_pull_up(self.ctx, root) {
            self.proofs.push(format!(
                "voltage obligation '>= {}V' (node '{}'): net already pulled up — satisfied",
                vmin, nodes.join(", ")
            ));
            return;
        }
        let parts = self.free_parts();
        // 2026-09-23 (E14b slice 5): a MIN obligation is satisfied by ANY
        // pull-up resistance — a released (high-Z) net sits at the rail
        // regardless of the resistor value — so distinct values are still
        // interchangeable for satisfaction and the pick is deterministic
        // (D13: the choice never matters). Switches keep the distinct-value
        // ambiguity (a multi-pole switch has different capacity).
        let Some(part) = parts.first() else {
            self.errors.push(format!(
                "voltage obligation '>= {}V' (node '{}') has no pull-up part: no free `spec PullUp: true` two-pin part remains. Add one (e.g. a resistor) or wire the net to a drive explicitly",
                vmin, nodes.join(", ")
            ));
            return;
        };
        match self.qualifying_rail(vmin) {
            Some((rail, v)) => self.wire(PullUpWire {
                part: part.clone(),
                root: root.to_string(),
                rail,
                rail_volts: v,
                vmin,
                nodes: nodes.to_vec(),
            }),
            None => self.errors.push(format!(
                "voltage obligation '>= {}V' (node '{}') has no qualifying rail: no driven supply net reaches {}V (driven: {}). Add a drive or lower the obligation",
                vmin,
                nodes.join(", "),
                vmin,
                self.rails
                    .iter()
                    .map(|(r, v)| format!("{}V@{}", v, r))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// A MAX obligation: hold the net at or below Vmax by forcing a path
    /// to the instance's return rail. A net that is ALSO pulled up needs a
    /// switchable part (it cannot be held low while the pull-up holds it
    /// high — that is a mechanism); an isolated net wires directly.
    fn force_low(&mut self, root: &str, inst: &str, vmax: f64, nodes: &[String]) {
        let Some(ret) = self.return_root(inst).or_else(|| self.any_return_root()) else {
            self.errors.push(format!(
                "voltage obligation '<= {}V' (node '{}') has no return rail: no Return-class pin is connected anywhere. Wire a ground path explicitly",
                vmax, nodes.join(", ")
            ));
            return;
        };
        if *root == ret {
            self.proofs.push(format!(
                "voltage obligation '<= {}V' (node '{}'): net is the return rail — satisfied",
                vmax, nodes.join(", ")
            ));
            return;
        }
        if self.net_has_switchable_to(root, &ret) {
            self.proofs.push(format!(
                "voltage obligation '<= {}V' (node '{}'): net already switchable to return — satisfied",
                vmax, nodes.join(", ")
            ));
            return;
        }
        let parts = self.free_switchable_parts();
        if parts.len() > 1 && self.values_distinct(&parts) {
            self.errors.push(format!(
                "voltage obligation '<= {}V' (node '{}') is ambiguous: {} switchable parts of differing value are free ({}). Wire the low path explicitly, or unpop the extras",
                vmax, nodes.join(", "), parts.len(), parts.join(", ")
            ));
            return;
        }
        let part = parts.first().cloned();
        let Some(part) = part else {
            if net_has_pull_up(self.ctx, root) {
                self.errors.push(format!(
                    "voltage obligation '<= {}V' (node '{}') cannot hold the net low: it is pulled up but no free switchable part remains to open it. Add a switch (its pins ascribe `: Path`), or wire the low path explicitly",
                    vmax, nodes.join(", ")
                ));
            } else {
                self.wire_low(LowHoldWire {
                    part: None,
                    root: root.to_string(),
                    ret,
                    vmax,
                    nodes: nodes.to_vec(),
                });
            }
            return;
        };
        self.wire_low(LowHoldWire {
            part: Some(part),
            root: root.to_string(),
            ret,
            vmax,
            nodes: nodes.to_vec(),
        });
    }

    /// Free `spec PullUp` parts, sorted — the pick among interchangeable
    /// parts is deterministic.
    fn free_parts(&self) -> Vec<String> {
        let mut parts: Vec<String> = self
            .ctx
            .instances
            .values()
            .filter(|c| {
                self.ctx
                    .type_props
                    .get(&c.type_name)
                    .and_then(|m| m.get("pull_up"))
                    .and_then(property_bool)
                    .unwrap_or(false)
            })
            .filter(|c| pins_free(self.ctx, c))
            .map(|c| c.name.clone())
            .collect();
        parts.sort();
        parts
    }

    /// Free switchable parts — a type with at least two Switchable-class
    /// pins where at least one switchable pin is unconnected (the return
    /// side may already be wired through a guard equality), sorted.
    fn free_switchable_parts(&self) -> Vec<String> {
        let mut parts: Vec<String> = self
            .ctx
            .instances
            .values()
            .filter(|c| {
                self.switchable_pins(&c.name).len() >= 2
                    && self
                        .switchable_pins(&c.name)
                        .iter()
                        .any(|(n, _)| !pin_connected(self.ctx.ds, &pin_key(&c.name, n)))
            })
            .map(|c| c.name.clone())
            .collect();
        parts.sort();
        parts
    }

    /// The Switchable-class pins of an instance, sorted (index order).
    fn switchable_pins(&self, inst: &str) -> Vec<(String, u64)> {
        let Some(c) = self.ctx.instances.get(inst) else {
            return Vec::new();
        };
        let Some(ti) = self.ctx.type_info.get(&c.type_name) else {
            return Vec::new();
        };
        ti.pins
            .iter()
            .enumerate()
            .filter(|(i, _)| ti.pin_classes[*i].switchable)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// The instance's own Return-class pin root — the natural return rail
    /// for its nets.
    fn return_root(&mut self, inst: &str) -> Option<String> {
        let c = self.ctx.instances.get(inst)?;
        let ti = self.ctx.type_info.get(&c.type_name)?;
        let ret = ti
            .pins
            .iter()
            .enumerate()
            .find(|(i, _)| ti.pin_classes[*i].return_pin)
            .map(|(_, p)| p)?;
        Some(self.ctx.ds.find(&pin_key(inst, &ret.0)))
    }

    /// Any connected Return-class pin root, deterministically chosen.
    fn any_return_root(&mut self) -> Option<String> {
        let mut roots: Vec<String> = self
            .ctx
            .instances
            .values()
            .filter_map(|c| {
                let ti = self.ctx.type_info.get(&c.type_name)?;
                let ret = ti
                    .pins
                    .iter()
                    .enumerate()
                    .find(|(i, _)| ti.pin_classes[*i].return_pin)
                    .map(|(_, p)| p)?;
                let key = pin_key(&c.name, &ret.0);
                if self.ctx.ds.contains(&key) {
                    Some(self.ctx.ds.find(&key))
                } else {
                    None
                }
            })
            .collect();
        roots.sort();
        roots.first().cloned()
    }

    /// Whether a declared switchable part already spans the two nets.
    fn net_has_switchable_to(&mut self, root: &str, ret: &str) -> bool {
        let Some(inst) = self
            .ctx
            .instances
            .values()
            .find(|c| self.switchable_pins(&c.name).len() >= 2) else {
            return false;
        };
        let pins = self.switchable_pins(&inst.name);
        pins.iter()
            .any(|(n, _)| self.ctx.ds.find(&pin_key(&inst.name, n)) == *root)
            && pins.iter()
                .any(|(n, _)| self.ctx.ds.find(&pin_key(&inst.name, n)) == *ret)
    }

    /// Whether the free parts carry more than one distinct value — the
    /// pick then MATTERS (pull-up strength) and D13 demands explicit wiring.
    fn values_distinct(&self, parts: &[String]) -> bool {
        let distinct: std::collections::BTreeSet<&str> = parts
            .iter()
            .filter_map(|p| {
                self.ctx.instances[p]
                    .properties
                    .iter()
                    .find(|(k, _)| k == "value")
                    .map(|(_, v)| v.as_str())
            })
            .collect();
        distinct.len() > 1
    }

    /// Lowest driven rail at or above `vmin`, with its volts. None when no
    /// rail qualifies OR when multiple rails tie at the minimal volts —
    /// both are hard errors, surfaced by the caller's message.
    fn qualifying_rail(&mut self, vmin: f64) -> Option<(String, f64)> {
        let mut cands: Vec<(f64, String)> = self
            .rails
            .iter()
            .filter(|(_, v)| **v >= vmin)
            .map(|(r, v)| (*v, r.clone()))
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        let best = cands.first()?;
        let ties: Vec<&String> = cands
            .iter()
            .filter(|(v, _)| (v - best.0).abs() < f64::EPSILON)
            .map(|(_, r)| r)
            .collect();
        if ties.len() > 1 {
            self.errors.push(format!(
                "voltage obligation '>= {}V' is ambiguous: multiple rails tie at the minimal {}V ({}). Tie the net to one rail explicitly (`inst.pin = rail.pin;` in a node guard) or raise the obligation",
                vmin, best.0, ties.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
            return None;
        }
        Some((ties[0].clone(), best.0))
    }

    /// Wire the chosen pull-up part between the obligation net and the rail.
    fn wire(&mut self, w: PullUpWire) {
        let Some(pi) = self.ctx.type_pins.get(&self.ctx.instances[w.part.as_str()].type_name) else {
            return;
        };
        let Some(pa) = pi.first() else {
            return;
        };
        let Some(pb) = pi.get(1) else {
            return;
        };
        let pa_key = pin_key(&w.part, &pa.0);
        let pb_key = pin_key(&w.part, &pb.0);
        self.ctx.ds.make(w.root.clone());
        self.ctx.ds.make(pa_key.clone());
        self.ctx.ds.make(w.rail.clone());
        self.ctx.ds.make(pb_key.clone());
        self.ctx.ds.union(&w.root, &pa_key);
        self.ctx.ds.union(&w.rail, &pb_key);
        self.proofs.push(format!(
            "pull-up forced by '>= {}V' (node '{}'): {}.{} <-> obligation net, {}.{} <-> rail ({}V) — obligation satisfied",
            w.vmin, w.nodes.join(", "), w.part, pa.0, w.part, pb.0, w.rail_volts
        ));
    }

    /// Wire the low-hold path: net → (switchable part) → return rail.
    /// The part's return side may already be wired (guard equality); only
    /// the free switchable pins are consumed here.
    fn wire_low(&mut self, w: LowHoldWire) {
        let Some(part) = &w.part else {
            self.ctx.ds.make(w.root.clone());
            self.ctx.ds.make(w.ret.clone());
            self.ctx.ds.union(&w.root, &w.ret);
            self.proofs.push(format!(
                "low-hold forced by '<= {}V' (node '{}'): net <-> return rail — obligation satisfied",
                w.vmax, w.nodes.join(", ")
            ));
            return;
        };
        let pins = self.switchable_pins(part);
        let mut free: Vec<&(String, u64)> = pins
            .iter()
            .filter(|(n, _)| !pin_connected(self.ctx.ds, &pin_key(part, n)))
            .collect();
        free.sort_by(|a, b| a.0.cmp(&b.0));
        let Some(pa) = free.first() else {
            return;
        };
        let pa_key = pin_key(part, &pa.0);
        self.ctx.ds.make(w.root.clone());
        self.ctx.ds.make(pa_key.clone());
        self.ctx.ds.union(&w.root, &pa_key);
        if pins
            .iter()
            .any(|(n, _)| self.ctx.ds.find(&pin_key(part, n)) == *w.ret)
        {
            self.proofs.push(format!(
                "low-hold forced by '<= {}V' (node '{}'): {}.{} <-> net, other path pin on return rail — obligation satisfied",
                w.vmax, w.nodes.join(", "), part, pa.0
            ));
            return;
        }
        let Some(pb) = free.get(1) else {
            self.errors.push(format!(
                "low-hold obligation '<= {}V' (node '{}'): {}.{} has only one free switchable pin and its other path pin is not on the return rail — it cannot complete the low path. Wire the low path explicitly",
                w.vmax, w.nodes.join(", "), part, pa.0
            ));
            return;
        };
        let pb_key = pin_key(part, &pb.0);
        self.ctx.ds.make(w.ret.clone());
        self.ctx.ds.make(pb_key.clone());
        self.ctx.ds.union(&w.ret, &pb_key);
        self.proofs.push(format!(
            "low-hold forced by '<= {}V' (node '{}'): {}.{} <-> net, {}.{} <-> return rail — obligation satisfied",
            w.vmax, w.nodes.join(", "), part, pa.0, part, pb.0
        ));
    }
}

/// The resolved pull-up decision handed to `ObligationForcing::wire` —
/// bundled so the method stays under the parameter gate.
struct PullUpWire {
    part: String,
    root: String,
    rail: String,
    rail_volts: f64,
    vmin: f64,
    nodes: Vec<String>,
}

/// The resolved low-hold decision handed to `ObligationForcing::wire_low`
/// — `part` None means a direct net-to-return wire.
struct LowHoldWire {
    part: Option<String>,
    root: String,
    ret: String,
    vmax: f64,
    nodes: Vec<String>,
}

/// Driven supply rails: `[x.voltage == <literal>]` drives across all
/// contracts, resolved to union-find roots; only Supply-class pins count
/// as rails (a rail must be a real source, not an arbitrary driven net).
fn collect_driven_rails(items: &[TopLevel], ctx: &mut NetlistContext) -> BTreeMap<String, f64> {
    let mut rails: BTreeMap<String, f64> = BTreeMap::new();
    let mut eqs: Vec<(Expr, Expr)> = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else {
            continue;
        };
        collect_eq_triples(&t.contract.pre_condition, &mut eqs);
        collect_eq_triples(&t.contract.post_condition, &mut eqs);
    }
    for (l, r) in eqs {
        let Some((pin, volts)) = voltage_drive(&l, &r, ctx.instances, ctx.type_pins) else {
            continue;
        };
        let Some(classes) = pin_classes_of(ctx, &pin) else {
            continue;
        };
        if !classes.supply {
            continue;
        }
        let root = ctx.ds.find(&pin_key(&pin.component, &pin.pin));
        let entry = rails.entry(root).or_insert(0.0);
        if volts > *entry {
            *entry = volts;
        }
    }
    rails
}

/// 2026-09-23 (E14b): entry — guard, collect driven rails, then force
/// every voltage obligation (min → pull-up, max → low-hold).
fn force_voltage_obligations(
    items: &[TopLevel],
    ctx: &mut NetlistContext,
    obligations: &[VoltageObligation],
    errors: &mut Vec<String>,
    proofs: &mut Vec<String>,
) {
    if obligations.is_empty() {
        return;
    }
    let rails = collect_driven_rails(items, ctx);
    let mut forcing = ObligationForcing::new(ctx, rails, errors, proofs);
    forcing.run(obligations);
}

/// Pin-class properties of a resolved pin (for the rail filter) — the
/// type_info table is keyed by TYPE name, so the instance is resolved
/// first.
fn pin_classes_of<'a>(ctx: &'a NetlistContext, pin: &PinRef) -> Option<&'a PinClassProps> {
    let inst = ctx.instances.get(&pin.component)?;
    let oti = ctx.type_info.get(&inst.type_name)?;
    let idx = oti.pins.iter().position(|(n, _)| n == &pin.pin)?;
    oti.pin_classes.get(idx)
}

/// All of the part's pins are unconnected (never made).
fn pins_free(ctx: &NetlistContext, c: &ComponentInstance) -> bool {
    let Some(ps) = ctx.type_pins.get(&c.type_name) else {
        return false;
    };
    ps.iter().all(|(n, _)| !pin_connected(ctx.ds, &pin_key(&c.name, n)))
}

/// The net already has a `spec PullUp` part on it.
fn net_has_pull_up(ctx: &mut NetlistContext, root: &str) -> bool {
    let instances = &ctx.instances;
    let type_props = &ctx.type_props;
    let type_pins = &ctx.type_pins;
    let ds = &mut ctx.ds;
    instances.values().any(|c| {
        type_props
            .get(&c.type_name)
            .and_then(|m| m.get("pull_up"))
            .and_then(property_bool)
            .unwrap_or(false)
            && type_pins.get(&c.type_name).map_or(false, |ps| {
                ps.iter()
                    .any(|(n, _)| ds.find(&pin_key(&c.name, n)) == *root)
            })
    })
}

/// 2026-09-23 (E14a gate): pin-level drive intent `u1.en = true;` — the
/// pin completes against the sole unconnected drive-capable pin elsewhere
/// (same D13 rules as the instance form). An already-connected pin
/// records — the intent states participation, not a new edge.
fn complete_pin_intent(
    pin: &PinRef,
    node: &str,
    ctx: &mut NetlistContext,
) -> Result<String, String> {
    let key = pin_key(&pin.component, &pin.pin);
    if pin_connected(ctx.ds, &key) {
        return Ok(format!(
            "pin intent '{}.{} = true' (node '{}'): already connected — recorded",
            pin.component, pin.pin, node
        ));
    }
    let candidates = free_can_drive_candidates(ctx, &pin.component);
    match candidates.len() {
        0 => Err(format!(
            "pin intent '{}.{} = true' (node '{}') has no completion: no unconnected drive-capable pin (spec CanDrive) remains in scope",
            pin.component, pin.pin, node
        )),
        1 => {
            let mut parts = candidates[0].splitn(2, '.');
            let (co, cp) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let rk = pin_key(co, cp);
            ctx.ds.make(key.clone());
            ctx.ds.make(rk.clone());
            ctx.ds.union(&key, &rk);
            Ok(format!(
                "wired by pin intent '{}.{} = true' (node '{}'): {}.{} <-> {} — single drive-capable candidate",
                pin.component, pin.pin, node, pin.component, pin.pin, candidates[0]
            ))
        }
        n => Err(format!(
            "pin intent '{}.{} = true' (node '{}') is ambiguous: {} drive-capable pins could complete it ({}). State the connection explicitly in the node guard",
            pin.component, pin.pin, node, n, candidates.join(", ")
        )),
    }
}

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
    let candidates = free_can_drive_candidates(ctx, inst_name);
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
    let mut pin_intents: Vec<(String, PinRef)> = Vec::new();
    let mut obligations: Vec<VoltageObligation> = Vec::new();
    let mut bridges: Vec<BridgeRequest> = Vec::new();
    let mut opens: Vec<(Expr, Expr)> = Vec::new();
    for item in items {
        let TopLevel::Transaction(t) = item else {
            continue;
        };
        let mut sink = FactSink {
            node: t.name.as_str(),
            proofs: &mut proofs,
            errors: &mut errors,
            intents: &mut intents,
            pin_intents: &mut pin_intents,
            obligations: &mut obligations,
            bridges: &mut bridges,
            opens: &mut opens,
        };
        body_facts(t, ctx, &mut sink);
    }
    let synthesized = synthesize_bridges(ctx, &bridges, &mut proofs, &mut errors);
    // 2026-09-23 (E14b): voltage obligations realize BEFORE drive
    // completions — bus assembly + pull-ups connect the open-drain nets,
    // so an unassembled sda/scl never appears as a drive candidate for a
    // later instance intent (led1 saw five "free" IoOd pins and went
    // ambiguous). The forcing consumes passive parts (pull-up resistors,
    // switches), never CanDrive pins, so it cannot steal a completion.
    force_voltage_obligations(items, ctx, &obligations, &mut errors, &mut proofs);
    // 2026-09-23 (E14b slice 4): batch drive assignment first — several
    // open drive intents over interchangeable free drive-capable pins are
    // a perfect matching, wired deterministically; intents it cannot
    // resolve fall through to the per-intent completions (D13).
    let (unmatched_intents, unmatched_pins) =
        solve_drive_assignment(ctx, &intents, &pin_intents, &mut proofs);
    for (node, inst_name) in &unmatched_intents {
        match complete_intent(inst_name, node, ctx) {
            Ok(proof) => proofs.push(proof),
            Err(e) => errors.push(e),
        }
    }
    // 2026-09-23 (E14a gate): pin-level intents run after the instance
    // forms — facts and instance completions have claimed their copper,
    // so a pin intent records or completes against what remains.
    for (node, pin) in &unmatched_pins {
        match complete_pin_intent(pin, node, ctx) {
            Ok(proof) => proofs.push(proof),
            Err(e) => errors.push(e),
        }
    }
    // 2026-09-22 (D16 p3b): disconnections — two pins that must NOT be
    // connected. A wiring fact or mechanism bridge that would connect them
    // is a hard error (the complement of the phase-3 redundancy gate).
    for (a, b) in &opens {
        let (Some(ap), Some(bp)) = (
            resolve_pin(a, ctx.instances, ctx.type_pins),
            resolve_pin(b, ctx.instances, ctx.type_pins),
        ) else {
            errors.push(format!(
                "open must name two pins, got '{}' and '{}'",
                a, b
            ));
            continue;
        };
        let ak = pin_key(&ap.component, &ap.pin);
        let bk = pin_key(&bp.component, &bp.pin);
        if ctx.ds.find(&ak) == ctx.ds.find(&bk) {
            errors.push(format!(
                "open {}.{}, {}.{} — the pins are already connected (same net). The wire is declared open but copper ties them; remove the connection or the open",
                ap.component, ap.pin, bp.component, bp.pin
            ));
        } else {
            proofs.push(format!(
                "open (disconnected): {}.{} and {}.{} are on separate nets",
                ap.component, ap.pin, bp.component, bp.pin
            ));
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
            let strategy = br.strategy.as_deref().unwrap_or("-");
            format!(
                "{}.{} <-> {}.{} under {}.{} thru {} (node '{}')",
                br.a.component, br.a.pin, br.b.component, br.b.pin,
                br.control.component, br.control.pin, strategy, br.node
            )
        })
        .collect()
}

/// Group the union-find roots into named nets and report unconnected
/// non-nc pins as dangling — the netlist B-half, extracted so
/// derive_netlist stays a coordinator (E7 refactor).
///
/// 2026-09-22: nets carry structural labels only (`N1`, `N2`, …). The
/// emitter derives the readable label from pin classes + derived voltage
/// (GND / V{x} / N#) — names are physics, never author labels.
fn partition_nets(
    groups: &BTreeMap<String, Vec<PinRef>>,
    nc_pins: &std::collections::HashSet<String>,
) -> (Vec<Net>, Vec<String>) {
    let mut nets = Vec::new();
    let mut dangling = Vec::new();
    let mut net_index = 0;
    for (root, members) in groups {
        if members.len() >= 2 {
            net_index += 1;
            nets.push(Net {
                name: format!("N{}", net_index),
                pins: members.clone(),
            });
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

/// Group every declared pin by its final union-find root (deterministic —
/// sorted members, HashMap rule). Pins ascribed a no_connect class are
/// returned separately: they are intentionally unconnected and exempt from
/// the dangling-pin error.
fn group_pins(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
    ds: &mut DisjointSet,
    exempt_pins: &std::collections::HashSet<String>,
) -> (BTreeMap<String, Vec<PinRef>>, std::collections::HashSet<String>) {
    let mut groups: BTreeMap<String, Vec<PinRef>> = BTreeMap::new();
    let mut all_pins = collect_all_pins(instances, type_pins);
    all_pins.sort();
    let mut nc_pins = collect_no_connect_pins(instances, type_info);
    // 2026-09-22 (Slice B): an `unpop` part's pins are open in the absent
    // state — Nc-exempt from the dangling-pin error, exactly like a
    // no_connect-class pin.
    nc_pins.extend(exempt_pins.iter().cloned());
    for p in &all_pins {
        let key = pin_key(&p.component, &p.pin);
        ds.make(key.clone());
        let root = ds.find(&key.clone());
        groups.entry(root).or_default().push(p.clone());
    }
    (groups, nc_pins)
}

/// 2026-09-22 (Slice B): participation facts — `unpop <inst>;` (absent from
/// the BOM, both states verified) and `shortcircuit unpop <inst>: <Type>;`
/// (the present-state short is acknowledged). Returns the unpop instance
/// names, the shortcircuit-acknowledged names, the absent-state exempt pin
/// keys, the populated-part short warnings, and verification notes.
fn collect_participation(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
    Vec<String>,
    Vec<String>,
) {
    let mut unpop = std::collections::HashSet::new();
    let mut shortcircuit = std::collections::HashSet::new();
    let mut exempt = std::collections::HashSet::new();
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    for item in items {
        let (inst_name, acknowledged) = match item {
            TopLevel::Unpop(d) => (d.instance.as_str(), false),
            TopLevel::ShortCircuit(d) => (d.instance.as_str(), true),
            _ => continue,
        };
        let Some(inst) = instances.get(inst_name) else {
            warnings.push(format!(
                "participation fact names undeclared instance '{}' — declare it with `let`",
                inst_name
            ));
            continue;
        };
        if acknowledged {
            shortcircuit.insert(inst_name.to_string());
        }
        unpop.insert(inst_name.to_string());
        // The absent state: every pin of the part is open.
        let Some(ti) = type_pins.get(&inst.type_name) else { continue };
        for (pname, _) in ti {
            exempt.insert(pin_key(inst_name, pname));
        }
        notes.push(format!(
            "participation: '{}' is unpopulated — verified in both the absent (pins open) and \
             present (part conducts) configurations",
            inst_name
        ));
    }
    // 2026-09-22: `shortcircuit` on a POPULATED part is a warning + hint.
    for s in &shortcircuit {
        warnings.push(format!(
            "shortcircuit '{}' names a populated part — a definite short. If the part is meant \
             to stay open, declare `unpop {};`; if the short is intentional on an unpopulated \
             part, write `shortcircuit unpop {};`",
            s, s, s
        ));
    }
    (unpop, shortcircuit, exempt, warnings, notes)
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
    let bus_errors = collect_pin_unions(items, &instances, &type_pins, &mut ds);
    // 2026-09-21 (E14a): node-body intents — body wiring facts union
    // first; drive intents then complete the last open pin.
    let (intent_errors, intent_proofs, conditional_bridges) = {
        let mut ictx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
        collect_intents(&mut ictx, items)
    };

    // Group members by root; sort everything for determinism (HashMap rule).
    // 2026-09-22 (Slice B): participation facts collected first so the
    // absent-state pins (open, Nc-exempt) feed the grouping exemption.
    let (unpop, shortcircuit, exempt_pins, participation_warnings, participation_notes) =
        collect_participation(items, &instances, &type_pins);
    let (groups, nc_pins) =
        group_pins(&instances, &type_pins, &type_info, &mut ds, &exempt_pins);

    let (nets, dangling) = partition_nets(&groups, &nc_pins);

    // Voltage classes need the nets and instances before they move into the
    // result struct. 2026-09-22 (Slice B): acknowledged shorts suppress the
    // present-state shorted-supply error.
    let voltage = derive_voltage(
        items,
        &VoltageInputs { nets: &nets, shortcircuit: &shortcircuit },
        &instances,
        &type_pins,
        &type_info,
    );

    // 2026-09-21 (E13): the decoupling convention runs on the finished
    // netlist — every union is final when it fires.
    let mut ctx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
    let convention_errors = check_decoupling(&mut ctx, &unpop);
    // 2026-09-21 (E7): source-pin budgets run last — they read the
    // derived per-net currents the voltage fixpoint just produced.
    let budget_errors =
        check_budgets(items, &instances, &type_info, &nets, &voltage.net_voltage);
    // 2026-09-22 (ERC contention): two drive-capable non-open-drain pins on
    // one net is a short — checked on the finished netlist.
    let contention_errors =
        check_contention(&nets, &instances, &type_info, &shortcircuit);

    ElectronicsNetlist {
        components: instance_list,
        nets,
        dangling,
        is_electronics: true,
        class_errors,
        convention_errors,
        budget_errors,
        intent_errors,
        intent_proofs,
        conditional_bridges,
        voltage,
        type_info,
        unpop,
        shortcircuit,
        participation_notes,
        participation_warnings,
        contention_errors,
        bus_errors,
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
        type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
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
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };

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
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
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
            type Chip { pin a; pin b; reference "U"; spec Decouple: 100n; };

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
    fn chain_desugars_through_derive_netlist() {
        // A chain's steps desugar to ordinary reactive nodes; derive_netlist
        // sees them exactly as hand-written nodes and derives the wiring from
        // every step's body. Sign-offs (into ...) only accumulate guards —
        // they add no wiring of their own.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type In { spec KicadType: "input"; };
            type Chip { pin vout: Power; pin en: In; pin gnd: Ground; reference "U"; };
            type Led { pin a; pin k; reference "D"; };
            type Conn { pin gnd: Ground; pin vbus: Power; reference "J"; };

            let u1: Chip = Chip { value: "x" };
            let d1: Led = Led { value: "red" };
            let j1: Conn = Conn { value: "y" };

            chain power_up [j1.gnd.voltage == u1.gnd.voltage && d1.k.voltage == u1.gnd.voltage && u1.en.voltage == j1.vbus.voltage] {
                u1.en = true;
                into u1.vout.voltage == 3.3V;
                d1.a = u1.vout;
            };
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        // The final step wires d1.a = u1.out → d1.a and u1.out share a net,
        // and that net is driven (u1.out is Supply-class, CanDrive free).
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("d1.a")),
            "the chain's wiring facts must be proven: {:?}",
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
        assert!(e.contains("thru <Type>"), "{}", e);
    }

    #[test]
    fn relay_with_coil_pair_synthesizes_bridge() {
        // 2026-09-22 (asymmetric switch parts): a relay has a two-pin coil
        // (2 Control pins) + 2 path contacts. Both coil pins must land on
        // the condition net; the contacts bridge the wired pins.
        let src = r#"
            type Control { spec KicadType: "input"; spec Control: true; };
            type Path { spec KicadType: "passive"; spec Switchable: true; };
            type Relay { pin coil1: Control; pin coil2: Control;
                         pin c1: Path; pin c2: Path; reference "K"; tolerance any; };
            type Supply { pin vout; reference "S"; spec KicadType: "power_in"; spec Supply: true; };
            type Lamp { pin a; pin k; reference "L"; };
            type Jack { pin p1; pin p2; reference "J"; };
            let rly: Relay = Relay { value: "5v-coil" };
            let u1: Supply = Supply { value: "ctl" };
            let d1: Lamp = Lamp { value: "lamp" };
            let j1: Jack = Jack { value: "j" };
            let j2: Jack = Jack { value: "j2" };
            node n [d1.k.voltage == j1.p2.voltage && j2.p1.voltage == j1.p1.voltage
                && j2.p2.voltage == j1.p2.voltage] {
                when u1.vout.voltage == 5.0V { d1.a = rly.c1; } thru Relay;
            };
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert_eq!(nl.conditional_bridges.len(), 1, "{:?}", nl.conditional_bridges);
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("2 control pin(s)")),
            "the relay must wire BOTH coil pins: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn open_separate_pins_is_recorded() {
        let src = r#"
            type Conn { pin a; pin b; pin c; pin d; reference "J"; };
            let j1: Conn = Conn { value: "x" };
            node n [j1.a.voltage == j1.b.voltage && j1.c.voltage == j1.d.voltage] {
                open j1.a, j1.c;
            };
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("open (disconnected)") && p.contains("j1.a") && p.contains("j1.c")),
            "open on separate nets must be proven: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn open_connected_pins_is_an_error() {
        // The complement gate (D16 p3b): the pins share a net but the author
        // declares the wire open — copper ties them. Hard error.
        let src = r#"
            type Conn { pin a; pin b; reference "J"; };
            let j1: Conn = Conn { value: "x" };
            node n [j1.a.voltage == j1.b.voltage] {
                open j1.a, j1.b;
            };
        "#;
        let nl = analyze(src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("already connected") && nl.intent_errors[0].contains("open"),
            "{}",
            nl.intent_errors[0]
        );
    }

    // ── 2026-09-22 (plan 2026-09-22-electronics-participation-and-when-law,
    // Slice B): unpop dual-state + shortcircuit acknowledgment ──────────

    #[test]
    fn unpop_exempts_pins_from_dangling() {
        // An unpop part's pins are open in the absent state — no dangling
        // error, and the instance is recorded as unpopulated.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r1: Resistor = Resistor { value: "10k" };
            let j1: Conn = Conn { value: "x" };
            unpop r1;
            node n [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        assert!(nl.unpop.contains("r1"), "r1 must be recorded unpop: {:?}", nl.unpop);
        assert!(
            nl.participation_notes.iter().any(|n| n.contains("unpopulated") && n.contains("r1")),
            "dual-state note must be recorded: {:?}",
            nl.participation_notes
        );
    }

    #[test]
    fn shortcircuit_unpop_suppresses_present_state_short() {
        // Populating the wire shorts a net driven at two voltages; the
        // `shortcircuit unpop` acknowledgment suppresses the present-state
        // shorted-supply error.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Wire { pin a; pin b; reference "W"; tolerance any; };
            let w1: Wire = Wire { value: "0R" };
            shortcircuit unpop w1: Wire;
            txn apply
                [w1.a.voltage == w1.b.voltage && w1.a.voltage == 5.0 && w1.b.voltage == 3.3]
                [w1.a.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            !nl.voltage.violations.iter().any(|v| v.contains("shorted supply")),
            "acknowledged short must not error: {:?}",
            nl.voltage.violations
        );
        assert!(nl.shortcircuit.contains("w1"), "w1 must be recorded: {:?}", nl.shortcircuit);
    }

    #[test]
    fn unacknowledged_present_state_short_still_errors() {
        // Control: WITHOUT the acknowledgment, the same short is a hard
        // error — shortcircuit suppresses only the stated part.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Wire { pin a; pin b; reference "W"; tolerance any; };
            let w1: Wire = Wire { value: "0R" };
            txn apply
                [w1.a.voltage == w1.b.voltage && w1.a.voltage == 5.0 && w1.b.voltage == 3.3]
                [w1.a.voltage >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("shorted supply")),
            "unacknowledged short must error: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn shortcircuit_on_populated_part_warns() {
        // `shortcircuit r1;` (no unpop) on a populated part → warning with
        // the suggest-unpop hint, not an error.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r1: Resistor = Resistor { value: "10k" };
            let j1: Conn = Conn { value: "x" };
            shortcircuit r1;
            node n [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            nl.participation_warnings.iter().any(|w| w.contains("unpop r1;")),
            "populated short must warn with a hint: {:?}",
            nl.participation_warnings
        );
    }

    #[test]
    fn participation_fact_naming_undeclared_instance_warns() {
        let src = r#"
            type Conn { pin p1; pin p2; reference "J"; };
            let j1: Conn = Conn { value: "x" };
            unpop ghost;
            node n [j1.p1.voltage == j1.p2.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            nl.participation_warnings.iter().any(|w| w.contains("undeclared instance") && w.contains("ghost")),
            "undeclared participation name must warn: {:?}",
            nl.participation_warnings
        );
    }

    // ── 2026-09-22 (Slice C): the static when law — X implies Y ────────

    #[test]
    fn type_when_law_propagates_conditional_drive() {
        // A regulator's type-body law: when the input is >= 5V, the output
        // is driven at 3.3V. The law is inherited per instance and the
        // conditional drive joins the net's class.
        let src = r#"
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; tolerance any; };
            type Conn { pin p; pin g; reference "J"; tolerance any; };
            type Regulator { pin vin: Power; pin vout: Power; pin gnd: Power; reference "U"; tolerance any;
                when vin.voltage >= 5V { vout.voltage = 3.3V; }
            }
            let u1: Regulator = Regulator { value: "LM1117" };
            let j1: Conn = Conn { value: "j" };
            let j2: Conn = Conn { value: "j" };
            node n [u1.vin.voltage == j2.p.voltage && u1.gnd.voltage == j1.g.voltage
                    && u1.vout.voltage == j1.p.voltage && u1.vin.voltage == 9.0V
                    && j2.g.voltage == j1.g.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(nl.class_errors.is_empty(), "{:?}", nl.class_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        // The law drives vout at 3.3V under vin >= 5V — no contradiction
        // with the (9V, vin) drive, so no shorted-supply violation.
        assert!(
            !nl.voltage.violations.iter().any(|v| v.contains("shorted supply")),
            "the when-law must not conflict: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn when_law_contradiction_is_a_violation() {
        // The law says when the input is >= 5V, the output is 3.3V. Another
        // contract drives the output net at 5V unconditionally — while the
        // law's guard can hold, that is a contradiction.
        let src = r#"
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; tolerance any; };
            type Conn { pin p; reference "J"; tolerance any; };
            type Regulator { pin vin: Power; pin vout: Power; pin gnd: Power; reference "U"; tolerance any;
                when vin.voltage >= 5V { vout.voltage = 3.3V; }
            }
            let u1: Regulator = Regulator { value: "LM1117" };
            let j1: Conn = Conn { value: "j" };
            node n [u1.vin.voltage == 9.0V && u1.vout.voltage == j1.p.voltage && u1.vout.voltage == 5.0V] { };
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("shorted supply")),
            "a when-law contradicted by an unconditional drive must violate: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn mutually_exclusive_when_law_guards_do_not_conflict() {
        // Two laws with mutually-exclusive guards on the same pin drive it
        // at different voltages — but the guards can never both hold, so no
        // contradiction.
        let src = r#"
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; tolerance any; };
            type Switch { pin sel: Power; pin vout: Power; pin gnd: Power; reference "U"; tolerance any;
                when sel.voltage == 3.3V { vout.voltage = 1.8V; }
                when sel.voltage == 1.8V { vout.voltage = 3.3V; }
            }
            let u1: Switch = Switch { value: "x" };
            node n [u1.sel.voltage == 3.3V && u1.vout.voltage == 1.8V] { };
        "#;
        let nl = analyze(src);
        assert!(
            !nl.voltage.violations.iter().any(|v| v.contains("shorted supply")),
            "mutually-exclusive when-law guards must not conflict: {:?}",
            nl.voltage.violations
        );
    }

    // ── 2026-09-22 (ERC contention, D6/D12): two drive-capable pins ─────

    #[test]
    fn two_io_pins_on_one_net_is_contention() {
        // Two bidirectional (non-open-drain) drive-capable pins sharing a
        // net is contention — a short unless one is wired-AND.
        let src = r#"
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin p1: Io; pin p2: Io; reference "U"; tolerance any; };
            let u1: Mcu = Mcu { value: "u" };
            node n [u1.p1.voltage == u1.p2.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            !nl.contention_errors.is_empty(),
            "two Io pins on one net must be contention: {:?}",
            nl.contention_errors
        );
    }

    #[test]
    fn two_open_drain_pins_may_share_a_net() {
        // IoOd declares WiredAnd — open-drain wired-AND permits sharing.
        let src = r#"
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Mcu { pin p1: IoOd; pin p2: IoOd; reference "U"; tolerance any; };
            let u1: Mcu = Mcu { value: "u" };
            node n [u1.p1.voltage == u1.p2.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            nl.contention_errors.is_empty(),
            "open-drain wired-AND must not contend: {:?}",
            nl.contention_errors
        );
    }

    #[test]
    fn shortcircuit_acknowledged_pair_is_exempt_from_contention() {
        // An acknowledged intentional short is the author's stated intent —
        // exempt from contention, like the shorted-supply check.
        let src = r#"
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Wire { pin a: Io; pin b: Io; reference "W"; tolerance any; };
            let w1: Wire = Wire { value: "0R" };
            shortcircuit unpop w1: Wire;
            node n [w1.a.voltage == w1.b.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            nl.contention_errors.is_empty(),
            "an acknowledged short must be exempt from contention: {:?}",
            nl.contention_errors
        );
    }

    // ── 2026-09-22 (Slice 3): whole-bus equality ────────────────────────

    #[test]
    fn whole_bus_equality_unions_each_element() {
        // u2.gpio[0..=3] == u3.data[0..=3] expands to 4 element unions —
        // each element pair shares a net (inclusive range = 4 elements).
        let src = r#"
            type Chip { pin gpio[4]; pin gnd; reference "U"; tolerance any; };
            type Sensor { pin data[4]; pin gnd; reference "S"; tolerance any; };
            let u2: Chip = Chip { value: "u2" };
            let u3: Sensor = Sensor { value: "u3" };
            node n [u2.gpio[0..=3].voltage == u3.data[0..=3].voltage && u2.gnd.voltage == u3.gnd.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(nl.bus_errors.is_empty(), "{:?}", nl.bus_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        // 4 element nets + 1 gnd net.
        assert_eq!(nl.nets.len(), 5, "4 element pairs + gnd: {:?}", nl.nets);
        // Each element pair shares one net.
        for k in 0..4 {
            assert!(
                nl.nets.iter().any(|n| {
                    n.pins.iter().any(|p| p.component == "u2" && p.pin == format!("gpio[{}]", k))
                        && n.pins.iter().any(|p| p.component == "u3" && p.pin == format!("data[{}]", k))
                }),
                "element {} must share a net: {:?}",
                k,
                nl.nets
            );
        }
    }

    #[test]
    fn whole_bus_length_mismatch_is_an_error() {
        // Half-open [0..3] = 3 elements vs [0..7] = 7 — a hard error.
        let src = r#"
            type Chip { pin gpio[4]; pin gnd; reference "U"; tolerance any; };
            type Sensor { pin data[8]; pin gnd; reference "S"; tolerance any; };
            let u2: Chip = Chip { value: "u2" };
            let u3: Sensor = Sensor { value: "u3" };
            node n [u2.gpio[0..3].voltage == u3.data[0..7].voltage && u2.gnd.voltage == u3.gnd.voltage] { };
        "#;
        let nl = analyze(src);
        assert!(
            !nl.bus_errors.is_empty(),
            "a 3-vs-7 bus equality must error: {:?}",
            nl.bus_errors
        );
        assert!(nl.bus_errors[0].contains("3 elements") && nl.bus_errors[0].contains("7 elements"), "{}", nl.bus_errors[0]);
    }

    // ── 2026-09-23 (E1): bounded instance arrays ────────────────────────

    #[test]
    fn instance_array_elements_wire_through_resolve_pin() {
        // `r[0].a` / `r[1].b` resolve to the expanded element instances;
        // unions form the same nets the hand-unrolled lets would.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r[2]: Resistor = Resistor { value: "4k7" };
            let j1: Conn = Conn { value: "x" };
            node n [
                j1.p1.voltage == r[0].a.voltage && r[0].b.voltage == j1.p2.voltage &&
                j1.p1.voltage == r[1].a.voltage && r[1].b.voltage == j1.p2.voltage
            ] { };
        "#;
        let nl = analyze(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let names: Vec<&str> = nl.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["r[0]", "r[1]", "j1"], "{:?}", names);
        // Two nets: {j1.p1, r[0].a, r[1].a} and {j1.p2, r[0].b, r[1].b}.
        assert_eq!(nl.nets.len(), 2, "{:?}", nl.nets);
        for k in ["r[0]", "r[1]"] {
            assert!(
                nl.nets.iter().any(|n| n.pins.iter().any(|p| p.component == k)),
                "{} missing from nets: {:?}",
                k,
                nl.nets
            );
        }
    }

    #[test]
    fn instance_array_multi_dim_element_resolves() {
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; tolerance any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let m[i:2][j:2]: Resistor = Resistor { value: "x" };
            let j1: Conn = Conn { value: "x" };
            node n [
                j1.p1.voltage == m[1][1].a.voltage && m[1][1].b.voltage == j1.p2.voltage
            ] { };
        "#;
        let nl = analyze(src);
        assert_eq!(nl.components.len(), 5, "{:?}", nl.components);
        // The wired element's pins must not dangle; the other three
        // elements are declared but unwired — every one of their pins
        // MUST dangle (4 elements × 2 pins − 2 wired = 6).
        assert!(
            !nl.dangling.iter().any(|d| d.contains("m[1][1]")),
            "the wired element must not dangle: {:?}",
            nl.dangling
        );
        assert!(
            nl.dangling.iter().any(|d| d.contains("m[0][0]")),
            "unwired elements must dangle: {:?}",
            nl.dangling
        );
        assert_eq!(nl.dangling.len(), 6, "{:?}", nl.dangling);
    }

    #[test]
    fn thru_strategy_narrows_to_type() {
        // Two switches, different types: `via Fet` selects q1.
        let src = MECH_BOARD.replace(
            "let q1: Fet = Fet { value: \"bs170\" };",
            "let q1: Fet = Fet { value: \"bs170\" };
        type Relay { pin coil: Control; pin c1: Path; pin c2: Path; reference \"K\"; };
        let k1: Relay = Relay { value: \"g5le\" };",
        );
        let src = src.replace(
            "            };\n        }",
            "            } thru Fet;\n        }",
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
            "thru Fet must not pick the relay: {:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn thru_with_no_declared_instance_is_an_error() {
        let src = MECH_BOARD.replace(
            "            };\n        }",
            "            } thru Relay;\n        }",
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
    fn mechanism_condition_requires_voltage_literal() {
        // `x = banana` — the RHS must be a voltage literal. This is the
        // validation gate that closed the silent-acceptance hole: before
        // the 2026-09-22 slice, condition_control ignored the non-pin
        // operand and synthesized a bridge anyway.
        let src = MECH_BOARD.replace(
            "u1.gpio0.voltage == 3.3V",
            "u1.gpio0 = banana",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        let e = &nl.intent_errors[0];
        assert!(
            e.contains("voltage comparison") && e.contains("banana"),
            "{}",
            e
        );
        assert!(
            nl.conditional_bridges.is_empty(),
            "invalid condition must not synthesize: {:?}",
            nl.conditional_bridges
        );
    }

    #[test]
    fn mechanism_condition_voltage_field_requires_literal() {
        // `x.voltage == banana` — the voltage FORM but a non-literal RHS
        // is still rejected.
        let src = MECH_BOARD.replace(
            "u1.gpio0.voltage == 3.3V",
            "u1.gpio0.voltage == banana",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("banana"),
            "{}",
            nl.intent_errors[0]
        );
    }

    #[test]
    fn mechanism_condition_pin_needs_voltage_field() {
        // `x = 3.3V` — a literal, but the pin side has no `.voltage`
        // field, so it is not a voltage comparison. Rejected.
        let src = MECH_BOARD.replace(
            "u1.gpio0.voltage == 3.3V",
            "u1.gpio0 = 3.3V",
        );
        let nl = analyze(&src);
        assert_eq!(nl.intent_errors.len(), 1, "{:?}", nl.intent_errors);
        assert!(
            nl.intent_errors[0].contains("voltage comparison"),
            "{}",
            nl.intent_errors[0]
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
