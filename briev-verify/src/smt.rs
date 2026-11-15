// ── SMT Solver Integration ────────────────────────────────────────────
// 2026-07-12: Phase 5 — SMT-LIB query construction and solver invocation.
// All values are modeled as (_ BitVec N) — no type-specific theories.
// This aligns with the Bits-is-primitive architecture.
//
// 2026-11-14: Proof-result caching. The Z3 subprocess is spawned once per
// unique SMT-LIB query string per compilation. The cache is in-process only
// (no cross-compilation persistence) and is invalidated when the Z3 binary
// changes. See docs/plans/2026-11-14-proof-result-caching.md.

mod encoding;
mod symbols;

use crate::ast::{Expr, Type};
use crate::solver_response::parse_solver_output;
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

// ── Cache ──────────────────────────────────────────────────────────────────
/// Maximum entries before the cache stops inserting (OOM guard).
/// Never reached in normal use; set high enough to not affect correctness.
const PROOF_CACHE_MAX_ENTRIES: usize = 10_000;

/// In-process memoization of SMT query results, keyed by query string.
/// Key is the SMT-LIB query (deterministic output from build_vc_query).
/// Lazily initialized on first cache access via the `cache()` helper.
static PROOF_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

/// Mutex-protected cache statistics: (hits, misses).
/// Must be held together with the cache mutex to allow atomic reset per test.
static STATS: std::sync::Mutex<(usize, usize)> = std::sync::Mutex::new((0, 0));

/// Test-only mutex serializing cache tests. The process-global PROOF_CACHE and
/// STATS are shared across all tests in this binary, which run in parallel by
/// default. Any test that mutates or reads them (via `reset_cache_for_test`)
/// must hold this guard for its whole body to avoid cross-test races.
#[cfg(test)]
static CACHE_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

/// Acquire the shared cache-test serialization guard (test builds only).
///
/// 2026-11-15: `pub(crate)` so verification tests that can escalate to the
/// solver (`verify_bodies` on an obligation the structural rules do not decide)
/// serialize against the cache tests too. The cache and its statistics are
/// process-global, so any test that can reach them must hold this guard.
#[cfg(test)]
pub(crate) fn cache_test_guard() -> std::sync::MutexGuard<'static, ()> {
    CACHE_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the cache-cap warning has already been printed.
static CACHE_CAP_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Return the cache, initializing it on first use.
fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    PROOF_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cache entry: result plus the Z3 version string for invalidation.
#[derive(Clone)]
struct CacheEntry {
    result: SmtResult,
    /// Version of Z3 that produced this result. A mismatch means the cached
    /// result may not be valid (different solver → different model).
    version: String,
}

/// Captured Z3 binary version, used to invalidate the cache when Z3 changes.
/// Lazily initialized on first call to `z3_version()`.
static Z3_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Get the Z3 binary version string, if Z3 is available.
///
/// Unlike `z3_bin()` which returns a path, this returns the version text.
/// Both PATH and managed directory are checked, consistent with `z3_bin()`.
fn z3_version() -> Option<&'static String> {
    // Fast path: already resolved.
    if let Some(v) = Z3_VERSION.get() {
        return if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = find_z3_version() {
        return Some(Z3_VERSION.get_or_init(|| v));
    }
    // Not found: set empty string to avoid re-probing every call.
    Z3_VERSION.get_or_init(|| String::new());
    None
}

/// Probe every known Z3 location in order, returning the first version found.
fn find_z3_version() -> Option<String> {
    if let Some(v) = probe_z3_version(std::ffi::OsStr::new("z3")) {
        return Some(v);
    }
    let managed = managed_z3_path()?;
    if !managed.exists() {
        return None;
    }
    probe_z3_version(managed.as_os_str())
}

/// Probe a single solver binary for its version string.
fn probe_z3_version(bin: &std::ffi::OsStr) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let v = s.trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

/// Log a cache diagnostic to stderr when BRIEV_DIAG_PROVE is set.
fn diag_cache(query: &str, result: &SmtResult, hit: bool) {
    if std::env::var_os("BRIEV_DIAG_PROVE").is_some() {
        let label = if hit { "HIT " } else { "MISS" };
        eprintln!(
            "[prove] {} query_len={} result={:?}",
            label,
            query.len(),
            result
        );
    }
}

