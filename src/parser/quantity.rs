// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Physical-quantity suffix parsing (2026-09-24 component-laws plan).
//!
//! One parser serves spec values and expression literals so the two surfaces
//! cannot drift. Compact legacy suffixes remain for existing programs; new
//! component-law physics uses the canonical full-word forms (`330Ohm`,
//! `4.7kOhm`, `0.7Volt`), never the resistance symbols `R`/`Ω`.

/// A parsed unit suffix: an explicit dimension, a bare SI prefix whose
/// dimension comes from the spec key, or an E-series fraction.
#[derive(Debug)]
pub(crate) enum UnitSuffix {
    /// `[prefix][base]` — explicit dimension (`2mA`, `3.3Volt`, `4.7kOhm`).
    Explicit { scale: f64, dim: crate::ast::QuantityDim },
    /// `[prefix]` bare — dimension from the key (`100n` on a Farad key).
    BarePrefix { scale: f64 },
    /// `[prefix][digits]` E-series fraction — `4k7` → 4.7k.
    Fraction { scale: f64, frac: f64 },
}

/// The base dimension behind a compact base-unit letter (`V` → Volt,
/// `R`/`Ω` → Ohm). `m` is absent: it is the milli prefix; bare metre is
/// handled by the full-word dispatcher.
fn compact_base_dim(c: char) -> Option<crate::ast::QuantityDim> {
    match c {
        'V' => Some(crate::ast::QuantityDim::Volt),
        'A' => Some(crate::ast::QuantityDim::Amp),
        'R' | 'Ω' => Some(crate::ast::QuantityDim::Ohm),
        'F' => Some(crate::ast::QuantityDim::Farad),
        'H' => Some(crate::ast::QuantityDim::Henry),
        'W' => Some(crate::ast::QuantityDim::Watt),
        'K' => Some(crate::ast::QuantityDim::Kelvin),
        _ => None,
    }
}

/// The scale of a case-sensitive SI prefix (`m` milli, `M` mega, `k` kilo,
/// `K` Kelvin compact base).
fn prefix_scale(c: char) -> Option<f64> {
    match c {
        'p' => Some(1e-12),
        'n' => Some(1e-9),
        'u' => Some(1e-6),
        'm' => Some(1e-3),
        'k' => Some(1e3),
        'M' => Some(1e6),
        'G' => Some(1e9),
        _ => None,
    }
}

/// The dimension behind a canonical full-word base (`Ohm`, `Volt`).
fn full_base_dim(s: &str) -> Option<crate::ast::QuantityDim> {
    match s {
        "Volt" => Some(crate::ast::QuantityDim::Volt),
        "Amp" => Some(crate::ast::QuantityDim::Amp),
        "Ohm" => Some(crate::ast::QuantityDim::Ohm),
        "Farad" => Some(crate::ast::QuantityDim::Farad),
        "Henry" => Some(crate::ast::QuantityDim::Henry),
        "Hertz" => Some(crate::ast::QuantityDim::Hertz),
        "Watt" => Some(crate::ast::QuantityDim::Watt),
        "Kelvin" => Some(crate::ast::QuantityDim::Kelvin),
        "Metre" | "Meter" => Some(crate::ast::QuantityDim::Length),
        _ => None,
    }
}

/// The display name of a quantity dimension, for diagnostics.
pub(crate) fn dimension_name(d: crate::ast::QuantityDim) -> &'static str {
    match d {
        crate::ast::QuantityDim::Volt => "volts",
        crate::ast::QuantityDim::Amp => "amps",
        crate::ast::QuantityDim::Ohm => "ohms",
        crate::ast::QuantityDim::Farad => "farads",
        crate::ast::QuantityDim::Henry => "henries",
        crate::ast::QuantityDim::Hertz => "hertz",
        crate::ast::QuantityDim::Watt => "watts",
        crate::ast::QuantityDim::Kelvin => "kelvin",
        crate::ast::QuantityDim::Length => "length",
    }
}

