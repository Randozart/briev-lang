// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Minimal deterministic Manhattan router (2026-09-23 fab plan, follow-on).
//!
//! The author controls placement; the compiler routes the copper (no
//! routing control surface). Each net's pads connect in a sorted chain
//! with L-shaped (Manhattan) tracks on F.Cu; a straight path that crosses
//! a part footprint is joggled over it. Deterministic throughout — the
//! same input always produces the same copper. v1 is a point-to-point
//! router: track-to-track clearance DRC, vias, and multi-layer planning
//! are recorded future work.

use std::collections::BTreeMap;

use crate::analysis::electronics::{ComponentInstance, ElectronicsNetlist};
use crate::analysis::placement::BoardPlacement;

/// Track width (mm).
pub const TRACK_WIDTH: f64 = 0.25;
/// Clearance from a track to an obstacle footprint (mm).
const TRACK_CLEARANCE: f64 = 0.25;

/// One routed copper track on F.Cu.
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub net: usize,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// A footprint obstacle: the part name + its axis-aligned box (mm).
#[derive(Debug, Clone)]
struct Obstacle {
    name: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

/// Route every net's pads in a sorted chain. Returns the F.Cu segments
/// plus any detour warnings (v1: an unresolvable crossing is accepted
/// with a warning, never a silent short).
pub fn route_all(
    bp: &BoardPlacement,
    netlist: &ElectronicsNetlist,
    instances: &BTreeMap<String, ComponentInstance>,
) -> (Vec<Segment>, Vec<String>) {
    let obstacles = obstacles_of(bp, instances);
    let pads = pin_pad_positions(bp, netlist, instances);
    let mut segments = Vec::new();
    let mut warnings = Vec::new();
    for (i, net) in netlist.nets.iter().enumerate() {
        let (segs, warns) = route_net(i + 1, net, &pads, &obstacles);
        segments.extend(segs);
        warnings.extend(warns);
    }
    (segments, warnings)
}

/// Route one net's pins in a sorted chain.
fn route_net(
    net_number: usize,
    net: &crate::analysis::electronics::Net,
    pads: &BTreeMap<(String, String), (f64, f64)>,
    obstacles: &[Obstacle],
) -> (Vec<Segment>, Vec<String>) {
    let mut chain: Vec<(f64, f64, String)> = Vec::new();
    for pin in &net.pins {
        if let Some(pt) = pads.get(&(pin.component.clone(), pin.pin.clone())) {
            chain.push((pt.0, pt.1, pin.component.clone()));
        }
    }
    route_chain(net_number, &chain, &net.name, obstacles)
}

/// Connect the sorted chain's pads; returns the segments + any detours.
fn route_chain(
    net_number: usize,
    chain: &[(f64, f64, String)],
    net_name: &str,
    obstacles: &[Obstacle],
) -> (Vec<Segment>, Vec<String>) {
    let mut segments = Vec::new();
    let mut warnings = Vec::new();
    if chain.len() < 2 {
        return (segments, warnings);
    }
    let mut sorted: Vec<&(f64, f64, String)> = chain.iter().collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    for w in sorted.windows(2) {
        let a = w[0];
        let b = w[1];
        let exclude = vec![a.2.clone(), b.2.clone()];
        let (segs, warn) = route_pair((a.0, a.1), (b.0, b.1), obstacles, &exclude);
        if !warn.is_empty() {
            warnings.push(format!("net '{}': {}", net_name, warn.join(", ")));
        }
        segments.extend(segs.iter().map(|(x1, y1, x2, y2)| Segment {
            net: net_number,
            x1: *x1,
            y1: *y1,
            x2: *x2,
            y2: *y2,
        }));
    }
    (segments, warnings)
}

/// Route one pair of pads. A clean L (H-first, then V-first) wins; a
/// crossing L is joggled over its obstacle; an unresolvable crossing is
/// accepted with a warning.
fn route_pair(
    a: (f64, f64),
    b: (f64, f64),
    obstacles: &[Obstacle],
    exclude: &[String],
) -> (Vec<(f64, f64, f64, f64)>, Vec<String>) {
    let h = [(a.0, a.1, b.0, a.1), (b.0, a.1, b.0, b.1)];
    if h.iter().all(|s| clear(s, obstacles, exclude)) {
        return (h.to_vec(), Vec::new());
    }
    let v = [(a.0, a.1, a.0, b.1), (a.0, b.1, b.0, b.1)];
    if v.iter().all(|s| clear(s, obstacles, exclude)) {
        return (v.to_vec(), Vec::new());
    }
    if let Some(obs) = hit_obstacle(&h, obstacles, exclude) {
        // Jog over the first obstacle: rise above its top edge, cross,
        // drop to the target.
        let jog_y = obs.y2 + TRACK_CLEARANCE + TRACK_WIDTH;
        let detour = [
            (a.0, a.1, a.0, jog_y),
            (a.0, jog_y, b.0, jog_y),
            (b.0, jog_y, b.0, b.1),
        ];
        return (detour.to_vec(), vec!["jogged over an obstacle".into()]);
    }
    (h.to_vec(), vec!["unavoidable crossing".into()])
}

/// Whether an axis-aligned track clears every obstacle (minus the endpoint
/// parts) by TRACK_WIDTH/2 + TRACK_CLEARANCE.
fn clear(seg: &(f64, f64, f64, f64), obstacles: &[Obstacle], exclude: &[String]) -> bool {
    let (x1, y1, x2, y2) = *seg;
    let m = TRACK_WIDTH / 2.0 + TRACK_CLEARANCE;
    let minx = x1.min(x2) - m;
    let maxx = x1.max(x2) + m;
    let miny = y1.min(y2) - m;
    let maxy = y1.max(y2) + m;
    !obstacles.iter().any(|o| {
        !exclude.iter().any(|e| e == &o.name)
            && minx <= o.x2
            && o.x1 <= maxx
            && miny <= o.y2
            && o.y1 <= maxy
    })
}

/// The first obstacle crossed by any segment of `segs` (the jog target).
fn hit_obstacle<'a>(
    segs: &[(f64, f64, f64, f64)],
    obstacles: &'a [Obstacle],
    exclude: &[String],
) -> Option<&'a Obstacle> {
    obstacles.iter().find(|o| {
        !exclude.iter().any(|e| e == &o.name)
            && segs.iter().any(|s| {
                let (x1, y1, x2, y2) = *s;
                let minx = x1.min(x2);
                let maxx = x1.max(x2);
                let miny = y1.min(y2);
                let maxy = y1.max(y2);
                minx <= o.x2 && o.x1 <= maxx && miny <= o.y2 && o.y1 <= maxy
            })
    })
}

