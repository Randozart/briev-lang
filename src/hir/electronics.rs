// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (H5 port): contract-inferred electronics netlist over typed HIR.
//!
//! Mirrors the topology, participation, return-net, decoupling, contention,
//! and voltage-drive subset of `analysis::electronics` (the AST remains the
//! oracle until the KiCad consumer flips). Every fact is derived from HIR
//! type declarations, globals, hold laws, and callable contracts — never by
//! lowering back to the AST. The public result type is reused unchanged so a
//! later consumer flip needs no adapter.
//!
//! Coverage is deliberately explicit (the full AST pass is ~12k lines):
//! - covered: pin declarations + class properties + units, component
//!   instances and literal/spec fields, precondition pin equalities, return
//!   nets, participation (`unpop`/`shortcircuit`), decoupler auto-bridging,
//!   decoupling/contention checks, contract voltage drives, static hold-law
//!   conditional drives, tolerance checks.
//! - unsupported (reported as empty, never approximated): data tables and
//!   table-indexed instance fields, bus (`[a..b]`) and wildcard (`[*]`)
//!   equalities, series synthesis, constitutive-law elaboration (the AST
//!   `electronics_laws`/`electronics_dc` passes), current/power proofs,
//!   budgets, net<>/stdnet<> membership, modes, and mechanism synthesis.
//!   These are listed in the module tests so the gap is recorded.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirHoldLaw, HirProgram, HirTypeDef};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::electronics::{
    format_amps, format_volts, ComponentInstance, ElectronicsNetlist, Net, PinClassProps, PinRef,
    TypeInfo, VoltageCheck,
};
use crate::ast::{BinaryOpKind, PropertyValue, QuantityDim, Type, UnaryOpKind};
use std::collections::{BTreeMap, HashSet};

/// Derive the netlist from typed HIR. Mirrors `analysis::electronics`.
pub fn derive_netlist(program: &HirProgram) -> ElectronicsNetlist {
    let (type_pins, type_info, class_errors) = collect_type_pins(program);
    if type_pins.is_empty() {
        return ElectronicsNetlist::default();
    }
    let (instance_list, data_errors) = collect_instances(program, &type_pins);
    let instances: BTreeMap<String, &ComponentInstance> =
        instance_list.iter().map(|c| (c.name.clone(), c)).collect();
    let type_props: BTreeMap<String, &BTreeMap<String, PropertyValue>> = program
        .type_defs
        .iter()
        .map(|td| (td.name.clone(), &td.metadata))
        .collect();
    let tables = Tables {
        instances: &instances,
        type_info: &type_info,
        type_pins: &type_pins,
        type_props: &type_props,
    };

    let mut ds = DisjointSet::new();
    let bus_errors = collect_pin_unions(program, &instances, &type_pins, &mut ds);

    let (unpop, shortcircuit, exempt_pins, participation_warnings, participation_notes) =
        collect_participation(program, &instances, &type_pins);
    let mut intent_proofs = infer_return_net(&instances, &type_info, &unpop, &mut ds);
    let mut convention_errors = Vec::new();
    intent_proofs.extend(force_decoupler_bridges(
        &mut ds,
        &tables,
        &unpop,
        &mut convention_errors,
    ));

    let (groups, nc_pins) = group_pins(&instances, &type_pins, &type_info, &mut ds, &exempt_pins);
    let (nets, dangling) = partition_nets(&groups, &nc_pins);

    let mut voltage = derive_voltage(program, &nets, &shortcircuit, &tables);
    post_solve_checks(program, &nets, &tables, &unpop, &mut voltage);

    convention_errors.extend(check_decoupling(&mut ds, &tables, &unpop));
    let contention_errors = check_contention(&nets, &instances, &type_info, &shortcircuit);

    ElectronicsNetlist {
        components: instance_list,
        nets,
        dangling,
        is_electronics: true,
        class_errors,
        convention_errors,
        budget_errors: Vec::new(),
        intent_errors: data_errors,
        intent_proofs,
        conditional_bridges: Vec::new(),
        voltage,
        type_info,
        unpop,
        shortcircuit,
        participation_notes,
        participation_warnings,
        contention_errors,
        bus_errors,
        law_errors: Vec::new(),
        component_laws: Vec::new(),
    }
}

// ── Metadata readers ────────────────────────────────────────────────────

