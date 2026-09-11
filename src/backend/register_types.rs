// ── Shared Type Registration — Backend-Agnostic Universe Population ────
//
// 2026-08-10: Extracted from llvm/normalizer.rs so every backend normalizer
// registers user TypeDefs into the TypeUniverse uniformly. The casting graph
// resolves each backend's native type from (protocol, metadata) at codegen
// time — no per-backend llvm_type/js_type/bit_width baking here.
//
// Backends that derive a richer native type (LLVM IR types) do that
// separately in their own normalizer AFTER calling register_typedefs.

use crate::ast::*;
use crate::ast::PropertyValue;
use crate::type_universe::ResolvedType;
use crate::type_universe::TypeUniverse;

/// 2026-08-13 (layout-keywords plan): resolve a C-compatible `struct Name`
/// into a ResolvedType using its declared `spec` layout. THE single authority
/// for static-struct sizing — llvm/mod.rs StaticStruct registration calls this
/// instead of re-deriving bytes/alignment inline (rule 16: one precedence
/// table). §2.1 precedence: Bytes > Bits > slot-sum; alignment defaults to 8.
/// 2026-08-13 (layout-keywords plan Phase 6): a union's storage layout — the
/// largest aligned field storage (bytes) and the maximum field alignment.
/// All fields overlay at offset 0.
fn union_field_storage(fields: &[(String, crate::ast::Type)], universe: &TypeUniverse) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut align = 1u64;
    for (_, ty) in fields {
        let sz = crate::backend::llvm::types::type_size(ty, Some(universe));
        bytes = bytes.max(sz);
        align = align.max(sz.min(8));
    }
    (bytes, align)
}

pub fn static_struct_resolved_ty(
    def: &crate::ast::top::StructDef,
    universe: &TypeUniverse,
) -> ResolvedType {
    let fields: Vec<(String, Type)> = def.fields.iter()
        .map(|(n, t)| (n.clone(), t.clone()))
        .collect();
    let int_md = |key: &str| -> Option<u64> {
        def.metadata.get(key).and_then(|pv| match pv {
            PropertyValue::Int(n) if *n >= 0 => Some(*n as u64),
            _ => None,
        })
    };
    let declared_bits = int_md("bits");
    // 2026-08-13 (layout-keywords plan): `pack struct` size is bit-granular —
    // Σ field widths (zero padding), endian-independent. An explicit `spec
    // Bytes` still wins (§2.1 precedence), otherwise the packed volume rounds
    // up (div_ceil) to storage bytes.
    let packed_bits = if def.pack {
        Some(crate::type_universe::packed_total_bits(&fields, Some(universe)))
    } else {
        None
    };
    // 2026-08-13 (layout-keywords plan Phase 6): a union's fields overlay at
    // offset 0 — size is the largest aligned field storage, alignment the max
    // field alignment.
    let union_layout = if def.union {
        Some(union_field_storage(&fields, universe))
    } else {
        None
    };
    let slot_sum: u64 = fields.iter().map(|(_, ty)| {
        crate::backend::llvm::types::type_size(ty, Some(universe))
    }).sum();
    // §2.1 precedence: spec Bytes > (pack volume | declared Bits) > union
    // storage > slot sum.
    let declared_bytes = if def.pack {
        packed_bits.map(|b| b.div_ceil(8))
    } else {
        declared_bits.map(|b| b.div_ceil(8))
    };
    let bytes = int_md("bytes")
        .or(declared_bytes)
        .or(union_layout.map(|(ubytes, _)| ubytes))
        .unwrap_or(slot_sum);
    let max_bits = declared_bits.or(packed_bits).unwrap_or(bytes * 8);
    let default_align = match (def.pack, union_layout) {
        // Packed layouts are bit-contiguous: no inter-element padding, so the
        // default alignment is 1 (a `spec Alignment` declaration overrides).
        // A union's alignment is the max field alignment (the overlay storage
        // must satisfy every field).
        (true, _) => 1,
        (false, Some((_, ualign))) => ualign,
        (false, None) => 8,
    };
    ResolvedType {
        name: def.name.clone(),
        base: "Data".to_string(),
        bytes,
        min_bits: max_bits,
        max_bits,
        alignment: int_md("alignment").unwrap_or(default_align),
        // All `spec` keys (incl. endian) surface via reflection (`.^^`).
        properties: def.metadata.clone(),
        fields,
    }
}

