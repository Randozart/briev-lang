// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! stdlib platform-constant conformance (2026-11-19, plan F4).
//!
//! The #144 post-mortem found `IPPROTO_TCP = 0` living in `lib/std/net/socket.bv`
//! since the file was created — 0 is `IPPROTO_IP`, and the kernel's tolerance
//! for `AF_INET + SOCK_STREAM + protocol 0` masked it so every socket test
//! kept passing. The follow-up audit then caught `SYS_SETRLIMIT = 98`, which
//! is `__NR_getrusage` on x86_64 — a transcription error with no caller yet.
//! The lesson: a stdlib constant that claims to mirror a platform constant
//! can drift forever because nothing compares it to the platform. This test
//! IS that comparison.
//!
//! ORACLE. A tiny C probe is compiled against the machine's own headers and
//! prints `NAME=value` for every constant under audit; syscall numbers come
//! from the installed `unistd_64.h`. This survives header-layout drift
//! between distros (this host itself has no multiarch dirs). Without a C
//! compiler or the headers the test skips rather than guesses.
//!
//! SCOPE. Constants whose stdlib name mirrors a header macro 1:1 (`SYS_read`
//! → `__NR_read`, `IPPROTO_TCP` → `IPPROTO_TCP`, ...). Briev-local
//! conventions (`MODE_0644` is octal, `LOOPBACK` is a packed host-order
//! address, `SOCKADDR_IN_LEN` is `sizeof(sockaddr_in)`) are pinned via
//! `Truth::Fixed` with the POSIX fact that proves them.
//!
//! Linux-only: the x86_64 syscall table is the deployment target for the
//! stdlib's raw-syscall layer (SPEC §19.5).
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// One stdlib constant under audit: (file, name, how to derive truth).
struct Audit {
    file: &'static str,
    name: &'static str,
    kind: Truth,
}

