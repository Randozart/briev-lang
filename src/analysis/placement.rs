// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Board placement (2026-09-23 fab plan, `2026-09-23-ebv-fab-layer.md`).
//!
//! The fab section pins part positions; everything unplaced flows to a
//! deterministic auto-placer. Placement is the author's control (Rule 2:
//! pinned wins, the rest gets a good default), never a silent pick — the
//! result is fully deterministic (sorted) and board-aware (a board too
//! small to hold the parts is a hard error, not a clipping).

use std::collections::{BTreeMap, HashMap};

use crate::ast::{FabBlock, FabPlacement, PropertyValue, TopLevel};

/// One part's board position (mm) + rotation (degrees).
#[derive(Debug, Clone, Copy)]
pub struct PlacedPart {
    pub x: f64,
    pub y: f64,
    pub rot: f64,
}

/// The full board placement — one entry per instance, sorted by name.
#[derive(Debug, Clone)]
pub struct BoardPlacement {
    pub board: (f64, f64),
    pub parts: BTreeMap<String, PlacedPart>,
}

/// Grid pitch (mm): the candidate lattice for auto-placement. The
/// collision check (MIN_CLEARANCE) does the real spacing, so the lattice
/// can be finer than a part's footprint.
const PITCH: f64 = 3.0;
/// Margin from the board edge to the first part centre (mm).
const EDGE_MARGIN: f64 = 4.0;
/// Ring radius for decouplers placed near their decoupled part (mm).
const DECOUPLER_RING: f64 = 5.0;
/// Minimum part-to-part clearance (mm) — closer is a layout warning.
const MIN_CLEARANCE: f64 = 0.5;

/// Layout checks (2026-09-23 fab plan): containment (a part off the board
/// outline is a hard error — it can never be manufactured) and clearance
/// (parts closer than MIN_CLEARANCE are a warning). Both are provable from
/// the declared outline + footprints + positions — the "compiler proves the
/// rest" applied to layout.
pub fn check_layout(
    bp: &BoardPlacement,
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut boxes: Vec<(String, (f64, f64, f64, f64))> = Vec::new();
    for (name, part) in &bp.parts {
        match part_box(instances, name, part) {
            Err(e) => errors.push(e),
            Ok(bb) => {
                if !fits_board(&bb, bp.board) {
                    errors.push(format!(
                        "part '{}' at ({}mm, {}mm) does not fit the {}mm x {}mm board outline",
                        name, part.x, part.y, bp.board.0, bp.board.1
                    ));
                }
                boxes.push((name.clone(), bb));
            }
        }
    }
    (errors, clearance_warnings(&boxes))
}

/// The axis-aligned bounding box of a placed part (mm).
fn part_box(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    name: &str,
    part: &PlacedPart,
) -> Result<(f64, f64, f64, f64), String> {
    let inst = instances.get(name).ok_or_else(|| format!("unknown part '{}'", name))?;
    let package = inst
        .properties
        .iter()
        .find(|(k, _)| k == "package")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let Some(fp) = crate::analysis::footprints::lookup(package) else {
        return Err(format!(
            "no footprint for package '{}' (part '{}') — add an entry to config/footprints.dbvl",
            package, name
        ));
    };
    let (fw, fh) = fp.outline;
    let (w, h) = if part.rot.rem_euclid(180.0) == 90.0 { (fh, fw) } else { (fw, fh) };
    Ok((part.x - w / 2.0, part.y - h / 2.0, part.x + w / 2.0, part.y + h / 2.0))
}

/// Whether a box lies inside the board outline.
fn fits_board(bb: &(f64, f64, f64, f64), board: (f64, f64)) -> bool {
    bb.0 >= -1e-9 && bb.1 >= -1e-9 && bb.2 <= board.0 + 1e-9 && bb.3 <= board.1 + 1e-9
}

