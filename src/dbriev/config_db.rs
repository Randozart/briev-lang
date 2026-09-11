// ── ConfigDb — DB-backed shared config/board loader ─────────────────────
//
// 2026-08-03 (Phase 1a, plan docs/plans/2026-08-03-data-briev-config-and-
// board-hardware-map.md): the single routing point for reading .dbv/.dbvl
// configuration and board-map files. It dispatches through the v2 parser
// (`v2::parse_document` / `v2::parse_document_quoted`) and exposes a keyed
// lookup by capitalized constant — the shape both the address resolver and
// config consumers need. The TOML layer is migrated file-by-file onto this
// loader; the compiler never parses TOML for the migrated set.
//
// Design notes:
// - Each standalone `.dbvl` line parses as its own DataGroup holding one
//   DataEntry (probe-verified 2026-08-03). `ConfigDb` flattens groups into
//   a single key → entry index, so callers never see the grouping.
// - Keys are normalized to uppercase at index time; lookups are
//   case-insensitive (the resolver's `id.to_lowercase()` contract).
// - `>schema Name from "path"` registers a schema-import (v2.rs:325 pushes
//   the path into `doc.imports`); `resolve_schema_imports` pulls the
//   referenced schemas in so `.dbvl` line-tables carry their field names.
// - Hex literals parse as DataValue::String; typed accessors return the raw
//   string and let the caller radix-parse (matches the resolver today).

use crate::dbriev::v2::{parse_document, parse_document_quoted, DataEntry, DataField, DataValue, DbrievDocument};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Keyed access to a parsed .dbv/.dbvl document.
pub struct ConfigDb {
    doc: DbrievDocument,
    /// Uppercased key → index into `entries`.
    index: HashMap<String, usize>,
    /// Flat keyed entries in source order.
    entries: Vec<DataEntry>,
}

impl ConfigDb {
    /// Parse bare-token content (default mode, hex strings land as String).
    pub fn from_str(content: &str) -> Result<Self, String> {
        Self::from_doc(parse_document(content)?)
    }

    /// Parse with `--quoted` mode (allows `"..."` literals containing `;`/`}`).
    pub fn from_quoted_str(content: &str) -> Result<Self, String> {
        Self::from_doc(parse_document_quoted(content)?)
    }

    /// Read and parse a file, choosing quoted mode by `quoted`.
    pub fn from_file(path: &Path, quoted: bool) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
        if quoted {
            Self::from_quoted_str(&content)
        } else {
            Self::from_str(&content)
        }
    }

    /// Build the flattened index from a parsed document.
    fn from_doc(doc: DbrievDocument) -> Result<Self, String> {
        let mut index = HashMap::new();
        let mut entries = Vec::new();
        // Flat iteration over every group's entries — each standalone .dbvl
        // line is its own group holding one entry, so the total is linear.
        for entry in doc.data_groups.iter().flat_map(|g| &g.entries) {
            let Some(key) = &entry.key else { continue };
            let idx = entries.len();
            entries.push(entry.clone());
            index.insert(key.to_uppercase(), idx);
        }
        Ok(ConfigDb { doc, index, entries })
    }

    /// Pull in schemas referenced by `>schema Name from "path"` directives.
    /// `search_paths` is tried in order, then the bare path. A missing schema
    /// file is not an error — line-tables degrade to positional fields.
    pub fn resolve_schema_imports(&mut self, search_paths: &[PathBuf]) {
        let mut schemas = Vec::new();
        for import_path in &self.doc.imports {
            let resolved = search_paths
                .iter()
                .map(|p| p.join(import_path))
                .chain(std::iter::once(PathBuf::from(import_path)))
                .find(|p| p.exists());
            let Some(sp) = resolved else { continue };
            if let Ok(content) = std::fs::read_to_string(&sp) {
                if let Ok(imported) = parse_document(&content) {
                    schemas.extend(imported.schemas);
                }
            }
        }
        for schema in schemas {
            if !self.doc.schemas.iter().any(|s| s.name == schema.name) {
                self.doc.schemas.push(schema);
            }
        }
    }

    /// Number of keyed entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sorted key list (deterministic iteration).
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.entries.iter().filter_map(|e| e.key.clone()).collect();
        keys.sort();
        keys
    }

    /// Case-insensitive keyed lookup (matches `resolve_address`'s lowercase
    /// contract).
    pub fn get(&self, key: &str) -> Option<&DataEntry> {
        self.index.get(&key.to_uppercase()).map(|i| &self.entries[*i])
    }

    /// Positional field value at `idx`, if present.
    pub fn field(&self, key: &str, idx: usize) -> Option<&DataValue> {
        let entry = self.get(key)?;
        match entry.fields.get(idx) {
            Some(DataField::Positional(v)) | Some(DataField::Named(_, v)) => Some(v),
            _ => None,
        }
    }

    /// String field (hex literals arrive as String — caller radix-parses).
    pub fn field_string(&self, key: &str, idx: usize) -> Option<&str> {
        match self.field(key, idx) {
            Some(DataValue::String(s)) => Some(s),
            _ => None,
        }
    }

    /// Integer field.
    pub fn field_int(&self, key: &str, idx: usize) -> Option<i64> {
        match self.field(key, idx) {
            Some(DataValue::Int(n)) => Some(*n),
            _ => None,
        }
    }

    /// Float field.
    pub fn field_float(&self, key: &str, idx: usize) -> Option<f64> {
        match self.field(key, idx) {
            Some(DataValue::Float(f)) => Some(*f),
            _ => None,
        }
    }

    /// All keyed entries as a `key → first-string-field` map.
    ///
    /// 2026-08-03 (Phase 3): the shape registry-style configs
    /// (module-registry) need — every entry's key maps to its value string.
    pub fn string_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for entry in &self.entries {
            let Some(key) = &entry.key else { continue };
            let Some(DataField::Positional(DataValue::String(s))) = entry.fields.first() else {
                continue;
            };
            map.insert(key.clone(), s.clone());
        }
        map
    }

    /// Raw parsed document (schemas, imports).
    pub fn doc(&self) -> &DbrievDocument {
        &self.doc
    }
}

