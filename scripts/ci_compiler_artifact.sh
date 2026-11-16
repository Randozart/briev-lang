#!/usr/bin/env bash
# ===----------------------------------------------------------------------=== #
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# ===----------------------------------------------------------------------=== #
# 2026-11-15: native consumers need both runtime archives as well as brievc.
# Share one same-revision build with mode/hash checks; retire when CI consumes
# an equivalent qualified package with the same resource-root layout.
set -euo pipefail
cd "$(dirname "$0")/.."

files=(target/release/brievc
       target/compiler-in-briv/libbriev_accel_rt.a
       target/compiler-in-briv/libbriev_gpu_rt.a)
metadata=target/ci-compiler
archive=target/ci-compiler.tar.gz
revision=$(git rev-parse --verify HEAD)

check_inputs() {
    for file in "${files[@]}"; do
        if [[ ! -s "$file" ]]; then
            echo "compiler artifact is missing $file; rebuild the release compiler" >&2
            return 1
        fi
    done
    if [[ ! -x "${files[0]}" ]]; then
        echo "compiler artifact is not executable; restore its executable mode" >&2
        return 1
    fi
}

case "${1:-}" in
    pack)
        check_inputs
        mkdir -p "$metadata"
        printf '%s\n' "$revision" > "$metadata/revision"
        sha256sum "${files[@]}" > "$metadata/SHA256SUMS"
        tar -czf "$archive" "${files[@]}" "$metadata/revision" "$metadata/SHA256SUMS"
        ;;
    unpack)
        tar -xzf "$archive" "${files[@]}" "$metadata/revision" "$metadata/SHA256SUMS"
        if [[ "$(cat "$metadata/revision")" != "$revision" ]]; then
            echo "compiler artifact revision differs from this checkout; use this run's compiler artifact" >&2
            exit 1
        fi
        check_inputs
        sha256sum "${files[@]}" > "$metadata/ACTUALSUMS"
        if ! cmp -s "$metadata/SHA256SUMS" "$metadata/ACTUALSUMS"; then
            echo "compiler artifact checksum mismatch; rebuild or download the artifact again" >&2
            exit 1
        fi
        ;;
    *)
        echo "usage: scripts/ci_compiler_artifact.sh pack|unpack" >&2
        exit 2
        ;;
esac
