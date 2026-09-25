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
    /// 2026-09-24 (component laws): `spec Name: quantity;` fields — the
    /// structured physics channel. Invalid/missing quantities are absent so a
    /// law consumer reports a missing parameter instead of guessing.
    pub specs: BTreeMap<String, crate::ast::PropertyValue>,
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
    /// `spec Rating: 0.25W;` → Some(watts), `spec Rating: any;` → Some(INFINITY),
    /// no clause → None (the proven-dissipation violation decides).
    pub rating: Option<f64>,
    /// 2026-09-24 (component laws): type-level `spec Resistance` default in
    /// ohms. `None` when the type declares only the dimension or no value.
    pub resistance: Option<f64>,
    /// 2026-09-24 (component laws): the type declares constitutive laws.
    /// Such parts are solved from their law IR and are not part of the
    /// series-graph derivation at all.
    pub has_laws: bool,
    /// 2026-09-24 (component laws): type-level quantity defaults for law
    /// parameters, keyed by lowercase spec name. Dimension-only declarations
    /// are requirements and intentionally absent.
    pub spec_defaults: BTreeMap<String, crate::ast::PropertyValue>,
    /// 2026-09-24 (multi-state laws): the type acknowledges multiple valid
    /// operating states (`spec Bistable: true;`). An ambiguous connected
    /// group may retain states only when every member declares it.
    pub bistable: bool,
    /// 2026-09-24 (SPST modes): declared named modes, sorted by declaration.
    /// Empty for purely guarded-law components.
    pub modes: Vec<String>,
}

