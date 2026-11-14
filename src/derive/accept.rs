// ── Phase I.1 — `briev accept` Command ────────────────────────────────
// 2026-07-28: Fold doppelganger bodies from .derive.bv / .opt.bv into source.
// Never mutates source without .bak backup. Flat code: max 2 nesting.

use crate::ast::TopLevel;
use crate::derive::doppelganger::Doppelganger;
use std::fs;
use std::path::Path;

/// Handle `briev accept <file>` — fold doppelganger bodies into source.
/// 1. Find derivation blocks in the original source (by span)
/// 2. Read the doppelganger file (.derive.bv or .opt.bv)
/// 3. For each derivation block, extract the synthesized body from the doppelganger
/// 4. Replace `:= { ... };` with `{ body; };` in the original source
/// 5. Write result with .bak backup
pub fn handle_accept_command(file_path: &str, use_opt: bool) -> Result<(), String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("file not found: '{}'", file_path));
    }

    // Read and parse original source to find derivation blocks
    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {}", file_path, e))?;
    let token_spans = lex_source_with_spans(&source)?;
    let program = parse_tokens(&token_spans, &source)?;

    // Collect derivation blocks: (name, start_byte_of_:=, end_byte_after_;)
    let mut derivations: Vec<(String, usize, usize)> = Vec::new();
    for item in &program {
        let (name, derivation) = match item {
            TopLevel::Definition(d) => (d.name.as_str(), d.derivation.as_ref()),
            TopLevel::Transaction(t) => (t.name.as_str(), t.derivation.as_ref()),
            _ => continue,
        };
        if let Some(block) = derivation {
            // The derivation block span covers everything from `:=` through `};`
            let span_start = block.span.start;
            let span_end = block.span.end;
            // Find the `:=` within the span to know exact replacement bounds
            let block_text = &source[span_start..span_end];
            if let Some(eq_pos) = block_text.find(":=") {
                let replace_start = span_start + eq_pos;
                derivations.push((name.to_string(), replace_start, span_end));
            } else {
                // Fallback: use entire span (shouldn't happen with valid syntax)
                derivations.push((name.to_string(), span_start, span_end));
            }
        }
    }

    if derivations.is_empty() {
        eprintln!("[accept] no derivation blocks found in '{}'", file_path);
        return Ok(());
    }

    // Locate and read the doppelganger
    let shadow_path = if use_opt {
        Doppelganger::opt_path_for(path)
    } else {
        Doppelganger::derive_path_for(path)
    };
    if !shadow_path.exists() {
        return Err(format!(
            "no doppelganger at '{}' — run 'briev derive' first",
            shadow_path.display()
        ));
    }
    let shadow_source = fs::read_to_string(&shadow_path)
        .map_err(|e| format!("cannot read '{}': {}", shadow_path.display(), e))?;
    let shadow_spans = lex_source_with_spans(&shadow_source)?;
    let shadow_program = parse_tokens(&shadow_spans, &shadow_source)?;

    // For each derivation, find the body text in the doppelganger.
    // The doppelganger has `{ body; } := { derivation; };` — extract body by
    // scanning from span_start in the doppelganger to the `:=` marker.
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for (name, orig_start, orig_end) in &derivations {
        // In the doppelganger, text was inserted at `block.span.start`
        // making the doppelganger's `:=` shift forward by the body length.
        // Scan from the original start position in the shadow source to find `:=`.
        if *orig_start >= shadow_source.len() {
            eprintln!("warn: '{}' span out of range in doppelganger", name);
            continue;
        }
        let after = &shadow_source[*orig_start..];
        if let Some(eq_pos) = after.find(":=") {
            // Body is everything from orig_start up to `:=` in the shadow
            let body_text = &after[..eq_pos];
            // Trim the ` { ` prefix and ` } ` suffix that write_doppelganger adds
            let trimmed = body_text.trim();
            // Remove leading `{` and trailing `}` if present
            let clean_body = trimmed
                .strip_prefix('{')
                .unwrap_or(trimmed)
                .strip_suffix('}')
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            // The replacement is ` { body; };` — same as a normal function body
            let replacement = format!(" {{\n    {};\n}};", clean_body);
            replacements.push((*orig_start, *orig_end, replacement));
        }
    }

    if replacements.is_empty() {
        return Err("no bodies found in doppelganger — did 'briev derive' succeed?".into());
    }

    // Sort in reverse order to preserve byte offsets during surgery
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));

    // Create .bak backup before modifying
    let backup_path = path.with_extension("bv.bak");
    fs::copy(path, &backup_path)
        .map_err(|e| format!("cannot create backup '{}': {}", backup_path.display(), e))?;

    // Apply all replacements
    let mut result = source.clone();
    for (start, end, body) in &replacements {
        let prefix = &result[..*start];
        let suffix = &result[*end..];
        result = format!("{}{}{}", prefix, body, suffix);
    }

    fs::write(path, &result).map_err(|e| format!("cannot write '{}': {}", file_path, e))?;
    eprintln!(
        "[accept] folded {} body(ies) into '{}' (backup at '{}')",
        replacements.len(),
        file_path,
        backup_path.display()
    );

    Ok(())
}

/// Lex a Briev source file into (Token, byte_range) pairs.
fn lex_source_with_spans(
    source: &str,
) -> Result<Vec<(crate::lexer::Token, std::ops::Range<usize>)>, String> {
    use logos::Logos;
    let mut lexer = crate::lexer::Token::lexer(source);
    let mut result = Vec::new();
    while let Some(token_result) = lexer.next() {
        match token_result {
            Ok(token) => {
                let span = lexer.span();
                result.push((token, span));
            }
            Err(_) => return Err("lex error".to_string()),
        }
    }
    Ok(result)
}

/// Parse tokens with spans into a program.
fn parse_tokens(
    token_spans: &[(crate::lexer::Token, std::ops::Range<usize>)],
    source: &str,
) -> Result<Vec<TopLevel>, String> {
    let mut parser = crate::parser::Parser::new(token_spans.to_vec(), source);
    parser
        .parse_program()
        .map_err(|e| format!("parse error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a temporary .bv file with a derivation block.
    fn write_temp_bv(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.bv");
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}", content).unwrap();
        (dir, path)
    }

    #[test]
    fn test_accept_no_derivation_blocks() {
        let content = "defn add(a: Int, b: Int) -> Int { a + b; };";
        let (_dir, path) = write_temp_bv(content);
        let result = handle_accept_command(path.to_str().unwrap(), false);
        // No derivation blocks — should succeed with a message
        assert!(result.is_ok());
    }

    #[test]
    fn test_accept_no_doppelganger() {
        // Parser expects `{ body } := { derivation }` order
        let content = "defn add(a: Int, b: Int) -> Int { a + b; } := { 2, 3 -> 5; };";
        let (_dir, path) = write_temp_bv(content);
        let result = handle_accept_command(path.to_str().unwrap(), false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("doppelganger"));
    }

    #[test]
    fn test_accept_nonexistent_file() {
        let result = handle_accept_command("/nonexistent/path.bv", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("file not found"));
    }
}
