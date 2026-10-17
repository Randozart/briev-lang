// ── Compiler-in-Briev: Briev passes loaded through the GLUE C ABI ─────
// 2026-08-04 (plan 2026-08-04-compiler-in-briev-dogfood-ffi, P3+P5): Briev
// passes (lib/compiler/needs_state.bv, lib/compiler/soa_reorder.bv) are
// compiled by brievc (build.rs) into target/compiler-in-briev/*.so and loaded
// HERE via dlopen — the same way a host language calls a Briev bridge. Each
// pass has a Rust reference that runs when its library is absent (first build
// / no prebuilt brievc). Transition tests assert the Briev result equals the
// reference.

use std::ffi::{c_char, c_int, c_void, CString};

use once_cell::sync::OnceCell;

use crate::ast::TopLevel;

/// A dlopen'd Briev pass: `compute(state, proj) -> i64`. The i64's MEANING is
/// pass-specific (needs_state: the bitmask; soa_reorder: the address of a
/// `[total][idx0]...` permutation buffer).
struct LoadedPass {
    _handle: *mut c_void,
    compute: unsafe extern "C" fn(state: *const c_void, proj: *const c_char) -> i64,
    init_state: unsafe extern "C" fn() -> i64,
}

// The pass functions are pure over the projection string; the handle is a
// shared immutable library. The fn pointers are thread-safe to call.
unsafe impl Send for LoadedPass {}
unsafe impl Sync for LoadedPass {}

const RTLD_NOW: c_int = 2;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

impl LoadedPass {
    /// Load a pass library. `path` is the compiled-time value of the
    /// cargo:rustc-env var set by build.rs (option_env!, since rustc-env is
    /// not a runtime var); `compute_symbol` is the pass's export name. None =
    /// not built (first build) or not loadable — the caller falls back to Rust.
    fn load(path: Option<&'static str>, compute_symbol: &str) -> Option<LoadedPass> {
        let path = path?.to_string();
        if path.is_empty() {
            return None;
        }
        let c_path = CString::new(path).ok()?;
        // SAFETY: dlopen with a NUL-terminated path, RTLD_NOW.
        let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            return None;
        }
        let compute = unsafe {
            dlsym(handle, CString::new(compute_symbol).ok()?.as_ptr() as *const c_char)
        };
        let init_state = unsafe {
            dlsym(handle, b"__briev_init_state\0".as_ptr() as *const c_char)
        };
        if compute.is_null() || init_state.is_null() {
            return None;
        }
        // SAFETY: the symbols were resolved to the declared C signatures.
        Some(LoadedPass {
            _handle: handle,
            compute: unsafe { std::mem::transmute(compute) },
            init_state: unsafe { std::mem::transmute(init_state) },
        })
    }

    /// Run the pass over a serialized projection and return the raw i64 result.
    fn compute(&self, proj: &str) -> i64 {
        let c_proj = match CString::new(proj) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        // SAFETY: the pass functions are pure; init_state gives the ABI state
        // handle (the pass never dereferences it), compute reads the C string.
        let state = unsafe { (self.init_state)() };
        unsafe { (self.compute)(state as *const c_void, c_proj.as_ptr()) }
    }
}

// ── needs_state pass (P3) ──────────────────────────────────────────────

static NEEDS_STATE_PASS: OnceCell<Option<LoadedPass>> = OnceCell::new();

fn needs_state_pass() -> Option<&'static LoadedPass> {
    NEEDS_STATE_PASS
        .get_or_init(|| LoadedPass::load(option_env!("BRIEV_COMPILER_IN_BRIEV_SO"), "needs_state_compute"))
        .as_ref()
}

/// Compute the needs_state map for a program, preferring the Briev pass when
/// its library is present. Falls back to the Rust reference (export_abi.rs).
/// The two must agree — asserted by tests/c_driver_needs_state.rs.
pub fn compute_export_needs_state(items: &[TopLevel], plain: &std::collections::HashSet<String>) -> std::collections::HashMap<String, bool> {
    if let Some(pass) = needs_state_pass() {
        let proj = crate::analysis::needs_state_projection::serialize_needs_state_projection(items);
        let mask = pass.compute(&proj);
        // Reconstruct the map in the SAME order the serializer emitted the
        // export section (sorted by defn name): bit i ↔ export[i].
        let mut exports: Vec<String> = Vec::new();
        for item in items {
            if let TopLevel::Export(e) = item {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    exports.push(d.name.clone());
                }
            }
        }
        exports.sort();
        let mut map = std::collections::HashMap::new();
        for (i, name) in exports.into_iter().enumerate() {
            map.insert(name, (mask >> i) & 1 == 1);
        }
        return map;
    }
    // Fallback: the Rust reference (also the source of truth on first build).
    crate::analysis::export_abi::compute_export_needs_state(items, plain)
}

// ── soa_reorder pass (P5) ──────────────────────────────────────────────

static SOA_REORDER_PASS: OnceCell<Option<LoadedPass>> = OnceCell::new();

fn soa_reorder_pass() -> Option<&'static LoadedPass> {
    SOA_REORDER_PASS
        .get_or_init(|| LoadedPass::load(option_env!("BRIEV_COMPILER_IN_BRIEV_SOA_SO"), "soa_reorder_compute"))
        .as_ref()
}