/// One derived electrical node.
#[derive(Debug, Clone)]
pub struct Net {
    pub name: String,
    pub pins: Vec<PinRef>,
    /// 2026-09-25 (E14b-7): the author's net label (net<>/stdnet<>) when
    /// the net carries one — the emitter prefers it over the
    /// physics-derived label; the structural name stays the identity.
    pub author_label: Option<String>,
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
    /// 2026-09-24 (component laws, Slice 2): constitutive-law elaboration
    /// errors. Hard diagnostics: an invalid law cannot prove physics.
    pub law_errors: Vec<String>,
    /// 2026-09-24 (component laws, Slice 2): validated per-instance
    /// constitutive laws. Slice 3's DC solver consumes this IR.
    pub component_laws: Vec<crate::analysis::electronics_laws::ComponentLaws>,
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
#[derive(Debug, Clone, Default)]
pub struct VoltageCheck {
    /// Net name → driven voltage class (max of agreeing drives).
    pub net_voltage: BTreeMap<String, f64>,
    /// Net name → derived current class (B4: V/R through series parts).
    pub net_current: BTreeMap<String, f64>,
    /// 2026-09-24 (component laws): branch current per pin from the DC law
    /// solve. Preferred over the net-level heuristic when present.
    pub pin_current: BTreeMap<(String, String), f64>,
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

/// 2026-09-24 (quantities Phase 2): resolve the type-level envelope values
/// from the body metadata — `spec Tolerance: 3.6V;`/`spec Rating: 0.25W;`
/// state the limit, `any` declares unrated (INFINITY), a dimension-only
/// declaration (`spec Tolerance: Volt;`) or no key at all leaves the value
/// absent (the driven-net/unrated checks own that case).
fn envelope_values(
    td: &crate::ast::top::TypeDef,
) -> (Option<f64>, Option<f64>) {
    let tolerance = match td.body.metadata.get("tolerance") {
        Some(crate::ast::PropertyValue::Quantity { si, dimension })
            if *dimension == crate::ast::QuantityDim::Volt => Some(*si),
        Some(crate::ast::PropertyValue::Identifier(v)) if v == "any" => {
            Some(f64::INFINITY)
        }
        _ => None,
    };
    let rating = match td.body.metadata.get("rating") {
        Some(crate::ast::PropertyValue::Quantity { si, dimension })
            if *dimension == crate::ast::QuantityDim::Watt => Some(*si),
        Some(crate::ast::PropertyValue::Identifier(v)) if v == "any" => {
            Some(f64::INFINITY)
        }
        _ => None,
    };
    (tolerance, rating)
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
                // `spec Tolerance: 3.3V;` rates the pins; `spec Tolerance:
                // any;` declares unrated (a decision); absent = unrated (B4
                // wires the driven-net violation for the no-clause case).
                let (tolerance, rating) = envelope_values(td);
                // 2026-09-24 (component laws): a type-level resistance value
                // is the parameter default. A dimension-only declaration
                // (`Identifier("Ohm")`) is not a value.
                let resistance = match td.body.metadata.get("resistance") {
                    Some(crate::ast::PropertyValue::Quantity { si, dimension }) if *dimension == crate::ast::QuantityDim::Ohm => Some(*si),
                    _ => None,
                };
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
                let spec_defaults = td
                    .body
                    .metadata
                    .iter()
                    .filter_map(|(key, value)| match value {
                        crate::ast::PropertyValue::Quantity { .. } => {
                            Some((key.clone(), value.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                // 2026-09-24 (multi-state laws): an acknowledged multi-state
                // component may keep every consistent operating state.
                let bistable = matches!(
                    td.body.metadata.get("bistable"),
                    Some(crate::ast::PropertyValue::Bool(true))
                );
                info.insert(
                    td.name.clone(),
                    TypeInfo { reference_prefix: prefix, pins, pin_classes, tolerance, rating, resistance, has_laws: !td.body.when_laws.is_empty() || !td.body.modes.is_empty(), spec_defaults, bistable, modes: td.body.modes.iter().map(|m| m.name.clone()).collect() },
                );
            }
        }
    }
    (out, info, class_errors)
}

/// Law-pass adapter: expose the already-shared type/pin collector without
/// making the whole electronics module's helpers public (2026-09-24).
pub(crate) fn collect_type_pins_for_laws(
    items: &[TopLevel],
) -> (
    BTreeMap<String, Vec<(String, u64)>>,
    BTreeMap<String, TypeInfo>,
    Vec<String>,
) {
    collect_type_pins(items)
}

/// Law-pass adapter: collect component instances on the same definitions the
/// netlist uses, so law instances cannot diverge from topology instances.
pub(crate) fn collect_instances_for_laws(
    items: &[TopLevel],
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<ComponentInstance> {
    collect_instances(items, type_pins)
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
        let Some(Expr::StructLiteral { type_name: lit_ty, fields, specs }) = expr else { continue };
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
        // Spec payloads are structured quantities (SI + dimension). A malformed
        // spec value is not smuggled into properties — a later law consumer
        // sees it as missing and names the fix.
        let spec_values = collect_spec_quantities(specs);
        out.push(ComponentInstance { name: name.clone(), type_name, properties, specs: spec_values });
    }
    out
}

/// Convert component-literal `spec` entries to structured SI quantities.
/// Known spec names use their canonical metadata key; malformed quantities
/// are absent so a law consumer reports the missing parameter, never guesses.
fn collect_spec_quantities(
    specs: &[(String, Expr)],
) -> BTreeMap<String, crate::ast::PropertyValue> {
    let mut out = BTreeMap::new();
    for (name, expr) in specs {
        let Some((key, quantity)) = spec_quantity(name, expr) else { continue };
        out.insert(key, quantity);
    }
    out
}

/// One spec entry → (storage key, SI quantity). None for a malformed value.
fn spec_quantity(name: &str, expr: &Expr) -> Option<(String, crate::ast::PropertyValue)> {
    let Expr::UnitLiteral { value, unit } = expr else { return None };
    let Some(crate::parser::quantity::UnitSuffix::Explicit { scale, dim }) =
        crate::parser::quantity::parse_unit_suffix(unit)
    else {
        return None;
    };
    let key = crate::parser::spec_name_to_key(name)
        .map(str::to_string)
        .unwrap_or_else(|| name.to_lowercase());
    Some((
        key,
        crate::ast::PropertyValue::Quantity { si: value * scale, dimension: dim },
    ))
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

/// Extract a physical quantity from a numeric expression in `expected`
/// units. Bare numbers remain the compact legacy spelling; canonical
/// full-word units are normalized through the shared suffix parser.
fn extract_quantity(expr: &Expr, expected: crate::ast::QuantityDim) -> Option<f64> {
    match expr {
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => {
            extract_quantity(inner, expected).map(|value| -value)
        }
        Expr::UnitLiteral { value, unit } => {
            crate::parser::quantity::quantity_si(*value, unit, expected).map(|(si, _)| si)
        }
        _ => extract_numeric(expr),
    }
}

/// Extract a voltage value from an expression — V/Volt or bare numeric.
fn extract_voltage(expr: &Expr) -> Option<f64> {
    extract_quantity(expr, crate::ast::QuantityDim::Volt)
}

/// Extract a current value from an expression — A/Amp, mA/mAmp, or bare numeric.
fn extract_current(expr: &Expr) -> Option<f64> {
    extract_quantity(expr, crate::ast::QuantityDim::Amp)
}

/// Extract a resistance value from an expression — Ohm/kOhm or bare numeric.
fn extract_resistance(expr: &Expr) -> Option<f64> {
    extract_quantity(expr, crate::ast::QuantityDim::Ohm)
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
    // Current/power proofs and contract bounds run after the component-law
    // DC solve can add its pin-exact quantities (2026-09-24 Slice 6).
    check
}

/// Inputs to the post-solve proof pass: contract bounds, legacy series
/// derivation, and law-aware power all consume the same finished net index.
struct ProofInputs<'a> {
    items: &'a [TopLevel],
    nets: &'a [Net],
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
    states: &'a std::collections::BTreeSet<String>,
    modes: &'a BTreeMap<String, Vec<String>>,
    assigned_modes: &'a BTreeMap<String, String>,
    unpop: &'a std::collections::HashSet<String>,
}

/// Everything needed after contract drives are classified: DC solve, proofs,
/// and budget roll-up.
struct PostSolveContext<'a> {
    items: &'a [TopLevel],
    nets: &'a [Net],
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
    modes: &'a BTreeMap<String, Vec<String>>,
    unpop: &'a std::collections::HashSet<String>,
}

/// Solve laws, derive electrical quantities, prove bounds, and check budgets.
/// Returns `(law errors, budget errors)`; both are hard at the emitter.
fn post_solve_checks(
    ctx: PostSolveContext<'_>,
    voltage: &mut VoltageCheck,
) -> (Vec<String>, Vec<String>) {
    let (states, law_errors) = solve_law_states(DcInput {
        components: ctx.laws,
        nets: ctx.nets,
        instances: ctx.instances,
        type_pins: ctx.type_pins,
        type_info: ctx.type_info,
        modes: ctx.modes,
        unpop: ctx.unpop,
        drives: &voltage.net_voltage,
    });
    let mut law_all = law_errors;
    law_all.extend(validate_node_mode_states(&ctx, &states));
    let mut budget_all = Vec::new();
    let mut proved_all = voltage.proved.clone();
    // The first deterministic state is the representative emitter view; all
    // states are independently proved and their diagnostics aggregated.
    let mut representative: Option<crate::analysis::electronics_dc::DcSolution> = None;
    for state in states {
        // Start clean per state so solved tolerance errors are not confused
        // with the base drive diagnostics already carried by `voltage`.
        let mut state_check = voltage.clone();
        state_check.violations.clear();
        state_check.net_voltage.extend(state.net_voltage.clone());
        state_check.pin_current.extend(state.pin_current.clone());
        check_tolerance(
            ctx.nets,
            ctx.instances,
            ctx.type_info,
            &state_check.net_voltage,
            &mut state_check.violations,
        );
        derive_electrical_proofs(
            ProofInputs {
                items: ctx.items,
                nets: ctx.nets,
                instances: ctx.instances,
                type_pins: ctx.type_pins,
                type_info: ctx.type_info,
                laws: ctx.laws,
                states: &state.states,
                modes: ctx.modes,
                assigned_modes: &state.modes,
                unpop: ctx.unpop,
            },
            &mut state_check,
        );
        let state_budget = check_budgets(BudgetInputs {
            items: ctx.items,
            instances: ctx.instances,
            type_info: ctx.type_info,
            nets: ctx.nets,
            voltage: &state_check,
            laws: ctx.laws,
            states: &state.states,
        });
        budget_all.extend(state_budget);
        voltage.violations.extend(state_check.violations);
        voltage.net_current.extend(state_check.net_current);
        // Representative view: first solved value wins. Later states are
        // still proved, but do not relabel the schematic.
        for (net, volt) in state_check.net_voltage {
            voltage.net_voltage.entry(net).or_insert(volt);
        }
        proved_all.extend(state_check.proved);
        representative.get_or_insert(state.clone());
    }
    // Record the mandatory absent omission explicitly: an absent state with
    // no branch quantity is a solved participation state, not a vacuous pass.
    for law in ctx.laws {
        if ctx.unpop.contains(&law.instance) {
            proved_all.push(format!(
                "[participation=absent] component-law part '{}' omitted — pins open, no law contribution",
                law.instance
            ));
        }
    }
    if let Some(state) = representative {
        // Merge, never replace: nets outside law groups keep their contract
        // drives, and law groups contribute only their own solved nets.
        voltage.net_voltage.extend(state.net_voltage);
        voltage.pin_current.extend(state.pin_current);
    }
    voltage.proved = proved_all;
    (law_all, budget_all)
}

/// Every node with a mode-selection precondition must match at least one
/// solved global state. Unknown modes/instances are errors even if another
/// predicate would have made the node inapplicable.
fn validate_node_mode_states(
    ctx: &PostSolveContext<'_>,
    states: &[crate::analysis::electronics_dc::DcSolution],
) -> Vec<String> {
    ctx.items
        .iter()
        .filter_map(|item| match item {
            TopLevel::Transaction(t) => validate_one_mode_node(t, states, ctx.modes),
            _ => None,
        })
        .collect()
}

/// Evaluate one node across all states and explain zero-match failures.
fn validate_one_mode_node(
    transaction: &crate::ast::Transaction,
    states: &[crate::analysis::electronics_dc::DcSolution],
    modes: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let mut saw_mode = false;
    let mut matched = false;
    for state in states {
        let evaluated = match eval_mode_predicate(&transaction.contract.pre_condition, modes, &state.modes) {
            Ok(evaluated) => evaluated,
            Err(error) => return Some(format!("[node '{}'] {error}", transaction.name)),
        };
        saw_mode |= evaluated.referenced;
        matched |= evaluated.value;
    }
    if saw_mode && !matched {
        return Some(format!(
            "[node '{}'] its mode precondition matches none of the {} solved operating states — \
             the node is physically unsatisfiable. fix: select a declared mode that the law \
             solver can reach",
            transaction.name,
            states.len()
        ));
    }
    None
}

/// Prefix a diagnostic with its deterministic operating-state labels.
fn state_prefix(states: &std::collections::BTreeSet<String>) -> String {
    if states.is_empty() {
        return String::new();
    }
    format!("[{}] ", states.iter().cloned().collect::<Vec<_>>().join("; "))
}

/// Derive legacy current classes, law-aware power, and prove current bounds.
/// Called after the DC solve so law quantities participate in every proof.
fn derive_electrical_proofs(input: ProofInputs<'_>, check: &mut VoltageCheck) {
    let pin_to_net = pin_net_index(input.nets);
    derive_current(
        &pin_to_net,
        input.instances,
        input.type_info,
        check.net_voltage.clone(),
        check,
    );
    derive_power(&pin_to_net, input.instances, input.type_info, check);
    derive_law_power(
        LawPowerContext {
            laws: input.laws,
            instances: input.instances,
            type_pins: input.type_pins,
            nets: input.nets,
            type_info: input.type_info,
            pin_to_net: &pin_to_net,
            states: input.states,
        },
        check,
    );
    check_current_bounds(
        input.items,
        input.instances,
        input.type_pins,
        &CurrentBoundTables {
            pin_to_net: &pin_to_net,
            states: input.states,
            modes: input.modes,
            assigned: input.assigned_modes,
        },
        check,
    );
    // 2026-09-25 (quantities Phase 4): the absolute-maximum current
    // envelope — unconditional, beside the volt tolerance.
    check_max_current(input.instances, input.type_info, &pin_to_net, check, input.unpop);
}

/// The effective absolute-maximum current envelope for one pin — the
/// instance's derating, then the type's pin-qualified row, then the
/// type's uniform. Amp quantities only; anything else is absent.
fn max_current_for(
    inst: &ComponentInstance,
    ti: &TypeInfo,
    pname: &str,
) -> Option<f64> {
    let pick = |pv: &crate::ast::PropertyValue| match pv {
        crate::ast::PropertyValue::Quantity { si, dimension }
            if *dimension == crate::ast::QuantityDim::Amp =>
        {
            Some(*si)
        }
        _ => None,
    };
    inst.specs
        .get("max_current")
        .and_then(pick)
        .or_else(|| ti.spec_defaults.get(&format!("max_current:{pname}")).and_then(pick))
        .or_else(|| ti.spec_defaults.get("max_current").and_then(pick))
}

/// 2026-09-25 (quantities Phase 4, plan
/// `2026-09-25-quantities-phase4-envelopes.md`): the absolute-maximum
/// current envelope — unconditional, like volts over Tolerance. Law-exact
/// current first, the series fixpoint as fallback. A pin whose current
/// the solver cannot derive stays unproven (the anti-vacuity hard error
/// is reserved for stated bounds); a pin over its envelope is a hard
/// violation.
fn check_max_current(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    pin_to_net: &BTreeMap<(String, String), String>,
    check: &mut VoltageCheck,
    unpop: &std::collections::HashSet<String>,
) {
    // (instance, pin, limit) rows first — flat iteration, no nested loop.
    let rows: Vec<(&&ComponentInstance, &String, f64)> = instances
        .values()
        .filter(|inst| !unpop.contains(&inst.name))
        .filter_map(|inst| {
            let ti = type_info.get(&inst.type_name)?;
            Some(
                ti.pins
                    .iter()
                    .filter_map(|(pname, _)| {
                        max_current_for(inst, ti, pname).map(|limit| (pname, limit))
                    })
                    .map(move |(pname, limit)| (inst, pname, limit)),
            )
        })
        .flatten()
        .collect();
    for (inst, pname, limit) in rows {
        let key = (inst.name.clone(), pname.clone());
        let derived = check
            .pin_current
            .get(&key)
            .copied()
            .or_else(|| pin_to_net.get(&key).and_then(|n| check.net_current.get(n).copied()));
        let Some(derived) = derived else {
            continue;
        };
        if derived.abs() > limit + f64::EPSILON {
            check.violations.push(format!(
                "pin '{}.{}' carries a derived current of {} but its absolute maximum \
                 current is {} — the rating is violated. fix: lower the boundary voltage \
                 or the series resistance, or fit a part rated for the current",
                inst.name,
                pname,
                format_amps(derived.abs()),
                format_amps(limit)
            ));
        } else {
            check.proved.push(format!(
                "I({}.{}) = {} <= {} — absolute maximum",
                inst.name,
                pname,
                format_amps(derived.abs()),
                format_amps(limit)
            ));
        }
    }
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
/// The effective volt tolerance for one pin — the instance's derating,
/// then the type's pin-qualified row, then the type's uniform. Volt
/// quantities only; anything else is absent.
fn tolerance_for(
    inst: &ComponentInstance,
    ti: &TypeInfo,
    pname: &str,
) -> Option<f64> {
    let pick = |pv: &crate::ast::PropertyValue| match pv {
        crate::ast::PropertyValue::Quantity { si, dimension }
            if *dimension == crate::ast::QuantityDim::Volt =>
        {
            Some(*si)
        }
        _ => None,
    };
    inst.specs
        .get("tolerance")
        .and_then(pick)
        .or_else(|| ti.spec_defaults.get(&format!("tolerance:{pname}")).and_then(pick))
        .or_else(|| ti.spec_defaults.get("tolerance").and_then(pick))
        .or(ti.tolerance)
}

/// The effective watt rating for one part — the instance's derating,
/// then the type's uniform.
fn rating_for(inst: &ComponentInstance, ti: &TypeInfo) -> Option<f64> {
    let pick = |pv: &crate::ast::PropertyValue| match pv {
        crate::ast::PropertyValue::Quantity { si, dimension }
            if *dimension == crate::ast::QuantityDim::Watt =>
        {
            Some(*si)
        }
        _ => None,
    };
    inst.specs.get("rating").and_then(pick).or(ti.rating)
}

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
            let Some(tol) = tolerance_for(inst, info, &p.pin) else {
                violations.push(format!(
                    "net '{}' is driven at {} but pin '{}.{}' (of {}) has no tolerance clause — \
                     an unrated pin on a driven net is an undeclared decision. \
                     fix: add `spec Tolerance: <max>V;` (pin-qualified: `spec Tolerance: {}: <max>V;`) \
                     rated for {}, or `spec Tolerance: any;` to declare \
                     the pin unrated on purpose.",
                    net.name, format_volts(class), p.component, p.pin, inst.type_name, p.pin, format_volts(class)
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

/// Collect current-bound obligations from POSTconditions:
/// `[x.pin.current <= B]` (upper) / `[x.pin.current >= B]` (lower).
struct CurrentBound {
    transaction: String,
    precondition: Expr,
    pin: PinRef,
    bound: f64,
    is_upper: bool,
}

fn collect_current_bounds(items: &[TopLevel], instances: &BTreeMap<String, &ComponentInstance>, type_pins: &BTreeMap<String, Vec<(String, u64)>>) -> Vec<CurrentBound> {
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
                out.push(CurrentBound {
                    transaction: t.name.clone(),
                    precondition: t.contract.pre_condition.clone(),
                    pin,
                    bound: lit,
                    is_upper,
                });
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

/// Collect the series parts: two-pin instances whose physics comes from a
/// structured `spec Resistance` or a type-level resistance default. The
/// opaque `value` label is never parsed (annotation-vs-physics doctrine,
/// plan 2026-09-24-retire-legacy-value-physics) — it carries no derivation.
fn collect_series_parts(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    pin_to_net: &BTreeMap<(String, String), String>,
) -> Vec<SeriesPart> {
    let mut parts = Vec::new();
    for inst in instances.values() {
        let Some(info) = type_info.get(&inst.type_name) else { continue };
        if info.pins.len() != 2 || info.has_laws {
            continue;
        };
        // Physics source: the instance's structured spec, then the type-level
        // default. The `value` annotation is carried to KiCad and never read.
        let ohms = match inst.specs.get("resistance") {
            Some(crate::ast::PropertyValue::Quantity { si, dimension })
                if *dimension == crate::ast::QuantityDim::Ohm && *si > 0.0 => Some(*si),
            _ => info.resistance.filter(|r| *r > 0.0),
        };
        let Some(ohms) = ohms else { continue };
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
            raw: format_ohms(ohms),
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
        match rating_for(inst, info) {
            None => check.violations.push(format!(
                "part '{}' ({}, {}) dissipates a derived {} but its type declares no power rating — \
                 an unstated rating on a proven-dissipating part is an undeclared decision. \
                 fix: add `spec Rating: <watts>W;` rated above the derived dissipation, or `spec Rating: any;` \
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

/// Law-bearing instance plus the net/type tables needed for `P = Σ V·I`.
struct LawPowerContext<'a> {
    laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    nets: &'a [Net],
    type_info: &'a BTreeMap<String, TypeInfo>,
    pin_to_net: &'a BTreeMap<(String, String), String>,
    states: &'a std::collections::BTreeSet<String>,
}

/// 2026-09-24 (Slice 6): component-law power proof. A fully solved law part
/// has an operating point at every pin, so absorbed power is `Σ V·I`. A
/// missing pin quantity means the model proves no complete dissipation —
/// nothing is forced, never a guessed zero.
fn derive_law_power(ctx: LawPowerContext<'_>, check: &mut VoltageCheck) {
    let state = state_prefix(ctx.states);
    for law in ctx.laws {
        let Some(inst) = ctx.instances.get(&law.instance) else { continue };
        let Some(info) = ctx.type_info.get(&inst.type_name) else { continue };
        let Some(pins) = ctx.type_pins.get(&inst.type_name) else { continue };
        let quantities = law_power(
            &law.instance,
            pins,
            ctx.pin_to_net,
            &check.net_voltage,
            &check.pin_current,
        );
        let Some((power, proof)) = quantities else { continue; };
        if power <= f64::EPSILON {
            // Zero solved dissipation proves no thermal decision.
            continue;
        }
        match rating_for(inst, info) {
            None => check.violations.push(format!(
                "{state}component-law part '{}' ({}) dissipates a derived {} but its type declares no power \
                 rating — an unstated rating on a proven-dissipating law part is an undeclared \
                 decision. fix: add `spec Rating: <watts>W;` rated above the derived dissipation, or \
                 `spec Rating: any;` to declare the part unrated on purpose.",
                law.instance,
                inst.type_name,
                format_watts(power)
            )),
            Some(rated) if power > rated + f64::EPSILON => check.violations.push(format!(
                "{state}component-law part '{}' ({}) dissipates a derived {} but is rated {} — the solved \
                 operating point exceeds the declared rating. fix: raise the rating to a real part \
                 class, change the law parameters, or lower the boundary drive.",
                law.instance,
                inst.type_name,
                format_watts(power),
                format_watts(rated)
            )),
            Some(rated) => check.proved.push(format!(
                "{state}{} P({}) = {} <= {} rating — component-law DC",
                proof,
                law.instance,
                format_watts(power),
                format_watts(rated)
            )),
        }
    }
}

/// Compute one law part's absorbed power. `None` when any pin lacks a solved
/// voltage/current pair.
fn law_power(
    instance: &str,
    pins: &[(String, u64)],
    pin_to_net: &BTreeMap<(String, String), String>,
    net_voltage: &BTreeMap<String, f64>,
    pin_current: &BTreeMap<(String, String), f64>,
) -> Option<(f64, &'static str)> {
    let mut power = 0.0;
    for (pin, _) in pins {
        let key = (instance.to_string(), pin.clone());
        let net = pin_to_net.get(&key)?;
        let voltage = net_voltage.get(net).copied()?;
        let current = pin_current.get(&key).copied()?;
        power += voltage * current;
    }
    // A rating bounds dissipation; an active law may carry the negative sign.
    Some((power.abs(), "component-law DC:"))
}

/// Catalog of declared named modes per component instance.
fn mode_catalog(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
) -> BTreeMap<String, Vec<String>> {
    instances
        .iter()
        .filter_map(|(instance, inst)| {
            type_info.get(&inst.type_name).and_then(|info| {
                (!info.modes.is_empty()).then(|| (instance.clone(), info.modes.clone()))
            })
        })
        .collect()
}

/// Evaluate a node precondition against one named-mode assignment. Leaves
/// with no mode reference are treated as true for state selection: checking
/// the bound in an extra state is conservative. Non-mode operands under OR
/// cannot be decided here and are rejected rather than widened silently.
fn mode_predicate_holds(
    expr: &Expr,
    transaction: &str,
    modes: &BTreeMap<String, Vec<String>>,
    assigned: &BTreeMap<String, String>,
) -> Result<bool, String> {
    let evaluated = eval_mode_predicate(expr, modes, assigned).map_err(|error| {
        format!("[node '{transaction}'] {error}")
    })?;
    Ok(evaluated.value)
}

/// Boolean value plus whether the predicate referenced a component mode.
#[derive(Debug, Clone, Copy)]
struct ModeEvaluation {
    value: bool,
    referenced: bool,
}

fn eval_mode_predicate(
    expr: &Expr,
    modes: &BTreeMap<String, Vec<String>>,
    assigned: &BTreeMap<String, String>,
) -> Result<ModeEvaluation, String> {
    match expr {
        Expr::Bool(value) => Ok(ModeEvaluation { value: *value, referenced: false }),
        Expr::Identifier(name) => {
            if name == "true" {
                Ok(ModeEvaluation { value: true, referenced: false })
            } else if name == "false" {
                Ok(ModeEvaluation { value: false, referenced: false })
            } else {
                Ok(ModeEvaluation { value: true, referenced: false })
            }
        }
        Expr::Field(base, mode) => {
            let Expr::Identifier(instance) = base.as_ref() else {
                return Ok(ModeEvaluation { value: true, referenced: false });
            };
            let Some(declared) = modes.get(instance) else {
                // Ordinary non-electronics member guards do not select modes.
                return Ok(ModeEvaluation { value: true, referenced: false });
            };
            if !declared.contains(mode) {
                return Err(format!(
                    "state assertion '{instance}.{mode}' names an undeclared mode — declared modes: {}",
                    declared.join(", ")
                ));
            }
            Ok(ModeEvaluation {
                value: assigned.get(instance).is_some_and(|selected| selected == mode),
                referenced: true,
            })
        }
        Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner) => {
            let value = eval_mode_predicate(inner, modes, assigned)?;
            Ok(ModeEvaluation { value: !value.value, referenced: value.referenced })
        }
        Expr::BinaryOp(crate::ast::BinaryOpKind::And, left, right) => {
            combine_mode_predicates(eval_mode_predicate(left, modes, assigned)?, eval_mode_predicate(right, modes, assigned)?, false)
        }
        Expr::BinaryOp(crate::ast::BinaryOpKind::Or, left, right) => {
            combine_mode_predicates(eval_mode_predicate(left, modes, assigned)?, eval_mode_predicate(right, modes, assigned)?, true)
        }
        _ => Ok(ModeEvaluation { value: true, referenced: false }),
    }
}

/// Combine two evaluated predicates and reject an OR whose non-mode side
/// cannot be decided for state filtering.
fn combine_mode_predicates(
    left: ModeEvaluation,
    right: ModeEvaluation,
    is_or: bool,
) -> Result<ModeEvaluation, String> {
    if is_or && (left.referenced ^ right.referenced) {
        return Err(
            "an OR state selector mixes a component mode with a non-mode guard — state applicability is not decidable. fix: put the non-mode guard in a separate condition or use AND".to_string()
        );
    }
    Ok(ModeEvaluation {
        value: if is_or { left.value || right.value } else { left.value && right.value },
        referenced: left.referenced || right.referenced,
    })
}

/// Shared tables for current-bound proofs; keeps the walker under the
/// parameter gate now that state provenance is required.
struct CurrentBoundTables<'a> {
    pin_to_net: &'a BTreeMap<(String, String), String>,
    states: &'a std::collections::BTreeSet<String>,
    modes: &'a BTreeMap<String, Vec<String>>,
    assigned: &'a BTreeMap<String, String>,
}

/// Prove (or violate) the postcondition current bounds against the derived
/// current classes.
fn check_current_bounds(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    tables: &CurrentBoundTables<'_>,
    check: &mut VoltageCheck,
) {
    for bound in collect_current_bounds(items, instances, type_pins) {
        let key = (bound.pin.component.clone(), bound.pin.pin.clone());
        let Some(net_name) = tables.pin_to_net.get(&key) else {
            continue;
        };
        // A law solve is pin-exact; the legacy net-current class is fallback
        // only for programs that have not migrated to component laws.
        let law_current = check.pin_current.get(&key).copied();
        let Some(derived) = law_current.or_else(|| check.net_current.get(net_name).copied())
        else {
            continue;
        };
        let site = CurrentBoundSite {
            pin: &bound.pin,
            net_name,
            derived,
            law_current: law_current.is_some(),
            bound: bound.bound,
            is_upper: bound.is_upper,
            states: tables.states,
        };
        // A node precondition selects its applicable operating states. If it
        // does not hold here, the bound is not evidence in this state.
        match mode_predicate_holds(
            &bound.precondition,
            &bound.transaction,
            tables.modes,
            tables.assigned,
        ) {
            Ok(true) => record_current_bound(site, check),
            Ok(false) => {}
            Err(error) => check.violations.push(error),
        }
    }
}

/// One resolved current bound and its derivation provenance.
struct CurrentBoundSite<'a> {
    pin: &'a PinRef,
    net_name: &'a str,
    derived: f64,
    law_current: bool,
    bound: f64,
    is_upper: bool,
    states: &'a std::collections::BTreeSet<String>,
}

/// Record one proved/violated signed current bound.
fn record_current_bound(site: CurrentBoundSite<'_>, check: &mut VoltageCheck) {
    let holds = if site.is_upper {
        site.derived <= site.bound + f64::EPSILON
    } else {
        site.derived >= site.bound - f64::EPSILON
    };
    let relation = if site.is_upper { "<=" } else { ">=" };
    let provenance = if site.law_current {
        "component-law DC"
    } else {
        "series-current fixpoint"
    };
    let state = state_prefix(site.states);
    if holds {
        check.proved.push(format!(
            "{state}I({}.{}) = {} {} {} — {}",
            site.pin.component,
            site.pin.pin,
            format_amps(site.derived),
            relation,
            format_amps(site.bound),
            provenance
        ));
        return;
    }
    let direction = if site.is_upper { "upper" } else { "lower" };
    let state = state_prefix(site.states);
    check.violations.push(format!(
        "{state}pin '{}.{}' on net '{}' carries a derived current of {} but its postcondition {} \
         bound is {} — the bound is violated by the derived physics. fix: adjust the series \
         resistance/law parameters or the boundary voltage so the solved current satisfies it.",
        site.pin.component,
        site.pin.pin,
        site.net_name,
        format_amps(site.derived),
        direction,
        format_amps(site.bound)
    ));
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

/// Canonical diagnostic spelling for resistance (2026-09-24 ASCII units):
/// compact `R` with an SI prefix; full-word `Ohm` is an equal surface alias.
fn format_ohms(r: f64) -> String {
    let (prefix, scale) = ohm_prefix(r);
    let v = r / scale;
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{}{}R", s, prefix)
}

/// Pick the largest deterministic SI prefix not exceeding `r`.
fn ohm_prefix(r: f64) -> (&'static str, f64) {
    const PREFIXES: [(f64, &'static str); 4] =
        [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1e-3, "m")];
    PREFIXES
        .into_iter()
        .find(|(lower, _)| r >= *lower)
        .map(|(lower, prefix)| (prefix, lower))
        .unwrap_or(("", 1.0))
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
    /// 2026-09-25 (E14b-7): `net<>`/`stdnet<>` attachments per instance —
    /// the supply-membership ladder's declared-intent channel.
    net_names: BTreeMap<String, Vec<NetAttach>>,
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
            net_names: collect_net_names(items),
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

/// The finished-netlist checks, in check order: the E13 decoupling
/// verification (the auto-bridged obligations included — every union is
/// final when it fires) and the ERC contention scan (two drive-capable
/// non-open-drain pins on one net is a short, 2026-09-22). Returned
/// separately — they land in distinct netlist fields.
fn finished_netlist_checks(
    ctx: &mut NetlistContext,
    unpop: &std::collections::HashSet<String>,
    shortcircuit: &std::collections::HashSet<String>,
    nets: &[Net],
) -> (Vec<String>, Vec<String>) {
    let convention = check_decoupling(ctx, unpop);
    let contention = check_contention(nets, ctx.instances, ctx.type_info, shortcircuit);
    (convention, contention)
}

/// Author net labels keyed by FINAL union-find root — resolved after the
/// intent pass, because body wiring can merge nets further (E14b-7).
fn resolve_net_labels(
    bindings: &BTreeMap<String, (String, String)>,
    ds: &mut DisjointSet,
) -> BTreeMap<String, String> {
    bindings
        .iter()
        .map(|(_, (root, label))| (ds.find(root), label.clone()))
        .collect()
}

/// The return-topology bundle (2026-09-25, E14b-6): participation facts
/// plus the forcing outputs — one struct instead of a 7-tuple across the
/// netlist pipeline (FactSink pattern).
struct ReturnTopology {
    unpop: std::collections::HashSet<String>,
    shortcircuit: std::collections::HashSet<String>,
    exempt_pins: std::collections::HashSet<String>,
    participation_warnings: Vec<String>,
    participation_notes: Vec<String>,
    topology_proofs: Vec<String>,
    topology_errors: Vec<String>,
    /// Author net bindings (net<>/stdnet<>): name → (root at bind time,
    /// emitter label). The caller resolves final roots post-intents.
    net_bindings: BTreeMap<String, (String, String)>,
}

/// 2026-09-25 (E14b-6): the return-topology pass, in dependency order —
/// participation facts first (the absent-state set gates both forcing
/// rules and feeds the grouping exemption), then return-net inference
/// (the rail obligation forcing needs), then decoupler auto-bridging.
/// All of it precedes the intent pass so drive completions see final
/// nets. `ds` is mutated in place.
fn force_return_topology(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
    ds: &mut DisjointSet,
) -> ReturnTopology {
    let (unpop, shortcircuit, exempt_pins, participation_warnings, participation_notes) =
        collect_participation(items, instances, type_pins);
    let mut topology_proofs = infer_return_net(instances, type_info, &unpop, ds);
    let mut topology_errors = Vec::new();

    // 2026-09-25 (E14b-7): supply-rail membership by refutation — before
    // bridging, whose bridge test consumes the final supply nets.
    let net_bindings: BTreeMap<String, (String, String)> = {
        let mut mctx = NetlistContext::new(items, ds, type_pins, type_info, instances);
        let (rails, source_pins) = collect_driven_rails_full(items, &mut mctx);
        let (membership_proofs, membership_errors, bindings) =
            infer_rail_membership(&mut mctx, &rails, &source_pins, &unpop);
        topology_proofs.extend(membership_proofs);
        topology_errors.extend(membership_errors);
        bindings
    };
    {
        let mut bctx = NetlistContext::new(items, ds, type_pins, type_info, instances);
        topology_proofs.extend(force_decoupler_bridges(&mut bctx, &unpop, &mut topology_errors));
    }
    ReturnTopology {
        unpop,
        shortcircuit,
        exempt_pins,
        participation_warnings,
        participation_notes,
        topology_proofs,
        topology_errors,
        net_bindings,
    }
}

/// 2026-09-25 (E14b-7): one `net<>`/`stdnet<>` attachment on an instance
/// `let` — an optional pin qualifier, the net name, and whether the name
/// is registry-backed (`stdnet`, carries `spec NetVoltage`) or
/// board-local (`net`, opaque to the compiler, Rule 15).
#[derive(Debug, Clone)]
pub struct NetAttach {
    pub pin: Option<String>,
    pub name: String,
    pub standard: bool,
}

/// Collect the net-name attachments from instance `let` modifiers. The
/// annotation value is the canonical payload the modifier scanner built
/// ("NAME" or "pin:NAME,..."); identifiers cannot contain the separators,
/// so the split is unambiguous.
fn collect_net_names(items: &[TopLevel]) -> BTreeMap<String, Vec<NetAttach>> {
    let mut out: BTreeMap<String, Vec<NetAttach>> = BTreeMap::new();
    // (let name, standard, payload) rows first — flat iteration, no
    // nested loop (one loop per dimension).
    let rows: Vec<(String, bool, String)> = items
        .iter()
        .filter_map(|it| match it {
            TopLevel::Statement(s) => Some(s.as_ref()),
            _ => None,
        })
        .filter_map(|stmt| match stmt {
            Statement::Let { name, modifiers, .. } => Some((name, modifiers)),
            _ => None,
        })
        .flat_map(|(name, modifiers)| modifiers.iter().map(move |ann| (name, ann)))
        .filter_map(|(name, ann)| {
            let standard = match ann.name.as_str() {
                "net" => false,
                "stdnet" => true,
                _ => return None,
            };
            let Some(Expr::Quoted(bytes)) = &ann.value else {
                return None;
            };
            Some((name.clone(), standard, String::from_utf8_lossy(bytes).to_string()))
        })
        .collect();
    for (name, standard, part) in rows
        .iter()
        .flat_map(|(name, standard, payload)| {
            payload.split(',').map(move |p| (name, *standard, p))
        })
    {
        let (pin, net_name) = match part.split_once(':') {
            Some((p, n)) => (Some(p.to_string()), n.to_string()),
            None => (None, part.to_string()),
        };
        out.entry(name.clone()).or_default().push(NetAttach {
            pin,
            name: net_name,
            standard,
        });
    }
    out
}

/// The declared `spec NetVoltage` of a standard net — the registry row is
/// any type whose body declares the key. The compiler reads the declared
/// property, never the name (Rule 15).
fn stdnet_expected_volts(
    type_props: &BTreeMap<String, &std::collections::HashMap<String, crate::ast::PropertyValue>>,
    name: &str,
) -> Option<f64> {
    type_props.get(name).and_then(|m| m.get("net_voltage")).and_then(|pv| match pv {
        crate::ast::PropertyValue::Quantity { si, dimension }
            if *dimension == crate::ast::QuantityDim::Volt =>
        {
            Some(*si)
        }
        _ => None,
    })
}

/// The declared `spec KicadLabel` of a standard net — the emitter label
/// when the bare name is not the spelling the tools expect.
fn stdnet_label(
    type_props: &BTreeMap<String, &std::collections::HashMap<String, crate::ast::PropertyValue>>,
    name: &str,
) -> Option<String> {
    type_props.get(name).and_then(|m| m.get("kicad_label")).and_then(property_string)
}

/// The name bindings of one membership pass — the duplicate-name rule in
/// ONE place (three call sites shared it before, E14b-7).
struct NetBindings<'a> {
    bound: &'a mut BTreeMap<String, (String, String)>,
    errors: &'a mut Vec<String>,
}

impl NetBindings<'_> {
    /// The current bindings, for propagation lookups.
    fn snapshot(&self) -> &BTreeMap<String, (String, String)> {
        self.bound
    }
}

impl NetBindings<'_> {
    /// Bind a net name to a rail root. A name refusing to span two rails
    /// is the whole point: two rails are not one net.
    fn bind(&mut self, name: &str, root: &str, label: String) {
        match self.bound.get(name) {
            Some((existing, _)) if existing != root => {
                self.errors.push(format!(
                    "net name '{}' cannot name two different rails — two rails are not one \
                     net. fix: use distinct names, or state the wire that merges them",
                    name
                ));
            }
            Some(_) => {}
            None => {
                self.bound.insert(name.to_string(), (root.to_string(), label));
            }
        }
    }
}

/// Resolve one attachment's pin: the explicit qualifier, or the sole
/// supply pin for the bare form. An ambiguous bare form names the pins.
fn attach_pin(
    inst_name: &str,
    supplies: &[&(String, u64)],
    att: &NetAttach,
    errors: &mut Vec<String>,
) -> Option<String> {
    let pname = match &att.pin {
        Some(p) => p.clone(),
        None => {
            if supplies.len() != 1 {
                let pins = supplies
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                errors.push(format!(
                    "'{}' declares {} supply pins ({}) — a bare net name is ambiguous. fix: \
                     qualify per pin (`{}<in: VBUS, vout: V3V3>`)",
                    inst_name,
                    supplies.len(),
                    pins,
                    if att.standard { "stdnet" } else { "net" }
                ));
                return None;
            }
            supplies[0].0.clone()
        }
    };
    Some(pname)
}

/// The type-side world of one attachment — a bundle under the parameter
/// gate.
struct AttachCheck<'a> {
    type_props:
        &'a BTreeMap<String, &'a std::collections::HashMap<String, crate::ast::PropertyValue>>,
    ti: &'a TypeInfo,
    inst_name: &'a str,
}

/// One attachment's class + registry validation. Errors state what, why,
/// and the fix; `None` drops the attachment from the pass.
fn validate_attach(
    check: &AttachCheck,
    pname: &str,
    att: &NetAttach,
    errors: &mut Vec<String>,
) -> Option<(String, bool)> {
    let (type_props, ti, inst_name) = (check.type_props, check.ti, check.inst_name);
    let Some(idx) = ti.pins.iter().position(|(n, _)| *n == pname) else {
        errors.push(format!(
            "'{}' has no pin '{}' for the net name '{}'",
            inst_name, pname, att.name
        ));
        return None;
    };
    if ti.pin_classes[idx].return_pin {
        errors.push(format!(
            "'{}.{}' is a return-class pin — the return net is class-inferred and takes no \
             net name",
            inst_name, pname
        ));
        return None;
    }
    if !ti.pin_classes[idx].supply {
        errors.push(format!(
            "'{}.{}' is not a supply-class pin — net names attach to supply membership only",
            inst_name, pname
        ));
        return None;
    }
    if att.standard && stdnet_expected_volts(type_props, &att.name).is_none() {
        errors.push(format!(
            "no standard net '{}' is declared — declare a type '{}' with `spec NetVoltage` \
             (and optional `spec KicadLabel`), or use the board-local form net<{}>",
            att.name, att.name, att.name
        ));
        return None;
    }
    Some((att.name.clone(), att.standard))
}

/// Expand + validate the `net<>`/`stdnet<>` attachments of each populated
/// instance into per-pin entries.
fn expand_net_attachments(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    type_props: &BTreeMap<String, &std::collections::HashMap<String, crate::ast::PropertyValue>>,
    net_names: &BTreeMap<String, Vec<NetAttach>>,
    unpop: &std::collections::HashSet<String>,
) -> (BTreeMap<(String, String), (String, bool)>, Vec<String>) {
    let mut named = BTreeMap::new();
    let mut errors = Vec::new();
    let rows: Vec<(&&ComponentInstance, &TypeInfo, &Vec<NetAttach>)> = instances
        .values()
        .filter(|inst| !unpop.contains(&inst.name))
        .filter_map(|inst| Some((inst, type_info.get(&inst.type_name)?, net_names.get(&inst.name)?)))
        .collect();
    let attach_rows: Vec<(&&ComponentInstance, &TypeInfo, &NetAttach)> = rows
        .into_iter()
        .flat_map(|(inst, ti, attaches)| {
            attaches.iter().map(move |att| (inst, ti, att))
        })
        .collect();
    for (inst, ti, att) in attach_rows {
        let supplies = rail_pins(ti, false);
        let Some(pname) = attach_pin(&inst.name, &supplies, att, &mut errors) else {
            continue;
        };
        let check = AttachCheck { type_props, ti, inst_name: &inst.name };
        let Some(entry) = validate_attach(&check, &pname, att, &mut errors) else {
            continue;
        };
        let key = (inst.name.clone(), pname);
        if let Some((existing, _)) = named.get(&key) {
            errors.push(format!(
                "'{}.{}' carries two net names ('{}' and '{}') — one pin, one net name",
                inst.name, key.1, existing, entry.0
            ));
            continue;
        }
        named.insert(key, entry);
    }
    (named, errors)
}

/// Bind names through connected pins first — a drive-fact source pin
/// (j1.vbus) IS its rail; a pin pre-wired by a guard equality sits on a
/// final net. Both bind their name to that net's root.
fn bind_connected_names(
    ctx: &mut NetlistContext,
    unpop: &std::collections::HashSet<String>,
    named: &BTreeMap<(String, String), (String, bool)>,
    bindings: &mut NetBindings,
) {
    let instances = ctx.instances;
    let type_info = ctx.type_info;
    let type_props = ctx.type_props.clone();
    let rows: Vec<(&&ComponentInstance, &(String, u64))> = instances
        .values()
        .filter(|inst| !unpop.contains(&inst.name))
        .filter_map(|inst| {
            let ti = type_info.get(&inst.type_name)?;
            Some((inst, rail_pins(ti, false)))
        })
        .flat_map(|(inst, pins)| pins.into_iter().map(move |p| (inst, p)))
        .collect();
    for (inst, (pname, _)) in rows {
        let key = pin_key(&inst.name, pname);
        let Some(att) = named.get(&(inst.name.clone(), pname.clone())) else {
            continue;
        };
        if !pin_connected(ctx.ds, &key) {
            continue;
        }
        let root = ctx.ds.find(&key);
        let label = stdnet_label(&type_props, &att.0).unwrap_or_else(|| att.0.clone());
        bindings.bind(&att.0, &root, label);
    }
}

/// The membership obligations of one pass — flat list, no nested loop
/// (the pass is inherently O(instances × pins)).
fn membership_obligations(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    named: &BTreeMap<(String, String), (String, bool)>,
    unpop: &std::collections::HashSet<String>,
) -> Vec<(String, String, Option<f64>, Option<(String, bool)>)> {
    let mut obligations: Vec<(String, String, Option<f64>, Option<(String, bool)>)> = Vec::new();
    for inst in instances.values() {
        if unpop.contains(&inst.name) {
            continue;
        }
        let Some(ti) = type_info.get(&inst.type_name) else {
            continue;
        };
        obligations.extend(rail_pins(ti, false).iter().map(|(n, _)| {
            (
                inst.name.clone(),
                n.clone(),
                ti.tolerance,
                named.get(&(inst.name.clone(), n.clone())).cloned(),
            )
        }));
    }
    obligations
}

/// Resolve one obligation through the ladder: expectation filter →
/// tolerance refutation → propagation toward bound-named rails →
/// enumerated ambiguity.
/// The ladder's fix text per attachment shape.
fn membership_fix(attachment: &Option<(String, bool)>) -> String {
    match attachment {
        Some((name, true)) => format!("correct the {} expectation or add the drive", name),
        Some((name, false)) => format!("state the wire, or correct the {} name", name),
        None => "state the wire, or name the intended net (net<name> / stdnet<NAME>)".to_string(),
    }
}

/// Why a pin has no rail to join — the expectation names itself when the
/// admission failed on declared volts, else the tolerance.
fn no_rail_why(
    attachment: &Option<(String, bool)>,
    expectation: Option<f64>,
    rated: &str,
) -> String {
    match expectation {
        Some(exp) => format!(
            "{} expects a {}-driven rail",
            attachment.as_ref().map(|(n, _)| n.as_str()).unwrap_or(""),
            format_volts(exp)
        ),
        None => format!("no driven rail is within its tolerance ({})", rated),
    }
}

/// Why the unique survivor is the answer — a declared standard net, or
/// plain tolerance refutation.
fn unique_membership_why(attachment: &Option<(String, bool)>) -> String {
    attachment
        .as_ref()
        .filter(|(_, standard)| *standard)
        .map(|(n, _)| format!("standard net {} drives it", n))
        .unwrap_or_else(|| "the only driven rail within tolerance".to_string())
}

/// The driven-rail list as it appears in diagnostics ("3.3 V@root, ...").
fn membership_drive_list(rails: &BTreeMap<String, f64>) -> String {
    rails
        .iter()
        .map(|(r, v)| format!("{}@{}", format_volts(*v), r))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The rails this obligation may join: within pin tolerance, and — when a
/// stdnet expectation is present — driven at exactly the expected volts.
fn membership_candidates(
    rails: &BTreeMap<String, f64>,
    tolerance: Option<f64>,
    expectation: Option<f64>,
) -> Vec<(String, f64)> {
    rails
        .iter()
        .filter(|(_, v)| tolerance.map_or(true, |tol| **v <= tol))
        .filter(|(_, v)| expectation.map_or(true, |exp| (**v - exp).abs() < f64::EPSILON))
        .map(|(r, v)| (r.clone(), *v))
        .collect()
}

fn resolve_one_membership(
    ctx: &mut NetlistContext,
    rails: &BTreeMap<String, f64>,
    source_pins: &std::collections::HashSet<String>,
    bindings: &mut NetBindings,
    obligation: (String, String, Option<f64>, Option<(String, bool)>),
) -> Option<String> {
    let (inst, pname, tolerance, attachment) = obligation;
    let key = pin_key(&inst, &pname);
    if pin_connected(ctx.ds, &key) || source_pins.contains(&key) {
        return None;
    }
    let expectation = attachment
        .as_ref()
        .filter(|(_, standard)| *standard)
        .and_then(|(name, _)| stdnet_expected_volts(&ctx.type_props.clone(), name));
    let rated = match tolerance {
        None => "unrated".to_string(),
        Some(v) => format!("<= {}", format_volts(v)),
    };
    let drive_list = membership_drive_list(rails);
    let candidates = membership_candidates(rails, tolerance, expectation);
    let fix = membership_fix(&attachment);
    match candidates.len() {
        0 => {
            let why = no_rail_why(&attachment, expectation, &rated);
            bindings.errors.push(format!(
                "supply pin '{}.{}' has no rail to join: {}; driven rails are {}. fix: {}",
                inst, pname, why, drive_list, fix
            ));
            None
        }
        1 => {
            let (root, v) = &candidates[0];
            ctx.ds.make(key.clone());
            ctx.ds.union(&key, root);
            let proof = format!(
                "membership inferred: {}.{} joins the {} rail ({}; rated {})",
                inst,
                pname,
                format_volts(*v),
                unique_membership_why(&attachment),
                rated
            );
            if let Some((name, _)) = &attachment {
                let label = stdnet_label(&ctx.type_props.clone(), name)
                    .unwrap_or_else(|| name.clone());
                bindings.bind(name, root, label);
            }
            Some(proof)
        }
        n => {
            // Rung 3: propagation — candidates on a bound-named rail.
            let named_candidates: std::collections::BTreeSet<String> = candidates
                .iter()
                .filter_map(|(r, _)| {
                    bindings
                        .snapshot()
                        .iter()
                        .find_map(|(name, (root, _))| (root == r).then_some(name.clone()))
                })
                .collect();
            match named_candidates.len() {
                1 => {
                    let name = named_candidates.iter().next().unwrap();
                    let (root, _) = bindings.snapshot()[name].clone();
                    ctx.ds.make(key.clone());
                    ctx.ds.union(&key, &root);
                    if let Some((own, _)) = &attachment {
                        let label = stdnet_label(&ctx.type_props.clone(), own)
                            .unwrap_or_else(|| own.clone());
                        bindings.bind(own, &root, label);
                    }
                    Some(format!(
                        "membership propagated: {}.{} joins {} (the only bound-named rail \
                         among its candidates; rated {})",
                        inst, pname, name, rated
                    ))
                }
                _ => {
                    let cands = candidates
                        .iter()
                        .map(|(r, v)| format!("{}@{}", format_volts(*v), r))
                        .collect::<Vec<_>>()
                        .join(", ");
                    bindings.errors.push(format!(
                        "supply pin '{}.{}' is membership-ambiguous: {} rails are within its \
                         tolerance ({}) and no single bound net name decides among them. \
                         fix: name the intended net (net<name> / stdnet<NAME>)",
                        inst, pname, n, cands
                    ));
                    None
                }
            }
        }
    }
}

/// 2026-09-25 (E14b-7, plan `2026-09-25-ebv-e14b-rail-membership.md`):
/// supply-rail membership — the full ladder. Rung 1: a `stdnet<>`
/// expectation filters candidates to rails driven at the declared
/// `spec NetVoltage`; expectations constrain, they never drive. Rung 2:
/// pin tolerance refutes; a unique survivor unions silently (proof line,
/// D3 provenance). Rung 3: propagation — an ambiguous pin with exactly
/// one bound-named candidate rail joins it. Rung 4: residual ambiguity
/// is a hard diagnostic enumerating the candidates and naming the
/// keywords. Rail source pins are exempt (a driven pin defines its
/// rail). Names bind to rails only through physics-forced pins; the
/// compiler never reads a name as physics (Rule 15). Returns the wiring
/// proofs, the errors, and the name→(root, label) bindings.
fn infer_rail_membership(
    ctx: &mut NetlistContext,
    rails: &BTreeMap<String, f64>,
    source_pins: &std::collections::HashSet<String>,
    unpop: &std::collections::HashSet<String>,
) -> (
    Vec<String>,
    Vec<String>,
    BTreeMap<String, (String, String)>,
) {
    let instances = ctx.instances;
    let type_info = ctx.type_info;
    let type_props = ctx.type_props.clone();
    let net_names = ctx.net_names.clone();
    let mut proofs = Vec::new();
    let mut errors = Vec::new();
    let mut bound: BTreeMap<String, (String, String)> = BTreeMap::new();
    let (named, expand_errors) =
        expand_net_attachments(instances, type_info, &type_props, &net_names, unpop);
    errors.extend(expand_errors);
    {
        let mut bindings = NetBindings { bound: &mut bound, errors: &mut errors };
        bind_connected_names(ctx, unpop, &named, &mut bindings);
        // A rail's drive-fact source pin IS the rail — its name binds
        // there even though nothing wires it (the plan's forced-pin list
        // includes the source).
        for (inst, pname, _, attachment) in
            membership_obligations(instances, type_info, &named, unpop)
        {
            let key = pin_key(&inst, &pname);
            let (true, Some((name, _))) = (source_pins.contains(&key), attachment) else {
                continue;
            };
            let root = ctx.ds.find(&key);
            let label = stdnet_label(&type_props, &name).unwrap_or_else(|| name.clone());
            bindings.bind(&name, &root, label);
        }
        for obligation in membership_obligations(instances, type_info, &named, unpop) {
            if let Some(proof) =
                resolve_one_membership(ctx, rails, source_pins, &mut bindings, obligation)
            {
                proofs.push(proof);
            }
        }
    }
    (proofs, errors, bound)
}

/// 2026-09-25 (E14b-6, design-record gate delta item 1): return-net
/// inference — every pin whose class declares `spec Return: true` unions
/// into the board's single return net. D6 class semantics, not a choice
/// among candidates: there is nothing to enumerate, so no D13 ambiguity
/// arises. An author needing isolated returns does not ascribe the Return
/// class — split returns stay ordinary explicit equalities. Unpopulated
/// instances contribute no copper. Returns wiring-report provenance.
fn infer_return_net(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    unpop: &std::collections::HashSet<String>,
    ds: &mut DisjointSet,
) -> Vec<String> {
    // Collect (instance, pin) members first — flat iteration, no nested
    // loop (the pass is inherently O(instances × pins)).
    let members: Vec<(String, String)> = instances
        .iter()
        .filter(|(name, _)| !unpop.contains(*name))
        .filter_map(|(name, inst)| {
            let ti = type_info.get(&inst.type_name)?;
            Some(
                rail_pins(ti, true)
                    .iter()
                    .map(|(n, _)| (name.clone(), n.clone()))
                    .collect::<Vec<(String, String)>>(),
            )
        })
        .flatten()
        .collect();
    let mut proofs = Vec::new();
    let mut base: Option<String> = None;
    for (name, pname) in members {
        let key = pin_key(&name, &pname);
        ds.make(key.clone());
        match &base {
            None => base = Some(ds.find(&key)),
            Some(b) => ds.union(&key, b),
        }
        proofs.push(format!(
            "return net: {}.{} joined the board return net (class declares spec Return)",
            name, pname
        ));
    }
    proofs
}

/// The board return net's root — the return-class pin of the first
/// populated instance declaring one (infer_return_net unions them all, so
/// any member's root identifies the net).
fn populated_return_root(
    ctx: &mut NetlistContext,
    unpop: &std::collections::HashSet<String>,
) -> Option<String> {
    let instances = ctx.instances;
    let type_info = ctx.type_info;
    for inst in instances.values() {
        if unpop.contains(&inst.name) {
            continue;
        }
        let Some(ti) = type_info.get(&inst.type_name) else {
            continue;
        };
        if let Some((pname, _)) = rail_pins(ti, true).first() {
            let key = pin_key(&inst.name, pname);
            ctx.ds.make(key.clone());
            return Some(ctx.ds.find(&key));
        }
    }
    None
}

/// Whether the type declares `spec Decoupler: true`.
fn is_decoupler_type(
    type_props: &BTreeMap<String, &std::collections::HashMap<String, crate::ast::PropertyValue>>,
    type_name: &str,
) -> bool {
    type_props
        .get(type_name)
        .and_then(|m| m.get("decoupler"))
        .and_then(property_bool)
        .unwrap_or(false)
}

/// Whether the type is auto-bridgeable: exactly two connectable (non-NC)
/// pins, so the return/supply assignment is forced up to symmetry.
fn auto_bridgeable_pins(
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
    type_name: &str,
) -> Option<Vec<String>> {
    let pins = type_pins.get(type_name)?;
    let ti = type_info.get(type_name)?;
    let mut names: Vec<String> = pins
        .iter()
        .enumerate()
        .filter(|(i, _)| !ti.pin_classes[*i].no_connect)
        .map(|(_, (n, _))| n.clone())
        .collect();
    if names.len() != 2 {
        return None;
    }
    names.sort();
    Some(names)
}

/// 2026-09-25 (E14b-6): decoupler auto-bridging — the E13 decoupling
/// convention as a forcing rule. Each supply-class pin of a populated
/// `spec Decouple` instance whose net has no bridging decoupler yet takes
/// the next fully-free populated two-pin `spec Decoupler` part: pin[0]
/// joins the return net, pin[1] joins the supply pin's net. The pins are
/// symmetric, so the pick is immaterial (deterministic, D13). Parts with
/// any other connectable-pin count are never auto-wired; impossible
/// obligations fall through to check_decoupling's what/why/fix diagnostic.
/// A demanded bridge with no return net anywhere is a hard error here.
fn force_decoupler_bridges(
    ctx: &mut NetlistContext,
    unpop: &std::collections::HashSet<String>,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut proofs = Vec::new();
    let instances = ctx.instances;
    let type_info = ctx.type_info;
    // Owned map of shared bags — cloned so the loop can hold it across
    // `bridges_rail`'s mutable context borrow (the values are `&HashMap`).
    let type_props = ctx.type_props.clone();
    let type_pins = ctx.type_pins;
    let Some(ret) = populated_return_root(ctx, unpop) else {
        // D13: a demanded bridge with no return net is a hard error. Only
        // an ENUMERABLE obligation demands it — a Decouple type without a
        // supply-class pin has none, and check_decoupling already reports
        // that more specific defect.
        let demanded = instances.values().any(|c| {
            !unpop.contains(&c.name)
                && type_props
                    .get(&c.type_name)
                    .map(|m| m.contains_key("decouple"))
                    .unwrap_or(false)
                && type_info
                    .get(&c.type_name)
                    .map(|ti| !rail_pins(ti, false).is_empty())
                    .unwrap_or(false)
        });
        if demanded {
            errors.push(
                "decoupling obligation has no return net: no populated instance declares a \
                 Return-class pin, so a decoupling part has nothing to bridge to. fix: ascribe a \
                 return class (e.g. `: Ground`) to a rail pin of the supplied type, or state the \
                 supply pin's attachment explicitly"
                    .to_string(),
            );
        }
        return proofs;
    };
    // 2026-09-25 (E14b-6): only `spec Decoupler` parts may satisfy the
    // bridge test — the supply instance itself always spans its own
    // supply/return pins and must never count (check_decoupling filters
    // the same way).
    let decouplers: Vec<&ComponentInstance> = instances
        .values()
        .copied()
        .filter(|c| !unpop.contains(&c.name) && is_decoupler_type(&type_props, &c.type_name))
        .collect();
    let mut free_caps: Vec<&ComponentInstance> = decouplers
        .iter()
        .copied()
        .filter(|c| pins_free(ctx, c))
        .collect();
    let mut state = BridgeState {
        decouplers,
        free_caps,
        type_props,
        type_info,
        type_pins,
        ret,
    };
    // Collect (instance, supply-pin) obligations first — flat iteration,
    // no nested loop (the pass is inherently O(instances × pins)).
    let mut obligations: Vec<(&ComponentInstance, String)> = Vec::new();
    for inst in instances.values() {
        if unpop.contains(&inst.name) {
            continue;
        }
        let declares = state
            .type_props
            .get(&inst.type_name)
            .map(|m| m.contains_key("decouple"))
            .unwrap_or(false);
        if !declares {
            continue;
        }
        let Some(ti) = state.type_info.get(&inst.type_name) else {
            continue;
        };
        obligations.extend(rail_pins(ti, false).iter().map(|(n, _)| (*inst, n.clone())));
    }
    for (inst, pname) in &obligations {
        if let Some(proof) = bridge_one_supply_pin(ctx, &mut state, inst, pname) {
            proofs.push(proof);
        }
    }
    proofs
}

/// Working set for the auto-bridging pass — one bundle instead of a
/// parameter ladder (FactSink pattern). `type_props` is an owned map of
/// shared bags so it can be held across `bridges_rail`'s mutable context
/// borrow.
struct BridgeState<'a> {
    decouplers: Vec<&'a ComponentInstance>,
    free_caps: Vec<&'a ComponentInstance>,
    type_props: BTreeMap<String, &'a std::collections::HashMap<String, crate::ast::PropertyValue>>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    ret: String,
}

/// Bridge one supply pin when its net lacks a decoupler: consume the next
/// free two-pin `spec Decoupler` part — return side to the return net,
/// supply side to the pin's net. `None` = already bridged or no free part
/// (the E13 check reports the latter).
fn bridge_one_supply_pin(
    ctx: &mut NetlistContext,
    state: &mut BridgeState,
    inst: &ComponentInstance,
    pname: &str,
) -> Option<String> {
    let s_key = pin_key(&inst.name, pname);
    ctx.ds.make(s_key.clone());
    let s_root = ctx.ds.find(&s_key);
    if state
        .decouplers
        .iter()
        .any(|cap| bridges_rail(ctx, cap, &s_root, &state.ret))
    {
        return None;
    }
    let idx = state.free_caps.iter().position(|cap| {
        auto_bridgeable_pins(state.type_pins, state.type_info, &cap.type_name).is_some()
    })?;
    let cap = state.free_caps.remove(idx);
    let names = auto_bridgeable_pins(state.type_pins, state.type_info, &cap.type_name)?;
    let (r_key, s_cap_key) = (pin_key(&cap.name, &names[0]), pin_key(&cap.name, &names[1]));
    ctx.ds.make(r_key.clone());
    ctx.ds.make(s_cap_key.clone());
    ctx.ds.union(&r_key, &state.ret);
    ctx.ds.union(&s_cap_key, &s_root);
    Some(format!(
        "decoupling convention: {} auto-bridged ({} <-> return net, {} <-> {}.{})",
        cap.name, names[0], names[1], inst.name, pname
    ))
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

/// Budget inputs after the law solve: branch quantities plus the legacy
/// series fallback (2026-09-24 Slice 6).
struct BudgetInputs<'a> {
    items: &'a [TopLevel],
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    nets: &'a [Net],
    voltage: &'a VoltageCheck,
    laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
    states: &'a std::collections::BTreeSet<String>,
}

/// 2026-09-21 (E7, design record D4): source-pin budgets — `budget
/// u1.out <= 250mA;` caps the derived current draw across the pin's net.
/// The roll-up is the KCL boundary sum over the B4-derived part graph or the
/// component-law operating point; nets with no derived draw pass vacuously —
/// nothing provable flows. Black-box draws (IC internals) are not yet
/// derivable; the intent machinery owns that later. Violations are hard
/// budget_errors.
fn check_budgets(input: BudgetInputs<'_>) -> Vec<String> {
    // resolve_pin wants the (name, number) table; TypeInfo.pins is the
    // same data, number-sorted — derive the view locally.
    let type_pins: BTreeMap<String, Vec<(String, u64)>> = input
        .type_info
        .iter()
        .map(|(name, ti)| (name.clone(), ti.pins.clone()))
        .collect();
    let mut errors = Vec::new();
    if !input
        .items
        .iter()
        .any(|i| matches!(i, TopLevel::Budget(_)))
    {
        return errors;
    }
    // The part graph, rebuilt from the finished nets (cheap at board
    // scale; the B4 passes own the incremental machinery).
    let mut pin_to_net: BTreeMap<(String, String), String> = BTreeMap::new();
    for net in input.nets {
        for p in &net.pins {
            pin_to_net.insert((p.component.clone(), p.pin.clone()), net.name.clone());
        }
    }
    let parts = collect_series_parts(input.instances, input.type_info, &pin_to_net);
    for item in input.items {
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
        let Some(pin) = resolve_pin(pin_expr, input.instances, &type_pins) else {
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
        let Some(net) = net_of(input.nets, &key) else { continue };
        let law_draw = law_net_draw(net, input.laws, input.voltage);
        let Some((drawn, provenance)) = law_draw
            .map(|draw| (draw, "component-law DC"))
            .or_else(|| {
                net_draw(&net.name, &parts, &input.voltage.net_voltage)
                    .map(|draw| (draw, "B4 fixpoint"))
            })
        else {
            continue; // no derived draw on this net — nothing to cap
        };
        if drawn > limit {
            errors.push(format!(
                "{}budget exceeded: '{}.{}' allows {} but its net draws {} — the derived sum over \
                 the net ({provenance}) is past the stated limit. Raise the budget or reduce the draw",
                state_prefix(input.states),
                pin.component,
                pin.pin,
                format_amps(limit),
                format_amps(drawn)
            ));
        }
    }
    errors
}

/// Sum |I| over law-bearing pins attached to one net. Pin-exact law currents
/// are signed at the component boundary; a supply draw is magnitude.
fn law_net_draw(
    net: &Net,
    laws: &[crate::analysis::electronics_laws::ComponentLaws],
    voltage: &VoltageCheck,
) -> Option<f64> {
    let law_components: std::collections::HashSet<&str> = laws
        .iter()
        .map(|law| law.instance.as_str())
        .collect();
    let mut total = 0.0;
    let mut any = false;
    for pin in &net.pins {
        if !law_components.contains(pin.component.as_str()) {
            continue;
        }
        let Some(current) = voltage.pin_current.get(&(pin.component.clone(), pin.pin.clone())) else {
            continue;
        };
        total += current.abs();
        any = true;
    }
    any.then_some(total)
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

    /// 2026-09-25 (E14b-6): the single-free-pin low-path completion —
    /// the part's wired sibling sits on the obligation net (p1-pre-wired)
    /// or on the return rail; the free pin completes the opposite side.
    /// The sibling check MUST precede any root-side union — unioning the
    /// free pin to the obligation net first would short the net to return.
    fn complete_single_free_low(
        &mut self,
        w: &LowHoldWire,
        part: &str,
        pins: &[(String, u64)],
        pa: &(String, u64),
    ) {
        let pa_key = pin_key(part, &pa.0);
        let sibling_on_ret = pins
            .iter()
            .any(|(n, _)| n != &pa.0 && self.ctx.ds.find(&pin_key(part, n)) == w.ret);
        let sibling_on_root = pins
            .iter()
            .any(|(n, _)| n != &pa.0 && self.ctx.ds.find(&pin_key(part, n)) == w.root);
        self.ctx.ds.make(pa_key.clone());
        if sibling_on_ret {
            self.ctx.ds.make(w.root.clone());
            self.ctx.ds.union(&pa_key, &w.root);
            self.proofs.push(format!(
                "low-hold forced by '<= {}V' (node '{}'): {}.{} <-> net, other path pin on \
                 return rail — obligation satisfied",
                w.vmax,
                w.nodes.join(", "),
                part,
                pa.0
            ));
            return;
        }
        if sibling_on_root {
            self.ctx.ds.make(w.ret.clone());
            self.ctx.ds.union(&pa_key, &w.ret);
            self.proofs.push(format!(
                "low-hold forced by '<= {}V' (node '{}'): {}.{} <-> return rail, sibling path \
                 pin already on the net — obligation satisfied",
                w.vmax,
                w.nodes.join(", "),
                part,
                pa.0
            ));
            return;
        }
        self.errors.push(format!(
            "low-hold obligation '<= {}V' (node '{}'): {}.{} has only one free switchable pin \
             and its other path pin is neither on the net nor on the return rail — it cannot \
             complete the low path. Wire the low path explicitly",
            w.vmax,
            w.nodes.join(", "),
            part,
            pa.0
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
        if free.len() == 1 {
            self.complete_single_free_low(&w, part, &pins, pa);
            return;
        }
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
        // free.len() >= 2 here — the single-free case returned above.
        let pb = free[1];
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
    collect_driven_rails_full(items, ctx).0
}

/// The driven supply rails, two ways: root → volts (the forcing maps) and
/// the SOURCE pin keys themselves — a driven pin defines its rail and is
/// exempt from membership inference (2026-09-25, E14b-7: a rail's source
/// cannot "join" another rail).
fn collect_driven_rails_full(
    items: &[TopLevel],
    ctx: &mut NetlistContext,
) -> (BTreeMap<String, f64>, std::collections::HashSet<String>) {
    let mut rails: BTreeMap<String, f64> = BTreeMap::new();
    let mut source_pins: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        let key = pin_key(&pin.component, &pin.pin);
        source_pins.insert(key.clone());
        let root = ctx.ds.find(&key);
        let entry = rails.entry(root).or_insert(0.0);
        if volts > *entry {
            *entry = volts;
        }
    }
    (rails, source_pins)
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
    author_labels: &BTreeMap<String, String>,
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
                author_label: author_labels.get(root).cloned(),
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

/// Solve-input bundle for the DC pass; `voltage` is separate because it is
/// both the boundary source and the merge destination.
struct DcInput<'a> {
    components: &'a [crate::analysis::electronics_laws::ComponentLaws],
    nets: &'a [Net],
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    modes: &'a BTreeMap<String, Vec<String>>,
    unpop: &'a std::collections::HashSet<String>,
    drives: &'a BTreeMap<String, f64>,
}

/// Solve mandatory law participation states. Present always runs; absent
/// runs only when at least one law-bearing component is unpopulated.
fn solve_law_states(input: DcInput<'_>) -> (Vec<crate::analysis::electronics_dc::DcSolution>, Vec<String>) {
    let mut states = Vec::new();
    let mut errors = Vec::new();
    let mut solve_participation = |label: &str, components: &[crate::analysis::electronics_laws::ComponentLaws], unpop: &std::collections::HashSet<String>| {
        let (mut solved, mut solve_errors) = crate::analysis::electronics_dc::solve_dc_laws(
            &crate::analysis::electronics_dc::DcContext {
                nets: input.nets,
                components,
                instances: input.instances,
                type_pins: input.type_pins,
                type_info: input.type_info,
                unpop,
                drives: input.drives,
            },
        );
        for state in &mut solved {
            state.states.insert(format!("participation={label}"));
        }
        for error in &mut solve_errors {
            *error = format!("[participation={label}] {error}");
        }
        states.append(&mut solved);
        errors.append(&mut solve_errors);
    };
    solve_participation("present", input.components, &std::collections::HashSet::new());
    let absent_components: Vec<crate::analysis::electronics_laws::ComponentLaws> = if input
        .unpop
        .is_empty()
    {
        Vec::new()
    } else {
        input
            .components
            .iter()
            .filter(|law| !input.unpop.contains(&law.instance))
            .cloned()
            .collect()
    };
    if !input.unpop.is_empty() {
        solve_participation("absent", &absent_components, input.unpop);
    }
    (states, errors)
}

/// Elaborate component constitutive laws with the same instance/type tables
/// the netlist uses (2026-09-24 component laws).
fn elaborate_law_ir(
    items: &[TopLevel],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> crate::analysis::electronics_laws::LawIr {
    crate::analysis::electronics_laws::elaborate_component_laws(
        items,
        &crate::analysis::electronics_laws::LawContext {
            instances,
            type_info,
            type_pins,
        },
    )
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

    // 2026-09-24 (component laws, Slice 2): elaborate constitutive laws
    // before topology-dependent checks; Slice 3 consumes the IR.
    let law_ir = elaborate_law_ir(items, &instances, &type_info, &type_pins);
    let (mut law_errors, component_laws) = (law_ir.errors, law_ir.components);

    let mut ds = DisjointSet::new();
    let bus_errors = collect_pin_unions(items, &instances, &type_pins, &mut ds);
    // 2026-09-25 (E14b-6): return topology — participation facts, the
    // return-net union, and decoupler auto-bridging, in that order (the
    // absent-state set gates both passes; forcing precedes the intent
    // pass so completions see final nets).
    let topo = force_return_topology(items, &instances, &type_pins, &type_info, &mut ds);
    // 2026-09-21 (E14a): node-body intents — body wiring facts union
    // first; drive intents then complete the last open pin.
    let (intent_errors, mut intent_proofs, conditional_bridges) = {
        let mut ictx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
        collect_intents(&mut ictx, items)
    };
    intent_proofs.extend(topo.topology_proofs);

    // 2026-09-25 (E14b-7): author labels keyed by FINAL root — resolved
    // here, after the intent pass (body wiring can merge nets further).
    let net_labels = resolve_net_labels(&topo.net_bindings, &mut ds);

    let (groups, nc_pins) =
        group_pins(&instances, &type_pins, &type_info, &mut ds, &topo.exempt_pins);
    let (nets, dangling) = partition_nets(&groups, &nc_pins, &net_labels);

    // Voltage classes need the nets and instances before they move into the
    // result struct. 2026-09-22 (Slice B): acknowledged shorts suppress the
    // present-state shorted-supply error.
    let mut voltage = derive_voltage(
        items,
        &VoltageInputs { nets: &nets, shortcircuit: &topo.shortcircuit },
        &instances,
        &type_pins,
        &type_info,
    );

    // 2026-09-24 (Slice 6): law solve, current/power proofs, and budgets run
    // in one post-drive pipeline so solver quantities participate everywhere.
    let (law_solve_errors, budget_errors) = post_solve_checks(
        PostSolveContext {
            items,
            nets: &nets,
            instances: &instances,
            type_pins: &type_pins,
            type_info: &type_info,
            laws: &component_laws,
            modes: &mode_catalog(&instances, &type_info),
            unpop: &topo.unpop,
        },
        &mut voltage,
    );
    law_errors.extend(law_solve_errors);

    // 2026-09-21 (E13) + 2026-09-22 (ERC contention): the finished-netlist
    // checks — decoupling verification (the auto-bridged obligations
    // included; every union is final when they fire) and the contention
    // scan. Forcing errors land first in the convention vector.
    let mut ctx = NetlistContext::new(items, &mut ds, &type_pins, &type_info, &instances);
    let (checked_convention_errors, contention_errors) =
        finished_netlist_checks(&mut ctx, &topo.unpop, &topo.shortcircuit, &nets);
    let mut convention_errors = topo.topology_errors;
    convention_errors.extend(checked_convention_errors);

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
        unpop: topo.unpop,
        shortcircuit: topo.shortcircuit,
        participation_notes: topo.participation_notes,
        participation_warnings: topo.participation_warnings,
        contention_errors,
        bus_errors,
        law_errors,
        component_laws,
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

            txn on [u1.vdd.voltage == 3.3V] [u1.vdd.current >= 0.0] { }
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

    // ── 2026-09-25 (E14b-6): return-net inference + decoupler
    // auto-bridging ──────────────────────────────────────────────────────

    #[test]
    fn return_pins_union_without_explicit_equality() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; };

            let u1: Chip = Chip { value: "a" };
            let u2: Chip = Chip { value: "b" };
        "#;
        let nl = analyze(src);
        let idx = pin_net_index(&nl.nets);
        assert_eq!(
            idx[&("u1".to_string(), "vss".to_string())],
            idx[&("u2".to_string(), "vss".to_string())],
            "return-class pins union into one net: {:?}",
            idx
        );
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("return net") && p.contains("u1.vss")),
            "{:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn unpop_return_pin_stays_off_the_return_net() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; };

            let u1: Chip = Chip { value: "a" };
            let u2: Chip = Chip { value: "b" };
            unpop u2;
        "#;
        let nl = analyze(src);
        let idx = pin_net_index(&nl.nets);
        assert!(
            !idx.contains_key(&("u2".to_string(), "vss".to_string())),
            "an absent part contributes no copper — its pins group nowhere: {:?}",
            idx
        );
        assert!(
            nl.dangling.iter().any(|d| d.contains("u1.vss")),
            "u1.vss is the only populated return pin — a lone pin dangles: {:?}",
            nl.dangling
        );
    }

    #[test]
    fn decoupler_auto_bridges_without_explicit_equality() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
            type Cap { pin a; pin b; reference "C"; spec Decoupler: true; };

            let u1: Chip = Chip { value: "mcu" };
            let c1: Cap = Cap { value: "100n" };

            txn on [u1.vdd.voltage == 3.3V] [u1.vdd.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.convention_errors.is_empty(),
            "the convention auto-bridges: {:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        // The cap's pins are symmetric — the return/supply assignment is
        // forced up to symmetry (D13: the pick is immaterial).
        let cap_nets = [
            idx[&("c1".to_string(), "a".to_string())].clone(),
            idx[&("c1".to_string(), "b".to_string())].clone(),
        ];
        let ret_net = idx[&("u1".to_string(), "vss".to_string())].clone();
        let sup_net = idx[&("u1".to_string(), "vdd".to_string())].clone();
        assert!(
            cap_nets.contains(&ret_net) && cap_nets.contains(&sup_net) && ret_net != sup_net,
            "cap bridges supply net {} to return net {}: {:?}",
            sup_net,
            ret_net,
            idx
        );
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("auto-bridged") && p.contains("c1")),
            "{:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn unpop_decoupler_never_auto_bridges() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
            type Cap { pin a; pin b; reference "C"; spec Decoupler: true; };

            let u1: Chip = Chip { value: "mcu" };
            let c1: Cap = Cap { value: "100n" };
            unpop c1;

            txn on [u1.vdd.voltage == 3.3V] [u1.vdd.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.convention_errors.len(), 1, "{:?}", nl.convention_errors);
        assert!(
            nl.convention_errors[0].contains("no decoupling part bridges"),
            "{}",
            nl.convention_errors[0]
        );
    }

    #[test]
    fn three_pin_decoupler_is_not_auto_wired() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
            type Cap3 { pin a; pin b; pin c; reference "C"; spec Decoupler: true; };

            let u1: Chip = Chip { value: "mcu" };
            let c1: Cap3 = Cap3 { value: "array" };

            txn on [u1.vdd.voltage == 3.3V] [u1.vdd.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.convention_errors.len(), 1, "{:?}", nl.convention_errors);
        assert!(
            nl.convention_errors[0].contains("no decoupling part bridges"),
            "{}",
            nl.convention_errors[0]
        );
    }

    #[test]
    fn decouple_obligation_without_return_net_is_an_error() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Chip { pin vdd: Power; reference "U"; spec Decouple: 100n; };
            type Cap { pin a; pin b; reference "C"; spec Decoupler: true; };

            let u1: Chip = Chip { value: "mcu" };
            let c1: Cap = Cap { value: "100n" };
        "#;
        let nl = analyze(src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("no return net")),
            "{:?}",
            nl.convention_errors
        );
    }

    // ── 2026-09-25 (E14b-7): membership by refutation ────────────────────

    /// Two driven rails, a tolerance-refuted supply pin: the 5V rail is
    /// not within the 4V tolerance, so membership is forced to the 3.3V
    /// rail with no author input.
    const REFUTATION_BOARD: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Src { pin p: Power; reference "U"; };
        type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Tolerance: 4V; };

        let s1: Src = Src { value: "usb" };
        let s2: Src = Src { value: "ldo" };
        let u1: Chip = Chip { value: "mcu" };

        txn r1 [s1.p.voltage == 5.0V] [s1.p.current >= 0.0] { }
        txn r2 [s2.p.voltage == 3.3V] [s2.p.current >= 0.0] { }
    "#;

    #[test]
    fn membership_infers_unique_survivor() {
        let nl = analyze(REFUTATION_BOARD);
        assert!(
            nl.convention_errors.is_empty(),
            "tolerance refutation forces membership: {:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        assert_eq!(
            idx[&("u1".to_string(), "vdd".to_string())],
            idx[&("s2".to_string(), "p".to_string())],
            "u1.vdd joins the only rail within tolerance: {:?}",
            idx
        );
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("membership inferred") && p.contains("u1.vdd")),
            "{:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn membership_zero_candidates_is_an_error() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Src { pin p: Power; reference "U"; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Tolerance: 2V; };

            let s1: Src = Src { value: "ldo" };
            let u1: Chip = Chip { value: "mcu" };

            txn r1 [s1.p.voltage == 3.3V] [s1.p.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("no driven rail is within its tolerance") && e.contains("u1.vdd")),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn membership_ambiguity_enumerates_rails() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Src { pin p: Power; reference "U"; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; };

            let s1: Src = Src { value: "usb" };
            let s2: Src = Src { value: "ldo" };
            let u1: Chip = Chip { value: "mcu" };

            txn r1 [s1.p.voltage == 5.0V] [s1.p.current >= 0.0] { }
            txn r2 [s2.p.voltage == 3.3V] [s2.p.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("membership-ambiguous") && e.contains("u1.vdd")),
            "{:?}",
            nl.convention_errors
        );
        let ambiguous = nl
            .convention_errors
            .iter()
            .find(|e| e.contains("membership-ambiguous"))
            .unwrap();
        assert!(
            ambiguous.contains("5 V@s1") && ambiguous.contains("3.3 V@s2"),
            "both rails enumerated: {ambiguous}"
        );
    }

    #[test]
    fn unpop_supply_pin_skips_membership() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Src { pin p: Power; reference "U"; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; };

            let s1: Src = Src { value: "usb" };
            let u1: Chip = Chip { value: "a" };
            let u2: Chip = Chip { value: "b" };
            unpop u2;

            txn r1 [s1.p.voltage == 5.0V] [s1.p.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert!(
            !nl.convention_errors
                .iter()
                .any(|e| e.contains("u2.vdd")),
            "an absent part demands nothing: {:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        assert_eq!(
            idx[&("u1".to_string(), "vdd".to_string())],
            idx[&("s1".to_string(), "p".to_string())],
            "the populated chip infers onto the sole rail: {:?}",
            idx
        );
    }

    // ── 2026-09-25 (E14b-7 slice 2): net<> / stdnet<> ────────────────────

    const NET_REGISTRY: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type VBUS { spec NetVoltage: 5V; };
        type V3V3 { spec NetVoltage: 3.3V; spec KicadLabel: "+3V3"; };
        type Src { pin p: Power; reference "U"; };
        type Chip { pin vdd: Power; pin vss: Ground; reference "U"; };
        type ChipA { pin vdd: Power; pin vss: Ground; reference "U"; spec Tolerance: 4V; };
        type Ldo { pin in: Power; pin vout: Power; pin gnd: Ground; reference "U"; };
    "#;

    fn net_board(extra: &str) -> String {
        format!(
            r#"{}
            let s1: Src = Src {{ value: "usb" }};
            let s2: Src = Src {{ value: "ldo" }};

            txn r1 [s1.p.voltage == 5.0V] [s1.p.current >= 0.0] {{ }}
            txn r2 [s2.p.voltage == 3.3V] [s2.p.current >= 0.0] {{ }}
            {}"#,
            NET_REGISTRY, extra
        )
    }

    #[test]
    fn stdnet_expectation_forces_membership_and_labels() {
        let src = net_board("stdnet<V3V3> let u1: Chip = Chip { value: \"mcu\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors.is_empty(),
            "the 3.3V expectation decides: {:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        assert_eq!(
            idx[&("u1".to_string(), "vdd".to_string())],
            idx[&("s2".to_string(), "p".to_string())],
            "u1.vdd joins the 3.3V rail: {:?}",
            idx
        );
        let net_name = &idx[&("u1".to_string(), "vdd".to_string())];
        let net = nl.nets.iter().find(|n| &n.name == net_name).unwrap();
        assert_eq!(
            net.author_label.as_deref(),
            Some("+3V3"),
            "the registry KicadLabel flows to the net: {:?}",
            net
        );
    }

    #[test]
    fn stdnet_unknown_name_is_an_error() {
        let src = net_board("stdnet<V1V8> let u1: Chip = Chip { value: \"mcu\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("no standard net 'V1V8' is declared")),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn stdnet_expectation_without_matching_drive_is_an_error() {
        // Only a 5V rail is driven; the V3V3 expectation admits nothing.
        let src = net_board("stdnet<V3V3> let u1: Chip = Chip { value: \"mcu\" };");
        let src = src.replace(
            "txn r2 [s2.p.voltage == 3.3V] [s2.p.current >= 0.0] { }",
            "let s2: Src = Src { value: \"idle\" };",
        );
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1.vdd") && e.contains("V3V3 expects a 3.3 V-driven rail")),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn bare_form_on_multi_supply_is_an_error() {
        let src = net_board("stdnet<VBUS> let u1: Ldo = Ldo { value: \"ldo\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors.iter().any(|e| {
                e.contains("u1") && e.contains("2 supply pins") && e.contains("qualify")
            }),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn qualified_form_covers_both_supply_pins() {
        let src = net_board("stdnet<in: VBUS, vout: V3V3> let u1: Ldo = Ldo { value: \"ldo\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors.is_empty(),
            "{:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        assert_eq!(
            idx[&("u1".to_string(), "in".to_string())],
            idx[&("s1".to_string(), "p".to_string())],
            "u1.in joins the VBUS rail: {:?}",
            idx
        );
        assert_eq!(
            idx[&("u1".to_string(), "vout".to_string())],
            idx[&("s2".to_string(), "p".to_string())],
            "u1.vout joins the V3V3 rail: {:?}",
            idx
        );
    }

    #[test]
    fn net_binds_through_forced_pin_and_propagates() {
        let src = net_board("net<v3_3> let a: ChipA = ChipA { value: \"a\" };\nlet b: Chip = Chip { value: \"b\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors.is_empty(),
            "the forced pin binds v3_3; the ambiguous pin propagates: {:?}",
            nl.convention_errors
        );
        let idx = pin_net_index(&nl.nets);
        let a_net = idx[&("a".to_string(), "vdd".to_string())].clone();
        assert_eq!(
            a_net,
            idx[&("s2".to_string(), "p".to_string())],
            "a.vdd forced onto 3.3V"
        );
        assert_eq!(
            idx[&("b".to_string(), "vdd".to_string())],
            a_net,
            "b.vdd propagates to the bound net"
        );
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("membership propagated") && p.contains("b.vdd")),
            "{:?}",
            nl.intent_proofs
        );
    }

