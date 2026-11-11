//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::*;

fn plan(source: &str) -> ReclamationPlan {
    let source = format!("import \"std/cast_lanes.bv\"; import \"std/collections.bv\";\n{source}");
    let (items, universe) =
        crate::library::parse_and_check("scratch/structured_lifetimes.bv", &source).unwrap();
    let live = crate::analysis::defn_liveness::DefnLiveness::build(&items);
    analyze(&items, &live.live, Some(&universe))
}

#[test]
fn enum_payloads_and_intrinsic_methods_license_local_consumers() {
    let plan = plan(
        r#"
        enum Payload { Values(List<Int>), Empty };
        defn inspect(text: String) -> Bool {
            let value: Payload = Values([1,2]);
            match value { Values(items) => items.Count#() == 2, Empty => false }
        };
        let done: Bool = false;
        node main [!done][done] { let valid: Bool = inspect("[1,2]"); done = true; endprogram; };
    "#,
    );
    assert!(plan.regions.contains("inspect"), "{plan:?}");
    assert!(!plan.barriers.contains("inspect"));
}

#[test]
fn escaping_slices_keep_backing_storage_alive() {
    let plan = plan(
        r#"
        obj Saved { text: String; };
        defn retain(saved: Saved, text: String) -> Int {
            let combined: String = text+"suffix";
            saved.text = combined[1:3];
            term 1;
        };
        let done: Bool = false;
        node main [!done][done] { let saved: Saved = Saved { text: "" }; retain(saved,"prefix"); done = true; endprogram; };
    "#,
    );
    assert!(!plan.regions.contains("retain"), "{plan:?}");
}

#[test]
fn slices_and_indexed_casts_are_reclaimed_by_scalar_consumers() {
    let plan = plan(
        r#"
        defn inspect(text: String) -> Int {
            let slice: String = text[1:3];
            let character: Char = slice[0] as Char;
            term character as Int;
        };
        let done: Bool = false;
        node main [!done][done] { let value: Int = inspect("abcd"); done = true; endprogram; };
    "#,
    );
    assert!(plan.regions.contains("inspect"), "{plan:?}");
    assert!(!plan.barriers.contains("inspect"));
}

#[test]
fn scalar_constant_bytes_do_not_alias_constant_storage() {
    let plan = plan(
        r#"
        const FLAG: Int = 4;
        defn inspect() -> Int {
            let memory: Ptr = Malloc#(16);
            let other: Ptr = Malloc#(32);
            Store#(memory,FLAG,8);
            let value: Int = Load#(memory,8);
            Store#(other+value,memory,8);
            term 0;
        };
        let done: Bool = false;
        node main [!done][done] { let value: Int = inspect(); done = true; endprogram; };
    "#,
    );
    assert!(!plan.barriers.contains("inspect"), "{plan:?}");
    assert!(plan.regions.contains("inspect"), "{plan:?}");
}

#[test]
fn single_bindings_do_not_alias_their_collection_elements() {
    let plan = plan(
        r#"
        defn inspect() -> Int {
            let values: List<Ptr> = [];
            let pointer: Ptr = Malloc#(8);
            Store#(pointer,42,8);
            values <- pointer;
            term SysCall#(1,1,values[0],8);
        };
        let done: Bool = false;
        node main [!done][done] { inspect(); done = true; endprogram; };
    "#,
    );
    assert!(!plan.barriers.contains("inspect"), "{plan:?}");
    assert!(plan.regions.contains("inspect"), "{plan:?}");
}

#[test]
fn indexed_user_operations_remain_conservative() {
    let plan = plan(
        r#"
        defn inspect(value: Stack<Int,8>) -> Int { term value[0]; };
    "#,
    );
    assert!(plan.barriers.contains("inspect"), "{plan:?}");
    assert!(!plan.regions.contains("inspect"));
}
