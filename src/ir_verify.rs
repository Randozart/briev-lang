// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Post-codegen verification of the module the compiler is about to hand to the
//! linker.
//!
//! 2026-11-18 (plan `docs/plans/2026-11-18-compiler-diagnostics-ir-verification.md`):
//! before this, `brievc build --llvm` wrote the `.ll` and returned **0** even when
//! LLVM could not parse it, and a normal build surfaced only
//! `clang failed to compile '<file>.bv'` with the real diagnostic unattributed on
//! stderr. Two codegen bugs in one session (the f16 store, the loop-carried Float)
//! were invisible for exactly that reason: a compile step that reports success on a
//! module the toolchain rejects.
//!
//! The check is one `opt -passes=verify` invocation (falling back to
//! `clang -x ir -c`), on the host target only. Its verdict is about
//! **well-formedness**, not correctness: a module that types-checks can still
//! compute the wrong number, so this never substitutes for an arithmetic or
//! differential gate.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Outcome of verifying one emitted module.
pub enum IrVerify {
    /// The module is well-formed, or no verifier is installed on this host.
    Ok {
        /// False when neither `opt` nor `clang` could be spawned.
        ran: bool,
    },
    /// The module is invalid. The payload is a ready-to-print diagnostic.
    Invalid(String),
}

/// Verify `ir` as an LLVM module.
///
/// `source_label` names the program in the message (the author's file).
/// `artifact_hint` is where the caller will leave the `.ll` so the author has a
/// reproducer; it is only mentioned, never created here.
pub fn verify_llvm_ir(ir: &str, source_label: &str, artifact_hint: &str) -> IrVerify {
    // A per-run temp file: the verifier wants a path, and the module is the
    // thing under test, so it must be the exact bytes we are about to ship.
    let named = temp_path();
    if std::fs::write(&named, ir).is_err() {
        return IrVerify::Ok { ran: false };
    }
    let verdict = run_verifier(&named);
    let _ = std::fs::remove_file(&named);
    match verdict {
        None => IrVerify::Ok { ran: false },
        Some(None) => IrVerify::Ok { ran: true },
        Some(Some(stderr)) => IrVerify::Invalid(render(&stderr, ir, source_label, artifact_hint)),
    }
}

fn temp_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "briev-ir-verify-{}-{}.ll",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    p
}