/// Pairwise clearance warnings over the placed bounding boxes, via an
/// x-sorted sweep so the scan is near-linear (parts are few, but the
/// comparison is never quadratic in the whole set).
fn clearance_warnings(boxes: &[(String, (f64, f64, f64, f64))]) -> Vec<String> {
    let mut sorted: Vec<(&String, (f64, f64, f64, f64))> =
        boxes.iter().map(|(n, b)| (n, *b)).collect();
    sorted.sort_by(|a, b| a.1 .0.total_cmp(&b.1 .0));
    let mut warnings = Vec::new();
    for i in 0..sorted.len() {
        warnings.extend(warn_close(&sorted, i));
    }
    warnings
}

/// The parts ahead of `i` whose boxes are close enough to matter.
fn warn_close(
    sorted: &[(&String, (f64, f64, f64, f64))],
    i: usize,
) -> Vec<String> {
    let (na, a) = &sorted[i];
    let mut warnings = Vec::new();
    for (nb, b) in sorted.iter().skip(i + 1) {
        if b.0 > a.2 + MIN_CLEARANCE {
            break;
        }
        if aabb_clear(a, b, MIN_CLEARANCE) {
            continue;
        }
        warnings.push(format!(
            "parts '{}' and '{}' are closer than {}mm",
            na, nb, MIN_CLEARANCE
        ));
    }
    warnings
}

/// Whether two axis-aligned boxes are at least `min` apart on both axes.
fn aabb_clear(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64), min: f64) -> bool {
    a.2 < b.0 - min || b.2 < a.0 - min || a.3 < b.1 - min || b.3 < a.1 - min
}

/// Place the board from the fab section. None when no fab section exists
/// (no board requested); Err on a board too small to hold the parts.
pub fn place_board(
    items: &[TopLevel],
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    type_props: &BTreeMap<String, &HashMap<String, PropertyValue>>,
) -> Result<Option<BoardPlacement>, String> {
    let Some(fab) = items.iter().find_map(|it| match it {
        TopLevel::FabBlock(f) => Some(f),
        _ => None,
    }) else {
        return Ok(None);
    };
    let mut placed = apply_pinned(fab, instances)?;
    let (w, h) = fab.board;
    let decouplers = collect_decouplers(instances, type_props);
    let decoupled = first_decoupled(instances, type_props);

    // Collision-aware grid: every unplaced part flows to the first cell
    // whose footprint box fits the board AND overlaps no already-placed
    // part (pinned or earlier grid) — the auto-placer never manufactures
    // a clearance warning it could have avoided. Decouplers included, so
    // nothing is ever off-board.
    let mut placed_boxes = placed_boxes(&placed, instances);
    let mut grid_names: Vec<String> = instances
        .keys()
        .filter(|n| !placed.contains_key(*n))
        .cloned()
        .collect();
    grid_names.sort();
    grid_place_all(
        &mut placed,
        &mut placed_boxes,
        &grid_names,
        instances,
        fab.board,
    )?;

    // Decouplers near the decoupled part — a ring point is used only when
    // its footprint box fits the board AND overlaps no other placed part.
    if let Some((cx, cy)) = decoupled
        .as_ref()
        .and_then(|d| placed.get(d).map(|p| (p.x, p.y)))
    {
        decoupler_relocate(
            &mut placed,
            &mut placed_boxes,
            &decouplers,
            instances,
            RingTarget { cx, cy, w, h },
        );
    }

    Ok(Some(BoardPlacement {
        board: fab.board,
        parts: placed,
    }))
}