/// Look up a cached result for a query string, if the Z3 version matches.
///
/// Returns `None` both on a true miss and on a version mismatch (a stale result
/// from another solver), so the caller re-runs the query in both cases.
fn cache_lookup(key: &str) -> Option<SmtResult> {
    let version = z3_version()?;
    let guard = cache().lock().ok()?;
    let entry = guard.get(key)?;
    if entry.version != *version {
        return None;
    }
    Some(entry.result.clone())
}

/// Insert a solver result into the cache, honoring the entry cap.
/// Emits a one-time warning the first time the cap is reached.
fn insert_cache(key: String, result: SmtResult, version: String) {
    let Ok(mut guard) = cache().lock() else {
        return;
    };
    if guard.len() < PROOF_CACHE_MAX_ENTRIES {
        guard.insert(key, CacheEntry { result, version });
    } else if !CACHE_CAP_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "[prove] warning: proof cache hit cap of {} entries, caching disabled",
            PROOF_CACHE_MAX_ENTRIES
        );
    }
}

/// Result of an SMT query.
#[derive(Debug, Clone, PartialEq)]
pub enum SmtResult {
    /// The formula is satisfiable (counterexample exists).
    Sat(Vec<(String, String)>),
    /// The formula is unsatisfiable (provably true).
    Unsat,
    /// The solver could not determine satisfiability.
    Unknown,
}

/// Prove a formula using the SMT solver.
/// The formula is a Boolean expression encoded as SMT-LIB.
///
/// 2026-11-14: Results are cached by SMT-LIB query string so identical
/// pre/post constraint checks within one compilation avoid redundant Z3 spawns.
pub fn prove_smt(formula: &Expr, _param_types: &[(String, Type)]) -> SmtResult {
    let query = build_smt_query(formula);

    // ── Cache lookup ────────────────────────────────────────────────────────
    if let Some(result) = cache_lookup(&query) {
        STATS.lock().unwrap().0 += 1;
        diag_cache(&query, &result, true);
        return result;
    }

    // ── Cache miss: spawn Z3 ─────────────────────────────────────────────────
    let Some(bin) = z3_bin() else {
        return SmtResult::Unknown;
    };
    let result = match Command::new(&bin)
        .arg("-in")
        .arg("-smt2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = writeln!(stdin, "{}", query);
            }
            match child.wait_with_output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    parse_solver_output(&stdout)
                }
                Err(_) => SmtResult::Unknown,
            }
        }
        Err(_) => SmtResult::Unknown,
    };

    // ── Cache insert ────────────────────────────────────────────────────────
    if let Some(v) = z3_version() {
        insert_cache(query.clone(), result.clone(), v.clone());
    }
    STATS.lock().unwrap().1 += 1;
    diag_cache(&query, &result, false);
    result
}

/// 2026-07-16: P6 — Prove an SMT-LIB2 formula string directly (not from a Briev Expr).
/// Writes formula to stdin of z3 -in -smt2, parses output.
/// Returns Unsat if the formula is unsatisfiable (property holds).
///
/// 2026-11-14: Caching. Same query strings within one compilation avoid a second
/// Z3 subprocess spawn. Cache key is the SMT-LIB string (already deterministic
/// from build_vc_query). Entries are invalidated when Z3's version string changes.
pub fn prove_smt_formula(formula: &str, _timeout_ms: u64) -> SmtResult {
    // ── Fast path: cache lookup ──────────────────────────────────────────────
    if let Some(result) = cache_lookup(formula) {
        STATS.lock().unwrap().0 += 1;
        diag_cache(formula, &result, true);
        return result;
    }

    // ── Cache miss: spawn Z3 ─────────────────────────────────────────────────
    let Some(bin) = z3_bin() else {
        return SmtResult::Unknown;
    };
    let result = match Command::new(&bin)
        .arg("-in")
        .arg("-smt2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = writeln!(stdin, "{formula}");
            }
            drop(child.stdin.take());
            match child.wait_with_output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    parse_solver_output(&stdout)
                }
                Err(_) => SmtResult::Unknown,
            }
        }
        Err(_) => SmtResult::Unknown,
    };

    // ── Cache insert ────────────────────────────────────────────────────────
    if let Some(v) = z3_version() {
        insert_cache(formula.to_string(), result.clone(), v.clone());
    }
    STATS.lock().unwrap().1 += 1;
    diag_cache(formula, &result, false);
    result
}

/// Build an SMT-LIB query from a Briev expression.
/// All values are encoded as (_ BitVec 64) — the only primitive.
fn build_smt_query(expr: &Expr) -> String {
    let Some(term) = encoding::expect(expr, encoding::Sort::Boolean) else {
        return encoding_diagnostic();
    };
    let mut names = std::collections::BTreeSet::new();
    collect_free_idents(expr, &mut names);
    let mut query = query_header(&names);
    query.push_str(&format!("(assert {term})\n(check-sat)\n"));
    query
}

