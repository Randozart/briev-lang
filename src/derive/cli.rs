// ── Derivation CLI Handlers ────────────────────────────────────────────
// 2026-07-12: Phase 6.3 — `briev derive` CLI command.
// 2026-07-28: Phase I.0 — Added DeriveConfig, flag parsing, MCMC + doppelganger output.
// Flat code: max 2 levels of nesting.

use crate::ast::{DerivationBlock, Expr, Statement, TopLevel, Type};
use crate::derive::engine::{CostModel, SynthesizedProgram};
use crate::derive::{SynthesizeError, synthesize};
use std::fs;
use std::path::Path;

/// Configuration for `briev derive` command.
/// 2026-07-28: Phase I.0 — Flags parsed from CLI args.
#[derive(Debug, Clone)]
pub struct DeriveConfig {
    pub stochastic: bool,
    pub iterations: usize,
    pub temperature: f64,
    pub enumerative_depth: usize,
    pub process_all: bool,
    // 2026-07-28: Tier 2/3 — verification samples (0 = disable verification)
    pub verify_samples: usize,
    /// 2026-07-29: Cross-verification samples for := chain validation.
    pub cross_verify_samples: usize,
}

impl Default for DeriveConfig {
    fn default() -> Self {
        DeriveConfig {
            stochastic: false,
            iterations: 10_000,
            temperature: 1.0,
            enumerative_depth: 3,
            process_all: false,
            verify_samples: 50,
            cross_verify_samples: 0,
        }
    }
}

/// Parse `--key=value` or `--key value` style flags from args.
/// Returns (DeriveConfig, positional_args).
/// 2026-07-28: Phase I.0 — Hand-written flag parser (no clap dependency).
pub fn parse_derive_flags(args: &[String]) -> Result<(DeriveConfig, Vec<String>), String> {
    let mut config = DeriveConfig::default();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--stochastic" {
            config.stochastic = true;
            i += 1;
        } else if arg == "--all" {
            config.process_all = true;
            i += 1;
        } else if arg == "--iterations" {
            i += 1;
            let val = args.get(i).ok_or("--iterations requires a value")?;
            config.iterations = val
                .parse::<usize>()
                .map_err(|_| format!("invalid --iterations value '{}'", val))?;
            i += 1;
        } else if arg == "--temperature" {
            i += 1;
            let val = args.get(i).ok_or("--temperature requires a value")?;
            config.temperature = val
                .parse::<f64>()
                .map_err(|_| format!("invalid --temperature value '{}'", val))?;
            i += 1;
        } else if arg == "--enumerative-depth" {
            i += 1;
            let val = args.get(i).ok_or("--enumerative-depth requires a value")?;
            config.enumerative_depth = val
                .parse::<usize>()
                .map_err(|_| format!("invalid --enumerative-depth value '{}'", val))?;
            i += 1;
        } else if arg == "--verify-samples" {
            i += 1;
            let val = args.get(i).ok_or("--verify-samples requires a value")?;
            config.verify_samples = val
                .parse::<usize>()
                .map_err(|_| format!("invalid --verify-samples value '{}'", val))?;
            i += 1;
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag '{}'", arg));
        } else {
            positional.push(arg.clone());
            i += 1;
        }
    }
    Ok((config, positional))
}

