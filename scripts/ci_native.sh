#!/usr/bin/env bash
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
# 2026-11-15: each independent native gate owns one CI matrix job and log.
# Keep the existing commands; retire this dispatch if each gate becomes a
# reusable workflow with equivalent independent failure/artifact handling.
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-}" in
    effect-shape)
        bash benchmarks/effect_shape_guard.sh --quick
        ;;
    targets)
        ./target/release/brievc targets
        ./target/release/brievc build tests/fixtures/minimal.bv --llvm --triple aarch64-unknown-linux-gnu --out /tmp/tier2-aarch64
        ./target/release/brievc build tests/fixtures/minimal.bv --llvm --triple riscv64-unknown-none --out /tmp/tier2-riscv64
        if ./target/release/brievc build tests/fixtures/minimal.bv --triple definitely-not-a-target --out /tmp/tier2-unsupported; then
            echo "unsupported target unexpectedly succeeded" >&2
            exit 1
        fi
        ;;
    cross-target)
        # 2026-11-24 (BUGS.md aarch64 brk): the `targets` case above compiles
        # ONE trivial fixture per triple, which cannot see an emission defect
        # that only fires on a construct the corpus exercises (arenas/syscalls).
        # This case sweeps the whole self-calibrating corpus instead; retire it
        # into `targets` once that case is corpus-wide.
        #
        # 2026-11-25 (wasm32 supported-target plan P0): wasm32 is a SHIPPED
        # target — the web build of a `.rbv` program — so it is gated here, not
        # inspected by hand. The sweep now also covers every `.rbv` program,
        # which it did not before, so the web path is IR-verified against its own
        # target on every PR. BRIEV_CROSS_TARGET_CHECK=1 additionally makes the
        # baseline test assert that config/cross-target-baseline.toml matches
        # reality, so a fixed bug forces the recorded number down.
        bash scripts/cross_target_ir.sh aarch64-unknown-linux-gnu wasm32-unknown-wasi
        # The cheap half of the ratchet (record integrity: every shipped triple
        # declared, no silent raise, the web path still in the corpus). The
        # expensive half — measured count must EQUAL the record — is already
        # enforced by the script above, which fails on a stale baseline, so this
        # deliberately does NOT set BRIEV_CROSS_TARGET_CHECK (that would sweep
        # twice for the same answer).
        cargo test --release --test cross_target_baseline
        ;;
    accel)
        bash benchmarks/accel_reactor_nonhang.sh
        ;;
    determinism)
        bash benchmarks/ll_determinism_check.sh --quick
        ;;
    parity)
        bash benchmarks/parity/run.sh cast_cases float_cases float_fmt_cases int_cases parse_cases string_cases
        ;;
    *)
        echo "usage: scripts/ci_native.sh effect-shape|targets|cross-target|accel|determinism|parity" >&2
        exit 2
        ;;
esac
