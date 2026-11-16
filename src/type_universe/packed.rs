// ── Packed Struct Layout Derivation ───────────────────────────────────
// 2026-08-13 (layout-keywords plan, Phase 2): bit-contiguous field offsets
// for `pack struct`. This is the SINGLE authority for packed layout — a pure
// function of the declared fields + declared endian (SPEC §2.5). Consumed by
// LLVM codegen (field read/write, type emission, sizes) and the interpreter
// cast path. The type itself does NOT store the derived layout (types hold
// protocol + declared metadata only; see SPEC §2.1 / Boxed Cat Typing).

use crate::ast::Type;
use crate::type_universe::TypeUniverse;

/// One packed field's slice into the struct's byte image.
///
/// Extraction (endian-invariant): load `cov` consecutive bytes starting at
/// `byte`, interpret them as a little-endian integer (LE) or big-endian
/// integer (BE), right-shift by `shift`, mask to `bits`.
///
///   LE: value = (loadN_le(byte, cov) >> shift) & mask   (shift = within)
///   BE: value = (loadN_be(byte, cov) >> shift) & mask   (shift = cov*8 - within - bits)
/// where `within` = the field's bit position inside `byte` measured from that
/// byte's LSB (LE) or MSB (BE) — see `packed_field_offsets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedField {
    pub name: String,
    /// Bit width of the field.
    pub bits: u64,
    /// Byte index where the covering region starts (address order).
    pub byte: u64,
    /// Number of covering bytes read for this field (ceil((within+bits)/8)).
    pub cov: u64,
    /// Right-shift applied to the loading (after endian interpretation).
    pub shift: u8,
    /// Endian variant used to derive this slice.
    pub endian: EndianKind,
}

/// 2026-08-13: declared bit order (§2.5). `Target` collapses to the native
/// (little/whole-byte-compatible) convention — never a surprise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndianKind {
    /// Native byte order / bit convention — behaves like today's whole-byte
    /// structs (LSB-first within each byte); also the interpretation of
    /// `Target`.
    Little,
    /// MSB-first within each byte; multi-byte fields big-endian.
    Big,
}

/// Map a `spec Endian` metadata value (Identifier "Big"/"Little"/"Target",
/// or absent) to an `EndianKind`. The parser already rejects unknown values;
/// anything unrecognized here defaults to native.
pub fn endian_from_spec(value: Option<&str>) -> EndianKind {
    match value {
        Some("Big") => EndianKind::Big,
        Some("Little") => EndianKind::Little,
        // `Target` or absent → native. A packed struct with no declared
        // endian is byte-for-byte what the target would lay out anyway.
        _ => EndianKind::Little,
    }
}