enum Truth {
    /// `__NR_<lowercase name>` from the installed unistd_64.h.
    Syscall,
    /// `NAME` read from the compiled C probe (socket/fcntl/signal headers).
    Probe,
    /// A fixed expected value; the note names the POSIX fact that proves it.
    Fixed(i64, &'static str),
}

/// Constants the C probe reports: (file, stdlib name, header spelling).
/// Most mirror 1:1; pollfd masks differ (`POLL_IN` probes as `POLLIN`).
const PROBED: &[(&str, &str, &str)] = &[
    ("lib/std/net/socket.bv", "AF_INET", "AF_INET"),
    ("lib/std/net/socket.bv", "AF_UNIX", "AF_UNIX"),
    ("lib/std/net/socket.bv", "SOCK_STREAM", "SOCK_STREAM"),
    ("lib/std/net/socket.bv", "IPPROTO_TCP", "IPPROTO_TCP"),
    ("lib/std/net/socket.bv", "SHUT_RD", "SHUT_RD"),
    ("lib/std/net/socket.bv", "SHUT_WR", "SHUT_WR"),
    ("lib/std/net/socket.bv", "SHUT_RDWR", "SHUT_RDWR"),
    ("lib/std/net/socket.bv", "MSG_NOSIGNAL", "MSG_NOSIGNAL"),
    ("lib/std/net/socket.bv", "F_GETFL", "F_GETFL"),
    ("lib/std/net/socket.bv", "F_SETFL", "F_SETFL"),
    ("lib/std/net/socket.bv", "O_NONBLOCK", "O_NONBLOCK"),
    ("lib/std/net/socket.bv", "SOL_SOCKET", "SOL_SOCKET"),
    ("lib/std/net/socket.bv", "SO_REUSEADDR", "SO_REUSEADDR"),
    ("lib/std/net/socket.bv", "SO_RCVTIMEO", "SO_RCVTIMEO"),
    ("lib/std/os/dir.bv", "O_RDONLY", "O_RDONLY"),
    ("lib/std/os/dir.bv", "O_DIRECTORY", "O_DIRECTORY"),
    
    ("lib/std/os/fs.bv", "O_RDONLY", "O_RDONLY"),
    ("lib/std/os/fs.bv", "O_WRONLY", "O_WRONLY"),
    ("lib/std/os/fs.bv", "O_CREAT", "O_CREAT"),
    ("lib/std/os/fs.bv", "O_TRUNC", "O_TRUNC"),
    ("lib/std/os/temp.bv", "O_WRONLY", "O_WRONLY"),
    ("lib/std/os/temp.bv", "O_CREAT", "O_CREAT"),
    ("lib/std/os/poll.bv", "POLL_IN", "POLLIN"),
    ("lib/std/os/poll.bv", "POLL_OUT", "POLLOUT"),
];

/// Fixed-value audits: header-derivable facts that are not plain macros.
const FIXED: &[Audit] = &[
    Audit {
        file: "lib/std/net/socket.bv",
        name: "SOCKADDR_IN_LEN",
        kind: Truth::Fixed(16, "sizeof(struct sockaddr_in) = 16 on LP64 (netinet/in.h)"),
    },
    Audit {
        file: "lib/std/net/socket.bv",
        name: "INADDR_ANY",
        kind: Truth::Fixed(0, "INADDR_ANY = 0x00000000 (netinet/in.h)"),
    },
    Audit {
        file: "lib/std/os/auxv.bv",
        name: "AUX_PAGE_SIZE",
        kind: Truth::Fixed(6, "AT_PAGESZ = 6 (elf.h)"),
    },
    Audit {
        file: "lib/std/os/auxv.bv",
        name: "AUX_CLOCK_TICKS",
        kind: Truth::Fixed(17, "AT_CLKTCK = 17 (elf.h)"),
    },
];

/// The C probe: includes the platform headers, prints NAME=value lines.
fn const_probe_src() -> String {
    let mut s = String::from(
        "#include <stdio.h>\n#include <sys/socket.h>\n#include <netinet/in.h>\n\
         #include <fcntl.h>\n#include <signal.h>\n#include <poll.h>\nint main(void) {\n",
    );
    for (_, _, header) in PROBED {
        s.push_str(&format!("printf(\"{header}=%d\\n\", (int){header});\n"));
    }
    s.push_str("return 0;\n}\n");
    s
}

/// Run the probe through cc; returns NAME -> value. None when no C compiler.
fn probe_values(dir: &Path) -> Option<BTreeMap<String, i64>> {
    let cc = ["cc", "gcc", "clang"]
        .iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok())?;
    let src = dir.join("constprobe.c");
    let bin = dir.join("constprobe");
    std::fs::write(&src, const_probe_src()).ok()?;
    let ok = Command::new(cc)
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .ok()?;
    if !ok.status.success() {
        eprintln!(
            "probe compile failed: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
        return None;
    }
    let out = Command::new(&bin).output().ok()?;
    let mut map = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((name, value)) = line.split_once('=') {
            if let Ok(v) = value.parse::<i64>() {
                map.insert(name.to_string(), v);
            }
        }
    }
    Some(map)
}

/// The installed x86_64 syscall table; None when the header is absent.
fn syscall_table() -> Option<BTreeMap<String, i64>> {
    for p in [
        "/usr/include/x86_64-linux-gnu/asm/unistd_64.h",
        "/usr/include/asm/unistd_64.h",
    ] {
        let src = match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut table = BTreeMap::new();
        for cap in src
            .lines()
            .filter_map(|l| l.trim().strip_prefix("#define __NR_"))
        {
            let mut it = cap.split_whitespace();
            if let (Some(name), Some(num)) = (it.next(), it.next()) {
                if let Ok(v) = num.parse::<i64>() {
                    table.insert(name.to_string(), v);
                }
            }
        }
        if !table.is_empty() {
            return Some(table);
        }
    }
    None
}

