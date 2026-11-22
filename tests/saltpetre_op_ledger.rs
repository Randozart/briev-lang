//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Op-backend ledger gate for the saltpetre op surface.
//!
//! Plan: `.opencode/plans/2026-11-22-saltpetre-op-backend-ledger.md`.
//! Research: `.opencode/plans/2026-11-22-saltpetre-op-surface-research.md`.
//!
//! Saltpetre is the compat layer: an op may be served by Briev code today and
//! by a vendor lib tomorrow, and the destination for each is Briev-emitted
//! codegen. `lib/saltpetre/op-backends.dbvl` records, per op, which backend
//! serves it and — when that backend is delegated — the measurement that
//! retires the delegation.
//!
//! This gate is what keeps that record honest. It is textual on purpose (the
//! `idiom_ledger_gate.rs` pattern): the ledger and the dispatcher source ARE
//! the contract, so no parser dependency is needed to compare them.
//!
//! Doctrine note: the compiler is never allowed to delegate
//! (`abv-gpu-doctrine.md` §1.3 — "FFI lanes exist for users; they are not the
//! codegen path"). A *library* may, which is why saltpetre's FFI arms are
//! legitimate — and why invariants 3 and 4 exist: an arm must name its
//! retirement, and a `briev` row must not secretly call FFI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

const LEDGER: &str = "lib/saltpetre/op-backends.dbvl";
const DISPATCHER: &str = "lib/saltpetre/exec.bv";
const CENSUS: &str = "lib/saltpetre/op-census.txt";

/// Helpers this work moved to the stdlib floor. Each must have exactly one
/// `defn` across `lib/std/**` + `lib/saltpetre/**` (invariant 7).
const EXTRACTED_HELPERS: &[&str] = &[
    "dims_product",
    "strides_of",
    "round_even",
    "pow_float",
    "f32_from_bytes",
    "int_from_bytes_le",
];

/// Backends an op may declare.
const BACKEND_BRIEV: &str = "briev";
/// A `briev` row's graduation is the literal `none` (nothing to retire).
const GRADUATION_NONE: &str = "none";
/// A `briev` row's graduation may instead be a `promote:` claim (see
/// `lib/saltpetre/op-backends.dbvl` "GRADUATION FORMS").
const GRADUATION_PROMOTE: &str = "promote: ";

/// One ledger row: which backend serves an op, and how a delegated backend
/// retires (or, for a promoted `briev` row, the evidence that it holds its own).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    op: String,
    backend: String,
    imp: String,
    graduation: String,
    evidence: String,
}

impl Row {
    /// True when this row delegates to a vendor lib (op-level or model-level).
    fn is_delegated(&self) -> bool {
        self.backend.starts_with("ffi(") || self.backend.starts_with("model(")
    }

    /// True when this row claims the op has been promoted to first-class Briev
    /// code and holds its own on a measured shape.
    fn is_promoted(&self) -> bool {
        self.graduation.starts_with(GRADUATION_PROMOTE)
    }
}

/// A `promote:` claim, split out of a `briev` row's graduation text.
///
/// Shape: `promote: <geometry> <dtype> @ <device> vs <vendor> <dtype> — ratio
/// <= <N>, BAD=0, CANARY_BAD=0, compiled-on-vulkan;` with an `evidence:` field
/// naming the results doc. Every piece is required because a promotion is a
/// measured claim: without the shape it means nothing, without the competitor
/// it means nothing, and without the evidence it cannot be re-read.
#[derive(Debug)]
struct Promotion {
    geometry: String,
    device: String,
    bound: f64,
}