/// The exact bit width of a `Bits<N>` type, in either of its two AST forms
/// (`Type::Bits(n)` and the `Applied("Bits", [Number(n)])` alias produced by
/// the parser). None for anything else.
pub fn bits_width(ty: &Type) -> Option<u64> {
    match ty {
        Type::Bits(n) => Some(*n),
        Type::Applied(name, args) if name == "Bits" => match args.first() {
            Some(Type::Number(n)) => Some((*n).max(0) as u64),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve one field's bit width. `Bits(n)` is exact; anything else resolves
/// through the universe (fallback: the target word, 64 bits). A packed field
/// is expected to be exact-width (`Bits<N>`); a flexible width resolves to
/// the word so bit-contiguity does not silently shift.
pub fn field_bits(ty: &Type, universe: Option<&TypeUniverse>) -> u64 {
    match ty {
        Type::Bits(n) => *n,
        // 2026-08-13: `Bits<48>` parses as Applied("Bits", [Number(48)]) — the
        // exact-width alias — resolve it here so packed layout never sees the
        // generic-application default (64).
        Type::Applied(name, args) if name == "Bits" => match args.first() {
            Some(Type::Number(n)) => (*n).max(0) as u64,
            _ => 64,
        },
        _ => universe
            .and_then(|u| crate::type_universe::resolve_type(u, ty))
            .map(|b| b.max_bits)
            .unwrap_or(64),
    }
}

/// Derive the packed slice for every field, in declaration order.
///
/// Bits lay out bit-contiguously from the struct start; bit position p lands
/// in byte p/8. Little: the field's low bit is at bit p%8 (LSB-numbered) of
/// that byte. Big (MSB-first): the field's HIGH bit is at bit (7 − p%8) of
/// byte p/8 — the first byte of a big-endian packing holds the field's most
/// significant bits, matching C bitfield reality (GCC/MSVC pack LSB-first on
/// little-endian; ARM big-endian packs MSB-first).
///
/// Whole-byte fields (bits % 8 == 0) get byte-aligned zero-shift slices under
/// BOTH conventions — the default is identical to a whole-byte struct.
pub fn packed_field_offsets(
    fields: &[(String, Type)],
    endian: Option<&str>,
    universe: Option<&TypeUniverse>,
) -> Vec<PackedField> {
    let kind = endian_from_spec(endian);
    let mut result = Vec::with_capacity(fields.len());
    let mut p: u64 = 0;
    for (name, ty) in fields {
        let bits = field_bits(ty, universe);
        let byte = p / 8;
        let within = (p % 8) as u8;
        let (cov, shift) = match kind {
            EndianKind::Little => {
                let cov = ((within as u64 + bits).div_ceil(8)).max(1);
                (cov, within)
            }
            EndianKind::Big => {
                // The field's MSB sits `within` bits below the covered region's
                // MSB (bit stream position byte*8 + within). Interpreting the
                // covered bytes as a big-endian integer, stream bit s lands at
                // integer bit (cov*8 − 1 − (s − byte*8)), so the field occupies
                // the TOP of the region starting at integer bit
                // cov*8 − 1 − within. Extracting it is a right-shift of
                // (cov*8 − within − bits) — NOT cov*8 − bits, which only holds
                // when within == 0 (2026-08-13: corrected — sub-byte BE fields
                // in the low nibble of their byte shifted off their own bits).
                let cov = ((within as u64 + bits).div_ceil(8)).max(1);
                let shift = (cov as u8) * 8 - within - bits as u8;
                (cov, shift)
            }
        };
        result.push(PackedField {
            name: name.clone(),
            bits,
            byte,
            cov,
            shift,
            endian: kind,
        });
        p += bits;
    }
    result
}

/// Total packed volume in bits (Σ field widths). Endian-independent.
pub fn packed_total_bits(fields: &[(String, Type)], universe: Option<&TypeUniverse>) -> u64 {
    fields.iter().map(|(_, t)| field_bits(t, universe)).sum()
}

/// Storage bytes for a packed struct: ceil(total_bits / 8) — zero padding.
pub fn packed_bytes(fields: &[(String, Type)], universe: Option<&TypeUniverse>) -> u64 {
    packed_total_bits(fields, universe).div_ceil(8)
}

/// True when every field of the packed struct is whole-byte (bits % 8 == 0),
/// i.e. the struct can use a native `<{ ... }>` LLVM packed type with GEP.
pub fn is_whole_byte_packed(fields: &[(String, Type)], universe: Option<&TypeUniverse>) -> bool {
    fields.iter().all(|(_, t)| field_bits(t, universe) % 8 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, bits: u64) -> (String, Type) {
        (name.to_string(), Type::Bits(bits))
    }

    fn eth() -> Vec<(String, Type)> {
        vec![f("dst_mac", 48), f("src_mac", 48), f("ethertype", 16)]
    }

    #[test]
    fn whole_byte_offsets_are_endian_neutral() {
        // 48/48/16: byte-aligned slices under BOTH conventions.
        let le = packed_field_offsets(&eth(), Some("Little"), None);
        let be = packed_field_offsets(&eth(), Some("Big"), None);
        let expected = |i: usize, byte: u64, cov: u64| {
            assert_eq!(le[i].byte, byte, "le byte");
            assert_eq!(le[i].cov, cov, "le cov");
            assert_eq!(le[i].shift, 0, "le shift");
            assert_eq!(be[i].byte, byte, "be byte");
            assert_eq!(be[i].cov, cov, "be cov");
            assert_eq!(be[i].shift, 0, "be shift");
        };
        expected(0, 0, 6);
        expected(1, 6, 6);
        expected(2, 12, 2);
        assert!(is_whole_byte_packed(&eth(), None));
        assert_eq!(packed_bytes(&eth(), None), 14);
    }

    #[test]
    fn sub_byte_little_lsb_first() {
        let fields = vec![f("a", 12), f("b", 4), f("c", 8)];
        let offs = packed_field_offsets(&fields, Some("Little"), None);
        assert_eq!(
            offs[0],
            PackedField {
                name: "a".into(),
                bits: 12,
                byte: 0,
                cov: 2,
                shift: 0,
                endian: EndianKind::Little,
            }
        );
        assert_eq!(
            offs[1],
            PackedField {
                name: "b".into(),
                bits: 4,
                byte: 1,
                cov: 1,
                shift: 4,
                endian: EndianKind::Little,
            }
        );
        assert_eq!(
            offs[2],
            PackedField {
                name: "c".into(),
                bits: 8,
                byte: 2,
                cov: 1,
                shift: 0,
                endian: EndianKind::Little,
            }
        );
        assert!(!is_whole_byte_packed(&fields, None));
        assert_eq!(packed_bytes(&fields, None), 3);
    }

    #[test]
    fn sub_byte_big_msb_first() {
        let fields = vec![f("a", 12), f("b", 4), f("c", 8)];
        let offs = packed_field_offsets(&fields, Some("Big"), None);
        // a: bits 0..11 → byte0 whole + upper nibble of byte1. BE integer of
        // bytes {b0,b1} = b0<<8|b1; a occupies the top → shift 16−0−12 = 4.
        assert_eq!(
            offs[0],
            PackedField {
                name: "a".into(),
                bits: 12,
                byte: 0,
                cov: 2,
                shift: 4,
                endian: EndianKind::Big,
            }
        );
        // b: bits 12..15 → LOWER nibble of byte1 (a's low nibble holds the
        // upper four bits). Single-byte BE integer = the byte; b is its low
        // nibble → shift 8−4−4 = 0.
        assert_eq!(
            offs[1],
            PackedField {
                name: "b".into(),
                bits: 4,
                byte: 1,
                cov: 1,
                shift: 0,
                endian: EndianKind::Big,
            }
        );
        // c: whole byte at b2.
        assert_eq!(offs[2].byte, 2);
        assert_eq!(offs[2].cov, 1);
        assert_eq!(offs[2].shift, 0);
    }

    #[test]
    fn default_endian_is_native_little() {
        let offs = packed_field_offsets(&eth(), None, None);
        assert_eq!(offs[0].endian, EndianKind::Little);
        // "Target" is native too.
        let target = packed_field_offsets(&eth(), Some("Target"), None);
        assert_eq!(target[0].endian, EndianKind::Little);
    }

    #[test]
    fn packed_bytes_ceil() {
        assert_eq!(packed_bytes(&eth(), None), 14);
        assert_eq!(packed_bytes(&vec![f("x", 4), f("y", 4)], None), 1);
        assert_eq!(
            packed_bytes(&vec![f("x", 0)], None),
            0,
            "zero-width stays empty"
        );
    }
}
