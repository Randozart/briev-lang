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

use crate::analysis::electronics::{ComponentInstance, ElectronicsNetlist};
use crate::backend::capabilities::BackendCapabilities;

pub const CAPABILITIES: BackendCapabilities = BackendCapabilities {
    name: "electronics",
    nature: "PCB schematic (KiCad S-expression)",
    // 2026-09-11: contract expressions are PROVEN at compile time, never
    // executed — so comparisons/arithmetic/float literals inside [pre][post]
    // obligations are part of the electronics surface. What electronics
    // forbids is executable surface: bodies, calls, runtime, concurrency.
    int_literals: true,
    floats: true,
    strings: true,
    bool_char_literals: false,
    int_ops: true,
    unary_ops: true,
    calls: false,
    intrinsics: false,
    if_expr: false,
    match_expr: false,
    block_expr: false,
    field_access: true,
    index: false,
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
    assign_stmt: false,
    arrow_assign: false,
    guarded_stmt: false,
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
}

pub struct ElectronicsBackend;

impl ElectronicsBackend {
    /// Emit the `.kicad_sch` text. Fails on dangling pins AND on electrical
    /// violations — an incomplete or electrically-unsound board never leaves
    /// the compiler.
    pub fn generate(netlist: &ElectronicsNetlist) -> Result<String, Vec<String>> {
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
    /// coordinates for routing. Reference designators run PER TYPE
    /// (R1, R2… D1… J1…), KiCad style.
    fn emit_instances(
        netlist: &ElectronicsNetlist,
        components: &[ComponentInstance],
        out: &mut String,
    ) -> Vec<(String, String, f64, f64)> {
        let mut pin_xy = Vec::new();
        let mut type_counters: std::collections::BTreeMap<String, usize> = Default::default();
        for (idx, comp) in components.iter().enumerate() {
            let counter = type_counters.entry(comp.type_name.clone()).or_insert(0);
            *counter += 1;
            let (x, y) = (
                if idx % 2 == 0 { PLACE_X_LEFT } else { PLACE_X_RIGHT },
                PLACE_Y0 + idx as f64 * PLACE_PITCH,
            );
            let reference = Self::reference_of(netlist, &comp.type_name, *counter);
            let value = Self::property_of(comp, "value").unwrap_or_else(|| comp.type_name.clone());
            let footprint = Self::property_of(comp, "package").unwrap_or_default();
            let placement = Placement {
                comp,
                x,
                y,
                reference: &reference,
                value: &value,
                footprint: &footprint,
            };
            Self::emit_instance(out, &placement);
            for (x_off, y_off, pin_name, _) in Self::pin_offsets(netlist, &comp.type_name) {
                pin_xy.push((comp.name.clone(), pin_name, x + x_off, y + y_off));
            }
        }
        out.push('\n');
        pin_xy
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

    fn reference_of(netlist: &ElectronicsNetlist, type_name: &str, counter: usize) -> String {
        let prefix = netlist
            .type_info
            .get(type_name)
            .map(|t| t.reference_prefix.as_str())
            .unwrap_or("U");
        format!("{}{}", prefix, counter)
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
        out.push_str("    (in_bom yes) (on_board yes)\n");
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
            type Led { pin a; pin k; reference "D"; tolerance 3.6; };
            type Connector { pin vcc; pin gnd; reference "J"; };

            let j1: Connector = Connector { value: "JST-2" };
            let r1: Resistor = Resistor { value: "330" };
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
            type Chip { pin vdd: Power; pin vss: Ground; reference "U"; spec Decouple: "100n"; };

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
            type Power { pin vout; reference "P"; tolerance any; };
            type Led { pin a; pin k; reference "D"; tolerance 3.3; };
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
            type Ground { spec KicadType: "power_in"; spec Return: true; tolerance any; };
            type Power { spec KicadType: "power_in"; spec Supply: true; tolerance any; };
            type Led { pin a; pin k; reference "D"; tolerance any; };
            type Conn { pin vbus: Power; pin gnd: Ground; reference "J"; tolerance any; };
            type Mcu { pin vdd: Power; pin gnd: Ground; reference "U"; tolerance any; };

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
}
