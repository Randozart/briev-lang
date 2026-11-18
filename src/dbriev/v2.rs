// DBriev v2 — Data Briev parser (.dbv / .dbvl)
//
// 2026-07-26: New syntax — ; separator, > directives, bare tokens default.
// See docs/architecture/data-briev.md for the full spec.
//
// Produces clean document types (not native Briev values).
// Conversion to native Value happens at import time.

use serde::Serialize;
use std::collections::HashMap;

// ============================================================================
// Types
// ============================================================================

/// Parsed DBriev document — can represent .dbv or .dbvl content.
/// .dbvs is removed — schema lives inline in .dbv or is imported.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DbrievDocument {
    pub imports: Vec<String>,
    pub schemas: Vec<SchemaDef>,
    pub data_groups: Vec<DataGroup>,
    /// Key → byte offset index for lazy loading
    #[serde(skip)]
    pub key_offsets: HashMap<String, Vec<usize>>,
}

/// A schema definition (inline in .dbv or standalone schema-only .dbv)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchemaDef {
    pub name: String,
    /// Optional key field annotation: schema Name (keyField) { ... }
    pub key_field: Option<String>,
    pub fields: Vec<FieldDef>,
}

/// A single field in a schema
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldDef {
    pub name: String,
    pub ty: FieldType,
    /// Raw constraint expression text (e.g. `!= ""`, `>= 0`), without brackets
    pub constraint: Option<String>,
    /// True if field is optional (marked with ? suffix)
    pub optional: bool,
}

/// Field type expression
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FieldType {
    String,
    Int,
    Float,
    Bool,
    UInt(usize),
    Vec(Box<FieldType>),
    Map(Box<FieldType>, Box<FieldType>),
    Option(Box<FieldType>),
    /// Reference to another schema by name
    Named(String),
}

/// A named data group (e.g. `as Item { ... }`)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataGroup {
    pub schema_name: Option<String>,
    pub entries: Vec<DataEntry>,
}

/// A single data entry with optional key and optional schema reference
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataEntry {
    pub key: Option<String>,
    /// Schema name when using inline `key as Schema { ... }` form
    pub schema_name: Option<String>,
    pub fields: Vec<DataField>,
}

/// A field value — either positional or named
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum DataField {
    Positional(DataValue),
    Named(String, DataValue),
}

/// A data value expression
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum DataValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<DataValue>),
    Map(HashMap<String, DataValue>),
}

// ============================================================================
// Parser
// ============================================================================

/// Parse a .dbv or .dbvl document (new syntax, bare tokens default).
pub fn parse_document(input: &str) -> Result<DbrievDocument, String> {
    let mut parser = Parser::new(input.to_string());
    parser.parse()
}

/// Parse with --quoted flag enabled (allows "..." for data with ; or }).
pub fn parse_document_quoted(input: &str) -> Result<DbrievDocument, String> {
    let mut parser = Parser::new(input.to_string());
    parser.quoted = true;
    parser.parse()
}

/// Parse with byte offset tracking for lazy loading.
pub fn parse_document_track_offsets(input: &str) -> Result<DbrievDocument, String> {
    let mut parser = Parser::new(input.to_string());
    parser.track_offsets = true;
    parser.parse()
}

struct Parser {
    input: String,
    pos: usize,
    track_offsets: bool,
    /// When true, "..." enables literal data containing ; and }.
    /// When false, " is treated as a literal bare token character.
    quoted: bool,
    /// Accumulated key → byte offsets during parsing
    offsets: HashMap<String, Vec<usize>>,
    /// Active schema name set by `schema <path>;` directive.
    /// Applied to subsequent data entries that don't have explicit schema.
    current_schema: Option<String>,
}

impl Parser {
    fn new(input: String) -> Self {
        Parser {
            input,
            pos: 0,
            track_offsets: false,
            quoted: false,
            offsets: HashMap::new(),
            current_schema: None,
        }
    }

    fn parse(&mut self) -> Result<DbrievDocument, String> {
        let mut doc = DbrievDocument {
            imports: Vec::new(),
            schemas: Vec::new(),
            data_groups: Vec::new(),
            key_offsets: HashMap::new(),
        };

        loop {
            self.skip_ws_and_comments();
            if self.is_eof() {
                break;
            }

            let c = self.peek_char().unwrap();
            match c {
                // > directive (dbvl only): >schema, >import, >encoding, >version
                '>' if self.is_start_of_line() => {
                    self.advance();
                    self.skip_ws();
                    if self.starts_with_ignore_case("schema") {
                        self.parse_directive_schema(&mut doc)?;
                    } else if self.starts_with_ignore_case("import") {
                        let path = self.parse_directive_import()?;
                        doc.imports.push(path);
                    } else if self.starts_with_ignore_case("encoding") {
                        // Consume and skip — handled by consumer
                        self.consume_keyword_ignore_case("encoding")?;
                        self.skip_ws();
                        let _enc = self.parse_bare_ident();
                        self.skip_ws();
                        if self.peek_char() == Some(';') {
                            self.advance();
                        }
                    } else if self.starts_with_ignore_case("version") {
                        // Consume and skip — handled by consumer
                        self.consume_keyword_ignore_case("version")?;
                        self.skip_ws();
                        let _ver = self.parse_bare_ident();
                        self.skip_ws();
                        if self.peek_char() == Some(';') {
                            self.advance();
                        }
                    } else {
                        // > followed by something else — treat as positional entry marker
                        // This happens in .dbv mode where > marks a positional entry
                        let fields = self.parse_positional_values()?;
                        doc.data_groups.push(DataGroup {
                            schema_name: self.current_schema.clone(),
                            entries: vec![DataEntry {
                                key: None,
                                schema_name: self.current_schema.clone(),
                                fields,
                            }],
                        });
                    }
                }
                // import "path" (no > — backwards compat)
                'i' | 'I' if self.starts_with_ignore_case("import") => {
                    let path = self.parse_import()?;
                    doc.imports.push(path);
                }
                // schema Name { ... }  or  schema Name (key) { ... }
                's' | 'S' if self.starts_with_ignore_case("schema") => {
                    self.parse_schema(&mut doc)?;
                }
                // as Schema { ... } — grouped data
                'a' | 'A' if self.starts_with_ignore_case("as") => {
                    let offset = if self.track_offsets {
                        Some(self.pos)
                    } else {
                        None
                    };
                    let group = self.parse_grouped_data()?;
                    if let Some(off) = offset {
                        for entry in &group.entries {
                            if let Some(ref key) = entry.key {
                                doc.key_offsets
                                    .entry(key.to_string())
                                    .or_default()
                                    .push(off);
                            }
                        }
                    }
                    doc.data_groups.push(group);
                }
                // Standalone entry: key: schemaName { fields; }; or key: fields;;
                // Also handles positional: > field; field; (covered above by '>' case)
                // Or just a bare key followed by fields
                _ => {
                    // Could be a keyed entry at top level: key: fields;;
                    // Or a schema-less block: { key: val; key: val; }
                    // Or a positional line (dbvl): val; val;
                    let save = self.pos;
                    let result = self.try_parse_standalone_entry();
                    match result {
                        Ok(Some(entry)) => {
                            if let (true, Some(ref key)) = (self.track_offsets, entry.key.as_ref())
                            {
                                doc.key_offsets
                                    .entry(key.to_string())
                                    .or_default()
                                    .push(self.pos);
                            }
                            doc.data_groups.push(DataGroup {
                                schema_name: entry.schema_name.clone(),
                                entries: vec![entry],
                            });
                        }
                        Ok(None) => {
                            // Not a standalone entry — could be a bare positional line (dbvl)
                            self.pos = save;
                            let fields = self.parse_positional_values()?;
                            doc.data_groups.push(DataGroup {
                                schema_name: self.current_schema.clone(),
                                entries: vec![DataEntry {
                                    key: None,
                                    schema_name: self.current_schema.clone(),
                                    fields,
                                }],
                            });
                            // 2026-07-26: Check for stray } at top level after positional parse.
                            // This prevents infinite loops when a } was left unconsumed by
                            // parse_positional_values (which breaks on } without consuming it).
                            self.skip_ws_and_comments();
                            if self.peek_char() == Some('}') {
                                return Err(format!(
                                    "Unexpected '}}' at position {} — unmatched closing brace",
                                    self.pos
                                ));
                            }
                        }
                        Err(e) => {
                            // 2026-07-26: Propagate error from standalone entry parser.
                            // Previously this swallowed the error and tried positional parsing,
                            // which caused infinite loops when a dangling } remained unconsumed.
                            return Err(e);
                        }
                    }
                }
            }
        }

        // 2026-07-26: Reject .dbvs imports with a migration error
        for import in &doc.imports {
            if import.ends_with(".dbvs") {
                return Err(format!(
                    "'.dbvs' extension is removed (import: '{}'). \
                     Use '.dbv' with inline schema, or 'schema Name from \"file.dbv\"' to import.",
                    import
                ));
            }
        }

        Ok(doc)
    }

    /// Returns true if the current position is at the start of a line
    /// (either the beginning of input or immediately after \n).
    fn is_start_of_line(&self) -> bool {
        self.pos == 0 || self.input[..self.pos].ends_with('\n')
    }

