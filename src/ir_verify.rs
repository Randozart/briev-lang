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

/// Who rejected the module when Briev's own pre-check did: LLVM never saw it.
const OWN_CHECK: &str = "Briev's emitted-module check";

/// Verify `ir` as an LLVM module.
///
/// `source_label` names the program in the message (the author's file).
/// `artifact_hint` is where the caller will leave the `.ll` so the author has a
/// reproducer; it is only mentioned, never created here.
pub fn verify_llvm_ir(ir: &str, source_label: &str, artifact_hint: &str) -> IrVerify {
    // Cheap textual pre-check that needs no toolchain: a call target with no
    // `define` and no `declare` is invalid IR whatever LLVM thinks, and it is the
    // one class whose cause is always "codegen emitted a call to something the
    // program never defined". It catches the case that motivated this module —
    // `call ptr @str_to_c(ptr …)` from a protocol binding whose function was never
    // imported — without touching legitimate modules that DECLARE a protocol and
    // define its bindings elsewhere (they emit no casts, hence no calls).
    if let Some(msg) = undefined_call_targets(ir) {
        return IrVerify::Invalid(render(OWN_CHECK, &msg, ir, source_label, artifact_hint));
    }
    // The companion check the toolchain does not perform: LLVM 23 accepts a call
    // whose arguments do not match the callee's parameter list (and accepts a
    // wrong argument count), so `opt -passes=verify` alone would ship the bug.
    // See `mismatched_call_signatures` for the measurement and for issue #133.
    if let Some(msg) = mismatched_call_signatures(ir) {
        return IrVerify::Invalid(render(OWN_CHECK, &msg, ir, source_label, artifact_hint));
    }
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
        Some(Some(stderr)) => IrVerify::Invalid(render("LLVM", &stderr, ir, source_label, artifact_hint)),
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
fn render(
    objector: &str,
    stderr: &str,
    ir: &str,
    source_label: &str,
    artifact_hint: &str,
) -> String {
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
         why:  {objector} rejected the module: {cause}\n{excerpt}\n{trace}{in_fn}  \
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

/// `Some(reason)` when the module calls a symbol it never defines or declares.
///
/// Only CALL targets are considered (`call … @name(`): a name that appears in a
/// global, a metadata node or a `getelementptr` is not a call, and treating those
/// as symbols produced false positives when this check lived at the type level
/// (two stdlib modules that DECLARE protocols and leave the bindings to the
/// importer were rejected). Emitting a call to an undefined function is always a
/// compiler defect — the definition either exists and was dropped, or never did.
fn undefined_call_targets(ir: &str) -> Option<String> {
    let mut defined: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for line in ir.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("define ") {
            if let Some(n) = rest.split('@').nth(1) {
                defined.insert(n.split('(').next().unwrap_or(n));
            }
        } else if let Some(rest) = t.strip_prefix("declare ") {
            if let Some(n) = rest.split('@').nth(1) {
                defined.insert(n.split('(').next().unwrap_or(n));
            }
        }
    }
    let mut missing: Vec<String> = Vec::new();
    for line in ir.lines() {
        let t = line.trim_start();
        if !t.contains("call ") {
            continue;
        }
        // Every `@name(` on a call line is a call target (direct calls, and the
        // symbol operand of an indirect call's declaration is never here).
        let mut rest = t;
        while let Some(at) = rest.find('@') {
            rest = &rest[at + 1..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '$')
                .collect();
            if name.is_empty() {
                continue;
            }
            let after = rest.get(name.len()..).unwrap_or("");
            if !after.starts_with('(') {
                continue; // a reference (e.g. a function pointer), not a call target
            }
            // `@llvm.*` are LLVM's own intrinsics: always available, never
            // declared in the module. Flagging them made this check reject two
            // healthy programs (`@llvm.smax.i64`, `@llvm.exp.f32`) — the
            // allowance ratchet caught it, which is exactly what it is for.
            if name.starts_with("llvm.") {
                continue;
            }
            if !defined.contains(name.as_str()) && !missing.contains(&name) {
                missing.push(name);
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    missing.sort();
    Some(format!(
        "the module calls {} with no definition or declaration",
        missing
            .iter()
            .map(|n| format!("'@{n}'"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// `Some(reason)` when a call passes arguments whose representation disagrees
/// with the callee's declared parameter list.
///
/// This is the check the external toolchain does not perform. Verified on LLVM
/// 23.1.5: `opt -passes=verify`, `llvm-as` and `clang -c` all accept
/// `define i64 @f(i64)` called as `call i64 @f(ptr %p)`, and accept a call with
/// the wrong argument COUNT. A result-type mismatch is still caught, so the
/// verifier runs — it simply stopped policing call/definition agreement. A
/// module like that compiles to a binary that passes a pointer where an integer
/// is read (issue #133: the address of a string literal was read as a
/// `StringBuilder*` and the literal's stored length came back as a pointer to
/// address 3).
///
/// Emitting such a call is always a compiler defect. The fatal instance is issue
/// #133 (a name carrying two definitions: the call site resolved against a
/// `String` signature while the module emitted the `StringBuilder` one), which is
/// repaired in the frontend — one name, one definition (SPEC §881). This layer
/// holds the invariants it can prove with no false positives: argument count and
/// representation class. See `compare_arguments` for why widths are exempt.
fn mismatched_call_signatures(ir: &str) -> Option<String> {
    let signatures = collect_signatures(ir);
    if signatures.is_empty() {
        return None;
    }
    ir.lines()
        .enumerate()
        .find_map(|(index, line)| check_call_line(index + 1, line, &signatures))
}

/// One line's call sites against the signature table. Split out so each function
/// holds a single loop (the inner work is per-call, bounded by argument count).
fn check_call_line(
    line_no: usize,
    line: &str,
    signatures: &std::collections::HashMap<String, (Vec<String>, bool)>,
) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.contains("call ") {
        return None;
    }
    for (name, args) in direct_call_sites(trimmed) {
        let Some((params, variadic)) = signatures.get(&name) else {
            continue; // undefined targets are a different diagnostic's job
        };
        let expected = if *variadic {
            &params[..params.len().saturating_sub(1)]
        } else {
            &params[..]
        };
        if let Some(why) = compare_arguments(line_no, &name, &args, expected, *variadic) {
            return Some(why);
        }
    }
    None
}

/// Compare one call's arguments with the callee's parameters.
///
/// Two invariants are proved here, both with zero false positives on the corpus:
///
/// 1. **Arity** — the call passes exactly as many arguments as the callee takes.
/// 2. **Representation class** — an argument is an integer-ish value, a floating
///    value, or something else, and the two sides must agree on which.
///
/// Widths are deliberately NOT compared (`i32` vs `i64`, and `ptr` vs `i64`, are
/// one class). Those aliases exist in the emitter by convention — heap handles
/// and function values cross call boundaries as `i64` while some callees declare
/// `ptr` — and both land in the same integer register, so the pair is chosen
/// deliberately rather than by mistake. They are also indistinguishable *at this
/// layer* from a real defect: issue #133 passed a `String` where a `StringBuilder`
/// handle was expected, which is `ptr` vs `i64` and crashes because the callee
/// dereferences the pointer's pointee as a struct. That defect is a name
/// collision, so it is caught in the frontend (one name, one definition), not
/// here. What this check does catch is a genuine cross-class error — an integer
/// where a floating value is read, an aggregate where a scalar is.
fn compare_arguments(
    line_no: usize,
    name: &str,
    args: &str,
    expected: &[String],
    variadic: bool,
) -> Option<String> {
    let got = top_level_arguments(args);
    if !variadic && got.len() != expected.len() {
        return Some(describe_mismatch(
            line_no,
            name,
            &format!("{} argument(s)", got.len()),
            &format!("{} parameter(s)", expected.len()),
        ));
    }
    for (position, (actual, declared)) in got.iter().zip(expected).enumerate() {
        let actual_head = type_head(actual);
        let declared_head = type_head(declared);
        let (actual_class, declared_class) = (class_of(&actual_head), class_of(&declared_head));
        if actual_class != declared_class {
            return Some(describe_mismatch(
                line_no,
                name,
                &format!("argument {position} is '{actual_head}' (a {actual_class})"),
                &format!("parameter {position} is '{declared_head}' (a {declared_class})"),
            ));
        }
    }
    None
}

/// The representation class of an LLVM type head: how the value is read, not how
/// wide it is. `i64`, `i32`, `ptr` and `i1` are all integer-class; `float`,
/// `double` and `half` are floating-class; aggregates, vectors and `void` are
/// their own class and must match exactly.
fn class_of(type_head: &str) -> &'static str {
    if type_head == "ptr" || (type_head.starts_with('i') && type_head[1..].chars().all(|c| c.is_ascii_digit())) {
        return "integer";
    }
    if matches!(type_head, "float" | "double" | "half" | "fp128" | "x86_fp80" | "bfloat") {
        return "floating";
    }
    "aggregate"
}

/// The words of the diagnostic: what the call passes, what the callee takes, and
/// why the two disagreeing is the compiler's defect rather than the author's.
fn describe_mismatch(line: usize, name: &str, actual: &str, declared: &str) -> String {
    format!(
        "briev:{line}:1: error: '@{name}' is called with {actual} but its definition \
         takes {declared}. The call site and the callee disagree, which means one name \
         reached codegen with two definitions (SPEC §881 permits one declaration per \
         name across modules), so a value is passed in the wrong representation"
    )
}

/// `name -> (parameter types, is variadic)` for every `define`/`declare`.
fn collect_signatures(ir: &str) -> std::collections::HashMap<String, (Vec<String>, bool)> {
    let mut signatures = std::collections::HashMap::new();
    for line in ir.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix("define ")
            .or_else(|| trimmed.strip_prefix("declare "))
        else {
            continue;
        };
        let Some((name, params)) = split_callee(rest) else {
            continue;
        };
        let fields = top_level_arguments(&params);
        let variadic = fields.iter().any(|f| f.trim() == "...");
        signatures.insert(
            name,
            (
                fields.iter().map(|f| f.trim().to_string()).collect(),
                variadic,
            ),
        );
    }
    signatures
}

/// Every `@name(arguments)` in a line that is a direct call target.
///
/// A name followed by anything other than `(` is a reference (a function pointer
/// passed as a value, a global), not a call site.
fn direct_call_sites(line: &str) -> Vec<(String, String)> {
    let mut sites = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '$')
            .collect();
        if name.is_empty() || name.starts_with("llvm.") {
            continue;
        }
        let after = rest.get(name.len()..).unwrap_or("");
        if !after.starts_with('(') {
            continue;
        }
        if let Some(args) = balanced(&after[1..], '(', ')') {
            sites.push((name, args));
        }
    }
    sites
}

/// The callee name and its parameter list from the tail of a `define`/`declare`.
fn split_callee(rest: &str) -> Option<(String, String)> {
    let at = rest.find('@')?;
    let after = &rest[at + 1..];
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '$')
        .collect();
    if name.is_empty() {
        return None;
    }
    let tail = after.get(name.len()..)?;
    let open = tail.find('(')?;
    let params = balanced(&tail[open + 1..], '(', ')')?;
    Some((name, params))
}

/// Text up to the `close` that matches an already-consumed `open`, skipping
/// nested brackets of every kind (attribute groups like `dereferenceable(8)`,
/// aggregates like `{ i64, ptr }`, and vectors like `<4 x float>`).
fn balanced(text: &str, open: char, close: char) -> Option<String> {
    let mut depth = 1usize;
    let mut out = String::new();
    for c in text.chars() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(out);
            }
        }
        out.push(c);
    }
    None
}