/// Parse a unit-suffix identifier. `mA` → Explicit(1e-3, Amp); `n` →
/// BarePrefix(1e-9); `k7` → Fraction(1e3, 0.7); `Ohm`/`kOhm` →
/// Explicit(1e0/1e3, Ohm). `R`/`Ω` remain legacy-only, not canonical new
/// physics. None when the string is not a unit suffix.
pub(crate) fn parse_unit_suffix(s: &str) -> Option<UnitSuffix> {
    if s == "Hz" {
        return Some(UnitSuffix::Explicit {
            scale: 1.0,
            dim: crate::ast::QuantityDim::Hertz,
        });
    }
    if let Some(dim) = full_base_dim(s) {
        return Some(UnitSuffix::Explicit { scale: 1.0, dim });
    }
    // Length spellings predate the full-word table: `m` is both the milli
    // prefix and the metre base, so `mm`/`cm` cannot flow through the
    // prefix dispatch.
    match s {
        "m" => {
            return Some(UnitSuffix::Explicit {
                scale: 1.0,
                dim: crate::ast::QuantityDim::Length,
            })
        }
        "mm" => {
            return Some(UnitSuffix::Explicit {
                scale: 1e-3,
                dim: crate::ast::QuantityDim::Length,
            })
        }
        "cm" => {
            return Some(UnitSuffix::Explicit {
                scale: 1e-2,
                dim: crate::ast::QuantityDim::Length,
            })
        }
        _ => {}
    }
    let first = s.chars().next()?;
    if let Some(dim) = compact_base_dim(first) {
        if s.len() != 1 {
            return None;
        }
        return Some(UnitSuffix::Explicit { scale: 1.0, dim });
    }
    let scale = prefix_scale(first)?;
    let rest = &s[1..];
    if rest.is_empty() {
        return Some(UnitSuffix::BarePrefix { scale });
    }
    if rest == "Hz" {
        return Some(UnitSuffix::Explicit {
            scale,
            dim: crate::ast::QuantityDim::Hertz,
        });
    }
    if let Some(dim) = full_base_dim(rest) {
        return Some(UnitSuffix::Explicit { scale, dim });
    }
    if let Some(dim) = compact_base_dim(rest.chars().next()?) {
        if rest.len() != 1 {
            return None;
        }
        return Some(UnitSuffix::Explicit { scale, dim });
    }
    if rest.chars().all(|c| c.is_ascii_digit()) {
        let frac = rest.parse::<f64>().ok()? / 10f64.powi(rest.len() as i32);
        return Some(UnitSuffix::Fraction { scale, frac });
    }
    None
}

/// Is this adjacent literal suffix a physical quantity? Used by the
/// expression parser to distinguish `3.3Volt` from `3x` tagged literals.
pub(crate) fn is_quantity_suffix(s: &str) -> bool {
    parse_unit_suffix(s).is_some()
}

/// Convert a numeric unit literal to SI under an expected dimension. A bare
/// numeric expression is not accepted: component physics states its unit.
pub(crate) fn quantity_si(
    value: f64,
    unit: &str,
    expected: crate::ast::QuantityDim,
) -> Option<(f64, crate::ast::QuantityDim)> {
    match parse_unit_suffix(unit) {
        Some(UnitSuffix::Explicit { scale, dim }) if dim == expected => {
            Some((value * scale, dim))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_unit_suffix, UnitSuffix};

    /// Canonical full-word units parse to SI with the correct dimension —
    /// the component-law surface spells resistance as `Ohm`, never `Ω`.
    #[test]
    fn canonical_full_word_units_resolve_to_si() {
        let expect = |suffix: &str, scale: f64, dim| {
            match parse_unit_suffix(suffix) {
                Some(UnitSuffix::Explicit { scale: got, dim: got_dim }) => {
                    assert_eq!(got_dim, dim, "suffix {suffix}");
                    assert!((got - scale).abs() < 1e-12, "suffix {suffix}: {got}");
                }
                other => panic!("suffix {suffix}: expected Explicit, got {other:?}"),
            }
        };
        expect("Ohm", 1.0, crate::ast::QuantityDim::Ohm);
        expect("kOhm", 1e3, crate::ast::QuantityDim::Ohm);
        expect("Volt", 1.0, crate::ast::QuantityDim::Volt);
        expect("mVolt", 1e-3, crate::ast::QuantityDim::Volt);
        expect("mAmp", 1e-3, crate::ast::QuantityDim::Amp);
        expect("nFarad", 1e-9, crate::ast::QuantityDim::Farad);
    }

    /// A non-unit identifier is not silently accepted as physics.
    #[test]
    fn non_unit_rejected() {
        assert!(parse_unit_suffix("Banana").is_none());
        assert!(parse_unit_suffix("kBanana").is_none());
    }
}
