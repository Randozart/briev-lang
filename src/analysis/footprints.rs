// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Footprint geometry library (2026-09-23 fab plan, `2026-09-23-ebv-fab-layer.md`).
//!
//! Loaded from `config/footprints.dbvl` — a Data Briev line table baked
//! via `include_str!`, the same pattern as `config_tuning::parse_ir_lowering`.
//! Pads are POSITIONAL in pin-number order; a footprint's pads never couple
//! to pin NAMES (Rules 14/15 — the compiler knows no vocabulary). All
//! geometry in mm, relative to the part origin (centre).
//!
//! The library is the board-reality half of the annotation doctrine: the
//! compiler carries it (never interprets it semantically beyond placement),
//! and a vendor-accurate geometry set is a DATA refinement, never a
//! compiler change.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A pad position/size in mm, relative to the part origin.
#[derive(Debug, Clone, Copy)]
pub struct Pad {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// A package footprint: pads in pin-number order + the part outline (W×H mm).
#[derive(Debug, Clone)]
pub struct Footprint {
    pub pads: Vec<Pad>,
    pub outline: (f64, f64),
}

/// The footprint library, loaded once from `config/footprints.dbvl`.
pub fn footprints() -> &'static HashMap<String, Footprint> {
    static CACHE: OnceLock<HashMap<String, Footprint>> = OnceLock::new();
    CACHE.get_or_init(|| parse_footprints(include_str!("../../config/footprints.dbvl")))
}

/// Look up a package's footprint; None when the package is unknown (the
/// fab layer turns that into a compile error at use).
pub fn lookup(package: &str) -> Option<&Footprint> {
    footprints().get(package)
}

/// Parse the `footprint.<id>: <pad_x>; <pad_y>; <pad_size>; … <w>; <h>;`
/// line table. Malformed geometry is a hard error — a bad library entry
/// must never silently place a part wrong.
fn parse_footprints(content: &str) -> HashMap<String, Footprint> {
    let mut out = HashMap::new();
    for (line_no, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let (id, nums) = parse_footprint_line(line_no, line);
        let fp = build_footprint(line_no, &id, &nums);
        out.insert(id, fp);
    }
    out
}

/// Split a footprint line into its id and numeric fields.
fn parse_footprint_line(line_no: usize, line: &str) -> (String, Vec<f64>) {
    let Some((head, fields)) = line.split_once(':') else {
        panic!("footprints.dbvl:{line_no}: expected 'footprint.<id>: <fields>;', got '{line}'");
    };
    let Some(id) = head.trim().strip_prefix("footprint.") else {
        panic!("footprints.dbvl:{line_no}: key must start 'footprint.', got '{head}'");
    };
    let mut nums = Vec::new();
    for field in fields.split(';') {
        let f = field.trim();
        if f.is_empty() {
            continue;
        }
        match f.parse::<f64>() {
            Ok(n) => nums.push(n),
            Err(_) => panic!(
                "footprints.dbvl:{line_no}: '{}' is not a number in '{}'",
                f, id
            ),
        }
    }
    (id.to_string(), nums)
}

/// Build a footprint from its numeric fields (pad triples + outline).
fn build_footprint(line_no: usize, id: &str, nums: &[f64]) -> Footprint {
    if nums.len() < 5 || (nums.len() - 2) % 3 != 0 {
        panic!(
            "footprints.dbvl:{line_no}: '{}' needs (pad_x, pad_y, pad_size) triples + outline w/h, got {} values",
            id,
            nums.len()
        );
    }
    let w = nums[nums.len() - 2];
    let h = nums[nums.len() - 1];
    let pads = nums[..nums.len() - 2]
        .chunks(3)
        .map(|c| Pad {
            x: c[0],
            y: c[1],
            size: c[2],
        })
        .collect();
    Footprint { pads, outline: (w, h) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprints_dbvl_parses_all_packages() {
        let lib = footprints();
        for pkg in ["0603", "0402", "0805", "SMD", "SOT-25", "QFN-32", "DFN-4", "HDR-2x5"] {
            assert!(lib.contains_key(pkg), "missing footprint for {pkg}");
        }
        let f = &lib["0603"];
        assert_eq!(f.pads.len(), 2);
        assert_eq!(f.outline, (1.6, 0.8));
        // pads are positional, symmetric around the origin
        assert!((f.pads[0].x + f.pads[1].x).abs() < 1e-9);
        assert!(f.pads[0].size > 0.0 && f.pads[0].size < 1.0);
    }

    #[test]
    fn footprint_pad_count_serves_the_largest_pin_part() {
        // QFN-32 serves the 15-pin Mcu; DFN-4 the 5-pin Sensor; the shared
        // "SMD" package serves both 2-pin UsbMicro and Switch.
        assert!(lookup("QFN-32").unwrap().pads.len() >= 15);
        assert!(lookup("DFN-4").unwrap().pads.len() >= 5);
        assert_eq!(lookup("SMD").unwrap().pads.len(), 2);
        assert!(lookup("HDR-2x5").unwrap().pads.len() >= 9);
    }

    #[test]
    fn unknown_package_is_none() {
        assert!(lookup("not-a-package").is_none());
    }
}