/// The footprint boxes of every placed part (axis-aligned, rotation
/// applied) — the routing obstacles.
fn obstacles_of(
    bp: &BoardPlacement,
    instances: &BTreeMap<String, ComponentInstance>,
) -> Vec<Obstacle> {
    let mut out = Vec::new();
    for (name, part) in &bp.parts {
        let Some(fp) = footprint_of(instances, name) else {
            continue;
        };
        let (fw, fh) = fp.outline;
        let (w, h) = if part.rot.rem_euclid(180.0) == 90.0 { (fh, fw) } else { (fw, fh) };
        out.push(Obstacle {
            name: name.clone(),
            x1: part.x - w / 2.0,
            y1: part.y - h / 2.0,
            x2: part.x + w / 2.0,
            y2: part.y + h / 2.0,
        });
    }
    out
}

/// Each pin's pad centre on the board (component, pin) → (x, y).
fn pin_pad_positions(
    bp: &BoardPlacement,
    netlist: &ElectronicsNetlist,
    instances: &BTreeMap<String, ComponentInstance>,
) -> BTreeMap<(String, String), (f64, f64)> {
    let mut out = BTreeMap::new();
    let mut flat: Vec<(String, String, f64, f64)> = Vec::new();
    for (name, part) in &bp.parts {
        let Some(fp) = footprint_of(instances, name) else {
            continue;
        };
        let (c, s) = (part.rot.to_radians().cos(), part.rot.to_radians().sin());
        let Some(inst) = instances.get(name) else {
            continue;
        };
        let Some(ti) = netlist.type_info.get(&inst.type_name) else {
            continue;
        };
        flat.extend(
            part_pads(part, fp, ti, c, s)
                .into_iter()
                .map(|(pin, px, py)| (name.clone(), pin, px, py)),
        );
    }
    for (name, pin, px, py) in flat {
        out.insert((name, pin), (px, py));
    }
    out
}

/// A part's pads at their board positions: (pin_name, x, y).
fn part_pads(
    part: &crate::analysis::placement::PlacedPart,
    fp: &crate::analysis::footprints::Footprint,
    ti: &crate::analysis::electronics::TypeInfo,
    c: f64,
    s: f64,
) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    for (i, pad) in fp.pads.iter().enumerate() {
        let px = part.x + (pad.x * c - pad.y * s);
        let py = part.y + (pad.x * s + pad.y * c);
        let Some((pin_name, _)) = ti.pins.get(i) else {
            continue;
        };
        out.push((pin_name.clone(), px, py));
    }
    out
}

/// The footprint of an instance by its package annotation.
fn footprint_of<'a>(
    instances: &'a BTreeMap<String, ComponentInstance>,
    name: &str,
) -> Option<&'a crate::analysis::footprints::Footprint> {
    let inst = instances.get(name)?;
    let package = inst
        .properties
        .iter()
        .find(|(k, _)| k == "package")
        .map(|(_, v)| v.as_str())?;
    crate::analysis::footprints::lookup(package)
}