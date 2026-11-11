// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10 (SPEC §8.11): reference-interpreter engine for `test` blocks.
//!
//! `test domain { expect …; };` declarations are typechecked but never
//! lowered or emitted. `brievc test` compiles the file to typed AST, strips
//! the test blocks for program setup, and executes each body in a FRESH
//! interpreter (fresh `init` seeding; program globals/imports shared
//! read-only). The interpreter is the reference engine (Rule 5).

use crate::ast::{Statement, TopLevel};
use crate::interpreter::Interpreter;
use crate::pipeline::{compile_to_typed, BuildOptions};

/// Result of compiling a file for tests: `None` means the file declares no
/// test blocks; `Some` may have an empty report list when `--domain` matched
/// nothing.
#[derive(Debug, Clone)]
pub struct TestRun {
    pub reports: Vec<TestReport>,
    pub domains: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TestReport {
    /// `domain#n` (n is 1-based within the domain, in source order).
    pub label: String,
    pub passed: bool,
    /// Failure text (empty on pass).
    pub detail: String,
}

/// Does the source lex to a `test` keyword? Cheap pre-check before paying for
/// a typed compile in the runner.
pub fn source_has_tests(source: &str) -> bool {
    crate::lexer::tokenize(source)
        .map(|tokens| tokens.iter().any(|(t, _)| matches!(t, crate::lexer::Token::Test)))
        .unwrap_or(false)
}

/// Compile and run every test block in `source`. `domain` filters by exact
/// domain label. `Ok(None)` = no test blocks; `Ok(Some(empty))` = blocks exist
/// but none match the filter.
pub fn run_tests(
    file_path: &str,
    source: &str,
    domain: Option<&str>,
) -> Result<Option<TestRun>, String> {
    let opts = BuildOptions {
        file_path: file_path.to_string(),
        ..Default::default()
    };
    let (items, _universe) = compile_to_typed(file_path, source, &opts)?;

    let mut domains: Vec<String> = Vec::new();
    let mut all_tests: Vec<(String, Vec<Statement>)> = Vec::new();
    for item in &items {
        if let TopLevel::TestBlock(t) = item {
            if !domains.contains(&t.domain) {
                domains.push(t.domain.clone());
            }
            all_tests.push((t.domain.clone(), t.body.clone()));
        }
    }
    if all_tests.is_empty() {
        return Ok(None);
    }
    let tests: Vec<(String, Vec<Statement>)> = all_tests
        .into_iter()
        .filter(|(d, _)| domain.map(|want| want == d).unwrap_or(true))
        .collect();

    let mut program: Vec<TopLevel> = items
        .iter()
        .filter(|item| !matches!(item, TopLevel::TestBlock(_)))
        .cloned()
        .collect();

    let mut reports = Vec::with_capacity(tests.len());
    let mut per_domain: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (domain_name, body) in tests {
        let ordinal = per_domain.entry(domain_name.clone()).or_insert(0);
        *ordinal += 1;
        let label = format!("{}#{}", domain_name, ordinal);
        let detail = match run_one(&mut program, &body) {
            Ok(()) => String::new(),
            Err(err) => err,
        };
        reports.push(TestReport { label, passed: detail.is_empty(), detail });
    }
    Ok(Some(TestRun { reports, domains }))
}

fn run_one(program: &mut [TopLevel], body: &[Statement]) -> Result<(), String> {
    let mut interp = Interpreter::new();
    interp.load_program(program);
    interp
        .seed_inits(program)
        .map_err(|e| format!("seed inits: {e:?}"))?;
    for item in program.iter() {
        if let TopLevel::Statement(stmt) = item {
            interp
                .exec_stmt(stmt)
                .map_err(|e| format!("setup: {e}"))?;
        }
    }
    for stmt in body {
        interp.exec_stmt(stmt).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}
