// ── Shared helper for the c_driver integration-test family ────────────────
// 2026-11-22: each c_driver suite builds into a fixed dir under
// `std::env::temp_dir()` and removes it at the end. Under `cargo test`
// (one process) that is safe. But the project's gate runner is
// cargo-nextest, which executes every test in its own process, and a full
// gate is itself re-run concurrently by other agent sessions / CI. Two
// processes that build into the SAME named dir (same test name) can
// `remove_dir_all` the other's output between writing the `.ll` and
// linking it. Symptom on CI (run under `-j 4`):
//
//   llc: error: .../briev_c_driver_test/pp-types.opt.ll:
//         Could not open input file
//   Fatal error: can't create .../briev_cancel_test/cancel.briev_rt.o
//
// The fix mirrors `tests/saltpetre_gemm_baseline.rs` (trunk a53db50da): a
// cross-process claim via atomic `create_dir` on a sibling `.lock` dir,
// released on drop, with a 300 s deadline so a leaked claim fails loudly
// instead of hanging. The out dir itself is still created and removed by
// the owner; the claim only serialises the whole build/run/teardown window
// across processes that share the same dir name.

use std::path::PathBuf;

/// A held cross-process claim on a temp-dir sibling lock dir. Released on
/// drop. Bind it as `let _claim = …` so it lives to the end of the test.
pub struct DirClaim {
    dir: PathBuf,
}

impl Drop for DirClaim {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Claim `out_dir` (by its sibling `.out_dir` lock) via atomic `create_dir`,
/// waiting while another process holds it.
///
/// The deadline turns a leaked claim into a loud failure, not a hang.
pub fn claim_out_dir(out_dir: &std::path::Path) -> DirClaim {
    let lock_dir = out_dir.with_file_name(format!(
        ".{}",
        out_dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if std::fs::create_dir(&lock_dir).is_ok() {
            return DirClaim { dir: lock_dir };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "c_driver lock {} held for over 300 s; remove it if no build is running",
            lock_dir.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