fn root() -> PathBuf {
    PathBuf::from(PROJECT_ROOT)
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Strip a trailing `;` and surrounding whitespace from one ledger field.
fn field_value(field: &str) -> &str {
    field.trim().trim_end_matches(';').trim()
}

/// Parse one `a: b; c: d;` ledger line into a [`Row`].
///
/// Returns `Err` on a missing/extra field, an unknown backend form, or a
/// delegated row without a retirement condition — the three ways a row can lie
/// (invariants 3 and 5).
fn parse_row(line: &str) -> Result<Row, String> {
    let mut op = None;
    let mut backend = None;
    let mut imp = None;
    let mut graduation = None;
    let mut evidence = None;
    for field in line.split(';') {
        let Some((key, value)) = field.split_once(':') else {
            continue;
        };
        let value = field_value(value).to_string();
        match key.trim() {
            "op" => op = Some(value),
            "backend" => backend = Some(value),
            "impl" => imp = Some(value),
            "graduation" => graduation = Some(value),
            "evidence" => evidence = Some(value),
            other if !other.is_empty() => {
                return Err(format!("unknown ledger field '{other}' in: {line}"));
            }
            _ => {}
        }
    }
    let op = op.ok_or_else(|| format!("ledger row has no `op:` field: {line}"))?;
    let backend = backend.ok_or_else(|| format!("op '{op}' has no `backend:` field: {line}"))?;
    let imp = imp.ok_or_else(|| format!("op '{op}' has no `impl:` field: {line}"))?;
    let graduation = graduation.unwrap_or_default();
    let row = Row {
        op,
        backend,
        imp,
        graduation,
        evidence: evidence.unwrap_or_default(),
    };
    validate_backend(&row)?;
    validate_promotion(&row)?;
    Ok(row)
}

/// Parse the acceptance half of a `promote:` claim: the `ratio <= <N>` bound.
fn promotion_bound(acceptance: &str) -> Result<f64, String> {
    const TAG: &str = "ratio <= ";
    let idx = acceptance
        .find(TAG)
        .ok_or_else(|| "promotion has no `ratio <= <N>` acceptance bound".to_string())?;
    let rest = &acceptance[idx + TAG.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end]
        .parse::<f64>()
        .map_err(|e| format!("promotion bound '{rest}' is not a number: {e}"))
}

/// Split a `promote:` graduation into its measured shape, device and bound.
///
/// Errors (rather than defaults) on every missing part: a promotion that does
/// not say where it was measured is not a claim this project can check.
fn parse_promotion(graduation: &str) -> Result<Promotion, String> {
    let body = graduation
        .strip_prefix(GRADUATION_PROMOTE)
        .ok_or_else(|| "not a promotion".to_string())?;
    let (head, acceptance) = body
        .split_once(" \u{2014} ")
        .ok_or_else(|| format!("promotion '{body}' names no acceptance after an em dash"))?;
    let (geometry, rest) = head
        .split_once(" @ ")
        .ok_or_else(|| format!("promotion '{head}' names no `@ <device>`"))?;
    let (device, _vendor) = rest
        .split_once(" vs ")
        .ok_or_else(|| format!("promotion '{rest}' names no `vs <vendor>` competitor"))?;
    // Compare acceptance tokens exactly. A substring test is not enough:
    // `CANARY_BAD=0` contains `BAD=0`, so a claim that dropped the primary
    // correctness token would still pass. The canary test pins this.
    let tokens: Vec<&str> = acceptance.split(',').map(str::trim).collect();
    for gate in ["BAD=0", "CANARY_BAD=0", "compiled-on-vulkan"] {
        if !tokens.contains(&gate) {
            return Err(format!(
                "promotion acceptance '{acceptance}' omits the `{gate}` token — a promoted 
                 number must carry the same-geometry correctness gate that produced it"
            ));
        }
    }
    Ok(Promotion {
        geometry: geometry.trim().to_string(),
        device: device.trim().to_string(),
        bound: promotion_bound(acceptance)?,
    })
}

/// Only a `briev` row can be promoted, and it must name its evidence doc.
fn validate_promotion(row: &Row) -> Result<(), String> {
    if !row.is_promoted() {
        return Ok(());
    }
    if row.backend != BACKEND_BRIEV {
        return Err(format!(
            "op '{}' claims `promote:` while its backend is '{}' — promotion means the op IS \
             Briev code; a delegated row is retired by its own `graduation`, not promoted",
            row.op, row.backend
        ));
    }
    parse_promotion(&row.graduation).map_err(|e| format!("op '{}': {e}", row.op))?;
    let path = root().join(&row.evidence);
    if row.evidence.is_empty() || !path.is_file() {
        return Err(format!(
            "op '{}' claims `promote:` but its `evidence:` field names no readable results doc \
             ('{}') — a promotion without committed evidence is a memory",
            row.op, row.evidence
        ));
    }
    Ok(())
}

/// Reject a backend form we do not recognise, and enforce the graduation rule
/// for delegated rows (invariant 3).
fn validate_backend(row: &Row) -> Result<(), String> {
    let b = row.backend.as_str();
    if b == BACKEND_BRIEV {
        return Ok(());
    }
    let delegated = b.starts_with("ffi(") || b.starts_with("model(");
    if !delegated || !b.ends_with(')') || b.len() < 6 {
        return Err(format!(
            "op '{}' declares backend '{b}': expected `briev`, `ffi(<lib>)` or `model(<lib>)`",
            row.op
        ));
    }
    if row.graduation.is_empty() || row.graduation == GRADUATION_NONE {
        return Err(format!(
            "op '{}' delegates to '{b}' with no retirement condition — a delegated row MUST \
             name its `graduation` (the A/B that retires it). An escape hatch without a \
             retirement is a permanent dependency.",
            row.op
        ));
    }
    Ok(())
}

/// Parse the whole ledger text; duplicate ops are an error.
fn parse_ledger(text: &str) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") || !t.starts_with("op:") {
            continue;
        }
        let row = parse_row(t).map_err(|e| format!("line {}: {e}", n + 1))?;
        if let Some(prev) = rows.iter().find(|r| r.op == row.op) {
            return Err(format!(
                "duplicate ledger rows for op '{}' ({:?} and {:?})",
                prev.op, prev.backend, row.backend
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// The ops the dispatcher actually has: every `when node.op_type == "X"` arm.
///
/// The terminal `unsupported ONNX op` arm is not a dispatch and is not
/// matched, because it never compares against a literal op name.
fn dispatched_ops(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(idx) = line.find("node.op_type == \"") else {
            continue;
        };
        let rest = &line[idx + "node.op_type == \"".len()..];
        let Some(end) = rest.find('"') else { continue };
        out.push(rest[..end].to_string());
    }
    out
}

/// Compare ledger rows against the dispatcher in BOTH directions
/// (invariants 1 and 2): no unrecorded op, no invented op.
///
/// Both directions go through hash sets so the check is linear in the op count
/// — a nested scan over rows × ops is O(n²) and shows up as a complexity
/// diagnostic (Rule 5 / flatten nested loops).
fn validate_coverage(rows: &[Row], ops: &[String]) -> Result<(), String> {
    let recorded: HashSet<&str> = rows.iter().map(|r| r.op.as_str()).collect();
    let dispatched: HashSet<&str> = ops.iter().map(String::as_str).collect();
    let mut missing: Vec<&str> = dispatched.difference(&recorded).copied().collect();
    let mut orphan: Vec<&str> = recorded.difference(&dispatched).copied().collect();
    missing.sort_unstable();
    orphan.sort_unstable();
    if !missing.is_empty() {
        return Err(format!(
            "dispatched op(s) with no ledger row: {missing:?} — add a row to {LEDGER}"
        ));
    }
    if !orphan.is_empty() {
        return Err(format!(
            "ledger row(s) for ops the dispatcher does not have: {orphan:?} — the row would \
             claim coverage that does not exist"
        ));
    }
    Ok(())
}

/// Every `impl:` must name a real `defn` somewhere under `lib/saltpetre`
/// (invariant 5).
fn validate_impls(rows: &[Row], saltpetre_src: &str) -> Result<(), String> {
    for row in rows {
        let needle = format!("defn {}(", row.imp);
        if !saltpetre_src.contains(&needle) {
            return Err(format!(
                "op '{}' names impl '{}', but no `{needle}` exists under lib/saltpetre",
                row.op, row.imp
            ));
        }
    }
    Ok(())
}

/// A `briev` row must not secretly delegate: the dispatcher may not reference
/// FFI at all (invariant 4).
fn validate_briev_is_ffi_free(rows: &[Row], dispatcher_src: &str) -> Result<(), String> {
    let any_briev = rows.iter().any(|r| r.backend == BACKEND_BRIEV);
    if !any_briev {
        return Ok(());
    }
    for marker in ["frgn ", "#Link<"] {
        if dispatcher_src.contains(marker) {
            return Err(format!(
                "a `briev` ledger row exists, but {DISPATCHER} references FFI (`{marker}`) — \
                 the ledger would claim Briev-native for an op that delegates"
            ));
        }
    }
    Ok(())
}

/// Read one directory, pushing subdirectories onto `stack` and `.bv` files
/// onto `out`. Split out of [`bv_files`] so the traversal loop stays one level
/// deep (Rule: flat control flow, max 2 nesting levels).
fn visit_dir(dir: &Path, stack: &mut Vec<PathBuf>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            stack.push(path);
            continue;
        }
        if path.extension().is_some_and(|e| e == "bv") {
            out.push(path);
        }
    }
}

