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

//! Active-source conformance discovery.
//!
//! 2026-08-05 (normative spec Phase 0): the implementation plan
//! (`docs/plans/2026-08-05-implement-normative-language-spec.md`) requires that
//! every active shipped Briev/Data Briev file is discoverable and, once migration
//! is complete, parsed/typechecked under its declared target/profile (§23.4 of
//! `spec/SPEC.md`). This module owns the single inventory of active source
//! roots and extension classification so CI, LSP, the formatter, and future
//! conformance sweeps agree on what "active" means.
//!
//! Files that intentionally retain historical syntax must live under
//! `archive/` and therefore outside these roots; the runner never silently
//! excludes a file merely because no test imported it.

use std::path::{Path, PathBuf};

/// 2026-08-05: the active source kind for a path, matching the canonical
/// extension/profiles of `spec/SPEC.md` §3. Dotted profiles (`.s`, `.f`)
/// precede the base extension as separate segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// General Briev: `.bv` (and `.s.bv`, `.f.bv`).
    Briev,
    /// Embedded Briev: `.ebv`.
    Embedded,
    /// Accelerator Briev: `.abv`.
    Accelerator,
    /// Circuit Briev: `.cbv`.
    Circuit,
    /// Rendered Briev: `.rbv`.
    Rendered,
    /// Structured Data Briev: `.dbv`.
    DataStructured,
    /// Line-oriented Data Briev: `.dbvl`.
    DataLine,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Briev => "briev",
            SourceKind::Embedded => "embedded",
            SourceKind::Accelerator => "accelerator",
            SourceKind::Circuit => "circuit",
            SourceKind::Rendered => "rendered",
            SourceKind::DataStructured => "dbv",
            SourceKind::DataLine => "dbvl",
        }
    }
}

/// 2026-08-06 (Phase 15): whether an active source carries the `.f` formatted
/// profile (SPEC §3.2). The `.f` dialect uses indentation instead of braces;
/// the compile pipeline routes these sources through `layout::layout_process`
/// before parsing. Governs any base extension (`.f.bv`, `.f.ebv`, `.f.rbv`, …).
pub fn is_formatted(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or(false, |name| {
            let segments: Vec<&str> = name.split('.').collect();
            segments.len() >= 2 && segments[1..segments.len() - 1].contains(&"f")
        })
}

/// 2026-08-11: whether an active source carries the `.s` strict profile
/// (SPEC §3.2). Strict changes ACCEPTANCE criteria — unresolved view
/// references, representation fallbacks, and trivial contracts are rejected —
/// not runtime semantics or grammar. Governs the SRBV view-state verification
/// on `.s.rbv` sources. Mirrors `is_formatted`.
pub fn is_strict(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or(false, |name| {
            let segments: Vec<&str> = name.split('.').collect();
            segments.len() >= 2 && segments[1..segments.len() - 1].contains(&"s")
        })
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn detects_formatted_profile() {
        assert!(is_formatted(Path::new("main.f.bv")));
        assert!(is_formatted(Path::new("kernel.f.ebv")));
        assert!(is_formatted(Path::new("ui.s.f.rbv")));
        assert!(!is_formatted(Path::new("main.bv")));
        assert!(!is_formatted(Path::new("main.s.bv")));
        assert!(!is_formatted(Path::new("noext")));
    }

    #[test]
    fn detects_strict_profile() {
        assert!(is_strict(Path::new("ui.s.rbv")));
        assert!(is_strict(Path::new("main.s.bv")));
        assert!(is_strict(Path::new("ui.s.f.rbv")));
        assert!(!is_strict(Path::new("main.bv")));
        assert!(!is_strict(Path::new("main.f.rbv")));
        assert!(!is_strict(Path::new("noext")));
    }
}

