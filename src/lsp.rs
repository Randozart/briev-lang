// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

use crate::ast::TopLevel;
use crate::errors::{Diagnostic, ErrorMode, Severity, Span};
use crate::import_resolver;
use crate::parser;
use crate::proof_engine;
use crate::typechecker;
use lsp_server::{Connection, Message, Notification, Request, Response};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

/// Auto-launch configuration for LSP server
pub struct AutoLaunchConfig {
    pub verbose: bool,
}

impl Default for AutoLaunchConfig {
    fn default() -> Self {
        AutoLaunchConfig { verbose: false }
    }
}

/// Symbol table entry for LSP
struct SymbolEntry {
    name: String,
    kind: u32,
    uri: String,
    line: usize,
    column: usize,
    name_len: usize,
}

fn strip_codicil_blocks(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut output = Vec::new();
    let mut in_codicil_block = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "[route]" || trimmed == "[pre]" || trimmed == "[post]" {
            in_codicil_block = true;
            continue;
        }
        if in_codicil_block {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if !trimmed.starts_with('[')
                && !trimmed.starts_with("method")
                && !trimmed.starts_with("path")
                && !trimmed.starts_with("middleware")
                && !trimmed.starts_with("context")
                && !trimmed.starts_with("handler")
                && !trimmed.starts_with("response")
                && !trimmed.starts_with("params")
            {
                in_codicil_block = false;
            } else {
                continue;
            }
        }
        if !in_codicil_block {
            output.push(line);
        }
    }

    while output.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        output.pop();
    }

    output.join("\n")
}

pub struct LspServer {
    connection: Connection,
    documents: Arc<Mutex<DocumentStore>>,
    codicil_mode: bool,
}

struct DocumentStore {
    docs: HashMap<String, DocumentState>,
}

struct DocumentState {
    text: String,
    version: i32,
    program: Option<Vec<TopLevel>>,
}

