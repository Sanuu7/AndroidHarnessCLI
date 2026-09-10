#!/usr/bin/env bash
# Build the static binaries that install.sh serves and refresh dist/.
#
#   ./scripts/release.sh
#
# dist/ is committed on purpose: it is what the one-line installer downloads,
# so a normal HTTPS fetch is enough and no release assets are needed.

set -euo pipefail
cd "$(dirname "$0")/.."

for target in aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  rustup target add "$target" >/dev/null 2>&1 || true
  echo ">> $target"
  cargo build --release --target "$target"
done

mkdir -p dist
cp target/aarch64-unknown-linux-musl/release/harness dist/harness-aarch64
cp target/x86_64-unknown-linux-musl/release/harness dist/harness-x86_64

echo ">> dist/"
ls -lh dist | tail -n +2