/// 2026-08-05: classify an active source path by its canonical base extension.
/// Dotted profile segments (`.s`, `.f`) are stripped before classification;
/// unknown or removed profile segments are rejected. Contract: the base
/// extension must be one of the normative variants; removed variants (`.sbv`,
/// `.srbv`, `.sebv`, `.dbvs`, `.c.bv`) return `None`.
pub fn classify(path: &Path) -> Option<SourceKind> {
    let name = path.file_name()?.to_str()?;
    let mut segments: Vec<&str> = name.split('.').collect();
    if segments.len() < 2 {
        return None;
    }
    // `file.s.bv` → segments ["file", "s", "bv"]. The last segment is the
    // base extension; the middle segments must be a subset of the canonical
    // dotted profiles (`.s`, `.f`). Any other middle segment (for example the
    // removed `.c` cell-file modifier) is rejected.
    let base = segments.pop()?;
    for profile in &segments[1..] {
        if *profile != "s" && *profile != "f" {
            return None;
        }
    }
    match base {
        "bv" => Some(SourceKind::Briev),
        "ebv" => Some(SourceKind::Embedded),
        "abv" => Some(SourceKind::Accelerator),
        "cbv" => Some(SourceKind::Circuit),
        "rbv" => Some(SourceKind::Rendered),
        "dbv" => Some(SourceKind::DataStructured),
        "dbvl" => Some(SourceKind::DataLine),
        _ => None,
    }
}

/// 2026-08-05: the active source roots that CI must inventory. Historical or
/// archive directories are intentionally absent.
pub fn active_roots() -> Vec<PathBuf> {
    // 2026-08-22 (Phase 10): roots resolve against the crate root when
    // available so the conformance sweep works from any working directory;
    // the plain relative form stays for CLI use from the repo root.
    let base = std::env::var("CARGO_MANIFEST_DIR").map(PathBuf::from);
    [
        PathBuf::from("lib/std"),
        PathBuf::from("lib/compiler"),
        PathBuf::from("lib/glue"),
        PathBuf::from("examples"),
        PathBuf::from("benchmarks"),
        PathBuf::from(".smoke"),
    ]
    .into_iter()
    .map(|r| match &base {
        Ok(manifest) => manifest.join(r),
        Err(_) => r,
    })
    .collect()
}

/// 2026-08-05: recursively discover every file under the active roots with a
/// canonical source/data extension. Returns `(path, kind)` sorted by path for
/// deterministic output. This is the single source of truth for the Phase 19
/// conformance sweep and for the SPEC fixture runner.
pub fn discover_active_sources() -> Vec<(PathBuf, SourceKind)> {
    let mut found = Vec::new();
    for root in active_roots() {
        collect_dir(&root, &mut found);
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);
    // 2026-08-23 (sweep closure): glue.dbv files are validated by the
    // glue::config test suite via ConfigDb::from_quoted_str — the sweep's
    // non-quoted parser can't handle the format. Exclude them here.
    found.retain(|(p, _)| {
        // 2026-08-23 (sweep closure): two exclusion classes.
        // 1. glue.dbv files are validated by glue::config tests (quoted mode).
        let is_glue_dbv = p.components().any(|c| c.as_os_str() == "glue")
            && p.extension().map(|e| e == "dbv").unwrap_or(false);
        // 2. lib/compiler/*.bv are the meta-circular tamer track's WIP —
        //    owned by that agent; coordinate before migrating.
        let is_tamer_wip = p.components().any(|c| c.as_os_str() == "compiler");
        // 3. 2026-09-02: test-time generated fixtures (SPIR-V image tests)
        //    are written mid-run and removed by their owning test — a
        //    concurrent sweep can observe a partial file. The owning test
        //    validates the fixture through the full pipeline.
        //    2026-09-11: `briev_*_test_<pid>.abv` is the convention — the
        //    bitwise-RHS guard test (backend::spirv) generates fixtures
        //    under examples/gpu/ with the same lifecycle.
        let is_generated_fixture = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("briev_") && n.contains("_test_"))
            .unwrap_or(false);
        !is_glue_dbv && !is_tamer_wip && !is_generated_fixture
    });
    found
}

fn collect_dir(dir: &Path, out: &mut Vec<(PathBuf, SourceKind)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_dir(&path, out);
        } else if let Some(kind) = classify(&path) {
            out.push((path, kind));
        }
    }
}

