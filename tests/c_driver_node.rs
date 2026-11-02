// ── Python ↔ Node Cross-Language Bridge Test ────────────────────────────
// 2026-08-03 (plan 2026-08-03-glue-folders-node-bridge): Python and Node have
// no mature native binding between them. Briv's composite (the CStr <-> String
// meld, NUL-invariant String, zero-copy str_to_c) is their only common
// interface. Both call the same bridge exports; the composite String crosses
// the process boundary via the runtime's file I/O (persist/load) — the only
// transport both languages share is Briv itself.
//
// Flow: Node saves+persists → Python loads (a Node-originated value consumed
// by Python) → Python saves+persists → Node loads (the reverse). Numeric bump
// round-trips in-process on both sides. Toolchain-guarded.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn python_node_cross_language_bridge() {
    for tool in ["cc", "ar", "llc", "clang", "python3-config", "python3", "node"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let base = std::env::temp_dir().join("briev_py_node_bridge");
    let _ = std::fs::remove_dir_all(&base);
    let node_dir = base.join("node");
    let py_dir = base.join("py");
    std::fs::create_dir_all(&node_dir).unwrap();
    std::fs::create_dir_all(&py_dir).unwrap();

    let bv = format!("{}/examples/glue-host/node_bridge.bv", PROJECT_ROOT);

    let ext_node = Command::new(brievc)
        .args(["extension", &bv, "node", "--out", &node_dir.to_string_lossy()])
        .output().expect("failed brievc extension node");
    assert!(ext_node.status.success(), "node ext failed: {}", String::from_utf8_lossy(&ext_node.stderr));

    let ext_py = Command::new(brievc)
        .args(["extension", &bv, "python", "--out", &py_dir.to_string_lossy()])
        .output().expect("failed brievc extension python");
    assert!(ext_py.status.success(), "python ext failed: {}", String::from_utf8_lossy(&ext_py.stderr));

    // Step 1: Node saves + persists "hello from node".
    let node_x = node_dir.join("x.dat");
    let node_script = node_dir.join("step1.cjs");
    std::fs::write(&node_script, format!(
        "const b = require('{}');\nconst fs = require('fs');\n\
         const v = b.save('hello from node');\n\
         if (v !== 'hello from node') throw new Error('node save: ' + v);\n\
         const p = b.persist('{}');\n\
         if (p !== 'hello from node') throw new Error('node persist: ' + p);\n\
         const f = fs.readFileSync('{}', 'utf8');\n\
         if (f !== 'hello from node') throw new Error('file content: ' + f);\n\
         const c = b.bump(5);\n\
         if (c !== 5) throw new Error('node bump: ' + c);\n\
         console.log('NODE OK');\n",
        node_dir.join("node_bridge.node").to_string_lossy(),
        node_x.to_string_lossy(),
        node_x.to_string_lossy(),
    )).unwrap();
    let s1 = Command::new("node").current_dir(&node_dir).arg(&node_script)
        .output().expect("failed node step1");
    assert!(s1.status.success(), "node step1: {}", String::from_utf8_lossy(&s1.stderr));
    assert!(String::from_utf8_lossy(&s1.stdout).contains("NODE OK"));

    // Step 2: Python loads the Node-originated value, then saves+persists its own.
    let py_y = py_dir.join("y.dat");
    let py_script = py_dir.join("step2.py");
    std::fs::write(&py_script, format!(
        "import node_bridge as b\n\
         v = b.load('{}')\n\
         assert v == 'hello from node', 'python load: ' + v\n\
         s = b.save('hello from python')\n\
         assert s == 'hello from python'\n\
         p = b.persist('{}')\n\
         assert p == 'hello from python'\n\
         c = b.bump(3)\n\
         assert c == 3\n\
         print('PYTHON OK')\n",
        node_x.to_string_lossy(),
        py_y.to_string_lossy(),
    )).unwrap();
    let s2 = Command::new("python3").current_dir(&py_dir).arg(&py_script)
        .output().expect("failed python step2");
    assert!(s2.status.success(), "python step2: {}", String::from_utf8_lossy(&s2.stderr));
    assert!(String::from_utf8_lossy(&s2.stdout).contains("PYTHON OK"));

    // Step 3: Node loads the Python-originated value.
    let node_script2 = node_dir.join("step3.cjs");
    std::fs::write(&node_script2, format!(
        "const b = require('{}');\n\
         const v = b.load('{}');\n\
         if (v !== 'hello from python') throw new Error('node load: ' + v);\n\
         console.log('NODE2 OK');\n",
        node_dir.join("node_bridge.node").to_string_lossy(),
        py_y.to_string_lossy(),
    )).unwrap();
    let s3 = Command::new("node").current_dir(&node_dir).arg(&node_script2)
        .output().expect("failed node step3");
    assert!(s3.status.success(), "node step3: {}", String::from_utf8_lossy(&s3.stderr));
    assert!(String::from_utf8_lossy(&s3.stdout).contains("NODE2 OK"));
}