/// Place every unplaced part on the first collision-free grid cell —
/// one pass over the names consuming cells in order (deterministic).
fn grid_place_all(
    placed: &mut BTreeMap<String, PlacedPart>,
    placed_boxes: &mut BTreeMap<String, (f64, f64, f64, f64)>,
    grid_names: &[String],
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    board: (f64, f64),
) -> Result<(), String> {
    let cols = grid_cells(board.0);
    let max_cells = cols * grid_cells(board.1);
    let mut cell = 0usize;
    let mut placed_count = 0usize;
    while placed_count < grid_names.len() {
        if cell >= max_cells {
            return Err(format!(
                "the board {}mm x {}mm is too small for {} auto-placed parts at {}mm pitch — \
                 enlarge the board or pin positions with `place <inst> @ (...)`",
                board.0,
                board.1,
                grid_names.len(),
                PITCH
            ));
        }
        let name = &grid_names[placed_count];
        let (x, y) = cell_pos(cell, cols, board);
        let bb = part_box_at(instances, name, x, y);
        // Unknown footprint → place best-effort (the layout check reports
        // it as an error later); known footprint → must fit and clear.
        let free = match bb {
            None => true,
            Some(b) => fits_board(&b, board) && !overlaps_any(&b, placed_boxes, name),
        };
        if free {
            placed.insert(name.clone(), PlacedPart { x, y, rot: 0.0 });
            if let Some(b) = bb {
                placed_boxes.insert(name.clone(), b);
            }
            placed_count += 1;
        }
        cell += 1;
    }
    Ok(())
}

/// The number of cells along one board axis at the grid pitch.
fn grid_cells(axis: f64) -> usize {
    ((axis - 2.0 * EDGE_MARGIN) / PITCH).floor().max(1.0) as usize
}

/// The position of grid cell `cell` (row-major, from the top-left margin).
fn cell_pos(cell: usize, cols: usize, _board: (f64, f64)) -> (f64, f64) {
    let row = cell / cols;
    let col = cell % cols;
    (
        EDGE_MARGIN + col as f64 * PITCH,
        EDGE_MARGIN + row as f64 * PITCH,
    )
}

/// The axis-aligned bounding box of `name` centred at `(x, y)`, when its
/// footprint is known.
fn part_box_at(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    name: &str,
    x: f64,
    y: f64,
) -> Option<(f64, f64, f64, f64)> {
    let inst = instances.get(name)?;
    let package = inst
        .properties
        .iter()
        .find(|(k, _)| k == "package")
        .map(|(_, v)| v.as_str())?;
    let fp = crate::analysis::footprints::lookup(package)?;
    let (fw, fh) = fp.outline;
    Some((x - fw / 2.0, y - fh / 2.0, x + fw / 2.0, y + fh / 2.0))
}

/// The bounding boxes of every placed part (unknown footprints skipped —
/// the layout check reports them later).
fn placed_boxes(
    placed: &BTreeMap<String, PlacedPart>,
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
) -> BTreeMap<String, (f64, f64, f64, f64)> {
    let mut out = BTreeMap::new();
    for (name, part) in placed {
        if let Some(bb) = part_box_at(instances, name, part.x, part.y) {
            out.insert(name.clone(), bb);
        }
    }
    out
}

/// Whether `bb` overlaps any box in `boxes` (excluding `self_name`), with
/// at least MIN_CLEARANCE separation.
fn overlaps_any(
    bb: &(f64, f64, f64, f64),
    boxes: &BTreeMap<String, (f64, f64, f64, f64)>,
    self_name: &str,
) -> bool {
    boxes
        .iter()
        .any(|(name, other)| name != self_name && !aabb_clear(bb, other, MIN_CLEARANCE))
}

/// Move each decoupler to a ring point around the decoupled part, keeping
/// its grid slot when no ring point fits the board AND clears every other
/// placed part (the ring must never manufacture a clearance warning).
fn decoupler_relocate(
    placed: &mut BTreeMap<String, PlacedPart>,
    placed_boxes: &mut BTreeMap<String, (f64, f64, f64, f64)>,
    decouplers: &std::collections::BTreeSet<String>,
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    target: RingTarget,
) {
    for name in decouplers {
        let Some((fw, fh)) = footprint_box(instances, name) else {
            continue;
        };
        let Some((x, y)) = ring_fit_point(&target, fw, fh, placed_boxes, name) else {
            continue;
        };
        if let Some(part) = placed.get_mut(name) {
            part.x = x;
            part.y = y;
        }
        let bb = (x - fw / 2.0, y - fh / 2.0, x + fw / 2.0, y + fh / 2.0);
        placed_boxes.insert(name.clone(), bb);
    }
}

