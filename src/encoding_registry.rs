// ── Encoding Registry ──────────────────────────────────────────────────
// 2026-07-18: All encoding names are quoted strings resolved through
// config/encodings.dbvl. No hardcoded PascalCase table — the compiler
// knows zero encoding semantics. Each entry specifies:
//   - char_width: 0 = variable-width (Index# delegates to runtime/stdlib)
//                1+ = fixed-width (Index# emits direct GEP, O(1))
//   - ops.index_at, ops.char_len: stdlib function names for runtime dispatch
//
// 2026-08-03 (Phase 3, data-briev-config plan): migrated from encodings.toml
// to the flat .dbvl line-table form (`name: char_width; index_at; char_len;`).
// The .toml remains until the parity test proves identical output.
//
// Lookup chain:
//   1. config/encodings.dbvl (all names, quoted)
//   2. char_width = 0 (conservative — delegate to stdlib)

use std::collections::HashMap;
use std::path::Path;

/// How Index# is implemented for this encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IndexMode {
    /// Fixed-width: emit GEP (O(1)). char_width must be > 0.
    Direct,
    /// Variable-width: emit runtime scan loop or delegate to stdlib.
    Scan,
}

/// Operations that the backend can emit for this encoding.
/// When present, the compiler emits a call to the named stdlib function.
/// When absent, the backend falls back to byte-level ops (length = handle[1],
/// index = byte GEP).
#[derive(Debug, Clone)]
pub struct EncodingOps {
    /// stdlib function for Index#(s, i) -> Int (character at byte offset).
    /// Signature: i64 @fn_name(ptr %s, i64 %i)
    pub index_at: Option<String>,
    /// stdlib function for Length#(s) -> Int (character count).
    /// Signature: i64 @fn_name(ptr %s)
    pub char_len: Option<String>,
}

/// Character width, indexing strategy, and stdlib ops for an encoding.
#[derive(Debug, Clone)]
pub struct EncodingInfo {
    /// Bytes per character. 0 = variable-width (must use Scan or stdlib call).
    pub char_width: u64,
    /// Indexing strategy: Direct (GEP) or Scan (runtime loop).
    pub index_mode: IndexMode,
    /// Optional stdlib function names for encoding-aware operations.
    pub ops: EncodingOps,
}

/// Load config/encodings.dbvl. Returns empty map if file is missing.
fn config_encodings() -> HashMap<String, EncodingInfo> {
    let path = crate::resource_root().join("config/encodings.dbvl");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let db = match crate::dbriev::config_db::ConfigDb::from_str(&content) {
        Ok(db) => db,
        Err(_) => return HashMap::new(),
    };
    let mut result = HashMap::new();
    for key in db.keys() {
        let Some(enc_key) = key.strip_prefix("encoding.") else {
            continue;
        };
        let char_width = db.field_int(&key, 0).unwrap_or(0) as u64;
        let index_mode = if char_width > 0 { IndexMode::Direct } else { IndexMode::Scan };
        let ops = EncodingOps {
            index_at: db.field_string(&key, 1).map(|s| s.to_string()),
            char_len: db.field_string(&key, 2).map(|s| s.to_string()),
        };
        result.insert(enc_key.to_string(), EncodingInfo { char_width, index_mode, ops });
    }
    result
}