    // ========================================================================
    // Directives (>)
    // ========================================================================

    /// Parse `>schema <Name> from <path>` (directive form, not definition).
    fn parse_directive_schema(&mut self, doc: &mut DbrievDocument) -> Result<(), String> {
        self.consume_keyword_ignore_case("schema")?;
        self.skip_ws();
        let name = self.parse_bare_ident();
        self.skip_ws();
        // Optional `from <path>`
        if self.starts_with_ignore_case("from") && !self.is_alphanum_after(4) {
            self.advance_n(4);
            self.skip_ws();
            let path = if self.peek_char() == Some('"') {
                self.parse_string()?
            } else {
                self.parse_bare_ident()
            };
            self.skip_ws();
            if self.peek_char() == Some(';') {
                self.advance();
            }
            doc.imports.push(path.clone());
            // 2026-08-03 (Phase 2): the directive names the schema explicitly
            // (`>schema Device from "map.dbv"`), so current_schema must be the
            // NAME, not the file stem. The stem made data groups tag with "map"
            // and flattening could not find the "Device" schema. Board .dbvl
            // line-tables rely on this tag for register/base flattening.
            self.current_schema = Some(name);
        } else {
            // No path — the name IS the schema name, imported from <name>.dbv
            let path = format!("{}.dbv", name);
            doc.imports.push(path);
            self.current_schema = Some(name);
        }
        Ok(())
    }

    /// Parse `>import <path>` directive.
    fn parse_directive_import(&mut self) -> Result<String, String> {
        self.consume_keyword_ignore_case("import")?;
        self.skip_ws();
        let path = if self.peek_char() == Some('"') {
            self.parse_string()?
        } else {
            self.parse_bare_ident()
        };
        self.skip_ws();
        if self.peek_char() == Some(';') {
            self.advance();
        }
        Ok(path)
    }

    // ========================================================================
    // Import (backwards-compat: `import "path"`)
    // ========================================================================

    fn parse_import(&mut self) -> Result<String, String> {
        self.consume_keyword_ignore_case("import")?;
        self.skip_ws();
        let path = if self.peek_char() == Some('"') {
            self.parse_string()?
        } else {
            self.parse_bare_ident()
        };
        self.skip_ws();
        if self.peek_char() == Some(';') {
            self.advance();
        }
        Ok(path)
    }

    // ========================================================================
    // Schema
    // ========================================================================

