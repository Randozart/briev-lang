//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored by Otto Westerlund (@odecode); adopted with attribution.
use super::*;

fn metadata_fixture_ir(type_count: usize) -> String {
    let source = "obj MetadataLate { value: Int; }; let count: Int = 0; node run [count < 100][count == 100] { count = count+1; };";
    let tokens = crate::lexer::tokenize(source).expect("tokens");
    let items = crate::parser::Parser::new(tokens, source)
        .parse_program()
        .expect("program");
    let mut universe = crate::type_universe::TypeUniverse::new();
    let integer = universe.get("Int").expect("bootstrap integer").clone();
    for index in 0..type_count {
        let mut entry = integer.clone();
        entry.name = format!("MetadataFixture{index}");
        universe.types.insert(entry.name.clone(), entry);
    }
    LlvmBackend::new()
        .with_type_universe(universe)
        .generate(&items, None)
}

fn assert_metadata_complete(output: &str) {
    let mut defined = std::collections::HashSet::new();
    for line in output.lines() {
        let Some((identifier, _)) = line.strip_prefix('!').and_then(|s| s.split_once(" = ")) else {
            continue;
        };
        let Ok(identifier) = identifier.parse::<usize>() else {
            continue;
        };
        assert!(
            defined.insert(identifier),
            "duplicate metadata !{identifier}"
        );
    }
    assert!(output.contains("StateAliasScope"));
    for reference in output.split('!').skip(1) {
        let digits: String = reference
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(identifier) = digits.parse::<usize>() {
            assert!(
                defined.contains(&identifier),
                "undefined metadata !{identifier}"
            );
        }
    }
}

#[test]
fn module_metadata_ids_remain_unique_with_large_type_universes() {
    for type_count in [0, 50, 100, 180] {
        assert_metadata_complete(&metadata_fixture_ir(type_count));
    }
}

#[test]
fn module_metadata_lifetime_exceeds_function_resets_and_snapshots() {
    let mut backend = LlvmBackend::new();
    backend.ctx.metadata_counter = 181;
    let first = emit_loop_metadata_nodes(
        &mut backend.ctx.metadata_counter,
        &mut backend.ctx.pending_metadata,
        false,
    );
    let snapshot = FunctionGuard::new(&backend.fun);
    backend.fun.reset();
    let second = emit_loop_metadata_nodes(
        &mut backend.ctx.metadata_counter,
        &mut backend.ctx.pending_metadata,
        true,
    );
    snapshot.restore_preserve_counters(&mut backend.fun);
    assert!(second > first);
    assert!(backend
        .ctx
        .pending_metadata
        .contains(&format!("!{first} =")));
    assert!(backend
        .ctx
        .pending_metadata
        .contains(&format!("!{second} =")));
}