/// 2026-07-16: Register all TopLevel::TypeDef items into the TypeUniverse.
/// Extracts byte size from layout metadata or slots, attaches field annotations,
/// and registers each type so later validation can look it up.
///
/// 2026-08-10: Extracted from llvm/normalizer.rs — shared by all backends.
/// int_bits is the TARGET default width used when a type has no width metadata
/// and no primordial entry (Phase 3, §8.6).
pub fn register_typedefs(items: &[TopLevel], universe: &mut TypeUniverse, int_bits: u64) -> Result<(), String> {
    for item in items {
        let td = match item {
            TopLevel::TypeDef(td) => td,
            _ => continue,
        };
        // 2026-07-30: Bit is the axiomatic anchor — never overrideable.
        // A type declaration for Bit in user code or stdlib is an error.
        if td.name == "Bit" {
            return Err("'Bit' is a compiler primitive and cannot be redeclared".to_string());
        }
        // 2026-08-26 (Track B): ENUM TypeDefs (all slots `__variant_*`) are
        // tagged sums whose runtime image is the boxed {tag, payload} pair
        // behind a one-word handle. Register a HANDLE shape: no fields,
        // one word of storage — variant slots must NOT become struct fields
        // (they emitted invalid `{ i64, void }` aggregates and wrong
        // by-value layouts). To undo: remove this branch.
        if !td.body.slots.is_empty()
            && td.body.slots.iter().all(|s| s.name.starts_with("__variant_"))
        {
            let word = std::mem::size_of::<usize>() as u64;
            let rt = crate::type_universe::ResolvedType {
                name: td.name.clone(),
                base: td.name.clone(),
                bytes: word,
                min_bits: 64,
                max_bits: 64,
                alignment: word,
                properties: [("Cast.Int".to_string(), crate::ast::PropertyValue::Int(1))]
                    .into_iter()
                    .collect(),
                fields: Vec::new(),
            };
            universe.types.insert(td.name.clone(), rt);
            continue;
        }
        // 2026-07-26: Read bits/maxbits/minbits metadata independently.
        // bits <~ N → exact (min=max=N), maxbits <~ N → ceiling (min=0, max=N),
        // minbits <~ N → floor (min=N, max=primordial). Fallback: primordial values.
        let iv = |key: &str| -> Option<u64> {
            td.body.metadata.get(key).and_then(|pv| {
                if let PropertyValue::Int(n) = pv { Some(*n as u64) } else { None }
            })
        };
        let exact_bits = iv("bits");
        let ceiling = iv("maxbits");
        let floor = iv("minbits");
        // 2026-08-13 (layout-keywords plan): `spec Bytes: N` is the AUTHORITATIVE
        // storage size (§2.1 precedence: Bytes > Bits > slot-sum > primordial).
        // All bits metadata here are BIT counts (spec Bits/!> bits); the byte
        // conversion rounds UP via div_ceil so sub-byte widths keep a byte.
        let bytes_override = iv("bytes");
        // 2026-07-31: Phase 3 (§8.6) — clone the primordial so the warning
        // closures below can borrow `universe.warnings` mutably without
        // conflicting with an outstanding immutable borrow of the universe.
        let primordial = universe.get(&td.name).cloned();
        // 2026-07-31: Phase 3 (§8.6) — a type with no primordial falls back to
        // the TARGET int width (int_bits), not a hardcoded 64; a diagnostic is
        // recorded so the fallback is never silent.
        let prim_max = primordial.as_ref().map(|p| p.max_bits).unwrap_or_else(|| {
            universe.warnings.push(format!(
                "normalizer: type '{}' has no primordial entry and no `!> bits` \
                 metadata — defaulting max width to target int width ({})",
                td.name, int_bits
            ));
            int_bits
        });
        let prim_min = primordial.as_ref().map(|p| p.min_bits).unwrap_or(0);
        let (min_bits, max_bits) = if let Some(bits) = exact_bits {
            (bits, bits)
        } else {
            let f = floor.unwrap_or(prim_min);
            let c = ceiling.unwrap_or(prim_max);
            (f.min(c), c.max(f))
        };
        let bytes = bytes_override
            .or_else(|| ceiling.map(|b| b.div_ceil(8)))
            .or_else(|| exact_bits.map(|b| b.div_ceil(8)))
            .or_else(|| {
                if td.body.slots.is_empty() { return None; }
                // 2026-08-04 (compiler-in-Briev): sum slot sizes via
                // type_size (NOT raw rt.bytes). Flexible primordials (Int,
                // String, ...) register bytes=0 ("not yet resolved"); reading
                // rt.bytes directly collapsed `ListBuffer<T> { data: Ptr<T>,
                // cap: Int }` to 8 bytes (Ptr=8 + Int=0) and List<T>.len
                // collided with inner.cap at offset 8. type_size resolves the
                // flexible protocols (Cast.Int → 8, Cast.String → 8) and
                // fixes Ptr at one word.
                // 2026-08-15 (coll plan §3.3): a `coll obj` adds two hidden
                // slots (`cap` + `len`); the total includes them.
                let mut total: u64 = td.body.slots.iter().map(|slot| {
                    crate::backend::llvm::types::type_size(&slot.ty, Some(universe))
                }).sum();
                if td.coll {
                    total += 2 * crate::backend::llvm::types::type_size(&Type::int(), Some(universe));
                }
                Some(total)
            })
            .unwrap_or_else(|| primordial.as_ref().map(|p| p.bytes).unwrap_or_else(|| {
                // 2026-07-31: Phase 3 (§8.6) — no metadata-derived size and no
                // primordial: assume 8 bytes (conservative max scalar) and record
                // the fallback so it is not silent.
                universe.warnings.push(format!(
                    "normalizer: type '{}' has no size metadata and no primordial \
                     entry — assuming 8 bytes",
                    td.name
                ));
                8
            }));
        let alignment = td.body.metadata.get("alignment")
            .and_then(|pv| {
                if let PropertyValue::Int(n) = pv { Some(*n as u64) } else { None }
            })
            .unwrap_or_else(|| primordial.as_ref().map(|p| p.alignment).unwrap_or_else(|| {
                // 2026-07-31: Phase 3 (§8.6) — conservative alignment fallback
                // (min(bytes, 8)); recorded so it is not silent.
                universe.warnings.push(format!(
                    "normalizer: type '{}' has no alignment metadata and no \
                     primordial entry — assuming alignment {}",
                    td.name, bytes.min(8)
                ));
                bytes.min(8)
            }));
        let mut properties: std::collections::HashMap<String, PropertyValue> = td.body.metadata.clone();
        // 2026-08-13 (layout-keywords plan): `spec` keys — including
        // `endian` (Big/Little/Target) — share the metadata map with `!>`,
        // so they surface on `ResolvedType.properties` verbatim and are
        // queryable from Briev via reflection (`.^^`). Phase 2 pack emission
        // reads `endian` here (absent ⇒ Target/native).
        // 2026-08-04 (compiler-in-Briev): when re-registering a primordial (e.g.
        // `type Int: Int { ... }` in bootstrap.bv), inherit the primordial's
        // protocol Cast.* properties. The flexible-protocol fallbacks in
        // type_size (types.rs) key on Cast.Int/String/Float/Bool — without
        // them, `type Int: Int` (empty metadata) registers bytes=0 and
        // `type_size(Int)` returns 0, collapsing any struct containing an Int
        // slot (ListBuffer.cap → 0 → List<T>.len collides with inner.cap).
        if let Some(prim) = &primordial {
            for (k, v) in &prim.properties {
                if k.starts_with("Cast.") {
                    properties.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        // 2026-08-03: the declared protocol hashword is the base when there is
        // no parent type — `type CStr: #String<C_String>` must register base
        // "#String<C_String>" (not "Bit") so type_to_protocol resolves it to
        // (String, C_String) and the casting graph derives its ABI (ptr).
        // 2026-09-11 (fundamentals doctrine, B1): a type with NO parent and
        // NO protocol SELF-ROOTS — base = its own name. A parentless declared
        // type IS its own category root (`type Volt { }` is the Volt
        // category); the old `Bit` default silently classified every plain
        // struct as a bit type. Consumers comparing bases use the walk, which
        // now treats a self-base as the root.
        let base = td.parent.as_ref()
            .and_then(|e| match e.as_ref() { Expr::Identifier(n) => Some(n.clone()), _ => None })
            .or_else(|| td.protocol.clone())
            .unwrap_or_else(|| td.name.clone());
        let mut fields: Vec<(String, Type)> = td.body.slots.iter()
            .map(|s| (s.name.clone(), s.ty.clone()))
            .collect();
        // 2026-08-15 (coll plan §3.3): a `coll obj` appends two hidden trailing
        // slots — `cap` then `len` (compiler-owned capacity + length). For
        // `List` (sequence member `inner.data: Ptr<T>`) this reproduces the
        // canonical `[inner.data, cap, len]` layout byte-for-byte. The hidden
        // slots are part of the ResolvedType fields so the IR type and the
        // universe's `bytes` both include them.
        if td.coll {
            fields.push(("cap".to_string(), Type::int()));
            fields.push(("len".to_string(), Type::int()));
        }
        let mut rt = ResolvedType {
            name: td.name.clone(),
            base,
            bytes,
            min_bits,
            max_bits,
            alignment,
            properties,
            fields,
        };

        // 2026-07-20: No CTD/ALU/encoding inheritance.
        // Hashword op signatures replace these properties entirely.

        universe.register(rt);
    }

    // 2026-07-30: Cast. properties are no longer injected by the normalizer.
    // Protocol membership is determined by the casting graph via type_to_protocol()
    // and is_protocol_member(). The casting graph hardcodes base protocol lanes
    // and receives proto declarations via register_protocol_def().
    //
    // Cast. properties from primordial seeding are retained for backward compat
    // during migration. They will be removed in a future cleanup pass after
    // is_protocol_member() fully transitions to the casting graph.
    Ok(())
}

/// 2026-08-26 (bug sweep B2, SPEC §2.1): the structural layout fallback used
/// by backend backfill sites (`TopLevel::Obj` and type-agnostic TypeDef
/// re-registration inside LlvmBackend::generate). Slot-sum bytes, alignment
/// 8 — the SAME structural defaults static_struct_resolved_ty applies — but
/// the fallback is RECORDED in universe.warnings: a representation no source
/// declaration pins is visible to the user, never silent. Deduped per name.
pub fn record_structural_layout(
    universe: &mut TypeUniverse,
    name: &str,
    base: &str,
    fields: &[(String, Type)],
) -> ResolvedType {
    let bytes: u64 = fields.iter().map(|(_, ty)| {
        crate::backend::llvm::types::type_size(ty, Some(universe))
    }).sum();
    let marker = format!("structural layout fallback for '{name}'");
    if !universe.warnings.iter().any(|w| w.contains(&marker)) {
        universe.warnings.push(format!(
            "warning: {marker} — slot-sum bytes ({bytes}), alignment 8. \
             Declare !> bytes/alignment on '{name}' to pin its representation."
        ));
    }
    ResolvedType {
        name: name.to_string(),
        base: base.to_string(),
        bytes,
        min_bits: bytes * 8,
        max_bits: bytes * 8,
        alignment: 8,
        properties: std::collections::HashMap::new(),
        fields: fields.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
    use std::collections::HashMap;
    use super::*;

    fn make_type_def(name: &str, slots: Vec<(&str, Type)>) -> TopLevel {
        make_type_def_full(name, slots, vec![])
    }

    fn make_type_def_full(
        name: &str,
        slots: Vec<(&str, Type)>,
        meta: Vec<(&str, i64)>,
    ) -> TopLevel {
        let mut metadata = HashMap::new();
        for (k, v) in meta {
            metadata.insert(k.to_string(), crate::ast::PropertyValue::Int(v));
        }
        TopLevel::TypeDef(Box::new(TypeDef {
            name: name.to_string(),
            type_params: vec![],
            parent: None,
            protocol: Some("Bit".to_string()),
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
            body: TypeDefBody {
                pins: Vec::new(),
                reference: None,
                tolerance: None,
                slots: slots.into_iter().map(|(n, ty)| TypeDefSlot {
                    name: n.to_string(), ty, bit_range: None,
                }).collect(),
                metadata,
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        }))
    }

    #[test]
    fn test_register_typedefs_bit_is_forbidden() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def("Bit", vec![])];
        assert!(register_typedefs(&items, &mut u, 64).is_err());
    }

    #[test]
    fn test_register_typedefs_registers_custom_struct() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def(
            "Point",
            vec![("x", Type::int()), ("y", Type::int())],
        )];
        register_typedefs(&items, &mut u, 64).unwrap();
        let rt = u.get("Point").expect("registered");
        assert_eq!(rt.bytes, 16);
        // 2026-09-11 (Phase A5): the declared protocol becomes the base in its
        // BARE spelling — category hashword spellings are retiring.
        assert_eq!(rt.base, "Bit");
    }

    fn make_type_def_meta(name: &str, meta: Vec<(&str, i64)>) -> TopLevel {
        make_type_def_full(name, vec![], meta)
    }

    // ── Phase 1 (layout-keywords): spec-driven size/alignment ─────────

    #[test]
    fn test_register_typedefs_spec_bits_sets_bytes_ceil() {
        // `spec Bits: 4` → 4 bits → 1 storage byte (div_ceil). Sub-byte widths
        // keep a byte; the old floor division (`bits / 8`) gave 0 bytes.
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def_meta("Nibble", vec![("bits", 4)])];
        register_typedefs(&items, &mut u, 64).unwrap();
        let rt = u.get("Nibble").expect("registered");
        assert_eq!(rt.bytes, 1);
        assert_eq!(rt.min_bits, 4);
        assert_eq!(rt.max_bits, 4);
        assert_eq!(rt.alignment, 1);
    }

    #[test]
    fn test_register_typedefs_spec_bytes_is_authoritative() {
        // §2.1 precedence: Bytes > Bits. `spec Bytes: 4` overrides bits.
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def_meta("Frame", vec![("bytes", 4), ("bits", 12)])];
        register_typedefs(&items, &mut u, 64).unwrap();
        let rt = u.get("Frame").expect("registered");
        assert_eq!(rt.bytes, 4);
        assert_eq!(rt.min_bits, 12);
        assert_eq!(rt.max_bits, 12);
    }

    #[test]
    fn test_register_typedefs_spec_alignment() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def_meta("Packed", vec![("alignment", 1)])];
        register_typedefs(&items, &mut u, 64).unwrap();
        assert_eq!(u.get("Packed").expect("registered").alignment, 1);
    }

    #[test]
    fn test_register_typedefs_byte_aligned_bits_unchanged() {
        // `spec Bits: 64` → 8 bytes, identical to the `!> bits: 64` behavior.
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def_meta("Wordish", vec![("bits", 64)])];
        register_typedefs(&items, &mut u, 64).unwrap();
        let rt = u.get("Wordish").expect("registered");
        assert_eq!(rt.bytes, 8);
        assert_eq!(rt.max_bits, 64);
    }

    // ── StaticStruct: spec sizing via static_struct_resolved_ty ───────

    fn make_struct_def(name: &str, fields: Vec<(&str, Type)>, meta: Vec<(&str, i64)>) -> crate::ast::top::StructDef {
        let mut metadata = HashMap::new();
        for (k, v) in meta {
            metadata.insert(k.to_string(), crate::ast::PropertyValue::Int(v));
        }
        crate::ast::top::StructDef {
            name: name.to_string(),
            type_params: vec![],
            fields: fields.into_iter().map(|(n, t)| (n.to_string(), t)).collect(),
            metadata,
            span: None,
            seq: false,
            pack: false,
            union: false,
            coll: false,
        }
    }

    #[test]
    fn test_static_struct_spec_bytes_is_authoritative() {
        // §2.1: Bytes wins over Bits. `spec Bytes: 3` overrides `spec Bits: 20`.
        let u = TypeUniverse::new();
        let def = make_struct_def(
            "Packet",
            vec![("tag", Type::int()), ("payload", Type::ptr(Type::bits(8)))],
            vec![("bytes", 3), ("bits", 20), ("alignment", 1)],
        );
        let rt = static_struct_resolved_ty(&def, &u);
        assert_eq!(rt.bytes, 3);
        assert_eq!(rt.max_bits, 20);
        assert_eq!(rt.alignment, 1);
        assert_eq!(rt.fields.len(), 2);
    }

    #[test]
    fn test_structural_fallback_records_warning_once() {
        // Bug sweep B2 (SPEC §2.1): an unpinned representation must be
        // RECORDED, never silent — and deduped across repeated registration.
        let mut u = TypeUniverse::new();
        let fields = vec![
            ("a".to_string(), Type::int()),
            ("b".to_string(), Type::int()),
        ];
        let rt = record_structural_layout(&mut u, "S", "Data", &fields);
        assert_eq!(rt.bytes, 16);
        assert_eq!(rt.alignment, 8);
        assert_eq!(
            u.warnings.iter().filter(|w| w.contains("'S'")).count(), 1,
            "one recorded fallback per type name"
        );
        let _ = record_structural_layout(&mut u, "S", "Data", &fields);
        assert_eq!(
            u.warnings.iter().filter(|w| w.contains("'S'")).count(), 1,
            "re-registration dedupes"
        );
    }

    #[test]
    fn test_static_struct_spec_bits_ceil_no_bytes() {
        // `spec Bits: 12` with no Bytes → 12.div_ceil(8) = 2 bytes.
        let u = TypeUniverse::new();
        let def = make_struct_def("SubByte", vec![("x", Type::bits(8))], vec![("bits", 12)]);
        let rt = static_struct_resolved_ty(&def, &u);
        assert_eq!(rt.bytes, 2);
        assert_eq!(rt.max_bits, 12);
    }

    #[test]
    fn test_static_struct_falls_back_to_slot_sum() {
        // No spec → Σ slot sizes (fe(tag: Int)=8, payload ptr=8) = 16.
        let u = TypeUniverse::new();
        let def = make_struct_def(
            "Legacy",
            vec![("tag", Type::int()), ("payload", Type::ptr(Type::bits(8)))],
            vec![],
        );
        let rt = static_struct_resolved_ty(&def, &u);
        assert_eq!(rt.bytes, 16);
        assert_eq!(rt.alignment, 8);
        assert_eq!(rt.max_bits, 128);
    }
}