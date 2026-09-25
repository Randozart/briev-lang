//! 2026-09-11 (Part C, Electronics Briev): KiCad schematic backend.
//!
//! Consumes the frontend-derived netlist (`analysis::electronics`) and emits
//! a KiCad 7 S-expression schematic (`.kicad_sch`, version 20230121). The
//! backend makes NO netlist decisions and NO type-name matches (Rule 15):
//! component symbols are generic boxes whose geometry is computed from the
//! type's pin list; reference prefixes come from `!>` metadata and values
//! from instance literal fields — configuration the source carries, never
//! compiler knowledge.
//!
//! Strictest surface in the family: a static netlist, no runtime, no codegen
//! beyond the schematic. Dangling pins (already diagnosed by the analysis)
//! fail the compile here — the backend never emits an incomplete board.

pub mod routing;

use std::collections::BTreeMap;
use crate::analysis::electronics::{ComponentInstance, ElectronicsNetlist};
use crate::backend::capabilities::BackendCapabilities;

pub const CAPABILITIES: BackendCapabilities = BackendCapabilities {
    name: "electronics",
    nature: "PCB schematic (KiCad S-expression)",
    // 2026-09-11: contract expressions are PROVEN at compile time, never
    // executed — so comparisons/arithmetic/float literals inside [pre][post]
    // obligations are part of the electronics surface. What electronics
    // forbids is executable surface: calls, runtime, concurrency.
    // 2026-09-23 (E14a gate): the declarative surface caught up —
    // `bool_char_literals` (proven Bool values; the `inst = true` intent),
    // `index` (pin-array/instance-array element access — E11/E1 analysis
    // surface, never executed), `assign_stmt` (node-body wiring/intent
    // FACTS consumed by netlist derivation), `guarded_stmt` (in-body `when`
    // mechanism conditions, D16). None of these lower to runnable code —
    // the backend emits from the derived netlist only.
    int_literals: true,
    floats: true,
    strings: true,
    bool_char_literals: true,
    int_ops: true,
    unary_ops: true,
    calls: false,
    intrinsics: false,
    if_expr: false,
    match_expr: false,
    block_expr: false,
    field_access: true,
    index: true,
    slices_ranges: false,
    tuple_list_literals: false,
    struct_literal: true,
    lambda: false,
    casts: false,
    is_type: false,
    deref_addr_of: false,
    spawn: false,
    obj_ports: false,
    cells: false,
    extern_cells: false,
    await_expr: false,
    method_calls: false,
    reflect: false,
    plugin_intercept: false,
    derivation_blocks: false,
    within: false,
    let_stmt: true,
    assign_stmt: true,
    arrow_assign: false,
    guarded_stmt: true,
    term_endprogram: false,
    break_stmt: false,
    trap_stmt: false,
    match_stmt: false,
    foreach: false,
    inline_asm: false,
    concurrency_sections: false,
    defer_stmt: false,
    lifetime_hints: false,
    metadata_assign: false,
    rollback: false,
    gate_stmt: false,
    trg_bindings: false,
    yield_stmt: false,
};

/// Symbol geometry (mm, KiCad grid): pin x offset from the instance origin.
const PIN_X: f64 = 5.08;
/// Vertical distance between pins on one side.
const PIN_PITCH: f64 = 2.54;
/// Instance placement: two columns so nets route horizontally between
/// them; even index left, odd right (deterministic by sorted name).
const PLACE_X_LEFT: f64 = 101.6;
const PLACE_X_RIGHT: f64 = 152.4;
const PLACE_Y0: f64 = 50.8;
const PLACE_PITCH: f64 = 25.4;

/// Format a schematic coordinate: round to the KiCad grid (0.01 mm), never
/// emitting float noise like 76.19999999999999.
fn coord(v: f64) -> String {
    format!("{:.2}", (v * 100.0).round() / 100.0)
}

/// Shared pin layout: symbol-local offsets for a pin list, left/right split
/// (extra pin on the left), stacked symmetrically around y=0. Used by BOTH
/// the symbol definition and the instance wire routing so the two can never
/// disagree.
fn pin_layout(pins: &[(String, u64)]) -> Vec<(f64, f64, String, u64)> {
    let n = pins.len();
    let left_count = (n + 1) / 2;
    let mut out = Vec::new();
    for (i, (name, number)) in pins.iter().enumerate() {
        let (x, side_idx, side_count) = if i < left_count {
            (-PIN_X, i as f64, left_count as f64)
        } else {
            (PIN_X, (i - left_count) as f64, (n - left_count) as f64)
        };
        let y = (side_idx - (side_count - 1.0) / 2.0) * PIN_PITCH;
        out.push((x, y, name.clone(), *number));
    }
    out
}

/// Everything emit_instance renders, bundled to keep the parameter list flat.
struct Placement<'a> {
    comp: &'a ComponentInstance,
    x: f64,
    y: f64,
    reference: &'a str,
    value: &'a str,
    footprint: &'a str,
    /// 2026-09-22 (Slice B): an `unpop` part is excluded from the BOM
    /// (`in_bom no`) but its pads remain on the board.
    unpop: bool,
}

pub struct ElectronicsBackend;

