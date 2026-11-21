// obj_linear_tests — the linearity gate's diagnostic matrix (evented-net
// plan Wave 2, invariant 4). The negative fixtures are UNIT tests, not
// tier1 corpus entries: the corpus runs every .bv it finds, and a negative
// fixture must FAIL compilation — running it would break the harness. Each
// test asserts the exact diagnostic text (what/why/fix discipline).

use crate::analysis::obj_linear::analyze;
use crate::ast::TopLevel;
use crate::lexer::tokenize;
use crate::parser::Parser;

fn parse_program(source: &str) -> Vec<TopLevel> {
    let tokens = tokenize(source).expect("fixture must lex");
    let mut p = Parser::new(tokens, source);
    p.parse_program().expect("fixture must parse")
}

fn errors_of(src: &str) -> Vec<String> {
    analyze(&parse_program(src))
}

const THING: &str = "obj Thing { handle: Int; attr linear: true; };";

#[test]
fn leak_at_scope_end_is_an_error() {
    let src = format!(
        "{THING}\nnode entry [beginprogram][true] {{\n\
         let t: Thing = Thing {{ handle: 5 }};\n\
         Print#(t.handle);\n\
         endprogram Print#(\"\");\n}};"
    );
    let errs = errors_of(&src);
    assert!(
        errs.iter().any(|e| e.contains("still live") && e.contains("'t'")),
        "expected a leak diagnostic for 't', got {errs:?}"
    );
}

#[test]
fn double_consume_is_an_error() {
    let src = format!(
        "{THING}\ndefn thing_close(t: Thing) -> Int {{ term t.handle; }};\n\
         node entry [beginprogram][true] {{\n\
         let t: Thing = Thing {{ handle: 5 }};\n\
         let a: Int = thing_close(t);\n\
         let b: Int = thing_close(t);\n\
         Print#(a + b);\n\
         endprogram Print#(\"\");\n}};"
    );
    let errs = errors_of(&src);
    assert!(
        errs.iter()
            .any(|e| e.contains("used after it was consumed")),
        "expected a use-after-consume diagnostic, got {errs:?}"
    );
}

#[test]
fn use_after_move_is_an_error() {
    let src = format!(
        "{THING}\ndefn thing_close(t: Thing) -> Int {{ term t.handle; }};\n\
         node entry [beginprogram][true] {{\n\
         let t: Thing = Thing {{ handle: 5 }};\n\
         let u: Thing = t;\n\
         let a: Int = thing_close(t);\n\
         Print#(a);\n\
         endprogram Print#(\"\");\n}};"
    );
    let errs = errors_of(&src);
    assert!(
        errs.iter().any(|e| e.contains("'u'") && e.contains("still live")),
        "expected a leak diagnostic for 'u' (the moved-in handle), got {errs:?}"
    );
}

#[test]
fn handover_then_close_is_clean() {
    // The task_board shape: handover in one guard, close in the sibling —
    // path-scoped discharge (v1 allows the hole; never a false positive).
    let src = format!(
        "{THING}\ndefn use_it(t: Thing) -> Int {{ term t.handle; }};\n\
         defn closer(t: Thing) -> Int {{ term t.handle; }};\n\
         node entry [beginprogram][true] {{\n\
         let t: Thing = Thing {{ handle: 5 }};\n\
         when t.handle >= 0 {{\n\
         let a: Int = use_it(t);\n\
         Print#(a);\n}};\n\
         when t.handle < 0 {{\n\
         let b: Int = closer(t);\n\
         Print#(b);\n}};\n\
         endprogram Print#(\"\");\n}};"
    );
    let errs = errors_of(&src);
    assert!(
        errs.is_empty(),
        "the when-chain shape must stay clean (path-scoped discharge), got {errs:?}"
    );
}

#[test]
fn happy_consume_passes() {
    let src = format!(
        "{THING}\ndefn thing_close(t: Thing) -> Int {{ term t.handle; }};\n\
         node entry [beginprogram][true] {{\n\
         let t: Thing = Thing {{ handle: 5 }};\n\
         let a: Int = thing_close(t);\n\
         Print#(a);\n\
         endprogram Print#(\"\");\n}};"
    );
    assert!(errors_of(&src).is_empty(), "happy consume must be clean");
}

#[test]
fn non_linear_objs_are_untracked() {
    let src = "obj Plain { handle: Int; };\n\
               node entry [beginprogram][true] {\n\
               let a: Plain = Plain { handle: 1 };\n\
               let b: Plain = a;\n\
               let c: Plain = a;\n\
               Print#(b.handle + c.handle);\n\
               endprogram Print#(\"\");\n};";
    assert!(
        errors_of(src).is_empty(),
        "objs without attr linear must stay untracked (copy semantics)"
    );
}

#[test]
fn socket_double_close_in_body_is_an_error() {
    // Self-contained: the unit test parses only (no import resolution), so
    // the closer is spelled locally — the discipline shape is identical to
    // the real sock_close case (which the brievc-check negative fixture
    // pins end-to-end).
    let src = "obj FakeSock { fd: Int; attr linear: true; };\n\
               defn sock_close(s: FakeSock) -> Int { term s.fd; };\n\
               defn demo_double_close() -> Int {\n\
               let s: FakeSock = FakeSock { fd: 3 };\n\
               when s.fd >= 0 {\n\
               let first: Int = sock_close(s);\n\
               let second: Int = sock_close(s);\n\
               term first + second;\n};\n\
               term 0 - 1;\n};";
    let errs = errors_of(src);
    assert!(
        errs.iter()
            .any(|e| e.contains("used after it was consumed") && e.contains("sock_close")),
        "expected the double-close diagnostic, got {errs:?}"
    );
}
