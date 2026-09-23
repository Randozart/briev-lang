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

/// Grid pitch (mm): clearance between adjacent auto-placed parts.
const PITCH: f64 = 10.0;
/// Margin from the board edge to the first part centre (mm).
const EDGE_MARGIN: f64 = 6.0;
/// Ring radius for decouplers placed near their decoupled part (mm).
const DECOUPLER_RING: f64 = 5.0;

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

    // Grid-place unplaced NON-decoupler parts first (board-aware).
    let mut grid_names: Vec<String> = instances
        .keys()
        .filter(|n| !placed.contains_key(*n) && !decouplers.contains(*n))
        .cloned()
        .collect();
    grid_names.sort();
    let grid = grid_positions(&grid_names, w, h)?;
    for (name, (x, y)) in grid_names.iter().zip(grid) {
        placed.insert(name.clone(), PlacedPart { x, y, rot: 0.0 });
    }

    // Decouplers near the decoupled part (or the grid when none exists).
    let mut ring_offset = 0usize;
    for name in decouplers {
        if placed.contains_key(&name) {
            continue;
        }
        let (cx, cy) = match decoupled {
            Some(ref d) => placed.get(d).map(|p| (p.x, p.y)).unwrap_or((0.0, 0.0)),
            None => (0.0, 0.0),
        };
        let ang = ring_offset as f64 * (std::f64::consts::TAU / 8.0);
        let x = cx + DECOUPLER_RING * ang.cos();
        let y = cy + DECOUPLER_RING * ang.sin();
        ring_offset += 1;
        placed.insert(name, PlacedPart { x, y, rot: 0.0 });
    }

    Ok(Some(BoardPlacement { board: fab.board, parts: placed }))
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
/// proximity anchor for decouplers.
fn first_decoupled(
    instances: &BTreeMap<String, crate::analysis::electronics::ComponentInstance>,
    type_props: &BTreeMap<String, &HashMap<String, PropertyValue>>,
) -> Option<String> {
    instances
        .iter()
        .find(|(_, inst)| property_true(type_props, &inst.type_name, "decouple"))
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

/// Row-major grid positions for `names` inside the board outline (mm).
fn grid_positions(names: &[String], board_w: f64, board_h: f64) -> Result<Vec<(f64, f64)>, String> {
    let cols = ((board_w - 2.0 * EDGE_MARGIN) / PITCH).floor().max(1.0) as usize;
    let rows = (names.len() + cols - 1) / cols;
    let needed_h = EDGE_MARGIN + rows as f64 * PITCH;
    if needed_h > board_h + f64::EPSILON {
        return Err(format!(
            "the board {}mm x {}mm is too small for {} auto-placed parts at {}mm pitch (needs ~{}mm) — \
             enlarge the board or pin positions with `place <inst> @ (...)`",
            board_w,
            board_h,
            names.len(),
            PITCH,
            needed_h as i64
        ));
    }
    Ok(names
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let row = i / cols;
            let col = i % cols;
            (
                EDGE_MARGIN + col as f64 * PITCH,
                EDGE_MARGIN + row as f64 * PITCH,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(name: &str, type_name: &str) -> (String, crate::analysis::electronics::ComponentInstance) {
        let inst = crate::analysis::electronics::ComponentInstance {
            name: name.into(),
            type_name: type_name.into(),
            properties: Vec::new(),
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
}