    #[test]
    fn two_named_candidates_stay_ambiguous() {
        let src = net_board("net<vbus> let s1x: Src = Src { value: \"x\" };\nlet c: Chip = Chip { value: \"c\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("c.vdd") && e.contains("no single bound net name decides")),
            "s1.p (net<vbus>) and s2.p (unnamed) leave c.vdd ambiguous: {:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn return_pin_takes_no_net_name() {
        let src = net_board("net<gnd> let u1: Chip = Chip { value: \"mcu\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1.vdd") && e.contains("membership-ambiguous")),
            "the bare form lands on vdd (the sole supply pin), which stays ambiguous; vss is \
             the return and takes no name: {:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn non_supply_pin_takes_no_net_name() {
        let src = net_board("stdnet<in: VBUS, gnd: V3V3> let u1: Ldo = Ldo { value: \"ldo\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1.gnd") && e.contains("return-class pin")),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn identifier_opacity_no_registry_read_for_net() {
        // net<VBUS> is board-LOCAL even though a standard net VBUS exists:
        // the opaque form never consults the registry (Rule 15).
        let src = net_board("net<VBUS> let u1: Chip = Chip { value: \"mcu\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1.vdd") && e.contains("membership-ambiguous")),
            "no expectation is applied — the unrated pin stays ambiguous: {:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn duplicate_pin_qualifier_is_an_error() {
        let src = net_board("stdnet<in: VBUS, in: V3V3> let u1: Ldo = Ldo { value: \"ldo\" };");
        let nl = analyze(&src);
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1.in") && e.contains("two net names")),
            "{:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn net_modifier_on_a_node_is_a_parse_error() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            net<VBUS> async node n [true] [true] { }
        "#;
        let tokens = tokenize(src);
        assert!(tokens.is_ok());
        let mut p = Parser::new(tokens.unwrap(), src);
        assert!(p.parse_program().is_err(), "net<> names a let's net, not a node");
    }

