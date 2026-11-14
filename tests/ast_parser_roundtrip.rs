// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// ast_parser_roundtrip.rs — restored from src/ast/{canonical,visit_mut}.rs when
// the AST moved into the `briev-verify` crate.
//
// Those tests parse Briev source through the real frontend, which that crate
// cannot do — it has no parser by design. They live here, where the parser is,
// so the coverage survived the carve-out instead of being dropped with the files.
//
// Recovered verbatim from 5abfcea43^ with only the import paths adapted.

mod canonical_roundtrip {
    use briev_compiler::ast::*;
    use briev_compiler::lexer::tokenize;
    use briev_compiler::parser::Parser;

    /// 2026-08-05 (Phase 2): parse a Briev source string into top-level items.
    fn parse(source: &str) -> Result<Vec<TopLevel>, String> {
        let tokens = tokenize(source).map_err(|e| format!("lex: {}", e))?;
        let mut parser = Parser::new(tokens, source);
        parser.parse_program().map_err(|e| format!("parse: {}", e))
    }

    /// 2026-08-05 (Phase 2): the round-trip contract.
    /// `format(parse(format(parse(source)))) == format(parse(source))`.
    fn assert_idempotent(source: &str) {
        let items = parse(source).unwrap_or_else(|e| panic!("first parse failed: {e}\n{source}"));
        let first = format_program(&items);
        let reparsed =
            parse(&first).unwrap_or_else(|e| panic!("reparse failed: {e}\n--- output:\n{first}"));
        let second = format_program(&reparsed);
        assert_eq!(
            first, second,
            "formatter is not idempotent for:\n{source}\n--- output:\n{first}\n--- reformat:\n{second}"
        );
    }

    const FIXTURES: &[&str] = &[
        "import <std/collections>;\n",
        "import \"std/string\";\n",
        "import { Map, Set } from \"std/collections\";\n",
        "const Max: Int = 10;\n",
        "defn add(a: Int, b: Int) -> Int [a >= 0][b >= 0] {\n  term a + b;\n};\n",
        "defn empty() [true][true] {};\n",
        "txn increment()[count < Max][count == count] {\n  count = count + 1;\n  term;\n};\n",
        "node update [ready][!ready] {\n  ready = false;\n  term;\n};\n",
        "async node tick [pending][!pending] {\n  pending = false;\n  term;\n};\n",
        "struct Point {\n  x: Float;\n  y: Float;\n};\n",
        "type Meters: Int {\n  value: Int;\n};\n",
        "trg input_ready @ device;\n",
        "$const Limit = 32;\n",
        "$let current = 0;\n",
        "defn guarded(x: Int) -> Int [true][x >= 0] {\n  when x > 0 {\n    term x;\n  };\n  term 0;\n};\n",
        "defn looped(items: List<Int>) -> Int [true][true] {\n  let acc: Int = 0;\n  foreach(item in items) {\n    acc = acc + item;\n  };\n  term acc;\n};\n",
        "defn lifetime(x: Int) -> Int [true][true] {\n  let buf: Ptr<Int> = Malloc#(4);\n  keep buf;\n  term x;\n};\n",
        "defn watchdog(a: Int) -> Int [a >= 0][a >= 0] ?[progress] within 10ms {\n  term a;\n};\n",
        // 2026-11-08 (attr plan): module-level `attr` metadata round-trips.
        "attr notes: \"alpha\";\n",
        "attr a: 1;\nattr b: 2;\n",
        "attr flags: [fast, contract];\n",
        // 2026-08-13 (layout-keywords plan): physical-layout metadata round-trips.
        "type W8: Int {\n  spec Bits: 8;\n};\n",
        "struct Flags {\n  spec Bytes: 1;\n  spec Alignment: 1;\n  a: Bool;\n};\n",
        "type Frame: Bit {\n  spec Alignment: 2;\n  spec Bits: 12;\n  spec MaxBits: 16;\n  spec Bytes: 4;\n  spec Endian: Big;\n};\n",
        // 2026-08-13 (layout-keywords plan): `pack struct` round-trips with the
        // prefix preserved, alongside its spec metadata.
        "pack struct Eth {\n  spec Endian: Big;\n  dst: Bit<48>;\n  src: Bit<48>;\n  etype: Bit<16>;\n};\n",
        "seq pack struct Mix {\n  spec Alignment: 1;\n  a: Bit<12>;\n  b: Bit<4>;\n};\n",
        // 2026-08-13 (layout-keywords plan Phase 5): the `atomic` field
        // modifier round-trips through the prefix (not `attr atomic_fields`).
        "struct Counter {\n  atomic count: Int;\n  other: Int;\n};\n",
        // 2026-08-13 (layout-keywords plan Phase 6): `union` round-trips.
        "union Word {\n  i: Int;\n  f: Float;\n};\n",
        // 2026-08-09 (init kind): runtime-seeded invariant round-trips.
        "init BufSize: Int = get_env_int!(\"BUFSIZE\");\n",
        "init BufferSize: [64 | lo..hi] Int = 64;\n",
        "init BitLayout: [16 | 32 | 64] Int = 16;\n",
        "init Layout: [16 | 32 | 64] Int {\n  term pick(target);\n};\n",
    ];