impl LspServer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (connection, _) = Connection::stdio();

        Ok(LspServer {
            connection,
            documents: Arc::new(Mutex::new(DocumentStore {
                docs: HashMap::new(),
            })),
            codicil_mode: false,
        })
    }

    pub fn new_with_config(config: AutoLaunchConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let (connection, _) = Connection::stdio();

        if config.verbose {
            eprintln!("Briev Language Server started");
            eprintln!("  Features: hover, definition, completion, documentSymbol, workspaceSymbol");
        }

        Ok(LspServer {
            connection,
            documents: Arc::new(Mutex::new(DocumentStore {
                docs: HashMap::new(),
            })),
            codicil_mode: false,
        })
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let server_capabilities = serde_json::json!({
            "textDocumentSync": {
                "openClose": true,
                "change": 1, // Full
            },
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "completionProvider": {
                "resolveProvider": false,
                "triggerCharacters": [".", "#"]
            }
        });

        let initialization_params = self.connection.initialize(server_capabilities)?;
        info!("LSP initialized with params: {:?}", initialization_params);

        // Check if we're in a Codicil project
        if let Some(root_uri) = initialization_params
            .get("rootUri")
            .and_then(|v| v.as_str())
        {
            let root_path = root_uri.strip_prefix("file://").unwrap_or(root_uri);
            let mut check_path = std::path::PathBuf::from(root_path);
            while check_path.parent().is_some() {
                // Check for codicil.toml OR .codicil folder
                if check_path.join("codicil.toml").exists() || check_path.join(".codicil").exists()
                {
                    self.codicil_mode = true;
                    info!("Codicil project detected - Codicil mode enabled");
                    // Try to load .codicil/config.toml for additional settings
                    if let Ok(config) =
                        std::fs::read_to_string(check_path.join(".codicil/config.toml"))
                    {
                        info!("Loaded Codicil config: {}", config);
                    }
                    break;
                }
                if !check_path.pop() {
                    break;
                }
            }
        }

        loop {
            let msg = self.connection.receiver.recv()?;
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    self.handle_request(req);
                }
                Message::Response(resp) => self.handle_response(resp),
                Message::Notification(notif) => self.handle_notification(notif),
            }
        }
    }

    fn handle_request(&self, req: Request) {
        match req.method.as_str() {
            "textDocument/hover" => {
                if let Ok(params) = serde_json::from_value(req.params) {
                    self.handle_hover(req.id, params);
                }
            }
            "textDocument/definition" => {
                if let Ok(params) = serde_json::from_value(req.params) {
                    self.handle_definition(req.id, params);
                }
            }
            "textDocument/completion" => {
                if let Ok(params) = serde_json::from_value(req.params) {
                    self.handle_completion(req.id, params);
                }
            }
            "textDocument/documentSymbol" => {
                self.handle_document_symbol(req.id, req.params);
            }
            "workspace/symbol" => {
                self.handle_workspace_symbol(req.id, req.params);
            }
            _ => {
                warn!("Unknown request method: {}", req.method);
            }
        }
    }

    fn handle_notification(&mut self, notif: Notification) {
        match notif.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(params) = serde_json::from_value(notif.params) {
                    self.handle_did_open_json(params);
                }
            }
            "textDocument/didChange" => {
                if let Ok(params) = serde_json::from_value(notif.params) {
                    self.handle_did_change_json(params);
                }
            }
            _ => {
                // Ignore unknown notifications
            }
        }
    }

    fn handle_did_open_json(&mut self, params: Value) {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let text = params["textDocument"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let version = params["textDocument"]["version"].as_i64().unwrap_or(0) as i32;

        {
            let mut docs = self.documents.lock().unwrap();
            docs.docs.insert(
                uri.clone(),
                DocumentState {
                    text: text.clone(),
                    version,
                    program: None,
                },
            );
        }

        self.check_document(&uri, &text);
    }

    fn handle_did_change_json(&mut self, params: Value) {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let version = params["textDocument"]["version"].as_i64().unwrap_or(0) as i32;
        let text = params["contentChanges"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        {
            let mut docs = self.documents.lock().unwrap();
            if let Some(doc) = docs.docs.get_mut(&uri) {
                doc.text = text.clone();
                doc.version = version;
            } else {
                return;
            }
        }

        self.check_document(&uri, &text);
    }

    fn check_document(&self, uri: &str, text: &str) {
        let (diagnostics, program) = self.run_type_check(uri, text);

        {
            let mut docs = self.documents.lock().unwrap();
            if let Some(doc) = docs.docs.get_mut(uri) {
                doc.program = program;
            }
        }

        let params = serde_json::json!({
            "uri": uri,
            "diagnostics": diagnostics
        });

        let notif = Notification::new("textDocument/publishDiagnostics".to_string(), params);
        let _ = self.connection.sender.send(Message::Notification(notif));
    }

    fn run_type_check(&self, uri: &str, text: &str) -> (Vec<Value>, Option<Vec<TopLevel>>) {
        let is_rbv = uri.ends_with(".rbv");
        let is_dbriev = uri.ends_with(".dbv") || uri.ends_with(".dbvl");

        if is_dbriev {
            let mut diagnostics = Vec::new();
            if let Err(e) = crate::dbriev::v2::parse_document(text).map(|_| ()) {
                diagnostics.push(serde_json::json!({
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    },
                    "severity": 1,
                    "source": "dbriev-parser",
                    "message": e,
                }));
            }
            return (diagnostics, None);
        }

        if self.codicil_mode && !is_rbv {
            info!("Codicil mode enabled - ignoring [route], [pre], [post] blocks");
        }

        let source = self.extract_briev_source(text, is_rbv, self.codicil_mode);

        let tokens = {
            use logos::Logos;
            crate::lexer::Token::lexer(source.as_str()).filter_map(|t| t.ok()).zip(0..).map(|(t, i)| (t, i..i+1)).collect::<Vec<_>>()
        };
        let mut parser = parser::Parser::new(tokens, &source);
        let mut program = match parser.parse_program() {
            Ok(p) => p,
            Err(e) => {
                let diag = self.syntax_error_to_json(&e);
                return (vec![diag], None);
            }
        };

        let type_diagnostics = Vec::new();
        let proof_errors = Vec::new();

        let mut diagnostics = Vec::new();

        for diag in type_diagnostics {
            diagnostics.push(self.diagnostic_to_json(&diag));
        }

        for err in proof_errors {
            diagnostics.push(self.proof_error_to_json(&err));
        }

        (diagnostics, Some(program))
    }

    fn extract_briev_source(&self, source: &str, is_rbv: bool, codicil_mode: bool) -> String {
        if !is_rbv {
            if codicil_mode {
                return strip_codicil_blocks(source);
            }
            return source.to_string();
        }

        let mut output = String::with_capacity(source.len());
        let mut in_script = false;
        let mut current_pos = 0;

        while current_pos < source.len() {
            if !in_script {
                if source[current_pos..].starts_with("<script") {
                    let after_script = &source[current_pos + 7..];
                    let next_char = after_script.chars().next();
                    let is_real_script_tag = next_char.is_none()
                        || next_char == Some('>')
                        || next_char == Some(' ')
                        || next_char == Some('\t')
                        || next_char == Some('\n');

                    if is_real_script_tag {
                        if let Some(tag_end_rel) = source[current_pos..].find('>') {
                            let tag_end = current_pos + tag_end_rel + 1;
                            // Mask the <script ...> tag itself byte-by-byte
                            for c in source[current_pos..tag_end].chars() {
                                if c == '\n' {
                                    output.push('\n');
                                } else {
                                    // Use same number of bytes as the character
                                    for _ in 0..c.len_utf8() {
                                        output.push(' ');
                                    }
                                }
                            }
                            current_pos = tag_end;
                            in_script = true;
                            continue;
                        }
                    }
                }
                // Outside script, mask everything byte-by-byte
                let c = source[current_pos..].chars().next().unwrap();
                if c == '\n' {
                    output.push('\n');
                } else {
                    for _ in 0..c.len_utf8() {
                        output.push(' ');
                    }
                }
                current_pos += c.len_utf8();
            } else {
                if source[current_pos..].starts_with("</script>") {
                    in_script = false;
                    // Mask the </script> tag itself byte-by-byte
                    for c in "</script>".chars() {
                        if c == '\n' {
                            output.push('\n');
                        } else {
                            for _ in 0..c.len_utf8() {
                                output.push(' ');
                            }
                        }
                        current_pos += c.len_utf8();
                    }
                    continue;
                }
                // Inside script, keep characters as they are
                let c = source[current_pos..].chars().next().unwrap();
                output.push(c);
                current_pos += c.len_utf8();
            }
        }
        output
    }

    fn syntax_error_to_json(&self, err: &crate::errors::SyntaxError) -> Value {
        use crate::errors::SyntaxError;
        let (message, span) = match err {
            SyntaxError::UnexpectedToken {
                expected,
                found,
                span,
            } => (format!("Expected {}, found {}", expected, found), *span),
            SyntaxError::UnexpectedEOF { expected, span } => {
                (format!("Expected {}, found EOF", expected), *span)
            }
            SyntaxError::InvalidExpression { reason, span } => {
                (format!("Invalid expression: {}", reason), *span)
            }
            SyntaxError::InvalidStatement { reason, span } => {
                (format!("Invalid statement: {}", reason), *span)
            }
            SyntaxError::InvalidType { type_name, span } => {
                (format!("Invalid type: {}", type_name), *span)
            }
            SyntaxError::StagedFeature { feature, span } => (
                format!("Staged feature '{}' is normative but not yet implemented", feature),
                *span,
            ),
            SyntaxError::ReservedKeyword { keyword, span } => (
                format!("'{}' is a reserved keyword and cannot be used as an identifier", keyword),
                *span,
            ),
            SyntaxError::BacktrackLimit { span } => (
                "Input is too deeply nested to parse (backtracking limit reached)".to_string(),
                *span,
            ),
            SyntaxError::ParseErrors(_) => (
                "multiple parse errors".to_string(),
                Span::new(0, 0, 0, 0),
            ),
        };

        serde_json::json!({
            "range": {
                "start": { "line": span.line.saturating_sub(1), "character": span.column.saturating_sub(1) },
                "end": { "line": span.line.saturating_sub(1), "character": span.column + 1 }
            },
            "severity": 1,
            "source": "briev-parser",
            "message": message
        })
    }

    fn diagnostic_to_json(&self, diag: &Diagnostic) -> Value {
        let severity = match diag.severity {
            Severity::Error => 1,
            Severity::Warning => 2,
            Severity::Info => 3,
            Severity::Note => 4,
        };

        let range = if let Some(span) = diag.span {
            serde_json::json!({
                "start": { "line": span.line.saturating_sub(1), "character": span.column.saturating_sub(1) },
                "end": { "line": span.line.saturating_sub(1), "character": span.column + 1 }
            })
        } else {
            serde_json::json!({
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 1 }
            })
        };

        let mut message = diag.title.clone();
        if !diag.explanation.is_empty() {
            message.push_str("\n\n");
            message.push_str(&diag.explanation.join("\n"));
        }
        if !diag.hints.is_empty() {
            message.push_str("\n\nhint: ");
            message.push_str(&diag.hints.join("\n"));
        }

        serde_json::json!({
            "range": range,
            "severity": severity,
            "code": diag.code,
            "source": "briev",
            "message": message
        })
    }

    fn proof_error_to_json(&self, err: &crate::errors::ProofError) -> Value {
        use crate::errors::ProofError;
        let (span, msg) = match err {
            ProofError::UnreachableState { span, reason, .. } => (*span, format!("unreachable state: {}", reason)),
            ProofError::PostconditionUnsatisfiable { span, reason, .. } => (*span, format!("postcondition unsatisfiable: {}", reason)),
            ProofError::NoAcceptingPath { span, reason, .. } => (*span, format!("no accepting path: {}", reason)),
            ProofError::MutualExclusionViolation { span, .. } => (*span, "mutual exclusion violation".to_string()),
            ProofError::UnhandledOutcome { span, .. } => (*span, "unhandled outcome".to_string()),
            ProofError::TrueAssertionFailure { span, reason, .. } => (*span, format!("true assertion failure: {}", reason)),
            ProofError::CircularDependency { span, .. } => (*span, "circular dependency".to_string()),
            ProofError::ImpossiblePrecondition { span, condition, .. } => (*span, format!("impossible precondition: {}", condition)),
            ProofError::PostconditionMutationViolation { span, explanation, .. } => (*span, format!("postcondition mutation: {}", explanation)),
            ProofError::TrivialPrecondition { span, .. } => (*span, "trivial precondition".to_string()),
            ProofError::TrivialPostcondition { span, .. } => (*span, "trivial postcondition".to_string()),
        };
        let range = serde_json::json!({
            "start": { "line": span.line.saturating_sub(1), "character": span.column.saturating_sub(1) },
            "end": { "line": span.line.saturating_sub(1), "character": span.column + 1 }
        });
        serde_json::json!({
            "range": range,
            "severity": 1,
            "code": "proof",
            "source": "briev-proof",
            "message": msg
        })
    }

    fn handle_completion(&self, id: lsp_server::RequestId, _params: Value) {
        // 2026-08-05 (Phase 1): completions derive from the canonical language
        // vocab (`src/vocab.rs`), not a hand-maintained list. The lexer, LSP,
        // highlighter, and formatter must agree on the vocabulary.
        let vocab = crate::vocab::LanguageVocab::canonical();
        let mut keywords: Vec<String> = vocab
            .canonical_keywords()
            .map(|k| k.name.clone())
            .collect();
        keywords.extend(vocab.intrinsics.iter().cloned());
        // Bootstrap type names and operand hashwords are offered as bare
        // completions (the lexer tokenizes them as identifiers/hashwords).
        keywords.extend(
            ["Int", "Float", "Bool", "String", "Char", "Void", "Ptr", "Bits"]
                .iter()
                .map(|s| s.to_string()),
        );
        keywords.sort();
        keywords.dedup();

        // Add Codicil-specific completions when in Codicil mode
        if self.codicil_mode {
            keywords.extend(vec![
                "[route]".to_string(),
                "[pre]".to_string(),
                "[post]".to_string(),
                "method = \"GET\"".to_string(),
                "method = \"POST\"".to_string(),
                "method = \"PUT\"".to_string(),
                "method = \"DELETE\"".to_string(),
                "method = \"PATCH\"".to_string(),
                "path = \"/\"".to_string(),
                "middleware = []".to_string(),
                "context = \"server\"".to_string(),
                "response.status".to_string(),
                "response.body".to_string(),
                "params.".to_string(),
            ]);
        }

        let completions: Vec<Value> = keywords
            .into_iter()
            .map(|k| {
                serde_json::json!({
                    "label": k,
                    "kind": 14, // Keyword
                })
            })
            .collect();

        let resp = Response::new_ok(id, completions);
        let _ = self.connection.sender.send(Message::Response(resp));
    }

    fn handle_hover(&self, id: lsp_server::RequestId, params: Value) {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        let line = params["position"]["line"].as_u64().unwrap_or(0) as usize + 1;
        let character = params["position"]["character"].as_u64().unwrap_or(0) as usize + 1;

        let docs = self.documents.lock().unwrap();
        if let Some(doc) = docs.docs.get(uri) {
            if let Some(program) = &doc.program {
                for item in program {
                    if let Some(span) = item_span(item) {
                        let name = item_name(item);
                        if line == span.line
                            && character >= span.column
                            && character <= span.column + name.len()
                        {
                            let content = format!("**{}**\n\n{}", name, item_description(item));
                            let result = serde_json::json!({
                                "contents": {
                                    "kind": "markdown",
                                    "value": content
                                }
                            });
                            let resp = Response::new_ok(id, result);
                            let _ = self.connection.sender.send(Message::Response(resp));
                            return;
                        }
                    }
                }
            }
        }

        let resp = Response::new_ok(id, serde_json::Value::Null);
        let _ = self.connection.sender.send(Message::Response(resp));
    }

    fn handle_definition(&self, id: lsp_server::RequestId, params: Value) {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        let line = params["position"]["line"].as_u64().unwrap_or(0) as usize + 1;
        let character = params["position"]["character"].as_u64().unwrap_or(0) as usize + 1;

        let docs = self.documents.lock().unwrap();
        if let Some(doc) = docs.docs.get(uri) {
            if let Some(program) = &doc.program {
                for item in program {
                    if let Some(span) = item_span(item) {
                        let name = item_name(item);
                        if line == span.line
                            && character >= span.column
                            && character <= span.column + name.len()
                        {
                            let result = serde_json::json!({
                                "uri": uri,
                                "range": {
                                    "start": { "line": span.line - 1, "character": span.column - 1 },
                                    "end": { "line": span.line - 1, "character": span.column + name.len() - 1 }
                                }
                            });
                            let resp = Response::new_ok(id, result);
                            let _ = self.connection.sender.send(Message::Response(resp));
                            return;
                        }
                    }
                }
            }
        }

        let resp = Response::new_ok(id, serde_json::Value::Null);
        let _ = self.connection.sender.send(Message::Response(resp));
    }

    fn handle_response(&self, _resp: Response) {}

    /// Build a symbol table from a program for a given URI
    fn build_symbol_table(&self, program: &[TopLevel], uri: &str) -> Vec<SymbolEntry> {
        program.iter().filter_map(|item| {
            let name = item_name(item);
            let span = item_span(item)?;
            let kind: u32 = match item {
                TopLevel::Transaction(_) => 6,    // Method
                TopLevel::StateDecl(_) => 13,      // Variable
                TopLevel::Trigger(_) => 25,        // Event
                TopLevel::Obj(_) => 23,         // Struct
                TopLevel::Enum(_) => 10,           // Module
                TopLevel::ForeignBinding(_) => 24, // Operator
                TopLevel::Definition(_) => 12,     // Function
                TopLevel::Constant(_) => 14,       // Constant
                TopLevel::Signature(_) => 12,      // Function
                TopLevel::Init(_) => 14,            // Constant (runtime-seeded)
                _ => return None,
            };
            let name_len = name.len();
            Some(SymbolEntry {
                name,
                kind,
                uri: uri.to_string(),
                line: span.line,
                column: span.column,
                name_len,
            })
        }).collect()
    }

    /// Handle textDocument/documentSymbol request
    fn handle_document_symbol(&self, id: lsp_server::RequestId, params: Value) {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
        let docs = self.documents.lock().unwrap();
        let result: Vec<Value> = if let Some(doc) = docs.docs.get(&uri) {
            if let Some(program) = &doc.program {
                let symbols = self.build_symbol_table(program, &uri);
                symbols.iter().map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "kind": s.kind,
                        "location": {
                            "uri": s.uri,
                            "range": {
                                "start": { "line": s.line - 1, "character": s.column - 1 },
                                "end": { "line": s.line - 1, "character": s.column + s.name_len - 1 }
                            }
                        }
                    })
                }).collect()
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        let resp = Response::new_ok(id, result);
        let _ = self.connection.sender.send(Message::Response(resp));
    }

    /// Handle workspace/symbol request
    fn handle_workspace_symbol(&self, id: lsp_server::RequestId, params: Value) {
        let query = params["query"].as_str().unwrap_or("").to_lowercase();
        let docs = self.documents.lock().unwrap();
        let mut result: Vec<Value> = Vec::new();

        for (uri, doc) in &docs.docs {
            if let Some(program) = &doc.program {
                let symbols = self.build_symbol_table(program, uri);
                for sym in symbols {
                    if query.is_empty() || sym.name.to_lowercase().contains(&query) {
                        result.push(serde_json::json!({
                            "name": sym.name,
                            "kind": sym.kind,
                            "location": {
                                "uri": sym.uri,
                                "range": {
                                    "start": { "line": sym.line - 1, "character": sym.column - 1 },
                                    "end": { "line": sym.line - 1, "character": sym.column + sym.name_len - 1 }
                                }
                            }
                        }));
                    }
                }
            }
        }

        let resp = Response::new_ok(id, result);
        let _ = self.connection.sender.send(Message::Response(resp));
    }
}

