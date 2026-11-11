// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P3 (2026-11-11): interpreter-over-corpus differential.
//!
//! Each corpus program terminates with `endprogram <value>`; the native exit
//! code and the reference interpreter's `ProgramExit` value must agree. This
//! compares observable results without capturing stdout (the interpreter
//! prints through `print!`).

use std::path::{Path, PathBuf};
use std::process::Command;

use briev_compiler::errors::RuntimeError;
use briev_compiler::interpreter::Interpreter;
use briev_compiler::reactor::Reactor;

struct Case {
    name: &'static str,
    src: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "gcd_chain",
        src: r#"
defn gcd(a: Int, b: Int) -> Int {
    when b == 0 { term a; };
    term gcd(b, a % b);
};
node entry [beginprogram][true] {
    let v: Int = gcd(1071, 462) + gcd(84, 36);
    endprogram v % 256;
};
"#,
    },
    Case {
        name: "bubble_sort_checksum",
        src: r#"
let done: Bool = false;
defn bubble(xs: List<Int>) -> List<Int> {
    let n: Int = Count#(xs);
    foreach ii in 0..n {
        foreach jj in 0..(n - 1 - ii) {
            when xs[jj] > xs[jj + 1] {
                let t: Int = xs[jj];
                xs[jj] = xs[jj + 1];
                xs[jj + 1] = t;
            };
        };
    };
    term xs;
};
node entry [beginprogram][true] {
    let xs: List<Int> = [5, 1, 4, 2, 8];
    let s: List<Int> = bubble(xs);
    let sum: Int = 0;
    foreach v in s { sum = sum + v; };
    endprogram sum;
};
"#,
    },
    Case {
        name: "fib_iterative",
        src: r#"
defn fib(n: Int) -> Int {
    let a: Int = 0;
    let b: Int = 1;
    foreach _step in 0..n {
        let t: Int = a + b;
        a = b;
        b = t;
    };
    term a;
};
node entry [beginprogram][true] {
    endprogram fib(20) % 256;
};
"#,
    },
    Case {
        name: "binary_search",
        src: r#"
defn search(xs: List<Int>, target: Int, lo: Int, hi: Int) -> Int {
    when lo > hi { term -1; };
    let mid: Int = (lo + hi) / 2;
    when xs[mid] == target { term mid; };
    when xs[mid] < target { term search(xs, target, mid + 1, hi); };
    term search(xs, target, lo, mid - 1);
};
node entry [beginprogram][true] {
    let xs: List<Int> = [1, 3, 5, 7, 9, 11];
    endprogram (search(xs, 7, 0, Count#(xs) - 1) + 2) % 256;
};
"#,
    },
    Case {
        name: "sieve_count",
        src: r#"
defn primes(limit: Int) -> Int {
    let flags: List<Bool> = [];
    foreach i in 0..limit { flags <- true; };
    foreach p in 0..limit {
        when p >= 2 && flags[p] {
            foreach m in 0..limit {
                when m >= p * p && m % p == 0 { flags[m] = false; };
            };
        };
    };
    let count: Int = 0;
    foreach p in 0..limit { when p >= 2 && flags[p] { count = count + 1; }; };
    term count;
};
node entry [beginprogram][true] {
    endprogram primes(50);
};
"#,
    },
    Case {
        name: "matrix_checksum",
        src: r#"
defn matmul(a: List<Int>, b: List<Int>) -> List<Int> {
    let result: List<Int> = [0, 0, 0, 0];
    foreach i in 0..2 {
        foreach j in 0..2 {
            let s: Int = 0;
            foreach k in 0..2 { s = s + a[i * 2 + k] * b[k * 2 + j]; };
            result[i * 2 + j] = s;
        };
    };
    term result;
};
node entry [beginprogram][true] {
    let a: List<Int> = [1, 2, 3, 4];
    let b: List<Int> = [5, 6, 7, 8];
    let c: List<Int> = matmul(a, b);
    let sum: Int = 0;
    foreach v in c { sum = sum + v; };
    endprogram sum % 256;
};
"#,
    },
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_interpreter(path: &Path, src: &str) -> Result<i64, String> {
    let opts = briev_compiler::pipeline::BuildOptions {
        file_path: path.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let (resolved, _universe) =
        briev_compiler::pipeline::compile_to_typed(&path.to_string_lossy(), src, &opts)
            .map_err(|e| format!("pipeline: {e}"))?;
    let mut interp = Interpreter::new();
    interp.load_program(&resolved);
    interp.seed_inits(&resolved).map_err(|e| format!("seed: {e:?}"))?;
    for item in &resolved {
        if let briev_compiler::ast::TopLevel::Statement(s) = item {
            interp.exec_stmt(s).map_err(|e| format!("seed stmt: {e:?}"))?;
        }
    }
    let mut reactor = Reactor::new();
    reactor.build_from_program(&resolved);
    loop {
        match reactor.run(&mut interp) {
            Ok(true) => {}
            Ok(false) => return Err("equilibrium without endprogram".into()),
            Err(RuntimeError::ProgramExit(v)) => {
                return v.as_i64().ok_or_else(|| "non-integer exit".to_string())
            }
            Err(e) => return Err(format!("runtime: {e:?}")),
        }
    }
}

fn run_native(dir: &Path, src_path: &Path, name: &str) -> Result<i32, String> {
    let out = dir.join(format!("out_{name}"));
    let status = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build"])
        .arg(src_path)
        .args(["--llvm", "--out"])
        .arg(&out)
        .env("BRIEV_HOME", root())
        .current_dir(root())
        .output()
        .map_err(|e| format!("spawn brievc: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "build: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let bin = dir.join(name);
    let link = Command::new("clang")
        .args(["-O0", "-march=native"])
        .arg(out.join(format!("{name}.ll")))
        .arg(root().join("lib/runtime/briev_rt.c"))
        .arg("-o")
        .arg(&bin)
        .output()
        .map_err(|e| format!("spawn clang: {e}"))?;
    if !link.status.success() {
        return Err(format!(
            "link: {}",
            String::from_utf8_lossy(&link.stderr)
        ));
    }
    let run = Command::new(&bin)
        .output()
        .map_err(|e| format!("run: {e}"))?;
    Ok(run.status.code().unwrap_or(-1))
}

#[test]
fn interpreter_and_native_agree_on_corpus_results() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("interpreter_corpus");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for case in CASES {
        let src_path = dir.join(format!("{}.bv", case.name));
        std::fs::write(&src_path, case.src).unwrap();
        let native = run_native(&dir, &src_path, case.name)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let interp = run_interpreter(&src_path, case.src)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(
            i64::from(native),
            interp,
            "{}: native exit {native} vs interpreter {interp}",
            case.name
        );
    }
}
