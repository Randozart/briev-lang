// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (H5 full-surface port): the complete electronics netlist over
//! typed HIR.
//!
//! Mirrors the full consumer surface of `analysis::electronics` (the AST
//! remains the oracle until the KiCad consumer flips; parity is asserted by
//! exact canonical dumps). Every fact is derived from HIR nodes — types,
//! globals, contracts, bodies, hold laws, modes, data tables, budgets —
//! never by lowering back to the AST. The public result type is reused
//! unchanged so a later consumer flip needs no adapter.
//!
//! Coverage (the full AST pass is ~12k lines):
//! - pins + class properties + units, instances (literal AND table-indexed
//!   fields), precondition pin equalities (plain, bus `[a..b]`, wildcard
//!   `[*]`), return nets, participation (`unpop`/`shortcircuit`),
//!   `net<>`/`stdnet<>` membership (the four-rung ladder) and author net
//!   labels, decoupler auto-bridging, decoupling/contention checks,
//!   contract voltage drives, static hold-law conditional drives,
//!   mutually-exclusive guard reasoning, tolerance checks, node-body
//!   intents (wires, drive intents, `open` disconnections, voltage/current
//!   obligations with pull-up/low-hold/series forcing and bus assembly),
//!   mechanism synthesis (`when … { … } through <Type>`), series synthesis
//!   (E-series window), constitutive-law elaboration (`super::
//!   electronics_laws`, HIR recursion over shared pure algebra), the DC
//!   operating-point solve (reused verbatim from `analysis::
//!   electronics_dc` — its inputs are only shared result types), named
//!   modes (catalog, per-mode DC, node mode validation), current/power
//!   proofs (B4 fixpoint, law power, max-current, drive capability),
//!   budgets.
//! - Known HIR-side deviation, documented: guard joint-satisfiability uses
//!   the interval-based HIR reasoner (unrecognised guards are conservatively
//!   satisfiable) instead of the AST's proof-engine call; recognised forms
//!   agree.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirDataTable, HirHoldLaw, HirProgram, HirTypeDef};
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
    let (mut instance_list, data_errors) = collect_instances(program, &type_pins);
    let instances: BTreeMap<String, &ComponentInstance> =
        instance_list.iter().map(|c| (c.name.clone(), c)).collect();
    let type_props: BTreeMap<String, &BTreeMap<String, PropertyValue>> = program
        .type_defs
        .iter()
        .map(|td| (td.name.clone(), &td.metadata))
        .collect();

    let mut ds = DisjointSet::new();
    let bus_errors = collect_pin_unions(program, &instances, &type_pins, &mut ds);

    // 2026-11-12 (full-surface port): probe the law elaboration once to
    // learn the law-born rail volts, synthesize unstated series-part
    // resistances from the E-series window, then re-elaborate (the AST
    // pipeline's E15 slice-2 shape). The maps are rebuilt after the mutation.
    let probe_laws = {
        let probe_instances: BTreeMap<String, &ComponentInstance> =
            instance_list.iter().map(|c| (c.name.clone(), c)).collect();
        super::electronics_laws::elaborate_component_laws(
            program,
            &crate::analysis::electronics_laws::LawContext {
                instances: &probe_instances,
                type_info: &type_info,
                type_pins: &type_pins,
            },
        )
        .components
    };
    // `probe_instances` is dropped here, releasing the borrow before the
    // synthesis pass mutates the instance list (specs/value injection).
    let mut synthesis_proofs = Vec::new();
    let synthesis_tables = SynthesisTables {
        type_pins: &type_pins,
        type_info: &type_info,
        type_props: &type_props,
    };
    let synthesis_errors = synthesize_series_values(
        program,
        &mut instance_list,
        &ds,
        &synthesis_tables,
        &probe_laws,
        &mut synthesis_proofs,
    );
    let instances: BTreeMap<String, &ComponentInstance> =
        instance_list.iter().map(|c| (c.name.clone(), c)).collect();

    // 2026-11-12 (full-surface port): the real law elaboration — before
    // topology-dependent checks; the DC solve consumes the IR post-drive.
    let law_ir = super::electronics_laws::elaborate_component_laws(
        program,
        &crate::analysis::electronics_laws::LawContext {
            instances: &instances,
            type_info: &type_info,
            type_pins: &type_pins,
        },
    );
    let (mut law_errors, component_laws) = (law_ir.errors, law_ir.components);

    // The post-synthesis tables — rebuilt from the mutated instance list
    // (the E-series pass injects specs/values), exactly like the AST map
    // rebuild.
    let instances: BTreeMap<String, &ComponentInstance> =
        instance_list.iter().map(|c| (c.name.clone(), c)).collect();
    let tables = Tables {
        instances: &instances,
        type_info: &type_info,
        type_pins: &type_pins,
        type_props: &type_props,
    };

    // 2026-11-12 (full-surface port): return topology in dependency order —
    // participation, return net, rail membership (the ladder), decoupler
    // auto-bridging. Forcing precedes the intent pass so completions see
    // final nets.
    let (unpop, shortcircuit, exempt_pins, participation_warnings, participation_notes) =
        collect_participation(program, &instances, &type_pins);
    let mut topology_proofs = infer_return_net(&instances, &type_info, &unpop, &mut ds);
    let mut convention_errors = Vec::new();
    let net_bindings = infer_rail_membership_hir(
        program,
        &tables,
        &component_laws,
        &unpop,
        &mut ds,
        &mut topology_proofs,
        &mut convention_errors,
    );
    topology_proofs.extend(force_decoupler_bridges(
        &mut ds,
        &tables,
        &unpop,
        &mut convention_errors,
    ));

    // 2026-11-12 (full-surface port): node-body intents — wiring facts,
    // drive intents, mechanism bridges, voltage/current obligations, opens.
    let (mut intent_errors, mut body_proofs, conditional_bridges) =
        run_intent_pass(program, &tables, &component_laws, &mut ds);
    intent_errors.extend(synthesis_errors);
    intent_errors.extend(data_errors);
    // AST order: intent + synthesis proofs first, topology/return proofs last.
    let mut intent_proofs = body_proofs.drain(..).collect::<Vec<_>>();
    intent_proofs.extend(synthesis_proofs);
    intent_proofs.extend(topology_proofs);

    // 2026-11-12 (full-surface port): author net labels keyed by FINAL root,
    // resolved after the intent pass (body wiring merges nets further).
    let net_labels = resolve_net_labels(&net_bindings, &mut ds);

    let (groups, nc_pins) = group_pins(&instances, &type_pins, &type_info, &mut ds, &exempt_pins);
    let (nets, dangling) = partition_nets(&groups, &nc_pins, &net_labels);

    let mut voltage = derive_voltage(program, &nets, &shortcircuit, &tables);

    // 2026-11-12 (full-surface port): law solve, current/power proofs, and
    // budgets in one post-drive pipeline; then the finished-netlist checks.
    let dc_drives = merge_law_rail_boundaries(&nets, &component_laws, &tables, voltage.net_voltage.clone());
    let (law_solve_errors, budget_errors) =
        post_solve_checks(program, &nets, &tables, &component_laws, &unpop, &dc_drives, &mut voltage);
    law_errors.extend(law_solve_errors);

    convention_errors.extend(check_decoupling(&mut ds, &tables, &unpop));
    let contention_errors = check_contention(&nets, &instances, &type_info, &shortcircuit);

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
        law_errors,
        component_laws,
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

pub(crate) fn collect_type_pins(
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

pub(crate) fn collect_instances(
    program: &HirProgram,
    type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> (Vec<ComponentInstance>, Vec<String>) {
    let mut errors = Vec::new();
    let tables = collect_data_tables(program, &mut errors);
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
            .filter_map(|(fname, fexpr)| {
                resolve_instance_field(fname, fexpr, &tables, &mut errors)
            })
            .collect();
        out.push(ComponentInstance {
            name: global.name.clone(),
            type_name,
            properties,
            specs: collect_spec_quantities(specs),
        });
    }
    (out, errors)
}

/// The declared data tables by name (2026-09-27 E2, HIR port). A duplicate
/// name is a hard error naming it; the first declaration wins for resolution.
fn collect_data_tables<'a>(
    program: &'a HirProgram,
    errors: &mut Vec<String>,
) -> BTreeMap<String, &'a HirDataTable> {
    let mut tables = BTreeMap::new();
    for t in &program.data_tables {
        if tables.insert(t.name.clone(), t).is_some() {
            errors.push(format!(
                "data table '{}' is declared more than once — rename one declaration so index \
 resolution is unambiguous",
 t.name
 ));
 }
 }
 tables
}

/// One instance-literal field → (name, string value), or None when the
/// field carries no schematic property. Literal forms keep their shape; a
/// table-indexed field resolves to its cell; everything else stays
/// property-less. A table resolution failure pushes a hard error.
fn resolve_instance_field(
 fname: &str,
 fexpr: &HirExpr,
 tables: &BTreeMap<String, &HirDataTable>,
 errors: &mut Vec<String>,
) -> Option<(String, String)> {
 if let Some(v) = literal_instance_field(fname, fexpr) {
 return Some(v);
 }
 resolve_table_instance_field(fname, fexpr, tables, errors)
}

/// Resolve `value: tbl[i…]` to the cell's literal string (2026-09-27 E2/D2,
/// HIR port). Bare non-table identifiers and Field-rooted chains stay
/// property-less; every failure names the table and index.
fn resolve_table_instance_field(
 fname: &str,
 fexpr: &HirExpr,
 tables: &BTreeMap<String, &HirDataTable>,
 errors: &mut Vec<String>,
) -> Option<(String, String)> {
 let (root, idxs) = table_index_chain(fexpr);
 let HirExprKind::Identifier(root_name) = &root.kind else {
 return None;
 };
 if idxs.as_ref().is_some_and(Vec::is_empty) {
 if tables.contains_key(root_name) {
 errors.push(format!(
 "the '{}' field names data table '{}' without an index — state the cell, for \
 example '{}[0]'",
 fname, root_name, root_name
 ));
 }
 return None;
 }
 let Some(table) = tables.get(root_name) else {
 errors.push(format!(
 "the '{}' field indexes '{}', which is not a declared data table — declare it as \
 'data {}' or name a table that exists",
 fname, root_name, root_name
 ));
 return None;
 };
 let Some(idxs) = idxs else {
 errors.push(format!(
 "the '{}' field indexes data table '{}' with a non-literal index — use integer \
 literals, for example '{}[0]'",
 fname, root_name, root_name
 ));
 return None;
 };
 if idxs.len() != table.dims.len() {
 errors.push(format!(
 "the '{}' field indexes data table '{}' with {} indices, but the table declares {} \
 dimensions — state every index",
 fname,
 root_name,
 idxs.len(),
 table.dims.len()
 ));
 return None;
 }
 let Some(cell) = table_cell(table, &idxs) else {
 let shown = idxs
 .iter()
 .map(i64::to_string)
 .collect::<Vec<_>>()
 .join(", ");
 let extents = table
 .dims
 .iter()
 .map(|d| format!("[{}]", d))
 .collect::<String>();
 errors.push(format!(
 "the '{}' field indexes data table '{}' at [{}], out of range — the table declares {}",
 fname, root_name, shown, extents
 ));
 return None;
 };
 Some((fname.to_string(), cell.to_string()))
}

/// Flatten `tbl[a][b]` → (root expr, indices outermost-last). A non-literal
/// anywhere in the chain yields None for the indices.
fn table_index_chain(expr: &HirExpr) -> (&HirExpr, Option<Vec<i64>>) {
 let mut idxs = Vec::new();
 let mut cur = expr;
 let mut ok = true;
 while let HirExprKind::Index(inner, idx) = &cur.kind {
 match &idx.kind {
 HirExprKind::Literal(HirLiteral::Int(d)) => idxs.push(*d),
 _ => ok = false,
 }
 cur = inner.as_ref();
 }
 idxs.reverse();
 (cur, if ok { Some(idxs) } else { None })
}

/// Row-major cell lookup: dims `[D0][D1]…` with indices `[i0][i1]…` →
/// `flat = ((i0·D1 + i1)·D2 + i2)…`. None when any index is out of range.
fn table_cell(table: &HirDataTable, idxs: &[i64]) -> Option<i64> {
 let mut flat: u64 = 0;
 for (d, &i) in idxs.iter().enumerate() {
 if i < 0 || i as u64 >= table.dims[d] {
 return None;
 }
 flat = flat * table.dims[d] + i as u64;
 }
 table.cells.get(flat as usize).copied()
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
 if let HirExprKind::Field {
 receiver: base,
 name: arr_name,
 } = &inner.kind {
 let HirExprKind::Identifier(inst_name) = &base.kind else {
 return None;
 };
 let HirExprKind::Literal(HirLiteral::Int(d)) = &idx.kind else {
 return None;
 };
 (inst_name.clone(), format!("{}[{}]", arr_name, d))
 } else {
 (instance_array_name(receiver)?, pin_name.clone())
 }
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
 let mut errors = Vec::new();
 let mut pairs = Vec::new();
 for callable in all_transactions(program) {
 let Some(contract) = &callable.contract else {
 continue;
 };
 collect_eq_pairs(&contract.pre, &mut pairs);
 }
 for (l, r) in pairs {
 // 2026-09-22 (whole-bus equality, HIR port): a range-indexed pair
 // expands to N element unions; a length mismatch is a hard error.
 match expand_bus_pair(&l, &r, instances, type_pins) {
 BusPair::Pairs(expanded) => {
 for (lp, rp) in expanded {
 union_pins(ds, &lp, &rp);
 }
 continue;
 }
 BusPair::LengthMismatch(e) => {
 errors.push(e);
 continue;
 }
 BusPair::NotBus => {}
 }
 // 2026-09-28 (E3, HIR port): a `[*]` wildcard on either side
 // expands to every element; `scalar == wildcard` broadcasts.
 let wc = wildcard_bus_access(&l);
 let wr = wildcard_bus_access(&r);
 if wc.is_some() || wr.is_some() {
 apply_wildcard_pair(&l, &r, &wc, &wr, instances, type_pins, ds, &mut errors);
 continue;
 }
 let (Some(lp), Some(rp)) = (
 resolve_pin(l, instances, type_pins),
 resolve_pin(r, instances, type_pins),
 ) else {
 continue;
 };
 union_pins(ds, &lp, &rp);
 }
 errors
}

/// Bus-pair classification: both sides range-indexed (the element pairs),
/// a bus comparison with mismatched lengths, or not a bus at all.
enum BusPair {
 NotBus,
 Pairs(Vec<(PinRef, PinRef)>),
 LengthMismatch(String),
}

fn union_pins(ds: &mut DisjointSet, lp: &PinRef, rp: &PinRef) {
 let lk = pin_key(&lp.component, &lp.pin);
 let rk = pin_key(&rp.component, &rp.pin);
 ds.make(lk.clone());
 ds.make(rk.clone());
 ds.union(&lk, &rk);
}