/// Split an argument or parameter list at its top-level commas.
fn top_level_arguments(list: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in list.chars() {
        match c {
            '(' | '<' | '{' | '[' => depth += 1,
            ')' | '>' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                fields.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        fields.push(current.trim().to_string());
    }
    fields
}

/// The representation a field names first: `i64 %x` -> `i64`, `ptr noalias %p`
/// -> `ptr`. LLVM always writes the type first, so this needs no attribute table.
fn type_head(field: &str) -> String {
    field.split_whitespace().next().unwrap_or("").to_string()
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
            while j < bytes.len()
                && (bytes[j].is_alphanumeric() || bytes[j] == '_' || bytes[j] == '.')
            {
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
        assert!(
            !trace.contains("add i64 0, 0"),
            "wrong function's register: {trace}"
        );
    }

    #[test]
    fn registers_in_reads_names_and_ignores_stray_percent() {
        assert_eq!(registers_in("%a, %b %a 50%"), vec!["%a", "%b"]);
        assert!(registers_in("no registers").is_empty());
    }

    #[test]
    fn an_argument_of_the_wrong_representation_class_is_caught() {
        // A floating callee given an integer bit pattern reads a value that was
        // never converted — `fptosi`/`sitofp` was skipped. Nothing downstream
        // notices: the toolchain accepts the call, so the number arrives as
        // garbage. This is the class the emitted-module check owns.
        let ir = "define float @f(float %x) {\nentry:\n  ret float %x\n}\n\
                  define i32 @main() {\nentry:\n  %1 = call float @f(i64 42)\n  ret i32 0\n}\n";
        let why = mismatched_call_signatures(ir).expect("the class mismatch must be reported");
        assert!(why.contains("'@f'"), "names the callee: {why}");
        assert!(why.contains("a floating"), "names what was declared: {why}");
        assert!(
            why.contains("argument 0 is 'i64' (a integer)"),
            "names what was passed: {why}"
        );
        let full = verify_llvm_ir(ir, "repro.bv", "/tmp/repro.ll");
        let IrVerify::Invalid(rendered) = full else {
            panic!("the module must be rejected before the toolchain runs");
        };
        assert!(
            rendered.contains("Briev's emitted-module check rejected the module"),
            "the objector must be named truthfully: {rendered}"
        );
        assert!(
            !rendered.contains("LLVM rejected"),
            "LLVM accepts this module: {rendered}"
        );
    }

    #[test]
    fn integer_class_aliases_are_deliberate_and_stay_exempt() {
        // `ptr` and `i64` are one class on purpose: heap handles and function
        // values cross Briev call boundaries as i64 while the callee may declare
        // ptr, and both land in the same register. Issue #133 is `ptr`-vs-`i64`
        // too, and crashes — but for a semantic reason (a String address read as
        // a StringBuilder), which this layer cannot see. Pinning the exemption
        // here makes it a decision someone chose, so removing it is a visible
        // change rather than an accident.
        let alias = "define i64 @len(i64 %arg0) {\nentry:\n  ret i64 %arg0\n}\n\
                     define i32 @main() {\nentry:\n  %1 = call i64 @len(ptr @str.28)\n  ret i32 0\n}\n";
        assert!(
            mismatched_call_signatures(alias).is_none(),
            "the i64/ptr alias must stay exempt: {:?}",
            mismatched_call_signatures(alias)
        );
        // Width aliases are exempt for the same reason (same register class).
        let width = "define i64 @char_to_str(i64 %c) {\nentry:\n  ret i64 %c\n}\n\
                     define i32 @main() {\nentry:\n  %1 = call i64 @char_to_str(i32 65)\n  ret i32 0\n}\n";
        assert!(mismatched_call_signatures(width).is_none());
    }

    #[test]
    fn a_call_with_the_wrong_argument_count_is_caught() {
        let ir = "define i64 @f(i64 %x) {\nentry:\n  ret i64 %x\n}\ndefine i64 @g() {\nentry:\n  %r = call i64 @f()\n  ret i64 %r\n}\n";
        let why = mismatched_call_signatures(ir).expect("the arity mismatch must be reported");
        assert!(why.contains("0 argument(s)"), "{why}");
        assert!(why.contains("1 parameter(s)"), "{why}");
    }

    #[test]
    fn matching_calls_variadics_and_intrinsics_are_not_reported() {
        // Attributes do not move the type out of first position; a variadic
        // callee accepts any tail; a function pointer argument is a `ptr` on
        // both sides; intrinsics are never declared in the module.
        let ok = "declare i32 @printf(ptr, ...)\n\
                  declare ptr @str_to_c(ptr)\n\
                  define void @sink(ptr %f, i64 noundef %n) {\nentry:\n  ret void\n}\n\
                  define i32 @main() {\nentry:\n  %1 = call i32 (ptr, ...) @printf(ptr @fmt, i64 noundef 3, ptr null)\n\
                  %2 = call ptr @str_to_c(ptr %1)\n\
                  call void @sink(ptr @str_to_c, i64 noundef 2)\n\
                  %3 = call i64 @llvm.smax.i64(i64 1, i64 2)\n  ret i32 0\n}\n";
        assert!(
            mismatched_call_signatures(ok).is_none(),
            "healthy module rejected: {:?}",
            mismatched_call_signatures(ok)
        );
    }

    #[test]
    fn a_call_to_an_undefined_symbol_is_caught_without_a_toolchain() {
        let ir =
            "define i32 @main() {\nentry:\n  %1 = call ptr @str_to_c(ptr %2)\n  ret i32 0\n}\n";
        let why = undefined_call_targets(ir).expect("the missing callee must be reported");
        assert!(why.contains("@str_to_c"), "{why}");
        // A reference that is not a call (function pointer in a global) is not a
        // missing callee, and a declared one is fine.
        let ok = "declare ptr @str_to_c(ptr)\ndefine i32 @main() {\nentry:\n  %1 = call ptr @str_to_c(ptr null)\n  ret i32 0\n}\n";
        assert!(
            undefined_call_targets(ok).is_none(),
            "{:?}",
            undefined_call_targets(ok)
        );
        let indirect =
            "define i32 @main() {\nentry:\n  %f = load ptr, ptr @cmp_int\n  ret i32 0\n}\n";
        assert!(
            undefined_call_targets(indirect).is_none(),
            "a global reference is not a call"
        );
        // LLVM intrinsics need no declaration in the module.
        let intrinsic = "define i32 @main() {\nentry:\n  %1 = call i64 @llvm.smax.i64(i64 1, i64 2)\n  ret i32 0\n}\n";
        assert!(
            undefined_call_targets(intrinsic).is_none(),
            "{:?}",
            undefined_call_targets(intrinsic)
        );
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