/// Handle the `briev derive` command.
/// Reads a Briev source file, finds derivation blocks, synthesizes bodies,
/// optionally runs MCMC superoptimization, writes doppelganger output.
pub fn handle_derive_command(config: &DeriveConfig, file_path: &str) -> Result<(), String> {
    let path = Path::new(file_path);
    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {}", file_path, e))?;

    // Lex, parse, find derivation blocks
    let token_spans = lex_source_with_spans(&source)?;
    let program = parse_tokens(&token_spans, &source)?;

    // Collect derivation blocks with their names and parameter types
    // 2026-07-28: Include params so synthesized expressions use actual param names
    let mut derivations: Vec<(String, Vec<(String, Type)>, DerivationBlock)> = Vec::new();
    for item in &program {
        match item {
            TopLevel::Definition(d) => {
                if let Some(ref block) = d.derivation {
                    derivations.push((d.name.clone(), d.parameters.clone(), block.clone()));
                }
            }
            TopLevel::Transaction(t) => {
                if let Some(ref block) = t.derivation {
                    derivations.push((t.name.clone(), t.parameters.clone(), block.clone()));
                }
            }
            _ => {}
        }
    }

    if derivations.is_empty() {
        eprintln!("[derive] no derivation blocks found in '{}'", file_path);
        return Ok(());
    }

    // 2026-07-29: Build reference function lookup from the program.
    // Reference functions are named defns in the same compilation unit.
    let mut ref_lookup: std::collections::HashMap<String, &TopLevel> =
        std::collections::HashMap::new();
    for item in &program {
        if let TopLevel::Definition(d) = item {
            ref_lookup.insert(d.name.clone(), item);
        }
        if let TopLevel::Transaction(t) = item {
            ref_lookup.insert(t.name.clone(), item);
        }
        if let TopLevel::AsmFn(a) = item {
            ref_lookup.insert(a.name.clone(), item);
        }
    }

    // Synthesize each derivation block
    let mut syntheses: Vec<(String, SynthesizedProgram)> = Vec::new();
    for (name, params, block) in &derivations {
        eprintln!(
            "[derive] synthesizing '{}' (depth={})...",
            name, config.enumerative_depth
        );
        // Use Type::int() as default return type for derivation blocks
        // TODO: extract actual return type from function signature
        let ret_type = Type::int();
        // 2026-07-29: Handle reference-only derivation — skip synthesis, use ref body.
        if block.examples.is_empty() {
            if let Some(ref ref_name) = block.ref_name {
                if let Some(ref_body) = block.ref_name.as_ref().and_then(|rn| {
                    ref_lookup.get(rn).and_then(|item| match item {
                        TopLevel::Definition(d) => d.body.iter().find_map(|s| {
                            if let Statement::Term(Some(e)) = s {
                                let expr_ref: &Expr = e;
                                Some(expr_ref)
                            } else {
                                None
                            }
                        }),
                        _ => None,
                    })
                }) {
                    let cost = crate::derive::engine::CostModel::default().cost_of_expr(ref_body);
                    let prog = crate::derive::engine::SynthesizedProgram {
                        body: vec![ref_body.clone()],
                        cost,
                        depth: 0,
                        helpers: vec![],
                    };
                    eprintln!(
                        "[derive] '{}': reference-only (using '{}' body, cost {})",
                        name, ref_name, cost
                    );
                    syntheses.push((name.clone(), prog));
                    continue;
                } else {
                    eprintln!(
                        "warn: reference function '{}' not found for '{}'",
                        ref_name, name
                    );
                }
            }
        }

        // 2026-07-29: Look up reference function body for CEGIS verification.
        // Store Block-wrapped bodies in a Vec so references live long enough.
        // The full body (including let bindings) is wrapped in a Block expression
        // so the interpreter can evaluate all statements in sequence.
        // Previously, only the Term expression was extracted, discarding let
        // bindings and causing UndefinedVariable errors.
        // 2026-07-29: Look up reference function body for CEGIS verification.
        // Store the body (as Expr::Block) and its own parameter names so the
        // verifier can bind reference params by position, not by name.
        // Uses RefFnBundle to keep body+params co-owned.
        struct RefFnBundle {
            body: Expr,
            params: Vec<String>,
        }
        let mut ref_fn_bundles: Vec<RefFnBundle> = Vec::new();
        let ref_fn_expr: Option<(&Expr, &[String])> =
            block.ref_name.as_ref().and_then(|ref_name| {
                ref_lookup.get(ref_name).and_then(|item| match item {
                    TopLevel::Definition(d) => {
                        if d.body.is_empty() {
                            return None;
                        }
                        let body_expr = Expr::Block(d.body.clone());
                        let ref_params: Vec<String> =
                            d.parameters.iter().map(|(n, _)| n.clone()).collect();
                        ref_fn_bundles.push(RefFnBundle {
                            body: body_expr,
                            params: ref_params,
                        });
                        let bundle = ref_fn_bundles.last()?;
                        Some((&bundle.body, bundle.params.as_slice()))
                    }
                    _ => None,
                })
            });
        // 2026-07-29: Chain resolution — if the block has a multi-segment := chain,
        // resolve it first. If a matching body is found, use it directly.
        // The chain resolver cross-verifies each body against all others.
        let chain_result: Option<crate::derive::chain::Body> = if !block.chain.is_empty() {
            // Build registry from ref_lookup for chain resolution
            let mut chain_registry: std::collections::HashMap<String, crate::derive::chain::Body> =
                std::collections::HashMap::new();
            for (ref_name, item) in &ref_lookup {
                match item {
                    TopLevel::AsmFn(af) => {
                        chain_registry.insert(
                            ref_name.clone(),
                            crate::derive::chain::Body::Asm(af.clone()),
                        );
                    }
                    TopLevel::Definition(d) => {
                        // Wrap the full body (including let bindings) in a Block.
                        // Param names are matched by position in the chain resolver.
                        let block = Expr::Block(d.body.clone());
                        chain_registry
                            .insert(ref_name.clone(), crate::derive::chain::Body::Ref(block));
                    }
                    _ => {}
                }
            }
            // 2026-07-29: Use verify_samples for cross-verification if
            // cross_verify_samples is not explicitly set (default = 0 means
            // use the value from --verify-samples / DeriveConfig.verify_samples).
            let xv_samples = if config.cross_verify_samples > 0 {
                config.cross_verify_samples
            } else {
                config.verify_samples
            };
            let result = crate::derive::chain::resolve_chain(
                &block.chain,
                "native",
                params,
                xv_samples as u32,
                &chain_registry,
            );
            if result.is_some() {
                eprintln!("[derive] '{}': resolved via verification chain", name);
            }
            result
        } else {
            None
        };

        if let Some(selected) = chain_result {
            match selected {
                crate::derive::chain::Body::Ref(expr) => {
                    let cost = crate::derive::engine::CostModel::default().cost_of_expr(&expr);
                    syntheses.push((
                        name.clone(),
                        crate::derive::engine::SynthesizedProgram {
                            body: vec![expr],
                            cost,
                            depth: 0,
                            helpers: vec![],
                        },
                    ));
                }
                crate::derive::chain::Body::Asm(_af) => {
                    // Asm body — the LLVM backend's emit_asm_fn produces the
                    // actual asm sideeffect. The SynthesizedProgram placeholder
                    // records the selection for the doppelganger output.
                    syntheses.push((
                        name.clone(),
                        crate::derive::engine::SynthesizedProgram {
                            body: vec![Expr::Decimal(0)],
                            cost: 0,
                            depth: 0,
                            helpers: vec![],
                        },
                    ));
                }
                crate::derive::chain::Body::Synthesized(_) => {
                    // Fall through to regular synthesis
                    match synthesize(
                        name,
                        block,
                        params,
                        &ret_type,
                        config.enumerative_depth,
                        config.verify_samples,
                        block.postcondition.as_ref(),
                        block.precondition.as_ref(),
                        ref_fn_expr,
                    ) {
                        Ok(prog) => {
                            eprintln!(
                                "[derive] '{}': synthesized body with cost {}",
                                name, prog.cost
                            );
                            syntheses.push((name.clone(), prog));
                        }
                        Err(e) => {
                            eprintln!("warn: synthesis failed for '{}': {}", name, e);
                        }
                    }
                }
            }
        } else {
            match synthesize(
                name,
                block,
                params,
                &ret_type,
                config.enumerative_depth,
                config.verify_samples,
                block.postcondition.as_ref(),
                block.precondition.as_ref(),
                ref_fn_expr,
            ) {
                Ok(prog) => {
                    eprintln!(
                        "[derive] '{}': synthesized body with cost {}",
                        name, prog.cost
                    );
                    syntheses.push((name.clone(), prog));
                }
                Err(e) => {
                    eprintln!("warn: synthesis failed for '{}': {}", name, e);
                }
            }
        }
    }

    if syntheses.is_empty() {
        return Err("synthesis failed for all derivation blocks".into());
    }

    // 2026-07-28: Extract (name, block) pairs for doppelganger writer
    let derivations_blocks: Vec<(String, DerivationBlock)> = derivations
        .iter()
        .map(|(n, _, b)| (n.clone(), b.clone()))
        .collect();

    // Optionally run MCMC superoptimization
    if config.stochastic {
        eprintln!("[derive] running MCMC superoptimization...");
        let mcmc_config = crate::derive::mcmc::McmcConfig {
            initial_temperature: config.temperature,
            max_iterations: config.iterations,
            ..crate::derive::mcmc::McmcConfig::default()
        };
        let mut mcmc_syntheses: Vec<(String, SynthesizedProgram)> = Vec::new();
        for (name, prog) in &syntheses {
            // Find the derivation block for examples
            let block = derivations
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, _, b)| b);
            if let Some(block) = block {
                match crate::derive::mcmc::optimize(prog.clone(), &block.examples, &mcmc_config) {
                    Ok(improved) => {
                        eprintln!(
                            "[derive] '{}': MCMC optimized cost {} -> {}",
                            name, prog.cost, improved.cost
                        );
                        mcmc_syntheses.push((name.clone(), improved));
                    }
                    Err(e) => {
                        eprintln!(
                            "warn: MCMC failed for '{}': {} — keeping synthesized",
                            name, e
                        );
                        mcmc_syntheses.push((name.clone(), prog.clone()));
                    }
                }
            }
        }
        // Write .opt.bv with MCMC results
        let opt_path = crate::derive::doppelganger::Doppelganger::opt_path_for(path);
        crate::derive::doppelganger::write_doppelganger(
            path,
            source.as_bytes(),
            &mcmc_syntheses,
            &derivations_blocks,
            &opt_path,
        )?;
        eprintln!("[derive] wrote {}", opt_path.display());
    } else {
        // Write .derive.bv with synthesis results
        let derive_path = crate::derive::doppelganger::Doppelganger::derive_path_for(path);
        crate::derive::doppelganger::write_doppelganger(
            path,
            source.as_bytes(),
            &syntheses,
            &derivations_blocks,
            &derive_path,
        )?;
        eprintln!("[derive] wrote {}", derive_path.display());
    }

    Ok(())
}