/// Expand `u2.gpio[0..7] == u3.data[0..7]` into the element pairs.
fn expand_bus_pair(
 l: &HirExpr,
 r: &HirExpr,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> BusPair {
 let (Some((li, la, lo, hi, lincl)), Some((ri, ra, ro, rh, rincl))) =
 (range_index_pin(l), range_index_pin(r))
 else {
 return BusPair::NotBus;
 };
 let llen = if lincl { hi - lo + 1 } else { hi - lo };
 let rlen = if rincl { rh - ro + 1 } else { rh - ro };
 if llen != rlen {
 return BusPair::LengthMismatch(format!(
 "whole-bus equality '{}' ({} elements) vs '{}' ({} elements) — the buses must be the \
 same length. fix: use matching ranges (e.g. `[0..7]` on both sides).",
 dump_expr(l),
 llen,
 dump_expr(r),
 rlen
 ));
 }
 if lo > hi || ro > rh {
 return BusPair::LengthMismatch(format!(
 "whole-bus equality '{}' vs '{}' — an empty or reversed range",
 dump_expr(l),
 dump_expr(r)
 ));
 }
 let mut out = Vec::with_capacity(llen as usize);
 for k in 0..llen {
 let (Some(lp), Some(rp)) = (
 lookup_pin(&li, &format!("{}[{}]", la, lo + k), instances, type_pins),
 lookup_pin(&ri, &format!("{}[{}]", ra, ro + k), instances, type_pins),
 ) else {
 return BusPair::NotBus;
 };
 out.push((lp, rp));
 }
 BusPair::Pairs(out)
}

/// `u2.gpio[0..7].voltage` → (inst, arr, lo, hi, inclusive). Any other
/// shape → None. The HIR shape is Field("voltage") over Index(Range).
fn range_index_pin(expr: &HirExpr) -> Option<(String, String, u64, u64, bool)> {
 let HirExprKind::Field { receiver, name } = &expr.kind else {
 return None;
 };
 if name != "voltage" {
 return None;
 }
 let HirExprKind::Index(base, idx) = &receiver.kind else {
 return None;
 };
 let HirExprKind::Range {
 start,
 end,
 inclusive,
 } = &idx.kind
 else {
 return None;
 };
 let HirExprKind::Field {
 receiver: inst_expr,
 name: arr,
 } = &base.kind
 else {
 return None;
 };
 let HirExprKind::Identifier(inst_name) = &inst_expr.kind else {
 return None;
 };
 let (HirExprKind::Literal(HirLiteral::Int(lo)), HirExprKind::Literal(HirLiteral::Int(hi))) =
 (&start.kind, &end.kind)
 else {
 return None;
 };
 Some((inst_name.clone(), arr.clone(), *lo as u64, *hi as u64, *inclusive))
}

/// 2026-09-28 (E3, HIR port): expand a `[*]` wildcard pair and union the
/// resulting element pins. Wildcard == wildcard is element-wise (paired by
/// flattened index; mismatch is a hard error); scalar == wildcard
/// broadcasts.
fn apply_wildcard_pair(
 l: &HirExpr,
 r: &HirExpr,
 wc: &Option<WildcardBus>,
 wr: &Option<WildcardBus>,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
 ds: &mut DisjointSet,
 errors: &mut Vec<String>,
) {
 match (wc, wr) {
 (Some(lb), Some(rb)) => {
 let le = expand_wildcard(lb, instances, type_pins);
 let re = expand_wildcard(rb, instances, type_pins);
 if le.len() != re.len() {
 errors.push(format!(
 "whole-bus equality '{}' ({} elements) vs '{}' ({} elements) — the buses \
 must be the same length",
 dump_expr(l),
 le.len(),
 dump_expr(r),
 re.len()
 ));
 return;
 }
 for (lp, rp) in le.iter().zip(re.iter()) {
 union_pins(ds, lp, rp);
 }
 }
 (Some(lb), None) => union_broadcast(lb, r, instances, type_pins, ds),
 (None, Some(rb)) => union_broadcast(rb, l, instances, type_pins, ds),
 (None, None) => {}
 }
}

/// Broadcast — `scalar == wildcard`: the scalar pin joins every expanded
/// element on one net.
fn union_broadcast(
 bus: &WildcardBus,
 scalar_expr: &HirExpr,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
 ds: &mut DisjointSet,
) {
 let elements = expand_wildcard(bus, instances, type_pins);
 let Some(sp) = resolve_pin(scalar_expr, instances, type_pins) else {
 return;
 };
 let sk = pin_key(&sp.component, &sp.pin);
 ds.make(sk.clone());
 for ep in &elements {
 let ek = pin_key(&ep.component, &ep.pin);
 ds.make(ek.clone());
 ds.union(&sk, &ek);
 }
}

/// A `[*]` wildcard element access in a pin equality, two shapes: a PIN
/// array on a concrete instance (`u2.gpio[*].voltage`), or an INSTANCE
/// array (`t[*].a.voltage`).
enum WildcardBus {
 PinArray { inst: String, arr: String },
 InstanceArray { arr: String, pin: String },
}

/// Peel `Index(_, Wildcard)` layers down to a bare `Identifier` root.
fn wildcard_instance_root(expr: &HirExpr) -> Option<String> {
 let mut cur = expr;
 let mut saw_wildcard = false;
 loop {
 match &cur.kind {
 HirExprKind::Index(inner, idx) => {
 if !matches!(&idx.kind, HirExprKind::Wildcard) {
 return None;
 }
 saw_wildcard = true;
 cur = inner.as_ref();
 }
 HirExprKind::Identifier(name) if saw_wildcard => return Some(name.clone()),
 _ => return None,
 }
 }
}

/// Recognize a `[*]` wildcard pin access (the two shapes). None for a
/// scalar pin access — the caller falls back to single-pin resolution.
fn wildcard_bus_access(expr: &HirExpr) -> Option<WildcardBus> {
 let HirExprKind::Field { receiver, name } = &expr.kind else {
 return None;
 };
 if name != "voltage" {
 return None;
 }
 match &receiver.kind {
 HirExprKind::Index(base, idx) => {
 if !matches!(&idx.kind, HirExprKind::Wildcard) {
 return None;
 }
 let HirExprKind::Field {
 receiver: inst_expr,
 name: arr,
 } = &base.kind
 else {
 return None;
 };
 let HirExprKind::Identifier(inst_name) = &inst_expr.kind else {
 return None;
 };
 Some(WildcardBus::PinArray {
 inst: inst_name.clone(),
 arr: arr.clone(),
 })
 }
 HirExprKind::Field { receiver: base, name: pin } => {
 let arr = wildcard_instance_root(base)?;
 Some(WildcardBus::InstanceArray {
 arr,
 pin: pin.clone(),
 })
 }
 _ => None,
 }
}

/// The numeric index tuple inside an array-element name (`t[3][17]` →
/// `[3, 17]`), so element names order row-major, not lexicographically.
fn element_index_key(name: &str) -> Vec<i64> {
 let mut out = Vec::new();
 let mut i = 0;
 let bytes = name.as_bytes();
 while i < name.len() {
 if bytes[i] == b'[' {
 let end = name[i..].find(']').map(|k| i + k).unwrap_or(name.len());
 let digits = &name[i + 1..end.min(name.len())];
 if let Ok(n) = digits.parse::<i64>() {
 out.push(n);
 }
 i = end;
 } else {
 i += 1;
 }
 }
 out
}

/// Expand a `[*]` wildcard bus to its element `PinRef`s in declaration
/// (row-major) order. Pin-array wildcards follow pin-number order;
/// instance-array wildcards follow the numeric index tuple.
fn expand_wildcard(
 bus: &WildcardBus,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Vec<PinRef> {
 match bus {
 WildcardBus::PinArray { inst, arr } => {
 let Some(comp) = instances.get(inst) else {
 return Vec::new();
 };
 let Some(pins) = type_pins.get(&comp.type_name) else {
 return Vec::new();
 };
 let prefix = format!("{}[", arr);
 let mut els: Vec<&(String, u64)> = pins
 .iter()
 .filter(|(n, _)| n.starts_with(&prefix))
 .collect();
 els.sort_by_key(|(n, _)| element_index_key(n));
 els.iter()
 .map(|(n, num)| PinRef {
 component: inst.clone(),
 pin: n.clone(),
 number: *num,
 })
 .collect()
 }
 WildcardBus::InstanceArray { arr, pin } => {
 let prefix = format!("{}[", arr);
 let mut names: Vec<String> = instances
 .keys()
 .filter(|k| k.starts_with(&prefix))
 .cloned()
 .collect();
 names.sort_by_key(|k| element_index_key(k));
 names
 .iter()
 .filter_map(|name| lookup_pin(name, pin, instances, type_pins))
 .collect()
 }
 }
}

/// Source text of an expression for diagnostics (the AST oracle formats
/// `Expr` via Display; HIR reconstructs the bounded electrical fragment).
fn dump_expr(expr: &HirExpr) -> String {
 super::to_ast::expr(expr).map_or_else(
 || "<unrepresentable>".to_string(),
 |e| e.to_string(),
 )
}

/// ALL top-level transaction callables — the AST analysis scans every
/// `TopLevel::Transaction` item regardless of export or sync wrapping.
fn all_transactions(program: &HirProgram) -> impl Iterator<Item = &super::program::HirCallable> {
 program
 .callables
 .iter()
 .filter(|c| c.kind == super::program::HirCallableKind::Transaction)
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
 ds: &DisjointSet,
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
 for callable in all_transactions(program) {
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

// ── Rail membership: net<>/stdnet<> (E14b-7, HIR port) ──────────────────

/// One `net<>`/`stdnet<>` attachment on an instance `let` — an optional
/// pin qualifier, the net name, and whether the name is registry-backed
/// (`stdnet`, carries `spec NetVoltage`) or board-local (`net`).
#[derive(Debug, Clone)]
struct NetAttach {
 pin: Option<String>,
 name: String,
 standard: bool,
}

/// Collect the net-name attachments from instance `let` modifiers. The
/// annotation payload is the canonical quoted string ("NAME" or
/// "pin:NAME,..."); identifiers cannot contain the separators.
fn collect_net_names(program: &HirProgram) -> BTreeMap<String, Vec<NetAttach>> {
 let mut out: BTreeMap<String, Vec<NetAttach>> = BTreeMap::new();
 let mut rows: Vec<(String, bool, String)> = Vec::new();
 for global in &program.globals {
 for (ann_name, value) in &global.annotations {
 let standard = match ann_name.as_str() {
 "net" => false,
 "stdnet" => true,
 _ => continue,
 };
 let Some(Some(HirExpr {
 kind: HirExprKind::Literal(HirLiteral::Bytes(bytes)),
 ..
 })) = value.as_ref().map(Some)
 else {
 continue;
 };
 rows.push((global.name.clone(), standard, String::from_utf8_lossy(bytes).to_string()));
 }
 }
 for (name, standard, payload) in rows {
 for part in payload.split(',') {
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
 }
 out
}

/// The declared `spec NetVoltage` of a standard net — the registry row is
/// any type whose metadata declares the key (Rule 15: never the name).
fn stdnet_expected_volts(
 type_props: &BTreeMap<String, &BTreeMap<String, PropertyValue>>,
 name: &str,
) -> Option<f64> {
 type_props.get(name).and_then(|m| m.get("net_voltage")).and_then(|pv| match pv {
 PropertyValue::Quantity { si, dimension } if *dimension == QuantityDim::Volt => Some(*si),
 _ => None,
 })
}

/// The declared `spec KicadLabel` of a standard net — the emitter label
/// when the bare name is not the spelling the tools expect.
fn stdnet_label(
 type_props: &BTreeMap<String, &BTreeMap<String, PropertyValue>>,
 name: &str,
) -> Option<String> {
 type_props
 .get(name)
 .and_then(|m| m.get("kicad_label"))
 .and_then(property_string)
}

/// The name bindings of one membership pass — the duplicate-name rule in
/// ONE place (a name refusing to span two rails is the whole point: two
/// rails are not one net).
struct NetBindings<'a> {
 bound: &'a mut BTreeMap<String, (String, String)>,
 errors: &'a mut Vec<String>,
}

impl NetBindings<'_> {
 fn snapshot(&self) -> &BTreeMap<String, (String, String)> {
 self.bound
 }

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
 match &att.pin {
 Some(p) => Some(p.clone()),
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
 Some(supplies[0].0.clone())
 }
 }
}

/// One attachment's class + registry validation. Errors state what, why,
/// and the fix; `None` drops the attachment from the pass.
fn validate_attach(
 tables: &Tables<'_>,
 ti: &TypeInfo,
 inst_name: &str,
 pname: &str,
 att: &NetAttach,
 errors: &mut Vec<String>,
) -> Option<(String, bool)> {
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
 if att.standard && stdnet_expected_volts(tables.type_props, &att.name).is_none() {
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
 tables: &Tables<'_>,
 net_names: &BTreeMap<String, Vec<NetAttach>>,
 unpop: &HashSet<String>,
) -> (BTreeMap<(String, String), (String, bool)>, Vec<String>) {
 let mut named = BTreeMap::new();
 let mut errors = Vec::new();
 for inst in tables.instances.values() {
 if unpop.contains(&inst.name) {
 continue;
 }
 let Some(ti) = tables.type_info.get(&inst.type_name) else {
 continue;
 };
 let Some(attaches) = net_names.get(&inst.name) else {
 continue;
 };
 for att in attaches {
 let supplies = rail_pins(ti, false);
 let Some(pname) = attach_pin(&inst.name, &supplies, att, &mut errors) else {
 continue;
 };
 let Some(entry) =
 validate_attach(tables, ti, &inst.name, &pname, att, &mut errors)
 else {
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
 }
 (named, errors)
}

/// Bind names through connected pins first — a drive-fact source pin
/// (j1.vbus) IS its rail; a pin pre-wired by a guard equality sits on a
/// final net. Both bind their name to that net's root.
fn bind_connected_names(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 unpop: &HashSet<String>,
 named: &BTreeMap<(String, String), (String, bool)>,
 bindings: &mut NetBindings<'_>,
) {
 for inst in tables.instances.values() {
 if unpop.contains(&inst.name) {
 continue;
 }
 let Some(ti) = tables.type_info.get(&inst.type_name) else {
 continue;
 };
 for (pname, _) in rail_pins(ti, false) {
 let key = pin_key(&inst.name, pname);
 let Some(att) = named.get(&(inst.name.clone(), pname.clone())) else {
 continue;
 };
 if !ds.contains(&key) {
 continue;
 }
 let root = ds.find(&key);
 let label = stdnet_label(tables.type_props, &att.0).unwrap_or_else(|| att.0.clone());
 bindings.bind(&att.0, &root, label);
 }
 }
}

/// The membership obligations of one pass — one row per populated supply
/// pin: (instance, pin, tolerance, declared attachment).
type MembershipObligation = (String, String, Option<f64>, Option<(String, bool)>);

fn membership_obligations(
 tables: &Tables<'_>,
 named: &BTreeMap<(String, String), (String, bool)>,
 unpop: &HashSet<String>,
) -> Vec<MembershipObligation> {
 let mut obligations: Vec<MembershipObligation> = Vec::new();
 for inst in tables.instances.values() {
 if unpop.contains(&inst.name) {
 continue;
 }
 let Some(ti) = tables.type_info.get(&inst.type_name) else {
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

/// Resolve one obligation through the ladder: expectation filter →
/// tolerance refutation → propagation toward bound-named rails →
/// enumerated ambiguity.
fn resolve_one_membership(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 rails: &BTreeMap<String, f64>,
 source_pins: &HashSet<String>,
 bindings: &mut NetBindings<'_>,
 obligation: MembershipObligation,
) -> Option<String> {
 let (inst, pname, tolerance, attachment) = obligation;
 let key = pin_key(&inst, &pname);
 if ds.contains(&key) || source_pins.contains(&key) {
 return None;
 }
 let expectation = attachment
 .as_ref()
 .filter(|(_, standard)| *standard)
 .and_then(|(name, _)| stdnet_expected_volts(tables.type_props, name));
 let rated = match tolerance {
 None => "unrated".to_string(),
 Some(v) => format!("<= {}", format_volts(v)),
 };
 let candidates = membership_candidates(rails, tolerance, expectation);
 let fix = membership_fix(&attachment);
 match candidates.len() {
 0 => {
 let why = no_rail_why(&attachment, expectation, &rated);
 bindings.errors.push(format!(
 "supply pin '{}.{}' has no rail to join: {}; driven rails are {}. fix: {}",
 inst,
 pname,
 why,
 membership_drive_list(rails),
 fix
 ));
 None
 }
 1 => {
 let (root, v) = &candidates[0];
 ds.make(key.clone());
 ds.union(&key, root);
 if let Some((name, _)) = &attachment {
 let label = stdnet_label(tables.type_props, name).unwrap_or_else(|| name.clone());
 bindings.bind(name, root, label);
 }
 Some(format!(
 "membership inferred: {}.{} joins the {} rail ({}; rated {})",
 inst,
 pname,
 format_volts(*v),
 unique_membership_why(&attachment),
 rated
 ))
 }
 n => resolve_membership_propagation(ds, tables, bindings, &inst, &pname, &key, &attachment, &candidates, n, &rated),
 }
}

/// Rung 3/4: one bound-named candidate rail joins it; residual ambiguity
/// is a hard diagnostic enumerating the candidates.
#[allow(clippy::too_many_arguments)]
fn resolve_membership_propagation(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 bindings: &mut NetBindings<'_>,
 inst: &str,
 pname: &str,
 key: &str,
 attachment: &Option<(String, bool)>,
 candidates: &[(String, f64)],
 n: usize,
 rated: &str,
) -> Option<String> {
 let named_candidates: std::collections::BTreeSet<String> = candidates
 .iter()
 .filter_map(|(r, _)| {
 bindings
 .snapshot()
 .iter()
 .find_map(|(name, (root, _))| (root == r).then_some(name.clone()))
 })
 .collect();
 if let Some(name) = named_candidates.iter().next() {
 if named_candidates.len() == 1 {
 let (root, _) = bindings.snapshot()[name].clone();
 ds.make(key.to_string());
 ds.union(key, &root);
 if let Some((own, _)) = attachment {
 let label = stdnet_label(tables.type_props, own).unwrap_or_else(|| own.clone());
 bindings.bind(own, &root, label);
 }
 return Some(format!(
 "membership propagated: {}.{} joins {} (the only bound-named rail \
 among its candidates; rated {})",
 inst, pname, name, rated
 ));
 }
 }
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

/// Supply-rail membership — the full ladder, entry. Rung 1: a `stdnet<>`
/// expectation filters candidates to rails driven at the declared
/// `spec NetVoltage`. Rung 2: pin tolerance refutes; a unique survivor
/// unions silently (proof line). Rung 3: propagation toward bound-named
/// rails. Rung 4: residual ambiguity is a hard diagnostic. Rail source
/// pins are exempt (a driven pin defines its rail). Names bind to rails
/// only through physics-forced pins (Rule 15). Returns the name bindings.
#[allow(clippy::too_many_arguments)]
fn infer_rail_membership_hir(
 program: &HirProgram,
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 unpop: &HashSet<String>,
 ds: &mut DisjointSet,
 proofs: &mut Vec<String>,
 errors: &mut Vec<String>,
) -> BTreeMap<String, (String, String)> {
 let (rails, source_pins, birth_proofs) =
 collect_driven_rails_full(program, tables, laws, ds);
 proofs.extend(birth_proofs);
 let net_names = collect_net_names(program);
 let (named, expand_errors) = expand_net_attachments(tables, &net_names, unpop);
 errors.extend(expand_errors);
 let mut bound: BTreeMap<String, (String, String)> = BTreeMap::new();
 {
 let mut bindings = NetBindings { bound: &mut bound, errors };
 bind_connected_names(ds, tables, unpop, &named, &mut bindings);
 for (inst, pname, _, attachment) in membership_obligations(tables, &named, unpop) {
 let key = pin_key(&inst, &pname);
 let (true, Some((name, _))) = (source_pins.contains(&key), attachment) else {
 continue;
 };
 let root = ds.find(&key);
 let label = stdnet_label(tables.type_props, &name).unwrap_or_else(|| name.clone());
 bindings.bind(&name, &root, label);
 }
 for obligation in membership_obligations(tables, &named, unpop) {
 if let Some(proof) =
 resolve_one_membership(ds, tables, &rails, &source_pins, &mut bindings, obligation)
 {
 proofs.push(proof);
 }
 }
 }
 bound
}

/// The driven supply rails, two ways: root → volts (the forcing maps) and
/// the SOURCE pin keys themselves. Contract facts AND component laws birth
/// rails; birth proofs accompany the merge.
fn collect_driven_rails_full(
 program: &HirProgram,
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 ds: &mut DisjointSet,
) -> (BTreeMap<String, f64>, HashSet<String>, Vec<String>) {
 let mut rails: BTreeMap<String, f64> = BTreeMap::new();
 let mut source_pins: HashSet<String> = HashSet::new();
 let mut eqs: Vec<(&HirExpr, &HirExpr)> = Vec::new();
 for callable in all_transactions(program) {
 let Some(contract) = &callable.contract else {
 continue;
 };
 collect_eq_pairs(&contract.pre, &mut eqs);
 collect_eq_pairs(&contract.post, &mut eqs);
 }
 for (l, r) in eqs {
 let Some((pin, volts)) = voltage_drive(l, r, tables.instances, tables.type_pins) else {
 continue;
 };
 if !pin_classes_of(tables, &pin).map_or(false, |c| c.supply) {
 continue;
 }
 let key = pin_key(&pin.component, &pin.pin);
 source_pins.insert(key.clone());
 let root = ds.find(&key);
 let entry = rails.entry(root).or_insert(0.0);
 if volts > *entry {
 *entry = volts;
 }
 }
 let (law_rails, law_sources, proofs) = law_rail_births(laws, tables, ds);
 for (root, volts) in law_rails {
 let entry = rails.entry(root).or_insert(0.0);
 if volts > *entry {
 *entry = volts;
 }
 }
 source_pins.extend(law_sources);
 (rails, source_pins, proofs)
}

/// Rails born by component laws — an unconditional law pinning one pin's
/// voltage to a constant drives its net exactly as a contract fact does.
/// Supply-class pins only; guarded laws and named modes do not birth rails.
fn law_rail_births(
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 tables: &Tables<'_>,
 ds: &mut DisjointSet,
) -> (BTreeMap<String, f64>, HashSet<String>, Vec<String>) {
 let mut rails: BTreeMap<String, f64> = BTreeMap::new();
 let mut sources: HashSet<String> = HashSet::new();
 let mut proofs: Vec<String> = Vec::new();
 let mut seen: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
 let births = laws
 .iter()
 .flat_map(|component| component.laws.iter())
 .filter(|law| law.guard == crate::analysis::electronics_laws::LawGuard::Always)
 .flat_map(|law| {
 law.equations
 .iter()
 .filter_map(|eq| crate::analysis::electronics::constant_voltage_pin(&eq.expression))
 });
 for (pin, volts) in births {
 if !pin_classes_of(tables, &pin).map_or(false, |c| c.supply) {
 continue;
 }
 let key = pin_key(&pin.component, &pin.pin);
 let root = ds.find(&key);
 let entry = rails.entry(root).or_insert(0.0);
 if volts > *entry {
 *entry = volts;
 }
 sources.insert(key);
 if seen.insert((pin.component.clone(), pin.pin.clone())) {
 proofs.push(format!(
 "rail born: {}.{} drives the {} rail (component law)",
 pin.component,
 pin.pin,
 format_volts(volts)
 ));
 }
 }
 (rails, sources, proofs)
}

/// One pin's class from the resolved tables.
fn pin_classes_of<'a>(tables: &Tables<'a>, pin: &PinRef) -> Option<&'a PinClassProps> {
 let inst = tables.instances.get(&pin.component)?;
 let ti = tables.type_info.get(&inst.type_name)?;
 let idx = ti.pins.iter().position(|(n, _)| n == &pin.pin)?;
 ti.pin_classes.get(idx)
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

// ── Node-body intents, mechanisms, obligation forcing (E14a/D16/E14b/E15) ──

/// A mechanism bridge request (D16 phase 2): a conditional connection
/// A-B, closed when the control pin's region holds.
struct BridgeRequest {
 node: String,
 a: PinRef,
 b: PinRef,
 control: PinRef,
 strategy: Option<String>,
}

/// A voltage obligation from a node body — `inst.pin.voltage OP <V>;`.
struct VoltageObligation {
 node: String,
 pin: PinRef,
 volts: f64,
 min: bool,
}

/// A current obligation — `pin.current >= I;` (min only forces wiring).
struct CurrentObligation {
 node: String,
 pin: PinRef,
 amps: f64,
 min: bool,
}

/// The region context threading through a guarded-fact walk (D16):
/// `control` Some means mechanizable; `cond` Some means signal-level but
/// not mechanizable (the D7 gate); `strategy` is the `through` selection.
#[derive(Clone, Copy)]
struct RegionCtx<'a> {
 cond: Option<&'a str>,
 control: Option<&'a PinRef>,
 strategy: Option<&'a str>,
}

/// Diagnostic sink for body-fact collection — keeps the walker's
/// parameter list flat.
struct FactSink<'a> {
 node: &'a str,
 proofs: &'a mut Vec<String>,
 errors: &'a mut Vec<String>,
 intents: &'a mut Vec<(String, String)>,
 pin_intents: &'a mut Vec<(String, PinRef)>,
 obligations: &'a mut Vec<VoltageObligation>,
 current_obligations: &'a mut Vec<CurrentObligation>,
 bridges: &'a mut Vec<BridgeRequest>,
 opens: &'a mut Vec<(HirExpr, HirExpr)>,
}

/// Does this expression mention a pin of a declared instance? Pin-mentioning
/// conditions are SIGNAL-LEVEL (the conditional-mechanism trigger).
fn mentions_pin(expr: &HirExpr, tables: &Tables<'_>) -> bool {
 if resolve_pin(expr, tables.instances, tables.type_pins).is_some() {
 return true;
 }
 match &expr.kind {
 HirExprKind::Binary { left, right, .. } => {
 mentions_pin(left, tables) || mentions_pin(right, tables)
 }
 HirExprKind::Field { receiver, .. } => mentions_pin(receiver, tables),
 HirExprKind::Index(l, r) => mentions_pin(l, tables) || mentions_pin(r, tables),
 _ => false,
 }
}

/// The control pin of a mechanizable condition, plus an optional validation
/// error. A single comparison whose one operand resolves to a pin — the
/// condition's net feeds the mechanism control. The exact shape is
/// validated: the pin operand must be a `.voltage` access and the other
/// operand a voltage literal.
fn condition_control(expr: &HirExpr, tables: &Tables<'_>) -> (Option<PinRef>, Option<String>) {
 let HirExprKind::Binary { left, right, .. } = &expr.kind else {
 return (None, None);
 };
 let lpin = resolve_pin(left, tables.instances, tables.type_pins);
 let rpin = resolve_pin(right, tables.instances, tables.type_pins);
 let (pin_side, other, ctrl) = match (lpin, rpin) {
 (Some(p), None) => (left.as_ref(), right.as_ref(), p),
 (None, Some(p)) => (right.as_ref(), left.as_ref(), p),
 _ => return (None, None),
 };
 if voltage_operand(pin_side, tables).is_some()
 && matches!(&other.kind, HirExprKind::UnitLiteral { .. })
 {
 return (Some(ctrl), None);
 }
 let shape = format!("{}.{}", ctrl.component, ctrl.pin);
 (
 None,
 Some(format!(
 "mechanism condition must be a single pin voltage comparison (e.g. `{shape}.voltage == 3.3V`), or a mechanism-bodied `hold ... through Type;` — got `{shape} = {}`, where the pin side must be a `.voltage` access and the non-pin side a voltage literal",
 dump_expr(other)
 )),
 )
}

/// `expr` is a `.voltage` access on a declared pin. Anything else → None.
fn voltage_operand(expr: &HirExpr, tables: &Tables<'_>) -> Option<PinRef> {
 let HirExprKind::Field { receiver, name } = &expr.kind else {
 return None;
 };
 if name != "voltage" {
 return None;
 }
 resolve_pin(receiver, tables.instances, tables.type_pins)
}

/// Pull the parser's law markers out of a guarded body: `through <Name>`
/// yields the strategy; the `hold` marker is consumed whole.
fn take_law_markers(body: &[HirStmt]) -> (Option<String>, Vec<HirStmt>) {
 let mut strategy = None;
 let mut rest = Vec::with_capacity(body.len());
 for stmt in body {
 match &stmt.kind {
 HirStmtKind::MetadataAssignment(k, PropertyValue::Identifier(name)) if k == "through" => {
 strategy = Some(name.clone());
 }
 HirStmtKind::MetadataAssignment(k, PropertyValue::Bool(true)) if k == "hold" => {}
 _ => rest.push(stmt.clone()),
 }
 }
 (strategy, rest)
}

impl FactSink<'_> {
 /// One body assignment (E14a/D16): mechanism bridge, conditional-carve
 /// error, drive intent (instance or pin form), or pin-to-pin wire fact.
 fn fact_assign(&mut self, target: &HirExpr, value: &HirExpr, reg: RegionCtx<'_>, tables: &Tables<'_>, ds: &mut DisjointSet) {
 if reg.control.is_some() {
 self.mechanism_bridge(target, value, reg, tables, ds);
 return;
 }
 if let Some(desc) = reg.cond {
 self.errors.push(format!(
 "wiring inside `when {desc}` (node '{}') is conditional — copper cannot be. \
 State the condition in the node guard (making the wiring unconditional in \
 that region), or make it a single pin comparison and declare a switching \
 part (`hold ... through Type;`, mechanism synthesis)",
 self.node
 ));
 return;
 }
 let on = matches!(&value.kind, HirExprKind::Literal(HirLiteral::Bool(true)));
 if on {
 if let HirExprKind::Identifier(name) = &target.kind {
 self.intents.push((self.node.to_string(), name.clone()));
 return;
 }
 }
 if matches!(&value.kind, HirExprKind::Literal(HirLiteral::Bool(_))) {
 self.pin_drive_intent(target, on, tables);
 return;
 }
 self.wire_fact(target, value, tables, ds);
 }

 /// A wiring fact under a mechanizable condition becomes a bridge
 /// request — both sides must resolve to pins.
 fn mechanism_bridge(
 &mut self,
 target: &HirExpr,
 value: &HirExpr,
 reg: RegionCtx<'_>,
 tables: &Tables<'_>,
 ds: &mut DisjointSet,
 ) {
 let Some(ctrl) = reg.control else {
 self.errors.push(format!(
 "wiring under a mechanism (node '{}') must be a pin-to-pin bridge",
 self.node
 ));
 return;
 };
 let _ = ds;
 let (Some(a), Some(b)) = (
 resolve_pin(target, tables.instances, tables.type_pins),
 resolve_pin(value, tables.instances, tables.type_pins),
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

 /// Pin-level drive intent `u1.en = true;` — the same completion rules
 /// as the instance form, addressed at one pin. A Bool RHS is ALWAYS an
 /// intent form: a target that is not a pin errors instead of falling
 /// through silently.
 fn pin_drive_intent(&mut self, target: &HirExpr, on: bool, tables: &Tables<'_>) {
 let Some(lp) = resolve_pin(target, tables.instances, tables.type_pins) else {
 self.errors.push(format!(
 "Bool assignment to '{}' (node '{}') is not an electronics fact — write a \
 drive intent (`inst = true;` or `inst.pin = true;`) or a pin-to-pin wire \
 (`a = b;`)",
 dump_expr(target),
 self.node
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
 fn wire_fact(&mut self, target: &HirExpr, value: &HirExpr, tables: &Tables<'_>, ds: &mut DisjointSet) {
 let (Some(lp), Some(rp)) = (
 resolve_pin(target, tables.instances, tables.type_pins),
 resolve_pin(value, tables.instances, tables.type_pins),
 ) else {
 self.errors.push(format!(
 "assignment '{}' = '{}' (node '{}') is not an electronics fact — both sides \
 must resolve to declared pins (`u1.vout = r1.a;`), or use a drive intent \
 (`inst = true;` / `inst.pin = true;`)",
 dump_expr(target),
 dump_expr(value),
 self.node
 ));
 return;
 };
 union_pins(ds, &lp, &rp);
 self.proofs.push(format!(
 "wired in node '{}': {}.{} <-> {}.{}",
 self.node, lp.component, lp.pin, rp.component, rp.pin
 ));
 }

 /// A body voltage obligation. A MIN bound (`pin.voltage >= V`) forces a
 /// pull-up; a MAX bound (`<=`/`<`) forces a low-hold path. A MIN bound
 /// on `pin.current` forces a series part. Any other bare expression is
 /// not an electronics fact.
 fn obligation_fact(&mut self, expr: &HirExpr, tables: &Tables<'_>) {
 let HirExprKind::Binary { op, left, right } = &expr.kind else {
 self.not_a_fact(expr);
 return;
 };
 let min = match op {
 BinaryOpKind::Ge | BinaryOpKind::Gt => true,
 BinaryOpKind::Le | BinaryOpKind::Lt => false,
 _ => {
 self.not_a_fact(expr);
 return;
 }
 };
 let (l, r) = (left.as_ref(), right.as_ref());
 if let Some((pin, volts)) = voltage_drive(l, r, tables.instances, tables.type_pins) {
 self.obligations.push(VoltageObligation {
 node: self.node.to_string(),
 pin,
 volts,
 min,
 });
 return;
 }
 if let Some((pin, amps)) = current_obligation_pair(l, r, tables) {
 if min {
 self.current_obligations.push(CurrentObligation {
 node: self.node.to_string(),
 pin,
 amps,
 min,
 });
 return;
 }
 }
 self.not_a_fact(expr);
 }

 fn not_a_fact(&mut self, expr: &HirExpr) {
 self.errors.push(format!(
 "expression '{}' (node '{}') is not an electronics fact — write a drive intent \
 (`inst = true;` / `inst.pin = true;`), a pin-to-pin wire (`a = b;`), or a \
 voltage obligation (`inst.pin.voltage >= 3.3V;`)",
 dump_expr(expr),
 self.node
 ));
 }

 /// Walk statements, threading the region context (E14a/D16).
 fn walk(&mut self, stmts: &[HirStmt], reg: RegionCtx<'_>, tables: &Tables<'_>, ds: &mut DisjointSet) {
 for stmt in stmts {
 match &stmt.kind {
 HirStmtKind::Guarded(c, inner) => {
 walk_guarded(c, inner, reg, tables, ds, self);
 }
 HirStmtKind::Assign(target, value) => {
 self.fact_assign(target, value, reg, tables, ds);
 }
 HirStmtKind::Open(a, b) => {
 self.opens.push((a.clone(), b.clone()));
 }
 HirStmtKind::Expression(expr) => {
 self.obligation_fact(expr, tables);
 }
 _ => {}
 }
 }
 }
}

/// The child condition description for a guarded region (D16): None when
/// the region is mechanizable or a plain carve; a compound description
/// when signal-level (D7 propagates through nested carves).
fn child_cond_str(
 control: &Option<PinRef>,
 reg_cond: Option<&str>,
 inner_mentions_pin: bool,
 c: &HirExpr,
) -> Option<String> {
 if control.is_some() {
 return None;
 }
 if reg_cond.is_none() && !inner_mentions_pin {
 return None;
 }
 Some(match reg_cond {
 Some(outer) => format!("{outer} && {}", dump_expr(c)),
 None => format!("{}", dump_expr(c)),
 })
}

/// The Guarded arm of the walk: classify the condition — mechanizable
/// (single pin comparison → bridges), signal-level but not mechanizable
/// (D7 error), or region-level carve. The signal description propagates.
fn walk_guarded(
 c: &HirExpr,
 inner: &[HirStmt],
 reg: RegionCtx<'_>,
 tables: &Tables<'_>,
 ds: &mut DisjointSet,
 sink: &mut FactSink<'_>,
) {
 let (strategy, inner_rest) = take_law_markers(inner);
 let (control, cond_err) = condition_control(c, tables);
 if let Some(e) = cond_err {
 sink.errors.push(e);
 return;
 }
 let child_control = if control.is_some() {
 control.clone()
 } else {
 reg.control.cloned()
 };
 let child_cond = child_cond_str(&child_control, reg.cond, mentions_pin(c, tables), c);
 sink.walk(
 &inner_rest,
 RegionCtx {
 cond: child_cond.as_deref(),
 control: child_control.as_ref(),
 strategy: strategy.as_deref().or(reg.strategy),
 },
 tables,
 ds,
 );
}

/// Body facts of one reactive transaction (E14a/D16, HIR port).
fn body_facts(callable: &super::program::HirCallable, tables: &Tables<'_>, ds: &mut DisjointSet, sink: &mut FactSink<'_>) {
 sink.walk(
 &callable.body.statements,
 RegionCtx {
 cond: None,
 control: None,
 strategy: None,
 },
 tables,
 ds,
 );
}

/// The pin+volts of a voltage obligation pair — one side is a `.voltage`
/// pin access, the other a voltage literal, either orientation.
fn voltage_obligation_pair(l: &HirExpr, r: &HirExpr, tables: &Tables<'_>) -> Option<(PinRef, f64)> {
 if let (Some(p), Some(v)) = (
 resolve_voltage_pin(l, tables.instances, tables.type_pins),
 extract_voltage(r),
 ) {
 return Some((p, v));
 }
 if let (Some(p), Some(v)) = (
 resolve_voltage_pin(r, tables.instances, tables.type_pins),
 extract_voltage(l),
 ) {
 return Some((p, v));
 }
 None
}

/// The current sibling — `pin.current >= <amp quantity>`, either order.
fn current_obligation_pair(l: &HirExpr, r: &HirExpr, tables: &Tables<'_>) -> Option<(PinRef, f64)> {
 if let (Some(p), Some(v)) = (
 resolve_current_pin(l, tables.instances, tables.type_pins),
 extract_current(r),
 ) {
 return Some((p, v));
 }
 if let (Some(p), Some(v)) = (
 resolve_current_pin(r, tables.instances, tables.type_pins),
 extract_current(l),
 ) {
 return Some((p, v));
 }
 None
}

/// `inst.pin.current` on a declared instance, resolved to the pin.
fn resolve_current_pin(
 expr: &HirExpr,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
) -> Option<PinRef> {
 let HirExprKind::Field { receiver, name } = &expr.kind else {
 return None;
 };
 if name != "current" {
 return None;
 }
 resolve_pin(receiver, instances, type_pins)
}

fn extract_current(expr: &HirExpr) -> Option<f64> {
 extract_quantity(expr, QuantityDim::Amp)
}

/// Eligibility of one declared instance as a bridge mechanism (D16): a
/// type with one or two Control pins and at least two Switchable pins,
/// whose control pin(s) are unconnected or already on the condition's net.
fn is_mechanism(
 ds: &mut DisjointSet,
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
 controls.iter().all(|(cname, _)| {
 let ckey = pin_key(&inst.name, cname);
 !(ds.contains(&ckey) && ds.find(&ckey) != *ctrl_root)
 })
}

/// Candidate mechanisms for a bridge request: declared instances whose
/// type qualifies, narrowed by the `via` strategy when given (type name).
fn mechanism_candidates(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 strategy: &Option<String>,
 control: &PinRef,
) -> Vec<String> {
 let ctrl_root = ds.find(&pin_key(&control.component, &control.pin));
 let mut out: Vec<String> = Vec::new();
 for inst in tables.instances.values() {
 let Some(ti) = tables.type_info.get(&inst.type_name) else {
 continue;
 };
 if let Some(t) = strategy {
 if inst.type_name != *t {
 continue;
 }
 }
 if is_mechanism(ds, inst, ti, &ctrl_root) {
 out.push(inst.name.clone());
 }
 }
 out
}

/// Wire one mechanism (D16 phase 2): control <- condition net, the two
/// path pins <- the bridged pins. Records the conditional bridge with
/// provenance; the schematic emits the switch as ordinary copper.
fn synthesize_one(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 mech_name: &str,
 br: &BridgeRequest,
 proofs: &mut Vec<String>,
) {
 let Some(mech) = tables.instances.get(mech_name) else {
 return;
 };
 let Some(ti) = tables.type_info.get(&mech.type_name) else {
 return;
 };
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
 let mut members = vec![(br.control.component.clone(), br.control.pin.clone())];
 for cpin in &controls {
 members.push((mech.name.clone(), cpin.0.clone()));
 }
 members.push((mech.name.clone(), paths[0].0.clone()));
 members.push((br.a.component.clone(), br.a.pin.clone()));
 members.push((mech.name.clone(), paths[1].0.clone()));
 members.push((br.b.component.clone(), br.b.pin.clone()));
 let keys: Vec<String> = members.iter().map(|(c, p)| pin_key(c, p)).collect();
 for k in &keys {
 ds.make(k.clone());
 }
 for k in keys.windows(2) {
 ds.union(&k[0], &k[1]);
 }
 proofs.push(format!(
 "mechanism '{}' bridges {}.{} <-> {}.{} under {}.{} (node '{}') with {} control pin(s)",
 mech.name,
 br.a.component,
 br.a.pin,
 br.b.component,
 br.b.pin,
 br.control.component,
 br.control.pin,
 br.node,
 controls.len()
 ));
}

/// Resolve every bridge request (D16 phase 2): exactly one candidate
/// mechanism after narrowing — synthesize; none, or several — hard
/// error naming the candidates and the strategy form (D13, never a
/// silent pick).
fn synthesize_bridges(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 bridges: &[BridgeRequest],
 proofs: &mut Vec<String>,
 errors: &mut Vec<String>,
) -> Vec<String> {
 let mut synthesized = Vec::new();
 for br in bridges {
 let a_key = pin_key(&br.a.component, &br.a.pin);
 let b_key = pin_key(&br.b.component, &br.b.pin);
 if ds.find(&a_key) == ds.find(&b_key) {
 errors.push(format!(
 "bridge {}.{} <-> {}.{} under {}.{} (node '{}') is redundant: the pins are already unconditionally connected — the switch can never open them. Remove the unconditional wiring or the mechanism",
 br.a.component, br.a.pin, br.b.component, br.b.pin,
 br.control.component, br.control.pin, br.node
 ));
 continue;
 }
 let mut candidates = mechanism_candidates(ds, tables, &br.strategy, &br.control);
 candidates.sort();
 match candidates.len() {
 0 => {
 let strategy_hint = match &br.strategy {
 Some(t) => format!(
 "strategy names '{}' but no qualifying mechanism of that type is declared — declare one with one Control pin (or a two-pin coil) and at least two Path pins",
 t
 ),
 None => "no qualifying mechanism is declared — declare one with one Control pin (or a two-pin coil) and at least two Path pins, or narrow with `hold ... through Type;`".to_string(),
 };
 errors.push(format!(
 "bridge {}.{} <-> {}.{} under {}.{} (node '{}') cannot be synthesized: {}",
 br.a.component, br.a.pin, br.b.component, br.b.pin,
 br.control.component, br.control.pin, br.node, strategy_hint
 ));
 }
 1 => {
 synthesize_one(ds, tables, &candidates[0], br, proofs);
 let strategy = br.strategy.as_deref().unwrap_or("-");
 synthesized.push(format!(
 "{}.{} <-> {}.{} under {}.{} through {} (node '{}')",
 br.a.component, br.a.pin, br.b.component, br.b.pin,
 br.control.component, br.control.pin, strategy, br.node
 ));
 }
 n => {
 errors.push(format!(
 "bridge {}.{} <-> {}.{} under {}.{} (node '{}') is ambiguous: {} mechanisms could synthesize it ({}). Disambiguate with the strategy clause: `hold ... through <Type>;` or pre-wire one mechanism's control pin",
 br.a.component, br.a.pin, br.b.component, br.b.pin,
 br.control.component, br.control.pin, br.node, n, candidates.join(", ")
 ));
 }
 }
 }
 synthesized
}

/// Free drive-capable candidate pins for an intent completion —
/// unconnected `spec CanDrive` pins of every OTHER instance, sorted.
fn free_can_drive_candidates(tables: &Tables<'_>, ds: &mut DisjointSet, exclude_inst: &str) -> Vec<String> {
 let mut candidates: Vec<String> = Vec::new();
 for (other, oi) in tables.instances {
 if other == exclude_inst {
 continue;
 }
 let Some(oti) = tables.type_info.get(&oi.type_name) else {
 continue;
 };
 for (i, (n, _)) in oti.pins.iter().enumerate() {
 let key = pin_key(other, n);
 if oti.pin_classes[i].can_drive && !ds.contains(&key) {
 candidates.push(format!("{}.{}", other, n));
 }
 }
 }
 candidates.sort();
 candidates
}

/// Complete one drive intent (E14a): the instance must have exactly one
/// open pin, and exactly one unconnected drive-capable pin (spec
/// CanDrive) may exist elsewhere. One of each — the intent wires them.
fn complete_intent(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 inst_name: &str,
 node: &str,
) -> Result<String, String> {
 let Some(inst) = tables.instances.get(inst_name) else {
 return Err(format!(
 "drive intent '{} = true' (node '{}') names no declared instance",
 inst_name, node
 ));
 };
 let Some(ti) = tables.type_info.get(&inst.type_name) else {
 return Ok(format!(
 "intent '{} = true' (node '{}'): typeless — recorded",
 inst_name, node
 ));
 };
 let Some(pins) = tables.type_pins.get(&inst.type_name) else {
 return Ok(format!(
 "intent '{} = true' (node '{}'): typeless — recorded",
 inst_name, node
 ));
 };
 let open: Vec<&(String, u64)> = pins
 .iter()
 .filter(|(n, _)| !ds.contains(&pin_key(inst_name, n)))
 .collect();
 if open.is_empty() {
 return Ok(format!(
 "intent '{} = true' (node '{}'): fully wired — recorded",
 inst_name, node
 ));
 }
 if open.len() > 1 {
 let names: Vec<String> = open
 .iter()
 .map(|(n, _)| format!("{}.{}", inst_name, n))
 .collect();
 return Err(format!(
 "drive intent '{} = true' (node '{}') cannot complete: {} pins of '{}' are unconnected ({}). Wire all but one — the intent completes the last open pin",
 inst_name, node, open.len(), inst_name, names.join(", ")
 ));
 }
 let candidates = free_can_drive_candidates(tables, ds, inst_name);
 match candidates.len() {
 0 => Err(format!(
 "drive intent '{} = true' (node '{}') has no completion: no unconnected drive-capable pin (spec CanDrive) remains in scope",
 inst_name, node
 )),
 1 => {
 let mut parts = candidates[0].splitn(2, '.');
 let (co, cp) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
 let lk = pin_key(inst_name, &open[0].0);
 let rk = pin_key(co, cp);
 ds.make(lk.clone());
 ds.make(rk.clone());
 ds.union(&lk, &rk);
 Ok(format!(
 "wired by intent '{} = true' (node '{}'): {}.{} <-> {} — single drive-capable candidate",
 inst_name, node, inst_name, open[0].0, candidates[0]
 ))
 }
 n => Err(format!(
 "drive intent '{} = true' (node '{}') is ambiguous: {} drive-capable pins could complete '{}.{}' ({}). State the connection explicitly in the node guard",
 inst_name, node, n, inst_name, open[0].0, candidates.join(", ")
 )),
 }
}

/// Pin-level drive intent completion: the pin completes against the sole
/// unconnected drive-capable pin elsewhere. An already-connected pin
/// records — the intent states participation, not a new edge.
fn complete_pin_intent(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 pin: &PinRef,
 node: &str,
) -> Result<String, String> {
 let key = pin_key(&pin.component, &pin.pin);
 if ds.contains(&key) {
 return Ok(format!(
 "pin intent '{}.{} = true' (node '{}'): already connected — recorded",
 pin.component, pin.pin, node
 ));
 }
 let candidates = free_can_drive_candidates(tables, ds, &pin.component);
 match candidates.len() {
 0 => Err(format!(
 "pin intent '{}.{} = true' (node '{}') has no completion: no unconnected drive-capable pin (spec CanDrive) remains in scope",
 pin.component, pin.pin, node
 )),
 1 => {
 let mut parts = candidates[0].splitn(2, '.');
 let (co, cp) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
 let rk = pin_key(co, cp);
 ds.make(key.clone());
 ds.make(rk.clone());
 ds.union(&key, &rk);
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

/// Whether the demand came from an instance intent or a pin intent.
enum DriveKind {
 Instance,
 Pin,
}

/// One open drive intent awaiting a pin — bundled for the assignment sort.
struct DriveDemand {
 node: String,
 target: PinRef,
 kind: DriveKind,
}

/// The exactly-one unconnected pin of an instance, if any.
fn instance_single_open(tables: &Tables<'_>, ds: &mut DisjointSet, inst_name: &str) -> Option<String> {
 let inst = tables.instances.get(inst_name)?;
 let pins = tables.type_pins.get(&inst.type_name)?;
 let open: Vec<&String> = pins
 .iter()
 .filter(|(n, _)| !ds.contains(&pin_key(inst_name, n)))
 .map(|(n, _)| n)
 .collect();
 if open.len() == 1 {
 Some(open[0].clone())
 } else {
 None
 }
}

/// All unconnected drive-capable pins, sorted — the batch's supply.
fn free_can_drive_pins(tables: &Tables<'_>, ds: &mut DisjointSet) -> Vec<(String, String)> {
 let mut pins: Vec<(String, String)> = Vec::new();
 for (other, oi) in tables.instances.iter() {
 let Some(oti) = tables.type_info.get(&oi.type_name) else {
 continue;
 };
 for (i, (n, _)) in oti.pins.iter().enumerate() {
 if oti.pin_classes[i].can_drive && !ds.contains(&pin_key(other, n)) {
 pins.push((other.to_string(), n.clone()));
 }
 }
 }
 pins.sort();
 pins
}

/// The free pins are all interchangeable — same PinClassProps — so any
/// matching is electrically equivalent and a deterministic pick is not a
/// silent choice (D13: the choice never matters).
fn pins_interchangeable(tables: &Tables<'_>, ds: &mut DisjointSet, pins: &[(String, String)]) -> bool {
 let mut props: Option<&PinClassProps> = None;
 for (comp, name) in pins {
 let pin = PinRef {
 component: comp.clone(),
 pin: name.clone(),
 number: 0,
 };
 let Some(c) = pin_classes_of(tables, &pin) else {
 return false;
 };
 match props {
 Some(p) if *p != *c => return false,
 None => props = Some(c),
 _ => {}
 }
 }
 true
}

/// Wire the perfect matching — sorted demand ↔ sorted supply.
fn wire_drive_matching(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 mut demands: Vec<DriveDemand>,
 supply: &[(String, String)],
 proofs: &mut Vec<String>,
) {
 demands.sort_by(|a, b| {
 (a.node.as_str(), a.target.component.as_str(), a.target.pin.as_str()).cmp(&(
 b.node.as_str(),
 b.target.component.as_str(),
 b.target.pin.as_str(),
 ))
 });
 for (i, d) in demands.iter().enumerate() {
 let (scomp, spin) = &supply[i];
 let tkey = pin_key(&d.target.component, &d.target.pin);
 let skey = pin_key(scomp, spin);
 ds.make(tkey.clone());
 ds.make(skey.clone());
 ds.union(&tkey, &skey);
 proofs.push(format!(
 "wired by drive assignment (node '{}'): {}.{} <-> {}.{} — matched among interchangeable free drive-capable pins",
 d.node, d.target.component, d.target.pin, scomp, spin
 ));
 }
 let _ = tables;
}

/// Return unresolved demands to the per-intent lists.
fn reclaim_demands(
 demands: Vec<DriveDemand>,
 unmatched_inst: &mut Vec<(String, String)>,
 unmatched_pin: &mut Vec<(String, PinRef)>,
) {
 for d in demands {
 match d.kind {
 DriveKind::Instance => unmatched_inst.push((d.node, d.target.component)),
 DriveKind::Pin => unmatched_pin.push((d.node, d.target)),
 }
 }
}

/// Batch drive assignment (E14b slice 4): several open drive intents
/// competing for interchangeable free drive-capable pins are a perfect
/// matching, wired deterministically. Unresolvable intents fall through
/// to the per-intent completions unchanged (their D13 errors stand).
fn solve_drive_assignment(
 ds: &mut DisjointSet,
 tables: &Tables<'_>,
 intents: &[(String, String)],
 pin_intents: &[(String, PinRef)],
 proofs: &mut Vec<String>,
) -> (Vec<(String, String)>, Vec<(String, PinRef)>) {
 let mut unmatched_inst: Vec<(String, String)> = Vec::new();
 let mut unmatched_pin: Vec<(String, PinRef)> = Vec::new();
 let mut demands: Vec<DriveDemand> = Vec::new();
 for (node, inst_name) in intents {
 match instance_single_open(tables, ds, inst_name) {
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
 if ds.contains(&pin_key(&pin.component, &pin.pin)) {
 unmatched_pin.push((node.clone(), pin.clone()));
 } else {
 demands.push(DriveDemand {
 node: node.clone(),
 target: pin.clone(),
 kind: DriveKind::Pin,
 });
 }
 }
 if demands.len() >= 2 {
 let supply = free_can_drive_pins(tables, ds);
 if supply.len() < demands.len() || !pins_interchangeable(tables, ds, &supply) {
 reclaim_demands(demands, &mut unmatched_inst, &mut unmatched_pin);
 return (unmatched_inst, unmatched_pin);
 }
 wire_drive_matching(ds, tables, demands, &supply, proofs);
 return (unmatched_inst, unmatched_pin);
 }
 reclaim_demands(demands, &mut unmatched_inst, &mut unmatched_pin);
 (unmatched_inst, unmatched_pin)
}

/// 2026-09-21 (E14a slice 1) + D13: node-body intents. Body wiring facts
/// union first; voltage/current obligations force (pull-up, low-hold,
/// series); drive intents then complete the last open pin; `open`
/// disconnections verify. Returns (errors, proofs, conditional bridges).
fn run_intent_pass(
 program: &HirProgram,
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 ds: &mut DisjointSet,
) -> (Vec<String>, Vec<String>, Vec<String>) {
 let mut errors = Vec::new();
 let mut proofs: Vec<String> = Vec::new();
 let mut intents: Vec<(String, String)> = Vec::new();
 let mut pin_intents: Vec<(String, PinRef)> = Vec::new();
 let mut obligations: Vec<VoltageObligation> = Vec::new();
 let mut current_obligations: Vec<CurrentObligation> = Vec::new();
 let mut bridges: Vec<BridgeRequest> = Vec::new();
 let mut opens: Vec<(HirExpr, HirExpr)> = Vec::new();
 for callable in all_transactions(program) {
 let mut sink = FactSink {
 node: &callable.name,
 proofs: &mut proofs,
 errors: &mut errors,
 intents: &mut intents,
 pin_intents: &mut pin_intents,
 obligations: &mut obligations,
 current_obligations: &mut current_obligations,
 bridges: &mut bridges,
 opens: &mut opens,
 };
 body_facts(callable, tables, ds, &mut sink);
 }
 let synthesized = synthesize_bridges(ds, tables, &bridges, &mut proofs, &mut errors);
 // Voltage obligations realize BEFORE drive completions — bus assembly
 // + pull-ups connect the open-drain nets first. The forcing consumes
 // passive parts, never CanDrive pins, so it cannot steal a completion.
 force_obligations(program, tables, laws, ds, &obligations, &current_obligations, &mut errors, &mut proofs);
 let (unmatched_intents, unmatched_pins) =
 solve_drive_assignment(ds, tables, &intents, &pin_intents, &mut proofs);
 for (node, inst_name) in &unmatched_intents {
 match complete_intent(ds, tables, inst_name, node) {
 Ok(proof) => proofs.push(proof),
 Err(e) => errors.push(e),
 }
 }
 for (node, pin) in &unmatched_pins {
 match complete_pin_intent(ds, tables, pin, node) {
 Ok(proof) => proofs.push(proof),
 Err(e) => errors.push(e),
 }
 }
 // 2026-09-22 (D16 p3b): disconnections — two pins that must NOT be
 // connected. A wiring fact or mechanism bridge that would connect
 // them is a hard error.
 for (a, b) in &opens {
 let (Some(ap), Some(bp)) = (
 resolve_pin(a, tables.instances, tables.type_pins),
 resolve_pin(b, tables.instances, tables.type_pins),
 ) else {
 errors.push(format!(
 "open must name two pins, got '{}' and '{}'",
 dump_expr(a),
 dump_expr(b)
 ));
 continue;
 };
 let ak = pin_key(&ap.component, &ap.pin);
 let bk = pin_key(&bp.component, &bp.pin);
 if ds.find(&ak) == ds.find(&bk) {
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

// ── Voltage/current obligation forcing (E14b/E15, HIR port) ─────────────

/// Everything the series wiring renders, bundled to keep the parameter
/// list flat.
struct SeriesWire<'a> {
 part: &'a str,
 root: &'a str,
 rail: &'a str,
 vrail: f64,
 vf: f64,
 amps: f64,
 nodes: String,
 comp: String,
 pin: String,
}

/// The resolved pull-up decision handed to `ObligationForcing::wire`.
struct PullUpWire {
 part: String,
 root: String,
 rail: String,
 rail_volts: f64,
 vmin: f64,
 nodes: Vec<String>,
}

/// The resolved low-hold decision handed to `wire_low` — `part` None means
/// a direct net-to-return wire.
struct LowHoldWire {
 part: Option<String>,
 root: String,
 ret: String,
 vmax: f64,
 nodes: Vec<String>,
}

/// The voltage-obligation forcing pass — bundles the working set (union
/// find, tables, driven rails, diagnostic sinks). MIN obligations (`>=`)
/// force a pull-up to the lowest qualifying driven rail; MAX obligations
/// (`<=`) force a low-hold path to the return rail; MIN current
/// obligations force a series part.
struct ObligationForcing<'a, 'b> {
 ds: &'a mut DisjointSet,
 tables: &'a Tables<'b>,
 rails: BTreeMap<String, f64>,
 errors: &'a mut Vec<String>,
 proofs: &'a mut Vec<String>,
}

/// Entry: guard (dedup same-root obligations), then force.
#[allow(clippy::too_many_arguments)]
fn force_obligations(
 program: &HirProgram,
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 ds: &mut DisjointSet,
 obligations: &[VoltageObligation],
 current_obligations: &[CurrentObligation],
 errors: &mut Vec<String>,
 proofs: &mut Vec<String>,
) {
 let (rails, _, _) = collect_driven_rails_full(program, tables, laws, ds);
 let mut f = ObligationForcing {
 ds,
 tables,
 rails,
 errors,
 proofs,
 };
 f.run(obligations);
 f.run_current(current_obligations);
}

impl<'a, 'b> ObligationForcing<'a, 'b> {
 fn run(&mut self, obligations: &[VoltageObligation]) {
 if obligations.is_empty() {
 return;
 }
 self.assemble_buses(obligations);
 let mut mins: BTreeMap<String, (f64, String, Vec<String>)> = BTreeMap::new();
 let mut maxs: BTreeMap<String, (f64, String, Vec<String>)> = BTreeMap::new();
 for ob in obligations {
 let root = self.ds.find(&pin_key(&ob.pin.component, &ob.pin.pin));
 let table = if ob.min { &mut mins } else { &mut maxs };
 let entry = table
 .entry(root)
 .or_insert((0.0, String::new(), Vec::new()));
 if ob.volts > entry.0 {
 entry.0 = ob.volts;
 }
 entry.1 = ob.pin.component.clone();
 if !entry.2.contains(&ob.node) {
 entry.2.push(ob.node.clone());
 }
 }
 for (root, (vmin, _inst, nodes)) in mins {
 self.force_pull_up(&root, vmin, &nodes);
 }
 for (root, (vmax, inst, nodes)) in maxs {
 self.force_low(&root, &inst, vmax, &nodes);
 }
 }

 /// Bus assembly (E14b-3): MIN obligations on same-name WiredAnd-class
 /// pins at the SAME voltage union into one net. Different names,
 /// voltages, or non-WiredAnd pins never union; each keeps its own net
 /// and pull-up (honest D13).
 fn assemble_buses(&mut self, obligations: &[VoltageObligation]) {
 let mut groups: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
 let mut volts: BTreeMap<(String, i64), f64> = BTreeMap::new();
 for ob in obligations {
 if !ob.min {
 continue;
 }
 let Some(classes) = pin_classes_of(self.tables, &ob.pin) else {
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
 "bus assembled by shared min obligation '>= {}': {} — same-name WiredAnd pins",
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
 self.ds.make(first.clone());
 self.ds.make(k.clone());
 self.ds.union(first, k);
 }
 }

 /// Current obligations (E15 slice 1): a MIN current obligation on a
 /// load pin forces a free `spec SeriesPart` between the pin's net and
 /// the feeding rail. Dedup per pin, max amps wins.
 fn run_current(&mut self, obligations: &[CurrentObligation]) {
 let mut per_pin: BTreeMap<(String, String), (f64, Vec<String>)> = BTreeMap::new();
 for ob in obligations {
 let key = (ob.pin.component.clone(), ob.pin.pin.clone());
 let entry = per_pin.entry(key).or_insert((0.0, Vec::new()));
 if ob.amps > entry.0 {
 entry.0 = ob.amps;
 }
 if !entry.1.contains(&ob.node) {
 entry.1.push(ob.node.clone());
 }
 }
 for ((comp, pin), (amps, nodes)) in per_pin {
 self.force_series(&comp, &pin, amps, &nodes);
 }
 }

 /// One current obligation: the load's anode net needs a fed path
 /// through a series part. The load must declare its forward physics,
 /// the rail must exceed that drop, and a stated series resistance must
 /// actually deliver the amps.
 fn force_series(&mut self, comp: &str, pin: &str, amps: f64, nodes: &[String]) {
 let root = self.ds.find(&pin_key(comp, pin));
 if self.net_has_series_part(&root) {
 self.proofs.push(format!(
 "current obligation '>= {}' (node '{}'): net already series-fed — satisfied",
 format_amps(amps),
 nodes.join(", ")
 ));
 return;
 }
 let Some((vf, rdyn)) = self.load_forward_physics(comp) else {
 self.errors.push(format!(
 "current obligation '>= {}' (node '{}') on '{}.{}': the load declares no ForwardVoltage physics — the derived current would be a guess, not a proof. fix: declare the load's law physics (`spec ForwardVoltage` + `spec DynamicResistance` at the instance or type), or wire the series path explicitly",
 format_amps(amps),
 nodes.join(", "),
 comp,
 pin
 ));
 return;
 };
 let Some((rail, vrail)) = self.qualifying_rail_above(vf) else {
 self.errors.push(format!(
 "current obligation '>= {}' (node '{}') cannot be fed: no driven rail exceeds the load's forward drop ({}). Drive a rail or lower the drop",
 format_amps(amps),
 nodes.join(", "),
 format_volts(vf)
 ));
 return;
 };
 let Some(part) = self.pick_series_part((vrail, vf, rdyn), amps, nodes) else {
 return;
 };
 self.wire_series(SeriesWire {
 part: &part,
 root: &root,
 rail: &rail,
 vrail,
 vf,
 amps,
 nodes: nodes.join(", "),
 comp: comp.to_string(),
 pin: pin.to_string(),
 });
 }

 /// The load's forward physics — `(ForwardVoltage, DynamicResistance)`,
 /// instance value first, type default second.
 fn load_forward_physics(&self, comp: &str) -> Option<(f64, f64)> {
 let inst = self.tables.instances.get(comp)?;
 let vf = inst.specs.get("forwardvoltage").and_then(quantity_volt).or_else(|| {
 self.tables
 .type_info
 .get(&inst.type_name)
 .and_then(|ti| ti.spec_defaults.get("forwardvoltage"))
 .and_then(quantity_volt)
 })?;
 let rdyn = inst.specs.get("dynamicresistance").and_then(quantity_ohm)
 .or_else(|| {
 self.tables
 .type_info
 .get(&inst.type_name)
 .and_then(|ti| ti.spec_defaults.get("dynamicresistance"))
 .and_then(quantity_ohm)
 })
 .unwrap_or(0.0);
 Some((vf, rdyn))
 }

 /// Value-aware pick among the free series parts: the LARGEST stated
 /// resistance that still delivers `amps` from `(vrail - vf)` across
 /// `rdyn`. Candidates without a stated value are the synthesis pass's
 /// input and are skipped here.
 fn pick_series_part(
 &mut self,
 boundary: (f64, f64, f64),
 amps: f64,
 nodes: &[String],
 ) -> Option<String> {
 let (vrail, vf, rdyn) = boundary;
 let candidates = self.free_series_parts();
 let mut picked: Option<String> = None;
 let mut stated_any = false;
 for cand in &candidates {
 let Some(r_series) = self.stated_resistance(cand) else {
 continue;
 };
 stated_any = true;
 if (vrail - vf) / (r_series + rdyn) >= amps {
 picked = Some(cand.clone());
 }
 }
 if picked.is_some() {
 return picked;
 }
 if stated_any || !candidates.is_empty() {
 self.errors.push(format!(
 "current obligation '>= {}' (node '{}') cannot be met: the rail at {} minus the forward drop {} leaves {} for the series part — no free `spec SeriesPart` part states a resistance small enough to deliver the amps. fix: state a Resistance at or below {}Ohm on a free part, or add one",
 format_amps(amps),
 nodes.join(", "),
 format_volts(vrail),
 format_volts(vf),
 format_volts(vrail - vf),
 ((vrail - vf) / amps) as i64
 ));
 } else {
 self.errors.push(format!(
 "current obligation '>= {}' (node '{}') has no series part: no free `spec SeriesPart: true` two-pin part remains. Add one (e.g. a resistor) or wire the load's feed explicitly",
 format_amps(amps),
 nodes.join(", ")
 ));
 }
 None
 }

 /// Whether ANY pin of a `spec SeriesPart` type sits on the net.
 fn net_has_series_part(&mut self, root: &str) -> bool {
 let t = self.tables;
 t.instances.values().any(|c| {
 t.type_props
 .get(&c.type_name)
 .and_then(|m| m.get("series_part"))
 .and_then(property_bool)
 .unwrap_or(false)
 && t.type_pins.get(&c.type_name).map_or(false, |ps| {
 ps.iter()
 .any(|(n, _)| self.ds.find(&pin_key(&c.name, n)) == *root)
 })
 })
 }

 /// Free `spec SeriesPart` two-pin parts, sorted.
 fn free_series_parts(&self) -> Vec<String> {
 let t = self.tables;
 let mut parts: Vec<String> = t
 .instances
 .values()
 .filter(|c| {
 t.type_props
 .get(&c.type_name)
 .and_then(|m| m.get("series_part"))
 .and_then(property_bool)
 .unwrap_or(false)
 })
 .filter(|c| {
 t.type_pins
 .get(&c.type_name)
 .map_or(false, |pins| pins.len() == 2)
 })
 .filter(|c| pins_free(self.ds, c, t.type_pins))
 .map(|c| c.name.clone())
 .collect();
 parts.sort();
 parts
 }

 /// Lowest driven rail strictly ABOVE the forward drop, with its volts.
 /// Ties at the minimum are ambiguous (D13) — hard error.
 fn qualifying_rail_above(&mut self, vf: f64) -> Option<(String, f64)> {
 let mut cands: Vec<(f64, String)> = self
 .rails
 .iter()
 .filter(|(_, v)| **v > vf + f64::EPSILON)
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
 "the series feed is ambiguous: multiple rails tie at the minimal {} above the forward drop ({}). Tie the load to one rail explicitly (`inst.pin = rail.pin;`) or raise the obligation",
 best.0,
 ties.iter()
 .map(|s| s.as_str())
 .collect::<Vec<_>>()
 .join(", ")
 ));
 return None;
 }
 Some((ties[0].clone(), best.0))
 }

 /// The part's stated resistance — the instance's `spec Resistance`
 /// value, else the type-level default. `value` strings are never read
 /// as physics.
 fn stated_resistance(&self, part: &str) -> Option<f64> {
 let c = self.tables.instances.get(part)?;
 c.specs.get("resistance").and_then(quantity_ohm).or_else(|| {
 self.tables
 .type_info
 .get(&c.type_name)
 .and_then(|ti| ti.resistance)
 })
 }

 /// Wire the chosen series part between the obligation net and the rail.
 fn wire_series(&mut self, w: SeriesWire<'_>) {
 let Some(pins) = self
 .tables
 .type_pins
 .get(self.tables.instances[w.part].type_name.as_str())
 else {
 return;
 };
 let (Some(pa), Some(pb)) = (pins.first(), pins.get(1)) else {
 return;
 };
 let pa_key = pin_key(w.part, &pa.0);
 let pb_key = pin_key(w.part, &pb.0);
 self.ds.make(w.root.to_string());
 self.ds.make(pa_key.clone());
 self.ds.make(w.rail.to_string());
 self.ds.make(pb_key.clone());
 self.ds.union(w.root, &pa_key);
 self.ds.union(w.rail, &pb_key);
 self.proofs.push(format!(
 "series part forced by current obligation '>= {}' (node '{}'): {}.{} <-> {}.{} (load anode), {}.{} <-> {} rail ({}, forward drop {}) — the DC solve proves the bound through it",
 format_amps(w.amps),
 w.nodes,
 w.part,
 pa.0,
 w.comp,
 w.pin,
 w.part,
 pb.0,
 w.rail,
 format_volts(w.vrail),
 format_volts(w.vf)
 ));
 }

 /// A MIN obligation: wire a free `spec PullUp` part between the net
 /// and the lowest qualifying driven rail.
 fn force_pull_up(&mut self, root: &str, vmin: f64, nodes: &[String]) {
 if self.rails.get(root).map_or(false, |v| *v >= vmin) {
 let v = self.rails[root];
 self.proofs.push(format!(
 "voltage obligation '>= {}' (node '{}'): net already driven at {} — satisfied",
 vmin,
 nodes.join(", "),
 format_volts(v)
 ));
 return;
 }
 if self.net_has_pull_up(root) {
 self.proofs.push(format!(
 "voltage obligation '>= {}' (node '{}'): net already pulled up — satisfied",
 vmin,
 nodes.join(", ")
 ));
 return;
 }
 let Some(part) = self.free_parts().first().cloned() else {
 self.errors.push(format!(
 "voltage obligation '>= {}' (node '{}') has no pull-up part: no free `spec PullUp: true` two-pin part remains. Add one (e.g. a resistor) or wire the net to a drive explicitly",
 vmin, nodes.join(", ")
 ));
 return;
 };
 match self.qualifying_rail(vmin) {
 Some((rail, v)) => self.wire(PullUpWire {
 part,
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
 .map(|(r, v)| format!("{}@{}", v, r))
 .collect::<Vec<_>>()
 .join(", ")
 )),
 }
 }

 /// A MAX obligation: hold the net at or below Vmax by forcing a path
 /// to the instance's return rail.
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
 "voltage obligation '<= {}' (node '{}'): net is the return rail — satisfied",
 vmax,
 nodes.join(", ")
 ));
 return;
 }
 if self.net_has_switchable_to(root, &ret) {
 self.proofs.push(format!(
 "voltage obligation '<= {}' (node '{}'): net already switchable to return — satisfied",
 vmax, nodes.join(", ")
 ));
 return;
 }
 let parts = self.free_switchable_parts();
 if parts.len() > 1 && self.values_distinct(&parts) {
 self.errors.push(format!(
 "voltage obligation '<= {}' (node '{}') is ambiguous: {} switchable parts of differing value are free ({}). Wire the low path explicitly, or unpop the extras",
 vmax, nodes.join(", "), parts.len(), parts.join(", ")
 ));
 return;
 }
 let part = parts.first().cloned();
 match part {
 None => {
 if self.net_has_pull_up(root) {
 self.errors.push(format!(
 "voltage obligation '<= {}' (node '{}') cannot hold the net low: it is pulled up but no free switchable part remains to open it. Add a switch (its pins ascribe `: Path`), or wire the low path explicitly",
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
 }
 Some(part) => self.wire_low(LowHoldWire {
 part: Some(part),
 root: root.to_string(),
 ret,
 vmax,
 nodes: nodes.to_vec(),
 }),
 }
 }

 /// The net already has a `spec PullUp` part on it.
 fn net_has_pull_up(&mut self, root: &str) -> bool {
 let t = self.tables;
 t.instances.values().any(|c| {
 t.type_props
 .get(&c.type_name)
 .and_then(|m| m.get("pull_up"))
 .and_then(property_bool)
 .unwrap_or(false)
 && t.type_pins.get(&c.type_name).map_or(false, |ps| {
 ps.iter()
 .any(|(n, _)| self.ds.find(&pin_key(&c.name, n)) == *root)
 })
 })
 }

 /// Free `spec PullUp` parts, sorted.
 fn free_parts(&self) -> Vec<String> {
 let t = self.tables;
 let mut parts: Vec<String> = t
 .instances
 .values()
 .filter(|c| {
 t.type_props
 .get(&c.type_name)
 .and_then(|m| m.get("pull_up"))
 .and_then(property_bool)
 .unwrap_or(false)
 })
 .filter(|c| pins_free(self.ds, c, t.type_pins))
 .map(|c| c.name.clone())
 .collect();
 parts.sort();
 parts
 }

 /// Free switchable parts — at least two Switchable pins with at least
 /// one unconnected (the return side may already be wired), sorted.
 fn free_switchable_parts(&self) -> Vec<String> {
 let t = self.tables;
 let mut parts: Vec<String> = t
 .instances
 .values()
 .filter(|c| {
 let pins = self.switchable_pins(&c.name);
 pins.len() >= 2
 && pins
 .iter()
 .any(|(n, _)| !self.ds.contains(&pin_key(&c.name, n)))
 })
 .map(|c| c.name.clone())
 .collect();
 parts.sort();
 parts
 }

 /// The Switchable-class pins of an instance, sorted (index order).
 fn switchable_pins(&self, inst: &str) -> Vec<(String, u64)> {
 let Some(c) = self.tables.instances.get(inst) else {
 return Vec::new();
 };
 let Some(ti) = self.tables.type_info.get(&c.type_name) else {
 return Vec::new();
 };
 ti.pins
 .iter()
 .enumerate()
 .filter(|(i, _)| ti.pin_classes[*i].switchable)
 .map(|(_, p)| p.clone())
 .collect()
 }

 /// The instance's own Return-class pin root.
 fn return_root(&mut self, inst: &str) -> Option<String> {
 let c = self.tables.instances.get(inst)?;
 let ti = self.tables.type_info.get(&c.type_name)?;
 let ret = ti
 .pins
 .iter()
 .enumerate()
 .find(|(i, _)| ti.pin_classes[*i].return_pin)
 .map(|(_, p)| p)?;
 Some(self.ds.find(&pin_key(inst, &ret.0)))
 }

 /// Any connected Return-class pin root, deterministically chosen.
 fn any_return_root(&mut self) -> Option<String> {
 let mut roots: Vec<String> = self
 .tables
 .instances
 .values()
 .filter_map(|c| {
 let ti = self.tables.type_info.get(&c.type_name)?;
 let ret = ti
 .pins
 .iter()
 .enumerate()
 .find(|(i, _)| ti.pin_classes[*i].return_pin)
 .map(|(_, p)| p)?;
 let key = pin_key(&c.name, &ret.0);
 self.ds.contains(&key).then(|| self.ds.find(&key))
 })
 .collect();
 roots.sort();
 roots.first().cloned()
 }

 /// Whether a declared switchable part already spans the two nets.
 fn net_has_switchable_to(&mut self, root: &str, ret: &str) -> bool {
 let Some(inst) = self
 .tables
 .instances
 .values()
 .find(|c| self.switchable_pins(&c.name).len() >= 2)
 else {
 return false;
 };
 let pins = self.switchable_pins(&inst.name);
 pins.iter()
 .any(|(n, _)| self.ds.find(&pin_key(&inst.name, n)) == *root)
 && pins
 .iter()
 .any(|(n, _)| self.ds.find(&pin_key(&inst.name, n)) == *ret)
 }

 /// Whether the free parts carry more than one distinct value — the
 /// pick then MATTERS (pull-up strength) and D13 demands explicit wiring.
 fn values_distinct(&self, parts: &[String]) -> bool {
 let distinct: std::collections::BTreeSet<&str> = parts
 .iter()
 .filter_map(|p| {
 self.tables.instances[p]
 .properties
 .iter()
 .find(|(k, _)| k == "value")
 .map(|(_, v)| v.as_str())
 })
 .collect();
 distinct.len() > 1
 }

 /// Lowest driven rail at or above `vmin`. None when no rail qualifies
 /// OR when multiple rails tie at the minimal volts — both are hard
 /// errors, surfaced by the caller's message.
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
 "voltage obligation '>= {}' is ambiguous: multiple rails tie at the minimal {} ({}). Tie the net to one rail explicitly (`inst.pin = rail.pin;` in a node guard) or raise the obligation",
 vmin, best.0, ties.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
 ));
 return None;
 }
 Some((ties[0].clone(), best.0))
 }

 /// Wire the chosen pull-up part between the obligation net and the rail.
 fn wire(&mut self, w: PullUpWire) {
 let Some(pi) = self
 .tables
 .type_pins
 .get(self.tables.instances[w.part.as_str()].type_name.as_str())
 else {
 return;
 };
 let (Some(pa), Some(pb)) = (pi.first(), pi.get(1)) else {
 return;
 };
 let pa_key = pin_key(&w.part, &pa.0);
 let pb_key = pin_key(&w.part, &pb.0);
 self.ds.make(w.root.clone());
 self.ds.make(pa_key.clone());
 self.ds.make(w.rail.clone());
 self.ds.make(pb_key.clone());
 self.ds.union(&w.root, &pa_key);
 self.ds.union(&w.rail, &pb_key);
 self.proofs.push(format!(
 "pull-up forced by '>= {}' (node '{}'): {}.{} <-> obligation net, {}.{} <-> rail ({}) — obligation satisfied",
 w.vmin, w.nodes.join(", "), w.part, pa.0, w.part, pb.0, format_volts(w.rail_volts)
 ));
 }

 /// 2026-09-25 (E14b-6): the single-free-pin low-path completion —
 /// the part's wired sibling sits on the obligation net (p1-pre-wired)
 /// or on the return rail; the free pin completes the opposite side.
 fn complete_single_free_low(&mut self, w: &LowHoldWire, part: &str, pins: &[(String, u64)], pa: &(String, u64)) {
 let pa_key = pin_key(part, &pa.0);
 let sibling_on_ret = pins
 .iter()
 .any(|(n, _)| n != &pa.0 && self.ds.find(&pin_key(part, n)) == w.ret);
 let sibling_on_root = pins
 .iter()
 .any(|(n, _)| n != &pa.0 && self.ds.find(&pin_key(part, n)) == w.root);
 self.ds.make(pa_key.clone());
 if sibling_on_ret {
 self.ds.make(w.root.clone());
 self.ds.union(&pa_key, &w.root);
 self.proofs.push(format!(
 "low-hold forced by '<= {}' (node '{}'): {}.{} <-> net, other path pin on return rail — obligation satisfied",
 w.vmax,
 w.nodes.join(", "),
 part,
 pa.0
 ));
 return;
 }
 if sibling_on_root {
 self.ds.make(w.ret.clone());
 self.ds.union(&pa_key, &w.ret);
 self.proofs.push(format!(
 "low-hold forced by '<= {}' (node '{}'): {}.{} <-> return rail, sibling path pin already on the net — obligation satisfied",
 w.vmax,
 w.nodes.join(", "),
 part,
 pa.0
 ));
 return;
 }
 self.errors.push(format!(
 "low-hold obligation '<= {}' (node '{}'): {}.{} has only one free switchable pin and its other path pin is neither on the net nor on the return rail — it cannot complete the low path. Wire the low path explicitly",
 w.vmax,
 w.nodes.join(", "),
 part,
 pa.0
 ));
 }

 /// Wire the low-hold path: net → (switchable part) → return rail.
 fn wire_low(&mut self, w: LowHoldWire) {
 let Some(part) = &w.part else {
 self.ds.make(w.root.clone());
 self.ds.make(w.ret.clone());
 self.ds.union(&w.root, &w.ret);
 self.proofs.push(format!(
 "low-hold forced by '<= {}' (node '{}'): net <-> return rail — obligation satisfied",
 w.vmax, w.nodes.join(", ")
 ));
 return;
 };
 let pins = self.switchable_pins(part);
 let mut free: Vec<&(String, u64)> = pins
 .iter()
 .filter(|(n, _)| !self.ds.contains(&pin_key(part, n)))
 .collect();
 free.sort_by(|a, b| a.0.cmp(&b.0));
 let Some(pa) = free.first() else {
 return;
 };
 if free.len() == 1 {
 let pa = (*pa).clone();
 self.complete_single_free_low(&w, part, &pins, &pa);
 return;
 }
 let pa_key = pin_key(part, &pa.0);
 self.ds.make(w.root.clone());
 self.ds.make(pa_key.clone());
 self.ds.union(&w.root, &pa_key);
 if pins
 .iter()
 .any(|(n, _)| self.ds.find(&pin_key(part, n)) == w.ret)
 {
 self.proofs.push(format!(
 "low-hold forced by '<= {}' (node '{}'): {}.{} <-> net, other path pin on return rail — obligation satisfied",
 w.vmax, w.nodes.join(", "), part, pa.0
 ));
 return;
 }
 let pb = free[1];
 let pb_key = pin_key(part, &pb.0);
 self.ds.make(w.ret.clone());
 self.ds.make(pb_key.clone());
 self.ds.union(&w.ret, &pb_key);
 self.proofs.push(format!(
 "low-hold forced by '<= {}' (node '{}'): {}.{} <-> net, {}.{} <-> return rail — obligation satisfied",
 w.vmax, w.nodes.join(", "), part, pa.0, part, pb.0
 ));
 }
}

/// Quantity property pickers (SI + dimension) shared by the physics passes.
fn quantity_volt(pv: &PropertyValue) -> Option<f64> {
 match pv {
 PropertyValue::Quantity { si, dimension } if *dimension == QuantityDim::Volt => Some(*si),
 _ => None,
 }
}

fn quantity_amp(pv: &PropertyValue) -> Option<f64> {
 match pv {
 PropertyValue::Quantity { si, dimension } if *dimension == QuantityDim::Amp => Some(*si),
 _ => None,
 }
}

fn quantity_ohm(pv: &PropertyValue) -> Option<f64> {
 match pv {
 PropertyValue::Quantity { si, dimension } if *dimension == QuantityDim::Ohm => Some(*si),
 _ => None,
 }
}

// ── Series synthesis (E15 slice 2, HIR port) ────────────────────────────

/// The synthesis pre-pass's read-only tables (no instances — the pass
/// builds its own map from the mutable instance list).
#[derive(Clone, Copy)]
struct SynthesisTables<'a> {
 type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
 type_info: &'a BTreeMap<String, TypeInfo>,
 type_props: &'a BTreeMap<String, &'a BTreeMap<String, PropertyValue>>,
}

/// One synthesis decision — the window, the E24 pick, and the obligation.
struct SynthesisDecision {
 comp: String,
 pin: String,
 amps: f64,
 imax: f64,
 value: f64,
 rmin: f64,
 rmax: f64,
 vrail: f64,
}

/// The synthesis pre-pass's working set.
struct SynthesisContext<'a> {
 instances: BTreeMap<String, &'a ComponentInstance>,
 obligations: BTreeMap<(String, String), f64>,
 tables: SynthesisTables<'a>,
 probe_laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
}

/// The two outputs the decision loop writes: decisions and refusals.
struct SynthesisOut {
 decisions: Vec<SynthesisDecision>,
 errors: Vec<String>,
}

/// The one free `spec SeriesPart` two-pin part with no stated resistance —
/// the sorted-first candidate is the deterministic synthesis target (D13).
fn free_series_part_name(
 instance_list: &[ComponentInstance],
 ds: &DisjointSet,
 tables: &SynthesisTables<'_>,
 taken: &HashSet<String>,
) -> Option<String> {
 let mut names: Vec<&str> = instance_list.iter().map(|c| c.name.as_str()).collect();
 names.sort_unstable();
 for name in names {
 if taken.contains(name) {
 continue;
 }
 let Some(c) = instance_list.iter().find(|c| c.name == name) else {
 continue;
 };
 let is_series = tables
 .type_props
 .get(&c.type_name)
 .and_then(|m| m.get("series_part"))
 .and_then(property_bool)
 .unwrap_or(false);
 if !is_series {
 continue;
 }
 let Some(pins) = tables.type_pins.get(&c.type_name) else {
 continue;
 };
 if pins.len() != 2 || c.specs.contains_key("resistance") {
 continue;
 }
 let free = pins
 .iter()
 .all(|(n, _)| !ds.contains(&pin_key(&c.name, n)));
 if free {
 return Some(name.to_string());
 }
 }
 None
}

/// MIN current obligations from node bodies, deduped per load pin (max
/// amps wins). Resolution uses the same instances the synthesis pass sees.
fn collect_min_current_obligations(
 program: &HirProgram,
 instance_list: &[ComponentInstance],
 tables: &SynthesisTables<'_>,
) -> BTreeMap<(String, String), f64> {
 let instances: BTreeMap<String, &ComponentInstance> =
 instance_list.iter().map(|c| (c.name.clone(), c)).collect();
 let mut obligations: BTreeMap<(String, String), f64> = BTreeMap::new();
 for callable in all_transactions(program) {
 collect_current_obligations_stmts(
 &callable.body.statements,
 &instances,
 tables.type_pins,
 &mut obligations,
 );
 }
 obligations
}

/// Walk a node body (through guarded regions) collecting MIN current
/// obligations.
fn collect_current_obligations_stmts(
 body: &[HirStmt],
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
 out: &mut BTreeMap<(String, String), f64>,
) {
 for stmt in body {
 match &stmt.kind {
 HirStmtKind::Guarded(_, inner) => {
 collect_current_obligations_stmts(inner, instances, type_pins, out);
 }
 HirStmtKind::Expression(expr) => {
 collect_current_from_expr(expr, instances, type_pins, out);
 }
 _ => {}
 }
 }
}

/// One expression: a Ge/Gt binary op with a current pin on one side and a
/// bare amp value on the other is a MIN current obligation.
fn collect_current_from_expr(
 expr: &HirExpr,
 instances: &BTreeMap<String, &ComponentInstance>,
 type_pins: &BTreeMap<String, Vec<(String, u64)>>,
 out: &mut BTreeMap<(String, String), f64>,
) {
 let HirExprKind::Binary { op, left, right } = &expr.kind else {
 return;
 };
 if !matches!(op, BinaryOpKind::Ge | BinaryOpKind::Gt) {
 return;
 }
 let (l, r) = (left.as_ref(), right.as_ref());
 let Some(p) = resolve_current_pin(l, instances, type_pins)
 .or_else(|| resolve_current_pin(r, instances, type_pins))
 else {
 return;
 };
 let Some(v) = extract_current(l).or_else(|| extract_current(r)) else {
 return;
 };
 let key = (p.component.clone(), p.pin.clone());
 let entry = out.entry(key).or_insert(0.0);
 if v > *entry {
 *entry = v;
 }
}

/// Decide one obligation's synthesis: the window, the E24 pick, or the
/// error that explains why there is no pick.
fn decide_synthesis(
 program: &HirProgram,
 comp: &str,
 pin: &str,
 amps: f64,
 ctx: &SynthesisContext<'_>,
 out: &mut SynthesisOut,
) {
 let Some(inst) = ctx.instances.get(comp) else {
 return;
 };
 let Some((vf, rdyn)) = load_forward_physics_raw(inst, ctx.tables.type_info) else {
 return;
 };
 let Some(imax) = inst.specs.get("max_current").and_then(quantity_amp) else {
 return;
 };
 let Some(vrail) = feeding_rail_volts(program, vf, ctx) else {
 return;
 };
 let rmin = (vrail - vf) / imax - rdyn;
 let rmax = (vrail - vf) / amps - rdyn;
 if rmin > rmax {
 out.errors.push(format!(
 "the current obligation '>= {}' on '{}.{}' has an inverted E-series window: the MaxCurrent bound ({}) needs at most {} ohm, but the obligation needs at least {} ohm. fix: lower the obligation or raise the load's MaxCurrent",
 format_amps(amps),
 comp,
 pin,
 format_amps(imax),
 rmin as i64,
 rmax as i64
 ));
 return;
 }
 let Some(value) = crate::analysis::electronics::e_series_value_in_window(rmin, rmax) else {
 out.errors.push(format!(
 "the current obligation '>= {}' on '{}.{}' has an empty E-series window: no preferred value lands in [{}, {}] ohm. fix: adjust the obligation, the rail, or state the resistance outright",
 format_amps(amps),
 comp,
 pin,
 rmin as i64,
 rmax as i64
 ));
 return;
 };
 out.decisions.push(SynthesisDecision {
 comp: comp.to_string(),
 pin: pin.to_string(),
 amps,
 imax,
 value,
 rmin,
 rmax,
 vrail,
 });
}

/// Apply every decision: inject the resistance into the target part's
/// specs and value string, and record the proof line.
fn apply_syntheses(
 instance_list: &mut [ComponentInstance],
 ds: &DisjointSet,
 tables: &SynthesisTables<'_>,
 decisions: &[SynthesisDecision],
 proofs: &mut Vec<String>,
) {
 let mut taken: HashSet<String> = HashSet::new();
 for d in decisions {
 let Some(part) = free_series_part_name(instance_list, ds, tables, &taken) else {
 continue;
 };
 taken.insert(part.clone());
 let Some(c) = instance_list.iter_mut().find(|c| c.name == part) else {
 continue;
 };
 c.specs.insert(
 "resistance".to_string(),
 PropertyValue::Quantity {
 si: d.value,
 dimension: QuantityDim::Ohm,
 },
 );
 c.properties.retain(|(k, _)| k != "value");
 c.properties
 .push(("value".to_string(), format!("{}Ohm", d.value as i64)));
 proofs.push(format!(
 "E-series synthesis: {} := {}Ohm (window [{}, {}] ohm from the {} rail)",
 c.name,
 d.value as i64,
 d.rmin as i64,
 d.rmax as i64,
 format_volts(d.vrail)
 ));
 }
}

/// 2026-09-26 (E15 slice 2): synthesize unstated series-part resistances
/// from the E-series window. For every MIN current obligation on a load
/// pin, the rail volts and the load's `MaxCurrent` bound give the
/// resistance window; the smallest E-series step inside it is injected
/// into the sorted-first FREE `spec SeriesPart` part that states no
/// resistance of its own. Runs BEFORE the real law elaboration.
fn synthesize_series_values(
 program: &HirProgram,
 instance_list: &mut [ComponentInstance],
 ds: &DisjointSet,
 tables: &SynthesisTables<'_>,
 probe_laws: &[crate::analysis::electronics_laws::ComponentLaws],
 proofs: &mut Vec<String>,
) -> Vec<String> {
 let ctx = SynthesisContext {
 instances: instance_list.iter().map(|c| (c.name.clone(), c)).collect(),
 obligations: {
 let obligations = collect_min_current_obligations(program, instance_list, tables);
 obligations
 },
 tables: *tables,
 probe_laws,
 };
 let mut out = SynthesisOut {
 decisions: Vec::new(),
 errors: Vec::new(),
 };
 let obligations: Vec<((String, String), f64)> = ctx
 .obligations
 .iter()
 .map(|(k, v)| (k.clone(), *v))
 .collect();
 for ((comp, pin), amps) in obligations {
 decide_synthesis(program, &comp, &pin, amps, &ctx, &mut out);
 }
 apply_syntheses(instance_list, ds, tables, &out.decisions, proofs);
 out.errors
}

/// The load's forward physics from raw tables — `(ForwardVoltage,
/// DynamicResistance)`, instance value first, type default second.
fn load_forward_physics_raw(
 inst: &ComponentInstance,
 type_info: &BTreeMap<String, TypeInfo>,
) -> Option<(f64, f64)> {
 let ti = type_info.get(&inst.type_name);
 let vf = inst.specs.get("forwardvoltage").and_then(quantity_volt).or_else(|| {
 ti.and_then(|ti| ti.spec_defaults.get("forwardvoltage"))
 .and_then(quantity_volt)
 })?;
 let rdyn = inst.specs.get("dynamicresistance").and_then(quantity_ohm)
 .or_else(|| {
 ti.and_then(|ti| ti.spec_defaults.get("dynamicresistance"))
 .and_then(quantity_ohm)
 })
 .unwrap_or(0.0);
 Some((vf, rdyn))
}

/// The volts of the rail that would feed a load pin: the LOWEST driven
/// supply above the forward drop. None when no supply qualifies.
fn feeding_rail_volts(program: &HirProgram, vf: f64, ctx: &SynthesisContext<'_>) -> Option<f64> {
 collect_supply_rail_candidates(program, ctx)
 .into_iter()
 .filter(|volts| *volts > vf + f64::EPSILON)
 .min_by(f64::total_cmp)
}

/// All driven supply-pin volts from both sources, pre-filtered to the
/// rail-class pins.
fn collect_supply_rail_candidates(program: &HirProgram, ctx: &SynthesisContext<'_>) -> Vec<f64> {
 let mut out: Vec<f64> = contract_drive_rail_volts(program, ctx);
 out.extend(law_birth_rail_volts(ctx));
 out
}

/// Contract voltage drives on supply pins (pre + post conditions).
fn contract_drive_rail_volts(program: &HirProgram, ctx: &SynthesisContext<'_>) -> Vec<f64> {
 let mut out: Vec<f64> = Vec::new();
 let mut eqs: Vec<(&HirExpr, &HirExpr)> = Vec::new();
 for callable in all_transactions(program) {
 let Some(contract) = &callable.contract else {
 continue;
 };
 collect_eq_pairs(&contract.pre, &mut eqs);
 collect_eq_pairs(&contract.post, &mut eqs);
 }
 for (l, r) in eqs {
 if let Some((pin, v)) = voltage_drive(l, r, &ctx.instances, ctx.tables.type_pins) {
 if is_supply_pin(&pin, ctx) {
 out.push(v);
 }
 }
 }
 out
}

/// Law-born rails from the probe elaboration (E14b-8).
fn law_birth_rail_volts(ctx: &SynthesisContext<'_>) -> Vec<f64> {
 let mut out: Vec<f64> = Vec::new();
 for component in ctx.probe_laws {
 for law in &component.laws {
 if law.guard != crate::analysis::electronics_laws::LawGuard::Always {
 continue;
 }
 for volts in law_birth_from_law(law, ctx) {
 out.push(volts);
 }
 }
 }
 out
}

/// One law's always-guarded constant-voltage equations on supply pins.
fn law_birth_from_law(
 law: &crate::analysis::electronics_laws::ElaboratedLaw,
 ctx: &SynthesisContext<'_>,
) -> Vec<f64> {
 law.equations
 .iter()
 .filter_map(|eq| crate::analysis::electronics::constant_voltage_pin(&eq.expression))
 .filter(|(pin, _)| is_supply_pin(pin, ctx))
 .map(|(_, volts)| volts)
 .collect()
}

/// True when the pin's class is supply-classified.
fn is_supply_pin(pin: &PinRef, ctx: &SynthesisContext<'_>) -> bool {
 ctx.instances
 .get(&pin.component)
 .and_then(|inst| ctx.tables.type_info.get(&inst.type_name))
 .and_then(|ti| {
 ti.pins
 .iter()
 .position(|(n, _)| n == &pin.pin)
 .and_then(|i| ti.pin_classes.get(i))
 })
 .map_or(false, |c| c.supply)
}

// ── Post-solve: DC states, mode validation, proofs, budgets ─────────────

/// Everything the proof passes need after contract drives are classified.
struct ProofInputs<'a> {
 program: &'a HirProgram,
 nets: &'a [Net],
 tables: &'a Tables<'a>,
 laws: &'a [crate::analysis::electronics_laws::ComponentLaws],
 states: &'a std::collections::BTreeSet<String>,
 modes: &'a BTreeMap<String, Vec<String>>,
 assigned_modes: &'a BTreeMap<String, String>,
 unpop: &'a HashSet<String>,
}

/// Solve laws, derive electrical quantities, prove bounds, and check
/// budgets. Returns `(law errors, budget errors)`; both are hard at the
/// emitter. The DC solve is reused verbatim from `analysis::electronics_dc`
/// (its inputs are only shared result types).
#[allow(clippy::too_many_arguments)]
fn post_solve_checks(
 program: &HirProgram,
 nets: &[Net],
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 unpop: &HashSet<String>,
 dc_drives: &BTreeMap<String, f64>,
 voltage: &mut VoltageCheck,
) -> (Vec<String>, Vec<String>) {
 let modes = mode_catalog(tables);
 let (states, law_errors) = solve_law_states(nets, tables, laws, unpop, dc_drives);
 let mut law_all = law_errors;
 law_all.extend(validate_node_mode_states(program, &states, &modes));
 let mut budget_all = Vec::new();
 let mut proved_all = voltage.proved.clone();
 let mut representative: Option<crate::analysis::electronics_dc::DcSolution> = None;
 for state in states {
 let mut state_check = voltage.clone();
 state_check.violations.clear();
 state_check.net_voltage.extend(state.net_voltage.clone());
 state_check.pin_current.extend(state.pin_current.clone());
 check_tolerance(
 nets,
 tables.instances,
 tables.type_info,
 &state_check.net_voltage,
 &mut state_check.violations,
 );
 derive_electrical_proofs(
 ProofInputs {
 program,
 nets,
 tables,
 laws,
 states: &state.states,
 modes: &modes,
 assigned_modes: &state.modes,
 unpop,
 },
 &mut state_check,
 );
 let state_budget = check_budgets(program, tables, nets, laws, &state.states, &state_check);
 budget_all.extend(state_budget);
 voltage.violations.extend(state_check.violations);
 voltage.net_current.extend(state_check.net_current);
 for (net, volt) in state_check.net_voltage {
 voltage.net_voltage.entry(net).or_insert(volt);
 }
 proved_all.extend(state_check.proved);
 representative.get_or_insert(state.clone());
 }
 // Record the mandatory absent omission explicitly.
 for law in laws {
 if unpop.contains(&law.instance) {
 proved_all.push(format!(
 "[participation=absent] component-law part '{}' omitted — pins open, no law contribution",
 law.instance
 ));
 }
 }
 if let Some(state) = representative {
 voltage.net_voltage.extend(state.net_voltage);
 voltage.pin_current.extend(state.pin_current);
 }
 voltage.proved = proved_all;
 (law_all, budget_all)
}

/// Solve mandatory law participation states. Present always runs; absent
/// runs only when at least one law-bearing component is unpopulated.
fn solve_law_states(
 nets: &[Net],
 tables: &Tables<'_>,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 unpop: &HashSet<String>,
 drives: &BTreeMap<String, f64>,
) -> (Vec<crate::analysis::electronics_dc::DcSolution>, Vec<String>) {
 let mut states = Vec::new();
 let mut errors = Vec::new();
 let mut solve_participation = |label: &str,
 components: &[crate::analysis::electronics_laws::ComponentLaws],
 solve_unpop: &HashSet<String>| {
 let (mut solved, mut solve_errors) = crate::analysis::electronics_dc::solve_dc_laws(
 &crate::analysis::electronics_dc::DcContext {
 nets,
 components,
 instances: tables.instances,
 type_pins: tables.type_pins,
 type_info: tables.type_info,
 unpop: solve_unpop,
 drives,
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
 solve_participation("present", laws, &HashSet::new());
 if !unpop.is_empty() {
 let absent: Vec<crate::analysis::electronics_laws::ComponentLaws> = laws
 .iter()
 .filter(|law| !unpop.contains(&law.instance))
 .cloned()
 .collect();
 solve_participation("absent", &absent, unpop);
 }
 (states, errors)
}

/// Catalog of declared named modes per component instance.
fn mode_catalog(tables: &Tables<'_>) -> BTreeMap<String, Vec<String>> {
 tables
 .instances
 .iter()
 .filter_map(|(instance, inst)| {
 tables.type_info.get(&inst.type_name).and_then(|info| {
 (!info.modes.is_empty()).then(|| (instance.clone(), info.modes.clone()))
 })
 })
 .collect()
}

/// Every node with a mode-selection precondition must match at least one
/// solved global state.
fn validate_node_mode_states(
 program: &HirProgram,
 states: &[crate::analysis::electronics_dc::DcSolution],
 modes: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
 let mut out = Vec::new();
 for callable in all_transactions(program) {
 let Some(contract) = &callable.contract else {
 continue;
 };
 if let Some(error) = validate_one_mode_node(callable, &contract.pre, states, modes) {
 out.push(error);
 }
 }
 out
}

/// Evaluate one node across all states and explain zero-match failures.
fn validate_one_mode_node(
 callable: &super::program::HirCallable,
 pre: &HirExpr,
 states: &[crate::analysis::electronics_dc::DcSolution],
 modes: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
 let mut saw_mode = false;
 let mut matched = false;
 for state in states {
 let evaluated = match eval_mode_predicate(pre, modes, &state.modes) {
 Ok(evaluated) => evaluated,
 Err(error) => return Some(format!("[node '{}'] {error}", callable.name)),
 };
 saw_mode |= evaluated.referenced;
 matched |= evaluated.value;
 }
 if saw_mode && !matched {
 return Some(format!(
 "[node '{}'] its mode precondition matches none of the {} solved operating states — \
 the node is physically unsatisfiable. fix: select a declared mode that the law \
 solver can reach",
 callable.name,
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

/// Boolean value plus whether the predicate referenced a component mode.
#[derive(Debug, Clone, Copy)]
struct ModeEvaluation {
 value: bool,
 referenced: bool,
}

fn always_true_evaluation() -> ModeEvaluation {
 ModeEvaluation {
 value: true,
 referenced: false,
 }
}

/// Evaluate a node precondition against one named-mode assignment (HIR
/// recursion). Non-mode operands are treated as true for state selection;
/// an OR mixing a mode with a non-mode guard is rejected, not widened.
fn eval_mode_predicate(
 expr: &HirExpr,
 modes: &BTreeMap<String, Vec<String>>,
 assigned: &BTreeMap<String, String>,
) -> Result<ModeEvaluation, String> {
 match &expr.kind {
 HirExprKind::Literal(HirLiteral::Bool(value)) => Ok(ModeEvaluation {
 value: *value,
 referenced: false,
 }),
 HirExprKind::Identifier(_) => Ok(always_true_evaluation()),
 HirExprKind::Field { receiver, name } => {
 let HirExprKind::Identifier(instance) = &receiver.kind else {
 return Ok(always_true_evaluation());
 };
 let Some(declared) = modes.get(instance) else {
 return Ok(always_true_evaluation());
 };
 if !declared.contains(name) {
 return Err(format!(
 "state assertion '{instance}.{name}' names an undeclared mode — declared modes: {}",
 declared.join(", ")
 ));
 }
 Ok(ModeEvaluation {
 value: assigned.get(instance).is_some_and(|selected| selected == name),
 referenced: true,
 })
 }
 HirExprKind::Unary {
 op: UnaryOpKind::Not,
 inner,
 } => {
 let value = eval_mode_predicate(inner, modes, assigned)?;
 Ok(ModeEvaluation {
 value: !value.value,
 referenced: value.referenced,
 })
 }
 HirExprKind::Binary { op, left, right } => {
 let l = eval_mode_predicate(left, modes, assigned)?;
 let r = eval_mode_predicate(right, modes, assigned)?;
 combine_mode_predicates(l, r, *op == BinaryOpKind::Or)
 }
 _ => Ok(always_true_evaluation()),
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
 "an OR state selector mixes a component mode with a non-mode guard — state applicability is not decidable. fix: put the non-mode guard in a separate condition or use AND".to_string(),
 );
 }
 Ok(ModeEvaluation {
 value: if is_or { left.value || right.value } else { left.value && right.value },
 referenced: left.referenced || right.referenced,
 })
}

/// Evaluate a node precondition's mode selection for one bound site.
fn mode_predicate_holds(
 expr: &HirExpr,
 transaction: &str,
 modes: &BTreeMap<String, Vec<String>>,
 assigned: &BTreeMap<String, String>,
) -> Result<bool, String> {
 eval_mode_predicate(expr, modes, assigned)
 .map(|evaluated| evaluated.value)
 .map_err(|error| format!("[node '{transaction}'] {error}"))
}

/// Derive legacy current classes, law-aware power, and prove current
/// bounds — called after the DC solve so law quantities participate.
fn derive_electrical_proofs(input: ProofInputs<'_>, check: &mut VoltageCheck) {
 let pin_to_net = pin_net_index(input.nets);
 derive_current(
 &pin_to_net,
 input.tables.instances,
 input.tables.type_info,
 check.net_voltage.clone(),
 check,
 );
 derive_power(&pin_to_net, input.tables, check);
 derive_law_power(&input, &pin_to_net, check);
 check_current_bounds(&input, &pin_to_net, check);
 check_max_current(input.tables.instances, input.tables.type_info, &pin_to_net, check, input.unpop);
 check_drive_capability(&input, &pin_to_net, check);
}

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
/// opaque `value` label is never parsed.
fn collect_series_parts(
 instances: &BTreeMap<String, &ComponentInstance>,
 type_info: &BTreeMap<String, TypeInfo>,
 pin_to_net: &BTreeMap<(String, String), String>,
) -> Vec<SeriesPart> {
 let mut parts = Vec::new();
 for inst in instances.values() {
 let Some(info) = type_info.get(&inst.type_name) else {
 continue;
 };
 if info.pins.len() != 2 || info.has_laws {
 continue;
 };
 let ohms = inst.specs.get("resistance").and_then(quantity_ohm)
 .filter(|r| *r > 0.0)
 .or_else(|| info.resistance.filter(|r| *r > 0.0));
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
 raw: crate::analysis::electronics::format_ohms_pub(ohms),
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
 divided: HashSet<String>,
}

/// Is `net` a divider mid node? Unclassed, not driven, exactly two attached
/// parts, both far sides classed, and the far sides are DIFFERENT nets.
fn is_divider_mid(net: &str, g: &PartGraph, drives: &BTreeMap<String, f64>) -> bool {
 if g.voltage.contains_key(net) || drives.contains_key(net) || g.divided.contains(net) {
 return false;
 }
 let Some(idxs) = g.attached.get(net) else {
 return false;
 };
 if idxs.len() != 2 {
 return false;
 }
 let (pa, pb) = (&g.parts[idxs[0]], &g.parts[idxs[1]]);
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
 let (pa, pb) = (&g.parts[idxs[0]], &g.parts[idxs[1]]);
 let far_a = if pa.net_a == mid { &pa.net_b } else { &pa.net_a };
 let far_b = if pb.net_a == mid { &pb.net_b } else { &pb.net_a };
 let (va, vb) = (g.voltage[far_a], g.voltage[far_b]);
 let v_mid = (va / pa.ohms + vb / pb.ohms) / (1.0 / pa.ohms + 1.0 / pb.ohms);
 g.voltage.insert(mid.clone(), v_mid);
 g.divided.insert(mid.clone());
 any = true;
 proved.push(format!(
 "V({}) = {} — voltage divider ({} = {} and {} = {})",
 mid,
 format_volts(v_mid),
 far_a,
 format_volts(va),
 far_b,
 format_volts(vb)
 ));
 }
 any
}

/// (b)+(c) Contribution + KCL pass: per-part Ohm currents rebuilt from
/// scratch against the current voltage classes, then SUMMED per net.
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
 contributions
 .entry(target.clone())
 .or_default()
 .push(current);
 check.proved.push(format!(
 "I({} -> {}) = {} / {} ({} = {}) = {} — Ohm's law through the series part",
 source_net,
 target,
 format_volts(across),
 format_volts(p.ohms),
 p.name,
 p.raw,
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
/// series current, voltage dividers, and Kirchhoff parallel summing.
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
 check.net_voltage = g.voltage;
}

/// The effective power rating for one part — the instance's `spec Rating`,
/// else the type-level envelope. Watt quantities only.
fn rating_for(inst: &ComponentInstance, ti: &TypeInfo) -> Option<f64> {
 match inst.specs.get("rating") {
 Some(PropertyValue::Quantity { si, dimension }) if *dimension == QuantityDim::Watt => {
 Some(*si)
 }
 _ => ti.rating,
 }
}

/// 2026-09-12 (power ratings): P = V²/R per part from the FINAL voltage
/// classes. A part with both endpoints classed and ΔV > 0 has PROVEN
/// dissipation — the rating clause must declare the decision.
fn derive_power(
 pin_to_net: &BTreeMap<(String, String), String>,
 tables: &Tables<'_>,
 check: &mut VoltageCheck,
) {
 let parts = collect_series_parts(tables.instances, tables.type_info, pin_to_net);
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
 let Some(inst) = tables.instances.get(&p.name) else {
 continue;
 };
 let Some(info) = tables.type_info.get(&inst.type_name) else {
 continue;
 };
 match rating_for(inst, info) {
 None => check.violations.push(format!(
 "part '{}' ({}, {}) dissipates a derived {} but its type declares no power rating — \
 an unstated rating on a proven-dissipating part is an undeclared decision. \
 fix: add `spec Rating: <watts>W;` rated above the derived dissipation, or `spec Rating: any;` \
 to declare the part unrated on purpose.",
 p.name,
 inst.type_name,
 p.raw,
 crate::analysis::electronics::format_watts_pub(watts)
 )),
 Some(rated) if watts > rated + f64::EPSILON => check.violations.push(format!(
 "part '{}' ({}, {}) dissipates a derived {} but is rated {} — the derived physics \
 exceeds the declared rating. fix: raise the rating to a real part class, spread the \
 drop across more parts, or lower the drive.",
 p.name,
 inst.type_name,
 p.raw,
 crate::analysis::electronics::format_watts_pub(watts),
 crate::analysis::electronics::format_watts_pub(rated)
 )),
 Some(rated) => check.proved.push(format!(
 "P({}) = {} <= {} rating — within the declared rating",
 p.name,
 crate::analysis::electronics::format_watts_pub(watts),
 crate::analysis::electronics::format_watts_pub(rated)
 )),
 }
 }
}

/// Compute one law part's absorbed power. `None` when any pin lacks a
/// solved voltage/current pair.
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
 Some((power.abs(), "component-law DC:"))
}

/// 2026-09-24 (Slice 6): component-law power proof. A fully solved law
/// part has an operating point at every pin, so absorbed power is `Σ V·I`.
fn derive_law_power(
 input: &ProofInputs<'_>,
 pin_to_net: &BTreeMap<(String, String), String>,
 check: &mut VoltageCheck,
) {
 let state = state_prefix(input.states);
 for law in input.laws {
 let Some(inst) = input.tables.instances.get(&law.instance) else {
 continue;
 };
 let Some(info) = input.tables.type_info.get(&inst.type_name) else {
 continue;
 };
 let Some(pins) = input.tables.type_pins.get(&inst.type_name) else {
 continue;
 };
 let Some((power, proof)) = law_power(
 &law.instance,
 pins,
 pin_to_net,
 &check.net_voltage,
 &check.pin_current,
 ) else {
 continue;
 };
 if power <= f64::EPSILON {
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
 crate::analysis::electronics::format_watts_pub(power)
 )),
 Some(rated) if power > rated + f64::EPSILON => check.violations.push(format!(
 "{state}component-law part '{}' ({}) dissipates a derived {} but is rated {} — the solved \
 operating point exceeds the declared rating. fix: raise the rating to a real part \
 class, change the law parameters, or lower the boundary drive.",
 law.instance,
 inst.type_name,
 crate::analysis::electronics::format_watts_pub(power),
 crate::analysis::electronics::format_watts_pub(rated)
 )),
 Some(rated) => check.proved.push(format!(
 "{state}{} P({}) = {} <= {} rating — component-law DC",
 proof,
 law.instance,
 crate::analysis::electronics::format_watts_pub(power),
 crate::analysis::electronics::format_watts_pub(rated)
 )),
 }
 }
}

/// The effective absolute-maximum current envelope for one pin — the
/// instance's derating, then the type's pin-qualified row, then the
/// type's uniform. Amp quantities only.
fn max_current_for(inst: &ComponentInstance, ti: &TypeInfo, pname: &str) -> Option<f64> {
 inst.specs.get("max_current").and_then(quantity_amp)
 .or_else(|| ti.spec_defaults.get(&format!("max_current:{pname}")).and_then(quantity_amp))
 .or_else(|| ti.spec_defaults.get("max_current").and_then(quantity_amp))
}

/// The absolute-maximum current envelope — unconditional, like volts over
/// Tolerance. Law-exact current first, the series fixpoint as fallback.
fn check_max_current(
 instances: &BTreeMap<String, &ComponentInstance>,
 type_info: &BTreeMap<String, TypeInfo>,
 pin_to_net: &BTreeMap<(String, String), String>,
 check: &mut VoltageCheck,
 unpop: &HashSet<String>,
) {
 let mut rows: Vec<(String, String, f64)> = Vec::new();
 for inst in instances.values() {
 if unpop.contains(&inst.name) {
 continue;
 }
 let Some(ti) = type_info.get(&inst.type_name) else {
 continue;
 };
 for (pname, _) in &ti.pins {
 if let Some(limit) = max_current_for(inst, ti, pname) {
 rows.push((inst.name.clone(), pname.clone(), limit));
 }
 }
 }
 for (name, pname, limit) in rows {
 let key = (name.clone(), pname.clone());
 let derived = check.pin_current.get(&key).copied().or_else(|| {
 pin_to_net
 .get(&key)
 .and_then(|n| check.net_current.get(n).copied())
 });
 let Some(derived) = derived else {
 continue;
 };
 if derived.abs() > limit + f64::EPSILON {
 check.violations.push(format!(
 "pin '{}.{}' carries a derived current of {} but its absolute maximum \
 current is {} — the rating is violated. fix: lower the boundary voltage \
 or the series resistance, or fit a part rated for the current",
 name,
 pname,
 format_amps(derived.abs()),
 format_amps(limit)
 ));
 } else {
 check.proved.push(format!(
 "I({}.{}) = {} <= {} — absolute maximum",
 name,
 pname,
 format_amps(derived.abs()),
 format_amps(limit)
 ));
 }
 }
}

/// The effective drive-current capability for one pin — the same envelope
/// ladder as `max_current_for`.
fn drive_current_for(inst: &ComponentInstance, ti: &TypeInfo, pname: &str) -> Option<f64> {
 inst.specs.get("drive_current").and_then(quantity_amp)
 .or_else(|| ti.spec_defaults.get(&format!("drive_current:{pname}")).and_then(quantity_amp))
 .or_else(|| ti.spec_defaults.get("drive_current").and_then(quantity_amp))
}

/// The effective fan-in bound for one pin — the same envelope ladder. A
/// dimensionless count (Int property); anything else is absent.
fn fan_in_for(inst: &ComponentInstance, ti: &TypeInfo, pname: &str) -> Option<u64> {
 let pick = |pv: Option<&PropertyValue>| match pv {
 Some(PropertyValue::Int(n)) if *n >= 0 => Some(*n as u64),
 _ => None,
 };
 pick(inst.specs.get("fan_in"))
 .or_else(|| ti.spec_defaults.get(&format!("fan_in:{pname}")).and_then(|v| pick(Some(v))))
 .or_else(|| ti.spec_defaults.get("fan_in").and_then(|v| pick(Some(v))))
}

/// Whether one pin's class may hold a drive (`spec CanDrive: true`).
fn pin_can_drive(ti: &TypeInfo, pname: &str) -> bool {
 ti.pins
 .iter()
 .position(|(n, _)| n == pname)
 .and_then(|i| ti.pin_classes.get(i))
 .map_or(false, |c| c.can_drive)
}

/// The driven-node check (E8): a net with exactly one `can_drive` pin has
/// a DRIVER; `spec DriveCurrent` caps the net's derived draw and `spec
/// FanIn` caps the load-branch count. Unpop drivers are exempt.
fn check_drive_capability(
 input: &ProofInputs<'_>,
 pin_to_net: &BTreeMap<(String, String), String>,
 check: &mut VoltageCheck,
) {
 for net in input.nets {
 let Some((drv, inst, ti)) = net_driver(input, net) else {
 continue;
 };
 let load_count = net.pins.len().saturating_sub(1);
 if let Some(max_fan) = fan_in_for(inst, ti, &drv.pin) {
 fan_in_verdict(drv, max_fan, load_count, input.states, check);
 }
 let Some(limit) = drive_current_for(inst, ti, &drv.pin) else {
 continue;
 };
 let Some(draw) = driven_net_draw(input, net, pin_to_net, check) else {
 continue;
 };
 drive_current_verdict(drv, limit, draw, input.states, check);
 }
}

/// The lone drive-capable pin of a net with its instance and type info —
/// the net's DRIVER. Zero drive pins and two or more (ERC contention,
/// refused upstream) both mean "no check". An unpop driver is no driver.
fn net_driver<'a>(
 input: &ProofInputs<'a>,
 net: &'a Net,
) -> Option<(&'a PinRef, &'a ComponentInstance, &'a TypeInfo)> {
 let drivers: Vec<&PinRef> = net
 .pins
 .iter()
 .filter(|p| {
 input
 .tables
 .instances
 .get(&p.component)
 .and_then(|inst| input.tables.type_info.get(&inst.type_name))
 .map_or(false, |ti| pin_can_drive(ti, &p.pin))
 })
 .collect();
 if drivers.len() != 1 {
 return None;
 }
 let drv = drivers[0];
 if input.unpop.contains(&drv.component) {
 return None;
 }
 let inst = input.tables.instances.get(&drv.component)?;
 let ti = input.tables.type_info.get(&inst.type_name)?;
 Some((drv, inst, ti))
}

/// The net's derived draw with its provenance — component-law DC first,
/// the B4 series fixpoint as fallback.
fn driven_net_draw(
 input: &ProofInputs<'_>,
 net: &Net,
 pin_to_net: &BTreeMap<(String, String), String>,
 check: &mut VoltageCheck,
) -> Option<(f64, &'static str)> {
 let law_draw = law_net_draw(net, input.laws, check);
 if let Some(draw) = law_draw {
 return Some((draw, "component-law DC"));
 }
 let parts = collect_series_parts(input.tables.instances, input.tables.type_info, pin_to_net);
 net_draw(&net.name, &parts, &check.net_voltage).map(|draw| (draw, "B4 fixpoint"))
}

/// Fan-in verdict: the load-branch count against the declared maximum.
fn fan_in_verdict(
 drv: &PinRef,
 max_fan: u64,
 load_count: usize,
 states: &std::collections::BTreeSet<String>,
 check: &mut VoltageCheck,
) {
 if load_count > max_fan as usize {
 check.violations.push(format!(
 "{}fan-in exceeded: '{}.{}' drives {} loads on one net but its FanIn \
 declares {} — the part cannot see that many branches. fix: declare a \
 higher FanIn, split the net, or buffer it",
 state_prefix(states),
 drv.component,
 drv.pin,
 load_count,
 max_fan
 ));
 } else {
 check.proved.push(format!(
 "fan-in {} <= {} at {}.{} — declared drive fan-in",
 load_count, max_fan, drv.component, drv.pin
 ));
 }
}

/// Drive-current verdict: the net's derived draw against the part's
/// declared sourcing capability.
fn drive_current_verdict(
 drv: &PinRef,
 limit: f64,
 draw: (f64, &'static str),
 states: &std::collections::BTreeSet<String>,
 check: &mut VoltageCheck,
) {
 let (drawn, provenance) = draw;
 if drawn > limit + f64::EPSILON {
 check.violations.push(format!(
 "{}drive capability exceeded: '{}.{}' is rated to source {} but its net \
 draws {} — the derived sum over the driven node ({provenance}) is past the \
 part's DriveCurrent. fix: a stronger driver, lighter loads, or series \
 resistance",
 state_prefix(states),
 drv.component,
 drv.pin,
 format_amps(limit),
 format_amps(drawn)
 ));
 } else {
 check.proved.push(format!(
 "I({}.{} net) = {} <= {} — drive capability ({provenance})",
 drv.component,
 drv.pin,
 format_amps(drawn),
 format_amps(limit)
 ));
 }
}

/// E7: total derived current crossing net N — the KCL boundary sum over
/// every series part attached to N, direction-agnostic.
fn net_draw(net: &str, parts: &[SeriesPart], net_voltage: &BTreeMap<String, f64>) -> Option<f64> {
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

/// Sum |I| over law-bearing pins attached to one net.
fn law_net_draw(
 net: &Net,
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 voltage: &VoltageCheck,
) -> Option<f64> {
 let law_components: HashSet<&str> = laws.iter().map(|law| law.instance.as_str()).collect();
 let mut total = 0.0;
 let mut any = false;
 for pin in &net.pins {
 if !law_components.contains(pin.component.as_str()) {
 continue;
 }
 let Some(current) = voltage
 .pin_current
 .get(&(pin.component.clone(), pin.pin.clone()))
 else {
 continue;
 };
 total += current.abs();
 any = true;
 }
 any.then_some(total)
}

/// A current-bound obligation from a postcondition: `[pin.current <= B]`
/// (upper) / `[pin.current >= B]` (lower), carrying the node precondition
/// for mode-state filtering.
struct CurrentBound {
 transaction: String,
 precondition: HirExpr,
 pin: PinRef,
 bound: f64,
 is_upper: bool,
}

/// Collect current-bound obligations from POSTconditions of all nodes.
fn collect_current_bounds(
 program: &HirProgram,
 tables: &Tables<'_>,
) -> Vec<CurrentBound> {
 let mut out = Vec::new();
 for callable in all_transactions(program) {
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
 let Some(pin) = resolve_current_pin(pin_expr, tables.instances, tables.type_pins) else {
 continue;
 };
 out.push(CurrentBound {
 transaction: callable.name.clone(),
 precondition: contract.pre.clone(),
 pin,
 bound: lit,
 is_upper: matches!(op, BinaryOpKind::Le | BinaryOpKind::Lt),
 });
 }
 }
 out
}

/// Collect comparison operand triples (`op` ∈ Le/Lt/Ge/Gt) from an
/// expression tree — the postcondition physics bounds.
fn collect_compare_pairs<'a>(
 expr: &'a HirExpr,
 out: &mut Vec<(&'a HirExpr, BinaryOpKind, &'a HirExpr)>,
) {
 if let HirExprKind::Binary { op, left, right } = &expr.kind {
 match op {
 BinaryOpKind::Le | BinaryOpKind::Lt | BinaryOpKind::Ge | BinaryOpKind::Gt => {
 out.push((left.as_ref(), *op, right.as_ref()))
 }
 BinaryOpKind::And | BinaryOpKind::Or => {
 collect_compare_pairs(left, out);
 collect_compare_pairs(right, out);
 }
 _ => {}
 }
 }
}

/// Shared tables for current-bound proofs.
struct CurrentBoundTables<'a> {
 pin_to_net: &'a BTreeMap<(String, String), String>,
 states: &'a std::collections::BTreeSet<String>,
 modes: &'a BTreeMap<String, Vec<String>>,
 assigned: &'a BTreeMap<String, String>,
 unpop: &'a HashSet<String>,
}

/// Prove (or violate) the postcondition current bounds against the
/// derived current classes.
fn check_current_bounds(
 input: &ProofInputs<'_>,
 pin_to_net: &BTreeMap<(String, String), String>,
 check: &mut VoltageCheck,
) {
 let tables = CurrentBoundTables {
 pin_to_net,
 states: input.states,
 modes: input.modes,
 assigned: input.assigned_modes,
 unpop: input.unpop,
 };
 for bound in collect_current_bounds(input.program, input.tables) {
 let key = (bound.pin.component.clone(), bound.pin.pin.clone());
 let Some(net_name) = tables.pin_to_net.get(&key) else {
 continue;
 };
 let law_current = check.pin_current.get(&key).copied();
 let Some(derived) = law_current.or_else(|| check.net_current.get(net_name).copied()) else {
 record_bound_vacuity(&bound, &tables, input.tables, check);
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
 match mode_predicate_holds(&bound.precondition, &bound.transaction, tables.modes, tables.assigned) {
 Ok(true) => record_current_bound(site, check),
 Ok(false) => {}
 Err(error) => check.violations.push(error),
 }
 }
}

/// The anti-vacuity rule: a bound the solver cannot attempt is vacuously
/// proven — never silent. In an absent-participation state an unpop
/// part's pins carry no copper, so the bound is not evidence there.
fn record_bound_vacuity(
 bound: &CurrentBound,
 tables: &CurrentBoundTables<'_>,
 full: &Tables<'_>,
 check: &mut VoltageCheck,
) {
 let absent_state = tables.states.contains("participation=absent")
 && tables.unpop.contains(&bound.pin.component);
 if absent_state {
 return;
 }
 let inst = full.instances.get(&bound.pin.component);
 let type_name = inst.map(|c| c.type_name.as_str()).unwrap_or("?");
 check.violations.push(format!(
 "pin '{}.{}' (of {}) has a current bound (from '{}') but no derivable current — the component declares no law physics that solves it, so the bound would be vacuously proven. fix: declare the component's laws (`when` + spec parameters), or remove the bound",
 bound.pin.component, bound.pin.pin, type_name, bound.transaction
 ));
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

/// E7 (D4): source-pin budgets — `budget u1.out <= 250mA;` caps the
/// derived current draw across the pin's net. Violations are hard
/// budget_errors; nets with no derived draw pass vacuously.
fn check_budgets(
 program: &HirProgram,
 tables: &Tables<'_>,
 nets: &[Net],
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 states: &std::collections::BTreeSet<String>,
 voltage: &VoltageCheck,
) -> Vec<String> {
 let mut errors = Vec::new();
 if program.budgets.is_empty() {
 return errors;
 }
 let mut pin_to_net: BTreeMap<(String, String), String> = BTreeMap::new();
 for net in nets {
 for p in &net.pins {
 pin_to_net.insert((p.component.clone(), p.pin.clone()), net.name.clone());
 }
 }
 let parts = collect_series_parts(tables.instances, tables.type_info, &pin_to_net);
 for budget in &program.budgets {
 check_one_budget(budget, tables, nets, laws, states, voltage, &parts, &mut errors);
 }
 errors
}

/// One budget statement: classify the comparison shape, resolve the pin
/// and the current limit, compare against the derived draw.
#[allow(clippy::too_many_arguments)]
fn check_one_budget(
 budget: &super::program::HirBudget,
 tables: &Tables<'_>,
 nets: &[Net],
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 states: &std::collections::BTreeSet<String>,
 voltage: &VoltageCheck,
 parts: &[SeriesPart],
 errors: &mut Vec<String>,
) {
 let HirExprKind::Binary { op, left, right } = &budget.contract.kind else {
 errors.push("a budget must state `budget <instance>.<pin> <= <current>;`".to_string());
 return;
 };
 let (pin_expr, limit_expr) = match op {
 BinaryOpKind::Le => (left.as_ref(), right.as_ref()),
 BinaryOpKind::Ge => (right.as_ref(), left.as_ref()),
 _ => {
 errors.push(
 "a budget compares a pin with a current limit using `<=` (or `>=`, reversed)"
 .to_string(),
 );
 return;
 }
 };
 let Some(pin) = resolve_pin(pin_expr, tables.instances, tables.type_pins) else {
 errors.push(
 "a budget targets a pin — state `budget <instance>.<pin> <= <current>;`".to_string(),
 );
 return;
 };
 let Some(limit) = extract_current(limit_expr) else {
 errors.push(
 "a budget limit is a current — state it with a unit (`250mA`, `1.5A`) or as a bare ampere number"
 .to_string(),
 );
 return;
 };
 let key = pin_key(&pin.component, &pin.pin);
 let Some(net) = nets
 .iter()
 .find(|n| n.pins.iter().any(|p| pin_key(&p.component, &p.pin) == key))
 else {
 return;
 };
 let law_draw = law_net_draw(net, laws, voltage);
 let Some((drawn, provenance)) = law_draw
 .map(|draw| (draw, "component-law DC"))
 .or_else(|| net_draw(&net.name, parts, &voltage.net_voltage).map(|draw| (draw, "B4 fixpoint")))
 else {
 return;
 };
 if drawn > limit {
 errors.push(format!(
 "{}budget exceeded: '{}.{}' allows {} but its net draws {} — the derived sum over \
 the net ({provenance}) is past the stated limit. Raise the budget or reduce the draw",
 state_prefix(states),
 pin.component,
 pin.pin,
 format_amps(limit),
 format_amps(drawn)
 ));
 }
}

/// 2026-09-26 (E15/E14b-8 parity): law-born rails are FIXED boundaries in
/// every law group's solve — merged into the DC drive map.
fn merge_law_rail_boundaries(
 nets: &[Net],
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 tables: &Tables<'_>,
 mut base: BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
 for net in nets {
 for p in &net.pins {
 if let Some(volts) = law_birth_volts(laws, p, tables) {
 base.entry(net.name.clone()).or_insert(volts);
 }
 }
 }
 base
}

/// The law-born rail volts on ONE pin, ds-free: the pin's component law
/// has an always-guarded equation pinning this supply-class pin to a
/// constant.
fn law_birth_volts(
 laws: &[crate::analysis::electronics_laws::ComponentLaws],
 pin: &PinRef,
 tables: &Tables<'_>,
) -> Option<f64> {
 let inst = tables.instances.get(&pin.component)?;
 let ti = tables.type_info.get(&inst.type_name)?;
 let idx = ti.pins.iter().position(|(n, _)| n == &pin.pin)?;
 if !ti.pin_classes.get(idx).map_or(false, |c| c.supply) {
 return None;
 }
 laws.iter()
 .find(|l| l.instance == pin.component)?
 .laws
 .iter()
 .filter(|law| law.guard == crate::analysis::electronics_laws::LawGuard::Always)
 .flat_map(|law| law.equations.iter())
 .filter_map(|eq| crate::analysis::electronics::constant_voltage_pin(&eq.expression))
 .find(|(p, _)| p.component == pin.component && p.pin == pin.pin)
 .map(|(_, volts)| volts)
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
 // 2026-11-12 (full-surface port): the elaborated law IR is now part of
 // the parity surface — deterministic Debug dump in instance order.
 out.insert(
 "component_laws".into(),
 nl.component_laws
 .iter()
 .map(|c| format!("{c:?}"))
 .collect::<Vec<_>>()
 .join("\n"),
 );
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
 //! AST-vs-HIR parity for the full electronics surface. Every case
 //! asserts exact canonical equality of the covered result surface
 //! (including the elaborated law IR and author net labels).
 //!
 //! Construct families exercised: pins/units/classes, instances (literal
 //! and table-indexed), plain/bus/wildcard equalities, participation,
 //! decoupling, contention, voltage drives + tolerances, hold laws and
 //! mutually exclusive guards, net<>/stdnet<> membership and labels,
 //! node-body intents (wires, drive intents, opens), voltage/current
 //! obligations with pull-up/low-hold/series forcing and bus assembly,
 //! mechanism synthesis (`when … { … } through Type`), E-series series
 //! synthesis, constitutive laws + DC operating points, named modes,
 //! current/power proofs, budgets.

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
 fn corpus_parity_for_ebv_fixtures() {
 let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
 let mut files = Vec::new();
 for rel in ["tests/electronics", "tests/fixtures/electronics/parity", "examples/electronics", "examples/silicon"] {
 let dir = root.join(rel);
 let Ok(entries) = std::fs::read_dir(&dir) else { continue };
 for entry in entries.flatten() {
 let path = entry.path();
 if path.extension().is_some_and(|e| e == "ebv" || e == "bv") {
 files.push(path);
 }
 }
 }
 files.sort();
 assert!(!files.is_empty(), "no electronics fixtures found");
 let mut diffs = Vec::new();
 for path in &files {
 let src = std::fs::read_to_string(path).expect("read fixture");
 let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
 let (items, program) = parse_lower(&src);
 let ast = canonical_dump(&crate::analysis::electronics::derive_netlist(&items));
 let hir = canonical_dump(&derive_netlist(&program));
 if ast != hir {
 diffs.push(name);
 }
 }
 assert!(diffs.is_empty(), "HIR/AST electronics drift on fixtures: {diffs:?}");
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

 // ── Data tables (2026-09-27 E2) ─────────────────────────────────────

 #[test]
 fn data_tables_resolve_indexed_fields() {
 let src = r#"
 data w: Int[2][2] = [ [ 10, 22 ], [ 330, 47 ] ];
 type Resistor { pin a; pin b; reference "R"; };
 let r1: Resistor = Resistor { value: w[1][0]; };
 let r2: Resistor = Resistor { value: w[0][1]; };
 "#;
 let dump = parity(src);
 assert!(dump["components"].contains("r1=>Resistor[value=330]"), "{}", dump["components"]);
 assert!(dump["components"].contains("r2=>Resistor[value=22]"), "{}", dump["components"]);
 assert!(dump["intent_errors"].is_empty());
 }

 #[test]
 fn data_table_errors_match() {
 let no_index = r#"
 data w: Int[2] = [ 1, 2 ];
 type R { pin a; pin b; reference "R"; };
 let r1: R = R { value: w; };
 "#;
 let dump = parity(no_index);
 assert!(dump["intent_errors"].contains("without an index"), "{}", dump["intent_errors"]);

 let out_of_range = r#"
 data w: Int[2] = [ 1, 2 ];
 type R { pin a; pin b; reference "R"; };
 let r1: R = R { value: w[5]; };
 "#;
 let dump = parity(out_of_range);
 assert!(dump["intent_errors"].contains("out of range"), "{}", dump["intent_errors"]);

 let missing = r#"
 type R { pin a; pin b; reference "R"; };
 let r1: R = R { value: nope[0]; };
 "#;
 let dump = parity(missing);
 assert!(dump["intent_errors"].contains("not a declared data table"), "{}", dump["intent_errors"]);
 }

 // ── Bus ranges + wildcards (whole-bus equality) ─────────────────────

 #[test]
 fn bus_range_equality_matches() {
 let src = r#"
 type Chip { pin gpio[8]; reference "U"; };
 type Header { pin data[8]; reference "J"; };
 let u2: Chip = Chip { value: "u" };
 let u3: Header = Header { value: "j" };
 txn on [u2.gpio[0..=7].voltage == u3.data[0..=7].voltage] { }
 "#;
 let dump = parity(src);
 assert!(dump["bus_errors"].is_empty(), "{}", dump["bus_errors"]);
 assert_eq!(dump["nets"].lines().count(), 8, "{}", dump["nets"]);
 assert!(dump["dangling"].is_empty());
 }

 #[test]
 fn bus_range_mismatch_is_a_hard_error() {
 let src = r#"
 type Chip { pin gpio[8]; reference "U"; };
 type Header { pin data[4]; reference "J"; };
 let u2: Chip = Chip { value: "u" };
 let u3: Header = Header { value: "j" };
 txn on [u2.gpio[0..7].voltage == u3.data[0..3].voltage] { }
 "#;
 let dump = parity(src);
 assert!(dump["bus_errors"].contains("the buses must be the same length"), "{}", dump["bus_errors"]);
 }

 #[test]
 fn wildcard_pin_array_equality_matches() {
 let src = r#"
 type Chip { pin gpio[4]; reference "U"; };
 type Header { pin p[4]; reference "J"; };
 let u2: Chip = Chip { value: "u" };
 let j1: Header = Header { value: "j" };
 txn on [u2.gpio[*].voltage == j1.p[*].voltage] { }
 "#;
 let dump = parity(src);
 assert!(dump["bus_errors"].is_empty(), "{}", dump["bus_errors"]);
 assert_eq!(dump["nets"].lines().count(), 4, "{}", dump["nets"]);
 }

 
 #[test]
 fn wildcard_length_mismatch_is_a_hard_error() {
 let src = r#"
 type Chip { pin gpio[4]; reference "U"; };
 type Header { pin p[2]; reference "J"; };
 let u2: Chip = Chip { value: "u" };
 let j1: Header = Header { value: "j" };
 txn on [u2.gpio[*].voltage == j1.p[*].voltage] { }
 "#;
 let dump = parity(src);
 assert!(dump["bus_errors"].contains("the buses must be the same length"), "{}", dump["bus_errors"]);
 }

 // ── net<>/stdnet<> membership (E14b-7) ──────────────────────────────

 
 #[test]
 fn membership_ambiguity_is_a_hard_error() {
 let src = r#"
 type Power { spec KicadType: "power_in"; spec Supply: true; spec Tolerance: any; };
 type Ground { spec KicadType: "power_in"; spec Return: true; };
 type Reg { pin vin: Power; pin vout: Power; pin gnd: Ground; reference "U"; spec Tolerance: any; };
 type Load { pin vdd: Power; pin vss: Ground; reference "U"; spec Tolerance: any; };

 let u1: Reg = Reg { value: "reg" };
 let u2: Load = Load { value: "load" };
 let g1: Ground = Ground { value: "g" };

 txn on [g1.voltage == u1.gnd.voltage && u2.vss.voltage == u1.gnd.voltage]
 [u1.vin.voltage == 5V && u1.vout.voltage == 3.3V]
 { }
 "#;
 let dump = parity(src);
 assert!(
 dump["convention_errors"].contains("membership-ambiguous"),
 "{}",
 dump["convention_errors"]
 );
 }

 #[test]
 fn net_names_require_supply_pins() {
 let src = r#"
 type In { spec KicadType: "input"; };
 type Chip { pin io: In; reference "U"; };
 net<io: VBUS> let u1: Chip = Chip { value: "u" };
 "#;
 let dump = parity(src);
 assert!(
 dump["convention_errors"].contains("not a supply-class pin"),
 "{}",
 dump["convention_errors"]
 );
 }

 // ── Node-body intents + mechanisms (E14a/D16) ───────────────────────

 #[test]
 fn body_wire_facts_and_drive_intents_match() {
 let wire = r#"
 type A { pin p; pin n; reference "A"; };
 type B { pin q; reference "B"; };
 let a1: A = A { value: "a" };
 let b1: B = B { value: "b" };
 txn on { a1.p = b1.q; }
 "#;
 let dump = parity(wire);
 assert!(dump["intent_proofs"].contains("wired in node 'on': a1.p <-> b1.q"), "{}", dump["intent_proofs"]);
 assert_eq!(dump["nets"].lines().count(), 1);

 let intent = r#"
 type Mcu { pin p1: Io; reference "U"; spec Tolerance: any; };
 type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
 type Sensor { pin sda: Io; reference "S"; };
 let u1: Mcu = Mcu { value: "u" };
 let s1: Sensor = Sensor { value: "s" };
 txn on { s1 = true; }
 "#;
 let dump = parity(intent);
 assert!(dump["intent_proofs"].contains("wired by intent"), "{}", dump["intent_proofs"]);
 }

 #[test]
 fn open_disconnection_matches() {
 let src = r#"
 type Chip { pin a; pin b; reference "U"; };
 let u1: Chip = Chip { value: "1" };
 let u2: Chip = Chip { value: "2" };
 txn n { open u1.a, u2.a; }
 "#;
 let dump = parity(src);
 assert!(dump["intent_proofs"].contains("open (disconnected)"), "{}", dump["intent_proofs"]);
 }

 #[test]
 fn open_conflict_is_a_hard_error() {
 let src = r#"
 type Chip { pin a; pin b; reference "U"; };
 let u1: Chip = Chip { value: "1" };
 let u2: Chip = Chip { value: "2" };
 txn on [u1.a.voltage == u2.a.voltage] { open u1.a, u2.a; }
 "#;
 let dump = parity(src);
 assert!(
 dump["intent_errors"].contains("already connected (same net)"),
 "{}",
 dump["intent_errors"]
 );
 }

 
 
 
 
 #[test]
 fn current_obligation_without_physics_is_a_hard_error() {
 let src = r#"
 type Power { spec KicadType: "power_in"; spec Supply: true; };
 type Ground { spec KicadType: "power_in"; spec Return: true; };
 type Load { pin a; pin k; reference "L"; };
 let p1: Power = Power { value: "p" };
 let g1: Ground = Ground { value: "g" };
 let l1: Load = Load { value: "l" };
 txn lit [p1.voltage == l1.a.voltage && g1.voltage == l1.k.voltage]
 [p1.voltage == 5V]
 {
 l1.a.current >= 10mA;
 }
 "#;
 let dump = parity(src);
 assert!(
 dump["intent_errors"].contains("declares no ForwardVoltage physics"),
 "{}",
 dump["intent_errors"]
 );
 }

 // ── Laws + DC + modes + proofs + budgets ────────────────────────────

 
 
 #[test]
 fn undeclared_mode_is_a_hard_error() {
 let src = r#"
 type Resistor { pin a; pin b; reference "R"; mode open { a.current == 0Amp; } };
 let r1: Resistor = Resistor { value: "R"; };
 txn on [r1.conducting] { }
 "#;
 let dump = parity(src);
 assert!(
 dump["law_errors"].contains("names an undeclared mode"),
 "{}",
 dump["law_errors"]
 );
 }

 
 
 }
