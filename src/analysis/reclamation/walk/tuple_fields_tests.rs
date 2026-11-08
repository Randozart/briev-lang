//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::*;

fn plan(body: &str) -> ReclamationPlan {
    let source = format!(r#"
        defn inspect() -> Int {{
            let inner: Ptr = Malloc#(8);
            let carrying: Ptr = Malloc#(8);
            Store#(carrying,inner,8);
            let clean: Ptr = Malloc#(8);
            let pair: (Ptr,Ptr) = (clean,carrying);
            {body}
        }};
    "#);
    let tokens = crate::lexer::tokenize(&source).unwrap();
    let items = crate::parser::Parser::new(tokens, &source).parse_program().unwrap();
    crate::analysis::reclamation::analyze(&items, &Default::default(), None)
}

#[test]
fn separate_tuple_fields_do_not_publish_unselected_pointer_contents() {
    for body in [
        "term SysCall#(1,1,pair.0,8);",
        "let (safe,ignored): (Ptr,Ptr) = pair; term SysCall#(1,1,safe,8);",
        "term match pair { (safe,ignored) => SysCall#(1,1,safe,8), };",
        "let nested: ((Ptr,Ptr),Ptr) = (pair,clean); term match nested { ((safe,ignored),other) => SysCall#(1,1,safe,8), };",
    ] {
        let result = plan(body);
        assert!(result.regions.contains("inspect"), "{body}: {result:?}");
        assert!(!result.barriers.contains("inspect"), "{body}: {result:?}");
    }
}

#[test]
fn selected_or_raw_tuple_contents_still_escape() {
    for body in [
        "term SysCall#(1,1,pair.1,8);",
        "let (ignored,unsafe): (Ptr,Ptr) = pair; term SysCall#(1,1,unsafe,8);",
        "term match pair { (ignored,unsafe) => SysCall#(1,1,unsafe,8), };",
        "let nested: ((Ptr,Ptr),Ptr) = (pair,clean); term match nested { ((ignored,unsafe),other) => SysCall#(1,1,unsafe,8), };",
        "term SysCall#(1,1,Load#(pair,8),8);",
    ] {
        let result = plan(body);
        assert!(result.barriers.contains("inspect"), "{body}: {result:?}");
        assert!(!result.regions.contains("inspect"), "{body}: {result:?}");
    }
}
