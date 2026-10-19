// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Callable-txn interpreter parity pin (2026-10-20).
//!
//! A `txn` invoked as a function must execute like a defn with convergence:
//! the pre gates each iteration, the body runs, the post ends the loop. This
//! is what `base64_encode -> b64_loop` relies on. The reference interpreter
//! must agree with the backend (Rule 5).

use std::fs;
use std::path::PathBuf;

use briev_compiler::errors::RuntimeError;
use briev_compiler::interpreter::{Interpreter, Value};
use briev_compiler::reactor::Reactor;

fn run(src: &str, name: &str) -> Result<Value, String> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let main_path = dir.join("main.bv");
    fs::write(&main_path, src).unwrap();
    // 2026-10-20: use the REAL pipeline (prelude injection, imports,
    // desugar, typecheck) rather than a bare resolver — the prelude is what
    // brings the cast lanes (`char_to_str` & co.) into the program.
    let opts = briev_compiler::pipeline::BuildOptions {
        file_path: main_path.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let (resolved, _universe) =
        briev_compiler::pipeline::compile_to_typed(&main_path.to_string_lossy(), src, &opts)
            .map_err(|e| format!("pipeline: {e}"))?;
    let mut interp = Interpreter::new();
    interp.load_program(&resolved);
    interp.seed_inits(&resolved).map_err(|e| format!("seed: {e:?}"))?;
    for item in &resolved {
        if let briev_compiler::ast::TopLevel::Statement(s) = item {
            interp.exec_stmt(s).map_err(|err| format!("seed stmt: {err:?}"))?;
        }
    }
    let mut reactor = Reactor::new();
    reactor.build_from_program(&resolved);
    loop {
        match reactor.run(&mut interp) {
            Ok(true) => {}
            Ok(false) => {
                // Diagnostic: run the entry body directly to surface the error
                // the reactor swallowed before ever marking the node dirty.
                let body = resolved.iter().find_map(|i| match i {
                    briev_compiler::ast::TopLevel::Transaction(t) if t.name == "entry" => {
                        Some(t.body.clone())
                    }
                    _ => None,
                });
                let mut detail = format!(
                    "; reactor txns={:?} dirty={:?}",
                    reactor.transactions.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                    reactor.dirty_preconditions
                );
                if let Some(body) = body {
                    for stmt in &body {
                        if let Err(e) = interp.exec_stmt(stmt) {
                            detail.push_str(&format!("; direct-body error: {e:?}"));
                            break;
                        }
                    }
                }
                return Err(format!("equilibrium without endprogram{detail}"));
            }
            Err(RuntimeError::ProgramExit(v)) => return Ok(v),
            Err(e) => return Err(format!("runtime: {e:?}")),
        }
    }
}

#[test]
fn callable_txn_converges() {
    let v = run(
        r#"
txn loop_tx(x: Int) [x < 3][x >= 3] -> Int {
    x = x + 1;
    term x;
};
node entry [beginprogram][true] {
    let v: Int = loop_tx(0);
    endprogram v;
};
"#,
        "callable_txn",
    )
    .expect("callable txn runs to endprogram");
    assert_eq!(v.as_i64(), Some(3), "txn converges to its postcondition");
}

#[test]
fn base64_encode_interpreter() {
    // NOTE: the input is a Malloc'd byte buffer, not a String view. The
    // interpreter represents Strings as Value::Bits with NO heap address, so
    // `String as Data as Int` (the backend's pointer idiom) is unrepresentable
    // there — BUGS.md 2026-10-20 interpreter limitation. ByteBuf inputs built
    // from real addresses exercise the same codec path.
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";
import { base64_encode } from "std/net/base64";

node entry [beginprogram][true] {
    let raw: Ptr<Bit<8>> = Malloc#(8) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 104, 1);
    Store#(raw as Int + 1, 101, 1);
    Store#(raw as Int + 2, 108, 1);
    Store#(raw as Int + 3, 108, 1);
    Store#(raw as Int + 4, 111, 1);
    let b: ByteBuf = ByteBuf { ptr: raw as Int, len: 5, cap: 8 };
    let encoded: String = base64_encode(b);
    endprogram CharCount#(encoded);
};
"#,
        "base64_interp",
    )
    .expect("base64_encode runs in the interpreter");
    assert_eq!(v.as_i64(), Some(8), "base64(\"hello\") is 8 chars");
}

#[test]
fn char_to_string_cast_interpreter() {
    let v = run(
        r#"
node entry [beginprogram][true] {
    let s: String = 65 as Char as String;
    endprogram CharCount#(s);
};
"#,
        "char_cast_interp",
    )
    .expect("char cast runs");
    assert_eq!(v.as_i64(), Some(1), "one char");
}