/// `None` = no verifier could be spawned (skip). `Some(None)` = valid.
/// `Some(Some(stderr))` = invalid, with the tool's own text.
fn run_verifier(path: &Path) -> Option<Option<String>> {
    // `opt -passes=verify` is the pure verifier: it parses, verifies and does no
    // codegen, so its message is about the module and nothing else.
    if let Ok(out) = Command::new("opt")
        .args(["-passes=verify", "-disable-output"])
        .arg(path)
        .output()
    {
        if out.status.success() {
            return Some(None);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !stderr.is_empty() {
            return Some(Some(stderr));
        }
    }
    // Fallback: clang's IR front end (the parser the build path already uses).
    if let Ok(out) = Command::new("clang")
        .args(["-Wno-override-module", "-x", "ir", "-c"])
        .arg(path)
        .arg("-o")
        .arg(path.with_extension("o"))
        .output()
    {
        let _ = std::fs::remove_file(path.with_extension("o"));
        if out.status.success() {
            return Some(None);
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !stderr.is_empty() {
            return Some(Some(stderr));
        }
    }
    None
}

/// Turn the tool's stderr into the house-style block: what, why (with the
/// offending line), a note that this is the compiler's fault, and how to report.
fn render(stderr: &str, ir: &str, source_label: &str, artifact_hint: &str) -> String {
    let (line_no, cause) = first_error(stderr);
    let excerpt = line_no
        .map(|n| excerpt_around(ir, n))
        .unwrap_or_else(|| String::from("(line number not reported)"));
    let trace = definition_trace(ir, &cause, line_no);
    // Name the enclosing function: the registers are local to it, and it is the
    // only "where in the program" this layer can honestly offer.
    let enclosing = function_range(ir, line_no)
        .0
        .checked_sub(1)
        .and_then(|idx| ir.lines().nth(idx))
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("define "));
    let in_fn = enclosing
        .map(|l| format!("        the instruction is in: {l}\n"))
        .unwrap_or_default();
    format!(
        "the compiler produced invalid IR for '{source_label}'\n  \
         why:  LLVM rejected the module: {cause}\n{excerpt}\n{trace}{in_fn}  \
         note: this is a compiler defect, not a mistake in your program\n  \
         fix:  the module is kept at '{artifact_hint}' — pass it, with the smallest \
         program that reproduces it, to whoever maintains the compiler"
    )
}

/// The first `file:line:col: error: …` in the tool's output.
fn first_error(stderr: &str) -> (Option<usize>, String) {
    for raw in stderr.lines() {
        let line = raw.trim();
        if !line.contains("error:") {
            continue;
        }
        // Both tools print `[tool: ]path:LINE:COL: error: …`, so the last two
        // numeric colon-separated fields before the message are line and column.
        // (`split(':').nth(1)` — the obvious first draft — returns the PATH for
        // `opt: /tmp/x.ll:817:33: error: …` and loses the line entirely.)
        let pos = line.find("error:").unwrap_or(0);
        let cause = line[pos + "error:".len()..].trim().to_string();
        let mut nums = line[..pos]
            .rsplit(':')
            .filter_map(|t| t.trim().parse::<usize>().ok());
        let _col = nums.next();
        let line_no = nums.next();
        return (line_no, cause);
    }
    (
        None,
        stderr
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .to_string(),
    )
}

/// Where the registers the message names were DEFINED, one hop at a time.
///
/// The verifier reports a use (`%t36` is the wrong type here); the author needs
/// the definition (`%t36 = load i64, ptr %t20`) and ideally what that loaded from
/// (`%t20 = alloca i64`). Emitter breadcrumbs would name the Briev statement
/// itself, but they would bloat every `.ll` and skew the IR-comparison tests, so
/// this reads the module we already have: no emitter change, no risk, and most of
/// the value.
fn definition_trace(ir: &str, cause: &str, at_line: Option<usize>) -> String {
    // Registers are FUNCTION-local: `%t36` in one function has nothing to do with
    // `%t36` in another. Searching the whole module found a definition 200 lines
    // away in a different function — a confident, wrong pointer, which is worse
    // than none. Confine the search to the enclosing `define … }` body.
    let (lo, hi) = function_range(ir, at_line);
    let mut out = String::new();
    let mut seen: Vec<String> = Vec::new();
    let mut frontier = registers_in(cause);
    for _hop in 0..2 {
        let mut next = Vec::new();
        for reg in frontier.clone() {
            if seen.contains(&reg) {
                continue;
            }
            seen.push(reg.clone());
            if let Some((line_no, text)) = find_definition(ir, &reg, lo, hi) {
                out.push_str(&format!(
                    "        {reg} is defined at line {line_no}: {}\n",
                    text.trim()
                ));
                // Follow the operands a load/alloca hands back.
                for operand in registers_in(&text) {
                    if operand != reg {
                        next.push(operand);
                    }
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    out
}

/// The `%tN` names mentioned in a message, in order, without duplicates.
fn registers_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '%' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_alphanumeric() || bytes[j] == '_' || bytes[j] == '.') {
                j += 1;
            }
            if j > i + 1 {
                let reg: String = bytes[i..j].iter().collect();
                if !out.contains(&reg) {
                    out.push(reg);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The line that defines `reg` (`%tN = …`), skipping uses.
fn find_definition(ir: &str, reg: &str, lo: usize, hi: usize) -> Option<(usize, String)> {
    let needle = format!("{reg} = ");
    for (idx, line) in ir.lines().enumerate() {
        let n = idx + 1;
        if n < lo || n > hi {
            continue;
        }
        if line.trim_start().starts_with(&needle) {
            return Some((n, line.to_string()));
        }
    }
    None
}

/// The 1-based line bounds of the `define …` body containing `at_line`.
/// Falls back to the whole module when the line is unknown or outside a function.
fn function_range(ir: &str, at_line: Option<usize>) -> (usize, usize) {
    let Some(target) = at_line else {
        return (1, usize::MAX);
    };
    let lines: Vec<&str> = ir.lines().collect();
    let mut start = 1;
    for (idx, line) in lines.iter().enumerate() {
        if idx + 1 > target {
            break;
        }
        if line.starts_with("define ") {
            start = idx + 1;
        }
    }
    let mut end = lines.len();
    for (idx, line) in lines.iter().enumerate() {
        if idx + 1 <= start {
            continue;
        }
        if line.starts_with("define ") {
            end = idx;
            break;
        }
    }
    (start, end)
}

/// The offending line with one line of context either side, indented, so the
/// reader sees the instruction rather than hunting the file.
fn excerpt_around(ir: &str, line_no: usize) -> String {
    let lines: Vec<&str> = ir.lines().collect();
    if line_no == 0 || line_no > lines.len() {
        return String::new();
    }
    let lo = line_no.saturating_sub(2);
    let hi = (line_no + 1).min(lines.len());
    let mut out = String::new();
    for idx in lo..hi {
        let marker = if idx + 1 == line_no { ">" } else { " " };
        out.push_str(&format!(
            "        {marker} {:>6} | {}\n",
            idx + 1,
            lines[idx].trim_end()
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "define i64 @f() {\nentry:\n  ret i64 0\n}\n";
    const BAD: &str = "define i64 @f() {\nentry:\n  %1 = add i64 0, 0\n  %2 = fadd float 0.0, %1\n  ret i64 0\n}\n";

    #[test]
    fn a_well_formed_module_passes_and_names_no_defect() {
        match verify_llvm_ir(GOOD, "ok.bv", "/tmp/ok.ll") {
            IrVerify::Ok { .. } => {}
            IrVerify::Invalid(msg) => panic!("valid IR rejected: {msg}"),
        }
    }

    #[test]
    fn an_invalid_module_is_reported_as_a_compiler_defect_with_its_line() {
        // Skips cleanly on a host without opt/clang.
        match verify_llvm_ir(BAD, "bad.bv", "/tmp/bad.ll") {
            IrVerify::Ok { ran } => {
                if ran {
                    panic!("invalid IR passed verification");
                }
            }
            IrVerify::Invalid(msg) => {
                assert!(msg.contains("invalid IR for 'bad.bv'"), "{msg}");
                assert!(
                    msg.contains("this is a compiler defect, not a mistake in your program"),
                    "the author must not be blamed: {msg}"
                );
                assert!(
                    msg.contains("/tmp/bad.ll"),
                    "the reproducer path is missing: {msg}"
                );
                // The offending instruction is quoted, not just a register name.
                assert!(msg.contains("fadd float 0.0, %1"), "{msg}");
            }
        }
    }

    #[test]
    fn the_trace_follows_a_register_to_its_definition() {
        let ir = "define i64 @f() {\nentry:\n  %t20 = alloca i64\n  %t36 = load i64, ptr %t20\n  ret i64 0\n}\n";
        let trace = definition_trace(
            ir,
            "'%t36' defined with type 'i64' but expected 'float'",
            Some(4),
        );
        assert!(trace.contains("%t36 is defined at line 4"), "{trace}");
        assert!(trace.contains("load i64, ptr %t20"), "{trace}");
        assert!(trace.contains("%t20 is defined at line 3"), "{trace}");
    }

    #[test]
    fn the_trace_never_crosses_a_function_boundary() {
        // Two functions reuse %t36. The failing use is in the SECOND one, whose
        // %t36 is a load; the first one's `add` must not be reported.
        let ir = "define i64 @a() {\nentry:\n  %t36 = add i64 0, 0\n  ret i64 0\n}\ndefine i64 @b() {\nentry:\n  %t36 = load i64, ptr %p\n  ret i64 0\n}\n";
        let trace = definition_trace(ir, "error about %t36", Some(8));
        assert!(trace.contains("load i64, ptr %p"), "{trace}");
        assert!(!trace.contains("add i64 0, 0"), "wrong function's register: {trace}");
    }

    #[test]
    fn registers_in_reads_names_and_ignores_stray_percent() {
        assert_eq!(registers_in("%a, %b %a 50%"), vec!["%a", "%b"]);
        assert!(registers_in("no registers").is_empty());
    }

    #[test]
    fn the_line_number_survives_both_tool_prefixes() {
        // `opt` prefixes the tool name, `clang` does not; both end
        // `path:LINE:COL: error: …`. The first draft read the path as the line.
        let (n, cause) = first_error("opt: /tmp/x.ll:817:33: error: bad operand\n");
        assert_eq!(n, Some(817));
        assert_eq!(cause, "bad operand");
        let (n2, _) = first_error("/tmp/x.ll:12:5: error: nope\n");
        assert_eq!(n2, Some(12));
    }

    #[test]
    fn excerpt_marks_the_offending_line_and_bounds_itself() {
        let text = "a\nb\nc\nd\ne\n";
        let ex = excerpt_around(text, 3);
        assert!(ex.contains("> "), "the failing line must be marked: {ex}");
        assert!(ex.contains("c"), "{ex}");
        // Never panics past either end.
        assert!(excerpt_around(text, 1).contains('a'));
        assert!(excerpt_around(text, 99).is_empty());
        assert!(excerpt_around(text, 0).is_empty());
    }
}