/// Collect every `lib/**/*.bv` file under the given roots.
fn bv_files(roots: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.iter().map(|r| root().join(r)).collect();
    while let Some(dir) = stack.pop() {
        visit_dir(&dir, &mut stack, &mut out);
    }
    out.sort();
    out
}

/// Count `defn <name>(` occurrences across the given roots (invariant 7).
fn defn_occurrences(name: &str, roots: &[&str]) -> usize {
    let needle = format!("defn {name}(");
    bv_files(roots)
        .iter()
        .map(|p| {
            std::fs::read_to_string(p)
                .unwrap_or_default()
                .matches(&needle)
                .count()
        })
        .sum()
}

/// Parse `lib/saltpetre/op-census.txt` into `(briev, delegated, total)`.
fn parse_census(text: &str) -> Result<(usize, usize, usize), String> {
    let grab = |key: &str| -> Result<usize, String> {
        text.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .find_map(|l| l.trim().strip_prefix(key))
            .ok_or_else(|| format!("{CENSUS} has no `{key}` row"))?
            .trim()
            .parse::<usize>()
            .map_err(|e| format!("{CENSUS}: `{key}` is not a number: {e}"))
    };
    Ok((grab("briev=")?, grab("delegated=")?, grab("total=")?))
}

/// The census may only improve: `briev` up, `delegated` down, `total` fixed
/// (invariant 8).
fn validate_census(rows: &[Row], recorded: (usize, usize, usize)) -> Result<(), String> {
    let briev = rows.iter().filter(|r| r.backend == BACKEND_BRIEV).count();
    let delegated = rows.iter().filter(|r| r.is_delegated()).count();
    let total = rows.len();
    let (rec_briev, rec_delegated, rec_total) = recorded;
    if briev < rec_briev {
        return Err(format!(
            "op census regressed: briev {briev} < recorded {rec_briev}. Retirement only moves \
             forward; an op that was Briev-native may not become delegated."
        ));
    }
    if delegated > rec_delegated {
        return Err(format!(
            "op census regressed: delegated {delegated} > recorded {rec_delegated}"
        ));
    }
    if total != rec_total {
        return Err(format!(
            "op census total changed: {total} != recorded {rec_total} — the dispatched op set \
             moved; update {CENSUS} deliberately and say why"
        ));
    }
    Ok(())
}