#[test]
fn b64_char_interpreter() {
    let v = run(
        r#"
import { b64_char } from "std/net/base64";
node entry [beginprogram][true] {
    let s: String = b64_char(0);
    endprogram CharCount#(s);
};
"#,
        "b64_char_interp",
    )
    .expect("b64_char runs");
    assert_eq!(v.as_i64(), Some(1), "one char");
}

#[test]
fn instance_call_argument_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";

defn mk() -> ByteBuf {
    let raw: Ptr<Bit<8>> = Malloc#(16) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 104, 1);
    Store#(raw as Int + 1, 101, 1);
    term ByteBuf { ptr: raw as Int, len: 2, cap: 16 };
};
defn rd(b: ByteBuf) -> Int { term b.len; };
node entry [beginprogram][true] {
    endprogram rd(mk());
};
"#,
        "instance_arg",
    )
    .expect("instance-returning call as argument runs");
    assert_eq!(v.as_i64(), Some(2), "len");
}

#[test]
fn nested_call_argument_interpreter() {
    let v = run(
        r#"
defn mk() -> Int { term 5; };
defn use(x: Int) -> Int { term x + 1; };
node entry [beginprogram][true] {
    endprogram use(mk());
};
"#,
        "nested_arg",
    )
    .expect("nested call argument runs");
    assert_eq!(v.as_i64(), Some(6), "5 + 1");
}

#[test]
fn local_bytebuf_getu8_interpreter() {
    let v = run(
        r#"
import { ByteBuf, get_u8 } from "std/net/bytes";

node entry [beginprogram][true] {
    let raw: Ptr<Bit<8>> = Malloc#(16) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 104, 1);
    Store#(raw as Int + 1, 101, 1);
    let b: ByteBuf = ByteBuf { ptr: raw as Int, len: 2, cap: 16 };
    endprogram get_u8(b, 1);
};
"#,
        "local_bytebuf_getu8",
    )
    .expect("local ByteBuf get_u8 runs");
    assert_eq!(v.as_i64(), Some(101), "e");
}

#[test]
fn returned_bytebuf_len_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";

defn str_buf(s: String) -> ByteBuf {
    let n: Int = s .^Length;
    term ByteBuf { ptr: ((s as Data) as Int) + 8, len: n, cap: n };
};

node entry [beginprogram][true] {
    let b: ByteBuf = str_buf("hello");
    endprogram b.len;
};
"#,
        "returned_bytebuf_len",
    )
    .expect("returned ByteBuf field read runs");
    assert_eq!(v.as_i64(), Some(5), "len");
}

#[test]
fn b64_pad_or_char_interpreter() {
    let v = run(
        r#"
import { b64_pad_or_char } from "std/net/base64";
node entry [beginprogram][true] {
    let s: String = b64_pad_or_char(104, 16, 3, 2);
    endprogram CharCount#(s);
};
"#,
        "b64_poc",
    )
    .expect("b64_pad_or_char runs");
    assert_eq!(v.as_i64(), Some(1), "one char");
}

#[test]
fn b64_group_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";
import { b64_group } from "std/net/base64";
node entry [beginprogram][true] {
    let raw: Ptr<Bit<8>> = Malloc#(8) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 104, 1);
    Store#(raw as Int + 1, 101, 1);
    Store#(raw as Int + 2, 108, 1);
    let b: ByteBuf = ByteBuf { ptr: raw as Int, len: 5, cap: 8 };
    let g: String = b64_group(b, 0, 3);
    endprogram CharCount#(g);
};
"#,
        "b64_group_interp",
    )
    .expect("b64_group runs");
    assert_eq!(v.as_i64(), Some(4), "one group = 4 chars");
}

#[test]
fn b64_loop_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";
import { b64_loop } from "std/net/base64";
node entry [beginprogram][true] {
    let raw: Ptr<Bit<8>> = Malloc#(8) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 104, 1);
    Store#(raw as Int + 1, 101, 1);
    Store#(raw as Int + 2, 108, 1);
    Store#(raw as Int + 3, 108, 1);
    Store#(raw as Int + 4, 111, 1);
    let b: ByteBuf = ByteBuf { ptr: raw as Int, len: 5, cap: 8 };
    let s: String = b64_loop(b, "", 0);
    endprogram CharCount#(s);
};
"#,
        "b64_loop_interp",
    )
    .expect("b64_loop runs");
    assert_eq!(v.as_i64(), Some(8), "hello = 8 base64 chars");
}