fn item_span(item: &TopLevel) -> Option<Span> {
    match item {
        TopLevel::Transaction(t) => t.span,
        TopLevel::StateDecl(s) => s.span,
        TopLevel::Trigger(t) => t.span,
        TopLevel::Obj(s) => s.span,
        TopLevel::Enum(e) => e.span,
        TopLevel::ForeignBinding(fb) => fb.span,
        TopLevel::Definition(d) => d.contract.span,
        TopLevel::Init(i) => i.span,
        _ => None,
    }
}

fn item_name(item: &TopLevel) -> String {
    match item {
        TopLevel::Signature(s) => s.name.clone(),
        TopLevel::Definition(d) => d.name.clone(),
        TopLevel::Transaction(t) => t.name.clone(),
        TopLevel::StateDecl(s) => s.name.clone(),
        TopLevel::Trigger(t) => t.name.clone(),
        TopLevel::Constant(c) => c.name.clone(),
        TopLevel::Init(i) => i.name.clone(),
        TopLevel::Obj(s) => s.name.clone(),
        TopLevel::Enum(e) => e.name.clone(),
        TopLevel::ForeignBinding(fb) => fb.effective_briev_name().to_string(),
        _ => "unnamed".to_string(),
    }
}

fn item_description(item: &TopLevel) -> String {
    match item {
        TopLevel::Transaction(t) => format!(
            "transaction{}{}",
            if t.is_async { " async" } else { "" },
            if t.is_reactive { " rct" } else { "" }
        ),
        TopLevel::StateDecl(_) => "state variable".to_string(),
        TopLevel::Trigger(_) => "hardware trigger".to_string(),
        TopLevel::Signature(_) => "function signature".to_string(),
        TopLevel::Definition(_) => "function definition".to_string(),
        TopLevel::Constant(_) => "constant".to_string(),
        TopLevel::Obj(_) => "struct".to_string(),
        TopLevel::Enum(_) => "enum".to_string(),
        TopLevel::ForeignBinding(_) => "foreign binding".to_string(),
        _ => "".to_string(),
    }
}