/// Read the `key=value` gate record out of a results doc.
///
/// The record is the fenced block that starts with `saltpetre_op=`, so the doc
/// itself decides which lines the gate reads: editing prose cannot move a number
/// out from under the check, and a claim cannot be restated in words the gate
/// does not parse.
fn gate_record(doc: &str) -> Result<Vec<(String, String)>, String> {
    let lines: Vec<&str> = doc.lines().map(str::trim).collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("saltpetre_op="))
        .ok_or_else(|| "results doc has no `saltpetre_op=` gate record block".to_string())?;
    let mut out: Vec<(String, String)> = Vec::new();
    for line in &lines[start..] {
        if *line == "```" {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
}

/// Look up one textual field of a gate record.
fn gate_token<'a>(block: &'a [(String, String)], key: &str) -> Result<&'a str, String> {
    block
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| format!("gate record has no `{key}` figure"))
}

/// Look up one numeric field of a gate record.
fn gate_number(block: &[(String, String)], key: &str) -> Result<f64, String> {
    let raw = gate_token(block, key)?;
    raw.parse::<f64>()
        .map_err(|e| format!("gate record `{key}` = '{raw}' is not a number: {e}"))
}

/// Cross-check a promotion against its evidence doc: same op, same shape, same
/// device, gates observed, and a measured ratio that actually satisfies the
/// claimed bound. The ratio is recomputed from the two p50s rather than trusted
/// from the doc's own `ratio_vs_openvino`, so the doc cannot round its own way
/// into a tighter claim.
fn cross_check_evidence(row: &Row, promo: &Promotion, doc: &str) -> Result<(), String> {
    let block = gate_record(doc)?;
    let same_shape = format!("{}", promo.geometry.replace(' ', "-"));
    for (key, want) in [
        ("saltpetre_op", row.op.as_str()),
        ("geometry", same_shape.as_str()),
        ("device", promo.device.as_str()),
        ("BAD", "0"),
        ("CANARY_BAD", "0"),
        ("COMPILED_ON_VULKAN", "1"),
    ] {
        let got = gate_token(&block, key)?;
        if got != want {
            return Err(format!(
                "{}: row '{}' claims {key}={want} but its evidence says {key}={got}",
                row.evidence, row.op
            ));
        }
    }
    let within = gate_number(&block, "within")?;
    if (within - promo.bound).abs() > f64::EPSILON {
        return Err(format!(
            "op '{}': row claims `ratio <= {:.1}` but the evidence was accepted at {within} — \
             a bound tightened in the ledger without re-measuring is a false claim",
            row.op, promo.bound
        ));
    }
    let briev_us = gate_number(&block, "briev_gpu_p50_us")?;
    let vendor_us = gate_number(&block, "openvino_gpu_p50_us")?;
    if vendor_us <= 0.0 {
        return Err(format!("op '{}': vendor p50 must be positive", row.op));
    }
    let measured = briev_us / vendor_us;
    let recorded = gate_number(&block, "ratio_vs_openvino")?;
    if (measured - recorded).abs() > 0.02 {
        return Err(format!(
            "{}: `ratio_vs_openvino={recorded}` does not follow from {briev_us}/{vendor_us} \
             = {measured:.2}",
            row.evidence
        ));
    }
    if measured > promo.bound {
        return Err(format!(
            "op '{}': measured ratio {measured:.2}x at {} exceeds the claimed bound {:.1}x",
            row.op, promo.geometry, promo.bound
        ));
    }
    Ok(())
}