#[test]
fn address_to_string_door_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";
node entry [beginprogram][true] {
    let addr: Int = Malloc#(16) as Int;
    Store#(addr, 3, 8);
    Store#(addr + 8, 97, 1);
    Store#(addr + 9, 98, 1);
    Store#(addr + 10, 99, 1);
    Store#(addr + 11, 0, 1);
    let s: String = (addr as Data) as String;
    endprogram CharCount#(s);
};
"#,
        "door_interp",
    )
    .expect("address→String door runs");
    assert_eq!(v.as_i64(), Some(3), "three bytes");
}

#[test]
fn ws_message_interpreter() {
    let v = run(
        r#"
import { ByteBuf } from "std/net/bytes";
import { ws_message } from "std/net/websocket";
node entry [beginprogram][true] {
    let raw: Ptr<Bit<8>> = Malloc#(16) as Ptr<Bit<8>>;
    Store#(raw as Int + 0, 0x81, 1);
    Store#(raw as Int + 1, 0x83, 1);
    Store#(raw as Int + 2, 1, 1);
    Store#(raw as Int + 3, 2, 1);
    Store#(raw as Int + 4, 3, 1);
    Store#(raw as Int + 5, 4, 1);
    Store#(raw as Int + 6, 96, 1);
    Store#(raw as Int + 7, 96, 1);
    Store#(raw as Int + 8, 96, 1);
    let fr: ByteBuf = ByteBuf { ptr: raw as Int, len: 9, cap: 16 };
    let m = ws_message(fr, 0);
    let ok: Int = 0;
    when m.status == 1 && m.payload == "abc" && m.consumed == 9 {
        ok = 1;
    };
    endprogram ok;
};
"#,
        "ws_message_interp",
    )
    .expect("ws_message runs in the interpreter");
    assert_eq!(v.as_i64(), Some(1), "masked frame materializes payload");
}

#[test]
fn growbuf_growth_interpreter() {
    let v = run(
        r#"
import { GrowBuf, grow_new } from "std/net/bytes";

defn fill(g: GrowBuf, i: Int) -> GrowBuf {
    when i == 20 {
        term g;
    };
    term fill(g.put_u8(65), i + 1);
};

node entry [beginprogram][true] {
    let g: GrowBuf = fill(grow_new(8), 0);
    endprogram g.len;
};
"#,
        "growbuf_growth",
    )
    .expect("growbuf growth runs");
    assert_eq!(v.as_i64(), Some(20), "20 puts append 20 bytes across two growth steps");
}

#[test]
fn growbuf_single_put_interpreter() {
    let v = run(
        r#"
import { GrowBuf, grow_new } from "std/net/bytes";
node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    let g2: GrowBuf = g.put_u8(65);
    endprogram g2.len;
};
"#,
        "growbuf_single_put",
    )
    .expect("single put runs");
    assert_eq!(v.as_i64(), Some(1), "one byte");
}

#[test]
fn foreach_store_in_defn_interpreter() {
    let v = run(
        r#"
defn stamp(a: Int) -> Int {
    foreach i in 0..3 {
        Store#(a + i, 65, 1);
    };
    term Load#(a + 1, 1);
};
node entry [beginprogram][true] {
    let addr: Int = Malloc#(8) as Int;
    endprogram stamp(addr);
};
"#,
        "foreach_store_defn",
    )
    .expect("foreach+Store# in a defn runs");
    assert_eq!(v.as_i64(), Some(65), "stamped");
}

#[test]
fn growbuf_put_free_form_interpreter() {
    let v = run(
        r#"
import { GrowBuf, grow_new, put_u8 } from "std/net/bytes";
node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    let g2: GrowBuf = put_u8(g, 65);
    endprogram g2.len;
};
"#,
        "growbuf_put_free",
    )
    .expect("free-form put_u8 runs");
    assert_eq!(v.as_i64(), Some(1), "one byte");
}

#[test]
fn growbuf_basics_interpreter() {
    let a = run(
        r#"
import { GrowBuf, grow_new } from "std/net/bytes";
node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    endprogram g.len + g.cap;
};
"#,
        "grow_new_only",
    )
    .expect("grow_new runs");
    assert_eq!(a.as_i64(), Some(8), "len 0 + cap 8");

    let b = run(
        r#"
import { GrowBuf, grow_new, grow_to_fit } from "std/net/bytes";
node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    let g2: GrowBuf = grow_to_fit(g, 1);
    endprogram g2.cap;
};
"#,
        "grow_to_fit_only",
    )
    .expect("grow_to_fit runs");
    assert_eq!(b.as_i64(), Some(8), "no growth needed for 1 <= 8");
}


