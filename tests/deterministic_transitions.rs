//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Scripted transition differential testing; see docs/guides/deterministic-testing.md.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use briev_compiler::{interpreter::Interpreter, lexer::tokenize, parser::Parser};
use proptest::{
    prelude::*,
    strategy::BoxedStrategy,
    test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Case {
    #[serde(default)]
    family: u8,
    initial: i64,
    threshold: i64,
    bias: i64,
    events: Vec<i64>,
}

fn cases(family: u8) -> BoxedStrategy<Case> {
    (
        -100i64..101,
        -50i64..51,
        -10i64..11,
        prop::collection::vec(-20i64..21, 1..17),
    )
        .prop_map(move |(initial, threshold, bias, events)| Case {
            family,
            initial,
            threshold,
            bias,
            events,
        })
        .boxed()
}

#[test]
fn reference_preserves_guard_boundary_and_event_order() {
    let case = Case {
        family: 0,
        initial: 7,
        threshold: 0,
        bias: 3,
        events: vec![0, 2, -4, 1],
    };
    assert_eq!(reference(&case).unwrap(), vec![7, 12, 16, 20]);
}

#[test]
fn reference_campaign_families_have_known_traces() {
    let traces = [
        [7, 12, 16, 20],
        [1, 0, -1, 0],
        [3, 1, -3, -2],
        [10, 8, 11, 10],
        [10, 15, 18, 22],
        [7, 10, 14, 17],
        [7, 2, -4, 1],
    ];
    for (family, expected) in traces.iter().enumerate() {
        let case = Case {
            family: family as u8,
            initial: 7,
            threshold: 0,
            bias: 3,
            events: vec![0, 2, -4, 1],
        };
        assert_eq!(reference(&case).unwrap(), expected, "family {family}");
    }
}

#[test]
fn reference_collection_and_match_traces() {
    for (family, expected) in [
        (8, [0, 4, -8, 2]),
        (10, [7, 7, 7, 1]),
        (12, [10, 8, 12, 11]),
    ] {
        let case = Case {
            family,
            initial: 7,
            threshold: 0,
            bias: 3,
            events: vec![0, 2, -4, 1],
        };
        assert_eq!(reference(&case).unwrap(), expected, "family {family}");
    }
}

#[test]
fn reference_list_rebinding_preserves_original_value() {
    let case = Case {
        family: 9,
        initial: 7,
        threshold: 0,
        bias: 3,
        events: vec![0, 2, -4, 1],
    };
    assert_eq!(reference(&case).unwrap(), vec![7, 9, 5, 6]);
}

#[test]
fn reference_enum_construction_and_match_trace() {
    // Family 14: construct Mode.Active(e) / Mode.Idle and match it, returning
    // Int. Exercises native enum (Sum) layout + match against the interpreter.
    let case = Case {
        family: 14,
        initial: 7,
        threshold: 0,
        bias: 3,
        events: vec![0, 2, -4, 1],
    };
    assert_eq!(reference(&case).unwrap(), vec![7, 9, 5, 6]);
}

#[test]
#[ignore = "confirmed interpreter tuple projection defect; see transition campaign report"]
fn interpreter_tuple_projection_reproducer() {
    let case = Case {
        family: 7,
        initial: 0,
        threshold: 0,
        bias: 0,
        events: vec![0],
    };
    assert_eq!(reference(&case).unwrap(), vec![0]);
}

#[test]
#[ignore = "confirmed interpreter tuple destructuring defect; see transition campaign report"]
fn interpreter_tuple_destructuring_reproducer() {
    let case = Case {
        family: 11,
        initial: 0,
        threshold: 0,
        bias: 0,
        events: vec![0],
    };
    assert_eq!(reference(&case).unwrap(), vec![0]);
}

fn function(case: &Case) -> String {
    let bias = case.bias;
    let threshold = case.threshold;
    let divisor = bias.abs() + 1;
    let body = match case.family {
        0 => format!("when e > {threshold} {{ term s + e + ({bias}); }}; term s - e;"),
        1 => format!("term (s + e) / {divisor};"),
        2 => format!("term (s + e) % {divisor};"),
        3 => format!("when e > {threshold} {{ when s < 0 {{ term s + e; }}; term s - e; }}; term s + ({bias});"),
        4 => format!("let x: Int = s; when e > {threshold} {{ x = x + e; }}; x = x + ({bias}); term x;"),
        5 => format!("when e != 0 && s / e > {threshold} {{ term s + ({bias}); }}; term s - e;"),
        6 => format!("let values: List<Int> = [s, e, ({bias})]; term values[(e * e) % 3];"),
        7 => format!("let pair = (s + e, s - e); when e > {threshold} {{ term pair.0; }}; term pair.1;"),
        8 => "let values: List<Int> = [s, e]; values[0] = e; term values[0] + values[1];".into(),
        9 => "let values: List<Int> = [s, e]; let alias: List<Int> = values; alias[0] = e; term values[0] + alias[0];".into(),
        10 => "let values = make_values(s, e); term values[(e * e) % 2];".into(),
        11 => "let pair = (s + e, s - e); let (left, right) = pair; term left - right;".into(),
        12 => format!("match e {{ 0 => {{ when s > {threshold} {{ term s + ({bias}); }}; }}, _ => {{ term s - e; }} }} term s + e;"),
        14 => "let k: Mode = Mode.Idle; when e != 0 { k = Mode.Active(e); }; term match k { Mode.Active(n) => s + n, Mode.Idle => s - e, };".into(),
        _ => panic!("unknown transition family {}", case.family),
    };
    let helper = if case.family == 10 {
        "defn make_values(s: Int, e: Int) -> List<Int> { term [s, e]; };\n"
    } else if case.family == 14 {
        "enum Mode { Idle, Active(Int) };\n"
    } else {
        ""
    };
    format!("{helper}defn step(s: Int, e: Int) -> Int {{\n{body}\n}};\n")
}

fn reference(case: &Case) -> Result<Vec<i64>, String> {
    let mut source = function(case);
    source.push_str(&format!("let sample_0: Int = {};\n", case.initial));
    for (index, event) in case.events.iter().enumerate() {
        source.push_str(&format!(
            "let sample_{}: Int = step(sample_{index}, ({event}));\n",
            index + 1
        ));
    }
    let items = reference_items(&source)?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let briev_compiler::ast::TopLevel::Statement(stmt) = item {
            interpreter.exec_stmt(stmt).map_err(|e| e.to_string())?;
        }
    }
    (1..=case.events.len())
        .map(|i| {
            interpreter
                .state
                .get(&format!("sample_{i}"))
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("missing interpreter sample {i}"))
        })
        .collect()
}

