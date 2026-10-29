#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
# Build a reproducible preview artifact tree and compressed archive.
# The archive contains the compiler, stdlib/config data, release notes,
# license text, and detached SHA-256 sums.
#
# Usage: scripts/package_release.sh <vX.Y.Z-suffix> [output-dir]
# Example: scripts/package_release.sh v0.1.0-preview target/release-package
set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:-v0.1.0-preview}"
output="${2:-target/release-package}"
case "$version" in
    v*) ;;
    *) echo "usage: $0 <vX.Y.Z-suffix> [output-dir]" >&2; exit 2 ;;
esac
name="briev-${version}"
staging="$output/$name"

cargo build --release --bin brievc

rm -rf "$staging"
mkdir -p "$staging/lib" "$staging/config"
install -m 0755 target/release/brievc "$staging/brievc"
# Ship every stdlib/plugin/runtime directory; the prelude and linker need more
# than lib/std. Test scratch files in lib/ are intentionally excluded.
while IFS= read -r -d '' dir; do
    cp -R "$dir" "$staging/lib/$(basename "$dir")"
done < <(find lib -mindepth 1 -maxdepth 1 -type d -print0)
# The compiler probes config/ relative to its working directory. Ship the full
# baked policy set so a package extraction is self-contained.
cp -R config "$staging/config"
# System stage plugins are searched in CWD first, then beside the resource
# root. A self-contained package must ship them.
cp -R plugins "$staging/plugins"
# One runnable native example and one split-view example support the smoke test.
mkdir -p "$staging/examples"
cp examples/hello.bv "$staging/examples/hello.bv"
cp examples/counter.rbv "$staging/examples/counter.rbv"
cp LICENSE NOTICE README.md CHANGELOG.md RELEASE_NOTES.md "$staging/"

(
    cd "$output"
    tar --sort=name --mtime='UTC 2026-10-28 00:00:00' --owner=0 --group=0 \
        -cf - "$(basename "$staging")" | gzip -n > "$name.tar.gz"
)

(
    cd "$output"
    sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
)

echo "artifact: $output/$name.tar.gz"
sha256sum "$output/$name.tar.gz"
