// LSP wiring smoke test — daily-driver plan Increment 4
// (docs/plans/2026-10-12-daily-driver-quick-wins.md).
//
// The VS Code client launches `<briev> lsp` over stdio with
// Content-Length framing (syntax-highlighter/client/extension.js). This
// test speaks the same protocol: spawn `brievc lsp`, send `initialize`,
// assert a capabilities response, then `shutdown` + `exit` for a clean
// teardown. It fails if the main.rs dispatch arm is ever removed.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn framed(msg: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).expect("serialize");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// Read one Content-Length-framed message from the child's stdout.
fn read_framed(out: &mut impl Read) -> Option<String> {
    // Headers end at the first \r\n\r\n.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match out.read(&mut byte) {
            Ok(0) => return None, // EOF before a complete header
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))?
        .trim()
        .parse()
        .ok()?;
    let mut body = vec![0u8; len];
    out.read_exact(&mut body).ok()?;
    Some(String::from_utf8_lossy(&body).into_owned())
}

#[test]
fn lsp_initialize_responds_with_capabilities() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let mut child = Command::new(brievc)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn brievc lsp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    // initialize — the first message of every LSP session.
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": null,
            "capabilities": {}
        }
    });
    stdin
        .write_all(&framed(&initialize))
        .expect("write initialize");

    // The response must carry server capabilities.
    let resp = read_framed(&mut stdout).expect("initialize response");
    let v: serde_json::Value =
        serde_json::from_str(&resp).expect("response is JSON");
    assert_eq!(v["id"], 1, "response id");
    assert!(
        v["result"]["capabilities"].is_object(),
        "capabilities object missing in: {resp}"
    );

    // `initialized` notification — the lsp-server state machine requires it
    // before shutdown (the spec's handshake: initialize → initialized → …).
    let initialized = serde_json::json!({
        "jsonrpc": "2.0", "method": "initialized", "params": {}
    });
    stdin
        .write_all(&framed(&initialized))
        .expect("write initialized");

    // Clean teardown: shutdown request (server returns Ok) + exit
    // notification (loop breaks, child exits).
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null
    });
    stdin.write_all(&framed(&shutdown)).expect("write shutdown");
    let exit = serde_json::json!({
        "jsonrpc": "2.0", "method": "exit", "params": null
    });
    stdin.write_all(&framed(&exit)).expect("write exit");
    drop(stdin);

    let status = child.wait().expect("wait for brievc lsp");
    assert!(status.success(), "lsp server exited with {status}");
}