/// A diagnostic script emits no satisfiability verdict. The response parser
/// returns Unknown; unsupported syntax cannot be cached as a proof.
fn encoding_diagnostic() -> String {
    "(echo \"briev: unsupported or ill-sorted expression\")\n".into()
}

fn query_header(names: &std::collections::BTreeSet<String>) -> String {
    let mut query = "(set-option :produce-models true)\n(set-logic QF_BV)\n".to_string();
    let symbols: std::collections::BTreeSet<_> =
        names.iter().map(|name| sanitize_name(name)).collect();
    for symbol in symbols {
        query.push_str(&format!("(declare-const {symbol} (_ BitVec 64))\n"));
    }
    query
}

/// Legacy term API: an unbound sentinel fails closed in existing raw-query
/// consumers. New proof consumers should use the fallible, sorted API instead.
fn encode_expr_smt(expr: &Expr) -> String {
    encoding::encode(expr).map_or_else(|| "briev_encoding_error".into(), |term| term.text)
}

/// 2026-07-23: Build an SMT-LIB query to prove a contract condition.
/// Declares free variables for `#Self` and all named params,
/// then asserts `(not condition)` — proving UNSAT means the
/// condition always holds.
pub fn build_contract_query(expr: &Expr, params: &[(String, Type)]) -> String {
    let Some(condition) = encoding::expect(expr, encoding::Sort::Boolean) else {
        return encoding_diagnostic();
    };
    let mut names = std::collections::BTreeSet::from(["#Self".to_owned()]);
    names.extend(params.iter().map(|(name, _)| name.clone()));
    let mut free = std::collections::BTreeSet::new();
    collect_free_idents(expr, &mut free);
    let declared: std::collections::BTreeSet<_> =
        names.iter().map(|name| sanitize_name(name)).collect();
    if free
        .iter()
        .any(|name| !declared.contains(&sanitize_name(name)))
    {
        return encoding_diagnostic();
    }
    let mut q = query_header(&names);

    // Assert the negation: if UNSAT, the condition always holds
    q.push_str("(assert (not ");
    q.push_str(&condition);
    q.push_str("))\n");

    q.push_str("(check-sat)\n");
    q
}

/// Build an SMT-LIB verification-condition query for ONE symbolic path.
///
/// The path's facts (precondition plus each guard with its taken polarity) and
/// the NEGATION of the postcondition are asserted together, so `Unsat` means the
/// facts imply the postcondition — the body establishes its contract on this
/// path. Free identifiers are declared as `(_ BitVec 64)` constants, which is
/// exactly the universal quantification weakest-precondition reasoning needs, so
/// callers need no parameter list to use it correctly.
pub fn build_vc_query(facts: &[Expr], post: &Expr) -> String {
    // Sorted so the emitted query is byte-identical across processes.
    let mut free = std::collections::BTreeSet::new();
    for fact in facts {
        collect_free_idents(fact, &mut free);
    }
    collect_free_idents(post, &mut free);

    let Some(goal) = encoding::expect(post, encoding::Sort::Boolean) else {
        return encoding_diagnostic();
    };
    let Some(encoded_facts): Option<Vec<String>> = facts
        .iter()
        .map(|fact| encoding::expect(fact, encoding::Sort::Boolean))
        .collect()
    else {
        return encoding_diagnostic();
    };
    let mut q = query_header(&free);

    // Assumptions first, then the negated goal.
    for fact in encoded_facts {
        q.push_str("(assert ");
        q.push_str(&fact);
        q.push_str(")\n");
    }
    q.push_str("(assert (not ");
    q.push_str(&goal);
    q.push_str("))\n");
    q.push_str("(check-sat)\n");
    q.push_str("(get-model)\n");
    q
}

/// Collect the free identifiers an expression mentions (sorted, deterministic).
fn collect_free_idents(expr: &Expr, out: &mut std::collections::BTreeSet<String>) {
    match expr {
        Expr::Identifier(name) => {
            out.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_free_idents(l, out);
            collect_free_idents(r, out);
        }
        Expr::UnaryOp(_, e) => collect_free_idents(e, out),
        _ => {}
    }
}

/// Sanitize a Briev identifier for SMT-LIB.
/// Preserve the intentional self aliases; encode all other UTF-8 names in a disjoint namespace.
fn sanitize_name(name: &str) -> String {
    symbols::encode(name)
}