/// Whole-surface check: ledger parses, covers the dispatcher, names real impls,
/// and keeps `briev` rows free of FFI.
fn validate_surface(
    ledger: &str,
    dispatcher: &str,
    saltpetre_src: &str,
) -> Result<Vec<Row>, String> {
    let rows = parse_ledger(ledger)?;
    let ops = dispatched_ops(dispatcher);
    validate_coverage(&rows, &ops)?;
    validate_impls(&rows, saltpetre_src)?;
    validate_briev_is_ffi_free(&rows, dispatcher)?;
    Ok(rows)
}

#[test]
fn every_dispatched_op_has_a_row() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    let ops = dispatched_ops(&read(DISPATCHER));
    assert!(
        !ops.is_empty(),
        "dispatcher scan found no `node.op_type ==` arms"
    );
    validate_coverage(&rows, &ops).expect("every dispatched op is in the ledger");
}

#[test]
fn no_orphan_ledger_rows() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    let ops = dispatched_ops(&read(DISPATCHER));
    let recorded: HashSet<&str> = rows.iter().map(|r| r.op.as_str()).collect();
    let dispatched: HashSet<&str> = ops.iter().map(String::as_str).collect();
    let mut orphan: Vec<&str> = recorded.difference(&dispatched).copied().collect();
    orphan.sort_unstable();
    assert!(
        orphan.is_empty(),
        "ledger rows with no dispatcher arm: {orphan:?}"
    );
}

#[test]
fn delegated_ops_name_their_graduation() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    for row in rows.iter().filter(|r| r.is_delegated()) {
        assert!(
            !row.graduation.is_empty() && row.graduation != GRADUATION_NONE,
            "op '{}' delegates to '{}' without a retirement condition",
            row.op,
            row.backend
        );
    }
}

/// Every `promote:` row must be a well-formed, backed claim (invariant 9): a
/// shape, a device, a competitor, the correctness tokens, and an evidence doc
/// that exists. Mutations of each of those are rejected in the canary below.
#[test]
fn briev_rows_carry_promotion_specs() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    let promoted: Vec<&Row> = rows.iter().filter(|r| r.is_promoted()).collect();
    assert!(
        !promoted.is_empty(),
        "no `promote:` row in {LEDGER} — the first graduation (Gemm) must be recorded, \
         otherwise promotion status lives only in a plan document"
    );
    for row in promoted {
        let promo =
            parse_promotion(&row.graduation).unwrap_or_else(|e| panic!("op '{}': {e}", row.op));
        assert!(
            promo.bound > 0.0,
            "op '{}': promotion bound must be positive",
            row.op
        );
        assert!(
            !row.evidence.is_empty(),
            "op '{}': promotion names no evidence doc",
            row.op
        );
    }
}

