// ── Phase E — Doppelganger Write-Back ─────────────────────────────────
// 2026-07-28: Phase E.0 — Doppelganger file management (path resolution,
// full-source writer, build system integration).
// Flat code: each function max 2 levels of nesting.

use crate::ast::{DerivationBlock, Expr};
use crate::derive::engine::SynthesizedProgram;
use std::path::{Path, PathBuf};

/// 2026-07-28: Phase E.0 — Doppelganger file management.
pub struct Doppelganger;

impl Doppelganger {
    /// Determine the doppelganger path for a given source file.
    /// Replaces `.bv` with `.derive.bv`.
    pub fn derive_path_for(source: &Path) -> PathBuf {
        let stem = source.file_stem().unwrap_or_default();
        source.with_file_name(format!("{}.derive.bv", stem.to_string_lossy()))
    }

    /// Determine the optimized doppelganger path for a given source file.
    /// Replaces `.bv` with `.opt.bv`.
    pub fn opt_path_for(source: &Path) -> PathBuf {
        let stem = source.file_stem().unwrap_or_default();
        source.with_file_name(format!("{}.opt.bv", stem.to_string_lossy()))
    }

    /// Resolve the best available source for a given file path.
    /// Order: .opt.bv > .derive.bv > .bv
    pub fn resolve(source: &Path) -> PathBuf {
        let opt = Self::opt_path_for(source);
        if opt.exists() {
            return opt;
        }
        let derive = Self::derive_path_for(source);
        if derive.exists() {
            return derive;
        }
        source.to_path_buf()
    }

    /// Format a synthesized program body as a Briev source string.
    pub fn format_body(prog: &SynthesizedProgram) -> String {
        let mut out = String::new();
        for expr in &prog.body {
            out.push_str(&format!("{}", expr));
        }
        out
    }

    /// 2026-07-28: Format an ite chain (Expr::If) as `when` guards with `term`.
    /// The if-then-else chain from SMT ite is converted to valid Briev:
    ///   when cond1 { term val1; };
    ///   when cond2 { term val2; };
    ///   term else_val;
    /// Returns None if the body is not an ite chain (use format_body instead).
    pub fn format_ite_body(prog: &SynthesizedProgram) -> Option<String> {
        let expr = prog.body.first()?;
        if !matches!(expr, Expr::If(_, _, _)) {
            return None;
        }
        let mut out = String::new();
        fn decompose_ite(expr: &Expr, out: &mut String, indent: usize) {
            match expr {
                Expr::If(cond, then, else_) => {
                    let pad = "    ".repeat(indent);
                    out.push_str(&format!(
                        "{}when {} {{\n{}        term {};\n{}}};\n",
                        pad, cond, pad, then, pad
                    ));
                    if let Some(e) = else_ {
                        decompose_ite(e, out, indent);
                    }
                }
                _ => {
                    let pad = "    ".repeat(indent);
                    out.push_str(&format!("{}term {};\n", pad, expr));
                }
            }
        }
        decompose_ite(expr, &mut out, 1);
        Some(out)
    }
}