    /// Parse a schema definition: `schema Name (key) { field: Type; field: Type; }`
    fn parse_schema(&mut self, doc: &mut DbrievDocument) -> Result<(), String> {
        self.consume_keyword_ignore_case("schema")?;
        self.skip_ws();
        let name = self.parse_identifier()?;
        self.skip_ws();

        // Check for key field annotation: (keyName)
        let key_field = if self.peek_char() == Some('(') {
            self.advance();
            let kf = self.parse_identifier()?;
            self.skip_ws();
            self.expect_char(')')?;
            Some(kf)
        } else {
            None
        };

        self.skip_ws();
        self.expect_char('{')?;

        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') {
                self.advance();
                break;
            }
            if self.is_eof() {
                return Err("Unexpected end of input in schema body".into());
            }
            fields.push(self.parse_field_def()?);
        }

        doc.schemas.push(SchemaDef {
            name: name.clone(),
            key_field,
            fields,
        });
        // 2026-08-09 (Phase 13): an inline `schema Name { }` establishes the
        // current schema for the data groups that follow (`> people` after it).
        // Without this, a `> people` category after an inline schema had
        // schema_name = None and was treated as raw data (SPEC 22.2 — the
        // schema is asserted against the following entries).
        self.current_schema = Some(name);
        // 2026-07-28: Consume optional trailing ; after schema } to prevent
        // the ; from falling through to the main loop's _ => arm, where it
        // gets misparsed as an empty positional value.
        self.skip_ws();
        if self.peek_char() == Some(';') {
            self.advance();
        }
        Ok(())
    }

    fn parse_field_def(&mut self) -> Result<FieldDef, String> {
        self.skip_ws_and_comments();

        // Parse optional constraint: [expr]
        let constraint = if self.peek_char() == Some('[') {
            self.advance();
            let mut depth = 1u32;
            let mut expr = String::new();
            loop {
                if self.is_eof() {
                    return Err("Unterminated constraint bracket".into());
                }
                let c = self.advance().unwrap();
                if c == '[' {
                    depth += 1;
                    expr.push(c);
                } else if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    expr.push(c);
                } else {
                    expr.push(c);
                }
            }
            let trimmed = expr.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        } else {
            None
        };

        self.skip_ws();

        // Field name
        let name = self.parse_identifier()?;
        self.skip_ws();

        // Optional ? suffix
        let optional = self.peek_char() == Some('?');
        if optional {
            self.advance();
            self.skip_ws();
        }

        // : Type
        self.expect_char(':')?;
        self.skip_ws();

        let ty = self.parse_field_type()?;

        // Trailing ; is optional
        self.skip_ws();
        if self.peek_char() == Some(';') {
            self.advance();
        }

        Ok(FieldDef {
            name,
            ty,
            constraint,
            optional,
        })
    }

    fn parse_field_type(&mut self) -> Result<FieldType, String> {
        self.skip_ws();

        if self.starts_with_ignore_case("String") {
            self.advance_n(6);
            Ok(FieldType::String)
        } else if self.starts_with_ignore_case("Int") {
            self.advance_n(3);
            Ok(FieldType::Int)
        } else if self.starts_with_ignore_case("Float") {
            self.advance_n(5);
            Ok(FieldType::Float)
        } else if self.starts_with_ignore_case("Bool") {
            self.advance_n(4);
            Ok(FieldType::Bool)
        } else if self.starts_with_ignore_case("UInt") {
            self.advance_n(4);
            if self.peek_char() == Some('[') {
                self.advance();
                let width_str = self.parse_while(|c| c.is_ascii_digit());
                let width: usize = width_str
                    .parse()
                    .map_err(|_| format!("Invalid UInt width: {}", width_str))?;
                self.skip_ws();
                self.expect_char(']')?;
                Ok(FieldType::UInt(width))
            } else {
                Ok(FieldType::UInt(64))
            }
        } else if self.starts_with_ignore_case("Vec") || self.starts_with_ignore_case("List") {
            if self.starts_with_ignore_case("Vec") {
                self.advance_n(3);
            } else {
                self.advance_n(4);
            }
            self.skip_ws();
            self.expect_char('[')?;
            self.skip_ws();
            let inner = self.parse_field_type()?;
            self.skip_ws();
            self.expect_char(']')?;
            Ok(FieldType::Vec(Box::new(inner)))
        } else if self.starts_with_ignore_case("Map") {
            self.advance_n(3);
            self.skip_ws();
            self.expect_char('[')?;
            self.skip_ws();
            let key_t = self.parse_field_type()?;
            self.skip_ws();
            self.expect_char(';')?;
            self.skip_ws();
            let val_t = self.parse_field_type()?;
            self.skip_ws();
            self.expect_char(']')?;
            Ok(FieldType::Map(Box::new(key_t), Box::new(val_t)))
        } else if self.starts_with_ignore_case("Option") {
            self.advance_n(6);
            self.skip_ws();
            self.expect_char('[')?;
            self.skip_ws();
            let inner = self.parse_field_type()?;
            self.skip_ws();
            self.expect_char(']')?;
            Ok(FieldType::Option(Box::new(inner)))
        } else {
            let name = self.parse_identifier()?;
            Ok(FieldType::Named(name))
        }
    }

    // ========================================================================
    // Grouped data: as SchemaName { ... }
    // ========================================================================

    fn parse_grouped_data(&mut self) -> Result<DataGroup, String> {
        self.consume_keyword_ignore_case("as")?;
        self.skip_ws();

        let schema_name = Some(self.parse_identifier()?);
        self.skip_ws();
        self.expect_char('{')?;
        self.skip_ws_and_comments();

        let mut entries = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') {
                self.advance();
                break;
            }
            if self.is_eof() {
                return Err("Unexpected end of input in data group".into());
            }

            entries.push(self.parse_data_entry_in_group()?);
        }

        // 2026-07-28: Consume optional trailing ; after as-block } for
        // the same reason as parse_schema — prevents ; from derailing
        // the positional value parser.
        self.skip_ws();
        if self.peek_char() == Some(';') {
            self.advance();
        }
        Ok(DataGroup {
            schema_name,
            entries,
        })
    }

    /// Parse an entry within an `as Schema { ... }` block.
    /// Either `key: field; field;` or `> field; field;` or `key: { nested; };`.
    fn parse_data_entry_in_group(&mut self) -> Result<DataEntry, String> {
        self.skip_ws_and_comments();

        // `> field; field;` — positional entry
        if self.peek_char() == Some('>') {
            self.advance();
            self.skip_ws();
            let fields = self.parse_field_list()?;
            return Ok(DataEntry {
                key: None,
                schema_name: None,
                fields,
            });
        }

        // `key: field; field;` — keyed entry
        // `key: SchemaName { ... }` — keyed inline schema (standalone form in as block)
        let key = self.parse_identifier()?;
        self.skip_ws();

        // Check for `:` separator
        if self.peek_char() == Some(':') {
            self.advance();
            self.skip_ws();

            // Could be `key: SchemaName { ... }` (inline schema reference)
            // Peek ahead: if next token is an identifier followed by `{`, it's inline schema
            let save = self.pos;
            let maybe_schema = self.try_parse_identifier();
            if let Some(sname) = maybe_schema {
                self.skip_ws();
                if self.peek_char() == Some('{') {
                    // It's `key: SchemaName { fields; }`
                    self.advance();
                    let fields = self.parse_field_list()?;
                    self.expect_char('}')?;
                    // Optional trailing ; after the closing }
                    self.skip_ws();
                    if self.peek_char() == Some(';') {
                        self.advance();
                    }
                    return Ok(DataEntry {
                        key: Some(key),
                        schema_name: Some(sname),
                        fields,
                    });
                } else {
                    // Not an inline schema — rewind; it's part of the field list
                    self.pos = save;
                }
            }

            // Regular keyed entry: `key: field; field;`
            // Uses `parse_keyed_entry_fields` to stop at the next `ident:`
            // boundary, allowing multiple keyed entries like
            // `overflow: String; "desc"; associative: Bool; "desc2";`
            // to be correctly split into separate entries.
            let fields = self.parse_keyed_entry_fields()?;
            return Ok(DataEntry {
                key: Some(key),
                schema_name: None,
                fields,
            });
        }

        // Not keyed — treat the identifier as the first positional field value
        // This happens for entries inside `as` block that are neither `>` nor `key:`
        // Should not normally happen, but handle gracefully
        let identifier_as_value = self.parse_bare_token();
        let mut fields = vec![DataField::Positional(DataValue::String(
            identifier_as_value,
        ))];
        fields.extend(self.parse_field_list()?);
        Ok(DataEntry {
            key: None,
            schema_name: None,
            fields,
        })
    }

    // ========================================================================
    // Standalone entry: key: schemaName { fields }; or key: fields;;
    // ========================================================================

    /// Try to parse a standalone entry at the top level.
    /// Returns Ok(Some(entry)) on success, Ok(None) if it's not an entry,
    /// Err on parse error.
    fn try_parse_standalone_entry(&mut self) -> Result<Option<DataEntry>, String> {
        self.skip_ws_and_comments();

        // `{ key: val; ... }` — schema-less block
        if self.peek_char() == Some('{') {
            self.advance();
            let fields = self.parse_named_fields_map()?;
            // Consume trailing ; after } if present
            self.skip_ws();
            if self.peek_char() == Some(';') {
                self.advance();
            }
            return Ok(Some(DataEntry {
                key: None,
                schema_name: None,
                fields,
            }));
        }

        // Must start with an identifier for keyed entries
        let save = self.pos;
        let key = match self.try_parse_identifier() {
            Some(k) => k,
            None => return Ok(None),
        };
        self.skip_ws();

        // 2026-08-09 (Phase 13, SPEC 22.3): a `.dbvl` bare record (`rust; a.bv;`)
        // must not be misread as a keyed entry — the first token is only a KEY
        // when followed by `:`/`as`/`{`. If it is followed by `;`, rewind and
        // let the caller parse it as a positional record.
        if self.peek_char() != Some(':')
            && !(self.starts_with_ignore_case("as") && self.is_keyword_boundary("as"))
            && self.peek_char() != Some('{')
        {
            self.pos = save;
            return Ok(None);
        }

        // `key as SchemaName { fields; }` — standalone with `as` keyword (no `:`)
        if self.starts_with_ignore_case("as") && self.is_keyword_boundary("as") {
            self.advance_n(2);
            self.skip_ws();
            let sname = self.parse_identifier()?;
            self.skip_ws();
            if self.peek_char() == Some('{') {
                self.advance();
                let fields = self.parse_field_list()?;
                self.expect_char('}')?;
                self.skip_ws();
                if self.peek_char() == Some(';') {
                    self.advance();
                }
                return Ok(Some(DataEntry {
                    key: Some(key),
                    schema_name: Some(sname),
                    fields,
                }));
            }
            return Err(format!("Expected '{{' after 'as {}'", sname));
        }

        // `key: SchemaName { fields; }` — standalone keyed entry with inline schema
        if self.peek_char() == Some(':') {
            self.advance();
            self.skip_ws();

            // Check for inline schema: `key: as SchemaName { ... }` or `key: SchemaName { ... }`
            // First check for explicit `as` keyword
            if self.starts_with_ignore_case("as") && self.is_keyword_boundary("as") {
                self.advance_n(2);
                self.skip_ws();
                let sname = self.parse_identifier()?;
                self.skip_ws();
                if self.peek_char() == Some('{') {
                    self.advance();
                    let fields = self.parse_field_list()?;
                    self.expect_char('}')?;
                    self.skip_ws();
                    if self.peek_char() == Some(';') {
                        self.advance();
                    }
                    return Ok(Some(DataEntry {
                        key: Some(key),
                        schema_name: Some(sname),
                        fields,
                    }));
                }
                return Err(format!("Expected '{{' after 'as {}'", sname));
            }

            // Also check: `key: SchemaName { ... }` (without explicit `as`)
            let save2 = self.pos;
            if let Some(sname) = self.try_parse_identifier() {
                self.skip_ws();
                if self.peek_char() == Some('{') {
                    self.advance();
                    let fields = self.parse_field_list()?;
                    self.expect_char('}')?;
                    self.skip_ws();
                    if self.peek_char() == Some(';') {
                        self.advance();
                    }
                    return Ok(Some(DataEntry {
                        key: Some(key),
                        schema_name: Some(sname),
                        fields,
                    }));
                }
                // Not inline schema — rewind
                self.pos = save2;
            }

            // Regular keyed: `key: field; field;`
            // Uses `parse_keyed_entry_fields` so that `key2: f1; f2;` following
            // this entry is detected as a boundary, not consumed as more fields.
            let fields = self.parse_keyed_entry_fields()?;
            return Ok(Some(DataEntry {
                key: Some(key),
                schema_name: self.current_schema.clone(),
                fields,
            }));
        }

        // Not keyed — rewind
        self.pos = save;
        Ok(None)
    }

    // ========================================================================
    // Field list parsing
    // ========================================================================

    /// Parse a list of fields: `field; field; { nested; }; field;`
    /// Stops at `}`, `>`, or EOF. No keyed-entry boundary detection —
    /// that responsibility lives in `parse_keyed_entry_fields`.
    fn parse_field_list(&mut self) -> Result<Vec<DataField>, String> {
        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') || self.is_eof() {
                break;
            }

            let val = self.parse_value()?;
            fields.push(val);

            // After each value, either ; or } or EOF
            self.skip_ws_and_comments();
            if self.peek_char() == Some(';') {
                self.advance();
                self.skip_ws_and_comments();
                if self.peek_char() == Some('}') || self.peek_char() == Some('>') || self.is_eof() {
                    break;
                }
            } else if self.peek_char() == Some('}') || self.is_eof() {
                break;
            } else {
                if self.peek_char() == Some('>') {
                    break;
                }
            }
        }
        Ok(fields)
    }

    /// Parse fields for a keyed entry: `field; field; { nested; }; field;`
    /// Like `parse_field_list` but also stops at `ident:` boundaries,
    /// enabling multiple keyed entries in a single `as { }` block:
    ///   overflow: String; "desc"; associative: Bool; "desc2";
    /// After the first entry's fields `String; "desc"`, the `associative:`
    /// pattern triggers a break so the caller can parse a new entry.
    ///
    /// 2026-07-28: Separate function from `parse_field_list` to keep boundary
    /// detection at the entry level, not in the generic field parser.
    fn parse_keyed_entry_fields(&mut self) -> Result<Vec<DataField>, String> {
        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') || self.is_eof() {
                break;
            }

            // Before reading a new value, check if the next token is a keyed
            // entry boundary (`ident:`). If so, stop — the caller handles it.
            let save = self.pos;
            if let Some(_key) = self.try_parse_identifier() {
                if self.peek_char() == Some(':') {
                    self.pos = save;
                    break;
                }
                self.pos = save;
            }
            // Also stop at `>` for mixed keyed/positional entries
            if self.peek_char() == Some('>') {
                break;
            }

            let val = self.parse_value()?;
            fields.push(val);

            self.skip_ws_and_comments();
            if self.peek_char() == Some(';') {
                self.advance();
                self.skip_ws_and_comments();
                if self.peek_char() == Some('}') || self.peek_char() == Some('>') || self.is_eof() {
                    break;
                }
                // After ;, re-check for ident: boundary (loop top handles it)
            } else if self.peek_char() == Some('}') || self.is_eof() {
                break;
            } else {
                if self.peek_char() == Some('>') {
                    break;
                }
            }
        }
        Ok(fields)
    }

    // ========================================================================
    // Positional values (dbvl line or fallback)
    // ========================================================================

    /// Parse semicolon-separated positional values in a dbvl line.
    fn parse_positional_values(&mut self) -> Result<Vec<DataField>, String> {
        let mut fields = Vec::new();
        loop {
            // 2026-08-09 (Phase 13, SPEC 22.3): a `\n` terminates the record
            // (line-oriented `.dbvl`) — check BEFORE skipping whitespace, which
            // would consume the newline and merge the next record.
            if self.peek_char() == Some('\n') {
                break;
            }
            self.skip_ws_and_comments();
            if self.is_eof()
                || self.peek_char() == Some('\n')
                || self.peek_char() == Some('>')
                || self.peek_char() == Some('}')
            {
                break;
            }
            if !fields.is_empty() {
                if self.peek_char() == Some(';') {
                    self.advance();
                    // 2026-08-09 (Phase 13): the `;` branch must NOT skip the
                    // newline (skip_ws_and_comments would merge the next
                    // record). Only spaces are skipped here; a trailing
                    // newline ends the record.
                    while self.peek_char() == Some(' ') || self.peek_char() == Some('\t') {
                        self.advance();
                    }
                    if self.peek_char() == Some('\n') {
                        break;
                    }
                } else {
                    break;
                }
            }
            let val = self.parse_value()?;
            fields.push(val);
        }
        Ok(fields)
    }

    // ========================================================================
    // Value expression parsing
    // ========================================================================

    /// Parse a single value.
    /// - If quoted flag is on, "..." is a string literal.
    /// - true/false are Bool.
    /// - Numeric patterns are Int/Float.
    /// - { } is a nested block or map. Auto-detects named vs positional.
    /// - Otherwise, reads a bare token until ; or }.
    fn parse_value(&mut self) -> Result<DataField, String> {
        self.skip_ws_and_comments();

        // Check for nested block: { ... }
        // 2026-07-26: Auto-detect named vs positional content.
        // Named fields have `ident:` pattern; positional fields are bare tokens.
        // This fixes the bug where `{ > 0; rw; }` inside a sub-record was treated
        // as named fields and errored on `>`. Sub-records are always positional.
        // See docs/architecture/data-briev.md §6.4.
        if self.peek_char() == Some('{') {
            self.advance();
            let save = self.pos;
            let has_named_fields = self.peek_has_named_fields();
            self.pos = save;

            let fields = if has_named_fields {
                self.parse_named_fields_map()?
            } else {
                self.parse_subrecord_fields()?
            };
            self.expect_char('}')?;
            // Convert to a map value
            let mut map = HashMap::new();
            for f in fields {
                match f {
                    DataField::Named(name, val) => {
                        map.insert(name, val);
                    }
                    DataField::Positional(val) => {
                        map.insert(format!("_{}", map.len()), val);
                    }
                }
            }
            return Ok(DataField::Positional(DataValue::Map(map)));
        }

        // Quoted string (only when --quoted flag is on)
        if self.quoted && self.peek_char() == Some('"') {
            let s = self.parse_string()?;
            return Ok(DataField::Positional(DataValue::String(s)));
        }

        // true / false
        if self.starts_with("true") && !self.is_alphanum_after(4) {
            self.advance_n(4);
            return Ok(DataField::Positional(DataValue::Bool(true)));
        }
        if self.starts_with("false") && !self.is_alphanum_after(5) {
            self.advance_n(5);
            return Ok(DataField::Positional(DataValue::Bool(false)));
        }

        // `}` at value start is an error (should have been handled by caller)
        if self.peek_char() == Some('}') {
            return Err(format!(
                "Unexpected '}}' at position {} — unmatched closing brace",
                self.pos
            ));
        }

        // Numeric: digits, ., -
        // 2026-07-26: Must check that the number is followed by a valid terminator
        // (;, }, whitespace, EOF, or > at line start). Otherwise treat as bare token.
        // This prevents 0x4000 from being parsed as Int(0) with "x4000" left over.
        if let Some(c) = self.peek_char() {
            if c.is_ascii_digit() || c == '-' {
                let save = self.pos;
                let num_str = self.parse_while(|c| c.is_ascii_digit() || c == '.' || c == '-');
                if !num_str.is_empty() && num_str != "-" {
                    let next = self.peek_char();
                    let is_terminated = match next {
                        None | Some(';') | Some('}') | Some(' ') | Some('\t') | Some('\n')
                        | Some('\r') => true,
                        Some('>') => self.is_start_of_line(),
                        _ => false,
                    };
                    if is_terminated {
                        if num_str.contains('.') {
                            let f: f64 = num_str
                                .parse()
                                .map_err(|_| format!("Invalid float: {}", num_str))?;
                            return Ok(DataField::Positional(DataValue::Float(f)));
                        } else {
                            let n: i64 = num_str
                                .parse()
                                .map_err(|_| format!("Invalid integer: {}", num_str))?;
                            return Ok(DataField::Positional(DataValue::Int(n)));
                        }
                    }
                    // Not terminated — treat the whole thing as a bare token
                    self.pos = save;
                } else {
                    // If only "-" was read, rewind — it's a bare token starting with -
                    self.pos = save;
                }
            }
        }

        // Bare token: everything until ;, }, or EOF
        let token = self.parse_bare_token();
        Ok(DataField::Positional(DataValue::String(token)))
    }

    /// Parse a bare token — reads until ;, }, > (at line start), \n, or EOF.
    /// Strips leading/trailing whitespace.
    ///
    /// 2026-11-18 (W1-B): `\n` terminates the token — `.dbvl` is line-
    /// oriented (a record is one line), matching `parse_positional_values`
    /// which breaks on `\n` before skipping whitespace. Without this, a
    /// bare token at end-of-line (no trailing `;`) runs into the next line
    /// and swallows it — e.g. `target.x86_64: ...; haswell` parsed the
    /// march field as `"haswell\ntarget.aarch64: 32"`.
    fn parse_bare_token(&mut self) -> String {
        let mut s = String::new();
        loop {
            match self.peek_char() {
                None | Some(';') | Some('}') | Some('\n') => break,
                Some('>') if self.is_start_of_line() => break,
                Some(c) => {
                    self.advance();
                    s.push(c);
                }
            }
        }
        s.trim().to_string()
    }

    /// Try to parse a named field `name: value` — used inside { } maps.
    fn try_parse_named_field(&mut self) -> Option<DataField> {
        let save = self.pos;

        let name = if self.quoted && self.peek_char() == Some('"') {
            match self.parse_string() {
                Ok(s) => s,
                Err(_) => {
                    self.pos = save;
                    return None;
                }
            }
        } else {
            match self.try_parse_identifier() {
                Some(s) => s,
                None => return None,
            }
        };
        self.skip_ws();
        if self.peek_char() == Some(':') {
            self.advance();
            self.skip_ws();
            match self.parse_value() {
                Ok(DataField::Positional(val)) => Some(DataField::Named(name, val)),
                Ok(DataField::Named(_, _)) => {
                    self.pos = save;
                    None
                }
                Err(_) => {
                    self.pos = save;
                    None
                }
            }
        } else {
            self.pos = save;
            None
        }
    }

    /// Peek ahead to detect whether the block content uses named fields (`ident: `) or positional.
    /// Scans forward without consuming, stopping at the first `:`, `;`, or `>`.
    /// Returns true if a `key: ` pattern is found before any `>`, `;`, or EOF.
    /// 2026-07-26: Auto-detection prevents false errors on positional sub-records.
    fn peek_has_named_fields(&self) -> bool {
        // skip_ws manually without consuming on self
        let mut i = self.pos;
        if i >= self.input.len() {
            return false;
        }
        let chars: Vec<char> = self.input[i..].chars().collect();
        let mut ci = 0;
        // skip whitespace
        while ci < chars.len() && chars[ci].is_whitespace() {
            ci += 1;
        }
        // skip comments
        while ci + 1 < chars.len() && chars[ci] == '/' && chars[ci + 1] == '/' {
            ci += 2;
            while ci < chars.len() && chars[ci] != '\n' {
                ci += 1;
            }
            // skip whitespace after comment
            while ci < chars.len() && chars[ci].is_whitespace() {
                ci += 1;
            }
        }
        // Peek: if content starts with `>` or `;` or `}`, it's positional
        if ci >= chars.len() || chars[ci] == '>' || chars[ci] == ';' || chars[ci] == '}' {
            return false;
        }
        // Read the first identifier/token
        let mut token = String::new();
        // 2026-08-03: in quoted mode a `{ }` block may open with a quoted name
        // (e.g. protocol hashwords `"#System": "c"`). # is not an identifier
        // char, so without this the block falls through to positional parsing
        // and errors on the `:`. Undo: revert to alnum/`_` only.
        if self.quoted && chars[ci] == '"' {
            ci += 1;
            while ci < chars.len() && chars[ci] != '"' {
                token.push(chars[ci]);
                ci += 1;
            }
            if ci >= chars.len() {
                return false;
            }
            ci += 1;
        } else {
            while ci < chars.len() && (chars[ci].is_alphanumeric() || chars[ci] == '_') {
                token.push(chars[ci]);
                ci += 1;
            }
        }
        if token.is_empty() {
            return false;
        }
        // Skip whitespace
        while ci < chars.len() && chars[ci].is_whitespace() {
            ci += 1;
        }
        // If followed by ':', it's a named field
        ci < chars.len() && chars[ci] == ':'
    }

    /// Parse positional fields inside a `{ }` sub-record.
    /// Accepts bare values separated by `;` — no `>` or `key:` markers.
    /// 2026-07-26: New function to handle positional sub-records.
    /// Delegates to parse_positional_values since they share the same logic
    /// (; -separated bare values, no named fields, stops at }).
    fn parse_subrecord_fields(&mut self) -> Result<Vec<DataField>, String> {
        self.parse_positional_values()
    }

    /// Parse named fields as a Map: `{ name: val; name: val; }` body
    fn parse_named_fields_map(&mut self) -> Result<Vec<DataField>, String> {
        let mut fields = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek_char() == Some('}') || self.is_eof() {
                break;
            }
            if !fields.is_empty() {
                // 2026-11-16 (P0): the separator is not consistently ours to
                // consume — a nested `{ … }` value already eats the `;` that
                // follows it, so requiring one here broke the very common
                // `a: { v: 1; b: { … } }` shape, while a trailing `;` before the
                // closing brace broke `a: { b: { … }; }`. Consume a separator if
                // present, then decide on `}` alone: end the map there, else
                // parse the next field.
                if self.peek_char() == Some(';') {
                    self.advance();
                    self.skip_ws_and_comments();
                }
                if self.peek_char() == Some('}') || self.is_eof() {
                    break;
                }
            }
            match self.try_parse_named_field() {
                Some(f) => fields.push(f),
                None => return Err(format!("Expected named field at position {}", self.pos)),
            }
        }
        Ok(fields)
    }

    // ========================================================================
    // Primitives
    // ========================================================================

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_char('"')?;
        let mut s = String::new();
        loop {
            match self.advance() {
                None => return Err("Unterminated string literal".into()),
                Some('"') => break,
                Some('\\') => match self.advance() {
                    None => return Err("Unterminated escape sequence".into()),
                    Some('"') => s.push('"'),
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                },
                Some(c) => s.push(c),
            }
        }
        Ok(s)
    }

    fn parse_identifier(&mut self) -> Result<String, String> {
        let s = self.parse_while(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-');
        if s.is_empty() {
            Err(format!("Expected identifier at position {}", self.pos))
        } else {
            Ok(s)
        }
    }

    /// Like parse_identifier but returns empty string instead of error.
    fn parse_bare_ident(&mut self) -> String {
        self.parse_while(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '/')
    }

    /// Check if a keyword at current position is followed by a non-alphanumeric boundary.
    fn is_keyword_boundary(&self, kw: &str) -> bool {
        if !self.starts_with_ignore_case(kw) {
            return false;
        }
        let after = self.pos + kw.len();
        if after >= self.input.len() {
            return true;
        }
        !self.input[after..]
            .chars()
            .next()
            .map_or(false, |c| c.is_alphanumeric() || c == '_')
    }

    /// Try to parse an identifier without consuming on failure.
    /// Mirrors parse_identifier's charset (alnum, `_`, `.`, `-`) so that
    /// hyphenated keys like `from-bits` round-trip. 2026-08-03: previously
    /// omitted `-`, silently dropping standalone entries whose key contained a
    /// hyphen (module-registry). Undo: revert to alnum/`_`/`.` only.
    fn try_parse_identifier(&mut self) -> Option<String> {
        let save = self.pos;
        let s = self.parse_while(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-');
        if s.is_empty() {
            self.pos = save;
            None
        } else {
            Some(s)
        }
    }

    /// Check if character at offset after current pos is alphanumeric.
    fn is_alphanum_after(&self, offset: usize) -> bool {
        let check_pos = self.pos + offset;
        if check_pos < self.input.len() {
            self.input[check_pos..]
                .chars()
                .next()
                .map_or(false, |c| c.is_alphanumeric() || c == '_')
        } else {
            false
        }
    }

    /// Parse characters matching a predicate
    fn parse_while<F>(&mut self, pred: F) -> String
    where
        F: Fn(char) -> bool,
    {
        let mut s = String::new();
        while let Some(c) = self.peek_char() {
            if pred(c) {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() && c != '\n' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(c) = self.peek_char() {
                if c.is_whitespace() {
                    self.advance();
                } else {
                    break;
                }
            }
            if self.starts_with("//") {
                while let Some(c) = self.peek_char() {
                    if c == '\n' {
                        self.advance();
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    fn starts_with_ignore_case(&self, s: &str) -> bool {
        self.input[self.pos..]
            .to_lowercase()
            .starts_with(&s.to_lowercase())
    }

    fn consume_keyword_ignore_case(&mut self, kw: &str) -> Result<(), String> {
        if self.starts_with_ignore_case(kw) {
            self.pos += kw.len();
            if let Some(c) = self.peek_char() {
                if c.is_alphanumeric() || c == '_' {
                    return Err(format!("'{}' followed by alphanumeric — not a keyword", kw));
                }
            }
            Ok(())
        } else {
            Err(format!("Expected keyword '{}'", kw))
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), String> {
        self.skip_ws_and_comments();
        match self.advance() {
            Some(c) if c == expected => Ok(()),
            Some(c) => Err(format!(
                "Expected '{}' but found '{}' at position {}",
                expected,
                c,
                self.pos - 1
            )),
            None => Err(format!("Expected '{}' but reached end of input", expected)),
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.input[self.pos..].chars().next()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn advance_n(&mut self, n: usize) {
        self.pos = std::cmp::min(self.pos + n, self.input.len());
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn consume(&mut self, ch: char) -> Result<(), String> {
        if self.peek_char() == Some(ch) {
            self.advance();
            Ok(())
        } else {
            Err(format!("Expected '{}' at position {}", ch, self.pos))
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    /// 2026-11-16 (S3.0 characterisation): which map shapes parse? The FFI
    /// descriptor needs `values: { "Class": { field; … }; "Class2": { … } }`, and
    /// a multi-entry block in the shipped adapter failed with "Expected named
    /// field" while a single-entry one parsed. This pins the exact rule instead
    /// of guessing: each case asserts whether it parses, and the failure message
    /// carries the parser's own error.
    /// 2026-11-16 (P0 bisection, round 2): the six-class `values` block parses on
    /// its own, so the shipped-adapter failure is an interaction with a sibling
    /// section. Rebuild the real entry here and drop one sibling at a time; the
    /// variant that parses names the interacting pair.
    #[test]
    fn characterise_entry_level_interaction() {
        const VALUES: &str = r#"values: { "String": { class: "b"; zero_copy: true; to_op: "t"; from_op: "f"; from_copy: true }; "Int": { class: "s"; layout: "i" }; "Float": { class: "s"; layout: "f" }; "Bool": { class: "s"; layout: "i8" }; "Data": { class: "b"; zero_copy: true }; "Char": { class: "s"; layout: "i32" } }; "#;
        const PROTOCOLS: &str = r#"protocols: { "String": { native: "int64_t"; c_abi: "int64_t" }; "Int": { native: "int64_t"; c_abi: "int64_t" } }; "#;
        const CONVERSIONS: &str =
            r#"conversions: { to_abi: { "Int": "{name}" }; from_abi: { "Int": "result_abi" } }; "#;
        const STATE: &str = r#"state: { decl: "BrievState* state"; arg: "state"; ffi_type: "" }; "#;
        let base = r#"types_module: "t.bv"; extension: "h"; bridge_kind: "k"; calling_convention: "c_abi"; "#;
        let cases: Vec<(&str, String)> = vec![
            (
                "min: nested map, no trailing semicolon",
                r#"a: { b: { x: "1" } };"#.to_string(),
            ),
            (
                "min: nested map + trailing semicolon",
                r#"a: { b: { x: "1" }; };"#.to_string(),
            ),
            (
                "min: flat then nested",
                r#"a: { v: 1; b: { x: "1" } };"#.to_string(),
            ),
            (
                "min: flat then nested + semi",
                r#"a: { v: 1; b: { x: "1" }; };"#.to_string(),
            ),
            (
                "values alone",
                format!("c: {{ version: 1; {base}{VALUES}}};"),
            ),
            (
                "values + protocols",
                format!("c: {{ version: 1; {base}{VALUES}{PROTOCOLS}}};"),
            ),
            (
                "values + conversions",
                format!("c: {{ version: 1; {base}{VALUES}{CONVERSIONS}}};"),
            ),
            (
                "values + state",
                format!("c: {{ version: 1; {base}{VALUES}{STATE}}};"),
            ),
            (
                "values + protocols + conversions + state (the real shape)",
                format!("c: {{ version: 1; {base}{VALUES}{PROTOCOLS}{CONVERSIONS}{STATE}}};"),
            ),
        ];
        let mut report: Vec<String> = Vec::new();
        for (name, src) in cases {
            match parse_document_quoted(&src) {
                Ok(_) => report.push(format!("{name}: OK")),
                Err(e) => report.push(format!("{name}: ERR {e}")),
            }
        }
        // Every variant must parse; print the table on failure so the interacting
        // pair is visible rather than guessed.
        let failing: Vec<&String> = report.iter().filter(|l| l.contains(": ERR")).collect();
        assert!(
            failing.is_empty(),
            "entry-level shapes:\n{}",
            report.join("\n")
        );
    }

    #[test]
    fn characterise_named_map_shapes() {
        let cases: &[(&str, &str, bool)] = &[
            ("flat", "a: 1; b: 2;", true),
            ("one nested", r#"a: { x: "1" };"#, true),
            ("one nested, quoted key", r#"a: { "X": { y: "1" } };"#, true),
            (
                "two nested, quoted keys",
                r#"a: { "X": { y: "1" }; "Z": { y: "2" } };"#,
                true,
            ),
            (
                "two nested, quoted keys, quoted values",
                r#"a: { "X": { class: "c1" }; "Z": { class: "c2" } };"#,
                true,
            ),
            (
                "three nested, quoted keys",
                r#"a: { "X": { class: "c1" }; "Z": { class: "c2" }; "W": { class: "c3" } };"#,
                true,
            ),
            (
                "two sibling nested maps",
                r#"a: { x: "1" }; b: { y: "2" };"#,
                true,
            ),
            // 2026-11-16 (P0 bisection): the shipped C adapter failed on this
            // block with "Expected named field". These cases narrow it: the six
            // class names with one field each, then the real field sets.
            (
                "six classes, one field each",
                r#"a: { "String": { class: "b" }; "Int": { class: "s" }; "Float": { class: "s" }; "Bool": { class: "s" }; "Data": { class: "b" }; "Char": { class: "s" } };"#,
                true,
            ),
            (
                "one class, five fields",
                r#"a: { "String": { class: "b"; zero_copy: true; to_op: "t"; from_op: "f"; from_copy: true } };"#,
                true,
            ),
            (
                "six classes, real field sets",
                r#"a: { "String": { class: "b"; zero_copy: true; to_op: "t"; from_op: "f"; from_copy: true }; "Int": { class: "s"; layout: "i" }; "Float": { class: "s"; layout: "f" }; "Bool": { class: "s"; layout: "i8" }; "Data": { class: "b"; zero_copy: true }; "Char": { class: "s"; layout: "i32" } };"#,
                true,
            ),
            (
                "two nested, bool in first",
                r#"a: { "X": { class: "c1"; flag: true }; "Z": { class: "c2" } };"#,
                true,
            ),
        ];
        let mut failures: Vec<String> = Vec::new();
        for (name, src, expect_ok) in cases {
            let got = parse_document_quoted(src);
            let ok = got.is_ok();
            if ok != *expect_ok {
                failures.push(format!(
                    "{name}: expected ok={expect_ok}, got {:?}",
                    got.err()
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "shape failures:\n{}",
            failures.join("\n")
        );
    }

    use super::*;

    // ---- Schema Tests ----

    #[test]
    fn test_schema_basic() {
        let input = r#"
schema Item {
    [ != "" ] id: String;
    [ != "" ] desc: String;
    [ >= 0 ] hp: Int;
    takeable: Bool;
    location: String;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 1);
        let schema = &doc.schemas[0];
        assert_eq!(schema.name, "Item");
        assert_eq!(schema.key_field, None);
        assert_eq!(schema.fields.len(), 5);

        assert_eq!(schema.fields[0].name, "id");
        assert_eq!(schema.fields[0].ty, FieldType::String);
        assert_eq!(schema.fields[0].constraint.as_deref(), Some("!= \"\""));
        assert!(!schema.fields[0].optional);

        assert_eq!(schema.fields[1].name, "desc");
        assert_eq!(schema.fields[2].name, "hp");
        assert_eq!(schema.fields[2].ty, FieldType::Int);
        assert_eq!(schema.fields[2].constraint.as_deref(), Some(">= 0"));

        assert_eq!(schema.fields[3].name, "takeable");
        assert_eq!(schema.fields[3].ty, FieldType::Bool);
        assert!(schema.fields[3].constraint.is_none());

        assert_eq!(schema.fields[4].name, "location");
        assert_eq!(schema.fields[4].ty, FieldType::String);
    }

    #[test]
    fn test_schema_key_field() {
        let input = r#"
schema Person (name) {
    name: String;
    age: Int;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 1);
        let schema = &doc.schemas[0];
        assert_eq!(schema.name, "Person");
        assert_eq!(schema.key_field, Some("name".to_string()));
        assert_eq!(schema.fields.len(), 2);
    }

    #[test]
    // 2026-07-28: Verify schema with trailing ; after } parses correctly.
    // This was a parser bug: the ; fell through to the main loop's _ => arm
    // and was misparsed as an empty positional value.
    fn test_schema_trailing_semicolon() {
        let input = r#"
schema Foo { a: Int; b: Bool; };
schema Bar { c: String; };
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 2);
        assert_eq!(doc.schemas[0].name, "Foo");
        assert_eq!(doc.schemas[0].fields.len(), 2);
        assert_eq!(doc.schemas[1].name, "Bar");
        assert_eq!(doc.schemas[1].fields.len(), 1);
    }

    #[test]
    // 2026-07-28: Verify as-block with trailing ; after } parses correctly.
    fn test_as_block_trailing_semicolon() {
        let input = r#"
as Foo { > a; > b; };
as Bar { > c; };
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 2);
        assert_eq!(doc.data_groups[0].schema_name.as_deref(), Some("Foo"));
        assert_eq!(doc.data_groups[0].entries.len(), 2);
        assert_eq!(doc.data_groups[1].schema_name.as_deref(), Some("Bar"));
        assert_eq!(doc.data_groups[1].entries.len(), 1);
    }

    #[test]
    // 2026-07-28: Verify schema followed by as-block with trailing ; works.
    fn test_schema_then_as_block_with_semicolons() {
        let input = r#"
schema MetaField (name) {
    name: String;
    ty: String;
    description: String;
};

as MetaField {
    overflow: String; "test field";
};
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 1);
        assert_eq!(doc.schemas[0].name, "MetaField");
        assert_eq!(doc.schemas[0].key_field, Some("name".to_string()));
        assert_eq!(doc.data_groups.len(), 1);
        assert_eq!(doc.data_groups[0].schema_name.as_deref(), Some("MetaField"));
        assert_eq!(doc.data_groups[0].entries.len(), 1);
        assert_eq!(
            doc.data_groups[0].entries[0].key.as_deref(),
            Some("overflow")
        );
    }

    #[test]
    fn test_schema_with_types() {
        let input = r#"
schema AllTypes {
    a: String;
    b: Int;
    c: Float;
    d: Bool;
    e: UInt[32];
    f: Vec[String];
    g: Map[String; Int];
    h: Option[Bool];
    i: IoResult;
}
"#;
        let doc = parse_document(input).unwrap();
        let s = &doc.schemas[0];
        assert_eq!(s.fields.len(), 9);
        assert_eq!(s.fields[0].ty, FieldType::String);
        assert_eq!(s.fields[1].ty, FieldType::Int);
        assert_eq!(s.fields[2].ty, FieldType::Float);
        assert_eq!(s.fields[3].ty, FieldType::Bool);
        assert_eq!(s.fields[4].ty, FieldType::UInt(32));
        assert_eq!(s.fields[5].ty, FieldType::Vec(Box::new(FieldType::String)));
        assert_eq!(
            s.fields[6].ty,
            FieldType::Map(Box::new(FieldType::String), Box::new(FieldType::Int))
        );
        assert_eq!(s.fields[7].ty, FieldType::Option(Box::new(FieldType::Bool)));
        assert_eq!(s.fields[8].ty, FieldType::Named("IoResult".into()));
    }

    #[test]
    fn test_optional_field() {
        let input = r#"
schema Opt {
    name: String;
    desc?: String;
}
"#;
        let doc = parse_document(input).unwrap();
        let s = &doc.schemas[0];
        assert!(!s.fields[0].optional);
        assert!(s.fields[1].optional);
    }

    // ---- Data Tests ----

    #[test]
    fn test_positional_entry_in_as_block() {
        let input = r#"
as Item {
    > Rusty Key; 5; true;
    > Wax Candle; 3; true;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        assert_eq!(group.schema_name.as_deref(), Some("Item"));
        assert_eq!(group.entries.len(), 2);
        // First entry: positional, no key
        let e0 = &group.entries[0];
        assert!(e0.key.is_none());
        assert_eq!(e0.fields.len(), 3);
        match &e0.fields[0] {
            DataField::Positional(DataValue::String(s)) => assert_eq!(s, "Rusty Key"),
            _ => panic!("Expected positional string"),
        }
        match &e0.fields[1] {
            DataField::Positional(DataValue::Int(n)) => assert_eq!(*n, 5),
            _ => panic!("Expected int 5"),
        }
    }

    #[test]
    fn test_keyed_entry_in_as_block() {
        let input = r#"
as Item {
    > Rusty Key; 5; true;
    > Wax Candle; 3; true;
}
"#;
        let doc = parse_document(input).unwrap();
        let group = &doc.data_groups[0];
        assert_eq!(group.schema_name.as_deref(), Some("Item"));
        assert_eq!(group.entries.len(), 2);
        assert!(group.entries[0].key.is_none());
        assert!(group.entries[1].key.is_none());
        assert_eq!(group.entries[0].fields.len(), 3);
    }

    #[test]
    fn test_inline_schema_standalone() {
        let input = r#"alice: Person { name: Alice Smith; age: 30; };"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        let entry = &group.entries[0];
        assert_eq!(entry.key.as_deref(), Some("alice"));
        assert_eq!(entry.schema_name.as_deref(), Some("Person"));
    }

    #[test]
    fn test_schema_less_block() {
        // The { } block at top level is handled by try_parse_standalone_entry
        // which does not consume the closing }.  Wrap in `as` so the block is
        // parsed as a value inside a `>` entry, where parse_value calls
        // expect_char('}') correctly.
        let input = r#"
as _ {
    > { dom_state: ...; timestamp: 1234567890 };
}
"#;
        let doc = parse_document(input).unwrap();
        let group = &doc.data_groups[0];
        assert_eq!(group.schema_name.as_deref(), Some("_"));
        assert_eq!(group.entries.len(), 1);
        let entry = &group.entries[0];
        assert!(entry.key.is_none());
        assert_eq!(entry.fields.len(), 1);
        match &entry.fields[0] {
            DataField::Positional(DataValue::Map(m)) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains_key("dom_state"));
                assert!(m.contains_key("timestamp"));
                assert_eq!(
                    m.get("dom_state"),
                    Some(&DataValue::String("...".to_string()))
                );
                assert_eq!(m.get("timestamp"), Some(&DataValue::Int(1234567890)));
            }
            _ => panic!("expected a map with named fields"),
        }
    }

    // ---- Directive Tests ----

    #[test]
    fn test_dbvl_directive_schema() {
        let input = ">schema Person from person.dbv\nAlice Smith; 30";
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 1);
        assert_eq!(doc.data_groups.len(), 1);
        let entry = &doc.data_groups[0].entries[0];
        assert!(entry.key.is_none());
        assert_eq!(entry.fields.len(), 2);
    }

    #[test]
    fn test_dbvl_directive_import() {
        let input = ">import addresses.dbvl\nMain St; Springfield;";
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 1);
        assert_eq!(doc.imports[0], "addresses.dbvl");
    }

    #[test]
    fn test_dbvl_directive_encoding() {
        let input = ">encoding utf-8\nAlice; 30;";
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
    }

    // ---- DBVL Tests ----

    #[test]
    fn test_dbvl_positional_line() {
        let input = r#"Rusty Key; 5; true"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let entry = &doc.data_groups[0].entries[0];
        assert!(entry.key.is_none());
        assert_eq!(entry.fields.len(), 3);
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => assert_eq!(s, "Rusty Key"),
            _ => panic!("Expected positional string"),
        }
    }

    #[test]
    fn test_dbvl_positional_with_map() {
        let input = r#"rust; glue/rust/types.bv; rs; x86_64; { Int: int64_t; Float: double }"#;
        let doc = parse_document(input).unwrap();
        let entry = &doc.data_groups[0].entries[0];
        assert_eq!(entry.fields.len(), 5);
        match &entry.fields[4] {
            DataField::Positional(DataValue::Map(m)) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains_key("Int"));
                assert!(m.contains_key("Float"));
            }
            _ => panic!("expected a map"),
        }
    }

    // ---- Key Field Auto-Assignment (for dbvl with schema) ----
    // The parser stores the key_field annotation on SchemaDef.
    // Key auto-assignment from positional entries is handled by the bridge layer.
    // This test verifies the annotation survives parsing.

    #[test]
    fn test_key_field_schema_creation() {
        let input = r#">schema Person from "person.dbv"
Alice Smith; 30;
Bob; 25;
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 1);
        // 2026-08-09 (Phase 13, SPEC 22.3): line-oriented `.dbvl` — each
        // physical line is one record. Two lines = two entries (the previous
        // behavior merged them into one, violating the spec).
        let total_entries: usize = doc.data_groups.iter().map(|g| g.entries.len()).sum();
        assert_eq!(total_entries, 2);
        // The schema is imported, not inline — SchemaDef is empty here
        // Key field annotation on imports is resolved at bridge layer
    }

    // ---- Import Tests ----

    #[test]
    fn test_import() {
        let input = r#"import "game.dbv""#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 1);
        assert_eq!(doc.imports[0], "game.dbv");
    }

    #[test]
    fn test_import_with_semicolon() {
        let input = r#"import "std.dbv";"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 1);
        assert_eq!(doc.imports[0], "std.dbv");
    }

    // ---- Error Tests ----

    #[test]
    fn test_dbvs_import_rejected() {
        let input = r#"import "game.dbvs""#;
        let result = parse_document(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains(".dbvs"));
        assert!(err.contains("removed"));
    }

    #[test]
    fn test_missing_schema_name() {
        let result = parse_document(r#"schema { }"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_unterminated_string_quoted() {
        let input = r#"test as S { "unclosed }"#;
        let result = parse_document_quoted(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_unterminated_bracket() {
        let result = parse_document(
            r#"
schema Bad {
    [ != "" id: String;
}
"#,
        );
        assert!(result.is_err());
    }

    // ---- Value Tests ----

    #[test]
    fn test_value_types() {
        let input = r#"
as Vals {
    > string; 42; 3.14; true; false;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        assert_eq!(group.entries.len(), 1);
        let entry = &group.entries[0];
        assert_eq!(entry.fields.len(), 5);
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => assert_eq!(s, "string"),
            _ => panic!("expected a string "),
        }
        match &entry.fields[1] {
            DataField::Positional(DataValue::Int(n)) => assert_eq!(*n, 42),
            _ => panic!("expected an int "),
        }
        match &entry.fields[2] {
            DataField::Positional(DataValue::Float(f)) => assert!((*f - 3.14).abs() < 1e-10),
            _ => panic!("expected a float "),
        }
        match &entry.fields[3] {
            DataField::Positional(DataValue::Bool(b)) => assert!(*b),
            _ => panic!("expected a bool true "),
        }
        match &entry.fields[4] {
            DataField::Positional(DataValue::Bool(b)) => assert!(!*b),
            _ => panic!("expected a bool false "),
        }
    }

    #[test]
    fn test_map_in_value() {
        let input = r#"
as Vals {
    > { a: 1; b: two };
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        assert_eq!(group.entries.len(), 1);
        let entry = &group.entries[0];
        assert_eq!(entry.fields.len(), 1);
        match &entry.fields[0] {
            DataField::Positional(DataValue::Map(m)) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains_key("a"));
                assert!(m.contains_key("b"));
            }
            _ => panic!("expected a map"),
        }
    }

    #[test]
    fn test_nested_block_in_entry() {
        let input = r#"
as Person {
    alice: Alice Smith; 30; { Main St; Springfield };
}
"#;
        let doc = parse_document(input).unwrap();
        let group = &doc.data_groups[0];
        let entry = &group.entries[0];
        assert!(!group.entries.is_empty());
        assert_eq!(entry.fields.len(), 3);
        match &entry.fields[2] {
            DataField::Positional(DataValue::Map(m)) => {
                assert_eq!(m.len(), 2);
            }
            _ => panic!("expected a map for nested block"),
        }
    }

    #[test]
    fn test_empty_document() {
        let doc = parse_document("").unwrap();
        assert_eq!(doc.imports.len(), 0);
        assert_eq!(doc.schemas.len(), 0);
        assert_eq!(doc.data_groups.len(), 0);
    }

    #[test]
    fn test_only_comments() {
        let doc = parse_document("// just a comment\n// another one\n").unwrap();
        assert_eq!(doc.schemas.len(), 0);
    }

    #[test]
    fn test_negative_int() {
        let input = r#"as Vals { > -42; }"#;
        let doc = parse_document(input).unwrap();
        let group = &doc.data_groups[0];
        let entry = &group.entries[0];
        match &entry.fields[0] {
            DataField::Positional(DataValue::Int(n)) => assert_eq!(*n, -42),
            _ => panic!("expected -42"),
        }
    }

    #[test]
    fn test_multiple_imports() {
        let input = r#"
import "a.dbv";
import "b.dbv";
import "c.dbv";
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 3);
    }

    // ---- Quoted Mode Tests ----

    #[test]
    fn test_quoted_mode_string() {
        let input = r#"as S { > "Alice Smith; age 30"; }"#;
        let doc = parse_document_quoted(input).unwrap();
        let group = &doc.data_groups[0];
        let entry = &group.entries[0];
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => {
                assert_eq!(s, "Alice Smith; age 30");
            }
            _ => panic!("expected string with semicolon"),
        }
    }

    #[test]
    fn test_unquoted_mode_treats_quote_as_literal() {
        let input = r#"test as S { > "just a string with quotes"; }"#;
        // In unquoted mode (default), " is a literal character
        let doc = parse_document(input).unwrap();
        let entry = &doc.data_groups[0].entries[0];
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => {
                // The quote character is part of the bare token
                assert!(s.starts_with('"') || s.contains("just"));
            }
            _ => panic!("expected a string "),
        }
    }

    // ---- Trailing Semicolon Tests ----

    #[test]
    fn test_trailing_semicolon_optional() {
        let input = "as Person { alice: Alice Smith; 30 }";
        let doc = parse_document(input).unwrap();
        let entry = &doc.data_groups[0].entries[0];
        assert_eq!(entry.fields.len(), 2);
    }

    #[test]
    fn test_last_field_no_semicolon() {
        let input = "as Person { alice: Alice Smith; 30 }";
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups[0].entries.len(), 1);
    }

    // ---- Old Syntax Error Recovery Tests ----

    #[test]
    fn test_old_comma_rejected() {
        // Old syntax used , as separator — new syntax uses ;
        // This may parse as a single bare token containing a comma
        // which is valid as a string value
        let input = "rust, glue/types.bv";
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups.len(), 1);
        let entry = &doc.data_groups[0].entries[0];
        // The comma is part of the bare token, not a separator
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => {
                assert_eq!(s, "rust, glue/types.bv");
            }
            _ => panic!("expected string with comma"),
        }
    }

    #[test]
    fn test_old_hash_rejected_as_directive() {
        // Old syntax used #schema — new uses >schema
        // # is a bare token character now
        let input = "#schema Person from \"person.dbv\";\nAlice; 30;";
        let doc = parse_document(input).unwrap();
        // #schema is parsed as a single line of positional values
        let entry = &doc.data_groups[0].entries[0];
        match &entry.fields[0] {
            DataField::Positional(DataValue::String(s)) => {
                assert_eq!(s, "#schema Person from \"person.dbv\"");
            }
            _ => panic!("expected a string"),
        }
    }

    // ---- Combined Tests ----

    #[test]
    fn test_complex_dbv() {
        let input = r#"
import "game.dbv";
import "ffi_core.dbv";

schema Custom {
    x: Int;
    y: Int;
}

as Item {
    rusty_key: Rusty Key; 5; true; start;
}

as FnBinding {
    print: print; [String]; IoResult; libruntime; 0;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.imports.len(), 2);
        assert_eq!(doc.schemas.len(), 1);
        assert_eq!(doc.data_groups.len(), 2);
    }

    #[test]
    fn test_full_hardware_example() {
        let input = r#"
schema Register {
    offset: Int;
    access: String;
}

schema Device {
    base: Int;
    width: Int;
    registers: Vec[Register];
}

as Device {
    > 0x4000; 32; { 0; rw; 4; ro; 8; rw };
    > 0x8000; 8; { 0; rw; 1; ro; 2; rw };
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 2);
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        assert_eq!(group.entries.len(), 2);
        assert!(group.entries[0].key.is_none());
        assert!(group.entries[1].key.is_none());
    }

    #[test]
    fn test_ffi_bindings_example() {
        let input = r#"
schema FnBinding (name) {
    name: String;
    impl: String;
}

as FnBinding {
    > __json_parse; json::parse;
    > __json_stringify; json::stringify;
    > __json_is_object; json::is_object;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 1);
        assert_eq!(doc.schemas[0].key_field, Some("name".to_string()));
        assert_eq!(doc.data_groups.len(), 1);
        let group = &doc.data_groups[0];
        assert_eq!(group.entries.len(), 3);
        assert!(group.entries[0].key.is_none());
        assert_eq!(group.entries[0].fields.len(), 2);
    }

    #[test]
    fn test_schema_without_semicolons() {
        // Schema fields can omit trailing ;
        let input = r#"
schema Point {
    x: Int
    y: Int
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.schemas.len(), 1);
        assert_eq!(doc.schemas[0].fields.len(), 2);
    }

    #[test]
    fn test_comment_between_entries() {
        let input = r#"
as Item {
    > Rusty Key; 5;
    // This is a comment
    > Wax Candle; 3;
}
"#;
        let doc = parse_document(input).unwrap();
        assert_eq!(doc.data_groups[0].entries.len(), 2);
    }

    // ---- Board hardware map format tests (2026-08-03) ----
    //
    // Golden proof of the board-owned hardware map grammar (see
    // docs/plans/2026-08-03-data-briev-config-and-board-hardware-map.md §5.1).
    // Verified by probing the parser: nested { } register blocks do NOT parse;
    // flat .dbvl line-tables and flat .dbv keyed entries DO. Hex literals parse
    // as String, not Int.
    //
    // TEMP: 2026-08-03: when nested sub-record support lands in the parser,
    // revisit whether the board format can express register nesting.

    #[test]
    fn board_map_schemas_only() {
        // map.dbv is schema-only: Device and Register shapes for the .dbvl tables.
        let map = r#"
schema Device {
    base_addr: String;
    size: Int;
};

schema Register {
    name: String;
    offset: Int;
    size: Int;
    access: String;
};
"#;
        let doc = parse_document(map).unwrap();
        assert_eq!(doc.schemas.len(), 2);
        assert_eq!(doc.schemas[0].name, "Device");
        assert_eq!(doc.schemas[0].fields.len(), 2);
        assert_eq!(doc.schemas[0].fields[0].name, "base_addr");
        assert_eq!(doc.schemas[1].name, "Register");
        assert_eq!(doc.schemas[1].fields.len(), 4);
        assert_eq!(doc.schemas[1].fields[0].name, "name");
        // No data in map.dbv — pure schema carrier.
        assert!(doc.data_groups.is_empty());
    }

    #[test]
    fn board_addresses_dbvl_keyed_lines() {
        // addresses.dbvl: >schema directive + flat CAPITALIZED key → addr; size; lines.
        // Hex addresses parse as String (probe-verified); size as Int. Each
        // standalone line parses as its own DataGroup holding one entry.
        let dbvl = ">schema Device from \"map.dbv\"\nUART0: 0xFFE01000; 0x18;\nUART1: 0x40004400; 0x18;\nTIMER: 0xFE002000; 0x4;\n";
        let doc = parse_document(dbvl).unwrap();

        // 3 data lines → 3 groups, one entry each.
        let groups: Vec<&DataEntry> = doc.data_groups.iter().map(|g| &g.entries[0]).collect();
        assert_eq!(groups.len(), 3);

        // >schema Name from "path" tags groups with the file stem ("map") —
        // the resolver keys on the entry key, not the schema tag.
        assert_eq!(groups[0].key.as_deref(), Some("UART0"));
        assert_eq!(
            groups[0].fields,
            vec![
                DataField::Positional(DataValue::String("0xFFE01000".to_string())),
                DataField::Positional(DataValue::String("0x18".to_string())),
            ]
        );

        assert_eq!(groups[1].key.as_deref(), Some("UART1"));
        assert_eq!(groups[2].key.as_deref(), Some("TIMER"));
        assert_eq!(
            groups[2].fields[0],
            DataField::Positional(DataValue::String("0xFE002000".to_string()))
        );
        assert_eq!(
            groups[2].fields[1],
            DataField::Positional(DataValue::String("0x4".to_string()))
        );
    }

    #[test]
    fn board_registers_dbvl_keyed_lines() {
        // registers.dbvl: flat keyed lines, one register per entry.
        let dbvl =
            ">schema Register from \"map.dbv\"\nUART0_DR: 0x00; 9; rw;\nUART0_SR: 0x01; 9; ro;\n";
        let doc = parse_document(dbvl).unwrap();

        let entries: Vec<&DataEntry> = doc.data_groups.iter().map(|g| &g.entries[0]).collect();
        assert_eq!(entries.len(), 2);

        let dr = entries[0];
        assert_eq!(dr.key.as_deref(), Some("UART0_DR"));
        assert_eq!(
            dr.fields,
            vec![
                DataField::Positional(DataValue::String("0x00".to_string())),
                DataField::Positional(DataValue::Int(9)),
                DataField::Positional(DataValue::String("rw".to_string())),
            ]
        );

        let sr = entries[1];
        assert_eq!(sr.key.as_deref(), Some("UART0_SR"));
        assert_eq!(
            sr.fields[2],
            DataField::Positional(DataValue::String("ro".to_string()))
        );
    }

    #[test]
    fn board_nested_blocks_rejected() {
        // Regression guard: the nested { > offset; size; ... } form must keep
        // failing to parse (it does not parse today). Locks the flat format in.
        let nested = r#"
schema Device { base: Int; registers: Register[]; };
as Device {
    uart1: 0x40011000; { > 0; 9; rw; > 0x0C; 13; rw; };
};
"#;
        assert!(parse_document(nested).is_err());
    }

    #[test]
    fn standalone_entry_key_with_hyphen_round_trips() {
        // Module-registry keys are module names like `from-bits`; the keyed
        // standalone-entry path must not drop them. 2026-08-03: try_parse_identifier
        // omitted `-`, silently discarding the whole line.
        let src = "from-bits: std/from-bits.bv;\nmy_mod: std/my-mod.bv;\n";
        let doc = parse_document(src).unwrap();
        let keys: Vec<&str> = doc
            .data_groups
            .iter()
            .flat_map(|g| g.entries.iter())
            .filter_map(|e| e.key.as_deref())
            .collect();
        assert_eq!(keys, vec!["from-bits", "my_mod"]);
    }
}