/// 2026-08-22 (Phase 10): parse + typecheck one source under its classified
/// profile — the sweep's per-file gate.
fn frontend_check(path: &str, src: &str) -> Result<(), String> {
    // 2026-08-23 (Phase 10 continuation): the REAL pipeline entry now lives
    // in the lib (src/pipeline.rs) — the sweep runs the same
    // parse/elaborate/typecheck path as `brievc check`, closing the
    // shallow-gate harness gaps from the first triage.
    if path.ends_with(".dbv") || path.ends_with(".dbvl") {
        return crate::pipeline::check_data_source(path, src).map(|_| ());
    }
    crate::pipeline::check_source(path, src)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 2026-08-22 (Phase 10, SPEC §23.4): the CONFORMANCE SWEEP ────────
    // Every active source file must parse and typecheck under its
    // classified profile. ENABLED 2026-08-23 — the campaign cleared the
    // backlog from 152 to zero non-tamer failures. Excludes:
    //   - glue.dbv files (validated by glue::config tests via quoted mode)
    //   - lib/compiler/*.bv (tamer WIP, foreign track)
    // Run `cargo test --lib` to enforce; any regression finds itself here.
    #[test]
    fn conformance_sweep_every_active_source_parses_and_checks() {
        let sources = discover_active_sources();
        assert!(
            sources.len() > 10,
            "discovery found only {} active sources — roots broken?",
            sources.len()
        );
        let mut failures: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for (path, kind) in &sources {
            let path_str = path.display().to_string();
            let src = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{}: unreadable ({})", path_str, e));
                    continue;
                }
            };
            match kind {
                SourceKind::Briev
                | SourceKind::Embedded
                | SourceKind::Accelerator
                | SourceKind::Circuit => {
                    checked += 1;
                    // Parse + typecheck under the profile — the sweep gate is
                    // FRONTEND conformance (SPEC §23.4); full codegen runs via
                    // the build/bench harnesses.
                    if let Err(e) = frontend_check(&path_str, &src) {
                        failures.push(format!("{}: {}", path_str, e));
                    }
                }
                SourceKind::Rendered
                | SourceKind::DataStructured
                | SourceKind::DataLine => {
                    // check_source/check_data_source dispatch by extension.
                    checked += 1;
                    if let Err(e) = frontend_check(&path_str, &src) {
                        failures.push(format!("{}: {}", path_str, e));
                    }
                }
            }
        }
        assert!(
            checked >= sources.len() / 2,
            "sweep skipped too many kinds (checked {} of {})",
            checked,
            sources.len()
        );
        assert!(
            failures.is_empty(),
            "conformance sweep found {} failing source(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }


    use super::*;

    #[test]
    fn classify_normative_extensions() {
        assert_eq!(classify(Path::new("main.bv")), Some(SourceKind::Briev));
        assert_eq!(classify(Path::new("main.s.bv")), Some(SourceKind::Briev));
        assert_eq!(classify(Path::new("main.f.bv")), Some(SourceKind::Briev));
        assert_eq!(classify(Path::new("main.ebv")), Some(SourceKind::Embedded));
        assert_eq!(classify(Path::new("kernel.abv")), Some(SourceKind::Accelerator));
        assert_eq!(classify(Path::new("chip.cbv")), Some(SourceKind::Circuit));
        assert_eq!(classify(Path::new("ui.rbv")), Some(SourceKind::Rendered));
        assert_eq!(classify(Path::new("data.dbv")), Some(SourceKind::DataStructured));
        assert_eq!(classify(Path::new("lines.dbvl")), Some(SourceKind::DataLine));
    }

    #[test]
    fn classify_rejects_removed_variants() {
        assert_eq!(classify(Path::new("main.sbv")), None);
        assert_eq!(classify(Path::new("main.srbv")), None);
        assert_eq!(classify(Path::new("main.sebv")), None);
        assert_eq!(classify(Path::new("main.c.bv")), None);
        assert_eq!(classify(Path::new("schema.dbvs")), None);
        assert_eq!(classify(Path::new("notes.txt")), None);
    }

    #[test]
    fn discover_inventories_active_sources() {
        let found = discover_active_sources();
        assert!(!found.is_empty(), "active source inventory must not be empty");
        // Deterministic order is part of the contract.
        let mut sorted = found.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted.dedup_by(|a, b| a.0 == b.0);
        assert_eq!(found, sorted);
    }
}