fn reference_items(source: &str) -> Result<Vec<briev_compiler::ast::TopLevel>, String> {
    if std::env::var_os("BRIEV_SIM_TYPED_REFERENCE").is_some() {
        let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
        let path = directory.path().join("reference.bv");
        fs::write(&path, source).map_err(|e| e.to_string())?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let options = briev_compiler::pipeline::BuildOptions {
            file_path: path.to_string_lossy().into_owned(),
            stdlib_path: Some(root.join("lib").to_string_lossy().into_owned()),
            config_dir: Some(root.join("config").to_string_lossy().into_owned()),
            ..Default::default()
        };
        return briev_compiler::pipeline::compile_to_typed(
            &path.to_string_lossy(),
            source,
            &options,
        )
        .map(|(items, _)| items)
        .map_err(|e| e.to_string());
    }
    let tokens = tokenize(source).map_err(|e| e.to_string())?;
    Parser::new(tokens, source)
        .parse_program()
        .map_err(|e| e.to_string())
}

fn checked(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|e| format!("{command:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?}: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn native_trace(binary: &Path, case: &Case) -> Result<Vec<i64>, String> {
    let mut state = case.initial;
    let mut trace = Vec::new();
    for event in &case.events {
        let text = checked(
            Command::new("timeout")
                .arg("10")
                .arg(binary)
                .env("SIM_STATE", state.to_string())
                .env("SIM_EVENT", event.to_string()),
        )?;
        state = text
            .trim()
            .parse()
            .map_err(|e| format!("native output {text:?}: {e}"))?;
        trace.push(state);
    }
    Ok(trace)
}

fn compare(case: &Case, directory: &Path) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = format!(
        "{}\nnode entry [beginprogram][true] {{\nPrint#(step(GetEnvInt#(\"SIM_STATE\"), GetEnvInt#(\"SIM_EVENT\")));\nterm;\n}};\n",
        function(case)
    );
    let path = directory.join("transition.bv");
    fs::write(&path, source).map_err(|e| e.to_string())?;
    let expected = reference(case)?;
    checked(
        Command::new("timeout")
            .arg("60")
            .arg(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .env_remove("BRIEV_HIR_EMIT")
            .arg("build")
            .arg(&path)
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory),
    )?;
    for level in ["-O0", "-O3"] {
        let binary = directory.join(format!("transition{level}"));
        checked(
            Command::new("timeout")
                .arg("60")
                .arg("clang")
                .arg(level)
                .arg(directory.join("transition.ll"))
                .arg(root.join("lib/runtime/briev_rt.c"))
                .args(["-lm", "-ldl", "-o"])
                .arg(&binary),
        )?;
        let mut actual = native_trace(&binary, case)?;
        // 2026-11-09: observation-only mutation proves detection and shrinking;
        // remove this opt-in hook when an equivalent harness mutation gate replaces it.
        if std::env::var_os("BRIEV_SIM_DEMO_FAULT").is_some() {
            actual[0] += 1;
        }
        if actual != expected {
            return Err(format!(
                "{level} trace mismatch\ninterpreter: {expected:?}\nnative: {actual:?}"
            ));
        }
    }
    Ok(())
}