    // ── 2026-09-25 (quantities Phase 4, slice 1): spec MaxCurrent ────────

    /// A driven LED chain with law physics: (3.3 − 1.8) V across 330 Ω ≈
    /// 4.5 mA through the LED — inside the type's 20 mA envelope.
    const LED_ENVELOPE_BOARD: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Rail { pin hi: Power; pin lo: Power; reference "P"; };
        type Resistor { pin a; pin b; reference "R"; spec Resistance: Ohm;
            when true {
                a.voltage - b.voltage == Resistance * a.current;
                a.current + b.current == 0;
            };
        };
        type Led { pin a; pin k; reference "D"; spec MaxCurrent: 20mA; spec ForwardVoltage: Volt; spec DynamicResistance: Ohm;
            when a.voltage - k.voltage >= ForwardVoltage {
                a.current == (a.voltage - k.voltage - ForwardVoltage) / DynamicResistance;
            };
        };
        let p: Rail = Rail { value: "rail" };
        let r: Resistor = Resistor { spec Resistance: 330Ohm };
        let d: Led = Led { spec ForwardVoltage: 1.8Volt; spec DynamicResistance: 10Ohm };
        txn on [p.hi.voltage == r.a.voltage && r.b.voltage == d.a.voltage && d.k.voltage == p.lo.voltage && p.hi.voltage == 3.3Volt && p.lo.voltage == 0.0Volt] [d.a.current > 0] { }
    "#;

