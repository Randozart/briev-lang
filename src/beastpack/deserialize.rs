// 2026-07-25: Beastpack binary deserialization.
// Reads the .beastpack binary format, verifies checksum and version,
// decompresses (if flagged), and parses the BEAST text back into AST.

use crate::ast::TopLevel;
use crate::beast::from_beast;
use crate::type_universe::TypeUniverse;

use super::serialize::{BEASTPACK_MAGIC, BEASTPACK_VERSION, FLAG_COMPRESSED};

/// Deserialize a .beastpack binary back into a typed program.
///
/// Returns the list of TopLevel items and the TypeUniverse.
/// Validates magic, version, and checksum before parsing.
pub fn deserialize(data: &[u8]) -> Result<(Vec<TopLevel>, TypeUniverse), String> {
    let header_size = 10 + 4 + 8 + 4 + 4 + 8; // magic + version + seed + flags + reserved + data_size
    if data.len() < header_size + 32 {
        return Err("beastpack: file too short".into());
    }

    // 1. Verify magic
    if &data[0..10] != BEASTPACK_MAGIC {
        return Err("beastpack: invalid magic".into());
    }

    // 2. Verify checksum
    let checksum_end = data.len();
    let checksum_start = checksum_end - 32;
    let stored_checksum = &data[checksum_start..checksum_end];
    let computed = blake3::hash(&data[..checksum_start]);
    if computed.as_bytes() != stored_checksum {
        return Err("beastpack: checksum mismatch (file may be corrupted)".into());
    }

    // 3. Read header fields
    let version = u32::from_le_bytes([data[10], data[11], data[12], data[13]]);
    if version != BEASTPACK_VERSION {
        return Err(format!(
            "beastpack: version mismatch (expected {}, got {})",
            BEASTPACK_VERSION, version
        ));
    }

    let _obfuscation_seed = u64::from_le_bytes([
        data[14], data[15], data[16], data[17], data[18], data[19], data[20], data[21],
    ]);

    let flags = u32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let _reserved = [data[26], data[27], data[28], data[29]];

    let data_size = u64::from_le_bytes([
        data[30], data[31], data[32], data[33], data[34], data[35], data[36], data[37],
    ]) as usize;

    // 4. Extract data section (after header, before checksum)
    let data_start = header_size;
    let data_end = data_start + data_size;
    if data_end > checksum_start {
        return Err("beastpack: data section extends past checksum".into());
    }
    let raw_data = &data[data_start..data_end];

    // 5. Decompress if flagged
    let text_bytes: Vec<u8> = if flags & FLAG_COMPRESSED != 0 {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut decoder = GzDecoder::new(raw_data);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| format!("beastpack: decompression failed: {}", e))?;
        decompressed
    } else {
        raw_data.to_vec()
    };

    // 6. Parse BEAST text
    let text = String::from_utf8(text_bytes)
        .map_err(|_| String::from("beastpack: data is not valid UTF-8"))?;
    from_beast(&text)
}
