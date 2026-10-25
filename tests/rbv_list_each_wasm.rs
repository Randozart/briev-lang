// wasm32 generic collection + struct b-each regression (2026-10-23).
//
// `List<Int>` construction + b-each rendering on wasm32 is pinned here.
// Struct-element lists and multiple List<T> instantiations remain blocked on
// the monomorphized collection registry (obj_members/operator_defs are keyed
// by the base name "List" today); that limitation is documented in the plan,
// not hidden by an app workaround.
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

fn build_fixture(brievc: &str, stem: &str, out: &Path) {
    let fixture = Path::new(ROOT).join(format!("tests/fixtures/{stem}.rbv"));
    let build = Command::new(brievc)
        .args(["build", &fixture.to_string_lossy(), "--split", "--out", &out.to_string_lossy()])
        .output()
        .expect("spawn brievc");
    assert!(build.status.success(), "{stem} build failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let ll = out.join(format!("{stem}.ll"));
    let ir = std::fs::read_to_string(&ll).expect("ll missing");
    assert!(ir.contains("target triple = \"wasm32-"), "{stem} is not wasm32:\n{ir}");
    let obj = out.join(format!("{stem}.o"));
    let llc = Command::new("llc")
        .args(["-filetype=obj", "-relocation-model=pic", "-o", &obj.to_string_lossy(), &ll.to_string_lossy()])
        .output()
        .expect("spawn llc");
    assert!(llc.status.success(), "{stem}: llc rejected wasm IR:\n{}", String::from_utf8_lossy(&llc.stderr));
}

#[test]
fn per_instantiation_list_each_builds_for_wasm() {
    if !has("llc") {
        eprintln!("SKIP: llc not available");
        return;
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out = Path::new(ROOT).join("target/briev_list_each_wasm");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();

    build_fixture(brievc, "list_int_each_wasm", &out);
    let int_ir = std::fs::read_to_string(out.join("list_int_each_wasm.ll")).unwrap();
    assert!(int_ir.contains("define i32 @__view_items_numbers"), "int materializer missing:\n{int_ir}");

    build_fixture(brievc, "list_task_each_wasm", &out);
    let task_ir = std::fs::read_to_string(out.join("list_task_each_wasm.ll")).unwrap();
    assert!(task_ir.contains("define i32 @__view_items_tasks"), "struct materializer missing:\n{task_ir}");
    let shim = std::fs::read_to_string(out.join("list_task_each_wasm.mjs")).unwrap();
    assert!(shim.contains("item[\"title\"]"), "struct item projection missing:\n{shim}");
    assert!(shim.contains("itemKey(\"item.title\""), "struct key projection missing:\n{shim}");

    // Both instantiations in one unit must each resolve their own op scaffold.
    build_fixture(brievc, "list_multi_each_wasm", &out);
    let multi_ir = std::fs::read_to_string(out.join("list_multi_each_wasm.ll")).unwrap();
    assert!(multi_ir.contains("define i32 @__view_items_numbers"), "int materializer missing in multi:\n{multi_ir}");
    assert!(multi_ir.contains("define i32 @__view_items_tasks"), "task materializer missing in multi:\n{multi_ir}");
    let multi_html = std::fs::read_to_string(out.join("list_multi_each_wasm.html")).unwrap();
    assert_eq!(multi_html.matches("id=\"rbv-li-").count(), 2,
        "both b-each containers need stable ids:\n{multi_html}");

    // Browser JSON scalar widths (int/bool/finite float) through the real
    // WASM backend, not just an IR string check.
    build_fixture(brievc, "browser_json_values", &out);

    // JSON stringify exercises runtime `.^Length` on a collection parameter
    // on wasm32 (target-width handle, i64 len header).
    build_fixture(brievc, "json_stringify_wasm", &out);

    // List of boxed tuples: index 1 must use the canonical 8-byte word
    // stride and unbox the i32 handle, not treat Ptr<Tuple> as a 4-byte
    // scalar pointer.
    build_fixture(brievc, "list_tuple_index_wasm", &out);
    let tuple_ir = std::fs::read_to_string(out.join("list_tuple_index_wasm.ll")).unwrap();
    let body = tuple_ir.split("define i32 @read_second").nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("read_second definition");
    assert!(body.contains("getelementptr i64"), "tuple list index must use word stride:\n{body}");
    assert!(!body.contains("getelementptr i32"), "tuple list index must not use 4-byte stride:\n{body}");

    // Tuple return/destructure/field access on wasm32 (the float_fmt `sscale`
    // shape used by JSON parsing).
    build_fixture(brievc, "tuple_return_wasm", &out);
}