/// Write the doppelganger file with synthesized bodies injected.
/// 2026-07-28: Phase E.1 — full-source doppelganger writer.
pub fn write_doppelganger(
    source_path: &Path,
    source_bytes: &[u8],
    syntheses: &[(String, SynthesizedProgram)],
    derivations: &[(String, DerivationBlock)],
    output_path: &Path,
) -> Result<(), String> {
    let mut bytes = source_bytes.to_vec();

    // Collect insertions: (byte_offset, body_string)
    // Insert body BEFORE the derivation block's opening position
    let mut insertions: Vec<(usize, String)> = Vec::new();
    for ((name, prog), (_, block)) in syntheses.iter().zip(derivations.iter()) {
        let insert_at = block.span.start as usize;
        // 2026-07-28: Use `term expr;` — Briev's termination statement.
        // In a callable txn / defn, `term expr` stores the value to %result and
        // branches to the convergence check. This is the correct way to return
        // a value from a synthesized body.
        // For ite chains (SMT results), use when-guard format instead.
        let mut body_str = if let Some(ite_body) = Doppelganger::format_ite_body(prog) {
            format!(" {{\n{}}} ", ite_body)
        } else {
            format!(" {{\n    term {};\n}} ", Doppelganger::format_body(prog))
        };
        // 2026-07-29: Prepend helper defn blocks before the synthesized body.
        // Only helpers with use_count > 0 are emitted. This is the ephemeral
        // library concept: helpers created during search are only persisted
        // in output if consumed by the final expression.
        if !prog.helpers.is_empty() {
            let mut helpers_str = String::from("\n");
            for h in &prog.helpers {
                if h.use_count == 0 {
                    continue;
                }
                let params: Vec<String> = h
                    .params
                    .iter()
                    .zip(h.param_types.iter())
                    .map(|(n, t)| format!("{}: {}", n, t))
                    .collect();
                helpers_str.push_str(&format!(
                    "// 2026-07-29: Auto-discovered helper (abstraction discovery)\n\
                     defn {}({}) -> {} {{ {} }};\n\n",
                    h.name,
                    params.join(", "),
                    h.ret_type,
                    Doppelganger::format_body(&SynthesizedProgram {
                        body: vec![h.body.clone()],
                        cost: h.body_cost,
                        depth: 0,
                        helpers: vec![],
                    }),
                ));
            }
            if !helpers_str.trim().is_empty() {
                body_str = format!("{}{}", helpers_str, body_str);
            }
        }
        eprintln!("[derive] {}: inserting body at byte {}", name, insert_at);
        insertions.push((insert_at, body_str));
    }

    // Sort in reverse byte order to preserve offsets during insertion
    insertions.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));

    for (offset, body_str) in insertions {
        let mut new = Vec::with_capacity(bytes.len() + body_str.len());
        new.extend_from_slice(&bytes[..offset]);
        new.extend_from_slice(body_str.as_bytes());
        new.extend_from_slice(&bytes[offset..]);
        bytes = new;
    }

    std::fs::write(output_path, &bytes)
        .map_err(|e| format!("cannot write '{}': {}", output_path.display(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Expr};
    use crate::errors::Span;
    use std::path::Path;

    fn dummy_span() -> Span {
        Span::dummy()
    }

    fn make_prog(body: Vec<Expr>, cost: u64) -> SynthesizedProgram {
        SynthesizedProgram {
            body,
            cost,
            depth: 0,
            helpers: vec![],
        }
    }

    #[test]
    fn test_derive_path_for() {
        let p = Path::new("foo.bv");
        assert_eq!(Doppelganger::derive_path_for(p), Path::new("foo.derive.bv"));
    }

    #[test]
    fn test_opt_path_for() {
        let p = Path::new("foo.bv");
        assert_eq!(Doppelganger::opt_path_for(p), Path::new("foo.opt.bv"));
    }

    #[test]
    fn test_resolve_source_only() {
        // No actual files exist, so resolve should return the original
        let p = Path::new("nonexistent.bv");
        let resolved = Doppelganger::resolve(p);
        assert_eq!(resolved, p);
    }

    #[test]
    fn test_write_doppelganger_draft() {
        // Use a temporary directory for the output
        let dir = std::env::temp_dir().join("derive_test_write");
        let _ = std::fs::create_dir_all(&dir);
        let source_path = dir.join("test.bv");
        let output_path = Doppelganger::derive_path_for(&source_path);

        let source = "defn add(x: Int, y: Int) -> Int := { 2, 2 -> 4; };";
        std::fs::write(&source_path, source).unwrap();

        // Create a derivation block with span pointing to the `:=` area
        let block = DerivationBlock {
            examples: vec![],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };

        let prog = make_prog(
            vec![Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("y".into())),
            )],
            3,
        );

        write_doppelganger(
            &source_path,
            source.as_bytes(),
            &[("add".to_string(), prog)],
            &[("add".to_string(), block)],
            &output_path,
        )
        .unwrap();

        assert!(output_path.exists(), "doppelganger file should exist");
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("x + y"), "should contain synthesized body");
        assert!(
            content.contains(":="),
            "should still contain derivation block"
        );

        // Cleanup
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&output_path);
    }

    #[test]
    fn test_write_doppelganger_multi_function() {
        let dir = std::env::temp_dir().join("derive_test_multi");
        let _ = std::fs::create_dir_all(&dir);
        let source_path = dir.join("multi.bv");
        let output_path = Doppelganger::derive_path_for(&source_path);

        let source = "defn add(x: Int, y: Int) -> Int := { 2, 2 -> 4; };\ndefn double(x: Int) -> Int := { 3, 3 -> 6; };";
        std::fs::write(&source_path, source).unwrap();

        let block1 = DerivationBlock {
            examples: vec![],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let block2 = DerivationBlock {
            examples: vec![],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let block2 = DerivationBlock {
            examples: vec![],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };

        let prog1 = make_prog(
            vec![Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("y".into())),
            )],
            3,
        );
        let prog2 = make_prog(
            vec![Expr::BinaryOp(
                BinaryOpKind::Mul,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(2)),
            )],
            4,
        );

        write_doppelganger(
            &source_path,
            source.as_bytes(),
            &[("add".to_string(), prog1), ("double".to_string(), prog2)],
            &[("add".to_string(), block1), ("double".to_string(), block2)],
            &output_path,
        )
        .unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("x + y"));
        assert!(content.contains("x * 2"));

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&output_path);
    }
}
