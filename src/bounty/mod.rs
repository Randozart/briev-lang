// 2026-07-25: .bounty data format — portable distribution container.
// Bundles .lair + .beastpack + manifest into a single platform-independent file.
// The tamer system tool reads .bounty files to produce native binaries.

pub const BOUNTY_MAGIC: &[u8; 9] = b"BOUNDATA\0";
pub const BOUNTY_VERSION: u32 = 1;

pub const SECTION_LAIR: u8 = 1;
pub const SECTION_BEASTPACK: u8 = 2;
pub const SECTION_MANIFEST: u8 = 3;
/// 2026-07-30: User's program compiled to .lair bytecode (data for tamer to interpret).
pub const SECTION_USER_LAIR: u8 = 4;

/// Layout:
///   Offset  Size  Field
///   0       9     Magic: "BOUNDATA\0"
///   9       4     Version (u32 LE)
///   13      4     Flags (u32 LE)
///   17      4     Section count (u32 LE)
///   21      N*21  Section table: entries of [type: u8, offset: u64, size: u64]
///   21+N*21 —    Section data (.lair, .beastpack, manifest)

/// A section entry in the .bounty file.
#[derive(Debug, Clone)]
pub struct BountySection {
    pub type_id: u8,
    pub offset: u64,
    pub size: u64,
}

/// Write a .bounty file from its component parts (3-section: lair + beastpack + manifest).
pub fn write_bounty(lair_data: &[u8], beastpack_data: &[u8], manifest: &str) -> Vec<u8> {
    let sections = vec![
        (SECTION_LAIR, lair_data.len()),
        (SECTION_BEASTPACK, beastpack_data.len()),
        (SECTION_MANIFEST, manifest.len()),
    ];
    let data_map: std::collections::HashMap<u8, &[u8]> = [
        (SECTION_LAIR, lair_data as &[u8]),
        (SECTION_BEASTPACK, beastpack_data as &[u8]),
        (SECTION_MANIFEST, manifest.as_bytes()),
    ]
    .into();
    write_bounty_impl(&sections, &data_map)
}

/// 2026-07-30: 4-section variant — includes user .lair for the tamer to interpret.
pub fn write_bounty_full(
    tamer_lair: &[u8],
    user_lair: &[u8],
    beastpack: &[u8],
    manifest: &str,
) -> Vec<u8> {
    let sections = vec![
        (SECTION_LAIR, tamer_lair.len()),
        (SECTION_BEASTPACK, beastpack.len()),
        (SECTION_MANIFEST, manifest.len()),
        (SECTION_USER_LAIR, user_lair.len()),
    ];
    let data_map: std::collections::HashMap<u8, &[u8]> = [
        (SECTION_LAIR, tamer_lair as &[u8]),
        (SECTION_BEASTPACK, beastpack as &[u8]),
        (SECTION_MANIFEST, manifest.as_bytes()),
        (SECTION_USER_LAIR, user_lair as &[u8]),
    ]
    .into();
    write_bounty_impl(&sections, &data_map)
}