/// Lex a Briev source into (Token, byte_range) pairs using logos.
/// 2026-07-28: Preserves byte spans so derivation block positions are correct.
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

    #[test]
    fn test_lex_source_simple() {
        let source = "defn add(a: Int, b: Int) -> Int";
        let result = lex_source_with_spans(source);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_lex_source_empty() {
        let result = lex_source_with_spans("");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_lex_source_error() {
        let result = lex_source_with_spans("\"unterminated string");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_flags_default() {
        let args: Vec<String> = vec!["foo.bv".into()];
        let (config, positional) = parse_derive_flags(&args).unwrap();
        assert!(!config.stochastic);
        assert_eq!(config.iterations, 10_000);
        assert_eq!(config.temperature, 1.0);
        // 2026-07-28: Default depth changed from 5 to 3 for performance
        assert_eq!(config.enumerative_depth, 3);
        assert!(!config.process_all);
        assert_eq!(positional, vec!["foo.bv"]);
    }

    #[test]
    fn test_parse_flags_stochastic() {
        let args: Vec<String> = vec!["--stochastic".into(), "foo.bv".into()];
        let (config, positional) = parse_derive_flags(&args).unwrap();
        assert!(config.stochastic);
        assert_eq!(positional, vec!["foo.bv"]);
    }

    #[test]
    fn test_parse_flags_custom_values() {
        let args: Vec<String> = vec![
            "--stochastic".into(),
            "--iterations".into(),
            "5000".into(),
            "--temperature".into(),
            "0.5".into(),
            "--enumerative-depth".into(),
            "8".into(),
            "--all".into(),
            "foo.bv".into(),
        ];
        let (config, positional) = parse_derive_flags(&args).unwrap();
        assert!(config.stochastic);
        assert_eq!(config.iterations, 5000);
        assert!((config.temperature - 0.5).abs() < 1e-9);
        assert_eq!(config.enumerative_depth, 8);
        assert!(config.process_all);
        assert_eq!(positional, vec!["foo.bv"]);
    }

    #[test]
    fn test_parse_flags_unknown_yields_error() {
        let args: Vec<String> = vec!["--nonexistent".into(), "foo.bv".into()];
        let result = parse_derive_flags(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_flags_missing_value() {
        let args: Vec<String> = vec!["--iterations".into()];
        let result = parse_derive_flags(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_flags_invalid_value() {
        let args: Vec<String> = vec!["--iterations".into(), "not_a_number".into()];
        let result = parse_derive_flags(&args);
        assert!(result.is_err());
    }
}