/// Resolve a logical config name to a concrete file in `config_dir`.
/// Data Briev extensions only: `<name>.dbvl`, then `<name>.dbv`.
///
/// 2026-08-03 (Phase 1a → 3): migration seam — as configs moved TOML → DB the
/// resolved path just changed extension. 2026-08-03 (Phase 3-complete): all
/// six configs are DB now and the `.toml` fallback is removed; a stale
/// pre-migration `.toml` in a profile dir is no longer picked up (the baked
/// fallback covers it). `"__baked__"` (the compile-time fallback marker from
/// `config_resolver::resolve_config_dir`) maps to the repo's `config/`
/// directory.
pub fn resolve_config_file(config_dir: &Path, name: &str) -> Option<PathBuf> {
    let dir = if config_dir.to_string_lossy() == "__baked__" {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config")
    } else {
        config_dir.to_path_buf()
    };
    for ext in [".dbvl", ".dbv"] {
        let candidate = dir.join(format!("{}{}", name, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Load a registry-style config (`name → string`) from the resolved config
/// dir, as a Data Briev line table.
///
/// 2026-08-03 (Phase 3): the shared seam for `module-registry`. `config_dir`
/// is the already-resolved dir (or `"__baked__"`). Absent or unparseable →
/// empty map (callers fall back to literal resolution), matching the
/// pre-migration TOML semantics.
pub fn load_string_registry(config_dir: &Path, name: &str) -> HashMap<String, String> {
    let Some(path) = resolve_config_file(config_dir, name) else {
        return HashMap::new();
    };
    ConfigDb::from_file(&path, false)
        .map(|db| db.string_map())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbriev::v2::DataValue;

    const ADDRESSES: &str = "\
>schema Device from \"map.dbv\"\n\
UART0: 0xFFE01000; 0x18;\n\
UART1: 0x40004400; 0x18;\n\
TIMER: 0xFE002000; 0x4;\n";

    /// Pre-migration config/module-registry.toml, frozen as the golden
    /// reference for parity_module_registry_dbvl_matches_toml. 2026-08-03: the
    /// .toml file is deleted; edits to config/module-registry.dbvl must keep
    /// this test green.
    const MODULE_REGISTRY_TOML_GOLDEN: &str = r#"
[modules]
"prelude"         = "std/prelude.bv"
"option"          = "std/option.bv"
"result"          = "std/result.bv"
"char"            = "std/char.bv"
"string"          = "std/string.bv"
"string_builder"  = "std/string_builder.bv"
"collections"     = "std/collections.bv"
"iterator"        = "std/iterator.bv"
"hashmap"         = "std/hashmap.bv"
"hashset"         = "std/hashset.bv"
"stack"           = "std/stack.bv"
"queue"           = "std/queue.bv"
"ptr"             = "std/ptr.bv"
"io"              = "std/io.bv"
"out"             = "std/out.bv"
"env"             = "std/env.bv"
"process"         = "std/process.bv"
"time"            = "std/time.bv"
"http"            = "std/http.bv"
"json"            = "std/json.bv"
"encoding"        = "std/encoding.bv"
"bits"            = "std/bits.bv"
"from-bits"       = "std/from-bits.bv"
"atomic"          = "std/atomic.bv"
"state"           = "std/state.bv"
"system"          = "std/system.bv"
"console"         = "std/console.bv"
"tty"             = "std/tty.bv"
"gpu"             = "std/gpu.bv"
"spatial"         = "std/spatial.bv"
"xxhash"          = "std/xxhash.bv"
"skiplist"        = "std/skiplist.bv"
"types"           = "std/types.bv"
"core"            = "std/core"
"c"               = "std/c"
"ffi"             = "std/ffi"
"os"              = "std/os"
"ext"             = "std/ext"
"#;

    /// Pre-migration TOML shape for module-registry (the `[modules]` table).
    #[derive(serde::Deserialize)]
    struct TomlRegistry {
        modules: std::collections::HashMap<String, String>,
    }

    #[test]
    fn indexes_keyed_lines_case_insensitively() {
        let db = ConfigDb::from_str(ADDRESSES).unwrap();
        assert_eq!(db.len(), 3);

        // Lookup by any case — the resolver contract.
        let uart0 = db.get("UART0").unwrap();
        assert_eq!(uart0.key.as_deref(), Some("UART0"));
        assert_eq!(db.get("uart0").unwrap().key.as_deref(), Some("UART0"));
        assert_eq!(db.get("Timer").unwrap().key.as_deref(), Some("TIMER"));
        assert!(db.get("NOPE").is_none());
    }

    #[test]
    fn typed_field_access_hex_string_int() {
        let db = ConfigDb::from_str(ADDRESSES).unwrap();
        // Hex literals arrive as String (parser behavior) — the caller
        // radix-parses, exactly like resolve_address does today.
        assert_eq!(db.field_string("UART0", 0), Some("0xFFE01000"));
        assert_eq!(db.field_string("UART0", 1), Some("0x18"));
        assert_eq!(db.field_string("TIMER", 1), Some("0x4"));
        // Decimal literals arrive as Int.
        let db = ConfigDb::from_str(">schema T\nX: 16; 0x20;\n").unwrap();
        assert_eq!(db.field_int("X", 0), Some(16));
        assert_eq!(db.field_string("X", 1), Some("0x20"));
        // Out-of-range access is None, not a panic.
        assert!(db.field("UART0", 9).is_none());
    }

    #[test]
    fn keys_are_sorted_and_deterministic() {
        let db = ConfigDb::from_str(ADDRESSES).unwrap();
        assert_eq!(db.keys(), vec!["TIMER", "UART0", "UART1"]);
    }

    #[test]
    fn quoted_mode_parses_escaped_strings() {
        // alloc-strategies templates arrive as quoted values. Real templates
        // start with `%{v}_p = ...` — the leading `%` keeps them out of the
        // nested-block branch; the quoted string carries the `;`s and escapes.
        let db = ConfigDb::from_quoted_str(
            ">schema Strategy\n\
             pool_serial: \"%{v}_p = call ptr @pool_alloc(i64 {size});\"; none;\n",
        )
        .unwrap();
        let s = db.field_string("pool_serial", 0).unwrap();
        assert!(s.contains("call ptr @pool_alloc"));
        // The `;` inside the quoted template survives quoted-mode parsing.
        assert!(s.contains(';'));
    }

    #[test]
    fn quoted_mode_preserves_multiline_templates() {
        // `\n` escapes inside quoted templates become real newlines.
        let db = ConfigDb::from_quoted_str(
            ">schema Strategy\n\
             mmap_shared: \"%{v}_p = call ptr @mmap_shared(i64 {size})\\n%{v} = ptrtoint ptr %{v}_p to i64\";\n",
        )
        .unwrap();
        let s = db.field_string("mmap_shared", 0).unwrap();
        assert!(s.contains('\n'));
        assert!(s.contains("ptrtoint ptr %{v}_p to i64"));
    }

    #[test]
    fn bare_mode_rejects_templates_with_braces() {
        // Without quoted mode, `{size}` is taken as a nested sub-record block
        // and the trailing `}` errors. alloc-strategies templates therefore
        // REQUIRE quoted mode — this is the migration gate for Phase 4.
        let bare = ConfigDb::from_str(
            ">schema Strategy\nHOT_LOOP: \"%{v}_p = call ptr @pool_alloc(i64 {size})\";\n",
        );
        assert!(bare.is_err(), "bare mode must reject brace-containing templates");
    }

    #[test]
    fn schema_imports_resolve_across_files() {
        // Write a map.dbv + addresses.dbvl into a temp dir and resolve the
        // `>schema Device from "map.dbv"` import.
        let dir = std::env::temp_dir().join("briev-configdb-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("map.dbv"),
            "schema Device { base_addr: String; size: Int; };\n",
        )
        .unwrap();
        std::fs::write(dir.join("addresses.dbvl"), ADDRESSES).unwrap();

        let mut db = ConfigDb::from_file(&dir.join("addresses.dbvl"), false).unwrap();
        db.resolve_schema_imports(&[dir.clone()]);
        let doc = db.doc();
        assert!(doc.schemas.iter().any(|s| s.name == "Device"));

        // Cleanup.
        let _ = std::fs::remove_file(dir.join("map.dbv"));
        let _ = std::fs::remove_file(dir.join("addresses.dbvl"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn missing_schema_import_is_not_fatal() {
        let db = ConfigDb::from_str(
            ">schema Device from \"no-such-file.dbv\"\nUART0: 0xFFE01000; 0x18;\n",
        )
        .unwrap();
        let mut db = db;
        db.resolve_schema_imports(&[]);
        // Line still accessible by key.
        assert_eq!(db.field_string("UART0", 0), Some("0xFFE01000"));
    }

    #[test]
    fn resolve_config_file_prefers_db_extension() {
        // Real baked config dir: only Data Briev extensions resolve.
        let baked = Path::new("__baked__");
        // targets migrated to .dbvl (Phase 3) — resolves to .dbvl.
        let t = resolve_config_file(baked, "targets").unwrap();
        assert_eq!(t.extension().unwrap().to_str(), Some("dbvl"));

        // A name with no file anywhere resolves to None.
        assert!(resolve_config_file(baked, "no-such-config").is_none());
    }

    #[test]
    fn resolve_config_file_prefers_db_extension_in_dir() {
        // A directory with both forms resolves to the Data Briev one.
        let dir = std::env::temp_dir().join("briev-configdb-resolve");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("demo.dbv"), "schema Demo { a: Int; };\n").unwrap();

        let path = resolve_config_file(&dir, "demo").unwrap();
        assert_eq!(path.file_name().unwrap().to_str(), Some("demo.dbv"));

        // Cleanup.
        let _ = std::fs::remove_file(dir.join("demo.dbv"));
        let _ = std::fs::remove_dir(&dir);
    }

    // ── Config-parity harness (Phase 1a) ──────────────────────────────────
    //
    // Proves a `.dbvl` address table loaded through ConfigDb yields exactly the
    // addresses the current resolver produces from config/address-map.dbvl.
    // Phase 2 retargets resolve_address onto the board's addresses.dbvl; this
    // test locks the data contract so the swap is output-identical.

    fn radix_parse_hex(s: &str) -> u64 {
        let clean = s.trim_start_matches("0x").trim_start_matches("0X");
        u64::from_str_radix(clean, 16).unwrap()
    }

    /// The address-map.dbvl contents as a .dbvl line-table (the Phase 2 board
    /// form). Keys are CAPITALIZED constants per the plan.
    const ADDRESS_MAP_DBVL: &str = "\
>schema AddressEntry from \"map.dbv\"\n\
UART: 0xFFE01000; 0x1000;\n\
UART0: 0xFFE01000; 0x1000;\n\
GPIO: 0xFE001000; 0x1000;\n\
GPIO0: 0xFE001000; 0x1000;\n\
TIMER: 0xFE002000; 0x1000;\n\
TIMER0: 0xFE002000; 0x1000;\n\
SPI: 0xFE003000; 0x1000;\n\
SPI0: 0xFE003000; 0x1000;\n\
I2C: 0xFE004000; 0x1000;\n\
I2C0: 0xFE004000; 0x1000;\n\
DMA: 0xFE005000; 0x1000;\n\
DMA0: 0xFE005000; 0x1000;\n";

    #[test]
    fn parity_address_map_dbvl_matches_resolver() {
        let db = ConfigDb::from_str(ADDRESS_MAP_DBVL).unwrap();
        assert_eq!(db.len(), 12);

        // Every name in config/address-map.dbvl resolves to the same address
        // through the DB table as through the current resolver.
        for name in ["uart", "uart0", "gpio", "gpio0", "timer", "timer0",
                     "spi", "spi0", "i2c", "i2c0", "dma", "dma0"] {
            let upper = name.to_uppercase();
            let addr = db.field_string(&upper, 0).map(radix_parse_hex).unwrap();
            assert_eq!(
                addr,
                crate::address_resolver::resolve_address(name),
                "DB address for {name} diverges from current resolver"
            );
        }
    }

    #[test]
    fn real_stm32f407_board_files_parse() {
        // Locks the actual committed board data (Phase 2). The stm32f407
        // board map owns UART1/GPIOA at their real addresses and the register
        // detail table carries per-register rows.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("lib").join("boards").join("stm32f407");

        let mut addrs = ConfigDb::from_file(&dir.join("addresses.dbvl"), false).unwrap();
        addrs.resolve_schema_imports(&[dir.clone()]);
        assert_eq!(addrs.field_string("UART1", 0), Some("0x40011000"));
        assert_eq!(addrs.field_string("UART2", 0), Some("0x40004400"));
        assert_eq!(addrs.field_string("GPIOA", 0), Some("0x40020000"));
        assert_eq!(addrs.field_string("GPIOB", 0), Some("0x40020400"));
        // Schema carrier (map.dbv) resolved through the >schema import.
        assert!(addrs.doc().schemas.iter().any(|s| s.name == "Device"));

        let mut regs = ConfigDb::from_file(&dir.join("registers.dbvl"), false).unwrap();
        regs.resolve_schema_imports(&[dir.clone()]);
        assert_eq!(regs.field_string("UART1_DR", 0), Some("0x00"));
        assert_eq!(regs.field_int("UART1_DR", 1), Some(9));
        assert_eq!(regs.field_string("UART1_DR", 2), Some("rw"));
        assert_eq!(regs.field_string("GPIOA_BSRR", 2), Some("wo"));
    }

    #[test]
    fn parity_module_registry_dbvl_matches_toml() {
        // Phase 3 migration gate for module-registry: the .dbvl form must
        // produce exactly the same name→path map as the .toml it replaced.
        // 2026-08-03: the .toml is deleted; this is now a GOLDEN test — the
        // pre-migration TOML is baked below and re-parsed.
        let db_map = load_string_registry(Path::new("__baked__"), "module-registry");
        assert!(
            !db_map.is_empty(),
            "config/module-registry.dbvl must load via the baked config dir"
        );

        let toml_map: TomlRegistry = toml::from_str(MODULE_REGISTRY_TOML_GOLDEN).unwrap();

        assert_eq!(db_map.len(), toml_map.modules.len());
        for (name, path) in &toml_map.modules {
            assert_eq!(
                db_map.get(name).map(String::as_str),
                Some(path.as_str()),
                "module-registry entry '{name}' diverges between .dbvl and .toml"
            );
        }
    }

    #[test]
    fn load_string_registry_reads_dbvl() {
        // A temp config dir with a .dbvl line table loads via the registry.
        let dir = std::env::temp_dir().join("briev-configdb-registry");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("demo.dbvl"), "a: one;\nb: two;\n").unwrap();

        let map = load_string_registry(&dir, "demo");
        assert_eq!(map.get("a").map(String::as_str), Some("one"));
        assert_eq!(map.get("b").map(String::as_str), Some("two"));

        // Cleanup.
        let _ = std::fs::remove_file(dir.join("demo.dbvl"));
        let _ = std::fs::remove_dir(&dir);
    }

    // ── Flat-config grammar (Phase 3) ────────────────────────────────────
    // 2026-08-03: verifies the parser accepts the flattened key forms the
    // committed config/*.dbvl files depend on (dotted/hyphenated/leading-dot
    // keys, space-separated list fields, quoted protocol map names, // comments).
    // These were probes during migration and are now REGRESSION GUARDS: the
    // golden parity tests prove the DBVL output; these prove the parser keeps
    // accepting the exact shapes the configs are written in.

    #[test]
    fn probe_dotted_and_hyphenated_keys() {
        let src = "\
encoding.UTF-8: 0; std.encoding.UTF8.index_at; std.encoding.UTF8.char_count;
encoding.ASCII: 1;
target.x86_64: 16; 4.0; 4;
target.wasm32: 4294967295; 4.0; 4;
";
        let db = ConfigDb::from_str(src).unwrap();
        assert_eq!(db.field_int("encoding.UTF-8", 0), Some(0));
        assert_eq!(db.field_string("encoding.UTF-8", 1), Some("std.encoding.UTF8.index_at"));
        assert_eq!(db.field_int("encoding.ASCII", 0), Some(1));
        assert_eq!(db.field_int("target.x86_64", 0), Some(16));
        assert_eq!(db.field("target.x86_64", 1), Some(&DataValue::Float(4.0)));
        assert_eq!(db.field_int("target.wasm32", 0), Some(4294967295));
    }

    #[test]
    fn probe_leading_dot_key_and_space_list_field() {
        let src = "\
.bv: llvm; --budget 256; prelude env print entry script; none; 50;
.cbv: circt; ; prelude-hw;
";
        let db = ConfigDb::from_str(src).unwrap();
        assert_eq!(db.field_string(".bv", 0), Some("llvm"));
        // Space-separated list fields round-trip as a single string.
        assert_eq!(db.field_string(".bv", 1), Some("--budget 256"));
        assert_eq!(db.field_string(".bv", 2), Some("prelude env print entry script"));
        assert_eq!(db.field_int(".bv", 4), Some(50));
        // Optional trailing fields simply absent.
        assert_eq!(db.field_string(".cbv", 3), None);
    }

    #[test]
    fn probe_protocol_map_quoted_names() {
        // protocols.toml stores #System → "c". # is not an identifier char, so
        // the map names arrive as quoted strings in quoted mode. peek_has_named_fields
        // must recognize the opening `"` as a named-field start (fixed 2026-08-03).
        let src = "\
x86_64-linux: { \"#System\": \"c\" };
wasm32-wasi: { \"#System\": \"wasi_snapshot_preview1\"; \"#Web\": \"wasm_runtime\" };
";
        let db = ConfigDb::from_quoted_str(src).unwrap();
        match db.field("x86_64-linux", 0) {
            Some(DataValue::Map(m)) => assert_eq!(m.get("#System"), Some(&DataValue::String("c".into()))),
            other => panic!("expected map, got {:?}", other),
        }
        match db.field("wasm32-wasi", 0) {
            Some(DataValue::Map(m)) => {
                assert_eq!(m.get("#System"), Some(&DataValue::String("wasi_snapshot_preview1".into())));
                assert_eq!(m.get("#Web"), Some(&DataValue::String("wasm_runtime".into())));
            }
            other => panic!("expected map, got {:?}", other),
        }
    }

    #[test]
    fn only_double_slash_comments_are_skipped() {
        // 2026-08-03: `#` lines are NOT comments in the v2 grammar. A `#` line
        // without a `;` is consumed by parse_positional_values as one bare token
        // that swallows the following keyed line too — so `#` comments silently
        // destroy data. Config files must use `//`; this locks both directions.
        let src = "\
# this is a TOML-style comment, not skipped
real_key: 1;
";
        let db = ConfigDb::from_str(src).unwrap();
        // The `#` line consumed the real_key line — no keyed entries survive.
        assert!(db.is_empty());

        // The supported comment form round-trips normally.
        let src = "// comment\nreal_key: 1;\n";
        let db = ConfigDb::from_str(src).unwrap();
        assert_eq!(db.field_int("real_key", 0), Some(1));
    }
}
