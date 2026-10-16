// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Console diagnostic blocks (plan 2026-10-16-term-semantics-and-tails §5).
//!
//! Serious diagnostics render as their own block (what/why/fix + sites);
//! composite analyses collect into ONE block. NEW diagnostics are born
//! here; the ~235 scattered eprintln sites migrate MIGRATE-WHEN-TOUCHED
//! (Rule 18). Pure renderers — the pipeline owns when strings reach the
//! console.

use crate::errors::Severity;

/// One reported location inside a block. `context` is a single source line
/// (or a short descriptor when the site is not a source span — e.g. a
/// synthetic monitor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub context: String,
}

impl Site {
    pub fn new(path: &str, line: usize, column: usize, context: &str) -> Self {
        Site {
            path: path.to_string(),
            line,
            column,
            context: context.to_string(),
        }
    }
}

/// A console diagnostic block: severity header + what, why (proof where
/// one exists), fix, and an optional multi-site list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub severity: Severity,
    pub what: String,
    pub why: Vec<String>,
    pub fix: Vec<String>,
    pub sites: Vec<Site>,
}

impl Block {
    pub fn new(severity: Severity, what: impl Into<String>) -> Self {
        Block {
            severity,
            what: what.into(),
            why: Vec::new(),
            fix: Vec::new(),
            sites: Vec::new(),
        }
    }

    pub fn warning(what: impl Into<String>) -> Self {
        Block::new(Severity::Warning, what)
    }

    pub fn why(mut self, line: impl Into<String>) -> Self {
        self.why.push(line.into());
        self
    }

    pub fn fix(mut self, line: impl Into<String>) -> Self {
        self.fix.push(line.into());
        self
    }

    pub fn site(mut self, site: Site) -> Self {
        self.sites.push(site);
        self
    }

    /// The block text. First line is `severity: what` so existing
    /// stderr-grepping tooling keeps matching the `warning:` prefix.
    pub fn render(&self) -> String {
        let mut out = format!("{}: {}\n", severity_label(self.severity), self.what);
        for line in &self.why {
            out.push_str(&format!("  why:  {line}\n"));
        }
        for line in &self.fix {
            out.push_str(&format!("  fix:  {line}\n"));
        }
        if !self.sites.is_empty() {
            out.push_str("  sites:\n");
            for s in &self.sites {
                out.push_str(&format!(
                    "    {}:{}:{}  {}\n",
                    s.path, s.line, s.column, s.context
                ));
            }
        }
        out
    }
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Info => "info",
    }
}

/// Collects blocks and composite notes during a compile; the pipeline
/// flushes once at the end (errors print immediately at their sites — the
/// sink carries warnings + notes). Deterministic order: insertion order.
#[derive(Debug, Default)]
pub struct Sink {
    pub warnings: Vec<Block>,
    pub notes: Vec<String>,
}

impl Sink {
    pub fn new() -> Self {
        Sink::default()
    }

    pub fn warn(&mut self, block: Block) {
        self.warnings.push(block);
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    pub fn is_empty(&self) -> bool {
        self.warnings.is_empty() && self.notes.is_empty()
    }

    /// One composite notes block (when any notes exist) followed by each
    /// warning as its own block. Empty sink renders nothing.
    pub fn render_flush(&self) -> String {
        let mut out = String::new();
        if !self.notes.is_empty() {
            out.push_str(&format!("info: analysis notes ({})\n", self.notes.len()));
            for n in &self.notes {
                out.push_str(&format!("  - {n}\n"));
            }
        }
        for w in &self.warnings {
            out.push_str(&w.render());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_block_what_why_fix_layout() {
        let b = Block::warning("no path ends this program")
            .why("walked the live program; no terminator reachable")
            .fix("add `when <done> { endprogram; };` or `when true { reside; };`");
        let r = b.render();
        assert!(r.starts_with("warning: no path ends this program\n"));
        assert!(r.contains("  why:  walked the live program"));
        assert!(r.contains("  fix:  add `when <done>"));
        assert!(!r.contains("sites:"), "no sites section without sites");
    }

    #[test]
    fn sites_section_lists_every_site_in_order() {
        let b = Block::warning("two sites")
            .site(Site::new("a.bv", 3, 5, "node main"))
            .site(Site::new("b.bv", 9, 1, "defn helper"));
        let r = b.render();
        assert!(r.contains("  sites:\n"));
        assert!(r.contains("    a.bv:3:5  node main\n"));
        assert!(r.contains("    b.bv:9:1  defn helper\n"));
        let first = r.find("a.bv").expect("first site");
        let second = r.find("b.bv").expect("second site");
        assert!(first < second, "insertion order preserved");
    }

    #[test]
    fn severity_labels() {
        assert!(Block::new(Severity::Error, "e").render().starts_with("error: "));
        assert!(Block::new(Severity::Note, "n").render().starts_with("note: "));
        assert!(Block::new(Severity::Info, "i").render().starts_with("info: "));
    }

    #[test]
    fn composite_notes_render_as_one_block() {
        let mut s = Sink::new();
        s.note("counter 'i' retains its store");
        s.note("op Drop deferred for 'buf'");
        let r = s.render_flush();
        assert!(r.starts_with("info: analysis notes (2)\n"));
        assert!(r.contains("  - counter 'i' retains its store\n"));
        assert!(r.contains("  - op Drop deferred for 'buf'\n"));
        assert_eq!(r.matches("info: analysis notes").count(), 1, "one block");
    }

    #[test]
    fn empty_sink_renders_nothing() {
        let s = Sink::new();
        assert!(s.is_empty());
        assert_eq!(s.render_flush(), "");
    }

    #[test]
    fn flush_lists_notes_then_warning_blocks() {
        let mut s = Sink::new();
        s.note("one note");
        s.warn(Block::warning("first warning"));
        s.warn(Block::warning("second warning"));
        let r = s.render_flush();
        let note_at = r.find("analysis notes").expect("notes block");
        let w1 = r.find("warning: first warning").expect("w1");
        let w2 = r.find("warning: second warning").expect("w2");
        assert!(note_at < w1 && w1 < w2, "notes first, then insertion order");
    }
}