/// The decoupler ring geometry: ring centre (the decoupled part) + the
/// board bounds the ring points must stay inside.
#[derive(Clone, Copy)]
struct RingTarget {
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
}

/// The first ring point around the target whose footprint box fits the
/// board AND overlaps no placed part (except the decoupler itself) —
/// deterministic (8 points, first fit wins).
fn ring_fit_point(
    target: &RingTarget,
    fw: f64,
    fh: f64,
    placed_boxes: &BTreeMap<String, (f64, f64, f64, f64)>,
    self_name: &str,
) -> Option<(f64, f64)> {
    for k in 0..8 {
        let ang = k as f64 * (std::f64::consts::TAU / 8.0);
        let x = target.cx + DECOUPLER_RING * ang.cos();
        let y = target.cy + DECOUPLER_RING * ang.sin();
        let bb = (x - fw / 2.0, y - fh / 2.0, x + fw / 2.0, y + fh / 2.0);
        if fits_board(&bb, (target.w, target.h)) && !overlaps_any(&bb, placed_boxes, self_name) {
            return Some((x, y));
        }
    }
    None
}

/// The footprint outline of an instance (W, H mm), if known.
fn footprint_box(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    name: &str,
) -> Option<(f64, f64)> {
    let inst = instances.get(name)?;
    let package = inst
        .properties
        .iter()
        .find(|(k, _)| k == "package")
        .map(|(_, v)| v.as_str())?;
    crate::analysis::footprints::lookup(package).map(|fp| fp.outline)
}

/// Pinned placements from the fab section; unknown instances are an error.
fn apply_pinned(
    fab: &FabBlock,
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
) -> Result<BTreeMap<String, PlacedPart>, String> {
    let mut out = BTreeMap::new();
    for p in &fab.placements {
        if !instances.contains_key(&p.inst) {
            return Err(format!(
                "fab place '{}' names no declared instance",
                p.inst
            ));
        }
        out.insert(
            p.inst.clone(),
            PlacedPart {
                x: p.x,
                y: p.y,
                rot: p.rot,
            },
        );
    }
    Ok(out)
}

/// Instances whose type declares `spec Decoupler`.
fn collect_decouplers(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    type_props: &BTreeMap<String, &HashMap<String, PropertyValue>>,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (name, inst) in instances {
        if property_true(type_props, &inst.type_name, "decoupler") {
            out.insert(name.clone());
        }
    }
    out
}

/// The first instance whose type declares `spec Decouple` (sorted) — the
/// proximity anchor for decouplers. The key's PRESENCE is the declaration
/// (its value is a quantity, e.g. `spec Decouple: 100n`).
fn first_decoupled(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    type_props: &BTreeMap<String, &HashMap<String, PropertyValue>>,
) -> Option<String> {
    instances
        .iter()
        .find(|(_, inst)| {
            type_props
                .get(&inst.type_name)
                .map_or(false, |m| m.contains_key("decouple"))
        })
        .map(|(name, _)| name.clone())
}

