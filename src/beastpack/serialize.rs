// 2026-07-25: Beastpack binary serialization.
// Wraps the existing BEAST text format with a binary header, compression,
// and blake3 checksum. No serde derives needed on AST types — reuses the
// battle-tested to_beast() text serializer.

use crate::ast::TopLevel;
use crate::beast::to_beast;
use crate::type_universe::TypeUniverse;

use super::strip::strip_items;

/// Magic bytes at the start of every .beastpack file.
pub const BEASTPACK_MAGIC: &[u8; 10] = b"BEASTPACK\0";

/// Current .beastpack format version.
pub const BEASTPACK_VERSION: u32 = 1;

/// Flag: data section is gzip-compressed.
pub const FLAG_COMPRESSED: u32 = 1 << 0;

/// Serialize a typed AST to the .beastpack binary format.
///
/// Strips Source$/Comment$ metadata, serializes to BEAST text,
/// compresses with gzip, and wraps with header + blake3 checksum.
pub fn serialize(items: &[TopLevel], universe: &TypeUniverse, obfuscation_seed: u64) -> Vec<u8> {
    // 1. Strip sensitive metadata
    let clean = strip_items(items);

    // 2. Serialize to BEAST text
    let text = to_beast(&clean, universe);
    let text_bytes = text.as_bytes();

    // 3. Compress with gzip
    use flate2::Compression;
    use flate2::write::GzEncoder;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(text_bytes).unwrap();
    let compressed = encoder.finish().unwrap();

    // 4. Build header
    let data_size = compressed.len() as u64;
    let header_size = 10 + 4 + 8 + 4 + 4 + 8; // magic + version + seed + flags + reserved + data_size
    let mut buf = Vec::with_capacity(header_size as usize + data_size as usize + 32);

    buf.extend_from_slice(BEASTPACK_MAGIC); // 10 bytes: magic
    buf.extend_from_slice(&BEASTPACK_VERSION.to_le_bytes()); // 4 bytes: version
    buf.extend_from_slice(&obfuscation_seed.to_le_bytes()); // 8 bytes: seed (0 = none)
    buf.extend_from_slice(&FLAG_COMPRESSED.to_le_bytes()); // 4 bytes: flags
    buf.extend_from_slice(&[0u8; 4]); // 4 bytes: reserved
    buf.extend_from_slice(&data_size.to_le_bytes()); // 8 bytes: data size
    buf.extend_from_slice(&compressed); // N bytes: compressed data

    // 5. Checksum (blake3 of everything before the checksum)
    let checksum = blake3::hash(&buf);
    buf.extend_from_slice(checksum.as_bytes()); // 32 bytes: checksum

    buf
}

use std::io::Write;
