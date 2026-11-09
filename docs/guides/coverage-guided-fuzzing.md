# Coverage-guided fuzzing (`cargo-fuzz`)

The deterministic transition harness (`docs/guides/deterministic-testing.md`)
compiles and runs real native code, which is precise but slow. `cargo-fuzz`
complements it: it runs the compiler **in-process**, instruments every branch,
and mutates inputs to reach new paths automatically. Use both.

| Harness | What it exercises | Detects |
|---|---|---|
| `cargo-fuzz` targets | Lexer, parser, interpreter, semantic model | Panics/hangs, and (for the model targets) wrong interpreter results |
| `deterministic_transitions` | Real LLVM build + link + native run | Native miscompiles, crashes, interpreter/backend disagreement |

## Why a separate `fuzz/` workspace

`fuzz/` is its own Cargo workspace with `briev-compiler` as a path dependency.
It needs nightly Rust and links libFuzzer; keeping it separate means production
builds (`cargo build`, `cargo test --lib`) are unaffected and never pull
libFuzzer. This is the standard `cargo-fuzz` layout.

## Targets

| Target | Input model | Pass/fail |
|---|---|---|
| `frontend` | Raw bytes → lex → parse | Panics and hangs are failures; parse errors are normal |
| `arithmetic` | Bytes → generated integer program → interpreter vs model | Any disagreement fails |
| `interpreter` | As `arithmetic`, plus tuple projection/destructuring | Reproduces **known OPEN** tuple defects |

`arithmetic` deliberately avoids tuples so it stays green and validates the
oracle. `interpreter` includes tuples and currently *fails immediately* — that
is the point: it independently rediscovers the open defects rather than being
told about them.

## Setup (once)

```bash
rustup toolchain install nightly
cargo install cargo-fuzz --version 0.13.2 --locked
```

The fuzz crate pins `libfuzzer-sys = "=0.4.13"`. Use the nightly pinned in
`rust-toolchain.toml`-style discipline for reproducible replay; a different
nightly can change codegen and coverage.

**Always pass `--target` explicitly in automation.** `cargo-fuzz`'s default
target is baked into the binary at build time (`current_platform`). A prebuilt
cargo-fuzz (e.g. via `taiki-e/install-action`) can default to `*-musl`, where
ASan fails with *"sanitizer is incompatible with statically linked libc"*.
Derive the host triple instead:

```bash
HOST=$(rustc -vV | sed -n 's/^host: //p')
cargo +nightly fuzz run --target "$HOST" arithmetic fuzz/corpus/arithmetic -- -max_total_time=120
```

## Run

```bash
# Build every target
cargo +nightly fuzz build

# Seed the working corpora (copied, never overwritten)
mkdir -p fuzz/corpus/frontend fuzz/corpus/arithmetic fuzz/corpus/interpreter
cp -n fuzz/seeds/frontend/*    fuzz/corpus/frontend/    || true
cp -n fuzz/seeds/arithmetic/*  fuzz/corpus/arithmetic/  || true
cp -n fuzz/seeds/interpreter/* fuzz/corpus/interpreter/ || true

# Bounded smoke (what CI runs on a PR)
cargo +nightly fuzz run frontend    fuzz/corpus/frontend    -- -max_total_time=30 -print_final_stats=1
cargo +nightly fuzz run arithmetic  fuzz/corpus/arithmetic  -- -max_total_time=60 -print_final_stats=1

# Nightly discovery (all targets; interpreter is expected to find the tuple bug)
cargo +nightly fuzz run interpreter fuzz/corpus/interpreter -- -max_total_time=300 -fork=$(nproc) -print_final_stats=1
```

`-print_final_stats=1` reports executed inputs and coverage — quote those
numbers, not a wall-clock impression.

## Triage a finding

libFuzzer writes each crashing input to `fuzz/artifacts/<target>/crash-*`.

```bash
# 1. Replay the exact input and read the failure
cargo +nightly fuzz run <target> fuzz/artifacts/<target>/crash-<hash>

# 2. Minimize to the smallest failing input
cargo +nightly fuzz tmin <target> fuzz/artifacts/<target>/crash-<hash>

# 3. Copy the minimized input somewhere durable and reviewable
cp fuzz/artifacts/<target>/minimized-* fuzz/cases/<name>.input

# 4. Confirm the same input fails via the deterministic harness, if it is a
#    native-code bug, then open an issue with the input + metadata.
```

Hand-crafted reproducers live in `fuzz/cases/` and are run explicitly:

```bash
cargo +nightly fuzz run interpreter fuzz/cases/tuple-projection.input
```

## Guardrails

- **Never weaken the oracle to make a target green.** A disagreement is a real
  finding until proven otherwise. If the oracle itself is wrong, fix the oracle
  and say so in the PR.
- **No `catch_unwind` in the targets.** libFuzzer must see genuine panics.
- **Rediscovery vs new.** Check `BUGS.md` before claiming a new defect; label
  rediscoveries as such.
- **Bounds are explicit.** The model bounds state and uses nonzero divisors so
  disagreements are actionable; when widening the model, widen the bounds and
  tests together.
- **Artifacts are not tests.** A raw crashing input is evidence; a minimized,
  committed, replaying case is the regression.