fn exercise(case: &Case, artifact: &Path) -> Result<(), TestCaseError> {
    let directory = tempfile::tempdir().map_err(|e| TestCaseError::fail(e.to_string()))?;
    if let Err(error) = compare(case, directory.path()) {
        fs::create_dir_all(artifact).expect("create simulation artifact directory");
        fs::write(
            artifact.join("case.json"),
            serde_json::to_vec_pretty(case).unwrap(),
        )
        .unwrap();
        fs::write(artifact.join("failure.txt"), &error).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let metadata = serde_json::json!({
            "schema": 1,
            "revision": checked(Command::new("git").current_dir(root).args(["rev-parse", "HEAD"])),
            "working_tree": checked(Command::new("git").current_dir(root).args(["status", "--short"])),
            "clang": checked(Command::new("clang").arg("--version")),
            "rustc": checked(Command::new("rustc").arg("--version")),
            "demo_fault": std::env::var_os("BRIEV_SIM_DEMO_FAULT").is_some(),
            "typed_reference": std::env::var_os("BRIEV_SIM_TYPED_REFERENCE").is_some(),
            "seed": std::env::var("BRIEV_SIM_SEED").unwrap_or_else(|_| "1".into()),
            "target": "x86_64-linux", "optimization_levels": ["-O0", "-O3"]
        });
        fs::write(
            artifact.join("metadata.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();
        for name in ["transition.bv", "transition.ll"] {
            let source = directory.path().join(name);
            if source.exists() {
                fs::copy(source, artifact.join(name)).unwrap();
            }
        }
        return Err(TestCaseError::fail(error));
    }
    Ok(())
}

#[test]
fn scripted_integer_transitions() {
    let artifact = std::env::var_os("BRIEV_SIM_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/simulation-failure"));
    if let Some(path) = std::env::var_os("BRIEV_SIM_REPLAY") {
        let case: Case = serde_json::from_slice(&fs::read(path).expect("read replay case"))
            .expect("decode replay case");
        assert!(
            !case.events.is_empty() && case.events.len() <= 16,
            "replay needs 1..=16 events"
        );
        assert!(
            (-100..=100).contains(&case.initial)
                && (-50..=50).contains(&case.threshold)
                && (-10..=10).contains(&case.bias)
        );
        assert!(case.events.iter().all(|v| (-20..=20).contains(v)));
        assert!(case.family <= 14, "family must be 0..=14");
        exercise(&case, &artifact).unwrap();
        return;
    }
    let seed: u64 = std::env::var("BRIEV_SIM_SEED")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .expect("integer seed");
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let family: u8 = std::env::var("BRIEV_SIM_FAMILY")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .expect("integer family");
    assert!(family <= 14, "family must be 0..=14");
    let count = std::env::var("BRIEV_SIM_CASES")
        .unwrap_or_else(|_| "4".into())
        .parse()
        .expect("positive case count");
    assert!(count > 0, "case count must be positive");
    let config = Config {
        cases: count,
        max_shrink_iters: 32,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    eprintln!(
        "simulation family={family} seed={seed} cases={count}; failures: {}",
        artifact.display()
    );
    let result = runner.run(&cases(family), |case| exercise(&case, &artifact));
    if let Err(proptest::test_runner::TestError::Fail(_, case)) = &result {
        // Save the final counterexample, not the last attempted shrink candidate.
        let _ = exercise(case, &artifact);
    }
    result.unwrap();
}
