# Target support contract

**Status:** v0.1.0-preview contract, 2026-10-28
**Source of truth:** `config/targets.dbvl`, `config/protocols.dbvl`,
`config/isr-targets.dbvl`, and `brievc targets`.

`brievc targets` prints the baked extension→backend table:

```text
EXTENSION  BACKEND   TRIPLE                    PLUGINS
.abv       spirv     (profile default)         prelude,execute-many
.bad       bad       (host)                    prelude
.bv        llvm      x86_64-unknown-linux-gnu  prelude-native,env,print,inline-frgn,entry,script,execute-many
.ebv       electronics (profile default)       prelude-electronics
.rbv       webstack  wasm32-unknown-wasi       prelude-web
.sbv       circt     (profile default)         prelude-hw
```

## Tier 1 — release blocking

| Surface | Extension | Backend | Target | Evidence |
|---|---|---|---|---|
| Native Linux | `.bv` | LLVM | `x86_64-unknown-linux-gnu` | `cargo test --all`, daily corpus, benchmarks |
| Browser | `.rbv` | webstack | `wasm32-unknown-wasi` | `browser_pending_peer.mjs`, I11 runner, determinism |

Tier 1 must build, run, typecheck, and pass the release gates on every
`v0.1.0-preview` artifact. The compiler may not silently assume x86_64 for a
Tier 1 wasm target or vice versa.

## Tier 2 — compile-only

These targets have a backend/config path but are not release-blocking:

- `aarch64` / `arm64` LLVM triples;
- `riscv64` and `thumbv7m` freestanding LLVM triples;
- `wasm64`;
- `.bad` target triples through `--target`.

Tier 2 means “the compiler emits a module/object and reports toolchain errors
cleanly.” It does not mean a runtime, libc, or hardware test exists.

## Experimental

- `.ebv` electronics.
- `.sbv` CIRCT/silicon.
- `.abv` SPIR-V kernels.
- GPU PTX/offload.
- Any target not listed in `config/targets.dbvl` or the target tuning table.

Experimental surfaces may change without a deprecation window and do not block
the preview.

## Rules

1. A target is supported only if it is in the table above and the relevant
   gates pass on the release commit.
2. `--triple <unknown>` must fail with a toolchain/user diagnostic; a compiler
   panic is a bug.
3. Adding a Tier 1 target requires a runnable artifact, a CI job, and a
   correctness gate.
4. Benchmarks and docs must state the target triple; “Briev is fast” without a
   target is not evidence.
5. The support table is data-driven. Do not add target-name match arms to the
   backend; extend `config/targets.dbvl` or the frontend-derived target
   settings.

## Commands

```bash
brievc targets
brievc build hello.bv                       # Tier 1 native
brievc build page.rbv --split               # Tier 1 wasm32
brievc build kernel.b.bv --triple riscv64-unknown-none
cargo test --all
bash benchmarks/toolchain_matrix.sh         # needs MovieLens u.data
```
