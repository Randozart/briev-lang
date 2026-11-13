// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// unbounded_parse.rs — the `unbounded` waiver and its optional `until`.
//
// SPEC 10.9: `unbounded` declares that a loop carries **no termination
// obligation**. It is a waiver, not an assertion — the loop may end, and showing
// that it does is simply not required — so the parser only records the intent and
// leaves the body untouched.
//
// These go through `parse_and_check`, so they also pin that the new modifier
// prefix does not confuse the typechecker.

use briev_compiler::ast::TopLevel;
use briev_compiler::library::parse_and_check;

/// Parse and return the single `node` declaration, ignoring state `let`s.
fn node_item(src: &str) -> briev_compiler::ast::Transaction {
    let (items, _universe) = parse_and_check("<test>", src).expect("parse_and_check");
    items
        .into_iter()
        .find_map(|item| match item {
            TopLevel::Transaction(txn) => Some(txn),
            _ => None,
        })
        .expect("a node declaration")
}

/// `unbounded node` records the waiver on the transaction.
#[test]
fn unbounded_node_records_the_waiver() {
    let txn = node_item(
        "let done: Bool = false;\nunbounded node server [true][done] { done = true; term; };",
    );
    assert!(
        txn.modifiers.iter().any(|m| m.name == "unbounded"),
        "the waiver must be recorded: {:?}",
        txn.modifiers.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
}

/// `unbounded until [C] node` records the declared exit condition with it.
#[test]
fn unbounded_until_records_the_exit_condition() {
    let txn = node_item(
        "let done: Bool = false;\nlet shutdown: Bool = false;\n\
         unbounded until [shutdown] node server [true][done] { done = true; term; };",
    );
    let until = txn
        .modifiers
        .iter()
        .find(|m| m.name == "until")
        .expect("the exit condition must be recorded");
    assert!(
        until.value.is_some(),
        "`until` must carry its condition, not just its name"
    );
}

/// A plain node records neither: the waiver is opt-in.
#[test]
fn a_plain_node_carries_no_waiver() {
    let txn = node_item("let done: Bool = false;\nnode poll [true][done] { done = true; term; };");
    assert!(
        !txn.modifiers.iter().any(|m| m.name == "unbounded"),
        "unbounded must never be inferred"
    );
}
