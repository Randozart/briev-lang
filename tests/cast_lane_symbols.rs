//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Lane-definition drift guard.
//!
//! The casting graph emits `call @<symbol>` for every `ExtCall` lane and the
//! backend SKIPS the C declare when a Briev `defn` of that name exists in the
//! compilation unit. Two stdlib files carry those defns as twins:
//!
//!   * `lib/std/cast_lanes.bv`    — the native process (may call Linux syscalls)
//!   * `lib/std/cast_portable.bv` — the browser/wasm unit (no syscalls)
//!
//! A lane added to only one twin compiles natively and then fails the browser
//! build in the verifier with `use of undefined value '@<symbol>'` — the exact
//! failure that issues #153/#154/#155 produced for `str_to_float64`. This test
//! makes that drift mechanical instead of discovered late.
//!
//! The rule is one-directional on purpose: the portable unit is a documented
//! SUBSET (native-only process/output lanes stay in `cast_lanes.bv` and are
//! reached through intrinsics, not graph lanes). So any graph lane symbol that
//! `cast_lanes.bv` defines must also be defined by `cast_portable.bv`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Every `defn <name>(` (optionally `out defn <name>(`) declared in a file.
fn defined_defns(path: &Path) -> BTreeSet<String> {
    let source =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut names = BTreeSet::new();
    for line in source.lines() {
        let line = line.trim_start();
        let rest = line
            .strip_prefix("out defn ")
            .or_else(|| line.strip_prefix("defn "));
        let Some(rest) = rest else { continue };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        // A definition, not a forward declaration: `defn f(...)` carries its
        // parameter list on the same line or (for multi-line signatures) the
        // name is still followed by `(` on the next non-empty line. Both the
        // native and portable files always put `(` on the signature line.
        if line[line.find(&name).unwrap_or(0)..].contains('(') {
            names.insert(name);
        }
    }
    names
}

#[test]
fn every_graph_lane_symbol_defined_in_one_twin_is_defined_in_both() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let native = defined_defns(&root.join("lib/std/cast_lanes.bv"));
    let portable = defined_defns(&root.join("lib/std/cast_portable.bv"));
    let graph = briev_compiler::casting::graph::CastingGraph::new();
    let lanes: BTreeSet<String> = graph.ext_call_lane_symbols().into_iter().collect();

    // Symbols the graph can emit that the native unit implements in Briev.
    let mut missing: Vec<&str> = Vec::new();
    for lane in &lanes {
        if native.contains(lane) && !portable.contains(lane) {
            missing.push(lane);
        }
    }
    assert!(
        missing.is_empty(),
        "cast lane symbol(s) defined in lib/std/cast_lanes.bv but missing from \
         lib/std/cast_portable.bv: {missing:?} — the browser/wasm unit would emit \
         `call @<symbol>` with no definition. Add the same `out defn` to the \
         portable twin (keep both files' binary32/binary64 doors symmetric)."
    );
}

#[test]
fn the_guard_actually_sees_the_binary64_doors() {
    // A guard that silently matches nothing is worse than no guard: pin that
    // the enumeration reaches the two doors issues #153/#154/#155 added.
    let graph = briev_compiler::casting::graph::CastingGraph::new();
    let lanes: BTreeSet<String> = graph.ext_call_lane_symbols().into_iter().collect();
    for expected in [
        "str_to_float64",
        "float64_to_str",
        "str_to_float",
        "float_to_str",
    ] {
        assert!(
            lanes.contains(expected),
            "cast lane enumeration missed `{expected}` — the drift guard would \
             not protect it"
        );
    }
}
