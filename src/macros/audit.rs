// ── Static Capability Audit (briev audit) ────────────────────────────
// 2026-07-23: Scans .bv files for $ intrinsic usage and categorizes
// each call by severity. Reports capabilities required by each file.
//
// Severity levels:
//   HIGH   — ShellCmd$, HttpFetch$ (external execution / network)
//   MEDIUM — FileWrite$, FileRead$, ConfigGet$, SysQuery$ (I/O or query)
//   LOW    — All other $ intrinsics (AST navigation, strings, etc.)
//
// Flat control flow: max 2 levels deep.

use std::collections::BTreeMap;
use std::path::Path;

/// Severity of a detected $ intrinsic call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    High,
    Medium,
    Low,
}

/// A single detected $ intrinsic call in a file.
#[derive(Debug, Clone)]
pub struct IntrinsicUse {
    pub line: usize,
    pub snippet: String,
    pub intrinsic: String,
    pub severity: Severity,
}

/// All audit results for a single file.
#[derive(Debug, Clone)]
pub struct FileAudit {
    pub path: String,
    pub uses: Vec<IntrinsicUse>,
}

/// Map severity to a display label.
pub fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::High => "HIGH",
        Severity::Medium => "MEDIUM",
        Severity::Low => "LOW",
    }
}

/// Determine the severity of a $ intrinsic name.
fn intrinsic_severity(name: &str) -> Severity {
    match name {
        "ShellCmd$" | "HttpFetch$" => Severity::High,
        "FileWrite$" | "FileRead$" | "ConfigGet$" | "SysQuery$" | "EnvGet$" => Severity::Medium,
        _ => Severity::Low,
    }
}

/// Scan a single source line for $ intrinsic calls and return matches.
fn scan_line(line: &str, line_num: usize) -> Vec<IntrinsicUse> {
    let mut results = Vec::new();
    let mut pos = 0;
    let bytes = line.as_bytes();
    while pos < bytes.len() {
        // Look for pattern: <identifier>$(
        // Find '$' character
        let Some(dollar_pos) = bytes[pos..].iter().position(|&b| b == b'$') else {
            break;
        };
        let abs_dollar = pos + dollar_pos;

        // Check that the '$' is followed by '(' (a call) and that
        // there is at least one identifier char before '$'
        // (excludes bare `$(Stage)` syntax).
        if abs_dollar > 0
            && abs_dollar + 1 < bytes.len()
            && bytes[abs_dollar + 1] == b'('
            && bytes[abs_dollar - 1].is_ascii_alphanumeric()
        {
            // Extract the identifier before '$'
            let mut start = abs_dollar;
            while start > 0 {
                let c = bytes[start - 1];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    start -= 1;
                } else {
                    break;
                }
            }
            let name = String::from_utf8_lossy(&bytes[start..abs_dollar + 1]).to_string();

            // Walk forward to find the matching ')' for snippet extraction
            let snippet_end = bytes.len().min(abs_dollar + 41);
            let snippet = line[abs_dollar..snippet_end].to_string();

            let sev = intrinsic_severity(&name);
            results.push(IntrinsicUse {
                line: line_num,
                snippet,
                intrinsic: name,
                severity: sev,
            });
        }

        pos = abs_dollar + 1;
    }
    results
}

/// Audit a single .bv file for $ intrinsic usage.
pub fn audit_file(path: &str) -> Result<FileAudit, String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {}", path, e))?;
    let mut uses = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let line_num = i + 1;
        let mut found = scan_line(line, line_num);
        uses.append(&mut found);
    }
    Ok(FileAudit {
        path: path.to_string(),
        uses,
    })
}

/// Discover all .bv files under a given directory (recursive).
pub fn discover_bv_files(dir: &str) -> Vec<String> {
    let mut results = Vec::new();
    let base = Path::new(dir);
    if !base.is_dir() {
        return results;
    }
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir_path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("bv") {
                results.push(path.to_string_lossy().to_string());
            }
        }
    }
    results.sort();
    results
}