/// Shared layout writer.
fn write_bounty_impl(
    sections: &[(u8, usize)],
    data_map: &std::collections::HashMap<u8, &[u8]>,
) -> Vec<u8> {
    let section_count = sections.len() as u32;
    // Header: magic(9) + version(4) + flags(4) + count(4) = 21 bytes
    let header_size = 21usize;
    // Section table: type(1) + offset(8) + size(8) = 17 bytes per entry
    let table_size = section_count as usize * 17;

    // Calculate offsets (relative to end of section table)
    let mut current_offset = (header_size + table_size) as u64;
    let mut section_entries: Vec<BountySection> = Vec::new();
    for (type_id, size) in sections {
        section_entries.push(BountySection {
            type_id: *type_id,
            offset: current_offset,
            size: *size as u64,
        });
        current_offset += *size as u64;
    }

    let total_size = current_offset as usize;

    let mut buf = Vec::with_capacity(total_size);

    // Header
    buf.extend_from_slice(BOUNTY_MAGIC);
    buf.extend_from_slice(&BOUNTY_VERSION.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // flags
    buf.extend_from_slice(&section_count.to_le_bytes());

    // Section table
    for entry in &section_entries {
        buf.push(entry.type_id);
        buf.extend_from_slice(&entry.offset.to_le_bytes());
        buf.extend_from_slice(&entry.size.to_le_bytes());
    }

    // Section data
    for entry in &section_entries {
        if let Some(data) = data_map.get(&entry.type_id) {
            buf.extend_from_slice(data);
        }
    }

    buf
}

/// Read the section table from a .bounty file.
pub fn read_sections(data: &[u8]) -> Result<Vec<BountySection>, String> {
    if data.len() < 21 {
        return Err("bounty: file too short".into());
    }
    if &data[0..9] != BOUNTY_MAGIC {
        return Err("bounty: invalid magic".into());
    }
    let version = u32::from_le_bytes([data[9], data[10], data[11], data[12]]);
    if version != BOUNTY_VERSION {
        return Err(format!(
            "bounty: version mismatch (expected {}, got {})",
            BOUNTY_VERSION, version
        ));
    }
    let section_count = u32::from_le_bytes([data[17], data[18], data[19], data[20]]) as usize;
    let table_start = 21;

    let mut sections = Vec::with_capacity(section_count);
    for i in 0..section_count {
        let entry_start = table_start + i * 17;
        if entry_start + 17 > data.len() {
            return Err("bounty: section table truncated".into());
        }
        let type_id = data[entry_start];
        let offset = u64::from_le_bytes([
            data[entry_start + 1],
            data[entry_start + 2],
            data[entry_start + 3],
            data[entry_start + 4],
            data[entry_start + 5],
            data[entry_start + 6],
            data[entry_start + 7],
            data[entry_start + 8],
        ]);
        let size = u64::from_le_bytes([
            data[entry_start + 9],
            data[entry_start + 10],
            data[entry_start + 11],
            data[entry_start + 12],
            data[entry_start + 13],
            data[entry_start + 14],
            data[entry_start + 15],
            data[entry_start + 16],
        ]);
        sections.push(BountySection {
            type_id,
            offset,
            size,
        });
    }
    Ok(sections)
}

/// Extract a section's data from a .bounty file by type ID.
pub fn extract_section(data: &[u8], type_id: u8) -> Result<Vec<u8>, String> {
    let sections = read_sections(data)?;
    for section in &sections {
        if section.type_id == type_id {
            let start = section.offset as usize;
            let end = start + section.size as usize;
            if end > data.len() {
                return Err("bounty: section data extends past file end".into());
            }
            return Ok(data[start..end].to_vec());
        }
    }
    Err(format!("bounty: section type {} not found", type_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read_bounty() {
        let lair = vec![1, 2, 3, 4];
        let beastpack = vec![5, 6, 7, 8, 9];
        let manifest = r#"{"version":1}"#;

        let bounty = write_bounty(&lair, &beastpack, manifest);
        assert_eq!(&bounty[0..9], BOUNTY_MAGIC);

        let sections = read_sections(&bounty).unwrap();
        assert_eq!(sections.len(), 3);

        // Extract and verify each section
        let extracted_lair = extract_section(&bounty, SECTION_LAIR).unwrap();
        assert_eq!(extracted_lair, lair);

        let extracted_beastpack = extract_section(&bounty, SECTION_BEASTPACK).unwrap();
        assert_eq!(extracted_beastpack, beastpack);

        let extracted_manifest = extract_section(&bounty, SECTION_MANIFEST).unwrap();
        assert_eq!(String::from_utf8(extracted_manifest).unwrap(), manifest);
    }

    #[test]
    fn test_invalid_magic() {
        let data = vec![0u8; 30];
        let result = read_sections(&data);
        assert!(result.is_err());
    }
}