impl ElectronicsBackend {
    /// Emit the `.kicad_sch` text. Fails on dangling pins AND on electrical
    /// violations — an incomplete or electrically-unsound board never leaves
    /// the compiler.
    pub fn generate(netlist: &ElectronicsNetlist) -> Result<String, Vec<String>> {
        // 2026-09-22 (Slice B): participation warnings are NOT errors — a
        // populated short or an undeclared participation name is surfaced
        // as a note; the board still emits.
        for w in &netlist.participation_warnings {
            eprintln!("note: {}", w);
        }
        // 2026-09-21 (E14a): drive-intent failures — an intent that could
        // not complete, or completed ambiguously. Refuse FIRST: intent
        // diagnostics outrank downstream diagnostics on an incomplete
        // board (the intent is what was supposed to complete it).
        if !netlist.intent_errors.is_empty() {
            let mut errs = vec![
                "cannot emit schematic: a drive intent could not be completed".to_string(),
            ];
            errs.extend(netlist.intent_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        if !netlist.dangling.is_empty() {
            let mut errs = vec!["cannot emit schematic: the netlist is incomplete".to_string()];
            errs.extend(netlist.dangling.iter().map(|d| format!("  {}", d)));
            return Err(errs);
        }
        // 2026-09-21 (E12): unresolved pin-class ascriptions — the class
        // type is not in scope. Refuse before anything else emits.
        if !netlist.class_errors.is_empty() {
            let mut errs = vec![
                "cannot emit schematic: a pin names a class type that is not in scope".to_string(),
            ];
            errs.extend(netlist.class_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-21 (E13): decoupling-convention violations — an instance
        // of a `spec Decouple` type without a bridging `spec Decoupler`
        // part on its rail. Refuse before anything else emits.
        if !netlist.convention_errors.is_empty() {
            let mut errs = vec![
                "cannot emit schematic: the decoupling convention is violated".to_string(),
            ];
            errs.extend(netlist.convention_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-21 (E7): source-pin budgets — a net's derived draw past
        // its stated budget. Refuse before anything else emits.
        if !netlist.budget_errors.is_empty() {
            let mut errs =
                vec!["cannot emit schematic: a source-pin budget is exceeded".to_string()];
            errs.extend(netlist.budget_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-22 (ERC contention): two drive-capable non-open-drain pins
        // on one net is a short — refused like any other electrical violation.
        if !netlist.contention_errors.is_empty() {
            let mut errs =
                vec!["cannot emit schematic: a net is driven by contending pins".to_string()];
            errs.extend(netlist.contention_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-24 (component laws): an invalid constitutive law cannot
        // prove physics — refuse before any schematic is emitted.
        if !netlist.law_errors.is_empty() {
            let mut errs =
                vec!["cannot emit schematic: a component law is invalid".to_string()];
            errs.extend(netlist.law_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-22 (whole-bus equality): a mismatched bus length is a hard
        // error — the buses must agree element-wise.
        if !netlist.bus_errors.is_empty() {
            let mut errs =
                vec!["cannot emit schematic: a whole-bus equality is malformed".to_string()];
            errs.extend(netlist.bus_errors.iter().map(|e| format!("  {}", e)));
            return Err(errs);
        }
        // 2026-09-11 (B4): voltage/current proving — shorted supplies,
        // over-voltage into rated pins, undeclared unrated pins, and
        // postcondition current bounds violated by derived physics.
        if !netlist.voltage.violations.is_empty() {
            let mut errs = vec![
                "cannot emit schematic: the electrical contracts are violated".to_string(),
            ];
            errs.extend(netlist.voltage.violations.iter().map(|v| format!("  {}", v)));
            return Err(errs);
        }

        let mut components = netlist.components.clone();
        components.sort_by(|a, b| a.name.cmp(&b.name));

        let mut out = String::new();
        Self::emit_header(&mut out);
        Self::emit_symbol_library(netlist, &components, &mut out);
        let pin_xy = Self::emit_instances(netlist, &components, &mut out);
        Self::emit_all_nets(netlist, &pin_xy, &mut out);
        out.push_str(")\n");
        Ok(out)
    }

    /// lib_symbols: one generic symbol per distinct component TYPE, in
    /// first-appearance order of the sorted instance list (deterministic).
    fn emit_symbol_library(netlist: &ElectronicsNetlist, components: &[ComponentInstance], out: &mut String) {
        out.push_str("  (lib_symbols\n");
        let mut lib_done: Vec<&str> = Vec::new();
        for comp in components {
            if lib_done.contains(&comp.type_name.as_str()) {
                continue;
            }
            let info = netlist
                .type_info
                .get(&comp.type_name)
                .expect("analysis guarantees TypeInfo for every component type");
            Self::emit_symbol_def(
                out,
                &comp.type_name,
                &info.reference_prefix,
                &info.pins,
                &info.pin_classes,
            );
            lib_done.push(&comp.type_name);
        }
        out.push_str("  )\n\n");
    }

    /// Emit instances on the two placement columns; returns the global pin
    /// coordinates for routing. Reference designators run PER PREFIX
    /// across the whole sheet (U1, U2… R1, R2… J1, J2…) — two types
    /// sharing `reference "U"` (Ldo, Mcu, Sensor) must number U1, U2, U3,
    /// never three U1s: duplicate references are invalid KiCad.
    fn emit_instances(
        netlist: &ElectronicsNetlist,
        components: &[ComponentInstance],
        out: &mut String,
    ) -> Vec<(String, String, f64, f64)> {
        let mut pin_xy = Vec::new();
        let refs = Self::reference_map(netlist, components);
        for (idx, comp) in components.iter().enumerate() {
            let (x, y) = (
                if idx % 2 == 0 { PLACE_X_LEFT } else { PLACE_X_RIGHT },
                PLACE_Y0 + idx as f64 * PLACE_PITCH,
            );
            let reference = refs.get(&comp.name).cloned().unwrap_or_default();
            let value = Self::property_of(comp, "value").unwrap_or_else(|| comp.type_name.clone());
            let footprint = Self::property_of(comp, "package").unwrap_or_default();
            let placement = Placement {
                comp,
                x,
                y,
                reference: &reference,
                value: &value,
                footprint: &footprint,
                unpop: netlist.unpop.contains(&comp.name),
            };
            Self::emit_instance(out, &placement);
            for (x_off, y_off, pin_name, _) in Self::pin_offsets(netlist, &comp.type_name) {
                pin_xy.push((comp.name.clone(), pin_name, x + x_off, y + y_off));
            }
        }
        out.push('\n');
        pin_xy
    }

    /// Per-prefix reference designators in DECLARATION order (U1, U2…,
    /// R1, R2…, J1, J2…) — shared by the schematic and the board so a
    /// part's reference is the same in both.
    fn reference_map(
        netlist: &ElectronicsNetlist,
        components: &[ComponentInstance],
    ) -> std::collections::BTreeMap<String, String> {
        let mut prefix_counters: std::collections::BTreeMap<String, usize> = Default::default();
        let mut refs = std::collections::BTreeMap::new();
        for comp in components {
            let prefix = Self::prefix_of(netlist, &comp.type_name);
            let counter = prefix_counters.entry(prefix.clone()).or_insert(0);
            *counter += 1;
            refs.insert(comp.name.clone(), format!("{}{}", prefix, *counter));
        }
        refs
    }

    /// Wires + labels: each net is a chain of its member pins in placement
    /// order — L-shaped hops through the inter-column channel.
    fn emit_all_nets(
        netlist: &ElectronicsNetlist,
        pin_xy: &[(String, String, f64, f64)],
        out: &mut String,
    ) {
        for net in &netlist.nets {
            let mut pts: Vec<(f64, f64)> = Vec::new();
            for p in &net.pins {
                if let Some(&(_, _, x, y)) =
                    pin_xy.iter().find(|(c, pn, _, _)| c == &p.component && pn == &p.pin)
                {
                    pts.push((x, y));
                }
            }
            let label = Self::net_label(netlist, net);
            Self::emit_net(out, net, &pts, &label);
        }
    }

    /// The readable schematic label for a net — derived from WHAT the net
    /// is, never an author name (2026-09-22): a net touching a return-class
    /// pin is `GND`; a net with a derived drive voltage is `V{volts}` (e.g.
    /// `V3.3`); anything else keeps its structural `N#` identity. The
    /// physics (pin classes + derived voltage) is the source of truth.
    fn net_label(netlist: &ElectronicsNetlist, net: &crate::analysis::electronics::Net) -> String {
        let inst_to_type: std::collections::BTreeMap<&str, &str> = netlist
            .components
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str()))
            .collect();
        for p in &net.pins {
            let return_class = inst_to_type
                .get(p.component.as_str())
                .and_then(|ty| netlist.type_info.get(*ty))
                .map(|ti| {
                    ti.pins
                        .iter()
                        .enumerate()
                        .find(|(_, (pn, _))| *pn == p.pin)
                        .map(|(i, _)| ti.pin_classes[i].return_pin)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if return_class {
                return "GND".to_string();
            }
        }
        if let Some(v) = netlist.voltage.net_voltage.get(&net.name) {
            if v.abs() > f64::EPSILON {
                let volts = format!("{:.1}", v).trim_end_matches(".0").to_string();
                return format!("V{}", volts);
            }
        }
        net.name.clone()
    }

    // ── geometry ──────────────────────────────────────────────────────

    /// Pin offsets relative to the instance origin, in symbol-local coords,
    /// via the shared layout.
    fn pin_offsets(netlist: &ElectronicsNetlist, type_name: &str) -> Vec<(f64, f64, String, u64)> {
        let info = netlist
            .type_info
            .get(type_name)
            .expect("analysis guarantees TypeInfo for every component type");
        pin_layout(&info.pins)
    }

    // ── helpers ───────────────────────────────────────────────────────

    fn prefix_of(netlist: &ElectronicsNetlist, type_name: &str) -> String {
        netlist
            .type_info
            .get(type_name)
            .map(|t| t.reference_prefix.clone())
            .unwrap_or_else(|| "U".to_string())
    }

    fn property_of(comp: &ComponentInstance, key: &str) -> Option<String> {
        comp.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn uuid(tag: &str) -> String {
        // Deterministic UUID-shaped identifiers — schematics diff cleanly.
        let mut h: u64 = 1_469_598_103_934_665_603;
        for b in tag.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(1_099_511_628_211);
        }
        format!("b713e5c0-0000-4000-8000-{:012}", h & 0xFFFF_FFFF_FFFF)
    }

    // ── emission ──────────────────────────────────────────────────────

    fn emit_header(out: &mut String) {
        out.push_str("(kicad_sch\n");
        out.push_str("  (version 20230121)\n");
        out.push_str("  (generator briev)\n");
        out.push_str(&format!("  (uuid \"{}\")\n", Self::uuid("sheet")));
        out.push_str("  (paper \"A4\")\n");
    }

    fn emit_symbol_def(
        out: &mut String,
        name: &str,
        reference: &str,
        pins: &[(String, u64)],
        classes: &[crate::analysis::electronics::PinClassProps],
    ) {
        out.push_str(&format!("    (symbol \"{}\" (in_bom yes) (on_board yes)\n", name));
        out.push_str(&format!(
            "      (property \"Reference\" \"{}\" (at 0 5.08 0) (effects (font (size 1.27 1.27))))\n",
            reference
        ));
        out.push_str(&format!(
            "      (property \"Value\" \"{}\" (at 0 -5.08 0) (effects (font (size 1.27 1.27))))\n",
            name
        ));
        // Body: generic rectangle sized to the pin count.
        let half_h = (pins.len() as f64).max(2.0) * PIN_PITCH / 2.0;
        out.push_str(&format!("      (symbol \"{}_0_1\"\n", name));
        out.push_str(&format!(
            "        (rectangle (start -2.54 {}) (end 2.54 {}) (stroke (width 0.254) (type default)) (fill (type background)))\n",
            half_h, -half_h
        ));
        out.push_str("      )\n");
        out.push_str(&format!("      (symbol \"{}_1_1\"\n", name));
        // 2026-09-21 (E12): the KiCad electrical pin type comes from the
        // pin's class fundamental (`spec KicadType`), resolved in analysis.
        // Unclassed pins default to `passive` — pre-E12 output unchanged.
        for ((x, y, pname, number), cls) in
            pin_layout(pins).into_iter().zip(classes.iter())
        {
            let angle = if x < 0.0 { 0 } else { 180 };
            out.push_str(&format!(
                "        (pin {} line (at {} {} {}) (length 2.54) (name \"{}\" (effects (font (size 1.27 1.27)))) (number \"{}\" (effects (font (size 1.27 1.27)))))\n",
                cls.kicad_type, x, y, angle, pname, number
            ));
        }
        out.push_str("      )\n");
        out.push_str("    )\n");
    }

    fn emit_instance(out: &mut String, p: &Placement) {
        let x = p.x;
        let y = p.y;
        out.push_str(&format!(
            "  (symbol (lib_id \"{}\") (at {} {} 0) (unit 1)\n",
            p.comp.type_name, coord(x), coord(y)
        ));
        // 2026-09-22 (Slice B): an `unpop` part keeps its pads on the board
        // but is excluded from the BOM.
        if p.unpop {
            out.push_str("    (in_bom no) (on_board yes)\n");
        } else {
            out.push_str("    (in_bom yes) (on_board yes)\n");
        }
        out.push_str(&format!("    (uuid \"{}\")\n", Self::uuid(&format!("inst:{}", p.comp.name))));
        out.push_str(&format!(
            "    (property \"Reference\" \"{}\" (at {} {} 0) (effects (font (size 1.27 1.27))))\n",
            p.reference,
            coord(x - 7.62),
            coord(y - 3.81)
        ));
        out.push_str(&format!(
            "    (property \"Value\" \"{}\" (at {} {} 0) (effects (font (size 1.27 1.27))))\n",
            p.value,
            coord(x - 7.62),
            coord(y + 3.81)
        ));
        out.push_str(&format!(
            "    (property \"Footprint\" \"{}\" (at {} {} 0) (effects (font (size 1.27 1.27)) (hide yes)))\n",
            p.footprint, coord(x), coord(y)
        ));
        out.push_str("    (instances (project briev\n");
        out.push_str(&format!(
            "      (path \"/{}\" (reference \"{}\") (unit 1))\n",
            Self::uuid("sheet"),
            p.reference
        ));
        out.push_str("    ))\n");
        out.push_str("  )\n");
    }

    fn emit_net(out: &mut String, net: &crate::analysis::electronics::Net, pts: &[(f64, f64)], label: &str) {
        if pts.len() < 2 {
            return;
        }
        // L-shaped hops between consecutive members (top-down placement).
        let mut ordered: Vec<(f64, f64)> = pts.to_vec();
        ordered.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap()
                .then(a.0.partial_cmp(&b.0).unwrap())
        });
        for w in ordered.windows(2) {
            let (x1, y1) = w[0];
            let (x2, y2) = w[1];
            // Route through the channel BETWEEN the placement columns —
            // never through component bodies. Crossings are fine: crossing
            // wires do not join in schematic semantics.
            let mid = (PLACE_X_LEFT + PLACE_X_RIGHT) / 2.0;
            for seg in [(x1, y1, mid, y1), (mid, y1, mid, y2), (mid, y2, x2, y2)] {
                // Two components in the same column can share pin x — the
                // horizontal hops collapse to zero length; skip those.
                if seg.0 == seg.2 && seg.1 == seg.3 {
                    continue;
                }
                out.push_str(&format!(
                    "  (wire (pts (xy {} {}) (xy {} {})) (stroke (width 0) (type default)) (uuid \"{}\"))\n",
                    coord(seg.0), coord(seg.1), coord(seg.2), coord(seg.3),
                    Self::uuid(&format!(
                        "wire:{}:{}:{}:{}:{}",
                        label, coord(seg.0), coord(seg.1), coord(seg.2), coord(seg.3)
                    ))
                ));
            }
        }
        // One net label at the topmost member.
        let (x, y) = ordered[0];
        out.push_str(&format!(
            "  (label \"{}\" (at {} {} 0) (effects (font (size 1.27 1.27)) (justify (left bottom))) (uuid \"{}\"))\n",
            label,
            coord(x + 1.27),
            coord(y),
            Self::uuid(&format!("label:{}:{}:{}", label, coord(x), coord(y)))
        ));
    }

    /// Emit the `.kicad_pcb` board when the program carries a `fab`
    /// section. None when no board is requested; Err on a layout the
    /// declared physics/geometry cannot satisfy (off-board parts, unknown
    /// footprints, a board too small).
    pub fn generate_board(
        netlist: &ElectronicsNetlist,
        items: &[crate::ast::TopLevel],
    ) -> Result<Option<String>, Vec<String>> {
        let type_props = crate::analysis::electronics::collect_type_metadata(items);
        let instances: BTreeMap<String, ComponentInstance> = netlist
            .components
            .iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();
        let bp = match crate::analysis::placement::place_board(items, &instances, &type_props) {
            Ok(None) => return Ok(None),
            Ok(Some(bp)) => bp,
            Err(e) => return Err(vec![format!("cannot emit board: {}", e)]),
        };
        let (errs, warns) = crate::analysis::placement::check_layout(&bp, &instances);
        for w in &warns {
            eprintln!("note: {}", w);
        }
        if !errs.is_empty() {
            let mut out = vec!["cannot emit board: the layout is invalid".to_string()];
            out.extend(errs.iter().map(|e| format!("  {}", e)));
            return Err(out);
        }
        Ok(Some(Self::emit_pcb(&bp, netlist, &instances)))
    }

    /// The `.kicad_pcb` text (KiCad 7, version 20230121): outline on
    /// Edge.Cuts, one footprint per placed part with pads wired to their
    /// nets. No tracks yet — routing is the follow-on slice.
    fn emit_pcb(
        bp: &crate::analysis::placement::BoardPlacement,
        netlist: &ElectronicsNetlist,
        instances: &BTreeMap<String, ComponentInstance>,
    ) -> String {
        let mut out = String::new();
        out.push_str("(kicad_pcb (version 20230121) (generator \"briev-compiler\")\n");
        out.push_str("  (general (thickness 1.6))\n");
        for (i, net) in netlist.nets.iter().enumerate() {
            let label = Self::net_label(netlist, net);
            out.push_str(&format!("  (net {} \"{}\")\n", i + 1, label));
        }
        out.push_str(&format!(
            "  (gr_rect (start 0 0) (end {} {}) (stroke (width 0.1) (type solid)) (fill none) (layer \"Edge.Cuts\") (tstamp \"00000000-0000-0000-0000-000000000000\"))\n",
            coord(bp.board.0),
            coord(bp.board.1)
        ));
        let refs = Self::reference_map(netlist, &netlist.components);
        let pin_nets = Self::pin_nets(netlist);
        for (name, part) in &bp.parts {
            let Some(inst) = instances.get(name) else {
                continue;
            };
            let package = Self::property_of(inst, "package").unwrap_or_default();
            let Some(fp) = crate::analysis::footprints::lookup(&package) else {
                continue;
            };
            let reference = refs.get(name).cloned().unwrap_or_default();
            let value = Self::property_of(inst, "value").unwrap_or_else(|| inst.type_name.clone());
            let ti = netlist.type_info.get(&inst.type_name);
            let (c, s) = (part.rot.to_radians().cos(), part.rot.to_radians().sin());
            out.push_str(&format!("  (footprint \"{}\"\n", package));
            out.push_str("    (layer \"F.Cu\")\n");
            out.push_str(&format!("    (at {} {} {})\n", coord(part.x), coord(part.y), part.rot));
            out.push_str(&format!(
                "    (fp_text reference \"{}\" (at 0 {}) (layer \"F.SilkS\"))\n",
                reference,
                coord(-1.6)
            ));
            out.push_str(&format!(
                "    (fp_text value \"{}\" (at 0 {}) (layer \"F.Fab\"))\n",
                value,
                coord(1.6)
            ));
            for (i, pad) in fp.pads.iter().enumerate() {
                let px = pad.x * c - pad.y * s;
                let py = pad.x * s + pad.y * c;
                let pin_name = ti
                    .and_then(|t| t.pins.get(i))
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                let (netn, netname) = pin_nets
                    .get(&(name.clone(), pin_name))
                    .cloned()
                    .unwrap_or((0, String::new()));
                out.push_str(&format!(
                    "    (pad \"{}\" smd rect (at {} {}) (size {} {}) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\") (net {} \"{}\"))\n",
                    i + 1,
                    coord(px),
                    coord(py),
                    coord(pad.size),
                    coord(pad.size),
                    netn,
                    netname
                ));
            }
            out.push_str("  )\n");
        }
        // Auto-routed copper (fab plan follow-on): each net's pads connect
        // in a sorted chain on F.Cu; detours surface as notes.
        let (segments, warnings) = routing::route_all(bp, netlist, instances);
        for w in &warnings {
            eprintln!("note: {}", w);
        }
        let net_names: BTreeMap<usize, String> = netlist
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (i + 1, Self::net_label(netlist, n)))
            .collect();
        for seg in segments {
            out.push_str(&format!(
                "  (segment (start {} {}) (end {} {}) (width {}) (layer \"F.Cu\") (net {} \"{}\"))\n",
                coord(seg.x1),
                coord(seg.y1),
                coord(seg.x2),
                coord(seg.y2),
                coord(routing::TRACK_WIDTH),
                seg.net,
                net_names.get(&seg.net).cloned().unwrap_or_default()
            ));
        }
        out.push_str(")\n");
        out
    }

    /// Pin → (net number, net name), from the derived nets.
    fn pin_nets(
        netlist: &ElectronicsNetlist,
    ) -> BTreeMap<(String, String), (usize, String)> {
        let mut map = BTreeMap::new();
        for (i, net) in netlist.nets.iter().enumerate() {
            for pin in &net.pins {
                map.insert(
                    (pin.component.clone(), pin.pin.clone()),
                    (i + 1, net.name.clone()),
                );
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::electronics::derive_netlist;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    fn netlist_of(src: &str) -> ElectronicsNetlist {
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
    fn emits_class_electrical_pin_types() {
        // 2026-09-21 (E12): KiCad electrical pin types come from the class
        // fundamentals' spec KicadType properties; unclassed = passive.
        let src = r#"
            type Power { spec KicadType: "power_in"; };
            type Nc { spec KicadType: "no_connect"; spec NoConnect: true; };
            type Chip { pin vdd: Power; pin prog: Nc; pin gnd; reference "U"; };
            type Header { pin p1: Power; pin gnd; reference "J"; };

            let u1: Chip = Chip { value: "x" };
            let j1: Header = Header { value: "y" };

            txn on
                [j1.p1.voltage == u1.vdd.voltage && j1.gnd.voltage == u1.gnd.voltage]
                [u1.vdd.current >= 0.0]
            { }
        "#;
        let nl = netlist_of(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert_eq!(sch.matches("(pin power_in line").count(), 2, "vdd + p1");
        assert_eq!(sch.matches("(pin no_connect line").count(), 1, "prog");
        assert_eq!(sch.matches("(pin passive line").count(), 2, "unclassed gnd pins");
    }

    #[test]
    fn emits_pin_array_elements() {
        // 2026-09-21 (E11): expanded array elements emit as ordinary pins
        // named `gpio[0]`… — KiCad pin names are arbitrary strings.
        let src = r#"
            type Chip { pin gpio[2]; reference "U"; };
            type Header { pin a; pin b; reference "J"; };

            let u1: Chip = Chip { value: "x" };
            let j1: Header = Header { value: "y" };

            txn on
                [j1.a.voltage == u1.gpio[0].voltage && j1.b.voltage == u1.gpio[1].voltage]
                [u1.gpio[0].current >= 0.0 && u1.gpio[0].current <= 0.02]
            { }
        "#;
        let nl = netlist_of(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert!(sch.contains("name \"gpio[0]\""), "{sch}");
        assert!(sch.contains("name \"gpio[1]\""));
        assert_eq!(sch.matches("(wire ").count(), 6, "2 nets × 3 segments");
    }

    #[test]
    fn refuses_ambiguous_intents() {
        // 2026-09-21 (E14a): an ambiguous drive intent never leaves the
        // compiler — the candidates are enumerated, never chosen silently.
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
        let nl = netlist_of(src);
        let err = ElectronicsBackend::generate(&nl).unwrap_err();
        assert!(err[0].contains("drive intent"), "{:?}", err);
    }

    #[test]
    fn refuses_budget_violations() {
        // 2026-09-21 (E7): a net drawing past its stated budget never
        // leaves the compiler.
        let src = r#"
            type Resistor { pin a; pin b; reference "R"; };
            type Led { pin a; pin k; reference "D"; spec Tolerance: 3.6V; };
            type Connector { pin vcc; pin gnd; reference "J"; };

            let j1: Connector = Connector { value: "JST-2" };
            let r1: Resistor = Resistor { value: "330", spec Resistance: 330Ohm; };
            let d1: Led = Led { value: "red" };

            budget j1.vcc <= 0.005;

            txn on
                [j1.vcc.voltage == 3.3V && j1.vcc.voltage == r1.a.voltage &&
                 r1.b.voltage == d1.a.voltage && d1.k.voltage == j1.gnd.voltage]
                [d1.a.current > 0.0 && d1.a.current <= 0.02]
            { }
        "#;
        let nl = netlist_of(src);
        let err = ElectronicsBackend::generate(&nl).unwrap_err();
        assert!(err[0].contains("budget is exceeded"), "{:?}", err);
    }

    #[test]
    fn refuses_decoupling_violations() {
        // 2026-09-21 (E13): an un-decoupled `spec Decouple` instance never
        // leaves the compiler.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: 100n; };

            let u1: Chip = Chip { value: "mcu" };

            txn on
                [u1.vdd.voltage == u1.vss.voltage]
                [true]
            { }
        "#;
        let nl = netlist_of(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let err = ElectronicsBackend::generate(&nl).unwrap_err();
        assert!(err[0].contains("decoupling convention"), "{:?}", err);
    }

    #[test]
    fn emits_balanced_sexpr_with_components_and_wires() {
        let nl = netlist_of(LED_CIRCUIT);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert_eq!(sch.matches('(').count(), sch.matches(')').count(), "balanced parens");
        assert!(sch.starts_with("(kicad_sch"));
        assert!(sch.contains("(lib_symbols"));
        assert!(sch.contains("\"Resistor\"") && sch.contains("\"Led\"") && sch.contains("\"Connector\""));
        // 3 two-pin nets × 3 segments per hop = 9 wires.
        assert_eq!(sch.matches("(wire ").count(), 9);
        assert!(sch.contains("\"N1\"") && sch.contains("\"N3\""));
    }

    #[test]
    fn reference_metadata_drives_designators() {
        let nl = netlist_of(LED_CIRCUIT);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert!(sch.contains("\"J1\""), "connector ref missing. refs found: J-refs={:?}", sch.lines().filter(|l| l.contains("property \"Reference\"")).collect::<Vec<_>>());
        assert!(sch.contains("\"R1\""));
        assert!(sch.contains("\"D1\""));
    }

    #[test]
    fn value_and_package_become_schematic_properties() {
        let nl = netlist_of(LED_CIRCUIT);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert!(sch.contains("(property \"Value\" \"330\""));
        assert!(sch.contains("(property \"Value\" \"red\""));
        assert!(sch.contains("(property \"Value\" \"JST-2\""));
    }

    #[test]
    fn dangling_netlist_refuses_emission() {
        let src = r#"
            struct Pin { voltage: Float; };
            type A { pin x; pin y;     reference "A";
};
            type B { pin z;     reference "B";
};
            let a: A = A { };
            let b: B = B { };
            txn t [a.x.voltage == a.y.voltage] [a.x.voltage >= 0.0] { }
        "#;
        let nl = netlist_of(src);
        let err = ElectronicsBackend::generate(&nl).unwrap_err();
        assert!(err[0].contains("incomplete"), "{}", err[0]);
        assert!(err.iter().any(|e| e.contains("b.z")));
    }

    #[test]
    fn overvoltage_board_refuses_emission() {
        // 2026-09-11 (B4): electrical violations fail the compile exactly
        // like dangling pins — an unsound board never leaves the compiler.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Power { pin vout; reference "P"; spec Tolerance: any; };
            type Led { pin a; pin k; reference "D"; spec Tolerance: 3.3V; };
            let p1: Power = Power { };
            let d1: Led = Led { };
            txn apply
                [p1.vout.voltage == d1.a.voltage && d1.k.voltage == p1.vout.voltage && p1.vout.voltage == 5.0]
                [d1.a.voltage >= 0.0]
            { }
        "#;
        let nl = netlist_of(src);
        let err = ElectronicsBackend::generate(&nl).unwrap_err();
        assert!(err[0].contains("electrical contracts are violated"), "{}", err[0]);
        assert!(err.iter().any(|e| e.contains("tolerates only 3.3")), "{:?}", err);
    }

    #[test]
    fn emission_is_deterministic() {
        let a = ElectronicsBackend::generate(&netlist_of(LED_CIRCUIT)).unwrap();
        let b = ElectronicsBackend::generate(&netlist_of(LED_CIRCUIT)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn net_labels_derive_from_physics_not_author_names() {
        // 2026-09-22 (retract plan): nets are labelled by WHAT they are —
        // a return-class net is GND, a driven supply net is V{volts}, the
        // rest stay structural N#. No author net names exist anymore.
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Ground { spec KicadType: "power_in"; spec Return: true; spec Tolerance: any; };
            type Power { spec KicadType: "power_in"; spec Supply: true; spec Tolerance: any; };
            type Led { pin a; pin k; reference "D"; spec Tolerance: any; };
            type Conn { pin vbus: Power; pin gnd: Ground; reference "J"; spec Tolerance: any; };
            type Mcu { pin vdd: Power; pin gnd: Ground; reference "U"; spec Tolerance: any; };

            let j1: Conn = Conn { value: "j" };
            let u1: Mcu = Mcu { value: "u" };
            let d1: Led = Led { value: "red" };

            txn powered
                [j1.vbus.voltage == u1.vdd.voltage && j1.gnd.voltage == u1.gnd.voltage
                 && d1.a.voltage == u1.vdd.voltage && d1.k.voltage == u1.gnd.voltage
                 && j1.vbus.voltage == 3.3]
                [d1.a.current > 0.0 && d1.a.current <= 0.02]
            { }
        "#;
        let nl = netlist_of(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        assert!(sch.contains("\"GND\""), "a return-class net must label GND: {sch}");
        assert!(sch.contains("\"V3.3\""), "a 3.3V driven supply net must label V3.3: {sch}");
    }

    #[test]
    fn unpop_part_is_excluded_from_bom() {
        // 2026-09-22 (Slice B): an unpop part keeps its pads (on_board yes)
        // but is excluded from the BOM (in_bom no).
        let src = r#"
            struct Pin { voltage: Float; current: Float; };
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r1: Resistor = Resistor { value: "10k" };
            let j1: Conn = Conn { value: "x" };
            unpop r1;
            txn on [j1.p1.voltage == r1.a.voltage && j1.p2.voltage == r1.b.voltage] { }
        "#;
        let nl = netlist_of(src);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        let r1_inst = sch
            .lines()
            .filter(|l| l.contains("lib_id \"Resistor\""))
            .collect::<Vec<_>>();
        assert!(!r1_inst.is_empty(), "the resistor instance must emit: {sch}");
        assert!(
            sch.contains("(in_bom no) (on_board yes)"),
            "unpop part must be in_bom no: {sch}"
        );
        assert!(
            sch.contains("(in_bom yes) (on_board yes)"),
            "populated parts stay in_bom yes: {sch}"
        );
    }

    // ── 2026-09-23 (E1): instance arrays vs hand-unrolled reference ────

    /// E1's gate: the array form and the hand-unrolled form emit
    /// byte-identical schematics once the standalone UUID lines are
    /// stripped — instance UUIDs hash the instance NAME (`r[0]` vs `r0`),
    /// which legitimately differs; wires/labels keep theirs inline and
    /// must match.
    fn strip_standalone_uuids(sch: &str) -> String {
        sch.lines()
            .filter(|l| !l.trim_start().starts_with("(uuid \""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn instance_array_emission_matches_hand_unrolled() {
        let array_src = r#"
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r[3]: Resistor = Resistor { value: "4k7" };
            let j1: Conn = Conn { value: "x" };
            node n [
                j1.p1.voltage == r[0].a.voltage && r[0].b.voltage == j1.p2.voltage &&
                j1.p1.voltage == r[1].a.voltage && r[1].b.voltage == j1.p2.voltage &&
                j1.p1.voltage == r[2].a.voltage && r[2].b.voltage == j1.p2.voltage
            ] { };
        "#;
        let flat_src = r#"
            type Resistor { pin a; pin b; reference "R"; spec Tolerance: any; };
            type Conn { pin p1; pin p2; reference "J"; };
            let r0: Resistor = Resistor { value: "4k7" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "4k7" };
            let j1: Conn = Conn { value: "x" };
            node n [
                j1.p1.voltage == r0.a.voltage && r0.b.voltage == j1.p2.voltage &&
                j1.p1.voltage == r1.a.voltage && r1.b.voltage == j1.p2.voltage &&
                j1.p1.voltage == r2.a.voltage && r2.b.voltage == j1.p2.voltage
            ] { };
        "#;
        let arr_nl = netlist_of(array_src);
        assert!(arr_nl.dangling.is_empty(), "{:?}", arr_nl.dangling);
        let arr_sch = ElectronicsBackend::generate(&arr_nl).unwrap();

        let flat_nl = netlist_of(flat_src);
        assert!(flat_nl.dangling.is_empty(), "{:?}", flat_nl.dangling);
        let flat_sch = ElectronicsBackend::generate(&flat_nl).unwrap();

        assert_eq!(
            strip_standalone_uuids(&arr_sch),
            strip_standalone_uuids(&flat_sch),
            "array emission must match the hand-unrolled reference"
        );
        // And the array emission is deterministic across runs.
        let arr_sch2 = ElectronicsBackend::generate(&netlist_of(array_src)).unwrap();
        assert_eq!(arr_sch, arr_sch2, "emission must be deterministic");
    }

    // ── E14a gate (plan 2026-09-23-ebv-gate-fixture.md) ────────────────

    /// Parse one source into program items.
    fn items_of(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    /// The gate fixture with the electronics stdlib prepended — the order
    /// the prelude plugin produces (stdlib first, fixture later so its
    /// declarations win the metadata tables).
    fn fixture_items(fixture: &str) -> Vec<crate::ast::TopLevel> {
        let mut items = items_of(include_str!("../../../lib/std/electronics.bv"));
        items.extend(items_of(fixture));
        items
    }

    fn gate_fixture() -> &'static str {
        include_str!("../../../examples/electronics/usb_sensor.ebv")
    }

    /// Fixture text with one replacement — asserts the anchor existed so a
    /// drifted fixture fails loudly instead of silently testing nothing.
    fn mutate(fixture: &str, from: &str, to: &str) -> String {
        assert!(
            fixture.contains(from),
            "fixture drift: missing mutation anchor {from:?}"
        );
        fixture.replacen(from, to, 1)
    }

    /// Every hard error vector empty — the Slice 2 gate's zero-error rule.
    fn assert_clean(nl: &ElectronicsNetlist, label: &str) {
        assert!(nl.intent_errors.is_empty(), "{label} intent: {:?}", nl.intent_errors);
        assert!(nl.class_errors.is_empty(), "{label} class: {:?}", nl.class_errors);
        assert!(
            nl.convention_errors.is_empty(),
            "{label} convention: {:?}",
            nl.convention_errors
        );
        assert!(nl.budget_errors.is_empty(), "{label} budget: {:?}", nl.budget_errors);
        assert!(nl.dangling.is_empty(), "{label} dangling: {:?}", nl.dangling);
        assert!(
            nl.contention_errors.is_empty(),
            "{label} contention: {:?}",
            nl.contention_errors
        );
        assert!(nl.bus_errors.is_empty(), "{label} bus: {:?}", nl.bus_errors);
    }

    fn net_names(nl: &ElectronicsNetlist) -> Vec<String> {
        nl.nets.iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn usb_sensor_gate_fixture_derives_and_emits() {
        // Slice 2 gate: derive_netlist on the real fixture reports zero
        // errors across every vector, and the emitter produces a sheet
        // with unique per-prefix designators (U1-U3, J1-J2, C1-C3, R1-R4).
        let nl = derive_netlist(&fixture_items(gate_fixture()));
        assert_clean(&nl, "usb_sensor");
        // 2026-09-25 (E14b-6): the return side is solver-inferred — the
        // button's low path is forced by the `<= 0.3V` obligation (the
        // p1-pre-wired Spst completes to the return net), and the
        // decoupling convention auto-bridges the supply nets. Gate delta
        // items 1 and 4 of the design record.
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("low-hold forced") && p.contains("sw1")),
            "sw1 low path forced: {:?}",
            nl.intent_proofs
        );
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("auto-bridged") && p.contains("c[0]")),
            "c[0] auto-bridged: {:?}",
            nl.intent_proofs
        );
        assert!(
            !nl.dangling.iter().any(|d| d.contains("sw1.b")),
            "sw1.b must not dangle: {:?}",
            nl.dangling
        );
        assert!(
            nl.nets.len() >= 4,
            "rails + gnd + signals expected, got {} nets",
            nl.nets.len()
        );
        let sch = ElectronicsBackend::generate(&nl).unwrap();
        for want in ["U1", "U2", "U3", "J1", "J2", "C3", "R4", "D1", "SW1"] {
            let needle = format!("\"{want}\"");
            assert!(sch.contains(&needle), "{want} designator missing: {needle}");
        }
        // Instance designators are digit-suffixed (C1…); bare letters (C, U)
        // are lib_symbols template defaults and legitimately repeat per type.
        let refs: Vec<String> = sch
            .lines()
            .filter(|l| l.contains("property \"Reference\" \"") && l.contains("(at "))
            .filter_map(|l| l.split('"').nth(3).map(String::from))
            .filter(|r| r.ends_with(|c: char| c.is_ascii_digit()))
            .collect();
        let mut unique = refs.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            refs.len(),
            unique.len(),
            "duplicate instance references: {refs:?}"
        );
        assert_eq!(refs.len(), 14, "one ref per part: {refs:?}");
    }

    #[test]
    fn usb_sensor_emission_is_deterministic() {
        // Slice 4: two independent derivations (fresh HashMaps, fresh
        // SipHash seeds) produce byte-identical sheets and net names.
        let (sch1, names1) = {
            let nl = derive_netlist(&fixture_items(gate_fixture()));
            (ElectronicsBackend::generate(&nl).unwrap(), net_names(&nl))
        };
        let (sch2, names2) = {
            let nl = derive_netlist(&fixture_items(gate_fixture()));
            (ElectronicsBackend::generate(&nl).unwrap(), net_names(&nl))
        };
        assert_eq!(names1, names2, "net names must be stable");
        assert_eq!(sch1, sch2, "emission must be byte-identical across runs");
    }

    #[test]
    fn error_matrix_missing_led_driver_lists_candidates() {
        // Case 1: remove the led1 drive intent — the remaining en intent
        // now faces TWO free interchangeable pins (batch matching needs
        // the full demand set) and D13 demands the candidates be
        // enumerated, never a silent pick.
        let fx = mutate(gate_fixture(), "led1 = true;", "");
        let nl = derive_netlist(&fixture_items(&fx));
        let joined = nl.intent_errors.join("\n");
        assert!(
            nl.intent_errors.iter().any(|e| e.contains("en") && e.contains("ambiguous")),
            "ambiguous en intent expected: {:?}",
            nl.intent_errors
        );
        assert!(joined.contains("u2.gpio[0]"), "candidates enumerated: {joined}");
        assert!(joined.contains("u2.gpio[1]"), "candidates enumerated: {joined}");
    }

    #[test]
    fn error_matrix_removed_decap_breaks_convention() {
        // Case 3 (2026-09-25, E14b-6): removing a decoupling capacitor now
        // means shrinking the population — free decouplers are consumed by
        // the forcing rule one-to-one, so one cap fewer than supply pins
        // leaves the last obligation unbridgeable. The E13 convention must
        // refuse.
        let fx = mutate(gate_fixture(), "let c[i:2]: Capacitor", "let c[i:1]: Capacitor");
        let nl = derive_netlist(&fixture_items(&fx));
        assert!(
            nl.convention_errors
                .iter()
                .any(|e| e.contains("u1") && e.contains("supply pin 'vout'")),
            "u1.vout decoupling convention must fail: {:?}",
            nl.convention_errors
        );
    }

    #[test]
    fn error_matrix_open_on_wired_pins_refuses() {
        // Case 4: `open` naming two pins already tied by the rail facts —
        // the disconnection is impossible; hard error (D16 p3b).
        let fx = mutate(
            gate_fixture(),
            "led1 = true;",
            "led1 = true;\n    open u1.vout, u2.vdd;",
        );
        let nl = derive_netlist(&fixture_items(&fx));
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains("open") && e.contains("already connected")),
            "open-on-wired must refuse: {:?}",
            nl.intent_errors
        );
    }

    // ── E14b slice 1 (plan 2026-09-23-ebv-e14b-pullup-forcing.md) ──────

    /// A minimal board with an open-drain net that must be pulled up.
    const PULL_UP_BOARD: &str = r#"
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
        type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
        type Mcu { pin vdd: Power; pin sda: IoOd; reference "U"; };
        let u1: Mcu = Mcu { value: "x" };
        let r1: Resistor = Resistor { value: "4k7" };
        async node n [
            u1.vdd.voltage == 3.3V && u1.sda.voltage == u2_sda
        ] [u1.vdd.voltage == 3.3V] {
        }
    "#;

    #[test]
    fn e14b_min_obligation_forces_pull_up() {
        // `u2.sda.voltage >= 2.7V` on an undriven WiredAnd net → the free
        // PullUp part wires between the net and the lowest qualifying rail
        // (3.3V, not 5V). The obligation records as a proof, not a wire.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin sda: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r1: Resistor = Resistor { value: "4k7" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.sda.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("pull-up forced") && p.contains("2.7V")),
            "obligation must force a pull-up with provenance: {:?}",
            nl.intent_proofs
        );
        // Both r1 pins are now on nets (no dangling).
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_no_pull_up_part_is_a_hard_error() {
        // Obligation with no free PullUp part → D13 no-completion error.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Mcu { pin vdd: Power; pin sda: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.sda.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains(">= 2.7V") && e.contains("no pull-up part")),
            "{:?}",
            nl.intent_errors
        );
    }

    #[test]
    fn e14b_identical_pull_ups_assign_not_ambiguous() {
        // Two obligations, two identical-value pull-ups → a perfect
        // matching: the forced assignment is deterministic, NOT ambiguous.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin sda: IoOd; pin scl: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "4k7" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.sda.voltage >= 2.7V;
                u1.scl.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        let forced = nl
            .intent_proofs
            .iter()
            .filter(|p| p.contains("pull-up forced"))
            .count();
        assert_eq!(forced, 2, "one pull-up per obligation: {:?}", nl.intent_proofs);
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_distinct_value_pull_ups_assign_deterministically() {
        // One obligation, two free pull-ups of different value — a MIN
        // obligation is satisfied by any pull-up resistance, so the pick
        // is deterministic (D13: the choice never matters). The unused
        // part stays free (dangles).
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin sda: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "10k" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.sda.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            1,
            "one obligation → one deterministic pull-up: {:?}",
            nl.intent_proofs
        );
        assert!(
            nl.dangling.iter().any(|d| d.contains("r2")),
            "the unused part dangles: {:?}",
            nl.dangling
        );
    }

    #[test]
    fn e14b_value_aware_matching_assigns_all_buses() {
        // Three obligations (two i2c buses + a button pull-up) × three
        // free pull-ups of differing value (4k7, 4k7, 10k) — every net
        // gets one, no ambiguity, nothing dangles.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin sda: IoOd; pin scl: IoOd; pin gpio: Io; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r_pu0: Resistor = Resistor { value: "4k7" };
            let r_pu1: Resistor = Resistor { value: "4k7" };
            let r_btn: Resistor = Resistor { value: "10k" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.sda.voltage >= 2.7V;
                u1.scl.voltage >= 2.7V;
                u1.gpio.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            3,
            "every obligation gets a pull-up: {:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    /// A pulled-up IO pin plus a switch — the low-hold forcing target.
    const LOW_HOLD_BOARD: &str = r#"
        type Ground { spec KicadType: "power_in"; spec Return: true; };
        type Power { spec KicadType: "power_in"; spec Supply: true; };
        type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
        type Path { spec KicadType: "passive"; spec Switchable: true; };
        type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
        type Mcu { pin vdd: Power; pin gnd: Ground; pin gpio: Io; reference "U"; };
        type Switch { pin p1: Path; pin p2: Path; reference "SW"; };
        let u1: Mcu = Mcu { value: "x" };
        let r1: Resistor = Resistor { value: "10k" };
        let sw1: Switch = Switch { value: "SPST" };
        async node n [
            u1.vdd.voltage == 3.3V && u1.gnd.voltage == sw1.b.voltage
        ] [u1.vdd.voltage == 3.3V] {
            r1.a = u1.vdd;
            r1.b = u1.gpio;
            u1.gpio.voltage <= 0.3V;
        }
    "#;

    #[test]
    fn e14b_max_obligation_forces_switch_path() {
        // `u1.gpio.voltage <= 0.3V` on a pulled-up net → the free switch
        // wires net→p1; p2 is already on the return rail via the guard.
        let nl = netlist_of(LOW_HOLD_BOARD);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("low-hold forced") && p.contains("sw1.p1")),
            "low-hold must wire the switch with provenance: {:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_pulled_up_net_without_switch_errors() {
        // Pulled-up net, no switchable part → the net cannot be held low.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin gnd: Ground; pin gpio: Io; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r1: Resistor = Resistor { value: "10k" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                r1.a = u1.vdd;
                r1.b = u1.gpio;
                u1.gpio.voltage <= 0.3V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains("<= 0.3V") && e.contains("cannot hold the net low")),
            "{:?}",
            nl.intent_errors
        );
    }

    #[test]
    fn e14b_isolated_net_wires_direct_to_return() {
        // No pull-up, no switch — the net is permanently low; direct wire.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Conn { pin vbus: Power; pin gnd: Ground; reference "J"; };
            type Mcu { pin vdd: Power; pin gnd: Ground; pin gpio: Io; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let j1: Conn = Conn { value: "x" };
            async node n [
                j1.vbus.voltage == u1.vdd.voltage
                && u1.vdd.voltage == 3.3V
                && j1.gnd.voltage == u1.gnd.voltage
            ] [u1.vdd.voltage == 3.3V] {
                u1.gpio.voltage <= 0.3V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs
                .iter()
                .any(|p| p.contains("low-hold forced") && p.contains("net <-> return rail")),
            "{:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_distinct_switches_are_ambiguous() {
        // Two free switches of differing value → the pick matters; D13.
        let src = r#"
            type Ground { spec KicadType: "power_in"; spec Return: true; };
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Path { spec KicadType: "passive"; spec Switchable: true; };
            type Mcu { pin vdd: Power; pin gnd: Ground; pin gpio: Io; reference "U"; };
            type Switch { pin p1: Path; pin p2: Path; reference "SW"; };
            let u1: Mcu = Mcu { value: "x" };
            let sw1: Switch = Switch { value: "SPST" };
            let sw2: Switch = Switch { value: "DPDT" };
            async node n [
                u1.vdd.voltage == 3.3V && u1.gnd.voltage == sw1.b.voltage
            ] [u1.vdd.voltage == 3.3V] {
                u1.gpio.voltage <= 0.3V;
            }
        "#;
let nl = netlist_of(src);
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains("ambiguous") && e.contains("sw1") && e.contains("sw2")),
            "{:?}",
            nl.intent_errors
        );
    }

    // ── E14b slice 4 (plan 2026-09-23-ebv-e14b-drive-assignment.md) ────

    #[test]
    fn e14b_drive_assignment_matches_interchangeable_pins() {
        // en (pin intent) + led1 (instance intent) both need a drive;
        // gpio[0], gpio[1] are interchangeable Io pins → a perfect
        // matching assigns deterministically, no ambiguity error.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type In { spec KicadType: "input"; };
            type Led { pin a; pin k; reference "D"; };
            type Mcu { pin vdd: Power; pin gpio[2]: Io; pin en: In; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let led1: Led = Led { value: "green" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                led1.a = u1.vdd;
                u1.en = true;
                led1 = true;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("drive assignment")).count(),
            2,
            "both intents must resolve: {:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_drive_shortage_is_a_no_completion_error() {
        // led1 consumes the only free pin; the en intent then has no
        // completion → D13 error, never a silent drop.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type In { spec KicadType: "input"; };
            type Led { pin a; pin k; reference "D"; };
            type Mcu { pin vdd: Power; pin gpio[2]: Io; pin en: In; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let led1: Led = Led { value: "green" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                led1.a = u1.gpio[1];
                u1.en = true;
                led1 = true;
            }
        "#;
        let nl = netlist_of(src);
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains("u1.en") && e.contains("no completion")),
            "{:?}",
            nl.intent_errors
        );
    }

    #[test]
    fn e14b_mixed_class_supply_is_ambiguous() {
        // gpio (Io) + an Out pin compete — the class of the completer
        // matters, so D13 enumerates all candidates instead of picking.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Out { spec KicadType: "output"; spec CanDrive: true; };
            type Led { pin a; pin k; reference "D"; };
            type Chip { pin o: Out; reference "U"; };
            type Mcu { pin vdd: Power; pin gpio[2]: Io; pin en: Io; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let u2: Chip = Chip { value: "y" };
            let led1: Led = Led { value: "green" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                led1.a = u1.vdd;
                u1.en = true;
                led1 = true;
            }
        "#;
        let nl = netlist_of(src);
        assert!(
            nl.intent_errors
                .iter()
                .any(|e| e.contains("ambiguous") && e.contains("u2.o")),
            "mixed-class supply must enumerate: {:?}",
            nl.intent_errors
        );
    }

    #[test]
    fn e14b_same_name_wired_and_obligations_assemble_the_bus() {
        // u1.od and u2.od both demand >= 2.7V → one bus, one pull-up.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin od: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let u2: Mcu = Mcu { value: "y" };
            let r1: Resistor = Resistor { value: "4k7" };
            async node n [
                u1.vdd.voltage == u2.vdd.voltage && u1.vdd.voltage == 3.3V
            ] [u1.vdd.voltage == 3.3V] {
                u1.od.voltage >= 2.7V;
                u2.od.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            nl.intent_proofs.iter().any(|p| p.contains("bus assembled") && p.contains("u1") && p.contains("u2")),
            "{:?}",
            nl.intent_proofs
        );
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            1,
            "one bus → one pull-up: {:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_different_names_keep_separate_nets() {
        // od1 vs od2 — different signal names never union; each bus gets
        // its own pull-up.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin od1: IoOd; pin od2: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "4k7" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {
                u1.od1.voltage >= 2.7V;
                u1.od2.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            !nl.intent_proofs.iter().any(|p| p.contains("bus assembled")),
            "different names must NOT assemble: {:?}",
            nl.intent_proofs
        );
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            2,
            "{:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_non_wired_and_pins_never_assemble() {
        // Same name, same volts, but NOT open-drain → separate nets (two
        // independent outputs must not short).
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Io { spec KicadType: "bidirectional"; spec CanDrive: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin outp: Io; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let u2: Mcu = Mcu { value: "y" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "4k7" };
            async node n [
                u1.vdd.voltage == u2.vdd.voltage && u1.vdd.voltage == 3.3V
            ] [u1.vdd.voltage == 3.3V] {
                u1.outp.voltage >= 2.7V;
                u2.outp.voltage >= 2.7V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            !nl.intent_proofs.iter().any(|p| p.contains("bus assembled")),
            "non-WiredAnd pins must NOT assemble: {:?}",
            nl.intent_proofs
        );
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            2,
            "{:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    #[test]
    fn e14b_differing_voltages_never_assemble() {
        // Same name, different obligation voltages → different pull-up
        // targets; separate nets.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type IoOd { spec KicadType: "open_collector"; spec CanDrive: true; spec WiredAnd: true; };
            type Resistor { pin a; pin b; reference "R"; spec PullUp: true; };
            type Mcu { pin vdd: Power; pin od: IoOd; reference "U"; };
            let u1: Mcu = Mcu { value: "x" };
            let u2: Mcu = Mcu { value: "y" };
            let r1: Resistor = Resistor { value: "4k7" };
            let r2: Resistor = Resistor { value: "4k7" };
            async node n [
                u1.vdd.voltage == u2.vdd.voltage && u1.vdd.voltage == 3.3V
            ] [u1.vdd.voltage == 3.3V] {
                u1.od.voltage >= 2.7V;
                u2.od.voltage >= 3.0V;
            }
        "#;
        let nl = netlist_of(src);
        assert!(nl.intent_errors.is_empty(), "{:?}", nl.intent_errors);
        assert!(
            !nl.intent_proofs.iter().any(|p| p.contains("bus assembled")),
            "differing voltages must NOT assemble: {:?}",
            nl.intent_proofs
        );
        assert_eq!(
            nl.intent_proofs.iter().filter(|p| p.contains("pull-up forced")).count(),
            2,
            "{:?}",
            nl.intent_proofs
        );
        assert!(nl.dangling.is_empty(), "{:?}", nl.dangling);
    }

    // ── fab layer (plan 2026-09-23-ebv-fab-layer.md) ──────────────────

    #[test]
    fn fab_board_emits_pcb() {
        // The gate fixture's fab section produces a .kicad_pcb: outline,
        // physics-derived net labels, one footprint per placed part with
        // wired pads — and byte-deterministic across runs.
        let items = fixture_items(gate_fixture());
        let nl = derive_netlist(&items);
        let board = ElectronicsBackend::generate_board(&nl, &items)
            .unwrap()
            .expect("the fixture carries a fab section");
        assert!(board.starts_with("(kicad_pcb"), "{board}");
        assert!(
            board.contains("(net 1 \"GND\"") && board.contains("V3.3"),
            "physics-derived nets: {board}"
        );
        assert!(
            board.contains("(gr_rect (start 0 0) (end 40.00 30.00)"),
            "board outline: {board}"
        );
        let footprints = board.matches("(footprint \"").count();
        assert!(footprints >= 14, "one per part: {footprints}");
        assert!(
            board.contains("(pad \"1\" smd rect"),
            "pads wired: {board}"
        );
        assert!(
            board.contains("(segment (start"),
            "routed copper: {board}"
        );
        // Determinism: an independent derivation is byte-identical (the
        // routed segments are part of that — routing is deterministic).
        let nl2 = derive_netlist(&fixture_items(gate_fixture()));
        let board2 = ElectronicsBackend::generate_board(&nl2, &fixture_items(gate_fixture()))
            .unwrap()
            .unwrap();
        assert_eq!(board, board2, "board emission must be deterministic");
    }

    #[test]
    fn fab_off_board_is_rejected() {
        // A pinned part past the outline edge is a hard error, never a
        // clipped board.
        let fx = mutate(
            gate_fixture(),
            "place j2 @ (33mm, 15mm)",
            "place j2 @ (45mm, 15mm)",
        );
        let items = fixture_items(&fx);
        let nl = derive_netlist(&items);
        let err = ElectronicsBackend::generate_board(&nl, &items).unwrap_err();
        assert!(
            err.iter().any(|e| e.contains("does not fit")),
            "{:?}",
            err
        );
    }

    #[test]
    fn fab_unknown_package_is_rejected() {
        let fx = mutate(gate_fixture(), "package: \"0603\"", "package: \"not-a-package\"");
        let items = fixture_items(&fx);
        let nl = derive_netlist(&items);
        let err = ElectronicsBackend::generate_board(&nl, &items).unwrap_err();
        assert!(
            err.iter().any(|e| e.contains("no footprint")),
            "{:?}",
            err
        );
    }

    #[test]
    fn no_fab_means_no_board() {
        // A program without a fab section asks for no board.
        let src = r#"
            type Power { spec KicadType: "power_in"; spec Supply: true; };
            type Chip { pin vdd: Power; reference "U"; };
            let u1: Chip = Chip { value: "x", package: "SMD" };
            async node n [u1.vdd.voltage == 3.3V] [u1.vdd.voltage == 3.3V] {}
        "#;
        let items = items_of(src);
        let nl = derive_netlist(&items);
        let board = ElectronicsBackend::generate_board(&nl, &items).unwrap();
        assert!(board.is_none(), "no fab section → no board");
    }
}