/// Generated identifier shared by external SMT proof binders and term encoding.
pub fn encode_identifier_for_proof(name: &str) -> String {
    symbols::encode(name)
}

/// Encode only supported word-valued expressions for protocol proof bodies.
pub fn try_encode_word_for_proof(expr: &Expr) -> Option<String> {
    encoding::expect(expr, encoding::Sort::Word)
}

pub(crate) fn supported_boolean(expr: &Expr) -> bool {
    encoding::expect(expr, encoding::Sort::Boolean).is_some()
}

/// Fast word-model postconditions may contain directly tracked fields; this
/// preserves their structural proofs without pretending to encode them in SMT.
pub(crate) fn supported_word_postcondition(expr: &Expr) -> bool {
    encoding::postcondition_sort(expr) == Some(encoding::Sort::Boolean)
}

/// 2026-07-23: Quick check if z3 is on PATH by spawning with --version.
/// Also checks the managed binary directory (~/.local/share/briev-compiler/bin/).
pub fn is_z3_available() -> bool {
    z3_bin().is_some()
}

/// Resolve the z3 binary: PATH first, then the managed install directory.
///
/// One resolution rule for every call site. `is_z3_available` accepting a
/// location that the query path cannot reach is how a solver install turns into
/// silent `Unknown` verdicts instead of proofs.
fn z3_bin() -> Option<std::ffi::OsString> {
    // PATH: keep the bare name so the OS resolves it (preserves `which z3`).
    if std::process::Command::new("z3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut c| c.wait())
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Some(std::ffi::OsString::from("z3"));
    }
    let managed = managed_z3_path()?;
    managed.exists().then(|| managed.into_os_string())
}

/// Path to z3 in the managed binary directory.
fn managed_z3_path() -> Option<std::path::PathBuf> {
    let base = if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        std::path::PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").ok()?;
        std::path::PathBuf::from(home).join(".local").join("share")
    };
    Some(base.join("briev-compiler").join("bin").join("z3"))
}