    #[test]
    fn max_current_envelope_proves() {
        let nl = analyze(LED_ENVELOPE_BOARD);
        assert!(
            !nl.voltage.violations.iter().any(|v| v.contains("absolute maximum")),
            "{:?}",
            nl.voltage.violations
        );
        assert!(
            nl.voltage.proved
                .iter()
                .any(|p| p.contains("d.a") && p.contains("absolute maximum")),
            "{:?}",
            nl.voltage.proved
        );
    }

    #[test]
    fn max_current_violation_is_an_error() {
        let src = LED_ENVELOPE_BOARD.replace("spec Resistance: 330Ohm", "spec Resistance: 10Ohm");
        let nl = analyze(&src);
        assert!(
            nl.voltage.violations
                .iter()
                .any(|v| v.contains("d.a") && v.contains("absolute maximum current is 20.0 mA")),
            "{:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn instance_max_current_override_wins() {
        // The type allows 20 mA; the instance is derated to 4 mA — the
        // 4.5 mA operating point violates the INSTANCE rating.
        let src = LED_ENVELOPE_BOARD.replace(
            "spec ForwardVoltage: 1.8Volt; spec DynamicResistance: 10Ohm",
            "spec ForwardVoltage: 1.8Volt; spec DynamicResistance: 10Ohm; spec MaxCurrent: 4mA",
        );
        let nl = analyze(&src);
        assert!(
            nl.voltage.violations
                .iter()
                .any(|v| v.contains("d.a") && v.contains("absolute maximum current is 4.0 mA")),
            "the instance derating decides: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn pin_qualified_max_current_beats_uniform() {
        // Asymmetric envelope: pin a is derated to 4 mA, uniform 20 mA
        // would pass — the pin row decides.
        let src = LED_ENVELOPE_BOARD
            .replace("spec MaxCurrent: 20mA", "spec MaxCurrent: a: 4mA, k: 1A");
        let nl = analyze(&src);
        assert!(
            nl.voltage.violations
                .iter()
                .any(|v| v.contains("d.a") && v.contains("absolute maximum current is 4.0 mA")),
            "the pin-qualified row decides: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn unpop_led_skips_envelope() {
        let src = LED_ENVELOPE_BOARD
            .replace("spec Resistance: 330Ohm", "spec Resistance: 10Ohm")
            .replace("let d: Led", "unpop d;\n        let d: Led");
        let nl = analyze(&src);
        assert!(
            !nl.voltage
                .violations
                .iter()
                .any(|v| v.contains("d.a") && v.contains("absolute maximum")),
            "an absent part violates nothing: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn max_current_wrong_dim_is_a_parse_error() {
        let src = r#"
            type Led { pin a; pin k; reference "D"; };
            let d: Led = Led { spec MaxCurrent: 3.3V };
        "#;
        let tokens = tokenize(src);
        let mut p = Parser::new(tokens.unwrap(), src);
        let err = format!("{}", p.parse_program().unwrap_err());
        assert!(
            err.contains("the key expects amp"),
            "wrong-dim envelope is a parse error: {err}"
        );
    }

    // ── 2026-09-25 (quantities Phase 4, slice 2): pin-qualified envelopes ─

    /// Two driven rails; the chip's tolerance rows are per pin (6 V on the
    /// 5 V rail, 3.6 V on the 3.3 V rail).
    const PIN_TOLERANCE_BOARD: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Src { pin p: Power; reference "S"; spec Tolerance: any; };
        type Chip { pin vdd: Power; pin aux: Power; pin gnd: Ground; reference "U"; spec Tolerance: vdd: 6V, aux: 3.6V; };
        let s5: Src = Src { value: "5v" };
        let s3: Src = Src { value: "3v3" };
        let u: Chip = Chip { value: "mcu" };
        txn r5 [s5.p.voltage == u.vdd.voltage && s5.p.voltage == 5.0Volt] [s5.p.current >= 0.0] { }
        txn r3 [s3.p.voltage == u.aux.voltage && s3.p.voltage == 3.3Volt] [s3.p.current >= 0.0] { }
    "#;

    #[test]
    fn pin_qualified_tolerance_proves_per_pin() {
        let nl = analyze(PIN_TOLERANCE_BOARD);
        assert!(
            !nl.voltage
                .violations
                .iter()
                .any(|v| v.contains("tolerates only") || v.contains("no tolerance clause")),
            "both pins prove within their rows: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn pin_qualified_tolerance_violates_only_the_pin() {
        // aux derated to 3.2 V: the 3.3 V rail violates aux and ONLY aux —
        // vdd's 6 V row is untouched by the uniform absence.
        let src = PIN_TOLERANCE_BOARD.replace("aux: 3.6V", "aux: 3.2V");
        let nl = analyze(&src);
        let aux = nl
            .voltage
            .violations
            .iter()
            .find(|v| v.contains("u.aux") && v.contains("tolerates only 3.2 V"));
        assert!(aux.is_some(), "{:?}", nl.voltage.violations);
        assert!(
            !nl.voltage
                .violations
                .iter()
                .any(|v| v.contains("u.vdd") && v.contains("tolerates only")),
            "vdd's row is independent: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn instance_tolerance_override_wins() {
        // Type row 6 V; the instance derates to 4.5 V — the 5 V rail
        // violates the INSTANCE rating, not the type's.
        let src = PIN_TOLERANCE_BOARD.replace(
            "let u: Chip = Chip { value: \"mcu\" };",
            "let u: Chip = Chip { value: \"mcu\"; spec Tolerance: 4.5V };",
        );
        let nl = analyze(&src);
        assert!(
            nl.voltage
                .violations
                .iter()
                .any(|v| v.contains("u.vdd") && v.contains("tolerates only 4.5 V")),
            "the instance derating decides: {:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn instance_rating_override_wins() {
        // A 10 Ω part across 3.3 V dissipates ~1.09 W; the instance
        // derates the rating to 0.5 W — violation names the derated value.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Rail { pin hi: Power; pin lo: Power; reference "P"; };
            type Resistor { pin a; pin b; reference "R"; spec Resistance: Ohm; spec Rating: 5W;
                when true {
                    a.voltage - b.voltage == Resistance * a.current;
                    a.current + b.current == 0;
                };
            };
            let p: Rail = Rail { value: "rail" };
            let r: Resistor = Resistor { spec Resistance: 10Ohm; spec Rating: 0.5W };
            txn on [p.hi.voltage == r.a.voltage && r.b.voltage == p.lo.voltage && p.hi.voltage == 3.3Volt && p.lo.voltage == 0.0Volt] [r.a.current >= 0.0] { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage
                .violations
                .iter()
                .any(|v| v.contains("r") && v.contains("rated 500.0 mW")),
            "the instance derating decides: {:?}",
            nl.voltage.violations
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
        type Led { pin a; pin k; reference "D"; spec Tolerance: 3.6V; };
        type Connector { pin vcc; pin gnd; reference "J"; };

        let j1: Connector = Connector { value: "JST-2" };
        let r1: Resistor = Resistor { value: "330", spec Resistance: 330Ohm; };
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
                         pin c1: Path; pin c2: Path; reference "K"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
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
            type Wire { pin a; pin b; reference "W"; spec Tolerance: any; };
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
            type Wire { pin a; pin b; reference "W"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
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
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; spec Tolerance: any; };
            type Conn { pin p; pin g; reference "J"; spec Tolerance: any; };
            type Regulator { pin vin: Power; pin vout: Power; pin gnd: Power; reference "U"; spec Tolerance: any;
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
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; spec Tolerance: any; };
            type Conn { pin p; reference "J"; spec Tolerance: any; };
            type Regulator { pin vin: Power; pin vout: Power; pin gnd: Power; reference "U"; spec Tolerance: any;
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
            type Power { pin vout; reference "P"; spec KicadType: "power_in"; spec Supply: true; spec Tolerance: any; };
            type Switch { pin sel: Power; pin vout: Power; pin gnd: Power; reference "U"; spec Tolerance: any;
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
            type Mcu { pin p1: Io; pin p2: Io; reference "U"; spec Tolerance: any; };
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
            type Mcu { pin p1: IoOd; pin p2: IoOd; reference "U"; spec Tolerance: any; };
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
            type Wire { pin a: Io; pin b: Io; reference "W"; spec Tolerance: any; };
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
            type Chip { pin gpio[4]; pin gnd; reference "U"; spec Tolerance: any; };
            type Sensor { pin data[4]; pin gnd; reference "S"; spec Tolerance: any; };
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
            type Chip { pin gpio[4]; pin gnd; reference "U"; spec Tolerance: any; };
            type Sensor { pin data[8]; pin gnd; reference "S"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
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
    fn component_spec_resistance_is_structured_si() {
        // 2026-09-24 (component laws): `spec Resistance` is carried as SI +
        // dimension, separate from the opaque BOM label.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; spec Resistance: Ohm; };
            let r1: Resistor = Resistor { value: "330R"; spec Resistance: 4.7kOhm; };
        "#;
        let nl = analyze(src);
        assert_eq!(nl.components.len(), 1);
        match nl.components[0].specs.get("resistance") {
            Some(crate::ast::PropertyValue::Quantity { si, dimension }) => {
                assert_eq!(*dimension, crate::ast::QuantityDim::Ohm);
                assert!((si - 4700.0).abs() < 1e-9);
            }
            other => panic!("expected structured 4.7kOhm, got {other:?}"),
        }
    }

    #[test]
    fn spec_resistance_is_the_sole_physics_source() {
        // Structured physics is the sole truth: the opaque BOM label may say
        // anything without changing the derivation.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; spec Resistance: Ohm; };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source {};
            let r1: Resistor = Resistor { value: "999R"; spec Resistance: 1kOhm; };
            txn drive
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage && r1.b.voltage == v1.n.voltage]
                [r1.b.current <= 0.1]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage
                .net_current
                .values()
                .any(|i| (i - 0.0033).abs() < 1e-12),
            "spec Resistance 1kOhm should derive 3.3mA, got {:?}",
            nl.voltage.net_current
        );
    }

    #[test]
    fn value_annotation_alone_never_derives_series_physics() {
        // 2026-09-24 (retire legacy value physics): `value` is an annotation.
        // The SAME circuit as the spec test, but with no `spec Resistance`,
        // must derive no current at all — the BOM label carries no physics.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source {};
            let r1: Resistor = Resistor { value: "1k" };
            txn drive
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage && r1.b.voltage == v1.n.voltage]
                [r1.b.current <= 0.1]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage.net_current.is_empty(),
            "value-only resistor must derive no current, got {:?}",
            nl.voltage.net_current
        );
        assert!(
            !nl.voltage
                .proved
                .iter()
                .any(|p| p.contains("Ohm's law")),
            "no Ohm's-law proof may come from a label: {:?}",
            nl.voltage.proved
        );
    }

    #[test]
    fn dc_law_solve_derives_resistor_branch_current() {
        let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R"; spec Tolerance: any; spec Rating: 0.05W;
                spec Resistance: Ohm;
                when true {
                    a.voltage - b.voltage == Resistance * a.current;
                    a.current + b.current == 0;
                }
            };
            type Supply { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Supply = Supply { };
            let r1: Resistor = Resistor { spec Resistance: 330Ohm; };
            txn drive
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                 && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [r1.b.current <= 0.011]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        let into_a = nl.voltage.pin_current.get(&("r1".into(), "a".into())).copied();
        let into_b = nl.voltage.pin_current.get(&("r1".into(), "b".into())).copied();
        assert!(into_a.map_or(false, |i| (i - 0.01).abs() < 1e-9), "{into_a:?}");
        assert!(into_b.map_or(false, |i| (i + 0.01).abs() < 1e-9), "{into_b:?}");
        assert!(nl.voltage.violations.is_empty(), "{:?}", nl.voltage.violations);
    }

    #[test]
    fn dc_law_solve_classes_divider_mid_net() {
        let resistor = r#"
            type Resistor {
                pin a; pin b;
                reference "R"; spec Tolerance: any; spec Rating: 0.05W;
                spec Resistance: Ohm;
                when true {
                    a.voltage - b.voltage == Resistance * a.current;
                    a.current + b.current == 0;
                }
            };
            type Supply { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Supply = Supply { };
            let top: Resistor = Resistor { spec Resistance: 1kOhm; };
            let bottom: Resistor = Resistor { spec Resistance: 1kOhm; };
            txn drive
                [v1.p.voltage == 5.0V && v1.p.voltage == top.a.voltage
                 && top.b.voltage == bottom.a.voltage
                 && bottom.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [top.b.voltage >= 2.0V && top.b.voltage <= 3.0V]
            { }
        "#;
        let nl = analyze(resistor);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        let mid = nl
            .voltage
            .net_voltage
            .values()
            .find(|v| (**v - 2.5).abs() < 1e-9)
            .copied();
        assert!(mid.is_some(), "mid net classes to 2.5 V: {:?}", nl.voltage.net_voltage);
        let top_current = nl.voltage.pin_current.get(&("top".into(), "a".into())).copied();
        assert!(top_current.map_or(false, |i| (i - 0.0025).abs() < 1e-9), "{top_current:?}");
    }

    #[test]
    fn underdetermined_dc_law_group_is_a_hard_error() {
        let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R"; spec Tolerance: any;
                spec Resistance: Ohm;
                when true {
                    a.current + b.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let r1: Resistor = Resistor { spec Resistance: 1kOhm; };
            txn open_load
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                 && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [true]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.law_errors
                .iter()
                .any(|e| e.contains("no unique DC operating point") && e.contains("free variable")),
            "{:?}",
            nl.law_errors
        );
    }

    #[test]
    fn contradictory_dc_law_boundaries_are_a_hard_error() {
        let src = r#"
            type Wire {
                pin a; pin b;
                reference "W"; spec Tolerance: any;
                when true {
                    a.voltage == b.voltage;
                    a.current + b.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let w1: Wire = Wire { };
            txn contradiction
                [v1.p.voltage == 3.3V && v1.p.voltage == w1.a.voltage
                 && w1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [true]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.law_errors
                .iter()
                .any(|e| e.contains("no DC operating point")),
            "{:?}",
            nl.law_errors
        );
    }

    #[test]
    fn law_power_requires_a_rating_and_checks_it() {
        let body = |rating: &str| format!(
            r#"
                type Resistor {{
                    pin a; pin b;
                    reference "R"; spec Tolerance: any; {rating}
                    spec Resistance: Ohm;
                    when true {{
                        a.voltage - b.voltage == Resistance * a.current;
                        a.current + b.current == 0;
                    }}
                }};
                type Source {{ pin p; pin n; reference "V"; spec Tolerance: any; }};
                let v1: Source = Source {{ }};
                let r1: Resistor = Resistor {{ spec Resistance: 1kR; }};
                txn drive
                    [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                     && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                    [true]
                {{ }}
            "#
        );
        let missing = analyze(&body(""));
        assert!(
            missing.voltage.violations.iter().any(|e| e.contains("proven-dissipating law part")),
            "{:?}",
            missing.voltage.violations
        );
        let over = analyze(&body("spec Rating: 0.01W;"));
        assert!(
            over.voltage.violations.iter().any(|e| e.contains("exceeds the declared rating")),
            "{:?}",
            over.voltage.violations
        );
        let within = analyze(&body("spec Rating: 0.05W;"));
        assert!(
            within.voltage.violations.is_empty(),
            "{:?}",
            within.voltage.violations
        );
        assert!(within
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("P(r1)") && p.contains("component-law DC")));
    }

    #[test]
    fn budget_uses_component_law_current() {
        let body = |limit: &str| format!(
            r#"
                type Resistor {{
                    pin a; pin b;
                    reference "R"; spec Tolerance: any; spec Rating: 0.05W;
                    spec Resistance: Ohm;
                    when true {{
                        a.voltage - b.voltage == Resistance * a.current;
                        a.current + b.current == 0;
                    }}
                }};
                type Source {{ pin p; pin n; reference "V"; spec Tolerance: any; }};
                let v1: Source = Source {{ }};
                let r1: Resistor = Resistor {{ spec Resistance: 330R; }};
                budget v1.p {limit};
                txn drive
                    [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                     && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                    [true]
                {{ }}
            "#
        );
        let over = analyze(&body("<= 5mA"));
        assert_eq!(over.budget_errors.len(), 1, "{:?}", over.budget_errors);
        assert!(
            over.budget_errors[0].contains("component-law DC"),
            "{}",
            over.budget_errors[0]
        );
        let under = analyze(&body("<= 20mAmp"));
        assert!(under.budget_errors.is_empty(), "{:?}", under.budget_errors);
    }

    #[test]
    fn lower_current_bound_uses_signed_law_current() {
        let body = |bound: &str| format!(
            r#"
                type Resistor {{
                    pin a; pin b;
                    reference "R"; spec Tolerance: any; spec Rating: 0.05W;
                    spec Resistance: Ohm;
                    when true {{
                        a.voltage - b.voltage == Resistance * a.current;
                        a.current + b.current == 0;
                    }}
                }};
                type Source {{ pin p; pin n; reference "V"; spec Tolerance: any; }};
                let v1: Source = Source {{ }};
                let r1: Resistor = Resistor {{ spec Resistance: 330R; }};
                txn drive
                    [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                     && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                    [r1.a.current {bound}]
                {{ }}
            "#
        );
        let pass = analyze(&body(">= 5mAmp"));
        assert!(pass.voltage.violations.is_empty(), "{:?}", pass.voltage.violations);
        assert!(pass
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("I(r1.a)") && p.contains("component-law DC")));
        let fail = analyze(&body(">= 20mAmp"));
        assert!(
            fail.voltage.violations.iter().any(|e| e.contains("lower bound")),
            "{:?}",
            fail.voltage.violations
        );
    }

    #[test]
    fn unpop_law_part_solves_both_participation_states() {
        let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R"; spec Tolerance: any; spec Rating: any;
                spec Resistance: Ohm;
                when true {
                    a.voltage - b.voltage == Resistance * a.current;
                    a.current + b.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let r1: Resistor = Resistor { spec Resistance: 1kR; };
            unpop r1;
            txn open
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                 && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [true]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        // Both mandatory participation states solve. The present state has
        // the full law operating point; the absent state omits its current.
        assert!(
            nl.voltage
                .proved
                .iter()
                .any(|p| p.contains("participation=present") && p.contains("P(r1)")),
            "{:?}",
            nl.voltage.proved
        );
        assert!(
            nl.voltage
                .proved
                .iter()
                .any(|p| p.contains("participation=absent")),
            "{:?}",
            nl.voltage.proved
        );
    }

    #[test]
    fn unpop_present_state_enforces_current_bound() {
        // The absent state has no branch current, but a bound must hold in
        // every state: the populated 3.3V / 1kR state violates 2mAmp.
        let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R"; spec Tolerance: any; spec Rating: any;
                spec Resistance: Ohm;
                when true {
                    a.voltage - b.voltage == Resistance * a.current;
                    a.current + b.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let r1: Resistor = Resistor { spec Resistance: 1kR; };
            unpop r1;
            txn open
                [v1.p.voltage == 3.3V && v1.p.voltage == r1.a.voltage
                 && r1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [r1.a.current >= 5mAmp]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        assert!(
            nl.voltage
                .violations
                .iter()
                .any(|e| e.contains("participation=present") && e.contains("lower")),
            "{:?}",
            nl.voltage.violations
        );
    }

    #[test]
    fn ambiguous_bistable_group_without_authority_is_an_error() {
        let src = r#"
            type Flip {
                pin a; pin k;
                reference "F"; spec Tolerance: any; spec Rating: any;
                when a.current >= 0Amp { a.current == 1Amp; }
                when a.current < 0Amp { a.current == -1Amp; }
                when true { a.current + k.current == 0; }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let f1: Flip = Flip { };
            txn drive
                [v1.p.voltage == 1.0V && v1.p.voltage == f1.a.voltage
                 && f1.k.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [true]
            { }
        "#;
        let nl = analyze(src);
        assert_eq!(nl.law_errors.len(), 1, "{:?}", nl.law_errors);
        assert!(
            nl.law_errors[0].contains("2 DC operating points")
                && nl.law_errors[0].contains("spec Bistable: true"),
            "{}",
            nl.law_errors[0]
        );
    }

    #[test]
    fn acknowledged_bistable_law_proves_all_states() {
        let body = |bound: &str| format!(
            r#"
                type Flip {{
                    pin a; pin k;
                    reference "F"; spec Tolerance: any; spec Rating: any;
                    spec Bistable: true;
                    when a.current >= 0Amp {{ a.current == 1Amp; }}
                    when a.current < 0Amp {{ a.current == -1Amp; }}
                    when true {{ a.current + k.current == 0; }}
                }};
                type Source {{ pin p; pin n; reference "V"; spec Tolerance: any; }};
                let v1: Source = Source {{ }};
                let f1: Flip = Flip {{ }};
                txn drive
                    [v1.p.voltage == 1.0V && v1.p.voltage == f1.a.voltage
                     && f1.k.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                    [f1.a.current {bound}]
                {{ }}
            "#
        );
        let pass = analyze(&body("<= 2Amp"));
        assert!(pass.law_errors.is_empty(), "{:?}", pass.law_errors);
        assert!(pass.voltage.violations.is_empty(), "{:?}", pass.voltage.violations);
        assert!(pass
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("mode=01") && p.contains("I(f1.a)")));
        assert!(pass
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("mode=02") && p.contains("I(f1.a)")));

        // The negative state violates a non-negative lower bound even though
        // the positive state satisfies it: contracts hold in every state.
        let mixed = analyze(&body(">= 0Amp"));
        assert!(
            mixed
                .voltage
                .violations
                .iter()
                .any(|e| e.contains("mode=02") && e.contains("lower bound")),
            "{:?}",
            mixed.voltage.violations
        );
    }

    #[test]
    fn operating_state_ebv_fixtures_are_checked_in_all_states() {
        // 2026-09-24 (multi-state laws): these sources are the durable
        // language-facing tests, not just inline strings in Rust.
        let valid = analyze(include_str!(
            "../../tests/electronics/operating_states.ebv"
        ));
        assert!(valid.law_errors.is_empty(), "{:?}", valid.law_errors);
        assert!(valid.voltage.violations.is_empty(), "{:?}", valid.voltage.violations);
        assert!(valid
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("mode=01") && p.contains("participation=present")));
        assert!(valid
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("mode=02") && p.contains("participation=present")));
        assert!(valid
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("participation=absent") && p.contains("'r1' omitted")));

        let mixed = analyze(include_str!(
            "../../tests/electronics/operating_states_bound.ebv"
        ));
        assert!(mixed.law_errors.is_empty(), "{:?}", mixed.law_errors);
        assert!(
            mixed
                .voltage
                .violations
                .iter()
                .any(|e| e.contains("mode=02") && e.contains("lower bound")),
            "{:?}",
            mixed.voltage.violations
        );
    }

    #[test]
    fn spst_mode_ebv_fixture_selects_operating_states() {
        // 2026-09-24 (SPST modes): the checked-in language fixture proves
        // pressed behavior only in closed and released behavior only in open.
        let nl = analyze(include_str!("../../tests/electronics/spst_modes.ebv"));
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        assert!(nl.voltage.violations.is_empty(), "{:?}", nl.voltage.violations);
        assert!(nl
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("sw1=closed") && p.contains("I(r1.a)")));
        assert!(nl
            .voltage
            .proved
            .iter()
            .any(|p| p.contains("sw1=open") && p.contains("I(r1.a)")));

        let typo = analyze(include_str!(
            "../../tests/electronics/spst_unknown_mode.ebv"
        ));
        assert!(
            typo.law_errors
                .iter()
                .chain(typo.voltage.violations.iter())
                .any(|e| e.contains("undeclared mode") && e.contains("closedd")),
            "law={:?} violations={:?}",
            typo.law_errors,
            typo.voltage.violations
        );
    }

    #[test]
    fn contradictory_mode_precondition_has_no_operating_state() {
        let src = r#"
            type Spst {
                pin a; pin b;
                reference "SW"; spec Tolerance: any; spec Rating: any;
                mode closed {
                    a.voltage == b.voltage;
                    a.current + b.current == 0;
                }
                mode open {
                    a.current == 0Amp;
                    b.current == 0Amp;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let sw1: Spst = Spst { };
            async node impossible
                [v1.p.voltage == 1.0V && v1.p.voltage == sw1.a.voltage
                 && sw1.b.voltage == v1.n.voltage && v1.n.voltage == 0.0V
                 && sw1.closed && !sw1.closed]
                [true]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.law_errors
                .iter()
                .any(|e| e.contains("matches none") && e.contains("'impossible'")),
            "{:?}",
            nl.law_errors
        );
    }

    #[test]
    fn guarded_diode_law_solves_forward_mode() {
        let src = r#"
            type Diode {
                pin a; pin k;
                reference "D"; spec Tolerance: any; spec Rating: any;
                when a.voltage - k.voltage >= 0.7Volt {
                    a.current ==
                        (a.voltage - k.voltage - 0.7Volt) / 100Ohm;
                }
                when a.voltage - k.voltage < 0.7Volt {
                    a.current == 0Amp;
                }
                when true {
                    a.current + k.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let d1: Diode = Diode { };
            txn forward
                [v1.p.voltage == 3.3V && v1.p.voltage == d1.a.voltage
                 && d1.k.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [d1.a.current <= 0.03]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        let current = nl.voltage.pin_current.get(&("d1".into(), "a".into())).copied();
        assert!(current.map_or(false, |i| (i - 0.026).abs() < 1e-9), "{current:?}");
    }

    #[test]
    fn guarded_diode_law_solves_reverse_mode() {
        let src = r#"
            type Diode {
                pin a; pin k;
                reference "D"; spec Tolerance: any; spec Rating: any;
                when a.voltage - k.voltage >= 0.7Volt {
                    a.current ==
                        (a.voltage - k.voltage - 0.7Volt) / 100Ohm;
                }
                when a.voltage - k.voltage < 0.7Volt {
                    a.current == 0Amp;
                }
                when true {
                    a.current + k.current == 0;
                }
            };
            type Source { pin p; pin n; reference "V"; spec Tolerance: any; };
            let v1: Source = Source { };
            let d1: Diode = Diode { };
            txn reverse
                [v1.p.voltage == -3.3V && d1.a.voltage == v1.p.voltage
                 && d1.k.voltage == v1.n.voltage && v1.n.voltage == 0.0V]
                [d1.a.current <= 0.001]
            { }
        "#;
        let nl = analyze(src);
        assert!(nl.law_errors.is_empty(), "{:?}", nl.law_errors);
        let current = nl.voltage.pin_current.get(&("d1".into(), "a".into())).copied();
        assert!(current.map_or(false, |i| i.abs() < 1e-12), "{current:?}");
    }

    #[test]
    fn transitive_closure_merges_chain_into_one_net() {
        let src = r#"
            struct Pin { voltage: Float; };
            type A { pin x; pin y;     reference "A";
};
    let site = MembershipSite {
        inst: inst.clone(),
        pname: pname.clone(),
        rated: rated.clone(),
        attachment: attachment.clone(),
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
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Load { pin vin; reference "L"; spec Tolerance: 5.5V; };
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
            type Led { pin a; pin k; spec Tolerance: 3.3V;     reference "L";
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
            type Rail { pin hi; pin lo; reference "R"; spec Tolerance: any; };
            type Load { pin vin; reference "L"; spec Tolerance: any; };
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
        // is an UNDECLARED decision — a violation; `spec Tolerance: any;` is the
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
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Load { pin vin; reference "L"; spec Tolerance: any; };
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

    #[test]
    fn spec_tolerance_and_rating_feed_the_checks() {
        // 2026-09-24 (quantities Phase 2): the envelopes read from the spec
        // channel — rated, declared-unrated, and over-dissipating behave
        // exactly like the clause forms they replace.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; spec Tolerance: 12V; };
            type Load { pin vin; reference "L"; spec Tolerance: 3.3V; };
            let p1: Power = Power { };
            let l1: Load = Load { };
            txn apply
                [p1.vout.voltage == l1.vin.voltage && p1.vout.voltage == 5.0]
                [l1.vin.current >= 0.0]
            { }
        "#;
        let nl = analyze(src);
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("tolerates only 3.3 V")),
            "spec Tolerance must rate the pin: {:?}",
            nl.voltage.violations
        );

        let any_src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Load { pin vin; reference "L"; spec Tolerance: any; };
            let p1: Power = Power { };
            let l1: Load = Load { };
            txn apply
                [p1.vout.voltage == l1.vin.voltage && p1.vout.voltage == 12.0]
                [l1.vin.current >= 0.0]
            { }
        "#;
        let nl = analyze(any_src);
        assert!(
            nl.voltage.violations.is_empty(),
            "`spec Tolerance: any;` declares the decision: {:?}",
            nl.voltage.violations
        );

        let rated = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; spec Rating: 0.25W; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { spec Resistance: 330Ohm; };
            txn apply
                [p1.vout.voltage == r1.a.voltage && r1.b.voltage == p1.gnd.voltage && p1.vout.voltage == 12.0 && p1.gnd.voltage == 0.0]
                [r1.b.current > 0.0]
            { }
        "#;
        let nl = analyze(rated);
        assert!(
            nl.voltage.violations.iter().any(|v| v.contains("rated 250.0 mW")),
            "`spec Rating: 0.25W;` must rate the dissipation: {:?}",
            nl.voltage.violations
        );
    }

    // ── B4 flagship: Ohm's-law current derivation ────────────────────

    #[test]
    fn ohms_law_derives_current_and_proves_bounds() {
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Led { pin a; pin k; reference "D"; spec Tolerance: 3.6V; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330", spec Resistance: 330Ohm; };
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
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; spec Rating: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; spec Rating: 0.25W; };
            type Sensor { pin s; reference "S"; spec Tolerance: 2.0V; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "1k", spec Resistance: 1kOhm; };
            let r2: Resistor = Resistor { value: "1k", spec Resistance: 1kOhm; };
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
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; spec Rating: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; spec Rating: 0.25W; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "660", spec Resistance: 660Ohm; };
            let r2: Resistor = Resistor { value: "660", spec Resistance: 660Ohm; };
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
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Led { pin a; pin k; reference "D"; spec Tolerance: 3.6V; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "33", spec Resistance: 33Ohm; };
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
            type Rail { pin hi; pin lo; reference "R"; spec Tolerance: any; };
            type Load { pin vin; reference "L"; spec Tolerance: any; };
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
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; spec Rating: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; spec Rating: 0.25W; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330", spec Resistance: 330Ohm; };
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
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; spec Rating: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            let p1: Power = Power { };
            let r1: Resistor = Resistor { value: "330", spec Resistance: 330Ohm; };
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
            type Power { pin vout; pin gnd; reference "P"; spec Tolerance: any; spec Rating: any; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; spec Tolerance: any; };
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
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Connector { pin p1 = 1; pin p2 = 2; reference "J"; spec Tolerance: any; };
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