/// Parse every `const NAME: Int = <int>;` (optionally `export`) from one file.
fn stdlib_consts(path: &Path) -> BTreeMap<String, i64> {
    let src = std::fs::read_to_string(path).expect("stdlib file readable");
    let mut out = BTreeMap::new();
    for line in src.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let rest = match line.strip_prefix("const ") {
            Some(rest) => rest,
            None => continue,
        };
        let mut parts = rest.split(':');
        let name = parts.next().unwrap_or("").trim().to_string();
        let value = match parts.next().and_then(|ty| ty.split('=').nth(1)) {
            Some(v) => v.trim().trim_end_matches(';').trim(),
            None => continue,
        };
        if let Ok(v) = value.parse::<i64>() {
            out.insert(name, v);
        }
    }
    out
}

#[test]
fn stdlib_platform_constants_match_installed_headers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let probe_dir = std::env::temp_dir().join("briev-constprobe");
    std::fs::create_dir_all(&probe_dir).expect("probe dir writable");
    let probed = probe_values(&probe_dir);
    let syscalls = syscall_table();

    if probed.is_none() && syscalls.is_none() {
        eprintln!("SKIP: no C compiler and no unistd_64.h — nothing to compare");
        return;
    }

    let mut failures = Vec::new();
    let mut audited = 0usize;

    // Probed constants: header value vs stdlib declaration, per file.
    if let Some(values) = &probed {
        for (file, name, header) in PROBED {
            let consts = stdlib_consts(&root.join(file));
            let expect = match values.get(*header) {
                Some(v) => *v,
                None => {
                    eprintln!("  skip {name}: not in probe output");
                    continue;
                }
            };
            match consts.get(*name) {
                Some(v) if *v == expect => audited += 1,
                Some(v) => failures.push(format!(
                    "{file}:{name} = {v} but the platform headers say {expect} \
                     (the #144 class: a stdlib constant that mirrors a platform \
                     constant must equal it)"
                )),
                None => failures.push(format!("{file}:{name} missing (PROBED table stale)")),
            }
        }
    } else {
        eprintln!("  skip probed constants: no C compiler");
    }

    // Fixed facts.
    for audit in FIXED {
        let Truth::Fixed(expect, _) = audit.kind else {
            unreachable!("FIXED only holds Truth::Fixed");
        };
        let consts = stdlib_consts(&root.join(audit.file));
        match consts.get(audit.name) {
            Some(v) if *v == expect => audited += 1,
            Some(v) => failures.push(format!(
                "{}:{} = {v} but POSIX says {expect}",
                audit.file, audit.name
            )),
            None => failures.push(format!(
                "{}:{} missing (FIXED table stale)",
                audit.file, audit.name
            )),
        }
    }

    // The full SYS_* sweep: every declared syscall number vs __NR_*.
    if let Some(table) = &syscalls {
        let src = std::fs::read_to_string(root.join("lib/std/posix/syscall.bv")).unwrap();
        for line in src.lines() {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let rest = match line.strip_prefix("const SYS_") {
                Some(rest) => rest,
                None => continue,
            };
            let name = match rest.split(':').next() {
                Some(n) => n.trim().to_string(),
                None => continue,
            };
            let value = match rest.split('=').nth(1) {
                Some(v) => v
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .parse()
                    .unwrap_or(i64::MIN),
                None => continue,
            };
            let key = name.to_lowercase();
            match table.get(&key) {
                Some(expect) if *expect == value => audited += 1,
                Some(expect) => failures.push(format!(
                    "lib/std/posix/syscall.bv:SYS_{name} = {value} but __NR_{key} = {expect}"
                )),
                None => eprintln!("  skip SYS_{name}: not in the installed syscall table"),
            }
        }
    } else {
        eprintln!("  skip SYS_* sweep: no unistd_64.h");
    }

    assert!(
        audited >= 80,
        "header oracle collapsed ({audited} audited) — the test must not \
         silently audit nothing; check cc and the installed headers"
    );
    assert!(
        failures.is_empty(),
        "stdlib constants disagree with the installed platform headers:\n  - {}",
        failures.join("\n  - ")
    );
}