pub fn run_lsp_server(_mode: ErrorMode) {
    let mut server = LspServer::new().expect("Failed to create LSP server");
    if let Err(e) = server.run() {
        eprintln!("LSP server error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_briev_source_rbv() {
        let lsp = LspServer {
            connection: Connection::stdio().0,
            documents: Arc::new(Mutex::new(DocumentStore {
                docs: HashMap::new(),
            })),
            codicil_mode: false,
        };

        let rbv_source = r#"
<script type="briev">
let x: Int = 10;
</script>
<view>
  <div>Test</div>
</view>
"#;
        let extracted = lsp.extract_briev_source(rbv_source, true, false);

        // The script tag should be replaced by spaces/newlines
        assert!(extracted.contains("let x: Int = 10;"));
        assert!(!extracted.contains("<script"));
        assert!(!extracted.contains("<view>"));
        assert!(!extracted.contains("<div>"));

        // Lines should be preserved
        let original_lines: Vec<&str> = rbv_source.lines().collect();
        let extracted_lines: Vec<&str> = extracted.lines().collect();
        assert_eq!(original_lines.len(), extracted_lines.len());

        // Line 3 (1-based) should contain the code
        assert!(extracted_lines[2].contains("let x: Int = 10;"));
    }

    #[test]
    fn test_extract_briev_source_rbv_with_other_tags() {
        let lsp = LspServer {
            connection: Connection::stdio().0,
            documents: Arc::new(Mutex::new(DocumentStore {
                docs: HashMap::new(),
            })),
            codicil_mode: false,
        };

        let rbv_source = r#"
<p>This is <scripting> test</p>
<script>
let x = 1;
</script>
"#;
        let extracted = lsp.extract_briev_source(rbv_source, true, false);

        // <scripting> should be masked
        assert!(!extracted.contains("<scripting>"));
        // let x = 1; should be preserved
        assert!(extracted.contains("let x = 1;"));
    }

    #[test]
    fn test_extract_briev_source_rbv_byte_accuracy() {
        let lsp = LspServer {
            connection: Connection::stdio().0,
            documents: Arc::new(Mutex::new(DocumentStore {
                docs: HashMap::new(),
            })),
            codicil_mode: false,
        };

        // Source with multi-byte character (🦀 is 4 bytes)
        let rbv_source = "🦀<script>let x = 1;</script>";
        let extracted = lsp.extract_briev_source(rbv_source, true, false);

        assert_eq!(rbv_source.len(), extracted.len());
        assert!(extracted.contains("let x = 1;"));

        // Find position of "let" in both
        let original_pos = rbv_source.find("let").unwrap();
        let extracted_pos = extracted.find("let").unwrap();
        assert_eq!(original_pos, extracted_pos);
    }
}