/// The promotion numbers must still be the measured ones: the results doc is
/// re-read and the ratio recomputed from its p50s (invariant 9).
#[test]
fn results_doc_has_gated_numbers() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    for row in rows.iter().filter(|r| r.is_promoted()) {
        let promo =
            parse_promotion(&row.graduation).unwrap_or_else(|e| panic!("op '{}': {e}", row.op));
        let doc = read(&row.evidence);
        cross_check_evidence(row, &promo, &doc)
            .unwrap_or_else(|e| panic!("promotion evidence mismatch: {e}"));
    }
}

/// The promotion gate must be observed failing (canary discipline) — a gate
/// that has never failed is a gate you do not know works.
#[test]
fn gate_rejects_bad_promotions() {
    let good = "promote: 512^3 f32 @ PTL-Xe3-vulkan vs openvino f32 — ratio <= 2.0, \
                BAD=0, CANARY_BAD=0, compiled-on-vulkan";
    let promo = parse_promotion(good).expect("the shipped form parses");
    assert_eq!(promo.geometry, "512^3 f32");
    assert_eq!(promo.device, "PTL-Xe3-vulkan");
    assert!((promo.bound - 2.0).abs() < 1e-9);

    for bad in [
        "promote: 512^3 f32 vs openvino f32 — ratio <= 2.0, BAD=0, CANARY_BAD=0, compiled-on-vulkan",
        "promote: 512^3 f32 @ PTL vs openvino — BAD=0, CANARY_BAD=0, compiled-on-vulkan",
        "promote: 512^3 f32 @ PTL vs openvino — ratio <= 2.0, CANARY_BAD=0, compiled-on-vulkan",
        "promote: 512^3 f32 @ PTL vs openvino — ratio <= 2.0, BAD=0, compiled-on-vulkan",
        "promote: 512^3 f32 @ PTL vs openvino — ratio <= 2.0, BAD=0, CANARY_BAD=0, timeout",
    ] {
        assert!(
            parse_promotion(bad).is_err(),
            "a promotion missing a shape/device/competitor/gate token must be rejected: {bad}"
        );
    }

    // A delegated row cannot be "promoted": promotion means the op IS Briev code.
    let delegated = Row {
        op: "Gemm".into(),
        backend: "ffi(cublas)".into(),
        imp: "gemm".into(),
        graduation: good.into(),
        evidence: String::new(),
    };
    let err = validate_promotion(&delegated).expect_err("a delegated promotion must be rejected");
    assert!(err.contains("Briev code"), "unexpected error: {err}");

    // And a promotion with no committed evidence is a memory, not a claim.
    let unbacked = Row {
        op: "Gemm".into(),
        backend: BACKEND_BRIEV.into(),
        imp: "gemm".into(),
        graduation: good.into(),
        evidence: "benchmarks/results/does-not-exist.md".into(),
    };
    let err = validate_promotion(&unbacked).expect_err("missing evidence must be rejected");
    assert!(err.contains("evidence"), "unexpected error: {err}");

    // A doc whose own ratio exceeds the claimed bound must be refused, and so
    // must a bound tightened in the ledger without re-measuring.
    let doc = read("benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md");
    let row = Row {
        op: "Gemm".into(),
        backend: BACKEND_BRIEV.into(),
        imp: "gemm".into(),
        graduation: good.into(),
        evidence: "benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md".into(),
    };
    cross_check_evidence(&row, &promo, &doc).expect("the shipped evidence matches its claim");

    let tighter = parse_promotion(&good.replace("ratio <= 2.0", "ratio <= 0.5"))
        .expect("the form still parses");
    let err = cross_check_evidence(&row, &tighter, &doc)
        .expect_err("a bound tighter than the measurement must be refused");
    assert!(err.contains("ratio"), "unexpected error: {err}");
}

#[test]
fn briev_rows_are_ffi_free() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    validate_briev_is_ffi_free(&rows, &read(DISPATCHER)).expect("briev rows do not delegate");
}