/// Compute the AoS → SoA item permutation via the Briev pass, when its
/// library is present. Returns None when the pass is unavailable (caller falls
/// back to the Rust reorder_fields). The permutation is the pass's Malloc'd
/// `[total][idx0]...[idx_{N-1}]` buffer; read and freed here.
pub fn compute_soa_permutation(items: &[TopLevel]) -> Option<Vec<usize>> {
    let pass = soa_reorder_pass()?;
    let proj = crate::analysis::soa_projection::serialize_soa_projection(items);
    let addr = pass.compute(&proj);
    if addr == 0 {
        return None;
    }
    let buf = addr as *const i64;
    // SAFETY: the pass wrote [total][idx...] to a Malloc'd buffer.
    let total = unsafe { *buf };
    if total < 0 || total as usize != items.len() {
        return None;
    }
    let mut perm = Vec::with_capacity(total as usize);
    for i in 0..total as usize {
        // SAFETY: the pass fills exactly `total` indices.
        perm.push(unsafe { *buf.add(1 + i) } as usize);
    }
    // The pass allocated the buffer with Malloc# (= malloc); free it.
    unsafe { rt_free(buf as *mut c_void) }
    Some(perm)
}

#[link(name = "c")]
unsafe extern "C" {
    fn free(ptr: *mut c_void);
}

fn rt_free(ptr: *mut c_void) {
    unsafe { free(ptr) }
}

// ── diagnostics ────────────────────────────────────────────────────────

/// Whether the compiled Briev needs_state pass library is available to load.
pub fn pass_available() -> bool {
    needs_state_pass().is_some()
}

/// Whether the compiled Briev soa_reorder pass library is available to load.
pub fn soa_pass_available() -> bool {
    soa_reorder_pass().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_let(name: &str, expr: crate::ast::Expr) -> TopLevel {
        TopLevel::Statement(Box::new(crate::ast::Statement::Let {
            names: vec![],
            name: name.to_string(),
            ty: Some(crate::ast::Type::float()),
            expr: Some(expr),
            modifiers: vec![],
        }))
    }

    /// The dlopen path must actually run (not silently fall back). Builds the
    /// bridges, runs BOTH the Briev pass and the Rust reference, and requires
    /// equality — and that the Briev path was used when the .so was produced.
    #[test]
    fn briev_pass_matches_reference_when_loaded() {
        let root = env!("CARGO_MANIFEST_DIR");
        let corpus = [
            "examples/glue-host/boundary.bv",
            "examples/glue-host/node_bridge.bv",
            "examples/glue-host/cancel.bv",
            "examples/glue-host/rank.bv",
            "examples/glue-host/bench.bv",
        ];
        let mut used_briev = false;
        for rel in corpus {
            let src = format!("{}/{}", root, rel);
            let source = std::fs::read_to_string(&src).unwrap();
            let (items, _) = crate::library::parse_and_check(&src, &source).unwrap();
            let plain = crate::analysis::storage_class::classify_plain_lets(&items, None, &std::collections::HashSet::new());
            let briev = compute_export_needs_state(&items, &plain);
            let reference = crate::analysis::export_abi::compute_export_needs_state(&items, &plain);
            assert_eq!(briev, reference, "Briev pass diverged from reference for {}", rel);
            if needs_state_pass().is_some() {
                used_briev = true;
            }
        }
        if option_env!("BRIEV_COMPILER_IN_BRIEV_SO").map_or(false, |p| !p.is_empty()) {
            assert!(used_briev, "pass library was built but never used");
        }
    }

    /// The soa_reorder pass (when loaded) must reproduce reorder_fields on a
    /// small AoS program: bx0, by0, bx1 → bx0, bx1, by0. Group "bx" is safe
    /// (2 members, no sibling refs); "by" is singleton. Expected permutation:
    /// [0, 2, 1] (bx0, bx1, by0).
    #[test]
    fn soa_pass_matches_reorder_fields() {
        let bx0 = state_let("bx0", crate::ast::Expr::Decimal(1));
        let by0 = state_let("by0", crate::ast::Expr::Decimal(2));
        let bx1 = state_let(
            "bx1",
            crate::ast::Expr::BinaryOp(
                crate::ast::BinaryOpKind::Add,
                Box::new(crate::ast::Expr::Decimal(1)),
                Box::new(crate::ast::Expr::Decimal(1)),
            ),
        );
        let items = vec![bx0, by0, bx1];
        let reference: Vec<usize> = crate::analysis::soa_reorder::reorder_fields(&items)
            .iter()
            .map(|out| items.iter().position(|orig| name_of(orig) == name_of(out)).unwrap())
            .collect();
        assert_eq!(reference, vec![0, 2, 1], "reference expected AoS→SoA order");
        if let Some(perm) = compute_soa_permutation(&items) {
            assert_eq!(perm, vec![0, 2, 1], "soa pass diverged from reorder_fields");
        }
        if option_env!("BRIEV_COMPILER_IN_BRIEV_SOA_SO").map_or(false, |p| !p.is_empty()) {
            assert!(soa_pass_available(), "soa pass library was built but never used");
        }
    }

    fn name_of(item: &TopLevel) -> String {
        match item {
            TopLevel::Statement(s) => match s.as_ref() {
                crate::ast::Statement::Let { name, .. } => name.clone(),
                _ => "stmt".to_string(),
            },
            TopLevel::Constant(c) => c.name.clone(),
            TopLevel::Transaction(t) => t.name.clone(),
            _ => "unknown".to_string(),
        }
    }
}
