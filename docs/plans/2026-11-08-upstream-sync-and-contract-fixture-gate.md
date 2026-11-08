# Upstream sync and LLVM fixture contract gate

## Scope and evidence

Fetched upstream `origin/main` at `735d1023`, fast-forwarded local main and the
layout-fix branch, and restored the pending layout changes without conflicts.
The preservation stash remains available. Updating fork/main is blocked by an
invalid GitHub credential; no remote update is claimed.

The old library gate failed 78 LLVM tests identically on base and patched trees.
`parse_bv_source` incorrectly invokes `prove_contract`, the simultaneous protocol
invariant checker, for transaction/node before-and-after conditions. Production
`src/compile.rs` instead calls `detect_tautology` for transactions and definitions.
The proof engine documents this distinction explicitly. Installing Z3 exposes
the helper mismatch even for a valid false-to-true node transition.

## Change and verification

Use the production tautology gate for both transaction and definition fixtures.
Retain the legality check: add positive before/after and negative explicit and
functional tautology regressions. Do not change authored contracts or the protocol
checker. Run the LLVM group and full library gate, then the native layout
regression. Update BUGS.md and the backend test-helper rationale in the same
change; append exact results here. This is correctness/test-infrastructure work,
with no runtime performance claim.

## Sync details

Local `main`, `origin/main`, and the fix branch now point to `735d1023`.
The fork's fetched `main` remains `4e30bff5`; HTTPS push fails because the saved
GitHub credential is invalid. An SSH attempt also fails host-key verification;
verification was not disabled. Re-authenticate with `gh auth login -h github.com`
before retrying the fast-forward push. The named preservation stash is retained
after successful application, and no source conflicts remained.

## Final verification

On integrated base `735d1023`, with Z3 4.13.3 installed:

- LLVM backend group: 304 passed, zero failed.
- Full `cargo test --locked --lib`: 3,543 passed, zero failed.
- Native nested-object regression: passed, exercising AST/HIR at O0/O3.
- Compiler build: passed without warnings.
- Full ten-program IR determinism gate: passed.
- `git diff --check`: passed.

Logs are `/tmp/opencode/contract-gate-{backend-tests,library-tests,native-tests,
build,determinism}.log`. The library gate is now green; this is not a claim that
every Cargo integration target or the multi-user application journey passed.
Praetor and Kani remain unavailable on this host. The separate application
use-after-free findings remain open. Fork push still requires valid credentials.

Authentication was subsequently restored. The fork push succeeded without force:
local `main`, `origin/main` and `fork/main` now all resolve to `735d1023`.

Praetor was subsequently installed from `Randozart/praetor-lsp` at `81f190ab`.
The changed-file comparison for `fdceb550` against `735d1023` passes: 271 baseline
diagnostics in each tree, no new/worsened findings, and reduced complexity/length
in existing functions. Full invocation and evidence are in
`docs/architecture/praetor-log.md`. Only Kani remains unavailable from the tooling
previously recorded as missing.