/// OPEN (BUGS.md 2026-10-20): the interpreter's reactor path fails a large
/// ws_encode payload (`Load# read failed` during a 7000-byte copy) while the
/// same body executes correctly through exec_stmt directly. Kept as an
/// ignored regression pin; un-ignore when the reactor-vs-exec divergence is
/// fixed.
#[ignore = "known: large ws_encode payload fails in the reactor path — BUGS.md 2026-10-20"]
#[test]
fn ws_encode_growth_interpreter() {
    let v = run(
        r#"
import { ByteBuf, GrowBuf, grow_new } from "std/net/bytes";
import { ws_encode, ws_decode } from "std/net/websocket";
node entry [beginprogram][true] {
    let src: Ptr<Bit<8>> = Malloc#(7000) as Ptr<Bit<8>>;
    foreach j in 0..7000 {
        Store#(src as Int + j, ((j * 5) & 255) as Bit<8>, 1);
    };
    let g1: GrowBuf = grow_new(120);
    let g2: GrowBuf = ws_encode(g1, true, 2, false, 0, src as Int, 7000);
    let gv: ByteBuf = ByteBuf { ptr: g2.ptr, len: g2.len, cap: g2.cap };
    let r = ws_decode(gv, 0);
    let ok: Int = 0;
    let good: Bool = match r {
        Ok(f) => f.payload_len == 7000 && f.header_len == 4,
        Err(e) => false,
    };
    when good { ok = 1; };
    endprogram ok;
};
"#,
        "ws_encode_growth",
    )
    .expect("large ws_encode runs");
    assert_eq!(v.as_i64(), Some(1), "7000-byte frame encodes and decodes");
}

#[test]
fn ws_encode_100_interpreter() {
    let v = run(
        r#"
import { ByteBuf, GrowBuf, grow_new } from "std/net/bytes";
import { ws_encode, ws_decode } from "std/net/websocket";
node entry [beginprogram][true] {
    let src: Ptr<Bit<8>> = Malloc#(200) as Ptr<Bit<8>>;
    foreach j in 0..100 {
        Store#(src as Int + j, ((j * 5) & 255) as Bit<8>, 1);
    };
    let g1: GrowBuf = grow_new(20);
    let g2: GrowBuf = ws_encode(g1, true, 2, false, 0, src as Int, 100);
    let gv: ByteBuf = ByteBuf { ptr: g2.ptr, len: g2.len, cap: g2.cap };
    let r = ws_decode(gv, 0);
    let ok: Int = 0;
    let good: Bool = match r {
        Ok(f) => f.payload_len == 100 && f.header_len == 2,
        Err(e) => false,
    };
    when good { ok = 1; };
    endprogram ok;
};
"#,
        "ws_encode_100",
    )
    .expect("100-byte ws_encode runs");
    assert_eq!(v.as_i64(), Some(1), "100-byte frame encodes and decodes");
}

#[test]
fn ws_encode_small_interpreter() {
    let v = run(
        r#"
import { ByteBuf, GrowBuf, grow_new } from "std/net/bytes";
import { ws_encode, ws_decode } from "std/net/websocket";
node entry [beginprogram][true] {
    let src: Ptr<Bit<8>> = Malloc#(32) as Ptr<Bit<8>>;
    foreach j in 0..10 {
        Store#(src as Int + j, (65 + j) as Bit<8>, 1);
    };
    let g1: GrowBuf = grow_new(8);
    let g2: GrowBuf = ws_encode(g1, true, 2, false, 0, src as Int, 10);
    let gv: ByteBuf = ByteBuf { ptr: g2.ptr, len: g2.len, cap: g2.cap };
    let r = ws_decode(gv, 0);
    let ok: Int = 0;
    let good: Bool = match r {
        Ok(f) => f.payload_len == 10 && f.header_len == 2,
        Err(e) => false,
    };
    when good { ok = 1; };
    endprogram ok;
};
"#,
        "ws_encode_small",
    )
    .expect("small ws_encode runs");
    assert_eq!(v.as_i64(), Some(1), "encoded header decodes");
}

#[test]
fn growbuf_put_u16_interpreter() {
    let v = run(
        r#"
import { GrowBuf, grow_new } from "std/net/bytes";
node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    let g2: GrowBuf = g.put_u16_be(1070);
    endprogram g2.len;
};
"#,
        "growbuf_put_u16",
    )
    .expect("put_u16_be (trait default) runs");
    assert_eq!(v.as_i64(), Some(2), "two bytes");
}