fn property_string(pv: &PropertyValue) -> Option<String> {
    match pv {
        PropertyValue::String(s) | PropertyValue::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

fn property_bool(pv: &PropertyValue) -> Option<bool> {
    match pv {
        PropertyValue::Bool(b) => Some(*b),
        PropertyValue::Identifier(s) if s == "true" => Some(true),
        PropertyValue::Identifier(s) if s == "false" => Some(false),
        _ => None,
    }
}

fn envelope_values(metadata: &BTreeMap<String, PropertyValue>) -> (Option<f64>, Option<f64>) {
    let tolerance = match metadata.get("tolerance") {
        Some(PropertyValue::Quantity { si, dimension }) if *dimension == QuantityDim::Volt => {
            Some(*si)
        }
        Some(PropertyValue::Identifier(v)) if v == "any" => Some(f64::INFINITY),
        _ => None,
    };
    let rating = match metadata.get("rating") {
        Some(PropertyValue::Quantity { si, dimension }) if *dimension == QuantityDim::Watt => {
            Some(*si)
        }
        Some(PropertyValue::Identifier(v)) if v == "any" => Some(f64::INFINITY),
        _ => None,
    };
    (tolerance, rating)
}

fn resolve_pin_class_props(
    td: &HirTypeDef,
    pin: &super::program::HirPinDecl,
    class_defs: &BTreeMap<String, &BTreeMap<String, PropertyValue>>,
    class_errors: &mut Vec<String>,
) -> PinClassProps {
    let Some(cname) = pin.class_ref.as_deref() else {
        return PinClassProps::default();
    };
    match class_defs.get(cname) {
        Some(md) => PinClassProps {
            kicad_type: md
                .get("kicad_type")
                .and_then(property_string)
                .unwrap_or_else(|| "passive".to_string()),
            no_connect: md
                .get("no_connect")
                .and_then(property_bool)
                .unwrap_or(false),
            supply: md.get("supply").and_then(property_bool).unwrap_or(false),
            return_pin: md.get("return").and_then(property_bool).unwrap_or(false),
            can_drive: md.get("can_drive").and_then(property_bool).unwrap_or(false),
            control: md.get("control").and_then(property_bool).unwrap_or(false),
            switchable: md
                .get("switchable")
                .and_then(property_bool)
                .unwrap_or(false),
            wired_and: md.get("wired_and").and_then(property_bool).unwrap_or(false),
        },
        None => {
            class_errors.push(format!(
                "pin '{}.{}' ascribes class '{}' — no such type is in scope. Declare it (see std/electronics.bv: Power, Ground, In, Out, Io, IoOd, Nc) or drop the ascription",
                td.name, pin.name, cname
            ));
            PinClassProps::default()
        }
    }
}

// ── Type pins and instances ─────────────────────────────────────────────

fn collect_type_pins(
    program: &HirProgram,
) -> (
    BTreeMap<String, Vec<(String, u64)>>,
    BTreeMap<String, TypeInfo>,
    Vec<String>,
) {
    let class_defs: BTreeMap<String, &BTreeMap<String, PropertyValue>> = program
        .type_defs
        .iter()
        .map(|td| (td.name.clone(), &td.metadata))
        .collect();
    let mut pins_out = BTreeMap::new();
    let mut info_out = BTreeMap::new();
    let mut class_errors = Vec::new();
    for td in &program.type_defs {
        if td.pins.is_empty() {
            continue;
        }
        let prefix = td.reference.clone().unwrap_or_else(|| {
            td.name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "U".to_string())
        });
        let (tolerance, rating) = envelope_values(&td.metadata);
        let resistance = match td.metadata.get("resistance") {
            Some(PropertyValue::Quantity { si, dimension }) if *dimension == QuantityDim::Ohm => {
                Some(*si)
            }
            _ => None,
        };
        let mut pins: Vec<(String, u64)> =
            td.pins.iter().map(|p| (p.name.clone(), p.number)).collect();
        pins.sort_by_key(|&(_, n)| n);
        pins_out.insert(td.name.clone(), pins.clone());

        let default_unit = td
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "U".to_string());
        let (pin_units, unit_groups) = unit_layout(td, &pins, &default_unit);
        let pin_classes = pins
            .iter()
            .map(|(pname, _)| {
                let pin = td
                    .pins
                    .iter()
                    .find(|p| p.name == *pname)
                    .expect("declared pin");
                resolve_pin_class_props(td, pin, &class_defs, &mut class_errors)
            })
            .collect();

        let spec_defaults = td
            .metadata
            .iter()
            .filter(|(_, value)| {
                matches!(value, PropertyValue::Quantity { .. })
                    || matches!(value, PropertyValue::Int(n) if *n >= 0)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let bistable = matches!(td.metadata.get("bistable"), Some(PropertyValue::Bool(true)));
        info_out.insert(
            td.name.clone(),
            TypeInfo {
                reference_prefix: prefix,
                pins,
                pin_classes,
                tolerance,
                rating,
                resistance,
                has_laws: !td.hold_laws.is_empty() || !td.modes.is_empty(),
                spec_defaults,
                bistable,
                modes: td.modes.clone(),
                pin_units,
                unit_groups,
            },
        );
    }
    (pins_out, info_out, class_errors)
}

fn unit_layout(
    td: &HirTypeDef,
    sorted_pins: &[(String, u64)],
    default_unit: &str,
) -> (Vec<String>, Vec<(String, Vec<usize>)>) {
    let mut unit_order: Vec<String> = Vec::new();
    let mut unit_members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (decl_idx, pin) in td.pins.iter().enumerate() {
        let unit = pin.unit.clone().unwrap_or_else(|| default_unit.to_string());
        if !unit_order.contains(&unit) {
            unit_order.push(unit.clone());
        }
        unit_members.entry(unit).or_default().push(decl_idx);
    }
    let groups = unit_order
        .iter()
        .map(|u| (u.clone(), unit_members.get(u).cloned().unwrap_or_default()))
        .collect();
    let per_pin = sorted_pins
        .iter()
        .map(|(pname, _)| {
            td.pins
                .iter()
                .find(|p| p.name == *pname)
                .and_then(|p| p.unit.clone())
                .unwrap_or_else(|| default_unit.to_string())
        })
        .collect();
    (per_pin, groups)
}

fn collect_instances(
    program: &HirProgram,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> (Vec<ComponentInstance>, Vec<String>) {
    let mut out = Vec::new();
    for global in &program.globals {
        let Some(HirExpr {
            kind:
                HirExprKind::StructLiteral {
                    type_name,
                    fields,
                    specs,
                },
            ..
        }) = global.expr.as_ref()
        else {
            continue;
        };
        let declared = global.ty.as_ref().map_or("", |t| match t {
            Type::Custom(n) | Type::Applied(n, _) => n.as_str(),
            _ => "",
        });
        let type_name = if type_pins.contains_key(declared) {
            declared.to_string()
        } else if type_pins.contains_key(type_name) {
            type_name.clone()
        } else {
            continue;
        };
        let properties = fields
            .iter()
            .filter_map(|(fname, fexpr)| literal_instance_field(fname, fexpr))
            .collect();
        out.push(ComponentInstance {
            name: global.name.clone(),
            type_name,
            properties,
            specs: collect_spec_quantities(specs),
        });
    }
    (out, Vec::new())
}

fn literal_instance_field(fname: &str, fexpr: &HirExpr) -> Option<(String, String)> {
    match &fexpr.kind {
        HirExprKind::Literal(HirLiteral::Bytes(bytes)) => Some((
            fname.to_string(),
            String::from_utf8_lossy(bytes).into_owned(),
        )),
        HirExprKind::Literal(HirLiteral::Int(d)) => Some((fname.to_string(), d.to_string())),
        HirExprKind::Literal(HirLiteral::Float(f)) => Some((fname.to_string(), f.to_string())),
        HirExprKind::UnitLiteral { value, unit } => {
            Some((fname.to_string(), format!("{}{}", value, unit)))
        }
        _ => None,
    }
}

fn collect_spec_quantities(specs: &[(String, HirExpr)]) -> BTreeMap<String, PropertyValue> {
    let mut out = BTreeMap::new();
    for (name, expr) in specs {
        let HirExprKind::UnitLiteral { value, unit } = &expr.kind else {
            continue;
        };
        let Some(crate::parser::quantity::UnitSuffix::Explicit { scale, dim }) =
            crate::parser::quantity::parse_unit_suffix(unit)
        else {
            continue;
        };
        let key = crate::parser::spec_name_to_key(name)
            .map(str::to_string)
            .unwrap_or_else(|| name.to_lowercase());
        out.insert(
            key,
            PropertyValue::Quantity {
                si: value * scale,
                dimension: dim,
            },
        );
    }
    out
}

// ── Union-find and pin resolution ───────────────────────────────────────

struct DisjointSet {
    parent: BTreeMap<String, String>,
}

impl DisjointSet {
    fn new() -> Self {
        Self {
            parent: BTreeMap::new(),
        }
    }
    fn contains(&self, k: &str) -> bool {
        self.parent.contains_key(k)
    }
    fn make(&mut self, k: String) {
        self.parent.entry(k.clone()).or_insert(k);
    }
    fn find(&mut self, k: &str) -> String {
        let mut root = k.to_string();
        while let Some(parent) = self.parent.get(&root) {
            if *parent == root {
                break;
            }
            root = parent.clone();
        }
        let mut cur = k.to_string();
        while let Some(parent) = self.parent.get(&cur).cloned() {
            if parent == cur {
                break;
            }
            self.parent.insert(cur.clone(), root.clone());
            cur = parent;
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

/// Shared lookup tables for the electrical passes.
struct Tables<'a> {
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_info: &'a BTreeMap<String, TypeInfo>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    type_props: &'a BTreeMap<String, &'a BTreeMap<String, PropertyValue>>,
}

fn pin_key(component: &str, pin: &str) -> String {
    format!("{}\u{1F}{}", component, pin)
}

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

fn resolve_pin(
    expr: &HirExpr,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<PinRef> {
    let mut cur = expr;
    while let HirExprKind::Field { receiver, .. } = &cur.kind {
        if matches!(receiver.kind, HirExprKind::Field { .. }) {
            cur = receiver.as_ref();
        } else {
            break;
        }
    }
    if let HirExprKind::Index(inner, idx) = &cur.kind {
        let HirExprKind::Field {
            receiver,
            name: arr_name,
        } = &inner.kind
        else {
            return None;
        };
        let HirExprKind::Identifier(inst_name) = &receiver.kind else {
            return None;
        };
        let HirExprKind::Literal(HirLiteral::Int(d)) = &idx.kind else {
            return None;
        };
        return lookup_pin(
            inst_name,
            &format!("{}[{}]", arr_name, d),
            instances,
            type_pins,
        );
    }
    let HirExprKind::Field {
        receiver,
        name: pin_name,
    } = &cur.kind
    else {
        return None;
    };
    let (inst_name, pin_name) = match &receiver.kind {
        HirExprKind::Identifier(inst_name) => (inst_name.clone(), pin_name.clone()),
        HirExprKind::Index(inner, idx) => {
            let HirExprKind::Field {
                receiver: base,
                name: arr_name,
            } = &inner.kind
            else {
                return None;
            };
            let HirExprKind::Identifier(inst_name) = &base.kind else {
                return None;
            };
            let HirExprKind::Literal(HirLiteral::Int(d)) = &idx.kind else {
                return None;
            };
            (inst_name.clone(), format!("{}[{}]", arr_name, d))
        }
        _ => (instance_array_name(receiver)?, pin_name.clone()),
    };
    lookup_pin(&inst_name, &pin_name, instances, type_pins)
}

fn instance_array_name(expr: &HirExpr) -> Option<String> {
    let mut suffix = String::new();
    let mut cur = expr;
    loop {
        match &cur.kind {
            HirExprKind::Index(inner, idx) => {
                let HirExprKind::Literal(HirLiteral::Int(d)) = &idx.kind else {
                    return None;
                };
                suffix = format!("[{}]{}", d, suffix);
                cur = inner.as_ref();
            }
            HirExprKind::Identifier(name) if !suffix.is_empty() => {
                return Some(format!("{}{}", name, suffix));
            }
            _ => return None,
        }
    }
}

fn resolve_voltage_pin(
    expr: &HirExpr,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<PinRef> {
    let HirExprKind::Field { receiver, name } = &expr.kind else {
        return None;
    };
    if name != "voltage" {
        return None;
    }
    resolve_pin(receiver, instances, type_pins)
}

fn extract_numeric(expr: &HirExpr) -> Option<f64> {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Float(f)) => Some(*f),
        HirExprKind::Literal(HirLiteral::Int(d)) => Some(*d as f64),
        HirExprKind::UnitLiteral { value, unit } => {
            Some(crate::parser::quantity::literal_si(*value, unit))
        }
        _ => None,
    }
}

fn extract_quantity(expr: &HirExpr, expected: QuantityDim) -> Option<f64> {
    match &expr.kind {
        HirExprKind::Unary {
            op: UnaryOpKind::Neg,
            inner,
        } => extract_quantity(inner, expected).map(|v| -v),
        HirExprKind::UnitLiteral { value, unit } => {
            crate::parser::quantity::quantity_si(*value, unit, expected).map(|(si, _)| si)
        }
        _ => extract_numeric(expr),
    }
}

fn extract_voltage(expr: &HirExpr) -> Option<f64> {
    extract_quantity(expr, QuantityDim::Volt)
}

fn collect_eq_pairs<'a>(expr: &'a HirExpr, out: &mut Vec<(&'a HirExpr, &'a HirExpr)>) {
    if let HirExprKind::Binary { op, left, right } = &expr.kind {
        match op {
            BinaryOpKind::Eq => out.push((left, right)),
            BinaryOpKind::And | BinaryOpKind::Or => {
                collect_eq_pairs(left, out);
                collect_eq_pairs(right, out);
            }
            _ => {}
        }
    }
}

fn collect_pin_unions(
    program: &HirProgram,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    ds: &mut DisjointSet,
) -> Vec<String> {
    let mut pairs = Vec::new();
    for callable in bare_transactions(program) {
        let Some(contract) = &callable.contract else {
            continue;
        };
        collect_eq_pairs(&contract.pre, &mut pairs);
    }
    for (l, r) in pairs {
        let (Some(lp), Some(rp)) = (
            resolve_pin(l, instances, type_pins),
            resolve_pin(r, instances, type_pins),
        ) else {
            continue;
        };
        union_pins(ds, &lp, &rp);
    }
    Vec::new()
}

fn union_pins(ds: &mut DisjointSet, lp: &PinRef, rp: &PinRef) {
    let lk = pin_key(&lp.component, &lp.pin);
    let rk = pin_key(&rp.component, &rp.pin);
    ds.make(lk.clone());
    ds.make(rk.clone());
    ds.union(&lk, &rk);
}

/// Bare, non-exported, non-sync transaction callables — the AST walker's
/// `TopLevel::Transaction` reachability set.
fn bare_transactions(program: &HirProgram) -> impl Iterator<Item = &super::program::HirCallable> {
    program.callables.iter().filter(|c| {
        c.kind == super::program::HirCallableKind::Transaction
            && !c.exported
            && c.sync_domains.is_empty()
    })
}

// ── Participation, return nets, decoupling ──────────────────────────────

fn collect_participation(
    program: &HirProgram,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> (
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
    Vec<String>,
    Vec<String>,
) {
    let mut unpop = HashSet::new();
    let mut shortcircuit = HashSet::new();
    let mut exempt = HashSet::new();
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    for fact in &program.participations {
        let Some(inst) = instances.get(&fact.instance) else {
            warnings.push(format!(
                "participation fact names undeclared instance '{}' — declare it with `let`",
                fact.instance
            ));
            continue;
        };
        if fact.shortcircuit {
            shortcircuit.insert(fact.instance.clone());
        }
        unpop.insert(fact.instance.clone());
        let Some(ti) = type_pins.get(&inst.type_name) else {
            continue;
        };
        for (pname, _) in ti {
            exempt.insert(pin_key(&fact.instance, pname));
        }
        notes.push(format!(
            "participation: '{}' is unpopulated — verified in both the absent (pins open) and \
             present (part conducts) configurations",
            fact.instance
        ));
    }
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

fn infer_return_net(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    unpop: &HashSet<String>,
    ds: &mut DisjointSet,
) -> Vec<String> {
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

fn collect_no_connect_pins(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
) -> HashSet<String> {
    let mut nc = HashSet::new();
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

fn populated_return_root(
    ds: &mut DisjointSet,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    unpop: &HashSet<String>,
) -> Option<String> {
    for inst in instances.values() {
        if unpop.contains(&inst.name) {
            continue;
        }
        let Some(ti) = type_info.get(&inst.type_name) else {
            continue;
        };
        if let Some((pname, _)) = rail_pins(ti, true).first() {
            let key = pin_key(&inst.name, pname);
            ds.make(key.clone());
            return Some(ds.find(&key));
        }
    }
    None
}

fn is_decoupler_type(
    type_props: &BTreeMap<String, &BTreeMap<String, PropertyValue>>,
    type_name: &str,
) -> bool {
    type_props
        .get(type_name)
        .and_then(|m| m.get("decoupler"))
        .and_then(property_bool)
        .unwrap_or(false)
}

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

fn pins_free(
    ds: &mut DisjointSet,
    c: &ComponentInstance,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> bool {
    let Some(pins) = type_pins.get(&c.type_name) else {
        return false;
    };
    pins.iter().all(|(p, _)| !ds.contains(&pin_key(&c.name, p)))
}

fn bridges_rail(
    ds: &mut DisjointSet,
    cap: &ComponentInstance,
    tables: &Tables<'_>,
    s_root: &str,
    r_root: &str,
) -> bool {
    let Some(cp) = tables.type_pins.get(&cap.type_name) else {
        return false;
    };
    let Some(cti) = tables.type_info.get(&cap.type_name) else {
        return false;
    };
    cp.iter().enumerate().any(|(i1, (n1, _))| {
        ds.find(&pin_key(&cap.name, n1)) == s_root
            && cp.iter().enumerate().any(|(i2, (n2, _))| {
                i1 != i2
                    && !cti.pin_classes[i2].no_connect
                    && ds.find(&pin_key(&cap.name, n2)) == r_root
            })
    })
}

fn returns_of(
    ds: &mut DisjointSet,
    inst: &ComponentInstance,
    type_info: &BTreeMap<String, TypeInfo>,
) -> Vec<String> {
    let Some(ti) = type_info.get(&inst.type_name) else {
        return Vec::new();
    };
    rail_pins(ti, true)
        .iter()
        .map(|(rname, _)| ds.find(&pin_key(&inst.name, rname)))
        .collect()
}

fn force_decoupler_bridges(
    ds: &mut DisjointSet,
    tables: &Tables<'_>,
    unpop: &HashSet<String>,
    errors: &mut Vec<String>,
) -> Vec<String> {
    let mut proofs = Vec::new();
    let Some(ret) = populated_return_root(ds, tables.instances, tables.type_info, unpop) else {
        let demanded = tables.instances.values().any(|c| {
            !unpop.contains(&c.name)
                && tables
                    .type_props
                    .get(&c.type_name)
                    .map(|m| m.contains_key("decouple"))
                    .unwrap_or(false)
                && tables
                    .type_info
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
    let decouplers: Vec<&ComponentInstance> = tables
        .instances
        .values()
        .copied()
        .filter(|c| !unpop.contains(&c.name) && is_decoupler_type(tables.type_props, &c.type_name))
        .collect();
    let mut free_caps: Vec<&ComponentInstance> = decouplers
        .iter()
        .copied()
        .filter(|c| pins_free(ds, c, tables.type_pins))
        .collect();
    let mut obligations: Vec<(&ComponentInstance, String)> = Vec::new();
    for inst in tables.instances.values() {
        if unpop.contains(&inst.name) {
            continue;
        }
        if !tables
            .type_props
            .get(&inst.type_name)
            .map(|m| m.contains_key("decouple"))
            .unwrap_or(false)
        {
            continue;
        }
        let Some(ti) = tables.type_info.get(&inst.type_name) else {
            continue;
        };
        obligations.extend(rail_pins(ti, false).iter().map(|(n, _)| (*inst, n.clone())));
    }
    let mut state = BridgeState {
        decouplers,
        free_caps,
        tables,
        ret,
    };
    for (inst, pname) in &obligations {
        let Some(proof) = bridge_one_supply_pin(ds, &mut state, inst, pname) else {
            continue;
        };
        proofs.push(proof);
    }
    proofs
}

struct BridgeState<'a> {
    decouplers: Vec<&'a ComponentInstance>,
    free_caps: Vec<&'a ComponentInstance>,
    tables: &'a Tables<'a>,
    ret: String,
}

fn bridge_one_supply_pin(
    ds: &mut DisjointSet,
    state: &mut BridgeState<'_>,
    inst: &ComponentInstance,
    pname: &str,
) -> Option<String> {
    let s_key = pin_key(&inst.name, pname);
    ds.make(s_key.clone());
    let s_root = ds.find(&s_key);
    let t = state.tables;
    if state
        .decouplers
        .iter()
        .any(|cap| bridges_rail(ds, cap, t, &s_root, &state.ret))
    {
        return None;
    }
    let idx = state
        .free_caps
        .iter()
        .position(|cap| auto_bridgeable_pins(t.type_pins, t.type_info, &cap.type_name).is_some())?;
    let cap = state.free_caps.remove(idx);
    let names = auto_bridgeable_pins(t.type_pins, t.type_info, &cap.type_name)?;
    let (r_key, s_cap_key) = (pin_key(&cap.name, &names[0]), pin_key(&cap.name, &names[1]));
    ds.make(r_key.clone());
    ds.make(s_cap_key.clone());
    ds.union(&r_key, &state.ret);
    ds.union(&s_cap_key, &s_root);
    Some(format!(
        "decoupling convention: {} auto-bridged ({} <-> return net, {} <-> {}.{})",
        cap.name, names[0], names[1], inst.name, pname
    ))
}

fn check_decoupling(
    ds: &mut DisjointSet,
    tables: &Tables<'_>,
    unpop: &HashSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let decouplers: Vec<&ComponentInstance> = tables
        .instances
        .values()
        .filter(|c| !unpop.contains(&c.name) && is_decoupler_type(tables.type_props, &c.type_name))
        .copied()
        .collect();
    for inst in tables.instances.values() {
        let declares = tables
            .type_props
            .get(&inst.type_name)
            .map(|m| m.contains_key("decouple"))
            .unwrap_or(false);
        if !declares {
            continue;
        }
        let Some(ti) = tables.type_info.get(&inst.type_name) else {
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
            let s_root = ds.find(&pin_key(&inst.name, pname));
            let r_roots = returns_of(ds, inst, tables.type_info);
            let bridged = decouplers.iter().any(|cap| {
                r_roots
                    .iter()
                    .any(|r| bridges_rail(ds, cap, tables, &s_root, r))
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

fn group_pins(
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
    type_info: &BTreeMap<String, TypeInfo>,
    ds: &mut DisjointSet,
    exempt_pins: &HashSet<String>,
) -> (BTreeMap<String, Vec<PinRef>>, HashSet<String>) {
    let mut groups: BTreeMap<String, Vec<PinRef>> = BTreeMap::new();
    let mut all_pins = Vec::new();
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
    let mut nc_pins = collect_no_connect_pins(instances, type_info);
    nc_pins.extend(exempt_pins.iter().cloned());
    for p in &all_pins {
        let key = pin_key(&p.component, &p.pin);
        ds.make(key.clone());
        let root = ds.find(&key);
        groups.entry(root).or_default().push(p.clone());
    }
    (groups, nc_pins)
}

fn partition_nets(
    groups: &BTreeMap<String, Vec<PinRef>>,
    nc_pins: &HashSet<String>,
) -> (Vec<Net>, Vec<String>) {
    let mut nets = Vec::new();
    let mut dangling = Vec::new();
    let mut net_index = 0;
    for members in groups.values() {
        if members.len() >= 2 {
            net_index += 1;
            nets.push(Net {
                name: format!("N{}", net_index),
                pins: members.clone(),
                author_label: None,
            });
            continue;
        }
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
    (nets, dangling)
}

// ── Voltage drives ──────────────────────────────────────────────────────

fn pin_net_index(nets: &[Net]) -> BTreeMap<(String, String), String> {
    let mut out = BTreeMap::new();
    for net in nets {
        for p in &net.pins {
            out.insert((p.component.clone(), p.pin.clone()), net.name.clone());
        }
    }
    out
}

fn voltage_drive(
    l: &HirExpr,
    r: &HirExpr,
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

fn collect_drives(
    program: &HirProgram,
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<(String, f64, String)> {
    let mut drives = Vec::new();
    for callable in bare_transactions(program) {
        let Some(contract) = &callable.contract else {
            continue;
        };
        for cond in [&contract.pre, &contract.post] {
            let mut pairs = Vec::new();
            collect_eq_pairs(cond, &mut pairs);
            for (l, r) in pairs {
                let Some((pin, v)) = voltage_drive(l, r, instances, type_pins) else {
                    continue;
                };
                let Some(net_name) = pin_to_net.get(&(pin.component.clone(), pin.pin.clone()))
                else {
                    continue;
                };
                drives.push((
                    net_name.clone(),
                    v,
                    format!(
                        "'{}.{}.voltage == {}' in txn '{}'",
                        pin.component,
                        pin.pin,
                        format_volts(v),
                        callable.name
                    ),
                ));
            }
        }
    }
    drives
}

struct ConditionalDrive {
    net: String,
    volts: f64,
    guard: HirExpr,
    source: String,
}

struct DriveCtx<'a> {
    pin_to_net: &'a BTreeMap<(String, String), String>,
    instances: &'a BTreeMap<String, &'a ComponentInstance>,
    type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
}

fn collect_when_drives(
    program: &HirProgram,
    pin_to_net: &BTreeMap<(String, String), String>,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<ConditionalDrive> {
    let ctx = DriveCtx {
        pin_to_net,
        instances,
        type_pins,
    };
    let mut out = Vec::new();
    for law in &program.hold_laws {
        collect_when_fact_drives(law, &ctx, "top-level", &mut out);
    }
    for td in &program.type_defs {
        if td.hold_laws.is_empty() {
            continue;
        }
        for (inst_name, inst) in instances {
            if inst.type_name != td.name {
                continue;
            }
            for law in &td.hold_laws {
                let qualified = qualify_law(law, inst_name, td);
                collect_when_fact_drives(
                    &qualified,
                    &ctx,
                    &format!("type '{}' instance '{}'", td.name, inst_name),
                    &mut out,
                );
            }
        }
    }
    out
}

fn collect_when_fact_drives(
    law: &HirHoldLaw,
    ctx: &DriveCtx<'_>,
    source: &str,
    out: &mut Vec<ConditionalDrive>,
) {
    for fact in &law.facts {
        let pair = match &fact.kind {
            HirStmtKind::Assign(l, r) => Some((l, r)),
            HirStmtKind::Expression(e) => match &e.kind {
                HirExprKind::Binary {
                    op: BinaryOpKind::Eq,
                    left,
                    right,
                } => Some((left.as_ref(), right.as_ref())),
                _ => None,
            },
            _ => None,
        };
        let Some((l, r)) = pair else { continue };
        let Some((pin, v)) = voltage_drive(l, r, ctx.instances, ctx.type_pins) else {
            continue;
        };
        let Some(net) = ctx
            .pin_to_net
            .get(&(pin.component.clone(), pin.pin.clone()))
        else {
            continue;
        };
        out.push(ConditionalDrive {
            net: net.clone(),
            volts: v,
            guard: law.guard.clone(),
            source: format!("hold-law ({}) '{}.{}'", source, pin.component, pin.pin),
        });
    }
}

fn qualify_law(law: &HirHoldLaw, instance: &str, td: &HirTypeDef) -> HirHoldLaw {
    HirHoldLaw {
        guard: qualify_pin_refs(&law.guard, instance, td),
        facts: law
            .facts
            .iter()
            .map(|stmt| qualify_pin_refs_stmt(stmt, instance, td))
            .collect(),
    }
}

fn is_pin_name(td: &HirTypeDef, name: &str) -> bool {
    td.pins.iter().any(|p| p.name == name)
}

fn qualify_pin_refs(expr: &HirExpr, instance: &str, td: &HirTypeDef) -> HirExpr {
    let mut out = expr.clone();
    out.kind = match &expr.kind {
        HirExprKind::Identifier(name) if is_pin_name(td, name) => HirExprKind::Field {
            receiver: Box::new(HirExpr {
                id: expr.id,
                span: expr.span,
                ty: None,
                kind: HirExprKind::Identifier(instance.to_string()),
            }),
            name: name.clone(),
        },
        HirExprKind::Field { receiver, name } => HirExprKind::Field {
            receiver: Box::new(qualify_pin_refs(receiver, instance, td)),
            name: name.clone(),
        },
        HirExprKind::Binary { op, left, right } => HirExprKind::Binary {
            op: *op,
            left: Box::new(qualify_pin_refs(left, instance, td)),
            right: Box::new(qualify_pin_refs(right, instance, td)),
        },
        _ => return expr.clone(),
    };
    out
}

fn qualify_pin_refs_stmt(stmt: &HirStmt, instance: &str, td: &HirTypeDef) -> HirStmt {
    let mut out = stmt.clone();
    out.kind = match &stmt.kind {
        HirStmtKind::Assign(l, r) => HirStmtKind::Assign(
            qualify_pin_refs(l, instance, td),
            qualify_pin_refs(r, instance, td),
        ),
        HirStmtKind::Expression(e) => HirStmtKind::Expression(qualify_pin_refs(e, instance, td)),
        _ => return stmt.clone(),
    };
    out
}

fn derive_voltage(
    program: &HirProgram,
    nets: &[Net],
    shortcircuit: &HashSet<String>,
    tables: &Tables<'_>,
) -> VoltageCheck {
    let pin_to_net = pin_net_index(nets);
    let drives = collect_drives(program, &pin_to_net, tables.instances, tables.type_pins);
    let conditional = collect_when_drives(program, &pin_to_net, tables.instances, tables.type_pins);
    let mut check = classify_drives(drives, conditional, nets, shortcircuit);
    check_tolerance(
        nets,
        tables.instances,
        tables.type_info,
        &check.net_voltage,
        &mut check.violations,
    );
    check
}

fn classify_drives(
    drives: Vec<(String, f64, String)>,
    conditional: Vec<ConditionalDrive>,
    nets: &[Net],
    shortcircuit: &HashSet<String>,
) -> VoltageCheck {
    let mut check = VoltageCheck::default();
    let mut by_net: BTreeMap<String, Vec<(f64, Option<HirExpr>, String)>> = BTreeMap::new();
    for (net, v, src) in drives {
        by_net.entry(net).or_default().push((v, None, src));
    }
    for cd in conditional {
        by_net
            .entry(cd.net.clone())
            .or_default()
            .push((cd.volts, Some(cd.guard), cd.source));
    }
    for (net_name, ds) in by_net {
        let (in_force, contradictions) = drive_conflicts(&ds);
        if !contradictions.is_empty() {
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
        check.net_voltage.insert(
            net_name,
            in_force.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        );
    }
    check
}

fn drive_conflicts(ds: &[(f64, Option<HirExpr>, String)]) -> (Vec<f64>, Vec<String>) {
    let mut in_force = Vec::new();
    let mut contradictions = Vec::new();
    for i in 0..ds.len() {
        let (vi, guard_i, si) = &ds[i];
        if guard_satisfiable(guard_i.as_ref()) {
            in_force.push(*vi);
        }
        for (vj, guard_j, sj) in ds.iter().skip(i + 1) {
            if (vi - vj).abs() <= f64::EPSILON {
                continue;
            }
            if guards_joinsat(guard_i.as_ref(), guard_j.as_ref()) {
                contradictions.push(format!("'{}' vs '{}'", si, sj));
            }
        }
    }
    (in_force, contradictions)
}

/// HIR guard satisfiability for the unconditional-drive subset. A `None`
/// guard is unconditional. `true` is always satisfiable; `false` never.
/// Single-variable comparison ranges are conjoined and checked for an empty
/// interval. An unrecognised guard is conservatively satisfiable.
fn guards_joinsat(a: Option<&HirExpr>, b: Option<&HirExpr>) -> bool {
    let mut constraints: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for guard in [a, b].into_iter().flatten() {
        let Some(one) = guard_constraints(guard) else {
            continue;
        };
        for (key, (lo, hi)) in one {
            let entry = constraints
                .entry(key)
                .or_insert((f64::NEG_INFINITY, f64::INFINITY));
            entry.0 = entry.0.max(lo);
            entry.1 = entry.1.min(hi);
        }
    }
    constraints.values().all(|(lo, hi)| lo <= hi)
}

fn guard_satisfiable(guard: Option<&HirExpr>) -> bool {
    match guard {
        None => true,
        Some(g) => match &g.kind {
            HirExprKind::Literal(HirLiteral::Bool(b)) => *b,
            _ => guards_joinsat(Some(g), None),
        },
    }
}

fn guard_constraints(expr: &HirExpr) -> Option<BTreeMap<String, (f64, f64)>> {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Bool(true)) => Some(BTreeMap::new()),
        HirExprKind::Binary {
            op: BinaryOpKind::And,
            left,
            right,
        } => {
            let mut out = guard_constraints(left)?;
            out.extend(guard_constraints(right)?);
            Some(out)
        }
        HirExprKind::Binary { op, left, right } => comparison_constraint(*op, left, right)
            .or_else(|| comparison_constraint(*op, right, left)),
        _ => None,
    }
}

fn comparison_constraint(
    op: BinaryOpKind,
    pin: &HirExpr,
    literal: &HirExpr,
) -> Option<BTreeMap<String, (f64, f64)>> {
    let Some(value) = extract_numeric(literal) else {
        return None;
    };
    let key = expr_key(pin)?;
    let (lo, hi) = match op {
        BinaryOpKind::Eq => (value, value),
        BinaryOpKind::Ge => (value, f64::INFINITY),
        BinaryOpKind::Gt => (value, f64::INFINITY),
        BinaryOpKind::Le => (f64::NEG_INFINITY, value),
        BinaryOpKind::Lt => (f64::NEG_INFINITY, value),
        _ => return None,
    };
    let mut out = BTreeMap::new();
    out.insert(key, (lo, hi));
    Some(out)
}

fn expr_key(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Identifier(name) => Some(name.clone()),
        HirExprKind::Field { receiver, name } => Some(format!("{}.{}", expr_key(receiver)?, name)),
        HirExprKind::Index(array, idx) => Some(format!("{}[{:?}]", expr_key(array)?, idx.kind)),
        _ => None,
    }
}

fn tolerance_for(inst: &ComponentInstance, ti: &TypeInfo, pname: &str) -> Option<f64> {
    let pick = |pv: &PropertyValue| match pv {
        PropertyValue::Quantity { si, dimension } if *dimension == QuantityDim::Volt => Some(*si),
        _ => None,
    };
    inst.specs
        .get("tolerance")
        .and_then(pick)
        .or_else(|| {
            ti.spec_defaults
                .get(&format!("tolerance:{pname}"))
                .and_then(pick)
        })
        .or_else(|| ti.spec_defaults.get("tolerance").and_then(pick))
        .or(ti.tolerance)
}

fn check_tolerance(
    nets: &[Net],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    net_voltage: &BTreeMap<String, f64>,
    violations: &mut Vec<String>,
) {
    for net in nets {
        let Some(class) = net_voltage.get(&net.name).copied() else {
            continue;
        };
        for p in &net.pins {
            let Some(inst) = instances.get(&p.component) else {
                continue;
            };
            let Some(info) = type_info.get(&inst.type_name) else {
                continue;
            };
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
                    net.name,
                    format_volts(class),
                    p.component,
                    p.pin,
                    inst.type_name,
                    format_volts(tol),
                    format_volts(class)
                ));
            }
        }
    }
}

/// Post-solve checks over the HIR subset. The AST always yields one
/// `participation=present` state (plus `absent` when participation facts
/// exist), so the base drive diagnostics are re-checked once per state and
/// current-bound vacuity is reported — exactly the AST pipeline shape.
fn post_solve_checks(
    program: &HirProgram,
    nets: &[Net],
    tables: &Tables<'_>,
    unpop: &HashSet<String>,
    voltage: &mut VoltageCheck,
) {
    let mut labels = vec!["participation=present".to_string()];
    if !unpop.is_empty() {
        labels.push("participation=absent".to_string());
    }
    let mut proved_all = voltage.proved.clone();
    for label in &labels {
        let mut state_check = voltage.clone();
        state_check.violations.clear();
        check_tolerance(
            nets,
            tables.instances,
            tables.type_info,
            &state_check.net_voltage,
            &mut state_check.violations,
        );
        check_current_bounds(
            program,
            nets,
            tables,
            StateView { label, unpop },
            &mut state_check,
        );
        voltage.violations.extend(state_check.violations);
        proved_all.extend(state_check.proved);
    }
    voltage.proved = proved_all;
}

struct CurrentBound {
    transaction: String,
    pin: PinRef,
    bound: f64,
    is_upper: bool,
}

fn collect_compare_pairs<'a>(
    expr: &'a HirExpr,
    out: &mut Vec<(&'a HirExpr, BinaryOpKind, &'a HirExpr)>,
) {
    if let HirExprKind::Binary { op, left, right } = &expr.kind {
        match op {
            BinaryOpKind::Le | BinaryOpKind::Lt | BinaryOpKind::Ge | BinaryOpKind::Gt => {
                out.push((left, *op, right))
            }
            BinaryOpKind::And | BinaryOpKind::Or => {
                collect_compare_pairs(left, out);
                collect_compare_pairs(right, out);
            }
            _ => {}
        }
    }
}

fn collect_current_bounds(
    program: &HirProgram,
    instances: &BTreeMap<String, &ComponentInstance>,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<CurrentBound> {
    let resolve_current = |expr: &HirExpr| -> Option<PinRef> {
        let HirExprKind::Field { receiver, name } = &expr.kind else {
            return None;
        };
        if name != "current" {
            return None;
        }
        resolve_pin(receiver, instances, type_pins)
    };
    let mut out = Vec::new();
    for callable in bare_transactions(program) {
        let Some(contract) = &callable.contract else {
            continue;
        };
        let mut pairs = Vec::new();
        collect_compare_pairs(&contract.post, &mut pairs);
        for (l, op, r) in pairs {
            let (pin_expr, lit) = match (extract_current(r), extract_current(l)) {
                (Some(v), _) => (l, v),
                (None, Some(v)) => (r, v),
                _ => continue,
            };
            let Some(pin) = resolve_current(pin_expr) else {
                continue;
            };
            out.push(CurrentBound {
                transaction: callable.name.clone(),
                pin,
                bound: lit,
                is_upper: matches!(op, BinaryOpKind::Le | BinaryOpKind::Lt),
            });
        }
    }
    out
}

fn extract_current(expr: &HirExpr) -> Option<f64> {
    extract_quantity(expr, QuantityDim::Amp)
}

struct StateView<'a> {
    label: &'a str,
    unpop: &'a HashSet<String>,
}

fn check_current_bounds(
    program: &HirProgram,
    nets: &[Net],
    tables: &Tables<'_>,
    state: StateView<'_>,
    check: &mut VoltageCheck,
) {
    let pin_to_net = pin_net_index(nets);
    for bound in collect_current_bounds(program, tables.instances, tables.type_pins) {
        let key = (bound.pin.component.clone(), bound.pin.pin.clone());
        let Some(net_name) = pin_to_net.get(&key) else {
            continue;
        };
        // No law/DC or series derivation exists in the HIR subset, so a
        // stated bound is vacuous unless the part is absent in this state.
        let absent_state =
            state.label == "participation=absent" && state.unpop.contains(&bound.pin.component);
        if absent_state {
            continue;
        }
        let inst = tables.instances.get(&bound.pin.component);
        let type_name = inst.map(|c| c.type_name.as_str()).unwrap_or("?");
        check.violations.push(format!(
            "pin '{}.{}' (of {}) has a current bound (from '{}') but no derivable current — the component declares no law physics that solves it, so the bound would be vacuously proven. fix: declare the component's laws (`when` + spec parameters), or remove the bound",
            bound.pin.component, bound.pin.pin, type_name, bound.transaction
        ));
    }
}

fn check_contention(
    nets: &[Net],
    instances: &BTreeMap<String, &ComponentInstance>,
    type_info: &BTreeMap<String, TypeInfo>,
    shortcircuit: &HashSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for net in nets {
        let mut can_drive: Vec<(&str, &str, bool)> = Vec::new();
        for p in &net.pins {
            if shortcircuit.contains(&p.component) {
                continue;
            }
            let Some(inst) = instances.get(&p.component) else {
                continue;
            };
            let Some(ti) = type_info.get(&inst.type_name) else {
                continue;
            };
            let Some((i, _)) = ti.pins.iter().enumerate().find(|(_, (pn, _))| *pn == p.pin) else {
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
        if can_drive.iter().any(|(_, _, wired)| !wired) {
            let names = can_drive
                .iter()
                .map(|(c, p, _)| format!("{}.{}", c, p))
                .collect::<Vec<_>>()
                .join(", ");
            errors.push(format!(
                "net '{}' is driven by {} pins that are not open-drain ({}). Two drive-capable \
                 pins on one net is contention — a short unless the class declares `spec \
                 WiredAnd: true` (open-drain wired-AND). fix: make the extra drivers open-drain, \
                 or declare the short intentional with `shortcircuit unpop …;`",
                net.name,
                can_drive.len(),
                names
            ));
        }
    }
    errors
}

// Test-only canonical dump shared by AST/HIR parity assertions.
#[cfg(test)]
pub(crate) fn canonical_dump(nl: &ElectronicsNetlist) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("is_electronics".into(), nl.is_electronics.to_string());
    out.insert("components".into(), dump_components(&nl.components));
    out.insert("nets".into(), dump_nets(&nl.nets));
    out.insert("dangling".into(), nl.dangling.join("\n"));
    out.insert("type_info".into(), dump_type_info(nl));
    out.insert("voltage".into(), dump_voltage(&nl.voltage));
    out.insert("unpop".into(), dump_set("unpop", &nl.unpop));
    out.insert("shortcircuit".into(), dump_set("short", &nl.shortcircuit));
    for (key, value) in [
        ("class_errors", &nl.class_errors),
        ("convention_errors", &nl.convention_errors),
        ("budget_errors", &nl.budget_errors),
        ("intent_errors", &nl.intent_errors),
        ("intent_proofs", &nl.intent_proofs),
        ("conditional_bridges", &nl.conditional_bridges),
        ("participation_notes", &nl.participation_notes),
        ("participation_warnings", &nl.participation_warnings),
        ("contention_errors", &nl.contention_errors),
        ("bus_errors", &nl.bus_errors),
        ("law_errors", &nl.law_errors),
    ] {
        out.insert(key.into(), value.join("\n"));
    }
    // Constitutive-law IR is intentionally excluded (see module coverage).
    out
}

#[cfg(test)]
fn dump_components(components: &[ComponentInstance]) -> String {
    components
        .iter()
        .map(|c| {
            let props = c
                .properties
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            let specs = c
                .specs
                .iter()
                .map(|(k, v)| format!("{}={:?}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            format!("{}=>{}[{}]{{{}}}", c.name, c.type_name, props, specs)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn dump_nets(nets: &[Net]) -> String {
    nets.iter()
        .map(|n| {
            let pins = n
                .pins
                .iter()
                .map(|p| format!("{}.{}/{}", p.component, p.pin, p.number))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}:{}:{}",
                n.name,
                pins,
                n.author_label.clone().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn dump_type_info(nl: &ElectronicsNetlist) -> String {
    nl.type_info
        .iter()
        .map(|(name, ti)| {
            let pins = ti
                .pins
                .iter()
                .map(|(n, num)| format!("{}={}", n, num))
                .collect::<Vec<_>>()
                .join(",");
            let classes = ti
                .pin_classes
                .iter()
                .map(|c| {
                    format!(
                        "{}|{}|{}|{}|{}|{}|{}|{}",
                        c.kicad_type,
                        c.no_connect,
                        c.supply,
                        c.return_pin,
                        c.can_drive,
                        c.control,
                        c.switchable,
                        c.wired_and
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let defaults = ti
                .spec_defaults
                .iter()
                .map(|(k, v)| format!("{}={:?}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            let units = ti
                .unit_groups
                .iter()
                .map(|(u, idx)| format!("{}:{:?}", u, idx))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{}:ref={}:pins=[{}]:classes=[{}]:tol={:?}:rating={:?}:res={:?}:laws={}:defs=[{}]:bi={}:modes={:?}:units=[{}]:pu={:?}",
                name,
                ti.reference_prefix,
                pins,
                classes,
                ti.tolerance,
                ti.rating,
                ti.resistance,
                ti.has_laws,
                defaults,
                ti.bistable,
                ti.modes,
                units,
                ti.pin_units
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn dump_voltage(voltage: &VoltageCheck) -> String {
    format!(
        "nets={:?}\ncurrent={:?}\npin_current={:?}\nproved={:?}\nviolations={:?}",
        voltage.net_voltage,
        voltage.net_current,
        voltage.pin_current,
        voltage.proved,
        voltage.violations
    )
}

#[cfg(test)]
fn dump_set(label: &str, set: &HashSet<String>) -> String {
    let mut values: Vec<&String> = set.iter().collect();
    values.sort();
    format!("{}={:?}", label, values)
}

#[cfg(test)]
mod tests {
    //! AST-vs-HIR parity for the electronics netlist subset. Every case
    //! asserts exact canonical equality of the covered result surface.
    //!
    //! Unsupported constructs (must stay out of parity sources): data tables,
    //! bus/wildcard equalities, series synthesis, constitutive law/DC IR,
    //! current/power proofs, budgets, net<>/stdnet<> membership, modes, and
    //! mechanism synthesis.

    use super::*;
    use crate::ast::TopLevel;

    fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (items, program)
    }

    fn parity(src: &str) -> BTreeMap<String, String> {
        let (items, program) = parse_lower(src);
        let ast = canonical_dump(&crate::analysis::electronics::derive_netlist(&items));
        let hir = canonical_dump(&derive_netlist(&program));
        assert_eq!(hir, ast, "HIR/AST electronics drift:\n{src}");
        ast
    }

    #[test]
    fn empty_program_matches() {
        let dump = parity("");
        assert_eq!(dump["is_electronics"], "false");
        assert_eq!(parse_lower("").1.type_defs.len(), 0);
    }

    #[test]
    fn pin_declarations_reference_and_units_match() {
        let src = r#"
            type OpAmp {
                pin in_a = 1 unit A; pin in_b = 2 unit A; pin o = 3 unit A;
                pin in_c = 4 unit B; pin in_d = 5 unit B;
                reference "U"; spec Tolerance: any;
            };
        "#;
        let dump = parity(src);
        assert!(dump["type_info"].contains("ref=U"));
        assert!(dump["type_info"].contains("A:[0, 1, 2]"));
        assert!(dump["type_info"].contains("B:[3, 4]"));
    }

    #[test]
    fn components_and_precondition_nets_match() {
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; };
            type Led { pin a; pin k; reference "D"; };
            type Connector { pin vcc; pin gnd; reference "J"; };

            let j1: Connector = Connector { value: "JST-2" };
            let r1: Resistor = Resistor { value: "330" };
            let d1: Led = Led { value: "red" };

            txn powered
                [j1.vcc.voltage == r1.a.voltage && r1.b.voltage == d1.a.voltage && d1.k.voltage == j1.gnd.voltage]
                [d1.a.current > 0.0 && d1.a.current <= 0.02]
            { }
        "#;
        let dump = parity(src);
        assert!(dump["components"].contains("r1=>Resistor[value=330]"));
        assert_eq!(dump["nets"].lines().count(), 3);
        assert!(dump["dangling"].is_empty());
    }

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
        let dump = parity(src);
        assert!(dump["dangling"].is_empty());
        assert!(dump["class_errors"].is_empty());
    }

    #[test]
    fn unknown_class_is_a_hard_error() {
        let src = r#"
            type Widget { pin x: Bogus; reference "W"; };
        "#;
        let dump = parity(src);
        assert_eq!(dump["class_errors"].lines().count(), 1);
        assert!(dump["class_errors"].contains("Bogus"));
    }

    #[test]
    fn participation_unpop_opens_pins() {
        let src = r#"
            type Chip { pin a; pin b; reference "U"; };

            let u1: Chip = Chip { value: "a" };
            let u2: Chip = Chip { value: "b" };
            unpop u2;
        "#;
        let dump = parity(src);
        assert!(dump["unpop"].contains("u2"));
        assert!(dump["participation_notes"].contains("u2"));
        assert_eq!(dump["nets"], "");
    }

    #[test]
    fn decoupling_auto_bridging_matches() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
            type Cap { pin p; pin n; reference "C"; spec Decoupler: true; };
            type Header { pin p: Power; pin g: Ground; reference "J"; };

            let u1: Chip = Chip { value: "mcu" };
            let c1: Cap = Cap { value: "100n" };
            let j1: Header = Header { value: "hdr" };

            txn on [j1.p.voltage == u1.vdd.voltage && j1.g.voltage == u1.vss.voltage] { }
        "#;
        let dump = parity(src);
        assert!(dump["convention_errors"].is_empty());
        assert!(dump["intent_proofs"].contains("c1 auto-bridged"));
    }

    #[test]
    fn voltage_drive_and_tolerance_match() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Tolerance: 3.3V; };
            type Header { pin p: Power; pin g: Ground; reference "J"; spec Tolerance: any; };

            let u1: Chip = Chip { value: "x" };
            let j1: Header = Header { value: "y" };

            txn on
                [j1.p.voltage == u1.vdd.voltage && j1.g.voltage == u1.vss.voltage]
                [u1.vdd.voltage == 5.0V]
            { }
        "#;
        let dump = parity(src);
        assert!(dump["voltage"].contains("violations"));
        assert!(dump["voltage"].contains("tolerates only 3.3 V"));
    }

    #[test]
    fn hold_law_conditional_drive_matches() {
        let src = r#"
            type Power { pin p; pin g; reference "P"; spec Tolerance: any; };
            type Conn { pin p; pin g; reference "J"; spec Tolerance: any; };
            let u1: Power = Power { value: "lm1117" };
            let j1: Conn = Conn { value: "j" };
            let j2: Conn = Conn { value: "j" };
            txn n [u1.p.voltage == j1.p.voltage && u1.g.voltage == j1.g.voltage
                   && u1.p.voltage == j2.p.voltage && u1.g.voltage == j2.g.voltage] { };
            hold u1.g.voltage >= 5V { u1.p.voltage = 3.3V; }
        "#;
        let dump = parity(src);
        assert!(dump["voltage"].contains("N1") || dump["voltage"].contains("N2"));
        assert!(dump["voltage"].contains("3.3"));
        assert!(!dump["voltage"].contains("shorted supply"));
    }

    #[test]
    fn mutually_exclusive_hold_guards_match() {
        let src = r#"
            type Switch { pin sel; pin vout; reference "U"; spec Tolerance: any; };
            let u1: Switch = Switch { value: "x" };
            txn n [u1.sel.voltage == u1.vout.voltage] { };
            hold u1.sel.voltage == 3.3V { u1.vout.voltage = 1.8V; }
            hold u1.sel.voltage == 1.8V { u1.vout.voltage = 3.3V; }
        "#;
        let dump = parity(src);
        assert!(!dump["voltage"].contains("shorted supply"));
    }

    #[test]
    fn contention_between_drive_capable_pins_matches() {
        let src = r#"
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Mcu { pin p1: Io; pin p2: Io; reference "U"; spec Tolerance: any; };
            let u1: Mcu = Mcu { value: "u" };
            txn n [u1.p1.voltage == u1.p2.voltage] { };
        "#;
        let dump = parity(src);
        assert!(dump["contention_errors"].contains("contention"));
    }

    #[test]
    fn pin_array_elements_wire_by_index() {
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
        let dump = parity(src);
        assert!(dump["dangling"].is_empty());
        assert_eq!(dump["nets"].lines().count(), 4);
    }

    #[test]
    fn shortcircuit_participation_warns_on_populated_part() {
        let src = r#"
            type Chip { pin a; pin b; reference "U"; };
            let u1: Chip = Chip { value: "x" };
            shortcircuit u1;
        "#;
        let dump = parity(src);
        assert!(dump["shortcircuit"].contains("u1"));
        assert!(dump["participation_warnings"].contains("definite short"));
    }

    #[test]
    fn missing_decoupling_is_a_convention_error() {
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };
            type Header { pin p: Power; pin g: Ground; reference "J"; };

            let u1: Chip = Chip { value: "mcu" };
            let j1: Header = Header { value: "hdr" };

            txn on [j1.p.voltage == u1.vdd.voltage && j1.g.voltage == u1.vss.voltage] { }
        "#;
        let dump = parity(src);
        assert!(dump["convention_errors"].contains("no decoupling part bridges supply pin 'vdd'"));
    }
}