/// Run a full audit: scan plugins/ and the given source file path.
/// Returns a list of file audits, sorted by path.
pub fn run_audit(source_file: Option<&str>) -> Result<Vec<FileAudit>, String> {
    let mut results = BTreeMap::new();

    // Scan system plugins
    for (name, file_path) in crate::macros::lockfile::discover_plugin_files(None) {
        match audit_file(&file_path) {
            Ok(audit) => {
                results.insert(format!("system:{}", name), audit);
            }
            Err(e) => eprintln!("warning: {}", e),
        }
    }

    // Scan specified source file if provided
    if let Some(src) = source_file {
        match audit_file(src) {
            Ok(audit) => {
                results.insert(src.to_string(), audit);
            }
            Err(e) => eprintln!("warning: {}", e),
        }
    }

    // Scan lib/std/ directory for .bv files
    for path in discover_bv_files("lib/std") {
        if !results.contains_key(&path) {
            match audit_file(&path) {
                Ok(audit) => {
                    results.insert(path, audit);
                }
                Err(e) => eprintln!("warning: {}", e),
            }
        }
    }

    let mut sorted: Vec<FileAudit> = results.into_values().collect();
    sorted.sort_by_key(|a| a.path.clone());
    Ok(sorted)
}

/// Print the audit report to stdout.
pub fn print_audit(results: &[FileAudit]) {
    for file in results {
        println!("{}", file.path);
        for use_entry in &file.uses {
            let label = severity_label(use_entry.severity);
            println!(
                "  [{:>6}] {}:{}   {}",
                label, use_entry.line, use_entry.intrinsic, use_entry.snippet
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_line_empty() {
        let results = scan_line("Tag$(\"defn\")", 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].intrinsic, "Tag$");
        assert_eq!(results[0].severity, Severity::Low);
    }

    #[test]
    fn test_scan_line_multiple() {
        let results = scan_line("Tag$(\"x\").Count$()", 1);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].intrinsic, "Tag$");
        assert_eq!(results[1].intrinsic, "Count$");
    }

    #[test]
    fn test_scan_line_high_severity() {
        let results = scan_line("ShellCmd$(\"curl\", url)", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].intrinsic, "ShellCmd$");
        assert_eq!(results[0].severity, Severity::High);
        assert_eq!(results[0].line, 5);
    }

    #[test]
    fn test_scan_line_medium_severity() {
        let results = scan_line("FileWrite$(\"out.txt\", data)", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].intrinsic, "FileWrite$");
        assert_eq!(results[0].severity, Severity::Medium);
    }

    #[test]
    fn test_scan_line_no_intrinsic() {
        let results = scan_line("let x = 42;", 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_line_skips_stage_block() {
        // $(Stage) blocks should not be detected as $ intrinsics
        let results = scan_line("$(Parsed) { EmitInfo$(\"hi\"); }", 1);
        let names: Vec<&str> = results.iter().map(|r| r.intrinsic.as_str()).collect();
        assert_eq!(
            names,
            vec!["EmitInfo$"],
            "should only find EmitInfo$, not bare $("
        );
    }

    #[test]
    fn test_scan_line_no_dollar_call() {
        // "price$" is not followed by '(' so it should not match
        let results = scan_line("let price$ = 100;", 1);
        assert!(results.is_empty());
    }

    #[test]
    fn test_severity_label() {
        assert_eq!(severity_label(Severity::High), "HIGH");
        assert_eq!(severity_label(Severity::Medium), "MEDIUM");
        assert_eq!(severity_label(Severity::Low), "LOW");
    }

    #[test]
    fn test_intrinsic_severity_mapping() {
        assert_eq!(intrinsic_severity("ShellCmd$"), Severity::High);
        assert_eq!(intrinsic_severity("HttpFetch$"), Severity::High);
        assert_eq!(intrinsic_severity("FileWrite$"), Severity::Medium);
        assert_eq!(intrinsic_severity("FileRead$"), Severity::Medium);
        assert_eq!(intrinsic_severity("SysQuery$"), Severity::Medium);
        assert_eq!(intrinsic_severity("Tag$"), Severity::Low);
        assert_eq!(intrinsic_severity("StrLen$"), Severity::Low);
    }

    #[test]
    fn test_audit_file_not_found() {
        let result = audit_file("/nonexistent/file.bv");
        assert!(result.is_err());
    }

    #[test]
    fn test_audit_finds_intrinsics() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.bv");
        std::fs::write(
            &file_path,
            "$(Parsed) {\n    let data = FileRead$(\"x.txt\");\n    ShellCmd$(\"curl\");\n};",
        )
        .unwrap();

        let audit = audit_file(file_path.to_string_lossy().as_ref()).unwrap();
        assert_eq!(audit.uses.len(), 2);
        // Line 2: FileRead$
        assert_eq!(audit.uses[0].intrinsic, "FileRead$");
        assert_eq!(audit.uses[0].line, 2);
        // Line 3: ShellCmd$
        assert_eq!(audit.uses[1].intrinsic, "ShellCmd$");
        assert_eq!(audit.uses[1].line, 3);
        assert_eq!(audit.uses[1].severity, Severity::High);
    }
}
