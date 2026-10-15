// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

// ── The core-usable dunder grep-gate (Phase-1 end, plan
// 2026-10-15-core-usable-purge.md §Phase 1 / §Gate) ─────────────────────
//
// The `__` prefix was a frgn marker; the Briev-source-visible surface is
// now plain house-style names (`print_int`, `rt_now_ns`, `argv_has`, …).
// This gate GUARANTEES the surface stays clean: zero `defn __` /
// `out defn __` / `let __` under `lib/**/*.bv` and `plugins/parsed/*.bv`,
// with an EMPTY allowlist. A new dunder defn/let anywhere in the shipped
// stdlib or a prelude fails the suite.
//
// The `@__` LLVM-symbol check covers the same tree but exempts
// `lib/compiler/**` — the dogfood compiler legitimately WRITES IR-text
// string literals that contain `@__briev_*` / `@__wait_for_event` (plan:
// "out of scope: dogfood IR-text strings"). Comments are stripped before
// every check, so a `// defn __old_name` historical note is not a hit.
//
// Undo: if the `__` frgn marker is reintroduced as the source-visible
// surface, delete this gate (the dunder + frgn gates are the only thing
// pinning the plain-name, pure-Briev surface forever).
//
// Sibling in this file: the frgn-allowlist gate (`frgn_only_in_interop_lanes`)
// pins `frgn` to the interop lanes (`lib/std/web/`, `lib/tamer/`,
// `lib/glue/`); after the Phase-2 pure-.bv xxhash rewrite, lib/std/ffi/ is
// frgn-free.

use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Strip `//` line comments so a historical note mentioning a dunder name
/// is not counted as a live surface hit. (No block comments exist in the
/// shipped `.bv` surface; `//` is the house comment.)
fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Every `.bv` under `root`, recursively, sorted for determinism.
fn collect_bv(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_bv(&p, out);
        } else if p.extension().is_some_and(|x| x == "bv") {
            out.push(p);
        }
    }
    out.sort();
}

/// The gate's scan set: the shipped stdlib + the prelude plugins.
fn gate_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_bv(&Path::new(PROJECT_ROOT).join("lib"), &mut files);
    collect_bv(&Path::new(PROJECT_ROOT).join("plugins/parsed"), &mut files);
    files
}

/// A `@__` hit is a dunder LLVM-symbol reference in `.bv` source. The
/// dogfood compiler (`lib/compiler/**`) writes such symbols into generated
/// IR text and is exempt; everywhere else a live `@__` is a leaked dunder.
fn is_dogfood_compiler(path: &Path) -> bool {
    path.strip_prefix(Path::new(PROJECT_ROOT).join("lib/compiler"))
        .is_ok()
}

/// One file's `defn __` / `out defn __` / `let __` hits (comments stripped).
fn dunder_hits_in(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let rel = path.strip_prefix(PROJECT_ROOT).unwrap_or(path);
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let code = strip_comment(line);
            let what = if code.contains("defn __") {
                Some("defn __")
            } else if code.contains("let __") {
                Some("let __")
            } else {
                None
            };
            what.map(|w| format!("{}:{}: {}  [{}]", rel.display(), i + 1, line.trim(), w))
        })
        .collect()
}

/// One file's live `@__` LLVM-dunder hits (comments stripped; dogfood
/// compiler exempt — its IR-text string literals legitimately embed them).
fn llvm_dunder_hits_in(path: &Path) -> Vec<String> {
    if is_dogfood_compiler(path) {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let rel = path.strip_prefix(PROJECT_ROOT).unwrap_or(path);
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let code = strip_comment(line);
            if code.contains("@__") {
                Some(format!("{}:{}: {}", rel.display(), i + 1, line.trim()))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn no_dunder_defn_let_in_shipped_bv() {
    let hits: Vec<String> = gate_files().iter().flat_map(|f| dunder_hits_in(f)).collect();
    assert!(
        hits.is_empty(),
        "core-usable dunder gate: the shipped .bv surface must contain no \
         `defn __` / `out defn __` / `let __` (allowlist: empty). Use the \
         plain house-style name instead (plan 2026-10-15-core-usable-purge). \
         Hits:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_dunder_llvm_symbol_in_shipped_bv() {
    let hits: Vec<String> = gate_files().iter().flat_map(|f| llvm_dunder_hits_in(f)).collect();
    assert!(
        hits.is_empty(),
        "core-usable dunder gate: the shipped .bv surface must not reference \
         a `@__` LLVM dunder symbol (lib/compiler dogfood is exempt; \
         comments are stripped). Hits:\n{}",
        hits.join("\n")
    );
}

// ── The frgn allowlist gate (Phase 2, same plan) ─────────────────────────
//
// `frgn` survives ONLY where interop IS the feature (operator decision):
// lib/std/web/ (JS), lib/tamer/ (host), lib/glue/ (product). Core logic
// must not lean on another language. After the Phase-2 pure-.bv xxhash
// rewrite, lib/std/ffi/ is frgn-free. This gate pins that: any live
// `frgn` / `export frgn` declaration in lib/**/*.bv OUTSIDE the allowlist
// fails the suite. Undo: delete this test if the interop-lane policy
// changes.

/// The only directories in `lib/` allowed to carry a live `frgn`.
const FRGN_ALLOWLIST: &[&str] = &["lib/std/web/", "lib/tamer/", "lib/glue/"];

/// One file's live `frgn` declaration hits (comments stripped). Returns
/// `path:line: text` for every declaration outside the allowlist.
fn frgn_hits_in(path: &Path) -> Vec<String> {
    let rel = path.strip_prefix(PROJECT_ROOT).unwrap_or(path);
    let rel_str = rel.display().to_string();
    if FRGN_ALLOWLIST.iter().any(|a| rel_str.starts_with(a)) {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let code = strip_comment(line).trim_start();
            let is_frgn = code.starts_with("frgn ") || code.starts_with("export frgn ");
            is_frgn.then(|| format!("{}:{}: {}", rel_str, i + 1, line.trim()))
        })
        .collect()
}

#[test]
fn frgn_only_in_interop_lanes() {
    let hits: Vec<String> = gate_files().iter().flat_map(|f| frgn_hits_in(f)).collect();
    assert!(
        hits.is_empty(),
        "frgn allowlist gate: `frgn` is permitted only in the interop lanes \
         (lib/std/web/, lib/tamer/, lib/glue/). Core .bv must be pure Briev \
         (plan 2026-10-15-core-usable-purge §Phase 2). Move the binding into \
         an allowlisted lane or rewrite it in Briev. Hits:\n{}",
        hits.join("\n")
    );
}