    #[test]
    fn formatter_is_idempotent_on_canonical_fixtures() {
        for fixture in FIXTURES {
            assert_idempotent(fixture);
        }
    }

    #[test]
    fn formatter_handles_blocky_statements() {
        assert_idempotent(
            "defn nested(x: Int) -> Int [true][true] {\n  when x > 0 {\n    term x;\n  };\n  term 0;\n};\n",
        );
    }

    /// 2026-11-08 (attr plan): a type body containing mixed `spec` and `attr`
    /// metadata prints both forms and round-trips.
    #[test]
    fn formatter_preserves_spec_and_attr_metadata() {
        let src = "type W: Int {\n  attr ctd: Add;\n  spec Bits: 8;\n  spec Endian: Little;\n};\n";
        let items = parse(src).expect("parse");
        let out = format_program(&items);
        assert!(out.contains("spec Bits: 8;"), "output: {out}");
        assert!(out.contains("spec Endian: Little;"), "output: {out}");
        assert!(out.contains("attr ctd: Add;"), "output: {out}");
        assert_idempotent(src);
    }
}


mod visit_mut_roundtrip {
    use briev_compiler::ast::{BinaryOpKind, Expr};
    use briev_compiler::lexer::tokenize;
    use briev_compiler::parser::Parser;

    #[test]
    fn mutation_is_postorder_and_exactly_once() {
        let mut expression = Expr::BinaryOp(BinaryOpKind::Add,
            Box::new(Expr::Decimal(1)), Box::new(Expr::Decimal(2)));
        let mut order = Vec::new();
        briev_compiler::ast::visit_mut::walk_expr(&mut expression, &mut |expression| match expression {
            Expr::Decimal(value) => { order.push(*value); *value += 10; },
            Expr::BinaryOp(_, left, right) => {
                assert_eq!(**left, Expr::Decimal(11));
                assert_eq!(**right, Expr::Decimal(12));
                order.push(3);
            },
            _ => {}
        });
        assert_eq!(order, [1, 2, 3]);
    }

    #[test]
    fn mutable_and_readonly_walkers_cover_the_same_payloads() {
        let source = r#"
            defn walk(value: Int) [value >= 0][term >= 0] -> Int {
                foreach item in 0..2 { when item > 0 { value = value+item; }; };
                match value { 0 => { value = 1; }, _ => { value = 2; }, };
                let nested: Int = { let local: Int = 3; local; };
                term nested;
            };
        "#;
        let tokens = tokenize(source).expect("visitor tokens");
        let mut program = Parser::new(tokens, source).parse_program().expect("visitor syntax");
        let original = program.clone();
        let mut before = std::collections::HashMap::new();
        let mut after = std::collections::HashMap::new();
        for item in &program {
            briev_compiler::ast::visit::walk_toplevel(item, &mut |expression| {
                *before.entry(std::mem::discriminant(expression)).or_insert(0) += 1;
            });
        }
        for item in &mut program {
            briev_compiler::ast::visit_mut::walk_toplevel(item, &mut |expression| {
                *after.entry(std::mem::discriminant(expression)).or_insert(0) += 1;
            });
        }
        assert_eq!(before, after);
        assert_eq!(format!("{program:?}"), format!("{original:?}"));
    }
}