/// Resolve an encoding name to its character width, index mode, and stdlib ops.
/// Unknown names default to char_width = 0 (delegate to runtime).
pub fn get_encoding_info(name: &str) -> EncodingInfo {
    let config = config_encodings();
    if let Some(info) = config.get(name) {
        return info.clone();
    }
    EncodingInfo { char_width: 0, index_mode: IndexMode::Scan, ops: EncodingOps { index_at: None, char_len: None } }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-migration config/encodings.toml, frozen as the golden reference for
    /// parity_encodings_dbvl_matches_toml. 2026-08-03: the .toml file is
    /// deleted; edits to config/encodings.dbvl must keep this test green.
    const ENCODINGS_TOML_GOLDEN: &str = r#"
["encoding.UTF-8"]
char_width = 0
ops.index_at = "std.encoding.UTF8.index_at"
ops.char_len = "std.encoding.UTF8.char_count"

["encoding.ASCII"]
char_width = 1

["encoding.Latin1"]
char_width = 1

["encoding.UTF-16"]
char_width = 0
ops.index_at = "std.encoding.UTF16.index_at"
ops.char_len = "std.encoding.UTF16.char_count"

["encoding.UTF-32"]
char_width = 4

["encoding.shift_jis"]
char_width = 0
ops.index_at = "std.encoding.shift_jis.index_at"
ops.char_len = "std.encoding.shift_jis.char_count"

["encoding.windows_1252"]
char_width = 1

["encoding.iso_8859_15"]
char_width = 1

["encoding.utf_16le"]
char_width = 0

["encoding.utf_16be"]
char_width = 0

["encoding.euc_jp"]
char_width = 0

["encoding.gb2312"]
char_width = 0

["encoding.koi8_r"]
char_width = 1
"#;

    #[test]
    fn test_UTF8_from_config() {
        let info = get_encoding_info("UTF-8");
        assert_eq!(info.char_width, 0);
        assert!(matches!(info.index_mode, IndexMode::Scan));
    }

    #[test]
    fn test_ASCII_from_config() {
        let info = get_encoding_info("ASCII");
        assert_eq!(info.char_width, 1);
        assert!(matches!(info.index_mode, IndexMode::Direct));
    }

    #[test]
    fn test_unknown_encoding_falls_through() {
        let info = get_encoding_info("nonexistent_encoding");
        assert_eq!(info.char_width, 0);
    }

    #[test]
    fn test_UTF8_has_stdlib_ops() {
        let info = get_encoding_info("UTF-8");
        assert!(info.ops.index_at.is_some());
        assert!(info.ops.char_len.is_some());
    }

    #[test]
    fn test_ASCII_no_stdlib_ops() {
        let info = get_encoding_info("ASCII");
        assert!(info.ops.index_at.is_none());
        assert!(info.ops.char_len.is_none());
    }

    #[test]
    fn parity_encodings_dbvl_matches_toml() {
        // Phase 3 migration gate: config/encodings.dbvl must produce exactly
        // the char_width/ops map the .toml it replaced produced. 2026-08-03:
        // the .toml is deleted; this is now a GOLDEN test — the pre-migration
        // TOML is baked below and re-parsed with its pre-migration shape.
        let db_map = config_encodings();

        let raw: HashMap<String, toml::Value> = toml::from_str(ENCODINGS_TOML_GOLDEN).unwrap();
        let mut toml_map = HashMap::new();
        for (key, value) in raw {
            if let Some(enc_key) = key.strip_prefix("encoding.") {
                if let toml::Value::Table(table) = value {
                    let char_width = table.get("char_width").and_then(|v| v.as_integer()).unwrap_or(0) as u64;
                    let index_mode = if char_width > 0 { IndexMode::Direct } else { IndexMode::Scan };
                    let ops = EncodingOps {
                        index_at: table.get("ops").and_then(|v| v.get("index_at")).and_then(|v| v.as_str().map(|s| s.to_string())),
                        char_len: table.get("ops").and_then(|v| v.get("char_len")).and_then(|v| v.as_str().map(|s| s.to_string())),
                    };
                    toml_map.insert(enc_key.to_string(), EncodingInfo { char_width, index_mode, ops });
                }
            }
        }

        assert_eq!(db_map.len(), toml_map.len());
        for (name, expected) in &toml_map {
            let actual = db_map.get(name)
                .unwrap_or_else(|| panic!("encoding '{}' missing from encodings.dbvl", name));
            assert_eq!(actual.char_width, expected.char_width);
            assert_eq!(actual.index_mode, expected.index_mode);
            assert_eq!(actual.ops.index_at, expected.ops.index_at);
            assert_eq!(actual.ops.char_len, expected.ops.char_len);
        }
    }
}
