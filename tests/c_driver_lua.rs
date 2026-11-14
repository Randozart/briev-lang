// ── Lua Round-Trip Test (native C module) ──────────────────────────────
// 2026-08-04 (plan 2026-08-04-ship-common-language-environments): `briev
// extension <bridge> lua` renders + builds a Lua C module (luaopen_<bridge>);
// strings cross via luaL_checkstring / lua_pushstring on the composite
// NUL-invariant data. Toolchain-guarded on a Lua built from source at
// ~/briv-tools/lua-*/src/lua (dlopen + -Wl,-E).

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

fn lua_bin() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tools = std::path::Path::new(&home).join("briv-tools");
    if let Ok(entries) = std::fs::read_dir(&tools) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.is_dir()
                && dir
                    .file_name()
                    .map_or(false, |n| n.to_string_lossy().starts_with("lua-"))
            {
                let bin = dir.join("src/lua");
                if bin.exists() {
                    return Some(bin);
                }
            }
        }
    }
    None
}

#[test]
fn lua_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let Some(lua) = lua_bin() else {
        eprintln!("SKIP: lua not found at ~/briv-tools/lua-*/src/lua");
        return;
    };
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let base = std::env::temp_dir().join("briev_lua_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let ext = Command::new(brievc)
        .args(["extension", &bv, "lua", "--out", &base.to_string_lossy()])
        .output()
        .expect("failed brievc extension lua");
    assert!(
        ext.status.success(),
        "lua ext failed: {}",
        String::from_utf8_lossy(&ext.stderr)
    );
    assert!(base.join("boundary.so").exists(), "boundary.so missing");

    let script = base.join("check.lua");
    std::fs::write(
        &script,
        r#"
package.cpath = package.cpath .. ';' .. arg[1] .. '/?.so'
local b = require('boundary')
assert(b.echo('hello') == 'hello', 'echo')
assert(b.greet('world') == 'world', 'greet')
assert(b.identity(3.5) == 3.5, 'identity')
assert(b.join('foo', 'bar') == 'foobar', 'join')
print('LUA OK')
"#,
    )
    .unwrap();

    let run = Command::new(&lua)
        .arg(&script)
        .arg(base.to_string_lossy().to_string())
        .output()
        .expect("failed lua");
    assert!(
        run.status.success(),
        "lua failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("LUA OK"));
}