/// 2026-07-23: Public SMT encoding for proof use.
/// Same as encode_expr_smt but exposed for external callers
/// (protocol_graph round-trip verification needs it).
pub fn encode_expr_smt_for_proof(expr: &Expr) -> String {
    encode_expr_smt(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reset the cache and atomic counters for test isolation.
    /// Z3_VERSION cannot be reset (OnceLock is set-once per process), so tests
    /// that depend on Z3 being absent must check `is_z3_available()` and skip
    /// when Z3 is installed.
    #[cfg(test)]
    pub fn reset_cache_for_test() {
        if let Some(cache) = PROOF_CACHE.get() {
            if let Ok(mut guard) = cache.lock() {
                guard.clear();
            }
        }
        {
            let mut guard = STATS.lock().unwrap();
            guard.0 = 0;
            guard.1 = 0;
        }
        CACHE_CAP_WARNED.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn test_encode_bool() {
        let _guard = cache_test_guard();
        reset_cache_for_test();
        assert_eq!(encode_expr_smt(&Expr::Bool(true)), "(= #b1 #b1)");
        assert_eq!(encode_expr_smt(&Expr::Bool(false)), "(= #b0 #b1)");
    }

    #[test]
    fn test_encode_eq() {
        let _guard = cache_test_guard();
        reset_cache_for_test();
        let expr = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(Expr::Decimal(42)),
            Box::new(Expr::Decimal(42)),
        );
        let smt = encode_expr_smt(&expr);
        assert!(smt.starts_with("(= "));
        assert!(smt.contains("#x000000000000002a"));
    }

    #[test]
    fn test_encode_and() {
        let _guard = cache_test_guard();
        reset_cache_for_test();
        let expr = Expr::BinaryOp(
            crate::ast::BinaryOpKind::And,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Bool(true)),
        );
        let smt = encode_expr_smt(&expr);
        assert!(smt.starts_with("(and "));
    }

    #[test]
    fn test_build_query() {
        let _guard = cache_test_guard();
        reset_cache_for_test();
        let query = build_smt_query(&Expr::Bool(true));
        assert!(query.contains("set-logic QF_BV"));
        assert!(query.contains("check-sat"));
    }

    #[test]
    fn test_cache_hit_avoids_spawn() {
        let _guard = cache_test_guard();
        // Requires Z3 to be present; otherwise there's nothing to cache.
        if !is_z3_available() {
            return;
        }
        reset_cache_for_test();

        // A valid unsatisfiable query: "false" assertion is always unsat.
        let q = "(set-logic QF_BV)(assert false)(check-sat)";

        let (before_h, before_m) = {
            let g = STATS.lock().unwrap();
            (g.0, g.1)
        };

        // First call: cache miss.
        let r1 = prove_smt_formula(q, 1000);
        let m1 = STATS.lock().unwrap().1;
        let h1 = STATS.lock().unwrap().0;
        assert_eq!(
            m1 - before_m,
            1,
            "first call: misses should increment by 1 (was {before_m} -> {m1})"
        );
        assert_eq!(
            h1 - before_h,
            0,
            "first call: hits should not change (was {before_h} -> {h1})"
        );

        // Second call: cache hit.
        let r2 = prove_smt_formula(q, 1000);
        assert_eq!(r1, r2);
        let h2 = STATS.lock().unwrap().0;
        let m2 = STATS.lock().unwrap().1;
        assert_eq!(
            h2 - h1,
            1,
            "second call: hits should increment by 1 (was {h1} -> {h2})"
        );
        assert_eq!(
            m2, m1,
            "second call: misses should not change (was {m1} -> {m2})"
        );
    }

    #[test]
    fn test_cache_miss_for_different_queries() {
        let _guard = cache_test_guard();
        if !is_z3_available() {
            return;
        }
        reset_cache_for_test();

        let q1 = "(set-logic QF_BV)(assert false)(check-sat)";
        let q2 = "(set-logic QF_BV)(assert true)(check-sat)";

        let before_m = STATS.lock().unwrap().1;
        let before_h = STATS.lock().unwrap().0;
        prove_smt_formula(q1, 1000);
        let m1 = STATS.lock().unwrap().1;
        let h1 = STATS.lock().unwrap().0;
        assert_eq!(
            m1 - before_m,
            1,
            "first query: misses +1 (was {before_m} -> {m1})"
        );
        assert_eq!(
            h1 - before_h,
            0,
            "first query: no hits (was {before_h} -> {h1})"
        );

        prove_smt_formula(q2, 1000);
        let m2 = STATS.lock().unwrap().1;
        let h2 = STATS.lock().unwrap().0;
        assert_eq!(m2 - m1, 1, "different query: misses +1 (was {m1} -> {m2})");
        assert_eq!(h2 - h1, 0, "different query: no hits (was {h1} -> {h2})");
    }

    #[test]
    fn test_prove_smt_caches() {
        let _guard = cache_test_guard();
        // prove_smt (AST-based) also caches by its SMT-LIB string output.
        if !is_z3_available() {
            return;
        }
        reset_cache_for_test();

        let expr = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(Expr::Decimal(1)),
            Box::new(Expr::Decimal(1)),
        );

        let before_m = STATS.lock().unwrap().1;
        let r1 = prove_smt(&expr, &[]);
        assert_eq!(
            STATS.lock().unwrap().1,
            before_m + 1,
            "first prove_smt call should be a miss"
        );

        let r2 = prove_smt(&expr, &[]);
        assert_eq!(r1, r2);
        assert_eq!(
            STATS.lock().unwrap().1,
            before_m + 1,
            "second call should NOT add another miss"
        );
        assert_eq!(STATS.lock().unwrap().0, 1, "second call should be a hit");
    }

    #[test]
    fn test_cache_returns_unknown_when_z3_absent() {
        let _guard = cache_test_guard();
        // This test verifies the cache is NOT used when Z3 is absent.
        // If Z3 is installed on this machine, skip — we cannot test "Z3 absent"
        // in that environment without mocking z3_bin().
        if is_z3_available() {
            // When Z3 IS available: verify the cache still works normally.
            reset_cache_for_test();
            let q = "(set-logic QF_BV)(assert false)(check-sat)";
            let r = prove_smt_formula(q, 1000);
            // With Z3: should NOT be Unknown (either Sat or Unsat, never Unknown
            // for a well-formed query on an available solver).
            assert!(
                !matches!(r, SmtResult::Unknown),
                "Z3 is available, expected a real result, got {r:?}"
            );
            return;
        }

        // Z3 not available: verify Unknown is returned and cache is untouched.
        reset_cache_for_test();
        let result = prove_smt_formula("(check-sat)", 1000);
        assert!(
            matches!(result, SmtResult::Unknown),
            "without Z3, expected Unknown, got {result:?}"
        );
        // No spawn → no cache activity.
        assert_eq!(STATS.lock().unwrap().0, 0);
        assert_eq!(STATS.lock().unwrap().1, 0);
    }
}