fn property_true(
    type_props: &BTreeMap<String, &HashMap<String, PropertyValue>>,
    type_name: &str,
    key: &str,
) -> bool {
    type_props
        .get(type_name)
        .and_then(|m| m.get(key))
        .and_then(|v| match v {
            PropertyValue::Bool(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(name: &str, type_name: &str) -> (String, crate::analysis::electronics::ComponentInstance) {
        let inst = crate::analysis::electronics::ComponentInstance {
            name: name.into(),
            type_name: type_name.into(),
            properties: Vec::new(),
            specs: Default::default(),
        };
        (name.into(), inst)
    }

    fn fab_block(board: (f64, f64), placements: Vec<FabPlacement>) -> FabBlock {
        FabBlock {
            board,
            placements,
            span: None,
        }
    }

    #[test]
    fn pinned_wins_and_rest_grids_deterministically() {
        let items = vec![TopLevel::FabBlock(fab_block(
            (30.0, 20.0),
            vec![FabPlacement {
                inst: "u1".into(),
                x: 25.0,
                y: 5.0,
                rot: 90.0,
            }],
        ))];
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![instance("u1", "Chip"), instance("r1", "Res")].into_iter().collect();
        let bp = place_board(&items, &instances, &BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(bp.parts.len(), 2);
        // pinned wins
        let u1 = &bp.parts["u1"];
        assert_eq!((u1.x, u1.y, u1.rot), (25.0, 5.0, 90.0));
        // the rest flowed deterministically
        let r1 = &bp.parts["r1"];
        assert_eq!((r1.x, r1.y, r1.rot), (EDGE_MARGIN, EDGE_MARGIN, 0.0));
    }

    #[test]
    fn board_too_small_for_parts_is_an_error() {
        let items = vec![TopLevel::FabBlock(fab_block((10.0, 10.0), vec![]))];
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            (0..4).map(|i| instance(&format!("r{i}"), "Res")).into_iter().collect();
        let err = place_board(&items, &instances, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("too small"), "{err}");
    }

    #[test]
    fn no_fab_section_means_no_board() {
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![instance("r1", "Res")].into_iter().collect();
        let bp = place_board(&[], &instances, &BTreeMap::new()).unwrap();
        assert!(bp.is_none());
    }

    #[test]
    fn place_names_no_instance() {
        let items = vec![TopLevel::FabBlock(fab_block(
            (30.0, 20.0),
            vec![FabPlacement {
                inst: "ghost".into(),
                x: 1.0,
                y: 1.0,
                rot: 0.0,
            }],
        ))];
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![instance("r1", "Res")].into_iter().collect();
        let err = place_board(&items, &instances, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("ghost"), "{err}");
    }

    fn part_with_package(name: &str, package: &str) -> (String, crate::analysis::electronics::ComponentInstance) {
        let inst = crate::analysis::electronics::ComponentInstance {
            name: name.into(),
            type_name: "Res".into(),
            properties: vec![("package".into(), package.into())],
            specs: Default::default(),
        };
        (name.into(), inst)
    }

    #[test]
    fn containment_error_when_part_is_off_board() {
        let bp = BoardPlacement {
            board: (10.0, 10.0),
            parts: BTreeMap::from([
                ("r1".into(), PlacedPart { x: 20.0, y: 5.0, rot: 0.0 }),
            ]),
        };
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![part_with_package("r1", "0603")].into_iter().collect();
        let (errs, _warns) = check_layout(&bp, &instances);
        assert!(
            errs.iter().any(|e| e.contains("does not fit")),
            "{:?}",
            errs
        );
    }

    #[test]
    fn unknown_package_is_a_layout_error() {
        let bp = BoardPlacement {
            board: (10.0, 10.0),
            parts: BTreeMap::from([
                ("r1".into(), PlacedPart { x: 5.0, y: 5.0, rot: 0.0 }),
            ]),
        };
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![part_with_package("r1", "not-a-package")].into_iter().collect();
        let (errs, _) = check_layout(&bp, &instances);
        assert!(
            errs.iter().any(|e| e.contains("no footprint")),
            "{:?}",
            errs
        );
    }

    #[test]
    fn clearance_warning_when_parts_overlap() {
        let bp = BoardPlacement {
            board: (30.0, 20.0),
            parts: BTreeMap::from([
                ("r1".into(), PlacedPart { x: 5.0, y: 5.0, rot: 0.0 }),
                ("r2".into(), PlacedPart { x: 5.3, y: 5.0, rot: 0.0 }),
            ]),
        };
        let instances: BTreeMap<String, crate::analysis::electronics::ComponentInstance> =
            vec![part_with_package("r1", "0603"), part_with_package("r2", "0603")]
                .into_iter()
                .collect();
        let (errs, warns) = check_layout(&bp, &instances);
        assert!(errs.is_empty(), "{:?}", errs);
        assert!(
            warns.iter().any(|w| w.contains("closer than")),
            "{:?}",
            warns
        );
    }
}