#[test]
fn ledger_parses_and_impls_exist() {
    let rows = validate_surface(&read(LEDGER), &read(DISPATCHER), &read(DISPATCHER))
        .expect("ledger is well formed and its impls exist");
    let briev = rows.iter().filter(|r| r.backend == BACKEND_BRIEV).count();
    let delegated = rows.iter().filter(|r| r.is_delegated()).count();
    println!(
        "op-backend census: briev={briev} delegated={delegated} total={}",
        rows.len()
    );
}

#[test]
fn generic_helpers_are_single_definition() {
    for name in EXTRACTED_HELPERS {
        let count = defn_occurrences(name, &["lib/std", "lib/saltpetre"]);
        assert_eq!(
            count, 1,
            "helper `{name}` has {count} definitions across lib/std + lib/saltpetre; \
             extracted helpers must exist exactly once (Rule 17: DRY)"
        );
    }
}

#[test]
fn census_only_improves() {
    let rows = parse_ledger(&read(LEDGER)).expect("ledger parses");
    let recorded = parse_census(&read(CENSUS)).expect("census parses");
    validate_census(&rows, recorded).expect("census only improves");
}

/// The gate must be observed failing (canary discipline): each malformed
/// ledger shape below must be rejected, not tolerated.
#[test]
fn gate_rejects_bad_ledgers() {
    let ungraduated = "op: Gemm; backend: ffi(cublas); impl: gemm; graduation: none;\n";
    let err = parse_ledger(ungraduated).expect_err("ungraduated ffi row must be rejected");
    assert!(err.contains("retirement"), "unexpected error: {err}");

    let missing_graduation = "op: Gemm; backend: model(openvino); impl: gemm;\n";
    assert!(
        parse_ledger(missing_graduation).is_err(),
        "a delegated row with no graduation field must be rejected"
    );

    let bad_backend = "op: Gemm; backend: cublas; impl: gemm; graduation: none;\n";
    assert!(
        parse_ledger(bad_backend).is_err(),
        "a bare library name is not a backend form"
    );

    let duplicate = "op: Gemm; backend: briev; impl: gemm; graduation: none;\n\
                     op: Gemm; backend: briev; impl: gemm; graduation: none;\n";
    assert!(
        parse_ledger(duplicate).is_err(),
        "duplicate op rows must be rejected"
    );

    let ok = parse_ledger(
        "op: Gemm; backend: ffi(cublas); impl: gemm; \
         graduation: Gemm 512x512x512 @ ptl vs cublas — within 3%;\n",
    )
    .expect("a graduated delegation is valid");
    assert_eq!(ok.len(), 1);
    assert!(ok[0].is_delegated());

    // Coverage in both directions.
    let rows = vec![Row {
        op: "Gemm".into(),
        backend: BACKEND_BRIEV.into(),
        imp: "gemm".into(),
        graduation: GRADUATION_NONE.into(),
        evidence: String::new(),
    }];
    let dispatched = vec!["Gemm".to_string(), "Conv".to_string()];
    let err = validate_coverage(&rows, &dispatched).expect_err("Conv must be reported missing");
    assert!(err.contains("Conv"), "unexpected error: {err}");

    let extra = vec![
        rows[0].clone(),
        Row {
            op: "Ghost".into(),
            backend: BACKEND_BRIEV.into(),
            imp: "gemm".into(),
            graduation: GRADUATION_NONE.into(),
            evidence: String::new(),
        },
    ];
    let err = validate_coverage(&extra, &["Gemm".to_string()]).expect_err("Ghost is an orphan");
    assert!(err.contains("Ghost"), "unexpected error: {err}");

    // A `briev` row may not coexist with FFI in the dispatcher.
    let err = validate_briev_is_ffi_free(&rows, "frgn ov_core_create(core: Int) from #Link<x>;")
        .expect_err("FFI in the dispatcher must contradict a briev row");
    assert!(err.contains("FFI"), "unexpected error: {err}");

    // The census ratchet must catch a regression in either direction.
    let err = validate_census(&rows, (2, 0, 1)).expect_err("briev below recorded must fail");
    assert!(err.contains("regressed"), "unexpected error: {err}");
}

/// Sanity: the project root resolves to a directory containing our files.
#[test]
fn paths_resolve() {
    for rel in [LEDGER, DISPATCHER, CENSUS] {
        let path: &Path = &root().join(rel);
        assert!(path.is_file(), "{} is missing", path.display());
    }
}
