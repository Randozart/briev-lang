// ── Text & Binary Operations ───────────────────────────────────────────
// 2026-07-21: Text selection operations for Source$ and Ir$ targets,
// and binary operations for Bin$ target. Both use a shared pattern
// engine for find/replace. Max 2 levels. Flat dispatch.

use std::path::Path;

/// A set of matched byte ranges in a text buffer.
#[derive(Debug, Clone)]
pub struct TextSelection {
    pub ranges: Vec<(usize, usize)>,
}

impl TextSelection {
    pub fn empty() -> Self {
        TextSelection { ranges: vec![] }
    }

    pub fn count(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Line numbers (1-indexed) for each matched range.
    pub fn lines(&self, text: &str) -> Vec<usize> {
        self.ranges
            .iter()
            .map(|(start, _)| text[..*start].chars().filter(|&c| c == '\n').count() + 1)
            .collect()
    }

    /// Take the first N matches.
    pub fn first(&self, n: usize) -> TextSelection {
        let limit = n.min(self.ranges.len());
        TextSelection {
            ranges: self.ranges[..limit].to_vec(),
        }
    }

    /// Take the last N matches.
    pub fn last(&self, n: usize) -> TextSelection {
        let start = self.ranges.len().saturating_sub(n);
        TextSelection {
            ranges: self.ranges[start..].to_vec(),
        }
    }

    /// Take the Nth match (0-indexed).
    pub fn nth(&self, n: usize) -> TextSelection {
        if n < self.ranges.len() {
            TextSelection {
                ranges: vec![self.ranges[n]],
            }
        } else {
            TextSelection::empty()
        }
    }

    /// Find all matches of a regex/literal pattern in text.
    pub fn find(text: &str, pattern: &str) -> TextSelection {
        let mut ranges = Vec::new();
        let mut search_start = 0;
        loop {
            let Some(pos) = text[search_start..].find(pattern) else {
                break;
            };
            let start = search_start + pos;
            let end = start + pattern.len();
            ranges.push((start, end));
            search_start = end;
        }
        TextSelection { ranges }
    }

    /// Replace each matched region with replacement text.
    pub fn replace_with(&self, text: &mut String, replacement: &str) {
        // Process in reverse order to preserve positions
        let mut ranges = self.ranges.clone();
        ranges.sort_by(|a, b| b.0.cmp(&a.0)); // descending by start
        for (start, end) in &ranges {
            text.drain(*start..*end);
            text.insert_str(*start, replacement);
        }
    }

    /// Insert text at the beginning of each matched region.
    pub fn insert_before(&self, text: &mut String, content: &str) {
        let mut starts: Vec<usize> = self.ranges.iter().map(|(s, _)| *s).collect();
        starts.sort_unstable_by(|a, b| b.cmp(a)); // descending
        for start in starts {
            text.insert_str(start, content);
        }
    }

    /// Insert text after each matched region.
    pub fn insert_after(&self, text: &mut String, content: &str) {
        let mut ends: Vec<usize> = self.ranges.iter().map(|(_, e)| *e).collect();
        ends.sort_unstable_by(|a, b| b.cmp(a)); // descending
        for end in ends {
            text.insert_str(end, content);
        }
    }

    /// Delete all matched regions.
    pub fn delete(&self, text: &mut String) {
        let mut ranges = self.ranges.clone();
        ranges.sort_by(|a, b| b.0.cmp(&a.0)); // descending by start
        for (start, end) in &ranges {
            text.drain(*start..*end);
        }
    }
}

/// Prepend text to the beginning of the buffer.
pub fn prepend(text: &mut String, content: &str) {
    text.insert_str(0, content);
}

/// Append text to the end of the buffer.
pub fn append(text: &mut String, content: &str) {
    text.push_str(content);
}

// ── Binary Operations ──────────────────────────────────────────────────

/// Get the path to the binary as a string.
pub fn binary_path(bin_path: &Path) -> String {
    bin_path.to_string_lossy().to_string()
}

/// Get the file size of the binary.
pub fn binary_size(bin_path: &Path) -> Result<u64, String> {
    bin_path
        .metadata()
        .map_err(|e| format!("cannot stat binary: {}", e))
        .map(|m| m.len())
}

/// Read bytes from the binary at given offset and length.
pub fn binary_read_bytes(bin_path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, String> {
    use std::fs::File;
    use std::io::Read;
    let mut file = File::open(bin_path).map_err(|e| format!("cannot open binary: {}", e))?;
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|e| format!("cannot seek binary: {}", e))?;
    let mut buf = vec![0u8; len];
    let n = file
        .read(&mut buf)
        .map_err(|e| format!("cannot read binary: {}", e))?;
    buf.truncate(n);
    Ok(buf)
}

/// Run an external command with {{path}} template substitution.
pub fn binary_run(bin_path: &Path, cmd_template: &str) -> Result<(), String> {
    let cmd_str = cmd_template.replace("{{path}}", &binary_path(bin_path));
    let parts: Vec<&str> = cmd_str.split_whitespace().collect();
    if parts.is_empty() {
        return Err("empty command".into());
    }
    let output = std::process::Command::new(parts[0])
        .args(&parts[1..])
        .output()
        .map_err(|e| format!("cannot run '{}': {}", cmd_str, e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("command '{}' failed: {}", cmd_str, stderr.trim()));
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_single() {
        let text = "hello world hello";
        let sel = TextSelection::find(text, "hello");
        assert_eq!(sel.count(), 2);
    }

    #[test]
    fn test_replace_with() {
        let mut text = "hello world".to_string();
        let sel = TextSelection::find(&text, "world");
        sel.replace_with(&mut text, "there");
        assert_eq!(text, "hello there");
    }

    #[test]
    fn test_insert_before() {
        let mut text = "hello world".to_string();
        let sel = TextSelection::find(&text, "world");
        sel.insert_before(&mut text, "brave ");
        assert_eq!(text, "hello brave world");
    }

    #[test]
    fn test_insert_after() {
        let mut text = "hello world".to_string();
        let sel = TextSelection::find(&text, "hello");
        sel.insert_after(&mut text, " there");
        assert_eq!(text, "hello there world");
    }

    #[test]
    fn test_delete() {
        let mut text = "hello world".to_string();
        let sel = TextSelection::find(&text, " world");
        sel.delete(&mut text);
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_lines() {
        let text = "line1\nline2\nline3";
        let sel = TextSelection::find(text, "line");
        let lines = sel.lines(text);
        assert_eq!(lines, vec![1, 2, 3]);
    }

    #[test]
    fn test_prepend_append() {
        let mut text = "middle".to_string();
        prepend(&mut text, "start ");
        append(&mut text, " end");
        assert_eq!(text, "start middle end");
    }

    #[test]
    fn test_first_last_nth() {
        let sel = TextSelection {
            ranges: vec![(0, 5), (10, 15), (20, 25)],
        };
        assert_eq!(sel.first(2).count(), 2);
        assert_eq!(sel.last(1).count(), 1);
        assert_eq!(sel.last(1).ranges[0], (20, 25));
        assert_eq!(sel.nth(0).ranges[0], (0, 5));
        assert!(sel.nth(10).is_empty());
    }

    #[test]
    fn test_no_match() {
        let sel = TextSelection::find("hello world", "zzz");
        assert!(sel.is_empty());
        assert_eq!(sel.count(), 0);
    }
}
